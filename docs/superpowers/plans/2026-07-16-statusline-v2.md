# Status line v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ai-usage-tray --statusline` imprime dos líneas coloreadas con rama git, modelo, contexto de sesión y límites 5h/7d con progress bars, usando la paleta de la app.

**Architecture:** Un módulo nuevo de formateo puro (`statusline_format.rs`, patrón espejo de `menu_format.rs`) contiene toda la lógica ANSI/barras/composición y es 100% testeable sin IO. `statusline.rs` sigue siendo la capa IO: parsea el stdin (structs serde extendidos), obtiene la rama con `git`, lee el snapshot y mapea todo a un struct `StatuslineRender` que le pasa al formateador. La ruta `--statusline` sigue GUI-free y sin red.

**Tech Stack:** Rust (edición del crate existente), serde/serde_json, `std::process::Command` para git. **Sin dependencias nuevas.**

**Spec:** `docs/superpowers/specs/2026-07-16-statusline-v2-design.md`

## Global Constraints

- Ruta `--statusline` GUI-free y sin red (comentario en `statusline.rs:49` — se mantiene). Único proceso externo permitido: `git`.
- Sin crates nuevos en `src-tauri/Cargo.toml`.
- Paleta exacta de `ui/panel.html`: accent `#e8824a` (232,130,74), texto `#e8eaf0` (232,234,240), muted = blanco 45% sobre `#12141a` ≈ (125,126,129), verde `#2ec9a7` (46,201,167), ámbar `#e8a23c` (232,162,60), rojo `#e85a5a` (232,90,90). ANSI truecolor `\x1b[38;2;R;G;Bm`, reset `\x1b[0m`.
- Umbrales de nivel idénticos al panel (`levelClass` en `ui/panel.html:254`): `pct > 85` rojo, `pct >= 60` ámbar, resto verde.
- Barras: glifos `▰▱`, ancho 10, redondeo `(pct/10).round()` (idéntico a `format_limit_line` en `menu_format.rs:42`).
- Campo ausente → segmento omitido; nunca un 0% falso. Línea sin segmentos → no se imprime.
- Copy en español: label `hoy` en minúscula.
- Todos los comandos `cargo` se ejecutan desde `src-tauri/`.
- Commits **sin** trailer `Co-Authored-By` (regla del repo).

---

### Task 1: Módulo `statusline_format` — colores, niveles y barra

**Files:**
- Create: `src-tauri/src/statusline_format.rs`
- Modify: `src-tauri/src/main.rs:17` (añadir `mod statusline_format;` en orden alfabético, entre `mod statusline;` y `mod summary;`)
- Test: mismo archivo (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nada.
- Produces: `pub(crate) const RESET/ACCENT/TEXT/MUTED/GREEN/AMBER/RED: &str`; `fn level_color(pct: f64) -> &'static str`; `fn bar(pct: f64) -> String`; helper de test `pub(crate) fn strip_ansi(s: &str) -> String` (bajo `#[cfg(test)]`). Task 2 añade `render` a este mismo archivo; Task 5 lo consume.

- [ ] **Step 1: Crear el módulo con los tests fallando**

Crear `src-tauri/src/statusline_format.rs`:

```rust
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
```

- [ ] **Step 2: Registrar el módulo en `main.rs`**

En `src-tauri/src/main.rs`, después de `mod statusline;` (línea 17):

```rust
mod statusline;
mod statusline_format;
mod summary;
```

- [ ] **Step 3: Correr los tests y verificar que pasan**

Run: `cargo test statusline_format -- --nocapture`
Expected: 3 passed. (Si `level_color`/`bar` dan warning `dead_code` por no usarse aún fuera de tests, es esperado en esta task; desaparece en Task 2.)

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/statusline_format.rs src-tauri/src/main.rs
git commit -m "feat: módulo statusline_format con paleta ANSI y barras de progreso"
```

---

### Task 2: `render` — composición de las dos líneas

**Files:**
- Modify: `src-tauri/src/statusline_format.rs` (añadir `StatuslineRender`, `gauge`, `render` y tests)

**Interfaces:**
- Consumes: `bar`, `level_color`, constantes de Task 1.
- Produces (Task 5 depende de esto, exacto):

```rust
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

pub fn render(r: &StatuslineRender) -> String
```

- [ ] **Step 1: Escribir los tests fallando**

Añadir dentro de `mod tests` en `statusline_format.rs`:

```rust
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
```

- [ ] **Step 2: Correr los tests y verificar que fallan**

Run: `cargo test statusline_format`
Expected: FAIL — `cannot find struct StatuslineRender` / `cannot find function render`.

- [ ] **Step 3: Implementar `gauge` y `render`**

Añadir en `statusline_format.rs` (antes de `strip_ansi`):

```rust
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
```

- [ ] **Step 4: Correr los tests y verificar que pasan**

Run: `cargo test statusline_format`
Expected: 9 passed (3 de Task 1 + 6 nuevos).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/statusline_format.rs
git commit -m "feat: render de dos líneas del status line con segmentos opcionales"
```

---

### Task 3: Parseo de los campos nuevos del stdin

**Files:**
- Modify: `src-tauri/src/statusline.rs:18-37` (extender `StatuslineInput` y structs anidados)
- Test: mismo archivo, `mod tests`

**Interfaces:**
- Consumes: nada nuevo.
- Produces (Task 5 depende de esto, exacto): campos `workspace: Option<WorkspaceInfo>` (con `current_dir: Option<String>`), `context_window: Option<ContextWindowInfo>` (con `used_percentage: Option<f64>`), `rate_limits: Option<RateLimitsInfo>` (con `five_hour`/`seven_day: Option<RateLimitWindow>`, cada uno con `used_percentage: Option<f64>`) en `StatuslineInput`.

- [ ] **Step 1: Escribir el test fallando**

Añadir en `mod tests` de `statusline.rs`:

```rust
    #[test]
    fn parses_workspace_context_window_and_rate_limits() {
        let json = r#"{
            "session_id": "s1",
            "cwd": "/home/user/p",
            "model": {"display_name": "Sonnet 5"},
            "workspace": {"current_dir": "/home/user/p/sub"},
            "context_window": {"used_percentage": 41.2},
            "rate_limits": {
                "five_hour": {"used_percentage": 62.0, "resets_at": 1789000000},
                "seven_day": {"used_percentage": 34.5}
            }
        }"#;
        let input: StatuslineInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.workspace.unwrap().current_dir.as_deref(), Some("/home/user/p/sub"));
        assert_eq!(input.context_window.unwrap().used_percentage, Some(41.2));
        let limits = input.rate_limits.unwrap();
        assert_eq!(limits.five_hour.unwrap().used_percentage, Some(62.0));
        assert_eq!(limits.seven_day.unwrap().used_percentage, Some(34.5));
    }
```

(`resets_at` va a propósito en el JSON sin campo correspondiente: documenta que los campos desconocidos se toleran.)

- [ ] **Step 2: Correr el test y verificar que falla**

Run: `cargo test statusline::tests::parses_workspace`
Expected: FAIL de compilación — `no field workspace on type StatuslineInput`.

- [ ] **Step 3: Extender los structs**

Reemplazar `StatuslineInput` en `statusline.rs:18-24` y añadir los anidados debajo de `CostInfo`:

```rust
#[derive(Debug, Deserialize, Default)]
struct StatuslineInput {
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<ModelInfo>,
    cost: Option<CostInfo>,
    workspace: Option<WorkspaceInfo>,
    context_window: Option<ContextWindowInfo>,
    rate_limits: Option<RateLimitsInfo>,
}

#[derive(Debug, Deserialize, Default)]
struct WorkspaceInfo {
    current_dir: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ContextWindowInfo {
    used_percentage: Option<f64>,
}

#[derive(Debug, Deserialize, Default)]
struct RateLimitsInfo {
    five_hour: Option<RateLimitWindow>,
    seven_day: Option<RateLimitWindow>,
}

#[derive(Debug, Deserialize, Default)]
struct RateLimitWindow {
    used_percentage: Option<f64>,
}
```

Además, el helper de test existente `input_with_model` (statusline.rs:143) debe terminar con `..Default::default()` para no enumerar los campos nuevos:

```rust
    fn input_with_model(name: &str) -> StatuslineInput {
        StatuslineInput {
            session_id: Some("s1".into()),
            cwd: Some("/home/user/project-a".into()),
            model: Some(ModelInfo { display_name: Some(name.to_string()) }),
            cost: Some(CostInfo {
                total_cost_usd: Some(1.42),
                total_duration_ms: Some(340_000),
                total_lines_added: Some(58),
                total_lines_removed: Some(12),
            }),
            ..Default::default()
        }
    }
```

- [ ] **Step 4: Correr los tests y verificar que pasan**

Run: `cargo test statusline`
Expected: todos los tests de `statusline` y `statusline_format` en verde (los `build_line_*` viejos siguen pasando: los campos nuevos aún no se usan).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/statusline.rs
git commit -m "feat: parsear workspace, context_window y rate_limits del stdin"
```

---

### Task 4: `git_branch` — rama actual del workspace

**Files:**
- Modify: `src-tauri/src/statusline.rs` (nueva función debajo de `persist_session`)
- Test: mismo archivo, `mod tests`

**Interfaces:**
- Consumes: nada.
- Produces (Task 5 depende de esto, exacto): `fn git_branch(dir: Option<&str>) -> Option<String>`.

- [ ] **Step 1: Escribir los tests fallando**

```rust
    #[test]
    fn git_branch_none_outside_a_repo_or_without_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(git_branch(Some(dir.path().to_str().unwrap())), None);
        assert_eq!(git_branch(None), None);
    }

    #[test]
    fn git_branch_reads_current_branch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-b", "feature-x", path])
            .output()
            .unwrap();
        assert!(init.status.success());
        assert_eq!(git_branch(Some(path)), Some("feature-x".to_string()));
    }
```

Nota: `git branch --show-current` resuelve el symref de HEAD, así que funciona en un repo recién creado sin commits. El primer test depende de que `tempdir()` no esté dentro de un repo git (cierto para `/tmp`).

- [ ] **Step 2: Correr los tests y verificar que fallan**

Run: `cargo test statusline::tests::git_branch`
Expected: FAIL de compilación — `cannot find function git_branch`.

- [ ] **Step 3: Implementar**

Debajo de `persist_session` en `statusline.rs`:

```rust
/// `git branch --show-current` en el directorio de la sesión. Salida
/// vacía (HEAD detached) y cualquier fallo (sin git, sin repo, sin dir)
/// significan lo mismo: no hay rama que mostrar.
fn git_branch(dir: Option<&str>) -> Option<String> {
    let dir = dir?;
    let output = std::process::Command::new("git")
        .args(["-C", dir, "branch", "--show-current"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}
```

- [ ] **Step 4: Correr los tests y verificar que pasan**

Run: `cargo test statusline::tests::git_branch`
Expected: 2 passed. (Warning `dead_code` esperado hasta Task 5.)

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/statusline.rs
git commit -m "feat: detectar la rama git actual para el status line"
```

---

### Task 5: Cablear `run` → `build_output` → `render` y migrar los tests viejos

**Files:**
- Modify: `src-tauri/src/statusline.rs` — reemplazar `build_line` (líneas 118-132) por `build_output`, actualizar `run` (líneas 53-61), reescribir los 3 tests `build_line_*` (líneas 157-179)

**Interfaces:**
- Consumes: `StatuslineRender` y `render` (Task 2), campos nuevos (Task 3), `git_branch` (Task 4), `strip_ansi` (Task 1).
- Produces: `fn build_output(input: &StatuslineInput, branch: Option<&str>, snapshot: Option<&Snapshot>) -> String` (interno; nada más lo consume).

- [ ] **Step 1: Reescribir los tests `build_line_*` como tests fallando de `build_output`**

Reemplazar los tres tests `build_line_*` en `statusline.rs` por:

```rust
    #[test]
    fn build_output_prefers_stdin_limits_over_snapshot_title() {
        let mut input = input_with_model("Sonnet 5");
        input.context_window = Some(ContextWindowInfo { used_percentage: Some(41.0) });
        input.rate_limits = Some(RateLimitsInfo {
            five_hour: Some(RateLimitWindow { used_percentage: Some(62.0) }),
            seven_day: Some(RateLimitWindow { used_percentage: Some(34.0) }),
        });
        let snapshot = Snapshot {
            written_at: now(),
            tray_title: Some("5h 99% · 7d 99%".into()),
            today_cost: Some("$4.30".into()),
            refresh_interval_secs: 60,
        };
        let out = build_output(&input, Some("main"), Some(&snapshot));
        assert_eq!(
            crate::statusline_format::strip_ansi(&out),
            "🌿 main · Sonnet 5 · ctx ▰▰▰▰▱▱▱▱▱▱ 41%\n5h ▰▰▰▰▰▰▱▱▱▱ 62% · 7d ▰▰▰▱▱▱▱▱▱▱ 34% · hoy $4.30"
        );
    }

    #[test]
    fn build_output_falls_back_to_snapshot_title_without_stdin_limits() {
        let snapshot = Snapshot {
            written_at: now(),
            tray_title: Some("5h 62% · 7d 34%".into()),
            today_cost: Some("$4.30".into()),
            refresh_interval_secs: 60,
        };
        let out = build_output(&input_with_model("Sonnet 5"), None, Some(&snapshot));
        assert_eq!(
            crate::statusline_format::strip_ansi(&out),
            "Sonnet 5\n5h 62% · 7d 34% · hoy $4.30"
        );
    }

    #[test]
    fn build_output_model_only_without_snapshot() {
        let out = build_output(&input_with_model("Sonnet 5"), None, None);
        assert_eq!(crate::statusline_format::strip_ansi(&out), "Sonnet 5");
    }

    #[test]
    fn build_output_empty_when_nothing_is_known() {
        assert_eq!(build_output(&StatuslineInput::default(), None, None), "");
    }
```

- [ ] **Step 2: Correr los tests y verificar que fallan**

Run: `cargo test statusline::tests::build_output`
Expected: FAIL de compilación — `cannot find function build_output`.

- [ ] **Step 3: Reemplazar `build_line` por `build_output` y actualizar `run`**

Reemplazar la función `build_line` completa (statusline.rs:118-132) por:

```rust
/// Extrae de stdin+snapshot los datos ya listos para pintar y delega en
/// statusline_format::render. El tray_title del snapshot viaja siempre
/// como fallback; render lo ignora si el stdin trajo límites reales.
fn build_output(
    input: &StatuslineInput,
    branch: Option<&str>,
    snapshot: Option<&Snapshot>,
) -> String {
    let limits = input.rate_limits.as_ref();
    let render = StatuslineRender {
        branch,
        model: input.model.as_ref().and_then(|m| m.display_name.as_deref()),
        context_pct: input.context_window.as_ref().and_then(|c| c.used_percentage),
        five_hour_pct: limits
            .and_then(|l| l.five_hour.as_ref())
            .and_then(|w| w.used_percentage),
        seven_day_pct: limits
            .and_then(|l| l.seven_day.as_ref())
            .and_then(|w| w.used_percentage),
        fallback_limits_text: snapshot.and_then(|s| s.tray_title.as_deref()),
        today_cost: snapshot.and_then(|s| s.today_cost.as_deref()),
    };
    crate::statusline_format::render(&render)
}
```

Añadir el import al inicio del archivo (junto a los `use` existentes):

```rust
use crate::statusline_format::StatuslineRender;
```

Actualizar `run` (statusline.rs:53-61):

```rust
pub fn run(history_db_path: &Path, snapshot_path: &Path) {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let input: StatuslineInput = serde_json::from_str(&raw).unwrap_or_default();

    persist_session(history_db_path, &input);

    let dir = input
        .workspace
        .as_ref()
        .and_then(|w| w.current_dir.as_deref())
        .or(input.cwd.as_deref());
    let branch = git_branch(dir);
    println!("{}", build_output(&input, branch.as_deref(), read_snapshot(snapshot_path).as_ref()));
}
```

Nota: `strip_ansi` está bajo `#[cfg(test)]`, disponible para estos tests; `pub(crate)` en las constantes y `StatuslineRender` ya lo permite Task 1/2.

- [ ] **Step 4: Correr la suite completa y verificar que pasa**

Run: `cargo test`
Expected: toda la suite en verde; cero warnings de `dead_code` sobre `git_branch`/`bar`/`render`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/statusline.rs
git commit -m "feat: status line de dos líneas con rama, contexto y límites coloreados"
```

---

### Task 6: Verificación end-to-end y cierre

**Files:**
- Ninguno nuevo (solo verificación; fix inline si algo falla).

- [ ] **Step 1: Compilar el binario real**

Run: `cargo build`
Expected: compila sin errores ni warnings nuevos.

- [ ] **Step 2: Ejercer el flujo real con un stdin de muestra**

Sin `session_id` a propósito, para que `persist_session` no escriba en la DB real del usuario:

```bash
printf '%s' '{"model":{"display_name":"Sonnet 5"},"workspace":{"current_dir":"'"$PWD"'/.."},"context_window":{"used_percentage":41},"rate_limits":{"five_hour":{"used_percentage":62},"seven_day":{"used_percentage":88}}}' | ./target/debug/ai-usage-tray --statusline
```

Expected: dos líneas con colores ANSI visibles en la terminal —
línea 1: `🌿 feat/statusline-v2 · Sonnet 5 · ctx ▰▰▰▰▱▱▱▱▱▱ 41%` (rama en claro, modelo naranja, gauge verde);
línea 2: `5h ▰▰▰▰▰▰▱▱▱▱ 62% · 7d ▰▰▰▰▰▰▰▰▰▱ 88%` (62% ámbar, 88% rojo) — más `hoy $X.XX` solo si el tray está corriendo con snapshot fresco.

- [ ] **Step 3: Probar el caso degradado (stdin vacío)**

Run: `printf '' | ./target/debug/ai-usage-tray --statusline`
Expected: línea vacía (o solo el fallback del snapshot si el tray corre) — sin panic.

- [ ] **Step 4: Suite completa final**

Run: `cargo test`
Expected: verde total.

- [ ] **Step 5: Commit final (solo si hubo fixes en esta task)**

```bash
git add -A && git commit -m "fix: ajustes de verificación end-to-end del status line"
```

---

## Self-review del plan

- **Cobertura del spec:** dos líneas ✔ (Task 2/5), rama git ✔ (Task 4), contexto ✔ (Task 3/5), límites stdin con fallback a snapshot ✔ (Task 2/5), `hoy` del snapshot ✔ (Task 5), paleta y umbrales del panel ✔ (Task 1), glifos ▰▱×10 ✔ (Task 1), omisión sin 0% falso ✔ (Task 2), GUI-free/sin red ✔ (constraints), tests de umbrales 59/60/85/86 ✔ (Task 1).
- **Placeholders:** ninguno; todo step tiene código o comando exacto.
- **Consistencia de tipos:** `StatuslineRender` (Task 2) coincide campo a campo con lo que construye `build_output` (Task 5); `git_branch(Option<&str>) -> Option<String>` coincide entre Task 4 y 5.
