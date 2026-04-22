//! Error type for claude-ultra-http.
//!
//! Modeled after reqwest::Error:
//! - Opaque struct (pointer-sized via Box<Inner>)
//! - Classification methods: is_connect(), is_tls(), is_proxy(), is_timeout(), is_status(), is_body()
//! - URL context: with_url(), without_url(), url()
//! - std::error::Error source chain

use std::error::Error as StdError;
use std::fmt;

type BoxError = Box<dyn StdError + Send + Sync>;

/// The errors that may occur when processing an HTTP request.
///
/// Use `is_*()` methods to classify errors without matching on internals.
/// Errors may carry the URL that caused them — use `without_url()` to strip
/// sensitive information before logging.
pub struct Error {
    inner: Box<Inner>,
}

struct Inner {
    kind: Kind,
    source: Option<BoxError>,
    url: Option<String>,
    step: Option<ConnectStep>,
}

/// Which step of proxy connection establishment failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectStep {
    /// Step 1: TCP connect to proxy server.
    TcpConnect,
    /// Step 2: CONNECT tunnel establishment.
    Tunnel,
    /// Step 3: TLS handshake over tunnel.
    TlsHandshake,
    /// Step 4: HTTP/1.1 handshake.
    HttpHandshake,
}

#[derive(Debug)]
enum Kind {
    /// TCP connection failed (refused, unreachable, DNS).
    Connect,
    /// TLS handshake failed (not certificate-related).
    Tls,
    /// TLS certificate verification failed (not retryable).
    TlsCertificate,
    /// Proxy authentication failed (407).
    ProxyAuth,
    /// CONNECT tunnel establishment failed.
    Tunnel,
    /// Request/response timeout.
    Timeout,
    /// HTTP error status from upstream.
    Status(u16),
    /// Response body error (stream closed, interrupted).
    Body,
    /// License verification error.
    License(crate::license::LicenseError),
}

// ─── Constructors ───────────────────────────────────────────

impl Error {
    fn new(kind: Kind, message: impl Into<String>) -> Self {
        let msg = message.into();
        let source: Option<BoxError> = if msg.is_empty() {
            None
        } else {
            Some(msg.into())
        };
        Error {
            inner: Box::new(Inner {
                kind,
                source,
                url: None,
                step: None,
            }),
        }
    }

    /// Connection failed (TCP level).
    pub fn connect(message: impl Into<String>) -> Self {
        Self::new(Kind::Connect, message)
    }

    /// TLS handshake failed.
    pub fn tls(message: impl Into<String>) -> Self {
        Self::new(Kind::Tls, message)
    }

    /// TLS certificate verification failed (not retryable).
    pub fn tls_certificate(message: impl Into<String>) -> Self {
        Self::new(Kind::TlsCertificate, message)
    }

    /// Proxy authentication failed (407).
    pub fn proxy_auth(message: impl Into<String>) -> Self {
        Self::new(Kind::ProxyAuth, message)
    }

    /// CONNECT tunnel failed.
    pub fn tunnel(message: impl Into<String>) -> Self {
        Self::new(Kind::Tunnel, message)
    }

    /// Request/response timeout.
    pub fn timeout() -> Self {
        Self::new(Kind::Timeout, "operation timed out")
    }

    /// HTTP error status.
    pub fn status(code: u16, message: impl Into<String>) -> Self {
        let msg = message.into();
        let source: Option<BoxError> = if msg.is_empty() {
            None
        } else {
            Some(msg.into())
        };
        Error {
            inner: Box::new(Inner {
                kind: Kind::Status(code),
                source,
                url: None,
                step: None,
            }),
        }
    }

    /// Response body / stream error.
    pub fn body(message: impl Into<String>) -> Self {
        Self::new(Kind::Body, message)
    }

    /// License verification error.
    pub fn license(le: crate::license::LicenseError) -> Self {
        let msg = le.to_string();
        Error {
            inner: Box::new(Inner {
                kind: Kind::License(le),
                source: Some(msg.into()),
                url: None,
                step: None,
            }),
        }
    }
}

// ─── Classification (reqwest-style is_*() methods) ──────────

impl Error {
    /// Returns true if the error is related to connecting (TCP, DNS, proxy, tunnel).
    pub fn is_connect(&self) -> bool {
        matches!(
            self.inner.kind,
            Kind::Connect | Kind::ProxyAuth | Kind::Tunnel
        )
    }

    /// Returns true if the error is TLS-related (handshake or certificate).
    pub fn is_tls(&self) -> bool {
        matches!(self.inner.kind, Kind::Tls | Kind::TlsCertificate)
    }

    /// Returns true if the error is proxy-specific (auth or tunnel).
    pub fn is_proxy(&self) -> bool {
        matches!(self.inner.kind, Kind::ProxyAuth | Kind::Tunnel)
    }

    /// Returns true if the error is a timeout.
    pub fn is_timeout(&self) -> bool {
        matches!(self.inner.kind, Kind::Timeout)
    }

    /// Returns true if the error is an HTTP status error.
    pub fn is_status(&self) -> bool {
        matches!(self.inner.kind, Kind::Status(_))
    }

    /// Returns true if the error is a response body error.
    pub fn is_body(&self) -> bool {
        matches!(self.inner.kind, Kind::Body)
    }

    /// Returns the HTTP status code, if this is a status error.
    pub fn status_code(&self) -> Option<u16> {
        match self.inner.kind {
            Kind::Status(code) => Some(code),
            _ => None,
        }
    }

    /// Returns true if the error is a license error.
    pub fn is_license(&self) -> bool {
        matches!(self.inner.kind, Kind::License(_))
    }

    /// Returns the license error details, if this is a license error.
    pub fn license_error(&self) -> Option<&crate::license::LicenseError> {
        match &self.inner.kind {
            Kind::License(le) => Some(le),
            _ => None,
        }
    }

    /// Returns true if the error is potentially retryable.
    ///
    /// TLS certificate, body/stream, and license errors are NOT retryable.
    /// Everything else (connect, TLS handshake, proxy, timeout, HTTP status) is.
    pub fn is_retryable(&self) -> bool {
        !matches!(self.inner.kind, Kind::TlsCertificate | Kind::Body | Kind::License(_))
    }
}

// ─── URL context (reqwest-style) ────────────────────────────

impl Error {
    /// Returns the URL associated with this error, if any.
    pub fn url(&self) -> Option<&str> {
        self.inner.url.as_deref()
    }

    /// Add a URL to this error (overwrites any existing).
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.inner.url = Some(url.into());
        self
    }

    /// Strip the URL from this error (e.g. to remove sensitive info before logging).
    pub fn without_url(mut self) -> Self {
        self.inner.url = None;
        self
    }

    /// Returns the proxy connection step where the error occurred, if any.
    pub fn step(&self) -> Option<ConnectStep> {
        self.inner.step
    }

    /// Tag this error with the proxy connection step where it occurred.
    pub fn with_step(mut self, step: ConnectStep) -> Self {
        self.inner.step = Some(step);
        self
    }
}

// ─── String-based error classification ──────────────────────

impl Error {
    /// Classify an error from a string message.
    /// Inspects common patterns to determine the error kind.
    /// Order matters: more specific checks first.
    pub fn classify_str(err: &str) -> Self {
        let lower = err.to_lowercase();

        // Certificate errors (most specific TLS error)
        if lower.contains("certificate") || lower.contains("cert verify") {
            return Self::tls_certificate(err.to_string());
        }

        // Proxy auth (407) — before general "connect" check
        if lower.contains("407") || lower.contains("proxy auth") {
            return Self::proxy_auth(err.to_string());
        }

        // TLS/SSL errors
        if lower.contains("ssl") || lower.contains("tls") || lower.contains("handshake") {
            return Self::tls(err.to_string());
        }

        // Timeout
        if lower.contains("timeout") || lower.contains("timed out") {
            return Self::timeout();
        }

        // Tunnel/connect failures (general — after more specific checks)
        if lower.contains("tunnel") {
            return Self::tunnel(err.to_string());
        }

        // Default: connect error
        Self::connect(err.to_string())
    }

    /// Classify a hyper client error.
    /// Inspects the error chain to determine the failure stage.
    pub fn classify_hyper_error(err: &hyper_util::client::legacy::Error) -> Self {
        let err_str = format!("{}", err);
        let source_chain = format!("{:?}", err);

        if err.is_connect() {
            // TLS errors in connect phase
            if source_chain.contains("ssl")
                || source_chain.contains("SSL")
                || source_chain.contains("tls")
                || source_chain.contains("TLS")
            {
                if source_chain.contains("certificate")
                    || source_chain.contains("CERTIFICATE")
                    || source_chain.contains("verify")
                {
                    return Self::tls_certificate(err_str);
                }
                return Self::tls(err_str);
            }

            // Tunnel failure
            if source_chain.contains("tunnel") || source_chain.contains("CONNECT") {
                return Self::tunnel(err_str);
            }

            // Proxy auth
            if source_chain.contains("407") || source_chain.contains("auth") {
                return Self::proxy_auth(err_str);
            }

            // Default connect failure
            return Self::connect(err_str);
        }

        // Non-connect errors: inspect string patterns
        let lower = err_str.to_lowercase();
        if lower.contains("timeout") || lower.contains("timed out") {
            return Self::timeout();
        }

        if lower.contains("broken pipe")
            || lower.contains("connection reset")
            || lower.contains("closed")
        {
            return Self::body("stream closed".to_string());
        }

        // Default
        Self::connect(err_str)
    }
}

// ─── Display ────────────────────────────────────────────────

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner.kind {
            Kind::Connect => write!(f, "connection error")?,
            Kind::Tls => write!(f, "TLS handshake error")?,
            Kind::TlsCertificate => write!(f, "TLS certificate error")?,
            Kind::ProxyAuth => write!(f, "proxy authentication error")?,
            Kind::Tunnel => write!(f, "tunnel connection error")?,
            Kind::Timeout => write!(f, "request timeout")?,
            Kind::Status(code) => write!(f, "HTTP status error ({})", code)?,
            Kind::Body => write!(f, "response body error")?,
            Kind::License(ref le) => write!(f, "license error: {}", le)?,
        }

        if let Some(step) = self.inner.step {
            let step_name = match step {
                ConnectStep::TcpConnect => "tcp_connect",
                ConnectStep::Tunnel => "tunnel",
                ConnectStep::TlsHandshake => "tls_handshake",
                ConnectStep::HttpHandshake => "http_handshake",
            };
            write!(f, " [{}]", step_name)?;
        }

        if let Some(ref source) = self.inner.source {
            write!(f, ": {}", source)?;
        }

        if let Some(ref url) = self.inner.url {
            write!(f, " for url ({})", url)?;
        }

        Ok(())
    }
}

// ─── Debug ──────────────────────────────────────────────────

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("Error");
        builder.field("kind", &self.inner.kind);
        if let Some(step) = self.inner.step {
            builder.field("step", &step);
        }
        if let Some(ref url) = self.inner.url {
            builder.field("url", url);
        }
        if let Some(ref source) = self.inner.source {
            builder.field("source", source);
        }
        builder.finish()
    }
}

// ─── std::error::Error ─────────────────────────────────────

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.inner.source.as_ref().map(|e| &**e as _)
    }
}
