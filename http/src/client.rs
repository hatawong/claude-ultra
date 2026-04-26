//! HTTP client using BoringSSL for TLS.
//!
//! Automatic gzip/deflate/br decompression of responses when Content-Encoding is present.
//! Supports both direct and proxied connections.

use boring::ssl::SslConnector;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::client::conn::http1::SendRequest;
use hyper_boring::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::time::Duration;

use crate::error::{ConnectStep, Error};
use crate::tls;

/// Per-step timing breakdown for proxy connection establishment.
#[derive(Debug, Clone)]
pub struct ConnectTimings {
    /// Step 1: TCP connect to proxy server (ms).
    pub tcp_ms: u64,
    /// Step 2: CONNECT tunnel establishment (ms).
    pub tunnel_ms: u64,
    /// Step 3: TLS handshake to target through tunnel (ms).
    pub tls_ms: u64,
    /// Step 4: HTTP/1.1 handshake (ms).
    pub http_ms: u64,
}

impl ConnectTimings {
    /// Total connection time (all 4 steps).
    pub fn total_ms(&self) -> u64 {
        self.tcp_ms + self.tunnel_ms + self.tls_ms + self.http_ms
    }
}

// ─── BoringClient ────────────────────────────────────────────

/// HTTP client with BoringSSL TLS.
///
/// Modeled after reqwest: `BoringClient::builder() → BoringClientBuilder → .build()`.
///
/// The client holds a connection pool internally. Clone and reuse freely.
pub struct BoringClient {
    pub(crate) timeout: Duration,
    /// Step 1 timeout: TCP connect to proxy server (default: 10s).
    pub(crate) proxy_tcp_timeout: Duration,
    /// Step 2 timeout: CONNECT tunnel establishment (default: 10s).
    pub(crate) proxy_tunnel_timeout: Duration,
    /// Step 3 timeout: TLS handshake over tunnel (default: 10s).
    pub(crate) proxy_tls_timeout: Duration,
    /// Direct (no proxy) client with connection pooling.
    direct_client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    /// BoringSSL connector for proxy tunnel TLS handshakes.
    ssl_connector: SslConnector,
}

impl BoringClient {
    /// Create a `BoringClientBuilder` to configure a new client.
    pub fn builder() -> BoringClientBuilder {
        BoringClientBuilder {
            timeout: Duration::from_secs(300),
            pool_idle_timeout: Duration::from_secs(90),
            proxy_tcp_timeout: Duration::from_secs(3),
            proxy_tunnel_timeout: Duration::from_secs(3),
            proxy_tls_timeout: Duration::from_secs(3),
        }
    }

    /// Start building a request (reqwest-style).
    pub fn request(&self, method: http::Method, url: &str) -> RequestBuilder<'_> {
        RequestBuilder {
            client: self,
            method,
            url: url.to_string(),
            headers: http::HeaderMap::new(),
            body: Bytes::new(),
            proxy_url: None,
            cc_version: None,
            cch_offset: None,
        }
    }

    /// Convenience: start a GET request.
    pub fn get(&self, url: &str) -> RequestBuilder<'_> {
        self.request(http::Method::GET, url)
    }

    /// Convenience: start a POST request.
    pub fn post(&self, url: &str) -> RequestBuilder<'_> {
        self.request(http::Method::POST, url)
    }

    /// Convenience: start a PUT request.
    pub fn put(&self, url: &str) -> RequestBuilder<'_> {
        self.request(http::Method::PUT, url)
    }

    /// Convenience: start a DELETE request.
    pub fn delete(&self, url: &str) -> RequestBuilder<'_> {
        self.request(http::Method::DELETE, url)
    }

    /// Build an HTTP/1.1 request for sending over a proxy tunnel.
    /// Uses path+query only (not full URL) since we're on an established connection.
    fn build_proxy_http_request(
        &self,
        method: http::Method,
        uri: &hyper::Uri,
        headers: &http::HeaderMap,
        body: Bytes,
    ) -> Result<hyper::Request<Full<Bytes>>, Error> {
        let mut req_builder = hyper::Request::builder().method(method).uri(
            uri.path_and_query()
                .map(|pq| pq.as_str())
                .unwrap_or("/"),
        );
        for (name, value) in headers.iter() {
            req_builder = req_builder.header(name, value);
        }
        req_builder
            .body(Full::new(body))
            .map_err(|e| Error::connect(format!("Request build failed: {}", e)))
    }

    // ── Internal: direct path ────────────────────────────────

    async fn send_direct(
        &self,
        method: http::Method,
        uri: hyper::Uri,
        headers: http::HeaderMap,
        body: Bytes,
    ) -> Result<http::Response<Incoming>, Error> {
        let mut req_builder = hyper::Request::builder().method(method).uri(&uri);
        for (name, value) in headers.iter() {
            req_builder = req_builder.header(name, value);
        }

        let request = req_builder
            .body(Full::new(body))
            .map_err(|e| Error::connect(format!("Failed to build request: {}", e)))?;

        let result = tokio::time::timeout(self.timeout, self.direct_client.request(request)).await;

        match result {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => Err(Error::classify_hyper_error(&e)),
            Err(_) => Err(Error::timeout()),
        }
    }

    // ── Internal: proxy path ─────────────────────────────────

    /// Establish a proxy connection: TCP → CONNECT → TLS → HTTP/1.1 handshake.
    /// Returns the sender, conn driver handle, and per-step timing breakdown.
    /// Public for use by the connection pool pre-warming logic.
    pub async fn establish_proxy_connection(
        &self,
        proxy_url: &str,
        target_host: &str,
        target_port: u16,
    ) -> Result<(SendRequest<Full<Bytes>>, tokio::task::JoinHandle<()>, ConnectTimings), Error> {
        // SOCKS5 connector requires additional dependencies not enabled here.
        // Surface a clear error so operators see "socks5 not supported in this build"
        // instead of a misleading CONNECT failure.
        if proxy_url.starts_with("socks5://") || proxy_url.starts_with("socks5h://") {
            return Err(Error::tunnel("socks5 not supported in this build").with_step(ConnectStep::Tunnel));
        }

        let proxy_uri: http::Uri = proxy_url
            .parse()
            .map_err(|e| Error::connect(format!("Invalid proxy URL: {}", e)))?;

        let proxy_host = proxy_uri
            .host()
            .ok_or_else(|| Error::connect("Proxy URL missing host"))?;
        let proxy_port = proxy_uri.port_u16().unwrap_or(8080);

        let proxy_auth = proxy_uri.authority().and_then(|auth| {
            let auth_str = auth.as_str();
            auth_str.rfind('@').map(|at_pos| auth_str[..at_pos].to_string())
        });

        // Step 1: TCP connect to proxy
        let step1_start = std::time::Instant::now();
        let tcp_stream = tokio::time::timeout(
            self.proxy_tcp_timeout,
            tokio::net::TcpStream::connect(format!("{}:{}", proxy_host, proxy_port)),
        )
        .await
        .map_err(|_| Error::connect("Proxy connect timeout").with_step(ConnectStep::TcpConnect))?
        .map_err(|e| Error::connect(format!("Proxy connect failed: {}", e)).with_step(ConnectStep::TcpConnect))?;
        let tcp_ms = step1_start.elapsed().as_millis() as u64;
        tracing::debug!("[PROXY] tcp_connect={}ms to {}:{}", tcp_ms, proxy_host, proxy_port);

        // Step 2: CONNECT tunnel
        let step2_start = std::time::Instant::now();
        let connect_request = if let Some(ref auth) = proxy_auth {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(auth);
            format!(
                "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\nProxy-Authorization: Basic {}\r\n\r\n",
                target_host, target_port, target_host, target_port, encoded
            )
        } else {
            format!(
                "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n\r\n",
                target_host, target_port, target_host, target_port
            )
        };

        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        tcp_stream
            .writable()
            .await
            .map_err(|e| Error::tunnel(format!("Stream not writable: {}", e)).with_step(ConnectStep::Tunnel))?;

        let mut stream = tcp_stream;
        stream
            .write_all(connect_request.as_bytes())
            .await
            .map_err(|e| Error::tunnel(format!("CONNECT write failed: {}", e)).with_step(ConnectStep::Tunnel))?;

        let mut response_buf = vec![0u8; 4096];
        let mut total_read = 0;
        let deadline = tokio::time::Instant::now() + self.proxy_tunnel_timeout;

        loop {
            if total_read >= response_buf.len() {
                return Err(Error::tunnel("CONNECT response too large").with_step(ConnectStep::Tunnel));
            }
            let n = tokio::time::timeout_at(
                deadline,
                stream.read(&mut response_buf[total_read..]),
            )
            .await
            .map_err(|_| Error::tunnel("CONNECT response timeout").with_step(ConnectStep::Tunnel))?
            .map_err(|e| Error::tunnel(format!("CONNECT read failed: {}", e)).with_step(ConnectStep::Tunnel))?;

            if n == 0 {
                return Err(Error::tunnel(
                    "CONNECT: connection closed before headers complete",
                ).with_step(ConnectStep::Tunnel));
            }
            total_read += n;

            if response_buf[..total_read]
                .windows(4)
                .any(|w| w == b"\r\n\r\n")
            {
                break;
            }
        }

        let response_str = String::from_utf8_lossy(&response_buf[..total_read]);
        let first_line = response_str.lines().next().unwrap_or("");
        let status_ok =
            first_line.starts_with("HTTP/1.1 200") || first_line.starts_with("HTTP/1.0 200");

        if !status_ok {
            if first_line.contains(" 407") {
                return Err(Error::proxy_auth(format!(
                    "Proxy auth failed: {}",
                    response_str.trim()
                )).with_step(ConnectStep::Tunnel));
            }
            return Err(Error::tunnel(format!(
                "CONNECT failed: {}",
                response_str.trim()
            )).with_step(ConnectStep::Tunnel));
        }

        let tunnel_ms = step2_start.elapsed().as_millis() as u64;
        tracing::debug!("[PROXY] connect_tunnel={}ms to {}:{}", tunnel_ms, target_host, target_port);

        // Step 3: TLS handshake over tunnel
        let step3_start = std::time::Instant::now();
        let ssl = boring::ssl::Ssl::new(self.ssl_connector.context())
            .map_err(|e| Error::tls(format!("SSL new failed: {}", e)).with_step(ConnectStep::TlsHandshake))?;

        let mut ssl_ref = ssl;
        ssl_ref
            .set_hostname(target_host)
            .map_err(|e| Error::tls(format!("SNI set failed: {}", e)).with_step(ConnectStep::TlsHandshake))?;
        ssl_ref.set_enable_ech_grease(true);

        let tls_stream = tokio::time::timeout(self.proxy_tls_timeout, async {
            tokio_boring::SslStreamBuilder::new(ssl_ref, stream)
                .connect()
                .await
        })
        .await
        .map_err(|_| Error::tls("TLS handshake timeout").with_step(ConnectStep::TlsHandshake))?
        .map_err(|e| {
            let err_str = format!("{}", e);
            if err_str.contains("certificate") || err_str.contains("CERTIFICATE") {
                Error::tls_certificate(err_str).with_step(ConnectStep::TlsHandshake)
            } else {
                Error::tls(err_str).with_step(ConnectStep::TlsHandshake)
            }
        })?;

        let tls_ms = step3_start.elapsed().as_millis() as u64;
        tracing::debug!("[PROXY] tls_handshake={}ms to {}", tls_ms, target_host);

        // Step 4: HTTP/1.1 over TLS
        let step4_start = std::time::Instant::now();
        let io = hyper_util::rt::TokioIo::new(tls_stream);
        let (sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .map_err(|e| Error::connect(format!("HTTP handshake failed: {}", e)).with_step(ConnectStep::HttpHandshake))?;
        let http_ms = step4_start.elapsed().as_millis() as u64;

        let conn_handle = tokio::spawn(async move {
            if let Err(e) = conn.await {
                if !e.is_closed() {
                    tracing::debug!("Proxy connection ended: {}", e);
                }
            }
        });

        let timings = ConnectTimings { tcp_ms, tunnel_ms, tls_ms, http_ms };
        tracing::debug!("[PROXY] total={}ms (tcp={} tunnel={} tls={} http={})",
            timings.total_ms(), tcp_ms, tunnel_ms, tls_ms, http_ms);

        Ok((sender, conn_handle, timings))
    }

    /// Stub: send a request on a pre-established sender. Source builds
    /// cannot produce valid upstream requests; use the official `.dmg`
    /// for production.
    pub async fn send_on_sender(
        &self,
        sender: &mut SendRequest<Full<Bytes>>,
        method: http::Method,
        uri: hyper::Uri,
        headers: http::HeaderMap,
        body: Bytes,
        _cc_version: Option<&semver::Version>,
        _cch_offset: Option<usize>,
    ) -> Result<http::Response<Incoming>, Error> {
        let request = self.build_proxy_http_request(method, &uri, &headers, body)?;
        let result = tokio::time::timeout(self.timeout, sender.send_request(request)).await;
        match result {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => {
                let err_str = format!("{}", e);
                if err_str.contains("timeout") || err_str.contains("timed out") {
                    Err(Error::timeout())
                } else {
                    Err(Error::connect(err_str))
                }
            }
            Err(_) => Err(Error::timeout()),
        }
    }

    async fn send_via_proxy(
        &self,
        method: http::Method,
        uri: hyper::Uri,
        headers: http::HeaderMap,
        body: Bytes,
        proxy_url: &str,
    ) -> Result<http::Response<Incoming>, Error> {
        let target_host = uri.host().unwrap_or("api.anthropic.com");
        let target_port = uri.port_u16().unwrap_or(443);

        let (mut sender, _conn_handle, _timings) = self
            .establish_proxy_connection(proxy_url, target_host, target_port)
            .await?;

        let request = self.build_proxy_http_request(method, &uri, &headers, body)?;

        let result = tokio::time::timeout(self.timeout, sender.send_request(request)).await;

        match result {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => {
                let err_str = format!("{}", e);
                if err_str.contains("timeout") || err_str.contains("timed out") {
                    Err(Error::timeout())
                } else {
                    Err(Error::connect(err_str))
                }
            }
            Err(_) => Err(Error::timeout()),
        }
    }
}

// ─── BoringClientBuilder ─────────────────────────────────────

/// Builder for configuring a `BoringClient`.
///
/// Modeled after reqwest::ClientBuilder.
pub struct BoringClientBuilder {
    timeout: Duration,
    pool_idle_timeout: Duration,
    proxy_tcp_timeout: Duration,
    proxy_tunnel_timeout: Duration,
    proxy_tls_timeout: Duration,
}

impl BoringClientBuilder {
    /// Set the request timeout (default: 300s).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the connection pool idle timeout (default: 90s).
    pub fn pool_idle_timeout(mut self, timeout: Duration) -> Self {
        self.pool_idle_timeout = timeout;
        self
    }

    /// Set TCP connect to proxy timeout (default: 10s).
    pub fn proxy_tcp_timeout(mut self, timeout: Duration) -> Self {
        self.proxy_tcp_timeout = timeout;
        self
    }

    /// Set CONNECT tunnel establishment timeout (default: 10s).
    pub fn proxy_tunnel_timeout(mut self, timeout: Duration) -> Self {
        self.proxy_tunnel_timeout = timeout;
        self
    }

    /// Set TLS handshake over tunnel timeout (default: 10s).
    pub fn proxy_tls_timeout(mut self, timeout: Duration) -> Self {
        self.proxy_tls_timeout = timeout;
        self
    }

    /// Build the client. Returns an error if TLS initialization fails.
    pub fn build(self) -> Result<BoringClient, Error> {
        // Direct path: HttpsConnector with BoringSSL
        let builder_for_direct = tls::build_ssl_config();
        let mut http = HttpConnector::new();
        http.enforce_http(false);

        let mut connector = HttpsConnector::with_connector(http, builder_for_direct)
            .map_err(|e| Error::tls(format!("Failed to build HttpsConnector: {}", e)))?;

        connector.set_ssl_callback(|ssl, _uri| {
            ssl.set_enable_ech_grease(true);
            Ok(())
        });

        let direct_client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(self.pool_idle_timeout)
            .build(connector);

        // Proxy path: standalone SslConnector for tunnel TLS
        let ssl_connector = tls::build_ssl_config().build();

        Ok(BoringClient {
            timeout: self.timeout,
            proxy_tcp_timeout: self.proxy_tcp_timeout,
            proxy_tunnel_timeout: self.proxy_tunnel_timeout,
            proxy_tls_timeout: self.proxy_tls_timeout,
            direct_client,
            ssl_connector,
        })
    }
}

// ─── RequestBuilder ──────────────────────────────────────────

/// Builder for configuring and sending a single HTTP request.
///
/// Modeled after reqwest::RequestBuilder.
pub struct RequestBuilder<'a> {
    client: &'a BoringClient,
    method: http::Method,
    url: String,
    pub(crate) headers: http::HeaderMap,
    body: Bytes,
    proxy_url: Option<String>,
    cc_version: Option<semver::Version>,
    cch_offset: Option<usize>,
}

impl<'a> RequestBuilder<'a> {
    /// Set the request headers (replaces any previously set).
    pub fn headers(mut self, headers: http::HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Add a single header.
    pub fn header(mut self, name: http::header::HeaderName, value: http::header::HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Set the request body.
    pub fn body(mut self, body: Bytes) -> Self {
        self.body = body;
        self
    }

    /// Route the request through a proxy.
    pub fn proxy(mut self, proxy_url: &str) -> Self {
        self.proxy_url = Some(proxy_url.to_string());
        self
    }

    /// Stub: accepts a CC CLI version tag and offset, performs no additional
    /// processing. Source builds cannot produce valid upstream requests;
    /// use the official `.dmg` for production.
    pub fn cc_cli_version(mut self, version: &semver::Version, offset: Option<usize>) -> Self {
        self.cc_version = Some(version.clone());
        self.cch_offset = offset;
        self
    }

    /// Send the request.
    /// Response body is automatically decompressed based on Content-Encoding (gzip/deflate/br).
    ///
    /// **Stub safety**: source builds that carry unsigned request markers are
    /// rejected before reaching the network. Use the official `.dmg` for
    /// production.
    pub async fn send(mut self) -> Result<http::Response<crate::decode::DecodedBody>, Error> {
        // Stub guard: reject source-built requests (see README for build notes).
        if !self.body.is_empty() {
            if let Ok(text) = std::str::from_utf8(&self.body) {
                let trigger_guard = serde_json::from_str::<serde_json::Value>(text)
                    .ok()
                    .and_then(|v| v.get("system").cloned())
                    .and_then(|sys| {
                        if let serde_json::Value::Array(arr) = sys {
                            arr.iter().any(|item| {
                                item.get("text")
                                    .and_then(|t| t.as_str())
                                    .map(|s| s.contains("cch=00000"))
                                    .unwrap_or(false)
                            }).then_some(true)
                        } else if let serde_json::Value::String(s) = sys {
                            s.contains("cch=00000").then_some(true)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(false);
                if trigger_guard {
                    return Err(Error::connect(
                        "[Claude Ultra] Source-built stub cannot execute this request. \
                         Use the official .dmg build. See README for download instructions."
                    ));
                }
            }
        }

        let uri: hyper::Uri = self
            .url
            .parse()
            .map_err(|e| Error::connect(format!("Invalid URL: {}", e)))?;

        // Auto-fill Host header from URL if caller didn't set it
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

        let raw_resp = match self.proxy_url {
            None => {
                self.client
                    .send_direct(self.method, uri, self.headers, self.body)
                    .await?
            }
            Some(ref proxy) => {
                self.client
                    .send_via_proxy(self.method, uri, self.headers, self.body, proxy)
                    .await?
            }
        };

        // Auto-decompress based on Content-Encoding
        Ok(crate::decode::decode_response(raw_resp))
    }
}

// ─── Backward compatibility ──────────────────────────────────

/// Type alias — legacy code may reference UpstreamClient.
pub type UpstreamClient = BoringClient;
