# Integración con la statusLine de Claude Code — Spec de diseño

**Fecha:** 2026-07-16
**Estado:** Propuesto — pendiente de aprobación
**Contexto:** Segunda de tres fases de la sesión de brainstorming (2026-07-16). Fase 1 (`2026-07-16-usage-history-trends-design.md`, ya implementada) dejó un `history.db` propio con rollups diarios. Esta fase lo reutiliza para dos cosas a la vez: (a) mostrar datos de la app en la terminal, en la propia statusLine de Claude Code; (b) capturar datos de sesión (líneas añadidas/eliminadas, coste, duración) que hoy no existen en ningún archivo que la app pueda leer — solo Claude Code los conoce, y solo se los pasa a un script externo vía la statusLine. Fase 3 (modo contest local) reutilizará las tablas de ambas fases.

## Objetivo

Claude Code soporta un `statusLine.command` configurable en `~/.claude/settings.json`: un
comando que Claude Code invoca repetidamente (con JSON por stdin) y cuya primera línea de
stdout se pinta al pie de la terminal. Hoy CodeContextAI no participa en absoluto en ese
mecanismo. Esta fase añade un modo `--statusline` al mismo binario que:

1. **Produce**: imprime una línea compacta con el estado ya calculado por la app (% 5h/7d,
   coste de hoy) directamente en la terminal donde se usa Claude Code — sin abrir el panel.
2. **Consume**: aprovecha que Claude Code, en cada invocación, manda por stdin datos de sesión
   que la app nunca ha tenido — coste total de la sesión, líneas añadidas/eliminadas, duración —
   y los persiste, habilitando KPIs que el parser de JSONL no puede dar.

Instalar esto significa editar `~/.claude/settings.json`, un archivo que la app no posee. Esa
edición **nunca es automática ni silenciosa**: vive como una acción explícita en la vista de
Ajustes del panel — la app le pregunta al usuario si quiere instalarla, y solo actúa si
contesta que sí. El resultado (instalada o no) se recuerda como una preferencia propia de la
app, igual que el resto de `Preferences`.

## Alcance

- Nuevo modo de arranque del mismo binario (`ai-usage-tray --statusline`), detectado **antes**
  de inicializar Tauri/GTK — se ejecuta muchas veces por sesión de Claude Code y no puede
  pagar el coste de arrancar la GUI cada vez.
- Nueva tabla `sessions` en el `history.db` de la fase 1 (mismo archivo, no uno nuevo).
- Nuevos KPIs en el panel: líneas añadidas/eliminadas de hoy y nº de sesiones de hoy — solo
  en la tab de Claude Code (opencode no tiene equivalente a la statusLine).
- **Instalación asistida desde Ajustes**: un botón que le pregunta al usuario si quiere
  configurar la statusLine, escribe `~/.claude/settings.json` por él (fusionando, nunca
  pisando el resto del archivo) y guarda el resultado en `Preferences.statusline_installed`.
  Incluye poder desinstalarla desde el mismo lugar.
- **Fuera de alcance de esta fase:**
  - Encadenar automáticamente con una statusLine que el usuario ya tenga configurada (Claude
    Code solo permite un `statusLine.command`; si hay una distinta a la nuestra, se avisa y se
    pide confirmación explícita para reemplazarla — pero no se fusionan dos scripts en uno).
  - Alertas y modo contest — fase 3, reutiliza la tabla `sessions`.

## Decisiones tomadas (brainstorming)

1. **Un solo binario, modo detectado por argumento**, no un binario/crate nuevo — evita
   duplicar `pricing.rs`/`menu_format.rs`/`history.rs` y mantiene "sin dependencias nuevas".
   `main()` mira `std::env::args()` antes de tocar `tauri::Builder`; si es `--statusline`, corre
   la ruta ligera y termina (`std::process::exit`), sin tocar tray/webview/GTK en absoluto —
   así una terminal puede invocarlo decenas de veces por sesión sin coste de arranque de GUI.
2. **El lado "produce" lee un snapshot pre-calculado, nunca recalcula ni llama a la red.**
   La app (ya corriendo, refrescando cada `refresh_interval_secs`) escribe
   `config_dir()/statusline_snapshot.json` en cada `refresh_all` con los mismos datos que ya
   arma `menu_format::format_tray_title` (texto ya formateado: "5h 62% · 7d 34%", coste de hoy).
   `--statusline` solo lee ese archivo. Si no existe o tiene más de
   `2 × refresh_interval_secs` de antigüedad (app cerrada o colgada), no inventa nada: omite esa
   parte de la línea en vez de mostrar un dato viejo como si fuera actual.
3. **El lado "consume" no depende de que la app esté corriendo.** Cada invocación abre
   `history.db` directamente (mismo `history::open`), sin pasar por la app/tray, y hace upsert
   de una fila en `sessions` con los datos de esa invocación. Funciona incluso si el usuario
   nunca abrió la GUI — un uso legítimo es "solo quiero los datos, nunca abro el panel".
4. **`sessions` se actualiza por `session_id`, quedándose con el ÚLTIMO valor visto** (Claude
   Code manda los totales acumulados de la sesión en cada invocación, no deltas) — mismo
   principio de "quedarse con la última ocurrencia" que ya usa `parsers/claude_code.rs` para
   deduplicar por `message.id`.
5. **`history.db` pasa a modo WAL** (`PRAGMA journal_mode=WAL`) al abrirse — con dos escritores
   potenciales (la app de fondo + invocaciones frecuentes de `--statusline`), rusqlite en modo
   por defecto (rollback journal) podría devolver "database is locked" bajo contención. WAL
   permite un escritor y lectores concurrentes sin bloquear.
6. **Nunca romper la statusLine del usuario.** Cualquier fallo (JSON de stdin ilegible, sin
   snapshot, `history.db` no abre) debe imprimir algo razonable (aunque sea una cadena vacía o
   solo el nombre del modelo que ya viene en el JSON de entrada) y salir con código 0, rápido —
   igual que el resto de la app, ningún fallo aquí puede bloquear ni ensuciar la sesión de
   Claude Code del usuario.
7. **Sin backfill.** A diferencia de la fase 1, no existe ningún archivo con el histórico de
   líneas añadidas/eliminadas — ese dato solo existe hacia adelante, desde que el usuario
   instala el hook. La tabla `sessions` empieza vacía y crece con el uso.
8. **La instalación es una pregunta, no un paso automático.** Ajustes gana una tarjeta
   "Integración con la terminal" con un botón **"Instalar"**. Al pulsarlo, la app primero LEE
   `~/.claude/settings.json` (comando `check_statusline_cmd`) y decide qué mostrar:
   - Sin `statusLine.command` configurado → confirma directo y escribe.
   - Ya configurado con **nuestro propio comando** (reinstalación, p. ej. tras mover el
     binario) → botón dice "Reinstalar" en vez de "Instalar".
   - Configurado con **otro comando** → se muestra ese valor tal cual y el botón pasa a
     "Reemplazar" — nunca se pisa sin que el usuario vea antes qué había.
   Solo tras la confirmación del usuario se escribe el archivo.
9. **Fusión no destructiva del `settings.json` de Claude Code.** Se parsea como
   `serde_json::Value` genérico y se modifica *solo* la clave `statusLine`; el resto del
   archivo (hooks, permisos, modelo, etc.) se conserva byte a byte en valor. Para no reordenar
   alfabéticamente las claves del usuario al reescribir, `serde_json` habilita el feature
   `preserve_order` (ya es dependencia; esto es un feature flag, no una dependencia nueva).
10. **La preferencia vive en la app, no se adivina leyendo el archivo de Claude Code en cada
    arranque.** `Preferences.statusline_installed: bool` (con su ruta de comando instalada, para
    poder detectar reinstalación tras mover el binario) se guarda en `preferences.json` al
    instalar/desinstalar — consistente con cómo ya persiste todo lo demás en `Preferences`.
11. **Desinstalar es quirúrgico.** El botón "Desinstalar" solo borra la clave `statusLine` si su
    valor coincide EXACTAMENTE con lo que la app instaló (mismo comando guardado en
    `Preferences`) — si el usuario lo cambió a mano después, desinstalar avisa y no toca nada,
    para no borrar una personalización suya por error.

## Arquitectura

| Pieza | Responsabilidad |
|---|---|
| `src-tauri/src/main.rs` | Detecta `--statusline` antes de `tauri::Builder`; delega a `statusline::run()` y sale. `refresh_all` escribe `statusline_snapshot.json` en cada ciclo. Registra los comandos nuevos. |
| `src-tauri/src/statusline.rs` (nuevo) | Lee JSON de stdin (contrato de Claude Code), hace upsert en `sessions`, lee el snapshot, arma e imprime la línea final. |
| `src-tauri/src/claude_settings.rs` (nuevo) | Lee/fusiona/escribe `~/.claude/settings.json`: `read_statusline_command`, `install_statusline`, `uninstall_statusline` — todo por `serde_json::Value` genérico, tocando solo la clave `statusLine`. |
| `src-tauri/src/history.rs` | Nueva tabla `sessions` + `upsert_session` + `today_session_stats`; `open()` activa WAL. |
| `src-tauri/src/preferences.rs` | `Preferences` gana `statusline_installed: bool` y `statusline_installed_command: Option<String>` (para detectar reinstalación/desinstalación quirúrgica), ambos con `#[serde(default)]`. |
| `src-tauri/src/dashboard.rs` | `AgentDashboard` (solo Claude Code) gana `lines_today: Option<LinesDelta>` (`+N / -M`) y `sessions_today: Option<u32>`, alimentados desde `history::today_session_stats`. |
| `ui/panel.html` | Nueva línea/tile con líneas añadidas/eliminadas y nº de sesiones de hoy. Ajustes gana la tarjeta "Integración con la terminal" (estado + botón instalar/reemplazar/desinstalar). |

Sin dependencias nuevas de Cargo ni de JS — el parseo de stdin y de `settings.json` usa
`serde_json`, ya presente; se le habilita el feature `preserve_order` (decisión 9).

### Comandos Tauri nuevos

- `check_statusline_cmd() -> StatuslineStatus` — lee `~/.claude/settings.json` (sin escribir) y
  devuelve:
  ```jsonc
  { "state": "not_configured" }               // no hay statusLine.command
  { "state": "ours", "command": "…" }         // coincide con lo instalado por la app
  { "state": "foreign", "command": "otro…" }  // hay algo distinto — se muestra antes de pedir confirmación
  ```
- `install_statusline_cmd() -> Result<(), String>` — escribe/fusiona, actualiza `Preferences`.
  Se llama solo tras la confirmación del usuario en el panel (la pregunta ya la hizo la UI;
  este comando asume el "sí").
- `uninstall_statusline_cmd() -> Result<(), String>` — borra la clave solo si coincide con lo
  instalado (decisión 11); si no coincide, devuelve error explicativo para que el panel lo
  muestre en vez de tocar el archivo.

### Contrato de entrada (JSON por stdin, lo que Claude Code envía)

Solo los campos que consumimos (el resto se ignora):

```jsonc
{
  "session_id": "abc123",
  "cwd": "/home/user/project-a",
  "model": { "display_name": "claude-sonnet-5" },
  "cost": {
    "total_cost_usd": 1.42,
    "total_duration_ms": 340000,
    "total_lines_added": 58,
    "total_lines_removed": 12
  }
}
```

Cualquier campo ausente o de tipo inesperado se trata como si no existiera (mismo principio
de tolerancia que `parsers/claude_code.rs` con `Option`/`unwrap_or`).

### Esquema (`sessions`, añadida a `history.db`)

```sql
CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    date TEXT NOT NULL,          -- fecha LOCAL de la última actualización, para filtrar "hoy"
    project TEXT,
    model TEXT,
    cost_usd REAL NOT NULL,
    lines_added INTEGER NOT NULL,
    lines_removed INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    updated_at TEXT NOT NULL     -- RFC3339, para depuración/expiración futura
);
```

### `statusline_snapshot.json` (escrito por la app, leído por `--statusline`)

```jsonc
{
  "written_at": "2026-07-16T20:05:00Z",
  "tray_title": "5h 62% · 7d 34%",   // idéntico a format_tray_title; None -> ausente
  "today_cost": "$4.30"
}
```

### Línea de salida (ejemplo)

```
5h 62% · 7d 34% · hoy $4.30 · claude-sonnet-5
```

Si no hay snapshot (app cerrada) o está vencido, se omite esa parte y solo queda el modelo
(dato que ya viene en el JSON de entrada, sin coste ni red):

```
claude-sonnet-5
```

## Manejo de errores

- Snapshot ausente/corrupto/vencido: se omite esa parte de la línea, nunca se muestra un dato
  viejo sin avisar ni se cae el proceso.
- `history.db` no se puede abrir: se salta el upsert de sesión (se pierde ese dato puntual,
  no se reintenta) y se sigue con la parte "produce" igual.
- JSON de stdin ilegible o campos ausentes: upsert de sesión se salta; la línea de salida cae
  al mínimo (nombre de modelo si estaba en un JSON parcialmente válido, si no cadena vacía).
- El proceso `--statusline` **siempre** sale con código 0 y en <100ms de trabajo propio (sin
  red, sin escaneo de JSONL) — cualquier excepción se captura y degrada al camino de arriba.

## Testing

- Rust (unit, `tempfile`): `upsert_session` crea/actualiza por `session_id`; una segunda
  invocación con más líneas/coste actualiza la fila (no duplica); `today_session_stats` filtra
  por fecha local y suma líneas/cuenta sesiones; parseo del JSON de stdin tolera campos
  ausentes; construcción de la línea de salida con/sin snapshot, con snapshot vencido.
- Rust (unit, `tempfile`, para `claude_settings.rs`): `read_statusline_command` distingue
  `not_configured`/`ours`/`foreign`; `install_statusline` sobre un `settings.json` con otras
  claves (hooks, permisos) las deja intactas; instalar sobre un archivo inexistente lo crea;
  `uninstall_statusline` borra la clave cuando coincide y NO toca el archivo (y devuelve error)
  cuando el comando configurado es distinto al guardado en `Preferences`.
- `cargo test` completo en verde.
- Smoke manual: desde Ajustes, instalar la statusLine (confirmar que aparece en
  `~/.claude/settings.json` sin tocar el resto del archivo), abrir Claude Code y ver la línea
  aparecer y cambiar; cerrar CodeContextAI y confirmar que la parte de límites desaparece
  mientras el modelo sigue mostrándose; volver a Ajustes y desinstalar; editar la statusLine a
  mano y confirmar que "Desinstalar" avisa en vez de borrarla.

## Fuera de alcance

- Encadenar automáticamente con una statusLine ya existente del usuario (se avisa y se pide
  reemplazar, pero no se fusionan dos scripts en uno).
- Alertas de presupuesto basadas en burn rate — reutiliza estos datos, no se implementa aquí.
- Modo contest / mejores marcas personales — fase 3.
- Expiración/purga de filas de `sessions`.
