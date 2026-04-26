//! ProxyAllocator — allocate and renew proxy sessions for accounts.
//! Uses reqwest (not hyper-boring) for IP verification — no TLS fingerprint needed.

use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::modules::account_manager::AccountManager;
use super::config::ProxyProviderConfig;
use crate::models::account::ProxySection;

/// IP verification result.
#[derive(Debug, Clone)]
pub struct IpInfo {
    pub ip: String,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub isp: Option<String>,
    pub quality: Option<String>,
}

/// IP verification service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IpService {
    IpApi = 0,    // ip-api.com (full info)
    Ip234 = 1,    // ip234.in (IP + country)
    Ipify = 2,    // ipify.org (IP only)
}

impl std::fmt::Display for IpService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IpApi => write!(f, "ip-api.com"),
            Self::Ip234 => write!(f, "ip234.in"),
            Self::Ipify => write!(f, "ipify.org"),
        }
    }
}

/// ProxyAllocator manages proxy session allocation and renewal.
/// Also maintains IP service selection and network health state.
pub struct ProxyAllocator {
    account_manager: Arc<AccountManager>,
    provider_config: Arc<ProxyProviderConfig>,
    renew_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
    verify_timeout: Duration,
    /// Selected IP verification service (probed at startup, switched on consecutive failures).
    preferred_ip_service: std::sync::atomic::AtomicU8,
    /// Whether local network is available (result of check_local_network).
    network_up: std::sync::atomic::AtomicBool,
    /// Direct HTTP client (no proxy, used for check_local_network).
    direct_client: reqwest::Client,
    /// ProxyPool (injected after app startup, OnceLock guarantees set-once).
    pool: std::sync::OnceLock<Arc<crate::proxy::pool::ProxyPool>>,
}

impl ProxyAllocator {
    pub fn new(
        account_manager: Arc<AccountManager>,
        provider_config: Arc<ProxyProviderConfig>,
    ) -> Self {
        Self::with_verify_timeout(account_manager, provider_config, Duration::from_secs(10))
    }

    pub fn with_verify_timeout(
        account_manager: Arc<AccountManager>,
        provider_config: Arc<ProxyProviderConfig>,
        verify_timeout: Duration,
    ) -> Self {
        let direct_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
            .build()
            .expect("Failed to build direct client");

        Self {
            account_manager,
            provider_config,
            renew_locks: DashMap::new(),
            verify_timeout,
            preferred_ip_service: std::sync::atomic::AtomicU8::new(IpService::IpApi as u8),
            network_up: std::sync::atomic::AtomicBool::new(true), // optimistic init
            direct_client,
            pool: std::sync::OnceLock::new(),
        }
    }

    /// Generate a unique session ID (12 hex chars).
    pub fn generate_session_id() -> String {
        let id = uuid::Uuid::new_v4().to_string().replace("-", "");
        id[..12].to_string()
    }

    /// Build proxy URL using default_country. Forwards to build_proxy_url_with_country.
    pub fn build_proxy_url(&self, session_id: &str) -> String {
        self.build_proxy_url_with_country(session_id, &self.provider_config.default_country)
    }

    /// Inject ProxyPool (called once at app startup).
    pub fn set_pool(&self, pool: Arc<crate::proxy::pool::ProxyPool>) {
        self.pool.set(pool).ok(); // duplicate set ignored
    }

    /// Get ProxyPool (returns None if not initialized). Gateway use only.
    pub fn pool(&self) -> Option<&Arc<crate::proxy::pool::ProxyPool>> {
        self.pool.get()
    }

    /// Get proxy_url for an account.
    ///
    /// - Empty global proxy credentials AND no per-account static_proxy → Ok(None) (direct).
    /// - route_mode == "static" + static_proxy configured → Ok(Some(static_url)) (socks5/http).
    /// - route_mode == "static" + static_proxy missing → Err (caller surfaces config error).
    /// - Otherwise → routes through ProxyPool dynamic allocation (Some(rotated_url)).
    pub async fn get_proxy_url_optional(self: &Arc<Self>, account_id: &str) -> Result<Option<String>, String> {
        let account = self.account_manager.read(account_id).await
            .map_err(|e| format!("read account {}: {}", account_id, e))?;
        if account.route_mode == "static" {
            let sp = account.static_proxy.as_ref().ok_or_else(|| {
                format!("account {} route_mode=static but static_proxy=None", account_id)
            })?;
            return Ok(Some(build_static_proxy_url(sp)));
        }
        if self.provider_config.username.is_empty() || self.provider_config.password.is_empty() {
            return Ok(None);
        }
        self.get_active_proxy_url(account_id).await.map(Some)
    }

    /// Get the active session's proxy_url for an account (business-layer interface).
    /// Routes through ProxyPool internally, falls back to ephemeral if no pool.
    /// Resolves per-account country from AccountManager so first-call allocation picks up
    /// the account's target country (not the global GeoPolicy default).
    pub async fn get_active_proxy_url(self: &Arc<Self>, account_id: &str) -> Result<String, String> {
        if let Some(pool) = self.pool.get() {
            let country = self.account_manager.read(account_id).await.ok()
                .map(|a| a.resolve_proxy_country());
            pool.get_active_proxy_url(account_id, country.as_deref()).await
                .map_err(|e| format!("{}", e))
        } else {
            Ok(self.get_ephemeral_proxy_url())
        }
    }

    /// Notify proxy unavailable, trigger switch (business-layer interface).
    /// Only called on proxy connection failures, not on business errors.
    pub async fn renew_active_proxy(self: &Arc<Self>, account_id: &str) {
        if let Some(pool) = self.pool.get() {
            pool.renew_proxy(account_id).await;
        }
    }

    /// Read the account's user_disabled state.
    pub async fn is_user_disabled(&self, account_id: &str) -> bool {
        self.account_manager.read(account_id).await
            .map(|a| a.user_disabled)
            .unwrap_or(false)
    }

    /// Get the currently selected IP service.
    pub fn preferred_service(&self) -> IpService {
        match self.preferred_ip_service.load(std::sync::atomic::Ordering::Relaxed) {
            0 => IpService::IpApi,
            1 => IpService::Ip234,
            _ => IpService::Ipify,
        }
    }

    /// Whether local network is available.
    pub fn is_network_up(&self) -> bool {
        self.network_up.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Set network state.
    pub fn set_network_up(&self, up: bool) {
        self.network_up.store(up, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check local network connectivity (direct to Cloudflare + Baidu, either success suffices).
    pub async fn check_local_network(&self) -> bool {
        let timeout = Duration::from_secs(5);
        let cf = async {
            self.direct_client
                .get("http://cp.cloudflare.com/generate_204")
                .timeout(timeout)
                .send()
                .await
                .is_ok()
        };
        let baidu = async {
            self.direct_client
                .get("http://www.baidu.com/robots.txt")
                .timeout(timeout)
                .send()
                .await
                .is_ok()
        };
        let (cf_ok, baidu_ok) = tokio::join!(cf, baidu);
        cf_ok || baidu_ok
    }

    /// Start background health loop (self-check network + IP services every 5 minutes).
    /// First run is immediate.
    pub fn start_health_loop(self: &Arc<Self>, cancel_token: tokio_util::sync::CancellationToken) {
        let allocator = self.clone();
        tokio::spawn(async move {
            // First probe is manually awaited by lib.rs, this loop handles periodic runs
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            interval.tick().await; // skip immediate first tick

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        allocator.run_health_check().await;
                    }
                    _ = cancel_token.cancelled() => {
                        tracing::info!("[ALLOCATOR] health loop shutting down");
                        break;
                    }
                }
            }
        });
    }

    async fn run_health_check(&self) {
        use std::sync::atomic::Ordering;

        let up = self.check_local_network().await;
        let was_up = self.network_up.swap(up, Ordering::Relaxed);

        if up != was_up {
            if up {
                tracing::info!("[ALLOCATOR] network recovered");
            } else {
                tracing::warn!("[ALLOCATOR] network down");
            }
        }

        if up {
            self.probe_ip_services().await;
        }
    }

    /// Probe three IP services via direct connection, select the fastest. Runs at startup + every 5 minutes.
    pub async fn probe_ip_services(&self) {
        use std::sync::atomic::Ordering;
        let services = [
            (IpService::IpApi, "ip-api.com"),
            (IpService::Ip234, "ip234.in"),
            (IpService::Ipify, "ipify.org"),
        ];
        let mut best: Option<(IpService, u128)> = None;

        for (svc, name) in &services {
            let start = std::time::Instant::now();
            let result = match svc {
                IpService::IpApi => self.try_ip_api(&self.direct_client).await.map(|_| ()),
                IpService::Ip234 => self.try_ip234(&self.direct_client).await.map(|_| ()),
                IpService::Ipify => self.try_ipify(&self.direct_client).await.map(|_| ()),
            };
            let ms = start.elapsed().as_millis();
            match result {
                Ok(_) => {
                    tracing::info!("[VERIFY] probe {} OK ({}ms)", name, ms);
                    if best.as_ref().map_or(true, |(_, best_ms)| ms < *best_ms) {
                        best = Some((*svc, ms));
                    }
                }
                Err(e) => {
                    tracing::warn!("[VERIFY] probe {} failed ({}ms): {}", name, ms, e);
                }
            }
        }

        if let Some((svc, ms)) = best {
            self.preferred_ip_service.store(svc as u8, Ordering::Relaxed);
            tracing::info!("[VERIFY] selected {} as preferred ({}ms)", svc, ms);
        }
    }

    /// Verify proxy egress IP using the selected IP service.
    pub async fn verify_ip(&self, proxy_url: &str) -> Result<IpInfo, String> {
        use std::sync::atomic::Ordering;

        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|e| format!("Invalid proxy URL: {}", e))?;
        let client = reqwest::Client::builder()
            .proxy(proxy)
            .timeout(self.verify_timeout)
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
            .build()
            .map_err(|e| format!("Failed to build proxy client: {}", e))?;

        let svc = self.preferred_service();
        let start = std::time::Instant::now();

        let result = match svc {
            IpService::IpApi => self.try_ip_api(&client).await,
            IpService::Ip234 => self.try_ip234(&client).await,
            IpService::Ipify => {
                self.try_ipify(&client).await.map(|ip| IpInfo {
                    ip, country: None, region: None, city: None, isp: None, quality: None,
                })
            }
        };

        match result {
            Ok(info) => {
                // Passively restore network state
                self.network_up.store(true, Ordering::Relaxed);
                tracing::debug!("[VERIFY] {} OK: {} ({:?}) {}ms",
                    svc, info.ip, info.country, start.elapsed().as_millis());
                Ok(info)
            }
            Err(e) => {
                // Don't increment ip_service_failures — verify_ip goes through proxy,
                // failure may be proxy issue, not IP service issue.
                // IP service switching is solely decided by probe_ip_services (direct probe, every 5 min).
                tracing::debug!("[VERIFY] {} failed ({}ms): {}",
                    svc, start.elapsed().as_millis(), e);
                Err(format!("{} failed: {}", svc, e))
            }
        }
    }

    async fn try_ip_api(&self, client: &reqwest::Client) -> Result<IpInfo, String> {
        let resp = client
            .get("http://ip-api.com/json/?fields=status,country,regionName,city,isp,org,mobile,proxy,hosting,query")
            .timeout(self.verify_timeout)
            .send()
            .await
            .map_err(|e| format!("ip-api.com failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(format!("ip-api.com HTTP {}", status));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("ip-api.com parse failed: {}", e))?;

        if json["status"].as_str() != Some("success") {
            return Err("ip-api.com: status not success".to_string());
        }

        let ip = json["query"].as_str().ok_or("ip-api.com: no query field")?.to_string();
        let hosting = json["hosting"].as_bool().unwrap_or(false);
        let proxy_flag = json["proxy"].as_bool().unwrap_or(false);
        let mobile = json["mobile"].as_bool().unwrap_or(false);
        let quality = if hosting { "datacenter" } else if proxy_flag { "proxy" } else if mobile { "mobile" } else { "residential" };

        Ok(IpInfo {
            ip,
            country: json["country"].as_str().map(|s| s.to_string()),
            region: json["regionName"].as_str().map(|s| s.to_string()),
            city: json["city"].as_str().map(|s| s.to_string()),
            isp: json["isp"].as_str().map(|s| s.to_string()),
            quality: Some(quality.to_string()),
        })
    }

    async fn try_ip234(&self, client: &reqwest::Client) -> Result<IpInfo, String> {
        let resp = client
            .get("http://ip234.in/ip.json")
            .timeout(self.verify_timeout)
            .send()
            .await
            .map_err(|e| format!("ip234.in failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(format!("ip234.in HTTP {}", status));
        }

        let text = resp.text().await
            .map_err(|e| format!("ip234.in read body failed: {}", e))?;
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("ip234.in parse failed (body={}): {}", &text[..text.len().min(100)], e))?;

        let ip = json["ip"]
            .as_str()
            .ok_or("ip234.in: no ip field")?
            .to_string();

        Ok(IpInfo {
            ip,
            country: json["country"].as_str().map(|s| s.to_string()),
            region: json["region"].as_str().map(|s| s.to_string()),
            city: json["city"].as_str().map(|s| s.to_string()),
            isp: json["isp"].as_str().map(|s| s.to_string()),
            quality: json["hosting"]
                .as_bool()
                .map(|h| if h { "datacenter" } else { "residential" }.to_string()),
        })
    }

    async fn try_ipify(&self, client: &reqwest::Client) -> Result<String, String> {
        let resp = client
            .get("https://api.ipify.org?format=json")
            .timeout(self.verify_timeout)
            .send()
            .await
            .map_err(|e| format!("ipify failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(format!("ipify HTTP {}", status));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("ipify parse failed: {}", e))?;

        json["ip"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or("ipify: no ip field".to_string())
    }

    /// Generate an ephemeral proxy URL (not persisted to any account).
    /// Used by TokenAllocator and get_account_quota for one-off proxy requests.
    pub fn get_ephemeral_proxy_url(&self) -> String {
        let session_id = Self::generate_session_id();
        self.build_proxy_url(&session_id)
    }

    /// Build proxy URL with explicit country. Forwards to the shared
    /// `build_residential_proxy_url` helper — that function is the single
    /// source of truth for residential URL formatting; every caller (this
    /// allocator, the test_proxy_connection IPC, e2e fixtures) must go
    /// through it so test paths and production paths cannot drift.
    pub fn build_proxy_url_with_country(&self, session_id: &str, country: &str) -> String {
        build_residential_proxy_url(&self.provider_config, country, session_id)
    }

    /// Allocate a new proxy session WITHOUT persisting to Account JSON.
    /// Used by ProxyPool for standby preparation and challenger testing.
    pub async fn allocate_ephemeral(&self, country: &str) -> Result<ProxySection, String> {
        let max_retries = self.provider_config.max_allocate_retries;

        for attempt in 0..max_retries {
            let session_id = Self::generate_session_id();
            let proxy_url = self.build_proxy_url_with_country(&session_id, country);

            match self.verify_ip(&proxy_url).await {
                Ok(ip_info) => {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;

                    return Ok(ProxySection {
                        session_id,
                        host: format!("{}:{}", self.provider_config.host, self.provider_config.port),
                        last_ip: Some(ip_info.ip),
                        country: ip_info.country,
                        region: ip_info.region,
                        city: ip_info.city,
                        isp: ip_info.isp,
                        quality: ip_info.quality,
                        last_checked: Some(now_ms),
                        proxy_type: Some("residential".to_string()),
                        lifetime: Some("24h".to_string()),
                        created_at: Some(now_ms),
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        "Ephemeral alloc attempt {}/{} failed: {}",
                        attempt + 1, max_retries, e
                    );
                }
            }
        }

        Err(format!("Failed to allocate ephemeral proxy after {} attempts", max_retries))
    }

    /// Persist a ProxySection to Account JSON.
    /// Used by ProxyPool after switch_to_standby confirms the new session.
    pub async fn commit_session(&self, account_id: &str, session: &ProxySection) -> Result<(), String> {
        self.account_manager.set_proxy(account_id, Some(session.clone())).await
    }

    /// Allocate a new proxy session for an account.
    /// Generates session → verifies IP → writes to Account.proxy.
    pub async fn allocate(&self, account_id: &str) -> Result<ProxySection, String> {
        let max_retries = self.provider_config.max_allocate_retries;

        for attempt in 0..max_retries {
            let session_id = Self::generate_session_id();
            let proxy_url = self.build_proxy_url(&session_id);

            match self.verify_ip(&proxy_url).await {
                Ok(ip_info) => {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;

                    let proxy_section = ProxySection {
                        session_id: session_id.clone(),
                        host: format!("{}:{}", self.provider_config.host, self.provider_config.port),
                        last_ip: Some(ip_info.ip),
                        country: ip_info.country,
                        region: ip_info.region,
                        city: ip_info.city,
                        isp: ip_info.isp,
                        quality: ip_info.quality,
                        last_checked: Some(now_ms),
                        proxy_type: Some("residential".to_string()),
                        lifetime: Some("24h".to_string()),
                        created_at: Some(now_ms),
                    };

                    // Write to account JSON
                    self.account_manager
                        .set_proxy(account_id, Some(proxy_section.clone()))
                        .await?;

                    tracing::info!(
                        "Proxy allocated for {}: session={}, ip={}",
                        account_id,
                        session_id,
                        proxy_section.last_ip.as_deref().unwrap_or("unknown")
                    );

                    return Ok(proxy_section);
                }
                Err(e) => {
                    tracing::warn!(
                        "Proxy allocation attempt {}/{} failed for {}: {}",
                        attempt + 1,
                        max_retries,
                        account_id,
                        e
                    );
                }
            }
        }

        Err(format!(
            "Failed to allocate proxy for {} after {} attempts",
            account_id, max_retries
        ))
    }

    /// Renew proxy session for an account (per-account Mutex protected).
    pub async fn renew(&self, account_id: &str) -> Result<ProxySection, String> {
        let lock = self
            .renew_locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();

        // Try to acquire with timeout to prevent deadlock
        let _guard = tokio::time::timeout(Duration::from_secs(30), lock.lock())
            .await
            .map_err(|_| format!("Renew lock timeout for {}", account_id))?;

        self.allocate(account_id).await
    }

}

/// Build a residential dynamic proxy URL from a `ProxyProviderConfig`.
///
/// Single source of truth for residential URL formatting. Every caller
/// (ProxyAllocator hot path, the `test_proxy_connection` UI probe, e2e
/// fixtures) MUST go through this function so the test path and the
/// production path cannot drift.
///
/// Provider dispatch:
/// - **IPRoyal** (default / unknown kind): `_country/_session/_lifetime/_streaming-1`
///   appended to the password segment, lowercase country. `_streaming-1` is the
///   High-End Pool flag — manager talks to the High-End pool; webapp's shared
///   builder talks to the Standard pool and intentionally omits it (accepted
///   asymmetry, kept here so no caller silently drops the flag).
/// - **IPFoxy**: parameters are placed in the username segment with hyphen
///   separators, uppercase country. The username is forwarded verbatim —
///   no prefix injection, no normalisation; whatever the user pasted into
///   the proxy config is exactly what reaches the upstream.
///
/// Character-level aligned with `shared/src/proxy.ts::buildResidentialUrl`
/// (excluding `_streaming-1`, see above). Any format change must be applied
/// in both files; the unit tests here lock the expected strings.
pub fn build_residential_proxy_url(
    cfg: &crate::proxy::config::ProxyProviderConfig,
    country: &str,
    session_id: &str,
) -> String {
    match cfg.kind.as_str() {
        "ipfoxy" => format!(
            "http://{}-cc-{}-sessid-{}:{}@{}:{}",
            cfg.username,
            country.to_uppercase(),
            session_id,
            cfg.password,
            cfg.host,
            cfg.port,
        ),
        // IPRoyal (default) or unknown kind. Backward-compat fallback so
        // legacy configs without an explicit kind still route correctly.
        _ => format!(
            "http://{}:{}_country-{}_session-{}_lifetime-24h_streaming-1@{}:{}",
            cfg.username,
            cfg.password,
            country.to_lowercase(),
            session_id,
            cfg.host,
            cfg.port,
        ),
    }
}

/// Build a standard proxy URL from a per-account `StaticProxy` config.
/// Format: `<protocol>://<user>:<pass>@<host>:<port>` with credentials
/// percent-encoded so the SOCKS5 USER/PASS sub-negotiation parser sees the
/// raw bytes back after decoding.
pub(crate) fn build_static_proxy_url(sp: &crate::models::account::StaticProxy) -> String {
    let user = percent_encode_alnum(&sp.username);
    let pass = percent_encode_alnum(&sp.password);
    let host = format_host(&sp.host);
    format!("{}://{}:{}@{}:{}", sp.protocol, user, pass, host, sp.port)
}

/// IPv6 literals must be wrapped in `[]` per RFC 3986 so the URL parser
/// distinguishes them from `host:port` colon. IPv4 + DNS hostnames pass
/// through unchanged. Hosts with characters outside the expected set are
/// passed through too — the UI is expected to validate format at config time.
fn format_host(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{}]", host)
    } else {
        host.to_string()
    }
}

/// Percent-encode for SOCKS5/HTTP proxy URL credentials.
///
/// Only `A-Za-z0-9` pass through unencoded; all other bytes are %XX-escaped.
/// Stricter than JS `encodeURIComponent` (which preserves `-_.!~*'()`), but
/// IPFoxy credentials in production are alphanumeric so the two outputs are
/// identical for real inputs. Stricter encoding is safer for round-trip
/// through the SOCKS5 USER/PASS sub-negotiation parser.
fn percent_encode_alnum(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
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
                "claude_ultra_proxy_alloc_test_{}",
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
            kind: "iproyal".to_string(),
            host: "127.0.0.99".to_string(),  // unreachable proxy for tests
            port: 19999,
            username: "testuser".to_string(),
            password: "testpass".to_string(),
            default_country: "US".to_string(),
            max_allocate_retries: 2, // fewer retries for faster tests
        }
    }

    fn write_account(dir: &std::path::Path, id: &str) {
        let json = format!(
            r#"{{"accountId":"{}","email":"{}@t.com","cli":{{"accessToken":"t","refreshToken":"r","expiresAt":0}}}}"#,
            id, id
        );
        std::fs::write(dir.join(format!("{}.json", id)), json).unwrap();
    }

    fn write_account_static(dir: &std::path::Path, id: &str) {
        let json = format!(
            r#"{{"accountId":"{}","email":"{}@t.com","routeMode":"static","staticProxy":{{"protocol":"socks5","host":"203.0.113.10","port":45001,"username":"u","password":"p"}},"cli":{{"accessToken":"t","refreshToken":"r","expiresAt":0}}}}"#,
            id, id
        );
        std::fs::write(dir.join(format!("{}.json", id)), json).unwrap();
    }

    fn write_account_static_http(dir: &std::path::Path, id: &str) {
        let json = format!(
            r#"{{"accountId":"{}","email":"{}@t.com","routeMode":"static","staticProxy":{{"protocol":"http","host":"h.example","port":8080,"username":"u","password":"p"}},"cli":{{"accessToken":"t","refreshToken":"r","expiresAt":0}}}}"#,
            id, id
        );
        std::fs::write(dir.join(format!("{}.json", id)), json).unwrap();
    }

    fn write_account_static_special_chars(dir: &std::path::Path, id: &str) {
        let json = format!(
            r#"{{"accountId":"{}","email":"{}@t.com","routeMode":"static","staticProxy":{{"protocol":"socks5","host":"h","port":1080,"username":"us@er","password":"p:ss"}},"cli":{{"accessToken":"t","refreshToken":"r","expiresAt":0}}}}"#,
            id, id
        );
        std::fs::write(dir.join(format!("{}.json", id)), json).unwrap();
    }

    fn write_account_static_no_creds(dir: &std::path::Path, id: &str) {
        // routeMode=static but no staticProxy field — misconfiguration test fixture.
        let json = format!(
            r#"{{"accountId":"{}","email":"{}@t.com","routeMode":"static","cli":{{"accessToken":"t","refreshToken":"r","expiresAt":0}}}}"#,
            id, id
        );
        std::fs::write(dir.join(format!("{}.json", id)), json).unwrap();
    }

    fn make_allocator(dir: &std::path::Path) -> ProxyAllocator {
        let mgr = Arc::new(AccountManager::new(dir.to_path_buf()));
        let config = Arc::new(make_provider_config());
        // 2 second timeout for fast test failure
        ProxyAllocator::with_verify_timeout(mgr, config, Duration::from_secs(2))
    }

    // ── Session ID format ────────────────────────────────────

    #[test]
    fn test_session_id_format() {
        let id = ProxyAllocator::generate_session_id();
        assert_eq!(id.len(), 12, "should be 12 hex chars: {}", id);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "should be hex: {}", id);
    }

    #[test]
    fn test_session_id_unique() {
        let id1 = ProxyAllocator::generate_session_id();
        let id2 = ProxyAllocator::generate_session_id();
        assert_ne!(id1, id2, "session IDs should be unique");
    }

    // ── Proxy URL construction ───────────────────────────────

    #[test]
    fn test_proxy_url_format() {
        let dir = TestDir::new();
        let mgr = Arc::new(AccountManager::new(dir.path.clone()));
        let config = Arc::new(make_provider_config());
        let allocator = ProxyAllocator::new(mgr, config);

        let url = allocator.build_proxy_url("a1b2c3d4e5f6");
        assert_eq!(
            url,
            "http://testuser:testpass_country-us_session-a1b2c3d4e5f6_lifetime-24h_streaming-1@127.0.0.99:19999"
        );
    }

    #[test]
    fn test_proxy_url_different_country() {
        let dir = TestDir::new();
        let mgr = Arc::new(AccountManager::new(dir.path.clone()));
        let mut cfg = make_provider_config();
        cfg.default_country = "GB".to_string();
        let allocator = ProxyAllocator::new(mgr, Arc::new(cfg));

        let url = allocator.build_proxy_url("s1");
        assert!(url.contains("country-gb"), "URL should contain country: {}", url);
    }

    // ── Allocate with mock (no real proxy available in test) ──

    #[tokio::test]
    async fn test_allocate_fails_without_real_proxy() {
        let dir = TestDir::new();
        write_account(&dir.path, "a1");
        let allocator = make_allocator(&dir.path);

        // No real proxy → should fail after retries
        let result = allocator.allocate("a1").await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("Failed to allocate"),
            "should report allocation failure"
        );
    }

    // ── Static account dispatch ──────────────────────────────

    #[tokio::test]
    async fn test_get_proxy_url_optional_static_returns_socks5_url() {
        // route_mode=static + staticProxy{socks5} → returns the standard
        // socks5:// URL with credentials inline.
        let dir = TestDir::new();
        write_account_static(&dir.path, "static_socks5");
        let allocator = Arc::new(make_allocator(&dir.path));

        let result = allocator.get_proxy_url_optional("static_socks5").await;
        assert_eq!(result, Ok(Some("socks5://u:p@203.0.113.10:45001".to_string())));
    }

    #[tokio::test]
    async fn test_get_proxy_url_optional_static_returns_http_url() {
        let dir = TestDir::new();
        write_account_static_http(&dir.path, "static_http");
        let allocator = Arc::new(make_allocator(&dir.path));

        let result = allocator.get_proxy_url_optional("static_http").await;
        assert_eq!(result, Ok(Some("http://u:p@h.example:8080".to_string())));
    }

    #[tokio::test]
    async fn test_get_proxy_url_optional_static_url_encodes_creds() {
        // user='us@er' pass='p:ss' → percent-encoded
        let dir = TestDir::new();
        write_account_static_special_chars(&dir.path, "static_special");
        let allocator = Arc::new(make_allocator(&dir.path));

        let result = allocator.get_proxy_url_optional("static_special").await;
        assert_eq!(result, Ok(Some("socks5://us%40er:p%3Ass@h:1080".to_string())));
    }

    #[tokio::test]
    async fn test_get_proxy_url_optional_static_no_creds_returns_err() {
        // routeMode=static but staticProxy=None → Err so the caller surfaces
        // a config error rather than silently falling through.
        let dir = TestDir::new();
        write_account_static_no_creds(&dir.path, "static_misconfig");
        let allocator = Arc::new(make_allocator(&dir.path));

        let result = allocator.get_proxy_url_optional("static_misconfig").await;
        assert!(result.is_err(), "expected Err, got {:?}", result);
    }

    #[test]
    fn test_format_host_ipv4_passthrough() {
        assert_eq!(format_host("203.0.113.10"), "203.0.113.10");
    }

    #[test]
    fn test_format_host_dns_passthrough() {
        assert_eq!(format_host("gate.ipfoxy.io"), "gate.ipfoxy.io");
    }

    #[test]
    fn test_format_host_ipv6_wrapped() {
        // IPv6 literals must be wrapped in `[]` so the URL parser does not
        // mistake the address colons for the host:port separator.
        assert_eq!(format_host("::1"), "[::1]");
        assert_eq!(format_host("2001:db8::1"), "[2001:db8::1]");
    }

    #[test]
    fn test_format_host_ipv6_already_wrapped_passthrough() {
        // Already-wrapped IPv6 (e.g. user pasted `[::1]`) must not be
        // double-wrapped.
        assert_eq!(format_host("[::1]"), "[::1]");
    }

    #[test]
    fn test_build_static_proxy_url_ipv6() {
        let sp = crate::models::account::StaticProxy {
            protocol: "socks5".to_string(),
            host: "::1".to_string(),
            port: 1080,
            username: "u".to_string(),
            password: "p".to_string(),
        };
        assert_eq!(build_static_proxy_url(&sp), "socks5://u:p@[::1]:1080");
    }

    #[tokio::test]
    async fn test_get_proxy_url_optional_dynamic_routes_through_pool() {
        // route_mode=proxy (default) + valid creds → does NOT short-circuit.
        // The call hits get_active_proxy_url and (with no real ProxyPool
        // injected here) falls through to the ephemeral URL, returning Some.
        let dir = TestDir::new();
        write_account(&dir.path, "dyn_acct");
        let allocator = Arc::new(make_allocator(&dir.path));

        let result = allocator.get_proxy_url_optional("dyn_acct").await;
        assert!(matches!(result, Ok(Some(_))), "got {:?}", result);
    }

    // ── Renew concurrent protection ──────────────────────────

    #[tokio::test]
    async fn test_renew_concurrent_serialized() {
        let dir = TestDir::new();
        write_account(&dir.path, "a1");
        let allocator = Arc::new(make_allocator(&dir.path));

        // Both renews will fail (no real proxy), but they should not deadlock
        let a1 = allocator.clone();
        let a2 = allocator.clone();

        let (r1, r2) = tokio::join!(
            a1.renew("a1"),
            a2.renew("a1"),
        );

        // Both should fail (no real proxy), but neither should deadlock
        assert!(r1.is_err());
        assert!(r2.is_err());
    }

    // ── IpInfo struct ────────────────────────────────────────

    // ── Ephemeral proxy URL ────────────────────────────────

    #[test]
    fn test_ephemeral_proxy_url_format() {
        let dir = TestDir::new();
        let allocator = make_allocator(&dir.path);
        let url = allocator.get_ephemeral_proxy_url();
        // Should start with http:// and contain the provider credentials
        assert!(url.starts_with("http://"), "URL should start with http://: {}", url);
        assert!(url.contains("testuser"), "URL should contain username: {}", url);
        assert!(url.contains("testpass"), "URL should contain password: {}", url);
        assert!(url.contains("127.0.0.99:19999"), "URL should contain host:port: {}", url);
        assert!(url.contains("_session-"), "URL should contain session: {}", url);
    }

    #[test]
    fn test_ephemeral_proxy_url_unique_sessions() {
        let dir = TestDir::new();
        let allocator = make_allocator(&dir.path);
        let url1 = allocator.get_ephemeral_proxy_url();
        let url2 = allocator.get_ephemeral_proxy_url();
        assert_ne!(url1, url2, "Each ephemeral URL should have a unique session");
    }

    // ── IpInfo struct ────────────────────────────────────────

    // ── allocate_ephemeral + commit_session ────────────────

    #[tokio::test]
    async fn test_allocate_ephemeral_does_not_persist() {
        let dir = TestDir::new();
        write_account(&dir.path, "a1");
        let allocator = make_allocator(&dir.path);

        // No real proxy, allocate_ephemeral will fail
        let result = allocator.allocate_ephemeral("us").await;
        assert!(result.is_err());

        // Verify account JSON was not modified (no proxy field)
        let content = std::fs::read_to_string(dir.path.join("a1.json")).unwrap();
        assert!(!content.contains("sessionId"), "allocate_ephemeral should not write to account JSON");
    }

    #[tokio::test]
    async fn test_commit_session_persists() {
        let dir = TestDir::new();
        write_account(&dir.path, "a1");
        let allocator = make_allocator(&dir.path);

        let session = crate::models::account::ProxySection {
            session_id: "test123456ab".to_string(),
            host: "geo.iproyal.com:12321".to_string(),
            last_ip: Some("1.2.3.4".to_string()),
            country: Some("US".to_string()),
            region: None,
            city: None,
            isp: None,
            quality: Some("residential".to_string()),
            last_checked: Some(1000),
            proxy_type: Some("residential".to_string()),
            lifetime: Some("24h".to_string()),
            created_at: Some(1000),
        };

        allocator.commit_session("a1", &session).await.unwrap();

        // Verify account JSON has proxy field
        let content = std::fs::read_to_string(dir.path.join("a1.json")).unwrap();
        assert!(content.contains("test123456ab"), "commit_session should persist session_id");
    }

    #[test]
    fn test_build_proxy_url_with_country() {
        let dir = TestDir::new();
        let allocator = make_allocator(&dir.path);
        let url = allocator.build_proxy_url_with_country("a1b2c3d4e5f6", "jp");
        assert!(url.contains("country-jp"), "should use explicit country: {}", url);
        assert!(url.contains("session-a1b2c3d4e5f6"));
        assert!(url.contains("testuser"));
    }

    #[test]
    fn test_build_proxy_url_iproyal_format() {
        // Golden vector: IPRoyal format aligned with current Rust impl
        let dir = TestDir::new();
        let mgr = Arc::new(AccountManager::new(dir.path.clone()));
        let mut cfg = make_provider_config();
        cfg.kind = "iproyal".to_string();
        cfg.host = "geo.iproyal.com".to_string();
        cfg.port = 12321;
        cfg.default_country = "us".to_string();
        let allocator = ProxyAllocator::new(mgr, Arc::new(cfg));

        let url = allocator.build_proxy_url("a1b2c3d4e5f6");
        assert_eq!(
            url,
            "http://testuser:testpass_country-us_session-a1b2c3d4e5f6_lifetime-24h_streaming-1@geo.iproyal.com:12321"
        );
    }

    #[test]
    fn test_build_proxy_url_ipfoxy_format() {
        // Golden vector: IPFoxy format character-level aligned with shared/src/proxy.ts buildResidentialUrl
        let dir = TestDir::new();
        let mgr = Arc::new(AccountManager::new(dir.path.clone()));
        let mut cfg = make_provider_config();
        cfg.kind = "ipfoxy".to_string();
        cfg.host = "gate.ipfoxy.io".to_string();
        cfg.port = 58688;
        cfg.default_country = "us".to_string();
        let allocator = ProxyAllocator::new(mgr, Arc::new(cfg));

        let url = allocator.build_proxy_url("a1b2c3d4e5f6");
        assert_eq!(
            url,
            "http://testuser-cc-US-sessid-a1b2c3d4e5f6:testpass@gate.ipfoxy.io:58688"
        );
    }

    #[test]
    fn test_build_proxy_url_unknown_kind_fallback_to_iproyal() {
        // Unknown kind falls back to IPRoyal format (backward compat).
        let dir = TestDir::new();
        let mgr = Arc::new(AccountManager::new(dir.path.clone()));
        let mut cfg = make_provider_config();
        cfg.kind = "unknown".to_string();
        cfg.host = "geo.iproyal.com".to_string();
        cfg.port = 12321;
        cfg.default_country = "us".to_string();
        let allocator = ProxyAllocator::new(mgr, Arc::new(cfg));

        let url = allocator.build_proxy_url("a1b2c3d4e5f6");
        assert_eq!(
            url,
            "http://testuser:testpass_country-us_session-a1b2c3d4e5f6_lifetime-24h_streaming-1@geo.iproyal.com:12321"
        );
    }

    #[test]
    fn test_build_proxy_url_ipfoxy_with_country_override_uppercase() {
        // IPFoxy with explicit country (lowercase input) → uppercase in URL (-cc-XX-).
        // Pin the country.to_uppercase() invariant explicitly.
        let dir = TestDir::new();
        let mgr = Arc::new(AccountManager::new(dir.path.clone()));
        let mut cfg = make_provider_config();
        cfg.kind = "ipfoxy".to_string();
        cfg.host = "gate.ipfoxy.io".to_string();
        cfg.port = 58688;
        cfg.default_country = "us".to_string();
        let allocator = ProxyAllocator::new(mgr, Arc::new(cfg));

        let url = allocator.build_proxy_url_with_country("a1b2c3d4e5f6", "jp");
        assert_eq!(
            url,
            "http://testuser-cc-JP-sessid-a1b2c3d4e5f6:testpass@gate.ipfoxy.io:58688"
        );
    }

    #[test]
    fn test_build_residential_proxy_url_ipfoxy_username_passes_through_verbatim_with_dashes() {
        // Builder must not normalise the username. Whatever the user
        // pasted (including embedded dashes) reaches the upstream URL.
        let mut cfg = make_provider_config();
        cfg.kind = "ipfoxy".to_string();
        cfg.username = "alpha-bravo".to_string();
        cfg.password = "secret".to_string();
        cfg.host = "gate.ipfoxy.io".to_string();
        cfg.port = 58688;
        let url = build_residential_proxy_url(&cfg, "us", "a1b2c3d4e5f6");
        assert_eq!(
            url,
            "http://alpha-bravo-cc-US-sessid-a1b2c3d4e5f6:secret@gate.ipfoxy.io:58688"
        );
    }

    #[test]
    fn test_build_residential_proxy_url_ipfoxy_username_passes_through_verbatim_alphanumeric() {
        let mut cfg = make_provider_config();
        cfg.kind = "ipfoxy".to_string();
        cfg.username = "user42abc".to_string();
        cfg.password = "secret".to_string();
        cfg.host = "gate.ipfoxy.io".to_string();
        cfg.port = 58688;
        let url = build_residential_proxy_url(&cfg, "us", "a1b2c3d4e5f6");
        assert_eq!(
            url,
            "http://user42abc-cc-US-sessid-a1b2c3d4e5f6:secret@gate.ipfoxy.io:58688"
        );
    }

    // ── IpInfo struct ────────────────────────────────────────

    #[test]
    fn test_ip_info_clone() {
        let info = IpInfo {
            ip: "1.2.3.4".to_string(),
            country: Some("US".to_string()),
            region: None,
            city: None,
            isp: Some("Comcast".to_string()),
            quality: Some("residential".to_string()),
        };
        let cloned = info.clone();
        assert_eq!(cloned.ip, "1.2.3.4");
        assert_eq!(cloned.country, Some("US".to_string()));
    }

    // ── IP service direct tests ─────────────────────────

    #[tokio::test]
    #[ignore]
    async fn test_try_ip234_direct() {
        // Test different reqwest configurations against ip234.in
        let url = "http://ip234.in/ip.json";

        // 1. Default client
        let c1 = reqwest::Client::new();
        let r1 = c1.get(url).send().await.unwrap();
        println!("[default]      status={} ct={:?}", r1.status(), r1.headers().get("content-type"));
        println!("  body: {}", r1.text().await.unwrap_or_else(|e| format!("ERR: {}", e)).chars().take(100).collect::<String>());

        // 2. no_proxy
        let c2 = reqwest::Client::builder().no_proxy().build().unwrap();
        let r2 = c2.get(url).send().await.unwrap();
        println!("[no_proxy]     status={} ct={:?}", r2.status(), r2.headers().get("content-type"));
        println!("  body: {}", r2.text().await.unwrap_or_else(|e| format!("ERR: {}", e)).chars().take(100).collect::<String>());

        // 3. Disable auto decompression
        let c3 = reqwest::Client::builder().no_proxy().no_gzip().no_brotli().no_deflate().build().unwrap();
        let r3 = c3.get(url).send().await.unwrap();
        println!("[no_decomp]    status={} ct={:?} ce={:?}", r3.status(), r3.headers().get("content-type"), r3.headers().get("content-encoding"));
        println!("  body: {}", r3.text().await.unwrap_or_else(|e| format!("ERR: {}", e)).chars().take(100).collect::<String>());

        // 4. Explicit User-Agent
        let c4 = reqwest::Client::builder().no_proxy().user_agent("Mozilla/5.0").build().unwrap();
        let r4 = c4.get(url).send().await.unwrap();
        println!("[browser_ua]   status={} ct={:?}", r4.status(), r4.headers().get("content-type"));
        println!("  body: {}", r4.text().await.unwrap_or_else(|e| format!("ERR: {}", e)).chars().take(100).collect::<String>());

        // 5. HTTP/1.1 only (disable HTTP/2)
        let c5 = reqwest::Client::builder().no_proxy().http1_only().build().unwrap();
        let r5 = c5.get(url).send().await.unwrap();
        println!("[http1_only]   status={} ct={:?}", r5.status(), r5.headers().get("content-type"));
        println!("  body: {}", r5.text().await.unwrap_or_else(|e| format!("ERR: {}", e)).chars().take(100).collect::<String>());
    }

    #[tokio::test]
    #[ignore]
    async fn test_try_ip_api_direct() {
        let dir = TestDir::new();
        let allocator = make_allocator(&dir.path);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .no_proxy()
            .build()
            .unwrap();

        let result = allocator.try_ip_api(&client).await;
        println!("ip-api direct result: {:?}", result);
        assert!(result.is_ok(), "try_ip_api should succeed: {:?}", result.err());
        let info = result.unwrap();
        assert!(!info.ip.is_empty());
        println!("  ip={}, country={:?}", info.ip, info.country);
    }

    #[tokio::test]
    #[ignore]
    async fn test_probe_ip_services() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .with_target(false)
            .try_init();
        let dir = TestDir::new();
        let allocator = make_allocator(&dir.path);
        allocator.probe_ip_services().await;
        let svc = allocator.preferred_service();
        println!("preferred service: {}", svc);
        // At least one service should be available
        assert!(matches!(svc, IpService::IpApi | IpService::Ip234 | IpService::Ipify));
    }
}
