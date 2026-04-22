//! ClientManager — manages a pool of CliClients with round-robin selection.
//!
//! Migrated from gateway/token_manager.rs. Uses CliClient instead of ProxyToken.
//! Adds TempUnschedule support for temporary account exclusion.
//! Does not depend on Tauri.

use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use claude_ultra_http::BoringClient;
use crate::models::account::Account;
use crate::models::quota::QuotaSnapshot;
#[cfg(test)]
use crate::models::quota::ClaudeAILimit;
use crate::modules::cli_client::CliClient;

/// Per-account runtime state — not persisted, generated on startup.
#[derive(Debug, Clone)]
pub struct AccountRuntimeState {
    pub session_uuid: String,
    pub device_id: String,
    pub account_uuid: String, // Anthropic's account UUID (from get_profile)
    pub quota: Option<QuotaSnapshot>,
    pub model_rate_limits: HashMap<String, Instant>,
}

/// Manages a pool of CliClients with round-robin selection, watermark filtering,
/// model rate-limit filtering, and TempUnschedule.
pub struct ClientManager {
    /// CliClient pool keyed by account_id.
    clients: Arc<DashMap<String, CliClient>>,
    /// Per-account runtime state (session_uuid, device_id, quota, model rate limits).
    runtime_state: DashMap<String, AccountRuntimeState>,
    /// Ordered account IDs for round-robin.
    ordered_ids: RwLock<Vec<String>>,
    /// Round-robin index.
    current_index: Arc<AtomicUsize>,
    /// Cancellation token for shutdown.
    cancel_token: CancellationToken,
    /// Shared BoringClient for constructing CliClients.
    boring_client: Arc<BoringClient>,
    /// Temp unschedule map: account_id → (until_ms, reason).
    temp_unschedulable: DashMap<String, (i64, String)>,
    /// Session affinity: session_id → (account_id, last_used_epoch_ms)
    session_map: DashMap<String, (String, i64)>,
    /// Watermark thresholds (0.0-1.0).
    safety_watermark: f64,
    alert_watermark: f64,
}

impl ClientManager {
    pub fn new(boring_client: Arc<BoringClient>) -> Self {
        Self {
            clients: Arc::new(DashMap::new()),
            runtime_state: DashMap::new(),
            ordered_ids: RwLock::new(Vec::new()),
            current_index: Arc::new(AtomicUsize::new(0)),
            cancel_token: CancellationToken::new(),
            boring_client,
            temp_unschedulable: DashMap::new(),
            session_map: DashMap::new(),
            safety_watermark: 0.8,
            alert_watermark: 0.95,
        }
    }

    pub fn with_watermarks(mut self, safety: f64, alert: f64) -> Self {
        self.safety_watermark = safety;
        self.alert_watermark = alert;
        self
    }

    /// Validate device_id format: must be exactly 64 lowercase hex chars.
    fn is_valid_device_id(device_id: &str) -> bool {
        device_id.len() == 64 && device_id.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Validate account_uuid format: must be a valid UUID.
    fn is_valid_account_uuid(uuid: &str) -> bool {
        uuid::Uuid::parse_str(uuid).is_ok()
    }

    /// Load accounts from V3 JSON directory. Creates CliClient from cli sub-object.
    /// Skips: disabled, user_disabled, cli=None, unparseable files.
    pub fn load_clients(&self, dir: &Path) -> Result<usize, String> {
        if !dir.exists() {
            return Ok(0);
        }

        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("Failed to read accounts dir: {}", e))?;

        self.clients.clear();
        self.runtime_state.clear();
        self.temp_unschedulable.clear();
        self.session_map.clear();

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.extension().is_some_and(|ext| ext == "json") {
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("Failed to read {}: {}", path.display(), e);
                    continue;
                }
            };

            let account: Account = match serde_json::from_str(&content) {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!("Failed to parse {}: {}", path.display(), e);
                    eprintln!("Warning: failed to parse {}: {}", path.display(), e);
                    continue;
                }
            };

            if account.disabled {
                tracing::info!("Skipping disabled account {}", account.account_id);
                continue;
            }
            if account.user_disabled {
                tracing::info!("Skipping user-disabled account {}", account.account_id);
                continue;
            }
            let cli_cred = match &account.cli {
                Some(c) => c,
                None => {
                    tracing::info!("Skipping account {} (no CLI credentials)", account.account_id);
                    continue;
                }
            };

            let id = account.account_id.clone();

            if !Self::is_valid_device_id(&cli_cred.device_id) {
                tracing::warn!("Skipping account {} (invalid device_id: {})", id, &cli_cred.device_id);
                continue;
            }
            if !Self::is_valid_account_uuid(&account.account_uuid) {
                tracing::warn!("Skipping account {} (invalid account_uuid: {})", id, &account.account_uuid);
                continue;
            }

            let cli = CliClient::new(
                self.boring_client.clone(),
                id.clone(),
                cli_cred.access_token.clone(),
                cli_cred.refresh_token.clone(),
                cli_cred.expires_at,
                None, // proxy_url set by handler from ProxyAllocator
            );

            let runtime = AccountRuntimeState {
                session_uuid: uuid::Uuid::new_v4().to_string(),
                device_id: cli_cred.device_id.clone(),
                account_uuid: account.account_uuid.clone(),
                quota: None, // populated at runtime by Gateway handler
                model_rate_limits: HashMap::new(),
            };

            self.runtime_state.insert(id.clone(), runtime);
            self.clients.insert(id, cli);
        }

        let mut ids: Vec<String> = self.clients.iter().map(|e| e.key().clone()).collect();
        ids.sort();
        *self.ordered_ids.write().unwrap() = ids;

        Ok(self.clients.len())
    }

    /// Legacy: load from proxy-tokens/ directory (snake_case JSON).
    /// Kept for backward compatibility with E2E tests.
    pub fn load_tokens(&self, dir: &Path) -> Result<usize, String> {
        if !dir.exists() {
            return Ok(0);
        }

        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("Failed to read proxy-tokens dir: {}", e))?;

        self.clients.clear();
        self.runtime_state.clear();
        self.temp_unschedulable.clear();
        self.session_map.clear();

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.extension().is_some_and(|ext| ext == "json") {
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("Failed to read {}: {}", path.display(), e);
                    continue;
                }
            };

            #[derive(serde::Deserialize)]
            #[allow(dead_code)]
            struct LegacyToken {
                account_id: String,
                #[serde(default)]
                email: String,
                access_token: String,
                #[serde(default = "default_plan")]
                plan: String,
                #[serde(default)]
                refresh_token: Option<String>,
                #[serde(default)]
                expires_at: Option<i64>,
                #[serde(default)]
                account_uuid: Option<String>,
                #[serde(default)]
                upstream_proxy: Option<String>,
                #[serde(default)]
                device_id: Option<String>,
            }

            match serde_json::from_str::<LegacyToken>(&content) {
                Ok(lt) => {
                    let id = lt.account_id.clone();
                    let device_id = lt.device_id.unwrap_or_default();
                    if !Self::is_valid_device_id(&device_id) {
                        tracing::warn!("Skipping legacy token {} (invalid device_id: {})", id, &device_id);
                        continue;
                    }
                    let account_uuid = lt.account_uuid.unwrap_or_default();
                    if !Self::is_valid_account_uuid(&account_uuid) {
                        tracing::warn!("Skipping legacy token {} (invalid account_uuid: {})", id, &account_uuid);
                        continue;
                    }
                    let cli = CliClient::new(
                        self.boring_client.clone(),
                        id.clone(),
                        lt.access_token,
                        lt.refresh_token.unwrap_or_default(),
                        lt.expires_at.unwrap_or(0),
                        lt.upstream_proxy,
                    );
                    let runtime = AccountRuntimeState {
                        session_uuid: uuid::Uuid::new_v4().to_string(),
                        device_id,
                        account_uuid,
                        quota: None,
                        model_rate_limits: HashMap::new(),
                    };
                    self.runtime_state.insert(id.clone(), runtime);
                    self.clients.insert(id, cli);
                }
                Err(e) => {
                    tracing::warn!("Failed to parse {}: {}", path.display(), e);
                    continue;
                }
            }
        }

        let mut ids: Vec<String> = self.clients.iter().map(|e| e.key().clone()).collect();
        ids.sort();
        *self.ordered_ids.write().unwrap() = ids;

        Ok(self.clients.len())
    }

    /// Add a single account to the live pool (hot-load without restart).
    /// Returns true if added, false if already exists or ineligible.
    pub fn add_client(&self, account: &Account) -> bool {
        if self.clients.contains_key(&account.account_id) {
            return false;
        }
        if account.disabled || account.user_disabled {
            return false;
        }
        let cli_cred = match &account.cli {
            Some(c) => c,
            None => return false,
        };

        let id = account.account_id.clone();
        if !Self::is_valid_device_id(&cli_cred.device_id) {
            tracing::warn!("Skipping account {} (invalid device_id: {})", id, &cli_cred.device_id);
            return false;
        }
        if !Self::is_valid_account_uuid(&account.account_uuid) {
            tracing::warn!("Skipping account {} (invalid account_uuid: {})", id, &account.account_uuid);
            return false;
        }
        let cli = CliClient::new(
            self.boring_client.clone(),
            id.clone(),
            cli_cred.access_token.clone(),
            cli_cred.refresh_token.clone(),
            cli_cred.expires_at,
            None, // proxy_url set by handler from ProxyAllocator
        );
        let runtime = AccountRuntimeState {
            session_uuid: uuid::Uuid::new_v4().to_string(),
            device_id: cli_cred.device_id.clone(),
            account_uuid: account.account_uuid.clone(),
            quota: None,
            model_rate_limits: HashMap::new(),
        };

        self.runtime_state.insert(id.clone(), runtime);
        self.clients.insert(id.clone(), cli);

        // Update ordered_ids (deduplicate before push)
        if let Ok(mut ids) = self.ordered_ids.write() {
            if !ids.contains(&id) {
                ids.push(id);
                ids.sort();
            }
        }

        tracing::info!("Hot-loaded account {} into pool", account.account_id);
        true
    }

    /// Select a client via round-robin.
    /// Filters: disabled (removed from pool), attempted, model rate-limited, watermark, temp_unschedulable.
    /// Returns CliClient clone with proxy_url=None (handler sets proxy).
    pub fn get_client(&self, attempted: &HashSet<String>, model: Option<&str>) -> Option<CliClient> {
        let ids = self.ordered_ids.read().unwrap();
        let now_ms = chrono::Utc::now().timestamp_millis();

        let available: Vec<&String> = ids
            .iter()
            .filter(|id| {
                if attempted.contains(*id) {
                    return false;
                }
                // Must exist in pool
                if self.clients.get(*id).is_none() {
                    return false;
                }
                // TempUnschedule check
                if let Some(entry) = self.temp_unschedulable.get(*id) {
                    if now_ms < entry.0 {
                        return false; // still temp-unscheduled
                    }
                }
                // Model rate-limit check
                if let Some(m) = model {
                    if let Some(state) = self.runtime_state.get(*id) {
                        if let Some(until) = state.model_rate_limits.get(m) {
                            if Instant::now() < *until {
                                return false;
                            }
                        }
                    }
                }
                // Watermark check
                if let Some(state) = self.runtime_state.get(*id) {
                    if let Some(ref q) = state.quota {
                        let max_util = self.max_utilization(q);
                        if max_util >= self.safety_watermark {
                            return false;
                        }
                    }
                }
                // Token expiry check: skip accounts with expiring tokens
                // (5 min buffer — tighter than AccountMonitor's 90 min refresh window)
                if let Some(cli) = self.clients.get(*id) {
                    let token_buffer_ms: i64 = 5 * 60 * 1000;
                    if now_ms + token_buffer_ms >= cli.expires_at {
                        return false;
                    }
                }
                true
            })
            .collect();

        if available.is_empty() {
            return None;
        }

        let idx = self.current_index.fetch_add(1, Ordering::Relaxed) % available.len();
        self.clients.get(available[idx]).map(|c| c.value().clone())
    }

    /// Session-affinity wrapper: same session_id always routes to the same account.
    /// Falls back to round-robin if session has no mapping or mapped account is unavailable.
    pub fn get_client_with_session(
        &self,
        session_id: Option<&str>,
        attempted: &HashSet<String>,
        model: Option<&str>,
    ) -> Option<CliClient> {
        // 1. Try session affinity
        if let Some(sid) = session_id {
            if let Some(entry) = self.session_map.get(sid) {
                let aid = entry.value().0.clone();
                // Check if the account is still usable
                if let Some(client) = self.clients.get(&aid) {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let token_buffer_ms: i64 = 5 * 60 * 1000;
                    // Check each condition individually for diagnostics
                    let not_attempted = !attempted.contains(&aid);
                    let not_temp_unsched = !self.temp_unschedulable.get(&aid).map_or(false, |e| now_ms < e.0);
                    let model_ok = model.map_or(true, |m| self.is_model_available(&aid, m));
                    let quota_ok = self.runtime_state.get(&aid).map_or(true, |state| {
                        state.quota.as_ref().map_or(true, |q| self.max_utilization(q) < self.alert_watermark)
                    });
                    let token_ok = now_ms + token_buffer_ms < client.expires_at;

                    let is_available = not_attempted && not_temp_unsched && model_ok && quota_ok && token_ok;
                    if is_available {
                        // Update last-used timestamp
                        drop(entry);
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        self.session_map.insert(sid.to_string(), (aid.clone(), now_ms));
                        tracing::debug!("Session affinity hit: {} → {}", sid, aid);
                        return Some(client.value().clone());
                    }
                    // Log which condition failed
                    tracing::warn!(
                        "Session affinity unavailable: {} → {} (attempted={} temp_unsched={} model={} quota={} token={})",
                        sid, aid, !not_attempted, !not_temp_unsched, !model_ok, !quota_ok, !token_ok
                    );
                } else {
                    tracing::warn!("Session affinity miss: {} → {} not in clients DashMap", sid, aid);
                }
                // Account unavailable — remove stale mapping
                tracing::debug!("Session affinity miss (unavailable): {} → {}, reassigning", sid, aid);
                self.session_map.remove(sid);
            }
        }

        // 2. Fall back to round-robin
        let client = self.get_client(attempted, model)?;

        // 3. Record new mapping
        if let Some(sid) = session_id {
            let now_ms = chrono::Utc::now().timestamp_millis();
            self.session_map.insert(sid.to_string(), (client.account_id.clone(), now_ms));
            tracing::debug!("Session affinity new: {} → {}", sid, client.account_id);
        }

        Some(client)
    }

    /// Backward-compatible get_client without model filter.
    pub fn get_client_simple(&self, attempted: &HashSet<String>) -> Option<CliClient> {
        self.get_client(attempted, None)
    }

    /// Disable and remove a client from the pool.
    pub fn disable_client(&self, account_id: &str, reason: &str) {
        self.clients.remove(account_id);
        // Also remove from ordered_ids
        if let Ok(mut ids) = self.ordered_ids.write() {
            ids.retain(|id| id != account_id);
        }
        // Clean up session affinity pointing to this account
        self.session_map.retain(|_, (aid, _)| aid != account_id);
        tracing::warn!("Account {} disabled: {}", account_id, reason);
    }

    /// Evict session_map entries older than `max_age`.
    pub fn evict_stale_sessions(&self, max_age: Duration) {
        let cutoff = chrono::Utc::now().timestamp_millis() - max_age.as_millis() as i64;
        let before = self.session_map.len();
        self.session_map.retain(|_, (_, ts)| *ts > cutoff);
        let evicted = before - self.session_map.len();
        if evicted > 0 {
            tracing::info!("Session affinity: evicted {} stale entries (>{:?})", evicted, max_age);
        }
    }

    /// Temporarily exclude an account from selection.
    pub fn temp_unschedule(&self, account_id: &str, duration: Duration, reason: &str) {
        let until_ms = chrono::Utc::now().timestamp_millis() + duration.as_millis() as i64;
        self.temp_unschedulable.insert(
            account_id.to_string(),
            (until_ms, reason.to_string()),
        );
        tracing::info!(
            "Account {} temp-unscheduled for {}ms: {}",
            account_id, duration.as_millis(), reason
        );
    }

    /// Mark a model as rate-limited for a specific account.
    pub fn mark_model_rate_limited(&self, account_id: &str, model: &str, duration: Duration) {
        if let Some(mut state) = self.runtime_state.get_mut(account_id) {
            state.model_rate_limits.insert(model.to_string(), Instant::now() + duration);
        }
    }

    /// Check if a specific model is available for an account.
    pub fn is_model_available(&self, account_id: &str, model: &str) -> bool {
        if let Some(state) = self.runtime_state.get(account_id) {
            if let Some(until) = state.model_rate_limits.get(model) {
                return Instant::now() >= *until;
            }
        }
        true
    }

    pub fn get_runtime_state(&self, account_id: &str) -> Option<AccountRuntimeState> {
        self.runtime_state.get(account_id).map(|r| r.value().clone())
    }

    pub fn update_quota(&self, account_id: &str, quota: QuotaSnapshot) {
        if let Some(mut state) = self.runtime_state.get_mut(account_id) {
            state.quota = Some(quota);
        }
    }

    /// Update a client's tokens after refresh (write-back to pool).
    pub fn update_client_token(
        &self,
        account_id: &str,
        new_access_token: String,
        new_refresh_token: String,
        new_expires_at: i64,
    ) {
        if let Some(mut cli) = self.clients.get_mut(account_id) {
            cli.access_token = new_access_token;
            cli.refresh_token = new_refresh_token;
            cli.expires_at = new_expires_at;
        }
    }

    pub fn has_client(&self, account_id: &str) -> bool {
        self.clients.contains_key(account_id)
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    pub fn available_count(&self) -> usize {
        // All clients in pool are available (disabled ones are removed)
        self.clients.len()
    }

    pub fn cancel(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    pub fn shutdown(&self) {
        self.cancel_token.cancel();
    }

    fn max_utilization(&self, quota: &QuotaSnapshot) -> f64 {
        let five_h = quota.five_hour.as_ref().map(|w| w.utilization).unwrap_or(0.0);
        let seven_d = quota.seven_day.as_ref().map(|w| w.utilization).unwrap_or(0.0);
        five_h.max(seven_d)
    }
}

fn default_plan() -> String {
    "max".to_string()
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
                "claude_ultra_client_mgr_test_{}",
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

    fn write_file(dir: &Path, filename: &str, json: &str) {
        std::fs::write(dir.join(filename), json).unwrap();
    }

    fn make_test_uuid(seed: &str) -> String {
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, seed.as_bytes()).to_string()
    }

    fn make_v3_account(account_id: &str, email: &str) -> String {
        format!(
            r#"{{
                "accountId": "{}",
                "email": "{}",
                "accountUuid": "{}",
                "cli": {{
                    "accessToken": "sk-ant-oat01-{}",
                    "refreshToken": "sk-ant-ort01-{}",
                    "expiresAt": 2000003600000,
                    "deviceId": "{:064x}"
                }}
            }}"#,
            account_id, email, make_test_uuid(account_id), account_id, account_id,
            account_id.as_bytes().iter().fold(0u64, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u64))
        )
    }

    fn make_legacy_token(account_id: &str, email: &str) -> String {
        let far_future = chrono::Utc::now().timestamp_millis() + 24 * 60 * 60 * 1000;
        format!(
            r#"{{
                "account_id": "{}",
                "email": "{}",
                "access_token": "sk-test-token-{}",
                "plan": "max",
                "account_uuid": "{}",
                "device_id": "{:064x}",
                "expires_at": {}
            }}"#,
            account_id, email, account_id, make_test_uuid(account_id),
            account_id.as_bytes().iter().fold(0u64, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u64)),
            far_future
        )
    }

    fn make_manager() -> ClientManager {
        let bc = Arc::new(BoringClient::builder().build().unwrap());
        ClientManager::new(bc)
    }

    // ── Basic round-robin ────────────────────────────────────

    #[test]
    fn test_load_v3_and_get_client() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_v3_account("a1", "a1@t.com"));
        let cm = make_manager();
        let count = cm.load_clients(&dir.path).unwrap();
        assert_eq!(count, 1);
        let cli = cm.get_client_simple(&HashSet::new()).unwrap();
        assert_eq!(cli.account_id, "a1");
        assert_eq!(cli.access_token, "sk-ant-oat01-a1");
    }

    #[test]
    fn test_round_robin() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_v3_account("a1", "a1@t.com"));
        write_file(&dir.path, "a2.json", &make_v3_account("a2", "a2@t.com"));
        let cm = make_manager();
        cm.load_clients(&dir.path).unwrap();
        let empty = HashSet::new();
        let t1 = cm.get_client_simple(&empty).unwrap();
        let t2 = cm.get_client_simple(&empty).unwrap();
        assert!(t1.account_id == "a1" || t1.account_id == "a2");
        assert!(t2.account_id == "a1" || t2.account_id == "a2");
    }

    // ── disable_client ──────────────────────────────────────

    #[test]
    fn test_disable_client() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_v3_account("a1", "a1@t.com"));
        write_file(&dir.path, "a2.json", &make_v3_account("a2", "a2@t.com"));
        let cm = make_manager();
        cm.load_clients(&dir.path).unwrap();
        cm.disable_client("a1", "HTTP 401");
        assert_eq!(cm.available_count(), 1);
        let cli = cm.get_client_simple(&HashSet::new()).unwrap();
        assert_eq!(cli.account_id, "a2");
    }

    // ── model rate limit ────────────────────────────────────

    #[test]
    fn test_model_rate_limit_skips() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_v3_account("a1", "a1@t.com"));
        write_file(&dir.path, "a2.json", &make_v3_account("a2", "a2@t.com"));
        let cm = make_manager();
        cm.load_clients(&dir.path).unwrap();
        cm.mark_model_rate_limited("a1", "opus", Duration::from_secs(30));
        let cli = cm.get_client(&HashSet::new(), Some("opus")).unwrap();
        assert_eq!(cli.account_id, "a2");
    }

    // ── watermark ───────────────────────────────────────────

    #[test]
    fn test_watermark_at_safety_excluded() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_v3_account("a1", "a1@t.com"));
        let cm = make_manager();
        cm.load_clients(&dir.path).unwrap();
        cm.update_quota("a1", make_quota(0.8, 0.3));
        assert!(cm.get_client_simple(&HashSet::new()).is_none());
    }

    // ── temp_unschedule ─────────────────────────────────────

    #[test]
    fn test_temp_unschedule_skips() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_v3_account("a1", "a1@t.com"));
        write_file(&dir.path, "a2.json", &make_v3_account("a2", "a2@t.com"));
        let cm = make_manager();
        cm.load_clients(&dir.path).unwrap();
        cm.temp_unschedule("a1", Duration::from_secs(60), "429 rate limited");
        let cli = cm.get_client_simple(&HashSet::new()).unwrap();
        assert_eq!(cli.account_id, "a2");
    }

    #[test]
    fn test_temp_unschedule_expired_recovers() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_v3_account("a1", "a1@t.com"));
        let cm = make_manager();
        cm.load_clients(&dir.path).unwrap();
        // Set unschedule in the past
        let past_ms = chrono::Utc::now().timestamp_millis() - 1000;
        cm.temp_unschedulable.insert("a1".to_string(), (past_ms, "test".to_string()));
        assert!(cm.get_client_simple(&HashSet::new()).is_some());
    }

    // ── empty pool ──────────────────────────────────────────

    #[test]
    fn test_empty_pool_returns_none() {
        let cm = make_manager();
        assert!(cm.get_client_simple(&HashSet::new()).is_none());
    }

    // ── client_count / available_count ──────────────────────

    #[test]
    fn test_client_count() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_v3_account("a1", "a1@t.com"));
        write_file(&dir.path, "a2.json", &make_v3_account("a2", "a2@t.com"));
        let cm = make_manager();
        cm.load_clients(&dir.path).unwrap();
        assert_eq!(cm.client_count(), 2);
        assert_eq!(cm.available_count(), 2);
    }

    // ── update_client_token ─────────────────────────────────

    #[test]
    fn test_update_client_token() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_v3_account("a1", "a1@t.com"));
        let cm = make_manager();
        cm.load_clients(&dir.path).unwrap();
        let far_future = chrono::Utc::now().timestamp_millis() + 24 * 60 * 60 * 1000;
        cm.update_client_token("a1", "new-at".to_string(), "new-rt".to_string(), far_future);
        let cli = cm.get_client_simple(&HashSet::new()).unwrap();
        assert_eq!(cli.access_token, "new-at");
        assert_eq!(cli.refresh_token, "new-rt");
        assert_eq!(cli.expires_at, far_future);
    }

    // ── legacy load_tokens ──────────────────────────────────

    #[test]
    fn test_legacy_load_tokens() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_legacy_token("a1", "a1@t.com"));
        let cm = make_manager();
        assert_eq!(cm.load_tokens(&dir.path).unwrap(), 1);
        let cli = cm.get_client_simple(&HashSet::new()).unwrap();
        assert_eq!(cli.account_id, "a1");
    }

    // ── runtime state ───────────────────────────────────────

    #[test]
    fn test_runtime_state_generated() {
        let dir = TestDir::new();
        write_file(&dir.path, "a1.json", &make_v3_account("a1", "a1@t.com"));
        let cm = make_manager();
        cm.load_clients(&dir.path).unwrap();
        let state = cm.get_runtime_state("a1").unwrap();
        assert!(!state.session_uuid.is_empty());
        assert_eq!(state.device_id.len(), 64);
    }

    // ── Negative: invalid fingerprint fields are rejected ───

    #[test]
    fn test_empty_account_uuid_skipped() {
        let dir = TestDir::new();
        // Account with empty accountUuid
        let json = r#"{
            "accountId": "a1", "email": "a1@t.com", "accountUuid": "",
            "cli": { "accessToken": "tok", "refreshToken": "ref", "expiresAt": 2000003600000,
                     "deviceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
        }"#;
        write_file(&dir.path, "a1.json", json);
        let cm = make_manager();
        assert_eq!(cm.load_clients(&dir.path).unwrap(), 0, "empty account_uuid should be skipped");
    }

    #[test]
    fn test_invalid_account_uuid_skipped() {
        let dir = TestDir::new();
        // Account with non-UUID accountUuid
        let json = r#"{
            "accountId": "a1", "email": "a1@t.com", "accountUuid": "not-a-uuid",
            "cli": { "accessToken": "tok", "refreshToken": "ref", "expiresAt": 2000003600000,
                     "deviceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
        }"#;
        write_file(&dir.path, "a1.json", json);
        let cm = make_manager();
        assert_eq!(cm.load_clients(&dir.path).unwrap(), 0, "invalid account_uuid should be skipped");
    }

    #[test]
    fn test_empty_device_id_skipped() {
        let dir = TestDir::new();
        let json = format!(r#"{{
            "accountId": "a1", "email": "a1@t.com", "accountUuid": "{}",
            "cli": {{ "accessToken": "tok", "refreshToken": "ref", "expiresAt": 2000003600000 }}
        }}"#, make_test_uuid("a1"));
        write_file(&dir.path, "a1.json", &json);
        let cm = make_manager();
        assert_eq!(cm.load_clients(&dir.path).unwrap(), 0, "empty device_id should be skipped");
    }

    #[test]
    fn test_invalid_device_id_skipped() {
        let dir = TestDir::new();
        let json = format!(r#"{{
            "accountId": "a1", "email": "a1@t.com", "accountUuid": "{}",
            "cli": {{ "accessToken": "tok", "refreshToken": "ref", "expiresAt": 2000003600000,
                      "deviceId": "too-short" }}
        }}"#, make_test_uuid("a1"));
        write_file(&dir.path, "a1.json", &json);
        let cm = make_manager();
        assert_eq!(cm.load_clients(&dir.path).unwrap(), 0, "invalid device_id should be skipped");
    }

    // ── Helper ──────────────────────────────────────────────

    fn make_quota(five_h_util: f64, seven_d_util: f64) -> QuotaSnapshot {
        QuotaSnapshot {
            status: "allowed".to_string(),
            five_hour: Some(ClaudeAILimit {
                status: "allowed".to_string(),
                utilization: five_h_util,
                reset_at: 1000000000,
            }),
            seven_day: Some(ClaudeAILimit {
                status: "allowed".to_string(),
                utilization: seven_d_util,
                reset_at: 2000000000,
            }),
            overage: None,
            representative_claim: None,
            fallback: None,
            fallback_percentage: None,
            reset_at: None,
            organization_id: None,
            updated_at: 1000,
        }
    }
}
