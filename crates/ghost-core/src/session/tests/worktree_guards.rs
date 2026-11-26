use super::*;

pub(super) async fn enable_worktree_fixture(rig: &mut Rig) {
    use ghost_process::{
        HelperEnv, HelperSpec, LeftoverRegistry, find_exe, process_env, run_helper,
    };
    let vars = process_env();
    let git = find_exe("git", &vars).expect("Git is required for owned worktree tests");
    let resolved = Arc::new(ResolvedEnv {
        vars,
        source: EnvSource::Fallback {
            reason: "owned worktree test".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    for command in [
        vec!["init", "-b", "main"],
        vec!["config", "core.autocrlf", "false"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@localhost",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    ] {
        let output = run_helper(
            &LeftoverRegistry::disabled(),
            HelperSpec {
                program: git.clone(),
                args: command.into_iter().map(Into::into).collect(),
                cwd: rig.folder.clone(),
                env: HelperEnv::Resolved(Arc::clone(&resolved)),
                set: vec![],
                new_session: true,
                timeout: Duration::from_secs(15),
                max_stdout: 8192,
            },
        )
        .await
        .expect("owned Git fixture");
        assert_eq!(
            output.exit.and_then(|exit| exit.code),
            Some(0),
            "{}",
            output.stderr_tail
        );
    }
    let service = crate::gitworkspaces::GitWorkspaces::new(
        &rig.profile,
        EnvResolver::fixed((*resolved).clone()),
        LeftoverRegistry::disabled(),
    );
    let inner = Arc::get_mut(&mut rig.manager.inner).expect("only test owner");
    Arc::get_mut(&mut inner.shared)
        .expect("no actors")
        .deps
        .worktrees = Some(service);
}

fn relocation_driver(native: &str) -> (ScriptedDriver, ghost_drivers::testing::ScriptHandle) {
    let mut ready = ready_step(native);
    if let Step::Emit(DriverEvent::Ready(info)) = &mut ready {
        info.capabilities.resume = true;
    }
    ScriptedDriver::new(vec![
        ready,
        Step::Expect(CommandMatch::Prompt),
        delta("saved reply"),
        Step::EmitInTurn(DriverEvent::Event {
            turn: None,
            event: GhostEvent::MessageCompleted {
                message_id: "m1".into(),
            },
        }),
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
async fn relocation_preserves_history_and_sends_only_the_reviewed_handoff_on_the_next_prompt() {
    use ghost_drivers::driver::LoggedCommand;
    use ghost_protocol::{
        PromptPart,
        git::{IsolationInput, WorktreeDestination},
    };
    let (first, h1) = relocation_driver("original-folder-session");
    let (second, h2) = relocation_driver("isolated-folder-session");
    let mut rig = rig(vec![first, second], WriterConfig::STANDARD, Knobs::STANDARD);
    enable_worktree_fixture(&mut rig).await;
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager
        .send(&chat.id, "original visible prompt".into())
        .await
        .expect("send");
    h1.wait_for(CommandMatch::Prompt).await;
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.stop(&chat.id).await.expect("stop");
    eventually("source retired", || {
        !rig.manager.shared().live.contains(&chat.id)
    })
    .await;
    let isolation = rig
        .manager
        .isolation_preview(
            &chat.id,
            IsolationInput {
                slug: "move".into(),
                bring_changes: false,
            },
        )
        .await
        .expect("isolation preview");
    let created = rig
        .manager
        .isolation_apply(&chat.id, &isolation.token)
        .await
        .expect("create worktree");
    let preview = rig
        .manager
        .worktree_relocation_preview(
            &chat.id,
            &created.worktree.id,
            WorktreeDestination::Isolated,
        )
        .await
        .expect("move preview");
    assert!(
        preview.handoff.contains("original visible prompt")
            && preview.handoff.contains("saved reply")
    );
    assert!(h2.commands().is_empty(), "preview sends nothing");
    let result = rig
        .manager
        .worktree_relocation_apply(&chat.id, &preview.token)
        .await
        .expect("move");
    assert!(result.prompt.is_none());
    assert!(h2.commands().is_empty(), "apply sends nothing");
    assert!(
        rig.manager
            .worktree_relocation_apply(&chat.id, &preview.token)
            .await
            .is_err()
    );
    let load = rig
        .store
        .load_chat(&chat.id)
        .await
        .expect("saved relocation");
    assert_eq!(
        load.chat.folder.canonicalize().expect("folder"),
        PathBuf::from(&preview.to).canonicalize().expect("target")
    );
    let pending = load.continuation.expect("durable handoff");
    assert_eq!(pending.handoff, preview.handoff);
    assert!(pending.force_fresh);
    assert_eq!(
        rig.manager
            .conversation_turns(&chat.id, None)
            .await
            .expect("history")
            .turns
            .len(),
        1
    );
    rig.manager
        .send(&chat.id, "continue here".into())
        .await
        .expect("next prompt");
    let LoggedCommand::Prompt { parts, .. } = h2.wait_for(CommandMatch::Prompt).await else {
        panic!("prompt");
    };
    assert!(matches!(&parts[0], PromptPart::Text { text } if text.starts_with(&preview.handoff)));
    assert_eq!(
        parts[1],
        PromptPart::Text {
            text: "continue here".into()
        }
    );
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert_eq!(
        rig.manager
            .conversation_turns(&chat.id, None)
            .await
            .expect("retained history")
            .turns
            .len(),
        2
    );
    rig.manager.stop(&chat.id).await.expect("stop isolated");
    eventually("isolated retired", || {
        !rig.manager.shared().live.contains(&chat.id)
    })
    .await;
    let direct = rig
        .manager
        .worktree_relocation_preview(&chat.id, &created.worktree.id, WorktreeDestination::Direct)
        .await
        .expect("return preview");
    rig.manager
        .worktree_relocation_apply(&chat.id, &direct.token)
        .await
        .expect("return");
    assert_eq!(
        rig.store
            .load_chat(&chat.id)
            .await
            .expect("return saved")
            .chat
            .folder
            .canonicalize()
            .expect("folder"),
        rig.folder.canonicalize().expect("source")
    );
    assert!(
        Path::new(&created.worktree.path).is_dir(),
        "return keeps isolated work"
    );
}

#[tokio::test]
async fn relocation_refuses_working_destinations_and_changed_conversations() {
    use ghost_protocol::git::{IsolationInput, WorktreeDestination};
    let (destination, destination_handle) = ScriptedDriver::new(vec![
        ready_step("destination"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let mut rig = rig(
        vec![idle_driver("source"), destination],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    enable_worktree_fixture(&mut rig).await;
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.stop(&chat.id).await.expect("stop source");
    eventually("retired", || !rig.manager.shared().live.contains(&chat.id)).await;
    let isolation = rig
        .manager
        .isolation_preview(
            &chat.id,
            IsolationInput {
                slug: "busy".into(),
                bring_changes: false,
            },
        )
        .await
        .expect("preview");
    let worktree = rig
        .manager
        .isolation_apply(&chat.id, &isolation.token)
        .await
        .expect("create")
        .worktree;
    let preview = rig
        .manager
        .worktree_relocation_preview(&chat.id, &worktree.id, WorktreeDestination::Isolated)
        .await
        .expect("move");
    let other = rig
        .manager
        .create_chat("mock", Path::new(&worktree.path))
        .await
        .expect("destination chat");
    status(&rig, &other.id, SessionStatus::Ready).await;
    rig.manager
        .send(&other.id, "working there".into())
        .await
        .expect("send");
    destination_handle.wait_for(CommandMatch::Prompt).await;
    match rig
        .manager
        .worktree_relocation_apply(&chat.id, &preview.token)
        .await
    {
        Err(SessionError::FolderBusy { blockers, .. }) => {
            assert_eq!(blockers.chats.len(), 1);
            assert_eq!(blockers.chats[0].chat_id, other.id);
        }
        Err(other) => panic!("expected the working destination to be named, got {other}"),
        Ok(_) => panic!("a working destination must refuse the move"),
    }
    assert_eq!(
        rig.store
            .load_chat(&chat.id)
            .await
            .expect("unchanged")
            .chat
            .folder
            .canonicalize()
            .expect("saved folder"),
        rig.folder.canonicalize().expect("source")
    );
    rig.manager.stop(&other.id).await.expect("stop destination");
    eventually("destination retired", || {
        !rig.manager.shared().live.contains(&other.id)
    })
    .await;
    let preview = rig
        .manager
        .worktree_relocation_preview(&chat.id, &worktree.id, WorktreeDestination::Isolated)
        .await
        .expect("review again");
    rig.manager
        .chat_rename(&chat.id, "changed after preview")
        .await
        .expect("rename");
    assert!(
        rig.manager
            .worktree_relocation_apply(&chat.id, &preview.token)
            .await
            .is_err()
    );
}

fn idle_driver(native: &str) -> ScriptedDriver {
    ScriptedDriver::new(vec![
        ready_step(native),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ])
    .0
}

#[tokio::test]
async fn git_folder_reservations_refuse_live_agents_and_block_starts_until_release() {
    let rig = rig(
        vec![idle_driver("first"), idle_driver("second")],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert!(
        rig.manager
            .reserve_git_folders(vec![rig.folder.clone()])
            .await
            .is_err()
    );
    rig.manager.stop(&chat.id).await.expect("stop");
    eventually("retired", || !rig.manager.shared().live.contains(&chat.id)).await;
    let reservation = rig
        .manager
        .reserve_git_folders(vec![rig.folder.clone()])
        .await
        .expect("reserve");
    assert!(
        rig.manager
            .send(&chat.id, "must remain unsent".into())
            .await
            .expect_err("send blocked")
            .to_string()
            .contains("change")
    );
    drop(reservation);
    rig.manager
        .start(&chat.id, false)
        .await
        .expect("start after release");
    status(&rig, &chat.id, SessionStatus::Ready).await;
}

#[tokio::test]
async fn a_saved_held_queue_neither_blocks_git_nor_becomes_an_agent_prompt() {
    let rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .store
        .create_chat(crate::chatstore::NewChat {
            agent_id: "mock".into(),
            agent_host: Default::default(),
            folder: rig.folder.clone(),
            device_id: "device".into(),
            now: crate::time::now_millis(),
        })
        .await
        .expect("saved chat");
    let id = chat.id.clone();
    rig.store.with_store(move|store| {
        store.conn().execute("INSERT INTO queued_prompts (id,chat_id,text,held,created_at,device_id) VALUES ('held-fixture',?1,'preserved',1,1,'device')",[id]).expect("owned queued fixture");
    });
    // A held message waits for the person, so it does not stand in the way of Git.
    let reservation = rig
        .manager
        .reserve_git_folders(vec![rig.folder.clone()])
        .await
        .expect("a held message does not block Git");
    drop(reservation);
    let load = rig.store.load_chat(&chat.id).await.expect("load");
    assert_eq!(load.queue.len(), 1);
    assert_eq!(load.queue[0].text, "preserved");
    assert!(load.queue[0].held);
    assert!(!rig.manager.shared().live.contains(&chat.id));
    assert!(
        rig.manager
            .shared()
            .live
            .reserve_folder(&live::normalize_path(rig.folder.clone())),
        "the reservation was released"
    );
    rig.manager
        .shared()
        .live
        .release_folder(&live::normalize_path(rig.folder.clone()));
}

/// A turn that runs until the chat is stopped.
fn working_driver(native: &str) -> (ScriptedDriver, ghost_drivers::testing::ScriptHandle) {
    ScriptedDriver::new(vec![
        ready_step(native),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ])
}

#[tokio::test]
async fn a_commit_beside_an_idle_agent_holds_its_new_turns_until_it_ends() {
    let (driver, handle) = working_driver("beside");
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    enable_worktree_fixture(&mut rig).await;
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@localhost"),
        ("commit.gpgSign", "false"),
    ] {
        super::project_tools::fixture_git(&rig.folder, &["config", key, value]).await;
    }
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    // A commit with the agent idle beside it: nothing is stopped.
    std::fs::write(rig.folder.join("note.txt"), "committed\n").expect("file");
    let preview = rig
        .manager
        .git_commit_preview(
            &chat.id,
            ghost_protocol::git::CommitInput {
                message: "Add a note".into(),
                paths: vec!["note.txt".into()],
            },
        )
        .await
        .expect("commit review");
    rig.manager
        .git_commit_apply(&chat.id, &preview.token)
        .await
        .expect("commit beside an idle agent");
    assert!(
        rig.manager.shared().live.contains(&chat.id),
        "it kept running"
    );
    assert!(handle.commands().is_empty(), "nothing reached the agent");
    // While a quiet change holds the folder, a new message waits for it...
    let held = rig
        .manager
        .hold_quiet(vec![rig.folder.clone()])
        .await
        .expect("quiet hold");
    assert!(
        rig.manager
            .send(&chat.id, "after the commit".into())
            .await
            .expect("send")
            .queued
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        handle.commands().is_empty(),
        "the turn waits for the change"
    );
    // ...and an exclusive change is kept out.
    assert!(
        !rig.manager
            .shared()
            .live
            .reserve_folder(&live::normalize_path(rig.folder.clone()))
    );
    // Once it ends, the message goes.
    drop(held);
    handle.wait_for(CommandMatch::Prompt).await;
    // A working agent refuses a quiet change, by name.
    match rig.manager.hold_quiet(vec![rig.folder.clone()]).await {
        Err(SessionError::FolderBusy { message, blockers }) => {
            assert_eq!(blockers.chats.len(), 1);
            assert_eq!(blockers.chats[0].chat_id, chat.id);
            assert_eq!(
                blockers.chats[0].state,
                ghost_protocol::workspace::BlockerState::Working
            );
            assert!(message.contains("is working in this folder"), "{message}");
        }
        Err(other) => panic!("expected the working chat to be named, got {other}"),
        Ok(_) => panic!("a quiet change must wait for a running turn"),
    }
}

/// A quiet change refused by a working agent names that agent only: an idle one beside it may stay,
/// so "Stop these chats" never stops it for nothing.
#[tokio::test]
async fn a_quiet_change_names_only_the_chats_in_its_way() {
    let (idle, idle_handle) = ScriptedDriver::new(vec![ready_step("idle")]);
    let (work, work_handle) = working_driver("work");
    let rig = rig(vec![idle, work], WriterConfig::STANDARD, Knobs::STANDARD);
    let idle = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("idle chat");
    status(&rig, &idle.id, SessionStatus::Ready).await;
    let busy = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("working chat");
    status(&rig, &busy.id, SessionStatus::Ready).await;
    rig.manager
        .send(&busy.id, "keep working".into())
        .await
        .expect("send");
    work_handle.wait_for(CommandMatch::Prompt).await;
    match rig.manager.hold_quiet(vec![rig.folder.clone()]).await {
        Err(SessionError::FolderBusy { message, blockers }) => {
            let named: Vec<_> = blockers.chats.iter().map(|chat| &chat.chat_id).collect();
            assert_eq!(named, [&busy.id], "only the working chat is in the way");
            assert_eq!(blockers.terminals, 0);
            assert!(message.contains("is working in this folder"), "{message}");
        }
        Err(other) => panic!("expected the working chat to be named, got {other}"),
        Ok(_) => panic!("a quiet change must wait for a running turn"),
    }
    assert!(idle_handle.commands().is_empty(), "the idle agent runs on");
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

/// A start held back by a quiet change goes only if something still wants it: Stop, or the message
/// it was for being discarded, leaves the agent stopped when the change ends.
#[tokio::test]
async fn a_start_held_by_a_quiet_change_is_dropped_by_stop_or_a_discarded_message() {
    let (driver, _) = ScriptedDriver::new(vec![ready_step("held")]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
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
        .expect("saved chat")
        .id;
    let quiet = || rig.manager.hold_quiet(vec![rig.folder.clone()]);
    // A message sent while the change holds the folder, then discarded.
    let held = quiet().await.expect("quiet hold");
    let sent = rig
        .manager
        .send(&chat, "start for this".into())
        .await
        .expect("send");
    assert!(sent.queued);
    rig.manager
        .cancel_queued(&chat, &sent.prompt_id)
        .await
        .expect("discard");
    drop(held);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !rig.manager.shared().live.contains(&chat),
        "nothing starts for a discarded message"
    );
    // A start asked for while it holds the folder, then stopped.
    let held = quiet().await.expect("quiet hold");
    rig.manager.start(&chat, false).await.expect("start");
    rig.manager.stop(&chat).await.expect("stop");
    drop(held);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !rig.manager.shared().live.contains(&chat),
        "a stopped start stays stopped"
    );
    // A start asked for while it holds the folder, left alone, goes once it ends.
    let held = quiet().await.expect("quiet hold");
    rig.manager.start(&chat, false).await.expect("start");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!rig.manager.shared().live.contains(&chat), "it waits");
    drop(held);
    status(&rig, &chat, SessionStatus::Ready).await;
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn exclusive_actions_stop_idle_agents_and_name_working_ones() {
    use ghost_drivers::driver::LoggedCommand;
    use ghost_protocol::git::IsolationInput;
    let (working, working_handle) = working_driver("working");
    let (quiet, quiet_handle) = ScriptedDriver::new(vec![ready_step("quiet")]);
    let mut rig = rig(
        vec![quiet, working],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    enable_worktree_fixture(&mut rig).await;
    let idle = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("idle chat");
    status(&rig, &idle.id, SessionStatus::Ready).await;
    let busy = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("working chat");
    status(&rig, &busy.id, SessionStatus::Ready).await;
    rig.manager
        .send(&busy.id, "keep working".into())
        .await
        .expect("send");
    working_handle.wait_for(CommandMatch::Prompt).await;
    let isolation = rig
        .manager
        .isolation_preview(
            &idle.id,
            IsolationInput {
                slug: "apart".into(),
                bring_changes: false,
            },
        )
        .await
        .expect("preview");
    match rig
        .manager
        .isolation_apply(&idle.id, &isolation.token)
        .await
    {
        Err(SessionError::FolderBusy { message, blockers }) => {
            let named: Vec<_> = blockers.chats.iter().map(|chat| &chat.chat_id).collect();
            assert_eq!(named, [&busy.id], "only the working chat is in the way");
            assert!(message.contains("is working in this folder"), "{message}");
        }
        Err(other) => panic!("expected the working chat to be named, got {other}"),
        Ok(_) => panic!("a working agent's folder cannot be rewritten"),
    }
    // Refused before anything stopped: the idle agent runs on.
    assert!(rig.manager.shared().live.contains(&idle.id));
    assert!(
        quiet_handle.commands().is_empty(),
        "nothing was stopped for it"
    );
    assert!(rig.manager.shared().live.contains(&busy.id));
    rig.manager.stop(&busy.id).await.expect("stop");
    eventually("working agent stopped", || {
        !rig.manager.shared().live.contains(&busy.id)
    })
    .await;
    let made = rig
        .manager
        .isolation_apply(&idle.id, &isolation.token)
        .await
        .expect("isolate once nothing works there");
    assert_eq!(
        made.worktree.state,
        ghost_protocol::git::WorktreeState::Ready
    );
    // Then the idle agent was stopped first; it resumes on its next message.
    assert_eq!(quiet_handle.commands(), [LoggedCommand::Shutdown]);
}
