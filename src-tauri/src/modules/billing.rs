//! Billing module — model pricing + cost calculation for gateway requests.

use serde::{Deserialize, Serialize};

/// Token usage extracted from SSE stream.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_tokens: u32,
    pub cache_read_tokens: u32,
}

impl TokenUsage {
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }
}

/// Cost breakdown for a single request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_creation_cost: f64,
    pub cache_read_cost: f64,
    pub total_cost: f64,
}

/// Per-million-token pricing for a model.
struct ModelPricing {
    input: f64,
    output: f64,
    cache_creation: f64,
    cache_read: f64,
}

/// Normalize model name variants to a canonical form for pricing lookup.
/// Returns a `&'static str` — all return values are string literals.
pub fn normalize_model(model: &str) -> &'static str {
    let m = model.to_lowercase();
    if m.contains("opus-4-7") {
        "claude-opus-4-7"
    } else if m.contains("opus-4") {
        "claude-opus-4-6"
    } else if m.contains("sonnet-4") {
        "claude-sonnet-4-6"
    } else if m.contains("haiku-4") {
        "claude-haiku-4-5"
    } else {
        "unknown"
    }
}

fn get_pricing(normalized_model: &str) -> Option<ModelPricing> {
    match normalized_model {
        "claude-opus-4-7" | "claude-opus-4-6" => Some(ModelPricing {
            input: 5.0,
            output: 25.0,
            cache_creation: 6.25,
            cache_read: 0.50,
        }),
        "claude-sonnet-4-6" => Some(ModelPricing {
            input: 3.0,
            output: 15.0,
            cache_creation: 3.75,
            cache_read: 0.30,
        }),
        "claude-haiku-4-5" => Some(ModelPricing {
            input: 1.0,
            output: 5.0,
            cache_creation: 1.25,
            cache_read: 0.10,
        }),
        _ => None,
    }
}

/// Calculate cost for given token usage and model.
pub fn calculate_cost(model: &str, usage: &TokenUsage) -> CostBreakdown {
    let normalized = normalize_model(model);
    let pricing = match get_pricing(normalized) {
        Some(p) => p,
        None => return CostBreakdown::default(),
    };

    let input_cost = (usage.input_tokens as f64 / 1_000_000.0) * pricing.input;
    let output_cost = (usage.output_tokens as f64 / 1_000_000.0) * pricing.output;
    let cache_creation_cost =
        (usage.cache_creation_tokens as f64 / 1_000_000.0) * pricing.cache_creation;
    let cache_read_cost = (usage.cache_read_tokens as f64 / 1_000_000.0) * pricing.cache_read;

    CostBreakdown {
        input_cost,
        output_cost,
        cache_creation_cost,
        cache_read_cost,
        total_cost: input_cost + output_cost + cache_creation_cost + cache_read_cost,
    }
}

/// Extract token usage from a JSON SSE data line.
/// Looks for `usage` object in message_delta or message_stop events.
pub fn extract_usage_from_sse_line(line: &str) -> Option<TokenUsage> {
    // SSE format: "data: {json}"
    let json_str = line.strip_prefix("data: ")?;
    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;

    // Usage can be in top-level "usage" or nested in "message.usage"
    let usage = value.get("usage").or_else(|| {
        value.get("message").and_then(|m| m.get("usage"))
    })?;

    Some(TokenUsage {
        input_tokens: usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        output_tokens: usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        cache_creation_tokens: usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        cache_read_tokens: usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Model normalization ──────────────────────────────────

    #[test]
    fn test_normalize_opus_variants() {
        assert_eq!(normalize_model("claude-opus-4-7"), "claude-opus-4-7");
        assert_eq!(normalize_model("claude-opus-4-7[1m]"), "claude-opus-4-7");
        assert_eq!(normalize_model("claude-opus-4-6"), "claude-opus-4-6");
        assert_eq!(normalize_model("claude-opus-4-6[1m]"), "claude-opus-4-6");
        assert_eq!(normalize_model("claude-opus-4-20250514"), "claude-opus-4-6");
    }

    #[test]
    fn test_normalize_sonnet_variants() {
        assert_eq!(normalize_model("claude-sonnet-4-6"), "claude-sonnet-4-6");
        assert_eq!(normalize_model("claude-sonnet-4-20250514"), "claude-sonnet-4-6");
    }

    #[test]
    fn test_normalize_haiku_variants() {
        assert_eq!(normalize_model("claude-haiku-4-5"), "claude-haiku-4-5");
        assert_eq!(normalize_model("claude-haiku-4-5-20241001"), "claude-haiku-4-5");
    }

    #[test]
    fn test_normalize_unknown_model() {
        assert_eq!(normalize_model("gpt-4"), "unknown");
        assert_eq!(normalize_model("unknown-model"), "unknown");
    }

    // ── Cost calculation ─────────────────────────────────────

    #[test]
    fn test_cost_opus_1m_tokens() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        };
        let cost = calculate_cost("claude-opus-4-6", &usage);
        assert!((cost.input_cost - 5.0).abs() < 0.001);
        assert!((cost.total_cost - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_cost_opus_47_same_as_46() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        };
        let c46 = calculate_cost("claude-opus-4-6", &usage);
        let c47 = calculate_cost("claude-opus-4-7", &usage);
        assert!((c46.total_cost - c47.total_cost).abs() < 0.001);
        assert!((c47.input_cost - 5.0).abs() < 0.001);
        assert!((c47.output_cost - 25.0).abs() < 0.001);
    }

    #[test]
    fn test_cost_sonnet_mixed() {
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            cache_creation_tokens: 10_000,
            cache_read_tokens: 200_000,
        };
        let cost = calculate_cost("claude-sonnet-4-6", &usage);
        // input: 0.1 * 3 = 0.3
        // output: 0.05 * 15 = 0.75
        // cache_creation: 0.01 * 3.75 = 0.0375
        // cache_read: 0.2 * 0.30 = 0.06
        let expected = 0.3 + 0.75 + 0.0375 + 0.06;
        assert!((cost.total_cost - expected).abs() < 0.0001);
    }

    #[test]
    fn test_cost_unknown_model_zero() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        };
        let cost = calculate_cost("gpt-4", &usage);
        assert!((cost.total_cost).abs() < 0.0001);
    }

    // ── SSE usage extraction ─────────────────────────────────

    #[test]
    fn test_extract_usage_message_delta() {
        let line = r#"data: {"type":"message_delta","usage":{"input_tokens":1000,"output_tokens":500}}"#;
        let usage = extract_usage_from_sse_line(line).unwrap();
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 500);
    }

    #[test]
    fn test_extract_usage_with_cache() {
        let line = r#"data: {"type":"message_delta","usage":{"input_tokens":5000,"output_tokens":2000,"cache_creation_input_tokens":1000,"cache_read_input_tokens":3000}}"#;
        let usage = extract_usage_from_sse_line(line).unwrap();
        assert_eq!(usage.input_tokens, 5000);
        assert_eq!(usage.output_tokens, 2000);
        assert_eq!(usage.cache_creation_tokens, 1000);
        assert_eq!(usage.cache_read_tokens, 3000);
        assert_eq!(usage.total_tokens(), 11000);
    }

    #[test]
    fn test_extract_usage_message_start() {
        let line = r#"data: {"type":"message_start","message":{"usage":{"input_tokens":100,"output_tokens":0}}}"#;
        let usage = extract_usage_from_sse_line(line).unwrap();
        assert_eq!(usage.input_tokens, 100);
    }

    #[test]
    fn test_extract_usage_no_usage_field() {
        let line = r#"data: {"type":"content_block_delta","delta":{"text":"hello"}}"#;
        assert!(extract_usage_from_sse_line(line).is_none());
    }

    #[test]
    fn test_extract_usage_not_data_line() {
        let line = r#"event: message_delta"#;
        assert!(extract_usage_from_sse_line(line).is_none());
    }

    #[test]
    fn test_extract_usage_empty_data() {
        let line = "data: [DONE]";
        assert!(extract_usage_from_sse_line(line).is_none());
    }
}
