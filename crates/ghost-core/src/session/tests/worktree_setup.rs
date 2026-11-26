use super::*;
use ghost_drivers::driver::LoggedCommand;
use ghost_protocol::worktree_setup::WorktreeSetupState;

async fn pending_chat(rig: &Rig) -> String {
    let chat = rig
        .store
        .create_chat(crate::chatstore::NewChat {
            agent_host: Default::default(),
            agent_id: "mock".into(),
            folder: rig.folder.clone(),
            device_id: "device".into(),
            now: now_millis(),
        })
        .await
        .unwrap();
    let id = chat.id.clone();
    let (saved, done) = oneshot::channel();
    rig.store.with_store(move |store| {
        store.conn().execute("INSERT INTO worktree_setup_attempts VALUES (?1,'device','fixture-tree',1,'awaiting',NULL)",[id]).unwrap();
        let _=saved.send(());
    });
    done.await.unwrap();
    chat.id
}

/// A start that waited for room and then finds its worktree's setup asked again stops, and the
/// chat list hears so instead of showing it starting for good.
#[tokio::test]
async fn a_start_that_waited_for_room_says_it_stopped_when_setup_holds_it() {
    let (idle, idle_handle) = ScriptedDriver::new(vec![
        ready_step("idle"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Wait(Duration::from_millis(500)),
        Step::Exit(DriverExit::Closed),
    ]);
    let rig = with_limits(
        ManagerLimits {
            max_live_agents: 1,
            ..ManagerLimits::STANDARD
        },
        || rig(vec![idle], WriterConfig::STANDARD, Knobs::STANDARD),
    );
    let first = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &first.id, SessionStatus::Ready).await;
    let chat = pending_chat(&rig).await;
    // Its setup had succeeded, so it may start; there is no room, so it waits.
    rig.manager
        .set_setup_attempt(&chat, Some(1), WorktreeSetupState::Succeeded, None)
        .await
        .unwrap();
    rig.manager.start(&chat, false).await.unwrap();
    idle_handle.wait_for(CommandMatch::Shutdown).await;
    status(&rig, &chat, SessionStatus::Starting).await;
    // Setup is asked again meanwhile; once the slot frees, the start cannot go on.
    rig.manager
        .set_setup_attempt(&chat, None, WorktreeSetupState::Awaiting, None)
        .await
        .unwrap();
    status(&rig, &chat, SessionStatus::Stopped).await;
    assert!(!rig.manager.shared().live.contains(&chat));
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn queued_setup_events_report_exact_holds_when_other_messages_are_already_held() {
    for state in [WorktreeSetupState::Failed, WorktreeSetupState::Interrupted] {
        let rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
        let chat = pending_chat(&rig).await;
        let sink = Arc::new(Sink::default());
        rig.manager.attach(&chat, sink.clone()).await.unwrap();
        let first = rig
            .manager
            .send(&chat, "before failure".into())
            .await
            .unwrap();
        rig.manager
            .set_setup_attempt(&chat, Some(1), WorktreeSetupState::Running, None)
            .await
            .unwrap()
            .unwrap();
        rig.manager
            .set_setup_attempt(&chat, Some(1), state, Some("Retry setup.".into()))
            .await
            .unwrap()
            .unwrap();
        let second = rig
            .manager
            .send(&chat, "after failure".into())
            .await
            .unwrap();
        eventually("new held prompt is reported authoritatively", || {
            lock(&sink.frames)
                .iter()
                .flat_map(|batch| &batch.events)
                .any(|event| {
                    matches!(event, GhostEvent::PromptLifecycle {
                    prompt_id,
                    state: ghost_protocol::PromptState::Queued,
                    held: Some(true),
                    ..
                } if prompt_id == &second.prompt_id)
                })
        })
        .await;
        assert!(
            lock(&sink.frames)
                .iter()
                .flat_map(|batch| &batch.events)
                .any(|event| {
                    matches!(event, GhostEvent::PromptLifecycle {
                    prompt_id,
                    state: ghost_protocol::PromptState::Queued,
                    held: Some(false),
                    ..
                } if prompt_id == &first.prompt_id)
                }),
            "the earlier unheld event is not rewritten by the later setup failure"
        );
        let retry = rig
            .manager
            .set_setup_attempt(&chat, None, WorktreeSetupState::Awaiting, None)
            .await
            .unwrap()
            .unwrap();
        let third = rig
            .manager
            .send(&chat, "during retry".into())
            .await
            .unwrap();
        rig.manager
            .set_setup_attempt(
                &chat,
                Some(retry.generation),
                WorktreeSetupState::Running,
                None,
            )
            .await
            .unwrap()
            .unwrap();
        rig.manager
            .set_setup_attempt(
                &chat,
                Some(retry.generation),
                state,
                Some("Retry failed.".into()),
            )
            .await
            .unwrap()
            .unwrap();
        eventually(
            "retry failure reports its preexisting unheld follower",
            || {
                lock(&sink.frames)
                    .iter()
                    .flat_map(|batch| &batch.events)
                    .any(|event| {
                        matches!(event, GhostEvent::PromptLifecycle {
                    prompt_id,
                    state: ghost_protocol::PromptState::Queued,
                    held: Some(true),
                    ..
                } if prompt_id == &third.prompt_id)
                    })
            },
        )
        .await;
        let attached = rig
            .manager
            .attach(&chat, Arc::new(Sink::default()))
            .await
            .unwrap();
        assert_eq!(attached.snapshot.queue.len(), 3);
        assert_eq!(
            attached
                .snapshot
                .queue
                .iter()
                .map(|row| &row.prompt_id)
                .collect::<Vec<_>>(),
            [&first.prompt_id, &second.prompt_id, &third.prompt_id]
        );
        assert!(attached.snapshot.queue.iter().all(|row| row.held));
        let sequences = lock(&sink.frames)
            .iter()
            .flat_map(|batch| {
                (0..batch.events.len()).map(move |index| batch.seq_start + index as u32)
            })
            .collect::<Vec<_>>();
        assert!(
            sequences.windows(2).all(|pair| pair[0] < pair[1]),
            "hold updates retain monotonic live event identities"
        );
    }
}

#[tokio::test]
async fn delayed_setup_allows_durable_compose_edit_reorder_and_releases_fifo_once() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("setup-queue"),
        Step::Expect(CommandMatch::Prompt),
        Step::EndTurn {
            stop_reason: StopReason::EndTurn,
            usage: None,
            error: None,
        },
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = pending_chat(&rig).await;
    assert!(rig.manager.start(&chat, false).await.is_err());
    let first = rig.manager.send(&chat, "first draft".into()).await.unwrap();
    rig.manager
        .ask(&chat, |reply| ChatMsg::ReserveSetup {
            id: "setup-reservation".into(),
            reply,
        })
        .await
        .unwrap();
    rig.manager
        .set_setup_attempt(&chat, Some(1), WorktreeSetupState::Running, None)
        .await
        .unwrap()
        .unwrap();
    let second = rig.manager.send(&chat, "second".into()).await.unwrap();
    rig.manager
        .edit_queued(&chat, &first.prompt_id, "first edited".into())
        .await
        .unwrap();
    rig.manager
        .move_queued(&chat, &second.prompt_id, 0)
        .await
        .unwrap();
    let attach = rig
        .manager
        .attach(&chat, Arc::new(Sink::default()))
        .await
        .unwrap();
    assert_eq!(
        attach
            .snapshot
            .queue
            .iter()
            .map(|row| row.prompt_id.as_str())
            .collect::<Vec<_>>(),
        [second.prompt_id.as_str(), first.prompt_id.as_str()]
    );
    assert!(matches!(
        attach.snapshot.session.queue_wait,
        Some(ghost_protocol::QueueWait::Setup {
            state: WorktreeSetupState::Running,
            ..
        })
    ));
    assert!(
        handle.commands().is_empty(),
        "no agent commands before setup succeeds"
    );
    assert!(rig.manager.start(&chat, false).await.is_err());
    rig.manager
        .actor(&chat)
        .send(ChatMsg::ReleaseGit {
            id: "setup-reservation".into(),
        })
        .unwrap();
    rig.manager
        .set_setup_attempt(&chat, Some(1), WorktreeSetupState::Succeeded, None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        rig.manager
            .set_setup_attempt(&chat, Some(1), WorktreeSetupState::Succeeded, None)
            .await
            .unwrap()
            .is_none()
    );
    eventually("two prompts after successful setup", || {
        handle
            .commands()
            .iter()
            .filter(|command| matches!(command, LoggedCommand::Prompt { .. }))
            .count()
            == 2
    })
    .await;
    let texts = handle
        .commands()
        .into_iter()
        .filter_map(|command| {
            if let LoggedCommand::Prompt { parts, .. } = command {
                parts.into_iter().find_map(|part| {
                    if let ghost_protocol::PromptPart::Text { text } = part {
                        Some(text)
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["second", "first edited"]);
    let saved = rig.store.read_setup_attempt(&chat).await.unwrap().unwrap();
    assert_eq!(saved.state, WorktreeSetupState::Succeeded);
}

#[tokio::test]
async fn failure_holds_messages_and_retry_ignores_old_generation_completion() {
    let (driver, handle) = ScriptedDriver::new(vec![ready_step("setup-retry"), Step::Hang]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = pending_chat(&rig).await;
    rig.manager.send(&chat, "keep this".into()).await.unwrap();
    rig.manager
        .set_setup_attempt(&chat, Some(1), WorktreeSetupState::Running, None)
        .await
        .unwrap()
        .unwrap();
    rig.manager
        .set_setup_attempt(
            &chat,
            Some(1),
            WorktreeSetupState::Failed,
            Some("Setup exited with code 2. Retry setup.".into()),
        )
        .await
        .unwrap()
        .unwrap();
    let failed = rig
        .manager
        .attach(&chat, Arc::new(Sink::default()))
        .await
        .unwrap();
    assert!(failed.snapshot.queue[0].held);
    assert!(matches!(
        failed.snapshot.session.queue_wait,
        Some(ghost_protocol::QueueWait::Setup {
            state: WorktreeSetupState::Failed,
            ..
        })
    ));
    let retry = rig
        .manager
        .set_setup_attempt(&chat, None, WorktreeSetupState::Awaiting, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.generation, 2);
    assert!(
        rig.manager
            .set_setup_attempt(&chat, Some(1), WorktreeSetupState::Succeeded, None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(rig.manager.start(&chat, false).await.is_err());
    rig.manager
        .set_setup_attempt(&chat, Some(2), WorktreeSetupState::Running, None)
        .await
        .unwrap()
        .unwrap();
    rig.manager
        .set_setup_attempt(&chat, Some(2), WorktreeSetupState::Succeeded, None)
        .await
        .unwrap()
        .unwrap();
    status(&rig, &chat, SessionStatus::Ready).await;
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::Prompt { .. })),
        "failed prompts remain held after retry success"
    );
}
