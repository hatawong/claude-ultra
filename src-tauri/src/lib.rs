pub mod models;
pub mod modules;
pub mod commands;
pub mod gateway;
pub mod proxy;
mod subprocess;
mod tray;

// Re-export Account for integration tests
pub use models::account::Account;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;
use tokio::sync::RwLock;

use modules::account_manager::AccountManager;
use modules::account_monitor::AccountMonitor;
use modules::token_allocator::TokenAllocator;
use modules::server_client::ServerClient;
use models::config::{AppConfig, GatewayConfig, ProxyMode};
use proxy::allocator::ProxyAllocator;
use proxy::config::ProxyProviderConfig;
use gateway::security::SecurityState;
use modules::security_db;
use gateway::server::GatewayInstance;
use modules::client_manager::ClientManager;
use subprocess::SubprocessManager;

/// Managed state for the gateway service.
pub struct GatewayServiceState {
    pub instance: RwLock<Option<GatewayInstance>>,
    #[cfg(feature = "internal")]
    pub transparent_instance: RwLock<Option<GatewayInstance>>,
    pub client_manager: Arc<ClientManager>,
    pub account_manager: Arc<AccountManager>,
    pub proxy_allocator: Arc<ProxyAllocator>,
    pub proxy_provider_config: Arc<ProxyProviderConfig>,
    pub token_allocator: Arc<TokenAllocator>,
    pub subprocess_manager: Arc<SubprocessManager>,
    pub security_state: Option<SecurityState>,
    pub gateway_config: RwLock<GatewayConfig>,
    /// Shared logging flag — updated by set_logging_enabled, read by handler AtomicBool
    pub enable_logging: Arc<AtomicBool>,
    pub gateway_db: Option<Arc<modules::gateway_db::GatewayDb>>,
    pub boring_client: Arc<claude_ultra_http::BoringClient>,
    /// Server API client (reqwest direct, not BoringClient)
    pub server_client: Arc<ServerClient>,
    /// Guards init_services() idempotency — true after first successful init
    pub services_initialized: Arc<AtomicBool>,
    /// AccountMonitor instance — held for stop() on shutdown
    pub account_monitor: RwLock<Option<Arc<AccountMonitor>>>,
    /// Proxy mode: Proxied (with credentials) or Direct (no proxy, user's real IP)
    pub proxy_mode: ProxyMode,
}

/// Build ProxyProviderConfig from AppConfig.
/// Credentials (kind/username/password/host/port) come from proxy.residential,
/// while default_country reads top-level proxy.default_country (unified field, same level as default_type).
pub fn proxy_provider_config_from(app_config: &AppConfig) -> ProxyProviderConfig {
    let r = &app_config.proxy.residential;
    ProxyProviderConfig {
        kind: r.kind.clone(),
        username: r.username.clone(),
        password: r.password.clone(),
        host: r.host.clone(),
        port: r.port,
        default_country: app_config.proxy.default_country.clone(),
        max_allocate_retries: 3,
    }
}

impl GatewayServiceState {
    /// Synchronous constructor: creates data structures only, no async tasks.
    /// Operations requiring tokio runtime (ProxyPool background tasks, AccountMonitor, load_clients)
    /// are deferred to async spawn in setup(). Do not .await in this function.
    fn new(gateway_db: Option<Arc<modules::gateway_db::GatewayDb>>, app_config: &AppConfig) -> Self {
        let config = app_config.gateway.clone();
        let accounts_dir = get_accounts_dir().unwrap_or_else(|_| {
            let home = dirs::home_dir().expect("Cannot find home directory");
            home.join(".claude-ultra").join("accounts")
        });

        // NOTE: Legacy `region` → `country` migration removed (no live users
        // carry it; existing JSON cleaned manually via `jq 'del(.region)'`).

        let account_manager = Arc::new(AccountManager::new(accounts_dir));
        let provider_config = Arc::new(proxy_provider_config_from(app_config));
        let proxy_allocator = Arc::new(ProxyAllocator::new(
            account_manager.clone(),
            provider_config.clone(),
        ));

        // Initialize security state from SQLite (uses shared GatewayDb)
        let security_state = gateway_db.as_ref().and_then(|db| {
            match SecurityState::new(db.clone()) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::error!("SecurityState init failed: {}", e);
                    None
                }
            }
        });

        let boring_client = Arc::new(
            claude_ultra_http::BoringClient::builder()
                .build()
                .expect("Failed to build BoringClient"),
        );

        // session affinity persistence — load existing sticky sessions
        // (with TTL prune) so a restart does not roam active sessions.
        // start_flush_task is deferred to init_services_inner (async ctx)
        // since it spawns a tokio task.
        let sessions_path = dirs::home_dir()
            .map(|h| h.join(".claude-ultra").join("sessions.json"));
        let client_manager = {
            let mut cm = ClientManager::new(boring_client.clone());
            if let Some(p) = sessions_path {
                cm = cm.with_sessions_path(p);
            }
            Arc::new(cm)
        };

        // ProxyPool: app-level singleton, same lifetime as GatewayServiceState
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let proxy_pool = proxy::pool::ProxyPool::new(
            proxy::pool::PoolConfig::default(),
            boring_client.clone(),
            proxy_allocator.clone(),
            proxy::pool::GeoPolicy {
                expected_country: provider_config.default_country.clone(),
            },
            cancel_token,
        );
        // start_background_tasks requires tokio runtime, deferred to async context
        proxy_allocator.set_pool(proxy_pool);

        let token_allocator = Arc::new(TokenAllocator::new(
            account_manager.clone(),
            proxy_allocator.clone(),
            boring_client.clone(),
        ));

        let server_client = Arc::new(ServerClient::new(app_config.server_url.clone()));

        Self {
            instance: RwLock::new(None),
            #[cfg(feature = "internal")]
            transparent_instance: RwLock::new(None),
            client_manager,
            account_manager,
            proxy_allocator,
            proxy_provider_config: provider_config,
            token_allocator,
            subprocess_manager: Arc::new(SubprocessManager::new()),
            security_state,
            enable_logging: Arc::new(AtomicBool::new(config.enable_logging)),
            gateway_config: RwLock::new(config),
            gateway_db,
            boring_client,
            server_client,
            services_initialized: Arc::new(AtomicBool::new(false)),
            account_monitor: RwLock::new(None),
            proxy_mode: models::config::get_proxy_mode(app_config),
        }
    }
}

fn get_accounts_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot find home directory")?;
    Ok(home.join(".claude-ultra").join("accounts"))
}

/// Fetch license from server and inject into global guard.
/// Used at startup (bootstrap) and by periodic refresh task.
#[cfg(not(feature = "internal"))]
async fn refresh_license_once(server_client: &Arc<ServerClient>) -> Result<(), String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LicenseRefreshResponse {
        license_token: String,
        plan: String,
        max_accounts: u32,
        plan_expires_at: Option<u64>,
    }

    let resp: LicenseRefreshResponse = server_client
        .get("/license/refresh")
        .await?;

    // Set global license guard
    claude_ultra_http::license::set_license(&resp.license_token)
        .map_err(|e| format!("License invalid: {}", e))?;

    // Update auth state (plan/maxAccounts may have changed)
    if let Some(mut auth) = server_client.get_auth_state() {
        auth.license_token = Some(resp.license_token);
        auth.user.plan = resp.plan;
        auth.user.max_accounts = resp.max_accounts;
        auth.user.plan_expires_at = resp.plan_expires_at;
        server_client.set_auth_state(auth)?;
    }

    Ok(())
}

/// Check if a Server error string indicates auth failure (401/403).
/// Server returns errors as "CODE: message" format.
#[cfg(not(feature = "internal"))]
fn is_auth_error(err: &str) -> bool {
    err.starts_with("AUTH_REQUIRED:") || err.starts_with("AUTH_INVALID:") || err.starts_with("INVALID_TOKEN:")
}

/// Second-stage initialization: start all background services.
/// Called automatically in internal mode, or by frontend after login in distribution mode.
/// Idempotent — repeated calls are no-ops.
pub async fn init_services_inner(handle: &tauri::AppHandle) -> Result<(), String> {
    let state = handle.state::<GatewayServiceState>();

    // Idempotency guard
    if state.services_initialized.swap(true, Ordering::SeqCst) {
        tracing::info!("Services already initialized, skipping");
        return Ok(());
    }

    // ── Clean orphan shell accounts (no credentials, older than 60s) ──
    {
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64 - 60_000;
        if let Ok(all) = models::account::list_accounts_in_dir(
            &state.account_manager.accounts_dir(),
        ) {
            let mut cleaned = 0u32;
            for a in all {
                if a.android.is_none() && a.web.is_none() && a.cli.is_none() && a.created_at < cutoff {
                    let _ = state.account_manager.delete(&a.account_id).await;
                    cleaned += 1;
                }
            }
            if cleaned > 0 {
                tracing::info!("Cleaned {} orphan shell account(s) on startup", cleaned);
            }
        }
    }

    // ── License injection (distribution mode only) ──
    #[cfg(not(feature = "internal"))]
    {
        // Try local license first, then force refresh from server
        let mut license_ok = false;
        if let Some(ref auth) = state.server_client.get_auth_state() {
            if let Some(ref lt) = auth.license_token {
                if claude_ultra_http::license::set_license(lt).is_ok() {
                    tracing::info!("License loaded from local auth.json");
                    license_ok = true;
                }
            }
        }
        if !license_ok {
            tracing::info!("No valid local license, fetching from server...");
            match refresh_license_once(&state.server_client).await {
                Ok(_) => tracing::info!("License fetched from server"),
                Err(e) => tracing::warn!("License fetch failed: {} (services will start but Gateway proxy will be limited)", e),
            }
        }
    }

    let accounts_dir = state.account_manager.accounts_dir().to_path_buf();

    // session affinity persistence — debounced flush task (needs tokio
    // runtime). Lives across all proxy modes since session_map is mode-
    // independent.
    state.client_manager.start_flush_task();
    tracing::info!("ClientManager session-affinity flush task started");

    // ProxyPool + network check only in Proxied mode
    if state.proxy_mode == ProxyMode::Proxied {
        // Start ProxyPool background tasks (needs tokio runtime)
        if let Some(pool) = state.proxy_allocator.pool() {
            pool.start_background_tasks();
            tracing::info!("ProxyPool background tasks started");
        }

        // Network check + IP service probe (must complete before other components)
        {
            let up = state.proxy_allocator.check_local_network().await;
            state.proxy_allocator.set_network_up(up);
            if up {
                state.proxy_allocator.probe_ip_services().await;
            } else {
                tracing::warn!("Local network down at startup, skipping IP service probe");
            }
        }
    } else {
        tracing::info!("Direct mode (no proxy credentials) — skipping ProxyPool and network probe");
    }

    // Inject consumers into AccountManager for change routing
    state.account_manager.set_consumers(
        modules::account_change::AccountConsumers::new(
            state.client_manager.clone(),
            state.proxy_allocator.pool().expect("ProxyPool required").clone(),
            state.token_allocator.clone(),
            handle.clone(),
        )
    );

    let account_monitor = Arc::new(AccountMonitor::new(
        state.account_manager.clone(),
        state.token_allocator.clone(),
        state.proxy_allocator.clone(),
        state.boring_client.clone(),
    ));
    account_monitor.clone().start();
    *state.account_monitor.write().await = Some(account_monitor);
    tracing::info!("AccountMonitor started (check interval: 1800s)");

    // ── License refresh task (distribution mode only) ──
    #[cfg(not(feature = "internal"))]
    {
        let server_client = state.server_client.clone();
        let cancel = if let Some(pool) = state.proxy_allocator.pool() {
            pool.cancel_token()
        } else {
            tokio_util::sync::CancellationToken::new()
        };
        tokio::spawn(async move {
            // Always do an immediate server refresh (even if local license loaded above).
            // This ensures plan changes / revocations are detected at startup.
            match refresh_license_once(&server_client).await {
                Ok(_) => tracing::info!("License refreshed from server (startup)"),
                Err(ref e) if is_auth_error(e) => {
                    tracing::error!("License refresh: auth rejected, clearing session");
                    server_client.clear_auth_state();
                    claude_ultra_http::license::clear_license();
                    // Frontend will detect not_logged_in on next status check
                    return;
                }
                Err(e) => tracing::warn!("License refresh failed at startup: {} (using cached license)", e),
            }

            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1800));
            interval.tick().await; // consume immediate tick
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match refresh_license_once(&server_client).await {
                            Ok(_) => tracing::info!("License refreshed"),
                            Err(ref e) if is_auth_error(e) => {
                                tracing::error!("License refresh: auth rejected, clearing session");
                                server_client.clear_auth_state();
                                claude_ultra_http::license::clear_license();
                                break;
                            }
                            Err(e) => tracing::warn!("License refresh failed: {}", e),
                        }
                    }
                    _ = cancel.cancelled() => {
                        tracing::info!("License refresh task stopped");
                        break;
                    }
                }
            }
        });
        tracing::info!("License refresh task started (interval: 1800s)");
    }

    let count = match state.client_manager.load_clients(&accounts_dir) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to load accounts: {}", e);
            return Ok(());
        }
    };

    if count == 0 {
        tracing::info!(
            "No accounts with CLI credentials yet — gateway will start with empty pool"
        );
    }

    // TEMP: allow dev-mode auto-start by commenting out the cfg gates below.
    // Restore the original "dev never auto-starts" behavior by re-enabling the
    // two cfg attributes.
    // #[cfg(debug_assertions)]
    // {
    //     tracing::info!("Dev mode detected, skipping gateway auto-start.");
    // }

    // #[cfg(not(debug_assertions))]
    {
        let config = state.gateway_config.read().await.clone();

        if !config.auto_start {
            tracing::info!("Gateway auto_start disabled, skipping.");
            return Ok(());
        }

        match gateway::server::start_gateway_server_with_proxy(
            &config,
            state.client_manager.clone(),
            Some(state.account_manager.clone()),
            Some(state.proxy_allocator.clone()),
            Some(state.proxy_provider_config.clone()),
            Some(state.proxy_allocator.pool().expect("ProxyPool required").clone()),
            state.security_state.clone(),
            Some(state.token_allocator.clone()),
            Some(state.enable_logging.clone()),
            state.gateway_db.clone(),
            state.proxy_mode,
        )
        .await
        {
            Ok(gw_instance) => {
                tracing::info!(
                    "Gateway started on :{}, {} accounts loaded",
                    gw_instance.port,
                    count
                );
                let mut instance = state.instance.write().await;
                *instance = Some(gw_instance);
                drop(instance);

                // Optionally start the transparent audit server on a separate
                // port (internal builds only, enabled via config). Only runs
                // after the main port came up so the Stopped UI state never
                // diverges from a silently-listening audit port.
                #[cfg(feature = "internal")]
                {
                    if config.transparent_enabled {
                        if config.transparent_port == config.port {
                            tracing::error!(
                                "transparent_port ({}) conflicts with main port ({}), not starting",
                                config.transparent_port,
                                config.port
                            );
                        } else {
                            match gateway::server::start_transparent_server(
                                config.transparent_port,
                                config.request_timeout,
                                state.enable_logging.clone(),
                                state.gateway_db.clone(),
                            )
                            .await
                            {
                                Ok(inst) => {
                                    tracing::info!(
                                        "Transparent audit server started on 127.0.0.1:{}",
                                        inst.port
                                    );
                                    let mut ti = state.transparent_instance.write().await;
                                    *ti = Some(inst);
                                }
                                Err(e) => {
                                    tracing::error!("Failed to start transparent server: {}", e);
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to start gateway: {}", e);
                #[cfg(feature = "internal")]
                if config.transparent_enabled {
                    tracing::warn!(
                        "Main gateway failed to start, skipping transparent audit server"
                    );
                }
            }
        }
    }

    Ok(())
}

/// Shutdown services in reverse order: Gateway → AccountMonitor → ProxyPool.
/// Resets services_initialized so init can be called again after re-login.
pub async fn shutdown_services_inner(handle: &tauri::AppHandle) -> Result<(), String> {
    let state = handle.state::<GatewayServiceState>();

    // Stop Gateway
    {
        let mut instance = state.instance.write().await;
        if let Some(gw) = instance.take() {
            gw.stop().await;
            tracing::info!("Gateway stopped");
        }
    }

    // Stop Transparent audit server (internal builds only)
    #[cfg(feature = "internal")]
    {
        let mut ti = state.transparent_instance.write().await;
        if let Some(gw) = ti.take() {
            gw.stop().await;
            tracing::info!("Transparent audit server stopped");
        }
    }

    // Stop AccountMonitor
    {
        let mut monitor = state.account_monitor.write().await;
        if let Some(m) = monitor.take() {
            m.stop();
            tracing::info!("AccountMonitor stopped");
        }
    }

    // Stop ProxyPool background tasks (cancel token + rebuild channel + reset flag)
    if let Some(pool) = state.proxy_allocator.pool() {
        pool.stop();
        tracing::info!("ProxyPool stopped");
    }

    // Clear license guard
    claude_ultra_http::license::clear_license();
    tracing::info!("License cleared");

    // Reset idempotency guard so services can be re-initialized after re-login
    state.services_initialized.store(false, Ordering::SeqCst);
    tracing::info!("Services shutdown complete");

    Ok(())
}

pub fn run() {
    // Initialize tracing with log bridge layer
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .with(gateway::log_bridge::LogBridgeLayer::new())
        .init();

    // Initialize shared SQLite database (gateway logs + security tables)
    let db_path = modules::gateway_db::get_db_path().unwrap_or_else(|_| {
        let home = dirs::home_dir().expect("Cannot find home directory");
        home.join(".claude-ultra").join("gateway_logs.db")
    });
    let gateway_db = match modules::gateway_db::GatewayDb::new(&db_path) {
        Ok(db) => {
            let db = std::sync::Arc::new(db);
            if let Err(e) = security_db::init_security_tables(&db) {
                tracing::warn!("Failed to initialize security tables: {}", e);
            }
            Some(db)
        }
        Err(e) => {
            tracing::warn!("Failed to initialize gateway DB: {}", e);
            None
        }
    };

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build());

    // Single instance: only in release mode (dev coexists with production app)
    #[cfg(not(debug_assertions))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = app.get_webview_window("main").map(|window| {
                let _ = window.show();
                let _ = window.set_focus();
                #[cfg(target_os = "macos")]
                {
                    app.set_activation_policy(tauri::ActivationPolicy::Regular)
                        .unwrap_or(());
                }
            });
        }));
    }

    // GatewayDb is required — unwrap is safe, app can't function without it
    let gateway_db = gateway_db.expect("GatewayDb initialization failed");

    // Load unified config
    let app_config = models::config::load_app_config().unwrap_or_default();

    builder
        .manage(gateway_db.clone())
        .manage(GatewayServiceState::new(Some(gateway_db.clone()), &app_config))
        .setup(|app| {
            // Dev mode: add [DEV] to window title for visual distinction
            #[cfg(debug_assertions)]
            {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_title("Claude Ultra [DEV]");
                }
            }

            gateway::log_bridge::init_log_bridge(app.handle().clone());
            tray::create_tray(app.handle())?;

            // Internal mode: auto-init services (behavior identical to pre-refactor)
            #[cfg(feature = "internal")]
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = init_services_inner(&handle).await {
                        tracing::error!("Failed to init services (internal mode): {}", e);
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            // Auth
            commands::auth::get_auth_status,
            commands::auth::start_device_flow,
            commands::auth::poll_device_flow,
            commands::auth::logout,
            commands::auth::activate_subscription,
            commands::auth::init_services,
            commands::auth::shutdown_services,
            // Proxy
            commands::proxy::test_proxy_connection,
            commands::static_proxy::test_static_proxy_connection,
            commands::static_proxy::test_static_proxy_dryrun,
            commands::proxy::get_proxy_mode,
            // UI
            commands::ui::show_main_window,
            commands::ui::set_window_theme,
            commands::ui::get_claude_settings,
            commands::ui::sync_claude_settings,
            #[cfg(feature = "internal")]
            commands::ui::sync_claude_settings_transparent,
            commands::ui::restore_claude_settings,
            // Account management
            models::account::list_accounts,
            models::account::delete_account,
            models::account::update_account_label,
            models::account::toggle_user_status,
            models::account::import_accounts,
            models::account::reorder_accounts,
            // App config
            models::config::load_config,
            models::config::save_config,
            // Gateway
            commands::gateway::start_gateway,
            commands::gateway::stop_gateway,
            commands::gateway::get_gateway_status,
            commands::gateway::get_gateway_connection_info,
            commands::gateway::update_gateway_config,
            commands::gateway::regenerate_api_key,
            commands::gateway::test_vercel_connection,
            // Subprocess
            commands::subprocess_cmd::start_oauth,
            commands::subprocess_cmd::start_web_login,
            commands::subprocess_cmd::pause_subprocess,
            commands::subprocess_cmd::resume_subprocess,
            commands::subprocess_cmd::abort_subprocess,
            commands::subprocess_cmd::is_subprocess_running,
            commands::subprocess_cmd::get_subprocess_log,
            commands::subprocess_cmd::clear_subprocess_log,
            commands::subprocess_cmd::get_task,
            commands::subprocess_cmd::list_tasks,
            commands::subprocess_cmd::check_webapp_ready,
            // Accounts
            commands::accounts::update_account_route,
            commands::accounts::add_account_and_login,
            // Gateway DB (request logs + token stats)
            modules::gateway_db::get_request_logs,
            modules::gateway_db::get_token_stats,
            modules::gateway_db::get_cost_summary,
            modules::gateway_db::get_logs_count,
            modules::gateway_db::clear_gateway_logs,
            modules::gateway_db::get_log_detail,
            commands::gateway::set_logging_enabled,
            // Debug console (log bridge)
            gateway::log_bridge::enable_debug_console,
            gateway::log_bridge::disable_debug_console,
            gateway::log_bridge::is_debug_console_enabled,
            gateway::log_bridge::get_debug_console_logs,
            gateway::log_bridge::clear_debug_console_logs,
            // Account quota
            models::account::get_account_quota,
            // Security
            commands::security::get_security_config,
            commands::security::update_security_config,
            commands::security::list_whitelist,
            commands::security::add_whitelist,
            commands::security::remove_whitelist,
            commands::security::list_blacklist,
            commands::security::add_blacklist,
            commands::security::remove_blacklist,
            commands::security::get_access_logs,
            commands::security::get_ip_statistics,
            commands::security::enable_lan_sharing,
            commands::security::disable_lan_sharing,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            match event {
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                        app_handle
                            .set_activation_policy(tauri::ActivationPolicy::Regular)
                            .unwrap_or(());
                    }
                }
                tauri::RunEvent::ExitRequested { .. } => {
                    // Save window state on quit (Dock right-click → Quit)
                    use tauri_plugin_window_state::{AppHandleExt, StateFlags};
                    let _ = app_handle.save_window_state(StateFlags::all());

                    // Kill all running subprocesses (abort → wait → SIGTERM)
                    let state = app_handle.state::<GatewayServiceState>();
                    let mgr = state.subprocess_manager.clone();
                    tauri::async_runtime::block_on(mgr.kill_all());
                }
                _ => {}
            }
        });
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// Test the full service lifecycle state machine without AppHandle.
    /// Simulates: init → (idempotent init) → shutdown → re-init
    #[tokio::test]
    async fn test_services_lifecycle_state_machine() {
        let home = dirs::home_dir().unwrap();
        let _accounts_dir = home.join(".claude-ultra").join("accounts");

        // Build minimal GatewayServiceState
        let gateway_db = {
            let db_path = modules::gateway_db::get_db_path().unwrap_or_else(|_| {
                home.join(".claude-ultra").join("gateway_logs.db")
            });
            modules::gateway_db::GatewayDb::new(&db_path).ok().map(Arc::new)
        };
        let test_config = AppConfig::default();
        let state = GatewayServiceState::new(gateway_db, &test_config);

        // ── Stage 1: Initial state ──
        assert!(!state.services_initialized.load(Ordering::SeqCst));
        assert!(state.account_monitor.read().await.is_none());

        // ── Stage 2: Simulate init (AtomicBool guard) ──
        let was_init = state.services_initialized.swap(true, Ordering::SeqCst);
        assert!(!was_init, "first init should succeed");

        // Start ProxyPool
        if let Some(pool) = state.proxy_allocator.pool() {
            pool.start_background_tasks();
            assert!(pool.is_background_started());
        }

        // Start AccountMonitor
        let monitor = Arc::new(AccountMonitor::new(
            state.account_manager.clone(),
            state.token_allocator.clone(),
            state.proxy_allocator.clone(),
            state.boring_client.clone(),
        ));
        monitor.clone().start();
        *state.account_monitor.write().await = Some(monitor);
        assert!(state.account_monitor.read().await.is_some());

        // ── Stage 3: Idempotent init — second call is no-op ──
        let was_init_again = state.services_initialized.swap(true, Ordering::SeqCst);
        assert!(was_init_again, "second init should be rejected (already initialized)");

        // ── Stage 4: Shutdown ──
        // Stop Gateway (None in test, so nothing to do)
        {
            let mut instance = state.instance.write().await;
            assert!(instance.is_none(), "no gateway in test");
            let _ = instance.take();
        }

        // Stop AccountMonitor
        {
            let mut monitor = state.account_monitor.write().await;
            if let Some(m) = monitor.take() {
                m.stop();
            }
        }
        assert!(state.account_monitor.read().await.is_none());

        // Stop ProxyPool
        if let Some(pool) = state.proxy_allocator.pool() {
            pool.stop();
            assert!(!pool.is_background_started());
        }

        // Reset init flag
        state.services_initialized.store(false, Ordering::SeqCst);
        assert!(!state.services_initialized.load(Ordering::SeqCst));

        // ── Stage 5: Re-init after shutdown ──
        let was_init_third = state.services_initialized.swap(true, Ordering::SeqCst);
        assert!(!was_init_third, "re-init after shutdown should succeed");

        // ProxyPool can restart
        if let Some(pool) = state.proxy_allocator.pool() {
            pool.start_background_tasks();
            assert!(pool.is_background_started());
        }

        // New AccountMonitor can start
        let monitor2 = Arc::new(AccountMonitor::new(
            state.account_manager.clone(),
            state.token_allocator.clone(),
            state.proxy_allocator.clone(),
            state.boring_client.clone(),
        ));
        monitor2.clone().start();
        *state.account_monitor.write().await = Some(monitor2.clone());
        assert!(state.account_monitor.read().await.is_some());

        // Cleanup
        monitor2.stop();
        if let Some(pool) = state.proxy_allocator.pool() {
            pool.stop();
        }
    }

    /// Verify ClientManager load_clients is safe to call repeatedly (clear + reload).
    #[test]
    fn test_client_manager_reload_safe() {
        let home = dirs::home_dir().unwrap();
        let accounts_dir = home.join(".claude-ultra").join("accounts");
        let bc = Arc::new(claude_ultra_http::BoringClient::builder().build().unwrap());
        let cm = ClientManager::new(bc);

        let count1 = cm.load_clients(&accounts_dir).unwrap_or(0);
        let count2 = cm.load_clients(&accounts_dir).unwrap_or(0);
        assert_eq!(count1, count2, "repeated load_clients should produce same count");
    }
}
