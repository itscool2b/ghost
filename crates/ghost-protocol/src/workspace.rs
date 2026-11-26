//! Project and chat organization. These records do not alter the transcript's wire contract.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::Millis;
use crate::session::{ChatSummary, FolderState};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorkspaceFilter {
    /// Archived chats are excluded unless the caller explicitly asks to include them.
    pub include_archived: bool,
    /// Filters chats only; projects remain available as move destinations.
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorkspaceSnapshot {
    pub projects: Vec<ProjectSummary>,
    pub chats: Vec<WorkspaceChat>,
}

/// Device-local launch context. This is never part of a stored transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChatFolderContext {
    pub primary: String,
    pub additional: Vec<String>,
    pub notices: Vec<String>,
    /// True once these exact folders have been passed to the current driver generation.
    pub active: bool,
    pub supports_additional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub default_agent: Option<String>,
    pub default_model: Option<String>,
    pub folders: Vec<ProjectFolder>,
    pub created_at: Millis,
    pub updated_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectFolder {
    pub id: String,
    pub project_id: String,
    pub display_name: String,
    /// This device's path only. An unavailable device mapping is never borrowed from another.
    pub path: Option<String>,
    pub folder_state: FolderState,
    pub unavailable_reason: Option<String>,
    pub kind: FolderKind,
    pub git_remote_url: Option<String>,
    pub git_default_branch: Option<String>,
    /// When this folder is a worktree Ghost made: the folder it was made from (this device's
    /// path). A new chat never starts in one by itself; it starts from that source.
    #[serde(default)]
    pub worktree_of: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FolderKind {
    Local,
    Git,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorkspaceChat {
    pub chat: ChatSummary,
    pub project_id: String,
    pub folder_id: Option<String>,
    pub pinned: bool,
    pub archived: bool,
    /// The agent, model and thinking level this chat plans and builds with, where chosen.
    #[serde(default)]
    pub roles: ChatRoles,
}

/// An agent, and optionally its model and thinking level, chosen for one kind of work.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RoleModel {
    pub agent: String,
    /// `None` leaves the agent's own choice.
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The kinds of work a chat can name a model for (owner, 19 November 2026: models by role are
/// "a chat by chat thing not a settings thing").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    /// Applied when Plan is turned on in the chat, if the chat is on the role's agent.
    Plan,
    /// Offered as "Accept and build with" when a plan is accepted. In a coordinator's chat, the
    /// workers it starts use it unless it names another agent or model.
    Build,
}

impl ChatRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Build => "build",
        }
    }
}

/// A chat's choice for each kind of work. `None`: nothing is chosen, and nothing changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChatRoles {
    #[serde(default)]
    pub plan: Option<RoleModel>,
    #[serde(default)]
    pub build: Option<RoleModel>,
}

impl ChatRoles {
    pub fn get(&self, role: ChatRole) -> Option<&RoleModel> {
        match role {
            ChatRole::Plan => self.plan.as_ref(),
            ChatRole::Build => self.build.as_ref(),
        }
    }

    pub fn set(&mut self, role: ChatRole, choice: Option<RoleModel>) {
        match role {
            ChatRole::Plan => self.plan = choice,
            ChatRole::Build => self.build = choice,
        }
    }
}

/// Complete editable project values. `None` clears an optional field on update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectInput {
    pub name: String,
    pub color: Option<String>,
    pub default_agent: Option<String>,
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectCreate {
    pub project: ProjectInput,
    pub folder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChatCreatePreview {
    pub folder: ProjectFolder,
    /// Sessions working in this canonical folder (starting, running a turn or background work,
    /// open in a terminal, or just started or finished) or waiting for the person's answer. An
    /// agent idle there does not count: it edits nothing until it is given a message.
    pub busy_chats: Vec<ChatSummary>,
}

/// What a picked folder already is on this device, however the pick spelled it (a link, `..`,
/// letter case where the file system ignores it): Rust compares canonical folders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FolderMatches {
    /// The folder as Ghost stores it: canonical, in its plain spelling.
    pub path: String,
    /// The project folders it already is: the one whose chat was active most recently first, then
    /// the oldest. A folder is in several projects only when it was added to each on purpose.
    pub matches: Vec<FolderMatch>,
    /// When it is a worktree Ghost manages: a new chat starts from its source instead.
    pub worktree: Option<WorktreeOrigin>,
}

/// A project folder that is the picked folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FolderMatch {
    pub folder_id: String,
    pub project_id: String,
    pub project_name: String,
    /// When the folder's most recent chat was last active; none without chats.
    pub last_chat_at: Option<Millis>,
}

/// The branch and source folder of a worktree Ghost manages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WorktreeOrigin {
    pub branch: String,
    pub source: String,
}

/// What a chat in the way of a change to a folder is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BlockerState {
    /// Starting, running a turn or background work, or stopping.
    Working,
    /// Waiting for the person to answer a request or a question.
    Waiting,
    /// Open in its agent's own terminal.
    Terminal,
    /// Ready, with nothing running: stopping it loses nothing.
    Idle,
}

/// A chat in the way of a change to a folder, for the person to see and stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FolderBlocker {
    pub chat_id: String,
    pub title: Option<String>,
    pub agent_name: String,
    /// The name of the chat's project.
    pub project: Option<String>,
    pub state: BlockerState,
}

/// What stands in the way of a change to a folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FolderBlockers {
    pub chats: Vec<FolderBlocker>,
    /// Project terminals (and Run) open in the folder: close them to continue.
    pub terminals: u32,
}

/// Which folders `folder_blockers` looks at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FolderScope {
    /// The chat's folder.
    Chat { chat_id: String },
    /// One project folder.
    Folder { folder_id: String },
    /// Every folder of a project.
    Project { project_id: String },
}

/// What stopping several chats did, by chat id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChatsStopped {
    /// Stopped: their agents' processes are gone.
    pub stopped: Vec<String>,
    /// Left running because they were not idle (stopping idle agents only).
    pub kept: Vec<String>,
    /// Still stopping when the wait ended.
    pub stopping: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectChatCreate {
    pub agent_id: String,
    pub folder_id: String,
    /// Explicit acknowledgment of the busy-folder warning. The server rechecks before creation.
    pub allow_busy_folder: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChatMove {
    pub project_id: String,
    /// Must belong to the destination project. `None` preserves the current primary path by
    /// attaching a matching folder to that project, never by moving files on disk.
    pub folder_id: Option<String>,
    pub allow_busy_folder: bool,
}
