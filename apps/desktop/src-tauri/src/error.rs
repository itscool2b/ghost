//! The error every fallible command returns to the UI.
//!
//! A stable `code` is what the UI's logic switches on; `message` is a full sentence it can show as
//! is, matching the style of ghost-core's errors. Every [`SessionError`] maps to exactly one code,
//! with the error's own sentence as the message.

use ghost_core::session::SessionError;
use serde::{Deserialize, Serialize};

/// What went wrong, as a stable code the UI can switch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AppErrorCode {
    /// An argument was outside what the command accepts.
    InvalidArgument,
    /// Too many demo streams are already running.
    TooManyStreams,
    /// The main window does not exist (it was closed, or creating it failed).
    WindowUnavailable,
    /// A command that only works in smoke mode (`GHOST_SMOKE`) was called outside it.
    NotInSmokeMode,
    /// Startup could not determine a data folder at all, so there is nothing to show.
    DataDirUnknown,
    /// The system file manager could not show the data folder.
    RevealFailed,
    /// The database repair notice could not be dismissed: the database is closed, or the record of
    /// the repair could not be removed. The notice stays, and shows again on the next launch.
    AcknowledgeFailed,
    /// No chat has this id.
    ChatNotFound,
    /// No agent has this id.
    UnknownAgent,
    /// The agent is missing or broken here; the message says why.
    AgentUnavailable,
    /// The folder can't be used for a chat; the message says why.
    InvalidFolder,
    /// The message is empty.
    PromptEmpty,
    /// The message is longer than the limit.
    PromptTooLarge,
    /// The chat's queue is full.
    QueueFull,
    /// The permission request or question was already answered, withdrawn or closed.
    RequestNotPending,
    /// The option, its value, or the answer is not one the request offers.
    OptionInvalid,
    /// The agent isn't running in the chat.
    SessionNotReady,
    /// The most agents that may run at once already run and none is idle to make room; the
    /// message says what they are doing.
    TooManyAgents,
    /// Chats or project terminals are in the way of a change to a folder: `blockers` names them,
    /// so the page can offer to stop them and try again.
    FolderBusy,
    /// Too many views are open for the chat, or the webview.
    TooManySubscriptions,
    /// The profile database can't be reached right now.
    StorageUnavailable,
    /// The full output is not available: no longer kept, or not saved yet while the disk is full.
    /// The message says which.
    BlobMissing,
    /// Ghost is quitting.
    ShuttingDown,
    /// Something went wrong inside Ghost.
    Internal,
    /// The folder picker could not be shown.
    DialogFailed,
}

/// A command failure, serialized as `{ code, message }`, and `blockers` for `folder_busy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, thiserror::Error)]
#[error("{message}")]
pub struct AppError {
    pub code: AppErrorCode,
    pub message: String,
    /// What stands in the way of a folder change (`folder_busy` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blockers: Option<ghost_protocol::workspace::FolderBlockers>,
}

impl AppError {
    pub fn new(code: AppErrorCode, message: impl Into<String>) -> AppError {
        AppError {
            code,
            message: message.into(),
            blockers: None,
        }
    }

    pub fn invalid_argument(message: impl Into<String>) -> AppError {
        AppError::new(AppErrorCode::InvalidArgument, message)
    }
}

impl From<SessionError> for AppError {
    fn from(error: SessionError) -> AppError {
        if let SessionError::FolderBusy { message, blockers } = error {
            return AppError {
                code: AppErrorCode::FolderBusy,
                message,
                blockers: Some(blockers),
            };
        }
        let code = match &error {
            SessionError::Organization(_) => AppErrorCode::InvalidArgument,
            SessionError::FolderBusy { .. } => AppErrorCode::FolderBusy,
            SessionError::ChatNotFound(_) => AppErrorCode::ChatNotFound,
            SessionError::UnknownAgent(_) => AppErrorCode::UnknownAgent,
            SessionError::AgentUnavailable(_) => AppErrorCode::AgentUnavailable,
            SessionError::InvalidFolder { .. } => AppErrorCode::InvalidFolder,
            SessionError::PromptEmpty => AppErrorCode::PromptEmpty,
            SessionError::PromptTooLarge { .. } => AppErrorCode::PromptTooLarge,
            SessionError::QueueFull(_) => AppErrorCode::QueueFull,
            SessionError::RequestNotPending => AppErrorCode::RequestNotPending,
            SessionError::OptionInvalid(_) => AppErrorCode::OptionInvalid,
            SessionError::SessionNotReady => AppErrorCode::SessionNotReady,
            SessionError::TooManyAgents { .. } => AppErrorCode::TooManyAgents,
            SessionError::TooManySubscriptions => AppErrorCode::TooManySubscriptions,
            SessionError::StorageUnavailable(_) => AppErrorCode::StorageUnavailable,
            SessionError::BlobMissing(_) => AppErrorCode::BlobMissing,
            SessionError::ShuttingDown => AppErrorCode::ShuttingDown,
            SessionError::Internal(_) => AppErrorCode::Internal,
        };
        AppError::new(code, error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn serializes_as_code_and_message() {
        let error = AppError::new(AppErrorCode::TooManyStreams, "Too many.");
        assert_eq!(
            serde_json::to_value(&error).expect("serialize"),
            serde_json::json!({ "code": "too_many_streams", "message": "Too many." })
        );
        assert_eq!(error.to_string(), "Too many.");
        assert_eq!(
            serde_json::to_value(AppErrorCode::AcknowledgeFailed).expect("serialize"),
            "acknowledge_failed"
        );
    }

    /// Every session error variant, each mapped to its own code with its own sentence.
    #[test]
    fn every_session_error_maps_to_one_code() {
        let cases = [
            (SessionError::ChatNotFound("c".into()), "chat_not_found"),
            (SessionError::UnknownAgent("x".into()), "unknown_agent"),
            (
                SessionError::AgentUnavailable("Node.js 22 or newer is required.".into()),
                "agent_unavailable",
            ),
            (
                SessionError::InvalidFolder {
                    path: "/gone".into(),
                    reason: "it does not exist".into(),
                },
                "invalid_folder",
            ),
            (SessionError::PromptEmpty, "prompt_empty"),
            (
                SessionError::PromptTooLarge { bytes: 2, limit: 1 },
                "prompt_too_large",
            ),
            (SessionError::QueueFull(20), "queue_full"),
            (SessionError::RequestNotPending, "request_not_pending"),
            (SessionError::OptionInvalid("no".into()), "option_invalid"),
            (SessionError::SessionNotReady, "session_not_ready"),
            (
                SessionError::TooManyAgents {
                    limit: 16,
                    message: "16 agents are running and none is idle: 16 working. Stop one, then try again.".into(),
                },
                "too_many_agents",
            ),
            (SessionError::TooManySubscriptions, "too_many_subscriptions"),
            (
                SessionError::FolderBusy {
                    message: "\u{201c}Fix tests\u{201d} in Ghost is working in this folder. Stop it, then try again.".into(),
                    blockers: Default::default(),
                },
                "folder_busy",
            ),
            (
                SessionError::StorageUnavailable("busy".into()),
                "storage_unavailable",
            ),
            (
                SessionError::BlobMissing("The full output is no longer available.".into()),
                "blob_missing",
            ),
            (SessionError::ShuttingDown, "shutting_down"),
            (SessionError::Internal("boom".into()), "internal"),
        ];
        let mut codes = HashSet::new();
        for (error, code) in cases {
            let sentence = error.to_string();
            let app = AppError::from(error);
            assert_eq!(
                serde_json::to_value(app.code).expect("serialize"),
                serde_json::json!(code)
            );
            assert_eq!(app.message, sentence);
            assert!(codes.insert(code), "{code} is used twice");
        }
        assert_eq!(codes.len(), 17, "one case per variant");
    }
}
