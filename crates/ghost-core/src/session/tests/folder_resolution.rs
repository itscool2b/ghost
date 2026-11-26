use super::*;
use ghost_protocol::workspace::{ProjectCreate, ProjectInput, WorkspaceFilter};

/// How a folder is saved and shown: canonical, in its plain spelling (no `\\?\` on Windows).
fn shown(folder: &Path) -> PathBuf {
    crate::fsutil::plain_path(&folder.canonicalize().unwrap())
}

fn input() -> ProjectInput {
    ProjectInput {
        name: "Remote workspace".into(),
        color: None,
        default_agent: None,
        default_model: None,
    }
}

#[tokio::test]
async fn locate_refreshes_parked_actor_and_refuses_stale_review_or_live_native_history() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("native"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let project = rig
        .manager
        .project_create(ProjectCreate {
            project: input(),
            folder: rig.folder.to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    let chat = rig
        .store
        .create_chat(NewChat {
            folder: rig.folder.canonicalize().unwrap(),
            agent_host: Default::default(),
            agent_id: "mock".into(),
            device_id: "device".into(),
            now: 1,
        })
        .await
        .unwrap();
    let attached = rig
        .manager
        .attach(&chat.id, Arc::new(Sink::default()))
        .await
        .unwrap();
    assert_eq!(
        PathBuf::from(attached.snapshot.chat.folder),
        shown(&rig.folder)
    );
    let destination = rig._dir.path().join("copy");
    std::fs::create_dir(&destination).unwrap();
    let review = rig
        .manager
        .folder_locate_review(&project.folders[0].id, destination.to_str().unwrap())
        .await
        .unwrap();
    let moved = rig
        .manager
        .folder_locate_apply(review.clone())
        .await
        .unwrap();
    assert_eq!(PathBuf::from(moved.path.unwrap()), shown(&destination));
    assert_eq!(
        PathBuf::from(
            rig.manager
                .chat_folder_context(&chat.id)
                .await
                .unwrap()
                .primary
        ),
        shown(&destination)
    );
    assert!(rig.manager.folder_locate_apply(review).await.is_err());
    rig.manager.start(&chat.id, false).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let review = rig
        .manager
        .folder_locate_review(&project.folders[0].id, rig.folder.to_str().unwrap())
        .await
        .unwrap();
    assert!(
        rig.manager
            .folder_locate_apply(review.clone())
            .await
            .is_err()
    );
    rig.manager.stop(&chat.id).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Stopped).await;
    assert!(rig.manager.folder_locate_apply(review).await.is_err());
    let workspace = rig
        .manager
        .workspace_snapshot(WorkspaceFilter::default())
        .await
        .unwrap();
    assert_eq!(
        PathBuf::from(workspace.projects[0].folders[0].path.as_ref().unwrap()),
        shown(&destination)
    );
}

#[tokio::test]
async fn search_and_apply_recheck_origin_without_network_or_index_changes() {
    let mut rig = rig(vec![], WriterConfig::STANDARD, Knobs::STANDARD);
    super::worktree_guards::enable_worktree_fixture(&mut rig).await;
    // This is our disposable fixture's local Git config, never user agent configuration.
    let config = rig.folder.join(".git/config");
    let mut contents = std::fs::read_to_string(&config).unwrap();
    contents.push_str("\n[remote \"origin\"]\n\turl = git@github.com:fixture/resolver.git\n");
    std::fs::write(&config, &contents).unwrap();
    let index = std::fs::read(rig.folder.join(".git/index")).ok();
    let project = rig
        .manager
        .project_create(ProjectCreate {
            project: input(),
            folder: rig.folder.to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    rig.manager
        .folder_set_code_roots(vec![rig.folder.to_string_lossy().into_owned()])
        .await
        .unwrap();
    let found = rig
        .manager
        .folder_resolution(&project.folders[0].id, true)
        .await
        .unwrap();
    assert_eq!(
        found.matches,
        vec![shown(&rig.folder).to_string_lossy().into_owned()]
    );
    let review = rig
        .manager
        .folder_locate_review(&project.folders[0].id, rig.folder.to_str().unwrap())
        .await
        .unwrap();
    std::fs::write(
        &config,
        contents.replace("fixture/resolver", "fixture/changed"),
    )
    .unwrap();
    assert!(rig.manager.folder_locate_apply(review).await.is_err());
    assert_eq!(std::fs::read(rig.folder.join(".git/index")).ok(), index);
}
