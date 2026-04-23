//! Subprocess IPC commands — start/stop/pause/resume OAuth & web login, log access.

use crate::GatewayServiceState;
use crate::subprocess;

/// Helper: ensure no old subprocess is running for this task_id.
pub(super) async fn abort_if_running(mgr: &subprocess::SubprocessManager, task_id: &str) -> Result<(), String> {
    if !mgr.is_running(task_id).await { return Ok(()); }

    if let Err(e) = mgr.abort(task_id).await {
        tracing::warn!("[abort_if_running:{}] abort returned err: {}", task_id, e);
    }

    for _ in 0..20 {
        if !mgr.is_running(task_id).await { return Ok(()); }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    if mgr.is_running(task_id).await {
        return Err(format!("Failed to stop old subprocess {}", task_id));
    }
    Ok(())
}

#[tauri::command]
pub async fn start_oauth(
    account_id: String,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    let account = state.account_manager.read(&account_id).await?;
    let country = account.resolve_country();

    let (command, mut args) = subprocess::get_webapp_command();
    args.push("oauth".to_string());
    args.push(format!("--id={}", account_id));
    args.push("--app=manager".to_string());
    args.push("--auto".to_string());
    args.push(format!("--country={}", country));

    let task_id = format!("web-oauth-{}", account_id);
    abort_if_running(&state.subprocess_manager, &task_id).await?;

    state
        .subprocess_manager
        .spawn_and_watch(
            task_id.clone(),
            account_id,
            subprocess::SubprocessType::Webapp,
            "web-oauth".to_string(),
            &command,
            args,
            app_handle,
            state.account_manager.clone(),
        )
        .await?;

    Ok(task_id)
}

/// Start web login subprocess for an existing account.
#[tauri::command]
pub async fn start_web_login(
    account_id: String,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    let account = state.account_manager.read(&account_id).await?;
    let country = account.resolve_country();

    let (command, mut args) = subprocess::get_webapp_command();
    args.push("login".to_string());
    args.push(format!("--id={}", account_id));
    args.push("--app=manager".to_string());
    args.push("--auto".to_string());
    args.push(format!("--country={}", country));

    let task_id = format!("web-login-{}", account_id);
    abort_if_running(&state.subprocess_manager, &task_id).await?;

    state
        .subprocess_manager
        .spawn_and_watch(
            task_id.clone(),
            account_id,
            subprocess::SubprocessType::Webapp,
            "web-login".to_string(),
            &command,
            args,
            app_handle,
            state.account_manager.clone(),
        )
        .await?;

    Ok(task_id)
}

/// Pause a running subprocess.
#[tauri::command]
pub async fn pause_subprocess(
    task_id: String,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    state.subprocess_manager.send_stdin(&task_id, r#"{"type":"pause"}"#).await?;
    state.subprocess_manager.mark_paused(&task_id).await;
    Ok(())
}

/// Resume a paused subprocess.
#[tauri::command]
pub async fn resume_subprocess(
    task_id: String,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    state.subprocess_manager.send_stdin(&task_id, r#"{"type":"resume"}"#).await?;
    state.subprocess_manager.mark_resumed(&task_id).await;
    Ok(())
}

/// Check if a subprocess task is still running.
#[tauri::command]
pub async fn is_subprocess_running(
    task_id: String,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<bool, String> {
    Ok(state.subprocess_manager.is_running(&task_id).await)
}

/// Abort a running subprocess.
#[tauri::command]
pub async fn abort_subprocess(
    task_id: String,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    state.subprocess_manager.abort(&task_id).await
}

/// Validate task_id: only allow alphanumeric, hyphens, underscores.
fn validate_task_id(task_id: &str) -> Result<(), String> {
    if task_id.is_empty() || task_id.len() > 200 {
        return Err("Invalid task_id length".to_string());
    }
    if !task_id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("Invalid task_id characters: {}", task_id));
    }
    Ok(())
}

fn get_log_dir() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_default().join(".claude-ultra").join("logs")
}

/// Read subprocess log file.
#[tauri::command]
pub async fn get_subprocess_log(task_id: String) -> Result<String, String> {
    validate_task_id(&task_id)?;
    let path = get_log_dir().join(format!("{}.log", task_id));
    if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read log: {}", e))
    } else {
        Ok(String::new())
    }
}

/// Clear subprocess log file.
#[tauri::command]
pub async fn clear_subprocess_log(task_id: String) -> Result<(), String> {
    validate_task_id(&task_id)?;
    let path = get_log_dir().join(format!("{}.log", task_id));
    if path.exists() {
        std::fs::write(&path, "").map_err(|e| format!("Failed to clear log: {}", e))?;
    }
    Ok(())
}

/// Get Task snapshot by task_id (returns None if never spawned).
#[tauri::command]
pub async fn get_task(
    task_id: String,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<Option<serde_json::Value>, String> {
    match state.subprocess_manager.get_task(&task_id) {
        Some(task_arc) => {
            let t = task_arc.read().await;
            Ok(Some(serde_json::to_value(&*t).unwrap_or_default()))
        }
        None => Ok(None),
    }
}

/// List all Task snapshots (includes exited tasks with final status).
#[tauri::command]
pub async fn list_tasks(
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<Vec<serde_json::Value>, String> {
    // Collect Arcs first to release DashMap shard guards before .await on each RwLock.
    // Holding iter() shard read lock across .await risks blocking concurrent inserts
    // (stdout watcher) on the same shard and degrading to a serialized bottleneck.
    let tasks = state.subprocess_manager.tasks();
    let arcs: Vec<std::sync::Arc<tokio::sync::RwLock<crate::subprocess::SubprocessTask>>> =
        tasks.iter().map(|e| e.value().clone()).collect();
    let mut out = Vec::new();
    for arc in arcs {
        let t = arc.read().await;
        out.push(serde_json::to_value(&*t).unwrap_or_default());
    }
    Ok(out)
}

/// Pre-flight check for webapp prerequisites.
#[derive(serde::Serialize)]
pub struct PreflightResult {
    pub ready: bool,
    pub issues: Vec<String>,
}

#[tauri::command]
pub async fn check_webapp_ready() -> PreflightResult {
    let mut issues = Vec::new();

    let bun_path = subprocess::find_bun();
    let bun_ok = std::process::Command::new(&bun_path)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !bun_ok {
        issues.push("Bun runtime not found. Install: https://bun.sh".to_string());
    }

    let chrome_ok = std::path::Path::new("/Applications/Google Chrome.app").exists();
    if !chrome_ok {
        issues.push("Google Chrome not found in /Applications".to_string());
    }

    let (cmd, args) = subprocess::get_webapp_command();
    let webapp_path = if args.len() >= 2 { &args[1] } else { &cmd };
    let webapp_ok = std::path::Path::new(webapp_path).exists();
    if !webapp_ok {
        issues.push(format!("Webapp not found at: {}", webapp_path));
    }

    if !bun_ok {
        issues.push("macOS: if Bun is installed but blocked, go to System Settings → Privacy & Security and allow the app.".to_string());
    }

    PreflightResult {
        ready: issues.is_empty(),
        issues,
    }
}
