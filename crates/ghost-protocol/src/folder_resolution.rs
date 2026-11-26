//! Device-local folder discovery and reviewed mapping.
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FolderResolution {
    pub folder: crate::workspace::ProjectFolder,
    pub roots: Vec<String>,
    pub matches: Vec<String>,
    pub truncated: bool,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FolderLocationReview {
    pub folder_id: String,
    pub path: String,
    pub revision: String,
    pub notices: Vec<String>,
}
