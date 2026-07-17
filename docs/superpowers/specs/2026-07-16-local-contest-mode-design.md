# Modo contest local (récords personales) — Spec de diseño

**Fecha:** 2026-07-16
**Estado:** Propuesto — pendiente de aprobación
**Contexto:** Tercera y última fase de la sesión de brainstorming (2026-07-16). Fase 1
(`2026-07-16-usage-history-trends-design.md`, implementada) dejó `daily_usage` (rollup diario
por agente). Fase 2 (`2026-07-16-statusline-integration-design.md`, implementada) agregó
`sessions` (líneas añadidas/eliminadas por sesión, solo Claude Code, opt-in). Esta fase no
agrega ninguna tabla nueva — son consultas derivadas sobre las dos que ya existen. Decisión ya
tomada en el brainstorming original: el "contest" es **local, contra vos mismo** (mejores
marcas propias, rachas) — nunca un leaderboard entre personas ni nada que requiera red o
cuentas, para no romper el principio de "sin servidor propio" del proyecto.

## Objetivo

Convertir el historial que ya se junta en algo que se siente como un pequeño juego: cada
agente (Claude Code / opencode) muestra su **racha de días activos** y su **mejor día**, calculados
puramente a partir de `daily_usage` — funciona para los dos agentes por igual, sin depender de
la statusLine de la fase 2. Cuando además hay datos de `sessions` (Claude Code con la
statusLine instalada), se suma el **mejor día por líneas añadidas**.

## Alcance

- Nuevo módulo `records.rs`: calcula racha actual, racha más larga y mejor día (tokens) a
  partir de `daily_usage`; mejor día por líneas a partir de `sessions` cuando existe.
- Nueva tarjeta "Récords personales" en cada tab del panel (Claude Code y opencode).
- **Fuera de alcance de esta fase:**
  - Cualquier comparación entre distintas personas/equipos (el brainstorming ya descartó esto
    a favor de "local-only": ver decisión previa en la fase 2).
  - Sistema de insignias/logros ("¡7 días seguidos!") — se limita a los dos números concretos
    (racha, mejor día); un sistema de logros es una idea futura, no esta fase.
  - Notificaciones/toasts al batir un récord — requeriría una API de notificaciones que hoy no
    es dependencia del proyecto.
  - Alertas de presupuesto — quedaron fuera desde la fase 1, siguen sin implementarse.

## Decisiones tomadas (brainstorming)

1. **Sin tablas nuevas.** Todo se deriva por consulta de `daily_usage` y `sessions`
   (`history.db`, ya existente) en el momento del refresh — mismo principio que el resto de la
   app: nada se duplica, se computa desde la fuente ya persistida.
2. **Racha = días consecutivos con `tokens > 0`** en `daily_usage` para ese agente. Se guarda
   como fecha local (`YYYY-MM-DD`), igual criterio que el resto de la app.
3. **Gracia de un día para la racha actual.** Si el último día activo fue HOY o AYER, la racha
   sigue contando (evita que se vea "rota" a las 00:01 antes de que el usuario haya hecho nada
   hoy). Si el último día activo fue hace 2 días o más, la racha actual es 0 — aunque la racha
   más larga histórica se siga mostrando igual.
4. **Mejor día = el día con más tokens** en `daily_usage` para ese agente (empate: el más
   reciente). Se muestra como "3.2M tok · 10 jul".
5. **Mejor día por líneas** (`sessions.lines_added` agregado por día) solo aparece si el agente
   tiene al menos una fila en `sessions` — mismo patrón de "ausente, nunca cero falso" que ya
   usan `lines_today`/`sessions_today` de la fase 2. Hoy solo puede pasar para Claude Code.
6. **Se calcula en cada refresh, no se cachea.** Son un `GROUP BY`/scan sobre una tabla que en
   la práctica tiene, como mucho, unos pocos miles de filas (un registro por día de uso real) —
   sin impacto de rendimiento medible; no justifica una tabla de "récords" separada.
7. **Disponible para ambos agentes.** A diferencia de la fase 2 (statusLine, exclusiva de
   Claude Code), racha y mejor día por tokens no dependen de ningún hook — opencode los muestra
   igual.

## Arquitectura

| Pieza | Responsabilidad |
|---|---|
| `src-tauri/src/records.rs` (nuevo) | `personal_records(conn, agent, today) -> PersonalRecords`: racha actual/más larga, mejor día por tokens, mejor día por líneas (`Option`). |
| `src-tauri/src/main.rs` | `refresh_all` calcula `records_by_agent: HashMap<Agent, PersonalRecords>` (mismo patrón que `trends`/`session_stats`; ausente si `history.db` no abrió) y lo pasa a `dashboard::build_payload`. |
| `src-tauri/src/dashboard.rs` | `AgentDashboard` gana `records: Option<RecordsDto>` (racha, mejor día, mejor día de líneas — todo pre-formateado). |
| `ui/panel.html` | Nueva tarjeta "Récords personales" por tab, debajo de "Líneas hoy"/"Últimos 30 días". |

Sin tablas ni dependencias nuevas.

### `PersonalRecords` (records.rs)

```rust
pub struct PersonalRecords {
    pub current_streak_days: u32,
    pub longest_streak_days: u32,
    pub best_day: Option<BestDay>,       // None solo si daily_usage está vacío para el agente
    pub best_lines_day: Option<BestLinesDay>, // None si no hay filas en `sessions`
}

pub struct BestDay { pub date: String, pub tokens: u64 }
pub struct BestLinesDay { pub date: String, pub lines_added: u64 }
```

### Contrato del panel (`RecordsDto`, todo pre-formateado salvo los conteos)

```jsonc
{
  "current_streak_days": 5,
  "longest_streak_days": 12,
  "best_day": "3.2M tok · 10 jul",
  "best_lines_day": "+420 · 12 jul"   // ausente (no null) si no hay datos de sessions
}
```

## Aspecto de la tarjeta

```
RÉCORDS PERSONALES
🔥 Racha actual        5 días
🏆 Racha más larga     12 días
📈 Mejor día           3.2M tok · 10 jul
✍️ Mejor día (líneas)  +420 · 12 jul     ← solo si hay datos de sessions
```

## Manejo de errores

- `history.db` no abrió: `records` ausente para todos los agentes — la tarjeta simplemente no
  se pinta (mismo criterio que trends/lines_today cuando `history.db` no está disponible).
- Agente con datos pero sin ninguna fila con `tokens > 0` (no debería pasar, `build_summary` ya
  exige eventos no vacíos, pero por si acaso): racha 0/0 y `best_day: None` — la tarjeta oculta
  esas líneas en vez de mostrar un `0 tok` inventado.
- Fechas corruptas/no parseables en `daily_usage` (no deberían existir, las escribe la propia
  app): se descartan al calcular la racha en vez de romper el cálculo completo.

## Testing

- Rust (unit, `tempfile`): racha de días consecutivos vs. con huecos; gracia de "hoy o ayer"
  para la racha actual (activo hoy, activo ayer, activo hace 2 días → 0); racha más larga
  distinta de la actual cuando la racha activa es más corta que un pico anterior; mejor día por
  tokens con empate (se queda con el más reciente); mejor día por líneas ausente sin filas en
  `sessions`, presente y agregando por día cuando hay varias sesiones el mismo día.
- `cargo test` completo en verde.
- Smoke manual: con datos reales de varios días, abrir el panel y comprobar que racha/mejor día
  coinciden con lo que se ve en el sparkink de tendencia; instalar la statusLine, generar
  actividad, y confirmar que aparece "Mejor día (líneas)".

## Fuera de alcance

- Comparación entre personas/equipos.
- Insignias/logros, notificaciones de "nuevo récord".
- Alertas de presupuesto.
- Persistir/cachear los récords en una tabla propia.
