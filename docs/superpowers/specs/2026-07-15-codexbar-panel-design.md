# Panel webview estilo codexBar — Spec de diseño

**Fecha:** 2026-07-15
**Estado:** aprobado en sesión de brainstorming
**Sustituye a:** `2026-07-15-menu-redesign-design.md` (pulido del menú nativo — descartado: el menú nativo GTK renderiza el submenú de proyectos inline/solapado en el escritorio del autor y no da el control visual que se busca).
**Prerequisito:** ya implementado el plan `2026-07-15-real-claude-limits-and-provider-abstraction.md` (límites reales, `Provider` trait, `LimitsSnapshot`).

## Objetivo

Reemplazar el menú nativo cargado de datos (y con bugs de renderizado GTK) por un **panel webview** al estilo codexBar: tabs por agente, barras de límite real con countdown vivo, stat-tiles de uso, tablas por proyecto y por modelo, y ajustes inline. El menú nativo queda mínimo.

## Decisiones tomadas (brainstorming)

1. **Panel webview** como UI principal; menú nativo reducido a `📊 Panel · ⟳ Refrescar · Salir`.
2. **Tabs separados por agente** (Claude Code / opencode), no tiles apilados.
3. Apertura por **ítem de menú** (en Linux/appindicator el click del icono no llega a la app); ventana popover **sin bordes, always-on-top, se oculta al perder foco**, fuera de la barra de tareas.
4. KPIs por tab: **barras de límite real + countdown**, **Hoy/Mes/7 días** (tokens y costo), **tabla por proyecto**, **desglose por modelo** (agregación nueva).
5. Estilo: **oscuro fijo con glassmorphism interno y color funcional** (ver sección Estilo).
6. **La ventana de Preferencias se elimina**; sus ajustes útiles pasan a una vista inline del panel. Los presupuestos `budget_*_usd` pierden toda UI (persisten en `preferences.json` para futuras alertas).
7. Datos por **pull + push**: comando al abrir + evento en cada refresh.

## Arquitectura

| Pieza | Responsabilidad |
|---|---|
| `ui/panel.html` (nuevo) | Panel completo: tabs, KPIs, vista de ajustes. Vanilla HTML/CSS/JS, sin frameworks ni assets externos. |
| `src-tauri/src/dashboard.rs` (nuevo) | `DashboardPayload` serializable y su construcción desde `Vec<AgentSection>` + estado de precios. |
| `src-tauri/src/summary.rs` | Nueva agregación `by_model: Vec<ModelBreakdown>` en `AgentSummary` (espejo de `by_project`). |
| `src-tauri/src/main.rs` | Ventana `panel` (crear/mostrar/ocultar, posición junto al cursor), comando `get_dashboard_cmd`, evento `dashboard-updated`, menú de bandeja mínimo, eliminación de la ventana `preferences`. |
| `src-tauri/src/tray.rs` | `build_menu` queda mínimo (3 ítems); desaparece `append_agent_section` y todo el render de datos. |
| `ui/preferences.html` | **Se elimina.** |

Sin dependencias nuevas de Cargo. El título del tray (`format_tray_title`, "5h 62% · 7d 34%") **se mantiene** tal cual.

### Ventana del panel

- ~380×560 px, `decorations: false`, `always_on_top: true`, `skip_taskbar: true`, `resizable: false`, `transparent: true` (esquinas redondeadas vía CSS; si el compositor no soporta transparencia, el fondo del body cubre y queda rectangular — aceptable).
- Ítem de menú `panel`: si la ventana existe se muestra/enfoca; si no, se crea. Posición: junto al cursor vía `AppHandle::cursor_position()`, ajustada para no salirse de pantalla; fallback esquina superior derecha.
- Se oculta (no se destruye) en `blur` (evento `WindowEvent::Focused(false)`), para reabrir instantáneo.

### Flujo de datos

1. `refresh_all` construye `Vec<AgentSection>` (ya existente) → `dashboard::build_payload(&sections, pricing_status, now)` → guarda en `AppState.dashboard: Mutex<Option<DashboardPayload>>` → `app.emit("dashboard-updated", &payload)`.
2. El panel al cargar llama `get_dashboard_cmd` (devuelve `Option<DashboardPayload>`; `None` ⇒ "Cargando…") y se suscribe al evento para actualizarse en vivo.
3. El botón ⟳ del panel invoca el mismo `refresh` que el ítem del menú (`spawn_blocking(refresh_all)`).
4. La vista de ajustes reutiliza `get_preferences_cmd` / `save_preferences_cmd` / `get_pricing_status_cmd` existentes.

### `DashboardPayload` (contrato JS)

Regla: **números formateados en Rust** (reutiliza `format_tokens`/`format_usd` ya testeados); el JS solo pinta. Excepciones crudas: `used_percent` (ancho de barra) y `resets_at` ISO-8601 (countdown vivo).

```jsonc
{
  "refreshed_at": "14:32",              // hora local pre-formateada
  "agents": [
    {
      "id": "claude_code",              // clave estable para el tab
      "label": "Claude Code",
      "limits": {                        // null => sin límites reales
        "five_hour": { "used_percent": 62.0, "resets_at": "2026-07-16T03:40:00Z" },  // ventanas null se omiten
        "seven_day": { "used_percent": 34.0, "resets_at": "2026-07-16T10:00:00Z" }
      },
      "estimated_block": "resetea en ~1h 12m",  // solo si limits == null y hay bloque 5h activo local
      "today":  { "tokens": "1.2M tok", "cost": "$4.30" },
      "month":  { "tokens": "48M tok",  "cost": "$132.10" },
      "week":   { "tokens": "9.8M tok", "cost": "$22.40" },
      "by_project": [ { "name": "ai-context", "tokens": "1.2M tok", "cost": "$3.20" } ],
      "by_model":   [ { "name": "claude-sonnet-5", "tokens": "38M tok", "cost": "$98.20" } ]
    }
  ]
}
```

Un agente sin eventos locales no aparece en `agents` (regla existente de `build_summary`); su tab se pinta igualmente con "Sin actividad registrada". Si `agents` está vacío, el panel muestra el estado vacío global.

## UI del panel

```
┌──────────────────────────────────┐
│  [ Claude Code ]  [ opencode ]   │  tabs, underline animado color-agente
│ ┌──────────── glass ───────────┐ │
│ │ Límite 5h              62%   │ │  barra gradiente semáforo
│ │ ▐████████▌░░░░  resetea 1h12m│ │  countdown tick 1s desde resets_at
│ │ Límite 7d              34%   │ │
│ │ ▐████▌░░░░░░░░  resetea 4d8h │ │
│ └──────────────────────────────┘ │
│ ┌ HOY ────┐┌ MES ───┐┌ 7 DÍAS ─┐ │
│ │ 1.2M tok││ 48M tok││ 9.8M tok│ │  stat tiles glass, número grande
│ │ $4.30   ││ $132.10││ $22.40  │ │  en color de acento
│ └─────────┘└────────┘└─────────┘ │
│ ┌ Por proyecto ────────────────┐ │
│ │ ai-context    1.2M    $3.20  │ │
│ └──────────────────────────────┘ │
│ ┌ Por modelo ──────────────────┐ │
│ │ sonnet-5      38M     $98.20 │ │
│ └──────────────────────────────┘ │
│  ⟳ 14:32                    ⚙   │  footer: refresh + ajustes inline
└──────────────────────────────────┘
```

- **Countdown vivo:** `setInterval` 1 s recalcula "resetea en Xh Ym" desde `resets_at`; al llegar a 0 muestra "reseteado" hasta el próximo refresh.
- **Fallback sin límites reales:** la tarjeta de límites se sustituye por una línea `Bloque 5h activo · {estimated_block} (estimado)` en ámbar tenue; si tampoco hay bloque, la tarjeta no se pinta.
- **Vista de ajustes (⚙):** reemplaza el contenido del panel con transición (mismo tamaño): intervalo de refresco (número), actualizar precios por red (toggle), mostrar % junto al icono (toggle), estado de precios (última actualización + eventos sin costo, solo lectura), botón Guardar y ← volver.

## Estilo (glassmorphism + color funcional)

- Base: fondo `#12141a` con dos **glows radiales** grandes y difusos que cambian con el tab activo (naranja `#e8824a` para Claude, azul `#4a9de8` para opencode, ~8% alpha).
- **Tarjetas glass:** `background: rgba(255,255,255,0.06)`, `backdrop-filter: blur(24px)`, `border: 1px solid rgba(255,255,255,0.12)`, radio 12 px, sombra suave. (El blur actúa sobre los glows internos del panel — no se depende del blur del compositor.)
- **Barras de límite semáforo:** gradiente de relleno según `used_percent` — verde `#3ecf8e` (<60), ámbar `#e8b84a` (60–85), rojo `#e85a5a` (>85); pista `rgba(255,255,255,0.08)`; el texto del countdown hereda el color.
- **Stat tiles:** número grande (18–20 px, semibold) en el color de acento del agente; etiqueta 10 px uppercase gris `rgba(255,255,255,0.45)`.
- Tipografía del sistema (`system-ui`); tabular-nums para cifras.
- Esquinas de la ventana redondeadas (16 px) sobre fondo transparente.

## Manejo de errores

- Sin payload aún (primer scan en curso): "Cargando…".
- `limits: null`: fallback estimado descrito arriba; nunca barras con datos inventados ni 0% falso.
- Ventana `five_hour`/`seven_day` ausente dentro de `limits`: esa barra no se pinta (nunca 0%).
- Agente sin datos: tab presente con "Sin actividad registrada".
- Fallo al guardar ajustes: mensaje de error inline en la vista de ajustes.
- Transparencia no soportada por el compositor: el panel queda rectangular con fondo sólido — sin código extra.

## Testing

- **Rust (unit):** `by_model` en `summary.rs` (agrupa, ordena por tokens, casos sin precio); `dashboard::build_payload` (payload completo, `limits: null` + bloque estimado, agente ausente, `agents` vacío); serialización JSON del payload (nombres de campo estables — contrato con el JS).
- **JS:** sin framework de test; el JS es capa tonta de pintado. Verificación manual.
- **Smoke manual final:** panel con token vivo (barras semáforo + countdown que decrementa), credenciales renombradas (línea estimada), tab opencode, vista de ajustes (guardar y comprobar efecto), ocultar al perder foco, reabrir desde el menú, menú nativo de 3 ítems.

## Iteración 3 (2026-07-15, smoke tests automatizados — VIGENTE)

El popover (auto-hide al perder foco) resultó irreparablemente frágil en Wayland/Hyprland: el foco rebota con el menú GTK y con focus-follows-mouse, y un toplevel Wayland no puede ni posicionarse ni leer el cursor. Decisión final:

1. **El menú nativo vuelve a mostrar la info básica** como líneas planas (sin submenús, que es lo que GTK renderiza mal): por agente, barras de límite `▰▱` con reset (`format_limit_line`), o línea estimada de bloque 5h, y `Hoy/Mes/7 días  Xtok · $Y` (`format_stat_line`). Ítems finales: `📊 Ver más…`, `⟳ Refrescar · HH:MM`, `Salir`. El menú se reconstruye en cada refresh.
2. **El panel webview es la vista de detalle** detrás de `Ver más…`: ventana **normal decorada** (400×620, redimensionable), sin auto-hide; el botón cerrar del WM la **oculta** (CloseRequested interceptado) para reabrir instantáneo. Tabs, barras semáforo, tiles, tablas por proyecto/modelo y ajustes inline como estaban.
3. En Hyprland se registra `windowrule float on, match:class ai-usage-tray` (sintaxis ≥0.53) para que la ventana flote; cosmético, no-op en otros compositores.
4. Fixes de la pasada de smoke tests: nombres de proyecto acortados al último segmento del path (`short_project_name`), timeout del download LiteLLM 10s→60s (cuerpo de 1.6 MB), fecha de precios con `toLocaleString`, tokens de tiles sin sufijo " tok".
5. Hook de dev para tests (`AI_TRAY_DEV_PANEL=<archivo>`): tocar el archivo abre el panel; escribir `settings`/`back`/`tab:<id>`/`refresh` lo controla vía evento `dev-command`. Inerte sin la env var.

## Iteración 2 (2026-07-15, tras smoke test manual — SUPERSEDED por iteración 3)

Hallazgos: (a) el panel se ocultaba solo al abrirse — el foco rebota al cerrarse el menú GTK y dispara `Focused(false)` inmediato; (b) se quiere el panel como *menú* de la barra: compacto al abrir, con el detalle como opción.

Decisiones:

1. **Una ventana, dos modos.** Compacto **340×330** por defecto: tabs + barras de límite (o línea estimada) + stat tiles + footer. Botón **"Más ▸"** en el footer expande la MISMA ventana a **380×560** mostrando además las tablas por proyecto/modelo; en expandido el footer muestra **"‹ Menos"** y el engranaje ⚙ (ajustes solo en modo expandido). El modo se recuerda mientras la app corre (variable JS; siempre reabre en el último modo usado).
2. **Comando `set_panel_mode_cmd(expanded: bool)`**: redimensiona la ventana y re-clampa la posición a la pantalla con el nuevo tamaño. El CSS pasa a `width: 100vw; height: 100vh` para seguir el tamaño de la ventana.
3. **Periodo de gracia anti-cierre:** `AppState.panel_shown_at` se marca en cada apertura; un `Focused(false)` dentro de los **500 ms** siguientes NO oculta — re-asierta el foco (`set_focus`). Pasada la gracia, blur = ocultar (comportamiento popover intacto).

## Fuera de alcance

- Blur real del compositor (KWin/Wayland) tras la ventana.
- Más proveedores en el panel (el `Provider` trait es la costura; los tabs se generan desde `agents[]`).
- Alertas de presupuesto (los `budget_*_usd` quedan dormidos en `preferences.json`).
- Gráficas históricas (sparklines) — idea futura.
