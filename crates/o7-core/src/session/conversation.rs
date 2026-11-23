use super::*;
use crate::chatstore::conversation::{
    ApplyContinuation, ConversationCommand, ConversationReply, ConversationSnapshot,
    PendingContinuation,
};
use o7_agents::agent_of;
use o7_drivers::driver::NativeStartOptions;
use o7_protocol::{
    PromptPart,
    conversation::{
        ContinuationAction, ContinuationPreview, ContinuationResult, ConversationTurns,
        EditablePrompt,
    },
};

pub(super) struct PreparedContinuation {
    pub(super) expires: Instant,
    pub(super) apply: ApplyContinuation,
    pub(super) prompt: Option<(String, Vec<PromptPart>)>,
    pub(super) relocation: Option<super::gitworkspaces::PreparedRelocation>,
}

fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::Organization(message.into())
}

impl SessionManager {
    pub async fn conversation_pending(
        &self,
        chat_id: &str,
    ) -> Result<Option<o7_protocol::conversation::PendingHandoff>, SessionError> {
        match self
            .ask(chat_id, |reply| ChatMsg::Conversation {
                command: ConversationCommand::Pending {
                    chat_id: chat_id.into(),
                },
                reply,
            })
            .await?
        {
            ConversationReply::Pending(pending) => Ok(pending),
            _ => Err(invalid("The saved handoff response was invalid.")),
        }
    }
    pub async fn conversation_turns(
        &self,
        chat_id: &str,
        before_ordinal: Option<u32>,
    ) -> Result<ConversationTurns, SessionError> {
        let command = ConversationCommand::Turns {
            chat_id: chat_id.into(),
            before: before_ordinal,
        };
        match self
            .ask(chat_id, |reply| ChatMsg::Conversation {
                command: command.clone(),
                reply,
            })
            .await?
        {
            ConversationReply::Turns(turns) => Ok(turns),
            _ => Err(invalid("The conversation history response was invalid.")),
        }
    }

    pub async fn conversation_editable(
        &self,
        chat_id: &str,
        turn_id: &str,
    ) -> Result<EditablePrompt, SessionError> {
        let snapshot = self
            .conversation_snapshot(chat_id, Some(turn_id.into()), None, false)
            .await?;
        let turn = snapshot
            .selected
            .ok_or_else(|| invalid("This turn is unavailable."))?;
        let (text, has_context) =
            crate::conversation::editable(&turn).ok_or_else(ambiguous_edit)?;
        Ok(EditablePrompt {
            turn_id: turn.id,
            text,
            has_context,
        })
    }

    async fn conversation_snapshot(
        &self,
        chat_id: &str,
        selected: Option<String>,
        target_agent: Option<String>,
        include_selected: bool,
    ) -> Result<ConversationSnapshot, SessionError> {
        let command = ConversationCommand::Snapshot {
            chat_id: chat_id.into(),
            selected,
            target_agent,
            include_selected,
        };
        match self
            .ask(chat_id, |reply| ChatMsg::Conversation {
                command: command.clone(),
                reply,
            })
            .await?
        {
            ConversationReply::Snapshot(snapshot) => Ok(*snapshot),
            _ => Err(invalid("The conversation preview response was invalid.")),
        }
    }

    pub async fn continuation_preview(
        &self,
        chat_id: &str,
        action: ContinuationAction,
    ) -> Result<ContinuationPreview, SessionError> {
        self.continuation_preview_for_folder(chat_id, action, None)
            .await
    }

    pub(super) async fn continuation_preview_for_folder(
        &self,
        chat_id: &str,
        action: ContinuationAction,
        relocation: Option<super::gitworkspaces::PreparedRelocation>,
    ) -> Result<ContinuationPreview, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let (selected, target, include_selected) = match &action {
            ContinuationAction::FreshSession | ContinuationAction::ContinueHere => {
                (None, None, true)
            }
            ContinuationAction::SwitchAgent { agent_id } => (None, Some(agent_id.clone()), true),
            ContinuationAction::Retry { turn_id } | ContinuationAction::Edit { turn_id, .. } => {
                (Some(turn_id.clone()), None, false)
            }
            ContinuationAction::Fork { turn_id } => (Some(turn_id.clone()), None, true),
        };
        if selected.as_ref().is_some_and(|id| id.len() > 128)
            || target.as_ref().is_some_and(|id| id.len() > 128)
        {
            return Err(invalid("The continuation identifier is invalid."));
        }
        let snapshot = self
            .conversation_snapshot(chat_id, selected, target.clone(), include_selected)
            .await?;
        let continue_here = matches!(action, ContinuationAction::ContinueHere);
        if continue_here && !snapshot.remote_history {
            return Err(invalid(
                "This chat has no new remote history to continue from.",
            ));
        }
        if let Some(relocation) = &relocation
            && live::normalize_path(snapshot.chat.folder.clone())
                != live::normalize_path(relocation.from.clone())
        {
            return Err(invalid(
                "The chat folder changed while relocation was being reviewed.",
            ));
        }
        if snapshot
            .selected
            .as_ref()
            .is_some_and(|turn| turn.status == "running")
        {
            return Err(invalid(
                "Wait for the selected turn to finish before continuing from it.",
            ));
        }
        let agent_id = target.unwrap_or_else(|| {
            snapshot.selected.as_ref().map_or_else(
                || snapshot.chat.agent_id.clone(),
                |turn| turn.agent_id.clone(),
            )
        });
        // Chats name agents; a saved session names the connection that made it, which decides
        // whether it can fork natively.
        let agent = self
            .deps()
            .detector
            .agent(&agent_id)
            .ok_or_else(|| SessionError::UnknownAgent(agent_id.clone()))?;
        let agent_id = agent_of(&agent_id).to_owned();
        let switch = matches!(action, ContinuationAction::SwitchAgent { .. });
        let creates_chat = matches!(action, ContinuationAction::Fork { .. });
        let session = snapshot.target_session.as_ref().filter(|session| {
            session.host == snapshot.chat.agent_host && session.cwd == snapshot.chat.folder
        });
        let spec = session
            .and_then(|session| self.deps().detector.spec(&session.agent_id))
            .unwrap_or(agent);
        let resumes_native = relocation.is_none()
            && !snapshot.remote_history
            && switch
            && session
                .and_then(|session| session.native_session_id.as_ref())
                .is_some()
            && session
                .and_then(|session| session.capabilities.as_ref())
                .is_some_and(|capabilities| capabilities.resume);
        let mut warnings = Vec::new();
        if matches!(action, ContinuationAction::FreshSession) {
            warnings.push("A new native session will use only the handoff shown here. Earlier messages and saved sessions remain intact. No files are changed and no prompt is sent until your next message.".into());
        }
        if continue_here {
            warnings.push("A fresh native session will receive the reviewed context in your next visible prompt. Saved native sessions remain unchanged.".into());
        }
        if switch
            && !resumes_native
            && snapshot
                .target_session
                .as_ref()
                .and_then(|session| session.native_session_id.as_ref())
                .is_some()
        {
            warnings.push("This saved native session cannot be safely resumed here. A new session will receive the visible handoff.".into());
        }
        let uses_native_fork = creates_chat
            && !snapshot.remote_history
            && session
                .and_then(|session| session.capabilities.as_ref())
                .is_some_and(|capabilities| capabilities.fork)
            && matches!(
                spec.kind,
                o7_protocol::DriverKind::Claude | o7_protocol::DriverKind::Pi
            )
            && session
                .and_then(|session| session.native_session_id.as_ref())
                .is_some()
            && snapshot.selected.as_ref().is_some_and(|turn| {
                turn.ordinal == snapshot.latest_ordinal
                    && snapshot.target_last_turn == Some(turn.ordinal)
            });
        if creates_chat {
            warnings.push("The new chat uses the same working folder. The source conversation remains available.".into());
        }
        if !switch && !continue_here && !matches!(action, ContinuationAction::FreshSession) {
            warnings.push("Continuing from an earlier message does not revert files. Review Changes separately if the folder should be restored.".into());
        }
        let (files, diff_stat) = match self.chat_changes(chat_id, None).await {
            Ok(changes) if changes.checkpoint.is_some() => {
                let stat = if let (Some(service), Some(folder_id), Some(checkpoint)) = (
                    &self.deps().checkpoints,
                    &snapshot.chat.folder_id,
                    &changes.checkpoint,
                ) {
                    match service
                        .diff_stat(chat_id, folder_id, &snapshot.chat.folder, &checkpoint.id)
                        .await
                    {
                        Ok(stat) if stat.is_empty() => {
                            "No changes since the latest checkpoint.".into()
                        }
                        Ok(stat) => format!("Since the latest before-turn checkpoint:\n{stat}"),
                        Err(_) => "Git diff summary is unavailable for this preview.".into(),
                    }
                } else {
                    "Git diff summary is unavailable for this preview.".into()
                };
                let value = changes
                    .files
                    .iter()
                    .take(40)
                    .map(|file| format!("{:?}: {}", file.kind, file.path))
                    .collect::<Vec<_>>()
                    .join("\n");
                let files = if value.is_empty() {
                    "No changes since the latest checkpoint.".into()
                } else {
                    value
                };
                (files, stat)
            }
            _ => (
                "Checkpoint file changes are unavailable.".into(),
                "Git diff summary is unavailable without a saved checkpoint.".into(),
            ),
        };
        let (mut handoff, truncated) = crate::conversation::handoff(
            &snapshot.turns,
            if resumes_native {
                snapshot.target_last_turn
            } else {
                None
            },
            &snapshot.chat.folder.display().to_string(),
            &files,
            &diff_stat,
        );
        if uses_native_fork {
            handoff.clear();
            warnings.push("The new native session forks the complete source session with a distinct ID; no synthetic history is sent.".into());
        }
        if let Some(relocation) = &relocation {
            let header = format!(
                "Working folder changes from {} to {}.\nA new native session continues with this visible context.\n\n",
                relocation.from.display(),
                relocation.to.display()
            );
            if header.len() > 4096 {
                return Err(invalid(
                    "The folder paths exceed the visible handoff limit.",
                ));
            }
            let available = crate::conversation::HANDOFF_BYTES.saturating_sub(header.len());
            if handoff.len() > available {
                warnings.push(
                    "Earlier context was shortened to leave room for the visible folder change."
                        .into(),
                );
            }
            handoff = format!("{header}{}", crate::conversation::cut(&handoff, available));
            warnings.push("Moving the chat preserves all earlier messages and saved native sessions. No files are copied or reverted, and no agent prompt is sent now.".into());
        }
        if truncated || snapshot.truncated {
            warnings
                .push("Earlier context was shortened to fit the visible handoff budget.".into());
        }
        let prompt = match &action {
            ContinuationAction::Retry { .. } => {
                let turn = snapshot
                    .selected
                    .as_ref()
                    .ok_or_else(|| invalid("The selected prompt is unavailable."))?;
                Some((
                    turn.prompt.clone(),
                    if turn.parts.is_empty() {
                        vec![PromptPart::Text {
                            text: turn.prompt.clone(),
                        }]
                    } else {
                        turn.parts.clone()
                    },
                ))
            }
            ContinuationAction::Edit { text, .. } => {
                if text.len() > self.deps().limits.max_prompt_bytes {
                    return Err(invalid("The edited prompt is too large."));
                }
                let turn = snapshot
                    .selected
                    .as_ref()
                    .ok_or_else(|| invalid("The selected prompt is unavailable."))?;
                Some(crate::conversation::edited(turn, text.clone()).ok_or_else(ambiguous_edit)?)
            }
            _ => None,
        };
        if let Some((text, _)) = &prompt
            && (text.trim().is_empty()
                || text.len() + handoff.len() + 32 > self.deps().limits.max_prompt_bytes)
        {
            return Err(invalid(
                "The prompt plus visible handoff is empty or too large. Shorten the edited text.",
            ));
        }
        let token = crate::ids::new_id();
        let expires = Instant::now() + Duration::from_secs(600);
        let preview = ContinuationPreview {
            token: token.clone(),
            source_chat_id: chat_id.into(),
            agent_id: agent_id.clone(),
            creates_chat,
            resumes_native,
            uses_native_fork,
            handoff: handoff.clone(),
            prompt: prompt.as_ref().map(|(text, _)| text.clone()),
            warnings,
            expires_at: crate::time::now_millis() + 600_000,
        };
        let parts = snapshot
            .selected
            .as_ref()
            .map(|turn| turn.parts.clone())
            .unwrap_or_default();
        let apply = ApplyContinuation {
            source_chat: chat_id.into(),
            source_turn: snapshot.selected.as_ref().map(|turn| turn.id.clone()),
            expected_next_seq: snapshot.next_seq,
            expected_updated_at: snapshot.chat.updated_at,
            expected_folder: snapshot.chat.folder.clone(),
            expected_active_agent: snapshot.chat.agent_id.clone(),
            expected_target_session: snapshot
                .target_session
                .as_ref()
                .map(|session| (session.id.clone(), session.native_session_id.clone())),
            target_agent: agent_id,
            destination_chat: creates_chat.then(crate::ids::new_id),
            destination_folder: relocation.as_ref().map(|value| value.to.clone()),
            pending: PendingContinuation {
                handoff,
                force_fresh: !resumes_native && !uses_native_fork,
                resume_source: uses_native_fork
                    .then(|| session.and_then(|session| session.native_session_id.clone()))
                    .flatten(),
                connection: uses_native_fork
                    .then(|| session.map(|session| session.agent_id.clone()))
                    .flatten(),
                native_start: if uses_native_fork {
                    NativeStartOptions {
                        fork: true,
                        new_session_id: Some(crate::ids::new_id()),
                        ..Default::default()
                    }
                } else {
                    NativeStartOptions::default()
                },
            },
            parts,
        };
        let mut prepared = lock(&self.shared().continuations);
        prepared.retain(|_, value| value.expires > Instant::now());
        if prepared.len() >= 32 {
            return Err(invalid(
                "Too many continuation previews are open. Close one before trying again.",
            ));
        }
        prepared.insert(
            token,
            PreparedContinuation {
                expires,
                apply,
                prompt,
                relocation,
            },
        );
        Ok(preview)
    }

    pub async fn continuation_apply(
        &self,
        chat_id: &str,
        token: &str,
    ) -> Result<ContinuationResult, SessionError> {
        let (_organization, _) = self.hold_chat_folder(chat_id, &[]).await?;
        let prepared = lock(&self.shared().continuations)
            .remove(token)
            .ok_or_else(|| {
                invalid("This continuation preview expired or was already used. Review it again.")
            })?;
        if prepared.expires <= Instant::now() || prepared.apply.source_chat != chat_id {
            return Err(invalid(
                "This preview no longer belongs to the selected conversation.",
            ));
        }
        if prepared.relocation.is_some() {
            return Err(invalid(
                "Use the reviewed worktree relocation action for this preview.",
            ));
        }
        self.ask(chat_id, |reply| ChatMsg::Continue {
            apply: Box::new(prepared.apply.clone()),
            prompt: prepared.prompt.clone(),
            reply,
        })
        .await
    }
}

fn ambiguous_edit() -> SessionError {
    invalid(
        "This older prompt does not distinguish attached text from editable text. Retry it unchanged, or send a new prompt with the attachments selected again.",
    )
}
