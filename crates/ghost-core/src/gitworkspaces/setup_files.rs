//! Explicit copies use no-follow handles and create-only publication. Contents stay out of previews.
use super::{
    Result,
    setup::{invalid, validate_path},
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const FILE_LIMIT: usize = 1024 * 1024;
const TOTAL_LIMIT: usize = 16 * FILE_LIMIT;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Fingerprint {
    hash: [u8; 32],
    pub bytes: u32,
}
pub(super) type Files = BTreeMap<String, (Fingerprint, Vec<u8>)>;

fn parent(root: &Dir, path: &str, create: bool) -> Result<(Dir, String)> {
    validate_path(path)?;
    walk(root, path, create)
}

/// Opens `path`'s folder from `root` one level at a time without following links, creating
/// missing levels when `create`. The caller has checked the path.
fn walk(root: &Dir, path: &str, create: bool) -> Result<(Dir, String)> {
    let mut dir = root.try_clone()?;
    let parts = path.split('/').collect::<Vec<_>>();
    for part in &parts[..parts.len() - 1] {
        if create {
            match dir.create_dir(part) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        dir = dir.open_dir_nofollow(part)?;
    }
    Ok((dir, parts[parts.len() - 1].into()))
}

fn normalized(root: &Path, path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    path.canonicalize().unwrap_or_else(|_| {
        let mut result = PathBuf::new();
        for part in path.components() {
            match part {
                Component::ParentDir => {
                    result.pop();
                }
                Component::CurDir => {}
                other => result.push(other),
            }
        }
        result
    })
}

pub(super) fn collect(source: &Path, paths: &[String], protected: &[PathBuf]) -> Result<Files> {
    let source = source.canonicalize()?;
    let protected = protected
        .iter()
        .map(|path| normalized(&source, path))
        .collect::<Vec<_>>();
    let root = Dir::open_ambient_dir(&source, cap_std::ambient_authority())?;
    let mut files = BTreeMap::new();
    let mut total = 0;
    for path in paths {
        validate_path(path)?;
        if protected.iter().any(|protected| {
            source.starts_with(protected) || source.join(path).starts_with(protected)
        }) {
            return Err(invalid(
                "Agent configuration and app profile paths cannot supply setup files.",
            ));
        }
        let (dir, name) = parent(&root, path, false)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let file = dir.open_with(name, &options)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.len() > FILE_LIMIT as u64 {
            return Err(invalid(
                "Setup copies require regular files no larger than 1 MiB.",
            ));
        }
        let mut bytes = Vec::new();
        file.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        total += bytes.len();
        if bytes.len() > FILE_LIMIT || total > TOTAL_LIMIT {
            return Err(invalid("Setup files exceed the 16 MiB total limit."));
        }
        let fingerprint = Fingerprint {
            hash: Sha256::digest(&bytes).into(),
            bytes: bytes.len() as u32,
        };
        files.insert(path.clone(), (fingerprint, bytes));
    }
    Ok(files)
}

pub(super) fn absent(destination: &Path, paths: impl Iterator<Item = String>) -> Result<()> {
    let root = Dir::open_ambient_dir(destination, cap_std::ambient_authority())?;
    for path in paths {
        validate_path(&path)?;
        match parent(&root, &path, false) {
            Ok((dir, name)) => match dir.symlink_metadata(name) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
                Ok(_) => {
                    return Err(invalid(
                        "A setup destination already exists. Existing files are never overwritten; remove it from the recipe or review it separately.",
                    ));
                }
            },
            Err(super::GitError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// On partial failure, return exactly the already published paths. Their bytes stay available;
/// no cleanup guesses which destination files still belong to this invocation.
pub(super) fn publish(destination: &Path, files: &Files) -> (Vec<String>, Option<String>) {
    let mut copied = Vec::new();
    let result = (|| -> Result<()> {
        absent(destination, files.keys().cloned())?;
        let root = Dir::open_ambient_dir(destination, cap_std::ambient_authority())?;
        for (path, (_, bytes)) in files {
            let (dir, name) = parent(&root, path, true)?;
            create_new(&dir, &name, bytes, || copied.push(path.clone()))?;
        }
        Ok(())
    })();
    let warning=result.err().map(|error|format!("Setup stopped: {error}. Already copied files remain in the worktree. No command was started. Review them before preparing setup again."));
    (copied, warning)
}

/// Writes `bytes` as the new file `name` in `dir`: staged under a temporary name, then hard
/// linked. `published` runs as soon as the file exists under its own name.
fn create_new(dir: &Dir, name: &str, bytes: &[u8], published: impl FnOnce()) -> Result<()> {
    let temporary = format!(".auracode-setup-{}", crate::ids::new_id());
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut staged = dir.open_with(&temporary, &options)?;
    staged.write_all(bytes)?;
    staged.sync_all()?;
    drop(staged);
    // hard_link cannot replace an existing file, including a late concurrent writer.
    if let Err(error) = dir.hard_link(&temporary, dir, name) {
        let _ = dir.remove_file(&temporary);
        return Err(error.into());
    }
    published();
    dir.remove_file(temporary)?;
    #[cfg(unix)]
    crate::fsutil::fsync_dir(dir)?;
    Ok(())
}

/// Copies files the app itself names (instruction files), never paths a person typed, so agent
/// folders such as `.claude/` may hold them. Create-only: a destination that already exists is
/// kept, and a missing source, a link or anything but a regular file is skipped. Links are never
/// followed. Returns the copied paths and, for each file that could not be copied, why.
pub(super) fn copy_known(
    source: &Path,
    destination: &Path,
    paths: &[String],
) -> Result<(Vec<String>, Vec<String>)> {
    let from = Dir::open_ambient_dir(source, cap_std::ambient_authority())?;
    let to = Dir::open_ambient_dir(destination, cap_std::ambient_authority())?;
    let mut copied = Vec::new();
    let mut problems = Vec::new();
    for path in paths {
        match copy_one(&from, &to, path) {
            Ok(true) => copied.push(path.clone()),
            Ok(false) => {}
            Err(error) => problems.push(format!("{path} ({error})")),
        }
    }
    Ok((copied, problems))
}

fn copy_one(from: &Dir, to: &Dir, path: &str) -> Result<bool> {
    let missing = |error: &std::io::Error| error.kind() == std::io::ErrorKind::NotFound;
    let (dir, name) = match walk(from, path, false) {
        Ok(found) => found,
        Err(super::GitError::Io(error)) if missing(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    let file = match dir.open_with(&name, &options) {
        Ok(file) => file,
        Err(error) if missing(&error) => return Ok(false),
        // A link is never followed, and skipped like any other non-file. Linux refuses to open
        // one without following it (ELOOP), where macOS opens the link itself.
        Err(_)
            if dir
                .symlink_metadata(&name)
                .is_ok_and(|meta| meta.file_type().is_symlink()) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error.into()),
    };
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > FILE_LIMIT {
        return Err(invalid("larger than 1 MiB"));
    }
    let (dir, name) = walk(to, path, true)?;
    match dir.symlink_metadata(&name) {
        Ok(_) => return Ok(false),
        Err(error) if missing(&error) => {}
        Err(error) => return Err(error.into()),
    }
    create_new(&dir, &name, &bytes, || {})?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_env_copy_is_create_only_and_provider_roots_are_never_read() {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join(".env"), "FAKE_FIXTURE=1").unwrap();
        let paths = vec![".env".into()];
        let files = collect(source.path(), &paths, &[]).unwrap();
        assert!(collect(source.path(), &paths, &[source.path().to_path_buf()]).is_err());
        let result = publish(dest.path(), &files);
        assert_eq!(result.0, paths);
        assert!(result.1.is_none());
        std::fs::write(dest.path().join(".env"), "later edit").unwrap();
        assert!(publish(dest.path(), &files).1.is_some());
        assert_eq!(
            std::fs::read(dest.path().join(".env")).unwrap(),
            b"later edit"
        );
    }

    #[test]
    fn known_copies_reach_agent_folders_and_never_overwrite() {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("CLAUDE.md"), "Personal.").unwrap();
        std::fs::create_dir(source.path().join(".claude")).unwrap();
        std::fs::write(source.path().join(".claude").join("CLAUDE.md"), "Nested.").unwrap();
        std::fs::write(source.path().join("AGENTS.md"), "Source text.").unwrap();
        std::fs::write(dest.path().join("AGENTS.md"), "Already here.").unwrap();
        let paths = ["CLAUDE.md", ".claude/CLAUDE.md", "AGENTS.md", "GEMINI.md"].map(String::from);
        let (copied, problems) = copy_known(source.path(), dest.path(), &paths).unwrap();
        assert_eq!(copied, ["CLAUDE.md", ".claude/CLAUDE.md"]);
        assert!(problems.is_empty());
        assert_eq!(
            std::fs::read(dest.path().join(".claude").join("CLAUDE.md")).unwrap(),
            b"Nested."
        );
        assert_eq!(
            std::fs::read(dest.path().join("AGENTS.md")).unwrap(),
            b"Already here."
        );
        assert!(!dest.path().join("GEMINI.md").exists());
        std::fs::write(source.path().join("GEMINI.md"), vec![b'x'; FILE_LIMIT + 1]).unwrap();
        let (copied, problems) =
            copy_known(source.path(), dest.path(), &["GEMINI.md".into()]).unwrap();
        assert!(copied.is_empty());
        assert_eq!(problems, ["GEMINI.md (larger than 1 MiB)"]);
        assert!(!dest.path().join("GEMINI.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn known_copies_skip_links() {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("AGENTS.md"), "Shared.").unwrap();
        std::os::unix::fs::symlink("AGENTS.md", source.path().join("CLAUDE.md")).unwrap();
        let (copied, problems) =
            copy_known(source.path(), dest.path(), &["CLAUDE.md".into()]).unwrap();
        assert!(copied.is_empty() && problems.is_empty());
        assert!(dest.path().join("CLAUDE.md").symlink_metadata().is_err());
    }
}
