use crate::limits::{LimitsSnapshot, RateWindow};
use crate::windows::TokenCost;
use chrono::{DateTime, Local, TimeZone, Utc};

pub const EMPTY_STATE_MESSAGE: &str = "No se detectó actividad de agentes IA en este equipo";

pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M tok", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        let k = tokens as f64 / 1_000.0;
        if k >= 999.95 {
            format!("{:.1}M tok", tokens as f64 / 1_000_000.0)
        } else {
            format!("{k:.1}K tok")
        }
    } else {
        format!("{tokens} tok")
    }
}

pub fn format_usd(amount: f64) -> String {
    format!("${amount:.2}")
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

/// One flat menu line for a REAL limit window. The percent sits right
/// after the label — the line's tail is what a narrow menu clips, so the
/// headline number must never live there.
/// e.g. "5h 62%  ▰▰▰▰▰▰▱▱▱▱ · resetea en ~1h 12m"
pub fn format_limit_line(label: &str, window: &RateWindow, now: DateTime<Utc>) -> String {
    let filled = (window.used_percent / 10.0).round() as usize;
    let bar: String = "▰".repeat(filled) + &"▱".repeat(10 - filled);
    let mut line = format!("{label} {:.0}%  {bar}", window.used_percent);
    if let Some(resets_at) = window.resets_at {
        line.push_str(&format!(" · {}", format_reset_in(resets_at, now)));
    }
    line
}

/// Fallback line when no real limit data is available but the local
/// heuristic sees an active 5h block.
pub fn format_estimated_block_line(reset_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    format!("Bloque 5h activo · {} (estimado)", format_reset_in(reset_at, now))
}

/// Flat usage line: "Hoy  1.2M tok · $4.30".
pub fn format_stat_line(label: &str, cost: &TokenCost) -> String {
    format!("{label}  {} · {}", format_tokens(cost.tokens), format_usd(cost.cost))
}

/// Local wall-clock "HH:MM" of the last refresh, shown inside the
/// Refrescar item (the menu is a static snapshot; relative ages go stale).
pub fn format_updated_at_in<Tz: TimeZone>(refreshed_at: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    refreshed_at.with_timezone(tz).format("%H:%M").to_string()
}

pub fn format_updated_at(refreshed_at: DateTime<Utc>) -> String {
    format_updated_at_in(refreshed_at, &Local)
}

/// Compact headline for the tray icon itself, e.g. "5h 62% · 7d 34%".
/// `None` when no real limit data is available — the tray then shows only
/// the icon, never a stale or estimated number.
pub fn format_tray_title(limits: Option<&LimitsSnapshot>) -> Option<String> {
    let limits = limits?;
    let mut parts: Vec<String> = Vec::new();
    if let Some(window) = &limits.five_hour {
        parts.push(format!("5h {:.0}%", window.used_percent));
    }
    if let Some(window) = &limits.seven_day {
        parts.push(format!("7d {:.0}%", window.used_percent));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
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
    fn format_tokens_rolls_over_to_m_just_under_a_million() {
        assert_eq!(format_tokens(999_999), "1.0M tok");
        assert_eq!(format_tokens(999_949), "999.9K tok");
    }

    #[test]
    fn format_tray_title_composes_available_windows() {
        use crate::limits::{LimitsSnapshot, RateWindow};
        let full = LimitsSnapshot {
            five_hour: Some(RateWindow::new(62.0, None)),
            seven_day: Some(RateWindow::new(34.0, None)),
        };
        assert_eq!(format_tray_title(Some(&full)), Some("5h 62% · 7d 34%".to_string()));

        let weekly_only = LimitsSnapshot {
            five_hour: None,
            seven_day: Some(RateWindow::new(34.0, None)),
        };
        assert_eq!(format_tray_title(Some(&weekly_only)), Some("7d 34%".to_string()));

        assert_eq!(format_tray_title(None), None);
        let empty = LimitsSnapshot { five_hour: None, seven_day: None };
        assert_eq!(format_tray_title(Some(&empty)), None);
    }

    #[test]
    fn format_limit_line_percent_first_so_menu_edge_never_clips_it() {
        use crate::limits::RateWindow;
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let resets = now + Duration::minutes(72);
        assert_eq!(
            format_limit_line("5h", &RateWindow::new(62.0, Some(resets)), now),
            "5h 62%  ▰▰▰▰▰▰▱▱▱▱ · resetea en ~1h 12m"
        );
        assert_eq!(
            format_limit_line("7d", &RateWindow::new(0.0, None), now),
            "7d 0%  ▱▱▱▱▱▱▱▱▱▱"
        );
        assert_eq!(
            format_limit_line("5h", &RateWindow::new(100.0, None), now),
            "5h 100%  ▰▰▰▰▰▰▰▰▰▰"
        );
    }

    #[test]
    fn format_estimated_block_line_wraps_reset() {
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        assert_eq!(
            format_estimated_block_line(now + Duration::minutes(30), now),
            "Bloque 5h activo · resetea en ~30m (estimado)"
        );
    }

    #[test]
    fn format_stat_line_joins_tokens_and_cost() {
        let cost = crate::windows::TokenCost { tokens: 1_200_000, cost: 4.3, unpriced_count: 0 };
        assert_eq!(format_stat_line("Hoy", &cost), "Hoy  1.2M tok · $4.30");
    }

    #[test]
    fn format_updated_at_renders_local_hh_mm() {
        use chrono::FixedOffset;
        let refreshed = Utc.with_ymd_and_hms(2026, 7, 14, 18, 32, 5).unwrap();
        let tz = FixedOffset::west_opt(4 * 3600).unwrap();
        assert_eq!(format_updated_at_in(refreshed, &tz), "14:32");
    }

    #[test]
    fn format_reset_in_hours_and_minutes() {
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let reset_at = now + Duration::minutes(72);
        assert_eq!(format_reset_in(reset_at, now), "resetea en ~1h 12m");
        let reset_soon = now + Duration::minutes(9);
        assert_eq!(format_reset_in(reset_soon, now), "resetea en ~9m");
    }
}
