#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cache;
mod claude_oauth;
mod claude_settings;
mod dashboard;
mod history;
mod limits;
mod menu_format;
mod model;
mod parsers;
mod preferences;
mod price_fetch;
mod pricing;
mod provider;
mod records;
mod statusline;
mod summary;
mod tray;
mod windows;

use chrono::Utc;
use dashboard::DashboardPayload;
use model::{Agent, UsageEvent};
use preferences::Preferences;
use pricing::PricingTable;
use provider::{ClaudeProvider, OpenCodeProvider, Provider};
use std::sync::Mutex;
use summary::AgentSection;
use tauri::tray::TrayIcon;
use tauri::{AppHandle, Emitter, Manager};

pub struct AppState {
    pub providers: Mutex<Vec<Box<dyn Provider>>>,
    pub preferences: Mutex<Preferences>,
    pub pricing: Mutex<PricingTable>,
    pub tray: Mutex<Option<TrayIcon>>,
    /// Timestamp of the last *successful* network pricing-table refresh
    /// (i.e. fetch + parse both succeeded, not a fallback). `None` until the
    /// first success.
    pub last_pricing_update: Mutex<Option<chrono::DateTime<Utc>>>,
    /// Timestamp of the last pricing refresh ATTEMPT (successful or not);
    /// backs off failed attempts so an offline machine doesn't re-download
    /// on every 60s cycle.
    pub last_pricing_attempt: Mutex<Option<chrono::DateTime<Utc>>>,
    /// Count of events (summed across both agents' current-month windows)
    /// whose model wasn't found in the pricing table, so their cost
    /// couldn't be calculated. Surfaced read-only in Preferences.
    pub unpriced_count: Mutex<u64>,
    /// Last dashboard payload, served to the panel when it (re)opens.
    /// `None` until the first scan finishes.
    pub dashboard: Mutex<Option<DashboardPayload>>,
    /// App-owned daily history (tokens/cost per day per agent), used for
    /// trend sparklines. `None` when `history.db` couldn't be opened —
    /// trends are then simply absent, same as any other degrade-in-silence
    /// failure in this app.
    pub history: Mutex<Option<rusqlite::Connection>>,
}

fn claude_projects_dir() -> std::path::PathBuf {
    dirs::home_dir().expect("home dir must resolve").join(".claude").join("projects")
}

fn opencode_db_path() -> std::path::PathBuf {
    if let Ok(custom) = std::env::var("OPENCODE_DATA_DIR") {
        return std::path::PathBuf::from(custom).join("opencode.db");
    }
    let primary = dirs::data_local_dir()
        .expect("data-local dir must resolve")
        .join("opencode")
        .join("opencode.db");
    if primary.exists() {
        return primary;
    }
    // opencode resolves its data dir xdg-style even on Windows, where that
    // lands in ~/.local/share rather than %LOCALAPPDATA%.
    dirs::home_dir()
        .map(|h| h.join(".local").join("share").join("opencode").join("opencode.db"))
        .filter(|p| p.exists())
        .unwrap_or(primary)
}

fn config_dir() -> std::path::PathBuf {
    dirs::config_dir().expect("config dir must resolve").join("ai-usage-tray")
}

fn history_db_path() -> std::path::PathBuf {
    config_dir().join("history.db")
}

fn statusline_snapshot_path() -> std::path::PathBuf {
    config_dir().join("statusline_snapshot.json")
}

/// Pricing changes rarely; refresh at most daily, retrying hourly after a
/// failed attempt.
fn should_refresh_pricing(
    last_attempt: Option<chrono::DateTime<Utc>>,
    last_success: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
) -> bool {
    let attempt_due = last_attempt.map_or(true, |t| now - t >= chrono::Duration::hours(1));
    let success_due = last_success.map_or(true, |t| now - t >= chrono::Duration::hours(24));
    attempt_due && success_due
}

pub fn refresh_all(app: &AppHandle) {
    let state = app.state::<AppState>();
    let now = Utc::now();

    let gathered: Vec<(Agent, Vec<UsageEvent>, Option<crate::limits::LimitsSnapshot>)> = {
        let mut providers = state.providers.lock().unwrap();
        providers
            .iter_mut()
            .map(|p| (p.agent(), p.gather_events(), p.fetch_limits(now)))
            .collect()
    };

    let prefs = state.preferences.lock().unwrap().clone();
    let pricing_due = should_refresh_pricing(
        *state.last_pricing_attempt.lock().unwrap(),
        *state.last_pricing_update.lock().unwrap(),
        now,
    );
    if prefs.network_pricing_refresh_enabled && pricing_due {
        *state.last_pricing_attempt.lock().unwrap() = Some(now);
        let current = state.pricing.lock().unwrap().clone();
        let source = price_fetch::HttpPriceSource {
            url: "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json".to_string(),
        };
        let (refreshed, succeeded) = price_fetch::refresh_pricing_table_with_status(&source, current);
        *state.pricing.lock().unwrap() = refreshed;
        if succeeded {
            *state.last_pricing_update.lock().unwrap() = Some(now);
        }
    }
    let pricing = state.pricing.lock().unwrap().clone();

    {
        let history_guard = state.history.lock().unwrap();
        if let Some(conn) = history_guard.as_ref() {
            if !history::is_backfilled(conn) {
                for (agent, events, _) in &gathered {
                    history::backfill(conn, events, *agent, &pricing);
                }
                history::mark_backfilled(conn, now);
            }
        }
    }

    let sections: Vec<AgentSection> = gathered
        .into_iter()
        .filter_map(|(agent, events, limits)| {
            summary::build_summary(agent, &events, &pricing, now)
                .map(|summary| AgentSection { summary, limits })
        })
        .collect();

    let unpriced_count =
        summary::total_unpriced_this_month(sections.iter().map(|s| &s.summary));
    *state.unpriced_count.lock().unwrap() = unpriced_count;

    let (trends, session_stats, records_by_agent): (
        std::collections::HashMap<Agent, Vec<history::TrendPoint>>,
        std::collections::HashMap<Agent, history::SessionStats>,
        std::collections::HashMap<Agent, records::PersonalRecords>,
    ) = {
        let history_guard = state.history.lock().unwrap();
        match history_guard.as_ref() {
            Some(conn) => {
                let today_local_naive = now.with_timezone(&chrono::Local).date_naive();
                let today_local = today_local_naive.to_string();
                for section in &sections {
                    history::upsert_day(
                        conn,
                        &today_local,
                        section.summary.agent,
                        section.summary.today.tokens,
                        section.summary.today.cost,
                        section.summary.by_project.len(),
                        section.summary.by_model.len(),
                    );
                }
                let trends = sections
                    .iter()
                    .map(|s| (s.summary.agent, history::read_last_n_days(conn, s.summary.agent, 30)))
                    .collect();
                // Only present for an agent that has ever sent statusLine
                // data (phase 2) — absence, not a false zero, is how the
                // panel tells "never installed the hook" apart from
                // "installed but nothing happened today".
                let session_stats = sections
                    .iter()
                    .filter(|s| history::has_any_sessions(conn, s.summary.agent))
                    .map(|s| {
                        (s.summary.agent, history::today_session_stats(conn, s.summary.agent, &today_local))
                    })
                    .collect();
                // Streaks/best-day (phase 3) — derived on the fly from the
                // same rows just upserted above, no extra table.
                let records_by_agent = sections
                    .iter()
                    .map(|s| {
                        (s.summary.agent, records::personal_records(conn, s.summary.agent, today_local_naive))
                    })
                    .collect();
                (trends, session_stats, records_by_agent)
            }
            None => (
                std::collections::HashMap::new(),
                std::collections::HashMap::new(),
                std::collections::HashMap::new(),
            ),
        }
    };

    let payload = dashboard::build_payload(&sections, &trends, &session_stats, &records_by_agent, now);
    *state.dashboard.lock().unwrap() = Some(payload.clone());
    let _ = app.emit("dashboard-updated", &payload);

    let title = if prefs.show_tray_metric {
        sections
            .iter()
            .find_map(|s| menu_format::format_tray_title(s.limits.as_ref()))
    } else {
        None
    };
    let claude_today_cost = sections
        .iter()
        .find(|s| s.summary.agent == Agent::ClaudeCode)
        .map(|s| menu_format::format_usd(s.summary.today.cost));
    statusline::write_snapshot(
        &statusline_snapshot_path(),
        title.as_deref(),
        claude_today_cost.as_deref(),
        prefs.refresh_interval_secs,
        now,
    );

    let tray_guard = state.tray.lock().unwrap();
    if let Some(tray_icon) = tray_guard.as_ref() {
        if let Ok(menu) = tray::build_menu(app, &sections, now) {
            let _ = tray_icon.set_menu(Some(menu));
        }
        // Not every Linux DE renders appindicator labels/tooltips;
        // failures are cosmetic, so ignore them.
        let _ = tray_icon.set_title(title.as_deref());
        let _ = tray_icon.set_tooltip(title.as_deref());
    }
}

fn main() {
    // Claude Code invokes this many times per session (debounced per
    // turn); it must never pay the cost of starting Tauri/GTK, so this is
    // checked and handled before the builder is ever touched.
    if std::env::args().any(|arg| arg == "--statusline") {
        statusline::run(&history_db_path(), &statusline_snapshot_path());
        return;
    }

    tauri::Builder::default()
        .manage(AppState {
            providers: Mutex::new(vec![
                Box::new(ClaudeProvider::new(
                    claude_projects_dir(),
                    claude_oauth::default_credentials_path(),
                )) as Box<dyn Provider>,
                Box::new(OpenCodeProvider::new(opencode_db_path())),
            ]),
            preferences: Mutex::new(preferences::load(&config_dir())),
            pricing: Mutex::new(pricing::embedded_pricing_table()),
            tray: Mutex::new(None),
            last_pricing_update: Mutex::new(None),
            last_pricing_attempt: Mutex::new(None),
            unpriced_count: Mutex::new(0),
            dashboard: Mutex::new(None),
            history: Mutex::new(history::open(&history_db_path())),
        })
        .on_window_event(|window, event| {
            // Closing the panel hides it (instant reopen, listeners intact);
            // the app lives in the tray, so no window should ever exit it.
            if window.label() == "panel"
                && matches!(event, tauri::WindowEvent::CloseRequested { .. })
            {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                }
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();

            // Tray-only app: keep it out of the macOS Dock.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            hyprland_register_panel_float_rule();

            // First-run loading state so the tray never blocks or shows
            // stale/misleading data while the initial scan runs.
            let loading_item =
                tauri::menu::MenuItemBuilder::new("Cargando…").enabled(false).build(app)?;
            let menu = tauri::menu::MenuBuilder::new(app).item(&loading_item).build()?;

            let tray_icon = tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("bundled icon").clone())
                // Windows opens tray menus on right click only by default;
                // left click should work too (no-op on Linux/macOS).
                .show_menu_on_left_click(true)
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    "refresh" => {
                        let h = app.clone();
                        tauri::async_runtime::spawn_blocking(move || refresh_all(&h));
                    }
                    "panel" => {
                        let _ = open_panel_window(app);
                    }
                    _ => {}
                })
                .build(app)?;

            *app.state::<AppState>().tray.lock().unwrap() = Some(tray_icon);

            // Dev/testing hook: AI_TRAY_DEV_PANEL=<path> opens the panel
            // whenever that file's mtime changes (touch it to re-open), so
            // smoke tests can exercise it without clicking the tray menu.
            if let Ok(trigger) = std::env::var("AI_TRAY_DEV_PANEL") {
                let panel_handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let path = std::path::PathBuf::from(trigger);
                    let mut last = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                        let mtime =
                            std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                        if mtime != last {
                            last = mtime;
                            let action = std::fs::read_to_string(&path).unwrap_or_default();
                            let action = action.trim();
                            if action.is_empty() || action == "open" {
                                let _ = open_panel_window(&panel_handle);
                            } else {
                                let _ = panel_handle.emit("dev-command", action.to_string());
                            }
                        }
                    }
                });
            }

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
                        let secs = state.preferences.lock().unwrap().refresh_interval_secs;
                        secs.max(10)
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
            save_preferences_cmd,
            get_pricing_status_cmd,
            get_dashboard_cmd,
            refresh_cmd,
            check_statusline_cmd,
            install_statusline_cmd,
            uninstall_statusline_cmd
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
    let h = app.clone();
    tauri::async_runtime::spawn_blocking(move || refresh_all(&h));
    Ok(())
}

#[derive(serde::Serialize)]
struct PricingStatus {
    last_updated: Option<chrono::DateTime<Utc>>,
    unpriced_count: u64,
}

#[tauri::command]
fn get_pricing_status_cmd(app: AppHandle) -> PricingStatus {
    let state = app.state::<AppState>();
    let last_updated = *state.last_pricing_update.lock().unwrap();
    let unpriced_count = *state.unpriced_count.lock().unwrap();
    PricingStatus { last_updated, unpriced_count }
}

#[cfg(test)]
mod pricing_throttle_tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn t0() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap()
    }

    #[test]
    fn refreshes_when_never_attempted() {
        assert!(should_refresh_pricing(None, None, t0()));
    }

    #[test]
    fn skips_when_success_is_fresh() {
        let last = Some(t0() - Duration::hours(2));
        assert!(!should_refresh_pricing(last, last, t0()));
    }

    #[test]
    fn refreshes_when_success_is_a_day_old() {
        let success = Some(t0() - Duration::hours(25));
        assert!(should_refresh_pricing(success, success, t0()));
    }

    #[test]
    fn failed_attempt_backs_off_for_an_hour() {
        // Attempted 30 min ago, never succeeded: wait.
        assert!(!should_refresh_pricing(Some(t0() - Duration::minutes(30)), None, t0()));
        // Attempted 61 min ago, never succeeded: retry.
        assert!(should_refresh_pricing(Some(t0() - Duration::minutes(61)), None, t0()));
    }
}

#[tauri::command]
fn get_dashboard_cmd(app: AppHandle) -> Option<DashboardPayload> {
    app.state::<AppState>().dashboard.lock().unwrap().clone()
}

#[tauri::command]
fn refresh_cmd(app: AppHandle) {
    let h = app.clone();
    tauri::async_runtime::spawn_blocking(move || refresh_all(&h));
}

/// Read-only: never writes `~/.claude/settings.json`. Lets the panel show
/// what's there (nothing / ours / something else) before ever asking the
/// user to confirm an install.
#[tauri::command]
fn check_statusline_cmd() -> Result<claude_settings::StatuslineState, String> {
    let command = claude_settings::our_statusline_command()?;
    claude_settings::check_statusline(&claude_settings::default_settings_path(), &command)
}

/// Only ever called after the panel's own confirmation dialog — this
/// command assumes the user already said yes.
#[tauri::command]
fn install_statusline_cmd(app: AppHandle) -> Result<(), String> {
    let command = claude_settings::our_statusline_command()?;
    claude_settings::install_statusline(&claude_settings::default_settings_path(), &command)?;
    let state = app.state::<AppState>();
    let mut prefs = state.preferences.lock().unwrap().clone();
    prefs.statusline_installed = true;
    prefs.statusline_installed_command = Some(command);
    preferences::save(&config_dir(), &prefs).map_err(|e| e.to_string())?;
    *state.preferences.lock().unwrap() = prefs;
    Ok(())
}

#[tauri::command]
fn uninstall_statusline_cmd(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let expected = state
        .preferences
        .lock()
        .unwrap()
        .statusline_installed_command
        .clone()
        .ok_or_else(|| "esta app no tiene una statusLine instalada".to_string())?;
    claude_settings::uninstall_statusline(&claude_settings::default_settings_path(), &expected)?;
    let mut prefs = state.preferences.lock().unwrap().clone();
    prefs.statusline_installed = false;
    prefs.statusline_installed_command = None;
    preferences::save(&config_dir(), &prefs).map_err(|e| e.to_string())?;
    *state.preferences.lock().unwrap() = prefs;
    Ok(())
}

/// On Hyprland, float the dashboard window (utility-window feel) instead
/// of letting it tile. Cosmetic; a no-op on any other compositor.
/// Hyprland ≥0.53 field syntax: "<field> <value>, match:<prop> <regex>".
fn hyprland_register_panel_float_rule() {
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_err() {
        return;
    }
    let _ = std::process::Command::new("hyprctl")
        .args(["keyword", "windowrule", "float on, match:class ai-usage-tray"])
        .output();
}

/// The full-detail dashboard, opened from the tray menu's "Ver más…".
/// A regular decorated window: the WM places it, the user closes it
/// (CloseRequested is intercepted to hide, keeping reopen instant).
fn open_panel_window(app: &AppHandle) -> tauri::Result<()> {
    let window = match app.get_webview_window("panel") {
        Some(window) => window,
        None => tauri::WebviewWindowBuilder::new(
            app,
            "panel",
            tauri::WebviewUrl::App("panel.html".into()),
        )
        .title("AI Usage")
        .inner_size(400.0, 620.0)
        .resizable(true)
        .visible(false)
        .build()?,
    };
    window.show()?;
    window.set_focus()
}
