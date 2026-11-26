use super::*;
use ghost_protocol::changes::{
    ChangeDiff, ChangesSnapshot, HunkRevert, NativeRewind, RevertPreview, RevertResult,
};

pub(super) struct FolderMutation {
    shared: Arc<Shared>,
    folder: PathBuf,
}

impl Drop for FolderMutation {
    fn drop(&mut self) {
        self.shared.live.release_folder(&self.folder);
    }
}

impl SessionManager {
    /// Restores, or with `dry_run` lists, the files the agent's own checkpoints hold for before
    /// `turn_id`'s message (Claude's file rewind). The session must be running and idle.
    pub async fn chat_rewind_files(
        &self,
        chat_id: &str,
        turn_id: String,
        dry_run: bool,
    ) -> Result<NativeRewind, SessionError> {
        tokio::time::timeout(
            super::OPTION_TIMEOUT,
            self.ask(chat_id, |reply| super::actor::ChatMsg::RewindFiles {
                turn_id: turn_id.clone(),
                dry_run,
                reply,
            }),
        )
        .await
        .unwrap_or(Err(SessionError::SessionNotReady))
    }

    pub async fn chat_changes(
        &self,
        chat_id: &str,
        turn_id: Option<String>,
    ) -> Result<ChangesSnapshot, SessionError> {
        let (chat, checkpoint) = self
            .ask(chat_id, |reply| ChatMsg::CheckpointContext {
                turn_id: turn_id.clone(),
                reply,
            })
            .await?;
        let Some(checkpoint) = checkpoint else {
            return Ok(ChangesSnapshot {
                checkpoint: None,
                files: Vec::new(),
                unavailable_reason: Some("This turn has no saved checkpoint.".into()),
            });
        };
        let service = self.checkpoint_service()?;
        service
            .changes(chat_id, folder_id(&chat)?, &chat.folder, &checkpoint)
            .await
            .map_err(checkpoint_error)
    }

    pub async fn chat_change_diff(
        &self,
        chat_id: &str,
        checkpoint_id: &str,
        path: &str,
    ) -> Result<ChangeDiff, SessionError> {
        let (chat, _) = self
            .deps()
            .store
            .checkpoint_context(chat_id, None)
            .await
            .map_err(|error| storage_error(&error))?;
        self.checkpoint_service()?
            .diff(
                chat_id,
                folder_id(&chat)?,
                &chat.folder,
                checkpoint_id,
                path,
            )
            .await
            .map_err(checkpoint_error)
    }

    pub async fn checkpoint_revert_preview(
        &self,
        chat_id: &str,
        checkpoint_id: &str,
        paths: Vec<String>,
    ) -> Result<RevertPreview, SessionError> {
        let (chat, _) = self
            .deps()
            .store
            .checkpoint_context(chat_id, None)
            .await
            .map_err(|error| storage_error(&error))?;
        self.checkpoint_service()?
            .preview_revert(
                chat_id,
                folder_id(&chat)?,
                &chat.folder,
                checkpoint_id,
                paths,
            )
            .await
            .map_err(checkpoint_error)
    }

    /// Previews putting back the checkpoint's lines for hunks of one modified file; its revert
    /// goes through `checkpoint_revert` like any other.
    pub async fn checkpoint_hunk_revert_preview(
        &self,
        chat_id: &str,
        checkpoint_id: &str,
        selection: HunkRevert,
    ) -> Result<RevertPreview, SessionError> {
        let (chat, _) = self
            .deps()
            .store
            .checkpoint_context(chat_id, None)
            .await
            .map_err(|error| storage_error(&error))?;
        self.checkpoint_service()?
            .preview_hunk_revert(
                chat_id,
                folder_id(&chat)?,
                &chat.folder,
                checkpoint_id,
                selection,
            )
            .await
            .map_err(checkpoint_error)
    }

    pub async fn checkpoint_revert(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<RevertResult, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let (_organization, chat) = self.hold_chat_folder(chat_id, &[]).await?;
        folder_id(&chat)?;
        self.checkpoint_service()?;
        let path = chat.folder.clone();
        let folder = tokio::task::spawn_blocking(move || live::normalize_path(path))
            .await
            .map_err(|_| SessionError::Organization("Resolving the folder failed.".into()))?;
        // Reverting rewrites files: agents there that are not working stop first (this chat's
        // included), and what still works is named before anything stops.
        let holders = self
            .exclusive_can_go(std::slice::from_ref(&folder), None, &[])
            .await?;
        self.stop_unless_working(super::stopping::StopScope::Chats(holders))
            .await;
        if !self.shared().live.reserve_folder(&folder) {
            return Err(self
                .folder_busy(std::slice::from_ref(&folder), Vec::new(), None)
                .await);
        }
        let guard = Arc::new(FolderMutation {
            shared: Arc::clone(self.shared()),
            folder,
        });
        self.ask(chat_id, |reply| ChatMsg::Revert {
            token: token.into(),
            guard: Arc::clone(&guard),
            reply,
        })
        .await
    }

    fn checkpoint_service(&self) -> Result<Arc<crate::checkpoints::Checkpoints>, SessionError> {
        self.deps().checkpoints.clone().ok_or_else(|| {
            SessionError::Organization("Checkpoints are unavailable in this runtime.".into())
        })
    }
}

pub(super) fn checkpoint_error(error: crate::checkpoints::CheckpointError) -> SessionError {
    SessionError::Organization(error.to_string())
}

fn folder_id(chat: &ChatRecord) -> Result<&str, SessionError> {
    chat.folder_id.as_deref().ok_or_else(|| {
        SessionError::Organization("Locate this chat's folder before using checkpoints.".into())
    })
}
