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
/// Merges into existing env — only overwrites the keys set here
/// (`ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY`, `DISABLE_TELEMETRY`,
/// `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, `ENABLE_PROMPT_CACHING_1H`).
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
    // Force CLI to mark cache_control entries with ttl="1h" regardless of
    // auth mode. Large sessions evict quickly on the default 5m TTL.
    env_obj.insert("ENABLE_PROMPT_CACHING_1H".to_string(), serde_json::json!("1"));

    // Atomic write + 0600: settings.json contains ANTHROPIC_API_KEY
    crate::modules::secure_fs::secure_write_json(&path, &settings)?;

    Ok("Claude Code settings synced.".to_string())
}

fn get_claude_settings_path() -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot find home directory")?;
    Ok(home.join(".claude").join("settings.json"))
}

/// Sync Claude Code settings.json env for transparent audit mode.
/// Sets `ANTHROPIC_BASE_URL` to the transparent port and **removes**
/// `ANTHROPIC_API_KEY` so the CLI falls back to OAuth credentials,
/// avoiding leaking the gateway-only key to the upstream.
#[cfg(feature = "internal")]
#[tauri::command]
pub async fn sync_claude_settings_transparent(base_url: String) -> Result<String, String> {
    let path = get_claude_settings_path()?;

    let mut settings: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?
    } else {
        serde_json::json!({})
    };

    merge_transparent_env(&mut settings, &base_url)?;

    crate::modules::secure_fs::secure_write_json(&path, &settings)?;
    Ok("Claude Code settings synced (transparent mode).".to_string())
}

/// Pure helper: mutate `settings` JSON to point at the transparent base URL,
/// strip `ANTHROPIC_API_KEY`, and write the standard recommended env flags.
#[cfg(feature = "internal")]
fn merge_transparent_env(
    settings: &mut serde_json::Value,
    base_url: &str,
) -> Result<(), String> {
    let env = settings
        .as_object_mut()
        .ok_or("settings.json is not an object")?
        .entry("env")
        .or_insert(serde_json::json!({}));
    let env_obj = env.as_object_mut().ok_or("env is not an object")?;

    env_obj.insert("ANTHROPIC_BASE_URL".to_string(), serde_json::json!(base_url));
    env_obj.remove("ANTHROPIC_API_KEY");
    env_obj.insert("DISABLE_TELEMETRY".to_string(), serde_json::json!("1"));
    env_obj.insert(
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(),
        serde_json::json!("1"),
    );
    env_obj.insert("ENABLE_PROMPT_CACHING_1H".to_string(), serde_json::json!("1"));
    Ok(())
}

/// Restore Claude Code settings.json to its non-gateway state by removing
/// only the upstream identity fields (`ANTHROPIC_BASE_URL` and
/// `ANTHROPIC_API_KEY`). Telemetry / cache flags previously written are
/// kept since they are user-friendly defaults regardless of routing.
#[tauri::command]
pub async fn restore_claude_settings() -> Result<String, String> {
    let path = get_claude_settings_path()?;

    if !path.exists() {
        return Ok("Claude Code settings restored (no file present).".to_string());
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

    merge_restore_env(&mut settings)?;

    crate::modules::secure_fs::secure_write_json(&path, &settings)?;
    Ok("Claude Code settings restored.".to_string())
}

/// Pure helper: remove the gateway identity env entries but keep any other
/// settings (including telemetry / prompt-cache flags) untouched.
fn merge_restore_env(settings: &mut serde_json::Value) -> Result<(), String> {
    let env = settings
        .as_object_mut()
        .ok_or("settings.json is not an object")?
        .entry("env")
        .or_insert(serde_json::json!({}));
    let env_obj = env.as_object_mut().ok_or("env is not an object")?;
    env_obj.remove("ANTHROPIC_BASE_URL");
    env_obj.remove("ANTHROPIC_API_KEY");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(feature = "internal")]
    #[test]
    fn merge_transparent_sets_base_url_and_removes_api_key() {
        let mut settings = json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://localhost:9000",
                "ANTHROPIC_API_KEY": "sk-gateway-secret",
                "OTHER": "keep",
            }
        });
        merge_transparent_env(&mut settings, "http://localhost:9001").unwrap();
        let env = settings["env"].as_object().unwrap();
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://localhost:9001");
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
        assert_eq!(env["OTHER"], "keep");
    }

    #[cfg(feature = "internal")]
    #[test]
    fn merge_transparent_creates_env_when_missing() {
        let mut settings = json!({});
        merge_transparent_env(&mut settings, "http://localhost:9001").unwrap();
        let env = settings["env"].as_object().unwrap();
        assert_eq!(env["ANTHROPIC_BASE_URL"], "http://localhost:9001");
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
    }

    #[cfg(feature = "internal")]
    #[test]
    fn merge_transparent_writes_recommended_env_flags() {
        let mut settings = json!({});
        merge_transparent_env(&mut settings, "http://localhost:9001").unwrap();
        let env = settings["env"].as_object().unwrap();
        assert_eq!(env["DISABLE_TELEMETRY"], "1");
        assert_eq!(env["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"], "1");
        assert_eq!(env["ENABLE_PROMPT_CACHING_1H"], "1");
    }

    #[cfg(feature = "internal")]
    #[test]
    fn merge_transparent_no_op_when_api_key_already_absent() {
        let mut settings = json!({
            "env": { "ANTHROPIC_BASE_URL": "http://localhost:9000" }
        });
        merge_transparent_env(&mut settings, "http://localhost:9001").unwrap();
        let env = settings["env"].as_object().unwrap();
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn merge_restore_removes_only_base_url_and_api_key() {
        let mut settings = json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://localhost:9000",
                "ANTHROPIC_API_KEY": "sk-gateway",
                "DISABLE_TELEMETRY": "1",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
                "ENABLE_PROMPT_CACHING_1H": "1",
                "OTHER": "keep",
            }
        });
        merge_restore_env(&mut settings).unwrap();
        let env = settings["env"].as_object().unwrap();
        assert!(!env.contains_key("ANTHROPIC_BASE_URL"));
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
        assert_eq!(env["DISABLE_TELEMETRY"], "1");
        assert_eq!(env["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"], "1");
        assert_eq!(env["ENABLE_PROMPT_CACHING_1H"], "1");
        assert_eq!(env["OTHER"], "keep");
    }

    #[test]
    fn merge_restore_no_op_when_already_clean() {
        let mut settings = json!({
            "env": { "DISABLE_TELEMETRY": "1" }
        });
        merge_restore_env(&mut settings).unwrap();
        let env = settings["env"].as_object().unwrap();
        assert!(!env.contains_key("ANTHROPIC_BASE_URL"));
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
        assert_eq!(env["DISABLE_TELEMETRY"], "1");
    }

    #[test]
    fn merge_restore_creates_env_when_missing() {
        let mut settings = json!({});
        merge_restore_env(&mut settings).unwrap();
        let env = settings["env"].as_object().unwrap();
        assert!(env.is_empty());
    }
}
