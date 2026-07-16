// src-tauri/src/parsers/opencode.rs
use crate::model::{Agent, UsageEvent};
use chrono::{TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

/// Opens opencode's SQLite database read-only. Safe to open concurrently
/// with opencode itself: its database runs in WAL mode, which supports
/// concurrent readers. Returns `None` if the file doesn't exist (opencode
/// not installed/used on this machine) or can't be opened.
pub fn open_read_only(db_path: &Path) -> Option<Connection> {
    if !db_path.exists() {
        return None;
    }
    Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// Reads one `UsageEvent` per opencode session (session-level aggregated
/// token totals, not per-message) by joining `session` to `project` for the
/// project path. Sessions with a NULL `model` (created but never actually
/// used) are skipped. NULL token columns default to 0. Returns an empty
/// vec (never panics) if the schema doesn't match or the query fails.
pub fn load_all(conn: &Connection) -> Vec<UsageEvent> {
    let mut stmt = match conn.prepare(
        "SELECT s.time_created, s.model, \
                COALESCE(s.tokens_input, 0), COALESCE(s.tokens_output, 0), \
                COALESCE(s.tokens_cache_read, 0), COALESCE(s.tokens_cache_write, 0), \
                p.worktree \
         FROM session s JOIN project p ON s.project_id = p.id",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, String>(6)?,
        ))
    });

    let Ok(rows) = rows else {
        return Vec::new();
    };

    let mut events = Vec::new();
    for row in rows.flatten() {
        let (time_created_ms, model, input, output, cache_read, cache_write, project) = row;
        let Some(model) = model else {
            continue;
        };
        let Some(timestamp) = Utc.timestamp_millis_opt(time_created_ms).single() else {
            continue;
        };
        events.push(UsageEvent {
            agent: Agent::OpenCode,
            project,
            model,
            input_tokens: input.max(0) as u64,
            output_tokens: output.max(0) as u64,
            cache_write_tokens: cache_write.max(0) as u64,
            cache_read_tokens: cache_read.max(0) as u64,
            timestamp,
        });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an in-memory SQLite DB with just the columns `load_all`
    /// actually selects, so tests don't depend on opencode being installed
    /// or on the full real schema (which has many more columns than these).
    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT);
             CREATE TABLE session (
                 id TEXT PRIMARY KEY,
                 project_id TEXT,
                 model TEXT,
                 time_created INTEGER,
                 tokens_input INTEGER,
                 tokens_output INTEGER,
                 tokens_cache_read INTEGER,
                 tokens_cache_write INTEGER
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn loads_one_event_per_session_joined_to_project() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES ('proj1', '/home/user/project-b')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, model, time_created, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write)
             VALUES ('sess1', 'proj1', 'claude-sonnet-5', 1752529200000, 500, 120, 900, 40)",
            [],
        )
        .unwrap();

        let events = load_all(&conn);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model, "claude-sonnet-5");
        assert_eq!(events[0].project, "/home/user/project-b");
        assert_eq!(events[0].input_tokens, 500);
        assert_eq!(events[0].output_tokens, 120);
        assert_eq!(events[0].cache_read_tokens, 900);
        assert_eq!(events[0].cache_write_tokens, 40);
    }

    #[test]
    fn skips_session_with_null_model() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES ('proj1', '/home/user/project-b')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, model, time_created, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write)
             VALUES ('sess1', 'proj1', NULL, 1752529200000, 0, 0, 0, 0)",
            [],
        )
        .unwrap();

        assert!(load_all(&conn).is_empty());
    }

    #[test]
    fn defaults_null_token_columns_to_zero() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES ('proj1', '/home/user/project-b')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, project_id, model, time_created, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write)
             VALUES ('sess1', 'proj1', 'claude-sonnet-5', 1752529200000, NULL, NULL, NULL, NULL)",
            [],
        )
        .unwrap();

        let events = load_all(&conn);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].total_tokens(), 0);
    }

    #[test]
    fn open_read_only_returns_none_for_missing_file() {
        assert!(open_read_only(Path::new("/nonexistent/for/sure/opencode.db")).is_none());
    }

    #[test]
    fn open_read_only_opens_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        // Create the file first (open_read_only must not create it).
        Connection::open(&path).unwrap();
        assert!(open_read_only(&path).is_some());
    }
}
