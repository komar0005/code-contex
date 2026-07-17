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
    // Two potential writers (the long-lived tray app + frequent, short-lived
    // `--statusline` invocations from phase 2) — the default rollback
    // journal can return "database is locked" under contention. WAL allows
    // one writer and concurrent readers without blocking.
    conn.pragma_update(None, "journal_mode", "WAL")?;
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
        );
        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY,
            date TEXT NOT NULL,
            agent TEXT NOT NULL,
            project TEXT,
            model TEXT,
            cost_usd REAL NOT NULL,
            lines_added INTEGER NOT NULL,
            lines_removed INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_agent_date ON sessions(agent, date);",
    )
}

/// One session's latest known totals, as fed by Claude Code's statusLine
/// hook (`crate::statusline`). Claude Code sends session-cumulative totals
/// on every invocation, not deltas, so `upsert_session` always overwrites
/// with the newest values — same "keep the last occurrence" principle as
/// `parsers/claude_code.rs` uses for JSONL message ids.
pub struct SessionUpdate<'a> {
    pub session_id: &'a str,
    pub date: &'a str,
    pub agent: Agent,
    pub project: Option<&'a str>,
    pub model: Option<&'a str>,
    pub cost_usd: f64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub duration_ms: u64,
    pub updated_at: DateTime<Utc>,
}

pub fn upsert_session(conn: &Connection, update: &SessionUpdate) {
    let _ = conn.execute(
        "INSERT INTO sessions
            (session_id, date, agent, project, model, cost_usd, lines_added, lines_removed, duration_ms, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(session_id) DO UPDATE SET
            date = excluded.date,
            agent = excluded.agent,
            project = excluded.project,
            model = excluded.model,
            cost_usd = excluded.cost_usd,
            lines_added = excluded.lines_added,
            lines_removed = excluded.lines_removed,
            duration_ms = excluded.duration_ms,
            updated_at = excluded.updated_at",
        params![
            update.session_id,
            update.date,
            agent_key(update.agent),
            update.project,
            update.model,
            update.cost_usd,
            update.lines_added as i64,
            update.lines_removed as i64,
            update.duration_ms as i64,
            update.updated_at.to_rfc3339(),
        ],
    );
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionStats {
    pub sessions: u32,
    pub lines_added: u64,
    pub lines_removed: u64,
}

/// Aggregates `sessions` rows for `agent` on local calendar day `date`.
/// Missing/corrupt table returns zeros, same tolerance as the rest of this
/// module — callers that need to distinguish "zero today" from "feature
/// never used" should check `has_any_sessions` first.
pub fn today_session_stats(conn: &Connection, agent: Agent, date: &str) -> SessionStats {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(lines_added), 0), COALESCE(SUM(lines_removed), 0)
         FROM sessions WHERE agent = ?1 AND date = ?2",
        params![agent_key(agent), date],
        |row| {
            Ok(SessionStats {
                sessions: row.get::<_, i64>(0)? as u32,
                lines_added: row.get::<_, i64>(1)? as u64,
                lines_removed: row.get::<_, i64>(2)? as u64,
            })
        },
    )
    .unwrap_or_default()
}

/// Whether `agent` has ever had a session recorded — gates whether the
/// panel shows the lines-added/removed tile at all (vs. a user who simply
/// never installed the statusLine hook).
pub fn has_any_sessions(conn: &Connection, agent: Agent) -> bool {
    conn.query_row(
        "SELECT 1 FROM sessions WHERE agent = ?1 LIMIT 1",
        params![agent_key(agent)],
        |_| Ok(()),
    )
    .is_ok()
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

    fn session_update<'a>(session_id: &'a str, lines_added: u64, updated_at: DateTime<Utc>) -> SessionUpdate<'a> {
        SessionUpdate {
            session_id,
            date: "2026-07-16",
            agent: Agent::ClaudeCode,
            project: Some("proj-a"),
            model: Some("claude-sonnet-5"),
            cost_usd: 1.5,
            lines_added,
            lines_removed: 3,
            duration_ms: 60_000,
            updated_at,
        }
    }

    #[test]
    fn has_any_sessions_false_until_first_upsert() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("history.db")).unwrap();
        assert!(!has_any_sessions(&conn, Agent::ClaudeCode));
        upsert_session(&conn, &session_update("s1", 10, Utc.with_ymd_and_hms(2026, 7, 16, 12, 0, 0).unwrap()));
        assert!(has_any_sessions(&conn, Agent::ClaudeCode));
        assert!(!has_any_sessions(&conn, Agent::OpenCode));
    }

    #[test]
    fn upsert_session_same_id_twice_keeps_latest_values() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("history.db")).unwrap();
        let t0 = Utc.with_ymd_and_hms(2026, 7, 16, 12, 0, 0).unwrap();
        upsert_session(&conn, &session_update("s1", 10, t0));
        upsert_session(&conn, &session_update("s1", 58, t0 + chrono::Duration::minutes(5)));
        let stats = today_session_stats(&conn, Agent::ClaudeCode, "2026-07-16");
        assert_eq!(stats.sessions, 1); // same session_id, not a second row
        assert_eq!(stats.lines_added, 58);
    }

    #[test]
    fn today_session_stats_sums_across_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("history.db")).unwrap();
        let t0 = Utc.with_ymd_and_hms(2026, 7, 16, 12, 0, 0).unwrap();
        upsert_session(&conn, &session_update("s1", 10, t0));
        upsert_session(&conn, &session_update("s2", 20, t0));
        let stats = today_session_stats(&conn, Agent::ClaudeCode, "2026-07-16");
        assert_eq!(stats.sessions, 2);
        assert_eq!(stats.lines_added, 30);
        assert_eq!(stats.lines_removed, 6);
    }

    #[test]
    fn today_session_stats_zero_for_other_day() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("history.db")).unwrap();
        let t0 = Utc.with_ymd_and_hms(2026, 7, 16, 12, 0, 0).unwrap();
        upsert_session(&conn, &session_update("s1", 10, t0));
        let stats = today_session_stats(&conn, Agent::ClaudeCode, "2026-07-15");
        assert_eq!(stats, SessionStats::default());
    }
}
