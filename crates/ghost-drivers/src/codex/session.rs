//! One owner for protocol correlation, callback obligations, deadlines and process shutdown.
use super::{options, requests, translate, wire};
use crate::driver::{
    DriverCommand, DriverContext, DriverError, DriverEvent, DriverExit, DriverStatus,
    LoggedCommand, PermissionOutcome, ReadyInfo, ResumeOutcome, SessionSetup, SteerResult,
};
use crate::jsonrpc::{self, Incoming, RpcError, RpcOut};
use futures::future::BoxFuture;
use ghost_process::{
    LogNote, MAX_LINE_BYTES, PIPE_EOF_GRACE, ProtocolLog, STDERR_TAIL_BYTES, ShutdownPlan,
    SpawnSpec, StdoutItem,
};
use ghost_protocol::session::{AuthCommand, AuthPrompt, ConfigValue};
use ghost_protocol::{
    Autonomy, Capabilities, ErrorKind, GhostEvent, LimitWindow, NoticeLevel, StopReason,
    TokenUsage, UiAnswer, UiPrompt, UiRequest, UsageScope,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::time::Instant;

fn error(kind: ErrorKind, message: impl Into<String>) -> DriverError {
    DriverError {
        kind,
        message: message.into(),
        detail: None,
    }
}
fn protocol_error(message: &str) -> DriverError {
    error(ErrorKind::ProtocolError, message)
}
fn log_command(log: &ProtocolLog, command: &DriverCommand) {
    if let Ok(value) = serde_json::to_value(LoggedCommand::from(command)) {
        log.command(value);
    }
}

fn auth_error(value: &Value) -> bool {
    let code = &value["codexErrorInfo"];
    code == "unauthorized"
        || [
            "httpConnectionFailed",
            "responseStreamConnectionFailed",
            "responseStreamDisconnected",
            "responseTooManyFailedAttempts",
        ]
        .iter()
        .any(|variant| code[*variant]["httpStatusCode"] == 401)
}

fn native_version(user_agent: &str) -> Option<String> {
    let desktop = user_agent
        .strip_prefix("Codex Desktop/")
        .and_then(|tail| tail.split_whitespace().next());
    desktop
        .into_iter()
        .chain(user_agent.split_whitespace().filter_map(|part| {
            part.strip_prefix("codex_cli_rs/")
                .or_else(|| part.strip_prefix("codex/"))
        }))
        .find_map(|version| {
            let fields: Vec<_> = version.split('.').collect();
            (fields.len() == 3
                && fields.iter().all(|field| {
                    !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit())
                }))
            .then(|| version.into())
        })
}

enum Purpose {
    Mcp {
        server_name: String,
        action: ghost_protocol::mcp::McpSessionAction,
        reply:
            tokio::sync::oneshot::Sender<Result<ghost_protocol::mcp::McpActionResult, DriverError>>,
    },
    Initialize,
    Open,
    Models,
    Collaboration,
    Start(String),
    Steer {
        native_turn: String,
        reply: tokio::sync::oneshot::Sender<SteerResult>,
    },
    Interrupt,
    Limits,
}
struct Pending {
    purpose: Purpose,
    deadline: Instant,
}
struct ActiveTurn {
    aura_id: String,
    native_id: Option<String>,
    interrupt: bool,
    usage: Option<TokenUsage>,
}
struct Callback {
    id: Value,
    turn: Option<String>,
    native_turn: Option<String>,
    body: requests::Callback,
}

struct ModelRecovery {
    failure: DriverError,
    request_id: Option<String>,
    model: Option<String>,
}

struct Session {
    rpc: RpcOut,
    log: ProtocolLog,
    setup: SessionSetup,
    spawn_id: String,
    pending: BTreeMap<i64, Pending>,
    callbacks: BTreeMap<String, Callback>,
    next_callback: u64,
    outbox: VecDeque<DriverEvent>,
    held: VecDeque<DriverCommand>,
    deferred: VecDeque<(String, Value)>,
    translator: translate::Translator,
    thread: Option<String>,
    native_turn: Option<String>,
    active: Option<ActiveTurn>,
    idle: bool,
    ready: bool,
    opened: bool,
    models_done: bool,
    model_recovery: Option<ModelRecovery>,
    model_recovery_attempted: bool,
    models: Vec<wire::Model>,
    cursors: BTreeSet<String>,
    model: String,
    effort: Option<String>,
    workflow: String,
    collaboration_modes: BTreeSet<String>,
    /// Its MCP servers states as it reported them starting (`mcpServer/startupStatus/updated`).
    mcp_states: Vec<ghost_protocol::session::LoadedMcpServer>,
    version: Option<String>,
    limits: BTreeMap<String, Value>,
    limits_dirty: bool,
    failure: Option<DriverError>,
    stopping: bool,
    init_deadline: Instant,
}

impl Session {
    fn failed_write(&mut self) {
        self.failure
            .get_or_insert_with(|| error(ErrorKind::AgentExited, "Codex closed its input."));
    }
    fn request(&mut self, method: &str, params: Value, purpose: Purpose) {
        match self.rpc.request(method, params) {
            Ok(id) => {
                self.pending.insert(
                    id,
                    Pending {
                        deadline: Instant::now()
                            + if matches!(&purpose, Purpose::Initialize | Purpose::Open) {
                                self.setup.timings.open_idle
                            } else {
                                self.setup.timings.request
                            },
                        purpose,
                    },
                );
            }
            Err(_) => self.failed_write(),
        }
    }
    fn respond(&mut self, id: Value, result: Value) {
        if self.rpc.respond(&id, result).is_err() {
            self.failed_write();
        }
    }
    fn reject(&mut self, id: Value, message: &str) {
        if self
            .rpc
            .respond_error(&id, jsonrpc::codes::INVALID_PARAMS, message)
            .is_err()
        {
            self.failed_write();
        }
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
    fn fail(&mut self, failure: DriverError) {
        if self.failure.is_some() {
            return;
        }
        if self.ready {
            self.outbox.push_back(DriverEvent::Event {
                turn: None,
                event: GhostEvent::Error {
                    kind: failure.kind,
                    message: failure.message.clone(),
                    detail: failure.detail.clone(),
                    recoverable: false,
                },
            });
        }
        self.failure = Some(failure);
    }
    fn authentication_required(&mut self) {
        self.outbox.push_back(DriverEvent::AuthRequired(AuthPrompt {
            message: "Codex needs you to sign in again.".into(),
            commands: vec![AuthCommand {
                label: "Sign in to Codex".into(),
                argv: vec![
                    self.setup.launch.program.to_string_lossy().into_owned(),
                    "login".into(),
                ],
                env: Vec::new(),
            }],
        }));
        self.fail(error(
            ErrorKind::AuthRequired,
            "Codex needs you to sign in again.",
        ));
    }
    fn options(&mut self) {
        let mut options = options::options(&self.models, &self.model, self.effort.as_deref());
        if self.collaboration_modes.contains("plan") && self.collaboration_modes.contains("default")
        {
            options.push(crate::controls::workflow(&self.workflow));
        }
        self.outbox.push_back(DriverEvent::Options(options));
    }
    fn run_state(&mut self, idle: bool) {
        self.idle = idle;
        self.outbox.push_back(DriverEvent::RunState { idle });
    }
    fn ready(&mut self) {
        if self.ready
            || !self.opened
            || !self.models_done
            || self.failure.is_some()
            || self.stopping
        {
            return;
        }
        self.ready = true;
        self.run_state(self.idle);
        self.options();
        self.outbox
            .push_back(DriverEvent::Autonomy(options::autonomy(
                self.setup.autonomy,
            )));
        self.outbox.push_back(DriverEvent::Ready(ReadyInfo {
            native_session_id: self.thread.clone(),
            agent_name: Some("Codex".into()),
            agent_version: self.version.clone(),
            spawn_id: self.spawn_id.clone(),
            replay: None,
            resumed: if self.setup.resume.is_some() {
                ResumeOutcome::Resumed
            } else {
                ResumeOutcome::New
            },
            capabilities: Capabilities {
                steer: true,
                images: true,
                interrupt: true,
                resume: true,
                set_model: !self.models.is_empty(),
                set_effort: !self.models.is_empty(),
                permissions: true,
                plans: true,
                usage: true,
                limits: true,
                autonomy_levels: vec![Autonomy::FullAuto, Autonomy::AutoEdit, Autonomy::Ask],
                ..Default::default()
            },
        }));
        self.request("collaborationMode/list", json!({}), Purpose::Collaboration);
        self.refresh_limits();
        while let Some((method, params)) = self.deferred.pop_front() {
            self.notification(&method, params);
        }
    }
    fn open(&mut self) {
        if self.setup.mcp_servers.iter().any(|s| {
            matches!(
                s.definition.transport,
                ghost_protocol::mcp::McpTransport::Sse { .. }
            )
        }) {
            self.fail(protocol_error("Codex does not support legacy SSE MCP servers. Choose streamable HTTP or stdio before restarting."));
            return;
        }
        let (approval, sandbox, _) = options::policies(self.setup.autonomy);
        let mut params = json!({"cwd":self.setup.agent_folder,"approvalPolicy":approval,"approvalsReviewer":"user","sandbox":sandbox});
        if !self.setup.mcp_servers.is_empty() {
            params["config"] =
                json!({"mcp_servers":crate::mcp::codex_session(&self.setup.mcp_servers)});
        }
        if let Some(ConfigValue::Choice { value }) = self.setup.desired.get("model") {
            params["model"] = json!(value);
        }
        let method = if let Some(thread) = &self.setup.resume {
            params["threadId"] = json!(thread);
            params["excludeTurns"] = json!(false);
            "thread/resume"
        } else {
            "thread/start"
        };
        self.outbox
            .push_back(DriverEvent::Status(DriverStatus::Opening));
        self.request(method, params, Purpose::Open);
        if self.models_done {
            return;
        }
        self.request(
            "model/list",
            json!({"includeHidden":false}),
            Purpose::Models,
        );
    }
    fn waiting_for_model_recovery(&self) -> bool {
        self.model_recovery
            .as_ref()
            .is_some_and(|recovery| recovery.request_id.is_some())
    }
    fn offer_model_recovery(&mut self) {
        if !self.models_done || self.waiting_for_model_recovery() {
            return;
        }
        let Some(mut recovery) = self.model_recovery.take() else {
            return;
        };
        let defaults = self
            .models
            .iter()
            .filter(|model| model.is_default)
            .collect::<Vec<_>>();
        let Some(default) = defaults.first().filter(|_| defaults.len() == 1) else {
            self.fail(recovery.failure);
            return;
        };
        if matches!(self.setup.desired.get("model"), Some(ConfigValue::Choice { value }) if value == &default.model || value == &default.id)
        {
            self.fail(recovery.failure);
            return;
        }
        let model = default.model.clone();
        let request_id = format!("{}.model-recovery", self.spawn_id);
        let reason = recovery
            .failure
            .message
            .chars()
            .take(1536)
            .collect::<String>();
        let shown = model.chars().take(256).collect::<String>();
        recovery.request_id = Some(request_id.clone());
        recovery.model = Some(model);
        self.model_recovery = Some(recovery);
        self.outbox.push_back(DriverEvent::UiRequest { turn: None, request: UiRequest {
            request_id,
            prompt: UiPrompt::Select {
                message: format!("Codex could not open this session: {reason}\n\nYou can try its advertised default model, {shown}. This may not fix the reported error. Your saved thread and queued prompts stay intact. Only opening is retried; no prompt is resent. The model changes only after opening succeeds."),
                options: vec![format!("Use {shown} and retry opening once"), "Keep current model and stop".into()],
            },
        }});
    }
    fn answer_model_recovery(&mut self, request_id: &str, answer: &Option<UiAnswer>) -> bool {
        if !self
            .model_recovery
            .as_ref()
            .is_some_and(|recovery| recovery.request_id.as_deref() == Some(request_id))
        {
            return false;
        }
        if !matches!(answer, None | Some(UiAnswer::Select { index: 0 | 1 })) {
            return true;
        }
        let Some(recovery) = self.model_recovery.take() else {
            return true;
        };
        // Core has already resolved a user answer. UiWithdrawn is reserved for an independent
        // cancellation; emitting it here would falsely report that the answer lost a race.
        if !matches!(answer, Some(UiAnswer::Select { index: 0 })) {
            self.fail(recovery.failure);
            return true;
        }
        let Some(model) = recovery.model else {
            self.fail(recovery.failure);
            return true;
        };
        self.model_recovery_attempted = true;
        self.setup.desired.insert(
            "model".into(),
            ConfigValue::Choice {
                value: model.clone(),
            },
        );
        self.setup.desired.remove("effort");
        self.notice(format!("You chose Codex's advertised default model, {model}. Retrying session opening once; saved prompts remain queued until it succeeds."));
        self.init_deadline = Instant::now()
            + self
                .setup
                .timings
                .open_ceiling
                .min(self.setup.launch.startup_timeout);
        self.open();
        true
    }
    fn incoming(&mut self, incoming: Incoming) {
        match incoming {
            Incoming::Response { id, result } => {
                if let Some(id) = id.as_i64() {
                    self.response(id, result);
                }
            }
            Incoming::Notification { method, params } => self.notification(&method, params),
            Incoming::Request { id, method, params } => self.callback(id, &method, params),
            Incoming::Invalid { .. } => {
                self.notice("Codex sent a malformed protocol message; it was ignored.")
            }
        }
    }
    fn response(&mut self, id: i64, result: Result<Value, RpcError>) {
        let Some(pending) = self.pending.remove(&id) else {
            return;
        };
        if let Purpose::Mcp {
            server_name,
            action,
            reply,
        } = pending.purpose
        {
            let answer=result.map_err(|_|protocol_error("Codex rejected the MCP session operation. Inspect its native MCP diagnostics.")).and_then(|value|crate::mcp::response(&value,action,&server_name,true));
            let _ = reply.send(answer);
            return;
        }
        // Correlate a reply even after settlement or Stop. The native exact-turn precondition
        // prevents late delivery to another turn; local lifecycle changes cannot prove delivery.
        if let Purpose::Steer { native_turn, reply } = pending.purpose {
            let outcome = match result {
                Ok(value) if value["turnId"].as_str() == Some(native_turn.as_str()) => {
                    SteerResult::Accepted
                }
                Ok(_) => {
                    self.fail(protocol_error("Codex returned an invalid steering acknowledgement. Delivery is unconfirmed."));
                    SteerResult::Unconfirmed {
                        message: "Codex did not confirm delivery to the selected turn.".into(),
                    }
                }
                Err(err) => SteerResult::Rejected {
                    message: err.message,
                },
            };
            let _ = reply.send(outcome);
            return;
        }
        if self.stopping && !matches!(pending.purpose, Purpose::Start(_) | Purpose::Interrupt) {
            return;
        }
        let value = match result {
            Ok(value) => value,
            Err(err) => {
                if err.data.as_ref().is_some_and(auth_error) {
                    self.authentication_required();
                    return;
                }
                match pending.purpose {
                    Purpose::Open if !self.model_recovery_attempted => {
                        self.model_recovery = Some(ModelRecovery {
                            failure: error(ErrorKind::AgentError, err.message),
                            request_id: None,
                            model: None,
                        });
                        self.offer_model_recovery();
                    }
                    Purpose::Collaboration => {
                        self.notice(
                            "Codex could not list native collaboration modes. Plan is unavailable.",
                        );
                        if self.setup.desired.contains_key(crate::controls::WORKFLOW) {
                            self.fail(protocol_error(
                                "The saved native workflow could not be verified.",
                            ));
                        }
                    }
                    Purpose::Models => {
                        self.models.clear();
                        self.models_done = true;
                        self.notice(
                            "Codex could not list its models. The current model remains available.",
                        );
                        self.offer_model_recovery();
                        self.ready();
                    }
                    Purpose::Limits => {
                        self.notice("Codex could not refresh usage limits.");
                        if std::mem::take(&mut self.limits_dirty) {
                            self.refresh_limits();
                        }
                    }
                    Purpose::Start(turn) => {
                        if self.active.as_ref().is_some_and(|a| a.aura_id == turn) {
                            self.withdraw_turn(Some(&turn));
                            self.active = None;
                            self.outbox.push_back(DriverEvent::TurnEnded {
                                turn_id: turn,
                                stop_reason: StopReason::Error,
                                usage: None,
                                error: Some(error(ErrorKind::AgentError, err.message)),
                            });
                            // A rejected start never created a native turn.
                            self.run_state(self.native_turn.is_none());
                        }
                    }
                    _ => self.fail(error(ErrorKind::AgentError, err.message)),
                }
                return;
            }
        };
        match pending.purpose {
            Purpose::Mcp { .. } | Purpose::Steer { .. } => {
                unreachable!("session replies handled above")
            }
            Purpose::Initialize => {
                match serde_json::from_value::<wire::InitializeResponse>(value) {
                    Ok(info) if !info.user_agent.is_empty() => {
                        self.version = native_version(&info.user_agent);
                        if self.rpc.notify("initialized", json!({})).is_err() {
                            self.failed_write();
                            return;
                        }
                        self.open();
                    }
                    _ => self.fail(protocol_error(
                        "Codex returned an invalid initialization response.",
                    )),
                }
            }
            Purpose::Open => match serde_json::from_value::<wire::OpenResponse>(value) {
                Ok(open)
                    if !open.thread.id.is_empty()
                        && !matches!(
                            open.thread.status,
                            wire::ThreadStatus::Unknown
                                | wire::ThreadStatus::NotLoaded
                                | wire::ThreadStatus::SystemError
                        ) =>
                {
                    if open.approvals_reviewer != "user" {
                        self.fail(protocol_error("Codex did not keep approval decisions with the user. The owned session was stopped before accepting prompts."));
                        return;
                    }
                    if self
                        .setup
                        .resume
                        .as_ref()
                        .is_some_and(|id| *id != open.thread.id)
                    {
                        self.fail(protocol_error("Codex resumed a different thread."));
                        return;
                    }
                    self.thread = Some(open.thread.id);
                    self.idle = open.thread.status.idle();
                    self.native_turn = open
                        .thread
                        .turns
                        .iter()
                        .find(|t| t.status == wire::TurnStatus::InProgress)
                        .map(|t| t.id.clone());
                    self.model = open.model;
                    self.effort = match self.setup.desired.get("effort") {
                        Some(ConfigValue::Choice { value }) if value != "default" => {
                            Some(value.clone())
                        }
                        Some(ConfigValue::Choice { .. }) => None,
                        _ => open.reasoning_effort,
                    };
                    let (expected, _, sandbox) = options::policies(self.setup.autonomy);
                    if open.approval_policy != expected || open.sandbox["type"] != sandbox["type"] {
                        self.notice("Codex applied different permission settings. Its restrictions still apply to each turn.");
                    }
                    self.opened = true;
                    self.ready();
                }
                _ => self.fail(protocol_error("Codex returned an invalid thread response.")),
            },
            Purpose::Collaboration => {
                self.collaboration_modes = value["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m["mode"].as_str())
                    .filter(|m| matches!(*m, "plan" | "default"))
                    .map(str::to_owned)
                    .collect();
                if let Some(ConfigValue::Choice { value }) =
                    self.setup.desired.get(crate::controls::WORKFLOW)
                {
                    let native = if value == "execute" {
                        "default"
                    } else {
                        value.as_str()
                    };
                    if !matches!(value.as_str(), "plan" | "execute")
                        || !self.collaboration_modes.contains(native)
                    {
                        self.fail(protocol_error("The saved Codex workflow is unavailable. Select an offered native workflow before sending."));
                        return;
                    }
                    self.workflow = value.clone();
                }
                self.options();
            }
            Purpose::Models => match serde_json::from_value::<wire::ModelListResponse>(value) {
                Ok(page) => {
                    for model in page.data {
                        if !model.id.is_empty()
                            && !model.model.is_empty()
                            && !self.models.iter().any(|m| m.model == model.model)
                        {
                            self.models.push(model);
                        }
                    }
                    if self.models.len() > 1000 {
                        self.fail(protocol_error("Codex returned too many model entries."));
                        return;
                    }
                    if let Some(cursor) = page.next_cursor {
                        if self.cursors.len() >= 32 || !self.cursors.insert(cursor.clone()) {
                            self.fail(protocol_error("Codex repeated a model-list cursor."));
                            return;
                        }
                        self.request(
                            "model/list",
                            json!({"includeHidden":false,"cursor":cursor}),
                            Purpose::Models,
                        );
                    } else {
                        self.models_done = true;
                        self.offer_model_recovery();
                        self.ready();
                    }
                }
                Err(_) => {
                    self.models.clear();
                    self.models_done = true;
                    self.notice("Codex returned an invalid model list.");
                    self.offer_model_recovery();
                    self.ready();
                }
            },
            Purpose::Start(aura_id) => match serde_json::from_value::<wire::TurnResponse>(value) {
                Ok(reply) if !reply.turn.id.is_empty() => {
                    if let Some(active) = &mut self.active {
                        if active.aura_id != aura_id
                            || active
                                .native_id
                                .as_ref()
                                .is_some_and(|id| *id != reply.turn.id)
                        {
                            self.fail(protocol_error("Codex returned a mismatched turn ID."));
                            return;
                        }
                        active.native_id = Some(reply.turn.id.clone());
                        self.native_turn = Some(reply.turn.id.clone());
                        if active.interrupt {
                            self.interrupt();
                        }
                        if reply.turn.status != wire::TurnStatus::InProgress {
                            self.completed(reply.turn);
                        }
                    }
                }
                _ => self.fail(protocol_error("Codex returned an invalid turn response.")),
            },
            Purpose::Interrupt => {}
            Purpose::Limits => {
                if std::mem::take(&mut self.limits_dirty) {
                    self.refresh_limits();
                } else {
                    self.limits(value);
                }
            }
        }
    }
    /// Codex reports each MCP server's state as it starts; the session shows the latest.
    fn mcp_startup(&mut self, params: &Value) {
        use ghost_protocol::session::{LoadedMcpServer, McpLoadState, SessionLoaded};
        let Some(name) = params["name"].as_str().filter(|name| !name.is_empty()) else {
            return;
        };
        let state = match params["status"].as_str() {
            Some("ready") => McpLoadState::Connected,
            Some("starting") => McpLoadState::Starting,
            Some("failed") => McpLoadState::Failed,
            Some("cancelled") => McpLoadState::Off,
            _ => McpLoadState::Unknown,
        };
        if let Some(server) = self
            .mcp_states
            .iter_mut()
            .find(|server| server.name == name)
        {
            server.state = state;
        } else if self.mcp_states.len() < 64 {
            self.mcp_states.push(LoadedMcpServer {
                name: name.to_owned(),
                state,
                source: None,
            });
        } else {
            return;
        }
        self.outbox.push_back(DriverEvent::Loaded(
            SessionLoaded {
                mcp_servers: self.mcp_states.clone(),
                ..Default::default()
            }
            .bounded(),
        ));
    }
    fn notification(&mut self, method: &str, params: Value) {
        if !self.ready && !matches!(method, "warning" | "configWarning") {
            let bytes: usize = self
                .deferred
                .iter()
                .map(|(_, value)| value.to_string().len())
                .sum();
            if self.deferred.len() >= 128
                || bytes.saturating_add(params.to_string().len()) > 8 * 1024 * 1024
            {
                self.fail(protocol_error(
                    "Codex sent too many updates before initialization completed.",
                ));
            } else {
                self.deferred.push_back((method.into(), params));
            }
            return;
        }
        if method == "account/rateLimits/updated" {
            self.limits(params);
            return;
        }
        if matches!(method, "warning" | "configWarning") {
            self.notice(
                params["message"]
                    .as_str()
                    .or_else(|| params["summary"].as_str())
                    .unwrap_or("Codex reported a warning."),
            );
            return;
        }
        // Only the owned native thread can update this chat, including subagent traffic.
        // A server can be shared by every thread, when its report names none.
        if method == "mcpServer/startupStatus/updated" {
            if params["threadId"].is_null() || params["threadId"].as_str() == self.thread.as_deref()
            {
                self.mcp_startup(&params);
            }
            return;
        }
        if self.thread.as_deref() != params["threadId"].as_str() || self.thread.is_none() {
            return;
        }
        match method {
            "thread/status/changed" => {
                match serde_json::from_value::<wire::ThreadStatus>(params["status"].clone()) {
                    Ok(wire::ThreadStatus::Idle) => self.run_state(true),
                    Ok(wire::ThreadStatus::Active) => self.run_state(false),
                    _ => self.fail(protocol_error(
                        "Codex reported an unavailable thread state.",
                    )),
                }
            }
            "thread/closed" => self.fail(error(ErrorKind::AgentExited, "Codex closed the thread.")),
            "turn/started" => {
                if let Ok(turn) = serde_json::from_value::<wire::Turn>(params["turn"].clone()) {
                    if turn.id.is_empty() {
                        return;
                    }
                    if let Some(active) = &mut self.active {
                        if active.native_id.as_ref().is_some_and(|id| *id != turn.id) {
                            return;
                        }
                        active.native_id = Some(turn.id.clone());
                    }
                    self.native_turn = Some(turn.id);
                    self.run_state(false);
                    if self.active.as_ref().is_some_and(|a| a.interrupt) {
                        self.interrupt();
                    }
                }
            }
            "turn/completed" => {
                match serde_json::from_value::<wire::Turn>(params["turn"].clone()) {
                    Ok(turn) => self.completed(turn),
                    Err(_) => self.fail(protocol_error("Codex returned a malformed turn completion. The owned session was stopped to avoid uncertain state.")),
                }
            }
            "serverRequest/resolved" => {
                let key = self
                    .callbacks
                    .iter()
                    .find(|(_, callback)| callback.id == params["requestId"])
                    .map(|(key, _)| key.clone());
                if let Some(key) = key {
                    self.withdraw(&key, false);
                }
            }
            "thread/name/updated" => self.outbox.push_back(DriverEvent::Title(
                params["threadName"].as_str().map(str::to_owned),
            )),
            _ => {
                if let Some(native) = params["turnId"].as_str()
                    && self.native_turn.as_deref() != Some(native)
                {
                    return;
                }
                let turn = self
                    .active
                    .as_ref()
                    .and_then(|a| (a.native_id == self.native_turn).then(|| a.aura_id.clone()));
                if method == "thread/tokenUsage/updated"
                    && let Some(active) = &mut self.active
                {
                    active.usage = token_usage(&params["tokenUsage"]["last"]);
                }
                for event in self.translator.notification(method, &params) {
                    self.outbox.push_back(DriverEvent::Event {
                        turn: turn.clone(),
                        event,
                    });
                }
                if self.translator.overflowed() {
                    self.fail(protocol_error(
                        "Codex exceeded the per-turn item limit. The owned session was stopped.",
                    ));
                }
            }
        }
    }
    fn completed(&mut self, turn: wire::Turn) {
        if self.native_turn.as_deref() != Some(&turn.id) {
            return;
        }
        if matches!(
            turn.status,
            wire::TurnStatus::InProgress | wire::TurnStatus::Unknown
        ) {
            self.fail(protocol_error(
                "Codex completed a turn without a final status.",
            ));
            return;
        }
        let active = self.active.take();
        self.withdraw_turn(active.as_ref().map(|a| a.aura_id.as_str()));
        self.native_turn = None;
        if let Some(active) = active {
            self.pending
                .retain(|_, p| !matches!(&p.purpose,Purpose::Start(id) if id==&active.aura_id));
            let (stop_reason, failed) = match turn.status {
                wire::TurnStatus::Completed => (StopReason::EndTurn, false),
                wire::TurnStatus::Interrupted => (StopReason::Cancelled, false),
                _ => (StopReason::Error, true),
            };
            self.outbox.push_back(DriverEvent::TurnEnded {
                turn_id: active.aura_id,
                stop_reason,
                usage: active.usage,
                error: failed.then(|| {
                    error(
                        ErrorKind::AgentError,
                        turn.error
                            .as_ref()
                            .and_then(|e| e["message"].as_str())
                            .unwrap_or("Codex failed this turn."),
                    )
                }),
            });
        }
        self.translator.finish_turn();
        if turn.error.as_ref().is_some_and(auth_error) {
            self.authentication_required();
            return;
        }
        self.refresh_limits();
    }
    fn callback(&mut self, id: Value, method: &str, params: Value) {
        if !(id.is_string() || id.as_i64().is_some()) {
            self.reject(id, "Codex callback IDs must be strings or integers.");
            return;
        }
        if self.callbacks.values().any(|callback| callback.id == id) {
            return;
        }
        if self.callbacks.len() >= 256 {
            self.reject(id, "Too many outstanding Codex requests.");
            return;
        }
        if method == "mcpServer/elicitation/request" {
            self.elicitation(id, &params);
            return;
        }
        if self.thread.as_deref() != params["threadId"].as_str() || self.thread.is_none() {
            self.reject(id, "The callback does not belong to this thread.");
            return;
        }
        let native_turn = params["turnId"].as_str().map(str::to_owned);
        if native_turn.as_ref().is_none_or(String::is_empty)
            || params["itemId"].as_str().is_none_or(str::is_empty)
            || (method == "item/tool/requestUserInput" && !params["isBlocking"].is_boolean())
        {
            self.reject(
                id,
                "Codex sent a callback without valid item, turn or input metadata.",
            );
            return;
        }
        self.next_callback += 1;
        let key = format!("{}.request.{}", self.spawn_id, self.next_callback);
        let turn = self
            .active
            .as_ref()
            .and_then(|a| (a.native_id == native_turn).then(|| a.aura_id.clone()));
        let (body, event) = if method == "item/tool/requestUserInput" {
            match requests::questions(&params) {
                Ok(questions) => (
                    requests::Callback::Questions(questions.clone()),
                    DriverEvent::UiRequest {
                        turn: turn.clone(),
                        request: UiRequest {
                            request_id: key.clone(),
                            prompt: UiPrompt::Questions {
                                message: "Codex needs your input".into(),
                                questions,
                            },
                        },
                    },
                ),
                Err(message) => {
                    self.respond(id, json!({"answers":{}}));
                    self.notice(message);
                    return;
                }
            }
        } else if let Some((body, mut request)) = requests::approval(method, &params, key.clone()) {
            if let Some(item) = params["itemId"].as_str() {
                self.translator.enrich_permission(item, &mut request);
            }
            (
                body,
                DriverEvent::Permission {
                    turn: turn.clone(),
                    request,
                },
            )
        } else {
            self.reject(id, "This Codex request is not supported by Ghost.");
            return;
        };
        if self.stopping
            || native_turn
                .as_ref()
                .is_some_and(|native| self.native_turn.as_ref() != Some(native))
        {
            self.respond(id, body.cancel());
            return;
        }
        self.callbacks.insert(
            key,
            Callback {
                id,
                turn,
                native_turn,
                body,
            },
        );
        self.outbox.push_back(event);
    }
    /// An MCP server's form or browser step, which Codex asks its client to show. One Ghost
    /// cannot show is cancelled with a notice saying where to finish it.
    fn elicitation(&mut self, id: Value, params: &Value) {
        if self.thread.is_none() || self.thread.as_deref() != params["threadId"].as_str() {
            self.reject(id, "The callback does not belong to this thread.");
            return;
        }
        let source = params["serverName"].as_str();
        let message = params["message"]
            .as_str()
            .filter(|message| !message.trim().is_empty())
            .unwrap_or("An MCP server needs your input.");
        let parsed = if self.stopping {
            Err(String::new())
        } else {
            match params["mode"].as_str() {
                Some("form") => {
                    crate::elicitation::form(message, source, &params["requestedSchema"], None)
                }
                Some("url") => crate::elicitation::link(
                    message,
                    source,
                    params["url"].as_str().unwrap_or_default(),
                ),
                _ => Err("An MCP server asked for input Ghost cannot show.".into()),
            }
        };
        let (prompt, spec) = match parsed {
            Ok(parsed) => parsed,
            Err(reason) => {
                self.respond(id, json!({"action":"cancel","content":null}));
                if !reason.is_empty() {
                    self.notice(format!(
                        "{reason} It was cancelled; finish it in the Codex CLI if you need it."
                    ));
                }
                return;
            }
        };
        self.next_callback += 1;
        let key = format!("{}.request.{}", self.spawn_id, self.next_callback);
        let native_turn = params["turnId"]
            .as_str()
            .filter(|turn| !turn.is_empty())
            .map(str::to_owned);
        let turn = self.active.as_ref().and_then(|active| {
            (native_turn.is_some() && active.native_id == native_turn)
                .then(|| active.aura_id.clone())
        });
        self.outbox.push_back(DriverEvent::UiRequest {
            turn: turn.clone(),
            request: UiRequest {
                request_id: key.clone(),
                prompt,
            },
        });
        self.callbacks.insert(
            key,
            Callback {
                id,
                turn,
                native_turn,
                body: requests::Callback::Elicitation(spec),
            },
        );
    }
    fn withdraw(&mut self, key: &str, respond: bool) {
        if let Some(callback) = self.callbacks.remove(key) {
            if respond {
                self.respond(callback.id, callback.body.cancel());
            }
            self.outbox.push_back(match callback.body {
                requests::Callback::Approval { .. } => DriverEvent::PermissionWithdrawn {
                    request_id: key.into(),
                },
                requests::Callback::Questions(_) | requests::Callback::Elicitation(_) => {
                    DriverEvent::UiWithdrawn {
                        request_id: key.into(),
                    }
                }
            });
        }
    }
    fn withdraw_turn(&mut self, turn: Option<&str>) {
        let keys: Vec<_> = self
            .callbacks
            .iter()
            .filter(|(_, c)| c.turn.as_deref() == turn && c.native_turn == self.native_turn)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            self.withdraw(&key, true);
        }
    }
    fn withdraw_all(&mut self) {
        if let Some(recovery) = self.model_recovery.take() {
            if let Some(request_id) = recovery.request_id {
                self.outbox
                    .push_back(DriverEvent::UiWithdrawn { request_id });
            }
            self.fail(recovery.failure);
        }
        for key in self.callbacks.keys().cloned().collect::<Vec<_>>() {
            self.withdraw(&key, true);
        }
    }
    fn interrupt(&mut self) {
        self.withdraw_all();
        if let Some(active) = &mut self.active {
            active.interrupt = true;
        }
        if self
            .pending
            .values()
            .any(|p| matches!(p.purpose, Purpose::Interrupt))
        {
            return;
        }
        if let (Some(thread), Some(turn)) = (&self.thread, &self.native_turn) {
            self.request(
                "turn/interrupt",
                json!({"threadId":thread,"turnId":turn}),
                Purpose::Interrupt,
            );
        } else if self.ready && self.active.is_none() && !self.idle && !self.stopping {
            self.fail(protocol_error(
                "Codex did not provide the active turn ID. Stopping this owned session.",
            ));
        }
    }
    fn command(&mut self, command: DriverCommand) {
        log_command(&self.log, &command);
        if !self.ready
            && matches!(
                command,
                DriverCommand::Prompt { .. }
                    | DriverCommand::SetOption { .. }
                    | DriverCommand::SetAutonomy { .. }
            )
        {
            self.held.push_back(command);
            return;
        }
        match command {
            DriverCommand::McpAction {
                server_name,
                action,
                reply,
            } => {
                if !self.ready
                    || !self.idle
                    || !self
                        .setup
                        .mcp_servers
                        .iter()
                        .any(|s| s.definition.name == server_name)
                {
                    let _ = reply.send(Err(protocol_error(
                        "Select a shared MCP server loaded in this idle session.",
                    )));
                    return;
                }
                let Some(thread) = self.thread.clone() else {
                    let _ = reply.send(Err(protocol_error("The Codex thread is unavailable.")));
                    return;
                };
                let (method, params) = match action {
                    ghost_protocol::mcp::McpSessionAction::Status => (
                        "mcpServerStatus/list",
                        json!({"threadId":thread,"serverName":server_name,"limit":1,"detail":"full"}),
                    ),
                    ghost_protocol::mcp::McpSessionAction::Login => (
                        "mcpServer/oauth/login",
                        json!({"threadId":thread,"name":server_name,"timeoutSecs":120}),
                    ),
                    _ => {
                        crate::mcp::unsupported(
                            reply,
                            "Restart this idle chat to apply its latest MCP snapshot.",
                        );
                        return;
                    }
                };
                self.request(
                    method,
                    params,
                    Purpose::Mcp {
                        server_name,
                        action,
                        reply,
                    },
                );
            }
            DriverCommand::Steer {
                turn_id,
                message_id,
                parts,
                reply,
            } => {
                let native_turn = self
                    .active
                    .as_ref()
                    .filter(|active| {
                        self.ready
                            && !self.stopping
                            && self.failure.is_none()
                            && !self.idle
                            && !active.interrupt
                            && active.aura_id == turn_id
                    })
                    .and_then(|active| active.native_id.clone());
                let Some(native_turn) = native_turn else {
                    let _ = reply.send(SteerResult::Rejected {
                        message: "The selected turn is no longer running or has not started yet."
                            .into(),
                    });
                    return;
                };
                let input =
                    match crate::attachments::codex(&parts, self.setup.attachment_dir.as_deref()) {
                        Ok(input) => input,
                        Err(message) => {
                            let _ = reply.send(SteerResult::Rejected { message });
                            return;
                        }
                    };
                self.request("turn/steer", json!({"threadId":self.thread,"expectedTurnId":native_turn,"clientUserMessageId":message_id,"input":input}), Purpose::Steer { native_turn, reply });
            }
            DriverCommand::Prompt { turn, parts } => {
                if self.active.is_some() || !self.idle || !self.callbacks.is_empty() {
                    self.outbox.push_back(DriverEvent::TurnEnded {
                        turn_id: turn.turn_id,
                        stop_reason: StopReason::Error,
                        usage: None,
                        error: Some(protocol_error("Codex is not ready for another prompt.")),
                    });
                    return;
                }
                let input =
                    match crate::attachments::codex(&parts, self.setup.attachment_dir.as_deref()) {
                        Ok(input) => input,
                        Err(message) => {
                            self.outbox.push_back(DriverEvent::TurnEnded {
                                turn_id: turn.turn_id,
                                stop_reason: StopReason::Error,
                                usage: None,
                                error: Some(protocol_error(&message)),
                            });
                            return;
                        }
                    };
                let (approval, _, sandbox) = options::policies(self.setup.autonomy);
                let mut params = json!({"threadId":self.thread,"input":input,"model":self.model,"cwd":self.setup.agent_folder,"approvalPolicy":approval,"approvalsReviewer":"user","sandboxPolicy":sandbox,"clientUserMessageId":turn.turn_id});
                if let Some(effort) = self.effort.as_ref().or_else(|| {
                    self.models
                        .iter()
                        .find(|model| model.model == self.model)
                        .map(|model| &model.default_reasoning_effort)
                }) {
                    params["effort"] = json!(effort);
                }
                self.active = Some(ActiveTurn {
                    aura_id: turn.turn_id.clone(),
                    native_id: None,
                    interrupt: false,
                    usage: None,
                });
                self.run_state(false);
                if self.collaboration_modes.contains("plan")
                    && self.collaboration_modes.contains("default")
                {
                    params["collaborationMode"] = json!({"mode": if self.workflow == "plan" { "plan" } else { "default" }, "settings":{"model":self.model,"reasoning_effort":params.get("effort").cloned().unwrap_or(Value::Null),"developer_instructions":null}});
                }
                self.request("turn/start", params, Purpose::Start(turn.turn_id));
            }
            DriverCommand::Interrupt => {
                while let Some(command) = self.held.pop_front() {
                    if let DriverCommand::Prompt { turn, .. } = command {
                        self.outbox.push_back(DriverEvent::TurnEnded {
                            turn_id: turn.turn_id,
                            stop_reason: StopReason::Cancelled,
                            usage: None,
                            error: None,
                        });
                    }
                }
                self.interrupt();
            }
            DriverCommand::EditPlan { .. } | DriverCommand::DeclinePlan { .. } => {}
            DriverCommand::RespondPermission {
                request_id,
                outcome,
            } => {
                let result = self
                    .callbacks
                    .get(&request_id)
                    .and_then(|callback| match outcome {
                        PermissionOutcome::Selected(ref decision) => {
                            callback.body.permission_answer(decision)
                        }
                        PermissionOutcome::Cancelled => Some(callback.body.cancel()),
                    });
                if let Some(result) = result
                    && let Some(callback) = self.callbacks.remove(&request_id)
                {
                    self.respond(callback.id, result);
                }
            }
            DriverCommand::RespondUi { request_id, answer } => {
                if self.answer_model_recovery(&request_id, &answer) {
                    return;
                }
                let result = self
                    .callbacks
                    .get(&request_id)
                    .and_then(|callback| match &answer {
                        Some(answer) => callback.body.question_answer(answer),
                        None => Some(callback.body.cancel()),
                    });
                if let Some(result) = result
                    && let Some(callback) = self.callbacks.remove(&request_id)
                {
                    self.respond(callback.id, result);
                }
            }
            DriverCommand::SetOption {
                option_id,
                value,
                reply,
            } => {
                if option_id == crate::controls::WORKFLOW {
                    if let ConfigValue::Choice { value } = &value {
                        let native = if value == "execute" {
                            "default"
                        } else {
                            value.as_str()
                        };
                        if matches!(value.as_str(), "plan" | "execute")
                            && self.collaboration_modes.contains(native)
                            && self.idle
                            && self.active.is_none()
                            && self.callbacks.is_empty()
                        {
                            self.workflow = value.clone();
                            self.options();
                            let _ = reply.send(Ok(()));
                            return;
                        }
                    }
                    let _ = reply.send(Err(protocol_error(
                        "Codex does not offer that workflow or is busy.",
                    )));
                    return;
                }
                if options::validate(&self.models, &self.model, &option_id, &value) {
                    let ConfigValue::Choice { value: choice } = &value else {
                        unreachable!()
                    };
                    if option_id == "model" {
                        self.model = choice.clone();
                        self.effort = None;
                    } else {
                        self.effort = (choice != "default").then(|| choice.clone());
                    }
                    self.setup.desired.insert(option_id, value);
                    self.options();
                    let _ = reply.send(Ok(()));
                } else {
                    let _ = reply.send(Err(protocol_error("Codex does not offer that setting.")));
                }
            }
            DriverCommand::SetAutonomy { autonomy, reply } => {
                self.setup.autonomy = autonomy;
                let state = options::autonomy(autonomy);
                self.outbox.push_back(DriverEvent::Autonomy(state.clone()));
                let _ = reply.send(Ok(state));
            }
            DriverCommand::Shutdown { .. } => {}
            DriverCommand::RewindFiles { reply, .. } => {
                let _ = reply.send(Err(protocol_error(
                    "Native Codex file rewind is not supported.",
                )));
            }
        }
    }
    fn refresh_limits(&mut self) {
        if !self.ready || self.stopping || self.failure.is_some() {
            return;
        }
        if self
            .pending
            .values()
            .any(|p| matches!(p.purpose, Purpose::Limits))
        {
            self.limits_dirty = true;
            return;
        }
        self.request("account/rateLimits/read", Value::Null, Purpose::Limits);
    }
    fn limits(&mut self, value: Value) {
        if let Some(buckets) = value["rateLimitsByLimitId"].as_object() {
            for (key, bucket) in buckets.iter().take(32) {
                self.merge_limit_bucket(key, bucket);
            }
        } else if value["rateLimits"].is_object() {
            let bucket = &value["rateLimits"];
            let key = bucket["limitId"].as_str().unwrap_or("codex");
            self.merge_limit_bucket(key, bucket);
        }
        let mut windows = Vec::new();
        for (key, bucket) in &self.limits {
            for name in ["primary", "secondary"] {
                let window = &bucket[name];
                let Some(percent) = window["usedPercent"].as_f64().filter(|v| v.is_finite()) else {
                    continue;
                };
                let duration = window["windowDurationMins"]
                    .as_i64()
                    .filter(|v| *v > 0)
                    .map(|v| format!("{v} min"))
                    .unwrap_or_else(|| name.into());
                windows.push(LimitWindow {
                    label: format!(
                        "{} ({duration})",
                        bucket["limitName"].as_str().unwrap_or(key)
                    ),
                    used_percent: percent.clamp(0.0, 100.0),
                    resets_at: window["resetsAt"]
                        .as_i64()
                        .and_then(|seconds| seconds.checked_mul(1000)),
                });
            }
        }
        if !windows.is_empty() {
            self.outbox.push_back(DriverEvent::Event {
                turn: None,
                event: GhostEvent::LimitsUpdated { windows },
            });
        }
    }
    fn merge_limit_bucket(&mut self, key: &str, bucket: &Value) {
        if key.len() > 128 || (!self.limits.contains_key(key) && self.limits.len() >= 32) {
            return;
        }
        let current = self.limits.entry(key.into()).or_insert_with(|| json!({}));
        if let Some(name) = bucket["limitName"].as_str() {
            current["limitName"] = json!(name.chars().take(256).collect::<String>());
        }
        for name in ["primary", "secondary"] {
            if let Some(window) = bucket.get(name) {
                current[name] = if window.is_null() {
                    Value::Null
                } else {
                    json!({"usedPercent":window["usedPercent"].as_f64(), "windowDurationMins":window["windowDurationMins"].as_i64(), "resetsAt":window["resetsAt"].as_i64()})
                };
            }
        }
    }
    fn expire(&mut self) {
        let now = Instant::now();
        if !self.ready
            && !self.waiting_for_model_recovery()
            && now >= self.init_deadline
            && self.failure.is_none()
        {
            self.fail(error(
                ErrorKind::StartTimeout,
                "Codex did not open the thread before its deadline.",
            ));
        }
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.deadline)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            let Some(p) = self.pending.remove(&id) else {
                continue;
            };
            match p.purpose {
                Purpose::Mcp{reply,..}=>{let _=reply.send(Err(protocol_error("Codex did not acknowledge the MCP operation before its deadline.")));},
                Purpose::Steer { reply, .. } => {
                    let message = "Codex did not acknowledge steering before its deadline. Delivery is unconfirmed.";
                    let _ = reply.send(SteerResult::Unconfirmed { message: message.into() });
                    self.fail(error(ErrorKind::AgentError, message));
                },
                Purpose::Models=>{self.models.clear();self.models_done=true;self.notice("Codex did not return its model list before the deadline.");self.offer_model_recovery();self.ready();},
                Purpose::Collaboration=>{self.notice("Codex did not list collaboration modes before its deadline."); if self.setup.desired.contains_key(crate::controls::WORKFLOW) { self.fail(protocol_error("The saved native workflow could not be verified.")); }},
                Purpose::Limits=>{self.limits_dirty=false;self.notice("Codex did not refresh usage limits before the deadline.");},
                _=>self.fail(error(ErrorKind::AgentError,"Codex did not acknowledge an operation before its deadline. The session was stopped to avoid uncertain state.")),
            }
        }
    }
}

fn token_usage(value: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        input_tokens: value["inputTokens"].as_u64()?,
        output_tokens: value["outputTokens"].as_u64()?,
        cache_read_tokens: value["cachedInputTokens"].as_u64(),
        cache_write_tokens: value["cacheWriteInputTokens"].as_u64(),
        reasoning_tokens: value["reasoningOutputTokens"].as_u64(),
        scope: UsageScope::Turn,
    })
}

pub(super) async fn run(ctx: DriverContext) -> DriverExit {
    let DriverContext {
        mut commands,
        events,
        spawner,
        setup,
    } = ctx;
    if setup.native_start != crate::driver::NativeStartOptions::default() {
        return DriverExit::StartFailed(protocol_error(
            "Native startup overrides are not supported by the Codex driver yet.",
        ));
    }
    if !setup.add_dirs.is_empty() {
        return DriverExit::StartFailed(protocol_error(
            "Additional directories are not supported by the native Codex driver yet.",
        ));
    }
    let spec = SpawnSpec {
        launch: {
            let mut launch = setup.launch.clone();
            crate::mcp::codex_environment(&setup.mcp_servers, &mut launch);
            launch
        },
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
            command=commands.recv()=>match command {None|Some(DriverCommand::Shutdown {..})=>return DriverExit::Closed,Some(command)=>held.push_back(command)},
            permit=events.reserve(bytes),if !early.is_empty()=>{let Ok(permit)=permit else {return DriverExit::Closed;};if let Some(event)=early.pop_front() {permit.send(event);}},
            result=&mut spawning=>match result {Ok(process)=>break process,Err(err)=>return DriverExit::StartFailed(crate::acp::spawn_failure(&err))},
        }
    };
    process
        .log
        .note(LogNote::Start(setup.start_note(&process.spawn_id)));
    let writer = process.io.stdin.clone();
    let init_deadline =
        Instant::now() + setup.timings.open_ceiling.min(setup.launch.startup_timeout);
    let mut s = Session {
        rpc: RpcOut::new(process.io.stdin, false),
        log: process.log,
        setup,
        spawn_id: process.spawn_id.clone(),
        pending: BTreeMap::new(),
        callbacks: BTreeMap::new(),
        next_callback: 0,
        outbox: early,
        held,
        deferred: VecDeque::new(),
        translator: translate::Translator::new(process.spawn_id),
        thread: None,
        native_turn: None,
        active: None,
        idle: false,
        ready: false,
        opened: false,
        models_done: false,
        model_recovery: None,
        model_recovery_attempted: false,
        models: Vec::new(),
        cursors: BTreeSet::new(),
        model: String::new(),
        effort: None,
        workflow: "execute".into(),
        collaboration_modes: BTreeSet::new(),
        mcp_states: Vec::new(),
        version: None,
        limits: BTreeMap::new(),
        limits_dirty: false,
        failure: None,
        stopping: false,
        init_deadline,
    };
    s.outbox
        .push_back(DriverEvent::Status(DriverStatus::Initializing));
    s.request("initialize",json!({"clientInfo":{"name":"ghost","title":"Ghost ADE","version":s.setup.client_version},"capabilities":{"experimentalApi":true}}),Purpose::Initialize);
    let control = process.control;
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
            closing = Some((Instant::now(), ShutdownPlan::STANDARD));
        }
        if let Some((due, plan)) = closing
            && (Instant::now() >= due || s.idle)
        {
            s.stopping = true;
            closing = None;
            s.withdraw_all();
            s.pending.clear();
            s.held.clear();
            s.rpc.close();
            shutdown = Some(control.shutdown(plan));
        }
        if eof && process_done && s.outbox.is_empty() {
            break;
        }
        if s.ready
            && !requested_close
            && !s.stopping
            && !process_done
            && s.failure.is_none()
            && let Some(command) = s.held.pop_front()
        {
            s.command(command);
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
            .chain(
                (!s.ready
                    && !s.waiting_for_model_recovery()
                    && s.failure.is_none()
                    && !requested_close)
                    .then_some(s.init_deadline),
            )
            .chain(closing.map(|c| c.0))
            .chain(drain_deadline.filter(|_| !full && !eof))
            .min()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400));
        tokio::select! {
            _=writer.writer_closed(),if !s.stopping&&!process_done&&s.failure.is_none()=>{
                if let Some(info)=control.try_exited() {
                    exited=Some(info);process_done=true;
                } else {
                    s.fail(error(ErrorKind::AgentExited,"Codex stopped reading its input. The owned session was stopped."));
                }
            },
            command=commands.recv(),if !requested_close&&s.failure.is_none()&&!s.stopping&&!process_done=>match command {
                None|Some(DriverCommand::Shutdown {..})=>{
                    let plan=match command {Some(DriverCommand::Shutdown {plan})=>plan,_=>crate::driver::StopPlan {cancel_grace:Duration::ZERO,process:ShutdownPlan::STANDARD}};
                    s.log.command(json!({"type":"shutdown"}));requested_close=true;s.stopping=true;s.outbox.push_back(DriverEvent::Status(DriverStatus::Closing));if !s.idle {s.interrupt();}closing=Some((Instant::now()+plan.cancel_grace,plan.process));
                },
                Some(command)=>s.command(command),
            },
            permit=events.reserve(bytes),if !s.outbox.is_empty()=>{let Ok(permit)=permit else {control.kill_tree();return DriverExit::Closed;};if let Some(event)=s.outbox.pop_front() {permit.send(event);}},
            item=stdout.next(),if !eof&&!full=>match item {
                Some(StdoutItem::Line(line))=>{for message in jsonrpc::parse_line(&line) {s.incoming(message);}},
                Some(StdoutItem::Oversized {..})=>s.fail(protocol_error("Codex sent a protocol message larger than the supported limit.")),
                None=>{eof=true;if !process_done&&shutdown.is_none() {closing=Some((Instant::now()+Duration::from_secs(2),ShutdownPlan::FORCE));}},
            },
            info=&mut exit,if !process_done=>{exited=Some(info);process_done=true;},
            report=async {if let Some(task)=&mut shutdown {Some(task.await)} else {None}},if shutdown.is_some()=>{shutdown=None;process_done=true;if let Some(report)=report {exited=exited.or(report.exit);}},
            _=tokio::time::sleep_until(deadline)=>{s.expire();if drain_deadline.is_some_and(|due|Instant::now()>=due)&&!full {eof=true;}},
        }
    }
    if requested_close {
        DriverExit::Closed
    } else if !s.ready {
        let mut failure = s.failure.unwrap_or_else(|| {
            error(
                ErrorKind::AgentExited,
                "Codex exited before opening the thread.",
            )
        });
        let tail = control.stderr_tail(STDERR_TAIL_BYTES);
        if failure.detail.is_none() && !tail.trim().is_empty() {
            failure.detail = Some(tail);
        }
        DriverExit::StartFailed(failure)
    } else {
        DriverExit::Crashed {
            exit: exited,
            stderr_tail: control.stderr_tail(STDERR_TAIL_BYTES),
        }
    }
}
