//! Driver tests over `FakeSpawner` pipes: startup, every reply row, the main loop's phases,
//! permissions, cancellation, options and autonomy, and the exit paths.

#[path = "callback_lifecycle_tests.rs"]
mod callback_lifecycle;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ghost_process::testing::{FakeAgentEnd, FakeCall, FakeLine, FakeSpawner};
use ghost_process::{
    EnvMap, EnvSource, ExitInfo, LaunchSpec, LogFactory, ProcessCwd, Redactor, ResolvedEnv,
    ShutdownPlan, SpawnError, TopKeys,
};
use ghost_protocol::session::{ConfigKind, ConfigValue};
use ghost_protocol::{
    AutoAnswer, Autonomy, AutonomyState, ErrorKind, GhostEvent, NoticeLevel, PermissionOptionKind,
    PermissionRequest, PromptPart, StopReason, TokenUsage, ToolContent, ToolKind, UsageScope,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::*;
use crate::driver::{
    AgentSpawner, DriverTimings, EventReceiver, PermissionOutcome, ReadyInfo, TurnRef,
    event_channel,
};
use crate::testing::{DIRECT_LOG_CHAT, DirectSpawner};

const SESSION: &str = "sess-1";

fn launch() -> LaunchSpec {
    LaunchSpec {
        label: "fake acp".into(),
        program: PathBuf::from("/opt/bin/agent"),
        args: vec!["acp".into()],
        cwd: ProcessCwd::ChatFolder,
        set_env: Vec::new(),
        daemons: Vec::new(),
        startup_timeout: Duration::from_secs(60),
    }
}

fn setup(dialect: &str) -> SessionSetup {
    SessionSetup {
        mcp_servers: Vec::new(),
        attachment_dir: None,
        attachment_writer: None,
        chat_id: "chat".into(),
        agent_id: dialect.into(),
        dialect: dialect.into(),
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

fn init_result() -> Value {
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {"loadSession": true, "promptCapabilities": {"image": true},
                              "sessionCapabilities": {"resume": {}}},
        "authMethods": [],
        "agentInfo": {"name": "fake-agent", "title": "Fake Agent", "version": "1.2.3"},
    })
}

/// Spawns through `FakeSpawner` with a log that writes nothing, so tests leave no files behind (a
/// `DirectSpawner` writes one per spawn once protocol logs are written).
struct QuietSpawner(Arc<FakeSpawner>);

impl AgentSpawner for QuietSpawner {
    fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        ghost_process::ProcessSpawner::spawn(&*self.0, spec, ProtocolLog::disabled())
    }
}

struct Harness {
    /// The spawn id `Ready` reported.
    spawn_id: String,
    commands: Option<mpsc::UnboundedSender<DriverCommand>>,
    events: EventReceiver,
    run: JoinHandle<DriverExit>,
    end: FakeAgentEnd,
    exit: Option<oneshot::Sender<ExitInfo>>,
    spawner: Arc<FakeSpawner>,
}

impl Harness {
    async fn start(setup: SessionSetup) -> Harness {
        Harness::start_through(setup, |fake| Arc::new(QuietSpawner(fake))).await
    }

    /// `start`, spawning through what `spawner` makes of the fake spawner.
    async fn start_through(
        setup: SessionSetup,
        spawner: impl FnOnce(Arc<FakeSpawner>) -> Arc<dyn AgentSpawner>,
    ) -> Harness {
        let (fake, mut ends) = FakeSpawner::new();
        let (commands, receiver) = mpsc::unbounded_channel();
        let (sender, events) = event_channel();
        let ctx = DriverContext {
            commands: receiver,
            events: sender,
            spawner: spawner(Arc::clone(&fake)),
            setup,
        };
        let run = tokio::spawn(Box::new(AcpDriver::new()).run(ctx));
        let mut end = timeout(Duration::from_secs(5), ends.recv())
            .await
            .expect("the driver spawns")
            .expect("an agent end");
        let (stand_in, _) = oneshot::channel();
        let exit = std::mem::replace(&mut end.exit, stand_in);
        Harness {
            spawn_id: String::new(),
            commands: Some(commands),
            events,
            run,
            end,
            exit: Some(exit),
            spawner: fake,
        }
    }

    /// The next line the driver wrote.
    async fn line(&mut self) -> Value {
        let line = timeout(Duration::from_secs(5), self.end.from_client.recv())
            .await
            .expect("the driver writes a line in time")
            .expect("stdin is open");
        serde_json::from_str(&line).expect("the driver writes JSON")
    }

    /// The next line, which must be a request for `method`: its id and params.
    async fn request(&mut self, method: &str) -> (Value, Value) {
        let line = self.line().await;
        assert_eq!(line["method"], method, "{line}");
        assert!(line.get("id").is_some(), "a request: {line}");
        (line["id"].clone(), line["params"].clone())
    }

    /// The driver wrote nothing more (so far).
    fn no_line(&mut self) {
        if let Ok(line) = self.end.from_client.try_recv() {
            panic!("unexpected line from the driver: {line}");
        }
    }

    fn send(&self, value: Value) {
        self.end
            .to_client
            .send(FakeLine::Line(value.to_string()))
            .expect("stdout open");
    }

    fn raw(&self, line: &str) {
        self.end
            .to_client
            .send(FakeLine::Line(line.into()))
            .expect("stdout open");
    }

    fn reply(&self, id: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn reply_error(&self, id: &Value, code: i64, message: &str) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    }

    fn update(&self, update: Value) {
        self.update_for(SESSION, update);
    }

    fn update_for(&self, session: &str, update: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": "session/update",
                         "params": {"sessionId": session, "update": update}}));
    }

    fn agent_request(&self, id: Value, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    fn eof(&self) {
        let _ = self.end.to_client.send(FakeLine::Eof);
    }

    fn exit_with(&mut self, code: i32) {
        if let Some(exit) = self.exit.take() {
            let _ = exit.send(ExitInfo {
                code: Some(code),
                signal: None,
                at_ms: 0,
            });
        }
    }

    fn command(&self, command: DriverCommand) {
        self.commands
            .as_ref()
            .expect("commands open")
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
        timeout(Duration::from_secs(30), self.events.recv())
            .await
            .expect("an event in time")
            .expect("the event channel is open")
    }

    /// Events up to and including the first that `stop` matches.
    async fn events_until(&mut self, stop: impl Fn(&DriverEvent) -> bool) -> Vec<DriverEvent> {
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

    /// Every event until the driver returns, and how it returned.
    async fn finish(mut self) -> (Vec<DriverEvent>, DriverExit) {
        let mut seen = Vec::new();
        while let Ok(Some(event)) = timeout(Duration::from_secs(60), self.events.recv()).await {
            seen.push(event);
        }
        let exit = timeout(Duration::from_secs(60), self.run)
            .await
            .expect("the driver returns")
            .expect("the driver does not panic");
        (seen, exit)
    }

    fn calls(&self) -> Vec<FakeCall> {
        self.spawner.controls()[0].calls()
    }
}

/// Answers `initialize` with `init` and `session/new` with `new`, and returns every event up to
/// `Ready`.
async fn open_with(harness: &mut Harness, init: Value, new: Value) -> Vec<DriverEvent> {
    let (id, params) = harness.request("initialize").await;
    assert_eq!(params, requests::initialize("0.1.0"));
    harness.reply(&id, init);
    let (id, params) = harness.request("session/new").await;
    assert_eq!(params, json!({"cwd": "/work", "mcpServers": []}));
    harness.reply(&id, new);
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    harness.spawn_id = ready_info(&events).spawn_id.clone();
    events
}

/// A generic agent, opened with no options.
async fn ready_generic() -> Harness {
    let mut harness = Harness::start(setup("generic")).await;
    open_with(&mut harness, init_result(), json!({"sessionId": SESSION})).await;
    harness
}

fn ready_info(events: &[DriverEvent]) -> &ReadyInfo {
    events
        .iter()
        .find_map(|event| match event {
            DriverEvent::Ready(info) => Some(info),
            _ => None,
        })
        .expect("a Ready event")
}

fn transcript(events: &[DriverEvent]) -> Vec<(Option<String>, GhostEvent)> {
    events
        .iter()
        .filter_map(|event| match event {
            DriverEvent::Event { turn, event } => Some((turn.clone(), event.clone())),
            _ => None,
        })
        .collect()
}

fn turn_ended(
    events: &[DriverEvent],
) -> (String, StopReason, Option<TokenUsage>, Option<DriverError>) {
    events
        .iter()
        .find_map(|event| match event {
            DriverEvent::TurnEnded {
                turn_id,
                stop_reason,
                usage,
                error,
            } => Some((turn_id.clone(), *stop_reason, usage.clone(), error.clone())),
            _ => None,
        })
        .expect("a TurnEnded event")
}

fn is_turn_ended(event: &DriverEvent) -> bool {
    matches!(event, DriverEvent::TurnEnded { .. })
}

fn chunk(text: &str) -> Value {
    json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}})
}

fn permission_params(tool_call_id: &str, kind: &str) -> Value {
    json!({
        "sessionId": SESSION,
        "toolCall": {"toolCallId": tool_call_id, "title": "Run ls", "kind": kind, "status": "pending",
                     "rawInput": {"command": "ls"}},
        "options": [
            {"optionId": "allow_once", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "allow_always", "name": "Allow always", "kind": "allow_always"},
            {"optionId": "deny", "name": "Deny", "kind": "reject_once"},
        ],
    })
}

fn permission_of(event: &DriverEvent) -> Option<(Option<String>, PermissionRequest)> {
    match event {
        DriverEvent::Permission { turn, request } => Some((turn.clone(), request.clone())),
        _ => None,
    }
}

async fn next_permission(harness: &mut Harness) -> (Option<String>, PermissionRequest) {
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Permission { .. }))
        .await;
    events.last().and_then(permission_of).expect("a permission")
}

// ---- startup -----------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn startup_reports_each_status_then_ready_with_capabilities() {
    let mut harness = Harness::start(setup("generic")).await;
    let events = open_with(&mut harness, init_result(), json!({"sessionId": SESSION})).await;
    let statuses: Vec<DriverStatus> = events
        .iter()
        .filter_map(|event| match event {
            DriverEvent::Status(status) => Some(*status),
            _ => None,
        })
        .collect();
    assert_eq!(
        statuses,
        [
            DriverStatus::Spawning,
            DriverStatus::Initializing,
            DriverStatus::Opening,
            DriverStatus::Configuring,
            DriverStatus::Ready
        ]
    );
    let info = ready_info(&events);
    assert_eq!(info.native_session_id.as_deref(), Some(SESSION));
    assert_eq!(info.agent_name.as_deref(), Some("Fake Agent"));
    assert_eq!(info.agent_version.as_deref(), Some("1.2.3"));
    assert_eq!(info.resumed, ResumeOutcome::New);
    assert_eq!(info.replay, None);
    let spawn_id = uuid::Uuid::parse_str(&info.spawn_id).expect("the spawner's UUIDv7");
    assert_eq!(spawn_id.get_version_num(), 7);
    let capabilities = &info.capabilities;
    assert!(capabilities.images && capabilities.interrupt && capabilities.resume);
    assert!(
        capabilities.permissions
            && capabilities.plans
            && capabilities.usage
            && capabilities.slash_commands
    );
    assert!(
        !capabilities.steer && !capabilities.fork && !capabilities.rewind && !capabilities.limits
    );
    assert!(!capabilities.set_model && !capabilities.set_effort && !capabilities.add_dirs);
    assert_eq!(
        capabilities.autonomy_levels,
        [Autonomy::FullAuto, Autonomy::AutoEdit, Autonomy::Ask]
    );
    // The generic dialect changes no mode: Full auto answers the eight tool kinds.
    assert!(events.contains(&DriverEvent::Autonomy(AutonomyState {
        requested: Autonomy::FullAuto,
        native_mode: None,
        auto_answer: AutoAnswer::All,
        note: None,
    })));
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn a_failed_spawn_is_a_start_failure_by_kind() {
    for (error, kind) in [
        (
            SpawnError::ProgramNotFound {
                program: "hermes".into(),
            },
            ErrorKind::AgentUnavailable,
        ),
        (
            SpawnError::InvalidCwd {
                path: PathBuf::from("/gone"),
                reason: "missing".into(),
            },
            ErrorKind::FolderMissing,
        ),
        (
            SpawnError::CwdNotAllowed {
                path: PathBuf::from("/private"),
            },
            ErrorKind::FolderNotAllowed,
        ),
        (SpawnError::NoRuntime, ErrorKind::SpawnFailed),
    ] {
        let (spawner, _ends) = FakeSpawner::new();
        spawner.fail_next(error);
        let (_commands, receiver) = mpsc::unbounded_channel();
        let (sender, mut events) = event_channel();
        let exit = Box::new(AcpDriver::new())
            .run(DriverContext {
                commands: receiver,
                events: sender,
                spawner: Arc::new(QuietSpawner(spawner)),
                setup: setup("generic"),
            })
            .await;
        assert!(
            matches!(exit, DriverExit::StartFailed(DriverError { kind: k, .. }) if k == kind),
            "{exit:?}"
        );
        assert_eq!(
            events.recv().await,
            Some(DriverEvent::Status(DriverStatus::Spawning))
        );
    }
}

#[tokio::test(start_paused = true)]
async fn agent_exits_before_initialize() {
    let mut harness = Harness::start(setup("generic")).await;
    harness.request("initialize").await;
    harness
        .end
        .stderr
        .send("Traceback: something broke".into())
        .expect("stderr");
    tokio::time::sleep(Duration::from_millis(1)).await;
    harness.exit_with(3);
    harness.eof();
    let (events, exit) = harness.finish().await;
    let DriverExit::StartFailed(error) = exit else {
        panic!("expected StartFailed, got {exit:?}");
    };
    assert_eq!(error.kind, ErrorKind::AgentExited);
    assert_eq!(
        error.message,
        "The agent exited before it finished starting (exit code 3)."
    );
    assert_eq!(
        error.detail.as_deref(),
        Some("Traceback: something broke\n"),
        "the raw tail"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, DriverEvent::Ready(_)))
    );
}

#[tokio::test(start_paused = true)]
async fn hermes_breakaway_hint() {
    let mut harness = Harness::start(setup("hermes")).await;
    harness.request("initialize").await;
    harness
        .end
        .stderr
        .send("ERROR could not start the gateway: windows_breakaway_unavailable".into())
        .expect("stderr");
    tokio::time::sleep(Duration::from_millis(1)).await;
    harness.exit_with(1);
    harness.eof();
    let (_, exit) = harness.finish().await;
    let DriverExit::StartFailed(error) = exit else {
        panic!("expected StartFailed, got {exit:?}");
    };
    assert_eq!(error.kind, ErrorKind::AgentExited);
    assert!(
        error.message.ends_with(dialect::HERMES_BREAKAWAY_HINT),
        "{}",
        error.message
    );
}

#[tokio::test(start_paused = true)]
async fn initialize_timeout_stops_process() {
    let mut harness = Harness::start(setup("generic")).await;
    harness.request("initialize").await;
    // The agent never answers; the deadline is the launch's start timeout.
    let (_, exit) = harness.finish().await;
    let DriverExit::StartFailed(error) = exit else {
        panic!("expected StartFailed");
    };
    assert_eq!(error.kind, ErrorKind::StartTimeout);
    assert_eq!(error.message, "The agent did not answer within 60 seconds.");
}

#[tokio::test(start_paused = true)]
async fn initialize_timeout_runs_the_force_chain() {
    let mut harness = Harness::start(setup("generic")).await;
    harness.request("initialize").await;
    // 60 s to the deadline, then the force chain's 2 s before its kill.
    tokio::time::sleep(Duration::from_secs(65)).await;
    assert!(
        harness
            .calls()
            .contains(&FakeCall::Shutdown(ShutdownPlan::FORCE))
    );
    assert!(*harness.end.killed.borrow(), "the wedged agent was killed");
}

#[tokio::test(start_paused = true)]
async fn initialize_reply_rows() {
    // Protocol version other than 1: ProtocolMismatch, process stopped.
    let mut harness = Harness::start(setup("generic")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, json!({"protocolVersion": 2}));
    let (_, exit) = harness.finish().await;
    assert_eq!(
        exit,
        DriverExit::StartFailed(DriverError {
            kind: ErrorKind::ProtocolMismatch,
            message: "This agent speaks ACP protocol version 2; Ghost supports version 1.".into(),
            detail: None,
        })
    );

    // An error reply: AgentError with the agent's message.
    let mut harness = Harness::start(setup("generic")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply_error(&id, -32603, "Internal error: config broken");
    let (_, exit) = harness.finish().await;
    assert!(
        matches!(exit, DriverExit::StartFailed(DriverError { kind: ErrorKind::AgentError, ref message, .. })
        if message == "Internal error: config broken")
    );
}

#[tokio::test(start_paused = true)]
async fn session_new_reply_rows() {
    // -32000: AuthRequired with the terminal methods translated, then StartFailed.
    let mut harness = Harness::start(setup("hermes")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, json!({"protocolVersion": 1, "authMethods": [
        {"id": "hermes-setup", "name": "Configure Hermes provider", "type": "terminal", "args": ["--setup"]},
        {"id": "openrouter", "name": "openrouter runtime credentials"},
    ]}));
    let (id, _) = harness.request("session/new").await;
    harness.reply_error(&id, -32000, "Authentication required");
    let (events, exit) = harness.finish().await;
    let auth = events
        .iter()
        .find_map(|event| match event {
            DriverEvent::AuthRequired(prompt) => Some(prompt.clone()),
            _ => None,
        })
        .expect("AuthRequired");
    assert_eq!(
        auth.message,
        "Hermes needs you to sign in. Run the command below in a terminal, then press Retry."
    );
    assert_eq!(auth.commands.len(), 1);
    assert_eq!(auth.commands[0].argv, ["/opt/bin/agent", "acp", "--setup"]);
    assert!(matches!(
        exit,
        DriverExit::StartFailed(DriverError {
            kind: ErrorKind::AuthRequired,
            ..
        })
    ));

    // Another error: AgentError.
    let mut harness = Harness::start(setup("generic")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/new").await;
    harness.reply_error(&id, -32603, "cwd is not a directory");
    let (_, exit) = harness.finish().await;
    assert!(
        matches!(exit, DriverExit::StartFailed(DriverError { kind: ErrorKind::AgentError, ref message, .. })
        if message == "cwd is not a directory")
    );
}

#[tokio::test(start_paused = true)]
async fn codex_auth_falls_back_to_codex_login() {
    let mut codex = setup("codex-acp");
    codex.launch.set_env = vec![("CODEX_PATH".into(), "/opt/codex/bin/codex".into())];
    let mut harness = Harness::start(codex).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(
        &id,
        json!({"protocolVersion": 1, "authMethods": [{"id": "api-key", "name": "API Key"}]}),
    );
    let (id, _) = harness.request("session/new").await;
    harness.reply_error(&id, -32000, "Authentication required");
    let (events, _) = harness.finish().await;
    let auth = events
        .iter()
        .find_map(|event| match event {
            DriverEvent::AuthRequired(prompt) => Some(prompt.clone()),
            _ => None,
        })
        .expect("AuthRequired");
    assert_eq!(auth.commands[0].argv, ["/opt/codex/bin/codex", "login"]);
    assert!(auth.message.starts_with("Codex needs you to sign in."));
}

/// Opening with `resume` set: the variants of A23.
async fn open_resume(init: Value) -> (Harness, String, Value) {
    let mut resume = setup("generic");
    resume.resume = Some("old-1".into());
    let mut harness = Harness::start(resume).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init);
    let line = harness.line().await;
    let method = line["method"].as_str().expect("a method").to_owned();
    if method != "session/new" {
        assert_eq!(
            line["params"],
            json!({"sessionId": "old-1", "cwd": "/work", "mcpServers": []})
        );
    }
    let id = line["id"].clone();
    (harness, method, id)
}

#[tokio::test(start_paused = true)]
async fn open_resume_variants() {
    // Resume advertised: session/resume, Resumed.
    let (mut harness, method, id) = open_resume(init_result()).await;
    assert_eq!(method, "session/resume");
    harness.reply(&id, json!({}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    let info = ready_info(&events);
    assert_eq!(
        (info.native_session_id.as_deref(), &info.resumed),
        (Some("old-1"), &ResumeOutcome::Resumed)
    );

    // Load only: session/load, Loaded.
    let (mut harness, method, id) =
        open_resume(json!({"protocolVersion": 1, "agentCapabilities": {"loadSession": true}}))
            .await;
    assert_eq!(method, "session/load");
    harness.reply(&id, json!(null));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    assert_eq!(ready_info(&events).resumed, ResumeOutcome::Loaded);

    // Neither: session/new, Fresh, and the notice.
    let (mut harness, method, id) = open_resume(json!({"protocolVersion": 1})).await;
    assert_eq!(method, "session/new");
    harness.reply(&id, json!({"sessionId": "new-1"}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    assert_eq!(
        ready_info(&events).resumed,
        ResumeOutcome::Fresh {
            reason: CANNOT_CONTINUE.into()
        }
    );
    assert!(transcript(&events).iter().any(|(turn, event)| turn.is_none()
        && matches!(event, GhostEvent::Notice { message, .. } if message == &fresh_notice(CANNOT_CONTINUE))));

    // -32002, -32601, -32602: a new session, Fresh with the agent's message, and the notice.
    for code in [-32002, -32601, -32602] {
        let (mut harness, _, id) = open_resume(init_result()).await;
        harness.reply_error(&id, code, "Session not found");
        let (id, _) = harness.request("session/new").await;
        harness.reply(&id, json!({"sessionId": "new-2"}));
        let events = harness
            .events_until(|event| matches!(event, DriverEvent::Ready(_)))
            .await;
        let info = ready_info(&events);
        assert_eq!(info.native_session_id.as_deref(), Some("new-2"));
        assert_eq!(
            info.resumed,
            ResumeOutcome::Fresh {
                reason: "Session not found".into()
            }
        );
        assert!(transcript(&events).iter().any(|(_, event)|
            matches!(event, GhostEvent::Notice { message, .. } if message.starts_with("Could not continue the earlier session (Session not found)"))));
    }

    // Another error: StartFailed (the native id stays with the session).
    let (harness, _, id) = open_resume(init_result()).await;
    harness.reply_error(&id, -32603, "The gateway is restarting");
    let (_, exit) = harness.finish().await;
    assert!(matches!(
        exit,
        DriverExit::StartFailed(DriverError {
            kind: ErrorKind::AgentError,
            ..
        })
    ));

    // -32000 on resume: AuthRequired.
    let (harness, _, id) = open_resume(init_result()).await;
    harness.reply_error(&id, -32000, "Authentication required");
    let (events, exit) = harness.finish().await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DriverEvent::AuthRequired(_)))
    );
    assert!(matches!(
        exit,
        DriverExit::StartFailed(DriverError {
            kind: ErrorKind::AuthRequired,
            ..
        })
    ));

    // No reply: StartTimeout after the open deadline, the process stopped.
    let (harness, _, _) = open_resume(init_result()).await;
    let (_, exit) = harness.finish().await;
    assert!(matches!(
        exit,
        DriverExit::StartFailed(DriverError {
            kind: ErrorKind::StartTimeout,
            ..
        })
    ));
}

/// codex-acp answers the resume of a thread that has no rollout yet (Codex writes one at a
/// thread's first turn, so every session never prompted) with -32603 "Internal error" and the
/// reason in `data.details`, as the real-agent probe showed (spec 10.2 step 7). The codex-acp
/// dialect reads that as not found: a new session, Fresh, and the notice with Codex's reason.
#[tokio::test(start_paused = true)]
async fn codex_resume_of_a_thread_without_a_rollout_starts_a_new_session() {
    let no_rollout = |id: &Value, details: &str| {
        json!({"jsonrpc": "2.0", "id": id, "error": {
            "code": -32603, "message": "Internal error", "data": {"details": details}}})
    };
    let resume = |dialect: &str| {
        let mut setup = setup(dialect);
        setup.resume = Some("old-1".into());
        setup
    };
    let mut harness = Harness::start(resume("codex-acp")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/resume").await;
    harness.send(no_rollout(&id, "no rollout found for thread id old-1"));
    let (id, _) = harness.request("session/new").await;
    harness.reply(&id, json!({"sessionId": "new-1"}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    let info = ready_info(&events);
    assert_eq!(info.native_session_id.as_deref(), Some("new-1"));
    assert_eq!(
        info.resumed,
        ResumeOutcome::Fresh {
            reason: "no rollout found for thread id old-1".into()
        }
    );
    assert!(
        transcript(&events)
            .iter()
            .any(|(turn, event)| turn.is_none()
                && matches!(event, GhostEvent::Notice { message, .. }
            if message == &fresh_notice("no rollout found for thread id old-1")))
    );

    // Any other internal error from codex-acp still ends the start, the id kept for Retry.
    let mut harness = Harness::start(resume("codex-acp")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/resume").await;
    harness.send(no_rollout(&id, "the app-server exited"));
    let (_, exit) = harness.finish().await;
    assert!(matches!(
        exit,
        DriverExit::StartFailed(DriverError {
            kind: ErrorKind::AgentError,
            ..
        })
    ));

    // Another agent's -32603 is a failed start too, whatever its details say.
    let mut harness = Harness::start(resume("generic")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/resume").await;
    harness.send(no_rollout(&id, "no rollout found for thread id old-1"));
    let (_, exit) = harness.finish().await;
    assert!(matches!(
        exit,
        DriverExit::StartFailed(DriverError {
            kind: ErrorKind::AgentError,
            ..
        })
    ));
}

#[tokio::test(start_paused = true)]
async fn the_open_deadline_resets_on_every_update_up_to_the_ceiling() {
    let (mut harness, _, id) = open_resume(init_result()).await;
    // A replay update every 50 s keeps the open alive past the 60 s idle limit.
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_secs(50)).await;
        harness.update_for("old-1", chunk("replayed"));
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    harness.reply(&id, json!({}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    assert!(
        transcript(&events).is_empty(),
        "replayed transcript is dropped"
    );

    // But 10 minutes in all ends it, updates or not: the last update at 550 s would keep it
    // open until 610 s.
    let (harness, _, _) = open_resume(init_result()).await;
    let start = tokio::time::Instant::now();
    for _ in 0..11 {
        tokio::time::sleep(Duration::from_secs(50)).await;
        harness.update_for("old-1", chunk("replayed"));
    }
    let (_, exit) = harness.finish().await;
    assert!(matches!(
        exit,
        DriverExit::StartFailed(DriverError {
            kind: ErrorKind::StartTimeout,
            ..
        })
    ));
    let took = start.elapsed();
    assert!(
        took >= Duration::from_secs(600) && took < Duration::from_secs(610),
        "{took:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn hermes_replay_summary_counts_user_turns_and_keeps_the_last_reply() {
    let mut hermes = setup("hermes");
    hermes.resume = Some("h-1".into());
    let mut harness = Harness::start(hermes).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (open, _) = harness.request("session/resume").await;
    let user = |text: &str| json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": text}});
    harness.update_for("h-1", user("first"));
    harness.update_for("h-1", chunk("one"));
    harness.update_for("h-1", user("second"));
    harness.update_for(
        "h-1",
        json!({"sessionUpdate": "tool_call", "toolCallId": "t", "title": "Read"}),
    );
    harness.update_for("h-1", chunk("Done with "));
    harness.update_for("h-1", chunk("the fix."));
    harness.update_for(
        "h-1",
        json!({"sessionUpdate": "session_info_update", "title": "Fix it"}),
    );
    harness.reply(&open, json!({"modes": {"currentModeId": "dont_ask", "availableModes": [{"id": "dont_ask", "name": "Don't Ask"}]}}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    let summary = ready_info(&events).replay.clone().expect("a summary");
    assert_eq!(summary.user_turns, 2);
    assert_eq!(summary.last_reply.as_deref(), Some("Done with the fix."));
    assert_eq!(
        summary.last_reply_sha256,
        Some(map::hex(&Sha256::digest("Donewiththefix.".as_bytes())))
    );
    // State kinds are applied during the replay; transcript kinds are not.
    assert!(events.contains(&DriverEvent::Title(Some("Fix it".into()))));
    assert!(transcript(&events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_replayed_reply_with_no_text_but_whitespace_is_no_reply() {
    let user = |text: &str| json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": text}});
    for chunks in [vec![""], vec![" ", "\n\t", ""]] {
        let mut hermes = setup("hermes");
        hermes.resume = Some("h-1".into());
        let mut harness = Harness::start(hermes).await;
        let (id, _) = harness.request("initialize").await;
        harness.reply(&id, init_result());
        let (open, _) = harness.request("session/resume").await;
        harness.update_for("h-1", user("first"));
        harness.update_for("h-1", chunk("An answer."));
        harness.update_for("h-1", user("second"));
        for text in &chunks {
            harness.update_for("h-1", chunk(text));
        }
        harness.reply(&open, json!({}));
        let events = harness
            .events_until(|event| matches!(event, DriverEvent::Ready(_)))
            .await;
        // As the session hashes no reply for a turn without agent text, the summary carries
        // none: an empty hash would read as Hermes having continued the session.
        assert_eq!(
            ready_info(&events).replay,
            Some(ReplaySummary {
                user_turns: 2,
                last_reply: None,
                last_reply_sha256: None,
            }),
            "{chunks:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn updates_before_the_new_session_is_named_are_buffered() {
    let mut harness = Harness::start(setup("generic")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (open, _) = harness.request("session/new").await;
    harness.update(json!({"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "plan", "description": "Plan first"}]}));
    harness.update(chunk("Hello before the reply"));
    harness.update_for("someone-else", chunk("not ours"));
    harness.reply(&open, json!({"sessionId": SESSION}));
    let mut events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    // Rule 1: the transcript event waits for Ready.
    assert!(transcript(&events).is_empty());
    events.extend(
        harness
            .events_until(|event| matches!(event, DriverEvent::Commands(_)))
            .await,
    );
    let after_ready = transcript(&events);
    assert_eq!(after_ready.len(), 1);
    assert!(
        matches!(&after_ready[0], (None, GhostEvent::MessageDelta { text, .. }) if text == "Hello before the reply")
    );
}

#[tokio::test(start_paused = true)]
async fn foreign_session_update_ignored() {
    let mut harness = ready_generic().await;
    harness.update_for("another-session", chunk("not ours"));
    harness.update(json!({"sessionUpdate": "session_info_update", "title": "Ours"}));
    assert_eq!(
        harness.event().await,
        DriverEvent::Title(Some("Ours".into()))
    );
}

#[tokio::test(start_paused = true)]
async fn desired_option_not_offered() {
    let mut chosen = setup("generic");
    chosen.desired.insert(
        "model".into(),
        ConfigValue::Choice {
            value: "gpt-old".into(),
        },
    );
    chosen.desired.insert(
        "effort".into(),
        ConfigValue::Choice {
            value: "high".into(),
        },
    );
    chosen
        .desired
        .insert("gone".into(), ConfigValue::Toggle { value: true });
    let mut harness = Harness::start(chosen).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/new").await;
    harness.reply(&id, json!({"sessionId": SESSION, "configOptions": [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "gpt-new",
         "options": [{"value": "gpt-new", "name": "GPT New"}]},
        {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "low",
         "options": [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]},
    ]}));
    // The offered value is re-applied, one request at a time.
    let (id, params) = harness.request("session/set_config_option").await;
    assert_eq!(
        params,
        json!({"sessionId": SESSION, "configId": "effort", "value": "high"})
    );
    harness.reply(&id, json!({"configOptions": [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "gpt-new",
         "options": [{"value": "gpt-new", "name": "GPT New"}]},
        {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "high",
         "options": [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]},
    ]}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    let notices: Vec<String> = transcript(&events)
        .into_iter()
        .filter_map(|(_, event)| match event {
            GhostEvent::Notice { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(
        notices,
        [
            "The setting \"gone\" you chose earlier isn't offered anymore.",
            "The model you chose earlier (gpt-old) isn't offered anymore; using GPT New.",
        ]
    );
    let info = ready_info(&events);
    assert!(info.capabilities.set_model && info.capabilities.set_effort);
    harness.no_line();
}

// ---- autonomy ----------------------------------------------------------------------------------

fn claude_new_reply(modes: &[&str]) -> Value {
    let options: Vec<Value> = modes
        .iter()
        .map(|mode| json!({"value": mode, "name": mode}))
        .collect();
    json!({"sessionId": SESSION, "configOptions": [
        {"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "default", "options": options},
    ]})
}

fn autonomy_of(events: &[DriverEvent]) -> Vec<AutonomyState> {
    events
        .iter()
        .filter_map(|event| match event {
            DriverEvent::Autonomy(state) => Some(state.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn claude_full_auto_switches_to_bypass() {
    let mut harness = Harness::start(setup("claude-acp")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/new").await;
    harness.reply(
        &id,
        claude_new_reply(&["default", "acceptEdits", "bypassPermissions"]),
    );
    let (id, params) = harness.request("session/set_config_option").await;
    assert_eq!(
        params,
        json!({"sessionId": SESSION, "configId": "mode", "value": "bypassPermissions"})
    );
    harness.reply(&id, json!({"configOptions": [
        {"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "bypassPermissions",
         "options": [{"value": "default"}, {"value": "acceptEdits"}, {"value": "bypassPermissions"}]},
    ]}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    assert_eq!(
        autonomy_of(&events),
        [AutonomyState {
            requested: Autonomy::FullAuto,
            native_mode: Some("bypassPermissions".into()),
            auto_answer: AutoAnswer::Never,
            note: None
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn claude_without_bypass_runs_accept_edits_with_the_note() {
    // Not offered: skipped without asking the agent.
    let mut harness = Harness::start(setup("claude-acp")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/new").await;
    harness.reply(&id, claude_new_reply(&["default", "acceptEdits", "plan"]));
    let (id, params) = harness.request("session/set_config_option").await;
    assert_eq!(params["value"], "acceptEdits");
    harness.reply(&id, json!({"configOptions": [
        {"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "acceptEdits",
         "options": [{"value": "default"}, {"value": "acceptEdits"}, {"value": "plan"}]},
    ]}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    assert_eq!(
        autonomy_of(&events),
        [AutonomyState {
            requested: Autonomy::FullAuto,
            native_mode: Some("acceptEdits".into()),
            auto_answer: AutoAnswer::Never,
            note: Some(dialect::CLAUDE_NO_BYPASS_NOTE.into()),
        }]
    );

    // Offered but refused: the next candidate.
    let mut harness = Harness::start(setup("claude-acp")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/new").await;
    harness.reply(
        &id,
        claude_new_reply(&["default", "acceptEdits", "bypassPermissions"]),
    );
    let (id, params) = harness.request("session/set_config_option").await;
    assert_eq!(params["value"], "bypassPermissions");
    harness.reply_error(&id, -32602, "bypassPermissions is disabled by settings");
    let (id, params) = harness.request("session/set_config_option").await;
    assert_eq!(params["value"], "acceptEdits");
    harness.reply(&id, json!({}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    assert_eq!(
        autonomy_of(&events)[0].native_mode.as_deref(),
        Some("acceptEdits")
    );
}

#[tokio::test(start_paused = true)]
async fn no_matching_mode_falls_back_to_asking_with_a_note() {
    let mut harness = Harness::start(setup("codex-acp")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/new").await;
    harness.reply(&id, json!({"sessionId": SESSION}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    assert_eq!(
        autonomy_of(&events),
        [AutonomyState {
            requested: Autonomy::FullAuto,
            native_mode: None,
            auto_answer: AutoAnswer::Never,
            note: Some("Ghost could not switch Codex to a matching mode (it has no modes), so it asks before every action.".into()),
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn set_autonomy_replies_with_the_state_and_a_hand_picked_mode_turns_auto_answers_off() {
    let mut harness = Harness::start(setup("hermes")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/new").await;
    harness.reply(&id, json!({"sessionId": SESSION, "modes": {"currentModeId": "dont_ask", "availableModes": [
        {"id": "default", "name": "Default"}, {"id": "accept_edits", "name": "Accept Edits"}, {"id": "dont_ask", "name": "Don't Ask"}]}}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    // Already in dont_ask: no request, Kinds [execute].
    assert_eq!(
        autonomy_of(&events)[0].auto_answer,
        AutoAnswer::Kinds {
            kinds: vec![ToolKind::Execute]
        }
    );
    harness.no_line();

    // SetAutonomy(Ask): session/set_mode default, then the reply with the state.
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetAutonomy {
        autonomy: Autonomy::Ask,
        reply,
    });
    let (id, params) = harness.request("session/set_mode").await;
    assert_eq!(params, json!({"sessionId": SESSION, "modeId": "default"}));
    // The agent echoes the mode before replying: not a hand pick.
    harness.update(json!({"sessionUpdate": "current_mode_update", "currentModeId": "default"}));
    harness.reply(&id, json!({}));
    let state = timeout(Duration::from_secs(5), answer)
        .await
        .expect("answered")
        .expect("sent")
        .expect("ok");
    assert_eq!(
        state,
        AutonomyState {
            requested: Autonomy::Ask,
            native_mode: Some("default".into()),
            auto_answer: AutoAnswer::Never,
            note: Some(dialect::HERMES_ASK_NOTE.into())
        }
    );

    // The agent reports a mode the candidate did not pick: auto-answers off with the note.
    harness
        .update(json!({"sessionUpdate": "current_mode_update", "currentModeId": "accept_edits"}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Autonomy(_)))
        .await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DriverEvent::Options(_)))
    );
    assert_eq!(
        autonomy_of(&events),
        [AutonomyState {
            requested: Autonomy::Ask,
            native_mode: Some("accept_edits".into()),
            auto_answer: AutoAnswer::Never,
            note: Some("You picked the Accept Edits mode by hand, so Ghost asks before every action. Choose an autonomy level to hand control back.".into()),
        }]
    );
    // Back to the candidate's mode: restored, with the candidate's own note instead of the hand
    // pick's.
    harness.update(json!({"sessionUpdate": "current_mode_update", "currentModeId": "default"}));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Autonomy(_)))
        .await;
    assert_eq!(
        autonomy_of(&events)[0].note.as_deref(),
        Some(dialect::HERMES_ASK_NOTE)
    );
}

// ---- options -----------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn set_option_reply_rows() {
    let mut harness = Harness::start(setup("generic")).await;
    open_with(&mut harness, init_result(), json!({"sessionId": SESSION, "configOptions": [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "a",
         "options": [{"value": "a", "name": "A"}, {"value": "b", "name": "B"}]},
        {"id": "fast", "name": "Fast", "type": "boolean", "currentValue": false},
    ]})).await;
    // A value not offered: refused without contacting the agent.
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetOption {
        option_id: "model".into(),
        value: ConfigValue::Choice { value: "z".into() },
        reply,
    });
    let refused = answer.await.expect("answered").expect_err("refused");
    assert_eq!(refused.kind, ErrorKind::Other);
    harness.no_line();

    // Success: the reply's list replaces the options.
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetOption {
        option_id: "fast".into(),
        value: ConfigValue::Toggle { value: true },
        reply,
    });
    let (id, params) = harness.request("session/set_config_option").await;
    assert_eq!(
        params,
        json!({"sessionId": SESSION, "configId": "fast", "type": "boolean", "value": true})
    );
    harness.reply(
        &id,
        json!({"configOptions": [
            {"id": "fast", "name": "Fast", "type": "boolean", "currentValue": true},
        ]}),
    );
    assert_eq!(answer.await.expect("answered"), Ok(()));
    let options = match harness.event().await {
        DriverEvent::Options(options) => options,
        other => panic!("expected Options, got {other:?}"),
    };
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].kind, ConfigKind::Toggle { current: true });

    // -32602: the agent's message.
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetOption {
        option_id: "fast".into(),
        value: ConfigValue::Toggle { value: false },
        reply,
    });
    let (id, _) = harness.request("session/set_config_option").await;
    harness.reply_error(&id, -32602, "Fast mode is not available on this plan");
    let error = answer.await.expect("answered").expect_err("refused");
    assert_eq!(
        (error.kind, error.message.as_str()),
        (
            ErrorKind::AgentError,
            "Fast mode is not available on this plan"
        )
    );

    // No answer within 30 s.
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetOption {
        option_id: "fast".into(),
        value: ConfigValue::Toggle { value: false },
        reply,
    });
    harness.request("session/set_config_option").await;
    let error = answer.await.expect("answered").expect_err("timed out");
    assert!(error.message.contains("30 seconds"), "{}", error.message);
}

#[tokio::test(start_paused = true)]
async fn legacy_set_mode_and_set_model_rows() {
    let mut hermes = setup("hermes");
    hermes.autonomy = Autonomy::Ask;
    let mut harness = Harness::start(hermes).await;
    open_with(&mut harness, init_result(), json!({"sessionId": SESSION,
        "modes": {"currentModeId": "default", "availableModes": [{"id": "default", "name": "Default"}, {"id": "dont_ask", "name": "Don't Ask"}]},
        "models": {"currentModelId": "p:a", "availableModels": [{"modelId": "p:a", "name": "A"}, {"modelId": "p:b", "name": "B"}]}})).await;
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetOption {
        option_id: "model".into(),
        value: ConfigValue::Choice {
            value: "p:b".into(),
        },
        reply,
    });
    let (id, params) = harness.request("session/set_model").await;
    assert_eq!(params, json!({"sessionId": SESSION, "modelId": "p:b"}));
    harness.reply(&id, json!({}));
    assert_eq!(answer.await.expect("answered"), Ok(()));
    let DriverEvent::Options(options) = harness.event().await else {
        panic!("Options");
    };
    let model = options
        .iter()
        .find(|option| option.id == "model")
        .expect("model");
    assert!(matches!(&model.kind, ConfigKind::Select { current, .. } if current == "p:b"));

    // An error reply.
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetOption {
        option_id: "model".into(),
        value: ConfigValue::Choice {
            value: "p:a".into(),
        },
        reply,
    });
    let (id, _) = harness.request("session/set_model").await;
    harness.reply_error(&id, -32602, "No provider serves p:a");
    assert_eq!(
        answer
            .await
            .expect("answered")
            .expect_err("refused")
            .message,
        "No provider serves p:a"
    );

    // set_mode by hand: a hand pick.
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetOption {
        option_id: "mode".into(),
        value: ConfigValue::Choice {
            value: "dont_ask".into(),
        },
        reply,
    });
    let (id, params) = harness.request("session/set_mode").await;
    assert_eq!(params, json!({"sessionId": SESSION, "modeId": "dont_ask"}));
    harness.reply(&id, json!(null));
    assert_eq!(answer.await.expect("answered"), Ok(()));
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Autonomy(_)))
        .await;
    assert_eq!(
        autonomy_of(&events)[0].native_mode.as_deref(),
        Some("dont_ask")
    );
    assert_eq!(autonomy_of(&events)[0].auto_answer, AutoAnswer::Never);
}

#[tokio::test(start_paused = true)]
async fn empty_config_reply_ignored() {
    let mut harness = Harness::start(setup("hermes")).await;
    open_with(&mut harness, init_result(), json!({"sessionId": SESSION,
        "modes": {"currentModeId": "dont_ask", "availableModes": [{"id": "dont_ask", "name": "Don't Ask"}]}})).await;
    harness.update(
        json!({"sessionUpdate": "config_option_update", "configOptions": [
            {"id": "edit_approval_policy", "name": "Edits", "type": "select", "currentValue": "ask",
             "options": [{"value": "ask"}, {"value": "session"}]},
        ]}),
    );
    let DriverEvent::Options(options) = harness.event().await else {
        panic!("Options");
    };
    assert_eq!(options.len(), 2);
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetOption {
        option_id: "edit_approval_policy".into(),
        value: ConfigValue::Choice {
            value: "session".into(),
        },
        reply,
    });
    let (id, _) = harness.request("session/set_config_option").await;
    harness.reply(&id, json!({"configOptions": []}));
    assert_eq!(answer.await.expect("answered"), Ok(()));
    let DriverEvent::Options(options) = harness.event().await else {
        panic!("Options");
    };
    assert_eq!(options.len(), 2, "the empty list changed nothing");
    let policy = options
        .iter()
        .find(|option| option.id == "edit_approval_policy")
        .expect("kept");
    assert!(matches!(&policy.kind, ConfigKind::Select { current, .. } if current == "session"));
}

// ---- hand-picked modes and autonomy changes ----------------------------------------------------

/// Hermes' legacy modes with `current` in effect, and two models.
fn hermes_open(current: &str) -> Value {
    json!({"sessionId": SESSION,
        "modes": {"currentModeId": current, "availableModes": [
            {"id": "default", "name": "Default"}, {"id": "accept_edits", "name": "Accept Edits"},
            {"id": "dont_ask", "name": "Don't Ask"}]},
        "models": {"currentModelId": "p:a", "availableModels": [
            {"modelId": "p:a", "name": "A"}, {"modelId": "p:b", "name": "B"}]}})
}

fn execute_only() -> AutoAnswer {
    AutoAnswer::Kinds {
        kinds: vec![ToolKind::Execute],
    }
}

fn hand_picked_note(name: &str) -> Option<String> {
    Some(format!(
        "You picked the {name} mode by hand, so Ghost asks before every action. Choose an autonomy level to hand control back."
    ))
}

fn state(
    requested: Autonomy,
    native_mode: &str,
    auto_answer: AutoAnswer,
    note: Option<String>,
) -> AutonomyState {
    AutonomyState {
        requested,
        native_mode: Some(native_mode.into()),
        auto_answer,
        note,
    }
}

/// Hermes in Full auto, already in `dont_ask`, so the start changes no mode: Kinds [execute].
async fn ready_hermes_full_auto() -> Harness {
    let mut harness = Harness::start(setup("hermes")).await;
    let events = open_with(&mut harness, init_result(), hermes_open("dont_ask")).await;
    assert_eq!(
        autonomy_of(&events),
        [state(Autonomy::FullAuto, "dont_ask", execute_only(), None)]
    );
    harness.no_line();
    harness
}

fn set_option(
    harness: &Harness,
    option_id: &str,
    value: &str,
) -> oneshot::Receiver<Result<(), DriverError>> {
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetOption {
        option_id: option_id.into(),
        value: ConfigValue::Choice {
            value: value.into(),
        },
        reply,
    });
    answer
}

fn set_autonomy(
    harness: &Harness,
    autonomy: Autonomy,
) -> oneshot::Receiver<Result<AutonomyState, DriverError>> {
    let (reply, answer) = oneshot::channel();
    harness.command(DriverCommand::SetAutonomy { autonomy, reply });
    answer
}

/// The answer, which comes without the agent answering anything: within 1 ms of paused time.
async fn answered_now<T>(answer: oneshot::Receiver<T>) -> T {
    timeout(Duration::from_millis(1), answer)
        .await
        .expect("answered at once, without waiting for the agent")
        .expect("the driver answers")
}

/// Events up to the next `Autonomy`, and its state.
async fn next_autonomy(harness: &mut Harness) -> (Vec<DriverEvent>, AutonomyState) {
    let events = harness
        .events_until(|event| matches!(event, DriverEvent::Autonomy(_)))
        .await;
    let state = autonomy_of(&events).pop().expect("an Autonomy event");
    (events, state)
}

/// Every event up to a marker update sent now: what the driver emitted before it.
async fn events_before_marker(harness: &mut Harness, marker: &str) -> Vec<DriverEvent> {
    harness.update(json!({"sessionUpdate": "session_info_update", "title": marker}));
    let mut events = harness
        .events_until(|event| event == &DriverEvent::Title(Some(marker.into())))
        .await;
    events.pop();
    events
}

/// The Mode option's pending value in the last `Options` among `events`.
fn last_pending_mode(events: &[DriverEvent]) -> Option<Option<ConfigValue>> {
    events.iter().rev().find_map(|event| match event {
        DriverEvent::Options(options) => options
            .iter()
            .find(|option| option.id == "mode")
            .map(|option| option.pending.clone()),
        _ => None,
    })
}

fn notices(events: &[DriverEvent]) -> Vec<String> {
    transcript(events)
        .into_iter()
        .filter_map(|(_, event)| match event {
            GhostEvent::Notice { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_mode_picked_during_a_hermes_turn_turns_auto_answers_off_at_once() {
    let mut harness = ready_hermes_full_auto().await;
    harness.prompt("T", "work");
    let (prompt, _) = harness.request("session/prompt").await;
    assert_eq!(
        answered_now(set_option(&harness, "mode", "default")).await,
        Ok(())
    );
    // Before the turn ends: the change is pending with a notice, and Hermes' dangerous commands
    // are no longer answered for the user.
    let (events, picked) = next_autonomy(&mut harness).await;
    assert_eq!(
        last_pending_mode(&events),
        Some(Some(ConfigValue::Choice {
            value: "default".into()
        }))
    );
    assert!(transcript(&events).iter().any(|(turn, event)| turn.as_deref() == Some("T")
        && matches!(event, GhostEvent::Notice { message, .. } if message == "Hermes changes mode when the current turn ends.")));
    assert_eq!(
        picked,
        state(
            Autonomy::FullAuto,
            "default",
            AutoAnswer::Never,
            hand_picked_note("Default")
        )
    );
    assert!(!events.iter().any(is_turn_ended), "before TurnEnded");
    harness.no_line();

    // The change goes out after the turn; its success announces nothing new.
    harness.reply(&prompt, json!({"stopReason": "end_turn"}));
    let (id, params) = harness.request("session/set_mode").await;
    assert_eq!(params, json!({"sessionId": SESSION, "modeId": "default"}));
    harness.reply(&id, json!({}));
    let events = events_before_marker(&mut harness, "after").await;
    assert!(events.iter().any(is_turn_ended));
    assert_eq!(last_pending_mode(&events), Some(None));
    assert!(autonomy_of(&events).is_empty(), "{events:?}");
}

#[tokio::test(start_paused = true)]
async fn picking_the_candidates_mode_again_mid_turn_drops_the_parked_change_and_restores_auto_answers()
 {
    let mut harness = ready_hermes_full_auto().await;
    harness.prompt("T", "work");
    let (prompt, _) = harness.request("session/prompt").await;
    assert_eq!(
        answered_now(set_option(&harness, "mode", "default")).await,
        Ok(())
    );
    let (_, picked) = next_autonomy(&mut harness).await;
    assert_eq!(picked.auto_answer, AutoAnswer::Never);
    // Don't Ask again, still during the turn: nothing waits for the turn's end any more.
    assert_eq!(
        answered_now(set_option(&harness, "mode", "dont_ask")).await,
        Ok(())
    );
    let (events, restored) = next_autonomy(&mut harness).await;
    assert_eq!(
        restored,
        state(Autonomy::FullAuto, "dont_ask", execute_only(), None)
    );
    assert_eq!(last_pending_mode(&events), Some(None));
    assert!(notices(&events).is_empty(), "{events:?}");
    harness.reply(&prompt, json!({"stopReason": "end_turn"}));
    harness.events_until(is_turn_ended).await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn repicking_the_current_mode_mid_turn_parks_nothing() {
    let mut harness = ready_hermes_full_auto().await;
    harness.prompt("T", "work");
    let (prompt, _) = harness.request("session/prompt").await;
    assert_eq!(
        answered_now(set_option(&harness, "mode", "dont_ask")).await,
        Ok(())
    );
    let events = events_before_marker(&mut harness, "marker").await;
    assert!(
        notices(&events).is_empty()
            && autonomy_of(&events).is_empty()
            && last_pending_mode(&events).is_none(),
        "no notice, no pending change, no autonomy change: {events:?}"
    );
    harness.reply(&prompt, json!({"stopReason": "end_turn"}));
    harness.events_until(is_turn_ended).await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn a_parked_mode_change_that_fails_leaves_the_state_of_the_mode_hermes_kept() {
    let mut harness = ready_hermes_full_auto().await;
    harness.prompt("T", "work");
    let (prompt, _) = harness.request("session/prompt").await;
    assert_eq!(
        answered_now(set_option(&harness, "mode", "default")).await,
        Ok(())
    );
    next_autonomy(&mut harness).await;
    harness.reply(&prompt, json!({"stopReason": "end_turn"}));
    let (id, _) = harness.request("session/set_mode").await;
    harness.reply_error(&id, -32603, "Session is busy");
    // Hermes stayed in Don't Ask, so auto-answers follow it back.
    let (events, kept) = next_autonomy(&mut harness).await;
    assert!(notices(&events).contains(&"Hermes did not change mode (Session is busy).".into()));
    assert_eq!(last_pending_mode(&events), Some(None));
    assert_eq!(
        kept,
        state(Autonomy::FullAuto, "dont_ask", execute_only(), None)
    );
}

#[tokio::test(start_paused = true)]
async fn a_prompt_waits_for_the_changes_queued_before_it() {
    let mut harness = ready_hermes_full_auto().await;
    harness.prompt("T1", "one");
    let (first, _) = harness.request("session/prompt").await;
    assert_eq!(
        answered_now(set_option(&harness, "mode", "default")).await,
        Ok(())
    );
    // A model change is no mode change, so it goes out at once; Hermes keeps it waiting on its
    // guarded-model card.
    let model = set_option(&harness, "model", "p:b");
    let (set_model, _) = harness.request("session/set_model").await;
    harness.reply(&first, json!({"stopReason": "end_turn"}));
    harness.events_until(is_turn_ended).await;
    // The next prompt waits for both changes: sent now, it would run under the previous mode
    // (recorded with Hermes 0.22.5, which also refused the mode change that raced it).
    harness.prompt("T2", "two");
    tokio::time::sleep(Duration::from_millis(10)).await;
    harness.no_line();
    harness.reply(&set_model, json!({}));
    assert_eq!(model.await.expect("answered"), Ok(()));
    let (set_mode, params) = harness.request("session/set_mode").await;
    assert_eq!(params["modeId"], "default");
    harness.no_line();
    harness.reply(&set_mode, json!({}));
    let (_, params) = harness.request("session/prompt").await;
    assert_eq!(params["prompt"][0]["text"], "two");
}

#[tokio::test(start_paused = true)]
async fn an_interrupted_waiting_prompt_ends_cancelled_and_never_goes_out() {
    let mut harness = ready_hermes_full_auto().await;
    let mode = set_option(&harness, "mode", "default");
    let (set_mode, _) = harness.request("session/set_mode").await;
    harness.prompt("T", "work");
    harness.command(DriverCommand::Interrupt);
    let events = harness.events_until(is_turn_ended).await;
    assert!(events.iter().any(|event| matches!(
        event,
        DriverEvent::TurnEnded { turn_id, stop_reason: StopReason::Cancelled, error: None, .. }
            if turn_id == "T"
    )));
    harness.reply(&set_mode, json!({}));
    assert_eq!(mode.await.expect("answered"), Ok(()));
    tokio::time::sleep(Duration::from_millis(10)).await;
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn parked_changes_go_before_changes_queued_behind_them() {
    let mut harness = ready_hermes_full_auto().await;
    harness.prompt("T", "work");
    let (prompt, _) = harness.request("session/prompt").await;
    // First pick: parked for the turn's end.
    assert_eq!(
        answered_now(set_option(&harness, "mode", "accept_edits")).await,
        Ok(())
    );
    // A model change goes out and stays unanswered; a second pick queues behind it.
    let model = set_option(&harness, "model", "p:b");
    let (set_model, _) = harness.request("session/set_model").await;
    let later = set_option(&harness, "mode", "default");
    harness.reply(&prompt, json!({"stopReason": "end_turn"}));
    harness.events_until(is_turn_ended).await;
    harness.reply(&set_model, json!({}));
    assert_eq!(model.await.expect("answered"), Ok(()));
    // The parked pick came first, so it goes first, and the later pick has the last word.
    let (id, params) = harness.request("session/set_mode").await;
    assert_eq!(params["modeId"], "accept_edits");
    harness.reply(&id, json!({}));
    let (id, params) = harness.request("session/set_mode").await;
    assert_eq!(params["modeId"], "default");
    harness.reply(&id, json!({}));
    assert_eq!(later.await.expect("answered"), Ok(()));
}

#[tokio::test(start_paused = true)]
async fn a_hand_pick_turns_hermes_full_auto_off_and_the_candidates_mode_or_an_autonomy_level_restores_it()
 {
    let mut harness = ready_hermes_full_auto().await;
    // By hand, while idle: the mode changes, and auto-answers go off.
    let answer = set_option(&harness, "mode", "accept_edits");
    let (id, params) = harness.request("session/set_mode").await;
    assert_eq!(params["modeId"], "accept_edits");
    harness.reply(&id, json!({}));
    assert_eq!(answer.await.expect("answered"), Ok(()));
    let (_, picked) = next_autonomy(&mut harness).await;
    assert_eq!(
        picked,
        state(
            Autonomy::FullAuto,
            "accept_edits",
            AutoAnswer::Never,
            hand_picked_note("Accept Edits")
        )
    );
    // Don't Ask by hand: the candidate's mode, so Kinds [execute] again.
    let answer = set_option(&harness, "mode", "dont_ask");
    let (id, _) = harness.request("session/set_mode").await;
    harness.reply(&id, json!({}));
    assert_eq!(answer.await.expect("answered"), Ok(()));
    let (_, restored) = next_autonomy(&mut harness).await;
    assert_eq!(
        restored,
        state(Autonomy::FullAuto, "dont_ask", execute_only(), None)
    );
    // The agent reports another mode: off again. A SetAutonomy restores the candidate in its
    // reply, before the mode change even goes out.
    harness.update(json!({"sessionUpdate": "current_mode_update", "currentModeId": "default"}));
    let (_, reported) = next_autonomy(&mut harness).await;
    assert_eq!(reported.auto_answer, AutoAnswer::Never);
    let reply = set_autonomy(&harness, Autonomy::FullAuto);
    assert_eq!(
        answered_now(reply).await,
        Ok(state(Autonomy::FullAuto, "dont_ask", execute_only(), None))
    );
    let (id, params) = harness.request("session/set_mode").await;
    assert_eq!(params["modeId"], "dont_ask");
    // The agent's echo while the change is on its way is no hand pick.
    harness.update(json!({"sessionUpdate": "current_mode_update", "currentModeId": "dont_ask"}));
    harness.reply(&id, json!({}));
    let events = events_before_marker(&mut harness, "after").await;
    assert!(autonomy_of(&events).is_empty(), "{events:?}");
}

#[tokio::test(start_paused = true)]
async fn the_generic_dialect_turns_auto_answers_off_for_a_mode_picked_by_hand() {
    let generic_mode = |current: &str| {
        json!({"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": current,
               "options": [{"value": "default", "name": "Default"}, {"value": "plan", "name": "Plan Mode"}]})
    };
    let mut harness = Harness::start(setup("generic")).await;
    let events = open_with(
        &mut harness,
        init_result(),
        json!({"sessionId": SESSION, "configOptions": [generic_mode("default")]}),
    )
    .await;
    // Full auto changes no mode and answers the eight tool kinds.
    assert_eq!(
        autonomy_of(&events),
        [state(Autonomy::FullAuto, "default", AutoAnswer::All, None)]
    );
    // A mode picked by hand: auto-answers off.
    let answer = set_option(&harness, "mode", "plan");
    let (id, params) = harness.request("session/set_config_option").await;
    assert_eq!(params["value"], "plan");
    harness.reply(&id, json!({"configOptions": [generic_mode("plan")]}));
    assert_eq!(answer.await.expect("answered"), Ok(()));
    let (_, picked) = next_autonomy(&mut harness).await;
    assert_eq!(
        picked,
        state(
            Autonomy::FullAuto,
            "plan",
            AutoAnswer::Never,
            hand_picked_note("Plan")
        )
    );
    // Back in the mode autonomy found: All again.
    harness.update(json!({"sessionUpdate": "current_mode_update", "currentModeId": "default"}));
    let (_, restored) = next_autonomy(&mut harness).await;
    assert_eq!(
        restored,
        state(Autonomy::FullAuto, "default", AutoAnswer::All, None)
    );
    // A mode the agent reports counts too; a SetAutonomy restores All, measured from that mode.
    harness.update(json!({"sessionUpdate": "current_mode_update", "currentModeId": "plan"}));
    let (_, reported) = next_autonomy(&mut harness).await;
    assert_eq!(reported.auto_answer, AutoAnswer::Never);
    assert_eq!(
        answered_now(set_autonomy(&harness, Autonomy::FullAuto)).await,
        Ok(state(Autonomy::FullAuto, "plan", AutoAnswer::All, None))
    );
    let events = events_before_marker(&mut harness, "settled").await;
    assert!(autonomy_of(&events).is_empty(), "{events:?}");
    harness.update(json!({"sessionUpdate": "current_mode_update", "currentModeId": "default"}));
    let (_, moved) = next_autonomy(&mut harness).await;
    assert_eq!(
        moved,
        state(
            Autonomy::FullAuto,
            "default",
            AutoAnswer::Never,
            hand_picked_note("Default")
        )
    );
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn set_autonomy_takes_effect_at_once_and_its_mode_change_follows_in_order() {
    let mut harness = ready_hermes_full_auto().await;
    // A model change is on its way and unanswered.
    let model = set_option(&harness, "model", "p:b");
    let (set_model, _) = harness.request("session/set_model").await;
    // Lowering to Ask: the policy changes now, in the reply; the mode change waits its turn.
    assert_eq!(
        answered_now(set_autonomy(&harness, Autonomy::Ask)).await,
        Ok(state(
            Autonomy::Ask,
            "default",
            AutoAnswer::Never,
            Some(dialect::HERMES_ASK_NOTE.into())
        ))
    );
    harness.no_line();
    harness.reply(&set_model, json!({}));
    assert_eq!(model.await.expect("answered"), Ok(()));
    let (id, params) = harness.request("session/set_mode").await;
    assert_eq!(params["modeId"], "default");
    harness.reply(&id, json!({}));
    let events = events_before_marker(&mut harness, "after").await;
    assert!(autonomy_of(&events).is_empty(), "the reply said it all");
}

#[tokio::test(start_paused = true)]
async fn a_set_autonomy_fallback_is_announced_as_soon_as_it_is_chosen() {
    let mut ask = setup("claude-acp");
    ask.autonomy = Autonomy::Ask;
    let mut harness = Harness::start(ask).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/new").await;
    harness.reply(
        &id,
        claude_new_reply(&["default", "acceptEdits", "bypassPermissions"]),
    );
    harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    // Full auto: bypass at once, in the reply.
    assert_eq!(
        answered_now(set_autonomy(&harness, Autonomy::FullAuto)).await,
        Ok(state(
            Autonomy::FullAuto,
            "bypassPermissions",
            AutoAnswer::Never,
            None
        ))
    );
    let (id, params) = harness.request("session/set_config_option").await;
    assert_eq!(params["value"], "bypassPermissions");
    harness.reply_error(&id, -32602, "bypassPermissions is disabled by settings");
    // The fallback takes effect as soon as it is chosen, announced since the reply went out.
    let (_, fallback) = next_autonomy(&mut harness).await;
    assert_eq!(
        fallback,
        state(
            Autonomy::FullAuto,
            "acceptEdits",
            AutoAnswer::Never,
            Some(dialect::CLAUDE_NO_BYPASS_NOTE.into())
        )
    );
    let (id, params) = harness.request("session/set_config_option").await;
    assert_eq!(params["value"], "acceptEdits");
    // That one is refused too: the last resort.
    harness.reply_error(&id, -32602, "no such mode");
    let (_, last) = next_autonomy(&mut harness).await;
    assert_eq!(
        last,
        state(
            Autonomy::FullAuto,
            "default",
            AutoAnswer::Never,
            Some("Ghost could not switch Claude Code to a matching mode (no such mode), so it asks before every action.".into())
        )
    );
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn a_later_set_autonomy_replaces_one_still_on_its_way() {
    let mut ask = setup("claude-acp");
    ask.autonomy = Autonomy::Ask;
    let mut harness = Harness::start(ask).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/new").await;
    harness.reply(
        &id,
        claude_new_reply(&["default", "acceptEdits", "bypassPermissions"]),
    );
    harness
        .events_until(|event| matches!(event, DriverEvent::Ready(_)))
        .await;
    assert!(
        answered_now(set_autonomy(&harness, Autonomy::FullAuto))
            .await
            .is_ok()
    );
    let (bypass, _) = harness.request("session/set_config_option").await;
    // The user changes their mind before the agent answers: the new level applies at once.
    assert_eq!(
        answered_now(set_autonomy(&harness, Autonomy::AutoEdit)).await,
        Ok(state(
            Autonomy::AutoEdit,
            "acceptEdits",
            AutoAnswer::Never,
            None
        ))
    );
    // The replaced change is refused: no fallback of it, no note about bypass.
    harness.reply_error(&bypass, -32602, "bypassPermissions is disabled by settings");
    let (id, params) = harness.request("session/set_config_option").await;
    assert_eq!(params["value"], "acceptEdits");
    harness.reply(&id, json!({}));
    let events = events_before_marker(&mut harness, "after").await;
    assert!(autonomy_of(&events).is_empty(), "{events:?}");
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn a_set_autonomy_nobody_waits_for_is_announced() {
    let mut harness = ready_hermes_full_auto().await;
    // The session gave up on the reply (its IPC budget ran out): the state still reaches it.
    let (reply, answer) = oneshot::channel();
    drop(answer);
    harness.command(DriverCommand::SetAutonomy {
        autonomy: Autonomy::Ask,
        reply,
    });
    let (_, announced) = next_autonomy(&mut harness).await;
    assert_eq!(
        announced,
        state(
            Autonomy::Ask,
            "default",
            AutoAnswer::Never,
            Some(dialect::HERMES_ASK_NOTE.into())
        )
    );
}

#[tokio::test(start_paused = true)]
async fn a_set_option_is_answered_within_the_request_timeout_of_its_arrival() {
    let mut harness = Harness::start(setup("generic")).await;
    open_with(
        &mut harness,
        init_result(),
        json!({"sessionId": SESSION, "configOptions": [
            model_option("a"),
            {"id": "fast", "name": "Fast", "type": "boolean", "currentValue": false},
        ]}),
    )
    .await;
    let start = tokio::time::Instant::now();
    // The first change goes out and is never answered; the second arrives 10 s later.
    let first = set_option(&harness, "model", "b");
    harness.request("session/set_config_option").await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    let (reply, second) = oneshot::channel();
    harness.command(DriverCommand::SetOption {
        option_id: "fast".into(),
        value: ConfigValue::Toggle { value: true },
        reply,
    });
    assert!(first.await.expect("answered").is_err());
    assert_eq!(start.elapsed(), Duration::from_secs(30));
    // The second goes out with what is left of its 30 s, and times out 30 s after it arrived.
    harness.request("session/set_config_option").await;
    assert!(second.await.expect("answered").is_err());
    assert_eq!(start.elapsed(), Duration::from_secs(40));

    // A change that waited its whole 30 s behind another is answered without being sent.
    let first = set_option(&harness, "model", "b");
    harness.request("session/set_config_option").await;
    let second = set_option(&harness, "model", "b");
    assert!(first.await.expect("answered").is_err());
    assert_eq!(
        second.await.expect("answered"),
        Err(DriverError {
            kind: ErrorKind::AgentError,
            message: OPTION_TOO_LATE.into(),
            detail: None,
        })
    );
    tokio::time::sleep(Duration::from_millis(10)).await;
    harness.no_line();
}

// ---- turns -------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_turn_tags_its_events_and_ends_once() {
    let mut harness = ready_generic().await;
    harness.prompt("T1", "Fix the build");
    let (id, params) = harness.request("session/prompt").await;
    assert_eq!(
        params,
        json!({"sessionId": SESSION, "prompt": [{"type": "text", "text": "Fix the build"}]})
    );
    harness.update(chunk("On it."));
    harness.update(json!({"sessionUpdate": "usage_update", "used": 10, "size": 100}));
    harness.reply(&id, json!({"stopReason": "end_turn", "usage": {"inputTokens": 5, "outputTokens": 3, "totalTokens": 8}}));
    harness.update(chunk("after the reply"));
    let mut events = harness.events_until(is_turn_ended).await;
    events.extend(
        harness
            .events_until(|event| matches!(event, DriverEvent::Event { turn: None, .. }))
            .await,
    );
    let (turn_id, stop_reason, usage, error) = turn_ended(&events);
    assert_eq!(
        (turn_id.as_str(), stop_reason, error),
        ("T1", StopReason::EndTurn, None)
    );
    // The generic dialect reports session totals: the first turn of a spawn says so.
    assert_eq!(usage.expect("usage").scope, UsageScope::Session);
    let tags: Vec<Option<String>> = transcript(&events)
        .into_iter()
        .map(|(turn, _)| turn)
        .collect();
    assert_eq!(
        tags,
        [
            Some("T1".into()),
            Some("T1".into()),
            Some("T1".into()),
            None
        ]
    );
    let ended_at = events.iter().position(is_turn_ended).expect("ended");
    assert!(
        matches!(&events[ended_at - 1], DriverEvent::Event { event: GhostEvent::MessageCompleted { message_id }, .. } if message_id == "T1.m1")
    );
}

#[tokio::test(start_paused = true)]
async fn prompt_reply_rows() {
    let mut harness = ready_generic().await;
    // Every known stop reason maps by name; another one is Other.
    for (index, (wire, reason)) in [
        ("end_turn", StopReason::EndTurn),
        ("max_tokens", StopReason::MaxTokens),
        ("max_turn_requests", StopReason::MaxTurnRequests),
        ("refusal", StopReason::Refusal),
        ("cancelled", StopReason::Cancelled),
        ("out_of_budget", StopReason::Other),
    ]
    .into_iter()
    .enumerate()
    {
        let turn = format!("T{index}");
        harness.prompt(&turn, "go");
        let (id, _) = harness.request("session/prompt").await;
        harness.reply(&id, json!({"stopReason": wire}));
        let events = harness.events_until(is_turn_ended).await;
        assert_eq!(turn_ended(&events).1, reason, "{wire}");
    }
    // -32800: Cancelled.
    harness.prompt("C", "go");
    let (id, _) = harness.request("session/prompt").await;
    harness.reply_error(&id, -32800, "Request cancelled");
    let events = harness.events_until(is_turn_ended).await;
    assert_eq!(
        (turn_ended(&events).1, turn_ended(&events).3),
        (StopReason::Cancelled, None)
    );
    // Another error: Error with the agent's message; the session stays ready.
    harness.prompt("E", "go");
    let (id, _) = harness.request("session/prompt").await;
    harness.reply_error(&id, -32603, "Rate limited: try again in 30 s");
    let events = harness.events_until(is_turn_ended).await;
    let (_, reason, _, error) = turn_ended(&events);
    assert_eq!(reason, StopReason::Error);
    assert_eq!(
        error,
        Some(DriverError {
            kind: ErrorKind::AgentError,
            message: "Rate limited: try again in 30 s".into(),
            detail: None
        })
    );
    assert_eq!(
        harness.event().await,
        DriverEvent::Status(DriverStatus::Ready)
    );
    // -32000: AuthRequired first, then TurnEnded with AuthRequired.
    harness.prompt("A", "go");
    let (id, _) = harness.request("session/prompt").await;
    harness.reply_error(&id, -32000, "Authentication required");
    let events = harness.events_until(is_turn_ended).await;
    let auth_at = events
        .iter()
        .position(|event| matches!(event, DriverEvent::AuthRequired(_)))
        .expect("AuthRequired");
    assert!(auth_at < events.iter().position(is_turn_ended).expect("ended"));
    assert_eq!(
        turn_ended(&events).3.map(|error| error.kind),
        Some(ErrorKind::AuthRequired)
    );
}

#[tokio::test(start_paused = true)]
async fn usage_scopes_per_dialect() {
    // claude-acp reports per turn.
    let mut harness = Harness::start(setup("claude-acp")).await;
    open_with(&mut harness, init_result(), json!({"sessionId": SESSION})).await;
    harness.prompt("T", "go");
    let (id, _) = harness.request("session/prompt").await;
    harness.reply(&id, json!({"stopReason": "end_turn", "usage": {"inputTokens": 7, "outputTokens": 2, "cachedReadTokens": 4, "cachedWriteTokens": 1, "thoughtTokens": 3}}));
    let events = harness.events_until(is_turn_ended).await;
    assert_eq!(
        turn_ended(&events).2,
        Some(TokenUsage {
            input_tokens: 7,
            output_tokens: 2,
            cache_read_tokens: Some(4),
            cache_write_tokens: Some(1),
            reasoning_tokens: Some(3),
            scope: UsageScope::Turn
        })
    );

    // Hermes reports session totals: the first turn says so, later ones are differences.
    let mut harness = Harness::start(setup("hermes")).await;
    open_with(&mut harness, init_result(), json!({"sessionId": SESSION})).await;
    for (turn, input, output, expected) in [
        (
            "T1",
            100,
            10,
            TokenUsage {
                input_tokens: 100,
                output_tokens: 10,
                cache_read_tokens: Some(50),
                cache_write_tokens: None,
                reasoning_tokens: None,
                scope: UsageScope::Session,
            },
        ),
        (
            "T2",
            160,
            25,
            TokenUsage {
                input_tokens: 60,
                output_tokens: 15,
                cache_read_tokens: Some(30),
                cache_write_tokens: None,
                reasoning_tokens: None,
                scope: UsageScope::Turn,
            },
        ),
    ] {
        harness.prompt(turn, "go");
        let (id, _) = harness.request("session/prompt").await;
        let cached = if turn == "T1" { 50 } else { 80 };
        harness.reply(&id, json!({"stopReason": "end_turn", "usage": {"inputTokens": input, "outputTokens": output, "totalTokens": input + output, "cachedReadTokens": cached}}));
        let events = harness.events_until(is_turn_ended).await;
        assert_eq!(turn_ended(&events).2, Some(expected), "{turn}");
    }
}

#[tokio::test(start_paused = true)]
async fn image_parts_end_the_turn_at_once() {
    let mut harness = ready_generic().await;
    harness.command(DriverCommand::Prompt {
        turn: TurnRef {
            turn_id: "T".into(),
        },
        parts: vec![PromptPart::Image {
            blob: "ab".into(),
            mime: "image/png".into(),
        }],
    });
    let events = harness.events_until(is_turn_ended).await;
    let (_, reason, _, error) = turn_ended(&events);
    assert_eq!(
        (reason, error.map(|error| error.kind)),
        (StopReason::Error, Some(ErrorKind::Internal))
    );
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn end_turn_after_cancel() {
    let mut harness = ready_generic().await;
    harness.prompt("T", "count to 300");
    let (id, _) = harness.request("session/prompt").await;
    harness.command(DriverCommand::Interrupt);
    let cancel = harness.line().await;
    assert_eq!(
        cancel,
        json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": SESSION}})
    );
    // Idempotent: a second interrupt sends nothing.
    harness.command(DriverCommand::Interrupt);
    // Updates after the cancel are mapped as usual.
    harness.update(chunk("299, 300"));
    harness.reply(&id, json!({"stopReason": "end_turn"}));
    let events = harness.events_until(is_turn_ended).await;
    assert!(events.contains(&DriverEvent::Status(DriverStatus::Cancelling)));
    assert_eq!(
        turn_ended(&events).1,
        StopReason::EndTurn,
        "shown as the truth"
    );
    assert_eq!(transcript(&events).len(), 2, "the delta and its completion");
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn an_error_after_a_cancel_counts_as_cancelled() {
    let mut harness = ready_generic().await;
    harness.prompt("T", "go");
    let (id, _) = harness.request("session/prompt").await;
    harness.command(DriverCommand::Interrupt);
    harness.request_or_notification("session/cancel").await;
    harness.reply_error(&id, -32603, "aborted");
    let events = harness.events_until(is_turn_ended).await;
    assert_eq!(
        (turn_ended(&events).1, turn_ended(&events).3),
        (StopReason::Cancelled, None)
    );
}

impl Harness {
    async fn request_or_notification(&mut self, method: &str) -> Value {
        let line = self.line().await;
        assert_eq!(line["method"], method, "{line}");
        line
    }
}

#[tokio::test(start_paused = true)]
async fn interrupt_without_a_prompt_does_nothing() {
    let mut harness = ready_generic().await;
    harness.command(DriverCommand::Interrupt);
    tokio::time::sleep(Duration::from_secs(1)).await;
    harness.no_line();
}

// ---- permissions -------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn permission_requests_are_forwarded_tagged_and_answered() {
    let mut harness = ready_generic().await;
    harness.prompt("T", "list files");
    let (prompt, _) = harness.request("session/prompt").await;
    harness.update(json!({"sessionUpdate": "tool_call", "toolCallId": "call_1", "title": "Run ls", "kind": "execute"}));
    harness.agent_request(
        json!(41),
        "session/request_permission",
        permission_params("call_1", "execute"),
    );
    let (turn, request) = next_permission(&mut harness).await;
    assert_eq!(turn.as_deref(), Some("T"));
    assert_eq!(request.request_id, "T.p1");
    assert_eq!(
        request.tool_call_id.as_deref(),
        Some("T.call_1"),
        "it names the block shown"
    );
    assert_eq!(request.kind, Some(ToolKind::Execute));
    assert_eq!(request.raw_input, Some(json!({"command": "ls"})));
    assert_eq!(request.options.len(), 3);
    harness.command(DriverCommand::RespondPermission {
        request_id: "T.p1".into(),
        outcome: PermissionOutcome::Selected("allow_once".into()),
    });
    assert_eq!(
        harness.line().await,
        json!({"jsonrpc": "2.0", "id": 41, "result": {"outcome": {"outcome": "selected", "optionId": "allow_once"}}})
    );
    // An unknown or already answered request id is ignored.
    harness.command(DriverCommand::RespondPermission {
        request_id: "T.p1".into(),
        outcome: PermissionOutcome::Cancelled,
    });
    harness.reply(&prompt, json!({"stopReason": "end_turn"}));
    harness.events_until(is_turn_ended).await;
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn unknown_option_kind() {
    let mut harness = ready_generic().await;
    let mut params = permission_params("perm-check-1", "execute");
    params["options"] = json!([{"optionId": "allow_session", "name": "Allow for session", "kind": "allow_for_session"}]);
    harness.agent_request(json!("q-1"), "session/request_permission", params);
    let (turn, request) = next_permission(&mut harness).await;
    assert_eq!(turn, None, "outside a turn: untagged");
    assert_eq!(request.options[0].kind, PermissionOptionKind::Other);
    assert_eq!(
        request.tool_call_id, None,
        "a permission's own tool call names no block"
    );
    assert_eq!(request.request_id, format!("{}.p1", harness.spawn_id));
}

#[tokio::test(start_paused = true)]
async fn permission_without_options() {
    let mut harness = ready_generic().await;
    harness.agent_request(
        json!(7),
        "session/request_permission",
        json!({"sessionId": SESSION, "toolCall": {"toolCallId": "x"}, "options": []}),
    );
    let (_, request) = next_permission(&mut harness).await;
    assert!(request.options.is_empty());
    assert_eq!(request.title, UNTITLED_PERMISSION);
    harness.command(DriverCommand::RespondPermission {
        request_id: request.request_id,
        outcome: PermissionOutcome::Cancelled,
    });
    assert_eq!(
        harness.line().await,
        json!({"jsonrpc": "2.0", "id": 7, "result": {"outcome": {"outcome": "cancelled"}}})
    );
}

#[tokio::test(start_paused = true)]
async fn concurrent_permissions() {
    let mut harness = ready_generic().await;
    harness.prompt("T", "go");
    let (prompt, _) = harness.request("session/prompt").await;
    harness.agent_request(
        json!(1),
        "session/request_permission",
        permission_params("a", "edit"),
    );
    harness.agent_request(
        json!(2),
        "session/request_permission",
        permission_params("b", "execute"),
    );
    let (_, first) = next_permission(&mut harness).await;
    let (_, second) = next_permission(&mut harness).await;
    assert_eq!(
        (first.request_id.as_str(), second.request_id.as_str()),
        ("T.p1", "T.p2")
    );
    // Answered independently, in any order.
    harness.command(DriverCommand::RespondPermission {
        request_id: "T.p2".into(),
        outcome: PermissionOutcome::Selected("deny".into()),
    });
    assert_eq!(harness.line().await["id"], 2);
    // The turn's end answers what is still pending with `cancelled`.
    harness.reply(&prompt, json!({"stopReason": "end_turn"}));
    assert_eq!(
        harness.line().await,
        json!({"jsonrpc": "2.0", "id": 1, "result": {"outcome": {"outcome": "cancelled"}}})
    );
    harness.events_until(is_turn_ended).await;
}

#[tokio::test(start_paused = true)]
async fn late_permission_after_cancel() {
    let mut harness = ready_generic().await;
    harness.prompt("T", "go");
    let (prompt, _) = harness.request("session/prompt").await;
    harness.agent_request(
        json!(1),
        "session/request_permission",
        permission_params("a", "edit"),
    );
    let (_, open) = next_permission(&mut harness).await;
    harness.command(DriverCommand::Interrupt);
    assert_eq!(harness.line().await["method"], "session/cancel");
    // The open request is answered `cancelled` at the interrupt.
    assert_eq!(
        harness.line().await,
        json!({"jsonrpc": "2.0", "id": 1, "result": {"outcome": {"outcome": "cancelled"}}})
    );
    // A request after the cancel is answered `cancelled` at once and never forwarded.
    harness.agent_request(
        json!(2),
        "session/request_permission",
        permission_params("b", "edit"),
    );
    assert_eq!(
        harness.line().await,
        json!({"jsonrpc": "2.0", "id": 2, "result": {"outcome": {"outcome": "cancelled"}}})
    );
    harness.reply(&prompt, json!({"stopReason": "cancelled"}));
    let events = harness.events_until(is_turn_ended).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, DriverEvent::Permission { .. }))
    );
    assert_eq!(turn_ended(&events).1, StopReason::Cancelled);
    // The user's late answer to the cancelled card goes nowhere.
    harness.command(DriverCommand::RespondPermission {
        request_id: open.request_id,
        outcome: PermissionOutcome::Selected("allow_once".into()),
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn withdrawn_permissions_are_answered_and_reported() {
    let mut harness = ready_generic().await;
    harness.agent_request(
        json!("bg-1"),
        "session/request_permission",
        permission_params("a", "execute"),
    );
    let (_, request) = next_permission(&mut harness).await;
    harness.send(
        json!({"jsonrpc": "2.0", "method": "$/cancel_request", "params": {"requestId": "bg-1"}}),
    );
    assert_eq!(
        harness.event().await,
        DriverEvent::PermissionWithdrawn {
            request_id: request.request_id.clone()
        }
    );
    assert_eq!(
        harness.line().await,
        json!({"jsonrpc": "2.0", "id": "bg-1", "result": {"outcome": {"outcome": "cancelled"}}})
    );
    // Other ids are ignored.
    harness
        .send(json!({"jsonrpc": "2.0", "method": "$/cancel_request", "params": {"requestId": 99}}));
    tokio::time::sleep(Duration::from_millis(10)).await;
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn malformed_permission_params_are_answered_invalid() {
    let mut harness = ready_generic().await;
    harness.agent_request(
        json!(5),
        "session/request_permission",
        json!({"sessionId": SESSION, "options": []}),
    );
    let reply = harness.line().await;
    assert_eq!(reply["id"], 5);
    assert_eq!(reply["error"]["code"], -32602);
}

#[tokio::test(start_paused = true)]
async fn agent_requests_auracode_does_not_offer_get_method_not_found() {
    let mut harness = ready_generic().await;
    for (id, method) in [
        (1, "fs/read_text_file"),
        (2, "fs/write_text_file"),
        (3, "terminal/create"),
        (4, "terminal/output"),
        (5, "elicitation/create"),
        (6, "x/teleport"),
    ] {
        harness.agent_request(json!(id), method, json!({}));
        let reply = harness.line().await;
        assert_eq!(reply["id"], id);
        assert_eq!(reply["error"]["code"], -32601, "{method}");
    }
}

#[tokio::test(start_paused = true)]
async fn auth_status_redacted() {
    let mut harness = ready_generic().await;
    harness.send(
        json!({"jsonrpc": "2.0", "method": "_auth/status_update", "params": {
        "authStatus": {"account": {"email": "someone@example.com", "organization": "Acme"}}}}),
    );
    harness.send(json!({"jsonrpc": "2.0", "method": "x/some_notification", "params": {}}));
    harness.update(json!({"sessionUpdate": "session_info_update", "title": "Marker"}));
    // Nothing of either notification becomes an event, so nothing reaches the database.
    assert_eq!(
        harness.event().await,
        DriverEvent::Title(Some("Marker".into()))
    );
    harness.no_line();
}

// ---- stdout and the exit -----------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn invalid_lines_and_batches() {
    let mut harness = ready_generic().await;
    harness.raw("this is not json");
    harness.raw("{\"jsonrpc\":\"2.0\"}");
    harness.raw(&format!(
        "[{},{}]",
        json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": SESSION, "update": {"sessionUpdate": "session_info_update", "title": "one"}}}),
        json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": SESSION, "update": {"sessionUpdate": "session_info_update", "title": "two"}}}),
    ));
    assert_eq!(
        harness.event().await,
        DriverEvent::Title(Some("one".into()))
    );
    assert_eq!(
        harness.event().await,
        DriverEvent::Title(Some("two".into()))
    );
    harness.no_line();
}

#[tokio::test(start_paused = true)]
async fn oversized_request_is_answered() {
    let mut harness = ready_generic().await;
    harness
        .end
        .to_client
        .send(FakeLine::Oversized {
            bytes: 70_000_000,
            keys: TopKeys {
                id: Some(json!(12)),
                method: Some("session/request_permission".into()),
                kind: None,
            },
        })
        .expect("stdout");
    let reply = harness.line().await;
    assert_eq!(
        reply,
        json!({"jsonrpc": "2.0", "id": 12, "error": {"code": -32603,
        "message": "Ghost can't accept a request of 67 MB; the limit is 64 MiB."}})
    );
}

#[tokio::test(start_paused = true)]
async fn oversized_response_fails_request() {
    // The initialize reply: the start fails.
    let mut harness = Harness::start(setup("generic")).await;
    let (id, _) = harness.request("initialize").await;
    harness
        .end
        .to_client
        .send(FakeLine::Oversized {
            bytes: 80 * 1024 * 1024,
            keys: TopKeys {
                id: Some(id),
                method: None,
                kind: None,
            },
        })
        .expect("stdout");
    let (_, exit) = harness.finish().await;
    assert_eq!(
        exit,
        DriverExit::StartFailed(DriverError {
            kind: ErrorKind::ProtocolError,
            message: "Ghost dropped a response of 80 MB; the limit is 64 MiB.".into(),
            detail: None,
        })
    );

    // A prompt's reply: the turn ends with the error.
    let mut harness = ready_generic().await;
    harness.prompt("T", "go");
    let (id, _) = harness.request("session/prompt").await;
    harness
        .end
        .to_client
        .send(FakeLine::Oversized {
            bytes: 70_000_000,
            keys: TopKeys {
                id: Some(id),
                method: None,
                kind: None,
            },
        })
        .expect("stdout");
    let events = harness.events_until(is_turn_ended).await;
    assert_eq!(
        turn_ended(&events).3.map(|error| error.kind),
        Some(ErrorKind::ProtocolError)
    );
}

#[tokio::test(start_paused = true)]
async fn an_oversized_notification_becomes_a_notice_and_the_next_line_is_intact() {
    let mut harness = ready_generic().await;
    harness
        .end
        .to_client
        .send(FakeLine::Oversized {
            bytes: 70_000_000,
            keys: TopKeys {
                id: None,
                method: Some("session/update".into()),
                kind: None,
            },
        })
        .expect("stdout");
    harness.update(json!({"sessionUpdate": "session_info_update", "title": "next"}));
    assert_eq!(
        harness.event().await,
        DriverEvent::Event {
            turn: None,
            event: GhostEvent::Notice {
                level: NoticeLevel::Warning,
                message: OVERSIZED_UPDATE_NOTICE.into()
            },
        }
    );
    assert_eq!(
        harness.event().await,
        DriverEvent::Title(Some("next".into()))
    );
}

#[tokio::test(start_paused = true)]
async fn stdout_eof_with_live_process_kills_tree() {
    let harness = ready_generic().await;
    harness.eof();
    let (_, exit) = harness.finish_watching_kill().await;
    assert!(
        matches!(
            exit,
            DriverExit::Crashed {
                exit: Some(ExitInfo {
                    signal: Some(9),
                    ..
                }),
                ..
            }
        ),
        "{exit:?}"
    );
}

impl Harness {
    /// `finish`, asserting the tree was killed about 2 s after stdout ended.
    async fn finish_watching_kill(self) -> (Vec<DriverEvent>, DriverExit) {
        let start = tokio::time::Instant::now();
        let control = Arc::clone(&self.spawner.controls()[0]);
        let result = self.finish().await;
        assert!(control.calls().contains(&FakeCall::KillTree));
        assert!(start.elapsed() >= EXIT_AFTER_EOF_GRACE);
        result
    }
}

#[tokio::test(start_paused = true)]
async fn crash_mid_turn_ends_with_the_raw_tail_and_no_turn_end() {
    let mut harness = ready_generic().await;
    harness.prompt("T", "go");
    harness.request("session/prompt").await;
    harness.update(chunk("partial"));
    harness
        .end
        .stderr
        .send("panic: sk-ant-api03-secret leaked here".into())
        .expect("stderr");
    tokio::time::sleep(Duration::from_millis(1)).await;
    harness.exit_with(101);
    harness.eof();
    let (events, exit) = harness.finish().await;
    assert!(!events.iter().any(is_turn_ended), "the actor ends the turn");
    assert!(transcript(&events).iter().any(
        |(_, event)| matches!(event, GhostEvent::MessageDelta { text, .. } if text == "partial")
    ));
    assert_eq!(
        exit,
        DriverExit::Crashed {
            exit: Some(ExitInfo {
                code: Some(101),
                signal: None,
                at_ms: 0
            }),
            stderr_tail: "panic: sk-ant-api03-secret leaked here\n".into(),
        }
    );
}

#[tokio::test(start_paused = true)]
async fn exit_under_backpressure_loses_nothing() {
    let mut harness = ready_generic().await;
    harness.prompt("T", "go");
    let (prompt, _) = harness.request("session/prompt").await;
    // The agent writes 2,000 updates and its reply, then exits, while nobody reads events.
    for index in 0..2_000 {
        harness.update(chunk(&format!("{index} ")));
    }
    harness.reply(&prompt, json!({"stopReason": "end_turn"}));
    harness.exit_with(0);
    harness.eof();
    // Far longer than the 2 s pipe grace passes before anyone reads.
    tokio::time::sleep(Duration::from_secs(30)).await;
    let (events, exit) = harness.finish().await;
    let text: String = transcript(&events)
        .into_iter()
        .filter_map(|(_, event)| match event {
            GhostEvent::MessageDelta { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    let expected: String = (0..2_000).map(|index| format!("{index} ")).collect();
    assert_eq!(text, expected, "every line was handled");
    assert_eq!(turn_ended(&events).1, StopReason::EndTurn);
    assert!(matches!(
        exit,
        DriverExit::Crashed {
            exit: Some(ExitInfo { code: Some(0), .. }),
            ..
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn a_descendant_holding_stdout_gets_the_pipe_grace() {
    let mut harness = ready_generic().await;
    harness.exit_with(0);
    // stdout never ends: after the grace the driver stops reading and returns.
    let start = tokio::time::Instant::now();
    let (_, exit) = harness.finish().await;
    assert!(matches!(exit, DriverExit::Crashed { .. }));
    assert!(start.elapsed() >= PIPE_EOF_GRACE);
}

#[tokio::test(start_paused = true)]
async fn a_descendant_that_keeps_writing_after_the_exit_gets_only_the_pipe_grace() {
    let mut harness = ready_generic().await;
    harness.exit_with(0);
    let exited = tokio::time::Instant::now();
    // A descendant holds the pipe and writes a line every 500 ms for five minutes.
    let to_client = harness.end.to_client.clone();
    let writer = tokio::spawn(async move {
        for index in 0..600 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let update = json!({"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": SESSION,
                "update": {"sessionUpdate": "session_info_update", "title": format!("t{index}")}}});
            if to_client.send(FakeLine::Line(update.to_string())).is_err() {
                return;
            }
        }
    });
    let (events, exit) = harness.finish().await;
    let took = exited.elapsed();
    assert!(matches!(exit, DriverExit::Crashed { .. }), "{exit:?}");
    // Lines do not stretch the grace: 2 s of reading in all, and the crash is reported.
    assert!(
        took >= PIPE_EOF_GRACE && took < PIPE_EOF_GRACE + Duration::from_millis(500),
        "{took:?}"
    );
    let titles = events
        .iter()
        .filter(|event| matches!(event, DriverEvent::Title(_)))
        .count();
    assert!(
        titles >= 3,
        "what came within the grace was handled: {titles}"
    );
    writer.abort();
}

#[tokio::test(start_paused = true)]
async fn the_pipe_grace_pauses_while_the_session_is_slow_and_never_starts_over() {
    let mut harness = ready_generic().await;
    harness.exit_with(0);
    let exited = tokio::time::Instant::now();
    // 1 s of the grace passes with nothing to read.
    tokio::time::sleep(Duration::from_secs(1)).await;
    // Then far more than the outbox holds arrives while nobody reads events: the grace waits.
    for index in 0..1_000 {
        harness.update(chunk(&format!("{index} ")));
    }
    tokio::time::sleep(Duration::from_secs(30)).await;
    // Once the session reads again, the rest of the grace runs: 1 s, not a fresh 2 s.
    let (events, exit) = harness.finish().await;
    let took = exited.elapsed();
    assert!(matches!(exit, DriverExit::Crashed { .. }), "{exit:?}");
    assert!(
        took >= Duration::from_secs(32) && took < Duration::from_millis(32_500),
        "{took:?}"
    );
    let deltas = transcript(&events)
        .iter()
        .filter(|(_, event)| matches!(event, GhostEvent::MessageDelta { .. }))
        .count();
    assert_eq!(deltas, 1_000, "every line was handled");
}

// ---- shutdown ----------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_the_turn_answers_requests_and_closes() {
    let mut harness = ready_generic().await;
    harness.agent_request(
        json!("bg"),
        "session/request_permission",
        permission_params("bg", "execute"),
    );
    next_permission(&mut harness).await;
    harness.prompt("T", "go");
    let (prompt, _) = harness.request("session/prompt").await;
    harness.command(DriverCommand::Shutdown {
        plan: StopPlan {
            cancel_grace: Duration::from_secs(3),
            process: ShutdownPlan::STANDARD,
        },
    });
    assert_eq!(harness.line().await["method"], "session/cancel");
    // The turn's reply within the grace ends it with its own reason.
    harness.reply(&prompt, json!({"stopReason": "cancelled"}));
    // The untagged request is answered `cancelled` before stdin closes.
    assert_eq!(
        harness.line().await,
        json!({"jsonrpc": "2.0", "id": "bg", "result": {"outcome": {"outcome": "cancelled"}}})
    );
    assert_eq!(
        harness.end.from_client.recv().await,
        None,
        "stdin reached its end"
    );
    // The agent exits on stdin EOF.
    harness.exit_with(0);
    harness.eof();
    let (events, exit) = harness.finish().await;
    assert_eq!(exit, DriverExit::Closed);
    assert_eq!(turn_ended(&events).1, StopReason::Cancelled);
    assert!(events.contains(&DriverEvent::Status(DriverStatus::Closing)));
}

#[tokio::test(start_paused = true)]
async fn a_cancel_that_never_lands_waits_only_the_grace() {
    let mut harness = ready_generic().await;
    harness.prompt("T", "go");
    harness.request("session/prompt").await;
    harness.command(DriverCommand::Shutdown {
        plan: StopPlan {
            cancel_grace: Duration::from_secs(3),
            process: ShutdownPlan::STANDARD,
        },
    });
    assert_eq!(harness.line().await["method"], "session/cancel");
    let start = tokio::time::Instant::now();
    // stdin closes after the grace.
    assert_eq!(
        timeout(Duration::from_secs(10), harness.end.from_client.recv())
            .await
            .expect("closed in time"),
        None
    );
    assert!(start.elapsed() >= Duration::from_secs(3));
    let (events, exit) = harness.finish().await;
    assert_eq!(exit, DriverExit::Closed);
    assert!(
        !events.iter().any(is_turn_ended),
        "the actor ends the turn Interrupted"
    );
}

#[tokio::test(start_paused = true)]
async fn a_closed_command_channel_is_a_shutdown_with_zero_grace() {
    let mut harness = ready_generic().await;
    harness.prompt("T", "go");
    harness.request("session/prompt").await;
    harness.commands = None;
    assert_eq!(harness.line().await["method"], "session/cancel");
    assert_eq!(
        timeout(Duration::from_secs(1), harness.end.from_client.recv())
            .await
            .expect("closed at once"),
        None
    );
    let calls_before_end = Arc::clone(&harness.spawner.controls()[0]);
    let (_, exit) = harness.finish().await;
    assert_eq!(exit, DriverExit::Closed);
    assert!(
        calls_before_end
            .calls()
            .contains(&FakeCall::Shutdown(ShutdownPlan::FORCE))
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_before_ready_closes() {
    let mut harness = Harness::start(setup("generic")).await;
    harness.request("initialize").await;
    harness.command(DriverCommand::Shutdown {
        plan: StopPlan {
            cancel_grace: Duration::from_secs(3),
            process: ShutdownPlan::STANDARD,
        },
    });
    assert_eq!(
        timeout(Duration::from_secs(1), harness.end.from_client.recv())
            .await
            .expect("closed"),
        None
    );
    harness.exit_with(0);
    harness.eof();
    let (_, exit) = harness.finish().await;
    assert_eq!(exit, DriverExit::Closed);
}

#[tokio::test(start_paused = true)]
async fn a_dropped_receiver_discards_events_and_the_driver_still_ends() {
    let harness = ready_generic().await;
    let Harness {
        commands,
        events,
        run,
        end,
        exit,
        spawner,
        ..
    } = harness;
    drop(events);
    for index in 0..500 {
        end.to_client
            .send(FakeLine::Line(
                json!({"jsonrpc": "2.0", "method": "session/update",
            "params": {"sessionId": SESSION, "update": chunk(&index.to_string())}})
                .to_string(),
            ))
            .expect("stdout");
    }
    if let Some(exit) = exit {
        let _ = exit.send(ExitInfo {
            code: Some(0),
            signal: None,
            at_ms: 0,
        });
    }
    let _ = end.to_client.send(FakeLine::Eof);
    let result = timeout(Duration::from_secs(60), run)
        .await
        .expect("returns")
        .expect("no panic");
    assert!(matches!(result, DriverExit::Crashed { .. }));
    drop((commands, spawner));
}

// ---- commands and plans ------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn commands_are_debounced_to_the_last_list() {
    let mut harness = ready_generic().await;
    harness.update(json!({"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "a", "description": "A"}]}));
    tokio::time::sleep(Duration::from_millis(100)).await;
    harness.update(json!({"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "b", "description": "B", "input": {"hint": "what"}}]}));
    let start = tokio::time::Instant::now();
    let DriverEvent::Commands(commands) = harness.event().await else {
        panic!("Commands");
    };
    assert!(start.elapsed() >= Duration::from_millis(250));
    assert_eq!(commands.len(), 1);
    assert_eq!(
        (commands[0].name.as_str(), commands[0].input_hint.as_deref()),
        ("b", Some("what"))
    );
}

#[tokio::test(start_paused = true)]
async fn unknown_updates_are_skipped_and_the_stream_continues() {
    let mut harness = ready_generic().await;
    harness.update(json!({"sessionUpdate": "brand_new_kind", "payload": [1, 2, 3]}));
    harness.update(json!({"sessionUpdate": "agent_message_chunk", "content": 42}));
    harness.update(json!({"sessionUpdate": "plan", "entries": [{"content": "Step", "priority": "high", "status": "pending"}]}));
    let DriverEvent::Event {
        event: GhostEvent::PlanUpdated { entries },
        ..
    } = harness.event().await
    else {
        panic!("PlanUpdated");
    };
    assert_eq!(entries.len(), 1);
}

#[test]
fn mode_names_read_once_in_the_hand_picked_note() {
    assert_eq!(mode_phrase("Plan Mode"), "Plan");
    assert_eq!(mode_phrase("plan mode "), "plan");
    assert_eq!(mode_phrase("Accept Edits"), "Accept Edits");
    assert_eq!(mode_phrase(" Mode"), " Mode");
    assert_eq!(mode_phrase("Mode"), "Mode");
    assert_eq!(mode_phrase("Modé mode"), "Modé");
}

/// Holds every spawn for a while before it starts (a macOS privacy prompt answered slowly).
struct SlowSpawner {
    inner: QuietSpawner,
    delay: Duration,
}

impl AgentSpawner for SlowSpawner {
    fn spawn(
        &self,
        spec: ghost_process::SpawnSpec,
    ) -> futures::future::BoxFuture<'static, Result<ghost_process::SpawnedProcess, SpawnError>>
    {
        let spawning = self.inner.spawn(spec);
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            spawning.await
        })
    }
}

#[tokio::test(start_paused = true)]
async fn the_start_timeout_covers_only_the_launch() {
    let (spawner, mut ends) = FakeSpawner::new();
    let (_commands, receiver) = mpsc::unbounded_channel();
    let (sender, mut events) = event_channel();
    let slow = SlowSpawner {
        inner: QuietSpawner(spawner),
        delay: Duration::from_secs(120),
    };
    let run = tokio::spawn(Box::new(AcpDriver::new()).run(DriverContext {
        commands: receiver,
        events: sender,
        spawner: Arc::new(slow),
        setup: setup("generic"),
    }));
    let mut end = timeout(Duration::from_secs(300), ends.recv())
        .await
        .expect("spawned")
        .expect("an end");
    let line = end.from_client.recv().await.expect("initialize");
    let id = serde_json::from_str::<Value>(&line).expect("json")["id"].clone();
    // 120 s went into the spawn; the agent answers 59 s after it started.
    tokio::time::sleep(Duration::from_secs(59)).await;
    end.to_client
        .send(FakeLine::Line(
            json!({"jsonrpc": "2.0", "id": id, "result": init_result()}).to_string(),
        ))
        .expect("stdout");
    let line = end.from_client.recv().await.expect("session/new");
    assert!(line.contains("session/new"), "{line}");
    assert!(!run.is_finished());
    assert_eq!(
        events.recv().await,
        Some(DriverEvent::Status(DriverStatus::Spawning))
    );
}

#[test]
fn the_outbox_is_full_at_64_events_or_8_mib_with_held_events_counted() {
    // The driver reads stdout only while its outbox has room: 64 events or 8 MiB.
    let (sender, _receiver) = event_channel();
    let mut outbox = Outbox::new(sender);
    let small = DriverEvent::Title(Some("x".into()));
    for _ in 0..OUTBOX_EVENTS - 1 {
        outbox.push(small.clone());
    }
    assert!(!outbox.is_full());
    outbox.push(small.clone());
    assert!(outbox.is_full(), "64 events");
    let (sender, _receiver) = event_channel();
    let mut outbox = Outbox::new(sender);
    outbox.push(DriverEvent::Title(Some("x".repeat(OUTBOX_BYTES))));
    assert!(outbox.is_full(), "8 MiB");
    // Once the session stops reading for good, nothing is kept.
    outbox.discard();
    outbox.push(DriverEvent::Title(None));
    outbox.hold(DriverEvent::Title(None));
    assert!(!outbox.has_head() && !outbox.is_full());

    // Events held for Ready count toward the same bound, and go out after it in order.
    let (sender, _receiver) = event_channel();
    let mut outbox = Outbox::new(sender);
    outbox.push(small.clone());
    for _ in 0..OUTBOX_EVENTS - 1 {
        outbox.hold(small.clone());
    }
    assert!(outbox.is_full(), "held events count");
    assert_eq!(outbox.queue.len(), 1, "but they cannot be handed over yet");
    outbox.release_held();
    assert_eq!(outbox.queue.len(), OUTBOX_EVENTS);
    // A start that ends without Ready drops them, and with them their weight.
    let (sender, _receiver) = event_channel();
    let mut outbox = Outbox::new(sender);
    outbox.hold(DriverEvent::Title(Some("x".repeat(OUTBOX_BYTES))));
    assert!(outbox.is_full());
    assert_eq!(outbox.drop_held(), 1);
    assert!(!outbox.is_full() && !outbox.has_head());
}

#[tokio::test(start_paused = true)]
async fn slow_consumer_blocks_agent_not_memory() {
    let mut harness = ready_generic().await;
    // The agent writes 2,000 updates and then a request, while nobody reads events.
    for index in 0..2_000 {
        harness.update(chunk(&format!("{index} ")));
    }
    harness.agent_request(json!("after-the-flood"), "x/unknown", json!({}));
    tokio::time::sleep(Duration::from_secs(5)).await;
    // The driver stopped reading once its outbox was full (about 256 events in the channel, 64
    // in the outbox and 16 lines in the reader's channel): the request is still in the pipe.
    harness.no_line();
    // Once the session reads again, everything arrives in order and the request is answered.
    let expected: String = (0..2_000).map(|index| format!("{index} ")).collect();
    let mut text = String::new();
    while text.len() < expected.len() {
        if let DriverEvent::Event {
            event: GhostEvent::MessageDelta { text: piece, .. },
            ..
        } = harness.event().await
        {
            text.push_str(&piece);
        }
    }
    assert_eq!(text, expected);
    let reply = harness.line().await;
    assert_eq!(reply["id"], "after-the-flood");
    assert_eq!(reply["error"]["code"], -32601);
}

/// A generic agent's model option, with `current` as its value.
fn model_option(current: &str) -> Value {
    json!({"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": current,
           "options": [{"value": "a", "name": "A"}, {"value": "b", "name": "B"}]})
}

/// A generic agent configuring an earlier choice of model, whose answer the agent sends only after
/// 2,000 updates and before a request of its own; nobody reads events.
async fn flood_while_configuring() -> Harness {
    let mut chosen = setup("generic");
    chosen
        .desired
        .insert("model".into(), ConfigValue::Choice { value: "b".into() });
    let mut harness = Harness::start(chosen).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (id, _) = harness.request("session/new").await;
    harness.reply(
        &id,
        json!({"sessionId": SESSION, "configOptions": [model_option("a")]}),
    );
    let (set, params) = harness.request("session/set_config_option").await;
    assert_eq!(params["value"], "b");
    for index in 0..2_000 {
        harness.update(chunk(&format!("{index} ")));
    }
    harness.reply(&set, json!({"configOptions": [model_option("b")]}));
    harness.agent_request(json!("after-the-flood"), "x/unknown", json!({}));
    tokio::time::sleep(Duration::from_secs(5)).await;
    harness
}

#[tokio::test(start_paused = true)]
async fn a_flood_while_configuring_pauses_stdout_and_loses_nothing() {
    let mut harness = flood_while_configuring().await;
    // Transcript events wait for Ready and count toward the outbox, so reading stopped after
    // about 64 of them: the request is unanswered and memory stays bounded.
    harness.no_line();
    // The choice's answer sits behind the flood, so its request times out and the start goes on.
    tokio::time::sleep(Duration::from_secs(30)).await;
    let expected: String = (0..2_000).map(|index| format!("{index} ")).collect();
    let mut events = Vec::new();
    let mut text = String::new();
    while text.len() < expected.len() {
        let event = harness.event().await;
        if let DriverEvent::Event {
            event: GhostEvent::MessageDelta { text: piece, .. },
            ..
        } = &event
        {
            text.push_str(piece);
        }
        events.push(event);
    }
    assert_eq!(text, expected, "nothing was dropped, and the order held");
    let ready_at = events
        .iter()
        .position(|event| matches!(event, DriverEvent::Ready(_)))
        .expect("Ready");
    let first_delta = events
        .iter()
        .position(|event| {
            matches!(
                event,
                DriverEvent::Event {
                    event: GhostEvent::MessageDelta { .. },
                    ..
                }
            )
        })
        .expect("a delta");
    assert!(ready_at < first_delta, "rule 1");
    assert!(transcript(&events).iter().any(|(_, event)| matches!(event,
        GhostEvent::Notice { message, .. } if message == "Ghost could not restore the model you chose earlier (the agent did not answer within 30 seconds).")));
    let reply = harness.line().await;
    assert_eq!(reply["id"], "after-the-flood");
    assert_eq!(reply["error"]["code"], -32601);
}

#[tokio::test(start_paused = true)]
async fn a_start_that_ends_while_events_wait_for_ready_still_returns() {
    // A stop: what waited for Ready can never be handed over, so it no longer keeps stdout paused.
    let mut harness = flood_while_configuring().await;
    harness.no_line();
    harness.command(DriverCommand::Shutdown {
        plan: StopPlan {
            cancel_grace: Duration::from_secs(3),
            process: ShutdownPlan::STANDARD,
        },
    });
    let (events, exit) = harness.finish().await;
    assert_eq!(exit, DriverExit::Closed);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, DriverEvent::Ready(_)))
    );
    assert!(
        transcript(&events)
            .iter()
            .all(|(_, event)| matches!(event, GhostEvent::Notice { .. })),
        "rule 1: no transcript event without Ready"
    );

    // The agent's exit, with a descendant still holding the pipe: the same, within the grace.
    let mut harness = flood_while_configuring().await;
    harness.no_line();
    harness.exit_with(1);
    let (events, exit) = harness.finish().await;
    assert!(
        matches!(
            exit,
            DriverExit::StartFailed(DriverError {
                kind: ErrorKind::AgentExited,
                ..
            })
        ),
        "{exit:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, DriverEvent::Ready(_)))
    );
}

#[tokio::test(start_paused = true)]
async fn early_updates_are_bounded_by_bytes_too() {
    let mut harness = Harness::start(setup("generic")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (open, _) = harness.request("session/new").await;
    // Three updates of 3 MiB before the reply names the session: the third would take the buffer
    // past 8 MiB, so it is dropped.
    for marker in ["first", "second", "third"] {
        harness.update(chunk(&format!("{marker}:{}", "x".repeat(3 * 1024 * 1024))));
    }
    harness.reply(&open, json!({"sessionId": SESSION}));
    harness.update(json!({"sessionUpdate": "session_info_update", "title": "after"}));
    let events = harness
        .events_until(|event| event == &DriverEvent::Title(Some("after".into())))
        .await;
    let markers: Vec<String> = transcript(&events)
        .into_iter()
        .filter_map(|(_, event)| match event {
            GhostEvent::MessageDelta { text, .. } => text.split(':').next().map(str::to_owned),
            _ => None,
        })
        .collect();
    assert_eq!(markers, ["first", "second"]);

    // One update larger than the whole buffer still passes when it comes alone.
    let mut harness = Harness::start(setup("generic")).await;
    let (id, _) = harness.request("initialize").await;
    harness.reply(&id, init_result());
    let (open, _) = harness.request("session/new").await;
    harness.update(chunk(&format!("big:{}", "x".repeat(EARLY_UPDATE_BYTES))));
    harness.reply(&open, json!({"sessionId": SESSION}));
    harness.update(json!({"sessionUpdate": "session_info_update", "title": "after"}));
    let events = harness
        .events_until(|event| event == &DriverEvent::Title(Some("after".into())))
        .await;
    assert!(transcript(&events).iter().any(
        |(_, event)| matches!(event, GhostEvent::MessageDelta { text, .. } if text.starts_with("big:"))
    ));
}

#[tokio::test(start_paused = true)]
async fn stdout_ending_during_a_graceful_stop_leaves_the_process_to_its_chain() {
    let mut harness = ready_generic().await;
    harness.command(DriverCommand::Shutdown {
        plan: StopPlan {
            cancel_grace: Duration::from_secs(3),
            process: ShutdownPlan::STANDARD,
        },
    });
    assert_eq!(
        timeout(Duration::from_secs(1), harness.end.from_client.recv())
            .await
            .expect("closed"),
        None
    );
    // The agent closes stdout at once but takes 4 s to exit, within the chain's 5 s EOF grace.
    harness.eof();
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        !harness.calls().contains(&FakeCall::KillTree),
        "no early kill"
    );
    harness.exit_with(0);
    let (_, exit) = harness.finish().await;
    assert_eq!(exit, DriverExit::Closed);
}

// ---- the protocol log ------------------------------------------------------------------------

/// The records of a protocol log file, read again until `done` holds or 5 s pass: the writer task
/// appends on the blocking pool, a little after the driver handed a record over.
async fn read_log_until(path: &std::path::Path, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    for _ in 0..250 {
        if let Ok(text) = std::fs::read_to_string(path) {
            let records: Vec<Value> = text
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            if done(&records) {
                return records;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{} never held the expected records", path.display());
}

/// Contract rule 6: right after the spawner's spawn note comes the driver's start note (no env),
/// and every command it receives becomes a `cmd` record in the order it was processed, a command
/// held before `Ready` included. The log is read back from disk, in a fresh temp folder.
#[tokio::test]
async fn the_log_has_the_start_note_after_the_spawn_note_and_every_command_in_order() {
    let root =
        std::env::temp_dir().join(format!("ghost-drivers-log-test-{}", uuid::Uuid::now_v7()));
    let logs = LogFactory::new(
        root.clone(),
        Arc::new(Redactor::standard()),
        tokio::runtime::Handle::current(),
    );
    let mut harness =
        Harness::start_through(setup("generic"), |fake| DirectSpawner::new(fake, logs)).await;
    // Held until Ready, and logged when it arrives.
    let early = set_autonomy(&harness, Autonomy::Ask);
    open_with(&mut harness, init_result(), json!({"sessionId": SESSION})).await;
    assert!(early.await.expect("answered").is_ok());
    harness.prompt("T", "go");
    let (prompt, _) = harness.request("session/prompt").await;
    harness.command(DriverCommand::Interrupt);
    harness.request_or_notification("session/cancel").await;
    harness.reply(&prompt, json!({"stopReason": "cancelled"}));
    harness.events_until(is_turn_ended).await;
    harness.command(DriverCommand::Shutdown {
        plan: StopPlan {
            cancel_grace: Duration::from_secs(1),
            process: ShutdownPlan::STANDARD,
        },
    });
    assert_eq!(harness.end.from_client.recv().await, None, "stdin closed");
    harness.exit_with(0);
    harness.eof();
    let spawn_id = harness.spawn_id.clone();
    let (_, exit) = harness.finish().await;
    assert_eq!(exit, DriverExit::Closed);
    let path = root.join(DIRECT_LOG_CHAT).join(format!("{spawn_id}.jsonl"));
    let records = read_log_until(&path, |records| {
        records
            .iter()
            .any(|record| record["cmd"]["type"] == "shutdown")
    })
    .await;
    assert_eq!(records[0]["d"], "note");
    assert!(records[0]["note"].get("spawn").is_some(), "{}", records[0]);
    assert_eq!(records[1]["d"], "note");
    let start = &records[1]["note"]["start"];
    assert_eq!(start["spawn_id"], spawn_id.as_str(), "{}", records[1]);
    assert_eq!(start["agent_id"], "generic");
    assert!(start.get("env").is_none(), "no env in the start note");
    let commands: Vec<&str> = records
        .iter()
        .filter(|record| record["d"] == "cmd")
        .filter_map(|record| record["cmd"]["type"].as_str())
        .collect();
    assert_eq!(
        commands,
        ["set_autonomy", "prompt", "interrupt", "shutdown"]
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ---- display terminals -------------------------------------------------------------------------

/// codex-acp's display terminals: the command's output streams on the tool call's updates and
/// shows as its terminal (a held chunk on the next timer tick), then its exit code and everything.
#[tokio::test(start_paused = true)]
async fn an_agents_display_terminal_streams_its_output_and_exit() {
    let terminal_of = |event: &DriverEvent| match event {
        DriverEvent::Event {
            event: GhostEvent::ToolUpdate(update),
            ..
        } => update.content.as_ref().and_then(|content| {
            content.iter().find_map(|entry| match entry {
                ToolContent::Terminal {
                    output, exit_code, ..
                } => Some((output.clone(), *exit_code)),
                _ => None,
            })
        }),
        _ => None,
    };
    let mut harness = ready_generic().await;
    harness.prompt("T", "run the tests");
    let (prompt, _) = harness.request("session/prompt").await;
    harness.update(
        json!({"sessionUpdate": "tool_call", "toolCallId": "exec-1", "title": "cargo test",
        "kind": "execute", "status": "in_progress",
        "content": [{"type": "terminal", "terminalId": "exec-1"}],
        "_meta": {"terminal_info": {"terminal_id": "exec-1", "cwd": "/w"}}}),
    );
    let output = |data: &str| {
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "exec-1",
        "_meta": {"terminal_output": {"terminal_id": "exec-1", "data": data}}})
    };
    harness.update(output("\u{1b}[32mrunning\u{1b}[0m 2 tests\r\n"));
    let first = harness
        .events_until(|event| terminal_of(event).is_some())
        .await;
    assert_eq!(
        terminal_of(first.last().unwrap()),
        Some(("running 2 tests\n".to_owned(), None))
    );
    // Within the throttle window the next chunk is held, then shows on the timer.
    harness.update(output("test a ... ok\n"));
    let held = harness
        .events_until(|event| terminal_of(event).is_some())
        .await;
    assert_eq!(
        terminal_of(held.last().unwrap()),
        Some(("running 2 tests\ntest a ... ok\n".to_owned(), None))
    );
    harness.update(
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "exec-1",
        "status": "completed",
        "_meta": {"terminal_exit": {"terminal_id": "exec-1", "exit_code": 0, "signal": null}}}),
    );
    let end = harness
        .events_until(|event| terminal_of(event).is_some())
        .await;
    assert_eq!(
        terminal_of(end.last().unwrap()),
        Some(("running 2 tests\ntest a ... ok\n".to_owned(), Some(0)))
    );
    harness.reply(&prompt, json!({"stopReason": "end_turn"}));
    harness.events_until(is_turn_ended).await;
}

/// The client says it shows display terminals: codex-acp streams output only to clients that do.
#[tokio::test(start_paused = true)]
async fn initialize_advertises_display_terminal_output() {
    let mut harness = Harness::start(setup("generic")).await;
    let (_, params) = harness.request("initialize").await;
    assert_eq!(
        params["clientCapabilities"]["_meta"]["terminal_output"],
        json!(true)
    );
    // It still runs no command for the agent.
    assert_eq!(params["clientCapabilities"]["terminal"], json!(false));
}
