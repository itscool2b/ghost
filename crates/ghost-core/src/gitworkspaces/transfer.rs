//! Reviewed working-file snapshots copied only into a newly created app-owned checkout.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, OpenOptions};
use ghost_protocol::git::GitFileState;

use super::{GitError, Result};
use crate::checkpoints::{
    apply_file,
    scan::{self, Fingerprint},
};

const BYTE_LIMIT: usize = 100 * 1024 * 1024;
const PATH_LIMIT: usize = 5000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Snapshot {
    files: BTreeMap<String, Option<Fingerprint>>,
    protected: Vec<PathBuf>,
    pub(super) warnings: Vec<String>,
}

pub(super) type Contents = BTreeMap<String, Option<(Fingerprint, Vec<u8>)>>;
type Destination = BTreeMap<String, Option<Fingerprint>>;

fn invalid(message: &str) -> GitError {
    GitError::Message(message.into())
}
fn file_error(error: impl std::fmt::Display) -> GitError {
    GitError::Message(error.to_string())
}

pub(super) fn snapshot(
    root: &Path,
    files: &[GitFileState],
    protected: &[PathBuf],
) -> Result<Snapshot> {
    let canonical_root = root.canonicalize()?;
    let root = canonical_root.as_path();
    let protected = protected
        .iter()
        .map(|path| {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                root.join(path)
            };
            path.canonicalize().unwrap_or_else(|_| {
                let mut normalized = PathBuf::new();
                for component in path.components() {
                    if component == Component::ParentDir {
                        normalized.pop();
                    } else if component != Component::CurDir {
                        normalized.push(component);
                    }
                }
                normalized
            })
        })
        .collect::<Vec<_>>();
    if protected.iter().any(|path| root.starts_with(path)) {
        return Err(invalid(
            "Agent configuration and app profile folders cannot supply worktree copies.",
        ));
    }
    let paths = files
        .iter()
        .flat_map(|file| std::iter::once(&file.path).chain(file.original_path.iter()))
        .collect::<BTreeSet<_>>();
    if paths.len() > PATH_LIMIT {
        return Err(invalid(
            "Bringing changes is limited to 5,000 paths. Keep the source unchanged and select a smaller set with Git first.",
        ));
    }
    let mut snapshot = Snapshot {
        files: BTreeMap::new(),
        protected,
        warnings: Vec::new(),
    };
    let mut folded = BTreeSet::new();
    let mut bytes = 0;
    let mut omitted = 0;
    for path in paths {
        if !folded.insert(path.to_lowercase()) {
            return Err(invalid(
                "Changed filenames collide on a case-insensitive filesystem. Resolve the collision before bringing changes.",
            ));
        }
        let read = if snapshot
            .protected
            .iter()
            .any(|protected| root.join(path).starts_with(protected))
        {
            Err(crate::checkpoints::CheckpointError::Message(
                "Protected root".into(),
            ))
        } else {
            scan::read(root, path)
        };
        match read {
            Ok(value) => {
                let fingerprint = value.map(|(fingerprint, _)| fingerprint);
                bytes += fingerprint.as_ref().map_or(0, |value| value.bytes as usize);
                if bytes > BYTE_LIMIT {
                    return Err(invalid(
                        "Bringing changes exceeds the 100 MiB limit. The source folder is unchanged.",
                    ));
                }
                snapshot.files.insert(path.clone(), fingerprint);
            }
            Err(_) => {
                omitted += 1;
                if snapshot.warnings.len() < 25 {
                    snapshot.warnings.push(format!("Not copied: {}. Protected configuration, links, special files and files over 10 MiB require separate explicit setup.", crate::conversation::cut(path, 256)));
                }
            }
        }
    }
    if omitted > 25 {
        snapshot.warnings.push(format!(
            "{} additional unsupported or protected paths are not copied.",
            omitted - 25
        ));
    }
    Ok(snapshot)
}

pub(super) fn contents(root: &Path, snapshot: &Snapshot) -> Result<Contents> {
    let mut contents = BTreeMap::new();
    for (path, expected) in &snapshot.files {
        let value = scan::read(root, path).map_err(file_error)?;
        if value.as_ref().map(|(fingerprint, _)| fingerprint) != expected.as_ref() {
            return Err(invalid(
                "Working-file contents changed after isolation preview. Review a new preview; no source files were changed.",
            ));
        }
        contents.insert(path.clone(), value);
    }
    Ok(contents)
}

/// Destination originals and complete replacements are retained if publication fails. The source
/// is never modified, and hard-link publication refuses to overwrite another writer's new path.
pub(super) fn destination(root: &Path, contents: &Contents) -> Result<Destination> {
    contents
        .keys()
        .map(|path| {
            scan::read(root, path)
                .map(|value| (path.clone(), value.map(|(fingerprint, _)| fingerprint)))
                .map_err(file_error)
        })
        .collect()
}

pub(super) fn apply(root: &Path, contents: &Contents, expected: &Destination) -> Result<()> {
    if &destination(root, contents)? != expected {
        return Err(invalid(
            "The new worktree changed before copying started. Its files were preserved for review.",
        ));
    }
    let directory = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    let backup_name = format!(".auracode-transfer-{}", crate::ids::new_id());
    directory.create_dir(&backup_name)?;
    let backup = directory.open_dir_nofollow(&backup_name)?;
    let mut journal = backup.open_with(
        "recovery.json",
        OpenOptions::new().write(true).create_new(true),
    )?;
    let manifest = serde_json::json!({"version": 1, "files": contents.keys().enumerate().map(|(index,path)|serde_json::json!({"path":path,"original":format!("original-{index}"),"replacement":format!("replacement-{index}")})).collect::<Vec<_>>()});
    journal.write_all(
        serde_json::to_string_pretty(&manifest)
            .map_err(file_error)?
            .as_bytes(),
    )?;
    journal.sync_all()?;
    drop(journal);
    for (index, (path, replacement)) in contents.iter().enumerate() {
        let original = expected.get(path).and_then(Option::as_ref);
        if original.is_none() && replacement.is_none() {
            continue;
        }
        apply_file(&directory, &backup, root, path, index, original, replacement.as_ref()).map_err(|error| invalid(&format!("Bringing changes stopped: {error}. Original and replacement files remain in {backup_name} for review.")))?;
    }
    // Delete only the exact files this operation created, through its owned directory handle.
    for (index, (path, replacement)) in contents.iter().enumerate() {
        if expected.get(path).is_some_and(Option::is_some) {
            backup.remove_file(format!("original-{index}"))?;
        }
        if replacement.is_some() {
            backup.remove_file(format!("replacement-{index}"))?;
        }
    }
    backup.remove_file("recovery.json")?;
    drop(backup);
    directory.remove_dir(&backup_name)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_provider_roots_are_excluded_before_copying_ordinary_filenames() {
        let source = tempfile::tempdir().expect("source");
        let custom = source.path().join("custom-provider-root");
        std::fs::create_dir(&custom).expect("custom config");
        std::fs::write(custom.join("ordinary.data"), b"owned sensitive fixture")
            .expect("fixture only");
        let files = vec![GitFileState {
            path: "custom-provider-root/ordinary.data".into(),
            index: "?".into(),
            worktree: "?".into(),
            original_path: None,
        }];
        let snapshot = snapshot(source.path(), &files, std::slice::from_ref(&custom))
            .expect("bounded exclusion");
        assert!(snapshot.files.is_empty());
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("ordinary.data"))
        );
        assert!(
            contents(source.path(), &snapshot)
                .expect("no protected reads")
                .is_empty()
        );
        assert!(super::snapshot(&custom, &[], std::slice::from_ref(&custom)).is_err());
    }

    #[test]
    fn a_destination_edit_is_preserved_before_any_copy() {
        let source = tempfile::tempdir().expect("source");
        let destination_root = tempfile::tempdir().expect("destination");
        std::fs::write(source.path().join("file.txt"), b"reviewed source").expect("source file");
        std::fs::write(destination_root.path().join("file.txt"), b"original")
            .expect("destination file");
        let files = vec![GitFileState {
            path: "file.txt".into(),
            index: ".".into(),
            worktree: "M".into(),
            original_path: None,
        }];
        let snapshot = snapshot(source.path(), &files, &[]).expect("snapshot");
        let contents = contents(source.path(), &snapshot).expect("contents");
        let baseline = destination(destination_root.path(), &contents).expect("baseline");
        std::fs::write(destination_root.path().join("file.txt"), b"newer work")
            .expect("concurrent edit");
        assert!(apply(destination_root.path(), &contents, &baseline).is_err());
        assert_eq!(
            std::fs::read(destination_root.path().join("file.txt")).expect("retained"),
            b"newer work"
        );
        assert_eq!(
            std::fs::read_dir(destination_root.path())
                .expect("files")
                .count(),
            1
        );
    }
}
