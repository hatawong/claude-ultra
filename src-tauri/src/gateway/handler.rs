use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::net::SocketAddr;
use bytes::Bytes;
use http_body_util::BodyExt;
use std::collections::HashSet;
use std::sync::Arc;
use dashmap::DashMap;
use std::time::Duration;

use claude_ultra_http::BoringClient;
use crate::modules::account_manager::AccountManager;
use crate::modules::billing::{self, TokenUsage};
use crate::modules::client_manager::ClientManager;
use crate::modules::token_allocator::TokenAllocator;
use crate::proxy::config::ProxyProviderConfig;
use crate::proxy::pool::{ProxyPool, ProxyError};
use super::builder::{self as builder, RequestContext};
use crate::modules::gateway_db;
use crate::proxy::allocator::ProxyAllocator;
use crate::models::quota;
use crate::gateway::route::ActualRoute;

/// First-byte timeout for SSE streams (30 seconds).
const FIRST_BYTE_TIMEOUT_SECS: u64 = 30;

/// Quota persist throttle: minimum interval between disk writes per account.
const QUOTA_PERSIST_INTERVAL_SECS: u64 = 60;

/// Shared application state for all handlers.
#[derive(Clone)]
pub struct AppState {
    pub client_manager: Arc<ClientManager>,
    pub client: Arc<BoringClient>,
    pub max_retries: u32,
    pub account_manager: Option<Arc<AccountManager>>,
    pub proxy_allocator: Option<Arc<ProxyAllocator>>,
    pub proxy_provider_config: Option<Arc<ProxyProviderConfig>>,
    pub proxy_pool: Option<Arc<ProxyPool>>,
    pub token_allocator: Option<Arc<TokenAllocator>>,
    pub enable_logging: Arc<std::sync::atomic::AtomicBool>,
    /// Throttle: last quota persist time per account (epoch ms).
    pub quota_last_persisted: Arc<DashMap<String, i64>>,
    pub gateway_db: Option<Arc<crate::modules::gateway_db::GatewayDb>>,
    /// Proxy mode: Proxied (ProxyPool) or Direct (BoringClient direct).
    pub proxy_mode: crate::models::config::ProxyMode,
    pub upstream_base_url: String,
    pub vercel_api_key: String,
    pub has_proxy: bool,
    pub vercel_proxy_url: Option<String>,
}

/// GET /health — always returns 200 OK.
pub async fn health_check() -> impl IntoResponse {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

/// POST /v1/messages — core gateway handler with SSE passthrough.
pub async fn handle_messages(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    gateway_request(state, request, "/v1/messages", true, Some(addr)).await
}

/// POST /v1/messages/count_tokens — non-SSE JSON forwarding.
pub async fn handle_count_tokens(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    gateway_request(state, request, "/v1/messages/count_tokens", false, Some(addr)).await
}

#[cfg(feature = "internal")]
pub async fn handle_transparent_messages(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    transparent_entry(state, request, "/v1/messages", true, Some(addr)).await
}

#[cfg(feature = "internal")]
pub async fn handle_transparent_count_tokens(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    transparent_entry(state, request, "/v1/messages/count_tokens", false, Some(addr)).await
}

#[cfg(feature = "internal")]
async fn transparent_entry(
    state: AppState,
    request: Request<Body>,
    path: &str,
    endpoint_allows_sse: bool,
    client_addr: Option<SocketAddr>,
) -> Response {
    let start_time = std::time::Instant::now();
    let enable_logging = state.enable_logging.load(std::sync::atomic::Ordering::Relaxed);
    let (parts, body) = request.into_parts();
    let client_ip = client_addr.map(|a| a.ip().to_string());
    let user_agent = parts.headers.get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let body_bytes = match axum::body::to_bytes(body, 100 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("[FAIL] path={} body read: {} (transparent)", path, e);
            return (StatusCode::BAD_REQUEST, format!("body: {}", e)).into_response();
        }
    };
    transparent_forward(
        state, parts, body_bytes, path,
        endpoint_allows_sse, enable_logging,
        client_ip, user_agent, start_time,
    ).await
}


/// Core gateway logic with three-layer retry:
/// - Outer: account failover
/// - Middle: same-account retry
/// - Inner: proxy renew
///
/// Includes: Stream Written Guard, first-byte timeout, 401 auto-refresh,
/// 429 TempUnschedule, selection exhaustion backoff, context cancellation.
#[allow(unused_assignments)]
async fn gateway_request(
    state: AppState,
    request: Request<Body>,
    path: &str,
    // endpoint_allows_sse: whether this endpoint can ever return SSE.
    // Actual is_sse is determined by intersecting this with the request body's `stream` field.
    endpoint_allows_sse: bool,
    client_addr: Option<SocketAddr>,
) -> Response {
    let start_time = std::time::Instant::now();
    let enable_logging = state.enable_logging.load(std::sync::atomic::Ordering::Relaxed);
    let (mut parts, body) = request.into_parts();
    let client_ip = client_addr.map(|a| a.ip().to_string());
    let user_agent = parts.headers.get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let body_bytes = match axum::body::to_bytes(body, 100 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("Failed to read request body: {}", e);
            return (StatusCode::BAD_REQUEST, format!("Failed to read body: {}", e)).into_response();
        }
    };

    // Version gate — Err returns 400 immediately, no retry / no account state.
    let prepared = match super::policy::gate(&body_bytes, user_agent.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("policy rejected: {:?}", e);
            let body = serde_json::json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": format!("[Claude Ultra] request check failed: {}.", e)
                }
            })
            .to_string();
            return axum::response::Response::builder()
                .status(400)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body))
                .unwrap();
        }
    };
    // Apply UA override if version was adjusted.
    if let Some(new_ua) = prepared.ua_override.as_deref() {
        tracing::warn!(
            client_version = %prepared.version,
            "policy: version adjusted"
        );
        let ua: http::HeaderValue = new_ua
            .parse()
            .expect("ua_override must be a valid HeaderValue");
        parts.headers.insert("user-agent", ua);
    }
    // Read structured fields directly from the parsed Value (no extra parse of bytes).
    let model = prepared
        .value
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Determine actual streaming behavior:
    //   - If endpoint doesn't allow SSE (e.g. count_tokens) → always JSON
    //   - Otherwise, honor client's `stream` field from the request body (default false per Anthropic spec)
    let client_requested_stream = prepared
        .value
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let is_sse = endpoint_allows_sse && client_requested_stream;
    // Inbound body size (before Gateway rewrites). Used for request logging.
    let request_size = body_bytes.len() as u64;

    // Extract session_id from metadata.user_id for session affinity (directly from parsed Value).
    let session_id: Option<String> = prepared
        .value
        .get("metadata")
        .and_then(|m| m.get("user_id"))
        .and_then(|u| u.as_str())
        .and_then(|uid_str| serde_json::from_str::<serde_json::Value>(uid_str).ok())
        .and_then(|uid| uid.get("session_id").and_then(|s| s.as_str().map(String::from)));

    let mut attempted: HashSet<String> = HashSet::new();
    let mut switch_count: u32 = 0;
    let mut exhausted_retry_done = false;
    let mut last_error_recoverable = false;
    // Stable log ID for this request — survives retries
    let stable_log_id = uuid::Uuid::new_v4().to_string();
    let mut log_created = false;

    // Outer loop: account failover
    loop {
        if switch_count > state.max_retries {
            tracing::warn!("[HANDLER] max retries ({}) exhausted, giving up", state.max_retries);
            break;
        }

        // Failover delay (incremental)
        if switch_count > 0 {
            let delay = failover_delay(switch_count);
            tracing::info!("[HANDLER] failover #{}, delay={}ms", switch_count, delay.as_millis());
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }

        // Select client (session affinity → model rate limit → watermark → round-robin)
        let cli = match state.client_manager.get_client_with_session(
            session_id.as_deref(), &attempted, Some(&model),
        ) {
            Some(c) => c,
            None => {
                // Selection exhausted — try backoff once
                if last_error_recoverable && !exhausted_retry_done {
                    tracing::warn!("[HANDLER] selection exhausted but recoverable, backoff 2s and retry");
                    exhausted_retry_done = true;
                    attempted.clear();
                    tokio::time::sleep(Duration::from_millis(2000)).await;
                    continue;
                }
                tracing::error!("[HANDLER] no available accounts, returning 429 (attempted={:?})", attempted);
                let body = serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "rate_limit_error",
                        "message": "[Claude Ultra] All accounts have exceeded quota limits. Please wait for quota reset.",
                    }
                });
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    axum::Json(body),
                )
                    .into_response();
            }
        };

        let account_id = cli.account_id.clone();
        tracing::info!(
            "account={}, model={}, path={}",
            account_id, model, path
        );

        let runtime = match state.client_manager.get_runtime_state(&account_id) {
            Some(r) => r,
            None => {
                tracing::warn!("[HANDLER] runtime_state missing for {}, skipping", account_id);
                attempted.insert(account_id);
                switch_count += 1;
                continue;
            }
        };

        // Token validity is guaranteed by selection filter (ClientManager skips expiring tokens).
        // Token refresh is handled by AccountMonitor in the background.
        // No token precheck needed on the request path.

        let mapped_session_uuid = builder::compute_mapped_session_uuid(
            session_id.as_deref().unwrap_or(""),
            &runtime.account_uuid,
        );
        let request_context = RequestContext {
            device_id: runtime.device_id.clone(),
            account_uuid: runtime.account_uuid.clone(),
            access_token: cli.access_token.clone(),
            mapped_session_uuid,
        };

        // Middle loop: same-account retries
        let mut first_byte_timeout_count = 0u32;
        let max_proxy_retries = 4u32;
        let mut proxy_retry_count = 0u32;

        let should_failover = 'middle: loop {

            // Per-attempt metadata rewrite on the parsed Value, then serialize.
            let mut attempt_value = prepared.value.clone();
            match builder::apply_metadata_in_place(&mut attempt_value, &request_context) {
                Ok(()) => {}
                Err(e) if e.is_license() => {
                    // License errors: return immediately, no failover/retry
                    if let Some(le) = e.license_error() {
                        if let claude_ultra_http::LicenseError::ConcurrencyLimitExceeded { max, active } = le {
                            let body = format!(
                                r#"{{"type":"error","error":{{"type":"overloaded_error","message":"[Claude Ultra] Account limit exceeded (max: {}, active: {}). Please upgrade your Claude Ultra subscription at Settings > Account."}}}}"#,
                                max, active
                            );
                            return axum::response::Response::builder()
                                .status(429)
                                .header("content-type", "application/json")
                                .body(axum::body::Body::from(body))
                                .unwrap();
                        }
                    }
                    let msg = match e.license_error() {
                        Some(claude_ultra_http::LicenseError::LicenseExpired) =>
                            r#"{"type":"error","error":{"type":"overloaded_error","message":"[Claude Ultra] License expired. Please renew your subscription at Settings > Account."}}"#,
                        Some(claude_ultra_http::LicenseError::LicenseInvalid(_)) =>
                            r#"{"type":"error","error":{"type":"overloaded_error","message":"[Claude Ultra] Invalid license. Please re-login to Claude Ultra."}}"#,
                        _ =>
                            r#"{"type":"error","error":{"type":"overloaded_error","message":"[Claude Ultra] License required. Please log in to Claude Ultra."}}"#,
                    };
                    return axum::response::Response::builder()
                        .status(503)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(msg))
                        .unwrap();
                }
                Err(_) => {
                    // non-license error: leave metadata as-is and continue (matches prior fallback behavior).
                }
            };
            // Serialize the per-attempt body. Fail-fast on error.
            let modified_body: Vec<u8> = match serde_json::to_vec(&attempt_value) {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!(
                        "[HANDLER] failed to serialize prepared body: {} — returning 500",
                        e
                    );
                    return axum::response::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(
                            r#"{"type":"error","error":{"type":"api_error","message":"[Claude Ultra] Internal error: failed to serialize request body."}}"#,
                        ))
                        .unwrap();
                }
            };
            // Precompute placeholder offset.
            let cch_offset = builder::find_billing_cch_offset(&modified_body, &attempt_value);
            // Capture request body for logging (before send-path processing).
            let request_body_for_log = if enable_logging && !log_created {
                Some(String::from_utf8_lossy(&modified_body).into_owned())
            } else {
                None
            };

            // ── Route resolution ──
            let route_mode = state.client_manager.get_route_mode(&account_id);
            let proxy_country = state.client_manager.get_proxy_country(&account_id);

            let actual = if state.proxy_mode == crate::models::config::ProxyMode::Direct {
                ActualRoute::Direct
            } else {
                crate::gateway::route::resolve_route(
                    &route_mode,
                    &proxy_country,
                    state.has_proxy,
                    !state.vercel_api_key.is_empty(),
                )
            };

            let upstream_host = match &actual {
                ActualRoute::Vercel => "ai-gateway.vercel.sh",
                _ => "api.anthropic.com",
            };
            let vercel_key = if matches!(&actual, ActualRoute::Vercel) && !state.vercel_api_key.is_empty() {
                Some(state.vercel_api_key.as_str())
            } else {
                None
            };
            let final_headers =
                builder::build_outbound_headers(&parts.headers, &request_context, modified_body.len(), upstream_host, vercel_key);
            let url = match &actual {
                ActualRoute::Vercel =>
                    builder::build_outbound_url("https://ai-gateway.vercel.sh", path),
                _ =>
                    builder::build_outbound_url(&state.upstream_base_url, path),
            };

            tracing::info!(
                "[REQ ] account={} model={} route={}",
                account_id, model, actual.label()
            );
            let send_start = std::time::Instant::now();

            // Send request: three-way routing (Proxy / Vercel / Direct)
            let resp: http::Response<axum::body::Body> = match &actual {
                ActualRoute::Proxy(_) => {
                    let pool = state.proxy_pool.as_ref().expect("ProxyPool required for gateway");
                    let proxy_country = match &actual {
                        ActualRoute::Proxy(c) => c.as_str(),
                        _ => "us",
                    };
                    let client = pool.client_with_country(&account_id, proxy_country);
                    match client
                        .post(&url)
                        .headers(final_headers.clone())
                        .body(Bytes::from(modified_body.clone()))
                        .cc_cli_version(&prepared.version, cch_offset)
                        .send()
                        .await
                    {
                        Ok(proxy_resp) => {
                            let send_elapsed = send_start.elapsed();
                            tracing::info!(
                                "[RESP] account={} status={} send_ms={}",
                                account_id, proxy_resp.status().as_u16(), send_elapsed.as_millis()
                            );
                            let http_resp = proxy_resp.into_http_response();
                            http_resp.map(|body| axum::body::Body::new(body))
                        }
                        Err(ProxyError::ConnectionFailed(msg)) => {
                            let send_elapsed = send_start.elapsed();
                            tracing::warn!(
                                "[FAIL] account={} proxy connection failed: {} send_ms={} (retry {}/{})",
                                account_id, msg, send_elapsed.as_millis(),
                                proxy_retry_count + 1, max_proxy_retries
                            );
                            if proxy_retry_count < max_proxy_retries {
                                proxy_retry_count += 1;
                                pool.renew_proxy(&account_id).await;
                                continue 'middle;
                            }
                            tracing::error!("[HANDLER] proxy exhausted after {} retries: {}", max_proxy_retries, msg);
                            return (
                                StatusCode::BAD_GATEWAY,
                                "Proxy connection failed. Check proxy settings.".to_string(),
                            ).into_response();
                        }
                        Err(ProxyError::Exhausted(msg)) => {
                            let send_elapsed = send_start.elapsed();
                            tracing::warn!(
                                "[FAIL] account={} proxy exhausted: {} send_ms={}",
                                account_id, msg, send_elapsed.as_millis()
                            );
                            break 'middle true;
                        }
                        Err(e) => {
                            tracing::error!("[FAIL] account={} proxy error: {}", account_id, e);
                            last_error_recoverable = false;
                            break 'middle true;
                        }
                    }
                }
                ActualRoute::Vercel => {
                    let mut vercel_req = state.client
                        .post(&url)
                        .headers(final_headers.clone())
                        .body(Bytes::from(modified_body.clone()))
                        .cc_cli_version(&prepared.version, cch_offset);
                    if let Some(ref proxy) = state.vercel_proxy_url {
                        vercel_req = vercel_req.proxy(proxy);
                    }
                    match vercel_req.send().await
                    {
                        Ok(vercel_resp) => {
                            let send_elapsed = send_start.elapsed();
                            tracing::info!(
                                "[RESP] account={} status={} send_ms={} (vercel)",
                                account_id, vercel_resp.status().as_u16(), send_elapsed.as_millis()
                            );
                            vercel_resp.map(|body| axum::body::Body::new(body))
                        }
                        Err(e) => {
                            let send_elapsed = send_start.elapsed();
                            tracing::warn!(
                                "[FAIL] account={} vercel failed: {} send_ms={}",
                                account_id, e, send_elapsed.as_millis()
                            );
                            last_error_recoverable = e.is_retryable();
                            break 'middle true;
                        }
                    }
                }
                ActualRoute::Direct => {
                    match state.client
                        .post(&url)
                        .headers(final_headers.clone())
                        .body(Bytes::from(modified_body.clone()))
                        .cc_cli_version(&prepared.version, cch_offset)
                        .send()
                        .await
                    {
                        Ok(direct_resp) => {
                            let send_elapsed = send_start.elapsed();
                            tracing::info!(
                                "[RESP] account={} status={} send_ms={} (direct)",
                                account_id, direct_resp.status().as_u16(), send_elapsed.as_millis()
                            );
                            direct_resp.map(|body| axum::body::Body::new(body))
                        }
                        Err(e) => {
                            let send_elapsed = send_start.elapsed();
                            tracing::warn!(
                                "[FAIL] account={} direct connection failed: {} send_ms={}",
                                account_id, e, send_elapsed.as_millis()
                            );
                            last_error_recoverable = e.is_retryable();
                            break 'middle true;
                        }
                    }
                }
            };

            let status = resp.status();
            tracing::info!("status={}, account={}", status, account_id);

            // Parse rate-limit headers
            if let Some(snapshot) = quota::parse_quota_headers(resp.headers()) {
                // 1. Sync update ClientManager immediately (selection uses this)
                state.client_manager.update_quota(&account_id, snapshot.clone());

                // 2. Throttled: persist to disk + notify frontend (60s per account)
                let now_ms = chrono::Utc::now().timestamp_millis();
                let should_notify = state.quota_last_persisted
                    .get(&account_id)
                    .map_or(true, |last| now_ms - *last >= QUOTA_PERSIST_INTERVAL_SECS as i64 * 1000);

                if should_notify {
                    state.quota_last_persisted.insert(account_id.clone(), now_ms);
                    if let Some(ref am) = state.account_manager {
                        let utilization = crate::models::quota::snapshot_to_utilization(&snapshot);
                        let am = am.clone();
                        let account_id_clone = account_id.clone();
                        tokio::spawn(async move {
                            let _ = am.merge_utilization(&account_id_clone, utilization, snapshot).await;
                        });
                    }
                }
            }

            if status.as_u16() == 200 {
                // Resolve email from AccountManager (fallback to account_id)
                let account_email = if let Some(ref am) = state.account_manager {
                    am.read(&account_id).await.map(|a| a.email).unwrap_or_else(|_| account_id.clone())
                } else {
                    account_id.clone()
                };
                let model_clone = model.clone();
                let duration_ms = start_time.elapsed().as_millis() as u64;

                let req_headers_json = Some(headers_to_json(&final_headers));
                let resp_headers_json_snapshot = Some(headers_to_json(resp.headers()));

                if !is_sse {
                    let (response, resp_body, response_size, usage) = json_passthrough(resp, enable_logging).await;
                    if !log_created {
                        log_gateway_request_with_id(
                            &stable_log_id,
                            path, &model_clone, &account_id, &account_email,
                            200, duration_ms, request_size, response_size,
                            usage.as_ref(), None,
                            request_body_for_log.clone(), resp_body,
                            req_headers_json.clone(), resp_headers_json_snapshot.clone(),
                            enable_logging,
                            client_ip.clone(), user_agent.clone(),
                            state.gateway_db.clone(),
                        );
                        // Note: no need to set log_created=true — we return immediately.
                        // If future refactor turns this into a continue/break, also set log_created.
                    }
                    return response;
                }

                if !log_created {
                    log_gateway_request_with_id(
                        &stable_log_id,
                        path, &model_clone, &account_id, &account_email,
                        200, duration_ms, request_size, 0, None, None,
                        request_body_for_log.clone(), None,
                        req_headers_json.clone(), resp_headers_json_snapshot.clone(),
                        enable_logging,
                        client_ip.clone(), user_agent.clone(),
                        state.gateway_db.clone(),
                    );
                    log_created = true;
                }
                match sse_probe_and_stream(resp, stable_log_id.clone(), model_clone, &mut first_byte_timeout_count, enable_logging, state.gateway_db.clone()).await {
                    Ok(response) => return response,
                    Err(SseProbeError::FirstByteTimeout) => {
                        // First-byte timeout: first time → same account, second time → failover
                        if first_byte_timeout_count <= 1 {
                            tracing::warn!("First-byte timeout #{} for {}, same-account retry", first_byte_timeout_count, account_id);
                            continue 'middle;
                        } else {
                            tracing::warn!("First-byte timeout #{} for {}, failover", first_byte_timeout_count, account_id);
                            break 'middle true;
                        }
                    }
                    Err(SseProbeError::StreamInterrupted { bytes_written }) => {
                        if bytes_written > 0 {
                            // Stream Written Guard: already sent data to client → cannot failover
                            // But renew proxy so client retry gets a fresh session
                            tracing::warn!("Stream interrupted after {} bytes for {}, renew proxy + sending SSE error", bytes_written, account_id);
                            if let Some(ref pool) = state.proxy_pool {
                                pool.renew_proxy(&account_id).await;
                            }
                            let event = build_sse_error_event("upstream_error", "Stream interrupted");
                            return sse_error_response(&event);
                        } else {
                            tracing::warn!("Stream interrupted (0 bytes) for {}, failover", account_id);
                            break 'middle true;
                        }
                    }
                    Err(SseProbeError::Other(msg)) => {
                        tracing::warn!("SSE probe failed for {}: {}", account_id, msg);
                        break 'middle true;
                    }
                }
            }

            // Snapshot upstream headers before consuming the body, so the Abort branch
            // can forward retry-after / request-id / anthropic-* to the client.
            let upstream_err_headers = resp.headers().clone();
            let upstream_err_content_type = resp.headers().get("content-type").cloned();
            let error_body = read_error_body(resp).await;
            let error_json: Option<serde_json::Value> = serde_json::from_str(&error_body).ok();
            let error_type = error_json.as_ref()
                .and_then(|v| v["error"]["type"].as_str())
                .unwrap_or("unknown");
            let error_message = error_json.as_ref()
                .and_then(|v| v["error"]["message"].as_str())
                .unwrap_or("");
            tracing::warn!("HTTP {} on {}: type={} message={}", status, account_id, error_type, &error_message[..error_message.len().min(200)]);

            // Log all non-200 upstream errors to gateway_db
            if !log_created {
                let account_email = if let Some(ref am) = state.account_manager {
                    am.read(&account_id).await.map(|a| a.email).unwrap_or_else(|_| account_id.clone())
                } else {
                    account_id.clone()
                };
                let duration_ms = start_time.elapsed().as_millis() as u64;
                log_gateway_request_with_id(
                    &stable_log_id,
                    path, &model, &account_id, &account_email,
                    status.as_u16(), duration_ms, request_size, error_body.len() as u64,
                    None, Some(&error_body),
                    request_body_for_log.clone(), Some(error_body.clone()),
                    Some(headers_to_json(&final_headers)),
                    Some(headers_to_json(&upstream_err_headers)),
                    enable_logging,
                    client_ip.clone(), user_agent.clone(),
                    state.gateway_db.clone(),
                );
                log_created = true;
            }

            // Vercel route: all non-2xx → user_disabled (soft, recoverable) instead of
            // disabled (hard). Precise error matching deferred to future e2e tests.
            // AccountMonitor.get_usage() (direct to Anthropic via IPRoyal) handles real bans.
            let is_vercel_route = matches!(&actual, ActualRoute::Vercel);

            match status.as_u16() {
                400 if !is_vercel_route && error_body.to_lowercase().contains("organization has been disabled") => {
                    let reason = format!("HTTP 400: {}", error_body);
                    tracing::error!("HTTP 400 (org disabled) on {}: {}", account_id, error_message);
                    if let Some(ref am) = state.account_manager {
                        let _ = am.set_disabled(&account_id, Some(reason)).await;
                    }
                    last_error_recoverable = false;
                    break 'middle true;
                }
                401 | 402 | 403 if is_vercel_route => {
                    // Vercel path: soft disable (user can re-enable)
                    let reason = format!("[Vercel] HTTP {}: {}", status, &error_message[..error_message.len().min(200)]);
                    tracing::warn!("[VERCEL] {} on account={} — user_disabled (soft)", status, account_id);
                    if let Some(ref am) = state.account_manager {
                        let _ = am.set_user_disabled(&account_id, Some(reason)).await;
                    }
                    last_error_recoverable = false;
                    break 'middle true;
                }
                401 | 402 | 403 => {
                    // Direct/Proxy path: hard disable (Anthropic account issue)
                    let reason = format!("HTTP {}: {}", status, error_body);
                    tracing::error!("HTTP {} on {}: {}", status, account_id, error_message);
                    if let Some(ref am) = state.account_manager {
                        let _ = am.set_disabled(&account_id, Some(reason)).await;
                    }
                    last_error_recoverable = false;
                    break 'middle true;
                }
                429 => {
                    let reason = format!("HTTP 429: {}", error_body);
                    tracing::warn!("HTTP 429 on {}: {}", account_id, error_message);
                    if let Some(ref am) = state.account_manager {
                        let _ = am.set_user_disabled(&account_id, Some(reason)).await;
                    }
                    last_error_recoverable = false;
                    break 'middle true;
                }
                _ => {
                    let mut response = Response::new(Body::from(error_body));
                    *response.status_mut() = StatusCode::from_u16(status.as_u16())
                        .unwrap_or(StatusCode::BAD_REQUEST);
                    if let Some(ct) = upstream_err_content_type.as_ref() {
                        response.headers_mut().insert(http::header::CONTENT_TYPE, ct.clone());
                    }
                    copy_passthrough_headers(&upstream_err_headers, response.headers_mut());
                    return response;
                }
            }
        };

        if should_failover {
            tracing::warn!("[HANDLER] failover from account={}, switch_count={}", account_id, switch_count + 1);
            attempted.insert(account_id);
            switch_count += 1;
            log_created = false; // reset for next account
            continue;
        }

        break;
    }

    tracing::error!("[HANDLER] all retry attempts exhausted, returning 503");

    (StatusCode::SERVICE_UNAVAILABLE, "All retry attempts exhausted").into_response()
}

/// SSE probe errors for structured handling.
enum SseProbeError {
    FirstByteTimeout,
    StreamInterrupted { bytes_written: usize },
    Other(String),
}

/// SSE passthrough with first-chunk probing, token extraction, heartbeat keepalive,
/// and Stream Written Guard support.
async fn sse_probe_and_stream<B>(
    resp: http::Response<B>,
    log_id: String,
    model: String,
    first_byte_timeout_count: &mut u32,
    enable_logging: bool,
    db: Option<Arc<crate::modules::gateway_db::GatewayDb>>,
) -> Result<Response, SseProbeError>
where
    B: http_body::Body<Data = Bytes> + Send + Unpin + 'static,
    B::Error: std::fmt::Display + Send,
{
    let (parts, mut body) = resp.into_parts();
    let upstream_headers = parts.headers;

    // First chunk with configurable timeout
    let first_chunk = loop {
        match tokio::time::timeout(Duration::from_secs(FIRST_BYTE_TIMEOUT_SECS), body.frame()).await {
            Ok(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    if data.starts_with(b":") {
                        continue; // skip SSE comments
                    }
                    break Ok(Bytes::copy_from_slice(data));
                }
            }
            Ok(Some(Err(e))) => break Err(SseProbeError::Other(format!("SSE first chunk error: {}", e))),
            Ok(None) => break Err(SseProbeError::StreamInterrupted { bytes_written: 0 }),
            Err(_) => {
                *first_byte_timeout_count += 1;
                break Err(SseProbeError::FirstByteTimeout);
            }
        }
    }?;

    let mut response_headers = http::HeaderMap::new();
    response_headers.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    response_headers.insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    // Forward rate-limit / request-id / anthropic-* headers from upstream
    copy_passthrough_headers(&upstream_headers, &mut response_headers);

    let usage_holder = Arc::new(tokio::sync::Mutex::new(None::<TokenUsage>));
    let usage_writer = usage_holder.clone();

    // Accumulate response body for logging (no truncation)
    let body_buffer = Arc::new(tokio::sync::Mutex::new(Vec::<u8>::new()));
    let body_writer = body_buffer.clone();
    let capture_body = enable_logging;
    let db_for_stream = db;

    if let Ok(text) = std::str::from_utf8(&first_chunk) {
        for line in text.lines() {
            if let Some(u) = billing::extract_usage_from_sse_line(line) {
                *usage_writer.lock().await = Some(u);
            }
        }
    }
    if capture_body {
        let mut buf = body_writer.lock().await;
        buf.extend_from_slice(&first_chunk);
    }

    let stream = async_stream::stream! {
        yield Ok::<Bytes, std::io::Error>(first_chunk);

        loop {
            match tokio::time::timeout(Duration::from_secs(60), body.frame()).await {
                Ok(Some(Ok(frame))) => {
                    if let Some(data) = frame.data_ref() {
                        if let Ok(text) = std::str::from_utf8(data) {
                            for line in text.lines() {
                                if let Some(u) = billing::extract_usage_from_sse_line(line) {
                                    *usage_writer.lock().await = Some(u);
                                }
                            }
                        }
                        // Accumulate response body
                        if capture_body {
                            let mut buf = body_writer.lock().await;
                            buf.extend_from_slice(data);
                        }
                        yield Ok(Bytes::copy_from_slice(data));
                    }
                }
                Ok(Some(Err(e))) => {
                    tracing::error!("SSE stream error: {}", e);
                    let err_json = serde_json::json!({
                        "type": "error",
                        "error": {
                            "type": "overloaded_error",
                            "message": "Overloaded",
                        }
                    });
                    yield Ok(Bytes::from(format!("event: error\ndata: {}\n\n", err_json)));
                    break;
                }
                Ok(None) => break,
                Err(_) => {
                    yield Ok(Bytes::from_static(b": ping\n\n"));
                }
            }
        }

        // Stream ended — update usage and response body, then notify
        let final_usage = usage_holder.lock().await.clone();
        if let Some(usage) = final_usage {
            if let Some(ref db) = db_for_stream {
                let cost = billing::calculate_cost(&model, &usage);
                let lid = log_id.clone();
                let db = db.clone();
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    db.update_usage(&lid, &usage, &cost)
                }).await {
                    tracing::error!("Failed to update usage for {}: {}", log_id, e);
                }
            }
        }

        if capture_body {
            if let Some(ref db) = db_for_stream {
                let buf = body_buffer.lock().await;
                if !buf.is_empty() {
                    let resp_body = String::from_utf8_lossy(&buf).into_owned();
                    let lid = log_id.clone();
                    let db = db.clone();
                    if let Err(e) = tokio::task::spawn_blocking(move || {
                        db.update_response_body(&lid, &resp_body)
                    }).await {
                        tracing::error!("Failed to update response_body for {}: {}", log_id, e);
                    }
                }
            }
        }

        // Notify frontend after ALL updates complete
        emit_log_updated();
    };

    let body = Body::from_stream(stream);
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    *response.headers_mut() = response_headers;
    Ok(response)
}

/// Build an SSE error response for Stream Written Guard.
fn sse_error_response(event: &str) -> Response {
    let mut response_headers = http::HeaderMap::new();
    response_headers.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    let body = Body::from(event.to_string());
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    *response.headers_mut() = response_headers;
    response
}

/// Serialize HeaderMap to JSON array of [name, value] pairs for log storage.
fn headers_to_json(headers: &http::HeaderMap) -> String {
    let pairs: Vec<[String; 2]> = headers
        .iter()
        .map(|(n, v)| {
            [n.as_str().to_string(), v.to_str().unwrap_or("").to_string()]
        })
        .collect();
    serde_json::to_string(&pairs).unwrap_or_else(|_| "[]".to_string())
}

/// Copy passthrough headers from upstream response to gateway response.
///
/// Preserves client-facing observability and rate-limit signals:
/// - `retry-after`: CLI needs this to respect 429 backoff correctly
/// - `x-request-id` / `request-id`: correlation across upstream/gateway/client logs
/// - `anthropic-*`: ratelimit snapshot, beta features, organization headers
///
/// Does NOT copy:
/// - Hop-by-hop headers (connection, keep-alive, transfer-encoding, etc.)
/// - Gateway-managed headers (content-type, cache-control) — caller decides
fn copy_passthrough_headers(src: &http::HeaderMap, dst: &mut http::HeaderMap) {
    for (name, value) in src.iter() {
        let n = name.as_str().to_ascii_lowercase();
        let should_copy = n == "retry-after"
            || n == "x-request-id"
            || n == "request-id"
            || n.starts_with("anthropic-");
        if should_copy {
            dst.insert(name.clone(), value.clone());
        }
    }
}

/// Read error body from a non-200 response.
async fn read_error_body<B>(resp: http::Response<B>) -> String
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
{
    let body = resp
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes())
        .unwrap_or_default();
    String::from_utf8_lossy(&body).to_string()
}

/// Forward a non-SSE JSON response.
/// Returns (Response, response_body_for_log, response_size_bytes, usage_extracted_from_body).
///
/// Called only on 200 OK (see retry loop in `gateway_request`). For /v1/messages with
/// stream:false, Anthropic returns a single JSON object with a top-level `usage` field.
/// We parse it here so the Gateway can log accurate token counts and cost without the
/// SSE event pipeline.
///
/// `response_size` is returned unconditionally (independent of `capture_body`) so DB
/// logging records accurate byte counts even when body capture is disabled for privacy.
async fn json_passthrough<B>(resp: http::Response<B>, capture_body: bool) -> (Response, Option<String>, u64, Option<TokenUsage>)
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: std::fmt::Display,
{
    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes())
        .unwrap_or_default();

    let response_size = body.len() as u64;

    // Extract usage from the 200 JSON body
    let usage = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| extract_usage_from_json(&v));

    // Capture response body for logging
    let body_for_log = if capture_body {
        Some(String::from_utf8_lossy(&body).into_owned())
    } else {
        None
    };

    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
    if let Some(ct) = resp_headers.get("content-type") {
        if let Ok(v) = HeaderValue::from_bytes(ct.as_bytes()) {
            response
                .headers_mut()
                .insert(http::header::CONTENT_TYPE, v);
        }
    }
    // Forward rate-limit / request-id / anthropic-* headers for client observability
    copy_passthrough_headers(&resp_headers, response.headers_mut());
    (response, body_for_log, response_size, usage)
}

/// Extract TokenUsage from a non-streaming Anthropic Messages response JSON.
/// Expected shape: `{"usage": {"input_tokens": N, "output_tokens": N, ...}}`
fn extract_usage_from_json(value: &serde_json::Value) -> Option<TokenUsage> {
    let usage = value.get("usage")?;
    let input = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let output = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let cache_creation = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let cache_read = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    // Return None when all fields are zero/missing to avoid logging empty usage rows
    if input == 0 && output == 0 && cache_creation == 0 && cache_read == 0 {
        return None;
    }
    Some(TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_tokens: cache_creation,
        cache_read_tokens: cache_read,
    })
}

/// Log a gateway request to SQLite asynchronously and emit event.
/// Body fields are stored in DB but NOT included in the emitted event (memory optimization).
fn log_gateway_request_with_id(
    log_id: &str,
    path: &str,
    model: &str,
    account_id: &str,
    account_email: &str,
    status: u16,
    duration_ms: u64,
    request_size: u64,
    response_size: u64,
    usage: Option<&TokenUsage>,
    error: Option<&str>,
    request_body: Option<String>,
    response_body: Option<String>,
    request_headers: Option<String>,
    response_headers: Option<String>,
    enable_logging: bool,
    client_ip: Option<String>,
    user_agent: Option<String>,
    db: Option<Arc<crate::modules::gateway_db::GatewayDb>>,
) -> String {
    // Skip logging for count_tokens requests
    if path.contains("count_tokens") {
        return log_id.to_string();
    }

    let cost = usage.map(|u| billing::calculate_cost(model, u));

    let log = gateway_db::RequestLog {
        id: log_id.to_string(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        method: "POST".to_string(),
        url: path.to_string(),
        status,
        duration_ms,
        model: Some(model.to_string()),
        account_id: Some(account_id.to_string()),
        account_email: Some(account_email.to_string()),
        input_tokens: usage.map(|u| u.input_tokens),
        output_tokens: usage.map(|u| u.output_tokens),
        cache_creation_tokens: usage.map(|u| u.cache_creation_tokens),
        cache_read_tokens: usage.map(|u| u.cache_read_tokens),
        total_tokens: usage.map(|u| u.total_tokens()),
        input_cost: cost.as_ref().map(|c| c.input_cost),
        output_cost: cost.as_ref().map(|c| c.output_cost),
        cache_creation_cost: cost.as_ref().map(|c| c.cache_creation_cost),
        cache_read_cost: cost.as_ref().map(|c| c.cache_read_cost),
        total_cost: cost.as_ref().map(|c| c.total_cost),
        error: error.map(|e| e.to_string()),
        request_size: Some(request_size),
        response_size: Some(response_size),
        client_ip: client_ip.clone(),
        user_agent: user_agent.clone(),
        api_key_prefix: None,
        request_body,
        response_body,
        request_headers,
        response_headers,
    };

    let log_id = log.id.clone();

    if enable_logging {
        if let Some(db) = db {
            // Save log to DB, then notify frontend
            tokio::spawn(async move {
                let log_to_save = log;
                match tokio::task::spawn_blocking(move || db.save_log(&log_to_save)).await {
                    Ok(Ok(())) => emit_log_updated(),
                    Ok(Err(e)) => tracing::error!("Failed to save gateway log: {}", e),
                    Err(e) => tracing::error!("Failed to spawn save task: {}", e),
                }
            });
        }
    }

    log_id
}

/// Build a pseudo email for a transparent caller's pseudo-account log entry.
/// IPv6 literals are wrapped in `[...]` so the string stays closer to
/// RFC 5321 and avoids raw `:` characters leaking into UI columns.
#[cfg(feature = "internal")]
fn build_pseudo_email(host: &str) -> String {
    let host_part = if host.contains(':') && !host.starts_with('[') {
        format!("[{}]", host)
    } else {
        host.to_string()
    };
    format!("transparent@{}", host_part)
}

/// Transparent forward — no account selection, no body rewrite, no header rewrite.
/// Direct to api.anthropic.com (no proxy). Inbound Authorization passthrough.
#[cfg(feature = "internal")]
pub(super) async fn transparent_forward(
    state: AppState,
    parts: http::request::Parts,
    body_bytes: Bytes,
    path: &str,
    endpoint_allows_sse: bool,
    enable_logging: bool,
    client_ip: Option<String>,
    user_agent: Option<String>,
    start_time: std::time::Instant,
) -> Response {
    use http::HeaderValue;
    let stable_log_id = uuid::Uuid::new_v4().to_string();

    // Pseudo-account: aggregate by client IP. v5 UUID keeps it distinct from real
    // account UUIDs and stable per IP.
    let host = client_ip.as_deref().unwrap_or("unknown");
    let pseudo_email = build_pseudo_email(host);
    let pseudo_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        pseudo_email.as_bytes(),
    ).to_string();

    let query = parts.uri.query().map(|q| format!("?{}", q)).unwrap_or_default();
    let url = format!("https://api.anthropic.com{}{}", path, query);

    let mut outbound = http::HeaderMap::new();
    // Strip only protocol-critical entries that must be rewritten per hop.
    // The remaining hop-by-hop set (connection/keep-alive/te/trailer/
    // upgrade/proxy-*) is intentionally preserved so the outbound header
    // list matches what a native CLI request looks like on the wire.
    for (name, value) in parts.headers.iter() {
        let n = name.as_str().to_lowercase();
        if matches!(n.as_str(), "host" | "content-length" | "transfer-encoding") {
            continue;
        }
        outbound.insert(name.clone(), value.clone());
    }
    outbound.insert("host", HeaderValue::from_static("api.anthropic.com"));
    if let Ok(v) = HeaderValue::from_str(&body_bytes.len().to_string()) {
        outbound.insert("content-length", v);
    }

    let req_body_log = if enable_logging {
        Some(String::from_utf8_lossy(&body_bytes).into_owned())
    } else {
        None
    };
    let req_headers_log = Some(headers_to_json(&outbound));

    let parsed: Option<serde_json::Value> = serde_json::from_slice(&body_bytes).ok();
    let model = parsed
        .as_ref()
        .and_then(|v| v.get("model"))
        .and_then(|m| m.as_str())
        .unwrap_or("unknown")
        .to_string();
    let is_sse = endpoint_allows_sse
        && parsed
            .as_ref()
            .and_then(|v| v.get("stream"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    let request_size = body_bytes.len() as u64;

    tracing::info!(
        "[REQ ] account={} model={} route=transparent",
        pseudo_email, model
    );
    let send_start = std::time::Instant::now();
    let resp = match state
        .client
        .post(&url)
        .headers(outbound.clone())
        .body(body_bytes.clone())
        .send()
        .await
    {
        Ok(r) => r.map(axum::body::Body::new),
        Err(e) => {
            tracing::error!("[FAIL] account={} upstream error: {}", pseudo_email, e);
            let err_str = e.to_string();
            let duration_ms = start_time.elapsed().as_millis() as u64;
            // Emit a failure log so transparent audit has a record of upstream
            // errors instead of going silent.
            log_gateway_request_with_id(
                &stable_log_id,
                path,
                &model,
                &pseudo_id,
                &pseudo_email,
                502,
                duration_ms,
                request_size,
                0,
                None,
                Some(&err_str),
                req_body_log,
                None,
                req_headers_log,
                None,
                enable_logging,
                client_ip,
                user_agent,
                state.gateway_db.clone(),
            );
            return (StatusCode::BAD_GATEWAY, format!("upstream error: {}", err_str))
                .into_response();
        }
    };
    let status = resp.status();
    let resp_headers_log = Some(headers_to_json(resp.headers()));
    let duration_ms = start_time.elapsed().as_millis() as u64;
    tracing::info!(
        "[RESP] account={} status={} send_ms={} (transparent)",
        pseudo_email,
        status.as_u16(),
        send_start.elapsed().as_millis()
    );

    if !is_sse || status.as_u16() != 200 {
        let (response, resp_body_log, response_size, usage) =
            json_passthrough(resp, enable_logging).await;
        // Mirror the main gateway path: on non-2xx, surface the upstream
        // body as the log's `error` field so operators can filter without
        // parsing response_body.
        let error_field = if status.as_u16() != 200 {
            resp_body_log.as_deref()
        } else {
            None
        };
        log_gateway_request_with_id(
            &stable_log_id,
            path,
            &model,
            &pseudo_id,
            &pseudo_email,
            status.as_u16(),
            duration_ms,
            request_size,
            response_size,
            usage.as_ref(),
            error_field,
            req_body_log,
            resp_body_log.clone(),
            req_headers_log,
            resp_headers_log,
            enable_logging,
            client_ip,
            user_agent,
            state.gateway_db.clone(),
        );
        return response;
    }

    log_gateway_request_with_id(
        &stable_log_id,
        path,
        &model,
        &pseudo_id,
        &pseudo_email,
        200,
        duration_ms,
        request_size,
        0,
        None,
        None,
        req_body_log,
        None,
        req_headers_log,
        resp_headers_log,
        enable_logging,
        client_ip,
        user_agent,
        state.gateway_db.clone(),
    );

    let mut first_byte_timeout_count = 0u32;
    let log_id_for_fixup = stable_log_id.clone();
    let db_for_fixup = state.gateway_db.clone();
    match sse_probe_and_stream(
        resp,
        stable_log_id,
        model,
        &mut first_byte_timeout_count,
        enable_logging,
        state.gateway_db.clone(),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // Provisional log row was written with status=200 before probing.
            // The client is about to see a 502, so revise the persisted row
            // accordingly so the audit trail matches reality.
            let err_str = match e {
                SseProbeError::FirstByteTimeout => "sse first-byte timeout".to_string(),
                SseProbeError::StreamInterrupted { bytes_written } => {
                    format!("sse stream interrupted after {} bytes", bytes_written)
                }
                SseProbeError::Other(m) => m,
            };
            if let Some(db) = db_for_fixup {
                let log_id = log_id_for_fixup.clone();
                tokio::task::spawn_blocking(move || {
                    if let Err(err) = db.update_status_and_error(&log_id, 502, &err_str) {
                        tracing::error!(
                            "Failed to patch transparent SSE failure log: {}",
                            err
                        );
                    }
                });
            }
            (StatusCode::BAD_GATEWAY, "sse stream error").into_response()
        }
    }
}

/// Emit a lightweight notification — frontend reloads from DB.
fn emit_log_updated() {
    use super::log_bridge;
    if let Some(handle) = log_bridge::get_app_handle() {
        use tauri::Emitter;
        let _ = handle.emit("gateway://log-updated", ());
    }
}

fn failover_delay(switch_count: u32) -> Duration {
    Duration::from_millis(switch_count as u64 * 500)
}


#[cfg(test)]
fn is_stream_written(bytes_before: usize, bytes_after: usize) -> bool {
    bytes_after > bytes_before
}

fn build_sse_error_event(error_type: &str, message: &str) -> String {
    let json = serde_json::json!({
        "type": "error",
        "error": {
            "type": error_type,
            "message": message,
        }
    });
    format!("event: error\ndata: {}\n\n", json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::client_manager::ClientManager;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_extract_usage_from_json_full() {
        let v = serde_json::json!({
            "id": "msg_123",
            "model": "claude-opus-4-6",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_creation_input_tokens": 20,
                "cache_read_input_tokens": 30
            }
        });
        let u = extract_usage_from_json(&v).expect("should extract");
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
        assert_eq!(u.cache_creation_tokens, 20);
        assert_eq!(u.cache_read_tokens, 30);
    }

    #[test]
    fn test_extract_usage_from_json_minimal() {
        // Anthropic may omit cache fields for small requests
        let v = serde_json::json!({
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });
        let u = extract_usage_from_json(&v).expect("should extract even without cache");
        assert_eq!(u.input_tokens, 10);
        assert_eq!(u.output_tokens, 5);
        assert_eq!(u.cache_creation_tokens, 0);
        assert_eq!(u.cache_read_tokens, 0);
    }

    #[test]
    fn test_extract_usage_from_json_missing_usage() {
        let v = serde_json::json!({"id": "msg_123"});
        assert!(extract_usage_from_json(&v).is_none());
    }

    #[test]
    fn test_extract_usage_from_json_all_zero() {
        let v = serde_json::json!({
            "usage": { "input_tokens": 0, "output_tokens": 0 }
        });
        assert!(
            extract_usage_from_json(&v).is_none(),
            "all-zero usage should return None (no billing row)"
        );
    }

    /// Regression test for Bug #1 (`_.speed` crash on non-stream requests):
    /// Simulates the full `/v1/messages` stream:false flow — client's non-streaming
    /// request reaches Anthropic, upstream returns plain JSON, json_passthrough
    /// preserves JSON Content-Type (NOT text/event-stream) and extracts usage.
    #[tokio::test]
    async fn test_json_passthrough_non_stream_messages_flow() {
        // Realistic Anthropic non-streaming /v1/messages response
        let anthropic_response_body = serde_json::to_vec(&serde_json::json!({
            "id": "msg_01abc",
            "type": "message",
            "role": "assistant",
            "model": "claude-opus-4-6",
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 42,
                "output_tokens": 17,
                "cache_creation_input_tokens": 5,
                "cache_read_input_tokens": 8
            }
        })).unwrap();
        let body_len = anthropic_response_body.len() as u64;

        let upstream_resp: http::Response<http_body_util::Full<Bytes>> = http::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .header("x-request-id", "req_123")
            .body(http_body_util::Full::new(Bytes::from(anthropic_response_body.clone())))
            .unwrap();

        // Call with capture_body=true (logging enabled)
        let (response, body_for_log, response_size, usage) =
            json_passthrough(upstream_resp, true).await;

        // Response should preserve JSON Content-Type — NOT be forced to text/event-stream
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/json",
            "non-stream response must stay JSON, not be upgraded to SSE (root cause of Bug #1)"
        );
        assert_eq!(response.status().as_u16(), 200);

        // Response size must equal actual body bytes, independent of capture_body
        assert_eq!(response_size, body_len, "response_size must be actual body length");

        // Body captured for log when enabled
        assert!(body_for_log.is_some(), "body captured when logging enabled");
        assert!(body_for_log.as_ref().unwrap().contains("msg_01abc"));

        // Usage correctly extracted
        let u = usage.expect("usage must be extracted from 200 JSON");
        assert_eq!(u.input_tokens, 42);
        assert_eq!(u.output_tokens, 17);
        assert_eq!(u.cache_creation_tokens, 5);
        assert_eq!(u.cache_read_tokens, 8);
    }

    #[test]
    fn test_copy_passthrough_headers_allowlist() {
        let mut src = http::HeaderMap::new();
        src.insert("retry-after", "60".parse().unwrap());
        src.insert("x-request-id", "req_abc".parse().unwrap());
        src.insert("request-id", "legacy_req".parse().unwrap());
        src.insert("anthropic-ratelimit-requests-limit", "5000".parse().unwrap());
        src.insert("anthropic-organization-id", "org_xyz".parse().unwrap());
        // Headers that should NOT be forwarded (hop-by-hop / gateway-managed)
        src.insert("connection", "keep-alive".parse().unwrap());
        src.insert("transfer-encoding", "chunked".parse().unwrap());
        src.insert("content-type", "application/json".parse().unwrap());
        src.insert("cache-control", "no-store".parse().unwrap());
        src.insert("server", "cloudflare".parse().unwrap());

        let mut dst = http::HeaderMap::new();
        copy_passthrough_headers(&src, &mut dst);

        assert_eq!(dst.get("retry-after").unwrap(), "60");
        assert_eq!(dst.get("x-request-id").unwrap(), "req_abc");
        assert_eq!(dst.get("request-id").unwrap(), "legacy_req");
        assert_eq!(dst.get("anthropic-ratelimit-requests-limit").unwrap(), "5000");
        assert_eq!(dst.get("anthropic-organization-id").unwrap(), "org_xyz");
        // Excluded
        assert!(dst.get("connection").is_none());
        assert!(dst.get("transfer-encoding").is_none());
        assert!(dst.get("content-type").is_none(), "content-type is gateway-managed");
        assert!(dst.get("cache-control").is_none(), "cache-control is gateway-managed");
        assert!(dst.get("server").is_none());
    }

    #[test]
    fn test_copy_passthrough_headers_case_insensitive() {
        let mut src = http::HeaderMap::new();
        src.insert("Retry-After", "120".parse().unwrap());
        src.insert("Anthropic-Beta", "claude-code-20250219".parse().unwrap());
        let mut dst = http::HeaderMap::new();
        copy_passthrough_headers(&src, &mut dst);
        assert_eq!(dst.get("retry-after").unwrap(), "120");
        assert_eq!(dst.get("anthropic-beta").unwrap(), "claude-code-20250219");
    }

    #[tokio::test]
    async fn test_json_passthrough_forwards_rate_limit_headers() {
        // Regression test for P2-E: Gateway must forward retry-after, x-request-id,
        // and anthropic-* headers so the CLI can correctly honor rate limits and
        // diagnose issues via request ID correlation.
        let body_bytes = serde_json::to_vec(&serde_json::json!({
            "usage": { "input_tokens": 5, "output_tokens": 5 }
        })).unwrap();

        let upstream_resp: http::Response<http_body_util::Full<Bytes>> = http::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .header("retry-after", "30")
            .header("x-request-id", "req_P2E_test")
            .header("anthropic-ratelimit-requests-remaining", "42")
            .header("anthropic-organization-id", "org_gateway_test")
            // Should NOT be forwarded (hop-by-hop)
            .header("connection", "close")
            .body(http_body_util::Full::new(Bytes::from(body_bytes)))
            .unwrap();

        let (response, _body, _size, _usage) = json_passthrough(upstream_resp, false).await;

        assert_eq!(response.headers().get("retry-after").unwrap(), "30");
        assert_eq!(response.headers().get("x-request-id").unwrap(), "req_P2E_test");
        assert_eq!(
            response.headers().get("anthropic-ratelimit-requests-remaining").unwrap(),
            "42"
        );
        assert_eq!(
            response.headers().get("anthropic-organization-id").unwrap(),
            "org_gateway_test"
        );
        // Hop-by-hop should be stripped
        assert!(response.headers().get("connection").is_none());
        // Gateway preserves content-type
        assert_eq!(response.headers().get("content-type").unwrap(), "application/json");
    }

    /// Privacy case: logging disabled → body_for_log is None, but response_size still accurate.
    #[tokio::test]
    async fn test_json_passthrough_response_size_independent_of_capture() {
        let body_bytes = serde_json::to_vec(&serde_json::json!({
            "id": "msg_x",
            "usage": { "input_tokens": 1, "output_tokens": 2 }
        })).unwrap();
        let body_len = body_bytes.len() as u64;

        let upstream_resp: http::Response<http_body_util::Full<Bytes>> = http::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(http_body_util::Full::new(Bytes::from(body_bytes)))
            .unwrap();

        // capture_body=false (logging disabled for privacy)
        let (_response, body_for_log, response_size, usage) =
            json_passthrough(upstream_resp, false).await;

        assert!(body_for_log.is_none(), "body not captured when logging disabled");
        assert_eq!(response_size, body_len, "response_size must still be accurate when body not captured");
        assert!(usage.is_some(), "usage still extracted regardless of body capture");
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "claude_ultra_handler_test_{}",
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

    fn write_token_file(dir: &std::path::Path, filename: &str, json: &str) {
        std::fs::write(dir.join(filename), json).unwrap();
    }

    fn make_test_uuid(seed: &str) -> String {
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, seed.as_bytes()).to_string()
    }

    fn make_token_json(account_id: &str, email: &str) -> String {
        let far_future = chrono::Utc::now().timestamp_millis() + 24 * 60 * 60 * 1000; // +24h
        format!(
            r#"{{
                "account_id": "{}",
                "email": "{}",
                "access_token": "sk-ant-oat01-test-{}",
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

    fn setup_client_manager(accounts: &[(&str, &str)]) -> (Arc<ClientManager>, TestDir) {
        let dir = TestDir::new();
        for (id, email) in accounts {
            write_token_file(&dir.path, &format!("{}.json", id), &make_token_json(id, email));
        }
        let bc = Arc::new(BoringClient::builder().build().unwrap());
        let cm = Arc::new(ClientManager::new(bc));
        cm.load_tokens(&dir.path).unwrap();
        (cm, dir)
    }

    // ── Client selection tests ──────────────────────────────

    #[test]
    fn test_client_selection_round_robin() {
        let (cm, _dir) = setup_client_manager(&[("a1", "a1@t.com"), ("a2", "a2@t.com")]);
        let empty = HashSet::new();
        let t1 = cm.get_client_simple(&empty).unwrap();
        let t2 = cm.get_client_simple(&empty).unwrap();
        assert!(t1.account_id == "a1" || t1.account_id == "a2");
        assert!(t2.account_id == "a1" || t2.account_id == "a2");
    }

    #[test]
    fn test_disable_account_switches_to_next() {
        let (cm, _dir) = setup_client_manager(&[("a1", "a1@t.com"), ("a2", "a2@t.com")]);
        cm.disable_client("a1", "HTTP 401");
        let cli = cm.get_client_simple(&HashSet::new()).unwrap();
        assert_eq!(cli.account_id, "a2");
    }

    #[test]
    fn test_all_disabled_returns_none() {
        let (cm, _dir) = setup_client_manager(&[("a1", "a1@t.com")]);
        cm.disable_client("a1", "HTTP 401");
        assert!(cm.get_client_simple(&HashSet::new()).is_none());
    }

    #[test]
    fn test_attempted_accounts_skipped() {
        let (cm, _dir) = setup_client_manager(&[("a1", "a1@t.com"), ("a2", "a2@t.com")]);
        let mut attempted = HashSet::new();
        attempted.insert("a1".to_string());
        let cli = cm.get_client_simple(&attempted).unwrap();
        assert_eq!(cli.account_id, "a2");
    }

    #[test]
    fn test_all_attempted_returns_none() {
        let (cm, _dir) = setup_client_manager(&[("a1", "a1@t.com")]);
        let mut attempted = HashSet::new();
        attempted.insert("a1".to_string());
        assert!(cm.get_client_simple(&attempted).is_none());
    }

    #[test]
    fn test_runtime_state_generated_per_account() {
        let (cm, _dir) = setup_client_manager(&[("a1", "a1@t.com"), ("a2", "a2@t.com")]);
        let s1 = cm.get_runtime_state("a1").unwrap();
        let s2 = cm.get_runtime_state("a2").unwrap();
        assert_ne!(s1.device_id, s2.device_id, "each account has unique device_id");
        assert_eq!(s1.device_id.len(), 64);
        assert_eq!(s2.device_id.len(), 64);
    }

    #[test]
    fn test_quota_update() {
        let (cm, _dir) = setup_client_manager(&[("a1", "a1@t.com")]);
        assert!(cm.get_runtime_state("a1").unwrap().quota.is_none());

        let snapshot = crate::models::quota::QuotaSnapshot {
            status: "allowed".to_string(),
            five_hour: None,
            seven_day: None,
            overage: None,
            representative_claim: None,
            fallback: None,
            fallback_percentage: None,
            reset_at: None,
            organization_id: None,
            updated_at: 1000,
        };
        cm.update_quota("a1", snapshot);
        let state = cm.get_runtime_state("a1").unwrap();
        assert_eq!(state.quota.as_ref().unwrap().status, "allowed");
    }

    // ── Stream Guard tests ──────────────────────────────────

    #[test]
    fn test_stream_written_guard() {
        assert!(is_stream_written(0, 100));
        assert!(!is_stream_written(100, 100));
    }

    #[test]
    fn test_sse_error_event() {
        let event = build_sse_error_event("upstream_error", "Stream interrupted");
        assert!(event.starts_with("event: error\n"));
        assert!(event.contains("\"type\":\"error\""));
        assert!(event.contains("upstream_error"));
        assert!(event.ends_with("\n\n"));
    }

    #[cfg(feature = "internal")]
    fn pseudo_id_for(ip: &str) -> String {
        let email = build_pseudo_email(ip);
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, email.as_bytes()).to_string()
    }

    #[cfg(feature = "internal")]
    #[test]
    fn test_transparent_pseudo_id_deterministic() {
        let a = pseudo_id_for("127.0.0.1");
        let b = pseudo_id_for("127.0.0.1");
        assert_eq!(a, b);
    }

    #[cfg(feature = "internal")]
    #[test]
    fn test_transparent_pseudo_id_distinct_per_ip() {
        let v4 = pseudo_id_for("127.0.0.1");
        let v6 = pseudo_id_for("::1");
        let lan = pseudo_id_for("192.168.1.50");
        assert_ne!(v4, v6);
        assert_ne!(v4, lan);
        assert_ne!(v6, lan);
    }

    #[cfg(feature = "internal")]
    #[test]
    fn test_transparent_pseudo_email_ipv4_passthrough() {
        assert_eq!(build_pseudo_email("127.0.0.1"), "transparent@127.0.0.1");
        assert_eq!(build_pseudo_email("192.168.1.50"), "transparent@192.168.1.50");
    }

    #[cfg(feature = "internal")]
    #[test]
    fn test_transparent_pseudo_email_ipv6_bracketed() {
        assert_eq!(build_pseudo_email("::1"), "transparent@[::1]");
        assert_eq!(
            build_pseudo_email("2001:db8::1"),
            "transparent@[2001:db8::1]",
        );
    }

    #[cfg(feature = "internal")]
    #[test]
    fn test_transparent_pseudo_email_already_bracketed_not_doubled() {
        // A pre-bracketed literal should not accrete another layer.
        assert_eq!(build_pseudo_email("[::1]"), "transparent@[::1]");
    }

    #[cfg(feature = "internal")]
    #[test]
    fn test_transparent_pseudo_email_unknown_host() {
        assert_eq!(build_pseudo_email("unknown"), "transparent@unknown");
    }
}
