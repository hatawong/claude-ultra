//! Wire-level test: verifies outbound headers are sent in alphabetical order.
//! Starts a TCP listener, sends a request via hyper, captures raw bytes.

use bytes::Bytes;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

use claude_ultra_manager_lib::gateway::builder::{self as builder, AccountIdentity};

#[tokio::test]
async fn test_header_wire_order_is_alphabetical() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    // Build outbound headers using our fingerprint module
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
    client_headers.insert("user-agent", "claude-cli/2.1.92 (subscriber, cli)".parse().unwrap());
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

    let account = AccountIdentity {
        access_token: "sk-ant-oat01-test".to_string(),
        session_uuid: "aaaaaaaa-1111-2222-3333-bbbbbbbbbbbb".to_string(),
        device_id: "abcd".repeat(16),
        account_uuid: "uuid-test".to_string(),
    };

    let body_bytes = b"{}";
    let outbound_headers = builder::build_outbound_headers(&client_headers, &account, body_bytes.len());

    // Spawn client request
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

    // Accept and read with timeout
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

    println!("=== Raw HTTP request on wire ===");
    println!("{}", raw);

    // Parse header names in wire order
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

    println!("\n=== Wire header order ===");
    for (i, name) in wire_headers.iter().enumerate() {
        println!("  {}. {}", i + 1, name);
    }

    // Filter out headers hyper adds automatically (transfer-encoding)
    // content-length is now part of our sorted headers
    let our_headers: Vec<String> = wire_headers
        .iter()
        .filter(|h| *h != "transfer-encoding")
        .cloned()
        .collect();
    let mut expected = our_headers.clone();
    expected.sort();

    assert_eq!(
        our_headers, expected,
        "\nHeaders NOT in alphabetical order on wire!\nWire:   {:?}\nSorted: {:?}",
        our_headers, expected
    );

    println!("\nWire header order: ALPHABETICAL CONFIRMED");
}
