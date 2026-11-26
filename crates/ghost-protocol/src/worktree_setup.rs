//! Explicit device-local setup. Saving a recipe never executes it.
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeSetupState {
    Awaiting,
    Running,
    Failed,
    Interrupted,
    Succeeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeSetupAttempt {
    pub worktree_id: String,
    pub generation: u32,
    pub state: WorktreeSetupState,
    pub detail: Option<String>,
}

impl WorktreeSetupAttempt {
    pub fn blocks_agent(&self) -> bool {
        self.state != WorktreeSetupState::Succeeded
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeSetupInput {
    pub files: Vec<String>,
    pub command: Option<SetupCommand>,
    /// What Run starts in a worktree of this folder, with `GHOST_PORT` and `PORT` set to the
    /// worktree's first port: a dev server, say. Recipes saved before Run have none.
    #[serde(default)]
    pub run: Option<SetupCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SetupCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeSetup {
    pub source: String,
    pub input: WorktreeSetupInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SetupFile {
    pub path: String,
    pub bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeSetupPreview {
    pub token: String,
    pub worktree_id: String,
    pub source: String,
    pub destination: String,
    pub files: Vec<SetupFile>,
    pub command: Option<SetupCommand>,
    pub warnings: Vec<String>,
    pub expires_at: crate::Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeSetupResult {
    pub copied: Vec<String>,
    pub command_token: Option<String>,
    pub warning: Option<String>,
}

/// A worktree chat's Run: the saved command, if any, and the first of the ten local ports the
/// worktree's processes get (`GHOST_PORT`; Run also sets `PORT`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeRun {
    pub command: Option<SetupCommand>,
    pub port: u16,
    pub folder: String,
}
