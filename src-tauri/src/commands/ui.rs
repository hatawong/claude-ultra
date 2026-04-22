//! UI-level IPC commands — window theme, show window, Claude settings sync, etc.

use tauri::Manager;

#[tauri::command]
pub async fn show_main_window(window: tauri::Window) {
    window.get_webview_window("main").map(|w| w.show().ok());
}

/// Set window theme — syncs native UI elements (context menus, etc.) with app theme.
#[tauri::command]
pub async fn set_window_theme(window: tauri::Window, theme: String) -> Result<(), String> {
    use tauri::Theme;
    let tauri_theme = match theme.as_str() {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        _ => None,
    };
    window.set_theme(tauri_theme).map_err(|e| e.to_string())
}

/// Read current Claude Code settings.json env values.
#[tauri::command]
pub async fn get_claude_settings() -> Result<serde_json::Value, String> {
    let path = get_claude_settings_path()?;
    if !path.exists() {
        return Ok(serde_json::json!({ "env": {} }));
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let settings: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
    Ok(serde_json::json!({
        "env": settings.get("env").cloned().unwrap_or(serde_json::json!({})),
    }))
}

/// Sync gateway env vars into Claude Code settings.json.
/// Merges into existing env — only overwrites ANTHROPIC_BASE_URL, ANTHROPIC_API_KEY, DISABLE_TELEMETRY.
#[tauri::command]
pub async fn sync_claude_settings(
    base_url: String,
    api_key: String,
) -> Result<String, String> {
    let path = get_claude_settings_path()?;

    // Read existing or create empty
    let mut settings: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?
    } else {
        serde_json::json!({})
    };

    // Merge env
    let env = settings
        .as_object_mut()
        .ok_or("settings.json is not an object")?
        .entry("env")
        .or_insert(serde_json::json!({}));

    let env_obj = env
        .as_object_mut()
        .ok_or("env is not an object")?;

    env_obj.insert("ANTHROPIC_BASE_URL".to_string(), serde_json::json!(base_url));
    env_obj.insert("ANTHROPIC_API_KEY".to_string(), serde_json::json!(api_key));
    env_obj.insert("DISABLE_TELEMETRY".to_string(), serde_json::json!("1"));
    env_obj.insert("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(), serde_json::json!("1"));

    // Atomic write + 0600: settings.json contains ANTHROPIC_API_KEY
    crate::modules::secure_fs::secure_write_json(&path, &settings)?;

    Ok("Claude Code settings synced.".to_string())
}

fn get_claude_settings_path() -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot find home directory")?;
    Ok(home.join(".claude").join("settings.json"))
}
