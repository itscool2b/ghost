//! Explicit composer attachments. Stored identifiers are scoped to a chat by the backend.
use serde::{Deserialize, Serialize};
use specta::Type;

crate::lenient::lenient_enum! {
    /// Pending after a crash or a closed turn is unconfirmed, never proof of delivery.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum SteeringState {
        Pending = "pending",
        Accepted = "accepted",
        Rejected = "rejected",
        Unconfirmed = "unconfirmed",
    }
    fallback = Unconfirmed;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SteeringAck {
    pub message_id: String,
    pub state: SteeringState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ComposerAttachment {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub bytes: u32,
    pub image: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ComposerInput {
    pub text: String,
    pub attachments: Vec<String>,
    pub mentions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ComposerDraft {
    pub input: ComposerInput,
    pub files: Vec<ComposerAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FileMention {
    pub path: String,
    pub directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectFilePreview {
    pub path: String,
    pub bytes: u64,
    pub text: Option<String>,
    pub truncated: bool,
}

/// A persisted user message's image references, scoped to its owning chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatImageContext {
    Turn { turn_id: String },
    Steering { message_id: String },
    Queued { prompt_id: String },
}

/// Every reference, including a stored content hash, is checked against the chat's ownership
/// metadata or confined project folder before the backend reads image bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatImageSource {
    Stored {
        sha256: String,
    },
    Attachment {
        id: String,
    },
    Message {
        context: ChatImageContext,
        index: u32,
    },
    Project {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChatImage {
    pub source: ChatImageSource,
    pub name: String,
}

/// A bounded, decoded and re-encoded PNG. Original metadata and active formats are not exposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChatImagePreview {
    pub png_base64: String,
    pub width: u32,
    pub height: u32,
}
