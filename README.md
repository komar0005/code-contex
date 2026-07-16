# AI Usage Tray Widget

Tray/menu-bar app for macOS and Linux showing local token/cost consumption
for Claude Code and opencode. Reads only files these tools already write to
disk — no accounts, no telemetry, no server of its own.

## Build

```bash
cd src-tauri
cargo tauri build
```

Produces a platform-native bundle (`.app` on macOS, `.deb`/`.AppImage` on
Linux, depending on your `cargo tauri build` target flags).

## Linux prerequisite

The tray icon depends on `libappindicator`/`ayatana-appindicator` being
installed on your system. On Debian/Ubuntu:

```bash
sudo apt install libayatana-appindicator3-1
```

Other distros: install the equivalent package for your desktop environment.
Without it, the app runs but no tray icon appears.

## Data sources

- Claude Code: `~/.claude/projects/**/*.jsonl`
- opencode: `$OPENCODE_DATA_DIR/opencode.db` (defaults to
  `~/.local/share/opencode/opencode.db`), opened read-only. One usage entry
  per opencode session (session-level token totals), not per message.

If neither source has usage data, the tray shows an empty-state message
instead of the per-agent sections.

## Scope (v1)

- macOS and Linux only.
- No persistent history beyond what Claude Code/opencode retain on disk —
  every refresh recomputes from source files.
- Budget bars (5h block / 7-day window) are against a **personal budget you
  configure in Preferences**, not Anthropic's real account limit — that
  value isn't available locally.
