//! What travels between this desktop, the relay and a browser or Ghost for iPhone.
//!
//! The relay speaks one-letter text frames (`apps/relay/src/index.ts`): `o<cid>` a browser
//! connected, `k<cid>` that connection came in on the iPhone app's path (sent right after its
//! `o`), `c<cid>` it left (or, sent by the desktop, drop it), `m<cid> <payload>` its message,
//! `a<cid>` the desktop accepted it, `b<json>` a bot's tool call or its answer, `l<json>` the
//! desktop's bot links, `x` this socket was replaced. A relay that sends notifications says
//! `f{"push":1}` as soon as it accepted the desktop's socket; only then does the desktop send
//! `p<json>`, a notification for Apple, which the relay answers `P<json>`. An older relay ends
//! the desktop's socket on any frame it does not know, so `p` never goes where `f` did not come.
//!
//! A browser's payload is one letter and a body: `H` its hello, `W` the desktop's welcome, `R` a
//! refusal, all three in the clear and carrying no secret; and `E`, a message sealed by
//! [`crate::crypto::Channel`]. Sealed messages are JSON: a request (`id`, `op`), its reply (`re`),
//! or an event (`ev`). The iPhone app speaks the same, and is offered more ([`APP_FEATURES`]).

use ghost_protocol::{
    EventBatch, PermissionOption, UiAnswer, UiPrompt,
    session::{ChatAttach, ChatStatusUpdate, HistoryPage, PromptAck},
    workspace::{ProjectSummary, WorkspaceChat},
};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
/// The length of the id the relay gives each browser connection.
pub const CID_LENGTH: usize = 16;

/// A frame from the relay.
#[derive(Debug, PartialEq, Eq)]
pub enum FromRelay<'a> {
    Opened(&'a str),
    /// The connection just opened came in on the iPhone app's path, which no page can open.
    Native(&'a str),
    Closed(&'a str),
    Message {
        cid: &'a str,
        payload: &'a str,
    },
    Bot(&'a str),
    /// What the relay does beyond forwarding, said once it accepted this socket (`f`):
    /// `{"push":1}` when it sends notifications.
    Features(&'a str),
    /// How a notification this desktop asked the relay to send went (`P`).
    Pushed(&'a str),
    Replaced,
    Pong,
}

pub fn from_relay(frame: &str) -> Option<FromRelay<'_>> {
    if frame == "pong" {
        return Some(FromRelay::Pong);
    }
    if frame == "x" {
        return Some(FromRelay::Replaced);
    }
    let (kind, rest) = frame.split_at_checked(1)?;
    let cid = || {
        let cid = rest.get(..CID_LENGTH)?;
        cid.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            .then_some(cid)
    };
    match kind {
        "o" if rest.len() == CID_LENGTH => Some(FromRelay::Opened(cid()?)),
        "k" if rest.len() == CID_LENGTH => Some(FromRelay::Native(cid()?)),
        "c" if rest.len() == CID_LENGTH => Some(FromRelay::Closed(cid()?)),
        "m" => {
            let payload = rest.get(CID_LENGTH..)?.strip_prefix(' ')?;
            Some(FromRelay::Message {
                cid: cid()?,
                payload,
            })
        }
        "b" => Some(FromRelay::Bot(rest)),
        "f" => Some(FromRelay::Features(rest)),
        "P" => Some(FromRelay::Pushed(rest)),
        _ => None,
    }
}

/// What the relay said it does (`f`): whether it sends notifications. Anything it does not say,
/// or says in a way this build does not read, is no.
pub fn relay_pushes(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body).is_ok_and(|said| {
        said["push"] == serde_json::json!(1) || said["push"] == serde_json::json!(true)
    })
}

/// How a notification went, as the relay answers a `p` (`P`). `id` is the desktop's own for it;
/// `drop` says Apple no longer delivers to that address. `status` is Apple's answer, and
/// `reason` Apple's or the relay's own (`invalid`, `off`, `limited`, `unreachable`).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PushAnswer {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub drop: bool,
}

/// A notification for the relay to give Apple (`p`). The relay checks every part and builds
/// what Apple shows itself, in fixed words: the desktop gives only where it goes and the sealed
/// blob.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PushFrame {
    /// The desktop's own id for it, said back in the answer.
    pub id: String,
    /// The phone's address at Apple, in hex.
    pub to: String,
    /// `sandbox` or `production`.
    pub env: &'static str,
    /// `alert` or `liveactivity`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// An alert's collapse id: a newer one with the same id replaces it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collapse: Option<String>,
    /// The device it was sealed for.
    pub d: String,
    /// The sealed payload ([`crate::push`]).
    pub b: String,
    /// How long Apple keeps trying, in seconds.
    pub ttl: u32,
    /// An alert's sound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<bool>,
    /// A Live Activity update's priority: 10 when something newly needs the person, else 5.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    /// A Live Activity's `update` or `end`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<&'static str>,
}

/// A browser's first message: which credential it holds and its half of the session nonce.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Hello {
    pub v: u32,
    pub kind: CredentialKind,
    pub id: String,
    pub nonce: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    /// A pairing link, good once.
    Pair,
    /// A paired device.
    Device,
}

impl CredentialKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pair => "pair",
            Self::Device => "device",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Welcome {
    pub v: u32,
    pub nonce: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The link was used, expired or replaced, or the device was removed.
    Unknown,
    /// This build does not speak the browser's version.
    Version,
    /// Too many browsers are connected. Not said since 28 November 2026 (a full list lets one
    /// go instead); a page may still hear it from a computer with an older Ghost.
    Busy,
    /// The device is paired with another workspace of this computer than the one open now.
    Elsewhere,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Refused {
    pub reason: Refusal,
}

/// A sealed request from a browser.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: u32,
    #[serde(flatten)]
    pub op: Op,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Turns a pairing link into a device. The only request a pairing session may make.
    /// `client` is what the device says it is (Ghost for iPhone names itself); a browser says
    /// nothing.
    Pair {
        name: String,
        #[serde(default)]
        client: Option<Client>,
    },
    /// Projects, chats and agents.
    Overview,
    /// Follows one chat: its recent history now, then its live events. Replaces the chat this
    /// connection followed before.
    Open {
        chat_id: String,
    },
    /// Stops following.
    Close,
    /// The page of history before `before_seq`.
    History {
        chat_id: String,
        before_seq: u32,
    },
    Send {
        chat_id: String,
        text: String,
        /// Photos staged with [`Op::UploadBegin`], sent with the message (at most
        /// [`MAX_ATTACHMENTS`]). Honored only for a session offered [`ATTACHMENTS`]; with any,
        /// the text may be empty.
        #[serde(default)]
        attachments: Vec<String>,
    },
    Interrupt {
        chat_id: String,
    },
    Permission {
        chat_id: String,
        request_id: String,
        option_id: Option<String>,
    },
    Answer {
        chat_id: String,
        request_id: String,
        answer: Option<UiAnswer>,
    },
    /// A new chat in a project folder, with its first message.
    Create {
        folder_id: String,
        agent_id: Option<String>,
        text: String,
        /// As for [`Op::Send`].
        #[serde(default)]
        attachments: Vec<String>,
    },
    /// The models and thinking levels a chat offers, and what it is set to.
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
    /// Hands the chat to another agent, as the app's own handoff does. Answers with the chat to
    /// open next.
    SwitchAgent {
        chat_id: String,
        agent_id: String,
    },
    /// This browser forgets the desktop: its device is removed.
    Forget,
    /// What needs the person across the open chats, in one reading: the requests chats wait
    /// on, the chats working and the chats that finished in the last day ([`Inbox`]).
    Inbox,
    /// The chats whose title, project, agent or messages match `query` (1 to
    /// [`MAX_QUERY`] characters), at most `limit` of them ([`MAX_HITS`]).
    Search {
        query: String,
        limit: Option<u32>,
    },
    /// Archived chats, newest first: at most `limit` (1 to [`MAX_ARCHIVED`], 50 when unsaid) of
    /// those that last changed before `before` (Unix milliseconds; none: the newest).
    Archived {
        before: Option<i64>,
        limit: Option<u32>,
    },
    /// Sets a chat's access level: exactly `ask`, `auto_edit` or `full_auto`.
    SetAccess {
        chat_id: String,
        access: String,
    },
    /// Renames a chat: the title trimmed, 1 to [`MAX_TITLE`] characters on one line.
    Rename {
        chat_id: String,
        title: String,
    },
    /// Archives a chat, or brings it back.
    Archive {
        chat_id: String,
        archived: bool,
    },
    /// Starts a stopped chat's agent without sending anything.
    StartAgent {
        chat_id: String,
    },
    /// The files a chat changed: in its last turn (`scope` `turn`, the default), or on a branch
    /// of its own since that branch began (`branch`).
    Changes {
        chat_id: String,
        scope: Option<String>,
    },
    /// One of those files, before and after.
    Diff {
        chat_id: String,
        path: String,
        scope: Option<String>,
    },
    /// What a chat stands on: its branch, what is not committed, its agent's usage and its
    /// pull request.
    Facts {
        chat_id: String,
    },
    /// A project's brief and its revision.
    Memory {
        project_id: String,
    },
    /// Saves a project's brief as the person's own edit. With `base_revision`, only while the
    /// brief is still at that revision.
    SetMemory {
        project_id: String,
        markdown: String,
        base_revision: Option<String>,
    },
    /// A picture of what a chat's browser tab (`of` `browser`) or emulator (`device`) shows.
    Picture {
        chat_id: String,
        of: String,
    },
    /// Hands Ghost a task: a chat on a branch of its own, then a second agent's review. `when` is
    /// `now` (the default) or `tonight`.
    Task {
        goal: String,
        project_id: Option<String>,
        agent_id: Option<String>,
        review: Option<bool>,
        when: Option<String>,
        access: Option<String>,
        #[serde(default)]
        attachments: Vec<String>,
    },
    /// The tasks handed over lately, the queue for tonight and the night's limits.
    Tasks,
    /// Takes a task off the queue for tonight, whoever queued it.
    TaskCancel {
        queued_id: String,
    },
    /// Starts staging a photo for a message: its name, type (`image/png`, `image/jpeg`,
    /// `image/webp` or `image/gif`), size in bytes and SHA-256 in hex.
    UploadBegin {
        name: String,
        mime: String,
        size: u64,
        sha256: String,
    },
    /// The next piece of a staged photo: `index` counts from 0, `data` is standard base64 of at
    /// most [`UPLOAD_CHUNK`] bytes.
    UploadChunk {
        upload_id: String,
        index: u32,
        data: String,
    },
    /// Drops a staged photo.
    UploadAbort {
        upload_id: String,
    },
    /// An image of a chat: one it stored (`{"type":"stored","sha256"}`), an attachment of its
    /// message box (`{"type":"attachment","id"}`) or one a message carried
    /// (`{"type":"message","context","index"}`). Read as the window's own preview reads it.
    Image {
        chat_id: String,
        source: serde_json::Value,
        #[serde(default)]
        full: bool,
    },
    /// Where Apple delivers this device's notifications: `token` (64 to 200 hex characters),
    /// `env` (`sandbox` or `production`) and what it wants told (`prefs`, everything when
    /// unsaid). Answers whether the relay in front of this computer sends notifications.
    PushRegister {
        token: String,
        env: String,
        #[serde(default)]
        prefs: Option<PushPrefs>,
    },
    /// This device is told nothing more.
    PushUnregister,
    /// A notification now, to see that they arrive: at most one every ten seconds.
    PushTest,
    /// Where Apple delivers the updates of this computer's Live Activity on this device.
    Activity {
        token: String,
        env: String,
    },
}

fn yes() -> bool {
    true
}

/// What a device wants to be told ([`Op::PushRegister`]): each kind on unless it says otherwise,
/// of `scope` `all` chats (the default) or only `mine`, those it started, sent to or answered.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PushPrefs {
    #[serde(default = "yes")]
    pub asking: bool,
    #[serde(default = "yes")]
    pub finished: bool,
    #[serde(default = "yes")]
    pub failed: bool,
    #[serde(default = "yes")]
    pub tasks: bool,
    #[serde(default)]
    pub scope: Option<String>,
}

/// What a device says about itself when it pairs. It is only its word: the computer records Ghost
/// for iPhone only from a connection the relay marked as the app's ([`FromRelay::Native`]).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Client {
    /// `ios` from Ghost for iPhone.
    #[serde(default)]
    pub kind: Option<String>,
    /// The app's version, kept with the device (at most
    /// [`crate::registry::MAX_CLIENT_FIELD`] characters).
    #[serde(default)]
    pub version: Option<String>,
    /// The device's model, such as `iPhone17,1`. Read, and not kept yet.
    #[serde(default)]
    pub model: Option<String>,
}

impl Client {
    /// Whether it says it is Ghost for iPhone.
    pub fn ios(&self) -> bool {
        self.kind.as_deref() == Some("ios")
    }
}

/// A chat's model, thinking level and agent: [`Op::Options`] and the three requests after it.
pub const PICKERS: &str = "pickers";
/// [`Op::Inbox`].
pub const INBOX: &str = "inbox";
/// [`Op::Search`].
pub const SEARCH: &str = "search";
/// A chat's access level, its title, archiving it and the archived chats, starting its agent:
/// [`Op::Archived`], [`Op::SetAccess`], [`Op::Rename`], [`Op::Archive`], [`Op::StartAgent`], and
/// the access levels in [`Op::Options`]' answer.
pub const CONTROL: &str = "control";
/// What a chat changed and what it stands on: [`Op::Changes`], [`Op::Diff`], [`Op::Facts`].
pub const REVIEW: &str = "review";
/// A project's brief: [`Op::Memory`], [`Op::SetMemory`].
pub const MEMORY: &str = "memory";
/// What a chat's browser tab or emulator shows: [`Op::Picture`].
pub const SCREENS: &str = "screens";
/// Tasks handed over from the phone, and the queue for tonight: [`Op::Task`], [`Op::Tasks`],
/// [`Op::TaskCancel`].
pub const TASKS: &str = "tasks";
/// Photos in a message, and a chat's images: [`Op::UploadBegin`], [`Op::UploadChunk`],
/// [`Op::UploadAbort`], [`Op::Image`], and `attachments` in [`Op::Send`], [`Op::Create`] and
/// [`Op::Task`].
pub const ATTACHMENTS: &str = "attachments";
/// Notifications through Apple, sealed for the phone: [`Op::PushRegister`],
/// [`Op::PushUnregister`], [`Op::PushTest`].
pub const PUSH: &str = "push";
/// The Live Activity's updates through Apple: [`Op::Activity`].
pub const ACTIVITY: &str = "activity";

/// What this build offers a paired browser beyond the first version's requests, named in
/// [`Event::Ready`] so a page shows only the controls this desktop answers. A request the build
/// does not know ends the session, as anything outside the protocol does; one it knows but did
/// not offer is answered `not_offered`. A browser's list stays as it is, so what it is sent
/// does not change.
pub const FEATURES: &[&str] = &[PICKERS];
/// What Ghost for iPhone is offered: a session on the app's path (the relay's `k`) with a pairing
/// link, or with a device recorded as the app. Each later part of the app adds its name here and
/// the requests that need it to [`Op::feature`].
pub const APP_FEATURES: &[&str] = &[
    PICKERS,
    INBOX,
    SEARCH,
    CONTROL,
    REVIEW,
    MEMORY,
    SCREENS,
    TASKS,
    ATTACHMENTS,
    PUSH,
    ACTIVITY,
];

/// The longest search, in characters, and the most hits one returns.
pub const MAX_QUERY: usize = 200;
pub const MAX_HITS: usize = 50;
/// How many hits a search returns when it names no limit.
pub const DEFAULT_HITS: usize = 20;
/// The most archived chats one page lists, and how many when a request names no limit.
pub const MAX_ARCHIVED: usize = 100;
pub const DEFAULT_ARCHIVED: usize = 50;
/// The longest chat title, in characters.
pub const MAX_TITLE: usize = 200;
/// The most files a list of changes names.
pub const MAX_FILES: usize = 2_000;
/// The most of each side of a changed file sent, in bytes.
pub const MAX_SIDE: usize = 160 * 1024;
/// The longest brief, in bytes: every chat of the project receives it.
pub const MAX_BRIEF: usize = 4 * 1024;
/// The most a picture or an image takes in its answer, in bytes, its base64 and all.
pub const MAX_PICTURE: usize = 420 * 1024;
/// The size of one piece of a staged photo, in bytes before base64.
pub const UPLOAD_CHUNK: usize = 48 * 1024;
/// The most photos one message carries, and the most they take together as the chat keeps
/// them: what an agent's message may carry.
pub const MAX_ATTACHMENTS: usize = 8;
pub const MAX_PHOTOS: usize = 8 * 1024 * 1024;
/// The most reviews and comments of a pull request sent, and the most characters of each.
pub const MAX_NOTES: usize = 20;
pub const MAX_NOTE: usize = 2_000;
/// How many of the tasks handed over lately are listed.
pub const MAX_TASKS: usize = 40;

impl Op {
    /// The feature a request needs, or `None` for the first version's requests, which every
    /// session may make. A session is answered `not_offered` for a feature it was not offered.
    pub fn feature(&self) -> Option<&'static str> {
        match self {
            Self::Options { .. }
            | Self::SetModel { .. }
            | Self::SetThinking { .. }
            | Self::SwitchAgent { .. } => Some(PICKERS),
            Self::Inbox => Some(INBOX),
            Self::Search { .. } => Some(SEARCH),
            Self::Archived { .. }
            | Self::SetAccess { .. }
            | Self::Rename { .. }
            | Self::Archive { .. }
            | Self::StartAgent { .. } => Some(CONTROL),
            Self::Changes { .. } | Self::Diff { .. } | Self::Facts { .. } => Some(REVIEW),
            Self::Memory { .. } | Self::SetMemory { .. } => Some(MEMORY),
            Self::Picture { .. } => Some(SCREENS),
            Self::Task { .. } | Self::Tasks | Self::TaskCancel { .. } => Some(TASKS),
            Self::UploadBegin { .. }
            | Self::UploadChunk { .. }
            | Self::UploadAbort { .. }
            | Self::Image { .. } => Some(ATTACHMENTS),
            Self::PushRegister { .. } | Self::PushUnregister | Self::PushTest => Some(PUSH),
            Self::Activity { .. } => Some(ACTIVITY),
            Self::Pair { .. }
            | Self::Overview
            | Self::Open { .. }
            | Self::Close
            | Self::History { .. }
            | Self::Send { .. }
            | Self::Interrupt { .. }
            | Self::Permission { .. }
            | Self::Answer { .. }
            | Self::Create { .. }
            | Self::Forget => None,
        }
    }
}

/// How the desktop looks, so a paired page wears the same theme: the theme's saved name (one of
/// `ghost_core::onboarding::ThemePreference`'s, such as `claude` or `oh_my_pi`) and the
/// appearance (`system`, `light` or `dark`). A page checks both against the names it knows and
/// falls back to its own default for anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Look {
    pub theme: String,
    pub appearance: String,
}

/// What an [`Op`] returns.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Data {
    None {},
    Paired(Paired),
    Overview(Box<Overview>),
    Attach(Box<ChatAttach>),
    History(Box<HistoryPage>),
    Sent(PromptAck),
    Created {
        chat: Box<WorkspaceChat>,
        sent: PromptAck,
    },
    /// The chat was made, and its first message could not be sent: why, in words. Only for a
    /// session offered [`ATTACHMENTS`]; a browser is answered with the error, as before.
    Unsent {
        chat: Box<WorkspaceChat>,
        unsent: String,
    },
    Options(Choices),
    /// The chat a handoff to another agent left the person with.
    Switched {
        chat_id: String,
    },
    Inbox(Box<Inbox>),
    Search(SearchHits),
    Archived(Box<ArchivedChats>),
    Changes(Box<ChangedFiles>),
    Diff(Box<FileDiff>),
    Facts(Box<Facts>),
    Memory(Brief),
    /// The brief's revision once saved.
    SavedMemory {
        revision: Option<String>,
    },
    Picture(crate::backend::Picture),
    Started(TaskStarted),
    /// The task waits for tonight under this id (`q` and a few letters).
    Queued {
        queued: String,
    },
    Tasks(Box<TaskList>),
    UploadBegun {
        upload_id: String,
        /// How many bytes each piece carries, before base64.
        chunk: usize,
    },
    UploadProgress {
        /// The bytes received so far.
        received: usize,
        /// All of them arrived and match the SHA-256 given.
        complete: bool,
    },
    Image(crate::backend::ImagePreview),
    /// Whether the relay in front of this computer sends notifications ([`Op::PushRegister`]).
    Registered {
        relay: bool,
    },
}

/// What a chat lets a person choose from a paired device: its model and its thinking level, and
/// for a session offered [`CONTROL`] its access level too.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Choices {
    pub models: Vec<Choice>,
    pub model: Option<String>,
    pub thinking: Vec<Choice>,
    pub thinking_level: Option<String>,
    /// Left out for a browser, so its answer is as it was.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub access: Option<AccessChoices>,
}

/// A chat's access levels by the value [`Op::SetAccess`] takes, and its current one.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct AccessChoices {
    pub access: Vec<Choice>,
    pub access_mode: Option<String>,
}

/// A page of archived chats ([`Op::Archived`]), newest first.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArchivedChats {
    pub chats: Vec<WorkspaceChat>,
    /// Older archived chats remain: ask again with `before` the last one's `updated_at`.
    pub more: bool,
}

/// The files a chat changed ([`Op::Changes`]), at most [`MAX_FILES`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChangedFiles {
    pub files: Vec<ChangedFile>,
    /// Why there is nothing to list, in the computer's words.
    pub unavailable: Option<String>,
    /// `turn` or `branch`.
    pub scope: &'static str,
    /// The commit a branch's changes are counted from; none for a turn's.
    pub base: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    /// `added`, `modified` or `deleted`.
    pub kind: String,
}

/// One changed file ([`Op::Diff`]), each side at most [`MAX_SIDE`]. A side that is none did
/// not exist.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileDiff {
    pub before: Option<String>,
    pub after: Option<String>,
    /// There is no text to show.
    pub binary: bool,
    /// A side was longer than what is sent: this is its start.
    pub truncated: bool,
}

/// What a chat stands on ([`Op::Facts`]); a part that cannot be read is left out.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Facts {
    pub branch: Option<String>,
    /// The chat works in a Git worktree of its own.
    pub worktree: bool,
    /// Files changed in the folder and not committed.
    pub uncommitted: Option<usize>,
    /// The agent's usage limits as it last reported them.
    pub usage: Vec<Usage>,
    pub pull_request: Option<PullRequestFacts>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Usage {
    pub window: String,
    /// How much of it is used, 0 to 100.
    pub used: f64,
}

/// The pull request open from a chat's branch.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PullRequestFacts {
    pub number: u64,
    pub url: String,
    /// `open`, `merged` or `closed`.
    pub state: String,
    /// `none`, `pending`, `passed` or `failed`.
    pub checks: &'static str,
    /// The checks that failed, by name.
    pub failed: Vec<String>,
    /// The newest reviews and comments, oldest first: at most [`MAX_NOTES`] of
    /// [`MAX_NOTE`] characters each.
    pub notes: Vec<PullRequestNote>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PullRequestNote {
    pub author: String,
    /// `comment`, or the review's state.
    pub kind: String,
    /// Empty for an author who is not one of the repository's own people: anyone can comment
    /// on a public repository.
    pub body: String,
    pub trusted: bool,
}

/// A project's brief ([`Op::Memory`]).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Brief {
    pub markdown: String,
    /// What [`Op::SetMemory`] names as `base_revision`; none while no brief was saved.
    pub revision: Option<String>,
}

/// A task that started now ([`Op::Task`]).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TaskStarted {
    pub chat_id: String,
    /// Its own branch, where the folder is a Git repository.
    pub branch: Option<String>,
    /// The agent that reviews the work once it is done; none, no review.
    pub review_agent: Option<String>,
}

/// The tasks handed over lately and the queue for tonight ([`Op::Tasks`]).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TaskList {
    /// Newest first, at most [`MAX_TASKS`].
    pub tasks: Vec<TaskEntry>,
    /// Oldest first, as they start; one that could not start stays a day, with why.
    pub queued: Vec<QueuedEntry>,
    pub night: Night,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TaskEntry {
    pub chat_id: String,
    pub goal: String,
    pub branch: Option<String>,
    pub review_agent: Option<String>,
    /// The review's chat, once it started.
    pub reviewer_chat: Option<String>,
    pub created_at: i64,
    /// `bot` (a bot link or an automation) or `device` (the person, from a paired device).
    pub by: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QueuedEntry {
    pub id: String,
    pub goal: String,
    pub project_id: Option<String>,
    pub agent_id: Option<String>,
    pub review: Option<bool>,
    pub created_at: i64,
    pub failed: Option<String>,
    pub failed_at: Option<i64>,
    pub by: &'static str,
}

/// When queued tasks start, on the computer's clock: from `from` until `to` o'clock, at most
/// `budget` a night, `started` of them so far tonight.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Night {
    pub from: u8,
    pub to: u8,
    pub budget: usize,
    pub started: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct Paired {
    pub device_id: String,
    /// The device's key, sent once, inside the pairing session.
    pub device_key: String,
    /// What Ghost for iPhone is offered from now on, as its next `ready` names it. Only for the
    /// app: a browser's answer is as it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<&'static [&'static str]>,
}

/// What needs the person across the open chats ([`Op::Inbox`]), at most [`MAX_INBOX`] of each.
/// It reads the overview and the newest page of history of each chat it lists.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Inbox {
    /// The requests chats wait on, one entry each.
    pub asking: Vec<Asking>,
    pub working: Vec<Working>,
    /// Chats whose last turn ended in the last day, newest first.
    pub recent: Vec<Recent>,
}

/// The most entries of each part of [`Inbox`].
pub const MAX_INBOX: usize = 40;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Asking {
    pub chat_id: String,
    /// What it asks. None when the chat waits on something its newest history does not show
    /// yet: the chat itself says.
    pub request: Option<Asked>,
}

/// A request a chat waits on, as `permission` or `answer` answers it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Asked {
    pub id: String,
    /// `permission` or `question`.
    pub kind: &'static str,
    /// What it asks, as the agent wrote it: a permission's title, a question's message (a
    /// grouped prompt's first question).
    pub title: String,
    /// A permission's options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<PermissionOption>>,
    /// A question's prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<UiPrompt>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Working {
    pub chat_id: String,
    /// What it is doing, in one line: its latest step's title, else how the agent's last
    /// message begins, else its status.
    pub line: String,
    /// When its turn started, in Unix milliseconds.
    pub since: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Recent {
    pub chat_id: String,
    /// When its last turn ended, in Unix milliseconds.
    pub ended_at: i64,
    pub duration_ms: Option<u64>,
    /// What the turn came to, in one line: how it ended, a command still failing, how the
    /// agent's last message begins.
    pub line: String,
    /// How many files the turn's edits changed, as the window's card under its answer counts
    /// them. Left out when it changed none its history shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<usize>,
    /// The lines its edits added and removed, as each edit's own +/− counts them. Left out when
    /// no edit could be counted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<u64>,
    /// The chat's pull request as the computer last heard of it, without asking GitHub again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<RecentPullRequest>,
}

/// A pull request as the Inbox shows it beside a finished chat.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecentPullRequest {
    pub number: u64,
    /// `open`, `merged` or `closed`.
    pub state: String,
    /// `none`, `pending`, `passed` or `failed`.
    pub checks: String,
}

/// What [`Op::Search`] found, best first.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SearchHits {
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchHit {
    pub chat_id: String,
    pub title: String,
    /// The words around a match in the chat's messages; None when only its title, project or
    /// agent matched.
    pub snippet: Option<String>,
    /// When the chat last changed, in Unix milliseconds.
    pub at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub host_name: String,
    pub projects: Vec<ProjectSummary>,
    pub chats: Vec<WorkspaceChat>,
    pub agents: Vec<RemoteAgent>,
    /// The agent new chats start with when a project names none.
    pub default_agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteAgent {
    pub id: String,
    pub name: String,
    /// Installed and ready to start a chat.
    pub available: bool,
}

#[derive(Debug, Serialize)]
pub struct Reply<'a> {
    pub re: u32,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<&'a Data>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'a ErrorBody>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
}

/// What the desktop tells a browser without being asked.
#[derive(Debug, Serialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum Event<'a> {
    /// The first sealed message of a session: opening it proves the desktop holds the key.
    /// `features` is [`FEATURES`], or [`APP_FEATURES`] for Ghost for iPhone; a desktop older than
    /// the list sends none.
    Ready {
        host_name: &'a str,
        paired: bool,
        features: &'a [&'a str],
        /// The desktop's theme and appearance; absent from older desktops and from a backend that
        /// does not say.
        #[serde(skip_serializing_if = "Option::is_none")]
        look: Option<&'a Look>,
        /// The website this computer pairs through (`https://onghost.dev`), so the iPhone app
        /// follows it when it moves. Only on the app's path; a browser is on the site already.
        #[serde(skip_serializing_if = "Option::is_none")]
        site: Option<&'a str>,
        /// Whether the relay in front of this computer sends notifications now. Only on the
        /// app's path.
        #[serde(skip_serializing_if = "Option::is_none")]
        notifications: Option<bool>,
    },
    /// A chat's status, title or attention changed.
    Chat { update: &'a ChatStatusUpdate },
    /// Live events of the chat this connection follows.
    Frame { batch: &'a EventBatch },
    /// Live events were dropped (the connection fell behind): open the chat again.
    Lost { chat_id: &'a str },
    /// Everything changed (another profile was opened): start over from the overview.
    Reset,
}

/// A bot's tool call, as the relay hands it over.
#[derive(Debug, Deserialize)]
pub struct BotCall {
    pub rid: String,
    pub bot: String,
    pub token: String,
    pub tool: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct BotAnswer<'a> {
    pub rid: &'a str,
    pub text: &'a str,
    pub error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<&'a crate::backend::Picture>,
}

#[derive(Debug, Serialize)]
pub struct BotLink<'a> {
    pub id: &'a str,
    pub hash: &'a str,
    /// Whether the relay may offer this link the full-control tools. The computer checks each
    /// call itself; this only keeps the list a bot is shown honest.
    pub full: bool,
    /// Whether the relay may offer this link only the tools that read (`readOnlyHint`). The
    /// computer checks each call itself.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read: bool,
    /// Which set of tools this copy of the app answers ([`crate::bot::TOOLS_VERSION`]), for the
    /// relay to list that one. A relay from before the number ignores it.
    pub v: u32,
    /// An automation: its address takes what is sent to it (`/hook/`) and nothing else, and it
    /// is no MCP server. The computer refuses anything but a trigger from it all the same.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hook: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_frames_parse_strictly() {
        let cid = "abcDEF012345_-xy";
        assert_eq!(from_relay(&format!("o{cid}")), Some(FromRelay::Opened(cid)));
        assert_eq!(from_relay(&format!("k{cid}")), Some(FromRelay::Native(cid)));
        assert_eq!(from_relay(&format!("c{cid}")), Some(FromRelay::Closed(cid)));
        assert_eq!(
            from_relay(&format!("m{cid} E a b")),
            Some(FromRelay::Message {
                cid,
                payload: "E a b"
            })
        );
        assert_eq!(
            from_relay(&format!("m{cid} ")),
            Some(FromRelay::Message { cid, payload: "" })
        );
        assert_eq!(from_relay("b{}"), Some(FromRelay::Bot("{}")));
        assert_eq!(from_relay("x"), Some(FromRelay::Replaced));
        assert_eq!(from_relay("pong"), Some(FromRelay::Pong));
        // What the relay does, and how a notification went.
        assert_eq!(
            from_relay(r#"f{"push":1}"#),
            Some(FromRelay::Features(r#"{"push":1}"#))
        );
        assert_eq!(
            from_relay(r#"P{"id":"a","ok":true,"drop":false}"#),
            Some(FromRelay::Pushed(r#"{"id":"a","ok":true,"drop":false}"#))
        );
        assert!(relay_pushes(r#"{"push":1}"#));
        assert!(relay_pushes(r#"{"push":true,"later":2}"#));
        for no in ["", "{}", r#"{"push":0}"#, r#"{"push":"1"}"#, "push", "[1]"] {
            assert!(!relay_pushes(no), "{no}");
        }
        let answer: PushAnswer = serde_json::from_str(
            r#"{"id":"n1","ok":false,"status":410,"reason":"Unregistered","drop":true}"#,
        )
        .expect("answer");
        assert_eq!(
            answer,
            PushAnswer {
                id: "n1".into(),
                ok: false,
                status: Some(410),
                reason: Some("Unregistered".into()),
                drop: true,
            }
        );
        // What the relay leaves out, or sends as null, is no.
        let bare: PushAnswer =
            serde_json::from_str(r#"{"id":"","ok":false,"status":null}"#).expect("answer");
        assert!(!bare.ok && !bare.drop && bare.status.is_none() && bare.reason.is_none());
        for bad in [
            "",
            "o",
            "oshort",
            &format!("o{cid}x"),
            &format!("m{cid}"),
            &format!("m{cid}E"),
            "o abcDEF012345_-x",
            // The app's mark takes exactly one connection id, as `o` does.
            "k",
            "kshort",
            &format!("k{cid}x"),
            "k abcDEF012345_-x",
            "kabcDEF012345_-x!",
            "zzz",
            "é",
        ] {
            assert_eq!(from_relay(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn requests_name_one_operation_and_nothing_else() {
        let request: Request =
            serde_json::from_str(r#"{"id":7,"op":"send","chat_id":"c1","text":"Hi"}"#)
                .expect("send");
        assert_eq!(request.id, 7);
        assert_eq!(
            request.op,
            Op::Send {
                chat_id: "c1".into(),
                text: "Hi".into(),
                attachments: Vec::new(),
            }
        );
        let request: Request =
            serde_json::from_str(r#"{"id":1,"op":"permission","chat_id":"c","request_id":"r"}"#)
                .expect("declined permission");
        assert_eq!(
            request.op,
            Op::Permission {
                chat_id: "c".into(),
                request_id: "r".into(),
                option_id: None
            }
        );
        // A browser's pairing says nothing about itself; Ghost for iPhone names itself.
        let op = |text: &str| serde_json::from_str::<Request>(text).expect(text).op;
        assert_eq!(
            op(r#"{"id":1,"op":"pair","name":"Phone"}"#),
            Op::Pair {
                name: "Phone".into(),
                client: None
            }
        );
        assert_eq!(
            op(
                r#"{"id":1,"op":"pair","name":"","client":{"kind":"ios","version":"1.0","model":"iPhone17,1"}}"#
            ),
            Op::Pair {
                name: String::new(),
                client: Some(Client {
                    kind: Some("ios".into()),
                    version: Some("1.0".into()),
                    model: Some("iPhone17,1".into()),
                })
            }
        );
        assert_eq!(op(r#"{"id":2,"op":"inbox"}"#), Op::Inbox);
        assert_eq!(
            op(r#"{"id":3,"op":"search","query":"login"}"#),
            Op::Search {
                query: "login".into(),
                limit: None
            }
        );
        assert_eq!(
            op(r#"{"id":4,"op":"search","query":"login","limit":5}"#),
            Op::Search {
                query: "login".into(),
                limit: Some(5)
            }
        );
        // Whether a session was offered a request is the computer's to say: a browser asking
        // for one it was not offered is answered `not_offered` (tests/host.rs).
        for bad in [
            r#"{"op":"overview"}"#,
            r#"{"id":1}"#,
            r#"{"id":1,"op":"shell","command":"rm"}"#,
            r#"{"id":1,"op":"send","chat_id":"c1"}"#,
            r#"{"id":-1,"op":"overview"}"#,
            r#"{"id":1,"op":"search"}"#,
            r#"{"id":1,"op":"search","query":"x","limit":-1}"#,
            r#"{"id":1,"op":"pair","name":"A","client":{"kind":7}}"#,
        ] {
            assert!(serde_json::from_str::<Request>(bad).is_err(), "{bad}");
        }
    }

    /// Every request names the feature it needs; a browser is offered what it always was, and
    /// the iPhone app everything a browser is and more.
    #[test]
    fn each_request_belongs_to_a_feature_offered_by_name() {
        assert_eq!(FEATURES, ["pickers"]);
        assert_eq!(
            APP_FEATURES,
            [
                "pickers",
                "inbox",
                "search",
                "control",
                "review",
                "memory",
                "screens",
                "tasks",
                "attachments",
                "push",
                "activity"
            ]
        );
        assert!(FEATURES.iter().all(|name| APP_FEATURES.contains(name)));
        let op = |text: &str| serde_json::from_str::<Request>(text).expect(text).op;
        for (request, feature) in [
            (r#"{"id":1,"op":"pair","name":"A"}"#, None),
            (r#"{"id":1,"op":"overview"}"#, None),
            (r#"{"id":1,"op":"open","chat_id":"c"}"#, None),
            (r#"{"id":1,"op":"close"}"#, None),
            (
                r#"{"id":1,"op":"history","chat_id":"c","before_seq":1}"#,
                None,
            ),
            (r#"{"id":1,"op":"send","chat_id":"c","text":"x"}"#, None),
            (r#"{"id":1,"op":"interrupt","chat_id":"c"}"#, None),
            (
                r#"{"id":1,"op":"permission","chat_id":"c","request_id":"r"}"#,
                None,
            ),
            (
                r#"{"id":1,"op":"answer","chat_id":"c","request_id":"r"}"#,
                None,
            ),
            (r#"{"id":1,"op":"create","folder_id":"f","text":"x"}"#, None),
            (r#"{"id":1,"op":"forget"}"#, None),
            (r#"{"id":1,"op":"options","chat_id":"c"}"#, Some("pickers")),
            (
                r#"{"id":1,"op":"set_model","chat_id":"c","model":"m"}"#,
                Some("pickers"),
            ),
            (
                r#"{"id":1,"op":"set_thinking","chat_id":"c","level":"l"}"#,
                Some("pickers"),
            ),
            (
                r#"{"id":1,"op":"switch_agent","chat_id":"c","agent_id":"a"}"#,
                Some("pickers"),
            ),
            (r#"{"id":1,"op":"inbox"}"#, Some("inbox")),
            (r#"{"id":1,"op":"search","query":"x"}"#, Some("search")),
            (r#"{"id":1,"op":"archived"}"#, Some("control")),
            (
                r#"{"id":1,"op":"set_access","chat_id":"c","access":"ask"}"#,
                Some("control"),
            ),
            (
                r#"{"id":1,"op":"rename","chat_id":"c","title":"t"}"#,
                Some("control"),
            ),
            (
                r#"{"id":1,"op":"archive","chat_id":"c","archived":true}"#,
                Some("control"),
            ),
            (
                r#"{"id":1,"op":"start_agent","chat_id":"c"}"#,
                Some("control"),
            ),
            (r#"{"id":1,"op":"changes","chat_id":"c"}"#, Some("review")),
            (
                r#"{"id":1,"op":"diff","chat_id":"c","path":"a"}"#,
                Some("review"),
            ),
            (r#"{"id":1,"op":"facts","chat_id":"c"}"#, Some("review")),
            (r#"{"id":1,"op":"memory","project_id":"p"}"#, Some("memory")),
            (
                r#"{"id":1,"op":"set_memory","project_id":"p","markdown":""}"#,
                Some("memory"),
            ),
            (
                r#"{"id":1,"op":"picture","chat_id":"c","of":"browser"}"#,
                Some("screens"),
            ),
            (r#"{"id":1,"op":"task","goal":"g"}"#, Some("tasks")),
            (r#"{"id":1,"op":"tasks"}"#, Some("tasks")),
            (
                r#"{"id":1,"op":"task_cancel","queued_id":"q"}"#,
                Some("tasks"),
            ),
            (
                r#"{"id":1,"op":"upload_begin","name":"a","mime":"image/png","size":1,"sha256":"x"}"#,
                Some("attachments"),
            ),
            (
                r#"{"id":1,"op":"upload_chunk","upload_id":"u","index":0,"data":""}"#,
                Some("attachments"),
            ),
            (
                r#"{"id":1,"op":"upload_abort","upload_id":"u"}"#,
                Some("attachments"),
            ),
            (
                r#"{"id":1,"op":"image","chat_id":"c","source":{"type":"stored","sha256":"x"}}"#,
                Some("attachments"),
            ),
            (
                r#"{"id":1,"op":"push_register","token":"ab","env":"sandbox"}"#,
                Some("push"),
            ),
            (r#"{"id":1,"op":"push_unregister"}"#, Some("push")),
            (r#"{"id":1,"op":"push_test"}"#, Some("push")),
            (
                r#"{"id":1,"op":"activity","token":"ab","env":"sandbox"}"#,
                Some("activity"),
            ),
            // Photos ride on the first version's requests, honored by feature.
            (
                r#"{"id":1,"op":"send","chat_id":"c","text":"","attachments":["u"]}"#,
                None,
            ),
            (
                r#"{"id":1,"op":"create","folder_id":"f","text":"","attachments":["u"]}"#,
                None,
            ),
        ] {
            let feature_of = op(request).feature();
            assert_eq!(feature_of, feature, "{request}");
            // Whatever a request needs, the app is offered.
            assert!(
                feature_of.is_none_or(|name| APP_FEATURES.contains(&name)),
                "{request}"
            );
        }
    }

    #[test]
    fn replies_and_events_have_the_shape_the_browser_reads() {
        let reply = Reply {
            re: 3,
            ok: true,
            data: Some(&Data::None {}),
            error: None,
        };
        assert_eq!(
            serde_json::to_string(&reply).expect("reply"),
            r#"{"re":3,"ok":true,"data":{}}"#
        );
        let error = ErrorBody {
            code: "not_found",
            message: "No such chat.".into(),
        };
        let reply = Reply {
            re: 4,
            ok: false,
            data: None,
            error: Some(&error),
        };
        assert_eq!(
            serde_json::to_string(&reply).expect("reply"),
            r#"{"re":4,"ok":false,"error":{"code":"not_found","message":"No such chat."}}"#
        );
        // A browser's `ready` is what it was before the iPhone app.
        assert_eq!(
            serde_json::to_string(&Event::Ready {
                host_name: "Studio",
                paired: true,
                features: FEATURES,
                look: None,
                site: None,
                notifications: None,
            })
            .expect("event"),
            r#"{"ev":"ready","host_name":"Studio","paired":true,"features":["pickers"]}"#
        );
        let look = Look {
            theme: "claude".into(),
            appearance: "system".into(),
        };
        assert_eq!(
            serde_json::to_string(&Event::Ready {
                host_name: "Studio",
                paired: true,
                features: FEATURES,
                look: Some(&look),
                site: None,
                notifications: None,
            })
            .expect("event"),
            r#"{"ev":"ready","host_name":"Studio","paired":true,"features":["pickers"],"look":{"theme":"claude","appearance":"system"}}"#
        );
        // The app's names its features, the site and whether notifications go.
        assert_eq!(
            serde_json::to_string(&Event::Ready {
                host_name: "Studio",
                paired: true,
                features: APP_FEATURES,
                look: Some(&look),
                site: Some("https://onghost.dev"),
                notifications: Some(true),
            })
            .expect("event"),
            r#"{"ev":"ready","host_name":"Studio","paired":true,"features":["pickers","inbox","search","control","review","memory","screens","tasks","attachments","push","activity"],"look":{"theme":"claude","appearance":"system"},"site":"https://onghost.dev","notifications":true}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::Reset).expect("event"),
            r#"{"ev":"reset"}"#
        );
        // A browser's pairing answer and its pickers are what they were; the app's pairing
        // answer adds its features.
        let paired = |features| {
            serde_json::to_string(&Data::Paired(Paired {
                device_id: "d1".into(),
                device_key: "k1".into(),
                features,
            }))
            .expect("paired")
        };
        assert_eq!(paired(None), r#"{"device_id":"d1","device_key":"k1"}"#);
        assert_eq!(
            paired(Some(APP_FEATURES)),
            r#"{"device_id":"d1","device_key":"k1","features":["pickers","inbox","search","control","review","memory","screens","tasks","attachments","push","activity"]}"#
        );
        assert_eq!(
            serde_json::to_string(&Data::Options(Choices {
                models: vec![Choice {
                    value: "opus".into(),
                    name: "Opus 5.5".into(),
                }],
                model: Some("opus".into()),
                thinking: Vec::new(),
                thinking_level: None,
                access: None,
            }))
            .expect("choices"),
            r#"{"models":[{"value":"opus","name":"Opus 5.5"}],"model":"opus","thinking":[],"thinking_level":null}"#
        );
    }

    /// What the iPhone app's Inbox and Search read, field for field.
    #[test]
    fn the_inbox_and_search_have_the_shape_the_app_reads() {
        use ghost_protocol::PermissionOptionKind;
        let inbox = Inbox {
            asking: vec![
                Asking {
                    chat_id: "c1".into(),
                    request: Some(Asked {
                        id: "r1".into(),
                        kind: "permission",
                        title: "Run `npm test`".into(),
                        options: Some(vec![PermissionOption {
                            option_id: "allow".into(),
                            kind: PermissionOptionKind::AllowOnce,
                            label: "Allow once".into(),
                        }]),
                        prompt: None,
                    }),
                },
                Asking {
                    chat_id: "c2".into(),
                    request: Some(Asked {
                        id: "q1".into(),
                        kind: "question",
                        title: "Deploy now?".into(),
                        options: None,
                        prompt: Some(UiPrompt::Confirm {
                            message: "Deploy now?".into(),
                        }),
                    }),
                },
                Asking {
                    chat_id: "c3".into(),
                    request: None,
                },
            ],
            working: vec![Working {
                chat_id: "c4".into(),
                line: "Running cargo test".into(),
                since: 1_000,
            }],
            recent: vec![
                Recent {
                    chat_id: "c5".into(),
                    ended_at: 2_000,
                    duration_ms: Some(900),
                    line: "finished. Done.".into(),
                    files: None,
                    added: None,
                    removed: None,
                    pull_request: None,
                },
                Recent {
                    chat_id: "c6".into(),
                    ended_at: 1_500,
                    duration_ms: Some(720_000),
                    line: "finished. Fixed it.".into(),
                    files: Some(6),
                    added: Some(120),
                    removed: Some(8),
                    pull_request: Some(RecentPullRequest {
                        number: 12,
                        state: "open".into(),
                        checks: "passed".into(),
                    }),
                },
            ],
        };
        assert_eq!(
            serde_json::to_value(Data::Inbox(Box::new(inbox))).expect("inbox"),
            serde_json::json!({
                "asking": [
                    { "chat_id": "c1", "request": {
                        "id": "r1", "kind": "permission", "title": "Run `npm test`",
                        "options": [{ "option_id": "allow", "kind": "allow_once", "label": "Allow once" }],
                    } },
                    { "chat_id": "c2", "request": {
                        "id": "q1", "kind": "question", "title": "Deploy now?",
                        "prompt": { "type": "confirm", "message": "Deploy now?" },
                    } },
                    { "chat_id": "c3", "request": null },
                ],
                "working": [{ "chat_id": "c4", "line": "Running cargo test", "since": 1000 }],
                "recent": [
                    { "chat_id": "c5", "ended_at": 2000, "duration_ms": 900, "line": "finished. Done." },
                    {
                        "chat_id": "c6", "ended_at": 1500, "duration_ms": 720000, "line": "finished. Fixed it.",
                        "files": 6, "added": 120, "removed": 8,
                        "pull_request": { "number": 12, "state": "open", "checks": "passed" },
                    },
                ],
            })
        );
        assert_eq!(
            serde_json::to_value(Data::Search(SearchHits {
                hits: vec![
                    SearchHit {
                        chat_id: "c1".into(),
                        title: "Fix the login form".into(),
                        snippet: Some("…the login form…".into()),
                        at: 5,
                    },
                    SearchHit {
                        chat_id: "c2".into(),
                        title: "Login".into(),
                        snippet: None,
                        at: 4,
                    },
                ],
            }))
            .expect("hits"),
            serde_json::json!({ "hits": [
                { "chat_id": "c1", "title": "Fix the login form", "snippet": "…the login form…", "at": 5 },
                { "chat_id": "c2", "title": "Login", "snippet": null, "at": 4 },
            ] })
        );
    }
    /// Each of the app's requests reads with its fields and their defaults; one whose fields are
    /// missing or of the wrong type is outside the protocol. What a value may be (an access
    /// level, a scope, a source) is the computer's to answer `invalid`, so those read as text.
    #[test]
    fn the_apps_requests_read_with_their_fields() {
        let op = |text: &str| serde_json::from_str::<Request>(text).expect(text).op;
        assert_eq!(
            op(r#"{"id":1,"op":"archived"}"#),
            Op::Archived {
                before: None,
                limit: None
            }
        );
        assert_eq!(
            op(r#"{"id":1,"op":"archived","before":1700000000000,"limit":20}"#),
            Op::Archived {
                before: Some(1_700_000_000_000),
                limit: Some(20)
            }
        );
        assert_eq!(
            op(r#"{"id":1,"op":"set_access","chat_id":"c","access":"everything"}"#),
            Op::SetAccess {
                chat_id: "c".into(),
                access: "everything".into()
            }
        );
        assert_eq!(
            op(r#"{"id":1,"op":"archive","chat_id":"c","archived":false}"#),
            Op::Archive {
                chat_id: "c".into(),
                archived: false
            }
        );
        assert_eq!(
            op(r#"{"id":1,"op":"changes","chat_id":"c"}"#),
            Op::Changes {
                chat_id: "c".into(),
                scope: None
            }
        );
        assert_eq!(
            op(r#"{"id":1,"op":"diff","chat_id":"c","path":"src/a.ts","scope":"branch"}"#),
            Op::Diff {
                chat_id: "c".into(),
                path: "src/a.ts".into(),
                scope: Some("branch".into())
            }
        );
        assert_eq!(
            op(
                r##"{"id":1,"op":"set_memory","project_id":"p","markdown":"# Brief","base_revision":"r1"}"##
            ),
            Op::SetMemory {
                project_id: "p".into(),
                markdown: "# Brief".into(),
                base_revision: Some("r1".into())
            }
        );
        assert_eq!(
            op(r#"{"id":1,"op":"task","goal":"Fix it"}"#),
            Op::Task {
                goal: "Fix it".into(),
                project_id: None,
                agent_id: None,
                review: None,
                when: None,
                access: None,
                attachments: Vec::new()
            }
        );
        assert_eq!(
            op(
                r#"{"id":1,"op":"task","goal":"Fix it","project_id":"p1","agent_id":"codex","review":false,"when":"tonight","access":"auto_edit","attachments":["u1"]}"#
            ),
            Op::Task {
                goal: "Fix it".into(),
                project_id: Some("p1".into()),
                agent_id: Some("codex".into()),
                review: Some(false),
                when: Some("tonight".into()),
                access: Some("auto_edit".into()),
                attachments: vec!["u1".into()]
            }
        );
        assert_eq!(op(r#"{"id":1,"op":"tasks"}"#), Op::Tasks);
        assert_eq!(
            op(
                r#"{"id":1,"op":"upload_begin","name":"IMG_1.jpg","mime":"image/jpeg","size":8388608,"sha256":"ab"}"#
            ),
            Op::UploadBegin {
                name: "IMG_1.jpg".into(),
                mime: "image/jpeg".into(),
                size: 8_388_608,
                sha256: "ab".into()
            }
        );
        assert_eq!(
            op(r#"{"id":1,"op":"upload_chunk","upload_id":"u","index":3,"data":"QUJD"}"#),
            Op::UploadChunk {
                upload_id: "u".into(),
                index: 3,
                data: "QUJD".into()
            }
        );
        assert_eq!(
            op(r#"{"id":1,"op":"image","chat_id":"c","source":{"type":"attachment","id":"a1"}}"#),
            Op::Image {
                chat_id: "c".into(),
                source: serde_json::json!({ "type": "attachment", "id": "a1" }),
                full: false
            }
        );
        assert_eq!(
            op(r#"{"id":1,"op":"send","chat_id":"c","text":"","attachments":["u1","u2"]}"#),
            Op::Send {
                chat_id: "c".into(),
                text: String::new(),
                attachments: vec!["u1".into(), "u2".into()]
            }
        );
        assert_eq!(
            op(r#"{"id":1,"op":"create","folder_id":"f","agent_id":null,"text":"Hi"}"#),
            Op::Create {
                folder_id: "f".into(),
                agent_id: None,
                text: "Hi".into(),
                attachments: Vec::new()
            }
        );
        // Notifications: what a device wants is everything unless it says otherwise.
        assert_eq!(
            op(r#"{"id":1,"op":"push_register","token":"ab","env":"sandbox"}"#),
            Op::PushRegister {
                token: "ab".into(),
                env: "sandbox".into(),
                prefs: None
            }
        );
        assert_eq!(
            op(
                r#"{"id":1,"op":"push_register","token":"ab","env":"production","prefs":{"finished":false,"scope":"mine"}}"#
            ),
            Op::PushRegister {
                token: "ab".into(),
                env: "production".into(),
                prefs: Some(PushPrefs {
                    asking: true,
                    finished: false,
                    failed: true,
                    tasks: true,
                    scope: Some("mine".into())
                })
            }
        );
        assert_eq!(op(r#"{"id":1,"op":"push_unregister"}"#), Op::PushUnregister);
        assert_eq!(op(r#"{"id":1,"op":"push_test"}"#), Op::PushTest);
        assert_eq!(
            op(r#"{"id":1,"op":"activity","token":"ab","env":"sandbox"}"#),
            Op::Activity {
                token: "ab".into(),
                env: "sandbox".into()
            }
        );
        for bad in [
            r#"{"id":1,"op":"push_register","env":"sandbox"}"#,
            r#"{"id":1,"op":"push_register","token":"ab"}"#,
            r#"{"id":1,"op":"push_register","token":"ab","env":"sandbox","prefs":{"asking":"yes"}}"#,
            r#"{"id":1,"op":"push_register","token":7,"env":"sandbox"}"#,
            r#"{"id":1,"op":"activity","token":"ab"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(bad).is_err(), "{bad}");
        }
        for bad in [
            r#"{"id":1,"op":"archived","limit":-1}"#,
            r#"{"id":1,"op":"archived","before":"yesterday"}"#,
            r#"{"id":1,"op":"set_access","chat_id":"c"}"#,
            r#"{"id":1,"op":"rename","chat_id":"c"}"#,
            r#"{"id":1,"op":"archive","chat_id":"c"}"#,
            r#"{"id":1,"op":"archive","chat_id":"c","archived":"yes"}"#,
            r#"{"id":1,"op":"diff","chat_id":"c"}"#,
            r#"{"id":1,"op":"set_memory","project_id":"p"}"#,
            r#"{"id":1,"op":"picture","chat_id":"c"}"#,
            r#"{"id":1,"op":"task"}"#,
            r#"{"id":1,"op":"task","goal":"g","review":"yes"}"#,
            r#"{"id":1,"op":"task","goal":"g","attachments":"u1"}"#,
            r#"{"id":1,"op":"task_cancel"}"#,
            r#"{"id":1,"op":"upload_begin","name":"a","mime":"image/png","size":-1,"sha256":"x"}"#,
            r#"{"id":1,"op":"upload_begin","name":"a","mime":"image/png","sha256":"x"}"#,
            r#"{"id":1,"op":"upload_chunk","upload_id":"u","index":-1,"data":""}"#,
            r#"{"id":1,"op":"upload_chunk","upload_id":"u","data":""}"#,
            r#"{"id":1,"op":"image","chat_id":"c"}"#,
            r#"{"id":1,"op":"send","chat_id":"c","text":"x","attachments":[1]}"#,
        ] {
            assert!(serde_json::from_str::<Request>(bad).is_err(), "{bad}");
        }
    }

    /// What the app reads back from each of its requests, field for field.
    #[test]
    fn the_apps_answers_have_the_shape_it_reads() {
        let json = |data: Data| serde_json::to_value(data).expect("data");
        let choice = |value: &str, name: &str| Choice {
            value: value.into(),
            name: name.into(),
        };
        assert_eq!(
            json(Data::Options(Choices {
                models: vec![choice("opus", "Opus 5.5")],
                model: Some("opus".into()),
                thinking: Vec::new(),
                thinking_level: None,
                access: Some(AccessChoices {
                    access: vec![choice("ask", "Ask"), choice("full_auto", "Full access")],
                    access_mode: Some("ask".into()),
                }),
            })),
            serde_json::json!({
                "models": [{ "value": "opus", "name": "Opus 5.5" }], "model": "opus",
                "thinking": [], "thinking_level": null,
                "access": [{ "value": "ask", "name": "Ask" }, { "value": "full_auto", "name": "Full access" }],
                "access_mode": "ask",
            })
        );
        assert_eq!(
            json(Data::Changes(Box::new(ChangedFiles {
                files: vec![ChangedFile {
                    path: "src/login.ts".into(),
                    kind: "modified".into(),
                }],
                unavailable: None,
                scope: "branch",
                base: Some("abc123".into()),
            }))),
            serde_json::json!({
                "files": [{ "path": "src/login.ts", "kind": "modified" }],
                "unavailable": null, "scope": "branch", "base": "abc123",
            })
        );
        assert_eq!(
            json(Data::Diff(Box::new(FileDiff {
                before: None,
                after: Some("new".into()),
                binary: false,
                truncated: false,
            }))),
            serde_json::json!({ "before": null, "after": "new", "binary": false, "truncated": false })
        );
        assert_eq!(
            json(Data::Facts(Box::new(Facts {
                branch: Some("ghost/fix".into()),
                worktree: true,
                uncommitted: Some(2),
                usage: vec![Usage {
                    window: "5 hours".into(),
                    used: 42.0,
                }],
                pull_request: Some(PullRequestFacts {
                    number: 12,
                    url: "https://github.com/o/r/pull/12".into(),
                    state: "open".into(),
                    checks: "failed",
                    failed: vec!["test".into()],
                    notes: vec![PullRequestNote {
                        author: "stranger".into(),
                        kind: "comment".into(),
                        body: String::new(),
                        trusted: false,
                    }],
                }),
            }))),
            serde_json::json!({
                "branch": "ghost/fix", "worktree": true, "uncommitted": 2,
                "usage": [{ "window": "5 hours", "used": 42.0 }],
                "pull_request": {
                    "number": 12, "url": "https://github.com/o/r/pull/12", "state": "open",
                    "checks": "failed", "failed": ["test"],
                    "notes": [{ "author": "stranger", "kind": "comment", "body": "", "trusted": false }],
                },
            })
        );
        assert_eq!(
            json(Data::Memory(Brief {
                markdown: "# Brief".into(),
                revision: None,
            })),
            serde_json::json!({ "markdown": "# Brief", "revision": null })
        );
        assert_eq!(
            json(Data::SavedMemory {
                revision: Some("r2".into())
            }),
            serde_json::json!({ "revision": "r2" })
        );
        assert_eq!(
            json(Data::Picture(crate::backend::Picture {
                mime: "image/jpeg".into(),
                data: "QUJD".into(),
            })),
            serde_json::json!({ "mime": "image/jpeg", "data": "QUJD" })
        );
        assert_eq!(
            json(Data::Started(TaskStarted {
                chat_id: "c2".into(),
                branch: None,
                review_agent: Some("codex".into()),
            })),
            serde_json::json!({ "chat_id": "c2", "branch": null, "review_agent": "codex" })
        );
        assert_eq!(
            json(Data::Queued {
                queued: "qabc".into()
            }),
            serde_json::json!({ "queued": "qabc" })
        );
        assert_eq!(
            json(Data::Tasks(Box::new(TaskList {
                tasks: vec![TaskEntry {
                    chat_id: "c2".into(),
                    goal: "Fix it".into(),
                    branch: Some("ghost/fix-it".into()),
                    review_agent: None,
                    reviewer_chat: None,
                    created_at: 5,
                    by: "device",
                }],
                queued: vec![QueuedEntry {
                    id: "qabc".into(),
                    goal: "Tidy".into(),
                    project_id: Some("p1".into()),
                    agent_id: None,
                    review: Some(true),
                    created_at: 6,
                    failed: None,
                    failed_at: None,
                    by: "bot",
                }],
                night: Night {
                    from: 1,
                    to: 6,
                    budget: 3,
                    started: 0,
                },
            }))),
            serde_json::json!({
                "tasks": [{
                    "chat_id": "c2", "goal": "Fix it", "branch": "ghost/fix-it", "review_agent": null,
                    "reviewer_chat": null, "created_at": 5, "by": "device",
                }],
                "queued": [{
                    "id": "qabc", "goal": "Tidy", "project_id": "p1", "agent_id": null,
                    "review": true, "created_at": 6, "failed": null, "failed_at": null, "by": "bot",
                }],
                "night": { "from": 1, "to": 6, "budget": 3, "started": 0 },
            })
        );
        assert_eq!(
            json(Data::UploadBegun {
                upload_id: "u1".into(),
                chunk: UPLOAD_CHUNK,
            }),
            serde_json::json!({ "upload_id": "u1", "chunk": 49152 })
        );
        assert_eq!(
            json(Data::UploadProgress {
                received: 10,
                complete: true,
            }),
            serde_json::json!({ "received": 10, "complete": true })
        );
        assert_eq!(
            json(Data::Image(crate::backend::ImagePreview {
                mime: "image/png".into(),
                data: "QUJD".into(),
                width: 2,
                height: 1,
            })),
            serde_json::json!({ "mime": "image/png", "data": "QUJD", "width": 2, "height": 1 })
        );
        assert_eq!(
            json(Data::Archived(Box::new(ArchivedChats {
                chats: Vec::new(),
                more: false,
            }))),
            serde_json::json!({ "chats": [], "more": false })
        );
        assert_eq!(
            json(Data::Registered { relay: true }),
            serde_json::json!({ "relay": true })
        );
    }

    /// A notification as the relay reads it: where it goes, the sealed blob, and an alert's or a
    /// Live Activity's own parts only.
    #[test]
    fn a_push_frame_has_the_parts_the_relay_reads() {
        let alert = PushFrame {
            id: "s1".into(),
            to: "ab".repeat(32),
            env: "sandbox",
            kind: "alert",
            collapse: Some("Q29sbGFwc2VJZDEy".into()),
            d: "TW9ja0RldjE".into(),
            b: "AAAA".into(),
            ttl: 900,
            sound: Some(true),
            priority: None,
            event: None,
        };
        assert_eq!(
            serde_json::to_value(&alert).expect("json"),
            serde_json::json!({
                "id": "s1", "to": "ab".repeat(32), "env": "sandbox", "type": "alert",
                "collapse": "Q29sbGFwc2VJZDEy", "d": "TW9ja0RldjE", "b": "AAAA", "ttl": 900,
                "sound": true,
            })
        );
        let live = PushFrame {
            kind: "liveactivity",
            collapse: None,
            sound: None,
            priority: Some(5),
            event: Some("end"),
            ttl: 3_600,
            ..alert
        };
        assert_eq!(
            serde_json::to_value(&live).expect("json"),
            serde_json::json!({
                "id": "s1", "to": "ab".repeat(32), "env": "sandbox", "type": "liveactivity",
                "d": "TW9ja0RldjE", "b": "AAAA", "ttl": 3600, "priority": 5, "event": "end",
            })
        );
    }
}
