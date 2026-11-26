//! Stopping several chats at once: idle agents only, or every chat, waiting for real stops.

use super::*;
use ghost_drivers::driver::LoggedCommand;

/// A turn that runs until the chat is stopped.
fn working(native: &str) -> (ScriptedDriver, ghost_drivers::testing::ScriptHandle) {
    ScriptedDriver::new(vec![
        ready_step(native),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ])
}

fn card() -> Step {
    Step::EmitInTurn(DriverEvent::Permission {
        turn: None,
        request: ghost_protocol::PermissionRequest {
            request_id: "write".into(),
            tool_call_id: None,
            title: "May I write?".into(),
            kind: Some(ghost_protocol::ToolKind::Edit),
            raw_input: None,
            content: Vec::new(),
            locations: Vec::new(),
            options: vec![ghost_protocol::PermissionOption {
                option_id: "once".into(),
                kind: ghost_protocol::PermissionOptionKind::AllowOnce,
                label: "Allow once".into(),
            }],
            truncated: None,
        },
    })
}

/// What still stops is named, as the person knows each chat, for a profile switch or an install
/// that waits on it.
#[tokio::test]
async fn chats_still_stopping_are_named() {
    use super::super::stopping::still_stopping;
    let rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    let project = rig
        .manager
        .project_create(ghost_protocol::workspace::ProjectCreate {
            project: ghost_protocol::workspace::ProjectInput {
                name: "auracode".into(),
                color: None,
                default_agent: None,
                default_model: None,
            },
            folder: rig.folder.to_string_lossy().into_owned(),
        })
        .await
        .expect("project");
    let titled = rig
        .manager
        .chat_project_create(ghost_protocol::workspace::ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: project.folders[0].id.clone(),
            allow_busy_folder: true,
        })
        .await
        .expect("chat")
        .chat
        .id;
    rig.manager
        .chat_rename(&titled, "Fix the sidebar")
        .await
        .expect("rename");
    let names = rig.manager.chat_names(vec![titled]).await;
    assert_eq!(names, ["\u{201c}Fix the sidebar\u{201d} in auracode"]);
    let then = "Switch profiles again in a moment.";
    assert_eq!(
        still_stopping(&names, then),
        "\u{201c}Fix the sidebar\u{201d} in auracode is still stopping. Switch profiles again in a moment."
    );
    let many: Vec<String> = ["A", "B", "C", "D", "E"].map(String::from).into();
    assert_eq!(
        still_stopping(&many[..2], then),
        format!("A and B are still stopping. {then}")
    );
    assert_eq!(
        still_stopping(&many[..3], then),
        format!("A, B and C are still stopping. {then}")
    );
    assert_eq!(
        still_stopping(&many, then),
        format!("A, B and 3 more chats are still stopping. {then}")
    );
    assert_eq!(
        still_stopping(&[], then),
        format!("A chat is still stopping. {then}")
    );
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn stop_idle_leaves_working_waiting_and_queued_chats_and_stop_all_frees_every_slot() {
    let (idle, idle_handle) = ScriptedDriver::new(vec![ready_step("idle")]);
    let (work, work_handle) = working("work");
    let (asking, asking_handle) = ScriptedDriver::new(vec![
        ready_step("asking"),
        Step::Expect(CommandMatch::Prompt),
        card(),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let (queued, queued_handle) = working("queued");
    let (after, _) = ScriptedDriver::new(vec![ready_step("after")]);
    let rig = rig(
        vec![idle, work, asking, queued, after],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    let mut chats = Vec::new();
    for _ in 0..4 {
        let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
        status(&rig, &chat.id, SessionStatus::Ready).await;
        chats.push(chat.id);
    }
    let [idle, work, asking, queued] = <[String; 4]>::try_from(chats).unwrap();
    rig.manager.send(&work, "work".into()).await.unwrap();
    work_handle.wait_for(CommandMatch::Prompt).await;
    rig.manager.send(&asking, "ask".into()).await.unwrap();
    asking_handle.wait_for(CommandMatch::Prompt).await;
    rig.manager.send(&queued, "first".into()).await.unwrap();
    queued_handle.wait_for(CommandMatch::Prompt).await;
    assert!(
        rig.manager
            .send(&queued, "second".into())
            .await
            .unwrap()
            .queued
    );
    eventually("the card is open", || {
        lock(&rig.statuses.updates)
            .iter()
            .rev()
            .find(|update| update.chat_id == asking)
            .is_some_and(|update| update.needs_attention)
    })
    .await;
    let report = rig.manager.stop_idle(StopScope::All).await;
    assert_eq!(report.stopped, std::slice::from_ref(&idle));
    let mut busy = vec![work.clone(), asking.clone(), queued.clone()];
    busy.sort();
    assert_eq!(report.kept, busy);
    assert!(report.stopping.is_empty());
    // It answered once the idle agent's slot was really free.
    let live = &rig.manager.shared().live;
    assert!(!live.contains(&idle));
    assert_eq!(idle_handle.commands(), [LoggedCommand::Shutdown]);
    for chat in &busy {
        assert!(live.contains(chat), "{chat} keeps running");
    }
    assert!(
        work_handle
            .commands()
            .iter()
            .all(|command| *command != LoggedCommand::Shutdown)
    );
    // Stopping every chat waits until each slot is free, and leaves starts open.
    let report = rig
        .manager
        .stop_all(StopScope::All, Duration::from_secs(20))
        .await;
    assert_eq!(report.stopped, busy);
    assert!(report.stopping.is_empty());
    assert_eq!(live.count(), 0, "every slot is free when it answers");
    assert!(!rig.manager.has_live_sessions());
    let next = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &next.id, SessionStatus::Ready).await;
    // A chat stopped this way keeps what was queued, held for the person.
    let attach = rig
        .manager
        .attach(&queued, Arc::new(Sink::default()))
        .await
        .unwrap();
    rig.manager.detach(&attach.subscription_id);
    let queue = attach.snapshot.queue;
    assert_eq!(queue.len(), 1);
    assert!(queue[0].held);
}

#[tokio::test]
async fn stopping_named_chats_or_folders_leaves_the_others() {
    let (one, _) = ScriptedDriver::new(vec![ready_step("one")]);
    let (two, two_handle) = working("two");
    let (three, three_handle) = ScriptedDriver::new(vec![ready_step("three")]);
    let rig = rig(
        vec![one, two, three],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    let elsewhere = rig._dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let one = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    let two = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    let three = rig.manager.create_chat("mock", &elsewhere).await.unwrap();
    for chat in [&one.id, &two.id, &three.id] {
        status(&rig, chat, SessionStatus::Ready).await;
    }
    rig.manager.send(&two.id, "work".into()).await.unwrap();
    two_handle.wait_for(CommandMatch::Prompt).await;
    // A folder's idle agents only: the working one and the other folder's are left.
    let report = rig
        .manager
        .stop_idle(StopScope::Folders(vec![rig.folder.clone()]))
        .await;
    assert_eq!(report.stopped, std::slice::from_ref(&one.id));
    assert_eq!(report.kept, std::slice::from_ref(&two.id));
    assert!(three_handle.commands().is_empty());
    // Named chats, a working one included, and nothing else.
    let report = rig
        .manager
        .stop_all(
            StopScope::Chats(vec![two.id.clone(), "no-such-chat".into()]),
            Duration::from_secs(20),
        )
        .await;
    assert_eq!(report.stopped, std::slice::from_ref(&two.id));
    let live = &rig.manager.shared().live;
    assert!(!live.contains(&two.id));
    assert!(live.contains(&three.id));
}
