use crate::menu_format::{
    format_estimated_block_line, format_limit_line, format_stat_line, format_updated_at,
    EMPTY_STATE_MESSAGE,
};
use crate::model::Agent;
use crate::summary::AgentSection;
use chrono::{DateTime, Utc};
use tauri::image::Image;
use tauri::menu::{IconMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::{AppHandle, Wry};

/// Crisp monochrome glyphs for the action items — emoji render
/// inconsistently ("de palo") across DE menu themes.
const ICON_DASHBOARD: &[u8] = include_bytes!("../icons/menu/dashboard.png");
const ICON_REFRESH: &[u8] = include_bytes!("../icons/menu/refresh.png");
const ICON_POWER: &[u8] = include_bytes!("../icons/menu/power.png");

fn menu_icon(bytes: &'static [u8]) -> Option<Image<'static>> {
    Image::from_bytes(bytes).ok()
}

fn agent_label(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "Claude Code",
        Agent::OpenCode => "opencode",
    }
}

/// Appends one agent's basic lines. Flat items only — GTK submenus
/// misrender (inline/overlapped) on some DEs, so the detailed breakdowns
/// live in the webview panel behind "Ver más…".
fn append_agent_section<'a>(
    app: &'a AppHandle,
    builder: MenuBuilder<'a, Wry, AppHandle>,
    section: &AgentSection,
    now: DateTime<Utc>,
) -> tauri::Result<MenuBuilder<'a, Wry, AppHandle>> {
    let summary = &section.summary;
    let mut builder = builder;
    let header = MenuItemBuilder::new(agent_label(summary.agent)).enabled(false).build(app)?;
    builder = builder.item(&header);

    if summary.agent == Agent::ClaudeCode {
        if let Some(limits) = &section.limits {
            for (label, window) in [("5h", &limits.five_hour), ("7d", &limits.seven_day)] {
                if let Some(window) = window {
                    let line = MenuItemBuilder::new(format_limit_line(label, window, now))
                        .enabled(false)
                        .build(app)?;
                    builder = builder.item(&line);
                }
            }
        } else if let Some((_, reset_at)) = &summary.active_5h_block {
            let line = MenuItemBuilder::new(format_estimated_block_line(*reset_at, now))
                .enabled(false)
                .build(app)?;
            builder = builder.item(&line);
        }
    }

    for (label, cost) in [
        ("Hoy", &summary.today),
        ("Mes", &summary.month),
        ("7 días", &summary.last_7_days),
    ] {
        let line = MenuItemBuilder::new(format_stat_line(label, cost))
            .enabled(false)
            .build(app)?;
        builder = builder.item(&line);
    }
    Ok(builder.separator())
}

pub fn build_menu(
    app: &AppHandle,
    sections: &[AgentSection],
    now: DateTime<Utc>,
) -> tauri::Result<Menu<Wry>> {
    let mut builder = MenuBuilder::new(app);

    if sections.is_empty() {
        let empty = MenuItemBuilder::new(EMPTY_STATE_MESSAGE).enabled(false).build(app)?;
        builder = builder.item(&empty).separator();
    } else {
        for section in sections {
            builder = append_agent_section(app, builder, section, now)?;
        }
    }

    let panel_item = IconMenuItemBuilder::with_id("panel", "Ver más…")
        .icon(menu_icon(ICON_DASHBOARD).expect("embedded png"))
        .build(app)?;
    let refresh_item = IconMenuItemBuilder::with_id(
        "refresh",
        format!("Refrescar · {}", format_updated_at(now)),
    )
    .icon(menu_icon(ICON_REFRESH).expect("embedded png"))
    .build(app)?;
    let quit_item = IconMenuItemBuilder::with_id("quit", "Salir")
        .icon(menu_icon(ICON_POWER).expect("embedded png"))
        .build(app)?;

    builder
        .item(&panel_item)
        .item(&refresh_item)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&quit_item)
        .build()
}
