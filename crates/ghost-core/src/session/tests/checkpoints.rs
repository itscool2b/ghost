use super::*;
use ghost_drivers::driver::LoggedCommand;
use ghost_protocol::NoticeLevel;

#[path = "checkpoint_benchmarks.rs"]
mod benchmarks;

fn enable(rig: &mut Rig, git_available: bool) -> Arc<crate::checkpoints::Checkpoints> {
    let service = install(rig, git_available);
    rig.manager.set_automatic_checkpoints(true);
    service
}

fn install(rig: &mut Rig, git_available: bool) -> Arc<crate::checkpoints::Checkpoints> {
    let vars = if git_available {
        ghost_process::process_env()
    } else {
        EnvMap::new()
    };
    let env = EnvResolver::fixed(ResolvedEnv {
        vars,
        source: EnvSource::Fallback {
            reason: "checkpoint test".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    let service = crate::checkpoints::Checkpoints::new(
        &rig.profile,
        env,
        ghost_process::LeftoverRegistry::disabled(),
    );
    let inner = Arc::get_mut(&mut rig.manager.inner).expect("only test owner");
    Arc::get_mut(&mut inner.shared)
        .expect("no actors yet")
        .deps
        .checkpoints = Some(Arc::clone(&service));
    service
}

#[tokio::test]
async fn default_sends_without_git_or_writable_checkpoint_storage() {
    let (driver, handle) = script();
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    install(&mut rig, false);
    assert!(!rig.manager.automatic_checkpoints());
    let storage = rig.profile.checkpoints_dir();
    std::fs::remove_dir(&storage).expect("empty checkpoint directory");
    std::fs::write(&storage, b"checkpoint storage deliberately unavailable").unwrap();
    std::fs::write(rig.folder.join("work.txt"), b"original").unwrap();
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let sink = Arc::new(Sink::default());
    rig.manager.attach(&chat.id, sink.clone()).await.unwrap();
    rig.manager
        .send(&chat.id, "send immediately".into())
        .await
        .unwrap();
    handle.wait_for(CommandMatch::Prompt).await;
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let changes = rig.manager.chat_changes(&chat.id, None).await.unwrap();
    assert!(changes.checkpoint.is_none());
    assert!(changes.unavailable_reason.is_some());
    assert_eq!(
        std::fs::read(&storage).unwrap(),
        b"checkpoint storage deliberately unavailable"
    );
    assert!(!lock(&sink.frames).iter().flat_map(|frame| &frame.events).any(|event| {
        matches!(event, GhostEvent::SessionUpdated(info) if info.detail.as_deref().is_some_and(|detail| detail.contains("Saving checkpoint")))
    }));
}

#[tokio::test]
async fn live_opt_in_is_latched_for_capture_and_old_snapshots_remain_usable_when_off() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("checkpoint-policy"),
        Step::Expect(CommandMatch::Prompt),
        Step::EndTurn {
            stop_reason: StopReason::EndTurn,
            usage: None,
            error: None,
        },
        Step::Expect(CommandMatch::Prompt),
        Step::EndTurn {
            stop_reason: StopReason::EndTurn,
            usage: None,
            error: None,
        },
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let service = install(&mut rig, true);
    std::fs::write(rig.folder.join("work.txt"), b"original\r\n").unwrap();
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.set_automatic_checkpoints(true);
    let gate = service
        .hold_for_test(&crate::ids::new_id(), &rig.folder)
        .await;
    rig.manager
        .send(&chat.id, "protected turn".into())
        .await
        .unwrap();
    status(&rig, &chat.id, SessionStatus::Running).await;
    rig.manager.set_automatic_checkpoints(false);
    drop(gate);
    handle.wait_for(CommandMatch::Prompt).await;
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let saved = rig
        .manager
        .chat_changes(&chat.id, None)
        .await
        .unwrap()
        .checkpoint
        .expect("in-flight capture finishes");
    rig.manager
        .send(&chat.id, "without a new snapshot".into())
        .await
        .unwrap();
    eventually("second prompt reaches driver", || {
        handle
            .commands()
            .iter()
            .filter(|command| matches!(command, LoggedCommand::Prompt { .. }))
            .count()
            == 2
    })
    .await;
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert!(
        rig.manager
            .chat_changes(&chat.id, None)
            .await
            .unwrap()
            .checkpoint
            .is_none()
    );
    std::fs::write(rig.folder.join("work.txt"), b"later edit").unwrap();
    let older = rig
        .manager
        .chat_changes(&chat.id, Some(saved.turn_id))
        .await
        .unwrap();
    assert_eq!(older.checkpoint.unwrap().id, saved.id);
    assert_eq!(older.files.len(), 1);
    let preview = rig
        .manager
        .checkpoint_revert_preview(&chat.id, &saved.id, vec!["work.txt".into()])
        .await
        .unwrap();
    rig.manager.stop(&chat.id).await.unwrap();
    eventually("stop before reverting", || {
        !rig.manager.shared().live.contains(&chat.id)
    })
    .await;
    assert!(
        rig.manager
            .checkpoint_revert(&chat.id, &preview.token)
            .await
            .unwrap()
            .error
            .is_none()
    );
    assert_eq!(
        std::fs::read(rig.folder.join("work.txt")).unwrap(),
        b"original\r\n"
    );
}

fn script() -> (ScriptedDriver, ghost_drivers::testing::ScriptHandle) {
    ScriptedDriver::new(vec![
        ready_step("checkpoint-native"),
        Step::Expect(CommandMatch::Prompt),
        Step::EndTurn {
            stop_reason: StopReason::EndTurn,
            usage: None,
            error: None,
        },
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ])
}

#[tokio::test]
async fn capture_precedes_prompt_and_revert_stops_the_idle_agent_first() {
    let (driver, handle) = script();
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    enable(&mut rig, true);
    std::fs::write(rig.folder.join("work.txt"), b"original\r\n").expect("fixture");
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let ack = rig
        .manager
        .send(&chat.id, "visible prompt".into())
        .await
        .expect("send");
    assert!(ack.queued);
    handle.wait_for(CommandMatch::Prompt).await;
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let changes = rig
        .manager
        .chat_changes(&chat.id, None)
        .await
        .expect("changes");
    let checkpoint = changes.checkpoint.expect("capture persisted with turn");
    assert_eq!(checkpoint.file_count, 1);
    assert!(changes.files.is_empty());
    std::fs::write(rig.folder.join("work.txt"), "agent edit").expect("edit");
    let preview = rig
        .manager
        .checkpoint_revert_preview(&chat.id, &checkpoint.id, vec!["work.txt".into()])
        .await
        .expect("preview");
    // The agent finished its turn: reverting stops it first (it resumes on the next message),
    // and only once its process is gone are the files rewritten.
    let result = rig
        .manager
        .checkpoint_revert(&chat.id, &preview.token)
        .await
        .expect("revert");
    assert!(result.error.is_none());
    assert!(!rig.manager.shared().live.contains(&chat.id));
    assert!(
        handle
            .commands()
            .contains(&ghost_drivers::driver::LoggedCommand::Shutdown)
    );
    assert_eq!(
        std::fs::read(rig.folder.join("work.txt")).expect("restored"),
        b"original\r\n"
    );
}

#[tokio::test]
async fn checkpoint_failure_holds_the_prompt_without_sending_it_to_an_agent() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("missing-git"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    enable(&mut rig, false);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let sink = Arc::new(Sink::default());
    rig.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    rig.manager
        .send(&chat.id, "must not reach agent".into())
        .await
        .expect("enqueue");
    eventually("checkpoint failure shown", || lock(&sink.frames).iter().flat_map(|frame| &frame.events).any(|item| matches!(item, GhostEvent::Notice { message, .. } if message.contains("prompt was held")))).await;
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::Prompt { .. }))
    );
    let attached = rig
        .manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .expect("state");
    assert_eq!(attached.snapshot.queue.len(), 1);
    assert!(attached.snapshot.queue[0].held);
}

#[tokio::test]
async fn recovery_failure_holds_an_unsent_prompt_even_with_snapshots_off() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("interrupted-restore"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let service = install(&mut rig, true);
    std::fs::write(rig.folder.join("work.txt"), b"original").unwrap();
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let (record, _) = rig.store.checkpoint_context(&chat.id, None).await.unwrap();
    let saved = service
        .capture(
            &chat.id,
            &record.folder_id.unwrap(),
            &rig.folder,
            &crate::ids::new_id(),
        )
        .await
        .unwrap();
    let leftover = rig
        .folder
        .join(format!(".auracode-revert-{}", crate::ids::new_id()));
    std::fs::create_dir(&leftover).unwrap();
    std::fs::write(leftover.join("original-0"), b"only copy of newer work").unwrap();
    std::fs::write(leftover.join("recovery.json"), serde_json::to_vec(&serde_json::json!({"version":1,"checkpoint_id":saved.id,"files":[{"path":"work.txt","backup":"original-0"}]})).unwrap()).unwrap();
    std::fs::write(
        rig.profile.checkpoints_dir().join("reverts"),
        b"recovery unavailable",
    )
    .unwrap();
    let sink = Arc::new(Sink::default());
    rig.manager.attach(&chat.id, sink.clone()).await.unwrap();
    rig.manager
        .send(&chat.id, "wait for safe recovery".into())
        .await
        .unwrap();
    eventually("recovery failure shown", || lock(&sink.frames).iter().flat_map(|frame| &frame.events).any(|item| matches!(item, GhostEvent::Notice { message, .. } if message.contains("prompt was held")))).await;
    let attached = rig
        .manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .unwrap();
    assert_eq!(attached.snapshot.queue.len(), 1);
    assert!(attached.snapshot.queue[0].held);
    assert!(
        attached
            .snapshot
            .session
            .detail
            .as_deref()
            .unwrap()
            .starts_with("Workspace recovery failed")
    );
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::Prompt { .. }))
    );
    assert_eq!(
        std::fs::read(leftover.join("original-0")).unwrap(),
        b"only copy of newer work"
    );
    assert_eq!(
        std::fs::read(rig.folder.join("work.txt")).unwrap(),
        b"original"
    );
}

#[tokio::test]
async fn interrupt_cancels_capture_or_recovery_wait_and_holds_the_unsent_prompt() {
    for automatic in [true, false] {
        let (driver, handle) = ScriptedDriver::new(vec![
            ready_step("cancel-checkpoint"),
            Step::Expect(CommandMatch::Shutdown),
        ]);
        let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
        let service = enable(&mut rig, true);
        rig.manager.set_automatic_checkpoints(automatic);
        let gate = service
            .hold_for_test(&crate::ids::new_id(), &rig.folder)
            .await;
        let chat = rig
            .manager
            .create_chat("mock", &rig.folder)
            .await
            .expect("chat");
        status(&rig, &chat.id, SessionStatus::Ready).await;
        rig.manager
            .send(&chat.id, "cancel before capture".into())
            .await
            .expect("send");
        status(&rig, &chat.id, SessionStatus::Running).await;
        let preparing = rig
            .manager
            .attach(&chat.id, Arc::new(Sink::default()))
            .await
            .unwrap();
        assert_eq!(
            preparing.snapshot.session.detail.as_deref(),
            Some(if automatic {
                "Saving checkpoint before sending the prompt"
            } else {
                "Preparing workspace before sending the prompt"
            })
        );
        rig.manager.interrupt(&chat.id).await.expect("interrupt");
        drop(gate);
        status(&rig, &chat.id, SessionStatus::Ready).await;
        let attached = rig
            .manager
            .attach(&chat.id, Arc::new(Sink::default()))
            .await
            .expect("state");
        assert!(attached.snapshot.queue[0].held);
        assert!(
            !handle
                .commands()
                .iter()
                .any(|command| matches!(command, LoggedCommand::Prompt { .. }))
        );
    }
}

/// Every prompt waits for its checkpoint. Meanwhile it is on its way (`sending`), not queued: it
/// waits behind nothing. A prompt sent after it waits, and the queue says why. Then both go.
#[tokio::test]
async fn a_saving_checkpoint_says_why_the_prompt_waits() {
    // The first turn keeps running, so the second prompt is still waiting at the end.
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("checkpoint-native"),
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let service = enable(&mut rig, true);
    let gate = service
        .hold_for_test(&crate::ids::new_id(), &rig.folder)
        .await;
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let sink = Arc::new(Sink::default());
    rig.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    let full_text = format!("{}different ending", "unicode 🐢 ".repeat(80));
    let first = rig
        .manager
        .send(&chat.id, full_text.clone())
        .await
        .expect("send");
    status(&rig, &chat.id, SessionStatus::Running).await;
    let attached = rig
        .manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .expect("state");
    assert!(attached.snapshot.queue.is_empty(), "sending, not queued");
    assert_eq!(attached.snapshot.session.queue_wait, None);
    let sending = attached.snapshot.session.sending.expect("on its way");
    assert_eq!(sending.prompt_id, first.prompt_id);
    assert_eq!(sending.text, full_text);
    assert_eq!(
        sending.parts,
        vec![ghost_protocol::PromptPart::Text { text: full_text }]
    );
    assert!(sending.created_at > 0);
    eventually("the sending lifecycle", || {
        lock(&sink.frames)
            .iter()
            .flat_map(|frame| &frame.events)
            .any(|event| {
                matches!(event, GhostEvent::PromptLifecycle { prompt_id, state: ghost_protocol::PromptState::Sending, .. } if *prompt_id == first.prompt_id)
            })
    })
    .await;
    let second = rig
        .manager
        .send(&chat.id, "behind it".into())
        .await
        .expect("send");
    assert!(second.queued);
    let attached = rig
        .manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .expect("state");
    assert_eq!(
        attached
            .snapshot
            .queue
            .iter()
            .map(|row| row.prompt_id.as_str())
            .collect::<Vec<_>>(),
        [second.prompt_id.as_str()]
    );
    assert_eq!(
        attached.snapshot.session.queue_wait,
        Some(ghost_protocol::QueueWait::SavingCheckpoint)
    );
    drop(gate);
    handle.wait_for(CommandMatch::Prompt).await;
    let attached = rig
        .manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .expect("state");
    assert_eq!(attached.snapshot.session.sending, None);
    assert_eq!(
        attached.snapshot.queue.len(),
        1,
        "the second waits for the turn"
    );
    assert_eq!(
        attached.snapshot.session.queue_wait,
        Some(ghost_protocol::QueueWait::Working)
    );
}

#[tokio::test]
async fn background_work_started_during_capture_keeps_prompt_queued_until_idle() {
    let (driver, handle) = script();
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let service = enable(&mut rig, true);
    let gate = service
        .hold_for_test(&crate::ids::new_id(), &rig.folder)
        .await;
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager
        .send(&chat.id, "wait for background work".into())
        .await
        .expect("send");
    status(&rig, &chat.id, SessionStatus::Running).await;
    let sink = Arc::new(Sink::default());
    rig.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    handle.emit(DriverEvent::RunState { idle: false });
    handle.emit(DriverEvent::Event {
        turn: None,
        event: GhostEvent::Notice {
            level: NoticeLevel::Info,
            message: "background marker".into(),
        },
    });
    eventually("background state observed", || lock(&sink.frames).iter().flat_map(|frame| &frame.events).any(|event| matches!(event, GhostEvent::Notice { message, .. } if message == "background marker"))).await;
    drop(gate);
    // Acquiring the same workspace lease after capture ensures the Git operation has ended.
    let captured = service
        .hold_for_test(&crate::ids::new_id(), &rig.folder)
        .await;
    drop(captured);
    tokio::task::yield_now().await;
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::Prompt { .. }))
    );
    handle.emit(DriverEvent::RunState { idle: true });
    handle.wait_for(CommandMatch::Prompt).await;
}

#[tokio::test]
async fn a_legacy_background_permission_during_capture_does_not_block_the_next_turn() {
    use ghost_protocol::{PermissionOption, PermissionOptionKind, PermissionRequest};
    let (driver, handle) = script();
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let service = enable(&mut rig, true);
    let gate = service
        .hold_for_test(&crate::ids::new_id(), &rig.folder)
        .await;
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let sink = Arc::new(Sink::default());
    rig.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    rig.manager
        .send(&chat.id, "next legacy turn".into())
        .await
        .expect("send");
    status(&rig, &chat.id, SessionStatus::Running).await;
    handle.emit(DriverEvent::Permission {
        turn: None,
        request: PermissionRequest {
            request_id: "background-during-capture".into(),
            tool_call_id: None,
            title: "Background action".into(),
            kind: None,
            raw_input: None,
            content: vec![],
            locations: vec![],
            options: vec![PermissionOption {
                option_id: "allow".into(),
                kind: PermissionOptionKind::AllowOnce,
                label: "Allow once".into(),
            }],
            truncated: None,
        },
    });
    eventually("legacy background request observed", || {
        lock(&sink.frames)
            .iter()
            .flat_map(|frame| &frame.events)
            .any(|event| matches!(event, GhostEvent::PermissionRequested(_)))
    })
    .await;
    drop(gate);
    handle.wait_for(CommandMatch::Prompt).await;
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::RespondPermission { .. }))
    );
}
