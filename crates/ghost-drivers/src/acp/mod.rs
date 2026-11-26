//! The ACP driver: startup (spawn, `initialize`, open with resume, load or new, configure), the
//! main loop that handles commands, stdout and event permits in wire order on one task,
//! cancellation, permissions, and the closing and draining phases that hand over every event
//! before `run` returns.
//!
//! Everything happens on one task, in one `select!` loop that waits only on its own inputs and on
//! event-channel permits: commands, permits for the outbox head, stdout (while the outbox has room),
//! the process exit, the shutdown chain, and deadlines. Outgoing messages go through [`RpcOut`],
//! which never blocks. While the session is slow the outbox fills, stdout reading stops and the
//! agent blocks on its own write, so nothing it wrote is lost and memory stays bounded.
//!
//! Phases: Running (commands are read), Closing (a `Shutdown` or a failed start: cancel a running
//! turn and wait at most the grace for its reply, close stdin, run the shutdown chain while stdout
//! keeps being read), Draining (stdout ended or the process exited: no more commands; stdout is read
//! to its end, but after the exit for at most [`PIPE_EOF_GRACE`] in all, a grace that runs only
//! while the outbox has room; EOF with the process alive gets 2 s, then the tree is killed). `run`
//! returns once stdout ended, the process exited and the outbox is empty.

pub mod callbacks;
pub mod dialect;
pub mod map;
pub mod options;
pub mod permissions;
pub mod requests;
pub mod terminals;
mod vendor_updates;
pub mod wire;

#[cfg(test)]
mod callbacks_tests;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use ghost_process::{
    ExitInfo, LogNote, MAX_LINE_BYTES, PIPE_EOF_GRACE, ProcessControl, ProtocolLog,
    STDERR_TAIL_BYTES, ShutdownPlan, ShutdownReport, SpawnError, SpawnSpec, SpawnedProcess,
    StdoutItem, TopKeys,
};
use ghost_protocol::session::{AuthCommand, AuthPrompt, ConfigCategory, ConfigValue};
use ghost_protocol::{
    AutoAnswer, Autonomy, AutonomyState, Capabilities, ErrorKind, GhostEvent, NoticeLevel,
    PermissionRequest, PromptPart, SlashCommand, StopReason, TokenUsage, UiAnswer, UiRequest,
    UsageScope,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::driver::{
    AgentDriver, ChannelClosed, DriverCommand, DriverContext, DriverError, DriverEvent, DriverExit,
    DriverStatus, EventPermit, EventSender, LoggedCommand, PermissionOutcome, ReadyInfo,
    ReplaySummary, ResumeOutcome, SessionSetup, StopPlan,
};
use crate::jsonrpc::{Incoming, RpcError, RpcOut, codes, parse_line};
use dialect::{AcpDialect, AutonomyCandidate, DialectReply, dialect_for};
use map::{Mapped, Mapper};
use options::{OptionModel, value_label};
use permissions::{Pending, PendingPermissions};
use requests::methods;
use wire::{
    InitializeReply, OpenReply, PermissionParams, PromptReply, Update, UpdateNotification,
    WireUsage,
};

/// Events the driver holds before stdout reading pauses.
pub const OUTBOX_EVENTS: usize = 64;

/// Approximate bytes the driver holds before stdout reading pauses.
pub const OUTBOX_BYTES: usize = 8 * 1024 * 1024;

/// Updates buffered while `session/new` is pending; later ones are dropped with a log line.
pub const EARLY_UPDATES: usize = 256;

/// The JSON bytes those buffered updates may hold together (one larger update passes alone), so
/// the buffer is bounded by bytes like every other stage, not only by count.
pub const EARLY_UPDATE_BYTES: usize = 8 * 1024 * 1024;

/// The last reply a replay summary keeps.
pub const REPLAY_REPLY_BYTES: usize = 64 * 1024;

/// stdout ended while the process lives: this long for it to exit before the tree is killed.
pub const EXIT_AFTER_EOF_GRACE: Duration = Duration::from_secs(2);

/// A mode change the dialect calls retryable goes out again up to this often, waiting
/// `MODE_RETRY_WAIT` times the attempt before each.
pub const MODE_RETRIES: u32 = 3;
pub const MODE_RETRY_WAIT: Duration = Duration::from_millis(250);

/// The reason a resume falls back to a new session when the agent can continue none.
pub const CANNOT_CONTINUE: &str = "this agent can't continue earlier sessions";

/// The title of a permission request that names nothing.
pub const UNTITLED_PERMISSION: &str = "The agent asks for permission";

/// The answer to a `SetOption` that waited behind earlier changes for as long as an answer may
/// take, so the session hears back inside its IPC budget.
pub const OPTION_TOO_LATE: &str =
    "The agent did not get to this change in time, so Ghost did not send it.";

/// The notice for an update over the line cap.
pub const OVERSIZED_UPDATE_NOTICE: &str =
    "Ghost skipped an update from the agent that was larger than 64 MiB.";

/// The Agent Client Protocol driver.
#[derive(Debug, Clone, Copy, Default)]
pub struct AcpDriver;

impl AcpDriver {
    pub fn new() -> AcpDriver {
        AcpDriver
    }
}

impl AgentDriver for AcpDriver {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        run(ctx).boxed()
    }
}

async fn run(ctx: DriverContext) -> DriverExit {
    let DriverContext {
        mut commands,
        events,
        spawner,
        setup,
    } = ctx;
    let mut outbox = Outbox::new(events);
    let mut held = VecDeque::new();
    outbox.push(DriverEvent::Status(DriverStatus::Spawning));
    let spec = SpawnSpec {
        launch: setup.launch.clone(),
        folder: setup.folder.clone(),
        env: Arc::clone(&setup.env),
        max_line_bytes: MAX_LINE_BYTES,
    };
    let spawning = spawner.spawn(spec);
    let process = match spawn_phase(spawning, &mut commands, &mut outbox, &mut held).await {
        Spawned::Process(process) => process,
        Spawned::Failed(error) => {
            outbox.flush().await;
            return DriverExit::StartFailed(spawn_failure(&error));
        }
        Spawned::Stopped => {
            outbox.flush().await;
            return DriverExit::Closed;
        }
    };
    process
        .log
        .note(LogNote::Start(setup.start_note(&process.spawn_id)));
    for (_, command) in &held {
        log_command(&process.log, command);
    }
    Session::new(process, commands, outbox, held, setup)
        .run()
        .await
}

enum Spawned {
    Process(SpawnedProcess),
    Failed(SpawnError),
    Stopped,
}

/// Waits for the spawn while handing over events and watching for a stop. Other commands wait
/// for `Ready`.
async fn spawn_phase(
    mut spawning: BoxFuture<'static, Result<SpawnedProcess, SpawnError>>,
    commands: &mut mpsc::UnboundedReceiver<DriverCommand>,
    outbox: &mut Outbox,
    held: &mut VecDeque<(Instant, DriverCommand)>,
) -> Spawned {
    loop {
        let head = outbox.head_bytes();
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                None | Some(DriverCommand::Shutdown { .. }) => return Spawned::Stopped,
                Some(DriverCommand::Prompt { turn, .. }) => outbox.push(not_ready(turn.turn_id)),
                Some(other) => held.push_back((Instant::now(), other)),
            },
            permit = outbox.sender.reserve(head), if outbox.has_head() => outbox.deliver(permit),
            result = &mut spawning => {
                return match result {
                    Ok(process) => Spawned::Process(process),
                    Err(error) => Spawned::Failed(error),
                };
            }
        }
    }
}

/// How a failed spawn reads: a missing program, a missing or refused folder, or anything else.
pub fn spawn_failure(error: &SpawnError) -> DriverError {
    let kind = match error {
        SpawnError::ProgramNotFound { .. } | SpawnError::HostUnavailable { .. } => {
            ErrorKind::AgentUnavailable
        }
        SpawnError::InvalidCwd { .. } => ErrorKind::FolderMissing,
        SpawnError::CwdNotAllowed { .. } => ErrorKind::FolderNotAllowed,
        SpawnError::PermissionDenied { .. } | SpawnError::NoRuntime | SpawnError::Spawn { .. } => {
            ErrorKind::SpawnFailed
        }
    };
    DriverError {
        kind,
        message: error.to_string(),
        detail: None,
    }
}

fn not_ready(turn_id: String) -> DriverEvent {
    DriverEvent::TurnEnded {
        turn_id,
        stop_reason: StopReason::Error,
        usage: None,
        error: Some(internal("The agent isn't ready for a prompt yet.")),
    }
}

fn internal(message: &str) -> DriverError {
    DriverError {
        kind: ErrorKind::Internal,
        message: message.to_owned(),
        detail: None,
    }
}

fn log_command(log: &ProtocolLog, command: &DriverCommand) {
    if let Ok(value) = serde_json::to_value(LoggedCommand::from(command)) {
        log.command(value);
    }
}

/// Events waiting for permits, bounded so a slow session pauses stdout. Transcript events produced
/// before `Ready` wait here too (rule 1) and count toward the same bound, so an agent that streams
/// while the session is being configured pauses stdout instead of growing memory.
struct Outbox {
    sender: EventSender,
    /// False once the session dropped the receiver: later events are discarded.
    open: bool,
    queue: VecDeque<(DriverEvent, usize)>,
    /// Transcript events produced before `Ready`, handed over right after it.
    held: Vec<(DriverEvent, usize)>,
    /// Approximate bytes of `queue` and `held` together.
    bytes: usize,
}

impl Outbox {
    fn new(sender: EventSender) -> Outbox {
        Outbox {
            sender,
            open: true,
            queue: VecDeque::new(),
            held: Vec::new(),
            bytes: 0,
        }
    }

    fn push(&mut self, event: DriverEvent) {
        if !self.open {
            return;
        }
        let size = event.approx_bytes();
        self.bytes = self.bytes.saturating_add(size);
        self.queue.push_back((event, size));
    }

    /// Keeps a transcript event until `Ready`; it counts toward the bound meanwhile.
    fn hold(&mut self, event: DriverEvent) {
        if !self.open {
            return;
        }
        let size = event.approx_bytes();
        self.bytes = self.bytes.saturating_add(size);
        self.held.push((event, size));
    }

    /// `Ready` is queued: what was held follows it.
    fn release_held(&mut self) {
        self.queue.extend(self.held.drain(..));
    }

    /// The start ended without `Ready`, so what was held can never be handed over (rule 1).
    /// Returns how many events that was.
    fn drop_held(&mut self) -> usize {
        let count = self.held.len();
        for (_, size) in self.held.drain(..) {
            self.bytes = self.bytes.saturating_sub(size);
        }
        count
    }

    fn head_bytes(&self) -> usize {
        self.queue.front().map_or(0, |(_, size)| *size)
    }

    fn has_head(&self) -> bool {
        !self.queue.is_empty()
    }

    fn is_full(&self) -> bool {
        self.queue.len() + self.held.len() >= OUTBOX_EVENTS || self.bytes >= OUTBOX_BYTES
    }

    fn deliver(&mut self, permit: Result<EventPermit, ChannelClosed>) {
        match permit {
            Ok(permit) => {
                if let Some((event, size)) = self.queue.pop_front() {
                    self.bytes = self.bytes.saturating_sub(size);
                    permit.send(event);
                }
            }
            Err(ChannelClosed) => self.discard(),
        }
    }

    /// The session stopped reading (a force stop): nothing more will be handed over.
    fn discard(&mut self) {
        self.open = false;
        self.queue.clear();
        self.held.clear();
        self.bytes = 0;
    }

    /// Hands over everything queued (where nothing else needs waiting for).
    async fn flush(&mut self) {
        while self.has_head() {
            let permit = self.sender.reserve(self.head_bytes()).await;
            self.deliver(permit);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    Closing,
    Draining,
}

/// How `run` ends, decided when the end begins.
#[derive(Debug)]
enum End {
    Closed,
    /// Ended without being asked, after `Ready`.
    Crashed,
    /// A start problem; `with_tail` puts the stderr tail into the detail.
    StartFailed {
        error: DriverError,
        with_tail: bool,
    },
    /// The process ended before `Ready`.
    StartExited,
}

/// What a request to the agent is for.
enum Purpose {
    Initialize,
    Open(OpenKind),
    Prompt,
    SetOption(SetOptionOp),
    /// An autonomy candidate's mode change; `in_effect` when its policy already applies.
    Autonomy {
        op: AutonomyOp,
        candidate: AutonomyCandidate,
        mode: String,
        in_effect: bool,
    },
}

struct Request {
    purpose: Purpose,
    /// `None`: no deadline (a prompt), or one kept elsewhere (the open).
    deadline: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OpenKind {
    /// `fresh`: a resume was asked for and is not possible, for this reason.
    New {
        fresh: Option<String>,
    },
    Resume(String),
    Load(String),
}

/// The open's deadline: no reply and no update for `open_idle`, or `open_ceiling` in all.
struct OpenState {
    request: i64,
    started: Instant,
    last_activity: Instant,
}

impl OpenState {
    fn deadline(&self, setup: &SessionSetup) -> Instant {
        let idle = self.last_activity + setup.timings.open_idle;
        let ceiling = self.started + setup.timings.open_ceiling;
        idle.min(ceiling)
    }
}

/// Why a request got no usable reply.
#[derive(Debug)]
enum Failure {
    Rpc(RpcError),
    Timeout,
    /// The reply was over the line cap; the sentence says so.
    Dropped(String),
}

struct PromptState {
    request: i64,
    turn_id: String,
    cancelling: bool,
}

/// One change to options or autonomy; they run one at a time, in order. On a dialect that changes
/// mode only while idle, a mode change that comes up during a turn is parked until the turn ends,
/// then runs before anything queued behind it; a later mode change replaces a parked one.
enum Op {
    SetOption(SetOptionOp),
    Autonomy(AutonomyOp),
    /// The start's last step: `Ready`.
    Ready,
}

struct SetOptionOp {
    option_id: String,
    value: ConfigValue,
    reply: Option<oneshot::Sender<Result<(), DriverError>>>,
    origin: Origin,
    /// A `SetOption` is answered within one request timeout of its arrival, however long it
    /// waited behind earlier changes, so the answer comes inside the session's IPC budget.
    deadline: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// `SetOption` from the session.
    User,
    /// The user's earlier choice, re-applied at the start.
    Desired,
    /// A mode change that waited for the turn to end.
    Deferred,
}

/// One change of autonomy (spec 5.5.2). A `SetAutonomy` takes effect at once for the policy: each
/// candidate's policy applies as soon as the candidate is chosen, the first in the command's reply
/// and every fallback after it as `Autonomy`, while its mode change follows in order. The start
/// applies the candidate whose mode change worked, and announces it once.
struct AutonomyOp {
    requested: Autonomy,
    candidates: VecDeque<AutonomyCandidate>,
    /// The candidate whose policy is in effect while its mode change is on its way.
    in_effect: Option<AutonomyCandidate>,
    /// Why the last candidate did not apply, for the last resort's note.
    reason: Option<String>,
    reply: Option<oneshot::Sender<Result<AutonomyState, DriverError>>>,
    /// A policy applies when its candidate is chosen (`SetAutonomy`), not when its mode lands.
    at_once: bool,
    /// The count of `SetAutonomy` commands this op belongs to: a later one replaces it.
    generation: u64,
    /// How often its current mode change was sent again after a retryable refusal.
    retries: u32,
}

impl AutonomyOp {
    fn new(
        requested: Autonomy,
        dialect: &dyn AcpDialect,
        reply: Option<oneshot::Sender<Result<AutonomyState, DriverError>>>,
        at_once: bool,
        generation: u64,
    ) -> AutonomyOp {
        AutonomyOp {
            requested,
            candidates: dialect.autonomy_candidates(requested).into(),
            in_effect: None,
            reason: None,
            reply,
            at_once,
            generation,
            retries: 0,
        }
    }
}

/// The autonomy candidate in effect.
struct Applied {
    requested: Autonomy,
    candidate: AutonomyCandidate,
    /// What a mode picked by hand is measured against: the candidate's mode, or for a candidate
    /// that changes none, the agent's mode when it applied (`None` while the agent has no mode).
    reference: Option<String>,
}

impl Applied {
    fn state(&self, current_mode: Option<&str>) -> AutonomyState {
        AutonomyState {
            requested: self.requested,
            native_mode: self
                .candidate
                .mode
                .clone()
                .or_else(|| current_mode.map(str::to_owned)),
            auto_answer: self.candidate.auto_answer.clone(),
            note: self.candidate.note.clone(),
        }
    }
}

/// What a load or resume replayed, for dialects whose turns outlive the process.
struct ReplayCollector {
    user_turns: u32,
    in_user_run: bool,
    reply: String,
    hasher: Sha256,
    /// Agent text other than whitespace arrived after the last user run. Without it there is no
    /// reply and no hash, as the session's own reply hash has none for a turn without agent text.
    has_reply: bool,
}

impl ReplayCollector {
    fn new() -> ReplayCollector {
        ReplayCollector {
            user_turns: 0,
            in_user_run: false,
            reply: String::new(),
            hasher: Sha256::new(),
            has_reply: false,
        }
    }

    /// Counts runs of user message chunks and keeps the agent text after the last of them.
    fn observe(&mut self, update: &Update) {
        match update {
            Update::UserMessageChunk(_) => {
                if !self.in_user_run {
                    self.user_turns = self.user_turns.saturating_add(1);
                    self.in_user_run = true;
                    self.reply.clear();
                    self.hasher = Sha256::new();
                    self.has_reply = false;
                }
            }
            Update::AgentMessageChunk(chunk) => {
                self.in_user_run = false;
                let room = REPLAY_REPLY_BYTES.saturating_sub(self.reply.len());
                self.reply.push_str(prefix_within(&chunk.text, room));
                let mut buffer = [0; 4];
                for character in chunk.text.chars().filter(|c| !c.is_whitespace()) {
                    self.hasher
                        .update(character.encode_utf8(&mut buffer).as_bytes());
                    self.has_reply = true;
                }
            }
            _ => self.in_user_run = false,
        }
    }

    fn finish(self) -> ReplaySummary {
        ReplaySummary {
            user_turns: self.user_turns,
            last_reply: self.has_reply.then_some(self.reply),
            last_reply_sha256: self.has_reply.then(|| map::hex(&self.hasher.finalize())),
        }
    }
}

/// The longest prefix of `text` within `max` bytes, on a character boundary.
fn prefix_within(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// One agent process and its ACP session.
struct Session {
    commands: mpsc::UnboundedReceiver<DriverCommand>,
    commands_open: bool,
    outbox: Outbox,
    setup: SessionSetup,
    dialect: Box<dyn AcpDialect>,
    spawn_id: String,
    rpc: RpcOut,
    stdout: crate::incoming_images::ImageLineSource,
    stdout_done: bool,
    control: ProcessControl,
    exit_wait: BoxFuture<'static, ExitInfo>,
    exit: Option<ExitInfo>,
    exited: bool,
    log: ProtocolLog,
    chain: Option<BoxFuture<'static, ShutdownReport>>,
    close_plan: Option<ShutdownPlan>,
    phase: Phase,
    end: Option<End>,
    ready: bool,
    pending: BTreeMap<i64, Request>,
    init: InitializeReply,
    open: Option<OpenState>,
    session_id: Option<String>,
    resumed: ResumeOutcome,
    /// Updates for a session not named yet (`session/new` pending), and their JSON bytes.
    early: Vec<Value>,
    early_bytes: usize,
    replay: Option<ReplayCollector>,
    replay_summary: Option<ReplaySummary>,
    options: Option<OptionModel>,
    autonomy: Option<Applied>,
    /// An autonomy change is still on its way: its own mode change decides the mode, so a mode
    /// reported or applied meanwhile (an echo, an earlier pick) is not a hand pick.
    autonomy_settling: bool,
    /// How many `SetAutonomy` commands arrived; an op of an older count was replaced.
    autonomy_generation: u64,
    hand_picked: Option<String>,
    ops: VecDeque<Op>,
    op_busy: bool,
    /// An autonomy op whose refused mode change goes out again at this time; later ops wait.
    mode_retry: Option<(Instant, AutonomyOp)>,
    /// A prompt that arrived while mode or option changes were on their way: it goes out once
    /// they land, so it never runs under the agent's previous mode.
    waiting_prompt: Option<(String, Vec<PromptPart>)>,
    /// Mode changes parked until the turn ends (dialects that change mode only while idle).
    after_turn: Vec<Op>,
    /// Commands that arrived before `Ready`, with when they arrived.
    held: VecDeque<(Instant, DriverCommand)>,
    /// Transcript events of a start that ended without `Ready` (rule 1 forbids handing them over).
    dropped_before_ready: usize,
    mapper: Mapper,
    cursor_updates: vendor_updates::CursorUpdates,
    prompt: Option<PromptState>,
    permissions: PendingPermissions,
    ui_requests: Vec<PendingUi>,
    ui_counter: u64,
    /// The previous cumulative usage from this process (session-scope dialects).
    usage_total: Option<WireUsage>,
    commands_list: Option<Vec<SlashCommand>>,
    commands_deadline: Option<Instant>,
    unknown_tags: HashSet<String>,
    /// stdout's grace after the exit (spec 5.3.3) is a budget, spent only while stdout is awaited
    /// with room in the outbox: a slow session never eats it, and a descendant that keeps writing
    /// to the pipe cannot stretch it.
    eof_grace_used: Duration,
    /// Since when the grace has been running, while it runs.
    eof_grace_since: Option<Instant>,
    /// stdout ended with the process alive: when to kill it (and then, when to give up).
    exit_wait_deadline: Option<Instant>,
    killed: bool,
    /// Closing: how long the running turn may still answer its cancel.
    cancel_deadline: Option<Instant>,
}

struct PendingUi {
    request_id: String,
    rpc_id: Value,
    turn: Option<String>,
    call: callbacks::UiCall,
}

impl Session {
    fn on_ui_request(&mut self, id: Value, params: &Value, call: callbacks::UiCall) {
        let foreign = self.session_id.is_some()
            && params
                .get("sessionId")
                .is_some_and(|session| session.as_str() != self.session_id.as_deref());
        if foreign
            || self.phase != Phase::Running
            || (call.context.is_some() && !self.ready)
            || self.prompt.as_ref().is_some_and(|prompt| prompt.cancelling)
        {
            let _ = self.rpc.respond(&id, call.cancelled());
            return;
        }
        if self.ui_requests.len() >= 32
            || self.ui_requests.iter().any(|pending| pending.rpc_id == id)
        {
            let _ = self.rpc.respond_error(
                &id,
                codes::INVALID_PARAMS,
                "Too many or duplicate pending user input requests.",
            );
            return;
        }
        self.ui_counter += 1;
        let request_id = format!("{}.q{}", self.spawn_id, self.ui_counter);
        let turn = self.mapper.turn().map(str::to_owned);
        if let Some(context) = &call.context {
            let message_id = format!("{request_id}.plan");
            self.outbox.push(DriverEvent::Event {
                turn: turn.clone(),
                event: GhostEvent::MessageDelta {
                    message_id: message_id.clone(),
                    text: context.clone(),
                },
            });
            self.outbox.push(DriverEvent::Event {
                turn: turn.clone(),
                event: GhostEvent::MessageCompleted { message_id },
            });
        }
        self.outbox.push(DriverEvent::UiRequest {
            turn: turn.clone(),
            request: UiRequest {
                request_id: request_id.clone(),
                prompt: call.prompt.clone(),
            },
        });
        self.ui_requests.push(PendingUi {
            request_id,
            rpc_id: id,
            turn,
            call,
        });
    }

    fn on_ui_answer(&mut self, request_id: &str, answer: Option<&UiAnswer>) {
        let Some(index) = self
            .ui_requests
            .iter()
            .position(|pending| pending.request_id == request_id)
        else {
            return;
        };
        let pending = &self.ui_requests[index];
        let result = match answer {
            Some(answer) => pending.call.answer(answer),
            None => Some(pending.call.cancelled()),
        };
        if let Some(result) = result {
            let pending = self.ui_requests.remove(index);
            let _ = self.rpc.respond(&pending.rpc_id, result);
        }
    }

    fn cancel_ui(&mut self, turn: Option<&str>) {
        let (cancelled, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.ui_requests)
            .into_iter()
            .partition(|pending| turn.is_none() || pending.turn.as_deref() == turn);
        self.ui_requests = keep;
        for pending in cancelled {
            let _ = self.rpc.respond(&pending.rpc_id, pending.call.cancelled());
            self.outbox.push(DriverEvent::UiWithdrawn {
                request_id: pending.request_id,
            });
        }
    }

    fn new(
        process: SpawnedProcess,
        commands: mpsc::UnboundedReceiver<DriverCommand>,
        outbox: Outbox,
        held: VecDeque<(Instant, DriverCommand)>,
        setup: SessionSetup,
    ) -> Session {
        let SpawnedProcess {
            spawn_id,
            io,
            control,
            log,
        } = process;
        let dialect = dialect_for(&setup.dialect);
        let exit_wait = control.exited();
        Session {
            commands,
            commands_open: true,
            outbox,
            dialect,
            mapper: Mapper::new(spawn_id.clone()),
            cursor_updates: vendor_updates::CursorUpdates::default(),
            spawn_id,
            rpc: RpcOut::new(io.stdin, true),
            stdout: crate::incoming_images::ImageLineSource::new(
                io.stdout,
                setup.attachment_writer.clone(),
            ),
            stdout_done: false,
            control,
            exit_wait,
            exit: None,
            exited: false,
            log,
            chain: None,
            close_plan: None,
            phase: Phase::Running,
            end: None,
            ready: false,
            pending: BTreeMap::new(),
            init: InitializeReply::default(),
            open: None,
            session_id: None,
            resumed: ResumeOutcome::New,
            early: Vec::new(),
            early_bytes: 0,
            replay: None,
            replay_summary: None,
            options: None,
            autonomy: None,
            autonomy_settling: false,
            autonomy_generation: 0,
            hand_picked: None,
            ops: VecDeque::new(),
            op_busy: false,
            mode_retry: None,
            waiting_prompt: None,
            after_turn: Vec::new(),
            held,
            dropped_before_ready: 0,
            prompt: None,
            permissions: PendingPermissions::default(),
            ui_requests: Vec::new(),
            ui_counter: 0,
            usage_total: None,
            commands_list: None,
            commands_deadline: None,
            unknown_tags: HashSet::new(),
            eof_grace_used: Duration::ZERO,
            eof_grace_since: None,
            exit_wait_deadline: None,
            killed: false,
            cancel_deadline: None,
            setup,
        }
    }

    async fn run(mut self) -> DriverExit {
        self.outbox
            .push(DriverEvent::Status(DriverStatus::Initializing));
        let mut params = requests::initialize(&self.setup.client_version);
        if self.dialect.form_elicitation() {
            params["clientCapabilities"]["elicitation"] = serde_json::json!({"form": {}});
        }
        if let Ok(id) = self.rpc.request(methods::INITIALIZE, params) {
            let deadline = Instant::now() + self.setup.launch.startup_timeout;
            self.pending.insert(
                id,
                Request {
                    purpose: Purpose::Initialize,
                    deadline: Some(deadline),
                },
            );
        }
        loop {
            if self.finished() {
                return self.finish();
            }
            self.tick_eof_grace();
            let deadline = self.next_deadline();
            let head = self.outbox.head_bytes();
            tokio::select! {
                biased;
                command = self.commands.recv(), if self.phase == Phase::Running && self.commands_open => {
                    self.on_command(command);
                }
                permit = self.outbox.sender.reserve(head), if self.outbox.has_head() => {
                    self.outbox.deliver(permit);
                }
                item = self.stdout.next(), if !self.stdout_done && !self.outbox.is_full() => {
                    self.on_stdout(item);
                }
                exit = &mut self.exit_wait, if !self.exited => self.on_exit(exit),
                report = poll_chain(&mut self.chain), if self.chain.is_some() => self.on_chain(report),
                () = sleep_until(deadline), if deadline.is_some() => self.on_deadline(),
            }
        }
    }

    /// stdout ended (or got its grace), the process exited (or the chain ended), the chain is not
    /// running, and every event was handed over.
    fn finished(&self) -> bool {
        self.phase == Phase::Draining
            && self.stdout_done
            && self.exited
            && self.chain.is_none()
            && !self.outbox.has_head()
            && self.commands_list.is_none()
    }

    fn finish(self) -> DriverExit {
        if self.dropped_before_ready > 0 {
            tracing::debug!(
                count = self.dropped_before_ready,
                "transcript events of a start that never reached Ready were dropped"
            );
        }
        match self.end {
            Some(End::Closed) => DriverExit::Closed,
            Some(End::StartFailed {
                ref error,
                with_tail,
            }) => DriverExit::StartFailed(self.start_error(error.clone(), with_tail)),
            Some(End::StartExited) => {
                let error = DriverError {
                    kind: ErrorKind::AgentExited,
                    message: exited_message(self.exit),
                    detail: None,
                };
                DriverExit::StartFailed(self.start_error(error, true))
            }
            Some(End::Crashed) | None => DriverExit::Crashed {
                exit: self.exit,
                stderr_tail: self.control.stderr_tail(STDERR_TAIL_BYTES),
            },
        }
    }

    /// A start failure with the dialect's hint when the stderr tail matches, and the tail itself
    /// as the detail when asked for (raw: the session redacts it).
    fn start_error(&self, mut error: DriverError, with_tail: bool) -> DriverError {
        let tail = self.control.stderr_tail(STDERR_TAIL_BYTES);
        if let Some(hint) = self.dialect.start_failure_hint(&tail) {
            error.message = format!("{} {hint}", error.message);
        }
        if with_tail && error.detail.is_none() && !tail.trim().is_empty() {
            error.detail = Some(tail);
        }
        error
    }

    /// Runs stdout's grace while the process has exited, stdout is open and the outbox has room,
    /// and banks what it spent whenever one of those stops being true. Called at every loop turn.
    fn tick_eof_grace(&mut self) {
        let runs = self.exited && !self.stdout_done && !self.outbox.is_full();
        match (runs, self.eof_grace_since) {
            (true, None) => self.eof_grace_since = Some(Instant::now()),
            (false, Some(since)) => {
                self.eof_grace_used = self.eof_grace_used.saturating_add(since.elapsed());
                self.eof_grace_since = None;
            }
            _ => {}
        }
    }

    /// When stdout's grace runs out, while it runs.
    fn eof_grace_deadline(&self) -> Option<Instant> {
        self.eof_grace_since
            .map(|since| since + PIPE_EOF_GRACE.saturating_sub(self.eof_grace_used))
    }

    fn next_deadline(&self) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        let mut consider = |at: Option<Instant>| {
            if let Some(at) = at {
                next = Some(next.map_or(at, |next| next.min(at)));
            }
        };
        for request in self.pending.values() {
            consider(request.deadline);
        }
        consider(self.open.as_ref().map(|open| open.deadline(&self.setup)));
        consider(self.commands_deadline);
        consider(self.cancel_deadline);
        consider(self.eof_grace_deadline());
        consider(self.exit_wait_deadline);
        consider(self.mode_retry.as_ref().map(|(at, _)| *at));
        consider(self.mapper.terminal_due());
        next
    }

    // ---- inputs ----------------------------------------------------------------------------

    fn on_command(&mut self, command: Option<DriverCommand>) {
        let Some(command) = command else {
            // Contract rule 4: a closed command channel is a Shutdown with zero cancel grace.
            self.commands_open = false;
            self.begin_close(
                End::Closed,
                StopPlan {
                    cancel_grace: Duration::ZERO,
                    process: ShutdownPlan::FORCE,
                },
            );
            return;
        };
        log_command(&self.log, &command);
        match command {
            DriverCommand::McpAction { reply, .. } => crate::mcp::unsupported(
                reply,
                "ACP does not advertise MCP status or OAuth management. Use this agent's native customization route; restart an idle chat to reconnect its configured MCP servers.",
            ),
            DriverCommand::Shutdown { plan } => self.begin_close(End::Closed, plan),
            DriverCommand::Prompt { turn, parts } => self.on_prompt(turn.turn_id, &parts),
            DriverCommand::Interrupt => {
                if let Some((turn_id, _)) = self.waiting_prompt.take() {
                    // It never reached the agent.
                    self.outbox.push(DriverEvent::TurnEnded {
                        turn_id,
                        stop_reason: StopReason::Cancelled,
                        usage: None,
                        error: None,
                    });
                } else if self
                    .prompt
                    .as_ref()
                    .is_some_and(|prompt| !prompt.cancelling)
                {
                    self.cancel_prompt();
                }
                self.cancel_ui(None);
            }
            // No ACP agent takes an edited plan (`plan_edits` is off), so none is ever sent.
            DriverCommand::EditPlan { .. } => {}
            DriverCommand::RespondPermission {
                request_id,
                outcome,
            } => self.on_respond_permission(&request_id, &outcome),
            DriverCommand::RespondUi { request_id, answer } => {
                self.on_ui_answer(&request_id, answer.as_ref())
            }
            DriverCommand::Steer { reply, .. } => {
                let _ = reply.send(crate::driver::SteerResult::Rejected {
                    message: "This agent cannot steer an exact running turn.".into(),
                });
            }
            DriverCommand::RewindFiles { reply, .. } => {
                let _ = reply.send(Err(DriverError {
                    kind: ErrorKind::ProtocolError,
                    message: "This agent does not support native file rewinding.".into(),
                    detail: None,
                }));
            }
            command @ (DriverCommand::SetOption { .. } | DriverCommand::SetAutonomy { .. }) => {
                if self.ready {
                    self.queue_op(command, Instant::now());
                } else {
                    self.held.push_back((Instant::now(), command));
                }
            }
        }
    }

    fn on_stdout(&mut self, item: Option<StdoutItem>) {
        match item {
            None => {
                self.stdout_done = true;
                // A running shutdown chain owns the process's end (codex-acp needs more than 2 s
                // after EOF); otherwise the agent gets 2 s to exit before its tree is killed.
                if !self.exited && self.chain.is_none() && self.exit_wait_deadline.is_none() {
                    self.exit_wait_deadline = Some(Instant::now() + EXIT_AFTER_EOF_GRACE);
                }
                self.enter_draining();
            }
            Some(StdoutItem::Line(line)) => {
                for incoming in parse_line(&line) {
                    self.on_incoming(incoming);
                }
            }
            Some(StdoutItem::Oversized { bytes, keys }) => self.on_oversized(bytes, keys),
        }
    }

    fn on_exit(&mut self, exit: ExitInfo) {
        self.exited = true;
        self.exit = Some(exit);
        self.exit_wait_deadline = None;
        self.enter_draining();
    }

    fn on_chain(&mut self, report: ShutdownReport) {
        self.chain = None;
        tracing::debug!(
            spawn_id = %self.spawn_id,
            steps = ?report.steps,
            took_ms = u64::try_from(report.took.as_millis()).unwrap_or(u64::MAX),
            "the agent's process was shut down"
        );
        if !self.exited {
            self.exited = true;
            if self.exit.is_none() {
                self.exit = report.exit;
            }
            self.exit_wait_deadline = None;
        }
    }

    fn on_deadline(&mut self) {
        let now = Instant::now();
        // A running command's held output, due to show.
        for event in self.mapper.flush_terminals(now) {
            self.push(event);
        }
        let due: Vec<i64> = self
            .pending
            .iter()
            .filter(|(_, request)| request.deadline.is_some_and(|at| at <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            if let Some(request) = self.pending.remove(&id) {
                self.on_reply(id, request.purpose, Err(Failure::Timeout));
            }
        }
        if let Some(open) = &self.open
            && open.deadline(&self.setup) <= now
        {
            let request = open.request;
            self.open = None;
            self.pending.remove(&request);
            self.replay = None;
            self.fail_start(
                DriverError {
                    kind: ErrorKind::StartTimeout,
                    message: "The agent did not finish opening the session in time.".into(),
                    detail: None,
                },
                true,
                ShutdownPlan::FORCE,
            );
        }
        if self.commands_deadline.is_some_and(|at| at <= now) {
            self.flush_commands();
        }
        if self.mode_retry.as_ref().is_some_and(|(at, _)| *at <= now)
            && let Some((_, op)) = self.mode_retry.take()
        {
            self.op_busy = false;
            if self.phase == Phase::Running {
                self.step_autonomy(op);
                self.pump_ops();
            }
        }
        if self.cancel_deadline.is_some_and(|at| at <= now) {
            self.close_process();
        }
        if self.eof_grace_deadline().is_some_and(|at| at <= now) {
            // A descendant may hold the pipe open; what came within the grace is all there is.
            tracing::debug!(spawn_id = %self.spawn_id, "stdout stayed open after the exit");
            self.stdout_done = true;
            self.enter_draining();
        }
        if let Some(at) = self.exit_wait_deadline
            && at <= now
        {
            if self.killed {
                // The kill was not confirmed; nothing more can be done from here.
                self.exited = true;
                self.exit_wait_deadline = None;
            } else {
                tracing::debug!(spawn_id = %self.spawn_id, "stdout ended but the agent lives: killing it");
                self.killed = true;
                self.control.kill_tree();
                self.exit_wait_deadline = Some(now + EXIT_AFTER_EOF_GRACE);
            }
        }
    }

    fn on_incoming(&mut self, incoming: Incoming) {
        match incoming {
            Incoming::Request { id, method, params } => self.on_request(id, &method, &params),
            Incoming::Notification { method, params } => self.on_notification(&method, &params),
            Incoming::Response { id, result } => {
                let Some(request_id) = id.as_i64() else {
                    tracing::debug!("a response with an id Ghost never sent");
                    return;
                };
                match self.pending.remove(&request_id) {
                    Some(request) => {
                        self.on_reply(request_id, request.purpose, result.map_err(Failure::Rpc));
                    }
                    None => tracing::debug!(request_id, "a response for no pending request"),
                }
            }
            Incoming::Invalid { reason } => {
                tracing::debug!(spawn_id = %self.spawn_id, reason, "an invalid line from the agent");
            }
        }
    }

    fn on_request(&mut self, id: Value, method: &str, params: &Value) {
        if method == methods::SESSION_REQUEST_PERMISSION {
            self.on_permission_request(id, params);
            return;
        }
        let not_found = format!("Method not found: {method}");
        if not_advertised(method)
            && !(method == "elicitation/create" && self.dialect.form_elicitation())
        {
            let _ = self
                .rpc
                .respond_error(&id, codes::METHOD_NOT_FOUND, &not_found);
            return;
        }
        let _ = match self.dialect.on_request(method, params) {
            DialectReply::Respond(result) => self.rpc.respond(&id, result),
            DialectReply::Error { code, message } => self.rpc.respond_error(&id, code, &message),
            DialectReply::Deferred(call) => {
                self.on_ui_request(id, params, *call);
                return;
            }
            DialectReply::NotMine => {
                self.rpc
                    .respond_error(&id, codes::METHOD_NOT_FOUND, &not_found)
            }
        };
    }

    fn on_notification(&mut self, method: &str, params: &Value) {
        if self.dialect.id() == "cursor" && method.starts_with("cursor/") {
            if !self.ready
                || self.loading()
                || params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| Some(id) != self.session_id.as_deref())
            {
                return;
            }
            if let Some(update) = self.cursor_updates.notification(method, params) {
                self.apply_update(update, Instant::now());
                return;
            }
        }
        match method {
            methods::SESSION_UPDATE => self.on_update(params),
            methods::CANCEL_REQUEST => self.on_cancel_request(params),
            // Account data: never an event, never stored.
            methods::AUTH_STATUS_UPDATE => {}
            _ => {
                let events = self.dialect.on_notification(method, params);
                if events.is_empty() {
                    tracing::debug!(method_bytes = method.len(), "a notification Ghost ignores");
                }
                for event in events {
                    self.push(event);
                }
            }
        }
    }

    fn on_oversized(&mut self, bytes: u64, keys: TopKeys) {
        let size = bytes.div_ceil(1024 * 1024);
        match (keys.id, keys.method) {
            (Some(id), Some(_)) => {
                let message =
                    format!("Ghost can't accept a request of {size} MB; the limit is 64 MiB.");
                let _ = self.rpc.respond_error(&id, codes::INTERNAL_ERROR, &message);
            }
            (Some(id), None) => {
                let message =
                    format!("Ghost dropped a response of {size} MB; the limit is 64 MiB.");
                match id
                    .as_i64()
                    .and_then(|request_id| Some((request_id, self.pending.remove(&request_id)?)))
                {
                    Some((request_id, request)) => {
                        self.on_reply(request_id, request.purpose, Err(Failure::Dropped(message)));
                    }
                    None => tracing::debug!("an oversized response for no pending request"),
                }
            }
            (None, _) => {
                if let Some(open) = &mut self.open {
                    open.last_activity = Instant::now();
                }
                let event = DriverEvent::Event {
                    turn: self.mapper.turn().map(str::to_owned),
                    event: GhostEvent::Notice {
                        level: NoticeLevel::Warning,
                        message: OVERSIZED_UPDATE_NOTICE.into(),
                    },
                };
                self.push(event);
            }
        }
    }

    fn on_reply(&mut self, request_id: i64, purpose: Purpose, result: Result<Value, Failure>) {
        match purpose {
            Purpose::Initialize => self.on_initialized(result),
            Purpose::Open(kind) => self.on_opened(kind, result),
            Purpose::Prompt => self.on_prompt_reply(request_id, result),
            Purpose::SetOption(op) => {
                self.op_busy = false;
                self.finish_set_option(op, result);
                self.pump_ops();
            }
            Purpose::Autonomy {
                op,
                candidate,
                mode,
                in_effect,
            } => {
                self.op_busy = false;
                self.finish_autonomy(op, candidate, &mode, in_effect, result);
                self.pump_ops();
            }
        }
    }

    // ---- startup ---------------------------------------------------------------------------

    fn on_initialized(&mut self, result: Result<Value, Failure>) {
        match result {
            Ok(value) => {
                let reply = InitializeReply::parse(&value);
                if !reply.speaks_version_1() {
                    let message = match reply.protocol_version {
                        Some(_) => format!(
                            "This agent speaks ACP protocol version {}; Ghost supports version 1.",
                            reply.version_text()
                        ),
                        None => "This agent did not say which ACP protocol version it speaks; Ghost supports version 1.".into(),
                    };
                    let error = DriverError {
                        kind: ErrorKind::ProtocolMismatch,
                        message,
                        detail: None,
                    };
                    self.fail_start(error, false, ShutdownPlan::STANDARD);
                    return;
                }
                self.init = reply;
                self.start_open();
            }
            Err(Failure::Rpc(error)) => {
                self.fail_start(agent_error(&error), true, ShutdownPlan::STANDARD);
            }
            Err(Failure::Timeout) => {
                let seconds = self.setup.launch.startup_timeout.as_secs();
                let error = DriverError {
                    kind: ErrorKind::StartTimeout,
                    message: format!("The agent did not answer within {seconds} seconds."),
                    detail: None,
                };
                self.fail_start(error, true, ShutdownPlan::FORCE);
            }
            Err(Failure::Dropped(message)) => {
                self.fail_start(protocol_error(message), false, ShutdownPlan::STANDARD);
            }
        }
    }

    fn start_open(&mut self) {
        self.outbox.push(DriverEvent::Status(DriverStatus::Opening));
        let kind = match self.setup.resume.clone() {
            Some(id) if self.init.resume => OpenKind::Resume(id),
            Some(id) if self.init.load_session => OpenKind::Load(id),
            Some(_) => OpenKind::New {
                fresh: Some(CANNOT_CONTINUE.into()),
            },
            None => OpenKind::New { fresh: None },
        };
        if let OpenKind::New {
            fresh: Some(reason),
        } = &kind
        {
            let notice = fresh_notice(reason);
            self.push_notice(NoticeLevel::Info, notice);
        }
        self.send_open(kind);
    }

    fn send_open(&mut self, kind: OpenKind) {
        let cwd = self.setup.agent_folder.clone();
        let (method, mut params) = match &kind {
            OpenKind::New { .. } => (methods::SESSION_NEW, requests::new_session(&cwd)),
            OpenKind::Resume(id) => (methods::SESSION_RESUME, requests::resume_session(id, &cwd)),
            OpenKind::Load(id) => (methods::SESSION_LOAD, requests::load_session(id, &cwd)),
        };
        match crate::mcp::acp_session(
            &self.setup.mcp_servers,
            self.init.mcp_stdio,
            self.init.mcp_http,
            self.init.mcp_sse,
        ) {
            Ok((servers, omitted)) => {
                params["mcpServers"] = servers;
                if omitted {
                    self.push_notice(NoticeLevel::Info,"Project memory context remains available. This agent does not support Ghost's optional memory MCP tools.".into());
                }
            }
            Err(message) => {
                self.fail_start(
                    DriverError {
                        kind: ErrorKind::ProtocolError,
                        message: message.into(),
                        detail: None,
                    },
                    false,
                    ShutdownPlan::STANDARD,
                );
                return;
            }
        }
        self.replay = match kind {
            OpenKind::Resume(_) | OpenKind::Load(_)
                if self.dialect.replay_may_hold_unseen_work() =>
            {
                Some(ReplayCollector::new())
            }
            _ => None,
        };
        // A failed write means the agent is gone; its exit ends the start.
        if let Ok(id) = self.rpc.request(method, params) {
            let now = Instant::now();
            self.pending.insert(
                id,
                Request {
                    purpose: Purpose::Open(kind),
                    deadline: None,
                },
            );
            self.open = Some(OpenState {
                request: id,
                started: now,
                last_activity: now,
            });
        }
    }

    fn on_opened(&mut self, kind: OpenKind, result: Result<Value, Failure>) {
        self.open = None;
        match (kind, result) {
            (OpenKind::New { fresh }, Ok(value)) => {
                let reply = OpenReply::parse(&value);
                let Some(session_id) = reply.session_id.clone() else {
                    let error = protocol_error("The agent did not name its new session.".into());
                    self.fail_start(error, false, ShutdownPlan::STANDARD);
                    return;
                };
                self.session_id = Some(session_id);
                self.resumed = match fresh {
                    Some(reason) => ResumeOutcome::Fresh { reason },
                    None => ResumeOutcome::New,
                };
                self.replay = None;
                self.configure(&reply);
            }
            (OpenKind::Resume(id), Ok(value)) => {
                self.opened_existing(id, ResumeOutcome::Resumed, &value);
            }
            (OpenKind::Load(id), Ok(value)) => {
                self.opened_existing(id, ResumeOutcome::Loaded, &value);
            }
            (OpenKind::Resume(_) | OpenKind::Load(_), Err(Failure::Rpc(error)))
                if self.not_found_reason(&error).is_some() =>
            {
                self.replay = None;
                let reason = self.not_found_reason(&error).unwrap_or_default();
                self.push_notice(NoticeLevel::Info, fresh_notice(&reason));
                self.send_open(OpenKind::New {
                    fresh: Some(reason),
                });
            }
            (_, Err(Failure::Rpc(error))) if error.code == codes::AUTH_REQUIRED => {
                let prompt = self.auth_prompt();
                self.outbox.push(DriverEvent::AuthRequired(prompt));
                let error = DriverError {
                    kind: ErrorKind::AuthRequired,
                    message: format!("{} needs you to sign in.", self.agent_label()),
                    detail: None,
                };
                self.fail_start(error, false, ShutdownPlan::STANDARD);
            }
            (_, Err(Failure::Rpc(error))) => {
                self.fail_start(agent_error(&error), true, ShutdownPlan::STANDARD);
            }
            (_, Err(Failure::Timeout)) => {
                let error = DriverError {
                    kind: ErrorKind::StartTimeout,
                    message: "The agent did not finish opening the session in time.".into(),
                    detail: None,
                };
                self.fail_start(error, true, ShutdownPlan::FORCE);
            }
            (_, Err(Failure::Dropped(message))) => {
                self.fail_start(protocol_error(message), false, ShutdownPlan::STANDARD);
            }
        }
    }

    /// When an error answer to a resume or load means the session does not exist (spec 5.3.2):
    /// -32002, -32601 and -32602, and what the dialect reads as not found. The reason the
    /// fallback's notice gives.
    fn not_found_reason(&self, error: &RpcError) -> Option<String> {
        if matches!(
            error.code,
            codes::RESOURCE_NOT_FOUND | codes::METHOD_NOT_FOUND | codes::INVALID_PARAMS
        ) {
            return Some(if error.message.trim().is_empty() {
                format!("error {}", error.code)
            } else {
                error.message.clone()
            });
        }
        self.dialect.open_not_found(error)
    }

    /// A resume or load succeeded: the session keeps its id.
    fn opened_existing(&mut self, id: String, resumed: ResumeOutcome, value: &Value) {
        let reply = OpenReply::parse(value);
        self.session_id = Some(id);
        self.resumed = resumed;
        self.replay_summary = self.replay.take().map(ReplayCollector::finish);
        self.configure(&reply);
    }

    /// Builds the option model, applies autonomy and the user's earlier choices, then `Ready`.
    fn configure(&mut self, reply: &OpenReply) {
        if !self
            .dialect
            .autonomy_levels()
            .contains(&self.setup.autonomy)
        {
            self.fail_start(protocol_error(format!("{} cannot enforce the requested autonomy level. Choose Full auto; extension questions and plan approval still require an answer.", self.agent_label())), false, ShutdownPlan::STANDARD);
            return;
        }
        self.outbox
            .push(DriverEvent::Status(DriverStatus::Configuring));
        let mut model = OptionModel::from_open(reply);
        model.enable_native_controls(self.dialect.id());
        let restored_plan = model.current_mode() == Some("plan")
            && !matches!(self.setup.desired.get(crate::controls::WORKFLOW),Some(ConfigValue::Choice { value }) if value == "execute");
        if restored_plan
            && let Some(ConfigValue::Choice { value }) =
                self.setup.desired.get(crate::controls::ACCESS)
        {
            model.remember_execution_access(value);
        }
        for (id, value) in self.setup.desired.iter().filter(|(id, _)| {
            matches!(
                id.as_str(),
                crate::controls::ACCESS | crate::controls::WORKFLOW | "mode" | "legacy-mode"
            ) || model
                .get(id)
                .is_some_and(|option| option.category == ConfigCategory::Mode)
        }) {
            if let Err(message) = model.validate(id, value) {
                self.fail_start(
                    protocol_error(format!(
                        "The saved native control is unavailable: {message}"
                    )),
                    false,
                    ShutdownPlan::STANDARD,
                );
                return;
            }
        }
        self.outbox.push(DriverEvent::Options(model.options()));
        self.options = Some(model);
        // Updates that arrived before the session had a name.
        self.early_bytes = 0;
        for params in std::mem::take(&mut self.early) {
            self.on_update(&params);
        }
        let preserve_plan = self.current_mode().as_deref() == Some("plan")
            && !matches!(self.setup.desired.get(crate::controls::WORKFLOW), Some(ConfigValue::Choice { value }) if value == "execute");
        if !preserve_plan {
            self.ops.push_back(Op::Autonomy(AutonomyOp::new(
                self.setup.autonomy,
                self.dialect.as_ref(),
                None,
                false,
                self.autonomy_generation,
            )));
            self.autonomy_settling = true;
        } else {
            self.outbox.push(DriverEvent::Autonomy(AutonomyState {
                requested: self.setup.autonomy,
                native_mode: Some("plan".into()),
                auto_answer: AutoAnswer::Never,
                note: Some(
                    "Native Plan remains active. Implementation requires your approval.".into(),
                ),
            }));
        }
        for (option_id, value) in self.setup.desired.clone() {
            if restored_plan && option_id == crate::controls::ACCESS {
                continue;
            }
            self.ops.push_back(Op::SetOption(SetOptionOp {
                option_id,
                value,
                reply: None,
                origin: Origin::Desired,
                deadline: None,
            }));
        }
        self.ops.push_back(Op::Ready);
        self.pump_ops();
    }

    fn become_ready(&mut self) {
        self.ready = true;
        self.outbox.push(DriverEvent::Status(DriverStatus::Ready));
        let info = ReadyInfo {
            native_session_id: self.session_id.clone(),
            agent_name: self
                .init
                .agent_title
                .clone()
                .or_else(|| self.init.agent_name.clone()),
            agent_version: self.init.agent_version.clone(),
            capabilities: self.capabilities(),
            resumed: self.resumed.clone(),
            spawn_id: self.spawn_id.clone(),
            replay: self.replay_summary.take(),
        };
        self.outbox.push(DriverEvent::Ready(info));
        self.outbox.release_held();
        while let Some((arrived, command)) = self.held.pop_front() {
            match command {
                command @ (DriverCommand::SetOption { .. } | DriverCommand::SetAutonomy { .. }) => {
                    self.queue_op(command, arrived);
                }
                DriverCommand::RespondPermission {
                    request_id,
                    outcome,
                } => self.on_respond_permission(&request_id, &outcome),
                _ => {}
            }
        }
    }

    fn capabilities(&self) -> Capabilities {
        let has = |category| {
            self.options
                .as_ref()
                .is_some_and(|options| options.has_category(category))
        };
        Capabilities {
            images: self.init.image,
            steer: false,
            interrupt: true,
            resume: self.init.load_session || self.init.resume,
            fork: false,
            rewind: false,
            set_model: has(ConfigCategory::Model),
            set_effort: has(ConfigCategory::Effort),
            permissions: self.dialect.id() != "omp",
            plans: true,
            usage: true,
            limits: false,
            slash_commands: true,
            // An advertised extension is insufficient without an implemented request contract.
            add_dirs: false,
            plan_edits: false,
            autonomy_levels: self.dialect.autonomy_levels(),
        }
    }

    fn fail_start(&mut self, error: DriverError, with_tail: bool, process: ShutdownPlan) {
        self.begin_close(
            End::StartFailed { error, with_tail },
            StopPlan {
                cancel_grace: Duration::ZERO,
                process,
            },
        );
    }

    fn auth_prompt(&self) -> AuthPrompt {
        let env = &self.setup.env.vars;
        let launch = &self.setup.launch;
        let mut commands: Vec<AuthCommand> = self
            .init
            .auth_methods
            .iter()
            .filter_map(|method| self.dialect.auth_command(method, launch, env))
            .collect();
        if commands.is_empty() {
            commands = self.dialect.fallback_auth_commands(launch, env);
        }
        AuthPrompt {
            message: format!(
                "{} needs you to sign in. Run the command below in a terminal, then press Retry.",
                self.agent_label()
            ),
            commands,
        }
    }

    fn agent_label(&self) -> String {
        self.dialect
            .agent_label()
            .map(str::to_owned)
            .or_else(|| self.init.agent_title.clone())
            .or_else(|| self.init.agent_name.clone())
            .unwrap_or_else(|| self.setup.agent_id.clone())
    }

    // ---- updates ---------------------------------------------------------------------------

    fn on_update(&mut self, params: &Value) {
        let now = Instant::now();
        if let Some(open) = &mut self.open {
            open.last_activity = now;
        }
        let Some(notification) = UpdateNotification::parse(params) else {
            tracing::debug!("a session/update without an update");
            return;
        };
        let ours = match (&self.session_id, notification.session_id) {
            (Some(ours), Some(theirs)) => ours == theirs,
            (Some(_), None) => true,
            (None, theirs) => match self.open_kind() {
                Some(OpenKind::New { .. }) => {
                    // The reply names the session; until then nothing can be matched.
                    let size = json_bytes(params);
                    let fits = self.early.is_empty()
                        || (self.early.len() < EARLY_UPDATES
                            && self.early_bytes.saturating_add(size) <= EARLY_UPDATE_BYTES);
                    if fits {
                        self.early_bytes = self.early_bytes.saturating_add(size);
                        self.early.push(params.clone());
                    } else {
                        tracing::debug!(size, "an early update dropped: the buffer is full");
                    }
                    return;
                }
                Some(OpenKind::Resume(id) | OpenKind::Load(id)) => {
                    theirs.is_none_or(|theirs| theirs == id)
                }
                None => false,
            },
        };
        if !ours {
            tracing::debug!("an update for another session");
            return;
        }
        let update = match Update::parse(notification.update) {
            Ok(update) => update,
            Err(malformed) => {
                tracing::debug!(
                    tag_bytes = malformed.tag.as_ref().map(String::len),
                    reason = malformed.reason,
                    "a malformed update was dropped"
                );
                return;
            }
        };
        if self.loading() && update.is_transcript() {
            // Replay of the earlier conversation, which the chat already shows.
            if let Some(replay) = &mut self.replay {
                replay.observe(&update);
            }
            return;
        }
        self.apply_update(update, now);
    }

    fn open_kind(&self) -> Option<&OpenKind> {
        let open = self.open.as_ref()?;
        match &self.pending.get(&open.request)?.purpose {
            Purpose::Open(kind) => Some(kind),
            _ => None,
        }
    }

    fn loading(&self) -> bool {
        matches!(
            self.open_kind(),
            Some(OpenKind::Resume(_) | OpenKind::Load(_))
        )
    }

    fn apply_update(&mut self, update: Update, now: Instant) {
        if let Update::Unknown(tag) = &update
            && self.unknown_tags.insert(tag.clone())
        {
            tracing::info!(tag_bytes = tag.len(), "an update Ghost does not know");
        }
        match self.mapper.map(update, now) {
            Mapped::Events(events) => {
                for event in events {
                    self.push(event);
                }
            }
            Mapped::Commands(list) => {
                self.commands_list = Some(list);
                if self.phase == Phase::Draining {
                    self.flush_commands();
                } else {
                    self.commands_deadline = Some(now + self.setup.timings.commands_debounce);
                }
            }
            Mapped::Mode(mode) => self.on_mode_reported(&mode),
            Mapped::ConfigOptions(raw) => self.on_config_options(&raw),
            Mapped::Nothing(reason) => tracing::trace!(reason, "an update with nothing to show"),
        }
    }

    fn on_mode_reported(&mut self, mode: &str) {
        let Some(options) = &mut self.options else {
            tracing::debug!("a mode update before the session's options exist");
            return;
        };
        if options.set_current_mode(mode) {
            self.push_options();
        }
        self.check_hand_picked(mode);
    }

    fn on_config_options(&mut self, raw: &[Value]) {
        let Some(options) = &mut self.options else {
            tracing::debug!("a config option update before the session's options exist");
            return;
        };
        let before = options.current_mode().map(str::to_owned);
        options.replace_real(raw);
        let after = options.current_mode().map(str::to_owned);
        self.push_options();
        if before != after
            && let Some(mode) = after
        {
            self.check_hand_picked(&mode);
        }
    }

    fn flush_commands(&mut self) {
        self.commands_deadline = None;
        if let Some(list) = self.commands_list.take() {
            self.outbox.push(DriverEvent::Commands(list));
        }
    }

    /// Hands an event over; before `Ready` transcript events wait for it (rule 1), counting
    /// toward the outbox's bound. Once a start ended without `Ready`, nothing waits for it.
    fn push(&mut self, event: DriverEvent) {
        if self.ready || !holds_until_ready(&event) {
            self.outbox.push(event);
        } else if self.phase == Phase::Running {
            self.outbox.hold(event);
        } else {
            self.dropped_before_ready = self.dropped_before_ready.saturating_add(1);
        }
    }

    fn push_notice(&mut self, level: NoticeLevel, message: String) {
        let turn = self.mapper.turn().map(str::to_owned);
        self.push(DriverEvent::Event {
            turn,
            event: GhostEvent::Notice { level, message },
        });
    }

    fn push_options(&mut self) {
        if let Some(options) = &self.options {
            self.outbox.push(DriverEvent::Options(options.options()));
        }
    }

    // ---- turns -----------------------------------------------------------------------------

    fn on_prompt(&mut self, turn_id: String, parts: &[PromptPart]) {
        if !self.ready || self.session_id.is_none() {
            self.outbox.push(not_ready(turn_id));
            return;
        }
        if self.prompt.is_some() || self.waiting_prompt.is_some() {
            self.outbox.push(DriverEvent::TurnEnded {
                turn_id,
                stop_reason: StopReason::Error,
                usage: None,
                error: Some(internal("A turn is already running.")),
            });
            return;
        }
        let blocks = match crate::attachments::acp(
            parts,
            self.setup.attachment_dir.as_deref(),
            self.init.image,
        ) {
            Ok(blocks) => blocks,
            Err(message) => {
                self.outbox.push(DriverEvent::TurnEnded {
                    turn_id,
                    stop_reason: StopReason::Error,
                    usage: None,
                    error: Some(internal(&message)),
                });
                return;
            }
        };
        // Sent now, it would run under the agent's previous mode (and Hermes refuses a mode change
        // that races a prompt), so it waits for the changes queued before it.
        if self.ops_pending() {
            self.waiting_prompt = Some((turn_id, parts.to_vec()));
            return;
        }
        let session_id = self.session_id.clone().unwrap_or_default();
        for event in self.mapper.begin_turn(&turn_id, Instant::now()) {
            self.push(event);
        }
        self.outbox
            .push(DriverEvent::Status(DriverStatus::Prompting));
        match self.rpc.request(
            methods::SESSION_PROMPT,
            serde_json::json!({"sessionId": session_id, "prompt": blocks}),
        ) {
            Ok(request) => {
                self.pending.insert(
                    request,
                    Request {
                        purpose: Purpose::Prompt,
                        deadline: None,
                    },
                );
                self.prompt = Some(PromptState {
                    request,
                    turn_id,
                    cancelling: false,
                });
            }
            Err(_) => {
                for event in self.mapper.end_turn(Instant::now()) {
                    self.push(event);
                }
                self.outbox.push(DriverEvent::TurnEnded {
                    turn_id,
                    stop_reason: StopReason::Error,
                    usage: None,
                    error: Some(DriverError {
                        kind: ErrorKind::AgentExited,
                        message: "The agent stopped reading its input.".into(),
                        detail: None,
                    }),
                });
            }
        }
    }

    /// Spec 5.6.2: `session/cancel`, the turn's pending permission requests answered `cancelled`,
    /// later ones answered so at once, and the prompt's own reply awaited.
    fn cancel_prompt(&mut self) {
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        prompt.cancelling = true;
        let turn_id = prompt.turn_id.clone();
        self.outbox
            .push(DriverEvent::Status(DriverStatus::Cancelling));
        if let Some(session_id) = &self.session_id {
            let _ = self
                .rpc
                .notify(methods::SESSION_CANCEL, requests::cancel(session_id));
        }
        for (_, pending) in self.permissions.take_turn(&turn_id) {
            self.answer_cancelled(&pending);
        }
        self.cancel_ui(Some(&turn_id));
    }

    fn on_prompt_reply(&mut self, request: i64, result: Result<Value, Failure>) {
        let Some(prompt) = self.prompt.take_if(|prompt| prompt.request == request) else {
            return;
        };
        let (stop_reason, usage, error) = match result {
            Ok(value) => {
                let reply = PromptReply::parse(&value);
                if let Some(reason) = &reply.unknown_reason {
                    tracing::info!(
                        reason_bytes = reason.len(),
                        "a stop reason Ghost does not know"
                    );
                }
                if self.dialect.id() == "grok"
                    && value
                        .pointer("/_meta/usage/usageIsIncomplete")
                        .and_then(Value::as_bool)
                        == Some(true)
                {
                    self.push_notice(
                        NoticeLevel::Warning,
                        "Grok reports incomplete token usage for this turn.".into(),
                    );
                }
                let usage = self
                    .dialect
                    .prompt_usage(&value)
                    .map(|usage| self.turn_usage(usage));
                (reply.stop_reason, usage, None)
            }
            Err(Failure::Rpc(_)) if prompt.cancelling => (StopReason::Cancelled, None, None),
            Err(Failure::Rpc(error)) if error.code == codes::REQUEST_CANCELLED => {
                (StopReason::Cancelled, None, None)
            }
            Err(Failure::Rpc(error)) if error.code == codes::AUTH_REQUIRED => {
                let auth = self.auth_prompt();
                self.outbox.push(DriverEvent::AuthRequired(auth));
                let error = DriverError {
                    kind: ErrorKind::AuthRequired,
                    message: format!("{} needs you to sign in.", self.agent_label()),
                    detail: None,
                };
                (StopReason::Error, None, Some(error))
            }
            Err(Failure::Rpc(error)) => (StopReason::Error, None, Some(agent_error(&error))),
            Err(Failure::Dropped(_)) if prompt.cancelling => (StopReason::Cancelled, None, None),
            Err(Failure::Dropped(message)) => {
                (StopReason::Error, None, Some(protocol_error(message)))
            }
            Err(Failure::Timeout) => (
                StopReason::Error,
                None,
                Some(internal("The agent did not answer in time.")),
            ),
        };
        for event in self.mapper.end_turn(Instant::now()) {
            self.push(event);
        }
        // Rule 3: nothing of this turn stays unanswered (Hermes auto-denies only after 300 s).
        for (_, pending) in self.permissions.take_turn(&prompt.turn_id) {
            self.answer_cancelled(&pending);
        }
        self.cancel_ui(Some(&prompt.turn_id));
        self.outbox.push(DriverEvent::TurnEnded {
            turn_id: prompt.turn_id,
            stop_reason,
            usage,
            error,
        });
        match self.phase {
            Phase::Running => {
                self.outbox.push(DriverEvent::Status(DriverStatus::Ready));
                // Parked changes came up before anything still queued, so they go first.
                for op in self.after_turn.drain(..).rev() {
                    self.ops.push_front(op);
                }
                self.pump_ops();
            }
            Phase::Closing if self.cancel_deadline.is_some() => self.close_process(),
            Phase::Closing | Phase::Draining => {}
        }
    }

    /// Spec 5.5.4: turn-scope dialects report as is; session-scope ones report the difference
    /// from the previous cumulative value of this process, or the total the first time.
    fn turn_usage(&mut self, usage: WireUsage) -> TokenUsage {
        let as_is = |usage: &WireUsage, scope| TokenUsage {
            input_tokens: usage.input,
            output_tokens: usage.output,
            cache_read_tokens: usage.cached_read,
            cache_write_tokens: usage.cached_write,
            reasoning_tokens: usage.thought,
            scope,
        };
        match self.dialect.prompt_usage_scope() {
            UsageScope::Turn => as_is(&usage, UsageScope::Turn),
            UsageScope::Session => {
                let reported = match self.usage_total {
                    Some(before) => TokenUsage {
                        input_tokens: usage.input.saturating_sub(before.input),
                        output_tokens: usage.output.saturating_sub(before.output),
                        cache_read_tokens: difference(usage.cached_read, before.cached_read),
                        cache_write_tokens: difference(usage.cached_write, before.cached_write),
                        reasoning_tokens: difference(usage.thought, before.thought),
                        scope: UsageScope::Turn,
                    },
                    None => as_is(&usage, UsageScope::Session),
                };
                self.usage_total = Some(usage);
                reported
            }
        }
    }

    // ---- permissions -----------------------------------------------------------------------

    fn on_permission_request(&mut self, id: Value, params: &Value) {
        let parsed = match PermissionParams::parse(params) {
            Ok(parsed) => parsed,
            Err(reason) => {
                let message = format!("Ghost could not read this permission request: {reason}.");
                let _ = self.rpc.respond_error(&id, codes::INVALID_PARAMS, &message);
                return;
            }
        };
        if let (Some(ours), Some(theirs)) = (&self.session_id, &parsed.session_id)
            && ours != theirs
        {
            tracing::debug!("a permission request for another session, answered cancelled");
            self.answer_cancelled_id(&id);
            return;
        }
        let cancelling = self.prompt.as_ref().is_some_and(|prompt| prompt.cancelling);
        if cancelling || self.phase != Phase::Running {
            tracing::debug!(
                "a permission request while the turn or the session ends, answered cancelled"
            );
            self.answer_cancelled_id(&id);
            return;
        }
        let turn = self.mapper.turn().map(str::to_owned);
        let request_id = self.mapper.mint_permission();
        let tool = parsed.tool;
        let shown = self.mapper.permission_tool(&tool.tool_call_id);
        let title = tool
            .title
            .filter(|title| !title.is_empty())
            .or_else(|| shown.as_ref().map(|(_, _, title)| title.clone()))
            .or_else(|| tool.name.clone().filter(|name| !name.is_empty()))
            .unwrap_or_else(|| UNTITLED_PERMISSION.to_owned());
        let request = PermissionRequest {
            request_id: request_id.clone(),
            kind: tool
                .kind
                .or_else(|| shown.as_ref().map(|(_, kind, _)| *kind)),
            tool_call_id: shown.map(|(minted, _, _)| minted),
            title,
            raw_input: tool.raw_input,
            content: tool.content.unwrap_or_default(),
            locations: tool.locations.unwrap_or_default(),
            options: parsed.options,
            truncated: None,
        };
        self.permissions.insert(
            request_id,
            Pending {
                rpc_id: id,
                turn: turn.clone(),
                option_ids: request
                    .options
                    .iter()
                    .map(|option| option.option_id.clone())
                    .collect(),
            },
        );
        // Requests before Ready go out at once: Hermes asks while `session/set_model` waits.
        self.outbox.push(DriverEvent::Permission { turn, request });
    }

    fn on_respond_permission(&mut self, request_id: &str, outcome: &PermissionOutcome) {
        match self.permissions.take(request_id) {
            Some(pending) => {
                let _ = self
                    .rpc
                    .respond(&pending.rpc_id, requests::permission_reply(outcome));
            }
            None => tracing::debug!(request_id, "an answer for a request no longer pending"),
        }
    }

    /// `$/cancel_request` naming a pending request: answered `cancelled` anyway (the receiver must
    /// still answer), forgotten, and withdrawn.
    fn on_cancel_request(&mut self, params: &Value) {
        let Some(rpc_id) = params.get("requestId") else {
            return;
        };
        if let Some((request_id, pending)) = self.permissions.take_rpc(rpc_id) {
            self.answer_cancelled(&pending);
            self.push(DriverEvent::PermissionWithdrawn { request_id });
        }
        if let Some(index) = self
            .ui_requests
            .iter()
            .position(|pending| pending.rpc_id == *rpc_id)
        {
            let pending = self.ui_requests.remove(index);
            let _ = self.rpc.respond(&pending.rpc_id, pending.call.cancelled());
            self.outbox.push(DriverEvent::UiWithdrawn {
                request_id: pending.request_id,
            });
        }
    }

    fn answer_cancelled(&mut self, pending: &Pending) {
        self.answer_cancelled_id(&pending.rpc_id);
    }

    fn answer_cancelled_id(&mut self, id: &Value) {
        let _ = self.rpc.respond(
            id,
            requests::permission_reply(&PermissionOutcome::Cancelled),
        );
    }

    // ---- options and autonomy --------------------------------------------------------------

    /// Queues a change. A `SetAutonomy` puts its policy in effect at once (spec 5.5.2: "takes
    /// effect at once for the policy") and queues only its mode change; a `SetOption` is answered
    /// within one request timeout of `arrived`.
    fn queue_op(&mut self, command: DriverCommand, arrived: Instant) {
        match command {
            DriverCommand::SetOption {
                option_id,
                value,
                reply,
            } => self.ops.push_back(Op::SetOption(SetOptionOp {
                option_id,
                value,
                reply: Some(reply),
                origin: Origin::User,
                deadline: Some(arrived + self.setup.timings.request),
            })),
            DriverCommand::SetAutonomy { autonomy, reply } => {
                if !self.dialect.autonomy_levels().contains(&autonomy) {
                    let _ = reply.send(Err(protocol_error(format!(
                        "{} cannot enforce that autonomy level.",
                        self.agent_label()
                    ))));
                    return;
                }
                self.autonomy_generation = self.autonomy_generation.wrapping_add(1);
                let mut op = AutonomyOp::new(
                    autonomy,
                    self.dialect.as_ref(),
                    Some(reply),
                    true,
                    self.autonomy_generation,
                );
                let candidate = self.next_candidate(&mut op);
                self.put_in_effect(&mut op, candidate.clone());
                op.in_effect = Some(candidate);
                self.ops.push_back(Op::Autonomy(op));
            }
            _ => return,
        }
        self.pump_ops();
    }

    fn pump_ops(&mut self) {
        while self.phase == Phase::Running && !self.op_busy {
            let Some(op) = self.ops.pop_front() else {
                break;
            };
            match op {
                Op::Ready => self.become_ready(),
                Op::SetOption(op) => self.start_set_option(op),
                Op::Autonomy(op) => self.step_autonomy(op),
            }
        }
        if self.phase == Phase::Running
            && !self.ops_pending()
            && let Some((turn_id, parts)) = self.waiting_prompt.take()
        {
            self.on_prompt(turn_id, &parts);
        }
    }

    /// Mode or option changes still on their way to the agent.
    fn ops_pending(&self) -> bool {
        self.op_busy || !self.ops.is_empty() || self.mode_retry.is_some()
    }

    fn mode_option_id(&self) -> Option<String> {
        self.options
            .as_ref()
            .and_then(OptionModel::mode)
            .map(|option| option.id.clone())
    }

    /// The Mode option's current value.
    fn current_mode(&self) -> Option<String> {
        self.options
            .as_ref()
            .and_then(OptionModel::current_mode)
            .map(str::to_owned)
    }

    fn start_set_option(&mut self, op: SetOptionOp) {
        if matches!(
            op.option_id.as_str(),
            crate::controls::ACCESS | crate::controls::WORKFLOW
        ) && (self.prompt.is_some()
            || !self.permissions.is_empty()
            || !self.ui_requests.is_empty())
        {
            send(
                op.reply,
                Err(protocol_error(
                    "Native control changes require idle with no pending approvals or questions."
                        .into(),
                )),
            );
            return;
        }
        let Some(options) = &self.options else {
            send(op.reply, Err(internal("The agent isn't ready yet.")));
            return;
        };
        if let Err(message) = options.validate(&op.option_id, &op.value) {
            match op.origin {
                Origin::Desired => {
                    let notice = not_offered_notice(options, &op.option_id, &op.value);
                    self.push_notice(NoticeLevel::Info, notice);
                }
                Origin::User | Origin::Deferred => send(
                    op.reply,
                    Err(DriverError {
                        kind: ErrorKind::Other,
                        message,
                        detail: None,
                    }),
                ),
            }
            return;
        }
        let already = options.is_current(&op.option_id, &op.value);
        let is_mode = self.mode_option_id().as_deref() == Some(op.option_id.as_str())
            || matches!(
                op.option_id.as_str(),
                crate::controls::ACCESS | crate::controls::WORKFLOW
            );
        if already {
            if is_mode {
                // The latest pick wins: a mode change parked for the turn's end goes.
                self.drop_parked_mode_changes();
                self.set_pending_mode(None);
                if let ConfigValue::Choice { value } = &op.value {
                    self.check_hand_picked(value);
                }
            }
            send(op.reply, Ok(()));
            return;
        }
        if op
            .deadline
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            // It waited behind earlier changes for as long as an answer may take.
            send(
                op.reply,
                Err(DriverError {
                    kind: ErrorKind::AgentError,
                    message: OPTION_TOO_LATE.into(),
                    detail: None,
                }),
            );
            return;
        }
        if is_mode && self.dialect.mode_change_requires_idle() && self.prompt.is_some() {
            match op.origin {
                Origin::User => self.defer_mode_change(op),
                // A parked change whose turn ended while another request was on its way, with a
                // new turn begun meanwhile: it waits for this turn too.
                Origin::Deferred | Origin::Desired => self.park(Op::SetOption(op)),
            }
            return;
        }
        let session_id = self.session_id.clone().unwrap_or_default();
        let request = self
            .options
            .as_ref()
            .and_then(|options| options.request(&session_id, &op.option_id, &op.value));
        let Some((method, params)) = request else {
            send(op.reply, Err(internal("This option can't be set.")));
            return;
        };
        // A failed write means the agent is gone; the dropped reply says so.
        if let Ok(id) = self.rpc.request(method, params) {
            let deadline = op
                .deadline
                .unwrap_or_else(|| Instant::now() + self.setup.timings.request);
            self.pending.insert(
                id,
                Request {
                    purpose: Purpose::SetOption(op),
                    deadline: Some(deadline),
                },
            );
            self.op_busy = true;
        }
    }

    /// Hermes changes mode only while idle: a mode picked during a turn is parked until the turn
    /// ends and shown as pending. The hand-picked rule applies at once: the policy is Ghost's
    /// own and never waits for the agent.
    fn defer_mode_change(&mut self, op: SetOptionOp) {
        let ConfigValue::Choice { value } = op.value.clone() else {
            send(op.reply, Err(internal("This option can't be set.")));
            return;
        };
        let repeated = self
            .options
            .as_ref()
            .and_then(|options| options.pending(&op.option_id))
            == Some(&op.value);
        self.park(Op::SetOption(SetOptionOp {
            option_id: op.option_id,
            value: op.value,
            reply: None,
            origin: Origin::Deferred,
            deadline: None,
        }));
        self.set_pending_mode(Some(&value));
        if !repeated {
            let notice = format!(
                "{} changes mode when the current turn ends.",
                self.agent_label()
            );
            self.push_notice(NoticeLevel::Info, notice);
        }
        self.check_hand_picked(&value);
        send(op.reply, Ok(()));
    }

    /// Parks a mode change until the turn ends; it replaces whatever was parked before it.
    fn park(&mut self, op: Op) {
        self.drop_parked_mode_changes();
        self.after_turn.push(op);
    }

    /// Drops the mode changes parked for the turn's end: a newer change supersedes them. When one
    /// of them was the current `SetAutonomy`'s, that change no longer settles.
    fn drop_parked_mode_changes(&mut self) {
        let generation = self.autonomy_generation;
        for parked in self.after_turn.drain(..) {
            if let Op::Autonomy(op) = parked
                && op.generation == generation
            {
                self.autonomy_settling = false;
            }
        }
    }

    /// Shows a mode change waiting for the turn's end on the Mode option, or clears it; the
    /// options go out when that changes them.
    fn set_pending_mode(&mut self, mode: Option<&str>) {
        let Some(option_id) = self.mode_option_id() else {
            return;
        };
        let Some(options) = &mut self.options else {
            return;
        };
        let pending = mode.map(|mode| ConfigValue::Choice {
            value: mode.to_owned(),
        });
        if options.pending(&option_id) != pending.as_ref() {
            options.set_pending(&option_id, pending);
            self.push_options();
        }
    }

    fn finish_set_option(&mut self, op: SetOptionOp, result: Result<Value, Failure>) {
        let is_mode = self.mode_option_id().as_deref() == Some(op.option_id.as_str());
        match result {
            Ok(value) => {
                let list = wire::config_options_reply(&value);
                if let Some(options) = &mut self.options {
                    options.applied(&op.option_id, &op.value, list.as_deref());
                    if op.origin == Origin::Deferred {
                        options.set_pending(&op.option_id, None);
                    }
                }
                self.push_options();
                if is_mode && let Some(mode) = self.current_mode() {
                    self.check_hand_picked(&mode);
                }
                send(op.reply, Ok(()));
            }
            Err(failure) => {
                let message = failure_message(&failure, &self.setup);
                let name = self
                    .options
                    .as_ref()
                    .and_then(|options| options.get(&op.option_id))
                    .map_or_else(|| op.option_id.clone(), |option| option.name.to_lowercase());
                match op.origin {
                    Origin::User => send(
                        op.reply,
                        Err(DriverError {
                            kind: ErrorKind::AgentError,
                            message,
                            detail: failure_detail(&failure),
                        }),
                    ),
                    Origin::Desired => {
                        let notice = format!(
                            "Ghost could not restore the {name} you chose earlier ({message})."
                        );
                        self.push_notice(NoticeLevel::Warning, notice);
                    }
                    Origin::Deferred => {
                        if let Some(options) = &mut self.options {
                            options.set_pending(&op.option_id, None);
                        }
                        self.push_options();
                        let notice =
                            format!("{} did not change mode ({message}).", self.agent_label());
                        self.push_notice(NoticeLevel::Warning, notice);
                        // The hand-picked rule applied when the mode was picked; the agent kept
                        // its mode, so the state follows the mode it is really in.
                        if let Some(mode) = self.current_mode() {
                            self.check_hand_picked(&mode);
                        }
                    }
                }
            }
        }
    }

    /// The next candidate worth trying (spec 5.5.2): its mode is offered, or it changes none.
    /// Candidates whose mode is not offered are skipped without asking the agent; after the last
    /// comes the last resort, which changes no mode and asks before every action.
    fn next_candidate(&self, op: &mut AutonomyOp) -> AutonomyCandidate {
        while let Some(candidate) = op.candidates.pop_front() {
            let Some(mode) = &candidate.mode else {
                return candidate;
            };
            match self
                .options
                .as_ref()
                .filter(|options| options.mode().is_some())
            {
                Some(options) if options.offers_mode(mode) => return candidate,
                Some(_) => op.reason = Some(format!("it offers no {mode} mode")),
                None => op.reason = Some("it has no modes".into()),
            }
        }
        let reason = op
            .reason
            .take()
            .unwrap_or_else(|| "it offers no matching mode".into());
        AutonomyCandidate {
            mode: None,
            auto_answer: AutoAnswer::Never,
            note: Some(format!(
                "Ghost could not switch {} to a matching mode ({reason}), so it asks before every action.",
                self.agent_label()
            )),
        }
    }

    /// Puts a candidate's policy in effect before its mode change lands. Mode changes meanwhile
    /// are not hand picks: the op settles when its own change is done.
    fn put_in_effect(&mut self, op: &mut AutonomyOp, candidate: AutonomyCandidate) {
        let current = self.current_mode();
        let applied = Applied {
            requested: op.requested,
            reference: candidate.mode.clone().or_else(|| current.clone()),
            candidate,
        };
        let state = applied.state(current.as_deref());
        self.autonomy = Some(applied);
        self.hand_picked = None;
        self.autonomy_settling = true;
        self.reply_or_announce(op, state);
    }

    /// The op's reply, once; every later state goes out as `Autonomy`, and so does a reply the
    /// session no longer waits for (it gave up), so its state follows the driver's.
    fn reply_or_announce(&mut self, op: &mut AutonomyOp, state: AutonomyState) {
        match op.reply.take() {
            Some(reply) => {
                if let Err(Ok(state)) = reply.send(Ok(state)) {
                    self.outbox.push(DriverEvent::Autonomy(state));
                }
            }
            None => self.outbox.push(DriverEvent::Autonomy(state)),
        }
    }

    /// Spec 5.5.2: the first candidate whose mode is offered (or that has none) and whose mode
    /// change succeeds; the last resort asks before every action. An op a later `SetAutonomy`
    /// replaced does nothing more.
    fn step_autonomy(&mut self, mut op: AutonomyOp) {
        if op.generation != self.autonomy_generation {
            return;
        }
        loop {
            let (candidate, in_effect) = match op.in_effect.take() {
                Some(candidate) => (candidate, true),
                None => (self.next_candidate(&mut op), false),
            };
            let Some(mode) = candidate.mode.clone() else {
                self.complete_autonomy(op, candidate, in_effect);
                return;
            };
            // The options may have changed while the op waited.
            if !self
                .options
                .as_ref()
                .is_some_and(|options| options.offers_mode(&mode))
            {
                op.reason = Some(format!("it offers no {mode} mode"));
                continue;
            }
            if self.current_mode().as_deref() == Some(mode.as_str()) {
                self.complete_autonomy(op, candidate, in_effect);
                return;
            }
            // A fallback of a `SetAutonomy` takes effect as soon as it is chosen, too.
            let in_effect = if !in_effect && op.at_once {
                self.put_in_effect(&mut op, candidate.clone());
                true
            } else {
                in_effect
            };
            if self.dialect.mode_change_requires_idle() && self.prompt.is_some() {
                self.defer_autonomy(op, candidate, &mode, in_effect);
                return;
            }
            let Some(option_id) = self.mode_option_id() else {
                continue;
            };
            let session_id = self.session_id.clone().unwrap_or_default();
            let value = ConfigValue::Choice {
                value: mode.clone(),
            };
            let request = self
                .options
                .as_ref()
                .and_then(|options| options.request(&session_id, &option_id, &value));
            let Some((method, params)) = request else {
                continue;
            };
            // A failed write means the agent is gone; its end follows.
            if let Ok(id) = self.rpc.request(method, params) {
                let deadline = Instant::now() + self.setup.timings.request;
                self.pending.insert(
                    id,
                    Request {
                        purpose: Purpose::Autonomy {
                            op,
                            candidate,
                            mode,
                            in_effect,
                        },
                        deadline: Some(deadline),
                    },
                );
                self.op_busy = true;
            }
            return;
        }
    }

    /// Hermes changes mode only while idle: the mode change is parked until the turn ends and
    /// shown as pending, while the policy already applies.
    fn defer_autonomy(
        &mut self,
        mut op: AutonomyOp,
        candidate: AutonomyCandidate,
        mode: &str,
        in_effect: bool,
    ) {
        if in_effect {
            op.in_effect = Some(candidate);
        } else {
            op.candidates.push_front(candidate);
        }
        self.park(Op::Autonomy(op));
        self.set_pending_mode(Some(mode));
    }

    fn finish_autonomy(
        &mut self,
        mut op: AutonomyOp,
        candidate: AutonomyCandidate,
        mode: &str,
        in_effect: bool,
        result: Result<Value, Failure>,
    ) {
        let current = op.generation == self.autonomy_generation;
        match result {
            Ok(value) => {
                let list = wire::config_options_reply(&value);
                if let Some(option_id) = self.mode_option_id()
                    && let Some(options) = &mut self.options
                {
                    options.applied(
                        &option_id,
                        &ConfigValue::Choice {
                            value: mode.to_owned(),
                        },
                        list.as_deref(),
                    );
                }
                self.push_options();
                // A replaced op only tells the option model what the agent did.
                if current {
                    self.complete_autonomy(op, candidate, in_effect);
                }
            }
            Err(_) if !current => {}
            Err(Failure::Rpc(error))
                if op.retries < MODE_RETRIES && self.dialect.mode_change_retryable(&error) =>
            {
                // The same change again, after a wait; ops queued behind it keep waiting.
                op.retries += 1;
                if in_effect {
                    op.in_effect = Some(candidate);
                } else {
                    op.candidates.push_front(candidate);
                }
                let at = Instant::now() + MODE_RETRY_WAIT * op.retries;
                self.mode_retry = Some((at, op));
                self.op_busy = true;
            }
            Err(failure) => {
                op.reason = Some(failure_message(&failure, &self.setup));
                self.step_autonomy(op);
            }
        }
    }

    /// The op is done: its candidate applies (announced, unless its policy already was), the
    /// mode changes parked before it go, and hand picks count again, measured from the mode now
    /// in effect.
    fn complete_autonomy(
        &mut self,
        mut op: AutonomyOp,
        candidate: AutonomyCandidate,
        in_effect: bool,
    ) {
        let current = self.current_mode();
        let reference = candidate.mode.clone().or_else(|| current.clone());
        if in_effect {
            if let Some(applied) = &mut self.autonomy {
                applied.reference = reference;
            }
        } else {
            let applied = Applied {
                requested: op.requested,
                candidate,
                reference,
            };
            let state = applied.state(current.as_deref());
            self.autonomy = Some(applied);
            self.hand_picked = None;
            self.reply_or_announce(&mut op, state);
        }
        self.drop_parked_mode_changes();
        self.autonomy_settling = false;
        self.set_pending_mode(None);
    }

    /// Spec 5.5.2: a mode picked by hand, or reported by the agent, that differs from the applied
    /// candidate's mode turns auto-answers off; the candidate's mode again restores them. For a
    /// candidate that changes no mode, the mode it applied in stands for its mode. While an
    /// autonomy change settles, its own mode change decides, so nothing counts as a hand pick.
    fn check_hand_picked(&mut self, mode: &str) {
        if self.autonomy_settling {
            return;
        }
        let Some(applied) = &mut self.autonomy else {
            return;
        };
        let Some(reference) = applied.reference.clone() else {
            // The agent had no mode when the candidate applied: this is the mode it runs in.
            applied.reference = Some(mode.to_owned());
            return;
        };
        if mode != reference {
            if self.hand_picked.as_deref() == Some(mode) {
                return;
            }
            let name = self
                .options
                .as_ref()
                .map_or_else(|| mode.to_owned(), |options| options.mode_name(mode));
            let name = mode_phrase(&name);
            let state = AutonomyState {
                requested: applied.requested,
                native_mode: Some(mode.to_owned()),
                auto_answer: AutoAnswer::Never,
                note: Some(format!(
                    "You picked the {name} mode by hand, so Ghost asks before every action. Choose an autonomy level to hand control back."
                )),
            };
            self.hand_picked = Some(mode.to_owned());
            self.outbox.push(DriverEvent::Autonomy(state));
        } else if self.hand_picked.take().is_some() {
            let state = applied.state(Some(mode));
            self.outbox.push(DriverEvent::Autonomy(state));
        }
    }

    // ---- the end ---------------------------------------------------------------------------

    /// Spec 5.6.3: cancel a running turn and wait at most the grace for its reply, answer what is
    /// still pending, close stdin, then run the shutdown chain while stdout is read.
    fn begin_close(&mut self, end: End, plan: StopPlan) {
        if self.end.is_none() {
            self.end = Some(end);
        }
        if self.phase != Phase::Running {
            return;
        }
        self.phase = Phase::Closing;
        self.outbox.push(DriverEvent::Status(DriverStatus::Closing));
        self.stop_waiting();
        self.close_plan = Some(plan.process);
        if self.prompt.is_some() {
            if self
                .prompt
                .as_ref()
                .is_some_and(|prompt| !prompt.cancelling)
            {
                self.cancel_prompt();
            }
            if !plan.cancel_grace.is_zero() {
                self.cancel_deadline = Some(Instant::now() + plan.cancel_grace);
                return;
            }
        }
        self.close_process();
    }

    fn close_process(&mut self) {
        self.cancel_deadline = None;
        for (_, pending) in self.permissions.take_all() {
            self.answer_cancelled(&pending);
        }
        self.cancel_ui(None);
        self.rpc.close();
        if let Some(plan) = self.close_plan.take() {
            self.chain = Some(self.control.shutdown(plan));
        }
    }

    /// stdout ended or the process exited: no more commands; stdout is read to its end.
    fn enter_draining(&mut self) {
        if self.phase == Phase::Draining {
            return;
        }
        self.phase = Phase::Draining;
        self.cancel_ui(None);
        self.cancel_deadline = None;
        self.close_plan = None;
        if self.end.is_none() {
            self.end = Some(if self.ready {
                End::Crashed
            } else {
                End::StartExited
            });
        }
        self.stop_waiting();
        self.flush_commands();
    }

    /// Whatever waits for the agent's answer gets none: start requests, option and autonomy
    /// changes (their dropped replies read as "the agent stopped"), held commands. A running
    /// prompt's reply is still welcome. A start that ends here never reaches `Ready`, so the
    /// transcript events it held go too; otherwise they would keep stdout paused to the end.
    fn stop_waiting(&mut self) {
        self.pending
            .retain(|_, request| matches!(request.purpose, Purpose::Prompt));
        self.open = None;
        self.replay = None;
        self.early.clear();
        self.early_bytes = 0;
        self.op_busy = false;
        self.autonomy_settling = false;
        self.ops.clear();
        self.mode_retry = None;
        self.after_turn.clear();
        self.held.clear();
        if let Some((turn_id, _)) = self.waiting_prompt.take() {
            // Rule: every prompt ends once; this one never reached the agent.
            let closed = matches!(self.end, Some(End::Closed));
            self.outbox.push(DriverEvent::TurnEnded {
                turn_id,
                stop_reason: if closed {
                    StopReason::Cancelled
                } else {
                    StopReason::Error
                },
                usage: None,
                error: (!closed)
                    .then(|| internal("The agent stopped before this prompt went out.")),
            });
        }
        if !self.ready {
            let dropped = self.outbox.drop_held();
            self.dropped_before_ready = self.dropped_before_ready.saturating_add(dropped);
        }
    }
}

/// Rule 1: before `Ready`, transcript events wait (notices and usage go out).
fn holds_until_ready(event: &DriverEvent) -> bool {
    matches!(
        event,
        DriverEvent::Event { event, .. }
            if !matches!(event, GhostEvent::Notice { .. } | GhostEvent::UsageUpdated(_))
    )
}

fn not_advertised(method: &str) -> bool {
    matches!(
        method,
        "fs/read_text_file" | "fs/write_text_file" | "elicitation/create"
    ) || method.starts_with("terminal/")
}

fn send<T>(reply: Option<oneshot::Sender<T>>, value: T) {
    if let Some(reply) = reply {
        let _ = reply.send(value);
    }
}

fn difference(now: Option<u64>, before: Option<u64>) -> Option<u64> {
    now.map(|now| now.saturating_sub(before.unwrap_or(0)))
}

/// The length of `value` as JSON, counted without building the text.
fn json_bytes(value: &Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len());
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    match serde_json::to_writer(&mut count, value) {
        Ok(()) => count.0,
        // A JSON value always serializes; if it ever did not, it would count as too large.
        Err(_) => usize::MAX,
    }
}

/// The agent's error, raw (the session redacts it).
fn agent_error(error: &RpcError) -> DriverError {
    DriverError {
        kind: ErrorKind::AgentError,
        message: if error.message.trim().is_empty() {
            format!("The agent reported error {}.", error.code)
        } else {
            error.message.clone()
        },
        detail: error.data.as_ref().map(Value::to_string),
    }
}

fn protocol_error(message: String) -> DriverError {
    DriverError {
        kind: ErrorKind::ProtocolError,
        message,
        detail: None,
    }
}

fn failure_message(failure: &Failure, setup: &SessionSetup) -> String {
    match failure {
        Failure::Rpc(error) if !error.message.trim().is_empty() => error.message.clone(),
        Failure::Rpc(error) => format!("the agent reported error {}", error.code),
        Failure::Timeout => format!(
            "the agent did not answer within {} seconds",
            setup.timings.request.as_secs()
        ),
        Failure::Dropped(message) => message.clone(),
    }
}

fn failure_detail(failure: &Failure) -> Option<String> {
    match failure {
        Failure::Rpc(error) => error.data.as_ref().map(Value::to_string),
        Failure::Timeout | Failure::Dropped(_) => None,
    }
}

/// A mode's name for "the <name> mode": claude-acp calls one "Plan Mode", which would read
/// "the Plan Mode mode".
fn mode_phrase(name: &str) -> &str {
    let trimmed = name.trim_end();
    match trimmed.len().checked_sub(" mode".len()) {
        Some(cut)
            if trimmed.is_char_boundary(cut)
                && trimmed[cut..].eq_ignore_ascii_case(" mode")
                && cut > 0 =>
        {
            &trimmed[..cut]
        }
        _ => trimmed,
    }
}

fn fresh_notice(reason: &str) -> String {
    format!(
        "Could not continue the earlier session ({reason}); started a new one. The conversation above stays here, but the agent does not remember it."
    )
}

/// The notice for an earlier choice the agent no longer offers.
fn not_offered_notice(options: &OptionModel, option_id: &str, value: &ConfigValue) -> String {
    match options.get(option_id) {
        Some(option) => {
            let chosen = match value {
                ConfigValue::Choice { value } => value.clone(),
                ConfigValue::Toggle { .. } => value_label(option, value),
            };
            let current = value_label(option, &options::current_value(option));
            format!(
                "The {} you chose earlier ({chosen}) isn't offered anymore; using {current}.",
                option.name.to_lowercase()
            )
        }
        None => format!("The setting \"{option_id}\" you chose earlier isn't offered anymore."),
    }
}

fn exited_message(exit: Option<ExitInfo>) -> String {
    match exit {
        Some(ExitInfo {
            code: Some(code), ..
        }) => format!("The agent exited before it finished starting (exit code {code})."),
        Some(ExitInfo {
            signal: Some(signal),
            ..
        }) => format!("The agent exited before it finished starting (signal {signal})."),
        _ => "The agent exited before it finished starting.".into(),
    }
}

async fn poll_chain(chain: &mut Option<BoxFuture<'static, ShutdownReport>>) -> ShutdownReport {
    match chain {
        Some(chain) => chain.await,
        None => std::future::pending().await,
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
