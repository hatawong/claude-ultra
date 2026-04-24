//! Wire-level outbound header order. Starts a TCP listener, sends a request
//! via hyper, inspects the raw bytes, and verifies the header sequence
//! matches the banded order produced by `build_outbound_headers`.
//!
//! This test also acts as a dependency-upgrade gate: the current
//! implementation of `build_outbound_headers` relies on `http::HeaderMap`
//! preserving insertion order when iterated. The `http` crate documents
//! iteration order as arbitrary, so the invariant is not contractual.
//! `Cargo.lock` pins a known-good version. If this test fails after
//! bumping `http`, `hyper`, `hyper-util`, or `reqwest`, the upgraded
//! version no longer matches what we ship on the wire; switch
//! `build_outbound_headers` to return an ordered list before continuing.

use bytes::Bytes;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

use claude_ultra_manager_lib::gateway::builder::{
    self as builder, RequestContext, EXPECTED_OUTBOUND_HEADER_ORDER,
};

#[tokio::test]
async fn test_outbound_header_order_on_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let mut client_headers = http::HeaderMap::new();
    client_headers.insert("accept", "application/json".parse().unwrap());
    client_headers.insert("accept-encoding", "gzip, deflate, br, zstd".parse().unwrap());
    client_headers.insert(
        "anthropic-beta",
        "claude-code-20250219,interleaved-thinking-2025-05-14".parse().unwrap(),
    );
    client_headers.insert("anthropic-dangerous-direct-browser-access", "true".parse().unwrap());
    client_headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
    client_headers.insert("connection", "keep-alive".parse().unwrap());
    client_headers.insert("content-type", "application/json".parse().unwrap());
    client_headers.insert("host", "localhost:9000".parse().unwrap());
    client_headers.insert("user-agent", "claude-cli/0.0.0 (subscriber, cli)".parse().unwrap());
    client_headers.insert("x-api-key", "test-key".parse().unwrap());
    client_headers.insert("x-app", "cli".parse().unwrap());
    client_headers.insert("x-claude-code-session-id", "old-session".parse().unwrap());
    client_headers.insert("x-stainless-arch", "arm64".parse().unwrap());
    client_headers.insert("x-stainless-lang", "js".parse().unwrap());
    client_headers.insert("x-stainless-os", "MacOS".parse().unwrap());
    client_headers.insert("x-stainless-package-version", "0.80.0".parse().unwrap());
    client_headers.insert("x-stainless-retry-count", "0".parse().unwrap());
    client_headers.insert("x-stainless-runtime", "node".parse().unwrap());
    client_headers.insert("x-stainless-runtime-version", "v24.3.0".parse().unwrap());
    client_headers.insert("x-stainless-timeout", "600".parse().unwrap());

    let account = RequestContext {
        device_id: "abcd".repeat(16),
        account_uuid: "uuid-test".to_string(),
        access_token: "sk-ant-oat01-test".to_string(),
        mapped_session_uuid: "aaaaaaaa-1111-2222-3333-bbbbbbbbbbbb".to_string(),
    };

    let body_bytes = b"{}";
    let outbound_headers = builder::build_outbound_headers(&client_headers, &account, body_bytes.len(), "api.anthropic.com", None);

    let headers_clone = outbound_headers.clone();
    let send_task = tokio::spawn(async move {
        use http_body_util::Full;
        use hyper_util::client::legacy::connect::HttpConnector;
        use hyper_util::client::legacy::Client;
        use hyper_util::rt::TokioExecutor;

        let http = HttpConnector::new();
        let client = Client::builder(TokioExecutor::new()).build::<_, Full<Bytes>>(http);

        let url = format!("http://127.0.0.1:{}/v1/messages?beta=true", port);
        let uri: hyper::Uri = url.parse().unwrap();
        let mut req_builder = hyper::Request::builder().method("POST").uri(uri);
        for (name, value) in headers_clone.iter() {
            req_builder = req_builder.header(name, value);
        }
        let request = req_builder
            .body(Full::new(Bytes::from_static(b"{}")))
            .unwrap();
        let _ = client.request(request).await;
    });

    let (mut stream, _) = listener.accept().await.unwrap();
    let mut buf = vec![0u8; 8192];
    let n = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read(&mut buf),
    )
    .await
    .expect("read timeout")
    .expect("read error");

    let raw = String::from_utf8_lossy(&buf[..n]);
    drop(stream);
    drop(listener);
    let _ = send_task.await;

    let lines: Vec<&str> = raw.lines().collect();
    assert!(lines[0].starts_with("POST"), "First line should be POST");

    let mut wire_headers: Vec<String> = Vec::new();
    for line in &lines[1..] {
        if line.is_empty() {
            break;
        }
        if let Some(colon_pos) = line.find(':') {
            wire_headers.push(line[..colon_pos].to_lowercase());
        }
    }

    // `transfer-encoding` is injected by hyper and is not part of the
    // application-controlled header set.
    let filtered: Vec<String> = wire_headers
        .iter()
        .filter(|h| *h != "transfer-encoding")
        .cloned()
        .collect();

    let expected: Vec<String> = EXPECTED_OUTBOUND_HEADER_ORDER
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(filtered, expected);
}

#[test]
fn test_vercel_host_header() {
    let account = RequestContext {
        device_id: "dev-id".to_string(),
        account_uuid: "uuid-test".to_string(),
        access_token: "test-token".to_string(),
        mapped_session_uuid: "sess-uuid".to_string(),
    };
    let mut client_headers = http::HeaderMap::new();
    client_headers.insert("user-agent", "claude-cli/0.0.0 (external, cli)".parse().unwrap());
    client_headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
    client_headers.insert("x-app", "cli".parse().unwrap());

    let headers = builder::build_outbound_headers(&client_headers, &account, 100, "ai-gateway.vercel.sh", Some("vck_test_key"));
    assert_eq!(
        headers.get("host").unwrap().to_str().unwrap(),
        "ai-gateway.vercel.sh",
    );

    let vck = headers.get("x-ai-gateway-api-key").expect("missing x-ai-gateway-api-key");
    assert!(vck.to_str().unwrap().starts_with("Bearer vck_"));

    let names: Vec<String> = headers.iter().map(|(n, _)| n.as_str().to_string()).collect();
    let session_pos = names.iter().position(|n| n == "x-claude-code-session-id").unwrap();
    let vck_pos = names.iter().position(|n| n == "x-ai-gateway-api-key").unwrap();
    let app_pos = names.iter().position(|n| n == "x-app").unwrap();
    assert!(session_pos < vck_pos, "session id should come before vck in the banded order");
    assert!(vck_pos < app_pos, "vck should come before x-app alphabetically within its band");

    let headers_anthropic = builder::build_outbound_headers(&client_headers, &account, 100, "api.anthropic.com", None);
    assert_eq!(
        headers_anthropic.get("host").unwrap().to_str().unwrap(),
        "api.anthropic.com",
    );
    assert!(headers_anthropic.get("x-ai-gateway-api-key").is_none());
}
