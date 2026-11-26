//! One-step "new chat in its own worktree". The preview names everything that will happen; the
//! apply creates the worktree, saves the chat in it and runs the reviewed setup. A chat that cannot
//! be saved rolls back only the branch and folder this operation created.

use ghost_protocol::git::{
    ChatFolderMode, ManagedWorktree, WorktreeChatChip, WorktreeChatInput, WorktreeChatPreview,
    WorktreeChatResult,
};
use ghost_protocol::worktree_setup::{WorktreeSetupInput, WorktreeSetupResult};

use super::workspace::{bounded, checked_folder, missing, usable};
use super::*;
use crate::chatstore::organization::{Organization, OrganizationCommand};
use crate::gitworkspaces::{NewChatPlan, NewChatRequest};

pub(super) struct Prepared {
    chat_id: String,
    folder_id: String,
    source: PathBuf,
    agent_id: String,
    title: Option<String>,
    bring_changes: bool,
    setup: Option<WorktreeSetupInput>,
    expires: Instant,
}

fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::Organization(message.into())
}

fn present(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

impl SessionManager {
    /// Reviews a new chat whose folder will be a new worktree of a project folder. Nothing is
    /// created. A folder that is not a repository root returns the direct fallback instead.
    pub async fn worktree_chat_preview(
        &self,
        input: WorktreeChatInput,
    ) -> Result<WorktreeChatPreview, SessionError> {
        bounded(&input.agent_id, "Agent ID", 128)?;
        if self.deps().detector.agent(&input.agent_id).is_none() {
            return Err(SessionError::UnknownAgent(input.agent_id));
        }
        let title = present(&input.title);
        if let Some(title) = title {
            bounded(title, "Chat title", 512)?;
        }
        for (value, label) in [
            (present(&input.start_branch), "Starting branch"),
            (present(&input.branch), "Branch name"),
        ] {
            if let Some(value) = value {
                bounded(value, label, 256)?;
            }
        }
        let preview = self.chat_create_preview(&input.folder_id).await?;
        let path = usable(&preview.folder)?.to_owned();
        let source = checked_folder(&path).await?;
        if self
            .worktrees()?
            .profile_records()
            .await
            .map_err(|error| invalid(error.to_string()))?
            .iter()
            .any(|record| crate::fsutil::plain_path(Path::new(&record.path)) == source)
        {
            return Err(invalid(
                "Choose the original project folder as the source of a new worktree.",
            ));
        }
        let chat_id = crate::ids::new_id();
        let outcome = self
            .worktrees()?
            .new_chat_preview(NewChatRequest {
                chat_id: &chat_id,
                folder: &source,
                start_branch: present(&input.start_branch),
                branch: present(&input.branch),
                title,
                bring_changes: input.bring_changes,
            })
            .await
            .map_err(|error| invalid(error.to_string()))?;
        let mut plan = match outcome {
            NewChatPlan::Direct(message) => {
                return Ok(WorktreeChatPreview::NotGitRepository {
                    folder: path,
                    message,
                    chip: WorktreeChatChip {
                        mode: ChatFolderMode::Direct,
                        starting_branch: None,
                        branch: None,
                        label: format!("Direct in {}", preview.folder.display_name),
                    },
                });
            }
            NewChatPlan::Worktree(plan) => *plan,
        };
        let recipe = self
            .deps()
            .store
            .worktree_setup(crate::chatstore::worktree_setup::Command {
                chat_id: String::new(),
                source: path,
                save: None,
                folder_id: Some(input.folder_id.clone()),
                folder_save: false,
            })
            .await
            .map_err(|error| storage_error(&error))?
            .input;
        let saved = !recipe.files.is_empty() || recipe.command.is_some();
        let setup = (input.run_setup && saved).then_some(recipe);
        if let Some(setup) = &setup {
            if !setup.files.is_empty() {
                plan.warnings.push(format!(
                    "After creation, saved setup copies {} listed {} from the source folder. Existing files are never overwritten. Environment files may contain application secrets; their contents are not shown, logged or stored.",
                    setup.files.len(),
                    if setup.files.len() == 1 { "file" } else { "files" }
                ));
            }
            if let Some(command) = &setup.command {
                plan.warnings.push(format!(
                    "Then the saved setup command ({} {}) is ready to run in an owned terminal in the new worktree, before the agent starts. It can modify the worktree and uses your normal command environment.",
                    command.program,
                    command.args.join(" ")
                ));
            }
        } else if saved {
            plan.warnings.push(
                "The saved worktree setup is skipped for this chat. It can be reviewed later from the chat's Git panel."
                    .into(),
            );
        }
        plan.setup = setup.clone();
        let mut prepared = lock(&self.shared().worktree_chats);
        prepared.retain(|_, entry| entry.expires > Instant::now());
        if prepared.len() >= 32 {
            return Err(invalid(
                "Too many new worktree chat previews are open. Wait for one to expire.",
            ));
        }
        prepared.insert(
            plan.token.clone(),
            Prepared {
                chat_id,
                folder_id: input.folder_id,
                source,
                agent_id: ghost_agents::agent_of(&input.agent_id).to_owned(),
                title: title.map(str::to_owned),
                bring_changes: input.bring_changes,
                setup,
                expires: Instant::now() + Duration::from_secs(600),
            },
        );
        Ok(WorktreeChatPreview::Worktree {
            plan: Box::new(plan),
        })
    }

    /// Applies a reviewed worktree chat preview. The worktree is created first, then the chat is
    /// saved with the worktree as its folder. If the chat cannot be saved, the new branch and folder
    /// are removed while they are unchanged; otherwise they stay as a RecoveryRequired record.
    pub async fn worktree_chat_apply(
        &self,
        token: &str,
    ) -> Result<WorktreeChatResult, SessionError> {
        self.apply_worktree_chat_owned(token, true).await
    }

    /// [`worktree_chat_apply`](Self::worktree_chat_apply) without starting the agent: a project's
    /// worker is recorded as one first, so the session that starts has its tools.
    pub(crate) async fn worktree_chat_apply_stopped(
        &self,
        token: &str,
    ) -> Result<WorktreeChatResult, SessionError> {
        self.apply_worktree_chat_owned(token, false).await
    }

    async fn apply_worktree_chat_owned(
        &self,
        token: &str,
        start: bool,
    ) -> Result<WorktreeChatResult, SessionError> {
        let manager = self.clone();
        let token = token.to_owned();
        // A dropped IPC reply does not interrupt the owned Git command or the rollback journal.
        self.deps()
            .rt
            .spawn(async move { manager.apply_worktree_chat(&token, start).await })
            .await
            .map_err(|_| {
                invalid(
                    "The new chat task ended unexpectedly. Review the project's chats and worktrees before trying again.",
                )
            })?
    }

    async fn apply_worktree_chat(
        &self,
        token: &str,
        start: bool,
    ) -> Result<WorktreeChatResult, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        if self.deps().store.is_paused() {
            return Err(SessionError::StorageUnavailable(
                "Wait for saving to resume before creating a chat.".into(),
            ));
        }
        let prepared = lock(&self.shared().worktree_chats)
            .remove(token)
            .filter(|prepared| prepared.expires > Instant::now())
            .ok_or_else(|| {
                invalid("This worktree chat preview expired or was already used. Review it again.")
            })?;
        if self.deps().detector.agent(&prepared.agent_id).is_none() {
            return Err(SessionError::UnknownAgent(prepared.agent_id));
        }
        // Making the worktree and saving its chat go behind other changes to the source folder
        // only: another project's chats, terminals and Git actions never wait for this one.
        let source_order = self
            .shared()
            .organization
            .scoped(vec![prepared.source.clone()])
            .await;
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let folder = self.chat_create_preview(&prepared.folder_id).await?;
        if checked_folder(usable(&folder.folder)?).await? != prepared.source {
            return Err(invalid(
                "The project folder changed after the preview. Review it again.",
            ));
        }
        let service = self.worktrees()?;
        // Copying working files reads the source, so its chats and terminals must be idle. Without a
        // copy, Git only adds a branch and a worktree registration; the source is not touched.
        let source = if prepared.bring_changes {
            match self.hold_quiet(vec![prepared.source.clone()]).await {
                Ok(held) => Some(held),
                Err(error @ SessionError::FolderBusy { .. }) => {
                    // The review stays usable: stop what works there, then create it again.
                    lock(&self.shared().worktree_chats).insert(token.to_owned(), prepared);
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let worktree = service
            .new_chat_apply(&prepared.chat_id, token)
            .await
            .map_err(|error| invalid(error.to_string()))?;
        let organization = match self.save_worktree_chat(&prepared, &worktree).await {
            Ok(organization) => organization,
            Err(error) => {
                let rollback = service
                    .new_chat_rollback(&prepared.chat_id, &worktree.id, prepared.bring_changes)
                    .await;
                return Err(match rollback {
                    Ok(()) => invalid(format!(
                        "The chat could not be saved: {error}. The new worktree and branch were removed; the source folder was not changed."
                    )),
                    Err(kept) => invalid(format!(
                        "The chat could not be saved: {error}. The worktree was kept for review at {} on branch {}: {kept}",
                        worktree.path, worktree.branch
                    )),
                });
            }
        };
        drop(source);
        drop(source_order);
        // Setup and instruction copies go behind other changes to the new worktree.
        let worktree_order = self
            .shared()
            .organization
            .scoped(vec![PathBuf::from(&worktree.path)])
            .await;
        let mut warnings = Vec::new();
        let mut setup = None;
        if let Some(input) = prepared.setup.clone() {
            match self
                .new_worktree_setup(&prepared.chat_id, &worktree, input)
                .await
            {
                Ok(result) => {
                    if let Some(token) = &result.command_token {
                        service
                            .bind_setup_generation(&prepared.chat_id, token, 1)
                            .map_err(|error| invalid(error.to_string()))?;
                    }
                    if result.warning.is_some() || result.command_token.is_none() {
                        self.set_setup_attempt(
                            &prepared.chat_id,
                            Some(1),
                            if result.warning.is_some() {
                                ghost_protocol::worktree_setup::WorktreeSetupState::Failed
                            } else {
                                ghost_protocol::worktree_setup::WorktreeSetupState::Succeeded
                            },
                            result.warning.clone(),
                        )
                        .await?;
                    }
                    warnings.extend(result.warning.clone());
                    setup = Some(result);
                }
                // The error is a sentence of its own; its full stop ends this one too.
                Err(error) => {
                    let detail = format!(
                        "Setup failed: {}. Review and retry setup.",
                        error.to_string().trim_end_matches('.')
                    );
                    self.set_setup_attempt(
                        &prepared.chat_id,
                        Some(1),
                        ghost_protocol::worktree_setup::WorktreeSetupState::Failed,
                        Some(detail.clone()),
                    )
                    .await?;
                    warnings.push(detail);
                }
            }
        }
        // Instruction files Git ignores (a local AGENTS.md or CLAUDE.md) reach the worktree before
        // its agent starts, and after setup, so a setup copy of the same file is kept.
        warnings.extend(self.copy_worktree_instructions(&worktree).await);
        // The agent starts outside the folders' order: starting never waits on Git.
        drop(worktree_order);
        // A pending setup command runs first in its owned terminal; the agent then starts explicitly.
        let started = prepared.setup.is_none()
            || setup
                .as_ref()
                .is_some_and(|result| result.warning.is_none() && result.command_token.is_none());
        if started && start {
            let _ = self
                .ask(&prepared.chat_id, |reply| ChatMsg::Start {
                    fresh: false,
                    explicit: false,
                    reply,
                })
                .await;
        }
        let chat = self
            .workspace_view(organization)
            .await
            .chats
            .into_iter()
            .find(|chat| chat.chat.id == prepared.chat_id)
            .ok_or_else(missing)?;
        Ok(WorktreeChatResult {
            chat,
            worktree,
            setup,
            started,
            warnings,
        })
    }

    async fn save_worktree_chat(
        &self,
        prepared: &Prepared,
        worktree: &ManagedWorktree,
    ) -> Result<Organization, SessionError> {
        #[cfg(test)]
        if self
            .shared()
            .fail_worktree_chat_save
            .swap(false, Ordering::SeqCst)
        {
            return Err(SessionError::StorageUnavailable(
                "injected save failure".into(),
            ));
        }
        self.organization(OrganizationCommand::CreateWorktreeChat {
            setup_worktree_id: prepared.setup.as_ref().map(|_| worktree.id.clone()),
            id: prepared.chat_id.clone(),
            source_folder_id: prepared.folder_id.clone(),
            path: PathBuf::from(&worktree.path),
            agent_id: prepared.agent_id.clone(),
            title: prepared.title.clone(),
        })
        .await
    }

    /// Copies the reviewed setup files and prepares the command token, as the explicit setup flow
    /// does. Only the new worktree is reserved: its chat has not started and the source is read.
    async fn new_worktree_setup(
        &self,
        chat_id: &str,
        worktree: &ManagedWorktree,
        input: WorktreeSetupInput,
    ) -> Result<WorktreeSetupResult, SessionError> {
        let service = self.worktrees()?;
        let _reservations = self.hold_quiet(vec![PathBuf::from(&worktree.path)]).await?;
        let preview = service
            .setup_preview(chat_id, &worktree.id, input)
            .await
            .map_err(|error| invalid(error.to_string()))?;
        service
            .setup_apply(chat_id, &preview.token)
            .await
            .map_err(|error| invalid(error.to_string()))
    }
}
