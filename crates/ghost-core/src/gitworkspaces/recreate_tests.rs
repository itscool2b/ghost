use super::tests::{commit_file, isolated, rig};
use super::*;
use ghost_protocol::git::WorktreeState;

async fn rev(rig: &super::tests::Rig, folder: &Path, reference: &str) -> String {
    git::text(
        &rig.git
            .run(folder, git::args(&["rev-parse", reference]), 256)
            .await
            .expect("rev-parse"),
    )
    .expect("text")
}

#[test]
fn porcelain_registrations_keep_branch_lock_and_path_per_entry() {
    let parsed = git::parse_registrations(
        "worktree C:/repo\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n\nworktree C:/data/worktrees/abc\nHEAD 2222222222222222222222222222222222222222\nbranch refs/heads/ghost/x\nlocked Ghost id-1\nprunable gitdir file points to non-existent location\n\nworktree C:/other\nHEAD 3333333333333333333333333333333333333333\ndetached\nlocked\n",
    );
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0].branch.as_deref(), Some("refs/heads/main"));
    assert_eq!(parsed[0].locked, None);
    assert_eq!(parsed[1].path, "C:/data/worktrees/abc");
    assert_eq!(parsed[1].locked.as_deref(), Some("Ghost id-1"));
    assert_eq!(parsed[2].branch, None);
    assert_eq!(parsed[2].locked.as_deref(), Some(""));
    let temp = tempfile::tempdir().expect("temp");
    let path = temp.path().join("abc");
    let spelled = path.to_string_lossy().replace('\\', "/");
    assert!(git::same_path_text(&format!("{spelled}/"), &path));
    assert!(!git::same_path_text(&format!("{spelled}-other"), &path));
}

#[tokio::test]
async fn recreate_after_manual_deletion_replaces_the_locked_missing_registration() {
    let rig = rig().await;
    let worktree = isolated(&rig).await;
    let path = PathBuf::from(&worktree.path);
    assert!(
        rig.service
            .recreate_preview("chat", &worktree.id)
            .await
            .expect_err("folder present")
            .to_string()
            .contains("still exists")
    );
    commit_file(&rig, &path, "committed work\n").await;
    let committed = rev(&rig, &path, "HEAD").await;
    std::fs::write(path.join("uncommitted.txt"), "lost").expect("uncommitted");
    std::fs::remove_dir_all(&path).expect("deleted by hand");
    let listed = rig.service.list("chat").await.expect("list");
    assert_eq!(listed[0].state, WorktreeState::Missing);
    // Git keeps the locked registration of the missing folder. A single force cannot replace it.
    let single = rig
        .git
        .output(
            &rig.work,
            vec![
                "worktree".into(),
                "add".into(),
                "-f".into(),
                "--lock".into(),
                "--reason".into(),
                "fixture".into(),
                "--".into(),
                path.as_os_str().to_owned(),
                worktree.branch.clone().into(),
            ],
            8192,
        )
        .await
        .expect("single force");
    assert_ne!(single.exit.and_then(|exit| exit.code), Some(0));
    assert!(!path.exists());
    let preview = rig
        .service
        .recreate_preview("chat", &worktree.id)
        .await
        .expect("preview");
    assert_eq!(preview.commit, committed);
    assert_eq!(preview.branch, worktree.branch);
    assert!(preview.warnings.iter().any(|warning| {
        warning.contains("Uncommitted files that were in the deleted folder are gone")
    }));
    assert!(!path.exists(), "preview creates nothing");
    assert!(
        rig.service
            .recreate_apply("other", &worktree.id, &preview.token)
            .await
            .is_err()
    );
    let preview = rig
        .service
        .recreate_preview("chat", &worktree.id)
        .await
        .expect("preview again");
    let result = rig
        .service
        .recreate_apply("chat", &worktree.id, &preview.token)
        .await
        .expect("recreate");
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.worktree.state, WorktreeState::Ready);
    assert_eq!(result.commit.as_deref(), Some(committed.as_str()));
    assert_eq!(
        std::fs::read(path.join("file.txt")).expect("committed file"),
        b"committed work\n"
    );
    assert!(!path.join("uncommitted.txt").exists());
    assert_eq!(rev(&rig, &path, "HEAD").await, committed);
    let registered = git::registrations(&rig.git, &rig.work)
        .await
        .expect("registrations");
    assert_eq!(registered.len(), 2, "the stale registration was replaced");
    assert!(registered.iter().any(|entry| {
        entry.locked.as_deref() == Some(format!("auracode {}", worktree.id).as_str())
            && entry.branch.as_deref() == Some(format!("refs/heads/{}", worktree.branch).as_str())
    }));
    assert_eq!(
        rig.service.list("chat").await.expect("list")[0].state,
        WorktreeState::Ready
    );
    // The recreated worktree supports the normal handoff and actions again.
    rig.service
        .relocation_target(
            "chat",
            &worktree.id,
            ghost_protocol::git::WorktreeDestination::Isolated,
        )
        .await
        .expect("handoff target");
    assert!(
        rig.service
            .recreate_apply("chat", &worktree.id, &preview.token)
            .await
            .is_err(),
        "a token is single use"
    );
}

#[tokio::test]
async fn recreate_is_refused_when_the_branch_is_elsewhere_moved_or_deleted() {
    let rig = rig().await;
    let worktree = isolated(&rig).await;
    let path = PathBuf::from(&worktree.path);
    std::fs::remove_dir_all(&path).expect("deleted by hand");
    // The branch is checked out in the main worktree.
    rig.git
        .run(
            &rig.work,
            git::args(&[
                "checkout",
                "-q",
                "--ignore-other-worktrees",
                &worktree.branch,
            ]),
            8192,
        )
        .await
        .expect("checkout elsewhere");
    assert!(
        rig.service
            .recreate_preview("chat", &worktree.id)
            .await
            .expect_err("checked out elsewhere")
            .to_string()
            .contains("checked out in another worktree")
    );
    rig.git
        .run(&rig.work, git::args(&["checkout", "-q", "main"]), 8192)
        .await
        .expect("back to main");
    // A branch that moves after preview needs a new review.
    let preview = rig
        .service
        .recreate_preview("chat", &worktree.id)
        .await
        .expect("preview");
    commit_file(&rig, &rig.work, "main moves\n").await;
    let main = rev(&rig, &rig.work, "HEAD").await;
    rig.git
        .run(
            &rig.work,
            vec![
                "update-ref".into(),
                format!("refs/heads/{}", worktree.branch).into(),
                main.into(),
            ],
            8192,
        )
        .await
        .expect("move branch");
    assert!(
        rig.service
            .recreate_apply("chat", &worktree.id, &preview.token)
            .await
            .expect_err("moved")
            .to_string()
            .contains("changed after preview")
    );
    assert!(!path.exists());
    // A deleted branch cannot be recreated.
    rig.git
        .run(
            &rig.work,
            vec![
                "update-ref".into(),
                "-d".into(),
                format!("refs/heads/{}", worktree.branch).into(),
            ],
            8192,
        )
        .await
        .expect("delete branch");
    assert!(
        rig.service
            .recreate_preview("chat", &worktree.id)
            .await
            .expect_err("deleted branch")
            .to_string()
            .contains("no longer exists")
    );
    assert!(!path.exists());
    assert_eq!(
        rig.service.read_records().expect("records")[0].state,
        WorktreeState::Ready,
        "refusals never rewrite the record"
    );
}
