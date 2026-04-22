//! SubprocessManager — spawn and watch webapp/android child processes.
//!
//! Communication protocol:
//!   stdout (child → Rust): JSON lines — step/log/progress/result/error
//!   stdin  (Rust → child): JSON lines — abort/continue/input/config (future round)
//!   stderr: unstructured debug logs
//!   exit code: 0 = success, non-0 = failure

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::Emitter;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::models::account::CliClient;
use crate::modules::account_manager::AccountManager;

// ─── Message types ──────────────────────────────────────

/// Messages from child process stdout (JSON lines).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StdoutMessage {
    Step {
        step: u32,
        total: u32,
        name: String,
        status: String,
    },
    Log {
        level: String,
        msg: String,
    },
    Progress {
        percent: u32,
    },
    Result {
        success: bool,
        data: serde_json::Value,
    },
    Error {
        code: String,
        msg: String,
        retriable: bool,
    },
    Cookies {
        cookies: serde_json::Value,
        #[serde(rename = "sessionKey")]
        session_key: Option<String>,
    },
    Meta {
        data: serde_json::Value,
    },
}

/// Subprocess type identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubprocessType {
    Webapp,
}

/// Info about a running subprocess (returned by list_running).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessInfo {
    pub task_id: String,
    pub account_id: String,
    pub subprocess_type: SubprocessType,
    pub flow: String,
}

// ─── SubprocessHandle ───────────────────────────────────

struct SubprocessHandle {
    task_id: String,
    account_id: String,
    subprocess_type: SubprocessType,
    flow: String,
    stdin: Option<tokio::process::ChildStdin>,
    pid: u32,
    generation: u64,
}

// ─── SubprocessManager ──────────────────────────────────

pub struct SubprocessManager {
    processes: Arc<DashMap<String, SubprocessHandle>>,
    generation: AtomicU64,
}

impl SubprocessManager {
    pub fn new() -> Self {
        Self {
            processes: Arc::new(DashMap::new()),
            generation: AtomicU64::new(0),
        }
    }

    /// Spawn a child process and watch its stdout in background.
    ///
    /// - Emits Tauri events for each stdout JSON line.
    /// - On "result" message, writes token data into Account.cli via AccountManager.
    pub async fn spawn_and_watch(
        &self,
        task_id: String,
        account_id: String,
        subprocess_type: SubprocessType,
        flow: String,
        command: &str,
        args: Vec<String>,
        app_handle: tauri::AppHandle,
        account_manager: Arc<AccountManager>,
    ) -> Result<(), String> {
        let mut child = tokio::process::Command::new(command)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped())
            .env("CLAUDECODE", "")
            .env("ELECTRON_RUN_AS_NODE", "")
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {}", command, e))?;

        let pid = child.id().unwrap_or(0);
        let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
        let stderr = child.stderr.take().ok_or("Failed to capture stderr")?;
        let stdin = child.stdin.take();

        // Store handle with generation + PID for cleanup on app exit
        let gen = self.generation.fetch_add(1, Ordering::SeqCst);
        self.processes.insert(
            task_id.clone(),
            SubprocessHandle {
                task_id: task_id.clone(),
                account_id: account_id.clone(),
                subprocess_type,
                flow: flow.clone(),
                stdin,
                pid,
                generation: gen,
            },
        );

        let task_id_stdout = task_id.clone();
        let account_id_stdout = account_id.clone();
        let flow_stdout = flow.clone();
        let app = app_handle.clone();
        let acct_mgr = account_manager.clone();
        let processes = self.processes.clone();

        // Background task: read stderr — write to log file + emit to frontend + collect for error
        let task_id_stderr = task_id.clone();
        let app_stderr = app_handle.clone();
        let stderr_lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let stderr_lines_clone = stderr_lines.clone();
        // Log file: ~/.claude-ultra/logs/{task_id}.log
        let log_dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".claude-ultra")
            .join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join(format!("{}.log", task_id_stderr));
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            // Open log file in append mode
            let mut log_file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .await
                .ok();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("[subprocess:{}] {}", task_id_stderr, line);
                // Append to log file + flush
                if let Some(ref mut f) = log_file {
                    use tokio::io::AsyncWriteExt;
                    let _ = f.write_all(format!("{}\n", line).as_bytes()).await;
                    let _ = f.flush().await;
                }
                // Collect last 50 lines for error reporting
                {
                    let mut buf = stderr_lines_clone.lock().unwrap();
                    buf.push(line.clone());
                    if buf.len() > 50 {
                        buf.remove(0);
                    }
                }
                // Emit to frontend as log event
                let payload = serde_json::json!({
                    "taskId": task_id_stderr,
                    "data": { "type": "log", "level": "info", "msg": line },
                });
                let _ = app_stderr.emit("subprocess://log", &payload);
            }
        });

        // Background task: read stdout JSON lines
        let stderr_lines_for_error = stderr_lines.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            let mut got_result = false;

            while let Ok(Some(line)) = lines.next_line().await {
                // Try parse as JSON
                match serde_json::from_str::<StdoutMessage>(&line) {
                    Ok(msg) => {
                        let event_type = match &msg {
                            StdoutMessage::Step { .. } => "step",
                            StdoutMessage::Log { .. } => "log",
                            StdoutMessage::Progress { .. } => "progress",
                            StdoutMessage::Result { .. } => "result",
                            StdoutMessage::Error { .. } => "error",
                            StdoutMessage::Cookies { .. } => "cookies",
                            StdoutMessage::Meta { .. } => "meta",
                        };

                        let payload = serde_json::json!({
                            "taskId": task_id_stdout,
                            "accountId": account_id_stdout,
                            "data": serde_json::to_value(&msg).unwrap_or_default(),
                        });

                        let event_name = format!("subprocess://{}", event_type);
                        let _ = app.emit(&event_name, &payload);

                        // Handle result by flow type
                        if let StdoutMessage::Result { success: true, ref data } = msg {
                            got_result = true;
                            match flow_stdout.as_str() {
                                "oauth" => {
                                    if let Err(e) = handle_oauth_result(
                                        &acct_mgr, &account_id_stdout, data,
                                    ).await {
                                        tracing::error!(
                                            "[subprocess:{}] Failed to write Account.cli: {}",
                                            task_id_stdout, e
                                        );
                                    }
                                },
                                // web-login / keepalive: handled by frontend via events, Rust does not intervene
                                _ => {}
                            }
                        }
                        if let StdoutMessage::Result { success: false, .. } = msg {
                            got_result = true;
                        }

                        // Handle error
                        if let StdoutMessage::Error { ref code, ref msg, .. } = msg {
                            tracing::warn!(
                                "[subprocess:{}] Error: {} — {}",
                                task_id_stdout,
                                code,
                                msg
                            );
                        }
                    }
                    Err(_) => {
                        // Non-JSON line — log as debug
                        tracing::debug!("[subprocess:{}] (raw) {}", task_id_stdout, line);
                    }
                }
            }

            // Child exited — wait for exit code
            let exit_status = child.wait().await;
            let exit_code = exit_status
                .as_ref()
                .map(|s| s.code().unwrap_or(-1))
                .unwrap_or(-1);

            // Clean up handle immediately so is_running returns false
            // (before emitting terminal event, prevents frontend from thinking it's still running)
            if let Some(entry) = processes.get(&task_id_stdout) {
                if entry.value().generation == gen {
                    drop(entry);
                    processes.remove(&task_id_stdout);
                }
            }

            // Append error marker to log file for non-zero exit
            // (uses existing ❌ convention so frontend colors it red)
            if exit_code != 0 {
                let log_dir = dirs::home_dir()
                    .unwrap_or_default()
                    .join(".claude-ultra")
                    .join("logs");
                let log_path = log_dir.join(format!("{}.log", task_id_stdout));
                let _ = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&log_path)
                    .and_then(|mut f| {
                        use std::io::Write;
                        writeln!(f, "❌ Process exited with error (code={})", exit_code)
                    });
            }

            if !got_result {
                if exit_code == 0 {
                    let _ = app.emit(
                        "subprocess://completed",
                        serde_json::json!({
                            "taskId": task_id_stdout,
                            "accountId": account_id_stdout,
                            "exitCode": exit_code,
                        }),
                    );
                } else {
                    // Collect stderr last lines for error context
                    let stderr_tail = stderr_lines_for_error
                        .lock()
                        .map(|buf| buf.join("\n"))
                        .unwrap_or_default();
                    let _ = app.emit(
                        "subprocess://failed",
                        serde_json::json!({
                            "taskId": task_id_stdout,
                            "accountId": account_id_stdout,
                            "exitCode": exit_code,
                            "error": format!("Process exited with code {}", exit_code),
                            "stderr": stderr_tail,
                        }),
                    );
                }
            }

            tracing::info!(
                "[subprocess:{}] exited (code={})",
                task_id_stdout,
                exit_code
            );

            // Orphan shell accounts are cleaned on next app startup
            // (init_services_inner). Not cleaned here to avoid race with
            // user retrying in AddAccountDialog.
        });

        Ok(())
    }

    /// List currently running subprocesses.
    pub fn list_running(&self) -> Vec<SubprocessInfo> {
        self.processes
            .iter()
            .map(|entry| {
                let h = entry.value();
                SubprocessInfo {
                    task_id: h.task_id.clone(),
                    account_id: h.account_id.clone(),
                    subprocess_type: h.subprocess_type,
                    flow: h.flow.clone(),
                }
            })
            .collect()
    }

    /// Check if a subprocess is running.
    pub fn is_running(&self, task_id: &str) -> bool {
        self.processes.contains_key(task_id)
    }

    /// Send a JSON message to subprocess stdin.
    /// Takes stdin out of the map to avoid holding DashMap lock across await.
    pub async fn send_stdin(&self, task_id: &str, message: &str) -> Result<(), String> {
        let mut stdin = {
            let mut entry = self
                .processes
                .get_mut(task_id)
                .ok_or_else(|| format!("No subprocess with task_id: {}", task_id))?;
            entry.value_mut().stdin.take()
                .ok_or_else(|| "Subprocess stdin not available".to_string())?
        }; // DashMap lock released here

        let msg = format!("{}\n", message);
        let result = stdin
            .write_all(msg.as_bytes())
            .await
            .map(|_| ())
            .and(stdin.flush().await)
            .map_err(|e| format!("Failed to write to stdin: {}", e));

        // Put stdin back
        if let Some(mut entry) = self.processes.get_mut(task_id) {
            entry.value_mut().stdin = Some(stdin);
        }

        result
    }

    /// Send abort signal to subprocess via stdin.
    pub async fn abort(&self, task_id: &str) -> Result<(), String> {
        self.send_stdin(task_id, r#"{"type":"abort"}"#).await
    }

    /// Graceful shutdown: send abort to all via stdin → wait up to 3s → SIGKILL survivors.
    /// App exit needs fast, reliable termination so we skip SIGTERM and go straight
    /// to SIGKILL once the graceful window elapses.
    pub async fn kill_all(&self) {
        // 1. Send abort to all
        let task_ids: Vec<String> = self.processes.iter().map(|e| e.key().clone()).collect();
        for tid in &task_ids {
            let _ = self.send_stdin(tid, r#"{"type":"abort"}"#).await;
        }

        if task_ids.is_empty() { return; }

        // 2. Wait up to 3s for graceful exit
        for _ in 0..6 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if self.processes.is_empty() { return; }
        }

        // 3. SIGKILL survivors
        for entry in self.processes.iter() {
            let pid = entry.value().pid;
            if pid > 0 {
                tracing::info!("[subprocess] killing {} (pid={})", entry.key(), pid);
                let _ = std::process::Command::new("kill")
                    .args(["-9", &pid.to_string()])
                    .output();
            }
        }
        self.processes.clear();
    }
}

// ── Command resolution ─────────────────────────────────

// ─── OAuth result → Account.cli ─────────────────────────

async fn handle_oauth_result(
    account_manager: &AccountManager,
    account_id: &str,
    data: &serde_json::Value,
) -> Result<(), String> {
    let access_token = data["accessToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let refresh_token = data["refreshToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let expires_at = data["expiresAt"].as_i64().unwrap_or(0);
    let scopes_str = data["scopes"].as_str().unwrap_or(
        "user:inference user:profile user:sessions:claude_code user:mcp_servers user:file_upload",
    );
    let scopes: Vec<String> = scopes_str
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    if access_token.is_empty() {
        return Err("accessToken is empty".to_string());
    }

    let at = access_token.clone();
    let rt = refresh_token.clone();
    account_manager
        .update(account_id, |a| {
            let existing_device_id = a.cli.as_ref().map(|c| c.device_id.as_str()).unwrap_or("");
            a.cli = Some(CliClient {
                access_token: access_token.clone(),
                refresh_token: refresh_token.clone(),
                expires_at,
                scopes: scopes.clone(),
                last_activity: Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64,
                ),
                device_id: crate::models::account::ensure_device_id(existing_device_id),
            });
        })
        .await?;

    // Notify consumers: fresh OAuth token, add or update in ClientManager
    use crate::modules::account_change::AccountChange;
    account_manager.route_change(account_id, AccountChange::CliTokenUpdated {
        access_token: at,
        refresh_token: rt,
        expires_at,
    }).await;

    tracing::info!(
        "[oauth] Account {} CLI token updated (expires_at={})",
        account_id,
        expires_at
    );

    Ok(())
}

// ─── Webapp command resolution ──────────────────────────

/// Find the bun binary. Checks common install locations since Finder-launched
/// apps have a minimal PATH that excludes ~/.bun/bin and /opt/homebrew/bin.
pub fn find_bun() -> String {
    if let Ok(output) = std::process::Command::new("which")
        .arg("bun")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() && std::path::Path::new(&path).exists() {
                return path;
            }
        }
    }
    let home = dirs::home_dir().unwrap_or_default();
    let candidates = [
        home.join(".bun/bin/bun"),
        std::path::PathBuf::from("/opt/homebrew/bin/bun"),
        std::path::PathBuf::from("/usr/local/bin/bun"),
    ];
    for p in &candidates {
        if p.exists() {
            return p.to_string_lossy().to_string();
        }
    }
    "bun".to_string()
}

/// Resolve the webapp command.
///
/// Priority:
///   1. CLAUDE_ULTRA_WEBAPP env var (dev override)
///   2. Debug only: source tree dev path (never in release builds)
///   3. App bundle: ../Resources/webapp/webapp.js (packaged .dmg)
pub fn get_webapp_command() -> (String, Vec<String>) {
    let bun = find_bun();

    // 1. Dev override via environment variable
    if let Ok(path) = std::env::var("CLAUDE_ULTRA_WEBAPP") {
        if std::path::Path::new(&path).exists() {
            return (bun, vec!["run".to_string(), path]);
        }
    }

    // 2. Debug builds only: check local dev source tree
    #[cfg(debug_assertions)]
    {
        let home = dirs::home_dir().unwrap_or_default();
        let dev_path = home.join("claude-ultra/webapp/src/main.ts");
        if dev_path.exists() {
            return (
                bun.clone(),
                vec!["run".to_string(), dev_path.to_string_lossy().to_string()],
            );
        }
    }

    // 3. Packaged mode: bundled webapp.js in app Resources
    let exe = std::env::current_exe().unwrap_or_default();
    let bundled = exe
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("../Resources/webapp/webapp.js");
    if bundled.exists() {
        return (
            bun,
            vec!["run".to_string(), bundled.to_string_lossy().to_string()],
        );
    }

    // Fallback
    (bun, vec!["run".to_string(), bundled.to_string_lossy().to_string()])
}

// ─── Tests ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 1. StdoutMessage parsing ────────────────────────

    #[test]
    fn test_parse_step_message() {
        let json = r#"{"type":"step","step":1,"total":7,"name":"IP门控","status":"running"}"#;
        let msg: StdoutMessage = serde_json::from_str(json).unwrap();
        match msg {
            StdoutMessage::Step {
                step,
                total,
                name,
                status,
            } => {
                assert_eq!(step, 1);
                assert_eq!(total, 7);
                assert_eq!(name, "IP门控");
                assert_eq!(status, "running");
            }
            _ => panic!("Expected Step message"),
        }
    }

    #[test]
    fn test_parse_log_message() {
        let json = r#"{"type":"log","level":"info","msg":"浏览器 IP: 1.2.3.4"}"#;
        let msg: StdoutMessage = serde_json::from_str(json).unwrap();
        match msg {
            StdoutMessage::Log { level, msg } => {
                assert_eq!(level, "info");
                assert_eq!(msg, "浏览器 IP: 1.2.3.4");
            }
            _ => panic!("Expected Log message"),
        }
    }

    #[test]
    fn test_parse_progress_message() {
        let json = r#"{"type":"progress","percent":42}"#;
        let msg: StdoutMessage = serde_json::from_str(json).unwrap();
        match msg {
            StdoutMessage::Progress { percent } => {
                assert_eq!(percent, 42);
            }
            _ => panic!("Expected Progress message"),
        }
    }

    #[test]
    fn test_parse_result_success() {
        let json = r#"{"type":"result","success":true,"data":{"accessToken":"sk-ant-oat01-test","refreshToken":"sk-ant-ort01-test","expiresAt":1775516162349,"scopes":"user:inference user:profile"}}"#;
        let msg: StdoutMessage = serde_json::from_str(json).unwrap();
        match msg {
            StdoutMessage::Result { success, data } => {
                assert!(success);
                assert_eq!(data["accessToken"], "sk-ant-oat01-test");
                assert_eq!(data["refreshToken"], "sk-ant-ort01-test");
                assert_eq!(data["expiresAt"], 1775516162349_i64);
            }
            _ => panic!("Expected Result message"),
        }
    }

    #[test]
    fn test_parse_error_message() {
        let json = r#"{"type":"error","code":"cf-block","msg":"CF 403","retriable":true}"#;
        let msg: StdoutMessage = serde_json::from_str(json).unwrap();
        match msg {
            StdoutMessage::Error {
                code,
                msg,
                retriable,
            } => {
                assert_eq!(code, "cf-block");
                assert_eq!(msg, "CF 403");
                assert!(retriable);
            }
            _ => panic!("Expected Error message"),
        }
    }

    #[test]
    fn test_parse_invalid_json_fails() {
        let json = "not json at all";
        let result = serde_json::from_str::<StdoutMessage>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_unknown_type_fails() {
        let json = r#"{"type":"unknown","data":"test"}"#;
        let result = serde_json::from_str::<StdoutMessage>(json);
        assert!(result.is_err());
    }

    // ── 2. handle_oauth_result ──────────────────────────

    #[tokio::test]
    async fn test_handle_oauth_result_writes_cli() {
        let dir = std::env::temp_dir().join(format!(
            "claude_ultra_subprocess_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // Write a minimal account JSON
        let json = r#"{"accountId":"test_oauth","email":"test@example.com"}"#;
        std::fs::write(dir.join("test_oauth.json"), json).unwrap();

        let mgr = AccountManager::new(dir.clone());

        let data = serde_json::json!({
            "accessToken": "sk-ant-oat01-test-token",
            "refreshToken": "sk-ant-ort01-test-refresh",
            "expiresAt": 2000000000000_i64,
            "scopes": "user:inference user:profile user:sessions:claude_code",
        });

        handle_oauth_result(&mgr, "test_oauth", &data)
            .await
            .unwrap();

        // Verify CLI was written
        let account = mgr.read("test_oauth").await.unwrap();
        let cli = account.cli.unwrap();
        assert_eq!(cli.access_token, "sk-ant-oat01-test-token");
        assert_eq!(cli.refresh_token, "sk-ant-ort01-test-refresh");
        assert_eq!(cli.expires_at, 2000000000000);
        assert_eq!(cli.scopes.len(), 3);
        assert!(cli.last_activity.is_some());

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_handle_oauth_result_empty_token_rejected() {
        let dir = std::env::temp_dir().join(format!(
            "claude_ultra_subprocess_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let json = r#"{"accountId":"test_empty","email":"t@e.com"}"#;
        std::fs::write(dir.join("test_empty.json"), json).unwrap();

        let mgr = AccountManager::new(dir.clone());

        let data = serde_json::json!({
            "accessToken": "",
            "refreshToken": "sk-ant-ort01-test",
            "expiresAt": 0,
        });

        let result = handle_oauth_result(&mgr, "test_empty", &data).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("accessToken is empty"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_handle_oauth_result_default_scopes() {
        let dir = std::env::temp_dir().join(format!(
            "claude_ultra_subprocess_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let json = r#"{"accountId":"test_scopes","email":"t@e.com"}"#;
        std::fs::write(dir.join("test_scopes.json"), json).unwrap();

        let mgr = AccountManager::new(dir.clone());

        // No scopes field → should use default 5 scopes
        let data = serde_json::json!({
            "accessToken": "sk-ant-oat01-valid",
            "refreshToken": "sk-ant-ort01-valid",
            "expiresAt": 2000000000000_i64,
        });

        handle_oauth_result(&mgr, "test_scopes", &data)
            .await
            .unwrap();

        let account = mgr.read("test_scopes").await.unwrap();
        let cli = account.cli.unwrap();
        assert_eq!(cli.scopes.len(), 5);
        assert!(cli.scopes.contains(&"user:inference".to_string()));
        assert!(cli.scopes.contains(&"user:file_upload".to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── 3. get_webapp_command ───────────────────────────

    #[test]
    fn test_get_webapp_command_env_override() {
        // Uses CLAUDE_ULTRA_WEBAPP env var when set
        let tmp = std::env::temp_dir().join("test_webapp_entry.ts");
        std::fs::write(&tmp, "// test").unwrap();
        std::env::set_var("CLAUDE_ULTRA_WEBAPP", tmp.to_str().unwrap());
        let (cmd, args) = get_webapp_command();
        assert!(cmd.contains("bun") || cmd == "bun");
        assert!(args.contains(&"run".to_string()));
        assert!(args.last().unwrap().contains("test_webapp_entry"));
        std::env::remove_var("CLAUDE_ULTRA_WEBAPP");
        let _ = std::fs::remove_file(&tmp);
    }

    // ── 4. SubprocessManager basics ────────────────────

    #[test]
    fn test_subprocess_manager_new() {
        let mgr = SubprocessManager::new();
        assert_eq!(mgr.list_running().len(), 0);
        assert!(!mgr.is_running("nonexistent"));
    }

    // ── 5. StdoutMessage serialization roundtrip ───────

    #[test]
    fn test_stdout_message_roundtrip() {
        let messages = vec![
            StdoutMessage::Step {
                step: 0,
                total: 7,
                name: "IP门控".to_string(),
                status: "done".to_string(),
            },
            StdoutMessage::Log {
                level: "info".to_string(),
                msg: "test".to_string(),
            },
            StdoutMessage::Progress { percent: 50 },
            StdoutMessage::Result {
                success: true,
                data: serde_json::json!({"token": "test"}),
            },
            StdoutMessage::Error {
                code: "test".to_string(),
                msg: "error".to_string(),
                retriable: false,
            },
        ];

        for msg in messages {
            let json = serde_json::to_string(&msg).unwrap();
            let parsed: StdoutMessage = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, json2);
        }
    }
}
