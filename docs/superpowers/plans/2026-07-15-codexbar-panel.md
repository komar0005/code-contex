# CodexBar-Style Webview Panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the data-heavy (and GTK-buggy) native tray menu with a codexBar-style webview panel: per-agent tabs, real limit bars with live countdowns, stat tiles, per-project and per-model tables, and inline settings — per spec `docs/superpowers/specs/2026-07-15-codexbar-panel-design.md`.

**Architecture:** A new `dashboard.rs` builds a serializable `DashboardPayload` from the existing `Vec<AgentSection>` at the end of every `refresh_all`; the payload is stored in `AppState` and emitted as a `dashboard-updated` event. A new frameless `panel` window (`ui/panel.html`, vanilla JS) pulls the payload on load (`get_dashboard_cmd`) and re-renders on each event. The native menu shrinks to `📊 Panel · ⟳ Refrescar · Salir` (static — built once). The `preferences` window is deleted; its settings move to an inline view inside the panel.

**Tech Stack:** Rust (Tauri 2), serde/serde_json, chrono. Vanilla HTML/CSS/JS with `withGlobalTauri`. **No new Cargo or JS dependencies.**

## Global Constraints

- All user-visible copy is **Spanish** ("resetea en ~1h 12m", "Sin actividad registrada", "Cargando…").
- **No new dependencies** (Cargo or JS). Panel is self-contained vanilla HTML/CSS/JS.
- All tests run with `cargo test` from `src-tauri/`. Every task ends with the full suite green.
- Numbers are **pre-formatted in Rust** (reusing tested `format_tokens`/`format_usd`); JS receives strings. Raw exceptions: `used_percent` (bar width) and `resets_at` ISO-8601 (live countdown).
- A `None`/absent limit window must **never render as 0%** — the bar is simply not painted.
- The tray title metric (`format_tray_title`, "5h 62% · 7d 34%") keeps working unchanged.
- `save_preferences_cmd` takes the FULL `Preferences` struct — the panel settings view must round-trip unedited fields (including the now-UI-less `budget_*_usd`).
- Commits follow the repo's conventional style (`feat:`, `refactor:`), each ending with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Work happens on branch `feat/codexbar-panel` off `master`.

## File Structure

| File | Responsibility |
|---|---|
| `src-tauri/src/summary.rs` | Add `ModelBreakdown` + `by_model` aggregation to `AgentSummary` |
| `src-tauri/src/dashboard.rs` (new) | `DashboardPayload` types + `build_payload` (the JS contract) |
| `src-tauri/src/main.rs` | `AppState.dashboard`, emit event, `get_dashboard_cmd`, `refresh_cmd`, panel window (open/position/hide-on-blur), remove preferences window |
| `src-tauri/src/tray.rs` | Static minimal 3-item menu |
| `src-tauri/src/menu_format.rs` | Prune dead formatters (menu no longer renders data) |
| `ui/panel.html` (new) | The whole panel UI: tabs, KPIs, settings view, styles, countdown |
| `ui/preferences.html` | **Deleted** |

---

### Task 1: Per-model aggregation (`summary.rs`)

**Files:**
- Modify: `src-tauri/src/summary.rs`

**Interfaces:**
- Consumes: existing `UsageEvent` (`model: String`, `total_tokens()`), `event_cost`, `PricingTable`.
- Produces (used by Task 2):
  - `pub struct ModelBreakdown { pub model: String, pub tokens: u64, pub cost: f64 }`
  - `AgentSummary.by_model: Vec<ModelBreakdown>` — sorted by tokens desc, then model name asc; aggregates ALL events (all time), mirroring `by_project`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src-tauri/src/summary.rs`:

```rust
    #[test]
    fn aggregates_by_model_sorted_by_tokens() {
        let table = embedded_pricing_table();
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![
            event("proj-a", "claude-sonnet-5", now),
            event("proj-a", "claude-sonnet-5", now),
            event("proj-b", "claude-opus-4-8", now),
        ];
        let summary = build_summary(Agent::ClaudeCode, &events, &table, now).unwrap();
        assert_eq!(summary.by_model.len(), 2);
        assert_eq!(summary.by_model[0].model, "claude-sonnet-5");
        assert_eq!(summary.by_model[0].tokens, 2_000_000);
        assert!(summary.by_model[0].cost > 0.0);
        assert_eq!(summary.by_model[1].model, "claude-opus-4-8");
        assert_eq!(summary.by_model[1].tokens, 1_000_000);
    }

    #[test]
    fn by_model_unknown_model_has_zero_cost() {
        let table = embedded_pricing_table();
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![event("proj-a", "unknown-model", now)];
        let summary = build_summary(Agent::ClaudeCode, &events, &table, now).unwrap();
        assert_eq!(summary.by_model[0].cost, 0.0);
        assert_eq!(summary.by_model[0].tokens, 1_000_000);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test by_model`
Expected: compile error — no field `by_model` on `AgentSummary`.

- [ ] **Step 3: Write the implementation**

In `src-tauri/src/summary.rs`:

Add below `ProjectBreakdown`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct ModelBreakdown {
    pub model: String,
    pub tokens: u64,
    pub cost: f64,
}
```

Add `pub by_model: Vec<ModelBreakdown>,` to `AgentSummary` (below `by_project`).

In `build_summary`, below the `by_project` sort, add:

```rust
    let mut by_model_map: HashMap<String, (u64, f64)> = HashMap::new();
    for event in events {
        let entry = by_model_map.entry(event.model.clone()).or_insert((0, 0.0));
        entry.0 += event.total_tokens();
        if let Some(cost) = event_cost(event, pricing) {
            entry.1 += cost;
        }
    }
    let mut by_model: Vec<ModelBreakdown> = by_model_map
        .into_iter()
        .map(|(model, (tokens, cost))| ModelBreakdown { model, tokens, cost })
        .collect();
    by_model.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.model.cmp(&b.model)));
```

And add `by_model,` to the `Some(AgentSummary { ... })` literal (after `by_project`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test`
Expected: all PASS (2 new tests included).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/summary.rs
git commit -m "feat: aggregate usage by model in AgentSummary"
```

---

### Task 2: Dashboard payload (`dashboard.rs`)

**Files:**
- Create: `src-tauri/src/dashboard.rs`
- Modify: `src-tauri/src/main.rs` (add `mod dashboard;` alphabetically after `mod claude_oauth;`)

**Interfaces:**
- Consumes: `summary::{AgentSection, AgentSummary, ModelBreakdown, ProjectBreakdown}`, `limits::LimitsSnapshot`, `model::Agent`, `menu_format::{format_tokens, format_usd, format_reset_in}`, `windows::TokenCost`.
- Produces (used by Tasks 3–4):
  - `pub struct DashboardPayload { pub refreshed_at: String, pub agents: Vec<AgentDashboard> }` (all types `Serialize + Clone`)
  - `pub fn build_payload(sections: &[AgentSection], now: DateTime<Utc>) -> DashboardPayload`
  - JSON field names are the JS contract — see the serialization test, which locks them.

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/dashboard.rs` containing ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{LimitsSnapshot, RateWindow};
    use crate::model::Agent;
    use crate::pricing::embedded_pricing_table;
    use crate::summary::{build_summary, AgentSection};
    use crate::model::UsageEvent;
    use chrono::{TimeZone, Utc};

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap()
    }

    fn section(agent: Agent, limits: Option<LimitsSnapshot>) -> AgentSection {
        let event = UsageEvent {
            agent,
            project: "proj-a".into(),
            model: "claude-sonnet-5".into(),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            timestamp: now(),
        };
        let summary =
            build_summary(agent, &[event], &embedded_pricing_table(), now()).unwrap();
        AgentSection { summary, limits }
    }

    #[test]
    fn builds_agent_with_limits_and_formatted_tiles() {
        let limits = LimitsSnapshot {
            five_hour: Some(RateWindow::new(
                62.0,
                Some(Utc.with_ymd_and_hms(2026, 7, 15, 18, 0, 0).unwrap()),
            )),
            seven_day: Some(RateWindow::new(34.0, None)),
        };
        let payload = build_payload(&[section(Agent::ClaudeCode, Some(limits))], now());
        assert_eq!(payload.agents.len(), 1);
        let agent = &payload.agents[0];
        assert_eq!(agent.id, "claude_code");
        assert_eq!(agent.label, "Claude Code");
        let lim = agent.limits.as_ref().unwrap();
        assert_eq!(lim.five_hour.as_ref().unwrap().used_percent, 62.0);
        assert_eq!(
            lim.five_hour.as_ref().unwrap().resets_at.as_deref(),
            Some("2026-07-15T18:00:00+00:00")
        );
        assert!(lim.seven_day.as_ref().unwrap().resets_at.is_none());
        assert!(agent.estimated_block.is_none());
        assert_eq!(agent.today.tokens, "1.0M tok");
        assert_eq!(agent.month.tokens, "1.0M tok");
        assert_eq!(agent.week.tokens, "1.0M tok");
        assert!(agent.today.cost.starts_with('$'));
        assert_eq!(agent.by_project[0].name, "proj-a");
        assert_eq!(agent.by_model[0].name, "claude-sonnet-5");
    }

    #[test]
    fn no_limits_yields_estimated_block_from_active_5h_block() {
        let payload = build_payload(&[section(Agent::ClaudeCode, None)], now());
        let agent = &payload.agents[0];
        assert!(agent.limits.is_none());
        // The fixture event is at `now`, so a 5h block is active.
        let text = agent.estimated_block.as_deref().unwrap();
        assert!(text.starts_with("resetea en ~"), "got: {text}");
    }

    #[test]
    fn opencode_has_no_limits_and_no_estimated_block() {
        let payload = build_payload(&[section(Agent::OpenCode, None)], now());
        let agent = &payload.agents[0];
        assert_eq!(agent.id, "opencode");
        assert_eq!(agent.label, "opencode");
        assert!(agent.limits.is_none());
        assert!(agent.estimated_block.is_none()); // opencode never shows the Claude heuristic
    }

    #[test]
    fn empty_sections_yield_empty_agents() {
        let payload = build_payload(&[], now());
        assert!(payload.agents.is_empty());
    }

    #[test]
    fn serialization_contract_field_names_are_stable() {
        let limits = LimitsSnapshot {
            five_hour: Some(RateWindow::new(62.0, None)),
            seven_day: None,
        };
        let payload = build_payload(&[section(Agent::ClaudeCode, Some(limits))], now());
        let json = serde_json::to_value(&payload).unwrap();
        assert!(json["refreshed_at"].is_string());
        let agent = &json["agents"][0];
        assert_eq!(agent["id"], "claude_code");
        assert_eq!(agent["limits"]["five_hour"]["used_percent"], 62.0);
        assert!(agent["limits"].get("seven_day").is_none()); // absent, not null
        assert!(agent.get("estimated_block").is_none()); // absent when None
        assert!(agent["today"]["tokens"].is_string());
        assert!(agent["today"]["cost"].is_string());
        assert!(agent["by_project"][0]["name"].is_string());
        assert!(agent["by_model"][0]["tokens"].is_string());
    }

    #[test]
    fn clock_renders_local_hh_mm() {
        use chrono::FixedOffset;
        let t = Utc.with_ymd_and_hms(2026, 7, 15, 18, 32, 5).unwrap();
        let tz = FixedOffset::west_opt(4 * 3600).unwrap();
        assert_eq!(clock_in(t, &tz), "14:32");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test dashboard`
Expected: compile error — `build_payload` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `src-tauri/src/dashboard.rs`:

```rust
use crate::limits::LimitsSnapshot;
use crate::menu_format::{format_reset_in, format_tokens, format_usd};
use crate::model::Agent;
use crate::summary::AgentSection;
use crate::windows::TokenCost;
use chrono::{DateTime, Local, TimeZone, Utc};
use serde::Serialize;

/// The JSON contract with `ui/panel.html`. Numbers arrive pre-formatted
/// (Rust owns formatting; the JS only paints). Raw exceptions: bar widths
/// (`used_percent`) and countdown anchors (`resets_at`, ISO-8601).
#[derive(Debug, Clone, Serialize)]
pub struct DashboardPayload {
    /// Local wall-clock "HH:MM" of this refresh.
    pub refreshed_at: String,
    pub agents: Vec<AgentDashboard>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentDashboard {
    pub id: &'static str,
    pub label: &'static str,
    pub limits: Option<DashboardLimits>,
    /// Heuristic 5h-block reset ("resetea en ~1h 12m"), only when there is
    /// no real limit data AND a local block is active. Never for opencode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_block: Option<String>,
    pub today: StatTile,
    pub month: StatTile,
    pub week: StatTile,
    pub by_project: Vec<BreakdownRow>,
    pub by_model: Vec<BreakdownRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<DashboardWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<DashboardWindow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardWindow {
    pub used_percent: f64,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatTile {
    pub tokens: String,
    pub cost: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BreakdownRow {
    pub name: String,
    pub tokens: String,
    pub cost: String,
}

pub fn build_payload(sections: &[AgentSection], now: DateTime<Utc>) -> DashboardPayload {
    DashboardPayload {
        refreshed_at: clock_in(now, &Local),
        agents: sections.iter().map(|s| build_agent(s, now)).collect(),
    }
}

fn build_agent(section: &AgentSection, now: DateTime<Utc>) -> AgentDashboard {
    let summary = &section.summary;
    let (id, label) = match summary.agent {
        Agent::ClaudeCode => ("claude_code", "Claude Code"),
        Agent::OpenCode => ("opencode", "opencode"),
    };
    let limits = section.limits.as_ref().map(build_limits);
    // The heuristic estimate only makes sense for Claude (5h blocks) and
    // would contradict real data if both were shown.
    let estimated_block = if limits.is_none() && summary.agent == Agent::ClaudeCode {
        summary
            .active_5h_block
            .as_ref()
            .map(|(_, reset_at)| format_reset_in(*reset_at, now))
    } else {
        None
    };
    AgentDashboard {
        id,
        label,
        limits,
        estimated_block,
        today: tile(&summary.today),
        month: tile(&summary.month),
        week: tile(&summary.last_7_days),
        by_project: summary
            .by_project
            .iter()
            .map(|p| BreakdownRow {
                name: p.project.clone(),
                tokens: format_tokens(p.tokens),
                cost: format_usd(p.cost),
            })
            .collect(),
        by_model: summary
            .by_model
            .iter()
            .map(|m| BreakdownRow {
                name: m.model.clone(),
                tokens: format_tokens(m.tokens),
                cost: format_usd(m.cost),
            })
            .collect(),
    }
}

fn build_limits(limits: &LimitsSnapshot) -> DashboardLimits {
    let window = |w: &crate::limits::RateWindow| DashboardWindow {
        used_percent: w.used_percent,
        resets_at: w.resets_at.map(|t| t.to_rfc3339()),
    };
    DashboardLimits {
        five_hour: limits.five_hour.as_ref().map(window),
        seven_day: limits.seven_day.as_ref().map(window),
    }
}

fn tile(cost: &TokenCost) -> StatTile {
    StatTile { tokens: format_tokens(cost.tokens), cost: format_usd(cost.cost) }
}

fn clock_in<Tz: TimeZone>(t: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    t.with_timezone(tz).format("%H:%M").to_string()
}
```

Add `mod dashboard;` to `src-tauri/src/main.rs`'s module list (after `mod claude_oauth;`).

Note: `to_rfc3339()` on a UTC datetime renders `+00:00` (matching the test), and `format_reset_in` currently produces `resetea en ~1h 12m` — the JS wraps it as `Bloque 5h activo · {estimated_block} (estimado)`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test`
Expected: all PASS (6 new dashboard tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/dashboard.rs src-tauri/src/main.rs
git commit -m "feat: dashboard payload with formatted KPIs for the webview panel"
```

---

### Task 3: Rewire main.rs + minimal tray menu; prune dead menu code

**Files:**
- Modify: `src-tauri/src/main.rs`
- Modify: `src-tauri/src/tray.rs` (near-total rewrite, much smaller)
- Modify: `src-tauri/src/menu_format.rs` (delete dead formatters + their tests)
- Modify: `src-tauri/capabilities/default.json` (authorize the `panel` window for IPC)

**Interfaces:**
- Consumes: `dashboard::{build_payload, DashboardPayload}` (Task 2), existing `AgentSection`, `format_tray_title`.
- Produces (used by Task 4, the JS side):
  - Tauri command `get_dashboard_cmd() -> Option<DashboardPayload>`
  - Tauri command `refresh_cmd()` (spawns `refresh_all` off-thread)
  - Event `dashboard-updated` with a `DashboardPayload` body on every refresh
  - Window label `"panel"` serving `panel.html`, hidden on focus loss
  - Existing commands kept for the settings view: `get_preferences_cmd`, `save_preferences_cmd`, `get_pricing_status_cmd`

No new unit tests in this task (window/menu plumbing is not unit-testable here); the gate is `cargo test` staying green after deletions plus `cargo check` with no dead-code warnings.

- [ ] **Step 1: Rewrite `src-tauri/src/tray.rs`**

Replace the ENTIRE file content with:

```rust
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::{AppHandle, Wry};

/// Static minimal menu — all usage data lives in the webview panel now
/// (the native GTK menu misrenders submenus on some DEs and can't be
/// styled). Built once at startup; never rebuilt on refresh.
pub fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let panel_item = MenuItemBuilder::with_id("panel", "📊 Panel").build(app)?;
    let refresh_item = MenuItemBuilder::with_id("refresh", "⟳ Refrescar").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Salir").build(app)?;
    MenuBuilder::new(app)
        .item(&panel_item)
        .item(&refresh_item)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&quit_item)
        .build()
}
```

- [ ] **Step 2: Prune `src-tauri/src/menu_format.rs`**

Delete (functions AND their tests):
- `format_limit_line` and test `format_limit_line_renders_percent_bar`
- `format_budget_line` and tests `format_budget_line_at_zero_fifty_and_over_budget`, `zero_budget_with_spend_shows_full_bar`
- `format_updated_at`, `format_updated_at_in` and test `format_updated_at_renders_local_wall_clock`
- `EMPTY_STATE_MESSAGE` constant

Keep: `format_tokens`, `format_usd`, `format_reset_in`, `format_tray_title` and their tests.
Fix the imports line to `use crate::limits::LimitsSnapshot;` (RateWindow no longer referenced at top level) and drop `Local` from the chrono import if now unused.

- [ ] **Step 3: Rewire `src-tauri/src/main.rs`**

3a. Imports: add `use dashboard::DashboardPayload;` and `use tauri::Emitter;`. Remove `use summary::AgentSection;` only if the compiler flags it (it stays used in `refresh_all`).

3b. `AppState`: add below `unpriced_count`:

```rust
    /// Last dashboard payload, served to the panel when it (re)opens.
    /// `None` until the first scan finishes.
    pub dashboard: Mutex<Option<DashboardPayload>>,
```

and initialize `dashboard: Mutex::new(None),` in the `.manage(AppState { ... })` call.

3c. Replace the end of `refresh_all` — everything from `if let Ok(menu) = tray::build_menu(...)` to the end of the function — with:

```rust
    let payload = dashboard::build_payload(&sections, now);
    *state.dashboard.lock().unwrap() = Some(payload.clone());
    let _ = app.emit("dashboard-updated", &payload);

    if let Some(tray_icon) = state.tray.lock().unwrap().as_ref() {
        let title = if prefs.show_tray_metric {
            sections
                .iter()
                .find_map(|s| menu_format::format_tray_title(s.limits.as_ref()))
        } else {
            None
        };
        // Not every Linux DE renders appindicator labels/tooltips;
        // failures are cosmetic, so ignore them.
        let _ = tray_icon.set_title(title.as_deref());
        let _ = tray_icon.set_tooltip(title.as_deref());
    }
}
```

3d. In `main()`'s `.setup(...)`: delete the `loading_item`/`loading_menu` block (the static menu exists from the start) and build the tray with the real menu:

```rust
            let menu = tray::build_menu(&handle)?;

            let tray_icon = tauri::tray::TrayIconBuilder::new()
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
```

(The `"preferences"` arm disappears.)

3e. Add to the `tauri::Builder` chain (before `.setup(...)`) the blur-hide handler:

```rust
        .on_window_event(|window, event| {
            if window.label() == "panel"
                && matches!(event, tauri::WindowEvent::Focused(false))
            {
                let _ = window.hide();
            }
        })
```

3f. Replace `open_preferences_window` (delete it entirely) with the panel window management:

```rust
const PANEL_WIDTH: f64 = 380.0;
const PANEL_HEIGHT: f64 = 560.0;

fn open_panel_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("panel") {
        position_panel(app, &window);
        window.show()?;
        return window.set_focus();
    }
    let window = tauri::WebviewWindowBuilder::new(
        app,
        "panel",
        tauri::WebviewUrl::App("panel.html".into()),
    )
    .title("AI Usage")
    .inner_size(PANEL_WIDTH, PANEL_HEIGHT)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .visible(false)
    .build()?;
    position_panel(app, &window);
    window.show()?;
    window.set_focus()
}

/// Popover placement: centered under the cursor, clamped to the monitor.
/// Any failure (no cursor position on Wayland, no monitor info) keeps the
/// window manager's default placement — cosmetic, never an error.
fn position_panel(app: &AppHandle, window: &tauri::WebviewWindow) {
    const MARGIN: f64 = 12.0;
    let target = app.cursor_position().ok().and_then(|cursor| {
        let monitor = window
            .current_monitor()
            .ok()
            .flatten()
            .or_else(|| window.primary_monitor().ok().flatten())?;
        let scale = monitor.scale_factor();
        let origin = monitor.position().to_logical::<f64>(scale);
        let size = monitor.size().to_logical::<f64>(scale);
        let cursor = cursor.to_logical::<f64>(scale);
        let x = (cursor.x - PANEL_WIDTH / 2.0)
            .clamp(origin.x + MARGIN, origin.x + size.width - PANEL_WIDTH - MARGIN);
        let y = (cursor.y + MARGIN)
            .clamp(origin.y + MARGIN, origin.y + size.height - PANEL_HEIGHT - MARGIN);
        Some(tauri::LogicalPosition::new(x, y))
    });
    if let Some(position) = target {
        let _ = window.set_position(position);
    }
}
```

3g. Add the two new commands (near the other `#[tauri::command]` fns):

```rust
#[tauri::command]
fn get_dashboard_cmd(app: AppHandle) -> Option<DashboardPayload> {
    app.state::<AppState>().dashboard.lock().unwrap().clone()
}

#[tauri::command]
fn refresh_cmd(app: AppHandle) {
    let h = app.clone();
    tauri::async_runtime::spawn_blocking(move || refresh_all(&h));
}
```

and extend the handler list:

```rust
        .invoke_handler(tauri::generate_handler![
            get_preferences_cmd,
            save_preferences_cmd,
            get_pricing_status_cmd,
            get_dashboard_cmd,
            refresh_cmd
        ])
```

- [ ] **Step 4: Authorize the panel window in `src-tauri/capabilities/default.json`**

Tauri 2 scopes IPC permissions per window; with `"windows": []` every `invoke`/`listen` from the panel is denied. Replace the file content with:

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Panel window IPC (invoke + events)",
  "windows": ["panel"],
  "permissions": [
    "core:default"
  ]
}
```

- [ ] **Step 5: Run tests and check**

Run: `cd src-tauri && cargo test && cargo check`
Expected: all remaining tests PASS (the deleted formatter tests are gone); `cargo check` clean — no dead-code warnings, no leftover references to `EMPTY_STATE_MESSAGE`, `format_budget_line`, `format_updated_at`, `open_preferences_window`, or `AgentSection` in `tray.rs`.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/main.rs src-tauri/src/tray.rs src-tauri/src/menu_format.rs src-tauri/capabilities/default.json
git commit -m "refactor: minimal static tray menu; panel window plumbing with dashboard events"
```

---

### Task 4: The panel UI (`ui/panel.html`)

**Files:**
- Create: `ui/panel.html`

**Interfaces:**
- Consumes: `get_dashboard_cmd`, `refresh_cmd`, `get_preferences_cmd`, `save_preferences_cmd`, `get_pricing_status_cmd`, event `dashboard-updated` (Task 3); payload shape from Task 2's serialization test.
- Produces: the complete user-facing panel.

Design notes baked into the code below (from the spec): fixed dark theme `#12141a`; per-agent glow + accent (Claude `#e8824a`, opencode `#4a9de8`); glass cards (`rgba(255,255,255,0.06)` + `backdrop-filter: blur(24px)` + border `rgba(255,255,255,0.12)`); traffic-light bars (green `#3ecf8e` <60, amber `#e8b84a` 60–85, red `#e85a5a` >85); live 1s countdown from `resets_at`; tabs always show BOTH known agents (an agent absent from the payload renders "Sin actividad registrada"); settings view round-trips the full `Preferences` object.

- [ ] **Step 1: Create `ui/panel.html`**

Create the file with exactly this content:

```html
<!-- ui/panel.html — codexBar-style dashboard panel -->
<!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8" />
  <style>
    :root {
      --bg: #12141a;
      --text: #e8eaf0;
      --muted: rgba(255, 255, 255, 0.45);
      --card: rgba(255, 255, 255, 0.06);
      --card-border: rgba(255, 255, 255, 0.12);
      --track: rgba(255, 255, 255, 0.08);
      --green: #3ecf8e;
      --amber: #e8b84a;
      --red: #e85a5a;
      --accent: #e8824a; /* overridden per active tab */
    }
    * { margin: 0; padding: 0; box-sizing: border-box; }
    html, body { background: transparent; }
    body {
      font-family: system-ui, -apple-system, sans-serif;
      font-size: 13px;
      color: var(--text);
      -webkit-user-select: none;
      user-select: none;
    }
    .panel {
      position: relative;
      width: 380px;
      height: 560px;
      border-radius: 16px;
      background: var(--bg);
      border: 1px solid rgba(255, 255, 255, 0.10);
      overflow: hidden;
      display: flex;
      flex-direction: column;
    }
    /* Glassmorphism: the glow lives INSIDE the panel so backdrop-filter on
       the cards has something to blur — no compositor dependency. */
    .panel::before, .panel::after {
      content: "";
      position: absolute;
      width: 340px;
      height: 340px;
      border-radius: 50%;
      filter: blur(70px);
      pointer-events: none;
      transition: background 0.4s ease;
    }
    .panel::before { top: -120px; left: -80px; background: color-mix(in srgb, var(--accent) 16%, transparent); }
    .panel::after { bottom: -140px; right: -100px; background: color-mix(in srgb, var(--accent) 9%, transparent); }
    .panel.accent-claude { --accent: #e8824a; }
    .panel.accent-opencode { --accent: #4a9de8; }

    .view { position: relative; z-index: 1; display: flex; flex-direction: column; flex: 1; min-height: 0; }
    [hidden] { display: none !important; }

    nav { display: flex; gap: 4px; padding: 14px 16px 0; }
    nav button {
      flex: 1;
      background: none;
      border: none;
      color: var(--muted);
      font: inherit;
      font-weight: 600;
      padding: 8px 0 10px;
      cursor: pointer;
      border-bottom: 2px solid transparent;
      transition: color 0.2s, border-color 0.2s;
    }
    nav button.active { color: var(--text); border-bottom-color: var(--accent); }

    main { flex: 1; overflow-y: auto; padding: 12px 16px; display: flex; flex-direction: column; gap: 10px; }
    main::-webkit-scrollbar { width: 6px; }
    main::-webkit-scrollbar-thumb { background: var(--track); border-radius: 3px; }

    .card {
      background: var(--card);
      -webkit-backdrop-filter: blur(24px);
      backdrop-filter: blur(24px);
      border: 1px solid var(--card-border);
      border-radius: 12px;
      padding: 12px;
      box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
    }
    .card h4 {
      font-size: 10px;
      font-weight: 600;
      text-transform: uppercase;
      letter-spacing: 0.08em;
      color: var(--muted);
      margin-bottom: 8px;
    }

    .limit-row { margin-bottom: 10px; }
    .limit-row:last-child { margin-bottom: 0; }
    .limit-head { display: flex; justify-content: space-between; margin-bottom: 5px; }
    .limit-head .pct { font-weight: 700; font-variant-numeric: tabular-nums; }
    .bar { height: 7px; border-radius: 4px; background: var(--track); overflow: hidden; }
    .bar-fill { height: 100%; border-radius: 4px; transition: width 0.5s ease; }
    .level-ok .bar-fill { background: linear-gradient(90deg, #2ea873, var(--green)); }
    .level-ok .pct, .level-ok .countdown { color: var(--green); }
    .level-warn .bar-fill { background: linear-gradient(90deg, #c99a33, var(--amber)); }
    .level-warn .pct, .level-warn .countdown { color: var(--amber); }
    .level-hot .bar-fill { background: linear-gradient(90deg, #c74848, var(--red)); }
    .level-hot .pct, .level-hot .countdown { color: var(--red); }
    .countdown { font-size: 11px; margin-top: 4px; font-variant-numeric: tabular-nums; }
    .estimated { color: var(--amber); font-size: 12px; opacity: 0.85; }

    .tiles { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 8px; }
    .tile { text-align: center; padding: 10px 6px; }
    .tile .label { font-size: 9px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); }
    .tile .tokens { font-size: 15px; font-weight: 700; color: var(--accent); margin: 4px 0 2px; font-variant-numeric: tabular-nums; }
    .tile .cost { font-size: 11px; color: var(--muted); font-variant-numeric: tabular-nums; }

    table { width: 100%; border-collapse: collapse; font-variant-numeric: tabular-nums; }
    td { padding: 4px 0; font-size: 12px; }
    td.name { color: var(--text); max-width: 170px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    td.num { text-align: right; color: var(--muted); padding-left: 10px; white-space: nowrap; }

    .empty { text-align: center; color: var(--muted); padding: 40px 20px; }

    footer {
      display: flex;
      justify-content: space-between;
      align-items: center;
      padding: 10px 16px;
      border-top: 1px solid rgba(255, 255, 255, 0.08);
    }
    footer button {
      background: none;
      border: none;
      color: var(--muted);
      font: inherit;
      cursor: pointer;
      padding: 4px 8px;
      border-radius: 6px;
      transition: color 0.2s, background 0.2s;
    }
    footer button:hover { color: var(--text); background: var(--card); }

    /* Settings view */
    #view-settings main { gap: 12px; }
    .field { display: flex; justify-content: space-between; align-items: center; gap: 10px; }
    .field span { color: var(--text); }
    .field input[type="number"] {
      width: 80px;
      background: var(--card);
      border: 1px solid var(--card-border);
      border-radius: 6px;
      color: var(--text);
      padding: 5px 8px;
      font: inherit;
    }
    .field input[type="checkbox"] { accent-color: var(--accent); width: 16px; height: 16px; }
    .status-line { display: flex; justify-content: space-between; font-size: 12px; color: var(--muted); padding: 3px 0; }
    .status-line b { color: var(--text); font-weight: 600; }
    .save-btn {
      width: 100%;
      background: color-mix(in srgb, var(--accent) 22%, transparent);
      border: 1px solid color-mix(in srgb, var(--accent) 45%, transparent);
      color: var(--text);
      font: inherit;
      font-weight: 600;
      padding: 9px;
      border-radius: 8px;
      cursor: pointer;
    }
    .save-btn:hover { background: color-mix(in srgb, var(--accent) 32%, transparent); }
    #saveStatus { text-align: center; font-size: 12px; color: var(--green); height: 15px; }
    #saveStatus.error { color: var(--red); }
    .back-btn { align-self: flex-start; }
  </style>
</head>
<body>
  <div class="panel accent-claude" id="panelRoot">
    <div class="view" id="view-dashboard">
      <nav id="tabs"></nav>
      <main id="content"><div class="empty">Cargando…</div></main>
      <footer>
        <button id="refreshBtn" title="Refrescar">⟳ <span id="refreshedAt">—</span></button>
        <button id="settingsBtn" title="Ajustes">⚙</button>
      </footer>
    </div>

    <div class="view" id="view-settings" hidden>
      <nav><button class="active" style="cursor: default;">Ajustes</button></nav>
      <main>
        <div class="card">
          <h4>Refresco</h4>
          <label class="field"><span>Intervalo (segundos)</span>
            <input id="refreshInterval" type="number" min="10" step="10" /></label>
        </div>
        <div class="card">
          <h4>Opciones</h4>
          <label class="field"><span>Actualizar precios por red</span>
            <input id="networkPricing" type="checkbox" /></label>
          <label class="field" style="margin-top: 8px;"><span>Mostrar % junto al icono</span>
            <input id="showTrayMetric" type="checkbox" /></label>
        </div>
        <div class="card">
          <h4>Estado de precios</h4>
          <div class="status-line"><span>Última actualización</span><b id="lastPricingUpdate">—</b></div>
          <div class="status-line"><span>Eventos sin costo este mes</span><b id="unpricedCount">—</b></div>
        </div>
        <button class="save-btn" id="saveBtn">Guardar</button>
        <div id="saveStatus"></div>
      </main>
      <footer>
        <button class="back-btn" id="backBtn">← Volver</button>
      </footer>
    </div>
  </div>

  <script>
    const { invoke } = window.__TAURI__.core;
    const { listen } = window.__TAURI__.event;

    // Tabs are fixed: an agent missing from the payload still gets a tab
    // with an empty state, per spec.
    const KNOWN_AGENTS = [
      { id: "claude_code", label: "Claude Code", accent: "accent-claude" },
      { id: "opencode", label: "opencode", accent: "accent-opencode" },
    ];

    let payload = null;
    let activeTab = KNOWN_AGENTS[0].id;
    let currentPrefs = null; // full Preferences object — round-trips unedited fields

    const $ = (id) => document.getElementById(id);

    function levelClass(pct) {
      if (pct > 85) return "level-hot";
      if (pct >= 60) return "level-warn";
      return "level-ok";
    }

    function countdownText(resetsAtIso) {
      const ms = new Date(resetsAtIso) - Date.now();
      if (ms <= 0) return "reseteado";
      const totalMin = Math.floor(ms / 60000);
      const d = Math.floor(totalMin / 1440);
      const h = Math.floor((totalMin % 1440) / 60);
      const m = totalMin % 60;
      if (d > 0) return `resetea en ${d}d ${h}h`;
      if (h > 0) return `resetea en ${h}h ${m}m`;
      return `resetea en ${m}m`;
    }

    function renderTabs() {
      $("tabs").innerHTML = "";
      for (const agent of KNOWN_AGENTS) {
        const btn = document.createElement("button");
        btn.textContent = agent.label;
        btn.classList.toggle("active", agent.id === activeTab);
        btn.addEventListener("click", () => {
          activeTab = agent.id;
          $("panelRoot").className = `panel ${agent.accent}`;
          render();
        });
        $("tabs").appendChild(btn);
      }
    }

    function limitRow(label, win) {
      const level = levelClass(win.used_percent);
      const countdown = win.resets_at
        ? `<div class="countdown" data-resets-at="${win.resets_at}">${countdownText(win.resets_at)}</div>`
        : "";
      return `<div class="limit-row ${level}">
        <div class="limit-head"><span>${label}</span><span class="pct">${Math.round(win.used_percent)}%</span></div>
        <div class="bar"><div class="bar-fill" style="width: ${win.used_percent}%"></div></div>
        ${countdown}
      </div>`;
    }

    function breakdownCard(title, rows) {
      if (!rows.length) return "";
      const body = rows
        .map(
          (r) =>
            `<tr><td class="name" title="${r.name}">${r.name}</td>` +
            `<td class="num">${r.tokens}</td><td class="num">${r.cost}</td></tr>`
        )
        .join("");
      return `<div class="card"><h4>${title}</h4><table>${body}</table></div>`;
    }

    function render() {
      renderTabs();
      const content = $("content");
      if (!payload) {
        content.innerHTML = `<div class="empty">Cargando…</div>`;
        return;
      }
      $("refreshedAt").textContent = payload.refreshed_at;
      const agent = payload.agents.find((a) => a.id === activeTab);
      if (!agent) {
        content.innerHTML = `<div class="empty">Sin actividad registrada</div>`;
        return;
      }

      let html = "";
      if (agent.limits) {
        let rows = "";
        if (agent.limits.five_hour) rows += limitRow("Límite 5h", agent.limits.five_hour);
        if (agent.limits.seven_day) rows += limitRow("Límite 7d", agent.limits.seven_day);
        if (rows) html += `<div class="card">${rows}</div>`;
      } else if (agent.estimated_block) {
        html += `<div class="card"><div class="estimated">Bloque 5h activo · ${agent.estimated_block} (estimado)</div></div>`;
      }

      html += `<div class="tiles">
        ${tile("Hoy", agent.today)}${tile("Mes", agent.month)}${tile("7 días", agent.week)}
      </div>`;
      html += breakdownCard("Por proyecto", agent.by_project);
      html += breakdownCard("Por modelo", agent.by_model);
      content.innerHTML = html;
    }

    function tile(label, stat) {
      return `<div class="card tile"><div class="label">${label}</div>` +
        `<div class="tokens">${stat.tokens}</div><div class="cost">${stat.cost}</div></div>`;
    }

    // Live countdown: only touch the countdown nodes, no full re-render.
    setInterval(() => {
      for (const el of document.querySelectorAll("[data-resets-at]")) {
        el.textContent = countdownText(el.dataset.resetsAt);
      }
    }, 1000);

    // ---- Settings view ----
    async function openSettings() {
      currentPrefs = await invoke("get_preferences_cmd");
      $("refreshInterval").value = currentPrefs.refresh_interval_secs;
      $("networkPricing").checked = currentPrefs.network_pricing_refresh_enabled;
      $("showTrayMetric").checked = currentPrefs.show_tray_metric;
      const status = await invoke("get_pricing_status_cmd");
      $("lastPricingUpdate").textContent = status.last_updated
        ? status.last_updated.slice(0, 16).replace("T", " ")
        : "nunca";
      $("unpricedCount").textContent = status.unpriced_count;
      $("view-dashboard").hidden = true;
      $("view-settings").hidden = false;
    }

    function closeSettings() {
      $("view-settings").hidden = true;
      $("view-dashboard").hidden = false;
    }

    async function saveSettings() {
      const status = $("saveStatus");
      try {
        const prefs = {
          ...currentPrefs, // preserves budget_*_usd and any future fields
          refresh_interval_secs: Number($("refreshInterval").value),
          network_pricing_refresh_enabled: $("networkPricing").checked,
          show_tray_metric: $("showTrayMetric").checked,
        };
        await invoke("save_preferences_cmd", { prefs });
        currentPrefs = prefs;
        status.classList.remove("error");
        status.textContent = "Guardado.";
      } catch (e) {
        status.classList.add("error");
        status.textContent = "Error al guardar.";
      }
      setTimeout(() => (status.textContent = ""), 2000);
    }

    // ---- Wiring ----
    $("refreshBtn").addEventListener("click", () => invoke("refresh_cmd"));
    $("settingsBtn").addEventListener("click", openSettings);
    $("backBtn").addEventListener("click", closeSettings);
    $("saveBtn").addEventListener("click", saveSettings);

    listen("dashboard-updated", (event) => {
      payload = event.payload;
      render();
    });

    (async () => {
      payload = await invoke("get_dashboard_cmd");
      render();
    })();
  </script>
</body>
</html>
```

- [ ] **Step 2: Verify it builds and serves**

Run: `cd src-tauri && cargo check`
Expected: clean (the HTML is static frontend content; no Rust change). Sanity-check the JS/HTML by opening the raw file: `python3 -c "import html.parser; p = html.parser.HTMLParser(); p.feed(open('../ui/panel.html').read()); print('html ok')"`.

- [ ] **Step 3: Commit**

```bash
git add ui/panel.html
git commit -m "feat: codexBar-style dashboard panel with tabs, glass cards, and live countdowns"
```

---

### Task 5: Delete the preferences window; final verification

**Files:**
- Delete: `ui/preferences.html`

**Interfaces:**
- Consumes: nothing new. Task 3 already removed `open_preferences_window` and the menu item; Task 4's settings view replaced the UI.
- Produces: final state.

- [ ] **Step 1: Delete the file**

```bash
git rm ui/preferences.html
```

- [ ] **Step 2: Confirm nothing references it**

Run: `grep -rn "preferences.html" src-tauri/src ui/ || echo "no references"`
Expected: `no references`.

- [ ] **Step 3: Full suite**

Run: `cd src-tauri && cargo test && cargo check`
Expected: all PASS, clean check.

- [ ] **Step 4: Commit**

```bash
git commit -m "refactor: remove preferences window; settings live in the panel"
```

- [ ] **Step 5: Manual smoke test (needs the desktop)**

Run `cd src-tauri && cargo tauri dev` and verify:
1. Tray menu shows only `📊 Panel / ⟳ Refrescar / — / Salir`.
2. `📊 Panel` opens the frameless panel near the cursor; rounded corners; Claude glow orange.
3. Claude tab: 5h/7d bars with traffic-light colors matching claude.ai usage; countdowns tick every second.
4. Stat tiles Hoy/Mes/7 días; "Por proyecto" and "Por modelo" tables populated.
5. opencode tab: accent turns blue; tiles/tables render (or "Sin actividad registrada").
6. Click outside the panel → it hides; reopen from the menu → instant (window reused).
7. ⚙ → settings view; change the interval, Guardar, "Guardado." appears; ← Volver.
8. ⟳ in the panel triggers a refresh (footer clock updates).
9. Rename `~/.claude/.credentials.json` → ⟳ → Claude tab shows "Bloque 5h activo · resetea en ~… (estimado)" in amber; restore the file.
10. Tray title still shows "5h N% · 7d M%".

## Out of Scope (per spec)

- Real compositor blur behind the window (KWin/Wayland).
- More providers as tabs (seam ready: `KNOWN_AGENTS` + `Provider` trait).
- Budget alerts (`budget_*_usd` stay dormant in `preferences.json`).
- Historical sparklines.
