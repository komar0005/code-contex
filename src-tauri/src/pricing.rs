use crate::model::UsageEvent;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct ModelPricing {
    /// USD per 1M input tokens
    pub input: f64,
    /// USD per 1M output tokens
    pub output: f64,
    /// USD per 1M cache-write tokens
    pub cache_write: f64,
    /// USD per 1M cache-read tokens
    pub cache_read: f64,
}

pub type PricingTable = HashMap<String, ModelPricing>;

const EMBEDDED_PRICING_JSON: &str = include_str!("pricing_data.json");

pub fn embedded_pricing_table() -> PricingTable {
    serde_json::from_str(EMBEDDED_PRICING_JSON)
        .expect("embedded pricing_data.json must be valid JSON")
}

/// Returns `Some(cost_usd)` if `event.model` is in `table`, `None` if unknown.
pub fn event_cost(event: &UsageEvent, table: &PricingTable) -> Option<f64> {
    let price = table.get(&event.model)?;
    let cost = (event.input_tokens as f64 / 1_000_000.0) * price.input
        + (event.output_tokens as f64 / 1_000_000.0) * price.output
        + (event.cache_write_tokens as f64 / 1_000_000.0) * price.cache_write
        + (event.cache_read_tokens as f64 / 1_000_000.0) * price.cache_read;
    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Agent;
    use chrono::{TimeZone, Utc};

    fn event(model: &str, input: u64, output: u64, cache_write: u64, cache_read: u64) -> UsageEvent {
        UsageEvent {
            agent: Agent::ClaudeCode,
            project: "p".into(),
            model: model.into(),
            input_tokens: input,
            output_tokens: output,
            cache_write_tokens: cache_write,
            cache_read_tokens: cache_read,
            timestamp: Utc.with_ymd_and_hms(2026, 7, 14, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn embedded_table_parses_and_has_known_models() {
        let table = embedded_pricing_table();
        assert!(table.contains_key("claude-sonnet-5"));
    }

    #[test]
    fn computes_exact_cost_for_known_model() {
        let table = embedded_pricing_table();
        // 1_000_000 of each token type at claude-sonnet-5 rates:
        // 3.0 + 15.0 + 3.75 + 0.3 = 22.05
        let e = event("claude-sonnet-5", 1_000_000, 1_000_000, 1_000_000, 1_000_000);
        let cost = event_cost(&e, &table).unwrap();
        assert!((cost - 22.05).abs() < 1e-9);
    }

    #[test]
    fn returns_none_for_unknown_model() {
        let table = embedded_pricing_table();
        let e = event("some-future-model", 100, 100, 0, 0);
        assert!(event_cost(&e, &table).is_none());
    }
}
