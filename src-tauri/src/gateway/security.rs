//! Security middleware — IP-based access control for the gateway.
//!
//! SecurityState holds in-memory whitelist/blacklist (DashMap) loaded from SQLite.
//! ip_check_middleware runs before auth, checking client IP against the configured mode.

use axum::{
    body::Body,
    extract::{ConnectInfo, Request},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::modules::gateway_db::GatewayDb;
use crate::modules::security_db::{self, SecurityMode};

/// Shared security state — loaded from SQLite, updated via IPC commands.
#[derive(Clone)]
pub struct SecurityState {
    pub mode: Arc<parking_lot::RwLock<SecurityMode>>,
    pub whitelist: Arc<DashSet<String>>,
    pub blacklist: Arc<DashSet<String>>,
    db: Arc<GatewayDb>,
}

impl SecurityState {
    /// Create a new SecurityState and load initial data from SQLite.
    pub fn new(db: Arc<GatewayDb>) -> Result<Self, String> {
        let state = Self {
            mode: Arc::new(parking_lot::RwLock::new(SecurityMode::Off)),
            whitelist: Arc::new(DashSet::new()),
            blacklist: Arc::new(DashSet::new()),
            db,
        };
        state.reload()?;
        Ok(state)
    }

    /// Reload all security data from SQLite into memory.
    pub fn reload(&self) -> Result<(), String> {
        // Load config
        let config = security_db::get_security_config(&self.db)?;
        *self.mode.write() = config.mode;

        // Load whitelist
        self.whitelist.clear();
        for entry in security_db::list_whitelist(&self.db)? {
            self.whitelist.insert(entry.ip);
        }

        // Load blacklist (skip expired)
        self.blacklist.clear();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        for entry in security_db::list_blacklist(&self.db)? {
            if entry.expires_at.map_or(true, |exp| exp > now) {
                self.blacklist.insert(entry.ip);
            }
        }

        Ok(())
    }

    /// Check if an IP is whitelisted.
    ///
    /// Whitelist entries may be either:
    /// - Exact IPs (e.g. "127.0.0.1", "192.168.1.50")
    /// - IPv4 CIDR ranges (e.g. "192.168.1.0/24") — supported so "LAN sharing"
    ///   actually allows other devices on the same subnet, matching the UI promise.
    ///
    /// Match order: exact first (fast path, O(1)), then scan CIDR entries.
    pub fn is_whitelisted(&self, ip: &str) -> bool {
        // Fast path: exact match
        if self.whitelist.contains(ip) {
            return true;
        }
        // Slow path: CIDR range scan (only entries containing '/')
        self.whitelist.iter().any(|entry| {
            let s = entry.as_str();
            s.contains('/') && ipv4_in_cidr(ip, s).unwrap_or(false)
        })
    }

    pub fn is_blacklisted(&self, ip: &str) -> bool {
        // Blacklist is exact-match only for v1; CIDR ban is a follow-up.
        self.blacklist.contains(ip)
    }

    pub fn current_mode(&self) -> SecurityMode {
        *self.mode.read()
    }

    /// Add IP to whitelist: write SQLite first, then update memory.
    pub fn add_whitelist(&self, ip: &str, description: Option<&str>) -> Result<i64, String> {
        let id = security_db::add_whitelist(&self.db, ip, description)?;
        self.whitelist.insert(ip.to_string());
        Ok(id)
    }

    /// Remove IP from whitelist by ID.
    pub fn remove_whitelist(&self, id: i64, ip: &str) -> Result<bool, String> {
        let removed = security_db::remove_whitelist(&self.db, id)?;
        if removed {
            self.whitelist.remove(ip);
        }
        Ok(removed)
    }

    /// Add IP to blacklist: write SQLite first, then update memory.
    pub fn add_blacklist(
        &self,
        ip: &str,
        reason: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<i64, String> {
        let id = security_db::add_blacklist(&self.db, ip, reason, expires_at)?;
        self.blacklist.insert(ip.to_string());
        Ok(id)
    }

    /// Remove IP from blacklist by ID.
    pub fn remove_blacklist(&self, id: i64, ip: &str) -> Result<bool, String> {
        let removed = security_db::remove_blacklist(&self.db, id)?;
        if removed {
            self.blacklist.remove(ip);
        }
        Ok(removed)
    }

    /// Update security mode: write SQLite first, then update memory.
    pub fn set_mode(&self, mode: SecurityMode) -> Result<(), String> {
        let mut config = security_db::get_security_config(&self.db)?;
        config.mode = mode;
        security_db::update_security_config(&self.db, &config)?;
        *self.mode.write() = mode;
        Ok(())
    }
}

/// Axum middleware: check client IP against security rules.
/// Must be layered BEFORE auth middleware (runs first in the request pipeline).
pub async fn ip_check_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // /health is always public
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    let security_state = match request.extensions().get::<SecurityState>() {
        Some(s) => s.clone(),
        None => return next.run(request).await, // No security configured
    };

    let ip = addr.ip().to_string();
    let mode = security_state.current_mode();

    match mode {
        SecurityMode::Off => next.run(request).await,
        SecurityMode::Whitelist => {
            if security_state.is_whitelisted(&ip) {
                next.run(request).await
            } else {
                tracing::warn!("IP {} blocked: not in whitelist", ip);
                (StatusCode::FORBIDDEN, "Access denied: IP not in whitelist").into_response()
            }
        }
        SecurityMode::Blacklist => {
            if security_state.is_blacklisted(&ip) {
                tracing::warn!("IP {} blocked: blacklisted", ip);
                (StatusCode::FORBIDDEN, "Access denied: IP blacklisted").into_response()
            } else {
                next.run(request).await
            }
        }
    }
}

// ── IPv4 CIDR matching ─────────────────────────────────────
//
// Small hand-rolled matcher (no extra crate dependency). Supports IPv4 /0–/32.
// IPv6 is not supported — LAN sharing on macOS desktop primarily targets IPv4.

/// Returns true if `ip` falls inside `cidr` (e.g. "192.168.1.42" in "192.168.1.0/24").
/// Returns None if either argument is malformed.
fn ipv4_in_cidr(ip: &str, cidr: &str) -> Option<bool> {
    let (network_str, prefix_str) = cidr.split_once('/')?;
    let prefix: u8 = prefix_str.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    let network: std::net::Ipv4Addr = network_str.parse().ok()?;
    let addr: std::net::Ipv4Addr = ip.parse().ok()?;
    let network_bits = u32::from(network);
    let addr_bits = u32::from(addr);
    let mask: u32 = if prefix == 0 {
        0
    } else {
        !0u32 << (32 - prefix)
    };
    Some((network_bits & mask) == (addr_bits & mask))
}

/// Derive the /24 network string from an IPv4 address — **only for RFC1918 private ranges**.
///
/// Returns `Some("a.b.c.0/24")` only when the IP is within a private network where /24
/// expansion definitely stays inside the user's LAN:
///   - 10.0.0.0/8        (class A private)
///   - 172.16.0.0/12     (class B private)
///   - 192.168.0.0/16    (class C private)
///
/// Returns `None` for other ranges to prevent over-whitelisting:
///   - 100.64.0.0/10  (CGNAT / Tailscale / Zerotier overlays — neighbors are OTHER users)
///   - 169.254.0.0/16 (link-local, DHCP-failure auto IP)
///   - Public IPv4 addresses (VPS / direct-attached — expansion would whitelist ISP neighbors)
///   - Loopback, multicast, etc.
///
/// Callers must fall back to a single-IP whitelist when this returns `None`.
pub fn lan_ip_to_slash_24(ip: &str) -> Option<String> {
    let addr: std::net::Ipv4Addr = ip.parse().ok()?;
    if !is_rfc1918(&addr) {
        return None;
    }
    let o = addr.octets();
    Some(format!("{}.{}.{}.0/24", o[0], o[1], o[2]))
}

/// RFC 1918 private IPv4 network check.
/// 10.0.0.0/8 | 172.16.0.0/12 | 192.168.0.0/16
fn is_rfc1918(addr: &std::net::Ipv4Addr) -> bool {
    let o = addr.octets();
    o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
}

// ── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── CIDR matching tests ───────────────────────────────────

    #[test]
    fn test_ipv4_in_cidr_slash_24() {
        // Inside the subnet
        assert_eq!(ipv4_in_cidr("192.168.1.1", "192.168.1.0/24"), Some(true));
        assert_eq!(ipv4_in_cidr("192.168.1.254", "192.168.1.0/24"), Some(true));
        assert_eq!(ipv4_in_cidr("192.168.1.50", "192.168.1.0/24"), Some(true));
        // Outside
        assert_eq!(ipv4_in_cidr("192.168.2.1", "192.168.1.0/24"), Some(false));
        assert_eq!(ipv4_in_cidr("10.0.0.1", "192.168.1.0/24"), Some(false));
    }

    #[test]
    fn test_ipv4_in_cidr_edge_prefixes() {
        // /32 = single host
        assert_eq!(ipv4_in_cidr("1.2.3.4", "1.2.3.4/32"), Some(true));
        assert_eq!(ipv4_in_cidr("1.2.3.5", "1.2.3.4/32"), Some(false));
        // /0 = match anything
        assert_eq!(ipv4_in_cidr("1.2.3.4", "0.0.0.0/0"), Some(true));
        assert_eq!(ipv4_in_cidr("255.255.255.255", "0.0.0.0/0"), Some(true));
        // /16 = bigger subnet
        assert_eq!(ipv4_in_cidr("10.0.5.1", "10.0.0.0/16"), Some(true));
        assert_eq!(ipv4_in_cidr("10.1.0.1", "10.0.0.0/16"), Some(false));
    }

    #[test]
    fn test_ipv4_in_cidr_malformed() {
        assert_eq!(ipv4_in_cidr("not an ip", "192.168.1.0/24"), None);
        assert_eq!(ipv4_in_cidr("192.168.1.1", "not a cidr"), None);
        assert_eq!(ipv4_in_cidr("192.168.1.1", "192.168.1.0/33"), None); // prefix too large
        assert_eq!(ipv4_in_cidr("192.168.1.1", "192.168.1.0"), None); // missing slash
    }

    #[test]
    fn test_lan_ip_to_slash_24_rfc1918_accepted() {
        // All three RFC1918 ranges get /24 expansion
        assert_eq!(lan_ip_to_slash_24("192.168.1.50"), Some("192.168.1.0/24".to_string()));
        assert_eq!(lan_ip_to_slash_24("10.0.0.42"), Some("10.0.0.0/24".to_string()));
        assert_eq!(lan_ip_to_slash_24("10.255.255.1"), Some("10.255.255.0/24".to_string()));
        assert_eq!(lan_ip_to_slash_24("172.16.99.200"), Some("172.16.99.0/24".to_string()));
        assert_eq!(lan_ip_to_slash_24("172.31.1.1"), Some("172.31.1.0/24".to_string()));
    }

    #[test]
    fn test_lan_ip_to_slash_24_non_rfc1918_rejected() {
        // Regression test for v1.0.1 P2-D followup: must NOT expand to /24 for
        // non-private IPs, which would over-whitelist overlay / CGNAT / public neighbors.

        // CGNAT / Tailscale / Zerotier — neighbors are OTHER users
        assert_eq!(lan_ip_to_slash_24("100.64.0.1"), None);
        assert_eq!(lan_ip_to_slash_24("100.127.255.254"), None);
        // Link-local
        assert_eq!(lan_ip_to_slash_24("169.254.1.1"), None);
        // Public IP (Google DNS)
        assert_eq!(lan_ip_to_slash_24("8.8.8.8"), None);
        // Public IP at RFC1918 boundary (172.32.x.x is PUBLIC — 172.16-172.31 is private)
        assert_eq!(lan_ip_to_slash_24("172.32.0.1"), None);
        assert_eq!(lan_ip_to_slash_24("172.15.0.1"), None);
        // Public IP near 192.168 (192.169.x.x is PUBLIC)
        assert_eq!(lan_ip_to_slash_24("192.169.0.1"), None);
        assert_eq!(lan_ip_to_slash_24("192.167.0.1"), None);
        // Loopback
        assert_eq!(lan_ip_to_slash_24("127.0.0.1"), None);
        // Malformed input
        assert_eq!(lan_ip_to_slash_24("not-an-ip"), None);
    }

    #[test]
    fn test_is_rfc1918() {
        // Class A
        assert!(is_rfc1918(&"10.0.0.0".parse().unwrap()));
        assert!(is_rfc1918(&"10.255.255.255".parse().unwrap()));
        assert!(!is_rfc1918(&"11.0.0.1".parse().unwrap()));
        assert!(!is_rfc1918(&"9.255.255.255".parse().unwrap()));
        // Class B boundary
        assert!(!is_rfc1918(&"172.15.255.255".parse().unwrap()));
        assert!(is_rfc1918(&"172.16.0.0".parse().unwrap()));
        assert!(is_rfc1918(&"172.31.255.255".parse().unwrap()));
        assert!(!is_rfc1918(&"172.32.0.0".parse().unwrap()));
        // Class C boundary
        assert!(!is_rfc1918(&"192.167.255.255".parse().unwrap()));
        assert!(is_rfc1918(&"192.168.0.0".parse().unwrap()));
        assert!(is_rfc1918(&"192.168.255.255".parse().unwrap()));
        assert!(!is_rfc1918(&"192.169.0.0".parse().unwrap()));
    }

    #[test]
    fn test_is_whitelisted_supports_cidr() {
        // Regression test for P2-D: whitelist must match CIDR ranges so LAN sharing
        // allows other devices on the same subnet.
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();

        // Add a mix of exact IPs and CIDR
        state.add_whitelist("127.0.0.1", Some("localhost")).unwrap();
        state.add_whitelist("192.168.1.0/24", Some("LAN subnet")).unwrap();

        // Exact match
        assert!(state.is_whitelisted("127.0.0.1"));
        // CIDR match: any IP in 192.168.1.0/24 should match
        assert!(state.is_whitelisted("192.168.1.1"));
        assert!(state.is_whitelisted("192.168.1.50"));
        assert!(state.is_whitelisted("192.168.1.254"));
        // Outside subnet
        assert!(!state.is_whitelisted("192.168.2.1"));
        assert!(!state.is_whitelisted("10.0.0.1"));
    }

    #[test]
    fn test_is_whitelisted_exact_match_priority() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();
        // Only exact IP, no CIDR
        state.add_whitelist("192.168.1.100", Some("single host")).unwrap();
        assert!(state.is_whitelisted("192.168.1.100"));
        // A different IP in the "would-be" same subnet must NOT match (no CIDR entry)
        assert!(!state.is_whitelisted("192.168.1.101"));
    }

    fn make_test_db() -> Arc<GatewayDb> {
        let path = std::env::temp_dir().join(format!(
            "claude_ultra_security_test_{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(GatewayDb::new(&path).unwrap());
        security_db::init_security_tables(&db).unwrap();
        db
    }

    // ── SecurityState unit tests ────────────────────────────

    #[test]
    fn test_security_state_default_mode_off() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();
        assert_eq!(state.current_mode(), SecurityMode::Off);
        assert!(state.whitelist.is_empty());
        assert!(state.blacklist.is_empty());
    }

    #[test]
    fn test_security_state_add_whitelist() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();

        state.add_whitelist("192.168.1.1", Some("test")).unwrap();
        assert!(state.is_whitelisted("192.168.1.1"));
        assert!(!state.is_whitelisted("10.0.0.1"));
    }

    #[test]
    fn test_security_state_add_blacklist() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();

        state.add_blacklist("1.2.3.4", Some("spam"), None).unwrap();
        assert!(state.is_blacklisted("1.2.3.4"));
        assert!(!state.is_blacklisted("5.6.7.8"));
    }

    #[test]
    fn test_security_state_remove_whitelist() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();

        let id = state.add_whitelist("192.168.1.1", None).unwrap();
        assert!(state.is_whitelisted("192.168.1.1"));

        state.remove_whitelist(id, "192.168.1.1").unwrap();
        assert!(!state.is_whitelisted("192.168.1.1"));
    }

    #[test]
    fn test_security_state_set_mode() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();

        state.set_mode(SecurityMode::Whitelist).unwrap();
        assert_eq!(state.current_mode(), SecurityMode::Whitelist);

        // set_mode already verified via current_mode() above
    }

    #[test]
    fn test_security_state_reload_from_db() {
        let db = make_test_db();

        // Write directly to DB
        security_db::add_whitelist(&db, "10.0.0.1", None).unwrap();
        security_db::add_blacklist(&db, "5.5.5.5", None, None).unwrap();
        let mut config = security_db::get_security_config(&db).unwrap();
        config.mode = SecurityMode::Blacklist;
        security_db::update_security_config(&db, &config).unwrap();

        // Create state — should load from DB
        let state = SecurityState::new(db).unwrap();
        assert_eq!(state.current_mode(), SecurityMode::Blacklist);
        assert!(state.is_whitelisted("10.0.0.1"));
        assert!(state.is_blacklisted("5.5.5.5"));
    }

    #[test]
    fn test_security_state_reload_skips_expired_blacklist() {
        let db = make_test_db();
        let past = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 10;
        security_db::add_blacklist(&db, "expired.ip", None, Some(past)).unwrap();
        security_db::add_blacklist(&db, "active.ip", None, None).unwrap();

        let state = SecurityState::new(db).unwrap();
        assert!(!state.is_blacklisted("expired.ip"));
        assert!(state.is_blacklisted("active.ip"));
    }

    // ── Middleware integration tests ────────────────────────

    #[tokio::test]
    async fn test_middleware_off_mode_allows_all() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();

        let app = build_test_app(state);
        let resp = send_test_request(&app, "1.2.3.4", "/v1/messages").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_whitelist_allows_listed() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();
        state.set_mode(SecurityMode::Whitelist).unwrap();
        state.add_whitelist("192.168.1.1", None).unwrap();

        let app = build_test_app(state);
        let resp = send_test_request(&app, "192.168.1.1", "/v1/messages").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_whitelist_blocks_unlisted() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();
        state.set_mode(SecurityMode::Whitelist).unwrap();
        state.add_whitelist("192.168.1.1", None).unwrap();

        let app = build_test_app(state);
        let resp = send_test_request(&app, "10.0.0.1", "/v1/messages").await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_middleware_blacklist_blocks_listed() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();
        state.set_mode(SecurityMode::Blacklist).unwrap();
        state.add_blacklist("1.2.3.4", None, None).unwrap();

        let app = build_test_app(state);
        let resp = send_test_request(&app, "1.2.3.4", "/v1/messages").await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_middleware_blacklist_allows_unlisted() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();
        state.set_mode(SecurityMode::Blacklist).unwrap();
        state.add_blacklist("1.2.3.4", None, None).unwrap();

        let app = build_test_app(state);
        let resp = send_test_request(&app, "5.6.7.8", "/v1/messages").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_health_always_allowed() {
        let db = make_test_db();
        let state = SecurityState::new(db).unwrap();
        state.set_mode(SecurityMode::Whitelist).unwrap();
        // No IPs whitelisted — but /health should still work

        let app = build_test_app(state);
        let resp = send_test_request(&app, "1.2.3.4", "/health").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Test helpers ────────────────────────────────────────

    fn build_test_app(security_state: SecurityState) -> axum::Router {
        use axum::{middleware, routing::get};

        axum::Router::new()
            .route("/health", get(|| async { "ok" }))
            .route(
                "/v1/messages",
                axum::routing::post(|| async { "messages" }),
            )
            .layer(middleware::from_fn(ip_check_middleware))
            .layer(axum::Extension(security_state))
    }

    async fn send_test_request(
        app: &axum::Router,
        ip: &str,
        path: &str,
    ) -> Response {
        use axum::body::Body;
        use tower::ServiceExt;

        let addr: SocketAddr = format!("{}:12345", ip).parse().unwrap();

        let mut request = axum::http::Request::builder()
            .method(if path == "/health" { "GET" } else { "POST" })
            .uri(path)
            .body(Body::empty())
            .unwrap();

        request.extensions_mut().insert(ConnectInfo(addr));

        app.clone().oneshot(request).await.unwrap()
    }
}
