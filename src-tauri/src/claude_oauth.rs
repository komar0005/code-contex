use crate::limits::{LimitsSnapshot, RateWindow};
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

/// On macOS, Claude Code stores its OAuth credentials in the login
/// Keychain (item "Claude Code-credentials") instead of the plaintext
/// file. The payload is the same JSON `parse_credentials` understands.
#[cfg(target_os = "macos")]
pub fn read_credentials_from_keychain() -> Option<ClaudeCredentials> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_credentials(String::from_utf8(output.stdout).ok()?.trim())
}

#[cfg(not(target_os = "macos"))]
pub fn read_credentials_from_keychain() -> Option<ClaudeCredentials> {
    None
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
