# Contributing

Thanks for your interest! This is a small project; the workflow is simple.

## Development setup

Install [Rust](https://rustup.rs) and the
[Tauri 2 prerequisites](https://tauri.app/start/prerequisites/), then:

```bash
cd src-tauri
cargo tauri dev   # run the app
cargo test        # run the test suite
```

## Pull requests

- Open an issue first for anything bigger than a small fix, so we can agree
  on the approach before you invest time.
- Keep PRs focused: one change per PR.
- All tests must pass (`cargo test`) and `cargo check` must be warning-free.
- New behavior needs tests. The codebase is test-driven — parsers,
  formatters, and aggregation logic all have unit tests to mirror.
- User-visible strings are Spanish (matching the existing UI); code,
  comments, and commit messages are English.

## Adding a provider

The seam is the `Provider` trait (`src-tauri/src/provider.rs`): implement
`gather_events` (read local usage) and `fetch_limits` (real rate limits, or
`None`), then register it in `main.rs` and add a tab entry in
`ui/panel.html` (`KNOWN_AGENTS`).
