use super::worktree_guards::enable_worktree_fixture;
use super::*;
use ghost_protocol::git::{
    ChatFolderMode, WorktreeAction, WorktreeChatInput, WorktreeChatPlan, WorktreeChatPreview,
    WorktreeState,
};
use ghost_protocol::workspace::{ProjectChatCreate, ProjectCreate, ProjectInput, ProjectSummary};
use ghost_protocol::worktree_setup::{SetupCommand, WorktreeSetupInput};

fn idle_driver(native: &str) -> ScriptedDriver {
    ScriptedDriver::new(vec![
        ready_step(native),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ])
    .0
}

async fn project(rig: &Rig, folder: &Path) -> ProjectSummary {
    rig.manager
        .project_create(ProjectCreate {
            project: ProjectInput {
                name: "Parallel".into(),
                color: None,
                default_agent: Some("mock".into()),
                default_model: None,
            },
            folder: folder.to_string_lossy().into_owned(),
        })
        .await
        .expect("project")
}

fn input(project: &ProjectSummary) -> WorktreeChatInput {
    WorktreeChatInput {
        folder_id: project.folders[0].id.clone(),
        agent_id: "mock".into(),
        start_branch: None,
        branch: None,
        title: None,
        bring_changes: false,
        run_setup: true,
    }
}

async fn plan(rig: &Rig, input: WorktreeChatInput) -> WorktreeChatPlan {
    match rig
        .manager
        .worktree_chat_preview(input)
        .await
        .expect("preview")
    {
        WorktreeChatPreview::Worktree { plan } => *plan,
        WorktreeChatPreview::NotGitRepository { message, .. } => panic!("direct: {message}"),
    }
}

fn canonical(path: impl AsRef<Path>) -> PathBuf {
    path.as_ref().canonicalize().expect("canonical")
}

#[tokio::test]
async fn worktree_chat_is_created_in_its_own_worktree_while_the_source_is_busy() {
    let (working, working_handle) = ScriptedDriver::new(vec![
        ready_step("direct"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let mut rig = rig(
        vec![working, idle_driver("isolated")],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    enable_worktree_fixture(&mut rig).await;
    let project = project(&rig, &rig.folder).await;
    let direct = rig
        .manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: project.folders[0].id.clone(),
            allow_busy_folder: false,
        })
        .await
        .expect("direct chat");
    status(&rig, &direct.chat.id, SessionStatus::Ready).await;
    rig.manager
        .send(&direct.chat.id, "work in the source".into())
        .await
        .expect("send");
    working_handle.wait_for(CommandMatch::Prompt).await;
    let preview = plan(
        &rig,
        WorktreeChatInput {
            title: Some("Parallel fix".into()),
            ..input(&project)
        },
    )
    .await;
    assert_eq!(preview.branch, "ghost/parallel-fix");
    assert_eq!(preview.starting_branch, "main");
    assert_eq!(preview.chip.mode, ChatFolderMode::Worktree);
    assert_eq!(
        preview.chip.label,
        "Worktree from main on ghost/parallel-fix"
    );
    assert!(preview.setup.is_none());
    assert!(!Path::new(&preview.path).exists());
    // Bringing changes reads the source, so a source whose agent works is refused for that
    // variant only, naming the chat.
    let copying = plan(
        &rig,
        WorktreeChatInput {
            bring_changes: true,
            ..input(&project)
        },
    )
    .await;
    match rig.manager.worktree_chat_apply(&copying.token).await {
        Err(SessionError::FolderBusy { message, blockers }) => {
            assert_eq!(blockers.chats.len(), 1, "{message}");
            assert_eq!(blockers.chats[0].chat_id, direct.chat.id);
            assert!(message.contains("is working in this folder"), "{message}");
        }
        Err(other) => panic!("expected the working chat to be named, got {other}"),
        Ok(_) => panic!("copying from a working source must wait"),
    }
    assert!(!Path::new(&copying.path).exists());
    let result = rig
        .manager
        .worktree_chat_apply(&preview.token)
        .await
        .expect("apply");
    assert!(result.started);
    assert!(result.setup.is_none());
    assert_eq!(result.worktree.state, WorktreeState::Ready);
    assert_eq!(result.chat.project_id, project.id);
    assert_eq!(result.chat.chat.title.as_deref(), Some("Parallel fix"));
    assert_eq!(
        canonical(&result.chat.chat.folder),
        canonical(&preview.path)
    );
    assert!(
        rig.manager
            .worktree_chat_apply(&preview.token)
            .await
            .is_err(),
        "a token is single use"
    );
    let chat_id = result.chat.chat.id.clone();
    status(&rig, &chat_id, SessionStatus::Ready).await;
    let saved = rig.store.load_chat(&chat_id).await.expect("saved chat");
    assert_eq!(canonical(&saved.chat.folder), canonical(&preview.path));
    let workspace = rig.manager.git_workspace(&chat_id).await.expect("git");
    assert!(workspace.is_linked_worktree);
    assert_eq!(workspace.branch.as_deref(), Some("ghost/parallel-fix"));
    assert_eq!(
        rig.manager
            .managed_worktrees(&chat_id)
            .await
            .expect("records"),
        vec![result.worktree.clone()]
    );
    // Merge, PR and Discard work from the new chat as they do for a chat moved into a worktree.
    rig.manager
        .worktree_action_preview(&chat_id, &result.worktree.id, WorktreeAction::Discard)
        .await
        .expect("discard preview");
    let context = rig
        .manager
        .chat_folder_context(&chat_id)
        .await
        .expect("folder context");
    assert_eq!(canonical(&context.primary), canonical(&preview.path));
    assert!(
        context.additional.is_empty(),
        "the original checkout is excluded from the isolated session"
    );
    // The source chat kept running in the original folder.
    assert!(rig.manager.shared().live.contains(&direct.chat.id));
    // The worktree is known as the worktree it is: its project folder names its source, so a
    // new chat never starts in it by itself, and a pick of it says where it came from.
    let snapshot = rig
        .manager
        .workspace_snapshot(ghost_protocol::workspace::WorkspaceFilter {
            include_archived: true,
            project_id: None,
        })
        .await
        .expect("snapshot");
    let folders: Vec<_> = snapshot
        .projects
        .iter()
        .flat_map(|project| &project.folders)
        .collect();
    let at = |path: &Path| {
        folders
            .iter()
            .find(|folder| {
                folder
                    .path
                    .as_deref()
                    .is_some_and(|stored| canonical(stored) == canonical(path))
            })
            .copied()
    };
    let tree = at(Path::new(&preview.path)).expect("the worktree is a project folder");
    assert_eq!(
        tree.worktree_of.as_deref().map(canonical),
        Some(canonical(&rig.folder))
    );
    assert_eq!(at(&rig.folder).expect("source").worktree_of, None);
    let picked = rig
        .manager
        .workspace_folder_matches(&preview.path)
        .await
        .expect("matches");
    let origin = picked.worktree.expect("a managed worktree");
    assert_eq!(origin.branch, "ghost/parallel-fix");
    assert_eq!(canonical(&origin.source), canonical(&rig.folder));
    let source = rig
        .manager
        .workspace_folder_matches(&rig.folder.to_string_lossy())
        .await
        .expect("matches");
    assert_eq!(source.worktree, None);
    assert_eq!(source.matches[0].folder_id, project.folders[0].id);
}

#[tokio::test]
async fn an_unsaved_chat_rolls_back_its_new_worktree_and_branch() {
    let mut rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    enable_worktree_fixture(&mut rig).await;
    let project = project(&rig, &rig.folder).await;
    let preview = plan(&rig, input(&project)).await;
    rig.manager
        .shared()
        .fail_worktree_chat_save
        .store(true, Ordering::SeqCst);
    let error = rig
        .manager
        .worktree_chat_apply(&preview.token)
        .await
        .expect_err("save failure")
        .to_string();
    assert!(error.contains("were removed"), "{error}");
    assert!(!Path::new(&preview.path).exists());
    let branch = rig.folder.join(".git/refs/heads").join(&preview.branch);
    assert!(!branch.exists(), "the new branch was removed");
    let packed = std::fs::read_to_string(rig.folder.join(".git/packed-refs")).unwrap_or_default();
    assert!(!packed.contains(&preview.branch));
    let snapshot = rig
        .manager
        .workspace_snapshot(ghost_protocol::workspace::WorkspaceFilter {
            include_archived: true,
            project_id: None,
        })
        .await
        .expect("snapshot");
    assert!(snapshot.chats.is_empty(), "no chat was saved");
    // The next attempt with a fresh preview succeeds.
    let retry = plan(&rig, input(&project)).await;
    let created = rig
        .manager
        .worktree_chat_apply(&retry.token)
        .await
        .expect("retry");
    assert_eq!(created.worktree.state, WorktreeState::Ready);
}

#[tokio::test]
async fn a_plain_project_folder_returns_the_direct_fallback() {
    let mut rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    enable_worktree_fixture(&mut rig).await;
    let plain = tempfile::tempdir().expect("plain");
    let project = project(&rig, plain.path()).await;
    let preview = rig
        .manager
        .worktree_chat_preview(input(&project))
        .await
        .expect("preview");
    let WorktreeChatPreview::NotGitRepository { message, chip, .. } = preview else {
        panic!("expected the direct fallback");
    };
    assert!(message.contains("not a Git repository"), "{message}");
    assert_eq!(chip.mode, ChatFolderMode::Direct);
    assert!(chip.branch.is_none());
}

#[tokio::test]
async fn saved_setup_runs_after_creation_and_holds_the_agent_for_the_command() {
    let mut rig = rig(
        vec![idle_driver("setup-source")],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    enable_worktree_fixture(&mut rig).await;
    let project = project(&rig, &rig.folder).await;
    let direct = rig
        .manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: project.folders[0].id.clone(),
            allow_busy_folder: false,
        })
        .await
        .expect("direct chat");
    std::fs::write(rig.folder.join(".env"), "FIXTURE=one").expect("env fixture");
    let recipe = WorktreeSetupInput {
        run: None,
        files: vec![".env".into()],
        command: Some(SetupCommand {
            program: std::env::current_exe()
                .expect("exe")
                .to_string_lossy()
                .into(),
            args: vec!["--not-executed".into()],
        }),
    };
    rig.manager
        .worktree_setup_save(&direct.chat.id, recipe.clone())
        .await
        .expect("save recipe");
    let skipped = plan(
        &rig,
        WorktreeChatInput {
            run_setup: false,
            ..input(&project)
        },
    )
    .await;
    assert!(skipped.setup.is_none());
    assert!(
        skipped
            .warnings
            .iter()
            .any(|warning| warning.contains("skipped"))
    );
    let preview = plan(&rig, input(&project)).await;
    assert_eq!(preview.setup.as_ref(), Some(&recipe));
    assert!(
        preview
            .warnings
            .iter()
            .any(|warning| warning.contains("--not-executed"))
    );
    let result = rig
        .manager
        .worktree_chat_apply(&preview.token)
        .await
        .expect("apply");
    let setup = result.setup.expect("setup ran");
    assert_eq!(setup.copied, vec![".env"]);
    assert!(setup.command_token.is_some());
    assert!(!result.started, "the agent waits for the setup command");
    let attempt = rig
        .store
        .read_setup_attempt(&result.chat.chat.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        attempt.state,
        ghost_protocol::worktree_setup::WorktreeSetupState::Awaiting
    );
    assert_eq!(attempt.generation, 1);
    assert!(
        !rig.manager.shared().live.contains(&result.chat.chat.id),
        "no agent process was started"
    );
    assert_eq!(
        std::fs::read(Path::new(&result.worktree.path).join(".env")).expect("copied"),
        b"FIXTURE=one"
    );
    std::fs::write(rig.folder.join(".env"), "FIXTURE=changed-source").unwrap();
    let retry = rig
        .manager
        .worktree_setup_command_preview(&result.chat.chat.id, &result.worktree.id)
        .await
        .unwrap();
    assert!(retry.files.is_empty(), "retry never recopies setup files");
    let retried = rig
        .manager
        .worktree_setup_apply(&result.chat.chat.id, &retry.token)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(Path::new(&result.worktree.path).join(".env")).unwrap(),
        b"FIXTURE=one"
    );
    let error = rig
        .manager
        .prepare_worktree_setup_terminal(
            &result.chat.chat.id,
            setup.command_token.as_deref().unwrap(),
        )
        .await
        .err()
        .expect("old generation refused");
    assert!(
        error.to_string().contains("older setup attempt")
            || error.to_string().contains("Review setup"),
        "{error}"
    );
    let launch = rig
        .manager
        .prepare_worktree_setup_terminal(
            &result.chat.chat.id,
            retried.command_token.as_deref().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        rig.store
            .read_setup_attempt(&result.chat.chat.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        ghost_protocol::worktree_setup::WorktreeSetupState::Running
    );
    drop(launch);
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let attempt = rig
                .store
                .read_setup_attempt(&result.chat.chat.id)
                .await
                .unwrap()
                .unwrap();
            if attempt.state == ghost_protocol::worktree_setup::WorktreeSetupState::Failed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("launch failure is persisted by its Rust owner");
}

#[tokio::test]
async fn a_chatless_rollback_record_is_profile_scoped_discoverable_after_reopen_and_recoverable() {
    use ghost_protocol::git::WorktreeRecoveryAction;
    let mut rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    enable_worktree_fixture(&mut rig).await;
    let project = project(&rig, &rig.folder).await;
    let service = rig.manager.worktrees().unwrap();
    let outcome = service
        .new_chat_preview(crate::gitworkspaces::NewChatRequest {
            chat_id: "chat-that-could-not-be-saved",
            folder: &rig.folder,
            start_branch: None,
            branch: None,
            title: None,
            bring_changes: false,
        })
        .await
        .unwrap();
    let crate::gitworkspaces::NewChatPlan::Worktree(plan) = outcome else {
        panic!("Git fixture")
    };
    let worktree = service
        .new_chat_apply("chat-that-could-not-be-saved", &plan.token)
        .await
        .unwrap();
    std::fs::write(
        Path::new(&worktree.path).join("retained.txt"),
        "retained user work",
    )
    .unwrap();
    assert!(
        service
            .new_chat_rollback("chat-that-could-not-be-saved", &worktree.id, false)
            .await
            .is_err()
    );
    assert!(
        rig.manager
            .managed_worktrees("chat-that-could-not-be-saved")
            .await
            .is_err(),
        "the original route still requires a saved chat"
    );
    let reopened = crate::gitworkspaces::GitWorkspaces::new(
        &rig.profile,
        rig.manager.deps().env.clone(),
        ghost_process::LeftoverRegistry::disabled(),
    );
    let records = reopened.profile_records().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, WorktreeState::RecoveryRequired);
    assert!(
        records[0]
            .detail
            .as_ref()
            .unwrap()
            .contains("could not be saved")
    );
    assert_eq!(
        rig.manager
            .profile_worktrees(Some(&project.id))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        rig.manager
            .profile_worktrees(Some("another-project"))
            .await
            .unwrap()
            .is_empty()
    );
    let review = rig
        .manager
        .profile_worktree_recovery_preview(&worktree.id, WorktreeRecoveryAction::Reconcile)
        .await
        .unwrap();
    std::fs::write(
        Path::new(&worktree.path).join("changed-after-review.txt"),
        "new change",
    )
    .unwrap();
    assert!(
        rig.manager
            .profile_worktree_recovery_apply(&worktree.id, &review.token, false)
            .await
            .is_err()
    );
    let review = rig
        .manager
        .profile_worktree_recovery_preview(&worktree.id, WorktreeRecoveryAction::Reconcile)
        .await
        .unwrap();
    let recovered = rig
        .manager
        .profile_worktree_recovery_apply(&worktree.id, &review.token, false)
        .await
        .unwrap();
    assert_eq!(recovered.worktree.state, WorktreeState::Ready);
    assert_eq!(
        rig.manager.profile_worktrees(None).await.unwrap().len(),
        1,
        "a reconciled chatless record remains discoverable"
    );
    assert_eq!(
        std::fs::read(Path::new(&worktree.path).join("retained.txt")).unwrap(),
        b"retained user work"
    );
    assert!(
        rig.manager
            .profile_worktree_folder(&worktree.id)
            .await
            .unwrap()
            .is_dir()
    );
    let mut other = super::rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    enable_worktree_fixture(&mut other).await;
    assert!(
        other
            .manager
            .profile_worktrees(None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        other
            .manager
            .profile_worktree_recovery_preview(&worktree.id, WorktreeRecoveryAction::Reconcile)
            .await
            .is_err()
    );
    assert!(
        other
            .manager
            .profile_worktree_folder(&worktree.id)
            .await
            .is_err()
    );
    // Discovery survives a missing source repository, and review refuses to invent its state.
    std::fs::rename(
        rig.folder.join(".git"),
        rig.folder.join(".git-unavailable-fixture"),
    )
    .unwrap();
    assert_eq!(
        rig.manager
            .profile_worktrees(Some(&project.id))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        rig.manager
            .profile_worktree_recreate_preview(&worktree.id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_deleted_worktree_folder_is_recreated_for_its_stopped_chat() {
    let mut rig = rig(
        vec![idle_driver("recreate")],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    enable_worktree_fixture(&mut rig).await;
    let project = project(&rig, &rig.folder).await;
    let preview = plan(&rig, input(&project)).await;
    let result = rig
        .manager
        .worktree_chat_apply(&preview.token)
        .await
        .expect("apply");
    let chat_id = result.chat.chat.id.clone();
    status(&rig, &chat_id, SessionStatus::Ready).await;
    std::fs::remove_dir_all(&result.worktree.path).expect("deleted by hand");
    assert_eq!(
        rig.manager.managed_worktrees(&chat_id).await.expect("list")[0].state,
        WorktreeState::Missing
    );
    let recreate = rig
        .manager
        .worktree_recreate_preview(&chat_id, &result.worktree.id)
        .await
        .expect("recreate preview");
    // Its idle agent stops first (it resumes on the next message); then the folder is recreated.
    let recreated = rig
        .manager
        .worktree_recreate_apply(&chat_id, &result.worktree.id, &recreate.token)
        .await
        .expect("recreate");
    assert!(recreated.error.is_none(), "{:?}", recreated.error);
    assert!(!rig.manager.shared().live.contains(&chat_id));
    assert_eq!(recreated.worktree.state, WorktreeState::Ready);
    assert!(Path::new(&result.worktree.path).join(".git").is_file());
    assert_eq!(
        rig.manager.managed_worktrees(&chat_id).await.expect("list")[0].state,
        WorktreeState::Ready
    );
}

async fn setup_source(rig: &Rig, project: &ProjectSummary) {
    let source = rig
        .manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: project.folders[0].id.clone(),
            allow_busy_folder: false,
        })
        .await
        .unwrap();
    rig.manager
        .worktree_setup_save(
            &source.chat.id,
            WorktreeSetupInput {
                run: None,
                files: vec![],
                command: Some(SetupCommand {
                    program: std::env::current_exe().unwrap().to_string_lossy().into(),
                    args: vec!["--never-run".into()],
                }),
            },
        )
        .await
        .unwrap();
}

async fn failed_setup(
    rig: &Rig,
    chat: &str,
) -> ghost_protocol::worktree_setup::WorktreeSetupAttempt {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let attempt = rig.store.read_setup_attempt(chat).await.unwrap().unwrap();
            if attempt.state == ghost_protocol::worktree_setup::WorktreeSetupState::Failed {
                let snapshot = rig
                    .manager
                    .attach(chat, Arc::new(Sink::default()))
                    .await
                    .unwrap();
                let failed = snapshot
                    .snapshot
                    .session
                    .worktree_setup
                    .as_ref()
                    .is_some_and(|value| {
                        value.state == ghost_protocol::worktree_setup::WorktreeSetupState::Failed
                    });
                rig.manager.detach(&snapshot.subscription_id);
                if failed {
                    return attempt;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("native prelaunch failure persisted")
}

#[tokio::test]
async fn setup_terminal_reservation_and_pending_claim_are_generation_owned_and_hold_on_failure() {
    use ghost_protocol::worktree_setup::WorktreeSetupState;
    let mut rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    enable_worktree_fixture(&mut rig).await;
    let project = project(&rig, &rig.folder).await;
    setup_source(&rig, &project).await;
    let reviewed = plan(&rig, input(&project)).await;
    let created = rig
        .manager
        .worktree_chat_apply(&reviewed.token)
        .await
        .unwrap();
    let chat = &created.chat.chat.id;
    let token = created.setup.unwrap().command_token.unwrap();
    rig.manager
        .send(chat, "before reservation refusal".into())
        .await
        .unwrap();
    let terminals = crate::terminal::Terminals::default();
    // Every setup terminal the app allows is taken (a sign-in no longer counts against them).
    let busy: Vec<String> = (0..crate::terminal::MAX_SETUP_TERMINALS)
        .map(|_| terminals.reserve_setup("window").unwrap().pending())
        .collect();
    assert!(
        rig.manager
            .reserve_worktree_setup_terminal(&terminals, "window", chat, &token)
            .await
            .is_err()
    );
    assert!(
        failed_setup(&rig, chat)
            .await
            .detail
            .unwrap()
            .contains("could not be reserved")
    );
    rig.manager
        .send(chat, "after reservation refusal".into())
        .await
        .unwrap();
    let held = rig
        .manager
        .attach(chat, Arc::new(Sink::default()))
        .await
        .unwrap();
    assert!(held.snapshot.queue.iter().all(|row| row.held));
    for id in &busy {
        terminals.close("window", id).unwrap();
    }
    let retry = rig
        .manager
        .worktree_setup_command_preview(chat, &created.worktree.id)
        .await
        .unwrap();
    let result = rig
        .manager
        .worktree_setup_apply(chat, &retry.token)
        .await
        .unwrap();
    let fresh = result.command_token.unwrap();
    assert!(
        rig.manager
            .worktree_setup_apply(chat, &retry.token)
            .await
            .is_err(),
        "a consumed review cannot fail its replacement generation"
    );
    assert!(
        rig.manager
            .prepare_worktree_setup_terminal(chat, &token)
            .await
            .is_err()
    );
    assert!(
        rig.manager
            .prepare_worktree_setup_terminal(chat, "unrelated-token")
            .await
            .is_err()
    );
    assert_eq!(
        rig.store
            .read_setup_attempt(chat)
            .await
            .unwrap()
            .unwrap()
            .state,
        WorktreeSetupState::Awaiting
    );
    let id = rig
        .manager
        .reserve_worktree_setup_terminal(&terminals, "window", chat, &fresh)
        .await
        .unwrap();
    assert!(
        terminals
            .claim_setup("other-window", &id, chat, &fresh)
            .is_err()
    );
    assert!(
        terminals
            .claim_setup("window", &id, "another-chat", &fresh)
            .is_err()
    );
    assert!(terminals.claim_setup("window", &id, chat, &token).is_err());
    assert!(
        terminals.claim("window", &id).is_err(),
        "generic claims cannot steal setup admission"
    );
    assert_eq!(
        rig.store
            .read_setup_attempt(chat)
            .await
            .unwrap()
            .unwrap()
            .state,
        WorktreeSetupState::Running
    );
    terminals.close("window", &id).unwrap();
    assert!(terminals.claim_setup("window", &id, chat, &fresh).is_err());
    assert_eq!(failed_setup(&rig, chat).await.generation, 2);
    let retry = rig
        .manager
        .worktree_setup_command_preview(chat, &created.worktree.id)
        .await
        .unwrap();
    let result = rig
        .manager
        .worktree_setup_apply(chat, &retry.token)
        .await
        .unwrap();
    let token = result.command_token.unwrap();
    let id = rig
        .manager
        .reserve_worktree_setup_terminal(&terminals, "window", chat, &token)
        .await
        .unwrap();
    let (ticket, launch) = terminals.claim_setup("window", &id, chat, &token).unwrap();
    let completion = launch.completion.unwrap();
    drop(launch.guard);
    drop(ticket);
    completion.send(Ok(())).unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if rig
                .store
                .read_setup_attempt(chat)
                .await
                .unwrap()
                .unwrap()
                .state
                == WorktreeSetupState::Succeeded
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let held = rig
        .manager
        .attach(chat, Arc::new(Sink::default()))
        .await
        .unwrap();
    assert!(
        held.snapshot.queue.iter().all(|row| row.held),
        "successfully retrying does not auto-send failed messages"
    );
}

#[tokio::test]
async fn two_projects_setup_terminals_run_at_once() {
    let mut rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    enable_worktree_fixture(&mut rig).await;
    // A second repository, a project of its own.
    let second = rig._dir.path().join("second project");
    std::fs::create_dir(&second).unwrap();
    for args in [
        vec!["init", "-b", "main"],
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
        super::project_tools::fixture_git(&second, &args).await;
    }
    let first_project = project(&rig, &rig.folder).await;
    let second_project = project(&rig, &second).await;
    let terminals = crate::terminal::Terminals::default();
    let mut opened = Vec::new();
    for project in [&first_project, &second_project] {
        setup_source(&rig, project).await;
        let reviewed = plan(&rig, input(project)).await;
        let created = rig
            .manager
            .worktree_chat_apply(&reviewed.token)
            .await
            .expect("worktree chat");
        let token = created.setup.unwrap().command_token.unwrap();
        // The same window starts the second project's setup while the first one's is open.
        let id = rig
            .manager
            .reserve_worktree_setup_terminal(&terminals, "window", &created.chat.chat.id, &token)
            .await
            .expect("each project's setup gets its own terminal");
        opened.push((created.chat.chat.id, token, id));
    }
    assert_ne!(opened[0].2, opened[1].2);
    for (chat, token, id) in &opened {
        let (ticket, launch) = terminals
            .claim_setup("window", id, chat, token)
            .expect("claim");
        drop(launch);
        drop(ticket);
        assert_eq!(
            rig.store
                .read_setup_attempt(chat)
                .await
                .unwrap()
                .unwrap()
                .state,
            ghost_protocol::worktree_setup::WorktreeSetupState::Running,
            "neither attempt was marked failed"
        );
    }
}

#[tokio::test]
async fn expired_setup_receipts_and_folder_admission_fail_only_their_current_generation() {
    use ghost_protocol::worktree_setup::WorktreeSetupState;
    let mut rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    enable_worktree_fixture(&mut rig).await;
    let project = project(&rig, &rig.folder).await;
    setup_source(&rig, &project).await;
    for kind in [
        "expired",
        "reserved-folder",
        "missing-folder",
        "review-expired",
        "review-reserved-folder",
    ] {
        let reviewed = plan(&rig, input(&project)).await;
        let created = rig
            .manager
            .worktree_chat_apply(&reviewed.token)
            .await
            .unwrap();
        let chat = &created.chat.chat.id;
        let token = created.setup.unwrap().command_token.unwrap();
        rig.manager
            .send(chat, format!("held for {kind}"))
            .await
            .unwrap();
        let folder = PathBuf::from(&created.worktree.path);
        let reserved_folder = crate::session::live::normalize_path(folder.clone());
        let apply_review = if kind.starts_with("review-") {
            Some(
                rig.manager
                    .worktree_setup_command_preview(chat, &created.worktree.id)
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        match kind {
            "expired" => {
                rig.manager
                    .worktrees()
                    .unwrap()
                    .expire_setup_for_test(&token);
                rig.manager
                    .worktree_setup_command_preview(chat, &created.worktree.id)
                    .await
                    .unwrap();
            }
            "reserved-folder" | "review-reserved-folder" => {
                assert!(rig.manager.shared().live.reserve_folder(&reserved_folder))
            }
            "review-expired" => rig
                .manager
                .worktrees()
                .unwrap()
                .expire_setup_for_test(&apply_review.as_ref().unwrap().token),
            _ => std::fs::rename(&folder, folder.with_extension("retained-fixture")).unwrap(),
        }
        if let Some(review) = apply_review {
            assert!(
                rig.manager
                    .worktree_setup_apply(chat, &review.token)
                    .await
                    .is_err()
            );
        } else {
            assert!(
                rig.manager
                    .prepare_worktree_setup_terminal(chat, &token)
                    .await
                    .is_err()
            );
        }
        let failed = failed_setup(&rig, chat).await;
        assert_eq!(failed.state, WorktreeSetupState::Failed);
        assert!(!failed.detail.unwrap().is_empty());
        rig.manager
            .send(chat, "new after admission failure".into())
            .await
            .unwrap();
        let held = rig
            .manager
            .attach(chat, Arc::new(Sink::default()))
            .await
            .unwrap();
        assert!(held.snapshot.queue.iter().all(|row| row.held));
        if kind == "reserved-folder" || kind == "review-reserved-folder" {
            rig.manager.shared().live.release_folder(&reserved_folder);
        }
        if kind == "missing-folder" {
            std::fs::rename(folder.with_extension("retained-fixture"), &folder).unwrap();
        }
    }
}

#[tokio::test]
async fn new_worktree_chats_get_ignored_instruction_files_unless_turned_off() {
    let mut rig = rig(
        vec![idle_driver("first"), idle_driver("second")],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    enable_worktree_fixture(&mut rig).await;
    // info/exclude lives in the shared Git folder, so every worktree ignores CLAUDE.md too.
    let info = rig.folder.join(".git").join("info");
    std::fs::create_dir_all(&info).unwrap();
    std::fs::write(info.join("exclude"), "CLAUDE.md\n").unwrap();
    std::fs::write(rig.folder.join("CLAUDE.md"), "Local instructions.\n").unwrap();
    let project = project(&rig, &rig.folder).await;
    assert!(rig.manager.worktree_instruction_copies(), "on by default");
    let first = plan(
        &rig,
        WorktreeChatInput {
            title: Some("First".into()),
            ..input(&project)
        },
    )
    .await;
    let first = rig
        .manager
        .worktree_chat_apply(&first.token)
        .await
        .expect("first chat");
    assert!(first.warnings.is_empty(), "{:?}", first.warnings);
    assert_eq!(
        std::fs::read_to_string(Path::new(&first.worktree.path).join("CLAUDE.md")).unwrap(),
        "Local instructions.\n"
    );
    rig.manager.set_worktree_instruction_copies(false);
    let second = plan(
        &rig,
        WorktreeChatInput {
            title: Some("Second".into()),
            ..input(&project)
        },
    )
    .await;
    let second = rig
        .manager
        .worktree_chat_apply(&second.token)
        .await
        .expect("second chat");
    assert!(
        !Path::new(&second.worktree.path).join("CLAUDE.md").exists(),
        "turned off, a new worktree gets only what Git checks out"
    );
}

#[tokio::test]
async fn run_starts_the_saved_command_in_the_worktree_with_its_kept_ports() {
    let mut rig = rig(
        vec![idle_driver("run-source"), idle_driver("run-worktree")],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    enable_worktree_fixture(&mut rig).await;
    let project = project(&rig, &rig.folder).await;
    let direct = rig
        .manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: project.folders[0].id.clone(),
            allow_busy_folder: false,
        })
        .await
        .expect("direct chat");
    // A chat in the project folder itself has no worktree, so nothing to run.
    assert_eq!(
        rig.manager.worktree_run(&direct.chat.id).await.unwrap(),
        None
    );
    let program = std::env::current_exe().expect("exe");
    let run = SetupCommand {
        program: program.to_string_lossy().into(),
        args: vec!["--not-executed".into()],
    };
    rig.manager
        .worktree_setup_save(
            &direct.chat.id,
            WorktreeSetupInput {
                files: Vec::new(),
                command: None,
                run: Some(run.clone()),
            },
        )
        .await
        .expect("save recipe");
    let preview = plan(&rig, input(&project)).await;
    let created = rig
        .manager
        .worktree_chat_apply(&preview.token)
        .await
        .expect("apply");
    let chat = created.chat.chat.id.clone();
    let info = rig
        .manager
        .worktree_run(&chat)
        .await
        .expect("run info")
        .expect("a worktree chat can run");
    assert_eq!(info.command.as_ref(), Some(&run));
    assert_eq!(canonical(&info.folder), canonical(&preview.path));
    assert!((25_000..32_000).contains(&info.port));
    assert_eq!(info.port % 10, 0);
    // The block is kept, not given again.
    let again = rig.manager.worktree_run(&chat).await.unwrap().unwrap();
    assert_eq!(again.port, info.port);
    // With the chat's agent stopped, nothing else holds the worktree.
    if created.started {
        status(&rig, &chat, SessionStatus::Ready).await;
    }
    rig.manager.stop(&chat).await.expect("stop");
    status(&rig, &chat, SessionStatus::Stopped).await;
    let folder = super::super::live::normalize_path(canonical(&preview.path));
    let live = &rig.manager.shared().live;
    assert!(live.reserve_folder(&folder));
    live.release_folder(&folder);
    let plan = rig
        .manager
        .prepare_worktree_run(&chat)
        .await
        .expect("run plan");
    assert_eq!(canonical(&plan.spec.launch.program), canonical(&program));
    assert_eq!(
        plan.spec.launch.args,
        [std::ffi::OsString::from("--not-executed")]
    );
    let port = std::ffi::OsString::from(info.port.to_string());
    assert_eq!(
        plan.spec.launch.set_env,
        [
            ("GHOST_PORT".into(), port.clone()),
            ("O7_PORT".into(), port.clone()),
            ("AURACODE_PORT".into(), port.clone()),
            ("PORT".into(), port)
        ]
    );
    assert_eq!(canonical(&plan.spec.folder), canonical(&preview.path));
    // Like a project terminal, Run holds the worktree: folder operations wait until it stops.
    assert!(
        !live.reserve_folder(&folder),
        "a folder operation ran beside Run"
    );
    drop(plan);
    assert!(live.reserve_folder(&folder));
    live.release_folder(&folder);
    // Without a saved Run command there is nothing to start.
    rig.manager
        .worktree_setup_save(&direct.chat.id, WorktreeSetupInput::default())
        .await
        .expect("clear recipe");
    assert!(rig.manager.prepare_worktree_run(&chat).await.is_err());
    assert_eq!(
        rig.manager
            .worktree_run(&chat)
            .await
            .unwrap()
            .unwrap()
            .command,
        None
    );
}

#[tokio::test]
async fn a_slow_worktree_creation_does_not_delay_another_folder() {
    let mut rig = rig(
        vec![
            idle_driver("elsewhere"),
            idle_driver("worktree"),
            idle_driver("same-folder"),
        ],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    enable_worktree_fixture(&mut rig).await;
    let source = project(&rig, &rig.folder).await;
    let other_folder = rig._dir.path().join("other project");
    std::fs::create_dir(&other_folder).unwrap();
    let other = project(&rig, &other_folder).await;
    let preview = plan(&rig, input(&source)).await;
    // `git worktree add` is slow (a large repository, a slow disk): it holds the repository.
    let service = rig.manager.worktrees().unwrap();
    let slow = service.hold_repository_for_test(&rig.folder).await;
    let creating = tokio::spawn({
        let manager = rig.manager.clone();
        let token = preview.token.clone();
        async move { manager.worktree_chat_apply(&token).await }
    });
    // A chat in the same folder waits its turn: busy-folder checks stay in arrival order.
    let same_folder = tokio::spawn({
        let manager = rig.manager.clone();
        let folder_id = source.folders[0].id.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            manager
                .chat_project_create(ProjectChatCreate {
                    agent_id: "mock".into(),
                    folder_id,
                    allow_busy_folder: false,
                })
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!creating.is_finished(), "the creation waits on Git");
    // Everything about another folder goes on meanwhile.
    let quick = Duration::from_secs(5);
    let elsewhere = tokio::time::timeout(
        quick,
        rig.manager.chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: other.folders[0].id.clone(),
            allow_busy_folder: false,
        }),
    )
    .await
    .expect("a chat in another project starts at once")
    .expect("created");
    status(&rig, &elsewhere.chat.id, SessionStatus::Ready).await;
    tokio::time::timeout(quick, rig.manager.workspace_snapshot(Default::default()))
        .await
        .expect("the sidebar answers at once")
        .expect("snapshot");
    tokio::time::timeout(quick, rig.manager.chat_create_preview(&other.folders[0].id))
        .await
        .expect("New chat's folder check answers at once")
        .expect("preview");
    tokio::time::timeout(quick, rig.manager.profile_worktrees(None))
        .await
        .expect("retained worktrees answer at once")
        .expect("worktrees");
    let review = tokio::time::timeout(
        quick,
        rig.manager.project_terminal_review(&elsewhere.chat.id),
    )
    .await
    .expect("a new terminal tab's folder check answers at once");
    if let Ok(review) = review
        && let Some(shell) = review.shells.first()
    {
        drop(
            tokio::time::timeout(
                quick,
                rig.manager
                    .open_project_terminal(&elsewhere.chat.id, Some(shell.id)),
            )
            .await
            .expect("a terminal in another project opens at once"),
        );
    }
    assert!(
        !same_folder.is_finished(),
        "the same folder's chat waits its turn"
    );
    drop(slow);
    let made = tokio::time::timeout(quick, creating)
        .await
        .expect("the creation finishes once Git does")
        .unwrap()
        .expect("worktree chat");
    assert_eq!(made.worktree.state, WorktreeState::Ready);
    let direct = tokio::time::timeout(quick, same_folder)
        .await
        .expect("the waiting chat follows")
        .unwrap()
        .expect("direct chat");
    assert_ne!(
        canonical(&direct.chat.folder),
        canonical(&made.chat.chat.folder)
    );
}

/// Runs Git in `folder` for a fixture and returns what it printed.
async fn fixture_git(folder: &Path, command: &[&str]) -> String {
    use ghost_process::{
        HelperEnv, HelperSpec, LeftoverRegistry, find_exe, process_env, run_helper,
    };
    let vars = process_env();
    let program = find_exe("git", &vars).expect("Git is required for owned worktree tests");
    let output = run_helper(
        &LeftoverRegistry::disabled(),
        HelperSpec {
            program,
            args: command.iter().map(Into::into).collect(),
            cwd: folder.into(),
            env: HelperEnv::Resolved(Arc::new(ResolvedEnv {
                vars,
                source: EnvSource::Fallback {
                    reason: "owned worktree test".into(),
                },
                warnings: vec![],
                resolved_at_ms: 0,
            })),
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
    String::from_utf8(output.stdout)
        .expect("Git output")
        .trim()
        .to_owned()
}

#[tokio::test]
async fn a_chats_branch_starts_at_its_worktree_base_or_where_it_left_the_default_branch() {
    use crate::gitworkspaces::{BranchDiff, BranchFile};
    use ghost_protocol::changes::ChangeKind;
    let mut rig = rig(
        vec![idle_driver("worktree"), idle_driver("direct")],
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    enable_worktree_fixture(&mut rig).await;
    // An origin whose default branch is main, so the project folder records it.
    for command in [
        &[
            "config",
            "remote.origin.url",
            "git@github.com:fixture/project.git",
        ][..],
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    ] {
        fixture_git(&rig.folder, command).await;
    }
    let project = project(&rig, &rig.folder).await;
    assert_eq!(
        project.folders[0].git_default_branch.as_deref(),
        Some("main")
    );
    // A worktree chat starts at the commit its worktree was made from.
    let preview = plan(
        &rig,
        WorktreeChatInput {
            title: Some("Branch view".into()),
            ..input(&project)
        },
    )
    .await;
    let created = rig
        .manager
        .worktree_chat_apply(&preview.token)
        .await
        .expect("worktree chat");
    let chat_id = created.chat.chat.id.clone();
    std::fs::write(Path::new(&created.worktree.path).join("new.txt"), "new\n").unwrap();
    let changes = rig
        .manager
        .chat_branch_changes(&chat_id, None, 10)
        .await
        .expect("changes")
        .expect("a branch of its own");
    assert_eq!(changes.base, created.worktree.base_commit);
    assert_eq!(
        changes.branch.as_deref(),
        Some(created.worktree.branch.as_str())
    );
    assert_eq!(
        changes.files,
        [BranchFile {
            path: "new.txt".into(),
            kind: ChangeKind::Added,
        }]
    );
    assert_eq!(
        rig.manager
            .chat_branch_diff(&chat_id, None, "new.txt", 1024)
            .await
            .expect("diff"),
        Some(BranchDiff {
            before: None,
            after: Some("new\n".into()),
            binary: false,
            truncated: false,
        })
    );
    assert_eq!(
        rig.manager
            .chat_branch_diff(&chat_id, None, "missing.txt", 1024)
            .await
            .expect("not changed"),
        None
    );
    for path in ["", "a\nb", &"a".repeat(4097), ".env", "config/server.pem"] {
        assert!(
            rig.manager
                .chat_branch_diff(&chat_id, None, path, 1024)
                .await
                .is_err(),
            "{path:?} is refused"
        );
    }
    assert!(
        rig.manager
            .chat_branch_changes("no-such-chat", None, 10)
            .await
            .is_err()
    );
    // A chat in the project folder on its default branch has no branch of its own.
    let direct = rig
        .manager
        .chat_project_create(ProjectChatCreate {
            agent_id: "mock".into(),
            folder_id: project.folders[0].id.clone(),
            allow_busy_folder: false,
        })
        .await
        .expect("direct chat");
    assert_eq!(
        rig.manager
            .chat_branch_changes(&direct.chat.id, None, 10)
            .await
            .expect("on main"),
        None
    );
    // On another branch it starts where it left the default branch, committed or not.
    let main = fixture_git(&rig.folder, &["rev-parse", "HEAD"]).await;
    fixture_git(&rig.folder, &["checkout", "-b", "topic"]).await;
    std::fs::write(rig.folder.join("topic.txt"), "topic\n").unwrap();
    for command in [
        &["add", "--", "topic.txt"][..],
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@localhost",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "-m",
            "topic",
        ],
    ] {
        fixture_git(&rig.folder, command).await;
    }
    let topic_head = fixture_git(&rig.folder, &["rev-parse", "HEAD"]).await;
    std::fs::write(rig.folder.join("later.txt"), "later\n").unwrap();
    let added = |paths: &[&str]| {
        paths
            .iter()
            .map(|path| BranchFile {
                path: (*path).into(),
                kind: ChangeKind::Added,
            })
            .collect::<Vec<_>>()
    };
    let topic = rig
        .manager
        .chat_branch_changes(&direct.chat.id, None, 10)
        .await
        .expect("topic")
        .expect("a branch of its own");
    assert_eq!(topic.base, main);
    assert_eq!(topic.branch.as_deref(), Some("topic"));
    assert_eq!(topic.files, added(&["later.txt", "topic.txt"]));
    // A base the caller gives comes first.
    let given = rig
        .manager
        .chat_branch_changes(&direct.chat.id, Some(&topic_head), 10)
        .await
        .expect("given")
        .expect("a given base");
    assert_eq!(given.base, topic_head);
    assert_eq!(given.files, added(&["later.txt"]));
    assert_eq!(
        rig.manager
            .chat_branch_diff(&direct.chat.id, Some(&topic_head), "topic.txt", 1024)
            .await
            .expect("not changed since the given base"),
        None
    );
}
