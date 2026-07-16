# Historial de uso y tendencias — Spec de diseño

**Fecha:** 2026-07-16
**Estado:** Propuesto — pendiente de aprobación
**Contexto:** Primera de tres fases acordadas en sesión de brainstorming (2026-07-16) para evolucionar CodeContextAI más allá del snapshot actual: (1) esta spec — historial persistente y tendencias; (2) integración bidireccional con la statusLine de Claude Code (mostrar datos de la app en la terminal, y enriquecer KPIs con datos de sesión — líneas añadidas/eliminadas, tool-calls — que hoy no se leen); (3) modo "contest" **local** (mejores marcas personales, rachas, sin red ni cuentas) construido sobre el historial de esta fase. Las fases 2 y 3 tendrán spec propia una vez aprobada esta.

## Objetivo

Hoy la app no tiene memoria propia: cada refresco recalcula todo desde
`~/.claude/projects/**/*.jsonl` y el sqlite de opencode ("Decisión: sin base
de datos propia" en el spec original de 2026-07-14). Eso significa que no hay
forma de ver tendencias (¿subió mi gasto esta semana frente a la anterior?),
ni una base sobre la que construir alertas de presupuesto o el futuro modo
contest. Esta fase introduce un **historial diario persistente** (primera vez
que la app posee su propia base de datos) y lo expone como tendencias
(sparklines) en el panel existente.

## Alcance

- Snapshot diario por agente: tokens totales, coste, nº de proyectos activos,
  nº de modelos usados — reutilizando los mismos agregados que ya calcula
  `summary::build_summary` para "Hoy", solo que ahora se **congela un
  registro por día** en vez de recalcularlo siempre desde cero.
- Retención: sin límite por ahora (son filas pequeñas — un registro por día
  de uso, no por evento); acotable más adelante si hiciera falta.
- Nueva UI: sparkline de tokens/coste de los últimos 30 días por agente,
  dentro del panel existente (`ui/panel.html`).
- **Fuera de alcance de esta fase:** KPIs de sesión (líneas añadidas/
  eliminadas, tool-calls) — fase 2, trae datos nuevos del transcript.
  Alertas de presupuesto y modo contest — fases posteriores; reutilizan esta
  tabla pero no se implementan aquí.

## Decisiones tomadas (brainstorming)

1. **SQLite propio** vía `rusqlite` (ya es dependencia del proyecto, usada
   hoy para leer el sqlite de opencode) — **sin dependencias nuevas**.
   Archivo `history.db` en `config_dir()` (junto a `preferences.json`), nunca
   en los directorios de origen — la app jamás escribe donde Claude
   Code/opencode leen o escriben.
2. **Grano diario, no evento a evento.** Se guarda un rollup por
   `(fecha_local, agente)`, no los `UsageEvent` crudos — evita duplicar el
   JSONL ya parseado y mantiene la tabla pequeña indefinidamente.
3. **Upsert idempotente del día en curso.** En cada `refresh_all`, tras
   construir `AgentSummary`, se hace un upsert por agente con los totales de
   "hoy" recién calculados. Los días pasados nunca se reescriben (se
   congelan al cambiar la fecha local) — mismo supuesto que ya hace el resto
   de la app: los agentes no reescriben eventos de días anteriores.
4. **Backfill en el primer arranque de esta versión:** si `history.db` no
   existe todavía, además de crear la fila de hoy se recorren los eventos ya
   cargados (todo el histórico disponible en disco, no solo "mes en curso")
   y se generan filas retroactivas por cada día con actividad — así las
   tendencias no empiezan vacías para usuarios que ya llevaban tiempo usando
   la app. El backfill se marca como hecho en una tabla de metadatos para no
   repetirse.
5. **Sparklines, no un gráfico completo.** El panel gana una fila compacta
   de ~30 barras por agente, bajo los stat-tiles existentes; sin librería de
   gráficos — SVG generado a mano en Rust, igual que el resto de
   `dashboard.rs` pre-formatea todo antes de llegar al JS.
6. **Esquema versionado** con una tabla `meta(key, value)` (`schema_version`)
   para poder migrar sin romper instalaciones existentes.

## Arquitectura

| Pieza | Responsabilidad |
|---|---|
| `src-tauri/src/history.rs` (nuevo) | Apertura de `history.db`, migraciones, `upsert_today`, `read_last_n_days`, backfill. |
| `src-tauri/src/main.rs` | `AppState.history: Mutex<rusqlite::Connection>`; `refresh_all` llama a `history::upsert_today(...)` tras construir `sections`. |
| `src-tauri/src/dashboard.rs` | `AgentDashboard` gana `trend: Vec<TrendPoint>` (últimos 30 días; tokens/coste ya formateados salvo el valor crudo usado para la altura de la barra). |
| `ui/panel.html` | Nueva fila de sparkline bajo los stat-tiles, por tab de agente. |

Sin dependencias nuevas de Cargo ni de JS.

### Esquema

```sql
CREATE TABLE daily_usage (
    date TEXT NOT NULL,       -- 'YYYY-MM-DD', calendario LOCAL (igual criterio que "Hoy")
    agent TEXT NOT NULL,      -- 'claude_code' | 'opencode'
    tokens INTEGER NOT NULL,
    cost_usd REAL NOT NULL,
    project_count INTEGER NOT NULL,
    model_count INTEGER NOT NULL,
    PRIMARY KEY (date, agent)
);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
```

### Flujo de datos

1. `refresh_all` ya calcula `Vec<AgentSection>` con `summary.today` por
   agente (tokens, coste, nº de proyectos/modelos de hoy están disponibles
   ahí mismo).
2. Nuevo paso: `history::upsert_today(&conn, &sections, today_local)` — un
   upsert por agente presente en `sections`.
3. `dashboard::build_payload` gana un parámetro con los últimos 30 días por
   agente (leídos de `history::read_last_n_days`) para rellenar `trend`.
4. El panel pinta la sparkline con los mismos datos que ya recibe por el
   evento `dashboard-updated` existente — sin comando Tauri nuevo.

## Manejo de errores

- `history.db` no se puede abrir/crear (permisos, disco lleno): la app sigue
  funcionando exactamente igual que hoy (historial y tendencias ausentes,
  sin sparkline), con log a stderr — mismo principio de "degradar en
  silencio" que ya usan `claude_oauth.rs` y `price_fetch.rs`.
- Backfill parcial (algún día con datos incompletos): esa fila simplemente
  no se crea; el sparkline salta ese punto en vez de pintar un cero falso
  (mismo principio que "nunca 0% falso" en las barras de límite).
- Migración de esquema fallida: la app trata `history.db` como si no
  existiera (mismo fallback) en vez de fallar el arranque.

## Testing

- Rust (unit, con `tempfile`): esquema se crea desde cero; upsert del mismo
  día dos veces no duplica fila; upsert de dos agentes el mismo día crea dos
  filas; `read_last_n_days` respeta orden y límite; backfill genera una fila
  por día con actividad real usando el fixture JSONL multi-día ya existente
  en `tests/fixtures/`.
- `cargo test` completo debe seguir en verde (revisar el nº de tests base
  antes de empezar a implementar).
- Smoke manual: borrar `history.db`, arrancar la app, comprobar el backfill
  contra datos reales, dejarla un par de ciclos de refresco, confirmar que
  el sparkline crece con nuevos puntos.

## Fuera de alcance

- KPIs de sesión (líneas añadidas/eliminadas, tool-calls, duración) — fase 2
  (statusLine bidireccional).
- Alertas de presupuesto basadas en tendencia — reutilizan esta tabla, no se
  implementan aquí.
- Modo contest / mejores marcas personales locales — fase 3, reutiliza esta
  tabla.
- Retención/purga automática de filas antiguas.
- Exportar el historial (CSV/JSON).
