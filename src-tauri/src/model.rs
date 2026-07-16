use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Agent {
    ClaudeCode,
    OpenCode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageEvent {
    pub agent: Agent,
    pub project: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub timestamp: DateTime<Utc>,
}

impl UsageEvent {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_write_tokens + self.cache_read_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn total_tokens_sums_all_four_fields() {
        let event = UsageEvent {
            agent: Agent::ClaudeCode,
            project: "p".into(),
            model: "m".into(),
            input_tokens: 10,
            output_tokens: 20,
            cache_write_tokens: 30,
            cache_read_tokens: 40,
            timestamp: Utc.with_ymd_and_hms(2026, 7, 14, 0, 0, 0).unwrap(),
        };
        assert_eq!(event.total_tokens(), 100);
    }
}
