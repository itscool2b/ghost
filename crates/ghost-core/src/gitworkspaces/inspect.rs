use std::path::{Path, PathBuf};

use ghost_protocol::git::{GitFileState, GitWorkspace};
use sha2::{Digest, Sha256};

use super::{
    GitError, Result,
    git::{Git, args, oid, text},
};

/// The blocker a repository without a commit shows; a first commit is still allowed.
pub(super) const NO_COMMIT: &str =
    "This repository has no commit yet. Create its first commit before isolation.";

#[derive(Debug, Clone)]
pub(super) struct Inspection {
    pub(super) view: GitWorkspace,
    pub(super) root: PathBuf,
    pub(super) common: PathBuf,
    pub(super) fingerprint: String,
}

pub(super) async fn inspect(git: &Git, folder: &Path) -> Result<Inspection> {
    let folder = folder.canonicalize().map_err(|_| {
        GitError::Message(
            "The working folder is unavailable. Locate it before using Git actions.".into(),
        )
    })?;
    let output = git
        .output(&folder, args(&["rev-parse", "--show-toplevel"]), 8192)
        .await?;
    if output.exit.as_ref().and_then(|exit| exit.code) != Some(0) {
        return Err(GitError::Message("This folder is not a Git working tree. Initialize it explicitly before creating isolation.".into()));
    }
    let root = PathBuf::from(text(&output.stdout)?).canonicalize()?;
    if root != folder {
        return Err(GitError::Message(
            "Choose the repository root as the primary folder before managing worktrees.".into(),
        ));
    }
    let common = absolute(
        &root,
        text(
            &git.run(&root, args(&["rev-parse", "--git-common-dir"]), 8192)
                .await?,
        )?,
    )
    .canonicalize()?;
    let gitdir = absolute(
        &root,
        text(
            &git.run(&root, args(&["rev-parse", "--git-dir"]), 8192)
                .await?,
        )?,
    )
    .canonicalize()?;
    let status = git
        .run(
            &root,
            args(&[
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ]),
            1024 * 1024,
        )
        .await?;
    let (branch, head, files, mut has_submodules) = parse_status(&status)?;
    has_submodules |= root.join(".gitmodules").try_exists()?;
    let mut blockers = Vec::new();
    if branch.is_none() {
        blockers.push("HEAD is detached. Switch to a branch before using Git actions.".into());
    }
    if head.is_none() {
        blockers.push(NO_COMMIT.into());
    }
    for (name, reason) in [
        ("MERGE_HEAD", "A merge is in progress."),
        ("CHERRY_PICK_HEAD", "A cherry-pick is in progress."),
        ("REVERT_HEAD", "A revert is in progress."),
        (
            "rebase-apply",
            "A rebase or patch application is in progress.",
        ),
        ("rebase-merge", "A rebase is in progress."),
        ("BISECT_LOG", "A bisect is in progress."),
    ] {
        if gitdir.join(name).try_exists()? {
            blockers.push(reason.into());
        }
    }
    if gitdir.join("index.lock").try_exists()? {
        blockers.push("Another Git operation holds the index lock. Wait for it to finish.".into());
    }
    if gitdir.join("HEAD.lock").try_exists()? {
        blockers.push("Another Git operation holds the branch lock. Wait for it to finish.".into());
    }
    if files
        .iter()
        .any(|file| file.index == "U" || file.worktree == "U")
        || status
            .split(|byte| *byte == 0)
            .any(|record| record.starts_with(b"u "))
    {
        blockers.push("Resolve the unmerged paths before using Git actions.".into());
    }
    let index = gitdir.join("index");
    let index_hash = if index.try_exists()? {
        let meta = std::fs::symlink_metadata(&index)?;
        if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > 64 * 1024 * 1024 {
            return Err(GitError::Message(
                "The Git index has an unsupported layout or size.".into(),
            ));
        }
        format!("{:x}", Sha256::digest(std::fs::read(index)?))
    } else {
        "none".into()
    };
    let mut hash = Sha256::new();
    hash.update(&status);
    hash.update(index_hash);
    hash.update(common.to_string_lossy().as_bytes());
    let view = GitWorkspace {
        root: root.to_string_lossy().into_owned(),
        branch,
        head,
        files,
        blockers,
        has_submodules,
        is_linked_worktree: common != gitdir,
    };
    Ok(Inspection {
        view,
        root,
        common,
        fingerprint: format!("{:x}", hash.finalize()),
    })
}

/// The canonical Git common directory of the repository `folder` is in: one name for a repository
/// and every worktree of it. `None` when Git cannot say (no repository, a missing folder).
pub(super) async fn common_dir(git: &Git, folder: &Path) -> Option<PathBuf> {
    let folder = folder.canonicalize().ok()?;
    let output = git
        .output(&folder, args(&["rev-parse", "--git-common-dir"]), 8192)
        .await
        .ok()?;
    if output.exit.as_ref().and_then(|exit| exit.code) != Some(0) {
        return None;
    }
    absolute(&folder, text(&output.stdout).ok()?)
        .canonicalize()
        .ok()
}

fn absolute(root: &Path, value: String) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

type ParsedStatus = (Option<String>, Option<String>, Vec<GitFileState>, bool);

fn parse_status(bytes: &[u8]) -> Result<ParsedStatus> {
    let mut fields = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut branch = None;
    let mut head = None;
    let mut files = Vec::new();
    let mut submodules = false;
    while let Some(field) = fields.next() {
        let value = std::str::from_utf8(field).map_err(|_| {
            GitError::Message(
                "A Git path is not valid UTF-8. Manage this repository with Git directly.".into(),
            )
        })?;
        if let Some(value) = value.strip_prefix("# branch.head ") {
            if value != "(detached)" {
                branch = Some(value.into());
            }
            continue;
        }
        if let Some(value) = value.strip_prefix("# branch.oid ") {
            if value != "(initial)" {
                if !oid(value) {
                    return Err(malformed());
                }
                head = Some(value.into());
            }
            continue;
        }
        if value.starts_with('#') {
            continue;
        }
        let (xy, path, sub, original) = match value.as_bytes().first() {
            Some(b'1') => {
                let parts = value.splitn(9, ' ').collect::<Vec<_>>();
                if parts.len() != 9 {
                    return Err(malformed());
                }
                (parts[1], parts[8], parts[2], None)
            }
            Some(b'2') => {
                let parts = value.splitn(10, ' ').collect::<Vec<_>>();
                if parts.len() != 10 {
                    return Err(malformed());
                }
                let source = fields.next().ok_or_else(malformed)?;
                let source = std::str::from_utf8(source)
                    .map_err(|_| malformed())?
                    .to_owned();
                (parts[1], parts[9], parts[2], Some(source))
            }
            Some(b'u') => {
                let parts = value.splitn(11, ' ').collect::<Vec<_>>();
                if parts.len() != 11 {
                    return Err(malformed());
                }
                (parts[1], parts[10], parts[2], None)
            }
            Some(b'?') => (
                "??",
                value.strip_prefix("? ").ok_or_else(malformed)?,
                "N...",
                None,
            ),
            Some(b'!') => continue,
            _ => return Err(malformed()),
        };
        if xy.len() != 2 || !xy.is_ascii() || path.is_empty() || path.chars().any(char::is_control)
        {
            return Err(malformed());
        }
        submodules |= sub.starts_with('S');
        files.push(GitFileState {
            path: path.into(),
            index: xy[..1].into(),
            worktree: xy[1..].into(),
            original_path: original,
        });
        if files.len() > 20_000 {
            return Err(GitError::Message(
                "This repository exceeds the 20,000 changed-path limit.".into(),
            ));
        }
    }
    Ok((branch, head, files, submodules))
}

fn malformed() -> GitError {
    GitError::Message("Git returned an unsupported status record. No files were changed.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn porcelain_paths_are_not_split_on_spaces_or_confused_with_rename_sources() {
        let bytes=b"# branch.oid 1111111111111111111111111111111111111111\0# branch.head main\x001 M. N... 100644 100644 100644 a b spaced file.txt\x002 R. N... 100644 100644 100644 a b R100 new name\0old name\0? untracked\0";
        let (branch, head, files, _) = parse_status(bytes).expect("status");
        assert_eq!(branch.as_deref(), Some("main"));
        assert!(head.is_some());
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "spaced file.txt");
        assert_eq!(files[1].original_path.as_deref(), Some("old name"));
    }
    #[test]
    fn malformed_paths_and_records_are_rejected() {
        assert!(parse_status(b"2 bad\0").is_err());
        assert!(parse_status(b"? line\nfile\0").is_err());
        assert!(parse_status(b"? \xff\0").is_err());
    }
}
