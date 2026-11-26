use super::*;
use crate::driver::{
    AgentSpawner, DriverCommand, DriverEvent, DriverTimings, EventReceiver, SessionSetup, StopPlan,
    TurnRef, event_channel,
};
use ghost_process::testing::{FakeAgentEnd, FakeLine, FakeSpawner};
use ghost_process::{
    EnvMap, EnvSource, LaunchSpec, ProcessCwd, ProcessSpawner, ProtocolLog, ResolvedEnv,
    ShutdownPlan, SpawnError, SpawnSpec, SpawnedProcess,
};
use ghost_protocol::{Autonomy, GhostEvent, PromptPart, StopReason};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio::time::timeout;

struct Quiet(Arc<FakeSpawner>);
fn steer(h: &Harness, turn: &str) -> tokio::sync::oneshot::Receiver<crate::driver::SteerResult> {
    let (reply, result) = tokio::sync::oneshot::channel();
    h.tx.send(DriverCommand::Steer {
        turn_id: turn.into(),
        message_id: "steer-1".into(),
        parts: vec![PromptPart::Text {
            text: "Focus on the tests.".into(),
        }],
        reply,
    })
    .unwrap();
    result
}

async fn running(h: &mut Harness) {
    h.begin().await;
    h.send(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"message-1","delta":"Working"}}));
    loop {
        if matches!(
            h.event().await,
            DriverEvent::Event {
                event: GhostEvent::MessageDelta { .. },
                ..
            }
        ) {
            break;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn native_image_payloads_are_scrubbed_before_tool_raw_input_without_storage() {
    let mut h = Harness::start().await;
    h.ready().await;
    running(&mut h).await;
    h.send(json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"mcpToolCall","id":"image-tool","server":"images","tool":"generate","status":"completed","result":{"content":[{"type":"image","mimeType":"image/png","data":"aW1hZ2U="}]}}}}));
    loop {
        if let DriverEvent::Event {
            event: GhostEvent::ToolCall(call),
            ..
        } = h.event().await
        {
            assert_eq!(call.tool_call_id, "image-tool");
            let saved = serde_json::to_string(&call).unwrap();
            assert!(!saved.contains("aW1hZ2U="));
            assert!(saved.contains("Image unavailable"));
            assert!(call.raw_input.is_some());
            break;
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn steering_uses_native_exact_turn_and_accepts_a_late_matching_acknowledgement() {
    let mut h = Harness::start().await;
    h.ready().await;
    running(&mut h).await;
    let reply = steer(&h, "aura-turn");
    let request = h.wire().await;
    assert_eq!(request["method"], "turn/steer");
    assert_eq!(
        request["params"],
        json!({"threadId":"thread-1","expectedTurnId":"turn-1","clientUserMessageId":"steer-1","input":[{"type":"text","text":"Focus on the tests."}]})
    );
    h.send(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed","items":[]}}}));
    h.send(json!({"id":request["id"],"result":{"turnId":"turn-1"}}));
    assert_eq!(reply.await.unwrap(), crate::driver::SteerResult::Accepted);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn steering_rejects_stale_local_turn_and_native_restrictions_without_queueing() {
    let mut h = Harness::start().await;
    h.ready().await;
    assert!(matches!(
        steer(&h, "aura-turn").await.unwrap(),
        crate::driver::SteerResult::Rejected { .. }
    ));
    running(&mut h).await;
    assert!(matches!(
        steer(&h, "old-turn").await.unwrap(),
        crate::driver::SteerResult::Rejected { .. }
    ));
    let reply = steer(&h, "aura-turn");
    let request = h.wire().await;
    assert_eq!(request["method"], "turn/steer");
    h.send(json!({"id":request["id"],"error":{"code":-32600,"message":"This active turn cannot be steered.","data":{"codexErrorInfo":"activeTurnNotSteerable"}}}));
    assert_eq!(
        reply.await.unwrap(),
        crate::driver::SteerResult::Rejected {
            message: "This active turn cannot be steered.".into()
        }
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn steering_wrong_turn_acknowledgement_is_unconfirmed_and_stops_the_session() {
    let mut h = Harness::start().await;
    h.ready().await;
    running(&mut h).await;
    let reply = steer(&h, "aura-turn");
    let request = h.wire().await;
    h.send(json!({"id":request["id"],"result":{"turnId":"different-turn"}}));
    assert!(matches!(
        reply.await.unwrap(),
        crate::driver::SteerResult::Unconfirmed { .. }
    ));
    while h.events.recv().await.is_some() {}
    assert!(matches!(h.run.await.unwrap(), DriverExit::Crashed { .. }));
}

#[tokio::test(start_paused = true)]
async fn steering_timeout_is_unconfirmed_and_never_retried() {
    let mut h = Harness::start().await;
    h.ready().await;
    running(&mut h).await;
    let reply = steer(&h, "aura-turn");
    assert_eq!(h.wire().await["method"], "turn/steer");
    assert!(matches!(
        reply.await.unwrap(),
        crate::driver::SteerResult::Unconfirmed { .. }
    ));
    while h.events.recv().await.is_some() {}
    assert!(matches!(h.run.await.unwrap(), DriverExit::Crashed { .. }));
}

impl AgentSpawner for Quiet {
    fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        self.0
            .spawn(spec, ProtocolLog::disabled_with_id("codex-test"))
    }
}
#[tokio::test(start_paused = true)]
async fn shared_mcp_is_immutable_thread_scoped_on_new_and_resumed_sessions_and_oauth() {
    for resume in [false, true] {
        let mut setup = setup();
        setup.mcp_servers = vec![crate::mcp::tests::server()];
        if resume {
            setup.resume = Some("thread-1".into());
        }
        let mut h = Harness::start_with(setup).await;
        let init = h.wire().await;
        h.send(json!({"id":init["id"],"result":{"userAgent":"codex/0.162.1"}}));
        assert_eq!(h.wire().await["method"], "initialized");
        let open = h.wire().await;
        assert_eq!(
            open["method"],
            if resume {
                "thread/resume"
            } else {
                "thread/start"
            }
        );
        assert_eq!(
            open["params"]["config"]["mcp_servers"]["docs"]["url"],
            "https://example.test/mcp"
        );
        assert!(!open.to_string().contains("short"));
        let models = h.wire().await;
        h.send(json!({"id":models["id"],"result":{"data":[],"nextCursor":null}}));
        h.send(json!({"id":open["id"],"result":{"thread":{"id":"thread-1","status":{"type":"idle"},"cliVersion":"0.162.1"},"model":"model-a","approvalPolicy":"untrusted","approvalsReviewer":"user","sandbox":{"type":"readOnly"}}}));
        while !matches!(h.event().await, DriverEvent::Ready(_)) {}
        let (reply, answer) = tokio::sync::oneshot::channel();
        h.tx.send(DriverCommand::McpAction {
            server_name: "docs".into(),
            action: ghost_protocol::mcp::McpSessionAction::Login,
            reply,
        })
        .unwrap();
        let login = h.wire().await;
        assert_eq!(login["method"], "mcpServer/oauth/login");
        assert_eq!(login["params"]["threadId"], "thread-1");
        assert_eq!(login["params"]["name"], "docs");
        h.send(json!({"id":login["id"],"result":{"authorizationUrl":"https://example.test/oauth?state=local"}}));
        assert_eq!(
            answer.await.unwrap().unwrap().state,
            "authorization_required"
        );
        h.stop().await;
    }
}
fn setup() -> SessionSetup {
    SessionSetup {
        mcp_servers: Vec::new(),
        attachment_dir: None,
        attachment_writer: None,
        chat_id: "chat".into(),
        agent_id: "codex".into(),
        dialect: "codex".into(),
        folder: PathBuf::from("/work"),
        agent_folder: "/work".into(),
        add_dirs: vec![],
        launch: LaunchSpec {
            label: "codex".into(),
            program: "/bin/codex".into(),
            args: vec!["app-server".into(), "--listen".into(), "stdio://".into()],
            cwd: ProcessCwd::ChatFolder,
            set_env: vec![],
            daemons: vec![],
            startup_timeout: Duration::from_secs(60),
        },
        env: Arc::new(ResolvedEnv {
            vars: EnvMap::new(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: vec![],
            resolved_at_ms: 0,
        }),
        resume: None,
        native_start: Default::default(),
        autonomy: Autonomy::Ask,
        desired: BTreeMap::new(),
        timings: DriverTimings::STANDARD,
        client_version: "0.1".into(),
    }
}
struct Harness {
    tx: mpsc::UnboundedSender<DriverCommand>,
    events: EventReceiver,
    end: FakeAgentEnd,
    run: tokio::task::JoinHandle<DriverExit>,
}
impl Harness {
    async fn start_with(setup: SessionSetup) -> Self {
        let (spawner, mut ends) = FakeSpawner::new();
        let (tx, commands) = mpsc::unbounded_channel();
        let (events, rx) = event_channel();
        let run = tokio::spawn(Box::new(CodexDriver::new()).run(DriverContext {
            commands,
            events,
            spawner: Arc::new(Quiet(spawner)),
            setup,
        }));
        let end = timeout(Duration::from_secs(2), ends.recv())
            .await
            .unwrap()
            .unwrap();
        Self {
            tx,
            events: rx,
            end,
            run,
        }
    }
    async fn start() -> Self {
        Self::start_with(setup()).await
    }
    async fn wire(&mut self) -> Value {
        loop {
            let frame: Value = serde_json::from_str(
                &timeout(Duration::from_secs(2), self.end.from_client.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert!(frame.get("jsonrpc").is_none());
            if frame["method"] == "collaborationMode/list" {
                self.send(json!({"id":frame["id"],"result":{"data":[{"mode":"plan","name":"Plan"},{"mode":"default","name":"Default"}]}}));
            } else if frame["method"] == "account/rateLimits/read" {
                self.send(json!({"id":frame["id"],"result":{"rateLimits":{}}}));
            } else {
                return frame;
            }
        }
    }
    fn send(&self, v: Value) {
        self.end
            .to_client
            .send(FakeLine::Line(v.to_string()))
            .unwrap();
    }
    async fn event(&mut self) -> DriverEvent {
        timeout(Duration::from_secs(2), self.events.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn ready(&mut self) -> crate::driver::ReadyInfo {
        let init = self.wire().await;
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["params"]["clientInfo"]["name"], "ghost");
        self.send(json!({"id":init["id"],"result":{"userAgent":"codex/0.162.1"}}));
        assert_eq!(self.wire().await["method"], "initialized");
        let open = self.wire().await;
        assert!(matches!(
            open["method"].as_str(),
            Some("thread/start" | "thread/resume")
        ));
        let models = self.wire().await;
        assert_eq!(models["method"], "model/list");
        self.send(json!({"id":models["id"],"result":{"data":[{"id":"m","model":"model-a","displayName":"Model A","defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"reasoningEffort":"medium","description":"Balanced"},{"reasoningEffort":"high","description":"High"}]}],"nextCursor":null}}));
        assert_eq!(open["params"]["approvalsReviewer"], "user");
        self.send(json!({"id":open["id"],"result":{"thread":{"id":"thread-1","sessionId":"tree-1","status":{"type":"idle"},"cliVersion":"0.162.1"},"model":"model-a","approvalPolicy":"untrusted","approvalsReviewer":"user","sandbox":{"type":"readOnly"}}}));
        loop {
            if let DriverEvent::Ready(info) = self.event().await {
                return info;
            }
        }
    }
    fn prompt(&self) {
        self.tx
            .send(DriverCommand::Prompt {
                turn: TurnRef {
                    turn_id: "aura-turn".into(),
                },
                parts: vec![PromptPart::Text {
                    text: "hello".into(),
                }],
            })
            .unwrap();
    }
    async fn begin(&mut self) {
        self.prompt();
        let request = self.wire().await;
        assert_eq!(request["method"], "turn/start");
        assert_eq!(request["params"]["approvalsReviewer"], "user");
        self.send(json!({"id":request["id"],"result":{"turn":{"id":"turn-1","status":"inProgress","items":[]}}}));
        self.send(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"inProgress","items":[]}}}));
    }
    async fn stop(mut self) {
        self.tx
            .send(DriverCommand::Shutdown {
                plan: StopPlan {
                    cancel_grace: Duration::ZERO,
                    process: ShutdownPlan::FORCE,
                },
            })
            .unwrap();
        while self.events.recv().await.is_some() {}
        assert_eq!(self.run.await.unwrap(), DriverExit::Closed);
    }
}

#[tokio::test(start_paused = true)]
async fn handshake_uses_thread_identity_and_advertises_only_implemented_capabilities() {
    let mut h = Harness::start().await;
    let ready = h.ready().await;
    assert_eq!(ready.native_session_id.as_deref(), Some("thread-1"));
    assert_eq!(ready.agent_version.as_deref(), Some("0.162.1"));
    assert!(
        ready.capabilities.steer
            && !ready.capabilities.fork
            && ready.capabilities.images
            && !ready.capabilities.add_dirs
    );
    h.prompt();
    let request = h.wire().await;
    assert_eq!(
        request["params"]["input"],
        json!([{"type":"text","text":"hello"}])
    );
    assert_eq!(request["params"]["threadId"], "thread-1");
    assert!(request["params"].get("developerInstructions").is_none());
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn unsupported_native_startup_and_rewind_are_rejected_explicitly() {
    let (spawner, mut ends) = FakeSpawner::new();
    let (_tx, commands) = mpsc::unbounded_channel();
    let (events, _rx) = event_channel();
    let mut settings = setup();
    settings.native_start.fork = true;
    let result = Box::new(CodexDriver::new())
        .run(DriverContext {
            commands,
            events,
            spawner: Arc::new(Quiet(Arc::clone(&spawner))),
            setup: settings,
        })
        .await;
    assert!(matches!(result, DriverExit::StartFailed(_)));
    assert!(spawner.controls().is_empty());
    assert!(ends.try_recv().is_err());

    let mut h = Harness::start().await;
    h.ready().await;
    let (reply, result) = tokio::sync::oneshot::channel();
    h.tx.send(DriverCommand::RewindFiles {
        user_message_id: "checkpoint".into(),
        dry_run: true,
        reply,
    })
    .unwrap();
    assert!(result.await.unwrap().is_err());
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn malformed_completion_stops_the_session_instead_of_leaving_a_stuck_turn() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    h.send(
        json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),
    );
    let mut failed = false;
    while let Some(event) = timeout(Duration::from_secs(12), h.events.recv())
        .await
        .unwrap()
    {
        failed |= matches!(
            event,
            DriverEvent::Event {
                event: GhostEvent::Error {
                    kind: ghost_protocol::ErrorKind::ProtocolError,
                    ..
                },
                ..
            }
        );
    }
    assert!(failed);
    assert!(matches!(h.run.await.unwrap(), DriverExit::Crashed { .. }));
}

#[tokio::test(start_paused = true)]
async fn quota_updates_bound_distinct_buckets_and_input_requests_require_blocking_metadata() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    h.send(json!({"id":"bad-question","method":"item/tool/requestUserInput","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","questions":[{"id":"q","header":"Q","question":"Which?"}]}}));
    let rejected = h.wire().await;
    assert_eq!(rejected["id"], "bad-question");
    assert!(rejected["error"].is_object());
    for n in 0..100 {
        h.send(json!({"method":"account/rateLimits/updated","params":{"rateLimits":{"limitId":format!("bucket-{n}"),"primary":{"usedPercent":n,"windowDurationMins":300,"resetsAt":123},"unknownMetadata":"discard me"}}}));
    }
    h.send(json!({"method":"thread/name/updated","params":{"threadId":"thread-1","threadName":"barrier"}}));
    let mut largest = 0;
    loop {
        match h.event().await {
            DriverEvent::Event {
                event: GhostEvent::LimitsUpdated { windows },
                ..
            } => {
                largest = largest.max(windows.len());
                assert!(windows.len() <= 32);
            }
            DriverEvent::Title(Some(title)) if title == "barrier" => break,
            DriverEvent::UiRequest { .. } => panic!("malformed question became user input"),
            _ => {}
        }
    }
    assert!(largest > 1);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn turn_completion_and_idle_are_separate_and_foreign_events_are_ignored() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    h.send(json!({"method":"item/agentMessage/delta","params":{"threadId":"other","turnId":"turn-1","itemId":"m","delta":"foreign"}}));
    h.send(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"m","delta":"hello"}}));
    h.send(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed","items":[]}}}));
    let mut text = String::new();
    loop {
        match h.event().await {
            DriverEvent::Event {
                event: GhostEvent::MessageDelta { text: delta, .. },
                ..
            } => text += &delta,
            DriverEvent::TurnEnded {
                turn_id,
                stop_reason,
                ..
            } => {
                assert_eq!(turn_id, "aura-turn");
                assert_eq!(stop_reason, StopReason::EndTurn);
                break;
            }
            DriverEvent::RunState { idle: true } => panic!("completion is not idle"),
            _ => {}
        }
    }
    assert_eq!(text, "hello");
    h.send(json!({"method":"thread/status/changed","params":{"threadId":"thread-1","status":{"type":"idle"}}}));
    assert!(matches!(
        h.event().await,
        DriverEvent::RunState { idle: true }
    ));
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn approval_ids_remain_exact_and_withdrawals_do_not_answer_twice() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    for id in [json!(17), json!("17")] {
        h.send(json!({"id":id,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"same-item","command":"echo test","startedAtMs":1}}));
    }
    let mut ids = Vec::new();
    while ids.len() < 2 {
        if let DriverEvent::Permission { request, .. } = h.event().await {
            ids.push(request.request_id);
        }
    }
    h.tx.send(DriverCommand::RespondPermission {
        request_id: ids[0].clone(),
        outcome: crate::driver::PermissionOutcome::Selected("accept".into()),
    })
    .unwrap();
    assert_eq!(
        h.wire().await,
        json!({"id":17,"result":{"decision":"accept"}})
    );
    h.send(json!({"method":"serverRequest/resolved","params":{"threadId":"thread-1","requestId":"17"}}));
    assert!(
        matches!(h.event().await,DriverEvent::PermissionWithdrawn {request_id} if request_id==ids[1])
    );
    h.tx.send(DriverCommand::RespondPermission {
        request_id: ids[1].clone(),
        outcome: crate::driver::PermissionOutcome::Selected("accept".into()),
    })
    .unwrap();
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn unavailable_approval_decisions_cannot_be_sent_even_by_direct_driver_commands() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    h.send(json!({"id":17,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"tool-1","command":"echo test","startedAtMs":1,"availableDecisions":["accept",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["echo"]}},"cancel"]}}));
    let request = loop {
        if let DriverEvent::Permission { request, .. } = h.event().await {
            break request;
        }
    };
    assert_eq!(
        request
            .options
            .iter()
            .map(|option| option.option_id.as_str())
            .collect::<Vec<_>>(),
        ["accept", "cancel"]
    );
    for decision in ["acceptForSession", "decline", "accept"] {
        h.tx.send(DriverCommand::RespondPermission {
            request_id: request.request_id.clone(),
            outcome: crate::driver::PermissionOutcome::Selected(decision.into()),
        })
        .unwrap();
    }
    assert_eq!(
        h.wire().await,
        json!({"id":17,"result":{"decision":"accept"}})
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn native_execution_setup_failure_remains_a_visible_failed_tool_and_completed_turn() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    let message =
        "Failed to create unified exec process: helper_unknown_error: setup refresh had errors";
    h.send(json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"exec-1","type":"commandExecution","command":"Get-Location","status":"inProgress","aggregatedOutput":null,"exitCode":null}}}));
    h.send(json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"exec-1","type":"commandExecution","command":"Get-Location","status":"failed","aggregatedOutput":message,"exitCode":-1}}}));
    h.send(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed","items":[]}}}));
    let mut saw_failure = false;
    loop {
        match h.event().await {
            DriverEvent::Event {
                event: GhostEvent::ToolUpdate(update),
                ..
            } if update.status == Some(ghost_protocol::ToolStatus::Failed) => {
                saw_failure = update.content.as_ref().is_some_and(|content| {
                    content.iter().any(|part| {
                        matches!(part, ghost_protocol::ToolContent::Terminal {
                            output, exit_code: Some(-1), ..
                        } if output.contains(message))
                    })
                });
            }
            DriverEvent::TurnEnded { stop_reason, .. } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                assert!(
                    saw_failure,
                    "a normal provider turn result must not hide a failed native tool"
                );
                break;
            }
            _ => {}
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn interrupt_waits_for_start_id_and_does_not_complete_on_acknowledgement() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    let start = h.wire().await;
    h.tx.send(DriverCommand::Interrupt).unwrap();
    h.send(json!({"id":start["id"],"result":{"turn":{"id":"turn-1","status":"inProgress","items":[]}}}));
    let interrupt = h.wire().await;
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(
        interrupt["params"],
        json!({"threadId":"thread-1","turnId":"turn-1"})
    );
    h.send(json!({"id":interrupt["id"],"result":{}}));
    h.send(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"interrupted","items":[]}}}));
    loop {
        if let DriverEvent::TurnEnded { stop_reason, .. } = h.event().await {
            assert_eq!(stop_reason, StopReason::Cancelled);
            break;
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn secret_questions_are_cancelled_without_forwarding_a_persisted_prompt() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    h.send(json!({"id":"secret","method":"item/tool/requestUserInput","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"i","isBlocking":true,"questions":[{"id":"q","header":"Secret","question":"Enter a private value","isSecret":true}]}}));
    assert_eq!(
        h.wire().await,
        json!({"id":"secret","result":{"answers":{}}})
    );
    loop {
        match h.event().await {
            DriverEvent::UiRequest { .. } => panic!("secret prompt must not be persisted"),
            DriverEvent::Event {
                event: GhostEvent::Notice { message, .. },
                ..
            } if message.starts_with("Secret input") => break,
            _ => {}
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_mcp_form_from_codex_is_shown_and_answered_with_the_mcp_result() {
    use ghost_protocol::forms::{UiFieldValue, UiValue};
    use ghost_protocol::{UiAnswer, UiPrompt};
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    h.send(
        json!({"id":"form","method":"mcpServer/elicitation/request","params":{
        "threadId":"thread-1","turnId":"turn-1","serverName":"tickets","mode":"form",
        "message":"File the ticket","requestedSchema":{"type":"object","properties":{
            "title":{"type":"string","maxLength":80},
            "priority":{"type":"integer","minimum":1,"maximum":3}
        },"required":["title"]}}}),
    );
    let request = loop {
        if let DriverEvent::UiRequest { request, turn } = h.event().await {
            assert!(turn.is_some(), "tied to the running turn");
            break request;
        }
    };
    assert!(
        matches!(&request.prompt, UiPrompt::Form { source, .. } if source.as_deref() == Some("tickets"))
    );
    h.tx.send(DriverCommand::RespondUi {
        request_id: request.request_id.clone(),
        answer: Some(UiAnswer::Form {
            accept: true,
            values: vec![
                UiFieldValue {
                    id: "title".into(),
                    value: UiValue::Text {
                        text: "Login fails".into(),
                    },
                },
                UiFieldValue {
                    id: "priority".into(),
                    value: UiValue::Number { number: 2.0 },
                },
            ],
        }),
    })
    .unwrap();
    assert_eq!(
        h.wire().await,
        json!({"id":"form","result":{"action":"accept","content":{"title":"Login fails","priority":2}}})
    );
    // Dismissing a second one cancels it, as the MCP result says.
    h.send(json!({"id":"link","method":"mcpServer/elicitation/request","params":{
        "threadId":"thread-1","turnId":null,"serverName":"tickets","mode":"url",
        "message":"Approve in the browser","url":"https://tickets.example/approve","elicitationId":"e2"}}));
    let request = loop {
        if let DriverEvent::UiRequest { request, .. } = h.event().await {
            break request;
        }
    };
    h.tx.send(DriverCommand::RespondUi {
        request_id: request.request_id,
        answer: None,
    })
    .unwrap();
    assert_eq!(
        h.wire().await,
        json!({"id":"link","result":{"action":"cancel","content":null}})
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn mcp_servers_starting_up_are_reported_as_loaded_for_this_thread_only() {
    use ghost_protocol::session::McpLoadState;
    let mut h = Harness::start().await;
    h.ready().await;
    h.send(json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":null,"name":"docs","status":"starting"}}));
    h.send(json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"other","name":"foreign","status":"failed"}}));
    h.send(json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"thread-1","name":"docs","status":"ready"}}));
    h.send(json!({"method":"thread/name/updated","params":{"threadId":"thread-1","threadName":"barrier"}}));
    let mut states = Vec::new();
    loop {
        match h.event().await {
            DriverEvent::Loaded(loaded) => states.push(
                loaded
                    .mcp_servers
                    .iter()
                    .map(|server| (server.name.clone(), server.state))
                    .collect::<Vec<_>>(),
            ),
            DriverEvent::Title(Some(title)) if title == "barrier" => break,
            _ => {}
        }
    }
    assert_eq!(
        states,
        [
            vec![("docs".to_owned(), McpLoadState::Starting)],
            vec![("docs".to_owned(), McpLoadState::Connected)],
        ]
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn setting_default_effort_clears_previous_turn_override() {
    let mut h = Harness::start().await;
    h.ready().await;
    for effort in ["high", "default"] {
        let (reply, wait) = tokio::sync::oneshot::channel();
        h.tx.send(DriverCommand::SetOption {
            option_id: "effort".into(),
            value: ghost_protocol::session::ConfigValue::Choice {
                value: effort.into(),
            },
            reply,
        })
        .unwrap();
        assert!(wait.await.unwrap().is_ok());
    }
    h.prompt();
    assert_eq!(h.wire().await["params"]["effort"], "medium");
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_missing_initialize_reply_stops_only_the_owned_process() {
    let mut h = Harness::start().await;
    h.wire().await;
    let events = async {
        let mut seen = Vec::new();
        while let Some(event) = h.events.recv().await {
            seen.push(event);
        }
        seen
    };
    let seen = timeout(Duration::from_secs(80), events)
        .await
        .expect("bounded startup and kill");
    assert!(
        !seen
            .iter()
            .any(|event| matches!(event, DriverEvent::Ready(_)))
    );
    assert!(matches!(h.run.await.unwrap(), DriverExit::StartFailed(_)));
    assert!(*h.end.killed.borrow());
}

#[tokio::test(start_paused = true)]
async fn a_dropped_receiver_force_stops_even_when_stdout_is_backpressured() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    for _ in 0..1000 {
        h.send(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"m","delta":"text"}}));
    }
    drop(h.events);
    assert_eq!(
        timeout(Duration::from_secs(2), h.run)
            .await
            .unwrap()
            .unwrap(),
        DriverExit::Closed
    );
    assert!(*h.end.killed.borrow());
}

#[tokio::test(start_paused = true)]
async fn known_process_exit_drains_buffered_content_and_result_without_a_false_stdin_error() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    for _ in 0..1000 {
        h.send(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"m","delta":"text"}}));
    }
    h.send(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed","items":[]}}}));
    h.end.to_client.send(FakeLine::Eof).unwrap();
    h.end
        .exit
        .send(ghost_process::ExitInfo {
            code: Some(0),
            signal: None,
            at_ms: 1,
        })
        .unwrap();
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(5)).await;
    let mut deltas = 0;
    let mut ends = 0;
    while let Some(event) = timeout(Duration::from_secs(12), h.events.recv())
        .await
        .unwrap()
    {
        match event {
            DriverEvent::Event {
                event: GhostEvent::MessageDelta { .. },
                ..
            } => deltas += 1,
            DriverEvent::TurnEnded {
                stop_reason: StopReason::EndTurn,
                ..
            } => ends += 1,
            DriverEvent::Event {
                event: GhostEvent::Error { message, .. },
                ..
            } => panic!("unexpected error: {message}"),
            _ => {}
        }
    }
    assert_eq!(deltas, 1000);
    assert_eq!(ends, 1);
    assert!(matches!(
        h.run.await.unwrap(),
        DriverExit::Crashed {
            exit: Some(ghost_process::ExitInfo { code: Some(0), .. }),
            ..
        }
    ));
    assert!(!*h.end.killed.borrow());
}

#[tokio::test(start_paused = true)]
async fn a_late_start_reply_cannot_finish_a_different_turn_or_change_its_identity() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    h.send(json!({"id":999,"result":{"turn":{"id":"wrong","status":"completed","items":[]}}}));
    h.send(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed","items":[]}}}));
    let mut ends = 0;
    while ends == 0 {
        if let DriverEvent::TurnEnded { turn_id, .. } = h.event().await {
            assert_eq!(turn_id, "aura-turn");
            ends += 1;
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn explicit_vendor_auth_failure_offers_native_login_without_reading_credentials() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    h.send(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"failed","items":[],"error":{"message":"Sign in","codexErrorInfo":"unauthorized"}}}}));
    loop {
        if let DriverEvent::AuthRequired(prompt) = h.event().await {
            assert_eq!(prompt.commands[0].argv, ["/bin/codex", "login"]);
            assert!(prompt.commands[0].env.is_empty());
            break;
        }
    }
    while h.events.recv().await.is_some() {}
    assert!(matches!(h.run.await.unwrap(), DriverExit::Crashed { .. }));
}

#[tokio::test(start_paused = true)]
async fn rejected_permission_mode_never_retries_or_sends_a_hidden_prompt() {
    let mut settings = setup();
    settings.autonomy = Autonomy::FullAuto;
    let mut h = Harness::start_with(settings).await;
    let init = h.wire().await;
    h.send(json!({"id":init["id"],"result":{"userAgent":"codex/0.162.1"}}));
    h.wire().await;
    let open = h.wire().await;
    assert_eq!(open["params"]["approvalPolicy"], "never");
    h.wire().await;
    h.send(
        json!({"id":open["id"],"error":{"code":-32600,"message":"Policy disallows full access"}}),
    );
    while h.events.recv().await.is_some() {}
    assert!(matches!(h.run.await.unwrap(), DriverExit::StartFailed(_)));
    while let Ok(line) = h.end.from_client.try_recv() {
        assert!(!line.contains("turn/start"));
    }
}

#[tokio::test(start_paused = true)]
async fn inherited_automatic_reviewer_cannot_masquerade_as_ask() {
    let mut h = Harness::start().await;
    let init = h.wire().await;
    h.send(json!({"id":init["id"],"result":{"userAgent":"codex/0.162.1"}}));
    h.wire().await;
    let open = h.wire().await;
    assert_eq!(open["params"]["approvalsReviewer"], "user");
    h.wire().await;
    let mut reply: Value = include_str!("../../tests/fixtures/codex/basic.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|row| row["d"] == "in" && row["msg"]["result"]["thread"].is_object())
        .unwrap()["msg"]
        .clone();
    reply["id"] = open["id"].clone();
    reply["result"]["approvalsReviewer"] = json!("auto_review");
    h.send(reply);
    while let Some(event) = h.events.recv().await {
        assert!(!matches!(event, DriverEvent::Ready(_)));
    }
    assert!(matches!(h.run.await.unwrap(), DriverExit::StartFailed(_)));
}

#[tokio::test(start_paused = true)]
async fn file_approval_names_only_its_matching_tool_and_keeps_reason_context() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    for (id, path) in [("file-one", "one.rs"), ("file-two", "two.rs")] {
        h.send(json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":id,"type":"fileChange","status":"inProgress","changes":[{"path":path,"kind":{"type":"update","move_path":null},"diff":format!("+content for {path}")}]}}}));
    }
    h.send(json!({"id":"approve-two","method":"item/fileChange/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"file-two","startedAtMs":1,"grantRoot":"/work","reason":"Requested update"}}));
    loop {
        if let DriverEvent::Permission { request, .. } = h.event().await {
            assert_eq!(request.tool_call_id.as_deref(), Some("file-two"));
            let shown = serde_json::to_string(&request.content).unwrap();
            assert!(shown.contains("two.rs") && shown.contains("+content for two.rs"));
            assert!(shown.contains("Requested update") && shown.contains("/work"));
            assert!(!shown.contains("one.rs"));
            break;
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn writer_failure_after_an_accepted_send_does_not_wait_for_request_timeout() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.end.from_client.close();
    h.prompt();
    let mut observed = false;
    timeout(Duration::from_secs(12), async {
        while let Some(event) = h.events.recv().await {
            if let DriverEvent::Event {
                event:
                    GhostEvent::Error {
                        kind: ghost_protocol::ErrorKind::AgentExited,
                        ..
                    },
                ..
            } = event
            {
                observed = true;
            }
        }
    })
    .await
    .expect("writer failure and owned shutdown are prompt");
    assert!(observed);
    assert!(*h.end.killed.borrow());
    assert!(matches!(h.run.await.unwrap(), DriverExit::Crashed { .. }));
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_callback_is_cancelled_before_turn_completion() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.begin().await;
    h.send(json!({"id":"approval","method":"item/fileChange/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"files","startedAtMs":1}}));
    loop {
        if matches!(h.event().await, DriverEvent::Permission { .. }) {
            break;
        }
    }
    h.send(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed","items":[]}}}));
    assert_eq!(
        h.wire().await,
        json!({"id":"approval","result":{"decision":"cancel"}})
    );
    assert!(matches!(
        h.event().await,
        DriverEvent::PermissionWithdrawn { .. }
    ));
    assert!(matches!(h.event().await, DriverEvent::TurnEnded { .. }));
    h.stop().await;
}

#[path = "tests/model_recovery.rs"]
mod model_recovery;

#[tokio::test(start_paused = true)]
async fn native_plan_uses_advertised_collaboration_and_builtin_instructions() {
    let mut h = Harness::start().await;
    h.ready().await;
    let list: Value = serde_json::from_str(&h.end.from_client.recv().await.unwrap()).unwrap();
    assert_eq!(list["method"], "collaborationMode/list");
    h.send(json!({"id":list["id"],"result":{"data":[{"mode":"plan","name":"Plan"},{"mode":"default","name":"Default"}]}}));
    loop {
        if matches!(h.event().await, DriverEvent::Options(ref options) if options.iter().any(|o| o.id == crate::controls::WORKFLOW))
        {
            break;
        }
    }
    let (reply, result) = tokio::sync::oneshot::channel();
    h.tx.send(DriverCommand::SetOption {
        option_id: crate::controls::WORKFLOW.into(),
        value: ghost_protocol::session::ConfigValue::Choice {
            value: "plan".into(),
        },
        reply,
    })
    .unwrap();
    result.await.unwrap().unwrap();
    h.prompt();
    let turn = h.wire().await;
    assert_eq!(turn["method"], "turn/start");
    assert_eq!(
        turn["params"]["collaborationMode"],
        json!({"mode":"plan","settings":{"model":"model-a","reasoning_effort":"medium","developer_instructions":null}})
    );
    h.stop().await;
}
