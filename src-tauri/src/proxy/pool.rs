//! ProxyPool — unified proxy connection management.
//!
//! Appears to handler as an HTTP client routed by account_id.
//! Internally manages connection pool, proxy allocation, and health checks.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use dashmap::DashMap;
use http_body_util::Full;
use hyper::client::conn::http1::SendRequest;
use tokio::sync::{mpsc, Mutex as TokioMutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use claude_ultra_http::BoringClient;

use crate::models::account::ProxySection;
use crate::proxy::allocator::ProxyAllocator;

// ─── PoolConfig ────────────────────────────────────────────

/// ProxyPool configuration parameters.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_conns_per_account: usize,
    pub max_lifetime: Duration,
    pub active_probe_interval: Duration,
    pub max_standby_per_account: usize,
    pub cold_account_threshold: Duration,
    pub slow_threshold_ms: u64,
    pub significance_margin_ms: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_conns_per_account: 4,
            max_lifetime: Duration::from_secs(900),
            active_probe_interval: Duration::from_secs(60),
            max_standby_per_account: 3,
            cold_account_threshold: Duration::from_secs(600),
            slow_threshold_ms: 2500,
            significance_margin_ms: 100,
        }
    }
}

// ─── GeoPolicy ─────────────────────────────────────────────

/// Geo-verification policy.
#[derive(Debug, Clone)]
pub struct GeoPolicy {
    pub expected_country: String,
}

impl GeoPolicy {
    /// Check if country matches (supports ISO code "US" and full name "United States").
    pub fn country_matches(&self, country: &str) -> bool {
        let expected = self.expected_country.to_lowercase();
        let actual = country.to_lowercase();
        if expected == actual { return true; }
        // ISO code → full name mapping (common)
        let aliases: &[(&str, &[&str])] = &[
            ("us", &["united states", "united states of america"]),
            ("jp", &["japan"]),
            ("kr", &["south korea", "korea, republic of"]),
            ("gb", &["united kingdom"]),
            ("de", &["germany"]),
            ("fr", &["france"]),
            ("au", &["australia"]),
            ("ca", &["canada"]),
            ("sg", &["singapore"]),
        ];
        for (code, names) in aliases {
            if expected == *code && names.contains(&actual.as_str()) { return true; }
            if names.contains(&expected.as_str()) && *code == actual { return true; }
        }
        false
    }
}

// ─── ProxyConn ─────────────────────────────────────────────

/// An established proxy connection (TCP → CONNECT → TLS → HTTP/1.1).
pub struct ProxyConn {
    pub sender: SendRequest<Full<Bytes>>,
    pub conn_handle: JoinHandle<()>,
    pub session_id: String,
    pub created_at: Instant,
    pub last_used: Instant,
}

impl ProxyConn {
    /// Whether the connection is closed.
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    /// Whether the connection has exceeded max_lifetime.
    pub fn is_expired(&self, max_lifetime: Duration) -> bool {
        self.created_at.elapsed() > max_lifetime
    }
}

impl Drop for ProxyConn {
    fn drop(&mut self) {
        // Abort conn driver task to prevent detached TCP connection leaks
        self.conn_handle.abort();
    }
}

// ─── ConnStat ──────────────────────────────────────────────

/// Single connection establishment timing record.
#[derive(Debug, Clone)]
pub struct ConnStat {
    pub connect_ms: u64,
    pub success: bool,
    pub timestamp: Instant,
}

// ─── ProxyError ────────────────────────────────────────────

/// ProxyPool error types. Handler decides retry strategy based on type.
#[derive(Debug)]
pub enum ProxyError {
    /// Proxy connection failed → handler failover
    ConnectionFailed(String),
    /// Proxy resources exhausted → handler failover
    Exhausted(String),
    /// Upstream HTTP error → handler HTTP retry
    Upstream(http::StatusCode, String),
    /// Request construction error
    Request(String),
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionFailed(msg) => write!(f, "proxy connection failed: {}", msg),
            Self::Exhausted(msg) => write!(f, "proxy exhausted: {}", msg),
            Self::Upstream(status, msg) => write!(f, "upstream HTTP {}: {}", status, msg),
            Self::Request(msg) => write!(f, "request error: {}", msg),
        }
    }
}

// ─── ActiveSession ─────────────────────────────────────────

/// Currently active proxy session + connection pool.
pub struct ActiveSession {
    pub session: ProxySection,
    pub proxy_url: String,
    pub idle_conns: Vec<ProxyConn>,
    pub checked_out: usize,
}

// ─── StandbySession ────────────────────────────────────────

/// Pre-warmed standby proxy session.
pub struct StandbySession {
    pub session: ProxySection,
    pub proxy_url: String,
    pub warm_conn: Option<ProxyConn>,
    pub avg_connect_ms: f64,
}

// ─── AccountProxy ──────────────────────────────────────────

/// All proxy state for one account. Protected by TokioMutex.
pub struct AccountProxy {
    pub active: ActiveSession,
    pub standby: Vec<StandbySession>,
    pub disabled: bool,
    pub stats: VecDeque<ConnStat>,
    pub avg_connect_ms: f64,
    pub failure_streak: u32,
    pub last_active_at: Instant,
}

// ─── ReleaseMsg ────────────────────────────────────────────

/// Connection release message. Sent via mpsc channel when WrappedBody is dropped.
pub enum ReleaseMsg {
    Return {
        account_id: String,
        conn: ProxyConn,
        is_pooled: bool,
    },
    Discard {
        account_id: String,
        is_pooled: bool,
    },
}

// ─── AcquireResult ─────────────────────────────────────────

/// Return value from acquire.
pub struct AcquireResult {
    pub conn: ProxyConn,
    pub session_id: String,
    pub is_pooled: bool,
}

// ─── Maintain task ──────────────────────────────────────────

/// Task input: probe / standby / challenger unified into one Task type.
enum TaskSetup {
    /// Probe: use existing proxy_url, no new session allocation.
    UseExisting(String),
    /// Standby / challenger: allocate_ephemeral(country) to get a new session.
    Allocate(String),
}

struct ProxyTask {
    setup: TaskSetup,
    verify_ip: bool,
    samples: usize,
}

/// Task output: unified result.
struct ProxyTaskResult {
    session: Option<ProxySection>,
    proxy_url: String,
    ip_info: Option<crate::proxy::allocator::IpInfo>,
    /// Per-establish (conn, connect_ms). One entry per successful attempt.
    conns: Vec<(ProxyConn, u64)>,
}

// ─── ProxyPool ─────────────────────────────────────────────

/// Unified proxy connection manager. Single instance for the entire app lifetime.
pub struct ProxyPool {
    pub accounts: DashMap<String, Arc<TokioMutex<AccountProxy>>>,
    ensure_locks: DashMap<String, Arc<TokioMutex<()>>>,
    switch_locks: DashMap<String, Arc<TokioMutex<()>>>,
    pub boring_client: Arc<BoringClient>,
    pub allocator: Arc<ProxyAllocator>,
    pub geo_policy: GeoPolicy,
    pub config: PoolConfig,
    pub release_tx: std::sync::RwLock<mpsc::Sender<ReleaseMsg>>,
    release_rx: std::sync::Mutex<Option<mpsc::Receiver<ReleaseMsg>>>,
    cancel_token: std::sync::RwLock<CancellationToken>,
    /// Guards start_background_tasks — reset on stop()
    background_started: std::sync::atomic::AtomicBool,
}

impl ProxyPool {
    pub fn new(
        config: PoolConfig,
        boring_client: Arc<BoringClient>,
        allocator: Arc<ProxyAllocator>,
        geo_policy: GeoPolicy,
        cancel_token: CancellationToken,
    ) -> Arc<Self> {
        let (release_tx, release_rx) = mpsc::channel(256);
        Arc::new(Self {
            accounts: DashMap::new(),
            ensure_locks: DashMap::new(),
            switch_locks: DashMap::new(),
            boring_client,
            allocator,
            geo_policy,
            config,
            release_tx: std::sync::RwLock::new(release_tx),
            release_rx: std::sync::Mutex::new(Some(release_rx)),
            cancel_token: std::sync::RwLock::new(cancel_token),
            background_started: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Stop all background tasks: cancel current token, replace with a fresh one,
    /// reset background_started flag and rebuild release channel so start_background_tasks
    /// can be called again after re-login.
    pub fn stop(&self) {
        // Cancel all running tasks
        {
            let token = self.cancel_token.read().unwrap().clone();
            token.cancel();
        }
        // Replace with fresh token
        {
            let mut token = self.cancel_token.write().unwrap();
            *token = CancellationToken::new();
        }
        // Rebuild release channel — old consumer exits (cancel_token cancelled),
        // old WrappedBody Drop handlers hold clones of old tx → send to dead channel (harmless).
        // New connections after restart will clone from the new tx.
        {
            let (tx, rx) = mpsc::channel(256);
            *self.release_tx.write().unwrap() = tx;
            *self.release_rx.lock().unwrap() = Some(rx);
        }
        // Clear per-account state — ensures ensure_proxy() will re-create AccountProxy
        // and re-spawn maintain loops on next init_services
        self.accounts.clear();
        self.ensure_locks.clear();
        self.switch_locks.clear();
        // Reset background flag
        self.background_started.store(false, std::sync::atomic::Ordering::SeqCst);
        tracing::info!("[POOL] stopped, ready for restart");
    }

    /// Disable account: mark disabled + clear idle connections and standby.
    /// AccountProxy remains in DashMap, never removed. Checked-out connections expire naturally.
    /// Shares ensure_lock with ensure_proxy to prevent interleaving that could create orphan accounts.
    pub async fn disable_in_pool(self: &Arc<Self>, account_id: &str) {
        let lock = self.ensure_locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        if let Some(arc) = self.accounts.get(account_id).map(|r| r.clone()) {
            let mut state = arc.lock().await;
            state.disabled = true;
            state.active.idle_conns.clear();
            state.standby.clear();
            tracing::info!("[POOL] account {} disabled (checked_out={})",
                account_id, state.active.checked_out);
        }
    }

    /// Enable account: clear disabled flag, connections rebuilt naturally on next acquire.
    pub async fn enable_in_pool(self: &Arc<Self>, account_id: &str) {
        if let Some(arc) = self.accounts.get(account_id).map(|r| r.clone()) {
            let mut state = arc.lock().await;
            state.disabled = false;
            tracing::info!("[POOL] account {} enabled", account_id);
        }
    }

    /// Ensure account has an active session. Lazily initialized on first request.
    pub async fn ensure_proxy(self: &Arc<Self>, account_id: &str) -> Result<(), ProxyError> {
        // Fast path
        if self.accounts.contains_key(account_id) {
            return Ok(());
        }

        // Slow path: per-account dedup lock
        let lock = self.ensure_locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Double-check
        if self.accounts.contains_key(account_id) {
            return Ok(());
        }

        // Allocate and persist (using geo_policy country, consistent with standby)
        let country = &self.geo_policy.expected_country;
        let session = self.allocator.allocate(account_id).await
            .map_err(|e| ProxyError::Exhausted(e))?;
        let proxy_url = self.allocator.build_proxy_url_with_country(
            &session.session_id, country,
        );

        // user_disabled accounts enter Pool in disabled mode (verify_ip only, no standby maintenance)
        let user_disabled = self.allocator.is_user_disabled(account_id).await;

        self.accounts.insert(account_id.to_string(), Arc::new(TokioMutex::new(
            AccountProxy {
                active: ActiveSession {
                    session,
                    proxy_url,
                    idle_conns: vec![],
                    checked_out: 0,
                },
                standby: vec![],

                disabled: user_disabled,

                stats: VecDeque::new(),
                avg_connect_ms: 0.0,
                failure_streak: 0,
                last_active_at: Instant::now(),
            }
        )));

        // Start per-account maintain loop
        self.spawn_account_maintain_loop(account_id.to_string());

        Ok(())
    }

    /// Acquire a proxy connection. Prefers idle_conns, otherwise creates new.
    pub async fn acquire(self: &Arc<Self>, account_id: &str) -> Result<AcquireResult, ProxyError> {
        // 1. ensure_proxy
        self.ensure_proxy(account_id).await?;

        // 2. Get account state (DashMap guard must not span .await)
        let arc = self.accounts.get(account_id)
            .map(|r| r.clone())
            .ok_or_else(|| ProxyError::Exhausted("account not in pool".to_string()))?;

        // 3. Check disabled + try to get idle connection
        let mut state = arc.lock().await;
        if state.disabled {
            return Err(ProxyError::Exhausted(format!("account {} is disabled", account_id)));
        }
        state.last_active_at = Instant::now();
        let session_id = state.active.session.session_id.clone();
        let proxy_url = state.active.proxy_url.clone();

        while let Some(conn) = state.active.idle_conns.pop() {
            if !conn.is_closed() && !conn.is_expired(self.config.max_lifetime) {
                state.active.checked_out += 1;
                tracing::debug!("[POOL] acquire hit for {}", account_id);
                return Ok(AcquireResult { conn, session_id, is_pooled: true });
            }
        }

        // 4. No idle connections, need to create new
        let current_total = state.active.idle_conns.len() + state.active.checked_out;
        let is_pooled = current_total < self.config.max_conns_per_account;
        if is_pooled {
            state.active.checked_out += 1;
        }
        drop(state);

        // 5. Establish connection (~1200ms, lock not held)
        tracing::debug!("[POOL] acquire miss for {}, establishing...", account_id);
        let establish_result = self.boring_client
            .establish_proxy_connection(&proxy_url, "api.anthropic.com", 443)
            .await;
        let (sender, conn_handle, _timings) = match establish_result {
            Ok(r) => r,
            Err(e) => {
                if is_pooled {
                    let tx = self.release_tx.read().unwrap().clone();
                    if let Err(send_err) = tx.send(ReleaseMsg::Discard {
                        account_id: account_id.to_string(),
                        is_pooled: true,
                    }).await {
                        tracing::error!("[POOL] release channel send failed, checked_out may leak: {}", send_err);
                    }
                }
                return Err(ProxyError::ConnectionFailed(format!("{}", e)));
            }
        };

        let conn = ProxyConn {
            sender,
            conn_handle,
            session_id: session_id.clone(),
            created_at: Instant::now(),
            last_used: Instant::now(),
        };

        // 6. Re-lock and check if session changed
        if is_pooled {
            let state = arc.lock().await;
            if state.active.session.session_id != session_id {
                drop(state);
                let tx = self.release_tx.read().unwrap().clone();
                if let Err(e) = tx.send(ReleaseMsg::Discard {
                    account_id: account_id.to_string(),
                    is_pooled: true,
                }).await {
                    tracing::error!("[POOL] release channel send failed: {}", e);
                }
                tracing::debug!("[POOL] acquire: session changed during establish, overflow");
                return Ok(AcquireResult { conn, session_id, is_pooled: false });
            }
        }

        Ok(AcquireResult { conn, session_id, is_pooled })
    }

    /// Start release consumer background task. Idempotent — repeated calls don't panic.
    pub fn start_release_consumer(self: &Arc<Self>) {
        let release_rx = match self.release_rx.lock().unwrap().take() {
            Some(rx) => rx,
            None => {
                tracing::warn!("[POOL] release consumer already started, skipping");
                return;
            }
        };
        let pool = self.clone();
        tokio::spawn(async move {
            pool.release_consumer_loop(release_rx).await;
        });
    }

    async fn release_consumer_loop(&self, mut release_rx: mpsc::Receiver<ReleaseMsg>) {
        let cancel = self.cancel_token.read().unwrap().clone();
        loop {
            tokio::select! {
                msg = release_rx.recv() => {
                    match msg {
                        Some(ReleaseMsg::Return { account_id, mut conn, is_pooled }) => {
                            let arc = match self.accounts.get(&account_id).map(|r| r.clone()) {
                                Some(a) => a,
                                None => continue,
                            };
                            let mut state = arc.lock().await;

                            if is_pooled
                                && conn.session_id == state.active.session.session_id
                                && !conn.is_closed()
                                && !conn.is_expired(self.config.max_lifetime)
                            {
                                conn.last_used = Instant::now();
                                state.active.idle_conns.push(conn);
                                tracing::debug!("[POOL] release return for {}", account_id);
                            } else {
                                tracing::debug!("[POOL] release discard for {}", account_id);
                            }

                            if is_pooled {
                                if state.active.checked_out == 0 {
                                    tracing::warn!("[POOL] checked_out underflow for {} (return), skipping", account_id);
                                } else {
                                    state.active.checked_out -= 1;
                                }
                            }
                        }
                        Some(ReleaseMsg::Discard { account_id, is_pooled }) => {
                            if is_pooled {
                                if let Some(arc) = self.accounts.get(&account_id).map(|r| r.clone()) {
                                    let mut state = arc.lock().await;
                                    if state.active.checked_out == 0 {
                                        tracing::warn!("[POOL] checked_out underflow for {} (discard), skipping", account_id);
                                    } else {
                                        state.active.checked_out -= 1;
                                    }
                                }
                            }
                        }
                        None => break,
                    }
                }
                _ = cancel.cancelled() => {
                    tracing::info!("[POOL] release consumer shutting down");
                    break;
                }
            }
        }
    }

    // ── Background tasks ────────────────────────────────────

    /// Start global background tasks (release consumer).
    /// Per-account maintain tasks auto-spawn when ensure_proxy creates AccountProxy.
    /// Whether background tasks have been started (and not yet stopped).
    /// Get a clone of the current cancel token (for child tasks to listen on).
    pub fn cancel_token(&self) -> tokio_util::sync::CancellationToken {
        self.cancel_token.read().unwrap().clone()
    }

    pub fn is_background_started(&self) -> bool {
        self.background_started.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn start_background_tasks(self: &Arc<Self>) {
        if self.background_started.swap(true, std::sync::atomic::Ordering::SeqCst) {
            tracing::info!("[POOL] background tasks already started, skipping");
            return;
        }
        self.start_release_consumer();
        let token = self.cancel_token.read().unwrap().clone();
        self.allocator.start_health_loop(token);
    }

    /// Start independent maintain loop for a single account. Called when ensure_proxy creates AccountProxy.
    fn spawn_account_maintain_loop(self: &Arc<Self>, account_id: String) {
        let pool = self.clone();
        tokio::spawn(async move {
            pool.account_maintain_loop(&account_id).await;
        });
    }

    // ── Maintain loop ───────────────────────────────────────

    async fn account_maintain_loop(self: &Arc<Self>, account_id: &str) {
        let cancel = self.cancel_token.read().unwrap().clone();
        let mut interval = tokio::time::interval(self.config.active_probe_interval);
        interval.tick().await; // skip immediate first tick

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.run_maintain_cycle(account_id).await;
                }
                _ = cancel.cancelled() => {
                    tracing::debug!("[POOL] account {} maintain loop shutting down", account_id);
                    break;
                }
            }
        }
    }

    /// Execute a ProxyTask (pure IO, no state mutation).
    async fn run_proxy_task(
        allocator: Arc<ProxyAllocator>,
        boring: Arc<BoringClient>,
        task: ProxyTask,
    ) -> Option<ProxyTaskResult> {
        let (session, proxy_url) = match task.setup {
            TaskSetup::UseExisting(url) => (None, url),
            TaskSetup::Allocate(country) => {
                let s = allocator.allocate_ephemeral(&country).await.ok()?;
                let url = allocator.build_proxy_url_with_country(&s.session_id, &country);
                (Some(s), url)
            }
        };

        let ip_info = if task.verify_ip {
            allocator.verify_ip(&proxy_url).await.ok()
        } else {
            None
        };

        let mut conns = Vec::new();
        for _ in 0..task.samples {
            match boring.establish_proxy_connection(&proxy_url, "api.anthropic.com", 443).await {
                Ok((sender, handle, timings)) => {
                    let session_id = session.as_ref()
                        .map(|s| s.session_id.clone())
                        .unwrap_or_default();
                    conns.push((ProxyConn {
                        sender, conn_handle: handle, session_id,
                        created_at: Instant::now(), last_used: Instant::now(),
                    }, timings.total_ms()));
                }
                Err(_) => {}
            }
        }

        Some(ProxyTaskResult { session, proxy_url, ip_info, conns })
    }

    /// One maintain cycle: 5 tasks in parallel (1 probe + 3 standby slots + 1 challenger) → process results → evict.
    ///
    /// Each standby slot behavior depends on state:
    ///   Has session → UseExisting + verify_ip (health probe)
    ///   Empty slot  → Allocate + samples=3 (acts as challenger, moves in if it wins)
    ///
    /// [4] Challenger always Allocate + samples=3, challenges the slowest standby.
    pub async fn run_maintain_cycle(self: &Arc<Self>, account_id: &str) {
        // Network down → skip all IO (global state from ProxyAllocator)
        if !self.allocator.is_network_up() {
            return;
        }

        let arc = match self.accounts.get(account_id).map(|r| r.clone()) {
            Some(a) => a,
            None => return,
        };
        let state = arc.lock().await;

        // Skip cold accounts
        if state.last_active_at.elapsed() > self.config.cold_account_threshold {
            return;
        }

        // Disabled accounts: verify_ip only (no establish/standby/challenger)
        if state.disabled {
            let active_url = state.active.proxy_url.clone();
            let current_ip = state.active.session.last_ip.clone().unwrap_or_default();
            drop(state);

            match self.allocator.verify_ip(&active_url).await {
                Ok(info) => {
                    // country=None (e.g. ipify mode) → skip country check
                    let country_bad = info.country.as_deref()
                        .map(|c| !self.geo_policy.country_matches(c))
                        .unwrap_or(false);
                    if country_bad {
                        tracing::warn!("[POOL] disabled {} country mismatch ({:?}), renewing",
                            account_id, info.country);
                        // Disabled accounts have no standby, allocate new session directly
                        let country = self.geo_policy.expected_country.clone();
                        if let Ok(session) = self.allocator.allocate_ephemeral(&country).await {
                            let proxy_url = self.allocator.build_proxy_url_with_country(
                                &session.session_id, &country,
                            );
                            let _ = self.allocator.commit_session(account_id, &session).await;
                            let mut state = arc.lock().await;
                            state.active.session = session;
                            state.active.proxy_url = proxy_url;
                        }
                    } else {
                        let ip_changed = !current_ip.is_empty() && info.ip != current_ip;
                        if ip_changed {
                            let mut state = arc.lock().await;
                            state.active.session.last_ip = Some(info.ip.clone());
                            tracing::debug!("[POOL] disabled {} IP rotated: {} → {}",
                                account_id, current_ip, info.ip);
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!("[POOL] disabled {} verify failed: {}", account_id, e);
                }
            }
            return;
        }

        let active_url = state.active.proxy_url.clone();
        let active_session_id = state.active.session.session_id.clone();
        let current_ip = state.active.session.last_ip.clone().unwrap_or_default();
        // Build standby slot tasks
        let mut standby_urls: Vec<Option<String>> = Vec::new();
        for i in 0..self.config.max_standby_per_account {
            standby_urls.push(state.standby.get(i).map(|s| s.proxy_url.clone()));
        }
        drop(state);

        let timeout = Duration::from_secs(20);
        let country = self.geo_policy.expected_country.clone();

        // ── Build 5 tasks ──

        let mut tasks = Vec::with_capacity(5);

        // [0] probe active
        tasks.push(ProxyTask {
            setup: TaskSetup::UseExisting(active_url),
            verify_ip: true,
            samples: 1,
        });

        // [1..3] standby slots: occupied → probe, empty → allocate as challenger
        for url in &standby_urls {
            match url {
                Some(url) => tasks.push(ProxyTask {
                    setup: TaskSetup::UseExisting(url.clone()),
                    verify_ip: true,
                    samples: 1,
                }),
                None => tasks.push(ProxyTask {
                    setup: TaskSetup::Allocate(country.clone()),
                    verify_ip: false,
                    samples: 3,
                }),
            }
        }

        // [4] challenger (always allocate new session)
        tasks.push(ProxyTask {
            setup: TaskSetup::Allocate(country.clone()),
            verify_ip: false,
            samples: 3,
        });

        // ── Spawn in parallel ──

        let task_desc: Vec<String> = tasks.iter().enumerate().map(|(i, t)| {
            let setup = match &t.setup {
                TaskSetup::UseExisting(_) => "probe",
                TaskSetup::Allocate(_) => "allocate",
            };
            format!("[{}]{}×{}", i, setup, t.samples)
        }).collect();
        tracing::info!("[POOL] {} cycle: {}", account_id, task_desc.join(" "));

        let handles: Vec<_> = tasks.into_iter().map(|task| {
            let allocator = self.allocator.clone();
            let boring = self.boring_client.clone();
            tokio::spawn(Self::run_proxy_task(allocator, boring, task))
        }).collect();

        // ── Wait for completion (max 10s) ──

        let join_results = tokio::time::timeout(
            timeout,
            futures::future::join_all(handles),
        ).await;

        let mut results: Vec<Option<ProxyTaskResult>> = match join_results {
            Ok(rs) => rs.into_iter().map(|r| r.ok().flatten()).collect(),
            Err(_) => {
                tracing::warn!("[POOL] {} maintain cycle timed out", account_id);
                return;
            }
        };

        // ── [0] Process active probe ──

        if let Some(mut probe) = results[0].take() {
            if let Some(info) = &probe.ip_info {
                // country=None (e.g. ipify mode) → skip country check
                let country_mismatch = info.country.as_deref()
                    .map(|c| !self.geo_policy.country_matches(c))
                    .unwrap_or(false);
                if country_mismatch {
                    tracing::warn!("[POOL] country mismatch for {}: expected {}, got {:?}, switching",
                        account_id, self.geo_policy.expected_country, info.country);
                    self.try_switch_to_standby(account_id).await;
                    return;
                }
                let ip_changed = !current_ip.is_empty() && info.ip != current_ip;
                if ip_changed {
                    let mut state = arc.lock().await;
                    state.active.session.last_ip = Some(info.ip.clone());
                    tracing::info!("[POOL] IP rotated for {}: {} → {} (country OK)",
                        account_id, current_ip, info.ip);
                }
            }

            let mut state = arc.lock().await;
            if let Some((mut conn, connect_ms)) = probe.conns.pop() {
                state.stats.push_back(ConnStat { connect_ms, success: true, timestamp: Instant::now() });
                if state.stats.len() > 20 { state.stats.pop_front(); }
                let success_stats: Vec<_> = state.stats.iter().filter(|s| s.success).collect();
                state.avg_connect_ms = if success_stats.is_empty() { 0.0 } else {
                    success_stats.iter().map(|s| s.connect_ms as f64).sum::<f64>() / success_stats.len() as f64
                };
                state.failure_streak = 0;

                // P1-2: verify session wasn't switched during probe
                let current_session_id = state.active.session.session_id.clone();
                if current_session_id == active_session_id {
                    let total = state.active.idle_conns.len() + state.active.checked_out;
                    if total < self.config.max_conns_per_account && !conn.is_closed() {
                        conn.session_id = current_session_id;
                        state.active.idle_conns.push(conn);
                    }
                } else {
                    tracing::debug!("[POOL] probe conn discarded: session switched during cycle");
                }
            } else {
                state.stats.push_back(ConnStat { connect_ms: 0, success: false, timestamp: Instant::now() });
                if state.stats.len() > 20 { state.stats.pop_front(); }
                state.failure_streak += 1;
            }

            if state.avg_connect_ms > self.config.slow_threshold_ms as f64 || state.failure_streak > 3 {
                tracing::warn!("[POOL] {} quality degraded: avg={}ms, streak={}, switching",
                    account_id, state.avg_connect_ms as u64, state.failure_streak);
                drop(state);
                self.try_switch_to_standby(account_id).await;
                return;
            }
        }

        // ── [1..3] Process standby slots ──

        let max_standby = self.config.max_standby_per_account;
        let margin = self.config.significance_margin_ms as f64;

        {
            let mut state = arc.lock().await;

            // Collect session_ids to remove first (P2-1: don't delete by index during forward iteration)
            let mut remove_ids: Vec<String> = Vec::new();

            for (slot_idx, url_opt) in standby_urls.iter().enumerate() {
                let result_idx = slot_idx + 1;
                let r = match results[result_idx].take() {
                    Some(r) => r,
                    None => continue,
                };

                match url_opt {
                    Some(url) => {
                        // Probe existing standby: match by URL (not index)
                        let standby_entry = state.standby.iter_mut()
                            .find(|s| s.proxy_url == *url);

                        if let Some(info) = &r.ip_info {
                            // country=None (e.g. ipify mode) → skip country check
                            let country_bad = info.country.as_deref()
                                .map(|c| !self.geo_policy.country_matches(c))
                                .unwrap_or(false);
                            if country_bad {
                                tracing::warn!("[POOL] standby country mismatch for {}, marking removal", url);
                                remove_ids.push(url.clone());
                                continue;
                            }
                        }

                        if let Some((_, ms)) = r.conns.first() {
                            if let Some(s) = standby_entry {
                                s.avg_connect_ms = *ms as f64;
                            }
                        } else {
                            tracing::warn!("[POOL] standby probe failed for {}, marking removal", url);
                            remove_ids.push(url.clone());
                        }
                    }
                    None => {
                        // Empty slot → challenger (samples=3), use median to move in
                        if r.conns.len() >= 3 {
                            let mut times: Vec<u64> = r.conns.iter().map(|(_, ms)| *ms).collect();
                            times.sort();
                            let median_ms = times[times.len() / 2] as f64;

                            if let (Some(session), Some((conn, _))) = (r.session, r.conns.into_iter().last()) {
                                if state.standby.len() < max_standby {
                                    tracing::info!("[POOL] new standby for {}: median {}ms (samples {:?})",
                                        account_id, median_ms, times);
                                    state.standby.push(StandbySession {
                                        session, proxy_url: r.proxy_url,
                                        warm_conn: Some(conn),
                                        avg_connect_ms: median_ms,
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // Batch remove bad standby entries
            if !remove_ids.is_empty() {
                state.standby.retain(|s| !remove_ids.contains(&s.proxy_url));
            }
            state.standby.sort_by(|a, b| a.avg_connect_ms.total_cmp(&b.avg_connect_ms));
        }

        // ── [4] Process challenger ──

        if let Some(cr) = results.get_mut(1 + max_standby).and_then(|r| r.take()) {
            if cr.conns.len() >= 3 {
                let mut times: Vec<u64> = cr.conns.iter().map(|(_, ms)| *ms).collect();
                times.sort();
                let median_ms = times[times.len() / 2] as f64;

                // Re-read worst (may have been updated by standby probe)
                let mut state = arc.lock().await;
                let current_worst = state.standby.last().map(|s| s.avg_connect_ms).unwrap_or(f64::MAX);

                if median_ms < current_worst - margin {
                    if let (Some(session), Some((conn, _))) = (cr.session, cr.conns.into_iter().last()) {
                        if state.standby.len() >= max_standby {
                            state.standby.pop();
                        }
                        tracing::info!("[POOL] challenger won for {}: median {}ms (samples {:?}) replaced worst {}ms",
                            account_id, median_ms, times, current_worst);
                        state.standby.push(StandbySession {
                            session, proxy_url: cr.proxy_url,
                            warm_conn: Some(conn),
                            avg_connect_ms: median_ms,
                        });
                        state.standby.sort_by(|a, b| a.avg_connect_ms.total_cmp(&b.avg_connect_ms));
                    }
                } else {
                    tracing::debug!("[POOL] challenger rejected for {}: median {}ms (need < {}ms)",
                        account_id, median_ms, current_worst - margin);
                }
            }
        }

        // ── evict ──

        {
            let mut state = arc.lock().await;
            let before = state.active.idle_conns.len();
            state.active.idle_conns.retain(|conn| {
                !conn.is_closed() && !conn.is_expired(self.config.max_lifetime)
            });
            let evicted = before - state.active.idle_conns.len();
            if evicted > 0 {
                tracing::debug!("[POOL] evicted {} idle conns for {}", evicted, account_id);
            }
        }
    }

    /// Switch to best standby session. Per-account switch lock prevents concurrency.
    pub async fn try_switch_to_standby(self: &Arc<Self>, account_id: &str) {
        let switch_lock = self.switch_locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone();
        let _switch_guard = switch_lock.lock().await;

        let arc = match self.accounts.get(account_id).map(|r| r.clone()) {
            Some(a) => a,
            None => return,
        };
        let state = arc.lock().await;

        if state.standby.is_empty() {
            let country = self.geo_policy.expected_country.clone();
            drop(state);

            match self.allocator.allocate_ephemeral(&country).await {
                Ok(session) => {
                    let proxy_url = self.allocator.build_proxy_url_with_country(
                        &session.session_id, &country,
                    );
                    if let Err(e) = self.allocator.commit_session(account_id, &session).await {
                        tracing::error!("[POOL] commit_session failed for {}: {}", account_id, e);
                        return;
                    }
                    let mut state = arc.lock().await;
                    state.active = ActiveSession {
                        session, proxy_url,
                        idle_conns: vec![],
                        checked_out: state.active.checked_out,
                    };
                    state.stats.clear();
                    state.failure_streak = 0;
                    tracing::info!("[POOL] switched {} to new session (no standby)", account_id);
                }
                Err(e) => {
                    tracing::error!("[POOL] fallback allocate failed for {}: {}", account_id, e);
                }
            }
            return;
        }

        // Remember session_id to commit (not index-dependent)
        let best_session = state.standby[0].session.clone();
        let best_session_id = best_session.session_id.clone();
        drop(state);

        if let Err(e) = self.allocator.commit_session(account_id, &best_session).await {
            tracing::error!("[POOL] commit_session failed for {}: {}", account_id, e);
            return;
        }

        // Locate by session_id precisely (P1-1: standby may change during await window)
        let mut state = arc.lock().await;
        let idx = state.standby.iter().position(|s| s.session.session_id == best_session_id);
        match idx {
            Some(i) => {
                let best = state.standby.remove(i);
                let old_checked_out = state.active.checked_out;
                state.active = ActiveSession {
                    session: best.session,
                    proxy_url: best.proxy_url,
                    idle_conns: best.warm_conn.into_iter().collect(),
                    checked_out: old_checked_out,
                };
                state.stats.clear();
                state.failure_streak = 0;
                tracing::info!("[POOL] switched {} to standby ({}ms)", account_id, best.avg_connect_ms);
            }
            None => {
                // Standby was cleared or changed during await (disable / another switch)
                // Use the already-committed session directly, but without warm_conn
                let old_checked_out = state.active.checked_out;
                let proxy_url = self.allocator.build_proxy_url_with_country(
                    &best_session_id, &self.geo_policy.expected_country,
                );
                state.active = ActiveSession {
                    session: best_session,
                    proxy_url,
                    idle_conns: vec![],
                    checked_out: old_checked_out,
                };
                state.stats.clear();
                state.failure_streak = 0;
                tracing::warn!("[POOL] switched {} to committed session (standby gone, no warm_conn)", account_id);
            }
        }
    }

    /// Called by handler postcheck: force proxy switch.
    pub async fn renew_proxy(self: &Arc<Self>, account_id: &str) {
        self.try_switch_to_standby(account_id).await;
    }

    /// Get the active session's proxy_url for an account.
    /// Used for non-api.anthropic.com requests (e.g. platform.claude.com token refresh).
    /// Internally auto-calls ensure_proxy — first call allocates session + starts maintain loop.
    pub async fn get_active_proxy_url(self: &Arc<Self>, account_id: &str) -> Result<String, ProxyError> {
        self.ensure_proxy(account_id).await?;
        let arc = self.accounts.get(account_id)
            .map(|r| r.clone())
            .ok_or_else(|| ProxyError::Exhausted("account not in pool after ensure".to_string()))?;
        let state = arc.lock().await;
        Ok(state.active.proxy_url.clone())
    }
}

// ─── ProxyClient ───────────────────────────────────────────

/// Per-account routed HTTP client. Handler sends requests through this.
#[derive(Clone)]
pub struct ProxyClient {
    pool: Arc<ProxyPool>,
    account_id: String,
}

impl ProxyClient {
    pub fn post(&self, url: &str) -> ProxyRequestBuilder {
        ProxyRequestBuilder {
            pool: self.pool.clone(),
            account_id: self.account_id.clone(),
            method: http::Method::POST,
            url: url.to_string(),
            headers: http::HeaderMap::new(),
            body: Bytes::new(),
            cc_version: None,
        }
    }

    pub fn get(&self, url: &str) -> ProxyRequestBuilder {
        ProxyRequestBuilder {
            pool: self.pool.clone(),
            account_id: self.account_id.clone(),
            method: http::Method::GET,
            url: url.to_string(),
            headers: http::HeaderMap::new(),
            body: Bytes::new(),
            cc_version: None,
        }
    }
}

impl ProxyPool {
    /// Create a ProxyClient bound to account_id.
    pub fn client(self: &Arc<Self>, account_id: &str) -> ProxyClient {
        ProxyClient {
            pool: self.clone(),
            account_id: account_id.to_string(),
        }
    }
}

// ─── ProxyRequestBuilder ───────────────────────────────────

pub struct ProxyRequestBuilder {
    pool: Arc<ProxyPool>,
    account_id: String,
    method: http::Method,
    url: String,
    headers: http::HeaderMap,
    body: Bytes,
    cc_version: Option<semver::Version>,
}

impl ProxyRequestBuilder {
    pub fn headers(mut self, h: http::HeaderMap) -> Self {
        self.headers = h;
        self
    }

    pub fn body(mut self, b: Bytes) -> Self {
        self.body = b;
        self
    }

    pub fn cc_cli_version(mut self, version: &semver::Version) -> Self {
        self.cc_version = Some(version.clone());
        self
    }

    pub async fn send(mut self) -> Result<ProxyResponse, ProxyError> {
        let acq = self.pool.acquire(&self.account_id).await?;
        let AcquireResult { mut conn, session_id: _, is_pooled } = acq;

        let uri: hyper::Uri = self.url.parse()
            .map_err(|e| ProxyError::Request(format!("invalid URL: {}", e)))?;

        // Auto-fill Host header if absent. Mirror BoringClient::RequestBuilder's
        // behavior exactly, including `host:port` for non-default ports.
        if !self.headers.contains_key(http::header::HOST) {
            if let Some(host) = uri.host() {
                let host_value = if let Some(port) = uri.port() {
                    format!("{}:{}", host, port)
                } else {
                    host.to_string()
                };
                if let Ok(v) = host_value.parse() {
                    self.headers.insert(http::header::HOST, v);
                }
            }
        }

        let result = self.pool.boring_client.send_on_sender(
            &mut conn.sender,
            self.method,
            uri,
            self.headers,
            self.body,
            self.cc_version.as_ref(),
        ).await;

        match result {
            Ok(raw_resp) => {
                let decoded = claude_ultra_http::decode_response(raw_resp);
                let (parts, body) = decoded.into_parts();

                let wrapped = WrappedBody {
                    inner: body,
                    release_tx: self.pool.release_tx.read().unwrap().clone(),
                    conn_info: Some(ConnReleaseInfo {
                        account_id: self.account_id.clone(),
                        conn: Some(conn),
                        is_pooled,
                    }),
                    body_complete: false,
                };

                Ok(ProxyResponse {
                    status: parts.status,
                    headers: parts.headers,
                    body: wrapped,
                })
            }
            Err(e) => {
                // Send failed, release checked_out.
                let tx = self.pool.release_tx.read().unwrap().clone();
                if let Err(send_err) = tx.send(ReleaseMsg::Discard {
                    account_id: self.account_id.clone(),
                    is_pooled,
                }).await {
                    tracing::error!("[POOL] release channel send failed: {}", send_err);
                }
                // Trigger standby switch (P2-4: per design doc)
                self.pool.try_switch_to_standby(&self.account_id).await;
                Err(ProxyError::ConnectionFailed(format!("send_on_sender: {}", e)))
            }
        }
    }
}

// ─── WrappedBody ───────────────────────────────────────────

use std::pin::Pin;
use std::task::{Context, Poll};
use claude_ultra_http::{DecodedBody, DecodeError};

struct ConnReleaseInfo {
    account_id: String,
    conn: Option<ProxyConn>,
    is_pooled: bool,
}

/// Wraps DecodedBody + connection release logic.
/// Automatically releases connection when body is consumed or dropped.
#[pin_project::pin_project(PinnedDrop)]
pub struct WrappedBody {
    #[pin]
    inner: DecodedBody,
    release_tx: mpsc::Sender<ReleaseMsg>,
    conn_info: Option<ConnReleaseInfo>,
    body_complete: bool,
}

impl http_body::Body for WrappedBody {
    type Data = Bytes;
    type Error = DecodeError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.project();

        match this.inner.poll_frame(cx) {
            Poll::Ready(None) => {
                *this.body_complete = true;
                Poll::Ready(None)
            }
            other => other,
        }
    }
}

#[pin_project::pinned_drop]
impl PinnedDrop for WrappedBody {
    fn drop(self: Pin<&mut Self>) {
        let this = self.project();
        if let Some(mut info) = this.conn_info.take() {
            if *this.body_complete {
                if let Some(conn) = info.conn.take() {
                    if let Err(e) = this.release_tx.try_send(ReleaseMsg::Return {
                        account_id: info.account_id,
                        conn,
                        is_pooled: info.is_pooled,
                    }) {
                        tracing::error!("[POOL] release channel full on Return, checked_out may leak: {}", e);
                    }
                }
            } else {
                if let Err(e) = this.release_tx.try_send(ReleaseMsg::Discard {
                    account_id: info.account_id,
                    is_pooled: info.is_pooled,
                }) {
                    tracing::error!("[POOL] release channel full on Discard, checked_out may leak: {}", e);
                }
            }
        }
    }
}

// ─── ProxyResponse ─────────────────────────────────────────

/// Response from ProxyPool. Connection auto-released when body is consumed or dropped.
pub struct ProxyResponse {
    status: http::StatusCode,
    headers: http::HeaderMap,
    body: WrappedBody,
}

impl ProxyResponse {
    pub fn status(&self) -> http::StatusCode {
        self.status
    }

    pub fn headers(&self) -> &http::HeaderMap {
        &self.headers
    }

    /// Consume response, return body for streaming reads.
    pub fn into_body(self) -> WrappedBody {
        self.body
    }

    /// Convert to standard http::Response (used by handler generic functions).
    pub fn into_http_response(self) -> http::Response<WrappedBody> {
        let mut resp = http::Response::new(self.body);
        *resp.status_mut() = self.status;
        *resp.headers_mut() = self.headers;
        resp
    }

    /// Read entire body at once.
    pub async fn bytes(self) -> Result<Bytes, ProxyError> {
        use http_body_util::BodyExt;
        let body = self.body;
        let collected = body.collect().await
            .map_err(|e| ProxyError::ConnectionFailed(format!("read body: {}", e)))?;
        Ok(collected.to_bytes())
    }
}

// ─── Tests ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use crate::modules::account_manager::AccountManager;
    use crate::proxy::allocator::ProxyAllocator;
    use crate::proxy::config::ProxyProviderConfig;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "claude_ultra_pool_test_{}",
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

    fn make_test_pool(dir: &Path) -> Arc<ProxyPool> {
        let am = Arc::new(AccountManager::new(dir.to_path_buf()));
        let config = Arc::new(make_provider_config());
        let allocator = Arc::new(ProxyAllocator::new(am, config));
        let boring = Arc::new(BoringClient::builder().build().unwrap());
        let cancel = CancellationToken::new();
        ProxyPool::new(
            PoolConfig::default(),
            boring,
            allocator,
            GeoPolicy { expected_country: "US".to_string() },
            cancel,
        )
    }

    fn make_test_proxy_section(session_id: &str) -> ProxySection {
        ProxySection {
            session_id: session_id.to_string(),
            host: "test:1234".to_string(),
            last_ip: None, country: None, region: None, city: None,
            isp: None, quality: None, last_checked: None,
            proxy_type: None, lifetime: None, created_at: None,
        }
    }

    // ── PoolConfig ──────────────────────────────────────────

    #[test]
    fn test_pool_config_defaults() {
        let config = PoolConfig::default();
        assert_eq!(config.max_conns_per_account, 4);
        assert_eq!(config.max_lifetime, Duration::from_secs(900));
        assert_eq!(config.max_standby_per_account, 3);
        assert_eq!(config.slow_threshold_ms, 2500);
        assert_eq!(config.significance_margin_ms, 100);
    }

    // ── ProxyError ──────────────────────────────────────────

    #[test]
    fn test_proxy_error_display() {
        let e = ProxyError::ConnectionFailed("timeout".to_string());
        assert!(e.to_string().contains("timeout"));

        let e = ProxyError::Exhausted("no standby".to_string());
        assert!(e.to_string().contains("exhausted"));

        let e = ProxyError::Upstream(http::StatusCode::TOO_MANY_REQUESTS, "rate limited".to_string());
        assert!(e.to_string().contains("429"));
    }

    // ── AccountProxy ────────────────────────────────────────

    #[test]
    fn test_account_proxy_initial_state() {
        let session = make_test_proxy_section("abc123");
        let account = AccountProxy {
            active: ActiveSession {
                session,
                proxy_url: "http://test".to_string(),
                idle_conns: vec![],
                checked_out: 0,
            },
            standby: vec![],
            disabled: false,
            stats: VecDeque::new(),
            avg_connect_ms: 0.0,
            failure_streak: 0,
            last_active_at: Instant::now(),
        };
        assert_eq!(account.active.idle_conns.len(), 0);
        assert_eq!(account.active.checked_out, 0);
        assert_eq!(account.standby.len(), 0);
    }

    // ── disable_in_pool / enable_in_pool ──────────────────

    #[tokio::test]
    async fn test_disable_in_pool() {
        let dir = TestDir::new();
        let pool = make_test_pool(&dir.path);

        let session = make_test_proxy_section("sess1");
        pool.accounts.insert("a1".to_string(), Arc::new(TokioMutex::new(
            AccountProxy {
                active: ActiveSession {
                    session, proxy_url: "http://test".to_string(),
                    idle_conns: vec![], checked_out: 0,
                },
                standby: vec![], disabled: false,
                stats: VecDeque::new(), avg_connect_ms: 0.0,
                failure_streak: 0, last_active_at: Instant::now(),
            }
        )));

        // After disable, AccountProxy remains in DashMap
        pool.disable_in_pool("a1").await;
        assert_eq!(pool.accounts.len(), 1, "should NOT remove from DashMap");

        let arc = pool.accounts.get("a1").unwrap().clone();
        let state = arc.lock().await;
        assert!(state.disabled);
        assert!(state.active.idle_conns.is_empty());
        assert!(state.standby.is_empty());
    }

    #[tokio::test]
    async fn test_enable_after_disable() {
        let dir = TestDir::new();
        let pool = make_test_pool(&dir.path);

        let session = make_test_proxy_section("sess1");
        pool.accounts.insert("a1".to_string(), Arc::new(TokioMutex::new(
            AccountProxy {
                active: ActiveSession {
                    session, proxy_url: "http://test".to_string(),
                    idle_conns: vec![], checked_out: 0,
                },
                standby: vec![], disabled: false,
                stats: VecDeque::new(), avg_connect_ms: 0.0,
                failure_streak: 0, last_active_at: Instant::now(),
            }
        )));

        pool.disable_in_pool("a1").await;
        pool.enable_in_pool("a1").await;

        let arc = pool.accounts.get("a1").unwrap().clone();
        let state = arc.lock().await;
        assert!(!state.disabled);
    }

    // ── release consumer ────────────────────────────────────

    #[tokio::test]
    async fn test_release_discard_decrements_checked_out() {
        let dir = TestDir::new();
        let pool = make_test_pool(&dir.path);

        let session = make_test_proxy_section("sess1");
        pool.accounts.insert("a1".to_string(), Arc::new(TokioMutex::new(
            AccountProxy {
                active: ActiveSession {
                    session, proxy_url: "http://test".to_string(),
                    idle_conns: vec![], checked_out: 3,
                },
                standby: vec![], disabled: false,
                stats: VecDeque::new(), avg_connect_ms: 0.0,
                failure_streak: 0, last_active_at: Instant::now(),
            }
        )));

        pool.release_tx.read().unwrap().send(ReleaseMsg::Discard {
            account_id: "a1".to_string(),
            is_pooled: true,
        }).await.unwrap();

        pool.start_release_consumer();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let arc = pool.accounts.get("a1").unwrap().clone();
        let state = arc.lock().await;
        assert_eq!(state.active.checked_out, 2);
    }

    #[tokio::test]
    async fn test_release_discard_overflow_no_decrement() {
        let dir = TestDir::new();
        let pool = make_test_pool(&dir.path);

        let session = make_test_proxy_section("sess1");
        pool.accounts.insert("a1".to_string(), Arc::new(TokioMutex::new(
            AccountProxy {
                active: ActiveSession {
                    session, proxy_url: "http://test".to_string(),
                    idle_conns: vec![], checked_out: 2,
                },
                standby: vec![], disabled: false,
                stats: VecDeque::new(), avg_connect_ms: 0.0,
                failure_streak: 0, last_active_at: Instant::now(),
            }
        )));

        pool.release_tx.read().unwrap().send(ReleaseMsg::Discard {
            account_id: "a1".to_string(),
            is_pooled: false,
        }).await.unwrap();

        pool.start_release_consumer();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let arc = pool.accounts.get("a1").unwrap().clone();
        let state = arc.lock().await;
        assert_eq!(state.active.checked_out, 2, "overflow discard should not decrement");
    }

    #[tokio::test]
    async fn test_release_after_remove_ignored() {
        let dir = TestDir::new();
        let pool = make_test_pool(&dir.path);

        // Don't insert any accounts, messages should be safely ignored
        pool.release_tx.read().unwrap().send(ReleaseMsg::Discard {
            account_id: "nonexistent".to_string(),
            is_pooled: true,
        }).await.unwrap();

        pool.start_release_consumer();
        tokio::time::sleep(Duration::from_millis(50)).await;
        // No panic = pass
    }

    // ── Lifecycle: start → stop → restart ───────────────────

    #[tokio::test]
    async fn test_pool_start_stop_restart() {
        let dir = TestDir::new();
        let pool = make_test_pool(&dir.path);

        // First start
        pool.start_background_tasks();
        assert!(pool.background_started.load(std::sync::atomic::Ordering::SeqCst));

        // Duplicate start should be no-op
        pool.start_background_tasks();

        // Insert an account before stop
        let pre_session = make_test_proxy_section("pre_stop");
        pool.accounts.insert("pre".to_string(), Arc::new(TokioMutex::new(
            AccountProxy {
                active: ActiveSession {
                    session: pre_session, proxy_url: "http://test".to_string(),
                    idle_conns: vec![], checked_out: 0,
                },
                standby: vec![], disabled: false,
                stats: VecDeque::new(), avg_connect_ms: 0.0,
                failure_streak: 0, last_active_at: Instant::now(),
            }
        )));
        assert_eq!(pool.accounts.len(), 1);

        // Stop — should clear accounts
        pool.stop();
        assert!(!pool.background_started.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(pool.accounts.len(), 0, "stop must clear accounts so maintain loops restart on re-init");

        // Wait for tasks to exit
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Restart after stop
        pool.start_background_tasks();
        assert!(pool.background_started.load(std::sync::atomic::Ordering::SeqCst));

        // Verify release channel works after restart
        let session = make_test_proxy_section("sess1");
        pool.accounts.insert("a1".to_string(), Arc::new(TokioMutex::new(
            AccountProxy {
                active: ActiveSession {
                    session, proxy_url: "http://test".to_string(),
                    idle_conns: vec![], checked_out: 3,
                },
                standby: vec![], disabled: false,
                stats: VecDeque::new(), avg_connect_ms: 0.0,
                failure_streak: 0, last_active_at: Instant::now(),
            }
        )));

        pool.release_tx.read().unwrap().send(ReleaseMsg::Discard {
            account_id: "a1".to_string(),
            is_pooled: true,
        }).await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let arc = pool.accounts.get("a1").unwrap().clone();
        let state = arc.lock().await;
        assert_eq!(state.active.checked_out, 2, "release consumer should process after restart");
    }

    #[tokio::test]
    async fn test_pool_stop_cancels_maintain_loop() {
        let dir = TestDir::new();
        let pool = make_test_pool(&dir.path);

        pool.start_background_tasks();

        // Spawn a maintain loop manually
        let cancel = pool.cancel_token.read().unwrap().clone();
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let exited_clone = exited.clone();
        tokio::spawn(async move {
            cancel.cancelled().await;
            exited_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        // Stop should cancel the token
        pool.stop();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(exited.load(std::sync::atomic::Ordering::SeqCst), "maintain loop should exit after stop");
    }

    #[tokio::test]
    async fn test_pool_stop_then_new_token_not_cancelled() {
        let dir = TestDir::new();
        let pool = make_test_pool(&dir.path);

        pool.start_background_tasks();
        pool.stop();

        // New token after stop should NOT be cancelled
        let new_token = pool.cancel_token.read().unwrap().clone();
        assert!(!new_token.is_cancelled(), "fresh token after stop should not be cancelled");
    }
}
