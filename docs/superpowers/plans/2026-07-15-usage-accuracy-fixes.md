# Usage Accuracy Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the accuracy bugs found in the post-merge audit of the AI Usage Tray Widget: ~2.1x token/cost overcounting from duplicate JSONL lines, UTC-instead-of-local calendar days, a permanently-frozen "Refrescado hace 0s" label, un-anchored 5h blocks, and two display artifacts.

**Architecture:** Surgical fixes to existing modules — no new modules, no new dependencies, no structural changes. The percent/budget-bar math itself was audited and is correct; every fix here is upstream of it (data accuracy) or cosmetic (label rendering).

**Tech Stack:** Existing Rust codebase in `src-tauri/` (Tauri 2, chrono, serde_json). Nothing new.

## Audit Evidence (why each fix exists)

Measured 2026-07-15 against the real `~/.claude/projects` on the development machine (178 files):

- The app counted **13,056** assistant-usage lines but only **5,927 unique `message.id`s** — Claude Code writes one JSONL line *per content block* of the same assistant message (thinking, text, tool_use…), each repeating the full `usage` object. Token overcount factor: **2.09x** (1,763M counted vs 842M real).
- **1,228** same-id lines carry *different* usage than the first occurrence (streaming updates) — so dedup must keep the **last** occurrence per id, which carries the final numbers.
- **20** lines have `model: "<synthetic>"` (Claude Code error placeholders, not real API calls); they pollute `unpriced_count`.
- The design spec (docs/superpowers/specs/2026-07-14-ai-usage-tray-widget-design.md, "Ventanas de tiempo") requires "Hoy / mes en curso" in **hora local del sistema**; `windows::is_same_calendar_day/month` compare in UTC. At UTC-4, "Hoy" rolls over at 8:00 PM local.
- `main.rs` calls `tray::build_menu(app, …, now, now)` — `last_refresh == now` at build time, and the menu text is frozen until the next rebuild, so the label always reads "Refrescado hace 0s". `AppState.last_refresh` is written but never read.

An implementer can re-verify the dedup numbers with: count `type=="assistant"` lines having `message.usage`, group by `message.id` per file, compare line count vs unique-id count.

## Global Constraints

- Platforms: **macOS and Linux only**. No Windows support.
- No new crate dependencies (chrono already provides `Local`, `FixedOffset`, `DurationRound`).
- No network calls beyond what already exists. No app-owned persistent database.
- Every task ends with the FULL suite green: `cd src-tauri && cargo test`. Baseline before this plan: **41 passing**. Tasks below explicitly migrate or delete a few existing tests; any other test breaking means the change is wrong.
- All user-facing strings remain in Spanish, matching the existing UI.
- Tests must be deterministic regardless of the machine's timezone: production code may use `chrono::Local`, but tests must ALWAYS go through the timezone-parameterized `_in` variants with an explicit `FixedOffset` — never assert through `Local`.

---

### Task 1: Deduplicate Claude Code events by message id; skip synthetic models

**Files:**
- Modify: `src-tauri/src/parsers/claude_code.rs`
- Modify: `src-tauri/tests/fixtures/claude_code_sample.jsonl`

**Interfaces:**
- Consumes: `model::{Agent, UsageEvent}` (unchanged).
- Produces: `parse_jsonl_content(content: &str, fallback_project: &str) -> Vec<UsageEvent>` — **signature unchanged**, callers (`main.rs::gather_claude_events`, the `FileCache` closure) need no edits. New behavior: one event per `message.id` (last occurrence wins), `<synthetic>` models skipped, result sorted by timestamp ascending.

- [ ] **Step 1: Rewrite the fixture to exercise dedup, last-wins, and synthetic-skip**

Replace the entire content of `src-tauri/tests/fixtures/claude_code_sample.jsonl` with exactly these 7 lines:

```
{"type":"assistant","timestamp":"2026-07-14T10:00:00.000Z","cwd":"/home/user/project-a","message":{"id":"msg_A","model":"claude-sonnet-5","usage":{"input_tokens":999,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
{"type":"user","timestamp":"2026-07-14T10:00:01.000Z","message":{"role":"user","content":"hi"}}
{"type":"assistant","timestamp":"2026-07-14T10:00:02.000Z","message":{"id":"msg_X","model":"claude-sonnet-5","usage":null}}
{this is not valid json,,,
{"type":"assistant","timestamp":"2026-07-14T10:00:03.000Z","cwd":"/home/user/project-a","message":{"id":"msg_A","model":"claude-sonnet-5","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":200,"cache_read_input_tokens":300}}}
{"type":"assistant","timestamp":"2026-07-14T11:30:00.000Z","message":{"id":"msg_B","model":"claude-haiku-4-5-20251001","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
{"type":"assistant","timestamp":"2026-07-14T11:45:00.000Z","message":{"id":"msg_C","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
```

Line semantics: line 1 and line 5 share `msg_A` (line 5 is the streaming-final version — its usage must win); line 6 has no `cwd` (exercises `fallback_project`); line 7 is a synthetic placeholder that must be skipped entirely.

- [ ] **Step 2: Update the test to assert the new behavior (it must FAIL against current code)**

Replace the existing `parses_only_valid_assistant_usage_lines` test in `src-tauri/src/parsers/claude_code.rs` with:

```rust
    #[test]
    fn dedupes_by_message_id_keeping_last_and_skips_synthetic() {
        let events = parse_jsonl_content(&fixture(), "fallback");
        // msg_A (deduped, last occurrence wins) + msg_B. msg_C is synthetic -> skipped.
        assert_eq!(events.len(), 2);

        // Sorted by timestamp: msg_A (10:00:03) first, msg_B (11:30) second.
        assert_eq!(events[0].model, "claude-sonnet-5");
        assert_eq!(events[0].project, "/home/user/project-a");
        // Last occurrence's usage, NOT the first line's input_tokens: 999.
        assert_eq!(events[0].input_tokens, 100);
        assert_eq!(events[0].output_tokens, 50);
        assert_eq!(events[0].cache_write_tokens, 200);
        assert_eq!(events[0].cache_read_tokens, 300);

        assert_eq!(events[1].model, "claude-haiku-4-5-20251001");
        assert_eq!(events[1].total_tokens(), 15);
        assert_eq!(events[1].project, "fallback");
    }
```

- [ ] **Step 3: Run it to confirm it fails**

```bash
cd src-tauri
cargo test parsers::claude_code::tests::dedupes_by_message_id_keeping_last_and_skips_synthetic
```

Expected: FAIL — current code returns 3 events (msg_A twice + msg_B) plus the synthetic one, i.e. `assert_eq!(events.len(), 2)` fails with left = 4.

- [ ] **Step 4: Implement dedup in `parse_jsonl_content`**

Replace the body of `parse_jsonl_content` in `src-tauri/src/parsers/claude_code.rs`. Add `use std::collections::HashMap;` to the imports, then:

```rust
/// Claude Code writes one JSONL line per content block of the same assistant
/// message (thinking, text, tool_use...), each repeating the full `usage`
/// object under the same `message.id` — counting lines directly overcounts
/// ~2x. Dedupe by message id, keeping the LAST occurrence: streaming updates
/// mean the final line carries the definitive usage. Lines with
/// `model: "<synthetic>"` are Claude Code error placeholders, not real API
/// calls, and are skipped entirely.
pub fn parse_jsonl_content(content: &str, fallback_project: &str) -> Vec<UsageEvent> {
    let mut by_id: HashMap<String, UsageEvent> = HashMap::new();
    let mut without_id: Vec<UsageEvent> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage").filter(|u| !u.is_null()) else {
            continue;
        };
        let Some(model) = message.get("model").and_then(Value::as_str) else {
            continue;
        };
        if model == "<synthetic>" {
            continue;
        }
        let Some(timestamp_str) = value.get("timestamp").and_then(Value::as_str) else {
            continue;
        };
        let Ok(timestamp) = DateTime::parse_from_rfc3339(timestamp_str) else {
            continue;
        };
        let project = value
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or(fallback_project)
            .to_string();

        let event = UsageEvent {
            agent: Agent::ClaudeCode,
            project,
            model: model.to_string(),
            input_tokens: usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            output_tokens: usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            cache_write_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            timestamp: timestamp.with_timezone(&Utc),
        };

        match message.get("id").and_then(Value::as_str) {
            // Later lines for the same message overwrite earlier ones.
            Some(id) => {
                by_id.insert(id.to_string(), event);
            }
            // No id: can't dedupe, keep as-is rather than lose data.
            None => without_id.push(event),
        }
    }
    let mut events: Vec<UsageEvent> = by_id.into_values().collect();
    events.append(&mut without_id);
    // HashMap iteration order is random; sort for deterministic output.
    events.sort_by_key(|e| e.timestamp);
    events
}
```

Note: dedup is intentionally **per file** — `parse_jsonl_content` receives one file's content, and the `FileCache` in `main.rs` caches per file, so a global-across-files dedup isn't possible at this layer. Per-file dedup captures the measured 2.09x; cross-file duplication was not observed in the audit.

- [ ] **Step 5: Run the parser tests, then the full suite**

```bash
cargo test parsers::claude_code::tests
cargo test
```

Expected: the new test passes; `discover_files_returns_empty_for_missing_dir` and `folder_slug_project_name_uses_parent_dir` still pass; full suite green (41 tests — same count: one test replaced one).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/parsers/claude_code.rs src-tauri/tests/fixtures/claude_code_sample.jsonl
git commit -m "fix: dedupe Claude Code usage by message id, skip synthetic models

Claude Code writes one JSONL line per content block of the same
assistant message, each repeating the full usage object. Counting
lines overcounted tokens/cost ~2.09x (measured against real logs).
Keep the last occurrence per message.id (streaming makes it the
definitive one) and skip '<synthetic>' error placeholders."
```

---

### Task 2: Calendar day/month in local time (per spec), timezone-injectable for tests

**Files:**
- Modify: `src-tauri/src/windows.rs`
- Modify: `src-tauri/src/summary.rs` (one timestamp literal in an existing test — see the correction note under Step 4 below; production code in this file is untouched)

**Interfaces:**
- Consumes: nothing new.
- Produces: `is_same_calendar_day(a, b)` / `is_same_calendar_month(a, b)` — **public signatures unchanged** (still take `DateTime<Utc>`), now comparing in `chrono::Local`; `summary.rs`'s production code needs no edits, only the one test-fixture timestamp described below. New public tz-parameterized variants `is_same_calendar_day_in<Tz>` / `is_same_calendar_month_in<Tz>` used by tests (and available to future callers).

- [ ] **Step 1: Write failing tests using an explicit FixedOffset**

In `src-tauri/src/windows.rs`, inside the existing `mod tests`, REPLACE the three existing calendar tests (`same_calendar_day_true_for_same_date_different_time`, `same_calendar_day_false_across_midnight`, `same_calendar_month_ignores_day`) — they assert UTC-day semantics through the public functions, which would make them fail on any machine west of UTC once the fix lands — with these five, all going through the `_in` variants with explicit offsets:

```rust
    use chrono::FixedOffset;

    fn utc_minus_4() -> FixedOffset {
        FixedOffset::west_opt(4 * 3600).unwrap()
    }

    fn utc_offset_zero() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    #[test]
    fn same_day_at_utc_when_offset_zero() {
        let a = Utc.with_ymd_and_hms(2026, 7, 14, 1, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 14, 23, 59, 0).unwrap();
        assert!(is_same_calendar_day_in(a, b, &utc_offset_zero()));
    }

    #[test]
    fn different_utc_days_can_be_same_local_day() {
        // 23:00Z Jul 14 and 01:00Z Jul 15 are both Jul 14 evening at UTC-4.
        let a = Utc.with_ymd_and_hms(2026, 7, 14, 23, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 15, 1, 0, 0).unwrap();
        assert!(!is_same_calendar_day_in(a, b, &utc_offset_zero()));
        assert!(is_same_calendar_day_in(a, b, &utc_minus_4()));
    }

    #[test]
    fn same_utc_day_can_be_different_local_days() {
        // 01:00Z and 23:00Z on Jul 15 are Jul 14 (21:00) and Jul 15 (19:00) at UTC-4.
        let a = Utc.with_ymd_and_hms(2026, 7, 15, 1, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 15, 23, 0, 0).unwrap();
        assert!(is_same_calendar_day_in(a, b, &utc_offset_zero()));
        assert!(!is_same_calendar_day_in(a, b, &utc_minus_4()));
    }

    #[test]
    fn month_boundary_respects_local_offset() {
        // 02:00Z Aug 1 is still Jul 31 at UTC-4.
        let a = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 8, 1, 2, 0, 0).unwrap();
        assert!(!is_same_calendar_month_in(a, b, &utc_offset_zero()));
        assert!(is_same_calendar_month_in(a, b, &utc_minus_4()));
    }

    #[test]
    fn same_calendar_month_ignores_day() {
        let a = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 7, 31, 23, 0, 0).unwrap();
        assert!(is_same_calendar_month_in(a, b, &utc_offset_zero()));
    }
```

- [ ] **Step 2: Run to confirm they fail to compile (functions don't exist yet)**

```bash
cargo test windows::tests
```

Expected: compile error — `is_same_calendar_day_in` not found.

- [ ] **Step 3: Implement the `_in` variants and re-point the public functions at `Local`**

In `src-tauri/src/windows.rs`, change the chrono import line to include `Local` and `TimeZone`:

```rust
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Utc};
```

Replace the two existing functions with:

```rust
/// "Hoy"/"mes en curso" use the SYSTEM LOCAL calendar per the design spec —
/// comparing in UTC would make the daily counter roll over mid-evening for
/// anyone west of UTC. The `_in` variants exist so tests can pin an explicit
/// offset instead of depending on the machine's timezone.
pub fn is_same_calendar_day_in<Tz: TimeZone>(a: DateTime<Utc>, b: DateTime<Utc>, tz: &Tz) -> bool {
    let a = a.with_timezone(tz);
    let b = b.with_timezone(tz);
    a.year() == b.year() && a.ordinal() == b.ordinal()
}

pub fn is_same_calendar_day(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    is_same_calendar_day_in(a, b, &Local)
}

pub fn is_same_calendar_month_in<Tz: TimeZone>(
    a: DateTime<Utc>,
    b: DateTime<Utc>,
    tz: &Tz,
) -> bool {
    let a = a.with_timezone(tz);
    let b = b.with_timezone(tz);
    a.year() == b.year() && a.month() == b.month()
}

pub fn is_same_calendar_month(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    is_same_calendar_month_in(a, b, &Local)
}
```

- [ ] **Step 4: Run the tests, then the full suite**

```bash
cargo test windows::tests
cargo test
```

Expected: 5 calendar tests + `aggregate_sums_tokens_and_tracks_unpriced` pass, but the full suite will show a NEW failure: `summary::tests::aggregates_today_month_and_by_project`.

> **Correction (found during implementation, 2026-07-15):** this plan's original text claimed `summary.rs`'s fixtures were "far from any offset boundary" — that's wrong. `earlier_this_month = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0)` sits exactly at 00:00 UTC on the 1st of the month: under correct local-time semantics, ANY negative UTC offset (all of the Americas) shifts it to June 30th local time, a different month than `now` (July 14). The fix under test in this task is working correctly — the pre-existing test fixture is the bug. Fix the fixture, not the implementation:
>
> In `src-tauri/src/summary.rs`'s `aggregates_today_month_and_by_project` test, change:
> ```rust
> let earlier_this_month = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
> ```
> to:
> ```rust
> let earlier_this_month = Utc.with_ymd_and_hms(2026, 7, 5, 12, 0, 0).unwrap();
> ```
> July 5th at noon UTC stays within July under any real-world UTC offset (-12 to +14). Leave `last_month = Utc.with_ymd_and_hms(2026, 6, 15, 0, 0, 0)` unchanged — June 15th at midnight UTC is comfortably mid-month and safe under any offset. This is a one-line change to a timestamp literal; no assertions change, since the test still expects `earlier_this_month` to land in the same month as `now`.
>
> This file (`summary.rs`) is otherwise still out of this task's scope — do not touch anything else in it.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/windows.rs src-tauri/src/summary.rs
git commit -m "fix: compute 'today'/'this month' in local time per spec

UTC comparison made the daily counter roll over at 8pm for UTC-4
users. Public signatures unchanged; tests pin explicit offsets so
they're deterministic on any machine."
```

---

### Task 3: Honest freshness label ("Actualizado a las HH:MM") and remove dead `last_refresh`

**Files:**
- Modify: `src-tauri/src/menu_format.rs`
- Modify: `src-tauri/src/tray.rs`
- Modify: `src-tauri/src/main.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `menu_format::format_updated_at(refreshed_at: DateTime<Utc>) -> String` (+ tz-injectable `format_updated_at_in`); `tray::build_menu` loses its `last_refresh` parameter — new signature `build_menu(app, claude, opencode, prefs, now: DateTime<Utc>)`; `AppState` loses the `last_refresh` field. `format_refreshed_at` is deleted.

Why absolute instead of relative: the tray menu is a static snapshot — a relative label ("hace 12s") is frozen at build time and lies more the longer the menu sits unopened. Today it always says "hace 0s" because `main.rs` passes `now, now`. An absolute local wall-clock time ("Actualizado a las 14:32") stays true forever.

- [ ] **Step 1: Write the failing test**

In `src-tauri/src/menu_format.rs`, inside `mod tests`, DELETE `format_refreshed_at_seconds_and_minutes` and add:

```rust
    #[test]
    fn format_updated_at_renders_local_wall_clock() {
        use chrono::FixedOffset;
        let refreshed = Utc.with_ymd_and_hms(2026, 7, 14, 18, 32, 5).unwrap();
        let tz = FixedOffset::west_opt(4 * 3600).unwrap();
        assert_eq!(format_updated_at_in(refreshed, &tz), "Actualizado a las 14:32");
    }
```

- [ ] **Step 2: Run to confirm compile failure**

```bash
cargo test menu_format::tests
```

Expected: compile error — `format_updated_at_in` not found (and `format_refreshed_at` now unreferenced by tests but still referenced by `tray.rs`, so it won't be flagged dead yet).

- [ ] **Step 3: Implement in `menu_format.rs`**

Delete `format_refreshed_at` entirely and add (adjusting the top import line to `use chrono::{DateTime, Local, TimeZone, Utc};`):

```rust
/// Absolute local wall-clock time of the last refresh. The tray menu is a
/// static snapshot, so a relative "hace Xs" label would freeze at build time
/// and grow stale; an absolute time stays true however long the menu sits.
pub fn format_updated_at_in<Tz: TimeZone>(refreshed_at: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    format!("Actualizado a las {}", refreshed_at.with_timezone(tz).format("%H:%M"))
}

pub fn format_updated_at(refreshed_at: DateTime<Utc>) -> String {
    format_updated_at_in(refreshed_at, &Local)
}
```

- [ ] **Step 4: Update `tray.rs`**

In `src-tauri/src/tray.rs`:
1. In the `use crate::menu_format::{...}` import list, replace `format_refreshed_at` with `format_updated_at`.
2. Change `build_menu`'s signature from `(app, claude, opencode, prefs, last_refresh: DateTime<Utc>, now: DateTime<Utc>)` to `(app, claude, opencode, prefs, now: DateTime<Utc>)` — delete the `last_refresh` parameter.
3. Change the label line from `format_refreshed_at(last_refresh, now)` to `format_updated_at(now)`.

- [ ] **Step 5: Update `main.rs`**

1. In `refresh_all`, change the `tray::build_menu(app, claude_summary.as_ref(), opencode_summary.as_ref(), &prefs, now, now)` call to pass a single `now`.
2. Delete the `last_refresh: Mutex<chrono::DateTime<Utc>>` field from `AppState`, its initializer `last_refresh: Mutex::new(Utc::now()),` in the `.manage(...)` block, and the `*state.last_refresh.lock().unwrap() = now;` line in `refresh_all`. (It was never read anywhere — verify with a grep for `last_refresh` before deleting; the only hits must be those three plus none elsewhere. Do NOT touch `last_pricing_update`, which IS read by `get_pricing_status_cmd`.)

- [ ] **Step 6: Build warning-clean and run the full suite**

```bash
cargo build
cargo test
```

Expected: zero warnings (if `format_refreshed_at` or the `AppState` field were left behind, dead-code warnings appear — fix by completing the deletion, not by `#[allow]`), full suite green.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/menu_format.rs src-tauri/src/tray.rs src-tauri/src/main.rs
git commit -m "fix: show absolute refresh time instead of frozen 'hace 0s'

build_menu received last_refresh == now, and the static menu froze
the relative label, so it always read 'Refrescado hace 0s'. Show the
local wall-clock time of the refresh instead, and drop the
never-read AppState.last_refresh field."
```

---

### Task 4: Anchor 5h blocks to the top of the hour

**Files:**
- Modify: `src-tauri/src/windows.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `compute_blocks` — signature unchanged; block `start` is now the first event's timestamp **floored to the hour (UTC)**, matching how Anthropic's real 5h windows are anchored (and how ccusage models them). `summary.rs` needs no edits.

- [ ] **Step 1: Write the failing tests**

In `src-tauri/src/windows.rs`, inside the existing `mod block_tests`, add:

```rust
    #[test]
    fn block_start_floors_to_the_hour() {
        let first_event = Utc.with_ymd_and_hms(2026, 7, 14, 8, 47, 12).unwrap();
        let blocks = compute_blocks(&[event_at(first_event)]);
        assert_eq!(blocks[0].start, Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap());
        assert_eq!(blocks[0].end, Utc.with_ymd_and_hms(2026, 7, 14, 13, 0, 0).unwrap());
    }

    #[test]
    fn fixed_anchor_not_sliding_window() {
        // First event 8:47 -> block [8:00, 13:00). An event at 12:30 (only
        // ~1h45m after the previous event) must still open NO new block; an
        // event at 13:00 must. A sliding-window bug (measuring the gap from
        // the previous event instead of from the block start) would keep
        // 13:00 inside the first block, since 13:00 - 12:30 < 5h.
        let t0 = Utc.with_ymd_and_hms(2026, 7, 14, 8, 47, 0).unwrap();
        let mid = Utc.with_ymd_and_hms(2026, 7, 14, 12, 30, 0).unwrap();
        let boundary = Utc.with_ymd_and_hms(2026, 7, 14, 13, 0, 0).unwrap();
        let blocks = compute_blocks(&[event_at(t0), event_at(mid), event_at(boundary)]);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].events.len(), 2);
        assert_eq!(blocks[1].start, boundary); // 13:00 floors to itself
    }
```

- [ ] **Step 2: Run to confirm the first one fails**

```bash
cargo test windows::block_tests
```

Expected: `block_start_floors_to_the_hour` FAILS (start is currently 8:47:12, not 8:00). `fixed_anchor_not_sliding_window` also fails (current code puts the 13:00 event in a second block starting 13:47? No — current anchor is 8:47, 8:47+5h = 13:47, so 13:00 stays in block 1: `assert_eq!(blocks.len(), 2)` fails with left = 1). Both failures are expected.

- [ ] **Step 3: Implement the hour floor**

In `src-tauri/src/windows.rs`, add `DurationRound` to the chrono import (`use chrono::{DateTime, Datelike, Duration, DurationRound, Local, TimeZone, Utc};`), then in `compute_blocks`, replace the block-creation push with:

```rust
        if needs_new_block {
            // Anchor to the top of the hour: Anthropic's real 5h windows
            // start on the hour containing the first message, not at the
            // message's exact timestamp.
            let start = event
                .timestamp
                .duration_trunc(Duration::hours(1))
                .unwrap_or(event.timestamp);
            blocks.push(Block {
                start,
                end: start + block_len,
                events: Vec::new(),
            });
        }
```

Also update the function's doc comment to mention the hour anchoring.

- [ ] **Step 4: Run block tests, then the full suite**

```bash
cargo test windows::block_tests
cargo test
```

Expected: all block tests pass. The four pre-existing block tests use `t0 = 8:00:00` exactly, so flooring is a no-op for them — they must still pass unchanged. Full suite green. Note `summary::tests::active_5h_block_present_when_recent_activity_exists` uses an event AT `now` (12:00:00) — block becomes [12:00, 17:00), still contains `now`. Passes.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/windows.rs
git commit -m "fix: anchor 5h blocks to the top of the hour

Matches how Anthropic's real session windows are anchored; the
'resetea en ~X' estimate was off by up to 59 minutes."
```

---

### Task 5: Display polish — over-budget bar with zero budget, and K→M rollover

**Files:**
- Modify: `src-tauri/src/menu_format.rs`

**Interfaces:**
- Consumes / Produces: `format_budget_line` and `format_tokens` — signatures unchanged, only edge-case output changes. `tray.rs` needs no edits.

- [ ] **Step 1: Write the failing tests**

In `src-tauri/src/menu_format.rs` tests, add:

```rust
    #[test]
    fn zero_budget_with_spend_shows_full_bar() {
        assert_eq!(
            format_budget_line("5h", 15.0, 0.0),
            "5h  [██████████] $15.00/$0.00"
        );
        // Zero budget, zero spend: still empty.
        assert_eq!(
            format_budget_line("5h", 0.0, 0.0),
            "5h  [░░░░░░░░░░] $0.00/$0.00"
        );
    }

    #[test]
    fn format_tokens_rolls_over_to_m_just_under_a_million() {
        assert_eq!(format_tokens(999_999), "1.0M tok");
        assert_eq!(format_tokens(999_949), "999.9K tok");
    }
```

- [ ] **Step 2: Run to confirm both fail**

```bash
cargo test menu_format::tests
```

Expected: `zero_budget_with_spend_shows_full_bar` fails (current code shows an empty bar for budget 0 regardless of spend); `format_tokens_rolls_over_to_m_just_under_a_million` fails (999,999 renders as "1000.0K tok").

- [ ] **Step 3: Implement both fixes**

In `format_budget_line`, replace the `pct` line:

```rust
    let pct = if budget > 0.0 {
        (spent / budget * 100.0).clamp(0.0, 100.0)
    } else if spent > 0.0 {
        // Any spend against a zero budget is over budget: show a full bar
        // rather than a misleading empty one.
        100.0
    } else {
        0.0
    };
```

In `format_tokens`, replace the K branch so values that would round-print as "1000.0K" roll over to M:

```rust
    } else if tokens >= 1_000 {
        let k = tokens as f64 / 1_000.0;
        if k >= 999.95 {
            format!("{:.1}M tok", tokens as f64 / 1_000_000.0)
        } else {
            format!("{k:.1}K tok")
        }
    } else {
```

- [ ] **Step 4: Run the module tests, then the full suite**

```bash
cargo test menu_format::tests
cargo test
```

Expected: all `menu_format` tests pass, including the pre-existing `format_tokens_boundaries` and `format_budget_line_at_zero_fifty_and_over_budget` (unchanged behavior in their ranges). Full suite green.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/menu_format.rs
git commit -m "fix: budget bar shows full when spending against zero budget; K rolls over to M"
```

---

## Out of Scope (deliberately)

- **Cache-read tokens in the "tok" figure**: `total_tokens()` includes cache reads (~90% of volume). Whether the tray should show "real" input/output separately is a product decision, not a bug — needs the user's call before touching.
- **opencode session-date attribution**: all of a session's usage is attributed to its creation date (documented tradeoff of the session-level SQLite read; opencode has no budget bars, so impact is limited to today/month bucketing).
- **Cross-file dedup**: not observed in the audit; per-file dedup captures the measured overcount.

## Self-Review Notes

- Coverage: every finding from the 2026-07-15 audit is either a task (dedup+synthetic → 1, local time → 2, frozen label → 3, hour anchor → 4, zero-budget bar + K/M rollover → 5) or explicitly listed out of scope with a reason.
- Type consistency: no public signature changes except `tray::build_menu` losing one parameter (Task 3 updates its only caller, `main.rs::refresh_all`, in the same task).
- Test determinism: every timezone-sensitive assertion goes through an `_in` variant with an explicit `FixedOffset`; nothing asserts through `chrono::Local`.
- Ordering: tasks are independent except 3 (depends on nothing but touches `menu_format.rs` like 5 — run 3 before 5 to avoid trivial merge friction) and 2/4 both touching `windows.rs` (run in order). Sequential execution 1→5 is safest.
