use ghost_protocol::git::{
    IsolationInput, IsolationPreview, IsolationResult, ManagedWorktree, WorktreeRecovery,
    WorktreeState,
};
use std::collections::HashSet;
use std::ffi::OsString;
use std::time::Duration;

use super::{
    git::{args, text},
    inspect::{Inspection, inspect},
    *,
};

pub(super) struct Prepared {
    source: Inspection,
    record: ManagedWorktree,
    expires: Instant,
    transfer: Option<transfer::Snapshot>,
}

fn invalid(message: &str) -> GitError {
    GitError::Message(message.into())
}

impl GitWorkspaces {
    pub async fn isolation_preview(
        &self,
        chat_id: &str,
        folder: &Path,
        input: IsolationInput,
    ) -> Result<IsolationPreview> {
        if chat_id.len() > 128 || chat_id.is_empty() {
            return Err(invalid("The chat identifier is invalid."));
        }
        let slug = slug(&input.slug)?;
        let git = self.git().await?;
        let source = inspect(&git, folder).await?;
        if !source.view.blockers.is_empty() {
            return Err(GitError::Message(source.view.blockers.join(" ")));
        }
        let transfer = if input.bring_changes {
            Some(self.snapshot_changes(&git, &source).await?)
        } else {
            None
        };
        let names = local_branches(&git, &source.root, "refs/heads/ghost/")
            .await?
            .into_iter()
            .map(|line| line.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let branch = unique_branch(&slug, &names)?;
        let (id, path) = self.allocate_path(&source.root)?;
        let mut warnings = Vec::new();
        if let Some(transfer) = &transfer {
            warnings.push("Reviewed working-file contents, deletions and nonignored untracked files are copied as unstaged changes. The source files and index are preserved; source staging is not recreated.".into());
            warnings.extend(transfer.warnings.clone());
        } else if !source.view.files.is_empty() {
            warnings.push("Uncommitted changes stay in the source folder. This worktree starts from the reviewed commit.".into());
        }
        if source.view.has_submodules {
            warnings.push(
                "Submodules are not initialized automatically. Review setup in the new worktree."
                    .into(),
            );
        }
        warnings.push(
            "Repository setup commands and ignored files are not copied or executed automatically."
                .into(),
        );
        let now = crate::time::now_millis();
        let record = creating_record(
            id,
            chat_id,
            &source,
            &path,
            branch.clone(),
            source
                .view
                .head
                .clone()
                .ok_or_else(|| invalid("The source commit is unavailable."))?,
            now,
        );
        let token = crate::ids::new_id();
        let preview = IsolationPreview {
            token: token.clone(),
            source: source.view.clone(),
            branch,
            // Shown to the person: the plain spelling of a Windows verbatim path.
            path: crate::fsutil::plain_path(Path::new(&record.path))
                .to_string_lossy()
                .into_owned(),
            bring_changes: input.bring_changes,
            warnings,
            expires_at: now + 600_000,
        };
        let mut prepared = self
            .prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prepared.retain(|_, entry| entry.expires > Instant::now());
        if prepared.len() >= 32 {
            return Err(invalid(
                "Too many isolation previews are open. Close one before trying again.",
            ));
        }
        prepared.insert(
            token,
            Prepared {
                source,
                record,
                expires: Instant::now() + Duration::from_secs(600),
                transfer,
            },
        );
        Ok(preview)
    }

    /// Call only for the user's explicit confirmation of the displayed preview. An uncertain Git
    /// outcome is retained as RecoveryRequired, with no force removal or branch rollback.
    pub async fn isolation_apply(&self, chat_id: &str, token: &str) -> Result<IsolationResult> {
        let prepared = self
            .prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(token)
            .ok_or_else(|| {
                invalid("This isolation preview expired or was already used. Review it again.")
            })?;
        if prepared.record.chat_id != chat_id || prepared.expires <= Instant::now() {
            return Err(invalid(
                "This isolation preview no longer belongs to the selected chat.",
            ));
        }
        let _repository = self.hold_common(&prepared.source.common).await;
        let git = self.git().await?;
        let current = inspect(&git, &prepared.source.root).await?;
        if current.fingerprint != prepared.source.fingerprint
            || current.common != prepared.source.common
            || !current.view.blockers.is_empty()
        {
            return Err(invalid(
                "The repository changed after the preview. Review a new preview before creating isolation.",
            ));
        }
        let contents = match prepared.transfer {
            Some(snapshot) => Some(self.capture_changes(&git, &current, snapshot).await?),
            None => None,
        };
        let record = self
            .add_worktree(&git, &current, prepared.record, contents)
            .await?;
        let warning=(record.state!=WorktreeState::Ready).then(||"Creation did not complete cleanly. The recorded path and branch were preserved for recovery.".into());
        Ok(IsolationResult {
            worktree: record,
            warning,
        })
    }

    /// Reads the reviewed working files of `source` for a later copy into a new worktree.
    pub(super) async fn snapshot_changes(
        &self,
        git: &git::Git,
        source: &Inspection,
    ) -> Result<transfer::Snapshot> {
        let root = source.root.clone();
        let files = source.view.files.clone();
        let protected = self.protected(git);
        tokio::task::spawn_blocking(move || transfer::snapshot(&root, &files, &protected))
            .await
            .map_err(|_| invalid("Could not inspect working-file contents."))?
    }

    /// Captures the reviewed bytes only when the working files still match `snapshot`.
    pub(super) async fn capture_changes(
        &self,
        git: &git::Git,
        current: &Inspection,
        snapshot: transfer::Snapshot,
    ) -> Result<transfer::Contents> {
        let root = current.root.clone();
        let files = current.view.files.clone();
        let protected = self.protected(git);
        tokio::task::spawn_blocking(move || {
            if transfer::snapshot(&root, &files, &protected)? != snapshot {
                return Err(invalid(
                    "Working-file contents changed after preview. Review isolation again.",
                ));
            }
            transfer::contents(&root, &snapshot)
        })
        .await
        .map_err(|_| invalid("Could not capture the reviewed working-file contents."))?
    }

    fn protected(&self, git: &git::Git) -> Vec<PathBuf> {
        git.protected
            .iter()
            .cloned()
            .chain(self.root.parent().map(Path::to_path_buf))
            .collect()
    }

    /// Reserves a new identifier and an unoccupied app-owned path outside the source repository.
    pub(super) fn allocate_path(&self, source_root: &Path) -> Result<(String, PathBuf)> {
        // The stable app-data worktree lives outside the original repository and watcher tree.
        let storage = self.root.canonicalize()?;
        if storage.starts_with(source_root) {
            return Err(invalid(
                "The application data directory is inside this repository. Choose an external app-data directory before isolation.",
            ));
        }
        let id = crate::ids::new_id();
        let leaf = id
            .rsplit('-')
            .next()
            .ok_or_else(|| invalid("Could not allocate a worktree identifier."))?;
        let path = git::path_argument(&storage.join(leaf));
        if cfg!(windows) && path.to_string_lossy().encode_utf16().count() > 200 {
            return Err(invalid(
                "The application data path is too long for a safe Windows worktree. Choose a shorter data directory.",
            ));
        }
        if path.try_exists()? {
            return Err(invalid(
                "The proposed worktree directory is already occupied. Review a new preview.",
            ));
        }
        Ok((id, path))
    }

    /// Journals `record` as Creating, runs `git worktree add -b`, verifies the result and copies the
    /// reviewed contents. Failures are journaled as RecoveryRequired on the returned record.
    pub(super) async fn add_worktree(
        &self,
        git: &git::Git,
        current: &Inspection,
        mut record: ManagedWorktree,
        contents: Option<transfer::Contents>,
    ) -> Result<ManagedWorktree> {
        if Path::new(&record.path)
            .parent()
            .map(std::fs::canonicalize)
            .transpose()?
            != Some(self.root.canonicalize()?)
        {
            return Err(invalid(
                "The application worktree directory changed after preview. Review a new preview.",
            ));
        }
        if Path::new(&record.path).try_exists()? {
            return Err(invalid(
                "The worktree path is now occupied. Review a new preview.",
            ));
        }
        if branch_exists(git, &current.root, &record.branch).await? {
            return Err(invalid(
                "The proposed branch already exists or could not be checked. Review a new preview.",
            ));
        }
        self.update_records(|records| {
            if records.len() >= 5000 {
                return Err(invalid(
                    "This profile has too many managed worktree records.",
                ));
            }
            records.push(record.clone());
            Ok(())
        })?;
        let command: Vec<OsString> = vec![
            "worktree".into(),
            "add".into(),
            "--lock".into(),
            "--reason".into(),
            format!("auracode {}", record.id).into(),
            "-b".into(),
            record.branch.clone().into(),
            "--".into(),
            record.path.clone().into(),
            record.base_commit.clone().into(),
        ];
        let created = git.run(&current.root, command, 64 * 1024).await;
        let mut verified = match created {
            Ok(_) => inspect(git, Path::new(&record.path)).await.and_then(|new| {
                if new.common == current.common
                    && new.view.branch.as_deref() == Some(&record.branch)
                    && new.view.head.as_deref() == Some(&record.base_commit)
                    && new.view.files.is_empty()
                {
                    Ok(())
                } else {
                    Err(invalid(
                        "The created worktree does not match the reviewed branch and commit.",
                    ))
                }
            }),
            Err(error) => Err(error),
        };
        if verified.is_ok()
            && let Some(contents) = contents
        {
            let root = PathBuf::from(&record.path);
            verified = async {
                let checked_root = root.clone();
                let (contents, baseline) = tokio::task::spawn_blocking(move || {
                    transfer::destination(&checked_root, &contents).map(|baseline| (contents, baseline))
                }).await.map_err(|_| invalid("Could not inspect the new worktree before copying."))??;
                let clean = inspect(git, &root).await?;
                if clean.common != current.common || clean.view.head.as_deref() != Some(&record.base_commit)
                    || clean.view.branch.as_deref() != Some(&record.branch) || !clean.view.files.is_empty()
                    || !clean.view.blockers.is_empty() {
                    return Err(invalid("The new worktree changed before copying. Its files were preserved for review."));
                }
                let copied_root = root.clone();
                tokio::task::spawn_blocking(move || transfer::apply(&copied_root, &contents, &baseline)).await
                    .map_err(|_| invalid("The file copy stopped unexpectedly. Inspect its retained recovery files."))??;
                let after = inspect(git, &root).await?;
                if after.common != current.common || after.view.head.as_deref() != Some(&record.base_commit)
                    || after.view.branch.as_deref() != Some(&record.branch) || !after.view.blockers.is_empty() {
                    return Err(invalid("The new worktree's Git state changed during copying. Review its preserved files and branch."));
                }
                Ok(())
            }.await;
        }
        match verified {
            Ok(()) => {
                record.state = WorktreeState::Ready;
                record.detail = None;
            }
            Err(error) => {
                record.state = WorktreeState::RecoveryRequired;
                record.detail = Some(error.to_string());
            }
        }
        // Into the registry as it is now: other worktrees' records changed while Git ran.
        self.save_record(&record)?;
        Ok(record)
    }
}

/// A Creating journal entry. Creation is recorded before Git starts.
pub(super) fn creating_record(
    id: String,
    chat_id: &str,
    source: &Inspection,
    path: &Path,
    branch: String,
    base_commit: String,
    now: ghost_protocol::Millis,
) -> ManagedWorktree {
    ManagedWorktree {
        id,
        chat_id: chat_id.into(),
        source: source.view.root.clone(),
        path: path.to_string_lossy().into_owned(),
        branch,
        base_commit,
        state: WorktreeState::Creating,
        detail: Some(
            "Creation is recorded before Git starts. An interrupted operation requires review."
                .into(),
        ),
        created_at: now,
        recovery: Some(WorktreeRecovery::Create),
        port: None,
    }
}

/// Local branch names under `prefix` (a `refs/heads/` path), most recently committed first. Full
/// names are stripped explicitly: `refname:short` may add `heads/` when a tag has the same name.
pub(super) async fn local_branches(
    git: &git::Git,
    root: &Path,
    prefix: &str,
) -> Result<Vec<String>> {
    let branches = git
        .run(
            root,
            args(&[
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname)",
                prefix,
            ]),
            1024 * 1024,
        )
        .await?;
    Ok(text(&branches)?
        .lines()
        .filter_map(|line| line.strip_prefix("refs/heads/"))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Whether `refs/heads/<branch>` exists. An unclear answer is an error, never "absent".
pub(super) async fn branch_exists(git: &git::Git, root: &Path, branch: &str) -> Result<bool> {
    let check = git
        .output(
            root,
            vec![
                "show-ref".into(),
                "--verify".into(),
                "--quiet".into(),
                format!("refs/heads/{branch}").into(),
            ],
            4096,
        )
        .await?;
    match check.exit.as_ref().and_then(|exit| exit.code) {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(invalid(
            "The proposed branch already exists or could not be checked. Review a new preview.",
        )),
    }
}

pub(super) fn slug(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 36
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || value.starts_with('-')
        || value.ends_with('-')
    {
        return Err(invalid(
            "Use 1 to 36 letters, numbers or internal hyphens for the branch name.",
        ));
    }
    Ok(value)
}

pub(super) fn unique_branch(slug: &str, names: &HashSet<String>) -> Result<String> {
    for suffix in 1..=1000 {
        let branch = if suffix == 1 {
            format!("ghost/{slug}")
        } else {
            format!("ghost/{slug}-{suffix}")
        };
        if !names.contains(&branch.to_ascii_lowercase()) {
            return Ok(branch);
        }
    }
    Err(invalid(
        "Too many branches use this name. Choose another name.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn branch_names_cannot_be_refs_options_or_windows_path_traversal() {
        for value in ["../main", "-force", "a/b", "x.lock", "x..y", "a\\b", ""] {
            assert!(slug(value).is_err(), "{value}");
        }
        assert_eq!(slug(" Hello-world ").expect("slug"), "hello-world");
        assert_eq!(
            unique_branch(
                "name",
                &HashSet::from(["ghost/name".into(), "ghost/name-2".into()])
            )
            .expect("suffix"),
            "ghost/name-3"
        );
    }
}
