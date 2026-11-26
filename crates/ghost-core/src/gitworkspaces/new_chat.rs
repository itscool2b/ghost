//! One-step worktrees for new chats. This module creates the branch and folder; the session saves
//! the chat afterwards. A chat that cannot be saved rolls back only what this operation created.

use std::collections::HashSet;
use std::time::Duration;

use ghost_protocol::git::{
    ChatFolderMode, ManagedWorktree, WorktreeChatChip, WorktreeChatPlan, WorktreeState,
};

use super::{
    create::{branch_exists, creating_record, local_branches, slug},
    git::{args, path_argument, registrations, resolve_commit, text},
    inspect::{Inspection, inspect},
    recovery::{ensure_locked, owned_directory},
    *,
};

pub(super) struct Prepared {
    source: Inspection,
    record: ManagedWorktree,
    start_ref: String,
    expires: Instant,
    transfer: Option<transfer::Snapshot>,
}

pub(crate) struct NewChatRequest<'a> {
    pub(crate) chat_id: &'a str,
    pub(crate) folder: &'a Path,
    pub(crate) start_branch: Option<&'a str>,
    pub(crate) branch: Option<&'a str>,
    pub(crate) title: Option<&'a str>,
    pub(crate) bring_changes: bool,
}

#[derive(Debug)]
pub(crate) enum NewChatPlan {
    /// The folder cannot hold a worktree chat. The message explains the direct fallback.
    Direct(String),
    Worktree(Box<WorktreeChatPlan>),
}

const MAX_LISTED_BRANCHES: usize = 200;
const NOT_GIT: &str = "This folder is not a Git repository. The chat works directly in the folder.";

fn invalid(message: &str) -> GitError {
    GitError::Message(message.into())
}

impl GitWorkspaces {
    /// Reviews a new worktree for a chat that does not exist yet. Nothing is created.
    pub(crate) async fn new_chat_preview(
        &self,
        request: NewChatRequest<'_>,
    ) -> Result<NewChatPlan> {
        if request.chat_id.is_empty() || request.chat_id.len() > 128 {
            return Err(invalid("The chat identifier is invalid."));
        }
        let git = match self.git().await {
            Ok(git) => git,
            // Without usable Git, a folder with no repository metadata is still a plain folder.
            Err(_) if std::fs::symlink_metadata(request.folder.join(".git")).is_err() => {
                return Ok(NewChatPlan::Direct(NOT_GIT.into()));
            }
            Err(error) => return Err(error),
        };
        if let Some(message) = direct_reason(&git, request.folder).await? {
            return Ok(NewChatPlan::Direct(message));
        }
        let source = inspect(&git, request.folder).await?;
        let branches = local_branches(&git, &source.root, "refs/heads/").await?;
        let starting_branch =
            match request.start_branch.map(str::trim) {
                Some(name) => {
                    if !branches.iter().any(|branch| branch == name) {
                        return Err(invalid("Choose an existing local branch to start from."));
                    }
                    name.to_owned()
                }
                None => source.view.branch.clone().ok_or_else(|| {
                    invalid("HEAD is detached. Choose a local branch to start from.")
                })?,
            };
        let start_ref = format!("refs/heads/{starting_branch}");
        let starting_commit = resolve_commit(&git, &source.root, &start_ref)
            .await
            .map_err(|_| {
                invalid(
                    "The starting branch has no commit yet. Create a first commit before starting a worktree chat.",
                )
            })?;
        let transfer = if request.bring_changes {
            if !source.view.blockers.is_empty() {
                return Err(GitError::Message(source.view.blockers.join(" ")));
            }
            if source.view.head.as_deref() != Some(starting_commit.as_str()) {
                return Err(invalid(
                    "Uncommitted changes can be brought only when the worktree starts from the source folder's current commit.",
                ));
            }
            Some(self.snapshot_changes(&git, &source).await?)
        } else {
            None
        };
        let existing = branches
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let (id, path) = self.allocate_path(&source.root)?;
        let (branch, branch_generated) = match request
            .branch
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            Some(name) => {
                checked_branch_name(&git, &source.root, name).await?;
                if conflicts(name, &existing) {
                    return Err(invalid(
                        "A branch with this name, or one that overlaps it, already exists. Choose another name; existing branches are never reused.",
                    ));
                }
                (name.to_owned(), false)
            }
            None => {
                let base = request
                    .title
                    .and_then(title_slug)
                    .unwrap_or_else(|| short_id(&id));
                (generated_branch(&slug(&base)?, &existing)?, true)
            }
        };
        let mut warnings = Vec::new();
        if let Some(transfer) = &transfer {
            warnings.push("Reviewed working-file contents, deletions and nonignored untracked files are copied as unstaged changes. The source files and index are preserved; source staging is not recreated.".into());
            warnings.extend(transfer.warnings.clone());
        } else if !source.view.files.is_empty() {
            warnings.push(format!(
                "Uncommitted changes stay in the source folder. The worktree starts from the last commit on {starting_branch}."
            ));
        }
        if transfer.is_none() && !source.view.blockers.is_empty() {
            warnings.push(format!(
                "The source folder reports: {} The new worktree starts from the last commit on {starting_branch} and does not change the source.",
                source.view.blockers.join(" ")
            ));
        }
        if source.view.has_submodules {
            warnings.push(
                "Submodules are not initialized automatically. Review setup in the new worktree."
                    .into(),
            );
        }
        let now = crate::time::now_millis();
        let record = creating_record(
            id,
            request.chat_id,
            &source,
            &path,
            branch.clone(),
            starting_commit.clone(),
            now,
        );
        let token = crate::ids::new_id();
        let plan = WorktreeChatPlan {
            token: token.clone(),
            source: source.view.clone(),
            starting_branch: starting_branch.clone(),
            starting_commit,
            branch: branch.clone(),
            branch_generated,
            // Shown to the person: the plain spelling of a Windows verbatim path.
            path: crate::fsutil::plain_path(Path::new(&record.path))
                .to_string_lossy()
                .into_owned(),
            bring_changes: request.bring_changes,
            local_branches: branches.into_iter().take(MAX_LISTED_BRANCHES).collect(),
            setup: None,
            chip: WorktreeChatChip {
                mode: ChatFolderMode::Worktree,
                label: format!("Worktree from {starting_branch} on {branch}"),
                starting_branch: Some(starting_branch),
                branch: Some(branch),
            },
            warnings,
            expires_at: now + 600_000,
        };
        let mut prepared = self
            .new_chats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prepared.retain(|_, entry| entry.expires > Instant::now());
        if prepared.len() >= 32 {
            return Err(invalid(
                "Too many new worktree chat previews are open. Wait for one to expire.",
            ));
        }
        prepared.insert(
            token,
            Prepared {
                source,
                record,
                start_ref,
                expires: Instant::now() + Duration::from_secs(600),
                transfer,
            },
        );
        Ok(NewChatPlan::Worktree(Box::new(plan)))
    }

    /// Creates the reviewed worktree. A failed creation is rolled back when that is provably safe;
    /// otherwise the record stays as RecoveryRequired with the reason. Either way it is an error.
    pub(crate) async fn new_chat_apply(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<ManagedWorktree> {
        let prepared = self
            .new_chats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(token)
            .ok_or_else(|| {
                invalid("This worktree chat preview expired or was already used. Review it again.")
            })?;
        if prepared.record.chat_id != chat_id || prepared.expires <= Instant::now() {
            return Err(invalid(
                "This worktree chat preview expired or was already used. Review it again.",
            ));
        }
        let _repository = self.hold_common(&prepared.source.common).await;
        let git = self.git().await?;
        let current = inspect(&git, &prepared.source.root).await?;
        if current.common != prepared.source.common {
            return Err(invalid(
                "The repository changed after the preview. Review a new preview.",
            ));
        }
        let commit = resolve_commit(&git, &current.root, &prepared.start_ref).await?;
        if commit != prepared.record.base_commit {
            return Err(invalid(
                "The starting branch moved after the preview. Review a new preview.",
            ));
        }
        let contents = match prepared.transfer {
            Some(snapshot) => {
                if current.fingerprint != prepared.source.fingerprint
                    || !current.view.blockers.is_empty()
                    || current.view.head.as_deref() != Some(commit.as_str())
                {
                    return Err(invalid(
                        "The repository changed after the preview. Review a new preview.",
                    ));
                }
                Some(self.capture_changes(&git, &current, snapshot).await?)
            }
            None => None,
        };
        let brought = contents.is_some();
        let mut record = self
            .add_worktree(&git, &current, prepared.record, contents)
            .await?;
        if record.state == WorktreeState::Ready {
            return Ok(record);
        }
        let detail = record.detail.clone().unwrap_or_default();
        match self.remove_created(&git, &record, brought).await {
            Ok(()) => Err(GitError::Message(format!(
                "The worktree could not be created: {detail} The new branch and folder were removed; the source folder was not changed."
            ))),
            Err(error) => {
                record.detail = Some(format!("{detail} Automatic rollback stopped: {error}"));
                self.save_record(&record)?;
                Err(GitError::Message(format!(
                    "The worktree could not be created. It was kept for review at {} on branch {}: {detail}",
                    record.path, record.branch
                )))
            }
        }
    }

    /// Undoes a new chat's worktree after its chat could not be saved. The folder and branch are
    /// removed only while they are exactly as this operation created them. Otherwise the record is
    /// kept as RecoveryRequired and the error explains what was retained.
    pub(crate) async fn new_chat_rollback(
        &self,
        chat_id: &str,
        id: &str,
        brought_changes: bool,
    ) -> Result<()> {
        let source = self.recovery_record(chat_id, id)?.source;
        let git = self.git().await?;
        let _repository = self.hold_repository(&git, Path::new(&source)).await;
        let mut record = self.recovery_record(chat_id, id)?;
        match self.remove_created(&git, &record, brought_changes).await {
            Ok(()) => Ok(()),
            Err(error) => {
                record.state = WorktreeState::RecoveryRequired;
                record.detail = Some(format!(
                    "The chat for this worktree could not be saved, and automatic rollback stopped: {error}"
                ));
                self.save_record(&record)?;
                Err(error)
            }
        }
    }

    async fn remove_created(
        &self,
        git: &git::Git,
        record: &ManagedWorktree,
        allow_changes: bool,
    ) -> Result<()> {
        let (active, _) = self.recovery_paths(record)?;
        if path_argument(Path::new(&record.path)) != active {
            return Err(invalid(
                "The worktree is outside its app-owned path. It was kept.",
            ));
        }
        let source = inspect(git, Path::new(&record.source)).await?;
        if owned_directory(&active)? {
            let folder = inspect(git, &active).await?;
            if folder.common != source.common
                || !folder.view.is_linked_worktree
                || folder.view.branch.as_deref() != Some(&record.branch)
                || folder.view.head.as_deref() != Some(&record.base_commit)
                || !folder.view.blockers.is_empty()
                || (!allow_changes && !folder.view.files.is_empty())
            {
                return Err(invalid(
                    "The new worktree changed after it was created. Its folder and branch were kept for review.",
                ));
            }
            ensure_locked(git, &source.root, &active, &record.id).await?;
            git.run(
                &source.root,
                vec![
                    "worktree".into(),
                    "unlock".into(),
                    active.as_os_str().to_owned(),
                ],
                8192,
            )
            .await?;
            let mut remove: Vec<std::ffi::OsString> = vec!["worktree".into(), "remove".into()];
            if allow_changes {
                // Only copies of the source's preserved working files can be present.
                remove.push("--force".into());
            }
            remove.push("--".into());
            remove.push(active.as_os_str().to_owned());
            if let Err(error) = git.run(&source.root, remove, 8192).await {
                let _ = ensure_locked(git, &source.root, &active, &record.id).await;
                return Err(error);
            }
            if owned_directory(&active)? {
                return Err(invalid(
                    "The new worktree folder is still present. It was kept for review.",
                ));
            }
        }
        let reference = format!("refs/heads/{}", record.branch);
        if registrations(git, &source.root)
            .await?
            .iter()
            .any(|entry| entry.branch.as_deref() == Some(reference.as_str()))
        {
            return Err(invalid(
                "The new branch is checked out in a worktree. It was kept.",
            ));
        }
        if branch_exists(git, &source.root, &record.branch).await? {
            // Deletes only while the branch still points at its starting commit.
            git.run(
                &source.root,
                vec![
                    "update-ref".into(),
                    "-d".into(),
                    reference.into(),
                    record.base_commit.clone().into(),
                ],
                4096,
            )
            .await
            .map_err(|_| invalid("The new branch has commits or moved. It was kept for review."))?;
        }
        self.update_records(|entries| {
            entries.retain(|entry| entry.id != record.id);
            Ok(())
        })
    }
}

/// `Some(reason)` when the folder cannot hold a worktree chat and the chat should work directly
/// in it: a plain folder, or a folder inside another repository that is not its root.
async fn direct_reason(git: &git::Git, folder: &Path) -> Result<Option<String>> {
    let folder = folder.canonicalize().map_err(|_| {
        invalid("The working folder is unavailable. Locate it before starting a chat.")
    })?;
    let output = git
        .output(&folder, args(&["rev-parse", "--show-toplevel"]), 8192)
        .await?;
    if output.exit.as_ref().and_then(|exit| exit.code) != Some(0) {
        // A repository Git refuses to open is not a plain folder. Inspection reports why.
        if std::fs::symlink_metadata(folder.join(".git")).is_ok() {
            return Ok(None);
        }
        return Ok(Some(NOT_GIT.into()));
    }
    let root = std::path::PathBuf::from(text(&output.stdout)?);
    if root.canonicalize().ok().as_ref() != Some(&folder) {
        return Ok(Some(
            "This folder is inside another Git repository but is not its root. The chat works directly in the folder.".into(),
        ));
    }
    Ok(None)
}

/// Validates a typed branch name with Git and a conservative, portable character set.
async fn checked_branch_name(git: &git::Git, root: &Path, name: &str) -> Result<()> {
    let reserved = |part: &str| {
        let base = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        matches!(base.as_str(), "con" | "prn" | "aux" | "nul")
            || (base.len() == 4
                && (base.starts_with("com") || base.starts_with("lpt"))
                && base.as_bytes()[3].is_ascii_digit())
    };
    if name.len() > 200
        || name.starts_with('-')
        || name.eq_ignore_ascii_case("HEAD")
        || name.contains("@{")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
        || name.split('/').any(reserved)
    {
        return Err(invalid(
            "Use letters, numbers, hyphens, underscores, dots and slashes for the branch name.",
        ));
    }
    let output = git
        .output(
            root,
            vec!["check-ref-format".into(), "--branch".into(), name.into()],
            4096,
        )
        .await?;
    if output.exit.as_ref().and_then(|exit| exit.code) != Some(0) || text(&output.stdout)? != name {
        return Err(invalid(
            "Git does not accept this branch name. Choose another name.",
        ));
    }
    Ok(())
}

/// True when creating `name` would reuse an existing branch or collide with one as a file or
/// directory on a case-insensitive filesystem. `existing` holds lowercased short names.
fn conflicts(name: &str, existing: &HashSet<String>) -> bool {
    let name = name.to_ascii_lowercase();
    existing.iter().any(|branch| {
        *branch == name
            || name.starts_with(&format!("{branch}/"))
            || branch.starts_with(&format!("{name}/"))
    })
}

fn generated_branch(slug: &str, existing: &HashSet<String>) -> Result<String> {
    for suffix in 1..=1000 {
        let branch = if suffix == 1 {
            format!("ghost/{slug}")
        } else {
            format!("ghost/{slug}-{suffix}")
        };
        if !conflicts(&branch, existing) {
            return Ok(branch);
        }
    }
    Err(invalid(
        "Too many branches use this name. Choose another name.",
    ))
}

/// A branch slug from a chat title: ASCII letters and digits joined by single hyphens.
fn title_slug(title: &str) -> Option<String> {
    let mut slug = String::new();
    for character in title.trim().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 36 {
            break;
        }
    }
    slug.truncate(36);
    let slug = slug.trim_matches('-');
    (!slug.is_empty()).then(|| slug.to_owned())
}

fn short_id(id: &str) -> String {
    id.rsplit('-')
        .next()
        .unwrap_or(id)
        .chars()
        .take(8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_slugs_and_generated_names_avoid_every_overlap() {
        assert_eq!(
            title_slug("  Fix the Login bug!  ").as_deref(),
            Some("fix-the-login-bug")
        );
        assert_eq!(title_slug("日本語").as_deref(), None);
        assert_eq!(
            title_slug(&"word ".repeat(20)).map(|slug| slug.len() <= 36),
            Some(true)
        );
        assert!(!title_slug(&"a-".repeat(30)).unwrap().ends_with('-'));
        let existing = HashSet::from(["ghost/fix".to_owned(), "ghost/fix-2/sub".to_owned()]);
        assert_eq!(generated_branch("fix", &existing).unwrap(), "ghost/fix-3");
        assert!(conflicts("GHOST/FIX", &existing));
        assert!(conflicts("ghost/fix/deeper", &existing));
        assert!(conflicts("ghost", &existing));
        assert!(!conflicts("ghost/fixed", &existing));
        assert_eq!(short_id("0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"), "0c1d2e3f");
    }
}
