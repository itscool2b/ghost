//! Bounded public release metadata. No vendor credentials, prompts or installer commands are used.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use ghost_protocol::agents::{AgentInstallation, InstallChannel, LatestRelease};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

const REFRESH_MS: i64 = 6 * 60 * 60 * 1000;
const FAILURE_RETRY_MS: i64 = 15 * 60 * 1000;
/// GitHub's answer for one release lists every asset: Codex's passed 256 KiB in November 2026,
/// and from then on Ghost never learned its latest version. Still bounded.
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_CACHE: usize = 128 * 1024;

enum FetchReply {
    NotModified,
    Document {
        etag: Option<String>,
        bytes: Vec<u8>,
    },
}
trait Fetcher: Send + Sync {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        etag: Option<&'a str>,
    ) -> BoxFuture<'a, Result<FetchReply, ()>>;
}
struct HttpFetcher(reqwest::Client);
impl Fetcher for HttpFetcher {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        etag: Option<&'a str>,
    ) -> BoxFuture<'a, Result<FetchReply, ()>> {
        Box::pin(async move {
            let mut request = self.0.get(url);
            if let Some(etag) = etag {
                request = request.header(reqwest::header::IF_NONE_MATCH, etag);
            }
            let mut response = request.send().await.map_err(|_| ())?;
            if response.status() == reqwest::StatusCode::NOT_MODIFIED {
                return Ok(FetchReply::NotModified);
            }
            if !response.status().is_success()
                || response
                    .content_length()
                    .is_some_and(|size| size > MAX_RESPONSE as u64)
            {
                return Err(());
            }
            let etag = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|value| value.to_str().ok())
                .filter(|tag| tag.len() <= 1024)
                .map(str::to_owned);
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
                if bytes.len() + chunk.len() > MAX_RESPONSE {
                    return Err(());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(FetchReply::Document { etag, bytes })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseSource {
    url: String,
    label: String,
    selector: Selector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selector {
    NpmTag,
    Github,
    Homebrew,
    AcpRegistry(&'static str),
}

impl ReleaseSource {
    /// URLs and JSON selectors come from reviewed providers, never native response payloads.
    pub fn for_installation(installation: &AgentInstallation) -> Option<Self> {
        let registry_id = match installation.agent_id.as_str() {
            "antigravity" => Some("antigravity-acp"),
            "cursor" => Some("cursor"),
            "opencode" => Some("opencode"),
            "opencode-v2" => Some("opencode-v2"),
            _ => None,
        };
        if !matches!(
            installation.channel,
            InstallChannel::Bundled { .. } | InstallChannel::Homebrew { .. }
        ) && let Some(id) = registry_id
        {
            return Some(Self {
                url: "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json".into(),
                label: "ACP Registry".into(),
                selector: Selector::AcpRegistry(id),
            });
        }
        let provider = crate::installation::provider_namespace(&installation.agent_id);
        match &installation.channel {
            InstallChannel::Homebrew { formula } if matches!(formula.as_str(), "claude-code" | "claude-code@latest" | "codex") => {
                let cask = installation.resolved_path.as_deref()?.replace('\\', "/").to_ascii_lowercase().contains("/caskroom/");
                Some(Self { url: format!("https://formulae.brew.sh/api/{}/{formula}.json", if cask { "cask" } else { "formula" }), label: "Homebrew".into(), selector: Selector::Homebrew })
            }
            InstallChannel::Bundled { .. } => None,
            _ => match provider {
                "claude" => Some(Self { url: "https://registry.npmjs.org/-/package/@anthropic-ai%2fclaude-code/dist-tags".into(), label: "npm latest".into(), selector: Selector::NpmTag }),
                "codex" => Some(Self { url: "https://api.github.com/repos/openai/codex/releases/latest".into(), label: "GitHub stable release".into(), selector: Selector::Github }),
                // The release a source names must be one this installation's updater installs.
                // Grok's own updater and npm's `latest` both follow the stable channel; the ACP
                // Registry lists the alpha, which neither installs, so Update could never clear
                // "available" (owner, 10 November 2026).
                "grok" => Some(Self { url: "https://registry.npmjs.org/-/package/@xai-official%2fgrok/dist-tags".into(), label: "npm latest".into(), selector: Selector::NpmTag }),
                "pi" => Some(Self { url: "https://registry.npmjs.org/-/package/@earendil-works%2fpi-coding-agent/dist-tags".into(), label: "npm latest".into(), selector: Selector::NpmTag }),
                "omp" => Some(Self { url: "https://registry.npmjs.org/-/package/@oh-my-pi%2fpi-coding-agent/dist-tags".into(), label: "npm latest".into(), selector: Selector::NpmTag }),
                _ => None,
            }
        }
    }

    fn parse(&self, bytes: &[u8]) -> Option<String> {
        if bytes.len() > MAX_RESPONSE {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let raw = match self.selector {
            Selector::NpmTag => value["latest"].as_str()?,
            Selector::Github => {
                if value["prerelease"] != false || value["draft"] != false {
                    return None;
                }
                value["tag_name"].as_str()?.strip_prefix("rust-v")?
            }
            Selector::Homebrew => value["versions"]["stable"]
                .as_str()
                .or_else(|| value["version"].as_str())?,
            Selector::AcpRegistry(id) => {
                if value["version"] != "1.0.0" {
                    return None;
                }
                let mut entries = value["agents"]
                    .as_array()?
                    .iter()
                    .filter(|agent| agent["id"] == id);
                let entry = entries.next()?;
                if entries.next().is_some() {
                    return None;
                }
                entry["version"].as_str()?
            }
        };
        if raw.is_empty()
            || raw.len() > 128
            || !raw
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-+,".contains(&byte))
        {
            return None;
        }
        crate::catalog::parse_version(raw).map(|_| raw.into())
    }

    fn cache_key(&self) -> String {
        match self.selector {
            Selector::AcpRegistry(id) => format!("{}#{id}", self.url),
            _ => self.url.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Cached {
    version: Option<String>,
    etag: Option<String>,
    checked_at: i64,
    attempted_at: i64,
}

impl Cached {
    fn due(&self, now: i64) -> bool {
        let age = now.saturating_sub(self.attempted_at);
        self.attempted_at == 0
            || age < 0
            || age
                >= if self.checked_at == self.attempted_at {
                    REFRESH_MS
                } else {
                    FAILURE_RETRY_MS
                }
    }

    fn snapshot(&self, source: &ReleaseSource, now: i64) -> LatestRelease {
        LatestRelease {
            version: self.version.clone(),
            source: source.label.clone(),
            checked_at: (self.checked_at > 0).then_some(self.checked_at),
            stale: self.checked_at == 0
                || now < self.checked_at
                || now.saturating_sub(self.checked_at) >= REFRESH_MS,
        }
    }
}

/// Cache one entry per reviewed source and selector. Requests share an entry lock; independent providers
/// do not block each other. Failed refreshes retain their last known version.
pub struct ReleaseCache {
    fetcher: Arc<dyn Fetcher>,
    path: PathBuf,
    entries: Mutex<BTreeMap<String, Arc<Mutex<Cached>>>>,
    save: Mutex<()>,
}

impl std::fmt::Debug for ReleaseCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReleaseCache")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ReleaseCache {
    pub fn new(path: PathBuf) -> Result<Self, reqwest::Error> {
        Ok(Self {
            fetcher: Arc::new(HttpFetcher(
                reqwest::Client::builder()
                    .https_only(true)
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(10))
                    .connect_timeout(Duration::from_secs(5))
                    .user_agent("auracode-release-check")
                    .build()?,
            )),
            path,
            entries: Mutex::new(BTreeMap::new()),
            save: Mutex::new(()),
        })
    }

    /// Only Ghost's own bounded cache file is read. Request targets always come from a newly
    /// constructed reviewed source; loading this file cannot introduce a network destination.
    pub async fn load(&self) {
        use tokio::io::AsyncReadExt;
        let Ok(meta) = tokio::fs::metadata(&self.path).await else {
            return;
        };
        if !meta.is_file() || meta.len() > MAX_CACHE as u64 {
            return;
        }
        let Ok(file) = tokio::fs::File::open(&self.path).await else {
            return;
        };
        let mut bytes = Vec::new();
        if file
            .take(MAX_CACHE as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .is_err()
            || bytes.len() > MAX_CACHE
        {
            return;
        }
        let Ok(entries) = serde_json::from_slice::<BTreeMap<String, Cached>>(&bytes) else {
            return;
        };
        if entries.len() > 64 {
            return;
        }
        let mut state = self.entries.lock().await;
        for (url, mut cached) in entries {
            if url.len() > 256
                || cached
                    .version
                    .as_ref()
                    .is_some_and(|version| version.len() > 128)
            {
                continue;
            }
            cached.etag = cached
                .etag
                .filter(|tag| tag.len() <= 1024 && !tag.chars().any(char::is_control));
            state
                .entry(url)
                .or_insert_with(|| Arc::new(Mutex::new(cached)));
        }
    }

    pub async fn refresh(&self, source: &ReleaseSource, now: i64) -> LatestRelease {
        let entry = self
            .entries
            .lock()
            .await
            .entry(source.cache_key())
            .or_default()
            .clone();
        let mut cached = entry.lock().await;
        let attempted = cached.due(now);
        if attempted {
            cached.attempted_at = now;
            match self
                .fetcher
                .fetch(&source.url, cached.etag.as_deref())
                .await
            {
                Ok(FetchReply::NotModified) if cached.version.is_some() => {
                    cached.checked_at = now;
                }
                Ok(FetchReply::Document { etag, bytes }) => {
                    if let Some(version) = source.parse(&bytes) {
                        cached.version = Some(version);
                        cached.etag = etag;
                        cached.checked_at = now;
                    }
                }
                _ => {}
            }
        }
        let snapshot = cached.snapshot(source, now);
        drop(cached);
        if attempted {
            self.save().await;
        }
        snapshot
    }

    async fn save(&self) {
        let _save = self.save.lock().await;
        let entries = self.entries.lock().await.clone();
        let mut values = BTreeMap::new();
        for (url, value) in entries {
            values.insert(url, value.lock().await.clone());
        }
        let Ok(bytes) = serde_json::to_vec(&values) else {
            return;
        };
        if bytes.len() > MAX_CACHE {
            return;
        }
        let Some(parent) = self.path.parent() else {
            return;
        };
        if tokio::fs::create_dir_all(parent).await.is_err() {
            return;
        }
        let partial = self
            .path
            .with_extension(format!("{}.partial", uuid::Uuid::now_v7()));
        if tokio::fs::write(&partial, bytes).await.is_err() {
            return;
        }
        if tokio::fs::rename(&partial, &self.path).await.is_err() {
            let _ = tokio::fs::remove_file(partial).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeFetcher {
        calls: AtomicUsize,
    }
    impl Fetcher for FakeFetcher {
        fn fetch<'a>(
            &'a self,
            _url: &'a str,
            etag: Option<&'a str>,
        ) -> BoxFuture<'a, Result<FetchReply, ()>> {
            Box::pin(async move {
                match self.calls.fetch_add(1, Ordering::SeqCst) {
                    0 => {
                        assert_eq!(etag, None);
                        Ok(FetchReply::Document {
                            etag: Some("\"one\"".into()),
                            bytes: br#"{"latest":"1.2.3"}"#.to_vec(),
                        })
                    }
                    1 => {
                        assert_eq!(etag, Some("\"one\""));
                        Ok(FetchReply::NotModified)
                    }
                    _ => Err(()),
                }
            })
        }
    }

    #[tokio::test]
    async fn etags_suppress_duplicate_requests_and_offline_refresh_retains_persisted_version() {
        let root = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeFetcher {
            calls: AtomicUsize::new(0),
        });
        let cache = ReleaseCache {
            fetcher: fake.clone(),
            path: root.path().join("releases.json"),
            entries: Mutex::new(BTreeMap::new()),
            save: Mutex::new(()),
        };
        let source = source(Selector::NpmTag);
        assert_eq!(
            cache.refresh(&source, 1).await.version.as_deref(),
            Some("1.2.3")
        );
        cache.refresh(&source, 2).await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        let revalidated = cache.refresh(&source, REFRESH_MS + 1).await;
        assert_eq!(revalidated.checked_at, Some(REFRESH_MS + 1));
        assert!(!revalidated.stale);
        let offline = cache.refresh(&source, REFRESH_MS * 2 + 1).await;
        assert_eq!(offline.version.as_deref(), Some("1.2.3"));
        assert!(offline.stale);
        let loaded = ReleaseCache {
            fetcher: fake.clone(),
            path: cache.path.clone(),
            entries: Mutex::new(BTreeMap::new()),
            save: Mutex::new(()),
        };
        loaded.load().await;
        assert_eq!(loaded.refresh(&source, REFRESH_MS * 2 + 2).await, offline);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 3);
    }
    fn source(selector: Selector) -> ReleaseSource {
        ReleaseSource {
            url: "https://example.invalid".into(),
            label: "fixture".into(),
            selector,
        }
    }

    struct RegistryFetcher;
    impl Fetcher for RegistryFetcher {
        fn fetch<'a>(
            &'a self,
            _url: &'a str,
            _etag: Option<&'a str>,
        ) -> BoxFuture<'a, Result<FetchReply, ()>> {
            Box::pin(async {
                Ok(FetchReply::Document { etag: Some("same-document".into()), bytes: br#"{"version":"1.0.0","agents":[{"id":"opencode","version":"1.19.4"},{"id":"opencode-v2","version":"2.1.4"}]}"#.to_vec() })
            })
        }
    }

    #[tokio::test]
    async fn registry_selectors_keep_separate_versions_in_one_shared_document() {
        let root = tempfile::tempdir().unwrap();
        let cache = ReleaseCache {
            fetcher: Arc::new(RegistryFetcher),
            path: root.path().join("releases.json"),
            entries: Mutex::new(BTreeMap::new()),
            save: Mutex::new(()),
        };
        let one = source(Selector::AcpRegistry("opencode"));
        let two = source(Selector::AcpRegistry("opencode-v2"));
        assert_eq!(
            cache.refresh(&one, 1).await.version.as_deref(),
            Some("1.19.4")
        );
        assert_eq!(
            cache.refresh(&two, 2).await.version.as_deref(),
            Some("2.1.4")
        );
        assert_eq!(
            cache.refresh(&one, 3).await.version.as_deref(),
            Some("1.19.4")
        );
        assert!(one.parse(br#"{"version":"1.0.0","agents":[{"id":"opencode","version":"1.0.0"},{"id":"opencode","version":"9.0.0"}]}"#).is_none());
        assert!(
            one.parse(br#"{"version":"2.0.0","agents":[{"id":"opencode","version":"1.0.0"}]}"#)
                .is_none()
        );
    }

    #[test]
    fn source_shapes_and_prereleases_are_not_confused() {
        assert_eq!(
            source(Selector::NpmTag).parse(br#"{"latest":"1.2.3","next":"2.0.0-beta"}"#),
            Some("1.2.3".into())
        );
        assert_eq!(
            source(Selector::Github)
                .parse(br#"{"tag_name":"rust-v0.164.0","prerelease":false,"draft":false}"#),
            Some("0.164.0".into())
        );
        assert!(
            source(Selector::Github)
                .parse(br#"{"tag_name":"rust-v0.164.0","prerelease":true,"draft":false}"#)
                .is_none()
        );
        assert!(
            source(Selector::Github)
                .parse(br#"{"tag_name":"other-v1.0.0","prerelease":false,"draft":false}"#)
                .is_none()
        );
        assert_eq!(
            source(Selector::Homebrew).parse(br#"{"version":"2.1.310"}"#),
            Some("2.1.310".into())
        );
        assert!(
            source(Selector::NpmTag)
                .parse(&vec![b' '; MAX_RESPONSE + 1])
                .is_none()
        );
    }

    #[test]
    fn a_release_source_names_a_version_the_installations_updater_installs() {
        use ghost_protocol::agents::{AgentHost, VersionVerification};
        let installation = |agent: &str, channel: InstallChannel| AgentInstallation {
            agent_id: agent.into(),
            provider: crate::installation::provider_namespace(agent).into(),
            host: AgentHost::Native,
            launch_path: "/opt/bin/agent".into(),
            resolved_path: Some("/opt/bin/agent".into()),
            version: "1.0.77".into(),
            channel,
            verification: VersionVerification::Unknown,
        };
        // Grok's own updater and npm both install the stable release. The ACP Registry lists the
        // alpha, which neither does: compared with it, Update never cleared "available".
        for agent in ["grok", "grok-stream"] {
            let source = ReleaseSource::for_installation(&installation(
                agent,
                InstallChannel::VendorStandalone,
            ))
            .expect("a source");
            assert_eq!(source.selector, Selector::NpmTag, "{agent}");
            assert!(
                source.url.contains("@xai-official%2fgrok/dist-tags"),
                "{agent}"
            );
            assert_eq!(
                source.parse(br#"{"latest":"1.0.77","alpha":"1.0.80"}"#),
                Some("1.0.77".into()),
                "{agent}"
            );
        }
        // GitHub's answer for one Codex release is larger than the old 256 KiB bound.
        let mut answer =
            br#"{"tag_name":"rust-v0.167.0","prerelease":false,"draft":false,"body":""#.to_vec();
        answer.extend(std::iter::repeat_n(b'a', 400 * 1024));
        answer.extend_from_slice(br#""}"#);
        assert_eq!(
            source(Selector::Github).parse(&answer),
            Some("0.167.0".into())
        );
    }

    #[test]
    fn successful_checks_wait_six_hours_failures_retry_without_losing_last_version() {
        let mut cached = Cached {
            version: Some("1.2.3".into()),
            checked_at: 10,
            attempted_at: 10,
            etag: Some("v1".into()),
        };
        assert!(!cached.due(REFRESH_MS));
        assert!(cached.due(REFRESH_MS + 10));
        cached.attempted_at = REFRESH_MS + 10;
        assert!(!cached.due(REFRESH_MS + 20));
        assert!(cached.due(REFRESH_MS + 10 + FAILURE_RETRY_MS));
        let stale = cached.snapshot(&source(Selector::NpmTag), REFRESH_MS + 20);
        assert!(stale.stale);
        assert_eq!(stale.version.as_deref(), Some("1.2.3"));
        assert!(cached.due(1), "clock rollback");
    }
}
