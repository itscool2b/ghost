//! Test doubles for the session manager's tests (feature `testing`).
//!
//! [`ScriptedDriver`] plays a list of [`Step`]s: wait for a command, emit an event, wait, spawn a
//! process through the session's spawner, exit, or hang like a wedged driver. A [`ScriptHandle`]
//! injects events and exits at any time and reads back every command the driver received.
//! [`ScriptedDriverFactory`] hands out queued drivers in order, and [`DirectSpawner`] is an
//! [`AgentSpawner`] over any [`ProcessSpawner`] (a `FakeSpawner` in tests) without a session.
//!
//! The session actor mints turn ids, so a script cannot name them: [`Step::EmitInTurn`] and
//! [`Step::EndTurn`] use the turn of the last `Prompt` the driver received, and
//! [`ScriptHandle::wait_for`] waits for a command (a `Prompt`, to read its turn id) without polling.
//!
//! A scripted driver keeps the contract's rules only as far as its steps say, so tests can break
//! them on purpose: it ends no turn, answers no permission and waits for no permit unless a step
//! does. What it always does: it records every command; it answers every `SetOption` and
//! `SetAutonomy` with the handle's replies (by default `Ok`, and the requested level with no mode
//! and `Never`), except while it hangs, when it keeps the replies unanswered; it waits for event
//! permits; and a closed command channel ends it with `Closed` (contract rule 4), except while it
//! hangs.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use o7_process::{
    LaunchSpec, LogFactory, MAX_LINE_BYTES, ProcessSpawner, SpawnError, SpawnSpec, SpawnedProcess,
};
use o7_protocol::{
    AutoAnswer, Autonomy, AutonomyState, DriverKind, ErrorKind, StopReason, TokenUsage,
};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::driver::{
    AgentDriver, AgentSpawner, DriverCommand, DriverContext, DriverError, DriverEvent, DriverExit,
    DriverFactory, EventSender, LoggedCommand, SessionSetup,
};

/// One step of a [`ScriptedDriver`].
#[derive(Debug)]
pub enum Step {
    /// Waits for the next command (in arrival order, counting those that arrived earlier); a
    /// mismatch panics the driver task, which the session sees as `Panicked`. A closed command
    /// channel satisfies `Shutdown` and `Any`, and ends the run with `Closed` for anything else.
    Expect(CommandMatch),
    /// Hands the event over, waiting for a permit (and answering commands meanwhile).
    Emit(DriverEvent),
    /// Like `Emit`, with the event tagged with the turn of the last `Prompt` the driver received:
    /// the `turn` of an `Event`, a `Permission` or a `UiRequest`, the `turn_id` of a `TurnEnded`.
    /// Other events go out unchanged. Before any `Prompt`, it panics the driver task.
    EmitInTurn(DriverEvent),
    /// `TurnEnded` for the turn of the last `Prompt` the driver received. Before any `Prompt`, it
    /// panics the driver task.
    EndTurn {
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
        error: Option<DriverError>,
    },
    /// Virtual time under paused clocks. Commands and injections are handled meanwhile.
    Wait(Duration),
    /// Spawns through the session's spawner and keeps the process until `run` returns (its control
    /// is recorded by the session's spawner). A failed spawn ends the run with `StartFailed`.
    Spawn(LaunchSpec),
    /// `run` returns this.
    Exit(DriverExit),
    /// Never emits or exits again, injections included; keeps reading and recording commands and
    /// never answers them (a wedged driver).
    Hang,
}

/// Which command an [`Step::Expect`] waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandMatch {
    Prompt,
    Steer,
    Interrupt,
    /// `Some` matches that request id only.
    RespondPermission {
        request_id: Option<String>,
    },
    RespondUi {
        request_id: Option<String>,
    },
    SetOption,
    SetAutonomy,
    RewindFiles,
    Shutdown,
    Any,
}

impl CommandMatch {
    fn matches(&self, command: &LoggedCommand) -> bool {
        let wanted = |expected: &Option<String>, got: &str| {
            expected.as_deref().is_none_or(|expected| expected == got)
        };
        match (self, command) {
            (CommandMatch::Any, _)
            | (CommandMatch::Prompt, LoggedCommand::Prompt { .. })
            | (CommandMatch::Steer, LoggedCommand::Steer { .. })
            | (CommandMatch::Interrupt, LoggedCommand::Interrupt)
            | (CommandMatch::SetOption, LoggedCommand::SetOption { .. })
            | (CommandMatch::SetAutonomy, LoggedCommand::SetAutonomy { .. })
            | (CommandMatch::RewindFiles, LoggedCommand::RewindFiles { .. })
            | (CommandMatch::Shutdown, LoggedCommand::Shutdown) => true,
            (
                CommandMatch::RespondPermission { request_id },
                LoggedCommand::RespondPermission {
                    request_id: got, ..
                },
            )
            | (
                CommandMatch::RespondUi { request_id },
                LoggedCommand::RespondUi {
                    request_id: got, ..
                },
            ) => wanted(request_id, got),
            _ => false,
        }
    }

    /// Whether a closed command channel (a `Shutdown` with zero grace) satisfies it.
    fn accepts_close(&self) -> bool {
        matches!(self, CommandMatch::Shutdown | CommandMatch::Any)
    }
}

/// A driver that plays [`Step`]s.
pub struct ScriptedDriver {
    steps: VecDeque<Step>,
    shared: Arc<Shared>,
    injected: mpsc::UnboundedReceiver<Injected>,
}

/// Reaches into a running [`ScriptedDriver`]. Cloning shares it.
#[derive(Clone)]
pub struct ScriptHandle {
    shared: Arc<Shared>,
    inject: mpsc::UnboundedSender<Injected>,
}

struct Shared {
    commands: Mutex<Vec<Received>>,
    /// How many commands arrived, so `wait_for` wakes on each.
    arrived: watch::Sender<usize>,
    option_reply: Mutex<Result<(), DriverError>>,
    steer_reply: Mutex<crate::driver::SteerResult>,
    rewind_reply: Mutex<Result<crate::driver::FileRewindResult, DriverError>>,
    /// `None`: the requested level, no native mode, `Never`.
    autonomy_reply: Mutex<Option<Result<AutonomyState, DriverError>>>,
}

/// A command the driver received.
struct Received {
    command: LoggedCommand,
    /// An earlier [`ScriptHandle::wait_for`] returned it.
    claimed: bool,
}

enum Injected {
    Emit(Box<DriverEvent>),
    Exit(DriverExit),
}

impl ScriptedDriver {
    pub fn new(steps: Vec<Step>) -> (ScriptedDriver, ScriptHandle) {
        let shared = Arc::new(Shared {
            commands: Mutex::new(Vec::new()),
            arrived: watch::Sender::new(0),
            option_reply: Mutex::new(Ok(())),
            steer_reply: Mutex::new(crate::driver::SteerResult::Rejected {
                message: "The scripted agent does not steer turns.".into(),
            }),
            rewind_reply: Mutex::new(Err(DriverError {
                kind: ErrorKind::ProtocolError,
                message: "The scripted agent does not rewind files.".into(),
                detail: None,
            })),
            autonomy_reply: Mutex::new(None),
        });
        let (inject, injected) = mpsc::unbounded_channel();
        let driver = ScriptedDriver {
            steps: steps.into(),
            shared: Arc::clone(&shared),
            injected,
        };
        (driver, ScriptHandle { shared, inject })
    }
}

impl fmt::Debug for ScriptedDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScriptedDriver")
            .field("steps", &self.steps)
            .finish_non_exhaustive()
    }
}

impl ScriptHandle {
    /// Injects an event, emitted in order with the steps' events (before the next step begins, or
    /// while one waits). Ignored once the driver ended or hangs.
    pub fn emit(&self, event: DriverEvent) {
        let _ = self.inject.send(Injected::Emit(Box::new(event)));
    }

    /// Makes `run` return `exit`, after the events emitted before it.
    pub fn exit(&self, exit: DriverExit) {
        let _ = self.inject.send(Injected::Exit(exit));
    }

    /// Every command received so far, in order.
    pub fn commands(&self) -> Vec<LoggedCommand> {
        lock(&self.shared.commands)
            .iter()
            .map(|received| received.command.clone())
            .collect()
    }

    /// Waits until the driver has received a command that `matcher` matches and that no earlier
    /// `wait_for` returned, then returns it. Commands count from the driver's start, so one that
    /// arrived before the call is returned at once; with none coming, it waits forever (wrap it in
    /// a timeout).
    pub async fn wait_for(&self, matcher: CommandMatch) -> LoggedCommand {
        let mut arrived = self.shared.arrived.subscribe();
        loop {
            if let Some(command) = self.shared.claim(&matcher) {
                return command;
            }
            // The handle keeps the sender alive, so this only ever waits for the next command.
            if arrived.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }

    /// What `SetOption` answers from now on; the default is `Ok`. An `Err` stands for an agent
    /// that refused the value (ACP -32602) or did not answer in time.
    pub fn set_option_reply(&self, reply: Result<(), DriverError>) {
        *lock(&self.shared.option_reply) = reply;
    }
    pub fn set_steer_reply(&self, reply: crate::driver::SteerResult) {
        *lock(&self.shared.steer_reply) = reply;
    }
    /// What `RewindFiles` answers from now on; by default the scripted agent refuses.
    pub fn set_rewind_reply(&self, reply: Result<crate::driver::FileRewindResult, DriverError>) {
        *lock(&self.shared.rewind_reply) = reply;
    }

    /// What `SetAutonomy` answers from now on. The default is the requested level, no native mode,
    /// [`AutoAnswer::Never`].
    pub fn set_autonomy_reply(&self, state: AutonomyState) {
        self.set_autonomy_result(Ok(state));
    }

    /// The same, or an error from now on (a mode change the agent refused).
    pub fn set_autonomy_result(&self, reply: Result<AutonomyState, DriverError>) {
        *lock(&self.shared.autonomy_reply) = Some(reply);
    }
}

impl fmt::Debug for ScriptHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScriptHandle")
            .field("commands", &self.commands())
            .finish_non_exhaustive()
    }
}

impl Shared {
    fn record(&self, command: LoggedCommand) {
        lock(&self.commands).push(Received {
            command,
            claimed: false,
        });
        self.arrived.send_modify(|count| *count += 1);
    }

    /// The first unclaimed command `matcher` matches, now claimed.
    fn claim(&self, matcher: &CommandMatch) -> Option<LoggedCommand> {
        let mut commands = lock(&self.commands);
        let received = commands
            .iter_mut()
            .find(|received| !received.claimed && matcher.matches(&received.command))?;
        received.claimed = true;
        Some(received.command.clone())
    }

    fn option_reply(&self) -> Result<(), DriverError> {
        lock(&self.option_reply).clone()
    }

    fn autonomy_reply(&self, requested: Autonomy) -> Result<AutonomyState, DriverError> {
        lock(&self.autonomy_reply)
            .clone()
            .unwrap_or(Ok(AutonomyState {
                requested,
                native_mode: None,
                auto_answer: AutoAnswer::Never,
                note: None,
            }))
    }
}

impl AgentDriver for ScriptedDriver {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        let ScriptedDriver {
            steps,
            shared,
            injected,
        } = *self;
        let DriverContext {
            commands,
            events,
            spawner,
            setup,
        } = ctx;
        let runner = Runner {
            commands,
            commands_open: true,
            events,
            events_open: true,
            spawner,
            setup,
            shared,
            injected,
            injections_open: true,
            received: VecDeque::new(),
            current_turn: None,
            processes: Vec::new(),
        };
        runner.run(steps).boxed()
    }
}

/// A running scripted driver.
struct Runner {
    commands: mpsc::UnboundedReceiver<DriverCommand>,
    commands_open: bool,
    events: EventSender,
    /// False once the session dropped the receiver: later events are discarded.
    events_open: bool,
    spawner: Arc<dyn AgentSpawner>,
    setup: SessionSetup,
    shared: Arc<Shared>,
    injected: mpsc::UnboundedReceiver<Injected>,
    injections_open: bool,
    /// Commands received and not yet consumed by an `Expect`.
    received: VecDeque<LoggedCommand>,
    /// The turn of the last `Prompt` received.
    current_turn: Option<String>,
    /// Kept alive until `run` returns.
    processes: Vec<SpawnedProcess>,
}

/// What woke a waiting runner.
enum Input {
    /// A command was accepted (recorded, answered, queued) or an event injected and emitted.
    Handled,
    CommandsClosed,
    Exit(DriverExit),
    Timeout,
}

impl Runner {
    async fn run(mut self, mut steps: VecDeque<Step>) -> DriverExit {
        while let Some(step) = steps.pop_front() {
            if let Some(exit) = self.drain_injections().await {
                return exit;
            }
            match step {
                Step::Emit(event) => self.emit(event).await,
                Step::EmitInTurn(event) => {
                    let turn_id = self.turn();
                    self.emit(in_turn(event, turn_id)).await;
                }
                Step::EndTurn {
                    stop_reason,
                    usage,
                    error,
                } => {
                    let turn_id = self.turn();
                    self.emit(DriverEvent::TurnEnded {
                        turn_id,
                        stop_reason,
                        usage,
                        error,
                    })
                    .await;
                }
                Step::Wait(duration) => {
                    // The channel may have closed while an earlier step waited for a permit.
                    if !self.commands_open {
                        return DriverExit::Closed;
                    }
                    let deadline = Instant::now() + duration;
                    loop {
                        match self.next_input(Some(deadline)).await {
                            Input::Handled => {}
                            Input::Timeout => break,
                            Input::CommandsClosed => return DriverExit::Closed,
                            Input::Exit(exit) => return exit,
                        }
                    }
                }
                Step::Expect(matcher) => {
                    let command = loop {
                        if let Some(command) = self.received.pop_front() {
                            break Some(command);
                        }
                        if !self.commands_open {
                            break None;
                        }
                        match self.next_input(None).await {
                            Input::Handled | Input::CommandsClosed | Input::Timeout => {}
                            Input::Exit(exit) => return exit,
                        }
                    };
                    match command {
                        Some(command) => {
                            assert!(
                                matcher.matches(&command),
                                "ScriptedDriver expected {matcher:?}, got {command:?}"
                            );
                        }
                        None if matcher.accepts_close() => {}
                        None => return DriverExit::Closed,
                    }
                }
                Step::Spawn(launch) => {
                    if let Err(exit) = self.spawn(launch).await {
                        return exit;
                    }
                }
                Step::Exit(exit) => return exit,
                Step::Hang => return self.hang().await,
            }
        }
        self.idle().await
    }

    /// No steps left: answers commands until a `Shutdown`, a closed channel, or an injected exit.
    async fn idle(mut self) -> DriverExit {
        loop {
            while let Some(command) = self.received.pop_front() {
                if command == LoggedCommand::Shutdown {
                    return DriverExit::Closed;
                }
            }
            if !self.commands_open {
                return DriverExit::Closed;
            }
            match self.next_input(None).await {
                Input::Handled | Input::Timeout => {}
                Input::CommandsClosed => return DriverExit::Closed,
                Input::Exit(exit) => return exit,
            }
        }
    }

    /// Reads and records commands forever, answering none of them.
    async fn hang(mut self) -> DriverExit {
        let mut unanswered = Vec::new();
        while self.commands_open {
            match self.commands.recv().await {
                Some(command) => {
                    self.shared.record(LoggedCommand::from(&command));
                    unanswered.push(command);
                }
                None => self.commands_open = false,
            }
        }
        std::future::pending().await
    }

    /// Emits what the handle injected before this point. Returns an injected exit.
    async fn drain_injections(&mut self) -> Option<DriverExit> {
        while self.injections_open {
            match self.injected.try_recv() {
                Ok(Injected::Emit(event)) => self.emit(*event).await,
                Ok(Injected::Exit(exit)) => return Some(exit),
                Err(mpsc::error::TryRecvError::Empty) => return None,
                Err(mpsc::error::TryRecvError::Disconnected) => self.injections_open = false,
            }
        }
        None
    }

    /// Waits for a command, an injection, or `deadline`, and handles what came.
    async fn next_input(&mut self, deadline: Option<Instant>) -> Input {
        tokio::select! {
            command = self.commands.recv(), if self.commands_open => match command {
                Some(command) => {
                    self.accept(command);
                    Input::Handled
                }
                None => {
                    self.commands_open = false;
                    Input::CommandsClosed
                }
            },
            injected = self.injected.recv(), if self.injections_open => match injected {
                Some(Injected::Emit(event)) => {
                    self.emit(*event).await;
                    Input::Handled
                }
                Some(Injected::Exit(exit)) => Input::Exit(exit),
                None => {
                    self.injections_open = false;
                    Input::Handled
                }
            },
            () = sleep_until(deadline) => Input::Timeout,
        }
    }

    /// Records a command, answers it when it carries a reply, and queues it for `Expect`.
    fn accept(&mut self, command: DriverCommand) {
        let logged = LoggedCommand::from(&command);
        self.shared.record(logged.clone());
        match command {
            DriverCommand::McpAction { reply, .. } => {
                crate::mcp::unsupported(reply, "The scripted agent does not connect MCP servers.")
            }
            DriverCommand::Steer { reply, .. } => {
                let _ = reply.send(lock(&self.shared.steer_reply).clone());
            }
            DriverCommand::Prompt { turn, .. } => self.current_turn = Some(turn.turn_id),
            DriverCommand::SetOption { reply, .. } => {
                let _ = reply.send(self.shared.option_reply());
            }
            DriverCommand::SetAutonomy { autonomy, reply } => {
                let _ = reply.send(self.shared.autonomy_reply(autonomy));
            }
            DriverCommand::RewindFiles { reply, .. } => {
                let _ = reply.send(lock(&self.shared.rewind_reply).clone());
            }
            DriverCommand::Interrupt
            | DriverCommand::EditPlan { .. }
            | DriverCommand::RespondPermission { .. }
            | DriverCommand::RespondUi { .. }
            | DriverCommand::Shutdown { .. } => {}
        }
        self.received.push_back(logged);
    }

    /// Hands `event` over once a permit is free, answering commands while it waits.
    async fn emit(&mut self, event: DriverEvent) {
        if !self.events_open {
            return;
        }
        let bytes = event.approx_bytes();
        loop {
            tokio::select! {
                biased;
                permit = self.events.reserve(bytes) => {
                    match permit {
                        Ok(permit) => permit.send(event),
                        Err(_) => self.events_open = false,
                    }
                    return;
                }
                command = self.commands.recv(), if self.commands_open => match command {
                    Some(command) => self.accept(command),
                    None => self.commands_open = false,
                },
            }
        }
    }

    /// The turn of the last `Prompt` received; a turn step before any `Prompt` is a broken script.
    fn turn(&self) -> String {
        match &self.current_turn {
            Some(turn_id) => turn_id.clone(),
            None => panic!("ScriptedDriver: a turn step came before any Prompt"),
        }
    }

    async fn spawn(&mut self, launch: LaunchSpec) -> Result<(), DriverExit> {
        let spec = SpawnSpec {
            launch,
            folder: self.setup.folder.clone(),
            env: Arc::clone(&self.setup.env),
            max_line_bytes: MAX_LINE_BYTES,
        };
        match self.spawner.spawn(spec).await {
            Ok(process) => {
                self.processes.push(process);
                Ok(())
            }
            Err(error) => Err(DriverExit::StartFailed(spawn_failure(&error))),
        }
    }
}

/// `event` tagged with `turn_id`, for [`Step::EmitInTurn`].
fn in_turn(event: DriverEvent, turn_id: String) -> DriverEvent {
    match event {
        DriverEvent::Event { event, .. } => DriverEvent::Event {
            turn: Some(turn_id),
            event,
        },
        DriverEvent::Permission { request, .. } => DriverEvent::Permission {
            turn: Some(turn_id),
            request,
        },
        DriverEvent::UiRequest { request, .. } => DriverEvent::UiRequest {
            turn: Some(turn_id),
            request,
        },
        DriverEvent::TurnEnded {
            stop_reason,
            usage,
            error,
            ..
        } => DriverEvent::TurnEnded {
            turn_id,
            stop_reason,
            usage,
            error,
        },
        untagged @ (DriverEvent::Status(_)
        | DriverEvent::RunState { .. }
        | DriverEvent::BackgroundWork { .. }
        | DriverEvent::Ready(_)
        | DriverEvent::NativeSessionId(_)
        | DriverEvent::PermissionWithdrawn { .. }
        | DriverEvent::UiWithdrawn { .. }
        | DriverEvent::Commands(_)
        | DriverEvent::Loaded(_)
        | DriverEvent::Options(_)
        | DriverEvent::Autonomy(_)
        | DriverEvent::Title(_)
        | DriverEvent::AuthRequired(_)
        | DriverEvent::UsageLimit(_)) => untagged,
    }
}

/// Sleeps until `deadline`, or forever without one.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// How a driver reports a failed spawn.
fn spawn_failure(error: &SpawnError) -> DriverError {
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

/// Hands out queued [`ScriptedDriver`]s in order. Once they run out, every driver it creates fails
/// to start, which shows in a test as a start-failure card.
pub struct ScriptedDriverFactory {
    queue: Mutex<VecDeque<ScriptedDriver>>,
    kinds: Mutex<Vec<DriverKind>>,
}

impl ScriptedDriverFactory {
    pub fn new(drivers: Vec<ScriptedDriver>) -> Arc<ScriptedDriverFactory> {
        Arc::new(ScriptedDriverFactory {
            queue: Mutex::new(drivers.into()),
            kinds: Mutex::new(Vec::new()),
        })
    }

    /// The kind of every driver created so far, in order.
    pub fn kinds(&self) -> Vec<DriverKind> {
        lock(&self.kinds).clone()
    }
}

impl fmt::Debug for ScriptedDriverFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScriptedDriverFactory")
            .field("queued", &lock(&self.queue).len())
            .field("kinds", &self.kinds())
            .finish()
    }
}

impl DriverFactory for ScriptedDriverFactory {
    fn create(&self, kind: DriverKind) -> Box<dyn AgentDriver> {
        lock(&self.kinds).push(kind);
        match lock(&self.queue).pop_front() {
            Some(driver) => Box::new(driver),
            None => Box::new(NoDriverLeft),
        }
    }
}

/// What an exhausted [`ScriptedDriverFactory`] creates.
struct NoDriverLeft;

impl AgentDriver for NoDriverLeft {
    fn run(self: Box<Self>, _ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        async {
            DriverExit::StartFailed(DriverError {
                kind: ErrorKind::Internal,
                message: "The ScriptedDriverFactory has no driver left for this start.".into(),
                detail: None,
            })
        }
        .boxed()
    }
}

/// The chat folder [`DirectSpawner`]'s logs go under.
pub const DIRECT_LOG_CHAT: &str = "direct";

/// An [`AgentSpawner`] over a [`ProcessSpawner`], without a session: it mints a UUIDv7 spawn id and
/// opens its log under [`DIRECT_LOG_CHAT`].
pub struct DirectSpawner {
    base: Arc<dyn ProcessSpawner>,
    logs: LogFactory,
}

impl DirectSpawner {
    pub fn new(base: Arc<dyn ProcessSpawner>, logs: LogFactory) -> Arc<DirectSpawner> {
        Arc::new(DirectSpawner { base, logs })
    }
}

impl fmt::Debug for DirectSpawner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DirectSpawner")
            .field("logs", &self.logs)
            .finish_non_exhaustive()
    }
}

impl AgentSpawner for DirectSpawner {
    fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        let spawn_id = uuid::Uuid::now_v7().hyphenated().to_string();
        let log = self.logs.open(DIRECT_LOG_CHAT, &spawn_id);
        self.base.spawn(spec, log)
    }
}

/// Locks `mutex`, recovering the data of a thread that panicked while holding it: a test double
/// must keep answering after a test's own panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use o7_process::testing::{FakeAgentEnd, FakeSpawner};
    use o7_process::{EnvMap, EnvSource, ProcessCwd, ProtocolLog, Redactor, ResolvedEnv};
    use o7_protocol::session::ConfigValue;
    use o7_protocol::{
        NoticeLevel, O7Event, PermissionOption, PermissionOptionKind, PermissionRequest,
        PromptPart, ToolKind, UsageScope,
    };
    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;

    use super::*;
    use crate::driver::{
        DRIVER_EVENT_BYTES, DriverTimings, EventReceiver, PermissionOutcome, StopPlan, TurnRef,
        event_channel,
    };

    fn launch() -> LaunchSpec {
        LaunchSpec {
            label: "fake".into(),
            program: PathBuf::from("/bin/fake"),
            args: Vec::new(),
            cwd: ProcessCwd::ChatFolder,
            set_env: Vec::new(),
            daemons: Vec::new(),
            startup_timeout: Duration::from_secs(5),
        }
    }

    fn setup() -> SessionSetup {
        SessionSetup {
            mcp_servers: Vec::new(),
            attachment_dir: None,
            attachment_writer: None,
            chat_id: "chat".into(),
            agent_id: "mock".into(),
            dialect: "mock".into(),
            folder: PathBuf::from("/work"),
            agent_folder: "/work".into(),
            add_dirs: Vec::new(),
            launch: launch(),
            env: Arc::new(ResolvedEnv {
                vars: EnvMap::new(),
                source: EnvSource::Fallback {
                    reason: "test".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            resume: None,
            native_start: Default::default(),
            autonomy: Autonomy::FullAuto,
            desired: BTreeMap::new(),
            timings: DriverTimings::STANDARD,
            client_version: "0.1.0".into(),
        }
    }

    /// A protocol log root in a fresh temp folder, removed when the guard drops, so these tests
    /// never write under the filesystem root (on Windows a root of `/logs` is `C:\logs`).
    struct TempLogs(PathBuf);

    impl TempLogs {
        fn new() -> TempLogs {
            TempLogs(
                std::env::temp_dir().join(format!("o7-drivers-testing-{}", uuid::Uuid::now_v7())),
            )
        }

        fn factory(&self) -> LogFactory {
            LogFactory::new(
                self.0.clone(),
                Arc::new(Redactor::standard()),
                tokio::runtime::Handle::current(),
            )
        }
    }

    impl Drop for TempLogs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Spawns through `FakeSpawner` with a log that writes nothing: the scripted driver's tests read
    /// no log, and [`DirectSpawner`]'s own logging is tested on its own.
    struct QuietSpawner(Arc<FakeSpawner>);

    impl AgentSpawner for QuietSpawner {
        fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
            self.0.spawn(spec, ProtocolLog::disabled())
        }
    }

    struct Session {
        commands: mpsc::UnboundedSender<DriverCommand>,
        events: EventReceiver,
        run: JoinHandle<DriverExit>,
        ends: mpsc::UnboundedReceiver<FakeAgentEnd>,
        spawner: Arc<FakeSpawner>,
    }

    /// Runs `driver` on a fake spawner, as the session actor would.
    fn start(driver: Box<dyn AgentDriver>) -> Session {
        let (spawner, ends) = FakeSpawner::new();
        let (commands, receiver) = mpsc::unbounded_channel();
        let (sender, events) = event_channel();
        let ctx = DriverContext {
            commands: receiver,
            events: sender,
            spawner: Arc::new(QuietSpawner(Arc::clone(&spawner))),
            setup: setup(),
        };
        let run = tokio::spawn(driver.run(ctx));
        Session {
            commands,
            events,
            run,
            ends,
            spawner,
        }
    }

    fn notice(text: &str) -> DriverEvent {
        DriverEvent::Event {
            turn: None,
            event: O7Event::Notice {
                level: NoticeLevel::Info,
                message: text.into(),
            },
        }
    }

    fn prompt(turn_id: &str) -> DriverCommand {
        DriverCommand::Prompt {
            turn: TurnRef {
                turn_id: turn_id.into(),
            },
            parts: vec![PromptPart::Text { text: "hi".into() }],
        }
    }

    fn turn_ended(turn_id: &str) -> DriverEvent {
        DriverEvent::TurnEnded {
            turn_id: turn_id.into(),
            stop_reason: StopReason::EndTurn,
            usage: None,
            error: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn steps_play_in_order_and_commands_are_recorded() {
        let (driver, handle) = ScriptedDriver::new(vec![
            Step::Emit(notice("ready")),
            Step::Expect(CommandMatch::Prompt),
            Step::Wait(Duration::from_secs(1)),
            Step::Emit(turn_ended("t1")),
            Step::Expect(CommandMatch::Shutdown),
            Step::Exit(DriverExit::Closed),
        ]);
        let mut session = start(Box::new(driver));
        assert_eq!(session.events.recv().await, Some(notice("ready")));
        session.commands.send(prompt("t1")).expect("send");
        assert_eq!(session.events.recv().await, Some(turn_ended("t1")));
        session
            .commands
            .send(DriverCommand::Shutdown {
                plan: StopPlan {
                    cancel_grace: Duration::ZERO,
                    process: o7_process::ShutdownPlan::STANDARD,
                },
            })
            .expect("send");
        assert_eq!(session.run.await.expect("join"), DriverExit::Closed);
        assert_eq!(
            handle.commands(),
            [
                LoggedCommand::Prompt {
                    turn_id: "t1".into(),
                    parts: vec![PromptPart::Text { text: "hi".into() }]
                },
                LoggedCommand::Shutdown
            ]
        );
    }

    #[tokio::test]
    async fn options_and_autonomy_are_answered_at_once() {
        let (driver, handle) = ScriptedDriver::new(vec![Step::Expect(CommandMatch::SetOption)]);
        let session = start(Box::new(driver));
        let (reply, answer) = oneshot::channel();
        session
            .commands
            .send(DriverCommand::SetOption {
                option_id: "model".into(),
                value: ConfigValue::Choice {
                    value: "fast".into(),
                },
                reply,
            })
            .expect("send");
        assert_eq!(answer.await.expect("answered"), Ok(()));

        let (reply, answer) = oneshot::channel();
        session
            .commands
            .send(DriverCommand::SetAutonomy {
                autonomy: Autonomy::Ask,
                reply,
            })
            .expect("send");
        let state = answer.await.expect("answered").expect("ok");
        assert_eq!(state.requested, Autonomy::Ask);
        assert_eq!(state.auto_answer, AutoAnswer::Never);

        let custom = AutonomyState {
            requested: Autonomy::FullAuto,
            native_mode: Some("acceptEdits".into()),
            auto_answer: AutoAnswer::Never,
            note: Some("bypass is off".into()),
        };
        handle.set_autonomy_reply(custom.clone());
        let (reply, answer) = oneshot::channel();
        session
            .commands
            .send(DriverCommand::SetAutonomy {
                autonomy: Autonomy::FullAuto,
                reply,
            })
            .expect("send");
        assert_eq!(answer.await.expect("answered"), Ok(custom));
        drop(session.commands);
        assert_eq!(session.run.await.expect("join"), DriverExit::Closed);
    }

    fn set_option(session: &Session) -> oneshot::Receiver<Result<(), DriverError>> {
        let (reply, answer) = oneshot::channel();
        session
            .commands
            .send(DriverCommand::SetOption {
                option_id: "model".into(),
                value: ConfigValue::Choice {
                    value: "gone".into(),
                },
                reply,
            })
            .expect("send");
        answer
    }

    fn set_autonomy(
        session: &Session,
        autonomy: Autonomy,
    ) -> oneshot::Receiver<Result<AutonomyState, DriverError>> {
        let (reply, answer) = oneshot::channel();
        session
            .commands
            .send(DriverCommand::SetAutonomy { autonomy, reply })
            .expect("send");
        answer
    }

    #[tokio::test]
    async fn option_and_autonomy_replies_can_be_errors() {
        let (driver, handle) = ScriptedDriver::new(Vec::new());
        let session = start(Box::new(driver));
        let refused = DriverError {
            kind: ErrorKind::AgentError,
            message: "Unknown model: gone".into(),
            detail: None,
        };
        handle.set_option_reply(Err(refused.clone()));
        assert_eq!(set_option(&session).await.expect("answered"), Err(refused));
        handle.set_option_reply(Ok(()));
        assert_eq!(set_option(&session).await.expect("answered"), Ok(()));

        let failed = DriverError {
            kind: ErrorKind::AgentError,
            message: "The agent refused the mode change.".into(),
            detail: Some("-32603".into()),
        };
        handle.set_autonomy_result(Err(failed.clone()));
        assert_eq!(
            set_autonomy(&session, Autonomy::Ask)
                .await
                .expect("answered"),
            Err(failed)
        );
        let state = AutonomyState {
            requested: Autonomy::Ask,
            native_mode: Some("default".into()),
            auto_answer: AutoAnswer::Never,
            note: None,
        };
        handle.set_autonomy_reply(state.clone());
        assert_eq!(
            set_autonomy(&session, Autonomy::Ask)
                .await
                .expect("answered"),
            Ok(state)
        );
        drop(session.commands);
        assert_eq!(session.run.await.expect("join"), DriverExit::Closed);
    }

    #[tokio::test]
    async fn turn_steps_tag_the_last_prompts_turn() {
        let request = PermissionRequest {
            request_id: "p1".into(),
            tool_call_id: None,
            title: "Run ls".into(),
            kind: Some(ToolKind::Execute),
            raw_input: None,
            content: Vec::new(),
            locations: Vec::new(),
            options: vec![PermissionOption {
                option_id: "allow".into(),
                kind: PermissionOptionKind::AllowOnce,
                label: "Allow once".into(),
            }],
            truncated: None,
        };
        let delta = O7Event::MessageDelta {
            message_id: "m1".into(),
            text: "ok".into(),
        };
        let usage = TokenUsage {
            input_tokens: 3,
            output_tokens: 1,
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            scope: UsageScope::Turn,
        };
        let (driver, _handle) = ScriptedDriver::new(vec![
            Step::Expect(CommandMatch::Prompt),
            Step::EmitInTurn(DriverEvent::Event {
                turn: None,
                event: delta.clone(),
            }),
            Step::EmitInTurn(DriverEvent::Permission {
                turn: None,
                request: request.clone(),
            }),
            Step::EmitInTurn(DriverEvent::Title(Some("Listing files".into()))),
            Step::EndTurn {
                stop_reason: StopReason::EndTurn,
                usage: Some(usage.clone()),
                error: None,
            },
        ]);
        let mut session = start(Box::new(driver));
        session
            .commands
            .send(prompt("019a-minted-by-the-actor"))
            .expect("send");
        let turn = Some("019a-minted-by-the-actor".to_owned());
        assert_eq!(
            session.events.recv().await,
            Some(DriverEvent::Event {
                turn: turn.clone(),
                event: delta
            })
        );
        assert_eq!(
            session.events.recv().await,
            Some(DriverEvent::Permission { turn, request })
        );
        assert_eq!(
            session.events.recv().await,
            Some(DriverEvent::Title(Some("Listing files".into()))),
            "an event that carries no turn goes out unchanged"
        );
        assert_eq!(
            session.events.recv().await,
            Some(DriverEvent::TurnEnded {
                turn_id: "019a-minted-by-the-actor".into(),
                stop_reason: StopReason::EndTurn,
                usage: Some(usage),
                error: None,
            })
        );
        drop(session.commands);
        assert_eq!(session.run.await.expect("join"), DriverExit::Closed);
    }

    #[tokio::test]
    async fn a_turn_step_before_any_prompt_panics_the_driver() {
        let (driver, _handle) = ScriptedDriver::new(vec![Step::EndTurn {
            stop_reason: StopReason::EndTurn,
            usage: None,
            error: None,
        }]);
        let session = start(Box::new(driver));
        assert!(session.run.await.expect_err("panicked").is_panic());
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_returns_each_matching_command_once() {
        let (driver, handle) = ScriptedDriver::new(Vec::new());
        let session = start(Box::new(driver));
        let waiting = tokio::spawn({
            let handle = handle.clone();
            async move { handle.wait_for(CommandMatch::Prompt).await }
        });
        tokio::task::yield_now().await;
        let _autonomy = set_autonomy(&session, Autonomy::Ask);
        session.commands.send(prompt("t1")).expect("send");
        session.commands.send(prompt("t2")).expect("send");
        let first = waiting.await.expect("join");
        assert_eq!(
            first,
            LoggedCommand::Prompt {
                turn_id: "t1".into(),
                parts: vec![PromptPart::Text { text: "hi".into() }]
            },
            "a command that arrives while it waits"
        );
        assert!(matches!(
            handle.wait_for(CommandMatch::Prompt).await,
            LoggedCommand::Prompt { turn_id, .. } if turn_id == "t2"
        ));
        assert_eq!(
            handle.wait_for(CommandMatch::SetAutonomy).await,
            LoggedCommand::SetAutonomy {
                autonomy: Autonomy::Ask
            },
            "an earlier command nobody claimed yet"
        );
        let again = tokio::time::timeout(
            Duration::from_millis(50),
            handle.wait_for(CommandMatch::Prompt),
        )
        .await;
        assert!(again.is_err(), "each command is returned once");
        assert_eq!(handle.commands().len(), 3, "commands() still lists all");
        drop(session.commands);
        assert_eq!(session.run.await.expect("join"), DriverExit::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn the_handle_injects_events_and_exits() {
        let (driver, handle) = ScriptedDriver::new(vec![Step::Wait(Duration::from_secs(60))]);
        let mut session = start(Box::new(driver));
        handle.emit(notice("injected"));
        assert_eq!(session.events.recv().await, Some(notice("injected")));
        handle.exit(DriverExit::Crashed {
            exit: None,
            stderr_tail: "boom".into(),
        });
        assert!(matches!(
            session.run.await.expect("join"),
            DriverExit::Crashed { .. }
        ));
    }

    #[tokio::test]
    async fn a_mismatched_command_panics_the_driver() {
        let (driver, _handle) =
            ScriptedDriver::new(vec![Step::Expect(CommandMatch::RespondPermission {
                request_id: Some("t.p1".into()),
            })]);
        let session = start(Box::new(driver));
        session
            .commands
            .send(DriverCommand::RespondPermission {
                request_id: "t.p2".into(),
                outcome: PermissionOutcome::Cancelled,
            })
            .expect("send");
        let error = session.run.await.expect_err("the driver panicked");
        assert!(error.is_panic());
    }

    #[tokio::test]
    async fn a_spawned_process_lives_until_run_returns() {
        let (driver, handle) = ScriptedDriver::new(vec![
            Step::Spawn(launch()),
            Step::Expect(CommandMatch::Interrupt),
            Step::Exit(DriverExit::Closed),
        ]);
        let mut session = start(Box::new(driver));
        let end = session.ends.recv().await.expect("spawned");
        assert_eq!(end.spec.folder, PathBuf::from("/work"));
        assert!(!*end.killed.borrow());
        session
            .commands
            .send(DriverCommand::Interrupt)
            .expect("send");
        assert_eq!(session.run.await.expect("join"), DriverExit::Closed);
        assert!(*end.killed.borrow(), "dropping the process killed it");
        assert_eq!(handle.commands(), [LoggedCommand::Interrupt]);
    }

    #[tokio::test]
    async fn a_failed_spawn_fails_the_start() {
        let (driver, _handle) = ScriptedDriver::new(vec![Step::Spawn(launch())]);
        let (spawner, _ends) = FakeSpawner::new();
        spawner.fail_next(SpawnError::ProgramNotFound {
            program: "hermes".into(),
        });
        let logs = TempLogs::new();
        let (_commands, receiver) = mpsc::unbounded_channel();
        let (sender, _events) = event_channel();
        let exit = Box::new(driver)
            .run(DriverContext {
                commands: receiver,
                events: sender,
                spawner: DirectSpawner::new(spawner, logs.factory()),
                setup: setup(),
            })
            .await;
        let DriverExit::StartFailed(error) = exit else {
            panic!("expected StartFailed, got {exit:?}");
        };
        assert_eq!(error.kind, ErrorKind::AgentUnavailable);
        assert_eq!(error.message, "hermes was not found");
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_driver_records_commands_but_answers_none() {
        let (driver, handle) = ScriptedDriver::new(vec![Step::Spawn(launch()), Step::Hang]);
        let mut session = start(Box::new(driver));
        let end = session.ends.recv().await.expect("spawned");
        let (reply, mut answer) = oneshot::channel();
        session
            .commands
            .send(DriverCommand::SetOption {
                option_id: "mode".into(),
                value: ConfigValue::Toggle { value: true },
                reply,
            })
            .expect("send");
        handle.exit(DriverExit::Closed);
        handle.emit(notice("never"));
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(!session.run.is_finished(), "a hung driver never exits");
        assert!(answer.try_recv().is_err(), "and never answers");
        assert_eq!(handle.commands().len(), 1);
        drop(session.commands);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(!session.run.is_finished(), "not even on a closed channel");
        // Only the session's force stop ends it: through the recorded control, and the abort.
        o7_process::ControlImpl::kill_tree(&*session.spawner.controls()[0]);
        assert!(*end.killed.borrow());
        session.run.abort();
        assert!(session.run.await.expect_err("aborted").is_cancelled());
    }

    #[tokio::test]
    async fn no_steps_left_means_idle_until_shutdown() {
        let (driver, handle) = ScriptedDriver::new(Vec::new());
        let session = start(Box::new(driver));
        session.commands.send(prompt("t")).expect("send");
        session
            .commands
            .send(DriverCommand::Shutdown {
                plan: StopPlan {
                    cancel_grace: Duration::ZERO,
                    process: o7_process::ShutdownPlan::QUIT,
                },
            })
            .expect("send");
        assert_eq!(session.run.await.expect("join"), DriverExit::Closed);
        assert_eq!(handle.commands().len(), 2);
    }

    #[tokio::test]
    async fn a_closed_channel_satisfies_an_expected_shutdown_and_ends_any_other_wait() {
        let (driver, _handle) = ScriptedDriver::new(vec![
            Step::Expect(CommandMatch::Shutdown),
            Step::Emit(notice("after the close")),
            Step::Expect(CommandMatch::Prompt),
            Step::Exit(DriverExit::Panicked),
        ]);
        let mut session = start(Box::new(driver));
        drop(session.commands);
        assert_eq!(session.events.recv().await, Some(notice("after the close")));
        assert_eq!(session.run.await.expect("join"), DriverExit::Closed);
    }

    /// The channel closes while an `Emit` waits for a permit (the session stopped reading): the next
    /// wait, and the idle phase after the last step, both end the run with `Closed`.
    #[tokio::test(start_paused = true)]
    async fn a_channel_closed_during_an_emit_ends_the_next_wait() {
        for steps in [
            vec![
                Step::Emit(notice("first")),
                Step::Emit(notice("second")),
                Step::Wait(Duration::from_secs(3_600)),
                Step::Exit(DriverExit::Panicked),
            ],
            vec![Step::Emit(notice("first")), Step::Emit(notice("second"))],
        ] {
            let (driver, _handle) = ScriptedDriver::new(steps);
            let (commands, receiver) = mpsc::unbounded_channel();
            let (sender, mut events) = event_channel();
            // Fill the channel so the first emit waits for a permit.
            let filler = sender.reserve(DRIVER_EVENT_BYTES).await.expect("room");
            let (spawner, _ends) = FakeSpawner::new();
            let logs = TempLogs::new();
            let run = tokio::spawn(Box::new(driver).run(DriverContext {
                commands: receiver,
                events: sender,
                spawner: DirectSpawner::new(spawner, logs.factory()),
                setup: setup(),
            }));
            tokio::time::sleep(Duration::from_millis(10)).await;
            drop(commands);
            tokio::time::sleep(Duration::from_millis(10)).await;
            drop(filler);
            assert_eq!(events.recv().await, Some(notice("first")));
            assert_eq!(events.recv().await, Some(notice("second")));
            assert_eq!(run.await.expect("join"), DriverExit::Closed);
        }
    }

    #[tokio::test]
    async fn the_factory_hands_out_drivers_in_order_then_fails_starts() {
        let (first, first_handle) = ScriptedDriver::new(vec![Step::Exit(DriverExit::Closed)]);
        let (second, _) = ScriptedDriver::new(vec![Step::Exit(DriverExit::Panicked)]);
        let factory = ScriptedDriverFactory::new(vec![first, second]);
        let run = |kind| {
            let session = start(factory.create(kind));
            async move { session.run.await.expect("join") }
        };
        assert_eq!(run(DriverKind::Mock).await, DriverExit::Closed);
        assert_eq!(run(DriverKind::Acp).await, DriverExit::Panicked);
        assert!(matches!(
            run(DriverKind::Mock).await,
            DriverExit::StartFailed(DriverError {
                kind: ErrorKind::Internal,
                ..
            })
        ));
        assert_eq!(
            factory.kinds(),
            [DriverKind::Mock, DriverKind::Acp, DriverKind::Mock]
        );
        assert!(first_handle.commands().is_empty());
    }

    #[tokio::test]
    async fn the_direct_spawner_names_each_spawn() {
        let (spawner, _ends) = FakeSpawner::new();
        let logs = TempLogs::new();
        let direct = DirectSpawner::new(spawner, logs.factory());
        let spec = SpawnSpec {
            launch: launch(),
            folder: PathBuf::from("/work"),
            env: setup().env,
            max_line_bytes: MAX_LINE_BYTES,
        };
        let first = direct.spawn(spec.clone()).await.expect("spawn");
        let second = direct.spawn(spec).await.expect("spawn");
        assert_ne!(first.spawn_id, second.spawn_id);
        let id = uuid::Uuid::parse_str(&first.spawn_id).expect("uuid");
        assert_eq!(id.get_version_num(), 7);
        assert_eq!(first.log.spawn_id(), first.spawn_id);
        // Each spawn's log is `<root>/direct/<spawn id>.jsonl`. Dropping a process kills it; once
        // both exit notes are on disk nothing else is written, so the guard's removal is final.
        let paths = [&first, &second].map(|process| {
            logs.0
                .join(DIRECT_LOG_CHAT)
                .join(format!("{}.jsonl", process.spawn_id))
        });
        drop((first, second));
        for path in paths {
            let mut written = String::new();
            for _ in 0..1000 {
                written = std::fs::read_to_string(&path).unwrap_or_default();
                if written.contains("\"exit\"") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(
                written.contains("\"spawn\""),
                "{}: {written}",
                path.display()
            );
            assert!(
                written.contains("\"exit\""),
                "{}: {written}",
                path.display()
            );
        }
    }
}
