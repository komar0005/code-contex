use chrono::{DateTime, Utc};

/// One provider rate-limit window as reported by the provider's API —
/// REAL account data, unlike the local heuristics in `windows.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct RateWindow {
    /// Percent of the window consumed, clamped to [0, 100].
    pub used_percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

impl RateWindow {
    pub fn new(used_percent: f64, resets_at: Option<DateTime<Utc>>) -> Self {
        Self { used_percent: used_percent.clamp(0.0, 100.0), resets_at }
    }
}

/// Real limit windows for one agent. A `None` window means the provider
/// did not report that lane (e.g. `five_hour: null` = no active session);
/// it must never be rendered as 0%.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitsSnapshot {
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_window_clamps_percent() {
        assert_eq!(RateWindow::new(150.0, None).used_percent, 100.0);
        assert_eq!(RateWindow::new(-5.0, None).used_percent, 0.0);
        assert_eq!(RateWindow::new(62.0, None).used_percent, 62.0);
    }
}
