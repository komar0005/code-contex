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
        // The keychain fallback only applies to the real credentials path:
        // tests point at temp paths and must stay hermetic.
        let creds = claude_oauth::read_credentials(&self.credentials_path).or_else(|| {
            if self.credentials_path == claude_oauth::default_credentials_path() {
                claude_oauth::read_credentials_from_keychain()
            } else {
                None
            }
        })?;
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
