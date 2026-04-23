//! Account management IPC commands — CRUD, route update, add-account flow.

use crate::GatewayServiceState;
use crate::subprocess;

/// Update account route_mode / route_country.
/// ClientManager cache sync handled by route_change(RouteUpdated).
#[tauri::command]
pub async fn update_account_route(
    account_id: String,
    route_mode: Option<String>,
    route_country: Option<String>,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    state.account_manager.set_route(&account_id, route_mode, route_country).await
}

// ── Add Account ────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAccountResult {
    pub account_id: String,
    pub task_id: String,
}

/// Create a minimal Account JSON on disk + spawn web login subprocess.
///
/// If `account_id` is provided, reuse existing shell (retry path).
/// Otherwise generate new UUIDv7 + create shell (first attempt).
#[tauri::command]
pub async fn add_account_and_login(
    account_id: Option<String>,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<AddAccountResult, String> {
    let (account_id, account) = if let Some(id) = account_id {
        let acc = state.account_manager.read(&id).await
            .map_err(|e| format!("account_id {} not found: {}", id, e))?;
        (id, acc)
    } else {
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

        let new_id = uuid::Uuid::now_v7().to_string();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Read initial country from config.proxy.residential.default_country
        let default_country = crate::models::config::load_app_config()
            .ok()
            .map(|c| c.proxy.residential.default_country.to_lowercase())
            .unwrap_or_else(|| "us".to_string());

        let account = crate::models::account::Account {
            account_id: new_id.clone(),
            email: String::new(),
            phone_number: String::new(),
            full_name: String::new(),
            custom_label: None,
            account_uuid: String::new(),
            org_id: String::new(),
            country: Some(default_country),
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
            route_mode: "proxy".to_string(),
            route_country: None,
            android: None,
            web: None,
            cli: None,
        };

        state.account_manager.write_new(&account).await?;
        (new_id, account)
    };

    let country = account.resolve_country();
    let (command, mut args) = subprocess::get_webapp_command();
    args.push("login".to_string());
    args.push(format!("--id={}", account_id));
    args.push("--app=manager".to_string());
    args.push("--auto".to_string());
    args.push(format!("--country={}", country));

    // web-add is a global singleton task — only one AddAccount operation at a time.
    let task_id = "web-add".to_string();
    super::subprocess_cmd::abort_if_running(&state.subprocess_manager, &task_id).await?;

    state
        .subprocess_manager
        .spawn_and_watch(
            task_id.clone(),
            account_id.clone(),
            subprocess::SubprocessType::Webapp,
            "web-add".to_string(),
            &command,
            args,
            app_handle,
            state.account_manager.clone(),
        )
        .await?;

    Ok(AddAccountResult { account_id, task_id })
}

