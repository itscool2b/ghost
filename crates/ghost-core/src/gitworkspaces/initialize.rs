//! Git generates metadata in an owned temporary directory. Publication creates only a new .git
//! through a pinned directory handle; Git never initializes or reinitializes the user's path.
use super::{GitError, GitWorkspaces, Result, git, inspect};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use ghost_protocol::git::{GitInitializePreview, GitWorkspace};
use same_file::Handle;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MARKER: &str = "auracode-initialization-incomplete";

pub(super) struct Prepared {
    chat_id: String,
    folder: PathBuf,
    identity: Handle,
    branch: String,
    expires: Instant,
}

fn invalid(message: impl Into<String>) -> GitError {
    GitError::Message(message.into())
}

fn branch(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value.starts_with(['-', '.'])
        || value.contains("..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/_-.".contains(&byte))
    {
        return Err(invalid(
            "Choose a short Git branch name using letters, digits, slash, dot, underscore or hyphen.",
        ));
    }
    Ok(())
}

fn folder(path: &Path, protected: &[PathBuf]) -> Result<(PathBuf, Handle)> {
    let path = path
        .canonicalize()
        .map_err(|_| invalid("The folder is unavailable. Locate it before initializing Git."))?;
    if !path.is_dir() || path.parent().is_none() {
        return Err(invalid("Choose a project folder, not a filesystem root."));
    }
    if protected
        .iter()
        .any(|root| root.canonicalize().is_ok_and(|root| path.starts_with(root)))
    {
        return Err(invalid(
            "Agent configuration and application data folders cannot be initialized through this action.",
        ));
    }
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => {
                return Err(invalid(
                    "This folder already belongs to a repository or contains Git metadata. Existing metadata is preserved.",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if ancestor.join("HEAD").is_file()
            && ancestor.join("objects").is_dir()
            && ancestor.join("refs").is_dir()
        {
            return Err(invalid(
                "This folder belongs to a bare repository. Existing metadata is preserved.",
            ));
        }
    }
    let identity = Handle::from_path(&path)?;
    Ok((path, identity))
}

impl GitWorkspaces {
    pub async fn initialize_preview(
        &self,
        chat_id: &str,
        path: &Path,
        initial_branch: &str,
    ) -> Result<GitInitializePreview> {
        if chat_id.is_empty() || chat_id.len() > 128 {
            return Err(invalid("The chat identifier is invalid."));
        }
        branch(initial_branch)?;
        let git = self.git().await?;
        let mut protected = git.protected.clone();
        protected.extend(self.root.parent().map(Path::to_path_buf));
        let (folder, identity) = folder(path, &protected)?;
        git.run(
            &folder,
            git::args(&["check-ref-format", "--branch", initial_branch]),
            8192,
        )
        .await?;
        let existing = git
            .output(&folder, git::args(&["rev-parse", "--git-dir"]), 8192)
            .await?;
        if existing.exit.as_ref().and_then(|exit| exit.code) == Some(0) {
            return Err(invalid("This folder already belongs to a repository."));
        }
        let token = crate::ids::new_id();
        let preview = GitInitializePreview {
            token: token.clone(), folder: git::path_argument(&folder).to_string_lossy().into_owned(),
            initial_branch: initial_branch.into(), expires_at: crate::time::now_millis() + 600_000,
            warnings: vec![
                "Creates only a new .git directory. Existing working files remain unchanged; nothing is staged or committed.".into(),
                "Create the first commit before adding an isolated worktree. No remote, template hooks or setup command runs.".into(),
            ],
        };
        let mut prepared = self
            .initializations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prepared.retain(|_, entry| entry.expires > Instant::now());
        if prepared.len() >= 32 {
            return Err(invalid(
                "Too many Git initialization previews are open. Close one before reviewing again.",
            ));
        }
        prepared.insert(
            token,
            Prepared {
                chat_id: chat_id.into(),
                folder,
                identity,
                branch: initial_branch.into(),
                expires: Instant::now() + Duration::from_secs(600),
            },
        );
        Ok(preview)
    }

    pub async fn initialize_apply(
        &self,
        chat_id: &str,
        path: &Path,
        token: &str,
    ) -> Result<GitWorkspace> {
        let prepared = self
            .initializations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(token)
            .ok_or_else(|| {
                invalid(
                    "This Git initialization preview expired or was already used. Review it again.",
                )
            })?;
        if prepared.chat_id != chat_id || prepared.expires <= Instant::now() {
            return Err(invalid(
                "This Git initialization preview is no longer current.",
            ));
        }
        let git = self.git().await?;
        let _repository = self.hold_repository(&git, path).await;
        let mut protected = git.protected.clone();
        protected.extend(self.root.parent().map(Path::to_path_buf));
        let (current, identity) = folder(path, &protected)?;
        if current != prepared.folder || identity != prepared.identity {
            return Err(invalid(
                "The folder changed since review. Review Git initialization again.",
            ));
        }
        let scratch = tempfile::Builder::new()
            .prefix("git-init-")
            .tempdir_in(&self.root)?;
        let generated = scratch.path().join("repository");
        let mut args = git::args(&["init"]);
        args.push(format!("--initial-branch={}", prepared.branch).into());
        args.push(
            format!(
                "--template={}",
                git::path_argument(&self.root.join("empty-hooks")).display()
            )
            .into(),
        );
        args.push("--".into());
        args.push(git::path_argument(&generated).into_os_string());
        git.run(scratch.path(), args, 8192).await?;
        let entries = snapshot(&generated.join(".git"))?;
        let target = prepared.folder.clone();
        let destination = target.clone();
        let identity = prepared.identity;
        tokio::task::spawn_blocking(move || publish(&destination, identity, entries)).await
            .map_err(|_| invalid("Git initialization stopped unexpectedly. Inspect the new .git directory before retrying."))??;
        let result = inspect::inspect(&git, &target).await?;
        if result.view.branch.as_deref() != Some(prepared.branch.as_str())
            || result.view.head.is_some()
        {
            return Err(invalid(
                "The repository changed during initialization. Its metadata is preserved for review.",
            ));
        }
        Ok(result.view)
    }
}

enum Entry {
    Directory(PathBuf),
    File(PathBuf, Vec<u8>),
}

fn snapshot(root: &Path) -> Result<Vec<Entry>> {
    fn visit(
        root: &Path,
        relative: &Path,
        entries: &mut Vec<Entry>,
        total: &mut usize,
    ) -> Result<()> {
        if relative.components().count() > 8 || entries.len() > 256 {
            return Err(invalid("Generated Git metadata exceeds its bounds."));
        }
        let mut children =
            std::fs::read_dir(root.join(relative))?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            if entries.len() >= 256 {
                return Err(invalid("Generated Git metadata exceeds its bounds."));
            }
            let path = relative.join(child.file_name());
            let metadata = child.metadata()?;
            if child.file_type()?.is_symlink() {
                return Err(invalid("Generated Git metadata contains a link."));
            }
            if metadata.is_dir() {
                entries.push(Entry::Directory(path.clone()));
                visit(root, &path, entries, total)?;
            } else if metadata.is_file() {
                let mut bytes = Vec::new();
                std::fs::File::open(child.path())?
                    .take(1024 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                *total += bytes.len();
                if *total > 1024 * 1024 {
                    return Err(invalid("Generated Git metadata exceeds its bounds."));
                }
                entries.push(Entry::File(path, bytes));
            } else {
                return Err(invalid(
                    "Generated Git metadata has an unsupported file type.",
                ));
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    visit(root, Path::new(""), &mut entries, &mut 0)?;
    Ok(entries)
}

fn publish(path: &Path, identity: Handle, entries: Vec<Entry>) -> Result<()> {
    let root = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
    if Handle::from_file(root.try_clone()?.into_std_file())? != identity {
        return Err(invalid("The reviewed folder was replaced."));
    }
    root.create_dir(".git")
        .map_err(|_| invalid("Git metadata appeared after review. It was not overwritten."))?;
    let repository = root.open_dir_nofollow(".git")?;
    let repository_identity = Handle::from_file(repository.try_clone()?.into_std_file())?;
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut marker = repository.open_with(MARKER, &options)?;
    marker.write_all(b"Auracode initialization is incomplete. Preserve this directory and review it before using Git.\n")?;
    marker.sync_all()?;
    drop(marker);
    for entry in entries {
        let path = match &entry {
            Entry::Directory(path) | Entry::File(path, _) => path,
        };
        let mut parent = repository.try_clone()?;
        if let Some(parts) = path.parent() {
            for part in parts.components() {
                parent = parent.open_dir_nofollow(part.as_os_str())?;
            }
        }
        let leaf = path
            .file_name()
            .ok_or_else(|| invalid("Invalid generated Git metadata path."))?
            .to_os_string();
        match entry {
            Entry::Directory(_) => parent.create_dir(&leaf)?,
            Entry::File(_, bytes) => {
                let mut file = parent.open_with(&leaf, &options)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
            }
        }
    }
    if Handle::from_path(path)? != identity
        || Handle::from_path(path.join(".git"))? != repository_identity
    {
        return Err(invalid(
            "The folder or Git directory moved during initialization. The created metadata was retained for review.",
        ));
    }
    repository.remove_file(MARKER)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{paths::DataRoot, profile::Profiles};
    use ghost_process::{EnvResolver, EnvSource, LeftoverRegistry, ResolvedEnv, process_env};
    use std::sync::Arc;

    struct Rig {
        _root: tempfile::TempDir,
        folder: PathBuf,
        service: Arc<GitWorkspaces>,
    }
    fn rig() -> Rig {
        let root = tempfile::tempdir().unwrap();
        let profile = Profiles::new(DataRoot::at(root.path().join("data")))
            .open_active()
            .unwrap();
        let folder = root.path().join("project with spaces");
        std::fs::create_dir(&folder).unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let mut vars = process_env();
        vars.insert("HOME".into(), home.clone().into_os_string());
        vars.insert("USERPROFILE".into(), home.into_os_string());
        let env = EnvResolver::fixed(ResolvedEnv {
            vars,
            source: EnvSource::Fallback {
                reason: "Git init fixture".into(),
            },
            warnings: vec![],
            resolved_at_ms: 0,
        });
        let service = GitWorkspaces::new(&profile, env, LeftoverRegistry::disabled());
        Rig {
            _root: root,
            folder,
            service,
        }
    }

    #[tokio::test]
    async fn initializes_only_metadata_and_preserves_working_bytes_and_unborn_index() {
        let rig = rig();
        std::fs::write(rig.folder.join("kept.txt"), b"existing working bytes\0\xff").unwrap();
        let review = rig
            .service
            .initialize_preview("chat", &rig.folder, "main")
            .await
            .unwrap();
        assert!(!rig.folder.join(".git").exists());
        let workspace = rig
            .service
            .initialize_apply("chat", &rig.folder, &review.token)
            .await
            .unwrap();
        assert_eq!(workspace.branch.as_deref(), Some("main"));
        assert!(workspace.head.is_none());
        assert!(
            workspace
                .blockers
                .iter()
                .any(|text| text.contains("first commit"))
        );
        assert_eq!(
            std::fs::read(rig.folder.join("kept.txt")).unwrap(),
            b"existing working bytes\0\xff"
        );
        assert!(!rig.folder.join(".git/index").exists());
        assert!(!rig.folder.join(".git").join(MARKER).exists());
        assert!(!rig.folder.join(".git/hooks").exists());
        assert!(
            rig.service
                .initialize_apply("chat", &rig.folder, &review.token)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn existing_or_nested_git_metadata_is_never_reinitialized() {
        let rig = rig();
        let review = rig
            .service
            .initialize_preview("chat", &rig.folder, "main")
            .await
            .unwrap();
        std::fs::create_dir(rig.folder.join(".git")).unwrap();
        std::fs::write(
            rig.folder.join(".git/config"),
            b"preserve even incomplete metadata",
        )
        .unwrap();
        assert!(
            rig.service
                .initialize_apply("chat", &rig.folder, &review.token)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(rig.folder.join(".git/config")).unwrap(),
            b"preserve even incomplete metadata"
        );
        let nested = rig.folder.join("nested");
        std::fs::create_dir(&nested).unwrap();
        assert!(
            rig.service
                .initialize_preview("chat", &nested, "main")
                .await
                .is_err()
        );
        assert!(!nested.join(".git").exists());
    }

    #[tokio::test]
    async fn changed_folder_identity_and_another_chat_cannot_use_the_review() {
        let rig = rig();
        let review = rig
            .service
            .initialize_preview("chat", &rig.folder, "main")
            .await
            .unwrap();
        let original = rig.folder.with_file_name("original");
        std::fs::rename(&rig.folder, &original).unwrap();
        std::fs::create_dir(&rig.folder).unwrap();
        assert!(
            rig.service
                .initialize_apply("chat", &rig.folder, &review.token)
                .await
                .is_err()
        );
        assert!(!rig.folder.join(".git").exists());
        assert!(!original.join(".git").exists());
        let review = rig
            .service
            .initialize_preview("chat", &rig.folder, "main")
            .await
            .unwrap();
        assert!(
            rig.service
                .initialize_apply("another", &rig.folder, &review.token)
                .await
                .is_err()
        );
        assert!(!rig.folder.join(".git").exists());
    }

    #[tokio::test]
    async fn invalid_branch_and_protected_profile_are_rejected_without_writes() {
        let rig = rig();
        for branch in ["", "-b", "../main", "x.lock", "space name", "@{-1}"] {
            assert!(
                rig.service
                    .initialize_preview("chat", &rig.folder, branch)
                    .await
                    .is_err(),
                "{branch}"
            );
            assert!(!rig.folder.join(".git").exists());
        }
        let profile = rig.service.root.parent().unwrap();
        assert!(
            rig.service
                .initialize_preview("chat", profile, "main")
                .await
                .is_err()
        );
        assert!(!profile.join(".git").exists());
    }

    #[test]
    fn create_only_publication_keeps_partial_metadata_marked_for_recovery() {
        let rig = rig();
        let entries = vec![
            Entry::File("config".into(), b"first".to_vec()),
            Entry::File("config".into(), b"overwrite".to_vec()),
        ];
        assert!(
            publish(
                &rig.folder,
                Handle::from_path(&rig.folder).unwrap(),
                entries
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(rig.folder.join(".git/config")).unwrap(),
            b"first"
        );
        assert!(rig.folder.join(".git").join(MARKER).is_file());
    }
}
