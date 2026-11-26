//! What the session manager's integration tests share: a manager over a temporary profile with
//! scripted drivers, fake processes and fixed agents, and sinks that record what they receive.
//!
//! Every test binary uses a different part of this module, so unused items are allowed here.
#![cfg(test)]
#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use ghost_agents::{AgentSpec, Detector, FixedAgent};
use ghost_core::chatstore::ChatStore;
use ghost_core::owner::ProfileOwner;
use ghost_core::paths::DataRoot;
use ghost_core::profile::{Profile, Profiles};
use ghost_core::session::{
    FrameSink, ManagerDeps, ManagerLimits, SessionManager, SessionTimings, SinkGone, StatusSink,
};
use ghost_core::store::Store;
use ghost_drivers::driver::{
    AgentDriver, DriverContext, DriverError, DriverEvent, DriverExit, DriverFactory, ReadyInfo,
    ResumeOutcome, SessionSetup,
};
use ghost_drivers::testing::{ScriptHandle, ScriptedDriver, Step};
use ghost_process::testing::{FakeAgentEnd, FakeSpawner};
use ghost_process::{
    EnvKey, EnvMap, EnvResolver, EnvSource, LaunchSpec, LogFactory, ProcessCwd, Redactor,
    ResolvedEnv,
};
use ghost_protocol::session::{
    Availability, ChatStatusUpdate, ChatSummary, HistoryItem, QueuedPrompt,
};
use ghost_protocol::{
    Capabilities, EventBatch, GhostEvent, PermissionOption, PermissionOptionKind,
    PermissionRequest, SessionStatus, ToolKind,
};
use ghost_protocol::{DriverKind, ErrorKind};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

/// The demo agent's id.
pub const MOCK: &str = "mock";

/// How long a test waits for something before it fails (virtual time on paused clocks).
pub const PATIENCE: Duration = Duration::from_secs(120);

pub fn mock_spec() -> AgentSpec {
    ghost_agents::get(MOCK).expect("the demo agent").clone()
}

pub fn ready() -> Availability {
    Availability::Ready {
        version: Some("1.0.0".into()),
        path: "built in".into(),
    }
}

/// The demo agent, ready, with no process.
pub fn mock_agent() -> FixedAgent {
    FixedAgent::new(mock_spec(), ready(), None)
}

/// A launch spec for an agent with a process (the scripted driver spawns it on a fake spawner).
pub fn launch() -> LaunchSpec {
    LaunchSpec {
        label: "fake agent".into(),
        program: PathBuf::from("/bin/fake-agent"),
        args: Vec::new(),
        cwd: ProcessCwd::ChatFolder,
        set_env: Vec::new(),
        daemons: Vec::new(),
        startup_timeout: Duration::from_secs(60),
    }
}

/// What a test configures beyond its drivers.
pub struct Options {
    pub agents: Vec<FixedAgent>,
    pub limits: ManagerLimits,
    pub timings: SessionTimings,
    pub env: EnvMap,
}

impl Default for Options {
    fn default() -> Options {
        let mut env = EnvMap::new();
        env.insert(EnvKey::from("PATH"), "/usr/bin".into());
        Options {
            agents: vec![mock_agent()],
            limits: ManagerLimits::STANDARD,
            timings: SessionTimings::STANDARD,
            env,
        }
    }
}

/// A session manager over a temporary profile.
pub struct Harness {
    pub dir: tempfile::TempDir,
    pub profile: Profile,
    /// A real folder for chats.
    pub folder: PathBuf,
    pub store: Arc<ChatStore>,
    pub manager: SessionManager,
    pub spawner: Arc<FakeSpawner>,
    pub ends: mpsc::UnboundedReceiver<FakeAgentEnd>,
    pub factory: Arc<Factory>,
    pub detector: Arc<Detector>,
    pub redactor: Arc<Redactor>,
    pub statuses: Arc<Statuses>,
}

impl Harness {
    pub fn new(drivers: Vec<ScriptedDriver>) -> Harness {
        Harness::with(drivers, Options::default())
    }

    pub fn with(drivers: Vec<ScriptedDriver>, options: Options) -> Harness {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = Profiles::new(DataRoot::at(dir.path().join("data")))
            .open_active()
            .expect("profile");
        let folder = dir.path().join("work");
        std::fs::create_dir_all(&folder).expect("chat folder");
        let owner = ProfileOwner::acquire(&profile).expect("owner");
        let (store, _) = Store::open(&profile).expect("store");
        let rt = Handle::current();
        let store = ChatStore::start(store, owner, &profile, "device".into(), rt.clone())
            .expect("chat store");
        Harness::over(dir, profile, folder, store, drivers, options)
    }

    /// A manager over an existing store (a restart of the same profile).
    pub fn over(
        dir: tempfile::TempDir,
        profile: Profile,
        folder: PathBuf,
        store: Arc<ChatStore>,
        drivers: Vec<ScriptedDriver>,
        options: Options,
    ) -> Harness {
        let rt = Handle::current();
        let env = EnvResolver::fixed(ResolvedEnv {
            vars: options.env,
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        });
        let detector = Detector::fixed(options.agents);
        let (spawner, ends) = FakeSpawner::new();
        let factory = Factory::new(drivers);
        let redactor = Arc::new(Redactor::standard());
        let logs = LogFactory::new(
            profile.logs_dir().join("protocol"),
            Arc::clone(&redactor),
            rt.clone(),
        );
        let manager = SessionManager::new(ManagerDeps {
            worktrees: None,
            checkpoints: None,
            store: Arc::clone(&store),
            env,
            spawner: spawner.clone(),
            detector: Arc::clone(&detector),
            drivers: factory.clone(),
            logs,
            redactor: Arc::clone(&redactor),
            device_id: "device".into(),
            app_version: "0.1.0".into(),
            limits: options.limits,
            timings: options.timings,
            rt,
        });
        let statuses = Statuses::new("main");
        manager.status_subscribe(statuses.clone());
        Harness {
            dir,
            profile,
            folder,
            store,
            manager,
            spawner,
            ends,
            factory,
            detector,
            redactor,
            statuses,
        }
    }

    /// Quits the way the app does (sessions, then the store) and starts again on the same profile.
    pub async fn restart(self, drivers: Vec<ScriptedDriver>, options: Options) -> Harness {
        let Harness {
            dir,
            profile,
            folder,
            store,
            manager,
            ..
        } = self;
        manager.shutdown_all(Duration::from_secs(8)).await;
        assert!(
            store.shutdown(Duration::from_secs(5)).await,
            "everything was saved"
        );
        drop(manager);
        Harness::reopen(dir, profile, folder, store, drivers, options)
    }

    /// Stops as a crash would: what reached the store is saved, the actors and their processes
    /// die without ending anything, and Ghost starts again on the same profile.
    pub async fn crash_and_restart(
        self,
        drivers: Vec<ScriptedDriver>,
        options: Options,
    ) -> Harness {
        let Harness {
            dir,
            profile,
            folder,
            store,
            manager,
            ..
        } = self;
        assert!(
            store.shutdown(Duration::from_secs(5)).await,
            "what reached the store was saved"
        );
        manager.kill_all_now();
        drop(manager);
        Harness::reopen(dir, profile, folder, store, drivers, options)
    }

    fn reopen(
        dir: tempfile::TempDir,
        profile: Profile,
        folder: PathBuf,
        old: Arc<ChatStore>,
        drivers: Vec<ScriptedDriver>,
        options: Options,
    ) -> Harness {
        drop(old);
        let owner = ProfileOwner::acquire(&profile).expect("the old store released the owner lock");
        let (store, _) = Store::open(&profile).expect("store");
        let store = ChatStore::start(store, owner, &profile, "device".into(), Handle::current())
            .expect("chat store");
        Harness::over(dir, profile, folder, store, drivers, options)
    }

    /// A new chat with the demo agent in the harness's folder (its agent starts).
    pub async fn chat(&self) -> ChatSummary {
        self.chat_with(MOCK).await
    }

    pub async fn chat_with(&self, agent: &str) -> ChatSummary {
        self.manager
            .create_chat(agent, &self.folder)
            .await
            .expect("create the chat")
    }

    /// Waits until the chat's status channel reports `status`.
    pub async fn status(&self, chat: &str, status: SessionStatus) {
        let statuses = Arc::clone(&self.statuses);
        let chat = chat.to_owned();
        eventually(&format!("{chat} becomes {status:?}"), move || {
            statuses
                .last(&chat)
                .is_some_and(|update| update.status == status)
        })
        .await;
    }

    /// Every stored event of the chat, oldest first, with its seq and turn.
    pub async fn history(&self, chat: &str) -> Vec<HistoryItem> {
        let mut items = Vec::new();
        let mut cursor = u32::MAX;
        loop {
            let page = self
                .manager
                .history(chat, cursor, Some(4 * 1024 * 1024))
                .await
                .expect("history");
            let mut older = page.items;
            older.extend(items);
            items = older;
            match (page.has_more, page.before_seq) {
                (true, Some(before)) => cursor = before,
                _ => return items,
            }
        }
    }

    /// The chat's queued prompts, as an attach's snapshot shows them.
    pub async fn queue(&self, chat: &str) -> Vec<QueuedPrompt> {
        let attach = self
            .manager
            .attach(chat, Sink::new("queue-probe"))
            .await
            .expect("attach");
        self.manager.detach(&attach.subscription_id);
        attach.snapshot.queue
    }

    /// Every stored event of the chat, oldest first.
    pub async fn stored(&self, chat: &str) -> Vec<GhostEvent> {
        self.history(chat)
            .await
            .into_iter()
            .map(|item| item.event)
            .collect()
    }
}

/// Hands out scripted drivers in order and records the setup each one started with. Once they run
/// out, a start fails with an internal error card.
pub struct Factory {
    queue: Mutex<VecDeque<ScriptedDriver>>,
    setups: Arc<Mutex<Vec<SessionSetup>>>,
}

impl Factory {
    pub fn new(drivers: Vec<ScriptedDriver>) -> Arc<Factory> {
        Arc::new(Factory {
            queue: Mutex::new(drivers.into()),
            setups: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Every driver's setup so far, in start order.
    pub fn setups(&self) -> Vec<SessionSetup> {
        self.setups.lock().expect("setups").clone()
    }

    pub fn push(&self, driver: ScriptedDriver) {
        self.queue.lock().expect("queue").push_back(driver);
    }
}

impl DriverFactory for Factory {
    fn create(&self, _kind: DriverKind) -> Box<dyn AgentDriver> {
        let driver = self
            .queue
            .lock()
            .expect("queue")
            .pop_front()
            .unwrap_or_else(|| {
                ScriptedDriver::new(vec![Step::Exit(DriverExit::StartFailed(DriverError {
                    kind: ErrorKind::Internal,
                    message: "The test has no driver left for this start.".into(),
                    detail: None,
                }))])
                .0
            });
        Box::new(Recording {
            driver,
            setups: Arc::clone(&self.setups),
        })
    }
}

/// A scripted driver that records its setup first.
struct Recording {
    driver: ScriptedDriver,
    setups: Arc<Mutex<Vec<SessionSetup>>>,
}

impl AgentDriver for Recording {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        self.setups.lock().expect("setups").push(ctx.setup.clone());
        Box::new(self.driver).run(ctx)
    }
}

/// Polls `condition` (on the test's clock) until it holds; fails after [`PATIENCE`].
pub async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting until {what}"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// A chat view that records every frame.
pub struct Sink {
    owner: String,
    frames: Mutex<Vec<EventBatch>>,
    gone: AtomicBool,
    panics: AtomicBool,
}

impl Sink {
    pub fn new(owner: &str) -> Arc<Sink> {
        Arc::new(Sink {
            owner: owner.to_owned(),
            frames: Mutex::new(Vec::new()),
            gone: AtomicBool::new(false),
            panics: AtomicBool::new(false),
        })
    }

    pub fn frames(&self) -> Vec<EventBatch> {
        self.frames.lock().expect("frames").clone()
    }

    /// Every event received, with its seq.
    pub fn events(&self) -> Vec<(u32, GhostEvent)> {
        self.frames()
            .into_iter()
            .flat_map(|frame| {
                let start = frame.seq_start;
                frame
                    .events
                    .into_iter()
                    .enumerate()
                    .map(move |(offset, event)| (start + offset as u32, event))
            })
            .collect()
    }

    /// From now on the receiver is gone.
    pub fn set_gone(&self) {
        self.gone.store(true, Ordering::SeqCst);
    }

    /// From now on a send panics (a sink that breaks its contract, to crash the actor).
    pub fn set_panics(&self) {
        self.panics.store(true, Ordering::SeqCst);
    }

    /// Waits until an event matching `matches` arrived; returns it.
    pub async fn wait_for(
        self: &Arc<Self>,
        what: &str,
        matches: impl Fn(&GhostEvent) -> bool,
    ) -> GhostEvent {
        let sink = Arc::clone(self);
        let mut found = None;
        eventually(what, || {
            found = sink
                .events()
                .into_iter()
                .map(|(_, event)| event)
                .find(|event| matches(event));
            found.is_some()
        })
        .await;
        found.expect("found")
    }
}

impl FrameSink for Sink {
    fn send(&self, frame: &EventBatch) -> Result<(), SinkGone> {
        assert!(
            !self.panics.load(Ordering::SeqCst),
            "this test's sink panics on purpose"
        );
        if self.gone.load(Ordering::SeqCst) {
            return Err(SinkGone);
        }
        self.frames.lock().expect("frames").push(frame.clone());
        Ok(())
    }

    fn owner(&self) -> &str {
        &self.owner
    }
}

/// A webview's status channel that records every update.
pub struct Statuses {
    owner: String,
    updates: Mutex<Vec<ChatStatusUpdate>>,
}

impl Statuses {
    pub fn new(owner: &str) -> Arc<Statuses> {
        Arc::new(Statuses {
            owner: owner.to_owned(),
            updates: Mutex::new(Vec::new()),
        })
    }

    pub fn last(&self, chat: &str) -> Option<ChatStatusUpdate> {
        self.updates
            .lock()
            .expect("updates")
            .iter()
            .rev()
            .find(|update| update.chat_id == chat)
            .cloned()
    }

    pub fn all(&self, chat: &str) -> Vec<ChatStatusUpdate> {
        self.updates
            .lock()
            .expect("updates")
            .iter()
            .filter(|update| update.chat_id == chat)
            .cloned()
            .collect()
    }
}

impl StatusSink for Statuses {
    fn send(&self, update: &ChatStatusUpdate) -> Result<(), SinkGone> {
        self.updates.lock().expect("updates").push(update.clone());
        Ok(())
    }

    fn owner(&self) -> &str {
        &self.owner
    }
}

/// What a scripted agent reports at `Ready`.
pub fn ready_info(native: &str) -> ReadyInfo {
    ReadyInfo {
        native_session_id: Some(native.into()),
        agent_name: Some("Demo".into()),
        agent_version: Some("1.0.0".into()),
        capabilities: Capabilities::default(),
        resumed: ResumeOutcome::New,
        spawn_id: "spawn-1".into(),
        replay: None,
    }
}

pub fn ready_step(native: &str) -> Step {
    Step::Emit(DriverEvent::Ready(ready_info(native)))
}

/// A transcript event for `EmitInTurn` (tagged with the turn the actor minted).
pub fn in_turn(event: GhostEvent) -> Step {
    Step::EmitInTurn(DriverEvent::Event { turn: None, event })
}

pub fn message(id: &str, text: &str) -> GhostEvent {
    GhostEvent::MessageDelta {
        message_id: id.into(),
        text: text.into(),
    }
}

/// A permission request with Allow once, Allow always and Reject once.
pub fn permission(id: &str, kind: Option<ToolKind>) -> PermissionRequest {
    PermissionRequest {
        request_id: id.into(),
        tool_call_id: None,
        title: format!("May I {id}?"),
        kind,
        raw_input: None,
        content: Vec::new(),
        locations: Vec::new(),
        options: vec![
            PermissionOption {
                option_id: "always".into(),
                kind: PermissionOptionKind::AllowAlways,
                label: "Always".into(),
            },
            PermissionOption {
                option_id: "once".into(),
                kind: PermissionOptionKind::AllowOnce,
                label: "Allow once".into(),
            },
            PermissionOption {
                option_id: "no".into(),
                kind: PermissionOptionKind::RejectOnce,
                label: "No".into(),
            },
        ],
        truncated: None,
    }
}

/// The turn id of the next prompt the driver received that no earlier call returned.
pub async fn prompt_turn(handle: &ScriptHandle) -> String {
    match command(handle, ghost_drivers::testing::CommandMatch::Prompt).await {
        ghost_drivers::driver::LoggedCommand::Prompt { turn_id, .. } => turn_id,
        other => panic!("expected a prompt, got {other:?}"),
    }
}

/// Wait for an external writer/driver handshake without advancing the test's paused clock.
/// Logical driver waits are advanced explicitly by the test; a timeout must use real time here,
/// because the profile writer is an OS thread and Tokio cannot observe its outstanding work.
pub async fn command(
    handle: &ScriptHandle,
    matcher: ghost_drivers::testing::CommandMatch,
) -> ghost_drivers::driver::LoggedCommand {
    let deadline = std::time::Instant::now() + PATIENCE;
    let received = handle.wait_for(matcher.clone());
    tokio::pin!(received);
    loop {
        tokio::select! {
            biased;
            command=&mut received=>return command,
            _=tokio::task::yield_now()=>{}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the driver never received {matcher:?}"
        );
    }
}

/// The events of `kind` in order.
pub fn kinds(events: &[GhostEvent]) -> Vec<&'static str> {
    events.iter().map(GhostEvent::kind).collect()
}
