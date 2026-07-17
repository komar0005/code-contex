// src-tauri/src/statusline.rs
//
// The `--statusline` runtime mode: Claude Code invokes the binary this way
// repeatedly (debounced per turn) with session JSON on stdin, and paints
// each line of our stdout as a row at the bottom of the terminal. This path must
// NEVER fail loudly, must NEVER call the network or rescan JSONL files,
// and must return in milliseconds — any of that would degrade the user's
// actual coding session. Per
// docs/superpowers/specs/2026-07-16-statusline-integration-design.md.

use crate::history::{self, SessionUpdate};
use crate::model::Agent;
use crate::statusline_format::StatuslineRender;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
struct StatuslineInput {
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<ModelInfo>,
    cost: Option<CostInfo>,
    workspace: Option<WorkspaceInfo>,
    context_window: Option<ContextWindowInfo>,
    rate_limits: Option<RateLimitsInfo>,
}

#[derive(Debug, Deserialize, Default)]
struct ModelInfo {
    display_name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct CostInfo {
    total_cost_usd: Option<f64>,
    total_duration_ms: Option<u64>,
    total_lines_added: Option<u64>,
    total_lines_removed: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct WorkspaceInfo {
    current_dir: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ContextWindowInfo {
    used_percentage: Option<f64>,
}

#[derive(Debug, Deserialize, Default)]
struct RateLimitsInfo {
    five_hour: Option<RateLimitWindow>,
    seven_day: Option<RateLimitWindow>,
}

#[derive(Debug, Deserialize, Default)]
struct RateLimitWindow {
    used_percentage: Option<f64>,
}

/// Written by the (separately running) tray app on every refresh; read here
/// so this mode never recomputes or hits the network itself.
#[derive(Debug, Serialize, Deserialize)]
struct Snapshot {
    written_at: DateTime<Utc>,
    tray_title: Option<String>,
    today_cost: Option<String>,
    refresh_interval_secs: u64,
}

/// Entry point for `ai-usage-tray --statusline`. Reads stdin, persists
/// whatever session data it can, and prints the status line. Called before
/// Tauri/GTK ever initializes — this path must stay GUI-free so a terminal
/// invoking it dozens of times per session pays no startup cost.
pub fn run(history_db_path: &Path, snapshot_path: &Path) {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let input: StatuslineInput = serde_json::from_str(&raw).unwrap_or_default();

    persist_session(history_db_path, &input);

    let dir = input
        .workspace
        .as_ref()
        .and_then(|w| w.current_dir.as_deref())
        .or(input.cwd.as_deref());
    let branch = git_branch(dir);
    println!("{}", build_output(&input, branch.as_deref(), read_snapshot(snapshot_path).as_ref()));
}

fn persist_session(db_path: &Path, input: &StatuslineInput) {
    let Some(session_id) = input.session_id.as_deref() else { return };
    // Independent of whether the tray app is running — a legitimate use is
    // "I only want the data, I never open the panel."
    let Some(conn) = history::open(db_path) else { return };
    let now = Utc::now();
    let date = now.with_timezone(&Local).date_naive().to_string();
    let cost = input.cost.as_ref();
    let update = SessionUpdate {
        session_id,
        date: &date,
        agent: Agent::ClaudeCode,
        project: input.cwd.as_deref(),
        model: input.model.as_ref().and_then(|m| m.display_name.as_deref()),
        cost_usd: cost.and_then(|c| c.total_cost_usd).unwrap_or(0.0),
        lines_added: cost.and_then(|c| c.total_lines_added).unwrap_or(0),
        lines_removed: cost.and_then(|c| c.total_lines_removed).unwrap_or(0),
        duration_ms: cost.and_then(|c| c.total_duration_ms).unwrap_or(0),
        updated_at: now,
    };
    history::upsert_session(&conn, &update);
}

/// `git branch --show-current` en el directorio de la sesión. Salida
/// vacía (HEAD detached) y cualquier fallo (sin git, sin repo, sin dir)
/// significan lo mismo: no hay rama que mostrar.
fn git_branch(dir: Option<&str>) -> Option<String> {
    let dir = dir?;
    let output = std::process::Command::new("git")
        .args(["-C", dir, "branch", "--show-current"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

/// Called by the tray app (`refresh_all`) every refresh cycle — never from
/// `--statusline` mode itself.
pub fn write_snapshot(
    path: &Path,
    tray_title: Option<&str>,
    today_cost: Option<&str>,
    refresh_interval_secs: u64,
    now: DateTime<Utc>,
) {
    let snapshot = Snapshot {
        written_at: now,
        tray_title: tray_title.map(String::from),
        today_cost: today_cost.map(String::from),
        refresh_interval_secs,
    };
    if let Ok(content) = serde_json::to_string(&snapshot) {
        let _ = std::fs::write(path, content);
    }
}

/// A snapshot older than 2x its own refresh interval means the tray app is
/// closed or hung — treated as absent rather than shown as if current.
fn read_snapshot(path: &Path) -> Option<Snapshot> {
    let content = std::fs::read_to_string(path).ok()?;
    let snapshot: Snapshot = serde_json::from_str(&content).ok()?;
    let max_age = chrono::Duration::seconds(2 * snapshot.refresh_interval_secs as i64);
    if Utc::now() - snapshot.written_at > max_age {
        return None;
    }
    Some(snapshot)
}

/// Extrae de stdin+snapshot los datos ya listos para pintar y delega en
/// statusline_format::render. El tray_title del snapshot viaja siempre
/// como fallback; render lo ignora si el stdin trajo límites reales.
fn build_output(
    input: &StatuslineInput,
    branch: Option<&str>,
    snapshot: Option<&Snapshot>,
) -> String {
    let limits = input.rate_limits.as_ref();
    let render = StatuslineRender {
        branch,
        model: input.model.as_ref().and_then(|m| m.display_name.as_deref()),
        context_pct: input.context_window.as_ref().and_then(|c| c.used_percentage),
        five_hour_pct: limits
            .and_then(|l| l.five_hour.as_ref())
            .and_then(|w| w.used_percentage),
        seven_day_pct: limits
            .and_then(|l| l.seven_day.as_ref())
            .and_then(|w| w.used_percentage),
        fallback_limits_text: snapshot.and_then(|s| s.tray_title.as_deref()),
        today_cost: snapshot.and_then(|s| s.today_cost.as_deref()),
    };
    crate::statusline_format::render(&render)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 16, 20, 0, 0).unwrap()
    }

    fn input_with_model(name: &str) -> StatuslineInput {
        StatuslineInput {
            session_id: Some("s1".into()),
            cwd: Some("/home/user/project-a".into()),
            model: Some(ModelInfo { display_name: Some(name.to_string()) }),
            cost: Some(CostInfo {
                total_cost_usd: Some(1.42),
                total_duration_ms: Some(340_000),
                total_lines_added: Some(58),
                total_lines_removed: Some(12),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn git_branch_none_outside_a_repo_or_without_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(git_branch(Some(dir.path().to_str().unwrap())), None);
        assert_eq!(git_branch(None), None);
    }

    #[test]
    fn git_branch_reads_current_branch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-b", "feature-x", path])
            .output()
            .unwrap();
        assert!(init.status.success());
        assert_eq!(git_branch(Some(path)), Some("feature-x".to_string()));
    }

    #[test]
    fn build_output_prefers_stdin_limits_over_snapshot_title() {
        let mut input = input_with_model("Sonnet 5");
        input.context_window = Some(ContextWindowInfo { used_percentage: Some(41.0) });
        input.rate_limits = Some(RateLimitsInfo {
            five_hour: Some(RateLimitWindow { used_percentage: Some(62.0) }),
            seven_day: Some(RateLimitWindow { used_percentage: Some(34.0) }),
        });
        let snapshot = Snapshot {
            written_at: now(),
            tray_title: Some("5h 99% · 7d 99%".into()),
            today_cost: Some("$4.30".into()),
            refresh_interval_secs: 60,
        };
        let out = build_output(&input, Some("main"), Some(&snapshot));
        assert_eq!(
            crate::statusline_format::strip_ansi(&out),
            "🌿 main · Sonnet 5 · ctx ▰▰▰▰▱▱▱▱▱▱ 41%\n5h ▰▰▰▰▰▰▱▱▱▱ 62% · 7d ▰▰▰▱▱▱▱▱▱▱ 34% · hoy $4.30"
        );
    }

    #[test]
    fn build_output_falls_back_to_snapshot_title_without_stdin_limits() {
        let snapshot = Snapshot {
            written_at: now(),
            tray_title: Some("5h 62% · 7d 34%".into()),
            today_cost: Some("$4.30".into()),
            refresh_interval_secs: 60,
        };
        let out = build_output(&input_with_model("Sonnet 5"), None, Some(&snapshot));
        assert_eq!(
            crate::statusline_format::strip_ansi(&out),
            "Sonnet 5\n5h 62% · 7d 34% · hoy $4.30"
        );
    }

    #[test]
    fn build_output_model_only_without_snapshot() {
        let out = build_output(&input_with_model("Sonnet 5"), None, None);
        assert_eq!(crate::statusline_format::strip_ansi(&out), "Sonnet 5");
    }

    #[test]
    fn build_output_empty_when_nothing_is_known() {
        assert_eq!(build_output(&StatuslineInput::default(), None, None), "");
    }

    #[test]
    fn read_snapshot_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_snapshot(&dir.path().join("missing.json")).is_none());
    }

    #[test]
    fn read_snapshot_none_when_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.json");
        write_snapshot(&path, Some("5h 62%"), Some("$4.30"), 60, now() - chrono::Duration::minutes(10));
        // Reader compares against real "now" (Utc::now()), so an ancient
        // written_at is stale regardless of when the test runs.
        assert!(read_snapshot(&path).is_none());
    }

    #[test]
    fn read_snapshot_some_when_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.json");
        write_snapshot(&path, Some("5h 62%"), Some("$4.30"), 60, Utc::now());
        let snapshot = read_snapshot(&path).unwrap();
        assert_eq!(snapshot.tray_title.as_deref(), Some("5h 62%"));
    }

    #[test]
    fn persist_session_without_session_id_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("history.db");
        persist_session(&db_path, &StatuslineInput::default());
        assert!(!db_path.exists()); // never even opened the db
    }

    #[test]
    fn persist_session_upserts_into_history_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("history.db");
        persist_session(&db_path, &input_with_model("claude-sonnet-5"));
        let conn = history::open(&db_path).unwrap();
        assert!(history::has_any_sessions(&conn, Agent::ClaudeCode));
    }

    #[test]
    fn parses_workspace_context_window_and_rate_limits() {
        let json = r#"{
            "session_id": "s1",
            "cwd": "/home/user/p",
            "model": {"display_name": "Sonnet 5"},
            "workspace": {"current_dir": "/home/user/p/sub"},
            "context_window": {"used_percentage": 41.2},
            "rate_limits": {
                "five_hour": {"used_percentage": 62.0, "resets_at": 1789000000},
                "seven_day": {"used_percentage": 34.5}
            }
        }"#;
        let input: StatuslineInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.workspace.unwrap().current_dir.as_deref(), Some("/home/user/p/sub"));
        assert_eq!(input.context_window.unwrap().used_percentage, Some(41.2));
        let limits = input.rate_limits.unwrap();
        assert_eq!(limits.five_hour.unwrap().used_percentage, Some(62.0));
        assert_eq!(limits.seven_day.unwrap().used_percentage, Some(34.5));
    }
}
