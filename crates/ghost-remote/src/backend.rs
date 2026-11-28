//! What a remote session can do on this desktop. The desktop app implements it over the profile
//! that is open; nothing here reaches further than these calls.

use std::sync::Arc;

pub use futures::future::BoxFuture;
use ghost_protocol::{
    EventBatch, UiAnswer,
    composer::ChatImageSource,
    search::ChatSearchHit,
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
    /// The person's own edit of a project's brief, from a paired device: saved as theirs, not
    /// as a bot's, and whatever the switch for agents' memory edits says. `base_revision` is the
    /// revision the edit was made on: none saves over whatever is there; `Some(None)` saves only
    /// while there is no brief yet, and `Some(Some(revision))` only while the brief is at that
    /// revision (`conflict` otherwise). `reason` is what the brief's history shows for it
    /// ("Edited from iPhone").
    SaveMemory {
        project_id: String,
        markdown: String,
        base_revision: Option<Option<String>>,
        reason: String,
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
    /// A project's brief, read or as saved. `revision` is what a save names as its base; none
    /// while no brief was saved, or from a backend that cannot say.
    Memory {
        markdown: String,
        revision: Option<String>,
    },
}

/// What a chat's own branch changed ([`Backend::branch_changes`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchChanges {
    /// Path and `added`, `modified` or `deleted`, sorted by path.
    pub files: Vec<(String, String)>,
    /// The commit the changes are counted from.
    pub base: Option<String>,
    /// Why there is nothing to list: the chat has no branch of its own, or the computer could
    /// not read it.
    pub unavailable: Option<String>,
}

/// One changed file, before and after. A side that is none did not exist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub before: Option<String>,
    pub after: Option<String>,
    pub binary: bool,
    pub truncated: bool,
}

/// A photo as a chat's message box will keep it ([`Backend::prepare_image`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedImage {
    pub bytes: Vec<u8>,
    /// Its name, with the extension of what it is now.
    pub name: String,
    pub mime: String,
}

/// An image of a chat for a paired device ([`Backend::image`]): its type, its bytes in standard
/// base64, and its size in pixels.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImagePreview {
    pub mime: String,
    pub data: String,
    pub width: u32,
    pub height: u32,
}

/// How a pull request's checks stand, all of them together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrChecks {
    None,
    Pending,
    Passed,
    /// The names of those that failed.
    Failed(Vec<String>),
}

impl PrChecks {
    /// One word, as it is kept: `none`, `pending`, `passed` or `failed`.
    pub fn word(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Passed => "passed",
            Self::Failed(_) => "failed",
        }
    }
}

/// A review or a comment on a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrNote {
    pub author: String,
    /// `comment`, or the review's state (`approved`, `changes requested`, `commented`).
    pub kind: String,
    pub body: String,
    /// Written by the repository's owner, a member or a collaborator: only their words are
    /// passed on, since anyone can comment on a public repository.
    pub trusted: bool,
}

/// The pull request open from a chat's branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    /// `open`, `merged` or `closed`.
    pub state: String,
    pub checks: PrChecks,
    /// Reviews and comments, oldest first.
    pub notes: Vec<PrNote>,
}

/// What a bot may see a picture of (`look`): a chat's browser tab, or its emulator's screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureOf {
    Browser,
    Device,
}

/// A picture for a bot, as MCP sends one: its type and its bytes in base64.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Picture {
    pub mime: String,
    pub data: String,
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
    /// Its pull request, in words (`#12 <address>, open; checks failed: test`).
    pub pull_request: Option<String>,
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
                "This computer's Ghost does not offer that yet.",
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
    /// The owners following `chat_id` now, the one that has followed longest first. A chat takes
    /// only a few followers ([`Backend::attach`] answers `busy` beyond them), and a browser that
    /// went away without closing still counts until the relay notices: the host lets the oldest
    /// of its own go to make room. A backend that does not say has nothing to let go.
    fn followers(&self, _chat_id: &str) -> Vec<String> {
        Vec::new()
    }
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
    /// The pull request open from `chat_id`'s branch, for a task's watcher. A backend that
    /// cannot say has none.
    fn pull_request<'a>(
        &'a self,
        _chat_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<PullRequest>, BackendError>> {
        Box::pin(async { Ok(None) })
    }
    /// What [`Backend::pull_request`] last said of `chat_id`'s branch, without asking GitHub
    /// again: for the iPhone app's Inbox, which is read often. A backend that keeps nothing, or
    /// has not looked lately, has none.
    fn known_pull_request(&self, _chat_id: &str) -> Option<PullRequest> {
        None
    }
    /// The hour on the computer's own clock (0 to 23), for the night's queue. A backend that
    /// cannot say starts nothing at night.
    fn local_hour(&self) -> Option<u8> {
        None
    }
    /// What `chat_id`'s browser tab or device shows now, for a full-control link (`look`). The
    /// chat is one of this workspace's: the caller found it in [`Backend::overview`].
    fn picture<'a>(
        &'a self,
        _chat_id: &'a str,
        _of: PictureOf,
    ) -> BoxFuture<'a, Result<Picture, BackendError>> {
        Box::pin(async {
            Err(BackendError::new(
                "unavailable",
                "Pictures are not available on this computer.",
            ))
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
    /// The open chats whose title, project, agent or completed messages match `query`, best
    /// first, as the window's own search finds them (archived chats left out). The query is
    /// already checked: one line of at most [`crate::wire::MAX_QUERY`] characters. A backend that
    /// cannot search refuses.
    fn search<'a>(
        &'a self,
        _query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<ChatSearchHit>, BackendError>> {
        Box::pin(async {
            Err(BackendError::new(
                "unavailable",
                "Search is not available on this computer.",
            ))
        })
    }
    /// The workspace's archived chats, in any order: what [`Backend::overview`] leaves out. A
    /// backend that cannot list them refuses.
    fn archived(&self) -> BoxFuture<'_, Result<Vec<WorkspaceChat>, BackendError>> {
        Box::pin(async {
            Err(BackendError::new(
                "unavailable",
                "Archived chats are not available on this computer.",
            ))
        })
    }
    /// What [`Backend::picture`] shows, small enough that its base64 is at most `max_bytes`. A
    /// backend that can make it smaller does; one it cannot fit is refused `too_large`.
    fn picture_within<'a>(
        &'a self,
        chat_id: &'a str,
        of: PictureOf,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<Picture, BackendError>> {
        Box::pin(async move {
            let picture = self.picture(chat_id, of).await?;
            if picture.data.len() > max_bytes {
                return Err(BackendError::new(
                    "too_large",
                    "The picture is too large to send to this device.",
                ));
            }
            Ok(picture)
        })
    }
    /// A photo a paired device sent, as `chat_id`'s message box will keep it once
    /// [`Backend::attach_image`] adds it, so a message's photos are measured before anything is
    /// made with them. `bytes` are a PNG, JPEG, WebP or GIF, whole and matching the SHA-256 the
    /// device gave; `mime` is the type it named. A backend that keeps photos as they come
    /// answers them unchanged.
    fn prepare_image<'a>(
        &'a self,
        bytes: Vec<u8>,
        name: &'a str,
        mime: &'a str,
    ) -> BoxFuture<'a, Result<PreparedImage, BackendError>> {
        Box::pin(async move {
            Ok(PreparedImage {
                bytes,
                name: name.to_owned(),
                mime: mime.to_owned(),
            })
        })
    }
    /// Adds a photo [`Backend::prepare_image`] answered to `chat_id`'s message box, and answers
    /// its attachment id for [`Backend::send_input`]. A backend without attachments refuses.
    fn attach_image<'a>(
        &'a self,
        _chat_id: &'a str,
        _bytes: Vec<u8>,
        _name: &'a str,
        _mime: &'a str,
    ) -> BoxFuture<'a, Result<String, BackendError>> {
        Box::pin(async {
            Err(BackendError::new(
                "unavailable",
                "Photos are not available on this computer.",
            ))
        })
    }
    /// Sends `text` with the attachments [`Backend::attach_image`] answered, as the window's
    /// message box does; the text may be empty when attachments go with it. A backend without
    /// attachments refuses.
    fn send_input<'a>(
        &'a self,
        _chat_id: &'a str,
        _text: String,
        _attachments: Vec<String>,
    ) -> BoxFuture<'a, Result<PromptAck, BackendError>> {
        Box::pin(async {
            Err(BackendError::new(
                "unavailable",
                "Photos are not available on this computer.",
            ))
        })
    }
    /// An image of `chat_id`, as the window's preview reads it (`full`: the larger one), its
    /// base64 at most `max_bytes`. The source is one the chat holds; the caller never passes a
    /// file by path. A backend without images refuses.
    fn image<'a>(
        &'a self,
        _chat_id: &'a str,
        _source: ChatImageSource,
        _full: bool,
        _max_bytes: usize,
    ) -> BoxFuture<'a, Result<ImagePreview, BackendError>> {
        Box::pin(async {
            Err(BackendError::new(
                "unavailable",
                "Images are not available on this computer.",
            ))
        })
    }
    /// What `chat_id`'s own branch changed since `base` (the commit its task started from, when
    /// known; otherwise the backend finds the branch's own), at most `max_files` files. A chat
    /// without a branch of its own, or a backend that cannot say, answers why in `unavailable`.
    fn branch_changes<'a>(
        &'a self,
        _chat_id: &'a str,
        _base: Option<&'a str>,
        _max_files: usize,
    ) -> BoxFuture<'a, Result<BranchChanges, BackendError>> {
        Box::pin(async {
            Ok(BranchChanges {
                unavailable: Some("This computer's Ghost cannot read a chat's branch.".into()),
                ..BranchChanges::default()
            })
        })
    }
    /// One file of [`Backend::branch_changes`], before and after, each side at most `max_bytes`.
    /// A path that is not among those changes is `not_found`, and nothing else is read.
    fn branch_diff<'a>(
        &'a self,
        _chat_id: &'a str,
        _base: Option<&'a str>,
        _path: &'a str,
        _max_bytes: usize,
    ) -> BoxFuture<'a, Result<Diff, BackendError>> {
        Box::pin(async {
            Err(BackendError::new(
                "unavailable",
                "This computer's Ghost cannot read a chat's branch.",
            ))
        })
    }
}
