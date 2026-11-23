//! What a remote session can do on this desktop. The desktop app implements it over the profile
//! that is open; nothing here reaches further than these calls.

use std::sync::Arc;

pub use futures::future::BoxFuture;
use o7_protocol::{
    EventBatch, UiAnswer,
    session::{ChatAttach, ChatStatusUpdate, HistoryPage, PromptAck},
    workspace::WorkspaceChat,
};

use crate::wire::{Look, Overview};

/// Why a call failed, in words the person at the other end can act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct BackendError {
    pub code: &'static str,
    pub message: String,
}

impl BackendError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The request was already answered, withdrawn or never existed.
    pub fn not_pending(&self) -> bool {
        self.code == "request_not_pending"
    }
}

/// Receives a followed chat's live events. `false` means the receiver is gone or fell behind:
/// send it nothing more.
pub type Frames = Arc<dyn Fn(&EventBatch) -> bool + Send + Sync>;
/// Receives every chat's status changes, with the same meaning of `false`.
pub type Statuses = Arc<dyn Fn(&ChatStatusUpdate) -> bool + Send + Sync>;

/// What a full-control bot link may ask beyond following and sending in chats: the rest of what
/// a person does with chats and projects in the app's window. Each is one call the app answers or
/// refuses. Settings, accounts and sign-in, installing or updating software, remote access itself,
/// deleting data, a raw terminal and reading files by path are not here, and no link reaches them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A new project from a folder on the computer.
    CreateProject {
        path: String,
        name: Option<String>,
    },
    /// The models, thinking levels and access modes a chat offers, and its current choices.
    Options {
        chat_id: String,
    },
    SetModel {
        chat_id: String,
        model: String,
    },
    SetThinking {
        chat_id: String,
        level: String,
    },
    /// `ask`, `auto_edit` or `full_auto`.
    SetAccess {
        chat_id: String,
        access: String,
    },
    /// Hands the chat to another agent: its history goes along as the app's own handoff does.
    SwitchAgent {
        chat_id: String,
        agent_id: String,
    },
    Rename {
        chat_id: String,
        title: String,
    },
    Archive {
        chat_id: String,
        archived: bool,
    },
    /// Starts a stopped chat's agent without sending anything.
    StartAgent {
        chat_id: String,
    },
    /// The files a chat's last turn changed.
    Changes {
        chat_id: String,
    },
    /// One changed file, before and after.
    Diff {
        chat_id: String,
        path: String,
    },
    ReadMemory {
        project_id: String,
    },
    WriteMemory {
        project_id: String,
        markdown: String,
    },
}

/// One thing a chat lets a person choose, as the app names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatOptions {
    pub models: Vec<Choice>,
    pub model: Option<String>,
    pub thinking: Vec<Choice>,
    pub thinking_level: Option<String>,
    /// Access modes by the value `set_access` takes.
    pub access: Vec<Choice>,
    pub access_mode: Option<String>,
}

/// The app's answer to an [`Action`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Done, in a sentence for the bot.
    Done(String),
    Project {
        id: String,
        name: String,
        folder_id: Option<String>,
    },
    /// The chat an action left the bot with (a handoff makes a new one).
    Chat {
        id: String,
    },
    Options(ChatOptions),
    Changes {
        /// Path and `added`, `modified` or `deleted`.
        files: Vec<(String, String)>,
        unavailable: Option<String>,
    },
    Diff {
        before: Option<String>,
        after: Option<String>,
        binary: bool,
        truncated: bool,
    },
    Memory {
        markdown: String,
    },
}

/// A chat made for a task: in a Git worktree of its own where the folder is a repository.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskChat {
    pub chat: WorkspaceChat,
    /// The new branch and the worktree's folder, when one was made.
    pub branch: Option<String>,
    pub path: Option<String>,
    /// The commit the branch started from.
    pub base: Option<String>,
}

/// What a chat stands on, for a reading of it: its branch, what is not committed, how much of
/// its agent's usage is gone. Each part is left out where it cannot be read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatFacts {
    pub branch: Option<String>,
    /// The chat works in a worktree of its own.
    pub worktree: bool,
    /// Files changed in the folder and not committed.
    pub uncommitted: Option<usize>,
    /// The agent's usage limits as it last reported them: a window's name and how much is used.
    pub usage: Vec<(String, f64)>,
}

pub trait Backend: Send + Sync + 'static {
    /// One [`Action`] of a full-control link. A backend that offers none refuses them all.
    fn act(&self, _action: Action) -> BoxFuture<'_, Result<Outcome, BackendError>> {
        Box::pin(async {
            Err(BackendError::new(
                "unavailable",
                "This computer's o7 does not offer that yet.",
            ))
        })
    }
    /// This backend for one workspace only: every call but `release` fails unless `profile` is
    /// the open workspace at the moment the call reaches its chats, checked together with taking
    /// them. The service calls nothing but a bound backend, with the workspace a session's key or
    /// a bot's token was accepted under. So however a switch of workspace and the service's own
    /// rescoping interleave, what was accepted for one workspace is never served another's chats.
    fn bound(self: Arc<Self>, profile: &str) -> Arc<dyn Backend>;
    /// How the desktop looks now, for a page to wear the same theme. It carries no chat or
    /// workspace content, so it is read once per session, before the browser has proved anything.
    fn look(&self) -> Option<Look> {
        None
    }
    /// Projects, their chats and the agents installed here.
    fn overview(&self) -> BoxFuture<'_, Result<Overview, BackendError>>;
    /// Sends every chat's status changes to `statuses` until `owner` is released. A second call
    /// for the same owner replaces the first.
    fn watch(&self, owner: &str, statuses: Statuses) -> Result<(), BackendError>;
    /// Follows a chat: its newest history now, then its live events to `frames`. One chat per
    /// owner; following another replaces it.
    fn attach<'a>(
        &'a self,
        chat_id: &'a str,
        owner: &'a str,
        frames: Frames,
    ) -> BoxFuture<'a, Result<ChatAttach, BackendError>>;
    /// Stops everything `owner` follows and watches.
    fn release(&self, owner: &str);
    /// The page of history before `before_seq` (`u32::MAX`: the newest).
    fn history<'a>(
        &'a self,
        chat_id: &'a str,
        before_seq: u32,
    ) -> BoxFuture<'a, Result<HistoryPage, BackendError>>;
    fn send<'a>(
        &'a self,
        chat_id: &'a str,
        text: String,
    ) -> BoxFuture<'a, Result<PromptAck, BackendError>>;
    fn interrupt<'a>(&'a self, chat_id: &'a str) -> BoxFuture<'a, Result<(), BackendError>>;
    /// Answers a permission request; no option declines it.
    fn permission<'a>(
        &'a self,
        chat_id: &'a str,
        request_id: &'a str,
        option_id: Option<String>,
    ) -> BoxFuture<'a, Result<(), BackendError>>;
    /// Answers a question; no answer dismisses it.
    fn answer<'a>(
        &'a self,
        chat_id: &'a str,
        request_id: &'a str,
        answer: Option<UiAnswer>,
    ) -> BoxFuture<'a, Result<(), BackendError>>;
    /// A new chat in a project's folder. No agent named: the project's, then the default.
    fn create<'a>(
        &'a self,
        folder_id: &'a str,
        agent_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<WorkspaceChat, BackendError>>;
    /// A new chat for a task: in a new Git worktree of the folder, on a new branch named from
    /// `title`, so its work touches nothing else; in the folder itself where it is not a
    /// repository. A backend without worktrees makes an ordinary chat.
    fn create_task_chat<'a>(
        &'a self,
        folder_id: &'a str,
        agent_id: Option<&'a str>,
        _title: &'a str,
    ) -> BoxFuture<'a, Result<TaskChat, BackendError>> {
        Box::pin(async move {
            Ok(TaskChat {
                chat: self.create(folder_id, agent_id).await?,
                branch: None,
                path: None,
                base: None,
            })
        })
    }
    /// What `chat_id` stands on (its agent is `agent_id`). A backend that cannot say says nothing.
    fn facts<'a>(
        &'a self,
        _chat_id: &'a str,
        _agent_id: &'a str,
    ) -> BoxFuture<'a, Result<ChatFacts, BackendError>> {
        Box::pin(async { Ok(ChatFacts::default()) })
    }
}
