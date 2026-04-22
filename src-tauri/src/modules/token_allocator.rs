//! TokenAllocator — ensure valid OAuth tokens with per-account locking.
//!
//! Pre-checks token expiry, refreshes via CliClient.refresh() if needed,
//! persists to Account JSON. Consumer notification (ClientManager) is handled
//! by the caller via route_change(CliTokenUpdated).

use dashmap::DashMap;
use std::sync::Arc;

use claude_ultra_http::BoringClient;
use crate::modules::account_manager::AccountManager;
use crate::modules::cli_client::{CliClient, CliClientError};
use crate::proxy::allocator::ProxyAllocator;

/// Buffer before expiry to trigger refresh (90 minutes in ms).
/// Must be > AccountMonitor check interval (3600s = 60min) to guarantee
/// at least one check within the buffer window.
const TOKEN_REFRESH_BUFFER_MS: i64 = 90 * 60 * 1000;

pub struct TokenAllocator {
    account_manager: Arc<AccountManager>,
    proxy_allocator: Arc<ProxyAllocator>,
    boring_client: Arc<BoringClient>,
    refresh_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

impl TokenAllocator {
    pub fn new(
        account_manager: Arc<AccountManager>,
        proxy_allocator: Arc<ProxyAllocator>,
        boring_client: Arc<BoringClient>,
    ) -> Self {
        Self {
            account_manager,
            proxy_allocator,
            boring_client,
            refresh_locks: DashMap::new(),
        }
    }

    /// Check if token expires within the buffer window.
    pub fn is_token_expiring(expires_at: i64) -> bool {
        let now_ms = chrono::Utc::now().timestamp_millis();
        now_ms + TOKEN_REFRESH_BUFFER_MS >= expires_at
    }

    /// Ensure the account's token is valid.
    /// Returns (access_token, refreshed: bool).
    /// refreshed=true means a new token was obtained and persisted to JSON.
    /// Caller should route_change(CliTokenUpdated) when refreshed=true.
    pub async fn ensure_valid_token(&self, account_id: &str) -> Result<(String, bool), String> {
        // Read account to check expiry
        let account = self.account_manager.read(account_id).await
            .map_err(|e| format!("Failed to read account {}: {}", account_id, e))?;
        let cli = account.cli.as_ref()
            .ok_or_else(|| format!("Account {} has no CLI credentials", account_id))?;

        // Not expiring → return current token, no refresh happened
        if !Self::is_token_expiring(cli.expires_at) {
            return Ok((cli.access_token.clone(), false));
        }

        let token = self.do_refresh(account_id, false).await?;
        Ok((token, true))
    }

    /// Force refresh the token regardless of expiry (used after 401 responses).
    /// Acquires per-account lock, performs double-check, and refreshes.
    pub async fn force_refresh_token(&self, account_id: &str) -> Result<String, String> {
        // Validate account exists and has CLI credentials before locking
        let account = self.account_manager.read(account_id).await
            .map_err(|e| format!("Failed to read account {}: {}", account_id, e))?;
        account.cli.as_ref()
            .ok_or_else(|| format!("Account {} has no CLI credentials", account_id))?;

        self.do_refresh(account_id, true).await
    }

    /// Core refresh logic: acquire per-account lock, double-check expiry, refresh.
    /// force=true skips the double-check (for force_refresh_token).
    async fn do_refresh(&self, account_id: &str, force: bool) -> Result<String, String> {
        let lock = self.refresh_locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();

        let _guard = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            lock.lock(),
        ).await.map_err(|_| format!("Refresh lock timeout for {}", account_id))?;

        // Double-check after acquiring lock (another thread may have refreshed)
        let account = self.account_manager.read(account_id).await
            .map_err(|e| format!("Failed to re-read account {}: {}", account_id, e))?;
        let cli = account.cli.as_ref()
            .ok_or_else(|| format!("Account {} has no CLI credentials", account_id))?;

        if !force && !Self::is_token_expiring(cli.expires_at) {
            // Another thread already refreshed while we were waiting for the lock
            return Ok(cli.access_token.clone());
        }

        let proxy_url = self.proxy_allocator.get_proxy_url_optional(account_id).await
            .map_err(|e| format!("get_proxy_url: {}", e))?;

        let mut temp_client = CliClient::new(
            self.boring_client.clone(),
            account_id.to_string(),
            cli.access_token.clone(),
            cli.refresh_token.clone(),
            cli.expires_at,
            proxy_url,
        );

        let result = match temp_client.refresh().await {
            Ok(r) => r,
            Err(e) => {
                // Only renew proxy on connection failures, not on business errors
                if matches!(&e, CliClientError::Http(_)) {
                    tracing::warn!("Token refresh proxy error for {}, renewing: {}", account_id, e);
                    self.proxy_allocator.renew_active_proxy(account_id).await;
                }
                return Err(format!("Token refresh failed for {}: {}", account_id, e));
            }
        };

        // Persist to Account JSON
        let at = result.access_token.clone();
        let rt = result.refresh_token.clone();
        let ea = result.expires_at;
        self.account_manager.update(account_id, move |a| {
            if let Some(ref mut cli) = a.cli {
                cli.access_token = at;
                cli.refresh_token = rt;
                cli.expires_at = ea;
            }
        }).await.map_err(|e| format!("Failed to persist token for {}: {}", account_id, e))?;

        // ClientManager notification is handled by the caller via route_change(CliTokenUpdated).
        // do_refresh only persists — it does not notify consumers directly.

        tracing::info!(
            "TokenAllocator: refreshed token for {}, new expires_at={}",
            account_id, result.expires_at
        );

        Ok(result.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::proxy::config::ProxyProviderConfig;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "claude_ultra_token_alloc_test_{}",
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

    fn make_provider_config() -> ProxyProviderConfig {
        ProxyProviderConfig {
            host: "127.0.0.99".to_string(),
            port: 19999,
            username: "testuser".to_string(),
            password: "testpass".to_string(),
            default_country: "US".to_string(),
            max_allocate_retries: 1,
        }
    }

    fn write_account(dir: &std::path::Path, id: &str, expires_at: i64) {
        let json = format!(
            r#"{{"accountId":"{}","email":"{}@t.com","cli":{{"accessToken":"at-{}","refreshToken":"rt-{}","expiresAt":{}}}}}"#,
            id, id, id, id, expires_at
        );
        std::fs::write(dir.join(format!("{}.json", id)), json).unwrap();
    }

    fn make_token_allocator(dir: &std::path::Path) -> (Arc<TokenAllocator>, Arc<AccountManager>) {
        let am = Arc::new(AccountManager::new(dir.to_path_buf()));
        let config = Arc::new(make_provider_config());
        let pa = Arc::new(ProxyAllocator::new(am.clone(), config));
        let bc = Arc::new(BoringClient::builder().build().unwrap());
        let ta = Arc::new(TokenAllocator::new(am.clone(), pa, bc));
        (ta, am)
    }

    // ── is_token_expiring ───────────────────────────────────

    #[test]
    fn test_is_token_expiring_expired() {
        let past = chrono::Utc::now().timestamp_millis() - 1000;
        assert!(TokenAllocator::is_token_expiring(past));
    }

    #[test]
    fn test_is_token_expiring_within_buffer() {
        // 60 minutes from now (< 90 min buffer)
        let soon = chrono::Utc::now().timestamp_millis() + 60 * 60 * 1000;
        assert!(TokenAllocator::is_token_expiring(soon));
    }

    #[test]
    fn test_is_token_expiring_far_future() {
        // 3 hours from now (> 90 min buffer)
        let future = chrono::Utc::now().timestamp_millis() + 3 * 60 * 60 * 1000;
        assert!(!TokenAllocator::is_token_expiring(future));
    }

    // ── ensure_valid_token: not expiring → returns immediately ──

    #[tokio::test]
    async fn test_ensure_valid_token_not_expiring_returns_current() {
        let dir = TestDir::new();
        let far_future = chrono::Utc::now().timestamp_millis() + 2 * 60 * 60 * 1000;
        write_account(&dir.path, "a1", far_future);
        let (ta, _) = make_token_allocator(&dir.path);

        let (token, refreshed) = ta.ensure_valid_token("a1").await.unwrap();
        assert_eq!(token, "at-a1", "should return current access_token without refresh");
        assert!(!refreshed, "should not have refreshed");
    }

    // ── ensure_valid_token: no CLI credentials → error ──

    #[tokio::test]
    async fn test_ensure_valid_token_no_cli_credentials() {
        let dir = TestDir::new();
        let json = r#"{"accountId":"nocli","email":"nocli@t.com"}"#;
        std::fs::write(dir.path.join("nocli.json"), json).unwrap();
        let am = Arc::new(AccountManager::new(dir.path.clone()));
        let config = Arc::new(make_provider_config());
        let pa = Arc::new(ProxyAllocator::new(am.clone(), config));
        let bc = Arc::new(BoringClient::builder().build().unwrap());
        let ta = TokenAllocator::new(am, pa, bc);

        let result = ta.ensure_valid_token("nocli").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no CLI credentials"));
    }

    // ── ensure_valid_token: nonexistent account → error ──

    #[tokio::test]
    async fn test_ensure_valid_token_nonexistent_account() {
        let dir = TestDir::new();
        let am = Arc::new(AccountManager::new(dir.path.clone()));
        let config = Arc::new(make_provider_config());
        let pa = Arc::new(ProxyAllocator::new(am.clone(), config));
        let bc = Arc::new(BoringClient::builder().build().unwrap());
        let ta = TokenAllocator::new(am, pa, bc);

        let result = ta.ensure_valid_token("nonexistent").await;
        assert!(result.is_err());
    }

    // ── ensure_valid_token: expired token → tries refresh → fails (no real proxy) ──

    #[tokio::test]
    async fn test_ensure_valid_token_expired_refresh_fails_no_proxy() {
        let dir = TestDir::new();
        // Token expired in the past
        let expired = chrono::Utc::now().timestamp_millis() - 60_000;
        write_account(&dir.path, "a1", expired);
        let (ta, _) = make_token_allocator(&dir.path);

        let result = ta.ensure_valid_token("a1").await;
        assert!(result.is_err(), "refresh should fail with no real proxy");
        assert!(result.unwrap_err().contains("Token refresh failed"));
    }

    // ── force_refresh_token: always refreshes regardless of expiry ──

    #[tokio::test]
    async fn test_force_refresh_token_on_valid_token_still_refreshes() {
        let dir = TestDir::new();
        // Token far in the future — ensure_valid_token would skip, but force should attempt refresh
        let far_future = chrono::Utc::now().timestamp_millis() + 2 * 60 * 60 * 1000;
        write_account(&dir.path, "a1", far_future);
        let (ta, _) = make_token_allocator(&dir.path);

        // force_refresh should attempt refresh even though token is valid
        // It will fail because no real proxy, but it should TRY (not return old token)
        let result = ta.force_refresh_token("a1").await;
        assert!(result.is_err(), "force refresh should attempt refresh and fail with no real proxy");
        assert!(result.unwrap_err().contains("Token refresh failed"));
    }

    #[tokio::test]
    async fn test_force_refresh_token_no_cli_credentials() {
        let dir = TestDir::new();
        let json = r#"{"accountId":"nocli","email":"nocli@t.com"}"#;
        std::fs::write(dir.path.join("nocli.json"), json).unwrap();
        let am = Arc::new(AccountManager::new(dir.path.clone()));
        let config = Arc::new(make_provider_config());
        let pa = Arc::new(ProxyAllocator::new(am.clone(), config));
        let bc = Arc::new(BoringClient::builder().build().unwrap());
        let ta = TokenAllocator::new(am, pa, bc);

        let result = ta.force_refresh_token("nocli").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no CLI credentials"));
    }

    // ── Concurrent refresh: per-account locking (no deadlock) ──

    #[tokio::test]
    async fn test_concurrent_ensure_valid_token_no_deadlock() {
        let dir = TestDir::new();
        let expired = chrono::Utc::now().timestamp_millis() - 60_000;
        write_account(&dir.path, "a1", expired);
        let (ta, _) = make_token_allocator(&dir.path);

        let ta1 = ta.clone();
        let ta2 = ta.clone();

        let (r1, r2) = tokio::join!(
            ta1.ensure_valid_token("a1"),
            ta2.ensure_valid_token("a1"),
        );

        // Both should fail (no real proxy), but no deadlock
        assert!(r1.is_err());
        assert!(r2.is_err());
    }
}
