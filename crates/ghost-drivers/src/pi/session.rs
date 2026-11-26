//! One owner correlates Pi RPC, run state, explicit extension dialogs and the owned process.
use super::{
    metadata::{self, Metadata},
    translate, ui, wire,
};
use crate::driver::{
    DriverCommand, DriverContext, DriverError, DriverEvent, DriverExit, DriverStatus,
    LoggedCommand, ReadyInfo, ResumeOutcome, SessionSetup, SteerResult, StopPlan,
};
use futures::future::BoxFuture;
use ghost_process::{
    LineSink, LogNote, MAX_LINE_BYTES, PIPE_EOF_GRACE, ProtocolLog, STDERR_TAIL_BYTES,
    ShutdownPlan, SpawnSpec, StdoutItem,
};
use ghost_protocol::{
    AutoAnswer, Autonomy, AutonomyState, Capabilities, ErrorKind, GhostEvent, NoticeLevel,
    PromptPart, StopReason, UiRequest, UsageInfo,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::oneshot, time::Instant};

type Reply = oneshot::Sender<Result<(), DriverError>>;
/// Said when Pi settled or stopped with a steering message it had queued but not yet read: the
/// queue is cleared so the message cannot open the next prompt.
const STEER_DROPPED: &str = "Pi finished before it read your steering message, so it was not delivered. Send it again as a new message.";
fn error(message: impl Into<String>) -> DriverError {
    DriverError {
        kind: ErrorKind::ProtocolError,
        message: message.into(),
        detail: None,
    }
}
fn setting_error() -> DriverError {
    error("Pi did not apply this setting before its deadline.")
}
fn log_command(log: &ProtocolLog, command: &DriverCommand) {
    if let Ok(value) = serde_json::to_value(LoggedCommand::from(command)) {
        log.command(value);
    }
}
fn is_config(command: &DriverCommand) -> bool {
    matches!(
        command,
        DriverCommand::SetOption { .. } | DriverCommand::SetAutonomy { .. }
    )
}
fn autonomy() -> AutonomyState {
    AutonomyState {
        requested: Autonomy::FullAuto,
        native_mode: None,
        auto_answer: AutoAnswer::Never,
        note: Some("Pi controls tool access. Extension dialogs always require your answer.".into()),
    }
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
}
fn native_id(setup: &SessionSetup) -> Result<String, DriverError> {
    if setup.autonomy != Autonomy::FullAuto {
        return Err(error(
            "Pi RPC cannot enforce Ask or Auto edit. Choose Full auto to start Pi.",
        ));
    }
    if setup.native_start.resume_at.is_some()
        || setup.native_start.file_checkpointing
        || !setup.add_dirs.is_empty()
    {
        return Err(error("Pi does not support these startup options."));
    }
    if setup.native_start.fork && setup.resume.is_none() {
        return Err(error("Pi fork requires a source session."));
    }
    if setup
        .resume
        .as_ref()
        .is_some_and(|s| s.trim().is_empty() || s.contains('\0') || s.starts_with('-'))
    {
        return Err(error("Pi resume needs a session id or transcript path."));
    }
    if setup.resume.is_some()
        && !setup.native_start.fork
        && setup.native_start.new_session_id.is_some()
    {
        return Err(error(
            "An ordinary Pi resume cannot also choose a new session id.",
        ));
    }
    let id = setup
        .native_start
        .new_session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    if !valid_id(&id) {
        return Err(error("Pi's new session id is invalid."));
    }
    if setup.native_start.fork && setup.resume.as_deref() == Some(&id) {
        return Err(error("Pi fork must use a different session id."));
    }
    Ok(id)
}
fn launch(setup: &SessionSetup, id: &str) -> ghost_process::LaunchSpec {
    let mut launch = setup.launch.clone();
    launch.args.extend(["--mode".into(), "rpc".into()]);
    if let Some(source) = &setup.resume {
        launch.args.extend([
            if setup.native_start.fork {
                "--fork"
            } else {
                "--session"
            }
            .into(),
            source.into(),
        ]);
    }
    if setup.resume.is_none() || setup.native_start.fork {
        launch.args.extend(["--session-id".into(), id.into()]);
    }
    launch
}

#[derive(Debug)]
enum Purpose {
    State,
    Models,
    Levels,
    Commands,
    Stats,
    Prompt,
    Clear,
    Abort,
    Setting(Option<Reply>),
    /// A steer for this aura turn. Pi queues it and reads it before its next model call.
    Steer {
        turn: String,
        reply: Option<oneshot::Sender<SteerResult>>,
    },
    /// Clears steering Pi queued after the turn it was meant for settled. `notify` says the
    /// message was already reported delivered, so dropping it needs a notice.
    Strand {
        notify: bool,
    },
}
struct Pending {
    command: String,
    purpose: Purpose,
    deadline: Instant,
    startup: bool,
    activity_revision: u64,
}
struct Dialog {
    frame: Value,
    turn: Option<String>,
    deadline: Option<Instant>,
}
struct Setting {
    reply: Option<Reply>,
    deadline: Instant,
}
struct Session {
    setup: SessionSetup,
    sink: LineSink,
    log: ProtocolLog,
    spawn_id: String,
    native_id: String,
    next_id: u64,
    pending: BTreeMap<String, Pending>,
    dialogs: BTreeMap<String, Dialog>,
    metadata: Metadata,
    translator: translate::Translator,
    outbox: VecDeque<DriverEvent>,
    held: VecDeque<(Instant, DriverCommand)>,
    before_ready: VecDeque<DriverEvent>,
    desired: VecDeque<(String, ghost_protocol::session::ConfigValue)>,
    setting: Option<Setting>,
    ready: bool,
    state_seen: bool,
    idle: bool,
    run_active: bool,
    compacting: bool,
    activity_revision: u64,
    turn: Option<String>,
    interrupted: bool,
    init_deadline: Instant,
    last_activity: Instant,
    failure: Option<DriverError>,
    stopping: bool,
    stats_dirty: bool,
    usage: UsageInfo,
    last_error: Option<String>,
    run_failed: bool,
    /// Steering messages Pi last reported in its queue (`queue_update`).
    queued_steering: usize,
}
impl Session {
    fn send(&mut self, frame: Value) {
        if self.sink.send(frame.to_string()).is_err() {
            self.failure
                .get_or_insert_with(|| error("Pi closed its input."));
        }
    }
    fn request(
        &mut self,
        mut frame: Value,
        purpose: Purpose,
        startup: bool,
        deadline: Option<Instant>,
    ) {
        self.next_id += 1;
        let id = format!("aura_{}", self.next_id);
        frame["id"] = json!(id);
        let command = frame["type"].as_str().unwrap_or_default().to_owned();
        self.pending.insert(
            id,
            Pending {
                command,
                purpose,
                startup,
                activity_revision: self.activity_revision,
                deadline: deadline.unwrap_or_else(|| Instant::now() + self.setup.timings.request),
            },
        );
        self.send(frame);
    }
    fn query(&mut self, name: &str, purpose: Purpose, startup: bool) {
        self.request(json!({"type":name}), purpose, startup, None);
    }
    fn notice(&mut self, message: impl Into<String>) {
        self.outbox.push_back(DriverEvent::Event {
            turn: None,
            event: GhostEvent::Notice {
                level: NoticeLevel::Warning,
                message: message.into(),
            },
        });
    }
    fn idle(&mut self, idle: bool) {
        if self.idle != idle {
            self.idle = idle;
            self.outbox.push_back(DriverEvent::RunState { idle });
        }
    }
    fn emit(&mut self, event: GhostEvent) {
        let event = DriverEvent::Event {
            turn: self.turn.clone(),
            event,
        };
        if self.ready {
            self.outbox.push_back(event);
        } else {
            // An extension may start work during loading. Never violate the actor's startup phase.
            if self.before_ready.len() >= 256
                || self
                    .before_ready
                    .iter()
                    .map(DriverEvent::approx_bytes)
                    .sum::<usize>()
                    >= 8 * 1024 * 1024
            {
                self.failure = Some(error(
                    "Pi produced too much output before session initialization completed.",
                ));
            } else {
                self.before_ready.push_back(event);
            }
        }
    }
    fn stats(&mut self) {
        if self
            .pending
            .values()
            .any(|p| matches!(p.purpose, Purpose::Stats))
        {
            self.stats_dirty = true;
        } else {
            self.query("get_session_stats", Purpose::Stats, false);
        }
    }
    fn refresh(&mut self) {
        for (command, purpose) in [
            ("get_state", Purpose::State),
            ("get_available_thinking_levels", Purpose::Levels),
            ("get_commands", Purpose::Commands),
        ] {
            if !self.pending.values().any(|p| p.command == command) {
                self.query(command, purpose, false);
            }
        }
        self.stats();
    }
    fn configuring(&self) -> bool {
        self.setting.is_some()
            || self.pending.values().any(|p| {
                matches!(
                    p.purpose,
                    Purpose::Setting(_) | Purpose::Clear | Purpose::Abort | Purpose::Strand { .. }
                ) || (self.ready && matches!(p.purpose, Purpose::State | Purpose::Levels))
            })
    }
    fn startup(&mut self) {
        if self.ready
            || self.stopping
            || self.failure.is_some()
            || !self.state_seen
            || self.configuring()
            || self.pending.values().any(|p| p.startup)
        {
            return;
        }
        if let Some((id, value)) = self.desired.pop_front() {
            if let Some(request) = self.metadata.request(&id, &value) {
                self.request(request, Purpose::Setting(None), true, None);
            } else {
                self.failure = Some(error(format!(
                    "Pi does not offer the requested {id} setting."
                )));
            }
            return;
        }
        self.outbox
            .push_back(DriverEvent::Options(self.metadata.options()));
        self.outbox.push_back(DriverEvent::Autonomy(autonomy()));
        self.outbox
            .push_back(DriverEvent::RunState { idle: self.idle });
        self.ready = true;
        self.outbox.push_back(DriverEvent::Ready(ReadyInfo {
            native_session_id: Some(self.native_id.clone()),
            agent_name: Some("Pi".into()),
            agent_version: None,
            spawn_id: self.spawn_id.clone(),
            replay: None,
            resumed: if self.setup.resume.is_some() && !self.setup.native_start.fork {
                ResumeOutcome::Resumed
            } else {
                ResumeOutcome::New
            },
            capabilities: Capabilities {
                images: true,
                steer: true,
                interrupt: true,
                resume: true,
                fork: true,
                set_model: !self.metadata.models.is_empty(),
                set_effort: !self.metadata.levels.is_empty(),
                usage: true,
                slash_commands: true,
                autonomy_levels: vec![Autonomy::FullAuto],
                ..Default::default()
            },
        }));
        self.outbox.append(&mut self.before_ready);
    }
    fn state(&mut self, data: &Value, requested_at_revision: u64) -> bool {
        let Some(id) = wire::text(data, "sessionId").filter(|s| valid_id(s)) else {
            return false;
        };
        let (Some(streaming), Some(compacting), Some(pending)) = (
            data["isStreaming"].as_bool(),
            data["isCompacting"].as_bool(),
            data["pendingMessageCount"].as_u64(),
        ) else {
            return false;
        };
        let expected =
            if self.state_seen || self.setup.resume.is_none() || self.setup.native_start.fork {
                Some(self.native_id.as_str())
            } else {
                self.setup.resume.as_deref().filter(|s| valid_id(s))
            };
        if expected.is_some_and(|expected| expected != id) {
            self.failure = Some(error("Pi opened a different session than requested."));
            return false;
        }
        self.native_id = id.into();
        self.state_seen = true;
        self.metadata.state(data);
        // The RPC handler may capture state before a concurrently emitted lifecycle event.
        // A delayed initial snapshot must not undo newer busy/idle evidence.
        if !self.ready && requested_at_revision == self.activity_revision {
            self.run_active = streaming || pending > 0;
            self.compacting = compacting;
            self.idle(!self.run_active && !self.compacting);
        }
        self.outbox.push_back(DriverEvent::Title(
            data["sessionName"].as_str().map(str::to_owned),
        ));
        true
    }
    fn finish(&mut self, reason: StopReason, err: Option<DriverError>) {
        for event in self.translator.close_open() {
            self.emit(event);
        }
        if let Some(turn) = self.turn.take() {
            self.withdraw(Some(&turn));
            self.outbox.push_back(DriverEvent::TurnEnded {
                turn_id: turn,
                stop_reason: reason,
                usage: self.translator.usage(),
                error: err,
            });
        }
    }
    fn withdraw(&mut self, turn: Option<&str>) {
        let ids: Vec<_> = self
            .dialogs
            .iter()
            .filter(|(_, d)| turn.is_none() || d.turn.as_deref() == turn)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(dialog) = self.dialogs.remove(&id) {
                self.send(ui::response(&dialog.frame, None));
                self.outbox
                    .push_back(DriverEvent::UiWithdrawn { request_id: id });
            }
        }
    }
    fn interrupt(&mut self) {
        self.withdraw(None);
        let held = std::mem::take(&mut self.held);
        for (arrived, command) in held {
            if let DriverCommand::Prompt { turn, .. } = command {
                self.outbox.push_back(DriverEvent::TurnEnded {
                    turn_id: turn.turn_id,
                    stop_reason: StopReason::Cancelled,
                    usage: None,
                    error: None,
                });
            } else {
                self.held.push_back((arrived, command));
            }
        }
        if self.interrupted {
            return;
        }
        self.interrupted = true;
        // Clearing first prevents a queued follow-up from starting after abort reaches idle.
        self.query("clear_queue", Purpose::Clear, false);
    }
    /// Pi takes a steer for the run in progress and reads it after the current tool calls, before
    /// its next model call; a run that would end continues for it. Only a running, unstopped turn
    /// whose prompt Pi has accepted can be steered.
    fn steer(
        &mut self,
        turn_id: String,
        parts: &[PromptPart],
        reply: oneshot::Sender<SteerResult>,
    ) {
        if reply.is_closed() {
            return;
        }
        let running = self.ready
            && !self.stopping
            && self.failure.is_none()
            && !self.interrupted
            && self.run_active
            && self.turn.as_deref() == Some(turn_id.as_str())
            && !self.pending.values().any(|p| {
                matches!(
                    p.purpose,
                    Purpose::Prompt | Purpose::Clear | Purpose::Abort | Purpose::Steer { .. }
                )
            });
        if !running {
            let _ = reply.send(SteerResult::Rejected {
                message: "The selected turn is no longer running or has not started yet.".into(),
            });
            return;
        }
        let mut request = match crate::attachments::pi(parts, self.setup.attachment_dir.as_deref())
        {
            Ok(request) => request,
            Err(message) => {
                let _ = reply.send(SteerResult::Rejected { message });
                return;
            }
        };
        request["type"] = json!("steer");
        self.request(
            request,
            Purpose::Steer {
                turn: turn_id,
                reply: Some(reply),
            },
            false,
            None,
        );
    }
    /// Clears steering Pi still holds once nothing will read it, unless a clear is already due.
    fn clear_stranded(&mut self, notify: bool) {
        if self.queued_steering == 0
            || self.stopping
            || self
                .pending
                .values()
                .any(|p| matches!(p.purpose, Purpose::Clear | Purpose::Strand { .. }))
        {
            return;
        }
        self.query("clear_queue", Purpose::Strand { notify }, false);
    }
    fn ui(&mut self, frame: Value) {
        let method = wire::text(&frame, "method").unwrap_or_default();
        if self.stopping {
            if ui::blocking(method) {
                self.send(ui::response(&frame, None));
            }
            return;
        }
        match method {
            "select" | "confirm" | "input" | "editor" => {
                let id = translate::id(frame["id"].as_str().unwrap_or_default());
                if self.dialogs.contains_key(&id) {
                    self.failure = Some(error("Pi reused a pending dialog id."));
                    return;
                }
                let Some(prompt) = ui::prompt(&frame) else {
                    self.send(ui::response(&frame, None));
                    self.notice(
                        "Pi requested an unsupported, oversized or malformed dialog; it was cancelled.",
                    );
                    return;
                };
                let request_bytes = frame.to_string().len();
                if self.dialogs.len() >= 256
                    || request_bytes > 1024 * 1024
                    || self
                        .dialogs
                        .values()
                        .map(|d| d.frame.to_string().len())
                        .sum::<usize>()
                        .saturating_add(request_bytes)
                        > 8 * 1024 * 1024
                {
                    self.send(ui::response(&frame, None));
                    self.notice(
                        "Pi exceeded the pending dialog limit; the new dialog was cancelled.",
                    );
                    return;
                }
                let deadline = frame["timeout"]
                    .as_u64()
                    .filter(|ms| *ms > 0)
                    .map(|ms| Instant::now() + Duration::from_millis(ms.min(86_400_000)));
                self.dialogs.insert(
                    id.clone(),
                    Dialog {
                        frame,
                        turn: self.turn.clone(),
                        deadline,
                    },
                );
                self.outbox.push_back(DriverEvent::UiRequest {
                    turn: self.turn.clone(),
                    request: UiRequest {
                        request_id: id,
                        prompt,
                    },
                });
            }
            "notify" => {
                if let Some(message) = frame["message"].as_str() {
                    self.notice(message);
                }
            }
            // Status widgets and composer replacement have no product surface. They never receive
            // replies and never become hidden user prompts.
            "setTitle" | "setStatus" | "setWidget" | "set_editor_text" => {}
            _ => {
                self.send(ui::response(&frame, None));
                self.notice("Pi requested an unknown extension interaction; it was cancelled.");
            }
        }
    }
    fn incoming(&mut self, incoming: wire::Incoming) {
        self.last_activity = Instant::now();
        match incoming {
            wire::Incoming::Invalid(_) => {
                self.failure = Some(error("Pi sent an invalid RPC envelope."));
            }
            wire::Incoming::Ui(frame) => self.ui(frame),
            wire::Incoming::Response {
                id,
                command,
                success,
                data,
                error: detail,
            } => {
                let Some(pending) = self.pending.remove(&id) else {
                    return;
                };
                if pending.command != command {
                    self.failure = Some(error(
                        "Pi replied with the wrong command for a pending request.",
                    ));
                    return;
                }
                if !success {
                    let mut err = error(format!("Pi rejected {command}."));
                    err.detail = detail;
                    match pending.purpose {
                        Purpose::Setting(reply) => {
                            if let Some(reply) = reply {
                                let _ = reply.send(Err(err.clone()));
                            } else {
                                self.failure = Some(err);
                            }
                        }
                        Purpose::Prompt => {
                            self.finish(StopReason::Error, Some(err));
                            self.idle(!self.run_active && !self.compacting);
                        }
                        Purpose::State | Purpose::Levels
                            if pending.startup || self.setting.is_some() =>
                        {
                            self.failure = Some(err)
                        }
                        Purpose::Clear | Purpose::Abort => self.failure = Some(err),
                        Purpose::Steer { reply, .. } => {
                            if let Some(reply) = reply {
                                let _ = reply.send(SteerResult::Rejected {
                                    message: err.detail.unwrap_or_else(|| {
                                        "Pi did not take the steering message.".into()
                                    }),
                                });
                            }
                        }
                        // Pi may still hold the message; a prompt waits for the clear, so
                        // failing to clear must stop the session rather than send it.
                        Purpose::Strand { .. } => self.failure = Some(err),
                        _ => self.notice(format!("Pi could not provide {command}.")),
                    }
                } else {
                    match pending.purpose {
                        Purpose::State => {
                            if !self.state(&data, pending.activity_revision) {
                                self.failure.get_or_insert_with(|| {
                                    error("Pi returned malformed or inconsistent session state.")
                                });
                            }
                        }
                        Purpose::Models => {
                            if !self.metadata.models(&data) {
                                self.notice("Pi returned an invalid model list.");
                            }
                        }
                        Purpose::Levels => {
                            if !self.metadata.levels(&data) {
                                if self.setting.is_some() {
                                    self.failure = Some(error(
                                        "Pi returned invalid thinking levels after changing a setting.",
                                    ));
                                } else {
                                    self.notice("Pi returned invalid thinking levels.");
                                }
                            }
                        }
                        Purpose::Commands => {
                            if !self.setup.mcp_servers.is_empty()
                                && !data["commands"].as_array().is_some_and(|commands| {
                                    commands
                                        .iter()
                                        .any(|c| c["name"] == "ghost-shared-mcp-ready")
                                })
                            {
                                if self
                                    .setup
                                    .mcp_servers
                                    .iter()
                                    .any(|server| !crate::mcp::managed(server))
                                {
                                    self.failure = Some(error(
                                        "Pi did not load Ghost's shared MCP extension. Update Pi to a compatible version and restart; no prompt was sent.",
                                    ));
                                } else {
                                    self.setup.mcp_servers.clear();
                                    self.notice("Project memory context remains available. Pi did not load the optional memory MCP tools.");
                                }
                            }
                            self.outbox
                                .push_back(DriverEvent::Commands(metadata::commands(&data)));
                            self.outbox.push_back(DriverEvent::Loaded(
                                ghost_protocol::session::SessionLoaded {
                                    skills: metadata::skills(&data),
                                    ..Default::default()
                                }
                                .bounded(),
                            ));
                        }
                        Purpose::Stats => {
                            if self.stats_dirty {
                                self.stats_dirty = false;
                                self.stats();
                            } else if data.get("cost").is_some()
                                || data.get("contextUsage").is_some()
                            {
                                self.usage = metadata::usage(&data);
                                self.outbox.push_back(DriverEvent::Event {
                                    turn: None,
                                    event: GhostEvent::UsageUpdated(self.usage.clone()),
                                });
                            }
                        }
                        Purpose::Setting(reply) => {
                            self.setting = Some(Setting {
                                reply,
                                deadline: pending.deadline,
                            });
                            // Pi may clamp thinking levels. Acknowledgement follows authoritative reads.
                            self.request(
                                json!({"type":"get_state"}),
                                Purpose::State,
                                pending.startup,
                                Some(pending.deadline),
                            );
                            self.request(
                                json!({"type":"get_available_thinking_levels"}),
                                Purpose::Levels,
                                pending.startup,
                                Some(pending.deadline),
                            );
                        }
                        Purpose::Prompt => match wire::text(&data, "disposition") {
                            Some("started" | "queued") => {}
                            Some("handled") => {
                                self.finish(StopReason::EndTurn, None);
                                self.idle(!self.run_active && !self.compacting);
                                if !self.stopping {
                                    self.refresh();
                                }
                            }
                            _ => {
                                self.failure =
                                    Some(error("Pi returned an unknown prompt disposition."))
                            }
                        },
                        Purpose::Steer { turn, reply } => {
                            let current = self.run_active
                                && !self.interrupted
                                && self.turn.as_deref() == Some(turn.as_str());
                            let result = match wire::text(&data, "disposition") {
                                // An input handler took it, or Pi queued it for this run.
                                Some("handled") => SteerResult::Accepted,
                                Some("queued") if current => SteerResult::Accepted,
                                Some("queued") => {
                                    // The turn settled while Pi took the message: nothing
                                    // will read it now, so it is cleared, never kept for later.
                                    self.clear_stranded(false);
                                    SteerResult::Rejected {
                                        message: "The turn finished before Pi read this message, so it was not delivered.".into(),
                                    }
                                }
                                _ => SteerResult::Unconfirmed {
                                    message: "Pi returned an unknown steering outcome. It was not retried.".into(),
                                },
                            };
                            if let Some(reply) = reply {
                                let _ = reply.send(result);
                            }
                        }
                        Purpose::Strand { notify } => {
                            self.queued_steering = 0;
                            if notify && data["steering"].as_array().is_some_and(|s| !s.is_empty())
                            {
                                self.notice(STEER_DROPPED);
                            }
                        }
                        Purpose::Clear => {
                            self.queued_steering = 0;
                            if data["steering"].as_array().is_some_and(|s| !s.is_empty()) {
                                self.notice("Pi had not read your steering message yet, so stopping dropped it.");
                            }
                            self.query("abort", Purpose::Abort, false)
                        }
                        Purpose::Abort => {
                            // The pinned RPC contract waits for the session to become idle. A handled
                            // prompt or extension-only run need not emit another agent_settled.
                            self.run_active = false;
                            self.compacting = false;
                            self.activity_revision = self.activity_revision.wrapping_add(1);
                            self.finish(StopReason::Cancelled, None);
                            self.idle(true);
                            self.interrupted = false;
                        }
                    }
                }
                if !self
                    .pending
                    .values()
                    .any(|p| matches!(p.purpose, Purpose::State | Purpose::Levels))
                    && let Some(setting) = self.setting.take()
                {
                    self.outbox
                        .push_back(DriverEvent::Options(self.metadata.options()));
                    if let Some(reply) = setting.reply {
                        let _ = reply.send(self.failure.clone().map_or(Ok(()), Err));
                    }
                }
                self.startup();
                if self.ready
                    && matches!(
                        command.as_str(),
                        "get_state" | "get_available_thinking_levels" | "get_available_models"
                    )
                {
                    self.outbox
                        .push_back(DriverEvent::Options(self.metadata.options()));
                }
            }
            wire::Incoming::Event(frame) => self.event(frame),
        }
    }
    fn event(&mut self, frame: Value) {
        if matches!(
            wire::text(&frame, "type"),
            Some(
                "agent_start"
                    | "agent_settled"
                    | "auto_retry_start"
                    | "compaction_start"
                    | "compaction_end"
            )
        ) {
            self.activity_revision = self.activity_revision.wrapping_add(1);
        }
        for event in self.translator.ingest(&frame) {
            self.emit(event);
        }
        if self.translator.is_exhausted() {
            self.failure = Some(error(
                "Pi exceeded the transcript tracking limit. Partial output was preserved; restart to continue.",
            ));
            return;
        }
        match wire::text(&frame, "type") {
            Some("agent_start" | "auto_retry_start") => {
                self.run_active = true;
                self.idle(false);
            }
            Some("compaction_start") => {
                self.compacting = true;
                self.idle(false);
            }
            Some("agent_end") => {} // Retry, compaction and queued work may still follow.
            Some("agent_settled") => {
                // Pi 1.0.2 emits this boundary without fields. Cancellation comes from our
                // interrupt or the completed assistant's stopReason, not an invented flag.
                let reason = if self.interrupted {
                    StopReason::Cancelled
                } else if self.run_failed {
                    StopReason::Error
                } else {
                    self.translator.reason()
                };
                let failure = if reason == StopReason::Error {
                    Some(DriverError {
                        kind: ErrorKind::AgentError,
                        message: "Pi could not finish the turn.".into(),
                        detail: self.last_error.take(),
                    })
                } else {
                    None
                };
                self.run_active = false;
                self.finish(reason, failure);
                self.idle(!self.compacting);
                self.interrupted = false;
                // Pi runs again for steering it holds before settling, so any left now came too
                // late for this turn.
                self.clear_stranded(true);
                if !self.stopping {
                    self.refresh();
                }
            }
            Some("message_end") if frame["message"]["role"] == "assistant" => {
                self.last_error = frame["message"]["errorMessage"].as_str().map(str::to_owned);
            }
            Some("thinking_level_changed") => {
                if let Some(level) = wire::text(&frame, "level") {
                    self.metadata.level = Some(level.into());
                    self.outbox
                        .push_back(DriverEvent::Options(self.metadata.options()));
                }
            }
            Some("session_info_changed") => self.outbox.push_back(DriverEvent::Title(
                frame["name"].as_str().map(str::to_owned),
            )),
            Some("compaction_end") => {
                // Manual compaction is independent of the agent loop and does not emit
                // agent_settled. Automatic compaction keeps run_active until that boundary.
                self.compacting = false;
                self.idle(!self.run_active);
                self.usage.context_used = None;
                self.outbox.push_back(DriverEvent::Event {
                    turn: None,
                    event: GhostEvent::UsageUpdated(self.usage.clone()),
                });
                if !self.stopping {
                    self.stats();
                }
            }
            Some("auto_retry_end") => {
                self.run_failed = frame["success"] == false;
                if self.run_failed {
                    self.last_error = frame["finalError"].as_str().map(str::to_owned);
                    self.notice("Pi exhausted its automatic retries.");
                }
            }
            Some("extension_error") => self.notice("A Pi extension reported an error."),
            Some("queue_update") => {
                self.queued_steering = frame["steering"].as_array().map_or(0, Vec::len);
                if !self.run_active {
                    self.clear_stranded(false);
                }
            }
            _ => {}
        }
    }
    fn command(&mut self, command: DriverCommand) {
        log_command(&self.log, &command);
        match command {
            DriverCommand::McpAction { reply, .. } => crate::mcp::unsupported(
                reply,
                "Pi RPC does not expose session MCP status or OAuth controls. Use Pi's /mcp native panel; restart this idle chat to apply shared definitions.",
            ),
            DriverCommand::Interrupt => self.interrupt(),
            DriverCommand::RespondUi { request_id, answer } => {
                if let Some(dialog) = self.dialogs.remove(&request_id) {
                    self.send(ui::response(&dialog.frame, answer));
                }
            }
            DriverCommand::EditPlan { .. } | DriverCommand::RespondPermission { .. } => {}
            DriverCommand::Steer {
                turn_id,
                parts,
                reply,
                ..
            } => self.steer(turn_id, &parts, reply),
            DriverCommand::RewindFiles { reply, .. } => {
                let _ = reply.send(Err(error(
                    "Pi does not support file rewind through this driver.",
                )));
            }
            other => self.held.push_back((Instant::now(), other)),
        }
    }
    fn dispatch(&mut self, arrived: Instant, command: DriverCommand) {
        match command {
            DriverCommand::Prompt { turn, parts } => {
                if self.turn.is_some() {
                    self.outbox.push_back(DriverEvent::TurnEnded {
                        turn_id: turn.turn_id,
                        stop_reason: StopReason::Error,
                        usage: None,
                        error: Some(error("Pi is already processing a prompt.")),
                    });
                    return;
                }
                let request =
                    match crate::attachments::pi(&parts, self.setup.attachment_dir.as_deref()) {
                        Ok(request) => request,
                        Err(message) => {
                            self.outbox.push_back(DriverEvent::TurnEnded {
                                turn_id: turn.turn_id,
                                stop_reason: StopReason::Error,
                                usage: None,
                                error: Some(error(message)),
                            });
                            return;
                        }
                    };
                self.translator.begin_turn();
                self.last_error = None;
                self.run_failed = false;
                self.turn = Some(turn.turn_id);
                self.interrupted = false;
                self.idle(false);
                self.outbox
                    .push_back(DriverEvent::Status(DriverStatus::Prompting));
                self.request(request, Purpose::Prompt, false, None);
            }
            DriverCommand::SetOption {
                option_id,
                value,
                reply,
            } => {
                if reply.is_closed() {
                    return;
                }
                if arrived + self.setup.timings.request <= Instant::now() {
                    let _ = reply.send(Err(setting_error()));
                    return;
                }
                if let Some(request) = self.metadata.request(&option_id, &value) {
                    self.request(
                        request,
                        Purpose::Setting(Some(reply)),
                        false,
                        Some(arrived + self.setup.timings.request),
                    );
                } else {
                    let _ = reply.send(Err(error("Pi does not offer this setting value.")));
                }
            }
            DriverCommand::SetAutonomy {
                autonomy: requested,
                reply,
            } => {
                let _ = reply.send(if requested == Autonomy::FullAuto {
                    Ok(autonomy())
                } else {
                    Err(error("Pi RPC cannot enforce Ask or Auto edit."))
                });
            }
            _ => {}
        }
    }
    fn expire(&mut self) {
        let now = Instant::now();
        if !self.ready
            && !self.stopping
            && (now >= self.init_deadline
                || now >= self.last_activity + self.setup.timings.open_idle)
        {
            self.failure
                .get_or_insert_with(|| error("Pi did not finish starting before its deadline."));
        }
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(pending) = self.pending.remove(&id) {
                match pending.purpose {
                    Purpose::Setting(reply) => {
                        if let Some(reply) = reply {
                            let _ = reply.send(Err(setting_error()));
                        }
                        self.failure = Some(error(
                            "Pi's setting outcome is unknown. Restart the session to reconcile it.",
                        ));
                    }
                    Purpose::State | Purpose::Levels
                        if pending.startup || self.setting.is_some() =>
                    {
                        self.failure = Some(error("Pi session state timed out."))
                    }
                    Purpose::Prompt | Purpose::Clear | Purpose::Abort | Purpose::Strand { .. } => {
                        self.failure = Some(error("Pi did not acknowledge a required command."))
                    }
                    Purpose::Steer { reply, .. } => {
                        if let Some(reply) = reply {
                            let _ = reply.send(SteerResult::Unconfirmed {
                                message:
                                    "Pi did not confirm the steering message. It was not retried."
                                        .into(),
                            });
                        }
                    }
                    _ => self.notice(format!(
                        "Pi did not answer {} before its deadline.",
                        pending.command
                    )),
                }
            }
        }
        if self.setting.as_ref().is_some_and(|s| s.deadline <= now)
            && let Some(setting) = self.setting.take()
        {
            if let Some(reply) = setting.reply {
                let _ = reply.send(Err(setting_error()));
            }
            self.failure = Some(error(
                "Pi's applied setting could not be confirmed. Restart the session.",
            ));
        }
        let held = std::mem::take(&mut self.held);
        for (arrived, command) in held {
            if is_config(&command) && arrived + self.setup.timings.request <= now {
                match command {
                    DriverCommand::SetOption { reply, .. } => {
                        let _ = reply.send(Err(setting_error()));
                    }
                    DriverCommand::SetAutonomy { reply, .. } => {
                        let _ = reply.send(Err(setting_error()));
                    }
                    _ => {}
                }
            } else {
                self.held.push_back((arrived, command));
            }
        }
        let ids: Vec<_> = self
            .dialogs
            .iter()
            .filter(|(_, d)| d.deadline.is_some_and(|t| t <= now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(dialog) = self.dialogs.remove(&id) {
                self.send(ui::response(&dialog.frame, None));
                self.outbox
                    .push_back(DriverEvent::UiWithdrawn { request_id: id });
            }
        }
        self.startup();
    }
}

pub(super) async fn run(ctx: DriverContext) -> DriverExit {
    let DriverContext {
        mut commands,
        events,
        spawner,
        setup,
    } = ctx;
    let native_id = match native_id(&setup) {
        Ok(id) => id,
        Err(err) => return DriverExit::StartFailed(err),
    };
    let mut native_launch = launch(&setup, &native_id);
    let _mcp_extension = if !setup.mcp_servers.is_empty() {
        use std::io::Write;
        if !setup.mcp_servers.iter().all(super::shared_mcp_supported) {
            return DriverExit::StartFailed(error(
                "Pi supports stdio and HTTP MCP with letters, numbers, underscores and hyphens in server names.",
            ));
        }
        let mut file = match tempfile::Builder::new()
            .prefix("ghost-shared-mcp-")
            .suffix(".ts")
            .tempfile()
        {
            Ok(file) => file,
            Err(_) => {
                return DriverExit::StartFailed(error(
                    "Ghost could not prepare Pi's session MCP extension.",
                ));
            }
        };
        if file.write_all(super::SHARED_MCP_EXTENSION).is_err() {
            return DriverExit::StartFailed(error(
                "Ghost could not write Pi's session MCP extension.",
            ));
        }
        native_launch
            .args
            .extend(["--extension".into(), file.path().as_os_str().to_owned()]);
        native_launch.set_env.push((
            "GHOST_SHARED_MCP".into(),
            super::shared_mcp_json(&setup.mcp_servers).into(),
        ));
        Some(file)
    } else {
        None
    };
    let spec = SpawnSpec {
        launch: native_launch,
        folder: setup.folder.clone(),
        env: Arc::clone(&setup.env),
        max_line_bytes: MAX_LINE_BYTES,
    };
    let mut spawning = spawner.spawn(spec);
    let mut early = VecDeque::from([DriverEvent::Status(DriverStatus::Spawning)]);
    let mut held = VecDeque::new();
    let process = loop {
        let bytes = early.front().map_or(0, DriverEvent::approx_bytes);
        tokio::select! {
            command=commands.recv()=>match command {None|Some(DriverCommand::Shutdown {..})=>return DriverExit::Closed,Some(command)=>held.push_back((Instant::now(),command))},
            permit=events.reserve(bytes),if !early.is_empty()=>{let Ok(permit)=permit else {return DriverExit::Closed;};if let Some(event)=early.pop_front() {permit.send(event);}},
            result=&mut spawning=>match result {Ok(process)=>break process,Err(err)=>return DriverExit::StartFailed(crate::acp::spawn_failure(&err))},
        }
    };
    let mut start = setup.start_note(&process.spawn_id);
    start["native_session_id"] = json!(native_id);
    process.log.note(LogNote::Start(start));
    for (_, command) in &held {
        log_command(&process.log, command);
    }
    let desired = setup
        .desired
        .iter()
        .filter(|(key, _)| key.as_str() == "model")
        .chain(
            setup
                .desired
                .iter()
                .filter(|(key, _)| key.as_str() != "model"),
        )
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let deadline = Instant::now() + setup.timings.open_ceiling.min(setup.launch.startup_timeout);
    let mut s = Session {
        setup,
        sink: process.io.stdin,
        log: process.log,
        spawn_id: process.spawn_id.clone(),
        native_id,
        next_id: 0,
        pending: BTreeMap::new(),
        dialogs: BTreeMap::new(),
        metadata: Metadata::default(),
        translator: translate::Translator::new(process.spawn_id),
        outbox: early,
        held,
        before_ready: VecDeque::new(),
        desired,
        setting: None,
        ready: false,
        state_seen: false,
        idle: false,
        run_active: false,
        compacting: false,
        activity_revision: 0,
        turn: None,
        interrupted: false,
        init_deadline: deadline,
        last_activity: Instant::now(),
        failure: None,
        stopping: false,
        stats_dirty: false,
        usage: UsageInfo::default(),
        last_error: None,
        run_failed: false,
        queued_steering: 0,
    };
    s.outbox
        .push_back(DriverEvent::Status(DriverStatus::Initializing));
    for (name, purpose) in [
        ("get_state", Purpose::State),
        ("get_available_models", Purpose::Models),
        ("get_available_thinking_levels", Purpose::Levels),
        ("get_commands", Purpose::Commands),
        ("get_session_stats", Purpose::Stats),
    ] {
        s.query(name, purpose, true);
    }
    let control = process.control;
    let stdin_observer = s.sink.clone();
    let mut stdout = crate::incoming_images::ImageLineSource::new(
        process.io.stdout,
        s.setup.attachment_writer.clone(),
    );
    let mut exit = control.exited();
    let mut exited = None;
    let mut process_done = false;
    let mut eof = false;
    let mut requested_close = false;
    let mut closing: Option<(Instant, ShutdownPlan)> = None;
    let mut shutdown: Option<BoxFuture<'static, ghost_process::ShutdownReport>> = None;
    let mut drain_used = Duration::ZERO;
    let mut drain_since: Option<Instant> = None;
    loop {
        if s.failure.is_some() && closing.is_none() && shutdown.is_none() && !process_done {
            if s.ready
                && let Some(failure) = &s.failure
            {
                s.outbox.push_back(DriverEvent::Event {
                    turn: None,
                    event: GhostEvent::Error {
                        kind: failure.kind,
                        message: failure.message.clone(),
                        detail: failure.detail.clone(),
                        recoverable: true,
                    },
                });
            }
            closing = Some((Instant::now(), ShutdownPlan::STANDARD));
        }
        if let Some((due, plan)) = closing
            && (Instant::now() >= due || s.idle)
        {
            s.stopping = true;
            closing = None;
            s.withdraw(None);
            s.pending.clear();
            s.held.clear();
            s.sink.close();
            shutdown = Some(control.shutdown(plan));
        }
        if eof && process_done && s.outbox.is_empty() {
            break;
        }
        while s.ready
            && s.idle
            && !s.configuring()
            && !eof
            && !requested_close
            && !s.stopping
            && !process_done
            && s.failure.is_none()
            && let Some((arrived, command)) = s.held.pop_front()
        {
            s.dispatch(arrived, command);
        }
        let bytes = s.outbox.front().map_or(0, DriverEvent::approx_bytes);
        let full = s.outbox.len() >= 64
            || s.outbox
                .iter()
                .map(DriverEvent::approx_bytes)
                .sum::<usize>()
                >= 8 * 1024 * 1024;
        match (process_done && !eof && !full, drain_since) {
            (true, None) => drain_since = Some(Instant::now()),
            (false, Some(since)) => {
                drain_used += since.elapsed();
                drain_since = None;
            }
            _ => {}
        }
        let drain_deadline =
            drain_since.map(|since| since + PIPE_EOF_GRACE.saturating_sub(drain_used));
        let deadline = s
            .pending
            .values()
            .map(|p| p.deadline)
            .chain(s.dialogs.values().filter_map(|d| d.deadline))
            .chain(
                s.held
                    .iter()
                    .filter(|(_, c)| is_config(c))
                    .map(|(arrived, _)| *arrived + s.setup.timings.request),
            )
            .chain(
                (!s.ready && s.failure.is_none() && !requested_close).then_some(
                    s.init_deadline
                        .min(s.last_activity + s.setup.timings.open_idle),
                ),
            )
            .chain(s.setting.as_ref().map(|setting| setting.deadline))
            .chain(closing.map(|c| c.0))
            .chain(drain_deadline.filter(|_| !full && !eof))
            .min()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400));
        tokio::select! {
            ()=stdin_observer.writer_closed(),if !s.stopping&&!process_done&&s.failure.is_none()=>{
                if let Some(info)=control.try_exited() {exited=Some(info);process_done=true;}
                else {s.failure=Some(error("Pi stopped accepting input. Restart the session to continue."));}
            },
            command=commands.recv(),if !requested_close&&s.failure.is_none()&&!s.stopping&&!process_done=>match command {
                None|Some(DriverCommand::Shutdown {..})=>{
                    let plan=match command {Some(DriverCommand::Shutdown {plan})=>plan,_=>StopPlan {cancel_grace:Duration::ZERO,process:ShutdownPlan::STANDARD}};
                    s.log.command(json!({"type":"shutdown"}));requested_close=true;s.stopping=true;
                    s.outbox.push_back(DriverEvent::Status(DriverStatus::Closing));if !s.idle {s.interrupt();}
                    closing=Some((Instant::now()+plan.cancel_grace,plan.process));
                },Some(command)=>s.command(command),
            },
            permit=events.reserve(bytes),if !s.outbox.is_empty()=>{let Ok(permit)=permit else {control.kill_tree();return DriverExit::Closed;};if let Some(event)=s.outbox.pop_front() {permit.send(event);}},
            item=stdout.next(),if !eof&&!full=>match item {
                Some(StdoutItem::Line(line))=>s.incoming(wire::parse_line(&line)),
                Some(StdoutItem::Oversized {..})=>s.failure=Some(error("Pi sent an oversized RPC frame; partial output was preserved.")),
                None=>{eof=true;if !process_done&&shutdown.is_none() {closing=Some((Instant::now()+Duration::from_secs(2),ShutdownPlan::FORCE));}}
            },
            info=&mut exit,if !process_done=>{exited=Some(info);process_done=true;},
            report=async {if let Some(task)=&mut shutdown {Some(task.await)} else {None}},if shutdown.is_some()=>{
                shutdown=None;process_done=true;if let Some(report)=report {exited=exited.or(report.exit);}
            },
            _=tokio::time::sleep_until(deadline)=>{s.expire();if drain_deadline.is_some_and(|due|Instant::now()>=due)&&!full {eof=true;}},
        }
    }
    if requested_close {
        DriverExit::Closed
    } else if !s.ready {
        let mut err = s
            .failure
            .unwrap_or_else(|| error("Pi exited before opening the session."));
        let tail = control.stderr_tail(STDERR_TAIL_BYTES);
        if err.detail.is_none() && !tail.trim().is_empty() {
            err.detail = Some(tail);
        }
        DriverExit::StartFailed(err)
    } else {
        DriverExit::Crashed {
            exit: exited,
            stderr_tail: control.stderr_tail(STDERR_TAIL_BYTES),
        }
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;
    #[test]
    fn fresh_resume_and_fork_args_preserve_the_native_identity_contract() {
        let mut setup = super::super::tests::setup();
        let fresh = native_id(&setup).unwrap();
        assert_ne!(fresh, setup.chat_id);
        let fresh_launch = launch(&setup, &fresh);
        assert!(
            fresh_launch
                .args
                .windows(2)
                .any(|w| w[0] == "--session-id" && w[1] == fresh.as_str())
        );
        setup.resume = Some("source-id".into());
        let resumed = launch(&setup, &native_id(&setup).unwrap());
        assert!(
            resumed
                .args
                .windows(2)
                .any(|w| w[0] == "--session" && w[1] == "source-id")
        );
        assert!(!resumed.args.iter().any(|a| a == "--session-id"));
        setup.native_start.fork = true;
        let fork = native_id(&setup).unwrap();
        let fork_launch = launch(&setup, &fork);
        assert!(
            fork_launch
                .args
                .windows(2)
                .any(|w| w[0] == "--fork" && w[1] == "source-id")
        );
        assert!(
            fork_launch
                .args
                .windows(2)
                .any(|w| w[0] == "--session-id" && w[1] == fork.as_str())
        );
        assert_eq!(setup.resume.as_deref(), Some("source-id"));
    }
    #[test]
    fn malformed_or_unsupported_startup_choices_fail_before_spawn() {
        let mut setup = super::super::tests::setup();
        setup.autonomy = Autonomy::Ask;
        assert!(native_id(&setup).is_err());
        setup.autonomy = Autonomy::FullAuto;
        setup.native_start.new_session_id = Some("../other".into());
        assert!(native_id(&setup).is_err());
        setup.native_start.new_session_id = None;
        setup.native_start.fork = true;
        assert!(native_id(&setup).is_err());
        setup.native_start.fork = false;
        setup.native_start.file_checkpointing = true;
        assert!(native_id(&setup).is_err());
    }
}
