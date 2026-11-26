//! Reviewed metadata removal. Files and retained conversation history are never deleted.
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceRemovalTarget {
    Project { id: String },
    Folder { id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRemovalAction {
    Remove,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemovedWorkspace {
    pub target: WorkspaceRemovalTarget,
    pub name: String,
    pub project_name: String,
    pub chat_count: u32,
    pub removed_at: crate::Millis,
    /// A folder in a removed project is restored after its project is restored.
    pub restore_unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorkspaceRemovalPreview {
    pub token: String,
    pub target: WorkspaceRemovalTarget,
    pub action: WorkspaceRemovalAction,
    pub name: String,
    pub chat_count: u32,
    pub folders: Vec<String>,
    pub notices: Vec<String>,
}
