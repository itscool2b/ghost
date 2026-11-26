//! Native run-idle can arrive after a prompt result. Exercise the smoke runner through the real
//! session actor and store, with a scripted driver and no external agent.

#![cfg(test)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::Machine;
use ghost_cli::args::{Scenario, SmokeArgs};
use ghost_cli::headless::Headless;
use ghost_cli::out::Out;
use ghost_cli::report::Outcome;
use ghost_cli::smoke::{self, SmokeTimings};
use ghost_cli::watch::Watch;
use ghost_core::session::{
    FrameSink, ManagerDeps, ManagerLimits, SessionManager, SessionTimings, SinkGone,
};
use ghost_drivers::driver::{DriverEvent, DriverExit, LoggedCommand, ReadyInfo, ResumeOutcome};
use ghost_drivers::testing::{
    CommandMatch, ScriptHandle, ScriptedDriver, ScriptedDriverFactory, Step,
};
use ghost_process::testing::FakeSpawner;
use ghost_process::{LogFactory, Redactor};
use ghost_protocol::{
    Autonomy, Capabilities, EventBatch, GhostEvent, PermissionOption, PermissionOptionKind,
    PermissionRequest, PromptState, StopReason, ToolCall, ToolKind, ToolStatus,
};

const TIMINGS: SmokeTimings = SmokeTimings {
    start: Duration::from_secs(5),
    stop: Duration::from_secs(5),
    turn: Duration::from_secs(2),
};

async fn headless(machine: &Machine, driver: ScriptedDriver) -> Headless {
    let mut headless = machine.headless(&[]).await;
    let rt = tokio::runtime::Handle::current();
    let redactor = Arc::new(Redactor::standard());
    let (spawner, _) = FakeSpawner::new();
    headless.manager = SessionManager::new(ManagerDeps {
        worktrees: None,
        checkpoints: None,
        store: Arc::clone(&headless.store),
        env: Arc::clone(&headless.env),
        spawner,
        detector: Arc::clone(&headless.detector),
        drivers: ScriptedDriverFactory::new(vec![driver]),
        logs: LogFactory::new(machine.path("logs"), Arc::clone(&redactor), rt.clone()),
        redactor,
        device_id: ghost_core::device::DeviceIdentity::load_or_create(
            &ghost_core::paths::DataRoot::at(&machine.profile_root),
        )
        .expect("fixture device")
        .identity()
        .device_id
        .clone(),
        app_version: "test".into(),
        limits: ManagerLimits::STANDARD,
        timings: SessionTimings::STANDARD,
        rt,
    });
    headless
}

fn permission_turn(id: &str, status: ToolStatus) -> Vec<Step> {
    vec![
        Step::Expect(CommandMatch::Prompt),
        Step::EmitInTurn(DriverEvent::Permission {
            turn: None,
            request: PermissionRequest {
                request_id: id.into(),
                tool_call_id: None,
                title: "Write a test file".into(),
                kind: None,
                raw_input: None,
                content: Vec::new(),
                locations: Vec::new(),
                options: vec![
                    PermissionOption {
                        option_id: "allow".into(),
                        kind: PermissionOptionKind::AllowOnce,
                        label: "Allow once".into(),
                    },
                    PermissionOption {
                        option_id: "deny".into(),
                        kind: PermissionOptionKind::RejectOnce,
                        label: "Deny".into(),
                    },
                ],
                truncated: None,
            },
        }),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some(id.into()),
        }),
        Step::EmitInTurn(DriverEvent::Event {
            turn: None,
            event: GhostEvent::ToolCall(ToolCall {
                tool_call_id: format!("tool-{id}"),
                kind: ToolKind::Edit,
                title: "Write a test file".into(),
                status,
                locations: Vec::new(),
                raw_input: None,
                tool_name: None,
                content: Vec::new(),
                truncated: None,
            }),
        }),
        Step::EndTurn {
            stop_reason: StopReason::EndTurn,
            usage: None,
            error: None,
        },
    ]
}

fn driver(second_turn: bool) -> (ScriptedDriver, ScriptHandle) {
    let mut steps = vec![
        Step::Emit(DriverEvent::RunState { idle: true }),
        Step::Emit(DriverEvent::Ready(ReadyInfo {
            native_session_id: Some("native-session".into()),
            agent_name: None,
            agent_version: None,
            capabilities: Capabilities {
                permissions: true,
                autonomy_levels: vec![Autonomy::Ask, Autonomy::FullAuto],
                ..Default::default()
            },
            resumed: ResumeOutcome::New,
            spawn_id: "fake-spawn".into(),
            replay: None,
        })),
        Step::Expect(CommandMatch::SetAutonomy),
    ];
    steps.extend(permission_turn("first", ToolStatus::Failed));
    if second_turn {
        // The test injects idle only after observing the second prompt in the actor's queue.
        steps.extend(permission_turn("second", ToolStatus::Completed));
        steps.push(Step::Emit(DriverEvent::RunState { idle: true }));
    }
    steps.extend([
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    ScriptedDriver::new(steps)
}

fn args() -> SmokeArgs {
    SmokeArgs {
        agent: "mock".into(),
        scenarios: vec![Scenario::Permission],
        model: None,
        effort: None,
    }
}

fn prompt_count(handle: &ScriptHandle) -> usize {
    handle
        .commands()
        .iter()
        .filter(|command| matches!(command, LoggedCommand::Prompt { .. }))
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_permission_smoke_skips_without_changing_mode_or_prompting() {
    let machine = Machine::new();
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::RunState { idle: true }),
        Step::Emit(DriverEvent::Ready(ReadyInfo {
            native_session_id: Some("native-no-permissions".into()),
            agent_name: None,
            agent_version: None,
            capabilities: Capabilities {
                autonomy_levels: vec![Autonomy::FullAuto],
                ..Default::default()
            },
            resumed: ResumeOutcome::New,
            spawn_id: "fake-spawn".into(),
            replay: None,
        })),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let headless = headless(&machine, driver).await;
    let folder = machine.path("repo");
    std::fs::create_dir(&folder).expect("folder");
    let (out, captured) = Out::capture();
    let result = smoke::run(&headless, &args(), &folder, TIMINGS, &out).await;
    assert_eq!(
        result.expect("smoke"),
        Outcome::Passed,
        "{}",
        captured.text()
    );
    assert!(captured.text().contains("no permission prompts were sent"));
    assert_eq!(prompt_count(&handle), 0);
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::SetAutonomy { .. }))
    );
    assert!(headless.shutdown().await.saved);
}

struct QueueObserver(tokio::sync::mpsc::UnboundedSender<()>);

impl FrameSink for QueueObserver {
    fn send(&self, batch: &EventBatch) -> Result<(), SinkGone> {
        if batch.events.iter().any(|event| {
            matches!(
                event,
                GhostEvent::PromptLifecycle {
                    state: PromptState::Queued,
                    ..
                }
            )
        }) {
            let _ = self.0.send(());
        }
        Ok(())
    }

    fn owner(&self) -> &str {
        "smoke-native-test"
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smoke_tracks_the_next_prompt_until_native_run_idle() {
    let machine = Machine::new();
    let (driver, handle) = driver(true);
    let headless = headless(&machine, driver).await;
    let folder = machine.path("repo");
    std::fs::create_dir(&folder).expect("folder");
    let (out, captured) = Out::capture();
    let args = args();
    let smoke = smoke::run(&headless, &args, &folder, TIMINGS, &out);
    let release = async {
        tokio::time::timeout(TIMINGS.start, handle.wait_for(CommandMatch::Prompt))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "first prompt: {error:?}; commands: {:?}; smoke output: {}",
                    handle.commands(),
                    captured.text()
                )
            });
        let chat = headless.manager.chats().await.expect("chats").remove(0);
        let (queued, mut queue_rx) = tokio::sync::mpsc::unbounded_channel();
        let attached = headless
            .manager
            .attach(&chat.id, Arc::new(QueueObserver(queued)))
            .await
            .expect("attach");
        assert!(
            !attached.snapshot.queue.is_empty()
                || tokio::time::timeout(TIMINGS.turn, queue_rx.recv())
                    .await
                    .expect("queued prompt deadline")
                    .is_some(),
            "second prompt was queued while native work remained busy"
        );
        assert_eq!(
            prompt_count(&handle),
            1,
            "only idle can release the next prompt"
        );
        handle.emit(DriverEvent::RunState { idle: true });
    };
    let (outcome, ()) = tokio::join!(smoke, release);
    assert_eq!(
        outcome.expect("smoke"),
        Outcome::Passed,
        "{}",
        captured.text()
    );
    assert_eq!(
        prompt_count(&handle),
        2,
        "both prompts reached the driver exactly once"
    );
    assert!(headless.shutdown().await.saved);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smoke_removes_its_queued_prompt_when_native_work_never_becomes_idle() {
    let machine = Machine::new();
    let (driver, handle) = driver(false);
    let headless = headless(&machine, driver).await;
    let folder = machine.path("repo");
    std::fs::create_dir(&folder).expect("folder");
    let (out, captured) = Out::capture();
    let outcome = smoke::run(&headless, &args(), &folder, TIMINGS, &out)
        .await
        .expect("smoke");
    assert_eq!(outcome, Outcome::Failed, "{}", captured.text());
    assert!(captured.text().contains("the prompt's turn never started"));
    assert_eq!(
        prompt_count(&handle),
        1,
        "the timed-out queued prompt never ran"
    );
    let chat = headless.manager.chats().await.expect("chats").remove(0);
    let attached = Watch::new()
        .attach(&headless.manager, &chat.id)
        .await
        .expect("attach");
    assert!(
        attached.snapshot.queue.is_empty(),
        "failed smoke leaves no prompt to run on restart"
    );
    handle.emit(DriverEvent::RunState { idle: true });
    // Force stop reaches Stopped before the retired driver's join releases its live slot.
    tokio::time::timeout(TIMINGS.stop, async {
        while headless.manager.has_live_sessions() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the force-stopped driver retires without another shutdown");
    assert_eq!(prompt_count(&handle), 1);
    assert!(!headless.manager.has_live_sessions());
    assert!(headless.shutdown().await.saved);
}
