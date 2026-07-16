# CodeContextAI

A tray / menu-bar widget that keeps your AI coding-agent usage visible at all
times: **real Anthropic rate limits** (the same 5h / 7-day meters you see on
claude.ai), token and cost totals, and per-project / per-model breakdowns —
for **Claude Code** and **opencode**.

Everything is computed locally from files those tools already write to disk.
No accounts, no telemetry, no server of its own.

## Features

- **Real rate-limit bars** — reads Claude Code's own OAuth credentials and
  queries Anthropic's usage endpoint, so the 5h and 7-day percentages and
  reset countdowns match your account exactly. When no credentials are
  available it falls back to a local estimate, clearly labeled `(estimado)`.
- **At-a-glance tray menu** — limit bars, today / month / 7-day tokens and
  cost per agent, right in the native menu.
- **Dashboard panel** ("Ver más…") — a dark glassmorphism window with
  per-agent tabs, traffic-light limit bars with live countdowns, stat tiles,
  and per-project / per-model tables, plus inline settings.
- **Tray headline** — optional "5h 62% · 7d 34%" label next to the icon
  (where the platform supports it).
- **30-day trend sparklines** per agent, backed by the app's own local
  history — no network, no account, just daily rollups next to your stats.
- **Terminal statusLine integration** — from the panel's Ajustes screen you
  can install a `statusLine.command` into `~/.claude/settings.json` so your
  5h/7d usage and today's cost show up right in Claude Code's terminal, plus
  it captures lines-added/removed and session counts the JSONL logs don't
  carry. Opt-in only: the app asks before touching that file, only ever
  edits the `statusLine` key, and never overwrites a command you already
  had configured without asking first.
- **Cost pricing** via the LiteLLM price table, refreshed at most daily.

## Install

Grab the installer for your platform from the
[latest release](https://github.com/komar0005/code-contex/releases):

| Platform | File |
|---|---|
| Windows | `*-setup.exe` (NSIS) or `*.msi` |
| Linux (Debian/Ubuntu) | `*.deb` |
| Linux (Fedora/openSUSE) | `*.rpm` |
| Linux (any) | `*.AppImage` |
| macOS | `*.dmg` (universal: Apple Silicon + Intel) |

### Linux notes

The tray icon requires an appindicator implementation. On Debian/Ubuntu:

```bash
sudo apt install libayatana-appindicator3-1
```

The `.deb`/`.rpm` packages declare it as a dependency; for the AppImage,
install it manually if no icon shows up.

### macOS notes

Releases are not code-signed (no Apple Developer certificate). If Gatekeeper
refuses to open the app:

```bash
xattr -cr "/Applications/ai-usage-tray.app"
```

On macOS, Claude Code stores its OAuth credentials in the login Keychain;
the app reads them from there automatically (item `Claude Code-credentials`).
macOS may ask once for permission to access it.

## Data sources

| Source | Location | Access |
|---|---|---|
| Claude Code usage | `~/.claude/projects/**/*.jsonl` | read-only |
| Claude Code credentials | `~/.claude/.credentials.json`, or the macOS Keychain | read-only, never written |
| Real limits | `GET https://api.anthropic.com/api/oauth/usage` | with the token above |
| opencode usage | `$OPENCODE_DATA_DIR/opencode.db` (default `~/.local/share/opencode/opencode.db`) | read-only SQLite |
| Model prices | LiteLLM's public price table | fetched at most daily |

The only network calls the app ever makes are the two listed above; both can
be observed in the source (`claude_oauth.rs`, `price_fetch.rs`). Preferences
are stored in your platform's config dir under `ai-usage-tray/preferences.json`.

## Build from source

Prerequisites: [Rust](https://rustup.rs) and the
[Tauri 2 system dependencies](https://tauri.app/start/prerequisites/).

```bash
cd src-tauri
cargo tauri dev      # run in development
cargo tauri build    # produce the platform installer
cargo test           # run the test suite
```

## License

[MIT](LICENSE)
