use super::tests::{Rig, commit_file, rig};
use super::*;
use ghost_protocol::git::{ChatFolderMode, WorktreeChatPlan, WorktreeState};

fn request<'a>(rig: &'a Rig, chat_id: &'a str) -> NewChatRequest<'a> {
    NewChatRequest {
        chat_id,
        folder: &rig.work,
        start_branch: None,
        branch: None,
        title: None,
        bring_changes: false,
    }
}

async fn plan(rig: &Rig, request: NewChatRequest<'_>) -> WorktreeChatPlan {
    match rig
        .service
        .new_chat_preview(request)
        .await
        .expect("preview")
    {
        NewChatPlan::Worktree(plan) => *plan,
        NewChatPlan::Direct(message) => panic!("unexpected direct outcome: {message}"),
    }
}

async fn head(rig: &Rig, folder: &Path, reference: &str) -> String {
    git::text(
        &rig.git
            .run(folder, git::args(&["rev-parse", reference]), 256)
            .await
            .expect("rev-parse"),
    )
    .expect("text")
}

async fn branch_present(rig: &Rig, branch: &str) -> bool {
    create::branch_exists(&rig.git, &rig.work, branch)
        .await
        .expect("branch check")
}

#[tokio::test]
async fn new_chat_worktree_starts_from_the_current_branch_and_preserves_the_source() {
    let rig = rig().await;
    std::fs::write(rig.work.join("file.txt"), "staged\n").expect("staged");
    rig.git
        .run(&rig.work, git::args(&["add", "--", "file.txt"]), 8192)
        .await
        .expect("stage");
    std::fs::write(rig.work.join("file.txt"), "unstaged\n").expect("unstaged");
    let index = std::fs::read(rig.work.join(".git/index")).expect("index");
    let source_head = std::fs::read(rig.work.join(".git/HEAD")).expect("head");
    let main = head(&rig, &rig.work, "HEAD").await;
    let preview = plan(
        &rig,
        NewChatRequest {
            title: Some("Fix the login bug!"),
            ..request(&rig, "new-chat")
        },
    )
    .await;
    assert_eq!(preview.starting_branch, "main");
    assert_eq!(preview.starting_commit, main);
    assert_eq!(preview.branch, "ghost/fix-the-login-bug");
    assert!(preview.branch_generated);
    assert_eq!(preview.chip.mode, ChatFolderMode::Worktree);
    assert_eq!(preview.chip.starting_branch.as_deref(), Some("main"));
    assert_eq!(
        preview.chip.branch.as_deref(),
        Some("ghost/fix-the-login-bug")
    );
    assert_eq!(preview.local_branches, vec!["main".to_owned()]);
    assert!(
        preview
            .warnings
            .iter()
            .any(|warning| warning.contains("stay in the source folder"))
    );
    assert!(!Path::new(&preview.path).starts_with(&rig.work));
    assert!(
        !Path::new(&preview.path).exists(),
        "preview creates nothing"
    );
    assert!(rig.service.read_records().expect("records").is_empty());
    assert!(
        rig.service
            .new_chat_apply("another-chat", &preview.token)
            .await
            .is_err(),
        "a token is bound to its chat"
    );
    let preview = plan(
        &rig,
        NewChatRequest {
            title: Some("Fix the login bug!"),
            ..request(&rig, "new-chat")
        },
    )
    .await;
    let worktree = rig
        .service
        .new_chat_apply("new-chat", &preview.token)
        .await
        .expect("apply");
    assert_eq!(
        worktree.state,
        WorktreeState::Ready,
        "{:?}",
        worktree.detail
    );
    assert_eq!(worktree.chat_id, "new-chat");
    assert_eq!(worktree.branch, "ghost/fix-the-login-bug");
    assert_eq!(worktree.base_commit, main);
    assert_eq!(
        head(&rig, Path::new(&worktree.path), "HEAD").await,
        main,
        "the worktree starts at the reviewed commit"
    );
    assert_eq!(
        std::fs::read(Path::new(&worktree.path).join("file.txt")).expect("checkout"),
        b"initial\n"
    );
    assert_eq!(
        std::fs::read(rig.work.join(".git/index")).expect("same index"),
        index
    );
    assert_eq!(
        std::fs::read(rig.work.join(".git/HEAD")).expect("same head"),
        source_head
    );
    assert_eq!(
        std::fs::read(rig.work.join("file.txt")).expect("same bytes"),
        b"unstaged\n"
    );
    assert_eq!(
        rig.service.list("new-chat").await.expect("registry"),
        vec![worktree.clone()]
    );
    assert!(
        rig.service
            .new_chat_apply("new-chat", &preview.token)
            .await
            .is_err(),
        "a token is single use"
    );
    // The association behaves like a chat moved into a worktree.
    let (record, target) = rig
        .service
        .relocation_target(
            "new-chat",
            &worktree.id,
            ghost_protocol::git::WorktreeDestination::Direct,
        )
        .await
        .expect("handoff target");
    assert_eq!(record, worktree);
    assert_eq!(target, rig.work.canonicalize().expect("source"));
}

#[tokio::test]
async fn another_local_branch_can_be_the_start_and_must_not_move_after_preview() {
    let rig = rig().await;
    let main = head(&rig, &rig.work, "HEAD").await;
    rig.git
        .run(&rig.work, git::args(&["branch", "release"]), 8192)
        .await
        .expect("branch");
    rig.git
        .run(&rig.work, git::args(&["checkout", "-q", "release"]), 8192)
        .await
        .expect("checkout release");
    commit_file(&rig, &rig.work, "release\n").await;
    let release = head(&rig, &rig.work, "HEAD").await;
    rig.git
        .run(&rig.work, git::args(&["checkout", "-q", "main"]), 8192)
        .await
        .expect("checkout main");
    // A tag with the same name never shadows the local branch.
    rig.git
        .run(&rig.work, git::args(&["tag", "release", "main"]), 8192)
        .await
        .expect("ambiguous tag");
    assert!(
        rig.service
            .new_chat_preview(NewChatRequest {
                start_branch: Some("missing"),
                ..request(&rig, "chat")
            })
            .await
            .is_err()
    );
    let preview = plan(
        &rig,
        NewChatRequest {
            start_branch: Some("release"),
            ..request(&rig, "chat")
        },
    )
    .await;
    assert_eq!(preview.starting_commit, release);
    assert!(preview.chip.label.contains("release"));
    assert!(preview.local_branches.contains(&"release".to_owned()));
    let worktree = rig
        .service
        .new_chat_apply("chat", &preview.token)
        .await
        .expect("apply");
    assert_eq!(head(&rig, Path::new(&worktree.path), "HEAD").await, release);
    assert_eq!(
        head(&rig, &rig.work, "HEAD").await,
        main,
        "source unchanged"
    );
    let moved = plan(
        &rig,
        NewChatRequest {
            start_branch: Some("release"),
            ..request(&rig, "second")
        },
    )
    .await;
    rig.git
        .run(
            &rig.work,
            git::args(&["branch", "-f", "release", "main"]),
            8192,
        )
        .await
        .expect("move release");
    assert!(
        rig.service
            .new_chat_apply("second", &moved.token)
            .await
            .expect_err("moved")
            .to_string()
            .contains("moved after the preview")
    );
    assert!(!Path::new(&moved.path).exists());
    assert!(!branch_present(&rig, &moved.branch).await);
}

#[tokio::test]
async fn branch_name_collisions_suffix_generated_names_and_refuse_typed_reuse() {
    let rig = rig().await;
    for branch in ["ghost/fix", "ghost/fix-2", "feature/one"] {
        rig.git
            .run(&rig.work, git::args(&["branch", branch]), 8192)
            .await
            .expect("fixture branch");
    }
    let generated = plan(
        &rig,
        NewChatRequest {
            title: Some("Fix"),
            ..request(&rig, "chat")
        },
    )
    .await;
    assert_eq!(generated.branch, "ghost/fix-3");
    for taken in ["main", "MAIN", "ghost/fix", "feature/one/deeper", "feature"] {
        assert!(
            rig.service
                .new_chat_preview(NewChatRequest {
                    branch: Some(taken),
                    ..request(&rig, "chat")
                })
                .await
                .expect_err(taken)
                .to_string()
                .contains("never reused"),
            "{taken}"
        );
    }
    let typed = plan(
        &rig,
        NewChatRequest {
            branch: Some("feature/two"),
            ..request(&rig, "chat")
        },
    )
    .await;
    assert_eq!(typed.branch, "feature/two");
    assert!(!typed.branch_generated);
    // A branch created after the preview is never reused by apply.
    rig.git
        .run(&rig.work, git::args(&["branch", "feature/two"]), 8192)
        .await
        .expect("late branch");
    let late_head = head(&rig, &rig.work, "refs/heads/feature/two").await;
    assert!(
        rig.service
            .new_chat_apply("chat", &typed.token)
            .await
            .is_err()
    );
    assert!(!Path::new(&typed.path).exists());
    assert_eq!(
        head(&rig, &rig.work, "refs/heads/feature/two").await,
        late_head
    );
    assert!(rig.service.read_records().expect("records").is_empty());
    let untitled = plan(&rig, request(&rig, "chat")).await;
    assert!(untitled.branch.starts_with("ghost/"));
    assert_eq!(untitled.branch.len(), "ghost/".len() + 8);
}

#[tokio::test]
async fn invalid_branch_names_are_refused_before_anything_is_created() {
    let rig = rig().await;
    for name in [
        "bad name",
        "-force",
        "a..b",
        "x.lock",
        "@{-1}",
        "HEAD",
        "ghost/con",
        "feat~1",
        "trailing/",
        "a//b",
        "quote\"name",
        "dot/.hidden",
        "unicode-\u{e9}",
    ] {
        assert!(
            rig.service
                .new_chat_preview(NewChatRequest {
                    branch: Some(name),
                    ..request(&rig, "chat")
                })
                .await
                .is_err(),
            "{name}"
        );
    }
    assert!(rig.service.read_records().expect("records").is_empty());
    let valid = plan(
        &rig,
        NewChatRequest {
            branch: Some("feature/new-ui_2.0"),
            ..request(&rig, "chat")
        },
    )
    .await;
    assert_eq!(valid.branch, "feature/new-ui_2.0");
}

#[tokio::test]
async fn non_git_folders_fall_back_to_direct_with_a_reason() {
    let rig = rig().await;
    let plain = tempfile::tempdir().expect("plain");
    let outcome = rig
        .service
        .new_chat_preview(NewChatRequest {
            folder: plain.path(),
            ..request(&rig, "chat")
        })
        .await
        .expect("direct outcome");
    assert!(
        matches!(&outcome, NewChatPlan::Direct(message) if message.contains("not a Git repository"))
    );
    std::fs::create_dir(rig.work.join("nested")).expect("nested");
    let nested = rig.work.join("nested");
    let outcome = rig
        .service
        .new_chat_preview(NewChatRequest {
            folder: &nested,
            ..request(&rig, "chat")
        })
        .await
        .expect("nested outcome");
    assert!(matches!(&outcome, NewChatPlan::Direct(message) if message.contains("not its root")));
    assert!(rig.service.read_records().expect("records").is_empty());
}

#[tokio::test]
async fn without_git_only_a_folder_lacking_repository_metadata_falls_back_to_direct() {
    let dir = tempfile::tempdir().expect("fixture");
    let profile =
        crate::profile::Profiles::new(crate::paths::DataRoot::at(dir.path().join("data")))
            .open_active()
            .expect("profile");
    let env = EnvResolver::fixed(ghost_process::ResolvedEnv {
        vars: Default::default(),
        source: ghost_process::EnvSource::Fallback {
            reason: "no Git fixture".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    let service = GitWorkspaces::new(&profile, env, LeftoverRegistry::disabled());
    let plain = dir.path().join("plain");
    std::fs::create_dir(&plain).expect("plain");
    let request = || NewChatRequest {
        chat_id: "chat",
        folder: &plain,
        start_branch: None,
        branch: None,
        title: None,
        bring_changes: false,
    };
    assert!(matches!(
        service.new_chat_preview(request()).await.expect("direct"),
        NewChatPlan::Direct(message) if message.contains("not a Git repository")
    ));
    std::fs::create_dir(plain.join(".git")).expect("repository metadata fixture");
    assert!(
        service
            .new_chat_preview(request())
            .await
            .expect_err("Git is required for a repository")
            .to_string()
            .contains("Install Git")
    );
}

#[tokio::test]
async fn bring_changes_copies_working_files_only_from_the_current_commit() {
    let rig = rig().await;
    std::fs::write(rig.work.join("file.txt"), b"working\r\nbytes\r\n").expect("edit");
    std::fs::write(rig.work.join("new.txt"), b"untracked").expect("untracked");
    let index = std::fs::read(rig.work.join(".git/index")).expect("index");
    rig.git
        .run(&rig.work, git::args(&["branch", "other"]), 8192)
        .await
        .expect("same-commit branch");
    let preview = plan(
        &rig,
        NewChatRequest {
            bring_changes: true,
            ..request(&rig, "chat")
        },
    )
    .await;
    assert!(preview.bring_changes);
    let worktree = rig
        .service
        .new_chat_apply("chat", &preview.token)
        .await
        .expect("apply");
    assert_eq!(
        worktree.state,
        WorktreeState::Ready,
        "{:?}",
        worktree.detail
    );
    let path = Path::new(&worktree.path);
    assert_eq!(
        std::fs::read(path.join("file.txt")).expect("copied"),
        b"working\r\nbytes\r\n"
    );
    assert_eq!(
        std::fs::read(path.join("new.txt")).expect("copied"),
        b"untracked"
    );
    assert_eq!(
        std::fs::read(rig.work.join(".git/index")).expect("same index"),
        index
    );
    assert_eq!(
        std::fs::read(rig.work.join("file.txt")).expect("source kept"),
        b"working\r\nbytes\r\n"
    );
    // A branch at another commit cannot receive working files made against HEAD.
    commit_file(&rig, path, "worktree commit\n").await;
    let elsewhere = head(&rig, path, "HEAD").await;
    rig.git
        .run(
            &rig.work,
            git::args(&["branch", "-f", "other", &elsewhere]),
            8192,
        )
        .await
        .expect("move other");
    assert!(
        rig.service
            .new_chat_preview(NewChatRequest {
                start_branch: Some("other"),
                bring_changes: true,
                ..request(&rig, "second")
            })
            .await
            .expect_err("other commit")
            .to_string()
            .contains("current commit")
    );
}

#[tokio::test]
async fn rollback_removes_only_an_unchanged_new_worktree_and_branch() {
    let rig = rig().await;
    let preview = plan(&rig, request(&rig, "chat")).await;
    let worktree = rig
        .service
        .new_chat_apply("chat", &preview.token)
        .await
        .expect("apply");
    rig.service
        .new_chat_rollback("chat", &worktree.id, false)
        .await
        .expect("rollback");
    assert!(!Path::new(&worktree.path).exists());
    assert!(!branch_present(&rig, &worktree.branch).await);
    assert!(rig.service.read_records().expect("records").is_empty());
    let registered = git::registrations(&rig.git, &rig.work)
        .await
        .expect("registrations");
    assert_eq!(registered.len(), 1, "only the main worktree remains");

    // New work in the worktree is never discarded by rollback.
    let preview = plan(&rig, request(&rig, "chat")).await;
    let worktree = rig
        .service
        .new_chat_apply("chat", &preview.token)
        .await
        .expect("apply");
    let path = PathBuf::from(&worktree.path);
    commit_file(&rig, &path, "agent work\n").await;
    let committed = head(&rig, &path, "HEAD").await;
    assert!(
        rig.service
            .new_chat_rollback("chat", &worktree.id, false)
            .await
            .is_err()
    );
    assert!(path.join("file.txt").exists());
    assert_eq!(
        head(&rig, &rig.work, &format!("refs/heads/{}", worktree.branch)).await,
        committed
    );
    let kept = rig.service.read_records().expect("records");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].state, WorktreeState::RecoveryRequired);
    assert!(
        kept[0]
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("could not be saved"))
    );

    // Uncommitted edits block a rollback that did not bring changes.
    let preview = plan(&rig, request(&rig, "third")).await;
    let worktree = rig
        .service
        .new_chat_apply("third", &preview.token)
        .await
        .expect("apply");
    std::fs::write(Path::new(&worktree.path).join("draft.txt"), "keep").expect("draft");
    assert!(
        rig.service
            .new_chat_rollback("third", &worktree.id, false)
            .await
            .is_err()
    );
    assert!(Path::new(&worktree.path).join("draft.txt").exists());
    assert!(branch_present(&rig, &worktree.branch).await);
}
