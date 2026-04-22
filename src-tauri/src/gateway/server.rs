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

/// Gateway status for IPC query.
#[derive(serde::Serialize)]
pub struct GatewayStatus {
    pub running: bool,
    pub port: u16,
    pub active_accounts: usize,
    pub total_accounts: usize,
}
