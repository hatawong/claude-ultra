use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;

use claude_ultra_http::BoringClient;
use crate::modules::account_manager::AccountManager;
use crate::modules::client_manager::ClientManager;
use super::auth::{auth_middleware, ProxyApiKey};
use super::config::GatewayConfig;
use crate::proxy::config::ProxyProviderConfig;
use crate::proxy::pool::ProxyPool;
use super::handler::{self, AppState};
use crate::proxy::allocator::ProxyAllocator;
use super::security::{ip_check_middleware, SecurityState};

/// A running gateway server instance.
pub struct GatewayInstance {
    pub port: u16,
    pub cancel_token: CancellationToken,
    pub server_handle: JoinHandle<()>,
}

impl GatewayInstance {
    pub async fn stop(self) {
        self.cancel_token.cancel();
        let _ = self.server_handle.await;
    }
}

/// Start the gateway server with the given config and token manager.
/// Optionally accepts AccountManager and ProxyAllocator for proxy management.
pub async fn start_gateway_server(
    config: &GatewayConfig,
    client_manager: Arc<ClientManager>,
) -> Result<GatewayInstance, String> {
    start_gateway_server_with_proxy(config, client_manager, None, None, None, None, None, None, None, None, crate::models::config::ProxyMode::Direct).await
}

/// Start the gateway server with full proxy management support.
pub async fn start_gateway_server_with_proxy(
    config: &GatewayConfig,
    client_manager: Arc<ClientManager>,
    account_manager: Option<Arc<AccountManager>>,
    proxy_allocator: Option<Arc<ProxyAllocator>>,
    proxy_provider_config: Option<Arc<ProxyProviderConfig>>,
    proxy_pool: Option<Arc<ProxyPool>>,
    security_state: Option<SecurityState>,
    token_allocator: Option<Arc<crate::modules::token_allocator::TokenAllocator>>,
    enable_logging: Option<Arc<std::sync::atomic::AtomicBool>>,
    gateway_db: Option<Arc<crate::modules::gateway_db::GatewayDb>>,
    proxy_mode: crate::models::config::ProxyMode,
) -> Result<GatewayInstance, String> {
    let client = Arc::new(
        BoringClient::builder()
            .timeout(Duration::from_secs(config.request_timeout))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("Failed to build BoringClient"),
    );
    // Create a fresh cancellation token per server instance (not reused from client_manager)
    let cancel_token = CancellationToken::new();

    let has_proxy = proxy_provider_config.as_ref()
        .map(|c| !c.username.is_empty() && !c.password.is_empty())
        .unwrap_or(false);

    let app_state = AppState {
        client_manager: client_manager.clone(),
        client,
        max_retries: config.max_retries,
        account_manager,
        proxy_allocator,
        proxy_provider_config,
        proxy_pool,
        token_allocator,
        enable_logging: enable_logging.unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicBool::new(config.enable_logging))),
        quota_last_persisted: Arc::new(dashmap::DashMap::new()),
        gateway_db,
        proxy_mode,
        upstream_base_url: config.upstream_base_url.clone(),
        vercel_api_key: config.vercel_api_key.clone(),
        has_proxy,
        vercel_proxy_url: config.vercel_proxy_url.clone(),
    };

    let api_key = config.api_key.clone();

    // Build router with auth + security middleware
    // Layer order (innermost runs first): handler → auth → ip_check
    let mut app = Router::new()
        .route("/health", get(handler::health_check))
        .route("/v1/messages", post(handler::handle_messages))
        .route(
            "/v1/messages/count_tokens",
            post(handler::handle_count_tokens),
        )
        // Inject ProxyApiKey extension + auth middleware
        .layer(middleware::from_fn(auth_middleware))
        .layer(axum::Extension(ProxyApiKey(api_key)));

    // IP check middleware (runs BEFORE auth — outer layer)
    if let Some(sec_state) = security_state {
        app = app
            .layer(middleware::from_fn(ip_check_middleware))
            .layer(axum::Extension(sec_state));
    }

    let app = app
        // 100MB body limit (Anthropic requests can include images)
        .layer(RequestBodyLimitLayer::new(100 * 1024 * 1024))
        .with_state(app_state);

    let addr = format!("{}:{}", config.bind_address, config.port);
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("Failed to bind {}: {}", addr, e))?;

    let port = listener.local_addr().map(|a| a.port()).unwrap_or(config.port);
    let bind_addr = config.bind_address.clone();

    let cancel = cancel_token.clone();
    let server_handle = tokio::spawn(async move {
        tracing::info!("Gateway server listening on {}:{}", bind_addr, port);
        let serve = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        );
        tokio::select! {
            result = serve => {
                if let Err(e) = result {
                    tracing::error!("Gateway server error: {}", e);
                }
            }
            _ = cancel.cancelled() => {
                tracing::info!("Gateway server shutting down");
            }
        }
    });

    Ok(GatewayInstance {
        port,
        cancel_token,
        server_handle,
    })
}

/// Start the transparent audit gateway server.
///
/// Internal-only (requires `internal` feature). Binds to `127.0.0.1:<port>` (no
/// LAN exposure), no auth, no account selection, no body rewrites — forwards
/// inbound requests as-is to `api.anthropic.com` using the caller's
/// Authorization. Shares gateway_db with the main server for unified logging.
///
/// # Security considerations
///
/// * **No authentication** — any local process can reach the port and issue
///   upstream requests with the caller-supplied Authorization. Bind is
///   pinned to the loopback interface and is not affected by the LAN
///   sharing toggle.
/// * **External tunnels bypass the loopback guarantee.** `ssh -L` forwards,
///   `docker run -p 127.0.0.1:<port>:<port>`, ngrok, or similar tools can
///   expose this port publicly. Do not run those alongside the transparent
///   port on shared machines.
/// * **Any HTTP client pointed at the port will have its request logged**
///   under a pseudo account (`transparent@<client_ip>`) and its own token
///   used upstream — the billing lands on that caller's account, not the
///   gateway pool.
#[cfg(feature = "internal")]
pub async fn start_transparent_server(
    port: u16,
    request_timeout: u64,
    enable_logging: Arc<std::sync::atomic::AtomicBool>,
    gateway_db: Option<Arc<crate::modules::gateway_db::GatewayDb>>,
) -> Result<GatewayInstance, String> {
    let client = Arc::new(
        BoringClient::builder()
            .timeout(Duration::from_secs(request_timeout))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("Failed to build BoringClient"),
    );
    let cancel_token = CancellationToken::new();

    // Minimal AppState for transparent path — handler only reads `client`,
    // `enable_logging`, and `gateway_db`. Other fields use sensible defaults
    // (empty ClientManager, no proxy, no account_manager, etc).
    let placeholder_client_manager = Arc::new(ClientManager::new(client.clone()));
    let app_state = AppState {
        client_manager: placeholder_client_manager,
        client,
        max_retries: 0,
        account_manager: None,
        proxy_allocator: None,
        proxy_provider_config: None,
        proxy_pool: None,
        token_allocator: None,
        enable_logging,
        quota_last_persisted: Arc::new(dashmap::DashMap::new()),
        gateway_db,
        proxy_mode: crate::models::config::ProxyMode::Direct,
        upstream_base_url: "https://api.anthropic.com".to_string(),
        vercel_api_key: String::new(),
        has_proxy: false,
        vercel_proxy_url: None,
    };

    let app = Router::new()
        .route("/health", get(handler::health_check))
        .route("/v1/messages", post(handler::handle_transparent_messages))
        .route(
            "/v1/messages/count_tokens",
            post(handler::handle_transparent_count_tokens),
        )
        .layer(RequestBodyLimitLayer::new(100 * 1024 * 1024))
        .with_state(app_state);

    // Bind hardcoded to localhost — no LAN exposure regardless of main port's
    // bind_address (LAN sharing toggle). User cannot configure this address.
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("Failed to bind {}: {}", addr, e))?;

    let bound_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);

    let cancel = cancel_token.clone();
    let server_handle = tokio::spawn(async move {
        tracing::info!("Transparent audit server listening on 127.0.0.1:{}", bound_port);
        let serve = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        );
        tokio::select! {
            result = serve => {
                if let Err(e) = result {
                    tracing::error!("Transparent server error: {}", e);
                }
            }
            _ = cancel.cancelled() => {
                tracing::info!("Transparent server shutting down");
            }
        }
    });

    Ok(GatewayInstance {
        port: bound_port,
        cancel_token,
        server_handle,
    })
}

/// Gateway status for IPC query.
#[derive(serde::Serialize)]
pub struct GatewayStatus {
    pub running: bool,
    pub port: u16,
    pub active_accounts: usize,
    pub total_accounts: usize,
}

#[cfg(all(test, feature = "internal"))]
mod transparent_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn make_logging_flag() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    /// Pick an ephemeral free port by binding :0 and releasing it.
    async fn pick_free_port() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    }

    #[tokio::test]
    async fn test_transparent_server_binds_and_stops() {
        let port = pick_free_port().await;
        let inst = start_transparent_server(port, 30, make_logging_flag(), None)
            .await
            .expect("should bind free port");
        assert_eq!(inst.port, port);
        inst.stop().await;
    }

    #[tokio::test]
    async fn test_transparent_server_bind_failure_returns_err() {
        // Occupy a port, then attempt to bind again — second call must fail.
        let port = pick_free_port().await;
        let _occupier = TcpListener::bind(format!("127.0.0.1:{}", port)).await.unwrap();
        let err = start_transparent_server(port, 30, make_logging_flag(), None)
            .await
            .err()
            .expect("bind on occupied port should fail");
        assert!(err.contains("Failed to bind"), "unexpected error: {}", err);
    }
}
