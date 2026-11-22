//! The chat actor: one per chat, owning its state, batcher, normalizer, subscribers, queue, open
//! turn and live driver; it never awaits inline, so IPC commands, driver events, store permits,
//! flushes and timers are all handled in one loop.
//!
//! Every wait is a future in `in_flight`, tagged with what it completes: a start's preparation
//! (folder check, detection, adapter install, launch spec, env; one task, dropped as a whole), page
//! reads for attaches and history, option and autonomy replies, the driver's join handle once its
//! event channel closed (so every event is applied before its exit is handled), the force-stop
//! chains, and the abort of a driver that has not returned 5 s after a kill.
//!
//! Drivers are generations: a force stop retires the current one at once (its event receiver is
//! dropped, so no late event is ever applied, and its processes get the FORCE chain), and the chat
//! reads `Stopped` at once. A retired generation, like one that crashed, keeps the chat's live slot
//! and native sessions until its processes are gone and its task ended; a new start waits for that
//! before it spawns anything, so one native session never has two processes.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
mod controls;
mod options;
mod rewind;
mod steering;

use futures::FutureExt;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use o7_agents::{AgentSpec, LaunchError, Recipe};
use o7_drivers::batch::{FrameBatcher, TurnTag};
use o7_drivers::driver::{
    DriverCommand, DriverContext, DriverError, DriverEvent, DriverExit, DriverStatus,
    DriverTimings, EventReceiver, PermissionOutcome, ReadyInfo, ResumeOutcome, SessionSetup,
    StopPlan, TurnRef, event_channel,
};
use o7_process::{
    ExitInfo, LaunchSpec, ProcessControl, ResolvedEnv, STDERR_TAIL_SHOWN_BYTES, ShutdownPlan,
};
use o7_protocol::session::{
    AuthPrompt, Availability, ChatAttach, ChatSnapshot, ChatSummary, ConfigCategory, ConfigKind,
    ConfigOption, ConfigValue, ErrorInfo, FolderState, HistoryPage, PromptAck, QueuedPrompt,
    SendingPrompt, SessionLists,
};
use o7_protocol::{
    AutoAnswer, Autonomy, AutonomyState, Capabilities, ErrorKind, HoldCause, LimitWindow, Millis,
    NoticeLevel, O7Event, PermissionDecision, PermissionRequest, PromptPart, PromptState,
    QueueWait, ResolvedBy, SessionInfo, SessionStatus, SlashCommand, StopReason, UiAnswer,
    UiPrompt, UiRequest, UsageInfo,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{AbortHandle, JoinError, JoinHandle};
use tokio::time::Instant;

use super::live::NativeSessionScope;
use super::normalize::{
    MESSAGE_BYTES, Normalizer, OpenCard, cap_chars, cap_escaped, suffix_escaped,
};
use super::policy::{Decision, decide};
use super::spawner::{ProcessBook, SessionSpawner};
use super::{ChatState, FrameSink, SessionError, Shared, SinkGone, lock};
use crate::chatstore::blobs::BLOB_UNSAVED;
use crate::chatstore::rows::{ReplyHasher, turn_status};
use crate::chatstore::{
    AgentSessionPatch, AgentSessionRow, ChatLoad, ChatRecord, ChatStore, EventRow, Op, QueuedRow,
    ReplayCheck, StoreError, TitleSource, TurnEnd, TurnRow, WaitError, await_reply, op_bytes,
};
use crate::ids::new_id;
use crate::time::now_millis;

/// New turns are refused this close to the end of the seq space (a turn can produce many events).
const SEQ_LIMIT: u32 = u32::MAX - (1 << 24);

/// A queued prompt's preview.
const PREVIEW_CHARS: usize = 256;

/// A first-line title.
const TITLE_CHARS: usize = 80;

/// A force-stopped driver that has not returned this long after the kill is aborted.
const ABORT_AFTER_KILL: Duration = Duration::from_secs(5);

/// `set_option` and `set_autonomy` wait this long for the driver (its 30 s plus margin).
const OPTION_REPLY_TIMEOUT: Duration = Duration::from_secs(35);

/// The withdrawals of answered requests are remembered for this many requests.
const ANSWERED_REMEMBERED: usize = 64;

/// The chat, the folder context of its running agent, and the connection it runs or would start
/// with (its saved session's, else its agent's current one).
pub(crate) type FolderContextReply = (
    ChatRecord,
    Option<o7_protocol::workspace::ChatFolderContext>,
    Option<AgentSpec>,
);

/// A message for a chat's actor. Every reply is a oneshot.
pub(crate) enum ChatMsg {
    McpAction {
        server_name: String,
        action: o7_protocol::mcp::McpSessionAction,
        reply: oneshot::Sender<Result<o7_protocol::mcp::McpActionResult, SessionError>>,
    },
    SetupChanged(o7_protocol::worktree_setup::WorktreeSetupAttempt),
    FolderContext {
        reply: oneshot::Sender<Result<FolderContextReply, SessionError>>,
    },
    ReserveWorkspace {
        id: String,
        restoring: bool,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    ReserveGit {
        id: String,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    ReserveSetup {
        id: String,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    ReleaseGit {
        id: String,
    },
    /// A quiet change to the chat's folder (a commit, a setup copy): refused while a turn, a
    /// start or a stop is under way; otherwise new turns and starts wait until `ReleaseQuiet`.
    HoldQuiet {
        id: String,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    ReleaseQuiet {
        id: String,
    },
    FolderLocated {
        reservation: String,
        record: ChatRecord,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    Conversation {
        command: crate::chatstore::conversation::ConversationCommand,
        reply: oneshot::Sender<
            Result<crate::chatstore::conversation::ConversationReply, SessionError>,
        >,
    },
    Continue {
        apply: Box<crate::chatstore::conversation::ApplyContinuation>,
        prompt: Option<(String, Vec<PromptPart>)>,
        reply: oneshot::Sender<Result<o7_protocol::conversation::ContinuationResult, SessionError>>,
    },
    CheckpointContext {
        turn_id: Option<String>,
        reply: oneshot::Sender<Result<(ChatRecord, Option<String>), SessionError>>,
    },
    Revert {
        token: String,
        guard: Arc<super::changes::FolderMutation>,
        reply: oneshot::Sender<Result<o7_protocol::changes::RevertResult, SessionError>>,
    },
    Organize {
        command: crate::chatstore::organization::OrganizationCommand,
        reply: oneshot::Sender<Result<crate::chatstore::organization::Organization, SessionError>>,
    },
    Attach {
        sink: Arc<dyn FrameSink>,
        subscription_id: String,
        reply: oneshot::Sender<Result<ChatAttach, SessionError>>,
    },
    Detach {
        subscription_id: String,
    },
    DropOwner {
        owner: String,
    },
    History {
        before_seq: u32,
        max_bytes: usize,
        reply: oneshot::Sender<Result<HistoryPage, SessionError>>,
    },
    Lists {
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
    },
    Send {
        text: String,
        parts: Vec<PromptPart>,
        reply: oneshot::Sender<Result<PromptAck, SessionError>>,
    },
    Steer {
        expected_turn_id: String,
        text: String,
        parts: Vec<PromptPart>,
        reply: oneshot::Sender<Result<o7_protocol::composer::SteeringAck, SessionError>>,
    },
    /// Restores files from the agent's own checkpoints to before a turn's message.
    RewindFiles {
        turn_id: String,
        dry_run: bool,
        reply: oneshot::Sender<Result<o7_protocol::changes::NativeRewind, SessionError>>,
    },
    CancelQueued {
        prompt_id: String,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    /// The words of a queued prompt as typed, for editing it in place.
    QueuedText {
        prompt_id: String,
        reply: oneshot::Sender<Result<String, SessionError>>,
    },
    /// Replaces a queued prompt's words; answers the queue after the change.
    EditQueued {
        prompt_id: String,
        text: String,
        reply: oneshot::Sender<Result<Vec<QueuedPrompt>, SessionError>>,
    },
    /// Moves a queued prompt to `index` (0 goes next); answers the queue after the change.
    MoveQueued {
        prompt_id: String,
        index: usize,
        reply: oneshot::Sender<Result<Vec<QueuedPrompt>, SessionError>>,
    },
    ResumeQueue {
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    /// The queued prompt goes next ("Send now"); answers the queue after the change.
    SendNow {
        prompt_id: String,
        reply: oneshot::Sender<Result<Vec<QueuedPrompt>, SessionError>>,
    },
    Interrupt {
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    ForceStop {
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    /// `explicit`: `chat_start` (a full slot is an error); otherwise a start a chat's creation or a
    /// prompt triggered (a full slot is an error card).
    Start {
        fresh: bool,
        explicit: bool,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    /// Opens the chat in its agent's own interface (a terminal chat): prepares as a start would,
    /// then answers with the launch instead of starting the driver. Chat waits while it is open.
    OpenTerminal {
        reply: oneshot::Sender<Result<TerminalOpened, SessionError>>,
    },
    /// The terminal `OpenTerminal` answered ended: it exited, or its page closed it.
    TerminalEnded {
        generation: u64,
    },
    Stop {
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    /// Stops the agent only while it idles (see `idles`), or with `even_shown` while it is not
    /// working though a view is open; answers whether it began stopping.
    StopIfIdle {
        even_shown: bool,
        reply: oneshot::Sender<Result<bool, SessionError>>,
    },
    /// The account switch: the agent of `agent` starts again on the account it starts on now,
    /// resuming the same session, and the held prompts go once it is ready.
    RestartOnAccount {
        agent: String,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    RespondPermission {
        request_id: String,
        option_id: Option<String>,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    RespondUi {
        request_id: String,
        answer: Option<UiAnswer>,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    SetControl {
        change: o7_protocol::session::SessionControlChange,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
    },
    SetOption {
        option_id: String,
        value: ConfigValue,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
    },
    SetAutonomy {
        autonomy: Autonomy,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
    },
    /// Before a blob read: a barrier to wait for when this chat's own `PutBlob` for the blob has not
    /// reached the store yet, `BlobMissing` when it waits on a paused store, `ChatNotFound` for a
    /// chat that does not exist.
    Blob {
        sha256: String,
        reply: oneshot::Sender<Result<Option<oneshot::Receiver<()>>, SessionError>>,
    },
    Shutdown {
        plan: StopPlan,
        reply: oneshot::Sender<()>,
    },
    /// Tests: the actor task panics.
    #[cfg(test)]
    Panic,
}

impl ChatMsg {
    /// Answers the message with `error` (a chat that does not exist, a store that is gone).
    fn refuse(self, error: SessionError) {
        match self {
            ChatMsg::ReserveWorkspace { reply, .. } | ChatMsg::ReserveGit { reply, .. } => {
                drop(reply.send(Err(error)))
            }
            ChatMsg::ReleaseGit { .. } | ChatMsg::ReleaseQuiet { .. } => {}
            ChatMsg::HoldQuiet { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::FolderLocated { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::Conversation { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::Continue { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::CheckpointContext { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::Revert { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::Organize { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::ReserveSetup { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::Attach { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::History { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::Lists { reply } => drop(reply.send(Err(error))),
            ChatMsg::FolderContext { reply } => drop(reply.send(Err(error))),
            ChatMsg::McpAction { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::Send { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::Steer { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::RewindFiles { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::SetControl { reply, .. }
            | ChatMsg::SetOption { reply, .. }
            | ChatMsg::SetAutonomy { reply, .. } => {
                drop(reply.send(Err(error)));
            }
            ChatMsg::Blob { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::QueuedText { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::EditQueued { reply, .. }
            | ChatMsg::MoveQueued { reply, .. }
            | ChatMsg::SendNow { reply, .. } => {
                drop(reply.send(Err(error)));
            }
            ChatMsg::CancelQueued { reply, .. }
            | ChatMsg::ResumeQueue { reply }
            | ChatMsg::Interrupt { reply }
            | ChatMsg::ForceStop { reply }
            | ChatMsg::Start { reply, .. }
            | ChatMsg::Stop { reply }
            | ChatMsg::RestartOnAccount { reply, .. }
            | ChatMsg::RespondPermission { reply, .. }
            | ChatMsg::RespondUi { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::Shutdown { reply, .. } => {
                let _ = reply.send(());
            }
            ChatMsg::OpenTerminal { reply } => drop(reply.send(Err(error))),
            ChatMsg::StopIfIdle { reply, .. } => drop(reply.send(Err(error))),
            ChatMsg::Detach { .. }
            | ChatMsg::DropOwner { .. }
            | ChatMsg::SetupChanged(_)
            | ChatMsg::TerminalEnded { .. } => {}
            #[cfg(test)]
            ChatMsg::Panic => {}
        }
    }
}

/// Loads the chat, then runs its actor until the manager is gone.
pub(crate) async fn run(
    chat_id: String,
    shared: Arc<Shared>,
    book: Arc<ProcessBook>,
    mut mailbox: mpsc::UnboundedReceiver<ChatMsg>,
) {
    let refusal = match shared.deps.store.load_chat_inner(&chat_id).await {
        Ok(Some(load)) => {
            ChatActor::new(chat_id, shared, book, load)
                .run(mailbox)
                .await;
            return;
        }
        Ok(None) => SessionError::ChatNotFound(chat_id),
        Err(error) => storage_error(&error),
    };
    // Nothing more arrives: the manager starts a new actor for the next call.
    mailbox.close();
    while let Ok(msg) = mailbox.try_recv() {
        msg.refuse(clone_error(&refusal));
    }
}

/// What a store error reads as for a caller.
pub(crate) fn storage_error(error: &StoreError) -> SessionError {
    match error {
        StoreError::BlobMissing(message) => SessionError::BlobMissing(message.clone()),
        other => SessionError::StorageUnavailable(other.to_string()),
    }
}

/// Why a start found no room: every slot is taken and no agent idles, so it says what they do.
fn too_many_agents(limit: usize, occupancy: super::live::Occupancy) -> SessionError {
    let parts: Vec<String> = [
        (occupancy.working, "working"),
        (occupancy.waiting, "waiting for your answer"),
        (occupancy.terminals, "open in a terminal"),
        (occupancy.shown, "open on screen"),
        (occupancy.stopping, "stopping"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, what)| format!("{count} {what}"))
    .collect();
    let list = match parts.as_slice() {
        [] => String::new(),
        [one] => format!(": {one}"),
        [rest @ .., last] => format!(": {} and {last}", rest.join(", ")),
    };
    let message = if limit == 1 {
        format!("1 agent is running and it is not idle{list}. Stop it, then try again.")
    } else {
        format!("{limit} agents are running and none is idle{list}. Stop one, then try again.")
    };
    SessionError::TooManyAgents { limit, message }
}

/// `SessionError` is not `Clone` (it is a public frozen type); the variants a refusal uses are.
fn clone_error(error: &SessionError) -> SessionError {
    match error {
        SessionError::ChatNotFound(id) => SessionError::ChatNotFound(id.clone()),
        SessionError::StorageUnavailable(message) => {
            SessionError::StorageUnavailable(message.clone())
        }
        SessionError::ShuttingDown => SessionError::ShuttingDown,
        other => SessionError::Internal(other.to_string()),
    }
}

/// One open view of the chat.
struct Subscriber {
    id: String,
    owner: String,
    sink: Arc<dyn FrameSink>,
}

/// The turn in flight.
struct OpenTurn {
    parts: Vec<PromptPart>,
    id: String,
    tag: Arc<str>,
    prompt_id: String,
    /// The prompt as sent, so a sign-in error can queue it again.
    text: String,
    prompt_created_at: Millis,
    started: Instant,
    timing: PromptTiming,
    interrupt_requested: bool,
    reply: ReplyHasher,
}

/// How a driver is being stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    None,
    /// A `Shutdown` was sent; `auth`: the stop after a prompt met a sign-in error.
    Graceful {
        auth: bool,
    },
}

/// The current driver.
struct LiveDriver {
    folder_context: o7_protocol::workspace::ChatFolderContext,
    generation: u64,
    commands: mpsc::UnboundedSender<DriverCommand>,
    /// `None` once the driver dropped its sender.
    events: Option<EventReceiver>,
    /// Joins `in_flight` once the event channel closed.
    join: Option<JoinHandle<DriverExit>>,
    /// Aborts the driver's task, wherever its join handle went.
    abort: AbortHandle,
    ready: bool,
    /// `None` keeps the original turn-based contract. Otherwise only an idle report permits
    /// another prompt, even after the current prompt's result has arrived...
    run_idle: Option<bool>,
    /// ...or the idle grace: the agent reported work in progress with no prompt of o7's
    /// open for `idle_grace` without reporting idle. Cleared by the next prompt or idle report.
    idle_assumed: bool,
    native_scope: NativeSessionScope,
    stop: Stop,
    spawn_id: Option<String>,
}

/// A start's preparation in flight.
struct Preparing {
    generation: u64,
    abort: AbortHandle,
    fresh: bool,
}

/// A terminal chat's launch, for the terminal registry to start, and what it left out.
pub struct TerminalOpened {
    pub spec: o7_process::SpawnSpec,
    /// Hand it back with `terminal_ended` when the terminal ends.
    pub generation: u64,
    /// Shared servers the agent's terminal cannot take, with why.
    pub omitted: Vec<String>,
    /// What the terminal types first, once the agent's interface is ready: the visible handoff
    /// when the chat moved here from another agent. Input, never an argument: a command line is
    /// readable by other programs and logs.
    pub first_input: Option<String>,
    /// What the terminal types to ask the agent's interface to exit.
    pub exit_input: String,
}

/// Where a terminal's conversation is read back from (terminal_import.rs): the agent's session
/// file once known, and its length when the terminal opened, so only what the terminal added is
/// recorded and turns Chat already holds are not.
#[derive(Debug, Clone)]
struct TerminalImport {
    vendor: super::terminal_import::Vendor,
    /// Where the agent keeps its sessions (Claude Code's config folder, Codex's home, ...).
    home: PathBuf,
    native_id: Option<String>,
    from: u64,
    folder: PathBuf,
    opened_at: Millis,
    /// The agent's own command, for agents read through it (Hermes, OpenCode).
    cli: super::terminal_import::Cli,
}

/// What reading a terminal's conversation found.
struct TerminalRead {
    imported: super::terminal_import::Imported,
    /// The session a new terminal started, learned from the agent's own record (Codex's rollout,
    /// OpenCode's and Hermes' session lists, ...).
    learned: Option<String>,
    /// The agent's turns cannot be read back (Antigravity, Cursor).
    unread: bool,
    /// The terminal's session is known (it resumed or named one, or one was learned), so Chat
    /// continues it.
    session_known: bool,
}

/// What a preparation produces.
struct Prepared {
    memory_mcp_available: bool,
    /// The chat is a coordinated project's coordinator or one of its workers.
    coordinated: bool,
    /// Shared MCP servers this agent cannot take, said in the chat as it starts.
    mcp_notice: Option<String>,
    mcp_servers: Vec<o7_protocol::mcp::ResolvedMcpServer>,
    /// The connection the start uses.
    spec: Box<AgentSpec>,
    folder_context: o7_protocol::workspace::ChatFolderContext,
    folder_key: PathBuf,
    launch: LaunchSpec,
    env: Arc<ResolvedEnv>,
    /// The added account the start signs in with; `None`: the agent's own sign-in.
    account: Option<crate::agent_accounts::AccountLaunch>,
}

/// Which connection a start uses: a saved session resumes through the connection that made it; a
/// new session takes the one its agent's Connection setting picks when the start begins.
enum StartConnection {
    Saved(Box<AgentSpec>),
    New { agent: String },
}

/// The connection a chat shows before it starts: its saved session's, else the one a new session
/// of its agent would use now.
fn chat_connection(
    detector: &o7_agents::Detector,
    chat: &ChatRecord,
    session: Option<&AgentSessionRow>,
) -> Option<AgentSpec> {
    match session {
        Some(session) => detector.spec(&session.agent_id),
        None => detector.connection_for(&chat.agent_id),
    }
}

/// Why a start failed before its driver ran.
struct StartFailure {
    kind: ErrorKind,
    message: String,
    folder: Option<FolderState>,
}

/// Who asked for a page.
enum PageFor {
    Attach {
        reply: oneshot::Sender<Result<ChatAttach, SessionError>>,
        subscription_id: String,
        next_seq: u32,
        snapshot: Box<ChatSnapshot>,
    },
    History {
        reply: oneshot::Sender<Result<HistoryPage, SessionError>>,
    },
}

struct NativeIdentitySave {
    generation: u64,
    native_id: String,
    row_id: String,
    previous_id: Option<String>,
}

/// What an `in_flight` future completes.
enum Done {
    TerminalRead {
        generation: u64,
        read: TerminalRead,
    },
    McpAction {
        generation: u64,
        reply: oneshot::Sender<Result<o7_protocol::mcp::McpActionResult, SessionError>>,
        result: Result<
            Result<
                Result<o7_protocol::mcp::McpActionResult, DriverError>,
                oneshot::error::RecvError,
            >,
            tokio::time::error::Elapsed,
        >,
    },
    Steering(Box<steering::Completion>),
    Rewind(Box<rewind::Completion>),
    Conversation {
        result: Result<crate::chatstore::conversation::ConversationReply, SessionError>,
        reply: oneshot::Sender<
            Result<crate::chatstore::conversation::ConversationReply, SessionError>,
        >,
    },
    Continued {
        result: Result<Box<ChatLoad>, SessionError>,
        prompt: Option<(String, Vec<PromptPart>)>,
        reply: oneshot::Sender<Result<o7_protocol::conversation::ContinuationResult, SessionError>>,
    },
    CheckpointContext {
        result: Result<(ChatRecord, Option<String>), SessionError>,
        reply: oneshot::Sender<Result<(ChatRecord, Option<String>), SessionError>>,
    },
    Reverted {
        result: Result<o7_protocol::changes::RevertResult, SessionError>,
        reply: oneshot::Sender<Result<o7_protocol::changes::RevertResult, SessionError>>,
    },
    MemoryPrepared {
        generation: u64,
        prompt: Prompt,
        turn_id: String,
        checkpoint_ref: Option<String>,
        timing: PromptTiming,
        result: Result<Result<crate::chatstore::memory::Reply, StoreError>, JoinError>,
    },
    Checkpointed {
        generation: u64,
        prompt: Prompt,
        turn_id: String,
        timing: PromptTiming,
        capture: bool,
        result: Result<
            crate::checkpoints::Result<Option<o7_protocol::changes::CheckpointSummary>>,
            JoinError,
        >,
    },
    Organized {
        result: Result<crate::chatstore::organization::Organization, SessionError>,
        reply: oneshot::Sender<Result<crate::chatstore::organization::Organization, SessionError>>,
    },
    NativeIdentitySaved {
        generation: u64,
        native_id: String,
        result: Result<Result<(), StoreError>, WaitError>,
    },
    Prepared {
        generation: u64,
        result: Result<Result<Prepared, StartFailure>, JoinError>,
    },
    Page {
        purpose: PageFor,
        result: Result<HistoryPage, SessionError>,
    },
    OptionReply {
        generation: u64,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
        option_id: String,
        value: ConfigValue,
        result: Result<
            Result<Result<(), DriverError>, oneshot::error::RecvError>,
            tokio::time::error::Elapsed,
        >,
    },
    AutonomyReply {
        generation: u64,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
        autonomy: Autonomy,
        result: Result<
            Result<Result<AutonomyState, DriverError>, oneshot::error::RecvError>,
            tokio::time::error::Elapsed,
        >,
    },
    DriverJoined {
        generation: u64,
        result: Result<DriverExit, JoinError>,
    },
    /// A FORCE chain on a process of a retired generation ended.
    Retired(u64),
    Abort(AbortHandle),
    /// A start waiting for room got a slot (`admitted`), or gave up waiting.
    Room {
        id: u64,
        admitted: bool,
    },
    Nothing,
}

/// The agent reported work in progress with no prompt of o7's open: since when, and when
/// the queue stops waiting for its idle report.
#[derive(Debug, Clone, Copy)]
struct IdleWait {
    generation: u64,
    deadline: Instant,
    since: Millis,
    sends_at: Millis,
}

/// A prompt about to be sent.
struct Prompt {
    parts: Vec<PromptPart>,
    id: String,
    text: String,
    created_at: Millis,
}

/// Monotonic, process-local latency measurements. These never change the stored turn duration.
struct PromptTiming {
    /// Absent for a queued prompt loaded from storage: its original monotonic clock is gone.
    accepted: Option<Instant>,
    dispatch_started: Instant,
    driver_enqueued: Option<Instant>,
    first_event_seen: bool,
    first_text_seen: bool,
}

impl PromptTiming {
    fn new(accepted: Option<Instant>) -> Self {
        Self {
            accepted,
            dispatch_started: Instant::now(),
            driver_enqueued: None,
            first_event_seen: false,
            first_text_seen: false,
        }
    }

    fn trace(
        &self,
        chat_id: &str,
        prompt_id: &str,
        turn_id: Option<&str>,
        stage: &'static str,
        outcome: &'static str,
        dispatch_eligible: Option<bool>,
    ) {
        tracing::info!(
            chat_id,
            prompt_id,
            turn_id,
            stage,
            outcome,
            dispatch_eligible = ?dispatch_eligible,
            acceptance = if self.accepted.is_some() { "live" } else { "unknown_recovered" },
            since_acceptance_ms = ?self.accepted.map(elapsed_ms),
            since_dispatch_attempt_ms = elapsed_ms(self.dispatch_started),
            since_driver_enqueue_ms = ?self.driver_enqueued.map(elapsed_ms),
            "prompt timing"
        );
    }
}

/// One chat's actor.
pub(crate) struct ChatActor {
    setup_reservation: bool,
    worktree_setup: Option<o7_protocol::worktree_setup::WorktreeSetupAttempt>,
    chat_id: String,
    shared: Arc<Shared>,
    store: Arc<ChatStore>,
    book: Arc<ProcessBook>,
    spec: Option<AgentSpec>,
    chat: ChatRecord,
    status: SessionStatus,
    detail: Option<String>,
    info_dirty: bool,
    revision: u32,
    batcher: FrameBatcher,
    /// About how many bytes the batcher holds; past a quarter of the outbox bound it flushes at
    /// once, so the outbox bound (and with it the backpressure) engages during a flood.
    batched_bytes: usize,
    normalizer: Normalizer,
    subscribers: Vec<Subscriber>,
    queue: Vec<QueuedRow>,
    /// Only queued prompts accepted by this actor. Removed on dispatch or cancellation, so this
    /// stays bounded by the prompt queue; recovered rows deliberately have no fabricated clock.
    prompt_accepted: BTreeMap<String, Instant>,
    turn: Option<OpenTurn>,
    /// The turn row of the turn whose `TurnStarted` is not flushed yet.
    begun: Option<TurnRow>,
    workspace_turn: Option<crate::chatstore::workspace_activity::Turn>,
    ordinal: u32,
    outbox: VecDeque<Op>,
    outbox_bytes: usize,
    driver: Option<LiveDriver>,
    preparing: Option<Preparing>,
    progress: Option<mpsc::UnboundedReceiver<String>>,
    in_flight: FuturesUnordered<BoxFuture<'static, Done>>,
    organizing: bool,
    steering_pending: bool,
    git_reservation: Option<String>,
    checkpointing: Option<(String, AbortHandle)>,
    checkpoint_capture: bool,
    memory_revision: Option<String>,
    memory_grant: Option<crate::memory_service::MemoryGrant>,
    /// The Browser plugin's tools for this generation's agent, revoked with the generation.
    browser_grant: Option<crate::browser_service::BrowserGrant>,
    /// The Mobile plugin's tools for this session's agent, on the chat's own device.
    mobile_grant: Option<crate::mobile_service::MobileGrant>,
    /// A coordinated project's tools for this generation's agent, revoked with the generation.
    coordinator_grant: Option<crate::coordinator_service::CoordinatorGrant>,
    generation: u64,
    /// Generations force-stopped or crashed whose processes or task still wind down, with how many
    /// FORCE chains and joins each still waits for. They keep the chat's live slot and native
    /// sessions, and a new start waits for them.
    retiring: Vec<(u64, usize)>,
    /// How many generations retire, for the preparation of a start to wait on.
    winding_down: watch::Sender<usize>,
    agent_session: Option<AgentSessionRow>,
    continuation: Option<crate::chatstore::conversation::PendingContinuation>,
    remote_history: bool,
    autonomy: Autonomy,
    autonomy_state: AutonomyState,
    controls: o7_protocol::session::SessionControlPreferences,
    control_pending: Option<(String, ConfigValue)>,
    mcp_pending: bool,
    /// A native file rewind awaits the agent's reply.
    rewind_pending: bool,
    implementation_approved: bool,
    desired: BTreeMap<String, ConfigValue>,
    options: Vec<ConfigOption>,
    /// The generation whose driver sent `options` last; 0 when none of this run's has.
    options_generation: u64,
    /// The current start chose no model, so its first lists show the model the agent starts on.
    default_shown: bool,
    /// A model change sent to the driver (generation, option, model): the lists that show that
    /// model decide the thinking level the next start restores.
    model_change: Option<(u64, String, String)>,
    commands: Vec<SlashCommand>,
    /// What the running session loaded, as its agent reported it after starting.
    loaded: Option<o7_protocol::session::SessionLoaded>,
    capabilities: Option<Capabilities>,
    agent_name: Option<String>,
    agent_version: Option<String>,
    native_session_id: Option<String>,
    /// The row save outlives a driver generation, so stop/restart cannot bypass persistence.
    native_identity_pending: Option<NativeIdentitySave>,
    usage: Option<UsageInfo>,
    limits: Option<Vec<LimitWindow>>,
    last_error: Option<ErrorInfo>,
    auth: Option<AuthPrompt>,
    /// The added account (id, name) the current or last start signed in with, so a stopped chat
    /// still says which account a refusal or a limit belongs to.
    account: Option<(String, String)>,
    /// The agent's last report that its usage limit is reached, until a turn completes or it
    /// starts again.
    usage_limit: Option<o7_protocol::UsageLimit>,
    folder_state: FolderState,
    replay_check: Option<ReplayCheck>,
    needs_attention: bool,
    /// Requests the user answered lately, for a withdrawal that crossed the answer.
    answered: VecDeque<(String, String, Option<Arc<str>>)>,
    next_flush: Option<Instant>,
    force_wait: Option<(Instant, u64)>,
    stop_deadline: Option<(Instant, u64)>,
    idle_deadline: Option<Instant>,
    /// The idle grace running for the live driver, if any.
    idle_wait: Option<IdleWait>,
    /// Why the queue was last held (a restart, until the actor holds it itself).
    hold_cause: HoldCause,
    /// The queue's wait as the last flushed `SessionInfo` said it.
    queue_wait: Option<QueueWait>,
    last_activity: Instant,
    quitting: bool,
    shutdown_reply: Option<oneshot::Sender<()>>,
    /// The quit was answered: everything was handed to the store, which closes next. From now on
    /// every message that would queue store work is refused instead of being queued and lost.
    sealed: bool,
    /// A start (fresh or not) to begin once the live driver stopped.
    pending_start: Option<bool>,
    /// A terminal waiting for its preparation; answered with its launch, or why it cannot open.
    terminal_request: Option<oneshot::Sender<Result<TerminalOpened, SessionError>>>,
    /// Pi's shared-MCP extension file while its terminal runs.
    terminal_extension: Option<tempfile::NamedTempFile>,
    /// The session the chat's terminal last used, for an agent whose Chat and terminal keep
    /// sessions apart (Cursor, Hermes).
    terminal_session: Option<crate::chatstore::TerminalSessionRecord>,
    /// The generation of the terminal open now. While it is, the chat's session is the
    /// terminal's: Chat does not start, and the live slot and session claim stay held.
    terminal: Option<u64>,
    /// Where the open terminal's conversation will be read back from when it ends.
    terminal_import: Option<TerminalImport>,
    /// The current start is a Start fresh.
    starting_fresh: bool,
    /// The native session the agent returned at `Ready` is open in another chat: the start fails
    /// once the driver stopped.
    conflict: bool,
    /// A start waiting for another chat's idle agent to stop and give it its slot.
    room: Option<RoomWait>,
    /// Counts this chat's waits for room, so a finished one is told from the current one.
    rooms: u64,
    /// How a start at the limit asks this chat's agent to stop when it is the idle one.
    make_room: Arc<tokio::sync::Notify>,
    /// Quiet changes to the chat's folder under way (a commit, a setup copy): new turns and
    /// starts wait for the last one to end.
    quiet_holds: std::collections::HashSet<String>,
    /// A start asked for while a quiet change held the folder, Start fresh if `true`.
    start_after_hold: Option<bool>,
    /// What this chat last reported to the live-agent table, so it reports only changes.
    reported: Option<Reported>,
}

/// A start waiting for room.
struct RoomWait {
    id: u64,
    fresh: bool,
    abort: AbortHandle,
}

/// What an actor last told the live-agent table it was doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reported {
    Idle(Instant),
    Working,
    Waiting,
    Terminal,
    Shown(Instant),
    Stopping,
}

/// A start waits this long for an idle agent to stop: a stop that has not ended by then is forced
/// well before.
const ROOM_WAIT: Duration = Duration::from_secs(60);

impl ChatActor {
    fn new(
        chat_id: String,
        shared: Arc<Shared>,
        book: Arc<ProcessBook>,
        load: ChatLoad,
    ) -> ChatActor {
        let ChatLoad {
            worktree_setup,
            initial_model,
            remote_history,
            continuation,
            chat,
            next_seq,
            next_turn_ordinal,
            agent_session,
            queue,
            recovered_turns,
            replay_check,
            terminal_session,
        } = load;
        if recovered_turns > 0 {
            tracing::info!(
                chat_id,
                recovered_turns,
                "recovered turns a stopped actor left running"
            );
        }
        let spec = chat_connection(&shared.deps.detector, &chat, agent_session.as_ref());
        let defaults = lock(&shared.new_chat_defaults).clone();
        let autonomy = agent_session
            .as_ref()
            .map_or(defaults.autonomy, |session| session.autonomy);
        let initial_model = initial_model.or_else(|| defaults.model_for(&chat.agent_id));
        let initial_effort = defaults
            .effort_for(&chat.agent_id, initial_model.as_deref())
            .map(|value| {
                let catalog = spec.as_ref().and_then(|spec| shared.catalogs.get(spec.id));
                (effort_option_id(catalog.as_ref()), value)
            });
        let controls = controls::preferences(agent_session.as_ref());
        let desired = initial_options(agent_session.as_ref(), initial_model, initial_effort);
        let store = Arc::clone(&shared.deps.store);
        let replay_check = replay_check.filter(|_| {
            !continuation
                .as_ref()
                .is_some_and(|value| value.force_fresh || value.native_start.fork)
        });
        ChatActor {
            remote_history,
            continuation,
            batcher: FrameBatcher::new(chat_id.clone()).starting_at(next_seq),
            batched_bytes: 0,
            chat_id,
            store,
            book,
            spec,
            status: SessionStatus::Stopped,
            detail: None,
            info_dirty: false,
            revision: 0,
            normalizer: Normalizer::default(),
            subscribers: Vec::new(),
            queue,
            prompt_accepted: BTreeMap::new(),
            turn: None,
            begun: None,
            workspace_turn: None,
            ordinal: next_turn_ordinal.saturating_sub(1),
            outbox: VecDeque::new(),
            outbox_bytes: 0,
            driver: None,
            preparing: None,
            progress: None,
            in_flight: FuturesUnordered::new(),
            organizing: false,
            steering_pending: false,
            git_reservation: None,
            checkpointing: None,
            checkpoint_capture: false,
            memory_revision: None,
            memory_grant: None,
            browser_grant: None,
            mobile_grant: None,
            coordinator_grant: None,
            generation: 0,
            retiring: Vec::new(),
            winding_down: watch::Sender::new(0),
            agent_session,
            autonomy,
            autonomy_state: stopped_autonomy(autonomy, None),
            controls,
            control_pending: None,
            mcp_pending: false,
            rewind_pending: false,
            implementation_approved: false,
            desired,
            options: Vec::new(),
            options_generation: 0,
            default_shown: false,
            model_change: None,
            commands: Vec::new(),
            loaded: None,
            capabilities: None,
            agent_name: None,
            agent_version: None,
            native_session_id: None,
            native_identity_pending: None,
            usage: None,
            limits: None,
            last_error: None,
            auth: None,
            account: None,
            usage_limit: None,
            folder_state: FolderState::Ok,
            replay_check,
            needs_attention: false,
            answered: VecDeque::new(),
            next_flush: None,
            force_wait: None,
            stop_deadline: None,
            idle_deadline: None,
            idle_wait: None,
            // Every queued row of a loaded chat comes back held: o7 restarted with it.
            hold_cause: HoldCause::Restart,
            queue_wait: None,
            worktree_setup,
            setup_reservation: false,
            last_activity: Instant::now(),
            quitting: false,
            shutdown_reply: None,
            sealed: false,
            pending_start: None,
            terminal_request: None,
            terminal_extension: None,
            terminal_session,
            terminal: None,
            terminal_import: None,
            starting_fresh: false,
            conflict: false,
            room: None,
            rooms: 0,
            make_room: Arc::new(tokio::sync::Notify::new()),
            reported: None,
            quiet_holds: std::collections::HashSet::new(),
            start_after_hold: None,
            shared,
            chat,
        }
    }

    async fn run(mut self, mut mailbox: mpsc::UnboundedReceiver<ChatMsg>) {
        self.publish();
        let mut workspace_changes = self.store.workspace_changes();
        let mut catalog_changes = self.shared.catalogs.subscribe();
        let mut catalogs_open = true;
        loop {
            // A flush that is due runs before anything else is read: a timer set for "now" only
            // fires on a later tick of a busy runtime, and the outbox bound must see the flush.
            if self.next_flush.is_some_and(|at| at <= Instant::now()) {
                self.flush();
            }
            let flush_at = self.next_flush;
            let timer_at = self.next_timer();
            let reading = self.reading_events();
            let head = self.outbox.front().map(op_bytes);
            let make_room = Arc::clone(&self.make_room);
            tokio::select! {
                biased;
                changed = workspace_changes.changed() => {
                    if changed.is_err() { break; }
                    if self.require_workspace().is_err() {
                        self.hold_queue(HoldCause::WorkspaceRemoved);
                        self.stop();
                        self.set_detail(Some("This project or folder was removed. Restore it from Removed projects and folders to continue.".into()));
                    }
                },
                msg = mailbox.recv() => match msg {
                    Some(msg) => self.on_msg(msg),
                    None => break,
                },
                () = make_room.notified() => self.make_room(),
                Some(done) = self.in_flight.next(), if !self.in_flight.is_empty() => self.on_done(done),
                () = sleep_until(flush_at), if flush_at.is_some() => self.flush(),
                () = sleep_until(timer_at), if timer_at.is_some() => self.on_timer(),
                permit = self.store.reserve(head.unwrap_or(1)), if head.is_some() => match permit {
                    Ok(permit) => {
                        if let Some(op) = self.outbox.pop_front() {
                            self.outbox_bytes = self.outbox_bytes.saturating_sub(op_bytes(&op));
                            permit.send(op);
                        }
                    }
                    Err(_) => self.store_closed(),
                },
                event = next_event(&mut self.driver), if reading => match event {
                    Some(event) => self.on_driver_event(event),
                    None => self.on_events_closed(),
                },
                detail = next_progress(&mut self.progress), if self.progress.is_some() => match detail {
                    Some(detail) => self.set_detail(Some(detail)),
                    None => self.progress = None,
                },
                changed = catalog_changes.changed(), if catalogs_open => match changed {
                    Ok(()) => self.catalogs_changed(),
                    Err(_) => catalogs_open = false,
                },
            }
            let workspace_busy = self.turn.is_some()
                || self.checkpointing.is_some()
                || self.preparing.is_some()
                || matches!(
                    self.status,
                    SessionStatus::Starting
                        | SessionStatus::Running
                        | SessionStatus::Cancelling
                        | SessionStatus::Stopping
                );
            if !workspace_busy {
                self.workspace_turn = None;
            } else if self.workspace_turn.is_none() {
                self.workspace_turn = self.store.begin_workspace_turn(&self.chat_id);
            }
            self.update_idle();
            self.refresh_queue_wait();
            self.finish_shutdown();
        }
        // The manager is gone: dropping the driver's command sender closes it (a shutdown with no
        // grace), and the recorded processes die with their last handles.
        self.book.kill_all();
    }

    fn reserve_workspace(
        &mut self,
        id: String,
        restoring: bool,
        reply: oneshot::Sender<Result<(), SessionError>>,
    ) {
        let result = if self.organizing
            || self.steering_pending
            || self.driver.is_some()
            || self.preparing.is_some()
            || !self.retiring.is_empty()
            || self.checkpointing.is_some()
            || self.turn.is_some()
            // A held prompt waits for the person, so it stays; one about to go does not.
            || (!restoring && self.queue.iter().any(|row| !row.held))
            || self.pending_start.is_some()
            || self.native_identity_pending.is_some()
            || !self.quiet_holds.is_empty()
            || self.room.is_some()
            || self.terminal.is_some()
            || self.quitting
            || self.shared.is_shutting_down()
        {
            Err(SessionError::Organization(format!(
                "{} is working in this folder.",
                self.agent_label()
            )))
        } else if self.store.is_paused() {
            Err(SessionError::StorageUnavailable(
                "Wait for saving to resume before changing this workspace.".into(),
            ))
        } else {
            self.organizing = true;
            self.git_reservation = Some(id.clone());
            Ok(())
        };
        if reply.send(result).is_err() && self.git_reservation.as_deref() == Some(&id) {
            self.git_reservation = None;
            self.organizing = false;
        }
    }

    fn reading_events(&self) -> bool {
        self.driver
            .as_ref()
            .is_some_and(|driver| driver.events.is_some())
            && self.outbox_bytes < self.shared.knobs.outbox_bytes
    }

    fn next_timer(&self) -> Option<Instant> {
        [
            self.force_wait.map(|(at, _)| at),
            self.stop_deadline.map(|(at, _)| at),
            self.idle_deadline,
            self.idle_wait.map(|wait| wait.deadline),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    // ---- IPC messages -------------------------------------------------------------------------

    fn on_msg(&mut self, msg: ChatMsg) {
        if self.sealed {
            match msg {
                // Answered from memory, nothing reaches the store.
                ChatMsg::Detach { .. }
                | ChatMsg::DropOwner { .. }
                | ChatMsg::ReleaseGit { .. }
                | ChatMsg::ReleaseQuiet { .. }
                | ChatMsg::Lists { .. }
                | ChatMsg::FolderContext { .. } => {}
                // A second quit is answered at once by `refuse`; anything else would queue work
                // for a store that is closing.
                _ => {
                    msg.refuse(SessionError::ShuttingDown);
                    return;
                }
            }
        }
        match msg {
            ChatMsg::SetupChanged(attempt) => {
                if self.worktree_setup.as_ref().is_some_and(|current| {
                    current.generation > attempt.generation
                        || (current.generation == attempt.generation
                            && (matches!(
                                current.state,
                                o7_protocol::worktree_setup::WorktreeSetupState::Succeeded
                                    | o7_protocol::worktree_setup::WorktreeSetupState::Failed
                                    | o7_protocol::worktree_setup::WorktreeSetupState::Interrupted
                            ) || (current.state
                                == o7_protocol::worktree_setup::WorktreeSetupState::Running
                                && attempt.state
                                    == o7_protocol::worktree_setup::WorktreeSetupState::Awaiting)))
                }) {
                    return;
                }
                let succeeded = !attempt.blocks_agent();
                let failed = matches!(
                    attempt.state,
                    o7_protocol::worktree_setup::WorktreeSetupState::Failed
                        | o7_protocol::worktree_setup::WorktreeSetupState::Interrupted
                );
                let failure = failed.then(|| {
                    attempt.detail.clone().unwrap_or_else(|| {
                        "Setup did not finish, so the terminal did not open. Retry setup.".into()
                    })
                });
                self.worktree_setup = Some(attempt);
                if let Some(detail) = failure {
                    self.hold_queue(HoldCause::Error);
                    // A terminal waiting for setup gets its answer.
                    self.refuse_terminal(SessionError::AgentUnavailable(detail));
                }
                self.info_dirty = true;
                self.schedule_flush();
                if succeeded {
                    // A terminal waiting for setup opens now (the start becomes its launch). One
                    // closed while it waited is forgotten, and the chat starts in Chat as before.
                    if self
                        .terminal_request
                        .as_ref()
                        .is_some_and(|waiting| waiting.is_closed())
                    {
                        self.terminal_request = None;
                    }
                    if let Err(error) = self.start(false, false) {
                        self.refuse_terminal(error);
                    }
                }
            }
            ChatMsg::ReserveGit { id, reply } => self.reserve_workspace(id, false, reply),
            ChatMsg::ReserveSetup { id, reply } => {
                self.reserve_workspace(id.clone(), true, reply);
                self.setup_reservation = self.git_reservation.as_deref() == Some(&id);
            }
            ChatMsg::ReserveWorkspace {
                id,
                restoring,
                reply,
            } => self.reserve_workspace(id, restoring, reply),
            ChatMsg::ReleaseGit { id } => {
                if self.git_reservation.as_deref() == Some(&id) {
                    let setup = self.setup_reservation;
                    self.setup_reservation = false;
                    self.git_reservation = None;
                    self.organizing = false;
                    // A prompt queued meanwhile (a restore keeps the queue) goes once it may.
                    self.dispatch_next();
                    if setup
                        && self
                            .worktree_setup
                            .as_ref()
                            .is_some_and(|attempt| !attempt.blocks_agent())
                        && self.status == SessionStatus::Stopped
                    {
                        let _ = self.start(false, false);
                    }
                }
            }
            ChatMsg::HoldQuiet { id, reply } => {
                let result = self.hold_quiet(id.clone());
                if reply.send(result).is_err() {
                    self.release_quiet(&id);
                }
            }
            ChatMsg::ReleaseQuiet { id } => self.release_quiet(&id),
            ChatMsg::FolderLocated {
                reservation,
                record,
                reply,
            } => {
                if self.git_reservation.as_deref() != Some(&reservation)
                    || record.id != self.chat_id
                {
                    let _ = reply.send(Err(SessionError::Organization(
                        "Folder ownership changed. Reopen this chat before starting it.".into(),
                    )));
                } else {
                    self.chat.folder = record.folder;
                    self.chat.folder_id = record.folder_id;
                    self.folder_state = FolderState::Ok;
                    self.info_dirty = true;
                    self.schedule_flush();
                    self.publish();
                    let _ = reply.send(Ok(()));
                }
            }
            ChatMsg::Conversation { command, reply } => self.conversation_read(command, reply),
            ChatMsg::Continue {
                apply,
                prompt,
                reply,
            } => self.continue_chat(apply, prompt, reply),
            ChatMsg::CheckpointContext { turn_id, reply } => {
                self.flush();
                let barrier = if self.store.is_paused() {
                    None
                } else {
                    let (saved, barrier) = oneshot::channel();
                    self.queue_op(Op::Barrier(saved));
                    Some(barrier)
                };
                let store = Arc::clone(&self.store);
                let rt = self.shared.deps.rt.clone();
                let chat_id = self.chat_id.clone();
                self.in_flight.push(
                    async move {
                        let result = async {
                            if let Some(barrier) = barrier {
                                await_reply(&rt, barrier, Duration::from_secs(5))
                                    .await
                                    .map_err(|_| {
                                        SessionError::StorageUnavailable(
                                            "Checkpoint history is still saving. Try again.".into(),
                                        )
                                    })?;
                            }
                            store
                                .checkpoint_context(&chat_id, turn_id)
                                .await
                                .map_err(|error| storage_error(&error))
                        }
                        .await;
                        Done::CheckpointContext { result, reply }
                    }
                    .boxed(),
                );
            }
            ChatMsg::Revert {
                token,
                guard,
                reply,
            } => self.revert(token, guard, reply),
            ChatMsg::Organize { command, reply } => self.organize(command, reply),
            ChatMsg::Attach {
                sink,
                subscription_id,
                reply,
            } => self.attach(sink, subscription_id, reply),
            ChatMsg::Detach { subscription_id } => {
                self.subscribers
                    .retain(|subscriber| subscriber.id != subscription_id);
                self.shared.forget_subscription(&subscription_id);
            }
            ChatMsg::DropOwner { owner } => {
                let dropped: Vec<String> = self
                    .subscribers
                    .iter()
                    .filter(|subscriber| subscriber.owner == owner)
                    .map(|subscriber| subscriber.id.clone())
                    .collect();
                self.subscribers
                    .retain(|subscriber| subscriber.owner != owner);
                for id in dropped {
                    self.shared.forget_subscription(&id);
                }
            }
            ChatMsg::History {
                before_seq,
                max_bytes,
                reply,
            } => {
                self.flush();
                self.read_page(before_seq, max_bytes, PageFor::History { reply });
            }
            ChatMsg::Lists { reply } => drop(reply.send(Ok(self.lists()))),
            ChatMsg::FolderContext { reply } => {
                // Before any start, follow the agent's current connection.
                let spec = if self.driver.is_some() || self.agent_session.is_some() {
                    self.spec.clone()
                } else {
                    chat_connection(&self.shared.deps.detector, &self.chat, None)
                };
                drop(
                    reply.send(Ok((
                        self.chat.clone(),
                        self.driver
                            .as_ref()
                            .map(|driver| driver.folder_context.clone()),
                        spec,
                    ))),
                );
            }
            ChatMsg::Send { text, parts, reply } => {
                let ack = self.send(text, parts);
                drop(reply.send(ack));
            }
            ChatMsg::Steer {
                expected_turn_id,
                text,
                parts,
                reply,
            } => self.steer(expected_turn_id, text, parts, reply),
            ChatMsg::RewindFiles {
                turn_id,
                dry_run,
                reply,
            } => self.rewind_files(turn_id, dry_run, reply),
            ChatMsg::CancelQueued { prompt_id, reply } => {
                let result = self.cancel_queued(&prompt_id);
                drop(reply.send(result));
            }
            ChatMsg::QueuedText { prompt_id, reply } => {
                let result = self.waiting(&prompt_id).and_then(|index| {
                    editable(&self.queue[index]).map(|(text, _)| text.to_owned())
                });
                drop(reply.send(result));
            }
            ChatMsg::EditQueued {
                prompt_id,
                text,
                reply,
            } => {
                let result = self.edit_queued(&prompt_id, text);
                drop(reply.send(result));
            }
            ChatMsg::MoveQueued {
                prompt_id,
                index,
                reply,
            } => {
                let result = self.move_queued(&prompt_id, index);
                drop(reply.send(result));
            }
            ChatMsg::ResumeQueue { reply } => {
                let result = self.require_workspace().map(|()| self.resume_queue());
                drop(reply.send(result));
            }
            ChatMsg::SendNow { prompt_id, reply } => {
                let result = self.send_now(&prompt_id);
                drop(reply.send(result));
            }
            ChatMsg::Interrupt { reply } => {
                self.interrupt();
                drop(reply.send(Ok(())));
            }
            ChatMsg::ForceStop { reply } => {
                self.force_stop();
                drop(reply.send(Ok(())));
            }
            ChatMsg::Start {
                fresh,
                explicit,
                reply,
            } => {
                let result = self.start(fresh, explicit);
                drop(reply.send(result));
            }
            ChatMsg::OpenTerminal { reply } => self.open_terminal(reply),
            ChatMsg::TerminalEnded { generation } => self.terminal_ended(generation),
            ChatMsg::RestartOnAccount { agent, reply } => {
                let result = self.restart_on_account(&agent);
                drop(reply.send(result));
            }
            ChatMsg::Stop { reply } => {
                // A start asked for while the agent stops (Start fresh, say) gives way to the Stop.
                self.pending_start = None;
                self.stop();
                drop(reply.send(Ok(())));
            }
            ChatMsg::StopIfIdle { even_shown, reply } => {
                let idle = self.idles() || (even_shown && self.idles_but_shown());
                if idle {
                    self.pending_start = None;
                    self.stop();
                }
                drop(reply.send(Ok(idle)));
            }
            ChatMsg::RespondPermission {
                request_id,
                option_id,
                reply,
            } => {
                let result = self.respond_permission(&request_id, option_id);
                drop(reply.send(result));
            }
            ChatMsg::RespondUi {
                request_id,
                answer,
                reply,
            } => {
                let result = self.respond_ui(&request_id, answer);
                drop(reply.send(result));
            }
            ChatMsg::SetControl { change, reply } => self.set_control(change, reply),
            ChatMsg::McpAction {
                server_name,
                action,
                reply,
            } => self.mcp_action(server_name, action, reply),
            ChatMsg::SetOption {
                option_id,
                value,
                reply,
            } => self.set_option(option_id, value, reply),
            ChatMsg::SetAutonomy { autonomy, reply } => self.set_autonomy(autonomy, reply),
            ChatMsg::Blob { sha256, reply } => {
                let barrier = self.blob_barrier(&sha256);
                drop(reply.send(barrier));
            }
            ChatMsg::Shutdown { plan, reply } => self.shutdown(plan, reply),
            #[cfg(test)]
            ChatMsg::Panic => panic!("a test asked the chat actor to panic"),
        }
    }

    fn attach(
        &mut self,
        sink: Arc<dyn FrameSink>,
        subscription_id: String,
        reply: oneshot::Sender<Result<ChatAttach, SessionError>>,
    ) {
        let owner = sink.owner().to_owned();
        // One view per chat per webview: a new attach replaces the webview's older subscription.
        let replaced: Vec<String> = self
            .subscribers
            .iter()
            .filter(|subscriber| subscriber.owner == owner)
            .map(|subscriber| subscriber.id.clone())
            .collect();
        self.subscribers
            .retain(|subscriber| subscriber.owner != owner);
        for id in replaced {
            self.shared.forget_subscription(&id);
        }
        if self.subscribers.len() >= self.shared.deps.limits.subscribers_per_chat {
            self.shared.forget_subscription(&subscription_id);
            drop(reply.send(Err(SessionError::TooManySubscriptions)));
            return;
        }
        self.flush();
        let next_seq = self.batcher.next_seq();
        let snapshot = Box::new(self.snapshot());
        // Every frame this sink carries from now on starts at or after `next_seq`.
        self.subscribers.push(Subscriber {
            id: subscription_id.clone(),
            owner,
            sink,
        });
        self.read_page(
            next_seq,
            crate::chatstore::history::clamp_page_bytes(None),
            PageFor::Attach {
                reply,
                subscription_id,
                next_seq,
                snapshot,
            },
        );
    }

    /// Starts a page read behind every write queued so far, or from saved rows while paused.
    fn read_page(&mut self, before_seq: u32, max_bytes: usize, purpose: PageFor) {
        let limit = self.shared.deps.timings.attach_timeout;
        let read: BoxFuture<'static, Result<HistoryPage, SessionError>> = if self.store.is_paused()
        {
            let unsaved = self.outbox_first_seq(before_seq);
            let store = Arc::clone(&self.store);
            let chat_id = self.chat_id.clone();
            async move {
                let mut page = store
                    .read_saved_page(&chat_id, before_seq, max_bytes)
                    .await
                    .map_err(|error| storage_error(&error))?;
                page.unsaved_from = match (page.unsaved_from, unsaved) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                Ok(page)
            }
            .boxed()
        } else {
            let (reply, answer) = oneshot::channel();
            self.queue_op(Op::ReadPage {
                chat_id: self.chat_id.clone(),
                before_seq,
                max_bytes,
                reply,
            });
            let rt = self.shared.deps.rt.clone();
            async move {
                match await_reply(&rt, answer, limit).await {
                    Ok(Ok(page)) => Ok(page),
                    Ok(Err(error)) => Err(storage_error(&error)),
                    Err(WaitError::TimedOut) => Err(SessionError::StorageUnavailable(
                        "it did not answer in time".into(),
                    )),
                    Err(WaitError::Gone) => Err(SessionError::StorageUnavailable(
                        StoreError::Closed.to_string(),
                    )),
                }
            }
            .boxed()
        };
        self.in_flight.push(
            async move {
                Done::Page {
                    purpose,
                    result: read.await,
                }
            }
            .boxed(),
        );
    }

    /// A blob a frame named may still wait in the outbox, its `PutBlob` behind other ops: then the
    /// store knows nothing of it yet. While saving is paused that reads as not saved yet; otherwise
    /// the read waits for a barrier queued behind it. `None`: the read can go ahead.
    fn blob_barrier(
        &mut self,
        sha256: &str,
    ) -> Result<Option<oneshot::Receiver<()>>, SessionError> {
        let unsent = self
            .outbox
            .iter()
            .any(|op| matches!(op, Op::PutBlob { sha256: queued, .. } if queued == sha256));
        if !unsent {
            return Ok(None);
        }
        if self.store.is_paused() {
            return Err(SessionError::BlobMissing(BLOB_UNSAVED.to_owned()));
        }
        let (saved, barrier) = oneshot::channel();
        self.queue_op(Op::Barrier(saved));
        Ok(Some(barrier))
    }

    /// The lowest seq below `cursor` of a row still in the outbox.
    fn outbox_first_seq(&self, cursor: u32) -> Option<u32> {
        self.outbox
            .iter()
            .filter_map(|op| match op {
                Op::AppendEvents { rows, .. } | Op::BeginTurn { rows, .. } => {
                    rows.iter().map(|row| row.seq).min()
                }
                _ => None,
            })
            .filter(|seq| *seq < cursor)
            .min()
    }

    fn conversation_read(
        &mut self,
        command: crate::chatstore::conversation::ConversationCommand,
        reply: oneshot::Sender<
            Result<crate::chatstore::conversation::ConversationReply, SessionError>,
        >,
    ) {
        if command.writes() || self.store.is_paused() {
            let _ = reply.send(Err(SessionError::StorageUnavailable(
                "Wait for saving to resume before reviewing a continuation.".into(),
            )));
            return;
        }
        self.flush();
        let (saved, barrier) = oneshot::channel();
        self.queue_op(Op::Barrier(saved));
        let store = Arc::clone(&self.store);
        let rt = self.shared.deps.rt.clone();
        self.in_flight.push(
            async move {
                let result = match await_reply(&rt, barrier, Duration::from_secs(5)).await {
                    Ok(()) => store
                        .conversation(command)
                        .await
                        .map_err(|error| storage_error(&error)),
                    Err(_) => Err(SessionError::StorageUnavailable(
                        "The conversation is still saving. Try again.".into(),
                    )),
                };
                Done::Conversation { result, reply }
            }
            .boxed(),
        );
    }

    fn continue_chat(
        &mut self,
        apply: Box<crate::chatstore::conversation::ApplyContinuation>,
        prompt: Option<(String, Vec<PromptPart>)>,
        reply: oneshot::Sender<Result<o7_protocol::conversation::ContinuationResult, SessionError>>,
    ) {
        if let Err(error) = self.require_workspace() {
            let _ = reply.send(Err(error));
            return;
        }
        if self.organizing
            || self.steering_pending
            || self.driver.is_some()
            || self.preparing.is_some()
            || !self.retiring.is_empty()
            || self.checkpointing.is_some()
            || self.turn.is_some()
            || !self.queue.is_empty()
            || self.pending_start.is_some()
            // A start waiting for room is a start: it would go on over the continuation.
            || self.room.is_some()
            || self.native_identity_pending.is_some()
            || self.quitting
            || self.shared.is_shutting_down()
        {
            let _ = reply.send(Err(SessionError::Organization(
                "Stop the chat and clear queued prompts before continuing from history.".into(),
            )));
            return;
        }
        if self.store.is_paused() {
            let _ = reply.send(Err(SessionError::StorageUnavailable(
                "Wait for saving to resume before changing the conversation.".into(),
            )));
            return;
        }
        self.organizing = true;
        self.flush();
        let (saved, barrier) = oneshot::channel();
        self.queue_op(Op::Barrier(saved));
        let store = Arc::clone(&self.store);
        let rt = self.shared.deps.rt.clone();
        self.in_flight.push(
            async move {
                let result = async {
                    await_reply(&rt, barrier, Duration::from_secs(5))
                        .await
                        .map_err(|_| {
                            SessionError::StorageUnavailable(
                                "The conversation is still saving. Try again.".into(),
                            )
                        })?;
                    let result = store
                        .conversation(crate::chatstore::conversation::ConversationCommand::Apply(
                            apply,
                        ))
                        .await
                        .map_err(|error| storage_error(&error))?;
                    let crate::chatstore::conversation::ConversationReply::Applied(load) = result
                    else {
                        return Err(SessionError::Internal(
                            "The continuation result was invalid.".into(),
                        ));
                    };
                    Ok(load)
                }
                .await;
                Done::Continued {
                    result,
                    prompt,
                    reply,
                }
            }
            .boxed(),
        );
    }

    fn continued(
        &mut self,
        result: Result<Box<ChatLoad>, SessionError>,
        prompt: Option<(String, Vec<PromptPart>)>,
        reply: oneshot::Sender<Result<o7_protocol::conversation::ContinuationResult, SessionError>>,
    ) {
        self.organizing = false;
        let result = result.and_then(|load| {
            let id = load.chat.id.clone();
            if id != self.chat_id {
                self.push(O7Event::Notice { level: NoticeLevel::Info, message: format!("Created a fork linked to this conversation: {id}. This source conversation is unchanged.") }, None);
                self.flush();
                return Ok(o7_protocol::conversation::ContinuationResult { chat_id: id, prompt: None });
            }
            self.chat = load.chat;
            self.remote_history = load.remote_history;
            self.agent_session = load.agent_session;
            self.continuation = load.continuation;
            self.terminal_session = load.terminal_session;
            self.replay_check = load.replay_check.filter(|_| !self.continuation.as_ref().is_some_and(|value| value.force_fresh || value.native_start.fork));
            self.spec = chat_connection(&self.shared.deps.detector, &self.chat, self.agent_session.as_ref());
            let defaults = lock(&self.shared.new_chat_defaults).clone();
            self.autonomy = self.agent_session.as_ref().map_or(defaults.autonomy, |session| session.autonomy);
            let initial_model = load.initial_model.or_else(|| defaults.model_for(&self.chat.agent_id));
            let initial_effort = defaults.effort_for(&self.chat.agent_id, initial_model.as_deref()).map(|value| (effort_option_id(self.catalog().as_ref()), value));
            self.desired = initial_options(self.agent_session.as_ref(), initial_model, initial_effort);
            self.controls = controls::preferences(self.agent_session.as_ref());
            self.control_pending = None;
            self.autonomy_state = stopped_autonomy(self.autonomy, None);
            self.options.clear();
            self.options_generation = 0;
            self.model_change = None;
            self.commands.clear();
            self.loaded = None;
            self.capabilities = None;
            self.native_session_id = None;
            self.agent_name = None;
            self.agent_version = None;
            self.usage = None;
            self.limits = None;
            self.last_error = None;
            self.auth = None;
            self.conflict = false;
            // The continuation replaces any start still waiting for room.
            self.end_room_wait();
            self.set_status(SessionStatus::Stopped, Some("Continuation saved. The next prompt includes its visible handoff.".into()));
            self.push(O7Event::Notice { level: NoticeLevel::Info, message: format!("Continuing with {}. Earlier messages remain intact; the reviewed context will appear in the next user prompt.",self.agent_label()) }, None);
            self.flush();
            let ack = prompt.map(|(text,parts)| self.send(text,parts)).transpose()?;
            Ok(o7_protocol::conversation::ContinuationResult { chat_id: id, prompt: ack })
        });
        let _ = reply.send(result);
    }

    fn revert(
        &mut self,
        token: String,
        guard: Arc<super::changes::FolderMutation>,
        reply: oneshot::Sender<Result<o7_protocol::changes::RevertResult, SessionError>>,
    ) {
        if self.organizing
            || self.steering_pending
            || self.driver.is_some()
            || self.preparing.is_some()
            || !self.retiring.is_empty()
            || self.checkpointing.is_some()
            || self.turn.is_some()
            // A held prompt waits for the person; one about to go would run on changed files.
            || self.queue.iter().any(|row| !row.held)
            || self.room.is_some()
            || self.quitting
            || self.shared.is_shutting_down()
        {
            let _ = reply.send(Err(SessionError::Organization(format!(
                "{} is working in this chat. Wait for it to finish, or stop it, then revert again.",
                self.agent_label()
            ))));
            return;
        }
        let Some(service) = self.shared.deps.checkpoints.clone() else {
            let _ = reply.send(Err(SessionError::Organization(
                "Checkpoints are unavailable in this runtime.".into(),
            )));
            return;
        };
        let Some(folder_id) = self.chat.folder_id.clone() else {
            let _ = reply.send(Err(SessionError::Organization(
                "Locate this chat's folder before reverting files.".into(),
            )));
            return;
        };
        self.organizing = true;
        let chat_id = self.chat_id.clone();
        let folder = self.chat.folder.clone();
        self.in_flight.push(
            async move {
                let result = service
                    .revert_guarded(&chat_id, &folder_id, &folder, &token, guard)
                    .await
                    .map_err(super::changes::checkpoint_error);
                Done::Reverted { result, reply }
            }
            .boxed(),
        );
    }

    fn organize(
        &mut self,
        command: crate::chatstore::organization::OrganizationCommand,
        reply: oneshot::Sender<Result<crate::chatstore::organization::Organization, SessionError>>,
    ) {
        use crate::chatstore::organization::OrganizationCommand;
        let moving = matches!(command, OrganizationCommand::MoveChat { .. });
        if self.organizing || self.quitting || self.shared.is_shutting_down() {
            let _ = reply.send(Err(SessionError::Organization(
                "Wait for the current chat change to finish.".into(),
            )));
            return;
        }
        if moving
            && (self.steering_pending
                || self.driver.is_some()
                || self.preparing.is_some()
                || !self.retiring.is_empty()
                || self.turn.is_some()
                || !self.queue.is_empty()
                || self.native_identity_pending.is_some()
                || self.pending_start.is_some()
                || self.room.is_some())
        {
            let _ = reply.send(Err(SessionError::Organization(
                "Stop the chat and clear queued prompts before moving it.".into(),
            )));
            return;
        }
        if self.store.is_paused() {
            let _ = reply.send(Err(SessionError::StorageUnavailable(
                "Wait for chat saving to resume before changing this chat.".into(),
            )));
            return;
        }
        self.organizing = true;
        self.flush();
        let (saved, barrier) = oneshot::channel();
        self.queue_op(Op::Barrier(saved));
        let store = Arc::clone(&self.store);
        let rt = self.shared.deps.rt.clone();
        self.in_flight.push(
            async move {
                // A timeout before enqueueing the mutation is safe. Once queued, only the actual
                // transaction result unlocks the actor, so a delayed folder move cannot race start.
                let result = match await_reply(&rt, barrier, Duration::from_secs(5)).await {
                    Ok(()) => store
                        .organize(command)
                        .await
                        .map_err(|error| storage_error(&error)),
                    Err(_) => Err(SessionError::StorageUnavailable(
                        "Earlier chat changes have not finished saving.".into(),
                    )),
                };
                Done::Organized { result, reply }
            }
            .boxed(),
        );
    }

    fn lists(&self) -> SessionLists {
        SessionLists {
            revision: self.revision,
            capabilities: self.capabilities.clone(),
            options: self.shown_options(),
            commands: self.commands.clone(),
            autonomy: self.autonomy_state.clone(),
            controls: self.control_lists(),
            loaded: self.loaded.clone(),
        }
    }

    fn send(&mut self, text: String, parts: Vec<PromptPart>) -> Result<PromptAck, SessionError> {
        self.require_workspace()?;
        self.require_remote_handoff()?;
        if self.status == SessionStatus::Ready && !self.controls_valid() {
            return Err(SessionError::OptionInvalid("A saved native control is invalid or unavailable. Choose an offered setting before sending.".into()));
        }
        if self.organizing && !self.setup_reservation {
            return Err(SessionError::Organization(
                "Wait for the chat change to finish.".into(),
            ));
        }
        if self.quitting || self.shared.is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        if text.trim().is_empty() {
            return Err(SessionError::PromptEmpty);
        }
        if parts
            .iter()
            .any(|part| matches!(part, PromptPart::Image { .. }))
            && self
                .capabilities
                .as_ref()
                .is_some_and(|capabilities| !capabilities.images)
        {
            return Err(SessionError::OptionInvalid(
                "This agent does not support images. Remove the image or choose another agent."
                    .into(),
            ));
        }
        let limit = self.shared.deps.limits.max_prompt_bytes;
        let bytes = text.len()
            + self.continuation.as_ref().map_or(0, |value| {
                if value.handoff.is_empty() {
                    0
                } else {
                    value.handoff.len() + 32
                }
            });
        if bytes > limit {
            return Err(SessionError::PromptTooLarge { bytes, limit });
        }
        if self.batcher.next_seq() > SEQ_LIMIT {
            return Err(SessionError::Internal(
                "This chat is too long to continue; start a new chat.".into(),
            ));
        }
        let prompt_id = new_id();
        let now = now_millis();
        // A prompt goes at once only with nothing queued before it: held prompts keep their place.
        if self.queue.is_empty() && self.can_dispatch() {
            self.accept_prompt(&prompt_id, false);
            let queued = self.dispatch(Prompt {
                id: prompt_id.clone(),
                parts,
                text,
                created_at: now,
            });
            return Ok(PromptAck { prompt_id, queued });
        }
        let max_queue = self.shared.deps.limits.max_queue;
        if self.queue.len() >= max_queue {
            return Err(SessionError::QueueFull(max_queue));
        }
        self.accept_prompt(&prompt_id, true);
        let row = QueuedRow {
            id: prompt_id.clone(),
            parts,
            chat_id: self.chat_id.clone(),
            text,
            held: self.worktree_setup.as_ref().is_some_and(|attempt| {
                matches!(
                    attempt.state,
                    o7_protocol::worktree_setup::WorktreeSetupState::Failed
                        | o7_protocol::worktree_setup::WorktreeSetupState::Interrupted
                )
            }),
            created_at: now,
        };
        let preview = cap_chars(row.text.clone(), PREVIEW_CHARS);
        let held = row.held;
        self.queue.push(row.clone());
        self.queue_op(Op::QueuePut(row));
        self.push(
            O7Event::PromptLifecycle {
                prompt_id: prompt_id.clone(),
                state: PromptState::Queued,
                preview: Some(preview),
                held: Some(held),
            },
            None,
        );
        self.info_dirty = true;
        self.advance();
        Ok(PromptAck {
            prompt_id,
            queued: true,
        })
    }

    fn accept_prompt(&mut self, prompt_id: &str, queued: bool) {
        self.prompt_accepted
            .insert(prompt_id.to_owned(), Instant::now());
        tracing::info!(
            chat_id = %self.chat_id,
            prompt_id,
            stage = "accepted",
            queued,
            "prompt timing"
        );
    }

    /// The queue's head may go now: it is sent when the agent can take it, or the agent starts
    /// when it is stopped (a full slot shows as a card; the prompt stays queued). A held head is
    /// the person's to send or discard, so nothing behind it moves on its own.
    fn advance(&mut self) {
        if !self.queue.first().is_some_and(|row| !row.held) {
            return;
        }
        if self.can_dispatch() {
            self.dispatch_next();
        } else if matches!(
            self.status,
            SessionStatus::Stopped | SessionStatus::Failed | SessionStatus::AuthRequired
        ) {
            // Behind a quiet change the queue starts the agent once the change ends (release_quiet
            // advances it), so a message discarded meanwhile starts nothing.
            if !self.quiet_holds.is_empty() {
                return;
            }
            let _ = self.start(false, false);
        }
    }

    /// `Ready` with no turn: a prompt goes out now.
    fn can_dispatch(&self) -> bool {
        !self
            .worktree_setup
            .as_ref()
            .is_some_and(|attempt| attempt.blocks_agent())
            && !self.organizing
            && self.quiet_holds.is_empty()
            && !self.steering_pending
            && self.control_pending.is_none()
            && self.controls_valid()
            && !self.mcp_pending
            && self.checkpointing.is_none()
            && self.status == SessionStatus::Ready
            && self.turn.is_none()
            && self.native_identity_pending.is_none()
            && self.driver.as_ref().is_some_and(|driver| {
                driver.ready
                    && driver.stop == Stop::None
                    && driver.events.is_some()
                    && (driver.run_idle.is_none()
                        || (driver_idle(driver) && !self.normalizer.has_open_requests()))
            })
    }

    fn cancel_queued(&mut self, prompt_id: &str) -> Result<(), SessionError> {
        let Some(index) = self.queue.iter().position(|row| row.id == prompt_id) else {
            return Err(SessionError::RequestNotPending);
        };
        let removed = self.queue.remove(index);
        self.prompt_accepted.remove(prompt_id);
        self.queue_op(Op::QueueRemove {
            id: prompt_id.to_owned(),
        });
        self.push(
            O7Event::PromptLifecycle {
                prompt_id: prompt_id.to_owned(),
                state: PromptState::Cancelled,
                preview: None,
                held: None,
            },
            None,
        );
        self.info_dirty = true;
        self.schedule_flush();
        // Discarding the held prompt everything waited behind lets the next one go, as if sent.
        if index == 0 && removed.held {
            self.advance();
        }
        Ok(())
    }

    /// The queue position of a prompt that still waits. The prompt a checkpoint is being saved for
    /// is already on its way to the agent, so it can no longer change.
    fn waiting(&self, prompt_id: &str) -> Result<usize, SessionError> {
        if self
            .checkpointing
            .as_ref()
            .is_some_and(|(id, _)| id == prompt_id)
        {
            return Err(SessionError::OptionInvalid(
                "This message is already on its way to the agent.".into(),
            ));
        }
        self.queue
            .iter()
            .position(|row| row.id == prompt_id)
            .ok_or(SessionError::RequestNotPending)
    }

    /// Replaces the words of a queued prompt. Its attachments and mentions, its place in the queue
    /// and whether it is held stay as they are.
    fn edit_queued(
        &mut self,
        prompt_id: &str,
        text: String,
    ) -> Result<Vec<QueuedPrompt>, SessionError> {
        let index = self.waiting(prompt_id)?;
        let (_, context) = editable(&self.queue[index])?;
        let display = format!("{text}{context}");
        if display.trim().is_empty() {
            return Err(SessionError::PromptEmpty);
        }
        let limit = self.shared.deps.limits.max_prompt_bytes;
        if display.len() > limit {
            return Err(SessionError::PromptTooLarge {
                bytes: display.len(),
                limit,
            });
        }
        let row = &mut self.queue[index];
        if let Some(PromptPart::Text { text: typed }) = row.parts.first_mut() {
            *typed = text;
        }
        row.text = display;
        let row = row.clone();
        self.queue_op(Op::QueuePut(row));
        self.info_dirty = true;
        self.schedule_flush();
        Ok(self.queued_prompts())
    }

    /// Moves a queued prompt to `index` (clamped to the end). The stored order follows: the rows'
    /// times are reassigned in the new order, so a reopened chat sends them in the same order.
    fn move_queued(
        &mut self,
        prompt_id: &str,
        index: usize,
    ) -> Result<Vec<QueuedPrompt>, SessionError> {
        self.waiting(prompt_id)?;
        // `index` counts the rows the person sees; the prompt on its way keeps its own place.
        let sending = self.sending_id().map(str::to_owned).and_then(|id| {
            let at = self.queue.iter().position(|row| row.id == id)?;
            Some((at, self.queue.remove(at)))
        });
        let from = self.waiting(prompt_id)?;
        let to = index.min(self.queue.len().saturating_sub(1));
        if from != to {
            let row = self.queue.remove(from);
            self.queue.insert(to, row);
            self.save_order(None);
        }
        if let Some((at, row)) = sending {
            self.queue.insert(at.min(self.queue.len()), row);
        }
        Ok(self.queued_prompts())
    }

    /// Reassigns the rows' times in their current order, so a reopened chat sends them in the same
    /// order, and saves every row whose time changed, and `also` (a row changed otherwise).
    fn save_order(&mut self, also: Option<&str>) {
        let mut times: Vec<Millis> = self.queue.iter().map(|row| row.created_at).collect();
        times.sort_unstable();
        let mut previous: Option<Millis> = None;
        let mut changed = Vec::new();
        for (row, time) in self.queue.iter_mut().zip(times) {
            let time = previous.map_or(time, |previous| time.max(previous.saturating_add(1)));
            previous = Some(time);
            if row.created_at != time || also == Some(row.id.as_str()) {
                row.created_at = time;
                changed.push(row.clone());
            }
        }
        for row in changed {
            self.queue_op(Op::QueuePut(row));
        }
        self.info_dirty = true;
        self.schedule_flush();
    }

    /// "Send now": `prompt_id` goes next, released if it was held; the other prompts keep their
    /// place and their hold. Then whatever stands in its way that the person may end is ended:
    /// a running turn is interrupted when the agent can interrupt one, an agent that reports only
    /// background work gets the prompt at once (it queues it), and a stopped agent starts. A
    /// starting agent, an open request or a saving checkpoint still comes first.
    fn send_now(&mut self, prompt_id: &str) -> Result<Vec<QueuedPrompt>, SessionError> {
        self.require_workspace()?;
        self.require_remote_handoff()?;
        if self.organizing {
            return Err(SessionError::Organization(
                "Wait for the chat change to finish.".into(),
            ));
        }
        if self.quitting || self.shared.is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let from = self.waiting(prompt_id)?;
        let mut row = self.queue.remove(from);
        row.held = false;
        self.queue.insert(0, row);
        self.save_order(Some(prompt_id));
        if self.can_dispatch() {
            self.dispatch_next();
            return Ok(self.queued_prompts());
        }
        match self.status {
            SessionStatus::Stopped | SessionStatus::Failed | SessionStatus::AuthRequired => {
                let _ = self.start(false, false);
            }
            SessionStatus::Running | SessionStatus::Ready
                if self.checkpointing.is_none() && !self.normalizer.has_open_requests() =>
            {
                if self.turn.is_some() {
                    let interruptible = self
                        .capabilities
                        .as_ref()
                        .is_none_or(|capabilities| capabilities.interrupt);
                    if interruptible
                        && !self
                            .turn
                            .as_ref()
                            .is_some_and(|turn| turn.interrupt_requested)
                    {
                        self.interrupt_for(prompt_id);
                    }
                } else if let Some(driver) = &mut self.driver
                    && driver.ready
                    && driver.stop == Stop::None
                    && driver.run_idle == Some(false)
                {
                    tracing::info!(chat_id = %self.chat_id, "Send now: the agent still reports background work; it gets the prompt now and queues it");
                    driver.idle_assumed = true;
                    self.idle_wait = None;
                    self.refresh_run_status();
                }
            }
            _ => {}
        }
        Ok(self.queued_prompts())
    }

    /// Interrupts the running turn so `prompt_id` goes next. Unlike Esc it holds nothing: the
    /// person asked for the queue to move on.
    fn interrupt_for(&mut self, prompt_id: &str) {
        let Some((turn_id, tag)) = self.turn.as_mut().map(|turn| {
            turn.interrupt_requested = true;
            (turn.id.clone(), Arc::clone(&turn.tag))
        }) else {
            return;
        };
        tracing::info!(chat_id = %self.chat_id, prompt_id, "Send now: interrupting the running turn");
        self.send_command(DriverCommand::Interrupt);
        for event in self
            .normalizer
            .resolve_turn_requests(&turn_id, ResolvedBy::Turn)
        {
            self.push_normalized(event, Some(Arc::clone(&tag)));
        }
        self.set_status(
            SessionStatus::Cancelling,
            Some("Stopping the turn to send your next message".into()),
        );
        self.refresh_attention();
    }

    /// The prompts that wait, as the queue shows them: the one on its way (see [`Self::sending`])
    /// is not waiting, and shows in the transcript instead.
    fn queued_prompts(&self) -> Vec<QueuedPrompt> {
        let sending = self.sending_id();
        self.queue
            .iter()
            .filter(|row| Some(row.id.as_str()) != sending)
            .map(shown)
            .collect()
    }

    /// The prompt on its way to the agent: its folder checkpoint is being saved, and it goes the
    /// moment that ends. It stays a stored queued row meanwhile (a crash brings it back held), but
    /// it does not wait behind anything, so it is not part of the queue the person sees. A Stop
    /// during the save holds it, and then it waits like any held prompt.
    fn sending_id(&self) -> Option<&str> {
        let (id, _) = self.checkpointing.as_ref()?;
        self.queue
            .iter()
            .any(|row| row.id == *id && !row.held)
            .then_some(id.as_str())
    }

    fn sending(&self) -> Option<SendingPrompt> {
        let id = self.sending_id()?;
        self.queue
            .iter()
            .find(|row| row.id == id)
            .map(|row| SendingPrompt {
                prompt_id: row.id.clone(),
                text: row.text.clone(),
                parts: row.parts.clone(),
                created_at: row.created_at,
            })
    }

    fn require_workspace(&self) -> Result<(), SessionError> {
        if self.store.workspace_available(&self.chat_id) {
            Ok(())
        } else {
            Err(SessionError::Organization("This project or folder was removed. Restore it from Removed projects and folders before continuing this chat.".into()))
        }
    }

    fn resume_queue(&mut self) {
        if self.require_workspace().is_err() {
            return;
        }
        for row in &mut self.queue {
            row.held = false;
        }
        self.queue_op(Op::QueueReleaseAll {
            chat_id: self.chat_id.clone(),
        });
        self.info_dirty = true;
        self.schedule_flush();
        match self.status {
            SessionStatus::Ready => self.dispatch_next(),
            SessionStatus::Stopped | SessionStatus::Failed | SessionStatus::AuthRequired => {
                let _ = self.start(false, false);
            }
            _ => {}
        }
    }

    /// Holds every queued row. Rows typed afterwards are not held but wait behind the held ones,
    /// which go only when the person sends them (or are discarded): order is kept.
    fn hold_queue(&mut self, cause: HoldCause) {
        if self.queue.iter().all(|row| row.held) {
            return;
        }
        self.hold_cause = cause;
        let changed: Vec<_> = self
            .queue
            .iter_mut()
            .filter_map(|row| {
                if row.held {
                    return None;
                }
                row.held = true;
                Some(O7Event::PromptLifecycle {
                    prompt_id: row.id.clone(),
                    state: PromptState::Queued,
                    preview: Some(cap_chars(row.text.clone(), PREVIEW_CHARS)),
                    held: Some(true),
                })
            })
            .collect();
        self.queue_op(Op::QueueHoldAll {
            chat_id: self.chat_id.clone(),
        });
        for event in changed {
            self.push(event, None);
        }
        self.info_dirty = true;
        self.schedule_flush();
    }

    fn dispatch_next(&mut self) {
        if self.require_workspace().is_err() {
            return;
        }
        if !self.can_dispatch() {
            return;
        }
        // The head goes first; a held head waits for the person, and everything behind it with it.
        if !self.queue.first().is_some_and(|row| !row.held) {
            return;
        }
        let row = self.queue.remove(0);
        self.dispatch(Prompt {
            id: row.id,
            parts: if row.parts.is_empty() {
                vec![PromptPart::Text {
                    text: row.text.clone(),
                }]
            } else {
                row.parts
            },
            text: row.text,
            created_at: row.created_at,
        });
    }

    /// Starts a turn for `prompt`: the turn's first rows go out in one `BeginTurn` (which removes
    /// the queued row), then the prompt goes to the driver.
    fn dispatch(&mut self, prompt: Prompt) -> bool {
        let timing = PromptTiming::new(self.prompt_accepted.get(&prompt.id).copied());
        timing.trace(
            &self.chat_id,
            &prompt.id,
            None,
            "dispatch_attempt",
            "started",
            None,
        );
        self.workspace_turn = None;
        let Some(admission) = self.store.begin_workspace_turn(&self.chat_id) else {
            let row = QueuedRow {
                id: prompt.id.clone(),
                chat_id: self.chat_id.clone(),
                text: prompt.text,
                parts: prompt.parts,
                held: true,
                created_at: prompt.created_at,
            };
            let preview = cap_chars(row.text.clone(), PREVIEW_CHARS);
            let prompt_id = row.id.clone();
            // It was next, so it keeps the head.
            self.queue.insert(0, row.clone());
            self.queue_op(Op::QueuePut(row));
            self.push(
                O7Event::PromptLifecycle {
                    prompt_id,
                    state: PromptState::Queued,
                    preview: Some(preview),
                    held: Some(true),
                },
                None,
            );
            self.info_dirty = true;
            self.hold_cause = HoldCause::WorkspaceRemoved;
            self.set_detail(Some("This workspace was removed. Restore it from Removed projects and folders before resuming this saved prompt.".into()));
            self.flush();
            return true;
        };
        self.workspace_turn = Some(admission);
        let turn_id = new_id();
        let Some(checkpoints) = self.shared.deps.checkpoints.clone() else {
            timing.trace(
                &self.chat_id,
                &prompt.id,
                Some(&turn_id),
                "checkpoint_result",
                "not_configured",
                Some(true),
            );
            self.dispatch_prepared(prompt, turn_id, None, timing);
            return false;
        };
        // Stored as a queued row during workspace preparation, so a crash brings it back (held). It
        // is the next to go, so it stands at the head; it is sending, not waiting
        // ([`Self::sending_id`]).
        let row = QueuedRow {
            id: prompt.id.clone(),
            chat_id: self.chat_id.clone(),
            text: prompt.text.clone(),
            parts: prompt.parts.clone(),
            held: false,
            created_at: prompt.created_at,
        };
        self.queue.insert(0, row.clone());
        self.queue_op(Op::QueuePut(row));
        self.push(
            O7Event::PromptLifecycle {
                prompt_id: prompt.id.clone(),
                state: PromptState::Sending,
                preview: Some(cap_chars(prompt.text.clone(), PREVIEW_CHARS)),
                held: None,
            },
            None,
        );
        self.info_dirty = true;
        let chat_id = self.chat_id.clone();
        let folder = self.chat.folder.clone();
        let folder_id = self.chat.folder_id.clone();
        let captured_turn = turn_id.clone();
        let capture = self.shared.automatic_checkpoints.load(Ordering::Acquire);
        let task = self.shared.deps.rt.spawn(async move {
            let folder_id = folder_id.ok_or_else(|| {
                crate::checkpoints::CheckpointError::Message(
                    "Locate this chat's folder before sending the prompt.".into(),
                )
            })?;
            if capture {
                checkpoints
                    .capture(&chat_id, &folder_id, &folder, &captured_turn)
                    .await
                    .map(Some)
            } else {
                checkpoints
                    .prepare_workspace(&chat_id, &folder_id, &folder)
                    .await?;
                Ok(None)
            }
        });
        self.checkpoint_capture = capture;
        self.checkpointing = Some((prompt.id.clone(), task.abort_handle()));
        let generation = self.generation;
        self.in_flight.push(
            async move {
                Done::Checkpointed {
                    generation,
                    prompt,
                    turn_id,
                    timing,
                    capture,
                    result: task.await,
                }
            }
            .boxed(),
        );
        self.set_status(
            SessionStatus::Running,
            Some(if capture {
                "Saving checkpoint before sending the prompt".into()
            } else {
                "Preparing workspace before sending the prompt".into()
            }),
        );
        self.flush();
        true
    }

    fn dispatch_prepared(
        &mut self,
        prompt: Prompt,
        turn_id: String,
        checkpoint_ref: Option<String>,
        timing: PromptTiming,
    ) {
        if self.driver.is_none() {
            return;
        }
        let row = QueuedRow {
            id: prompt.id.clone(),
            chat_id: self.chat_id.clone(),
            text: prompt.text.clone(),
            parts: prompt.parts.clone(),
            held: false,
            created_at: prompt.created_at,
        };
        if !self.queue.iter().any(|row| row.id == prompt.id) {
            self.queue.insert(0, row.clone());
            self.queue_op(Op::QueuePut(row));
        }
        let store = self.store.clone();
        let command = crate::chatstore::memory::Command::Snapshot {
            chat_id: self.chat_id.clone(),
            prompt_id: prompt.id.clone(),
            previous: self.memory_revision.clone(),
            embedded_context: prompt
                .parts
                .iter()
                .skip(1)
                .rev()
                .find_map(|part| match part {
                    PromptPart::Text { text } if crate::chatstore::memory::is_context(part) => {
                        Some(text.clone())
                    }
                    _ => None,
                }),
            // An agent with a memory grant can read a note when it wants one. One without (on
            // another host, or started without tools) is sent the notes with the brief.
            tools: self.memory_grant.is_some(),
        };
        let task = self
            .shared
            .deps
            .rt
            .spawn(async move { store.memory(command).await });
        self.checkpointing = Some((prompt.id.clone(), task.abort_handle()));
        let generation = self.generation;
        self.in_flight.push(
            async move {
                Done::MemoryPrepared {
                    generation,
                    prompt,
                    turn_id,
                    checkpoint_ref,
                    timing,
                    result: task.await,
                }
            }
            .boxed(),
        );
        self.set_status(
            SessionStatus::Running,
            Some("Saving visible project memory context before sending".into()),
        );
        self.flush();
    }

    fn dispatch_memory_ready(
        &mut self,
        mut prompt: Prompt,
        turn_id: String,
        checkpoint_ref: Option<String>,
        timing: PromptTiming,
    ) {
        if self.driver.is_none() {
            return;
        }
        let continuation_prefix = if let Some(continuation) = &mut self.continuation
            && !continuation.handoff.is_empty()
        {
            let handoff = std::mem::take(&mut continuation.handoff);
            let prefix = format!("{handoff}\n\nCurrent user prompt\n");
            prompt.text = format!("{prefix}{}", prompt.text);
            prompt.parts.insert(
                0,
                PromptPart::Text {
                    text: prefix.clone(),
                },
            );
            Some(prefix)
        } else {
            None
        };
        let Some(driver) = &mut self.driver else {
            return;
        };
        let commands = driver.commands.clone();
        if driver.run_idle.is_some() {
            driver.run_idle = Some(false);
        }
        driver.idle_assumed = false;
        self.idle_wait = None;
        self.queue.retain(|row| row.id != prompt.id);
        self.prompt_accepted.remove(&prompt.id);
        self.remote_history = false;
        self.ordinal = self.ordinal.saturating_add(1);
        let tag: Arc<str> = Arc::from(turn_id.as_str());
        let started_at = now_millis();
        self.begun = Some(TurnRow {
            continuation_prefix,
            checkpoint_ref,
            parts: prompt.parts.clone(),
            id: turn_id.clone(),
            chat_id: self.chat_id.clone(),
            agent_session_id: self
                .agent_session
                .as_ref()
                .map(|session| session.id.clone()),
            ordinal: self.ordinal,
            prompt_id: prompt.id.clone(),
            first_event_seq: 0,
            started_at,
        });
        self.turn = Some(OpenTurn {
            parts: prompt.parts.clone(),
            id: turn_id.clone(),
            tag: Arc::clone(&tag),
            prompt_id: prompt.id.clone(),
            text: prompt.text.clone(),
            prompt_created_at: prompt.created_at,
            started: Instant::now(),
            timing,
            interrupt_requested: false,
            reply: ReplyHasher::default(),
        });
        let tagged = Some(tag);
        self.normalizer.begin_turn();
        self.push_normalized(
            O7Event::TurnStarted {
                turn_id: turn_id.clone(),
                prompt_id: Some(prompt.id.clone()),
            },
            tagged.clone(),
        );
        self.push_normalized(
            O7Event::UserMessageDelta {
                message_id: format!("{turn_id}.u"),
                text: prompt.text.clone(),
            },
            tagged.clone(),
        );
        self.push_normalized(
            O7Event::PromptLifecycle {
                prompt_id: prompt.id,
                state: PromptState::Started,
                preview: None,
                held: None,
            },
            tagged,
        );
        if self.chat.title.is_none() {
            let title = first_line(&prompt.text);
            if !title.is_empty() {
                self.set_title(title, TitleSource::FirstLine);
            }
        }
        self.queue_op(Op::TouchChat {
            chat_id: self.chat_id.clone(),
            at: started_at,
        });
        self.chat.updated_at = self.chat.updated_at.max(started_at);
        self.status = SessionStatus::Running;
        self.detail = None;
        self.info_dirty = true;
        self.flush();
        self.publish();
        self.last_activity = Instant::now();
        let enqueued_at = Instant::now();
        let enqueued = commands
            .send(DriverCommand::Prompt {
                turn: TurnRef { turn_id },
                parts: prompt.parts,
            })
            .is_ok();
        if let Some(turn) = &mut self.turn {
            if enqueued {
                turn.timing.driver_enqueued = Some(enqueued_at);
            }
            turn.timing.trace(
                &self.chat_id,
                &turn.prompt_id,
                Some(&turn.id),
                "driver_enqueue",
                if enqueued {
                    "enqueued"
                } else {
                    "channel_closed"
                },
                None,
            );
        }
    }

    fn set_title(&mut self, title: String, source: TitleSource) {
        if self.chat.title_source == Some(TitleSource::Manual) && source != TitleSource::Manual {
            return;
        }
        if self.chat.title.as_deref() == Some(title.as_str()) {
            return;
        }
        self.chat.title = Some(title.clone());
        self.chat.title_source = Some(source);
        self.queue_op(Op::SetTitle {
            chat_id: self.chat_id.clone(),
            title,
            source,
        });
        self.info_dirty = true;
        self.schedule_flush();
        self.publish();
    }

    fn cancel_checkpoint(&mut self) {
        if let Some((_, abort)) = &self.checkpointing {
            abort.abort();
        }
    }

    fn interrupt(&mut self) {
        if self.checkpointing.is_some() {
            self.cancel_checkpoint();
            self.hold_queue(HoldCause::Stop);
            if self.driver.as_ref().is_none_or(driver_idle) && !self.normalizer.has_open_requests()
            {
                self.set_status(
                    SessionStatus::Ready,
                    Some("Prompt held before it reached the agent".into()),
                );
                return;
            }
        }
        match self.status {
            SessionStatus::Running => {
                let turn = self.turn.as_mut().map(|turn| {
                    turn.interrupt_requested = true;
                    (turn.id.clone(), Arc::clone(&turn.tag))
                });
                self.hold_queue(HoldCause::Stop);
                self.send_command(DriverCommand::Interrupt);
                if let Some((turn_id, tag)) = &turn {
                    for event in self
                        .normalizer
                        .resolve_turn_requests(turn_id, ResolvedBy::Turn)
                    {
                        self.push_normalized(event, Some(Arc::clone(tag)));
                    }
                }
                self.set_status(
                    SessionStatus::Cancelling,
                    Some(if turn.is_some() {
                        "Stopping the turn; press Esc again to force stop".into()
                    } else {
                        "Stopping background work; press Esc again to force stop".into()
                    }),
                );
                self.refresh_attention();
            }
            SessionStatus::Cancelling | SessionStatus::Starting => self.force_stop(),
            _ => {}
        }
    }

    fn force_stop(&mut self) {
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        self.cancel_checkpoint();
        if !matches!(
            self.status,
            SessionStatus::Running
                | SessionStatus::Cancelling
                | SessionStatus::Ready
                | SessionStatus::Starting
                | SessionStatus::Stopping
        ) {
            return;
        }
        self.hold_queue(HoldCause::Stop);
        self.pending_start = None;
        // A start waiting for room gives up its wait, and a terminal waiting on that start (or on
        // the restart just cancelled) hears so.
        self.end_room_wait();
        if let Some(reply) = self.terminal_request.take() {
            drop(reply.send(Err(SessionError::OptionInvalid(
                "Opening the terminal was stopped.".into(),
            ))));
        }
        if let Some(preparing) = self.preparing.take() {
            // Dropping the preparation kills a running install or probe.
            preparing.abort.abort();
            self.progress = None;
        }
        let driver_live = self
            .driver
            .as_ref()
            .is_some_and(|driver| driver.events.is_some());
        if driver_live && self.force_wait.is_none() && self.turn.is_some() {
            let generation = self.driver.as_ref().map_or(0, |driver| driver.generation);
            if let Some(turn) = &mut self.turn {
                turn.interrupt_requested = true;
            }
            self.send_command(DriverCommand::Interrupt);
            let wait = if self.turns_survive() {
                self.shared.deps.timings.daemon_cancel_wait
            } else {
                self.shared.deps.timings.force_cancel_wait
            };
            self.force_wait = Some((Instant::now() + wait, generation));
            let detail = if self.turns_survive() {
                format!(
                    "Stopping {} turn in its gateway",
                    possessive(&self.agent_label())
                )
            } else {
                "Stopping".to_owned()
            };
            self.set_detail(Some(detail));
            return;
        }
        if self.force_wait.is_none() {
            self.force_kill();
        }
    }

    /// Force stop, step 3 on: drop the event receiver, run the FORCE chain on every recorded
    /// process, end what is still open, and reach `Stopped`.
    fn force_kill(&mut self) {
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        self.force_wait = None;
        self.stop_deadline = None;
        self.end_room_wait();
        if let Some(mut driver) = self.driver.take() {
            // No late event of this driver is ever applied.
            driver.events = None;
            let generation = driver.generation;
            let controls = self.book.close(generation);
            // Each FORCE chain and the driver's join: the generation keeps the chat's live slot and
            // native sessions until its processes are gone and its task ended.
            self.retire(generation, controls.len() + 1);
            for control in controls {
                self.in_flight.push(force_chain(generation, control));
            }
            // The join is still here, or already in `in_flight` when the event channel closed first;
            // either way its `DriverJoined` comes, at the latest once the abort below lands.
            if let Some(join) = driver.join.take() {
                self.in_flight.push(join_driver(generation, join));
            }
            let abort = driver.abort.clone();
            self.in_flight.push(
                async move {
                    tokio::time::sleep(ABORT_AFTER_KILL).await;
                    Done::Abort(abort)
                }
                .boxed(),
            );
            // Retired: its commands channel closes with it, and its exit counts as this stop.
            drop(driver.commands);
        }
        let now = now_millis();
        if let Some(turn) = self.turn.take() {
            let tag = Some(Arc::clone(&turn.tag));
            for event in self.normalizer.close_turn(&turn.id, ResolvedBy::Turn, now) {
                self.push_normalized(event, tag.clone());
            }
            self.push_normalized(
                O7Event::TurnEnded {
                    turn_id: turn.id.clone(),
                    stop_reason: StopReason::Interrupted,
                    usage: None,
                    duration_ms: Some(elapsed_ms(turn.started)),
                    recovered: false,
                },
                tag.clone(),
            );
            self.push_normalized(
                O7Event::PromptLifecycle {
                    prompt_id: turn.prompt_id.clone(),
                    state: PromptState::Cancelled,
                    preview: None,
                    held: None,
                },
                tag.clone(),
            );
            self.push(
                O7Event::Notice {
                    level: NoticeLevel::Warning,
                    message: "Force stopped: o7 ended the agent's processes.".into(),
                },
                tag.as_deref(),
            );
            if self.turns_survive() {
                let message = format!(
                    "{} may still finish this turn in its gateway; o7 asked it to cancel. Whatever it finishes shows when this chat resumes.",
                    self.agent_label()
                );
                self.push(
                    O7Event::Notice {
                        level: NoticeLevel::Warning,
                        message,
                    },
                    tag.as_deref(),
                );
            }
        }
        for event in self.normalizer.close_untagged(ResolvedBy::System, now) {
            self.push_normalized(event, None);
        }
        self.release_if_idle();
        self.set_status(SessionStatus::Stopped, None);
        self.refresh_attention();
        self.flush();
    }

    fn stop(&mut self) {
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        self.cancel_checkpoint();
        // Every Stop holds what is queued until now, one that repeats a stop in progress included: a
        // prompt typed while the agent stops is not held, and would start it again once it stopped.
        self.hold_queue(HoldCause::Stop);
        // A start waiting for room ends here, whatever the status says (a continuation may have
        // set it Stopped meanwhile), and so does one held back by a quiet change.
        self.end_room_wait();
        self.start_after_hold = None;
        match self.status {
            SessionStatus::Stopped
            | SessionStatus::Failed
            | SessionStatus::AuthRequired
            | SessionStatus::Stopping => {
                return;
            }
            _ => {}
        }
        if self.driver.is_none() {
            if let Some(preparing) = self.preparing.take() {
                preparing.abort.abort();
                self.progress = None;
            }
            if let Some(reply) = self.terminal_request.take() {
                drop(reply.send(Err(SessionError::OptionInvalid(
                    "Opening the terminal was stopped.".into(),
                ))));
            }
            self.release_if_idle();
            self.set_status(SessionStatus::Stopped, None);
            return;
        }
        if let Some(turn) = &mut self.turn {
            turn.interrupt_requested = true;
        }
        let grace = if self.turns_survive() {
            self.shared.deps.timings.daemon_cancel_wait
        } else {
            self.shared.deps.timings.stop_cancel_grace
        };
        self.graceful_stop(
            StopPlan {
                cancel_grace: grace,
                process: ShutdownPlan::STANDARD,
            },
            false,
        );
    }

    /// Sends `Shutdown`; a stop not done in time is forced.
    fn graceful_stop(&mut self, plan: StopPlan, auth: bool) {
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        let Some(driver) = &mut self.driver else {
            return;
        };
        if driver.stop != Stop::None {
            return;
        }
        driver.stop = Stop::Graceful { auth };
        let generation = driver.generation;
        let _ = driver.commands.send(DriverCommand::Shutdown { plan });
        self.stop_deadline = Some((
            Instant::now() + self.shared.deps.timings.stop_kill_after,
            generation,
        ));
        self.set_status(SessionStatus::Stopping, Some("Stopping".into()));
    }

    fn turns_survive(&self) -> bool {
        self.spec
            .as_ref()
            .is_some_and(|spec| spec.turns_survive_process)
    }

    fn agent_label(&self) -> String {
        let name = self
            .spec
            .as_ref()
            .map_or_else(|| self.chat.agent_id.clone(), |spec| spec.name.to_owned());
        o7_agents::hosts::display_name(&name, &self.chat.agent_host)
    }

    fn require_remote_handoff(&self) -> Result<(), SessionError> {
        if self.remote_history
            && !self
                .continuation
                .as_ref()
                .is_some_and(|value| !value.handoff.is_empty())
        {
            return Err(SessionError::Organization("Review Continue here before sending to an agent. This device has new remote history that its saved native session has not seen.".into()));
        }
        Ok(())
    }

    /// Starts the agent. `explicit` (`chat_start`): a full slot is an error; otherwise a card.
    /// Starts the agent again so it signs in with the account its agent starts on now, resuming the
    /// same session, then releases the held prompts: they go once it is ready. A turn in progress
    /// is never cut short for a switch.
    fn restart_on_account(&mut self, agent: &str) -> Result<(), SessionError> {
        if o7_agents::agent_of(&self.chat.agent_id) != agent {
            return Err(SessionError::OptionInvalid(
                "This chat runs another agent now.".into(),
            ));
        }
        if self.turn.is_some()
            || matches!(
                self.status,
                SessionStatus::Running | SessionStatus::Cancelling
            )
        {
            return Err(SessionError::OptionInvalid(format!(
                "Wait for {} to finish this turn, or stop it, before switching accounts.",
                self.agent_label()
            )));
        }
        self.usage_limit = None;
        self.info_dirty = true;
        if self.status == SessionStatus::Starting {
            // The start under way may have read the earlier account: begin it again.
            self.stop();
            if self.status == SessionStatus::Stopping {
                self.pending_start = Some(false);
            } else {
                self.start(false, true)?;
            }
        } else {
            self.start(false, true)?;
        }
        self.resume_queue();
        Ok(())
    }

    fn start(&mut self, fresh: bool, explicit: bool) -> Result<(), SessionError> {
        if self.terminal.is_some() {
            return Err(SessionError::OptionInvalid(
                "This chat is open in the terminal. Close it, or switch to Chat, to continue here."
                    .into(),
            ));
        }
        if let Some(attempt) = self
            .worktree_setup
            .as_ref()
            .filter(|attempt| attempt.blocks_agent())
        {
            return Err(SessionError::Organization(
                attempt.detail.clone().unwrap_or_else(|| {
                    "Setup is running. Your messages stay queued until it succeeds.".into()
                }),
            ));
        }
        self.require_workspace()?;
        self.require_remote_handoff()?;
        if self.steering_pending {
            return Err(SessionError::OptionInvalid(
                "Wait for steering delivery to finish before restarting.".into(),
            ));
        }
        let fresh = fresh
            || self
                .continuation
                .as_ref()
                .is_some_and(|value| value.force_fresh);
        if self.organizing {
            return Err(SessionError::Organization(
                "Wait for the chat change to finish.".into(),
            ));
        }
        if self.quitting || self.shared.is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        if !self.quiet_holds.is_empty()
            && matches!(
                self.status,
                SessionStatus::Stopped | SessionStatus::Failed | SessionStatus::AuthRequired
            )
        {
            // A commit or a setup copy holds the folder: the start follows once it ends.
            self.start_after_hold = Some(fresh || self.start_after_hold.unwrap_or(false));
            return Ok(());
        }
        match self.status {
            SessionStatus::Starting if self.room.is_some() => {
                // Still waiting for room: it starts fresh once it has some, if asked now.
                self.wait_for_room(fresh);
                return Ok(());
            }
            SessionStatus::Starting => {
                if fresh && !self.starting_fresh {
                    // Start fresh instead: drop this start and begin again.
                    if let Some(preparing) = self.preparing.take() {
                        preparing.abort.abort();
                        self.progress = None;
                        self.release_if_idle();
                        self.status = SessionStatus::Stopped;
                    } else {
                        self.pending_start = Some(true);
                        self.stop();
                        return Ok(());
                    }
                } else {
                    return Ok(());
                }
            }
            SessionStatus::Ready | SessionStatus::Running | SessionStatus::Cancelling => {
                // A live agent is stopped first.
                self.pending_start = Some(fresh);
                self.stop();
                return Ok(());
            }
            SessionStatus::Stopping => {
                self.pending_start = Some(fresh);
                return Ok(());
            }
            SessionStatus::Stopped | SessionStatus::Failed | SessionStatus::AuthRequired => {}
        }
        // Never a second process over a live one: a driver, a preparation or a turn still here
        // means the agent runs, whatever the status says.
        if self.driver.is_some() || self.preparing.is_some() || self.turn.is_some() {
            tracing::warn!(chat_id = %self.chat_id, status = ?self.status, "a start found its agent still running; nothing more starts");
            return Ok(());
        }
        // A saved session resumes through the connection that made it; a new one (a first start,
        // Start fresh, a switch that cannot resume) takes the agent's Connection setting.
        // A native fork resumes its source session, so it takes that session's connection.
        let detector = &self.shared.deps.detector;
        let fork = self
            .continuation
            .as_ref()
            .filter(|value| value.resume_source.is_some())
            .and_then(|value| value.connection.clone());
        let saved = match fork {
            Some(connection) => Some(connection),
            None if !fresh => self
                .agent_session
                .as_ref()
                .map(|session| session.agent_id.clone()),
            None => None,
        };
        let connection = match saved {
            Some(id) => match detector.spec(&id) {
                Some(spec) => StartConnection::Saved(Box::new(spec)),
                None => {
                    let reason = format!(
                        "This chat's saved session ran through a connection this version of o7 does not have ({id}). Start a fresh session to continue."
                    );
                    self.start_failed(ErrorKind::AgentUnavailable, reason, None, false);
                    return Ok(());
                }
            },
            None => StartConnection::New {
                agent: self.chat.agent_id.clone(),
            },
        };
        let Some(spec) = (match &connection {
            StartConnection::Saved(spec) => Some((**spec).clone()),
            StartConnection::New { agent } => detector.agent(agent),
        }) else {
            let reason = SessionError::UnknownAgent(self.chat.agent_id.clone()).to_string();
            self.start_failed(ErrorKind::AgentUnavailable, reason, None, false);
            return Ok(());
        };
        if !fresh
            && self
                .agent_session
                .as_ref()
                .is_some_and(|session| session.host != self.chat.agent_host)
        {
            self.start_failed(ErrorKind::AgentUnavailable, "The saved session belongs to a different execution host. Start a fresh session to use this host.".into(), None, false);
            return Ok(());
        }
        // At the limit, the agent idle longest stops to make room and this start waits for its
        // slot; only when none idles is the start refused, saying what the others are doing.
        match self.shared.live.admit(&self.chat_id, &self.chat.folder) {
            super::live::Admission::Admitted => {}
            super::live::Admission::Closed => return Err(SessionError::ShuttingDown),
            super::live::Admission::Waiting => {
                self.wait_for_room(fresh);
                return Ok(());
            }
            super::live::Admission::Full(occupancy) => {
                let error = too_many_agents(self.shared.live.max(), occupancy);
                if explicit {
                    return Err(error);
                }
                self.start_failed(ErrorKind::SpawnFailed, error.to_string(), None, false);
                return Ok(());
            }
        }
        if self.workspace_turn.is_none() {
            self.workspace_turn = self.store.begin_workspace_turn(&self.chat_id);
            if self.workspace_turn.is_none() {
                self.shared.live.release(&self.chat_id);
                return Err(SessionError::Organization(
                    "The workspace was removed while starting. Restore it before continuing."
                        .into(),
                ));
            }
        }
        self.generation += 1;
        self.control_pending = None;
        self.implementation_approved = false;
        let generation = self.generation;
        self.starting_fresh = fresh;
        self.last_error = None;
        self.auth = None;
        let (progress, details) = mpsc::unbounded_channel();
        self.progress = Some(details);
        let winding_down = self.winding_down.subscribe();
        let deps = &self.shared.deps;
        let task = deps.rt.spawn(prepare(
            super::workspace_folders::LaunchFolders {
                chat: self.chat.clone(),
                store: Arc::clone(&deps.store),
                worktrees: deps.worktrees.clone(),
            },
            self.chat.agent_host.clone(),
            connection,
            Arc::clone(&deps.detector),
            Arc::clone(&deps.env),
            self.shared.agent_accounts(),
            progress,
            winding_down,
        ));
        let abort = task.abort_handle();
        self.in_flight.push(
            async move {
                Done::Prepared {
                    generation,
                    result: task.await,
                }
            }
            .boxed(),
        );
        self.preparing = Some(Preparing {
            generation,
            abort,
            fresh,
        });
        self.set_status(
            SessionStatus::Starting,
            Some(format!("Checking {}", spec.name)),
        );
        Ok(())
    }

    fn on_prepared(
        &mut self,
        generation: u64,
        result: Result<Result<Prepared, StartFailure>, JoinError>,
    ) {
        if self
            .preparing
            .as_ref()
            .map(|preparing| preparing.generation)
            != Some(generation)
        {
            return;
        }
        let Some(preparing) = self.preparing.take() else {
            return;
        };
        self.progress = None;
        match result {
            Err(error) if error.is_cancelled() => {
                if let Some(reply) = self.terminal_request.take() {
                    drop(reply.send(Err(SessionError::OptionInvalid(
                        "Opening the terminal was stopped.".into(),
                    ))));
                }
            }
            Err(_) => self.start_failed(
                ErrorKind::Internal,
                "Preparing the agent failed inside o7.".into(),
                None,
                false,
            ),
            Ok(Err(failure)) => {
                if let Some(state) = failure.folder {
                    self.folder_state = state;
                }
                self.start_failed(failure.kind, failure.message, None, false);
            }
            Ok(Ok(prepared)) => {
                if !self.shared.live.set_folders(
                    &self.chat_id,
                    std::iter::once(prepared.folder_key.clone())
                        .chain(
                            prepared
                                .folder_context
                                .additional
                                .iter()
                                .map(|path| super::live::normalize_path(PathBuf::from(path))),
                        )
                        .collect(),
                ) {
                    self.start_failed(ErrorKind::AgentUnavailable, "This folder is being reverted. Wait for the file operation to finish before starting the agent.".into(), None, false);
                    return;
                }
                self.folder_state = FolderState::Ok;
                if self.terminal_request.is_some() {
                    self.spawn_terminal(prepared);
                } else {
                    self.spawn_driver(prepared, preparing.fresh);
                }
            }
        }
    }

    /// Opens the chat in its agent's own interface. It goes through `start`, so every rule a
    /// start keeps holds (setup, workspace, recovery, a remote handoff to review, the live slot,
    /// an idle agent stopping first); a turn under way must finish or be stopped first, never cut.
    fn open_terminal(&mut self, reply: oneshot::Sender<Result<TerminalOpened, SessionError>>) {
        let refuse = |reply: oneshot::Sender<Result<TerminalOpened, SessionError>>, why: &str| {
            drop(reply.send(Err(SessionError::OptionInvalid(why.into()))));
        };
        // A request whose terminal closed while it waited (for setup, say) no longer counts.
        if self
            .terminal_request
            .as_ref()
            .is_some_and(|waiting| waiting.is_closed())
        {
            self.terminal_request = None;
        }
        if self.terminal.is_some() || self.terminal_request.is_some() {
            return refuse(reply, "This chat is already open in the terminal.");
        }
        if o7_drivers::terminal::TerminalAgent::of(o7_agents::agent_of(&self.chat.agent_id))
            .is_none()
        {
            return refuse(
                reply,
                "This chat's agent has no interface of its own to open in the terminal.",
            );
        }
        if self.chat.agent_host != o7_protocol::agents::AgentHost::Native {
            return refuse(reply, "The terminal runs agents on this device only.");
        }
        if self.turn.is_some()
            || matches!(
                self.status,
                SessionStatus::Running | SessionStatus::Cancelling
            )
        {
            return refuse(
                reply,
                "Wait for the turn to finish, or stop it, before opening the terminal.",
            );
        }
        // A new worktree's setup command runs first: the terminal waits for it and opens once it
        // succeeds, instead of failing (a failed setup answers it with its reason).
        if let Some(attempt) = self
            .worktree_setup
            .as_ref()
            .filter(|attempt| attempt.blocks_agent())
        {
            if matches!(
                attempt.state,
                o7_protocol::worktree_setup::WorktreeSetupState::Failed
                    | o7_protocol::worktree_setup::WorktreeSetupState::Interrupted
            ) {
                let detail = attempt.detail.clone().unwrap_or_else(|| {
                    "Setup did not finish. Retry setup before opening the terminal.".into()
                });
                drop(reply.send(Err(SessionError::AgentUnavailable(detail))));
                return;
            }
            self.terminal_request = Some(reply);
            return;
        }
        // A new chat starts its agent as it is made. A start still preparing becomes the
        // terminal's (on_prepared); one whose agent is already starting is stopped first, and the
        // terminal starts once it has stopped (after_stop).
        if self.status == SessionStatus::Starting
            && self.preparing.is_none()
            && self.driver.is_some()
        {
            self.pending_start = Some(false);
            self.stop();
            self.terminal_request = Some(reply);
            return;
        }
        self.terminal_request = Some(reply);
        if let Err(error) = self.start(false, true) {
            self.refuse_terminal(error);
        }
    }

    /// Answers a terminal open still waiting when no start will follow for it, so the view never
    /// waits on a start that will not come.
    fn refuse_terminal(&mut self, error: SessionError) {
        if let Some(reply) = self.terminal_request.take() {
            drop(reply.send(Err(error)));
        }
    }

    /// The prepared start, as the terminal's launch: the chat's session continues (a new session
    /// of an agent that takes an ID takes one now and is saved, so Chat continues it later), with
    /// the same account, shared MCP and project memory a driver would get.
    fn spawn_terminal(&mut self, prepared: Prepared) {
        use o7_drivers::terminal::{TerminalAgent, TerminalOptions, TerminalSession};
        let Some(reply) = self.terminal_request.take() else {
            return;
        };
        let spec = (*prepared.spec).clone();
        let fail = |this: &mut Self,
                    reply: oneshot::Sender<Result<TerminalOpened, SessionError>>,
                    why: String| {
            this.memory_grant = None;
            this.browser_grant = None;
            this.mobile_grant = None;
            this.coordinator_grant = None;
            this.release_if_idle();
            this.set_status(SessionStatus::Stopped, None);
            drop(reply.send(Err(SessionError::AgentUnavailable(why))));
        };
        let Some(agent) = TerminalAgent::of(spec.agent) else {
            let why = format!(
                "{} has no interface of its own to open in the terminal.",
                spec.name
            );
            return fail(self, reply, why);
        };
        // Through an ACP adapter, the terminal runs the CLI the adapter drives.
        let adapter_env = match &spec.recipe {
            Recipe::NpmAdapter { user_binary, .. } => Some(user_binary.env),
            _ => None,
        };
        let Some(program) = o7_drivers::terminal::interactive_program(
            agent,
            &prepared.launch,
            adapter_env,
            &prepared.env.vars,
        ) else {
            let why = format!(
                "o7 could not find {}'s own interface (`{}`) on this device. Install it, or keep this chat in Chat.",
                agent.label(),
                agent.program_name()
            );
            return fail(self, reply, why);
        };
        // The connection's own arguments are its protocol's (after an npm shim's entry script,
        // which stays); an adapter's or another program's are not the interface's at all.
        let protocol: &[&str] = match &spec.recipe {
            Recipe::Binary { args, .. }
            | Recipe::QualifiedBinary { args, .. }
            | Recipe::NativeBinary { args, .. } => args,
            Recipe::NpmAdapter { .. } | Recipe::Builtin => &[],
        };
        let replaced = program != prepared.launch.program;
        let mut base = prepared.launch.clone();
        base.args =
            o7_drivers::terminal::interactive_args(&prepared.launch.args, protocol, replaced);
        base.program = program;
        // Only a session with a conversation is resumed. The one a new chat starts for Chat has
        // no record until its first message, and an agent refuses to resume a session it never
        // wrote down, so the terminal starts a new session instead (nothing is lost: it had no
        // conversation). Where the agent's own record can be looked for, it decides; otherwise
        // the turns this chat ran on the session do.
        let home = terminal_home(agent, &prepared.launch, &prepared.env);
        let ran_turns = |id: &str| {
            self.replay_check
                .as_ref()
                .is_some_and(|check| check.native_session_id == id && check.turns > 0)
        };
        // An agent that keeps its terminal's sessions apart from Chat's resumes the session its
        // terminal last used here, unless the agent's own record shows it gone.
        let resume = if agent.shares_chat_sessions() {
            self.agent_session
                .as_ref()
                .filter(|session| session.agent_id == spec.id)
                .and_then(|session| session.native_session_id.clone())
                .filter(|id| home.recorded(id).unwrap_or_else(|| ran_turns(id)))
        } else {
            self.terminal_session
                .as_ref()
                .filter(|record| record.connection == spec.id)
                .map(|record| record.session.clone())
                .filter(|id| home.recorded(id) != Some(false))
        };
        let session = match resume {
            Some(id) => TerminalSession::Resume(id),
            None if agent.names_new_sessions() => {
                TerminalSession::Named(uuid::Uuid::now_v7().to_string())
            }
            None => TerminalSession::New,
        };
        let native_scope = NativeSessionScope::local(
            spec.id,
            &self.shared.deps.device_id,
            &prepared.env,
            &prepared.launch,
            &self.chat.folder,
        );
        if let TerminalSession::Resume(id) | TerminalSession::Named(id) = &session
            && !self.shared.live.claim(&native_scope, id, &self.chat_id)
        {
            return fail(
                self,
                reply,
                "This session is already open in another chat.".into(),
            );
        }
        self.memory_grant = crate::memory_service::MemoryGrant::start(
            self.store.clone(),
            self.chat_id.clone(),
            Arc::clone(&self.shared.agent_memory_writes),
            &self.shared.deps.rt,
        )
        .ok();
        let mut servers = prepared.mcp_servers;
        if prepared.memory_mcp_available
            && let Some(helper) = self.memory_grant.as_ref().and_then(|grant| grant.mcp(None))
        {
            servers.push(helper);
        }
        self.shared.deps.redactor.set_secrets(
            servers
                .iter()
                .flat_map(|s| {
                    s.credentials
                        .env
                        .values()
                        .chain(s.credentials.headers.values())
                })
                .cloned()
                .chain(
                    prepared
                        .account
                        .as_ref()
                        .map(|account| account.token.clone()),
                )
                .collect(),
        );
        let model_id = self
            .shared
            .catalogs
            .get(spec.id)
            .map_or_else(|| "model".to_owned(), |catalog| catalog.model.id);
        let model = match self.native_desired().get(&model_id) {
            Some(ConfigValue::Choice { value }) => Some(value.clone()),
            _ => None,
        };
        let add_dirs: Vec<PathBuf> = prepared
            .folder_context
            .additional
            .iter()
            .map(PathBuf::from)
            .collect();
        // A chat that moved here from another agent opens with the visible handoff, as Chat
        // would send it with the next prompt; a resumed session already has its context.
        let first_message = if matches!(session, TerminalSession::Resume(_)) {
            None
        } else {
            self.continuation
                .as_mut()
                .map(|continuation| std::mem::take(&mut continuation.handoff))
                .filter(|handoff| !handoff.is_empty())
                .map(|handoff| {
                    format!(
                        "{handoff}

The person continues this conversation here, in your terminal. Reply only that you have the context, then wait for their next message."
                    )
                })
        };
        // Pi loads shared servers through an extension file, kept while its terminal runs.
        self.terminal_extension = if agent == TerminalAgent::Pi && !servers.is_empty() {
            o7_drivers::terminal::pi_extension().ok()
        } else {
            None
        };
        let options = TerminalOptions {
            servers: &servers,
            autonomy: self.autonomy,
            session: session.clone(),
            model,
            add_dirs: &add_dirs,
            extension: self
                .terminal_extension
                .as_ref()
                .map(tempfile::NamedTempFile::path),
            env: Some(&prepared.env.vars),
        };
        let made = o7_drivers::terminal::build(agent, &base, &options);
        if let TerminalSession::Named(native_id) = &session {
            let now = now_millis();
            let row = AgentSessionRow {
                capabilities: None,
                host: self.chat.agent_host.clone(),
                id: new_id(),
                chat_id: self.chat_id.clone(),
                agent_id: spec.id.to_owned(),
                native_session_id: Some(native_id.clone()),
                model: None,
                effort: None,
                autonomy: self.autonomy,
                config_json: serde_json::to_string(&self.desired).ok(),
                controls_json: serde_json::to_string(&self.controls).ok(),
                cwd: self.chat.folder.clone(),
                created_at: now,
                updated_at: now,
            };
            self.agent_session = Some(row.clone());
            self.queue_op(Op::InsertAgentSession(row));
            self.native_session_id = Some(native_id.clone());
        }
        // Agents read through their own commands run them with the chat's environment, without
        // what the terminal itself added (its shared servers would start for nothing).
        let cli = super::terminal_import::Cli {
            program: base.program.clone(),
            env: cli_env(&prepared.launch, &prepared.env),
        };
        self.terminal_import = Some(terminal_import_plan(home, &session, &self.chat.folder, cli));
        let generation = self.generation;
        self.terminal = Some(generation);
        self.set_status(SessionStatus::Stopped, Some("Open in the terminal".into()));
        let opened = TerminalOpened {
            spec: o7_process::SpawnSpec {
                launch: made.launch,
                folder: self.chat.folder.clone(),
                env: prepared.env,
                max_line_bytes: o7_process::MAX_LINE_BYTES,
            },
            generation,
            omitted: made.omitted,
            // Typed into the terminal, so stripped of anything that would act as keys.
            first_input: first_message.map(|text| o7_drivers::terminal::typed_text(&text)),
            exit_input: agent.exit_input().to_owned(),
        };
        if reply.send(Ok(opened)).is_err() {
            // Nobody waits for it any more (the page closed): give the session back now.
            self.terminal_ended(generation);
        }
    }

    /// The terminal ended: the session, the live slot and project memory go back, and prompts
    /// queued meanwhile go to Chat.
    /// The terminal ended: its conversation is read back first (off the actor), and the session
    /// goes back only once those turns are recorded, so a prompt queued for Chat cannot land
    /// before them.
    fn terminal_ended(&mut self, generation: u64) {
        if self.terminal != Some(generation) {
            return;
        }
        let Some(plan) = self.terminal_import.take() else {
            return self.finish_terminal(generation, None);
        };
        self.set_detail(Some(
            "Bringing the terminal's conversation into the chat".into(),
        ));
        let task = tokio::task::spawn_blocking(move || read_terminal(&plan));
        self.in_flight.push(
            async move {
                let read = task.await.unwrap_or_else(|_| TerminalRead {
                    imported: Default::default(),
                    learned: None,
                    unread: false,
                    session_known: false,
                });
                Done::TerminalRead { generation, read }
            }
            .boxed(),
        );
    }

    /// Records what the terminal said, then gives the session, the live slot and project memory
    /// back, and sends prompts queued meanwhile.
    fn finish_terminal(&mut self, generation: u64, read: Option<TerminalRead>) {
        if self.terminal != Some(generation) {
            return;
        }
        if let Some(read) = read {
            self.record_terminal(read);
        }
        self.terminal = None;
        self.terminal_extension = None;
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        self.release_if_idle();
        self.set_status(SessionStatus::Stopped, None);
        self.revision = self.revision.wrapping_add(1);
        self.info_dirty = true;
        self.schedule_flush();
        self.after_stop();
    }

    /// The terminal's turns, recorded as turns of this chat: the same events a live turn makes
    /// (started, the person's message, the agent's answer, ended), so history, Find, sync, the
    /// Dashboard and handoffs read them as any other. Tool calls and thinking stay in the agent's
    /// own file. A session the terminal started and the agent named itself is saved, so the next
    /// terminal or Chat resumes it.
    fn record_terminal(&mut self, read: TerminalRead) {
        let agent =
            o7_drivers::terminal::TerminalAgent::of(o7_agents::agent_of(&self.chat.agent_id));
        let shares = agent.is_none_or(o7_drivers::terminal::TerminalAgent::shares_chat_sessions);
        // An agent whose Chat cannot continue its terminal's session (Cursor) keeps Chat's own; the
        // terminal's turns reach it with the next Chat message instead, as a visible handoff.
        if !shares && !read.imported.turns.is_empty() {
            let first = self.ordinal.saturating_add(1);
            let turns: Vec<crate::conversation::ContextTurn> = read
                .imported
                .turns
                .iter()
                .enumerate()
                .map(|(index, turn)| crate::conversation::ContextTurn {
                    id: String::new(),
                    ordinal: first.saturating_add(u32::try_from(index).unwrap_or(u32::MAX)),
                    agent_id: self.chat.agent_id.clone(),
                    status: "completed".into(),
                    prompt: turn.prompt.clone(),
                    parts: Vec::new(),
                    answer: turn.answer.clone(),
                })
                .collect();
            let (handoff, _) = crate::conversation::handoff(
                &turns,
                None,
                "Unchanged by o7; see the folder.",
                "Not read for a terminal session.",
            );
            match &mut self.continuation {
                Some(continuation) if !continuation.handoff.is_empty() => {
                    continuation.handoff = format!(
                        "{}

{handoff}",
                        continuation.handoff
                    );
                }
                Some(continuation) => continuation.handoff = handoff,
                None => {
                    self.continuation = Some(crate::chatstore::conversation::PendingContinuation {
                        handoff,
                        force_fresh: false,
                        resume_source: None,
                        native_start: Default::default(),
                        connection: None,
                    });
                }
            }
            self.push(
                O7Event::Notice {
                    level: NoticeLevel::Info,
                    message: format!(
                        "{} keeps its terminal and Chat sessions apart, so the next Chat message includes the terminal's turns as context.",
                        agent.map_or("The agent", o7_drivers::terminal::TerminalAgent::label)
                    ),
                },
                None,
            );
        }
        if !shares && let Some(session) = read.learned.clone() {
            let record = crate::chatstore::TerminalSessionRecord {
                connection: self
                    .spec
                    .as_ref()
                    .map_or_else(|| self.chat.agent_id.clone(), |spec| spec.id.to_owned()),
                session,
            };
            self.terminal_session = Some(record.clone());
            self.queue_op(Op::SaveTerminalSession {
                chat_id: self.chat_id.clone(),
                record: Some(record),
            });
        }
        let learned = read.learned.is_some() && shares;
        if let Some(thread) = read.learned.filter(|_| shares) {
            let now = now_millis();
            let agent_id = self
                .spec
                .as_ref()
                .map_or_else(|| self.chat.agent_id.clone(), |spec| spec.id.to_owned());
            let row = AgentSessionRow {
                capabilities: None,
                host: self.chat.agent_host.clone(),
                id: new_id(),
                chat_id: self.chat_id.clone(),
                agent_id,
                native_session_id: Some(thread.clone()),
                model: None,
                effort: None,
                autonomy: self.autonomy,
                config_json: serde_json::to_string(&self.desired).ok(),
                controls_json: serde_json::to_string(&self.controls).ok(),
                cwd: self.chat.folder.clone(),
                created_at: now,
                updated_at: now,
            };
            self.agent_session = Some(row.clone());
            self.queue_op(Op::InsertAgentSession(row));
            self.native_session_id = Some(thread.clone());
            // The session has a conversation (the agent wrote it down), so the next terminal
            // resumes it, even where none of its turns could be read back.
            self.replay_check = Some(ReplayCheck {
                native_session_id: thread,
                turns: 1,
                last_reply_sha256: None,
            });
        }
        let imported = read.imported;
        if let Some(check) = &mut self.replay_check
            && self.native_session_id.as_deref() == Some(check.native_session_id.as_str())
            && !imported.turns.is_empty()
        {
            let count = u32::try_from(imported.turns.len()).unwrap_or(u32::MAX);
            check.turns = if learned {
                count
            } else {
                check.turns.saturating_add(count)
            };
        }
        if read.unread {
            let agent =
                o7_drivers::terminal::TerminalAgent::of(o7_agents::agent_of(&self.chat.agent_id))
                    .map_or("The agent", o7_drivers::terminal::TerminalAgent::label);
            let message = if read.session_known {
                format!(
                    "{agent} keeps the terminal's turns in its own session, which Chat continues. o7 cannot copy them into this chat's history."
                )
            } else {
                format!(
                    "o7 could not find the session {agent}'s terminal started, so Chat continues from this chat's history."
                )
            };
            self.push(
                O7Event::Notice {
                    level: NoticeLevel::Info,
                    message,
                },
                None,
            );
        }
        for turn in imported.turns {
            let turn_id = new_id();
            let edited_turn = turn_id.clone();
            let prompt_id = new_id();
            let tag: Arc<str> = Arc::from(turn_id.as_str());
            let tagged = Some(Arc::clone(&tag));
            self.ordinal = self.ordinal.saturating_add(1);
            self.begun = Some(TurnRow {
                continuation_prefix: None,
                checkpoint_ref: None,
                parts: vec![PromptPart::Text {
                    text: turn.prompt.clone(),
                }],
                id: turn_id.clone(),
                chat_id: self.chat_id.clone(),
                agent_session_id: self
                    .agent_session
                    .as_ref()
                    .map(|session| session.id.clone()),
                ordinal: self.ordinal,
                prompt_id: prompt_id.clone(),
                first_event_seq: 0,
                started_at: if turn.started_at > 0 {
                    turn.started_at
                } else {
                    now_millis()
                },
            });
            self.normalizer.begin_turn();
            let events = [
                O7Event::TurnStarted {
                    turn_id: turn_id.clone(),
                    prompt_id: Some(prompt_id.clone()),
                },
                O7Event::UserMessageDelta {
                    message_id: format!("{turn_id}.u"),
                    text: turn.prompt,
                },
                O7Event::PromptLifecycle {
                    prompt_id: prompt_id.clone(),
                    state: PromptState::Started,
                    preview: None,
                    held: None,
                },
            ];
            for event in events {
                self.push_normalized(event, tagged.clone());
            }
            if !turn.answer.is_empty() {
                let message_id = format!("{turn_id}.a");
                self.push_normalized(
                    O7Event::MessageDelta {
                        message_id: message_id.clone(),
                        text: turn.answer,
                    },
                    tagged.clone(),
                );
                self.push_normalized(O7Event::MessageCompleted { message_id }, tagged.clone());
            }
            for event in self
                .normalizer
                .close_turn(&turn_id, ResolvedBy::Turn, now_millis())
            {
                self.push_normalized(event, tagged.clone());
            }
            self.push_normalized(
                O7Event::TurnEnded {
                    turn_id,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                    duration_ms: u64::try_from(turn.ended_at.saturating_sub(turn.started_at))
                        .ok()
                        .filter(|ms| *ms > 0),
                    recovered: false,
                },
                tagged.clone(),
            );
            self.push_normalized(
                O7Event::PromptLifecycle {
                    prompt_id,
                    state: PromptState::Completed,
                    preview: None,
                    held: None,
                },
                tagged,
            );
            self.flush();
            // Its edits, from the agent's own record: after the turn's row, which the flush queued.
            if !turn.edits.is_empty() {
                self.queue_op(Op::RecordLineEdits {
                    chat_id: self.chat_id.clone(),
                    turn_id: edited_turn,
                    edits: turn.edits,
                });
            }
        }
        if imported.skipped > 0 || imported.truncated {
            let mut parts = Vec::new();
            if imported.skipped > 0 {
                parts.push(format!(
                    "{} {} of the terminal's session could not be read",
                    imported.skipped,
                    if imported.skipped == 1 {
                        "line"
                    } else {
                        "lines"
                    }
                ));
            }
            if imported.truncated {
                parts.push("its session was too long to bring in whole".to_owned());
            }
            self.push(
                O7Event::Notice {
                    level: NoticeLevel::Info,
                    message: format!(
                        "{}. The agent's own session file still has everything.",
                        capitalized(&parts.join(", and "))
                    ),
                },
                None,
            );
        }
    }

    /// A new chat starts its agent before its first message, and Claude Code writes a session's
    /// file only with that message: a later start (after a restart, or once the agent stopped)
    /// that resumed the session would fail with "No conversation found". A saved Claude session
    /// that ran no turn and has no file starts again under the same id instead. One that ran
    /// turns still resumes, so a missing file is reported rather than quietly forgotten.
    fn unwritten_claude_session(&self, spec: &AgentSpec, prepared: &Prepared, id: &str) -> bool {
        spec.kind == o7_protocol::DriverKind::Claude
            && self
                .replay_check
                .as_ref()
                .is_some_and(|check| check.native_session_id == id && check.turns == 0)
            && uuid::Uuid::parse_str(id).is_ok()
            && terminal_home(
                o7_drivers::terminal::TerminalAgent::Claude,
                &prepared.launch,
                &prepared.env,
            )
            .session_file(id)
            .is_none()
    }

    fn spawn_driver(&mut self, prepared: Prepared, fresh: bool) {
        self.mcp_pending = false;
        if !prepared.memory_mcp_available {
            self.push(O7Event::Notice{level:NoticeLevel::Info,message:"Project memory remains visible reference context. Optional memory tools were omitted because native MCP configuration needs review or uses Aura's reserved server name.".into()},None);
        }
        if let Some(message) = prepared.mcp_notice.clone() {
            self.push(
                O7Event::Notice {
                    level: NoticeLevel::Info,
                    message,
                },
                None,
            );
        }
        // The connection the preparation chose is the one this start runs and its session records.
        let spec = (*prepared.spec).clone();
        self.spec = Some(spec.clone());
        // A new process reports what it loaded as it opens.
        self.loaded = None;
        // A new process: its own account, and no limit until it reports one.
        let account = prepared.account.clone();
        self.account = account
            .as_ref()
            .map(|account| (account.account_id.clone(), account.name.clone()));
        self.usage_limit = None;
        let mut native_start = native_start(spec.kind, self.continuation.as_ref());
        let resume = if let Some(source) = self
            .continuation
            .as_ref()
            .and_then(|value| value.resume_source.clone())
        {
            Some(source)
        } else if fresh {
            None
        } else {
            self.agent_session
                .as_ref()
                .and_then(|session| session.native_session_id.clone())
                .and_then(|id| {
                    if self.unwritten_claude_session(&spec, &prepared, &id) {
                        // Started again under its own id: nothing was said in it to lose.
                        native_start.new_session_id = Some(id);
                        None
                    } else {
                        Some(id)
                    }
                })
        };
        let native_scope = NativeSessionScope::local(
            spec.id,
            &self.shared.deps.device_id,
            &prepared.env,
            &prepared.launch,
            &self.chat.folder,
        );
        let claim_id = if native_start.fork {
            native_start.new_session_id.as_ref()
        } else {
            resume.as_ref()
        };
        if let Some(id) = claim_id
            && !self.shared.live.claim(&native_scope, id, &self.chat_id)
        {
            let message =
                SessionError::Internal("this session is already open in another chat".into())
                    .to_string();
            self.start_failed(ErrorKind::Internal, message, None, false);
            return;
        }
        self.memory_revision = None;
        self.memory_grant =
            if matches!(self.chat.agent_host, o7_protocol::agents::AgentHost::Native) {
                crate::memory_service::MemoryGrant::start(
                    self.store.clone(),
                    self.chat_id.clone(),
                    Arc::clone(&self.shared.agent_memory_writes),
                    &self.shared.deps.rt,
                )
                .ok()
            } else {
                None
            };
        let generation = self.generation;
        let desired = self.native_desired();
        let model_id = self
            .shared
            .catalogs
            .get(spec.id)
            .map_or_else(|| "model".to_owned(), |catalog| catalog.model.id);
        // Without a chosen model, the first lists show the one the agent starts on.
        self.default_shown = !desired.contains_key(&model_id);
        let deps = &self.shared.deps;
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (events, event_rx) = event_channel();
        let spawner = Arc::new(SessionSpawner::new(
            o7_process::HostSpawner::new(
                Arc::clone(&deps.spawner),
                o7_agents::hosts::execution(&self.chat.agent_host),
            ),
            deps.logs.clone(),
            self.chat_id.clone(),
            Arc::clone(&self.book),
            generation,
        ));
        // The Browser plugin's tools (docs/superpowers/plans/2026-11-09-browser.md): for a native
        // Chat session whose agent takes Aura's own MCP servers, while the plugin is on.
        self.browser_grant = match self.shared.browser_bridge() {
            Some(bridge)
                if self.chat.agent_host == o7_protocol::agents::AgentHost::Native
                    && prepared.memory_mcp_available
                    && spec.kind != o7_protocol::DriverKind::Headless
                    && self.shared.browser_tools.load(Ordering::Acquire) =>
            {
                crate::browser_service::BrowserGrant::start(
                    bridge,
                    self.chat_id.clone(),
                    Arc::clone(&self.shared.browser_tools),
                    &self.shared.deps.rt,
                )
                .ok()
            }
            _ => None,
        };
        // The Mobile plugin's tools (docs/superpowers/plans/2026-11-21-mobile.md), under the same
        // conditions as the browser's.
        self.mobile_grant = match self.shared.mobile_bridge() {
            Some(bridge)
                if self.chat.agent_host == o7_protocol::agents::AgentHost::Native
                    && prepared.memory_mcp_available
                    && spec.kind != o7_protocol::DriverKind::Headless
                    && self.shared.mobile_tools.load(Ordering::Acquire) =>
            {
                crate::mobile_service::MobileGrant::start(
                    bridge,
                    self.chat_id.clone(),
                    Arc::clone(&self.shared.mobile_tools),
                    &self.shared.deps.rt,
                )
                .ok()
            }
            _ => None,
        };
        // A coordinated project's tools (an experiment): for its coordinator and its workers,
        // under the same conditions as the browser's.
        self.coordinator_grant = match self.shared.coordination.bridge() {
            Some(bridge)
                if prepared.coordinated
                    && self.chat.agent_host == o7_protocol::agents::AgentHost::Native
                    && prepared.memory_mcp_available
                    && spec.kind != o7_protocol::DriverKind::Headless
                    && self.shared.coordination.enabled.load(Ordering::Acquire) =>
            {
                crate::coordinator_service::CoordinatorGrant::start(
                    bridge,
                    self.chat_id.clone(),
                    Arc::clone(&self.shared.coordination.enabled),
                    &self.shared.deps.rt,
                )
                .ok()
            }
            _ => None,
        };
        let setup = SessionSetup {
            mcp_servers: {
                let mut servers = prepared.mcp_servers;
                if self.chat.agent_host == o7_protocol::agents::AgentHost::Native
                    && prepared.memory_mcp_available
                    && spec.kind != o7_protocol::DriverKind::Headless
                    && let Some(helper) =
                        self.memory_grant.as_ref().and_then(|grant| grant.mcp(None))
                {
                    servers.push(helper);
                }
                if let Some(helper) = self.browser_grant.as_ref().and_then(|grant| grant.mcp()) {
                    servers.push(helper);
                }
                if let Some(helper) = self.mobile_grant.as_ref().and_then(|grant| grant.mcp()) {
                    servers.push(helper);
                }
                if let Some(helper) = self
                    .coordinator_grant
                    .as_ref()
                    .and_then(|grant| grant.mcp())
                {
                    servers.push(helper);
                }
                deps.redactor.set_secrets(
                    servers
                        .iter()
                        .flat_map(|s| {
                            s.credentials
                                .env
                                .values()
                                .chain(s.credentials.headers.values())
                        })
                        .cloned()
                        .chain(account.as_ref().map(|account| account.token.clone()))
                        .collect(),
                );
                servers
            },
            attachment_dir: Some(deps.store.attachment_dir().to_path_buf()),
            attachment_writer: Some(Arc::new(super::images::StoredImages {
                store: Arc::clone(&deps.store),
                chat_id: self.chat_id.clone(),
                rt: deps.rt.clone(),
            })),
            chat_id: self.chat_id.clone(),
            agent_id: spec.id.to_owned(),
            dialect: spec.dialect.to_owned(),
            folder: self.chat.folder.clone(),
            agent_folder: self.chat.folder.to_string_lossy().into_owned(),
            add_dirs: prepared
                .folder_context
                .additional
                .iter()
                .map(PathBuf::from)
                .collect(),
            launch: prepared.launch,
            env: prepared.env,
            resume: resume.clone(),
            native_start,
            autonomy: self.autonomy,
            desired,
            timings: DriverTimings::STANDARD,
            client_version: deps.app_version.clone(),
        };
        let driver = deps.drivers.create(spec.kind);
        let join = deps.rt.spawn(driver.run(DriverContext {
            commands: command_rx,
            events,
            spawner,
            setup,
        }));
        self.driver = Some(LiveDriver {
            folder_context: o7_protocol::workspace::ChatFolderContext {
                active: true,
                ..prepared.folder_context
            },
            generation,
            commands,
            events: Some(event_rx),
            abort: join.abort_handle(),
            join: Some(join),
            ready: false,
            run_idle: None,
            idle_assumed: false,
            native_scope,
            stop: Stop::None,
            spawn_id: None,
        });
        let detail = if resume.is_some() {
            "Resuming the earlier session".to_owned()
        } else {
            format!("Starting {}", spec.name)
        };
        self.set_detail(Some(detail));
    }

    fn respond_permission(
        &mut self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), SessionError> {
        let Some(card) = self.normalizer.card(request_id) else {
            return Err(SessionError::RequestNotPending);
        };
        let tag = card.tag.clone();
        let title = card.request.title.clone();
        let decision = match option_id {
            Some(option_id) => {
                if !card
                    .request
                    .options
                    .iter()
                    .any(|option| option.option_id == option_id)
                {
                    return Err(SessionError::OptionInvalid(format!(
                        "\"{option_id}\" isn't one of this request's options."
                    )));
                }
                PermissionDecision::Selected { option_id }
            }
            None => PermissionDecision::Cancelled,
        };
        if self.control_lists().workflow.effective.as_deref() == Some("plan")
            && card.request.kind == Some(o7_protocol::ToolKind::SwitchMode)
            && matches!(&decision, PermissionDecision::Selected { option_id } if card.request.options.iter().any(|o| &o.option_id == option_id && matches!(o.kind, o7_protocol::PermissionOptionKind::AllowOnce | o7_protocol::PermissionOptionKind::AllowAlways)))
        {
            self.implementation_approved = true;
        }
        let outcome = match &decision {
            PermissionDecision::Selected { option_id } => {
                PermissionOutcome::Selected(option_id.clone())
            }
            PermissionDecision::Cancelled => PermissionOutcome::Cancelled,
        };
        self.send_command(DriverCommand::RespondPermission {
            request_id: request_id.to_owned(),
            outcome,
        });
        let tag = self.open_tag(tag.as_deref());
        self.remember_answer(request_id, title, tag.clone());
        self.push(
            O7Event::PermissionResolved {
                request_id: request_id.to_owned(),
                decision,
                by: ResolvedBy::User,
            },
            tag.as_deref(),
        );
        self.refresh_attention();
        self.refresh_run_status();
        Ok(())
    }

    fn respond_ui(
        &mut self,
        request_id: &str,
        answer: Option<UiAnswer>,
    ) -> Result<(), SessionError> {
        let Some(question) = self.normalizer.question(request_id) else {
            return Err(SessionError::RequestNotPending);
        };
        if let Some(answer) = &answer
            && !fits(&question.request.prompt, answer)
        {
            return Err(SessionError::OptionInvalid(
                "This answer doesn't fit the question.".into(),
            ));
        }
        let tag = question.tag.clone();
        let title = prompt_message(&question.request.prompt).to_owned();
        self.send_command(DriverCommand::RespondUi {
            request_id: request_id.to_owned(),
            answer: answer.clone(),
        });
        let tag = self.open_tag(tag.as_deref());
        self.remember_answer(request_id, title, tag.clone());
        self.push(
            O7Event::UiResolved {
                request_id: request_id.to_owned(),
                answer,
                by: ResolvedBy::User,
            },
            tag.as_deref(),
        );
        self.refresh_attention();
        self.refresh_run_status();
        Ok(())
    }

    fn remember_answer(&mut self, request_id: &str, title: String, tag: Option<Arc<str>>) {
        self.answered.push_back((request_id.to_owned(), title, tag));
        while self.answered.len() > ANSWERED_REMEMBERED {
            self.answered.pop_front();
        }
    }

    fn set_option(
        &mut self,
        option_id: String,
        value: ConfigValue,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
    ) {
        self.set_option_inner(option_id, value, reply, false);
    }

    fn set_option_inner(
        &mut self,
        option_id: String,
        value: ConfigValue,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
        native_admitted: bool,
    ) {
        if !self.options_live() {
            // Kept for the next start, as access is.
            self.set_option_waiting(option_id, value, reply);
            return;
        }
        if let Err(error) = check_option(&self.options, &option_id, &value) {
            drop(reply.send(Err(error)));
            return;
        }
        if controls::is_control(&option_id) && !native_admitted {
            let _ = reply.send(Err(SessionError::OptionInvalid(
                "Use the reviewed native control command to change this setting.".into(),
            )));
            return;
        }
        if self
            .options
            .iter()
            .any(|o| o.id == option_id && o.category == ConfigCategory::Mode)
            && (!self.controls_idle()
                || self.control_lists().workflow.effective.as_deref() == Some("plan")
                || self.options.iter().any(|o| {
                    o.id == option_id
                        && matches!(&o.kind,ConfigKind::Select { current, .. } if current == "plan")
                }))
        {
            let _ = reply.send(Err(SessionError::OptionInvalid("Change native mode only while idle; leaving Plan requires implementation approval.".into())));
            return;
        }
        let (answer, receiver) = oneshot::channel();
        let generation = self.generation;
        self.note_model_change(&option_id, &value);
        self.send_command(DriverCommand::SetOption {
            option_id: option_id.clone(),
            value: value.clone(),
            reply: answer,
        });
        self.in_flight.push(
            async move {
                Done::OptionReply {
                    generation,
                    reply,
                    option_id,
                    value,
                    result: tokio::time::timeout(OPTION_REPLY_TIMEOUT, receiver).await,
                }
            }
            .boxed(),
        );
    }

    fn mcp_action(
        &mut self,
        server_name: String,
        action: o7_protocol::mcp::McpSessionAction,
        reply: oneshot::Sender<Result<o7_protocol::mcp::McpActionResult, SessionError>>,
    ) {
        if !self.controls_idle() || self.mcp_pending {
            let _=reply.send(Err(SessionError::OptionInvalid("Wait until the agent is idle, clear queued messages and answer pending approvals before managing MCP.".into())));
            return;
        }
        if action == o7_protocol::mcp::McpSessionAction::Restart {
            let result=self.start(false,true).map(|_|o7_protocol::mcp::McpActionResult{state:"restarting".into(),message:"Restart requested. The chat resumes its native session with a new immutable MCP snapshot.".into(),authorization_url:None,servers:Vec::new()});
            let _ = reply.send(result);
            return;
        }
        let (answer, receiver) = oneshot::channel();
        let generation = self.generation;
        self.mcp_pending = true;
        self.send_command(DriverCommand::McpAction {
            server_name,
            action,
            reply: answer,
        });
        self.in_flight.push(
            async move {
                Done::McpAction {
                    generation,
                    reply,
                    result: tokio::time::timeout(OPTION_REPLY_TIMEOUT, receiver).await,
                }
            }
            .boxed(),
        );
    }

    fn set_autonomy(
        &mut self,
        autonomy: Autonomy,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
    ) {
        self.set_autonomy_inner(autonomy, reply, false);
    }

    fn set_autonomy_inner(
        &mut self,
        autonomy: Autonomy,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
        native_admitted: bool,
    ) {
        let live = self
            .driver
            .as_ref()
            .is_some_and(|driver| driver.events.is_some() && driver.stop == Stop::None);
        if !live {
            self.store_autonomy(
                autonomy,
                stopped_autonomy(autonomy, Some("Applies when the agent starts.")),
            );
            drop(reply.send(Ok(self.lists())));
            return;
        }
        if !native_admitted && !self.controls_idle() {
            let _ = reply.send(Err(SessionError::OptionInvalid("Wait until the agent is idle, clear queued messages and answer pending approvals before changing access.".into())));
            return;
        }
        if self.control_lists().workflow.effective.as_deref() == Some("plan") {
            let _ = reply.send(Err(SessionError::OptionInvalid("Leave Plan with explicit implementation approval before changing execution access.".into())));
            return;
        }
        let (answer, receiver) = oneshot::channel();
        let generation = self.generation;
        self.send_command(DriverCommand::SetAutonomy {
            autonomy,
            reply: answer,
        });
        self.in_flight.push(
            async move {
                Done::AutonomyReply {
                    generation,
                    reply,
                    autonomy,
                    result: tokio::time::timeout(OPTION_REPLY_TIMEOUT, receiver).await,
                }
            }
            .boxed(),
        );
    }

    /// Records the requested level and its state, persists it, and answers what the new state
    /// would auto-answer.
    fn store_autonomy(&mut self, autonomy: Autonomy, state: AutonomyState) {
        self.autonomy = autonomy;
        self.controls.access = None;
        let controls_json = serde_json::to_string(&self.controls).ok();
        if let Some(session) = &mut self.agent_session {
            session.autonomy = autonomy;
            session.controls_json = controls_json.clone();
            let id = session.id.clone();
            self.queue_op(Op::UpdateAgentSession {
                id,
                patch: AgentSessionPatch {
                    capabilities: None,
                    native_session_id: None,
                    model: None,
                    effort: None,
                    autonomy: Some(autonomy),
                    config_json: None,
                    controls_json,
                    updated_at: now_millis(),
                },
            });
        }
        self.apply_autonomy_state(state);
    }

    fn apply_autonomy_state(&mut self, state: AutonomyState) {
        self.autonomy = state.requested;
        self.autonomy_state = state;
        self.revision = self.revision.wrapping_add(1);
        self.info_dirty = true;
        self.schedule_flush();
        self.answer_open_cards();
    }

    /// Every open card the current autonomy state would auto-answer is answered now.
    fn answer_open_cards(&mut self) {
        let cards: Vec<OpenCard> = self.normalizer.cards().to_vec();
        for card in cards {
            if let Decision::AutoAllow { option_id } = decide(&self.autonomy_state, self.planning_freely(), &card.request) {
                self.auto_answer(&card.request, option_id, card.tag.as_deref());
            }
        }
        self.refresh_attention();
    }

    fn auto_answer(&mut self, request: &PermissionRequest, option_id: String, tag: Option<&str>) {
        self.send_command(DriverCommand::RespondPermission {
            request_id: request.request_id.clone(),
            outcome: PermissionOutcome::Selected(option_id.clone()),
        });
        let tag = self.open_tag(tag);
        self.push(
            O7Event::PermissionResolved {
                request_id: request.request_id.clone(),
                decision: PermissionDecision::Selected { option_id },
                by: ResolvedBy::Auto,
            },
            tag.as_deref(),
        );
    }

    fn shutdown(&mut self, plan: StopPlan, reply: oneshot::Sender<()>) {
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        self.cancel_checkpoint();
        self.quitting = true;
        self.pending_start = None;
        if self.room.is_some() {
            self.end_room_wait();
            self.release_if_idle();
            self.set_status(SessionStatus::Stopped, None);
        }
        if let Some(preparing) = self.preparing.take() {
            preparing.abort.abort();
            self.progress = None;
            self.release_if_idle();
            self.set_status(SessionStatus::Stopped, None);
        }
        let stoppable = self
            .driver
            .as_ref()
            .is_some_and(|driver| driver.stop == Stop::None);
        if stoppable {
            self.hold_queue(HoldCause::Restart);
            if let Some(turn) = &mut self.turn {
                turn.interrupt_requested = true;
            }
            let grace = if self.turns_survive() {
                plan.cancel_grace
                    .max(self.shared.deps.timings.daemon_cancel_wait)
            } else {
                plan.cancel_grace
            };
            self.graceful_stop(
                StopPlan {
                    cancel_grace: grace,
                    process: plan.process,
                },
                false,
            );
        }
        self.shutdown_reply = Some(reply);
        self.flush();
    }

    /// Replies to a quit once the driver ended and everything was handed to the store. Work still
    /// in flight (a page read, an option's answer, a process winding down) can queue more ops when
    /// it completes, so the reply waits for it too; `shutdown_all`'s deadline bounds the wait.
    /// After the reply the actor is sealed (see [`Self::on_msg`]).
    fn finish_shutdown(&mut self) {
        if self.shutdown_reply.is_none()
            || self.steering_pending
            || self.organizing
            || self.checkpointing.is_some()
            || self.driver.is_some()
            || self.preparing.is_some()
            || !self.in_flight.is_empty()
            || !self.outbox.is_empty()
        {
            return;
        }
        if !self.batcher.is_empty() || self.info_dirty {
            self.flush();
            if !self.outbox.is_empty() {
                return;
            }
        }
        if let Some(reply) = self.shutdown_reply.take() {
            self.sealed = true;
            let _ = reply.send(());
        }
    }

    // ---- In-flight completions ----------------------------------------------------------------

    fn on_done(&mut self, done: Done) {
        match done {
            Done::Steering(completion) => self.steering_done(*completion),
            Done::Rewind(completion) => self.rewind_done(*completion),
            Done::Conversation { result, reply } => {
                drop(reply.send(result));
            }
            Done::Continued {
                result,
                prompt,
                reply,
            } => self.continued(result, prompt, reply),
            Done::CheckpointContext { result, reply } => {
                drop(reply.send(result));
            }
            Done::Reverted { result, reply } => {
                self.organizing = false;
                if let Ok(report) = &result {
                    self.push(
                        O7Event::Notice {
                            level: if report.error.is_some() {
                                NoticeLevel::Warning
                            } else {
                                NoticeLevel::Info
                            },
                            message: format!(
                                "Reverted {} {}. {}",
                                report.applied.len(),
                                if report.applied.len() == 1 {
                                    "file"
                                } else {
                                    "files"
                                },
                                report
                                    .error
                                    .as_deref()
                                    .unwrap_or("Original files were retained for recovery.")
                            ),
                        },
                        None,
                    );
                    self.flush();
                }
                let _ = reply.send(result);
            }
            Done::MemoryPrepared {
                generation,
                mut prompt,
                turn_id,
                checkpoint_ref,
                timing,
                result,
            } => {
                if generation != self.generation
                    || !self
                        .checkpointing
                        .as_ref()
                        .is_some_and(|(id, _)| *id == prompt.id)
                {
                    return;
                }
                self.checkpointing = None;
                if self
                    .driver
                    .as_ref()
                    .is_some_and(|driver| driver.ready && driver.stop == Stop::None)
                {
                    self.set_status(SessionStatus::Ready, None);
                }
                let eligible = self.can_dispatch()
                    && self
                        .queue
                        .iter()
                        .any(|row| row.id == prompt.id && !row.held)
                    && self.require_workspace().is_ok();
                match result {
                    Ok(Ok(crate::chatstore::memory::Reply::Snapshot(snapshot))) if eligible => {
                        if let Some(snapshot) = snapshot {
                            self.memory_revision = snapshot.revision;
                            if !snapshot.already_attached && !snapshot.visible_text.is_empty() {
                                prompt.text.push_str(&snapshot.visible_text);
                                prompt.parts.push(PromptPart::Text {
                                    text: snapshot.visible_text,
                                });
                            }
                        }
                        self.dispatch_memory_ready(prompt, turn_id, checkpoint_ref, timing);
                    }
                    Ok(Ok(_)) => {}
                    Err(error) if error.is_cancelled() => {}
                    failed => {
                        self.hold_queue(HoldCause::Error);
                        self.set_detail(Some(format!("Project memory context could not be saved. The prompt is held for retry: {}", match failed { Ok(Err(e))=>e.to_string(),_=>"memory preparation stopped".into() })));
                        self.flush();
                    }
                }
            }
            Done::Checkpointed {
                generation,
                prompt,
                turn_id,
                timing,
                capture,
                result,
            } => {
                if !self
                    .checkpointing
                    .as_ref()
                    .is_some_and(|(id, _)| *id == prompt.id)
                {
                    timing.trace(
                        &self.chat_id,
                        &prompt.id,
                        Some(&turn_id),
                        "checkpoint_result",
                        "stale_checkpoint",
                        Some(false),
                    );
                    return;
                }
                self.checkpointing = None;
                // The prompt is no longer on its way: it went, or it waits in the queue (held).
                self.info_dirty = true;
                self.schedule_flush();
                if generation != self.generation {
                    timing.trace(
                        &self.chat_id,
                        &prompt.id,
                        Some(&turn_id),
                        "checkpoint_result",
                        "stale_generation",
                        Some(false),
                    );
                    return;
                }
                if self
                    .driver
                    .as_ref()
                    .is_some_and(|driver| driver.ready && driver.stop == Stop::None)
                    && self.turn.is_none()
                {
                    let idle = self.driver.as_ref().is_some_and(|driver| {
                        driver.run_idle.is_none()
                            || (driver_idle(driver) && !self.normalizer.has_open_requests())
                    });
                    self.set_status(
                        if idle {
                            SessionStatus::Ready
                        } else {
                            SessionStatus::Running
                        },
                        None,
                    );
                }
                let eligible = matches!(&result, Ok(Ok(_)))
                    && self.can_dispatch()
                    && self
                        .queue
                        .iter()
                        .any(|row| row.id == prompt.id && !row.held);
                let outcome = match &result {
                    Ok(Ok(Some(_))) => "saved",
                    Ok(Ok(None)) => "disabled",
                    Err(error) if error.is_cancelled() => "cancelled",
                    _ => "failed",
                };
                timing.trace(
                    &self.chat_id,
                    &prompt.id,
                    Some(&turn_id),
                    "checkpoint_result",
                    outcome,
                    Some(eligible),
                );
                match result {
                    Ok(Ok(checkpoint)) => {
                        if eligible {
                            if let Some(checkpoint) = &checkpoint
                                && !checkpoint.warnings.is_empty()
                            {
                                self.push(
                                    O7Event::Notice {
                                        level: NoticeLevel::Warning,
                                        message: format!(
                                            "Checkpoint saved with exclusions: {}",
                                            checkpoint.warnings.join("; ")
                                        ),
                                    },
                                    None,
                                );
                            }
                            self.dispatch_prepared(
                                prompt,
                                turn_id,
                                checkpoint.map(|saved| saved.id),
                                timing,
                            );
                        }
                    }
                    Err(error) if error.is_cancelled() => {}
                    failed => {
                        self.hold_queue(HoldCause::Error);
                        let message = match failed {
                            Ok(Err(error)) => error.to_string(),
                            _ if capture => "Saving the checkpoint stopped unexpectedly.".into(),
                            _ => "Preparing the workspace stopped unexpectedly.".into(),
                        };
                        self.push(
                            O7Event::Notice {
                                level: NoticeLevel::Error,
                                message: format!(
                                    "The prompt was held before reaching the agent: {message}"
                                ),
                            },
                            None,
                        );
                        self.set_detail(Some(if capture {
                            "Checkpoint failed; resolve the problem and resume queued prompts".into()
                        } else {
                            "Workspace recovery failed; resolve the problem and resume queued prompts".into()
                        }));
                    }
                }
                self.dispatch_next();
            }
            Done::Organized { result, reply } => {
                self.organizing = false;
                if let Ok(organization) = &result
                    && let Some(record) = organization
                        .chats
                        .iter()
                        .find(|record| record.chat.id == self.chat_id)
                {
                    self.chat.folder = record.chat.folder.clone();
                    self.chat.folder_id = record.chat.folder_id.clone();
                    // A live automatic title can arrive while the transaction is in flight.
                    // Only the manual title is authoritative over that newer live state.
                    if record.chat.title_source == Some(TitleSource::Manual) {
                        self.chat.title = record.chat.title.clone();
                        self.chat.title_source = record.chat.title_source;
                    }
                    self.chat.updated_at = self.chat.updated_at.max(record.chat.updated_at);
                    self.info_dirty = true;
                    self.schedule_flush();
                    self.publish();
                }
                let _ = reply.send(result);
                self.dispatch_next();
            }
            Done::NativeIdentitySaved {
                generation,
                native_id,
                result,
            } => {
                let current = self
                    .native_identity_pending
                    .as_ref()
                    .is_some_and(|pending| {
                        pending.generation == generation && pending.native_id == native_id
                    });
                if current && let Some(pending) = self.native_identity_pending.take() {
                    if matches!(result, Ok(Ok(()))) {
                        self.refresh_run_status();
                        self.dispatch_next();
                    } else {
                        if let Some(session) = &mut self.agent_session
                            && session.id == pending.row_id
                        {
                            session.native_session_id = pending.previous_id.clone();
                        }
                        if self.native_session_id.as_ref() == Some(&native_id) {
                            self.native_session_id = pending.previous_id.clone();
                        }
                        if let Some(check) = &mut self.replay_check
                            && check.native_session_id == native_id
                        {
                            if let Some(previous) = pending.previous_id {
                                check.native_session_id = previous;
                            } else {
                                self.replay_check = None;
                            }
                        }
                        self.reject_native_identity(ErrorKind::Storage, "The native session id could not be saved. Restart the session after saving is available.".into());
                    }
                }
            }
            Done::McpAction {
                generation,
                reply,
                result,
            } => {
                if generation != self.generation {
                    let _ = reply.send(Err(SessionError::SessionNotReady));
                    return;
                }
                self.mcp_pending = false;
                let result=match result {Ok(Ok(result))=>result.map_err(|e|SessionError::OptionInvalid(e.message)),_=>Err(SessionError::OptionInvalid("The agent did not confirm the MCP operation. Refresh its status before retrying.".into()))};
                let _ = reply.send(result);
                self.dispatch_next();
            }
            Done::Prepared { generation, result } => self.on_prepared(generation, result),
            Done::Page { purpose, result } => self.on_page(purpose, result),
            Done::OptionReply {
                generation,
                reply,
                option_id,
                value,
                result,
            } => {
                if generation != self.generation {
                    let _ = reply.send(Err(SessionError::SessionNotReady));
                    return;
                }
                if controls::is_control(&option_id) {
                    for _ in 0..o7_drivers::driver::DRIVER_EVENT_CAPACITY {
                        let Some(event) = self
                            .driver
                            .as_mut()
                            .and_then(|d| d.events.as_mut())
                            .and_then(|events| events.try_recv().ok())
                        else {
                            break;
                        };
                        self.on_driver_event(event);
                    }
                }
                if !matches!(result, Ok(Ok(Ok(()))))
                    && self
                        .model_change
                        .as_ref()
                        .is_some_and(|(_, id, _)| *id == option_id)
                {
                    self.model_change = None;
                }
                let answer = match result {
                    Ok(Ok(Ok(()))) => {
                        if controls::is_control(&option_id) {
                            self.control_applied(&option_id, &value);
                        } else {
                            // The driver's lists follow its answer: the answer shows the choice.
                            self.show_applied(&option_id, &value);
                            self.desired.insert(option_id.clone(), value);
                            self.persist_desired();
                        }
                        Ok(self.lists())
                    }
                    Ok(Ok(Err(error))) => {
                        Err(SessionError::OptionInvalid(self.redact(&error.message)))
                    }
                    Ok(Err(_)) => Err(SessionError::SessionNotReady),
                    Err(_) => Err(SessionError::OptionInvalid(
                        "The agent didn't answer in time; the option is unchanged.".into(),
                    )),
                };
                if controls::is_control(&option_id) {
                    self.control_pending = None;
                    self.revision = self.revision.wrapping_add(1);
                    self.info_dirty = true;
                    self.schedule_flush();
                }
                drop(reply.send(answer.map(|_| self.lists())));
                self.dispatch_next();
            }
            Done::AutonomyReply {
                generation,
                reply,
                autonomy,
                result,
            } => {
                if generation != self.generation {
                    let _ = reply.send(Err(SessionError::SessionNotReady));
                    return;
                }
                let state = match result {
                    Ok(Ok(Ok(state))) => state,
                    Ok(Err(_)) => {
                        stopped_autonomy(autonomy, Some("Applies when the agent starts."))
                    }
                    Ok(Ok(Err(error))) => refused_autonomy(
                        autonomy,
                        &self.agent_label(),
                        &self.redact(&error.message),
                    ),
                    Err(_) => {
                        refused_autonomy(autonomy, &self.agent_label(), "it didn't answer in time")
                    }
                };
                self.store_autonomy(autonomy, state);
                if self.control_pending.as_ref().is_some_and(|(id,value)| id == o7_drivers::controls::ACCESS && matches!(value,ConfigValue::Choice { value } if value.starts_with("legacy:"))) { self.control_pending = None; self.revision=self.revision.wrapping_add(1);self.info_dirty=true;self.schedule_flush(); }
                drop(reply.send(Ok(self.lists())));
                self.dispatch_next();
            }
            Done::DriverJoined { generation, result } => self.on_driver_joined(generation, result),
            Done::Retired(generation) => self.retired_one(generation),
            Done::Abort(abort) => abort.abort(),
            Done::Room { id, admitted } => self.room_done(id, admitted),
            Done::Nothing => {}
            Done::TerminalRead { generation, read } => self.finish_terminal(generation, Some(read)),
        }
    }

    fn on_page(&mut self, purpose: PageFor, result: Result<HistoryPage, SessionError>) {
        match purpose {
            PageFor::History { reply } => drop(reply.send(result)),
            PageFor::Attach {
                reply,
                subscription_id,
                next_seq,
                snapshot,
            } => match result {
                Ok(history) => {
                    let attach = ChatAttach {
                        subscription_id: subscription_id.clone(),
                        next_seq,
                        snapshot: *snapshot,
                        history,
                    };
                    if reply.send(Ok(attach)).is_err() {
                        // The caller gave up (its bound passed): nobody reads this subscription.
                        self.subscribers
                            .retain(|subscriber| subscriber.id != subscription_id);
                        self.shared.forget_subscription(&subscription_id);
                    }
                }
                Err(error) => {
                    self.subscribers
                        .retain(|subscriber| subscriber.id != subscription_id);
                    self.shared.forget_subscription(&subscription_id);
                    drop(reply.send(Err(error)));
                }
            },
        }
    }

    fn persist_desired(&mut self) {
        let Some(session) = &mut self.agent_session else {
            return;
        };
        let json = serde_json::to_string(&self.desired).unwrap_or_else(|_| "{}".into());
        session.config_json = Some(json.clone());
        let id = session.id.clone();
        self.queue_op(Op::UpdateAgentSession {
            id,
            patch: AgentSessionPatch {
                controls_json: None,
                capabilities: None,
                native_session_id: None,
                model: None,
                effort: None,
                autonomy: None,
                config_json: Some(json),
                updated_at: now_millis(),
            },
        });
    }

    // ---- Driver events ------------------------------------------------------------------------

    fn on_driver_event(&mut self, event: DriverEvent) {
        self.last_activity = Instant::now();
        match event {
            DriverEvent::Status(status) => self.on_driver_status(status),
            DriverEvent::RunState { idle } => {
                if let Some(driver) = &mut self.driver {
                    driver.run_idle = Some(idle);
                    if idle {
                        driver.idle_assumed = false;
                        self.idle_wait = None;
                    }
                }
            }
            DriverEvent::Ready(info) => self.on_ready(info),
            DriverEvent::NativeSessionId(native_id) => self.on_native_session_id(native_id),
            DriverEvent::Event { turn, event } => {
                if matches!(event, O7Event::SteeringStatus { .. }) {
                    return;
                }
                // The Processes page lists every chat's tasks without opening the chat.
                if let O7Event::TaskUpdated(task) = &event {
                    self.book.note_task(task);
                }
                // Readiness, usage and empty deltas are not a response to the prompt. Only
                // correlate actual output with the matching open turn, never background events.
                let meaningful = match &event {
                    O7Event::MessageDelta { text, .. } | O7Event::ThoughtDelta { text, .. } => {
                        !text.is_empty()
                    }
                    O7Event::MessageCompleted { .. }
                    | O7Event::ThoughtCompleted { .. }
                    | O7Event::ToolCall(_)
                    | O7Event::ToolUpdate(_)
                    | O7Event::PlanUpdated { .. }
                    | O7Event::Notice { .. }
                    | O7Event::Error { .. } => true,
                    _ => false,
                };
                if meaningful {
                    self.observe_prompt_output(
                        turn.as_deref(),
                        event.kind(),
                        matches!(&event, O7Event::MessageDelta { .. }),
                    );
                }
                let tag = self.open_tag(turn.as_deref());
                match &event {
                    O7Event::MessageDelta { text, .. } => {
                        if let (Some(open), Some(_)) = (&mut self.turn, &tag) {
                            open.reply.update(text);
                        }
                    }
                    O7Event::UsageUpdated(usage) => self.usage = Some(usage.clone()),
                    O7Event::LimitsUpdated { windows } => {
                        // The account this process runs on reported them: the book keeps them
                        // for that account, so its usage shows even after this chat moves on.
                        self.shared.agent_usage.record(
                            o7_agents::agent_of(&self.chat.agent_id),
                            self.account.as_ref().map(|(id, _)| id.as_str()),
                            windows,
                            now_millis(),
                        );
                        self.limits = Some(windows.clone());
                    }
                    _ => {}
                }
                self.push(event, tag.as_deref());
            }
            DriverEvent::TurnEnded {
                turn_id,
                stop_reason,
                usage,
                error,
            } => {
                self.observe_prompt_output(Some(&turn_id), "turn_ended", false);
                self.on_turn_ended(&turn_id, stop_reason, usage, error);
            }
            DriverEvent::Permission { turn, request } => {
                self.observe_prompt_output(turn.as_deref(), "permission_requested", false);
                self.on_permission(turn.as_deref(), request)
            }
            DriverEvent::PermissionWithdrawn { request_id } => {
                self.on_withdrawn(&request_id, false)
            }
            DriverEvent::UiRequest { turn, request } => {
                self.observe_prompt_output(turn.as_deref(), "ui_request", false);
                self.on_question(turn.as_deref(), request);
            }
            DriverEvent::UiWithdrawn { request_id } => self.on_withdrawn(&request_id, true),
            DriverEvent::Commands(commands) => {
                self.commands = commands;
                self.revision = self.revision.wrapping_add(1);
                self.info_dirty = true;
                self.schedule_flush();
            }
            DriverEvent::Loaded(loaded) => {
                self.loaded = Some(loaded.bounded());
                self.revision = self.revision.wrapping_add(1);
                self.info_dirty = true;
                self.schedule_flush();
            }
            DriverEvent::Options(options) => {
                self.take_options(options);
                self.observe_native_controls();
                self.revision = self.revision.wrapping_add(1);
                self.info_dirty = true;
                self.schedule_flush();
            }
            DriverEvent::Autonomy(state) => self.apply_autonomy_state(state),
            DriverEvent::Title(title) => {
                if let Some(title) = title
                    .map(|title| first_line(&title))
                    .filter(|title| !title.is_empty())
                {
                    self.set_title(title, TitleSource::Agent);
                }
            }
            DriverEvent::AuthRequired(mut prompt) => {
                // An added account's token was refused. The vendor's sign-in would sign in the
                // agent's own account, so it is not offered; another account, or a new token, is.
                if let Some((_, name)) = &self.account {
                    prompt.message = format!(
                        "{} refused the {name} account's token. Tokens last a year and can be revoked. Continue on another account, or remove {name} and add it again.",
                        self.agent_label()
                    );
                    prompt.commands.clear();
                }
                self.auth = Some(prompt);
            }
            DriverEvent::UsageLimit(limit) => {
                if self.usage_limit.as_ref() != Some(&limit) {
                    self.usage_limit = Some(limit);
                    self.info_dirty = true;
                    self.schedule_flush();
                }
            }
        }
        self.refresh_run_status();
    }

    fn observe_prompt_output(
        &mut self,
        turn_id: Option<&str>,
        event_kind: &'static str,
        assistant_text: bool,
    ) {
        let Some(turn) = &mut self.turn else {
            return;
        };
        if turn_id != Some(turn.id.as_str()) || turn.timing.driver_enqueued.is_none() {
            return;
        }
        if !turn.timing.first_event_seen {
            turn.timing.first_event_seen = true;
            turn.timing.trace(
                &self.chat_id,
                &turn.prompt_id,
                Some(&turn.id),
                "first_driver_event",
                event_kind,
                None,
            );
        }
        if assistant_text && !turn.timing.first_text_seen {
            turn.timing.first_text_seen = true;
            turn.timing.trace(
                &self.chat_id,
                &turn.prompt_id,
                Some(&turn.id),
                "first_assistant_text",
                event_kind,
                None,
            );
        }
    }

    fn on_native_session_id(&mut self, native_id: String) {
        let Some(driver) = &mut self.driver else {
            return;
        };
        if !driver.ready
            || driver.stop != Stop::None
            || self.native_session_id.as_ref() == Some(&native_id)
        {
            return;
        }
        if native_id.is_empty() || native_id.chars().count() > 96 {
            self.reject_native_identity(
                ErrorKind::ProtocolError,
                "The agent reported an invalid native session id.".into(),
            );
            return;
        }
        if !self
            .shared
            .live
            .claim(&driver.native_scope, &native_id, &self.chat_id)
        {
            self.reject_native_identity(
                ErrorKind::Internal,
                SessionError::Internal("this session is already open in another chat".into())
                    .to_string(),
            );
            return;
        }
        let generation = driver.generation;
        let Some(session) = &mut self.agent_session else {
            self.reject_native_identity(
                ErrorKind::Internal,
                "The native session has no saved session row.".into(),
            );
            return;
        };
        let now = now_millis();
        self.native_identity_pending = Some(NativeIdentitySave {
            generation,
            native_id: native_id.clone(),
            row_id: session.id.clone(),
            previous_id: session.native_session_id.clone(),
        });
        session.native_session_id = Some(native_id.clone());
        session.updated_at = now;
        let row_id = session.id.clone();
        self.native_session_id = Some(native_id.clone());
        self.replay_check
            .get_or_insert_with(|| ReplayCheck {
                native_session_id: native_id.clone(),
                turns: 0,
                last_reply_sha256: None,
            })
            .native_session_id = native_id.clone();
        let (saved, acknowledgement) = oneshot::channel();
        self.queue_op(Op::SaveNativeIdentity {
            id: row_id,
            native_id: native_id.clone(),
            updated_at: now,
            reply: saved,
        });
        self.in_flight.push(
            async move {
                Done::NativeIdentitySaved {
                    generation,
                    native_id,
                    // A timed-out write may still commit later. Keep the gate until the
                    // writer acknowledges its result, even across driver stop/restart.
                    result: acknowledgement.await.map_err(|_| WaitError::Gone),
                }
            }
            .boxed(),
        );
        self.revision = self.revision.wrapping_add(1);
        self.info_dirty = true;
        self.schedule_flush();
    }

    fn reject_native_identity(&mut self, kind: ErrorKind, message: String) {
        self.hold_queue(HoldCause::Error);
        self.graceful_stop(
            StopPlan {
                cancel_grace: Duration::ZERO,
                process: ShutdownPlan::STANDARD,
            },
            false,
        );
        self.conflict = true;
        if let Some(turn_id) = self.turn.as_ref().map(|turn| turn.id.clone()) {
            self.on_turn_ended(
                &turn_id,
                StopReason::Error,
                None,
                Some(DriverError {
                    kind,
                    message,
                    detail: None,
                }),
            );
        } else {
            self.last_error = Some(ErrorInfo {
                kind,
                message: message.clone(),
                detail: None,
            });
            self.push(
                O7Event::Error {
                    kind,
                    message,
                    detail: None,
                    recoverable: true,
                },
                None,
            );
        }
    }

    /// The open turn's tag when `turn` names it; anything else is stored outside turns.
    fn open_tag(&self, turn: Option<&str>) -> Option<Arc<str>> {
        let turn = turn?;
        match &self.turn {
            Some(open) if open.id == turn => Some(Arc::clone(&open.tag)),
            _ => {
                tracing::debug!(chat_id = %self.chat_id, turn, "an event names a turn that is not open; it is stored outside turns");
                None
            }
        }
    }

    fn on_driver_status(&mut self, status: DriverStatus) {
        if self.status != SessionStatus::Starting {
            return;
        }
        let name = self.agent_label();
        let detail = match status {
            DriverStatus::Spawning | DriverStatus::Initializing => format!("Starting {name}"),
            DriverStatus::Opening => {
                if self
                    .agent_session
                    .as_ref()
                    .is_some_and(|session| session.native_session_id.is_some())
                    && !self.starting_fresh
                {
                    "Resuming the earlier session".to_owned()
                } else {
                    format!("Starting {name}")
                }
            }
            DriverStatus::Configuring => "Applying your settings".to_owned(),
            DriverStatus::Closing => "Stopping".to_owned(),
            DriverStatus::Ready | DriverStatus::Prompting | DriverStatus::Cancelling => return,
        };
        self.set_detail(Some(detail));
    }

    fn on_ready(&mut self, info: ReadyInfo) {
        let Some(spec) = self.spec.clone() else {
            return;
        };
        let Some(driver) = &mut self.driver else {
            return;
        };
        if driver.ready || driver.stop != Stop::None {
            return;
        }
        if info.native_session_id.as_ref().is_some_and(|native_id| {
            !self
                .shared
                .live
                .claim(&driver.native_scope, native_id, &self.chat_id)
        }) {
            // Two processes on one native session would interleave one transcript.
            let message =
                SessionError::Internal("this session is already open in another chat".into())
                    .to_string();
            self.last_error = Some(ErrorInfo {
                kind: ErrorKind::Internal,
                message: message.clone(),
                detail: None,
            });
            self.push(
                O7Event::Error {
                    kind: ErrorKind::Internal,
                    message,
                    detail: None,
                    recoverable: true,
                },
                None,
            );
            self.graceful_stop(
                StopPlan {
                    cancel_grace: Duration::ZERO,
                    process: ShutdownPlan::STANDARD,
                },
                false,
            );
            self.conflict = true;
            return;
        }
        driver.ready = true;
        if let Some(continuation) = &mut self.continuation {
            continuation.force_fresh = false;
            continuation.resume_source = None;
            continuation.connection = None;
            continuation.native_start = Default::default();
        }
        driver.spawn_id = Some(info.spawn_id.clone());
        let now = now_millis();
        let same_session = self
            .agent_session
            .as_ref()
            .is_some_and(|session| session.native_session_id == info.native_session_id);
        // Codex can apply a user-approved startup model recovery. Persist only the effective
        // choices reported before this successful Ready, never the attempted failed override.
        if spec.kind == o7_protocol::DriverKind::Codex {
            for option in &self.options {
                if matches!(
                    option.category,
                    ConfigCategory::Model | ConfigCategory::Effort
                ) && let ConfigKind::Select { current, .. } = &option.kind
                    && !current.is_empty()
                {
                    self.desired.insert(
                        option.id.clone(),
                        ConfigValue::Choice {
                            value: current.clone(),
                        },
                    );
                }
            }
        }
        self.forget_unoffered_effort();
        let config_json = serde_json::to_string(&self.desired).ok();
        let controls_json = serde_json::to_string(&self.controls).ok();
        let (model, effort) = (
            self.option_value(ConfigCategory::Model),
            self.option_value(ConfigCategory::Effort),
        );
        match (&info.resumed, &mut self.agent_session) {
            // A session started again under its own id (`unwritten_claude_session`) keeps its row.
            (
                ResumeOutcome::Resumed | ResumeOutcome::Loaded | ResumeOutcome::New,
                Some(session),
            ) if same_session => {
                session.updated_at = now;
                session.autonomy = self.autonomy;
                session.config_json = config_json.clone();
                session.controls_json = controls_json.clone();
                session.capabilities = Some(info.capabilities.clone());
                let id = session.id.clone();
                self.queue_op(Op::UpdateAgentSession {
                    id,
                    patch: AgentSessionPatch {
                        capabilities: Some(info.capabilities.clone()),
                        native_session_id: None,
                        model,
                        effort,
                        autonomy: Some(self.autonomy),
                        config_json,
                        controls_json: controls_json.clone(),
                        updated_at: now,
                    },
                });
            }
            _ => {
                let row = AgentSessionRow {
                    capabilities: Some(info.capabilities.clone()),
                    host: self.chat.agent_host.clone(),
                    id: new_id(),
                    chat_id: self.chat_id.clone(),
                    agent_id: spec.id.to_owned(),
                    native_session_id: info.native_session_id.clone(),
                    model,
                    effort,
                    autonomy: self.autonomy,
                    config_json,
                    controls_json: controls_json.clone(),
                    cwd: self.chat.folder.clone(),
                    created_at: now,
                    updated_at: now,
                };
                self.agent_session = Some(row.clone());
                self.queue_op(Op::InsertAgentSession(row));
            }
        }
        if self.starting_fresh && info.resumed == ResumeOutcome::New {
            self.push(
                O7Event::Notice {
                    level: NoticeLevel::Info,
                    message:
                        "Started a new session. The agent doesn't remember the conversation above."
                            .into(),
                },
                None,
            );
        }
        self.compare_replay(&info);
        match info.native_session_id.as_ref() {
            Some(native_id)
                if self
                    .replay_check
                    .as_ref()
                    .is_none_or(|check| &check.native_session_id != native_id) =>
            {
                self.replay_check = Some(ReplayCheck {
                    native_session_id: native_id.clone(),
                    turns: 0,
                    last_reply_sha256: None,
                });
            }
            None => self.replay_check = None,
            _ => {}
        }
        self.native_session_id = info.native_session_id;
        self.agent_name = info.agent_name;
        self.agent_version = info.agent_version;
        // The next chat on this connection names its access levels before its agent starts.
        self.shared
            .catalogs
            .learn_levels(spec.id, &info.capabilities.autonomy_levels);
        self.capabilities = Some(info.capabilities);
        self.revision = self.revision.wrapping_add(1);
        if self
            .driver
            .as_ref()
            .is_some_and(|driver| driver.run_idle.is_none())
        {
            self.set_status(SessionStatus::Ready, None);
        }
        self.refresh_run_status();
        self.dispatch_next();
    }

    /// Hermes' gateway can finish work while o7 is away: when the replay's newest reply
    /// differs from what was stored, or it counts more turns, the reply is shown outside any turn.
    fn compare_replay(&mut self, info: &ReadyInfo) {
        let Some(summary) = &info.replay else {
            return;
        };
        let Some(check) = &self.replay_check else {
            return;
        };
        if Some(check.native_session_id.as_str()) != info.native_session_id.as_deref() {
            return;
        }
        let Some(reply) = &summary.last_reply else {
            return;
        };
        let differs = summary.last_reply_sha256 != check.last_reply_sha256
            || summary.user_turns > check.turns;
        if !differs {
            return;
        }
        let id = format!("{}.r1", info.spawn_id);
        self.push(
            O7Event::Notice {
                level: NoticeLevel::Info,
                message: format!(
                    "{} continued this session while o7 was away (in its gateway or another {} app). Its latest reply is below; earlier steps aren't shown here.",
                    self.agent_label(),
                    self.agent_label()
                ),
            },
            None,
        );
        self.push(
            O7Event::MessageDelta {
                message_id: id.clone(),
                text: reply.clone(),
            },
            None,
        );
        self.push(O7Event::MessageCompleted { message_id: id }, None);
        if let Some(check) = &mut self.replay_check {
            check.last_reply_sha256 = summary.last_reply_sha256.clone();
            check.turns = check.turns.max(summary.user_turns);
        }
    }

    /// The current value of the option of `category`, if there is one.
    fn option_value(&self, category: ConfigCategory) -> Option<String> {
        self.options
            .iter()
            .find(|option| option.category == category)
            .and_then(|option| match &option.kind {
                ConfigKind::Select { current, .. } => Some(current.clone()),
                ConfigKind::Toggle { .. } => None,
            })
    }

    fn on_turn_ended(
        &mut self,
        turn_id: &str,
        stop_reason: StopReason,
        usage: Option<o7_protocol::TokenUsage>,
        error: Option<DriverError>,
    ) {
        let Some(turn) = self.turn.take_if(|turn| turn.id == turn_id) else {
            tracing::debug!(chat_id = %self.chat_id, turn_id, "a TurnEnded for a turn that is not open was ignored");
            return;
        };
        let tag = Some(Arc::clone(&turn.tag));
        let now = now_millis();
        for event in self.normalizer.close_turn(&turn.id, ResolvedBy::Turn, now) {
            self.push_normalized(event, tag.clone());
        }
        let auth = error
            .as_ref()
            .is_some_and(|error| error.kind == ErrorKind::AuthRequired);
        let limited = error
            .as_ref()
            .is_some_and(|error| error.kind == ErrorKind::UsageLimit);
        if let Some(error) = error
            .as_ref()
            .filter(|_| limited && self.usage_limit.is_none())
        {
            // The agent's own limit report usually came first; this covers one that did not.
            self.usage_limit = Some(o7_protocol::UsageLimit {
                message: self.error_text(&error.message, false),
                resets_at: None,
            });
            self.info_dirty = true;
        }
        if let Some(error) = &error {
            let message = self.error_text(&error.message, false);
            let detail = error
                .detail
                .as_deref()
                .map(|detail| self.error_text(detail, false));
            self.last_error = Some(ErrorInfo {
                kind: error.kind,
                message: message.clone(),
                detail: detail.clone(),
            });
            self.push(
                O7Event::Error {
                    kind: error.kind,
                    message,
                    detail,
                    recoverable: true,
                },
                tag.as_deref(),
            );
        }
        self.push_normalized(
            O7Event::TurnEnded {
                turn_id: turn.id.clone(),
                stop_reason,
                usage,
                duration_ms: Some(elapsed_ms(turn.started)),
                recovered: false,
            },
            tag.clone(),
        );
        let state = match stop_reason {
            StopReason::Cancelled | StopReason::Interrupted => PromptState::Cancelled,
            StopReason::Error => PromptState::Failed,
            StopReason::EndTurn
            | StopReason::MaxTokens
            | StopReason::MaxTurnRequests
            | StopReason::Refusal
            | StopReason::Other => PromptState::Completed,
        };
        // A completed turn shows the limit no longer applies (it reset, or extra usage took over).
        if state == PromptState::Completed && self.usage_limit.take().is_some() {
            self.info_dirty = true;
        }
        self.push_normalized(
            O7Event::PromptLifecycle {
                prompt_id: turn.prompt_id.clone(),
                state,
                preview: None,
                held: None,
            },
            tag,
        );
        self.flush();
        if let Some(check) = &mut self.replay_check
            && self.native_session_id.as_deref() == Some(check.native_session_id.as_str())
        {
            check.turns = check.turns.saturating_add(1);
            if let Some(hash) = turn.reply.clone().finish() {
                check.last_reply_sha256 = Some(hash);
            }
        }
        for control in self.book.all() {
            self.in_flight.push(
                async move {
                    control.record_escaped().await;
                    Done::Nothing
                }
                .boxed(),
            );
        }
        self.refresh_attention();
        if auth || limited {
            // The prompt goes back into the queue, held, keeping its place. After a sign-in the
            // process stops; at a usage limit it keeps running, for an account switch or the reset.
            if let Some(accepted) = turn.timing.accepted {
                self.prompt_accepted
                    .insert(turn.prompt_id.clone(), accepted);
            }
            let row = QueuedRow {
                id: turn.prompt_id.clone(),
                parts: turn.parts.clone(),
                chat_id: self.chat_id.clone(),
                text: turn.text.clone(),
                held: true,
                created_at: turn.prompt_created_at,
            };
            let at = self
                .queue
                .iter()
                .position(|queued| (queued.created_at, &queued.id) > (row.created_at, &row.id))
                .unwrap_or(self.queue.len());
            self.queue.insert(at, row.clone());
            self.queue_op(Op::QueuePut(row));
            let cause = if auth {
                HoldCause::SignIn
            } else {
                HoldCause::UsageLimit
            };
            self.hold_queue(cause);
            self.hold_cause = cause;
            if auth {
                self.graceful_stop(
                    StopPlan {
                        cancel_grace: Duration::ZERO,
                        process: ShutdownPlan::STANDARD,
                    },
                    true,
                );
                return;
            }
        }
        if self.force_wait.is_some() {
            self.force_kill();
            return;
        }
        if self.status == SessionStatus::Stopping {
            return;
        }
        if stop_reason == StopReason::Error && !turn.interrupt_requested {
            self.hold_queue(HoldCause::Error);
        }
        if !self
            .driver
            .as_ref()
            .is_some_and(|driver| driver.run_idle.is_some())
        {
            self.set_status(SessionStatus::Ready, None);
        }
        self.refresh_run_status();
        self.dispatch_next();
    }

    fn on_permission(&mut self, turn: Option<&str>, request: PermissionRequest) {
        let tag = self.open_tag(turn);
        let (events, blobs) = self.normalizer.apply(
            O7Event::PermissionRequested(request),
            tag.as_deref(),
            now_millis(),
        );
        for op in blobs {
            self.queue_op(op);
        }
        for event in events {
            let decision = match &event {
                O7Event::PermissionRequested(request) => {
                    Some((
                        request.clone(),
                        decide(&self.autonomy_state, self.planning_freely(), request),
                    ))
                }
                _ => None,
            };
            self.push_normalized(event, tag.clone());
            match decision {
                Some((request, Decision::AutoAllow { option_id })) => {
                    self.auto_answer(&request, option_id, tag.as_deref());
                }
                Some((_, Decision::Ask)) => self.flush(),
                None => {}
            }
        }
        self.refresh_attention();
    }

    fn on_question(&mut self, turn: Option<&str>, request: UiRequest) {
        let tag = self.open_tag(turn);
        self.push(O7Event::UiRequest(request), tag.as_deref());
        self.flush();
        self.refresh_attention();
    }

    fn on_withdrawn(&mut self, request_id: &str, question: bool) {
        let open_tag = if question {
            self.normalizer
                .question(request_id)
                .map(|question| question.tag.clone())
        } else {
            self.normalizer
                .card(request_id)
                .map(|card| card.tag.clone())
        };
        match open_tag {
            Some(tag) => {
                let tag = self.open_tag(tag.as_deref());
                let event = if question {
                    O7Event::UiResolved {
                        request_id: request_id.to_owned(),
                        answer: None,
                        by: ResolvedBy::Agent,
                    }
                } else {
                    O7Event::PermissionResolved {
                        request_id: request_id.to_owned(),
                        decision: PermissionDecision::Cancelled,
                        by: ResolvedBy::Agent,
                    }
                };
                self.push(event, tag.as_deref());
                self.refresh_attention();
            }
            None => {
                // Both crossed in flight: the user's answer stands, and the withdrawal is noted.
                if let Some((_, title, tag)) = self
                    .answered
                    .iter()
                    .find(|(id, _, _)| id == request_id)
                    .cloned()
                {
                    let tag =
                        tag.filter(|tag| self.turn.as_ref().is_some_and(|turn| *turn.tag == **tag));
                    self.push(
                        O7Event::Notice {
                            level: NoticeLevel::Info,
                            message: format!(
                                "The agent withdrew \"{title}\" before your answer reached it."
                            ),
                        },
                        tag.as_deref(),
                    );
                }
            }
        }
    }

    fn on_events_closed(&mut self) {
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        let Some(driver) = &mut self.driver else {
            return;
        };
        driver.events = None;
        let generation = driver.generation;
        if let Some(join) = driver.join.take() {
            self.in_flight.push(join_driver(generation, join));
        }
    }

    fn on_driver_joined(&mut self, generation: u64, result: Result<DriverExit, JoinError>) {
        let current = self
            .driver
            .as_ref()
            .is_some_and(|driver| driver.generation == generation);
        if !current {
            // A retired (force-stopped) driver ended: its exit counts as that stop.
            self.book.clear(generation);
            self.retired_one(generation);
            return;
        }
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        let Some(driver) = self.driver.take() else {
            return;
        };
        self.stop_deadline = None;
        self.force_wait = None;
        let exit = match result {
            Ok(exit) => exit,
            Err(_) => DriverExit::Panicked,
        };
        let stopping = matches!(driver.stop, Stop::Graceful { .. });
        let auth = driver.stop == Stop::Graceful { auth: true };
        if self.conflict {
            self.conflict = false;
            self.kill_generation(generation);
            self.release_if_idle();
            self.set_status(SessionStatus::Failed, None);
            self.after_stop();
            return;
        }
        match exit {
            DriverExit::Closed if stopping => self.closed(generation, auth),
            DriverExit::Crashed { .. } | DriverExit::Panicked if stopping => {
                // It ended while being stopped: no crash card, but nothing may stay running.
                self.kill_generation(generation);
                self.closed(generation, auth);
            }
            DriverExit::Closed => {
                // `Closed` means asked to stop and did. A driver that ends on its own broke that:
                // whatever ended it may still run, and nothing queued may start the agent again.
                tracing::warn!(chat_id = %self.chat_id, "a driver closed without being asked to stop; it counts as a crash");
                self.crashed(generation, crash_message(None), None);
            }
            DriverExit::Crashed { exit, stderr_tail } => {
                let message = crash_message(exit.as_ref());
                self.crashed(generation, message, Some(stderr_tail));
            }
            DriverExit::Panicked => self.crashed(
                generation,
                "The agent stopped unexpectedly: o7's driver for it failed.".into(),
                None,
            ),
            DriverExit::StartFailed(error) if stopping && error.kind != ErrorKind::AuthRequired => {
                self.kill_generation(generation);
                self.closed(generation, auth);
            }
            DriverExit::StartFailed(error) => self.start_failed_exit(generation, error),
        }
    }

    /// The FORCE chain on every process of `generation` (whose driver ended), then the record is
    /// cleared; the generation retires until the chains end.
    fn kill_generation(&mut self, generation: u64) {
        let controls = self.book.close(generation);
        self.retire(generation, controls.len());
        for control in controls {
            self.in_flight.push(force_chain(generation, control));
        }
        self.book.clear(generation);
    }

    /// `generation` was force-stopped or crashed: it keeps the chat's live slot and native sessions
    /// until `outstanding` more FORCE chains and joins of it have ended.
    fn retire(&mut self, generation: u64, outstanding: usize) {
        if outstanding == 0 {
            return;
        }
        match self
            .retiring
            .iter_mut()
            .find(|(retired, _)| *retired == generation)
        {
            Some((_, left)) => *left = left.saturating_add(outstanding),
            None => self.retiring.push((generation, outstanding)),
        }
        self.winding_down.send_replace(self.retiring.len());
    }

    /// A FORCE chain or the join of a retired generation ended; the last one frees what it held.
    fn retired_one(&mut self, generation: u64) {
        let Some(at) = self
            .retiring
            .iter()
            .position(|(retired, _)| *retired == generation)
        else {
            return;
        };
        let left = &mut self.retiring[at].1;
        *left = left.saturating_sub(1);
        if *left == 0 {
            self.retiring.remove(at);
            self.winding_down.send_replace(self.retiring.len());
            self.release_if_idle();
        }
    }

    /// Gives the chat's live slot and native sessions back once nothing of it runs: no driver, no
    /// start preparation, and no retired generation whose processes or task still wind down.
    fn release_if_idle(&self) {
        if !self.steering_pending
            && self.driver.is_none()
            && self.preparing.is_none()
            && self.retiring.is_empty()
            && self.terminal.is_none()
        {
            self.shared.live.release(&self.chat_id);
        }
    }

    /// The driver closed as asked.
    fn closed(&mut self, generation: u64, auth: bool) {
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        self.book.clear(generation);
        self.release_if_idle();
        let now = now_millis();
        if let Some(turn) = self.turn.take() {
            // The cancel grace ran out before the turn's own end.
            let tag = Some(Arc::clone(&turn.tag));
            for event in self.normalizer.close_turn(&turn.id, ResolvedBy::Turn, now) {
                self.push_normalized(event, tag.clone());
            }
            self.push_normalized(
                O7Event::TurnEnded {
                    turn_id: turn.id.clone(),
                    stop_reason: StopReason::Interrupted,
                    usage: None,
                    duration_ms: Some(elapsed_ms(turn.started)),
                    recovered: false,
                },
                tag.clone(),
            );
            self.push_normalized(
                O7Event::PromptLifecycle {
                    prompt_id: turn.prompt_id,
                    state: PromptState::Cancelled,
                    preview: None,
                    held: None,
                },
                tag,
            );
        }
        for event in self.normalizer.close_untagged(ResolvedBy::System, now) {
            self.push_normalized(event, None);
        }
        self.refresh_attention();
        self.set_status(
            if auth {
                SessionStatus::AuthRequired
            } else {
                SessionStatus::Stopped
            },
            None,
        );
        self.flush();
        self.after_stop();
    }

    /// What follows a stop: a requested restart, or prompts typed while it stopped.
    fn after_stop(&mut self) {
        if self.quitting {
            return;
        }
        if let Some(fresh) = self.pending_start.take() {
            if let Err(error) = self.start(fresh, false) {
                self.refuse_terminal(error);
            }
            return;
        }
        // A Stop that replaced the start a terminal waited for (a start still preparing answers it).
        if self.preparing.is_none() {
            self.refuse_terminal(SessionError::OptionInvalid(
                "Opening the terminal was stopped.".into(),
            ));
        }
        if self.status == SessionStatus::Stopped && self.queue.first().is_some_and(|row| !row.held)
        {
            let _ = self.start(false, false);
        }
    }

    /// The driver ended without being asked (or panicked, or was aborted).
    fn crashed(&mut self, generation: u64, message: String, stderr_tail: Option<String>) {
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        self.kill_generation(generation);
        self.release_if_idle();
        let detail = stderr_tail
            .map(|tail| self.error_text(&tail, true))
            .filter(|tail| !tail.trim().is_empty());
        let now = now_millis();
        let error = O7Event::Error {
            kind: ErrorKind::AgentExited,
            message: message.clone(),
            detail: detail.clone(),
            recoverable: true,
        };
        if let Some(turn) = self.turn.take() {
            let tag = Some(Arc::clone(&turn.tag));
            for event in self
                .normalizer
                .close_turn(&turn.id, ResolvedBy::System, now)
            {
                self.push_normalized(event, tag.clone());
            }
            self.push(error, tag.as_deref());
            self.push_normalized(
                O7Event::TurnEnded {
                    turn_id: turn.id.clone(),
                    stop_reason: StopReason::Error,
                    usage: None,
                    duration_ms: Some(elapsed_ms(turn.started)),
                    recovered: false,
                },
                tag.clone(),
            );
            self.push_normalized(
                O7Event::PromptLifecycle {
                    prompt_id: turn.prompt_id,
                    state: PromptState::Failed,
                    preview: None,
                    held: None,
                },
                tag,
            );
        } else {
            self.push(error, None);
        }
        for event in self.normalizer.close_untagged(ResolvedBy::System, now) {
            self.push_normalized(event, None);
        }
        self.hold_queue(HoldCause::Error);
        self.last_error = Some(ErrorInfo {
            kind: ErrorKind::AgentExited,
            message,
            detail,
        });
        self.refresh_attention();
        self.set_status(SessionStatus::Stopped, None);
        self.flush();
        self.pending_start = None;
        self.refuse_terminal(SessionError::AgentUnavailable(
            self.last_error
                .as_ref()
                .map_or_else(String::new, |error| error.message.clone()),
        ));
    }

    fn start_failed_exit(&mut self, generation: u64, error: DriverError) {
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        self.kill_generation(generation);
        if let Some(turn) = self.turn.take() {
            // A driver that reports a failed start mid-turn broke its contract; end the turn anyway.
            let tag = Some(Arc::clone(&turn.tag));
            for event in self
                .normalizer
                .close_turn(&turn.id, ResolvedBy::System, now_millis())
            {
                self.push_normalized(event, tag.clone());
            }
            self.push_normalized(
                O7Event::TurnEnded {
                    turn_id: turn.id.clone(),
                    stop_reason: StopReason::Error,
                    usage: None,
                    duration_ms: Some(elapsed_ms(turn.started)),
                    recovered: false,
                },
                tag.clone(),
            );
            self.push_normalized(
                O7Event::PromptLifecycle {
                    prompt_id: turn.prompt_id,
                    state: PromptState::Failed,
                    preview: None,
                    held: None,
                },
                tag,
            );
            self.hold_queue(HoldCause::Error);
        }
        let mut message = self.redact(&error.message);
        // A failed start's detail is usually the agent's stderr tail (5.3.2).
        let detail = error
            .detail
            .as_deref()
            .map(|detail| self.error_text(detail, true));
        let adapter = self
            .spec
            .as_ref()
            .is_some_and(|spec| matches!(spec.recipe, Recipe::NpmAdapter { .. }));
        let text = format!(
            "{}\n{}",
            error.message,
            error.detail.as_deref().unwrap_or_default()
        );
        if adapter
            && [
                "ERR_MODULE_NOT_FOUND",
                "MODULE_NOT_FOUND",
                "Cannot find module",
            ]
            .iter()
            .any(|marker| text.contains(marker))
            && let Some(spec) = &self.spec
        {
            self.shared.deps.detector.forget_install(spec);
            message = format!(
                "The {} adapter's files are damaged. Press Retry to download it again.",
                self.agent_label()
            );
        }
        self.start_failed(
            error.kind,
            message,
            detail,
            error.kind == ErrorKind::AuthRequired,
        );
        if let Some(fresh) = self.pending_start.take() {
            let _ = self.start(fresh, false);
        }
    }

    /// A start that failed: `Failed` (or `AuthRequired`), the card stored outside any turn. `detail`
    /// comes redacted and fitted ([`error_text`](Self::error_text)).
    fn start_failed(
        &mut self,
        kind: ErrorKind,
        message: String,
        detail: Option<String>,
        auth: bool,
    ) {
        if let Some(reply) = self.terminal_request.take() {
            drop(reply.send(Err(SessionError::AgentUnavailable(message.clone()))));
        }
        self.memory_grant = None;
        self.browser_grant = None;
        self.mobile_grant = None;
        self.coordinator_grant = None;
        self.release_if_idle();
        let message = self.error_text(&message, false);
        self.last_error = Some(ErrorInfo {
            kind,
            message: message.clone(),
            detail: detail.clone(),
        });
        self.push(
            O7Event::Error {
                kind,
                message,
                detail,
                recoverable: true,
            },
            None,
        );
        let now = now_millis();
        for event in self.normalizer.close_untagged(ResolvedBy::System, now) {
            self.push_normalized(event, None);
        }
        self.refresh_attention();
        self.set_status(
            if auth {
                SessionStatus::AuthRequired
            } else {
                SessionStatus::Failed
            },
            None,
        );
        self.flush();
    }

    /// The store closed with ops still here. A quit drains every actor before closing the store,
    /// so this happens only when a deadline ran out (or the store failed). Each op is named: a
    /// lost write at error level with the count kept for the quit's summary, a read (its waiter
    /// learns the store closed) at warn level.
    fn store_closed(&mut self) {
        let dropped = DroppedOps::of(self.outbox.drain(..));
        self.outbox_bytes = 0;
        if !dropped.writes.is_empty() {
            self.shared.lost_writes.fetch_add(
                u64::try_from(dropped.writes.len()).unwrap_or(u64::MAX),
                std::sync::atomic::Ordering::AcqRel,
            );
            tracing::error!(
                chat_id = %self.chat_id,
                writes = dropped.writes.len(),
                ops = %dropped.writes.join(", "),
                "o7's database closed before these writes were saved; they are lost"
            );
        }
        if !dropped.reads.is_empty() {
            tracing::warn!(
                chat_id = %self.chat_id,
                reads = dropped.reads.len(),
                ops = %dropped.reads.join(", "),
                "o7's database closed before these reads ran; their callers were told it closed"
            );
        }
    }

    // ---- Timers -------------------------------------------------------------------------------

    fn on_timer(&mut self) {
        let now = Instant::now();
        if let Some((at, generation)) = self.force_wait
            && at <= now
        {
            self.force_wait = None;
            if self
                .driver
                .as_ref()
                .is_some_and(|driver| driver.generation == generation)
            {
                self.force_kill();
            }
        }
        if let Some((at, generation)) = self.stop_deadline
            && at <= now
        {
            self.stop_deadline = None;
            if self
                .driver
                .as_ref()
                .is_some_and(|driver| driver.generation == generation)
            {
                self.force_stop();
            }
        }
        if let Some(at) = self.idle_deadline
            && at <= now
        {
            self.idle_deadline = None;
            tracing::info!(chat_id = %self.chat_id, "stopping an idle agent nobody views");
            self.stop();
        }
        if let Some(wait) = self.idle_wait
            && wait.deadline <= now
        {
            self.idle_wait = None;
            self.idle_grace_ended(wait.generation);
        }
    }

    /// The idle grace ran out: the agent still reports work in progress with no prompt of
    /// o7's open and nothing waiting for an answer. It counts as idle for the queue from now
    /// until its next prompt or idle report; a late busy report cannot take that back.
    fn idle_grace_ended(&mut self, generation: u64) {
        let open_requests = self.normalizer.has_open_requests();
        let agent = self.agent_label();
        let Some(driver) = &mut self.driver else {
            return;
        };
        if driver.generation != generation
            || !driver.ready
            || driver.stop != Stop::None
            || driver.run_idle != Some(false)
            || self.turn.is_some()
            || open_requests
        {
            return;
        }
        driver.idle_assumed = true;
        tracing::info!(
            chat_id = %self.chat_id,
            agent = %agent,
            grace_s = self.shared.deps.timings.idle_grace.as_secs(),
            "the agent reported work in progress and no idle within the grace after its turn; \
             treating it as idle so queued prompts can go (the agent queues them itself)"
        );
        self.refresh_run_status();
    }

    /// Whether the agent idles: `Ready`, nobody viewing (a remote view counts), nothing open or
    /// waiting to go, no work in the background. Such an agent stops after the idle time, or
    /// sooner to make room for another chat's start. An agent that only counts as idle after the
    /// idle grace still works, so it is never stopped.
    fn idles(&self) -> bool {
        self.status == SessionStatus::Ready
            && self.turn.is_none()
            && self.subscribers.is_empty()
            && !self.normalizer.has_open_requests()
            && !self.queue.first().is_some_and(|row| !row.held)
            && !self.quitting
            && self.terminal.is_none()
            && self
                .driver
                .as_ref()
                .is_none_or(|driver| driver.run_idle != Some(false))
    }

    /// Idle but for an open view: a change that rewrites the folder may stop it, losing nothing.
    fn idles_but_shown(&self) -> bool {
        self.status == SessionStatus::Ready
            && self.turn.is_none()
            && !self.normalizer.has_open_requests()
            && !self.queue.first().is_some_and(|row| !row.held)
            && !self.quitting
            && self.terminal.is_none()
            && self
                .driver
                .as_ref()
                .is_none_or(|driver| driver.run_idle != Some(false))
    }

    /// Idle eviction: idle long enough, by [`Self::idles`]. The live-agent table learns what the
    /// agent does whenever that changes, so a start at the limit can stop the one idle longest.
    fn update_idle(&mut self) {
        let idle = self.idles();
        self.idle_deadline =
            idle.then(|| self.last_activity + self.shared.deps.limits.idle_eviction);
        let now = if idle {
            Reported::Idle(self.last_activity)
        } else if self.terminal.is_some() && self.terminal_import.is_some() {
            Reported::Terminal
        } else if self.terminal.is_some()
            || matches!(
                self.status,
                SessionStatus::Stopping
                    | SessionStatus::Stopped
                    | SessionStatus::Failed
                    | SessionStatus::AuthRequired
            )
        {
            // A terminal that ended reads its conversation back, then gives the session back by
            // itself: it is on its way out, as a stopping agent is.
            Reported::Stopping
        } else if self.normalizer.has_open_requests() {
            Reported::Waiting
        } else if self.status == SessionStatus::Ready && !self.subscribers.is_empty() {
            Reported::Shown(self.last_activity)
        } else {
            Reported::Working
        };
        if self.reported == Some(now) {
            return;
        }
        self.reported = Some(now);
        let activity = match now {
            Reported::Idle(since) => super::live::Activity::Idle {
                since,
                notify: Arc::clone(&self.make_room),
            },
            Reported::Working => super::live::Activity::Working,
            Reported::Waiting => super::live::Activity::Waiting,
            Reported::Terminal => super::live::Activity::Terminal,
            Reported::Shown(since) => super::live::Activity::Shown { since },
            Reported::Stopping => super::live::Activity::Stopping,
        };
        if self.shared.live.report(&self.chat_id, activity) {
            // A start waits for room and no agent was idle to give it: this one is now.
            tracing::info!(chat_id = %self.chat_id, "stopping an idle agent to make room for a chat waiting to start");
            self.stop();
        }
    }

    /// A start at the limit chose this agent, idle longest, to make room. It stops if it still
    /// idles; one already on its way out keeps its turn; otherwise another idle agent is asked.
    fn make_room(&mut self) {
        if self.idles() {
            tracing::info!(chat_id = %self.chat_id, "stopping an idle agent to make room for another chat");
            self.stop();
            return;
        }
        let leaving = self.terminal.is_none()
            && self.preparing.is_none()
            && self.pending_start.is_none()
            && matches!(
                self.status,
                SessionStatus::Stopping
                    | SessionStatus::Stopped
                    | SessionStatus::Failed
                    | SessionStatus::AuthRequired
            );
        if !leaving {
            self.shared.live.decline_stop(&self.chat_id);
        }
    }

    /// Every slot is taken and an idle agent is stopping for this chat: its slot passes here,
    /// then the start goes on.
    fn wait_for_room(&mut self, fresh: bool) {
        if let Some(room) = &mut self.room {
            room.fresh |= fresh;
            return;
        }
        self.rooms = self.rooms.wrapping_add(1);
        let id = self.rooms;
        let shared = Arc::clone(&self.shared);
        let chat_id = self.chat_id.clone();
        let task = self.shared.deps.rt.spawn(async move {
            let mut released = shared.live.released();
            tokio::time::timeout(ROOM_WAIT, async {
                while !shared.live.contains(&chat_id) {
                    let _ = released.changed().await;
                }
            })
            .await
            .is_ok()
        });
        let abort = task.abort_handle();
        self.in_flight.push(
            async move {
                Done::Room {
                    id,
                    admitted: task.await.unwrap_or(false),
                }
            }
            .boxed(),
        );
        self.room = Some(RoomWait { id, fresh, abort });
        self.set_status(
            SessionStatus::Starting,
            Some("Waiting for an idle chat to stop and make room".into()),
        );
    }

    /// The wait for room ended: the start goes on in the slot it was given, or fails.
    fn room_done(&mut self, id: u64, admitted: bool) {
        let Some(room) = self.room.take_if(|room| room.id == id) else {
            return;
        };
        // Only a start still waiting goes on. Every path that ends or replaces a start ends its
        // wait (end_room_wait); this guards the rest, so no second process ever starts.
        if self.status != SessionStatus::Starting
            || self.driver.is_some()
            || self.preparing.is_some()
            || self.turn.is_some()
        {
            self.shared.live.cancel_wait(&self.chat_id);
            self.release_if_idle();
            return;
        }
        // The start begins again from where it waited. The status is set without telling anyone:
        // what happens next says where the chat stands.
        self.status = SessionStatus::Stopped;
        if !admitted {
            self.shared.live.cancel_wait(&self.chat_id);
            self.release_if_idle();
            let message = "No idle agent stopped in time to make room. Stop a chat you are not using, then try again.";
            self.start_failed(ErrorKind::SpawnFailed, message.into(), None, false);
            return;
        }
        match self.start(room.fresh, false) {
            // Held back (behind a quiet change, say), it stays stopped: the chat list hears so,
            // not the Starting it heard last.
            Ok(()) if self.status == SessionStatus::Stopped => self.publish_status(),
            Ok(()) => {}
            // Quitting, or held by its worktree's setup (which starts it once it succeeds): it
            // stops, saying so.
            Err(error)
                if matches!(error, SessionError::ShuttingDown)
                    || self
                        .worktree_setup
                        .as_ref()
                        .is_some_and(|attempt| attempt.blocks_agent()) =>
            {
                self.release_if_idle();
                self.refuse_terminal(error);
                self.publish_status();
            }
            // Anything else fails as a start fails: a card the person can act on.
            Err(error) => self.start_failed(ErrorKind::SpawnFailed, error.to_string(), None, false),
        }
    }

    /// Publishes the status as it is now, for a status set without telling anyone.
    fn publish_status(&mut self) {
        self.info_dirty = true;
        self.next_flush = Some(Instant::now());
        self.publish();
    }

    /// A quiet change to the folder begins: refused only while a turn, a start or a stop is
    /// under way (or the agent works in its terminal), so an idle agent may stay.
    fn hold_quiet(&mut self, id: String) -> Result<(), SessionError> {
        let busy = self.turn.is_some()
            || self.preparing.is_some()
            || self.room.is_some()
            || self.checkpointing.is_some()
            || self.steering_pending
            || self.pending_start.is_some()
            || self.terminal.is_some()
            || !self.retiring.is_empty()
            || self.normalizer.has_open_requests()
            || matches!(
                self.status,
                SessionStatus::Starting
                    | SessionStatus::Running
                    | SessionStatus::Cancelling
                    | SessionStatus::Stopping
            );
        if busy {
            return Err(SessionError::Organization(format!(
                "{} is working in this folder.",
                self.agent_label()
            )));
        }
        self.quiet_holds.insert(id);
        self.info_dirty = true;
        self.schedule_flush();
        Ok(())
    }

    /// A quiet change ended: what waited for the last one goes on.
    fn release_quiet(&mut self, id: &str) {
        if !self.quiet_holds.remove(id) || !self.quiet_holds.is_empty() {
            return;
        }
        self.info_dirty = true;
        self.schedule_flush();
        if let Some(fresh) = self.start_after_hold.take()
            && let Err(error) = self.start(fresh, false)
        {
            self.refuse_terminal(error);
        }
        // What is queued goes now, starting the agent if it is stopped.
        self.advance();
    }

    /// Ends a wait for room (a Stop, a quit).
    fn end_room_wait(&mut self) {
        if let Some(room) = self.room.take() {
            room.abort.abort();
            self.shared.live.cancel_wait(&self.chat_id);
        }
    }

    // ---- Frames, flushing and persistence -----------------------------------------------------

    /// Normalizes `event` and queues it for the next flush, tagged `turn`.
    fn push(&mut self, event: O7Event, turn: Option<&str>) {
        let (events, blobs) = self.normalizer.apply(event, turn, now_millis());
        // A blob goes out before the row that names it.
        for op in blobs {
            self.queue_op(op);
        }
        let tag: TurnTag = turn.map(Arc::from);
        for event in events {
            self.push_normalized(event, tag.clone());
        }
    }

    fn push_normalized(&mut self, event: O7Event, tag: TurnTag) {
        // A coordinated project hears of every turn that ends, however it ended.
        if matches!(event, O7Event::TurnEnded { .. }) {
            self.shared.coordination.turn_ended(&self.chat_id);
        }
        let flush_now = matches!(
            event,
            O7Event::TurnStarted { .. }
                | O7Event::TurnEnded { .. }
                | O7Event::PermissionRequested(_)
                | O7Event::UiRequest(_)
                | O7Event::Error { .. }
        );
        self.batched_bytes = self.batched_bytes.saturating_add(approx_bytes(&event));
        self.batcher.push_tagged(event, tag);
        if flush_now || self.batched_bytes >= self.shared.knobs.outbox_bytes / 4 {
            self.next_flush = Some(Instant::now());
        } else {
            self.schedule_flush();
        }
    }

    fn schedule_flush(&mut self) {
        if self.next_flush.is_none() {
            let timings = &self.shared.deps.timings;
            let every = if self.subscribers.is_empty() {
                timings.flush_idle
            } else {
                timings.flush_live
            };
            self.next_flush = Some(Instant::now() + every);
        }
    }

    /// Sends what is queued to every subscriber and queues its rows for the store.
    fn flush(&mut self) {
        self.next_flush = None;
        self.batched_bytes = 0;
        if self.info_dirty {
            self.info_dirty = false;
            let info = self.session_info();
            self.queue_wait.clone_from(&info.queue_wait);
            let (events, _) =
                self.normalizer
                    .apply(O7Event::SessionUpdated(info), None, now_millis());
            for event in events {
                self.batcher.push_tagged(event, None);
            }
        }
        let frames = self.batcher.flush_tagged();
        if frames.is_empty() {
            return;
        }
        for (frame, _) in &frames {
            let mut gone = Vec::new();
            self.subscribers
                .retain(|subscriber| match subscriber.sink.send(frame) {
                    Ok(()) => true,
                    Err(SinkGone) => {
                        gone.push(subscriber.id.clone());
                        false
                    }
                });
            for id in gone {
                self.shared.forget_subscription(&id);
            }
        }
        let now = now_millis();
        let mut rows: Vec<EventRow> = Vec::new();
        let mut begin: Option<(TurnRow, Vec<EventRow>)> = None;
        for (frame, tags) in frames {
            for (offset, (event, tag)) in frame.events.into_iter().zip(tags).enumerate() {
                let seq = frame
                    .seq_start
                    .wrapping_add(u32::try_from(offset).unwrap_or(u32::MAX));
                if !event.is_stored(tag.is_some()) {
                    continue;
                }
                let row = EventRow {
                    seq,
                    turn_id: tag.as_deref().map(str::to_owned),
                    kind: event.kind(),
                    payload: serde_json::to_string(&event).unwrap_or_else(|_| "{}".into()),
                    created_at: now,
                };
                match &event {
                    O7Event::TurnStarted { turn_id, .. } => {
                        if let Some((turn, turn_rows)) = begin.take() {
                            self.queue_op(Op::BeginTurn {
                                turn,
                                rows: turn_rows,
                            });
                        } else if !rows.is_empty() {
                            self.queue_op(Op::AppendEvents {
                                chat_id: self.chat_id.clone(),
                                rows: std::mem::take(&mut rows),
                            });
                        }
                        match self.begun.take_if(|turn| turn.id == *turn_id) {
                            Some(mut turn) => {
                                turn.first_event_seq = seq;
                                begin = Some((turn, vec![row]));
                            }
                            None => rows.push(row),
                        }
                    }
                    O7Event::TurnEnded {
                        turn_id,
                        stop_reason,
                        ..
                    } => {
                        let (turn_id, stop_reason) = (turn_id.clone(), *stop_reason);
                        match &mut begin {
                            Some((_, turn_rows)) => turn_rows.push(row),
                            None => rows.push(row),
                        }
                        if let Some((turn, turn_rows)) = begin.take() {
                            self.queue_op(Op::BeginTurn {
                                turn,
                                rows: turn_rows,
                            });
                        } else if !rows.is_empty() {
                            self.queue_op(Op::AppendEvents {
                                chat_id: self.chat_id.clone(),
                                rows: std::mem::take(&mut rows),
                            });
                        }
                        self.queue_op(Op::EndTurn(TurnEnd {
                            turn_id: turn_id.clone(),
                            status: turn_status(stop_reason),
                            stop_reason,
                            ended_at: now,
                            last_event_seq: seq,
                        }));
                        self.queue_op(Op::TouchChat {
                            chat_id: self.chat_id.clone(),
                            at: now,
                        });
                        // A late steering acknowledgement still belongs to this turn. Seal its
                        // history only after that attempt's final visible outcome has been saved.
                        if !self.steering_pending {
                            self.queue_op(Op::CompactTurn {
                                chat_id: self.chat_id.clone(),
                                turn_id,
                            });
                        }
                    }
                    _ => match &mut begin {
                        Some((_, turn_rows)) => turn_rows.push(row),
                        None => rows.push(row),
                    },
                }
            }
        }
        if let Some((turn, turn_rows)) = begin {
            self.queue_op(Op::BeginTurn {
                turn,
                rows: turn_rows,
            });
        } else if !rows.is_empty() {
            self.queue_op(Op::AppendEvents {
                chat_id: self.chat_id.clone(),
                rows,
            });
        }
    }

    fn queue_op(&mut self, op: Op) {
        self.outbox_bytes = self.outbox_bytes.saturating_add(op_bytes(&op));
        self.outbox.push_back(op);
    }

    fn send_command(&mut self, command: DriverCommand) {
        if let Some(driver) = &self.driver {
            let _ = driver.commands.send(command);
            self.last_activity = Instant::now();
        }
    }

    // ---- State ----------------------------------------------------------------------------------

    fn set_status(&mut self, status: SessionStatus, detail: Option<String>) {
        let changed = self.status != status;
        self.status = status;
        self.detail = detail;
        self.info_dirty = true;
        if changed {
            self.next_flush = Some(Instant::now());
            self.publish();
        } else {
            self.schedule_flush();
        }
    }

    fn set_detail(&mut self, detail: Option<String>) {
        if self.detail != detail {
            self.detail = detail;
            self.info_dirty = true;
            self.schedule_flush();
        }
    }

    /// Publishes `needs_attention` when it changed.
    fn refresh_attention(&mut self) {
        let needs = self.normalizer.has_open_requests();
        if needs != self.needs_attention {
            self.needs_attention = needs;
            self.publish();
        }
    }

    /// An opted-in driver's prompt can finish while its background work is still running. Work in
    /// progress with no prompt of o7's open and nothing to answer runs the idle grace
    /// ([`Self::idle_grace_ended`]).
    fn refresh_run_status(&mut self) {
        if self.checkpointing.is_some() {
            return;
        }
        let Some(driver) = &self.driver else {
            return;
        };
        if driver.run_idle.is_none() {
            return;
        }
        if !driver.ready || driver.stop != Stop::None || self.turn.is_some() {
            self.idle_wait = None;
            return;
        }
        let open_requests = self.normalizer.has_open_requests();
        if driver_idle(driver) && !open_requests {
            self.idle_wait = None;
            if self.status != SessionStatus::Ready {
                self.set_status(SessionStatus::Ready, None);
            }
            self.dispatch_next();
            return;
        }
        if open_requests {
            // The answer is the person's; the grace starts again once nothing waits for one.
            self.idle_wait = None;
        } else if self
            .idle_wait
            .is_none_or(|wait| wait.generation != driver.generation)
        {
            let grace = self.shared.deps.timings.idle_grace;
            let since = now_millis();
            self.idle_wait = Some(IdleWait {
                generation: driver.generation,
                deadline: Instant::now() + grace,
                since,
                sends_at: since
                    .saturating_add(i64::try_from(grace.as_millis()).unwrap_or(i64::MAX)),
            });
        }
        if !matches!(
            self.status,
            SessionStatus::Cancelling | SessionStatus::Running
        ) {
            self.set_status(
                SessionStatus::Running,
                Some("Agent work is still running".into()),
            );
        }
    }

    /// Why the queued prompts wait, while any are queued (see [`QueueWait`]). The first match in
    /// the order a person would have to clear them wins.
    fn compute_queue_wait(&self) -> Option<QueueWait> {
        // The prompt on its way does not wait: only the ones after it do.
        let sending = self.sending_id();
        let next = self
            .queue
            .iter()
            .find(|row| Some(row.id.as_str()) != sending)?;
        if !self.store.workspace_available(&self.chat_id) {
            return Some(QueueWait::WorkspaceRemoved);
        }
        if let Some(attempt) = self
            .worktree_setup
            .as_ref()
            .filter(|attempt| attempt.blocks_agent())
        {
            return Some(QueueWait::Setup {
                state: attempt.state,
                detail: attempt.detail.clone(),
            });
        }
        if next.held {
            return Some(QueueWait::Held {
                after: self.hold_cause,
            });
        }
        if self.organizing || !self.quiet_holds.is_empty() {
            return Some(QueueWait::ChatChanging);
        }
        match self.status {
            SessionStatus::Starting => return Some(QueueWait::Starting),
            SessionStatus::Stopping | SessionStatus::Cancelling => {
                return Some(QueueWait::Stopping);
            }
            SessionStatus::AuthRequired => return Some(QueueWait::SignIn),
            SessionStatus::Failed => {
                let missing = self.folder_state == FolderState::Missing
                    || self
                        .last_error
                        .as_ref()
                        .is_some_and(|error| error.kind == ErrorKind::FolderMissing);
                return Some(if missing {
                    QueueWait::FolderMissing
                } else {
                    QueueWait::StartFailed
                });
            }
            SessionStatus::Stopped => return Some(QueueWait::Stopped),
            SessionStatus::Ready | SessionStatus::Running => {}
        }
        if self.checkpointing.is_some() {
            return Some(if self.checkpoint_capture {
                QueueWait::SavingCheckpoint
            } else {
                QueueWait::ChatChanging
            });
        }
        let Some(driver) = self.driver.as_ref().filter(|driver| driver.ready) else {
            return Some(QueueWait::Starting);
        };
        // A turn waits on its open requests; between turns they hold the queue only for a driver
        // that reports its run state (the turn-based contract sends regardless).
        if (self.turn.is_some() || driver.run_idle.is_some()) && self.normalizer.has_open_requests()
        {
            return Some(QueueWait::NeedsAnswer {
                question: self.normalizer.cards().is_empty(),
            });
        }
        if self.turn.is_some() || self.steering_pending || self.native_identity_pending.is_some() {
            return Some(QueueWait::Working);
        }
        if !driver_idle(driver) {
            let wait = self
                .idle_wait
                .filter(|wait| wait.generation == driver.generation);
            return Some(QueueWait::FinishingTurn {
                since: wait.map(|wait| wait.since),
                sends_at: wait.map(|wait| wait.sends_at),
            });
        }
        // Ready with nothing in the way: the next prompt is on its way.
        None
    }

    /// Flushes a `SessionInfo` whenever the queue's wait changed.
    fn refresh_queue_wait(&mut self) {
        let wait = self.compute_queue_wait();
        if wait != self.queue_wait {
            self.queue_wait = wait;
            self.info_dirty = true;
            self.schedule_flush();
        }
    }

    fn publish(&self) {
        self.shared.publish(
            &self.chat_id,
            ChatState {
                status: self.status,
                needs_attention: self.needs_attention,
                title: self.chat.title.clone(),
                updated_at: self.chat.updated_at,
                folder_state: self.folder_state,
            },
        );
    }

    fn redact(&self, text: &str) -> String {
        self.shared.deps.redactor.redact_text(text).into_owned()
    }

    /// A driver's error text as its card shows it and `last_error` keeps it: redacted (5.1), then
    /// cut to the card's 2 KiB of JSON. A stderr tail keeps its end, which explains the failure;
    /// any other text keeps its start, as the normalizer would.
    fn error_text(&self, text: &str, stderr: bool) -> String {
        let redacted = self.redact(text);
        if stderr {
            suffix_escaped(&redacted, STDERR_TAIL_SHOWN_BYTES).to_owned()
        } else {
            cap_escaped(redacted, MESSAGE_BYTES)
        }
    }

    fn summary(&self) -> ChatSummary {
        ChatSummary {
            id: self.chat_id.clone(),
            title: self.chat.title.clone(),
            agent_id: self.chat.agent_id.clone(),
            agent_host: self.chat.agent_host.clone(),
            agent_name: self.agent_label(),
            folder: self.chat.folder.to_string_lossy().into_owned(),
            folder_state: self.folder_state,
            status: self.status,
            needs_attention: self.needs_attention,
            created_at: self.chat.created_at,
            updated_at: self.chat.updated_at,
        }
    }

    fn session_info(&self) -> SessionInfo {
        // A chat whose agent is not running names the choices its next start asks for.
        let waiting = (!self.options_live()).then(|| self.shown_options());
        let options = waiting.as_deref().unwrap_or(&self.options);
        SessionInfo {
            worktree_setup: self.worktree_setup.clone(),
            status: self.status,
            detail: self.detail.clone(),
            native_session_id: self.native_session_id.clone(),
            agent_name: self.agent_name.clone(),
            agent_version: self.agent_version.clone(),
            title: self.chat.title.clone(),
            model: option_label(options, ConfigCategory::Model),
            effort: option_label(options, ConfigCategory::Effort),
            mode: option_label(options, ConfigCategory::Mode),
            autonomy: self.autonomy,
            queue_held: self.queue.iter().any(|row| row.held),
            revision: self.revision,
            queue_wait: self.compute_queue_wait(),
            sending: self.sending(),
            account_id: self.account.as_ref().map(|(id, _)| id.clone()),
            usage_limit: self.usage_limit.clone(),
        }
    }

    fn snapshot(&self) -> ChatSnapshot {
        ChatSnapshot {
            chat: self.summary(),
            session: self.session_info(),
            queue: self.queued_prompts(),
            open_turn_id: self.turn.as_ref().map(|turn| turn.id.clone()),
            usage: self.usage.clone(),
            limits: self.limits.clone(),
            last_error: self.last_error.clone(),
            auth: self.auth.clone(),
            storage: self.store.status().borrow().clone(),
            tasks: self.normalizer.tasks().to_vec(),
        }
    }
}

impl Drop for ChatActor {
    fn drop(&mut self) {
        self.cancel_checkpoint();
    }
}

/// A queued row as the queue and the header show it.
fn shown(row: &QueuedRow) -> QueuedPrompt {
    QueuedPrompt {
        prompt_id: row.id.clone(),
        preview: cap_chars(row.text.clone(), PREVIEW_CHARS),
        held: row.held,
        created_at: row.created_at,
    }
}

/// Whether a driver counts as idle for the queue: it keeps the turn-based contract, reported idle,
/// or ran out the idle grace.
fn driver_idle(driver: &LiveDriver) -> bool {
    driver
        .run_idle
        .is_none_or(|idle| idle || driver.idle_assumed)
}

/// A queued prompt's typed words and the context after them (attached files' text, image and mention
/// lines). The composer always sends the typed words as the first part; a plain send has no parts.
fn editable(row: &QueuedRow) -> Result<(&str, &str), SessionError> {
    match row.parts.first() {
        None => Ok((row.text.as_str(), "")),
        Some(PromptPart::Text { text }) => row
            .text
            .strip_prefix(text.as_str())
            .map(|context| (text.as_str(), context))
            .ok_or_else(uneditable),
        Some(_) => Err(uneditable()),
    }
}

fn uneditable() -> SessionError {
    SessionError::OptionInvalid("This message can't be edited. Remove it and send it again.".into())
}

/// What a chat asks its agent for at start: its saved session's choices, or for a chat whose agent
/// never started, the project's or Settings' model and Settings' thinking level (`(option, level)`).
fn initial_options(
    session: Option<&AgentSessionRow>,
    initial_model: Option<String>,
    initial_effort: Option<(String, String)>,
) -> BTreeMap<String, ConfigValue> {
    let mut desired: BTreeMap<String, ConfigValue> = session
        .and_then(|session| session.config_json.as_deref())
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    if session.is_none() {
        if let Some(value) = initial_model {
            desired.insert("model".into(), ConfigValue::Choice { value });
        }
        if let Some((id, value)) = initial_effort {
            desired.insert(id, ConfigValue::Choice { value });
        }
    }
    desired
}

/// The id of the agent's thinking level option, as its catalog names it.
fn effort_option_id(catalog: Option<&super::catalogs::Catalog>) -> String {
    catalog
        .and_then(|catalog| catalog.efforts.values().flatten().next())
        .map_or_else(|| "effort".to_owned(), |option| option.id.clone())
}

/// Whether `option` offers `value`.
fn offers(option: &ConfigOption, value: &ConfigValue) -> bool {
    match (&option.kind, value) {
        (ConfigKind::Select { choices, .. }, ConfigValue::Choice { value }) => {
            choices.iter().any(|choice| choice.value == *value)
        }
        (ConfigKind::Toggle { .. }, ConfigValue::Toggle { .. }) => true,
        _ => false,
    }
}

/// Shows `value` as the option's current one; a choice it does not list is added, so it shows.
fn show_value(option: &mut ConfigOption, value: &ConfigValue) {
    match (&mut option.kind, value) {
        (ConfigKind::Select { current, choices }, ConfigValue::Choice { value }) => {
            if !value.is_empty() && !choices.iter().any(|choice| choice.value == *value) {
                choices.push(o7_protocol::session::ConfigChoice {
                    value: value.clone(),
                    name: value.clone(),
                    description: None,
                    group: None,
                });
            }
            current.clone_from(value);
        }
        (ConfigKind::Toggle { current }, ConfigValue::Toggle { value }) => *current = *value,
        _ => {}
    }
}

/// The display name of the current choice of the option of `category`; none for nothing chosen.
fn option_label(options: &[ConfigOption], category: ConfigCategory) -> Option<String> {
    let option = options.iter().find(|option| option.category == category)?;
    match &option.kind {
        ConfigKind::Select { current, .. } if current.is_empty() => None,
        ConfigKind::Select { current, choices } => Some(
            choices
                .iter()
                .find(|choice| choice.value == *current)
                .map_or_else(|| current.clone(), |choice| choice.name.clone()),
        ),
        ConfigKind::Toggle { .. } => None,
    }
}

fn current_value(option: &ConfigOption) -> ConfigValue {
    match &option.kind {
        ConfigKind::Select { current, .. } => ConfigValue::Choice {
            value: current.clone(),
        },
        ConfigKind::Toggle { current } => ConfigValue::Toggle { value: *current },
    }
}

/// The autonomy state while no agent runs (nothing is auto-answered).
fn stopped_autonomy(autonomy: Autonomy, note: Option<&str>) -> AutonomyState {
    AutonomyState {
        requested: autonomy,
        native_mode: None,
        auto_answer: AutoAnswer::Never,
        note: note.map(str::to_owned),
    }
}

/// The agent refused the level: o7 asks before every action.
fn refused_autonomy(autonomy: Autonomy, agent: &str, reason: &str) -> AutonomyState {
    AutonomyState {
        requested: autonomy,
        native_mode: None,
        auto_answer: AutoAnswer::Never,
        note: Some(format!(
            "o7 could not switch {agent} to a matching mode ({reason}), so it asks before every action."
        )),
    }
}

/// Whether `value` is valid for the option `option_id`.
fn check_option(
    options: &[ConfigOption],
    option_id: &str,
    value: &ConfigValue,
) -> Result<(), SessionError> {
    let Some(option) = options.iter().find(|option| option.id == option_id) else {
        return Err(SessionError::OptionInvalid(format!(
            "This agent has no option \"{option_id}\"."
        )));
    };
    match (&option.kind, value) {
        (ConfigKind::Select { choices, .. }, ConfigValue::Choice { value }) => {
            if choices.iter().any(|choice| choice.value == *value) {
                Ok(())
            } else {
                Err(SessionError::OptionInvalid(format!(
                    "\"{value}\" isn't one of the choices for {}.",
                    option.name
                )))
            }
        }
        (ConfigKind::Toggle { .. }, ConfigValue::Toggle { .. }) => Ok(()),
        _ => Err(SessionError::OptionInvalid(format!(
            "That kind of value doesn't fit {}.",
            option.name
        ))),
    }
}

/// Whether `answer` fits `prompt` (`Notify` takes none).
fn fits(prompt: &UiPrompt, answer: &UiAnswer) -> bool {
    match (prompt, answer) {
        (UiPrompt::Confirm { .. }, UiAnswer::Confirm { .. })
        | (UiPrompt::Input { .. }, UiAnswer::Input { .. }) => true,
        (UiPrompt::Select { options, .. }, UiAnswer::Select { index }) => {
            (*index as usize) < options.len()
        }
        // A browser step is opened (yes) or declined (no).
        (UiPrompt::Link { .. }, UiAnswer::Confirm { .. }) => true,
        (UiPrompt::Form { fields, .. }, UiAnswer::Form { accept, values }) => {
            o7_protocol::forms::check_form(fields, *accept, values).is_ok()
        }
        (UiPrompt::Questions { questions, .. }, UiAnswer::Questions { answers }) => {
            !questions.is_empty()
                && questions.len() == answers.len()
                && questions.iter().enumerate().all(|(index, question)| {
                    !question.id.is_empty()
                        && !questions[..index]
                            .iter()
                            .any(|other| other.id == question.id)
                        && answers
                            .iter()
                            .filter(|answer| answer.question_id == question.id)
                            .count()
                            == 1
                        && answers
                            .iter()
                            .find(|answer| answer.question_id == question.id)
                            .is_some_and(|answer| {
                                let text = answer.text.as_ref();
                                let choices = answer.selected.len() + usize::from(text.is_some());
                                choices > 0
                                    && (question.multi_select || choices == 1)
                                    && text.is_none_or(|text| {
                                        question.allow_free_text && !text.trim().is_empty()
                                    })
                                    && answer.selected.iter().enumerate().all(
                                        |(index, selected)| {
                                            (*selected as usize) < question.options.len()
                                                && !answer.selected[..index].contains(selected)
                                        },
                                    )
                            })
                })
        }
        _ => false,
    }
}

fn prompt_message(prompt: &UiPrompt) -> &str {
    match prompt {
        UiPrompt::Confirm { message }
        | UiPrompt::Select { message, .. }
        | UiPrompt::Input { message, .. }
        | UiPrompt::Questions { message, .. }
        | UiPrompt::Form { message, .. }
        | UiPrompt::Link { message, .. }
        | UiPrompt::Notify { message } => message,
    }
}

/// The first non-empty line, trimmed, at most 80 characters.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    cap_chars(line.to_owned(), TITLE_CHARS)
}

fn crash_message(exit: Option<&ExitInfo>) -> String {
    match exit {
        Some(ExitInfo {
            code: Some(code), ..
        }) => format!("The agent stopped unexpectedly (exit code {code})."),
        Some(ExitInfo {
            signal: Some(signal),
            ..
        }) => format!("The agent stopped unexpectedly (signal {signal})."),
        _ => "The agent stopped unexpectedly.".into(),
    }
}

/// "Hermes'", "Codex's".
fn possessive(name: &str) -> String {
    if name.ends_with('s') {
        format!("{name}'")
    } else {
        format!("{name}'s")
    }
}

/// About how much of the outbox an event will take: a delta's text, or a small fixed size for
/// the rest (they are within the event budget).
fn approx_bytes(event: &O7Event) -> usize {
    match event {
        O7Event::MessageDelta { text, .. }
        | O7Event::ThoughtDelta { text, .. }
        | O7Event::UserMessageDelta { text, .. } => text.len().saturating_add(64),
        _ => 1024,
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The ops an actor still held when the store closed, by name, split into lost writes and reads
/// (whose waiters learn the store closed, so no data goes with them).
#[derive(Debug, Default, PartialEq, Eq)]
struct DroppedOps {
    writes: Vec<&'static str>,
    reads: Vec<&'static str>,
}

impl DroppedOps {
    fn of(ops: impl IntoIterator<Item = Op>) -> DroppedOps {
        let mut dropped = DroppedOps::default();
        for op in ops {
            if op.is_read() {
                dropped.reads.push(op.name());
            } else {
                dropped.writes.push(op.name());
            }
        }
        dropped
    }
}

/// The FORCE chain on one process of a retired `generation`.
fn force_chain(generation: u64, control: ProcessControl) -> BoxFuture<'static, Done> {
    async move {
        let _report = control.shutdown(ShutdownPlan::FORCE).await;
        drop(control);
        Done::Retired(generation)
    }
    .boxed()
}

/// A driver's task, once it returns (or is aborted).
fn join_driver(generation: u64, join: JoinHandle<DriverExit>) -> BoxFuture<'static, Done> {
    async move {
        Done::DriverJoined {
            generation,
            result: join.await,
        }
    }
    .boxed()
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn next_event(driver: &mut Option<LiveDriver>) -> Option<DriverEvent> {
    match driver.as_mut().and_then(|driver| driver.events.as_mut()) {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

async fn next_progress(progress: &mut Option<mpsc::UnboundedReceiver<String>>) -> Option<String> {
    match progress {
        Some(progress) => progress.recv().await,
        None => std::future::pending().await,
    }
}

/// A start's preparation: the folder, the connection (a new session's is chosen here, once
/// detection can say which runs), detection, the adapter install and launch spec, the env. One
/// task, so dropping it (Esc, force stop, Stop, quit) kills a running install or probe. It begins
/// once no earlier generation of the chat winds down: a process still being killed holds its native
/// session, which this start may resume.
#[allow(clippy::too_many_arguments)]
async fn prepare(
    context: super::workspace_folders::LaunchFolders,
    agent_host: o7_protocol::agents::AgentHost,
    connection: StartConnection,
    detector: Arc<o7_agents::Detector>,
    env: Arc<o7_process::EnvResolver>,
    accounts: Option<Arc<crate::agent_accounts::AgentAccountsStore>>,
    progress: mpsc::UnboundedSender<String>,
    mut winding_down: watch::Receiver<usize>,
) -> Result<Prepared, StartFailure> {
    let folder = context.chat.folder.clone();
    // The actor holds the sender for as long as this task can matter.
    let _ = winding_down.wait_for(|retiring| *retiring == 0).await;
    if let o7_process::HostAvailability::Unavailable { reason } =
        o7_agents::hosts::execution(&agent_host).availability()
    {
        return Err(StartFailure {
            kind: ErrorKind::AgentUnavailable,
            message: reason,
            folder: None,
        });
    }
    check_folder(folder.clone()).await?;
    let folder_key = tokio::task::spawn_blocking(move || super::live::normalize_path(folder))
        .await
        .map_err(|_| StartFailure {
            kind: ErrorKind::Internal,
            message: "Resolving the folder failed.".into(),
            folder: None,
        })?;
    let unavailable = |error: LaunchError| match error {
        LaunchError::InstallFailed { reason } => StartFailure {
            kind: ErrorKind::SpawnFailed,
            message: reason,
            folder: None,
        },
        LaunchError::Missing { reason } | LaunchError::Broken { reason } => StartFailure {
            kind: ErrorKind::AgentUnavailable,
            message: reason,
            folder: None,
        },
    };
    let (spec, saved) = match connection {
        StartConnection::Saved(spec) => (spec, true),
        StartConnection::New { agent } => (
            Box::new(
                detector
                    .choose_connection(&agent)
                    .await
                    .map_err(unavailable)?,
            ),
            false,
        ),
    };
    match detector.detect(&spec).await {
        Availability::Missing { reason } | Availability::Broken { reason } => {
            // A saved session cannot move to another connection, but a fresh session can.
            let other = if saved {
                detector
                    .connection_for(o7_agents::agent_of(spec.id))
                    .filter(|other| other.id != spec.id)
            } else {
                None
            };
            let message = match other {
                Some(other)
                    if matches!(detector.detect(&other).await, Availability::Ready { .. }) =>
                {
                    format!(
                        "{reason} This chat's saved session uses the {} connection. Start fresh to continue in a new session on the {} connection.",
                        spec.connection, other.connection
                    )
                }
                _ => reason,
            };
            return Err(StartFailure {
                kind: ErrorKind::AgentUnavailable,
                message,
                folder: None,
            });
        }
        Availability::Ready { .. } | Availability::Checking => {}
    }
    let launch = detector
        .prepare_launch(&spec, progress.clone())
        .await
        .map_err(unavailable)?;
    let _ = progress.send("Reading your shell environment".into());
    let env = env.get().await;
    let account = start_account(accounts, &spec, &progress).await?;
    let (env, launch) = with_account(env, launch, account.as_ref());
    let (mcp_servers, memory_mcp_available, mcp_notice) =
        super::shared_mcp::prepare_launch(&context, &env, &spec, &agent_host)
            .await
            .map_err(|message| StartFailure {
                kind: ErrorKind::AgentUnavailable,
                message,
                folder: None,
            })?;
    let coordinated = matches!(
        context
            .store
            .coordinator(crate::chatstore::coordinator::Command::Member(
                context.chat.id.clone(),
            ))
            .await,
        Ok(crate::chatstore::coordinator::Reply::Member(Some(_)))
    );
    let folder_context = super::workspace_folders::resolve(
        context,
        super::workspace_folders::supported(Some(&*spec), &agent_host),
    )
    .await
    .map_err(|_| StartFailure {
        kind: ErrorKind::Storage,
        message: "Additional project folders could not be read. Retry before starting the agent."
            .into(),
        folder: None,
    })?;
    Ok(Prepared {
        memory_mcp_available,
        coordinated,
        mcp_notice,
        mcp_servers,
        spec,
        folder_context,
        launch,
        env,
        folder_key,
        account,
    })
}

/// The account a start of `spec` signs in with, read from secure storage on a blocking worker. A
/// chosen account that cannot be read fails the start: it never quietly runs on another account.
async fn start_account(
    accounts: Option<Arc<crate::agent_accounts::AgentAccountsStore>>,
    spec: &AgentSpec,
    progress: &mpsc::UnboundedSender<String>,
) -> Result<Option<crate::agent_accounts::AccountLaunch>, StartFailure> {
    let agent = o7_agents::agent_of(spec.id);
    let Some(accounts) = accounts.filter(|_| crate::agent_accounts::SUPPORTED.contains(&agent))
    else {
        return Ok(None);
    };
    let agent = agent.to_owned();
    let _ = progress.send("Reading the account's sign-in".into());
    tokio::task::spawn_blocking(move || accounts.launch(&agent))
        .await
        .map_err(|_| StartFailure {
            kind: ErrorKind::Internal,
            message: "Reading the account's sign-in failed.".into(),
            folder: None,
        })?
        .map_err(|error| StartFailure {
            kind: ErrorKind::AgentUnavailable,
            message: error.to_string(),
            folder: None,
        })
}

/// The env and launch of a start on an added account: Claude Code reads the account's token, and
/// the credentials it would prefer over that token are left out.
fn with_account(
    env: Arc<ResolvedEnv>,
    mut launch: LaunchSpec,
    account: Option<&crate::agent_accounts::AccountLaunch>,
) -> (Arc<ResolvedEnv>, LaunchSpec) {
    use crate::agent_accounts::{CLAUDE_OUTRANKING, CLAUDE_TOKEN_VARIABLE};
    let Some(account) = account else {
        return (env, launch);
    };
    let replaced = |name: &std::ffi::OsStr| {
        let key = o7_process::EnvKey::from(name);
        key == o7_process::EnvKey::from(CLAUDE_TOKEN_VARIABLE)
            || CLAUDE_OUTRANKING
                .iter()
                .any(|outranking| key == o7_process::EnvKey::from(*outranking))
    };
    let mut env = (*env).clone();
    env.vars.retain(|name, _| !replaced(name.as_os_str()));
    launch.set_env.retain(|(name, _)| !replaced(name));
    launch
        .set_env
        .push((CLAUDE_TOKEN_VARIABLE.into(), account.token.clone().into()));
    (Arc::new(env), launch)
}

/// Missing or not a folder: `FolderMissing`; refused by the OS (macOS privacy, no read
/// permission): `FolderNotAllowed`, never reported as missing.
async fn check_folder(folder: PathBuf) -> Result<(), StartFailure> {
    let shown = folder.display().to_string();
    let checked = tokio::task::spawn_blocking(move || super::folder_state(&folder, true)).await;
    match checked.unwrap_or(FolderState::Ok) {
        FolderState::Ok => Ok(()),
        FolderState::Missing => Err(StartFailure {
            kind: ErrorKind::FolderMissing,
            message: format!(
                "The chat's folder {shown} can't be found. Restore it (or reconnect its drive), then press Retry."
            ),
            folder: Some(FolderState::Missing),
        }),
        FolderState::NotAllowed => Err(StartFailure {
            kind: ErrorKind::FolderNotAllowed,
            message: format!(
                "o7 isn't allowed to open {shown}. Allow it in System Settings > Privacy & Security > Files and Folders, then press Retry."
            ),
            folder: Some(FolderState::NotAllowed),
        }),
    }
}

/// A start's native options: a continuation's (a fork, say), and for Claude its own file
/// checkpointing. Claude copies a file just before its edit tools change it, which costs far less
/// than o7's automatic checkpoints (off by default), so a sent message's Rewind can restore
/// what Claude edited in any Claude chat.
fn native_start(
    kind: o7_protocol::DriverKind,
    continuation: Option<&crate::chatstore::conversation::PendingContinuation>,
) -> o7_drivers::driver::NativeStartOptions {
    let mut options = continuation
        .map(|value| value.native_start.clone())
        .unwrap_or_default();
    if kind == o7_protocol::DriverKind::Claude {
        options.file_checkpointing = true;
    }
    options
}

#[cfg(test)]
mod native_start_tests {
    use super::native_start;
    use o7_protocol::DriverKind;

    #[test]
    fn only_claude_starts_with_its_own_file_checkpointing() {
        assert!(native_start(DriverKind::Claude, None).file_checkpointing);
        for kind in [
            DriverKind::Acp,
            DriverKind::Codex,
            DriverKind::Pi,
            DriverKind::Headless,
            DriverKind::Mock,
        ] {
            assert!(!native_start(kind, None).file_checkpointing, "{kind:?}");
        }
    }
}

#[cfg(test)]
mod dropped_ops_tests {
    use super::{DroppedOps, Op};

    #[test]
    fn dropped_ops_are_named_and_reads_are_told_apart_from_lost_writes() {
        let (page, _) = tokio::sync::oneshot::channel();
        let (barrier, _) = tokio::sync::oneshot::channel();
        let dropped = DroppedOps::of([
            Op::QueueHoldAll {
                chat_id: "c".into(),
            },
            Op::ReadPage {
                chat_id: "c".into(),
                before_seq: 7,
                max_bytes: 1024,
                reply: page,
            },
            Op::TouchChat {
                chat_id: "c".into(),
                at: 1,
            },
            Op::Barrier(barrier),
        ]);
        assert_eq!(
            dropped,
            DroppedOps {
                writes: vec!["queue_hold_all", "touch_chat"],
                reads: vec!["read_page", "barrier"],
            }
        );
        assert_eq!(DroppedOps::of([]), DroppedOps::default());
    }
}

#[cfg(test)]
mod question_validation_tests {
    use super::fits;
    use o7_protocol::forms::{UiField, UiFieldKind, UiFieldValue, UiValue};
    use o7_protocol::{UiAnswer, UiPrompt, UiQuestion, UiQuestionAnswer};

    #[test]
    fn a_form_takes_only_answers_its_fields_allow_and_a_link_takes_yes_or_no() {
        let form = UiPrompt::Form {
            message: "Deploy".into(),
            source: Some("deploy".into()),
            fields: vec![UiField {
                id: "count".into(),
                label: "Count".into(),
                description: None,
                required: true,
                kind: UiFieldKind::Number {
                    integer: true,
                    minimum: Some(1.0),
                    maximum: Some(3.0),
                    default: None,
                },
            }],
        };
        let answer = |number: f64| UiAnswer::Form {
            accept: true,
            values: vec![UiFieldValue {
                id: "count".into(),
                value: UiValue::Number { number },
            }],
        };
        assert!(fits(&form, &answer(2.0)));
        assert!(!fits(&form, &answer(4.0)));
        assert!(!fits(&form, &answer(1.5)));
        assert!(fits(
            &form,
            &UiAnswer::Form {
                accept: false,
                values: Vec::new()
            }
        ));
        assert!(!fits(&form, &UiAnswer::Confirm { yes: true }));
        let link = UiPrompt::Link {
            message: "Sign in".into(),
            source: None,
            url: "https://example.com".into(),
        };
        assert!(fits(&link, &UiAnswer::Confirm { yes: false }));
        assert!(!fits(&link, &answer(2.0)));
    }

    #[test]
    fn empty_or_ambiguous_question_ids_never_accept_an_answer() {
        let question = UiQuestion {
            id: "q1".into(),
            header: "Header".into(),
            question: "What next?".into(),
            options: Vec::new(),
            multi_select: false,
            allow_free_text: true,
        };
        let answer = UiQuestionAnswer {
            question_id: "q1".into(),
            selected: Vec::new(),
            text: Some("Continue".into()),
        };
        assert!(fits(
            &UiPrompt::Questions {
                message: String::new(),
                questions: vec![question.clone()]
            },
            &UiAnswer::Questions {
                answers: vec![answer.clone()]
            }
        ));
        assert!(!fits(
            &UiPrompt::Questions {
                message: String::new(),
                questions: Vec::new()
            },
            &UiAnswer::Questions {
                answers: Vec::new()
            }
        ));
        assert!(!fits(
            &UiPrompt::Questions {
                message: String::new(),
                questions: vec![question.clone(), question.clone()]
            },
            &UiAnswer::Questions {
                answers: vec![answer.clone(), answer]
            }
        ));
        assert!(!fits(
            &UiPrompt::Questions {
                message: String::new(),
                questions: vec![UiQuestion {
                    id: String::new(),
                    ..question
                }]
            },
            &UiAnswer::Questions {
                answers: vec![UiQuestionAnswer {
                    question_id: String::new(),
                    selected: Vec::new(),
                    text: Some("Continue".into())
                }]
            }
        ));
    }
}

/// Where an agent keeps its sessions: its config folder or home (an added account's
/// `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `PI_CODING_AGENT_DIR`, else under the person's home).
struct TerminalHome {
    vendor: super::terminal_import::Vendor,
    base: PathBuf,
}

impl TerminalHome {
    /// The session's file, when the agent keeps one and has written it.
    fn session_file(&self, id: &str) -> Option<PathBuf> {
        use super::terminal_import::{
            Vendor, antigravity_conversation, claude_session, codex_session, cursor_transcript,
            grok_session, pi_session,
        };
        match self.vendor {
            Vendor::Claude => claude_session(&self.base, id),
            Vendor::Codex => codex_session(&self.base, id),
            Vendor::Grok => grok_session(&self.base, id),
            Vendor::Pi => pi_session(&self.base, id),
            Vendor::Cursor => cursor_transcript(&self.base, id),
            Vendor::Antigravity => antigravity_conversation(&self.base, id),
            Vendor::Hermes | Vendor::OpenCode => None,
        }
    }

    /// Whether the agent wrote the session down, where its record can be looked for here (its
    /// session files); `None` where it cannot (a database, an undocumented store).
    fn recorded(&self, id: &str) -> Option<bool> {
        use super::terminal_import::Vendor;
        match self.vendor {
            Vendor::Hermes | Vendor::OpenCode => None,
            _ => Some(self.session_file(id).is_some()),
        }
    }
}

fn terminal_home(
    agent: o7_drivers::terminal::TerminalAgent,
    launch: &LaunchSpec,
    env: &ResolvedEnv,
) -> TerminalHome {
    use super::terminal_import::Vendor;
    use o7_drivers::terminal::TerminalAgent;
    let variable = |name: &str| -> Option<PathBuf> {
        launch
            .set_env
            .iter()
            .rev()
            .find(|(key, _)| o7_process::EnvKey::new(key.clone()) == o7_process::EnvKey::from(name))
            .map(|(_, value)| PathBuf::from(value))
            .or_else(|| {
                env.vars
                    .get(&o7_process::EnvKey::from(name))
                    .map(PathBuf::from)
            })
            .filter(|path| path.is_absolute())
    };
    let home = variable(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .or_else(|| variable("HOME"))
        .unwrap_or_default();
    let (vendor, base) = match agent {
        TerminalAgent::Claude => (
            Vendor::Claude,
            variable("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
        ),
        TerminalAgent::Codex => (
            Vendor::Codex,
            variable("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
        ),
        TerminalAgent::Grok => (
            Vendor::Grok,
            variable("GROK_HOME").unwrap_or_else(|| home.join(".grok")),
        ),
        TerminalAgent::Pi => (
            Vendor::Pi,
            variable("PI_CODING_AGENT_DIR").unwrap_or_else(|| home.join(".pi").join("agent")),
        ),
        TerminalAgent::OhMyPi => (Vendor::Pi, home.join(".omp").join("agent")),
        TerminalAgent::Hermes => (Vendor::Hermes, home),
        TerminalAgent::OpenCode => (Vendor::OpenCode, home),
        TerminalAgent::Antigravity => (
            Vendor::Antigravity,
            home.join(".gemini").join("antigravity-cli"),
        ),
        TerminalAgent::Cursor => (Vendor::Cursor, home),
    };
    TerminalHome { vendor, base }
}

/// The environment an agent's own command runs with when its sessions are read through it: the
/// chat's, and the connection's own variables (an added account's), as the agent's Chat start
/// gets them.
fn cli_env(
    launch: &LaunchSpec,
    env: &ResolvedEnv,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let mut vars: std::collections::BTreeMap<o7_process::EnvKey, std::ffi::OsString> = env
        .vars
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for (key, value) in &launch.set_env {
        vars.insert(o7_process::EnvKey::new(key.clone()), value.clone());
    }
    vars.into_iter()
        .map(|(key, value)| (key.as_os_str().to_owned(), value))
        .collect()
}

/// Where a terminal's conversation will be read from, and how much of its session file is
/// already there.
fn terminal_import_plan(
    home: TerminalHome,
    session: &o7_drivers::terminal::TerminalSession,
    folder: &std::path::Path,
    cli: super::terminal_import::Cli,
) -> TerminalImport {
    use super::terminal_import::length;
    use o7_drivers::terminal::TerminalSession;
    let native_id = match session {
        TerminalSession::Resume(id) | TerminalSession::Named(id) => Some(id.clone()),
        TerminalSession::New => None,
    };
    let file = native_id.as_deref().and_then(|id| home.session_file(id));
    TerminalImport {
        vendor: home.vendor,
        home: home.base,
        native_id,
        from: file.as_deref().map_or(0, length),
        folder: folder.to_path_buf(),
        opened_at: now_millis(),
        cli,
    }
}

/// Reads what the terminal added to its session (blocking: run off the actor), and learns the
/// session a new terminal started where the agent named it itself.
fn read_terminal(plan: &TerminalImport) -> TerminalRead {
    use super::terminal_import::{
        Imported, Vendor, claude_session, codex_session, cursor_transcript, grok_session,
        newest_antigravity_conversation, newest_codex_session, newest_cursor_chat,
        newest_pi_session, pi_session, read_hermes, read_opencode, read_since,
    };
    let id = plan.native_id.as_deref();
    let from_file = |file: Option<PathBuf>| {
        file.map(|path| read_since(plan.vendor, &path, plan.from))
            .unwrap_or_default()
    };
    let (imported, learned, unread) = match plan.vendor {
        Vendor::Claude => (
            from_file(id.and_then(|id| claude_session(&plan.home, id))),
            None,
            false,
        ),
        Vendor::Codex => match id {
            Some(id) => (from_file(codex_session(&plan.home, id)), None, false),
            None => match newest_codex_session(&plan.home, &plan.folder, plan.opened_at) {
                Some((path, thread)) => (read_since(Vendor::Codex, &path, 0), Some(thread), false),
                None => (Imported::default(), None, false),
            },
        },
        Vendor::Grok => (
            from_file(id.and_then(|id| grok_session(&plan.home, id))),
            None,
            false,
        ),
        Vendor::Pi => match id {
            Some(id) => (from_file(pi_session(&plan.home, id)), None, false),
            None => match newest_pi_session(&plan.home, &plan.folder, plan.opened_at) {
                Some((path, session)) => (read_since(Vendor::Pi, &path, 0), Some(session), false),
                None => (Imported::default(), None, false),
            },
        },
        Vendor::Hermes => {
            let (imported, learned) = read_hermes(&plan.cli, &plan.folder, id, plan.opened_at);
            (imported, learned, false)
        }
        Vendor::OpenCode => {
            let (imported, learned) = read_opencode(&plan.cli, &plan.folder, id, plan.opened_at);
            (imported, learned, false)
        }
        Vendor::Antigravity => (
            Imported::default(),
            id.is_none()
                .then(|| newest_antigravity_conversation(&plan.home, plan.opened_at))
                .flatten(),
            true,
        ),
        Vendor::Cursor => match id {
            Some(id) => (from_file(cursor_transcript(&plan.home, id)), None, false),
            None => match newest_cursor_chat(&plan.home, &plan.folder, plan.opened_at) {
                Some(chat) => {
                    let file = cursor_transcript(&plan.home, &chat);
                    (
                        file.map(|path| read_since(Vendor::Cursor, &path, 0))
                            .unwrap_or_default(),
                        Some(chat),
                        false,
                    )
                }
                None => (Imported::default(), None, true),
            },
        },
    };
    TerminalRead {
        session_known: id.is_some() || learned.is_some(),
        imported,
        learned,
        unread,
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
