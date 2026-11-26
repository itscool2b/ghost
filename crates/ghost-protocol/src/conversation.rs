//! Explicit history continuations and visible context transfer. Previews never call an agent.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{Millis, session::PromptAck};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ConversationTurn {
    pub turn_id: String,
    pub ordinal: u32,
    pub agent_id: String,
    pub status: String,
    pub prompt_preview: String,
    pub started_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ConversationTurns {
    /// Newest first, up to 50 turns. The full source transcript remains unchanged.
    pub turns: Vec<ConversationTurn>,
    pub before_ordinal: Option<u32>,
    pub has_more: bool,
    /// New remote history requires a reviewed visible handoff before a local agent can continue.
    #[serde(default)]
    pub can_continue_here: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct EditablePrompt {
    pub turn_id: String,
    /// Original free text only. Existing attachment and mention context is retained on edit.
    pub text: String,
    pub has_context: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContinuationAction {
    // Rebuild visible context in a fresh session when native history cannot be resumed.
    FreshSession,
    ContinueHere,
    SwitchAgent { agent_id: String },
    Retry { turn_id: String },
    Edit { turn_id: String, text: String },
    Fork { turn_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ContinuationPreview {
    pub token: String,
    pub source_chat_id: String,
    pub agent_id: String,
    pub creates_chat: bool,
    pub resumes_native: bool,
    pub uses_native_fork: bool,
    /// Exact bounded context shown before confirmation and included in the next visible prompt.
    pub handoff: String,
    /// Retry/edit prompt, including the visible labels/text for retained attachments and mentions.
    /// Switch/fork waits for the next user prompt and therefore has no prompt here.
    pub prompt: Option<String>,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ContinuationResult {
    pub chat_id: String,
    pub prompt: Option<PromptAck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PendingHandoff {
    pub agent_id: String,
    pub handoff: String,
    pub uses_native_fork: bool,
}
