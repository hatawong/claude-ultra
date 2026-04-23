//! AccountMonitor — background periodic token health checks + quota refresh.
//! Iterates all accounts with CLI credentials and calls TokenAllocator.ensure_valid_token().
//! Also refreshes quota data for accounts with missing or stale utilization.

use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::modules::account_manager::AccountManager;
use crate::modules::token_allocator::TokenAllocator;
use crate::proxy::allocator::ProxyAllocator;
/// Default check interval: 30 minutes.
const ACCOUNT_CHECK_INTERVAL_SECS: u64 = 1800;

/// Result of a single monitor check round.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckResult {
    /// Checks completed — token + quota counts.
    Completed {
        checked: usize,
        refreshed: usize,
        failed: usize,
        quota_refreshed: usize,
        quota_failed: usize,
    },
    /// No accounts with CLI credentials to check.
    NoAccounts,
}

pub struct AccountMonitor {
    account_manager: Arc<AccountManager>,
    token_allocator: Arc<TokenAllocator>,
    proxy_allocator: Arc<ProxyAllocator>,
    boring_client: Arc<claude_ultra_http::BoringClient>,
    check_interval_secs: u64,
    cancel_token: CancellationToken,
}

impl AccountMonitor {
    pub fn new(
        account_manager: Arc<AccountManager>,
        token_allocator: Arc<TokenAllocator>,
        proxy_allocator: Arc<ProxyAllocator>,
        boring_client: Arc<claude_ultra_http::BoringClient>,
    ) -> Self {
        Self {
            account_manager,
            token_allocator,
            proxy_allocator,
            boring_client,
            check_interval_secs: ACCOUNT_CHECK_INTERVAL_SECS,
            cancel_token: CancellationToken::new(),
        }
    }

    #[cfg(test)]
    pub fn with_interval(mut self, interval_secs: u64) -> Self {
        self.check_interval_secs = interval_secs;
        self
    }

    /// Start the background check loop. Runs first check immediately.
    /// The loop exits when `stop()` is called.
    pub fn start(self: Arc<Self>) {
        let cancel = self.cancel_token.clone();
        tokio::spawn(async move {
            // Run first check immediately
            let result = self.check_all().await;
            tracing::info!("AccountMonitor: initial check result: {:?}", result);

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!("AccountMonitor: cancelled, exiting loop");
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(self.check_interval_secs)) => {
                        let result = self.check_all().await;
                        tracing::info!("AccountMonitor: check result: {:?}", result);
                    }
                }
            }
        });
    }

    /// Stop the background check loop.
    pub fn stop(&self) {
        self.cancel_token.cancel();
    }

    /// Run one round of token checks + quota refresh. Returns the result for testing.
    pub async fn check_all(&self) -> CheckResult {
        let accounts = match self.account_manager.list().await {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("AccountMonitor: failed to list accounts: {}", e);
                return CheckResult::Completed {
                    checked: 0,
                    refreshed: 0,
                    failed: 0,
                    quota_refreshed: 0,
                    quota_failed: 0,
                };
            }
        };

        // All accounts with CLI credentials are eligible (token refresh logic checked per-account in loop)
        let eligible: Vec<_> = accounts
            .into_iter()
            .filter(|a| a.cli.is_some())
            .collect();

        if eligible.is_empty() {
            return CheckResult::NoAccounts;
        }

        let mut checked = 0;
        let mut refreshed = 0;
        let mut failed = 0;
        let mut quota_refreshed = 0;
        let mut quota_failed = 0;

        let now_ms = chrono::Utc::now().timestamp_millis();

        for account in &eligible {
            checked += 1;
            let cli = account.cli.as_ref().unwrap();
            let token_expired = cli.expires_at <= now_ms;

            // ── Token refresh ──
            // Not disabled + (expired or expiring soon) → refresh
            // Disabled accounts skip refresh (plan downgraded, refresh is pointless)
            if !account.disabled && (token_expired || TokenAllocator::is_token_expiring(cli.expires_at)) {
                match self.token_allocator.ensure_valid_token(&account.account_id).await {
                    Ok((_token, did_refresh)) => {
                        if did_refresh {
                            refreshed += 1;
                            tracing::info!("AccountMonitor: refreshed token for {}", account.account_id);
                            // set_cli in token_allocator already emitted CliUpdated
                        }
                    }
                    Err(e) => {
                        failed += 1;
                        tracing::warn!("AccountMonitor: token refresh failed for {}: {}", account.account_id, e);
                    }
                }
            }

            // ── Usage refresh ──
            // Not disabled + token not expired → refresh
            // Disabled accounts skip (get_usage returns 403)
            // Expired token accounts skip (get_usage returns 401)
            // Note: token may have just been refreshed above, re-read expires_at
            let token_still_expired = self.account_manager.read(&account.account_id).await
                .ok()
                .and_then(|a| a.cli.as_ref().map(|c| c.expires_at <= now_ms))
                .unwrap_or(true);
            if !account.disabled && !token_still_expired {
                match self.refresh_quota(&account.account_id).await {
                    Ok(_) => quota_refreshed += 1,
                    Err(e) => {
                        tracing::warn!("AccountMonitor: quota refresh failed for {}: {}", account.account_id, e);
                        quota_failed += 1;
                    }
                }
            }
        }

        tracing::info!(
            "AccountMonitor: checked {} accounts, refreshed {}, failed {}, quota_refreshed {}, quota_failed {}",
            checked, refreshed, failed, quota_refreshed, quota_failed
        );

        CheckResult::Completed {
            checked,
            refreshed,
            failed,
            quota_refreshed,
            quota_failed,
        }
    }

    /// Refresh quota for a single account by calling /api/oauth/usage.
    async fn refresh_quota(&self, account_id: &str) -> Result<(), String> {
        let account = self.account_manager.read(account_id).await
            .map_err(|e| format!("read: {}", e))?;
        let cli = account.cli.as_ref().ok_or("No CLI credentials")?;

        let proxy_url = self.proxy_allocator.get_proxy_url_optional(account_id).await
            .map_err(|e| format!("get_proxy_url: {}", e))?;

        let client = crate::modules::cli_client::CliClient::new(
            self.boring_client.clone(),
            account_id.to_string(),
            cli.access_token.clone(),
            cli.refresh_token.clone(),
            cli.expires_at,
            proxy_url,
        );

        let usage = match client.get_usage().await {
            Ok(u) => u,
            Err(e) => {
                if let crate::modules::cli_client::CliClientError::HttpStatus(status, ref msg) = e {
                    if matches!(status, 401 | 403) {
                        let reason = format!("HTTP {}: {}", status, msg);
                        tracing::warn!("get_usage: disabling account {} — HTTP {}", account_id, status);
                        let _ = self.account_manager.set_disabled(account_id, Some(reason)).await;
                        return Err(format!("get_usage: account disabled (HTTP {})", status));
                    }
                }
                // Only renew proxy on connection-level failures, not on business errors (parse etc.)
                if matches!(&e, crate::modules::cli_client::CliClientError::Http(_)) {
                    tracing::warn!("get_usage proxy error for {}, renewing: {}", account_id, e);
                    self.proxy_allocator.renew_active_proxy(account_id).await;
                }
                return Err(format!("get_usage: {}", e));
            }
        };
        let usage_value = serde_json::to_value(&usage)
            .map_err(|e| format!("serialize: {}", e))?;
        let snapshot = crate::models::quota::utilization_to_snapshot(&usage);

        self.account_manager.set_utilization(account_id, usage_value, snapshot)
            .await.map_err(|e| format!("set_utilization: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::client_manager::ClientManager;
    use crate::modules::token_allocator::TokenAllocator;
    use crate::proxy::allocator::ProxyAllocator;
    use crate::proxy::config::ProxyProviderConfig;
    use std::path::PathBuf;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "claude_ultra_acct_mon_test_{}",
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

    fn write_account(dir: &std::path::Path, id: &str, expires_at: i64, disabled: bool) {
        let json = format!(
            r#"{{"accountId":"{}","email":"{}@t.com","disabled":{},"cli":{{"accessToken":"at-{}","refreshToken":"rt-{}","expiresAt":{}}}}}"#,
            id, id, disabled, id, id, expires_at
        );
        std::fs::write(dir.join(format!("{}.json", id)), json).unwrap();
    }

    fn make_monitor(dir: &std::path::Path) -> Arc<AccountMonitor> {
        let am = Arc::new(AccountManager::new(dir.to_path_buf()));
        let config = Arc::new(make_provider_config());
        let pa = Arc::new(ProxyAllocator::new(am.clone(), config));
        let bc = Arc::new(claude_ultra_http::BoringClient::builder().build().unwrap());
        let cm = Arc::new(ClientManager::new(bc.clone()));
        cm.load_clients(dir).unwrap();
        let ta = Arc::new(TokenAllocator::new(am.clone(), pa.clone(), bc.clone()));
        Arc::new(AccountMonitor::new(am, ta, pa, bc))
    }

    // ── No accounts → NoAccounts ────────────────────────────

    #[tokio::test]
    async fn test_no_accounts_returns_no_accounts() {
        let dir = TestDir::new();
        let monitor = make_monitor(&dir.path);
        let result = monitor.check_all().await;
        assert_eq!(result, CheckResult::NoAccounts);
    }

    // ── Basics: no CLI / no accounts ──────────────────────────

    #[tokio::test]
    async fn test_no_cli_accounts_skipped() {
        let dir = TestDir::new();
        let json = r#"{"accountId":"nocli","email":"nocli@t.com"}"#;
        std::fs::write(dir.path.join("nocli.json"), json).unwrap();
        let monitor = make_monitor(&dir.path);
        assert_eq!(monitor.check_all().await, CheckResult::NoAccounts);
    }

    // ── Token refresh tests ──────────────────────────────────
    // Rule: not expired but expiring soon (within buffer) → refresh, otherwise skip

    #[tokio::test]
    async fn test_token_far_from_expiry_no_refresh() {
        // Token has 2 hours left, outside 90-minute buffer → no refresh
        let dir = TestDir::new();
        let far_future = chrono::Utc::now().timestamp_millis() + 2 * 60 * 60 * 1000;
        write_account(&dir.path, "a1", far_future, false);
        let monitor = make_monitor(&dir.path);
        let result = monitor.check_all().await;
        match result {
            CheckResult::Completed { refreshed, failed, .. } => {
                assert_eq!(refreshed, 0, "token far from expiry, no refresh");
                assert_eq!(failed, 0);
            }
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_token_expired_attempts_refresh() {
        // Token expired → attempt refresh (refresh_token may still be valid)
        let dir = TestDir::new();
        let expired = chrono::Utc::now().timestamp_millis() - 60_000;
        write_account(&dir.path, "a1", expired, false);
        let monitor = make_monitor(&dir.path);
        let result = monitor.check_all().await;
        match result {
            CheckResult::Completed { failed, .. } => {
                assert_eq!(failed, 1, "attempted refresh but failed without proxy");
            }
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    // ── Usage refresh tests ──────────────────────────────────
    // Rules: 1. no token → skip 2. expired token → skip 3. disabled → skip 4. otherwise → refresh

    #[tokio::test]
    async fn test_usage_valid_token_refreshed() {
        // Valid token + not disabled → refresh usage (fails due to no proxy)
        let dir = TestDir::new();
        let far_future = chrono::Utc::now().timestamp_millis() + 2 * 60 * 60 * 1000;
        write_account(&dir.path, "a1", far_future, false);
        let monitor = make_monitor(&dir.path);
        let result = monitor.check_all().await;
        match result {
            CheckResult::Completed { quota_failed, .. } => {
                assert_eq!(quota_failed, 1, "attempted refresh but failed without proxy");
            }
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_usage_expired_token_skip() {
        // Token expired → skip usage refresh
        let dir = TestDir::new();
        let expired = chrono::Utc::now().timestamp_millis() - 60_000;
        write_account(&dir.path, "a1", expired, false);
        let monitor = make_monitor(&dir.path);
        let result = monitor.check_all().await;
        match result {
            CheckResult::Completed { quota_refreshed, quota_failed, .. } => {
                assert_eq!(quota_refreshed, 0);
                assert_eq!(quota_failed, 0, "expired token should not attempt usage refresh");
            }
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_usage_disabled_skip() {
        // Disabled (banned) → skip usage refresh
        let dir = TestDir::new();
        let far_future = chrono::Utc::now().timestamp_millis() + 2 * 60 * 60 * 1000;
        write_account(&dir.path, "a1", far_future, true); // disabled
        let monitor = make_monitor(&dir.path);
        let result = monitor.check_all().await;
        match result {
            CheckResult::Completed { checked, quota_refreshed, quota_failed, .. } => {
                assert_eq!(checked, 1, "disabled accounts are still eligible");
                assert_eq!(quota_refreshed, 0);
                assert_eq!(quota_failed, 0, "disabled accounts should not attempt usage refresh");
            }
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_usage_mixed_accounts() {
        // a1: valid → refresh (fails), a2: expired → skip
        let dir = TestDir::new();
        let far_future = chrono::Utc::now().timestamp_millis() + 2 * 60 * 60 * 1000;
        let expired = chrono::Utc::now().timestamp_millis() - 60_000;
        write_account(&dir.path, "a1", far_future, false);
        write_account(&dir.path, "a2", expired, false);
        let monitor = make_monitor(&dir.path);
        let result = monitor.check_all().await;
        match result {
            CheckResult::Completed { checked, quota_failed, .. } => {
                assert_eq!(checked, 2);
                assert_eq!(quota_failed, 1, "only a1 should attempt usage refresh");
            }
            other => panic!("expected Completed, got {:?}", other),
        }
    }

    // ── CheckResult enum ────────────────────────────────────

    #[test]
    fn test_check_result_equality() {
        assert_eq!(CheckResult::NoAccounts, CheckResult::NoAccounts);
        assert_eq!(
            CheckResult::Completed { checked: 1, refreshed: 0, failed: 0, quota_refreshed: 0, quota_failed: 0 },
            CheckResult::Completed { checked: 1, refreshed: 0, failed: 0, quota_refreshed: 0, quota_failed: 0 },
        );
        assert_ne!(CheckResult::NoAccounts, CheckResult::Completed { checked: 0, refreshed: 0, failed: 0, quota_refreshed: 0, quota_failed: 0 });
    }

    #[test]
    fn test_check_result_debug() {
        let result = CheckResult::Completed { checked: 3, refreshed: 1, failed: 0, quota_refreshed: 0, quota_failed: 0 };
        let debug = format!("{:?}", result);
        assert!(debug.contains("Completed"));
        assert!(debug.contains("checked: 3"));
    }

    // ── Lifecycle: start → stop → restart ───────────────────

    fn make_monitor_with_interval(dir: &std::path::Path, interval_secs: u64) -> Arc<AccountMonitor> {
        let am = Arc::new(AccountManager::new(dir.to_path_buf()));
        let config = Arc::new(make_provider_config());
        let pa = Arc::new(ProxyAllocator::new(am.clone(), config));
        let bc = Arc::new(claude_ultra_http::BoringClient::builder().build().unwrap());
        let ta = Arc::new(TokenAllocator::new(am.clone(), pa.clone(), bc.clone()));
        Arc::new(AccountMonitor::new(am, ta, pa, bc).with_interval(interval_secs))
    }

    #[tokio::test]
    async fn test_monitor_stop_exits_loop() {
        let dir = TestDir::new();
        let monitor = make_monitor_with_interval(&dir.path, 1);

        monitor.clone().start();

        // Give loop time to run initial check
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Stop should cancel the token
        monitor.stop();

        // Verify cancel_token is cancelled
        assert!(monitor.cancel_token.is_cancelled());
    }

    #[tokio::test]
    async fn test_monitor_start_stop_restart() {
        let dir = TestDir::new();

        // First monitor: start → stop
        let monitor1 = make_monitor_with_interval(&dir.path, 3600);
        monitor1.clone().start();
        monitor1.stop();
        assert!(monitor1.cancel_token.is_cancelled());

        // Second monitor: fresh instance, start should work
        let monitor2 = make_monitor_with_interval(&dir.path, 3600);
        monitor2.clone().start();
        assert!(!monitor2.cancel_token.is_cancelled(), "new monitor should have fresh token");
        monitor2.stop();
        assert!(monitor2.cancel_token.is_cancelled());
    }
}
