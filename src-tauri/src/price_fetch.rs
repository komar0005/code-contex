use crate::pricing::{ModelPricing, PricingTable};
use serde_json::Value;

pub trait PriceSource {
    fn fetch(&self) -> Result<String, String>;
}

pub struct HttpPriceSource {
    pub url: String,
}

impl PriceSource for HttpPriceSource {
    fn fetch(&self) -> Result<String, String> {
        // The LiteLLM table is ~1.6 MB and this timeout covers the WHOLE
        // request including the body; 10s aborted mid-download on slow
        // links ("error decoding response body"). Runs at most daily.
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| e.to_string())?;
        client
            .get(&self.url)
            .send()
            .map_err(|e| e.to_string())?
            .text()
            .map_err(|e| e.to_string())
    }
}

/// Parses LiteLLM's `model_prices_and_context_window.json` format (USD per
/// single token, keyed by field names like `input_cost_per_token`) into our
/// internal `PricingTable` (USD per 1,000,000 tokens). Skips any entry
/// missing `input_cost_per_token`/`output_cost_per_token` (e.g. the
/// `"sample_spec"` sentinel, or non-chat entries like embeddings models)
/// rather than failing the whole parse.
pub fn parse_litellm_table(body: &str) -> Option<PricingTable> {
    let root: Value = serde_json::from_str(body).ok()?;
    let object = root.as_object()?;
    let mut table = PricingTable::new();
    for (model_name, entry) in object {
        // LiteLLM's file includes a "sample_spec" sentinel documenting the
        // schema. It carries `input_cost_per_token`/`output_cost_per_token`
        // set to 0.0 as placeholders, so it would otherwise pass the cost-
        // field presence check below and be mistaken for a real free model.
        if model_name == "sample_spec" {
            continue;
        }
        let input = entry.get("input_cost_per_token").and_then(Value::as_f64);
        let output = entry.get("output_cost_per_token").and_then(Value::as_f64);
        let (Some(input), Some(output)) = (input, output) else {
            continue;
        };
        let cache_read = entry
            .get("cache_read_input_token_cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let cache_write = entry
            .get("cache_creation_input_token_cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        table.insert(
            model_name.clone(),
            ModelPricing {
                input: input * 1_000_000.0,
                output: output * 1_000_000.0,
                cache_write: cache_write * 1_000_000.0,
                cache_read: cache_read * 1_000_000.0,
            },
        );
    }
    if table.is_empty() {
        None
    } else {
        Some(table)
    }
}

/// Tries to fetch and parse an updated pricing table from `source`. On any
/// failure (network error or a response with no parseable model entries),
/// logs the reason to stderr and returns `fallback` unchanged, alongside
/// whether the refresh actually succeeded (`true`) or fell back (`false`) —
/// callers that need to know (e.g. to record a "last successful update"
/// timestamp) can use this without duplicating the fallback/logging logic.
pub fn refresh_pricing_table_with_status(
    source: &dyn PriceSource,
    fallback: PricingTable,
) -> (PricingTable, bool) {
    match source.fetch() {
        Ok(body) => match parse_litellm_table(&body) {
            Some(table) => (table, true),
            None => {
                eprintln!("pricing refresh: could not parse any model entries from source");
                (fallback, false)
            }
        },
        Err(e) => {
            eprintln!("pricing refresh: fetch failed: {e}");
            (fallback, false)
        }
    }
}

/// Tries to fetch and parse an updated pricing table from `source`. On any
/// failure (network error or a response with no parseable model entries),
/// logs the reason to stderr and returns `fallback` unchanged.
///
/// Kept as a thin wrapper around `refresh_pricing_table_with_status` for
/// callers that don't need the success flag; production code
/// (`main.rs::refresh_all`) calls `refresh_pricing_table_with_status`
/// directly to record a "last successful update" timestamp, so this
/// function itself is currently only exercised by its own tests below.
#[allow(dead_code)]
pub fn refresh_pricing_table(source: &dyn PriceSource, fallback: PricingTable) -> PricingTable {
    refresh_pricing_table_with_status(source, fallback).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::embedded_pricing_table;

    struct FakeSource {
        response: Result<String, String>,
    }

    impl PriceSource for FakeSource {
        fn fetch(&self) -> Result<String, String> {
            self.response.clone()
        }
    }

    const LITELLM_FIXTURE: &str = r#"{
        "sample_spec": {"some_field": "ignore me, not a real model"},
        "claude-sonnet-5": {
            "input_cost_per_token": 0.000003,
            "output_cost_per_token": 0.000015,
            "cache_read_input_token_cost": 0.0000003,
            "cache_creation_input_token_cost": 0.00000375
        }
    }"#;

    #[test]
    fn valid_response_replaces_table() {
        let fallback = embedded_pricing_table();
        let source = FakeSource { response: Ok(LITELLM_FIXTURE.to_string()) };
        let table = refresh_pricing_table(&source, fallback);
        assert!(table.contains_key("claude-sonnet-5"));
        assert!(!table.contains_key("sample_spec"));
    }

    #[test]
    fn invalid_json_falls_back() {
        let fallback = embedded_pricing_table();
        let source = FakeSource { response: Ok("not json".to_string()) };
        let table = refresh_pricing_table(&source, fallback.clone());
        assert_eq!(table.len(), fallback.len());
        assert!(table.contains_key("claude-sonnet-5"));
    }

    #[test]
    fn fetch_error_falls_back() {
        let fallback = embedded_pricing_table();
        let source = FakeSource { response: Err("network down".to_string()) };
        let table = refresh_pricing_table(&source, fallback.clone());
        assert_eq!(table.len(), fallback.len());
    }

    #[test]
    fn with_status_reports_success_on_valid_response() {
        let fallback = embedded_pricing_table();
        let source = FakeSource { response: Ok(LITELLM_FIXTURE.to_string()) };
        let (table, succeeded) = refresh_pricing_table_with_status(&source, fallback);
        assert!(succeeded);
        assert!(table.contains_key("claude-sonnet-5"));
    }

    #[test]
    fn with_status_reports_failure_on_fetch_error_and_invalid_json() {
        let fallback = embedded_pricing_table();
        let source = FakeSource { response: Err("network down".to_string()) };
        let (_, succeeded) = refresh_pricing_table_with_status(&source, fallback.clone());
        assert!(!succeeded);

        let source = FakeSource { response: Ok("not json".to_string()) };
        let (_, succeeded) = refresh_pricing_table_with_status(&source, fallback);
        assert!(!succeeded);
    }

    #[test]
    fn parse_litellm_table_converts_per_token_rates_to_per_million_and_skips_sample_spec() {
        let table = parse_litellm_table(LITELLM_FIXTURE).unwrap();
        assert!(!table.contains_key("sample_spec"));
        let priced = table.get("claude-sonnet-5").unwrap();
        assert!((priced.input - 3.0).abs() < 1e-9);
        assert!((priced.output - 15.0).abs() < 1e-9);
        assert!((priced.cache_read - 0.3).abs() < 1e-9);
        assert!((priced.cache_write - 3.75).abs() < 1e-9);
    }

    #[test]
    fn parse_litellm_table_returns_none_when_no_entries_have_cost_fields() {
        assert!(parse_litellm_table("{}").is_none());
        assert!(parse_litellm_table(r#"{"sample_spec": {"some_field": 1}}"#).is_none());
    }
}
