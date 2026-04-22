//! AccountManager — unified concurrent access to Account JSON files.
//! All Account JSON writes go through this manager (per-account RwLock).

use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::models::account::Account;
use crate::modules::account_change::{AccountChange, AccountConsumers};

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

    /// Write to disk + route change to consumers.
    pub async fn apply_change<F>(
        &self,
        account_id: &str,
        f: F,
        change: AccountChange,
    ) -> Result<(), String>
    where
        F: FnOnce(&mut Account),
    {
        self.update(account_id, f).await?;
        self.route_change(account_id, change).await;
        Ok(())
    }

    /// Route a change to consumers. Called after persistence is done.
    /// Uses Box::pin for recursion: Created may chain into CliTokenUpdated.
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

                    // If account has cli with non-expired token, force-refresh to verify it's usable
                    let cli_expires_at = self.read(account_id).await
                        .ok()
                        .and_then(|a| a.cli.as_ref().map(|c| c.expires_at));
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let has_valid_cli = cli_expires_at.map_or(false, |ea| ea > now_ms);
                    if has_valid_cli {
                        match consumers.token_allocator.force_refresh_token(account_id).await {
                            Ok(_new_token) => {
                                // Token refreshed → CliTokenUpdated handles ClientManager + emit
                                if let Ok(account) = self.read(account_id).await {
                                    if let Some(cli) = account.cli.as_ref() {
                                        self.route_change(account_id, AccountChange::CliTokenUpdated {
                                            access_token: cli.access_token.clone(),
                                            refresh_token: cli.refresh_token.clone(),
                                            expires_at: cli.expires_at,
                                        }).await;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Created: token refresh failed for {}: {}", account_id, e);
                                consumers.emit_frontend("account://changed", account_id);
                            }
                        }
                    } else {
                        // No cli or token expired — just notify frontend
                        consumers.emit_frontend("account://changed", account_id);
                    }
                }
                AccountChange::CliTokenUpdated { access_token, refresh_token, expires_at } => {
                    if consumers.client_manager.has_client(account_id) {
                        consumers.client_manager.update_client_token(
                            account_id, access_token, refresh_token, expires_at,
                        );
                    } else {
                        // Only add to pool if account is not disabled
                        if let Ok(account) = self.read(account_id).await {
                            if !account.disabled && !account.user_disabled {
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
    pub async fn update<F>(&self, account_id: &str, f: F) -> Result<(), String>
    where
        F: FnOnce(&mut Account),
    {
        let lock = self.get_lock(account_id);
        let _guard = lock.write().await;
        let mut account = self.read_from_file(account_id)?;
        f(&mut account);
        self.write_to_file(account_id, &account)
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
