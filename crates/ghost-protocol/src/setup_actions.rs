//! Reviewed setup actions. Paths and arguments are supplied by Rust, never by the renderer.
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentInstallReview {
    pub agent_id: String,
    pub package: String,
    pub version: String,
    pub program: String,
    pub arguments: Vec<String>,
    pub working_directory: String,
    pub revision: String,
    pub timeout_seconds: u32,
    pub notices: Vec<String>,
}

/// What the person typed into the clone form. Rust checks every part; nothing here is trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CloneInput {
    /// The repository address as pasted: an https:// or SSH address.
    pub remote: String,
    /// The folder the new folder goes into, once chosen.
    pub parent: Option<String>,
    /// The new folder's name. Absent or empty means the repository's own name.
    pub name: Option<String>,
}

/// The part of the clone form a problem belongs to, so it can be shown beside that field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CloneField {
    Remote,
    Parent,
    Name,
    Git,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CloneProblem {
    pub field: CloneField,
    pub message: String,
}

/// The answer to a clone form: what was understood, and either a startable review or the first
/// problem in the form. Checking never creates a folder or runs Git.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CloneCheck {
    /// "owner/repository", once the address is understood.
    pub repository: Option<String>,
    /// The new folder's name: the one typed, or the repository's own.
    pub name: Option<String>,
    pub review: Option<CloneReview>,
    pub problem: Option<CloneProblem>,
}

/// One startable clone: the exact Git program and arguments that Start runs, and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CloneReview {
    pub id: String,
    pub remote: String,
    pub repository: String,
    pub parent: String,
    pub destination: String,
    /// The Git executable found on this computer.
    pub program: String,
    pub arguments: Vec<String>,
    /// The command as a person would type it: `git clone --progress -- <address> <folder>`.
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CloneState {
    Running,
    Ready,
    Canceled,
    Failed,
    /// Git is being stopped; the unfinished folder waits until the stop is confirmed.
    CleanupPending,
    /// A prior process stopped before the clone's lifecycle was settled. Its folder is preserved.
    Interrupted,
}

/// A durable clone notice. Invalid journals have no usable destination or remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CloneRecovery {
    pub id: String,
    pub state: CloneState,
    pub repository: Option<String>,
    pub remote: Option<String>,
    pub destination: Option<String>,
    pub detail: String,
    /// A completed clone still has its recorded directory identity and a regular Git directory.
    /// Adding it uses the normal project validation flow, never automatic project creation.
    pub can_add: bool,
    pub can_open: bool,
}

/// Git's own progress report, parsed from `git clone --progress`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CloneProgress {
    /// Git's stage, as Git names it: "Receiving objects", "Resolving deltas".
    pub stage: String,
    pub stage_percent: u8,
    /// The whole clone, weighting Git's stages.
    pub percent: u8,
    /// How much has arrived, as Git prints it: "12.40 MiB".
    pub received: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CloneRecord {
    pub id: String,
    /// Grows with every change, so a watcher can wait for the next one.
    pub revision: u32,
    pub remote: String,
    pub repository: String,
    pub destination: String,
    pub state: CloneState,
    pub detail: String,
    pub progress: Option<CloneProgress>,
    /// Git's last lines that are not progress, kept for a failed clone. Never logged.
    pub output: Vec<String>,
    /// Ghost removed the unfinished folder it created.
    pub removed: bool,
}
