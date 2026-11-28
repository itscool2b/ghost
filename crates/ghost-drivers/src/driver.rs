//! The driver contract: how a session actor and an agent driver talk.
//!
//! A driver is an actor. [`AgentDriver::run`] gets a [`DriverContext`]: an unbounded channel of
//! [`DriverCommand`]s in, a [`EventSender`] out (bounded by count and bytes), the [`AgentSpawner`]
//! every process goes through, and the [`SessionSetup`]. Only one direction is bounded, so the actor
//! and the driver can never wait on each other, and the spawner records every process for the
//! session, so force stop reaches them even when a driver is wedged.
//!
//! Contract every driver keeps:
//!
//! 1. Before [`DriverEvent::Ready`], only `Status`, `RunState`, `BackgroundWork`, `Options`, `Commands`,
//!    `Title`, `Autonomy`, `AuthRequired`, `Event` with a `Notice` or `UsageUpdated` (untagged), and
//!    untagged `Permission` and `UiRequest` (an agent may ask during setup). A fatal start problem ends `run` with
//!    [`DriverExit::StartFailed`] (after `AuthRequired` when that is the cause).
//! 2. Every `Prompt` gets exactly one [`DriverEvent::TurnEnded`] with its turn id, after every event
//!    tagged with that turn, unless `run` ends first (then the actor ends the turn itself).
//! 3. Every forwarded `Permission` or `UiRequest` is answered to the agent: a tagged one before its
//!    turn's `TurnEnded` (the user's answer, `cancelled` at interrupt or turn end, or withdrawal); an
//!    untagged one when the user answers, when the agent withdraws it, or at shutdown.
//! 4. A closed command channel is a `Shutdown` with zero cancel grace.
//! 5. Minted ids are deterministic, so replays compare exactly.
//! 6. Right after the spawner's spawn note, the driver writes `LogNote::Start` with
//!    [`SessionSetup::start_note`] (no env); every command it receives is logged as a `cmd` record
//!    ([`LoggedCommand`]), in processing order.
//! 7. Nothing is dropped while the receiver lives: the driver waits for permits instead, and `run`
//!    returns only after stdout ended (or got its grace after the exit) and every event was handed
//!    over. A failed send (the actor dropped the receiver at force stop) ends the driver's interest
//!    in events.
//! 8. An event read after the driver processed `Prompt(t)` and before that prompt's reply is tagged
//!    `t`; every other event is untagged, including updates the agent sends after its reply and
//!    requests from background work.
//! 9. A driver with work that outlives a prompt emits `RunState` before `Ready` and on every run
//!    state change. Its `TurnEnded` finishes the prompt only; `RunState { idle: true }` permits
//!    another prompt. Drivers that omit `RunState` retain the turn-based readiness contract. The
//!    session also sends the next prompt when the agent reported work in progress, with no prompt
//!    open, for its idle grace without an idle report, or when the person presses Send now; a
//!    driver whose agent queues input during such work accepts that prompt.
//! 10. A driver whose agent says which work of its own it left running (a command in the
//!     background, an agent it started) emits `BackgroundWork` whenever that count changes. It
//!     is separate from `RunState`: an agent may report itself idle, ready for the next prompt,
//!     while such work runs. The session does not stop an agent as idle while the count is above
//!     zero, since stopping it ends that work and what the agent would have done when it finished.
//!
//! Error text crosses raw: a driver holds no session redactor, so it puts stderr tails and the
//! agent's error text into [`DriverError`] and [`DriverExit::Crashed`] unredacted, and never writes
//! them to the app log. The session actor redacts them, with the session's secret literals, before
//! it shows or stores them.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use ghost_process::{
    ExitInfo, LaunchSpec, ResolvedEnv, ShutdownPlan, SpawnError, SpawnSpec, SpawnedProcess,
};
use ghost_protocol::session::{AuthPrompt, ConfigOption, ConfigValue};
use ghost_protocol::{
    Autonomy, AutonomyState, Capabilities, DriverKind, ErrorKind, GhostEvent, PermissionRequest,
    PromptPart, SlashCommand, StopReason, TokenUsage, UiAnswer, UiRequest,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

/// Runs one agent session.
pub trait AgentDriver: Send + 'static {
    /// Runs until the session ends; never panics on agent input. Returns only after stdout reached
    /// its end (or its grace after the process exited) and every event was handed to `ctx.events`.
    /// If the future is dropped or panics, the session actor force-stops every process this driver
    /// spawned.
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit>;
}

/// Creates the driver for a [`DriverKind`].
pub trait DriverFactory: Send + Sync + 'static {
    fn create(&self, kind: DriverKind) -> Box<dyn AgentDriver>;
}

/// What drivers spawn through; the session's implementation opens the protocol log and records the
/// control.
pub trait AgentSpawner: Send + Sync + 'static {
    fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>>;
}

/// Everything a driver runs with.
pub struct DriverContext {
    pub commands: mpsc::UnboundedReceiver<DriverCommand>,
    pub events: EventSender,
    pub spawner: Arc<dyn AgentSpawner>,
    pub setup: SessionSetup,
}

impl fmt::Debug for DriverContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DriverContext")
            .field("setup", &self.setup)
            .finish_non_exhaustive()
    }
}

/// Native startup choices. Drivers that do not support these keep the defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NativeStartOptions {
    /// An explicit new transcript UUID, used by deterministic replay. Fresh starts otherwise
    /// allocate their own UUID, independently of the chat id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_session_id: Option<String>,
    /// Continue `SessionSetup::resume` into a new transcript, preserving the source.
    #[serde(skip_serializing_if = "is_false")]
    pub fork: bool,
    /// Resume through this native user message UUID. Requires a resume source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_at: Option<String>,
    /// Explicitly enable the native SDK's file checkpointing.
    #[serde(skip_serializing_if = "is_false")]
    pub file_checkpointing: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// How the session wants the agent started.
#[derive(Debug, Clone)]
pub struct SessionSetup {
    /// Immutable launch snapshot. Values are never included in the start note.
    pub mcp_servers: Vec<ghost_protocol::mcp::ResolvedMcpServer>,
    /// Private, app-owned blob directory. Never an arbitrary source file or agent credential path.
    pub attachment_dir: Option<PathBuf>,
    /// Normalizes returned agent images into chat-owned profile attachments before mapping.
    pub attachment_writer: Option<Arc<dyn crate::incoming_images::AttachmentWriter>>,
    pub chat_id: String,
    pub agent_id: String,
    pub dialect: String,
    /// The chat folder on this machine.
    pub folder: PathBuf,
    /// The folder as the agent sees it (the ACP `cwd`); in M1a, `folder` as given.
    pub agent_folder: String,
    /// Explicit device-local project folders, populated only for a supported native transport.
    pub add_dirs: Vec<PathBuf>,
    pub launch: LaunchSpec,
    pub env: Arc<ResolvedEnv>,
    /// The native session to continue.
    pub resume: Option<String>,
    pub native_start: NativeStartOptions,
    pub autonomy: Autonomy,
    /// The user's explicit choices, by option id, re-applied when the agent starts.
    pub desired: BTreeMap<String, ConfigValue>,
    pub timings: DriverTimings,
    pub client_version: String,
}

impl SessionSetup {
    /// The `start` note a driver logs right after the spawn note (contract rule 6): what replay
    /// needs to rebuild the setup, and never the env.
    ///
    /// `{"agent_id":"hermes","dialect":"hermes","resume":null,"autonomy":"full_auto","desired":{},
    /// "spawn_id":"019a..."}`
    pub fn start_note(&self, spawn_id: &str) -> serde_json::Value {
        let mut note = serde_json::json!({
            "agent_id": self.agent_id,
            "dialect": self.dialect,
            "resume": self.resume,
            "autonomy": self.autonomy,
            "desired": self.desired,
            "spawn_id": spawn_id,
        });
        if self.native_start != NativeStartOptions::default() {
            note["native_start"] = serde_json::json!(self.native_start);
        }
        note
    }
}

/// A driver's deadlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverTimings {
    /// Opening a session fails after this long without a reply or an update.
    pub open_idle: Duration,
    /// ... or after this long in all (a long replay keeps the open alive until then).
    pub open_ceiling: Duration,
    /// Every other request to the agent, except a prompt, which has none.
    pub request: Duration,
    /// Slash command lists are debounced this long.
    pub commands_debounce: Duration,
}

impl DriverTimings {
    /// 60 s, 10 min, 30 s, 250 ms.
    pub const STANDARD: DriverTimings = DriverTimings {
        open_idle: Duration::from_secs(60),
        open_ceiling: Duration::from_secs(10 * 60),
        request: Duration::from_secs(30),
        commands_debounce: Duration::from_millis(250),
    };
}

/// The turn a prompt starts, minted by the session actor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRef {
    pub turn_id: String,
}

/// The answer to a permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// The option id the user (or the policy) chose.
    Selected(String),
    Cancelled,
}

/// How a graceful stop runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopPlan {
    /// How long a running turn gets to answer its cancel.
    pub cancel_grace: Duration,
    pub process: ShutdownPlan,
}

/// Native file checkpoint results. A preview does not modify files. A real rewind can skip
/// paths for link safety; callers must report those skips rather than promise a complete restore.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRewindResult {
    #[serde(default)]
    pub files_changed: Vec<String>,
    pub insertions: Option<u64>,
    pub deletions: Option<u64>,
    pub skipped_links: Option<u64>,
}

/// Native delivery acknowledgement. An uncertain response must never be retried automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SteerResult {
    Accepted,
    Rejected { message: String },
    Unconfirmed { message: String },
}

/// What the session actor asks of a driver.
#[derive(Debug)]
pub enum DriverCommand {
    McpAction {
        server_name: String,
        action: ghost_protocol::mcp::McpSessionAction,
        reply: oneshot::Sender<Result<ghost_protocol::mcp::McpActionResult, DriverError>>,
    },
    /// Sent only in Ready with no turn running; answered by exactly one [`DriverEvent::TurnEnded`].
    /// A part the driver does not support ends the turn at once with `Error` and
    /// [`ErrorKind::Internal`] (the actor checks first).
    Prompt {
        turn: TurnRef,
        parts: Vec<PromptPart>,
    },
    /// Explicit user input for exactly this running turn. Never queue it for a later turn.
    Steer {
        turn_id: String,
        message_id: String,
        parts: Vec<PromptPart>,
        reply: oneshot::Sender<SteerResult>,
    },
    /// Soft interrupt; idempotent; the turn still ends with `TurnEnded`.
    Interrupt,
    /// The plan of a pending plan approval, as the person edited it. The driver answers the
    /// approval that follows with this text as the approved plan. Sent only to a driver whose
    /// capabilities say `plan_edits`, and only for a request that carries a plan.
    EditPlan { request_id: String, plan: String },
    /// Declines a pending plan approval. With `note`, the agent reads the person's words with
    /// the refusal, in the same turn, and keeps planning. Without one, the turn ends there and
    /// the agent waits for the person. Sent only to a driver whose capabilities say
    /// `plan_notes`, and only for a request that is a plan approval; it stands in for the
    /// `RespondPermission` that would have declined it.
    DeclinePlan {
        request_id: String,
        note: Option<String>,
    },
    RespondPermission {
        request_id: String,
        outcome: PermissionOutcome,
    },
    /// `None` = dismissed.
    RespondUi {
        request_id: String,
        answer: Option<UiAnswer>,
    },
    SetOption {
        option_id: String,
        value: ConfigValue,
        reply: oneshot::Sender<Result<(), DriverError>>,
    },
    SetAutonomy {
        autonomy: Autonomy,
        reply: oneshot::Sender<Result<AutonomyState, DriverError>>,
    },
    /// Rewind native file checkpoints only. Conversation history is unchanged. Must be explicitly
    /// enabled at startup, and runs only while idle with no outstanding approvals or mutations.
    RewindFiles {
        user_message_id: String,
        dry_run: bool,
        reply: oneshot::Sender<Result<FileRewindResult, DriverError>>,
    },
    /// Graceful end: interrupt a running turn, wait `cancel_grace` for its reply, close stdin, run
    /// the shutdown chain.
    Shutdown { plan: StopPlan },
}

/// What a driver reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DriverEvent {
    Status(DriverStatus),
    /// Authoritative run state, including background work. Emitting this opts the session into
    /// waiting for idle separately from `TurnEnded`; emit the initial state before `Ready`.
    RunState {
        idle: bool,
    },
    /// How many pieces of its own work the agent left running in the background (rule 10).
    /// Untagged. A driver that never emits it has none.
    BackgroundWork {
        open: u32,
    },
    Ready(ReadyInfo),
    /// After Ready, resolves a resume path to the authoritative native transcript id. The actor
    /// must claim and save it before sending another prompt. It does not start a new session row.
    NativeSessionId(String),
    /// Transcript events, tagged with their turn (rule 8). Never `TurnStarted`, `TurnEnded`,
    /// `UserMessageDelta`, `SteeringStatus`, `SessionUpdated`, `PromptLifecycle`, `PermissionRequested`,
    /// `PermissionResolved`, `UiRequest` or `UiResolved`: the actor emits those.
    Event {
        turn: Option<String>,
        event: GhostEvent,
    },
    TurnEnded {
        turn_id: String,
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
        error: Option<DriverError>,
    },
    Permission {
        turn: Option<String>,
        request: PermissionRequest,
    },
    /// The driver already answered the agent `cancelled`.
    PermissionWithdrawn {
        request_id: String,
    },
    UiRequest {
        turn: Option<String>,
        request: UiRequest,
    },
    UiWithdrawn {
        request_id: String,
    },
    /// The full reduced list.
    Commands(Vec<SlashCommand>),
    /// What the session loaded, in full each time (see [`ghost_protocol::session::SessionLoaded`]).
    Loaded(ghost_protocol::session::SessionLoaded),
    /// The full list with current values.
    Options(Vec<ConfigOption>),
    Autonomy(AutonomyState),
    Title(Option<String>),
    AuthRequired(AuthPrompt),
    /// The agent's account reached its usage limit: its own words, and when the limit resets.
    UsageLimit(ghost_protocol::UsageLimit),
}

impl DriverEvent {
    /// The event's size as the event channel counts it: the length of its JSON.
    pub fn approx_bytes(&self) -> usize {
        let mut count = ByteCount(0);
        match serde_json::to_writer(&mut count, self) {
            Ok(()) => count.0,
            // Nothing here fails to serialize; if it ever did, the event passes alone.
            Err(_) => DRIVER_EVENT_BYTES,
        }
    }
}

/// Counts bytes instead of storing them.
struct ByteCount(usize);

impl io::Write for ByteCount {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Where a driver is in its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverStatus {
    Spawning,
    Initializing,
    Opening,
    Configuring,
    Ready,
    Prompting,
    Cancelling,
    Closing,
}

/// The agent is up and its session open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyInfo {
    /// None when the vendor creates its transcript only on the first visible prompt. The driver
    /// reports NativeSessionId after the vendor confirms it, never a fabricated resume target.
    pub native_session_id: Option<String>,
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
    pub capabilities: Capabilities,
    pub resumed: ResumeOutcome,
    pub spawn_id: String,
    pub replay: Option<ReplaySummary>,
}

/// What became of the native session the setup asked to continue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResumeOutcome {
    New,
    Resumed,
    Loaded,
    /// A new session although one was asked to continue.
    Fresh {
        reason: String,
    },
}

/// What a load or resume replayed, for dialects whose turns outlive the process (Hermes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaySummary {
    pub user_turns: u32,
    /// The last agent reply, at most 64 KiB.
    pub last_reply: Option<String>,
    /// Of that whole reply with all whitespace removed.
    pub last_reply_sha256: Option<String>,
}

/// A failure a driver reports. `message` is a sentence the UI shows. `message` and `detail` may
/// carry the agent's own text or a stderr tail, unredacted: the session redacts both before it
/// shows or stores them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct DriverError {
    pub kind: ErrorKind,
    pub message: String,
    pub detail: Option<String>,
}

/// How `run` ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DriverExit {
    /// Asked to stop, and did.
    Closed,
    /// Ended without being asked. `stderr_tail` is the raw tail
    /// ([`ControlImpl::stderr_tail`](ghost_process::ControlImpl::stderr_tail), not redacted): the
    /// session redacts it before it shows or stores it.
    Crashed {
        exit: Option<ExitInfo>,
        stderr_tail: String,
    },
    /// Never reached Ready.
    StartFailed(DriverError),
    /// Set by the session actor only.
    Panicked,
}

/// Events in the channel at most.
pub const DRIVER_EVENT_CAPACITY: usize = 256;

/// Approximate bytes in the channel at most; one larger event passes alone.
pub const DRIVER_EVENT_BYTES: usize = 8 * 1024 * 1024;

type Item = (DriverEvent, OwnedSemaphorePermit);

/// The event channel, bounded by count and by approximate bytes; one event larger than the byte
/// bound passes alone.
pub fn event_channel() -> (EventSender, EventReceiver) {
    let (tx, rx) = mpsc::channel(DRIVER_EVENT_CAPACITY);
    let bytes = Arc::new(Semaphore::new(DRIVER_EVENT_BYTES));
    (
        EventSender {
            tx,
            bytes: Arc::clone(&bytes),
        },
        EventReceiver { rx, bytes },
    )
}

/// The driver's end of the event channel.
#[derive(Debug, Clone)]
pub struct EventSender {
    tx: mpsc::Sender<Item>,
    bytes: Arc<Semaphore>,
}

impl EventSender {
    /// Waits until an event of `bytes` fits. Cancel-safe: use it in `select!` with the outbox
    /// head's size, then send that event with the permit.
    pub async fn reserve(&self, bytes: usize) -> Result<EventPermit, ChannelClosed> {
        if self.tx.is_closed() {
            return Err(ChannelClosed);
        }
        let wanted = u32::try_from(bytes.min(DRIVER_EVENT_BYTES)).unwrap_or(u32::MAX);
        let bytes = Arc::clone(&self.bytes)
            .acquire_many_owned(wanted)
            .await
            .map_err(|_| ChannelClosed)?;
        let slot = self
            .tx
            .clone()
            .reserve_owned()
            .await
            .map_err(|_| ChannelClosed)?;
        Ok(EventPermit { slot, bytes })
    }
}

/// Room for one event in the channel.
#[derive(Debug)]
pub struct EventPermit {
    slot: mpsc::OwnedPermit<Item>,
    bytes: OwnedSemaphorePermit,
}

impl EventPermit {
    pub fn send(self, event: DriverEvent) {
        self.slot.send((event, self.bytes));
    }
}

/// The session's end of the event channel.
#[derive(Debug)]
pub struct EventReceiver {
    rx: mpsc::Receiver<Item>,
    bytes: Arc<Semaphore>,
}

impl EventReceiver {
    /// Read an already queued event, preserving the driver's authoritative option updates before acknowledgements.
    pub fn try_recv(&mut self) -> Result<DriverEvent, tokio::sync::mpsc::error::TryRecvError> {
        self.rx.try_recv().map(|(event, _bytes)| event)
    }

    /// The next event, releasing its bytes; `None` once the driver dropped its sender.
    pub async fn recv(&mut self) -> Option<DriverEvent> {
        self.rx.recv().await.map(|(event, _bytes)| event)
    }
}

impl Drop for EventReceiver {
    fn drop(&mut self) {
        // A driver waiting for byte budget learns at once that nobody listens any more.
        self.bytes.close();
    }
}

/// The session dropped the event receiver (a force stop): nothing more will be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the session no longer reads this driver's events")]
pub struct ChannelClosed;

/// Serializable mirror of [`DriverCommand`] (no reply channels), written as `cmd` log records and
/// read by replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoggedCommand {
    McpAction {
        server_name: String,
        action: ghost_protocol::mcp::McpSessionAction,
    },
    Prompt {
        turn_id: String,
        parts: Vec<PromptPart>,
    },
    Steer {
        turn_id: String,
        message_id: String,
        parts: Vec<PromptPart>,
    },
    Interrupt,
    EditPlan {
        request_id: String,
        plan: String,
    },
    DeclinePlan {
        request_id: String,
        note: Option<String>,
    },
    RespondPermission {
        request_id: String,
        /// `None` = cancelled.
        option_id: Option<String>,
    },
    RespondUi {
        request_id: String,
        answer: Option<UiAnswer>,
    },
    SetOption {
        option_id: String,
        value: ConfigValue,
    },
    SetAutonomy {
        autonomy: Autonomy,
    },
    RewindFiles {
        user_message_id: String,
        dry_run: bool,
    },
    Shutdown,
}

impl From<&DriverCommand> for LoggedCommand {
    fn from(command: &DriverCommand) -> LoggedCommand {
        match command {
            DriverCommand::McpAction {
                server_name,
                action,
                ..
            } => LoggedCommand::McpAction {
                server_name: server_name.clone(),
                action: *action,
            },
            DriverCommand::Prompt { turn, parts } => LoggedCommand::Prompt {
                turn_id: turn.turn_id.clone(),
                parts: parts.clone(),
            },
            DriverCommand::Steer {
                turn_id,
                message_id,
                parts,
                ..
            } => LoggedCommand::Steer {
                turn_id: turn_id.clone(),
                message_id: message_id.clone(),
                parts: parts.clone(),
            },
            DriverCommand::Interrupt => LoggedCommand::Interrupt,
            DriverCommand::EditPlan { request_id, plan } => LoggedCommand::EditPlan {
                request_id: request_id.clone(),
                plan: plan.clone(),
            },
            DriverCommand::DeclinePlan { request_id, note } => LoggedCommand::DeclinePlan {
                request_id: request_id.clone(),
                note: note.clone(),
            },
            DriverCommand::RespondPermission {
                request_id,
                outcome,
            } => LoggedCommand::RespondPermission {
                request_id: request_id.clone(),
                option_id: match outcome {
                    PermissionOutcome::Selected(option_id) => Some(option_id.clone()),
                    PermissionOutcome::Cancelled => None,
                },
            },
            DriverCommand::RespondUi { request_id, answer } => LoggedCommand::RespondUi {
                request_id: request_id.clone(),
                answer: answer.clone(),
            },
            DriverCommand::SetOption {
                option_id, value, ..
            } => LoggedCommand::SetOption {
                option_id: option_id.clone(),
                value: value.clone(),
            },
            DriverCommand::SetAutonomy { autonomy, .. } => LoggedCommand::SetAutonomy {
                autonomy: *autonomy,
            },
            DriverCommand::RewindFiles {
                user_message_id,
                dry_run,
                ..
            } => LoggedCommand::RewindFiles {
                user_message_id: user_message_id.clone(),
                dry_run: *dry_run,
            },
            DriverCommand::Shutdown { .. } => LoggedCommand::Shutdown,
        }
    }
}

#[cfg(test)]
mod tests {
    use ghost_process::{EnvMap, EnvSource, ProcessCwd};
    use ghost_protocol::{NoticeLevel, StopReason};
    use serde_json::json;

    use super::*;

    fn notice(text: &str) -> DriverEvent {
        DriverEvent::Event {
            turn: None,
            event: GhostEvent::Notice {
                level: NoticeLevel::Info,
                message: text.into(),
            },
        }
    }

    #[tokio::test]
    async fn events_arrive_in_order() {
        let (sender, mut receiver) = event_channel();
        for text in ["a", "b", "c"] {
            let event = notice(text);
            sender
                .reserve(event.approx_bytes())
                .await
                .expect("room")
                .send(event);
        }
        drop(sender);
        for text in ["a", "b", "c"] {
            assert_eq!(receiver.recv().await, Some(notice(text)));
        }
        assert_eq!(receiver.recv().await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn the_channel_is_bounded_by_count() {
        let (sender, mut receiver) = event_channel();
        for _ in 0..DRIVER_EVENT_CAPACITY {
            sender.reserve(10).await.expect("room").send(notice("x"));
        }
        let full = tokio::time::timeout(Duration::from_secs(1), sender.reserve(10)).await;
        assert!(full.is_err(), "the 257th event waits");
        receiver.recv().await.expect("an event");
        sender
            .reserve(10)
            .await
            .expect("room again")
            .send(notice("y"));
    }

    #[tokio::test(start_paused = true)]
    async fn the_channel_is_bounded_by_bytes_and_a_huge_event_passes_alone() {
        let (sender, mut receiver) = event_channel();
        sender
            .reserve(DRIVER_EVENT_BYTES / 2)
            .await
            .expect("room")
            .send(notice("half"));
        let over = tokio::time::timeout(
            Duration::from_secs(1),
            sender.reserve(DRIVER_EVENT_BYTES / 2 + 1),
        )
        .await;
        assert!(over.is_err(), "more than the byte bound waits");
        // Larger than the whole bound: waits for everything, then passes alone.
        let sender_for_huge = sender.clone();
        let huge =
            tokio::spawn(async move { sender_for_huge.reserve(DRIVER_EVENT_BYTES * 3).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!huge.is_finished());
        assert_eq!(receiver.recv().await, Some(notice("half")));
        let permit = huge.await.expect("join").expect("passes alone");
        permit.send(notice("huge"));
        assert_eq!(receiver.recv().await, Some(notice("huge")));
    }

    #[tokio::test]
    async fn a_dropped_receiver_fails_reserves_even_while_waiting() {
        let (sender, receiver) = event_channel();
        let waiting = sender.reserve(DRIVER_EVENT_BYTES).await.expect("room");
        let blocked = tokio::spawn({
            let sender = sender.clone();
            async move { sender.reserve(1).await.map(|_| ()) }
        });
        tokio::task::yield_now().await;
        drop(receiver);
        assert_eq!(blocked.await.expect("join"), Err(ChannelClosed));
        assert!(matches!(sender.reserve(1).await, Err(ChannelClosed)));
        drop(waiting);
    }

    #[test]
    fn approx_bytes_counts_the_events_strings() {
        let small = notice("x");
        let large = notice(&"y".repeat(10_000));
        assert!(small.approx_bytes() > 0);
        assert!(large.approx_bytes() >= 10_000);
        assert_eq!(
            small.approx_bytes(),
            serde_json::to_vec(&small).expect("serialize").len()
        );
    }

    #[test]
    fn logged_commands_mirror_driver_commands() {
        let (reply, _rx) = oneshot::channel();
        let cases = [
            (
                DriverCommand::Prompt {
                    turn: TurnRef {
                        turn_id: "019a".into(),
                    },
                    parts: vec![PromptPart::Text {
                        text: "Reply with exactly: ok".into(),
                    }],
                },
                json!({"type": "prompt", "turn_id": "019a", "parts": [{"type": "text", "text": "Reply with exactly: ok"}]}),
            ),
            (DriverCommand::Interrupt, json!({"type": "interrupt"})),
            (
                DriverCommand::DeclinePlan {
                    request_id: "t.p0".into(),
                    note: Some("Which engines?".into()),
                },
                json!({"type": "decline_plan", "request_id": "t.p0", "note": "Which engines?"}),
            ),
            (
                DriverCommand::DeclinePlan {
                    request_id: "t.p0".into(),
                    note: None,
                },
                json!({"type": "decline_plan", "request_id": "t.p0", "note": null}),
            ),
            (
                DriverCommand::RespondPermission {
                    request_id: "t.p1".into(),
                    outcome: PermissionOutcome::Selected("allow".into()),
                },
                json!({"type": "respond_permission", "request_id": "t.p1", "option_id": "allow"}),
            ),
            (
                DriverCommand::RespondPermission {
                    request_id: "t.p2".into(),
                    outcome: PermissionOutcome::Cancelled,
                },
                json!({"type": "respond_permission", "request_id": "t.p2", "option_id": null}),
            ),
            (
                DriverCommand::RespondUi {
                    request_id: "u".into(),
                    answer: Some(UiAnswer::Select { index: 1 }),
                },
                json!({"type": "respond_ui", "request_id": "u", "answer": {"type": "select", "index": 1}}),
            ),
            (
                DriverCommand::SetOption {
                    option_id: "model".into(),
                    value: ConfigValue::Choice {
                        value: "fast".into(),
                    },
                    reply,
                },
                json!({"type": "set_option", "option_id": "model", "value": {"type": "choice", "value": "fast"}}),
            ),
            (
                DriverCommand::Shutdown {
                    plan: StopPlan {
                        cancel_grace: Duration::from_secs(3),
                        process: ShutdownPlan::STANDARD,
                    },
                },
                json!({"type": "shutdown"}),
            ),
        ];
        for (command, expected) in cases {
            let logged = LoggedCommand::from(&command);
            assert_eq!(serde_json::to_value(&logged).expect("serialize"), expected);
            let back: LoggedCommand = serde_json::from_value(expected).expect("deserialize");
            assert_eq!(back, logged);
        }
        let (reply, _rx) = oneshot::channel();
        assert_eq!(
            LoggedCommand::from(&DriverCommand::SetAutonomy {
                autonomy: Autonomy::Ask,
                reply
            }),
            LoggedCommand::SetAutonomy {
                autonomy: Autonomy::Ask
            }
        );
    }

    #[test]
    fn driver_events_and_exits_serialize_for_goldens() {
        let ended = DriverEvent::TurnEnded {
            turn_id: "t".into(),
            stop_reason: StopReason::Cancelled,
            usage: None,
            error: Some(DriverError {
                kind: ErrorKind::AgentError,
                message: "m".into(),
                detail: None,
            }),
        };
        let value = serde_json::to_value(&ended).expect("serialize");
        assert_eq!(value["type"], "turn_ended");
        assert_eq!(value["data"]["error"]["kind"], "agent_error");
        assert_eq!(
            serde_json::from_value::<DriverEvent>(value).expect("deserialize"),
            ended
        );
        for event in [
            DriverEvent::Commands(Vec::new()),
            DriverEvent::Title(None),
            DriverEvent::Status(DriverStatus::Opening),
            DriverEvent::Ready(ReadyInfo {
                native_session_id: Some("s".into()),
                agent_name: Some("Hermes".into()),
                agent_version: None,
                capabilities: Capabilities::default(),
                resumed: ResumeOutcome::Fresh {
                    reason: "gone".into(),
                },
                spawn_id: "019a".into(),
                replay: Some(ReplaySummary {
                    user_turns: 2,
                    last_reply: Some("ok".into()),
                    last_reply_sha256: None,
                }),
            }),
        ] {
            let text = serde_json::to_string(&event).expect("serialize");
            assert_eq!(
                serde_json::from_str::<DriverEvent>(&text).expect("deserialize"),
                event
            );
        }
        let crashed = DriverExit::Crashed {
            exit: Some(ExitInfo {
                code: Some(1),
                signal: None,
                at_ms: 5,
            }),
            stderr_tail: "boom".into(),
        };
        let text = serde_json::to_string(&crashed).expect("serialize");
        assert_eq!(
            serde_json::from_str::<DriverExit>(&text).expect("deserialize"),
            crashed
        );
    }

    #[test]
    fn the_start_note_names_the_setup_but_never_the_env() {
        let mut vars = EnvMap::new();
        vars.insert("ANTHROPIC_API_KEY".into(), "sk-ant-secret".into());
        let mut setup = SessionSetup {
            mcp_servers: Vec::new(),
            attachment_dir: None,
            attachment_writer: None,
            chat_id: "c".into(),
            agent_id: "hermes".into(),
            dialect: "hermes".into(),
            folder: PathBuf::from("/work"),
            agent_folder: "/work".into(),
            add_dirs: Vec::new(),
            launch: LaunchSpec {
                label: "hermes acp".into(),
                program: PathBuf::from("/bin/hermes"),
                args: vec!["acp".into()],
                cwd: ProcessCwd::ChatFolder,
                set_env: Vec::new(),
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(60),
            },
            env: Arc::new(ResolvedEnv {
                vars,
                source: EnvSource::Fallback {
                    reason: "test".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            resume: None,
            native_start: NativeStartOptions::default(),
            autonomy: Autonomy::FullAuto,
            desired: BTreeMap::new(),
            timings: DriverTimings::STANDARD,
            client_version: "0.1.0".into(),
        };
        let note = setup.start_note("019a");
        assert_eq!(
            serde_json::to_string(&note).expect("serialize"),
            r#"{"agent_id":"hermes","dialect":"hermes","resume":null,"autonomy":"full_auto","desired":{},"spawn_id":"019a"}"#
        );
        assert!(!format!("{setup:?}").contains("sk-ant-secret"));
        setup.native_start.fork = true;
        setup.native_start.resume_at = Some("019a0000-0000-7000-8000-000000000009".into());
        let note = setup.start_note("019a");
        assert_eq!(
            note["native_start"],
            serde_json::json!({"fork":true,"resume_at":"019a0000-0000-7000-8000-000000000009"})
        );
        assert_eq!(
            serde_json::from_value::<NativeStartOptions>(note["native_start"].clone())
                .expect("native options round trip"),
            setup.native_start
        );
    }
}
