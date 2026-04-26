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

    // Pre-approve the API key fingerprint and mark onboarding complete in
    // ~/.claude.json so a freshly-installed Claude Code CLI starts directly
    // into REPL: skips ApproveApiKey dialog (custom key prompt) and the
    // welcome/onboarding flow. Merge mode preserves any pre-existing user
    // state (theme, oauthAccount, recentProjects, etc).
    //
    // Failure here is non-fatal: settings.json is already written. Surface
    // the partial-success state so the user can investigate.
    match prepare_claude_json_for_gateway(&api_key) {
        Ok(()) => Ok("Claude Code settings synced.".to_string()),
        Err(e) => Ok(format!(
            "Claude Code settings.json synced; ~/.claude.json pre-approve failed: {}. \
             First-run ApproveApiKey dialog may still appear in CC CLI.",
            e,
        )),
    }
}

fn get_claude_settings_path() -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot find home directory")?;
    Ok(home.join(".claude").join("settings.json"))
}

fn get_claude_json_path() -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot find home directory")?;
    Ok(home.join(".claude.json"))
}

/// CC's fingerprint convention — last 20 chars of the API key.
/// `api_key.len()` is byte count; slicing by byte index would panic when the
/// last 20 bytes start mid–UTF-8 sequence. Iterate by char so non-ASCII
/// inputs (paste accident, emoji) cannot crash the IPC command.
fn api_key_fingerprint(api_key: &str) -> String {
    let chars: Vec<char> = api_key.chars().collect();
    if chars.len() <= 20 {
        return api_key.to_string();
    }
    chars[chars.len() - 20..].iter().collect()
}

/// Pure merger: set hasCompletedOnboarding=true and ensure fingerprint is
/// in customApiKeyResponses.approved. Never touches oauthAccount or other
/// fields. Returns Err only when the existing JSON has a wrong shape (root
/// not object, customApiKeyResponses not object, approved not array) —
/// callers should treat as fatal and not silently reset.
fn apply_claude_json_pre_approve(
    json: &mut serde_json::Value,
    fingerprint: &str,
) -> Result<(), String> {
    let obj = json
        .as_object_mut()
        .ok_or("~/.claude.json root is not an object")?;

    obj.insert("hasCompletedOnboarding".to_string(), serde_json::json!(true));

    let responses = obj
        .entry("customApiKeyResponses")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("customApiKeyResponses is not an object")?;
    let approved = responses
        .entry("approved")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or("customApiKeyResponses.approved is not an array")?;
    if !approved.iter().any(|v| v.as_str() == Some(fingerprint)) {
        approved.push(serde_json::json!(fingerprint));
    }
    Ok(())
}

/// Ensure ~/.claude.json contains hasCompletedOnboarding=true and the API
/// key's last-20-char fingerprint in customApiKeyResponses.approved.
///
/// Strict merge mode:
/// - Refuses to overwrite a corrupted file (parse error → Err); the user
///   may have repairable state (oauthAccount, OAuth tokens via
///   secureStorage, etc.) that a silent reset would invalidate.
/// - Never touches oauthAccount, theme, recentProjects or any other field.
/// - approved is append-only with dedup; the array may already contain
///   fingerprints from prior keys or from the CC ApproveApiKey dialog.
fn prepare_claude_json_for_gateway(api_key: &str) -> Result<(), String> {
    let path = get_claude_json_path()?;

    let mut json: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {}", path.display(), e))?;
        serde_json::from_str(&content).map_err(|e| {
            format!(
                "parse {}: {} (fix manually or delete to reset)",
                path.display(),
                e,
            )
        })?
    } else {
        serde_json::json!({})
    };

    apply_claude_json_pre_approve(&mut json, &api_key_fingerprint(api_key))?;

    crate::modules::secure_fs::secure_write_json(&path, &json)?;
    Ok(())
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

    // ── ~/.claude.json pre-approve helpers ──────────────────────

    #[test]
    fn fingerprint_takes_last_20_chars() {
        let fp = api_key_fingerprint("sk-ultra-deadbeefdeadbeefdeadbeefdeadbeef");
        assert_eq!(fp, "beefdeadbeefdeadbeef");
    }

    #[test]
    fn fingerprint_short_key_returns_full() {
        assert_eq!(api_key_fingerprint("short"), "short");
    }

    #[test]
    fn fingerprint_non_ascii_does_not_panic() {
        // Multi-byte chars in the trailing window must not panic the IPC.
        // The exact tail content is not asserted (CC's slice(-20) is in JS
        // chars and inputs are ASCII in practice); only the no-panic
        // contract matters here.
        let key: String = "x".repeat(10) + &"中".repeat(15);
        let fp = api_key_fingerprint(&key);
        assert_eq!(fp.chars().count(), 20);
    }

    #[test]
    fn pre_approve_writes_to_empty_json() {
        let mut j = json!({});
        apply_claude_json_pre_approve(&mut j, "abc123").unwrap();
        assert_eq!(j["hasCompletedOnboarding"], json!(true));
        assert_eq!(j["customApiKeyResponses"]["approved"], json!(["abc123"]));
    }

    #[test]
    fn pre_approve_preserves_oauth_account_and_other_state() {
        let mut j = json!({
            "theme": "dark",
            "numStartups": 42,
            "oauthAccount": {
                "accountUuid": "uuid-keep",
                "emailAddress": "user@x.com",
            },
            "recentProjects": ["/a", "/b"],
            "hasCompletedOnboarding": true,
        });
        apply_claude_json_pre_approve(&mut j, "fp").unwrap();
        assert_eq!(j["theme"], "dark");
        assert_eq!(j["numStartups"], 42);
        assert_eq!(j["oauthAccount"]["accountUuid"], "uuid-keep");
        assert_eq!(j["oauthAccount"]["emailAddress"], "user@x.com");
        assert_eq!(j["recentProjects"], json!(["/a", "/b"]));
        assert_eq!(j["hasCompletedOnboarding"], true);
        assert_eq!(j["customApiKeyResponses"]["approved"], json!(["fp"]));
    }

    #[test]
    fn pre_approve_dedup_appends_when_missing_keeps_others() {
        let mut j = json!({
            "customApiKeyResponses": {
                "approved": ["existing1", "existing2"],
                "rejected": ["bad-fp"],
            },
        });
        apply_claude_json_pre_approve(&mut j, "newfp").unwrap();
        assert_eq!(
            j["customApiKeyResponses"]["approved"],
            json!(["existing1", "existing2", "newfp"]),
        );
        assert_eq!(j["customApiKeyResponses"]["rejected"], json!(["bad-fp"]));
    }

    #[test]
    fn pre_approve_dedup_skips_when_present() {
        let mut j = json!({
            "customApiKeyResponses": {
                "approved": ["alreadyhere"],
            },
        });
        apply_claude_json_pre_approve(&mut j, "alreadyhere").unwrap();
        assert_eq!(
            j["customApiKeyResponses"]["approved"],
            json!(["alreadyhere"]),
        );
    }

    #[test]
    fn pre_approve_rejects_non_object_root() {
        let mut j = json!([1, 2, 3]);
        let err = apply_claude_json_pre_approve(&mut j, "fp").unwrap_err();
        assert!(err.contains("not an object"));
    }
}
