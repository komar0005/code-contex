use crate::model::{Agent, UsageEvent};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Claude Code writes one JSONL line per content block of the same assistant
/// message (thinking, text, tool_use...), each repeating the full `usage`
/// object under the same `message.id` — counting lines directly overcounts
/// ~2x. Dedupe by message id, keeping the LAST occurrence: streaming updates
/// mean the final line carries the definitive usage. Lines with
/// `model: "<synthetic>"` are Claude Code error placeholders, not real API
/// calls, and are skipped entirely.
pub fn parse_jsonl_content(content: &str, fallback_project: &str) -> Vec<UsageEvent> {
    let mut by_id: HashMap<String, UsageEvent> = HashMap::new();
    let mut without_id: Vec<UsageEvent> = Vec::new();
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
        if model == "<synthetic>" {
            continue;
        }
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

        let event = UsageEvent {
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
        };

        match message.get("id").and_then(Value::as_str) {
            // Later lines for the same message overwrite earlier ones.
            Some(id) => {
                by_id.insert(id.to_string(), event);
            }
            // No id: can't dedupe, keep as-is rather than lose data.
            None => without_id.push(event),
        }
    }
    let mut events: Vec<UsageEvent> = by_id.into_values().collect();
    events.append(&mut without_id);
    // HashMap iteration order is random; sort for deterministic output.
    events.sort_by_key(|e| e.timestamp);
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
    fn dedupes_by_message_id_keeping_last_and_skips_synthetic() {
        let events = parse_jsonl_content(&fixture(), "fallback");
        // msg_A (deduped, last occurrence wins) + msg_B. msg_C is synthetic -> skipped.
        assert_eq!(events.len(), 2);

        // Sorted by timestamp: msg_A (10:00:03) first, msg_B (11:30) second.
        assert_eq!(events[0].model, "claude-sonnet-5");
        assert_eq!(events[0].project, "/home/user/project-a");
        // Last occurrence's usage, NOT the first line's input_tokens: 999.
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].output_tokens, 50);
        assert_eq!(events[0].cache_write_tokens, 200);
        assert_eq!(events[0].cache_read_tokens, 300);

        assert_eq!(events[1].model, "claude-haiku-4-5-20251001");
        assert_eq!(events[1].total_tokens(), 15);
        assert_eq!(events[1].project, "fallback");
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
