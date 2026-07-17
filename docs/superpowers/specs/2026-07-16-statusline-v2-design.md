# Status line v2 — diseño

**Fecha:** 2026-07-16 · **Estado:** aprobado

## Objetivo

Mejorar la salida de `ai-usage-tray --statusline` (hoy texto plano
`5h 62% · 7d 34% · hoy $4.30 · claude-sonnet-5`) con rama git, progress
bars, contexto de la sesión y los colores de la app.

## Resultado visual

```
🌿 main · Sonnet 5 · ctx ▰▰▰▰▱▱▱▱▱▱ 41%
5h ▰▰▰▰▰▰▱▱▱▱ 62% · 7d ▰▰▰▱▱▱▱▱▱▱ 34% · hoy $4.30
```

- **Línea 1 — sesión:** rama git, modelo, contexto de la ventana con barra.
- **Línea 2 — cuenta:** límites 5h/7d con barra, gasto de hoy (agregado del tray).

Claude Code soporta salida multilínea (cada línea impresa es una fila) y
colores ANSI en el status line.

## Fuentes de datos

| Segmento | Fuente | Ausencia |
|---|---|---|
| Rama | `git branch --show-current` ejecutado en `workspace.current_dir` del stdin | no repo / detached → se omite |
| Modelo | `model.display_name` (stdin) | se omite |
| Contexto | `context_window.used_percentage` (stdin, pre-calculado por Claude Code) | `null` al inicio de sesión → se omite |
| 5h / 7d | `rate_limits.five_hour/seven_day.used_percentage` (stdin) | si el stdin no los trae (Claude Code viejo), fallback al `tray_title` del snapshot como texto plano |
| hoy $ | `today_cost` del snapshot del tray | tray cerrado/stale → se omite |

Cambio clave: los límites 5h/7d se leen del stdin de Claude Code (más
frescos, no requieren el tray corriendo); el snapshot queda como fallback
de límites y única fuente del gasto "hoy".

## Colores (paleta de la app, ANSI truecolor 24-bit)

Tomados de `ui/panel.html`:

- Modelo → accent Claude `#e8824a`.
- Rama → texto `#e8eaf0`.
- Separadores `·` y labels `5h` / `7d` / `ctx` / `hoy` → muted (gris).
- Barras y porcentajes → umbrales del panel (`levelClass`):
  verde `#2ec9a7` < 60 %, ámbar `#e8a23c` 60–85 %, rojo `#e85a5a` > 85 %.
- Glifos de barra `▰▱`, ancho 10 — idénticos a `format_limit_line` del
  menú del tray.

## Arquitectura

- Todo vive en `src-tauri/src/statusline.rs`:
  - `StatuslineInput` se extiende con `workspace { current_dir }`,
    `context_window { used_percentage }` y
    `rate_limits { five_hour, seven_day } { used_percentage }`.
  - `build_line` pasa a componer dos líneas (devuelve el bloque completo);
    la rama git se le inyecta como parámetro para mantenerlo testeable.
  - `persist_session` no cambia.
- La ruta `--statusline` sigue GUI-free y sin red; el único proceso
  externo es `git` (local y rápido).

## Manejo de errores

- Campo ausente → segmento omitido; nunca se muestra un 0 % falso
  (misma regla que `LimitsSnapshot`).
- Línea sin segmentos → no se imprime.
- stdin malformado → `unwrap_or_default()` como hoy (línea mínima o vacía).

## Testing

Tests unitarios de `build_line`: composición de segmentos, mapeo de
niveles de color en los umbrales (59/60/85/86), fallbacks (sin rama, sin
contexto, sin límites en stdin → snapshot, sin snapshot), y salida vacía
cuando no se sabe nada.

## Fuera de alcance

- "resetea en ~1h" en el statusline (vive en el menú del tray).
- Estado git sucio/staged.
- Configuración de segmentos por preferencias.
