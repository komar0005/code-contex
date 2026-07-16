# Rediseño del menú de bandeja — Spec de diseño

**Fecha:** 2026-07-15
**Estado:** SUSTITUIDO por `2026-07-15-codexbar-panel-design.md` — el menú nativo GTK resultó bugueado (submenú inline/solapado) y se decidió pivotar a un panel webview estilo codexBar. No implementar este spec.
**Prerequisito:** se implementa DESPUÉS del plan `docs/superpowers/plans/2026-07-15-real-claude-limits-and-provider-abstraction.md` (rediseña las líneas de límite que ese plan introduce).

## Objetivo

El menú nativo actual es visualmente pobre: barras pesadas `[█████░░░░░]`, columnas desalineadas por el hack de espacios (los menús usan fuente proporcional), dos familias de barras que parecen lo mismo sin serlo (límite real vs presupuesto personal), y líneas muertas. Este rediseño lo limpia **sin salir del menú nativo**.

## Decisiones tomadas (brainstorming)

1. **Solo pulir el menú nativo** — descartado el panel webview estilo codexbar (queda como idea futura).
2. **Barra fina `▰▱`** de 10 segmentos para los límites reales.
3. **El presupuesto personal desaparece del menú.** Las preferencias `budget_*_usd` se conservan en el archivo y en la ventana de Preferencias (uso futuro: alertas), pero el menú ya no las pinta.

## Aspecto final

### Con límites reales (caso normal)

```
Claude Code
▰▰▰▰▰▰▱▱▱▱  5h 62% · resetea 1h 12m
▰▰▰▱▱▱▱▱▱▱  7d 34%
Hoy 1.2M tok · $4.30
Mes 48M tok · $132.10
Ver por proyecto ▸
─────────────────────
opencode
Hoy 300K tok · $0.85
Mes 12M tok · $28.40
Ver por proyecto ▸
─────────────────────
⟳ Refrescar · 14:32
⚙ Preferencias
Salir
```

### Sin límites reales (fallback heurístico)

Nunca se dibuja una barra con datos inventados. Si no hay `LimitsSnapshot`, la sección de Claude muestra:

```
Claude Code
Bloque 5h activo · resetea en ~1h 12m (estimado)
Hoy 1.2M tok · $4.30
Mes 48M tok · $132.10
Ver por proyecto ▸
```

La línea "Bloque 5h activo…" solo aparece si hay un bloque 5h activo (heurística local existente). Si tampoco lo hay, la sección va directa a `Hoy`.

### Sin datos de ningún agente

Sin cambios: mensaje vacío actual (`EMPTY_STATE_MESSAGE`).

## Formatos exactos por línea

| Línea | Formato | Notas |
|---|---|---|
| Límite | `▰▰▰▰▰▰▱▱▱▱  5h 62% · resetea 1h 12m` | Barra 10 seg. (`▰` lleno, `▱` vacío, `round(pct/10)`), etiqueta `5h`/`7d`, `{:.0}%`. El sufijo `· resetea …` solo si `resets_at` presente; reutiliza la lógica de `format_reset_in` sin el prefijo "resetea en ~" actual → pasa a `resetea 1h 12m` / `resetea 42m`. |
| Bloque estimado | `Bloque 5h activo · resetea en ~1h 12m (estimado)` | Solo en fallback sin `LimitsSnapshot`. |
| Hoy / Mes | `Hoy 1.2M tok · $4.30` / `Mes 48M tok · $132.10` | Muere el padding con espacios; separador `·`. |
| Proyecto (submenú) | `ai-context · 1.2M tok · $3.20` | Mismo separador. |
| Refrescar | `⟳ Refrescar · 14:32` | Absorbe la línea "Actualizado a las 14:32" (que se elimina); hora local del último refresh. |

## Componentes afectados (solo capa de presentación)

- `src-tauri/src/menu_format.rs` —
  - `format_limit_line` pasa a una sola línea (barra `▰▱` + % + reset opcional); recibe `now` para el countdown.
  - Nueva `format_estimated_block_line(reset_at, now)` para el fallback.
  - `format_updated_at*` se reemplaza por `format_refresh_item(refreshed_at)` → `⟳ Refrescar · HH:MM`.
  - `format_budget_line` se elimina (junto con sus tests) al quedar sin llamadores.
  - Líneas Hoy/Mes/proyecto: separador `·` (cambio en los `format!` de `tray.rs`, no hace falta helper nuevo).
- `src-tauri/src/tray.rs` — `append_agent_section` compone el nuevo orden; el ítem `refresh` usa el texto combinado (sigue siendo clicable, id `"refresh"`); desaparece el ítem "Actualizado a las".
- **Sin cambios en:** `provider.rs`, `claude_oauth.rs`, `limits.rs`, `summary.rs`, `preferences.rs`, `ui/preferences.html` (los presupuestos siguen configurables), título del tray (`format_tray_title` de plan anterior).

## Manejo de errores

Sin estados nuevos: los degradados existentes (sin límites → fallback estimado; sin datos → empty state) se re-visten con el nuevo formato. Cualquier fallo del fetch de límites ya resulta en `limits: None` aguas arriba.

## Testing

- Unit tests sobre los formatters puros de `menu_format.rs` (mismo patrón actual): límite con y sin `resets_at`, 0%/62%/100%, línea estimada, línea de refresh con timezone fijada.
- Smoke manual: menú con token vivo (barras + reset), con credenciales renombradas (fallback estimado), sin datos (empty state).
