//! The session manager: one actor per chat, the chats' agents, their queues and their storage.
//!
//! Each chat gets an actor on its first call, kept for the app run (a stopped chat's actor is a parked
//! task). The actor owns the chat's state, its frame batcher, its subscribers, its prompt queue, the
//! open turn and the live driver, and never awaits anything inline: every wait is a future it polls
//! beside its mailbox. Drivers spawn through a per-chat spawner that records every process, so force
//! stop and quit reach them even when a driver is wedged. The desktop's commands are thin wrappers
//! over [`SessionManager`].
//!
//! A supervisor awaits each actor: if one panics, every process it recorded is killed, it is
//! removed (the next call recreates it, and loading the chat recovers its running turn), and the
//! chat reads `Stopped`.

mod activity;
mod actor;
pub use actor::TerminalOpened;
pub use composer::{Photo, prepare_photo};
mod agent_accounts;
mod agent_installations;
mod catalogs;
mod changes;
mod cloning;
mod composer;
mod config_library;
mod config_vendor;
mod conversation;
mod coordinator;
mod editors;
mod file_open;
mod folder_guards;
mod folder_locks;
mod folder_resolution;
mod gitworkspaces;
mod images;
mod installer;
mod instructions;
mod live;
mod login;
mod memory;
mod normalize;
mod plugin_library;
mod policy;
mod project_tools;
mod repository_publication;
mod search;
mod shared_mcp;
mod spawner;
mod stopping;
mod terminal_import;
#[cfg(test)]
mod tests;
mod workspace;
mod workspace_folders;
mod workspace_removal;
mod worktree_chat;
mod worktree_run;
mod worktree_setup;

pub use config_library::LibraryRoots;
pub use config_vendor::{PreparedVendor, VendorLaunch};
pub use plugin_library::{PluginFacts, PluginRefreshPause, assemble as assemble_plugin_library};
pub use project_tools::{EditorLaunch, EditorOpen, UrlTarget};
pub use stopping::{StopReport, StopScope, still_stopping};
pub use worktree_setup::WorktreeSetupLaunch;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};
use ghost_agents::Detector;
use ghost_drivers::driver::{DriverFactory, StopPlan};
use ghost_process::{
    EnvResolver, EnvSource, LogFactory, ProcessSpawner, Redactor, ResolverStatus, ShutdownPlan,
    child_env,
};
use ghost_protocol::session::{
    AgentEntry, Availability, BlobText, ChatAttach, ChatStatusUpdate, ChatSummary, ConfigValue,
    EnvState, EnvStatus, FolderState, HistoryPage, PromptAck, SessionLists, StorageState,
    StorageStatus,
};
use ghost_protocol::{Autonomy, EventBatch, Millis, SessionStatus, UiAnswer};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::chatstore::blobs::{BLOB_UNSAVED, MAX_BLOB_PAGE};
use crate::chatstore::history::clamp_page_bytes;
use crate::chatstore::{ChatRecord, ChatStore, NewChat, StoreError, WaitError, await_reply};
use crate::time::now_millis;
use actor::{ChatMsg, storage_error};
use live::LiveAgents;
use spawner::ProcessBook;

/// `create_chat` waits at most this long for the store.
const CREATE_TIMEOUT: Duration = Duration::from_secs(10);

/// `set_option` and `set_autonomy` give up after this (the actor's own 35 s plus margin).
const OPTION_TIMEOUT: Duration = Duration::from_secs(40);

/// A blob read waits this long for the chat's own `PutBlob` to reach the store.
const BLOB_WAIT: Duration = Duration::from_secs(5);

/// Permission denied, as `create_chat` reports it.
const FOLDER_NOT_ALLOWED: &str = "Ghost isn't allowed to open this folder. Allow it in System Settings > Privacy & Security > Files and Folders, then try again.";

/// An update admission barrier. Dropping it before shutdown lets the user continue normally.
/// It does not close storage, so editor drafts can still be flushed while it is held.
pub struct IdleSessionGuard<'a> {
    live: &'a LiveAgents,
}

impl Drop for IdleSessionGuard<'_> {
    fn drop(&mut self) {
        self.live.resume_admission();
    }
}

/// Keeps new agent starts, Git mutations and vendor/configuration maintenance out of a quit flow.
/// Existing owners finish normally. Cancellation reopens admission before permanent shutdown.
pub struct OwnedWorkDrain<'a> {
    live: &'a LiveAgents,
}

impl Drop for OwnedWorkDrain<'_> {
    fn drop(&mut self) {
        self.live.end_owned_drain();
    }
}

/// Everything the manager runs on. Every constructor that spawns tasks takes `rt`, because Tauri's
/// `setup` hook and run-event callbacks run outside any Tokio context.
pub struct ManagerDeps {
    pub worktrees: Option<Arc<crate::gitworkspaces::GitWorkspaces>>,
    pub store: Arc<ChatStore>,
    /// Snapshot/revert storage. Automatic capture is a separate, opt-in profile preference.
    pub checkpoints: Option<Arc<crate::checkpoints::Checkpoints>>,
    pub env: Arc<EnvResolver>,
    /// `ghost_process::Spawner`, or `FakeSpawner` in tests.
    pub spawner: Arc<dyn ProcessSpawner>,
    pub detector: Arc<Detector>,
    pub drivers: Arc<dyn DriverFactory>,
    pub logs: LogFactory,
    pub redactor: Arc<Redactor>,
    pub device_id: String,
    pub app_version: String,
    pub limits: ManagerLimits,
    pub timings: SessionTimings,
    pub rt: Handle,
}

impl fmt::Debug for ManagerDeps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagerDeps")
            .field("device_id", &self.device_id)
            .field("app_version", &self.app_version)
            .field("limits", &self.limits)
            .field("timings", &self.timings)
            .finish_non_exhaustive()
    }
}

/// Settings' defaults for new chats. Full auto unless changed; the agent's dialect still decides
/// what a level means for it, so a vendor's restrictions are never overridden.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewChatDefaults {
    pub autonomy: Autonomy,
    /// `model` and `effort` apply only to chats of this agent.
    pub agent: Option<String>,
    pub model: Option<String>,
    /// The thinking level, chosen for `model` (`None`: the agent's own default model).
    pub effort: Option<String>,
}

impl NewChatDefaults {
    /// The model a never-started chat of `agent_id` starts on, unless its project names one.
    pub(crate) fn model_for(&self, agent_id: &str) -> Option<String> {
        self.model.clone().filter(|_| self.applies_to(agent_id))
    }

    /// The thinking level a never-started chat of `agent_id` starts on when it starts on `model`:
    /// the level was chosen for the default model, so another one (a project's) leaves it out.
    pub(crate) fn effort_for(&self, agent_id: &str, model: Option<&str>) -> Option<String> {
        self.effort
            .clone()
            .filter(|_| self.applies_to(agent_id) && model == self.model.as_deref())
    }

    fn applies_to(&self, agent_id: &str) -> bool {
        self.agent
            .as_deref()
            .is_some_and(|agent| ghost_agents::agent_of(agent) == ghost_agents::agent_of(agent_id))
    }
}

/// How much the manager allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagerLimits {
    /// A start beyond this stops the agent idle longest to make room, and fails only when no
    /// agent is idle.
    pub max_live_agents: usize,
    pub max_queue: usize,
    pub max_prompt_bytes: usize,
    /// A `Ready` agent nobody views, with nothing open or queued, stops after this long.
    pub idle_eviction: Duration,
    /// The same agent with work of its own left running in the background (a build, a server, an
    /// agent it started) is not idle: stopping it ends that work. It stops only after this much
    /// longer quiet, so a server someone forgot does not hold an agent for good.
    pub background_eviction: Duration,
    pub subscribers_per_chat: usize,
    pub subscribers_per_owner: usize,
}

impl ManagerLimits {
    /// 16 live agents, 20 queued prompts, 1 MiB per prompt, 10 minutes idle (a day with work in
    /// the background), 4 views per chat, 16 per webview.
    pub const STANDARD: ManagerLimits = ManagerLimits {
        max_live_agents: 16,
        max_queue: 20,
        max_prompt_bytes: 1024 * 1024,
        idle_eviction: Duration::from_secs(10 * 60),
        background_eviction: Duration::from_secs(24 * 60 * 60),
        subscribers_per_chat: 4,
        subscribers_per_owner: 16,
    };
}

/// The manager's timers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionTimings {
    /// Frames flush this often while a chat has a subscriber...
    pub flush_live: Duration,
    /// ...and this often without one.
    pub flush_idle: Duration,
    /// A force stop waits this long for the turn's own end...
    pub force_cancel_wait: Duration,
    /// ...or this long when the agent's turns outlive its process (Hermes).
    pub daemon_cancel_wait: Duration,
    /// A Stop's cancel grace.
    pub stop_cancel_grace: Duration,
    /// Quitting's cancel grace.
    pub quit_cancel_grace: Duration,
    /// A Stop not done after this long is forced.
    pub stop_kill_after: Duration,
    /// `attach` fails after this long.
    pub attach_timeout: Duration,
    /// An agent that reports work in progress while no prompt of Ghost's is open (its Stop
    /// hooks, background tasks, a lost idle report) counts as idle for the queue after this long.
    /// Its own result-to-idle gap is milliseconds and Stop hooks take seconds, so 15 s never cuts
    /// a normal turn short, while a missing report delays the next prompt only briefly. The agent
    /// queues a prompt that reaches it while it still works.
    pub idle_grace: Duration,
}

impl SessionTimings {
    /// 16 ms, 100 ms, 200 ms, 5 s, 3 s, 2 s, 15 s, 5 s, 15 s.
    pub const STANDARD: SessionTimings = SessionTimings {
        flush_live: Duration::from_millis(16),
        flush_idle: Duration::from_millis(100),
        force_cancel_wait: Duration::from_millis(200),
        daemon_cancel_wait: Duration::from_secs(5),
        stop_cancel_grace: Duration::from_secs(3),
        quit_cancel_grace: Duration::from_secs(2),
        stop_kill_after: Duration::from_secs(15),
        attach_timeout: Duration::from_secs(5),
        idle_grace: Duration::from_secs(15),
    };
}

/// Where one open chat view's frames go (a Tauri channel in the app).
pub trait FrameSink: Send + Sync + 'static {
    /// Called on the actor task; must not block. An error removes the subscriber.
    fn send(&self, frame: &EventBatch) -> Result<(), SinkGone>;
    /// The webview label the subscription belongs to.
    fn owner(&self) -> &str;
}

/// Where one webview's chat-list updates go.
pub trait StatusSink: Send + Sync + 'static {
    fn send(&self, update: &ChatStatusUpdate) -> Result<(), SinkGone>;
    fn owner(&self) -> &str;
}

/// The sink's receiver is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkGone;

/// The manager's own limits, which tests shrink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Knobs {
    /// An actor stops reading its driver's events while this much waits in its outbox.
    pub(crate) outbox_bytes: usize,
    /// The chat list waits this long for its folder checks; a folder that has not answered by then
    /// (a hung network mount) shows what it showed last.
    pub(crate) folder_check_wait: Duration,
    /// An agent that started or ended a turn this recently still makes its folder busy for a new
    /// chat: it is likely to get the next message.
    pub(crate) busy_settle: Duration,
}

impl Knobs {
    /// The actor outbox holds 4 MiB; the chat list waits 1 s for folder checks; an agent counts as
    /// busy for 15 s after its last activity.
    pub(crate) const STANDARD: Knobs = Knobs {
        outbox_bytes: 4 * 1024 * 1024,
        folder_check_wait: Duration::from_secs(1),
        busy_settle: Duration::from_secs(15),
    };
}

/// The chat list's check of one folder: its last result, and whether one runs.
#[derive(Debug, Clone, Copy, Default)]
struct FolderCheck {
    last: Option<FolderState>,
    running: bool,
}

/// A chat's row in the list, as its actor last published it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatState {
    pub(crate) status: SessionStatus,
    pub(crate) needs_attention: bool,
    pub(crate) title: Option<String>,
    pub(crate) updated_at: Millis,
    /// What the last start found.
    pub(crate) folder_state: FolderState,
}

/// What the actors and the manager share. Actors hold it, never the manager itself, so dropping
/// the last `SessionManager` closes every mailbox and ends the actors.
pub(crate) struct Shared {
    pub(crate) deps: ManagerDeps,
    pub(crate) live: LiveAgents,
    pub(crate) knobs: Knobs,
    status_sinks: Mutex<Vec<(String, Arc<dyn StatusSink>)>>,
    chat_states: Mutex<HashMap<String, ChatState>>,
    /// Subscription id -> (chat, owner).
    subscriptions: Mutex<HashMap<String, (String, String)>>,
    shutting_down: AtomicBool,
    plugin_refresh: tokio::sync::watch::Sender<plugin_library::RefreshState>,
    /// Writes an actor still held after the store closed, so they were never saved. A quit
    /// drains every actor before the store closes, so this stays 0 unless a deadline ran out.
    pub(crate) lost_writes: AtomicU64,
    /// The chat list's folder checks, by folder.
    folders: Mutex<HashMap<PathBuf, FolderCheck>>,
    /// Folders a picked folder is being compared with, still resolving on a blocking thread: one
    /// that hangs (a dead network share) is not asked again until it answers.
    resolving: Mutex<HashSet<PathBuf>>,
    /// Orders busy-folder preflight, chat creation and moves, and Git and folder changes: per
    /// folder, so one folder's slow change never holds up another's (`folder_locks.rs`).
    organization: folder_locks::FolderLocks,
    continuations: Mutex<HashMap<String, conversation::PreparedContinuation>>,
    project_tools: Mutex<HashMap<String, project_tools::PreparedTools>>,
    removals: Mutex<HashMap<String, workspace_removal::Prepared>>,
    worktree_chats: Mutex<HashMap<String, worktree_chat::Prepared>>,
    /// Settings' defaults for chats whose agent has never started.
    pub(crate) new_chat_defaults: Mutex<NewChatDefaults>,
    /// Read once at dispatch so changing the preference never changes an in-flight capture.
    pub(crate) automatic_checkpoints: AtomicBool,
    /// Copy the source folder's Git-ignored instruction files into each new worktree. On by
    /// default, as the profile preference is.
    pub(crate) worktree_instruction_copies: AtomicBool,
    /// The profile's added agent accounts. `None` (tests, the CLI): agents use their own sign-in.
    pub(crate) agent_accounts: Mutex<Option<Arc<crate::agent_accounts::AgentAccountsStore>>>,
    /// What each agent account last reported about its usage limits.
    pub(crate) agent_usage: crate::agent_usage::UsageBook,
    /// Agents edit project memory directly through their memory grants. Every grant reads this on
    /// each request, so the profile's switch reaches running chats at once. On by default, as the
    /// profile preference is.
    pub(crate) agent_memory_writes: Arc<AtomicBool>,
    /// The Browser universal plugin is on: Chat sessions start with its tools, and every running
    /// session's tools read it on each call. On by default, as the profile preference is.
    pub(crate) browser_tools: Arc<AtomicBool>,
    /// The app's browser, which runs the tools (none in tests and the CLI).
    pub(crate) browser_bridge: Mutex<Option<Arc<dyn crate::browser_service::BrowserBridge>>>,
    /// The Mobile universal plugin is on: Chat sessions start with its tools, and every running
    /// session's tools read it on each call. On by default, as the profile preference is.
    pub(crate) mobile_tools: Arc<AtomicBool>,
    /// The app's devices, which run the tools (none in tests and the CLI).
    pub(crate) mobile_bridge: Mutex<Option<Arc<dyn crate::mobile_service::MobileBridge>>>,
    /// The Game universal plugin is on: Chat sessions in game projects start with its tools, and
    /// every running session's tools read it on each call. On by default, as the preference is.
    pub(crate) game_tools: Arc<AtomicBool>,
    /// The app's games, which run the tools (none in tests and the CLI).
    pub(crate) game_bridge: Mutex<Option<Arc<dyn crate::game_service::GameBridge>>>,
    /// Coordinated projects (an experiment): its switch, and what its engine keeps.
    pub(crate) coordination: coordinator::Coordination,
    /// The model catalog each agent connection last reported, kept across restarts, so a chat
    /// whose agent is not running still offers models and thinking levels (and Settings too).
    pub(crate) catalogs: Arc<catalogs::Catalogs>,
    /// Tests: checks of this folder take this long (a hung network mount); they are counted.
    #[cfg(test)]
    slow_folder: Mutex<Option<(PathBuf, Duration, Arc<AtomicU64>)>>,
    /// Tests: the next worktree chat record fails to save after its worktree exists.
    #[cfg(test)]
    fail_worktree_chat_save: AtomicBool,
}

impl Shared {
    pub(crate) fn agent_accounts(&self) -> Option<Arc<crate::agent_accounts::AgentAccountsStore>> {
        lock(&self.agent_accounts).clone()
    }

    pub(crate) fn browser_bridge(&self) -> Option<Arc<dyn crate::browser_service::BrowserBridge>> {
        lock(&self.browser_bridge).clone()
    }

    pub(crate) fn mobile_bridge(&self) -> Option<Arc<dyn crate::mobile_service::MobileBridge>> {
        lock(&self.mobile_bridge).clone()
    }

    pub(crate) fn game_bridge(&self) -> Option<Arc<dyn crate::game_service::GameBridge>> {
        lock(&self.game_bridge).clone()
    }

    pub(crate) fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    /// Records a chat's state and sends it to every status channel (a failed send removes it).
    pub(crate) fn publish(&self, chat_id: &str, state: ChatState) {
        let update = ChatStatusUpdate {
            chat_id: chat_id.to_owned(),
            status: state.status,
            needs_attention: state.needs_attention,
            title: state.title.clone(),
            updated_at: state.updated_at,
        };
        lock(&self.chat_states).insert(chat_id.to_owned(), state);
        lock(&self.status_sinks).retain(|(_, sink)| sink.send(&update).is_ok());
    }

    /// A subscription ended (detached, replaced, its sink gone, or its attach failed).
    pub(crate) fn forget_subscription(&self, id: &str) {
        lock(&self.subscriptions).remove(id);
    }

    fn state(&self, chat_id: &str) -> Option<ChatState> {
        lock(&self.chat_states).get(chat_id).cloned()
    }

    /// Tests: a folder set slow (a hung network mount) takes its delay here, counted.
    fn slow_disk(&self, folder: &Path) {
        #[cfg(test)]
        {
            // Release the hook lock before delaying one folder, so unrelated probes stay free.
            let slow = lock(&self.slow_folder).clone();
            if let Some((_, delay, checks)) = slow.filter(|(slow, _, _)| *slow == folder) {
                checks.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(delay);
            }
        }
        #[cfg(not(test))]
        let _ = folder;
    }

    /// Checks one folder for the chat list (`metadata` only), on a blocking thread, and records the
    /// result.
    fn check_folder(&self, folder: PathBuf) {
        self.slow_disk(&folder);
        let state = folder_state(&folder, false);
        let mut checks = lock(&self.folders);
        let check = checks.entry(folder).or_default();
        check.last = Some(state);
        check.running = false;
    }
}

/// The session manager. Cloning shares it.
#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<Inner>,
}

struct Inner {
    shared: Arc<Shared>,
    actors: Mutex<HashMap<String, ActorEntry>>,
    next_actor: AtomicU64,
}

/// One chat whose agent processes run: what the Processes page lists for it.
#[derive(Debug, Clone)]
pub struct ChatAgents {
    pub chat_id: String,
    pub processes: Vec<ghost_process::ProcessControl>,
    /// The latest record of each task its agent reported, oldest first (at most 64).
    pub tasks: Vec<ghost_protocol::tasks::BackgroundTask>,
}

/// A chat's running actor.
struct ActorEntry {
    id: u64,
    mailbox: mpsc::UnboundedSender<ChatMsg>,
    book: Arc<ProcessBook>,
}

impl fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionManager")
            .field("deps", &self.inner.shared.deps)
            .field("actors", &lock(&self.inner.actors).len())
            .finish()
    }
}

/// What a caller whose actor died reads.
fn stopped() -> SessionError {
    SessionError::Internal("the session stopped".into())
}

impl Inner {
    fn remove_actor(&self, chat_id: &str, id: u64) {
        let mut actors = lock(&self.actors);
        if actors.get(chat_id).is_some_and(|entry| entry.id == id) {
            actors.remove(chat_id);
        }
    }
}

impl SessionManager {
    pub fn new(deps: ManagerDeps) -> SessionManager {
        SessionManager::with_knobs(deps, Knobs::STANDARD)
    }

    /// [`new`](Self::new) with other actor limits (tests).
    pub(crate) fn with_knobs(deps: ManagerDeps, knobs: Knobs) -> SessionManager {
        let live = LiveAgents::new(deps.limits.max_live_agents);
        let catalogs = catalogs::Catalogs::new();
        catalogs.keep(Arc::clone(&deps.store), &deps.rt);
        SessionManager {
            inner: Arc::new(Inner {
                shared: Arc::new(Shared {
                    deps,
                    live,
                    knobs,
                    status_sinks: Mutex::new(Vec::new()),
                    chat_states: Mutex::new(HashMap::new()),
                    subscriptions: Mutex::new(HashMap::new()),
                    shutting_down: AtomicBool::new(false),
                    plugin_refresh: tokio::sync::watch::channel(Default::default()).0,
                    lost_writes: AtomicU64::new(0),
                    folders: Mutex::new(HashMap::new()),
                    resolving: Mutex::new(HashSet::new()),
                    organization: folder_locks::FolderLocks::new(),
                    continuations: Mutex::new(HashMap::new()),
                    project_tools: Mutex::new(HashMap::new()),
                    removals: Mutex::new(HashMap::new()),
                    worktree_chats: Mutex::new(HashMap::new()),
                    new_chat_defaults: Mutex::new(NewChatDefaults::default()),
                    automatic_checkpoints: AtomicBool::new(false),
                    worktree_instruction_copies: AtomicBool::new(true),
                    agent_accounts: Mutex::new(None),
                    agent_usage: crate::agent_usage::UsageBook::default(),
                    agent_memory_writes: Arc::new(AtomicBool::new(true)),
                    browser_tools: Arc::new(AtomicBool::new(true)),
                    browser_bridge: Mutex::new(None),
                    mobile_tools: Arc::new(AtomicBool::new(true)),
                    mobile_bridge: Mutex::new(None),
                    game_tools: Arc::new(AtomicBool::new(true)),
                    game_bridge: Mutex::new(None),
                    coordination: coordinator::Coordination::default(),
                    catalogs,
                    #[cfg(test)]
                    slow_folder: Mutex::new(None),
                    #[cfg(test)]
                    fail_worktree_chat_save: AtomicBool::new(false),
                }),
                actors: Mutex::new(HashMap::new()),
                next_actor: AtomicU64::new(1),
            }),
        }
    }

    fn deps(&self) -> &ManagerDeps {
        &self.inner.shared.deps
    }

    fn shared(&self) -> &Arc<Shared> {
        &self.inner.shared
    }

    /// The chat's actor, created on the first call.
    fn actor(&self, chat_id: &str) -> mpsc::UnboundedSender<ChatMsg> {
        let mut actors = lock(&self.inner.actors);
        if let Some(entry) = actors.get(chat_id)
            && !entry.mailbox.is_closed()
        {
            return entry.mailbox.clone();
        }
        let id = self.inner.next_actor.fetch_add(1, Ordering::Relaxed);
        let (mailbox, receiver) = mpsc::unbounded_channel();
        let book = Arc::new(ProcessBook::default());
        let rt = &self.deps().rt;
        let task = rt.spawn(actor::run(
            chat_id.to_owned(),
            Arc::clone(self.shared()),
            Arc::clone(&book),
            receiver,
        ));
        rt.spawn(supervise(
            Arc::downgrade(&self.inner),
            chat_id.to_owned(),
            id,
            Arc::clone(&book),
            task,
        ));
        actors.insert(
            chat_id.to_owned(),
            ActorEntry {
                id,
                mailbox: mailbox.clone(),
                book,
            },
        );
        mailbox
    }

    /// The chat's actor if it runs; never creates one.
    fn existing_actor(&self, chat_id: &str) -> Option<mpsc::UnboundedSender<ChatMsg>> {
        lock(&self.inner.actors)
            .get(chat_id)
            .map(|entry| entry.mailbox.clone())
    }

    /// Sends a message to the chat's actor and waits for its answer. An actor that closed its
    /// mailbox (a chat that failed to load) is replaced once.
    async fn ask<T>(
        &self,
        chat_id: &str,
        make: impl Fn(oneshot::Sender<Result<T, SessionError>>) -> ChatMsg,
    ) -> Result<T, SessionError> {
        // A quit asks only the actors that run when it begins to stop and drain; an actor started
        // after that would queue work the closing store never takes.
        if self.shared().is_shutting_down()
            && self
                .existing_actor(chat_id)
                .is_none_or(|mailbox| mailbox.is_closed())
        {
            return Err(SessionError::ShuttingDown);
        }
        for _ in 0..2 {
            let (reply, answer) = oneshot::channel();
            if self.actor(chat_id).send(make(reply)).is_ok() {
                return answer.await.unwrap_or_else(|_| Err(stopped()));
            }
            let mut actors = lock(&self.inner.actors);
            if actors
                .get(chat_id)
                .is_some_and(|entry| entry.mailbox.is_closed())
            {
                actors.remove(chat_id);
            }
        }
        Err(stopped())
    }

    /// Every agent with its availability. `refresh: false` answers at once from the detector's cache
    /// and never waits (`agents_list`): an agent whose first detection has not ended reads
    /// `Checking`. `refresh: true` resolves the env again, probes every agent, and waits for the
    /// results (`agents_refresh`).
    pub async fn agents(&self, refresh: bool) -> Vec<AgentEntry> {
        self.deps().detector.detect_all(refresh).await
    }

    /// An agent's Connection setting on this device (`None`: Automatic), then every agent as
    /// [`agents`](Self::agents) reports it. Applies to new sessions: a chat's saved session keeps
    /// resuming through the connection that made it.
    pub async fn set_agent_connection(
        &self,
        agent_id: &str,
        connection: Option<&str>,
    ) -> Result<Vec<AgentEntry>, SessionError> {
        let detector = &self.deps().detector;
        if detector.agent(agent_id).is_none() {
            return Err(SessionError::UnknownAgent(agent_id.to_owned()));
        }
        detector
            .set_connection(agent_id, connection)
            .await
            .map_err(SessionError::AgentUnavailable)?;
        Ok(detector.cached())
    }

    pub async fn agent_maintenance(
        &self,
        agent_id: &str,
    ) -> ghost_protocol::agents::AgentMaintenance {
        self.deps().detector.maintenance(agent_id).await
    }

    pub async fn agent_auth_status(
        &self,
        agent_id: &str,
    ) -> ghost_protocol::agents::AgentAuthStatus {
        self.deps().detector.auth_status(agent_id).await
    }

    /// Only an explicit user action invokes the vendor's updater. The detector revalidates the
    /// binary/channel, serializes aliases and verifies the resulting installation.
    pub async fn update_agent(&self, agent_id: &str) -> Result<(), SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        self.refuse_update_while_working(agent_id).await?;
        let manager = self.clone();
        let agent_id = agent_id.to_owned();
        // Closing an IPC caller must not release update admission while its vendor installer runs.
        self.deps()
            .rt
            .spawn(async move {
                let _guard = manager
                    .shared()
                    .live
                    .try_maintenance()
                    .ok_or(SessionError::ShuttingDown)?;
                if manager.shared().is_shutting_down() {
                    return Err(SessionError::ShuttingDown);
                }
                manager
                    .deps()
                    .detector
                    .update(&agent_id)
                    .await
                    .map_err(|error| SessionError::AgentUnavailable(error.to_string()))
            })
            .await
            .map_err(|_| SessionError::Internal("The vendor update task failed.".into()))?
    }

    /// What the agent's last update printed, for the person who ran it (memory only).
    pub fn agent_update_output(&self, agent_id: &str) -> Option<String> {
        self.deps().detector.update_output(agent_id)
    }

    /// An update replaces the agent's program files. Under a turn that can fail (Windows holds a
    /// running program's files) or break the turn, so an update waits for the agent's chats to
    /// finish. Every connection of the agent counts: they share one installation.
    pub(crate) async fn refuse_update_while_working(
        &self,
        agent_id: &str,
    ) -> Result<(), SessionError> {
        let provider = ghost_agents::installation::provider_namespace(agent_id);
        let records = self
            .deps()
            .store
            .chats()
            .await
            .map_err(|error| storage_error(&error))?;
        let working = records
            .iter()
            .filter(|record| {
                ghost_agents::installation::provider_namespace(&record.agent_id) == provider
            })
            .filter(|record| {
                self.shared().state(&record.id).is_some_and(|state| {
                    matches!(
                        state.status,
                        SessionStatus::Starting
                            | SessionStatus::Running
                            | SessionStatus::Cancelling
                            | SessionStatus::Stopping
                    )
                })
            })
            .count();
        if working == 0 {
            return Ok(());
        }
        let name = self
            .deps()
            .detector
            .agent(provider)
            .map_or_else(|| provider.to_owned(), |spec| spec.name.to_owned());
        Err(SessionError::AgentUnavailable(if working == 1 {
            format!("{name} has a chat working. Update it once that chat finishes.")
        } else {
            format!("{name} has {working} chats working. Update it once they finish.")
        }))
    }

    /// The env agents start with: how it was resolved, and the names the scrub removes.
    pub fn env_status(&self) -> EnvStatus {
        let env = &self.deps().env;
        let state = match env.status() {
            ResolverStatus::Resolving => EnvState::Resolving,
            ResolverStatus::Ready {
                source,
                took_ms,
                vars,
            } => EnvState::Ready {
                source: source_label(&source),
                took_ms: u32::try_from(took_ms).unwrap_or(u32::MAX),
                vars: u32::try_from(vars).unwrap_or(u32::MAX),
            },
            ResolverStatus::Fallback { reason } => EnvState::Fallback { reason },
        };
        let removed = env
            .current()
            .map(|resolved| child_env(&resolved, &[]).1.removed)
            .unwrap_or_default();
        EnvStatus { state, removed }
    }

    /// Starts a new resolution and returns the status.
    pub fn env_refresh(&self) -> EnvStatus {
        self.deps().env.refresh();
        self.env_status()
    }

    /// The person's resolved environment (their login shell's), waiting for the resolution. For
    /// reading settings only: what starts a process takes [`child_env`] of it.
    pub async fn resolved_env(&self) -> Arc<ghost_process::ResolvedEnv> {
        self.deps().env.get().await
    }

    /// Every chat for the list, the most recently updated first, with its live state and whether
    /// its folder can be used here (checked by `metadata` only, so listing never triggers a macOS
    /// privacy prompt; a start's deeper check is remembered).
    pub async fn chats(&self) -> Result<Vec<ChatSummary>, SessionError> {
        let records = self
            .deps()
            .store
            .chats()
            .await
            .map_err(|error| storage_error(&error))?;
        let found = self
            .check_folders(records.iter().map(|record| record.folder.clone()).collect())
            .await;
        Ok(records
            .into_iter()
            .map(|record| {
                let state = self.shared().state(&record.id);
                let found = found.get(&record.folder).copied();
                self.summary(record, state, found)
            })
            .collect())
    }

    /// Whether each folder exists, every one checked at once on the blocking pool, waiting at most
    /// the folder-check wait. A folder that has not answered by then (a hung network mount) reads
    /// as its last result, or not at all, and is not checked again while its check still runs.
    async fn check_folders(&self, mut folders: Vec<PathBuf>) -> HashMap<PathBuf, FolderState> {
        folders.sort();
        folders.dedup();
        let shared = self.shared();
        let mut started = Vec::new();
        {
            let mut checks = lock(&shared.folders);
            for folder in &folders {
                let check = checks.entry(folder.clone()).or_default();
                if !check.running {
                    check.running = true;
                    started.push(folder.clone());
                }
            }
        }
        let checks: FuturesUnordered<JoinHandle<()>> = started
            .into_iter()
            .map(|folder| {
                let shared = Arc::clone(shared);
                self.deps()
                    .rt
                    .spawn_blocking(move || shared.check_folder(folder))
            })
            .collect();
        let _ =
            tokio::time::timeout(shared.knobs.folder_check_wait, checks.collect::<Vec<_>>()).await;
        let checks = lock(&shared.folders);
        folders
            .into_iter()
            .filter_map(|folder| {
                let last = checks.get(&folder)?.last?;
                Some((folder, last))
            })
            .collect()
    }

    /// `found`: what the list's folder check found, `None` when it did not answer in time.
    fn summary(
        &self,
        record: ChatRecord,
        state: Option<ChatState>,
        found: Option<FolderState>,
    ) -> ChatSummary {
        let agent_name = self
            .deps()
            .detector
            .agent(&record.agent_id)
            .map_or_else(|| record.agent_id.clone(), |spec| spec.name.to_owned());
        let agent_name = ghost_agents::hosts::display_name(&agent_name, &record.agent_host);
        // The list never reads a folder (that is what a macOS privacy prompt guards), so a refusal
        // is the one the last start met; whether the folder exists is current, or what the last
        // start found when the check did not answer.
        let folder_state = match (found, state.as_ref().map(|state| state.folder_state)) {
            (Some(FolderState::Ok), Some(FolderState::NotAllowed)) => FolderState::NotAllowed,
            (Some(found), _) => found,
            (None, last) => last.unwrap_or(FolderState::Ok),
        };
        ChatSummary {
            id: record.id,
            title: state
                .as_ref()
                .and_then(|state| state.title.clone())
                .or(record.title),
            agent_id: record.agent_id,
            agent_host: record.agent_host,
            agent_name,
            folder: record.folder.to_string_lossy().into_owned(),
            folder_state,
            status: state
                .as_ref()
                .map_or(SessionStatus::Stopped, |state| state.status),
            needs_attention: state.as_ref().is_some_and(|state| state.needs_attention),
            created_at: record.created_at,
            updated_at: state.as_ref().map_or(record.updated_at, |state| {
                state.updated_at.max(record.updated_at)
            }),
        }
    }

    /// Creates a chat in `folder` and starts its agent.
    pub async fn create_chat(
        &self,
        agent_id: &str,
        folder: &Path,
    ) -> Result<ChatSummary, SessionError> {
        self.create_chat_on_host(agent_id, ghost_protocol::agents::AgentHost::Native, folder)
            .await
    }

    /// Creates a chat on the explicitly selected host. The folder is this device's local path;
    /// a qualified host bridge must translate it before the driver or Linux process receives it.
    pub async fn create_chat_on_host(
        &self,
        agent_id: &str,
        agent_host: ghost_protocol::agents::AgentHost,
        folder: &Path,
    ) -> Result<ChatSummary, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        if let ghost_process::HostAvailability::Unavailable { reason } =
            ghost_agents::hosts::execution(&agent_host).availability()
        {
            return Err(SessionError::AgentUnavailable(reason));
        }
        let detector = &self.deps().detector;
        if detector.agent(agent_id).is_none() {
            return Err(SessionError::UnknownAgent(agent_id.to_owned()));
        }
        // A chat names its agent; its first start picks the connection.
        let agent_id = ghost_agents::agent_of(agent_id);
        if let Some(entry) = detector
            .cached()
            .into_iter()
            .find(|entry| entry.id == agent_id)
        {
            match entry.availability {
                Availability::Missing { reason } | Availability::Broken { reason } => {
                    return Err(SessionError::AgentUnavailable(reason));
                }
                Availability::Checking | Availability::Ready { .. } => {}
            }
        }
        let path = folder.to_path_buf();
        let checked = tokio::task::spawn_blocking(move || check_new_folder(&path))
            .await
            .map_err(|_| SessionError::Internal("checking the folder failed".into()))?;
        checked?;
        let new = NewChat {
            agent_host,
            agent_id: agent_id.to_owned(),
            folder: folder.to_path_buf(),
            device_id: self.deps().device_id.clone(),
            now: now_millis(),
        };
        let record =
            match tokio::time::timeout(CREATE_TIMEOUT, self.deps().store.create_chat(new)).await {
                Ok(Ok(record)) => record,
                Ok(Err(error)) => return Err(storage_error(&error)),
                Err(_) => {
                    return Err(SessionError::StorageUnavailable(
                        "it did not answer in time".into(),
                    ));
                }
            };
        let chat_id = record.id.clone();
        // A full slot or a failed start shows as a card in the new chat.
        let _ = self
            .ask(&chat_id, |reply| ChatMsg::Start {
                fresh: false,
                explicit: false,
                reply,
            })
            .await;
        let state = self.shared().state(&chat_id);
        Ok(self.summary(record, state, Some(FolderState::Ok)))
    }

    /// The join protocol: flushes, fixes `next_seq`, reads the newest page behind every earlier
    /// write, and registers `sink` for every frame from `next_seq` on.
    pub async fn attach(
        &self,
        chat_id: &str,
        sink: Arc<dyn FrameSink>,
    ) -> Result<ChatAttach, SessionError> {
        let owner = sink.owner().to_owned();
        let subscription_id = crate::ids::new_id();
        {
            let mut subscriptions = lock(&self.shared().subscriptions);
            // The webview's subscription to this chat is replaced, so it does not count.
            let held = subscriptions
                .values()
                .filter(|(chat, holder)| *holder == owner && chat != chat_id)
                .count();
            if held >= self.deps().limits.subscribers_per_owner {
                return Err(SessionError::TooManySubscriptions);
            }
            subscriptions.insert(subscription_id.clone(), (chat_id.to_owned(), owner));
        }
        let limit = self.deps().timings.attach_timeout * 2;
        let attached = tokio::time::timeout(
            limit,
            self.ask(chat_id, |reply| ChatMsg::Attach {
                sink: Arc::clone(&sink),
                subscription_id: subscription_id.clone(),
                reply,
            }),
        )
        .await;
        match attached {
            Ok(Ok(attach)) => Ok(attach),
            Ok(Err(error)) => {
                self.shared().forget_subscription(&subscription_id);
                Err(error)
            }
            Err(_) => {
                self.detach(&subscription_id);
                Err(SessionError::StorageUnavailable(
                    "it did not answer in time".into(),
                ))
            }
        }
    }

    pub fn detach(&self, subscription_id: &str) {
        let removed = lock(&self.shared().subscriptions).remove(subscription_id);
        if let Some((chat_id, _)) = removed
            && let Some(mailbox) = self.existing_actor(&chat_id)
        {
            let _ = mailbox.send(ChatMsg::Detach {
                subscription_id: subscription_id.to_owned(),
            });
        }
    }

    /// Drops every subscription of one webview (its page reloaded), its status channel included.
    pub fn drop_owner(&self, owner: &str) {
        let chats: Vec<String> = {
            let mut subscriptions = lock(&self.shared().subscriptions);
            let mut chats: Vec<String> = subscriptions
                .values()
                .filter(|(_, holder)| holder == owner)
                .map(|(chat, _)| chat.clone())
                .collect();
            chats.sort();
            chats.dedup();
            subscriptions.retain(|_, (_, holder)| holder != owner);
            chats
        };
        for chat_id in chats {
            if let Some(mailbox) = self.existing_actor(&chat_id) {
                let _ = mailbox.send(ChatMsg::DropOwner {
                    owner: owner.to_owned(),
                });
            }
        }
        lock(&self.shared().status_sinks).retain(|(_, sink)| sink.owner() != owner);
    }

    pub async fn history(
        &self,
        chat_id: &str,
        before_seq: u32,
        max_bytes: Option<u32>,
    ) -> Result<HistoryPage, SessionError> {
        let max_bytes = clamp_page_bytes(max_bytes.map(|bytes| bytes as usize));
        self.ask(chat_id, |reply| ChatMsg::History {
            before_seq,
            max_bytes,
            reply,
        })
        .await
    }

    pub async fn lists(&self, chat_id: &str) -> Result<SessionLists, SessionError> {
        self.ask(chat_id, |reply| ChatMsg::Lists { reply }).await
    }

    pub async fn send(&self, chat_id: &str, text: String) -> Result<PromptAck, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        self.ask(chat_id, |reply| ChatMsg::Send {
            text: text.clone(),
            parts: vec![ghost_protocol::PromptPart::Text { text: text.clone() }],
            reply,
        })
        .await
    }

    pub async fn cancel_queued(&self, chat_id: &str, prompt_id: &str) -> Result<(), SessionError> {
        self.ask(chat_id, |reply| ChatMsg::CancelQueued {
            prompt_id: prompt_id.to_owned(),
            reply,
        })
        .await
    }

    /// The words of a queued prompt as typed, without its attachments' text or mention lines.
    pub async fn queued_text(
        &self,
        chat_id: &str,
        prompt_id: &str,
    ) -> Result<String, SessionError> {
        self.ask(chat_id, |reply| ChatMsg::QueuedText {
            prompt_id: prompt_id.to_owned(),
            reply,
        })
        .await
    }

    /// Replaces a queued prompt's words, keeping its attachments, mentions, place and hold.
    /// Answers the queue after the change.
    pub async fn edit_queued(
        &self,
        chat_id: &str,
        prompt_id: &str,
        text: String,
    ) -> Result<Vec<ghost_protocol::session::QueuedPrompt>, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        self.ask(chat_id, |reply| ChatMsg::EditQueued {
            prompt_id: prompt_id.to_owned(),
            text: text.clone(),
            reply,
        })
        .await
    }

    /// Moves a queued prompt to `index` (0 goes next). Answers the queue after the change.
    pub async fn move_queued(
        &self,
        chat_id: &str,
        prompt_id: &str,
        index: usize,
    ) -> Result<Vec<ghost_protocol::session::QueuedPrompt>, SessionError> {
        self.ask(chat_id, |reply| ChatMsg::MoveQueued {
            prompt_id: prompt_id.to_owned(),
            index,
            reply,
        })
        .await
    }

    pub async fn resume_queue(&self, chat_id: &str) -> Result<(), SessionError> {
        self.ask(chat_id, |reply| ChatMsg::ResumeQueue { reply })
            .await
    }

    /// "Send now": the queued prompt goes next, released if it was held. A running turn is
    /// interrupted first when the agent can interrupt one; an agent that only reports background
    /// work gets it at once; a stopped agent starts. The other queued prompts keep their place and
    /// hold. Answers the queue after the change.
    pub async fn send_now(
        &self,
        chat_id: &str,
        prompt_id: &str,
    ) -> Result<Vec<ghost_protocol::session::QueuedPrompt>, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        self.ask(chat_id, |reply| ChatMsg::SendNow {
            prompt_id: prompt_id.to_owned(),
            reply,
        })
        .await
    }

    pub async fn interrupt(&self, chat_id: &str) -> Result<(), SessionError> {
        self.ask(chat_id, |reply| ChatMsg::Interrupt { reply })
            .await
    }

    pub async fn force_stop(&self, chat_id: &str) -> Result<(), SessionError> {
        self.ask(chat_id, |reply| ChatMsg::ForceStop { reply })
            .await
    }

    pub async fn start(&self, chat_id: &str, fresh: bool) -> Result<(), SessionError> {
        self.ask(chat_id, |reply| ChatMsg::Start {
            fresh,
            explicit: true,
            reply,
        })
        .await
    }

    /// Opens a chat in its agent's own interface: the launch for a terminal, prepared as a start
    /// would be. While it is open, Chat does not start; `terminal_ended` gives the session back.
    pub async fn open_terminal(
        &self,
        chat_id: &str,
    ) -> Result<actor::TerminalOpened, SessionError> {
        self.ask(chat_id, |reply| ChatMsg::OpenTerminal { reply })
            .await
    }

    /// The terminal `open_terminal` answered ended (it exited, or its page closed it).
    pub fn terminal_ended(&self, chat_id: &str, generation: u64) {
        if let Some(mailbox) = self.existing_actor(chat_id) {
            let _ = mailbox.send(ChatMsg::TerminalEnded { generation });
        }
    }

    pub async fn stop(&self, chat_id: &str) -> Result<(), SessionError> {
        self.ask(chat_id, |reply| ChatMsg::Stop { reply }).await
    }

    /// `option_id: None` cancels the request.
    pub async fn respond_permission(
        &self,
        chat_id: &str,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), SessionError> {
        self.ask(chat_id, |reply| ChatMsg::RespondPermission {
            request_id: request_id.to_owned(),
            option_id: option_id.clone(),
            plan: None,
            note: None,
            reply,
        })
        .await
    }

    /// Accepts a plan waiting for approval as the person edited it: the agent is told `plan`
    /// is the approved plan. Refused unless the request is a plan approval, `option_id` is its
    /// allow-once answer and the agent takes edited plans.
    pub async fn accept_plan(
        &self,
        chat_id: &str,
        request_id: &str,
        option_id: &str,
        plan: &str,
    ) -> Result<(), SessionError> {
        self.ask(chat_id, |reply| ChatMsg::RespondPermission {
            request_id: request_id.to_owned(),
            option_id: Some(option_id.to_owned()),
            plan: Some(plan.to_owned()),
            note: None,
            reply,
        })
        .await
    }

    /// Declines a plan waiting for approval with the person's words: the agent reads `note`
    /// with the refusal, in the same turn, and keeps planning. Refused unless the request is a
    /// plan approval, `option_id` is its reject-once answer and the agent takes words with a
    /// refusal.
    pub async fn decline_plan(
        &self,
        chat_id: &str,
        request_id: &str,
        option_id: &str,
        note: &str,
    ) -> Result<(), SessionError> {
        self.ask(chat_id, |reply| ChatMsg::RespondPermission {
            request_id: request_id.to_owned(),
            option_id: Some(option_id.to_owned()),
            plan: None,
            note: Some(note.to_owned()),
            reply,
        })
        .await
    }

    /// `answer: None` dismisses the question.
    pub async fn respond_ui(
        &self,
        chat_id: &str,
        request_id: &str,
        answer: Option<UiAnswer>,
    ) -> Result<(), SessionError> {
        self.ask(chat_id, |reply| ChatMsg::RespondUi {
            request_id: request_id.to_owned(),
            answer: answer.clone(),
            reply,
        })
        .await
    }

    pub async fn set_control(
        &self,
        chat_id: &str,
        change: ghost_protocol::session::SessionControlChange,
    ) -> Result<SessionLists, SessionError> {
        let asked = self.ask(chat_id, |reply| ChatMsg::SetControl {
            change: change.clone(),
            reply,
        });
        tokio::time::timeout(OPTION_TIMEOUT, asked)
            .await
            .unwrap_or(Err(SessionError::SessionNotReady))
    }

    pub async fn set_option(
        &self,
        chat_id: &str,
        option_id: &str,
        value: ConfigValue,
    ) -> Result<SessionLists, SessionError> {
        let asked = self.ask(chat_id, |reply| ChatMsg::SetOption {
            option_id: option_id.to_owned(),
            value: value.clone(),
            reply,
        });
        tokio::time::timeout(OPTION_TIMEOUT, asked)
            .await
            .unwrap_or(Err(SessionError::SessionNotReady))
    }

    pub async fn set_autonomy(
        &self,
        chat_id: &str,
        autonomy: Autonomy,
    ) -> Result<SessionLists, SessionError> {
        let asked = self.ask(chat_id, |reply| ChatMsg::SetAutonomy { autonomy, reply });
        tokio::time::timeout(OPTION_TIMEOUT, asked)
            .await
            .unwrap_or_else(|_| {
                Err(SessionError::Internal(
                    "the agent did not answer in time".into(),
                ))
            })
    }

    /// One page of a blob's text (at most 256 KiB), ending on a character boundary. The read follows
    /// the chat's own `PutBlob` for it, so a blob a frame already named is never read too early.
    pub async fn blob(
        &self,
        chat_id: &str,
        sha256: &str,
        offset: u64,
        len: u32,
    ) -> Result<BlobText, SessionError> {
        let barrier = self
            .ask(chat_id, |reply| ChatMsg::Blob {
                sha256: sha256.to_owned(),
                reply,
            })
            .await?;
        if let Some(barrier) = barrier {
            self.blob_saved(barrier).await?;
        }
        self.deps()
            .store
            .read_blob(sha256, offset, len.min(MAX_BLOB_PAGE))
            .await
            .map_err(|error| storage_error(&error))
    }

    /// Waits until the chat's `PutBlob` for a blob reached the store (the barrier behind it was
    /// answered); saving that pauses meanwhile means the output isn't saved yet.
    async fn blob_saved(&self, barrier: oneshot::Receiver<()>) -> Result<(), SessionError> {
        let store = &self.deps().store;
        let unsaved = || SessionError::BlobMissing(BLOB_UNSAVED.to_owned());
        let mut status = store.status();
        let paused = status.wait_for(|status| matches!(status.state, StorageState::Paused { .. }));
        tokio::select! {
            saved = await_reply(&self.deps().rt, barrier, BLOB_WAIT) => match saved {
                Ok(()) => Ok(()),
                Err(_) if store.is_paused() => Err(unsaved()),
                Err(WaitError::TimedOut) => Err(SessionError::StorageUnavailable(
                    "it did not answer in time".into(),
                )),
                Err(WaitError::Gone) => Err(SessionError::StorageUnavailable(
                    StoreError::Closed.to_string(),
                )),
            },
            Ok(_) = paused => Err(unsaved()),
        }
    }

    /// The folder of the chat's protocol logs, the one "Show log" reveals.
    pub fn protocol_log_dir(&self, chat_id: &str) -> PathBuf {
        self.deps().logs.chat_dir(chat_id)
    }

    /// Explicit, bounded diagnostics for a saved chat in this profile only.
    pub async fn debug_bundle(&self, chat_id: &str) -> Result<String, SessionError> {
        if !self.chats().await?.iter().any(|chat| chat.id == chat_id) {
            return Err(SessionError::ChatNotFound(chat_id.to_owned()));
        }
        self.deps().logs.debug_bundle(chat_id).await.map_err(|_| {
            SessionError::StorageUnavailable(
                "The protocol log could not be read safely. Check its local permissions.".into(),
            )
        })
    }

    /// What a chat whose agent has never started begins with (Settings' General page). A chat's
    /// own saved session, and a project's default model, take precedence.
    pub fn set_new_chat_defaults(&self, defaults: NewChatDefaults) {
        *lock(&self.shared().new_chat_defaults) = defaults;
    }

    /// Applies the active profile's opt-in to subsequent prompts, including existing chats.
    /// Existing captures finish with the policy they started under; saved snapshots stay usable.
    pub fn set_automatic_checkpoints(&self, enabled: bool) {
        self.shared()
            .automatic_checkpoints
            .store(enabled, Ordering::Release);
    }

    pub fn automatic_checkpoints(&self) -> bool {
        self.shared().automatic_checkpoints.load(Ordering::Acquire)
    }

    /// Applies the active profile's choice to worktrees created from now on.
    pub fn set_worktree_instruction_copies(&self, enabled: bool) {
        self.shared()
            .worktree_instruction_copies
            .store(enabled, Ordering::Release);
    }

    pub fn worktree_instruction_copies(&self) -> bool {
        self.shared()
            .worktree_instruction_copies
            .load(Ordering::Acquire)
    }

    /// Applies the active profile's choice to every chat's memory tools at once, running chats
    /// included. Off: agents can still read memory, and cannot change it.
    pub fn set_agent_memory_writes(&self, enabled: bool) {
        self.shared()
            .agent_memory_writes
            .store(enabled, Ordering::Release);
    }

    pub fn agent_memory_writes(&self) -> bool {
        self.shared().agent_memory_writes.load(Ordering::Acquire)
    }

    /// The Browser plugin's switch. Sessions started while it is on get the browser tools; a
    /// running session's tools answer that it is off once it is.
    pub fn set_browser_tools(&self, enabled: bool) {
        self.shared()
            .browser_tools
            .store(enabled, Ordering::Release);
    }

    pub fn browser_tools(&self) -> bool {
        self.shared().browser_tools.load(Ordering::Acquire)
    }

    /// The app's browser, which runs the tools Chat sessions get.
    pub fn set_browser_bridge(
        &self,
        bridge: Option<Arc<dyn crate::browser_service::BrowserBridge>>,
    ) {
        *lock(&self.shared().browser_bridge) = bridge;
    }

    /// The Mobile plugin's switch. Sessions started while it is on get the device tools; a
    /// running session's tools answer that it is off once it is.
    pub fn set_mobile_tools(&self, enabled: bool) {
        self.shared().mobile_tools.store(enabled, Ordering::Release);
    }

    pub fn mobile_tools(&self) -> bool {
        self.shared().mobile_tools.load(Ordering::Acquire)
    }

    /// The app's devices, which run the tools Chat sessions get.
    pub fn set_mobile_bridge(&self, bridge: Option<Arc<dyn crate::mobile_service::MobileBridge>>) {
        *lock(&self.shared().mobile_bridge) = bridge;
    }

    /// The Game plugin's switch. Sessions started in a game project while it is on get the game
    /// tools; a running session's tools answer that it is off once it is.
    pub fn set_game_tools(&self, enabled: bool) {
        self.shared().game_tools.store(enabled, Ordering::Release);
    }

    pub fn game_tools(&self) -> bool {
        self.shared().game_tools.load(Ordering::Acquire)
    }

    /// The app's games, which run the tools Chat sessions in game projects get.
    pub fn set_game_bridge(&self, bridge: Option<Arc<dyn crate::game_service::GameBridge>>) {
        *lock(&self.shared().game_bridge) = bridge;
    }

    /// The folder `chat_id` works in on this device: the one its agent starts in, and so the one
    /// its game is run from.
    pub async fn chat_folder(&self, chat_id: &str) -> Result<PathBuf, SessionError> {
        self.composer_folder(chat_id).await
    }

    /// The model choices `agent_id` reported the last time one of its chats ran, on this device:
    /// its current connection's, else its connection's that ran last. Empty until one has run,
    /// since agents list their models only from a running session.
    pub fn reported_models(&self, agent_id: &str) -> Vec<ghost_protocol::session::ConfigChoice> {
        self.agent_catalog(agent_id)
            .map(|catalog| catalog.models().to_vec())
            .unwrap_or_default()
    }

    /// The thinking levels `model` of `agent_id` offered the last time it ran (`None`: the model
    /// the agent starts on). Empty when that is not known, or when the model offers none.
    pub fn reported_thinking(
        &self,
        agent_id: &str,
        model: Option<&str>,
    ) -> Vec<ghost_protocol::session::ConfigChoice> {
        let Some(catalog) = self.agent_catalog(agent_id) else {
            return Vec::new();
        };
        let Some(model) = model.or_else(|| catalog.default_model()) else {
            return Vec::new();
        };
        match catalog.effort(model).flatten().map(|option| &option.kind) {
            Some(ghost_protocol::session::ConfigKind::Select { choices, .. }) => choices.clone(),
            _ => Vec::new(),
        }
    }

    fn agent_catalog(&self, agent_id: &str) -> Option<catalogs::Catalog> {
        let agent = ghost_agents::agent_of(agent_id);
        let connection = self.deps().detector.connection_for(agent);
        self.shared()
            .catalogs
            .for_agent(agent, connection.as_ref().map(|spec| spec.id))
    }

    /// Registers the webview's status channel (replacing its earlier one); returns its id.
    pub fn status_subscribe(&self, sink: Arc<dyn StatusSink>) -> String {
        let id = crate::ids::new_id();
        let mut sinks = lock(&self.shared().status_sinks);
        let owner = sink.owner().to_owned();
        sinks.retain(|(_, existing)| existing.owner() != owner);
        sinks.push((id.clone(), sink));
        id
    }

    pub fn storage_status(&self) -> StorageStatus {
        self.deps().store.status().borrow().clone()
    }

    /// Whether any chat has a live agent (starting, running, or stopping).
    pub fn has_live_sessions(&self) -> bool {
        self.shared().live.count() > 0
            || lock(&self.inner.actors)
                .values()
                .any(|entry| !entry.book.is_empty())
    }

    /// Refuses live/starting/stopping agents and owned folder mutations, then atomically blocks
    /// their admission. Parked queued prompts remain persisted for the next app launch.
    pub fn try_quiesce_when_idle(&self) -> Option<IdleSessionGuard<'_>> {
        if self.shared().is_shutting_down() || !self.shared().live.try_quiesce() {
            return None;
        }
        Some(IdleSessionGuard {
            live: &self.shared().live,
        })
    }

    /// A normal quit must acquire this gate before waiting, then retain it through store closure.
    pub fn try_drain_owned_work(&self) -> Option<OwnedWorkDrain<'_>> {
        self.shared()
            .live
            .begin_owned_drain()
            .then(|| OwnedWorkDrain {
                live: &self.shared().live,
            })
    }

    /// Never cancels a Git mutation or vendor updater. The caller keeps the app open on timeout.
    pub async fn wait_owned_work(&self, deadline: Duration) -> bool {
        tokio::time::timeout(deadline, async {
            while self.shared().live.owned_work_count() != 0 {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .is_ok()
    }

    /// General shells require explicit quit confirmation instead of the protected mutation drain.
    pub fn project_terminal_count(&self) -> usize {
        self.shared().live.project_terminal_count()
    }

    pub async fn wait_project_terminals(&self, deadline: Duration) -> bool {
        tokio::time::timeout(deadline, async {
            while self.project_terminal_count() != 0 {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .is_ok()
    }

    /// Stops every live agent gracefully within `deadline`, then kills what is left. No new starts
    /// after this; every preparation is dropped.
    pub async fn shutdown_all(&self, deadline: Duration) -> ShutdownSummary {
        let started = Instant::now();
        self.shared().shutting_down.store(true, Ordering::Release);
        self.shared()
            .plugin_refresh
            .send_modify(|state| state.generation += 1);
        let mailboxes: Vec<mpsc::UnboundedSender<ChatMsg>> = lock(&self.inner.actors)
            .values()
            .map(|entry| entry.mailbox.clone())
            .collect();
        let plan = StopPlan {
            cancel_grace: self.deps().timings.quit_cancel_grace,
            process: ShutdownPlan::QUIT,
        };
        let mut replies = FuturesUnordered::new();
        for mailbox in mailboxes {
            let (reply, answer) = oneshot::channel();
            if mailbox.send(ChatMsg::Shutdown { plan, reply }).is_ok() {
                replies.push(answer);
            }
        }
        let sessions = replies.len();
        let mut graceful = 0;
        let _ = tokio::time::timeout(deadline, async {
            while let Some(answer) = replies.next().await {
                if answer.is_ok() {
                    graceful += 1;
                }
            }
        })
        .await;
        let forced = sessions - graceful;
        if forced > 0 {
            tracing::warn!(
                forced,
                "some chats did not stop in time; their processes are killed"
            );
            self.kill_all_now();
        }
        // The agents' model lists learned since the last save, before the store closes.
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            self.shared().catalogs.save_now(&self.deps().store),
        )
        .await;
        ShutdownSummary {
            sessions,
            graceful,
            forced,
            took: started.elapsed(),
        }
    }

    /// How many writes actors still held when the store had closed (each was logged by name).
    pub fn lost_writes(&self) -> u64 {
        self.shared().lost_writes.load(Ordering::Acquire)
    }

    /// Every chat whose agent processes still run, for the Processes page, by chat id: its
    /// processes in the order its drivers started them, and the tasks its agent reported.
    /// `ghost_process::tree` lists what each process started.
    pub fn agent_processes(&self) -> Vec<ChatAgents> {
        let books: Vec<(String, Arc<ProcessBook>)> = lock(&self.inner.actors)
            .iter()
            .map(|(chat_id, entry)| (chat_id.clone(), Arc::clone(&entry.book)))
            .collect();
        let mut chats: Vec<ChatAgents> = books
            .into_iter()
            .filter_map(|(chat_id, book)| {
                let processes: Vec<_> = book
                    .all()
                    .into_iter()
                    .filter(|control| control.try_exited().is_none())
                    .collect();
                (!processes.is_empty()).then(|| ChatAgents {
                    chat_id,
                    processes,
                    tasks: book.tasks(),
                })
            })
            .collect();
        chats.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
        chats
    }

    /// `kill_tree()` on every recorded process of every chat; synchronous, callable from any
    /// thread, never panics.
    pub fn kill_all_now(&self) {
        let books: Vec<Arc<ProcessBook>> = lock(&self.inner.actors)
            .values()
            .map(|entry| Arc::clone(&entry.book))
            .collect();
        for book in books {
            book.kill_all();
        }
    }

    /// Tests: from now on, checks of `folder` for the chat list take `delay`; returns their count.
    #[cfg(test)]
    pub(crate) fn slow_folder(&self, folder: &Path, delay: Duration) -> Arc<AtomicU64> {
        let checks = Arc::new(AtomicU64::new(0));
        *lock(&self.shared().slow_folder) =
            Some((folder.to_path_buf(), delay, Arc::clone(&checks)));
        checks
    }

    /// Tests: makes the chat's actor panic.
    #[cfg(test)]
    pub(crate) fn panic_actor(&self, chat_id: &str) {
        if let Some(mailbox) = self.existing_actor(chat_id) {
            let _ = mailbox.send(ChatMsg::Panic);
        }
    }
}

/// Awaits a chat's actor. A panic kills every process it recorded and removes it, so the next call
/// recreates it (loading the chat recovers its running turn).
async fn supervise(
    inner: Weak<Inner>,
    chat_id: String,
    id: u64,
    book: Arc<ProcessBook>,
    task: JoinHandle<()>,
) {
    let result = task.await;
    let panicked = matches!(&result, Err(error) if error.is_panic());
    if panicked {
        tracing::error!(
            chat_id,
            "a chat actor panicked; its processes were killed, and the next call starts it again"
        );
        book.kill_all();
    }
    let Some(inner) = inner.upgrade() else {
        return;
    };
    inner.remove_actor(&chat_id, id);
    if panicked {
        let shared = &inner.shared;
        shared.live.release(&chat_id);
        lock(&shared.subscriptions).retain(|_, (chat, _)| *chat != chat_id);
        let state = shared.state(&chat_id).map_or(
            ChatState {
                status: SessionStatus::Stopped,
                needs_attention: false,
                title: None,
                updated_at: now_millis(),
                folder_state: FolderState::Ok,
            },
            |state| ChatState {
                status: SessionStatus::Stopped,
                needs_attention: false,
                ..state
            },
        );
        shared.publish(&chat_id, state);
    }
}

/// Whether `folder` can be used: `Missing` when it is gone or not a folder, `NotAllowed` when the
/// OS refuses (macOS privacy, no read permission). `read` also lists one entry, which is what a
/// privacy refusal blocks.
pub(crate) fn folder_state(folder: &Path, read: bool) -> FolderState {
    match std::fs::metadata(folder) {
        Ok(meta) if meta.is_dir() => {
            if !read {
                return FolderState::Ok;
            }
            match std::fs::read_dir(folder) {
                Ok(mut entries) => match entries.next() {
                    Some(Err(error)) if error.kind() == io::ErrorKind::PermissionDenied => {
                        FolderState::NotAllowed
                    }
                    _ => FolderState::Ok,
                },
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    FolderState::NotAllowed
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => FolderState::Missing,
                Err(_) => FolderState::NotAllowed,
            }
        }
        Ok(_) => FolderState::Missing,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => FolderState::NotAllowed,
        Err(_) => FolderState::Missing,
    }
}

/// `create_chat`'s folder rules: absolute, valid Unicode (the ACP `cwd` is a JSON string), existing,
/// a folder, readable.
fn check_new_folder(folder: &Path) -> Result<(), SessionError> {
    let invalid = |reason: &str| SessionError::InvalidFolder {
        path: folder.display().to_string(),
        reason: reason.to_owned(),
    };
    if !folder.is_absolute() {
        return Err(invalid("it isn't an absolute path."));
    }
    if folder.to_str().is_none() {
        return Err(invalid("its name isn't valid Unicode."));
    }
    match std::fs::metadata(folder) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(invalid("it doesn't exist.")),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            Err(invalid(FOLDER_NOT_ALLOWED))
        }
        Err(error) => Err(invalid(&format!("{error}."))),
        Ok(meta) if !meta.is_dir() => Err(invalid("it isn't a folder.")),
        Ok(_) => match std::fs::read_dir(folder) {
            Ok(mut entries) => match entries.next() {
                Some(Err(error)) if error.kind() == io::ErrorKind::PermissionDenied => {
                    Err(invalid(FOLDER_NOT_ALLOWED))
                }
                _ => Ok(()),
            },
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                Err(invalid(FOLDER_NOT_ALLOWED))
            }
            Err(error) => Err(invalid(&format!("{error}."))),
        },
    }
}

/// Locks `mutex`, recovering the data of a thread that panicked while holding it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How the env was resolved, for the banner.
fn source_label(source: &EnvSource) -> String {
    match source {
        EnvSource::LoginShell {
            shell, interactive, ..
        } => {
            let name = shell.file_name().map_or_else(
                || shell.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            if *interactive {
                format!("{name} login shell")
            } else {
                format!("{name} login shell (non-interactive)")
            }
        }
        EnvSource::WindowsRegistry => "Windows registry".into(),
        EnvSource::Fallback { .. } => "fallback".into(),
    }
}

/// What [`SessionManager::shutdown_all`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownSummary {
    pub sessions: usize,
    pub graceful: usize,
    pub forced: usize,
    pub took: Duration,
}

/// Why a session call failed. Each variant maps to one IPC error code; the message is the sentence
/// the UI shows.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("{0}")]
    Organization(String),
    /// Chats or project terminals are in the way of a change to a folder; `blockers` names
    /// them, so the person can stop them and try again.
    #[error("{message}")]
    FolderBusy {
        message: String,
        blockers: ghost_protocol::workspace::FolderBlockers,
    },
    #[error("There is no chat with id {0}.")]
    ChatNotFound(String),
    #[error("Ghost doesn't know an agent called \"{0}\".")]
    UnknownAgent(String),
    #[error("{0}")]
    AgentUnavailable(String),
    #[error("{path} can't be used as the chat's folder: {reason}")]
    InvalidFolder { path: String, reason: String },
    #[error("The message is empty.")]
    PromptEmpty,
    #[error("The message is too long ({bytes} bytes; the limit is {limit}).")]
    PromptTooLarge { bytes: usize, limit: usize },
    #[error("{0} messages are already waiting in this chat. Remove one or wait for one to start.")]
    QueueFull(usize),
    #[error("This request is no longer waiting for an answer.")]
    RequestNotPending,
    #[error("{0}")]
    OptionInvalid(String),
    #[error("The agent isn't running in this chat. Start it, then try again.")]
    SessionNotReady,
    /// Every live-agent slot is taken and no agent is idle to make room; the message says what
    /// the running agents are doing.
    #[error("{message}")]
    TooManyAgents { limit: usize, message: String },
    #[error("Too many views are open for this chat.")]
    TooManySubscriptions,
    #[error("Ghost's database is not available: {0}")]
    StorageUnavailable(String),
    /// The chat store's sentence ([`StoreError::BlobMissing`](crate::chatstore::StoreError::BlobMissing)),
    /// passed on as is: the output is no longer kept, or not saved yet while the disk is full.
    #[error("{0}")]
    BlobMissing(String),
    #[error("Ghost is shutting down.")]
    ShuttingDown,
    #[error("Something went wrong inside Ghost: {0}")]
    Internal(String),
}
