//! Terminal chats: a chat opened in its agent's own interface, prepared as a start would be.
use super::*;

fn claude_rig() -> Rig {
    claude_rig_with(Vec::new())
}

fn claude_rig_with(drivers: Vec<ScriptedDriver>) -> Rig {
    rig_with_spec(
        ghost_drivers::testing::ScriptedDriverFactory::new(drivers),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
        ghost_agents::get("claude").expect("claude").clone(),
    )
}

/// A home folder for the next rig built on this thread, where Claude Code's files go.
fn with_home(build: impl FnOnce() -> Rig) -> (tempfile::TempDir, Rig) {
    let home = tempfile::tempdir().unwrap();
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    RIG_ENV.with(|extra| {
        extra.replace(vec![(
            variable.into(),
            home.path().to_string_lossy().into_owned(),
        )])
    });
    let rig = build();
    RIG_ENV.with(|extra| extra.replace(Vec::new()));
    (home, rig)
}

/// Claude Code's session file for `session` under `home`, as it writes one once a message is sent.
fn claude_file(home: &std::path::Path, session: &str) -> std::path::PathBuf {
    let project = home.join(".claude").join("projects").join("C--work-app");
    std::fs::create_dir_all(&project).unwrap();
    project.join(format!("{session}.jsonl"))
}

fn session_arg<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().find_map(|a| a.strip_prefix(flag))
}

fn argv(opened: &TerminalOpened) -> Vec<String> {
    opened
        .spec
        .launch
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[tokio::test]
async fn a_terminal_starts_a_claude_session_chat_continues_and_chat_waits_while_it_is_open() {
    let (home, rig) = with_home(claude_rig);
    let chat = rig
        .manager
        .create_chat("claude", &rig.folder)
        .await
        .unwrap();
    let opened = rig.manager.open_terminal(&chat.id).await.unwrap();
    let args = argv(&opened);
    // The agent's own interface: none of the protocol's arguments.
    assert!(
        !args.iter().any(|a| a == "-p" || a.contains("stream-json")),
        "{args:?}"
    );
    let session = args
        .iter()
        .find_map(|a| a.strip_prefix("--session-id="))
        .expect("a new session's ID")
        .to_owned();
    assert_eq!(opened.spec.folder, rig.folder);
    // A chat that did not move here from another agent has nothing to type first.
    assert_eq!(opened.first_input, None);
    // Chat does not start while the terminal holds the session, and a second terminal waits too.
    let start = rig.manager.start(&chat.id, false).await;
    assert!(
        start
            .as_ref()
            .is_err_and(|e| e.to_string().contains("open in the terminal")),
        "{start:?}"
    );
    assert!(rig.manager.open_terminal(&chat.id).await.is_err());
    // Once it ends, the next terminal continues the same session, which Claude Code wrote down.
    std::fs::write(claude_file(home.path(), &session), "").unwrap();
    rig.manager.terminal_ended(&chat.id, opened.generation);
    let again = reopened(&rig, &chat.id).await;
    assert!(
        argv(&again).contains(&format!("--resume={session}")),
        "{:?}",
        argv(&again)
    );
    // An ending reported for an older terminal changes nothing.
    rig.manager.terminal_ended(&chat.id, opened.generation);
    assert!(rig.manager.open_terminal(&chat.id).await.is_err());
    rig.manager.terminal_ended(&chat.id, again.generation);
}

/// Waits until the chat's own start has made its agent's driver (it has not said Ready yet).
async fn driver_made(factory: &ghost_drivers::testing::ScriptedDriverFactory) {
    for _ in 0..500 {
        if !factory.kinds().is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the chat's own start never reached its agent: {factory:?}");
}

/// A new chat starts its agent as it is made, so a terminal chosen in New chat opens while that
/// agent is still starting. The terminal waits for it to stop and then opens; it used to wait
/// forever.
#[tokio::test]
async fn a_terminal_opened_while_the_new_chats_agent_starts_opens_once_that_agent_stops() {
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let factory = ghost_drivers::testing::ScriptedDriverFactory::new(vec![driver]);
    let rig = rig_with_spec(
        factory.clone(),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
        ghost_agents::get("claude").expect("claude").clone(),
    );
    let chat = rig
        .manager
        .create_chat("claude", &rig.folder)
        .await
        .unwrap();
    driver_made(&factory).await;
    assert_eq!(rig.statuses.status(&chat.id), Some(SessionStatus::Starting));
    let opened = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        rig.manager.open_terminal(&chat.id),
    )
    .await
    .expect("the terminal opens instead of waiting forever")
    .unwrap();
    assert!(
        handle
            .commands()
            .iter()
            .any(|command| matches!(command, ghost_drivers::driver::LoggedCommand::Shutdown)),
        "the starting agent was asked to stop first"
    );
    assert!(session_arg(&argv(&opened), "--session-id=").is_some());
    rig.manager.terminal_ended(&chat.id, opened.generation);
}

/// Stop cannot end a terminal's session, so stopping every chat keeps a chat open in its terminal
/// at once instead of waiting out its deadline for it; once the terminal ends, the chat gives its
/// session back by itself.
#[tokio::test]
async fn stopping_chats_keeps_one_open_in_its_terminal_instead_of_waiting_for_it() {
    let (_home, rig) = with_home(claude_rig);
    let chat = rig
        .manager
        .create_chat("claude", &rig.folder)
        .await
        .unwrap();
    let opened = rig.manager.open_terminal(&chat.id).await.unwrap();
    assert!(rig.manager.shared().live.contains(&chat.id));
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        rig.manager.stop_all(
            super::super::stopping::StopScope::Chats(vec![chat.id.clone()]),
            std::time::Duration::from_secs(3),
        ),
    )
    .await
    .expect("answers without waiting out its deadline");
    assert_eq!(report.kept, vec![chat.id.clone()]);
    assert!(report.stopping.is_empty(), "{report:?}");
    assert!(report.stopped.is_empty(), "{report:?}");
    assert!(
        rig.manager.shared().live.contains(&chat.id),
        "still the terminal's"
    );
    rig.manager.terminal_ended(&chat.id, opened.generation);
    let report = rig
        .manager
        .stop_all(
            super::super::stopping::StopScope::Chats(vec![chat.id.clone()]),
            std::time::Duration::from_secs(10),
        )
        .await;
    assert!(report.kept.is_empty(), "{report:?}");
    assert!(report.stopping.is_empty(), "{report:?}");
    assert!(!rig.manager.shared().live.contains(&chat.id));
}

/// A claude chat in a new worktree whose setup command has not run yet.
async fn claude_chat_awaiting_setup(rig: &Rig) -> String {
    let chat = rig
        .store
        .create_chat(crate::chatstore::NewChat {
            agent_host: Default::default(),
            agent_id: "claude".into(),
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
        let _ = saved.send(());
    });
    done.await.unwrap();
    chat.id
}

#[tokio::test]
async fn a_terminal_chat_waits_for_its_worktree_setup_and_opens_when_it_succeeds() {
    use ghost_protocol::worktree_setup::WorktreeSetupState;
    let (_home, rig) = with_home(claude_rig);
    let chat = claude_chat_awaiting_setup(&rig).await;
    let opening = tokio::spawn({
        let manager = rig.manager.clone();
        let chat = chat.clone();
        async move { manager.open_terminal(&chat).await }
    });
    // Setup runs first: the terminal waits for it instead of failing.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!opening.is_finished(), "the terminal waits for setup");
    // Its terminal closed while it waited: the wait ends, and the chat opens again (it is not
    // "already open in the terminal").
    opening.abort();
    let _ = opening.await;
    let opening = tokio::spawn({
        let manager = rig.manager.clone();
        let chat = chat.clone();
        async move { manager.open_terminal(&chat).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !opening.is_finished(),
        "opened again, it waits for setup too"
    );
    rig.manager
        .set_setup_attempt(&chat, Some(1), WorktreeSetupState::Running, None)
        .await
        .unwrap();
    rig.manager
        .set_setup_attempt(&chat, Some(1), WorktreeSetupState::Succeeded, None)
        .await
        .unwrap();
    let opened = tokio::time::timeout(std::time::Duration::from_secs(20), opening)
        .await
        .expect("the terminal opens once setup succeeds")
        .unwrap()
        .expect("opened");
    assert!(session_arg(&argv(&opened), "--session-id=").is_some());
    rig.manager.terminal_ended(&chat, opened.generation);
    // A setup that fails answers a waiting terminal with its reason.
    let (_home, rig) = with_home(claude_rig);
    let chat = claude_chat_awaiting_setup(&rig).await;
    let opening = tokio::spawn({
        let manager = rig.manager.clone();
        let chat = chat.clone();
        async move { manager.open_terminal(&chat).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    rig.manager
        .set_setup_attempt(
            &chat,
            Some(1),
            WorktreeSetupState::Failed,
            Some("Setup exited with code 1. Retry setup.".into()),
        )
        .await
        .unwrap();
    let refused = tokio::time::timeout(std::time::Duration::from_secs(20), opening)
        .await
        .expect("a failed setup answers the terminal")
        .unwrap()
        .err()
        .expect("refused");
    assert!(
        refused.to_string().contains("Setup exited with code 1"),
        "{refused}"
    );
    // Opened again after it failed, it says why at once.
    let again = rig
        .manager
        .open_terminal(&chat)
        .await
        .err()
        .expect("refused");
    assert!(
        again.to_string().contains("Setup exited with code 1"),
        "{again}"
    );
}

/// The session a new chat starts for Chat has no file until its first message, and Claude Code
/// cannot resume it: the terminal starts a session of its own instead.
#[tokio::test]
async fn a_session_claude_code_never_wrote_down_is_not_resumed_in_the_terminal() {
    let unwritten = "01a1b21f-0000-7000-8000-000000000001";
    let (driver, _handle) = ScriptedDriver::new(vec![
        ready_step(unwritten),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let (_home, rig) = with_home(|| claude_rig_with(vec![driver]));
    let chat = rig
        .manager
        .create_chat("claude", &rig.folder)
        .await
        .unwrap();
    for _ in 0..500 {
        if rig.statuses.status(&chat.id) == Some(SessionStatus::Ready) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(rig.statuses.status(&chat.id), Some(SessionStatus::Ready));
    let opened = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        rig.manager.open_terminal(&chat.id),
    )
    .await
    .expect("the terminal opens")
    .unwrap();
    let args = argv(&opened);
    assert_eq!(session_arg(&args, "--resume="), None, "{args:?}");
    let session = session_arg(&args, "--session-id=").expect("a session of its own");
    assert_ne!(session, unwritten);
    rig.manager.terminal_ended(&chat.id, opened.generation);
}

#[tokio::test]
async fn a_chat_whose_agent_has_no_interface_of_its_own_stays_in_chat() {
    let rig = rig(Vec::new(), WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    let refused = rig.manager.open_terminal(&chat.id).await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("no interface of its own")),
        "{:?}",
        refused.err()
    );
}

#[tokio::test]
async fn a_grok_terminal_names_its_session_brings_its_turns_back_and_resumes_it_next_time() {
    let (home, rig) = with_home(|| {
        rig_with_spec(
            ghost_drivers::testing::ScriptedDriverFactory::new(Vec::new()),
            WriterConfig::STANDARD,
            Knobs::STANDARD,
            ghost_agents::get("grok").expect("grok").clone(),
        )
    });
    let chat = rig.manager.create_chat("grok", &rig.folder).await.unwrap();
    let opened = rig.manager.open_terminal(&chat.id).await.unwrap();
    let args = argv(&opened);
    // Grok Build's own interface: none of the ACP server's arguments.
    assert!(
        !args.iter().any(|a| a == "agent" || a == "stdio"),
        "{args:?}"
    );
    assert_eq!(opened.exit_input, "");
    let at = args
        .iter()
        .position(|a| a == "--session-id")
        .expect("a new session's ID");
    let session = args[at + 1].clone();
    assert!(uuid::Uuid::parse_str(&session).is_ok(), "{session}");
    // Grok Build writes its history as the person works in the terminal.
    let folder = home
        .path()
        .join(".grok")
        .join("sessions")
        .join("C%3A%5Cwork%5Capp")
        .join(&session);
    std::fs::create_dir_all(&folder).unwrap();
    let history = [
        serde_json::json!({"type":"system","content":"You are Grok Build"}),
        serde_json::json!({"type":"user","content":[{"type":"text","text":"<rules>"}],"synthetic_reason":"rules"}),
        serde_json::json!({"type":"user","content":[{"type":"text","text":"Tidy the README"}],"prompt_index":0}),
        serde_json::json!({"type":"assistant","content":"Tidied."}),
    ]
    .iter()
    .map(|line| line.to_string() + "
")
    .collect::<String>();
    std::fs::write(folder.join("chat_history.jsonl"), history).unwrap();
    rig.manager.terminal_ended(&chat.id, opened.generation);
    let again = reopened(&rig, &chat.id).await;
    let args = argv(&again);
    assert!(
        args.windows(2)
            .any(|w| w[0] == "--resume" && w[1] == session),
        "{args:?}"
    );
    assert_eq!(
        conversation(&rig, &chat.id).await,
        ["you: Tidy the README", "agent: Tidied."]
    );
}

/// The chat's user messages and answers, in order, from its saved history.
async fn conversation(rig: &Rig, chat: &str) -> Vec<String> {
    let page = rig.manager.history(chat, u32::MAX, None).await.unwrap();
    page.items
        .iter()
        .filter_map(|item| match &item.event {
            GhostEvent::UserMessageDelta { text, .. } => Some(format!("you: {text}")),
            GhostEvent::MessageDelta { text, .. } => Some(format!("agent: {text}")),
            _ => None,
        })
        .collect()
}

/// Waits until the chat can open its terminal again: the last one has finished ending.
async fn reopened(rig: &Rig, chat: &str) -> TerminalOpened {
    for _ in 0..200 {
        if let Ok(opened) = rig.manager.open_terminal(chat).await {
            return opened;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the terminal never finished ending");
}

#[tokio::test]
async fn a_terminals_conversation_is_recorded_once_it_ends_and_only_what_it_added() {
    let (home, rig) = with_home(claude_rig);
    let chat = rig
        .manager
        .create_chat("claude", &rig.folder)
        .await
        .unwrap();
    let opened = rig.manager.open_terminal(&chat.id).await.unwrap();
    let session = argv(&opened)
        .iter()
        .find_map(|a| a.strip_prefix("--session-id=").map(str::to_owned))
        .unwrap();
    // Claude Code writes its session file as the person works in the terminal.
    let file = claude_file(home.path(), &session);
    let line = |kind: &str, content: &str| {
        let body = if kind == "user" {
            serde_json::json!({"type":"user","message":{"role":"user","content":content},"timestamp":"2026-11-06T08:00:00.000Z"})
        } else {
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":content}]},"timestamp":"2026-11-06T08:00:05.000Z"})
        };
        body.to_string()
            + "
"
    };
    std::fs::write(
        &file,
        line("user", "Fix the header") + &line("assistant", "Fixed."),
    )
    .unwrap();
    rig.manager.terminal_ended(&chat.id, opened.generation);
    // The next terminal opens only once the turn is recorded, and continues the same session.
    let again = reopened(&rig, &chat.id).await;
    assert!(argv(&again).contains(&format!("--resume={session}")));
    assert_eq!(
        conversation(&rig, &chat.id).await,
        ["you: Fix the header", "agent: Fixed."]
    );
    // The second terminal adds a turn; the first is not recorded twice.
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(&(line("user", "Now the footer") + &line("assistant", "Done.")));
    std::fs::write(&file, text).unwrap();
    rig.manager.terminal_ended(&chat.id, again.generation);
    let _third = reopened(&rig, &chat.id).await;
    assert_eq!(
        conversation(&rig, &chat.id).await,
        [
            "you: Fix the header",
            "agent: Fixed.",
            "you: Now the footer",
            "agent: Done."
        ]
    );
}

#[test]
fn the_agent_list_offers_the_terminal_for_exactly_the_agents_it_can_launch() {
    for spec in ghost_agents::catalog() {
        assert_eq!(
            ghost_agents::has_terminal(spec.id),
            ghost_drivers::terminal::TerminalAgent::of(spec.agent).is_some(),
            "{}",
            spec.id
        );
    }
}

#[tokio::test]
async fn a_cursor_terminal_keeps_its_own_chat_whose_turns_reach_chat_as_a_handoff() {
    let (home, rig) = with_home(|| {
        rig_with_spec(
            ghost_drivers::testing::ScriptedDriverFactory::new(Vec::new()),
            WriterConfig::STANDARD,
            Knobs::STANDARD,
            ghost_agents::get("cursor").expect("cursor").clone(),
        )
    });
    let chat = rig
        .manager
        .create_chat("cursor", &rig.folder)
        .await
        .unwrap();
    let opened = rig.manager.open_terminal(&chat.id).await.unwrap();
    let args = argv(&opened);
    assert!(
        !args.iter().any(|a| a == "acp" || a.starts_with("--resume")),
        "{args:?}"
    );
    // Cursor names its new chat and writes it down as the person works.
    let id = "f2538249-b0ef-43a7-bb25-69d5c4040056";
    let meta = home.path().join(".cursor/chats/workspace").join(id);
    std::fs::create_dir_all(&meta).unwrap();
    std::fs::write(
        meta.join("meta.json"),
        serde_json::json!({
            "createdAtMs": crate::time::now_millis(),
            "cwd": rig.folder,
            "hasConversation": true,
        })
        .to_string(),
    )
    .unwrap();
    let transcript = home
        .path()
        .join(".cursor/projects/C-work-app/agent-transcripts")
        .join(id);
    std::fs::create_dir_all(&transcript).unwrap();
    let lines = [
        serde_json::json!({"role":"user","message":{"content":[{"type":"text","text":"<timestamp>now</timestamp>\n<user_query>\nList the files\n</user_query>"}]}}),
        serde_json::json!({"role":"assistant","message":{"content":[{"type":"text","text":"There are two."}]}}),
        serde_json::json!({"type":"turn_ended","status":"success"}),
    ];
    std::fs::write(
        transcript.join(format!("{id}.jsonl")),
        lines
            .iter()
            .map(|line| line.to_string() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    rig.manager.terminal_ended(&chat.id, opened.generation);
    // The next terminal continues the terminal's own Cursor chat (Chat keeps its own session).
    let again = reopened(&rig, &chat.id).await;
    assert!(
        argv(&again).contains(&format!("--resume={id}")),
        "{:?}",
        argv(&again)
    );
    assert_eq!(
        conversation(&rig, &chat.id).await,
        ["you: List the files", "agent: There are two."]
    );
    let page = rig.manager.history(&chat.id, u32::MAX, None).await.unwrap();
    assert!(page.items.iter().any(|item| matches!(
        &item.event,
        GhostEvent::Notice { message, .. } if message.contains("next Chat message includes the terminal's turns")
    )));
}
