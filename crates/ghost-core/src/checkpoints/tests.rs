use super::*;
use crate::paths::DataRoot;
use crate::profile::Profiles;
use ghost_process::{EnvSource, ResolvedEnv, process_env};

#[path = "cache_tests.rs"]
mod cache_tests;

struct Rig {
    _dir: tempfile::TempDir,
    work: PathBuf,
    service: Arc<Checkpoints>,
    profile: crate::profile::Profile,
    chat: String,
    folder: String,
}

fn rig() -> Rig {
    rig_with_nested_data(0)
}

fn rig_with_nested_data(depth: usize) -> Rig {
    let dir = tempfile::tempdir().expect("fixture");
    let mut data = dir.path().join("data");
    for _ in 0..depth {
        data = data.join("profile-with-a-long-parent-component");
    }
    let profile = Profiles::new(DataRoot::at(data))
        .open_active()
        .expect("profile");
    let work = dir.path().join("work");
    std::fs::create_dir(&work).expect("work");
    let env = EnvResolver::fixed(ResolvedEnv {
        vars: process_env(),
        source: EnvSource::Fallback {
            reason: "checkpoint fixture".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    let service = Checkpoints::new(&profile, env, LeftoverRegistry::disabled());
    Rig {
        _dir: dir,
        work,
        service,
        profile,
        chat: crate::ids::new_id(),
        folder: crate::ids::new_id(),
    }
}

#[cfg(windows)]
#[tokio::test]
async fn checkpoint_refs_work_beyond_the_legacy_windows_path_limit() {
    // The deepest profile whose repository Git still accepts. Temporary folder lengths differ
    // between machines, so the depth is measured rather than fixed.
    let rig = (0..8)
        .map(rig_with_nested_data)
        .take_while(|rig| {
            !beyond_git_dir_limit(&rig.service.root.join(format!("{}.git", rig.folder)))
        })
        .last()
        .expect("a profile path within Git's limit");
    std::fs::write(rig.work.join("long-profile.txt"), "before\n").expect("fixture");
    let checkpoint = rig
        .service
        .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
        .await
        .expect("checkpoint in a deeply nested profile");
    let reference = rig
        .service
        .root
        .join(format!("{}.git", rig.folder))
        .join("refs/aura")
        .join(&rig.chat)
        .join(&checkpoint.id);
    assert!(reference.to_string_lossy().encode_utf16().count() > 260);
    assert!(reference.is_file());
    std::fs::write(rig.work.join("long-profile.txt"), "after\n").expect("edit");
    let diff = rig
        .service
        .diff(
            &rig.chat,
            &rig.folder,
            &rig.work,
            &checkpoint.id,
            "long-profile.txt",
        )
        .await
        .expect("read the saved checkpoint");
    assert_eq!(diff.before.as_deref(), Some("before\n"));
    assert_eq!(diff.after.as_deref(), Some("after\n"));
    assert!(!rig.work.join(".git").exists());
}

#[cfg(windows)]
#[tokio::test]
async fn a_repository_beyond_git_dir_limit_fails_clearly_without_touching_the_folder() {
    let rig = (0..12)
        .map(rig_with_nested_data)
        .find(|rig| beyond_git_dir_limit(&rig.service.root.join(format!("{}.git", rig.folder))))
        .expect("a profile path beyond Git's limit");
    std::fs::write(rig.work.join("kept.txt"), "kept\n").expect("fixture");
    let error = rig
        .service
        .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
        .await
        .expect_err("Git cannot open this repository");
    assert_eq!(error.to_string(), GIT_DIR_TOO_LONG);
    assert!(
        !rig.service
            .root
            .join(format!("{}.git", rig.folder))
            .exists()
    );
    assert!(!rig.work.join(".git").exists());
    assert_eq!(
        std::fs::read_to_string(rig.work.join("kept.txt")).expect("kept"),
        "kept\n"
    );
}

#[tokio::test]
async fn non_git_checkpoint_preserves_bytes_ignores_and_reports_changes() {
    let rig = rig();
    std::fs::write(rig.work.join(".gitignore"), "ignored/\n").expect("ignore");
    std::fs::create_dir(rig.work.join("ignored")).expect("ignored directory");
    std::fs::write(rig.work.join("ignored/secret"), "not captured").expect("ignored file");
    std::fs::write(rig.work.join("text.txt"), b"first\r\nsecond\r\n").expect("text");
    std::fs::write(rig.work.join("binary.dat"), [0, 128, 255]).expect("binary");
    let checkpoint = rig
        .service
        .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
        .await
        .expect("checkpoint");
    assert_eq!(checkpoint.file_count, 3);
    assert!(!rig.work.join(".git").exists());
    std::fs::write(rig.work.join("text.txt"), "changed\n").expect("edit");
    std::fs::remove_file(rig.work.join("binary.dat")).expect("delete fixture");
    std::fs::write(rig.work.join("new.txt"), "new").expect("new");
    let changes = rig
        .service
        .changes(&rig.chat, &rig.folder, &rig.work, &checkpoint.id)
        .await
        .expect("changes");
    assert_eq!(changes.files.len(), 3);
    assert_eq!(
        changes
            .files
            .iter()
            .find(|file| file.path == "binary.dat")
            .expect("binary change")
            .kind,
        ChangeKind::Deleted
    );
    let diff = rig
        .service
        .diff(
            &rig.chat,
            &rig.folder,
            &rig.work,
            &checkpoint.id,
            "text.txt",
        )
        .await
        .expect("diff");
    assert_eq!(diff.before.as_deref(), Some("first\r\nsecond\r\n"));
    assert_eq!(diff.after.as_deref(), Some("changed\n"));
    assert!(!diff.binary && !diff.truncated);
    let stat = rig
        .service
        .diff_stat(&rig.chat, &rig.folder, &rig.work, &checkpoint.id)
        .await
        .expect("git stat");
    assert!(
        stat.contains("text.txt") && stat.contains("3 files changed"),
        "{stat}"
    );
}

#[tokio::test]
async fn checkpoint_never_changes_the_user_index_or_applies_attributes() {
    let rig = rig();
    let git = ghost_process::find_exe("git", &process_env()).expect("test Git");
    for args in [vec!["init"], vec!["add", "--", "staged.txt"]] {
        if args[0] == "add" {
            std::fs::write(rig.work.join("staged.txt"), "staged\n").expect("file");
        }
        let output = ghost_process::run_helper(
            &LeftoverRegistry::disabled(),
            ghost_process::HelperSpec {
                program: git.clone(),
                args: args.into_iter().map(Into::into).collect(),
                cwd: rig.work.clone(),
                env: ghost_process::HelperEnv::ProcessEnv,
                set: vec![],
                new_session: true,
                timeout: std::time::Duration::from_secs(10),
                max_stdout: 4096,
            },
        )
        .await
        .expect("fixture Git");
        assert_eq!(output.exit.and_then(|exit| exit.code), Some(0));
    }
    let index = std::fs::read(rig.work.join(".git/index")).expect("index before");
    let head = std::fs::read(rig.work.join(".git/HEAD")).expect("head before");
    std::fs::write(rig.work.join(".gitattributes"), "*.txt text eol=lf\n").expect("attributes");
    std::fs::write(rig.work.join("staged.txt"), b"unstaged\r\n").expect("working change");
    let checkpoint = rig
        .service
        .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
        .await
        .expect("checkpoint");
    assert_eq!(
        std::fs::read(rig.work.join(".git/index")).expect("index after"),
        index
    );
    assert_eq!(
        std::fs::read(rig.work.join(".git/HEAD")).expect("head after"),
        head
    );
    let diff = rig
        .service
        .diff(
            &rig.chat,
            &rig.folder,
            &rig.work,
            &checkpoint.id,
            "staged.txt",
        )
        .await
        .expect("diff");
    assert_eq!(diff.before.as_deref(), Some("unstaged\r\n"));
    assert_eq!(diff.before, diff.after);
}

#[test]
fn paths_reject_traversal_git_metadata_and_windows_streams() {
    for path in [
        "../secrets",
        "a/../b",
        "/absolute",
        "C:/absolute",
        "a\\b",
        "file:stream",
        ".git/config",
        "sub/.GIT/index",
        "line\nname",
        ".ssh/id_rsa",
        "nested/AUTH.JSON",
        ".env.local",
        "signing/private.KEY",
    ] {
        assert!(scan::validate(path).is_err(), "{path}");
    }
    assert!(scan::validate("folder with space/文档.txt").is_ok());
}

#[tokio::test]
async fn newly_ignored_saved_file_does_not_look_deleted() {
    let rig = rig();
    std::fs::write(rig.work.join("file.txt"), "before").expect("file");
    let checkpoint = rig
        .service
        .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
        .await
        .expect("checkpoint");
    std::fs::write(rig.work.join(".gitignore"), "file.txt\n").expect("ignore");
    let changes = rig
        .service
        .changes(&rig.chat, &rig.folder, &rig.work, &checkpoint.id)
        .await
        .expect("changes");
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.files[0].path, ".gitignore");
}

#[tokio::test]
async fn one_hunk_reverts_alone_and_a_later_edit_makes_it_stale() {
    use ghost_protocol::changes::{HunkRevert, LineHunk};
    let rig = rig();
    std::fs::write(rig.work.join("code.txt"), "one\ntwo\nthree\nfour\nfive\n").expect("before");
    let checkpoint = rig
        .service
        .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
        .await
        .expect("checkpoint");
    std::fs::write(
        rig.work.join("code.txt"),
        "one\nTWO\nthree\nfour\nFIVE\nsix\n",
    )
    .expect("edit");
    let diff = rig
        .service
        .diff(
            &rig.chat,
            &rig.folder,
            &rig.work,
            &checkpoint.id,
            "code.txt",
        )
        .await
        .expect("diff");
    let sha = diff.after_sha256.expect("current hash");
    // The second hunk: "five" became "FIVE" and "six" was added.
    let second = LineHunk {
        old_start: 4,
        old_lines: 1,
        new_start: 4,
        new_lines: 2,
    };
    let selection = |sha: &str| HunkRevert {
        path: "code.txt".into(),
        after_sha256: sha.into(),
        hunks: vec![second],
    };
    let preview = rig
        .service
        .preview_hunk_revert(
            &rig.chat,
            &rig.folder,
            &rig.work,
            &checkpoint.id,
            selection(&sha),
        )
        .await
        .expect("preview");
    assert_eq!(preview.files.len(), 1);
    let result = rig
        .service
        .revert(&rig.chat, &rig.folder, &rig.work, &preview.token)
        .await
        .expect("revert");
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(
        std::fs::read_to_string(rig.work.join("code.txt")).expect("reverted"),
        "one\nTWO\nthree\nfour\nfive\n"
    );
    // The original stays in recovery, as for any revert.
    let backup = PathBuf::from(&result.recovery_paths[0]);
    assert_eq!(
        std::fs::read_to_string(backup.join("original-0")).expect("original retained"),
        "one\nTWO\nthree\nfour\nFIVE\nsix\n"
    );
    // Counted against an older text, a hunk is refused.
    let stale = rig
        .service
        .preview_hunk_revert(
            &rig.chat,
            &rig.folder,
            &rig.work,
            &checkpoint.id,
            selection(&sha),
        )
        .await
        .expect_err("stale hash");
    assert!(stale.to_string().contains("Refresh Changes"), "{stale}");
    // An edit after the preview makes its revert stale; nothing newer is overwritten.
    let diff = rig
        .service
        .diff(
            &rig.chat,
            &rig.folder,
            &rig.work,
            &checkpoint.id,
            "code.txt",
        )
        .await
        .expect("diff");
    let first = HunkRevert {
        path: "code.txt".into(),
        after_sha256: diff.after_sha256.expect("current hash"),
        hunks: vec![LineHunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
        }],
    };
    let preview = rig
        .service
        .preview_hunk_revert(&rig.chat, &rig.folder, &rig.work, &checkpoint.id, first)
        .await
        .expect("preview");
    std::fs::write(rig.work.join("code.txt"), "one\nTwo\nthree\nfour\nfive\n").expect("newer edit");
    let error = rig
        .service
        .revert(&rig.chat, &rig.folder, &rig.work, &preview.token)
        .await
        .map(|result| result.error)
        .unwrap_or_else(|error| Some(error.to_string()));
    assert!(error.is_some_and(|error| error.contains("changed after the preview")));
    assert_eq!(
        std::fs::read_to_string(rig.work.join("code.txt")).expect("newer kept"),
        "one\nTwo\nthree\nfour\nfive\n"
    );
}

#[tokio::test]
async fn revert_restores_exact_bytes_and_retains_original_work_for_recovery() {
    let rig = rig();
    std::fs::write(rig.work.join("edit.txt"), b"before\r\n").expect("before");
    std::fs::create_dir(rig.work.join("removed")).expect("directory");
    std::fs::write(rig.work.join("removed/file.txt"), "restore directory").expect("deleted file");
    let checkpoint = rig
        .service
        .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
        .await
        .expect("checkpoint");
    std::fs::write(rig.work.join("edit.txt"), "external edit").expect("edit");
    std::fs::remove_file(rig.work.join("removed/file.txt")).expect("delete owned file");
    std::fs::remove_dir(rig.work.join("removed")).expect("delete empty owned directory");
    std::fs::write(rig.work.join("added.txt"), "new work").expect("added");
    let preview = rig
        .service
        .preview_revert(
            &rig.chat,
            &rig.folder,
            &rig.work,
            &checkpoint.id,
            vec![
                "edit.txt".into(),
                "removed/file.txt".into(),
                "added.txt".into(),
            ],
        )
        .await
        .expect("preview");
    let result = rig
        .service
        .revert(&rig.chat, &rig.folder, &rig.work, &preview.token)
        .await
        .expect("revert");
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.applied.len(), 3);
    assert_eq!(
        std::fs::read(rig.work.join("edit.txt")).expect("restored"),
        b"before\r\n"
    );
    assert_eq!(
        std::fs::read_to_string(rig.work.join("removed/file.txt")).expect("restored deletion"),
        "restore directory"
    );
    assert!(!rig.work.join("added.txt").exists());
    // The originals live in the profile's data folder; nothing of the revert stays in the folder.
    assert_eq!(result.recovery_paths.len(), 1);
    let backup = PathBuf::from(&result.recovery_paths[0]);
    assert!(
        backup.starts_with(rig.profile.checkpoints_dir().join("reverts")),
        "{}",
        backup.display()
    );
    assert_eq!(
        std::fs::read_dir(&rig.work)
            .expect("folder")
            .flatten()
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".auracode-revert-"))
            .count(),
        0
    );
    assert_eq!(
        std::fs::read_to_string(backup.join("original-0")).expect("new work retained"),
        "new work"
    );
    assert_eq!(
        std::fs::read_to_string(backup.join("original-1")).expect("edit retained"),
        "external edit"
    );
    assert!(backup.join("recovery.json").is_file());
    // The replacements were published and the checkpoint keeps their bytes.
    assert!(
        std::fs::read_dir(&backup)
            .expect("backup")
            .flatten()
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with("replacement-"))
    );
    assert!(
        rig.service
            .changes(&rig.chat, &rig.folder, &rig.work, &checkpoint.id)
            .await
            .expect("changes")
            .files
            .is_empty()
    );
}

/// Revert folders an earlier build (or an interrupted revert) left in the working folder move into
/// the profile's data folder before the next prompt, with or without an automatic checkpoint.
/// Other names and links stay.
#[tokio::test]
async fn leftover_revert_folders_move_into_the_data_folder() {
    for automatic in [true, false] {
        let rig = rig();
        std::fs::write(rig.work.join("file.txt"), "work").expect("file");
        let checkpoint = rig
            .service
            .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
            .await
            .expect("checkpoint");
        let token = crate::ids::new_id();
        let leftover = rig.work.join(format!(".auracode-revert-{token}"));
        std::fs::create_dir(&leftover).expect("leftover");
        std::fs::write(leftover.join("original-0"), "only copy of newer work").expect("original");
        std::fs::write(leftover.join("recovery.json"), serde_json::to_vec(&serde_json::json!({"version":1,"checkpoint_id":checkpoint.id,"files":[{"path":"file.txt","backup":"original-0"}]})).unwrap()).expect("journal");
        let invalid = rig
            .work
            .join(format!(".auracode-revert-{}", crate::ids::new_id()));
        std::fs::create_dir(&invalid).unwrap();
        std::fs::write(invalid.join("recovery.json"), "{}").unwrap();
        let mut skipped = Vec::new();
        for (id, path, backup, extra) in [
            (crate::ids::new_id(), "file.txt", "original-0", false),
            (checkpoint.id.clone(), "../escape", "original-0", false),
            (checkpoint.id.clone(), "file.txt", "../escape", false),
            (checkpoint.id.clone(), "file.txt", "original-0", true),
        ] {
            let candidate = rig
                .work
                .join(format!(".auracode-revert-{}", crate::ids::new_id()));
            std::fs::create_dir(&candidate).unwrap();
            std::fs::write(candidate.join("original-0"), "unrelated original").unwrap();
            std::fs::write(candidate.join("recovery.json"), serde_json::to_vec(&serde_json::json!({"version":1,"checkpoint_id":id,"files":[{"path":path,"backup":backup}]})).unwrap()).unwrap();
            if extra {
                std::fs::write(candidate.join("unrelated"), "preserve").unwrap();
            }
            skipped.push(candidate);
        }
        let unrelated = rig.work.join(".auracode-revert-notes");
        std::fs::create_dir(&unrelated).expect("unrelated");
        let link = {
            let elsewhere = rig._dir.path().join("elsewhere");
            std::fs::create_dir(&elsewhere).expect("elsewhere");
            let link = rig
                .work
                .join(format!(".auracode-revert-{}", crate::ids::new_id()));
            #[cfg(unix)]
            std::os::unix::fs::symlink(&elsewhere, &link).expect("link");
            #[cfg(windows)]
            {
                let output = std::process::Command::new("cmd")
                    .args(["/C", "mklink", "/J"])
                    .arg(&link)
                    .arg(&elsewhere)
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{output:?}");
            }
            link
        };
        if automatic {
            rig.service
                .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
                .await
                .expect("checkpoint");
        } else {
            rig.service
                .prepare_workspace(&rig.chat, &rig.folder, &rig.work)
                .await
                .expect("recovery without capture");
        }
        assert!(!leftover.exists());
        let moved = rig.profile.checkpoints_dir().join("reverts").join(&token);
        assert_eq!(
            std::fs::read_to_string(
                std::fs::read_dir(&moved)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("saved-original-0-"))
                    .unwrap()
            )
            .expect("moved intact"),
            "only copy of newer work"
        );
        assert!(moved.join("recovery.json").is_file());
        assert!(skipped.iter().all(|path| path.join("original-0").is_file()));
        assert!(invalid.is_dir(), "an arbitrary UUID directory stays");
        assert!(unrelated.is_dir(), "a name that is not a revert's stays");
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("link")
                .file_type()
                .is_symlink(),
            "a link is never followed or moved"
        );
        assert_eq!(
            std::fs::read_to_string(rig.work.join("file.txt")).expect("file"),
            "work"
        );
    }
}

#[tokio::test]
async fn changed_preview_is_rejected_before_any_file_is_modified() {
    let rig = rig();
    std::fs::write(rig.work.join("a.txt"), "old a").expect("a");
    std::fs::write(rig.work.join("b.txt"), "old b").expect("b");
    let checkpoint = rig
        .service
        .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
        .await
        .expect("checkpoint");
    std::fs::write(rig.work.join("a.txt"), "new a").expect("a");
    std::fs::write(rig.work.join("b.txt"), "new b").expect("b");
    let preview = rig
        .service
        .preview_revert(
            &rig.chat,
            &rig.folder,
            &rig.work,
            &checkpoint.id,
            vec!["a.txt".into(), "b.txt".into()],
        )
        .await
        .expect("preview");
    std::fs::write(rig.work.join("b.txt"), "newest external b").expect("outside edit");
    assert!(
        rig.service
            .revert(&rig.chat, &rig.folder, &rig.work, &preview.token)
            .await
            .expect_err("stale")
            .to_string()
            .contains("changed after")
    );
    assert_eq!(
        std::fs::read_to_string(rig.work.join("a.txt")).expect("a kept"),
        "new a"
    );
    assert_eq!(
        std::fs::read_to_string(rig.work.join("b.txt")).expect("b kept"),
        "newest external b"
    );
    assert!(
        rig.service
            .revert(&rig.chat, &rig.folder, &rig.work, &preview.token)
            .await
            .is_err(),
        "token is one-use"
    );
}

#[tokio::test]
async fn agent_config_and_oversized_files_are_excluded_without_becoming_deletions() {
    let rig = rig();
    std::fs::create_dir(rig.work.join(".ssh")).expect("owned private-directory fixture");
    for path in [
        ".ssh/id_rsa",
        "auth.json",
        ".env",
        ".env.local",
        "private.pem",
    ] {
        std::fs::write(rig.work.join(path), "synthetic fixture only, never read")
            .expect("owned private fixture");
    }
    std::fs::create_dir(rig.work.join(".codex")).expect("owned config fixture");
    std::fs::write(
        rig.work.join(".codex/auth.json"),
        "fixture only, never read",
    )
    .expect("owned config fixture");
    std::fs::write(rig.work.join("grows.bin"), "small").expect("small");
    std::fs::write(rig.work.join(".gitignore"), "private.txt\n").expect("ignore fixture");
    std::fs::write(rig.work.join("private.txt"), "ignored fixture").expect("ignored fixture");
    let checkpoint = rig
        .service
        .capture(&rig.chat, &rig.folder, &rig.work, &crate::ids::new_id())
        .await
        .expect("checkpoint");
    assert_eq!(checkpoint.file_count, 2);
    assert!(
        rig.service
            .diff(
                &rig.chat,
                &rig.folder,
                &rig.work,
                &checkpoint.id,
                ".codex/auth.json"
            )
            .await
            .is_err()
    );
    assert!(
        rig.service
            .diff(
                &rig.chat,
                &rig.folder,
                &rig.work,
                &checkpoint.id,
                "private.txt"
            )
            .await
            .is_err()
    );
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(rig.work.join("grows.bin"))
        .expect("fixture file");
    file.set_len(scan::FILE_LIMIT as u64 + 1)
        .expect("grow owned file");
    drop(file);
    let changes = rig
        .service
        .changes(&rig.chat, &rig.folder, &rig.work, &checkpoint.id)
        .await
        .expect("changes");
    assert!(
        changes.files.is_empty(),
        "an excluded existing file is not a deletion"
    );
    assert!(!changes.checkpoint.expect("summary").warnings.is_empty());
    assert!(
        rig.service
            .preview_revert(
                &rig.chat,
                &rig.folder,
                &rig.work,
                &checkpoint.id,
                vec!["grows.bin".into()]
            )
            .await
            .is_err()
    );
}

/// A worktree chat runs in a folder Ghost created inside the profile's worktrees folder. The
/// profile folder is protected, so every prompt of such a chat was held before reaching the agent.
/// The rest of the profile, and the worktrees folder itself, stay refused.
#[tokio::test]
async fn the_profiles_own_worktrees_are_checkpoint_workspaces() {
    let rig = rig();
    let worktree = rig.profile.worktrees_dir().join("59d4821b8e8c");
    std::fs::create_dir_all(&worktree).expect("worktree fixture");
    std::fs::write(
        worktree.join("math.js"),
        "export const add = (a, b) => a + b;\n",
    )
    .expect("worktree file");
    let checkpoint = rig
        .service
        .capture(&rig.chat, &rig.folder, &worktree, &crate::ids::new_id())
        .await
        .expect("a worktree chat checkpoints");
    assert_eq!(checkpoint.file_count, 1);
    std::fs::write(worktree.join("math.js"), "changed\n").expect("edit");
    let changes = rig
        .service
        .changes(&rig.chat, &rig.folder, &worktree, &checkpoint.id)
        .await
        .expect("changes");
    assert_eq!(changes.files.len(), 1);

    for refused in [
        rig.profile.dir.clone(),
        rig.profile.worktrees_dir(),
        rig.profile.logs_dir(),
    ] {
        std::fs::create_dir_all(&refused).expect("profile folder");
        let error = rig
            .service
            .capture(&rig.chat, &rig.folder, &refused, &crate::ids::new_id())
            .await
            .expect_err("a profile folder is never a checkpoint workspace");
        assert!(
            error
                .to_string()
                .contains("cannot be checkpoint workspaces"),
            "{}: {error}",
            refused.display()
        );
    }
}
