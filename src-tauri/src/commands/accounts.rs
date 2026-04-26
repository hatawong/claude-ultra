//! Account management IPC commands — CRUD, route update, add-account flow.

use crate::GatewayServiceState;
use crate::models::account::StaticProxy;
use crate::subprocess;

/// Validate update_account_route inputs against schema invariants.
/// Pulled out as a free function so unit tests can cover edge cases without
/// constructing a full Tauri State.
pub(crate) fn validate_route_update(
    route_mode: Option<&str>,
    has_static_proxy_arg: bool,
    clear_static_proxy: bool,
    existing_static_proxy: Option<&StaticProxy>,
) -> Result<(), String> {
    if has_static_proxy_arg && clear_static_proxy {
        return Err("static_proxy and clear_static_proxy are mutually exclusive".to_string());
    }
    if let Some("static") = route_mode {
        if clear_static_proxy {
            return Err("cannot clear static_proxy while setting route_mode=static".to_string());
        }
        if !has_static_proxy_arg && existing_static_proxy.is_none() {
            return Err("route_mode=static requires staticProxy".to_string());
        }
    }
    Ok(())
}

/// Update account route_mode / route_country / static_proxy.
/// ClientManager cache sync handled by route_change(RouteUpdated).
///
/// Static proxy update semantics:
/// - `static_proxy = Some(sp), clear_static_proxy = None/Some(false)` → set
/// - `static_proxy = None,     clear_static_proxy = None/Some(false)` → keep
/// - `static_proxy = None,     clear_static_proxy = Some(true)`       → clear
/// - `static_proxy = Some(_),  clear_static_proxy = Some(true)`       → Err (mutually exclusive)
/// - `route_mode = "static"` requires staticProxy (incoming or already on disk).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_account_route(
    account_id: String,
    route_mode: Option<String>,
    route_country: Option<String>,
    static_proxy: Option<StaticProxy>,
    clear_static_proxy: Option<bool>,
    // probed_proxy: pre-tested ProxySection snapshot (from successful Test)
    // to commit atomically with route_mode + staticProxy. UI captures this
    // from the Save path after Test passes; backend writes it via set_proxy.
    probed_proxy: Option<crate::models::account::ProxySection>,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    let want_clear = clear_static_proxy.unwrap_or(false);

    // Read existing for invariant check (before any mutation).
    let existing = if route_mode.as_deref() == Some("static") && static_proxy.is_none() && !want_clear {
        Some(state.account_manager.read(&account_id).await
            .map_err(|e| format!("read account {}: {}", account_id, e))?)
    } else {
        None
    };
    validate_route_update(
        route_mode.as_deref(),
        static_proxy.is_some(),
        want_clear,
        existing.as_ref().and_then(|a| a.static_proxy.as_ref()),
    )?;

    // Bundle all four fields into a single atomic write so concurrent
    // callers (Detail Save / Add retry / probe refresh) cannot interleave
    // and produce mixed routeMode/staticProxy/proxy states.
    let sp_update = if static_proxy.is_some() || want_clear {
        Some(if want_clear { None } else { static_proxy })
    } else {
        None
    };
    let proxy_update: Option<Option<crate::models::account::ProxySection>> = if want_clear {
        // clear_static_proxy=true: clear the snapshot — it described the
        // static IP that no longer applies. Avoids stale 'static-host-port'
        // sessId leaking into the dynamic path.
        Some(None)
    } else {
        // probed_proxy=Some: commit the freshly-tested snapshot atomically
        // with the route_mode/staticProxy change (Save path after a passing
        // Test). probed_proxy=None: leave a.proxy untouched.
        probed_proxy.map(Some)
    };
    let rc_update = route_country.map(Some);
    state.account_manager
        .set_route_bundle(&account_id, route_mode, rc_update, sp_update, proxy_update)
        .await
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
/// If `account_id` is provided, reuse existing shell (retry path) and
/// rewrite routing fields based on the user's latest intent: route_mode,
/// static_proxy, and a probed_proxy snapshot are all overwritten so the
/// behaviour matches update_account_route. The user may have changed mind
/// between attempts (e.g. proxy → static → proxy).
///
/// Otherwise generate new UUIDv7 + create shell (first attempt). If
/// route_mode == "static", static_proxy must be Some, and probed_proxy
/// (a freshly-tested ProxySection from test_static_proxy_dryrun) is
/// committed to a.proxy in the same write so the UI reflects the static IP
/// snapshot immediately.
#[tauri::command]
pub async fn add_account_and_login(
    account_id: Option<String>,
    route_mode: Option<String>,
    static_proxy: Option<StaticProxy>,
    probed_proxy: Option<crate::models::account::ProxySection>,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<AddAccountResult, String> {
    let rm = route_mode.unwrap_or_else(|| "proxy".to_string());
    if rm == "static" && static_proxy.is_none() {
        return Err("route_mode=static requires staticProxy".to_string());
    }

    // Stop any previous web-add subprocess BEFORE mutating disk state. If
    // abort fails, no Account JSON has been touched yet — user can retry
    // without leaving half-applied routing fields behind.
    let task_id = "web-add".to_string();
    super::subprocess_cmd::abort_if_running(&state.subprocess_manager, &task_id).await?;

    let account_id = if let Some(id) = account_id {
        // Retry path: ALWAYS rewrites the three routing fields from incoming
        // arguments — absent staticProxy means "clear", absent probedProxy
        // means "do not set" when staticProxy is Some, "clear" when None.
        // Differs from update_account_route (which keeps fields when caller
        // passes None). Frontend always carries full state forward.
        state.account_manager.read(&id).await
            .map_err(|e| format!("account_id {} not found: {}", id, e))?;

        let proxy_update: Option<Option<crate::models::account::ProxySection>> = if static_proxy.is_none() {
            // No staticProxy → clear ProxySection too (was a static snapshot).
            Some(None)
        } else if let Some(snapshot) = probed_proxy.clone() {
            // Fresh probe → commit atomically with staticProxy.
            Some(Some(snapshot))
        } else {
            // staticProxy=Some but no probe → preserve existing a.proxy.
            None
        };
        state.account_manager
            .set_route_bundle(
                &id,
                Some(rm.clone()),
                None,
                Some(static_proxy.clone()),
                proxy_update,
            ).await?;

        id
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

        // Initial country comes from config.proxy.default_country.
        let default_country = crate::models::config::load_app_config()
            .ok()
            .map(|c| c.proxy.default_country.to_lowercase())
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
            proxy: probed_proxy.clone(),
            static_proxy,
            utilization: None,
            route_mode: rm.clone(),
            route_country: None,
            android: None,
            web: None,
            cli: None,
        };

        state.account_manager.write_new(&account).await?;
        new_id
    };

    // Re-read the account to resolve country (retry path mutated route_mode
    // / static_proxy independently; first-attempt path also benefits from
    // the canonical disk view).
    let acc_for_country = state.account_manager.read(&account_id).await
        .map_err(|e| format!("read account {} for country: {}", account_id, e))?;
    let country = acc_for_country.resolve_country();
    let (command, mut args) = subprocess::get_webapp_command();
    args.push("login".to_string());
    args.push(format!("--id={}", account_id));
    args.push("--app=manager".to_string());
    args.push("--auto".to_string());
    args.push(format!("--country={}", country));

    // web-add is a global singleton task — only one AddAccount operation at a
    // time. abort_if_running was already called at the top of this function
    // before any disk mutation; just spawn the new task here.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_static_proxy() -> StaticProxy {
        StaticProxy {
            protocol: "socks5".to_string(),
            host: "h.example".to_string(),
            port: 1080,
            username: "u".to_string(),
            password: "p".to_string(),
        }
    }

    #[test]
    fn test_validate_set_and_clear_mutually_exclusive() {
        let err = validate_route_update(
            Some("static"),
            true,           // has static_proxy arg
            true,           // also clear
            None,
        ).unwrap_err();
        assert!(err.contains("mutually exclusive"));
    }

    #[test]
    fn test_validate_static_requires_static_proxy_when_none_existing() {
        let err = validate_route_update(
            Some("static"),
            false,          // no incoming static_proxy
            false,
            None,           // and no existing on disk
        ).unwrap_err();
        assert!(err.contains("requires staticProxy"));
    }

    #[test]
    fn test_validate_static_ok_with_incoming_static_proxy() {
        validate_route_update(Some("static"), true, false, None).unwrap();
    }

    #[test]
    fn test_validate_static_ok_with_existing_static_proxy() {
        let sp = sample_static_proxy();
        validate_route_update(Some("static"), false, false, Some(&sp)).unwrap();
    }

    #[test]
    fn test_validate_clear_ok_when_not_setting_static() {
        validate_route_update(Some("proxy"), false, true, None).unwrap();
    }

    #[test]
    fn test_validate_cannot_clear_while_setting_static() {
        let err = validate_route_update(Some("static"), false, true, None).unwrap_err();
        assert!(err.contains("cannot clear"));
    }

    #[test]
    fn test_validate_keep_existing_when_no_args() {
        // route_mode unchanged, no static_proxy ops → ok
        validate_route_update(None, false, false, None).unwrap();
    }
}

