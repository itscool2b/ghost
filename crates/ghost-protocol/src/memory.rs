use serde::{Deserialize, Serialize};
use specta::Type;

/// A project's memory: a short brief that every chat in the project receives, and notes that are
/// listed to every chat and read when one fits the task.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectMemory {
    pub project_id: String,
    /// The brief's own revision: what a save of the brief names as its base. `None` while no
    /// brief was ever saved.
    pub revision: Option<String>,
    /// The brief.
    pub markdown: String,
    /// When the brief was last saved.
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
    /// The newest revision of anything in this memory, the brief or a note. A chat is sent memory
    /// again when this moves past the one it last received.
    #[serde(default)]
    pub head: Option<String>,
    /// The notes, by name.
    #[serde(default)]
    pub notes: Vec<MemoryNoteSummary>,
}
/// A note as the list shows it: enough to decide whether to read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryNoteSummary {
    /// Lowercase letters, digits and dashes; a note keeps its name.
    pub name: String,
    /// One line that says when to read it.
    pub description: String,
    /// What a save or a delete of this note names as its base.
    pub revision: String,
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
    /// Who saved it last: `user`, `agent` or `bot`.
    pub by: String,
    pub bytes: u32,
}
/// A note with its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryNote {
    pub project_id: String,
    pub name: String,
    pub description: String,
    pub revision: String,
    pub markdown: String,
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
    pub by: String,
}
/// Saves a note whole. `expected_revision` is the revision that was read, or `None` to add a
/// note that does not exist yet.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveMemoryNote {
    pub project_id: String,
    pub name: String,
    pub expected_revision: Option<String>,
    pub description: String,
    pub markdown: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DeleteMemoryNote {
    pub project_id: String,
    pub name: String,
    pub expected_revision: Option<String>,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryRevision {
    pub revision: String,
    pub parent_revision: Option<String>,
    pub project_id: String,
    pub markdown: String,
    pub reason: String,
    pub source: String,
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
    /// The note this revision is of; `None` for the brief.
    #[serde(default)]
    pub note: Option<String>,
    /// The note's description at this revision.
    #[serde(default)]
    pub description: Option<String>,
    /// This revision removed the note.
    #[serde(default)]
    pub deleted: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MemoryProposal {
    pub id: String,
    pub project_id: String,
    pub base_revision: Option<String>,
    pub markdown: String,
    pub reason: String,
    pub chat_id: Option<String>,
    pub idempotency_key: String,
    pub state: String,
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveProjectMemory {
    pub project_id: String,
    pub expected_revision: Option<String>,
    pub markdown: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ProposeProjectMemory {
    pub project_id: String,
    pub base_revision: Option<String>,
    pub markdown: String,
    pub reason: String,
    pub chat_id: Option<String>,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AcceptMemoryProposal {
    pub project_id: String,
    pub proposal_id: String,
    pub expected_revision: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ProjectMemoryBundle {
    pub schema_version: u32,
    pub project_id: String,
    pub current: ProjectMemory,
    pub history: Vec<MemoryRevision>,
    pub proposals: Vec<MemoryProposal>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ImportProjectMemory {
    pub bundle: ProjectMemoryBundle,
    pub expected_revision: Option<String>,
}
/// An agent's direct edit: the revision it added, the one it replaced, and the chat it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentMemoryEdit {
    pub revision: String,
    pub parent_revision: Option<String>,
    /// The chat whose agent saved it; empty when a linked bot did.
    pub chat_id: String,
    /// The note it saved or removed; `None` when it saved the brief.
    #[serde(default)]
    pub note: Option<String>,
    /// It removed the note.
    #[serde(default)]
    pub deleted: bool,
}
/// What the window hears after a committed change to a project's memory, history or proposals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectMemoryChanged {
    /// Empty when changes were missed: every open project should be read again.
    pub project_id: Option<String>,
    /// Set only when an agent's direct edit added a revision.
    pub agent_edit: Option<AgentMemoryEdit>,
}
