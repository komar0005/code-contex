use crate::limits::LimitsSnapshot;
use crate::model::{Agent, UsageEvent};
use crate::pricing::{event_cost, PricingTable};
use crate::windows::{self, TokenCost};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectBreakdown {
    pub project: String,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelBreakdown {
    pub model: String,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone)]
pub struct AgentSummary {
    pub agent: Agent,
    pub today: TokenCost,
    pub month: TokenCost,
    pub active_5h_block: Option<(TokenCost, DateTime<Utc>)>,
    pub last_7_days: TokenCost,
    pub by_project: Vec<ProjectBreakdown>,
    pub by_model: Vec<ModelBreakdown>,
}

/// Everything the menu needs for one agent: local aggregates plus, when
/// available, real limit windows from the provider's API.
#[derive(Debug, Clone)]
pub struct AgentSection {
    pub summary: AgentSummary,
    pub limits: Option<LimitsSnapshot>,
}

/// Sums `unpriced_count` across all agents' current-month windows, i.e. how
/// many events this month had a model that wasn't in the pricing table (so
/// their cost couldn't be calculated).
pub fn total_unpriced_this_month<'a>(
    summaries: impl Iterator<Item = &'a AgentSummary>,
) -> u64 {
    summaries.map(|s| s.month.unpriced_count).sum()
}

/// Builds a summary for `agent` from `events` (already filtered to that
/// agent). Returns `None` when `events` is empty, per spec: an agent with no
/// local data gets no section in the UI at all.
pub fn build_summary(
    agent: Agent,
    events: &[UsageEvent],
    pricing: &PricingTable,
    now: DateTime<Utc>,
) -> Option<AgentSummary> {
    if events.is_empty() {
        return None;
    }

    let today: Vec<&UsageEvent> = events
        .iter()
        .filter(|e| windows::is_same_calendar_day(e.timestamp, now))
        .collect();
    let month: Vec<&UsageEvent> = events
        .iter()
        .filter(|e| windows::is_same_calendar_month(e.timestamp, now))
        .collect();

    let blocks = windows::compute_blocks(events);
    let active_5h_block = windows::active_block(&blocks, now)
        .map(|b| (windows::aggregate(b.events.iter(), pricing), b.end));

    let last_7 = windows::last_7_days(events, now);

    let mut by_project_map: HashMap<String, (u64, f64)> = HashMap::new();
    for event in events {
        let entry = by_project_map.entry(event.project.clone()).or_insert((0, 0.0));
        entry.0 += event.total_tokens();
        if let Some(cost) = event_cost(event, pricing) {
            entry.1 += cost;
        }
    }
    let mut by_project: Vec<ProjectBreakdown> = by_project_map
        .into_iter()
        .map(|(project, (tokens, cost))| ProjectBreakdown { project, tokens, cost })
        .collect();
    by_project.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.project.cmp(&b.project)));

    let mut by_model_map: HashMap<String, (u64, f64)> = HashMap::new();
    for event in events {
        let entry = by_model_map.entry(event.model.clone()).or_insert((0, 0.0));
        entry.0 += event.total_tokens();
        if let Some(cost) = event_cost(event, pricing) {
            entry.1 += cost;
        }
    }
    let mut by_model: Vec<ModelBreakdown> = by_model_map
        .into_iter()
        .map(|(model, (tokens, cost))| ModelBreakdown { model, tokens, cost })
        .collect();
    by_model.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.model.cmp(&b.model)));

    Some(AgentSummary {
        agent,
        today: windows::aggregate(today.into_iter(), pricing),
        month: windows::aggregate(month.into_iter(), pricing),
        active_5h_block,
        last_7_days: windows::aggregate(last_7.into_iter(), pricing),
        by_project,
        by_model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::embedded_pricing_table;
    use chrono::TimeZone;

    fn event(project: &str, model: &str, ts: DateTime<Utc>) -> UsageEvent {
        UsageEvent {
            agent: Agent::ClaudeCode,
            project: project.into(),
            model: model.into(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            timestamp: ts,
        }
    }

    #[test]
    fn empty_events_returns_none() {
        let table = embedded_pricing_table();
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        assert!(build_summary(Agent::ClaudeCode, &[], &table, now).is_none());
    }

    #[test]
    fn aggregates_today_month_and_by_project() {
        let table = embedded_pricing_table();
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let earlier_this_month = Utc.with_ymd_and_hms(2026, 7, 5, 12, 0, 0).unwrap();
        let last_month = Utc.with_ymd_and_hms(2026, 6, 15, 0, 0, 0).unwrap();

        let events = vec![
            event("proj-a", "claude-sonnet-5", now),
            event("proj-a", "claude-sonnet-5", earlier_this_month),
            event("proj-b", "claude-sonnet-5", last_month),
        ];

        let summary = build_summary(Agent::ClaudeCode, &events, &table, now).unwrap();
        assert_eq!(summary.today.tokens, 1_000_000);
        assert_eq!(summary.month.tokens, 2_000_000); // now + earlier_this_month
        assert_eq!(summary.by_project.len(), 2);
        let proj_a = summary.by_project.iter().find(|p| p.project == "proj-a").unwrap();
        assert_eq!(proj_a.tokens, 2_000_000);
    }

    #[test]
    fn active_5h_block_present_when_recent_activity_exists() {
        let table = embedded_pricing_table();
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![event("proj-a", "claude-sonnet-5", now)];
        let summary = build_summary(Agent::ClaudeCode, &events, &table, now).unwrap();
        assert!(summary.active_5h_block.is_some());
    }

    #[test]
    fn aggregates_by_model_sorted_by_tokens() {
        let table = embedded_pricing_table();
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![
            event("proj-a", "claude-sonnet-5", now),
            event("proj-a", "claude-sonnet-5", now),
            event("proj-b", "claude-opus-4-8", now),
        ];
        let summary = build_summary(Agent::ClaudeCode, &events, &table, now).unwrap();
        assert_eq!(summary.by_model.len(), 2);
        assert_eq!(summary.by_model[0].model, "claude-sonnet-5");
        assert_eq!(summary.by_model[0].tokens, 2_000_000);
        assert!(summary.by_model[0].cost > 0.0);
        assert_eq!(summary.by_model[1].model, "claude-opus-4-8");
        assert_eq!(summary.by_model[1].tokens, 1_000_000);
    }

    #[test]
    fn by_model_unknown_model_has_zero_cost() {
        let table = embedded_pricing_table();
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![event("proj-a", "unknown-model", now)];
        let summary = build_summary(Agent::ClaudeCode, &events, &table, now).unwrap();
        assert_eq!(summary.by_model[0].cost, 0.0);
        assert_eq!(summary.by_model[0].tokens, 1_000_000);
    }

    #[test]
    fn total_unpriced_this_month_over_iterator() {
        let table = embedded_pricing_table();
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![event("proj-a", "unknown-model", now)];
        let summary = build_summary(Agent::ClaudeCode, &events, &table, now).unwrap();
        assert_eq!(total_unpriced_this_month([&summary].into_iter()), 1);
        assert_eq!(total_unpriced_this_month(std::iter::empty()), 0);
    }
}
