use crate::history::{SessionStats, TrendPoint};
use crate::limits::LimitsSnapshot;
use crate::menu_format::{format_reset_in, format_tokens, format_usd};
use crate::model::Agent;
use crate::summary::AgentSection;
use crate::windows::TokenCost;
use chrono::{DateTime, Local, TimeZone, Utc};
use serde::Serialize;
use std::collections::HashMap;

/// The JSON contract with `ui/panel.html`. Numbers arrive pre-formatted
/// (Rust owns formatting; the JS only paints). Raw exceptions: bar widths
/// (`used_percent`) and countdown anchors (`resets_at`, ISO-8601).
#[derive(Debug, Clone, Serialize)]
pub struct DashboardPayload {
    /// Local wall-clock "HH:MM" of this refresh.
    pub refreshed_at: String,
    pub agents: Vec<AgentDashboard>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentDashboard {
    pub id: &'static str,
    pub label: &'static str,
    pub limits: Option<DashboardLimits>,
    /// Heuristic 5h-block reset ("resetea en ~1h 12m"), only when there is
    /// no real limit data AND a local block is active. Never for opencode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_block: Option<String>,
    pub today: StatTile,
    pub month: StatTile,
    pub week: StatTile,
    /// Pre-rendered 30-day sparkline (inline SVG markup) from the app's own
    /// history store; empty string when there's fewer than 2 days of
    /// history to draw a line through.
    pub trend_svg: String,
    /// Lines added/removed today, from the statusLine hook (phase 2). Only
    /// ever `Some` for Claude Code, and only once the user has installed
    /// that hook and it's sent at least one session — absent otherwise
    /// (never a false "0" for someone who never installed it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines_today: Option<LinesDelta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_today: Option<u32>,
    pub by_project: Vec<BreakdownRow>,
    pub by_model: Vec<BreakdownRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinesDelta {
    pub added: u64,
    pub removed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<DashboardWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<DashboardWindow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardWindow {
    pub used_percent: f64,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatTile {
    pub tokens: String,
    pub cost: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BreakdownRow {
    pub name: String,
    pub tokens: String,
    pub cost: String,
}

pub fn build_payload(
    sections: &[AgentSection],
    trends: &HashMap<Agent, Vec<TrendPoint>>,
    session_stats: &HashMap<Agent, SessionStats>,
    now: DateTime<Utc>,
) -> DashboardPayload {
    DashboardPayload {
        refreshed_at: clock_in(now, &Local),
        agents: sections.iter().map(|s| build_agent(s, trends, session_stats, now)).collect(),
    }
}

fn build_agent(
    section: &AgentSection,
    trends: &HashMap<Agent, Vec<TrendPoint>>,
    session_stats: &HashMap<Agent, SessionStats>,
    now: DateTime<Utc>,
) -> AgentDashboard {
    let summary = &section.summary;
    let (id, label) = match summary.agent {
        Agent::ClaudeCode => ("claude_code", "Claude Code"),
        Agent::OpenCode => ("opencode", "opencode"),
    };
    let limits = section.limits.as_ref().map(build_limits);
    // The heuristic estimate only makes sense for Claude (5h blocks) and
    // would contradict real data if both were shown.
    let estimated_block = if limits.is_none() && summary.agent == Agent::ClaudeCode {
        summary
            .active_5h_block
            .as_ref()
            .map(|(_, reset_at)| format_reset_in(*reset_at, now))
    } else {
        None
    };
    let stats = session_stats.get(&summary.agent);
    AgentDashboard {
        id,
        label,
        limits,
        estimated_block,
        today: tile(&summary.today),
        month: tile(&summary.month),
        week: tile(&summary.last_7_days),
        trend_svg: trends.get(&summary.agent).map(|p| render_trend_svg(p)).unwrap_or_default(),
        lines_today: stats.map(|s| LinesDelta { added: s.lines_added, removed: s.lines_removed }),
        sessions_today: stats.map(|s| s.sessions),
        by_project: summary
            .by_project
            .iter()
            .map(|p| BreakdownRow {
                name: short_project_name(&p.project),
                tokens: format_tokens(p.tokens),
                cost: format_usd(p.cost),
            })
            .collect(),
        by_model: summary
            .by_model
            .iter()
            .map(|m| BreakdownRow {
                name: m.model.clone(),
                tokens: format_tokens(m.tokens),
                cost: format_usd(m.cost),
            })
            .collect(),
    }
}

fn build_limits(limits: &LimitsSnapshot) -> DashboardLimits {
    let window = |w: &crate::limits::RateWindow| DashboardWindow {
        used_percent: w.used_percent,
        resets_at: w.resets_at.map(|t| t.to_rfc3339()),
    };
    DashboardLimits {
        five_hour: limits.five_hour.as_ref().map(window),
        seven_day: limits.seven_day.as_ref().map(window),
    }
}

/// Claude Code "project names" are full filesystem paths — in a 380px
/// panel every row would ellipsize to the identical "/home/user/Proj…".
/// Show only the last path segment (either separator, for Windows paths).
fn short_project_name(project: &str) -> String {
    project
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(project)
        .to_string()
}

fn tile(cost: &TokenCost) -> StatTile {
    StatTile { tokens: format_tokens(cost.tokens), cost: format_usd(cost.cost) }
}

/// Renders a 30-day token trend as inline SVG (viewBox 0..100 x 0..24), so
/// the panel JS stays a dumb paint layer — same principle as every other
/// pre-formatted field in this payload. `stroke="currentColor"` picks up the
/// agent's accent color from CSS. Fewer than 2 points means there's no line
/// to draw, so callers get an empty string (no card renders).
fn render_trend_svg(points: &[TrendPoint]) -> String {
    if points.len() < 2 {
        return String::new();
    }
    let max_tokens = points.iter().map(|p| p.tokens).max().unwrap_or(0);
    if max_tokens == 0 {
        return String::new();
    }
    let step = 100.0 / (points.len() - 1) as f64;
    let coords: Vec<String> = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let x = i as f64 * step;
            let y = 23.0 - (p.tokens as f64 / max_tokens as f64) * 22.0;
            format!("{x:.1},{y:.1}")
        })
        .collect();
    format!(
        r#"<svg viewBox="0 0 100 24" preserveAspectRatio="none" class="sparkline"><polyline points="{}" fill="none" stroke="currentColor" stroke-width="2" vector-effect="non-scaling-stroke"/></svg>"#,
        coords.join(" ")
    )
}

fn clock_in<Tz: TimeZone>(t: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    t.with_timezone(tz).format("%H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{LimitsSnapshot, RateWindow};
    use crate::model::Agent;
    use crate::model::UsageEvent;
    use crate::pricing::embedded_pricing_table;
    use crate::summary::{build_summary, AgentSection};
    use chrono::{TimeZone, Utc};

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap()
    }

    fn no_trends() -> HashMap<Agent, Vec<TrendPoint>> {
        HashMap::new()
    }

    fn no_sessions() -> HashMap<Agent, SessionStats> {
        HashMap::new()
    }

    fn section(agent: Agent, limits: Option<LimitsSnapshot>) -> AgentSection {
        let event = UsageEvent {
            agent,
            project: "proj-a".into(),
            model: "claude-sonnet-5".into(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            timestamp: now(),
        };
        let summary =
            build_summary(agent, &[event], &embedded_pricing_table(), now()).unwrap();
        AgentSection { summary, limits }
    }

    #[test]
    fn builds_agent_with_limits_and_formatted_tiles() {
        let limits = LimitsSnapshot {
            five_hour: Some(RateWindow::new(
                62.0,
                Some(Utc.with_ymd_and_hms(2026, 7, 15, 18, 0, 0).unwrap()),
            )),
            seven_day: Some(RateWindow::new(34.0, None)),
        };
        let payload =
            build_payload(&[section(Agent::ClaudeCode, Some(limits))], &no_trends(), &no_sessions(), now());
        assert_eq!(payload.agents.len(), 1);
        let agent = &payload.agents[0];
        assert_eq!(agent.id, "claude_code");
        assert_eq!(agent.label, "Claude Code");
        let lim = agent.limits.as_ref().unwrap();
        assert_eq!(lim.five_hour.as_ref().unwrap().used_percent, 62.0);
        assert_eq!(
            lim.five_hour.as_ref().unwrap().resets_at.as_deref(),
            Some("2026-07-15T18:00:00+00:00")
        );
        assert!(lim.seven_day.as_ref().unwrap().resets_at.is_none());
        assert!(agent.estimated_block.is_none());
        assert_eq!(agent.today.tokens, "1.0M tok");
        assert_eq!(agent.month.tokens, "1.0M tok");
        assert_eq!(agent.week.tokens, "1.0M tok");
        assert!(agent.today.cost.starts_with('$'));
        assert_eq!(agent.by_project[0].name, "proj-a");
        assert_eq!(agent.by_model[0].name, "claude-sonnet-5");
    }

    #[test]
    fn no_limits_yields_estimated_block_from_active_5h_block() {
        let payload = build_payload(&[section(Agent::ClaudeCode, None)], &no_trends(), &no_sessions(), now());
        let agent = &payload.agents[0];
        assert!(agent.limits.is_none());
        // The fixture event is at `now`, so a 5h block is active.
        let text = agent.estimated_block.as_deref().unwrap();
        assert!(text.starts_with("resetea en ~"), "got: {text}");
    }

    #[test]
    fn opencode_has_no_limits_and_no_estimated_block() {
        let payload = build_payload(&[section(Agent::OpenCode, None)], &no_trends(), &no_sessions(), now());
        let agent = &payload.agents[0];
        assert_eq!(agent.id, "opencode");
        assert_eq!(agent.label, "opencode");
        assert!(agent.limits.is_none());
        assert!(agent.estimated_block.is_none()); // opencode never shows the Claude heuristic
    }

    #[test]
    fn empty_sections_yield_empty_agents() {
        let payload = build_payload(&[], &no_trends(), &no_sessions(), now());
        assert!(payload.agents.is_empty());
    }

    #[test]
    fn serialization_contract_field_names_are_stable() {
        let limits = LimitsSnapshot {
            five_hour: Some(RateWindow::new(62.0, None)),
            seven_day: None,
        };
        let payload =
            build_payload(&[section(Agent::ClaudeCode, Some(limits))], &no_trends(), &no_sessions(), now());
        let json = serde_json::to_value(&payload).unwrap();
        assert!(json["refreshed_at"].is_string());
        let agent = &json["agents"][0];
        assert_eq!(agent["id"], "claude_code");
        assert_eq!(agent["limits"]["five_hour"]["used_percent"], 62.0);
        assert!(agent["limits"].get("seven_day").is_none()); // absent, not null
        assert!(agent.get("estimated_block").is_none()); // absent when None
        assert!(agent["today"]["tokens"].is_string());
        assert!(agent["today"]["cost"].is_string());
        assert!(agent["trend_svg"].is_string());
        assert!(agent["by_project"][0]["name"].is_string());
        assert!(agent["by_model"][0]["tokens"].is_string());
    }

    #[test]
    fn no_trend_history_yields_empty_svg() {
        let payload = build_payload(&[section(Agent::ClaudeCode, None)], &no_trends(), &no_sessions(), now());
        assert_eq!(payload.agents[0].trend_svg, "");
    }

    #[test]
    fn single_trend_point_yields_empty_svg() {
        let mut trends = HashMap::new();
        trends.insert(
            Agent::ClaudeCode,
            vec![TrendPoint { date: "2026-07-15".into(), tokens: 100, cost: 1.0 }],
        );
        let payload = build_payload(&[section(Agent::ClaudeCode, None)], &trends, &no_sessions(), now());
        assert_eq!(payload.agents[0].trend_svg, "");
    }

    #[test]
    fn two_or_more_trend_points_render_a_polyline() {
        let mut trends = HashMap::new();
        trends.insert(
            Agent::ClaudeCode,
            vec![
                TrendPoint { date: "2026-07-14".into(), tokens: 100, cost: 1.0 },
                TrendPoint { date: "2026-07-15".into(), tokens: 400, cost: 4.0 },
            ],
        );
        let payload = build_payload(&[section(Agent::ClaudeCode, None)], &trends, &no_sessions(), now());
        let svg = &payload.agents[0].trend_svg;
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<polyline"));
    }

    #[test]
    fn no_session_data_yields_absent_lines_and_sessions() {
        let payload = build_payload(&[section(Agent::ClaudeCode, None)], &no_trends(), &no_sessions(), now());
        assert!(payload.agents[0].lines_today.is_none());
        assert!(payload.agents[0].sessions_today.is_none());
        let json = serde_json::to_value(&payload).unwrap();
        assert!(json["agents"][0].get("lines_today").is_none()); // absent, not null
    }

    #[test]
    fn session_stats_populate_lines_and_sessions_today() {
        let mut stats = HashMap::new();
        stats.insert(Agent::ClaudeCode, SessionStats { sessions: 3, lines_added: 58, lines_removed: 12 });
        let payload = build_payload(&[section(Agent::ClaudeCode, None)], &no_trends(), &stats, now());
        let agent = &payload.agents[0];
        assert_eq!(agent.sessions_today, Some(3));
        let lines = agent.lines_today.as_ref().unwrap();
        assert_eq!(lines.added, 58);
        assert_eq!(lines.removed, 12);
    }

    #[test]
    fn project_names_shorten_to_last_path_segment() {
        assert_eq!(short_project_name("/home/user/projects/ai-context"), "ai-context");
        assert_eq!(short_project_name("plain-name"), "plain-name");
        assert_eq!(short_project_name("/trailing/slash/"), "slash");
        assert_eq!(short_project_name("/"), "/");
        assert_eq!(short_project_name("C:\\Users\\user\\proyecto"), "proyecto");
    }

    #[test]
    fn clock_renders_local_hh_mm() {
        use chrono::FixedOffset;
        let t = Utc.with_ymd_and_hms(2026, 7, 15, 18, 32, 5).unwrap();
        let tz = FixedOffset::west_opt(4 * 3600).unwrap();
        assert_eq!(clock_in(t, &tz), "14:32");
    }
}
