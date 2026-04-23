//! CliClient — Anthropic OAuth/API client
//!
//! Wraps all Anthropic API interactions.
//! Two HTTP header modes:
//! - OAuth headers (4 headers): token exchange / refresh / profile
//! - Messages headers (22 headers): /v1/messages requests

use bytes::Bytes;
use claude_ultra_http::{BoringClient, DecodedBody};
use http::{HeaderMap, HeaderValue, Method};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// CliClient error type
#[derive(Debug)]
pub enum CliClientError {
    Http(claude_ultra_http::Error),
    HttpStatus(u16, String),
    ParseError(String),
}

impl std::fmt::Display for CliClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {}", e),
            Self::HttpStatus(code, msg) => write!(f, "HTTP {}: {}", code, msg),
            Self::ParseError(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

impl From<claude_ultra_http::Error> for CliClientError {
    fn from(e: claude_ultra_http::Error) -> Self {
        Self::Http(e)
    }
}

// ─── Constants ──────────────────────────────────────────

const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const DEFAULT_SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// Token refresh buffer (milliseconds before expiry)
const REFRESH_BUFFER_MS: i64 = 5 * 60 * 1000;

/// Fallback version when `claude --version` fails.
use crate::gateway::policy::MAX_SUPPORTED_VERSION;

/// Global cached client version — detected once, reused forever.
static CC_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Get cached client version (detects on first call).
pub fn get_cc_version() -> &'static str {
    CC_VERSION.get_or_init(detect_cc_version)
}

fn detect_cc_version() -> String {
    let detected = match std::process::Command::new("claude").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            match parse_cc_version(&stdout) {
                Some(v) => v,
                None => MAX_SUPPORTED_VERSION.to_string(),
            }
        }
        _ => MAX_SUPPORTED_VERSION.to_string(),
    };
    // Limit to max supported version.
    if version_gt(&detected, MAX_SUPPORTED_VERSION) {
        MAX_SUPPORTED_VERSION.to_string()
    } else {
        detected
    }
}

fn version_gt(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> (u64, u64, u64) {
        let mut it = s.split('.');
        let major = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    };
    parse(a) > parse(b)
}

/// Map client version to bundled Anthropic SDK version.
pub fn sdk_version_for_cc(_cc_version: &str) -> &'static str {
    // All known versions (2.1.94-2.1.111) use SDK 0.81.0
    "0.81.0"
}

/// Parse version from `claude --version` output.
fn parse_cc_version(output: &str) -> Option<String> {
    let trimmed = output.trim();
    if let Some(rest) = trimmed.strip_prefix("claude-cli/") {
        let version = rest.split_whitespace().next()?;
        if version.contains('.') {
            return Some(version.to_string());
        }
    }
    let first_word = trimmed.split_whitespace().next()?;
    if first_word.chars().next()?.is_ascii_digit() && first_word.contains('.') {
        return Some(first_word.to_string());
    }
    None
}

// ─── Data types ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenResult {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub expires_in: i64,
    pub token_type: String,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
    pub scopes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileInfo {
    pub account_uuid: String,
    pub email: String,
    pub display_name: Option<String>,
    pub organization_uuid: String,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
    pub billing_type: Option<String>,
    pub has_extra_usage_enabled: bool,
    pub account_created_at: Option<String>,
    pub subscription_created_at: Option<String>,
}

/// Quota information parsed from /v1/messages response headers.
#[derive(Debug, Clone)]
pub struct QuotaHeaders {
    pub utilization_5h: f64,
    pub utilization_7d: Option<f64>,
    pub reset_5h: Option<i64>,
}

/// Rate limit window from /api/oauth/usage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RateLimit {
    pub utilization: Option<f64>,       // 0-100 percentage, null if not available
    pub resets_at: Option<String>,      // ISO 8601, null if not available
}

/// Extra usage info from /api/oauth/usage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
}

/// Full response from GET /api/oauth/usage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Utilization {
    pub five_hour: Option<RateLimit>,
    pub seven_day: Option<RateLimit>,
    pub seven_day_sonnet: Option<RateLimit>,
    pub seven_day_opus: Option<RateLimit>,
    pub seven_day_oauth_apps: Option<RateLimit>,
    pub extra_usage: Option<ExtraUsage>,
}

/// Known model short names → full API IDs.
pub const MODELS: &[(&str, &str)] = &[
    ("opus",   "claude-opus-4-6"),
    ("sonnet", "claude-sonnet-4-6"),
    ("haiku",  "claude-haiku-4-5-20251001"),
];

// ─── CliClient ──────────────────────────────────────────

#[derive(Clone)]
pub struct CliClient {
    /// BoringSSL HTTP client — exposed for crate-internal callers without a getter.
    pub client: Arc<BoringClient>,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub proxy_url: Option<String>,
    pub account_id: String,
    /// Per-account device_id (64 char hex SHA256)
    pub device_id: String,
    /// Per-account session UUID (generated once per process)
    pub session_uuid: String,
    /// Account UUID from profile
    pub account_uuid: String,
    /// Client version detected at startup (e.g. "2.1.111")
    pub cc_version: String,
}

impl CliClient {
    /// Create from account credentials
    pub fn new(
        client: Arc<BoringClient>,
        account_id: String,
        access_token: String,
        refresh_token: String,
        expires_at: i64,
        proxy_url: Option<String>,
    ) -> Self {
        Self {
            client,
            access_token,
            refresh_token,
            expires_at,
            proxy_url,
            account_id,
            device_id: String::new(),
            session_uuid: String::new(),
            account_uuid: String::new(),
            cc_version: get_cc_version().to_string(),
        }
    }

    /// Set identity fields (device_id, session_uuid, account_uuid)
    pub fn with_fingerprint(
        mut self,
        device_id: String,
        session_uuid: String,
        account_uuid: String,
    ) -> Self {
        self.device_id = device_id;
        self.session_uuid = session_uuid;
        self.account_uuid = account_uuid;
        self
    }

    // ─── Token state ────────────────────────────────────

    pub fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp_millis();
        now >= self.expires_at
    }

    pub fn needs_refresh(&self) -> bool {
        let now = chrono::Utc::now().timestamp_millis();
        now + REFRESH_BUFFER_MS >= self.expires_at
    }

    // ─── Token Refresh ──────────────────────────────────

    /// Exchange refresh_token for a new access_token
    pub async fn refresh(&mut self) -> Result<TokenResult, CliClientError> {
        let body = serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": self.refresh_token,
            "client_id": CLIENT_ID,
            "scope": DEFAULT_SCOPES,
        });

        let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
        let headers = self.build_axios_headers(body_bytes.len());

        let mut req = self
            .client
            .request(Method::POST, TOKEN_URL)
            .headers(headers)
            .body(Bytes::from(body_bytes));
        if let Some(ref proxy) = self.proxy_url {
            req = req.proxy(proxy);
        }

        let resp = req.send().await?;
        let (status, resp_body) = self.read_response_body(resp).await?;

        if status != 200 {
            let msg = String::from_utf8_lossy(&resp_body).to_string();
            return Err(CliClientError::HttpStatus(status, msg));
        }

        let result = self.parse_token_response(&resp_body)?;
        // Update internal state (refresh_token may rotate)
        self.access_token = result.access_token.clone();
        self.refresh_token = result.refresh_token.clone();
        self.expires_at = result.expires_at;
        Ok(result)
    }

    // ─── Token Exchange ─────────────────────────────────

    /// OAuth PKCE step 2: exchange authorization code for token
    pub async fn exchange_code(
        &mut self,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
        state: &str,
    ) -> Result<TokenResult, CliClientError> {
        let body = serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": redirect_uri,
            "client_id": CLIENT_ID,
            "code_verifier": code_verifier,
            "state": state,
        });

        let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
        let headers = self.build_axios_headers(body_bytes.len());

        let mut req = self
            .client
            .request(Method::POST, TOKEN_URL)
            .headers(headers)
            .body(Bytes::from(body_bytes));
        if let Some(ref proxy) = self.proxy_url {
            req = req.proxy(proxy);
        }

        let resp = req.send().await?;
        let (status, resp_body) = self.read_response_body(resp).await?;

        if status != 200 {
            let msg = String::from_utf8_lossy(&resp_body).to_string();
            return Err(CliClientError::HttpStatus(status, msg));
        }

        let result = self.parse_token_response(&resp_body)?;
        self.access_token = result.access_token.clone();
        self.refresh_token = result.refresh_token.clone();
        self.expires_at = result.expires_at;
        Ok(result)
    }

    // ─── Profile ────────────────────────────────────────

    /// Fetch account profile (subscriptionType / rateLimitTier / billingType etc.)
    pub async fn get_profile(&self) -> Result<ProfileInfo, CliClientError> {
        let headers = self.build_axios_headers_get();

        let mut req = self
            .client
            .request(Method::GET, PROFILE_URL)
            .headers(headers);
        if let Some(ref proxy) = self.proxy_url {
            req = req.proxy(proxy);
        }

        let resp = req.send().await?;
        let (status, resp_body) = self.read_response_body(resp).await?;

        if status != 200 {
            let msg = String::from_utf8_lossy(&resp_body).to_string();
            return Err(CliClientError::HttpStatus(status, msg));
        }

        self.parse_profile_response(&resp_body)
    }

    // ─── Auto-refresh ───────────────────────────────────

    /// Auto-refresh token if needed.
    /// Returns Some(result) if refreshed, None if still valid.
    pub async fn ensure_valid_token(&mut self) -> Result<Option<TokenResult>, CliClientError> {
        if self.needs_refresh() {
            let result = self.refresh().await?;
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }














    /// Fetch usage from /api/oauth/usage (no quota consumed).
    pub async fn get_usage(&self) -> Result<Utilization, CliClientError> {
        let url = "https://api.anthropic.com/api/oauth/usage";
        let mut headers = http::HeaderMap::new();
        // Match upstream client headers
        if let Ok(v) = http::HeaderValue::from_str("application/json, text/plain, */*") {
            headers.insert("accept", v);
        }
        if let Ok(v) = http::HeaderValue::from_str("gzip, compress, deflate, br") {
            headers.insert("accept-encoding", v);
        }
        if let Ok(v) = http::HeaderValue::from_str("oauth-2025-04-20") {
            headers.insert("anthropic-beta", v);
        }
        if let Ok(v) = http::HeaderValue::from_str(&format!("Bearer {}", self.access_token)) {
            headers.insert("authorization", v);
        }
        if let Ok(v) = http::HeaderValue::from_str("application/json") {
            headers.insert("content-type", v);
        }
        if let Ok(v) = http::HeaderValue::from_str(&format!("claude-code/{}", self.cc_version)) {
            headers.insert("user-agent", v);
        }

        let mut req = self.client.request(http::Method::GET, url).headers(headers);
        if let Some(ref proxy) = self.proxy_url {
            req = req.proxy(proxy);
        }

        let resp = req.send().await?;
        let (status, body) = self.read_response_body(resp).await?;

        if status != 200 {
            let msg = String::from_utf8_lossy(&body).to_string();
            return Err(CliClientError::HttpStatus(status, msg));
        }

        serde_json::from_slice(&body).map_err(|e| {
            CliClientError::ParseError(format!("Invalid usage JSON: {}", e))
        })
    }


    /// Parse quota/rate-limit info from response headers.
    pub fn parse_quota_from_headers(headers: &HeaderMap) -> Option<QuotaHeaders> {
        let utilization_5h = headers
            .get("anthropic-ratelimit-unified-5h-utilization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok());

        // Only return if we got at least the 5h utilization
        utilization_5h.map(|u5h| {
            let utilization_7d = headers
                .get("anthropic-ratelimit-unified-7d-utilization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<f64>().ok());
            let reset_5h = headers
                .get("anthropic-ratelimit-unified-5h-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok());
            QuotaHeaders {
                utilization_5h: u5h,
                utilization_7d,
                reset_5h,
            }
        })
    }

    // ─── Internal: Response helpers ─────────────────────

    /// Read response body (auto-decompressed by http layer)
    async fn read_response_body(
        &self,
        resp: http::Response<DecodedBody>,
    ) -> Result<(u16, Bytes), CliClientError> {
        let status = resp.status().as_u16();
        let body = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| CliClientError::ParseError(format!("Failed to read body: {}", e)))?
            .to_bytes();

        Ok((status, body))
    }

    // ─── Internal: Header builders ──────────────────────

    /// OAuth POST headers for token exchange / refresh
    fn build_axios_headers(&self, content_length: usize) -> HeaderMap {
        let mut headers = HeaderMap::new();
        // Alphabetical order
        headers.insert("accept", HeaderValue::from_static("application/json, text/plain, */*"));
        headers.insert(
            "accept-encoding",
            HeaderValue::from_static("gzip, compress, deflate, br"),
        );
        headers.insert(
            "content-length",
            HeaderValue::from_str(&content_length.to_string()).unwrap(),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        // Host is auto-derived from URL by the HTTP client
        headers.insert("user-agent", HeaderValue::from_static("axios/1.13.6"));
        headers
    }

    /// OAuth GET headers for profile fetch
    fn build_axios_headers_get(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("accept", HeaderValue::from_static("application/json, text/plain, */*"));
        headers.insert(
            "accept-encoding",
            HeaderValue::from_static("gzip, compress, deflate, br"),
        );
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", self.access_token)) {
            headers.insert("authorization", v);
        }
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("user-agent", HeaderValue::from_static("axios/1.13.6"));
        headers
    }

    // ─── Internal: Response parsers ─────────────────────

    fn parse_token_response(&self, body: &[u8]) -> Result<TokenResult, CliClientError> {
        let data: serde_json::Value = serde_json::from_slice(body).map_err(|e| {
            CliClientError::ParseError(format!("Invalid JSON in token response: {}", e))
        })?;

        let access_token = data["access_token"]
            .as_str()
            .ok_or_else(|| CliClientError::ParseError("Missing access_token".to_string()))?
            .to_string();
        let refresh_token = data["refresh_token"]
            .as_str()
            .unwrap_or(&self.refresh_token)
            .to_string();
        let expires_in = data["expires_in"].as_i64().unwrap_or(28800);
        let now = chrono::Utc::now().timestamp_millis();

        Ok(TokenResult {
            access_token,
            refresh_token,
            expires_at: now + (expires_in * 1000),
            expires_in,
            token_type: data["token_type"]
                .as_str()
                .unwrap_or("Bearer")
                .to_string(),
            subscription_type: data["subscription_type"].as_str().map(|s| s.to_string()),
            rate_limit_tier: data["rate_limit_tier"].as_str().map(|s| s.to_string()),
            scopes: data["scope"].as_str().map(|s| s.to_string()),
        })
    }

    fn parse_profile_response(&self, body: &[u8]) -> Result<ProfileInfo, CliClientError> {
        let data: serde_json::Value = serde_json::from_slice(body).map_err(|e| {
            CliClientError::ParseError(format!("Invalid JSON in profile response: {}", e))
        })?;

        let account = &data["account"];
        let org = &data["organization"];

        Ok(ProfileInfo {
            account_uuid: account["uuid"].as_str().unwrap_or("").to_string(),
            email: account["email"].as_str().unwrap_or("").to_string(),
            display_name: account["display_name"].as_str().map(|s| s.to_string()),
            organization_uuid: org["uuid"].as_str().unwrap_or("").to_string(),
            subscription_type: org["organization_type"].as_str().map(|s| {
                match s {
                    "claude_max" => "max",
                    "claude_pro" => "pro",
                    "claude_enterprise" => "enterprise",
                    "claude_team" => "team",
                    _ => s,
                }
                .to_string()
            }),
            rate_limit_tier: org["rate_limit_tier"].as_str().map(|s| s.to_string()),
            billing_type: org["billing_type"].as_str().map(|s| s.to_string()),
            has_extra_usage_enabled: org["has_extra_usage_enabled"].as_bool().unwrap_or(false),
            account_created_at: account["created_at"].as_str().map(|s| s.to_string()),
            subscription_created_at: org["subscription_created_at"]
                .as_str()
                .map(|s| s.to_string()),
        })
    }
}

// ─── Tests ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_token_response() {
        let client = CliClient {
            client: Arc::new(BoringClient::builder().build().unwrap()),
            access_token: String::new(),
            refresh_token: "old-refresh".to_string(),
            expires_at: 0,
            proxy_url: None,
            account_id: "test".to_string(),
            device_id: String::new(),
            session_uuid: String::new(),
            account_uuid: String::new(),
            cc_version: MAX_SUPPORTED_VERSION.to_string(),
        };

        let body = serde_json::json!({
            "access_token": "sk-ant-oat01-new",
            "refresh_token": "sk-ant-ort01-new",
            "expires_in": 28800,
            "token_type": "Bearer",
            "subscription_type": "max",
            "rate_limit_tier": "default_claude_max_20x",
            "scope": "user:inference user:profile"
        });

        let result = client
            .parse_token_response(serde_json::to_vec(&body).unwrap().as_slice())
            .unwrap();
        assert_eq!(result.access_token, "sk-ant-oat01-new");
        assert_eq!(result.refresh_token, "sk-ant-ort01-new");
        assert_eq!(result.expires_in, 28800);
        assert_eq!(result.subscription_type.as_deref(), Some("max"));
        assert_eq!(
            result.rate_limit_tier.as_deref(),
            Some("default_claude_max_20x")
        );
        assert!(result.expires_at > 0);
    }

    #[test]
    fn test_parse_token_response_refresh_token_rotation() {
        let client = CliClient {
            client: Arc::new(BoringClient::builder().build().unwrap()),
            access_token: String::new(),
            refresh_token: "old-refresh".to_string(),
            expires_at: 0,
            proxy_url: None,
            account_id: "test".to_string(),
            device_id: String::new(),
            session_uuid: String::new(),
            account_uuid: String::new(),
            cc_version: MAX_SUPPORTED_VERSION.to_string(),
        };

        // refresh_token absent in response → keep existing
        let body = serde_json::json!({
            "access_token": "sk-ant-oat01-new",
            "expires_in": 3600,
        });

        let result = client
            .parse_token_response(serde_json::to_vec(&body).unwrap().as_slice())
            .unwrap();
        assert_eq!(result.refresh_token, "old-refresh");
    }

    #[test]
    fn test_parse_profile_response() {
        let client = CliClient {
            client: Arc::new(BoringClient::builder().build().unwrap()),
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: 0,
            proxy_url: None,
            account_id: "test".to_string(),
            device_id: String::new(),
            session_uuid: String::new(),
            account_uuid: String::new(),
            cc_version: MAX_SUPPORTED_VERSION.to_string(),
        };

        let body = serde_json::json!({
            "account": {
                "uuid": "c5c83ed1-adb7-4ef1-b9e8-dc676bf28be7",
                "email": "test@example.com",
                "display_name": "Test User",
                "created_at": "2023-07-12T16:24:25.133970Z"
            },
            "organization": {
                "uuid": "ff91a96c-7e7b-445b-8322-8125c119ee85",
                "organization_type": "claude_max",
                "subscription_created_at": "2024-12-18T18:36:33.326560Z",
                "billing_type": "stripe_subscription",
                "has_extra_usage_enabled": false,
                "rate_limit_tier": "default_claude_max_20x"
            }
        });

        let result = client
            .parse_profile_response(serde_json::to_vec(&body).unwrap().as_slice())
            .unwrap();
        assert_eq!(result.account_uuid, "c5c83ed1-adb7-4ef1-b9e8-dc676bf28be7");
        assert_eq!(result.email, "test@example.com");
        assert_eq!(result.display_name.as_deref(), Some("Test User"));
        assert_eq!(result.subscription_type.as_deref(), Some("max"));
        assert_eq!(result.billing_type.as_deref(), Some("stripe_subscription"));
        assert!(!result.has_extra_usage_enabled);
    }

    #[test]
    fn test_is_expired() {
        let client = CliClient {
            client: Arc::new(BoringClient::builder().build().unwrap()),
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: chrono::Utc::now().timestamp_millis() - 1000, // expired 1s ago
            proxy_url: None,
            account_id: "test".to_string(),
            device_id: String::new(),
            session_uuid: String::new(),
            account_uuid: String::new(),
            cc_version: MAX_SUPPORTED_VERSION.to_string(),
        };
        assert!(client.is_expired());
        assert!(client.needs_refresh());
    }

    #[test]
    fn test_needs_refresh_within_buffer() {
        let client = CliClient {
            client: Arc::new(BoringClient::builder().build().unwrap()),
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: chrono::Utc::now().timestamp_millis() + 3 * 60 * 1000, // expires in 3 min
            proxy_url: None,
            account_id: "test".to_string(),
            device_id: String::new(),
            session_uuid: String::new(),
            account_uuid: String::new(),
            cc_version: MAX_SUPPORTED_VERSION.to_string(),
        };
        assert!(!client.is_expired());
        assert!(client.needs_refresh()); // 3 min < 5 min buffer
    }

    #[test]
    fn test_not_expired() {
        let client = CliClient {
            client: Arc::new(BoringClient::builder().build().unwrap()),
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: chrono::Utc::now().timestamp_millis() + 3600 * 1000, // 1 hour from now
            proxy_url: None,
            account_id: "test".to_string(),
            device_id: String::new(),
            session_uuid: String::new(),
            account_uuid: String::new(),
            cc_version: MAX_SUPPORTED_VERSION.to_string(),
        };
        assert!(!client.is_expired());
        assert!(!client.needs_refresh());
    }

    #[test]
    fn test_axios_headers_post() {
        let client = CliClient {
            client: Arc::new(BoringClient::builder().build().unwrap()),
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: 0,
            proxy_url: None,
            account_id: "test".to_string(),
            device_id: String::new(),
            session_uuid: String::new(),
            account_uuid: String::new(),
            cc_version: MAX_SUPPORTED_VERSION.to_string(),
        };

        let headers = client.build_axios_headers(42);
        assert_eq!(
            headers.get("accept").unwrap(),
            "application/json, text/plain, */*"
        );
        assert_eq!(
            headers.get("accept-encoding").unwrap(),
            "gzip, compress, deflate, br"
        );
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
        assert_eq!(headers.get("user-agent").unwrap(), "axios/1.13.6");
        assert_eq!(headers.get("content-length").unwrap(), "42");
    }

    #[test]
    fn test_axios_headers_get_has_authorization() {
        let client = CliClient {
            client: Arc::new(BoringClient::builder().build().unwrap()),
            access_token: "sk-ant-oat01-test".to_string(),
            refresh_token: String::new(),
            expires_at: 0,
            proxy_url: None,
            account_id: "test".to_string(),
            device_id: String::new(),
            session_uuid: String::new(),
            account_uuid: String::new(),
            cc_version: MAX_SUPPORTED_VERSION.to_string(),
        };

        let headers = client.build_axios_headers_get();
        assert_eq!(
            headers.get("authorization").unwrap(),
            "Bearer sk-ant-oat01-test"
        );
    }

}
