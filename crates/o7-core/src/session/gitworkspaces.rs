//! Actor and folder reservations around explicitly confirmed filesystem mutations.

use o7_protocol::git::{
    GitWorkspace, IsolationInput, IsolationPreview, IsolationResult, ManagedWorktree,
    WorktreeAction, WorktreeActionPreview, WorktreeActionResult, WorktreeDestination,
    WorktreeRecoveryAction, WorktreeRecoveryPreview, WorktreeRelocationPreview,
};

use super::*;

#[derive(Clone)]
pub(super) struct PreparedRelocation {
    pub(super) record: ManagedWorktree,
    pub(super) destination: WorktreeDestination,
    pub(super) from: PathBuf,
    pub(super) to: PathBuf,
}

/// Folders and chats held for a change, released when dropped.
pub(super) struct Reservations {
    pub(super) shared: Arc<Shared>,
    pub(super) folders: Vec<PathBuf>,
    pub(super) actors: Vec<(mpsc::UnboundedSender<ChatMsg>, String)>,
    /// A quiet hold (`folder_guards.rs`): agents stay and only wait. Otherwise a reservation.
    pub(super) quiet: bool,
}

impl Reservations {
    pub(super) fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            folders: Vec::new(),
            actors: Vec::new(),
            quiet: false,
        }
    }

    pub(super) fn quiet(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            folders: Vec::new(),
            actors: Vec::new(),
            quiet: true,
        }
    }
}

impl Drop for Reservations {
    fn drop(&mut self) {
        for (mailbox, id) in &self.actors {
            let id = id.clone();
            let _ = mailbox.send(if self.quiet {
                ChatMsg::ReleaseQuiet { id }
            } else {
                ChatMsg::ReleaseGit { id }
            });
        }
        if self.quiet {
            self.shared.live.release_quiet(&self.folders);
        } else {
            for folder in &self.folders {
                self.shared.live.release_folder(folder);
            }
        }
    }
}

fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::Organization(message.into())
}

/// `chat`, when it runs on this device: Git worktrees are local.
fn native_git(chat: ChatRecord) -> Result<ChatRecord, SessionError> {
    if chat.agent_host != o7_protocol::agents::AgentHost::Native {
        return Err(invalid(
            "Git worktrees are supported only on this device's native host.",
        ));
    }
    Ok(chat)
}

impl SessionManager {
    pub async fn profile_worktrees(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<ManagedWorktree>, SessionError> {
        let records = self
            .worktrees()?
            .profile_records()
            .await
            .map_err(|error| invalid(error.to_string()))?;
        let Some(project_id) = project_id else {
            return Ok(records);
        };
        let organization = self
            .deps()
            .store
            .organize(
                crate::chatstore::organization::OrganizationCommand::Snapshot(
                    o7_protocol::workspace::WorkspaceFilter {
                        include_archived: true,
                        project_id: Some(project_id.into()),
                    },
                ),
            )
            .await
            .map_err(|error| storage_error(&error))?;
        let sources = organization
            .projects
            .into_iter()
            .filter(|project| project.id == project_id)
            .flat_map(|project| project.folders)
            .filter_map(|folder| folder.path)
            .collect::<Vec<_>>();
        let plain = |text: &str| {
            crate::fsutil::plain_windows_path(text)
                .unwrap_or_else(|| text.into())
                .replace('\\', "/")
        };
        Ok(records
            .into_iter()
            .filter(|record| {
                sources
                    .iter()
                    .any(|path| plain(path) == plain(&record.source))
            })
            .collect())
    }

    pub async fn profile_worktree_folder(&self, id: &str) -> Result<PathBuf, SessionError> {
        self.worktrees()?
            .profile_record_folder(id)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    pub async fn profile_worktree_recovery_preview(
        &self,
        id: &str,
        action: WorktreeRecoveryAction,
    ) -> Result<WorktreeRecoveryPreview, SessionError> {
        let service = self.worktrees()?;
        let record = service
            .profile_record(id)
            .await
            .map_err(|error| invalid(error.to_string()))?;
        service
            .recovery_preview(&record.chat_id, id, action)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    pub async fn profile_worktree_recreate_preview(
        &self,
        id: &str,
    ) -> Result<o7_protocol::git::WorktreeRecreatePreview, SessionError> {
        let service = self.worktrees()?;
        let record = service
            .profile_record(id)
            .await
            .map_err(|error| invalid(error.to_string()))?;
        service
            .recreate_preview(&record.chat_id, id)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    pub async fn profile_worktree_recovery_apply(
        &self,
        id: &str,
        token: &str,
        recreate: bool,
    ) -> Result<WorktreeActionResult, SessionError> {
        let manager = self.clone();
        let id = id.to_owned();
        let token = token.to_owned();
        self.deps().rt.spawn(async move {
            let service=manager.worktrees()?;
            let read = (Arc::clone(&service), id.clone());
            let (_organization, (record, folders)) = manager.hold_folders(move || {
                let (service, id) = read.clone();
                Box::pin(async move {
                    let record=service.profile_record(&id).await.map_err(|error|invalid(error.to_string()))?;
                    let folders=if recreate {service.recreate_folders(&record.chat_id,&id).await}else{service.recovery_folders(&record.chat_id,&id).await}.map_err(|error|invalid(error.to_string()))?;
                    Ok((folders.clone(), (record, folders)))
                })
            }).await?;
            let _reservations=manager.hold_exclusive(folders, None, None).await?;
            let result=if recreate {service.recreate_apply(&record.chat_id,&id,&token).await}else{service.recovery_apply(&record.chat_id,&id,&token).await}.map_err(|error|invalid(error.to_string()))?;
            if recreate && result.error.is_none() {
                manager.copy_worktree_instructions_quietly(&result.worktree).await;
            }
            Ok(result)
        }).await.map_err(|_|invalid("Recovery stopped unexpectedly. The saved record and retained files remain available for review."))?
    }

    /// With the profile's switch on (Instructions), copies the source folder's Git-ignored
    /// instruction files into a worktree this app just created or recreated, before its agent
    /// starts there. Copying never fails the operation; a problem comes back as a sentence to show
    /// beside its result.
    pub(super) async fn copy_worktree_instructions(
        &self,
        worktree: &ManagedWorktree,
    ) -> Option<String> {
        if !self.worktree_instruction_copies()
            || worktree.state != o7_protocol::git::WorktreeState::Ready
        {
            return None;
        }
        let service = self.worktrees().ok()?;
        let copy = service
            .copy_ignored_instructions(
                std::path::Path::new(&worktree.source),
                std::path::Path::new(&worktree.path),
            )
            .await;
        match copy {
            Ok((_, problems)) if problems.is_empty() => None,
            Ok((_, problems)) => Some(format!(
                "These instruction files were not copied into the worktree: {}.",
                problems.join(", ")
            )),
            Err(error) => Some(format!(
                "Instruction files were not copied into the worktree: {}.",
                error.to_string().trim_end_matches('.')
            )),
        }
    }

    /// For results with no place for a warning: the problem goes to the log.
    async fn copy_worktree_instructions_quietly(&self, worktree: &ManagedWorktree) {
        if let Some(problem) = self.copy_worktree_instructions(worktree).await {
            tracing::warn!(%problem, "worktree instruction copy");
        }
    }
    pub async fn worktree_pr_preview(
        &self,
        chat_id: &str,
        worktree_id: &str,
        input: o7_protocol::git::PullRequestInput,
    ) -> Result<o7_protocol::git::PullRequestPreview, SessionError> {
        self.git_chat(chat_id).await?;
        self.worktrees()?
            .pull_request_preview(chat_id, worktree_id, input)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    pub async fn worktree_pr_apply(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<o7_protocol::git::PullRequestResult, SessionError> {
        let manager = self.clone();
        let chat_id = chat_id.to_owned();
        let token = token.to_owned();
        self.deps().rt.spawn(async move {
            manager.git_chat(&chat_id).await?;
            let service = manager.worktrees()?;
            let (_organization, folders) = manager.hold_ready_worktrees(&service, &chat_id).await?;
            // Publishing pushes the branch and opens the request: agents there only wait.
            let _reservations = manager.hold_quiet(folders).await?;
            service.pull_request_apply(&chat_id, &token).await.map_err(|error|invalid(error.to_string()))
        }).await.map_err(|_| invalid("The publishing task ended unexpectedly. Refresh GitHub to check the branch and pull request before retrying."))?
    }

    pub(super) fn worktrees(
        &self,
    ) -> Result<Arc<crate::gitworkspaces::GitWorkspaces>, SessionError> {
        self.deps()
            .worktrees
            .clone()
            .ok_or_else(|| invalid("Git worktrees are unavailable in this runtime."))
    }

    pub(super) async fn git_chat(&self, chat_id: &str) -> Result<ChatRecord, SessionError> {
        native_git(self.chat_record(chat_id).await?)
    }

    /// Holds the chat's folder for a Git change, with the chat as it is once held.
    async fn hold_git_chat(
        &self,
        chat_id: &str,
    ) -> Result<(super::folder_locks::FolderGuard, ChatRecord), SessionError> {
        let (guard, chat) = self.hold_chat_folder(chat_id, &[]).await?;
        Ok((guard, native_git(chat)?))
    }

    /// Holds the source and folder of every ready worktree the chat owns, which its worktree
    /// actions reserve, conservatively, though each binds to one record.
    async fn hold_ready_worktrees(
        &self,
        service: &Arc<crate::gitworkspaces::GitWorkspaces>,
        chat_id: &str,
    ) -> Result<(super::folder_locks::FolderGuard, Vec<PathBuf>), SessionError> {
        let read = (Arc::clone(service), chat_id.to_owned());
        self.hold_folders(move || {
            let (service, chat_id) = read.clone();
            Box::pin(async move {
                let folders: Vec<PathBuf> = service
                    .list(&chat_id)
                    .await
                    .map_err(|error| invalid(error.to_string()))?
                    .into_iter()
                    .filter(|record| record.state == o7_protocol::git::WorktreeState::Ready)
                    .flat_map(|record| [PathBuf::from(record.source), PathBuf::from(record.path)])
                    .collect();
                Ok((folders.clone(), folders))
            })
        })
        .await
    }

    /// Reviews committing chosen files of the chat's folder as they are now.
    pub async fn git_commit_preview(
        &self,
        chat_id: &str,
        input: o7_protocol::git::CommitInput,
    ) -> Result<o7_protocol::git::CommitPreview, SessionError> {
        let chat = self.git_chat(chat_id).await?;
        self.worktrees()?
            .commit_preview(&chat.folder, input)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    /// Commits a reviewed commit, with no agent working in the folder.
    pub async fn git_commit_apply(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<o7_protocol::git::CommitResult, SessionError> {
        let manager = self.clone();
        let chat_id = chat_id.to_owned();
        let token = token.to_owned();
        self.deps()
            .rt
            .spawn(async move {
                let (_organization, chat) = manager.hold_git_chat(&chat_id).await?;
                // An idle agent may stay; its next turn waits for the commit.
                let _reservations = manager.hold_quiet(vec![chat.folder.clone()]).await?;
                manager
                    .worktrees()?
                    .commit_apply(&chat.folder, &token)
                    .await
                    .map_err(|error| invalid(error.to_string()))
            })
            .await
            .map_err(|_| {
                invalid("The commit ended unexpectedly. Refresh the Git workspace before retrying.")
            })?
    }

    /// Reviews a fast-forward push of the chat folder's branch.
    pub async fn git_push_preview(
        &self,
        chat_id: &str,
    ) -> Result<o7_protocol::git::PushPreview, SessionError> {
        let chat = self.git_chat(chat_id).await?;
        self.worktrees()?
            .push_preview(&chat.folder)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    /// Pushes a reviewed push. It reads no working file (only the branch and Git's settings),
    /// so agents in the folder keep working; the push holds its repository, as every Git change
    /// does, and nothing else.
    pub async fn git_push_apply(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<o7_protocol::git::PushResult, SessionError> {
        let manager = self.clone();
        let chat_id = chat_id.to_owned();
        let token = token.to_owned();
        self.deps()
            .rt
            .spawn(async move {
                let chat = manager.git_chat(&chat_id).await?;
                manager
                    .worktrees()?
                    .push_apply(&chat.folder, &token)
                    .await
                    .map_err(|error| invalid(error.to_string()))
            })
            .await
            .map_err(|_| invalid("The push ended unexpectedly. Refresh the Git workspace and check the remote before retrying."))?
    }

    /// The pull request open from `chat_id`'s branch on GitHub, read with GitHub CLI as the
    /// person is signed in; none when there is none or GitHub CLI cannot say.
    pub async fn pull_request(
        &self,
        chat_id: &str,
    ) -> Result<Option<crate::gitworkspaces::PullRequestView>, SessionError> {
        let chat = self.git_chat(chat_id).await?;
        self.worktrees()?
            .pull_request(&chat.folder)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    pub async fn git_workspace(&self, chat_id: &str) -> Result<GitWorkspace, SessionError> {
        let chat = self.git_chat(chat_id).await?;
        self.worktrees()?
            .inspect(&chat.folder)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    pub async fn managed_worktrees(
        &self,
        chat_id: &str,
    ) -> Result<Vec<ManagedWorktree>, SessionError> {
        self.git_chat(chat_id).await?;
        self.worktrees()?
            .list(chat_id)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    /// What `chat_id`'s own branch changed against its base, committed or not; `None` when the
    /// chat has no branch of its own.
    pub async fn chat_branch_changes(
        &self,
        chat_id: &str,
        base: Option<&str>,
        max_files: usize,
    ) -> Result<Option<crate::gitworkspaces::BranchChanges>, SessionError> {
        let Some((folder, base)) = self.chat_branch_base(chat_id, base).await? else {
            return Ok(None);
        };
        self.worktrees()?
            .branch_changes(&folder, &base, max_files)
            .await
            .map(Some)
            .map_err(|error| invalid(error.to_string()))
    }

    /// One of those files, before (at the base) and after (in the working tree), each at most
    /// `max_bytes`; `None` when the chat has no branch of its own or the path is not among its
    /// changes. Credential and private-key files are not shown, as in the project viewer.
    pub async fn chat_branch_diff(
        &self,
        chat_id: &str,
        base: Option<&str>,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<crate::gitworkspaces::BranchDiff>, SessionError> {
        if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
            return Err(invalid("The file path is invalid."));
        }
        if super::composer::restricted(Path::new(path)) {
            return Err(invalid("Credential and private-key files are not shown."));
        }
        let Some((folder, base)) = self.chat_branch_base(chat_id, base).await? else {
            return Ok(None);
        };
        self.worktrees()?
            .branch_diff(&folder, &base, path, max_bytes)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    /// The chat's folder and the commit its own branch is compared against: `base` when it is a
    /// full commit id in the folder's repository; else the base commit recorded for the chat's
    /// worktree; else, on a branch other than its project folder's default branch, where it
    /// left the default branch.
    async fn chat_branch_base(
        &self,
        chat_id: &str,
        base: Option<&str>,
    ) -> Result<Option<(PathBuf, String)>, SessionError> {
        let chat = self.git_chat(chat_id).await?;
        let service = self.worktrees()?;
        if let Some(found) = service
            .recorded_branch_base(chat_id, &chat.folder, base)
            .await
            .map_err(|error| invalid(error.to_string()))?
        {
            return Ok(Some((chat.folder, found)));
        }
        let Some(default) = self.folder_default_branch(&chat).await? else {
            return Ok(None);
        };
        Ok(service
            .default_branch_base(&chat.folder, &default)
            .await
            .map_err(|error| invalid(error.to_string()))?
            .map(|found| (chat.folder, found)))
    }

    /// The default branch recorded for the chat's project folder, when known.
    async fn folder_default_branch(
        &self,
        chat: &ChatRecord,
    ) -> Result<Option<String>, SessionError> {
        let Some(folder_id) = chat.folder_id.as_deref() else {
            return Ok(None);
        };
        let organization = self
            .deps()
            .store
            .organize(
                crate::chatstore::organization::OrganizationCommand::Snapshot(
                    o7_protocol::workspace::WorkspaceFilter {
                        include_archived: true,
                        project_id: None,
                    },
                ),
            )
            .await
            .map_err(|error| storage_error(&error))?;
        Ok(organization
            .projects
            .into_iter()
            .flat_map(|project| project.folders)
            .find(|folder| folder.id == folder_id)
            .and_then(|folder| folder.git_default_branch))
    }

    pub async fn isolation_preview(
        &self,
        chat_id: &str,
        input: IsolationInput,
    ) -> Result<IsolationPreview, SessionError> {
        let chat = self.git_chat(chat_id).await?;
        self.worktrees()?
            .isolation_preview(chat_id, &chat.folder, input)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    pub async fn isolation_apply(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<IsolationResult, SessionError> {
        let manager = self.clone();
        let chat_id = chat_id.to_owned();
        let token = token.to_owned();
        self.deps()
            .rt
            .spawn(async move {
                let (_organization, chat) = manager.hold_git_chat(&chat_id).await?;
                let _reservations = manager
                    .hold_exclusive(vec![chat.folder], None, None)
                    .await?;
                let mut result = manager
                    .worktrees()?
                    .isolation_apply(&chat_id, &token)
                    .await
                    .map_err(|error| invalid(error.to_string()))?;
                if let Some(problem) = manager.copy_worktree_instructions(&result.worktree).await {
                    result.warning = Some(match result.warning.take() {
                        Some(warning) => format!("{warning} {problem}"),
                        None => problem,
                    });
                }
                Ok(result)
            })
            .await
            .map_err(|_| {
                invalid("The worktree task ended unexpectedly. Review the saved worktree state.")
            })?
    }

    pub async fn worktree_action_preview(
        &self,
        chat_id: &str,
        worktree_id: &str,
        action: WorktreeAction,
    ) -> Result<WorktreeActionPreview, SessionError> {
        self.git_chat(chat_id).await?;
        self.worktrees()?
            .action_preview(chat_id, worktree_id, action)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    pub async fn worktree_action_apply(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<WorktreeActionResult, SessionError> {
        let manager = self.clone();
        let chat_id = chat_id.to_owned();
        let token = token.to_owned();
        // A dropped IPC reply does not release reservations while the owned Git command runs.
        // The task finishes journaling success or recovery before releasing actors and folders.
        self.deps().rt.spawn(async move {
            manager.git_chat(&chat_id).await?;
            let service=manager.worktrees()?;
            // Conservatively reserve every active worktree owned by this chat as well as sources.
            // The service still binds the token to one exact record before mutation.
            let (_organization, folders) = manager.hold_ready_worktrees(&service, &chat_id).await?;
            let _reservations=manager.hold_exclusive(folders, None, None).await?;
            service.action_apply(&chat_id,&token).await.map_err(|error|invalid(error.to_string()))
        }).await.map_err(|_|invalid("The Git task ended unexpectedly. Review its recovery record before trying again."))?
    }

    pub(super) async fn reserve_git_folders(
        &self,
        folders: Vec<PathBuf>,
    ) -> Result<Reservations, SessionError> {
        self.reserve_git_folders_with(folders, None, None).await
    }

    /// The setup command's terminal rewrites the worktree for as long as it runs.
    pub(super) async fn reserve_setup_folders(
        &self,
        folders: Vec<PathBuf>,
        chat_id: &str,
    ) -> Result<Reservations, SessionError> {
        self.hold_exclusive(folders, None, Some(chat_id)).await
    }

    pub(super) async fn reserve_git_folders_with(
        &self,
        folders: Vec<PathBuf>,
        except: Option<&str>,
        setup_chat: Option<&str>,
    ) -> Result<Reservations, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        if self.deps().store.is_paused() {
            return Err(SessionError::StorageUnavailable(
                "Wait for saving to resume before Git actions.".into(),
            ));
        }
        let (folders, chats) = self.chats_in(folders).await?;
        let mut reservations = Reservations::new(Arc::clone(self.shared()));
        if !self.shared().live.reserve_folders(&folders) {
            if self.shared().live.quiescing_now() {
                return Err(SessionError::ShuttingDown);
            }
            return Err(self.folder_busy(&folders, Vec::new(), except).await);
        }
        reservations.folders.clone_from(&folders);
        let mut refused = Vec::new();
        for chat in chats {
            if Some(chat.as_str()) == except {
                continue;
            }
            let id = crate::ids::new_id();
            let mailbox = self.actor(&chat);
            let (reply, answer) = oneshot::channel();
            let message = if setup_chat == Some(chat.as_str()) {
                ChatMsg::ReserveSetup {
                    id: id.clone(),
                    reply,
                }
            } else {
                ChatMsg::ReserveGit {
                    id: id.clone(),
                    reply,
                }
            };
            mailbox.send(message).map_err(|_| stopped())?;
            // Register the release before awaiting, so cancellation cannot leave an actor locked.
            reservations.actors.push((mailbox, id));
            match answer.await.map_err(|_| stopped())? {
                Ok(()) => {}
                Err(SessionError::Organization(_)) => refused.push(chat),
                Err(error) => return Err(error),
            }
        }
        if !refused.is_empty() {
            drop(reservations);
            return Err(self.folder_busy(&folders, refused, except).await);
        }
        Ok(reservations)
    }

    pub async fn worktree_recovery_preview(
        &self,
        chat_id: &str,
        worktree_id: &str,
        action: WorktreeRecoveryAction,
    ) -> Result<WorktreeRecoveryPreview, SessionError> {
        self.git_chat(chat_id).await?;
        self.worktrees()?
            .recovery_preview(chat_id, worktree_id, action)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    pub async fn worktree_recovery_apply(
        &self,
        chat_id: &str,
        worktree_id: &str,
        token: &str,
    ) -> Result<WorktreeActionResult, SessionError> {
        let manager = self.clone();
        let chat_id = chat_id.to_owned();
        let worktree_id = worktree_id.to_owned();
        let token = token.to_owned();
        self.deps().rt.spawn(async move {
            manager.git_chat(&chat_id).await?;
            let service = manager.worktrees()?;
            let read = (Arc::clone(&service), chat_id.clone(), worktree_id.clone());
            let (_organization, folders) = manager.hold_folders(move || {
                let (service, chat_id, worktree_id) = read.clone();
                Box::pin(async move {
                    let folders = service.recovery_folders(&chat_id, &worktree_id).await
                        .map_err(|error| invalid(error.to_string()))?;
                    Ok((folders.clone(), folders))
                })
            }).await?;
            let _reservations = manager.hold_exclusive(folders, None, None).await?;
            service.recovery_apply(&chat_id, &worktree_id, &token).await
                .map_err(|error| invalid(error.to_string()))
        }).await.map_err(|_| invalid("The recovery task ended unexpectedly. Its saved state and files remain available for review."))?
    }

    /// Reviews recreating a worktree folder that was deleted outside the app, from its branch.
    pub async fn worktree_recreate_preview(
        &self,
        chat_id: &str,
        worktree_id: &str,
    ) -> Result<o7_protocol::git::WorktreeRecreatePreview, SessionError> {
        self.git_chat(chat_id).await?;
        self.worktrees()?
            .recreate_preview(chat_id, worktree_id)
            .await
            .map_err(|error| invalid(error.to_string()))
    }

    /// Only the two owned paths are reserved. Adding a worktree for an existing branch writes
    /// Git metadata only and does not touch the source folder's files or index.
    pub async fn worktree_recreate_apply(
        &self,
        chat_id: &str,
        worktree_id: &str,
        token: &str,
    ) -> Result<WorktreeActionResult, SessionError> {
        let manager = self.clone();
        let chat_id = chat_id.to_owned();
        let worktree_id = worktree_id.to_owned();
        let token = token.to_owned();
        self.deps().rt.spawn(async move {
            manager.git_chat(&chat_id).await?;
            let service = manager.worktrees()?;
            let read = (Arc::clone(&service), chat_id.clone(), worktree_id.clone());
            let (_organization, folders) = manager.hold_folders(move || {
                let (service, chat_id, worktree_id) = read.clone();
                Box::pin(async move {
                    let folders = service.recreate_folders(&chat_id, &worktree_id).await
                        .map_err(|error| invalid(error.to_string()))?;
                    Ok((folders.clone(), folders))
                })
            }).await?;
            let _reservations = manager.hold_exclusive(folders, None, None).await?;
            let result = service.recreate_apply(&chat_id, &worktree_id, &token).await
                .map_err(|error| invalid(error.to_string()))?;
            if result.error.is_none() {
                manager.copy_worktree_instructions_quietly(&result.worktree).await;
            }
            Ok(result)
        }).await.map_err(|_| invalid("The recreate task ended unexpectedly. Its saved state remains available for review."))?
    }

    pub async fn worktree_relocation_preview(
        &self,
        chat_id: &str,
        worktree_id: &str,
        destination: WorktreeDestination,
    ) -> Result<WorktreeRelocationPreview, SessionError> {
        let chat = self.git_chat(chat_id).await?;
        let (record, to) = self
            .worktrees()?
            .relocation_target(chat_id, worktree_id, destination)
            .await
            .map_err(|error| invalid(error.to_string()))?;
        let from = chat
            .folder
            .canonicalize()
            .map_err(|_| invalid("Locate the chat's current folder before moving it."))?;
        let expected = match destination {
            WorktreeDestination::Isolated => Path::new(&record.source),
            WorktreeDestination::Direct => Path::new(&record.path),
        }
        .canonicalize()
        .map_err(|_| invalid("The reviewed folder is unavailable."))?;
        if from != expected || from == to {
            return Err(invalid(
                "This chat is no longer in the expected source folder for that move.",
            ));
        }
        let preview = self
            .continuation_preview_for_folder(
                chat_id,
                o7_protocol::conversation::ContinuationAction::SwitchAgent {
                    agent_id: chat.agent_id,
                },
                Some(PreparedRelocation {
                    record: record.clone(),
                    destination,
                    from: from.clone(),
                    to: to.clone(),
                }),
            )
            .await?;
        Ok(WorktreeRelocationPreview {
            token: preview.token,
            worktree: record,
            from: from.to_string_lossy().into_owned(),
            to: to.to_string_lossy().into_owned(),
            handoff: preview.handoff,
            warnings: preview.warnings,
            expires_at: preview.expires_at,
        })
    }

    pub async fn worktree_relocation_apply(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<o7_protocol::conversation::ContinuationResult, SessionError> {
        let manager = self.clone();
        let chat_id = chat_id.to_owned();
        let token = token.to_owned();
        self.deps().rt.spawn(async move {
            let prepared = lock(&manager.shared().continuations).remove(&token)
                .ok_or_else(|| invalid("This relocation preview expired or was already used."))?;
            if prepared.expires <= Instant::now() || prepared.apply.source_chat != chat_id {
                return Err(invalid("This relocation preview no longer belongs to the selected chat."));
            }
            let relocation = prepared.relocation.clone().ok_or_else(|| invalid("This preview is not a worktree relocation."))?;
            let (_organization, _) = manager.hold_chat_folder(
                &chat_id, &[relocation.from.clone(), relocation.to.clone()],
            ).await?;
            let (record, destination) = manager.worktrees()?.relocation_target(&chat_id, &relocation.record.id, relocation.destination)
                .await.map_err(|error| invalid(error.to_string()))?;
            if !crate::gitworkspaces::unchanged_since_review(&record, &relocation.record) || destination != relocation.to {
                return Err(invalid("The worktree changed after relocation preview. Review it again."));
            }
            // Other chats are reserved here. Continue serializes the moving chat's own admission
            // and persistence barrier, while folder reservations prevent any new native process.
            let _reservations = match manager.hold_exclusive(
                vec![relocation.from.clone(), relocation.to.clone()], Some(&chat_id), None,
            ).await {
                Ok(reservations) => reservations,
                Err(error @ SessionError::FolderBusy { .. }) => {
                    // The review stays usable: stop what is in the way, then apply it again.
                    lock(&manager.shared().continuations).insert(token.clone(), prepared);
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            manager.ask(&chat_id, |reply| ChatMsg::Continue {
                apply: Box::new(prepared.apply.clone()),
                prompt: None,
                reply,
            }).await
        }).await.map_err(|_| invalid("The relocation task ended unexpectedly. Reload the chat to inspect its saved folder and handoff."))?
    }
}
