//! Recreates an app-owned worktree folder that was deleted outside the app, from its retained
//! branch. Uncommitted files from the deleted folder cannot be restored; commits are kept.

use std::time::Duration;

use ghost_protocol::git::{
    ManagedWorktree, WorktreeActionResult, WorktreeRecovery, WorktreeRecreatePreview, WorktreeState,
};

use super::{
    git::{path_argument, registrations, resolve_commit, same_path_text},
    inspect::{Inspection, inspect},
    recovery::{ensure_locked, owned_directory},
    *,
};

pub(super) struct Prepared {
    record: ManagedWorktree,
    commit: String,
    common: PathBuf,
    expires: Instant,
}

fn invalid(message: &str) -> GitError {
    GitError::Message(message.into())
}

impl GitWorkspaces {
    /// Both owned paths are reserved, even though neither folder exists.
    pub async fn recreate_folders(&self, chat_id: &str, id: &str) -> Result<Vec<PathBuf>> {
        let record = self.recovery_record(chat_id, id)?;
        let (active, retired) = self.recovery_paths(&record)?;
        Ok(vec![active, retired])
    }

    pub async fn recreate_preview(
        &self,
        chat_id: &str,
        id: &str,
    ) -> Result<WorktreeRecreatePreview> {
        let record = self.recovery_record(chat_id, id)?;
        let git = self.git().await?;
        let (active, source, commit) = self.recreate_check(&git, &record).await?;
        let short = commit.get(..12).unwrap_or(&commit);
        let warnings = vec![
            "Uncommitted files that were in the deleted folder are gone. Everything committed on the branch is kept.".into(),
            format!(
                "The folder is recreated at its original app-owned path from the last commit on {} ({short}).",
                record.branch
            ),
            "Setup files such as .env are not restored. Review worktree setup again afterwards.".into(),
        ];
        let token = crate::ids::new_id();
        let preview = WorktreeRecreatePreview {
            token: token.clone(),
            worktree: record.clone(),
            path: active.to_string_lossy().into_owned(),
            branch: record.branch.clone(),
            commit: commit.clone(),
            warnings,
            expires_at: crate::time::now_millis() + 600_000,
        };
        let mut prepared = self
            .recreations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prepared.retain(|_, value| value.expires > Instant::now());
        if prepared.len() >= 32 {
            return Err(invalid("Too many recreate previews are open."));
        }
        prepared.insert(
            token,
            Prepared {
                record,
                commit,
                common: source.common,
                expires: Instant::now() + Duration::from_secs(600),
            },
        );
        Ok(preview)
    }

    /// Runs `git worktree add -f -f --lock` for the reviewed branch. Double force replaces only the
    /// stale, locked registration of the deleted folder; a branch checked out anywhere else is
    /// refused before Git runs.
    pub async fn recreate_apply(
        &self,
        chat_id: &str,
        id: &str,
        token: &str,
    ) -> Result<WorktreeActionResult> {
        let prepared = self
            .recreations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(token)
            .ok_or_else(|| invalid("This recreate preview expired or was already used."))?;
        if prepared.expires <= Instant::now()
            || prepared.record.chat_id != chat_id
            || prepared.record.id != id
        {
            return Err(invalid(
                "This recreate preview no longer belongs to the selected chat.",
            ));
        }
        let _repository = self.hold_common(&prepared.common).await;
        let mut record = self.recovery_record(chat_id, id)?;
        if !records::unchanged_since_review(&record, &prepared.record) {
            return Err(invalid("The worktree record changed after preview."));
        }
        let git = self.git().await?;
        let (active, source, commit) = self.recreate_check(&git, &record).await?;
        if commit != prepared.commit || source.common != prepared.common {
            return Err(invalid(
                "The branch or repository changed after preview. Review it again.",
            ));
        }
        record.state = WorktreeState::Creating;
        record.recovery = Some(WorktreeRecovery::Create);
        record.detail = Some(
            "Recreating the deleted folder from its branch. An interrupted operation requires review."
                .into(),
        );
        self.save_record(&record)?;
        let outcome = async {
            git.run(
                &source.root,
                vec![
                    "worktree".into(),
                    "add".into(),
                    "-f".into(),
                    "-f".into(),
                    "--lock".into(),
                    "--reason".into(),
                    format!("auracode {}", record.id).into(),
                    "--".into(),
                    active.as_os_str().to_owned(),
                    record.branch.clone().into(),
                ],
                64 * 1024,
            )
            .await?;
            ensure_locked(&git, &source.root, &active, &record.id).await?;
            let created = inspect(&git, &active).await?;
            if created.common != source.common
                || !created.view.is_linked_worktree
                || created.view.branch.as_deref() != Some(&record.branch)
                || created.view.head.as_deref() != Some(commit.as_str())
                || !created.view.files.is_empty()
            {
                return Err(invalid(
                    "The recreated worktree does not match its branch. Its folder was kept for review.",
                ));
            }
            Ok::<_, GitError>(())
        }
        .await;
        let error = outcome.err().map(|error| error.to_string());
        if error.is_none() {
            record.path = active.to_string_lossy().into_owned();
            record.state = WorktreeState::Ready;
            record.detail = Some(
                "Recreated from its branch after the folder was removed outside the app. Uncommitted files from the removed folder could not be restored."
                    .into(),
            );
        } else {
            record.state = WorktreeState::RecoveryRequired;
            record.detail = error.clone();
        }
        self.save_record(&record)?;
        Ok(WorktreeActionResult {
            worktree: record,
            recovery: active.to_string_lossy().into_owned(),
            commit: Some(commit),
            error,
        })
    }

    async fn recreate_check(
        &self,
        git: &git::Git,
        record: &ManagedWorktree,
    ) -> Result<(PathBuf, Inspection, String)> {
        if !matches!(
            record.state,
            WorktreeState::Ready | WorktreeState::RecoveryRequired
        ) {
            return Err(invalid(
                "Only a worktree whose folder was deleted can be recreated.",
            ));
        }
        if matches!(record.recovery, Some(WorktreeRecovery::Merge { .. }))
            && record.state != WorktreeState::Ready
        {
            return Err(invalid(
                "Resolve this worktree's retained merge recovery before recreating its folder.",
            ));
        }
        let (active, retired) = self.recovery_paths(record)?;
        if path_argument(Path::new(&record.path)) != active {
            return Err(invalid(
                "This worktree was retired. Only an active worktree folder can be recreated.",
            ));
        }
        if owned_directory(&active)? || owned_directory(&retired)? {
            return Err(invalid(
                "The worktree folder still exists. Nothing needs to be recreated.",
            ));
        }
        let source = inspect(git, Path::new(&record.source)).await?;
        let reference = format!("refs/heads/{}", record.branch);
        let commit = resolve_commit(git, &source.root, &reference)
            .await
            .map_err(|_| {
                invalid(
                    "The worktree's branch no longer exists. The folder cannot be recreated from it.",
                )
            })?;
        let ours = format!("auracode {}", record.id);
        for entry in registrations(git, &source.root).await? {
            if entry.branch.as_deref() != Some(reference.as_str()) {
                continue;
            }
            let owned = entry.locked.as_deref().is_some_and(|reason| reason == ours)
                || same_path_text(&entry.path, &active);
            if !owned {
                return Err(invalid(
                    "The branch is checked out in another worktree. Switch that worktree to another branch first.",
                ));
            }
        }
        Ok((active, source, commit))
    }
}
