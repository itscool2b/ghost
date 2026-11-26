//! Starts, attaches, limits, restarts and quits, through the session manager with scripted
//! drivers on paused time.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ghost_agents::{FixedAgent, Gate, LaunchError};
use ghost_core::error::CoreError;
use ghost_core::owner::ProfileOwner;
use ghost_core::session::{ManagerLimits, SessionError};
use ghost_drivers::driver::{
    DriverError, DriverEvent, DriverExit, LoggedCommand, ReplaySummary, ResumeOutcome,
};
use ghost_drivers::testing::{CommandMatch, ScriptedDriver, Step};
use ghost_process::testing::FakeCall;
use ghost_protocol::session::{Availability, FolderState};
use ghost_protocol::{
    ErrorKind, GhostEvent, NoticeLevel, PermissionDecision, ResolvedBy, SessionInfo, SessionStatus,
    StopReason, ToolCall, ToolContent, ToolKind, ToolStatus,
};
use sha2::{Digest, Sha256};
use support::*;

fn end_turn(stop_reason: StopReason) -> Step {
    Step::EndTurn {
        stop_reason,
        usage: None,
        error: None,
    }
}

fn errors(events: &[GhostEvent]) -> Vec<(ErrorKind, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            GhostEvent::Error { kind, message, .. } => Some((*kind, message.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn unavailable_hosts_survive_restart_without_starting_a_native_driver() {
    use ghost_core::chatstore::NewChat;
    use ghost_protocol::agents::AgentHost;
    let h = Harness::new(Vec::new());
    for host in [
        AgentHost::Wsl {
            distro: "Ubuntu".into(),
        },
        AgentHost::Unsupported,
    ] {
        assert!(matches!(
            h.manager
                .create_chat_on_host(MOCK, host.clone(), &h.folder)
                .await,
            Err(SessionError::AgentUnavailable(_))
        ));
        let saved = h
            .store
            .create_chat(NewChat {
                agent_id: MOCK.into(),
                agent_host: host.clone(),
                folder: h.folder.clone(),
                device_id: "device".into(),
                now: 1,
            })
            .await
            .unwrap();
        h.manager.start(&saved.id, false).await.unwrap();
        h.status(&saved.id, SessionStatus::Failed).await;
        assert!(h.factory.setups().is_empty());
        let listed = h.manager.chats().await.unwrap();
        assert_eq!(
            listed
                .iter()
                .find(|chat| chat.id == saved.id)
                .unwrap()
                .agent_host,
            host
        );
    }
    let h = h.restart(Vec::new(), Options::default()).await;
    for chat in h.manager.chats().await.unwrap() {
        assert_ne!(chat.agent_host, AgentHost::Native);
        h.manager.start(&chat.id, false).await.unwrap();
        h.status(&chat.id, SessionStatus::Failed).await;
    }
    assert!(h.factory.setups().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_saved_native_id_from_another_host_is_never_resumed_locally() {
    let h = Harness::new(Vec::new());
    let record = h
        .store
        .create_chat(ghost_core::chatstore::NewChat {
            agent_id: MOCK.into(),
            agent_host: Default::default(),
            folder: h.folder.clone(),
            device_id: "device".into(),
            now: 1,
        })
        .await
        .unwrap();
    let conn = rusqlite::Connection::open(h.profile.db_path()).unwrap();
    conn.execute(
        "INSERT INTO agent_sessions (id, chat_id, agent_id, native_session_id, cwd, created_at, updated_at, device_id, host) VALUES ('s', ?1, ?2, 'remote-id', '/work', 1, 1, 'device', ?3)",
        rusqlite::params![record.id, MOCK, r#"{"type":"wsl","distro":"Ubuntu"}"#],
    ).unwrap();
    drop(conn);
    h.manager.start(&record.id, false).await.unwrap();
    h.status(&record.id, SessionStatus::Failed).await;
    assert!(h.factory.setups().is_empty());
    let snapshot = h
        .manager
        .attach(&record.id, Sink::new("main"))
        .await
        .unwrap()
        .snapshot;
    assert!(
        snapshot
            .last_error
            .unwrap()
            .message
            .contains("different execution host")
    );
}

fn details(sink: &Sink) -> Vec<String> {
    sink.events()
        .into_iter()
        .filter_map(|(_, event)| match event {
            GhostEvent::SessionUpdated(SessionInfo {
                detail: Some(detail),
                ..
            }) => Some(detail),
            _ => None,
        })
        .collect()
}

fn with_process(spec: ghost_agents::AgentSpec) -> FixedAgent {
    FixedAgent::new(spec, ready(), Some(launch()))
}

/// P1: an agent detection finds missing fails an existing chat's start with an
/// `AgentUnavailable` card, and `create_chat` refuses it from then on.
#[tokio::test(start_paused = true)]
async fn start_fails_when_agent_missing() {
    let reason = "Hermes was not found on your PATH.";
    let options = Options {
        agents: vec![
            FixedAgent::new(mock_spec(), Availability::Checking, None).with_detected(
                Availability::Missing {
                    reason: reason.into(),
                },
            ),
        ],
        ..Options::default()
    };
    let h = Harness::with(Vec::new(), options);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Failed).await;
    assert_eq!(
        errors(&h.stored(&chat.id).await),
        [(ErrorKind::AgentUnavailable, reason.to_owned())]
    );
    let attach = h
        .manager
        .attach(&chat.id, Sink::new("main"))
        .await
        .expect("attach");
    assert_eq!(
        attach.snapshot.last_error.map(|error| error.kind),
        Some(ErrorKind::AgentUnavailable)
    );
    assert!(matches!(
        h.manager.create_chat(MOCK, &h.folder).await,
        Err(SessionError::AgentUnavailable(message)) if message == reason
    ));
}

/// A chat created while its agent reads `Checking` waits for the detection before it starts.
#[tokio::test(start_paused = true)]
async fn a_start_waits_for_detection() {
    let gate = Gate::new();
    let options = Options {
        agents: vec![
            FixedAgent::new(mock_spec(), Availability::Checking, None)
                .with_detected(ready())
                .with_detect_gate(gate.clone()),
        ],
        ..Options::default()
    };
    let (driver, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = Harness::with(vec![driver], options);
    let chat = h.chat().await;
    assert_eq!(chat.status, SessionStatus::Starting);
    gate.until_waiting(1).await;
    assert!(h.factory.setups().is_empty());
    gate.open();
    h.status(&chat.id, SessionStatus::Ready).await;
}

/// P6: the first start of an adapter shows the install's detail while it runs.
#[tokio::test(start_paused = true)]
async fn install_detail() {
    let detail = "Downloading the Claude adapter (@agentclientprotocol/claude-agent-acp 0.95.0). This happens once and can take a few minutes.";
    let gate = Gate::new();
    let spec = ghost_agents::get("claude-acp").expect("claude-acp").clone();
    let options = Options {
        agents: vec![
            with_process(spec)
                .with_download(vec![detail.into()])
                .with_launch_gate(gate.clone()),
        ],
        ..Options::default()
    };
    let (driver, _) = ScriptedDriver::new(vec![Step::Spawn(launch()), ready_step("c1")]);
    let h = Harness::with(vec![driver], options);
    assert!(h.manager.agents(false).await[0].needs_download);
    let chat = h.chat_with("claude-acp").await;
    let sink = Sink::new("main");
    let attached = h
        .manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    gate.until_waiting(1).await;
    // Creating the chat started the download, so it can be under way before the view attaches:
    // then the attach's snapshot carries the detail, otherwise a later frame does.
    let in_snapshot = attached.snapshot.session.detail.as_deref() == Some(detail);
    let probe = sink.clone();
    eventually("the download detail shows", || {
        in_snapshot || details(&probe).iter().any(|shown| shown == detail)
    })
    .await;
    assert_eq!(
        h.statuses.last(&chat.id).map(|update| update.status),
        Some(SessionStatus::Starting)
    );
    gate.open();
    h.status(&chat.id, SessionStatus::Ready).await;
    assert!(
        !h.manager.agents(false).await[0].needs_download,
        "installed"
    );
}

/// A download that fails is a start failure with the reason.
#[tokio::test(start_paused = true)]
async fn an_install_failure_fails_the_start() {
    let reason = "Could not download the Claude adapter: npm ERR! network";
    let spec = ghost_agents::get("claude-acp").expect("claude-acp").clone();
    let options = Options {
        agents: vec![
            with_process(spec)
                .with_download(vec!["Downloading".into()])
                .with_launch_error(LaunchError::InstallFailed {
                    reason: reason.into(),
                }),
        ],
        ..Options::default()
    };
    let h = Harness::with(Vec::new(), options);
    let chat = h.chat_with("claude-acp").await;
    h.status(&chat.id, SessionStatus::Failed).await;
    assert_eq!(
        errors(&h.stored(&chat.id).await),
        [(ErrorKind::SpawnFailed, reason.to_owned())]
    );
}

/// P36: a folder deleted after the chat was created fails the start with `FolderMissing`, and
/// the list says so until the folder is back (a drive plugged in again).
#[tokio::test(start_paused = true)]
async fn start_fails_when_folder_missing() {
    let (driver, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = Harness::new(vec![driver]);
    let folder = h.dir.path().join("project");
    std::fs::create_dir_all(&folder).expect("folder");
    let chat = h.manager.create_chat(MOCK, &folder).await.expect("create");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    std::fs::remove_dir_all(&folder).expect("remove the folder");
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Failed).await;
    let errors = errors(&h.stored(&chat.id).await);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, ErrorKind::FolderMissing);
    let listed = h.manager.chats().await.expect("chats");
    assert_eq!(listed[0].folder_state, FolderState::Missing);
    std::fs::create_dir_all(&folder).expect("the folder is back");
    let listed = h.manager.chats().await.expect("chats");
    assert_eq!(listed[0].folder_state, FolderState::Ok);
}

/// P37: `create_chat` refuses a relative, missing, non-folder or non-Unicode folder, and an
/// unknown agent.
#[tokio::test(start_paused = true)]
async fn create_chat_rejects_bad_folders() {
    let h = Harness::new(Vec::new());
    let file = h.dir.path().join("notes.txt");
    std::fs::write(&file, "hi").expect("file");
    let cases: Vec<(std::path::PathBuf, &str)> = vec![
        (
            Path::new("relative/folder").to_path_buf(),
            "it isn't an absolute path.",
        ),
        (h.dir.path().join("missing"), "it doesn't exist."),
        (file, "it isn't a folder."),
    ];
    for (folder, reason) in cases {
        match h.manager.create_chat(MOCK, &folder).await {
            Err(SessionError::InvalidFolder { reason: got, path }) => {
                assert_eq!(got, reason, "{}", folder.display());
                assert_eq!(path, folder.display().to_string());
            }
            other => panic!(
                "{}: expected InvalidFolder, got {other:?}",
                folder.display()
            ),
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = [h.dir.path().as_os_str().as_bytes(), b"/caf\xe9"].concat();
        let weird = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&bytes));
        assert!(matches!(
            h.manager.create_chat(MOCK, &weird).await,
            Err(SessionError::InvalidFolder { reason, .. }) if reason == "its name isn't valid Unicode."
        ));
    }
    assert!(matches!(
        h.manager.create_chat("nobody", &h.folder).await,
        Err(SessionError::UnknownAgent(agent)) if agent == "nobody"
    ));
    assert!(h.manager.chats().await.expect("chats").is_empty());
}

/// P48: a folder the OS refuses fails the start with `FolderNotAllowed` and the System Settings
/// hint, never as missing; the list says "not allowed".
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn folder_permission_denied_is_not_missing() {
    use std::os::unix::fs::PermissionsExt;
    let (driver, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = Harness::new(vec![driver]);
    let folder = h.dir.path().join("private");
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(folder.join("file"), "x").expect("file");
    let chat = h.manager.create_chat(MOCK, &folder).await.expect("create");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    if std::fs::read_dir(&folder).is_ok() {
        // Running as root: permissions do not apply.
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        return;
    }
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Failed).await;
    let errors = errors(&h.stored(&chat.id).await);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).expect("chmod back");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, ErrorKind::FolderNotAllowed);
    assert!(
        errors[0]
            .1
            .contains("System Settings > Privacy & Security > Files and Folders"),
        "{}",
        errors[0].1
    );
    let listed = h.manager.chats().await.expect("chats");
    assert_eq!(listed[0].folder_state, FolderState::NotAllowed);
}

/// P38, as far as fixed agents reach: refreshing the agents (a re-probe) while one runs sends the
/// running agent nothing and leaves its process alone. Nothing in the manager reacts to detection,
/// and a fixed agent cannot change its answer mid-test, so this cannot fail today: the case itself,
/// an agent updated or removed while running, stays open for phase 2 with real detection
/// (`agents_refresh` while a session runs, its binary removed or upgraded).
#[tokio::test(start_paused = true)]
async fn refresh_keeps_live_sessions() {
    let options = Options {
        agents: vec![with_process(mock_spec())],
        ..Options::default()
    };
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::with(vec![driver], options);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let entries = h.manager.agents(true).await;
    assert_eq!(entries.len(), 1);
    assert!(
        handle.commands().is_empty(),
        "nothing reached the running agent"
    );
    assert!(
        h.spawner.controls()[0].calls().is_empty(),
        "its process was not touched"
    );
    h.manager
        .send(&chat.id, "still works".into())
        .await
        .expect("send");
    command(&handle, CommandMatch::Prompt).await;
    h.status(&chat.id, SessionStatus::Ready).await;
}

/// P42: an adapter whose files are damaged forgets its install; the card says Retry downloads it
/// again, and Retry does.
#[tokio::test(start_paused = true)]
async fn missing_module_forgets_install() {
    let spec = ghost_agents::get("claude-acp").expect("claude-acp").clone();
    let options = Options {
        agents: vec![
            with_process(spec).with_download(vec!["Downloading the Claude adapter".into()]),
        ],
        ..Options::default()
    };
    let (first, _) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        Step::Exit(DriverExit::StartFailed(DriverError {
            kind: ErrorKind::AgentExited,
            message: "The agent stopped before it answered.".into(),
            detail: Some("node:internal/modules/cjs/loader:1228\n  throw err;\nError: Cannot find module '@anthropic-ai/claude-agent-sdk'".into()),
        })),
    ]);
    let (second, _) = ScriptedDriver::new(vec![Step::Spawn(launch()), ready_step("c1")]);
    let h = Harness::with(vec![first, second], options);
    let chat = h.chat_with("claude-acp").await;
    h.status(&chat.id, SessionStatus::Failed).await;
    assert_eq!(h.detector.forgotten(), ["claude-acp"]);
    assert_eq!(
        errors(&h.stored(&chat.id).await),
        [(
            ErrorKind::AgentExited,
            "The Claude Code adapter's files are damaged. Press Retry to download it again."
                .to_owned()
        )]
    );
    assert!(
        h.manager.agents(false).await[0].needs_download,
        "the install marker is gone"
    );
    h.manager.start(&chat.id, false).await.expect("retry");
    h.status(&chat.id, SessionStatus::Ready).await;
    assert!(
        !h.manager.agents(false).await[0].needs_download,
        "Retry installed it again"
    );
    let controls = h.spawner.controls();
    assert!(
        controls[0]
            .calls()
            .contains(&FakeCall::Shutdown(ghost_process::ShutdownPlan::FORCE))
            || controls[0].was_killed()
    );
}

/// A23 and S42: Start fresh opens a new native session with a notice; the old session row stays.
#[tokio::test(start_paused = true)]
async fn start_fresh() {
    let (first, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let (second, _) = ScriptedDriver::new(vec![ready_step("n2")]);
    let h = Harness::new(vec![first, second]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.start(&chat.id, true).await.expect("start fresh");
    let probe = Arc::clone(&h.factory);
    eventually("a second driver starts", || probe.setups().len() == 2).await;
    h.status(&chat.id, SessionStatus::Ready).await;
    assert_eq!(h.factory.setups()[1].resume, None, "no resume");
    let stored = h.stored(&chat.id).await;
    assert!(stored.iter().any(|event| matches!(
        event,
        GhostEvent::Notice { level: NoticeLevel::Info, message } if message == "Started a new session. The agent doesn't remember the conversation above."
    )));
    let conn = rusqlite::Connection::open(h.profile.db_path()).expect("raw");
    let sessions: Vec<String> = conn
        .prepare(
            "SELECT native_session_id FROM agent_sessions WHERE chat_id = ?1 ORDER BY created_at",
        )
        .expect("prepare")
        .query_map([&chat.id], |row| row.get(0))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect();
    assert_eq!(sessions, ["n1", "n2"]);
}

/// A34: a force stop of an agent whose turns outlive its process waits up to 5 s for the cancel
/// to land, applying events meanwhile, and says the gateway may still finish.
#[tokio::test(start_paused = true)]
async fn force_stop_waits_for_daemon_cancel() {
    let hermes = ghost_agents::get("hermes").expect("hermes").clone();
    let options = Options {
        agents: vec![with_process(hermes.clone())],
        ..Options::default()
    };
    let (lands, lands_handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("h1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Interrupt),
        Step::Wait(Duration::from_secs(3)),
        in_turn(message("m1", "cancelled in the gateway")),
        end_turn(StopReason::Cancelled),
        Step::Hang,
    ]);
    let (silent, silent_handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("h1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let h = Harness::with(vec![lands, silent], options);
    let chat = h.chat_with("hermes").await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "work".into()).await.expect("send");
    // Running also covers workspace/memory preparation, before a turn reaches the driver.
    command(&lands_handle, CommandMatch::Prompt).await;
    h.status(&chat.id, SessionStatus::Running).await;
    h.manager.force_stop(&chat.id).await.expect("force stop");
    command(&lands_handle, CommandMatch::Interrupt).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_ne!(
        h.statuses.last(&chat.id).map(|update| update.status),
        Some(SessionStatus::Stopped),
        "not after 200 ms"
    );
    h.status(&chat.id, SessionStatus::Stopped).await;
    let stored = h.stored(&chat.id).await;
    assert!(stored.iter().any(|event| matches!(event, GhostEvent::MessageDelta { text, .. } if text == "cancelled in the gateway")));
    assert!(matches!(
        stored
            .iter()
            .rev()
            .find(|event| matches!(event, GhostEvent::TurnEnded { .. })),
        Some(GhostEvent::TurnEnded {
            stop_reason: StopReason::Cancelled,
            ..
        })
    ));
    let control = &h.spawner.controls()[0];
    eventually("the viewer is killed", || {
        control
            .calls()
            .contains(&FakeCall::Shutdown(ghost_process::ShutdownPlan::FORCE))
    })
    .await;

    // A cancel that never lands: killed after 5 s, with the gateway notice.
    h.manager.send(&chat.id, "more".into()).await.expect("send");
    // Allow retirement/startup timers to advance before the external-writer handshake.
    h.status(&chat.id, SessionStatus::Running).await;
    command(&silent_handle, CommandMatch::Prompt).await;
    let started = tokio::time::Instant::now();
    h.manager.force_stop(&chat.id).await.expect("force stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    assert!(
        started.elapsed() >= Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let stored = h.stored(&chat.id).await;
    assert!(stored.iter().any(|event| matches!(
        event,
        GhostEvent::Notice { message, .. } if message.starts_with("Hermes may still finish this turn in its gateway")
    )));
}

fn reply_hash(text: &str) -> String {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    Sha256::digest(compact.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn resumed(native: &str, replay: ReplaySummary) -> Step {
    let mut info = ready_info(native);
    info.resumed = ResumeOutcome::Resumed;
    info.replay = Some(replay);
    Step::Emit(DriverEvent::Ready(info))
}

/// A40: at `Ready`, a Hermes replay whose newest reply differs from what was stored (or that
/// counts more turns) adds a notice and that reply outside any turn; a matching replay adds
/// nothing, after a stop or a restart.
#[tokio::test(start_paused = true)]
async fn replay_summary_compared_at_ready() {
    let answer = "Here is   the answer.\n";
    let (first, _) = ScriptedDriver::new(vec![
        ready_step("h1"),
        Step::Expect(CommandMatch::Prompt),
        in_turn(message("m1", "Here is the ")),
        in_turn(message("m1", "answer.")),
        end_turn(StopReason::EndTurn),
    ]);
    let matching = ReplaySummary {
        user_turns: 1,
        last_reply: Some(answer.into()),
        last_reply_sha256: Some(reply_hash(answer)),
    };
    let (second, _) = ScriptedDriver::new(vec![resumed("h1", matching.clone())]);
    let h = Harness::new(vec![first, second]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager
        .send(&chat.id, "question".into())
        .await
        .expect("send");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Ready).await;
    let before = h.stored(&chat.id).await.len();

    // After a restart the comparison reads what was stored.
    let newer = ReplaySummary {
        user_turns: 2,
        last_reply: Some("A newer reply the gateway finished.".into()),
        last_reply_sha256: Some(reply_hash("A newer reply the gateway finished.")),
    };
    let (third, _) = ScriptedDriver::new(vec![resumed("h1", matching)]);
    let (fourth, _) = ScriptedDriver::new(vec![resumed("h1", newer)]);
    let h = h.restart(vec![third, fourth], Options::default()).await;
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Ready).await;
    let after_matching = h.stored(&chat.id).await;
    assert_eq!(
        after_matching.len(),
        before,
        "a matching replay adds nothing"
    );
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Ready).await;
    let history = h.history(&chat.id).await;
    let tail: Vec<_> = history.iter().rev().take(3).rev().collect();
    assert!(
        tail.iter().all(|item| item.turn_id.is_none()),
        "outside any turn"
    );
    assert!(
        matches!(&tail[0].event, GhostEvent::Notice { message, .. } if message.starts_with("Hermes continued this session") || message.contains("continued this session while Ghost was away"))
    );
    assert_eq!(
        tail[1].event,
        message("spawn-1.r1", "A newer reply the gateway finished.")
    );
    assert_eq!(
        tail[2].event,
        GhostEvent::MessageCompleted {
            message_id: "spawn-1.r1".into()
        }
    );
}

fn two_agents() -> Options {
    Options {
        limits: ManagerLimits {
            max_live_agents: 2,
            ..ManagerLimits::STANDARD
        },
        ..Options::default()
    }
}

/// S20: at the live-agent limit, a start stops the agent idle longest (ready, unwatched, nothing
/// open or queued) and takes its slot once its process is gone; the others keep running.
#[tokio::test(start_paused = true)]
async fn a_start_at_the_limit_stops_the_longest_idle_agent() {
    let (a, a_handle) = ScriptedDriver::new(vec![
        ready_step("na"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let (b, b_handle) = ScriptedDriver::new(vec![ready_step("nb")]);
    let (c, _) = ScriptedDriver::new(vec![ready_step("nc")]);
    let h = Harness::with(vec![a, b, c], two_agents());
    let a = h.chat().await;
    h.status(&a.id, SessionStatus::Ready).await;
    tokio::time::advance(Duration::from_secs(60)).await;
    let b = h.chat().await;
    h.status(&b.id, SessionStatus::Ready).await;
    tokio::time::advance(Duration::from_secs(60)).await;
    let c = h.chat().await;
    h.status(&c.id, SessionStatus::Ready).await;
    h.status(&a.id, SessionStatus::Stopped).await;
    assert_eq!(a_handle.commands(), [LoggedCommand::Shutdown]);
    assert!(
        b_handle.commands().is_empty(),
        "the newer idle agent runs on"
    );
    assert_eq!(
        h.statuses.last(&b.id).map(|update| update.status),
        Some(SessionStatus::Ready)
    );
    assert!(
        errors(&h.stored(&c.id).await).is_empty(),
        "the start made room instead of failing"
    );
    // An explicit start makes room the same way.
    let (d, _) = ScriptedDriver::new(vec![ready_step("nd")]);
    h.factory.push(d);
    let d = h
        .manager
        .create_chat(MOCK, &h.folder)
        .await
        .expect("create");
    h.status(&d.id, SessionStatus::Ready).await;
    h.status(&b.id, SessionStatus::Stopped).await;
    assert_eq!(b_handle.commands(), [LoggedCommand::Shutdown]);
}

/// S20: a start is refused only when no agent is idle, saying what the others are doing; one turn
/// ending is enough to make room again.
#[tokio::test(start_paused = true)]
async fn a_start_is_refused_only_when_every_agent_is_busy() {
    let (working, working_handle) = ScriptedDriver::new(vec![
        ready_step("working"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Any),
        end_turn(StopReason::EndTurn),
    ]);
    let (asking, asking_handle) = ScriptedDriver::new(vec![
        ready_step("asking"),
        Step::Expect(CommandMatch::Prompt),
        Step::EmitInTurn(DriverEvent::Permission {
            turn: None,
            request: permission("write", Some(ToolKind::Edit)),
        }),
    ]);
    let (third, _) = ScriptedDriver::new(vec![ready_step("third")]);
    let h = Harness::with(vec![working, asking, third], two_agents());
    let a = h.chat().await;
    let b = h.chat().await;
    h.status(&a.id, SessionStatus::Ready).await;
    h.status(&b.id, SessionStatus::Ready).await;
    h.manager.send(&a.id, "work".into()).await.expect("send");
    prompt_turn(&working_handle).await;
    h.manager.send(&b.id, "ask".into()).await.expect("send");
    prompt_turn(&asking_handle).await;
    h.status(&a.id, SessionStatus::Running).await;
    eventually("b asks", || {
        h.statuses
            .last(&b.id)
            .is_some_and(|update| update.needs_attention)
    })
    .await;
    let c = h.chat().await;
    h.status(&c.id, SessionStatus::Failed).await;
    let refusal = "2 agents are running and none is idle: 1 working and 1 waiting for your answer. Stop one, then try again.";
    assert_eq!(
        errors(&h.stored(&c.id).await),
        [(ErrorKind::SpawnFailed, refusal.to_owned())]
    );
    match h.manager.start(&c.id, false).await {
        Err(SessionError::TooManyAgents { limit: 2, message }) => assert_eq!(message, refusal),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(
        h.statuses.last(&a.id).map(|update| update.status),
        Some(SessionStatus::Running),
        "a working agent is never stopped for room"
    );
    // The turn ends: that agent is idle, so the next start takes its slot.
    h.manager.interrupt(&a.id).await.expect("interrupt");
    command(&working_handle, CommandMatch::Interrupt).await;
    h.status(&a.id, SessionStatus::Ready).await;
    h.manager.start(&c.id, false).await.expect("room again");
    h.status(&c.id, SessionStatus::Ready).await;
    h.status(&a.id, SessionStatus::Stopped).await;
    assert!(
        h.statuses
            .last(&b.id)
            .is_some_and(|update| update.needs_attention && update.status != SessionStatus::Stopped),
        "the agent waiting for an answer keeps waiting"
    );
}

/// S20: Esc on a start that waits for room ends the wait: the chat never starts later on its own
/// when the slot frees, and a start after it runs one agent, never a second one beside it.
#[tokio::test(start_paused = true)]
async fn a_start_stopped_while_it_waits_for_room_never_starts_on_its_own() {
    // The idle agent asked to make room takes a while to go.
    let (a, a_handle) = ScriptedDriver::new(vec![
        ready_step("na"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Wait(Duration::from_secs(10)),
        Step::Exit(DriverExit::Closed),
    ]);
    let (b, _) = ScriptedDriver::new(vec![ready_step("nb")]);
    let one = Options {
        limits: ManagerLimits {
            max_live_agents: 1,
            ..ManagerLimits::STANDARD
        },
        ..Options::default()
    };
    let h = Harness::with(vec![a, b], one);
    let a = h.chat().await;
    h.status(&a.id, SessionStatus::Ready).await;
    let b = h.chat().await;
    command(&a_handle, CommandMatch::Shutdown).await;
    h.status(&b.id, SessionStatus::Starting).await;
    assert_eq!(h.factory.setups().len(), 1, "b waits for room");
    // Esc while it waits.
    h.manager.interrupt(&b.id).await.expect("interrupt");
    h.status(&b.id, SessionStatus::Stopped).await;
    // The slot frees, and the wait would have run out long since: b stays as the person left it.
    h.status(&a.id, SessionStatus::Stopped).await;
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert_eq!(h.factory.setups().len(), 1, "b never started on its own");
    assert_eq!(
        h.statuses.last(&b.id).map(|update| update.status),
        Some(SessionStatus::Stopped)
    );
    assert!(errors(&h.stored(&b.id).await).is_empty(), "no late failure");
    // Started again, it runs one agent, and nothing starts a second one later.
    h.manager.start(&b.id, false).await.expect("start");
    h.status(&b.id, SessionStatus::Ready).await;
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert_eq!(h.factory.setups().len(), 2);
    assert_eq!(
        h.statuses.last(&b.id).map(|update| update.status),
        Some(SessionStatus::Ready)
    );
}

/// S20: a chat stopped to make room keeps its held messages and continues its own session on the
/// next message.
#[tokio::test(start_paused = true)]
async fn an_evicted_chat_resumes_its_session_on_the_next_message() {
    let (a1, _) = ScriptedDriver::new(vec![ready_step("na")]);
    let (b1, _) = ScriptedDriver::new(vec![ready_step("nb")]);
    let (c1, _) = ScriptedDriver::new(vec![ready_step("nc")]);
    let mut resumed_info = ready_info("na");
    resumed_info.resumed = ResumeOutcome::Resumed;
    let (a2, a2_handle) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Ready(resumed_info)),
        Step::Expect(CommandMatch::Prompt),
        in_turn(message("m1", "Back again.")),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::with(vec![a1, b1, c1, a2], two_agents());
    let a = h.chat().await;
    h.status(&a.id, SessionStatus::Ready).await;
    tokio::time::advance(Duration::from_secs(60)).await;
    let b = h.chat().await;
    h.status(&b.id, SessionStatus::Ready).await;
    tokio::time::advance(Duration::from_secs(60)).await;
    let c = h.chat().await;
    h.status(&c.id, SessionStatus::Ready).await;
    h.status(&a.id, SessionStatus::Stopped).await;
    // The next message starts it again on its own session, making room the same way.
    h.manager.send(&a.id, "again".into()).await.expect("send");
    let turn = prompt_turn(&a2_handle).await;
    assert!(!turn.is_empty());
    h.status(&a.id, SessionStatus::Ready).await;
    h.status(&b.id, SessionStatus::Stopped).await;
    let setups = h.factory.setups();
    assert_eq!(setups.len(), 4);
    assert_eq!(setups[0].resume, None);
    assert_eq!(
        setups[3].resume.as_deref(),
        Some("na"),
        "the evicted chat continues its session"
    );
    let stored = h.stored(&a.id).await;
    assert!(stored.contains(&message("m1", "Back again.")));
    assert!(errors(&stored).is_empty());
}

/// S28: one native session is never open in two chats: refused for the id an agent returns at
/// `Ready`, and for a resume id before spawning.
#[tokio::test(start_paused = true)]
async fn one_process_per_native_session() {
    let (a1, _) = ScriptedDriver::new(vec![ready_step("shared")]);
    let (b1, b1_handle) = ScriptedDriver::new(vec![
        ready_step("shared"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let h = Harness::new(vec![a1, b1]);
    let a = h.chat().await;
    h.status(&a.id, SessionStatus::Ready).await;
    let b = h.chat().await;
    h.status(&b.id, SessionStatus::Failed).await;
    assert_eq!(
        b1_handle.commands(),
        [LoggedCommand::Shutdown],
        "the second process is stopped"
    );
    assert_eq!(
        errors(&h.stored(&b.id).await),
        [(
            ErrorKind::Internal,
            "Something went wrong inside Ghost: this session is already open in another chat"
                .to_owned()
        )]
    );
    // Stop the first chat, let a third chat take the session, then resume the first: refused
    // before anything spawns.
    h.manager.stop(&a.id).await.expect("stop");
    h.status(&a.id, SessionStatus::Stopped).await;
    let (c1, _) = ScriptedDriver::new(vec![ready_step("shared")]);
    h.factory.push(c1);
    let c = h.chat().await;
    h.status(&c.id, SessionStatus::Ready).await;
    let started = h.factory.setups().len();
    h.manager.start(&a.id, false).await.expect("start");
    h.status(&a.id, SessionStatus::Failed).await;
    assert_eq!(
        h.factory.setups().len(),
        started,
        "no driver started for the conflicting resume"
    );
}

/// S28 and S20 after a force stop: the chat reads `Stopped` at once, but it keeps its native
/// session and its live slot until its process is gone and its wedged driver was aborted (5 s), so
/// another chat cannot take the slot meanwhile and an immediate Retry waits instead of opening the
/// same session in a second process.
#[tokio::test(start_paused = true)]
async fn a_force_stopped_agent_holds_its_session_until_its_process_is_gone() {
    let options = Options {
        agents: vec![with_process(mock_spec())],
        limits: ManagerLimits {
            max_live_agents: 1,
            ..ManagerLimits::STANDARD
        },
        ..Options::default()
    };
    let (wedged, wedged_handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let (resumed, _) = ScriptedDriver::new(vec![Step::Spawn(launch()), ready_step("n1")]);
    let h = Harness::with(vec![wedged, resumed], options);
    let a = h.chat().await;
    h.status(&a.id, SessionStatus::Ready).await;
    h.manager.send(&a.id, "go".into()).await.expect("send");
    command(&wedged_handle, CommandMatch::Prompt).await;
    h.status(&a.id, SessionStatus::Running).await;
    h.manager.force_stop(&a.id).await.expect("force stop");
    h.status(&a.id, SessionStatus::Stopped).await;
    let stopped = tokio::time::Instant::now();
    assert!(
        h.manager.has_live_sessions(),
        "its process is still being killed"
    );
    let b = h.chat().await;
    h.status(&b.id, SessionStatus::Failed).await;
    assert_eq!(
        errors(&h.stored(&b.id).await),
        [(
            ErrorKind::SpawnFailed,
            "1 agent is running and it is not idle: 1 stopping. Stop it, then try again."
                .to_owned()
        )],
        "the force-stopped agent still holds the only slot"
    );
    h.manager.start(&a.id, false).await.expect("Retry at once");
    let probe = Arc::clone(&h.factory);
    eventually("the Retry's driver starts", || probe.setups().len() == 2).await;
    assert!(
        stopped.elapsed() >= Duration::from_secs(4),
        "the Retry waited for the wedged driver's abort: {:?}",
        stopped.elapsed()
    );
    assert!(
        h.spawner.controls()[0].was_killed(),
        "the old process was gone before the new driver started"
    );
    h.status(&a.id, SessionStatus::Ready).await;
    assert_eq!(h.factory.setups()[1].resume.as_deref(), Some("n1"));
}

/// S35: a second process cannot own the profile; the first one's chats keep working.
#[tokio::test(start_paused = true)]
async fn second_owner_is_refused() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        in_turn(message("m1", "still here")),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    assert!(matches!(
        ProfileOwner::acquire(&h.profile),
        Err(CoreError::ProfileInUse { .. })
    ));
    h.manager
        .send(&chat.id, "hello".into())
        .await
        .expect("send");
    h.status(&chat.id, SessionStatus::Ready).await;
    let stored = h.stored(&chat.id).await;
    assert!(stored.contains(&message("m1", "still here")));
}

/// S7: attaching at any point of a turn, including just before it ends and compacts, the page
/// plus the frames rebuild the whole transcript with no gap and no duplicate.
#[tokio::test(start_paused = true)]
async fn attach_has_no_gap_or_duplicate() {
    let mut steps = vec![ready_step("n1"), Step::Expect(CommandMatch::Prompt)];
    for i in 0..60u64 {
        steps.push(in_turn(message("m1", &format!("chunk {i} "))));
        if i % 7 == 3 {
            steps.push(in_turn(GhostEvent::ToolCall(ToolCall {
                tool_call_id: format!("tool-{i}"),
                kind: ToolKind::Read,
                title: format!("Read {i}"),
                status: ToolStatus::Completed,
                locations: Vec::new(),
                raw_input: None,
                tool_name: None,
                content: Vec::new(),
                truncated: None,
            })));
        }
        steps.push(Step::Wait(Duration::from_millis(7 + (i * 13) % 31)));
    }
    steps.push(end_turn(StopReason::EndTurn));
    let (driver, _) = ScriptedDriver::new(steps);
    let options = Options {
        limits: ManagerLimits {
            subscribers_per_chat: 16,
            ..ManagerLimits::STANDARD
        },
        ..Options::default()
    };
    let h = Harness::with(vec![driver], options);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager
        .send(&chat.id, "stream".into())
        .await
        .expect("send");
    let mut views = Vec::new();
    let mut state = 0x1234_5678_u64;
    for i in 0..12 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        tokio::time::sleep(Duration::from_millis(state % 180)).await;
        let sink = Sink::new(&format!("view-{i}"));
        let attach = h
            .manager
            .attach(&chat.id, sink.clone())
            .await
            .expect("attach");
        views.push((sink, attach));
    }
    h.status(&chat.id, SessionStatus::Ready).await;
    let last = Sink::new("after");
    let after = h
        .manager
        .attach(&chat.id, last.clone())
        .await
        .expect("attach after the end");
    views.push((last, after));
    let expected: String = (0..60).map(|i| format!("chunk {i} ")).collect();
    for (sink, attach) in views {
        let page_seqs: Vec<u32> = attach.history.items.iter().map(|item| item.seq).collect();
        assert!(
            page_seqs.iter().all(|seq| *seq < attach.next_seq),
            "the page ends below next_seq"
        );
        let frames = sink.events();
        if let Some((first, _)) = frames.first() {
            assert_eq!(*first, attach.next_seq, "frames start at next_seq");
        }
        assert!(
            frames.windows(2).all(|pair| pair[1].0 == pair[0].0 + 1),
            "no gap between frames"
        );
        let mut text = String::new();
        let mut tools = std::collections::BTreeSet::new();
        for event in attach
            .history
            .items
            .iter()
            .map(|item| &item.event)
            .chain(frames.iter().map(|(_, event)| event))
        {
            match event {
                GhostEvent::MessageDelta {
                    message_id,
                    text: piece,
                } if message_id == "m1" => text.push_str(piece),
                GhostEvent::ToolCall(call) => {
                    assert!(
                        tools.insert(call.tool_call_id.clone()),
                        "a duplicate tool call"
                    );
                }
                _ => {}
            }
        }
        assert_eq!(text, expected, "the whole transcript, once");
        assert_eq!(tools.len(), 9);
    }
}

/// S10: a sink whose receiver is gone is removed.
#[tokio::test(start_paused = true)]
async fn failed_sink_is_removed() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        in_turn(message("m1", "a")),
        Step::Wait(Duration::from_secs(1)),
        in_turn(message("m1", "b")),
        end_turn(StopReason::EndTurn),
    ]);
    let options = Options {
        limits: ManagerLimits {
            subscribers_per_chat: 1,
            ..ManagerLimits::STANDARD
        },
        ..Options::default()
    };
    let h = Harness::with(vec![driver], options);
    let chat = h.chat().await;
    let gone = Sink::new("window-1");
    h.manager
        .attach(&chat.id, gone.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    assert!(matches!(
        h.manager.attach(&chat.id, Sink::new("window-2")).await,
        Err(SessionError::TooManySubscriptions)
    ));
    gone.set_gone();
    h.manager.send(&chat.id, "go".into()).await.expect("send");
    h.status(&chat.id, SessionStatus::Ready).await;
    let fresh = Sink::new("window-2");
    h.manager
        .attach(&chat.id, fresh)
        .await
        .expect("the failed sink freed its place");
}

/// U15: a new attach from the same webview replaces its older subscription to the chat; the
/// limits count views per chat and per webview.
#[tokio::test(start_paused = true)]
async fn attach_replaces_same_owner() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        in_turn(message("m1", "hello")),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let old = Sink::new("main");
    h.manager
        .attach(&chat.id, old.clone())
        .await
        .expect("attach");
    let new = Sink::new("main");
    h.manager
        .attach(&chat.id, new.clone())
        .await
        .expect("re-attach");
    let frames_before = old.frames().len();
    h.manager.send(&chat.id, "hi".into()).await.expect("send");
    new.wait_for("the turn ends", |event| {
        matches!(event, GhostEvent::TurnEnded { .. })
    })
    .await;
    assert_eq!(
        old.frames().len(),
        frames_before,
        "the replaced view gets nothing more"
    );
    // Four views per chat.
    for owner in ["a", "b", "c"] {
        h.manager
            .attach(&chat.id, Sink::new(owner))
            .await
            .expect("attach");
    }
    assert!(matches!(
        h.manager.attach(&chat.id, Sink::new("e")).await,
        Err(SessionError::TooManySubscriptions)
    ));
    // A page reload drops the webview's subscriptions.
    h.manager.drop_owner("a");
    h.manager
        .attach(&chat.id, Sink::new("e"))
        .await
        .expect("room after a page reload");
}

/// S43: rows written by a newer build load leniently: unknown enum names read as their fallback,
/// an unparsable `turn_ended` still ends its turn, and other unknown rows are skipped and counted.
#[tokio::test(start_paused = true)]
async fn newer_rows_load_leniently() {
    let h = Harness::new(Vec::new());
    let record = h
        .store
        .create_chat(ghost_core::chatstore::NewChat {
            agent_host: Default::default(),
            agent_id: MOCK.into(),
            folder: h.folder.clone(),
            device_id: "device".into(),
            now: 1,
        })
        .await
        .expect("chat");
    let conn = rusqlite::Connection::open(h.profile.db_path()).expect("raw");
    conn.busy_timeout(Duration::from_secs(5)).expect("busy");
    conn.execute(
        "INSERT INTO turns (id, chat_id, seq, status, started_at, device_id, first_event_seq, last_event_seq, compacted_at) \
         VALUES ('t1', ?1, 1, 'completed', 1, 'd', 0, 3, 5)",
        [&record.id],
    )
    .expect("turn");
    let rows = [
        (
            0,
            "turn_started",
            r#"{"type":"turn_started","turn_id":"t1","prompt_id":null}"#,
        ),
        (
            1,
            "notice",
            r#"{"type":"notice","level":"critical","message":"from the future","shiny":true}"#,
        ),
        (2, "hologram", r#"{"type":"hologram","depth":9}"#),
        (
            3,
            "turn_ended",
            r#"{"type":"turn_ended","turn_id":"t1","stop_reason":"teleported","usage":{"tokens":"many"}}"#,
        ),
    ];
    for (seq, kind, payload) in rows {
        conn.execute(
            "INSERT INTO events (id, chat_id, turn_id, seq, kind, payload, created_at, device_id) VALUES (?1, ?2, 't1', ?3, ?4, ?5, 1, 'd')",
            rusqlite::params![format!("e{seq}"), record.id, seq, kind, payload],
        )
        .expect("row");
    }
    drop(conn);
    let page = h
        .manager
        .history(&record.id, u32::MAX, None)
        .await
        .expect("history");
    assert_eq!(page.skipped, 1);
    let events: Vec<GhostEvent> = page.items.into_iter().map(|item| item.event).collect();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[1],
        GhostEvent::Notice {
            level: NoticeLevel::Info,
            message: "from the future".into()
        }
    );
    assert_eq!(
        events[2],
        GhostEvent::TurnEnded {
            turn_id: "t1".into(),
            stop_reason: StopReason::Other,
            usage: None,
            duration_ms: None,
            recovered: false
        }
    );
}

/// S24: quitting with running turns cancels, stops gracefully within the deadline, ends the turns
/// interrupted and hands everything to the store; a wedged chat is killed at the deadline.
#[tokio::test(start_paused = true)]
async fn shutdown_all_bounded() {
    let options = Options {
        agents: vec![with_process(mock_spec())],
        ..Options::default()
    };
    let (polite, polite_handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        in_turn(message("m1", "half")),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let (wedged, wedged_handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("n2"),
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let h = Harness::with(vec![polite, wedged], options);
    let a = h.chat().await;
    h.status(&a.id, SessionStatus::Ready).await;
    let b = h.chat().await;
    h.status(&b.id, SessionStatus::Ready).await;
    h.manager.send(&a.id, "one".into()).await.expect("send");
    h.manager.send(&b.id, "two".into()).await.expect("send");
    command(&polite_handle, CommandMatch::Prompt).await;
    command(&wedged_handle, CommandMatch::Prompt).await;
    h.status(&a.id, SessionStatus::Running).await;
    h.status(&b.id, SessionStatus::Running).await;
    let started = tokio::time::Instant::now();
    let summary = h.manager.shutdown_all(Duration::from_secs(8)).await;
    assert!(
        started.elapsed() <= Duration::from_secs(9),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        (summary.sessions, summary.graceful, summary.forced),
        (2, 1, 1),
        "{summary:?}"
    );
    let controls = h.spawner.controls();
    assert!(
        controls[1].calls().contains(&FakeCall::KillTree),
        "the wedged agent is killed"
    );
    assert!(matches!(
        h.manager.send(&a.id, "late".into()).await,
        Err(SessionError::ShuttingDown)
    ));
    assert!(matches!(
        h.manager.create_chat(MOCK, &h.folder).await,
        Err(SessionError::ShuttingDown)
    ));
    // A shut-down manager refuses all new store work, reads included, so check what the shutdown
    // saved by starting again on the same profile.
    let h = h.restart(Vec::new(), Options::default()).await;
    let history = h.history(&a.id).await;
    assert!(matches!(
        history.last().map(|item| &item.event),
        Some(GhostEvent::TurnEnded {
            stop_reason: StopReason::Interrupted,
            recovered: false,
            ..
        })
    ));
}

/// S1 and S37 through a crash: a turn left running and a card open outside a turn are recovered
/// at the next start: the turn ends interrupted and recovered, the card closes `System`, the
/// partial answer stays, and queued prompts come back held.
#[tokio::test(start_paused = true)]
async fn recovery_after_a_crash() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Emit(DriverEvent::Permission {
            turn: None,
            request: permission("bg", Some(ToolKind::Other)),
        }),
        Step::Expect(CommandMatch::Prompt),
        in_turn(message("m1", "Half an answer")),
        Step::Hang,
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager
        .send(&chat.id, "question".into())
        .await
        .expect("send");
    h.manager
        .send(&chat.id, "queued".into())
        .await
        .expect("send");
    sink.wait_for(
        "the partial answer",
        |event| matches!(event, GhostEvent::MessageDelta { message_id, .. } if message_id == "m1"),
    )
    .await;
    // Let the batch reach the store.
    tokio::time::sleep(Duration::from_secs(1)).await;
    h.history(&chat.id).await;
    let h = h.crash_and_restart(Vec::new(), Options::default()).await;
    let report = h.store.recovered().await;
    assert_eq!(report.interrupted_turns, 1);
    assert!(report.resolved_requests >= 1);
    let history = h.history(&chat.id).await;
    let events: Vec<&GhostEvent> = history.iter().map(|item| &item.event).collect();
    assert!(
        events.contains(&&message("m1", "Half an answer")),
        "the partial answer stays"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        GhostEvent::TurnEnded {
            stop_reason: StopReason::Interrupted,
            recovered: true,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        GhostEvent::PermissionResolved { request_id, decision: PermissionDecision::Cancelled, by: ResolvedBy::System } if request_id == "bg"
    )));
    let attach = h
        .manager
        .attach(&chat.id, Sink::new("main"))
        .await
        .expect("attach");
    assert_eq!(attach.snapshot.queue.len(), 1);
    assert!(
        attach.snapshot.queue[0].held,
        "nothing is sent on its own after a restart"
    );
    assert_eq!(
        attach.snapshot.session.status,
        SessionStatus::Stopped,
        "opening a chat starts nothing"
    );
}

/// S15: queued prompts survive a restart, held; Send queued releases them.
#[tokio::test(start_paused = true)]
async fn queue_survives_a_restart_held() {
    let (first, first_handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let h = Harness::new(vec![first]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "work".into()).await.expect("send");
    command(&first_handle, CommandMatch::Prompt).await;
    let later = h
        .manager
        .send(&chat.id, "later".into())
        .await
        .expect("send");
    assert!(later.queued);
    let (second, handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        end_turn(StopReason::EndTurn),
    ]);
    let h = h.restart(vec![second], Options::default()).await;
    let queue = h.queue(&chat.id).await;
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].prompt_id, later.prompt_id);
    assert!(queue[0].held);
    h.manager.resume_queue(&chat.id).await.expect("Send queued");
    match command(&handle, CommandMatch::Prompt).await {
        LoggedCommand::Prompt { parts, .. } => assert_eq!(
            parts,
            [ghost_protocol::PromptPart::Text {
                text: "later".into()
            }]
        ),
        other => panic!("a prompt, got {other:?}"),
    }
}

/// S15 and S24: quitting holds the rows queued before it in the database itself, before any
/// recovery at the next start could.
#[tokio::test(start_paused = true)]
async fn quit_holds_the_queue() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "work".into()).await.expect("send");
    let later = h
        .manager
        .send(&chat.id, "later".into())
        .await
        .expect("send");
    command(&handle, CommandMatch::Prompt).await;
    let summary = h.manager.shutdown_all(Duration::from_secs(8)).await;
    assert_eq!((summary.sessions, summary.forced), (1, 0));
    assert!(h.store.shutdown(Duration::from_secs(5)).await);
    let conn = rusqlite::Connection::open(h.profile.db_path()).expect("raw");
    let rows: Vec<(String, bool)> = conn
        .prepare("SELECT id, held FROM queued_prompts WHERE chat_id = ?1")
        .expect("prepare")
        .query_map([&chat.id], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect();
    assert_eq!(rows, [(later.prompt_id, true)]);
}

/// S36: quitting with no live session: `shutdown_all` does nothing, the store commits what is
/// queued and closes, and the next launch reports a clean shutdown.
#[tokio::test(start_paused = true)]
async fn quit_without_sessions_commits_and_closes() {
    let (driver, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    h.manager
        .set_autonomy(&chat.id, ghost_protocol::Autonomy::Ask)
        .await
        .expect("an op still queued at the quit");
    assert!(!h.manager.has_live_sessions());
    let summary = h.manager.shutdown_all(Duration::from_secs(8)).await;
    assert_eq!(summary.forced, 0);
    assert!(h.store.shutdown(Duration::from_secs(5)).await);
    let wal = h.profile.db_path().with_extension("db-wal");
    assert!(
        std::fs::metadata(&wal).map_or(true, |meta| meta.len() == 0),
        "the WAL was folded back"
    );
    let owner = ProfileOwner::acquire(&h.profile).expect("the owner lock was released");
    let (_store, info) = ghost_core::store::Store::open(&h.profile).expect("reopen");
    assert!(!info.unclean_shutdown);
    drop(owner);
}

/// The Processes page lists a chat's running agent process, and not once the chat is stopped.
#[tokio::test(start_paused = true)]
async fn agent_processes_lists_a_running_chat_until_it_stops() {
    let options = Options {
        agents: vec![with_process(mock_spec())],
        ..Options::default()
    };
    let (driver, _) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let h = Harness::with(vec![driver], options);
    assert!(h.manager.agent_processes().is_empty(), "no chat runs yet");
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let listed = h.manager.agent_processes();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].chat_id, chat.id);
    assert!(listed[0].tasks.is_empty(), "no task was reported");
    assert_eq!(
        listed[0]
            .processes
            .iter()
            .map(|control| control.pid())
            .collect::<Vec<_>>(),
        vec![ghost_process::ControlImpl::pid(
            h.spawner.controls()[0].as_ref()
        )]
    );
    h.manager.send(&chat.id, "go".into()).await.expect("send");
    h.status(&chat.id, SessionStatus::Running).await;
    h.manager.force_stop(&chat.id).await.expect("force stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    eventually("the stopped chat is no longer listed", || {
        h.manager.agent_processes().is_empty()
    })
    .await;
}

/// S34: force stop reaches a wedged driver through its recorded process.
#[tokio::test(start_paused = true)]
async fn force_stop_bypasses_wedged_driver() {
    let options = Options {
        agents: vec![with_process(mock_spec())],
        ..Options::default()
    };
    let (driver, _) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let h = Harness::with(vec![driver], options);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "go".into()).await.expect("send");
    h.status(&chat.id, SessionStatus::Running).await;
    h.manager.force_stop(&chat.id).await.expect("force stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    let control = &h.spawner.controls()[0];
    eventually("the process is killed", || control.was_killed()).await;
    // A second chat still works: the actor never waited on the wedged driver.
    h.manager.lists(&chat.id).await.expect("the actor answers");
}

/// Blob pages through the manager: "Load full output" reads a spilled output back in pages.
#[tokio::test(start_paused = true)]
async fn spilled_output_reads_back_in_pages() {
    let output = "x".repeat(100_000);
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        in_turn(GhostEvent::ToolCall(ToolCall {
            tool_call_id: "big".into(),
            kind: ToolKind::Execute,
            title: "Dump".into(),
            status: ToolStatus::Completed,
            locations: Vec::new(),
            raw_input: None,
            tool_name: None,
            content: vec![ToolContent::Text {
                text: output.clone(),
                truncated: None,
            }],
            truncated: None,
        })),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "dump".into()).await.expect("send");
    h.status(&chat.id, SessionStatus::Ready).await;
    let stored = h.stored(&chat.id).await;
    let sha = stored
        .iter()
        .find_map(|event| match event {
            GhostEvent::ToolCall(call) => match call.content.first() {
                Some(ToolContent::Text {
                    truncated: Some(truncation),
                    ..
                }) => truncation.blob.clone(),
                _ => None,
            },
            _ => None,
        })
        .expect("the output was cut and spilled");
    let mut offset = 0;
    let mut text = String::new();
    loop {
        let page = h
            .manager
            .blob(&chat.id, &sha, offset, 40_000)
            .await
            .expect("page");
        text.push_str(&page.text);
        offset = page.next_offset;
        if page.eof {
            break;
        }
    }
    assert_eq!(text, output);
    assert!(matches!(
        h.manager.blob(&chat.id, &"0".repeat(64), 0, 10).await,
        Err(SessionError::BlobMissing(message)) if message == "The full output is no longer available."
    ));
    assert!(matches!(
        h.manager.blob("nope", &sha, 0, 10).await,
        Err(SessionError::ChatNotFound(_))
    ));
}
