//! Explicit private repository publication. No command stages or commits user files.
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RepositoryPublicationInput {
    /// Empty selects the currently authenticated GitHub user during the explicit review.
    pub owner: String,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RepositoryPublicationDraft {
    pub input: RepositoryPublicationInput,
    pub unfinished: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RepositoryPublicationReview {
    pub token: String,
    pub folder_id: String,
    pub path: String,
    pub repository: String,
    pub url: String,
    pub branch: String,
    pub head_commit: String,
    pub private: bool,
    pub recovering: bool,
    pub will_create: bool,
    pub will_push: bool,
    pub notices: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryPublicationState {
    Complete,
    NeedsReview,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RepositoryPublicationResult {
    pub folder_id: String,
    pub url: String,
    pub repository: String,
    pub head_commit: String,
    pub state: RepositoryPublicationState,
    pub detail: String,
}
