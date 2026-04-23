//! AccountChange — describes a change to an Account that needs routing
//! to runtime consumers (ClientManager, ProxyPool, frontend).

use std::sync::Arc;
use crate::modules::client_manager::ClientManager;
use crate::modules::token_allocator::TokenAllocator;
use crate::proxy::pool::ProxyPool;

/// A change to an Account that needs to be routed to consumers.
pub enum AccountChange {
    /// Account created and ready (web login + profile written, possibly cli too).
    /// If cli exists, triggers token validation → CliUpdated on success.
    Created,
    /// Account deleted.
    Deleted,
    /// System-disabled (401/403/banned/token invalid).
    Disabled { reason: String },
    /// User manually toggled enable/disable.
    UserDisabledChanged { disabled: bool },
    /// Profile fields updated (email/fullName/subscription/etc.) — notify frontend only.
    ProfileUpdated,
    /// Route fields updated (route_mode / route_country) — update ClientManager cache + notify frontend.
    RouteUpdated,
    /// Proxy section updated (from allocation/commit/webapp result) — notify frontend only.
    ProxyUpdated,
    /// Web credentials updated (cookies + lastActivity) — notify frontend only.
    /// Utilization updated from get_usage (full) or gateway response headers (incremental).
    /// QuotaSnapshot is used for ClientManager.update_quota.
    UtilizationUpdated {
        snapshot: crate::models::quota::QuotaSnapshot,
    },
    WebUpdated,
    /// CLI section updated (new OAuth or token refresh) — sync ClientManager + notify.
    /// Handler reads fresh cli from disk (no payload).
    CliUpdated,
}

/// Runtime consumers that need to be notified of account changes.
/// Injected into AccountManager after all components are initialized.
pub struct AccountConsumers {
    pub client_manager: Arc<ClientManager>,
    pub proxy_pool: Arc<ProxyPool>,
    pub token_allocator: Arc<TokenAllocator>,
    app_handle: tauri::AppHandle,
}

impl AccountConsumers {
    pub fn new(
        client_manager: Arc<ClientManager>,
        proxy_pool: Arc<ProxyPool>,
        token_allocator: Arc<TokenAllocator>,
        app_handle: tauri::AppHandle,
    ) -> Self {
        Self { client_manager, proxy_pool, token_allocator, app_handle }
    }

    pub fn emit_frontend(&self, event: &str, account_id: &str) {
        use tauri::Emitter;
        let _ = self.app_handle.emit(event, account_id);
    }
}
