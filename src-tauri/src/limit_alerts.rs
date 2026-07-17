// Escalating desktop notifications when a REAL limit window nears
// exhaustion. Pure decision logic — the actual notification send lives in
// `refresh_all`, so everything here is unit-testable without a desktop.

use crate::limits::LimitsSnapshot;
use crate::menu_format::format_reset_in;
use crate::model::Agent;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// Percent at which the amber warning fires.
pub const WARNING_PERCENT: f64 = 80.0;
/// Percent at which the red critical alert fires.
pub const CRITICAL_PERCENT: f64 = 95.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertLevel {
    Warning,
    Critical,
}

impl AlertLevel {
    /// Freedesktop theme icon shown by the notification daemon — amber
    /// triangle for warning, red mark for error. Ignored on platforms
    /// without themed icons; the emoji in the title carries the color there.
    pub fn icon(self) -> &'static str {
        match self {
            AlertLevel::Warning => "dialog-warning",
            AlertLevel::Critical => "dialog-error",
        }
    }
}

fn level_for(used_percent: f64) -> Option<AlertLevel> {
    if used_percent >= CRITICAL_PERCENT {
        Some(AlertLevel::Critical)
    } else if used_percent >= WARNING_PERCENT {
        Some(AlertLevel::Warning)
    } else {
        None
    }
}

/// One desktop notification ready to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub level: AlertLevel,
    pub title: String,
    pub body: String,
}

fn agent_label(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "Claude Code",
        Agent::OpenCode => "opencode",
    }
}

fn build_alert(
    level: AlertLevel,
    agent: Agent,
    window_label: &str,
    used_percent: f64,
    resets_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Alert {
    let title = match level {
        AlertLevel::Warning => {
            format!("⚠️ {}: límite {window_label} al {used_percent:.0}%", agent_label(agent))
        }
        AlertLevel::Critical => {
            format!("🔴 {}: límite {window_label} al {used_percent:.0}%", agent_label(agent))
        }
    };
    let mut body = match level {
        AlertLevel::Warning => {
            format!("Los tokens de la ventana de {window_label} se están agotando")
        }
        AlertLevel::Critical => {
            format!("Ventana de {window_label} casi agotada (≥{CRITICAL_PERCENT:.0}%)")
        }
    };
    if let Some(resets_at) = resets_at {
        body.push_str(&format!(" · {}", format_reset_in(resets_at, now)));
    }
    Alert { level, title, body }
}

/// Remembers the highest level already notified per (agent, window) so the
/// periodic refresh never re-sends the same alert. Dropping back below a
/// threshold (window reset) re-arms it; jumping straight past both
/// thresholds sends only the critical one.
#[derive(Default)]
pub struct LimitAlerts {
    notified: HashMap<(Agent, &'static str), AlertLevel>,
}

impl LimitAlerts {
    pub fn check(
        &mut self,
        agent: Agent,
        limits: Option<&LimitsSnapshot>,
        now: DateTime<Utc>,
    ) -> Vec<Alert> {
        let Some(limits) = limits else { return Vec::new() };
        let windows: [(&'static str, _); 2] =
            [("5h", limits.five_hour.as_ref()), ("7d", limits.seven_day.as_ref())];
        let mut alerts = Vec::new();
        for (label, window) in windows {
            let Some(window) = window else { continue };
            let level = level_for(window.used_percent);
            let previous = self.notified.get(&(agent, label)).copied();
            match level {
                Some(level) if previous.map_or(true, |p| level > p) => {
                    self.notified.insert((agent, label), level);
                    alerts.push(build_alert(
                        level,
                        agent,
                        label,
                        window.used_percent,
                        window.resets_at,
                        now,
                    ));
                }
                Some(level) => {
                    // Same or lower level: keep state in sync (a drop from
                    // critical back to warning re-arms critical) but stay
                    // silent.
                    self.notified.insert((agent, label), level);
                }
                None => {
                    self.notified.remove(&(agent, label));
                }
            }
        }
        alerts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::RateWindow;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 16, 12, 0, 0).unwrap()
    }

    fn snapshot(five_hour: Option<f64>, seven_day: Option<f64>) -> LimitsSnapshot {
        LimitsSnapshot {
            five_hour: five_hour.map(|p| RateWindow::new(p, None)),
            seven_day: seven_day.map(|p| RateWindow::new(p, None)),
        }
    }

    #[test]
    fn no_alert_below_warning_threshold() {
        let mut alerts = LimitAlerts::default();
        assert!(alerts
            .check(Agent::ClaudeCode, Some(&snapshot(Some(79.9), Some(10.0))), now())
            .is_empty());
    }

    #[test]
    fn warning_fires_at_threshold_and_only_once() {
        let mut alerts = LimitAlerts::default();
        let fired = alerts.check(Agent::ClaudeCode, Some(&snapshot(Some(80.0), None)), now());
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].level, AlertLevel::Warning);
        assert_eq!(fired[0].title, "⚠️ Claude Code: límite 5h al 80%");
        // Next refresh at the same level stays silent.
        assert!(alerts
            .check(Agent::ClaudeCode, Some(&snapshot(Some(83.0), None)), now())
            .is_empty());
    }

    #[test]
    fn critical_fires_after_warning_escalation() {
        let mut alerts = LimitAlerts::default();
        alerts.check(Agent::ClaudeCode, Some(&snapshot(Some(85.0), None)), now());
        let fired = alerts.check(Agent::ClaudeCode, Some(&snapshot(Some(96.0), None)), now());
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].level, AlertLevel::Critical);
        assert_eq!(fired[0].title, "🔴 Claude Code: límite 5h al 96%");
    }

    #[test]
    fn jump_straight_to_critical_sends_only_critical() {
        let mut alerts = LimitAlerts::default();
        let fired = alerts.check(Agent::ClaudeCode, Some(&snapshot(Some(97.0), None)), now());
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].level, AlertLevel::Critical);
    }

    #[test]
    fn window_reset_rearms_alerts() {
        let mut alerts = LimitAlerts::default();
        alerts.check(Agent::ClaudeCode, Some(&snapshot(Some(96.0), None)), now());
        // Window resets: usage drops to near zero, alerts re-arm.
        alerts.check(Agent::ClaudeCode, Some(&snapshot(Some(2.0), None)), now());
        let fired = alerts.check(Agent::ClaudeCode, Some(&snapshot(Some(81.0), None)), now());
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].level, AlertLevel::Warning);
    }

    #[test]
    fn drop_from_critical_to_warning_stays_silent_but_rearms_critical() {
        let mut alerts = LimitAlerts::default();
        alerts.check(Agent::ClaudeCode, Some(&snapshot(Some(96.0), None)), now());
        assert!(alerts
            .check(Agent::ClaudeCode, Some(&snapshot(Some(85.0), None)), now())
            .is_empty());
        let fired = alerts.check(Agent::ClaudeCode, Some(&snapshot(Some(95.0), None)), now());
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].level, AlertLevel::Critical);
    }

    #[test]
    fn windows_are_tracked_independently() {
        let mut alerts = LimitAlerts::default();
        let fired = alerts.check(Agent::ClaudeCode, Some(&snapshot(Some(82.0), Some(96.0))), now());
        assert_eq!(fired.len(), 2);
        assert_eq!(fired[0].level, AlertLevel::Warning);
        assert!(fired[0].title.contains("5h"));
        assert_eq!(fired[1].level, AlertLevel::Critical);
        assert!(fired[1].title.contains("7d"));
    }

    #[test]
    fn missing_limits_or_windows_produce_nothing() {
        let mut alerts = LimitAlerts::default();
        assert!(alerts.check(Agent::ClaudeCode, None, now()).is_empty());
        assert!(alerts.check(Agent::ClaudeCode, Some(&snapshot(None, None)), now()).is_empty());
    }

    #[test]
    fn body_includes_reset_time_when_known() {
        let mut alerts = LimitAlerts::default();
        let limits = LimitsSnapshot {
            five_hour: Some(RateWindow::new(90.0, Some(now() + chrono::Duration::minutes(72)))),
            seven_day: None,
        };
        let fired = alerts.check(Agent::ClaudeCode, Some(&limits), now());
        assert_eq!(
            fired[0].body,
            "Los tokens de la ventana de 5h se están agotando · resetea en ~1h 12m"
        );
    }
}
