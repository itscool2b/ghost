use super::*;
use crate::driver::{DriverError, PermissionOutcome};
use ghost_process::{ExitInfo, MAX_LINE_BYTES, TopKeys};
use ghost_protocol::session::ConfigValue;
use ghost_protocol::{AutoAnswer, ErrorKind, GhostEvent};

fn initialized(mode: &str) -> Value {
    json!({"claude_code_version":"2.1.310","current_permission_mode":mode,
        "session_state":"idle","model":"haiku","models":[{
            "value":"haiku","displayName":"Haiku","supportsEffort":true,
            "supportedEffortLevels":["low","medium"]
        }],"commands":[]})
}

async fn reply(h: &Harness, request: &Value, payload: Value) {
    h.send(json!({"type":"control_response","response":{
        "subtype":"success","request_id":request["request_id"],"response":payload
    }}))
    .await;
}

async fn rejected(h: &Harness, request: &Value) {
    h.send(json!({"type":"control_response","response":{
        "subtype":"error","request_id":request["request_id"],"error":"policy rejected"
    }}))
    .await;
}

async fn start_failed(mut h: Harness) -> DriverError {
    timeout(Duration::from_secs(30), async move {
        while let Some(event) = h.events.recv().await {
            assert!(
                !matches!(event, DriverEvent::Ready(_)),
                "failed start cannot become ready"
            );
        }
        assert_eq!(
            h.end.from_client.recv().await,
            None,
            "failed start closes stdin"
        );
        let DriverExit::StartFailed(error) = h.run.await.expect("driver") else {
            panic!("expected a start failure");
        };
        error
    })
    .await
    .expect("failed driver shuts down")
}

async fn ready_event(h: &mut Harness) -> Vec<DriverEvent> {
    let mut events = Vec::new();
    loop {
        let event = h.event().await;
        let ready = matches!(event, DriverEvent::Ready(_));
        events.push(event);
        if ready {
            return events;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn initialize_rejection_and_invalid_session_states_close_without_ready() {
    let mut h = Harness::start().await;
    let initialize = h.wire().await;
    rejected(&h, &initialize).await;
    assert_eq!(start_failed(h).await.kind, ErrorKind::AgentError);

    for payload in [Value::Null, json!({}), json!({"session_state":"future"})] {
        let mut h = Harness::start().await;
        let initialize = h.wire().await;
        reply(&h, &initialize, payload).await;
        assert_eq!(start_failed(h).await.kind, ErrorKind::ProtocolMismatch);
    }
}

#[tokio::test(start_paused = true)]
async fn full_auto_refusal_only_falls_back_when_auto_edit_is_confirmed() {
    for native_mode in ["acceptEdits", "default"] {
        let mut settings = setup();
        settings.autonomy = Autonomy::FullAuto;
        let mut h = Harness::start_with(settings).await;
        let initialize = h.wire().await;
        reply(&h, &initialize, initialized(native_mode)).await;
        let change = h.wire().await;
        assert_eq!(
            change["request"],
            json!({"subtype":"set_permission_mode","mode":"bypassPermissions"})
        );
        rejected(&h, &change).await;
        if native_mode == "acceptEdits" {
            let events = ready_event(&mut h).await;
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, DriverEvent::Autonomy(state)
                if state.requested == Autonomy::FullAuto
                    && state.native_mode.as_deref() == Some("acceptEdits")
                    && state.auto_answer == AutoAnswer::Never
                    && state.note.is_some()))
            );
            h.stop().await;
        } else {
            assert_eq!(start_failed(h).await.kind, ErrorKind::AgentError);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn rejected_or_timed_out_initial_effort_never_opens_the_session() {
    for expires in [false, true] {
        let mut settings = setup();
        settings.desired.insert(
            "model".into(),
            ConfigValue::Choice {
                value: "haiku".into(),
            },
        );
        settings.desired.insert(
            "effort".into(),
            ConfigValue::Choice {
                value: "medium".into(),
            },
        );
        let mut h = Harness::start_with(settings).await;
        let initialize = h.wire().await;
        reply(&h, &initialize, initialized("default")).await;
        let effort = h.wire().await;
        assert_eq!(
            effort["request"],
            json!({"subtype":"apply_flag_settings","settings":{"effortLevel":"medium"}})
        );
        if expires {
            tokio::time::advance(Duration::from_secs(31)).await;
        } else {
            rejected(&h, &effort).await;
        }
        assert_eq!(start_failed(h).await.kind, ErrorKind::AgentError);
    }
}

#[tokio::test(start_paused = true)]
async fn stdout_eof_before_initialize_stops_the_process_without_ready() {
    let mut h = Harness::start().await;
    h.wire().await;
    h.end.to_client.send(FakeLine::Eof).expect("EOF");
    assert_eq!(start_failed(h).await.kind, ErrorKind::AgentExited);
}

#[tokio::test(start_paused = true)]
async fn closed_stdin_during_a_prompt_does_not_leave_the_turn_running_forever() {
    let mut h = Harness::start().await;
    h.ready().await;
    // Simulate EPIPE on the next write while the process and stdout remain alive. The writer
    // discovers this asynchronously, after LineSink::send has accepted the prompt.
    h.end.from_client.close();
    h.prompt();
    let stopped = timeout(Duration::from_secs(20), async {
        while h.events.recv().await.is_some() {}
    })
    .await
    .is_ok();
    if !stopped {
        h.stop().await;
        panic!(
            "a failed stdin writer must end the session without another command or stdout frame"
        );
    }
    assert!(matches!(
        h.run.await.expect("driver"),
        DriverExit::Crashed { .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn stdin_failure_still_drains_buffered_content_and_result() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    h.wire().await;
    h.end.from_client.close();
    h.tx.send(DriverCommand::Interrupt)
        .expect("write discovers EPIPE");
    loop {
        if matches!(h.event().await, DriverEvent::Event {
            event: ghost_protocol::GhostEvent::Notice { ref message, .. }, ..
        } if message.contains("stopped accepting input"))
        {
            break;
        }
    }
    h.send(json!({"type":"assistant","message":{"id":"buffered","content":[{"type":"text","text":"Already produced"}]}})).await;
    h.send(json!({"type":"result","subtype":"success","user_message_uuid":"t1"}))
        .await;
    h.end.to_client.send(FakeLine::Eof).expect("EOF");
    let mut text = String::new();
    let mut ended = 0;
    while let Some(event) = h.events.recv().await {
        match event {
            DriverEvent::Event {
                event: ghost_protocol::GhostEvent::MessageDelta { text: chunk, .. },
                ..
            } => text.push_str(&chunk),
            DriverEvent::TurnEnded { .. } => ended += 1,
            _ => {}
        }
    }
    assert_eq!(text, "Already produced");
    assert_eq!(ended, 1);
    assert!(matches!(
        h.run.await.expect("driver"),
        DriverExit::Crashed { .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn oversized_and_malformed_frames_do_not_hide_the_next_valid_result() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    h.wire().await;
    h.end
        .to_client
        .send(FakeLine::Oversized {
            bytes: MAX_LINE_BYTES as u64 + 1,
            keys: TopKeys {
                kind: Some("assistant".into()),
                ..TopKeys::default()
            },
        })
        .expect("oversized stdout");
    h.end
        .to_client
        .send(FakeLine::Line("{malformed".into()))
        .expect("malformed stdout");
    h.send(json!({"type":"result","subtype":"success","user_message_uuid":"t1"}))
        .await;
    let mut notices = 0;
    loop {
        match h.event().await {
            DriverEvent::Event {
                event: GhostEvent::Notice { .. },
                ..
            } => notices += 1,
            DriverEvent::TurnEnded {
                turn_id,
                stop_reason,
                ..
            } => {
                assert_eq!(turn_id, "t1");
                assert_eq!(stop_reason, StopReason::EndTurn);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(notices, 2);
    h.stop().await;
}

fn permission(id: &str) -> Value {
    let fixture =
        include_str!("../../../../../docs/spikes/claude-control/fixtures/turn-permission.jsonl");
    let mut frame = fixture
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|frame| {
            frame["type"] == "control_request" && frame["request"]["subtype"] == "can_use_tool"
        })
        .expect("recorded permission request");
    frame["request_id"] = json!(id);
    frame["request"]["input"] = json!({"command":format!("echo {id}"),"future":"preserve"});
    frame
}

#[tokio::test(start_paused = true)]
async fn oversized_control_and_result_frames_stop_instead_of_waiting_for_an_unanswerable_request() {
    for kind in [
        "control_request",
        "control_response",
        "control_cancel_request",
        "system",
        "result",
    ] {
        let mut h = Harness::start().await;
        h.ready().await;
        h.prompt();
        h.wire().await;
        h.end
            .to_client
            .send(FakeLine::Oversized {
                bytes: MAX_LINE_BYTES as u64 + 1,
                keys: TopKeys {
                    kind: Some(kind.into()),
                    ..Default::default()
                },
            })
            .unwrap();
        timeout(Duration::from_secs(20), async {
            while h.events.recv().await.is_some() {}
        })
        .await
        .expect("oversized required update closes the session");
        assert!(matches!(h.run.await.unwrap(), DriverExit::Crashed { .. }));
    }
}

#[tokio::test(start_paused = true)]
async fn concurrent_permissions_keep_out_of_order_answers_and_native_inputs_separate() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    h.wire().await;
    for id in ["first", "second"] {
        h.send(permission(id)).await;
    }
    let mut cards = 0;
    while cards < 2 {
        if matches!(h.event().await, DriverEvent::Permission { .. }) {
            cards += 1;
        }
    }
    for (id, choice) in [("second", "allow"), ("first", "deny")] {
        h.tx.send(DriverCommand::RespondPermission {
            request_id: id.into(),
            outcome: PermissionOutcome::Selected(choice.into()),
        })
        .expect("answer");
        let response = h.wire().await;
        assert_eq!(response["response"]["request_id"], id);
        assert_eq!(response["response"]["response"]["behavior"], choice);
        if choice == "allow" {
            assert_eq!(
                response["response"]["response"]["updatedInput"],
                permission(id)["request"]["input"]
            );
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn shutdown_denies_and_withdraws_every_open_permission_and_question_before_eof() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    h.wire().await;
    h.send(permission("permission")).await;
    h.send(
        json!({"type":"control_request","request_id":"question","request":{
            "subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{
                "question":"Which part?","header":"Scope","multiSelect":false,
                "options":[{"label":"Core"},{"label":"UI"}]
            }]}
        }}),
    )
    .await;
    let mut requests = 0;
    while requests < 2 {
        if matches!(
            h.event().await,
            DriverEvent::Permission { .. } | DriverEvent::UiRequest { .. }
        ) {
            requests += 1;
        }
    }
    h.tx.send(DriverCommand::Shutdown {
        plan: StopPlan {
            cancel_grace: Duration::ZERO,
            process: ShutdownPlan::STANDARD,
        },
    })
    .expect("shutdown");
    let mut denied = Vec::new();
    while let Some(line) = timeout(Duration::from_secs(2), h.end.from_client.recv())
        .await
        .expect("stdin closes")
    {
        let frame: Value = serde_json::from_str(&line).expect("JSON");
        if frame["type"] == "control_response" {
            assert_eq!(frame["response"]["response"]["behavior"], "deny");
            denied.push(
                frame["response"]["request_id"]
                    .as_str()
                    .expect("request ID")
                    .to_owned(),
            );
        }
    }
    denied.sort();
    assert_eq!(denied, ["permission", "question"]);
    h.end.to_client.send(FakeLine::Eof).expect("stdout EOF");
    h.end
        .exit
        .send(ExitInfo {
            code: Some(0),
            signal: None,
            at_ms: 0,
        })
        .expect("exit");
    let mut withdrawn = Vec::new();
    while let Some(event) = h.events.recv().await {
        match event {
            DriverEvent::PermissionWithdrawn { request_id }
            | DriverEvent::UiWithdrawn { request_id } => withdrawn.push(request_id),
            _ => {}
        }
    }
    withdrawn.sort();
    assert_eq!(withdrawn, denied);
    assert_eq!(h.run.await.expect("driver"), DriverExit::Closed);
}

#[tokio::test(start_paused = true)]
async fn unsupported_dialogs_reply_with_an_error_including_initialize_redelivery() {
    for redelivered in [false, true] {
        let mut h = Harness::start().await;
        let request = json!({"subtype":"request_user_dialog","dialog":{"future":"opaque"}});
        if redelivered {
            let initialize = h.wire().await;
            h.send(json!({"type":"control_response","response":{
                "subtype":"success","request_id":initialize["request_id"],"response":initialized("default"),
                "pending_user_dialog_requests":[{"request_id":"dialog","request":request}]
            }})).await;
        } else {
            h.ready().await;
            h.send(json!({"type":"control_request","request_id":"dialog","request":request}))
                .await;
        }
        let response = h.wire().await;
        assert_eq!(response["type"], "control_response");
        assert_eq!(response["response"]["request_id"], "dialog");
        assert_eq!(response["response"]["subtype"], "error");
        assert!(
            response["response"].get("response").is_none(),
            "unsupported dialogs are never answered as if accepted"
        );
        if redelivered {
            ready_event(&mut h).await;
        }
        h.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_saved_plan_restore_never_uses_the_legacy_full_auto_downgrade() {
    let mut settings = setup();
    settings.autonomy = Autonomy::FullAuto;
    settings.desired.insert(
        crate::controls::WORKFLOW.into(),
        ConfigValue::Choice {
            value: "plan".into(),
        },
    );
    let mut h = Harness::start_with(settings).await;
    let initialize = h.wire().await;
    reply(&h, &initialize, initialized("acceptEdits")).await;
    let access = h.wire().await;
    assert_eq!(
        access["request"],
        json!({"subtype":"set_permission_mode","mode":"bypassPermissions"})
    );
    reply(&h, &access, json!({"mode":"bypassPermissions"})).await;
    let change = h.wire().await;
    assert_eq!(
        change["request"],
        json!({"subtype":"set_permission_mode","mode":"plan"})
    );
    rejected(&h, &change).await;
    assert_eq!(start_failed(h).await.kind, ErrorKind::AgentError);
}

/// A chat that opens in Plan remembers Full access, which Claude has not granted in this
/// process. It is asked for first: granted, planning goes unasked; refused, every command asks.
#[tokio::test(start_paused = true)]
async fn a_chat_opening_in_plan_plans_unasked_only_once_claude_grants_full_access() {
    for granted in [true, false] {
        let mut settings = setup();
        settings.autonomy = Autonomy::FullAuto;
        settings.desired.insert(
            crate::controls::WORKFLOW.into(),
            ConfigValue::Choice {
                value: "plan".into(),
            },
        );
        let mut h = Harness::start_with(settings).await;
        let initialize = h.wire().await;
        reply(&h, &initialize, initialized("acceptEdits")).await;
        let access = h.wire().await;
        assert_eq!(
            access["request"],
            json!({"subtype":"set_permission_mode","mode":"bypassPermissions"})
        );
        if granted {
            reply(&h, &access, json!({"mode":"bypassPermissions"})).await;
        } else {
            rejected(&h, &access).await;
        }
        let plan = h.wire().await;
        assert_eq!(
            plan["request"],
            json!({"subtype":"set_permission_mode","mode":"plan"})
        );
        reply(&h, &plan, json!({"mode":"plan"})).await;
        ready_event(&mut h).await;
        h.prompt();
        h.wire().await;
        h.send(
            json!({"type":"control_request","request_id":"run","request":{
                "subtype":"can_use_tool","tool_name":"Bash","input":{"command":"git status"}
            }}),
        )
        .await;
        if granted {
            let answer = h.wire().await;
            assert_eq!(answer["response"]["request_id"], "run");
            assert_eq!(answer["response"]["response"]["behavior"], "allow");
        } else {
            loop {
                if let DriverEvent::Permission { request, .. } = h.event().await {
                    assert_eq!(request.request_id, "run");
                    break;
                }
            }
        }
        h.stop().await;
    }
}
