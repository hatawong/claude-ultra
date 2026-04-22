use axum::{
    body::Body,
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Auth middleware: validates x-api-key header against configured proxy key.
/// /health endpoint is exempt from authentication.
pub async fn auth_middleware(
    request: Request<Body>,
    next: Next,
) -> Response {
    // /health is always public
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    // Extract the expected API key from request extensions
    let expected_key = request
        .extensions()
        .get::<ProxyApiKey>()
        .map(|k| k.0.clone());

    let expected_key = match expected_key {
        Some(k) => k,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, "Proxy key not configured").into_response(),
    };

    // Check x-api-key header
    let provided_key = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok());

    match provided_key {
        Some(key) if key == expected_key => next.run(request).await,
        _ => (StatusCode::UNAUTHORIZED, "Unauthorized: invalid or missing x-api-key").into_response(),
    }
}

/// Extension type to pass the expected API key through request extensions.
#[derive(Clone)]
pub struct ProxyApiKey(pub String);


#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request as HttpRequest, StatusCode},
        middleware,
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    /// Build a test router with auth middleware and ProxyApiKey extension.
    /// Layer order: Extension (outer, runs first) → auth_middleware (inner).
    fn test_app(api_key: &str) -> Router {
        Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/v1/messages", axum::routing::post(|| async { "messages" }))
            .layer(middleware::from_fn(auth_middleware))
            .layer(axum::Extension(ProxyApiKey(api_key.to_string())))
    }

    #[tokio::test]
    async fn test_health_no_auth_required() {
        let app = test_app("test-key");
        let req = HttpRequest::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_missing_api_key_returns_401() {
        let app = test_app("test-key");
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/v1/messages")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_wrong_api_key_returns_401() {
        let app = test_app("test-key");
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("x-api-key", "wrong-key")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_valid_api_key_passes() {
        let app = test_app("test-key");
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("x-api-key", "test-key")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
