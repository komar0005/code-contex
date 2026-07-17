// src-tauri/src/records.rs
//
// "Local contest mode": personal-record queries derived from the existing
// `daily_usage` (phase 1) and `sessions` (phase 2) tables in history.db —
// no new tables, nothing persisted here. Streaks and best-day-by-tokens
// work for any agent; best-day-by-lines only ever has data for Claude Code,
// and only once the user opted into the statusLine hook. Per
// docs/superpowers/specs/2026-07-16-local-contest-mode-design.md.

use crate::history::agent_key;
use crate::model::Agent;
use chrono::{Duration, NaiveDate};
use rusqlite::{params, Connection};

#[derive(Debug, Clone, PartialEq)]
pub struct BestDay {
    pub date: String,
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BestLinesDay {
    pub date: String,
    pub lines_added: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PersonalRecords {
    pub current_streak_days: u32,
    pub longest_streak_days: u32,
    /// `None` only if the agent has no `daily_usage` history at all — in
    /// practice this never happens for an agent that made it into the
    /// dashboard, since it requires at least one usage event.
    pub best_day: Option<BestDay>,
    /// `None` until the agent has at least one row in `sessions` — never a
    /// false zero for someone who never installed the statusLine hook.
    pub best_lines_day: Option<BestLinesDay>,
}

pub fn personal_records(conn: &Connection, agent: Agent, today: NaiveDate) -> PersonalRecords {
    let dates = active_dates(conn, agent);
    let (current_streak_days, longest_streak_days) = streaks(&dates, today);
    PersonalRecords {
        current_streak_days,
        longest_streak_days,
        best_day: best_day_by_tokens(conn, agent),
        best_lines_day: best_lines_day(conn, agent),
    }
}

/// Local calendar days with real activity (`tokens > 0`), ascending —
/// unparseable dates are dropped rather than aborting the whole
/// computation (they shouldn't exist; this app is the only writer).
fn active_dates(conn: &Connection, agent: Agent) -> Vec<NaiveDate> {
    let mut stmt = match conn
        .prepare("SELECT date FROM daily_usage WHERE agent = ?1 AND tokens > 0 ORDER BY date ASC")
    {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![agent_key(agent)], |row| row.get::<_, String>(0));
    let Ok(rows) = rows else {
        return Vec::new();
    };
    rows.flatten().filter_map(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()).collect()
}

/// Returns `(current_streak, longest_streak)`. The current streak has a
/// one-day grace: if the last active day was today OR yesterday, it still
/// counts — otherwise it would read as "broken" at 00:01 before the user
/// has done anything yet today. Two or more days since the last activity
/// means the current streak is 0, even though the longest-ever streak is
/// still reported.
fn streaks(dates: &[NaiveDate], today: NaiveDate) -> (u32, u32) {
    let Some(&last) = dates.last() else {
        return (0, 0);
    };

    let mut longest = 1u32;
    let mut run = 1u32;
    for window in dates.windows(2) {
        if window[1] == window[0] + Duration::days(1) {
            run += 1;
        } else {
            longest = longest.max(run);
            run = 1;
        }
    }
    longest = longest.max(run);

    let current = if last == today || last == today - Duration::days(1) {
        let mut streak = 1u32;
        let mut cursor = last;
        for &date in dates.iter().rev().skip(1) {
            if date == cursor - Duration::days(1) {
                streak += 1;
                cursor = date;
            } else {
                break;
            }
        }
        streak
    } else {
        0
    };

    (current, longest)
}

/// Ties (same token count) resolve to the more recent day.
fn best_day_by_tokens(conn: &Connection, agent: Agent) -> Option<BestDay> {
    conn.query_row(
        "SELECT date, tokens FROM daily_usage
         WHERE agent = ?1 AND tokens > 0
         ORDER BY tokens DESC, date DESC LIMIT 1",
        params![agent_key(agent)],
        |row| Ok(BestDay { date: row.get(0)?, tokens: row.get::<_, i64>(1)? as u64 }),
    )
    .ok()
}

/// `None` when the agent has no `sessions` rows at all (statusLine hook
/// never installed, or never sent data yet).
fn best_lines_day(conn: &Connection, agent: Agent) -> Option<BestLinesDay> {
    conn.query_row(
        "SELECT date, SUM(lines_added) FROM sessions
         WHERE agent = ?1 GROUP BY date ORDER BY SUM(lines_added) DESC, date DESC LIMIT 1",
        params![agent_key(agent)],
        |row| Ok(BestLinesDay { date: row.get(0)?, lines_added: row.get::<_, i64>(1)? as u64 }),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{self, SessionUpdate};
    use chrono::Utc;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn no_history_yields_zeroed_records() {
        let dir = tempfile::tempdir().unwrap();
        let conn = history::open(&dir.path().join("history.db")).unwrap();
        let records = personal_records(&conn, Agent::ClaudeCode, date("2026-07-16"));
        assert_eq!(records.current_streak_days, 0);
        assert_eq!(records.longest_streak_days, 0);
        assert!(records.best_day.is_none());
        assert!(records.best_lines_day.is_none());
    }

    #[test]
    fn streak_counts_consecutive_active_days_ending_today() {
        let dir = tempfile::tempdir().unwrap();
        let conn = history::open(&dir.path().join("history.db")).unwrap();
        for d in ["2026-07-12", "2026-07-13", "2026-07-14", "2026-07-15", "2026-07-16"] {
            history::upsert_day(&conn, d, Agent::ClaudeCode, 1_000, 1.0, 1, 1);
        }
        let records = personal_records(&conn, Agent::ClaudeCode, date("2026-07-16"));
        assert_eq!(records.current_streak_days, 5);
        assert_eq!(records.longest_streak_days, 5);
    }

    #[test]
    fn current_streak_has_one_day_grace_for_yesterday() {
        let dir = tempfile::tempdir().unwrap();
        let conn = history::open(&dir.path().join("history.db")).unwrap();
        history::upsert_day(&conn, "2026-07-14", Agent::ClaudeCode, 1_000, 1.0, 1, 1);
        history::upsert_day(&conn, "2026-07-15", Agent::ClaudeCode, 1_000, 1.0, 1, 1);
        // "today" is the 16th; last activity was yesterday (the 15th).
        let records = personal_records(&conn, Agent::ClaudeCode, date("2026-07-16"));
        assert_eq!(records.current_streak_days, 2);
    }

    #[test]
    fn current_streak_breaks_after_two_days_of_inactivity() {
        let dir = tempfile::tempdir().unwrap();
        let conn = history::open(&dir.path().join("history.db")).unwrap();
        history::upsert_day(&conn, "2026-07-10", Agent::ClaudeCode, 1_000, 1.0, 1, 1);
        // "today" is the 16th; last activity was the 10th — 6 days ago.
        let records = personal_records(&conn, Agent::ClaudeCode, date("2026-07-16"));
        assert_eq!(records.current_streak_days, 0);
        assert_eq!(records.longest_streak_days, 1); // longest-ever is unaffected
    }

    #[test]
    fn longest_streak_can_exceed_a_broken_current_streak() {
        let dir = tempfile::tempdir().unwrap();
        let conn = history::open(&dir.path().join("history.db")).unwrap();
        for d in ["2026-07-01", "2026-07-02", "2026-07-03", "2026-07-04", "2026-07-05"] {
            history::upsert_day(&conn, d, Agent::ClaudeCode, 1_000, 1.0, 1, 1);
        }
        // Gap, then a short current streak of 2.
        history::upsert_day(&conn, "2026-07-15", Agent::ClaudeCode, 1_000, 1.0, 1, 1);
        history::upsert_day(&conn, "2026-07-16", Agent::ClaudeCode, 1_000, 1.0, 1, 1);
        let records = personal_records(&conn, Agent::ClaudeCode, date("2026-07-16"));
        assert_eq!(records.current_streak_days, 2);
        assert_eq!(records.longest_streak_days, 5);
    }

    #[test]
    fn days_with_zero_tokens_do_not_count_as_active() {
        let dir = tempfile::tempdir().unwrap();
        let conn = history::open(&dir.path().join("history.db")).unwrap();
        history::upsert_day(&conn, "2026-07-15", Agent::ClaudeCode, 0, 0.0, 0, 0);
        history::upsert_day(&conn, "2026-07-16", Agent::ClaudeCode, 500, 0.5, 1, 1);
        let records = personal_records(&conn, Agent::ClaudeCode, date("2026-07-16"));
        assert_eq!(records.current_streak_days, 1); // the zero day doesn't extend it
    }

    #[test]
    fn best_day_picks_highest_tokens_and_breaks_ties_by_recency() {
        let dir = tempfile::tempdir().unwrap();
        let conn = history::open(&dir.path().join("history.db")).unwrap();
        history::upsert_day(&conn, "2026-07-10", Agent::ClaudeCode, 5_000_000, 10.0, 1, 1);
        history::upsert_day(&conn, "2026-07-12", Agent::ClaudeCode, 5_000_000, 10.0, 1, 1); // tie, more recent
        history::upsert_day(&conn, "2026-07-14", Agent::ClaudeCode, 1_000_000, 2.0, 1, 1);
        let records = personal_records(&conn, Agent::ClaudeCode, date("2026-07-16"));
        let best = records.best_day.unwrap();
        assert_eq!(best.date, "2026-07-12");
        assert_eq!(best.tokens, 5_000_000);
    }

    #[test]
    fn best_lines_day_absent_without_any_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let conn = history::open(&dir.path().join("history.db")).unwrap();
        history::upsert_day(&conn, "2026-07-16", Agent::ClaudeCode, 1_000, 1.0, 1, 1);
        let records = personal_records(&conn, Agent::ClaudeCode, date("2026-07-16"));
        assert!(records.best_lines_day.is_none());
    }

    #[test]
    fn best_lines_day_aggregates_multiple_sessions_on_the_same_day() {
        let dir = tempfile::tempdir().unwrap();
        let conn = history::open(&dir.path().join("history.db")).unwrap();
        let now = Utc::now();
        let session = |id: &'static str, date: &'static str, lines: u64| SessionUpdate {
            session_id: id,
            date,
            agent: Agent::ClaudeCode,
            project: None,
            model: None,
            cost_usd: 0.0,
            lines_added: lines,
            lines_removed: 0,
            duration_ms: 0,
            updated_at: now,
        };
        history::upsert_session(&conn, &session("s1", "2026-07-14", 100));
        history::upsert_session(&conn, &session("s2", "2026-07-16", 200));
        history::upsert_session(&conn, &session("s3", "2026-07-16", 220));
        let records = personal_records(&conn, Agent::ClaudeCode, date("2026-07-16"));
        let best = records.best_lines_day.unwrap();
        assert_eq!(best.date, "2026-07-16");
        assert_eq!(best.lines_added, 420);
    }
}
