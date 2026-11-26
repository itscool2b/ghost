//! Local, explicit configuration inspection. No environment lookup, subprocesses or network calls.
//! The host supplies approved roots. Credentials, sessions and vendor-synced directories are not
//! part of an inventory. Content is untrusted data, never executable instructions for this crate.

mod copy;
pub mod declarations;
pub mod extensions;
pub mod instructions;
mod inventory;
mod mcp;
mod mcp_document;
mod safe_fs;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use specta::Type;

pub use copy::PreparedCopy;
pub use inventory::Library;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[serde(rename = "LibraryProvider")]
pub enum Provider {
    Claude,
    Codex,
    Pi,
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[serde(rename = "LibraryScope")]
pub enum Scope {
    User,
    Project,
}

/// Backend-only input. For Project, root is the project folder; for User, the provider's config
/// folder. Shared roots are the `skills` directory itself. Claude user MCP needs a separate,
/// explicit state-file source because `.claude.json` lives outside the usual `.claude` folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub provider: Provider,
    pub scope: Scope,
    pub root: PathBuf,
    pub claude_state_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[serde(rename = "LibraryItemKind")]
pub enum Kind {
    Skill,
    Instructions,
    McpServer,
    Plugin,
    Package,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename = "LibraryItemRef")]
pub struct ItemRef {
    pub source_id: String,
    pub kind: Kind,
    /// A logical key, never a caller-selected filesystem path.
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "LibraryItem")]
pub struct Item {
    pub item: ItemRef,
    pub provider: Provider,
    pub scope: Scope,
    pub label: String,
    pub relative_path: String,
    pub revision: String,
    pub bytes: u32,
    pub copy_blocked: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "LibrarySourceStatus")]
pub struct SourceStatus {
    pub id: String,
    pub provider: Provider,
    pub scope: Scope,
    pub available: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename = "LibraryInventory")]
pub struct Inventory {
    pub sources: Vec<SourceStatus>,
    pub items: Vec<Item>,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "LibraryItemPreview")]
pub struct ItemPreview {
    pub item: Item,
    pub text: String,
    pub files: Vec<String>,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "LibraryCopyFile")]
pub struct CopyFile {
    pub path: String,
    pub before_revision: Option<String>,
    pub after_revision: String,
    pub bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "LibraryCopyPreview")]
pub struct CopyPreview {
    pub id: String,
    pub from: ItemRef,
    pub to_source: String,
    pub files: Vec<CopyFile>,
    pub before_text: Option<String>,
    pub after_text: String,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "LibraryCopyResult")]
pub struct CopyResult {
    pub id: String,
    pub source_id: String,
    pub applied: Vec<String>,
    pub recovery_required: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "LibraryCopyReceipt")]
pub struct CopyReceipt {
    pub id: String,
    pub source_id: String,
    pub kind: Kind,
    pub files: Vec<String>,
    pub state: CopyState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", rename = "LibraryCopyState")]
pub enum CopyState {
    Complete,
    RecoveryRequired,
    RolledBack,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The selected configuration source is unavailable.")]
    Unavailable,
    #[error("The configuration path is unsupported, a link, or outside its approved folder.")]
    UnsafePath,
    #[error("The item exceeds the Library's bounded file or folder limits.")]
    Limit,
    #[error("The selected configuration is malformed or uses an unsupported format.")]
    Format,
    #[error("The item changed after it was displayed. Refresh the Library and try again.")]
    Stale,
    #[error("{0}")]
    Unsupported(String),
    #[error("The configuration file could not be read or written. Check folder permissions.")]
    Io,
}

impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        // OS errors and parser excerpts can include private paths or configuration values.
        Self::Io
    }
}

pub type Result<T> = std::result::Result<T, Error>;
