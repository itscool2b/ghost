//! Reviewable Git workspace operations. A preview never authorizes a later, changed repository.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::Millis;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct GitFileState {
    pub path: String,
    pub index: String,
    pub worktree: String,
    pub original_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct GitWorkspace {
    pub root: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub files: Vec<GitFileState>,
    pub blockers: Vec<String>,
    pub has_submodules: bool,
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct GitInitializePreview {
    pub token: String,
    pub folder: String,
    pub initial_branch: String,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct IsolationInput {
    pub slug: String,
    pub bring_changes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct IsolationPreview {
    pub token: String,
    pub source: GitWorkspace,
    pub branch: String,
    pub path: String,
    pub bring_changes: bool,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

/// One-step request for a new chat whose folder is a new app-owned worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeChatInput {
    /// The project folder the worktree is created from. It must be a repository root.
    pub folder_id: String,
    pub agent_id: String,
    /// A local branch to start from. `None` starts from the source folder's current branch.
    pub start_branch: Option<String>,
    /// The new branch. `None` picks a unique `ghost/<slug>` name. An existing branch is never reused.
    pub branch: Option<String>,
    /// Optional chat title. It also names an automatic branch.
    pub title: Option<String>,
    /// Copy the source's uncommitted working files. Only allowed from the current commit.
    pub bring_changes: bool,
    /// Run the source folder's saved worktree setup after creation.
    pub run_setup: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ChatFolderMode {
    Direct,
    Worktree,
}

/// A compact summary for a chip below the composer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeChatChip {
    pub mode: ChatFolderMode,
    pub starting_branch: Option<String>,
    pub branch: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeChatPlan {
    pub token: String,
    pub source: GitWorkspace,
    pub starting_branch: String,
    pub starting_commit: String,
    pub branch: String,
    /// True when the branch name was generated rather than typed.
    pub branch_generated: bool,
    pub path: String,
    pub bring_changes: bool,
    /// Local branches the starting branch can be chosen from, most recent first (bounded).
    pub local_branches: Vec<String>,
    /// The saved setup recipe that will run after creation, if any.
    pub setup: Option<crate::worktree_setup::WorktreeSetupInput>,
    pub chip: WorktreeChatChip,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorktreeChatPreview {
    Worktree {
        plan: Box<WorktreeChatPlan>,
    },
    /// The folder is not a Git repository. The chat works directly in the folder instead.
    NotGitRepository {
        folder: String,
        message: String,
        chip: WorktreeChatChip,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeChatResult {
    pub chat: crate::workspace::WorkspaceChat,
    pub worktree: ManagedWorktree,
    /// Present when setup ran. A `command_token` starts the reviewed command in the owned
    /// terminal (`worktree_setup_start`); the agent is then started explicitly.
    pub setup: Option<crate::worktree_setup::WorktreeSetupResult>,
    /// True when the agent start was requested.
    pub started: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeRecreatePreview {
    pub token: String,
    pub worktree: ManagedWorktree,
    pub path: String,
    pub branch: String,
    pub commit: String,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeState {
    Creating,
    Ready,
    Merging,
    Retiring,
    Restoring,
    RecoveryRequired,
    Missing,
    Retired,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ManagedWorktree {
    pub id: String,
    pub chat_id: String,
    pub source: String,
    pub path: String,
    pub branch: String,
    pub base_commit: String,
    pub state: WorktreeState,
    pub detail: Option<String>,
    #[serde(default)]
    pub recovery: Option<WorktreeRecovery>,
    pub created_at: Millis,
    /// The first of ten consecutive local ports kept for this worktree's processes
    /// (`GHOST_PORT`), given the first time one is needed. Records from before have none yet.
    #[serde(default)]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorktreeRecovery {
    Create,
    Retire,
    Restore,
    Merge {
        branch: String,
        before_ref: String,
        prepared_ref: String,
        before_commit: String,
        prepared_commit: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeRecoveryAction {
    Reconcile,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeRecoveryPreview {
    pub token: String,
    pub worktree: ManagedWorktree,
    pub action: WorktreeRecoveryAction,
    pub folder: GitWorkspace,
    pub target: Option<GitWorkspace>,
    pub destination: String,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeDestination {
    Isolated,
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeRelocationPreview {
    pub token: String,
    pub worktree: ManagedWorktree,
    pub from: String,
    pub to: String,
    pub handoff: String,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PullRequestInput {
    pub title: String,
    pub body: String,
    pub base_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PullRequestPreview {
    pub token: String,
    pub worktree_id: String,
    pub repository: String,
    pub repository_url: String,
    pub head_branch: String,
    pub head_commit: String,
    pub base_branch: String,
    pub title: String,
    pub body: String,
    pub will_push: bool,
    pub existing_url: Option<String>,
    pub diff_stat: String,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PullRequestResult {
    pub url: String,
    pub created: bool,
    pub head_commit: String,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct IsolationResult {
    pub worktree: ManagedWorktree,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorktreeAction {
    Merge { message: String },
    Discard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeActionPreview {
    pub token: String,
    pub worktree: ManagedWorktree,
    pub action: WorktreeAction,
    pub target: Option<GitWorkspace>,
    pub changes: String,
    pub conflicts: Vec<String>,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeActionResult {
    pub worktree: ManagedWorktree,
    pub recovery: String,
    pub commit: Option<String>,
    pub error: Option<String>,
}

/// Files of the chat's folder to commit, as they are in the folder, and the message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CommitInput {
    pub message: String,
    pub paths: Vec<String>,
}

/// The commit a review describes: exactly these files' current contents on top of `parent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CommitPreview {
    pub token: String,
    pub branch: String,
    /// `None` for a repository's first commit.
    pub parent: Option<String>,
    /// The person's Git identity, as `Name <email>`.
    pub author: String,
    /// Whether the person's Git settings sign commits.
    pub signed: bool,
    /// The files committed; a renamed file brings its old path.
    pub files: Vec<GitFileState>,
    pub message: String,
    pub diff_stat: String,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CommitResult {
    pub commit: String,
    pub branch: String,
    /// Set when the commit was made but something after it was not.
    pub warning: Option<String>,
}

/// A fast-forward push of the folder's branch: the remote, its branch and the commits it gains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PushPreview {
    pub token: String,
    pub branch: String,
    pub remote: String,
    /// Every address the remote pushes to, without any user name or password in them.
    pub remote_urls: Vec<String>,
    pub target: String,
    pub head: String,
    /// The remote branch's commit now, `None` when the push creates it.
    pub remote_head: Option<String>,
    /// The commits the remote gains, newest first, as `abc1234 Subject`, at most twenty.
    pub commits: Vec<String>,
    pub count: u32,
    /// Whether the push makes the remote branch this branch's upstream.
    pub sets_upstream: bool,
    pub warnings: Vec<String>,
    pub expires_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PushResult {
    pub remote: String,
    pub target: String,
    pub head: String,
    pub created: bool,
    pub warning: Option<String>,
}
