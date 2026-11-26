use super::*;
use ghost_protocol::workspace::{
    ChatMove, ChatRole, ProjectChatCreate, ProjectCreate, ProjectInput, RoleModel, WorkspaceFilter,
};

fn input(name: &str) -> ProjectInput {
    ProjectInput {
        name: name.into(),
        color: Some("#7766aa".into()),
        default_agent: Some("mock".into()),
        default_model: Some("small".into()),
    }
}

async fn project(rig: &Rig, name: &str) -> ghost_protocol::workspace::ProjectSummary {
    rig.manager
        .project_create(ProjectCreate {
            project: input(name),
            folder: rig.folder.to_string_lossy().into_owned(),
        })
        .await
        .expect("project")
}

fn waiting_driver(native: &str) -> (ScriptedDriver, ghost_drivers::testing::ScriptHandle) {
    ScriptedDriver::new(vec![
        ready_step(native),
        Step::Expect(CommandMatch::Shutdown),
    ])
}

struct ObserveModel {
    inner: ScriptedDriver,
    seen: Arc<Mutex<Option<ghost_protocol::session::ConfigValue>>>,
}
impl AgentDriver for ObserveModel {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        *lock(&self.seen) = ctx.setup.desired.get("model").cloned();
        Box::new(self.inner).run(ctx)
    }
}

#[tokio::test]
async fn project_default_model_reaches_startup_and_saved_session_settings_take_precedence() {
    use ghost_protocol::session::ConfigValue;
    let seen = Arc::new(Mutex::new(None));
    let (inner, _) = waiting_driver("project-default");
    let rig = rig_with(
        Arc::new(OneDriver(Mutex::new(Some(Box::new(ObserveModel {
            inner,
            seen: Arc::clone(&seen),
        }))))),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    let project = project(&rig, "Default model").await;
    let chat = rig
        .manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: project.folders[0].id.clone(),
            allow_busy_folder: false,
        })
        .await
        .unwrap();
    status(&rig, &chat.chat.id, SessionStatus::Ready).await;
    assert_eq!(
        *lock(&seen),
        Some(ConfigValue::Choice {
            value: "small".into()
        })
    );
    rig.manager.stop(&chat.chat.id).await.unwrap();
    status(&rig, &chat.chat.id, SessionStatus::Stopped).await;
    rig.manager
        .conversation_turns(&chat.chat.id, None)
        .await
        .unwrap();
    let mut updated = input("Default model");
    updated.default_model = Some("different".into());
    rig.manager
        .project_update(&project.id, updated)
        .await
        .unwrap();
    let saved = rig
        .store
        .load_chat_inner(&chat.chat.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.initial_model, None);
    let config: std::collections::BTreeMap<String, ConfigValue> = serde_json::from_str(
        saved
            .agent_session
            .as_ref()
            .unwrap()
            .config_json
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        config.get("model"),
        Some(&ConfigValue::Choice {
            value: "small".into()
        })
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn projects_update_clear_optional_values_and_reuse_device_folder() {
    let rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    let project = project(&rig, "First").await;
    assert_eq!(project.folders.len(), 1);
    let folder = rig
        .manager
        .project_add_folder(&project.id, &rig.folder.join(".").to_string_lossy())
        .await
        .expect("same folder");
    assert_eq!(folder.id, project.folders[0].id);
    let updated = rig
        .manager
        .project_update(
            &project.id,
            ProjectInput {
                name: "Renamed".into(),
                color: None,
                default_agent: None,
                default_model: None,
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.name, "Renamed");
    assert!(
        updated.color.is_none()
            && updated.default_agent.is_none()
            && updated.default_model.is_none()
    );
    assert!(
        rig.manager
            .project_update(&project.id, input("\n"))
            .await
            .is_err()
    );
    let snapshot = rig
        .manager
        .workspace_snapshot(WorkspaceFilter::default())
        .await
        .expect("snapshot");
    assert_eq!(snapshot.projects, vec![updated]);
}

/// Windows canonical paths are verbatim (`\\?\C:\...`). A project folder is saved and shown in its
/// plain spelling, and adding the same folder in its verbatim spelling finds the saved one.
#[tokio::test]
async fn project_folders_are_saved_in_their_plain_spelling() {
    let rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    let project = project(&rig, "First").await;
    let canonical = std::fs::canonicalize(&rig.folder).expect("canonical");
    let plain = crate::fsutil::plain_path(&canonical);
    let shown = project.folders[0]
        .path
        .clone()
        .expect("a path on this device");
    assert_eq!(shown, plain.to_string_lossy());
    assert!(!shown.starts_with(r"\\?\"), "{shown}");
    let again = rig
        .manager
        .project_add_folder(&project.id, &canonical.to_string_lossy())
        .await
        .expect("the verbatim spelling");
    assert_eq!(again.id, project.folders[0].id);
    assert_eq!(again.path.as_deref(), Some(shown.as_str()));
}

#[tokio::test]
async fn manual_titles_survive_late_agent_titles_and_archived_chats_remain_addressable() {
    let (driver, handle) = waiting_driver("rename");
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let renamed = rig
        .manager
        .chat_rename(&chat.id, "  My title  ")
        .await
        .expect("rename");
    assert_eq!(renamed.chat.title.as_deref(), Some("My title"));
    handle.emit(DriverEvent::Title(Some("Automatic title".into())));
    rig.manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .expect("actor barrier");
    let pinned = rig.manager.chat_pin(&chat.id, true).await.expect("pin");
    assert!(pinned.pinned);
    let archived = rig
        .manager
        .chat_archive(&chat.id, true)
        .await
        .expect("archive");
    assert!(archived.archived);
    assert!(
        rig.manager
            .workspace_snapshot(WorkspaceFilter::default())
            .await
            .expect("active")
            .chats
            .is_empty()
    );
    let all = rig
        .manager
        .workspace_snapshot(WorkspaceFilter {
            include_archived: true,
            project_id: Some(archived.project_id.clone()),
        })
        .await
        .expect("archive list");
    assert_eq!(all.chats.len(), 1);
    assert_eq!(all.chats[0].chat.title.as_deref(), Some("My title"));
    let stored = rig.store.load_chat(&chat.id).await.expect("durable title");
    assert_eq!(
        stored.chat.title_source,
        Some(crate::chatstore::TitleSource::Manual)
    );
    assert_eq!(stored.chat.title.as_deref(), Some("My title"));
    assert!(
        !rig.manager
            .chat_archive(&chat.id, false)
            .await
            .expect("restore")
            .archived
    );
}

#[tokio::test]
async fn active_move_is_refused_and_stopped_move_preserves_primary_folder() {
    let (driver, _) = waiting_driver("move");
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let destination = project(&rig, "Destination").await;
    let request = ChatMove {
        project_id: destination.id.clone(),
        folder_id: None,
        allow_busy_folder: false,
    };
    assert!(
        rig.manager
            .chat_move(&chat.id, request.clone())
            .await
            .expect_err("live move refused")
            .to_string()
            .contains("Stop the chat")
    );
    rig.manager.stop(&chat.id).await.expect("stop");
    eventually("all retired work ends", || {
        !rig.manager.shared().live.contains(&chat.id)
    })
    .await;
    let moved = rig
        .manager
        .chat_move(&chat.id, request)
        .await
        .expect("move");
    assert_eq!(moved.project_id, destination.id);
    assert_eq!(moved.chat.folder, chat.folder);
    assert_eq!(
        rig.store
            .load_chat(&chat.id)
            .await
            .expect("saved")
            .chat
            .folder
            .to_string_lossy(),
        chat.folder
    );
}

#[tokio::test]
async fn busy_folder_is_canonical_and_two_concurrent_creates_cannot_skip_warning() {
    let (driver, _) = waiting_driver("busy");
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let project = project(&rig, "Busy").await;
    let request = ProjectChatCreate {
        agent_id: "mock".into(),
        folder_id: project.folders[0].id.clone(),
        allow_busy_folder: false,
    };
    let (a, b) = tokio::join!(
        rig.manager.chat_project_create(request.clone()),
        rig.manager.chat_project_create(request)
    );
    assert_ne!(a.is_ok(), b.is_ok(), "exactly one start passes preflight");
    let alias = rig
        .manager
        .project_create(ProjectCreate {
            project: input("Alias"),
            folder: rig.folder.join(".").to_string_lossy().into_owned(),
        })
        .await
        .expect("alias project");
    let preview = rig
        .manager
        .chat_create_preview(&alias.folders[0].id)
        .await
        .expect("preview");
    assert_eq!(preview.busy_chats.len(), 1);
    assert_eq!(
        rig.manager
            .workspace_snapshot(WorkspaceFilter::default())
            .await
            .expect("snapshot")
            .chats
            .len(),
        1
    );
}

#[tokio::test]
async fn missing_folder_has_reason_and_cannot_create_a_chat() {
    let rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    let project = project(&rig, "Missing").await;
    std::fs::remove_dir(&rig.folder).expect("remove owned empty fixture folder");
    let preview = rig
        .manager
        .chat_create_preview(&project.folders[0].id)
        .await
        .expect("preview");
    assert_eq!(preview.folder.folder_state, FolderState::Missing);
    assert!(preview.folder.unavailable_reason.is_some());
    let result = rig
        .manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: project.folders[0].id.clone(),
            allow_busy_folder: true,
        })
        .await;
    assert!(result.is_err());
    assert!(
        rig.manager
            .workspace_snapshot(WorkspaceFilter::default())
            .await
            .expect("snapshot")
            .chats
            .is_empty()
    );
}

#[tokio::test]
async fn workspace_reads_saved_projects_while_saving_is_paused() {
    let (config, knobs) = tight();
    let (driver, _) = flooding_driver();
    let rig = rig(vec![driver], config, knobs);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    fill_disk(&rig.store);
    rig.manager
        .send(&chat.id, "fill fixture database".into())
        .await
        .expect("send");
    paused(&rig).await;
    let snapshot = tokio::time::timeout(
        Duration::from_secs(2),
        rig.manager.workspace_snapshot(WorkspaceFilter::default()),
    )
    .await
    .expect("saved read answers")
    .expect("snapshot");
    assert_eq!(snapshot.chats.len(), 1);
    assert!(
        rig.manager
            .chat_rename(&chat.id, "No late rename")
            .await
            .is_err()
    );
    free_disk(&rig.store);
}
#[tokio::test]
async fn codex_startup_recovery_is_always_asked_and_saves_only_successful_effective_model() {
    use ghost_drivers::driver::LoggedCommand;
    use ghost_protocol::session::{ConfigCategory, ConfigKind, ConfigOption, ConfigValue};
    use ghost_protocol::{UiAnswer, UiPrompt, UiRequest};
    let effective = ConfigOption {
        id: "model".into(),
        name: "Model".into(),
        description: None,
        category: ConfigCategory::Model,
        kind: ConfigKind::Select {
            current: "vendor-default".into(),
            choices: vec![],
        },
        pending: None,
    };
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Expect(CommandMatch::RespondUi {
            request_id: Some("model-recovery".into()),
        }),
        Step::Emit(DriverEvent::Options(vec![effective])),
        ready_step("recovered-model"),
        Step::Expect(CommandMatch::Prompt),
        Step::EndTurn {
            stop_reason: StopReason::EndTurn,
            usage: None,
            error: None,
        },
        Step::Expect(CommandMatch::Shutdown),
    ]);
    let mut spec = ghost_agents::get("mock").unwrap().clone();
    spec.kind = ghost_protocol::DriverKind::Codex;
    let rig = rig_with_spec(
        ghost_drivers::testing::ScriptedDriverFactory::new(vec![driver]),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
        spec,
    );
    let project = project(&rig, "Recovery").await;
    let chat = rig
        .manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: project.folders[0].id.clone(),
            allow_busy_folder: false,
        })
        .await
        .unwrap();
    let id = &chat.chat.id;
    let sink = Arc::new(Sink::default());
    rig.manager.attach(id, sink.clone()).await.unwrap();
    let ack = rig
        .manager
        .send(id, "one visible saved prompt".into())
        .await
        .unwrap();
    assert!(ack.queued);
    handle.emit(DriverEvent::UiRequest {
        turn: None,
        request: UiRequest {
            request_id: "model-recovery".into(),
            prompt: UiPrompt::Select {
                message: "Try the advertised default?".into(),
                options: vec!["Use default".into(), "Keep original".into()],
            },
        },
    });
    eventually("startup recovery question", || lock(&sink.frames).iter().flat_map(|frame| &frame.events).any(|event| matches!(event, GhostEvent::UiRequest(request) if request.request_id == "model-recovery"))).await;
    assert!(!handle.commands().iter().any(|command| matches!(
        command,
        LoggedCommand::RespondUi { .. } | LoggedCommand::Prompt { .. }
    )));
    rig.manager
        .respond_ui(id, "model-recovery", Some(UiAnswer::Select { index: 0 }))
        .await
        .unwrap();
    handle.wait_for(CommandMatch::Prompt).await;
    rig.manager.stop(id).await.unwrap();
    status(&rig, id, SessionStatus::Stopped).await;
    rig.manager.conversation_turns(id, None).await.unwrap();
    let saved = rig.store.load_chat_inner(id).await.unwrap().unwrap();
    let config: std::collections::BTreeMap<String, ConfigValue> = serde_json::from_str(
        saved
            .agent_session
            .as_ref()
            .unwrap()
            .config_json
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        config.get("model"),
        Some(&ConfigValue::Choice {
            value: "vendor-default".into()
        })
    );
    assert_eq!(
        handle
            .commands()
            .iter()
            .filter(|command| matches!(command, LoggedCommand::Prompt { .. }))
            .count(),
        1
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn failed_codex_startup_recovery_keeps_project_model_and_queued_text() {
    use ghost_protocol::{UiAnswer, UiPrompt, UiRequest};
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Expect(CommandMatch::RespondUi {
            request_id: Some("model-recovery".into()),
        }),
        Step::Exit(DriverExit::StartFailed(
            ghost_drivers::driver::DriverError {
                kind: ghost_protocol::ErrorKind::AgentError,
                message: "The reviewed retry also failed.".into(),
                detail: None,
            },
        )),
    ]);
    let mut spec = ghost_agents::get("mock").unwrap().clone();
    spec.kind = ghost_protocol::DriverKind::Codex;
    let rig = rig_with_spec(
        ghost_drivers::testing::ScriptedDriverFactory::new(vec![driver]),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
        spec,
    );
    let project = project(&rig, "Failed recovery").await;
    let chat = rig
        .manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: project.folders[0].id.clone(),
            allow_busy_folder: false,
        })
        .await
        .unwrap();
    let id = &chat.chat.id;
    let sink = Arc::new(Sink::default());
    rig.manager.attach(id, sink.clone()).await.unwrap();
    assert!(
        rig.manager
            .send(id, "retained visible prompt".into())
            .await
            .unwrap()
            .queued
    );
    handle.emit(DriverEvent::UiRequest {
        turn: None,
        request: UiRequest {
            request_id: "model-recovery".into(),
            prompt: UiPrompt::Select {
                message: "Try the advertised default?".into(),
                options: vec!["Use default".into(), "Keep original".into()],
            },
        },
    });
    eventually("recovery card", || lock(&sink.frames).iter().flat_map(|frame| &frame.events).any(|event| matches!(event, GhostEvent::UiRequest(request) if request.request_id == "model-recovery"))).await;
    rig.manager
        .respond_ui(id, "model-recovery", Some(UiAnswer::Select { index: 0 }))
        .await
        .unwrap();
    status(&rig, id, SessionStatus::Failed).await;
    rig.manager.conversation_turns(id, None).await.unwrap();
    let saved = rig.store.load_chat_inner(id).await.unwrap().unwrap();
    assert!(saved.agent_session.is_none());
    assert_eq!(saved.initial_model.as_deref(), Some("small"));
    assert_eq!(saved.queue.len(), 1);
    assert_eq!(saved.queue[0].text, "retained visible prompt");
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, ghost_drivers::driver::LoggedCommand::Prompt { .. }))
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn an_idle_agent_does_not_make_its_folder_busy() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("idle-here"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let knobs = Knobs {
        busy_settle: Duration::from_millis(400),
        ..Knobs::STANDARD
    };
    let rig = rig(vec![driver], WriterConfig::STANDARD, knobs);
    let project = project(&rig, "Idle").await;
    let folder_id = project.folders[0].id.clone();
    let chat = rig
        .manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: folder_id.clone(),
            allow_busy_folder: false,
        })
        .await
        .expect("chat");
    status(&rig, &chat.chat.id, SessionStatus::Ready).await;
    // Just started, it is likely to get the next message, so the folder counts as busy...
    assert_eq!(
        rig.manager
            .chat_create_preview(&folder_id)
            .await
            .expect("preview")
            .busy_chats
            .len(),
        1
    );
    // ...and once it has idled a moment, it does not: it edits nothing until it is given work.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let preview = rig
        .manager
        .chat_create_preview(&folder_id)
        .await
        .expect("preview");
    assert!(preview.busy_chats.is_empty(), "{:?}", preview.busy_chats);
    assert!(rig.manager.shared().live.contains(&chat.chat.id));
    // A turn makes it busy again.
    rig.manager
        .send(&chat.chat.id, "work".into())
        .await
        .expect("send");
    handle.wait_for(CommandMatch::Prompt).await;
    let busy = rig
        .manager
        .chat_create_preview(&folder_id)
        .await
        .expect("preview")
        .busy_chats;
    assert_eq!(busy.len(), 1);
    assert_eq!(busy[0].id, chat.chat.id);
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

/// A project folder that does not answer (a disconnected network share) matches nothing, and the
/// picker hears back within the folder check's wait instead of hanging with it.
#[tokio::test]
async fn a_folder_that_does_not_answer_never_holds_up_the_picker() {
    let rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    let here = project(&rig, "Here").await;
    let share = rig._dir.path().join("share");
    std::fs::create_dir(&share).unwrap();
    // One spelling throughout, as the app stores and picks it (macOS's /private/var, Windows' long
    // names without the \\?\ prefix): the slow-folder hook compares paths exactly.
    let share = crate::fsutil::plain_path(&share.canonicalize().unwrap());
    rig.manager
        .project_create(ProjectCreate {
            project: input("On the share"),
            folder: share.to_string_lossy().into_owned(),
        })
        .await
        .expect("share project");
    let slow = rig.manager.slow_folder(&share, Duration::from_secs(3));
    let started = std::time::Instant::now();
    let found = rig
        .manager
        .workspace_folder_matches(&rig.folder.to_string_lossy())
        .await
        .expect("matches");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "answered in {:?}",
        started.elapsed()
    );
    assert_eq!(found.matches.len(), 1);
    assert_eq!(found.matches[0].project_id, here.id);
    assert!(slow.load(Ordering::SeqCst) >= 1, "the share was asked");
    // Picking the share itself while it does not answer says so, in time.
    let refused = rig
        .manager
        .workspace_folder_matches(&share.to_string_lossy())
        .await
        .expect_err("refused");
    assert!(refused.to_string().contains("did not answer"), "{refused}");
}

#[tokio::test]
async fn a_differently_spelled_pick_resolves_to_its_project() {
    let rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    let first = project(&rig, "First").await;
    // The same folder in a second project, added there on purpose.
    let second = rig
        .manager
        .project_create(ProjectCreate {
            project: input("Second"),
            folder: rig.folder.to_string_lossy().into_owned(),
        })
        .await
        .expect("second project");
    let stored = first.folders[0].path.clone().expect("path");
    std::fs::create_dir(rig.folder.join("sub")).unwrap();
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut spellings = vec![rig.folder.join("."), rig.folder.join("sub").join("..")];
    #[cfg(unix)]
    {
        let link = rig._dir.path().join("linked project");
        std::os::unix::fs::symlink(&rig.folder, &link).unwrap();
        spellings.push(link);
    }
    for spelling in &spellings {
        let found = rig
            .manager
            .workspace_folder_matches(&spelling.to_string_lossy())
            .await
            .expect("matches");
        assert_eq!(found.path, stored, "{spelling:?}");
        assert_eq!(found.worktree, None);
        // No chats yet: the workspace's order.
        assert_eq!(
            found
                .matches
                .iter()
                .map(|found| (found.project_name.as_str(), found.folder_id.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("First", first.folders[0].id.as_str()),
                ("Second", second.folders[0].id.as_str())
            ],
            "{spelling:?}"
        );
    }
    // The project whose chat in that folder was active last comes first.
    rig.manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: second.folders[0].id.clone(),
            allow_busy_folder: true,
        })
        .await
        .expect("chat");
    let found = rig
        .manager
        .workspace_folder_matches(&spellings[0].to_string_lossy())
        .await
        .expect("matches");
    assert_eq!(found.matches[0].project_id, second.id);
    assert!(found.matches[0].last_chat_at.is_some());
    assert_eq!(found.matches[1].last_chat_at, None);
    // Another folder is in no project, and a file or a relative path is no folder at all.
    let elsewhere = rig._dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let none = rig
        .manager
        .workspace_folder_matches(&elsewhere.to_string_lossy())
        .await
        .expect("matches");
    assert!(none.matches.is_empty());
    let file = rig._dir.path().join("notes.txt");
    std::fs::write(&file, "notes").unwrap();
    assert!(
        rig.manager
            .workspace_folder_matches(&file.to_string_lossy())
            .await
            .is_err()
    );
    assert!(
        rig.manager
            .workspace_folder_matches("relative/folder")
            .await
            .is_err()
    );
    let _ = rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_chats_roles_are_checked_and_kept_as_agents() {
    let (driver, _handle) = waiting_driver("roles");
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig
        .manager
        .create_chat("mock", &rig.folder)
        .await
        .expect("chat");
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let role = |agent: &str, model: Option<&str>, effort: Option<&str>| RoleModel {
        agent: agent.into(),
        model: model.map(Into::into),
        effort: effort.map(Into::into),
    };
    // A connection is kept as its agent, as the default agent is.
    let set = rig
        .manager
        .chat_set_role(
            &chat.id,
            ChatRole::Plan,
            Some(role("claude-acp", Some("claude-fable-5-2"), Some("high"))),
        )
        .await
        .expect("plan role");
    assert_eq!(
        set.roles.plan,
        Some(role("claude", Some("claude-fable-5-2"), Some("high")))
    );
    assert_eq!(set.roles.build, None);
    // What is not an identifier is refused, and nothing changes.
    let long_agent = "a".repeat(129);
    let long_model = "m".repeat(257);
    for bad in [
        role("", None, None),
        role(&long_agent, None, None),
        role("codex", Some("line\nbreak"), None),
        role("codex", Some(&long_model), None),
        role("codex", None, Some("")),
    ] {
        assert!(
            rig.manager
                .chat_set_role(&chat.id, ChatRole::Build, Some(bad))
                .await
                .is_err()
        );
    }
    assert!(
        rig.manager
            .chat_set_role("missing", ChatRole::Build, Some(role("codex", None, None)))
            .await
            .is_err()
    );
    let snapshot = rig
        .manager
        .workspace_snapshot(WorkspaceFilter::default())
        .await
        .expect("snapshot");
    let listed = snapshot
        .chats
        .iter()
        .find(|entry| entry.chat.id == chat.id)
        .expect("listed");
    assert_eq!(listed.roles.build, None);
    assert!(listed.roles.plan.is_some());
    let cleared = rig
        .manager
        .chat_set_role(&chat.id, ChatRole::Plan, None)
        .await
        .expect("clear");
    assert_eq!(cleared.roles.plan, None);
}
