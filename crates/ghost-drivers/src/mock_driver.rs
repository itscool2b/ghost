//! The demo agent's driver: no process; it plays `mock`'s timeline through [`play`], offers
//! synthetic options and slash commands, and answers magic prompts (`/echo`, `/permission`, `/ask`,
//! `/crash`, ...) that exercise every path of the session manager and the UI.
//!
//! | Prompt | Behaviour |
//! |---|---|
//! | `/echo <text>` | one message with `<text>`, `EndTurn` with tiny usage |
//! | `/permission` | an `execute` tool call and a permission request (Allow once, Reject once); allow: tool completed, "Done."; reject: tool failed, "Skipped." |
//! | `/withdraw` | a permission request withdrawn after 1 s |
//! | `/ask` | a question, `Select { "Which color?", [Red, Green, Blue] }`; an answer: "You picked Green."; dismissed: "No answer." (`/ask withdraw`: withdrawn after 1 s) |
//! | `/background` | ends the turn, then 500 ms later an untagged tool update for the turn's tool and an untagged permission request |
//! | `/slow` | a long answer at 20 tokens/s |
//! | `/hang` | streams a little, never ends |
//! | `/ignore-cancel` | ignores `Interrupt` |
//! | `/fail` | `TurnEnded { Error, AgentError }` |
//! | `/crash` | `run` returns `Crashed` mid-turn with a scripted stderr tail |
//! | `/auth` | `AuthRequired`, then `TurnEnded { Error }` with `AuthRequired` |
//! | `/big` | a 200 KB tool output |
//! | `/stress <n>` | [`MockScript::stress`] with n tokens |
//! | anything else | [`MockScript::demo_turn`] |
//!
//! Each turn runs as its own task that sends what it emits to the driver's loop, which hands it on
//! with permits; adjacent deltas of one message merge while they wait, so a slow session costs
//! the script's text and no more.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use ghost_protocol::session::{
    AuthCommand, AuthPrompt, ConfigCategory, ConfigChoice, ConfigKind, ConfigOption, ConfigValue,
};
use ghost_protocol::{
    AutoAnswer, Autonomy, AutonomyState, Capabilities, ErrorKind, GhostEvent, NoticeLevel,
    PermissionOption, PermissionOptionKind, PermissionRequest, PromptPart, SlashCommand,
    StopReason, TokenUsage, ToolCall, ToolContent, ToolKind, ToolStatus, ToolUpdate, UiAnswer,
    UiPrompt, UiRequest, UsageScope,
};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::acp::dialect::EDIT_KINDS;
use crate::driver::{
    AgentDriver, DriverCommand, DriverContext, DriverError, DriverEvent, DriverExit, DriverStatus,
    EventSender, PermissionOutcome, ReadyInfo, ResumeOutcome, SessionSetup,
};
use crate::mock::{MockScript, MockStep, RunOutcome, play};

/// How the demo agent behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockBehavior {
    pub tokens_per_sec: u32,
    pub startup_delay: Duration,
    /// The start fails with this error after the delay.
    pub fail_start: Option<DriverError>,
}

impl Default for MockBehavior {
    /// 120 tokens/s, 300 ms, no failure.
    fn default() -> MockBehavior {
        MockBehavior {
            tokens_per_sec: 120,
            startup_delay: Duration::from_millis(300),
            fail_start: None,
        }
    }
}

/// The demo agent's driver.
#[derive(Debug, Clone)]
pub struct MockDriver {
    behavior: MockBehavior,
}

impl MockDriver {
    pub fn new(behavior: MockBehavior) -> MockDriver {
        MockDriver { behavior }
    }
}

impl AgentDriver for MockDriver {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        Mock::new(self.behavior, ctx).run().boxed()
    }
}

/// The rate of `/slow`.
pub const SLOW_TOKENS_PER_SEC: u32 = 20;

/// The size of `/big`'s tool output.
pub const BIG_OUTPUT_BYTES: usize = 200 * 1024;

/// What `/fail` says.
pub const FAIL_MESSAGE: &str = "The demo agent failed this turn on purpose.";

/// The stderr tail `/crash` leaves.
pub const CRASH_TAIL: &str = "thread 'main' panicked at src/agent.rs:42:5:\nthe demo agent crashed on purpose\nnote: run with `RUST_BACKTRACE=1` to display a backtrace\n";

/// The `/ask` choices.
pub const COLORS: [&str; 3] = ["Red", "Green", "Blue"];

/// The demo agent's slash commands: its magic prompts first, then ordinary commands that play the
/// demo turn.
pub const COMMANDS: [(&str, &str, Option<&str>); 40] = [
    (
        "echo",
        "Reply with exactly the text you type",
        Some("text to echo"),
    ),
    ("permission", "Ask before running a command", None),
    (
        "withdraw",
        "Ask, then withdraw the request after a second",
        None,
    ),
    (
        "ask",
        "Ask which color you like",
        Some("withdraw, to withdraw it after a second"),
    ),
    (
        "background",
        "End the turn, then report from the background",
        None,
    ),
    ("slow", "Answer slowly, 20 tokens a second", None),
    ("hang", "Start answering and never finish", None),
    ("ignore-cancel", "Keep going when asked to stop", None),
    ("fail", "Fail the turn on purpose", None),
    ("crash", "Crash the demo agent mid-turn", None),
    ("auth", "Pretend the demo agent needs a sign-in", None),
    ("big", "Show a 200 KB tool output", None),
    ("stress", "Stream a long answer", Some("number of tokens")),
    ("help", "Show what the demo agent can do", None),
    ("clear", "Start over with an empty context", None),
    (
        "compact",
        "Summarize the conversation to save context",
        Some("what to keep"),
    ),
    ("init", "Write a project guide for this folder", None),
    (
        "review",
        "Review the current changes",
        Some("branch or commit"),
    ),
    (
        "test",
        "Run the tests and explain failures",
        Some("test name"),
    ),
    ("lint", "Run the linters", None),
    ("format", "Format the changed files", None),
    ("build", "Build the project", None),
    (
        "explain",
        "Explain a file or a symbol",
        Some("file or symbol"),
    ),
    ("summarize", "Summarize what changed today", None),
    (
        "refactor",
        "Refactor a piece of code",
        Some("what to refactor"),
    ),
    ("docs", "Write documentation", Some("what to document")),
    ("commit", "Write a commit message", None),
    (
        "branch",
        "Create a branch for this work",
        Some("branch name"),
    ),
    ("diff", "Show the current diff", None),
    ("status", "Show the session status", None),
    ("todo", "List the open tasks", None),
    ("plan", "Plan before changing anything", Some("goal")),
    ("search", "Search the codebase", Some("query")),
    ("open", "Open a file", Some("path")),
    ("rename", "Rename a symbol everywhere", Some("old new")),
    (
        "translate",
        "Translate the user-facing text",
        Some("language"),
    ),
    ("benchmark", "Run the benchmarks", None),
    ("profile", "Profile a slow path", Some("what is slow")),
    ("release", "Prepare a release", Some("version")),
    (
        "model",
        "Show or switch the model",
        Some("mock-fast or mock-smart"),
    ),
];

/// The mock dialect's autonomy candidates.
fn candidate(requested: Autonomy) -> (&'static str, AutoAnswer) {
    match requested {
        Autonomy::FullAuto => ("full_auto", AutoAnswer::All),
        Autonomy::AutoEdit => (
            "auto_edit",
            AutoAnswer::Kinds {
                kinds: EDIT_KINDS.to_vec(),
            },
        ),
        Autonomy::Ask => ("ask", AutoAnswer::Never),
    }
}

fn select(
    id: &str,
    name: &str,
    category: ConfigCategory,
    current: &str,
    choices: &[(&str, &str)],
) -> ConfigOption {
    ConfigOption {
        id: id.into(),
        name: name.into(),
        description: None,
        category,
        kind: ConfigKind::Select {
            current: current.into(),
            choices: choices
                .iter()
                .map(|(value, name)| ConfigChoice {
                    value: (*value).into(),
                    name: (*name).into(),
                    description: None,
                    group: None,
                })
                .collect(),
        },
        pending: None,
    }
}

/// Model, Effort, Mode and a Fast toggle.
fn synthetic_options(mode: &str) -> Vec<ConfigOption> {
    vec![
        select(
            "model",
            "Model",
            ConfigCategory::Model,
            "mock-fast",
            &[("mock-fast", "Mock Fast"), ("mock-smart", "Mock Smart")],
        ),
        select(
            "effort",
            "Effort",
            ConfigCategory::Effort,
            "medium",
            &[("low", "Low"), ("medium", "Medium"), ("high", "High")],
        ),
        select(
            "mode",
            "Mode",
            ConfigCategory::Mode,
            mode,
            &[
                ("full_auto", "Full auto"),
                ("auto_edit", "Auto-edit"),
                ("ask", "Ask"),
            ],
        ),
        ConfigOption {
            id: "fast".into(),
            name: "Fast mode".into(),
            description: Some("Shorter answers, sooner".into()),
            category: ConfigCategory::ModelConfig,
            kind: ConfigKind::Toggle { current: false },
            pending: None,
        },
    ]
}

fn slash_commands() -> Vec<SlashCommand> {
    COMMANDS
        .iter()
        .map(|(name, description, hint)| SlashCommand {
            name: (*name).into(),
            description: (*description).into(),
            input_hint: hint.map(str::to_owned),
        })
        .collect()
}

/// What a turn task tells the driver's loop.
enum Msg {
    Event(Box<DriverEvent>),
    /// A permission request or question waits for its answer.
    Wait {
        request_id: String,
        turn: Option<String>,
        answer: oneshot::Sender<Answer>,
    },
    /// Withdrawn: the answer will not be read.
    Unwait {
        request_id: String,
    },
    TurnDone {
        turn_id: String,
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
        error: Option<DriverError>,
    },
    Crash,
}

impl Msg {
    fn event(event: DriverEvent) -> Self {
        Self::Event(Box::new(event))
    }
}

enum Answer {
    Permission(PermissionOutcome),
    Ui(Option<UiAnswer>),
}

struct Waiter {
    request_id: String,
    turn: Option<String>,
    answer: oneshot::Sender<Answer>,
}

struct ActiveTurn {
    turn_id: String,
    cancel: CancellationToken,
    ignore_interrupt: bool,
}

/// How `run` is ending.
struct Ending {
    exit: DriverExit,
    /// Waiting for the running turn's end until then (a stop's cancel grace).
    until: Option<Instant>,
}

struct Mock {
    behavior: MockBehavior,
    commands: mpsc::UnboundedReceiver<DriverCommand>,
    commands_open: bool,
    events: EventSender,
    events_open: bool,
    outbox: VecDeque<DriverEvent>,
    setup: SessionSetup,
    spawn_id: String,
    options: Vec<ConfigOption>,
    requested: Autonomy,
    hand_picked: Option<String>,
    ready: bool,
    held: VecDeque<DriverCommand>,
    turn: Option<ActiveTurn>,
    msgs: mpsc::UnboundedReceiver<Msg>,
    msgs_tx: mpsc::UnboundedSender<Msg>,
    waiters: Vec<Waiter>,
    tasks: Vec<JoinHandle<()>>,
    /// Counts permission requests outside any turn.
    untagged: Arc<AtomicU32>,
    ending: Option<Ending>,
}

impl Mock {
    fn new(behavior: MockBehavior, ctx: DriverContext) -> Mock {
        let DriverContext {
            commands,
            events,
            setup,
            ..
        } = ctx;
        let (msgs_tx, msgs) = mpsc::unbounded_channel();
        let requested = setup.autonomy;
        Mock {
            behavior,
            commands,
            commands_open: true,
            events,
            events_open: true,
            outbox: VecDeque::new(),
            setup,
            spawn_id: uuid::Uuid::now_v7().hyphenated().to_string(),
            options: synthetic_options(candidate(requested).0),
            requested,
            hand_picked: None,
            ready: false,
            held: VecDeque::new(),
            turn: None,
            msgs,
            msgs_tx,
            waiters: Vec::new(),
            tasks: Vec::new(),
            untagged: Arc::new(AtomicU32::new(0)),
            ending: None,
        }
    }

    async fn run(mut self) -> DriverExit {
        self.push(DriverEvent::Status(DriverStatus::Initializing));
        let started = Instant::now() + self.behavior.startup_delay;
        let mut start_pending = true;
        loop {
            if start_pending && self.ending.is_none() && Instant::now() >= started {
                start_pending = false;
                if let Some(error) = self.behavior.fail_start.clone() {
                    self.ending = Some(Ending {
                        exit: DriverExit::StartFailed(error),
                        until: None,
                    });
                } else {
                    self.become_ready();
                }
            }
            if let Some(ending) = &self.ending
                && (self.turn.is_none() || ending.until.is_none_or(|until| Instant::now() >= until))
            {
                return self.finish().await;
            }
            let wake = if start_pending {
                Some(started)
            } else {
                self.ending.as_ref().and_then(|ending| ending.until)
            };
            let head = self.outbox.front().map_or(0, DriverEvent::approx_bytes);
            tokio::select! {
                biased;
                command = self.commands.recv(), if self.commands_open && self.ending.is_none() => {
                    self.on_command(command);
                }
                permit = self.events.reserve(head), if !self.outbox.is_empty() => match permit {
                    Ok(permit) => {
                        if let Some(event) = self.outbox.pop_front() {
                            permit.send(event);
                        }
                    }
                    Err(_) => {
                        self.events_open = false;
                        self.outbox.clear();
                    }
                },
                msg = self.msgs.recv() => {
                    if let Some(msg) = msg {
                        self.on_msg(msg);
                    }
                }
                () = sleep_until(wake), if wake.is_some() => {}
            }
        }
    }

    /// Stops every task, forgets every open request (rule 3: the demo agent answers its own) and
    /// hands over what is left.
    async fn finish(mut self) -> DriverExit {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        self.waiters.clear();
        let exit = self
            .ending
            .take()
            .map_or(DriverExit::Closed, |ending| ending.exit);
        while let Some(event) = self.outbox.pop_front() {
            if !self.events_open {
                break;
            }
            match self.events.reserve(event.approx_bytes()).await {
                Ok(permit) => permit.send(event),
                Err(_) => break,
            }
        }
        exit
    }

    fn become_ready(&mut self) {
        self.push(DriverEvent::Status(DriverStatus::Configuring));
        for (option_id, value) in self.setup.desired.clone() {
            match self.validate(&option_id, &value) {
                Ok(()) => self.set_current(&option_id, &value),
                Err(_) => {
                    let notice = format!(
                        "The setting \"{option_id}\" you chose earlier isn't offered anymore."
                    );
                    self.push_event(
                        None,
                        GhostEvent::Notice {
                            level: NoticeLevel::Info,
                            message: notice,
                        },
                    );
                }
            }
        }
        self.push(DriverEvent::Options(self.options.clone()));
        self.push(DriverEvent::Commands(slash_commands()));
        let state = self.autonomy_state();
        self.push(DriverEvent::Autonomy(state));
        if let Some(mode) = self.current_mode() {
            self.check_hand_picked(&mode);
        }
        self.push(DriverEvent::Status(DriverStatus::Ready));
        let info = ReadyInfo {
            native_session_id: Some(format!("mock-{}", uuid::Uuid::now_v7().hyphenated())),
            agent_name: Some("Demo agent".into()),
            agent_version: Some(self.setup.client_version.clone()),
            capabilities: Capabilities {
                images: true,
                steer: false,
                interrupt: true,
                resume: false,
                fork: false,
                rewind: false,
                set_model: true,
                set_effort: true,
                permissions: true,
                plans: true,
                usage: true,
                limits: false,
                slash_commands: true,
                add_dirs: false,
                plan_edits: false,
                autonomy_levels: vec![Autonomy::FullAuto, Autonomy::AutoEdit, Autonomy::Ask],
            },
            // A resume quietly starts new: the demo agent remembers nothing.
            resumed: ResumeOutcome::New,
            spawn_id: self.spawn_id.clone(),
            replay: None,
        };
        self.push(DriverEvent::Ready(info));
        self.ready = true;
        while let Some(command) = self.held.pop_front() {
            self.on_command(Some(command));
        }
    }

    fn on_command(&mut self, command: Option<DriverCommand>) {
        let Some(command) = command else {
            self.commands_open = false;
            self.begin_end(Duration::ZERO);
            return;
        };
        match command {
            DriverCommand::McpAction { reply, .. } => {
                crate::mcp::unsupported(reply, "The demo agent does not connect MCP servers.")
            }
            DriverCommand::Shutdown { plan } => self.begin_end(plan.cancel_grace),
            DriverCommand::Prompt { turn, parts } => {
                if self.ready {
                    self.start_turn(turn.turn_id, &parts);
                } else {
                    self.push(turn_error(
                        turn.turn_id,
                        internal("The demo agent isn't ready yet."),
                    ));
                }
            }
            DriverCommand::Interrupt => {
                if let Some(turn) = &self.turn
                    && !turn.ignore_interrupt
                {
                    turn.cancel.cancel();
                }
            }
            DriverCommand::EditPlan { .. } => {}
            DriverCommand::RespondPermission {
                request_id,
                outcome,
            } => {
                self.answer(&request_id, Answer::Permission(outcome));
            }
            DriverCommand::RespondUi { request_id, answer } => {
                self.answer(&request_id, Answer::Ui(answer));
            }
            DriverCommand::SetOption {
                option_id,
                value,
                reply,
            } => {
                if !self.ready {
                    self.held.push_back(DriverCommand::SetOption {
                        option_id,
                        value,
                        reply,
                    });
                    return;
                }
                let result = self.set_option(&option_id, &value);
                let _ = reply.send(result);
            }
            DriverCommand::SetAutonomy { autonomy, reply } => {
                if !self.ready {
                    self.held
                        .push_back(DriverCommand::SetAutonomy { autonomy, reply });
                    return;
                }
                self.requested = autonomy;
                self.hand_picked = None;
                let mode = candidate(autonomy).0;
                self.set_current("mode", &ConfigValue::Choice { value: mode.into() });
                self.push(DriverEvent::Options(self.options.clone()));
                let _ = reply.send(Ok(self.autonomy_state()));
            }
            DriverCommand::Steer { reply, .. } => {
                let _ = reply.send(crate::driver::SteerResult::Rejected {
                    message: "This agent cannot steer an exact running turn.".into(),
                });
            }
            DriverCommand::RewindFiles { reply, .. } => {
                let _ = reply.send(Err(DriverError {
                    kind: ErrorKind::ProtocolError,
                    message: "The demo agent does not rewind files.".into(),
                    detail: None,
                }));
            }
        }
    }

    /// A stop: the running turn is cancelled (unless it ignores that) and gets `grace` to end.
    fn begin_end(&mut self, grace: Duration) {
        if self.ending.is_some() {
            return;
        }
        if let Some(turn) = &self.turn
            && !turn.ignore_interrupt
        {
            turn.cancel.cancel();
        }
        self.ending = Some(Ending {
            exit: DriverExit::Closed,
            until: (!grace.is_zero()).then(|| Instant::now() + grace),
        });
        if grace.is_zero() {
            // Nothing is awaited: the session ends whatever is still open.
            self.turn = None;
        }
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Event(event) => self.push(*event),
            Msg::Wait {
                request_id,
                turn,
                answer,
            } => self.waiters.push(Waiter {
                request_id,
                turn,
                answer,
            }),
            Msg::Unwait { request_id } => {
                self.waiters
                    .retain(|waiter| waiter.request_id != request_id);
            }
            Msg::TurnDone {
                turn_id,
                stop_reason,
                usage,
                error,
            } => {
                self.waiters
                    .retain(|waiter| waiter.turn.as_deref() != Some(turn_id.as_str()));
                if self
                    .turn
                    .as_ref()
                    .is_some_and(|turn| turn.turn_id == turn_id)
                {
                    self.turn = None;
                }
                self.push(DriverEvent::TurnEnded {
                    turn_id,
                    stop_reason,
                    usage,
                    error,
                });
                if self.ending.is_none() {
                    self.push(DriverEvent::Status(DriverStatus::Ready));
                }
            }
            Msg::Crash => {
                self.turn = None;
                self.ending = Some(Ending {
                    exit: DriverExit::Crashed {
                        exit: Some(ghost_process::ExitInfo {
                            code: Some(101),
                            signal: None,
                            at_ms: now_millis(),
                        }),
                        stderr_tail: CRASH_TAIL.into(),
                    },
                    until: None,
                });
            }
        }
    }

    fn answer(&mut self, request_id: &str, answer: Answer) {
        match self
            .waiters
            .iter()
            .position(|waiter| waiter.request_id == request_id)
        {
            Some(index) => {
                let waiter = self.waiters.remove(index);
                let _ = waiter.answer.send(answer);
            }
            None => tracing::debug!(
                request_id,
                "an answer for a request the demo agent no longer waits on"
            ),
        }
    }

    fn start_turn(&mut self, turn_id: String, parts: &[PromptPart]) {
        if self.turn.is_some() {
            self.push(turn_error(turn_id, internal("A turn is already running.")));
            return;
        }
        let mut text = String::new();
        for part in parts {
            match part {
                PromptPart::Text { text: part } => text.push_str(part),
                // The demo executes its text commands unchanged. Opaque images let native smoke
                // exercise attachment delivery and persistence without a paid model turn.
                PromptPart::Image { .. } => {}
                PromptPart::Mention { .. } => {
                    self.push(turn_error(
                        turn_id,
                        internal("The demo agent does not resolve file mentions."),
                    ));
                    return;
                }
            }
        }
        let plan = TurnPlan::parse(&text);
        let cancel = CancellationToken::new();
        self.turn = Some(ActiveTurn {
            turn_id: turn_id.clone(),
            cancel: cancel.clone(),
            ignore_interrupt: plan == TurnPlan::IgnoreCancel,
        });
        self.push(DriverEvent::Status(DriverStatus::Prompting));
        let task = TurnTask {
            turn_id,
            spawn_id: self.spawn_id.clone(),
            rate: self.behavior.tokens_per_sec,
            tx: self.msgs_tx.clone(),
            cancel,
            untagged: Arc::clone(&self.untagged),
        };
        self.tasks.push(tokio::spawn(task.run(plan)));
        self.tasks.retain(|task| !task.is_finished());
    }

    fn validate(&self, option_id: &str, value: &ConfigValue) -> Result<(), DriverError> {
        let invalid = |message: String| DriverError {
            kind: ErrorKind::Other,
            message,
            detail: None,
        };
        let Some(option) = self.options.iter().find(|option| option.id == option_id) else {
            return Err(invalid(format!(
                "The demo agent has no option \"{option_id}\"."
            )));
        };
        match (&option.kind, value) {
            (ConfigKind::Select { choices, .. }, ConfigValue::Choice { value })
                if choices.iter().any(|choice| choice.value == *value) =>
            {
                Ok(())
            }
            (ConfigKind::Toggle { .. }, ConfigValue::Toggle { .. }) => Ok(()),
            _ => Err(invalid(format!(
                "That value isn't offered for {}.",
                option.name
            ))),
        }
    }

    fn set_option(&mut self, option_id: &str, value: &ConfigValue) -> Result<(), DriverError> {
        self.validate(option_id, value)?;
        self.set_current(option_id, value);
        self.push(DriverEvent::Options(self.options.clone()));
        if option_id == "mode"
            && let ConfigValue::Choice { value } = value
        {
            self.check_hand_picked(value);
        }
        Ok(())
    }

    fn set_current(&mut self, option_id: &str, value: &ConfigValue) {
        if let Some(option) = self
            .options
            .iter_mut()
            .find(|option| option.id == option_id)
        {
            match (&mut option.kind, value) {
                (ConfigKind::Select { current, .. }, ConfigValue::Choice { value }) => {
                    current.clone_from(value);
                }
                (ConfigKind::Toggle { current }, ConfigValue::Toggle { value }) => {
                    *current = *value
                }
                _ => {}
            }
        }
    }

    fn current_mode(&self) -> Option<String> {
        self.options
            .iter()
            .find(|option| option.id == "mode")
            .and_then(|option| match &option.kind {
                ConfigKind::Select { current, .. } => Some(current.clone()),
                ConfigKind::Toggle { .. } => None,
            })
    }

    fn autonomy_state(&self) -> AutonomyState {
        let (mode, auto_answer) = candidate(self.requested);
        AutonomyState {
            requested: self.requested,
            native_mode: Some(mode.into()),
            auto_answer,
            note: None,
        }
    }

    /// The same rule as ACP agents: a mode picked by hand that differs from the autonomy level's
    /// mode turns auto-answers off; its own mode again restores them.
    fn check_hand_picked(&mut self, mode: &str) {
        let (candidate_mode, _) = candidate(self.requested);
        if mode != candidate_mode {
            if self.hand_picked.as_deref() == Some(mode) {
                return;
            }
            self.hand_picked = Some(mode.to_owned());
            let name = match mode {
                "full_auto" => "Full auto",
                "auto_edit" => "Auto-edit",
                "ask" => "Ask",
                other => other,
            };
            self.push(DriverEvent::Autonomy(AutonomyState {
                requested: self.requested,
                native_mode: Some(mode.to_owned()),
                auto_answer: AutoAnswer::Never,
                note: Some(format!(
                    "You picked the {name} mode by hand, so Ghost asks before every action. Choose an autonomy level to hand control back."
                )),
            }));
        } else if self.hand_picked.take().is_some() {
            let state = self.autonomy_state();
            self.push(DriverEvent::Autonomy(state));
        }
    }

    fn push_event(&mut self, turn: Option<String>, event: GhostEvent) {
        self.push(DriverEvent::Event { turn, event });
    }

    /// Queues an event; a delta right after a delta of the same message and turn merges into it.
    fn push(&mut self, event: DriverEvent) {
        if !self.events_open {
            return;
        }
        if let (
            Some(DriverEvent::Event {
                turn: last_turn,
                event: last,
            }),
            DriverEvent::Event { turn, event: next },
        ) = (self.outbox.back_mut(), &event)
            && last_turn == turn
        {
            match (last, next) {
                (
                    GhostEvent::MessageDelta {
                        message_id: a,
                        text: so_far,
                    },
                    GhostEvent::MessageDelta {
                        message_id: b,
                        text,
                    },
                )
                | (
                    GhostEvent::ThoughtDelta {
                        thought_id: a,
                        text: so_far,
                    },
                    GhostEvent::ThoughtDelta {
                        thought_id: b,
                        text,
                    },
                ) if a == b => {
                    so_far.push_str(text);
                    return;
                }
                _ => {}
            }
        }
        self.outbox.push_back(event);
    }
}

fn internal(message: &str) -> DriverError {
    DriverError {
        kind: ErrorKind::Internal,
        message: message.into(),
        detail: None,
    }
}

fn turn_error(turn_id: String, error: DriverError) -> DriverEvent {
    DriverEvent::TurnEnded {
        turn_id,
        stop_reason: StopReason::Error,
        usage: None,
        error: Some(error),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// What a prompt asks the demo agent for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TurnPlan {
    Echo(String),
    Permission,
    Withdraw,
    Ask { withdraw: bool },
    Background,
    Slow,
    Hang,
    IgnoreCancel,
    Fail,
    Crash,
    Auth,
    Big,
    Stress(u32),
    Demo,
}

impl TurnPlan {
    fn parse(text: &str) -> TurnPlan {
        let trimmed = text.trim();
        let (command, rest) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, ""), |(command, rest)| (command, rest.trim()));
        match command {
            "/echo" => TurnPlan::Echo(rest.to_owned()),
            "/permission" => TurnPlan::Permission,
            "/withdraw" => TurnPlan::Withdraw,
            "/ask" => TurnPlan::Ask {
                withdraw: rest == "withdraw",
            },
            "/background" => TurnPlan::Background,
            "/slow" => TurnPlan::Slow,
            "/hang" => TurnPlan::Hang,
            "/ignore-cancel" => TurnPlan::IgnoreCancel,
            "/fail" => TurnPlan::Fail,
            "/crash" => TurnPlan::Crash,
            "/auth" => TurnPlan::Auth,
            "/big" => TurnPlan::Big,
            "/stress" => TurnPlan::Stress(rest.parse().unwrap_or(2_000).min(1_000_000)),
            _ => TurnPlan::Demo,
        }
    }
}

/// One turn, on its own task.
struct TurnTask {
    turn_id: String,
    spawn_id: String,
    rate: u32,
    tx: mpsc::UnboundedSender<Msg>,
    cancel: CancellationToken,
    untagged: Arc<AtomicU32>,
}

impl TurnTask {
    async fn run(self, plan: TurnPlan) {
        match plan {
            TurnPlan::Echo(text) => {
                if !text.is_empty() {
                    self.say(&text);
                }
                let words = u64::try_from(text.split_whitespace().count()).unwrap_or(u64::MAX);
                self.done(StopReason::EndTurn, Some(tiny_usage(words)), None);
            }
            TurnPlan::Permission => self.permission().await,
            TurnPlan::Withdraw => self.withdraw().await,
            TurnPlan::Ask { withdraw } => self.ask(withdraw).await,
            TurnPlan::Background => self.background().await,
            TurnPlan::Slow => {
                let script = MockScript {
                    steps: vec![
                        MockStep::StreamText {
                            message_id: self.message_id(),
                            text: SLOW_ANSWER.into(),
                            tokens_per_sec: SLOW_TOKENS_PER_SEC,
                        },
                        MockStep::Emit(GhostEvent::MessageCompleted {
                            message_id: self.message_id(),
                        }),
                    ],
                };
                self.play_then_end(script).await;
            }
            TurnPlan::Hang | TurnPlan::IgnoreCancel => {
                let script = MockScript {
                    steps: vec![MockStep::StreamText {
                        message_id: self.message_id(),
                        text: "Working on it, this may take a while... ".into(),
                        tokens_per_sec: self.rate,
                    }],
                };
                let outcome = self.play(script).await.0;
                if outcome == RunOutcome::ReceiverGone {
                    return;
                }
                // `/ignore-cancel` is never cancelled; `/hang` ends only when it is.
                self.cancel.cancelled().await;
                self.done(StopReason::Cancelled, None, None);
            }
            TurnPlan::Fail => self.done(
                StopReason::Error,
                None,
                Some(DriverError {
                    kind: ErrorKind::AgentError,
                    message: FAIL_MESSAGE.into(),
                    detail: None,
                }),
            ),
            TurnPlan::Crash => {
                self.say("Working on it...");
                let _ = self.tx.send(Msg::Crash);
            }
            TurnPlan::Auth => {
                let prompt = AuthPrompt {
                    message: "The demo agent needs you to sign in. Run the command below in a terminal, then press Retry.".into(),
                    commands: vec![AuthCommand {
                        label: "Sign in to the demo agent".into(),
                        argv: vec!["demo-agent".into(), "login".into()],
                        env: Vec::new(),
                    }],
                };
                let _ = self.tx.send(Msg::event(DriverEvent::AuthRequired(prompt)));
                self.done(
                    StopReason::Error,
                    None,
                    Some(DriverError {
                        kind: ErrorKind::AuthRequired,
                        message: "The demo agent needs you to sign in.".into(),
                        detail: None,
                    }),
                );
            }
            TurnPlan::Big => {
                let tool = format!("{}.big", self.turn_id);
                self.emit(GhostEvent::ToolCall(tool_call(
                    &tool,
                    ToolKind::Read,
                    "Read `big.log`",
                    ToolStatus::InProgress,
                    json!({"path": "big.log"}),
                )));
                self.emit(GhostEvent::ToolUpdate(tool_update(
                    &tool,
                    ToolStatus::Completed,
                    big_output(),
                )));
                self.say("That log is 200 KB; the full output is kept.");
                self.done(StopReason::EndTurn, Some(tiny_usage(12)), None);
            }
            TurnPlan::Stress(tokens) => {
                self.play_then_end(MockScript::stress(&self.turn_id, self.rate, tokens))
                    .await;
            }
            TurnPlan::Demo => {
                self.play_then_end(MockScript::demo_turn(&self.turn_id, self.rate))
                    .await;
            }
        }
    }

    fn message_id(&self) -> String {
        format!("{}.m1", self.turn_id)
    }

    fn emit(&self, event: GhostEvent) {
        let _ = self.tx.send(Msg::event(DriverEvent::Event {
            turn: Some(self.turn_id.clone()),
            event,
        }));
    }

    /// One whole message.
    fn say(&self, text: &str) {
        let message_id = self.message_id();
        self.emit(GhostEvent::MessageDelta {
            message_id: message_id.clone(),
            text: text.into(),
        });
        self.emit(GhostEvent::MessageCompleted { message_id });
    }

    fn done(&self, stop_reason: StopReason, usage: Option<TokenUsage>, error: Option<DriverError>) {
        let _ = self.tx.send(Msg::TurnDone {
            turn_id: self.turn_id.clone(),
            stop_reason,
            usage,
            error,
        });
    }

    /// Plays `script`: the script's `TurnStarted` and `SessionUpdated` are dropped and its
    /// `TurnEnded` comes back instead of going out.
    async fn play(
        &self,
        script: MockScript,
    ) -> (RunOutcome, Option<(StopReason, Option<TokenUsage>)>) {
        let mut ended = None;
        let tx = self.tx.clone();
        let turn = Some(self.turn_id.clone());
        let outcome = play(script, self.cancel.clone(), |event| match event {
            GhostEvent::TurnEnded {
                stop_reason, usage, ..
            } => {
                ended = Some((stop_reason, usage));
                true
            }
            GhostEvent::TurnStarted { .. } | GhostEvent::SessionUpdated(_) => true,
            event => tx
                .send(Msg::event(DriverEvent::Event {
                    turn: turn.clone(),
                    event,
                }))
                .is_ok(),
        })
        .await;
        (outcome, ended)
    }

    async fn play_then_end(&self, script: MockScript) {
        match self.play(script).await {
            (RunOutcome::Completed, Some((stop_reason, usage))) => {
                self.done(stop_reason, usage, None)
            }
            (RunOutcome::Completed, None) => {
                self.done(StopReason::EndTurn, Some(tiny_usage(40)), None)
            }
            (RunOutcome::Cancelled, _) => self.done(StopReason::Cancelled, None, None),
            (RunOutcome::ReceiverGone, _) => {}
        }
    }

    /// Asks, and waits for the answer or the turn's cancellation (`None`).
    async fn wait(
        &self,
        request_id: &str,
        turn: Option<String>,
        event: DriverEvent,
        cancellable: bool,
    ) -> Option<Answer> {
        let (answer, answered) = oneshot::channel();
        let _ = self.tx.send(Msg::Wait {
            request_id: request_id.into(),
            turn,
            answer,
        });
        let _ = self.tx.send(Msg::event(event));
        if cancellable {
            tokio::select! {
                answer = answered => answer.ok(),
                () = self.cancel.cancelled() => None,
            }
        } else {
            answered.await.ok()
        }
    }

    async fn permission(&self) {
        let tool = format!("{}.run", self.turn_id);
        let title = "Run `ls -la`";
        self.emit(GhostEvent::ToolCall(tool_call(
            &tool,
            ToolKind::Execute,
            title,
            ToolStatus::Pending,
            json!({"command": "ls -la"}),
        )));
        let request = PermissionRequest {
            request_id: format!("{}.p1", self.turn_id),
            tool_call_id: Some(tool.clone()),
            title: title.into(),
            kind: Some(ToolKind::Execute),
            raw_input: Some(json!({"command": "ls -la"})),
            content: Vec::new(),
            locations: Vec::new(),
            options: allow_or_reject(),
            truncated: None,
        };
        let event = DriverEvent::Permission {
            turn: Some(self.turn_id.clone()),
            request,
        };
        let request_id = format!("{}.p1", self.turn_id);
        match self
            .wait(&request_id, Some(self.turn_id.clone()), event, true)
            .await
        {
            Some(Answer::Permission(PermissionOutcome::Selected(option))) if option == "allow" => {
                self.emit(GhostEvent::ToolUpdate(tool_update(
                    &tool,
                    ToolStatus::Completed,
                    "total 0\ndrwxr-xr-x  3 demo  staff  96 src\n".into(),
                )));
                self.say("Done.");
                self.done(StopReason::EndTurn, Some(tiny_usage(2)), None);
            }
            Some(_) => {
                self.emit(GhostEvent::ToolUpdate(tool_update(
                    &tool,
                    ToolStatus::Failed,
                    String::new(),
                )));
                self.say("Skipped.");
                self.done(StopReason::EndTurn, Some(tiny_usage(2)), None);
            }
            None => self.done(StopReason::Cancelled, None, None),
        }
    }

    async fn withdraw(&self) {
        let request_id = format!("{}.p1", self.turn_id);
        let request = PermissionRequest {
            request_id: request_id.clone(),
            tool_call_id: None,
            title: "Continue with the risky step?".into(),
            kind: Some(ToolKind::Other),
            raw_input: None,
            content: vec![ToolContent::Text {
                text: "The demo agent withdraws this request after a second.".into(),
                truncated: None,
            }],
            locations: Vec::new(),
            options: allow_or_reject(),
            truncated: None,
        };
        let (answer, answered) = oneshot::channel();
        let _ = self.tx.send(Msg::Wait {
            request_id: request_id.clone(),
            turn: Some(self.turn_id.clone()),
            answer,
        });
        let _ = self.tx.send(Msg::event(DriverEvent::Permission {
            turn: Some(self.turn_id.clone()),
            request,
        }));
        tokio::select! {
            answer = answered => {
                if answer.is_ok() {
                    self.say("You answered before the demo agent withdrew its request.");
                    self.done(StopReason::EndTurn, Some(tiny_usage(8)), None);
                } else {
                    self.done(StopReason::Cancelled, None, None);
                }
            }
            () = tokio::time::sleep(Duration::from_secs(1)) => {
                let _ = self.tx.send(Msg::Unwait { request_id: request_id.clone() });
                let _ = self.tx.send(Msg::event(DriverEvent::PermissionWithdrawn { request_id }));
                self.say("The demo agent withdrew its request.");
                self.done(StopReason::EndTurn, Some(tiny_usage(6)), None);
            }
            () = self.cancel.cancelled() => self.done(StopReason::Cancelled, None, None),
        }
    }

    async fn ask(&self, withdraw: bool) {
        let request_id = format!("{}.p1", self.turn_id);
        let request = UiRequest {
            request_id: request_id.clone(),
            prompt: UiPrompt::Select {
                message: "Which color?".into(),
                options: COLORS.iter().map(|color| (*color).to_owned()).collect(),
            },
        };
        let event = DriverEvent::UiRequest {
            turn: Some(self.turn_id.clone()),
            request,
        };
        let answered = if withdraw {
            let (answer, answered) = oneshot::channel();
            let _ = self.tx.send(Msg::Wait {
                request_id: request_id.clone(),
                turn: Some(self.turn_id.clone()),
                answer,
            });
            let _ = self.tx.send(Msg::event(event));
            tokio::select! {
                answer = answered => answer.ok(),
                () = tokio::time::sleep(Duration::from_secs(1)) => {
                    let _ = self.tx.send(Msg::Unwait { request_id: request_id.clone() });
                    let _ = self.tx.send(Msg::event(DriverEvent::UiWithdrawn { request_id }));
                    self.say("The demo agent withdrew its question.");
                    self.done(StopReason::EndTurn, Some(tiny_usage(5)), None);
                    return;
                }
                () = self.cancel.cancelled() => None,
            }
        } else {
            self.wait(&request_id, Some(self.turn_id.clone()), event, true)
                .await
        };
        match answered {
            Some(Answer::Ui(Some(UiAnswer::Select { index }))) => {
                let picked = usize::try_from(index)
                    .ok()
                    .and_then(|index| COLORS.get(index))
                    .copied();
                match picked {
                    Some(color) => self.say(&format!("You picked {color}.")),
                    None => self.say("You picked something the demo agent did not offer."),
                }
                self.done(StopReason::EndTurn, Some(tiny_usage(3)), None);
            }
            Some(_) => {
                self.say("No answer.");
                self.done(StopReason::EndTurn, Some(tiny_usage(2)), None);
            }
            None => self.done(StopReason::Cancelled, None, None),
        }
    }

    /// Ends the turn, then reports from the background: rule 8's late output, untagged.
    async fn background(&self) {
        let tool = format!("{}.watch", self.turn_id);
        self.emit(GhostEvent::ToolCall(tool_call(
            &tool,
            ToolKind::Execute,
            "Start a file watcher",
            ToolStatus::InProgress,
            json!({"command": "watch src"}),
        )));
        self.say("Started a watcher in the background.");
        self.done(StopReason::EndTurn, Some(tiny_usage(6)), None);
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = self.tx.send(Msg::event(DriverEvent::Event {
            turn: None,
            event: GhostEvent::ToolUpdate(tool_update(
                &tool,
                ToolStatus::Completed,
                "The watcher saw 3 changes.\n".into(),
            )),
        }));
        let number = self
            .untagged
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let request_id = format!("{}.p{number}", self.spawn_id);
        let request = PermissionRequest {
            request_id: request_id.clone(),
            tool_call_id: None,
            title: "Let the watcher write `cache/index.json`?".into(),
            kind: Some(ToolKind::Edit),
            raw_input: Some(json!({"path": "cache/index.json"})),
            content: Vec::new(),
            locations: Vec::new(),
            options: allow_or_reject(),
            truncated: None,
        };
        let event = DriverEvent::Permission {
            turn: None,
            request,
        };
        let message = match self.wait(&request_id, None, event, false).await {
            Some(Answer::Permission(PermissionOutcome::Selected(option))) if option == "allow" => {
                "The watcher wrote its cache."
            }
            Some(_) => "The watcher skipped its cache.",
            None => return,
        };
        let _ = self.tx.send(Msg::event(DriverEvent::Event {
            turn: None,
            event: GhostEvent::Notice {
                level: NoticeLevel::Info,
                message: message.into(),
            },
        }));
    }
}

fn allow_or_reject() -> Vec<PermissionOption> {
    vec![
        PermissionOption {
            option_id: "allow".into(),
            kind: PermissionOptionKind::AllowOnce,
            label: "Allow once".into(),
        },
        PermissionOption {
            option_id: "reject".into(),
            kind: PermissionOptionKind::RejectOnce,
            label: "Reject once".into(),
        },
    ]
}

fn tool_call(
    id: &str,
    kind: ToolKind,
    title: &str,
    status: ToolStatus,
    raw_input: serde_json::Value,
) -> ToolCall {
    ToolCall {
        tool_call_id: id.into(),
        kind,
        title: title.into(),
        status,
        locations: Vec::new(),
        raw_input: Some(raw_input),
        tool_name: Some(match kind {
            ToolKind::Read => "Read".into(),
            ToolKind::Execute => "Bash".into(),
            _ => "Tool".into(),
        }),
        content: Vec::new(),
        truncated: None,
    }
}

fn tool_update(id: &str, status: ToolStatus, output: String) -> ToolUpdate {
    ToolUpdate {
        tool_call_id: id.into(),
        status: Some(status),
        title: None,
        kind: None,
        locations: None,
        raw_input: None,
        tool_name: None,
        content: (!output.is_empty()).then(|| {
            vec![ToolContent::Text {
                text: output,
                truncated: None,
            }]
        }),
        truncated: None,
    }
}

fn tiny_usage(output: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: 8,
        output_tokens: output.max(1),
        cache_read_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
        scope: UsageScope::Turn,
    }
}

/// 200 KB of log lines.
fn big_output() -> String {
    let mut out = String::with_capacity(BIG_OUTPUT_BYTES + 64);
    let mut line = 0;
    while out.len() < BIG_OUTPUT_BYTES {
        line += 1;
        out.push_str(&format!(
            "2026-10-24T10:{:02}:{:02}Z INFO build step {line:05} finished in {} ms\n",
            (line / 60) % 60,
            line % 60,
            (line * 37) % 1000
        ));
    }
    out.truncate(BIG_OUTPUT_BYTES);
    out
}

const SLOW_ANSWER: &str = "Here is the slow answer, one token at a time, so you can watch it arrive. \
Streaming at twenty tokens a second leaves time to scroll, to press Stop, or to queue another \
message while this one is still being written. The demo agent has nothing urgent to say, so it \
takes its time: a few sentences about patience, about frames that stay small, and about a \
transcript that keeps up without jumping. When this paragraph ends, the turn ends too, with a \
tiny usage report.";

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use ghost_process::testing::FakeSpawner;
    use ghost_process::{
        EnvMap, EnvSource, LaunchSpec, ProcessCwd, ProcessSpawner, ProtocolLog, ResolvedEnv,
        SpawnError, SpawnSpec, SpawnedProcess,
    };
    use tokio::time::timeout;

    use super::*;
    use crate::driver::{
        AgentSpawner, DriverTimings, EventReceiver, StopPlan, TurnRef, event_channel,
    };

    /// Spawns through `FakeSpawner` with a log that writes nothing. The demo agent spawns
    /// nothing; a spawn would leave no file behind either.
    struct QuietSpawner(Arc<FakeSpawner>);

    impl AgentSpawner for QuietSpawner {
        fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
            self.0.spawn(spec, ProtocolLog::disabled())
        }
    }

    fn setup(autonomy: Autonomy, desired: BTreeMap<String, ConfigValue>) -> SessionSetup {
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
            launch: LaunchSpec {
                label: "demo".into(),
                program: PathBuf::from("/builtin/mock"),
                args: Vec::new(),
                cwd: ProcessCwd::ChatFolder,
                set_env: Vec::new(),
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(5),
            },
            env: Arc::new(ResolvedEnv {
                vars: EnvMap::new(),
                source: EnvSource::Fallback {
                    reason: "test".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            resume: Some("an-old-session".into()),
            native_start: Default::default(),
            autonomy,
            desired,
            timings: DriverTimings::STANDARD,
            client_version: "0.1.0".into(),
        }
    }

    struct Demo {
        commands: Option<mpsc::UnboundedSender<DriverCommand>>,
        events: EventReceiver,
        run: JoinHandle<DriverExit>,
    }

    impl Demo {
        fn start_with(behavior: MockBehavior, setup: SessionSetup) -> Demo {
            let (spawner, _ends) = FakeSpawner::new();
            let (commands, receiver) = mpsc::unbounded_channel();
            let (sender, events) = event_channel();
            let ctx = DriverContext {
                commands: receiver,
                events: sender,
                spawner: Arc::new(QuietSpawner(spawner)),
                setup,
            };
            let run = tokio::spawn(Box::new(MockDriver::new(behavior)).run(ctx));
            Demo {
                commands: Some(commands),
                events,
                run,
            }
        }

        async fn ready() -> (Demo, Vec<DriverEvent>) {
            let mut demo = Demo::start_with(
                MockBehavior::default(),
                setup(Autonomy::FullAuto, BTreeMap::new()),
            );
            let events = demo
                .until(|event| matches!(event, DriverEvent::Ready(_)))
                .await;
            (demo, events)
        }

        fn command(&self, command: DriverCommand) {
            self.commands
                .as_ref()
                .expect("open")
                .send(command)
                .expect("the driver reads commands");
        }

        fn prompt(&self, turn_id: &str, text: &str) {
            self.command(DriverCommand::Prompt {
                turn: TurnRef {
                    turn_id: turn_id.into(),
                },
                parts: vec![PromptPart::Text { text: text.into() }],
            });
        }

        async fn event(&mut self) -> DriverEvent {
            timeout(Duration::from_secs(120), self.events.recv())
                .await
                .expect("an event in time")
                .expect("open")
        }

        async fn until(&mut self, stop: impl Fn(&DriverEvent) -> bool) -> Vec<DriverEvent> {
            let mut seen = Vec::new();
            loop {
                let event = self.event().await;
                let done = stop(&event);
                seen.push(event);
                if done {
                    return seen;
                }
            }
        }

        async fn turn(&mut self, turn_id: &str, text: &str) -> Vec<DriverEvent> {
            self.prompt(turn_id, text);
            self.until(is_turn_ended).await
        }

        async fn finish(mut self) -> (Vec<DriverEvent>, DriverExit) {
            let mut seen = Vec::new();
            while let Ok(Some(event)) = timeout(Duration::from_secs(120), self.events.recv()).await
            {
                seen.push(event);
            }
            (seen, self.run.await.expect("no panic"))
        }

        /// Whether a `TurnEnded` arrives within `limit`.
        async fn ends_within(&mut self, limit: Duration) -> bool {
            let deadline = Instant::now() + limit;
            loop {
                match tokio::time::timeout_at(deadline, self.events.recv()).await {
                    Ok(Some(event)) if is_turn_ended(&event) => return true,
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => return false,
                }
            }
        }

        fn shutdown(&self, grace: Duration) {
            self.command(DriverCommand::Shutdown {
                plan: StopPlan {
                    cancel_grace: grace,
                    process: ghost_process::ShutdownPlan::STANDARD,
                },
            });
        }
    }

    fn is_turn_ended(event: &DriverEvent) -> bool {
        matches!(event, DriverEvent::TurnEnded { .. })
    }

    fn ended(events: &[DriverEvent]) -> (StopReason, Option<TokenUsage>, Option<DriverError>) {
        events
            .iter()
            .find_map(|event| match event {
                DriverEvent::TurnEnded {
                    stop_reason,
                    usage,
                    error,
                    ..
                } => Some((*stop_reason, usage.clone(), error.clone())),
                _ => None,
            })
            .expect("TurnEnded")
    }

    fn text_of(events: &[DriverEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                DriverEvent::Event {
                    event: GhostEvent::MessageDelta { text, .. },
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn tags(events: &[DriverEvent]) -> Vec<Option<String>> {
        events
            .iter()
            .filter_map(|event| match event {
                DriverEvent::Event { turn, .. }
                | DriverEvent::Permission { turn, .. }
                | DriverEvent::UiRequest { turn, .. } => Some(turn.clone()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn startup_offers_options_commands_and_autonomy_then_ready() {
        let start = Instant::now();
        let (_demo, events) = Demo::ready().await;
        assert!(
            start.elapsed() >= Duration::from_millis(300),
            "the startup delay"
        );
        assert_eq!(events[0], DriverEvent::Status(DriverStatus::Initializing));
        let options = events
            .iter()
            .find_map(|event| match event {
                DriverEvent::Options(options) => Some(options.clone()),
                _ => None,
            })
            .expect("Options");
        let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
        assert_eq!(ids, ["model", "effort", "mode", "fast"]);
        assert!(
            matches!(&options[2].kind, ConfigKind::Select { current, .. } if current == "full_auto")
        );
        let commands = events.iter().find_map(|event| match event {
            DriverEvent::Commands(commands) => Some(commands.len()),
            _ => None,
        });
        assert_eq!(commands, Some(40));
        assert!(events.contains(&DriverEvent::Autonomy(AutonomyState {
            requested: Autonomy::FullAuto,
            native_mode: Some("full_auto".into()),
            auto_answer: AutoAnswer::All,
            note: None,
        })));
        let DriverEvent::Ready(info) = events.last().expect("Ready") else {
            panic!("Ready")
        };
        assert!(
            info.native_session_id
                .as_deref()
                .unwrap()
                .starts_with("mock-")
        );
        assert_eq!(
            info.resumed,
            ResumeOutcome::New,
            "a resume quietly starts new"
        );
        assert!(
            info.capabilities.set_model
                && info.capabilities.set_effort
                && info.capabilities.images
                && !info.capabilities.resume
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_image_attachment_keeps_the_demo_text_command_intact() {
        let (mut demo, _) = Demo::ready().await;
        demo.command(DriverCommand::Prompt {
            turn: TurnRef {
                turn_id: "image-turn".into(),
            },
            parts: vec![
                PromptPart::Text {
                    text: "/echo image received".into(),
                },
                PromptPart::Image {
                    blob: "a".repeat(64),
                    mime: "image/png".into(),
                },
            ],
        });
        let events = demo.until(is_turn_ended).await;
        assert_eq!(text_of(&events), "image received");
        let (reason, _, error) = ended(&events);
        assert_eq!(reason, StopReason::EndTurn);
        assert!(error.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_start_fails_after_the_delay() {
        let error = DriverError {
            kind: ErrorKind::AgentUnavailable,
            message: "The demo agent is off.".into(),
            detail: None,
        };
        let demo = Demo::start_with(
            MockBehavior {
                fail_start: Some(error.clone()),
                ..MockBehavior::default()
            },
            setup(Autonomy::FullAuto, BTreeMap::new()),
        );
        let (events, exit) = demo.finish().await;
        assert_eq!(exit, DriverExit::StartFailed(error));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, DriverEvent::Ready(_)))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn desired_values_and_autonomy_shape_the_options() {
        let mut desired = BTreeMap::new();
        desired.insert(
            "model".to_owned(),
            ConfigValue::Choice {
                value: "mock-smart".into(),
            },
        );
        desired.insert(
            "mode".to_owned(),
            ConfigValue::Choice {
                value: "ask".into(),
            },
        );
        desired.insert("gone".to_owned(), ConfigValue::Toggle { value: true });
        let mut demo =
            Demo::start_with(MockBehavior::default(), setup(Autonomy::AutoEdit, desired));
        let events = demo
            .until(|event| matches!(event, DriverEvent::Ready(_)))
            .await;
        let options = events
            .iter()
            .find_map(|event| match event {
                DriverEvent::Options(options) => Some(options.clone()),
                _ => None,
            })
            .expect("Options");
        assert!(
            matches!(&options[0].kind, ConfigKind::Select { current, .. } if current == "mock-smart")
        );
        let autonomy: Vec<&AutonomyState> = events
            .iter()
            .filter_map(|event| match event {
                DriverEvent::Autonomy(state) => Some(state),
                _ => None,
            })
            .collect();
        assert_eq!(
            autonomy[0].auto_answer,
            AutoAnswer::Kinds {
                kinds: EDIT_KINDS.to_vec()
            }
        );
        // The hand-picked `ask` differs from Auto-edit's mode: auto-answers off.
        assert_eq!(autonomy[1].auto_answer, AutoAnswer::Never);
        assert!(
            autonomy[1]
                .note
                .as_deref()
                .is_some_and(|note| note.contains("Ask mode by hand"))
        );
        assert!(events.iter().any(|event| matches!(event, DriverEvent::Event { event: GhostEvent::Notice { message, .. }, .. } if message.contains("\"gone\""))));
    }

    #[tokio::test(start_paused = true)]
    async fn echo_answers_with_the_text() {
        let (mut demo, _) = Demo::ready().await;
        let events = demo.turn("T", "/echo hello there").await;
        assert_eq!(text_of(&events), "hello there");
        let (reason, usage, error) = ended(&events);
        assert_eq!((reason, error), (StopReason::EndTurn, None));
        assert_eq!(usage.map(|usage| usage.output_tokens), Some(2));
        assert!(tags(&events).iter().all(|tag| tag.as_deref() == Some("T")));
        assert!(events.iter().any(|event| matches!(event, DriverEvent::Event { event: GhostEvent::MessageCompleted { message_id }, .. } if message_id == "T.m1")));
    }

    #[tokio::test(start_paused = true)]
    async fn anything_else_plays_the_demo_turn() {
        let (mut demo, _) = Demo::ready().await;
        let events = demo.turn("T", "Please refactor main").await;
        assert!(!events.iter().any(|event| matches!(
            event,
            DriverEvent::Event {
                event: GhostEvent::TurnStarted { .. } | GhostEvent::SessionUpdated(_),
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(event, DriverEvent::Event { event: GhostEvent::ToolCall(call), .. } if call.tool_call_id == "T-read")));
        let (reason, usage, _) = ended(&events);
        assert_eq!(reason, StopReason::EndTurn);
        assert!(usage.is_some());
        assert!(tags(&events).iter().all(|tag| tag.as_deref() == Some("T")));
        assert_eq!(
            events.last(),
            Some(&DriverEvent::TurnEnded {
                turn_id: "T".into(),
                stop_reason: StopReason::EndTurn,
                usage,
                error: None
            })
        );
    }

    async fn permission_turn(answer: PermissionOutcome) -> Vec<DriverEvent> {
        let (mut demo, _) = Demo::ready().await;
        demo.prompt("T", "/permission");
        let mut events = demo
            .until(|event| matches!(event, DriverEvent::Permission { .. }))
            .await;
        let Some(DriverEvent::Permission { turn, request }) = events.last() else {
            panic!("Permission")
        };
        assert_eq!(turn.as_deref(), Some("T"));
        assert_eq!(request.kind, Some(ToolKind::Execute));
        assert_eq!(
            request
                .options
                .iter()
                .map(|option| option.kind)
                .collect::<Vec<_>>(),
            [
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::RejectOnce
            ]
        );
        demo.command(DriverCommand::RespondPermission {
            request_id: request.request_id.clone(),
            outcome: answer,
        });
        events.extend(demo.until(is_turn_ended).await);
        events
    }

    #[tokio::test(start_paused = true)]
    async fn permission_allow_completes_and_reject_skips() {
        let events = permission_turn(PermissionOutcome::Selected("allow".into())).await;
        assert!(events.iter().any(|event| matches!(event, DriverEvent::Event { event: GhostEvent::ToolUpdate(update), .. } if update.status == Some(ToolStatus::Completed))));
        assert_eq!(text_of(&events), "Done.");
        let events = permission_turn(PermissionOutcome::Selected("reject".into())).await;
        assert!(events.iter().any(|event| matches!(event, DriverEvent::Event { event: GhostEvent::ToolUpdate(update), .. } if update.status == Some(ToolStatus::Failed))));
        assert_eq!(text_of(&events), "Skipped.");
        let events = permission_turn(PermissionOutcome::Cancelled).await;
        assert_eq!(text_of(&events), "Skipped.");
    }

    #[tokio::test(start_paused = true)]
    async fn withdraw_withdraws_after_a_second() {
        let (mut demo, _) = Demo::ready().await;
        demo.prompt("T", "/withdraw");
        let asked = demo
            .until(|event| matches!(event, DriverEvent::Permission { .. }))
            .await;
        let Some(DriverEvent::Permission { request, .. }) = asked.last() else {
            panic!("Permission")
        };
        let start = Instant::now();
        let events = demo.until(is_turn_ended).await;
        assert!(start.elapsed() >= Duration::from_secs(1));
        assert!(events.contains(&DriverEvent::PermissionWithdrawn {
            request_id: request.request_id.clone()
        }));
        // An answer after the withdrawal goes nowhere.
        demo.command(DriverCommand::RespondPermission {
            request_id: request.request_id.clone(),
            outcome: PermissionOutcome::Cancelled,
        });
    }

    #[tokio::test(start_paused = true)]
    async fn mock_question() {
        // An answer.
        let (mut demo, _) = Demo::ready().await;
        demo.prompt("T", "/ask");
        let asked = demo
            .until(|event| matches!(event, DriverEvent::UiRequest { .. }))
            .await;
        let Some(DriverEvent::UiRequest { turn, request }) = asked.last() else {
            panic!("UiRequest")
        };
        assert_eq!(turn.as_deref(), Some("T"));
        assert_eq!(
            request.prompt,
            UiPrompt::Select {
                message: "Which color?".into(),
                options: vec!["Red".into(), "Green".into(), "Blue".into()]
            }
        );
        demo.command(DriverCommand::RespondUi {
            request_id: request.request_id.clone(),
            answer: Some(UiAnswer::Select { index: 1 }),
        });
        let events = demo.until(is_turn_ended).await;
        assert_eq!(text_of(&events), "You picked Green.");

        // Dismissed.
        demo.prompt("U", "/ask");
        let asked = demo
            .until(|event| matches!(event, DriverEvent::UiRequest { .. }))
            .await;
        let Some(DriverEvent::UiRequest { request, .. }) = asked.last() else {
            panic!("UiRequest")
        };
        demo.command(DriverCommand::RespondUi {
            request_id: request.request_id.clone(),
            answer: None,
        });
        let events = demo.until(is_turn_ended).await;
        assert_eq!(text_of(&events), "No answer.");

        // Withdrawn.
        demo.prompt("V", "/ask withdraw");
        let events = demo.until(is_turn_ended).await;
        assert!(events.contains(&DriverEvent::UiWithdrawn {
            request_id: "V.p1".into()
        }));
        assert_eq!(ended(&events).0, StopReason::EndTurn);

        // Interrupted while asked: the turn ends cancelled.
        demo.prompt("W", "/ask");
        demo.until(|event| matches!(event, DriverEvent::UiRequest { .. }))
            .await;
        demo.command(DriverCommand::Interrupt);
        let events = demo.until(is_turn_ended).await;
        assert_eq!(ended(&events).0, StopReason::Cancelled);
    }

    #[tokio::test(start_paused = true)]
    async fn background_output_arrives_untagged_after_the_turn() {
        let (mut demo, _) = Demo::ready().await;
        let events = demo.turn("T", "/background").await;
        assert_eq!(ended(&events).0, StopReason::EndTurn);
        // The next turn is running when the late output arrives: still untagged.
        demo.prompt("U", "/hang");
        let late = demo
            .until(|event| matches!(event, DriverEvent::Permission { turn: None, .. }))
            .await;
        assert!(late.iter().any(|event| matches!(event,
            DriverEvent::Event { turn: None, event: GhostEvent::ToolUpdate(update) } if update.tool_call_id == "T.watch")));
        let Some(DriverEvent::Permission { request, .. }) = late.last() else {
            panic!("Permission")
        };
        // The background request survives the next turn's end.
        demo.command(DriverCommand::Interrupt);
        demo.until(is_turn_ended).await;
        demo.command(DriverCommand::RespondPermission {
            request_id: request.request_id.clone(),
            outcome: PermissionOutcome::Selected("allow".into()),
        });
        let notice = demo
            .until(|event| {
                matches!(
                    event,
                    DriverEvent::Event {
                        event: GhostEvent::Notice { .. },
                        ..
                    }
                )
            })
            .await;
        assert!(
            matches!(notice.last(), Some(DriverEvent::Event { turn: None, event: GhostEvent::Notice { message, .. } }) if message == "The watcher wrote its cache.")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn slow_streams_at_twenty_tokens_a_second() {
        let (mut demo, _) = Demo::ready().await;
        let start = Instant::now();
        let events = demo.turn("T", "/slow").await;
        let tokens = SLOW_ANSWER.split_whitespace().count();
        assert!(
            start.elapsed() >= Duration::from_secs_f64(tokens as f64 / 20.0 - 1.0),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(text_of(&events), SLOW_ANSWER);
    }

    #[tokio::test(start_paused = true)]
    async fn hang_ends_only_when_interrupted() {
        let (mut demo, _) = Demo::ready().await;
        demo.prompt("T", "/hang");
        demo.until(|event| {
            matches!(
                event,
                DriverEvent::Event {
                    event: GhostEvent::MessageDelta { .. },
                    ..
                }
            )
        })
        .await;
        assert!(
            !demo.ends_within(Duration::from_secs(600)).await,
            "never ends by itself"
        );
        demo.command(DriverCommand::Interrupt);
        assert_eq!(
            ended(&demo.until(is_turn_ended).await).0,
            StopReason::Cancelled
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ignore_cancel_ignores_interrupts_and_a_stop_ends_without_a_turn_end() {
        let (mut demo, _) = Demo::ready().await;
        demo.prompt("T", "/ignore-cancel");
        demo.until(|event| {
            matches!(
                event,
                DriverEvent::Event {
                    event: GhostEvent::MessageDelta { .. },
                    ..
                }
            )
        })
        .await;
        demo.command(DriverCommand::Interrupt);
        assert!(!demo.ends_within(Duration::from_secs(60)).await);
        let start = Instant::now();
        demo.shutdown(Duration::from_secs(3));
        let (events, exit) = demo.finish().await;
        assert_eq!(exit, DriverExit::Closed);
        assert!(start.elapsed() >= Duration::from_secs(3), "the whole grace");
        assert!(!events.iter().any(is_turn_ended));
    }

    #[tokio::test(start_paused = true)]
    async fn fail_auth_and_big() {
        let (mut demo, _) = Demo::ready().await;
        let events = demo.turn("F", "/fail").await;
        assert_eq!(
            ended(&events),
            (
                StopReason::Error,
                None,
                Some(DriverError {
                    kind: ErrorKind::AgentError,
                    message: FAIL_MESSAGE.into(),
                    detail: None
                })
            )
        );

        let events = demo.turn("A", "/auth").await;
        let auth_at = events
            .iter()
            .position(|event| matches!(event, DriverEvent::AuthRequired(_)))
            .expect("AuthRequired");
        assert!(auth_at < events.len() - 1);
        assert_eq!(
            ended(&events).2.map(|error| error.kind),
            Some(ErrorKind::AuthRequired)
        );

        let events = demo.turn("B", "/big").await;
        let output = events.iter().find_map(|event| match event {
            DriverEvent::Event {
                event:
                    GhostEvent::ToolUpdate(ToolUpdate {
                        content: Some(content),
                        ..
                    }),
                ..
            } => match content.as_slice() {
                [ToolContent::Text { text, .. }] => Some(text.len()),
                _ => None,
            },
            _ => None,
        });
        assert_eq!(output, Some(BIG_OUTPUT_BYTES));
    }

    #[tokio::test(start_paused = true)]
    async fn crash_returns_crashed_mid_turn_with_the_scripted_tail() {
        let (demo, _) = Demo::ready().await;
        demo.prompt("T", "/crash");
        let (events, exit) = demo.finish().await;
        assert!(!events.iter().any(is_turn_ended));
        assert_eq!(text_of(&events), "Working on it...");
        let DriverExit::Crashed { exit, stderr_tail } = exit else {
            panic!("Crashed")
        };
        assert_eq!(stderr_tail, CRASH_TAIL);
        assert_eq!(exit.and_then(|exit| exit.code), Some(101));
    }

    #[tokio::test(start_paused = true)]
    async fn stress_streams_n_tokens() {
        let (mut demo, _) = Demo::ready().await;
        let events = demo.turn("T", "/stress 500").await;
        assert_eq!(ended(&events).1.map(|usage| usage.output_tokens), Some(500));
    }

    #[tokio::test(start_paused = true)]
    async fn options_and_autonomy_change_on_request() {
        let (mut demo, _) = Demo::ready().await;
        let (reply, answer) = oneshot::channel();
        demo.command(DriverCommand::SetOption {
            option_id: "model".into(),
            value: ConfigValue::Choice {
                value: "nope".into(),
            },
            reply,
        });
        assert_eq!(
            answer.await.expect("answered").map_err(|error| error.kind),
            Err(ErrorKind::Other)
        );
        let (reply, answer) = oneshot::channel();
        demo.command(DriverCommand::SetOption {
            option_id: "fast".into(),
            value: ConfigValue::Toggle { value: true },
            reply,
        });
        assert_eq!(answer.await.expect("answered"), Ok(()));
        let DriverEvent::Options(options) = demo.event().await else {
            panic!("Options")
        };
        assert_eq!(options[3].kind, ConfigKind::Toggle { current: true });

        // A mode by hand turns auto-answers off; the level's own mode restores them.
        let (reply, answer) = oneshot::channel();
        demo.command(DriverCommand::SetOption {
            option_id: "mode".into(),
            value: ConfigValue::Choice {
                value: "ask".into(),
            },
            reply,
        });
        assert_eq!(answer.await.expect("answered"), Ok(()));
        let events = demo
            .until(|event| matches!(event, DriverEvent::Autonomy(_)))
            .await;
        assert!(matches!(
            events.last(),
            Some(DriverEvent::Autonomy(AutonomyState {
                auto_answer: AutoAnswer::Never,
                note: Some(_),
                ..
            }))
        ));
        let (reply, answer) = oneshot::channel();
        demo.command(DriverCommand::SetOption {
            option_id: "mode".into(),
            value: ConfigValue::Choice {
                value: "full_auto".into(),
            },
            reply,
        });
        assert_eq!(answer.await.expect("answered"), Ok(()));
        let events = demo
            .until(|event| matches!(event, DriverEvent::Autonomy(_)))
            .await;
        assert!(matches!(
            events.last(),
            Some(DriverEvent::Autonomy(AutonomyState {
                auto_answer: AutoAnswer::All,
                note: None,
                ..
            }))
        ));

        // SetAutonomy replies with the state and moves the mode.
        let (reply, answer) = oneshot::channel();
        demo.command(DriverCommand::SetAutonomy {
            autonomy: Autonomy::Ask,
            reply,
        });
        let state = answer.await.expect("answered").expect("ok");
        assert_eq!(
            state,
            AutonomyState {
                requested: Autonomy::Ask,
                native_mode: Some("ask".into()),
                auto_answer: AutoAnswer::Never,
                note: None
            }
        );
        let DriverEvent::Options(options) = demo.event().await else {
            panic!("Options")
        };
        assert!(matches!(&options[2].kind, ConfigKind::Select { current, .. } if current == "ask"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_stop_during_a_turn_cancels_it_then_closes() {
        let (mut demo, _) = Demo::ready().await;
        demo.prompt("T", "Write the whole thing");
        demo.until(|event| matches!(event, DriverEvent::Event { .. }))
            .await;
        demo.shutdown(Duration::from_secs(3));
        let (events, exit) = demo.finish().await;
        assert_eq!(ended(&events).0, StopReason::Cancelled);
        assert_eq!(exit, DriverExit::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_channel_closes_and_commands_before_ready_wait() {
        let mut demo = Demo::start_with(
            MockBehavior::default(),
            setup(Autonomy::FullAuto, BTreeMap::new()),
        );
        demo.prompt("early", "hi");
        let (reply, answer) = oneshot::channel();
        demo.command(DriverCommand::SetAutonomy {
            autonomy: Autonomy::Ask,
            reply,
        });
        let early = demo.until(is_turn_ended).await;
        assert!(matches!(
            ended(&early),
            (
                StopReason::Error,
                None,
                Some(DriverError {
                    kind: ErrorKind::Internal,
                    ..
                })
            )
        ));
        demo.until(|event| matches!(event, DriverEvent::Ready(_)))
            .await;
        assert_eq!(
            answer.await.expect("answered").map(|state| state.requested),
            Ok(Autonomy::Ask),
            "held until Ready"
        );
        demo.command(DriverCommand::Interrupt);
        demo.commands = None;
        let (_, exit) = demo.finish().await;
        assert_eq!(exit, DriverExit::Closed);
    }

    /// Contract rules 1, 2 and 8 over everything the demo agent emitted for `prompts`.
    fn contract_violations(events: &[DriverEvent], prompts: &[&str]) -> Vec<String> {
        let mut violations = Vec::new();
        let ready_at = events
            .iter()
            .position(|event| matches!(event, DriverEvent::Ready(_)));
        for event in &events[..ready_at.unwrap_or(events.len())] {
            let allowed = matches!(
                event,
                DriverEvent::Status(_)
                    | DriverEvent::Options(_)
                    | DriverEvent::Commands(_)
                    | DriverEvent::Title(_)
                    | DriverEvent::Autonomy(_)
                    | DriverEvent::AuthRequired(_)
                    | DriverEvent::Event {
                        turn: None,
                        event: GhostEvent::Notice { .. } | GhostEvent::UsageUpdated(_)
                    }
            );
            if !allowed {
                violations.push(format!("before Ready: {event:?}"));
            }
        }
        let mut ended = Vec::new();
        for event in events {
            match event {
                DriverEvent::TurnEnded { turn_id, .. } => {
                    if ended.contains(turn_id) {
                        violations.push(format!("a second TurnEnded for {turn_id}"));
                    }
                    ended.push(turn_id.clone());
                }
                DriverEvent::Event {
                    turn: Some(turn),
                    event: inner,
                } => {
                    if ended.contains(turn) || !prompts.contains(&turn.as_str()) {
                        violations.push(format!("{event:?} tagged {turn}"));
                    }
                    if matches!(
                        inner,
                        GhostEvent::TurnStarted { .. }
                            | GhostEvent::TurnEnded { .. }
                            | GhostEvent::SessionUpdated(_)
                    ) {
                        violations.push(format!("a driver never emits {inner:?}"));
                    }
                }
                DriverEvent::Permission {
                    turn: Some(turn), ..
                }
                | DriverEvent::UiRequest {
                    turn: Some(turn), ..
                } if ended.contains(turn) => {
                    violations.push(format!("{event:?} after its turn ended"));
                }
                _ => {}
            }
        }
        for prompt in prompts {
            if !ended.iter().any(|turn| turn == prompt) {
                violations.push(format!("no TurnEnded for {prompt}"));
            }
        }
        violations
    }

    #[tokio::test(start_paused = true)]
    async fn the_demo_agent_keeps_the_driver_contract() {
        let (mut demo, mut events) = Demo::ready().await;
        let prompts = ["T1", "T2", "T3", "T4", "T5", "T6", "T7", "T8"];
        for (turn, text) in prompts.iter().zip([
            "/echo one",
            "Tell me about this repo",
            "/fail",
            "/auth",
            "/big",
            "/stress 300",
            "/withdraw",
            "/background",
        ]) {
            events.extend(demo.turn(turn, text).await);
        }
        // The background turn's late output arrives untagged after its end.
        events.extend(
            demo.until(|event| matches!(event, DriverEvent::Permission { turn: None, .. }))
                .await,
        );
        demo.shutdown(Duration::from_secs(3));
        let (rest, exit) = demo.finish().await;
        events.extend(rest);
        assert_eq!(exit, DriverExit::Closed);
        let violations = contract_violations(&events, &prompts);
        assert!(violations.is_empty(), "{violations:#?}");
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_deltas_merge_and_nothing_is_lost() {
        let (mut demo, _) = Demo::ready().await;
        demo.prompt("T", "/stress 3000");
        // Nobody reads for a long time: the outbox merges adjacent deltas.
        tokio::time::sleep(Duration::from_secs(120)).await;
        let events = demo.until(is_turn_ended).await;
        let deltas = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    DriverEvent::Event {
                        event: GhostEvent::MessageDelta { .. },
                        ..
                    }
                )
            })
            .count();
        // The channel takes 256 events as they come; whatever waits behind them merges into one
        // thought and one message delta.
        assert!(
            deltas <= crate::driver::DRIVER_EVENT_CAPACITY + 2,
            "{deltas} deltas"
        );
        assert!(deltas < 3000 / 10, "{deltas} deltas for 3,000 tokens");
        let script = MockScript::stress("T", 120, 3000);
        let expected: String = script
            .steps
            .iter()
            .filter_map(|step| match step {
                MockStep::StreamText { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text_of(&events), expected);
    }
}
