//! Review-only reconciliation and recoverable restore. No reset, prune or recursive removal.

use std::time::Duration;

use ghost_protocol::git::{
    ManagedWorktree, WorktreeActionResult, WorktreeRecovery, WorktreeRecoveryAction,
    WorktreeRecoveryPreview, WorktreeState,
};

use super::{
    git::{args, oid, path_argument, text},
    inspect::{Inspection, inspect},
    *,
};

pub(super) struct PreparedRecovery {
    record: ManagedWorktree,
    action: WorktreeRecoveryAction,
    folder: Inspection,
    source: Inspection,
    destination: PathBuf,
    state: WorktreeState,
    expires: Instant,
}

fn invalid(message: &str) -> GitError {
    GitError::Message(message.into())
}

impl GitWorkspaces {
    pub(super) fn recovery_record(&self, chat_id: &str, id: &str) -> Result<ManagedWorktree> {
        self.read_records()?
            .into_iter()
            .find(|record| record.id == id && record.chat_id == chat_id)
            .ok_or_else(|| invalid("This chat does not own that worktree."))
    }

    pub(super) fn recovery_paths(&self, record: &ManagedWorktree) -> Result<(PathBuf, PathBuf)> {
        let identifier = uuid::Uuid::parse_str(&record.id)
            .map_err(|_| invalid("The saved worktree identifier is invalid."))?
            .to_string();
        let leaf = identifier
            .rsplit('-')
            .next()
            .ok_or_else(|| invalid("The worktree identifier is invalid."))?;
        let root = self.root.canonicalize()?;
        let retired_root = root.join("retired");
        match std::fs::symlink_metadata(&retired_root) {
            Ok(meta) => {
                if linked(&meta) || !meta.is_dir() || retired_root.canonicalize()? != retired_root {
                    return Err(invalid(
                        "The retained directory no longer belongs to this profile.",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let active = path_argument(&root.join(leaf));
        let retired = path_argument(&retired_root.join(&identifier));
        let saved = path_argument(Path::new(&record.path));
        if saved != active && saved != retired {
            return Err(invalid(
                "The saved path is outside this worktree's owned directories.",
            ));
        }
        Ok((active, retired))
    }

    /// The manager reserves both possible paths before any recovery mutation, even if one is absent.
    pub async fn recovery_folders(&self, chat_id: &str, id: &str) -> Result<Vec<PathBuf>> {
        let record = self.recovery_record(chat_id, id)?;
        let (active, retired) = self.recovery_paths(&record)?;
        Ok(vec![PathBuf::from(record.source), active, retired])
    }

    pub async fn recovery_preview(
        &self,
        chat_id: &str,
        id: &str,
        action: WorktreeRecoveryAction,
    ) -> Result<WorktreeRecoveryPreview> {
        let record = self.recovery_record(chat_id, id)?;
        match action {
            WorktreeRecoveryAction::Restore if record.state == WorktreeState::Retired => {}
            WorktreeRecoveryAction::Reconcile
                if matches!(
                    record.state,
                    WorktreeState::Creating
                        | WorktreeState::Merging
                        | WorktreeState::Retiring
                        | WorktreeState::Restoring
                        | WorktreeState::RecoveryRequired
                ) => {}
            _ => return Err(invalid("This worktree does not need that recovery action.")),
        }
        let (active, retired) = self.recovery_paths(&record)?;
        let active_exists = owned_directory(&active)?;
        let retired_exists = owned_directory(&retired)?;
        let (actual, was_retired) = match (active_exists, retired_exists) {
            (true, false) => (&active, false),
            (false, true) => (&retired, true),
            (true, true) => {
                return Err(invalid(
                    "Both owned paths are occupied. Review them with Git; neither directory was changed.",
                ));
            }
            (false, false) => {
                return Err(invalid(
                    "The owned directory is missing. Its branch and chat are retained. Locate it with Git before recovery; no repository-wide prune was run.",
                ));
            }
        };
        if action == WorktreeRecoveryAction::Restore && !was_retired {
            return Err(invalid(
                "The retired directory changed. Reconcile its current location first.",
            ));
        }
        let git = self.git().await?;
        let folder = inspect(&git, actual).await?;
        let source = inspect(&git, Path::new(&record.source)).await?;
        if folder.common != source.common
            || !folder.view.is_linked_worktree
            || folder.view.branch.as_deref() != Some(&record.branch)
            || !folder.view.blockers.is_empty()
        {
            return Err(invalid(
                "The owned worktree's repository, branch or Git operation no longer matches its record.",
            ));
        }
        let mut target = None;
        if let Some(WorktreeRecovery::Merge {
            branch,
            before_ref,
            prepared_ref,
            before_commit,
            prepared_commit,
        }) = &record.recovery
        {
            if action == WorktreeRecoveryAction::Reconcile {
                verify_recovery_ref(&git, &source.root, before_ref, before_commit).await?;
                verify_recovery_ref(&git, &source.root, prepared_ref, prepared_commit).await?;
                let head = source.view.head.as_deref();
                if source.view.branch.as_deref() != Some(branch)
                    || !matches!(head, Some(value) if value == before_commit || value == prepared_commit)
                    || !source.view.files.is_empty()
                    || !source.view.blockers.is_empty()
                {
                    return Err(invalid(
                        "The merge target is between the retained outcomes or has new changes. Resolve it with Git before reconciliation; neither recovery ref was removed.",
                    ));
                }
                actions::verify_merge_result(
                    &git,
                    &source.root,
                    branch,
                    head.ok_or_else(|| invalid("The merge target commit is unavailable."))?,
                )
                .await?;
                target = Some(source.view.clone());
            }
        } else if action == WorktreeRecoveryAction::Reconcile
            && record.recovery.is_none()
            && record.state != WorktreeState::Creating
        {
            return Err(invalid(
                "This older record has no typed recovery journal. Review its saved details with Git before continuing.",
            ));
        }
        let state = if was_retired && action != WorktreeRecoveryAction::Restore {
            WorktreeState::Retired
        } else {
            WorktreeState::Ready
        };
        let destination = if action == WorktreeRecoveryAction::Restore {
            active
        } else {
            actual.clone()
        };
        let warnings = vec![
            "This action retains the branch, all file contents and any merge recovery refs.".into(),
            if action == WorktreeRecoveryAction::Restore {
                "Restore moves the retained directory to its original app-owned path. It does not start an agent or relocate the chat.".into()
            } else {
                "Reconcile records the verified native Git state. It does not reset files, change commits or remove Git locks.".into()
            },
        ];
        let token = crate::ids::new_id();
        let preview = WorktreeRecoveryPreview {
            token: token.clone(),
            worktree: record.clone(),
            action,
            folder: folder.view.clone(),
            target,
            destination: destination.to_string_lossy().into_owned(),
            warnings,
            expires_at: crate::time::now_millis() + 600_000,
        };
        let mut prepared = self
            .recoveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prepared.retain(|_, value| value.expires > Instant::now());
        if prepared.len() >= 32 {
            return Err(invalid("Too many recovery previews are open."));
        }
        prepared.insert(
            token,
            PreparedRecovery {
                record,
                action,
                folder,
                source,
                destination,
                state,
                expires: Instant::now() + Duration::from_secs(600),
            },
        );
        Ok(preview)
    }

    pub async fn recovery_apply(
        &self,
        chat_id: &str,
        id: &str,
        token: &str,
    ) -> Result<WorktreeActionResult> {
        let prepared = self
            .recoveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(token)
            .ok_or_else(|| invalid("This recovery preview expired or was already used."))?;
        if prepared.expires <= Instant::now()
            || prepared.record.chat_id != chat_id
            || prepared.record.id != id
        {
            return Err(invalid(
                "This recovery preview no longer belongs to the selected chat.",
            ));
        }
        let _repository = self.hold_common(&prepared.source.common).await;
        let mut record = self.recovery_record(chat_id, &prepared.record.id)?;
        if !records::unchanged_since_review(&record, &prepared.record) {
            return Err(invalid("The worktree record changed after preview."));
        }
        let (active, retired) = self.recovery_paths(&record)?;
        let actual = path_argument(&prepared.folder.root);
        if actual != active && actual != retired {
            return Err(invalid("The recovery source is outside app ownership."));
        }
        let alternate = if actual == active { &retired } else { &active };
        if !owned_directory(&actual)? || owned_directory(alternate)? {
            return Err(invalid("The recovery source is missing."));
        }
        let git = self.git().await?;
        let folder = inspect(&git, &actual).await?;
        let source = inspect(&git, &prepared.source.root).await?;
        if folder.fingerprint != prepared.folder.fingerprint
            || source.fingerprint != prepared.source.fingerprint
            || folder.common != source.common
            || folder.common != prepared.folder.common
        {
            return Err(invalid(
                "The repository changed after recovery preview. Review it again.",
            ));
        }
        let mut error = None;
        if prepared.action == WorktreeRecoveryAction::Reconcile
            && let Some(WorktreeRecovery::Merge {
                before_ref,
                prepared_ref,
                before_commit,
                prepared_commit,
                ..
            }) = &record.recovery
        {
            verify_recovery_ref(&git, &source.root, before_ref, before_commit).await?;
            verify_recovery_ref(&git, &source.root, prepared_ref, prepared_commit).await?;
        }
        if prepared.action == WorktreeRecoveryAction::Restore {
            if owned_directory(&prepared.destination)? {
                return Err(invalid(
                    "The restore destination is occupied. Nothing was moved.",
                ));
            }
            record.state = WorktreeState::Restoring;
            record.recovery = Some(WorktreeRecovery::Restore);
            record.detail = Some("Restoring the retained directory to its app-owned path.".into());
            self.save_record(&record)?;
            let outcome = async {
                ensure_locked(&git, &source.root, &actual, &record.id).await?;
                git.run(&source.root, vec!["worktree".into(), "unlock".into(), actual.as_os_str().to_owned()], 8192).await?;
                git.run(&source.root, vec!["worktree".into(), "move".into(), "--".into(), actual.as_os_str().to_owned(), prepared.destination.as_os_str().to_owned()], 8192).await?;
                ensure_locked(&git, &source.root, &prepared.destination, &record.id).await?;
                let restored = inspect(&git, &prepared.destination).await?;
                if restored.common != folder.common || restored.view.branch != folder.view.branch || restored.view.head != folder.view.head || restored.view.files != folder.view.files {
                    return Err(invalid("The restored worktree changed while it was moved. Review its retained path."));
                }
                Ok::<_, GitError>(())
            }.await;
            error = outcome.err().map(|value| value.to_string());
        } else {
            ensure_locked(&git, &source.root, &actual, &record.id).await?;
        }
        if error.is_none() {
            record.path = prepared.destination.to_string_lossy().into_owned();
            record.state = prepared.state;
            record.detail =
                Some("Recovered after verifying repository ownership and native Git state.".into());
        } else {
            record.state = WorktreeState::RecoveryRequired;
            record.detail = error.clone();
        }
        self.save_record(&record)?;
        Ok(WorktreeActionResult {
            worktree: record,
            recovery: prepared.destination.to_string_lossy().into_owned(),
            commit: None,
            error,
        })
    }
}

async fn verify_recovery_ref(
    git: &git::Git,
    root: &Path,
    name: &str,
    expected: &str,
) -> Result<()> {
    if !name.starts_with("refs/aura/recovery/")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-'))
        || !oid(expected)
    {
        return Err(invalid("The saved merge recovery reference is invalid."));
    }
    let actual = text(
        &git.run(root, args(&["rev-parse", "--verify", name]), 128)
            .await?,
    )?;
    if actual != expected {
        return Err(invalid(
            "A retained recovery reference changed. Review it with Git before continuing.",
        ));
    }
    Ok(())
}

pub(super) async fn ensure_locked(
    git: &git::Git,
    source: &Path,
    folder: &Path,
    id: &str,
) -> Result<()> {
    let git_dir = text(
        &git.run(folder, args(&["rev-parse", "--absolute-git-dir"]), 8192)
            .await?,
    )?;
    let git_file = folder.join(".git");
    let metadata = std::fs::symlink_metadata(&git_file)?;
    if linked(&metadata) || !metadata.is_file() {
        return Err(invalid(
            "The owned worktree's Git registration was replaced.",
        ));
    }
    let backpointer = Path::new(&git_dir).join("gitdir");
    let registered = read_metadata_text(&backpointer, 8192)?;
    if Path::new(registered.trim_end_matches(['\r', '\n'])).canonicalize()?
        != git_file.canonicalize()?
    {
        return Err(invalid(
            "Native Git registers this directory as a different worktree. Its lock was retained.",
        ));
    }
    let lock = Path::new(&git_dir).join("locked");
    match std::fs::symlink_metadata(&lock) {
        Ok(meta) if !linked(&meta) && meta.is_file() && meta.len() <= 4096 => {
            let reason = read_metadata_text(&lock, 4096)?;
            if reason.trim() == format!("auracode {id}")
                || reason.trim() == format!("auracode retired {id}")
            {
                return Ok(());
            }
            return Err(invalid(
                "This worktree has a lock owned by another operation. Review it with Git; the lock was retained.",
            ));
        }
        Ok(_) => {
            return Err(invalid(
                "The worktree ownership lock has an unsupported layout.",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    git.run(
        source,
        vec![
            "worktree".into(),
            "lock".into(),
            "--reason".into(),
            format!("auracode {id}").into(),
            "--".into(),
            folder.as_os_str().to_owned(),
        ],
        8192,
    )
    .await?;
    Ok(())
}

fn read_metadata_text(path: &Path, limit: usize) -> Result<String> {
    use std::io::Read;
    let metadata = std::fs::symlink_metadata(path)?;
    if linked(&metadata) || !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(invalid(
            "The Git ownership metadata has an unsupported layout or size.",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid(
            "The Git ownership metadata changed while it was read.",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| invalid("The Git ownership metadata cannot be represented."))
}

pub(super) fn owned_directory(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if linked(&meta) || !meta.is_dir() => Err(invalid(
            "An owned recovery path was replaced with a link or another file type.",
        )),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn linked(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}
