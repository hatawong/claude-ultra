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
use tokio::sync::RwLock;

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
    Profile {
        data: serde_json::Value,
    },
}

/// Subprocess type identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubprocessType {
    Webapp,
}

// ─── Task lifecycle ─────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Paused,
    Done,
    Failed,
    Aborted,
}

/// Process handle — stdin/pid/generation; Some while process alive, None after exit.
/// Not serialized (ChildStdin is not Serialize).
#[derive(Debug)]
pub struct ProcessHandle {
    pub stdin: Option<tokio::process::ChildStdin>,
    pub pid: u32,
    pub generation: u64,
}

/// Single source of truth for a subprocess task lifecycle.
/// Persists after process exit (retains final status/result for UI restore).
/// Note: percent is not stored — derived from step/total_steps + status at render time.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessTask {
    // Identity
    pub task_id: String,
    pub account_id: String,
    pub subprocess_type: SubprocessType,
    pub flow: String,
    // Lifecycle
    pub status: TaskStatus,
    pub step: u32,
    pub total_steps: u32,
    pub step_name: String,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub last_event_at: i64,
    // Process handle — skipped from IPC
    #[serde(skip)]
    pub process: Option<ProcessHandle>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Emit `subprocess://task` with the current task snapshot.
async fn emit_task_snapshot(app: &tauri::AppHandle, task_arc: &Arc<RwLock<SubprocessTask>>) {
    let snapshot = {
        let t = task_arc.read().await;
        serde_json::to_value(&*t).unwrap_or_default()
    };
    let _ = app.emit("subprocess://task", &snapshot);
}

// ─── SubprocessManager ──────────────────────────────────

pub struct SubprocessManager {
    tasks: Arc<DashMap<String, Arc<RwLock<SubprocessTask>>>>,
    generation: AtomicU64,
    app_handle: std::sync::OnceLock<tauri::AppHandle>,
}

impl SubprocessManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(DashMap::new()),
            generation: AtomicU64::new(0),
            app_handle: std::sync::OnceLock::new(),
        }
    }

    pub fn get_task(&self, task_id: &str) -> Option<Arc<RwLock<SubprocessTask>>> {
        self.tasks.get(task_id).map(|e| e.value().clone())
    }

    pub fn tasks(&self) -> Arc<DashMap<String, Arc<RwLock<SubprocessTask>>>> {
        self.tasks.clone()
    }

    async fn emit_task_updated(&self, task_id: &str) {
        if let (Some(app), Some(entry)) = (self.app_handle.get(), self.tasks.get(task_id)) {
            let task_arc = entry.value().clone();
            drop(entry);
            emit_task_snapshot(app, &task_arc).await;
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
        // Register AppHandle on first spawn for emit_task_updated to work.
        let _ = self.app_handle.set(app_handle.clone());

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

        // Create or replace Task in map. If an old Task exists (from previous run),
        // its process was already aborted by abort_if_running; overwrite wholesale.
        let gen = self.generation.fetch_add(1, Ordering::SeqCst);
        let started = now_ms();
        let task = Arc::new(RwLock::new(SubprocessTask {
            task_id: task_id.clone(),
            account_id: account_id.clone(),
            subprocess_type,
            flow: flow.clone(),
            status: TaskStatus::Running,
            step: 0,
            total_steps: 0,
            step_name: String::new(),
            error: None,
            result: None,
            started_at: started,
            finished_at: None,
            last_event_at: started,
            process: Some(ProcessHandle { stdin, pid, generation: gen }),
        }));
        self.tasks.insert(task_id.clone(), task.clone());

        // Emit initial snapshot so frontend can restore dialog immediately.
        emit_task_snapshot(&app_handle, &task).await;

        let task_id_stdout = task_id.clone();
        let account_id_stdout = account_id.clone();
        let flow_stdout = flow.clone();
        let app = app_handle.clone();
        let acct_mgr = account_manager.clone();
        let tasks = self.tasks.clone();
        let task_arc_stdout = task.clone();

        // Background task: read stderr — write to log file + emit to frontend + collect for error
        let task_id_stderr = task_id.clone();
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
            }
        });

        // Background task: read stdout JSON lines
        let stderr_lines_for_error = stderr_lines.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                // Try parse as JSON
                match serde_json::from_str::<StdoutMessage>(&line) {
                    Ok(msg) => {
                        // Update Task state (single source of truth). subprocess://task emitted after.
                        let task_changed = {
                            let mut t = task_arc_stdout.write().await;
                            t.last_event_at = now_ms();
                            match &msg {
                                StdoutMessage::Step { step, total, name, .. } => {
                                    t.step = *step;
                                    t.total_steps = *total;
                                    t.step_name = name.clone();
                                    true
                                }
                                StdoutMessage::Result { success: true, data } => {
                                    // Store result but keep status=Running; wait() exit will set Done
                                    // after browser close (webapp has post-complete 30s wait).
                                    t.result = Some(data.clone());
                                    true
                                }
                                StdoutMessage::Result { success: false, data } => {
                                    t.status = TaskStatus::Failed;
                                    t.error = data.get("error")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string())
                                        .or_else(|| Some("Task failed".to_string()));
                                    t.finished_at = Some(now_ms());
                                    true
                                }
                                StdoutMessage::Error { code, msg: emsg, .. } => {
                                    t.error = Some(format!("{}: {}", code, emsg));
                                    true
                                }
                                _ => false,
                            }
                        };
                        if task_changed {
                            emit_task_snapshot(&app, &task_arc_stdout).await;
                        }

                        // Direct write: cookies event → update Account.web (replaces frontend round-trip).
                        if let StdoutMessage::Cookies { ref cookies, ref session_key } = msg {
                            if let Some(sk) = session_key.as_deref() {
                                if !sk.is_empty() {
                                    // Parse failure must not clobber valid web state with an empty vec.
                                    match serde_json::from_value::<Vec<crate::models::account::CookieData>>(cookies.clone()) {
                                        Ok(cookie_vec) => {
                                            if let Err(e) = acct_mgr.set_web(&account_id_stdout, Some(cookie_vec)).await {
                                                tracing::error!(
                                                    "[subprocess:{}] write cookies failed: {}",
                                                    task_id_stdout, e
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                "[subprocess:{}] cookies parse failed, preserving existing web: {}",
                                                task_id_stdout, e
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // Direct write: profile event → update Account profile fields.
                        if let StdoutMessage::Profile { ref data } = msg {
                            let update = crate::modules::account_manager::ProfileUpdate {
                                email: data.get("email").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                account_uuid: data.get("accountUuid").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                org_id: data.get("orgId").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                full_name: data.get("fullName").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                subscription_type: data.get("subscriptionType").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                rate_limit_tier: data.get("rateLimitTier").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                billing_type: data.get("billingType").and_then(|v| v.as_str()).map(|s| s.to_string()),
                            };
                            if let Err(e) = acct_mgr.set_profile(&account_id_stdout, update).await {
                                tracing::error!(
                                    "[subprocess:{}] write profile failed: {}",
                                    task_id_stdout, e
                                );
                            }
                        }

                        // Handle result by flow type
                        if let StdoutMessage::Result { success: true, ref data } = msg {

                            // Common writes for web-add / web-login / web-oauth:
                            // final cookies, proxy, profile (overwrite mid-flow partial writes).
                            if matches!(flow_stdout.as_str(), "web-add" | "web-login" | "web-oauth") {
                                // Final web credentials
                                let sk = data.get("sessionKey").and_then(|v| v.as_str()).unwrap_or("");
                                if !sk.is_empty() {
                                    if let Some(cookies_val) = data.get("cookies") {
                                        // Parse failure must not clobber valid web state.
                                        match serde_json::from_value::<Vec<crate::models::account::CookieData>>(cookies_val.clone()) {
                                            Ok(cookie_vec) => {
                                                if let Err(e) = acct_mgr.set_web(&account_id_stdout, Some(cookie_vec)).await {
                                                    tracing::error!("[subprocess:{}] result set_web failed: {}", task_id_stdout, e);
                                                }
                                            }
                                            Err(e) => {
                                                tracing::error!(
                                                    "[subprocess:{}] result cookies parse failed, preserving existing web: {}",
                                                    task_id_stdout, e
                                                );
                                            }
                                        }
                                    }
                                }
                                // Proxy
                                if let Some(proxy_val) = data.get("proxy") {
                                    if !proxy_val.is_null() {
                                        match serde_json::from_value::<crate::models::account::ProxySection>(proxy_val.clone()) {
                                            Ok(ps) => {
                                                if let Err(e) = acct_mgr.set_proxy(&account_id_stdout, Some(ps)).await {
                                                    tracing::error!("[subprocess:{}] result set_proxy failed: {}", task_id_stdout, e);
                                                }
                                            }
                                            Err(e) => {
                                                tracing::warn!(
                                                    "[subprocess:{}] result.proxy failed to deserialize as ProxySection (skipping set_proxy): {} — payload={}",
                                                    task_id_stdout, e, proxy_val,
                                                );
                                            }
                                        }
                                    }
                                }
                                // Profile
                                let update = crate::modules::account_manager::ProfileUpdate {
                                    email: data.get("email").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                    account_uuid: data.get("accountUuid").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                    org_id: data.get("orgId").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                    full_name: data.get("fullName").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                    subscription_type: data.get("subscriptionType").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                    rate_limit_tier: data.get("rateLimitTier").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                    billing_type: data.get("billingType").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                };
                                if let Err(e) = acct_mgr.set_profile(&account_id_stdout, update).await {
                                    tracing::error!("[subprocess:{}] result set_profile failed: {}", task_id_stdout, e);
                                }
                            }

                            match flow_stdout.as_str() {
                                "web-oauth" => {
                                    // OAuth-specific: write CLI tokens (set_cli emits CliUpdated)
                                    if let Err(e) = handle_oauth_result(
                                        &acct_mgr, &account_id_stdout, data,
                                    ).await {
                                        tracing::error!(
                                            "[subprocess:{}] Failed to write Account.cli: {}",
                                            task_id_stdout, e
                                        );
                                    }
                                }
                                "web-add" => {
                                    // Account creation complete → trigger Created flow
                                    use crate::modules::account_change::AccountChange;
                                    acct_mgr.route_change(&account_id_stdout, AccountChange::Created).await;
                                }
                                _ => {}
                            }
                        }

                        // Handle error
                        if let StdoutMessage::Error { ref code, ref msg, .. } = msg {
                            tracing::warn!(
                                "[subprocess:{}] Error: {} — {}",
                                task_id_stdout,
                                code,
                                msg
                            );
                            // Suspended/banned → auto disable account (replaces frontend mark_account_disabled call)
                            let msg_lower = msg.to_lowercase();
                            if msg_lower.contains("suspended") || msg_lower.contains("banned") {
                                let date = chrono::Utc::now().format("%Y-%m-%d");
                                let reason = format!("Web login banned ({})", date);
                                if let Err(e) = acct_mgr.set_disabled(&account_id_stdout, Some(reason)).await {
                                    tracing::error!(
                                        "[subprocess:{}] auto-disable failed: {}",
                                        task_id_stdout, e
                                    );
                                }
                            }
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

            // Clear process handle so is_running() returns false.
            // Task object stays in map with final status.
            let task_arc_opt = tasks.get(&task_id_stdout).map(|e| e.value().clone());
            if let Some(task_arc) = task_arc_opt {
                {
                    let mut t = task_arc.write().await;
                    if let Some(ref ph) = t.process {
                        if ph.generation != gen {
                            // A newer generation replaced us; do nothing.
                            return;
                        }
                    }
                    t.process = None;
                    t.last_event_at = now_ms();
                    if t.finished_at.is_none() {
                        t.finished_at = Some(now_ms());
                    }
                    // If no terminal result emitted, set Failed from exit code
                    if matches!(t.status, TaskStatus::Running | TaskStatus::Paused) {
                        if exit_code == 0 {
                            t.status = TaskStatus::Done;
                        } else {
                            t.status = TaskStatus::Failed;
                            if t.error.is_none() {
                                t.error = Some(format!("Process exited with code {}", exit_code));
                            }
                        }
                    }
                }
                emit_task_snapshot(&app, &task_arc).await;
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

            // Task state already updated above (Done/Failed based on exit_code).
            // subprocess://task snapshot emitted after that update covers lifecycle end.
            // stderr_lines_for_error retained for future use (error reporting via Task.error).
            let _ = &stderr_lines_for_error;

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

    /// Check if a subprocess is running (process handle present).
    pub async fn is_running(&self, task_id: &str) -> bool {
        match self.tasks.get(task_id) {
            Some(entry) => entry.value().read().await.process.is_some(),
            None => false,
        }
    }

    /// Send a JSON message to subprocess stdin.
    /// On write failure (broken pipe etc.), stdin is permanently dropped — future
    /// send_stdin calls will return "Subprocess stdin not available".
    pub async fn send_stdin(&self, task_id: &str, message: &str) -> Result<(), String> {
        let task_arc = self
            .tasks
            .get(task_id)
            .ok_or_else(|| format!("No task with task_id: {}", task_id))?
            .value()
            .clone();

        // Take stdin out (hold write lock briefly, drop before await)
        let mut stdin = {
            let mut t = task_arc.write().await;
            let ph = t.process.as_mut()
                .ok_or_else(|| "Subprocess not running".to_string())?;
            ph.stdin.take()
                .ok_or_else(|| "Subprocess stdin not available".to_string())?
        };

        let msg = format!("{}\n", message);
        let write_result = stdin
            .write_all(msg.as_bytes())
            .await
            .and(stdin.flush().await)
            .map_err(|e| format!("Failed to write to stdin: {}", e));

        // Put stdin back ONLY on success. On failure, drop the stdin permanently
        // (broken pipe etc.) to prevent future callers from reusing a dead handle.
        if write_result.is_ok() {
            let mut t = task_arc.write().await;
            if let Some(ph) = t.process.as_mut() {
                ph.stdin = Some(stdin);
            }
        }

        write_result
    }

    /// Send abort signal to subprocess via stdin + mark status Aborted.
    /// User intent takes precedence: status is marked Aborted BEFORE attempting
    /// send_stdin, so stdin failure (broken pipe / concurrent take) doesn't
    /// invalidate the user's abort click. wait() exit won't overwrite Aborted
    /// (check excludes Aborted from the Running|Paused match).
    pub async fn abort(&self, task_id: &str) -> Result<(), String> {
        let changed = if let Some(entry) = self.tasks.get(task_id) {
            let task_arc = entry.value().clone();
            drop(entry);
            let mut t = task_arc.write().await;
            if matches!(t.status, TaskStatus::Running | TaskStatus::Paused) {
                t.status = TaskStatus::Aborted;
                t.last_event_at = now_ms();
                t.finished_at.get_or_insert(now_ms());
                true
            } else {
                false
            }
        } else {
            false
        };
        if changed { self.emit_task_updated(task_id).await; }

        // Best-effort send abort to webapp — failure is logged but not returned.
        if let Err(e) = self.send_stdin(task_id, r#"{"type":"abort"}"#).await {
            tracing::warn!("[subprocess:{}] abort send_stdin failed (best-effort): {}", task_id, e);
        }
        Ok(())
    }

    /// Mark status as Paused (caller still needs to send pause message via send_stdin).
    pub async fn mark_paused(&self, task_id: &str) {
        let changed = if let Some(entry) = self.tasks.get(task_id) {
            let task_arc = entry.value().clone();
            drop(entry);
            let mut t = task_arc.write().await;
            if t.status == TaskStatus::Running {
                t.status = TaskStatus::Paused;
                t.last_event_at = now_ms();
                true
            } else {
                false
            }
        } else {
            false
        };
        if changed { self.emit_task_updated(task_id).await; }
    }

    /// Mark status as Running (caller still needs to send resume message via send_stdin).
    pub async fn mark_resumed(&self, task_id: &str) {
        let changed = if let Some(entry) = self.tasks.get(task_id) {
            let task_arc = entry.value().clone();
            drop(entry);
            let mut t = task_arc.write().await;
            if t.status == TaskStatus::Paused {
                t.status = TaskStatus::Running;
                t.last_event_at = now_ms();
                true
            } else {
                false
            }
        } else {
            false
        };
        if changed { self.emit_task_updated(task_id).await; }
    }

    /// Graceful shutdown: send abort to all via stdin → wait up to 3s → SIGKILL survivors.
    pub async fn kill_all(&self) {
        // 1. Collect running task_ids + send abort to each
        let mut running_ids: Vec<String> = Vec::new();
        for entry in self.tasks.iter() {
            let t = entry.value().read().await;
            if t.process.is_some() {
                running_ids.push(t.task_id.clone());
            }
        }
        for tid in &running_ids {
            let _ = self.send_stdin(tid, r#"{"type":"abort"}"#).await;
        }

        if running_ids.is_empty() { return; }

        // 2. Wait up to 3s for graceful exit (process cleared when wait() completes)
        for _ in 0..6 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let mut still_running = false;
            for tid in &running_ids {
                if self.is_running(tid).await { still_running = true; break; }
            }
            if !still_running { return; }
        }

        // 3. SIGKILL survivors
        for tid in &running_ids {
            if let Some(entry) = self.tasks.get(tid) {
                let t = entry.value().read().await;
                if let Some(ref ph) = t.process {
                    if ph.pid > 0 {
                        tracing::info!("[subprocess] killing {} (pid={})", tid, ph.pid);
                        let _ = std::process::Command::new("kill")
                            .args(["-9", &ph.pid.to_string()])
                            .output();
                    }
                }
            }
        }
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

    account_manager
        .set_cli(account_id, access_token, refresh_token, expires_at, Some(scopes))
        .await?;
    // set_cli emits CliUpdated → ClientManager sync + frontend notify

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

    // 2. Debug builds only: check local dev source tree relative to crate root.
    //    CARGO_MANIFEST_DIR = crate dir. Try 1-level up (flat layout, e.g. public repo)
    //    then 2-levels up (nested layout where crate sits under manager/src-tauri/).
    #[cfg(debug_assertions)]
    {
        let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for rel in ["../webapp/src/main.ts", "../../webapp/src/main.ts"] {
            let dev_path = crate_dir.join(rel);
            if dev_path.exists() {
                return (
                    bun.clone(),
                    vec!["run".to_string(), dev_path.to_string_lossy().to_string()],
                );
            }
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
    fn test_parse_result_success() {
        let json = r#"{"type":"result","success":true,"data":{"accessToken":"stub-oat01-test","refreshToken":"stub-ort01-test","expiresAt":1775516162349,"scopes":"user:inference user:profile"}}"#;
        let msg: StdoutMessage = serde_json::from_str(json).unwrap();
        match msg {
            StdoutMessage::Result { success, data } => {
                assert!(success);
                assert_eq!(data["accessToken"], "stub-oat01-test");
                assert_eq!(data["refreshToken"], "stub-ort01-test");
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
            "accessToken": "stub-oat01-test-token",
            "refreshToken": "stub-ort01-test-refresh",
            "expiresAt": 2000000000000_i64,
            "scopes": "user:inference user:profile user:sessions:claude_code",
        });

        handle_oauth_result(&mgr, "test_oauth", &data)
            .await
            .unwrap();

        // Verify CLI was written
        let account = mgr.read("test_oauth").await.unwrap();
        let cli = account.cli.unwrap();
        assert_eq!(cli.access_token, "stub-oat01-test-token");
        assert_eq!(cli.refresh_token, "stub-ort01-test-refresh");
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
            "refreshToken": "stub-ort01-test",
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
            "accessToken": "stub-oat01-valid",
            "refreshToken": "stub-ort01-valid",
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

    #[tokio::test]
    async fn test_subprocess_manager_new() {
        let mgr = SubprocessManager::new();
        assert_eq!(mgr.tasks().len(), 0);
        assert!(!mgr.is_running("nonexistent").await);
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
