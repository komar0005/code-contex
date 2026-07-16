# AI Usage Tray Widget — Design

**Fecha:** 2026-07-14
**Estado:** Aprobado para pasar a plan de implementación

## Objetivo

App de bandeja del sistema (menu bar en macOS, systray en Linux) que muestra el
consumo (tokens y costo estimado en USD) de agentes de IA usados localmente —
Claude Code y opencode — leyendo directamente los archivos que estas
herramientas ya guardan en disco. Sin cuentas, sin backend propio, sin
telemetría.

## Alcance (v1)

- Plataformas: **macOS y Linux**. Windows fuera de alcance.
- Agentes: **Claude Code** y **opencode**. Si uno de los dos no está
  instalado/usado en la máquina, su sección se omite (ver "Manejo de
  errores").
- Alcance de datos: **agregado global** (todos los proyectos de la máquina)
  con **desglose expandible por proyecto**.
- Sin histórico persistente propio: cada refresco recalcula desde los
  archivos fuente (ver "Decisión: sin base de datos propia").
- Métricas: tokens y costo estimado en USD, más una estimación de "bloque de
  5 horas" y "ventana de 7 días" para dar una señal de cuánto se ha
  consumido recientemente.

## Arquitectura

```
┌─────────────────────────────────────────┐
│  Tray Icon (menu bar / systray)          │
│  ├─ click → dropdown con stats            │
│  └─ background: refresco periódico       │
└─────────────┬─────────────────────────────┘
              │
   ┌──────────┴───────────┐
   │   Rust backend         │
   │  - parsers (CC/opencode)│
   │  - cálculo de costo    │
   │  - estimación bloque 5h│
   └──────────┬───────────┘
              │ lee (solo lectura, sin red)
   ┌──────────┴────────────────────────┐
   │ ~/.claude/projects/**/*.jsonl      │
   │ ~/.local/share/opencode/storage/** │
   └─────────────────────────────────────┘
```

**Stack:** Tauri (Rust backend + frontend web ligero para el dropdown y la
ventana de preferencias). Elegido sobre Electron por menor consumo de RAM,
relevante porque la app corre en background indefinidamente.

## Decisión: sin base de datos propia

Se evaluó mantener una base de datos local (SQLite) con ingestión
incremental para preservar histórico aunque Claude Code/opencode roten sus
logs. Se descartó para v1: el usuario solo necesita ver el estado actual
(hoy/mes/bloque de 5h/7 días) al abrir el dropdown, no tendencias de meses.
Cada refresco recalcula directo de los archivos fuente, con una caché en
memoria por proceso (basada en `mtime` de archivo) para no re-parsear todo
el histórico en cada ciclo. La caché se pierde al reiniciar la app — es
aceptable porque no se persigue histórico duradero.

## Fuentes de datos y parsers

### Claude Code

- Ruta: `~/.claude/projects/**/*.jsonl`
- Por línea: si `type == "assistant"` y trae `usage`, extraer `model`,
  `input_tokens`, `output_tokens`, `cache_creation_input_tokens`,
  `cache_read_input_tokens` y el timestamp del mensaje.
- El proyecto se identifica por el nombre de la carpeta contenedora (slug de
  la ruta absoluta, ej. `-home-user-projects-ai-context`).
- Líneas corruptas o sin `usage` se ignoran sin interrumpir el parseo del
  archivo.

### opencode

- Ruta raíz: `~/.local/share/opencode/storage/` (ajustable si
  `OPENCODE_DATA_DIR` está definido en el entorno).
- Mensajes: `storage/message/{sessionID}/msg_*.json` — cada archivo trae
  tokens y timestamp.
- Proyecto: se resuelve cruzando con `storage/session/{projectHash}/{sessionID}.json`.
- El campo `cost` de opencode viene en `0` — el costo se calcula localmente
  igual que para Claude Code, nunca se usa ese campo.

## Cálculo de costo

- Tabla de precios por modelo empaquetada como **JSON estático embebido**
  (fallback offline garantizado).
- Si el refresco por red está habilitado (configurable, ver
  Preferencias), la app intenta actualizar esa tabla periódicamente desde
  una fuente pública (pricing DB de LiteLLM en GitHub). Si falla o no hay
  red, sigue usando la copia local sin bloquear ni mostrar error.
- Fórmula: `costo = Σ (tokens_por_tipo × precio_por_millón_del_modelo)`,
  separando input, output, cache-write y cache-read porque cada uno tiene
  precio distinto.
- Si un modelo no está en la tabla de precios (nuevo/desconocido): los
  tokens se siguen contando, pero esas entradas se marcan como "costo no
  calculado" en vez de inventar un precio. Se muestra un conteo agregado de
  cuántos mensajes quedaron sin precio, visible en Preferencias.

## Ventanas de tiempo

- **Hoy / mes en curso:** calendario natural (día natural, mes natural),
  hora local del sistema.
- **Bloque de 5 horas** (para el indicador de consumo reciente): ventana
  rodante que empieza en el primer mensaje de una racha de actividad y dura
  5h; un hueco sin actividad cierra el bloque y el siguiente mensaje abre
  uno nuevo. Aproxima el mecanismo real de los planes de Claude sin
  pretender leer el límite exacto del servidor (no existe API pública para
  eso).
- **Ventana de 7 días** (para el indicador semanal): ventana rodante de 7
  días naturales, no semana-calendario — porque no se conoce la fecha ancla
  real del ciclo de facturación del usuario.
- Estas ventanas de "bloque 5h" / "7 días" **solo aplican a Claude Code**;
  opencode no tiene un concepto equivalente de límite de plan.

## Presupuestos personales (no límites reales de Anthropic)

Anthropic no publica el límite exacto de tokens/mensajes por plan (varía
según mezcla de modelos), así que las barras de progreso del bloque de 5h y
de la ventana de 7 días **no** representan el límite real de la cuenta.
En su lugar, el usuario define su propio presupuesto personal en
Preferencias:

- Presupuesto por bloque de 5h (USD)
- Presupuesto por ventana de 7 días (USD)
- Presupuesto mensual (USD)

La barra de progreso se calcula contra esa meta personal, no contra un
límite de Anthropic. Esto evita mostrar un porcentaje engañoso sobre algo
que no podemos conocer con certeza.

## Preferencias (ventana nativa simple)

Accesible desde "⚙ Preferencias" en el dropdown. Contiene:

- Presupuesto por bloque de 5h / semanal / mensual (USD)
- Intervalo de refresco (default: 60s)
- Activar/desactivar el refresco de la tabla de precios por red
- Última actualización de la tabla de precios (fecha, informativo)
- Conteo de mensajes con "costo no calculado" (informativo)

## UI del dropdown

```
┌────────────────────────────────────┐
│  🤖 AI Usage                        │
├────────────────────────────────────┤
│  Claude Code                        │
│   Hoy       12.4K tok    $0.38      │
│   Mes       340K tok     $9.87      │
│   Bloque 5h  [████████░░] $7.80/$10 │
│      resetea en ~1h 12m             │
│   7 días     [██████░░░░] $34/$50   │
│   ▸ Ver por proyecto                │
├────────────────────────────────────┤
│  opencode                           │
│   Hoy       3.1K tok     $0.05      │
│   Mes       88K tok      $1.42      │
│   ▸ Ver por proyecto                │
├────────────────────────────────────┤
│   Actualizado a las 14:32            │
│   ⚙ Preferencias      ⟳ Refrescar   │
│   ⏻ Salir                            │
└────────────────────────────────────┘
```

- Cada sección de agente solo se muestra si hay datos para ese agente (ver
  "Manejo de errores").
- opencode no muestra barras de presupuesto en v1 (no tiene concepto de
  ventana/plan); se podría añadir después si se desea.
- "▸ Ver por proyecto" expande un submenú con el desglose de tokens/costo
  por carpeta de proyecto para el período actual.

## Mecanismo de refresco

- Timer interno cada N segundos (configurable, default 60s) que reescanea
  las carpetas fuente.
- Optimización: solo se re-parsean archivos cuyo `mtime` cambió desde el
  último ciclo (caché en memoria, no persistente).
- Botón manual "⟳ Refrescar" en el dropdown para forzar un ciclo inmediato.
- El dropdown muestra "Actualizado a las HH:MM" (hora local absoluta) para
  que el usuario sepa qué tan reciente es el dato. Se eligió una hora
  absoluta en vez de un texto relativo ("hace Xs") porque el menú nativo es
  una foto estática: un texto relativo quedaría congelado en el momento en
  que se construyó el menú y mentiría cada vez más mientras el usuario no
  lo reabra; una hora absoluta se mantiene siempre correcta.

## Manejo de errores y casos borde

- **Agente no instalado o sin uso:** si no existe la carpeta de datos del
  agente (o existe pero no contiene ningún archivo válido con `usage`),
  esa sección **se omite por completo** del dropdown — no se muestra vacía
  ni con ceros. Si ningún agente tiene datos, el dropdown muestra un
  estado vacío: "No se detectó actividad de agentes IA en este equipo".
- **Líneas/archivos corruptos:** se saltan silenciosamente sin interrumpir
  el parseo; se registran en un log de debug local, sin error visible en
  el tray.
- **Modelo desconocido en la tabla de precios:** ver sección "Cálculo de
  costo".
- **Falla el refresco de precios por red:** cae en silencio a la copia
  estática embebida; reintenta en el próximo ciclo programado.
- **Primer arranque con mucho histórico:** el escaneo inicial corre de
  forma asíncrona para no bloquear el ícono de tray; mientras tanto el
  dropdown muestra "Cargando…" en vez de datos parciales o engañosos.
- **Dependencia de tray en Linux:** Tauri usa `libappindicator` /
  `ayatana-appindicator` por debajo. Si la distro no lo tiene instalado, el
  ícono de tray no aparece — no hay fallback razonable dentro de la app;
  se documenta como requisito del sistema en el instalador/README.
- **Permisos de lectura:** los directorios fuente están dentro del home del
  usuario, no se esperan problemas de permisos en uso normal; si ocurre un
  error de I/O al leer, se trata igual que un archivo corrupto (se salta,
  se loguea, no se crashea la app).

## Fuera de alcance (v1)

- Windows.
- Histórico persistente / gráficas de tendencia de meses.
- Presupuestos o barras de progreso para opencode.
- Multi-cuenta o multi-perfil de Claude/opencode en la misma máquina.
- Cualquier llamada de red que no sea el refresco opcional de la tabla de
  precios (no hay autenticación, no hay telemetría, no hay servidor
  propio).

## Testing

- Parsers: tests unitarios con fixtures de archivos `.jsonl` / `.json` de
  ejemplo (incluyendo líneas corruptas, campos faltantes, modelos
  desconocidos) para validar agregación de tokens y cálculo de costo.
- Ventanas de tiempo: tests unitarios para la lógica de bloque de 5h
  (rachas con huecos) y ventana rodante de 7 días con timestamps fijos
  (sin depender del reloj real).
- Caso "agente no instalado": test que verifica que, sin la carpeta de
  datos correspondiente, la sección no aparece en el modelo de datos que
  consume la UI.
- UI del dropdown: verificación manual en macOS y Linux (systray real),
  dado que el testing automatizado de bandejas del sistema es limitado.
