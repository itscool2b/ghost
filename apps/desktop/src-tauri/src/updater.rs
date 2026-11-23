//! Signed app updates. React only renders status and requests an operation.
//!
//! Who may do what, and how it is checked:
//! - **What is installed** is decided by one thing: the updater signature, made in the Publish
//!   workflow with a private key that never leaves GitHub's secret store, checked here against the
//!   public key compiled into this build, over the exact bytes downloaded and bound to the version
//!   the manifest advertised. A host that served other bytes, an older version or another
//!   product's package fails that check, whatever the manifest says.
//! - **Where it is fetched from** is fixed: the manifest is one GitHub release asset of this
//!   repository, and a package must be an asset of one of its `v` releases. GitHub answers both
//!   with a redirect to its asset store; redirects are followed only to GitHub's own hosts, over
//!   HTTPS, a few at most. No endpoint, host or key comes from a setting, the page or the manifest.
//! - **When it is installed** is the person's choice: checking is automatic (Settings can turn it
//!   off) and reads one small public file; downloading and installing happen only on a button.
//! - No credential is read or sent, the webview has no updater permission, and a development
//!   build never installs anything (it can exercise the flow against a loopback test server).
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::{
    sessions::{self, AppQuitHost, ExitRequest, QuitHost, QuitOutcome, QuitQuestion},
    state::AppState,
    sync::lock,
};

const MAX_MANIFEST: usize = 64 * 1024;
const MAX_PACKAGE: usize = 512 * 1024 * 1024;
/// The first automatic check waits for the app to settle; later ones are a few a day.
const FIRST_CHECK: Duration = Duration::from_secs(45);
const CHECK_EVERY: Duration = Duration::from_secs(4 * 60 * 60);
/// GitHub answers a release asset with at most two redirects; a few more are allowed, not many.
const MAX_REDIRECTS: usize = 5;
/// The repository's release assets, and the only hosts GitHub redirects an asset request to.
///
/// The repository was `itscool2b/auracode` until 19 November 2026, and copies from before accept
/// packages under that path alone. So the release feed still names it (GitHub redirects it to
/// `itscool2b/o7`, a redirect within github.com), and this build accepts both. Only the owner can
/// make a repository under `itscool2b`, and every package's signature is checked anyway.
const RELEASES: [&str; 2] = [
    "/itscool2b/o7/releases/download/",
    "/itscool2b/auracode/releases/download/",
];
const ASSET_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

pub(crate) fn staging() -> bool {
    matches!(option_env!("O7_UPDATER_CHANNEL"), Some("staging"))
}
fn channel() -> &'static str {
    if staging() { "staging" } else { "stable" }
}
/// The newest stable release's manifest, or the one the rolling `staging` release carries.
fn manifest(staging: bool) -> &'static str {
    if staging {
        "https://github.com/itscool2b/o7/releases/download/staging/latest.json"
    } else {
        "https://github.com/itscool2b/o7/releases/latest/download/latest.json"
    }
}

/// Where this launch reads updates from and the key it trusts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Source {
    manifest: String,
    key: String,
    /// A development build reading a loopback test server: everything but installation runs.
    test: bool,
}

/// A released build reads GitHub with its compiled-in key. A development build has no source,
/// unless both test variables name a loopback manifest and a key: then it can check, download and
/// verify against a server on this machine, and never installs.
fn source() -> Option<Source> {
    if o7_core::paths::DEVELOPMENT_BUILD {
        return test_source(
            std::env::var("O7_UPDATE_TEST_MANIFEST").ok(),
            std::env::var("O7_UPDATE_TEST_PUBLIC_KEY").ok(),
        );
    }
    public_key().map(|key| Source {
        manifest: manifest(staging()).into(),
        key: key.into(),
        test: false,
    })
}

fn test_source(manifest: Option<String>, key: Option<String>) -> Option<Source> {
    let manifest = manifest?;
    let key = key
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())?;
    let url: reqwest::Url = manifest.parse().ok()?;
    loopback(&url).then(|| Source {
        manifest: url.into(),
        key,
        test: true,
    })
}

/// A plain `http://127.0.0.1:<port>/…` address: this machine and nothing else.
fn loopback(url: &reqwest::Url) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    Disabled,
    Idle,
    Checking,
    Available,
    Downloading,
    Ready,
    WaitingForIdle,
    Installing,
    RestartRequired,
    Failed,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct UpdateStatus {
    pub channel: String,
    pub phase: UpdatePhase,
    /// The version running now.
    pub current_version: String,
    /// Whether o7 looks for a newer version by itself (Settings, General).
    pub automatic: bool,
    /// The newer version, once a check has found one.
    pub version: Option<String>,
    pub notes: Option<String>,
    pub downloaded_bytes: u32,
    pub total_bytes: Option<u32>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type, thiserror::Error)]
#[error("{message}")]
pub struct UpdateError {
    pub message: String,
}

fn error(message: impl Into<String>) -> UpdateError {
    UpdateError {
        message: message.into(),
    }
}
fn public_key() -> Option<&'static str> {
    option_env!("O7_UPDATER_PUBLIC_KEY")
        .map(str::trim)
        .filter(|key| !key.is_empty())
}

struct Pending {
    update: Update,
    bytes: Option<Vec<u8>>,
}
struct Inner {
    status: UpdateStatus,
    pending: Option<Pending>,
}
pub struct Updates {
    busy: AtomicBool,
    /// Settings' "Check for updates automatically". On until a profile's preferences say otherwise.
    automatic: AtomicBool,
    source: Option<Source>,
    inner: Mutex<Inner>,
}

impl Default for Updates {
    fn default() -> Self {
        Self::with_source(
            source().filter(|_| cfg!(any(windows, target_os = "macos", target_os = "linux"))),
        )
    }
}

impl Updates {
    fn with_source(source: Option<Source>) -> Self {
        let enabled = source.is_some();
        let message = if enabled {
            None
        } else if o7_core::paths::DEVELOPMENT_BUILD {
            Some("A development build does not update itself.".into())
        } else {
            Some(
                "This build has no update signing key. Install a newer version from the download page."
                    .into(),
            )
        };
        Self {
            busy: AtomicBool::new(false),
            automatic: AtomicBool::new(true),
            source,
            inner: Mutex::new(Inner {
                pending: None,
                status: UpdateStatus {
                    channel: channel().into(),
                    phase: if enabled {
                        UpdatePhase::Idle
                    } else {
                        UpdatePhase::Disabled
                    },
                    current_version: String::new(),
                    automatic: true,
                    version: None,
                    notes: None,
                    downloaded_bytes: 0,
                    total_bytes: None,
                    message,
                },
            }),
        }
    }

    /// Settings' switch, applied as soon as it changes.
    pub fn set_automatic(&self, automatic: bool) {
        self.automatic.store(automatic, Ordering::Release);
    }
}

struct Operation<'a>(&'a Updates);
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        let mut inner = lock(&self.0.inner);
        if matches!(
            inner.status.phase,
            UpdatePhase::Checking | UpdatePhase::Downloading
        ) {
            inner.status.phase = UpdatePhase::Failed;
            inner.status.message =
                Some("The update operation was interrupted. Retry when ready.".into());
        }
        self.0.busy.store(false, Ordering::Release);
    }
}
impl Updates {
    fn snapshot(&self) -> UpdateStatus {
        let mut status = lock(&self.inner).status.clone();
        status.automatic = self.automatic.load(Ordering::Acquire);
        status
    }
    /// The status as the page sees it: with the version this app is.
    fn report(&self, app: &tauri::AppHandle) -> UpdateStatus {
        let mut status = self.snapshot();
        status.current_version = app.package_info().version.to_string();
        status
    }
    fn begin(&self) -> Result<Operation<'_>, UpdateError> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(error("Another update operation is still running."));
        }
        Ok(Operation(self))
    }
    fn phase(&self, phase: UpdatePhase, message: Option<String>) {
        let mut inner = lock(&self.inner);
        inner.status.phase = phase;
        inner.status.message = message;
    }
}

fn available(state: &AppState, updates: &Updates) -> Result<Source, UpdateError> {
    if state.quit.is_quitting() {
        return Err(error("o7 is already closing."));
    }
    if !cfg!(any(windows, target_os = "macos", target_os = "linux")) || state.launch.is_smoke() {
        return Err(error(
            "Application updates are unavailable for this launch.",
        ));
    }
    // A development build has a source only for the loopback test server, and never installs:
    // an update would replace the installed app, which may be running the session that built it.
    updates.source.clone().ok_or_else(|| {
        error(if o7_core::paths::DEVELOPMENT_BUILD {
            "Application updates are unavailable for a development build."
        } else {
            "This build has no configured update signing key."
        })
    })
}

#[tauri::command]
#[specta::specta]
pub fn app_update_status(app: tauri::AppHandle, updates: State<'_, Updates>) -> UpdateStatus {
    updates.report(&app)
}

#[tauri::command]
#[specta::specta]
pub async fn app_update_check(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    updates: State<'_, Updates>,
) -> Result<UpdateStatus, UpdateError> {
    check(&app, &state, &updates, false).await
}

/// Reads the manifest and records whether it offers a newer version. A check the person asked for
/// shows its failure; one o7 made by itself leaves no trace when it fails (being offline is
/// not news), and never disturbs a download or an installation.
async fn check(
    app: &tauri::AppHandle,
    state: &AppState,
    updates: &Updates,
    quiet: bool,
) -> Result<UpdateStatus, UpdateError> {
    let source = available(state, updates)?;
    let before = updates.snapshot();
    if quiet && !may_check_quietly(before.phase) {
        return Ok(updates.report(app));
    }
    let _operation = updates.begin()?;
    updates.phase(UpdatePhase::Checking, None);
    let result = async {
        let response = client(Duration::from_secs(30), source.test)?
            .get(&source.manifest)
            .send()
            .await
            .map_err(|_| error("Could not reach the update service. Retry when online."))?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let bytes = bounded_response(response, MAX_MANIFEST, |_| {}).await?;
        let manifest = serde_json::from_slice(&bytes)
            .map_err(|_| error("The update manifest is not valid JSON."))?;
        // The verifier refuses to exist without an endpoint, though this code fetches the
        // manifest itself and never asks it to: it gets the one address this build reads. (Until
        // 19 November 2026 it got none, so no check could ever have passed; no build had updates
        // on, so nobody met it.)
        let endpoint = source
            .manifest
            .parse()
            .map_err(|_| error("The update verifier could not be configured."))?;
        let updater = app
            .updater_builder()
            .pubkey(&source.key)
            .endpoints(vec![endpoint])
            .and_then(|builder| builder.build())
            .map_err(|_| error("The update verifier could not be configured."))?;
        // This API applies target/version selection and mandates a signed version, even when the
        // manifest was fetched separately. Separate fetch permits a hard metadata size bound.
        let update = updater
            .restore_update(manifest)
            .map_err(|_| error("No valid update for this platform was found in the manifest."))?;
        if let Some(update) = &update {
            validate_download_url(&update.download_url, &source)?;
        }
        Ok::<_, UpdateError>(update)
    }
    .await;
    match result {
        Ok(update) => {
            let mut inner = lock(&updates.inner);
            inner.status = UpdateStatus {
                channel: channel().into(),
                phase: if update.is_some() {
                    UpdatePhase::Available
                } else {
                    UpdatePhase::Idle
                },
                current_version: String::new(),
                automatic: true,
                version: update.as_ref().map(|u| u.version.clone()),
                notes: update.as_ref().and_then(|u| u.body.clone()),
                downloaded_bytes: 0,
                total_bytes: None,
                message: None,
            };
            inner.pending = update.map(|update| Pending {
                update,
                bytes: None,
            });
        }
        Err(err) if quiet => {
            // Back to what it was: an update found earlier stays on offer.
            tracing::debug!(error = %err.message, "automatic update check failed");
            updates.phase(before.phase, before.message);
        }
        Err(err) => {
            updates.phase(UpdatePhase::Failed, Some(err.message.clone()));
            return Err(err);
        }
    }
    Ok(updates.report(app))
}

/// An automatic check runs only from a resting state: never over a download, a verified package
/// waiting to install, or an installation.
fn may_check_quietly(phase: UpdatePhase) -> bool {
    matches!(
        phase,
        UpdatePhase::Idle | UpdatePhase::Available | UpdatePhase::Failed
    )
}

/// Looks for a newer version shortly after start and a few times a day, while Settings allows it.
/// It reads one small public file and sends nothing about this computer but the request itself.
/// Once a day it also adds this copy to the site's count (`usage_count`), with the same switch.
pub fn start_automatic_checks(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut wait = FIRST_CHECK;
        loop {
            tokio::time::sleep(wait).await;
            wait = CHECK_EVERY;
            let (Some(state), Some(updates)) =
                (app.try_state::<AppState>(), app.try_state::<Updates>())
            else {
                return;
            };
            if updates.source.is_none() || state.quit.is_quitting() {
                return;
            }
            if updates.automatic.load(Ordering::Acquire) {
                let _ = check(&app, &state, &updates, true).await;
                // A released stable build adds itself to the site's daily count of copies that
                // ran (usage_count.rs): never a test source, staging or a smoke launch.
                let released = updates.source.as_ref().is_some_and(|source| !source.test);
                if released
                    && !staging()
                    && !state.launch.is_smoke()
                    && let Some(dir) = state.data_dir()
                {
                    crate::usage_count::count_once_a_day(
                        dir.to_path_buf(),
                        app.package_info().version.to_string(),
                    )
                    .await;
                }
            }
        }
    });
}

#[tauri::command]
#[specta::specta]
pub async fn app_update_download(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    updates: State<'_, Updates>,
) -> Result<UpdateStatus, UpdateError> {
    let source = available(&state, &updates)?;
    let _operation = updates.begin()?;
    let update = lock(&updates.inner)
        .pending
        .as_ref()
        .map(|p| p.update.clone())
        .ok_or_else(|| error("Check for an update first."))?;
    validate_download_url(&update.download_url, &source)?;
    {
        let mut inner = lock(&updates.inner);
        inner.status.downloaded_bytes = 0;
        inner.status.total_bytes = None;
        if let Some(pending) = inner.pending.as_mut() {
            pending.bytes = None;
        }
    }
    updates.phase(UpdatePhase::Downloading, None);
    let result = async {
        let response = client(Duration::from_secs(900), source.test)?
            .get(update.download_url.clone())
            .send()
            .await
            .map_err(|_| error("The update download could not start. Retry when online."))?;
        lock(&updates.inner).status.total_bytes = response
            .content_length()
            .and_then(|n| u32::try_from(n).ok());
        let bytes = bounded_response(response, MAX_PACKAGE, |total| {
            lock(&updates.inner).status.downloaded_bytes = total as u32
        })
        .await?;
        update.verify(&bytes).map_err(|_| {
            error("The update signature or signed version is invalid. Nothing was installed.")
        })?;
        Ok::<_, UpdateError>(bytes)
    }
    .await;
    match result {
        Ok(bytes) => {
            if let Some(pending) = lock(&updates.inner).pending.as_mut() {
                pending.bytes = Some(bytes);
            }
            updates.phase(UpdatePhase::Ready, None);
        }
        Err(err) => {
            updates.phase(UpdatePhase::Failed, Some(err.message.clone()));
            return Err(err);
        }
    }
    Ok(updates.report(&app))
}

/// The one connection updates use. A released build speaks HTTPS only and follows a redirect only
/// to one of GitHub's asset hosts; the loopback test server gets plain HTTP and no redirects.
fn client(timeout: Duration, test: bool) -> Result<reqwest::Client, UpdateError> {
    let redirects = if test {
        reqwest::redirect::Policy::none()
    } else {
        reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if asset_host(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("redirected outside the release hosts")
            }
        })
    };
    reqwest::Client::builder()
        .https_only(!test)
        .redirect(redirects)
        .timeout(timeout)
        .user_agent(concat!("auracode/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| error("The update connection could not be configured."))
}

/// Whether a redirect may be followed: HTTPS to one of GitHub's asset hosts on the default port,
/// with no credentials in the address. Its query is GitHub's own short-lived signature.
fn asset_host(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| ASSET_HOSTS.contains(&host))
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn validate_download_url(url: &reqwest::Url, source: &Source) -> Result<(), UpdateError> {
    if source.test {
        return validate_test_url(url, source);
    }
    validate_channel_url(url, staging())
}

/// A package the manifest names must be a file of one of this repository's releases on its
/// channel: `/itscool2b/o7/releases/download/<tag>/<file>` (or the repository's earlier path,
/// [`RELEASES`]), the tag starting `v` (stable) or
/// `staging-v`, both parts plain names. Nothing else on GitHub, and nothing anywhere else.
fn validate_channel_url(url: &reqwest::Url, staging: bool) -> Result<(), UpdateError> {
    let outside = || error("The update artifact is outside the trusted release location.");
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(outside());
    }
    let rest = RELEASES
        .iter()
        .find_map(|prefix| url.path().strip_prefix(prefix))
        .ok_or_else(outside)?;
    let (tag, file) = rest.split_once('/').ok_or_else(outside)?;
    let plain = |part: &str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    let version = if staging {
        tag.strip_prefix("staging-v")
    } else {
        tag.strip_prefix('v')
    };
    if !plain(tag)
        || !plain(file)
        || !version.is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()))
    {
        return Err(outside());
    }
    Ok(())
}

/// The loopback test server's packages: the same origin as its manifest, a plain path.
fn validate_test_url(url: &reqwest::Url, source: &Source) -> Result<(), UpdateError> {
    let manifest: reqwest::Url = source
        .manifest
        .parse()
        .map_err(|_| error("The test manifest address is not valid."))?;
    if !loopback(url)
        || url.port() != manifest.port()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
    {
        return Err(error(
            "The update artifact is outside the trusted release location.",
        ));
    }
    Ok(())
}

fn check_size(current: usize, incoming: usize, limit: usize) -> Result<usize, UpdateError> {
    current
        .checked_add(incoming)
        .filter(|total| *total <= limit)
        .ok_or_else(|| error("The update response exceeds its size limit."))
}

async fn bounded_response(
    mut response: reqwest::Response,
    limit: usize,
    mut progress: impl FnMut(usize),
) -> Result<Vec<u8>, UpdateError> {
    if !response.status().is_success() {
        return Err(error(
            "The update service returned an unsuccessful response.",
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(error("The update response exceeds its size limit."));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| error("The update download was interrupted. Retry when ready."))?
    {
        let total = check_size(bytes.len(), chunk.len(), limit)?;
        bytes.extend_from_slice(&chunk);
        progress(total);
    }
    Ok(bytes)
}

/// Updates never accept the ordinary quit flow's option to discard unsaved work.
struct UpdateQuit<H>(H);
impl<H: QuitHost> QuitHost for UpdateQuit<H> {
    fn owned_work_guard(&self) -> Option<Box<dyn Send + '_>> {
        // install retains the transition, session-idle and terminal-idle barriers through this
        // entire flow and installation. Ordinary drain admission correctly refuses that state.
        Some(Box::new(()))
    }
    async fn wait_owned_work(&self, deadline: Duration) -> bool {
        self.0.wait_owned_work(deadline).await
    }
    fn owned_work_blocked(&self) {
        self.0.owned_work_blocked();
    }
    async fn flush_drafts(&self) -> bool {
        self.0.flush_drafts().await
    }
    fn hide_window(&self) {} // Keep progress and any failure visible.
    fn show_window(&self) {
        self.0.show_window();
    }
    fn storage(&self) -> o7_protocol::session::StorageStatus {
        self.0.storage()
    }
    async fn ask_to_quit_unsaved(&self, _question: QuitQuestion) -> bool {
        false
    }
    async fn stop_sessions(&self, deadline: Duration) -> o7_core::session::ShutdownSummary {
        self.0.stop_sessions(deadline).await
    }
    async fn close_store(&self, deadline: Duration) -> bool {
        self.0.close_store(deadline).await
    }
    fn close_registry(&self) {
        self.0.close_registry();
    }
    fn exit(&self, _code: i32) {} // Only the verified installer can exit after successful cleanup.
}

#[tauri::command]
#[specta::specta]
pub async fn app_update_install(app: tauri::AppHandle) -> Result<UpdateStatus, UpdateError> {
    // Installation and cleanup outlive a lost/reloaded IPC caller.
    match tauri::async_runtime::spawn(install(app.clone())).await {
        Ok(result) => result,
        Err(_) => {
            let state = app.state::<AppState>();
            if state.quit.is_quitting() {
                state.quit.abandon();
            }
            let updates = app.state::<Updates>();
            updates.phase(
                UpdatePhase::RestartRequired,
                Some("The update task failed. Restart o7 before retrying.".into()),
            );
            Ok(updates.report(&app))
        }
    }
}

async fn install(app: tauri::AppHandle) -> Result<UpdateStatus, UpdateError> {
    let state = app.state::<AppState>();
    let _transition = state.transition.try_lock().map_err(|_| {
        error("Wait for the profile transition to finish before installing an update.")
    })?;
    let updates = app.state::<Updates>();
    let source = available(&state, &updates)?;
    let _operation = updates.begin()?;
    if !lock(&updates.inner)
        .pending
        .as_ref()
        .is_some_and(|pending| pending.bytes.is_some())
    {
        return Err(error("Download and verify an update before installing it."));
    }
    if source.test {
        // A development build stops here: the package is verified once more and nothing is
        // installed, no chat is stopped and the app keeps running.
        return Ok(rehearse(&app, &updates).await);
    }
    let chats = state
        .chats()
        .ok_or_else(|| error("The current profile is unavailable. Restart o7 before updating."))?;
    if !matches!(
        chats.manager.storage_status().state,
        o7_protocol::session::StorageState::Ok
    ) {
        updates.phase(
            UpdatePhase::WaitingForIdle,
            Some("Resolve the storage error before installing an update.".into()),
        );
        return Ok(updates.report(&app));
    }
    // The person chose to install and restart (its confirmation says so). Terminals close first:
    // a chat open in its terminal gets its session back as that terminal ends, which Stop cannot
    // do. Then the chats' agents stop, each resuming its session after the restart, and
    // admission closes as before.
    crate::terminal::end_terminal_chats(&state, None);
    state.terminals.close_all();
    let report = chats
        .manager
        .stop_all(o7_core::session::StopScope::All, Duration::from_secs(30))
        .await;
    let _ = chats
        .manager
        .wait_project_terminals(Duration::from_secs(10))
        .await;
    let Some(_sessions) = chats.manager.try_quiesce_when_idle() else {
        let reason = if report.stopping.is_empty() {
            "Wait for Git changes and agent updates to finish, then retry installation.".to_owned()
        } else {
            let names = chats.manager.chat_names(report.stopping).await;
            o7_core::session::still_stopping(&names, "Retry installation in a moment.")
        };
        updates.phase(UpdatePhase::WaitingForIdle, Some(reason));
        return Ok(updates.report(&app));
    };
    let Some(_terminals) = state.terminals.try_quiesce_when_idle() else {
        updates.phase(
            UpdatePhase::WaitingForIdle,
            Some("Terminals are still closing. Retry installation in a moment.".into()),
        );
        return Ok(updates.report(&app));
    };
    if state.quit.on_exit_requested() != ExitRequest::StartQuit {
        return Err(error("o7 is already closing."));
    }
    updates.phase(UpdatePhase::Installing, None);
    let outcome = sessions::run_quit(
        &state.quit,
        &UpdateQuit(AppQuitHost {
            app: &app,
            chats: &chats,
        }),
        0,
    )
    .await;
    match outcome {
        QuitOutcome::Kept => {
            updates.phase(
                UpdatePhase::Ready,
                Some("Save your drafts and resolve storage errors before installing.".into()),
            );
            return Ok(updates.report(&app));
        }
        QuitOutcome::Exited {
            saved: true,
            summary: Some(summary),
        } if summary.forced == 0 => {}
        _ => {
            updates.phase(UpdatePhase::RestartRequired, Some("Cleanup could not be confirmed. The update was not installed. Restart o7 before retrying.".into()));
            return Ok(updates.report(&app));
        }
    }
    state.terminals.stop_all();
    if let Some(shell) = app.try_state::<std::sync::Arc<crate::native_shell::NativeShell>>() {
        shell.shutdown();
    }
    let pending = lock(&updates.inner)
        .pending
        .take()
        .ok_or_else(|| error("The verified update is no longer available."))?;
    let bytes = pending
        .bytes
        .ok_or_else(|| error("The verified update is no longer available."))?;
    // install verifies the package again. On Windows it exits after launching NSIS. macOS returns,
    // and so does Linux once the AppImage is replaced or pkexec has installed the deb or rpm.
    match tauri::async_runtime::spawn_blocking(move || pending.update.install(bytes)).await {
        Ok(Ok(())) => app.restart(),
        _ => {
            updates.phase(
                UpdatePhase::RestartRequired,
                Some(
                    "The installer failed after storage closed. Restart o7 and retry the update."
                        .into(),
                ),
            );
            Ok(updates.report(&app))
        }
    }
}

/// What a development build does instead of installing: checks the downloaded package against
/// its signature again, shows the installing state for a moment, and says nothing was installed.
async fn rehearse(app: &tauri::AppHandle, updates: &Updates) -> UpdateStatus {
    let verified = {
        let inner = lock(&updates.inner);
        inner.pending.as_ref().is_some_and(|pending| {
            pending
                .bytes
                .as_ref()
                .is_some_and(|bytes| pending.update.verify(bytes).is_ok())
        })
    };
    if !verified {
        updates.phase(
            UpdatePhase::Failed,
            Some(
                "The update signature or signed version is invalid. Nothing was installed.".into(),
            ),
        );
        return updates.report(app);
    }
    updates.phase(UpdatePhase::Installing, None);
    tokio::time::sleep(Duration::from_secs(2)).await;
    updates.phase(
        UpdatePhase::Ready,
        Some("Development build: the update was verified and not installed.".into()),
    );
    updates.report(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeQuit {
        drafts: bool,
        saved: bool,
        calls: Mutex<Vec<&'static str>>,
    }
    impl QuitHost for FakeQuit {
        fn owned_work_guard(&self) -> Option<Box<dyn Send + '_>> {
            // Match a real host while install already holds the stronger idle admission barrier.
            None
        }
        async fn flush_drafts(&self) -> bool {
            lock(&self.calls).push("flush");
            self.drafts
        }
        fn hide_window(&self) {
            lock(&self.calls).push("hide");
        }
        fn show_window(&self) {
            lock(&self.calls).push("show");
        }
        fn storage(&self) -> o7_protocol::session::StorageStatus {
            o7_protocol::session::StorageStatus {
                state: o7_protocol::session::StorageState::Ok,
                unsaved_bytes: 0,
            }
        }
        async fn ask_to_quit_unsaved(&self, _: QuitQuestion) -> bool {
            lock(&self.calls).push("discard");
            true
        }
        async fn stop_sessions(&self, _: Duration) -> o7_core::session::ShutdownSummary {
            lock(&self.calls).push("stop");
            o7_core::session::ShutdownSummary {
                sessions: 0,
                graceful: 0,
                forced: 0,
                took: Duration::ZERO,
            }
        }
        async fn close_store(&self, _: Duration) -> bool {
            lock(&self.calls).push("save");
            self.saved
        }
        fn close_registry(&self) {
            lock(&self.calls).push("registry");
        }
        fn exit(&self, _: i32) {
            lock(&self.calls).push("exit");
        }
    }

    #[tokio::test]
    async fn update_cleanup_never_offers_to_discard_an_unsaved_draft() {
        let quit = sessions::Quit::default();
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        let host = UpdateQuit(FakeQuit {
            drafts: false,
            saved: true,
            calls: Mutex::new(Vec::new()),
        });
        assert_eq!(sessions::run_quit(&quit, &host, 0).await, QuitOutcome::Kept);
        assert_eq!(*lock(&host.0.calls), ["flush", "show"]);
        assert!(!quit.is_quitting(), "the user can retry after saving");
    }

    #[tokio::test]
    async fn update_cleanup_waits_for_storage_and_never_exits_the_app_itself() {
        for saved in [true, false] {
            let quit = sessions::Quit::default();
            assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
            let host = UpdateQuit(FakeQuit {
                drafts: true,
                saved,
                calls: Mutex::new(Vec::new()),
            });
            let outcome = sessions::run_quit(&quit, &host, 0).await;
            assert!(
                matches!(outcome, QuitOutcome::Exited { saved: actual, .. } if actual == saved)
            );
            assert_eq!(*lock(&host.0.calls), ["flush", "stop", "save", "registry"]);
        }
    }

    const RELEASE: &str = "https://github.com/itscool2b/o7/releases/download";

    #[test]
    fn update_artifacts_are_files_of_this_repositorys_releases_and_nothing_else() {
        let ok = format!("{RELEASE}/v0.1.1570/o7_0.1.1570_x64-setup.exe");
        assert!(validate_channel_url(&ok.parse().unwrap(), false).is_ok());
        // The repository's earlier path, which the release feed names for copies from before.
        let earlier = "https://github.com/itscool2b/auracode/releases/download/v0.1.1630/o7_0.1.1630_x64-setup.exe";
        assert!(validate_channel_url(&earlier.parse().unwrap(), false).is_ok());
        for url in [
            // Another scheme, host, port or a credential.
            "http://github.com/itscool2b/o7/releases/download/v0.1.1/app.exe",
            "https://github.com.evil.example/itscool2b/o7/releases/download/v0.1.1/app.exe",
            "https://evil.example/itscool2b/o7/releases/download/v0.1.1/app.exe",
            "https://github.com:8443/itscool2b/o7/releases/download/v0.1.1/app.exe",
            "https://user@github.com/itscool2b/o7/releases/download/v0.1.1/app.exe",
            // Another repository, or another part of this one.
            "https://github.com/someone/auracode/releases/download/v0.1.1/app.exe",
            "https://github.com/itscool2b/other/releases/download/v0.1.1/app.exe",
            "https://github.com/itscool2b/o7/raw/main/app.exe",
            "https://github.com/itscool2b/o7/releases/latest/download/app.exe",
            // A query, a fragment, an encoded or a nested path, a hidden file, no file.
            "https://github.com/itscool2b/o7/releases/download/v0.1.1/app.exe?token=anything",
            "https://github.com/itscool2b/o7/releases/download/v0.1.1/app.exe#ignored",
            "https://github.com/itscool2b/o7/releases/download/v0.1.1/%2e%2e/app.exe",
            "https://github.com/itscool2b/o7/releases/download/v0.1.1/a/app.exe",
            "https://github.com/itscool2b/o7/releases/download/v0.1.1/.hidden",
            "https://github.com/itscool2b/o7/releases/download/v0.1.1/",
            "https://github.com/itscool2b/o7/releases/download/v0.1.1",
            // A tag that is not a version: a hand-made release cannot stand in for one.
            "https://github.com/itscool2b/o7/releases/download/build-2026-11-19/app.exe",
            "https://github.com/itscool2b/o7/releases/download/vendor/app.exe",
            "https://github.com/itscool2b/o7/releases/download/latest/app.exe",
        ] {
            assert!(
                validate_channel_url(&url.parse().unwrap(), false).is_err(),
                "{url}"
            );
        }
        // `..` is resolved by the parser before the check, so it cannot climb out unseen.
        let climbed: reqwest::Url = format!("{RELEASE}/v0.1.1/../../../../raw/main/app.exe")
            .parse()
            .unwrap();
        assert!(validate_channel_url(&climbed, false).is_err());
    }
    #[test]
    fn staging_and_stable_never_cross_release_tags() {
        let stable = format!("{RELEASE}/v0.2.0/app.exe").parse().unwrap();
        let staging = format!("{RELEASE}/staging-v0.2.0/app.exe").parse().unwrap();
        assert!(validate_channel_url(&stable, false).is_ok());
        assert!(validate_channel_url(&staging, true).is_ok());
        assert!(validate_channel_url(&stable, true).is_err());
        assert!(validate_channel_url(&staging, false).is_err());
        assert_ne!(manifest(true), manifest(false));
        for address in [manifest(true), manifest(false)] {
            let url: reqwest::Url = address.parse().unwrap();
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("github.com"));
            assert!(url.path().starts_with("/itscool2b/o7/releases/"));
        }
    }
    #[test]
    fn a_redirect_is_followed_only_to_githubs_asset_hosts_over_https() {
        for url in [
            "https://github.com/itscool2b/o7/releases/download/v0.1.1/latest.json",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/2?sp=r&sig=x",
            "https://objects.githubusercontent.com/github-production-release-asset/1/2?X-Amz-Signature=x",
        ] {
            assert!(asset_host(&url.parse().unwrap()), "{url}");
        }
        for url in [
            "http://release-assets.githubusercontent.com/a",
            "https://release-assets.githubusercontent.com.evil.example/a",
            "https://evil.githubusercontent.com/a",
            "https://raw.githubusercontent.com/itscool2b/o7/main/latest.json",
            "https://release-assets.githubusercontent.com:8443/a",
            "https://user:pass@release-assets.githubusercontent.com/a",
            "https://evil.example/a",
            "https://127.0.0.1/a",
        ] {
            assert!(!asset_host(&url.parse().unwrap()), "{url}");
        }
    }
    #[test]
    fn a_test_source_is_a_loopback_manifest_with_a_key_and_only_that() {
        let key = Some("a test public key".to_owned());
        let local = Some("http://127.0.0.1:8123/latest.json".to_owned());
        let source = test_source(local.clone(), key.clone()).unwrap();
        assert!(source.test);
        assert_eq!(source.manifest, "http://127.0.0.1:8123/latest.json");
        // No key, no manifest, or a manifest that is not this machine: no source at all.
        assert_eq!(test_source(local.clone(), None), None);
        assert_eq!(test_source(local, Some("  ".into())), None);
        assert_eq!(test_source(None, key.clone()), None);
        for manifest in [
            "https://127.0.0.1:8123/latest.json",
            "http://127.0.0.1/latest.json",
            "http://localhost:8123/latest.json",
            "http://192.168.1.20:8123/latest.json",
            "http://user@127.0.0.1:8123/latest.json",
            "http://evil.example:8123/latest.json",
            "not a url",
        ] {
            assert_eq!(
                test_source(Some(manifest.into()), key.clone()),
                None,
                "{manifest}"
            );
        }
        // Its packages come from the same port and nowhere else.
        assert!(
            validate_test_url(&"http://127.0.0.1:8123/app.exe".parse().unwrap(), &source).is_ok()
        );
        for url in [
            "http://127.0.0.1:9000/app.exe",
            "https://github.com/itscool2b/o7/releases/download/v9.9.9/app.exe",
            "http://127.0.0.1:8123/app.exe?x=1",
            "http://evil.example:8123/app.exe",
        ] {
            assert!(
                validate_test_url(&url.parse().unwrap(), &source).is_err(),
                "{url}"
            );
        }
        // And a released build's source never accepts a loopback package.
        assert!(
            validate_channel_url(&"http://127.0.0.1:8123/app.exe".parse().unwrap(), false).is_err()
        );
    }
    #[test]
    fn an_automatic_check_never_runs_over_a_download_or_an_installation() {
        for phase in [
            UpdatePhase::Idle,
            UpdatePhase::Available,
            UpdatePhase::Failed,
        ] {
            assert!(may_check_quietly(phase), "{phase:?}");
        }
        for phase in [
            UpdatePhase::Disabled,
            UpdatePhase::Checking,
            UpdatePhase::Downloading,
            UpdatePhase::Ready,
            UpdatePhase::WaitingForIdle,
            UpdatePhase::Installing,
            UpdatePhase::RestartRequired,
        ] {
            assert!(!may_check_quietly(phase), "{phase:?}");
        }
    }
    #[test]
    fn a_build_without_a_source_is_disabled_and_says_why() {
        let none = Updates::with_source(None);
        let status = none.snapshot();
        assert_eq!(status.phase, UpdatePhase::Disabled);
        assert!(status.message.is_some());
        let some = Updates::with_source(test_source(
            Some("http://127.0.0.1:8123/latest.json".into()),
            Some("a test public key".into()),
        ));
        assert_eq!(some.snapshot().phase, UpdatePhase::Idle);
        assert!(
            some.snapshot().automatic,
            "on until Settings says otherwise"
        );
        some.set_automatic(false);
        assert!(!some.snapshot().automatic);
    }
    #[test]
    fn chunk_bounds_apply_without_content_length_and_before_allocation() {
        assert_eq!(check_size(5, 5, 10).unwrap(), 10);
        assert!(check_size(5, 6, 10).is_err());
        assert!(check_size(usize::MAX, 1, usize::MAX).is_err());
    }
    #[test]
    fn abandoned_operations_unlock_retry_and_keep_failure_visible() {
        let updates = Updates::with_source(None);
        let operation = updates.begin().unwrap();
        updates.phase(UpdatePhase::Downloading, None);
        assert!(updates.begin().is_err());
        drop(operation);
        assert_eq!(updates.snapshot().phase, UpdatePhase::Failed);
        assert!(updates.begin().is_ok());
    }
}
