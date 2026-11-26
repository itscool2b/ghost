//! Chat sessions in the shell: the channels a chat view's frames and the chat list's updates travel
//! on, the startup wiring of the session manager, the page-load hook, the quit flow with its
//! question while saving is paused and its signals, and the answers of the folder picker and "Show
//! log".
//!
//! Everything here is glue over ghost-core's [`SessionManager`]. The parts with decisions in them are
//! written against small seams ([`QuitHost`], [`SignalHost`], [`Outside`], plain closures), so their
//! tests drive them with fakes instead of a running app.
//!
//! - **Channels.** [`ChannelSink`] carries one open chat view's frames, [`StatusChannelSink`] one
//!   webview's chat-list updates. A failed send answers [`SinkGone`], which removes the subscriber;
//!   detaching drops the last Rust clone, so Tauri ends the JavaScript channel.
//! - **Page loads.** A reloaded page has new channel callbacks, so frames for the old ones would be
//!   lost without an error. [`page_load`] drops every subscription of the webview through the
//!   manager, and each sink also carries the [`PageToken`] of the page that asked for it, so a sink
//!   registered while the reload was starting fails its first send instead of lingering (see
//!   [`Pages`] for an attach whose handler starts only after the reload began).
//! - **Startup** ([`start_chats`]): the chat store takes the opened store and the owner lock, then
//!   the leftover registry, the sweep and log retention (in the background), one redactor shared by
//!   the env resolver, the protocol logs and the manager, the detector, and the manager itself.
//! - **Quit** ([`Quit`], [`run_quit`]): the first exit request of any kind is prevented, the window
//!   hides, a paused store with unsaved data asks first, then every session stops within 8 s, the
//!   store commits and closes within 5 s, the registry file goes, and the app exits with the code
//!   it was asked for. `RunEvent::Exit` is the synchronous last resort ([`exit_last_resort`]).
//! - **Signals** (Unix, [`SignalListener`]): SIGTERM, SIGINT and SIGHUP start the same flow; a second
//!   one kills every agent at once and exits with 128 plus its number.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ghost_agents::Detector;
use ghost_core::chatstore::ChatStore;
use ghost_core::profile::Profile;
use ghost_core::session::{
    FrameSink, ManagerDeps, ManagerLimits, SessionManager, SessionTimings, ShutdownSummary,
    SinkGone, StatusSink,
};
use ghost_drivers::driver::DriverFactory;
use ghost_drivers::factory::StandardDriverFactory;
use ghost_process::{
    EnvResolver, EnvResolverConfig, LeftoverRegistry, LogFactory, ProcessSpawner, Redactor,
    RetentionPolicy, Spawner, SweepReport, sweep_stale,
};
use ghost_protocol::EventBatch;
use ghost_protocol::session::{ChatStatusUpdate, PauseReason, StorageState, StorageStatus};
use tauri::ipc::Channel;
use tauri::webview::PageLoadEvent;
use tokio::runtime::Handle;

use crate::boot::Booted;
use crate::error::{AppError, AppErrorCode};
use crate::sync::lock;

// ---------- Channels and pages ----------

/// Each webview's current page. A page that starts loading gets a new generation, and every
/// [`PageToken`] taken for the page before goes stale.
///
/// A command does not say which page sent it, and its handler can start after the next page began
/// loading: an attach the old page sent just before a reload would then get the new page's token
/// for a channel nobody reads (sending to it still succeeds), keeping that chat's frames flowing
/// into the webview and holding one of its subscriptions. So while a page loads, until the new
/// page subscribes to chat-list updates (the app store's first call) or finishes loading, a chat
/// view's token is stale from the start: its sink goes at its first frame, and a view of the new
/// page that met this joins again when its channel ends.
#[derive(Debug, Default)]
pub struct Pages {
    pages: Mutex<HashMap<String, Page>>,
}

#[derive(Debug, Default)]
struct Page {
    generation: Arc<AtomicU64>,
    /// A page started loading and has not shown itself yet.
    loading: bool,
}

impl Pages {
    pub(crate) fn invalidate_all(&self) -> Vec<String> {
        let mut pages = lock(&self.pages);
        for page in pages.values_mut() {
            page.generation.fetch_add(1, Ordering::AcqRel);
            page.loading = true;
        }
        pages.keys().cloned().collect()
    }
    fn token(&self, label: &str, settles: bool) -> PageToken {
        let mut pages = lock(&self.pages);
        let page = pages.entry(label.to_owned()).or_default();
        if settles {
            page.loading = false;
        }
        let generation = page.generation.load(Ordering::Acquire);
        PageToken {
            counter: Arc::clone(&page.generation),
            generation: (!page.loading).then_some(generation),
        }
    }

    /// The page a chat view's sink is made for now: stale from the start while a page loads,
    /// since the call may come from the page before it.
    pub fn for_view(&self, label: &str) -> PageToken {
        self.token(label, false)
    }

    /// The page a status channel's sink is made for now. The app store subscribes before anything
    /// else, so this is the new page's first call after a load: the page is settled from here on.
    pub fn for_status(&self, label: &str) -> PageToken {
        self.token(label, true)
    }

    /// A new page started loading in `label`.
    pub fn page_started(&self, label: &str) {
        let mut pages = lock(&self.pages);
        let page = pages.entry(label.to_owned()).or_default();
        page.generation.fetch_add(1, Ordering::AcqRel);
        page.loading = true;
    }

    /// The page in `label` finished loading: whatever the page before it sent was handled long
    /// before.
    pub fn page_finished(&self, label: &str) {
        if let Some(page) = lock(&self.pages).get_mut(label) {
            page.loading = false;
        }
    }
}

/// The page a sink was made for.
#[derive(Debug, Clone)]
pub struct PageToken {
    counter: Arc<AtomicU64>,
    /// `None`: made while a page loaded, so stale from the start.
    generation: Option<u64>,
}

impl PageToken {
    /// Whether that page is still the one its webview shows.
    pub fn is_current(&self) -> bool {
        self.generation == Some(self.counter.load(Ordering::Acquire))
    }
}

/// What a page load does: one that starts has new channel callbacks, so the webview's old
/// subscriptions are dropped (`drop_owner`, the manager's) and their sinks fail from now on; a
/// finished one settles the page. Returns whether the subscriptions were dropped.
pub fn page_load(
    pages: &Pages,
    label: &str,
    event: PageLoadEvent,
    drop_owner: impl FnOnce(&str),
) -> bool {
    match event {
        PageLoadEvent::Started => {
            pages.page_started(label);
            drop_owner(label);
            true
        }
        PageLoadEvent::Finished => {
            pages.page_finished(label);
            false
        }
    }
}

/// One open chat view's frames: the view's channel, the webview it belongs to, and the page that
/// asked for it.
pub struct ChannelSink {
    channel: Channel<EventBatch>,
    owner: String,
    page: PageToken,
}

impl ChannelSink {
    pub fn new(channel: Channel<EventBatch>, owner: impl Into<String>, page: PageToken) -> Self {
        ChannelSink {
            channel,
            owner: owner.into(),
            page,
        }
    }
}

impl FrameSink for ChannelSink {
    fn send(&self, frame: &EventBatch) -> Result<(), SinkGone> {
        if !self.page.is_current() {
            return Err(SinkGone);
        }
        self.channel.send(frame.clone()).map_err(|error| {
            tracing::debug!(%error, owner = %self.owner, chat = %frame.chat_id, "a chat view's channel failed; removing it");
            SinkGone
        })
    }

    fn owner(&self) -> &str {
        &self.owner
    }
}

/// One webview's chat-list updates.
pub struct StatusChannelSink {
    channel: Channel<ChatStatusUpdate>,
    owner: String,
    page: PageToken,
}

impl StatusChannelSink {
    pub fn new(
        channel: Channel<ChatStatusUpdate>,
        owner: impl Into<String>,
        page: PageToken,
    ) -> Self {
        StatusChannelSink {
            channel,
            owner: owner.into(),
            page,
        }
    }
}

impl StatusSink for StatusChannelSink {
    fn send(&self, update: &ChatStatusUpdate) -> Result<(), SinkGone> {
        if !self.page.is_current() {
            return Err(SinkGone);
        }
        self.channel.send(update.clone()).map_err(|error| {
            tracing::debug!(%error, owner = %self.owner, "a status channel failed; removing it");
            SinkGone
        })
    }

    fn owner(&self) -> &str {
        &self.owner
    }
}

// ---------- Startup ----------

/// What startup built for chats: the chat store, the session manager on top of it, and what
/// quitting needs.
pub struct ChatRuntime {
    pub store: Arc<ChatStore>,
    pub manager: SessionManager,
    pub registry: Arc<LeftoverRegistry>,
    /// Sentences that arrive after startup (the sweep's), shown with `app_info`'s notices.
    pub late_notices: Arc<Mutex<Vec<String>>>,
    pub(crate) services: Arc<RuntimeServices>,
    /// The profile's folder: the browser keeps its pages' data in `browser` inside it.
    pub profile_dir: PathBuf,
}

/// Process ownership and agent discovery belong to the app instance, not one profile. Reusing
/// these avoids opening competing registry files when a macOS profile switch prepares its target.
pub(crate) struct RuntimeServices {
    registry: Arc<LeftoverRegistry>,
    redactor: Arc<Redactor>,
    outside: Outside,
}

impl std::fmt::Debug for ChatRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatRuntime")
            .field("manager", &self.manager)
            .finish_non_exhaustive()
    }
}

/// The parts of the chat runtime that reach outside Ghost: the login shell, agent probes and
/// adapter installs, real processes, agent drivers. Tests pass their own.
#[derive(Clone)]
pub struct Outside {
    pub env: Arc<EnvResolver>,
    pub detector: Arc<Detector>,
    pub spawner: Arc<dyn ProcessSpawner>,
    pub drivers: Arc<dyn DriverFactory>,
}

impl Outside {
    /// The real ones: the user's login shell (its secret literals go to `redactor`), then
    /// everything [`Outside::over_env`] builds on it.
    pub fn real(
        registry: &Arc<LeftoverRegistry>,
        redactor: &Arc<Redactor>,
        data_dir: &Path,
        rt: &Handle,
    ) -> Outside {
        let env = EnvResolver::start(
            EnvResolverConfig {
                redactor: Some(Arc::clone(redactor)),
                ..EnvResolverConfig::standard()
            },
            Arc::clone(registry),
            rt.clone(),
        );
        let outside = Outside::over_env(env, registry, data_dir, rt);
        outside
            .detector
            .enable_release_checks(data_dir.join("catalog-releases.json"));
        outside
    }

    /// The real detector, processes and drivers over `env`: the catalog's agents with their
    /// adapters in `<data root>/adapters` (detection starts in the background once `env`
    /// resolves), processes registered in `registry`, and the drivers Ghost ships (the ACP
    /// driver and the demo agent's). [`Outside::real`] passes the login shell's resolver; a test
    /// passes a fixed env whose `PATH` finds no agent, so nothing it starts reaches a real one.
    pub fn over_env(
        env: Arc<EnvResolver>,
        registry: &Arc<LeftoverRegistry>,
        data_dir: &Path,
        rt: &Handle,
    ) -> Outside {
        let detector = Detector::with_preferences(
            Arc::clone(&env),
            Arc::clone(registry),
            data_dir.join(ADAPTERS_DIR),
            rt.clone(),
            data_dir.join("agent-executables.json"),
        );
        Outside {
            env,
            detector,
            spawner: Spawner::new(Arc::clone(registry), rt.clone()),
            drivers: StandardDriverFactory::new(),
        }
    }
}

/// `<data root>/run`: one leftover registry file per running Ghost process.
pub const RUN_DIR: &str = "run";

/// `<data root>/adapters`: the npm adapters, one folder per package version.
pub const ADAPTERS_DIR: &str = "adapters";

/// Starts the chat store and the session manager on what [`crate::boot::boot`] opened: the store
/// and the profile's owner lock move into the chat store's writer thread. `None` when startup did
/// not get that far (its error screen shows instead), and when the chat store cannot start, which
/// then becomes startup's fatal error. Every constructor that spawns gets `rt`, since Tauri's
/// `setup` hook runs outside any Tokio context.
pub fn start_chats(
    booted: &mut Booted,
    app_version: &str,
    rt: &Handle,
    outside: impl FnOnce(&Arc<LeftoverRegistry>, &Arc<Redactor>, &Path, &Handle) -> Outside,
) -> Option<ChatRuntime> {
    start_profile_chats(booted, app_version, rt, outside, None)
}

pub(crate) fn start_profile_chats(
    booted: &mut Booted,
    app_version: &str,
    rt: &Handle,
    outside: impl FnOnce(&Arc<LeftoverRegistry>, &Arc<Redactor>, &Path, &Handle) -> Outside,
    reuse: Option<Arc<RuntimeServices>>,
) -> Option<ChatRuntime> {
    if booted.fatal.is_some() {
        return None;
    }
    let (Some(data_dir), Some(meta), Some(dir), Some(device)) = (
        booted.data_dir.clone(),
        booted.profile.clone(),
        booted.profile_dir.clone(),
        booted.device.clone(),
    ) else {
        return None;
    };
    let (Some(store), Some(owner)) = (booted.store.take(), booted.owner.take()) else {
        return None;
    };
    let profile = Profile { meta, dir };
    let store = match ChatStore::start(store, owner, &profile, device.device_id.clone(), rt.clone())
    {
        Ok(store) => store,
        Err(error) => {
            tracing::error!(%error, "the chat store could not start");
            booted.fatal = Some(error.to_string());
            return None;
        }
    };

    let run_dir = data_dir.join(RUN_DIR);
    let registry = match reuse
        .as_ref()
        .map(|services| Ok(services.registry.clone()))
        .unwrap_or_else(|| LeftoverRegistry::open(&run_dir, rt.clone()))
    {
        Ok(registry) => registry,
        Err(error) => {
            tracing::warn!(%error, path = %run_dir.display(), "the leftover registry is unavailable; agents run without it");
            LeftoverRegistry::disabled()
        }
    };
    let late_notices = Arc::new(Mutex::new(Vec::new()));
    let own = registry.own().clone();
    let notices = Arc::clone(&late_notices);
    if reuse.is_none() {
        rt.spawn(async move {
            let report = sweep_stale(&run_dir, &own).await;
            log_sweep(&report);
            if let Some(notice) = sweep_notice(&report) {
                lock(&notices).push(notice);
            }
        });
    }

    let services = reuse.unwrap_or_else(|| {
        let redactor = Arc::new(Redactor::standard());
        Arc::new(RuntimeServices {
            outside: outside(&registry, &redactor, &data_dir, rt),
            registry: registry.clone(),
            redactor,
        })
    });
    let redactor = services.redactor.clone();
    let Outside {
        env,
        detector,
        spawner,
        drivers,
    } = services.outside.clone();
    let logs = LogFactory::new(
        profile.logs_dir().join("protocol"),
        Arc::clone(&redactor),
        rt.clone(),
    );
    let pruning = logs.clone();
    rt.spawn(async move {
        let report = pruning.prune(RetentionPolicy::STANDARD).await;
        if report.files_removed > 0 || !report.errors.is_empty() {
            tracing::info!(
                files = report.files_removed,
                bytes = report.bytes_removed,
                errors = report.errors.len(),
                "pruned old protocol logs"
            );
        }
    });
    let worktrees =
        ghost_core::gitworkspaces::GitWorkspaces::new(&profile, env.clone(), registry.clone());
    let checkpoints =
        ghost_core::checkpoints::Checkpoints::new(&profile, env.clone(), registry.clone());
    let manager = SessionManager::new(ManagerDeps {
        worktrees: Some(worktrees),
        checkpoints: Some(checkpoints),
        store: Arc::clone(&store),
        env,
        spawner,
        detector,
        drivers,
        logs,
        redactor,
        device_id: device.device_id,
        app_version: app_version.to_owned(),
        limits: ManagerLimits::STANDARD,
        timings: SessionTimings::STANDARD,
        rt: rt.clone(),
    });
    // Added agent accounts keep their tokens in secure storage and their names in a plain file.
    let tokens = crate::agent_account_commands::ProfileTokens::open()
        .map(|tokens| Arc::new(tokens) as Arc<dyn ghost_core::agent_accounts::TokenVault>);
    // Usage percentages and reset times only.
    manager.persist_agent_usage(&profile.dir.join("agent-usage.json"));
    manager.set_agent_accounts(Some(Arc::new(
        ghost_core::agent_accounts::AgentAccountsStore::open(&profile.dir, tokens),
    )));
    Some(ChatRuntime {
        store,
        manager,
        registry,
        late_notices,
        services,
        profile_dir: profile.dir.clone(),
    })
}

fn log_sweep(report: &SweepReport) {
    if report.files_removed == 0 && report.killed.is_empty() && report.errors.is_empty() {
        return;
    }
    tracing::info!(
        files = report.files_removed,
        killed = report.killed.len(),
        live_owners = report.skipped_live_owner,
        reused = report.skipped_reused,
        errors = report.errors.len(),
        "swept what earlier sessions left running"
    );
}

/// The startup notice when the sweep stopped processes a crashed session left running.
pub fn sweep_notice(report: &SweepReport) -> Option<String> {
    match report.killed.len() {
        0 => None,
        1 => Some(
            "Ghost stopped 1 agent process left running by an earlier session that ended unexpectedly."
                .into(),
        ),
        n => Some(format!(
            "Ghost stopped {n} agent processes left running by an earlier session that ended unexpectedly."
        )),
    }
}

// ---------- Quit ----------

/// `shutdown_all`'s deadline when quitting.
pub const QUIT_SESSIONS_DEADLINE: Duration = Duration::from_secs(8);
/// The chat store's deadline when quitting, after the sessions.
pub const QUIT_STORE_DEADLINE: Duration = Duration::from_secs(5);
/// Ordinary quit keeps the app open if an owned file operation or vendor updater needs more time.
pub const QUIT_OWNED_WORK_DEADLINE: Duration = Duration::from_secs(5);
/// How much longer than a step's own deadline the quit flow waits for it. The sessions and the
/// store keep their deadlines themselves; this bounds one that does not, since the window is
/// hidden and every other exit request waits for the flow.
pub const QUIT_STEP_MARGIN: Duration = Duration::from_secs(1);
/// The chat store's deadline at `RunEvent::Exit` when the quit flow did not close it.
pub const EXIT_STORE_DEADLINE: Duration = Duration::from_secs(1);

/// Where quitting stands, shared by the run-event callback, the quit task and the signal task.
#[derive(Debug, Default)]
pub struct Quit {
    phase: Mutex<QuitPhase>,
    store_closed: AtomicBool,
    /// The person already agreed to stop everything (the data cleanup's review): the next quit
    /// does not ask again. Cleared when Ghost keeps running.
    confirmed: AtomicBool,
    /// Signals since Ghost last kept running: the second one exits at once.
    signals: AtomicU32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum QuitPhase {
    #[default]
    Running,
    Quitting,
    Done,
}

/// What to do with an exit request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitRequest {
    /// The quit flow finished: let the app exit.
    Allow,
    /// The first request: prevent it and run the quit flow.
    StartQuit,
    /// A quit flow already runs: prevent this one too; the flow exits when it is done.
    Wait,
}

impl Quit {
    /// Decides on one `RunEvent::ExitRequested`, whatever its code or cause.
    pub fn on_exit_requested(&self) -> ExitRequest {
        let mut phase = lock(&self.phase);
        match *phase {
            QuitPhase::Running => {
                *phase = QuitPhase::Quitting;
                ExitRequest::StartQuit
            }
            QuitPhase::Quitting => ExitRequest::Wait,
            QuitPhase::Done => ExitRequest::Allow,
        }
    }

    /// Whether the quit flow finished (the app is exiting).
    pub fn is_done(&self) -> bool {
        *lock(&self.phase) == QuitPhase::Done
    }

    /// Whether a quit flow runs or finished.
    pub fn is_quitting(&self) -> bool {
        *lock(&self.phase) != QuitPhase::Running
    }

    /// Whether the quit flow closed the chat store (`RunEvent::Exit` then leaves it alone).
    pub fn store_closed(&self) -> bool {
        self.store_closed.load(Ordering::Acquire)
    }

    /// Counts a signal; returns how many arrived since Ghost last kept running, this one
    /// included.
    pub fn count_signal(&self) -> u32 {
        self.signals
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1)
    }

    /// The next quit stops agents and terminals without asking: the person already agreed in a
    /// review that said so (Settings' "Delete all Ghost data").
    pub fn confirm_ahead(&self) {
        self.confirmed.store(true, Ordering::Release);
    }

    fn confirmed_ahead(&self) -> bool {
        self.confirmed.load(Ordering::Acquire)
    }

    /// The quit flow died before it finished: the next exit request passes, and `RunEvent::Exit`
    /// kills and closes whatever is left.
    pub fn abandon(&self) {
        self.finish();
    }

    /// The user kept Ghost open: the flow is over, so a later signal starts a new one instead
    /// of counting as the second signal of this one.
    fn keep_running(&self) {
        *lock(&self.phase) = QuitPhase::Running;
        self.signals.store(0, Ordering::Release);
        self.confirmed.store(false, Ordering::Release);
    }

    fn finish(&self) {
        *lock(&self.phase) = QuitPhase::Done;
    }

    fn mark_store_closed(&self) {
        self.store_closed.store(true, Ordering::Release);
    }
}

/// What the quit flow acts on: the window, the dialog, the sessions, the store, the registry and
/// the app. The app's host is [`AppQuitHost`]; tests record calls.
pub trait QuitHost: Send + Sync {
    /// The opaque RAII guard blocks new owned work until this entire quit attempt ends.
    fn owned_work_guard(&self) -> Option<Box<dyn Send + '_>> {
        Some(Box::new(()))
    }
    fn wait_owned_work(&self, _deadline: Duration) -> impl Future<Output = bool> + Send {
        async { true }
    }
    fn owned_work_blocked(&self) {}
    fn project_terminal_count(&self) -> usize {
        0
    }
    fn close_project_terminals(&self, _deadline: Duration) -> impl Future<Output = bool> + Send {
        async { true }
    }
    fn terminal_cleanup_blocked(&self) {
        self.owned_work_blocked();
    }
    /// Save editor state while the database and webview are still available.
    fn flush_drafts(&self) -> impl Future<Output = bool> + Send {
        async { true }
    }
    fn hide_window(&self);
    fn show_window(&self);
    fn storage(&self) -> StorageStatus;
    /// Running/starting/stopping work or unresolved requests, excluding merely idle processes.
    fn active_work_count(&self) -> usize {
        0
    }
    /// Asks whether to quit although saving is paused; `true` quits.
    fn ask_to_quit_unsaved(&self, question: QuitQuestion) -> impl Future<Output = bool> + Send;
    fn stop_sessions(&self, deadline: Duration) -> impl Future<Output = ShutdownSummary> + Send;
    /// Commits what is queued and closes the store; `false` when not everything was saved.
    fn close_store(&self, deadline: Duration) -> impl Future<Output = bool> + Send;
    fn close_registry(&self);
    fn exit(&self, code: i32);
}

/// The question before quitting while saving is paused with unsaved data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuitQuestion {
    pub message: String,
    pub quit: String,
    pub keep: String,
}

/// The question to ask before quitting with `storage`, if any: only a paused store with unsaved
/// data loses something when Ghost quits.
pub fn quit_question(storage: &StorageStatus) -> Option<QuitQuestion> {
    let StorageState::Paused { reason, .. } = &storage.state else {
        return None;
    };
    if storage.unsaved_bytes == 0 {
        return None;
    }
    let message = match reason {
        PauseReason::DiskFull => {
            "Ghost can't save its latest messages because the disk is full. If you quit now, they are lost."
        }
        PauseReason::Busy => {
            "Ghost can't save its latest messages because its database is busy. If you quit now, they are lost."
        }
    };
    Some(QuitQuestion {
        message: message.into(),
        quit: "Quit anyway".into(),
        keep: "Keep Ghost open".into(),
    })
}

pub fn quit_question_with_work(storage: &StorageStatus, active: usize) -> Option<QuitQuestion> {
    let mut question = quit_question(storage);
    if active > 0 {
        let work = if active == 1 {
            "One chat is still active. Quitting stops its agent.".to_owned()
        } else {
            format!("{active} chats are still active. Quitting stops their agents.")
        };
        match &mut question {
            Some(question) => question.message = format!("{work}\n\n{}", question.message),
            None => {
                question = Some(QuitQuestion {
                    message: work,
                    quit: "Stop agents and quit".into(),
                    keep: "Keep Ghost open".into(),
                })
            }
        }
    }
    question
}

/// How a quit flow ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitOutcome {
    /// The user kept Ghost open; the window is back and the next exit request starts over.
    Kept,
    /// Everything stopped and the app was asked to exit with the request's code.
    Exited {
        /// `None` when the sessions did not stop within their deadline plus the margin.
        summary: Option<ShutdownSummary>,
        /// Whether the store saved everything before closing.
        saved: bool,
    },
}

/// The quit flow first drains owned file/vendor work without closing storage, keeping the app open
/// on timeout. Once drained: when saving is paused with unsaved data or work is active, ask first,
/// in the window, which comes forward for it (keeping Ghost open ends the quit); then hide the
/// window, stop every session within 8 s, commit and close the store within 5 s, delete the
/// registry file, and exit with `code`. The next exit request passes ([`Quit::on_exit_requested`]).
///
/// The window is hidden and every other exit request waits for this flow, so a step that overruns
/// a session/store deadline by [`QUIT_STEP_MARGIN`] is left behind and the flow carries on:
/// `RunEvent::Exit` then kills owned agent processes and closes the store if it did not close here.
/// The owned-file preflight never takes that forced path.
pub async fn run_quit(quit: &Quit, host: &impl QuitHost, code: i32) -> QuitOutcome {
    let Some(_owned_work) = host.owned_work_guard() else {
        host.show_window();
        host.owned_work_blocked();
        quit.keep_running();
        return QuitOutcome::Kept;
    };
    if !tokio::time::timeout(
        QUIT_OWNED_WORK_DEADLINE,
        host.wait_owned_work(QUIT_OWNED_WORK_DEADLINE),
    )
    .await
    .unwrap_or(false)
    {
        host.show_window();
        host.owned_work_blocked();
        quit.keep_running();
        return QuitOutcome::Kept;
    }
    let drafts_saved = tokio::time::timeout(Duration::from_secs(3), host.flush_drafts())
        .await
        .unwrap_or(false);
    let mut question = quit_question_with_work(&host.storage(), host.active_work_count());
    let terminals = host.project_terminal_count();
    if terminals > 0 {
        let message = if terminals == 1 {
            "One project terminal is open. Quitting stops its shell and owned child processes."
                .into()
        } else {
            format!(
                "{terminals} project terminals are open. Quitting stops their shells and owned child processes."
            )
        };
        match &mut question {
            Some(question) => {
                question.message = format!("{message}\n\n{}", question.message);
                question.quit = "Stop work and quit".into();
            }
            None => {
                question = Some(QuitQuestion {
                    message,
                    quit: "Close terminals and quit".into(),
                    keep: "Keep Ghost open".into(),
                })
            }
        }
    }
    if !drafts_saved {
        let message =
            "An unsent draft could not be confirmed saved. Quitting may discard its latest edits.";
        match &mut question {
            Some(question) => question.message = format!("{message}\n\n{}", question.message),
            None => {
                question = Some(QuitQuestion {
                    message: message.into(),
                    quit: "Quit anyway".into(),
                    keep: "Keep Ghost open".into(),
                })
            }
        }
    }
    if let Some(question) = question
        && !quit.confirmed_ahead()
        && !host.ask_to_quit_unsaved(question).await
    {
        host.show_window();
        quit.keep_running();
        tracing::info!("kept Ghost open after the quit question");
        return QuitOutcome::Kept;
    }
    host.hide_window();
    if terminals > 0
        && !tokio::time::timeout(
            QUIT_SESSIONS_DEADLINE,
            host.close_project_terminals(QUIT_SESSIONS_DEADLINE),
        )
        .await
        .unwrap_or(false)
    {
        host.show_window();
        host.terminal_cleanup_blocked();
        quit.keep_running();
        return QuitOutcome::Kept;
    }
    let summary = tokio::time::timeout(
        QUIT_SESSIONS_DEADLINE + QUIT_STEP_MARGIN,
        host.stop_sessions(QUIT_SESSIONS_DEADLINE),
    )
    .await
    .inspect_err(|_| {
        tracing::warn!("the sessions did not stop within their deadline; quitting anyway")
    })
    .ok();
    let saved = match tokio::time::timeout(
        QUIT_STORE_DEADLINE + QUIT_STEP_MARGIN,
        host.close_store(QUIT_STORE_DEADLINE),
    )
    .await
    {
        Ok(saved) => {
            quit.mark_store_closed();
            saved
        }
        Err(_) => {
            tracing::warn!(
                "the chat store did not close within its deadline; it gets one more second at exit"
            );
            false
        }
    };
    host.close_registry();
    match summary {
        Some(summary) => tracing::info!(
            sessions = summary.sessions,
            graceful = summary.graceful,
            forced = summary.forced,
            took_ms = u64::try_from(summary.took.as_millis()).unwrap_or(u64::MAX),
            saved,
            code,
            "quitting"
        ),
        None => tracing::info!(saved, code, "quitting"),
    }
    quit.finish();
    host.exit(code);
    QuitOutcome::Exited { summary, saved }
}

/// The app's quit host: the main window, tauri-plugin-dialog for the question, and the chat runtime.
pub struct AppQuitHost<'a> {
    pub app: &'a tauri::AppHandle,
    pub chats: &'a ChatRuntime,
}

impl QuitHost for AppQuitHost<'_> {
    fn owned_work_guard(&self) -> Option<Box<dyn Send + '_>> {
        // lib::quit already holds the transition mutex across runtime selection and this flow.
        self.chats
            .manager
            .try_drain_owned_work()
            .map(|guard| Box::new(guard) as Box<dyn Send>)
    }

    fn project_terminal_count(&self) -> usize {
        self.chats.manager.project_terminal_count()
    }

    async fn close_project_terminals(&self, deadline: Duration) -> bool {
        use tauri::Manager as _;
        self.app
            .state::<crate::state::AppState>()
            .terminals
            .close_projects();
        self.chats.manager.wait_project_terminals(deadline).await
    }

    fn terminal_cleanup_blocked(&self) {
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
        self.app.dialog().message("A project terminal's process cleanup could not be confirmed. Ghost stayed open and kept its folder protected. Review owned-process recovery before retrying quit.").title("Terminal cleanup needs attention").buttons(MessageDialogButtons::Ok).show(|_|{});
    }

    async fn wait_owned_work(&self, deadline: Duration) -> bool {
        self.chats.manager.wait_owned_work(deadline).await
    }

    fn owned_work_blocked(&self) {
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
        self.app.dialog().message("A folder operation, configuration copy, or agent update is still finishing. Ghost has stayed open to preserve that work. Try quitting again when it finishes.")
            .title("Work is still finishing").buttons(MessageDialogButtons::Ok).show(|_| {});
    }
    async fn flush_drafts(&self) -> bool {
        use tauri::{Emitter as _, Listener as _};
        let token = ghost_core::ids::new_id();
        let expected = token.clone();
        let (reply, answer) = tokio::sync::oneshot::channel();
        let reply = Mutex::new(Some(reply));
        let listener = self.app.listen("auracode-drafts-flushed", move |event| {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
                return;
            };
            if value["token"].as_str() == Some(expected.as_str())
                && let Some(reply) = lock(&reply).take()
            {
                let _ = reply.send(value["saved"].as_bool().unwrap_or(false));
            }
        });
        let sent = self
            .app
            .emit_to(crate::window::MAIN_WINDOW, "auracode-flush-drafts", token)
            .is_ok();
        let saved = if sent {
            tokio::time::timeout(Duration::from_secs(2), answer)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or(false)
        } else {
            false
        };
        self.app.unlisten(listener);
        saved
    }

    fn hide_window(&self) {
        use tauri::Manager as _;
        if let Some(window) = self.app.get_webview_window(crate::window::MAIN_WINDOW)
            && let Err(error) = window.hide()
        {
            tracing::warn!(%error, "could not hide the window while quitting");
        }
    }

    fn show_window(&self) {
        crate::window::focus_main(self.app);
    }

    fn storage(&self) -> StorageStatus {
        self.chats.manager.storage_status()
    }

    fn active_work_count(&self) -> usize {
        use tauri::Manager as _;
        self.app
            .try_state::<Arc<crate::native_shell::NativeShell>>()
            .map_or(0, |shell| shell.active_work_count())
    }

    async fn ask_to_quit_unsaved(&self, question: QuitQuestion) -> bool {
        use tauri::{Emitter as _, Listener as _};
        // The window comes forward and asks with its own dark dialog, Keep open focused.
        crate::window::focus_main(self.app);
        let token = ghost_core::ids::new_id();
        let (shown_tx, shown) = tokio::sync::oneshot::channel();
        let (answer_tx, answer) = tokio::sync::oneshot::channel();
        let shown_tx = Mutex::new(Some(shown_tx));
        let answer_tx = Mutex::new(Some(answer_tx));
        let expected = token.clone();
        let shown_listener = self.app.listen(QUIT_QUESTION_SHOWN_EVENT, move |event| {
            if token_of(event.payload()).as_deref() == Some(expected.as_str())
                && let Some(tx) = lock(&shown_tx).take()
            {
                let _ = tx.send(());
            }
        });
        let expected = token.clone();
        let answer_listener = self.app.listen(QUIT_ANSWER_EVENT, move |event| {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
                return;
            };
            if value["token"].as_str() == Some(expected.as_str())
                && let Some(tx) = lock(&answer_tx).take()
            {
                let _ = tx.send(value["quit"].as_bool() == Some(true));
            }
        });
        let sent = self
            .app
            .emit_to(
                crate::window::MAIN_WINDOW,
                QUIT_QUESTION_EVENT,
                serde_json::json!({
                    "token": token,
                    "message": question.message,
                    "quit": question.quit,
                    "keep": question.keep,
                }),
            )
            .is_ok();
        let in_app = ask_in_app(sent, shown, answer, QUIT_QUESTION_SHOWN_WITHIN).await;
        self.app.unlisten(shown_listener);
        self.app.unlisten(answer_listener);
        match in_app {
            Some(quit) => quit,
            None => {
                tracing::warn!(
                    "the window did not show the quit question in time; asking with the system dialog"
                );
                self.ask_natively(question).await
            }
        }
    }

    async fn stop_sessions(&self, deadline: Duration) -> ShutdownSummary {
        self.chats.manager.shutdown_all(deadline).await
    }

    async fn close_store(&self, deadline: Duration) -> bool {
        self.chats.store.shutdown(deadline).await
    }

    fn close_registry(&self) {
        if let Err(error) = self.chats.registry.close() {
            tracing::warn!(%error, "could not remove this instance's leftover registry file");
        }
    }

    fn exit(&self, code: i32) {
        self.app.exit(code);
    }
}

impl AppQuitHost<'_> {
    /// The fallback when the window cannot ask: the system dialog, Keep open first and default, so
    /// Enter, Esc and closing it all keep Ghost open; only the quit button quits.
    async fn ask_natively(&self, question: QuitQuestion) -> bool {
        use tauri_plugin_dialog::{DialogExt as _, MessageDialogButtons, MessageDialogKind};
        let (tx, rx) = tokio::sync::oneshot::channel();
        let quit_label = question.quit.clone();
        self.app
            .dialog()
            .message(question.message)
            .title("Ghost")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                question.keep,
                question.quit,
            ))
            .show_with_result(move |result| {
                // Nobody waits any more only when the quit task itself is gone.
                let _ = tx.send(native_quit_choice(&result, &quit_label));
            });
        match rx.await {
            Ok(quit) => quit,
            Err(_) => {
                // The dialog never answered: keeping Ghost open loses nothing.
                tracing::warn!("the quit question was not answered; keeping Ghost open");
                false
            }
        }
    }
}

/// The window's quit question: Rust sends it, the page answers that it shows it, then the choice.
pub const QUIT_QUESTION_EVENT: &str = "auracode-quit-question";
pub const QUIT_QUESTION_SHOWN_EVENT: &str = "auracode-quit-question-shown";
pub const QUIT_ANSWER_EVENT: &str = "auracode-quit-answer";
/// How long the page has to show the question before the system dialog asks instead. The person
/// then takes as long as they like to choose.
pub const QUIT_QUESTION_SHOWN_WITHIN: Duration = Duration::from_secs(3);

fn token_of(payload: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()?
        .get("token")?
        .as_str()
        .map(str::to_owned)
}

/// The in-app question's outcome: `None` when it was not sent or the page did not show it within
/// `within` (not loaded, hung), so the system dialog asks; otherwise the choice, waited for as long
/// as it takes. An answer channel that closes without a choice keeps Ghost open.
pub async fn ask_in_app(
    sent: bool,
    shown: tokio::sync::oneshot::Receiver<()>,
    answer: tokio::sync::oneshot::Receiver<bool>,
    within: Duration,
) -> Option<bool> {
    if !sent {
        return None;
    }
    match tokio::time::timeout(within, shown).await {
        Ok(Ok(())) => Some(answer.await.unwrap_or(false)),
        Ok(Err(_)) | Err(_) => None,
    }
}

/// The system dialog's buttons are Keep (first, the default) and the quit label: only that label
/// quits. Closing the dialog, Esc and Enter keep Ghost open.
pub fn native_quit_choice(
    result: &tauri_plugin_dialog::MessageDialogResult,
    quit_label: &str,
) -> bool {
    matches!(result, tauri_plugin_dialog::MessageDialogResult::Custom(label) if label == quit_label)
}

/// `RunEvent::Exit`, the synchronous last resort: Tauri runs it on the main thread outside any
/// Tokio context, after the quit flow or instead of it (macOS can end the app without an exit
/// request). Every recorded agent process is killed at once, and the store closes within 1 s
/// unless the quit flow closed it. Returns whether this closed the store.
pub fn exit_last_resort(quit: &Quit, chats: &ChatRuntime) -> bool {
    chats.manager.kill_all_now();
    if quit.store_closed() {
        return false;
    }
    if !chats.store.shutdown_blocking(EXIT_STORE_DEADLINE) {
        tracing::warn!("the chat store could not save everything before Ghost exited");
    }
    quit.mark_store_closed();
    true
}

// ---------- Signals ----------

/// What a signal acts on. The app's host is [`AppSignals`]; tests record calls.
pub trait SignalHost: Send + Sync + 'static {
    /// The first signal: the quit flow, through an exit request (`app.exit(0)`).
    fn request_exit(&self);
    /// A later one: kill every agent now...
    fn kill_now(&self);
    /// ...close the store within 1 s...
    fn close_store_now(&self);
    /// ...and end the process with `code`.
    fn exit_now(&self, code: i32);
}

/// What one signal does: the first starts the quit flow, any later one (the flow is running) kills
/// every agent, closes the store within 1 s and exits with 128 plus the signal's number. Keeping
/// Ghost open at the flow's question starts the count over. Returns the exit code of a hard
/// exit.
pub fn on_signal(quit: &Quit, host: &impl SignalHost, number: i32) -> Option<i32> {
    if quit.count_signal() == 1 {
        tracing::info!(signal = number, "quitting on a signal");
        host.request_exit();
        return None;
    }
    let code = 128 + number;
    tracing::warn!(
        signal = number,
        code,
        "another signal while quitting; exiting at once"
    );
    host.kill_now();
    host.close_store_now();
    host.exit_now(code);
    Some(code)
}

/// SIGHUP, SIGINT and SIGTERM: the same numbers on every Unix.
#[cfg(unix)]
pub const SIGHUP: i32 = 1;
#[cfg(unix)]
pub const SIGINT: i32 = 2;
#[cfg(unix)]
pub const SIGTERM: i32 = 15;

/// SIGTERM, SIGINT and SIGHUP, listened for from creation on: a Ctrl+C under `pnpm dev` or a
/// logout runs the quit flow.
#[cfg(unix)]
pub struct SignalListener {
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
    hup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl SignalListener {
    /// Installs the handlers; needs a Tokio context (call it inside `rt.spawn` or `rt.enter`).
    pub fn new() -> std::io::Result<SignalListener> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(SignalListener {
            term: signal(SignalKind::terminate())?,
            int: signal(SignalKind::interrupt())?,
            hup: signal(SignalKind::hangup())?,
        })
    }

    /// Handles signals until a hard exit (which a real host never returns from).
    pub async fn run(mut self, quit: Arc<Quit>, host: impl SignalHost) {
        loop {
            let number = tokio::select! {
                Some(()) = self.term.recv() => SIGTERM,
                Some(()) = self.int.recv() => SIGINT,
                Some(()) = self.hup.recv() => SIGHUP,
                else => return,
            };
            if on_signal(&quit, &host, number).is_some() {
                return;
            }
        }
    }
}

/// The app's signal host.
pub struct AppSignals {
    pub app: tauri::AppHandle,
}

impl SignalHost for AppSignals {
    fn request_exit(&self) {
        self.app.exit(0);
    }

    fn kill_now(&self) {
        use tauri::Manager as _;
        if let Some(state) = self.app.try_state::<crate::state::AppState>()
            && let Some(chats) = state.chats()
        {
            chats.manager.kill_all_now();
        }
    }

    fn close_store_now(&self) {
        use tauri::Manager as _;
        if let Some(state) = self.app.try_state::<crate::state::AppState>()
            && let Some(chats) = state.chats()
            && !state.quit.store_closed()
            && !chats.store.shutdown_blocking(EXIT_STORE_DEADLINE)
        {
            tracing::warn!("the chat store could not save everything before Ghost exited");
        }
    }

    fn exit_now(&self, code: i32) {
        std::process::exit(code);
    }
}

// ---------- Folder picker and protocol logs ----------

/// The folder picker's answer as `pick_folder` returns it: `None` when the user cancelled. A chat
/// folder must be valid Unicode (the agent gets it as a JSON string), so a name that is not reads
/// as a failure that says why rather than a mangled path.
pub fn picked_folder(
    picked: Option<tauri_plugin_dialog::FilePath>,
) -> Result<Option<String>, AppError> {
    let Some(picked) = picked else {
        return Ok(None);
    };
    // `simplified` drops a Windows `\\?\` prefix, which Node agents mishandle in a `cwd`.
    let path = picked.simplified().into_path().map_err(|error| {
        AppError::new(
            AppErrorCode::DialogFailed,
            format!("The folder picker returned something that isn't a folder: {error}"),
        )
    })?;
    path.into_os_string()
        .into_string()
        .map(Some)
        .map_err(|path| {
            AppError::new(
                AppErrorCode::DialogFailed,
                format!(
                    "{} can't be used: its name isn't valid Unicode.",
                    PathBuf::from(path).display()
                ),
            )
        })
}

/// The newest protocol log in a chat's log folder: logs are named `<spawn id>.jsonl` with UUIDv7
/// spawn ids, so the greatest name is the newest spawn; rotated files (`<spawn id>.1.jsonl`) are
/// older parts of a log. `None` when the folder holds none.
pub fn newest_log(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".jsonl")?;
            let current = !stem.is_empty() && !stem.contains('.');
            (current && entry.file_type().ok()?.is_file()).then_some(name)
        })
        .max()
        .map(|name| dir.join(name))
}

/// Reveals the newest protocol log in `dir` with `reveal` (the opener plugin's `reveal_item_in_dir`
/// in the app) and returns its path. Generic over `reveal` so tests can check what would be shown
/// without opening a file manager window.
pub fn reveal_newest_log<E: std::fmt::Display>(
    dir: &Path,
    reveal: impl FnOnce(&Path) -> Result<(), E>,
) -> Result<PathBuf, AppError> {
    let path = newest_log(dir).ok_or_else(|| {
        AppError::new(
            AppErrorCode::RevealFailed,
            "This chat has no protocol log yet: one is written once its agent starts.",
        )
    })?;
    reveal(&path).map_err(|error| {
        AppError::new(
            AppErrorCode::RevealFailed,
            format!(
                "Could not show {} in the file manager: {error}",
                path.display()
            ),
        )
    })?;
    Ok(path)
}

/// A chat id names one folder under the protocol log root and nothing else: a single normal path
/// component without separators. The page passes chat ids, so one like `../..` must never reach a
/// path join.
pub fn is_plain_id(id: &str) -> bool {
    let mut components = Path::new(id).components();
    !id.contains(['/', '\\'])
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use ghost_agents::FixedAgent;
    use ghost_drivers::testing::ScriptedDriverFactory;
    use ghost_process::testing::FakeSpawner;
    use ghost_process::{EnvMap, EnvSource, ResolvedEnv};
    use ghost_protocol::session::Availability;

    /// Nothing that reaches outside: a fixed env, the demo agent through a fixed detector, fake
    /// processes, no drivers.
    pub fn outside(
        _registry: &Arc<LeftoverRegistry>,
        _redactor: &Arc<Redactor>,
        _data_dir: &Path,
        _rt: &Handle,
    ) -> Outside {
        let mut vars = EnvMap::new();
        vars.insert("PATH".into(), "/usr/bin:/bin".into());
        let env = EnvResolver::fixed(ResolvedEnv {
            vars,
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        });
        let mock = ghost_agents::get("mock").expect("the demo agent").clone();
        let detector = Detector::fixed(vec![FixedAgent::new(
            mock,
            Availability::Ready {
                version: None,
                path: "builtin".into(),
            },
            None,
        )]);
        let (spawner, _ends) = FakeSpawner::new();
        Outside {
            env,
            detector,
            spawner,
            drivers: ScriptedDriverFactory::new(Vec::new()),
        }
    }

    /// [`start_chats`] on the current runtime with [`outside`].
    pub fn start(booted: &mut Booted) -> Option<ChatRuntime> {
        start_chats(booted, "0.1.0", &Handle::current(), outside)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use ghost_core::paths::DataRoot;
    use ghost_core::store::OpenOutcome;
    use tauri::ipc::InvokeResponseBody;

    use super::*;
    use crate::boot::{boot, boot_next};

    fn frame(chat: &str, seq: u32) -> EventBatch {
        EventBatch {
            chat_id: chat.into(),
            seq_start: seq,
            events: vec![ghost_protocol::GhostEvent::Notice {
                level: ghost_protocol::NoticeLevel::Info,
                message: format!("frame {seq}"),
            }],
        }
    }

    /// A channel whose messages land in `seen`, failing every send once `fail` is set.
    fn recording<T>(seen: &Arc<Mutex<Vec<String>>>, fail: &Arc<AtomicBool>) -> Channel<T> {
        let seen = Arc::clone(seen);
        let fail = Arc::clone(fail);
        Channel::new(move |body| {
            if fail.load(Ordering::Acquire) {
                return Err(tauri::Error::Io(std::io::Error::other(
                    "the webview is gone",
                )));
            }
            match body {
                InvokeResponseBody::Json(json) => lock(&seen).push(json),
                InvokeResponseBody::Raw(bytes) => {
                    lock(&seen).push(format!("{} bytes", bytes.len()))
                }
            }
            Ok(())
        })
    }

    #[test]
    fn a_channel_sink_sends_frames_as_json_and_names_its_webview() {
        let pages = Pages::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let fail = Arc::new(AtomicBool::new(false));
        let sink = ChannelSink::new(recording(&seen, &fail), "main", pages.for_view("main"));
        assert_eq!(sink.owner(), "main");
        sink.send(&frame("c1", 7)).expect("sent");
        let sent = lock(&seen).clone();
        assert_eq!(sent.len(), 1);
        let value: serde_json::Value = serde_json::from_str(&sent[0]).expect("JSON");
        assert_eq!(value["chat_id"], "c1");
        assert_eq!(value["seq_start"], 7);
        assert_eq!(value["events"][0]["type"], "notice");
    }

    /// A failed send answers `SinkGone`, which is how the manager removes a subscriber.
    #[test]
    fn a_channel_sink_removes_itself_on_a_send_error() {
        let pages = Pages::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let fail = Arc::new(AtomicBool::new(false));
        let sink = ChannelSink::new(recording(&seen, &fail), "main", pages.for_view("main"));
        fail.store(true, Ordering::Release);
        assert_eq!(sink.send(&frame("c1", 0)), Err(SinkGone));

        let status =
            StatusChannelSink::new(recording(&seen, &fail), "main", pages.for_status("main"));
        let update = ChatStatusUpdate {
            chat_id: "c1".into(),
            status: ghost_protocol::SessionStatus::Ready,
            needs_attention: false,
            title: None,
            updated_at: 1,
        };
        assert_eq!(status.send(&update), Err(SinkGone));
        fail.store(false, Ordering::Release);
        status.send(&update).expect("sent once the channel works");
        assert!(lock(&seen)[0].contains("\"status\":\"ready\""));
    }

    /// Counts its drops: a channel holding one shows when its last clone went, which for a
    /// channel the page made is when Tauri ends it on the JavaScript side.
    struct Counted(Arc<AtomicUsize>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// A channel that accepts everything and counts its drop in `dropped`.
    fn counted<T>(dropped: &Arc<AtomicUsize>) -> Channel<T> {
        let guard = Counted(Arc::clone(dropped));
        Channel::new(move |_| {
            let _keep = &guard;
            Ok(())
        })
    }

    /// Detaching drops the manager's clone of the sink, the last one: Tauri then ends the
    /// JavaScript channel.
    #[test]
    fn dropping_the_sink_drops_its_channel() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let channel = counted::<EventBatch>(&dropped);
        let pages = Pages::default();
        let sink: Arc<dyn FrameSink> =
            Arc::new(ChannelSink::new(channel, "main", pages.for_view("main")));
        let manager_clone = Arc::clone(&sink);
        drop(sink);
        assert_eq!(
            dropped.load(Ordering::Acquire),
            0,
            "the manager still holds it"
        );
        drop(manager_clone);
        assert_eq!(dropped.load(Ordering::Acquire), 1);
    }

    #[test]
    fn page_load_drops_subscriptions() {
        let pages = Pages::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let fail = Arc::new(AtomicBool::new(false));
        let view =
            |label: &str| ChannelSink::new(recording(&seen, &fail), label, pages.for_view(label));
        let update = ChatStatusUpdate {
            chat_id: "c1".into(),
            status: ghost_protocol::SessionStatus::Stopped,
            needs_attention: false,
            title: None,
            updated_at: 0,
        };
        let old_view = view("main");
        let old_status =
            StatusChannelSink::new(recording(&seen, &fail), "main", pages.for_status("main"));
        let other_window = view("other");
        old_view.send(&frame("c1", 0)).expect("the page is current");

        let mut dropped = Vec::new();
        assert!(!page_load(
            &pages,
            "main",
            PageLoadEvent::Finished,
            |owner| { dropped.push(owner.to_owned()) }
        ));
        assert!(dropped.is_empty(), "a finished load drops nothing");
        old_view.send(&frame("c1", 1)).expect("still current");

        assert!(page_load(&pages, "main", PageLoadEvent::Started, |owner| {
            dropped.push(owner.to_owned())
        }));
        assert_eq!(
            dropped,
            ["main"],
            "the manager drops the webview's subscriptions"
        );
        // Every sink of the old page fails from now on, so one the manager registered while the
        // reload started is removed at its first frame instead of lingering.
        assert_eq!(old_view.send(&frame("c1", 2)), Err(SinkGone));
        assert_eq!(old_status.send(&update), Err(SinkGone));
        other_window
            .send(&frame("c2", 0))
            .expect("another webview keeps its subscriptions");
        // An attach the old page sent just before the reload, whose handler starts only now,
        // cannot tell the pages apart: its sink is stale from the start.
        assert_eq!(view("main").send(&frame("c1", 3)), Err(SinkGone));

        // The new page subscribes to chat-list updates before anything else; from then on its
        // views work.
        let new_status =
            StatusChannelSink::new(recording(&seen, &fail), "main", pages.for_status("main"));
        new_status
            .send(&update)
            .expect("the new page's status channel works");
        view("main")
            .send(&frame("c1", 4))
            .expect("the new page's views work");

        // A page that never subscribes is settled once it has loaded.
        page_load(&pages, "main", PageLoadEvent::Started, |_| {});
        assert_eq!(view("main").send(&frame("c1", 5)), Err(SinkGone));
        page_load(&pages, "main", PageLoadEvent::Finished, |_| {});
        view("main")
            .send(&frame("c1", 6))
            .expect("settled once loaded");
        assert_eq!(lock(&seen).len(), 6);
    }

    /// The same through the session manager the app wires `page_load` to: a page that starts
    /// loading ends its webview's chat view and status channel, and leaves another webview's.
    #[tokio::test]
    async fn page_load_ends_the_webviews_channels_in_the_manager() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut booted = boot(Ok(DataRoot::at(dir.path().join("data"))));
        let chats = testing::start(&mut booted).expect("chats");
        let folder = dir.path().join("project");
        std::fs::create_dir(&folder).expect("a chat folder");
        let chat = chats
            .manager
            .create_chat("mock", &folder)
            .await
            .expect("a chat");
        let pages = Pages::default();
        let ended = [(); 3].map(|()| Arc::new(AtomicUsize::new(0)));
        let [view, status, other] = &ended;
        chats
            .manager
            .attach(
                &chat.id,
                Arc::new(ChannelSink::new(
                    counted(view),
                    "main",
                    pages.for_view("main"),
                )),
            )
            .await
            .expect("attach");
        chats
            .manager
            .status_subscribe(Arc::new(StatusChannelSink::new(
                counted(status),
                "main",
                pages.for_status("main"),
            )));
        chats
            .manager
            .attach(
                &chat.id,
                Arc::new(ChannelSink::new(
                    counted(other),
                    "other",
                    pages.for_view("other"),
                )),
            )
            .await
            .expect("attach from another webview");

        assert!(page_load(&pages, "main", PageLoadEvent::Started, |owner| {
            chats.manager.drop_owner(owner);
        }));
        for _ in 0..500 {
            if view.load(Ordering::Acquire) == 1 && status.load(Ordering::Acquire) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            view.load(Ordering::Acquire),
            1,
            "the chat view's channel ended"
        );
        assert_eq!(
            status.load(Ordering::Acquire),
            1,
            "the status channel ended"
        );
        assert_eq!(
            other.load(Ordering::Acquire),
            0,
            "another webview keeps its view"
        );
        chats.manager.shutdown_all(QUIT_SESSIONS_DEADLINE).await;
        assert!(chats.store.shutdown(QUIT_STORE_DEADLINE).await);
    }

    #[test]
    fn the_sweep_notice_counts_what_it_stopped() {
        let mut report = SweepReport::default();
        assert_eq!(sweep_notice(&report), None);
        report.killed.push((5001, "hermes acp".into()));
        assert_eq!(
            sweep_notice(&report).as_deref(),
            Some(
                "Ghost stopped 1 agent process left running by an earlier session that ended unexpectedly."
            )
        );
        report.killed.push((5002, "claude-agent-acp".into()));
        assert_eq!(
            sweep_notice(&report).as_deref(),
            Some(
                "Ghost stopped 2 agent processes left running by an earlier session that ended unexpectedly."
            )
        );
    }

    #[tokio::test]
    async fn startup_moves_the_store_and_the_owner_lock_into_the_chat_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("data"));
        let mut booted = boot(Ok(root.clone()));
        let chats = testing::start(&mut booted).expect("chats start");
        assert!(
            booted.store.is_none() && booted.owner.is_none(),
            "both moved"
        );
        assert_eq!(booted.fatal, None);

        let agents = chats.manager.agents(false).await;
        assert_eq!(agents.len(), 1, "the detector given");
        assert_eq!(chats.manager.storage_status(), StorageStatus::default());
        assert!(
            chats
                .manager
                .protocol_log_dir("c1")
                .ends_with(Path::new("logs").join("protocol").join("c1"))
        );
        let second = boot(Ok(root.clone()));
        assert!(
            second
                .fatal
                .as_deref()
                .is_some_and(|fatal| fatal.starts_with("Ghost is already running")),
            "the chat store holds the profile: {:?}",
            second.fatal
        );
        drop(second);

        assert!(chats.store.shutdown(QUIT_STORE_DEADLINE).await);
        drop(chats);
        let third = boot_next(&root);
        assert_eq!(third.fatal, None, "free once the store closed");
        assert_eq!(third.integrity_check, None, "and closed cleanly");
    }

    #[tokio::test]
    async fn a_failed_startup_starts_no_chats() {
        let mut booted = boot(Err(ghost_core::CoreError::NoDataDir));
        assert!(testing::start(&mut booted).is_none());
        assert!(booted.fatal.is_some());
    }

    // ---------- the quit flow ----------

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Hide,
        Show,
        Ask(String),
        StopSessions(Duration),
        CloseStore(Duration),
        CloseRegistry,
        Exit(i32),
        OwnedWorkBlocked,
        CloseProjectTerminals,
        TerminalCleanupBlocked,
    }

    /// Records every call; the store and the sessions are real when given, and either can be
    /// made to never finish.
    struct FakeHost {
        drafts_saved: bool,
        calls: Mutex<Vec<Call>>,
        storage: StorageStatus,
        quit_anyway: bool,
        chats: Option<ChatRuntime>,
        stuck_sessions: bool,
        stuck_store: bool,
        active_work: usize,
        stuck_owned_work: bool,
        project_terminals: usize,
        terminal_cleanup: bool,
    }

    impl FakeHost {
        fn new(storage: StorageStatus, quit_anyway: bool) -> FakeHost {
            FakeHost {
                drafts_saved: true,
                calls: Mutex::new(Vec::new()),
                storage,
                quit_anyway,
                chats: None,
                stuck_sessions: false,
                stuck_store: false,
                active_work: 0,
                stuck_owned_work: false,
                project_terminals: 0,
                terminal_cleanup: true,
            }
        }

        fn calls(&self) -> Vec<Call> {
            lock(&self.calls).clone()
        }

        fn record(&self, call: Call) {
            lock(&self.calls).push(call);
        }
    }

    impl QuitHost for FakeHost {
        fn owned_work_guard(&self) -> Option<Box<dyn Send + '_>> {
            match &self.chats {
                Some(chats) => chats
                    .manager
                    .try_drain_owned_work()
                    .map(|guard| Box::new(guard) as Box<dyn Send>),
                None => Some(Box::new(())),
            }
        }
        async fn wait_owned_work(&self, deadline: Duration) -> bool {
            if self.stuck_owned_work {
                return std::future::pending().await;
            }
            match &self.chats {
                Some(chats) => chats.manager.wait_owned_work(deadline).await,
                None => true,
            }
        }
        fn owned_work_blocked(&self) {
            self.record(Call::OwnedWorkBlocked);
        }
        fn project_terminal_count(&self) -> usize {
            self.project_terminals
        }
        async fn close_project_terminals(&self, _deadline: Duration) -> bool {
            self.record(Call::CloseProjectTerminals);
            self.terminal_cleanup
        }
        fn terminal_cleanup_blocked(&self) {
            self.record(Call::TerminalCleanupBlocked);
        }
        async fn flush_drafts(&self) -> bool {
            self.drafts_saved
        }
        fn hide_window(&self) {
            self.record(Call::Hide);
        }

        fn show_window(&self) {
            self.record(Call::Show);
        }

        fn storage(&self) -> StorageStatus {
            self.storage.clone()
        }

        fn active_work_count(&self) -> usize {
            self.active_work
        }

        async fn ask_to_quit_unsaved(&self, question: QuitQuestion) -> bool {
            self.record(Call::Ask(question.message));
            self.quit_anyway
        }

        async fn stop_sessions(&self, deadline: Duration) -> ShutdownSummary {
            self.record(Call::StopSessions(deadline));
            if self.stuck_sessions {
                return std::future::pending().await;
            }
            match &self.chats {
                Some(chats) => chats.manager.shutdown_all(deadline).await,
                None => ShutdownSummary {
                    sessions: 2,
                    graceful: 1,
                    forced: 1,
                    took: Duration::from_secs(3),
                },
            }
        }

        async fn close_store(&self, deadline: Duration) -> bool {
            self.record(Call::CloseStore(deadline));
            if self.stuck_store {
                return std::future::pending().await;
            }
            match &self.chats {
                Some(chats) => chats.store.shutdown(deadline).await,
                None => true,
            }
        }

        fn close_registry(&self) {
            self.record(Call::CloseRegistry);
            if let Some(chats) = &self.chats {
                chats.registry.close().expect("close the registry");
            }
        }

        fn exit(&self, code: i32) {
            self.record(Call::Exit(code));
        }
    }

    fn paused(reason: PauseReason, unsaved_bytes: u64) -> StorageStatus {
        StorageStatus {
            state: StorageState::Paused { reason, since: 1 },
            unsaved_bytes,
        }
    }

    #[tokio::test]
    async fn quit_cancel_preserves_project_shell_and_confirmation_waits_for_cleanup() {
        for confirmed in [false, true] {
            let quit = Quit::default();
            quit.on_exit_requested();
            let mut host = FakeHost::new(StorageStatus::default(), confirmed);
            host.project_terminals = 1;
            let result = run_quit(&quit, &host, 0).await;
            let calls = host.calls();
            // The question comes first, in the window; it hides only once quitting goes ahead.
            assert!(matches!(&calls[0],Call::Ask(message) if message.contains("project terminal")));
            if confirmed {
                assert!(matches!(result, QuitOutcome::Exited { .. }));
                assert_eq!(calls[1], Call::Hide);
                assert_eq!(calls[2], Call::CloseProjectTerminals);
                assert!(matches!(calls[3], Call::StopSessions(_)));
            } else {
                assert_eq!(result, QuitOutcome::Kept);
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[1], Call::Show);
            }
        }
        let quit = Quit::default();
        quit.on_exit_requested();
        let mut host = FakeHost::new(StorageStatus::default(), true);
        host.project_terminals = 1;
        host.terminal_cleanup = false;
        assert_eq!(run_quit(&quit, &host, 0).await, QuitOutcome::Kept);
        assert!(host.calls().ends_with(&[
            Call::CloseProjectTerminals,
            Call::Show,
            Call::TerminalCleanupBlocked
        ]));
        assert!(!host.calls().iter().any(|call| matches!(
            call,
            Call::CloseStore(_) | Call::StopSessions(_) | Call::Exit(_)
        )));
    }

    #[tokio::test]
    async fn ordinary_quit_keeps_outer_transition_exclusive_without_reacquiring_it() {
        let temp = tempfile::tempdir().unwrap();
        let mut booted = boot(Ok(DataRoot::at(temp.path().join("data"))));
        let chats = testing::start(&mut booted).expect("chats");
        let transition = tokio::sync::Mutex::new(());
        let outer = transition.lock().await;
        let quit = Quit::default();
        quit.on_exit_requested();
        let mut host = FakeHost::new(StorageStatus::default(), false);
        host.chats = Some(chats);
        assert!(transition.try_lock().is_err());
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(10), run_quit(&quit, &host, 0))
                .await
                .unwrap(),
            QuitOutcome::Exited { saved: true, .. }
        ));
        assert!(transition.try_lock().is_err());
        drop(outer);
        assert!(transition.try_lock().is_ok());
    }

    #[tokio::test]
    async fn a_failed_editor_flush_keeps_the_app_open_without_stopping_agents() {
        let quit = Quit::default();
        quit.on_exit_requested();
        let mut host = FakeHost::new(StorageStatus::default(), false);
        host.drafts_saved = false;
        assert_eq!(run_quit(&quit, &host, 0).await, QuitOutcome::Kept);
        assert!(
            matches!(host.calls().as_slice(), [Call::Ask(message), Call::Show] if message.contains("unsent draft"))
        );
        assert!(!quit.is_quitting());
    }

    #[tokio::test(start_paused = true)]
    async fn quit_keeps_storage_open_when_an_owned_mutation_cannot_finish_yet() {
        let quit = Quit::default();
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        let mut host = FakeHost::new(StorageStatus::default(), true);
        host.stuck_owned_work = true;
        let started = tokio::time::Instant::now();
        assert_eq!(run_quit(&quit, &host, 0).await, QuitOutcome::Kept);
        assert_eq!(started.elapsed(), QUIT_OWNED_WORK_DEADLINE);
        assert_eq!(host.calls(), [Call::Show, Call::OwnedWorkBlocked]);
        assert!(!quit.store_closed());
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
    }

    /// The window's own dialog answers when the page shows it in time, however long the choice
    /// takes; a page that never shows it, or a question that could not be sent, falls back to the
    /// system dialog; a page that goes away mid-question keeps Ghost open.
    #[tokio::test(start_paused = true)]
    async fn the_quit_question_asks_in_the_window_and_falls_back_to_the_system_dialog() {
        use tokio::sync::oneshot;
        let within = QUIT_QUESTION_SHOWN_WITHIN;

        let (shown_tx, shown) = oneshot::channel();
        let (answer_tx, answer) = oneshot::channel();
        let asking = tokio::spawn(ask_in_app(true, shown, answer, within));
        shown_tx.send(()).unwrap();
        // Long after the show deadline the person still decides.
        tokio::time::sleep(within * 20).await;
        answer_tx.send(true).unwrap();
        assert_eq!(asking.await.unwrap(), Some(true));

        let (shown_tx, shown) = oneshot::channel();
        let (answer_tx, answer) = oneshot::channel();
        shown_tx.send(()).unwrap();
        answer_tx.send(false).unwrap();
        assert_eq!(ask_in_app(true, shown, answer, within).await, Some(false));

        // Shown, then the page is gone without a choice: keep Ghost open.
        let (shown_tx, shown) = oneshot::channel();
        let (answer_tx, answer) = oneshot::channel::<bool>();
        shown_tx.send(()).unwrap();
        drop(answer_tx);
        assert_eq!(ask_in_app(true, shown, answer, within).await, Some(false));

        // Never shown: the system dialog asks, after exactly the deadline.
        let (_shown_tx, shown) = oneshot::channel::<()>();
        let (_answer_tx, answer) = oneshot::channel::<bool>();
        let started = tokio::time::Instant::now();
        assert_eq!(ask_in_app(true, shown, answer, within).await, None);
        assert_eq!(started.elapsed(), within);

        let (_shown_tx, shown) = oneshot::channel::<()>();
        let (_answer_tx, answer) = oneshot::channel::<bool>();
        assert_eq!(
            ask_in_app(false, shown, answer, within).await,
            None,
            "not sent"
        );
    }

    /// The system dialog lists Keep first, its default: only the quit label quits.
    #[test]
    fn the_system_dialog_quits_only_on_its_quit_button() {
        use tauri_plugin_dialog::MessageDialogResult as Result;
        let quit = "Stop agents and quit";
        assert!(native_quit_choice(&Result::Custom(quit.into()), quit));
        for kept in [
            Result::Custom("Keep Ghost open".into()),
            Result::Cancel,
            Result::Ok,
            Result::Yes,
            Result::No,
        ] {
            assert!(!native_quit_choice(&kept, quit), "{kept:?}");
        }
    }

    #[tokio::test]
    async fn quit_flow() {
        let quit = Quit::default();
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        assert_eq!(
            quit.on_exit_requested(),
            ExitRequest::Wait,
            "a second request while quitting waits for the flow"
        );
        let host = FakeHost::new(StorageStatus::default(), false);
        let outcome = run_quit(&quit, &host, 1).await;
        assert!(matches!(
            outcome,
            QuitOutcome::Exited {
                summary: Some(_),
                saved: true
            }
        ));
        assert_eq!(
            host.calls(),
            [
                Call::Hide,
                Call::StopSessions(Duration::from_secs(8)),
                Call::CloseStore(Duration::from_secs(5)),
                Call::CloseRegistry,
                Call::Exit(1),
            ],
            "no question while saving runs; the exit keeps the code it was asked for (smoke runs)"
        );
        assert!(quit.store_closed() && quit.is_done());
        assert_eq!(
            quit.on_exit_requested(),
            ExitRequest::Allow,
            "the exit the flow asked for passes"
        );
    }

    #[tokio::test]
    async fn quit_with_active_work_can_be_cancelled_without_stopping_or_closing_anything() {
        let quit = Quit::default();
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        let mut host = FakeHost::new(StorageStatus::default(), false);
        host.active_work = 2;
        assert_eq!(run_quit(&quit, &host, 0).await, QuitOutcome::Kept);
        let calls = host.calls();
        assert!(
            matches!(calls.as_slice(), [Call::Ask(message), Call::Show] if message.contains("2 chats"))
        );
        assert!(!quit.is_quitting());
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
    }

    #[tokio::test]
    async fn active_work_and_paused_storage_share_one_confirmation_before_cleanup() {
        let quit = Quit::default();
        quit.on_exit_requested();
        let mut host = FakeHost::new(paused(PauseReason::DiskFull, 10), true);
        host.active_work = 1;
        assert!(matches!(
            run_quit(&quit, &host, 0).await,
            QuitOutcome::Exited { .. }
        ));
        let calls = host.calls();
        let questions: Vec<_> = calls
            .iter()
            .filter_map(|call| match call {
                Call::Ask(message) => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(questions.len(), 1);
        assert!(questions[0].contains("One chat") && questions[0].contains("disk is full"));
        assert!(matches!(calls.get(2), Some(Call::StopSessions(_))));
    }

    #[tokio::test]
    async fn a_quit_confirmed_ahead_does_not_ask_again() {
        let quit = Quit::default();
        quit.confirm_ahead();
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        let mut host = FakeHost::new(paused(PauseReason::DiskFull, 4096), false);
        host.active_work = 2;
        assert!(matches!(
            run_quit(&quit, &host, 0).await,
            QuitOutcome::Exited { .. }
        ));
        let calls = host.calls();
        assert!(
            !calls.iter().any(|call| matches!(call, Call::Ask(_))),
            "{calls:?}"
        );
        assert_eq!(calls.first(), Some(&Call::Hide));

        // Kept open for another reason, the next quit asks as usual.
        let quit = Quit::default();
        quit.confirm_ahead();
        let mut host = FakeHost::new(paused(PauseReason::DiskFull, 4096), false);
        host.project_terminals = 1;
        host.terminal_cleanup = false;
        assert_eq!(run_quit(&quit, &host, 0).await, QuitOutcome::Kept);
        let host = FakeHost::new(paused(PauseReason::DiskFull, 4096), false);
        assert_eq!(run_quit(&quit, &host, 0).await, QuitOutcome::Kept);
        assert!(matches!(host.calls().first(), Some(Call::Ask(_))));
    }

    #[tokio::test]
    async fn quit_asks_when_paused() {
        // Keep Ghost open: the window comes back, nothing stops, and quitting starts over.
        let quit = Quit::default();
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        let host = FakeHost::new(paused(PauseReason::DiskFull, 4096), false);
        assert_eq!(run_quit(&quit, &host, 0).await, QuitOutcome::Kept);
        assert_eq!(
            host.calls(),
            [
                Call::Ask(
                    "Ghost can't save its latest messages because the disk is full. If you quit now, they are lost."
                        .into()
                ),
                Call::Show,
            ]
        );
        assert!(!quit.store_closed() && !quit.is_quitting());
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);

        // Quit anyway: the same bounded flow.
        let host = FakeHost::new(paused(PauseReason::Busy, 1), true);
        assert!(matches!(
            run_quit(&quit, &host, 0).await,
            QuitOutcome::Exited { .. }
        ));
        let calls = host.calls();
        assert!(
            matches!(&calls[0], Call::Ask(message) if message.contains("database is busy")),
            "{calls:?}"
        );
        assert_eq!(calls[1], Call::Hide, "hidden only once quitting goes ahead");
        assert_eq!(
            calls[2..],
            [
                Call::StopSessions(QUIT_SESSIONS_DEADLINE),
                Call::CloseStore(QUIT_STORE_DEADLINE),
                Call::CloseRegistry,
                Call::Exit(0),
            ]
        );
    }

    /// The window is hidden and every other exit request waits for the flow, so a step that never
    /// finishes is left behind at its deadline plus the margin, and the app still exits; the
    /// store, not closed here, gets its last second at `RunEvent::Exit`.
    #[tokio::test(start_paused = true)]
    async fn quit_carries_on_past_steps_that_never_finish() {
        let quit = Quit::default();
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        let mut host = FakeHost::new(StorageStatus::default(), false);
        host.stuck_sessions = true;
        host.stuck_store = true;
        let started = tokio::time::Instant::now();
        let outcome = run_quit(&quit, &host, 3).await;
        assert_eq!(
            started.elapsed(),
            QUIT_SESSIONS_DEADLINE + QUIT_STORE_DEADLINE + 2 * QUIT_STEP_MARGIN
        );
        assert_eq!(
            outcome,
            QuitOutcome::Exited {
                summary: None,
                saved: false
            }
        );
        assert_eq!(
            host.calls(),
            [
                Call::Hide,
                Call::StopSessions(QUIT_SESSIONS_DEADLINE),
                Call::CloseStore(QUIT_STORE_DEADLINE),
                Call::CloseRegistry,
                Call::Exit(3),
            ]
        );
        assert!(quit.is_done());
        assert!(
            !quit.store_closed(),
            "RunEvent::Exit still closes the store"
        );

        // Sessions that never stop do not keep a store that closes from closing.
        let quit = Quit::default();
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        let mut host = FakeHost::new(StorageStatus::default(), false);
        host.stuck_sessions = true;
        let outcome = run_quit(&quit, &host, 0).await;
        assert_eq!(
            outcome,
            QuitOutcome::Exited {
                summary: None,
                saved: true
            }
        );
        assert!(quit.store_closed());
    }

    #[test]
    fn an_abandoned_flow_lets_the_app_exit() {
        let quit = Quit::default();
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        quit.abandon();
        assert_eq!(quit.on_exit_requested(), ExitRequest::Allow);
        assert!(
            !quit.store_closed(),
            "RunEvent::Exit still closes the store"
        );
    }

    /// The folder picker and the quit question are called from Rust: the page gets no dialog
    /// permission, so it cannot open file dialogs of its own.
    #[test]
    fn the_page_has_no_dialog_permission() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).expect("JSON");
        for entry in capability["permissions"].as_array().expect("permissions") {
            let id = entry
                .as_str()
                .or_else(|| entry["identifier"].as_str())
                .expect("an identifier");
            assert!(!id.starts_with("dialog:"), "the capability grants {id}");
        }
    }

    #[test]
    fn only_a_paused_store_with_unsaved_data_asks() {
        assert_eq!(quit_question(&StorageStatus::default()), None);
        assert_eq!(quit_question(&paused(PauseReason::DiskFull, 0)), None);
        let failed = StorageStatus {
            state: StorageState::Failed {
                message: "damaged".into(),
            },
            unsaved_bytes: 10,
        };
        assert_eq!(
            quit_question(&failed),
            None,
            "a damaged store writes to its dead-letter file"
        );
        let question = quit_question(&paused(PauseReason::DiskFull, 10)).expect("asks");
        assert_eq!(question.quit, "Quit anyway");
        assert_eq!(question.keep, "Keep Ghost open");
    }

    /// This instance's leftover registry files in `run`: `<pid>-<start>.json` (spec 3.5).
    fn own_registry_files(run: &Path) -> Vec<String> {
        let prefix = format!("{}-", std::process::id());
        std::fs::read_dir(run)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter(|name| name.starts_with(&prefix) && name.ends_with(".json"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Quitting with no live session still commits the store and closes it: the write-ahead log is
    /// folded back, so the next launch sees a clean shutdown and checks nothing; and this
    /// instance's registry file, which the leftover registry wrote at startup (Unix), is gone.
    #[tokio::test]
    async fn quit_without_sessions_commits_and_closes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("data"));
        let mut booted = boot(Ok(root.clone()));
        let db = PathBuf::from(&booted.db.as_ref().expect("db").path);
        let wal = PathBuf::from(format!("{}-wal", db.display()));
        let run = booted.data_dir.clone().expect("data root").join(RUN_DIR);
        let chats = testing::start(&mut booted).expect("chats");
        if cfg!(unix) {
            assert_eq!(
                own_registry_files(&run).len(),
                1,
                "the leftover registry wrote this instance's file"
            );
        }
        assert!(
            std::fs::metadata(&wal).is_ok_and(|meta| meta.len() > 0),
            "the session wrote to its log"
        );
        assert!(!chats.manager.has_live_sessions());

        let quit = Quit::default();
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        let mut host = FakeHost::new(StorageStatus::default(), false);
        host.chats = Some(chats);
        let outcome = run_quit(&quit, &host, 0).await;
        assert!(
            matches!(outcome, QuitOutcome::Exited { saved: true, summary: Some(summary) } if summary.sessions == 0),
            "{outcome:?}"
        );
        assert!(!wal.exists(), "the log was folded back into the database");
        assert_eq!(
            own_registry_files(&run),
            Vec::<String>::new(),
            "the registry file is removed"
        );
        let chats = host.chats.take().expect("chats");
        assert!(
            !exit_last_resort(&quit, &chats),
            "RunEvent::Exit leaves a closed store alone"
        );
        drop(chats);

        let next = boot_next(&root);
        assert_eq!(next.fatal, None);
        assert_eq!(next.db.expect("db").outcome, OpenOutcome::Opened);
        assert_eq!(next.integrity_check, None, "a clean shutdown");
    }

    /// When the app ends without the quit flow (macOS can), `RunEvent::Exit` still kills the agents
    /// and closes the store.
    #[tokio::test]
    async fn exit_without_the_flow_closes_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("data"));
        let mut booted = boot(Ok(root.clone()));
        let chats = testing::start(&mut booted).expect("chats");
        let quit = Quit::default();
        assert!(exit_last_resort(&quit, &chats));
        assert!(quit.store_closed());
        assert!(!exit_last_resort(&quit, &chats), "only once");
        drop(chats);
        let next = boot_next(&root);
        assert_eq!(next.integrity_check, None);
    }

    // ---------- signals ----------

    #[derive(Default)]
    struct FakeSignals {
        calls: Mutex<Vec<String>>,
    }

    impl SignalHost for Arc<FakeSignals> {
        fn request_exit(&self) {
            lock(&self.calls).push("request_exit".into());
        }
        fn kill_now(&self) {
            lock(&self.calls).push("kill_now".into());
        }
        fn close_store_now(&self) {
            lock(&self.calls).push("close_store_now".into());
        }
        fn exit_now(&self, code: i32) {
            lock(&self.calls).push(format!("exit_now {code}"));
        }
    }

    #[test]
    fn the_first_signal_quits_and_a_second_exits_at_once() {
        let quit = Quit::default();
        let host = Arc::new(FakeSignals::default());
        assert_eq!(on_signal(&quit, &host, 15), None);
        assert_eq!(*lock(&host.calls), ["request_exit"]);
        assert_eq!(on_signal(&quit, &host, 2), Some(130));
        assert_eq!(
            *lock(&host.calls),
            [
                "request_exit",
                "kill_now",
                "close_store_now",
                "exit_now 130"
            ]
        );
    }

    /// Keeping Ghost open at the quit question ends that flow: the next signal, however much
    /// later, starts a new one rather than exiting at once and losing the unsaved data the
    /// question protected. A second signal during the new flow still exits at once.
    #[tokio::test]
    async fn a_signal_after_keeping_auracode_open_quits_again() {
        let quit = Quit::default();
        let host = Arc::new(FakeSignals::default());
        assert_eq!(on_signal(&quit, &host, 15), None);
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        let flow = FakeHost::new(paused(PauseReason::DiskFull, 10), false);
        assert_eq!(run_quit(&quit, &flow, 0).await, QuitOutcome::Kept);

        assert_eq!(on_signal(&quit, &host, 2), None, "a new flow");
        assert_eq!(*lock(&host.calls), ["request_exit", "request_exit"]);
        assert_eq!(quit.on_exit_requested(), ExitRequest::StartQuit);
        assert_eq!(on_signal(&quit, &host, 1), Some(129));
        assert_eq!(
            *lock(&host.calls),
            [
                "request_exit",
                "request_exit",
                "kill_now",
                "close_store_now",
                "exit_now 129"
            ]
        );
    }

    /// Marks the re-executed copy of the test binary that receives real signals.
    #[cfg(unix)]
    const SIGNAL_CHILD: &str = "GHOST_DESKTOP_TEST_SIGNAL_CHILD";

    /// Real signals, in a child copy of the test binary: once the handlers are installed, SIGTERM
    /// asks for the quit flow; after the user keeps Ghost open, SIGHUP asks for it again; and
    /// SIGINT during that flow exits with 130 after killing the agents and closing the store. A
    /// child, because a handled SIGINT would stop Ctrl+C from ending the test run itself.
    #[cfg(unix)]
    #[test]
    fn signal_starts_quit_flow() {
        if std::env::var_os(SIGNAL_CHILD).is_none() {
            crate::test_support::run_child("sessions::tests::signal_starts_quit_flow", |child| {
                child.env(SIGNAL_CHILD, "1");
            });
            return;
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let quit = Arc::new(Quit::default());
            let host = Arc::new(FakeSignals::default());
            let listener = SignalListener::new().expect("install the handlers");
            let task = tokio::spawn(listener.run(Arc::clone(&quit), Arc::clone(&host)));
            let me = std::process::id().to_string();
            let send = |name: &str| {
                let status = std::process::Command::new("kill")
                    .args(["-s", name, &me])
                    .status()
                    .expect("run kill");
                assert!(status.success(), "kill -s {name}");
            };
            let wait_for = |count: usize| {
                let host = Arc::clone(&host);
                async move {
                    for _ in 0..500 {
                        if lock(&host.calls).len() >= count {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    panic!("signal not handled: {:?}", lock(&host.calls));
                }
            };
            send("TERM");
            wait_for(1).await;
            assert_eq!(*lock(&host.calls), ["request_exit"]);
            // The quit question was answered with Keep Ghost open.
            quit.keep_running();
            send("HUP");
            wait_for(2).await;
            assert_eq!(*lock(&host.calls), ["request_exit", "request_exit"]);
            send("INT");
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("the listener ends after a hard exit")
                .expect("join");
            assert_eq!(
                *lock(&host.calls),
                [
                    "request_exit",
                    "request_exit",
                    "kill_now",
                    "close_store_now",
                    "exit_now 130"
                ]
            );
        });
    }

    // ---------- picker and logs ----------

    #[test]
    fn a_cancelled_picker_returns_nothing_and_a_folder_its_path() {
        assert_eq!(picked_folder(None).expect("cancelled"), None);
        let folder = tauri_plugin_dialog::FilePath::Path(PathBuf::from("/Users/me/code/app"));
        assert_eq!(
            picked_folder(Some(folder)).expect("picked").as_deref(),
            Some("/Users/me/code/app")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_name_that_is_not_unicode_is_refused() {
        use std::os::unix::ffi::OsStringExt as _;
        let name = std::ffi::OsString::from_vec(b"/tmp/caf\xe9".to_vec());
        let error = picked_folder(Some(tauri_plugin_dialog::FilePath::Path(PathBuf::from(
            name,
        ))))
        .expect_err("refused");
        assert_eq!(error.code, AppErrorCode::DialogFailed);
        assert!(
            error.message.ends_with("its name isn't valid Unicode."),
            "{}",
            error.message
        );
    }

    #[test]
    fn the_newest_log_is_the_greatest_current_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(newest_log(dir.path()), None, "an empty folder");
        assert_eq!(newest_log(&dir.path().join("missing")), None);
        for name in [
            "019a0000-0000-7000-8000-000000000001.jsonl",
            "019a0000-0000-7000-8000-000000000002.jsonl",
            "019a0000-0000-7000-8000-000000000003.1.jsonl",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(name), b"{}").expect("write");
        }
        std::fs::create_dir(
            dir.path()
                .join("019a0000-0000-7000-8000-000000000009.jsonl"),
        )
        .expect("a folder with a log's name");
        assert_eq!(
            newest_log(dir.path()),
            Some(
                dir.path()
                    .join("019a0000-0000-7000-8000-000000000002.jsonl")
            ),
            "rotated parts, other files and folders are skipped"
        );
    }

    #[test]
    fn show_log_reveals_the_newest_log_or_says_there_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut shown = Vec::new();
        let error = reveal_newest_log(dir.path(), |path| {
            shown.push(path.to_path_buf());
            Ok::<(), String>(())
        })
        .expect_err("no log yet");
        assert_eq!(error.code, AppErrorCode::RevealFailed);
        assert!(
            error
                .message
                .starts_with("This chat has no protocol log yet")
        );
        assert!(shown.is_empty());

        let log = dir
            .path()
            .join("019a0000-0000-7000-8000-000000000001.jsonl");
        std::fs::write(&log, b"{}").expect("write");
        let revealed = reveal_newest_log(dir.path(), |path| {
            shown.push(path.to_path_buf());
            Ok::<(), String>(())
        })
        .expect("revealed");
        assert_eq!(shown, std::slice::from_ref(&log));
        assert_eq!(revealed, log);

        let error = reveal_newest_log(dir.path(), |_| Err("no file manager is running"))
            .expect_err("failed");
        assert_eq!(error.code, AppErrorCode::RevealFailed);
        assert!(
            error
                .message
                .ends_with("in the file manager: no file manager is running"),
            "{}",
            error.message
        );
    }

    #[test]
    fn only_plain_ids_name_a_log_folder() {
        assert!(is_plain_id("019a0000-0000-7000-8000-000000000001"));
        for id in ["", "..", ".", "../etc", "a/b", "/abs", "a/", "./a"] {
            assert!(!is_plain_id(id), "{id:?}");
        }
    }
}
