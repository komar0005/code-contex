use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preferences {
    pub budget_5h_usd: f64,
    pub budget_7d_usd: f64,
    pub budget_monthly_usd: f64,
    pub refresh_interval_secs: u64,
    pub network_pricing_refresh_enabled: bool,
    /// Show "5h 62% · 7d 34%" next to the tray icon (where the desktop
    /// environment supports appindicator labels).
    #[serde(default = "default_show_tray_metric")]
    pub show_tray_metric: bool,
}

fn default_show_tray_metric() -> bool {
    true
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            budget_5h_usd: 10.0,
            budget_7d_usd: 50.0,
            budget_monthly_usd: 150.0,
            refresh_interval_secs: 60,
            network_pricing_refresh_enabled: true,
            show_tray_metric: true,
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
            show_tray_metric: false,
        };
        save(dir.path(), &prefs).unwrap();
        let loaded = load(dir.path());
        assert_eq!(loaded, prefs);
    }

    #[test]
    fn preferences_json_without_show_tray_metric_defaults_to_true() {
        // A file written by the previous app version must still load.
        let dir = tempfile::tempdir().unwrap();
        let legacy = r#"{
            "budget_5h_usd": 25.0,
            "budget_7d_usd": 100.0,
            "budget_monthly_usd": 300.0,
            "refresh_interval_secs": 30,
            "network_pricing_refresh_enabled": false
        }"#;
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(preferences_path(dir.path()), legacy).unwrap();
        let prefs = load(dir.path());
        assert!(prefs.show_tray_metric);
        assert_eq!(prefs.budget_5h_usd, 25.0); // other fields preserved
    }
}
