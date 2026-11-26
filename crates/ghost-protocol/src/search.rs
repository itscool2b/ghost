//! Bounded local-profile search results. Snippets are plain text, never HTML.
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct SearchResults {
    pub chats: Vec<ChatSearchHit>,
    pub projects: Vec<ProjectSearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ChatSearchHit {
    pub id: String,
    pub title: String,
    pub project: String,
    pub snippet: Option<String>,
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ProjectSearchHit {
    pub id: String,
    pub name: String,
}
