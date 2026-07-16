# AI Usage Tray Widget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Tauri-based tray/menu-bar app for macOS and Linux that shows local token/cost consumption for Claude Code and opencode, computed entirely from files those tools already write to disk.

**Architecture:** A Rust backend (inside `src-tauri/`) parses Claude Code's `~/.claude/projects/**/*.jsonl` files and reads opencode's SQLite database at `~/.local/share/opencode/opencode.db` (revised 2026-07-15 from an originally assumed `storage/**` file layout — see Task 4) into a shared `UsageEvent` model, prices them against an embedded (optionally network-refreshed) pricing table, aggregates them into calendar/rolling-window summaries, and renders the result as a native tray menu that's rebuilt on a timer. A tiny HTML/JS preferences window (no framework) lets the user set personal budget thresholds. No app-owned database (opencode's own pre-existing database is read, never written, exactly like Claude Code's log files), no accounts, no telemetry.

**Tech Stack:** Tauri 2 (Rust backend + vanilla HTML/JS for the one settings window), `serde`/`serde_json`, `chrono`, `walkdir`, `reqwest` (blocking, for the optional pricing refresh), `dirs`, `rusqlite` (bundled, for reading opencode's SQLite database — added during Task 4, not anticipated in the original Task 1 dependency list). Dev/test only: `tempfile`, `filetime`.

## Global Constraints

- Platforms: **macOS and Linux only**. No Windows support in v1.
- No network calls except the optional pricing-table refresh. No accounts, no auth, no telemetry.
- No app-owned persistent database in v1. Every refresh recomputes from the source files, using an **in-memory, per-process** mtime cache only (lost on restart — that's acceptable).
- An agent's section is **omitted entirely** from the tray menu (not shown with zeros) when that agent has no local usage data.
- Budget bars (5h block, 7-day window) represent a **user-defined personal budget**, never Anthropic's real account limit — copy must never imply it's the real plan limit.
- The 5h-block and 7-day-window budget indicators apply **only to Claude Code**. opencode never shows budget bars in v1.
- "Today" / "this month" use calendar boundaries in local system time; the 5h block and 7-day window are rolling, not calendar-aligned.

---

### Task 1: Scaffold the Tauri project with a bare tray icon

**Files:**
- Create: whole project scaffold via `cargo create-tauri-app` (produces `src-tauri/`, `ui/`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, icons, etc.)
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/tauri.conf.json`
- Modify: `src-tauri/src/main.rs`

**Interfaces:**
- Produces: a running app with a tray icon whose only menu item is "Salir" (quits the app). Later tasks replace this menu's contents but reuse the same `TrayIconBuilder` wiring.

- [ ] **Step 1: Scaffold with the vanilla (no JS framework) template**

```bash
cd /path/to/ai-context
cargo install create-tauri-app --locked
cargo create-tauri-app ai-usage-tray --manager cargo --template vanilla
mv ai-usage-tray/* ai-usage-tray/.[!.]* . 2>/dev/null; rmdir ai-usage-tray
```

This produces `src-tauri/` (Rust backend) and `ui/` (or `dist/`/`src/` depending on template version — rename it to `ui/` for clarity) with a static `index.html`.

- [ ] **Step 2: Set dependencies in `src-tauri/Cargo.toml`**

Replace the `[dependencies]` and add `[dev-dependencies]` sections with:

```toml
[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
walkdir = "2"
reqwest = { version = "0.12", features = ["blocking", "json"] }
dirs = "5"

[dev-dependencies]
tempfile = "3"
filetime = "0.2"
```

- [ ] **Step 3: Configure zero default windows in `src-tauri/tauri.conf.json`**

Find the `"app"` object and set:

```json
{
  "app": {
    "windows": [],
    "security": { "csp": null }
  }
}
```

(This app has no main window — everything lives in the tray, plus one on-demand Preferences window added in Task 16.)

- [ ] **Step 4: Replace `src-tauri/src/main.rs` with a minimal tray**

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let quit_item = MenuItemBuilder::with_id("quit", "Salir").build(app)?;
            let menu = MenuBuilder::new(app).item(&quit_item).build()?;

            TrayIconBuilder::new()
                .menu(&menu)
                .on_menu_event(|app, event| {
                    if event.id() == "quit" {
                        app.exit(0);
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 5: Build and manually verify**

```bash
cd src-tauri
cargo tauri dev
```

Manual check (no automated test possible for OS tray chrome): a tray icon appears in the macOS menu bar / Linux systray; clicking it shows a menu with only "Salir"; clicking "Salir" quits the app. On Linux, if no tray icon appears at all, install `libayatana-appindicator3-1` (Debian/Ubuntu) or your distro's equivalent and retry — this is a known Tauri/Linux systray dependency, documented again in Task 17.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: scaffold Tauri tray app with quit-only menu"
```

---

### Task 2: Shared usage data model

**Files:**
- Create: `src-tauri/src/model.rs`
- Modify: `src-tauri/src/main.rs` (add `mod model;`)

**Interfaces:**
- Produces: `Agent` enum (`ClaudeCode`, `OpenCode`), `UsageEvent` struct with fields `agent`, `project: String`, `model: String`, `input_tokens: u64`, `output_tokens: u64`, `cache_write_tokens: u64`, `cache_read_tokens: u64`, `timestamp: DateTime<Utc>`, and method `total_tokens() -> u64`. Every later task (parsers, pricing, windows, summary) is built directly on this type.

- [ ] **Step 1: Write the model with an inline test**

```rust
// src-tauri/src/model.rs
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
```

- [ ] **Step 2: Register the module**

In `src-tauri/src/main.rs`, add near the top:

```rust
mod model;
```

- [ ] **Step 3: Run the test**

```bash
cd src-tauri
cargo test model::tests::total_tokens_sums_all_four_fields
```

Expected: `test model::tests::total_tokens_sums_all_four_fields ... ok`

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/model.rs src-tauri/src/main.rs
git commit -m "feat: add shared UsageEvent model"
```

---

### Task 3: Claude Code parser

**Files:**
- Create: `src-tauri/src/parsers/mod.rs`
- Create: `src-tauri/src/parsers/claude_code.rs`
- Create: `src-tauri/tests/fixtures/claude_code_sample.jsonl`
- Modify: `src-tauri/src/main.rs` (add `mod parsers;`)

**Interfaces:**
- Consumes: `model::{Agent, UsageEvent}` from Task 2.
- Produces: `parsers::claude_code::parse_jsonl_content(content: &str, fallback_project: &str) -> Vec<UsageEvent>`, `parsers::claude_code::discover_files(claude_projects_dir: &Path) -> Vec<PathBuf>`, `parsers::claude_code::folder_slug_project_name(file_path: &Path) -> String`. Task 9 (summary builder) and Task 15 (refresh loop) call these directly.

Real Claude Code session lines look like this (verified against a live `~/.claude/projects/**/*.jsonl` file):

```json
{"type":"assistant","timestamp":"2026-07-15T03:15:50.960Z","sessionId":"f43339ee-...","cwd":"/path/to/ai-context","message":{"model":"claude-sonnet-5","usage":{"input_tokens":2,"cache_creation_input_tokens":27549,"cache_read_input_tokens":19656,"output_tokens":196}}}
```

Non-assistant lines, assistant lines with `usage: null` (streaming partials), and malformed lines all appear in real files and must be skipped without aborting the rest of the file.

- [ ] **Step 1: Write the fixture file**

```
// src-tauri/tests/fixtures/claude_code_sample.jsonl
{"type":"assistant","timestamp":"2026-07-14T10:00:00.000Z","sessionId":"s1","cwd":"/home/user/project-a","message":{"model":"claude-sonnet-5","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":200,"cache_read_input_tokens":300}}}
{"type":"user","timestamp":"2026-07-14T10:00:01.000Z","message":{"role":"user","content":"hi"}}
{"type":"assistant","timestamp":"2026-07-14T10:00:02.000Z","message":{"model":"claude-sonnet-5","usage":null}}
{this is not valid json,,,
{"type":"assistant","timestamp":"2026-07-14T11:30:00.000Z","cwd":"/home/user/project-a","message":{"model":"claude-haiku-4-5-20251001","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
```

- [ ] **Step 2: Write the failing test in `claude_code.rs`**

```rust
// src-tauri/src/parsers/claude_code.rs
use crate::model::{Agent, UsageEvent};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn parse_jsonl_content(content: &str, fallback_project: &str) -> Vec<UsageEvent> {
    let mut events = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage").filter(|u| !u.is_null()) else {
            continue;
        };
        let Some(model) = message.get("model").and_then(Value::as_str) else {
            continue;
        };
        let Some(timestamp_str) = value.get("timestamp").and_then(Value::as_str) else {
            continue;
        };
        let Ok(timestamp) = DateTime::parse_from_rfc3339(timestamp_str) else {
            continue;
        };
        let project = value
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or(fallback_project)
            .to_string();

        events.push(UsageEvent {
            agent: Agent::ClaudeCode,
            project,
            model: model.to_string(),
            input_tokens: usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            output_tokens: usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            cache_write_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            timestamp: timestamp.with_timezone(&Utc),
        });
    }
    events
}

pub fn discover_files(claude_projects_dir: &Path) -> Vec<PathBuf> {
    if !claude_projects_dir.exists() {
        return Vec::new();
    }
    WalkDir::new(claude_projects_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .map(|e| e.path().to_path_buf())
        .collect()
}

pub fn folder_slug_project_name(file_path: &Path) -> String {
    file_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/claude_code_sample.jsonl"
        ))
        .unwrap()
    }

    #[test]
    fn parses_only_valid_assistant_usage_lines() {
        let events = parse_jsonl_content(&fixture(), "fallback");
        assert_eq!(events.len(), 2);

        assert_eq!(events[0].model, "claude-sonnet-5");
        assert_eq!(events[0].project, "/home/user/project-a");
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].output_tokens, 50);
        assert_eq!(events[0].cache_write_tokens, 200);
        assert_eq!(events[0].cache_read_tokens, 300);

        assert_eq!(events[1].model, "claude-haiku-4-5-20251001");
        assert_eq!(events[1].total_tokens(), 15);
    }

    #[test]
    fn discover_files_returns_empty_for_missing_dir() {
        let files = discover_files(Path::new("/nonexistent/path/for/sure"));
        assert!(files.is_empty());
    }

    #[test]
    fn folder_slug_project_name_uses_parent_dir() {
        let name = folder_slug_project_name(Path::new(
            "/home/u/.claude/projects/-home-u-work-foo/abc.jsonl",
        ));
        assert_eq!(name, "-home-u-work-foo");
    }
}
```

Create `src-tauri/src/parsers/mod.rs`:

```rust
// src-tauri/src/parsers/mod.rs
pub mod claude_code;
```

- [ ] **Step 3: Register the module and run the tests**

In `src-tauri/src/main.rs`, add:

```rust
mod parsers;
```

```bash
cd src-tauri
cargo test parsers::claude_code::tests
```

Expected: 3 tests pass (`parses_only_valid_assistant_usage_lines`, `discover_files_returns_empty_for_missing_dir`, `folder_slug_project_name_uses_parent_dir`).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/parsers src-tauri/src/main.rs src-tauri/tests/fixtures/claude_code_sample.jsonl
git commit -m "feat: parse Claude Code session jsonl files into UsageEvent"
```

---

### Task 4: opencode parser

> **Revised 2026-07-15, superseding the original file-based design below.**
> The original Step 1 required confirming opencode's real on-disk schema
> before writing code, precisely because it wasn't reliably documented and
> was known to have changed across versions. That recon was carried out
> against a real, freshly installed `opencode-ai@latest` (v1.18.1): opencode
> does **not** store sessions/messages as flat JSON files under a `storage/`
> directory. It stores everything in a **WAL-mode SQLite database** at
> `~/.local/share/opencode/opencode.db` (confirmed via `opencode db path`
> and `sqlite3 ... ".schema"`; no `storage/` directory exists anywhere on
> the test machine, and the DB's migration history shows this schema is
> long-established, not a recent change). The task below reflects that
> reality. This does **not** violate the Global Constraint against an
> "app-owned persistent database" — that constraint is about this app never
> creating *its own* database for caching/history; opencode's database is a
> pre-existing data source it already owns, read here exactly like Claude
> Code's `.jsonl` files are read in Task 3.
>
> Relevant tables (from real `.schema` output):
> - `session(id, project_id, ..., directory, path, title, ..., cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write, ..., model, time_created, ...)`
> - `project(id, worktree, vcs, name, ..., time_created, ...)` — the project's absolute path is `worktree`.
>
> Design choice: read one `UsageEvent` per **session** (using the session
> row's own aggregated token columns) rather than per message. This sidesteps
> parsing `message.data`'s internal JSON shape, which was never verified
> against real populated data (no LLM provider was configured in the sandbox
> that did the recon, so no real message could be generated) — the session
> row's aggregates are real, verified columns. The tradeoff: a session that
> used more than one model over its lifetime gets attributed entirely to
> `session.model` (whichever model the session reports), and session-level
> timestamps are coarser than per-message ones. This is acceptable because,
> per the Global Constraints, **opencode never shows the 5h-block/7-day
> budget bars that need message-level precision** — those are Claude-Code
> only. opencode only ever displays today/month totals and a per-project
> breakdown, both of which tolerate session-level granularity fine.
> `session.cost` is intentionally never read — cost is always computed
> locally from tokens × our own pricing table (Task 5), for the same reason
> stated in the original design: consistency of methodology across both
> agents, not trusting an external cost field that may use different/stale
> pricing.

**Files:**
- Create: `src-tauri/src/parsers/opencode.rs`
- Modify: `src-tauri/src/parsers/mod.rs`
- Modify: `src-tauri/Cargo.toml` (add the `rusqlite` dependency — not anticipated in Task 1's original dependency list, needed now that the real storage format is known)

**Interfaces:**
- Consumes: `model::{Agent, UsageEvent}` from Task 2.
- Produces: `parsers::opencode::open_read_only(db_path: &Path) -> Option<rusqlite::Connection>`, `parsers::opencode::load_all(conn: &rusqlite::Connection) -> Vec<UsageEvent>`. Task 9 and Task 15 call these directly — Task 15 no longer walks a directory for opencode (superseding the original Task 15 text describing a `storage/message` walk); it opens the DB path and calls `load_all` once per refresh. No per-file mtime caching is needed for opencode (a Task 10 `FileCache` is not used here) — a single SQLite query against a local WAL database is cheap enough to re-run on every refresh; Task 10's cache remains used for Claude Code only.

- [ ] **Step 1: Add the `rusqlite` dependency**

In `src-tauri/Cargo.toml`, add to `[dependencies]`:

```toml
rusqlite = { version = "0.31", features = ["bundled"] }
```

The `bundled` feature compiles SQLite in rather than linking the system library, keeping the build portable across macOS and Linux without extra system package requirements.

- [ ] **Step 2: Write the parser**

```rust
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
```

- [ ] **Step 3: Register the module and run the tests**

```rust
// src-tauri/src/parsers/mod.rs
pub mod claude_code;
pub mod opencode;
```

```bash
cd src-tauri
cargo test parsers::opencode::tests
```

Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/parsers src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat: read opencode usage from its SQLite database, one event per session"
```

**If you have access to a real populated `opencode.db`** (a machine where `opencode run` has actually completed at least one exchange with a configured provider), it's worth a quick sanity check before moving on: `sqlite3 ~/.local/share/opencode/opencode.db "SELECT time_created, model, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write FROM session LIMIT 3;"` and confirm the column values look like real usage (non-null model, plausible token counts, plausible unix-ms timestamp). If anything looks structurally different from what Step 2 assumes, stop and flag it rather than proceeding — but do not block on this if no populated database is available in your environment; the schema itself (column names and types) was already confirmed against a real installation.

---

### Task 5: Pricing table and cost calculation

**Files:**
- Create: `src-tauri/src/pricing.rs`
- Create: `src-tauri/src/pricing_data.json`
- Modify: `src-tauri/src/main.rs` (add `mod pricing;`)

**Interfaces:**
- Consumes: `model::UsageEvent` from Task 2.
- Produces: `pricing::PricingTable` (type alias `HashMap<String, ModelPricing>`), `pricing::embedded_pricing_table() -> PricingTable`, `pricing::event_cost(event: &UsageEvent, table: &PricingTable) -> Option<f64>`. Task 6 (`windows::aggregate`), Task 9 (summary builder), and Task 12 (network refresh) all consume these.

- [ ] **Step 1: Write the static pricing table**

```json
// src-tauri/src/pricing_data.json
{
  "claude-sonnet-5": { "input": 3.0, "output": 15.0, "cache_write": 3.75, "cache_read": 0.3 },
  "claude-opus-4-8": { "input": 15.0, "output": 75.0, "cache_write": 18.75, "cache_read": 1.5 },
  "claude-haiku-4-5-20251001": { "input": 1.0, "output": 5.0, "cache_write": 1.25, "cache_read": 0.1 }
}
```

- [ ] **Step 2: Write `pricing.rs` with tests**

```rust
// src-tauri/src/pricing.rs
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
```

- [ ] **Step 3: Register the module and run tests**

```rust
mod pricing;
```

```bash
cd src-tauri
cargo test pricing::tests
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/pricing.rs src-tauri/src/pricing_data.json src-tauri/src/main.rs
git commit -m "feat: embed pricing table and compute per-event USD cost"
```

---

### Task 6: Calendar aggregation (today/month) and generic aggregate()

**Files:**
- Create: `src-tauri/src/windows.rs`
- Modify: `src-tauri/src/main.rs` (add `mod windows;`)

**Interfaces:**
- Consumes: `model::UsageEvent`, `pricing::{PricingTable, event_cost}` from Tasks 2 and 5.
- Produces: `windows::TokenCost { tokens: u64, cost: f64, unpriced_count: u64 }`, `windows::aggregate<'a>(events: impl Iterator<Item = &'a UsageEvent>, pricing: &PricingTable) -> TokenCost`, `windows::is_same_calendar_day(a, b) -> bool`, `windows::is_same_calendar_month(a, b) -> bool`. Tasks 7, 8, and 9 build directly on top of this file (all three land in `windows.rs`).

- [ ] **Step 1: Write `TokenCost` and `aggregate` with tests**

```rust
// src-tauri/src/windows.rs
use crate::model::UsageEvent;
use crate::pricing::{event_cost, PricingTable};
use chrono::{DateTime, Datelike, Utc};

#[derive(Debug, Clone, PartialEq)]
pub struct TokenCost {
    pub tokens: u64,
    pub cost: f64,
    pub unpriced_count: u64,
}

pub fn aggregate<'a>(
    events: impl Iterator<Item = &'a UsageEvent>,
    pricing: &PricingTable,
) -> TokenCost {
    let mut tokens = 0u64;
    let mut cost = 0.0;
    let mut unpriced_count = 0u64;
    for event in events {
        tokens += event.total_tokens();
        match event_cost(event, pricing) {
            Some(c) => cost += c,
            None => unpriced_count += 1,
        }
    }
    TokenCost { tokens, cost, unpriced_count }
}

pub fn is_same_calendar_day(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    a.year() == b.year() && a.ordinal() == b.ordinal()
}

pub fn is_same_calendar_month(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    a.year() == b.year() && a.month() == b.month()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Agent;
    use crate::pricing::embedded_pricing_table;
    use chrono::TimeZone;

    fn event(model: &str, ts: DateTime<Utc>) -> UsageEvent {
        UsageEvent {
            agent: Agent::ClaudeCode,
            project: "p".into(),
            model: model.into(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            timestamp: ts,
        }
    }

    #[test]
    fn aggregate_sums_tokens_and_tracks_unpriced() {
        let table = embedded_pricing_table();
        let t = Utc.with_ymd_and_hms(2026, 7, 14, 0, 0, 0).unwrap();
        let events = vec![event("claude-sonnet-5", t), event("unknown-model", t)];
        let result = aggregate(events.iter(), &table);
        assert_eq!(result.tokens, 2_000_000);
        assert_eq!(result.unpriced_count, 1);
        assert!((result.cost - 3.0).abs() < 1e-9); // only the priced event counts
    }

    #[test]
    fn same_calendar_day_true_for_same_date_different_time() {
        let a = Utc.with_ymd_and_hms(2026, 7, 14, 1, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 14, 23, 59, 0).unwrap();
        assert!(is_same_calendar_day(a, b));
    }

    #[test]
    fn same_calendar_day_false_across_midnight() {
        let a = Utc.with_ymd_and_hms(2026, 7, 14, 23, 59, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 1).unwrap();
        assert!(!is_same_calendar_day(a, b));
    }

    #[test]
    fn same_calendar_month_ignores_day() {
        let a = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 31, 23, 0, 0).unwrap();
        assert!(is_same_calendar_month(a, b));
    }
}
```

- [ ] **Step 2: Register the module and run tests**

```rust
mod windows;
```

```bash
cd src-tauri
cargo test windows::tests
```

Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/windows.rs src-tauri/src/main.rs
git commit -m "feat: add token/cost aggregation and calendar-day/month helpers"
```

---

### Task 7: 5-hour rolling block computation

**Files:**
- Modify: `src-tauri/src/windows.rs`

**Interfaces:**
- Consumes: `model::UsageEvent` from Task 2; builds on `windows.rs` from Task 6.
- Produces: `windows::Block { start: DateTime<Utc>, end: DateTime<Utc>, events: Vec<UsageEvent> }`, `windows::compute_blocks(events: &[UsageEvent]) -> Vec<Block>`, `windows::active_block<'a>(blocks: &'a [Block], now: DateTime<Utc>) -> Option<&'a Block>`. Task 9's summary builder calls both.

- [ ] **Step 1: Append the block logic and tests to `windows.rs`**

Add below the existing content in `src-tauri/src/windows.rs`:

```rust
use chrono::Duration;

const BLOCK_DURATION_HOURS: i64 = 5;

#[derive(Debug, Clone)]
pub struct Block {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub events: Vec<UsageEvent>,
}

/// Groups `events` into rolling 5h blocks: a new block starts at the first
/// event overall, or at any event whose timestamp is >= 5h after the start
/// of the current block. Later events within that 5h window join the same
/// block even if there's a smaller gap inside it — this mirrors how Claude's
/// plan session windows behave (fixed-length from first message, not
/// extended by later activity).
pub fn compute_blocks(events: &[UsageEvent]) -> Vec<Block> {
    let mut sorted: Vec<&UsageEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.timestamp);
    let block_len = Duration::hours(BLOCK_DURATION_HOURS);
    let mut blocks: Vec<Block> = Vec::new();
    for event in sorted {
        let needs_new_block = match blocks.last() {
            None => true,
            Some(b) => event.timestamp >= b.start + block_len,
        };
        if needs_new_block {
            blocks.push(Block {
                start: event.timestamp,
                end: event.timestamp + block_len,
                events: Vec::new(),
            });
        }
        blocks.last_mut().unwrap().events.push(event.clone());
    }
    blocks
}

/// Returns the block covering `now` (`block.start <= now < block.end`), if any.
pub fn active_block<'a>(blocks: &'a [Block], now: DateTime<Utc>) -> Option<&'a Block> {
    blocks.iter().find(|b| b.start <= now && now < b.end)
}

#[cfg(test)]
mod block_tests {
    use super::*;
    use crate::model::Agent;
    use chrono::TimeZone;

    fn event_at(ts: DateTime<Utc>) -> UsageEvent {
        UsageEvent {
            agent: Agent::ClaudeCode,
            project: "p".into(),
            model: "claude-sonnet-5".into(),
            input_tokens: 1,
            output_tokens: 1,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            timestamp: ts,
        }
    }

    #[test]
    fn events_within_5h_of_block_start_join_same_block() {
        let t0 = Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap();
        let events = vec![
            event_at(t0),
            event_at(t0 + Duration::hours(1)),
            event_at(t0 + Duration::minutes(4 * 60 + 59)),
        ];
        let blocks = compute_blocks(&events);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].events.len(), 3);
        assert_eq!(blocks[0].start, t0);
        assert_eq!(blocks[0].end, t0 + Duration::hours(5));
    }

    #[test]
    fn event_at_exactly_5h_starts_a_new_block() {
        let t0 = Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap();
        let events = vec![event_at(t0), event_at(t0 + Duration::hours(5))];
        let blocks = compute_blocks(&events);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].start, t0 + Duration::hours(5));
    }

    #[test]
    fn active_block_finds_block_covering_now() {
        let t0 = Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap();
        let events = vec![event_at(t0)];
        let blocks = compute_blocks(&events);
        let now = t0 + Duration::hours(2);
        assert!(active_block(&blocks, now).is_some());
    }

    #[test]
    fn active_block_none_when_last_activity_over_5h_ago() {
        let t0 = Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap();
        let events = vec![event_at(t0)];
        let blocks = compute_blocks(&events);
        let now = t0 + Duration::hours(6);
        assert!(active_block(&blocks, now).is_none());
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cd src-tauri
cargo test windows::block_tests
```

Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/windows.rs
git commit -m "feat: compute rolling 5-hour usage blocks"
```

---

### Task 8: 7-day rolling window

**Files:**
- Modify: `src-tauri/src/windows.rs`

**Interfaces:**
- Consumes: `model::UsageEvent`; builds on `windows.rs` from Tasks 6-7.
- Produces: `windows::last_7_days<'a>(events: &'a [UsageEvent], now: DateTime<Utc>) -> Vec<&'a UsageEvent>`. Task 9's summary builder calls this.

- [ ] **Step 1: Append the function and tests**

```rust
// append to src-tauri/src/windows.rs

/// Returns references to every event with `now - 7 days <= timestamp <= now`.
pub fn last_7_days<'a>(events: &'a [UsageEvent], now: DateTime<Utc>) -> Vec<&'a UsageEvent> {
    let cutoff = now - Duration::days(7);
    events
        .iter()
        .filter(|e| e.timestamp >= cutoff && e.timestamp <= now)
        .collect()
}

#[cfg(test)]
mod seven_day_tests {
    use super::*;
    use crate::model::Agent;
    use chrono::TimeZone;

    fn event_at(ts: DateTime<Utc>) -> UsageEvent {
        UsageEvent {
            agent: Agent::ClaudeCode,
            project: "p".into(),
            model: "claude-sonnet-5".into(),
            input_tokens: 1,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            timestamp: ts,
        }
    }

    #[test]
    fn includes_events_within_window_excludes_older() {
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![
            event_at(now - Duration::days(1)),
            event_at(now - Duration::days(6) - Duration::hours(23)),
            event_at(now - Duration::days(8)),
        ];
        let result = last_7_days(&events, now);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn boundary_at_exactly_7_days_is_included() {
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![event_at(now - Duration::days(7))];
        assert_eq!(last_7_days(&events, now).len(), 1);
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cd src-tauri
cargo test windows::seven_day_tests
```

Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/windows.rs
git commit -m "feat: add 7-day rolling window aggregation"
```

---

### Task 9: Per-agent summary builder

**Files:**
- Create: `src-tauri/src/summary.rs`
- Modify: `src-tauri/src/main.rs` (add `mod summary;`)

**Interfaces:**
- Consumes: `model::{Agent, UsageEvent}`, `pricing::{PricingTable, event_cost}`, `windows::{TokenCost, aggregate, is_same_calendar_day, is_same_calendar_month, compute_blocks, active_block, last_7_days}` from Tasks 2, 5, 6, 7, 8.
- Produces: `summary::ProjectBreakdown { project: String, tokens: u64, cost: f64 }`, `summary::AgentSummary { agent, today: TokenCost, month: TokenCost, active_5h_block: Option<(TokenCost, DateTime<Utc>)>, last_7_days: TokenCost, by_project: Vec<ProjectBreakdown> }`, `summary::build_summary(agent: Agent, events: &[UsageEvent], pricing: &PricingTable, now: DateTime<Utc>) -> Option<AgentSummary>`. Task 15's refresh loop calls `build_summary` once per agent and passes the result to Task 14's menu builder.

- [ ] **Step 1: Write `summary.rs` with tests**

```rust
// src-tauri/src/summary.rs
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

#[derive(Debug, Clone)]
pub struct AgentSummary {
    pub agent: Agent,
    pub today: TokenCost,
    pub month: TokenCost,
    pub active_5h_block: Option<(TokenCost, DateTime<Utc>)>,
    pub last_7_days: TokenCost,
    pub by_project: Vec<ProjectBreakdown>,
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
    by_project.sort_by(|a, b| b.tokens.cmp(&a.tokens));

    Some(AgentSummary {
        agent,
        today: windows::aggregate(today.into_iter(), pricing),
        month: windows::aggregate(month.into_iter(), pricing),
        active_5h_block,
        last_7_days: windows::aggregate(last_7.into_iter(), pricing),
        by_project,
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
        let earlier_this_month = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
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
}
```

- [ ] **Step 2: Register the module and run tests**

```rust
mod summary;
```

```bash
cd src-tauri
cargo test summary::tests
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/summary.rs src-tauri/src/main.rs
git commit -m "feat: build per-agent usage summaries from raw events"
```

---

### Task 10: In-memory mtime-based file cache

**Files:**
- Create: `src-tauri/src/cache.rs`
- Modify: `src-tauri/src/main.rs` (add `mod cache;`)

**Interfaces:**
- Produces: `cache::FileCache<T: Clone>::new() -> Self`, `.get_or_parse(&mut self, path: &Path, parse: impl FnOnce(&str) -> Vec<T>) -> Vec<T>`. Task 15's refresh loop holds one `FileCache<UsageEvent>` per agent inside `AppState`.

- [ ] **Step 1: Write the cache with tests**

```rust
// src-tauri/src/cache.rs
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct FileCache<T> {
    entries: HashMap<PathBuf, (SystemTime, Vec<T>)>,
}

impl<T: Clone> FileCache<T> {
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// Returns cached parsed items for `path` if its mtime hasn't changed
    /// since the last call; otherwise reads the file, calls `parse` on its
    /// content, caches the result, and returns it.
    pub fn get_or_parse(&mut self, path: &Path, parse: impl FnOnce(&str) -> Vec<T>) -> Vec<T> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some(mtime) = mtime {
            if let Some((cached_mtime, cached)) = self.entries.get(path) {
                if mtime == *cached_mtime {
                    return cached.clone();
                }
            }
        }
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let parsed = parse(&content);
        if let Some(mtime) = mtime {
            self.entries.insert(path.to_path_buf(), (mtime, parsed.clone()));
        }
        parsed
    }
}

impl<T: Clone> Default for FileCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Write;

    #[test]
    fn reparses_only_when_mtime_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "v1").unwrap();

        let calls = Cell::new(0);
        let mut cache: FileCache<String> = FileCache::new();

        let result1 = cache.get_or_parse(&path, |content| {
            calls.set(calls.get() + 1);
            vec![content.to_string()]
        });
        assert_eq!(result1, vec!["v1".to_string()]);
        assert_eq!(calls.get(), 1);

        // Same content, same mtime -> no reparse.
        let result2 = cache.get_or_parse(&path, |content| {
            calls.set(calls.get() + 1);
            vec![content.to_string()]
        });
        assert_eq!(result2, vec!["v1".to_string()]);
        assert_eq!(calls.get(), 1);

        // Change content and bump mtime explicitly (avoids filesystem mtime
        // resolution flakiness).
        {
            let mut f = std::fs::OpenOptions::new().write(true).truncate(true).open(&path).unwrap();
            f.write_all(b"v2").unwrap();
        }
        let new_mtime = filetime::FileTime::from_unix_time(
            filetime::FileTime::now().unix_seconds() + 5,
            0,
        );
        filetime::set_file_mtime(&path, new_mtime).unwrap();

        let result3 = cache.get_or_parse(&path, |content| {
            calls.set(calls.get() + 1);
            vec![content.to_string()]
        });
        assert_eq!(result3, vec!["v2".to_string()]);
        assert_eq!(calls.get(), 2);
    }
}
```

- [ ] **Step 2: Register the module and run the test**

```rust
mod cache;
```

```bash
cd src-tauri
cargo test cache::tests
```

Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/cache.rs src-tauri/src/main.rs
git commit -m "feat: add mtime-based in-memory file parse cache"
```

---

### Task 11: Preferences model and persistence

**Files:**
- Create: `src-tauri/src/preferences.rs`
- Modify: `src-tauri/src/main.rs` (add `mod preferences;`)

**Interfaces:**
- Produces: `preferences::Preferences { budget_5h_usd: f64, budget_7d_usd: f64, budget_monthly_usd: f64, refresh_interval_secs: u64, network_pricing_refresh_enabled: bool }` (with `Default`), `preferences::load(config_dir: &Path) -> Preferences`, `preferences::save(config_dir: &Path, prefs: &Preferences) -> std::io::Result<()>`. Task 14 (menu budget bars), Task 15 (refresh loop interval + network toggle), and Task 16 (preferences window commands) all consume this.

- [ ] **Step 1: Write `preferences.rs` with tests**

```rust
// src-tauri/src/preferences.rs
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preferences {
    pub budget_5h_usd: f64,
    pub budget_7d_usd: f64,
    pub budget_monthly_usd: f64,
    pub refresh_interval_secs: u64,
    pub network_pricing_refresh_enabled: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            budget_5h_usd: 10.0,
            budget_7d_usd: 50.0,
            budget_monthly_usd: 150.0,
            refresh_interval_secs: 60,
            network_pricing_refresh_enabled: true,
        }
    }
}

pub fn preferences_path(config_dir: &Path) -> PathBuf {
    config_dir.join("preferences.json")
}

pub fn load(config_dir: &Path) -> Preferences {
    std::fs::read_to_string(preferences_path(config_dir))
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

pub fn save(config_dir: &Path, prefs: &Preferences) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let content = serde_json::to_string_pretty(prefs).expect("Preferences always serializes");
    std::fs::write(preferences_path(config_dir), content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_default_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = load(dir.path());
        assert_eq!(prefs, Preferences::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = Preferences {
            budget_5h_usd: 25.0,
            budget_7d_usd: 100.0,
            budget_monthly_usd: 300.0,
            refresh_interval_secs: 30,
            network_pricing_refresh_enabled: false,
        };
        save(dir.path(), &prefs).unwrap();
        let loaded = load(dir.path());
        assert_eq!(loaded, prefs);
    }
}
```

- [ ] **Step 2: Register the module and run tests**

```rust
mod preferences;
```

```bash
cd src-tauri
cargo test preferences::tests
```

Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/preferences.rs src-tauri/src/main.rs
git commit -m "feat: add configurable budget preferences with JSON persistence"
```

---

### Task 12: Network pricing refresh with offline fallback

**Files:**
- Create: `src-tauri/src/price_fetch.rs`
- Modify: `src-tauri/src/main.rs` (add `mod price_fetch;`)

**Interfaces:**
- Consumes: `pricing::PricingTable` from Task 5.
- Produces: `price_fetch::PriceSource` trait (`fn fetch(&self) -> Result<String, String>`), `price_fetch::HttpPriceSource { url: String }` (implements `PriceSource` via blocking `reqwest`), `price_fetch::refresh_pricing_table(source: &dyn PriceSource, fallback: PricingTable) -> PricingTable`. Task 15's refresh loop calls `refresh_pricing_table` inside a `spawn_blocking` when `preferences.network_pricing_refresh_enabled` is true.

- [ ] **Step 1: Write `price_fetch.rs` with tests using a fake source**

```rust
// src-tauri/src/price_fetch.rs
use crate::pricing::PricingTable;

pub trait PriceSource {
    fn fetch(&self) -> Result<String, String>;
}

pub struct HttpPriceSource {
    pub url: String,
}

impl PriceSource for HttpPriceSource {
    fn fetch(&self) -> Result<String, String> {
        reqwest::blocking::get(&self.url)
            .map_err(|e| e.to_string())?
            .text()
            .map_err(|e| e.to_string())
    }
}

/// Tries to fetch and parse an updated pricing table from `source`. On any
/// failure (network error or invalid JSON), logs the reason to stderr and
/// returns `fallback` unchanged.
pub fn refresh_pricing_table(source: &dyn PriceSource, fallback: PricingTable) -> PricingTable {
    match source.fetch() {
        Ok(body) => match serde_json::from_str::<PricingTable>(&body) {
            Ok(table) => table,
            Err(e) => {
                eprintln!("pricing refresh: invalid JSON from source: {e}");
                fallback
            }
        },
        Err(e) => {
            eprintln!("pricing refresh: fetch failed: {e}");
            fallback
        }
    }
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

    #[test]
    fn valid_response_replaces_table() {
        let fallback = embedded_pricing_table();
        let source = FakeSource {
            response: Ok(r#"{"new-model":{"input":1.0,"output":2.0,"cache_write":0.5,"cache_read":0.1}}"#.to_string()),
        };
        let table = refresh_pricing_table(&source, fallback);
        assert!(table.contains_key("new-model"));
        assert!(!table.contains_key("claude-sonnet-5"));
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
}
```

- [ ] **Step 2: Register the module and run tests**

```rust
mod price_fetch;
```

```bash
cd src-tauri
cargo test price_fetch::tests
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/price_fetch.rs src-tauri/src/main.rs
git commit -m "feat: add pricing table network refresh with offline fallback"
```

---

### Task 13: Pure menu-text formatting functions

**Files:**
- Create: `src-tauri/src/menu_format.rs`
- Modify: `src-tauri/src/main.rs` (add `mod menu_format;`)

**Interfaces:**
- Consumes: `summary::AgentSummary`, `windows::TokenCost` from Task 9.
- Produces: `menu_format::format_tokens(u64) -> String`, `menu_format::format_usd(f64) -> String`, `menu_format::format_budget_line(label: &str, spent: f64, budget: f64) -> String`, `menu_format::format_reset_in(reset_at: DateTime<Utc>, now: DateTime<Utc>) -> String`, `menu_format::format_refreshed_at(last_refresh: DateTime<Utc>, now: DateTime<Utc>) -> String`, `menu_format::EMPTY_STATE_MESSAGE: &str`. Task 14's menu builder calls all of these directly to produce the exact strings shown in the tray dropdown.

- [ ] **Step 1: Write the formatting functions with tests**

```rust
// src-tauri/src/menu_format.rs
use chrono::{DateTime, Utc};

pub const EMPTY_STATE_MESSAGE: &str = "No se detectó actividad de agentes IA en este equipo";

pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M tok", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K tok", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens} tok")
    }
}

pub fn format_usd(amount: f64) -> String {
    format!("${amount:.2}")
}

/// Renders a 10-segment progress bar against a user-defined personal budget
/// (never against Anthropic's real account limit — that value isn't knowable
/// locally). `pct` is clamped to [0, 100] for the bar; the label still shows
/// exact spent/budget even past 100%.
pub fn format_budget_line(label: &str, spent: f64, budget: f64) -> String {
    let pct = if budget > 0.0 { (spent / budget * 100.0).clamp(0.0, 100.0) } else { 0.0 };
    let filled = (pct / 10.0).round() as usize;
    let bar: String = "█".repeat(filled) + &"░".repeat(10 - filled);
    format!("{label}  [{bar}] {}/{}", format_usd(spent), format_usd(budget))
}

pub fn format_reset_in(reset_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let remaining = reset_at - now;
    let total_minutes = remaining.num_minutes().max(0);
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if hours > 0 {
        format!("resetea en ~{hours}h {minutes}m")
    } else {
        format!("resetea en ~{minutes}m")
    }
}

pub fn format_refreshed_at(last_refresh: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - last_refresh).num_seconds().max(0);
    if secs < 60 {
        format!("Refrescado hace {secs}s")
    } else {
        format!("Refrescado hace {}min", secs / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    #[test]
    fn format_tokens_boundaries() {
        assert_eq!(format_tokens(999), "999 tok");
        assert_eq!(format_tokens(1_000), "1.0K tok");
        assert_eq!(format_tokens(1_500_000), "1.5M tok");
    }

    #[test]
    fn format_usd_rounds_to_two_decimals() {
        assert_eq!(format_usd(0.385), "$0.39");
        assert_eq!(format_usd(9.8), "$9.80");
    }

    #[test]
    fn format_budget_line_at_zero_fifty_and_over_budget() {
        assert_eq!(format_budget_line("5h", 0.0, 10.0), "5h  [░░░░░░░░░░] $0.00/$10.00");
        assert_eq!(format_budget_line("5h", 5.0, 10.0), "5h  [█████░░░░░] $5.00/$10.00");
        assert_eq!(format_budget_line("5h", 15.0, 10.0), "5h  [██████████] $15.00/$10.00");
    }

    #[test]
    fn format_reset_in_hours_and_minutes() {
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let reset_at = now + Duration::minutes(72);
        assert_eq!(format_reset_in(reset_at, now), "resetea en ~1h 12m");
        let reset_soon = now + Duration::minutes(9);
        assert_eq!(format_reset_in(reset_soon, now), "resetea en ~9m");
    }

    #[test]
    fn format_refreshed_at_seconds_and_minutes() {
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 30).unwrap();
        let recent = now - Duration::seconds(12);
        assert_eq!(format_refreshed_at(recent, now), "Refrescado hace 12s");
        let older = now - Duration::seconds(125);
        assert_eq!(format_refreshed_at(older, now), "Refrescado hace 2min");
    }
}
```

- [ ] **Step 2: Register the module and run tests**

```rust
mod menu_format;
```

```bash
cd src-tauri
cargo test menu_format::tests
```

Expected: 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/menu_format.rs src-tauri/src/main.rs
git commit -m "feat: add pure formatting functions for the tray menu text"
```

---

### Task 14: Native tray menu construction

**Files:**
- Create: `src-tauri/src/tray.rs`
- Modify: `src-tauri/src/main.rs`

**Interfaces:**
- Consumes: `summary::AgentSummary`, `preferences::Preferences`, all of `menu_format`, from Tasks 9, 11, 13.
- Produces: `tray::build_menu(app: &AppHandle, claude: Option<&AgentSummary>, opencode: Option<&AgentSummary>, prefs: &Preferences, last_refresh: DateTime<Utc>, now: DateTime<Utc>) -> tauri::Result<tauri::menu::Menu<tauri::Wry>>`. Task 15's refresh loop calls this every cycle and swaps it onto the tray via `TrayIcon::set_menu`.

This task is inherently an OS-integration task — there is no headless way to assert "the tray shows the right text" in CI. Per the design spec's own testing section, verification here is manual. The code itself must still be complete and correct; keep the pure-formatting logic (already tested in Task 13) doing all the string decisions, so this file is mostly wiring.

- [ ] **Step 1: Write `tray.rs`**

```rust
// src-tauri/src/tray.rs
use crate::menu_format::{
    format_budget_line, format_refreshed_at, format_reset_in, format_tokens, format_usd,
    EMPTY_STATE_MESSAGE,
};
use crate::model::Agent;
use crate::preferences::Preferences;
use crate::summary::AgentSummary;
use chrono::{DateTime, Utc};
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Wry};

fn agent_label(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "Claude Code",
        Agent::OpenCode => "opencode",
    }
}

fn append_agent_section(
    app: &AppHandle,
    builder: MenuBuilder<Wry, AppHandle>,
    summary: &AgentSummary,
    prefs: &Preferences,
    now: DateTime<Utc>,
) -> tauri::Result<MenuBuilder<Wry, AppHandle>> {
    let mut builder = builder;
    let header = MenuItemBuilder::new(agent_label(summary.agent)).enabled(false).build(app)?;
    builder = builder.item(&header);

    let today = MenuItemBuilder::new(format!(
        "Hoy       {}    {}",
        format_tokens(summary.today.tokens),
        format_usd(summary.today.cost)
    ))
    .enabled(false)
    .build(app)?;
    builder = builder.item(&today);

    let month = MenuItemBuilder::new(format!(
        "Mes       {}    {}",
        format_tokens(summary.month.tokens),
        format_usd(summary.month.cost)
    ))
    .enabled(false)
    .build(app)?;
    builder = builder.item(&month);

    if summary.agent == Agent::ClaudeCode {
        if let Some((block_cost, reset_at)) = &summary.active_5h_block {
            let block_line = MenuItemBuilder::new(format_budget_line(
                "Bloque 5h",
                block_cost.cost,
                prefs.budget_5h_usd,
            ))
            .enabled(false)
            .build(app)?;
            builder = builder.item(&block_line);

            let reset_line = MenuItemBuilder::new(format!("   {}", format_reset_in(*reset_at, now)))
                .enabled(false)
                .build(app)?;
            builder = builder.item(&reset_line);
        }

        let week_line = MenuItemBuilder::new(format_budget_line(
            "7 días",
            summary.last_7_days.cost,
            prefs.budget_7d_usd,
        ))
        .enabled(false)
        .build(app)?;
        builder = builder.item(&week_line);
    }

    let mut projects_submenu = SubmenuBuilder::new(app, "Ver por proyecto");
    for project in &summary.by_project {
        let item = MenuItemBuilder::new(format!(
            "{}   {}   {}",
            project.project,
            format_tokens(project.tokens),
            format_usd(project.cost)
        ))
        .enabled(false)
        .build(app)?;
        projects_submenu = projects_submenu.item(&item);
    }
    builder = builder.item(&projects_submenu.build()?);
    builder = builder.separator();

    Ok(builder)
}

pub fn build_menu(
    app: &AppHandle,
    claude: Option<&AgentSummary>,
    opencode: Option<&AgentSummary>,
    prefs: &Preferences,
    last_refresh: DateTime<Utc>,
    now: DateTime<Utc>,
) -> tauri::Result<Menu<Wry>> {
    let mut builder = MenuBuilder::new(app);

    if claude.is_none() && opencode.is_none() {
        let empty = MenuItemBuilder::new(EMPTY_STATE_MESSAGE).enabled(false).build(app)?;
        builder = builder.item(&empty).separator();
    } else {
        if let Some(summary) = claude {
            builder = append_agent_section(app, builder, summary, prefs, now)?;
        }
        if let Some(summary) = opencode {
            builder = append_agent_section(app, builder, summary, prefs, now)?;
        }
    }

    let refreshed = MenuItemBuilder::new(format_refreshed_at(last_refresh, now))
        .enabled(false)
        .build(app)?;
    let preferences_item = MenuItemBuilder::with_id("preferences", "⚙ Preferencias").build(app)?;
    let refresh_item = MenuItemBuilder::with_id("refresh", "⟳ Refrescar").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Salir").build(app)?;

    builder
        .item(&refreshed)
        .item(&preferences_item)
        .item(&refresh_item)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&quit_item)
        .build()
}
```

- [ ] **Step 2: Register the module**

```rust
mod tray;
```

- [ ] **Step 3: Manual verification checklist** (documented here since Tauri menu trees can't be asserted headlessly — wired into a running app in Task 15)

Once Task 15 wires `build_menu` into the tray, manually confirm:
1. With only Claude Code data present: only the "Claude Code" section shows, "opencode" section is absent.
2. With no data for either agent: the empty-state line appears and no agent sections do.
3. "Ver por proyecto" opens a submenu listing each project with correct tokens/cost.
4. Budget bars visually fill proportionally to `spent/budget` and clamp at 10/10 blocks past 100%.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/tray.rs src-tauri/src/main.rs
git commit -m "feat: build native tray menu from agent summaries"
```

---

### Task 15: Refresh loop, first-run loading state, and app wiring

**Files:**
- Modify: `src-tauri/src/main.rs`

**Interfaces:**
- Consumes: everything produced by Tasks 2-14.
- Produces: `AppState` (Tauri managed state) holding a `FileCache<UsageEvent>` for Claude Code, `Mutex<PricingTable>`, `Mutex<Preferences>`, `Mutex<DateTime<Utc>>` for last refresh, and the `TrayIcon` handle; a `refresh_all(app: &AppHandle)` function that Task 16's "Refrescar" handler and the periodic timer both call. This is the task where the app becomes runnable end-to-end.

> **Revised 2026-07-15**, following Task 4's revision: opencode data now
> comes from a SQLite database (`parsers::opencode::open_read_only` /
> `load_all`), not a directory walk, and isn't cached the way Claude Code's
> files are (a single local SQLite query is cheap enough to re-run every
> refresh). `AppState` no longer has an `opencode_cache` field, and
> `gather_opencode_events` opens the DB fresh each call. The wiring below
> supersedes the version originally drafted before Task 4's schema recon.

- [ ] **Step 1: Replace `src-tauri/src/main.rs` with the full wiring**

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cache;
mod menu_format;
mod model;
mod parsers;
mod preferences;
mod price_fetch;
mod pricing;
mod summary;
mod tray;
mod windows;

use cache::FileCache;
use chrono::Utc;
use model::{Agent, UsageEvent};
use preferences::Preferences;
use pricing::PricingTable;
use std::sync::Mutex;
use tauri::tray::TrayIcon;
use tauri::{AppHandle, Manager};

pub struct AppState {
    pub claude_cache: Mutex<FileCache<UsageEvent>>,
    pub preferences: Mutex<Preferences>,
    pub pricing: Mutex<PricingTable>,
    pub last_refresh: Mutex<chrono::DateTime<Utc>>,
    pub tray: Mutex<Option<TrayIcon>>,
}

fn claude_projects_dir() -> std::path::PathBuf {
    dirs::home_dir().expect("home dir must resolve").join(".claude").join("projects")
}

fn opencode_db_path() -> std::path::PathBuf {
    if let Ok(custom) = std::env::var("OPENCODE_DATA_DIR") {
        return std::path::PathBuf::from(custom).join("opencode.db");
    }
    dirs::data_local_dir()
        .expect("data-local dir must resolve")
        .join("opencode")
        .join("opencode.db")
}

fn config_dir() -> std::path::PathBuf {
    dirs::config_dir().expect("config dir must resolve").join("ai-usage-tray")
}

fn gather_claude_events(state: &AppState) -> Vec<UsageEvent> {
    let dir = claude_projects_dir();
    let mut cache = state.claude_cache.lock().unwrap();
    let mut events = Vec::new();
    for file in parsers::claude_code::discover_files(&dir) {
        let project = parsers::claude_code::folder_slug_project_name(&file);
        let parsed = cache.get_or_parse(&file, |content| {
            parsers::claude_code::parse_jsonl_content(content, &project)
        });
        events.extend(parsed);
    }
    events
}

fn gather_opencode_events() -> Vec<UsageEvent> {
    match parsers::opencode::open_read_only(&opencode_db_path()) {
        Some(conn) => parsers::opencode::load_all(&conn),
        None => Vec::new(),
    }
}

pub fn refresh_all(app: &AppHandle) {
    let state = app.state::<AppState>();

    let claude_events = gather_claude_events(&state);
    let opencode_events = gather_opencode_events();

    let prefs = state.preferences.lock().unwrap().clone();
    if prefs.network_pricing_refresh_enabled {
        let current = state.pricing.lock().unwrap().clone();
        let source = price_fetch::HttpPriceSource {
            url: "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json".to_string(),
        };
        let refreshed = price_fetch::refresh_pricing_table(&source, current);
        *state.pricing.lock().unwrap() = refreshed;
    }
    let pricing = state.pricing.lock().unwrap().clone();

    let now = Utc::now();
    let claude_summary = summary::build_summary(Agent::ClaudeCode, &claude_events, &pricing, now);
    let opencode_summary = summary::build_summary(Agent::OpenCode, &opencode_events, &pricing, now);

    *state.last_refresh.lock().unwrap() = now;

    if let Ok(menu) = tray::build_menu(
        app,
        claude_summary.as_ref(),
        opencode_summary.as_ref(),
        &prefs,
        now,
        now,
    ) {
        if let Some(tray_icon) = state.tray.lock().unwrap().as_ref() {
            let _ = tray_icon.set_menu(Some(menu));
        }
    }
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            claude_cache: Mutex::new(FileCache::new()),
            preferences: Mutex::new(preferences::load(&config_dir())),
            pricing: Mutex::new(pricing::embedded_pricing_table()),
            last_refresh: Mutex::new(Utc::now()),
            tray: Mutex::new(None),
        })
        .setup(|app| {
            let handle = app.handle().clone();

            // First-run loading state so the tray never blocks or shows
            // stale/misleading data while the initial scan runs.
            let loading_item =
                tauri::menu::MenuItemBuilder::new("Cargando…").enabled(false).build(app)?;
            let loading_menu = tauri::menu::MenuBuilder::new(app).item(&loading_item).build()?;

            let tray_icon = tauri::tray::TrayIconBuilder::new()
                .menu(&loading_menu)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    "refresh" => refresh_all(app),
                    "preferences" => {
                        let _ = crate::open_preferences_window(app);
                    }
                    _ => {}
                })
                .build(app)?;

            *app.state::<AppState>().tray.lock().unwrap() = Some(tray_icon);

            // Kick off the first (potentially slow) scan off the main thread.
            let refresh_handle = handle.clone();
            tauri::async_runtime::spawn_blocking(move || {
                refresh_all(&refresh_handle);
            });

            // Periodic refresh honoring the configured interval; re-reads the
            // interval each cycle in case Preferences changed it.
            let timer_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let interval_secs = {
                        let state = timer_handle.state::<AppState>();
                        state.preferences.lock().unwrap().refresh_interval_secs
                    };
                    tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
                    let h = timer_handle.clone();
                    tauri::async_runtime::spawn_blocking(move || refresh_all(&h));
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_preferences_cmd,
            save_preferences_cmd
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[tauri::command]
fn get_preferences_cmd(app: AppHandle) -> Preferences {
    app.state::<AppState>().preferences.lock().unwrap().clone()
}

#[tauri::command]
fn save_preferences_cmd(app: AppHandle, prefs: Preferences) -> Result<(), String> {
    preferences::save(&config_dir(), &prefs).map_err(|e| e.to_string())?;
    *app.state::<AppState>().preferences.lock().unwrap() = prefs;
    refresh_all(&app);
    Ok(())
}

fn open_preferences_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("preferences") {
        return window.set_focus();
    }
    tauri::WebviewWindowBuilder::new(
        app,
        "preferences",
        tauri::WebviewUrl::App("preferences.html".into()),
    )
    .title("Preferencias — AI Usage")
    .inner_size(360.0, 320.0)
    .resizable(false)
    .build()?;
    Ok(())
}
```

Note: `open_preferences_window` references `preferences.html`, which Task 16 creates. Until Task 16 lands, the "⚙ Preferencias" menu item will build a window pointing at a missing file — that's expected and fixed by the next task.

- [ ] **Step 2: Build and manually verify the end-to-end flow**

```bash
cd src-tauri
cargo build
cargo tauri dev
```

Manual check: tray shows "Cargando…" briefly, then either the empty-state message or real sections depending on what's on your machine (per Task 14's checklist); "⟳ Refrescar" immediately triggers a rebuild (watch the "Refrescado hace Xs" line reset); the app doesn't freeze during the initial scan.

- [ ] **Step 3: Run the full test suite to confirm nothing regressed**

```bash
cargo test
```

Expected: all tests from Tasks 2-13 still pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/main.rs
git commit -m "feat: wire refresh loop, tray menu, and app state end-to-end"
```

---

### Task 16: Preferences window

**Files:**
- Create: `ui/preferences.html`
- Modify: `src-tauri/tauri.conf.json` (ensure `ui/` — or your renamed frontend dir — is the configured `frontendDist`)

**Interfaces:**
- Consumes: `get_preferences_cmd` / `save_preferences_cmd` Tauri commands from Task 15.
- Produces: a working settings UI; no Rust-facing interface (leaf of the dependency graph).

- [ ] **Step 1: Write the settings form**

```html
<!-- ui/preferences.html -->
<!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8" />
  <style>
    body { font-family: -apple-system, sans-serif; padding: 16px; font-size: 13px; }
    label { display: block; margin-top: 10px; }
    input[type="number"] { width: 100px; }
    #status { margin-top: 12px; color: green; height: 16px; }
  </style>
</head>
<body>
  <h3>Presupuestos personales (USD)</h3>
  <label>Bloque de 5h <input id="budget5h" type="number" min="0" step="1" /></label>
  <label>Ventana de 7 días <input id="budget7d" type="number" min="0" step="1" /></label>
  <label>Mensual <input id="budgetMonthly" type="number" min="0" step="1" /></label>

  <h3>Refresco</h3>
  <label>Intervalo (segundos) <input id="refreshInterval" type="number" min="10" step="10" /></label>
  <label><input id="networkPricing" type="checkbox" /> Actualizar precios por red</label>

  <button id="save">Guardar</button>
  <div id="status"></div>

  <script>
    const { invoke } = window.__TAURI__.core;

    async function load() {
      const prefs = await invoke("get_preferences_cmd");
      document.getElementById("budget5h").value = prefs.budget_5h_usd;
      document.getElementById("budget7d").value = prefs.budget_7d_usd;
      document.getElementById("budgetMonthly").value = prefs.budget_monthly_usd;
      document.getElementById("refreshInterval").value = prefs.refresh_interval_secs;
      document.getElementById("networkPricing").checked = prefs.network_pricing_refresh_enabled;
    }

    document.getElementById("save").addEventListener("click", async () => {
      const prefs = {
        budget_5h_usd: Number(document.getElementById("budget5h").value),
        budget_7d_usd: Number(document.getElementById("budget7d").value),
        budget_monthly_usd: Number(document.getElementById("budgetMonthly").value),
        refresh_interval_secs: Number(document.getElementById("refreshInterval").value),
        network_pricing_refresh_enabled: document.getElementById("networkPricing").checked,
      };
      await invoke("save_preferences_cmd", { prefs });
      const status = document.getElementById("status");
      status.textContent = "Guardado.";
      setTimeout(() => (status.textContent = ""), 1500);
    });

    load();
  </script>
</body>
</html>
```

- [ ] **Step 2: Point Tauri's frontend dist at `ui/`**

In `src-tauri/tauri.conf.json`, under `"build"`, set:

```json
{
  "build": {
    "frontendDist": "../ui"
  }
}
```

- [ ] **Step 3: Manual verification**

```bash
cd src-tauri
cargo tauri dev
```

Click "⚙ Preferencias" in the tray menu: a window opens showing current values (defaults on first run); change a value, click "Guardar", see "Guardado." appear, close and reopen the window to confirm the value persisted; confirm the tray menu's budget bars reflect the new budgets after the save-triggered refresh.

- [ ] **Step 4: Commit**

```bash
git add ui/preferences.html src-tauri/tauri.conf.json
git commit -m "feat: add preferences window for personal budget thresholds"
```

---

### Task 17: Packaging and README

**Files:**
- Create: `README.md`

**Interfaces:** None — documentation only, no code interfaces.

- [ ] **Step 1: Write the README**

```markdown
# AI Usage Tray Widget

Tray/menu-bar app for macOS and Linux showing local token/cost consumption
for Claude Code and opencode. Reads only files these tools already write to
disk — no accounts, no telemetry, no server of its own.

## Build

\`\`\`bash
cd src-tauri
cargo tauri build
\`\`\`

Produces a platform-native bundle (`.app` on macOS, `.deb`/`.AppImage` on
Linux, depending on your `cargo tauri build` target flags).

## Linux prerequisite

The tray icon depends on `libappindicator`/`ayatana-appindicator` being
installed on your system. On Debian/Ubuntu:

\`\`\`bash
sudo apt install libayatana-appindicator3-1
\`\`\`

Other distros: install the equivalent package for your desktop environment.
Without it, the app runs but no tray icon appears.

## Data sources

- Claude Code: `~/.claude/projects/**/*.jsonl`
- opencode: `$OPENCODE_DATA_DIR/opencode.db` (defaults to
  `~/.local/share/opencode/opencode.db`), opened read-only. One usage entry
  per opencode session (session-level token totals), not per message.

If neither source has usage data, the tray shows an empty-state message
instead of the per-agent sections.

## Scope (v1)

- macOS and Linux only.
- No persistent history beyond what Claude Code/opencode retain on disk —
  every refresh recomputes from source files.
- Budget bars (5h block / 7-day window) are against a **personal budget you
  configure in Preferences**, not Anthropic's real account limit — that
  value isn't available locally.
\`\`\`
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add README with build instructions and Linux tray prerequisite"
```

---

## Self-Review Notes

- **Spec coverage:** every spec section maps to a task — architecture (1, 15), Claude Code parser (3), opencode parser (4), pricing/cost (5, 12), time windows (6, 7, 8), personal budgets (11, 13, 14), dropdown UI (13, 14), refresh mechanism (15), preferences window (16), error handling/empty-state (3, 4, 9, 14, 15), packaging (17).
- **opencode schema risk:** flagged explicitly in Task 4 rather than papered over — the implementer must confirm real field names against a live install before trusting the fixtures.
- **Type consistency check:** `UsageEvent`, `TokenCost`, `AgentSummary`, `ProjectBreakdown`, `PricingTable`, `Preferences`, `FileCache<T>` are each defined once (Tasks 2, 6, 9, 9, 5, 11, 10) and referenced with identical names/signatures in every later task that consumes them.
- **No placeholders:** every step has runnable code or exact commands; the one open unknown (opencode's exact JSON schema) is handled as a concrete reconnaissance step with real commands and a clear decision rule, not a "TBD".
