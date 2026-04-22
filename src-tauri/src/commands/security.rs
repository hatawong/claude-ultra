//! Security IPC commands — whitelist/blacklist, access logs, LAN sharing.

use crate::GatewayServiceState;
use crate::modules::{gateway_db::GatewayDb, security_db};
use std::sync::Arc;

#[tauri::command]
pub async fn get_security_config(
    db: tauri::State<'_, Arc<GatewayDb>>,
) -> Result<security_db::SecurityConfig, String> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || security_db::get_security_config(&db))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn update_security_config(
    config: security_db::SecurityConfig,
    db: tauri::State<'_, Arc<GatewayDb>>,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<(), String> {
    let mode = config.mode;
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || security_db::update_security_config(&db, &config))
        .await
        .map_err(|e| e.to_string())??;
    if let Some(ref sec) = state.security_state {
        *sec.mode.write() = mode;
    }
    Ok(())
}

#[tauri::command]
pub async fn list_whitelist(
    db: tauri::State<'_, Arc<GatewayDb>>,
) -> Result<Vec<security_db::WhitelistEntry>, String> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || security_db::list_whitelist(&db))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn add_whitelist(
    ip: String,
    description: Option<String>,
    db: tauri::State<'_, Arc<GatewayDb>>,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<i64, String> {
    if let Some(ref sec) = state.security_state {
        sec.add_whitelist(&ip, description.as_deref())
    } else {
        security_db::add_whitelist(&db, &ip, description.as_deref())
    }
}

#[tauri::command]
pub async fn remove_whitelist(
    id: i64,
    ip: String,
    db: tauri::State<'_, Arc<GatewayDb>>,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<bool, String> {
    if let Some(ref sec) = state.security_state {
        sec.remove_whitelist(id, &ip)
    } else {
        security_db::remove_whitelist(&db, id)
    }
}

#[tauri::command]
pub async fn list_blacklist(
    db: tauri::State<'_, Arc<GatewayDb>>,
) -> Result<Vec<security_db::BlacklistEntry>, String> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || security_db::list_blacklist(&db))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn add_blacklist(
    ip: String,
    reason: Option<String>,
    expires_at: Option<i64>,
    db: tauri::State<'_, Arc<GatewayDb>>,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<i64, String> {
    if let Some(ref sec) = state.security_state {
        sec.add_blacklist(&ip, reason.as_deref(), expires_at)
    } else {
        security_db::add_blacklist(&db, &ip, reason.as_deref(), expires_at)
    }
}

#[tauri::command]
pub async fn remove_blacklist(
    id: i64,
    ip: String,
    db: tauri::State<'_, Arc<GatewayDb>>,
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<bool, String> {
    if let Some(ref sec) = state.security_state {
        sec.remove_blacklist(id, &ip)
    } else {
        security_db::remove_blacklist(&db, id)
    }
}

#[tauri::command]
pub async fn get_access_logs(
    limit: usize,
    offset: usize,
    client_ip: Option<String>,
    search: Option<String>,
    db: tauri::State<'_, Arc<GatewayDb>>,
) -> Result<security_db::AccessLogResponse, String> {
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || {
        security_db::get_access_logs(&db, limit, offset, client_ip.as_deref(), search.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_ip_statistics(
    hours: Option<i64>,
    db: tauri::State<'_, Arc<GatewayDb>>,
) -> Result<security_db::IpStatsResponse, String> {
    let h = hours.unwrap_or(24);
    let db = db.inner().clone();
    tokio::task::spawn_blocking(move || security_db::get_ip_statistics(&db, h))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn enable_lan_sharing(
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    {
        let mut config = state.gateway_config.write().await;
        config.bind_address = "0.0.0.0".to_string();
        config.save();
    }
    if let Some(ref sec) = state.security_state {
        // Always allow localhost
        let _ = sec.add_whitelist("127.0.0.1", Some("localhost"));
        // Whitelist strategy:
        //   - If the local LAN IP is in an RFC1918 private range → expand to /24
        //     so other devices on the same subnet can connect (matches UI promise).
        //   - If not RFC1918 (CGNAT/Tailscale/public IP/link-local) → single-IP
        //     only, avoid over-whitelisting neighbors that aren't ours.
        //     Users on overlay/VPN networks can manually add CIDR ranges they trust.
        if let Some(lan_ip) = super::gateway::get_lan_ip() {
            if let Some(cidr) = crate::gateway::security::lan_ip_to_slash_24(&lan_ip) {
                let _ = sec.add_whitelist(&cidr, Some("LAN subnet (auto)"));
            } else {
                let _ = sec.add_whitelist(&lan_ip, Some("local machine (non-RFC1918)"));
            }
        }
        sec.set_mode(security_db::SecurityMode::Whitelist)?;
    }
    Ok("LAN sharing enabled. Restart gateway to apply.".to_string())
}

#[tauri::command]
pub async fn disable_lan_sharing(
    state: tauri::State<'_, GatewayServiceState>,
) -> Result<String, String> {
    {
        let mut config = state.gateway_config.write().await;
        config.bind_address = "127.0.0.1".to_string();
        config.save();
    }
    if let Some(ref sec) = state.security_state {
        sec.set_mode(security_db::SecurityMode::Off)?;
    }
    Ok("LAN sharing disabled. Restart gateway to apply.".to_string())
}
