//! AccountChange — describes a change to an Account that needs routing
//! to runtime consumers (ClientManager, ProxyPool, frontend).

use std::sync::Arc;
use crate::modules::client_manager::ClientManager;
use crate::modules::token_allocator::TokenAllocator;
use crate::proxy::pool::ProxyPool;

/// A change to an Account that needs to be routed to consumers.
pub enum AccountChange {
    /// Account created and ready (web login + profile written, possibly cli too).
    /// If cli exists, triggers token validation → CliTokenUpdated on success.
    Created,
    /// CLI token validated and ready for gateway use (add or update in ClientManager).
    CliTokenUpdated {
        access_token: String,
        refresh_token: String,
        expires_at: i64,
    },
    /// System-disabled (401/403/banned/token invalid).
    Disabled { reason: String },
    /// User manually toggled enable/disable.
    UserDisabledChanged { disabled: bool },
    /// Account deleted.
    Deleted,
    /// Utilization updated from get_usage (full) or gateway response headers (incremental).
    /// QuotaSnapshot is used for ClientManager.update_quota.
    UtilizationUpdated {
        snapshot: crate::models::quota::QuotaSnapshot,
    },
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
