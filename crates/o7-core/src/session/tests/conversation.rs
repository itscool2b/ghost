use super::*;
use o7_drivers::driver::{LoggedCommand, NativeStartOptions};
use o7_drivers::testing::ScriptHandle;
use o7_protocol::{PromptPart, conversation::ContinuationAction};

fn turn_driver(native: &str, answer: &str) -> (ScriptedDriver, ScriptHandle) {
    let mut ready = ready_step(native);
    if let Step::Emit(DriverEvent::Ready(info)) = &mut ready {
        info.capabilities.resume = true;
        info.capabilities.fork = true;
        info.capabilities.images = true;
    }
    ScriptedDriver::new(vec![
        ready,
        Step::Expect(CommandMatch::Prompt),
        delta(answer),
        Step::EmitInTurn(DriverEvent::Event {
            turn: None,
            event: O7Event::MessageCompleted {
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

async fn stop(rig: &Rig, chat: &str) {
    rig.manager.stop(chat).await.expect("stop");
    eventually("driver retired", || {
        !rig.manager.shared().live.contains(chat)
    })
    .await;
}

async fn turn(rig: &Rig, chat: &str, handle: &ScriptHandle, text: &str) -> String {
    rig.manager.send(chat, text.into()).await.expect("send");
    handle.wait_for(CommandMatch::Prompt).await;
    status(rig, chat, SessionStatus::Ready).await;
    rig.manager
        .conversation_turns(chat, None)
        .await
        .expect("turns")
        .turns[0]
        .turn_id
        .clone()
}

#[tokio::test]
async fn retry_preserves_source_and_exact_visible_handoff_without_recursive_prefixes() {
    let (first, h1) = turn_driver("original-native", "original final answer");
    let (second, h2) = turn_driver("retry-native", "replacement final answer");
    let rig = rig(vec![first, second], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let original = turn(&rig, &chat.id, &h1, "original prompt").await;
    stop(&rig, &chat.id).await;
    let preview = rig
        .manager
        .continuation_preview(
            &chat.id,
            ContinuationAction::Retry {
                turn_id: original.clone(),
            },
        )
        .await
        .expect("preview");
    assert!(!preview.handoff.contains("original final answer"));
    assert_eq!(preview.prompt.as_deref(), Some("original prompt"));
    assert!(h2.commands().is_empty(), "preview sends no model prompt");
    let result = rig
        .manager
        .continuation_apply(&chat.id, &preview.token)
        .await
        .expect("apply");
    assert!(result.prompt.is_some());
    let command = h2.wait_for(CommandMatch::Prompt).await;
    let LoggedCommand::Prompt { parts, .. } = command else {
        panic!("prompt");
    };
    assert!(matches!(&parts[0],PromptPart::Text { text } if text.starts_with(&preview.handoff)));
    assert_eq!(
        parts[1],
        PromptPart::Text {
            text: "original prompt".into()
        }
    );
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert_eq!(
        rig.manager
            .conversation_pending(&chat.id)
            .await
            .expect("pending"),
        None,
        "the retried turn delivered the handoff; the next message carries none"
    );
    let turns = rig
        .manager
        .conversation_turns(&chat.id, None)
        .await
        .expect("turns");
    assert_eq!(turns.turns.len(), 2);
    assert_eq!(turns.turns[1].turn_id, original);
    let editable = rig
        .manager
        .conversation_editable(&chat.id, &turns.turns[0].turn_id)
        .await
        .expect("editable");
    assert_eq!(editable.text, "original prompt");
    assert!(!editable.has_context);
    assert!(
        rig.manager
            .continuation_apply(&chat.id, &preview.token)
            .await
            .is_err(),
        "one-use token"
    );
}

#[tokio::test]
async fn changed_preview_and_live_session_cannot_change_conversation() {
    let (first, h1) = turn_driver("original-native", "answer");
    let rig = rig(vec![first], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let original = turn(&rig, &chat.id, &h1, "original prompt").await;
    let live = rig
        .manager
        .continuation_preview(
            &chat.id,
            ContinuationAction::Fork {
                turn_id: original.clone(),
            },
        )
        .await
        .expect("preview");
    assert!(
        rig.manager
            .continuation_apply(&chat.id, &live.token)
            .await
            .expect_err("live refused")
            .to_string()
            .contains("Stop")
    );
    stop(&rig, &chat.id).await;
    let stale = rig
        .manager
        .continuation_preview(&chat.id, ContinuationAction::Fork { turn_id: original })
        .await
        .expect("preview");
    rig.manager
        .chat_rename(&chat.id, "changed after preview")
        .await
        .expect("rename");
    assert!(
        rig.manager
            .continuation_apply(&chat.id, &stale.token)
            .await
            .expect_err("stale refused")
            .to_string()
            .contains("changed after")
    );
    assert_eq!(rig.manager.chats().await.expect("chats").len(), 1);
}

type Starts = Arc<Mutex<Vec<(String, Option<String>, NativeStartOptions)>>>;

#[tokio::test]
async fn fresh_session_reviews_complete_context_and_waits_for_the_next_visible_prompt() {
    let (first, h1) = turn_driver("missing-native", "saved final answer");
    let (second, h2) = turn_driver("fresh-native", "new answer");
    let (missing, _) = ScriptedDriver::new(vec![Step::Exit(DriverExit::StartFailed(
        o7_drivers::driver::DriverError {
            kind: o7_protocol::ErrorKind::AgentExited,
            message: "The native session is unavailable.".into(),
            detail: None,
        },
    ))]);
    let starts = Arc::new(Mutex::new(Vec::new()));
    let factory = Arc::new(RecordingFactory {
        inner: o7_drivers::testing::ScriptedDriverFactory::new(vec![first, missing, second]),
        starts: Arc::clone(&starts),
    });
    let rig = rig_with(factory, WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let original = turn(&rig, &chat.id, &h1, "saved original prompt").await;
    stop(&rig, &chat.id).await;
    rig.manager
        .start(&chat.id, false)
        .await
        .expect("attempt resume");
    status(&rig, &chat.id, SessionStatus::Failed).await;
    eventually("failed resume retired", || {
        !rig.manager.shared().live.contains(&chat.id)
    })
    .await;
    assert_eq!(lock(&starts)[1].1.as_deref(), Some("missing-native"));
    rig.manager
        .chat_archive(&chat.id, true)
        .await
        .expect("archive source");
    let preview = rig
        .manager
        .continuation_preview(&chat.id, ContinuationAction::FreshSession)
        .await
        .expect("fresh preview");
    assert!(preview.handoff.contains("saved original prompt"));
    assert!(preview.handoff.contains("saved final answer"));
    assert!(!preview.resumes_native && !preview.uses_native_fork && !preview.creates_chat);
    assert!(preview.prompt.is_none());
    assert_eq!(lock(&starts).len(), 2, "preview must not start an agent");
    let applied = rig
        .manager
        .continuation_apply(&chat.id, &preview.token)
        .await
        .expect("confirm handoff");
    assert_eq!(applied.chat_id, chat.id);
    assert!(applied.prompt.is_none());
    assert_eq!(
        lock(&starts).len(),
        2,
        "confirmation sends no prompt or agent start"
    );
    let pending = rig
        .manager
        .conversation_pending(&chat.id)
        .await
        .expect("pending")
        .expect("saved handoff");
    assert_eq!(pending.handoff, preview.handoff);
    let original_turns = rig
        .manager
        .conversation_turns(&chat.id, None)
        .await
        .expect("preserved turns");
    assert_eq!(original_turns.turns.len(), 1);
    assert_eq!(original_turns.turns[0].turn_id, original);
    let workspace = rig
        .manager
        .workspace_snapshot(o7_protocol::workspace::WorkspaceFilter {
            include_archived: true,
            project_id: None,
        })
        .await
        .expect("workspace");
    assert!(
        workspace
            .chats
            .iter()
            .find(|saved| saved.chat.id == chat.id)
            .expect("source")
            .archived
    );
    rig.manager
        .chat_archive(&chat.id, false)
        .await
        .expect("restore before continuing");
    turn(&rig, &chat.id, &h2, "continue now").await;
    {
        let starts = lock(&starts);
        assert_eq!(starts.len(), 3);
        assert!(
            starts[2].1.is_none(),
            "purged native id must never be resumed"
        );
        assert!(!starts[2].2.fork);
    }
    let prompt = h2
        .commands()
        .into_iter()
        .find_map(|command| match command {
            LoggedCommand::Prompt { parts, .. } => Some(parts),
            _ => None,
        })
        .expect("sent prompt");
    assert!(matches!(&prompt[0], PromptPart::Text { text } if text.starts_with(&preview.handoff)));
    assert_eq!(
        prompt[1],
        PromptPart::Text {
            text: "continue now".into()
        }
    );
    assert_eq!(
        rig.manager
            .conversation_turns(&chat.id, None)
            .await
            .expect("turns")
            .turns
            .len(),
        2
    );
}
struct RecordingFactory {
    inner: Arc<dyn DriverFactory>,
    starts: Starts,
}
struct RecordingDriver {
    inner: Box<dyn AgentDriver>,
    starts: Starts,
}
impl DriverFactory for RecordingFactory {
    fn create(&self, kind: o7_protocol::DriverKind) -> Box<dyn AgentDriver> {
        Box::new(RecordingDriver {
            inner: self.inner.create(kind),
            starts: Arc::clone(&self.starts),
        })
    }
}
impl AgentDriver for RecordingDriver {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        lock(&self.starts).push((
            ctx.setup.agent_id.clone(),
            ctx.setup.resume.clone(),
            ctx.setup.native_start.clone(),
        ));
        self.inner.run(ctx)
    }
}

#[tokio::test]
async fn switching_back_resumes_that_agents_session_and_only_hands_off_the_delta() {
    let (first, h1) = turn_driver("native-a", "answer A");
    let (second, h2) = turn_driver("native-b", "answer B");
    let (third, h3) = turn_driver("native-a", "answer A again");
    let starts = Arc::new(Mutex::new(Vec::new()));
    let factory = Arc::new(RecordingFactory {
        inner: o7_drivers::testing::ScriptedDriverFactory::new(vec![first, second, third]),
        starts: Arc::clone(&starts),
    });
    let mut rig = rig_with(factory, WriterConfig::STANDARD, Knobs::STANDARD);
    let mut other = o7_agents::get("mock").expect("mock").clone();
    other.id = "mock-other";
    let fixed = |spec| {
        FixedAgent::new(
            spec,
            o7_protocol::session::Availability::Ready {
                version: None,
                path: "fixture".into(),
            },
            Some(launch()),
        )
    };
    let shared = &mut Arc::get_mut(&mut rig.manager.inner)
        .expect("only owner")
        .shared;
    Arc::get_mut(shared).expect("no actors").deps.detector = Detector::fixed(vec![
        fixed(o7_agents::get("mock").unwrap().clone()),
        fixed(other),
    ]);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    turn(&rig, &chat.id, &h1, "prompt A").await;
    stop(&rig, &chat.id).await;
    let switch = rig
        .manager
        .continuation_preview(
            &chat.id,
            ContinuationAction::SwitchAgent {
                agent_id: "mock-other".into(),
            },
        )
        .await
        .expect("preview B");
    rig.manager
        .continuation_apply(&chat.id, &switch.token)
        .await
        .expect("switch B");
    turn(&rig, &chat.id, &h2, "prompt B").await;
    stop(&rig, &chat.id).await;
    let back = rig
        .manager
        .continuation_preview(
            &chat.id,
            ContinuationAction::SwitchAgent {
                agent_id: "mock".into(),
            },
        )
        .await
        .expect("preview A");
    assert!(back.resumes_native);
    assert!(back.handoff.contains("prompt B") && back.handoff.contains("answer B"));
    assert!(!back.handoff.contains("answer A\n"));
    rig.manager
        .continuation_apply(&chat.id, &back.token)
        .await
        .expect("switch A");
    turn(&rig, &chat.id, &h3, "new A prompt").await;
    assert_eq!(lock(&starts)[2].1.as_deref(), Some("native-a"));
}

/// A chat saved while Claude Code's ACP adapter was an agent of its own (`active_agent` and its
/// session both `claude-acp`) reads as Claude Code, keeps resuming its saved session through the
/// adapter although Automatic now picks the native driver, hands off to Codex and back to that
/// same adapter session, and only a fresh session follows the Connection setting.
#[tokio::test]
async fn a_chat_saved_on_the_adapter_keeps_resuming_through_it_across_handoffs() {
    let (first, h1) = turn_driver("acp-old", "answer through the adapter");
    let (second, h2) = turn_driver("codex-1", "answer from Codex");
    let (third, h3) = turn_driver("acp-old", "back on the adapter");
    let (fourth, _h4) = turn_driver("native-1", "unused");
    let starts = Arc::new(Mutex::new(Vec::new()));
    let factory = Arc::new(RecordingFactory {
        inner: o7_drivers::testing::ScriptedDriverFactory::new(vec![first, second, third, fourth]),
        starts: Arc::clone(&starts),
    });
    let mut rig = rig_with(factory, WriterConfig::STANDARD, Knobs::STANDARD);
    let connection = |id: &'static str| {
        let mut spec = o7_agents::get("mock").expect("mock").clone();
        spec.id = id;
        FixedAgent::new(
            spec,
            o7_protocol::session::Availability::Ready {
                version: None,
                path: "fixture".into(),
            },
            Some(launch()),
        )
    };
    let shared = &mut Arc::get_mut(&mut rig.manager.inner)
        .expect("only owner")
        .shared;
    Arc::get_mut(shared).expect("no actors").deps.detector = Detector::fixed(vec![
        connection("claude"),
        connection("claude-acp"),
        connection("codex"),
    ]);
    // The rows an earlier build wrote for a "Claude Code (ACP)" chat that ran one turn.
    let path = crate::chatstore::stored_path(&rig.folder);
    let (seeded, done) = tokio::sync::oneshot::channel();
    rig.store.with_store(move |store| {
        store
            .conn()
            .execute_batch(&format!(
                "INSERT INTO projects (id,name,created_at,updated_at,device_id) VALUES ('p','work',1,1,'device');
                 INSERT INTO project_folders (id,project_id,display_name,kind,origin_device_id,created_at,updated_at,device_id) VALUES ('f','p','work','local','device',1,1,'device');
                 INSERT INTO folder_locations (folder_id,device_id,path,last_seen_at) VALUES ('f','device','{path}',1);
                 INSERT INTO chats (id,project_id,folder_id,active_agent,created_at,updated_at,device_id) VALUES ('old','p','f','claude-acp',1,1,'device');
                 INSERT INTO agent_sessions (id,chat_id,agent_id,native_session_id,autonomy,cwd,created_at,updated_at,device_id,host) VALUES ('s','old','claude-acp','acp-old','full_auto','{path}',1,1,'device','{{\"type\":\"native\"}}');"
            ))
            .expect("seed");
        let _ = seeded.send(());
    });
    done.await.expect("seeded");
    let listed = rig.manager.chats().await.expect("chats");
    assert_eq!(listed[0].agent_id, "claude", "one Claude Code");
    rig.manager.start("old", false).await.expect("resume");
    status(&rig, "old", SessionStatus::Ready).await;
    turn(&rig, "old", &h1, "after the update").await;
    stop(&rig, "old").await;
    let to_codex = rig
        .manager
        .continuation_preview(
            "old",
            ContinuationAction::SwitchAgent {
                agent_id: "codex".into(),
            },
        )
        .await
        .expect("preview Codex");
    rig.manager
        .continuation_apply("old", &to_codex.token)
        .await
        .expect("switch to Codex");
    turn(&rig, "old", &h2, "Codex's turn").await;
    stop(&rig, "old").await;
    let back = rig
        .manager
        .continuation_preview(
            "old",
            ContinuationAction::SwitchAgent {
                agent_id: "claude".into(),
            },
        )
        .await
        .expect("preview Claude Code");
    assert!(back.resumes_native, "the adapter's saved session resumes");
    assert!(back.handoff.contains("Codex's turn") && back.handoff.contains("answer from Codex"));
    rig.manager
        .continuation_apply("old", &back.token)
        .await
        .expect("switch back");
    turn(&rig, "old", &h3, "Claude Code again").await;
    stop(&rig, "old").await;
    // A fresh session takes the Connection setting: Automatic picks the native driver.
    rig.manager.start("old", true).await.expect("fresh");
    status(&rig, "old", SessionStatus::Ready).await;
    let starts: Vec<(String, Option<String>)> = lock(&starts)
        .iter()
        .map(|(agent, resume, _)| (agent.clone(), resume.clone()))
        .collect();
    assert_eq!(
        starts,
        [
            ("claude-acp".to_owned(), Some("acp-old".to_owned())),
            ("codex".to_owned(), None),
            ("claude-acp".to_owned(), Some("acp-old".to_owned())),
            ("claude".to_owned(), None),
        ]
    );
    let turns = rig
        .manager
        .conversation_turns("old", None)
        .await
        .expect("turns")
        .turns;
    let agents: Vec<&str> = turns.iter().map(|turn| turn.agent_id.as_str()).collect();
    assert_eq!(
        agents,
        ["claude", "codex", "claude"],
        "turns name agents, newest first"
    );
}

#[tokio::test]
async fn fork_copies_only_selected_image_metadata_and_missing_metadata_rolls_back() {
    let (first, h1) = turn_driver("image-source", "answer");
    let rig = rig(vec![first], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("fixture PNG");
    let attachment = rig
        .manager
        .composer_paste(&chat.id, png.into_inner(), None)
        .await
        .expect("import");
    rig.manager
        .composer_send(
            &chat.id,
            o7_protocol::composer::ComposerInput {
                text: "look at this image".into(),
                attachments: vec![attachment.id],
                mentions: vec![],
            },
        )
        .await
        .expect("send");
    h1.wait_for(CommandMatch::Prompt).await;
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let original = rig
        .manager
        .conversation_turns(&chat.id, None)
        .await
        .expect("turns")
        .turns[0]
        .turn_id
        .clone();
    stop(&rig, &chat.id).await;
    let fork = rig
        .manager
        .continuation_preview(
            &chat.id,
            ContinuationAction::Fork {
                turn_id: original.clone(),
            },
        )
        .await
        .expect("preview");
    let result = rig
        .manager
        .continuation_apply(&chat.id, &fork.token)
        .await
        .expect("fork");
    assert!(result.prompt.is_none());
    let load = rig
        .store
        .load_chat(&result.chat_id)
        .await
        .expect("saved fork");
    assert_eq!(
        load.continuation.as_ref().expect("durable handoff").handoff,
        fork.handoff
    );
    assert!(
        load.agent_session.is_none(),
        "fork has a distinct future native session"
    );
    let source = chat.id.clone();
    let destination = result.chat_id.clone();
    rig.store.with_store(move |store| {
        let count: u32 = store.conn().query_row("SELECT COUNT(*) FROM composer_attachments a JOIN composer_attachments b ON a.sha256=b.sha256 AND a.id<>b.id WHERE a.chat_id=?1 AND b.chat_id=?2",rusqlite::params![source,destination],|row| row.get(0)).expect("metadata copied");
        assert_eq!(count,1);
    });
    let stale = rig
        .manager
        .continuation_preview(&chat.id, ContinuationAction::Fork { turn_id: original })
        .await
        .expect("preview");
    let source = chat.id.clone();
    rig.store.with_store(move |store| {
        store
            .conn()
            .execute(
                "DELETE FROM composer_attachments WHERE chat_id=?1",
                [source],
            )
            .expect("remove fixture metadata");
    });
    assert!(
        rig.manager
            .continuation_apply(&chat.id, &stale.token)
            .await
            .expect_err("missing metadata")
            .to_string()
            .contains("image is unavailable")
    );
    assert_eq!(
        rig.manager.chats().await.expect("chats").len(),
        2,
        "failed fork rolled back"
    );
}

#[tokio::test]
async fn latest_qualified_native_fork_uses_a_distinct_native_id() {
    let (first, h1) = turn_driver("source-native", "source answer");
    let (second, h2) = turn_driver("fork-native", "fork answer");
    let starts = Arc::new(Mutex::new(Vec::new()));
    let factory = Arc::new(RecordingFactory {
        inner: o7_drivers::testing::ScriptedDriverFactory::new(vec![first, second]),
        starts: Arc::clone(&starts),
    });
    let mut rig = rig_with(factory, WriterConfig::STANDARD, Knobs::STANDARD);
    let shared = &mut Arc::get_mut(&mut rig.manager.inner)
        .expect("only owner")
        .shared;
    let ready = |id: &str| {
        FixedAgent::new(
            o7_agents::get(id).expect("catalog").clone(),
            o7_protocol::session::Availability::Ready {
                version: None,
                path: "fixture".into(),
            },
            Some(launch()),
        )
    };
    Arc::get_mut(shared).expect("no actors").deps.detector =
        Detector::fixed(vec![ready("claude"), ready("claude-acp")]);
    let chat = rig
        .manager
        .create_chat("claude", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let selected = turn(&rig, &chat.id, &h1, "source prompt").await;
    stop(&rig, &chat.id).await;
    // New sessions now prefer the adapter, but a native fork resumes its native source.
    rig.manager
        .set_agent_connection("claude", Some("claude-acp"))
        .await
        .expect("connection");
    let preview = rig
        .manager
        .continuation_preview(&chat.id, ContinuationAction::Fork { turn_id: selected })
        .await
        .expect("preview");
    assert!(preview.uses_native_fork && preview.handoff.is_empty());
    let fork = rig
        .manager
        .continuation_apply(&chat.id, &preview.token)
        .await
        .expect("fork");
    turn(&rig, &fork.chat_id, &h2, "new fork prompt").await;
    let starts = lock(&starts);
    let (connection, source, options) = &starts[1];
    assert_eq!(connection, "claude", "the source session's connection");
    assert_eq!(source.as_deref(), Some("source-native"));
    assert!(options.fork);
    assert!(
        options.new_session_id.is_some() && options.new_session_id.as_deref() != source.as_deref()
    );
}

pub(super) async fn reopen_manager(rig: &mut Rig) {
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
    let deps = rig.manager.deps();
    let manager = SessionManager::new(ManagerDeps {
        worktrees: deps.worktrees.clone(),
        store: Arc::clone(&deps.store),
        checkpoints: deps.checkpoints.clone(),
        env: Arc::clone(&deps.env),
        spawner: Arc::clone(&deps.spawner),
        detector: Arc::clone(&deps.detector),
        drivers: Arc::clone(&deps.drivers),
        logs: deps.logs.clone(),
        redactor: Arc::clone(&deps.redactor),
        device_id: deps.device_id.clone(),
        app_version: deps.app_version.clone(),
        limits: deps.limits,
        timings: deps.timings,
        rt: deps.rt.clone(),
    });
    manager.status_subscribe(rig.statuses.clone());
    rig.manager = manager;
}

#[tokio::test]
async fn saved_handoff_survives_actor_reconstruction_and_is_consumed_once() {
    let (source, h1) = turn_driver("source", "saved answer");
    let (forked, h2) = turn_driver("fork", "first answer");
    let (later, h3) = turn_driver("fork", "later answer");
    let mut rig = rig(
        vec![source, forked, later],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let source = turn(&rig, &chat.id, &h1, "saved prompt").await;
    stop(&rig, &chat.id).await;
    let preview = rig
        .manager
        .continuation_preview(&chat.id, ContinuationAction::Fork { turn_id: source })
        .await
        .expect("preview");
    let fork = rig
        .manager
        .continuation_apply(&chat.id, &preview.token)
        .await
        .expect("fork");
    reopen_manager(&mut rig).await;
    turn(&rig, &fork.chat_id, &h2, "first prompt").await;
    let LoggedCommand::Prompt { parts, .. } = h2
        .commands()
        .into_iter()
        .find(|command| matches!(command, LoggedCommand::Prompt { .. }))
        .expect("prompt")
    else {
        panic!("prompt")
    };
    assert!(matches!(&parts[0],PromptPart::Text {text} if text.starts_with(&preview.handoff)));
    stop(&rig, &fork.chat_id).await;
    let load = rig.store.load_chat(&fork.chat_id).await.expect("saved");
    assert!(load.continuation.expect("continuation").handoff.is_empty());
    reopen_manager(&mut rig).await;
    turn(&rig, &fork.chat_id, &h3, "later prompt").await;
    let LoggedCommand::Prompt { parts, .. } = h3
        .commands()
        .into_iter()
        .find(|command| matches!(command, LoggedCommand::Prompt { .. }))
        .expect("prompt")
    else {
        panic!("prompt")
    };
    assert_eq!(
        parts,
        vec![PromptPart::Text {
            text: "later prompt".into()
        }]
    );
}

/// Natively, 6 November 2026: a new Claude chat starts its agent before its first message, and
/// Claude Code writes the session's file only with that message, so after a restart its first
/// message failed to start the agent ("No conversation found with session ID"). Such a session
/// starts again under its own id; one that ran a turn still resumes.
#[tokio::test]
async fn a_never_messaged_claude_session_starts_again_under_its_own_id() {
    let home = tempfile::tempdir().unwrap();
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    RIG_ENV.with(|extra| {
        extra.replace(vec![(
            variable.into(),
            home.path().to_string_lossy().into_owned(),
        )])
    });
    let native = uuid::Uuid::now_v7().to_string();
    let (created, _) = ScriptedDriver::new(vec![
        ready_step(&native),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let (first_message, h2) = turn_driver(&native, "first answer");
    let (resumed, _) = ScriptedDriver::new(vec![ready_step(&native)]);
    let starts = Arc::new(Mutex::new(Vec::new()));
    let factory = Arc::new(RecordingFactory {
        inner: o7_drivers::testing::ScriptedDriverFactory::new(vec![
            created,
            first_message,
            resumed,
        ]),
        starts: Arc::clone(&starts),
    });
    let mut rig = rig_with_spec(
        factory,
        WriterConfig::STANDARD,
        Knobs::STANDARD,
        o7_agents::get("claude").expect("claude").clone(),
    );
    RIG_ENV.with(|extra| extra.replace(Vec::new()));
    let chat = rig
        .manager
        .create_chat("claude", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    reopen_manager(&mut rig).await;
    turn(&rig, &chat.id, &h2, "first words").await;
    {
        let starts = lock(&starts);
        assert_eq!(starts[1].1, None, "Claude has no such session to resume");
        assert_eq!(starts[1].2.new_session_id.as_deref(), Some(native.as_str()));
    }
    let loaded = rig.store.load_chat(&chat.id).await.unwrap();
    assert_eq!(
        loaded
            .agent_session
            .and_then(|session| session.native_session_id),
        Some(native.clone()),
        "the chat keeps its session"
    );
    stop(&rig, &chat.id).await;
    rig.manager.start(&chat.id, false).await.expect("start");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let starts = lock(&starts);
    assert_eq!(
        starts[2].1.as_deref(),
        Some(native.as_str()),
        "a session that ran a turn resumes, file or not"
    );
    assert_eq!(starts[2].2.new_session_id, None);
}

#[tokio::test]
async fn a_handoff_carries_the_failing_command_and_its_error_with_secrets_hidden() {
    // Fake secrets, built here so no realistic-looking one sits in the source.
    let flag_value = format!("{}{}", "flag", "value-1234567890");
    let bearer = format!("{}{}", "abcdefghij", "klmnopqrstuv");
    let command = format!(
        "curl -H 'Authorization: Bearer {bearer}' --token {flag_value} https://example.test/api"
    );
    let mut ready = ready_step("tools-native");
    if let Step::Emit(DriverEvent::Ready(info)) = &mut ready {
        info.capabilities.resume = true;
    }
    let tool = |event| Step::EmitInTurn(DriverEvent::Event { turn: None, event });
    let (driver, _handle) = ScriptedDriver::new(vec![
        ready,
        Step::Expect(CommandMatch::Prompt),
        tool(O7Event::ToolCall(o7_protocol::ToolCall {
            tool_call_id: "run-1".into(),
            kind: o7_protocol::ToolKind::Execute,
            title: "Call the API".into(),
            status: o7_protocol::ToolStatus::InProgress,
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "command": command })),
            tool_name: None,
            content: Vec::new(),
            truncated: None,
        })),
        tool(O7Event::ToolUpdate(o7_protocol::ToolUpdate {
            tool_call_id: "run-1".into(),
            status: Some(o7_protocol::ToolStatus::Completed),
            title: None,
            kind: None,
            locations: None,
            raw_input: None,
            tool_name: None,
            content: Some(vec![o7_protocol::ToolContent::Terminal {
                output: "connecting\ncurl: (6) Could not resolve host: example.test\n".into(),
                exit_code: Some(6),
                truncated: None,
            }]),
            truncated: None,
        })),
        delta("The API call failed."),
        Step::EmitInTurn(DriverEvent::Event {
            turn: None,
            event: O7Event::MessageCompleted {
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
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager
        .send(&chat.id, "call the API".into())
        .await
        .expect("send");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    stop(&rig, &chat.id).await;
    let preview = rig
        .manager
        .continuation_preview(&chat.id, ContinuationAction::FreshSession)
        .await
        .expect("preview");
    let handoff = &preview.handoff;
    assert!(
        handoff.contains(&format!("\nWorking folder\n{}\n", rig.folder.display())),
        "{handoff}"
    );
    assert!(handoff.contains("\nCommands run (newest last)\n[turn 1] exit 6 | curl "));
    assert!(handoff.contains("\nLast failing command\n[turn 1] exit 6 | curl "));
    assert!(handoff.contains("First error\ncurl: (6) Could not resolve host: example.test"));
    assert!(handoff.contains("--token <SECRET>"), "{handoff}");
    assert!(!handoff.contains(&flag_value), "the flag's value is hidden");
    assert!(!handoff.contains(&bearer), "the bearer token is hidden");
}
