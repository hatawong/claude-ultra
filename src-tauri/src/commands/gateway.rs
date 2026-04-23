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

    let config = state.gateway_config.read().await.clone();

    // Pre-flight: reject a conflicting port pair up front so the main server
    // does not spin up only to have the transparent counterpart silently
    // skipped. Only reachable via hand-edited config.json (update IPC already
    // blocks conflicting requests).
    #[cfg(feature = "internal")]
    if config.transparent_enabled {
        validate_port_conflict(config.port, config.transparent_port)?;
    }

    let accounts_dir = state.account_manager.accounts_dir().to_path_buf();
    let count = state.client_manager.load_clients(&accounts_dir)?;

    if count == 0 {
        tracing::info!("No accounts with CLI credentials yet — gateway will start with empty pool");
    }

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
    drop(instance);

    // Start transparent audit server if enabled (internal builds only).
    // Conflict already rejected up front; failures here do not affect main.
    #[cfg(feature = "internal")]
    {
        if config.transparent_enabled {
            match crate::gateway::server::start_transparent_server(
                config.transparent_port,
                config.request_timeout,
                state.enable_logging.clone(),
                state.gateway_db.clone(),
            )
            .await
            {
                Ok(ti) => {
                    let mut slot = state.transparent_instance.write().await;
                    *slot = Some(ti);
                    tracing::info!(
                        "Transparent audit server started on 127.0.0.1:{}",
                        config.transparent_port
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to start transparent server: {}", e);
                }
            }
        }
    }

    let msg = format!("Gateway started on :{}, {} accounts loaded", port, count);
    tracing::info!("{}", msg);
    Ok(msg)
}

#[tauri::command]
pub async fn stop_gateway(
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    let mut instance = state.instance.write().await;
    let main_stopped = if let Some(gw) = instance.take() {
        gw.stop().await;
        tracing::info!("Gateway stopped");
        true
    } else {
        false
    };
    drop(instance);

    #[cfg(feature = "internal")]
    let transparent_stopped = {
        let mut slot = state.transparent_instance.write().await;
        if let Some(gw) = slot.take() {
            gw.stop().await;
            tracing::info!("Transparent audit server stopped");
            true
        } else {
            false
        }
    };
    #[cfg(feature = "internal")]
    {
        let fmt = |stopped: bool| if stopped { "stopped" } else { "not running" };
        Ok(format!(
            "Main: {}, Transparent: {}",
            fmt(main_stopped),
            fmt(transparent_stopped),
        ))
    }
    #[cfg(not(feature = "internal"))]
    {
        Ok(if main_stopped { "Gateway stopped".to_string() } else { "Gateway not running".to_string() })
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

    #[cfg(feature = "internal")]
    transparent_enabled: bool,
    #[cfg(feature = "internal")]
    transparent_port: u16,
    #[cfg(feature = "internal")]
    transparent_running: bool,
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
    #[cfg(feature = "internal")]
    let transparent_running = state.transparent_instance.read().await.is_some();

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

        #[cfg(feature = "internal")]
        transparent_enabled: config.transparent_enabled,
        #[cfg(feature = "internal")]
        transparent_port: config.transparent_port,
        #[cfg(feature = "internal")]
        transparent_running,
    })
}

// ── Port validation helpers ──────────────────────────────────

/// Check a user-supplied port is in the allowed range (1024..=65535).
/// Below 1024 requires privileges that the gateway is not designed for,
/// and is almost always a config mistake.
fn validate_port_range(field: &str, port: u16) -> Result<(), String> {
    if !(1024..=65535).contains(&port) {
        return Err(format!("{} ({}) must be in 1024..=65535", field, port));
    }
    Ok(())
}

/// Reject the two gateway ports being equal — they must bind independently.
#[cfg(feature = "internal")]
fn validate_port_conflict(main: u16, transparent: u16) -> Result<(), String> {
    if main == transparent {
        return Err(format!(
            "port ({}) conflicts with transparent_port ({})",
            main, transparent
        ));
    }
    Ok(())
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
    vercel_api_key: Option<String>,

    #[cfg(feature = "internal")]
    transparent_enabled: Option<bool>,
    #[cfg(feature = "internal")]
    transparent_port: Option<u16>,
}

#[tauri::command]
pub async fn update_gateway_config(
    request: UpdateGatewayConfigRequest,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    let mut config = state.gateway_config.write().await;

    // 1. Derive candidate values (do not mutate config yet).
    let new_port = request.port.unwrap_or(config.port);
    #[cfg(feature = "internal")]
    let new_transparent_port = request.transparent_port.unwrap_or(config.transparent_port);

    // 2. Validate both ports' ranges, regardless of which one changed.
    if request.port.is_some() {
        validate_port_range("port", new_port)?;
    }
    #[cfg(feature = "internal")]
    if request.transparent_port.is_some() {
        validate_port_range("transparent_port", new_transparent_port)?;
    }

    // 3. Validate conflict on the candidate pair, catches either direction.
    #[cfg(feature = "internal")]
    validate_port_conflict(new_port, new_transparent_port)?;

    // 4. All checks passed — apply atomically.
    if let Some(ba) = request.bind_address {
        config.bind_address = ba;
    }
    config.port = new_port;
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
    if let Some(vk) = request.vercel_api_key {
        config.vercel_api_key = vk;
    }
    #[cfg(feature = "internal")]
    {
        if let Some(v) = request.transparent_enabled {
            config.transparent_enabled = v;
        }
        config.transparent_port = new_transparent_port;
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

#[tauri::command]
pub async fn test_vercel_connection(
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    let config = state.gateway_config.read().await;
    let key = &config.vercel_api_key;
    if key.is_empty() {
        return Err("Vercel API key not configured".into());
    }
    // Verify key validity via cheapest model (mistral-nemo, ~$0.00000014 per call).
    // Uses OpenAI chat/completions format + Authorization: Bearer (Mode 1 API auth).
    // Limitation: uses reqwest (not BoringClient), no vercel_proxy_url, no BYOK.
    // Full path validated by E2E Gate 5/6.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("client build: {}", e))?;
    let resp = client
        .post("https://ai-gateway.vercel.sh/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", key))
        .header("content-type", "application/json")
        .body(r#"{"model":"mistral/mistral-nemo","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .map_err(|e| format!("request: {}", e))?;
    let status = resp.status().as_u16();
    if status == 200 {
        Ok("connected".into())
    } else {
        let body = resp.text().await.unwrap_or_default();
        let msg = body.get(..200).unwrap_or(&body);
        Err(format!("HTTP {}: {}", status, msg))
    }
}

#[cfg(test)]
mod port_validation_tests {
    use super::*;

    #[test]
    fn test_validate_port_range_accepts_1024_to_65535() {
        assert!(validate_port_range("port", 1024).is_ok());
        assert!(validate_port_range("port", 9000).is_ok());
        assert!(validate_port_range("port", 65535).is_ok());
    }

    #[test]
    fn test_validate_port_range_rejects_below_1024() {
        assert!(validate_port_range("port", 80).is_err());
        assert!(validate_port_range("port", 1023).is_err());
        assert!(validate_port_range("port", 0).is_err());
    }

    #[test]
    fn test_validate_port_range_error_includes_field_name() {
        let err = validate_port_range("transparent_port", 22).unwrap_err();
        assert!(err.contains("transparent_port"));
        assert!(err.contains("22"));
    }

    #[cfg(feature = "internal")]
    #[test]
    fn test_validate_port_conflict_rejects_equal_ports() {
        assert!(validate_port_conflict(9000, 9000).is_err());
        assert!(validate_port_conflict(1024, 1024).is_err());
    }

    #[cfg(feature = "internal")]
    #[test]
    fn test_validate_port_conflict_accepts_distinct_ports() {
        assert!(validate_port_conflict(9000, 9001).is_ok());
        assert!(validate_port_conflict(8080, 3000).is_ok());
    }

    #[cfg(feature = "internal")]
    #[test]
    fn test_validate_port_conflict_error_mentions_both_ports() {
        let err = validate_port_conflict(9000, 9000).unwrap_err();
        assert!(err.contains("9000"));
        assert!(err.contains("port"));
        assert!(err.contains("transparent_port"));
    }
}
