// src-tauri/src/history.rs
//
// App-owned daily history: one row per (local calendar day, agent) with
// already-aggregated tokens/cost, so the panel can show 30-day trends
// without re-scanning every source file on every refresh. Per
// docs/superpowers/specs/2026-07-16-usage-history-trends-design.md.
//
// Every failure here (can't open/create the db, broken schema) degrades to
// "no history" rather than breaking the app — same principle as
// claude_oauth.rs and price_fetch.rs.

use crate::model::{Agent, UsageEvent};
use crate::pricing::PricingTable;
use crate::windows;
use chrono::{DateTime, Local, Utc};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub fn agent_key(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claude_code",
        Agent::OpenCode => "opencode",
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrendPoint {
    pub date: String, // "YYYY-MM-DD", local calendar day
    pub tokens: u64,
    pub cost: f64,
}

/// Opens (creating if needed) the history database at `db_path` and applies
/// the schema. Returns `None` on any failure — callers must treat that
/// exactly like "no history yet", never crash.
pub fn open(db_path: &Path) -> Option<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let conn = Connection::open(db_path).ok()?;
    migrate(&conn).ok()?;
    Some(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS daily_usage (
            date TEXT NOT NULL,
            agent TEXT NOT NULL,
            tokens INTEGER NOT NULL,
            cost_usd REAL NOT NULL,
            project_count INTEGER NOT NULL,
            model_count INTEGER NOT NULL,
            PRIMARY KEY (date, agent)
        );
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
}

pub fn is_backfilled(conn: &Connection) -> bool {
    conn.query_row("SELECT value FROM meta WHERE key = 'backfilled_at'", [], |row| {
        row.get::<_, String>(0)
    })
    .is_ok()
}

pub fn mark_backfilled(conn: &Connection, at: DateTime<Utc>) {
    let _ = conn.execute(
        "INSERT INTO meta (key, value) VALUES ('backfilled_at', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![at.to_rfc3339()],
    );
}

/// Upserts one `(date, agent)` row. Called every refresh for "today" (the
/// only day allowed to be rewritten); backfill calls it once per past day.
pub fn upsert_day(
    conn: &Connection,
    date: &str,
    agent: Agent,
    tokens: u64,
    cost: f64,
    project_count: usize,
    model_count: usize,
) {
    let _ = conn.execute(
        "INSERT INTO daily_usage (date, agent, tokens, cost_usd, project_count, model_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(date, agent) DO UPDATE SET
            tokens = excluded.tokens,
            cost_usd = excluded.cost_usd,
            project_count = excluded.project_count,
            model_count = excluded.model_count",
        params![
            date,
            agent_key(agent),
            tokens as i64,
            cost,
            project_count as i64,
            model_count as i64
        ],
    );
}

/// Returns up to `n` days of history for `agent`, oldest first (left-to-right
/// for a sparkline). Missing/corrupt table returns an empty vec rather than
/// erroring — a sparkline just doesn't render.
pub fn read_last_n_days(conn: &Connection, agent: Agent, n: u32) -> Vec<TrendPoint> {
    let mut stmt = match conn.prepare(
        "SELECT date, tokens, cost_usd FROM daily_usage
         WHERE agent = ?1 ORDER BY date DESC LIMIT ?2",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![agent_key(agent), n], |row| {
        Ok(TrendPoint {
            date: row.get(0)?,
            tokens: row.get::<_, i64>(1)? as u64,
            cost: row.get(2)?,
        })
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    let mut points: Vec<TrendPoint> = rows.flatten().collect();
    points.reverse();
    points
}

/// One-time reconstruction of past days from already-loaded events, so
/// trends aren't empty for users who already had months of local data
/// before this feature existed. Groups by LOCAL calendar day (same rule as
/// "Hoy" elsewhere in the app), aggregates each day exactly like
/// `summary::build_summary` does for "today".
pub fn backfill(conn: &Connection, events: &[UsageEvent], agent: Agent, pricing: &PricingTable) {
    let mut by_day: HashMap<String, Vec<&UsageEvent>> = HashMap::new();
    for event in events {
        let local_date = event.timestamp.with_timezone(&Local).date_naive().to_string();
        by_day.entry(local_date).or_default().push(event);
    }
    for (date, day_events) in by_day {
        let agg = windows::aggregate(day_events.iter().copied(), pricing);
        let projects: HashSet<&str> = day_events.iter().map(|e| e.project.as_str()).collect();
        let models: HashSet<&str> = day_events.iter().map(|e| e.model.as_str()).collect();
        upsert_day(conn, &date, agent, agg.tokens, agg.cost, projects.len(), models.len());
    }
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
            input_tokens: 1_000,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            timestamp: ts,
        }
    }

    #[test]
    fn open_creates_schema_from_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("history.db")).unwrap();
        assert!(!is_backfilled(&conn));
        assert!(read_last_n_days(&conn, Agent::ClaudeCode, 30).is_empty());
    }

    #[test]
    fn upsert_same_day_twice_does_not_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("history.db")).unwrap();
        upsert_day(&conn, "2026-07-16", Agent::ClaudeCode, 100, 1.0, 1, 1);
        upsert_day(&conn, "2026-07-16", Agent::ClaudeCode, 250, 2.5, 2, 1);
        let points = read_last_n_days(&conn, Agent::ClaudeCode, 30);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].tokens, 250);
        assert!((points[0].cost - 2.5).abs() < 1e-9);
    }

    #[test]
    fn upsert_two_agents_same_day_creates_two_rows() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("history.db")).unwrap();
        upsert_day(&conn, "2026-07-16", Agent::ClaudeCode, 100, 1.0, 1, 1);
        upsert_day(&conn, "2026-07-16", Agent::OpenCode, 50, 0.5, 1, 1);
        assert_eq!(read_last_n_days(&conn, Agent::ClaudeCode, 30).len(), 1);
        assert_eq!(read_last_n_days(&conn, Agent::OpenCode, 30).len(), 1);
    }

    #[test]
    fn read_last_n_days_orders_oldest_first_and_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("history.db")).unwrap();
        for day in 1..=5 {
            upsert_day(&conn, &format!("2026-07-{day:02}"), Agent::ClaudeCode, day, 0.0, 1, 1);
        }
        let points = read_last_n_days(&conn, Agent::ClaudeCode, 3);
        let dates: Vec<&str> = points.iter().map(|p| p.date.as_str()).collect();
        assert_eq!(dates, vec!["2026-07-03", "2026-07-04", "2026-07-05"]);
    }

    #[test]
    fn backfill_creates_one_row_per_day_with_activity() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("history.db")).unwrap();
        let table = embedded_pricing_table();
        let day1 = Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap();
        let day2 = Utc.with_ymd_and_hms(2026, 7, 11, 9, 0, 0).unwrap();
        let events = vec![
            event("proj-a", "claude-sonnet-5", day1),
            event("proj-a", "claude-sonnet-5", day1),
            event("proj-b", "claude-opus-4-8", day2),
        ];
        backfill(&conn, &events, Agent::ClaudeCode, &table);
        let points = read_last_n_days(&conn, Agent::ClaudeCode, 30);
        assert_eq!(points.len(), 2);
        let d1 = points.iter().find(|p| p.date == "2026-07-10").unwrap();
        assert_eq!(d1.tokens, 2_000);
        let d2 = points.iter().find(|p| p.date == "2026-07-11").unwrap();
        assert_eq!(d2.tokens, 1_000);
    }

    #[test]
    fn mark_backfilled_is_idempotent_and_observable() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("history.db")).unwrap();
        assert!(!is_backfilled(&conn));
        let now = Utc.with_ymd_and_hms(2026, 7, 16, 0, 0, 0).unwrap();
        mark_backfilled(&conn, now);
        mark_backfilled(&conn, now);
        assert!(is_backfilled(&conn));
    }
}
