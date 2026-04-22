//! Account management IPC commands — CRUD, profile update, web login, add+import.

use crate::GatewayServiceState;
use crate::subprocess;

/// Update Account.web after successful web login.
#[tauri::command]
pub async fn update_web_login(
    account_id: String,
    cookies: serde_json::Value,
    session_key: String,
    proxy: Option<serde_json::Value>,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    let is_empty = session_key.is_empty()
        && matches!(&cookies, serde_json::Value::Array(arr) if arr.is_empty());

    if is_empty {
        state.account_manager.update(&account_id, |a| { a.web = None; }).await
    } else {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let cookie_vec: Vec<crate::models::account::CookieData> =
            serde_json::from_value(cookies).unwrap_or_default();
        let proxy_val = proxy.clone();
        state.account_manager.update(&account_id, |a| {
            a.web = Some(crate::models::account::WebClient {
                cookies: cookie_vec,
                local_storage: std::collections::HashMap::new(),
                last_activity: Some(now_ms),
            });
            if let Some(ref pv) = proxy_val {
                if !pv.is_null() {
                    if let Ok(ps) = serde_json::from_value::<crate::models::account::ProxySection>(pv.clone()) {
                        a.proxy = Some(ps);
                    }
                }
            }
        }).await
    }
}

/// Update account profile fields after web login.
#[tauri::command]
pub async fn update_account_profile(
    account_id: String,
    email: Option<String>,
    account_uuid: Option<String>,
    org_id: Option<String>,
    full_name: Option<String>,
    subscription_type: Option<String>,
    rate_limit_tier: Option<String>,
    billing_type: Option<String>,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    state.account_manager.update(&account_id, |a| {
        if let Some(ref e) = email { if !e.is_empty() { a.email = e.to_lowercase(); } }
        if let Some(ref u) = account_uuid { if !u.is_empty() { a.account_uuid = u.clone(); } }
        if let Some(ref o) = org_id { if !o.is_empty() { a.org_id = o.clone(); } }
        if let Some(ref n) = full_name { if !n.is_empty() { a.full_name = n.clone(); } }
        if let Some(ref s) = subscription_type { if !s.is_empty() { a.subscription_type = s.clone(); } }
        else if a.subscription_type == "unknown" { a.subscription_type = "free".to_string(); }
        if let Some(ref r) = rate_limit_tier { a.rate_limit_tier = Some(r.clone()); }
        if let Some(ref b) = billing_type { a.billing_type = Some(b.clone()); }
    }).await
}

/// Mark an account as disabled (banned/suspended).
#[tauri::command]
pub async fn mark_account_disabled(
    account_id: String,
    reason: String,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    use crate::modules::account_change::AccountChange;
    let change = AccountChange::Disabled { reason: reason.clone() };
    state.account_manager.apply_change(&account_id, |a| {
        a.disabled = true;
        a.disabled_reason = Some(reason);
        a.disabled_at = Some(now_ms);
    }, change).await
}

/// Notify that an account has been fully created (web login + profile written).
/// Routes the change to runtime consumers (ClientManager, frontend).
#[tauri::command]
pub async fn notify_account_created(
    account_id: String,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    use crate::modules::account_change::AccountChange;
    state.account_manager.route_change(&account_id, AccountChange::Created).await;
    Ok(())
}

// ── Add Account ────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAccountResult {
    pub account_id: String,
    pub task_id: String,
}

/// Create a minimal Account JSON on disk + spawn web login subprocess.
#[tauri::command]
pub async fn add_account_and_login(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<AddAccountResult, String> {
    // Clean up orphan empty accounts
    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64 - 600_000;
    if let Ok(all) = crate::models::account::list_accounts_in_dir(
        &state.account_manager.accounts_dir(),
    ) {
        for a in all {
            if a.android.is_none() && a.web.is_none() && a.cli.is_none() && a.created_at < cutoff {
                let _ = state.account_manager.delete(&a.account_id).await;
            }
        }
    }

    let account_id = uuid::Uuid::now_v7().to_string();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let account = crate::models::account::Account {
        account_id: account_id.clone(),
        email: String::new(),
        phone_number: String::new(),
        full_name: String::new(),
        custom_label: None,
        account_uuid: String::new(),
        org_id: String::new(),
        region: "US".to_string(),
        created_at: now_ms,
        subscription_type: "unknown".to_string(),
        login_method: "manual".to_string(),
        rate_limit_tier: None,
        subscription_renew_at: None,
        subscription_created_at: None,
        billing_type: None,
        has_extra_usage_enabled: false,
        disabled: false,
        disabled_reason: None,
        disabled_at: None,
        user_disabled: false,
        user_disabled_reason: None,
        user_disabled_at: None,
        proxy: None,
        utilization: None,
        android: None,
        web: None,
        cli: None,
    };

    state.account_manager.write_new(&account).await?;

    let (command, mut args) = subprocess::get_webapp_command();
    args.push("login".to_string());
    args.push(format!("--id={}", account_id));
    args.push("--app=manager".to_string());
    args.push("--auto".to_string());

    let task_id = format!("web-login-{}", account_id);
    super::subprocess_cmd::abort_if_running(&state.subprocess_manager, &task_id).await?;

    state
        .subprocess_manager
        .spawn_and_watch(
            task_id.clone(),
            account_id.clone(),
            subprocess::SubprocessType::Webapp,
            "web-login".to_string(),
            &command,
            args,
            app_handle,
            state.account_manager.clone(),
        )
        .await?;

    Ok(AddAccountResult { account_id, task_id })
}

// ── Finalize Account Import ────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthTokenData {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    #[serde(default)]
    pub scopes: String,
    #[serde(default)]
    pub cookies: Vec<serde_json::Value>,
    #[serde(default)]
    pub session_key: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeResult {
    pub email: String,
    pub subscription_type: String,
    pub proxy_ip: Option<String>,
}

/// Update existing minimal Account with CLI tokens + profile info + allocate proxy + hot-load.
#[tauri::command]
pub async fn finalize_account_import(
    account_id: String,
    email: String,
    token: OAuthTokenData,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<FinalizeResult, String> {
    let scopes: Vec<String> = if token.scopes.is_empty() {
        vec![
            "user:inference".into(),
            "user:profile".into(),
            "user:sessions:claude_code".into(),
            "user:mcp_servers".into(),
            "user:file_upload".into(),
        ]
    } else {
        token.scopes.split_whitespace().map(|s| s.to_string()).collect()
    };

    let cli_cred = crate::models::account::CliClient {
        access_token: token.access_token.clone(),
        refresh_token: token.refresh_token.clone(),
        expires_at: token.expires_at,
        scopes,
        last_activity: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        ),
        device_id: crate::models::account::generate_device_id(),
    };

    // proxy_url: Direct mode returns None (no proxy)
    let proxy_url = state.proxy_allocator.get_proxy_url_optional(&account_id).await
        .map_err(|e| format!("get_proxy_url: {}", e))?;

    let boring = claude_ultra_http::BoringClient::builder()
        .build()
        .map_err(|e| format!("BoringClient init failed: {}", e))?;
    let client = crate::modules::cli_client::CliClient::new(
        std::sync::Arc::new(boring),
        account_id.clone(),
        token.access_token.clone(),
        token.refresh_token.clone(),
        token.expires_at,
        proxy_url,
    );

    let profile = client
        .get_profile()
        .await
        .map_err(|e| format!("get_profile failed: {}", e))?;

    if !profile.email.is_empty() && profile.email != email {
        return Err(format!(
            "Email mismatch: account says '{}' but profile says '{}'. Wrong account?",
            email, profile.email
        ));
    }

    let sub_type = profile.subscription_type.clone().unwrap_or_else(|| "free".into());
    let sub_type_for_result = sub_type.clone();

    let profile_display_name = profile.display_name.unwrap_or_default();
    let profile_account_uuid = profile.account_uuid;
    let profile_org_uuid = profile.organization_uuid;
    let profile_rate_limit_tier = profile.rate_limit_tier;
    let profile_billing_type = profile.billing_type;
    let profile_has_extra = profile.has_extra_usage_enabled;

    let web_client = if !token.cookies.is_empty() {
        let cookies: Vec<crate::models::account::CookieData> = token.cookies.iter().filter_map(|c| {
            serde_json::from_value(c.clone()).ok()
        }).collect();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        Some(crate::models::account::WebClient {
            cookies,
            local_storage: std::collections::HashMap::new(),
            last_activity: Some(now_ms),
        })
    } else {
        None
    };

    state
        .account_manager
        .update(&account_id, move |a| {
            a.full_name = profile_display_name;
            a.account_uuid = profile_account_uuid;
            a.org_id = profile_org_uuid;
            a.subscription_type = sub_type;
            a.rate_limit_tier = profile_rate_limit_tier;
            a.billing_type = profile_billing_type;
            a.has_extra_usage_enabled = profile_has_extra;
            a.cli = Some(cli_cred);
            if let Some(web) = web_client {
                a.web = Some(web);
            }
        })
        .await?;

    let proxy_ip = match state.proxy_allocator.allocate(&account_id).await {
        Ok(section) => section.last_ip,
        Err(e) => {
            tracing::warn!("Proxy alloc failed for {}: {}", account_id, e);
            None
        }
    };

    // Notify consumers: account is ready with CLI token
    use crate::modules::account_change::AccountChange;
    state.account_manager.route_change(&account_id, AccountChange::Created).await;

    Ok(FinalizeResult {
        email,
        subscription_type: sub_type_for_result,
        proxy_ip,
    })
}
