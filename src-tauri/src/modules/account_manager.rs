//! AccountManager — unified concurrent access to Account JSON files.
//! All Account JSON writes go through this manager (per-account RwLock).

use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::models::account::Account;
use crate::modules::account_change::{AccountChange, AccountConsumers};

/// Partial profile update for `AccountManager::set_profile`.
/// Each field with Some gets written; None means leave unchanged.
/// Note: `country` is immutable (set at account creation); omit it from updates.
/// Route (route_mode / route_country) is managed separately via `set_route`.
#[derive(Debug, Default, Clone)]
pub struct ProfileUpdate {
    pub email: Option<String>,
    pub account_uuid: Option<String>,
    pub org_id: Option<String>,
    pub full_name: Option<String>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
    pub billing_type: Option<String>,
}

/// Manages concurrent read/write access to Account JSON files.
/// Uses per-account RwLock to prevent concurrent write corruption.
pub struct AccountManager {
    accounts_dir: PathBuf,
    locks: DashMap<String, Arc<tokio::sync::RwLock<()>>>,
    consumers: std::sync::OnceLock<AccountConsumers>,
}

impl AccountManager {
    pub fn new(accounts_dir: PathBuf) -> Self {
        Self {
            accounts_dir,
            locks: DashMap::new(),
            consumers: std::sync::OnceLock::new(),
        }
    }

    /// Inject consumers after all components are initialized.
    pub fn set_consumers(&self, consumers: AccountConsumers) {
        let _ = self.consumers.set(consumers);
    }

    /// Route a change to consumers. Called after persistence is done.
    /// Uses Box::pin for recursion: Created may chain into CliUpdated.
    pub(crate) fn route_change<'a>(
        &'a self,
        account_id: &'a str,
        change: AccountChange,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let consumers = match self.consumers.get() {
                Some(c) => c,
                None => return, // not yet initialized (startup phase)
            };

            match change {
                AccountChange::Created => {
                    // Notify frontend immediately — account appears in list right away
                    consumers.emit_frontend("account://changed", account_id);

                    // If account has cli with non-expired token, force-refresh to verify it's usable.
                    // Note: force_refresh_token internally calls set_cli which emits CliUpdated
                    // (handles ClientManager sync + frontend refresh); no outer route_change needed.
                    let cli_expires_at = self.read(account_id).await
                        .ok()
                        .and_then(|a| a.cli.as_ref().map(|c| c.expires_at));
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let has_valid_cli = cli_expires_at.map_or(false, |ea| ea > now_ms);
                    if has_valid_cli {
                        if let Err(e) = consumers.token_allocator.force_refresh_token(account_id).await {
                            tracing::warn!("Created: token refresh failed for {}: {}", account_id, e);
                        }
                    }
                }
                AccountChange::CliUpdated => {
                    // Read fresh cli from disk to sync ClientManager
                    if let Ok(account) = self.read(account_id).await {
                        if let Some(cli) = account.cli.as_ref() {
                            if consumers.client_manager.has_client(account_id) {
                                consumers.client_manager.update_client_token(
                                    account_id,
                                    cli.access_token.clone(),
                                    cli.refresh_token.clone(),
                                    cli.expires_at,
                                );
                            } else if !account.disabled && !account.user_disabled {
                                consumers.client_manager.add_client(&account);
                            }
                        }
                    }
                    consumers.emit_frontend("account://changed", account_id);
                }
                AccountChange::Disabled { reason } => {
                    consumers.client_manager.disable_client(account_id, &reason);
                    consumers.proxy_pool.disable_in_pool(account_id).await;
                    consumers.emit_frontend("account://changed", account_id);
                }
                AccountChange::UserDisabledChanged { disabled } => {
                    if disabled {
                        consumers.client_manager.disable_client(account_id, "user disabled");
                        consumers.proxy_pool.disable_in_pool(account_id).await;
                    } else {
                        // Re-enable: add back to pool if account has valid cli
                        if let Ok(account) = self.read(account_id).await {
                            if !account.disabled && account.cli.is_some() {
                                consumers.client_manager.add_client(&account);
                                consumers.proxy_pool.enable_in_pool(account_id).await;
                            }
                        }
                    }
                    consumers.emit_frontend("account://changed", account_id);
                }
                AccountChange::Deleted => {
                    // Note: JSON file is already deleted before route_change is called.
                    // Do NOT read account in this branch — the file no longer exists.
                    consumers.client_manager.disable_client(account_id, "deleted");
                    consumers.proxy_pool.disable_in_pool(account_id).await;
                    consumers.emit_frontend("account://changed", account_id);
                }
                AccountChange::UtilizationUpdated { snapshot } => {
                    consumers.client_manager.update_quota(account_id, snapshot);
                    consumers.emit_frontend("account://changed", account_id);
                }
                AccountChange::ProfileUpdated => {
                    consumers.emit_frontend("account://changed", account_id);
                }
                AccountChange::RouteUpdated => {
                    // Sync ClientManager cache — read back from disk to match persisted state
                    if let Ok(account) = self.read(account_id).await {
                        consumers.client_manager.set_route_mode(account_id, &account.route_mode);
                        consumers.client_manager.set_route_country(
                            account_id,
                            account.route_country.as_deref(),
                        );
                    }
                    consumers.emit_frontend("account://changed", account_id);
                }
                AccountChange::ProxyUpdated => {
                    consumers.emit_frontend("account://changed", account_id);
                }
                AccountChange::WebUpdated => {
                    consumers.emit_frontend("account://changed", account_id);
                }
            }
        })
    }

    fn get_lock(&self, account_id: &str) -> Arc<tokio::sync::RwLock<()>> {
        self.locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::RwLock::new(())))
            .clone()
    }

    fn account_path(&self, account_id: &str) -> Result<PathBuf, String> {
        Self::validate_account_id(account_id)?;
        Ok(self.accounts_dir.join(format!("{}.json", account_id)))
    }

    /// Validate account_id: reject path traversal and invalid characters.
    fn validate_account_id(id: &str) -> Result<(), String> {
        if id.is_empty() {
            return Err("account_id is empty".to_string());
        }
        if id.contains('/') || id.contains('\\') || id.contains("..") {
            return Err(format!("Invalid account_id: {}", id));
        }
        if !id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("Invalid account_id characters: {}", id));
        }
        Ok(())
    }

    /// Read an account by ID (with read lock).
    pub async fn read(&self, account_id: &str) -> Result<Account, String> {
        let lock = self.get_lock(account_id);
        let _guard = lock.read().await;
        self.read_from_file(account_id)
    }

    /// Update an account atomically: read → apply closure → write back.
    /// The closure is synchronous — all async work must be done before calling update.
    ///
    /// Internal only: external modules must use named field methods (set_profile, set_web, etc.)
    /// to preserve the single-writer invariant + route_change emission.
    pub(crate) async fn update<F>(&self, account_id: &str, f: F) -> Result<(), String>
    where
        F: FnOnce(&mut Account),
    {
        let lock = self.get_lock(account_id);
        let _guard = lock.write().await;
        let mut account = self.read_from_file(account_id)?;
        f(&mut account);
        self.write_to_file(account_id, &account)
    }

    /// Update profile fields (partial — only fields with Some get written).
    /// Emits AccountChange::ProfileUpdated (frontend list refresh).
    pub async fn set_profile(
        &self,
        account_id: &str,
        update: ProfileUpdate,
    ) -> Result<(), String> {
        self.update(account_id, |a| {
            if let Some(ref e) = update.email { if !e.is_empty() { a.email = e.to_lowercase(); } }
            if let Some(ref u) = update.account_uuid { if !u.is_empty() { a.account_uuid = u.clone(); } }
            if let Some(ref o) = update.org_id { if !o.is_empty() { a.org_id = o.clone(); } }
            if let Some(ref n) = update.full_name { if !n.is_empty() { a.full_name = n.clone(); } }
            if let Some(ref s) = update.subscription_type {
                if !s.is_empty() { a.subscription_type = s.clone(); }
            } else if a.subscription_type == "unknown" {
                a.subscription_type = "free".to_string();
            }
            if let Some(ref r) = update.rate_limit_tier { a.rate_limit_tier = Some(r.clone()); }
            if let Some(ref b) = update.billing_type { a.billing_type = Some(b.clone()); }
        }).await?;
        self.route_change(account_id, AccountChange::ProfileUpdated).await;
        Ok(())
    }

    /// Set custom label (user-provided nickname).
    /// - `Some(label)` → write label
    /// - `None` → clear
    /// Emits AccountChange::ProfileUpdated (frontend refresh).
    pub async fn set_label(
        &self,
        account_id: &str,
        label: Option<String>,
    ) -> Result<(), String> {
        self.update(account_id, move |a| {
            a.custom_label = label;
        }).await?;
        self.route_change(account_id, AccountChange::ProfileUpdated).await;
        Ok(())
    }

    /// Update route_mode and/or route_country.
    /// route_mode: only "proxy" / "vercel" / "direct" accepted; invalid values ignored (warn).
    /// route_country: empty string clears (None); only known PROXY_COUNTRIES accepted.
    /// Emits AccountChange::RouteUpdated (ClientManager cache sync + frontend refresh).
    pub async fn set_route(
        &self,
        account_id: &str,
        route_mode: Option<String>,
        route_country: Option<String>,
    ) -> Result<(), String> {
        self.update(account_id, |a| {
            if let Some(ref rm) = route_mode {
                let lc = rm.to_lowercase();
                if ["proxy", "vercel", "direct"].contains(&lc.as_str()) {
                    a.route_mode = lc;
                } else {
                    tracing::warn!("invalid route_mode '{}', ignoring", rm);
                }
            }
            if let Some(ref rc) = route_country {
                if rc.is_empty() {
                    a.route_country = None;
                } else {
                    let lower = rc.to_lowercase();
                    if crate::gateway::route::PROXY_COUNTRIES.contains(&lower.as_str()) {
                        a.route_country = Some(lower);
                    } else {
                        tracing::warn!("invalid route_country '{}', ignoring", rc);
                    }
                }
            }
        }).await?;
        self.route_change(account_id, AccountChange::RouteUpdated).await;
        Ok(())
    }

    /// Set Account.proxy.
    /// - `Some(section)` → write proxy section
    /// - `None` → clear (a.proxy = None)
    /// Emits AccountChange::ProxyUpdated (frontend refresh).
    pub async fn set_proxy(
        &self,
        account_id: &str,
        proxy: Option<crate::models::account::ProxySection>,
    ) -> Result<(), String> {
        self.update(account_id, move |a| { a.proxy = proxy; }).await?;
        self.route_change(account_id, AccountChange::ProxyUpdated).await;
        Ok(())
    }

    /// Mark account as (system-)disabled.
    /// - `Some(reason)` → set a.disabled=true + reason + disabled_at → emits Disabled
    /// - `None` → clear disabled state (re-enable) — no event
    pub async fn set_disabled(
        &self,
        account_id: &str,
        reason: Option<String>,
    ) -> Result<(), String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        match reason {
            Some(r) => {
                let r_clone = r.clone();
                self.update(account_id, move |a| {
                    a.disabled = true;
                    a.disabled_reason = Some(r);
                    a.disabled_at = Some(now_ms);
                    // CLI just triggered 401/403/banned — mark activity
                    if let Some(ref mut cli) = a.cli {
                        cli.last_activity = Some(now_ms);
                    }
                }).await?;
                self.route_change(account_id, AccountChange::Disabled { reason: r_clone }).await;
                Ok(())
            }
            None => {
                self.update(account_id, |a| {
                    a.disabled = false;
                    a.disabled_reason = None;
                    a.disabled_at = None;
                }).await
            }
        }
    }

    /// Set user-disable state.
    /// - `Some(reason)` → user disables; sets a.user_disabled=true + reason + timestamp
    /// - `None` → user re-enables; clears reason + timestamp
    /// Emits UserDisabledChanged.
    pub async fn set_user_disabled(
        &self,
        account_id: &str,
        reason: Option<String>,
    ) -> Result<(), String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let disabled = reason.is_some();
        self.update(account_id, move |a| {
            a.user_disabled = disabled;
            if let Some(r) = reason {
                a.user_disabled_reason = Some(r);
                a.user_disabled_at = Some(now_ms);
            } else {
                a.user_disabled_reason = None;
                a.user_disabled_at = None;
            }
        }).await?;
        self.route_change(account_id, AccountChange::UserDisabledChanged { disabled }).await;
        Ok(())
    }

    /// Set Account.utilization (full replace) and emit UtilizationUpdated.
    /// Also bumps cli.last_activity (get_usage call = CLI activity).
    pub async fn set_utilization(
        &self,
        account_id: &str,
        usage: serde_json::Value,
        snapshot: crate::models::quota::QuotaSnapshot,
    ) -> Result<(), String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        self.update(account_id, move |a| {
            a.utilization = Some(usage);
            if let Some(ref mut cli) = a.cli {
                cli.last_activity = Some(now_ms);
            }
        }).await?;
        self.route_change(account_id, AccountChange::UtilizationUpdated { snapshot }).await;
        Ok(())
    }

    /// Merge incoming utilization with existing + bump cli.last_activity.
    /// Used by gateway hot path (incremental quota updates from response headers).
    /// Emits UtilizationUpdated.
    pub async fn merge_utilization(
        &self,
        account_id: &str,
        incoming: crate::modules::cli_client::Utilization,
        snapshot: crate::models::quota::QuotaSnapshot,
    ) -> Result<(), String> {
        self.update(account_id, move |a| {
            let existing = a.utilization.as_ref()
                .and_then(|v| serde_json::from_value::<crate::modules::cli_client::Utilization>(v.clone()).ok());
            let merged = crate::models::quota::merge_utilization(existing, incoming);
            if let Ok(val) = serde_json::to_value(&merged) {
                a.utilization = Some(val);
            }
            if let Some(ref mut cli) = a.cli {
                cli.last_activity = Some(chrono::Utc::now().timestamp_millis());
            }
        }).await?;
        self.route_change(account_id, AccountChange::UtilizationUpdated { snapshot }).await;
        Ok(())
    }

    /// Set Account.web.
    /// - `Some(cookies)` → write WebClient with given cookies + current lastActivity
    /// - `None` → clear (a.web = None)
    /// Emits AccountChange::WebUpdated (frontend refresh).
    pub async fn set_web(
        &self,
        account_id: &str,
        cookies: Option<Vec<crate::models::account::CookieData>>,
    ) -> Result<(), String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        self.update(account_id, move |a| {
            a.web = cookies.map(|c| crate::models::account::WebClient {
                cookies: c,
                local_storage: std::collections::HashMap::new(),
                last_activity: Some(now_ms),
            });
        }).await?;
        self.route_change(account_id, AccountChange::WebUpdated).await;
        Ok(())
    }

    /// Set Account.cli.
    /// - `scopes = Some(..)`: full replace (preserves device_id via ensure_device_id)
    /// - `scopes = None`: partial refresh (only tokens + expires_at; preserves scopes/device_id/lastActivity). No-op if cli is None.
    /// Emits AccountChange::CliUpdated (ClientManager sync + frontend refresh).
    pub async fn set_cli(
        &self,
        account_id: &str,
        access_token: String,
        refresh_token: String,
        expires_at: i64,
        scopes: Option<Vec<String>>,
    ) -> Result<(), String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        self.update(account_id, move |a| {
            match scopes {
                Some(s) => {
                    let existing_device_id = a.cli.as_ref().map(|c| c.device_id.as_str()).unwrap_or("");
                    let device_id = crate::models::account::ensure_device_id(existing_device_id);
                    a.cli = Some(crate::models::account::CliClient {
                        access_token,
                        refresh_token,
                        expires_at,
                        scopes: s,
                        last_activity: Some(now_ms),
                        device_id,
                    });
                }
                None => {
                    if let Some(ref mut cli) = a.cli {
                        cli.access_token = access_token;
                        cli.refresh_token = refresh_token;
                        cli.expires_at = expires_at;
                        cli.last_activity = Some(now_ms);
                    }
                }
            }
        }).await?;
        self.route_change(account_id, AccountChange::CliUpdated).await;
        Ok(())
    }

    /// List all accounts in the directory.
    pub async fn list(&self) -> Result<Vec<Account>, String> {
        crate::models::account::list_accounts_in_dir(&self.accounts_dir)
    }

    /// Delete an account by removing its JSON file.
    pub async fn delete(&self, account_id: &str) -> Result<(), String> {
        let lock = self.get_lock(account_id);
        let _guard = lock.write().await;
        let path = self.account_path(account_id)?;
        if !path.exists() {
            return Err(format!("Account {} not found", account_id));
        }
        std::fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete {}: {}", path.display(), e))?;
        drop(_guard);
        self.locks.remove(account_id);
        Ok(())
    }

    /// Write a new account to the directory.
    pub async fn write_new(&self, account: &Account) -> Result<(), String> {
        let lock = self.get_lock(&account.account_id);
        let _guard = lock.write().await;
        let path = self.account_path(&account.account_id)?;
        if path.exists() {
            return Err(format!("Account {} already exists", account.account_id));
        }
        self.write_to_file(&account.account_id, account)
    }

    /// Get the accounts directory path.
    pub fn accounts_dir(&self) -> &Path {
        &self.accounts_dir
    }

    fn read_from_file(&self, account_id: &str) -> Result<Account, String> {
        let path = self.account_path(account_id)?;
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
    }

    fn write_to_file(&self, account_id: &str, account: &Account) -> Result<(), String> {
        let path = self.account_path(account_id)?;
        // Atomic write + 0600 permissions: account JSON contains session_key / cookies /
        // access_token / refresh_token — must never be world-readable or half-written.
        crate::modules::secure_fs::secure_write_json(&path, account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "claude_ultra_acct_mgr_test_{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn write_v3_account(dir: &Path, account_id: &str, email: &str) {
        let json = format!(
            r#"{{
                "accountId": "{}",
                "email": "{}",
                "cli": {{
                    "accessToken": "sk-ant-oat01-{}",
                    "refreshToken": "sk-ant-ort01-{}",
                    "expiresAt": 2000003600000
                }}
            }}"#,
            account_id, email, account_id, account_id
        );
        std::fs::write(dir.join(format!("{}.json", account_id)), json).unwrap();
    }

    #[tokio::test]
    async fn test_read_account() {
        let dir = TestDir::new();
        write_v3_account(&dir.path, "a1", "a1@test.com");
        let mgr = AccountManager::new(dir.path.clone());
        let account = mgr.read("a1").await.unwrap();
        assert_eq!(account.account_id, "a1");
        assert_eq!(account.email, "a1@test.com");
    }

    #[tokio::test]
    async fn test_read_nonexistent_returns_error() {
        let dir = TestDir::new();
        let mgr = AccountManager::new(dir.path.clone());
        assert!(mgr.read("no_such").await.is_err());
    }

    #[tokio::test]
    async fn test_update_then_read_consistent() {
        let dir = TestDir::new();
        write_v3_account(&dir.path, "a1", "a1@test.com");
        let mgr = AccountManager::new(dir.path.clone());

        mgr.update("a1", |a| {
            a.disabled = true;
            a.disabled_reason = Some("test disable".to_string());
        })
        .await
        .unwrap();

        let account = mgr.read("a1").await.unwrap();
        assert!(account.disabled);
        assert_eq!(account.disabled_reason, Some("test disable".to_string()));
    }

    #[tokio::test]
    async fn test_concurrent_writes_no_data_loss() {
        let dir = TestDir::new();
        // Account with a counter field we'll use custom_label for
        let json = r#"{"accountId":"cnt","email":"cnt@t.com","customLabel":"0"}"#;
        std::fs::write(dir.path.join("cnt.json"), json).unwrap();

        let mgr = Arc::new(AccountManager::new(dir.path.clone()));
        let mut handles = vec![];

        for i in 0..10 {
            let mgr = mgr.clone();
            handles.push(tokio::spawn(async move {
                mgr.update("cnt", |a| {
                    let current: i32 = a.custom_label.as_deref().unwrap_or("0").parse().unwrap_or(0);
                    a.custom_label = Some((current + 1).to_string());
                    // Also track which task ran
                    let _ = i;
                })
                .await
                .unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let account = mgr.read("cnt").await.unwrap();
        let final_val: i32 = account.custom_label.as_deref().unwrap().parse().unwrap();
        assert_eq!(final_val, 10, "All 10 concurrent updates should be applied sequentially");
    }

    #[tokio::test]
    async fn test_list_accounts() {
        let dir = TestDir::new();
        write_v3_account(&dir.path, "a1", "a1@test.com");
        write_v3_account(&dir.path, "a2", "a2@test.com");
        let mgr = AccountManager::new(dir.path.clone());
        let accounts = mgr.list().await.unwrap();
        assert_eq!(accounts.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_account() {
        let dir = TestDir::new();
        write_v3_account(&dir.path, "a1", "a1@test.com");
        let mgr = AccountManager::new(dir.path.clone());
        mgr.delete("a1").await.unwrap();
        assert!(!dir.path.join("a1.json").exists());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_returns_error() {
        let dir = TestDir::new();
        let mgr = AccountManager::new(dir.path.clone());
        assert!(mgr.delete("no_such").await.is_err());
    }

    #[tokio::test]
    async fn test_write_new_account() {
        let dir = TestDir::new();
        let mgr = AccountManager::new(dir.path.clone());
        let account: Account = serde_json::from_str(
            r#"{"accountId":"new1","email":"new@test.com"}"#
        ).unwrap();
        mgr.write_new(&account).await.unwrap();
        let loaded = mgr.read("new1").await.unwrap();
        assert_eq!(loaded.email, "new@test.com");
    }

    #[tokio::test]
    async fn test_write_new_duplicate_returns_error() {
        let dir = TestDir::new();
        write_v3_account(&dir.path, "a1", "a1@test.com");
        let mgr = AccountManager::new(dir.path.clone());
        let account: Account = serde_json::from_str(
            r#"{"accountId":"a1","email":"dup@test.com"}"#
        ).unwrap();
        assert!(mgr.write_new(&account).await.is_err());
    }

}
