use super::*;
use ghost_protocol::workspace::{ProjectChatCreate, ProjectCreate, ProjectInput, WorkspaceFilter};

struct ObserveFolders {
    inner: ScriptedDriver,
    seen: Arc<Mutex<Vec<PathBuf>>>,
}
impl AgentDriver for ObserveFolders {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        *lock(&self.seen) = ctx.setup.add_dirs.clone();
        Box::new(self.inner).run(ctx)
    }
}

#[tokio::test]
async fn additional_folders_reach_supported_driver_and_live_scope_stays_fixed_until_restart() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (inner, _) = ScriptedDriver::new(vec![
        ready_step("folders"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    let mut spec = ghost_agents::get("mock").unwrap().clone();
    spec.kind = ghost_protocol::DriverKind::Claude;
    let rig = rig_with_spec(
        Arc::new(OneDriver(Mutex::new(Some(Box::new(ObserveFolders {
            inner,
            seen: seen.clone(),
        }))))),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
        spec,
    );
    let project = rig
        .manager
        .project_create(ProjectCreate {
            project: ProjectInput {
                name: "Folders".into(),
                color: None,
                default_agent: None,
                default_model: None,
            },
            folder: rig.folder.to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    let extra = rig._dir.path().join("extra with spaces");
    std::fs::create_dir(&extra).unwrap();
    rig.manager
        .project_add_folder(&project.id, &extra.to_string_lossy())
        .await
        .unwrap();
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
    assert_eq!(*lock(&seen), vec![std::fs::canonicalize(&extra).unwrap()]);
    let before = rig
        .manager
        .chat_folder_context(&chat.chat.id)
        .await
        .unwrap();
    assert!(before.active && before.supports_additional);
    assert_eq!(before.additional.len(), 1);
    assert!(
        !rig.manager
            .shared()
            .live
            .reserve_folder(&super::super::live::normalize_path(extra.clone()))
    );
    assert!(
        !rig.manager
            .shared()
            .live
            .reserve_folder(&super::super::live::normalize_path(extra.join("child")))
    );
    let later = rig._dir.path().join("later");
    std::fs::create_dir(&later).unwrap();
    rig.manager
        .project_add_folder(&project.id, &later.to_string_lossy())
        .await
        .unwrap();
    assert_eq!(
        rig.manager
            .chat_folder_context(&chat.chat.id)
            .await
            .unwrap(),
        before
    );
    rig.manager.stop(&chat.chat.id).await.unwrap();
    eventually("folder driver retired", || {
        !rig.manager.shared().live.contains(&chat.chat.id)
    })
    .await;
    let after = rig
        .manager
        .chat_folder_context(&chat.chat.id)
        .await
        .unwrap();
    assert!(!after.active);
    assert_eq!(after.additional.len(), 2);
    // Paths are exposed only by this transient read, never a new transcript event field.
    let attached = rig
        .manager
        .attach(&chat.chat.id, Arc::new(Sink::default()))
        .await
        .unwrap();
    assert!(
        !serde_json::to_string(&attached.snapshot.session)
            .unwrap()
            .contains("extra with spaces")
    );
    rig.manager.shutdown_all(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn unsupported_driver_reports_extra_folders_without_forwarding_them() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (inner, _) = ScriptedDriver::new(vec![
        ready_step("primary-only"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    let rig = rig_with(
        Arc::new(OneDriver(Mutex::new(Some(Box::new(ObserveFolders {
            inner,
            seen: seen.clone(),
        }))))),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.stop(&chat.id).await.unwrap();
    eventually("stopped", || !rig.manager.shared().live.contains(&chat.id)).await;
    let workspace = rig
        .manager
        .workspace_snapshot(WorkspaceFilter::default())
        .await
        .unwrap();
    let extra = rig._dir.path().join("extra");
    std::fs::create_dir(&extra).unwrap();
    rig.manager
        .project_add_folder(&workspace.projects[0].id, &extra.to_string_lossy())
        .await
        .unwrap();
    let context = rig.manager.chat_folder_context(&chat.id).await.unwrap();
    assert!(!context.supports_additional);
    assert!(context.additional.is_empty());
    assert!(
        context
            .notices
            .iter()
            .any(|notice| notice.contains("only the primary"))
    );
    assert!(lock(&seen).is_empty());
    rig.manager.shutdown_all(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn isolated_folder_context_excludes_the_original_checkout_and_keeps_other_projects() {
    use ghost_protocol::git::{IsolationInput, WorktreeDestination};
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("isolation-folders"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    let mut rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    super::worktree_guards::enable_worktree_fixture(&mut rig).await;
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.stop(&chat.id).await.unwrap();
    eventually("isolated source stopped", || {
        !rig.manager.shared().live.contains(&chat.id)
    })
    .await;
    let workspace = rig
        .manager
        .workspace_snapshot(WorkspaceFilter::default())
        .await
        .unwrap();
    let other = rig._dir.path().join("other-project");
    std::fs::create_dir(&other).unwrap();
    rig.manager
        .project_add_folder(&workspace.projects[0].id, &other.to_string_lossy())
        .await
        .unwrap();
    let preview = rig
        .manager
        .isolation_preview(
            &chat.id,
            IsolationInput {
                slug: "folder-scope".into(),
                bring_changes: false,
            },
        )
        .await
        .unwrap();
    let created = rig
        .manager
        .isolation_apply(&chat.id, &preview.token)
        .await
        .unwrap();
    let move_review = rig
        .manager
        .worktree_relocation_preview(
            &chat.id,
            &created.worktree.id,
            WorktreeDestination::Isolated,
        )
        .await
        .unwrap();
    rig.manager
        .worktree_relocation_apply(&chat.id, &move_review.token)
        .await
        .unwrap();
    let load = rig.store.load_chat(&chat.id).await.unwrap();
    let scope = super::super::workspace_folders::resolve(
        super::super::workspace_folders::LaunchFolders {
            chat: load.chat,
            store: rig.store.clone(),
            worktrees: rig.manager.deps().worktrees.clone(),
        },
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        scope.additional,
        vec![std::fs::canonicalize(&other).unwrap().to_string_lossy()]
    );
    assert!(
        scope
            .notices
            .iter()
            .any(|notice| notice.contains("original checkout"))
    );
    assert!(!scope.active);
    rig.manager.shutdown_all(Duration::from_secs(2)).await;
}
