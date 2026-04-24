//! Version policy — validates and normalizes inbound requests.
//!
//! `Err(VersionError)` is terminal 400: no retry, no account state mutation.

use claude_ultra_http::compute_fp;
use semver::Version;
use serde_json::Value;

pub const MAX_SUPPORTED_VERSION: &str = "2.1.119";

fn clamp_user_agent(original_ua: &str, original_version: &Version, target_version: &str) -> String {
    let version_str = format!("{}", original_version);
    original_ua.replacen(&format!("/{}", version_str), &format!("/{}", target_version), 1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionError {
    NotFound,
    InvalidFormat(String),
    Mismatch { body: String, ua: String },
}

impl std::fmt::Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionError::NotFound => write!(f, "invalid request"),
            VersionError::InvalidFormat(_) => write!(f, "invalid format"),
            VersionError::Mismatch { .. } => write!(f, "value mismatch"),
        }
    }
}

impl std::error::Error for VersionError {}

#[derive(Debug, Clone)]
pub struct Prepared {
    pub value: Value,
    pub ua_override: Option<String>,
    pub version: Version,
}

impl Prepared {
    pub fn was_clamped(&self) -> bool {
        self.ua_override.is_some()
    }
}

pub fn gate(body: &[u8], user_agent: Option<&str>) -> Result<Prepared, VersionError> {
    let mut value: Value = serde_json::from_slice(body).map_err(|_| VersionError::NotFound)?;

    let billing_text = value
        .get("system")
        .and_then(|s| s.get(0))
        .and_then(|elem| elem.get("text"))
        .and_then(|t| t.as_str())
        .ok_or(VersionError::NotFound)?
        .to_string();

    let fields = parse_billing_fields(&billing_text)?;

    let body_v = parse_cc_cli_version(fields.cc_version)
        .ok_or_else(|| VersionError::InvalidFormat(fields.cc_version.to_string()))?;
    let ua_version_raw = user_agent.and_then(extract_cc_version_from_ua);
    let ua_v = match &ua_version_raw {
        Some(s) => Some(
            parse_cc_cli_version(s)
                .ok_or_else(|| VersionError::InvalidFormat(s.to_string()))?,
        ),
        None => None,
    };

    let version = match (Some(body_v.clone()), ua_v) {
        (Some(b), Some(u)) if b == u => b,
        (Some(b), Some(u)) => {
            return Err(VersionError::Mismatch {
                body: b.to_string(),
                ua: u.to_string(),
            })
        }
        (Some(b), None) => b,
        _ => unreachable!("body_v always Some after parse_billing_fields"),
    };

    let clamp_target: Version = Version::parse(MAX_SUPPORTED_VERSION)
        .expect("MAX_SUPPORTED_VERSION must be valid semver");
    let (effective_version, ua_override) = if version > clamp_target {
        let new_ua = clamp_user_agent(
            user_agent.unwrap_or("claude-cli/0.0.0 (external, cli)"),
            &version,
            MAX_SUPPORTED_VERSION,
        );
        (clamp_target.clone(), Some(new_ua))
    } else {
        (version, None)
    };

    let (original_version_str, original_fp) = split_version_fp(fields.cc_version)?;
    let matched_text = find_verified_user_text(
        &value, original_version_str, original_fp,
    )
    .ok_or_else(|| {
        VersionError::InvalidFormat(
            "no user message matches upstream fingerprint".to_string(),
        )
    })?;
    let effective_version_str = format!("{}", effective_version);
    let new_fp = compute_fp(&matched_text, &effective_version_str);
    let new_cc_version = format!("{}.{}", effective_version_str, new_fp);

    let new_text = rewrite_billing_header(&billing_text, &new_cc_version);

    if let Some(elem) = value
        .get_mut("system")
        .and_then(|s| s.get_mut(0))
        .and_then(|e| e.get_mut("text"))
    {
        *elem = Value::String(new_text);
    }

    Ok(Prepared {
        value,
        ua_override,
        version: effective_version,
    })
}

// ── Fingerprint verification ────────────────────────────────────────────────

fn find_verified_user_text(
    body: &Value,
    original_version: &str,
    original_fp: &str,
) -> Option<String> {
    let messages = body.get("messages").and_then(|m| m.as_array())?;
    let mut saw_user = false;
    let mut saw_verifiable_text = false;
    for msg in messages {
        if msg.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        saw_user = true;
        let content = &msg["content"];
        if let Some(s) = content.as_str() {
            saw_verifiable_text = true;
            if compute_fp(s, original_version) == original_fp {
                return Some(s.to_string());
            }
            continue;
        }
        if let Some(arr) = content.as_array() {
            for block in arr {
                if block.get("type").and_then(|t| t.as_str()) != Some("text") {
                    continue;
                }
                let text = match block.get("text").and_then(|t| t.as_str()) {
                    Some(t) => t,
                    None => continue,
                };
                if text.starts_with("<system-reminder>") {
                    continue;
                }
                saw_verifiable_text = true;
                if compute_fp(text, original_version) == original_fp {
                    return Some(text.to_string());
                }
            }
        }
    }
    if saw_user && !saw_verifiable_text && compute_fp("", original_version) == original_fp {
        return Some(String::new());
    }
    None
}

// ── Private helpers ─────────────────────────────────────────────────────────

#[derive(Debug)]
struct BillingFields<'a> {
    cc_version: &'a str,
}

fn split_header_prefix(text: &str) -> (&str, &str) {
    if let Some(pos) = text.find(": ") {
        let end = pos + 2;
        (&text[..end], &text[end..])
    } else {
        ("", text)
    }
}

fn parse_billing_fields(text: &str) -> Result<BillingFields<'_>, VersionError> {
    let (_, value_part) = split_header_prefix(text);
    let mut cc_version: Option<&str> = None;
    let mut cch_found = false;

    for frag in value_part.split(';') {
        let trimmed = frag.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(val) = trimmed.strip_prefix("cc_version=") {
            if cc_version.is_some() {
                return Err(VersionError::InvalidFormat(
                    "duplicate cc_version= in billing header".to_string(),
                ));
            }
            cc_version = Some(val);
        } else if trimmed.strip_prefix("cch=").is_some() {
            if cch_found {
                return Err(VersionError::InvalidFormat(
                    "duplicate cch= in billing header".to_string(),
                ));
            }
            cch_found = true;
        }
    }

    let cc_version = cc_version.ok_or(VersionError::NotFound)?;
    if !cch_found {
        return Err(VersionError::NotFound);
    }
    if cc_version.is_empty() {
        return Err(VersionError::InvalidFormat("empty cc_version".to_string()));
    }
    Ok(BillingFields { cc_version })
}

fn rewrite_billing_header(text: &str, new_cc_version: &str) -> String {
    let (prefix, value_part) = split_header_prefix(text);
    let parts: Vec<&str> = value_part.split(';').collect();
    let mut out = Vec::with_capacity(parts.len());
    for part in &parts {
        let trimmed = part.trim();
        if trimmed.strip_prefix("cc_version=").is_some() {
            if part.starts_with(' ') {
                out.push(format!(" cc_version={}", new_cc_version));
            } else {
                out.push(format!("cc_version={}", new_cc_version));
            }
        } else if trimmed.strip_prefix("cch=").is_some() {
            if part.starts_with(' ') {
                out.push(" cch=00000".to_string());
            } else {
                out.push("cch=00000".to_string());
            }
        } else {
            out.push(part.to_string());
        }
    }
    format!("{}{}", prefix, out.join(";"))
}

fn split_version_fp(raw: &str) -> Result<(&str, &str), VersionError> {
    let last_dot = raw.rfind('.').ok_or(VersionError::InvalidFormat(
        format!("cc_version missing fingerprint segment: {}", raw),
    ))?;
    let version_core = &raw[..last_dot];
    let fp = &raw[last_dot + 1..];
    if fp.is_empty() || version_core.is_empty() {
        return Err(VersionError::InvalidFormat(
            format!("cc_version missing fingerprint segment: {}", raw),
        ));
    }
    Ok((version_core, fp))
}

fn extract_cc_version_from_ua(ua: &str) -> Option<String> {
    let prefix = "claude-cli/";
    let rest = ua.trim_start().strip_prefix(prefix)?;
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '(')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn parse_cc_cli_version(raw: &str) -> Option<Version> {
    if let Ok(v) = Version::parse(raw) {
        return Some(v);
    }
    let mut it = raw.split('.');
    let major = parse_core_segment(it.next()?)?;
    let minor = parse_core_segment(it.next()?)?;
    let patch = parse_core_segment(it.next()?)?;
    Some(Version::new(major, minor, patch))
}

fn parse_core_segment(s: &str) -> Option<u64> {
    if s.is_empty() || !s.as_bytes().iter().all(u8::is_ascii_digit) {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return None;
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_billing_fields ──

    #[test]
    fn test_parse_canonical() {
        let f = parse_billing_fields("x-anthropic-billing-header: cc_version=2.1.111.b2b; cc_entrypoint=cli; cch=xxxxx;").unwrap();
        assert_eq!(f.cc_version, "2.1.111.b2b");
    }

    #[test]
    fn test_parse_rejects_invalid_key_prefix() {
        let err = parse_billing_fields("xcc_version=2.1.111; cch=abcde;").unwrap_err();
        assert_eq!(err, VersionError::NotFound);
    }

    #[test]
    fn test_parse_rejects_invalid_key_prefix_2() {
        let err = parse_billing_fields("cc_version=2.1.111; ycch=abcde;").unwrap_err();
        assert_eq!(err, VersionError::NotFound);
    }

    #[test]
    fn test_parse_rejects_duplicate_key() {
        let err = parse_billing_fields("cc_version=2.1.111; cc_version=2.1.200; cch=abcde;").unwrap_err();
        assert!(matches!(err, VersionError::InvalidFormat(_)));
    }

    #[test]
    fn test_parse_rejects_duplicate_key_2() {
        let err = parse_billing_fields("cc_version=2.1.111; cch=abcde; cch=fedcb;").unwrap_err();
        assert!(matches!(err, VersionError::InvalidFormat(_)));
    }

    #[test]
    fn test_parse_rejects_missing_key() {
        let err = parse_billing_fields("cch=abcde;").unwrap_err();
        assert_eq!(err, VersionError::NotFound);
    }

    #[test]
    fn test_parse_rejects_missing_key_2() {
        let err = parse_billing_fields("cc_version=2.1.111;").unwrap_err();
        assert_eq!(err, VersionError::NotFound);
    }

    #[test]
    fn test_parse_accepts_any_value() {
        // cch value is accepted as-is at parse time.
        assert!(parse_billing_fields("cc_version=2.1.111; cch=ZZZZZ;").is_ok());
        assert!(parse_billing_fields("cc_version=2.1.111; cch=;").is_ok());
    }

    // ── rewrite_billing_header ──

    #[test]
    fn test_rewrite_canonical() {
        let t = "x-anthropic-billing-header: cc_version=2.1.200.xyz; cc_entrypoint=cli; cch=abcde;";
        let out = rewrite_billing_header(t, "2.1.111.b2b");
        assert!(out.contains("cc_version=2.1.111.b2b"));
        assert!(out.contains("cc_entrypoint=cli"));
        assert!(out.contains("cch=00000"));
        assert!(!out.contains("2.1.200"));
        assert!(!out.contains("abcde"));
    }

    #[test]
    fn test_rewrite_preserves_other_fields() {
        let t = "cc_version=2.1.111.b2b; cc_entrypoint=sdk-cli; cc_workload=test; cch=xxxxx;";
        let out = rewrite_billing_header(t, "2.1.111.b2b");
        assert!(out.contains("cc_entrypoint=sdk-cli"), "cc_entrypoint passthrough");
        assert!(out.contains("cc_workload=test"), "cc_workload passthrough");
        assert!(out.contains("cch=00000"), "cch normalized");
    }

    #[test]
    fn test_rewrite_normalizes_non_ascii() {
        let t = "cc_version=2.1.111.b2b; cch=你好啊;";
        let out = rewrite_billing_header(t, "2.1.111.b2b");
        assert!(out.contains("cch=00000"));
    }

    // ── extract_cc_version_from_ua ──

    #[test]
    fn test_extract_ua_standard() {
        assert_eq!(
            extract_cc_version_from_ua("claude-cli/2.1.111 (external, cli)"),
            Some("2.1.111".to_string())
        );
    }

    #[test]
    fn test_extract_ua_leading_whitespace() {
        assert_eq!(
            extract_cc_version_from_ua(" claude-cli/2.1.111 (external, cli)"),
            Some("2.1.111".to_string())
        );
    }

    #[test]
    fn test_extract_ua_tab_separator() {
        assert_eq!(
            extract_cc_version_from_ua("claude-cli/2.1.111\t(external, cli)"),
            Some("2.1.111".to_string())
        );
    }

    #[test]
    fn test_extract_ua_wrong_prefix() {
        assert_eq!(extract_cc_version_from_ua("Mozilla/5.0"), None);
    }

    // ── parse_cc_cli_version ──

    #[test]
    fn test_parse_strict_semver() {
        assert_eq!(parse_cc_cli_version("2.1.111"), Some(Version::new(2, 1, 111)));
    }

    #[test]
    fn test_parse_with_fingerprint_hex() {
        assert_eq!(
            parse_cc_cli_version("2.1.111.b2b"),
            Some(Version::new(2, 1, 111))
        );
    }

    #[test]
    fn test_parse_with_fingerprint_numeric() {
        assert_eq!(
            parse_cc_cli_version("2.1.110.610"),
            Some(Version::new(2, 1, 110))
        );
    }

    #[test]
    fn test_parse_rejects_plus_prefix() {
        assert_eq!(parse_cc_cli_version("+2.1.111.fp"), None);
    }

    #[test]
    fn test_parse_rejects_leading_zero() {
        assert_eq!(parse_cc_cli_version("02.1.111.fp"), None);
    }

    #[test]
    fn test_parse_rejects_non_digit_core() {
        assert_eq!(parse_cc_cli_version("dev"), None);
        assert_eq!(parse_cc_cli_version("2.1"), None);
        assert_eq!(parse_cc_cli_version("abc.def.ghi"), None);
    }

    // ── gate (public API) ──

    #[test]
    #[cfg(feature = "internal")]
    fn test_gate_passes_through_safe_version() {
        let body = br#"{"messages":[{"role":"user","content":"hi"}],"system":[{"type":"text","text":"cc_version=2.1.111.b2b; cc_entrypoint=cli; cch=xxxxx;"}]}"#;
        let p = gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap();
        assert!(!p.was_clamped());
        let text = p.value["system"][0]["text"].as_str().unwrap();
        assert!(text.contains("cch=00000"));
        // fp recomputed: "hi" → chars="000" + version "2.1.111" → fp="b2b"
        assert!(text.contains("cc_version=2.1.111.b2b"), "fp recomputed, got: {}", text);
        assert_eq!(p.version, Version::new(2, 1, 111));
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_gate_passes_through_old_version() {
        let body = br#"{"messages":[{"role":"user","content":"hi"}],"system":[{"type":"text","text":"cc_version=2.1.97.d5e; cc_entrypoint=cli; cch=xxxxx;"}]}"#;
        let p = gate(body, Some("claude-cli/2.1.97 (external, cli)")).unwrap();
        assert!(!p.was_clamped());
        let text = p.value["system"][0]["text"].as_str().unwrap();
        assert!(text.contains("cch=00000"));
        assert!(text.contains("cc_version=2.1.97.d5e"), "fp recomputed for 2.1.97");
        assert_eq!(p.version, Version::new(2, 1, 97));
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_gate_adjusts_newer_version() {
        let body = br#"{"messages":[{"role":"user","content":"hi"}],"system":[{"type":"text","text":"cc_version=2.1.200.bf1; cc_entrypoint=cli; cch=xxxxx;"}]}"#;
        let p = gate(body, Some("claude-cli/2.1.200 (external, cli)")).unwrap();
        assert!(p.was_clamped());
        assert_eq!(p.ua_override.as_deref(), Some("claude-cli/2.1.119 (external, cli)"));
        let text = p.value["system"][0]["text"].as_str().unwrap();
        // fp recomputed with clamped version
        assert!(text.contains("cc_version=2.1.119."), "clamped to MAX, got: {}", text);
        assert!(text.contains("cch=00000"));
        assert!(!text.contains("2.1.200"));
        assert_eq!(p.version, Version::new(2, 1, 119));
    }

    #[test]
    fn test_gate_mismatch_rejects() {
        let body = br#"{"system":[{"type":"text","text":"cc_version=2.1.110.610; cc_entrypoint=cli; cch=xxxxx;"}]}"#;
        let err = gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap_err();
        assert!(matches!(err, VersionError::Mismatch { .. }));
    }

    #[test]
    fn test_gate_no_top_level_system_rejects() {
        let body = br#"{"messages":[{"role":"user","content":"cc_version=2.1.109;"}]}"#;
        assert_eq!(gate(body, None).unwrap_err(), VersionError::NotFound);
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_gate_messages_isolation() {
        let body = br#"{"messages":[{"role":"user","content":"cc_version=2.1.109;"}],"system":[{"type":"text","text":"cc_version=2.1.111.b07; cc_entrypoint=cli; cch=xxxxx;"}]}"#;
        let p = gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap();
        assert_eq!(p.version, Version::new(2, 1, 111));
    }

    #[test]
    fn test_gate_invalid_body_json_rejects() {
        let body = b"not json at all";
        assert_eq!(gate(body, None).unwrap_err(), VersionError::NotFound);
    }

    #[test]
    fn test_gate_invalid_format_rejects() {
        let body = br#"{"system":[{"type":"text","text":"cc_version=dev; cch=00000;"}]}"#;
        let err = gate(body, None).unwrap_err();
        assert!(matches!(err, VersionError::InvalidFormat(_)));
    }

    // ── validation ──

    #[test]
    fn test_gate_rejects_invalid_prefix() {
        let body = br#"{"system":[{"type":"text","text":"xcc_version=2.1.111; cch=abcde;"}]}"#;
        assert_eq!(gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap_err(), VersionError::NotFound);
    }

    #[test]
    fn test_gate_rejects_invalid_prefix_2() {
        let body = br#"{"system":[{"type":"text","text":"cc_version=2.1.111; ycch=abcde;"}]}"#;
        assert_eq!(gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap_err(), VersionError::NotFound);
    }

    // ── normalization ──

    #[test]
    #[cfg(feature = "internal")]
    fn test_gate_normalizes_invalid_value() {
        let body = br#"{"messages":[{"role":"user","content":"hi"}],"system":[{"type":"text","text":"cc_version=2.1.111.b2b; cch=ZZZZZ;"}]}"#;
        let p = gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap();
        let text = p.value["system"][0]["text"].as_str().unwrap();
        assert!(text.contains("cch=00000"), "non-hex cch overwritten");
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_gate_normalizes_non_ascii_value() {
        let body = br#"{"messages":[{"role":"user","content":"hi"}],"system":[{"type":"text","text":"cc_version=2.1.111.b2b; cch=\u4f60\u597d\u554a;"}]}"#;
        let p = gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap();
        let text = p.value["system"][0]["text"].as_str().unwrap();
        assert!(text.contains("cch=00000"), "non-ascii cch overwritten");
    }

    #[test]
    fn test_gate_rejects_missing_field() {
        let body = br#"{"system":[{"type":"text","text":"cch=xxxxx;"}]}"#;
        assert_eq!(gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap_err(), VersionError::NotFound);
    }

    #[test]
    fn test_gate_rejects_missing_field_2() {
        let body = br#"{"system":[{"type":"text","text":"cc_version=2.1.111;"}]}"#;
        assert_eq!(gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap_err(), VersionError::NotFound);
    }

    // ── version adjustment ──

    #[test]
    #[cfg(feature = "internal")]
    fn test_gate_output_varies_with_input() {
        // fp("Write a hello world in Python", "2.1.111")="3db"
        let body = br#"{"messages":[{"role":"user","content":"Write a hello world in Python"}],"system":[{"type":"text","text":"cc_version=2.1.111.3db; cc_entrypoint=cli; cch=xxxxx;"}]}"#;
        let p = gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap();
        let text = p.value["system"][0]["text"].as_str().unwrap();
        assert!(text.contains("cc_version=2.1.111.3db"), "fp recomputed from long prompt, got: {}", text);
    }

    #[test]
    fn test_gate_rejects_unverified_input() {
        // cc_version=2.1.111.zzz but fp("hi","2.1.111")="b2b" — "zzz" is fake.
        // Dual verification finds no matching message → 400.
        let body = br#"{"messages":[{"role":"user","content":"hi"}],"system":[{"type":"text","text":"cc_version=2.1.111.zzz; cc_entrypoint=cli; cch=xxxxx;"}]}"#;
        let err = gate(body, Some("claude-cli/2.1.111 (external, cli)")).unwrap_err();
        assert!(matches!(err, VersionError::InvalidFormat(_)));
    }

    #[test]
    fn test_gate_rejects_unverified_input_2() {
        // Fake fp with future version — still rejected because no message matches.
        let body = br#"{"messages":[{"role":"user","content":"hi"}],"system":[{"type":"text","text":"cc_version=2.1.200.zzz; cc_entrypoint=cli; cch=xxxxx;"}]}"#;
        let err = gate(body, Some("claude-cli/2.1.200 (external, cli)")).unwrap_err();
        assert!(matches!(err, VersionError::InvalidFormat(_)));
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_gate_output_adjusted_on_version_change() {
        let body = br#"{"messages":[{"role":"user","content":"hi"}],"system":[{"type":"text","text":"cc_version=2.1.200.bf1; cc_entrypoint=cli; cch=xxxxx;"}]}"#;
        let p = gate(body, Some("claude-cli/2.1.200 (external, cli)")).unwrap();
        let text = p.value["system"][0]["text"].as_str().unwrap();
        assert!(text.contains(&format!("cc_version={}.", MAX_SUPPORTED_VERSION)), "fp recomputed on clamp, got: {}", text);
    }

    // ── find_verified_user_text ──

    #[test]
    #[cfg(feature = "internal")]
    fn test_fp_real_scenario_sr_blocks_then_prompt() {
        let body = serde_json::json!({
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "<system-reminder>skills list here</system-reminder>"},
                {"type": "text", "text": "<system-reminder>context here</system-reminder>"},
                {"type": "text", "text": "hi"}
            ]}]
        });
        let result = find_verified_user_text(&body, "2.1.111", "b2b");
        assert_eq!(result, Some("hi".to_string()));
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_fp_real_scenario_sr_blocks_then_long_prompt() {
        let body = serde_json::json!({
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "<system-reminder>skills</system-reminder>"},
                {"type": "text", "text": "Write a hello world in Python"}
            ]}]
        });
        let result = find_verified_user_text(&body, "2.1.111", "3db");
        assert_eq!(result, Some("Write a hello world in Python".to_string()));
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_fp_string_content() {
        let body = serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}]
        });
        assert_eq!(find_verified_user_text(&body, "2.1.111", "b2b"), Some("hi".to_string()));
    }

    #[test]
    fn test_fp_no_match() {
        let body = serde_json::json!({
            "messages": [{"role": "user", "content": "hello world"}]
        });
        assert_eq!(find_verified_user_text(&body, "2.1.111", "zzz"), None);
    }

    #[test]
    fn test_fp_no_messages() {
        let body = serde_json::json!({"system": []});
        assert_eq!(find_verified_user_text(&body, "2.1.111", "b2b"), None);
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_fp_second_message_matches() {
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "<system-reminder>sr</system-reminder>"},
                    {"type": "text", "text": "first prompt that wont match"}
                ]},
                {"role": "assistant", "content": "response"},
                {"role": "user", "content": "Write a hello world in Python"}
            ]
        });
        let result = find_verified_user_text(&body, "2.1.111", "3db");
        assert_eq!(result, Some("Write a hello world in Python".to_string()));
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_fp_4_blocks_match_last() {
        let long_msg = "Write a comprehensive guide to building REST APIs with Rust using the Axum framework";
        assert_eq!(compute_fp(long_msg, "2.1.111"), "853");
        assert_eq!(compute_fp("hi", "2.1.111"), "b2b");

        let body = serde_json::json!({
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "<system-reminder>skills</system-reminder>"},
                {"type": "text", "text": "<system-reminder>context</system-reminder>"},
                {"type": "text", "text": "hi"},
                {"type": "text", "text": long_msg}
            ]}]
        });
        let result = find_verified_user_text(&body, "2.1.111", "853");
        assert_eq!(result, Some(long_msg.to_string()));
    }

    #[test]
    fn test_fp_empty_fallback_does_not_mask_real_mismatch() {
        let body = serde_json::json!({
            "messages": [{"role": "user", "content": "hello world"}]
        });
        assert_eq!(find_verified_user_text(&body, "2.1.111", "b2b"), None);
    }

    #[test]
    #[cfg(feature = "internal")]
    fn test_fp_empty_string_fallback() {
        let body = serde_json::json!({
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "<system-reminder>only sr blocks</system-reminder>"}
            ]}]
        });
        assert_eq!(find_verified_user_text(&body, "2.1.111", "b2b"), Some(String::new()));
    }
}
