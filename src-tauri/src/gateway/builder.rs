//! Outbound request builder — headers and metadata.

use http::{HeaderMap, HeaderName, HeaderValue};

/// Info needed to build outbound headers for a specific account.
pub struct AccountIdentity {
    pub access_token: String,
    pub session_uuid: String, // per-account X-Claude-Code-Session-Id
    pub device_id: String,    // per-account device_id (64 char hex SHA256)
    pub account_uuid: String, // from CliClient or AccountInfo
}

/// Headers that must be removed from client request before forwarding.
const HEADERS_TO_REMOVE: &[&str] = &[
    "x-api-key",
    "authorization",
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
    account: &AccountIdentity,
    content_length: usize,
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
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", account.access_token)) {
        entries.push(("authorization".to_string(), v));
    }

    // Add host
    entries.push((
        "host".to_string(),
        HeaderValue::from_static("api.anthropic.com"),
    ));

    // Add x-claude-code-session-id
    if let Ok(v) = HeaderValue::from_str(&account.session_uuid) {
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

/// Ensure anthropic-beta header contains oauth-2025-04-20.
fn ensure_oauth_beta(entries: &mut Vec<(String, HeaderValue)>) {
    let beta_key = "anthropic-beta";
    let required = "oauth-2025-04-20";

    if let Some(entry) = entries.iter_mut().find(|(n, _)| n == beta_key) {
        let current = entry.1.to_str().unwrap_or("");
        if !current.split(',').any(|b| b.trim() == required) {
            // Avoid leading comma when current is empty.
            let new_value = if current.trim().is_empty() {
                required.to_string()
            } else {
                format!("{},{}", current, required)
            };
            if let Ok(v) = HeaderValue::from_str(&new_value) {
                entry.1 = v;
            }
        }
    } else {
        // No anthropic-beta header at all — add it
        if let Ok(v) = HeaderValue::from_str(required) {
            entries.push((beta_key.to_string(), v));
        }
    }
}

/// Build the outbound URL with ?beta=true query param.
pub fn build_outbound_url(base_url: &str, path: &str) -> String {
    let separator = if path.contains('?') { "&" } else { "?" };
    format!("{}{}{}beta=true", base_url, path, separator)
}

/// Replace metadata.user_id in the request body JSON with per-account values.
/// Replace metadata.user_id with per-account deterministic mapping.
pub fn replace_metadata_user_id(
    body: &[u8],
    account: &AccountIdentity,
) -> Result<Vec<u8>, claude_ultra_http::Error> {
    let mut value: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Ok(body.to_vec()),
    };
    apply_metadata_in_place(&mut value, account)?;
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
    account: &AccountIdentity,
) -> Result<(), claude_ultra_http::Error> {
    // License check (distribution mode only)
    #[cfg(not(feature = "internal"))]
    {
        claude_ultra_http::license::check_license(&account.account_uuid)
            .map_err(claude_ultra_http::Error::license)?;
    }

    if let Some(metadata) = value.get_mut("metadata") {
        // Extract original session_id from the incoming metadata.user_id
        let original_session_id = metadata
            .get("user_id")
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.get("session_id").and_then(|s| s.as_str().map(String::from)))
            .unwrap_or_default();

        // Deterministic mapping: UUID v5 from "original_session_id@account_uuid"
        let mapped_session_id = if original_session_id.is_empty() {
            account.session_uuid.clone()
        } else {
            let input = format!("{}@{}", original_session_id, account.account_uuid);
            uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, input.as_bytes()).to_string()
        };

        let user_id_json = serde_json::json!({
            "device_id": account.device_id,
            "account_uuid": account.account_uuid,
            "session_id": mapped_session_id,
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

    fn test_account() -> AccountIdentity {
        AccountIdentity {
            access_token: "sk-ant-oat01-test-token".to_string(),
            session_uuid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            device_id: "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(),
            account_uuid: "11111111-2222-3333-4444-555555555555".to_string(),
        }
    }

    // ── Header building tests (≥10) ────────────────────────────────────

    #[test]
    fn test_authorization_replaced() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer sk-ant-oat01-test-token"
        );
    }

    #[test]
    fn test_session_id_replaced() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
        assert_eq!(
            headers.get("x-claude-code-session-id").unwrap().to_str().unwrap(),
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        );
    }

    #[test]
    fn test_x_api_key_removed() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
        assert!(headers.get("x-api-key").is_none(), "x-api-key must be removed");
    }

    #[test]
    fn test_x_client_request_id_generated() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
        let id = headers.get("x-client-request-id").expect("must have x-client-request-id");
        let id_str = id.to_str().unwrap();
        // UUID format: 8-4-4-4-12
        assert_eq!(id_str.len(), 36);
        assert_eq!(id_str.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn test_anthropic_beta_contains_oauth() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        assert!(
            beta.split(',').any(|b| b.trim() == "oauth-2025-04-20"),
            "anthropic-beta must contain oauth-2025-04-20, got: {}",
            beta
        );
    }

    #[test]
    fn test_anthropic_beta_preserves_existing() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        assert!(beta.contains("claude-code-20250219"), "original beta values must be preserved");
        assert!(beta.contains("interleaved-thinking-2025-05-14"));
    }

    #[test]
    fn test_anthropic_beta_no_duplicate_oauth() {
        let mut h = sample_client_headers();
        h.insert("anthropic-beta", "claude-code-20250219,oauth-2025-04-20".parse().unwrap());
        let headers = build_outbound_headers(&h, &test_account(), 1024);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        let oauth_count = beta.split(',').filter(|b| b.trim() == "oauth-2025-04-20").count();
        assert_eq!(oauth_count, 1, "oauth-2025-04-20 should not be duplicated");
    }

    #[test]
    fn test_anthropic_beta_empty_header_no_leading_comma() {
        // Regression for P3-B: empty anthropic-beta must not produce ",oauth-2025-04-20"
        let mut h = sample_client_headers();
        h.insert("anthropic-beta", "".parse().unwrap());
        let headers = build_outbound_headers(&h, &test_account(), 1024);
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        assert_eq!(beta, "oauth-2025-04-20", "no leading comma when current is empty");
    }

    #[test]
    fn test_host_replaced() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
        assert_eq!(
            headers.get("host").unwrap().to_str().unwrap(),
            "api.anthropic.com"
        );
    }

    #[test]
    fn test_stainless_headers_passthrough() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
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
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
        assert_eq!(
            headers.get("user-agent").unwrap().to_str().unwrap(),
            "claude-cli/2.1.92 (subscriber, cli)"
        );
    }

    #[test]
    fn test_anthropic_version_passthrough() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
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
        let headers = build_outbound_headers(&h, &test_account(), 9999);
        assert_eq!(
            headers.get("content-length").unwrap().to_str().unwrap(),
            "9999",
            "content-length must reflect actual body size, not client's value"
        );
    }

    #[test]
    fn test_headers_sorted_alphabetically() {
        let headers = build_outbound_headers(&sample_client_headers(), &test_account(), 1024);
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
        // session_id is deterministically mapped from original + account_uuid
        let expected_session = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            format!("old_session@{}", test_account().account_uuid).as_bytes(),
        ).to_string();
        assert_eq!(user_id["session_id"].as_str().unwrap(), expected_session);
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

        // Verify large content preserved
        assert_eq!(
            parsed["messages"][0]["content"].as_str().unwrap().len(),
            120_000
        );
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

    /// Full field-order integrity: all fields before and after stream injection
    /// must maintain their relative positions (except stream itself when newly added).
    #[test]
    #[cfg(feature = "internal")]
    fn test_field_order_integrity_after_metadata_replacement() {
        let raw = r#"{"model":"claude-opus-4-6","max_tokens":64000,"messages":[{"role":"user","content":"hi"}],"system":[],"tools":[],"metadata":{"user_id":"{\"device_id\":\"d1\",\"account_uuid\":\"a1\",\"session_id\":\"s1\"}"},"thinking":{"type":"adaptive"},"stream":true,"context_management":{},"output_config":{}}"#;
        let original_keys: Vec<String> = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(raw)
            .unwrap()
            .keys()
            .cloned()
            .collect();

        let result = replace_metadata_user_id(raw.as_bytes(), &test_account()).unwrap();
        let result_keys: Vec<String> = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&result)
            .unwrap()
            .keys()
            .cloned()
            .collect();

        assert_eq!(original_keys, result_keys, "all fields must maintain original order after full roundtrip");
    }
}
