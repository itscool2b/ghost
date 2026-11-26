//! The cleanup on temporary folders with real Git. Nothing here touches the real data folder or
//! any real agent home.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::*;

const FAST: Retry = Retry {
    attempts: 3,
    step: Duration::from_millis(10),
};

/// Git for building fixtures, with no global or system configuration of the person's.
fn fixture_git(dir: &Path, args: &[&str]) -> String {
    let empty = dir.parent().unwrap_or(dir).join(".fixture-gitconfig");
    if !empty.exists() {
        let _ = std::fs::write(&empty, "");
    }
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=auracode test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", &empty)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The person's repository with one commit on main.
fn repository(parent: &Path) -> PathBuf {
    let repo = parent.join("project");
    std::fs::create_dir_all(&repo).unwrap();
    fixture_git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("README.md"), "project\n").unwrap();
    fixture_git(&repo, &["add", "README.md"]);
    fixture_git(&repo, &["commit", "-q", "-m", "first"]);
    repo
}

/// A worktree made the way Ghost makes one: in the data folder, on its own branch, locked.
fn auracode_worktree(repo: &Path, root: &Path, name: &str, branch: &str) -> PathBuf {
    let path = root
        .join("profiles")
        .join("local")
        .join("worktrees")
        .join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    fixture_git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "--lock",
            "--reason",
            &format!("auracode {name}"),
            "-b",
            branch,
            path.to_str().unwrap(),
            "HEAD",
        ],
    );
    path
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A data folder with what Ghost keeps: identity, a profile with its database, checkpoints
/// (with a read-only Git object), logs, adapters and the Windows webview profile.
fn data_folder(root: &Path) {
    write(&root.join("device.json"), "{}");
    write(&root.join("state.json"), "{}");
    let profile = root.join("profiles").join("local");
    write(&profile.join("profile.json"), "{}");
    write(&profile.join("aura.db"), "db");
    write(&profile.join("owner.lock"), "");
    write(&profile.join("logs/protocol/chat/spawn.jsonl"), "{}");
    let object = profile.join("checkpoints/folder.git/objects/ab/cdef");
    write(&object, "object");
    let mut permissions = std::fs::metadata(&object).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&object, permissions).unwrap();
    write(
        &root.join("adapters/claude-agent-acp-1.0.0/node_modules/x/index.js"),
        "x",
    );
    write(&root.join("EBWebView/Default/Cookies"), "cookies");
}

/// The person's agent homes, with configuration Ghost must never touch.
fn agent_homes(home: &Path) -> Vec<PathBuf> {
    let files = vec![
        home.join(".claude/settings.json"),
        home.join(".claude/CLAUDE.md"),
        home.join(".codex/config.toml"),
        home.join(".pi/agent/settings.json"),
        home.join(".claude.json"),
    ];
    for file in &files {
        write(file, "the person's own");
    }
    files
}

struct Machine {
    _temp: tempfile::TempDir,
    home: PathBuf,
    dirs: PlatformDirs,
}

fn machine() -> Machine {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().to_path_buf();
    let home = base.join("home");
    let dirs = PlatformDirs {
        home: Some(home.clone()),
        local_data: Some(base.join("local")),
        config: Some(base.join("roaming")),
        cache: Some(base.join("local")),
        temp: Some(base.join("tmp")),
    };
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(base.join("tmp")).unwrap();
    Machine {
        _temp: temp,
        home,
        dirs,
    }
}

#[test]
fn a_machine_cleanup_removes_everything_and_touches_no_agent_home_or_repository() {
    let m = machine();
    let targets = Targets::machine(Os::Windows, &m.dirs, None);
    let root = targets.data_roots[0].clone();
    data_folder(&root);
    write(
        &m.dirs
            .config
            .clone()
            .unwrap()
            .join(crate::paths::APP_IDENTIFIER)
            .join(".window-state.json"),
        "{}",
    );
    let temp = m.dirs.temp.clone().unwrap();
    write(
        &temp.join("Ghost-0.2.0-updater-abc123/Ghost_0.2.0_x64-setup.exe"),
        "exe",
    );
    write(&temp.join("ghost-cli-probe-xyz/repo/file"), "x");
    write(
        &temp.join("auracode-preview/d/device.json"),
        "another app's test data",
    );
    write(&temp.join("unrelated.txt"), "not ours");
    let agents = agent_homes(&m.home);
    let repo = repository(&m.home.join("code"));
    let clean = auracode_worktree(&repo, &root, "clean0000001", "ghost/clean");

    let git = GitTool::from_path();
    let plan = plan(&targets, &git);
    assert_eq!(plan.worktrees.len(), 1);
    assert!(!plan.worktrees[0].unsaved(), "{:?}", plan.worktrees[0]);
    let kinds: Vec<_> = plan.items.iter().map(|item| item.kind).collect();
    assert_eq!(
        kinds,
        [
            ItemKind::Data,
            ItemKind::App,
            ItemKind::Temporary,
            ItemKind::Temporary
        ]
    );
    let report = execute(&plan, &WorktreeChoice::KeepUnsaved, &git, &FAST);

    assert!(report.complete(), "{:?}", report.failed);
    assert!(!root.exists());
    assert!(
        !m.dirs
            .config
            .clone()
            .unwrap()
            .join(crate::paths::APP_IDENTIFIER)
            .exists()
    );
    assert!(!temp.join("Ghost-0.2.0-updater-abc123").exists());
    assert!(!temp.join("ghost-cli-probe-xyz").exists());
    assert!(temp.join("auracode-preview/d/device.json").exists());
    assert!(temp.join("unrelated.txt").exists());
    assert!(!clean.exists());
    for file in agents {
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "the person's own");
    }
    // The repository: its files, its branches (Ghost's too) and no stale registration.
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "project\n"
    );
    let branches = fixture_git(&repo, &["branch", "--list", "--format=%(refname:short)"]);
    assert!(
        branches.lines().any(|branch| branch == "main"),
        "{branches}"
    );
    assert!(
        branches.lines().any(|branch| branch == "ghost/clean"),
        "{branches}"
    );
    let registered = fixture_git(&repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(registered.matches("worktree ").count(), 1, "{registered}");

    // A second run finds nothing and changes nothing.
    let again = super::plan(&targets, &git);
    assert!(again.is_empty(), "{:?}", again.items);
    let second = execute(&again, &WorktreeChoice::KeepUnsaved, &git, &FAST);
    assert!(second.complete() && second.removed.is_empty() && second.kept.is_empty());
}

#[test]
fn worktrees_with_unsaved_work_are_reported_and_kept_unless_chosen() {
    let m = machine();
    let targets = Targets::machine(Os::Windows, &m.dirs, None);
    let root = targets.data_roots[0].clone();
    data_folder(&root);
    let repo = repository(&m.home.join("code"));
    let dirty = auracode_worktree(&repo, &root, "dirty0000001", "ghost/dirty");
    write(&dirty.join("notes.txt"), "not committed");
    std::fs::write(dirty.join("README.md"), "edited\n").unwrap();
    let ahead = auracode_worktree(&repo, &root, "ahead0000001", "ghost/ahead");
    write(&ahead.join("feature.txt"), "feature");
    fixture_git(&ahead, &["add", "feature.txt"]);
    fixture_git(&ahead, &["commit", "-q", "-m", "feature"]);
    let detached = auracode_worktree(&repo, &root, "detach000001", "ghost/detached");
    fixture_git(&detached, &["checkout", "-q", "--detach"]);
    write(&detached.join("lost.txt"), "lost");
    fixture_git(&detached, &["add", "lost.txt"]);
    fixture_git(&detached, &["commit", "-q", "-m", "on no branch"]);
    let clean = auracode_worktree(&repo, &root, "clean0000002", "ghost/clean");

    let git_tool = GitTool::from_path();
    let plan = plan(&targets, &git_tool);
    let find = |path: &Path| {
        plan.worktrees
            .iter()
            .find(|worktree| worktree.path == path)
            .unwrap()
            .clone()
    };
    let dirty_report = find(&dirty);
    assert_eq!(dirty_report.changed_files, 2);
    assert_eq!(dirty_report.unique_commits, 0);
    assert_eq!(dirty_report.branch.as_deref(), Some("ghost/dirty"));
    assert_eq!(
        dirty_report
            .repository
            .as_deref()
            .map(std::fs::canonicalize)
            .map(Result::unwrap),
        Some(std::fs::canonicalize(&repo).unwrap())
    );
    assert_eq!(find(&ahead).unique_commits, 1);
    assert_eq!(find(&ahead).summary(), "1 commit only on ghost/ahead");
    assert_eq!(find(&detached).branch, None);
    assert_eq!(find(&detached).summary(), "1 commit on no branch");
    assert!(!find(&clean).unsaved());
    assert_eq!(plan.unsaved().count(), 3);

    // The default keeps every unsaved worktree, in place and registered, and unlocked.
    let report = execute(&plan, &WorktreeChoice::KeepUnsaved, &git_tool, &FAST);
    assert!(report.complete(), "{:?}", report.failed);
    assert_eq!(report.kept.len(), 3);
    assert_eq!(
        std::fs::read_to_string(dirty.join("notes.txt")).unwrap(),
        "not committed"
    );
    assert!(ahead.join("feature.txt").exists() && detached.join("lost.txt").exists());
    assert!(!clean.exists());
    assert!(!root.join("device.json").exists() && !root.join("EBWebView").exists());
    assert!(!root.join("profiles/local/aura.db").exists());
    let listed = fixture_git(&repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(listed.matches("worktree ").count(), 4, "{listed}");
    assert!(!listed.contains("locked"), "{listed}");

    // Choosing to delete one of them deletes that one only; its branch and commits stay.
    let again = super::plan(&targets, &git_tool);
    let chosen = WorktreeChoice::Remove(vec![ahead.clone()]);
    let report = execute(&again, &chosen, &git_tool, &FAST);
    assert!(report.complete(), "{:?}", report.failed);
    assert!(!ahead.exists());
    assert!(dirty.exists() && detached.exists());
    assert_eq!(
        fixture_git(&repo, &["log", "-1", "--format=%s", "ghost/ahead"]).trim(),
        "feature"
    );
    let listed = fixture_git(&repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(listed.matches("worktree ").count(), 3, "{listed}");
}

#[test]
fn a_data_folder_from_the_environment_loses_only_what_auracode_made_in_it() {
    let m = machine();
    let folder = m.home.join("chosen");
    data_folder(&folder);
    write(&folder.join("device.json.corrupt-1792840000000"), "{");
    write(&folder.join(".state.json.tmp-1a2b"), "{");
    write(&folder.join("my-notes.txt"), "the person's own");
    write(&folder.join("project/README.md"), "the person's own");
    let targets = Targets::data_folder(folder.clone());
    let git = GitTool::from_path();
    let plan = plan(&targets, &git);
    assert!(plan.items.iter().all(|item| item.kind == ItemKind::Entry));
    let report = execute(&plan, &WorktreeChoice::KeepUnsaved, &git, &FAST);
    assert!(report.complete(), "{:?}", report.failed);
    for gone in [
        "device.json",
        "state.json",
        "profiles",
        "adapters",
        "EBWebView",
    ] {
        assert!(!folder.join(gone).exists(), "{gone}");
    }
    assert!(!folder.join("device.json.corrupt-1792840000000").exists());
    assert!(!folder.join(".state.json.tmp-1a2b").exists());
    assert!(folder.join("my-notes.txt").exists() && folder.join("project/README.md").exists());

    // An emptied data folder of its own goes as well.
    let lone = m.home.join("lone");
    data_folder(&lone);
    let plan = super::plan(&Targets::data_folder(lone.clone()), &git);
    let report = execute(&plan, &WorktreeChoice::KeepUnsaved, &git, &FAST);
    assert!(report.complete());
    assert!(!lone.exists());
}

#[test]
fn the_profiles_a_cleanup_removes_name_their_agent_accounts_for_their_tokens() {
    let m = machine();
    let folder = m.home.join("chosen");
    data_folder(&folder);
    write(
        &folder.join("profiles/local/agent-accounts.json"),
        r#"{"version":1,"agents":{"claude":{"active":"b","accounts":[{"id":"a","name":"Work","added_at":1},{"id":"b","name":"Home","added_at":2}]}}}"#,
    );
    write(
        &folder.join("profiles/second/agent-accounts.json"),
        r#"{"version":1,"agents":{"claude":{"accounts":[{"id":"c","name":"Spare","added_at":3}]}}}"#,
    );
    write(&folder.join("profiles/broken/agent-accounts.json"), "{");
    let plan = plan(&Targets::data_folder(folder.clone()), &GitTool::from_path());
    assert_eq!(agent_account_ids(&plan), ["a", "b", "c"]);
}

#[tokio::test]
async fn custom_root_cleanup_removes_clone_journals_and_preserves_destination_repositories() {
    let m = machine();
    let folder = m.home.join("chosen");
    data_folder(&folder);
    write(&folder.join("my-notes.txt"), "unrelated custom-root file");
    let source = repository(&m.home.join("source"));
    let bin = m.home.join("synthetic-bin");
    let synthetic = bin.join(if cfg!(windows) { "git.exe" } else { "git" });
    write(&synthetic, "synthetic executable, never run");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&synthetic, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let env = std::sync::Arc::new(ghost_process::ResolvedEnv {
        vars: [("PATH".into(), bin.into_os_string())].into(),
        source: ghost_process::EnvSource::Fallback {
            reason: "fixture".into(),
        },
        warnings: Vec::new(),
        resolved_at_ms: 0,
    });
    let clones = crate::cloning::Clones::new(Some(folder.clone()));
    let mut destinations = Vec::new();
    for (parent, name) in [
        (&folder, "cloned-in-root"),
        (&m.home, "cloned-outside-root"),
    ] {
        let review = clones
            .check(
                "fixture",
                ghost_protocol::setup_actions::CloneInput {
                    remote: "https://github.com/fixture/repository".into(),
                    parent: Some(parent.to_string_lossy().into_owned()),
                    name: Some(name.into()),
                },
                env.clone(),
            )
            .await
            .unwrap()
            .review
            .unwrap();
        // This creates the real app-owned lifecycle record without executing the synthetic Git.
        let (prepared, job) = clones.take("fixture", &review.id).unwrap();
        drop(prepared);
        drop(job);
        let destination = parent.join(name);
        fixture_git(
            &m.home,
            &[
                "clone",
                "-q",
                source.to_str().unwrap(),
                destination.to_str().unwrap(),
            ],
        );
        write(&destination.join("untracked-note.txt"), "keep user work");
        let head = fixture_git(&destination, &["rev-parse", "HEAD"]);
        destinations.push((destination, head));
    }
    drop(clones);
    assert_eq!(std::fs::read_dir(folder.join("clones")).unwrap().count(), 2);
    let targets = Targets::data_folder(folder.clone());
    let git = GitTool::from_path();
    let reviewed = plan(&targets, &git);
    let report = execute(&reviewed, &WorktreeChoice::KeepUnsaved, &git, &FAST);
    assert!(report.complete(), "{report:?}");
    assert!(
        !folder.join("clones").exists(),
        "app-owned clone recovery journals must be removed"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("my-notes.txt")).unwrap(),
        "unrelated custom-root file"
    );
    for (destination, head) in &destinations {
        assert_eq!(fixture_git(destination, &["rev-parse", "HEAD"]), *head);
        assert_eq!(
            std::fs::read_to_string(destination.join("README.md")).unwrap(),
            "project\n"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("untracked-note.txt")).unwrap(),
            "keep user work"
        );
        assert!(destination.join(".git").is_dir());
    }
    let again = plan(&targets, &git);
    assert!(again.is_empty());
    let second = execute(&again, &WorktreeChoice::KeepUnsaved, &git, &FAST);
    assert!(second.complete() && second.removed.is_empty());
    for (destination, head) in &destinations {
        assert_eq!(fixture_git(destination, &["rev-parse", "HEAD"]), *head);
        assert!(destination.join("untracked-note.txt").exists());
    }
}

#[test]
fn the_macos_layout_covers_webkit_caches_and_preferences() {
    let m = machine();
    let targets = Targets::machine(Os::Macos, &m.dirs, None);
    let library = m.home.join("Library");
    let id = crate::paths::APP_IDENTIFIER;
    for dir in [
        library.join("WebKit").join(id),
        library.join("HTTPStorages").join(id),
        library
            .join("Saved Application State")
            .join(format!("{id}.savedState")),
        m.dirs.cache.clone().unwrap().join(id),
    ] {
        assert!(targets.owned_dirs.contains(&dir), "{dir:?}");
    }
    assert!(
        targets
            .owned_files
            .contains(&library.join("Preferences").join(format!("{id}.plist")))
    );
    // A data folder from the environment is cleaned too, by its entries.
    let env = m.home.join("custom");
    let with_env = Targets::machine(Os::Macos, &m.dirs, Some(env.clone()));
    assert_eq!(with_env.shared_roots, std::slice::from_ref(&env));
    assert!(with_env.data_roots.contains(&env));
    // The default folder given as the environment's is not counted twice.
    let default = targets.data_roots[0].clone();
    let same = Targets::machine(Os::Macos, &m.dirs, Some(default));
    assert!(same.shared_roots.is_empty());
}

#[test]
fn links_inside_the_data_folder_are_removed_as_links() {
    let m = machine();
    let targets = Targets::machine(Os::Windows, &m.dirs, None);
    let root = targets.data_roots[0].clone();
    data_folder(&root);
    let outside = m.home.join("projects").join("keep-me");
    write(&outside.join("file.txt"), "the person's own");
    // Native separators: cmd reads a forward slash as a switch.
    let link = root
        .join("profiles")
        .join("local")
        .join("attachments")
        .join("linked");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    #[cfg(windows)]
    {
        // A junction needs no privilege, unlike a symbolic link.
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(status.status.success(), "{status:?}");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let git = GitTool::from_path();
    let report = execute(
        &plan(&targets, &git),
        &WorktreeChoice::KeepUnsaved,
        &git,
        &FAST,
    );
    assert!(report.complete(), "{:?}", report.failed);
    assert!(!root.exists());
    assert_eq!(
        std::fs::read_to_string(outside.join("file.txt")).unwrap(),
        "the person's own"
    );
}

#[cfg(windows)]
fn hold(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    // No sharing at all: deleting the file fails while this handle is open.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

#[cfg(windows)]
#[test]
fn a_locked_file_is_retried_and_then_reported() {
    let m = machine();
    let targets = Targets::machine(Os::Windows, &m.dirs, None);
    let root = targets.data_roots[0].clone();
    data_folder(&root);
    let locked = root.join("EBWebView/Default/Cookies");
    let handle = hold(&locked);
    let git = GitTool::missing();
    let report = execute(
        &plan(&targets, &git),
        &WorktreeChoice::KeepUnsaved,
        &git,
        &FAST,
    );
    assert!(!report.complete());
    assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
    assert_eq!(report.failed[0].path, locked);
    assert_eq!(report.failed[0].attempts, FAST.attempts);
    // Everything else went; the folders holding the locked file stay for the report.
    assert!(locked.exists() && !root.join("device.json").exists());
    assert!(!root.join("profiles").exists());
    assert!(!report.removed.contains(&root));
    drop(handle);
    // Once the file is free, running again finishes the job.
    let report = execute(
        &plan(&targets, &git),
        &WorktreeChoice::KeepUnsaved,
        &git,
        &FAST,
    );
    assert!(report.complete(), "{:?}", report.failed);
    assert!(!root.exists());
}

#[cfg(windows)]
#[test]
fn a_file_freed_while_retrying_is_removed() {
    let m = machine();
    let targets = Targets::machine(Os::Windows, &m.dirs, None);
    let root = targets.data_roots[0].clone();
    data_folder(&root);
    let handle = hold(&root.join("EBWebView/Default/Cookies"));
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(handle);
    });
    let patient = Retry {
        attempts: 6,
        step: Duration::from_millis(60),
    };
    let git = GitTool::missing();
    let report = execute(
        &plan(&targets, &git),
        &WorktreeChoice::KeepUnsaved,
        &git,
        &patient,
    );
    release.join().unwrap();
    assert!(report.complete(), "{:?}", report.failed);
    assert!(!root.exists());
}

#[cfg(unix)]
#[test]
fn a_locked_file_is_retried_and_then_reported() {
    use std::os::unix::fs::PermissionsExt;
    // A folder outside the removal that denies changes stands in for a held file on Unix.
    let m = machine();
    let folder = m.home.join("chosen");
    data_folder(&folder);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap();
    let git = GitTool::missing();
    let report = execute(
        &plan(&Targets::data_folder(folder.clone()), &git),
        &WorktreeChoice::KeepUnsaved,
        &git,
        &FAST,
    );
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!report.complete());
    assert!(
        report
            .failed
            .iter()
            .all(|failure| failure.attempts == FAST.attempts)
    );
}

#[test]
fn without_git_every_worktree_counts_as_unsaved() {
    let m = machine();
    let targets = Targets::machine(Os::Windows, &m.dirs, None);
    let root = targets.data_roots[0].clone();
    data_folder(&root);
    let repo = repository(&m.home.join("code"));
    let worktree = auracode_worktree(&repo, &root, "nogit0000001", "ghost/nogit");
    let plan = plan(&targets, &GitTool::missing());
    assert_eq!(plan.unsaved().count(), 1);
    assert!(plan.worktrees[0].summary().contains("Git is not installed"));
    let report = execute(
        &plan,
        &WorktreeChoice::KeepUnsaved,
        &GitTool::missing(),
        &FAST,
    );
    assert!(report.complete());
    assert!(worktree.join("README.md").exists());
}

#[test]
fn a_running_profile_is_detected_without_creating_locks() {
    let m = machine();
    let root = m.home.join("data");
    data_folder(&root);
    write(&root.join("profiles/other/profile.json"), "{}");
    assert!(in_use(std::slice::from_ref(&root)).is_empty());
    assert!(!root.join("profiles/other/owner.lock").exists());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("profiles/local/owner.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    // A second handle stands in for another process: the lock conflicts across handles on
    // Windows and Unix alike.
    let busy = in_use(std::slice::from_ref(&root));
    assert_eq!(busy, [root.join("profiles").join("local")]);
    drop(lock);
    assert!(in_use(std::slice::from_ref(&root)).is_empty());
}
