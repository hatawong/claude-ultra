//! Outbound request builder — headers and metadata.

use http::{HeaderMap, HeaderName, HeaderValue};

/// Per-request context derived from the selected account.
pub struct RequestContext {
    pub device_id: String,
    pub account_uuid: String,
    pub access_token: String,
    pub mapped_session_uuid: String,
}

/// Stable per-account identifier.
pub fn compute_mapped_session_uuid(orig_session_id: &str, account_uuid: &str) -> String {
    let input = format!("{}@{}", orig_session_id, account_uuid);
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, input.as_bytes()).to_string()
}

/// Headers stripped from the client request before forwarding.
///
/// Deliberately **does not** include the full RFC 7230 hop-by-hop set
/// (`connection`, `keep-alive`, `te`, `trailer`, `upgrade`, `proxy-*`).
/// CC CLI sends `Connection: keep-alive` natively, and preserving the
/// exact client header list on the wire keeps the upstream fingerprint
/// aligned with a native CLI request. Only protocol-critical entries
/// (`host`, `content-length`, `transfer-encoding`) and the identity
/// entries gateway rewrites (`authorization`, `x-api-key`,
/// `x-ai-gateway-api-key`, `x-claude-code-session-id`) are removed.
const HEADERS_TO_REMOVE: &[&str] = &[
    "x-api-key",
    "authorization",
    "x-ai-gateway-api-key",
    "x-claude-code-session-id",
    "host",
    "content-length",
    "transfer-encoding",
];

/// Build outbound headers for proxied request.
/// Headers are inserted in alphabetical order to match Bun's behavior.
/// `content_length` is the final body size (after metadata replacement).
pub fn build_outbound_headers(
    client_headers: &HeaderMap,
    request_context: &RequestContext,
    content_length: usize,
    upstream_host: &str,
    vercel_api_key: Option<&str>,
) -> HeaderMap {
    // Collect client headers that we want to pass through (excluding removed ones)
    let mut passthrough: Vec<(String, HeaderValue)> = Vec::new();
    for (name, value) in client_headers.iter() {
        let name_lower = name.as_str().to_lowercase();
        if HEADERS_TO_REMOVE.iter().any(|h| *h == name_lower) {
            continue;
        }
        // user-agent: pass through as-is
        passthrough.push((name_lower, value.clone()));
    }

    // Build the final set with replacements/additions
    let mut entries: Vec<(String, HeaderValue)> = Vec::new();

    // Add passthrough entries
    for (name, value) in &passthrough {
        entries.push((name.clone(), value.clone()));
    }

    // Replace/add required headers
    // Remove existing entries that we're going to replace
    entries.retain(|(n, _)| {
        n != "authorization"
            && n != "x-claude-code-session-id"
            && n != "host"
            && n != "x-client-request-id"
    });

    // Add authorization
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", request_context.access_token)) {
        entries.push(("authorization".to_string(), v));
    }

    // Add host (parameterized for Vercel/Anthropic)
    entries.push((
        "host".to_string(),
        HeaderValue::from_str(upstream_host).unwrap_or_else(|_| {
            HeaderValue::from_static("api.anthropic.com")
        }),
    ));

    // Add x-claude-code-session-id
    if let Ok(v) = HeaderValue::from_str(&request_context.mapped_session_uuid) {
        entries.push(("x-claude-code-session-id".to_string(), v));
    }

    // Add x-client-request-id (new UUID per request)
    let request_id = uuid::Uuid::new_v4().to_string();
    if let Ok(v) = HeaderValue::from_str(&request_id) {
        entries.push(("x-client-request-id".to_string(), v));
    }

    // Add content-length (must be in sorted position, not appended after)
    if let Ok(v) = HeaderValue::from_str(&content_length.to_string()) {
        entries.push(("content-length".to_string(), v));
    }

    // Vercel API key (Mode 2 subscription proxy)
    if let Some(vck) = vercel_api_key {
        let bearer = format!("Bearer {}", vck);
        if let Ok(v) = HeaderValue::from_str(&bearer) {
            entries.push(("x-ai-gateway-api-key".to_string(), v));
        }
    }

    // Ensure anthropic-beta contains oauth-2025-04-20
    ensure_oauth_beta(&mut entries);

    // Sort by header name (alphabetical) to match Bun's behavior
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    // Build HeaderMap in sorted order
    let mut result = HeaderMap::new();
    for (name, value) in entries {
        if let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) {
            result.insert(header_name, value);
        }
    }

    result
}

/// Merge `oauth-2025-04-20` into a client-supplied `anthropic-beta` header.
///
/// Insertion position matches CC CLI's push order: right after
/// `claude-code-20250219` when present, otherwise at the front. Already
/// present → no-op.
///
/// Missing header is a no-op by design: the only caller here is the main
/// pooled path handling CC CLI traffic, and CC CLI always sends
/// `anthropic-beta`. A fallback for truly missing headers will be added when
/// a concrete consumer needs it, not speculatively.
fn ensure_oauth_beta(entries: &mut Vec<(String, HeaderValue)>) {
    let beta_key = "anthropic-beta";
    let required = "oauth-2025-04-20";

    if let Some(entry) = entries.iter_mut().find(|(n, _)| n == beta_key) {
        let current = entry.1.to_str().unwrap_or("");
        if current.split(',').any(|b| b.trim() == required) {
            return;
        }
        let parts: Vec<String> = current
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let new_parts: Vec<String> = if let Some(first) = parts.first() {
            if first == "claude-code-20250219" {
                let mut v: Vec<String> = Vec::with_capacity(parts.len() + 1);
                v.push(first.clone());
                v.push(required.to_string());
                v.extend(parts.iter().skip(1).cloned());
                v
            } else {
                let mut v: Vec<String> = Vec::with_capacity(parts.len() + 1);
                v.push(required.to_string());
                v.extend(parts.iter().cloned());
                v
            }
        } else {
            vec![required.to_string()]
        };
        let new_value = new_parts.join(",");
        if let Ok(v) = HeaderValue::from_str(&new_value) {
            entry.1 = v;
        }
    }
}

/// Build the outbound URL with ?beta=true query param.
pub fn build_outbound_url(base_url: &str, path: &str) -> String {
    let separator = if path.contains('?') { "&" } else { "?" };
    format!("{}{}{}beta=true", base_url, path, separator)
}

/// Locate the `cch=00000` offset in the serialized body. None if absent.
///
/// Uses the JSON-encoded form of `system[0].text` as the anchor so escape
/// sequences in the text do not break byte-level alignment with `body_bytes`.
pub fn find_billing_cch_offset(body_bytes: &[u8], value: &serde_json::Value) -> Option<usize> {
    let text = value.get("system")?.get(0)?.get("text")?.as_str()?;
    let encoded = serde_json::to_string(text).ok()?;
    let inner = encoded.strip_prefix('"')?.strip_suffix('"')?;
    let rel = inner.find("cch=00000")?;
    let anchor_pos = body_bytes
        .windows(inner.len())
        .position(|w| w == inner.as_bytes())?;
    Some(anchor_pos + rel)
}

/// Replace metadata.user_id in the request body JSON with per-account values.
/// Replace metadata.user_id with per-account deterministic mapping.
pub fn replace_metadata_user_id(
    body: &[u8],
    request_context: &RequestContext,
) -> Result<Vec<u8>, claude_ultra_http::Error> {
    let mut value: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Ok(body.to_vec()),
    };
    apply_metadata_in_place(&mut value, request_context)?;
    Ok(serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec()))
}

/// Mutate an already-parsed body `Value` in place:
/// - License check (in distribution builds)
/// - Replace `metadata.user_id` with a deterministic mapping based on the account
///
/// This is the primary API used by `handler`, which holds the body Value across
/// the retry loop. `replace_metadata_user_id` above is a bytes wrapper that parses
/// + serializes, retained for callers that work at the bytes layer.
///
/// Invariant: handler derives `is_sse` from the body's `stream` field before calling
/// into this function; do NOT modify `stream` here — SSE vs JSON routing depends
/// on the client's original intent being preserved end-to-end.
pub fn apply_metadata_in_place(
    value: &mut serde_json::Value,
    request_context: &RequestContext,
) -> Result<(), claude_ultra_http::Error> {
    // License check (distribution mode only)
    #[cfg(not(feature = "internal"))]
    {
        claude_ultra_http::license::check_license(&request_context.account_uuid)
            .map_err(claude_ultra_http::Error::license)?;
    }

    if let Some(metadata) = value.get_mut("metadata") {
        let user_id_json = serde_json::json!({
            "device_id": request_context.device_id,
            "account_uuid": request_context.account_uuid,
            "session_id": request_context.mapped_session_uuid,
        });
        metadata["user_id"] = serde_json::Value::String(user_id_json.to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set up license bypass for tests — in default (non-internal) mode,
    /// tests need a valid license or the check will fail.
    /// We clear the license guard so check_license returns NoLicense,
    /// then we need to handle the Result differently.
    /// Simplest: just use unwrap() — tests run with --features internal where check is no-op.
    /// For default mode: these tests are skipped via #[cfg(feature = "internal")].

    fn sample_client_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("accept", "application/json".parse().unwrap());
        h.insert("accept-encoding", "gzip, deflate, br, zstd".parse().unwrap());
        h.insert("anthropic-beta", "claude-code-20250219,interleaved-thinking-2025-05-14".parse().unwrap());
        h.insert("anthropic-dangerous-direct-browser-access", "true".parse().unwrap());
        h.insert("anthropic-version", "2023-06-01".parse().unwrap());
        h.insert("connection", "keep-alive".parse().unwrap());
        h.insert("content-type", "application/json".parse().unwrap());
        h.insert("host", "localhost:9000".parse().unwrap());
        h.insert("user-agent", "claude-cli/2.1.92 (subscriber, cli)".parse().unwrap());
        h.insert("x-api-key", "claude-ultra-proxy-key".parse().unwrap());
        h.insert("x-app", "cli".parse().unwrap());
        h.insert("x-claude-code-session-id", "client-session-id".parse().unwrap());
        h.insert("x-stainless-arch", "arm64".parse().unwrap());
        h.insert("x-stainless-lang", "js".parse().unwrap());
        h.insert("x-stainless-os", "MacOS".parse().unwrap());
        h.insert("x-stainless-package-version", "0.80.0".parse().unwrap());
        h.insert("x-stainless-retry-count", "0".parse().unwrap());
        h.insert("x-stainless-runtime", "node".parse().unwrap());
        h.insert("x-stainless-runtime-version", "v24.3.0".parse().unwrap());
        h.insert("x-stainless-timeout", "600".parse().unwrap());
        h
    }

    fn test_account() -> RequestContext {
        RequestContext {
            device_id: "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(),
            account_uuid: "11111111-2222-3333-4444-555555555555".to_string(),
            access_token: "sk-ant-oat01-test-token".to_string(),
            mapped_session_uuid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
        }
    }

    // ── Header building tests (≥10) ────────────────────────────────────

    #[test]
    fn test_authorization_replaced() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer sk-ant-oat01-test-token"
        );
    }

    #[test]
    fn test_session_id_replaced() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        assert_eq!(
            headers.get("x-claude-code-session-id").unwrap().to_str().unwrap(),
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        );
    }

    #[test]
    fn test_x_api_key_removed() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        assert!(headers.get("x-api-key").is_none(), "x-api-key must be removed");
    }

    #[test]
    fn test_x_client_request_id_generated() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        let id = headers.get("x-client-request-id").expect("must have x-client-request-id");
        let id_str = id.to_str().unwrap();
        // UUID format: 8-4-4-4-12
        assert_eq!(id_str.len(), 36);
        assert_eq!(id_str.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn test_anthropic_beta_contains_oauth() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        assert!(
            beta.split(',').any(|b| b.trim() == "oauth-2025-04-20"),
            "anthropic-beta must contain oauth-2025-04-20, got: {}",
            beta
        );
    }

    #[test]
    fn test_anthropic_beta_preserves_existing() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        assert!(beta.contains("claude-code-20250219"), "original beta values must be preserved");
        assert!(beta.contains("interleaved-thinking-2025-05-14"));
    }

    #[test]
    fn test_anthropic_beta_no_duplicate_oauth() {
        let mut h = sample_client_headers();
        h.insert("anthropic-beta", "claude-code-20250219,oauth-2025-04-20".parse().unwrap());
        let headers = build_outbound_headers(&h, &test_account(), 1024, "api.anthropic.com", None);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        let oauth_count = beta.split(',').filter(|b| b.trim() == "oauth-2025-04-20").count();
        assert_eq!(oauth_count, 1, "oauth-2025-04-20 should not be duplicated");
    }

    #[test]
    fn test_anthropic_beta_empty_header_no_leading_comma() {
        // Regression for P3-B: empty anthropic-beta must not produce ",oauth-2025-04-20"
        let mut h = sample_client_headers();
        h.insert("anthropic-beta", "".parse().unwrap());
        let headers = build_outbound_headers(&h, &test_account(), 1024, "api.anthropic.com", None);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        assert_eq!(beta, "oauth-2025-04-20", "no leading comma when current is empty");
    }

    #[test]
    fn test_host_replaced() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        assert_eq!(
            headers.get("host").unwrap().to_str().unwrap(),
            "api.anthropic.com"
        );
    }

    #[test]
    fn test_stainless_headers_passthrough() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        assert_eq!(headers.get("x-stainless-arch").unwrap().to_str().unwrap(), "arm64");
        assert_eq!(headers.get("x-stainless-lang").unwrap().to_str().unwrap(), "js");
        assert_eq!(headers.get("x-stainless-os").unwrap().to_str().unwrap(), "MacOS");
        assert_eq!(headers.get("x-stainless-package-version").unwrap().to_str().unwrap(), "0.80.0");
        assert_eq!(headers.get("x-stainless-runtime").unwrap().to_str().unwrap(), "node");
        assert_eq!(headers.get("x-stainless-runtime-version").unwrap().to_str().unwrap(), "v24.3.0");
        assert_eq!(headers.get("x-stainless-timeout").unwrap().to_str().unwrap(), "600");
    }

    #[test]
    fn test_user_agent_passthrough() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        assert_eq!(
            headers.get("user-agent").unwrap().to_str().unwrap(),
            "claude-cli/2.1.92 (subscriber, cli)"
        );
    }

    #[test]
    fn test_anthropic_version_passthrough() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        assert_eq!(
            headers.get("anthropic-version").unwrap().to_str().unwrap(),
            "2023-06-01"
        );
    }

    #[test]
    fn test_content_length_from_body_size() {
        // Client's content-length is removed and replaced with actual body size
        let mut h = sample_client_headers();
        h.insert("content-length", "12345".parse().unwrap());
        let headers = build_outbound_headers(&h, &test_account(), 9999, "api.anthropic.com", None);
        assert_eq!(
            headers.get("content-length").unwrap().to_str().unwrap(),
            "9999",
            "content-length must reflect actual body size, not client's value"
        );
    }

    #[test]
    fn test_headers_sorted_alphabetically() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024, "api.anthropic.com", None);
        let names: Vec<String> = headers.keys().map(|k| k.as_str().to_string()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "headers must be in alphabetical order");
    }

    // ── URL building ─────────────────────────────────────────────────────

    #[test]
    fn test_url_has_beta_true() {
        let url = build_outbound_url("https://api.anthropic.com", "/v1/messages");
        assert_eq!(url, "https://api.anthropic.com/v1/messages?beta=true");
    }

    #[test]
    fn test_url_count_tokens() {
        let url = build_outbound_url("https://api.anthropic.com", "/v1/messages/count_tokens");
        assert_eq!(url, "https://api.anthropic.com/v1/messages/count_tokens?beta=true");
    }

    #[test]
    fn test_url_with_existing_query_params() {
        let url = build_outbound_url("https://api.anthropic.com", "/v1/messages?foo=bar");
        assert_eq!(url, "https://api.anthropic.com/v1/messages?foo=bar&beta=true");
    }

    // ── Metadata replacement tests (≥3) ──────────────────────────────────

    #[test]
    #[cfg(feature = "internal")]
    fn test_metadata_user_id_replaced() {
        let body = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "max_tokens": 1024,
            "metadata": {
                "user_id": "{\"device_id\":\"old_device\",\"account_uuid\":\"old_uuid\",\"session_id\":\"old_session\"}"
            },
            "messages": [{"role": "user", "content": "hi"}]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let result = replace_metadata_user_id(&body_bytes, &test_account()).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();

        let user_id_str = parsed["metadata"]["user_id"].as_str().unwrap();
        let user_id: serde_json::Value = serde_json::from_str(user_id_str).unwrap();

        assert_eq!(user_id["device_id"].as_str().unwrap(), test_account().device_id);
        assert_eq!(user_id["account_uuid"].as_str().unwrap(), test_account().account_uuid);
        assert_eq!(user_id["session_id"].as_str().unwrap(), test_account().mapped_session_uuid);
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_metadata_no_metadata_field_unchanged() {
        let body = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "hi"}]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let result = replace_metadata_user_id(&body_bytes, &test_account()).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert!(parsed.get("metadata").is_none(), "no metadata should be added if not present");
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_metadata_large_body_preserved() {
        // Build a body > 100KB
        let large_content = "x".repeat(120_000);
        let body = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "max_tokens": 1024,
            "metadata": {
                "user_id": "{\"device_id\":\"old\",\"account_uuid\":\"old\",\"session_id\":\"old\"}"
            },
            "messages": [{"role": "user", "content": large_content}]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        assert!(body_bytes.len() > 100_000, "body must be > 100KB");

        let result = replace_metadata_user_id(&body_bytes, &test_account()).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();

        // Verify metadata replaced
        let user_id_str = parsed["metadata"]["user_id"].as_str().unwrap();
        let user_id: serde_json::Value = serde_json::from_str(user_id_str).unwrap();
        assert_eq!(user_id["device_id"].as_str().unwrap(), test_account().device_id);

        // Verify large content preserved (gateway may wrap string into array with a prepended hook block)
        let content = &parsed["messages"][0]["content"];
        let original_len = if let Some(arr) = content.as_array() {
            arr.iter()
                .find_map(|b| b.get("text").and_then(|t| t.as_str()).filter(|s| s.len() >= 100_000))
                .map(|s| s.len())
                .expect("original large text must be preserved in content array")
        } else {
            content.as_str().unwrap().len()
        };
        assert_eq!(original_len, 120_000);
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_metadata_content_length_changes() {
        let body = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "metadata": {
                "user_id": "{\"device_id\":\"a\",\"account_uuid\":\"b\",\"session_id\":\"c\"}"
            },
            "messages": [{"role": "user", "content": "hi"}]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let result = replace_metadata_user_id(&body_bytes, &test_account()).unwrap();
        // After replacement, the body should be longer
        assert_ne!(body_bytes.len(), result.len(), "body length should change after metadata replacement");
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_metadata_invalid_json_returns_original() {
        let body = b"not json at all";
        let result = replace_metadata_user_id(body, &test_account()).unwrap();
        assert_eq!(result, body.to_vec());
    }

    // ── Field order + stream injection tests ────────────────────

    #[test]
    fn test_roundtrip_preserves_field_order() {
        let raw = r#"{"model":"claude-opus-4-6","max_tokens":64000,"messages":[],"system":[],"tools":[],"metadata":{"user_id":"{}"},"thinking":{"type":"adaptive"},"stream":true,"context_management":{},"output_config":{}}"#;
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let roundtripped = serde_json::to_string(&value).unwrap();
        let orig_map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
        let rt_map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&roundtripped).unwrap();
        let original_keys: Vec<&str> = orig_map.keys().map(|s| s.as_str()).collect();
        let roundtrip_keys: Vec<&str> = rt_map.keys().map(|s| s.as_str()).collect();
        assert_eq!(original_keys, roundtrip_keys, "field order must be preserved after roundtrip");
    }

    /// Stream flag must be preserved as-is (not overridden) so the Gateway respects
    /// the client's stream intent. /v1/messages with stream:true → SSE path;
    /// with stream:false or count_tokens → JSON path.
    #[test]
    #[cfg(feature = "internal")]
    fn test_stream_flag_preserved_when_true() {
        let raw = r#"{"model":"claude-opus-4-6","stream":true,"max_tokens":64000,"metadata":{"user_id":"{}"},"messages":[]}"#;
        let body = raw.as_bytes();
        let result = replace_metadata_user_id(body, &test_account()).unwrap();
        let parsed: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["stream"], serde_json::Value::Bool(true), "stream=true must be preserved");
        let keys: Vec<&str> = parsed.keys().map(|s| s.as_str()).collect();
        assert_eq!(keys.iter().position(|&k| k == "stream").unwrap(), 1, "stream position preserved");
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_stream_flag_absent_when_not_in_body() {
        // When client omits stream, Gateway must NOT inject it — preserves non-stream semantics.
        let raw = r#"{"model":"claude-opus-4-6","max_tokens":64000,"metadata":{"user_id":"{}"},"messages":[]}"#;
        let body = raw.as_bytes();
        let result = replace_metadata_user_id(body, &test_account()).unwrap();
        let parsed: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&result).unwrap();
        assert!(parsed.get("stream").is_none(), "stream must not be injected when client omitted it");
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_stream_false_preserved() {
        // When client explicitly sends stream:false, Gateway must NOT override to true.
        let raw = r#"{"model":"claude-opus-4-6","stream":false,"metadata":{"user_id":"{}"},"messages":[]}"#;
        let body = raw.as_bytes();
        let result = replace_metadata_user_id(body, &test_account()).unwrap();
        let parsed: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["stream"], serde_json::Value::Bool(false), "stream=false must be preserved");
        let keys: Vec<&str> = parsed.keys().map(|s| s.as_str()).collect();
        let stream_pos = keys.iter().position(|&k| k == "stream").unwrap();
        assert_eq!(stream_pos, 1, "stream position must not change");
    }

    // ── compute_mapped_session_uuid pure fn ────────────────────────

    #[test]
    fn test_compute_mapped_session_uuid_stable() {
        let s1 = compute_mapped_session_uuid("orig1", "acc1");
        let s2 = compute_mapped_session_uuid("orig1", "acc1");
        assert_eq!(s1, s2, "same inputs must produce same output");
    }

    #[test]
    fn test_compute_mapped_session_uuid_account_isolation() {
        let s1 = compute_mapped_session_uuid("orig1", "acc1");
        let s2 = compute_mapped_session_uuid("orig1", "acc2");
        assert_ne!(s1, s2, "different accounts must produce different mapped sid");
    }

    #[test]
    fn test_compute_mapped_session_uuid_orig_isolation() {
        let s1 = compute_mapped_session_uuid("orig1", "acc1");
        let s2 = compute_mapped_session_uuid("orig2", "acc1");
        assert_ne!(s1, s2, "different orig sids must produce different mapped sid");
    }

    #[test]
    fn test_compute_mapped_session_uuid_empty_orig_stable() {
        let s1 = compute_mapped_session_uuid("", "acc1");
        let s2 = compute_mapped_session_uuid("", "acc1");
        assert_eq!(s1, s2, "empty orig must be handled deterministically");
        assert!(!s1.is_empty(), "empty orig must still yield a valid uuid");
    }

    // ── Invariant: header sid == body sid ─────────────────────────

    #[test]
    #[cfg(feature = "internal")]
    fn test_header_and_body_session_id_match_with_orig() {
        let ctx = test_account();
        let headers = build_outbound_headers(&sample_client_headers(), &ctx, 1024, "api.anthropic.com", None);
        let h_sid = headers.get("x-claude-code-session-id").unwrap().to_str().unwrap().to_string();

        let raw = r#"{"metadata":{"user_id":"{\"device_id\":\"x\",\"account_uuid\":\"y\",\"session_id\":\"orig123\"}"},"messages":[]}"#;
        let result = replace_metadata_user_id(raw.as_bytes(), &ctx).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        let uid_str = parsed["metadata"]["user_id"].as_str().unwrap();
        let uid: serde_json::Value = serde_json::from_str(uid_str).unwrap();
        let b_sid = uid["session_id"].as_str().unwrap().to_string();

        assert_eq!(h_sid, b_sid, "header and body session_id must be identical");
        assert_eq!(h_sid, ctx.mapped_session_uuid, "both must equal ctx.mapped_session_uuid");
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_header_and_body_session_id_match_empty_orig() {
        let ctx = test_account();
        let headers = build_outbound_headers(&sample_client_headers(), &ctx, 1024, "api.anthropic.com", None);
        let h_sid = headers.get("x-claude-code-session-id").unwrap().to_str().unwrap().to_string();

        let raw = r#"{"metadata":{"user_id":"{}"},"messages":[]}"#;
        let result = replace_metadata_user_id(raw.as_bytes(), &ctx).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        let uid_str = parsed["metadata"]["user_id"].as_str().unwrap();
        let uid: serde_json::Value = serde_json::from_str(uid_str).unwrap();
        let b_sid = uid["session_id"].as_str().unwrap().to_string();

        assert_eq!(h_sid, b_sid, "empty-orig path must still keep header == body");
        assert_eq!(h_sid, ctx.mapped_session_uuid);
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_header_and_body_session_id_match_across_accounts() {
        // Simulate failover: same orig_sid, two different accounts, each ctx independently aligned
        let mk = |aid: &str| {
            let device_id = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string();
            let mapped = compute_mapped_session_uuid("orig-shared", aid);
            RequestContext {
                device_id,
                account_uuid: aid.to_string(),
                access_token: "tok".to_string(),
                mapped_session_uuid: mapped,
            }
        };
        let ctx_a = mk("11111111-2222-3333-4444-555555555555");
        let ctx_b = mk("99999999-8888-7777-6666-555555555555");

        for ctx in [&ctx_a, &ctx_b] {
            let headers = build_outbound_headers(&sample_client_headers(), ctx, 1024, "api.anthropic.com", None);
            let h_sid = headers.get("x-claude-code-session-id").unwrap().to_str().unwrap().to_string();

            let raw = r#"{"metadata":{"user_id":"{\"device_id\":\"x\",\"account_uuid\":\"y\",\"session_id\":\"orig-shared\"}"},"messages":[]}"#;
            let result = replace_metadata_user_id(raw.as_bytes(), ctx).unwrap();
            let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
            let uid_str = parsed["metadata"]["user_id"].as_str().unwrap();
            let uid: serde_json::Value = serde_json::from_str(uid_str).unwrap();
            let b_sid = uid["session_id"].as_str().unwrap().to_string();

            assert_eq!(h_sid, b_sid, "per-account ctx must keep header == body");
            assert_eq!(h_sid, ctx.mapped_session_uuid);
        }
        assert_ne!(ctx_a.mapped_session_uuid, ctx_b.mapped_session_uuid, "different accounts must produce different mapped sid");
    }

    // ── anthropic-beta oauth insertion order ────────────

    #[test]
    fn test_anthropic_beta_oauth_inserted_after_claude_code() {
        // Mirrors CC CLI non-haiku order: [claude-code-20250219, oauth-2025-04-20, ...]
        let mut h = sample_client_headers();
        h.insert(
            "anthropic-beta",
            "claude-code-20250219,context-1m-2025-08-07,effort-2025-11-24".parse().unwrap(),
        );
        let headers = build_outbound_headers(&h, &test_account(), 1024, "api.anthropic.com", None);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        let flags: Vec<&str> = beta.split(',').map(|s| s.trim()).collect();
        assert_eq!(flags[0], "claude-code-20250219");
        assert_eq!(flags[1], "oauth-2025-04-20");
        assert_eq!(flags[2], "context-1m-2025-08-07");
        assert_eq!(flags[3], "effort-2025-11-24");
    }

    #[test]
    fn test_anthropic_beta_oauth_inserted_at_front_when_no_claude_code() {
        // Haiku / non-claude-code scenario: oauth lands at position 0
        let mut h = sample_client_headers();
        h.insert(
            "anthropic-beta",
            "context-1m-2025-08-07,effort-2025-11-24".parse().unwrap(),
        );
        let headers = build_outbound_headers(&h, &test_account(), 1024, "api.anthropic.com", None);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        let flags: Vec<&str> = beta.split(',').map(|s| s.trim()).collect();
        assert_eq!(flags[0], "oauth-2025-04-20");
        assert_eq!(flags[1], "context-1m-2025-08-07");
        assert_eq!(flags[2], "effort-2025-11-24");
    }

    // ── find_billing_cch_offset ──────────────────────────────────────

    #[test]
    #[cfg(feature = "internal")]
    fn test_find_billing_cch_offset_basic() {
        let v = serde_json::json!({
            "system": [{"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.114.e1c; cc_entrypoint=claude-vscode; cch=00000;"}],
            "messages": []
        });
        let body = serde_json::to_vec(&v).unwrap();
        let off = find_billing_cch_offset(&body, &v).unwrap();
        assert_eq!(&body[off..off + 9], b"cch=00000");
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_find_billing_cch_offset_ignores_user_content_literal() {
        // User content includes the placeholder literal.
        let v = serde_json::json!({
            "messages": [{"role":"user","content":"mention cch=00000 here"}],
            "system": [{"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.114.e1c; cc_entrypoint=claude-vscode; cch=00000;"}],
        });
        let body = serde_json::to_vec(&v).unwrap();
        let off = find_billing_cch_offset(&body, &v).unwrap();
        assert_eq!(&body[off..off + 9], b"cch=00000");
        // Must NOT be the earlier user-content occurrence.
        let user_occurrence = body.windows(9).position(|w| w == b"cch=00000").unwrap();
        assert_ne!(off, user_occurrence, "must not return user-content occurrence");
        assert!(off > user_occurrence);
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_find_billing_cch_offset_none_without_system() {
        let v = serde_json::json!({"messages": []});
        let body = serde_json::to_vec(&v).unwrap();
        assert_eq!(find_billing_cch_offset(&body, &v), None);
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_find_billing_cch_offset_none_without_cch_in_system_text() {
        let v = serde_json::json!({
            "system": [{"type":"text","text":"plain system prompt no placeholder"}],
        });
        let body = serde_json::to_vec(&v).unwrap();
        assert_eq!(find_billing_cch_offset(&body, &v), None);
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_find_billing_cch_offset_with_escaped_newline() {
        let v = serde_json::json!({
            "system": [{"type":"text","text":"line-a\nline-b\ncch=00000;"}],
        });
        let body = serde_json::to_vec(&v).unwrap();
        let off = find_billing_cch_offset(&body, &v).unwrap();
        assert_eq!(&body[off..off + 9], b"cch=00000");
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_find_billing_cch_offset_with_escaped_quote() {
        let v = serde_json::json!({
            "system": [{"type":"text","text":"say \"hi\" before cch=00000;"}],
        });
        let body = serde_json::to_vec(&v).unwrap();
        let off = find_billing_cch_offset(&body, &v).unwrap();
        assert_eq!(&body[off..off + 9], b"cch=00000");
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_find_billing_cch_offset_with_escaped_backslash() {
        let v = serde_json::json!({
            "system": [{"type":"text","text":"path C:\\tmp before cch=00000;"}],
        });
        let body = serde_json::to_vec(&v).unwrap();
        let off = find_billing_cch_offset(&body, &v).unwrap();
        assert_eq!(&body[off..off + 9], b"cch=00000");
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_find_billing_cch_offset_with_non_ascii() {
        // Non-ASCII chars round-trip through JSON without escape by default,
        // but the encoded byte sequence differs from the decoded one only in
        // multi-byte UTF-8, which this anchor strategy still tolerates.
        let v = serde_json::json!({
            "system": [{"type":"text","text":"序言 before cch=00000;"}],
        });
        let body = serde_json::to_vec(&v).unwrap();
        let off = find_billing_cch_offset(&body, &v).unwrap();
        assert_eq!(&body[off..off + 9], b"cch=00000");
    }

    #[test]
    fn test_anthropic_beta_oauth_preserves_existing_position() {
        // If oauth already present, leave it (don't move/re-insert)
        let mut h = sample_client_headers();
        h.insert(
            "anthropic-beta",
            "claude-code-20250219,oauth-2025-04-20,context-1m-2025-08-07".parse().unwrap(),
        );
        let headers = build_outbound_headers(&h, &test_account(), 1024, "api.anthropic.com", None);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        assert_eq!(beta, "claude-code-20250219,oauth-2025-04-20,context-1m-2025-08-07");
    }
}
