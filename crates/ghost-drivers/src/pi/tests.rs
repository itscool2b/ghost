use super::*;
use crate::driver::{
    AgentSpawner, DriverCommand, DriverEvent, DriverTimings, EventReceiver, SessionSetup,
    SteerResult, StopPlan, TurnRef, event_channel,
};
use ghost_process::testing::{FakeAgentEnd, FakeLine, FakeSpawner};
use ghost_process::{
    EnvMap, EnvSource, LaunchSpec, ProcessCwd, ProcessSpawner, ProtocolLog, ResolvedEnv,
    ShutdownPlan, SpawnError, SpawnSpec, SpawnedProcess,
};
use ghost_protocol::{Autonomy, GhostEvent, PromptPart, StopReason};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};

struct Quiet(Arc<FakeSpawner>);
impl AgentSpawner for Quiet {
    fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        self.0.spawn(spec, ProtocolLog::disabled())
    }
}
pub(super) fn setup() -> SessionSetup {
    SessionSetup {
        mcp_servers: Vec::new(),
        attachment_dir: None,
        attachment_writer: None,
        chat_id: "chat".into(),
        agent_id: "pi".into(),
        dialect: "pi".into(),
        folder: PathBuf::from("/work"),
        agent_folder: "/work".into(),
        add_dirs: vec![],
        launch: LaunchSpec {
            label: "pi".into(),
            program: "/bin/pi".into(),
            args: vec![],
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
        autonomy: Autonomy::FullAuto,
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
    async fn start() -> Self {
        Self::start_with(setup()).await
    }
    async fn start_with(setup: SessionSetup) -> Self {
        let (spawner, mut ends) = FakeSpawner::new();
        let (tx, commands) = mpsc::unbounded_channel();
        let (events, rx) = event_channel();
        let run = tokio::spawn(Box::new(PiDriver::new()).run(DriverContext {
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
    async fn wire(&mut self) -> Value {
        serde_json::from_str(
            &timeout(Duration::from_secs(2), self.end.from_client.recv())
                .await
                .expect("wire deadline")
                .expect("wire"),
        )
        .unwrap()
    }
    fn send(&self, value: Value) {
        self.end
            .to_client
            .send(FakeLine::Line(value.to_string()))
            .unwrap();
    }
    async fn event(&mut self) -> DriverEvent {
        timeout(Duration::from_secs(2), self.events.recv())
            .await
            .expect("event deadline")
            .unwrap()
    }
    fn reply(&self, request: &Value, data: Value) {
        self.send(json!({"type":"response","id":request["id"],"command":request["type"],"success":true,"data":data}));
    }
    fn id(&self) -> String {
        let args = &self.end.spec.launch.args;
        args.iter()
            .position(|s| s == "--session-id")
            .map(|n| args[n + 1].to_string_lossy().into_owned())
            .unwrap_or_else(|| "resumed".into())
    }
    fn state(&self) -> Value {
        json!({"sessionId":self.id(),"model":{"provider":"test","id":"model","name":"Model"},
        "thinkingLevel":"off","isStreaming":false,"isCompacting":false,"pendingMessageCount":0,"autoCompactionEnabled":true})
    }
    async fn answer_query(&mut self) {
        let query = self.wire().await;
        self.answer(&query);
    }
    fn answer(&self, query: &Value) {
        let data = match query["type"].as_str().unwrap() {
            "get_state" => self.state(),
            "get_available_models" => {
                json!({"models":[{"provider":"test","id":"model","name":"Model"}]})
            }
            "get_available_thinking_levels" => json!({"levels":["off","low","high"]}),
            "get_commands" => json!({"commands":[]}),
            "get_session_stats" => {
                json!({"tokens":{"input":0,"output":0},"cost":0.0,"contextUsage":{"tokens":0,"contextWindow":100000}})
            }
            other => panic!("unexpected query {other}"),
        };
        self.reply(query, data);
    }
    async fn ready(&mut self) {
        for _ in 0..5 {
            self.answer_query().await;
        }
        loop {
            if let DriverEvent::Ready(info) = self.event().await {
                assert_eq!(info.native_session_id, Some(self.id()));
                assert!(
                    info.capabilities.fork
                        && info.capabilities.steer
                        && !info.capabilities.permissions
                );
                break;
            }
        }
    }
    fn prompt(&self) {
        self.tx
            .send(DriverCommand::Prompt {
                turn: TurnRef {
                    turn_id: "turn".into(),
                },
                parts: vec![PromptPart::Text {
                    text: "hello".into(),
                }],
            })
            .unwrap();
    }
    /// A prompt Pi accepted and began, with output that proves the driver saw the run start.
    async fn running(&mut self) {
        self.prompt();
        let prompt = self.wire().await;
        self.reply(&prompt, json!({"disposition":"started"}));
        self.send(json!({"type":"agent_start"}));
        self.send(json!({"type":"message_start","message":{"role":"assistant"}}));
        self.send(json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Working"}}));
        loop {
            if let DriverEvent::Event {
                event: GhostEvent::MessageDelta { .. },
                ..
            } = self.event().await
            {
                break;
            }
        }
    }
    fn steer(&self, turn: &str) -> oneshot::Receiver<SteerResult> {
        let (reply, receive) = oneshot::channel();
        self.tx
            .send(DriverCommand::Steer {
                turn_id: turn.into(),
                message_id: "m1".into(),
                parts: vec![PromptPart::Text {
                    text: "also check the tests".into(),
                }],
                reply,
            })
            .unwrap();
        receive
    }
    /// Answers the refresh queries Pi's driver sends after a turn, until `kind` arrives.
    async fn wire_until(&mut self, kind: &str) -> Value {
        loop {
            let frame = self.wire().await;
            if frame["type"] == kind {
                return frame;
            }
            self.answer(&frame);
        }
    }
    async fn notice(&mut self) -> String {
        loop {
            if let DriverEvent::Event {
                event: GhostEvent::Notice { message, .. },
                ..
            } = self.event().await
            {
                return message;
            }
        }
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
async fn prompt_acceptance_and_agent_end_are_not_completion() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    let prompt = h.wire().await;
    h.reply(&prompt, json!({"disposition":"started"}));
    h.send(json!({"type":"agent_start"}));
    h.send(json!({"type":"agent_end","messages":[],"willRetry":true}));
    h.send(json!({"type":"agent_settled"}));
    let mut ends = 0;
    loop {
        match h.event().await {
            DriverEvent::TurnEnded { stop_reason, .. } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                ends += 1;
            }
            DriverEvent::RunState { idle: true } => break,
            _ => {}
        }
    }
    assert_eq!(ends, 1);
    h.stop().await;
}
#[tokio::test(start_paused = true)]
async fn a_steer_reaches_the_running_turn_and_counts_once_pi_queues_it() {
    let mut h = Harness::start().await;
    h.ready().await;
    // Nothing is running yet, and nothing is sent.
    assert!(matches!(
        h.steer("turn").await.unwrap(),
        SteerResult::Rejected { .. }
    ));
    h.running().await;
    assert!(matches!(
        h.steer("another").await.unwrap(),
        SteerResult::Rejected { .. }
    ));
    let reply = h.steer("turn");
    let steer = h.wire().await;
    assert_eq!(steer["type"], "steer");
    assert_eq!(steer["message"], "also check the tests");
    h.send(json!({"type":"queue_update","steering":["also check the tests"],"followUp":[]}));
    h.reply(&steer, json!({"disposition":"queued"}));
    assert_eq!(reply.await.unwrap(), SteerResult::Accepted);
    // Pi reads it before its next model call, then settles: nothing is left to clear.
    h.send(json!({"type":"queue_update","steering":[],"followUp":[]}));
    h.send(json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"also check the tests"}]}}));
    h.send(json!({"type":"agent_settled"}));
    loop {
        if let DriverEvent::TurnEnded { stop_reason, .. } = h.event().await {
            assert_eq!(stop_reason, StopReason::EndTurn);
            break;
        }
    }
    for _ in 0..4 {
        let query = h.wire().await;
        assert_ne!(query["type"], "clear_queue");
        h.answer(&query);
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn pi_refusing_a_steer_is_reported_with_its_reason() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.running().await;
    let reply = h.steer("turn");
    let steer = h.wire().await;
    h.send(json!({"type":"response","id":steer["id"],"command":"steer","success":false,"error":"Extension command \"/deploy\" cannot be queued."}));
    assert_eq!(
        reply.await.unwrap(),
        SteerResult::Rejected {
            message: "Extension command \"/deploy\" cannot be queued.".into()
        }
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_steer_pi_takes_after_the_turn_settled_is_cleared_and_not_delivered() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.running().await;
    let reply = h.steer("turn");
    let steer = h.wire().await;
    // Pi settled before it read the steer, then queued it with nothing left to read it.
    h.send(json!({"type":"agent_settled"}));
    h.send(json!({"type":"queue_update","steering":["also check the tests"],"followUp":[]}));
    h.reply(&steer, json!({"disposition":"queued"}));
    assert!(matches!(reply.await.unwrap(), SteerResult::Rejected { .. }));
    let clear = h.wire_until("clear_queue").await;
    // The next prompt waits until Pi confirms the queue is empty.
    h.prompt();
    let mut early = Vec::new();
    while let Ok(Some(line)) = timeout(Duration::from_millis(5), h.end.from_client.recv()).await {
        early.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    assert!(early.iter().all(|frame| frame["type"] != "prompt"));
    for frame in &early {
        h.answer(frame);
    }
    h.reply(
        &clear,
        json!({"steering":["also check the tests"],"followUp":[]}),
    );
    assert_eq!(h.wire_until("prompt").await["message"], "hello");
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_steer_still_queued_when_pi_settles_is_cleared_with_a_notice() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.running().await;
    let reply = h.steer("turn");
    let steer = h.wire().await;
    h.send(json!({"type":"queue_update","steering":["also check the tests"],"followUp":[]}));
    h.reply(&steer, json!({"disposition":"queued"}));
    assert_eq!(reply.await.unwrap(), SteerResult::Accepted);
    h.send(json!({"type":"agent_settled"}));
    let clear = h.wire_until("clear_queue").await;
    h.reply(
        &clear,
        json!({"steering":["also check the tests"],"followUp":[]}),
    );
    assert!(h.notice().await.contains("was not delivered"));
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn stopping_says_when_pi_had_not_read_a_steer() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.running().await;
    let reply = h.steer("turn");
    let steer = h.wire().await;
    h.send(json!({"type":"queue_update","steering":["also check the tests"],"followUp":[]}));
    h.reply(&steer, json!({"disposition":"queued"}));
    assert_eq!(reply.await.unwrap(), SteerResult::Accepted);
    h.tx.send(DriverCommand::Interrupt).unwrap();
    let clear = h.wire().await;
    assert_eq!(clear["type"], "clear_queue");
    // A stop never steers.
    assert!(matches!(
        h.steer("turn").await.unwrap(),
        SteerResult::Rejected { .. }
    ));
    h.reply(
        &clear,
        json!({"steering":["also check the tests"],"followUp":[]}),
    );
    assert!(h.notice().await.contains("stopping dropped it"));
    assert_eq!(h.wire().await["type"], "abort");
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn missing_mcp_extension_api_omits_only_optional_memory_tools_without_failing_session() {
    let mut setup = setup();
    setup.mcp_servers = vec![crate::mcp::tests::memory()];
    let mut h = Harness::start_with(setup).await;
    h.ready().await;
    h.stop().await;
}
#[tokio::test(start_paused = true)]
async fn missing_mcp_extension_api_refuses_real_shared_definitions_before_ready() {
    let mut setup = setup();
    setup.mcp_servers = vec![crate::mcp::tests::server()];
    let mut h = Harness::start_with(setup).await;
    for _ in 0..4 {
        h.answer_query().await;
    }
    let mut ready = false;
    while let Some(event) = h.events.recv().await {
        ready |= matches!(event, DriverEvent::Ready(_));
    }
    assert!(!ready);
    assert!(matches!(h.run.await.unwrap(), DriverExit::StartFailed(_)));
}
#[tokio::test(start_paused = true)]
async fn interrupt_clears_queue_before_abort_and_withdraws_dialogs() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    let prompt = h.wire().await;
    h.reply(&prompt, json!({"disposition":"started"}));
    h.send(json!({"type":"agent_start"}));
    h.send(json!({"type":"extension_ui_request","id":"ask","method":"confirm","title":"Approve","message":"Run extension?"}));
    loop {
        if matches!(h.event().await, DriverEvent::UiRequest { .. }) {
            break;
        }
    }
    h.tx.send(DriverCommand::Interrupt).unwrap();
    let cancelled = h.wire().await;
    assert_eq!(cancelled["cancelled"], true);
    let clear = h.wire().await;
    assert_eq!(clear["type"], "clear_queue");
    h.reply(&clear, json!({"steering":[],"followUp":[]}));
    let abort = h.wire().await;
    assert_eq!(abort["type"], "abort");
    // The actual native settlement has no aborted field and can precede the abort reply.
    h.send(json!({"type":"agent_settled"}));
    h.reply(&abort, Value::Null);
    let mut withdrawn = false;
    loop {
        match h.event().await {
            DriverEvent::UiWithdrawn { .. } => withdrawn = true,
            DriverEvent::TurnEnded { stop_reason, .. } => {
                assert!(withdrawn);
                assert_eq!(stop_reason, StopReason::Cancelled);
                break;
            }
            _ => {}
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn native_assistant_abort_is_preserved_at_the_empty_settled_boundary() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    let prompt = h.wire().await;
    h.reply(&prompt, json!({"disposition":"started"}));
    h.send(json!({"type":"agent_start"}));
    h.send(json!({"type":"message_end","message":{"role":"assistant","content":[],"stopReason":"aborted"}}));
    h.send(json!({"type":"agent_settled"}));
    loop {
        if let DriverEvent::TurnEnded { stop_reason, .. } = h.event().await {
            assert_eq!(stop_reason, StopReason::Cancelled);
            break;
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn manual_extension_compaction_returns_idle_without_agent_settled() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.send(json!({"type":"compaction_start","reason":"manual"}));
    loop {
        if matches!(h.event().await, DriverEvent::RunState { idle: false }) {
            break;
        }
    }
    h.send(json!({"type":"compaction_end","reason":"manual","aborted":false,"willRetry":false}));
    loop {
        let event = h.event().await;
        assert!(!matches!(event, DriverEvent::TurnEnded { .. }));
        if matches!(event, DriverEvent::RunState { idle: true }) {
            break;
        }
    }
    let stats = h.wire().await;
    assert_eq!(stats["type"], "get_session_stats");
    h.reply(
        &stats,
        json!({"cost":0,"contextUsage":{"tokens":null,"contextWindow":100000}}),
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn delayed_initial_state_does_not_overwrite_newer_run_events() {
    for was_streaming in [false, true] {
        let mut h = Harness::start().await;
        let mut queries = Vec::new();
        for _ in 0..5 {
            queries.push(h.wire().await);
        }
        let mut state = h.state();
        state["isStreaming"] = json!(was_streaming);
        h.send(json!({"type":"agent_start"}));
        if was_streaming {
            h.send(json!({"type":"agent_settled"}));
        }
        for query in queries {
            let data = match query["type"].as_str().unwrap() {
                "get_state" => state.clone(),
                "get_available_models" => json!({"models":[]}),
                "get_available_thinking_levels" => json!({"levels":["off"]}),
                "get_commands" => json!({"commands":[]}),
                "get_session_stats" => json!({"cost":0}),
                other => panic!("unexpected query {other}"),
            };
            h.reply(&query, data);
        }
        let mut last_idle = None;
        loop {
            match h.event().await {
                DriverEvent::RunState { idle } => last_idle = Some(idle),
                DriverEvent::Ready(_) => break,
                _ => {}
            }
        }
        assert_eq!(last_idle, Some(was_streaming));
        h.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn zero_dialog_timeout_remains_answerable_like_native_no_timeout() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.send(
        json!({"type":"extension_ui_request","id":"q","method":"input","title":"Name","timeout":0}),
    );
    loop {
        if matches!(h.event().await, DriverEvent::UiRequest { .. }) {
            break;
        }
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    h.tx.send(DriverCommand::RespondUi {
        request_id: "q".into(),
        answer: Some(ghost_protocol::UiAnswer::Input {
            text: "still valid".into(),
        }),
    })
    .unwrap();
    let response = h.wire().await;
    assert_eq!(
        response,
        json!({"type":"extension_ui_response","id":"q","value":"still valid"})
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn automatic_compaction_keeps_the_agent_busy_until_settlement() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    let prompt = h.wire().await;
    h.reply(&prompt, json!({"disposition":"started"}));
    h.send(json!({"type":"agent_start"}));
    h.send(json!({"type":"compaction_start","reason":"overflow"}));
    h.send(json!({"type":"compaction_end","reason":"overflow","aborted":false,"willRetry":true}));
    h.send(json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"continued"}}));
    loop {
        let event = h.event().await;
        assert!(!matches!(
            event,
            DriverEvent::RunState { idle: true } | DriverEvent::TurnEnded { .. }
        ));
        if matches!(
            event,
            DriverEvent::Event {
                event: ghost_protocol::GhostEvent::MessageDelta { .. },
                ..
            }
        ) {
            break;
        }
    }
    h.send(json!({"type":"agent_settled"}));
    loop {
        if matches!(h.event().await, DriverEvent::RunState { idle: true }) {
            break;
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn handled_prompt_finishes_without_an_agent_run() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    let prompt = h.wire().await;
    h.reply(&prompt, json!({"disposition":"handled"}));
    loop {
        if let DriverEvent::TurnEnded {
            turn_id,
            stop_reason,
            ..
        } = h.event().await
        {
            assert_eq!(turn_id, "turn");
            assert_eq!(stop_reason, StopReason::EndTurn);
            break;
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn applied_settings_require_authoritative_reads_before_queued_prompt() {
    use ghost_protocol::session::ConfigValue;
    let mut h = Harness::start().await;
    h.ready().await;
    let (reply, mut receive) = tokio::sync::oneshot::channel();
    h.tx.send(DriverCommand::SetOption {
        option_id: "effort".into(),
        value: ConfigValue::Choice {
            value: "high".into(),
        },
        reply,
    })
    .unwrap();
    let setting = h.wire().await;
    assert_eq!(setting["type"], "set_thinking_level");
    h.prompt();
    h.reply(&setting, Value::Null);
    let state = h.wire().await;
    assert_eq!(state["type"], "get_state");
    let levels = h.wire().await;
    assert_eq!(levels["type"], "get_available_thinking_levels");
    h.reply(&state, h.state());
    assert!(
        timeout(Duration::from_millis(10), &mut receive)
            .await
            .is_err()
    );
    assert!(h.end.from_client.try_recv().is_err());
    h.reply(&levels, json!({"levels":["off","low","high"]}));
    assert!(receive.await.unwrap().is_ok());
    assert_eq!(h.wire().await["type"], "prompt");
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn sent_setting_timeout_closes_and_never_releases_queued_prompt() {
    use ghost_protocol::session::ConfigValue;
    let mut h = Harness::start().await;
    h.ready().await;
    let (reply, receive) = tokio::sync::oneshot::channel();
    h.tx.send(DriverCommand::SetOption {
        option_id: "effort".into(),
        value: ConfigValue::Choice {
            value: "high".into(),
        },
        reply,
    })
    .unwrap();
    assert_eq!(h.wire().await["type"], "set_thinking_level");
    h.prompt();
    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(receive.await.unwrap().is_err());
    while h.events.recv().await.is_some() {}
    assert!(matches!(h.run.await.unwrap(), DriverExit::Crashed { .. }));
    while let Ok(line) = h.end.from_client.try_recv() {
        assert_ne!(
            serde_json::from_str::<Value>(&line).unwrap()["type"],
            "prompt"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn extension_dialog_timeout_withdraws_and_late_answer_is_ignored() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.send(json!({"type":"extension_ui_request","id":"q","method":"input","title":"Name","timeout":100}));
    loop {
        if matches!(h.event().await, DriverEvent::UiRequest { .. }) {
            break;
        }
    }
    tokio::time::advance(Duration::from_millis(101)).await;
    let cancel = h.wire().await;
    assert_eq!(cancel["cancelled"], true);
    loop {
        if matches!(h.event().await, DriverEvent::UiWithdrawn { .. }) {
            break;
        }
    }
    h.tx.send(DriverCommand::RespondUi {
        request_id: "q".into(),
        answer: Some(ghost_protocol::UiAnswer::Input {
            text: "late".into(),
        }),
    })
    .unwrap();
    assert!(
        timeout(Duration::from_millis(10), h.end.from_client.recv())
            .await
            .is_err()
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn oversized_dialog_is_cancelled_without_a_partial_question() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.send(json!({"type":"extension_ui_request","id":"oversized","method":"select",
        "title":"Choose","options":[format!("{} allow", "x".repeat(64)), format!("{} deny", "x".repeat(64))]}));
    assert_eq!(
        h.wire().await,
        json!({"type":"extension_ui_response","id":"oversized","cancelled":true})
    );
    loop {
        match h.event().await {
            DriverEvent::UiRequest { .. } => panic!("a truncated question must never be shown"),
            DriverEvent::Event {
                event: ghost_protocol::GhostEvent::Notice { message, .. },
                ..
            } if message.contains("oversized") => break,
            _ => {}
        }
    }
    h.send(
        json!({"type":"extension_ui_request","id":"small","method":"confirm","title":"Continue?"}),
    );
    loop {
        if let DriverEvent::UiRequest { request, .. } = h.event().await {
            assert_eq!(request.request_id, "small");
            break;
        }
    }
    h.tx.send(DriverCommand::RespondUi {
        request_id: "small".into(),
        answer: Some(ghost_protocol::UiAnswer::Confirm { yes: false }),
    })
    .unwrap();
    assert_eq!(
        h.wire().await,
        json!({"type":"extension_ui_response","id":"small","confirmed":false})
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn malformed_initial_state_fails_without_ready() {
    let mut h = Harness::start().await;
    let query = h.wire().await;
    h.reply(&query, json!({"sessionId":h.id()}));
    while let Some(event) = h.events.recv().await {
        assert!(!matches!(event, DriverEvent::Ready(_)));
    }
    assert!(matches!(h.run.await.unwrap(), DriverExit::StartFailed(_)));
}

#[tokio::test(start_paused = true)]
async fn missing_startup_reply_has_a_bounded_deadline() {
    let mut h = Harness::start().await;
    tokio::time::advance(Duration::from_secs(61)).await;
    while let Some(event) = h.events.recv().await {
        assert!(!matches!(event, DriverEvent::Ready(_)));
    }
    assert!(matches!(h.run.await.unwrap(), DriverExit::StartFailed(_)));
}

#[tokio::test(start_paused = true)]
async fn fresh_starts_in_the_same_chat_have_distinct_native_ids() {
    let first = Harness::start().await;
    let second = Harness::start().await;
    assert_ne!(first.id(), second.id());
    first.stop().await;
    second.stop().await;
}

#[tokio::test(start_paused = true)]
async fn exhausted_retries_end_as_error_even_without_an_assistant_snapshot() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    let prompt = h.wire().await;
    h.reply(&prompt, json!({"disposition":"started"}));
    h.send(json!({"type":"agent_start"}));
    h.send(json!({"type":"auto_retry_end","success":false,"attempt":3,"finalError":"provider unavailable"}));
    h.send(json!({"type":"agent_settled"}));
    loop {
        if let DriverEvent::TurnEnded {
            stop_reason, error, ..
        } = h.event().await
        {
            assert_eq!(stop_reason, StopReason::Error);
            assert_eq!(
                error.unwrap().detail.as_deref(),
                Some("provider unavailable")
            );
            break;
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_dropped_setting_receiver_never_mutates_native_state() {
    use ghost_protocol::session::ConfigValue;
    let mut h = Harness::start().await;
    h.ready().await;
    let (reply, receive) = tokio::sync::oneshot::channel();
    drop(receive);
    h.tx.send(DriverCommand::SetOption {
        option_id: "effort".into(),
        value: ConfigValue::Choice {
            value: "high".into(),
        },
        reply,
    })
    .unwrap();
    h.prompt();
    assert_eq!(h.wire().await["type"], "prompt");
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn process_exit_drains_buffered_transcript_under_event_backpressure() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    let prompt = h.wire().await;
    h.reply(&prompt, json!({"disposition":"started"}));
    h.send(json!({"type":"agent_start"}));
    h.send(json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
    let chunk = "x".repeat(256);
    for _ in 0..500 {
        h.send(json!({"type":"message_update","assistantMessageEvent":{
        "type":"text_delta","contentIndex":0,"delta":chunk}}));
    }
    h.send(json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":chunk.repeat(500)}],
        "stopReason":"stop","usage":{"input":1,"output":2}}}));
    h.send(json!({"type":"agent_settled"}));
    h.end.to_client.send(FakeLine::Eof).unwrap();
    let (placeholder, _) = tokio::sync::oneshot::channel();
    let exit = std::mem::replace(&mut h.end.exit, placeholder);
    exit.send(ghost_process::ExitInfo {
        code: Some(0),
        signal: None,
        at_ms: 0,
    })
    .unwrap();
    for _ in 0..500 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(10)).await;
    let mut text = String::new();
    let mut ended = 0;
    while let Some(event) = h.events.recv().await {
        match event {
            DriverEvent::Event {
                event: ghost_protocol::GhostEvent::MessageDelta { text: delta, .. },
                ..
            } => text.push_str(&delta),
            DriverEvent::TurnEnded { .. } => ended += 1,
            _ => {}
        }
    }
    assert_eq!(text, chunk.repeat(500));
    assert_eq!(ended, 1);
    assert!(matches!(h.run.await.unwrap(), DriverExit::Crashed { .. }));
}

#[cfg(feature = "replay")]
async fn replay_events(name: &str, source: &str) -> Vec<DriverEvent> {
    let fixture = crate::replay::Fixture::parse(name, source).unwrap();
    let report = crate::replay::run(&fixture).await;
    assert!(report.mismatches.is_empty(), "{:?}", report.mismatches);
    report
        .events
        .iter()
        .cloned()
        .map(|v| serde_json::from_value(v).unwrap())
        .collect()
}

#[cfg(feature = "replay")]
#[tokio::test(start_paused = true)]
async fn pinned_protocol_fixture_replays_without_envelope_or_contract_mismatches() {
    let events = replay_events(
        "pi-basic",
        include_str!("../../../../docs/spikes/pi-rpc/fixtures/basic.jsonl"),
    )
    .await;
    let ended: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let DriverEvent::TurnEnded {
                usage, stop_reason, ..
            } = event
            {
                Some((usage, stop_reason))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(ended.len(), 1);
    assert_eq!(*ended[0].1, StopReason::EndTurn);
    assert_eq!(ended[0].0.as_ref().unwrap().input_tokens, 3);
    let text: String = events
        .iter()
        .filter_map(|event| {
            if let DriverEvent::Event {
                event: ghost_protocol::GhostEvent::MessageDelta { text, .. },
                ..
            } = event
            {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(text, "ok");
}

#[cfg(feature = "replay")]
#[tokio::test(start_paused = true)]
async fn thinking_tool_replay_preserves_one_thought_tool_and_final_result() {
    use ghost_protocol::{GhostEvent, ToolContent, ToolStatus};
    let events = replay_events(
        "pi-thinking-tool",
        include_str!("../../../../docs/spikes/pi-rpc/fixtures/thinking-tool.jsonl"),
    )
    .await;
    let mut thought = String::new();
    let mut text = String::new();
    let mut completed_thoughts = 0;
    let mut tools = 0;
    let mut final_tool = None;
    let mut turns = 0;
    for event in events {
        match event {
            DriverEvent::Event { event, .. } => match event {
                GhostEvent::ThoughtDelta { text, .. } => thought.push_str(&text),
                GhostEvent::ThoughtCompleted { .. } => completed_thoughts += 1,
                GhostEvent::MessageDelta { text: delta, .. } => text.push_str(&delta),
                GhostEvent::ToolCall(tool) => {
                    tools += 1;
                    assert_eq!(tool.tool_call_id, "tool-read-1");
                }
                GhostEvent::ToolUpdate(update) if update.status == Some(ToolStatus::Completed) => {
                    final_tool = update.content;
                }
                _ => {}
            },
            DriverEvent::TurnEnded {
                usage, stop_reason, ..
            } => {
                turns += 1;
                assert_eq!(stop_reason, StopReason::EndTurn);
                let usage = usage.unwrap();
                assert_eq!(
                    (
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cache_read_tokens
                    ),
                    (8, 4, Some(5))
                );
            }
            _ => {}
        }
    }
    assert_eq!(thought, "Inspect the file.");
    assert_eq!(completed_thoughts, 1);
    assert_eq!(text, "ok");
    assert_eq!((tools, turns), (1, 1));
    assert!(
        matches!(final_tool.as_deref(), Some([ToolContent::Text { text, .. }]) if text == "complete file")
    );
}

#[cfg(feature = "replay")]
#[tokio::test(start_paused = true)]
async fn interrupted_dialog_replay_withdraws_before_ending_and_keeps_partial_text() {
    use ghost_protocol::GhostEvent;
    let events = replay_events(
        "pi-interrupted-dialog",
        include_str!("../../../../docs/spikes/pi-rpc/fixtures/interrupted-dialog.jsonl"),
    )
    .await;
    let request = events
        .iter()
        .position(|event| matches!(event, DriverEvent::UiRequest { .. }))
        .unwrap();
    let withdrawn = events.iter().position(|event| matches!(event, DriverEvent::UiWithdrawn { request_id } if request_id == "dialog-1")).unwrap();
    let ended = events
        .iter()
        .position(|event| {
            matches!(
                event,
                DriverEvent::TurnEnded {
                    stop_reason: StopReason::Cancelled,
                    ..
                }
            )
        })
        .unwrap();
    assert!(request < withdrawn && withdrawn < ended);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, DriverEvent::TurnEnded { .. }))
            .count(),
        1
    );
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            DriverEvent::Event {
                event: GhostEvent::MessageDelta { text, .. },
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "partial");
}
