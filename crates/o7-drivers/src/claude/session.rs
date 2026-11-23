//! One task owns control correlation, events and shutdown. Event backpressure pauses stdout but
//! never blocks incoming commands or the process shutdown chain.
use super::{translate, wire};
use crate::driver::{
    DriverCommand, DriverContext, DriverError, DriverEvent, DriverExit, DriverStatus,
    FileRewindResult, LoggedCommand, PermissionOutcome, ReadyInfo, ResumeOutcome, SessionSetup,
};
use futures::future::BoxFuture;
use o7_process::{
    LineSink, LogNote, MAX_LINE_BYTES, PIPE_EOF_GRACE, ProtocolLog, STDERR_TAIL_BYTES,
    ShutdownPlan, SpawnSpec, StdoutItem,
};
use o7_protocol::session::{AuthCommand, AuthPrompt, ConfigValue};
use o7_protocol::{
    AutoAnswer, Autonomy, AutonomyState, Capabilities, ErrorKind, LimitWindow, NoticeLevel,
    O7Event, PermissionOption, PermissionOptionKind, PermissionRequest, StopReason, UsageInfo,
};
use o7_protocol::{UiAnswer, UiPrompt, UiQuestion, UiQuestionOption, UiRequest};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::oneshot, time::Instant};

fn error(kind: ErrorKind, message: impl Into<String>) -> DriverError {
    DriverError {
        kind,
        message: message.into(),
        detail: None,
    }
}
fn config_timeout() -> DriverError {
    error(
        ErrorKind::AgentError,
        "Claude did not apply this setting before its deadline.",
    )
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
fn explicit_effort(value: &str) -> Option<String> {
    (value != translate::DEFAULT_EFFORT).then(|| value.to_owned())
}
fn auth_required(frame: &Value) -> bool {
    frame["type"] == "result" && frame["api_error_status"].as_u64() == Some(401)
}
/// Claude Code's own report that the account's usage limit is reached: the synthetic assistant
/// message and the turn's result carry `"api_error": "usage_limit_reached"` (seen in 2.1.316).
fn usage_limited(frame: &Value) -> bool {
    frame["api_error"] == "usage_limit_reached"
}
/// The limit in Claude's own words ("You've hit your 5-hour limit · resets 5:30pm (...)") and when
/// it resets (`api_error_params.rate_limit_info.resetsAt`, in seconds).
fn usage_limit(frame: &Value) -> o7_protocol::UsageLimit {
    let text = frame["message"]["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find_map(|block| block["text"].as_str()))
        .or_else(|| frame["result"].as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("Claude's usage limit is reached.");
    o7_protocol::UsageLimit {
        message: crate::acp::options::truncate(text, 240),
        resets_at: frame["api_error_params"]["rate_limit_info"]["resetsAt"]
            .as_i64()
            .and_then(|seconds| seconds.checked_mul(1000)),
    }
}
fn auth_error() -> DriverError {
    error(
        ErrorKind::AuthRequired,
        "Claude Code needs you to sign in again.",
    )
}
fn auth_prompt(program: &std::path::Path) -> AuthPrompt {
    AuthPrompt {
        message: auth_error().message,
        commands: vec![AuthCommand {
            label: "Sign in to Claude Code".into(),
            argv: vec![
                program.to_string_lossy().into_owned(),
                "auth".into(),
                "login".into(),
            ],
            env: Vec::new(),
        }],
    }
}
fn merge_usage(current: &mut UsageInfo, update: UsageInfo) -> bool {
    let before = current.clone();
    if update.context_used.is_some() {
        current.context_used = update.context_used;
    }
    if update.context_size.is_some() {
        current.context_size = update.context_size;
    }
    if update.cost.is_some() {
        current.cost = update.cost;
    }
    *current != before
}
fn reset_millis(value: &Value) -> Option<i64> {
    let date = time::OffsetDateTime::parse(
        value.as_str()?,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()?;
    i64::try_from(date.unix_timestamp_nanos().div_euclid(1_000_000)).ok()
}
fn usage_limits(payload: &Value) -> Vec<LimitWindow> {
    let limits = &payload["rate_limits"];
    let label = |name: &str| match name {
        "session" | "five_hour" => "5 hours".to_owned(),
        "weekly_all" | "seven_day" => "7 days".to_owned(),
        _ => crate::acp::options::truncate(name, 48),
    };
    if let Some(windows) = limits["limits"].as_array() {
        return windows
            .iter()
            .filter_map(|window| {
                let percent = window["percent"]
                    .as_f64()
                    .filter(|number| number.is_finite())?;
                let kind = window["kind"].as_str()?;
                let name = if kind == "weekly_scoped" {
                    window["scope"]["model"]["display_name"]
                        .as_str()
                        .map(|model| format!("7 days ({model})"))
                        .unwrap_or_else(|| label(kind))
                } else {
                    label(kind)
                };
                Some(LimitWindow {
                    label: crate::acp::options::truncate(&name, 48),
                    used_percent: percent.clamp(0.0, 100.0),
                    resets_at: reset_millis(&window["resets_at"]),
                })
            })
            .take(16)
            .collect();
    }
    limits
        .as_object()
        .map(|windows| {
            windows
                .iter()
                .filter_map(|(name, window)| {
                    let percent = window["utilization"]
                        .as_f64()
                        .filter(|number| number.is_finite())?;
                    Some(LimitWindow {
                        label: label(name),
                        used_percent: percent.clamp(0.0, 100.0),
                        resets_at: reset_millis(&window["resets_at"]),
                    })
                })
                .take(16)
                .collect()
        })
        .unwrap_or_default()
}
fn result_reason(frame: &Value, stream_reason: Option<&str>) -> StopReason {
    let terminal = frame["terminal_reason"].as_str().unwrap_or_default();
    let subtype = frame["subtype"].as_str().unwrap_or_default();
    let stop_reason = frame["stop_reason"].as_str().or(stream_reason);
    if auth_required(frame) {
        StopReason::Error
    } else if terminal.starts_with("aborted_") {
        StopReason::Cancelled
    } else if terminal == "max_turns" || subtype == "error_max_turns" {
        StopReason::MaxTurnRequests
    } else if frame["is_error"] == true
        || subtype.starts_with("error_")
        || matches!(
            terminal,
            "budget_exhausted" | "prompt_too_long" | "api_error"
        )
    {
        StopReason::Error
    } else if stop_reason == Some("max_tokens") {
        StopReason::MaxTokens
    } else if stop_reason == Some("refusal") {
        StopReason::Refusal
    } else if terminal.is_empty() || terminal == "completed" {
        StopReason::EndTurn
    } else {
        StopReason::Other
    }
}
fn mode(a: Autonomy) -> &'static str {
    match a {
        Autonomy::Ask => "default",
        Autonomy::AutoEdit => "acceptEdits",
        Autonomy::FullAuto => "bypassPermissions",
    }
}
fn autonomy(requested: Autonomy, native: &str) -> AutonomyState {
    // The vendor mode applies approvals. Remaining requests include explicit ask rules and
    // restrictions and must never be bypassed by our own permission policy.
    AutonomyState {
        requested,
        native_mode: Some(native.into()),
        auto_answer: AutoAnswer::Never,
        note: (native != mode(requested)).then(|| {
            "Claude is using a different permission mode from the requested setting.".into()
        }),
    }
}
fn model_metadata<'a>(init: &'a Value, model: &str) -> Option<&'a Value> {
    let models = init["models"].as_array()?;
    models
        .iter()
        .find(|entry| entry["value"].as_str() == Some(model))
        .or_else(|| {
            models
                .iter()
                .find(|entry| entry["resolvedModel"].as_str() == Some(model))
        })
}
fn model_identity<'a>(init: &'a Value, model: &'a str) -> &'a str {
    model_metadata(init, model)
        .and_then(|entry| entry["resolvedModel"].as_str())
        .unwrap_or(model)
}
fn native_session_id(setup: &SessionSetup) -> Result<String, DriverError> {
    let invalid = |message| error(ErrorKind::ProtocolError, message);
    let options = &setup.native_start;
    if setup
        .resume
        .as_ref()
        .is_some_and(|source| source.trim().is_empty() || source.contains('\0'))
    {
        return Err(invalid(
            "Claude's resume source must be a nonempty session id or transcript path.",
        ));
    }
    if (options.fork || options.resume_at.is_some()) && setup.resume.is_none() {
        return Err(invalid(
            "Claude's fork and resume-at options require a resume source.",
        ));
    }
    if let Some(at) = &options.resume_at {
        uuid::Uuid::parse_str(at)
            .map_err(|_| invalid("Claude's resume-at message id must be a UUID."))?;
    }
    let explicit = options
        .new_session_id
        .as_deref()
        .map(uuid::Uuid::parse_str)
        .transpose()
        .map_err(|_| invalid("Claude's new session id must be a UUID."))?;
    if let Some(source) = &setup.resume {
        if !options.fork {
            if explicit.is_some() {
                return Err(invalid(
                    "An explicit new Claude session id requires a fresh start or fork.",
                ));
            }
            return Ok(
                uuid::Uuid::parse_str(source).map_or_else(|_| source.clone(), |id| id.to_string())
            );
        }
        if explicit.is_some() && explicit == uuid::Uuid::parse_str(source).ok() {
            return Err(invalid(
                "A Claude fork must use a different session id from its source.",
            ));
        }
    }
    Ok(explicit.unwrap_or_else(uuid::Uuid::now_v7).to_string())
}

pub(super) fn launch(setup: &SessionSetup, native_id: &str) -> o7_process::LaunchSpec {
    let mut launch = setup.launch.clone();
    crate::mcp::claude(&setup.mcp_servers, &mut launch);
    // Apply Full auto only after initialize and before any prompt. A policy refusal can then
    // leave acceptEdits in effect without retrying an executed prompt.
    let initial = if setup.autonomy == Autonomy::FullAuto {
        "acceptEdits"
    } else {
        mode(setup.autonomy)
    };
    launch.args.extend([
        "--permission-mode".into(),
        initial.into(),
        "--allow-dangerously-skip-permissions".into(),
    ]);
    launch.args.push(
        match &setup.resume {
            Some(id) => format!("--resume={id}"),
            None => format!("--session-id={native_id}"),
        }
        .into(),
    );
    if setup.native_start.fork {
        launch.args.extend([
            "--fork-session".into(),
            format!("--session-id={native_id}").into(),
        ]);
    }
    if let Some(at) = &setup.native_start.resume_at {
        launch.args.push(format!("--resume-session-at={at}").into());
    }
    if let Some(ConfigValue::Choice { value: model }) = setup.desired.get("model") {
        launch.args.push(format!("--model={model}").into());
    }
    for dir in &setup.add_dirs {
        launch
            .args
            .extend(["--add-dir".into(), dir.as_os_str().to_owned()]);
    }
    launch
        .set_env
        .retain(|(key, _)| key != "CLAUDE_CODE_SDK_READS_SESSION_STATE");
    launch
        .set_env
        .push(("CLAUDE_CODE_SDK_READS_SESSION_STATE".into(), "1".into()));
    if setup.native_start.file_checkpointing {
        launch
            .set_env
            .retain(|(key, _)| key != "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING");
        launch.set_env.push((
            "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING".into(),
            "true".into(),
        ));
    }
    launch
}
enum Purpose {
    Mcp {
        server_name: String,
        action: o7_protocol::mcp::McpSessionAction,
        reply: oneshot::Sender<Result<o7_protocol::mcp::McpActionResult, DriverError>>,
    },
    Initialize,
    /// Asks Claude for Full access before a chat that opens in Plan takes its planning turn.
    ConfirmAccess,
    InitialMode,
    InitialEffort,
    Interrupt,
    Telemetry(TelemetryKind),
    Option {
        id: String,
        value: ConfigValue,
        reply: oneshot::Sender<Result<(), DriverError>>,
    },
    Autonomy {
        value: Autonomy,
        reply: oneshot::Sender<Result<AutonomyState, DriverError>>,
    },
    Rewind {
        dry_run: bool,
        reply: oneshot::Sender<Result<FileRewindResult, DriverError>>,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum TelemetryKind {
    Context,
    Usage,
}
impl TelemetryKind {
    fn index(self) -> usize {
        match self {
            Self::Context => 0,
            Self::Usage => 1,
        }
    }
    fn request(self) -> Value {
        match self {
            Self::Context => json!({"subtype":"get_context_usage","detail":"summary"}),
            Self::Usage => json!({"subtype":"get_usage","skip_behaviors":true}),
        }
    }
}
struct Pending {
    purpose: Purpose,
    deadline: Instant,
}
struct Approval {
    request: Value,
    turn: Option<String>,
    allowed: Vec<String>,
}
struct Questions {
    input: Value,
    questions: Vec<UiQuestion>,
    turn: Option<String>,
}
/// An MCP server's form or browser step, which Claude asks its host to show.
struct Elicit {
    spec: crate::elicitation::Elicitation,
    turn: Option<String>,
}
struct Session {
    sink: LineSink,
    log: ProtocolLog,
    setup: SessionSetup,
    native_id: String,
    spawn_id: String,
    next_id: u64,
    pending: BTreeMap<String, Pending>,
    approvals: BTreeMap<String, Approval>,
    questions: BTreeMap<String, Questions>,
    elicitations: BTreeMap<String, Elicit>,
    /// The last report of what the session loaded.
    loaded: Option<o7_protocol::session::SessionLoaded>,
    translator: translate::Translator,
    outbox: VecDeque<DriverEvent>,
    held: VecDeque<(Instant, DriverCommand)>,
    init: Value,
    ready: bool,
    idle: bool,
    /// Background tasks Claude Code last listed as open (`background_tasks_changed`).
    background: u32,
    turn: Option<String>,
    model: String,
    observed_model: Option<String>,
    effort: Option<String>,
    mode: String,
    execution_mode: String,
    /// Whether Claude itself has reported or accepted Full access while this process ran. A
    /// saved setting alone does not count: a policy may refuse it.
    full_access_seen: bool,
    /// The places Claude takes this chat's permission rules from, or `None` when they cannot
    /// be worked out (`rules`).
    rule_files: Option<Vec<std::path::PathBuf>>,
    observed_mode: Option<String>,
    init_deadline: Instant,
    commands_due: Option<Instant>,
    command_list: Option<Vec<o7_protocol::SlashCommand>>,
    failure: Option<DriverError>,
    stopping: bool,
    usage: UsageInfo,
    telemetry_dirty: [bool; 2],
}
impl Session {
    fn send(&mut self, value: Value) {
        if self.sink.send(value.to_string()).is_err() {
            self.failure
                .get_or_insert_with(|| error(ErrorKind::AgentExited, "Claude closed its input."));
        }
    }
    fn request(&mut self, request: Value, purpose: Purpose) {
        self.request_until(
            request,
            purpose,
            Instant::now() + self.setup.timings.request,
        );
    }
    fn request_until(&mut self, request: Value, purpose: Purpose, deadline: Instant) {
        self.next_id += 1;
        let id = format!("aura_{}", self.next_id);
        self.pending
            .insert(id.clone(), Pending { purpose, deadline });
        self.send(json!({"type":"control_request","request_id":id,"request":request}));
    }
    fn response(&mut self, id: &str, response: Value) {
        self.send(json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":response}}));
    }
    fn notice(&mut self, message: impl Into<String>) {
        self.outbox.push_back(DriverEvent::Event {
            turn: None,
            event: O7Event::Notice {
                level: NoticeLevel::Warning,
                message: message.into(),
            },
        });
    }
    fn oversized(&mut self, kind: Option<&str>) {
        if matches!(
            kind,
            Some(
                "control_request"
                    | "control_response"
                    | "control_cancel_request"
                    | "result"
                    | "system"
            )
        ) {
            let message = "Claude sent a required control update larger than 64 MiB. The session stopped with its partial output preserved.";
            self.notice(message);
            self.failure = Some(error(ErrorKind::ProtocolError, message));
        } else {
            self.notice("Claude sent an update larger than 64 MiB; it was skipped.");
        }
    }
    fn options(&mut self) {
        let mut options = translate::options(&self.init, &self.model, self.effort.as_deref());
        options.push(crate::controls::select(
            crate::controls::ACCESS,
            "Access",
            &self.execution_mode,
            vec![
                crate::controls::choice("default", "Ask for approval"),
                crate::controls::choice("acceptEdits", "Allow edits"),
                crate::controls::choice("bypassPermissions", "Full access"),
            ],
        ));
        options.push(crate::controls::workflow(if self.mode == "plan" {
            "plan"
        } else {
            "execute"
        }));
        self.outbox.push_back(DriverEvent::Options(options));
    }
    fn startup_mode(&self) -> Result<String, DriverError> {
        let execution = match self.setup.desired.get(crate::controls::ACCESS) {
            Some(ConfigValue::Choice { value })
                if matches!(
                    value.as_str(),
                    "default" | "acceptEdits" | "bypassPermissions"
                ) =>
            {
                value.clone()
            }
            Some(_) => {
                return Err(error(
                    ErrorKind::ProtocolError,
                    "The saved Claude access setting is unavailable.",
                ));
            }
            None => mode(self.setup.autonomy).into(),
        };
        match self.setup.desired.get(crate::controls::WORKFLOW) {
            Some(ConfigValue::Choice { value }) if value == "plan" => Ok("plan".into()),
            Some(ConfigValue::Choice { value }) if value == "execute" => Ok(execution),
            Some(_) => Err(error(
                ErrorKind::ProtocolError,
                "The saved Claude workflow is unavailable.",
            )),
            None if self.mode == "plan" => Ok("plan".into()),
            None => Ok(execution),
        }
    }
    /// Claude reported the model it runs, often as a resolved id (`claude-opus-5-5`). A report
    /// that resolves to the model already selected keeps the selection: several aliases resolve to
    /// one id (`default` and `opus`), and the first match would show the wrong one. A different
    /// model replaces it and drops an explicit effort chosen for the old one.
    fn apply_model(&mut self, model: &str) -> bool {
        if model.is_empty()
            || self.model == model
            || model_identity(&self.init, &self.model) == model_identity(&self.init, model)
        {
            return false;
        }
        self.select_model(model);
        true
    }
    /// The person chose `chosen` and Claude answered with the model it runs now (`reported`). The
    /// choice stays the selection unless Claude runs a model it does not resolve to.
    fn choose_model(&mut self, chosen: &str, reported: Option<&str>) {
        let next = match reported.filter(|model| !model.is_empty()) {
            Some(reported)
                if model_identity(&self.init, chosen) != model_identity(&self.init, reported) =>
            {
                reported
            }
            _ => chosen,
        };
        if !next.is_empty() && self.model != next {
            self.select_model(next);
        }
    }
    fn select_model(&mut self, model: &str) {
        if model_identity(&self.init, &self.model) != model_identity(&self.init, model) {
            self.effort = None;
        }
        self.model = model.to_owned();
    }
    fn native_metadata(&mut self, frame: &Value) {
        if frame["type"] != "system"
            || self.stopping
            || self.failure.is_some()
            || frame
                .get("parent_tool_use_id")
                .is_some_and(|parent| !parent.is_null())
        {
            return;
        }
        let init = frame["subtype"] == "init";
        if !init && frame["subtype"] != "status" {
            return;
        }
        if init && !self.native_identity(frame) {
            return;
        }
        // The first turn's init names what the session loaded; the initialize reply only its
        // subagents. A report goes out only when it changes.
        if init {
            let loaded = translate::loaded(frame);
            if self.loaded.as_ref() != Some(&loaded) {
                self.loaded = Some(loaded.clone());
                self.outbox.push_back(DriverEvent::Loaded(loaded));
            }
        }
        if init && let Some(model) = frame["model"].as_str().filter(|value| !value.is_empty()) {
            self.observed_model = Some(model.into());
            if self.apply_model(model) && self.ready {
                self.options();
            }
        }
        if let Some(native) = frame["permissionMode"]
            .as_str()
            .filter(|value| !value.is_empty())
        {
            self.observed_mode = Some(native.into());
            if native == "bypassPermissions" {
                self.full_access_seen = true;
            }
            if self.mode != native {
                self.mode = native.into();
                if native != "plan" {
                    self.execution_mode = native.into();
                }
                if self.ready {
                    self.options();
                    self.outbox
                        .push_back(DriverEvent::Autonomy(autonomy(self.setup.autonomy, native)));
                }
            }
        }
    }
    fn native_identity(&mut self, frame: &Value) -> bool {
        let actual = frame["session_id"]
            .as_str()
            .and_then(|id| uuid::Uuid::parse_str(id).ok());
        let message = match actual {
            None => Some(
                "Claude reported an invalid native session id. Restart the session to continue.",
            ),
            Some(actual) => {
                if let Ok(expected) = uuid::Uuid::parse_str(&self.native_id) {
                    if actual == expected {
                        return true;
                    }
                    Some(
                        "Claude reported a different native session id than requested. The session was stopped to protect its transcript.",
                    )
                } else if self.setup.resume.is_some() && !self.setup.native_start.fork {
                    self.native_id = actual.to_string();
                    if self.ready {
                        self.outbox
                            .push_back(DriverEvent::NativeSessionId(self.native_id.clone()));
                    }
                    return true;
                } else {
                    Some(
                        "Claude reported a different native session id than requested. The session was stopped to protect its transcript.",
                    )
                }
            }
        };
        if let Some(message) = message {
            self.notice(message);
            self.failure = Some(error(ErrorKind::ProtocolError, message));
        }
        false
    }
    fn emit_usage(&mut self, update: UsageInfo, turn: Option<String>) {
        if merge_usage(&mut self.usage, update) {
            self.outbox.push_back(DriverEvent::Event {
                turn,
                event: O7Event::UsageUpdated(self.usage.clone()),
            });
        }
    }
    fn refresh_telemetry(&mut self) {
        if !self.ready || self.stopping || self.failure.is_some() {
            return;
        }
        for kind in [TelemetryKind::Context, TelemetryKind::Usage] {
            if self.pending.values().any(|pending| matches!(pending.purpose, Purpose::Telemetry(pending_kind) if pending_kind == kind)) {
                self.telemetry_dirty[kind.index()] = true;
            } else {
                self.request(kind.request(), Purpose::Telemetry(kind));
            }
        }
    }
    fn telemetry_reply(&mut self, kind: TelemetryKind, payload: &Value, failed: bool) {
        let dirty = std::mem::take(&mut self.telemetry_dirty[kind.index()]);
        if self.stopping || self.failure.is_some() {
            return;
        }
        if failed {
            // Error strings can include account details. The user only needs to know the meter
            // could not refresh; leave its last successful values intact.
            self.notice(match kind {
                TelemetryKind::Context => "Claude could not refresh context usage.",
                TelemetryKind::Usage => "Claude could not refresh usage limits.",
            });
        } else if !dirty {
            match kind {
                TelemetryKind::Context => self.emit_usage(translate::context_usage(payload), None),
                TelemetryKind::Usage => {
                    self.emit_usage(translate::result_usage(&payload["session"]), None);
                    let windows = usage_limits(payload);
                    if !windows.is_empty() {
                        self.outbox.push_back(DriverEvent::Event {
                            turn: None,
                            event: O7Event::LimitsUpdated { windows },
                        });
                    }
                }
            }
        }
        if dirty {
            self.request(kind.request(), Purpose::Telemetry(kind));
        }
    }
    fn finish_start(&mut self) {
        if self.ready || self.failure.is_some() || self.stopping {
            return;
        }
        self.ready = true;
        self.outbox
            .push_back(DriverEvent::RunState { idle: self.idle });
        self.outbox.push_back(DriverEvent::Autonomy(autonomy(
            self.setup.autonomy,
            &self.mode,
        )));
        self.options();
        self.outbox
            .push_back(DriverEvent::Commands(translate::reduce_commands(
                &self.init,
            )));
        self.outbox.push_back(DriverEvent::Ready(ReadyInfo {
            native_session_id: Some(self.native_id.clone()),
            agent_name: Some("Claude Code".into()),
            agent_version: self.init["claude_code_version"].as_str().map(str::to_owned),
            capabilities: Capabilities {
                images: true,
                interrupt: true,
                resume: true,
                fork: true,
                rewind: self.setup.native_start.file_checkpointing,
                set_model: true,
                set_effort: true,
                permissions: true,
                usage: true,
                limits: true,
                slash_commands: true,
                add_dirs: true,
                plan_edits: true,
                autonomy_levels: vec![Autonomy::FullAuto, Autonomy::AutoEdit, Autonomy::Ask],
                ..Default::default()
            },
            resumed: if self.setup.resume.is_some() && !self.setup.native_start.fork {
                ResumeOutcome::Resumed
            } else {
                ResumeOutcome::New
            },
            spawn_id: self.spawn_id.clone(),
            replay: None,
        }));
        let loaded = translate::loaded(&self.init);
        self.loaded = Some(loaded.clone());
        self.outbox.push_back(DriverEvent::Loaded(loaded));
        self.refresh_telemetry();
    }
    /// A saved effort the model no longer offers (it was chosen for another model, or Claude
    /// changed its levels) is left out with a notice; the session starts at the model's default.
    fn configure_effort(&mut self) {
        if let Some(value) = self.setup.desired.get("effort").cloned() {
            match self.effort_request(&value) {
                Ok(request) => self.request(request, Purpose::InitialEffort),
                Err(_) => {
                    self.setup.desired.remove("effort");
                    self.outbox.push_back(DriverEvent::Event {
                        turn: None,
                        event: O7Event::Notice {
                            level: NoticeLevel::Info,
                            message: "This model does not offer the thinking level chosen earlier, so it starts at its default.".into(),
                        },
                    });
                    self.finish_start();
                }
            }
        } else {
            self.finish_start();
        }
    }
    fn effort_request(&self, value: &ConfigValue) -> Result<Value, DriverError> {
        let ConfigValue::Choice { value: effort } = value else {
            return Err(error(
                ErrorKind::ProtocolError,
                "Effort must be a supported level.",
            ));
        };
        if effort == translate::DEFAULT_EFFORT {
            return Ok(json!({"subtype":"apply_flag_settings","settings":{"effortLevel":null}}));
        }
        let model = model_metadata(&self.init, &self.model);
        if !model
            .and_then(|m| m["supportedEffortLevels"].as_array())
            .is_some_and(|levels| levels.iter().any(|l| l.as_str() == Some(effort)))
        {
            return Err(error(
                ErrorKind::ProtocolError,
                "This model does not offer that effort level.",
            ));
        }
        Ok(json!({"subtype":"apply_flag_settings","settings":{"effortLevel":effort}}))
    }
    fn configuring(&self) -> bool {
        self.pending.values().any(|p| {
            matches!(
                p.purpose,
                Purpose::Option { .. } | Purpose::Autonomy { .. } | Purpose::Rewind { .. }
            )
        })
    }
    fn command(&mut self, command: DriverCommand) {
        log_command(&self.log, &command);
        self.dispatch(Instant::now(), command);
    }
    fn dispatch(&mut self, arrived: Instant, command: DriverCommand) {
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
                    let _ = reply.send(Err(error(
                        ErrorKind::ProtocolError,
                        "Select a shared MCP server loaded in this idle session.",
                    )));
                    return;
                }
                if action == o7_protocol::mcp::McpSessionAction::Status {
                    self.request(
                        json!({"subtype":"mcp_status"}),
                        Purpose::Mcp {
                            server_name,
                            action,
                            reply,
                        },
                    );
                } else {
                    crate::mcp::unsupported(
                        reply,
                        "Claude's OAuth flow requires its native MCP panel. Session status and local header credentials remain available here.",
                    );
                }
            }
            DriverCommand::Prompt { turn, parts } => {
                // An open prompt of ours, or a request waiting for its answer, refuses another.
                // Claude Code's own work after a result (Stop hooks, background agents) does not:
                // it queues a prompt that arrives meanwhile, and the session sends one then only
                // after its idle grace or the person's Send now.
                if !self.ready
                    || self.turn.is_some()
                    || !self.approvals.is_empty()
                    || !self.questions.is_empty()
                    || !self.elicitations.is_empty()
                {
                    self.outbox.push_back(DriverEvent::TurnEnded {
                        turn_id: turn.turn_id,
                        stop_reason: StopReason::Error,
                        usage: None,
                        error: Some(error(
                            ErrorKind::Internal,
                            "Claude is not ready for another prompt.",
                        )),
                    });
                    return;
                }
                if self.configuring() {
                    self.held
                        .push_back((arrived, DriverCommand::Prompt { turn, parts }));
                    return;
                }
                let content = match crate::attachments::claude(
                    &parts,
                    self.setup.attachment_dir.as_deref(),
                ) {
                    Ok(content) => content,
                    Err(message) => {
                        self.outbox.push_back(DriverEvent::TurnEnded {
                            turn_id: turn.turn_id,
                            stop_reason: StopReason::Error,
                            usage: None,
                            error: Some(error(ErrorKind::ProtocolError, &message)),
                        });
                        return;
                    }
                };
                if !self.idle {
                    tracing::info!(
                        "Claude Code still reports work in progress; it queues this prompt behind it"
                    );
                }
                self.translator.begin_turn();
                self.idle = false;
                self.turn = Some(turn.turn_id.clone());
                self.outbox.push_back(DriverEvent::RunState { idle: false });
                self.send(json!({"type":"user","message":{"role":"user","content":content},"parent_tool_use_id":null,"session_id":self.native_id,"uuid":turn.turn_id}));
            }
            DriverCommand::Interrupt => self.interrupt(),
            DriverCommand::Steer { reply, .. } => {
                let _ = reply.send(crate::driver::SteerResult::Rejected {
                    message: "This agent cannot steer an exact running turn.".into(),
                });
            }
            DriverCommand::RewindFiles {
                user_message_id,
                dry_run,
                reply,
            } => {
                if reply.is_closed() {
                    return;
                }
                let rejection = if !self.setup.native_start.file_checkpointing {
                    Some("Native file checkpointing was not enabled for this session.")
                } else if uuid::Uuid::parse_str(&user_message_id).is_err() {
                    Some("The file checkpoint message id must be a UUID.")
                } else if !self.ready
                    || !self.idle
                    || self.turn.is_some()
                    || !self.approvals.is_empty()
                    || !self.questions.is_empty()
                    || !self.elicitations.is_empty()
                    || self.configuring()
                {
                    Some("Wait for Claude to become idle before rewinding files.")
                } else {
                    None
                };
                if let Some(message) = rejection {
                    let _ = reply.send(Err(error(ErrorKind::ProtocolError, message)));
                    return;
                }
                self.request(
                    json!({"subtype":"rewind_files","user_message_id":user_message_id,"dry_run":dry_run}),
                    Purpose::Rewind { dry_run, reply },
                );
            }
            DriverCommand::EditPlan { request_id, plan } => {
                // Claude Code takes the plan it reports as approved from the approval's own
                // input: a `plan` there is the plan as the person left it, and it says so to
                // the model ("Approved Plan (edited by user)"). Only a request that came with a
                // plan can have it replaced.
                if let Some(approval) = self.approvals.get_mut(&request_id)
                    && approval.request["input"]["plan"].is_string()
                {
                    approval.request["input"]["plan"] = Value::String(plan);
                }
            }
            DriverCommand::RespondPermission {
                request_id,
                outcome,
            } => {
                if let Some(approval) = self.approvals.remove(&request_id) {
                    let mut response =
                        json!({"behavior":"deny","message":"The user declined this request."});
                    if let PermissionOutcome::Selected(choice) = outcome
                        && approval.allowed.contains(&choice)
                    {
                        if choice == "allow" {
                            response = json!({"behavior":"allow","updatedInput":approval.request["input"]});
                        } else if let Some(index) = choice
                            .strip_prefix("always:")
                            .and_then(|n| n.parse::<usize>().ok())
                            && let Some(suggestion) = approval.request["permission_suggestions"]
                                .as_array()
                                .and_then(|s| s.get(index))
                        {
                            response = json!({"behavior":"allow","updatedInput":approval.request["input"],"updatedPermissions":[suggestion]});
                        }
                    }
                    self.response(&request_id, response);
                }
            }
            DriverCommand::RespondUi { request_id, answer } => {
                // A form answer that does not fit keeps the form open (the session checks it
                // first, so this is only a stale or forged answer).
                if let Some(result) = self
                    .elicitations
                    .get(&request_id)
                    .and_then(|pending| pending.spec.result(answer.as_ref()))
                {
                    self.elicitations.remove(&request_id);
                    self.response(&request_id, result);
                    return;
                }
                if let Some(pending) = self.questions.remove(&request_id) {
                    let mut input = pending.input;
                    let result = question_answers(&pending.questions, answer);
                    if let Some(answers) = result {
                        input["answers"] = answers;
                        self.response(
                            &request_id,
                            json!({"behavior":"allow","updatedInput":input}),
                        );
                    } else {
                        self.response(&request_id,json!({"behavior":"deny","message":"The questions were dismissed or the answers were invalid."}));
                    }
                }
            }
            DriverCommand::SetOption {
                option_id,
                value,
                reply,
            } => {
                if reply.is_closed() {
                    return;
                }
                let deadline = arrived + self.setup.timings.request;
                if Instant::now() >= deadline {
                    let _ = reply.send(Err(config_timeout()));
                    return;
                }
                if self.configuring() || !self.ready {
                    self.held.push_back((
                        arrived,
                        DriverCommand::SetOption {
                            option_id,
                            value,
                            reply,
                        },
                    ));
                    return;
                }
                let request = match (&*option_id, &value) {
                    ("model", ConfigValue::Choice { value: model }) => {
                        Ok(json!({"subtype":"set_model","model":model}))
                    }
                    (crate::controls::WORKFLOW, ConfigValue::Choice { value })
                        if matches!(value.as_str(), "plan" | "execute")
                            && self.idle
                            && self.turn.is_none()
                            && self.approvals.is_empty()
                            && self.questions.is_empty()
                            && self.elicitations.is_empty() =>
                    {
                        Ok(
                            json!({"subtype":"set_permission_mode","mode":if value == "plan" { "plan" } else { &self.execution_mode }}),
                        )
                    }
                    (crate::controls::ACCESS, ConfigValue::Choice { value })
                        if matches!(
                            value.as_str(),
                            "default" | "acceptEdits" | "bypassPermissions"
                        ) && self.mode != "plan" =>
                    {
                        Ok(json!({"subtype":"set_permission_mode","mode":value}))
                    }
                    ("effort", _) => self.effort_request(&value),
                    _ => Err(error(
                        ErrorKind::ProtocolError,
                        "This setting is not supported by Claude.",
                    )),
                };
                match request {
                    Ok(request) => self.request_until(
                        request,
                        Purpose::Option {
                            id: option_id,
                            value,
                            reply,
                        },
                        deadline,
                    ),
                    Err(err) => {
                        let _ = reply.send(Err(err));
                    }
                }
            }
            DriverCommand::SetAutonomy {
                autonomy: value,
                reply,
            } => {
                if reply.is_closed() {
                    return;
                }
                let deadline = arrived + self.setup.timings.request;
                if Instant::now() >= deadline {
                    let _ = reply.send(Err(config_timeout()));
                    return;
                }
                if self.configuring() || !self.ready {
                    self.held.push_back((
                        arrived,
                        DriverCommand::SetAutonomy {
                            autonomy: value,
                            reply,
                        },
                    ));
                    return;
                }
                self.request_until(
                    json!({"subtype":"set_permission_mode","mode":mode(value)}),
                    Purpose::Autonomy { value, reply },
                    deadline,
                );
            }
            DriverCommand::Shutdown { .. } => {}
        }
    }
    fn interrupt(&mut self) {
        let mut retained = VecDeque::new();
        while let Some((arrived, command)) = self.held.pop_front() {
            if let DriverCommand::Prompt { turn, .. } = command {
                self.outbox.push_back(DriverEvent::TurnEnded {
                    turn_id: turn.turn_id,
                    stop_reason: StopReason::Cancelled,
                    usage: None,
                    error: None,
                });
                self.outbox
                    .push_back(DriverEvent::RunState { idle: self.idle });
            } else {
                retained.push_back((arrived, command));
            }
        }
        self.held = retained;
        if !self
            .pending
            .values()
            .any(|p| matches!(p.purpose, Purpose::Interrupt))
        {
            self.request(json!({"subtype":"interrupt"}), Purpose::Interrupt);
        }
    }
    fn withdraw(&mut self, id: &str) {
        if self.approvals.remove(id).is_some() {
            self.outbox.push_back(DriverEvent::PermissionWithdrawn {
                request_id: id.into(),
            });
        }
        if self.questions.remove(id).is_some() || self.elicitations.remove(id).is_some() {
            self.outbox.push_back(DriverEvent::UiWithdrawn {
                request_id: id.into(),
            });
        }
    }
    fn withdraw_turn(&mut self) {
        let ids: Vec<_> = self
            .approvals
            .iter()
            .filter(|(_, p)| p.turn == self.turn)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.response(
                &id,
                json!({"behavior":"deny","message":"The turn has ended."}),
            );
            self.withdraw(&id);
        }
        let ids: Vec<_> = self
            .questions
            .iter()
            .filter(|(_, p)| p.turn == self.turn)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.response(
                &id,
                json!({"behavior":"deny","message":"The turn has ended."}),
            );
            self.withdraw(&id);
        }
        let ids: Vec<_> = self
            .elicitations
            .iter()
            .filter(|(_, p)| p.turn.is_some() && p.turn == self.turn)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.response(&id, json!({"action":"cancel"}));
            self.withdraw(&id);
        }
    }
    fn withdraw_all(&mut self) {
        let ids: Vec<_> = self.elicitations.keys().cloned().collect();
        for id in ids {
            self.response(&id, json!({"action":"cancel"}));
            self.withdraw(&id);
        }
        let ids = self
            .approvals
            .keys()
            .chain(self.questions.keys())
            .cloned()
            .collect::<Vec<_>>();
        for id in ids {
            self.response(
                &id,
                json!({"behavior":"deny","message":"The session is closing."}),
            );
            self.withdraw(&id);
        }
    }
    fn incoming(&mut self, incoming: wire::Incoming) {
        match incoming {
            wire::Incoming::Cancel { id } => self.withdraw(&id),
            wire::Incoming::Invalid(_) => {
                self.notice("Claude sent a malformed control frame; it was ignored.")
            }
            wire::Incoming::Request { id, request } => self.approval(id, request),
            wire::Incoming::Response { id, response } => self.control_reply(id, response),
            wire::Incoming::Event(frame) => self.event(frame),
        }
    }
    /// An MCP server's form or browser step. Claude cancels it itself if the reply is an error,
    /// so one o7 cannot show is cancelled with a notice saying where to finish it.
    fn elicitation(&mut self, id: String, request: &Value) {
        if self.elicitations.contains_key(&id) {
            return;
        }
        let source = request["display_name"]
            .as_str()
            .or(request["mcp_server_name"].as_str());
        let message = request["message"]
            .as_str()
            .filter(|message| !message.trim().is_empty())
            .unwrap_or("An MCP server needs your input.");
        let parsed = if self.stopping || self.failure.is_some() {
            Err(String::new())
        } else {
            match request["mode"].as_str() {
                Some("url") => crate::elicitation::link(
                    message,
                    source,
                    request["url"].as_str().unwrap_or_default(),
                ),
                None | Some("form") => {
                    let order: Option<Vec<String>> =
                        request["property_order"].as_array().map(|keys| {
                            keys.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        });
                    crate::elicitation::form(
                        message,
                        source,
                        &request["requested_schema"],
                        order.as_deref(),
                    )
                }
                Some(_) => Err("An MCP server asked for input o7 cannot show.".into()),
            }
        };
        match parsed {
            Ok((prompt, spec)) => {
                self.outbox.push_back(DriverEvent::UiRequest {
                    turn: self.turn.clone(),
                    request: UiRequest {
                        request_id: id.clone(),
                        prompt,
                    },
                });
                self.elicitations.insert(
                    id,
                    Elicit {
                        spec,
                        turn: self.turn.clone(),
                    },
                );
            }
            Err(reason) => {
                if !self.sink.is_closed() {
                    self.response(&id, json!({"action":"cancel"}));
                }
                if !reason.is_empty() {
                    self.notice(format!(
                        "{reason} It was cancelled; finish it in Claude Code's own terminal if you need it."
                    ));
                }
            }
        }
    }
    fn approval(&mut self, id: String, request: Value) {
        if request["subtype"] == "elicitation" {
            self.elicitation(id, &request);
            return;
        }
        if request["subtype"] == "request_user_dialog" {
            // No dialog kinds were advertised by our initialize request. The native dialog
            // payload is opaque. Reject the control request explicitly so it cannot remain
            // pending forever, without claiming to have answered its contents.
            if !self.sink.is_closed() {
                self.send(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"This native dialog is not supported by o7."}}));
            }
            self.notice("Claude requested an unsupported native dialog.");
            return;
        }
        if self.stopping || self.failure.is_some() {
            if !self.sink.is_closed() {
                self.response(
                    &id,
                    json!({"behavior":"deny","message":"The session is closing."}),
                );
            }
            return;
        }
        if self.approvals.contains_key(&id) || self.questions.contains_key(&id) {
            return;
        }
        if request["subtype"] != "can_use_tool" {
            self.send(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"This control request is not supported by o7."}}));
            return;
        }
        if request["tool_name"] == "AskUserQuestion" {
            if let Some(questions) = parse_questions(&request["input"]) {
                self.outbox.push_back(DriverEvent::UiRequest {
                    turn: self.turn.clone(),
                    request: UiRequest {
                        request_id: id.clone(),
                        prompt: UiPrompt::Questions {
                            message: "Claude needs your input".into(),
                            questions: questions.clone(),
                        },
                    },
                });
                self.questions.insert(
                    id,
                    Questions {
                        input: request["input"].clone(),
                        questions,
                        turn: self.turn.clone(),
                    },
                );
            } else {
                self.response(&id,json!({"behavior":"deny","message":"Claude sent an unsupported question structure."}));
            }
            return;
        }
        if self.plans_unasked(&request) {
            self.response(
                &id,
                json!({"behavior":"allow","updatedInput":request["input"]}),
            );
            return;
        }
        let title = request["title"]
            .as_str()
            .or(request["description"].as_str())
            .or(request["display_name"].as_str())
            .unwrap_or("Claude asks for permission")
            .to_owned();
        let mut options = vec![
            PermissionOption {
                option_id: "allow".into(),
                kind: PermissionOptionKind::AllowOnce,
                label: "Allow once".into(),
            },
            PermissionOption {
                option_id: "deny".into(),
                kind: PermissionOptionKind::RejectOnce,
                label: "Deny".into(),
            },
        ];
        if request["suppress_always_allow_rule"] != true
            && let Some(suggestions) = request["permission_suggestions"].as_array()
        {
            for (i, suggestion) in suggestions.iter().enumerate() {
                if suggestion["type"] == "addRules" && suggestion["behavior"] == "allow" {
                    options.push(PermissionOption {
                        option_id: format!("always:{i}"),
                        kind: PermissionOptionKind::AllowAlways,
                        label: "Always allow this rule".into(),
                    });
                }
            }
        }
        let native_tool = request["tool_use_id"].as_str().unwrap_or("");
        let allowed = options.iter().map(|o| o.option_id.clone()).collect();
        // Plan mode's own tools are marked, so approving ExitPlanMode counts as the implementation
        // approval that lets the session follow Claude out of Plan. Every other request stays
        // unclassified: Claude applies its own permission mode to them.
        let kind = request["tool_name"]
            .as_str()
            .map(super::translate::tool_kind)
            .filter(|kind| *kind == o7_protocol::ToolKind::SwitchMode);
        let card = PermissionRequest {
            request_id: id.clone(),
            tool_call_id: self.translator.tool_call_id(native_tool).map(str::to_owned),
            title,
            kind,
            raw_input: request.get("input").cloned(),
            content: vec![],
            locations: vec![],
            options,
            truncated: None,
        };
        self.approvals.insert(
            id,
            Approval {
                request,
                turn: self.turn.clone(),
                allowed,
            },
        );
        self.outbox.push_back(DriverEvent::Permission {
            turn: self.turn.clone(),
            request: card,
        });
    }
    /// Takes the mode Claude now runs in as the chat's execution access, unless it is Plan, and
    /// notes Full access once Claude has granted it.
    fn adopt_mode(&mut self) {
        if self.mode != "plan" {
            self.execution_mode = self.mode.clone();
        }
        if self.mode == "bypassPermissions" {
            self.full_access_seen = true;
        }
    }
    /// Whether a request made while planning is answered here instead of shown. Claude's Plan mode
    /// takes the place of the chat's access mode, so under Full access every read, search and
    /// command of a planning turn would otherwise ask, which that access level never does while
    /// executing. Where Claude has granted Full access in this process, such a request is allowed
    /// as that level would allow it, except:
    /// - leaving Plan, which is the person's approval of the plan;
    /// - a file edit, which a planning turn has no business making;
    /// - a request Claude says needs a person, or raises because of an ask rule, a hook or a
    ///   safety check: those ask under Full access too.
    ///
    /// Claude says why it asks (`decision_reason_type`). The reasons that are Plan standing in
    /// for the access mode are the ones answered, as recorded from real planning turns (21
    /// November 2026): none or `mode`; `other` ("A variable in this command can't be checked
    /// before it runs") and `subcommandResults`, which is how a command made of several, or one
    /// it cannot prove read-only, is asked about; and `workingDir`, a path outside the chat's
    /// folder. Any other reason, one this build does not know included, is shown.
    ///
    /// One case cannot be told apart from the request: an ask rule that matches one part of a
    /// compound command is reported as `subcommandResults` too. So such a request is answered
    /// only while no ask rule can exist in any place Claude takes this chat's rules from
    /// (`rules`), looked at when it arrives. With one written, or a place that cannot be
    /// looked at, it is shown.
    fn plans_unasked(&self, request: &Value) -> bool {
        if self.mode != "plan"
            || self.execution_mode != "bypassPermissions"
            || !self.full_access_seen
        {
            return false;
        }
        if request["requires_user_interaction"] == true || request["default_to_no"] == true {
            return false;
        }
        if !matches!(
            request["decision_reason_type"].as_str(),
            None | Some("mode" | "other" | "subcommandResults" | "workingDir")
        ) {
            return false;
        }
        let Some(tool) = request["tool_name"].as_str() else {
            return false;
        };
        if matches!(
            super::translate::tool_kind(tool),
            o7_protocol::ToolKind::SwitchMode | o7_protocol::ToolKind::Edit
        ) {
            return false;
        }
        request["decision_reason_type"] != "subcommandResults"
            || !super::rules::asks_by_rule(self.rule_files.as_deref())
    }
    fn control_reply(&mut self, id: String, response: Value) {
        let Some(pending) = self.pending.remove(&id) else {
            return;
        };
        if !self.ready && (self.failure.is_some() || self.stopping) {
            return;
        }
        let failure = (response["subtype"] == "error").then(|| {
            error(
                ErrorKind::AgentError,
                response["error"]
                    .as_str()
                    .unwrap_or("Claude rejected a control request."),
            )
        });
        let payload = &response["response"];
        match pending.purpose {
            Purpose::Mcp {
                server_name,
                action,
                reply,
            } => {
                let answer = if failure.is_some() {
                    Err(error(
                        ErrorKind::AgentError,
                        "Claude rejected the MCP status request.",
                    ))
                } else {
                    crate::mcp::response(payload, action, &server_name, false)
                };
                let _ = reply.send(answer);
            }
            Purpose::Initialize => {
                if let Some(err) = failure {
                    self.failure = Some(err);
                    return;
                }
                if !payload.is_object()
                    || !matches!(
                        payload["session_state"].as_str(),
                        Some("idle" | "running" | "requires_action")
                    )
                {
                    self.failure = Some(error(
                        ErrorKind::ProtocolMismatch,
                        "Claude returned an initialize response without a valid session state.",
                    ));
                    return;
                }
                self.init = payload.clone();
                self.mode = self
                    .observed_mode
                    .as_deref()
                    .or_else(|| payload["current_permission_mode"].as_str())
                    .unwrap_or("default")
                    .into();
                self.idle = payload["session_state"] == "idle";
                let desired = match self.setup.desired.get("model") {
                    Some(ConfigValue::Choice { value }) => Some(value.clone()),
                    _ => None,
                };
                // An early `system/init` names the resolved id; the alias chosen for it stays.
                self.model = match (self.observed_model.clone(), desired) {
                    (Some(observed), Some(desired))
                        if model_identity(&self.init, &observed)
                            == model_identity(&self.init, &desired) =>
                    {
                        desired
                    }
                    (Some(observed), _) => observed,
                    (None, Some(desired)) => desired,
                    (None, None) => payload["model"]
                        .as_str()
                        .map_or_else(|| "default".into(), str::to_owned),
                };
                for key in [
                    "pending_permission_requests",
                    "pending_user_dialog_requests",
                ] {
                    if let Some(requests) = response[key].as_array() {
                        for item in requests {
                            if let (Some(id), Some(request)) =
                                (item["request_id"].as_str(), item.get("request"))
                            {
                                self.approval(id.into(), request.clone());
                            }
                        }
                    }
                }
                let startup_mode = match self.startup_mode() {
                    Ok(mode) => mode,
                    Err(err) => {
                        self.failure = Some(err);
                        return;
                    }
                };
                self.execution_mode = match self.setup.desired.get(crate::controls::ACCESS) {
                    Some(ConfigValue::Choice { value }) => value.clone(),
                    _ => mode(self.setup.autonomy).into(),
                };
                if self.mode == "bypassPermissions" {
                    self.full_access_seen = true;
                }
                if startup_mode == "plan"
                    && self.execution_mode == "bypassPermissions"
                    && !self.full_access_seen
                {
                    // A chat that opens in Plan remembers Full access without Claude having
                    // granted it in this process. Ask for it first, so planning goes unasked
                    // only where Claude allows that level.
                    self.request(
                        json!({"subtype":"set_permission_mode","mode":"bypassPermissions"}),
                        Purpose::ConfirmAccess,
                    );
                } else if self.mode != startup_mode {
                    self.request(
                        json!({"subtype":"set_permission_mode","mode":startup_mode}),
                        Purpose::InitialMode,
                    );
                } else {
                    self.configure_effort();
                }
            }
            Purpose::ConfirmAccess => {
                if let Some(err) = failure {
                    self.notice(format!(
                        "Claude refused Full access, so planning asks before each command: {}",
                        err.message
                    ));
                    self.execution_mode = if self.mode == "plan" {
                        "default".into()
                    } else {
                        self.mode.clone()
                    };
                } else {
                    self.mode = payload["mode"]
                        .as_str()
                        .unwrap_or("bypassPermissions")
                        .into();
                    self.adopt_mode();
                }
                if self.mode == "plan" {
                    self.configure_effort();
                } else {
                    self.request(
                        json!({"subtype":"set_permission_mode","mode":"plan"}),
                        Purpose::InitialMode,
                    );
                }
            }
            Purpose::InitialMode => {
                if let Some(err) = failure {
                    if self.setup.autonomy == Autonomy::FullAuto
                        && self.mode == "acceptEdits"
                        && !self.setup.desired.contains_key(crate::controls::WORKFLOW)
                        && !self.setup.desired.contains_key(crate::controls::ACCESS)
                    {
                        self.notice(format!(
                            "Claude refused Full auto. Auto-edit remains active: {}",
                            err.message
                        ));
                    } else {
                        self.failure = Some(err);
                        return;
                    }
                } else {
                    self.mode = payload["mode"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| self.startup_mode().unwrap_or_else(|_| "plan".into()));
                    self.adopt_mode();
                }
                self.configure_effort();
            }
            Purpose::InitialEffort => {
                if let Some(err) = failure {
                    self.failure = Some(err);
                } else {
                    self.effort = self.setup.desired.get("effort").and_then(|v| {
                        if let ConfigValue::Choice { value: s } = v {
                            explicit_effort(s)
                        } else {
                            None
                        }
                    });
                    self.finish_start();
                }
            }
            Purpose::Option { id, value, reply } => {
                if let Some(err) = failure {
                    let _ = reply.send(Err(err));
                } else {
                    if let ConfigValue::Choice { value } = value {
                        if id == crate::controls::WORKFLOW || id == crate::controls::ACCESS {
                            let expected = if id == crate::controls::ACCESS {
                                value.as_str()
                            } else if value == "plan" {
                                "plan"
                            } else {
                                &self.execution_mode
                            };
                            self.mode = payload["mode"].as_str().unwrap_or(expected).into();
                            self.adopt_mode();
                            self.outbox.push_back(DriverEvent::Autonomy(autonomy(
                                self.setup.autonomy,
                                &self.mode,
                            )));
                        } else if id == "model" {
                            self.choose_model(&value, payload["model"].as_str());
                        } else {
                            self.effort = explicit_effort(&value);
                        }
                    }
                    self.options();
                    let _ = reply.send(Ok(()));
                }
            }
            Purpose::Autonomy { value, reply } => {
                if let Some(err) = failure {
                    let _ = reply.send(Err(err));
                } else {
                    self.mode = payload["mode"].as_str().unwrap_or(mode(value)).into();
                    self.setup.autonomy = value;
                    self.adopt_mode();
                    self.options();
                    let state = autonomy(value, &self.mode);
                    self.outbox.push_back(DriverEvent::Autonomy(state.clone()));
                    let _ = reply.send(Ok(state));
                }
            }
            Purpose::Interrupt => {
                if let Some(err) = failure {
                    self.notice(err.message);
                }
            }
            Purpose::Rewind { dry_run, reply } => {
                let result = match failure {
                    Some(err) => Err(err),
                    None if payload["canRewind"] == false => Err(error(
                        ErrorKind::AgentError,
                        payload["error"]
                            .as_str()
                            .unwrap_or("Claude could not rewind this file checkpoint."),
                    )),
                    None if payload["canRewind"] == true => serde_json::from_value(payload.clone())
                        .map_err(|_| {
                            error(
                                ErrorKind::ProtocolError,
                                "Claude returned an invalid file rewind result.",
                            )
                        }),
                    None => Err(error(
                        ErrorKind::ProtocolError,
                        "Claude did not confirm whether files can be rewound.",
                    )),
                };
                // Rewinds are not transactional. Even an explicit vendor error can arrive after
                // some files changed, so only a successful real rewind can release held work.
                if !dry_run && result.is_err() {
                    let message = "Claude did not confirm a successful file rewind. Check the files before restarting this session.";
                    self.notice(message);
                    self.failure = Some(error(ErrorKind::ProtocolError, message));
                }
                let _ = reply.send(result);
            }
            Purpose::Telemetry(kind) => self.telemetry_reply(kind, payload, failure.is_some()),
        }
    }
    fn event(&mut self, frame: Value) {
        let draining_transport_failure = self
            .failure
            .as_ref()
            .is_some_and(|failure| failure.kind == ErrorKind::AgentExited);
        self.native_metadata(&frame);
        // Transport teardown can still have valid content buffered. A protocol or identity
        // failure invalidates subsequent content too, since its provenance is no longer known.
        if self.failure.is_some() && (!self.ready || !draining_transport_failure) {
            return;
        }
        if frame["type"] == "system"
            && frame["subtype"] == "session_state_changed"
            && let Some(state) = frame["state"].as_str()
        {
            self.idle = state == "idle";
            self.outbox
                .push_back(DriverEvent::RunState { idle: self.idle });
        }
        // Claude Code reports itself idle while a command it left in the background still runs,
        // and lists those tasks apart: the session must know, or it stops the agent as idle and
        // the command, and what Claude would have done when it finished, end with it.
        if frame["type"] == "system"
            && frame["subtype"] == "background_tasks_changed"
            && let Some(tasks) = frame["tasks"].as_array()
        {
            let open = u32::try_from(tasks.len()).unwrap_or(u32::MAX);
            if open != self.background {
                self.background = open;
                self.outbox.push_back(DriverEvent::BackgroundWork { open });
            }
        }
        if frame["type"] == "system" && frame["subtype"] == "commands_changed" {
            self.command_list = Some(translate::reduce_commands(&frame));
            if self.commands_due.is_none() {
                self.commands_due = Some(Instant::now() + self.setup.timings.commands_debounce);
            }
        }
        if !self.ready {
            return;
        }
        if frame["type"] == "result"
            && let Some(id) = frame["user_message_uuid"].as_str()
            && self.turn.as_deref() != Some(id)
        {
            return;
        }
        for event in self.translator.ingest(&frame) {
            match event {
                O7Event::UsageUpdated(usage) => self.emit_usage(usage, self.turn.clone()),
                event => self.outbox.push_back(DriverEvent::Event {
                    turn: self.turn.clone(),
                    event,
                }),
            }
        }
        // A subagent's request or background work can meet the limit too: the account is limited
        // either way, so every report counts, with or without an open turn.
        if frame["type"] == "assistant" && usage_limited(&frame) {
            self.outbox
                .push_back(DriverEvent::UsageLimit(usage_limit(&frame)));
        }
        if frame["type"] == "result" && self.turn.is_some() {
            let needs_auth = auth_required(&frame);
            let reason = result_reason(&frame, self.translator.main_stop_reason());
            let failed = reason == StopReason::Error;
            self.withdraw_turn();
            if let Some(turn_id) = self.turn.take() {
                self.outbox.push_back(DriverEvent::TurnEnded {
                    turn_id,
                    stop_reason: reason,
                    usage: translate::token_usage(&frame),
                    error: failed.then(|| {
                        if needs_auth {
                            return auth_error();
                        }
                        if usage_limited(&frame) {
                            return error(ErrorKind::UsageLimit, usage_limit(&frame).message);
                        }
                        error(
                            ErrorKind::AgentError,
                            frame["errors"]
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(Value::as_str)
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                })
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| "Claude could not complete this turn.".into()),
                        )
                    }),
                });
            }
            if needs_auth {
                // The session actor sees AuthRequired on TurnEnded, holds the failed prompt and
                // requests an orderly shutdown. Keep the login command in a separate UI event.
                self.outbox.push_back(DriverEvent::AuthRequired(auth_prompt(
                    &self.setup.launch.program,
                )));
            } else {
                self.refresh_telemetry();
            }
        }
    }
    fn expire(&mut self) {
        let now = Instant::now();
        if !self.ready && !self.stopping && self.failure.is_none() && now >= self.init_deadline {
            self.failure = Some(error(
                ErrorKind::StartTimeout,
                "Claude did not initialize before the startup deadline.",
            ));
        }
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.deadline && !matches!(p.purpose, Purpose::Initialize))
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            let rewinding = self.pending.get(&id).is_some_and(|pending| {
                matches!(pending.purpose, Purpose::Rewind { dry_run: false, .. })
            });
            if self.pending.get(&id).is_some_and(|pending| {
                matches!(
                    pending.purpose,
                    Purpose::Option { .. }
                        | Purpose::Autonomy { .. }
                        | Purpose::Rewind { dry_run: false, .. }
                )
            }) {
                if let Some(pending) = self.pending.remove(&id) {
                    match pending.purpose {
                        Purpose::Option { reply, .. } => {
                            let _ = reply.send(Err(config_timeout()));
                        }
                        Purpose::Autonomy { reply, .. } => {
                            let _ = reply.send(Err(config_timeout()));
                        }
                        Purpose::Rewind { reply, .. } => {
                            let _ = reply.send(Err(error(ErrorKind::ProtocolError,
                                "Claude did not confirm the file rewind. Check the files before continuing.")));
                        }
                        _ => {}
                    }
                }
                let message = if rewinding {
                    "Claude did not confirm the file rewind. Check the files before restarting this session."
                } else {
                    "Claude did not confirm a settings change. Restart the session before sending another prompt."
                };
                self.notice(message);
                self.failure = Some(error(ErrorKind::ProtocolError, message));
                continue;
            }
            self.control_reply(id,json!({"subtype":"error","error":"Claude did not answer this control request in time."}));
        }
        let mut retained = VecDeque::new();
        while let Some((arrived, command)) = self.held.pop_front() {
            let expired = now >= arrived + self.setup.timings.request;
            match command {
                DriverCommand::SetOption { reply, .. } if expired || reply.is_closed() => {
                    let _ = reply.send(Err(config_timeout()));
                }
                DriverCommand::SetAutonomy { reply, .. } if expired || reply.is_closed() => {
                    let _ = reply.send(Err(config_timeout()));
                }
                command => retained.push_back((arrived, command)),
            }
        }
        self.held = retained;
        if self.commands_due.is_some_and(|due| now >= due) {
            self.commands_due = None;
            if let Some(commands) = self.command_list.take() {
                self.outbox.push_back(DriverEvent::Commands(commands));
            }
        }
    }
}

fn parse_questions(input: &Value) -> Option<Vec<UiQuestion>> {
    let items = input["questions"].as_array()?;
    if items.is_empty() || items.len() > 4 {
        return None;
    }
    let mut questions = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let question = item["question"].as_str()?.to_owned();
        if question.is_empty()
            || questions
                .iter()
                .any(|q: &UiQuestion| q.question == question)
        {
            return None;
        }
        let choices = item["options"].as_array()?;
        if !(2..=4).contains(&choices.len()) {
            return None;
        }
        let options = choices
            .iter()
            .map(|choice| {
                Some(UiQuestionOption {
                    label: choice["label"].as_str()?.into(),
                    description: choice["description"].as_str().unwrap_or("").into(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        questions.push(UiQuestion {
            id: format!("q{index}"),
            header: item["header"].as_str().unwrap_or("").into(),
            question,
            options,
            multi_select: item["multiSelect"].as_bool().unwrap_or(false),
            allow_free_text: true,
        });
    }
    Some(questions)
}

fn question_answers(questions: &[UiQuestion], answer: Option<UiAnswer>) -> Option<Value> {
    let UiAnswer::Questions { answers } = answer? else {
        return None;
    };
    if answers.len() != questions.len() {
        return None;
    }
    let mut result = serde_json::Map::new();
    for question in questions {
        let mut matching = answers.iter().filter(|a| a.question_id == question.id);
        let answer = matching.next()?;
        if matching.next().is_some() {
            return None;
        }
        let text = answer
            .text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if !question.multi_select
            && (answer.selected.len() > 1 || (!answer.selected.is_empty() && text.is_some()))
        {
            return None;
        }
        let mut labels = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for index in &answer.selected {
            if !seen.insert(index) {
                return None;
            }
            labels.push(question.options.get(*index as usize)?.label.clone());
        }
        if let Some(text) = text {
            labels.push(text.into());
        }
        if labels.is_empty() {
            return None;
        }
        result.insert(question.question.clone(), json!(labels.join(", ")));
    }
    Some(Value::Object(result))
}

pub(super) async fn run(ctx: DriverContext) -> DriverExit {
    let DriverContext {
        mut commands,
        events,
        spawner,
        setup,
    } = ctx;
    let native_id = match native_session_id(&setup) {
        Ok(id) => id,
        Err(error) => return DriverExit::StartFailed(error),
    };
    let spec = SpawnSpec {
        launch: launch(&setup, &native_id),
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
    let rule_files = super::rules::sources(&setup);
    let mut s = Session {
        sink: process.io.stdin,
        log: process.log,
        setup,
        rule_files,
        native_id,
        spawn_id: process.spawn_id.clone(),
        next_id: 0,
        pending: BTreeMap::new(),
        approvals: BTreeMap::new(),
        translator: translate::Translator::with_prefix(&process.spawn_id),
        outbox: early,
        held,
        init: Value::Null,
        ready: false,
        idle: false,
        background: 0,
        turn: None,
        model: "default".into(),
        observed_model: None,
        effort: None,
        mode: "default".into(),
        execution_mode: "default".into(),
        full_access_seen: false,
        observed_mode: None,
        init_deadline: Instant::now(),
        commands_due: None,
        command_list: None,
        failure: None,
        stopping: false,
        questions: BTreeMap::new(),
        elicitations: BTreeMap::new(),
        loaded: None,
        usage: UsageInfo::default(),
        telemetry_dirty: [false; 2],
    };
    s.init_deadline = Instant::now()
        + s.setup
            .timings
            .open_ceiling
            .min(s.setup.launch.startup_timeout);
    s.outbox
        .push_back(DriverEvent::Status(DriverStatus::Initializing));
    s.request(
        json!({"subtype":"initialize","hooks":null}),
        Purpose::Initialize,
    );
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
    let mut shutdown: Option<BoxFuture<'static, o7_process::ShutdownReport>> = None;
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
            s.sink.close();
            shutdown = Some(control.shutdown(plan));
        }
        if eof && process_done && s.outbox.is_empty() {
            break;
        }
        if s.ready
            && !s.configuring()
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
            .filter(|p| !matches!(p.purpose, Purpose::Initialize))
            .map(|p| p.deadline)
            .chain(
                s.held
                    .iter()
                    .filter(|(_, command)| is_config(command))
                    .map(|(arrived, _)| *arrived + s.setup.timings.request),
            )
            .chain((!s.ready && s.failure.is_none() && !requested_close).then_some(s.init_deadline))
            .chain(s.commands_due)
            .chain(closing.map(|c| c.0))
            .chain(drain_deadline.filter(|_| !full && !eof))
            .min()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400));
        tokio::select! {
            ()=stdin_observer.writer_closed(),if !s.stopping&&!process_done&&s.failure.is_none()=>{
                if let Some(info)=control.try_exited() {
                    // Process exit also closes stdin. It can win this select before `exit`;
                    // keep draining stdout without misclassifying that race as a write failure.
                    exited=Some(info);
                    process_done=true;
                } else {
                    let message = "Claude stopped accepting input. Restart the session to continue.";
                    s.notice(message);
                    s.failure=Some(error(ErrorKind::AgentExited,message));
                }
            },
            command=commands.recv(),if !requested_close&&s.failure.is_none()&&!s.stopping&&!process_done=>{match command {
                None|Some(DriverCommand::Shutdown {..})=>{let plan=match command {Some(DriverCommand::Shutdown {plan})=>plan,_=>crate::driver::StopPlan {cancel_grace:Duration::ZERO,process:ShutdownPlan::STANDARD}};
                    s.log.command(json!({"type":"shutdown"}));requested_close=true;s.stopping=true;s.outbox.push_back(DriverEvent::Status(DriverStatus::Closing));if !s.idle {s.interrupt();}closing=Some((Instant::now()+plan.cancel_grace,plan.process));},
                Some(command)=>s.command(command),
            }},
            permit=events.reserve(bytes),if !s.outbox.is_empty()=>{let Ok(permit)=permit else {control.kill_tree();return DriverExit::Closed;};if let Some(event)=s.outbox.pop_front() {permit.send(event);}},
            item=stdout.next(),if !eof&&!full=>match item {Some(StdoutItem::Line(line))=>s.incoming(wire::parse_line(&line)),Some(StdoutItem::Oversized {keys,..})=>s.oversized(keys.kind.as_deref()),None=>{eof=true;if !process_done && shutdown.is_none() {closing=Some((Instant::now()+Duration::from_secs(2),ShutdownPlan::FORCE));}}},
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
                "Claude exited before opening the session.",
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

#[cfg(test)]
#[path = "metadata_tests.rs"]
mod metadata_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn resetting_effort_sends_null_and_reports_default_after_an_explicit_choice() {
        let mut harness = super::super::tests::Harness::start().await;
        let initialize = harness.wire().await;
        let source =
            include_str!("../../../../docs/spikes/claude-control/fixtures/turn-basic.jsonl");
        let payload = source
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|frame| frame["response"]["response"]["models"].is_array())
            .expect("real initialize")["response"]["response"]
            .clone();
        harness.send(json!({"type":"control_response","response":{"subtype":"success","request_id":initialize["request_id"],"response":payload}})).await;
        loop {
            if matches!(harness.event().await, DriverEvent::Ready(_)) {
                break;
            }
        }
        for value in ["medium", translate::DEFAULT_EFFORT] {
            let (reply, receive) = oneshot::channel();
            harness
                .tx
                .send(DriverCommand::SetOption {
                    option_id: "effort".into(),
                    value: ConfigValue::Choice {
                        value: value.into(),
                    },
                    reply,
                })
                .expect("set effort");
            let request = harness.wire().await;
            assert_eq!(request["request"]["subtype"], "apply_flag_settings");
            assert_eq!(
                request["request"]["settings"]["effortLevel"],
                explicit_effort(value).map_or(Value::Null, Value::String)
            );
            harness.send(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":null}})).await;
            assert!(receive.await.expect("effort reply").is_ok());
            loop {
                if let DriverEvent::Options(options) = harness.event().await {
                    let effort = options
                        .iter()
                        .find(|option| option.id == "effort")
                        .expect("effort option");
                    let o7_protocol::session::ConfigKind::Select { current, choices } =
                        &effort.kind
                    else {
                        panic!("effort select")
                    };
                    assert_eq!(current, value);
                    assert!(choices.iter().any(|choice| choice.value == *current));
                    break;
                }
            }
        }
        assert_eq!(
            explicit_effort(translate::DEFAULT_EFFORT),
            None,
            "initial desired reset also stores no override"
        );
        harness.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn telemetry_errors_do_not_block_prompts_or_expose_vendor_account_text() {
        let mut harness = super::super::tests::Harness::start().await;
        let initialize = harness.wire().await;
        harness.send(json!({"type":"control_response","response":{
            "subtype":"success","request_id":initialize["request_id"],"response":{
                "current_permission_mode":"default","session_state":"idle","models":[],"commands":[]
            }
        }})).await;
        let mut requests = Vec::new();
        for _ in 0..2 {
            let wire = tokio::time::timeout(Duration::from_secs(2), harness.end.from_client.recv())
                .await
                .expect("telemetry deadline")
                .expect("telemetry request");
            requests.push(serde_json::from_str::<Value>(&wire).expect("telemetry JSON"));
        }
        assert_eq!(requests[0]["request"], TelemetryKind::Context.request());
        assert_eq!(requests[1]["request"], TelemetryKind::Usage.request());
        for request in requests {
            harness
                .send(json!({"type":"control_response","response":{
                    "subtype":"error","request_id":request["request_id"],
                    "error":"private-account@example.test belongs to Private Organization"
                }}))
                .await;
        }
        harness.prompt();
        assert_eq!(
            harness.wire().await["type"],
            "user",
            "telemetry never holds a prompt"
        );
        let mut notices = 0;
        while notices < 2 {
            let event = harness.event().await;
            let serialized = serde_json::to_string(&event).expect("serialize event");
            assert!(!serialized.contains("private-account"));
            assert!(!serialized.contains("Private Organization"));
            if let DriverEvent::Event {
                event: O7Event::Notice { message, .. },
                ..
            } = event
            {
                assert!(message.starts_with("Claude could not refresh"));
                notices += 1;
            }
        }
        harness.stop().await;
    }

    #[test]
    fn telemetry_requests_are_free_summaries_without_account_or_transcript_queries() {
        assert_eq!(
            TelemetryKind::Context.request(),
            json!({"subtype":"get_context_usage","detail":"summary"})
        );
        assert_eq!(
            TelemetryKind::Usage.request(),
            json!({"subtype":"get_usage","skip_behaviors":true})
        );
    }

    #[test]
    fn partial_and_empty_usage_updates_preserve_the_other_meter_fields() {
        let mut usage = UsageInfo::default();
        assert!(merge_usage(
            &mut usage,
            translate::result_usage(&json!({"total_cost_usd":0.03}))
        ));
        assert!(merge_usage(
            &mut usage,
            translate::context_usage(&json!({"totalTokens":250,"maxTokens":1000}))
        ));
        assert!(!merge_usage(&mut usage, UsageInfo::default()));
        assert_eq!(usage.context_used, Some(250));
        assert_eq!(usage.context_size, Some(1000));
        assert_eq!(usage.cost.as_ref().map(|cost| cost.amount), Some(0.03));
        assert!(merge_usage(
            &mut usage,
            translate::context_usage(&json!({"totalTokens":300}))
        ));
        assert!(merge_usage(
            &mut usage,
            translate::result_usage(&json!({"total_cost_usd":0.04}))
        ));
        assert_eq!(usage.context_used, Some(300));
        assert_eq!(usage.context_size, Some(1000));
        assert_eq!(usage.cost.as_ref().map(|cost| cost.amount), Some(0.04));
    }

    #[test]
    fn recorded_get_usage_normalizes_percentages_offsets_and_scoped_windows() {
        let source = include_str!("../../../../docs/spikes/claude-control/fixtures/controls.jsonl");
        let payload = source
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|frame| frame["response"]["request_id"] == "req_5_spike")
            .expect("recorded get_usage response")["response"]["response"]
            .clone();
        let windows = usage_limits(&payload);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].label, "5 hours");
        assert_eq!(windows[0].used_percent, 31.0);
        assert_eq!(windows[0].resets_at, Some(1792834799749));
        assert_eq!(windows[1].used_percent, 21.0);
        assert_eq!(windows[2].label, "7 days (Fable)");
        let serialized = serde_json::to_string(&windows).expect("serialize");
        assert!(!serialized.contains("subscription"));
        assert!(!serialized.contains("account"));
    }

    #[test]
    fn legacy_get_usage_windows_do_not_scale_percentages_or_accept_invalid_resets() {
        let windows = usage_limits(&json!({"rate_limits":{
            "five_hour":{"utilization":25,"resets_at":"2026-10-24T09:39:59.749255Z"},
            "seven_day":{"utilization":101,"resets_at":"invalid"},
            "bad":{"utilization":"unknown"}
        }}));
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].used_percent, 25.0);
        assert_eq!(windows[0].resets_at, Some(1792834799749));
        assert_eq!(windows[1].used_percent, 100.0);
        assert_eq!(windows[1].resets_at, None);
        assert!(usage_limits(&json!({})).is_empty());
    }

    #[test]
    fn only_confirmed_401_results_request_auth_with_the_users_native_binary() {
        assert!(auth_required(
            &json!({"type":"result","api_error_status":401})
        ));
        for frame in [
            json!({"type":"result","api_error_status":403}),
            json!({"type":"result","is_error":true}),
            json!({"type":"user","api_error_status":401}),
        ] {
            assert!(!auth_required(&frame));
        }
        let prompt = auth_prompt(std::path::Path::new("C:/Apps/Claude Code/claude.exe"));
        assert_eq!(
            prompt.commands[0].argv,
            ["C:/Apps/Claude Code/claude.exe", "auth", "login"]
        );
        assert!(prompt.commands[0].env.is_empty());
    }

    #[test]
    fn result_reasons_keep_native_limits_refusals_and_failures_distinct() {
        for (frame, expected) in [
            (
                json!({"terminal_reason":"aborted_tools","is_error":true}),
                StopReason::Cancelled,
            ),
            (
                json!({"terminal_reason":"aborted_streaming","is_error":true}),
                StopReason::Cancelled,
            ),
            (
                json!({"subtype":"error_max_turns","is_error":true}),
                StopReason::MaxTurnRequests,
            ),
            (
                json!({"terminal_reason":"max_turns","is_error":true}),
                StopReason::MaxTurnRequests,
            ),
            (json!({"stop_reason":"max_tokens"}), StopReason::MaxTokens),
            (json!({"stop_reason":"refusal"}), StopReason::Refusal),
            (
                json!({"subtype":"success","is_error":true,"api_error_status":401}),
                StopReason::Error,
            ),
            (
                json!({"terminal_reason":"budget_exhausted"}),
                StopReason::Error,
            ),
            (
                json!({"subtype":"error_during_execution"}),
                StopReason::Error,
            ),
            (
                json!({"subtype":"success","terminal_reason":null}),
                StopReason::EndTurn,
            ),
            (
                json!({"terminal_reason":"future_reason"}),
                StopReason::Other,
            ),
        ] {
            assert_eq!(result_reason(&frame, None), expected, "{frame}");
        }
    }
}
