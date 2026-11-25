use super::super::tests::{Rig, commit_file, isolated, rig};
use super::*;

use ChangeKind::{Added, Deleted, Modified};

fn write(folder: &Path, path: &str, contents: impl AsRef<[u8]>) {
    std::fs::write(folder.join(path), contents).expect("fixture file");
}

async fn git(rig: &Rig, folder: &Path, command: &[&str]) -> Vec<u8> {
    rig.git
        .run(folder, args(command), 1024 * 1024)
        .await
        .expect("fixture Git")
}

async fn commit_all(rig: &Rig, folder: &Path, message: &str) -> String {
    git(rig, folder, &["add", "--all"]).await;
    git(
        rig,
        folder,
        &["-c", "commit.gpgSign=false", "commit", "-m", message],
    )
    .await;
    text(&git(rig, folder, &["rev-parse", "HEAD"]).await).expect("commit")
}

/// Commits a base on `main`, then checks out `feature` and changes it every way there is, some
/// committed and some not. Returns the base.
async fn feature(rig: &Rig) -> String {
    let work = &rig.work;
    write(work, ".gitignore", "*.log\n");
    write(work, "modify.txt", "base line\n");
    write(work, "delete.txt", "deleted on the branch\n");
    write(work, "old name.txt", "renamed on the branch\n");
    write(work, "work.txt", "committed\n");
    write(work, "keep.txt", "unchanged\n");
    write(work, "revert.txt", "base text\n");
    write(work, "big.txt", "a line of text\n".repeat(1000));
    write(work, "binary.dat", [1, 0, 2, 0, 3]);
    let base = commit_all(rig, work, "base").await;
    git(rig, work, &["checkout", "-b", "feature"]).await;
    // Committed: a modification, a deletion, a rename, and a change undone below.
    write(work, "modify.txt", "branch line\n");
    std::fs::remove_file(work.join("delete.txt")).expect("delete");
    std::fs::rename(work.join("old name.txt"), work.join("new name.txt")).expect("rename");
    write(work, "revert.txt", "branch text\n");
    commit_all(rig, work, "branch").await;
    // Not committed.
    write(work, "work.txt", "uncommitted\n");
    write(work, "revert.txt", "base text\n");
    write(work, "big.txt", "another line of text\n".repeat(1000));
    write(work, "binary.dat", [1, 0, 2, 0, 4]);
    write(work, "untracked.txt", "new\n");
    write(work, "ignored.log", "ignored\n");
    // The same bytes again with another time: only its stat changed.
    write(work, "keep.txt", "unchanged\n");
    std::fs::File::options()
        .write(true)
        .open(work.join("keep.txt"))
        .expect("keep")
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1 << 30))
        .expect("an old time");
    base
}

fn files(list: &[(&str, ChangeKind)]) -> Vec<BranchFile> {
    list.iter()
        .map(|(path, kind)| BranchFile {
            path: (*path).into(),
            kind: *kind,
        })
        .collect()
}

fn feature_files() -> Vec<BranchFile> {
    files(&[
        ("big.txt", Modified),
        ("binary.dat", Modified),
        ("delete.txt", Deleted),
        ("modify.txt", Modified),
        ("new name.txt", Added),
        ("old name.txt", Deleted),
        ("untracked.txt", Added),
        ("work.txt", Modified),
    ])
}

async fn file_diff(rig: &Rig, base: &str, path: &str, max: usize) -> Option<BranchDiff> {
    rig.service
        .branch_diff(&rig.work, base, path, max)
        .await
        .expect("diff")
}

fn texts(before: Option<&str>, after: Option<&str>) -> Option<BranchDiff> {
    Some(BranchDiff {
        before: before.map(str::to_owned),
        after: after.map(str::to_owned),
        binary: false,
        truncated: false,
    })
}

#[tokio::test]
async fn changes_list_committed_uncommitted_and_untracked_files_against_the_base() {
    let rig = rig().await;
    let base = feature(&rig).await;
    let index = std::fs::read(rig.work.join(".git/index")).expect("index");
    let changes = rig
        .service
        .branch_changes(&rig.work, &base, 100)
        .await
        .expect("changes");
    assert_eq!(changes.base, base);
    assert_eq!(changes.branch.as_deref(), Some("feature"));
    // Not listed: the ignored file, the file only touched, the change undone and the unchanged.
    assert_eq!(changes.files, feature_files());
    assert!(!changes.truncated);
    assert_eq!(
        std::fs::read(rig.work.join(".git/index")).expect("index"),
        index,
        "the person's index is neither refreshed nor written"
    );
    assert_eq!(
        std::fs::read_dir(rig.service.root.join("indexes"))
            .expect("private indexes")
            .count(),
        0,
        "the private index copy is removed"
    );
    // From a subfolder, paths are still the repository's.
    std::fs::create_dir(rig.work.join("sub")).expect("subfolder");
    let from_sub = rig
        .service
        .branch_changes(&rig.work.join("sub"), &base, 100)
        .await
        .expect("from a subfolder");
    assert_eq!(from_sub.files, feature_files());
    let cut = rig
        .service
        .branch_changes(&rig.work, &base, 3)
        .await
        .expect("cut");
    assert_eq!(cut.files, feature_files()[..3]);
    assert!(cut.truncated);
    let exact = rig
        .service
        .branch_changes(&rig.work, &base, 8)
        .await
        .expect("exact");
    assert_eq!(exact.files, feature_files());
    assert!(!exact.truncated);
    let none = rig
        .service
        .branch_changes(&rig.work, &base, 0)
        .await
        .expect("none");
    assert!(none.files.is_empty() && none.truncated);
    assert!(
        rig.service
            .branch_changes(&rig.work, "main", 10)
            .await
            .is_err(),
        "a base is a full commit id"
    );
}

#[tokio::test]
async fn a_diff_reads_the_base_with_git_and_the_working_tree_from_disk() {
    let rig = rig().await;
    let base = feature(&rig).await;
    write(&rig.work, "accents.txt", "é".repeat(100));
    let diff = |path: &'static str| file_diff(&rig, &base, path, 1024 * 1024);
    assert_eq!(
        diff("modify.txt").await,
        texts(Some("base line\n"), Some("branch line\n")),
        "a committed change"
    );
    assert_eq!(
        diff("work.txt").await,
        texts(Some("committed\n"), Some("uncommitted\n")),
        "a change not committed"
    );
    assert_eq!(
        diff("new name.txt").await,
        texts(None, Some("renamed on the branch\n"))
    );
    assert_eq!(diff("untracked.txt").await, texts(None, Some("new\n")));
    assert_eq!(
        diff("old name.txt").await,
        texts(Some("renamed on the branch\n"), None)
    );
    assert_eq!(
        diff("delete.txt").await,
        texts(Some("deleted on the branch\n"), None)
    );
    assert_eq!(
        diff("big.txt").await,
        texts(
            Some(&"a line of text\n".repeat(1000)),
            Some(&"another line of text\n".repeat(1000))
        )
    );
    assert_eq!(
        file_diff(&rig, &base, "big.txt", 100).await,
        Some(BranchDiff {
            before: Some("a line of text\n".repeat(6) + "a line of "),
            after: Some("another line of text\n".repeat(4) + "another line of "),
            binary: false,
            truncated: true,
        })
    );
    assert_eq!(
        file_diff(&rig, &base, "accents.txt", 51).await,
        Some(BranchDiff {
            before: None,
            after: Some("é".repeat(25)),
            binary: false,
            truncated: true,
        }),
        "cut back to a whole character"
    );
    assert_eq!(
        diff("binary.dat").await,
        Some(BranchDiff {
            before: None,
            after: None,
            binary: true,
            truncated: false,
        })
    );
    #[cfg(unix)]
    {
        // Unlisted, the file is never opened: reading it would fail.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            rig.work.join("ignored.log"),
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("unreadable");
    }
    for path in [
        "ignored.log",
        "keep.txt",
        "revert.txt",
        "file.txt",
        "missing.txt",
        "",
        "../outside.txt",
        "./work.txt",
        "work.txt/",
        "sub//work.txt",
        ".git/config",
        ".git/index",
    ] {
        assert_eq!(diff(path).await, None, "{path:?} is not among the changes");
    }
}

#[tokio::test]
async fn an_edit_as_old_as_the_index_is_compared_by_its_contents() {
    let rig = rig().await;
    let work = &rig.work;
    git(&rig, work, &["config", "core.trustctime", "false"]).await;
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1 << 30);
    let age = |path: &Path| {
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("fixture file")
            .set_modified(old)
            .expect("an old time");
    };
    write(work, "same.txt", "aaaa");
    age(&work.join("same.txt"));
    let base = commit_all(&rig, work, "same").await;
    // Same size, same time, same file: its stat says nothing changed. Only the index being as
    // old as the file tells Git to compare the contents.
    write(work, "same.txt", "bbbb");
    age(&work.join("same.txt"));
    age(&work.join(".git/index"));
    let changes = rig
        .service
        .branch_changes(work, &base, 10)
        .await
        .expect("changes");
    assert_eq!(changes.files, files(&[("same.txt", Modified)]));
    assert_eq!(
        file_diff(&rig, &base, "same.txt", 100).await,
        texts(Some("aaaa"), Some("bbbb"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn links_in_the_working_tree_are_never_followed() {
    use std::os::unix::fs::symlink;
    let rig = rig().await;
    let base = feature(&rig).await;
    let outside = tempfile::tempdir().expect("outside");
    write(outside.path(), "secret.txt", "outside secret\n");
    symlink(outside.path(), rig.work.join("escape")).expect("folder link");
    symlink(outside.path().join("secret.txt"), rig.work.join("link.txt")).expect("file link");
    // A tracked file replaced by a link.
    std::fs::remove_file(rig.work.join("modify.txt")).expect("remove");
    symlink(
        outside.path().join("secret.txt"),
        rig.work.join("modify.txt"),
    )
    .expect("link");
    let changes = rig
        .service
        .branch_changes(&rig.work, &base, 100)
        .await
        .expect("changes");
    for (path, kind) in [
        ("escape", Added),
        ("link.txt", Added),
        ("modify.txt", Modified),
    ] {
        assert!(
            changes.files.contains(&BranchFile {
                path: path.into(),
                kind
            }),
            "{path}: {:?}",
            changes.files
        );
    }
    assert!(
        !changes
            .files
            .iter()
            .any(|file| file.path.starts_with("escape/"))
    );
    let binary = Some(BranchDiff {
        before: None,
        after: None,
        binary: true,
        truncated: false,
    });
    for path in ["escape", "link.txt", "modify.txt"] {
        assert_eq!(
            file_diff(&rig, &base, path, 1024).await,
            binary,
            "{path} is a link, not read"
        );
    }
    assert_eq!(
        file_diff(&rig, &base, "escape/secret.txt", 1024).await,
        None
    );
}

#[tokio::test]
async fn a_given_base_comes_first_and_the_default_branch_gives_a_merge_base() {
    let rig = rig().await;
    let base = text(&git(&rig, &rig.work, &["rev-parse", "HEAD"]).await).expect("base");
    git(&rig, &rig.work, &["checkout", "-b", "feature"]).await;
    commit_file(&rig, &rig.work, "feature\n").await;
    let feature_head = text(&git(&rig, &rig.work, &["rev-parse", "HEAD"]).await).expect("head");
    // The default branch moves on after the feature branch left it.
    git(&rig, &rig.work, &["checkout", "main"]).await;
    write(&rig.work, "main.txt", "later on main\n");
    commit_all(&rig, &rig.work, "main moves on").await;
    git(&rig, &rig.work, &["checkout", "feature"]).await;
    let service = &rig.service;
    let work = &rig.work;
    let recorded = |given: Option<&str>| {
        let given = given.map(str::to_owned);
        async move {
            service
                .recorded_branch_base("chat", work, given.as_deref())
                .await
                .expect("recorded base")
        }
    };
    assert_eq!(recorded(Some(&base)).await, Some(base.clone()));
    assert_eq!(
        recorded(Some(&base.to_ascii_uppercase())).await,
        Some(base.clone())
    );
    let tree = text(&git(&rig, &rig.work, &["rev-parse", "HEAD^{tree}"]).await).expect("tree");
    for given in [
        "",
        "main",
        "HEAD",
        &base[..12],
        &"1".repeat(40)[..],
        &"1".repeat(64)[..],
        tree.as_str(),
    ] {
        assert_eq!(
            recorded(Some(given)).await,
            None,
            "{given:?} is no commit id"
        );
    }
    assert_eq!(recorded(None).await, None, "no worktree of this chat here");
    assert_eq!(
        service
            .default_branch_base(&rig.work, "main")
            .await
            .expect("merge base"),
        Some(base.clone()),
        "where the branch left main, not where main is now"
    );
    for name in [
        "feature",
        "trunk",
        "main..feature",
        "-main",
        "main^",
        "main@{1}",
        "",
    ] {
        assert_eq!(
            service
                .default_branch_base(&rig.work, name)
                .await
                .expect("no base"),
            None,
            "{name:?}"
        );
    }
    let changes = service
        .branch_changes(&rig.work, &base, 10)
        .await
        .expect("changes");
    assert_eq!(changes.files, files(&[("file.txt", Modified)]));
    git(&rig, &rig.work, &["checkout", "--detach"]).await;
    assert_eq!(
        service
            .default_branch_base(&rig.work, "main")
            .await
            .expect("detached"),
        None
    );
    let detached = service
        .branch_changes(&rig.work, &base, 10)
        .await
        .expect("detached changes");
    assert_eq!(detached.branch, None);
    assert_eq!(
        service
            .recorded_branch_base("chat", &rig.work, Some(&feature_head))
            .await
            .expect("given"),
        Some(feature_head)
    );
    let plain = tempfile::tempdir().expect("plain folder");
    assert_eq!(
        service
            .recorded_branch_base("chat", plain.path(), Some(&base))
            .await
            .expect("plain"),
        None
    );
    assert_eq!(
        service
            .default_branch_base(plain.path(), "main")
            .await
            .expect("plain"),
        None
    );
    assert!(
        service
            .branch_changes(plain.path(), &base, 10)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_worktrees_base_is_the_commit_recorded_for_its_chat() {
    let rig = rig().await;
    let worktree = isolated(&rig).await;
    let path = Path::new(&worktree.path);
    commit_file(&rig, path, "isolated change\n").await;
    write(path, "new.txt", "new\n");
    let service = &rig.service;
    assert_eq!(
        service
            .recorded_branch_base("chat", path, None)
            .await
            .expect("recorded"),
        Some(worktree.base_commit.clone())
    );
    assert_eq!(
        service
            .recorded_branch_base("another chat", path, None)
            .await
            .expect("another chat"),
        None
    );
    assert_eq!(
        service
            .recorded_branch_base("chat", &rig.work, None)
            .await
            .expect("source"),
        None,
        "the source folder is not the chat's worktree"
    );
    let head = text(&git(&rig, path, &["rev-parse", "HEAD"]).await).expect("head");
    assert_eq!(
        service
            .recorded_branch_base("chat", path, Some(&head))
            .await
            .expect("given"),
        Some(head)
    );
    let changes = service
        .branch_changes(path, &worktree.base_commit, 10)
        .await
        .expect("changes");
    assert_eq!(changes.branch.as_deref(), Some(worktree.branch.as_str()));
    assert_eq!(
        changes.files,
        files(&[("file.txt", Modified), ("new.txt", Added)])
    );
    assert_eq!(
        service
            .branch_diff(path, &worktree.base_commit, "file.txt", 1024)
            .await
            .expect("diff"),
        texts(Some("initial\n"), Some("isolated change\n"))
    );
}

#[tokio::test]
async fn a_cut_read_comes_back_only_when_asked_for() {
    let rig = rig().await;
    let command = || args(&["cat-file", "blob", "HEAD:file.txt"]);
    assert!(rig.git.output(&rig.work, command(), 4).await.is_err());
    let cut = rig
        .git
        .output_with(
            &rig.work,
            command(),
            4,
            Extra {
                allow_cut: true,
                ..Extra::default()
            },
        )
        .await
        .expect("cut");
    assert!(cut.stdout_truncated);
    assert_eq!(cut.stdout, b"init");
}

#[test]
fn cut_listings_keep_whole_records_and_say_they_are_partial() {
    let mut listing = Listing::default();
    listing
        .add_tracked(b"M\0a.txt\0D\0b.txt\0A\0c.t", true)
        .expect("tracked");
    assert!(listing.partial);
    let mut listing = Listing::default();
    listing
        .add_tracked(b"M\0a.txt\0D\0b.txt\0T\0", true)
        .expect("tracked");
    assert_eq!(
        listing.files,
        BTreeMap::from([("a.txt".into(), Modified), ("b.txt".into(), Deleted)])
    );
    assert!(listing.partial);
    // Untracked: a nested repository's folder, a path gone from the index but on disk, and
    // paths that cannot be shown.
    let mut listing = Listing::default();
    listing.add_tracked(b"D\0b.txt\0", false).expect("tracked");
    assert!(!listing.partial);
    listing.add_untracked(b"b.txt\0nested/\0\xff\0line\nbreak\0", false);
    assert_eq!(
        listing.files,
        BTreeMap::from([("b.txt".into(), Modified), ("nested".into(), Added)])
    );
    assert!(listing.partial);
    assert!(
        Listing::default()
            .add_tracked(b"R100\0a\0b\0", false)
            .is_err()
    );
}

#[test]
fn a_binary_side_gives_no_text_and_a_cut_ends_on_a_character() {
    assert_eq!(side_text(b"plain".to_vec(), 10).as_deref(), Some("plain"));
    assert_eq!(side_text("éé".as_bytes().to_vec(), 3).as_deref(), Some("é"));
    assert_eq!(side_text(vec![b'a', 0, b'b'], 10), None);
    assert_eq!(side_text(vec![0xff, b'a'], 10), None);
    assert_eq!(side_text(vec![0xff, b'a'], 1), None);
    let mut late = vec![b'a'; BINARY_PROBE];
    late.push(0);
    assert!(
        side_text(late, BINARY_PROBE + 1).is_some(),
        "a NUL after the first 8000 bytes"
    );
    assert_eq!(
        diff(Side::Bytes(b"text".to_vec()), Side::Special, 10),
        BranchDiff {
            before: None,
            after: None,
            binary: true,
            truncated: false,
        }
    );
    assert_eq!(
        diff(Side::Bytes(vec![0; 20]), Side::Absent, 10),
        BranchDiff {
            before: None,
            after: None,
            binary: true,
            truncated: true,
        }
    );
}
