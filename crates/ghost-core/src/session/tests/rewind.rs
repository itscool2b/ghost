use super::*;
use ghost_drivers::driver::{DriverError, FileRewindResult, LoggedCommand};
use ghost_drivers::testing::ScriptHandle;
use ghost_protocol::{ErrorKind, changes::NativeRewind};

const TURN: &str = "019a0000-0000-7000-8000-000000000009";

fn driver(rewind: bool) -> (ScriptedDriver, ScriptHandle) {
    let Step::Emit(DriverEvent::Ready(mut ready)) = ready_step("rewind-native") else {
        unreachable!()
    };
    ready.capabilities.rewind = rewind;
    ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        Step::Emit(DriverEvent::Ready(ready)),
    ])
}

#[tokio::test]
async fn a_rewind_reaches_the_agent_for_that_message_and_reports_its_files() {
    let (driver, handle) = driver(true);
    handle.set_rewind_reply(Ok(FileRewindResult {
        files_changed: vec!["src/main.rs".into()],
        insertions: Some(2),
        deletions: Some(3),
        skipped_links: None,
    }));
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let preview = rig
        .manager
        .chat_rewind_files(&chat.id, TURN.into(), true)
        .await
        .unwrap();
    assert_eq!(
        preview,
        NativeRewind {
            files: vec!["src/main.rs".into()],
            insertions: Some(2),
            deletions: Some(3),
            skipped_links: None,
        }
    );
    let LoggedCommand::RewindFiles {
        user_message_id,
        dry_run,
    } = handle.wait_for(CommandMatch::RewindFiles).await
    else {
        panic!("rewind")
    };
    assert_eq!((user_message_id.as_str(), dry_run), (TURN, true));
    // The agent's own reason comes back, and the session stays usable.
    handle.set_rewind_reply(Err(DriverError {
        kind: ErrorKind::ProtocolError,
        message: "No file checkpoint found for this message.".into(),
        detail: None,
    }));
    let error = rig
        .manager
        .chat_rewind_files(&chat.id, TURN.into(), false)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("No file checkpoint found"),
        "{error}"
    );
    rig.manager
        .chat_rewind_files(&chat.id, TURN.into(), true)
        .await
        .unwrap_err();
}

#[tokio::test]
async fn a_session_without_native_rewind_refuses_before_asking_the_agent() {
    let (driver, handle) = driver(false);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let error = rig
        .manager
        .chat_rewind_files(&chat.id, TURN.into(), true)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("cannot restore files"),
        "{error}"
    );
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::RewindFiles { .. }))
    );
}

/// What the session loaded reaches its lists once the agent reports it, and a new process starts
/// without the last one's.
#[tokio::test]
async fn what_the_session_loaded_reaches_its_lists() {
    use ghost_protocol::session::{LoadedMcpServer, McpLoadState, SessionLoaded};
    let Step::Emit(DriverEvent::Ready(ready)) = ready_step("loaded-native") else {
        unreachable!()
    };
    let loaded = SessionLoaded {
        mcp_servers: vec![LoadedMcpServer {
            name: "docs".into(),
            state: McpLoadState::Connected,
            source: Some("user".into()),
        }],
        skills: vec!["deploy".into()],
        ..Default::default()
    };
    let (driver, _handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        Step::Emit(DriverEvent::Ready(ready)),
        Step::Emit(DriverEvent::Loaded(loaded.clone())),
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let mut lists = rig.manager.lists(&chat.id).await.unwrap();
    for _ in 0..50 {
        if lists.loaded.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        lists = rig.manager.lists(&chat.id).await.unwrap();
    }
    assert_eq!(lists.loaded, Some(loaded));
}
