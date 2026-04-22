//! Gateway IPC commands — start/stop/status, connection info, config, logging.

use crate::GatewayServiceState;
use crate::gateway::server::GatewayStatus;

#[tauri::command]
pub async fn start_gateway(
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    let mut instance = state.instance.write().await;
    if instance.is_some() {
        return Ok("Gateway already running".to_string());
    }

    let accounts_dir = state.account_manager.accounts_dir().to_path_buf();
    let count = state.client_manager.load_clients(&accounts_dir)?;

    if count == 0 {
        tracing::info!("No accounts with CLI credentials yet — gateway will start with empty pool");
    }

    let config = state.gateway_config.read().await.clone();
    let proxy_instance = crate::gateway::server::start_gateway_server_with_proxy(
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
    .await?;

    let port = proxy_instance.port;
    *instance = Some(proxy_instance);

    let msg = format!("Gateway started on :{}, {} accounts loaded", port, count);
    tracing::info!("{}", msg);
    Ok(msg)
}

#[tauri::command]
pub async fn stop_gateway(
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    let mut instance = state.instance.write().await;
    if let Some(gw) = instance.take() {
        gw.stop().await;
        tracing::info!("Gateway stopped");
        Ok("Gateway stopped".to_string())
    } else {
        Ok("Gateway not running".to_string())
    }
}

#[tauri::command]
pub async fn get_gateway_status(
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<GatewayStatus, String> {
    let instance = state.instance.read().await;
    Ok(GatewayStatus {
        running: instance.is_some(),
        port: instance.as_ref().map(|i| i.port).unwrap_or(9000),
        active_accounts: state.client_manager.available_count(),
        total_accounts: state.client_manager.client_count(),
    })
}

// ── Gateway Connection Info ──────────────────────────────────

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayConnectionInfo {
    running: bool,
    port: u16,
    bind_address: String,
    api_key: String,
    request_timeout: u64,
    auto_start: bool,
    admin_password: String,
    enable_logging: bool,
    lan_ip: Option<String>,
    active_accounts: usize,
}

pub(super) fn get_lan_ip() -> Option<String> {
    use std::net::UdpSocket;
    let targets = ["192.168.1.1:80", "10.0.0.1:80", "172.16.0.1:80", "8.8.8.8:80"];
    for target in &targets {
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            if socket.connect(target).is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    let ip = addr.ip().to_string();
                    if !ip.starts_with("198.18.") {
                        return Some(ip);
                    }
                }
            }
        }
    }
    None
}

#[tauri::command]
pub async fn get_gateway_connection_info(
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<GatewayConnectionInfo, String> {
    let instance = state.instance.read().await;
    let config = state.gateway_config.read().await;
    Ok(GatewayConnectionInfo {
        running: instance.is_some(),
        port: instance.as_ref().map(|i| i.port).unwrap_or(config.port),
        bind_address: config.bind_address.clone(),
        api_key: config.api_key.clone(),
        request_timeout: config.request_timeout,
        auto_start: config.auto_start,
        admin_password: config.admin_password.clone(),
        enable_logging: config.enable_logging,
        lan_ip: get_lan_ip(),
        active_accounts: state.client_manager.available_count(),
    })
}

// ── Gateway Config ───────────────────────────────────────────

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGatewayConfigRequest {
    bind_address: Option<String>,
    port: Option<u16>,
    api_key: Option<String>,
    request_timeout: Option<u64>,
    auto_start: Option<bool>,
    admin_password: Option<String>,
    enable_logging: Option<bool>,
}

#[tauri::command]
pub async fn update_gateway_config(
    request: UpdateGatewayConfigRequest,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    let mut config = state.gateway_config.write().await;
    if let Some(ba) = request.bind_address {
        config.bind_address = ba;
    }
    if let Some(p) = request.port {
        config.port = p;
    }
    if let Some(k) = request.api_key {
        config.api_key = k;
    }
    if let Some(t) = request.request_timeout {
        config.request_timeout = t.max(30).min(7200);
    }
    if let Some(a) = request.auto_start {
        config.auto_start = a;
    }
    if let Some(e) = request.enable_logging {
        config.enable_logging = e;
    }
    if let Some(p) = request.admin_password {
        config.admin_password = p;
    }
    config.save();
    Ok("Config updated. Restart gateway to apply.".to_string())
}

#[tauri::command]
pub async fn regenerate_api_key(
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    let new_key = format!("sk-ultra-{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
    let mut config = state.gateway_config.write().await;
    config.api_key = new_key.clone();
    config.save();
    Ok(new_key)
}

#[tauri::command]
pub async fn set_logging_enabled(
    enabled: bool,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    let mut config = state.gateway_config.write().await;
    config.enable_logging = enabled;
    config.save();
    state.enable_logging.store(enabled, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}
