//! Run in a worktree chat: the folder recipe's Run command, started in the chat's worktree in a
//! project terminal, with the worktree's ports (`gitworkspaces::run`).
use super::project_tools::ProjectTerminalPlan;
use super::*;
use ghost_protocol::worktree_setup::WorktreeRun;

impl SessionManager {
    /// The chat's Run command and the first of its worktree's ports; `None` for a chat that is
    /// not working in a ready worktree of its own.
    pub async fn worktree_run(&self, chat_id: &str) -> Result<Option<WorktreeRun>, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let chat = self.git_chat(chat_id).await?;
        let Ok((record, port)) = self.worktrees()?.worktree_port(chat_id).await else {
            return Ok(None);
        };
        let worktree = std::path::Path::new(&record.path);
        if worktree.canonicalize().ok() != chat.folder.canonicalize().ok() {
            return Ok(None);
        }
        let command = self.worktree_setup(chat_id).await?.input.run;
        Ok(Some(WorktreeRun {
            command,
            port,
            folder: crate::fsutil::plain_path(worktree)
                .to_string_lossy()
                .into_owned(),
        }))
    }

    /// Run's process, for a project terminal: the saved command in the chat's worktree. Like a
    /// project terminal it holds the folder, so worktree changes wait until it stops.
    pub async fn prepare_worktree_run(
        &self,
        chat_id: &str,
    ) -> Result<ProjectTerminalPlan, SessionError> {
        let command = self
            .worktree_setup(chat_id)
            .await?
            .input
            .run
            .ok_or_else(|| {
                SessionError::Organization(
                    "Save a Run command in this folder's setup first.".into(),
                )
            })?;
        let (_organization, _) = self.hold_chat_folder(chat_id, &[]).await?;
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let chat = self.git_chat(chat_id).await?;
        let spec = self
            .worktrees()?
            .run_command(chat_id, &command)
            .await
            .map_err(|error| SessionError::Organization(error.to_string()))?;
        if spec.folder.canonicalize().ok() != chat.folder.canonicalize().ok() {
            return Err(SessionError::Organization(
                "Run starts only in this chat's own worktree.".into(),
            ));
        }
        let guard = self.hold_folder(&spec.folder)?;
        Ok(ProjectTerminalPlan { spec, guard })
    }
}
