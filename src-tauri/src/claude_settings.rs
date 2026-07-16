// src-tauri/src/claude_settings.rs
//
// Reads/merges/writes Claude Code's own `~/.claude/settings.json` — a file
// this app doesn't own. Every write here is triggered by an explicit user
// action from the panel's Settings view (never automatic, never on
// startup) and touches ONLY the `statusLine` key, preserving everything
// else in the file untouched. Per
// docs/superpowers/specs/2026-07-16-statusline-integration-design.md.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn default_settings_path() -> PathBuf {
    dirs::home_dir().expect("home dir must resolve").join(".claude").join("settings.json")
}

/// The exact `statusLine.command` string this app would install: the
/// running binary's own path (quoted, in case it contains spaces) plus
/// `--statusline`. Comparing against this is how we tell "ours" apart from
/// "something the user configured".
pub fn our_statusline_command() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(format!("\"{}\" --statusline", exe.display()))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StatuslineState {
    NotConfigured,
    Ours { command: String },
    Foreign { command: String },
}

/// Read-only: never writes. Distinguishes "nothing configured" from "ours"
/// from "the user (or another tool) configured something else" so the
/// panel can ask before ever overwriting a foreign command.
pub fn check_statusline(path: &Path, our_command: &str) -> Result<StatuslineState, String> {
    let root = load_or_empty(path)?;
    Ok(match statusline_command(&root) {
        None => StatuslineState::NotConfigured,
        Some(cmd) if cmd == our_command => StatuslineState::Ours { command: cmd },
        Some(cmd) => StatuslineState::Foreign { command: cmd },
    })
}

/// Sets `statusLine.command` to `command`, merging into whatever else is in
/// the file. Only ever called after the panel has asked the user (and, if
/// something foreign was configured, shown them what it was).
pub fn install_statusline(path: &Path, command: &str) -> Result<(), String> {
    let mut root = load_or_empty(path)?;
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "settings.json no es un objeto JSON".to_string())?;
    obj.insert("statusLine".to_string(), json!({ "type": "command", "command": command }));
    write_settings(path, &root)
}

/// Removes the `statusLine` key ONLY if its command still matches
/// `expected_command` exactly — if the user changed it after we installed
/// it, this refuses rather than deleting their customization.
pub fn uninstall_statusline(path: &Path, expected_command: &str) -> Result<(), String> {
    let mut root = load_or_empty(path)?;
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "settings.json no es un objeto JSON".to_string())?;
    match statusline_command(&Value::Object(obj.clone())) {
        Some(cmd) if cmd == expected_command => {
            obj.remove("statusLine");
            write_settings(path, &root)
        }
        Some(_) => Err(
            "la statusLine configurada ya no es la que instaló esta app; no se modificó nada"
                .to_string(),
        ),
        None => Ok(()), // already absent — nothing to do
    }
}

fn statusline_command(root: &Value) -> Option<String> {
    root.get("statusLine")?.get("command")?.as_str().map(str::to_string)
}

/// Missing file -> empty object (nothing to preserve yet). An existing file
/// that fails to parse is surfaced as an error instead of being silently
/// discarded — we must never clobber content we can't understand.
fn load_or_empty(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content)
            .map_err(|e| format!("no se pudo leer {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("no se pudo leer {}: {e}", path.display())),
    }
}

fn write_settings(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = "\"/opt/ai-usage-tray\" --statusline";

    #[test]
    fn missing_file_is_not_configured() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(check_statusline(&path, OURS).unwrap(), StatuslineState::NotConfigured);
    }

    #[test]
    fn install_on_missing_file_creates_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        install_statusline(&path, OURS).unwrap();
        assert_eq!(
            check_statusline(&path, OURS).unwrap(),
            StatuslineState::Ours { command: OURS.to_string() }
        );
    }

    #[test]
    fn install_preserves_unrelated_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"permissions": {"allow": ["Bash"]}, "model": "opus"}"#).unwrap();
        install_statusline(&path, OURS).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(value["permissions"]["allow"][0], "Bash");
        assert_eq!(value["model"], "opus");
        assert_eq!(value["statusLine"]["command"], OURS);
    }

    #[test]
    fn install_over_foreign_command_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"statusLine": {"type": "command", "command": "my-script"}}"#)
            .unwrap();
        assert_eq!(
            check_statusline(&path, OURS).unwrap(),
            StatuslineState::Foreign { command: "my-script".to_string() }
        );
        install_statusline(&path, OURS).unwrap();
        assert_eq!(
            check_statusline(&path, OURS).unwrap(),
            StatuslineState::Ours { command: OURS.to_string() }
        );
    }

    #[test]
    fn uninstall_removes_matching_key_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"model": "opus"}"#).unwrap();
        install_statusline(&path, OURS).unwrap();
        uninstall_statusline(&path, OURS).unwrap();
        assert_eq!(check_statusline(&path, OURS).unwrap(), StatuslineState::NotConfigured);
        let content = std::fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(value["model"], "opus"); // untouched
    }

    #[test]
    fn uninstall_refuses_when_command_no_longer_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        install_statusline(&path, OURS).unwrap();
        // User (or something else) changed it after we installed.
        std::fs::write(&path, r#"{"statusLine": {"type": "command", "command": "custom"}}"#)
            .unwrap();
        assert!(uninstall_statusline(&path, OURS).is_err());
        // Untouched.
        assert_eq!(
            check_statusline(&path, OURS).unwrap(),
            StatuslineState::Foreign { command: "custom".to_string() }
        );
    }

    #[test]
    fn uninstall_when_already_absent_is_a_no_op_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"model": "opus"}"#).unwrap();
        assert!(uninstall_statusline(&path, OURS).is_ok());
    }

    #[test]
    fn unparseable_existing_file_errors_instead_of_being_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not valid json").unwrap();
        assert!(check_statusline(&path, OURS).is_err());
        assert!(install_statusline(&path, OURS).is_err());
        // File must be untouched.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not valid json");
    }
}
