//! One reviewed change, applied as a unit. Every file is checked against the bytes the person
//! reviewed, the replaced content is backed up outside the project first, and new bytes are
//! published with no-overwrite hard links. A failure part way puts back every file already changed.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, safe_fs};

pub(crate) mod recovery;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MetadataIntent {
    pub identity: String,
    pub before: Option<super::manifest::Entry>,
    pub after: Option<super::manifest::Entry>,
}

/// What a file must be right now for the reviewed change to still apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Expect {
    Missing,
    File(Vec<u8>),
    /// A symbolic link with exactly this target text. Replacing a link never touches its target.
    Link(PathBuf),
}

#[derive(Debug, Clone)]
pub(crate) struct Mutation {
    pub root: usize,
    /// Validated relative path inside its root.
    pub path: String,
    pub expect: Expect,
    /// `None` removes the file (its bytes stay in the backup).
    pub after: Option<Vec<u8>>,
    /// A case-only rename: the on-disk spelling that becomes `path`.
    pub rename_from: Option<String>,
}

/// Files that must still hold the reviewed bytes although nothing writes them (a copy's source).
#[derive(Debug, Clone)]
pub(crate) struct Dependency {
    pub root: usize,
    pub path: String,
    pub expect: Expect,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename = "InstructionsApplyResult")]
pub struct ApplyResult {
    pub id: String,
    pub changed: Vec<String>,
    /// Every changed file was put back after a failure.
    pub rolled_back: bool,
    /// Some file could not be put back; its backup is kept in the named folder.
    pub recovery_required: bool,
    pub message: Option<String>,
    pub backup: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    version: u32,
    id: String,
    roots: Vec<String>,
    #[serde(default)]
    canonical_roots: Vec<Option<String>>,
    files: Vec<JournalFile>,
    #[serde(default)]
    metadata: Option<MetadataIntent>,
}

#[derive(Serialize, Deserialize)]
struct JournalFile {
    root: usize,
    path: String,
    before: Option<String>,
    link: Option<String>,
    after: Option<String>,
    rename_from: Option<String>,
}

pub(crate) struct Transaction<'a> {
    pub id: String,
    pub roots: &'a [PathBuf],
    pub mutations: Vec<Mutation>,
    pub dependencies: Vec<Dependency>,
    /// The app-owned folder for journals and replaced content. Never inside an agent folder.
    pub backups: &'a Path,
    pub metadata: Option<MetadataIntent>,
    /// Test hook: fail before publishing the mutation at this index.
    #[cfg(test)]
    pub fail_at: Option<usize>,
}

const KEEP_BACKUPS: usize = 40;

fn current(root: &Dir, path: &str) -> Result<Expect> {
    match root.symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Expect::Missing),
        Err(_) => Err(Error::Io),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Ok(Expect::Link(root.read_link_contents(path)?))
        }
        Ok(_) => Ok(Expect::File(
            safe_fs::read(root, path)?.ok_or(Error::Stale)?,
        )),
    }
}

fn open_root(path: &Path) -> Result<Option<Dir>> {
    match Dir::open_ambient_dir(path, cap_std::ambient_authority()) {
        Ok(dir) => Ok(Some(dir)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Error::Unavailable),
    }
}

/// The parent folder of a relative path, created on demand when `create` is set.
fn parent(root: &Dir, path: &str, create: bool) -> Result<(Dir, String)> {
    safe_fs::validate(path)?;
    let mut directory = root.try_clone()?;
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return Ok((directory, part.to_owned()));
        }
        if create {
            match directory.create_dir(part) {
                Ok(()) | Err(_) => {}
            }
        }
        directory = directory
            .open_dir_nofollow(part)
            .map_err(|_| Error::UnsafePath)?;
    }
    Err(Error::UnsafePath)
}

fn temp_name(name: &str, id: &str, kind: &str) -> String {
    format!(
        ".{name}.auracode-{}.{kind}",
        &id[id.len().saturating_sub(12)..]
    )
}

fn write_new(dir: &Dir, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = dir.open_with(name, OpenOptions::new().write(true).create_new(true))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_backup(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(name))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn explain(error: &std::io::Error, path: &str) -> String {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => format!(
            "{path} could not be changed. It may be read-only, locked by another program, or in a folder you cannot write to."
        ),
        std::io::ErrorKind::AlreadyExists => {
            format!("{path} appeared after the review. Nothing was overwritten.")
        }
        _ => format!("{path} could not be changed. Check the folder's permissions."),
    }
}

impl Transaction<'_> {
    pub(crate) fn run(self) -> Result<ApplyResult> {
        let mut dirs = Vec::with_capacity(self.roots.len());
        for root in self.roots {
            dirs.push(open_root(root)?);
        }
        // Everything the review saw must be unchanged before the first write.
        for (root, path, expect) in self
            .mutations
            .iter()
            .map(|item| {
                (
                    item.root,
                    item.rename_from.as_deref().unwrap_or(&item.path),
                    &item.expect,
                )
            })
            .chain(
                self.dependencies
                    .iter()
                    .map(|item| (item.root, item.path.as_str(), &item.expect)),
            )
        {
            let found = match dirs.get(root).ok_or(Error::Unavailable)? {
                Some(dir) => current(dir, path)?,
                None => Expect::Missing,
            };
            if &found != expect {
                return Err(Error::Stale);
            }
        }
        if self.mutations.is_empty() && self.metadata.is_none() {
            return Ok(ApplyResult {
                id: self.id,
                changed: Vec::new(),
                rolled_back: false,
                recovery_required: false,
                message: None,
                backup: None,
            });
        }
        // Folders for a new user-level file (an agent that never ran) are created only now.
        for mutation in &self.mutations {
            if dirs[mutation.root].is_none() {
                std::fs::create_dir_all(&self.roots[mutation.root])?;
                dirs[mutation.root] = open_root(&self.roots[mutation.root])?;
            }
        }
        // Folders nothing writes to may stay missing (an agent that is not installed).
        let dir = |root: usize| dirs[root].as_ref().ok_or(Error::Unavailable);
        let backup = self.backups.join(&self.id);
        std::fs::create_dir_all(&backup)?;
        let journal = Journal {
            version: 2,
            id: self.id.clone(),
            roots: self
                .roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
            canonical_roots: self
                .roots
                .iter()
                .map(|root| {
                    std::fs::canonicalize(root)
                        .ok()
                        .map(|path| path.to_string_lossy().into_owned())
                })
                .collect(),
            files: self
                .mutations
                .iter()
                .map(|item| JournalFile {
                    root: item.root,
                    path: item.path.clone(),
                    before: match &item.expect {
                        Expect::File(bytes) => Some(safe_fs::hash(bytes)),
                        _ => None,
                    },
                    link: match &item.expect {
                        Expect::Link(target) => Some(target.to_string_lossy().into_owned()),
                        _ => None,
                    },
                    after: item.after.as_deref().map(safe_fs::hash),
                    rename_from: item.rename_from.clone(),
                })
                .collect(),
            metadata: self.metadata,
        };
        write_backup(
            &backup,
            "journal.json",
            &serde_json::to_vec_pretty(&journal).map_err(|_| Error::Format)?,
        )?;
        for (index, mutation) in self.mutations.iter().enumerate() {
            if let Expect::File(bytes) = &mutation.expect {
                write_backup(&backup, &format!("before-{index}"), bytes)?;
            }
        }
        recovery::mark(&backup, "backups-ready")?;
        prune(self.backups, &self.id);
        let mut failure = None;
        // The index is only used by the deterministic failure hook in test builds.
        #[cfg_attr(not(test), allow(clippy::unused_enumerate_index))]
        for (_index, mutation) in self.mutations.iter().enumerate() {
            #[cfg(test)]
            if self.fail_at == Some(_index) {
                failure = Some(format!(
                    "{} could not be changed. Check the folder's permissions.",
                    mutation.path
                ));
                break;
            }
            match dir(mutation.root)
                .map_err(Failure::from)
                .and_then(|root| publish(root, mutation, &self.id))
            {
                Ok(()) => {}
                Err(Failure::Stale) => {
                    failure = Some(format!(
                        "{} changed while the review was applied. Nothing was overwritten.",
                        mutation.path
                    ));
                    break;
                }
                Err(Failure::Io(error)) => {
                    failure = Some(explain(&error, &mutation.path));
                    break;
                }
            }
        }
        let backup_label = backup.to_string_lossy().into_owned();
        if let Some(message) = failure {
            let restored = recovery::resolve(&backup, &journal).is_ok();
            if restored {
                prune(self.backups, &self.id);
            }
            return Ok(ApplyResult {
                id: self.id,
                changed: Vec::new(),
                rolled_back: restored,
                recovery_required: !restored,
                message: Some(if restored {
                    format!("{message} Every file was put back as it was.")
                } else {
                    format!(
                        "{message} Some files could not be put back. Their earlier content is kept in the backup folder."
                    )
                }),
                backup: Some(backup_label),
            });
        }
        // This durable boundary precedes cleanup and the managed-copy record.
        let finished = recovery::mark(&backup, "files-committed")
            .and_then(|()| recovery::resolve(&backup, &journal));
        if finished.is_ok() {
            prune(self.backups, &self.id);
        }
        Ok(ApplyResult {
            id: self.id,
            changed: self
                .mutations
                .iter()
                .map(|mutation| mutation.path.clone())
                .collect(),
            rolled_back: false,
            recovery_required: finished.is_err(),
            message: finished.err().map(|_| "The files were saved, but recovery still needs attention. The journal and earlier content were kept.".into()),
            backup: Some(backup_label),
        })
    }
}

enum Failure {
    Stale,
    Io(std::io::Error),
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        match error {
            Error::Stale => Self::Stale,
            _ => Self::Io(std::io::Error::other("instruction file")),
        }
    }
}

fn publish(root: &Dir, mutation: &Mutation, id: &str) -> std::result::Result<(), Failure> {
    let (parent, name) = parent(root, &mutation.path, mutation.after.is_some())?;
    if let Some(from) = &mutation.rename_from {
        // A case-only rename keeps the same file; nothing else may have taken its place.
        let (from_parent, from_name) = self::parent(root, from, false)?;
        if safe_fs::children(&parent, "")?
            .iter()
            .any(|entry| entry == &name)
            || current(&from_parent, &from_name)? != mutation.expect
        {
            return Err(Failure::Stale);
        }
        from_parent.rename(&from_name, &parent, &name)?;
        if current(&parent, &name).ok().as_ref() != Some(&mutation.expect) {
            let _ = parent.rename(&name, &from_parent, &from_name);
            return Err(Failure::Stale);
        }
        return Ok(());
    }
    let temp = match &mutation.after {
        Some(bytes) => {
            let temp = temp_name(&name, id, "tmp");
            write_new(&parent, &temp, bytes)?;
            Some(temp)
        }
        None => None,
    };
    let old = match &mutation.expect {
        Expect::Missing => None,
        expect => {
            let old = temp_name(&name, id, "old");
            if current(&parent, &old)? != Expect::Missing {
                if let Some(temp) = &temp {
                    let _ = parent.remove_file(temp);
                }
                return Err(Failure::Stale);
            }
            if let Err(error) = parent.rename(&name, &parent, &old) {
                if let Some(temp) = &temp {
                    let _ = parent.remove_file(temp);
                }
                return Err(error.into());
            }
            // An editor may have replaced the file between the check and the move.
            if current(&parent, &old).ok().as_ref() != Some(expect) {
                let _ = parent.rename(&old, &parent, &name);
                if let Some(temp) = &temp {
                    let _ = parent.remove_file(temp);
                }
                return Err(Failure::Stale);
            }
            Some(old)
        }
    };
    if let Some(temp) = &temp
        && let Err(error) = link_into_place(&parent, temp, &name)
    {
        if let Some(old) = &old {
            let _ = restore(&parent, old, &name, &mutation.expect);
        }
        let _ = parent.remove_file(temp);
        return Err(error.into());
    }
    Ok(())
}

/// Publishes without overwriting: a file created at `name` meanwhile makes this fail.
fn link_into_place(parent: &Dir, temp: &str, name: &str) -> std::io::Result<()> {
    match parent.hard_link(temp, parent, name) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(error),
        Err(_) => {
            // File systems without hard links (FAT, some network shares): check, then rename.
            match parent.symlink_metadata(name) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    parent.rename(temp, parent, name)
                }
                _ => Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists)),
            }
        }
    }
}

fn restore(parent: &Dir, old: &str, name: &str, expect: &Expect) -> std::io::Result<()> {
    match parent.symlink_metadata(name) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
        Ok(_) => return Err(std::io::ErrorKind::AlreadyExists.into()),
    }
    match expect {
        // A link is moved back as itself; a file is linked back so nothing new is overwritten.
        Expect::Link(_) => parent.rename(old, parent, name),
        _ => link_into_place(parent, old, name),
    }
}

/// Keeps the newest backups. Ids are time-ordered UUIDs, so names sort by age.
fn prune(backups: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(backups) else {
        return;
    };
    let mut ids = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| recovery::prunable(&entry.path()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| uuid::Uuid::parse_str(name).is_ok())
        .collect::<Vec<_>>();
    ids.sort();
    let excess = ids.len().saturating_sub(KEEP_BACKUPS);
    for id in ids.into_iter().take(excess) {
        if id != keep {
            let _ = std::fs::remove_dir_all(backups.join(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Dir) {
        let root = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap();
        (root, backups, dir)
    }

    fn run(
        root: &Path,
        backups: &Path,
        mutations: Vec<Mutation>,
        fail_at: Option<usize>,
    ) -> Result<ApplyResult> {
        Transaction {
            id: uuid::Uuid::now_v7().to_string(),
            roots: &[root.to_path_buf()],
            mutations,
            dependencies: Vec::new(),
            backups,
            metadata: None,
            fail_at,
        }
        .run()
    }

    fn write(path: &str, expect: Expect, after: &[u8]) -> Mutation {
        Mutation {
            root: 0,
            path: path.into(),
            expect,
            after: Some(after.to_vec()),
            rename_from: None,
        }
    }

    #[test]
    fn a_reviewed_change_replaces_creates_and_backs_up_atomically() {
        let (root, backups, dir) = fixture();
        dir.write("AGENTS.md", b"old agents").unwrap();
        let result = run(
            root.path(),
            backups.path(),
            vec![
                write("AGENTS.md", Expect::File(b"old agents".to_vec()), b"new"),
                write("CLAUDE.md", Expect::Missing, b"@AGENTS.md\n"),
            ],
            None,
        )
        .unwrap();
        assert!(!result.rolled_back && !result.recovery_required);
        assert_eq!(dir.read("AGENTS.md").unwrap(), b"new");
        assert_eq!(dir.read("CLAUDE.md").unwrap(), b"@AGENTS.md\n");
        let backup = PathBuf::from(result.backup.unwrap());
        assert_eq!(
            std::fs::read(backup.join("before-0")).unwrap(),
            b"old agents"
        );
        assert!(backup.join("complete").exists());
        // No transaction leftovers in the project folder.
        let names = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn an_external_edit_after_review_refuses_before_any_write() {
        let (root, backups, dir) = fixture();
        dir.write("AGENTS.md", b"edited elsewhere").unwrap();
        let error = run(
            root.path(),
            backups.path(),
            vec![
                write("CLAUDE.md", Expect::Missing, b"@AGENTS.md\n"),
                write("AGENTS.md", Expect::File(b"reviewed".to_vec()), b"new"),
            ],
            None,
        )
        .unwrap_err();
        assert!(matches!(error, Error::Stale));
        assert!(dir.symlink_metadata("CLAUDE.md").is_err());
        assert_eq!(dir.read("AGENTS.md").unwrap(), b"edited elsewhere");
    }

    #[test]
    fn a_failure_part_way_puts_every_changed_file_back() {
        let (root, backups, dir) = fixture();
        dir.write("AGENTS.md", b"one").unwrap();
        dir.write("CLAUDE.md", b"two").unwrap();
        let result = run(
            root.path(),
            backups.path(),
            vec![
                write("AGENTS.md", Expect::File(b"one".to_vec()), b"ONE"),
                write("GEMINI.md", Expect::Missing, b"copy"),
                write("CLAUDE.md", Expect::File(b"two".to_vec()), b"TWO"),
            ],
            Some(2),
        )
        .unwrap();
        assert!(result.rolled_back);
        assert!(result.message.unwrap().contains("put back"));
        assert_eq!(dir.read("AGENTS.md").unwrap(), b"one");
        assert_eq!(dir.read("CLAUDE.md").unwrap(), b"two");
        assert!(dir.symlink_metadata("GEMINI.md").is_err());
        let names = std::fs::read_dir(root.path()).unwrap().count();
        assert_eq!(names, 2);
    }

    #[test]
    fn a_file_created_at_the_destination_meanwhile_is_never_overwritten() {
        let (_root, _backups, dir) = fixture();
        write_new(&dir, ".tmp", b"ours").unwrap();
        dir.write("CLAUDE.md", b"theirs").unwrap();
        assert!(link_into_place(&dir, ".tmp", "CLAUDE.md").is_err());
        assert_eq!(dir.read("CLAUDE.md").unwrap(), b"theirs");
    }

    #[test]
    fn removal_keeps_the_bytes_in_the_backup_and_rollback_restores_them() {
        let (root, backups, dir) = fixture();
        dir.write("AGENTS.override.md", b"override").unwrap();
        dir.write("AGENTS.md", b"base").unwrap();
        let result = run(
            root.path(),
            backups.path(),
            vec![
                Mutation {
                    root: 0,
                    path: "AGENTS.override.md".into(),
                    expect: Expect::File(b"override".to_vec()),
                    after: None,
                    rename_from: None,
                },
                write("AGENTS.md", Expect::File(b"base".to_vec()), b"new"),
            ],
            Some(1),
        )
        .unwrap();
        assert!(result.rolled_back);
        assert_eq!(dir.read("AGENTS.override.md").unwrap(), b"override");
        let ok = run(
            root.path(),
            backups.path(),
            vec![Mutation {
                root: 0,
                path: "AGENTS.override.md".into(),
                expect: Expect::File(b"override".to_vec()),
                after: None,
                rename_from: None,
            }],
            None,
        )
        .unwrap();
        assert!(dir.symlink_metadata("AGENTS.override.md").is_err());
        assert_eq!(
            std::fs::read(PathBuf::from(ok.backup.unwrap()).join("before-0")).unwrap(),
            b"override"
        );
    }

    #[test]
    fn new_files_in_a_missing_subfolder_and_a_missing_root_are_created() {
        let (root, backups, dir) = fixture();
        run(
            root.path(),
            backups.path(),
            vec![write(
                ".claude/CLAUDE.md",
                Expect::Missing,
                b"@../AGENTS.md\n",
            )],
            None,
        )
        .unwrap();
        assert_eq!(dir.read(".claude/CLAUDE.md").unwrap(), b"@../AGENTS.md\n");
        let missing = root.path().join("home/.pi/agent");
        Transaction {
            id: uuid::Uuid::now_v7().to_string(),
            roots: std::slice::from_ref(&missing),
            mutations: vec![write("AGENTS.md", Expect::Missing, b"global")],
            dependencies: Vec::new(),
            backups: backups.path(),
            metadata: None,
            fail_at: None,
        }
        .run()
        .unwrap();
        assert_eq!(std::fs::read(missing.join("AGENTS.md")).unwrap(), b"global");
    }

    #[test]
    fn a_changed_dependency_refuses_the_whole_change() {
        let (root, backups, dir) = fixture();
        dir.write("AGENTS.md", b"source edited").unwrap();
        let error = Transaction {
            id: uuid::Uuid::now_v7().to_string(),
            roots: &[root.path().to_path_buf()],
            mutations: vec![write("GEMINI.md", Expect::Missing, b"source")],
            dependencies: vec![Dependency {
                root: 0,
                path: "AGENTS.md".into(),
                expect: Expect::File(b"source".to_vec()),
            }],
            backups: backups.path(),
            metadata: None,
            fail_at: None,
        }
        .run()
        .unwrap_err();
        assert!(matches!(error, Error::Stale));
        assert!(dir.symlink_metadata("GEMINI.md").is_err());
    }

    #[test]
    fn old_backups_are_pruned_to_a_bounded_count() {
        let backups = tempfile::tempdir().unwrap();
        for _ in 0..(KEEP_BACKUPS + 5) {
            let id = uuid::Uuid::now_v7().to_string();
            let path = backups.path().join(&id);
            std::fs::create_dir(&path).unwrap();
            let journal = Journal {
                version: 2,
                id,
                roots: vec![],
                canonical_roots: vec![],
                files: vec![],
                metadata: None,
            };
            write_backup(
                &path,
                "journal.json",
                &serde_json::to_vec(&journal).unwrap(),
            )
            .unwrap();
            recovery::mark(&path, "rolled-back").unwrap();
            recovery::mark(&path, "settled").unwrap();
        }
        prune(backups.path(), "none");
        assert_eq!(
            std::fs::read_dir(backups.path()).unwrap().count(),
            KEEP_BACKUPS
        );
    }
}
