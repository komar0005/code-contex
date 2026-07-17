//! Formateo puro del status line de Claude Code: colores ANSI de la
//! paleta de la app (ui/panel.html), barras ▰▱ y composición de líneas.
//! Sin IO — la capa de datos vive en statusline.rs.

pub(crate) const RESET: &str = "\x1b[0m";
pub(crate) const ACCENT: &str = "\x1b[38;2;232;130;74m"; // #e8824a
pub(crate) const TEXT: &str = "\x1b[38;2;232;234;240m"; // #e8eaf0
pub(crate) const MUTED: &str = "\x1b[38;2;125;126;129m"; // blanco 45% sobre #12141a
pub(crate) const GREEN: &str = "\x1b[38;2;46;201;167m"; // #2ec9a7
pub(crate) const AMBER: &str = "\x1b[38;2;232;162;60m"; // #e8a23c
pub(crate) const RED: &str = "\x1b[38;2;232;90;90m"; // #e85a5a

/// Mismos umbrales que levelClass en ui/panel.html: >85 hot, >=60 warn.
fn level_color(pct: f64) -> &'static str {
    if pct > 85.0 {
        RED
    } else if pct >= 60.0 {
        AMBER
    } else {
        GREEN
    }
}

/// Barra de 10 celdas ▰▱ (mismo glifo y redondeo que format_limit_line
/// del menú del tray); relleno coloreado por nivel, vacío en muted.
fn bar(pct: f64) -> String {
    let filled = (pct.clamp(0.0, 100.0) / 10.0).round() as usize;
    format!(
        "{}{}{MUTED}{}{RESET}",
        level_color(pct),
        "▰".repeat(filled),
        "▱".repeat(10 - filled)
    )
}

/// Datos ya extraídos para pintar el status line. La capa IO
/// (statusline.rs) construye esto; aquí solo se formatea.
pub struct StatuslineRender<'a> {
    pub branch: Option<&'a str>,
    pub model: Option<&'a str>,
    pub context_pct: Option<f64>,
    pub five_hour_pct: Option<f64>,
    pub seven_day_pct: Option<f64>,
    /// tray_title del snapshot ("5h 62% · 7d 34%"); solo se usa si no hay
    /// límites del stdin.
    pub fallback_limits_text: Option<&'a str>,
    pub today_cost: Option<&'a str>,
}

/// "label ▰▰▰▱▱▱▱▱▱▱ 34%" — label en muted, barra y % por nivel.
fn gauge(label: &str, pct: f64) -> String {
    let pct = pct.clamp(0.0, 100.0);
    format!("{MUTED}{label}{RESET} {} {}{pct:.0}%{RESET}", bar(pct), level_color(pct))
}

/// Línea 1: sesión (rama, modelo, contexto). Línea 2: cuenta (límites,
/// gasto de hoy). Segmento ausente → se omite; línea vacía → no se
/// imprime; nada → cadena vacía.
pub fn render(r: &StatuslineRender) -> String {
    let sep = format!("{MUTED} · {RESET}");

    let mut session: Vec<String> = Vec::new();
    if let Some(branch) = r.branch {
        session.push(format!("{TEXT}🌿 {branch}{RESET}"));
    }
    if let Some(model) = r.model {
        session.push(format!("{ACCENT}{model}{RESET}"));
    }
    if let Some(pct) = r.context_pct {
        session.push(gauge("ctx", pct));
    }

    let mut account: Vec<String> = Vec::new();
    if let Some(pct) = r.five_hour_pct {
        account.push(gauge("5h", pct));
    }
    if let Some(pct) = r.seven_day_pct {
        account.push(gauge("7d", pct));
    }
    if account.is_empty() {
        if let Some(text) = r.fallback_limits_text {
            account.push(format!("{TEXT}{text}{RESET}"));
        }
    }
    if let Some(cost) = r.today_cost {
        account.push(format!("{MUTED}hoy{RESET} {TEXT}{cost}{RESET}"));
    }

    [session, account]
        .into_iter()
        .filter(|line| !line.is_empty())
        .map(|line| line.join(&sep))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Quita secuencias ANSI `ESC...m` para que los tests comparen el texto.
#[cfg(test)]
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c2 in chars.by_ref() {
                if c2 == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_color_matches_panel_thresholds() {
        assert_eq!(level_color(0.0), GREEN);
        assert_eq!(level_color(59.9), GREEN);
        assert_eq!(level_color(60.0), AMBER);
        assert_eq!(level_color(85.0), AMBER);
        assert_eq!(level_color(85.1), RED);
        assert_eq!(level_color(100.0), RED);
    }

    #[test]
    fn bar_fills_ten_cells_with_rounding() {
        assert_eq!(strip_ansi(&bar(0.0)), "▱▱▱▱▱▱▱▱▱▱");
        assert_eq!(strip_ansi(&bar(41.0)), "▰▰▰▰▱▱▱▱▱▱");
        assert_eq!(strip_ansi(&bar(62.0)), "▰▰▰▰▰▰▱▱▱▱");
        assert_eq!(strip_ansi(&bar(100.0)), "▰▰▰▰▰▰▰▰▰▰");
        assert_eq!(strip_ansi(&bar(150.0)), "▰▰▰▰▰▰▰▰▰▰"); // clamp
    }

    #[test]
    fn bar_paints_fill_by_level_and_empty_muted() {
        let b = bar(90.0);
        assert!(b.starts_with(RED));
        assert!(b.contains(MUTED));
        assert!(b.ends_with(RESET));
        assert!(bar(30.0).starts_with(GREEN));
        assert!(bar(70.0).starts_with(AMBER));
    }

    fn full() -> StatuslineRender<'static> {
        StatuslineRender {
            branch: Some("main"),
            model: Some("Sonnet 5"),
            context_pct: Some(41.0),
            five_hour_pct: Some(62.0),
            seven_day_pct: Some(34.0),
            fallback_limits_text: Some("5h 99% · 7d 99%"),
            today_cost: Some("$4.30"),
        }
    }

    #[test]
    fn render_full_two_lines_ignoring_fallback_when_stdin_limits_present() {
        assert_eq!(
            strip_ansi(&render(&full())),
            "🌿 main · Sonnet 5 · ctx ▰▰▰▰▱▱▱▱▱▱ 41%\n5h ▰▰▰▰▰▰▱▱▱▱ 62% · 7d ▰▰▰▱▱▱▱▱▱▱ 34% · hoy $4.30"
        );
    }

    #[test]
    fn render_uses_fallback_text_when_no_stdin_limits() {
        let r = StatuslineRender { five_hour_pct: None, seven_day_pct: None, ..full() };
        assert_eq!(
            strip_ansi(&render(&r)),
            "🌿 main · Sonnet 5 · ctx ▰▰▰▰▱▱▱▱▱▱ 41%\n5h 99% · 7d 99% · hoy $4.30"
        );
    }

    #[test]
    fn render_omits_missing_segments_without_leftover_separators() {
        let r = StatuslineRender {
            branch: None,
            context_pct: None,
            five_hour_pct: None,
            seven_day_pct: None,
            fallback_limits_text: None,
            today_cost: None,
            ..full()
        };
        assert_eq!(strip_ansi(&render(&r)), "Sonnet 5");
        assert!(!render(&r).contains('\n'));
    }

    #[test]
    fn render_line2_alone_when_session_line_is_empty() {
        let r = StatuslineRender { branch: None, model: None, context_pct: None, ..full() };
        assert_eq!(
            strip_ansi(&render(&r)),
            "5h ▰▰▰▰▰▰▱▱▱▱ 62% · 7d ▰▰▰▱▱▱▱▱▱▱ 34% · hoy $4.30"
        );
        assert!(!render(&r).contains('\n'));
    }

    #[test]
    fn render_empty_when_nothing_is_known() {
        let r = StatuslineRender {
            branch: None,
            model: None,
            context_pct: None,
            five_hour_pct: None,
            seven_day_pct: None,
            fallback_limits_text: None,
            today_cost: None,
        };
        assert_eq!(render(&r), "");
    }

    #[test]
    fn render_paints_model_accent_and_hot_context_red() {
        let r = StatuslineRender { context_pct: Some(90.0), ..full() };
        let out = render(&r);
        assert!(out.contains(&format!("{ACCENT}Sonnet 5{RESET}")));
        assert!(out.contains(RED));
    }

    #[test]
    fn render_partial_stdin_limits_show_available_lane_and_skip_fallback() {
        let r = StatuslineRender { seven_day_pct: None, ..full() };
        assert_eq!(
            strip_ansi(&render(&r)),
            "🌿 main · Sonnet 5 · ctx ▰▰▰▰▱▱▱▱▱▱ 41%\n5h ▰▰▰▰▰▰▱▱▱▱ 62% · hoy $4.30"
        );
    }

    #[test]
    fn render_clamps_out_of_range_percentages_in_display() {
        let r = StatuslineRender { context_pct: Some(-5.0), ..full() };
        assert!(strip_ansi(&render(&r)).contains("ctx ▱▱▱▱▱▱▱▱▱▱ 0%"));
    }
}
