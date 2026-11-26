//! The session manager's tests that need the crate's internals: a store whose FIFO and retry are
//! shrunk (saving pauses within a test), a small actor outbox, and the actor's panic hook.

mod checkpoints;
mod cloning;
mod controls;
mod conversation;
mod coordinator;
mod customization_queue;
mod folder_context;
mod folder_resolution;
mod memory;
mod new_chat_defaults;
mod project_tools;
mod queue_changes;
mod rewind;
mod steering;
mod stop_many;
mod terminal_chats;
mod workspace;
mod workspace_removal;
mod worktree_chat;
mod worktree_guards;
mod worktree_setup;

use std::sync::Mutex;
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use ghost_agents::{Detector, FixedAgent};
use ghost_drivers::driver::{
    AgentDriver, DriverContext, DriverEvent, DriverExit, ReadyInfo, ResumeOutcome,
};
use ghost_drivers::testing::{CommandMatch, ScriptedDriver, Step};
use ghost_process::testing::{FakeAgentEnd, FakeCall, FakeSpawner};
use ghost_process::{EnvKey, EnvMap, LaunchSpec, ProcessCwd, ResolvedEnv};
use ghost_protocol::session::{PauseReason, StorageState};
use ghost_protocol::{Capabilities, GhostEvent, StopReason};

use super::*;
use crate::chatstore::WriterConfig;
use crate::owner::ProfileOwner;
use crate::paths::DataRoot;
use crate::profile::{Profile, Profiles};
use crate::store::Store;

/// A manager over a temporary profile whose store and actors run with test limits.
struct Rig {
    _dir: tempfile::TempDir,
    profile: Profile,
    folder: PathBuf,
    store: Arc<ChatStore>,
    manager: SessionManager,
    spawner: Arc<FakeSpawner>,
    _ends: mpsc::UnboundedReceiver<FakeAgentEnd>,
    statuses: Arc<Statuses>,
}

fn launch() -> LaunchSpec {
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

fn ready_step(native: &str) -> Step {
    Step::Emit(DriverEvent::Ready(ReadyInfo {
        native_session_id: Some(native.into()),
        agent_name: Some("Demo".into()),
        agent_version: Some("1.0.0".into()),
        capabilities: Capabilities::default(),
        resumed: ResumeOutcome::New,
        spawn_id: "spawn-1".into(),
        replay: None,
    }))
}

fn delta(text: &str) -> Step {
    Step::EmitInTurn(DriverEvent::Event {
        turn: None,
        event: GhostEvent::MessageDelta {
            message_id: "m1".into(),
            text: text.into(),
        },
    })
}

fn rig(drivers: Vec<ScriptedDriver>, config: WriterConfig, knobs: Knobs) -> Rig {
    rig_with(
        ghost_drivers::testing::ScriptedDriverFactory::new(drivers),
        config,
        knobs,
    )
}

fn rig_with(drivers: Arc<dyn DriverFactory>, config: WriterConfig, knobs: Knobs) -> Rig {
    rig_with_spec(
        drivers,
        config,
        knobs,
        ghost_agents::get("mock").expect("mock").clone(),
    )
}

fn rig_with_spec(
    drivers: Arc<dyn DriverFactory>,
    config: WriterConfig,
    knobs: Knobs,
    spec: ghost_agents::AgentSpec,
) -> Rig {
    let dir = tempfile::tempdir().expect("tempdir");
    let profile = Profiles::new(DataRoot::at(dir.path().join("data")))
        .open_active()
        .expect("profile");
    let folder = dir.path().join("work");
    std::fs::create_dir_all(&folder).expect("folder");
    let owner = ProfileOwner::acquire(&profile).expect("owner");
    let (store, _) = Store::open(&profile).expect("store");
    let rt = Handle::current();
    let store = ChatStore::start_with(store, owner, &profile, "device".into(), rt.clone(), config)
        .expect("store");
    rig_from_store(dir, profile, folder, store, drivers, knobs, spec)
}

thread_local! {
    /// More variables for the next rig built on this thread (a test's home folder, say).
    static RIG_ENV: std::cell::RefCell<Vec<(String, String)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The limits of the next rig built on this thread, when not the standard ones.
    static RIG_LIMITS: std::cell::Cell<Option<ManagerLimits>> = const { std::cell::Cell::new(None) };
}

/// A rig built with `limits` (a cap of one agent, say).
fn with_limits(limits: ManagerLimits, build: impl FnOnce() -> Rig) -> Rig {
    RIG_LIMITS.with(|cell| cell.set(Some(limits)));
    let rig = build();
    RIG_LIMITS.with(|cell| cell.set(None));
    rig
}

fn rig_from_store(
    dir: tempfile::TempDir,
    profile: Profile,
    folder: PathBuf,
    store: Arc<ChatStore>,
    drivers: Arc<dyn DriverFactory>,
    knobs: Knobs,
    spec: ghost_agents::AgentSpec,
) -> Rig {
    let rt = Handle::current();
    let mut vars = EnvMap::new();
    vars.insert(EnvKey::from("PATH"), "/usr/bin".into());
    RIG_ENV.with(|extra| {
        for (key, value) in extra.borrow().iter() {
            vars.insert(EnvKey::from(key.as_str()), value.into());
        }
    });
    let env = EnvResolver::fixed(ResolvedEnv {
        vars,
        source: EnvSource::Fallback {
            reason: "test".into(),
        },
        warnings: Vec::new(),
        resolved_at_ms: 0,
    });
    let detector = Detector::fixed(vec![FixedAgent::new(
        spec,
        ghost_protocol::session::Availability::Ready {
            version: None,
            path: "built in".into(),
        },
        Some(launch()),
    )]);
    let (spawner, ends) = FakeSpawner::new();
    let redactor = Arc::new(Redactor::standard());
    let logs = LogFactory::new(
        profile.logs_dir().join("protocol"),
        Arc::clone(&redactor),
        rt.clone(),
    );
    let manager = SessionManager::with_knobs(
        ManagerDeps {
            worktrees: None,
            store: Arc::clone(&store),
            checkpoints: None,
            env,
            spawner: spawner.clone(),
            detector,
            drivers,
            logs,
            redactor,
            device_id: "device".into(),
            app_version: "0.1.0".into(),
            limits: RIG_LIMITS
                .with(std::cell::Cell::get)
                .unwrap_or(ManagerLimits::STANDARD),
            timings: SessionTimings::STANDARD,
            rt,
        },
        knobs,
    );
    let statuses = Arc::new(Statuses::default());
    manager.status_subscribe(statuses.clone());
    Rig {
        _dir: dir,
        profile,
        folder,
        store,
        manager,
        spawner,
        _ends: ends,
        statuses,
    }
}

/// A store that pauses quickly and a FIFO and outbox that fill quickly.
fn tight() -> (WriterConfig, Knobs) {
    (
        WriterConfig {
            fifo_ops: 8,
            fifo_bytes: 64 * 1024,
            disk_full_retry: Duration::from_millis(50),
            ..WriterConfig::STANDARD
        },
        Knobs {
            outbox_bytes: 64 * 1024,
            ..Knobs::STANDARD
        },
    )
}

#[derive(Default)]
struct Statuses {
    updates: Mutex<Vec<ChatStatusUpdate>>,
}

impl StatusSink for Statuses {
    fn send(&self, update: &ChatStatusUpdate) -> Result<(), SinkGone> {
        lock(&self.updates).push(update.clone());
        Ok(())
    }

    fn owner(&self) -> &str {
        "main"
    }
}

impl Statuses {
    fn status(&self, chat: &str) -> Option<SessionStatus> {
        lock(&self.updates)
            .iter()
            .rev()
            .find(|update| update.chat_id == chat)
            .map(|update| update.status)
    }
}

#[derive(Default)]
struct Sink {
    frames: Mutex<Vec<EventBatch>>,
}

impl FrameSink for Sink {
    fn send(&self, frame: &EventBatch) -> Result<(), SinkGone> {
        lock(&self.frames).push(frame.clone());
        Ok(())
    }

    fn owner(&self) -> &str {
        "main"
    }
}

async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting until {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn status(rig: &Rig, chat: &str, wanted: SessionStatus) {
    let statuses = Arc::clone(&rig.statuses);
    let chat = chat.to_owned();
    eventually(&format!("{chat} is {wanted:?}"), move || {
        statuses.status(&chat) == Some(wanted)
    })
    .await;
}

/// Caps the database at its current size (plus one page): the next batch that grows it fails
/// with SQLITE_FULL.
fn fill_disk(store: &ChatStore) {
    store.with_store(|store| {
        let pages: i64 = store
            .conn()
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .expect("pages");
        let _: i64 = store
            .conn()
            .query_row(
                &format!("PRAGMA max_page_count = {}", pages + 1),
                [],
                |row| row.get(0),
            )
            .expect("cap");
    });
}

fn free_disk(store: &ChatStore) {
    store.with_store(|store| {
        let _: i64 = store
            .conn()
            .query_row("PRAGMA max_page_count = 1073741823", [], |row| row.get(0))
            .expect("uncap");
    });
}

async fn paused(rig: &Rig) {
    let mut status = rig.store.status();
    tokio::time::timeout(
        Duration::from_secs(30),
        status.wait_for(|status| matches!(status.state, StorageState::Paused { .. })),
    )
    .await
    .expect("saving pauses")
    .expect("status");
}

/// A flood of agent text: 300 deltas of 2 KB each, then the turn's end and a crash.
fn flooding_driver() -> (ScriptedDriver, ghost_drivers::testing::ScriptHandle) {
    let mut steps = vec![
        Step::Spawn(launch()),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
    ];
    for i in 0..300 {
        steps.push(delta(&format!("{i:04}{}", "z".repeat(2000))));
    }
    steps.push(Step::EndTurn {
        stop_reason: StopReason::EndTurn,
        usage: None,
        error: None,
    });
    steps.push(Step::Exit(DriverExit::Crashed {
        exit: None,
        stderr_tail: String::new(),
    }));
    ScriptedDriver::new(steps)
}

fn flood_text() -> String {
    (0..300)
        .map(|i| format!("{i:04}{}", "z".repeat(2000)))
        .collect()
}

/// S3: a full disk pauses saving; the writer keeps only the failed batch, the FIFO and the
/// actor's outbox fill, and the backpressure reaches the agent, which blocks; the agent exiting
/// meanwhile loses nothing; freeing space resumes and every event is saved.
#[tokio::test]
async fn disk_full_pauses_without_loss() {
    let (config, knobs) = tight();
    let (driver, handle) = flooding_driver();
    let rig = rig(vec![driver], config, knobs);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    fill_disk(&rig.store);
    rig.manager
        .send(&chat.id, "write a lot".into())
        .await
        .expect("send");
    paused(&rig).await;
    assert!(matches!(
        rig.manager.storage_status().state,
        StorageState::Paused {
            reason: PauseReason::DiskFull,
            ..
        }
    ));
    // The agent is blocked: its turn's end and its exit stay unseen while saving is paused.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(rig.statuses.status(&chat.id), Some(SessionStatus::Running));
    assert!(handle.commands().len() == 1, "only the prompt reached it");
    free_disk(&rig.store);
    status(&rig, &chat.id, SessionStatus::Stopped).await;
    let mut storage = rig.store.status();
    tokio::time::timeout(
        Duration::from_secs(30),
        storage.wait_for(|status| status.state == StorageState::Ok),
    )
    .await
    .expect("saving resumes")
    .expect("status");
    let page = rig
        .manager
        .history(&chat.id, u32::MAX, Some(4 * 1024 * 1024))
        .await
        .expect("history");
    let text: String = page
        .items
        .iter()
        .filter_map(|item| match &item.event {
            GhostEvent::MessageDelta { message_id, text } if message_id == "m1" => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(text.len(), flood_text().len());
    assert_eq!(text, flood_text(), "nothing was lost");
    assert!(page.items.iter().any(|item| matches!(
        item.event,
        GhostEvent::TurnEnded {
            stop_reason: StopReason::EndTurn,
            ..
        }
    )));
}

/// S8: while saving is paused, an attach is served at once from saved rows, with the unsaved part
/// marked; chat lists answer at once; the view attaches again fully once saving resumes.
#[tokio::test]
async fn attach_while_paused_reads_saved_rows() {
    let (config, knobs) = tight();
    let (driver, _handle) = flooding_driver();
    let rig = rig(vec![driver], config, knobs);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    fill_disk(&rig.store);
    rig.manager
        .send(&chat.id, "write a lot".into())
        .await
        .expect("send");
    paused(&rig).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    let attach = rig
        .manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .expect("attach while paused");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert!(matches!(
        attach.snapshot.storage.state,
        StorageState::Paused { .. }
    ));
    let unsaved = attach
        .history
        .unsaved_from
        .expect("the unsaved rows are marked");
    assert!(attach.history.items.iter().all(|item| item.seq < unsaved));
    assert!(unsaved < attach.next_seq);
    let started = std::time::Instant::now();
    assert_eq!(rig.manager.chats().await.expect("chats").len(), 1);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "reads never wait behind the paused writer"
    );
    free_disk(&rig.store);
    status(&rig, &chat.id, SessionStatus::Stopped).await;
    let mut storage = rig.store.status();
    tokio::time::timeout(
        Duration::from_secs(30),
        storage.wait_for(|status| status.state == StorageState::Ok),
    )
    .await
    .expect("saving resumes")
    .expect("status");
    let again = rig
        .manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .expect("attach again");
    assert_eq!(again.history.unsaved_from, None);
}

/// S34: force stop works while saving is paused and the agent floods.
#[tokio::test]
async fn force_stop_while_storage_paused() {
    let (config, knobs) = tight();
    let (driver, _handle) = flooding_driver();
    let rig = rig(vec![driver], config, knobs);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    fill_disk(&rig.store);
    rig.manager
        .send(&chat.id, "write a lot".into())
        .await
        .expect("send");
    paused(&rig).await;
    // The actor still answers while its outbox is full: actors and drivers never wait on each other.
    rig.manager
        .lists(&chat.id)
        .await
        .expect("the actor answers");
    rig.manager.force_stop(&chat.id).await.expect("force stop");
    status(&rig, &chat.id, SessionStatus::Stopped).await;
    let control = &rig.spawner.controls()[0];
    eventually("the agent's process is force-stopped", || {
        control
            .calls()
            .contains(&FakeCall::Shutdown(ghost_process::ShutdownPlan::FORCE))
    })
    .await;
    free_disk(&rig.store);
}

/// S25: quitting while paused is bounded; what was not saved is dropped (and logged by count), and
/// the next start reads a consistent database.
#[tokio::test]
async fn quit_while_paused() {
    let (config, knobs) = tight();
    let (driver, _handle) = flooding_driver();
    let rig = rig(vec![driver], config, knobs);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    fill_disk(&rig.store);
    rig.manager
        .send(&chat.id, "write a lot".into())
        .await
        .expect("send");
    paused(&rig).await;
    let started = std::time::Instant::now();
    let summary = rig.manager.shutdown_all(Duration::from_millis(500)).await;
    assert_eq!(summary.forced, 1, "{summary:?}");
    assert!(
        !rig.store.shutdown(Duration::from_millis(300)).await,
        "something was unsaved"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    // The next start: recovery ends the turn the quit cut short.
    let Rig {
        profile,
        manager,
        store,
        ..
    } = rig;
    drop(manager);
    drop(store);
    let owner = ProfileOwner::acquire(&profile).expect("owner");
    let (reopened, _) = Store::open(&profile).expect("reopen");
    let store = ChatStore::start(
        reopened,
        owner,
        &profile,
        "device".into(),
        Handle::current(),
    )
    .expect("store");
    let report = store.recovered().await;
    assert!(report.interrupted_turns <= 1);
    let load = store.load_chat(&chat.id).await.expect("load");
    assert_eq!(load.chat.id, chat.id);
    assert!(store.shutdown(Duration::from_secs(5)).await);
}

/// An update replaces the agent's program files, so neither Update nor its terminal run starts
/// while one of the agent's chats has a turn under way. With every chat idle the request goes on
/// to the detector, which answers for itself.
#[tokio::test]
async fn an_agent_is_not_updated_under_a_working_chat() {
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        // The turn stays open.
        delta("working"),
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let refusal = |result: Result<(), SessionError>| match result {
        Err(SessionError::AgentUnavailable(reason)) => reason,
        other => panic!("expected a refusal, got {other:?}"),
    };

    // Idle: nothing of the session's stands in the way. (This rig's detector updates nothing.)
    let idle = refusal(rig.manager.update_agent("mock").await);
    assert!(!idle.contains("working"), "{idle}");

    rig.manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .expect("attach");
    rig.manager
        .send(&chat.id, "first".into())
        .await
        .expect("send");
    handle.wait_for(CommandMatch::Prompt).await;
    status(&rig, &chat.id, SessionStatus::Running).await;

    let working = refusal(rig.manager.update_agent("mock").await);
    assert!(
        working.ends_with("has a chat working. Update it once that chat finishes."),
        "{working}"
    );
    let terminal = refusal(rig.manager.prepare_agent_update("mock").await.map(drop));
    assert_eq!(terminal, working);
    // Nothing ran, so there is nothing an updater printed, and the chat is still at work.
    assert_eq!(rig.manager.agent_update_output("mock"), None);
    assert_eq!(rig.statuses.status(&chat.id), Some(SessionStatus::Running));
}

/// A quit drains every actor before the store closes. Writes queued while a turn runs (two
/// prompts, the hold the quit puts on them, the interrupted turn's end) are all saved; work that
/// reaches an actor after it answered its quit is refused instead of being queued for a closed
/// store; and no actor starts once the quit began. Nothing is lost and nothing is logged as lost.
#[tokio::test]
async fn quit_drains_queued_writes_and_refuses_late_work() {
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        // The turn stays open; with no steps left the driver closes on the quit's Shutdown.
        delta("working"),
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let unloaded = rig
        .store
        .create_chat(NewChat {
            agent_host: Default::default(),
            agent_id: "mock".into(),
            folder: rig.folder.clone(),
            device_id: "device".into(),
            now: now_millis(),
        })
        .await
        .expect("a chat no actor loaded")
        .id;
    rig.manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .expect("attach");
    rig.manager
        .send(&chat.id, "first".into())
        .await
        .expect("send");
    handle.wait_for(CommandMatch::Prompt).await;
    status(&rig, &chat.id, SessionStatus::Running).await;
    rig.manager
        .send(&chat.id, "queued one".into())
        .await
        .expect("queue");
    rig.manager
        .send(&chat.id, "queued two".into())
        .await
        .expect("queue");

    let summary = rig.manager.shutdown_all(Duration::from_secs(5)).await;
    assert_eq!((summary.sessions, summary.forced), (1, 0), "{summary:?}");

    // Late work: refused at once, never queued behind the closing store.
    assert!(matches!(
        rig.manager.send(&chat.id, "late".into()).await,
        Err(SessionError::ShuttingDown)
    ));
    assert!(matches!(
        rig.manager
            .attach(&chat.id, Arc::new(Sink::default()))
            .await,
        Err(SessionError::ShuttingDown)
    ));
    assert!(matches!(
        rig.manager.history(&chat.id, u32::MAX, None).await,
        Err(SessionError::ShuttingDown)
    ));
    assert!(matches!(
        rig.manager.send(&unloaded, "late".into()).await,
        Err(SessionError::ShuttingDown)
    ));
    assert!(
        rig.manager.lists(&chat.id).await.is_ok(),
        "answers from memory still work"
    );

    assert!(
        rig.store.shutdown(Duration::from_secs(5)).await,
        "everything queued was saved"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.manager.lost_writes(), 0);

    let Rig {
        profile,
        manager,
        store,
        ..
    } = rig;
    drop(manager);
    drop(store);
    let owner = ProfileOwner::acquire(&profile).expect("owner");
    let (reopened, _) = Store::open(&profile).expect("reopen");
    let store = ChatStore::start(
        reopened,
        owner,
        &profile,
        "device".into(),
        Handle::current(),
    )
    .expect("store");
    assert_eq!(
        store.recovered().await.interrupted_turns,
        0,
        "the turn ended"
    );
    let load = store.load_chat(&chat.id).await.expect("load");
    let queued: Vec<(&str, bool)> = load
        .queue
        .iter()
        .map(|row| (row.text.as_str(), row.held))
        .collect();
    assert_eq!(queued, [("queued one", true), ("queued two", true)]);
    assert!(store.shutdown(Duration::from_secs(5)).await);
}

/// S34: an agent flooding while saving is paused never deadlocks with its actor: the actor
/// stops reading but keeps answering, the driver keeps answering commands while it waits for room
/// (an option change completes), and once saving resumes everything flows and is saved.
#[tokio::test]
async fn actor_and_driver_never_deadlock() {
    let (config, knobs) = tight();
    let option = ghost_protocol::session::ConfigOption {
        id: "model".into(),
        name: "Model".into(),
        description: None,
        category: ghost_protocol::session::ConfigCategory::Model,
        kind: ghost_protocol::session::ConfigKind::Select {
            current: "fast".into(),
            choices: vec![
                ghost_protocol::session::ConfigChoice {
                    value: "fast".into(),
                    name: "Fast".into(),
                    description: None,
                    group: None,
                },
                ghost_protocol::session::ConfigChoice {
                    value: "smart".into(),
                    name: "Smart".into(),
                    description: None,
                    group: None,
                },
            ],
        },
        pending: None,
    };
    let mut steps = vec![
        Step::Spawn(launch()),
        Step::Emit(DriverEvent::Options(vec![option])),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
    ];
    for i in 0..300 {
        steps.push(delta(&format!("{i:04}{}", "z".repeat(2000))));
    }
    steps.push(Step::EndTurn {
        stop_reason: StopReason::EndTurn,
        usage: None,
        error: None,
    });
    // The option change arrived during the flood; then the prompt queued meanwhile.
    steps.push(Step::Expect(CommandMatch::SetOption));
    steps.push(Step::Expect(CommandMatch::Prompt));
    steps.push(Step::EndTurn {
        stop_reason: StopReason::EndTurn,
        usage: None,
        error: None,
    });
    let (driver, handle) = ScriptedDriver::new(steps);
    let rig = rig(vec![driver], config, knobs);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    fill_disk(&rig.store);
    rig.manager
        .send(&chat.id, "write a lot".into())
        .await
        .expect("send");
    paused(&rig).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        rig.statuses.status(&chat.id),
        Some(SessionStatus::Running),
        "blocked, not done"
    );
    let lists = tokio::time::timeout(
        Duration::from_secs(5),
        rig.manager.set_option(
            &chat.id,
            "model",
            ghost_protocol::session::ConfigValue::Choice {
                value: "smart".into(),
            },
        ),
    )
    .await
    .expect("no deadlock")
    .expect("the option change completes");
    assert_eq!(lists.options.len(), 1);
    // The actor reads no driver event while saving is paused, yet the answer shows the choice.
    assert!(matches!(
        &lists.options[0].kind,
        ghost_protocol::session::ConfigKind::Select { current, .. } if current == "smart"
    ));
    assert!(handle.commands().iter().any(|command| matches!(
        command,
        ghost_drivers::driver::LoggedCommand::SetOption { .. }
    )));
    let queued = rig
        .manager
        .send(&chat.id, "next".into())
        .await
        .expect("send while blocked");
    assert!(queued.queued);
    free_disk(&rig.store);
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let page = rig
        .manager
        .history(&chat.id, u32::MAX, Some(4 * 1024 * 1024))
        .await
        .expect("history");
    let text: String = page
        .items
        .iter()
        .filter_map(|item| match &item.event {
            GhostEvent::MessageDelta { message_id, text } if message_id == "m1" => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(text, flood_text());
}

/// The full output a spilled tool call carries: 100,000 bytes.
fn big_output() -> String {
    "x".repeat(100_000)
}

/// A turn that emits `deltas` deltas, each in its own flush, then a tool call whose output is
/// spilled to a blob, then hangs.
fn spilling_driver(deltas: usize) -> ScriptedDriver {
    let mut steps = vec![
        Step::Spawn(launch()),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
    ];
    for i in 0..deltas {
        steps.push(delta(&format!("{i:04}{}", "z".repeat(2000))));
        steps.push(Step::Wait(Duration::from_millis(20)));
    }
    steps.push(Step::EmitInTurn(DriverEvent::Event {
        turn: None,
        event: GhostEvent::ToolCall(ghost_protocol::ToolCall {
            tool_call_id: "big".into(),
            kind: ghost_protocol::ToolKind::Execute,
            title: "Dump".into(),
            status: ghost_protocol::ToolStatus::Completed,
            locations: Vec::new(),
            raw_input: None,
            tool_name: None,
            content: vec![ghost_protocol::ToolContent::Text {
                text: big_output(),
                truncated: None,
            }],
            truncated: None,
        }),
    }));
    steps.push(Step::Hang);
    ScriptedDriver::new(steps).0
}

/// The blob a tool call's output was spilled to, once a frame named it.
fn spilled_blob(sink: &Sink) -> Option<String> {
    lock(&sink.frames)
        .iter()
        .flat_map(|frame| frame.events.iter())
        .find_map(|event| match event {
            GhostEvent::ToolCall(call) => match call.content.first() {
                Some(ghost_protocol::ToolContent::Text {
                    truncated: Some(truncation),
                    ..
                }) => truncation.blob.clone(),
                _ => None,
            },
            _ => None,
        })
}

/// Reads a whole blob through the manager, page by page.
async fn read_blob(rig: &Rig, chat: &str, sha256: &str) -> Result<String, SessionError> {
    let mut text = String::new();
    let mut offset = 0;
    loop {
        let page = rig.manager.blob(chat, sha256, offset, 256 * 1024).await?;
        text.push_str(&page.text);
        offset = page.next_offset;
        if page.eof {
            return Ok(text);
        }
    }
}

/// S8 and 7.1 `chat_blob`: while saving is paused, a blob whose `PutBlob` still waits in the actor's
/// outbox (the FIFO is full) reads as not saved yet, though a frame already named it, never as gone;
/// once saving resumes it reads back whole.
#[tokio::test]
async fn a_blob_still_in_the_outbox_reads_as_unsaved_while_paused() {
    let (config, _) = tight();
    let rig = rig(vec![spilling_driver(40)], config, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let sink = Arc::new(Sink::default());
    rig.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    fill_disk(&rig.store);
    rig.manager
        .send(&chat.id, "dump it".into())
        .await
        .expect("send");
    paused(&rig).await;
    let probe = Arc::clone(&sink);
    eventually("a frame names the spilled output", || {
        spilled_blob(&probe).is_some()
    })
    .await;
    let sha = spilled_blob(&sink).expect("the blob");
    assert!(matches!(
        rig.manager.blob(&chat.id, &sha, 0, 1000).await,
        Err(SessionError::BlobMissing(message)) if message == crate::chatstore::blobs::BLOB_UNSAVED
    ));
    free_disk(&rig.store);
    let mut storage = rig.store.status();
    tokio::time::timeout(
        Duration::from_secs(30),
        storage.wait_for(|status| status.state == StorageState::Ok),
    )
    .await
    .expect("saving resumes")
    .expect("status");
    assert_eq!(
        read_blob(&rig, &chat.id, &sha).await.expect("saved now"),
        big_output()
    );
}

/// 7.1 `chat_blob` under backpressure: a blob read right after a frame named the blob follows the
/// chat's own `PutBlob`, which still waits in the actor's outbox behind a full FIFO, instead of
/// overtaking it and finding no file.
#[tokio::test]
async fn a_blob_read_follows_the_chats_own_put_blob() {
    let (config, _) = tight();
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("blob-memory-admission"),
        Step::Expect(CommandMatch::Prompt),
    ]);
    let rig = rig(vec![driver], config, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let sink = Arc::new(Sink::default());
    rig.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    rig.manager
        .send(&chat.id, "dump it".into())
        .await
        .expect("send");
    // A real prompt's memory snapshot commits before native delivery. Only then block the writer
    // and release the scripted output, so this still exercises PutBlob ordering under backpressure.
    let ghost_drivers::driver::LoggedCommand::Prompt { turn_id, .. } =
        handle.wait_for(CommandMatch::Prompt).await
    else {
        panic!("prompt")
    };
    let (release, held) = std::sync::mpsc::channel::<()>();
    let (entered, waiting) = oneshot::channel();
    rig.store.with_store(move |_| {
        let _ = entered.send(());
        let _ = held.recv_timeout(Duration::from_secs(30));
    });
    waiting.await.expect("writer is blocked");
    for index in 0..12 {
        handle.emit(DriverEvent::Event {
            turn: Some(turn_id.clone()),
            event: GhostEvent::MessageDelta {
                message_id: "blob-output".into(),
                text: format!("{index:04}{}", "z".repeat(2000)),
            },
        });
    }
    handle.emit(DriverEvent::Event {
        turn: Some(turn_id),
        event: GhostEvent::ToolCall(ghost_protocol::ToolCall {
            tool_call_id: "big".into(),
            kind: ghost_protocol::ToolKind::Execute,
            title: "Dump".into(),
            status: ghost_protocol::ToolStatus::Completed,
            locations: Vec::new(),
            raw_input: None,
            tool_name: None,
            content: vec![ghost_protocol::ToolContent::Text {
                text: big_output(),
                truncated: None,
            }],
            truncated: None,
        }),
    });
    let probe = Arc::clone(&sink);
    eventually("a frame names the spilled output", || {
        spilled_blob(&probe).is_some()
    })
    .await;
    let sha = spilled_blob(&sink).expect("the blob");
    let read = tokio::spawn({
        let manager = rig.manager.clone();
        let (chat_id, sha) = (chat.id.clone(), sha.clone());
        async move { manager.blob(&chat_id, &sha, 0, 256 * 1024).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(release);
    let page = read
        .await
        .expect("join")
        .expect("the blob reads back once its PutBlob is saved");
    assert_eq!(page.total, 100_000);
    assert_eq!(
        read_blob(&rig, &chat.id, &sha).await.expect("whole"),
        big_output()
    );
}

/// P36 and P48: the chat list never waits long for a folder. One whose check does not answer in time
/// (a hung network mount) shows what it showed last while the others are current, and it is not
/// checked again while its check still runs.
#[tokio::test]
async fn the_chat_list_never_waits_long_for_a_folder() {
    let drivers = vec![
        ScriptedDriver::new(vec![ready_step("a")]).0,
        ScriptedDriver::new(vec![ready_step("b")]).0,
    ];
    let knobs = Knobs {
        folder_check_wait: Duration::from_millis(500),
        ..Knobs::STANDARD
    };
    let rig = rig(drivers, WriterConfig::STANDARD, knobs);
    let slow = rig.folder.join("slow");
    let quick = rig.folder.join("quick");
    for folder in [&slow, &quick] {
        std::fs::create_dir_all(folder).expect("folder");
    }
    let a = rig.manager.create_chat("mock", &slow).await.expect("a");
    let b = rig.manager.create_chat("mock", &quick).await.expect("b");
    status(&rig, &a.id, SessionStatus::Ready).await;
    status(&rig, &b.id, SessionStatus::Ready).await;
    let states = |listed: Vec<ChatSummary>| -> HashMap<String, FolderState> {
        listed
            .into_iter()
            .map(|chat| (chat.id, chat.folder_state))
            .collect()
    };
    let listed = states(rig.manager.chats().await.expect("chats"));
    assert_eq!(
        (listed[&a.id], listed[&b.id]),
        (FolderState::Ok, FolderState::Ok)
    );

    let checks = rig.manager.slow_folder(&slow, Duration::from_millis(1500));
    // Start the delayed check first: the quick folder must not depend on blocking-pool order.
    let slow_check = tokio::spawn({
        let manager = rig.manager.clone();
        let slow = slow.clone();
        async move { manager.check_folders(vec![slow]).await }
    });
    eventually("the delayed folder check started", || {
        checks.load(Ordering::SeqCst) == 1
    })
    .await;
    std::fs::remove_dir_all(&quick).expect("remove");
    let started = std::time::Instant::now();
    let listed = states(rig.manager.chats().await.expect("chats"));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        (listed[&a.id], listed[&b.id]),
        (FolderState::Ok, FolderState::Missing),
        "the hung folder shows its last state, the other one is current"
    );
    std::fs::remove_dir_all(&slow).expect("remove");
    let started = std::time::Instant::now();
    let listed = states(rig.manager.chats().await.expect("chats"));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(listed[&a.id], FolderState::Ok);
    assert_eq!(
        checks.load(Ordering::SeqCst),
        1,
        "not checked again while its check runs"
    );
    // Once the hung check answers, its result shows, even while the next check hangs again.
    slow_check.await.expect("the initial folder check task");
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let listed = states(rig.manager.chats().await.expect("chats"));
    assert_eq!(listed[&a.id], FolderState::Missing);
    assert_eq!(checks.load(Ordering::SeqCst), 2);
}

/// A driver that reports `Ready`, drops its event sender, and then never returns: only an abort
/// ends it, which it records.
struct DropsItsEvents {
    aborted: Arc<AtomicBool>,
}

/// Records that the future holding it was dropped, as an abort does.
struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl AgentDriver for DropsItsEvents {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        let aborted = SetOnDrop(self.aborted);
        async move {
            let _aborted = aborted;
            let DriverContext {
                commands, events, ..
            } = ctx;
            let ready = DriverEvent::Ready(ReadyInfo {
                native_session_id: Some("n1".into()),
                agent_name: None,
                agent_version: None,
                capabilities: Capabilities::default(),
                resumed: ResumeOutcome::New,
                spawn_id: "spawn-1".into(),
                replay: None,
            });
            if let Ok(permit) = events.reserve(ready.approx_bytes()).await {
                permit.send(ready);
            }
            drop(events);
            let _commands = commands;
            std::future::pending::<DriverExit>().await
        }
        .boxed()
    }
}

/// Hands out one driver; later starts fail.
struct OneDriver(Mutex<Option<Box<dyn AgentDriver>>>);

impl DriverFactory for OneDriver {
    fn create(&self, _kind: ghost_protocol::DriverKind) -> Box<dyn AgentDriver> {
        lock(&self.0).take().unwrap_or_else(|| {
            Box::new(
                ScriptedDriver::new(vec![Step::Exit(DriverExit::StartFailed(
                    ghost_drivers::driver::DriverError {
                        kind: ghost_protocol::ErrorKind::Internal,
                        message: "The test has no driver left for this start.".into(),
                        detail: None,
                    },
                ))])
                .0,
            )
        })
    }
}

/// 6.5.4 force stop, step 3, and 6.5.6: a driver that already dropped its event sender and then
/// hangs is still aborted 5 s after the kill; until it ends, the chat keeps its live slot.
#[tokio::test(start_paused = true)]
async fn force_stop_aborts_a_driver_whose_events_already_closed() {
    let aborted = Arc::new(AtomicBool::new(false));
    let driver: Box<dyn AgentDriver> = Box::new(DropsItsEvents {
        aborted: Arc::clone(&aborted),
    });
    let rig = rig_with(
        Arc::new(OneDriver(Mutex::new(Some(driver)))),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    // Let the actor see the event channel close, so the join leaves the driver first.
    tokio::time::sleep(Duration::from_millis(100)).await;
    rig.manager.force_stop(&chat.id).await.expect("force stop");
    status(&rig, &chat.id, SessionStatus::Stopped).await;
    assert!(!aborted.load(Ordering::SeqCst));
    assert!(
        rig.manager.has_live_sessions(),
        "the chat keeps its slot while its driver runs"
    );
    let started = tokio::time::Instant::now();
    let probe = Arc::clone(&aborted);
    eventually("the wedged driver is aborted", || {
        probe.load(Ordering::SeqCst)
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "aborted 5 s after the kill: {:?}",
        started.elapsed()
    );
    eventually("its slot is free", || !rig.manager.has_live_sessions()).await;
}

/// S22: a chat actor that panics: its processes are killed, its views end, the chat reads
/// `Stopped`, and the next call recreates it, which recovers the running turn.
#[tokio::test]
async fn actor_panic_is_contained() {
    let (driver, _handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        delta("partial"),
        Step::Hang,
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("create");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let sink = Arc::new(Sink::default());
    rig.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    rig.manager.send(&chat.id, "go".into()).await.expect("send");
    status(&rig, &chat.id, SessionStatus::Running).await;
    // Let the turn's first rows reach the store.
    rig.manager
        .history(&chat.id, u32::MAX, None)
        .await
        .expect("history");
    rig.manager.panic_actor(&chat.id);
    status(&rig, &chat.id, SessionStatus::Stopped).await;
    let control = &rig.spawner.controls()[0];
    eventually("the processes are killed", || {
        control.calls().contains(&FakeCall::KillTree)
    })
    .await;
    eventually("the views end", || Arc::strong_count(&sink) == 1).await;
    assert!(!rig.manager.has_live_sessions());
    let page = rig
        .manager
        .history(&chat.id, u32::MAX, None)
        .await
        .expect("a new actor answers");
    assert!(page.items.iter().any(|item| matches!(
        item.event,
        GhostEvent::TurnEnded {
            stop_reason: StopReason::Interrupted,
            recovered: true,
            ..
        }
    )));
    let _ = &rig.profile;
}
