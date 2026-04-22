use http::HeaderMap;

use crate::modules::cli_client::{RateLimit, Utilization};

/// A single rate-limit window (5h / 7d / overage).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAILimit {
    pub status: String,
    pub utilization: f64,
    pub reset_at: i64, // Unix seconds
}

/// Snapshot of Anthropic rate-limit state parsed from response headers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub status: String, // "allowed" | "allowed_warning" | "rejected"
    pub five_hour: Option<ClaudeAILimit>,
    pub seven_day: Option<ClaudeAILimit>,
    pub overage: Option<ClaudeAILimit>,
    pub representative_claim: Option<String>,
    pub fallback: Option<String>,
    pub fallback_percentage: Option<f64>,
    pub reset_at: Option<i64>,
    pub organization_id: Option<String>,
    pub updated_at: i64,
}

/// Parse rate-limit headers from an upstream response into a QuotaSnapshot.
/// Returns None if the unified-status header is missing.
pub fn parse_quota_headers(headers: &HeaderMap) -> Option<QuotaSnapshot> {
    let status = get_header_str(headers, "anthropic-ratelimit-unified-status")?;

    Some(QuotaSnapshot {
        status,
        five_hour: parse_window(headers, "anthropic-ratelimit-unified-5h"),
        seven_day: parse_window(headers, "anthropic-ratelimit-unified-7d"),
        overage: parse_window(headers, "anthropic-ratelimit-unified-overage"),
        representative_claim: get_header_str(headers, "anthropic-ratelimit-unified-representative-claim"),
        fallback: get_header_str(headers, "anthropic-ratelimit-unified-fallback"),
        fallback_percentage: get_header_f64(headers, "anthropic-ratelimit-unified-fallback-percentage"),
        reset_at: get_header_i64(headers, "anthropic-ratelimit-unified-reset"),
        organization_id: get_header_str(headers, "anthropic-organization-id"),
        updated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    })
}

fn get_header_str(headers: &HeaderMap, key: &str) -> Option<String> {
    headers.get(key)?.to_str().ok().map(|s| s.to_string())
}

fn get_header_f64(headers: &HeaderMap, key: &str) -> Option<f64> {
    let v: f64 = headers.get(key)?.to_str().ok()?.parse().ok()?;
    if v.is_finite() { Some(v) } else { None }
}

fn get_header_i64(headers: &HeaderMap, key: &str) -> Option<i64> {
    headers.get(key)?.to_str().ok()?.parse().ok()
}

fn parse_window(headers: &HeaderMap, prefix: &str) -> Option<ClaudeAILimit> {
    let status = get_header_str(headers, &format!("{}-status", prefix))?;
    let utilization = get_header_f64(headers, &format!("{}-utilization", prefix))?;
    let reset_at = get_header_i64(headers, &format!("{}-reset", prefix))?;
    Some(ClaudeAILimit {
        status,
        utilization,
        reset_at,
    })
}

/// Convert a QuotaSnapshot (response headers, utilization 0-1) to Utilization (API format, utilization 0-100).
/// Only covers unified 5h + 7d. Per-model fields are left as None (use merge_utilization to preserve existing data).
pub fn snapshot_to_utilization(s: &QuotaSnapshot) -> Utilization {
    fn convert_window(w: &ClaudeAILimit) -> RateLimit {
        RateLimit {
            utilization: Some(w.utilization * 100.0),
            resets_at: chrono::DateTime::from_timestamp(w.reset_at, 0)
                .map(|dt| dt.to_rfc3339()),
        }
    }

    Utilization {
        five_hour: s.five_hour.as_ref().map(convert_window),
        seven_day: s.seven_day.as_ref().map(convert_window),
        seven_day_sonnet: None,
        seven_day_opus: None,
        seven_day_oauth_apps: None,
        extra_usage: None,
    }
}

/// Convert a Utilization (API format, utilization 0-100) to QuotaSnapshot (internal, utilization 0-1).
/// Inverse of snapshot_to_utilization. Used when get_usage result needs to feed into ClientManager.update_quota.
pub fn utilization_to_snapshot(u: &Utilization) -> QuotaSnapshot {
    fn convert_window(w: &RateLimit) -> Option<ClaudeAILimit> {
        let utilization = w.utilization? / 100.0;
        let reset_at = w.resets_at.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp())
            .unwrap_or(0);
        Some(ClaudeAILimit {
            status: if utilization >= 0.95 { "rejected" } else if utilization >= 0.8 { "allowed_warning" } else { "allowed" }.to_string(),
            utilization,
            reset_at,
        })
    }

    let five_hour = u.five_hour.as_ref().and_then(convert_window);
    let seven_day = u.seven_day.as_ref().and_then(convert_window);

    QuotaSnapshot {
        status: five_hour.as_ref().map(|w| w.status.clone()).unwrap_or_else(|| "allowed".to_string()),
        five_hour,
        seven_day,
        overage: None,
        representative_claim: None,
        fallback: None,
        fallback_percentage: None,
        reset_at: None,
        organization_id: None,
        updated_at: chrono::Utc::now().timestamp_millis(),
    }
}

/// Merge two Utilization values: non-None fields in new_val override existing, None fields preserve existing.
pub fn merge_utilization(existing: Option<Utilization>, new_val: Utilization) -> Utilization {
    match existing {
        None => new_val,
        Some(old) => Utilization {
            five_hour: new_val.five_hour.or(old.five_hour),
            seven_day: new_val.seven_day.or(old.seven_day),
            seven_day_sonnet: new_val.seven_day_sonnet.or(old.seven_day_sonnet),
            seven_day_opus: new_val.seven_day_opus.or(old.seven_day_opus),
            seven_day_oauth_apps: new_val.seven_day_oauth_apps.or(old.seven_day_oauth_apps),
            extra_usage: new_val.extra_usage.or(old.extra_usage),
        },
    }
}

/// Check if utilization data is stale (five_hour.resets_at is in the past or missing).
pub fn is_utilization_stale(val: &serde_json::Value) -> bool {
    let resets_at = val
        .get("five_hour")
        .and_then(|w| w.get("resets_at"))
        .and_then(|v| v.as_str());

    match resets_at {
        None => true,
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.timestamp_millis() < chrono::Utc::now().timestamp_millis())
            .unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    fn full_rate_limit_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("anthropic-ratelimit-unified-status", "allowed".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-status", "allowed".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-reset", "1775523600".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-utilization", "0.86".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-7d-status", "allowed".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-7d-reset", "1775919600".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-7d-utilization", "0.61".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-overage-status", "allowed".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-overage-reset", "1775512800".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-overage-utilization", "0.0".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-representative-claim", "five_hour".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-fallback-percentage", "0.5".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-fallback", "available".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-reset", "1775523600".parse().unwrap());
        h.insert("anthropic-organization-id", "ff91a96c-7e7b-445b-8322-8125c119ee85".parse().unwrap());
        h
    }

    #[test]
    fn test_parse_full_rate_limit_headers() {
        let headers = full_rate_limit_headers();
        let snapshot = parse_quota_headers(&headers).expect("should parse full headers");
        assert_eq!(snapshot.status, "allowed");
        assert_eq!(snapshot.organization_id.as_deref(), Some("ff91a96c-7e7b-445b-8322-8125c119ee85"));
        assert_eq!(snapshot.representative_claim.as_deref(), Some("five_hour"));
        assert_eq!(snapshot.fallback.as_deref(), Some("available"));
        assert_eq!(snapshot.fallback_percentage, Some(0.5));
        assert_eq!(snapshot.reset_at, Some(1775523600));
    }

    #[test]
    fn test_parse_five_hour_window() {
        let headers = full_rate_limit_headers();
        let snapshot = parse_quota_headers(&headers).unwrap();
        let w = snapshot.five_hour.expect("should have 5h window");
        assert_eq!(w.status, "allowed");
        assert!((w.utilization - 0.86).abs() < 0.001);
        assert_eq!(w.reset_at, 1775523600);
    }

    #[test]
    fn test_parse_seven_day_window() {
        let headers = full_rate_limit_headers();
        let snapshot = parse_quota_headers(&headers).unwrap();
        let w = snapshot.seven_day.expect("should have 7d window");
        assert_eq!(w.status, "allowed");
        assert!((w.utilization - 0.61).abs() < 0.001);
        assert_eq!(w.reset_at, 1775919600);
    }

    #[test]
    fn test_parse_overage_window() {
        let headers = full_rate_limit_headers();
        let snapshot = parse_quota_headers(&headers).unwrap();
        let w = snapshot.overage.expect("should have overage window");
        assert_eq!(w.status, "allowed");
        assert!((w.utilization - 0.0).abs() < 0.001);
        assert_eq!(w.reset_at, 1775512800);
    }

    #[test]
    fn test_parse_partial_headers_only_5h() {
        let mut h = HeaderMap::new();
        h.insert("anthropic-ratelimit-unified-status", "allowed_warning".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-status", "allowed_warning".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-reset", "1775523600".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-utilization", "0.95".parse().unwrap());
        let snapshot = parse_quota_headers(&h).expect("should parse partial");
        assert_eq!(snapshot.status, "allowed_warning");
        assert!(snapshot.five_hour.is_some());
        assert!(snapshot.seven_day.is_none());
        assert!(snapshot.overage.is_none());
    }

    #[test]
    fn test_parse_no_rate_limit_headers() {
        let h = HeaderMap::new();
        assert!(parse_quota_headers(&h).is_none());
    }

    #[test]
    fn test_utilization_boundary_zero() {
        let mut h = HeaderMap::new();
        h.insert("anthropic-ratelimit-unified-status", "allowed".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-status", "allowed".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-utilization", "0.0".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-reset", "1000000000".parse().unwrap());
        let snapshot = parse_quota_headers(&h).unwrap();
        let w = snapshot.five_hour.unwrap();
        assert!((w.utilization - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_utilization_boundary_one() {
        let mut h = HeaderMap::new();
        h.insert("anthropic-ratelimit-unified-status", "rejected".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-status", "rejected".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-utilization", "1.0".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-reset", "1000000000".parse().unwrap());
        let snapshot = parse_quota_headers(&h).unwrap();
        let w = snapshot.five_hour.unwrap();
        assert!((w.utilization - 1.0).abs() < f64::EPSILON);
        assert_eq!(snapshot.status, "rejected");
    }

    #[test]
    fn test_reset_at_unix_seconds_parsed_correctly() {
        let mut h = HeaderMap::new();
        h.insert("anthropic-ratelimit-unified-status", "allowed".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-reset", "1775523600".parse().unwrap());
        let snapshot = parse_quota_headers(&h).unwrap();
        assert_eq!(snapshot.reset_at, Some(1775523600_i64));
    }

    #[test]
    fn test_nan_inf_utilization_rejected() {
        for bad in &["NaN", "inf", "-inf", "Infinity"] {
            let mut h = HeaderMap::new();
            h.insert("anthropic-ratelimit-unified-status", "allowed".parse().unwrap());
            h.insert("anthropic-ratelimit-unified-5h-status", "allowed".parse().unwrap());
            h.insert("anthropic-ratelimit-unified-5h-utilization", bad.parse().unwrap());
            h.insert("anthropic-ratelimit-unified-5h-reset", "1775523600".parse().unwrap());
            let snapshot = parse_quota_headers(&h).unwrap();
            assert!(snapshot.five_hour.is_none(), "should reject {}", bad);
        }
    }

    // ── snapshot_to_utilization ─────────────────────────────

    #[test]
    fn test_snapshot_to_utilization_converts_values() {
        let headers = full_rate_limit_headers();
        let snapshot = parse_quota_headers(&headers).unwrap();
        let util = snapshot_to_utilization(&snapshot);

        // 0.86 → 86.0
        let fh = util.five_hour.unwrap();
        assert!((fh.utilization.unwrap() - 86.0).abs() < 0.01);
        assert!(fh.resets_at.as_ref().unwrap().contains("2026-"));

        // 0.61 → 61.0
        let sd = util.seven_day.unwrap();
        assert!((sd.utilization.unwrap() - 61.0).abs() < 0.01);
        assert!(sd.resets_at.as_ref().unwrap().contains("2026-"));

        // per-model fields are None
        assert!(util.seven_day_sonnet.is_none());
        assert!(util.seven_day_opus.is_none());
        assert!(util.extra_usage.is_none());
    }

    #[test]
    fn test_snapshot_to_utilization_invalid_timestamp() {
        // from_timestamp returns None for out-of-range values
        let snapshot = QuotaSnapshot {
            status: "allowed".to_string(),
            five_hour: Some(ClaudeAILimit {
                status: "allowed".to_string(),
                utilization: 0.5,
                reset_at: i64::MAX, // invalid timestamp
            }),
            seven_day: None,
            overage: None,
            representative_claim: None,
            fallback: None,
            fallback_percentage: None,
            reset_at: None,
            organization_id: None,
            updated_at: 0,
        };
        let util = snapshot_to_utilization(&snapshot);
        let fh = util.five_hour.unwrap();
        assert_eq!(fh.utilization, Some(50.0));
        assert!(fh.resets_at.is_none()); // invalid timestamp → None, not empty string
    }

    #[test]
    fn test_snapshot_to_utilization_partial() {
        let mut h = HeaderMap::new();
        h.insert("anthropic-ratelimit-unified-status", "allowed".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-status", "allowed".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-utilization", "0.5".parse().unwrap());
        h.insert("anthropic-ratelimit-unified-5h-reset", "1775523600".parse().unwrap());
        let snapshot = parse_quota_headers(&h).unwrap();
        let util = snapshot_to_utilization(&snapshot);
        assert!(util.five_hour.is_some());
        assert!(util.seven_day.is_none());
    }

    // ── merge_utilization ───────────────────────────────────

    #[test]
    fn test_merge_utilization_none_existing() {
        let new_val = crate::modules::cli_client::Utilization {
            five_hour: Some(crate::modules::cli_client::RateLimit {
                utilization: Some(50.0),
                resets_at: Some("2026-01-01T00:00:00+00:00".to_string()),
            }),
            seven_day: None,
            seven_day_sonnet: None,
            seven_day_opus: None,
            seven_day_oauth_apps: None,
            extra_usage: None,
        };
        let merged = merge_utilization(None, new_val.clone());
        assert_eq!(merged.five_hour.unwrap().utilization, Some(50.0));
    }

    #[test]
    fn test_merge_utilization_preserves_existing_per_model() {
        let existing = crate::modules::cli_client::Utilization {
            five_hour: Some(crate::modules::cli_client::RateLimit {
                utilization: Some(30.0),
                resets_at: Some("2026-01-01T00:00:00+00:00".to_string()),
            }),
            seven_day: None,
            seven_day_sonnet: Some(crate::modules::cli_client::RateLimit {
                utilization: Some(20.0),
                resets_at: Some("2026-01-07T00:00:00+00:00".to_string()),
            }),
            seven_day_opus: Some(crate::modules::cli_client::RateLimit {
                utilization: Some(10.0),
                resets_at: Some("2026-01-07T00:00:00+00:00".to_string()),
            }),
            seven_day_oauth_apps: None,
            extra_usage: None,
        };
        // new_val only has 5h (from snapshot conversion)
        let new_val = crate::modules::cli_client::Utilization {
            five_hour: Some(crate::modules::cli_client::RateLimit {
                utilization: Some(86.0),
                resets_at: Some("2026-04-12T05:00:00+00:00".to_string()),
            }),
            seven_day: None,
            seven_day_sonnet: None,
            seven_day_opus: None,
            seven_day_oauth_apps: None,
            extra_usage: None,
        };
        let merged = merge_utilization(Some(existing), new_val);
        // 5h overridden
        assert_eq!(merged.five_hour.unwrap().utilization, Some(86.0));
        // per-model preserved
        assert_eq!(merged.seven_day_sonnet.unwrap().utilization, Some(20.0));
        assert_eq!(merged.seven_day_opus.unwrap().utilization, Some(10.0));
    }

    // ── is_utilization_stale ────────────────────────────────

    #[test]
    fn test_is_utilization_stale_no_data() {
        let val = serde_json::json!({});
        assert!(is_utilization_stale(&val));
    }

    #[test]
    fn test_is_utilization_stale_past_reset() {
        let val = serde_json::json!({
            "five_hour": {
                "utilization": 50.0,
                "resets_at": "2020-01-01T00:00:00+00:00"
            }
        });
        assert!(is_utilization_stale(&val));
    }

    #[test]
    fn test_is_utilization_stale_future_reset() {
        let val = serde_json::json!({
            "five_hour": {
                "utilization": 50.0,
                "resets_at": "2099-01-01T00:00:00+00:00"
            }
        });
        assert!(!is_utilization_stale(&val));
    }
}
