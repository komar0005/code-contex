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
}
