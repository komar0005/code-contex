# Real Claude Limits + Provider Abstraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show Anthropic's *real* 5h/7d rate-limit percentages and reset times in the tray (fetched from the Claude OAuth usage endpoint using Claude Code's own credentials), behind a small provider abstraction that replaces the hardcoded two-agent plumbing.

**Architecture:** A new `claude_oauth` module reads `~/.claude/.credentials.json` and calls `GET https://api.anthropic.com/api/oauth/usage` (pattern copied from steipete/codexbar's `ClaudeOAuthUsageFetcher`), producing a normalized `LimitsSnapshot`. A `Provider` trait (`gather_events` + `fetch_limits`) wraps the existing Claude/opencode scanners so `refresh_all` and `build_menu` iterate a `Vec<Box<dyn Provider>>` instead of two hardcoded paths. The menu shows real limit bars when available and falls back to today's heuristics labeled "(estimado)". A compact "5h 62% · 7d 34%" headline goes on the tray icon itself, and the LiteLLM pricing fetch is throttled to a daily cadence.

**Tech Stack:** Rust (Tauri 2 app in `src-tauri/`), reqwest 0.12 blocking (already a dependency), serde/serde_json, chrono. No new dependencies.

## Global Constraints

- All user-visible copy is **Spanish**, matching existing menu strings ("Hoy", "resetea en ~1h 12m").
- **No new Cargo dependencies.** Reuse `reqwest` (blocking), `serde`, `serde_json`, `chrono`, `dirs`, `tempfile` (dev).
- All tests run with `cargo test` from `src-tauri/`. Every task must end with the full suite green.
- Network calls happen only inside `refresh_all`/helpers, which already run via `tauri::async_runtime::spawn_blocking` — never on the main thread.
- The OAuth usage endpoint (`/api/oauth/usage`, beta header `oauth-2025-04-20`) is **undocumented**. Every failure (missing/expired creds, 401, 429, network, unparseable body) must degrade silently to the current local-estimate behavior — never crash, never block the menu, log to stderr only.
- New `Preferences` fields MUST carry `#[serde(default = ...)]` so an existing `preferences.json` written by the current version still loads (today the struct has no defaults; a missing field would reset ALL preferences).
- Commit messages follow the repo's conventional style (`feat:`, `fix:`, `refactor:`), each ending with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>`.
- The repo's default branch is `master` (there is no `main`). Work happens on the `feat/real-claude-limits` branch (created off `master` when the plan docs were committed); use it or a worktree per superpowers:using-git-worktrees.

## Reference: the OAuth usage endpoint (from codexbar research)

- Request: `GET https://api.anthropic.com/api/oauth/usage` with headers
  `Authorization: Bearer <accessToken>`, `Accept: application/json`,
  `anthropic-beta: oauth-2025-04-20`, `User-Agent: claude-code/2.1.0`.
- Token source on Linux: `~/.claude/.credentials.json` →
  `{"claudeAiOauth": {"accessToken": "...", "expiresAt": <epoch millis>, ...}}`
  (verified present on this machine with exactly these keys).
- Response (fields we consume): `five_hour` and `seven_day`, each
  `{"utilization": <0–100 float>, "resets_at": "<RFC3339>"}` or `null`.
  `five_hour: null` means "no active session" — it must NOT render as 0%.
- 429 responses may carry `Retry-After` (seconds). codexbar gates further
  requests until that deadline; we replicate that in-memory.

## File Structure

| File | Responsibility |
|---|---|
| `src-tauri/src/limits.rs` (new) | Normalized limit model: `RateWindow`, `LimitsSnapshot` |
| `src-tauri/src/claude_oauth.rs` (new) | Credentials file parsing, usage-response parsing, HTTP client with 429 gate |
| `src-tauri/src/provider.rs` (new) | `Provider` trait, `ClaudeProvider`, `OpenCodeProvider` |
| `src-tauri/src/summary.rs` | Add `AgentSection`; generalize `total_unpriced_this_month` |
| `src-tauri/src/main.rs` | `AppState.providers`, provider-iterating `refresh_all`, pricing throttle |
| `src-tauri/src/tray.rs` | Section-slice `build_menu`, real-limit lines |
| `src-tauri/src/menu_format.rs` | `format_limit_line`, `format_tray_title` |
| `src-tauri/src/preferences.rs` | `show_tray_metric` field |
| `ui/preferences.html` | Checkbox for the tray metric |

---

### Task 1: Claude OAuth credentials reading (`claude_oauth.rs` part 1)

**Files:**
- Create: `src-tauri/src/claude_oauth.rs`
- Modify: `src-tauri/src/main.rs` (add `mod claude_oauth;` to the module list at the top, alphabetically: after `mod cache;`)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces (used by Tasks 3–4):
  - `pub struct ClaudeCredentials { pub access_token: String, pub expires_at: Option<DateTime<Utc>> }`
  - `impl ClaudeCredentials { pub fn is_expired(&self, now: DateTime<Utc>) -> bool }`
  - `pub fn default_credentials_path() -> PathBuf`
  - `pub fn read_credentials(path: &Path) -> Option<ClaudeCredentials>`
  - `pub fn parse_credentials(content: &str) -> Option<ClaudeCredentials>`

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/claude_oauth.rs` containing ONLY the test module for now:

```rust
#[cfg(test)]
mod credentials_tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    const VALID_FIXTURE: &str = r#"{
        "claudeAiOauth": {
            "accessToken": "sk-ant-oat01-test-token",
            "refreshToken": "sk-ant-ort01-test-refresh",
            "expiresAt": 1784109600000,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max"
        }
    }"#;

    #[test]
    fn parses_token_and_expiry_from_claude_code_credentials_file() {
        let creds = parse_credentials(VALID_FIXTURE).unwrap();
        assert_eq!(creds.access_token, "sk-ant-oat01-test-token");
        // 1784109600000 ms = 2026-07-15T10:00:00Z
        assert_eq!(
            creds.expires_at,
            Some(Utc.with_ymd_and_hms(2026, 7, 15, 10, 0, 0).unwrap())
        );
    }

    #[test]
    fn missing_or_empty_token_returns_none() {
        assert!(parse_credentials(r#"{"claudeAiOauth": {"expiresAt": 1}}"#).is_none());
        assert!(parse_credentials(r#"{"claudeAiOauth": {"accessToken": ""}}"#).is_none());
        assert!(parse_credentials(r#"{}"#).is_none());
        assert!(parse_credentials("not json").is_none());
    }

    #[test]
    fn missing_expiry_still_yields_credentials() {
        let creds =
            parse_credentials(r#"{"claudeAiOauth": {"accessToken": "tok"}}"#).unwrap();
        assert_eq!(creds.expires_at, None);
        let now = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
        assert!(!creds.is_expired(now)); // unknown expiry -> assume valid, let the API 401
    }

    #[test]
    fn is_expired_compares_against_now() {
        let creds = parse_credentials(VALID_FIXTURE).unwrap();
        let before = Utc.with_ymd_and_hms(2026, 7, 15, 9, 0, 0).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 7, 15, 11, 0, 0).unwrap();
        assert!(!creds.is_expired(before));
        assert!(creds.is_expired(after));
    }

    #[test]
    fn read_credentials_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_credentials(&dir.path().join("nope.json")).is_none());
    }

    #[test]
    fn read_credentials_reads_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        std::fs::write(&path, VALID_FIXTURE).unwrap();
        assert!(read_credentials(&path).is_some());
    }
}
```

Add `mod claude_oauth;` to `src-tauri/src/main.rs`'s module list.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test claude_oauth`
Expected: compile error — `parse_credentials` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `src-tauri/src/claude_oauth.rs` (above the test module):

```rust
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Claude Code's OAuth credentials, read from the same file the CLI writes
/// (`~/.claude/.credentials.json` on Linux). We only borrow the access
/// token; we never write this file.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeCredentials {
    pub access_token: String,
    pub expires_at: Option<DateTime<Utc>>,
}

impl ClaudeCredentials {
    /// Unknown expiry counts as valid: better to attempt the request and
    /// let a 401 tell us, than to silently never fetch.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.map(|t| now >= t).unwrap_or(false)
    }
}

#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OauthSection>,
}

#[derive(Deserialize)]
struct OauthSection {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    /// Epoch milliseconds, as Claude Code writes it.
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
}

pub fn default_credentials_path() -> PathBuf {
    dirs::home_dir()
        .expect("home dir must resolve")
        .join(".claude")
        .join(".credentials.json")
}

pub fn read_credentials(path: &Path) -> Option<ClaudeCredentials> {
    parse_credentials(&std::fs::read_to_string(path).ok()?)
}

pub fn parse_credentials(content: &str) -> Option<ClaudeCredentials> {
    let file: CredentialsFile = serde_json::from_str(content).ok()?;
    let section = file.claude_ai_oauth?;
    let access_token = section.access_token?;
    if access_token.is_empty() {
        return None;
    }
    let expires_at = section
        .expires_at
        .and_then(|ms| DateTime::from_timestamp_millis(ms));
    Some(ClaudeCredentials { access_token, expires_at })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test claude_oauth`
Expected: 6 tests PASS. Then run `cargo test` (full suite) — all green. `dead_code` warnings for not-yet-used functions are acceptable at this stage.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/claude_oauth.rs src-tauri/src/main.rs
git commit -m "feat: read Claude Code OAuth credentials from ~/.claude/.credentials.json"
```

---

### Task 2: Limit model + usage-response parsing (`limits.rs`, `claude_oauth.rs` part 2)

**Files:**
- Create: `src-tauri/src/limits.rs`
- Modify: `src-tauri/src/claude_oauth.rs` (add parsing)
- Modify: `src-tauri/src/main.rs` (add `mod limits;`)

**Interfaces:**
- Consumes: nothing from Task 1 (parsing is independent of credentials).
- Produces (used by Tasks 3–6):
  - `limits::RateWindow { pub used_percent: f64, pub resets_at: Option<DateTime<Utc>> }` with `RateWindow::new(used_percent: f64, resets_at: Option<DateTime<Utc>>) -> Self` (clamps percent to [0, 100])
  - `limits::LimitsSnapshot { pub five_hour: Option<RateWindow>, pub seven_day: Option<RateWindow> }`
  - `claude_oauth::parse_usage_response(body: &str) -> Option<LimitsSnapshot>`

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/limits.rs`:

```rust
use chrono::{DateTime, Utc};

/// One provider rate-limit window as reported by the provider's API —
/// REAL account data, unlike the local heuristics in `windows.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct RateWindow {
    /// Percent of the window consumed, clamped to [0, 100].
    pub used_percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

impl RateWindow {
    pub fn new(used_percent: f64, resets_at: Option<DateTime<Utc>>) -> Self {
        Self { used_percent: used_percent.clamp(0.0, 100.0), resets_at }
    }
}

/// Real limit windows for one agent. A `None` window means the provider
/// did not report that lane (e.g. `five_hour: null` = no active session);
/// it must never be rendered as 0%.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitsSnapshot {
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_window_clamps_percent() {
        assert_eq!(RateWindow::new(150.0, None).used_percent, 100.0);
        assert_eq!(RateWindow::new(-5.0, None).used_percent, 0.0);
        assert_eq!(RateWindow::new(62.0, None).used_percent, 62.0);
    }
}
```

Append to the top-level of `src-tauri/src/claude_oauth.rs` a new test module:

```rust
#[cfg(test)]
mod usage_parsing_tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    const USAGE_FIXTURE: &str = r#"{
        "five_hour": {"utilization": 62.0, "resets_at": "2026-07-15T18:00:00Z"},
        "seven_day": {"utilization": 34.0, "resets_at": "2026-07-20T00:00:00+00:00"},
        "seven_day_opus": null,
        "extra_usage": {"is_enabled": false}
    }"#;

    #[test]
    fn parses_five_hour_and_seven_day_windows() {
        let snapshot = parse_usage_response(USAGE_FIXTURE).unwrap();
        let five = snapshot.five_hour.unwrap();
        assert_eq!(five.used_percent, 62.0);
        assert_eq!(
            five.resets_at,
            Some(Utc.with_ymd_and_hms(2026, 7, 15, 18, 0, 0).unwrap())
        );
        let seven = snapshot.seven_day.unwrap();
        assert_eq!(seven.used_percent, 34.0);
    }

    #[test]
    fn null_five_hour_means_no_active_session_not_zero() {
        let body = r#"{"five_hour": null, "seven_day": {"utilization": 10.0}}"#;
        let snapshot = parse_usage_response(body).unwrap();
        assert!(snapshot.five_hour.is_none());
        assert!(snapshot.seven_day.is_some());
    }

    #[test]
    fn missing_resets_at_is_tolerated() {
        let body = r#"{"five_hour": {"utilization": 5.0}}"#;
        let snapshot = parse_usage_response(body).unwrap();
        assert_eq!(snapshot.five_hour.unwrap().resets_at, None);
    }

    #[test]
    fn utilization_over_100_clamps() {
        let body = r#"{"five_hour": {"utilization": 130.0}}"#;
        assert_eq!(
            parse_usage_response(body).unwrap().five_hour.unwrap().used_percent,
            100.0
        );
    }

    #[test]
    fn garbage_or_windowless_bodies_return_none() {
        assert!(parse_usage_response("not json").is_none());
        assert!(parse_usage_response("{}").is_none());
        assert!(parse_usage_response(r#"{"five_hour": null, "seven_day": null}"#).is_none());
        assert!(parse_usage_response(r#"{"five_hour": {"no_utilization": 1}}"#).is_none());
    }
}
```

Add `mod limits;` to `src-tauri/src/main.rs`'s module list.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test usage_parsing`
Expected: compile error — `parse_usage_response` not found (the `limits::tests` compile and pass).

- [ ] **Step 3: Write the implementation**

Add to `src-tauri/src/claude_oauth.rs` (below the credentials code, above the test modules), and extend the `use` block at the top of the file with `use crate::limits::{LimitsSnapshot, RateWindow};`:

```rust
/// Parses the body of `GET /api/oauth/usage`. Tolerant by design: unknown
/// fields are ignored, `null` windows stay `None`, and a body with no
/// recognizable window at all yields `None` (treated as a failed fetch).
/// `utilization` is a 0–100 percent, as codexbar consumes it.
pub fn parse_usage_response(body: &str) -> Option<LimitsSnapshot> {
    let root: serde_json::Value = serde_json::from_str(body).ok()?;
    let five_hour = parse_window(root.get("five_hour"));
    let seven_day = parse_window(root.get("seven_day"));
    if five_hour.is_none() && seven_day.is_none() {
        return None;
    }
    Some(LimitsSnapshot { five_hour, seven_day })
}

fn parse_window(value: Option<&serde_json::Value>) -> Option<RateWindow> {
    let object = value?.as_object()?;
    let utilization = object.get("utilization")?.as_f64()?;
    let resets_at = object
        .get("resets_at")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    Some(RateWindow::new(utilization, resets_at))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test`
Expected: all tests PASS, including the 5 new `usage_parsing_tests` and `limits::tests`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/limits.rs src-tauri/src/claude_oauth.rs src-tauri/src/main.rs
git commit -m "feat: parse Claude OAuth usage response into normalized limit windows"
```

---

### Task 3: OAuth usage HTTP client with 429 gate (`claude_oauth.rs` part 3)

**Files:**
- Modify: `src-tauri/src/claude_oauth.rs`

**Interfaces:**
- Consumes: `parse_usage_response` (Task 2), `LimitsSnapshot` (Task 2).
- Produces (used by Task 4):
  - `pub trait UsageSource: Send { fn fetch(&self, access_token: &str) -> Result<UsageHttpResponse, String>; }`
  - `pub struct UsageHttpResponse { pub status: u16, pub body: String, pub retry_after_secs: Option<u64> }`
  - `pub struct HttpUsageSource { pub url: String }` implementing `UsageSource`
  - `pub const USAGE_URL: &str`
  - `pub enum FetchOutcome { Success(LimitsSnapshot), Unauthorized, RateLimited, Failed }`
  - `pub struct OauthUsageClient` with `new(source: Box<dyn UsageSource>) -> Self` and `fetch(&mut self, access_token: &str, now: DateTime<Utc>) -> FetchOutcome`

The `UsageSource` trait mirrors the existing `PriceSource` pattern in `price_fetch.rs` — same testing approach with a fake source.

- [ ] **Step 1: Write the failing tests**

Append to `src-tauri/src/claude_oauth.rs`:

```rust
#[cfg(test)]
mod client_tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    struct FakeSource {
        response: Result<(u16, String, Option<u64>), String>,
        calls: Arc<AtomicU32>,
    }

    impl FakeSource {
        fn new(response: Result<(u16, String, Option<u64>), String>) -> Self {
            Self { response, calls: Arc::new(AtomicU32::new(0)) }
        }
    }

    impl UsageSource for FakeSource {
        fn fetch(&self, _access_token: &str) -> Result<UsageHttpResponse, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.response.clone().map(|(status, body, retry_after_secs)| UsageHttpResponse {
                status,
                body,
                retry_after_secs,
            })
        }
    }

    const OK_BODY: &str = r#"{"five_hour": {"utilization": 42.0}}"#;

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap()
    }

    #[test]
    fn success_returns_snapshot() {
        let mut client =
            OauthUsageClient::new(Box::new(FakeSource::new(Ok((200, OK_BODY.into(), None)))));
        match client.fetch("tok", now()) {
            FetchOutcome::Success(snapshot) => {
                assert_eq!(snapshot.five_hour.unwrap().used_percent, 42.0);
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[test]
    fn http_401_maps_to_unauthorized() {
        let mut client =
            OauthUsageClient::new(Box::new(FakeSource::new(Ok((401, String::new(), None)))));
        assert_eq!(client.fetch("tok", now()), FetchOutcome::Unauthorized);
    }

    #[test]
    fn network_error_and_bad_body_map_to_failed() {
        let mut client =
            OauthUsageClient::new(Box::new(FakeSource::new(Err("timeout".into()))));
        assert_eq!(client.fetch("tok", now()), FetchOutcome::Failed);

        let mut client =
            OauthUsageClient::new(Box::new(FakeSource::new(Ok((200, "{}".into(), None)))));
        assert_eq!(client.fetch("tok", now()), FetchOutcome::Failed);
    }

    #[test]
    fn http_429_blocks_further_calls_until_retry_after() {
        let source = Box::new(FakeSource::new(Ok((429, String::new(), Some(600)))));
        let mut client = OauthUsageClient::new(source);
        assert_eq!(client.fetch("tok", now()), FetchOutcome::RateLimited);

        // Within the block window: RateLimited WITHOUT touching the source.
        assert_eq!(
            client.fetch("tok", now() + Duration::seconds(30)),
            FetchOutcome::RateLimited
        );
        // After the window: the source is consulted again.
        assert_eq!(
            client.fetch("tok", now() + Duration::seconds(601)),
            FetchOutcome::RateLimited // fake still answers 429
        );
    }

    #[test]
    fn gate_skips_source_call_while_blocked() {
        let source = FakeSource::new(Ok((429, String::new(), Some(600))));
        let calls = source.calls.clone();
        let mut client = OauthUsageClient::new(Box::new(source));
        client.fetch("tok", now());
        client.fetch("tok", now() + Duration::seconds(30));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn missing_retry_after_uses_default_and_caps_at_one_hour() {
        let mut client =
            OauthUsageClient::new(Box::new(FakeSource::new(Ok((429, String::new(), None)))));
        client.fetch("tok", now());
        // Default gate is 300s: blocked at +299s, open again at +301s
        // (the fake then answers 429 again, which is fine — we only care
        // that the source was consulted, proven by gate_skips_source_call).
        assert_eq!(
            client.fetch("tok", now() + Duration::seconds(299)),
            FetchOutcome::RateLimited
        );

        let mut client = OauthUsageClient::new(Box::new(FakeSource::new(Ok((
            429,
            String::new(),
            Some(86_400), // absurd Retry-After: must cap at 3600s
        )))));
        client.fetch("tok", now());
        match client.fetch("tok", now() + Duration::seconds(3601)) {
            FetchOutcome::RateLimited => {} // consulted again, fake replies 429
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test client_tests`
Expected: compile error — `UsageSource`, `OauthUsageClient` not found.

- [ ] **Step 3: Write the implementation**

Add to `src-tauri/src/claude_oauth.rs` (above the test modules). Also add `#[derive(Debug, ...)]` requirements shown:

```rust
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// The endpoint currently requires this beta header (see codexbar's
/// ClaudeOAuthUsageFetcher, which this module mirrors).
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";
/// Fallback UA; the endpoint expects a claude-code client string.
const CLAUDE_CODE_USER_AGENT: &str = "claude-code/2.1.0";
const DEFAULT_RATE_LIMIT_GATE_SECS: u64 = 300;
const MAX_RATE_LIMIT_GATE_SECS: u64 = 3600;

pub struct UsageHttpResponse {
    pub status: u16,
    pub body: String,
    pub retry_after_secs: Option<u64>,
}

/// Abstracts the HTTP round-trip so `OauthUsageClient` is testable with a
/// fake — same pattern as `price_fetch::PriceSource`.
pub trait UsageSource: Send {
    fn fetch(&self, access_token: &str) -> Result<UsageHttpResponse, String>;
}

pub struct HttpUsageSource {
    pub url: String,
}

impl UsageSource for HttpUsageSource {
    fn fetch(&self, access_token: &str) -> Result<UsageHttpResponse, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .get(&self.url)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/json")
            .header("anthropic-beta", OAUTH_BETA_HEADER)
            .header("User-Agent", CLAUDE_CODE_USER_AGENT)
            .send()
            .map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let retry_after_secs = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse().ok());
        let body = response.text().map_err(|e| e.to_string())?;
        Ok(UsageHttpResponse { status, body, retry_after_secs })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchOutcome {
    Success(LimitsSnapshot),
    /// Token rejected — the user must run `claude` to re-authenticate.
    Unauthorized,
    /// Anthropic rate-limited us; a gate suppresses retries for a while.
    RateLimited,
    Failed,
}

pub struct OauthUsageClient {
    source: Box<dyn UsageSource>,
    blocked_until: Option<DateTime<Utc>>,
}

impl OauthUsageClient {
    pub fn new(source: Box<dyn UsageSource>) -> Self {
        Self { source, blocked_until: None }
    }

    pub fn fetch(&mut self, access_token: &str, now: DateTime<Utc>) -> FetchOutcome {
        if let Some(until) = self.blocked_until {
            if now < until {
                return FetchOutcome::RateLimited;
            }
        }
        let response = match self.source.fetch(access_token) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("claude oauth usage: fetch failed: {e}");
                return FetchOutcome::Failed;
            }
        };
        match response.status {
            200 => {
                self.blocked_until = None;
                match parse_usage_response(&response.body) {
                    Some(snapshot) => FetchOutcome::Success(snapshot),
                    None => {
                        eprintln!("claude oauth usage: unparseable 200 body");
                        FetchOutcome::Failed
                    }
                }
            }
            401 => FetchOutcome::Unauthorized,
            429 => {
                let secs = response
                    .retry_after_secs
                    .unwrap_or(DEFAULT_RATE_LIMIT_GATE_SECS)
                    .min(MAX_RATE_LIMIT_GATE_SECS);
                self.blocked_until = Some(now + chrono::Duration::seconds(secs as i64));
                FetchOutcome::RateLimited
            }
            other => {
                eprintln!("claude oauth usage: HTTP {other}");
                FetchOutcome::Failed
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test`
Expected: all PASS (6 new client tests included).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/claude_oauth.rs
git commit -m "feat: OAuth usage HTTP client with 429 retry-after gate"
```

---

### Task 4: Provider trait + refactor `refresh_all`/`build_menu` to iterate providers

**Files:**
- Create: `src-tauri/src/provider.rs`
- Modify: `src-tauri/src/summary.rs` (add `AgentSection`, generalize `total_unpriced_this_month`)
- Modify: `src-tauri/src/main.rs` (AppState, `refresh_all`, remove `gather_*` fns, add `mod provider;`)
- Modify: `src-tauri/src/tray.rs` (`build_menu` takes `&[AgentSection]`)

**Interfaces:**
- Consumes: `claude_oauth::{default_credentials_path, read_credentials, FetchOutcome, HttpUsageSource, OauthUsageClient, UsageSource, USAGE_URL}` (Tasks 1–3), `limits::LimitsSnapshot` (Task 2).
- Produces (used by Tasks 5–6):
  - `provider::Provider` trait: `fn agent(&self) -> Agent; fn gather_events(&mut self) -> Vec<UsageEvent>; fn fetch_limits(&mut self, now: DateTime<Utc>) -> Option<LimitsSnapshot>;`
  - `provider::ClaudeProvider::new(projects_dir: PathBuf, credentials_path: PathBuf) -> Self` and `::with_source(projects_dir, credentials_path, source: Box<dyn UsageSource>) -> Self`
  - `provider::OpenCodeProvider::new(db_path: PathBuf) -> Self`
  - `summary::AgentSection { pub summary: AgentSummary, pub limits: Option<LimitsSnapshot> }`
  - `summary::total_unpriced_this_month<'a>(summaries: impl Iterator<Item = &'a AgentSummary>) -> u64`
  - `tray::build_menu(app: &AppHandle, sections: &[AgentSection], prefs: &Preferences, now: DateTime<Utc>) -> tauri::Result<Menu<Wry>>`

Menu CONTENT does not change in this task — only plumbing. `AgentSection.limits` is carried but not yet rendered (Task 5 does that); expect a temporary `dead_code`-free build because the field is constructed in `main.rs`.

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/provider.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_oauth::{UsageHttpResponse, UsageSource};
    use chrono::{TimeZone, Utc};

    struct FakeUsageSource {
        status: u16,
        body: String,
    }

    impl UsageSource for FakeUsageSource {
        fn fetch(&self, _access_token: &str) -> Result<UsageHttpResponse, String> {
            Ok(UsageHttpResponse {
                status: self.status,
                body: self.body.clone(),
                retry_after_secs: None,
            })
        }
    }

    const CREDS: &str = r#"{"claudeAiOauth": {"accessToken": "tok", "expiresAt": 4102444800000}}"#;
    const EXPIRED_CREDS: &str = r#"{"claudeAiOauth": {"accessToken": "tok", "expiresAt": 1000}}"#;

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap()
    }

    #[test]
    fn claude_provider_fetches_limits_with_valid_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, CREDS).unwrap();
        let mut provider = ClaudeProvider::with_source(
            dir.path().join("projects"),
            creds_path,
            Box::new(FakeUsageSource {
                status: 200,
                body: r#"{"five_hour": {"utilization": 42.0}}"#.into(),
            }),
        );
        let limits = provider.fetch_limits(now()).unwrap();
        assert_eq!(limits.five_hour.unwrap().used_percent, 42.0);
    }

    #[test]
    fn claude_provider_returns_none_without_credentials_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut provider = ClaudeProvider::with_source(
            dir.path().join("projects"),
            dir.path().join("missing.json"),
            Box::new(FakeUsageSource { status: 200, body: "{}".into() }),
        );
        assert!(provider.fetch_limits(now()).is_none());
    }

    #[test]
    fn claude_provider_returns_none_with_expired_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, EXPIRED_CREDS).unwrap();
        let mut provider = ClaudeProvider::with_source(
            dir.path().join("projects"),
            creds_path,
            Box::new(FakeUsageSource { status: 200, body: "{}".into() }),
        );
        assert!(provider.fetch_limits(now()).is_none());
    }

    #[test]
    fn claude_provider_returns_none_on_unauthorized() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, CREDS).unwrap();
        let mut provider = ClaudeProvider::with_source(
            dir.path().join("projects"),
            creds_path,
            Box::new(FakeUsageSource { status: 401, body: String::new() }),
        );
        assert!(provider.fetch_limits(now()).is_none());
    }

    #[test]
    fn claude_provider_gathers_no_events_from_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut provider = ClaudeProvider::with_source(
            dir.path().join("no-projects"),
            dir.path().join("no-creds.json"),
            Box::new(FakeUsageSource { status: 200, body: "{}".into() }),
        );
        assert!(provider.gather_events().is_empty());
    }

    #[test]
    fn opencode_provider_has_no_limits_and_tolerates_missing_db() {
        let dir = tempfile::tempdir().unwrap();
        let mut provider = OpenCodeProvider::new(dir.path().join("opencode.db"));
        assert!(provider.fetch_limits(now()).is_none());
        assert!(provider.gather_events().is_empty());
        assert_eq!(provider.agent(), crate::model::Agent::OpenCode);
    }
}
```

In `src-tauri/src/summary.rs`, REPLACE the existing `total_unpriced_this_month` tests' call sites (see Step 3) and add this test to the existing `tests` module:

```rust
    #[test]
    fn total_unpriced_this_month_over_iterator() {
        let table = embedded_pricing_table();
        let now = Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap();
        let events = vec![event("proj-a", "unknown-model", now)];
        let summary = build_summary(Agent::ClaudeCode, &events, &table, now).unwrap();
        assert_eq!(total_unpriced_this_month([&summary].into_iter()), 1);
        assert_eq!(total_unpriced_this_month(std::iter::empty()), 0);
    }
```

Add `mod provider;` to `src-tauri/src/main.rs`'s module list.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test provider`
Expected: compile error — `ClaudeProvider` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `src-tauri/src/provider.rs`:

```rust
use crate::cache::FileCache;
use crate::claude_oauth::{
    self, FetchOutcome, HttpUsageSource, OauthUsageClient, UsageSource,
};
use crate::limits::LimitsSnapshot;
use crate::model::{Agent, UsageEvent};
use crate::parsers;
use chrono::{DateTime, Utc};
use std::path::PathBuf;

/// One tracked AI agent: knows how to gather its local usage events and,
/// when the provider exposes one, fetch real rate-limit data from its API.
pub trait Provider: Send {
    fn agent(&self) -> Agent;
    /// Scans this provider's local data source for usage events.
    fn gather_events(&mut self) -> Vec<UsageEvent>;
    /// Real limit windows from the provider's API. `None` means
    /// unsupported, unavailable, or failed — callers fall back to local
    /// estimates.
    fn fetch_limits(&mut self, now: DateTime<Utc>) -> Option<LimitsSnapshot>;
}

pub struct ClaudeProvider {
    projects_dir: PathBuf,
    credentials_path: PathBuf,
    cache: FileCache<UsageEvent>,
    usage_client: OauthUsageClient,
}

impl ClaudeProvider {
    pub fn new(projects_dir: PathBuf, credentials_path: PathBuf) -> Self {
        Self::with_source(
            projects_dir,
            credentials_path,
            Box::new(HttpUsageSource { url: claude_oauth::USAGE_URL.to_string() }),
        )
    }

    pub fn with_source(
        projects_dir: PathBuf,
        credentials_path: PathBuf,
        source: Box<dyn UsageSource>,
    ) -> Self {
        Self {
            projects_dir,
            credentials_path,
            cache: FileCache::new(),
            usage_client: OauthUsageClient::new(source),
        }
    }
}

impl Provider for ClaudeProvider {
    fn agent(&self) -> Agent {
        Agent::ClaudeCode
    }

    fn gather_events(&mut self) -> Vec<UsageEvent> {
        let mut events = Vec::new();
        for file in parsers::claude_code::discover_files(&self.projects_dir) {
            let project = parsers::claude_code::folder_slug_project_name(&file);
            let parsed = self.cache.get_or_parse(&file, |content| {
                parsers::claude_code::parse_jsonl_content(content, &project)
            });
            events.extend(parsed);
        }
        events
    }

    fn fetch_limits(&mut self, now: DateTime<Utc>) -> Option<LimitsSnapshot> {
        let creds = claude_oauth::read_credentials(&self.credentials_path)?;
        if creds.is_expired(now) {
            return None;
        }
        match self.usage_client.fetch(&creds.access_token, now) {
            FetchOutcome::Success(snapshot) => Some(snapshot),
            FetchOutcome::Unauthorized => {
                eprintln!("claude oauth usage: token rejected; run `claude` to re-authenticate");
                None
            }
            FetchOutcome::RateLimited | FetchOutcome::Failed => None,
        }
    }
}

pub struct OpenCodeProvider {
    db_path: PathBuf,
}

impl OpenCodeProvider {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }
}

impl Provider for OpenCodeProvider {
    fn agent(&self) -> Agent {
        Agent::OpenCode
    }

    fn gather_events(&mut self) -> Vec<UsageEvent> {
        match parsers::opencode::open_read_only(&self.db_path) {
            Some(conn) => parsers::opencode::load_all(&conn),
            None => Vec::new(),
        }
    }

    fn fetch_limits(&mut self, _now: DateTime<Utc>) -> Option<LimitsSnapshot> {
        None
    }
}
```

In `src-tauri/src/summary.rs`:
- Add `use crate::limits::LimitsSnapshot;` to the imports.
- Add below `AgentSummary`:

```rust
/// Everything the menu needs for one agent: local aggregates plus, when
/// available, real limit windows from the provider's API.
#[derive(Debug, Clone)]
pub struct AgentSection {
    pub summary: AgentSummary,
    pub limits: Option<LimitsSnapshot>,
}
```

- Replace `total_unpriced_this_month` (function AND doc comment) with:

```rust
/// Sums `unpriced_count` across all agents' current-month windows, i.e. how
/// many events this month had a model that wasn't in the pricing table (so
/// their cost couldn't be calculated).
pub fn total_unpriced_this_month<'a>(
    summaries: impl Iterator<Item = &'a AgentSummary>,
) -> u64 {
    summaries.map(|s| s.month.unpriced_count).sum()
}
```

- Delete the old `total_unpriced_this_month_sums_both_agents_and_handles_absence` test (its Option-based signature no longer exists); the new iterator test from Step 1 replaces it.

In `src-tauri/src/tray.rs`:
- Change imports: `use crate::summary::AgentSection;` (replacing `AgentSummary`).
- Change `append_agent_section` to take `section: &AgentSection` and read `let summary = &section.summary;` as its first line (body otherwise references `summary` exactly as today).
- Replace `build_menu`'s signature and body:

```rust
pub fn build_menu(
    app: &AppHandle,
    sections: &[AgentSection],
    prefs: &Preferences,
    now: DateTime<Utc>,
) -> tauri::Result<Menu<Wry>> {
    let mut builder = MenuBuilder::new(app);

    if sections.is_empty() {
        let empty = MenuItemBuilder::new(EMPTY_STATE_MESSAGE).enabled(false).build(app)?;
        builder = builder.item(&empty).separator();
    } else {
        for section in sections {
            builder = append_agent_section(app, builder, section, prefs, now)?;
        }
    }
    // ... the trailing refreshed/preferences/refresh/quit items are unchanged ...
```

In `src-tauri/src/main.rs`:
- Module list gains `mod provider;` (Task's Step 1) — imports gain `use provider::{ClaudeProvider, OpenCodeProvider, Provider};`, `use summary::AgentSection;`, `use model::Agent;` adjusted as needed (drop unused `UsageEvent` import if the compiler flags it; keep what's used).
- `AppState`: replace `pub claude_cache: Mutex<FileCache<UsageEvent>>` with `pub providers: Mutex<Vec<Box<dyn Provider>>>`. Remove the now-unused `use cache::FileCache;` import.
- Delete `gather_claude_events` and `gather_opencode_events`.
- Replace the body of `refresh_all` up to the `build_menu` call with:

```rust
pub fn refresh_all(app: &AppHandle) {
    let state = app.state::<AppState>();
    let now = Utc::now();

    let gathered: Vec<(Agent, Vec<UsageEvent>, Option<crate::limits::LimitsSnapshot>)> = {
        let mut providers = state.providers.lock().unwrap();
        providers
            .iter_mut()
            .map(|p| (p.agent(), p.gather_events(), p.fetch_limits(now)))
            .collect()
    };

    let prefs = state.preferences.lock().unwrap().clone();
    if prefs.network_pricing_refresh_enabled {
        let current = state.pricing.lock().unwrap().clone();
        let source = price_fetch::HttpPriceSource {
            url: "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json".to_string(),
        };
        let (refreshed, succeeded) = price_fetch::refresh_pricing_table_with_status(&source, current);
        *state.pricing.lock().unwrap() = refreshed;
        if succeeded {
            *state.last_pricing_update.lock().unwrap() = Some(now);
        }
    }
    let pricing = state.pricing.lock().unwrap().clone();

    let sections: Vec<AgentSection> = gathered
        .into_iter()
        .filter_map(|(agent, events, limits)| {
            summary::build_summary(agent, &events, &pricing, now)
                .map(|summary| AgentSection { summary, limits })
        })
        .collect();

    let unpriced_count =
        summary::total_unpriced_this_month(sections.iter().map(|s| &s.summary));
    *state.unpriced_count.lock().unwrap() = unpriced_count;

    if let Ok(menu) = tray::build_menu(app, &sections, &prefs, now) {
        if let Some(tray_icon) = state.tray.lock().unwrap().as_ref() {
            let _ = tray_icon.set_menu(Some(menu));
        }
    }
}
```

- In `main()`, replace the `claude_cache` field in the `.manage(AppState { ... })` call with:

```rust
            providers: Mutex::new(vec![
                Box::new(ClaudeProvider::new(
                    claude_projects_dir(),
                    claude_oauth::default_credentials_path(),
                )) as Box<dyn Provider>,
                Box::new(OpenCodeProvider::new(opencode_db_path())),
            ]),
```

- [ ] **Step 4: Run tests and build to verify**

Run: `cd src-tauri && cargo test && cargo check`
Expected: all tests PASS (6 new provider tests, updated summary test); `cargo check` clean (no leftover references to `claude_cache`/`gather_claude_events`).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/provider.rs src-tauri/src/summary.rs src-tauri/src/main.rs src-tauri/src/tray.rs
git commit -m "refactor: provider trait replaces hardcoded two-agent plumbing; carry real limits"
```

---

### Task 5: Render real limit bars in the menu, label heuristics "(estimado)"

**Files:**
- Modify: `src-tauri/src/menu_format.rs` (add `format_limit_line`)
- Modify: `src-tauri/src/tray.rs` (render limits in `append_agent_section`)

**Interfaces:**
- Consumes: `limits::RateWindow` (Task 2), `AgentSection` (Task 4), existing `format_reset_in`, `format_budget_line`.
- Produces: `menu_format::format_limit_line(label: &str, window: &RateWindow) -> String`.

Menu design (Claude section, top to bottom):
- With real limits: `Límite 5h  [██████░░░░] 62%` → indented reset line → `Límite 7d  [███░░░░░░░] 34%` → indented reset line (each reset line only if `resets_at` present). Personal-budget cost lines (`Bloque 5h $x/$y`, `7 días $x/$y`) stay below them, WITHOUT the old heuristic reset line (the real one above replaces it).
- Without real limits: exactly today's menu, except the heuristic 5h reset line gains the suffix `" (estimado)"`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src-tauri/src/menu_format.rs`:

```rust
    #[test]
    fn format_limit_line_renders_percent_bar() {
        use crate::limits::RateWindow;
        assert_eq!(
            format_limit_line("Límite 5h", &RateWindow::new(62.0, None)),
            "Límite 5h  [██████░░░░] 62%"
        );
        assert_eq!(
            format_limit_line("Límite 7d", &RateWindow::new(0.0, None)),
            "Límite 7d  [░░░░░░░░░░] 0%"
        );
        assert_eq!(
            format_limit_line("Límite 5h", &RateWindow::new(100.0, None)),
            "Límite 5h  [██████████] 100%"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test format_limit_line`
Expected: compile error — `format_limit_line` not found.

- [ ] **Step 3: Write the implementation**

In `src-tauri/src/menu_format.rs`, add `use crate::limits::RateWindow;` to the imports and add below `format_budget_line`:

```rust
/// Renders a 10-segment bar from a REAL provider-reported used-percent
/// (0–100, already clamped by `RateWindow::new`) — unlike
/// `format_budget_line`, which compares spend against a personal budget.
pub fn format_limit_line(label: &str, window: &RateWindow) -> String {
    let filled = (window.used_percent / 10.0).round() as usize;
    let bar: String = "█".repeat(filled) + &"░".repeat(10 - filled);
    format!("{label}  [{bar}] {:.0}%", window.used_percent)
}
```

In `src-tauri/src/tray.rs`, add `format_limit_line` to the `crate::menu_format` import list, and replace the whole `if summary.agent == Agent::ClaudeCode { ... }` block inside `append_agent_section` with:

```rust
    if summary.agent == Agent::ClaudeCode {
        if let Some(limits) = &section.limits {
            for (label, window) in [
                ("Límite 5h", &limits.five_hour),
                ("Límite 7d", &limits.seven_day),
            ] {
                if let Some(window) = window {
                    let line = MenuItemBuilder::new(format_limit_line(label, window))
                        .enabled(false)
                        .build(app)?;
                    builder = builder.item(&line);
                    if let Some(resets_at) = window.resets_at {
                        let reset_line =
                            MenuItemBuilder::new(format!("   {}", format_reset_in(resets_at, now)))
                                .enabled(false)
                                .build(app)?;
                        builder = builder.item(&reset_line);
                    }
                }
            }
        }

        if let Some((block_cost, reset_at)) = &summary.active_5h_block {
            let block_line = MenuItemBuilder::new(format_budget_line(
                "Bloque 5h",
                block_cost.cost,
                prefs.budget_5h_usd,
            ))
            .enabled(false)
            .build(app)?;
            builder = builder.item(&block_line);

            // The heuristic reset estimate is redundant (and possibly
            // contradictory) when the real reset time is shown above.
            if section.limits.is_none() {
                let reset_line = MenuItemBuilder::new(format!(
                    "   {} (estimado)",
                    format_reset_in(*reset_at, now)
                ))
                .enabled(false)
                .build(app)?;
                builder = builder.item(&reset_line);
            }
        }

        let week_line = MenuItemBuilder::new(format_budget_line(
            "7 días",
            summary.last_7_days.cost,
            prefs.budget_7d_usd,
        ))
        .enabled(false)
        .build(app)?;
        builder = builder.item(&week_line);
    }
```

- [ ] **Step 4: Run tests and build**

Run: `cd src-tauri && cargo test && cargo check`
Expected: all PASS, clean check.

- [ ] **Step 5: Manual smoke test**

Run: `cd src-tauri && cargo tauri dev` (or `cargo run`). Open the tray menu:
- If `~/.claude/.credentials.json` holds a live token: Claude section shows `Límite 5h`/`Límite 7d` bars with a reset countdown matching claude.ai's usage page.
- Disconnect network or rename the credentials file and hit "⟳ Refrescar": section falls back to today's lines with `(estimado)` on the 5h reset.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/menu_format.rs src-tauri/src/tray.rs
git commit -m "feat: show real Claude 5h/7d limit bars in menu; label heuristic fallback (estimado)"
```

---

### Task 6: Tray headline metric ("5h 62% · 7d 34%") with preference toggle

**Files:**
- Modify: `src-tauri/src/menu_format.rs` (add `format_tray_title`)
- Modify: `src-tauri/src/preferences.rs` (add `show_tray_metric`)
- Modify: `src-tauri/src/main.rs` (set title/tooltip in `refresh_all`)
- Modify: `ui/preferences.html` (checkbox)

**Interfaces:**
- Consumes: `LimitsSnapshot` (Task 2), `AgentSection` (Task 4).
- Produces: `menu_format::format_tray_title(limits: Option<&LimitsSnapshot>) -> Option<String>`, `Preferences.show_tray_metric: bool`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src-tauri/src/menu_format.rs`:

```rust
    #[test]
    fn format_tray_title_composes_available_windows() {
        use crate::limits::{LimitsSnapshot, RateWindow};
        let full = LimitsSnapshot {
            five_hour: Some(RateWindow::new(62.0, None)),
            seven_day: Some(RateWindow::new(34.0, None)),
        };
        assert_eq!(format_tray_title(Some(&full)), Some("5h 62% · 7d 34%".to_string()));

        let weekly_only = LimitsSnapshot {
            five_hour: None,
            seven_day: Some(RateWindow::new(34.0, None)),
        };
        assert_eq!(format_tray_title(Some(&weekly_only)), Some("7d 34%".to_string()));

        assert_eq!(format_tray_title(None), None);
        let empty = LimitsSnapshot { five_hour: None, seven_day: None };
        assert_eq!(format_tray_title(Some(&empty)), None);
    }
```

Add to the `tests` module in `src-tauri/src/preferences.rs`:

```rust
    #[test]
    fn preferences_json_without_show_tray_metric_defaults_to_true() {
        // A file written by the previous app version must still load.
        let dir = tempfile::tempdir().unwrap();
        let legacy = r#"{
            "budget_5h_usd": 25.0,
            "budget_7d_usd": 100.0,
            "budget_monthly_usd": 300.0,
            "refresh_interval_secs": 30,
            "network_pricing_refresh_enabled": false
        }"#;
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(preferences_path(dir.path()), legacy).unwrap();
        let prefs = load(dir.path());
        assert!(prefs.show_tray_metric);
        assert_eq!(prefs.budget_5h_usd, 25.0); // other fields preserved
    }
```

Also extend the existing `save_then_load_round_trips` test's `Preferences` literal with `show_tray_metric: false,`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test`
Expected: compile errors — `format_tray_title` not found; `Preferences` has no field `show_tray_metric`.

- [ ] **Step 3: Write the implementation**

`src-tauri/src/preferences.rs` — add the field with a serde default (constraint: old files must load):

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preferences {
    pub budget_5h_usd: f64,
    pub budget_7d_usd: f64,
    pub budget_monthly_usd: f64,
    pub refresh_interval_secs: u64,
    pub network_pricing_refresh_enabled: bool,
    /// Show "5h 62% · 7d 34%" next to the tray icon (where the desktop
    /// environment supports appindicator labels).
    #[serde(default = "default_show_tray_metric")]
    pub show_tray_metric: bool,
}

fn default_show_tray_metric() -> bool {
    true
}
```

And `show_tray_metric: true,` in the `Default` impl.

`src-tauri/src/menu_format.rs` — extend the limits import to `use crate::limits::{LimitsSnapshot, RateWindow};` and add:

```rust
/// Compact headline for the tray icon itself, e.g. "5h 62% · 7d 34%".
/// `None` when no real limit data is available — the tray then shows only
/// the icon, never a stale or estimated number.
pub fn format_tray_title(limits: Option<&LimitsSnapshot>) -> Option<String> {
    let limits = limits?;
    let mut parts: Vec<String> = Vec::new();
    if let Some(window) = &limits.five_hour {
        parts.push(format!("5h {:.0}%", window.used_percent));
    }
    if let Some(window) = &limits.seven_day {
        parts.push(format!("7d {:.0}%", window.used_percent));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}
```

`src-tauri/src/main.rs` — replace the menu-installation block at the end of `refresh_all` with:

```rust
    if let Ok(menu) = tray::build_menu(app, &sections, &prefs, now) {
        if let Some(tray_icon) = state.tray.lock().unwrap().as_ref() {
            let _ = tray_icon.set_menu(Some(menu));
            let title = if prefs.show_tray_metric {
                sections
                    .iter()
                    .find_map(|s| menu_format::format_tray_title(s.limits.as_ref()))
            } else {
                None
            };
            // Not every Linux DE renders appindicator labels/tooltips;
            // failures are cosmetic, so ignore them.
            let _ = tray_icon.set_title(title.as_deref());
            let _ = tray_icon.set_tooltip(title.as_deref());
        }
    }
```

`ui/preferences.html` — under the "Refresco" section, after the `networkPricing` label, add:

```html
  <label><input id="showTrayMetric" type="checkbox" /> Mostrar % de límite junto al icono</label>
```

In the `load()` function add:

```js
      document.getElementById("showTrayMetric").checked = prefs.show_tray_metric;
```

In the save handler's `prefs` object add:

```js
        show_tray_metric: document.getElementById("showTrayMetric").checked,
```

- [ ] **Step 4: Run tests and build**

Run: `cd src-tauri && cargo test && cargo check`
Expected: all PASS.

- [ ] **Step 5: Manual smoke test**

Run the app. With a live token, the tray icon shows "5h N% · 7d M%" beside it (DE permitting). Toggle the new checkbox off in Preferencias, save, and confirm the label disappears on the next refresh.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/menu_format.rs src-tauri/src/preferences.rs src-tauri/src/main.rs ui/preferences.html
git commit -m "feat: show limit percentages next to the tray icon (toggleable)"
```

---

### Task 7: Throttle pricing refresh to a daily cadence

**Files:**
- Modify: `src-tauri/src/main.rs`

**Interfaces:**
- Consumes: existing `price_fetch::refresh_pricing_table_with_status`, `AppState.last_pricing_update`.
- Produces: `main::should_refresh_pricing(last_attempt: Option<DateTime<Utc>>, last_success: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool`; new `AppState.last_pricing_attempt: Mutex<Option<DateTime<Utc>>>`.

Today the LiteLLM table (a multi-MB download) is fetched on EVERY refresh cycle (default: every 60s). Pricing changes rarely; fetch at most daily, retrying hourly after failures.

- [ ] **Step 1: Write the failing test**

Add at the bottom of `src-tauri/src/main.rs`:

```rust
#[cfg(test)]
mod pricing_throttle_tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn t0() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap()
    }

    #[test]
    fn refreshes_when_never_attempted() {
        assert!(should_refresh_pricing(None, None, t0()));
    }

    #[test]
    fn skips_when_success_is_fresh() {
        let last = Some(t0() - Duration::hours(2));
        assert!(!should_refresh_pricing(last, last, t0()));
    }

    #[test]
    fn refreshes_when_success_is_a_day_old() {
        let success = Some(t0() - Duration::hours(25));
        assert!(should_refresh_pricing(success, success, t0()));
    }

    #[test]
    fn failed_attempt_backs_off_for_an_hour() {
        // Attempted 30 min ago, never succeeded: wait.
        assert!(!should_refresh_pricing(Some(t0() - Duration::minutes(30)), None, t0()));
        // Attempted 61 min ago, never succeeded: retry.
        assert!(should_refresh_pricing(Some(t0() - Duration::minutes(61)), None, t0()));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test pricing_throttle`
Expected: compile error — `should_refresh_pricing` not found.

- [ ] **Step 3: Write the implementation**

In `src-tauri/src/main.rs`:

Add to `AppState` (below `last_pricing_update`):

```rust
    /// Timestamp of the last pricing refresh ATTEMPT (successful or not);
    /// backs off failed attempts so an offline machine doesn't re-download
    /// on every 60s cycle.
    pub last_pricing_attempt: Mutex<Option<chrono::DateTime<Utc>>>,
```

Initialize `last_pricing_attempt: Mutex::new(None),` in the `.manage(AppState { ... })` call.

Add above `refresh_all`:

```rust
/// Pricing changes rarely; refresh at most daily, retrying hourly after a
/// failed attempt.
fn should_refresh_pricing(
    last_attempt: Option<chrono::DateTime<Utc>>,
    last_success: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
) -> bool {
    let attempt_due = last_attempt.map_or(true, |t| now - t >= chrono::Duration::hours(1));
    let success_due = last_success.map_or(true, |t| now - t >= chrono::Duration::hours(24));
    attempt_due && success_due
}
```

In `refresh_all`, wrap the pricing block:

```rust
    let prefs = state.preferences.lock().unwrap().clone();
    let pricing_due = should_refresh_pricing(
        *state.last_pricing_attempt.lock().unwrap(),
        *state.last_pricing_update.lock().unwrap(),
        now,
    );
    if prefs.network_pricing_refresh_enabled && pricing_due {
        *state.last_pricing_attempt.lock().unwrap() = Some(now);
        let current = state.pricing.lock().unwrap().clone();
        let source = price_fetch::HttpPriceSource {
            url: "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json".to_string(),
        };
        let (refreshed, succeeded) = price_fetch::refresh_pricing_table_with_status(&source, current);
        *state.pricing.lock().unwrap() = refreshed;
        if succeeded {
            *state.last_pricing_update.lock().unwrap() = Some(now);
        }
    }
```

- [ ] **Step 4: Run tests and build**

Run: `cd src-tauri && cargo test && cargo check`
Expected: all PASS (4 new throttle tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/main.rs
git commit -m "feat: throttle LiteLLM pricing download to daily with hourly failure backoff"
```

---

## Out of Scope (deliberate, for future plans)

- **OAuth token refresh** via `https://platform.claude.com/v1/oauth/token` (public client ID `9d1c250a-e61b-44d9-88ed-5944d1962f5e`). Claude Code refreshes its own token whenever it runs; an expired token here just means the fallback estimate shows until the user next uses Claude Code.
- Detecting the installed Claude CLI version for the User-Agent (constant `claude-code/2.1.0` for now).
- Per-model weekly windows (`seven_day_opus`, the newer `limits[]` array shape) and `extra_usage` credits.
- More providers (Codex, Gemini, Copilot) — the `Provider` trait is the seam.
- opencode real limits (codexbar gets them from opencode.ai with browser cookies — heavier machinery).
- Adaptive refresh cadence (menu-open detection, battery/thermal signals).

## Verification at the end

1. `cd src-tauri && cargo test` — full suite green.
2. `cargo tauri dev`: menu shows `Límite 5h`/`Límite 7d` with values matching claude.ai/settings/usage; tray label shows the same percentages.
3. Rename `~/.claude/.credentials.json` → refresh → menu falls back to `(estimado)` lines, no crash; restore the file.
4. Watch stderr across two refresh cycles: the LiteLLM download log appears at most once.
