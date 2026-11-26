//! The demo agent end to end through the desktop's own startup (spec 7.2): `boot` on a fresh data
//! folder, then `start_chats` with the real chat store, leftover registry, sweep, shared redactor,
//! protocol logs and session manager, over [`Outside::over_env`], which builds the real detector,
//! process spawner and `StandardDriverFactory`. Detection uses an empty `PATH`. Checkpoints use a
//! separate Git-only `PATH` and fixture HOME, so no real agent or login shell is run.
//!
//! A chat is created with the demo agent, prompts run to the end of their turns, and the stored
//! turns are read back through the history API: the same transcript the live frames showed, and
//! again after a restart on the same data folder, from the database alone. Two more tests run the
//! demo agent's `/ignore-cancel` (A13) and `/background` (A14), which package C's tests could only
//! script before the demo agent's driver was merged.

// Helpers in an integration test count as test code only inside a `#[cfg(test)]` item (clippy.toml).
#[cfg(test)]
mod end_to_end {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use ghost_core::paths::DataRoot;
    use ghost_core::session::{FrameSink, SessionManager, SinkGone};
    use ghost_desktop::boot::{Booted, boot};
    use ghost_desktop::sessions::{
        ChatRuntime, Outside, QUIT_SESSIONS_DEADLINE, QUIT_STORE_DEADLINE, RUN_DIR, start_chats,
    };
    use ghost_process::{
        EnvKey, EnvMap, EnvResolver, EnvSource, ResolvedEnv, find_exe, process_env,
    };
    use ghost_protocol::session::{Availability, HistoryItem, HistoryPage};
    use ghost_protocol::{
        Autonomy, EventBatch, GhostEvent, NoticeLevel, ResolvedBy, SessionStatus, StopReason,
        ToolKind, ToolStatus,
    };
    use tokio::runtime::Handle;

    /// How long one step may take before the test fails.
    const STEP: Duration = Duration::from_secs(30);

    /// The webview the chat views belong to.
    const OWNER: &str = "main";

    /// What the first prompt asks the demo agent to echo.
    const ECHOED: &str = "Hello from the merged workspace";

    /// A prompt the demo agent answers with its full turn: thinking, two tools, a plan, usage.
    const DEMO_PROMPT: &str = "Tidy up src/main.rs";

    /// A data folder, a chat folder and an env that finds no agent, in one temp folder.
    struct Rig {
        dir: tempfile::TempDir,
        root: DataRoot,
        folder: PathBuf,
        env: Arc<EnvResolver>,
        helper_env: Arc<EnvResolver>,
    }

    impl Rig {
        fn new() -> Rig {
            let dir = tempfile::tempdir().expect("tempdir");
            let root = DataRoot::at(dir.path().join("data"));
            let folder = dir.path().join("work");
            std::fs::create_dir_all(&folder).expect("chat folder");
            let env = no_agents_env(dir.path());
            let git = find_exe("git", &process_env())
                .expect("Git is required for checkpoint integration");
            let helper_env = fixed_env(dir.path(), git.parent().expect("Git folder").to_owned());
            Rig {
                dir,
                root,
                folder,
                env,
                helper_env,
            }
        }

        /// `<data root>/run`, where the leftover registry keeps this instance's file.
        fn run_dir(&self) -> PathBuf {
            self.dir.path().join("data").join(RUN_DIR)
        }

        /// The desktop's startup on the rig's data folder, with the real outside parts.
        async fn start(&self) -> ChatRuntime {
            let mut booted = boot_free(&self.root).await;
            assert_eq!(booted.fatal, None, "startup failed");
            let env = Arc::clone(&self.env);
            let helper_env = Arc::clone(&self.helper_env);
            let chats = start_chats(
                &mut booted,
                "0.1.0",
                &Handle::current(),
                move |registry, _redactor, data_dir, rt| {
                    let mut outside = Outside::over_env(env, registry, data_dir, rt);
                    // Detection must never discover installed agents. Only checkpoint helpers
                    // need the installed Git directory, and they use the fixture's home.
                    outside.env = helper_env;
                    outside
                },
            )
            .expect("the chats start");
            assert!(
                booted.store.is_none() && booted.owner.is_none(),
                "the store and the owner lock moved into the chat store"
            );
            chats
        }
    }

    /// An env that finds no agent: `PATH` is an empty folder, `HOME` one of the test's own.
    fn no_agents_env(dir: &Path) -> Arc<EnvResolver> {
        let bin = dir.join("empty-bin");
        std::fs::create_dir_all(&bin).expect("bin folder");
        fixed_env(dir, bin)
    }

    fn fixed_env(dir: &Path, bin: PathBuf) -> Arc<EnvResolver> {
        let home = dir.join("home");
        std::fs::create_dir_all(&home).expect("home folder");
        let mut vars = EnvMap::new();
        vars.insert(EnvKey::from("PATH"), bin.into_os_string());
        vars.insert(EnvKey::from("HOME"), home.clone().into_os_string());
        vars.insert(EnvKey::from("USERPROFILE"), home.into_os_string());
        if let Some(root) = process_env().get(&EnvKey::from("SystemRoot")) {
            vars.insert(EnvKey::from("SystemRoot"), root.clone());
        }
        EnvResolver::fixed(ResolvedEnv {
            vars,
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        })
    }

    /// Boots `root` as a launch does, waiting out an owner lock the last run is still releasing.
    async fn boot_free(root: &DataRoot) -> Booted {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let booted = boot(Ok(root.clone()));
            let in_use = booted
                .fatal
                .as_deref()
                .is_some_and(|fatal| fatal.starts_with("Ghost is already running"));
            if !in_use || Instant::now() >= deadline {
                return booted;
            }
            drop(booted);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Quits the way the app's quit flow does: sessions, then the store, then the registry file.
    async fn quit(chats: ChatRuntime) {
        let summary = chats.manager.shutdown_all(QUIT_SESSIONS_DEADLINE).await;
        assert_eq!(summary.forced, 0, "every session stopped on its own");
        assert!(
            chats.store.shutdown(QUIT_STORE_DEADLINE).await,
            "the store saved everything"
        );
        chats.registry.close().expect("the registry file goes");
    }

    /// This process's leftover registry files under `run`.
    fn own_registry_files(run: &Path) -> Vec<PathBuf> {
        let prefix = format!("{}-", std::process::id());
        std::fs::read_dir(run)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One chat view's frames, as its channel receives them: every event with its seq.
    #[derive(Default)]
    struct View {
        events: Mutex<Vec<HistoryItem>>,
    }

    impl View {
        fn items(&self) -> Vec<HistoryItem> {
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    struct ViewSink(Arc<View>);

    impl FrameSink for ViewSink {
        fn send(&self, frame: &EventBatch) -> Result<(), SinkGone> {
            let mut events = self.0.events.lock().unwrap_or_else(PoisonError::into_inner);
            for (seq, event) in (frame.seq_start..).zip(&frame.events) {
                events.push(HistoryItem {
                    seq,
                    turn_id: None,
                    at: 0,
                    event: event.clone(),
                });
            }
            Ok(())
        }

        fn owner(&self) -> &str {
            OWNER
        }
    }

    /// A chat with the demo agent, watched as a chat view does, its agent ready.
    struct Chat {
        id: String,
        view: Arc<View>,
        subscription_id: String,
    }

    async fn open_chat(chats: &ChatRuntime, folder: &Path) -> Chat {
        let summary = chats
            .manager
            .create_chat("mock", folder)
            .await
            .expect("the chat is created");
        let view = Arc::new(View::default());
        let attach = chats
            .manager
            .attach(&summary.id, Arc::new(ViewSink(Arc::clone(&view))))
            .await
            .expect("the view attaches");
        assert!(
            turns(&attach.history.items).is_empty(),
            "a new chat has no turns"
        );
        wait_for_status(&chats.manager, &summary.id, SessionStatus::Ready).await;
        Chat {
            id: summary.id,
            view,
            subscription_id: attach.subscription_id,
        }
    }

    /// A turn reduced to what it says: the prompt, the reply, the tools it ran, how it ended.
    #[derive(Debug, Clone, PartialEq)]
    struct Turn {
        turn_id: String,
        prompt_id: Option<String>,
        user: String,
        reply: String,
        tools: Vec<ToolKind>,
        stop: Option<StopReason>,
    }

    /// The turns of seq-ordered events (history items or live frames): deltas joined in order, so
    /// a compacted turn and its live stream reduce to the same thing. Events outside turns (late
    /// output) are left out.
    fn turns(items: &[HistoryItem]) -> Vec<Turn> {
        let mut turns: Vec<Turn> = Vec::new();
        let mut open = false;
        for item in items {
            match &item.event {
                GhostEvent::TurnStarted { turn_id, prompt_id } => {
                    open = true;
                    turns.push(Turn {
                        turn_id: turn_id.clone(),
                        prompt_id: prompt_id.clone(),
                        user: String::new(),
                        reply: String::new(),
                        tools: Vec::new(),
                        stop: None,
                    });
                }
                GhostEvent::TurnEnded {
                    turn_id,
                    stop_reason,
                    ..
                } => {
                    let turn = turns.last_mut().expect("a turn");
                    assert_eq!(&turn.turn_id, turn_id, "a turn ends as it started");
                    turn.stop = Some(*stop_reason);
                    open = false;
                }
                event if open => {
                    let turn = turns.last_mut().expect("a turn");
                    match event {
                        GhostEvent::UserMessageDelta { text, .. } => turn.user.push_str(text),
                        GhostEvent::MessageDelta { text, .. } => turn.reply.push_str(text),
                        GhostEvent::ToolCall(call) => turn.tools.push(call.kind),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        turns
    }

    /// The chat's row in the chat list: its status and whether it needs attention.
    async fn row(manager: &SessionManager, chat_id: &str) -> (SessionStatus, bool) {
        let chat = manager
            .chats()
            .await
            .expect("the chat list")
            .into_iter()
            .find(|chat| chat.id == chat_id)
            .expect("the chat is listed");
        (chat.status, chat.needs_attention)
    }

    async fn status(manager: &SessionManager, chat_id: &str) -> SessionStatus {
        row(manager, chat_id).await.0
    }

    async fn wait_for_status(manager: &SessionManager, chat_id: &str, want: SessionStatus) {
        let deadline = Instant::now() + STEP;
        loop {
            let now = status(manager, chat_id).await;
            if now == want {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the chat never reached {want:?}; it is {now:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Waits until the live frames hold an event that `found` picks.
    async fn wait_for_event(view: &View, what: &str, found: impl Fn(&GhostEvent) -> bool) {
        let deadline = Instant::now() + STEP;
        while !view.items().iter().any(|item| found(&item.event)) {
            assert!(
                Instant::now() < deadline,
                "the live frames never held {what}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Waits until the live frames hold `count` ended turns.
    async fn wait_for_ended_turns(view: &View, count: usize) {
        let deadline = Instant::now() + STEP;
        loop {
            let ended = turns(&view.items())
                .iter()
                .filter(|turn| turn.stop.is_some())
                .count();
            if ended >= count {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the live frames hold {ended} ended turns, not {count}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn history(chats: &ChatRuntime, chat_id: &str) -> HistoryPage {
        let page = chats
            .manager
            .history(chat_id, u32::MAX, None)
            .await
            .expect("the history");
        assert!(!page.has_more, "one page holds the chat");
        assert_eq!(page.skipped, 0, "every row parses");
        assert_eq!(page.unsaved_from, None, "everything is saved");
        page
    }

    /// The span `[TurnStarted, TurnEnded]` of each turn in `items`, by seq.
    fn spans(items: &[HistoryItem]) -> Vec<(String, u32, u32)> {
        let mut spans = Vec::new();
        let mut start = None;
        for item in items {
            match &item.event {
                GhostEvent::TurnStarted { turn_id, .. } => {
                    start = Some((turn_id.clone(), item.seq))
                }
                GhostEvent::TurnEnded { .. } => {
                    let (turn_id, from) = start.take().expect("a turn ends after it starts");
                    spans.push((turn_id, from, item.seq));
                }
                _ => {}
            }
        }
        spans
    }

    /// Every stored row inside a turn's span carries that turn's id, except `untagged` ones: late
    /// output stored outside any turn. (A row after a turn's end may still carry its id: a force
    /// stop's notice follows the `TurnEnded` it caused.)
    fn assert_tagged(page: &HistoryPage, untagged: &[u32]) {
        let spans = spans(&page.items);
        for item in &page.items {
            let Some((turn_id, _, _)) = spans
                .iter()
                .find(|(_, from, to)| (*from..=*to).contains(&item.seq))
            else {
                continue;
            };
            let want = (!untagged.contains(&item.seq)).then(|| turn_id.clone());
            assert_eq!(item.turn_id, want, "row {}: {:?}", item.seq, item.event);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_demo_agent_runs_end_to_end_through_the_real_startup() {
        let rig = Rig::new();
        let run = rig.run_dir();
        let chats = rig.start().await;
        if cfg!(unix) {
            assert_eq!(
                own_registry_files(&run).len(),
                1,
                "the real leftover registry has this instance's file"
            );
        }

        // The real detector shows one entry per agent; alternate transports stay in that
        // entry's connections. With nothing on PATH only the built-in demo agent can run.
        let agents = chats.manager.agents(true).await;
        let ids: Vec<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "hermes",
                "claude",
                "codex",
                "antigravity",
                "cursor",
                "grok",
                "omp",
                "opencode",
                "pi",
                "mock"
            ],
            "the visible agents, without duplicate transport aliases"
        );
        for agent in &agents {
            match (agent.id.as_str(), &agent.availability) {
                ("mock", Availability::Ready { .. }) => {}
                (id, Availability::Missing { .. }) if id != "mock" => {}
                (id, availability) => panic!("{id} reads {availability:?}"),
            }
            let expected_connections: Option<&[&str]> = match agent.id.as_str() {
                "claude" => Some(&["claude", "claude-acp"]),
                "codex" => Some(&["codex", "codex-acp"]),
                "antigravity" => Some(&["antigravity", "agy-stream"]),
                "grok" => Some(&["grok", "grok-stream"]),
                "opencode" => Some(&["opencode", "opencode-v2"]),
                _ => None,
            };
            if let Some(expected) = expected_connections {
                let connections = agent
                    .connections
                    .as_ref()
                    .expect("alternate transports remain available as connections");
                assert_eq!(
                    connections.preferred, None,
                    "{} defaults to Automatic in a fresh profile",
                    agent.id
                );
                assert_eq!(
                    connections.active, None,
                    "{} has no runnable connection on an empty PATH",
                    agent.id
                );
                assert_eq!(
                    connections
                        .options
                        .iter()
                        .map(|connection| connection.id.as_str())
                        .collect::<Vec<_>>(),
                    expected,
                    "{} retains every connection in preference order",
                    agent.id
                );
                assert!(
                    connections.options.iter().all(|connection| matches!(
                        connection.availability,
                        Availability::Missing { .. }
                    )),
                    "{} cannot run a hidden connection either",
                    agent.id
                );
            } else {
                assert!(
                    agent.connections.is_none(),
                    "{} has only its single connection",
                    agent.id
                );
            }
        }

        let chat = open_chat(&chats, &rig.folder).await;

        // A prompt with a known answer, then the demo agent's full turn.
        let echo = format!("/echo {ECHOED}");
        let first = chats
            .manager
            .send(&chat.id, echo.clone())
            .await
            .expect("the first prompt is taken");
        assert!(first.queued, "the prompt waits for workspace preparation");
        wait_for_ended_turns(&chat.view, 1).await;
        wait_for_status(&chats.manager, &chat.id, SessionStatus::Ready).await;
        let second = chats
            .manager
            .send(&chat.id, DEMO_PROMPT.into())
            .await
            .expect("the second prompt is taken");
        wait_for_ended_turns(&chat.view, 2).await;

        // The stored turns, read back through the history API, say what the live frames said.
        let page = history(&chats, &chat.id).await;
        assert_tagged(&page, &[]);
        let stored = turns(&page.items);
        assert_eq!(stored.len(), 2, "{stored:#?}");
        assert_eq!(
            stored[0].prompt_id.as_deref(),
            Some(first.prompt_id.as_str())
        );
        assert_eq!(stored[0].user, echo);
        assert_eq!(stored[0].reply, ECHOED);
        assert_eq!(stored[0].stop, Some(StopReason::EndTurn));
        assert_eq!(
            stored[1].prompt_id.as_deref(),
            Some(second.prompt_id.as_str())
        );
        assert_eq!(stored[1].user, DEMO_PROMPT);
        assert!(!stored[1].reply.is_empty(), "the demo turn answers");
        assert_eq!(stored[1].tools, [ToolKind::Read, ToolKind::Edit]);
        assert_eq!(stored[1].stop, Some(StopReason::EndTurn));
        assert_eq!(stored, turns(&chat.view.items()), "stored and live agree");

        // Quit, then start again on the same data folder: the turns come back from the database
        // alone, and reading them starts no agent.
        chats.manager.detach(&chat.subscription_id);
        quit(chats).await;
        assert!(
            own_registry_files(&run).is_empty(),
            "the registry file went at quit"
        );

        let chats = rig.start().await;
        assert_eq!(
            status(&chats.manager, &chat.id).await,
            SessionStatus::Stopped
        );
        let page = history(&chats, &chat.id).await;
        assert_tagged(&page, &[]);
        assert_eq!(turns(&page.items), stored, "the same turns after a restart");
        assert_eq!(
            status(&chats.manager, &chat.id).await,
            SessionStatus::Stopped,
            "reading history starts no agent"
        );
        quit(chats).await;
        assert!(own_registry_files(&run).is_empty());
    }

    /// A13 with the demo agent's `/ignore-cancel`: the chat stays `Cancelling` while the agent
    /// ignores the cancel, and a force stop ends the turn as interrupted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_ignored_cancel_stays_cancelling_until_a_force_stop() {
        let rig = Rig::new();
        let chats = rig.start().await;
        let chat = open_chat(&chats, &rig.folder).await;
        chats
            .manager
            .send(&chat.id, "/ignore-cancel".into())
            .await
            .expect("the prompt is taken");
        wait_for_event(&chat.view, "the agent's first words", |event| {
            matches!(event, GhostEvent::MessageDelta { .. })
        })
        .await;
        chats
            .manager
            .interrupt(&chat.id)
            .await
            .expect("the interrupt");
        wait_for_status(&chats.manager, &chat.id, SessionStatus::Cancelling).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(
            status(&chats.manager, &chat.id).await,
            SessionStatus::Cancelling,
            "the agent ignored the cancel, and nothing stopped it on its own"
        );
        chats
            .manager
            .force_stop(&chat.id)
            .await
            .expect("the force stop");
        wait_for_status(&chats.manager, &chat.id, SessionStatus::Stopped).await;
        wait_for_ended_turns(&chat.view, 1).await;

        let page = history(&chats, &chat.id).await;
        assert_tagged(&page, &[]);
        let stored = turns(&page.items);
        assert_eq!(stored.len(), 1, "{stored:#?}");
        assert_eq!(stored[0].user, "/ignore-cancel");
        assert!(stored[0].reply.starts_with("Working on it"), "{stored:#?}");
        assert_eq!(stored[0].stop, Some(StopReason::Interrupted));
        assert!(page.items.iter().any(|item| matches!(
            &item.event,
            GhostEvent::Notice { level: NoticeLevel::Warning, message }
                if message == "Force stopped: Ghost ended the agent's processes."
        )));
        assert_eq!(stored, turns(&chat.view.items()), "stored and live agree");
        chats.manager.detach(&chat.subscription_id);
        quit(chats).await;
    }

    /// A14 with the demo agent's `/background` and a prompt queued behind it: the output the agent
    /// sends after the turn's reply is stored outside any turn. Depending on preparation duration,
    /// the next turn is preparing or running; the request survives that turn and can be answered.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_output_is_stored_outside_any_turn() {
        let rig = Rig::new();
        let chats = rig.start().await;
        let chat = open_chat(&chats, &rig.folder).await;
        // Ask, so the policy answers nothing: the background request must wait for the user.
        chats
            .manager
            .set_autonomy(&chat.id, Autonomy::Ask)
            .await
            .expect("autonomy");
        chats
            .manager
            .send(&chat.id, "/background".into())
            .await
            .expect("the first prompt is taken");
        chats
            .manager
            .send(&chat.id, DEMO_PROMPT.into())
            .await
            .expect("the second prompt is taken");
        wait_for_ended_turns(&chat.view, 2).await;
        wait_for_event(&chat.view, "the background permission request", |event| {
            matches!(event, GhostEvent::PermissionRequested(_))
        })
        .await;
        wait_for_status(&chats.manager, &chat.id, SessionStatus::Ready).await;
        assert_eq!(
            row(&chats.manager, &chat.id).await,
            (SessionStatus::Ready, true),
            "the open card needs attention"
        );

        let page = history(&chats, &chat.id).await;
        let spans = spans(&page.items);
        assert_eq!(spans.len(), 2, "{spans:?}");
        let update = page
            .items
            .iter()
            .find(|item| matches!(&item.event, GhostEvent::ToolUpdate(update) if update.tool_call_id.ends_with(".watch")))
            .expect("the late tool update is stored");
        let request = page
            .items
            .iter()
            .find(|item| matches!(item.event, GhostEvent::PermissionRequested(_)))
            .expect("the background request is stored");
        assert_eq!(update.turn_id, None, "the late update is untagged");
        assert_eq!(request.turn_id, None, "the background request is untagged");
        assert!(
            matches!(&update.event, GhostEvent::ToolUpdate(update) if update.status == Some(ToolStatus::Completed))
        );
        assert!(
            update.seq > spans[0].2 && request.seq > spans[0].2,
            "background output follows its originating turn, untagged: {update:?} in {spans:?}"
        );
        assert_tagged(&page, &[update.seq, request.seq]);
        let GhostEvent::PermissionRequested(asked) = &request.event else {
            unreachable!("found as a permission request");
        };
        assert!(
            !page.items.iter().any(|item| matches!(
                &item.event,
                GhostEvent::PermissionResolved { request_id, .. } if *request_id == asked.request_id
            )),
            "the background card survives the second turn's end"
        );

        // The card still answers, and the agent's answer to it arrives outside any turn too.
        chats
            .manager
            .respond_permission(&chat.id, &asked.request_id, Some("allow".into()))
            .await
            .expect("the answer");
        wait_for_event(&chat.view, "the watcher's notice", |event| {
            matches!(event, GhostEvent::Notice { message, .. } if message == "The watcher wrote its cache.")
        })
        .await;
        let page = history(&chats, &chat.id).await;
        assert!(page.items.iter().any(|item| item.turn_id.is_none()
            && matches!(
                &item.event,
                GhostEvent::PermissionResolved { request_id, by: ResolvedBy::User, .. }
                    if *request_id == asked.request_id
            )));
        assert_eq!(
            row(&chats.manager, &chat.id).await,
            (SessionStatus::Ready, false),
            "nothing waits for the user any more"
        );
        chats.manager.detach(&chat.subscription_id);
        quit(chats).await;
    }
}
