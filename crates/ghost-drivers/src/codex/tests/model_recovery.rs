use super::*;
use ghost_protocol::session::ConfigValue;
use ghost_protocol::{UiAnswer, UiPrompt};

fn configured(resume: bool) -> SessionSetup {
    let mut settings = setup();
    settings.autonomy = Autonomy::FullAuto;
    settings.resume = resume.then(|| "thread-1".into());
    settings.desired.insert(
        "model".into(),
        ConfigValue::Choice {
            value: "custom-or-retired".into(),
        },
    );
    settings.desired.insert(
        "effort".into(),
        ConfigValue::Choice {
            value: "high".into(),
        },
    );
    settings
}
fn model() -> Value {
    json!({"id":"m","model":"model-a","displayName":"Model A","description":"Owned test model","hidden":false,"isDefault":true,"defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"reasoningEffort":"medium","description":"Balanced"}]})
}
async fn initialized(h: &mut Harness) -> (Value, Value) {
    let init = h.wire().await;
    h.send(json!({"id":init["id"],"result":{"userAgent":"codex/0.162.1"}}));
    assert_eq!(h.wire().await["method"], "initialized");
    let open = h.wire().await;
    let models = h.wire().await;
    assert_eq!(models["method"], "model/list");
    (open, models)
}
fn opened(h: &Harness, open: &Value, chosen: &str) {
    h.send(json!({"id":open["id"],"result":{"thread":{"id":"thread-1","status":{"type":"idle"}},"model":chosen,"approvalPolicy":"never","approvalsReviewer":"user","sandbox":{"type":"dangerFullAccess"}}}));
}
async fn card(h: &mut Harness) -> String {
    loop {
        match h.event().await {
            DriverEvent::UiRequest {
                turn: None,
                request,
            } => {
                let UiPrompt::Select { message, options } = request.prompt else {
                    panic!("select recovery")
                };
                assert!(message.contains("could not open"));
                assert!(message.contains("original startup error"));
                assert!(!message.contains("model was removed"));
                assert_eq!(options.len(), 2);
                return request.request_id;
            }
            DriverEvent::Ready(_) => panic!("Ready before explicit recovery"),
            _ => {}
        }
    }
}
fn respond(h: &Harness, request_id: &str, index: Option<u32>) {
    h.tx.send(DriverCommand::RespondUi {
        request_id: request_id.into(),
        answer: index.map(|index| UiAnswer::Select { index }),
    })
    .unwrap();
}
async fn failed_open(h: &mut Harness) -> String {
    let (open, models) = initialized(h).await;
    h.send(json!({"id":open["id"],"error":{"code":-32600,"message":"original startup error"}}));
    h.send(json!({"id":models["id"],"result":{"data":[model()],"nextCursor":"more"}}));
    let next = h.wire().await;
    assert_eq!(next["method"], "model/list");
    assert_eq!(next["params"]["cursor"], "more");
    h.send(json!({"id":next["id"],"result":{"data":[],"nextCursor":null}}));
    card(h).await
}

#[tokio::test(start_paused = true)]
async fn startup_recovery_requires_user_choice_in_full_auto_and_opens_same_resume_once() {
    for resume in [false, true] {
        let mut h = Harness::start_with(configured(resume)).await;
        h.prompt(); // An early visible prompt must stay held, never be retried by recovery.
        let request = failed_open(&mut h).await;
        tokio::time::advance(Duration::from_secs(120)).await;
        tokio::task::yield_now().await;
        assert!(
            h.end.from_client.try_recv().is_err(),
            "no automatic open or prompt"
        );
        respond(&h, &request, Some(0));
        let retry = h.wire().await;
        assert_eq!(
            retry["method"],
            if resume {
                "thread/resume"
            } else {
                "thread/start"
            }
        );
        assert_eq!(retry["params"]["model"], "model-a");
        assert_eq!(retry["params"]["approvalPolicy"], "never");
        if resume {
            assert_eq!(retry["params"]["threadId"], "thread-1");
        }
        opened(&h, &retry, "model-a");
        let mut notice = false;
        loop {
            match h.event().await {
                DriverEvent::UiWithdrawn { .. } => panic!("a user answer is not withdrawn"),
                DriverEvent::Event {
                    event: GhostEvent::Notice { message, .. },
                    ..
                } => notice |= message.contains("You chose Codex's advertised default"),
                DriverEvent::Ready(info) => {
                    assert_eq!(info.native_session_id.as_deref(), Some("thread-1"));
                    break;
                }
                _ => {}
            }
        }
        assert!(notice);
        let prompt = h.wire().await;
        assert_eq!(prompt["method"], "turn/start");
        assert_eq!(prompt["params"]["model"], "model-a");
        assert_eq!(prompt["params"]["effort"], "medium");
        assert_eq!(prompt["params"]["input"][0]["text"], "hello");
        assert!(h.end.from_client.try_recv().is_err());
        h.stop().await;
    }
}
#[tokio::test(start_paused = true)]
async fn cancelling_startup_recovery_preserves_original_failure_without_retry() {
    for choice in [None, Some(1)] {
        let mut h = Harness::start_with(configured(true)).await;
        let request = failed_open(&mut h).await;
        respond(&h, &request, choice);
        while let Some(event) = h.events.recv().await {
            assert!(
                !matches!(event, DriverEvent::UiWithdrawn { .. }),
                "a user cancellation is already resolved"
            );
        }
        let DriverExit::StartFailed(error) = h.run.await.unwrap() else {
            panic!("failed startup")
        };
        assert_eq!(error.message, "original startup error");
        while let Ok(line) = h.end.from_client.try_recv() {
            assert!(!line.contains("thread/resume") && !line.contains("turn/start"));
        }
    }
}
#[tokio::test(start_paused = true)]
async fn successful_unlisted_custom_alias_is_never_replaced() {
    let mut h = Harness::start_with(configured(true)).await;
    let (open, models) = initialized(&mut h).await;
    assert_eq!(open["params"]["model"], "custom-or-retired");
    h.send(json!({"id":models["id"],"result":{"data":[model()],"nextCursor":null}}));
    opened(&h, &open, "custom-or-retired");
    loop {
        match h.event().await {
            DriverEvent::UiRequest { .. } => panic!("valid alias must not require recovery"),
            DriverEvent::Ready(_) => break,
            _ => {}
        }
    }
    h.stop().await;
}
#[tokio::test(start_paused = true)]
async fn failed_recovery_is_not_offered_or_retried_again() {
    let mut h = Harness::start_with(configured(true)).await;
    let request = failed_open(&mut h).await;
    respond(&h, &request, Some(0));
    let retry = h.wire().await;
    h.send(json!({"id":retry["id"],"error":{"code":-32603,"message":"still cannot open"}}));
    while let Some(event) = h.events.recv().await {
        assert!(!matches!(event, DriverEvent::UiRequest { .. }));
    }
    let DriverExit::StartFailed(error) = h.run.await.unwrap() else {
        panic!("failed startup")
    };
    assert_eq!(error.message, "still cannot open");
}
#[tokio::test(start_paused = true)]
async fn auth_errors_and_incomplete_or_ambiguous_model_lists_never_offer_recovery() {
    for case in 0..4 {
        let mut h = Harness::start_with(configured(true)).await;
        let (open, models) = initialized(&mut h).await;
        if case == 0 {
            h.send(json!({"id":models["id"],"result":{"data":[model()],"nextCursor":null}}));
            h.send(json!({"id":open["id"],"error":{"code":-32600,"message":"sign in","data":{"codexErrorInfo":"unauthorized"}}}));
        } else {
            h.send(
                json!({"id":open["id"],"error":{"code":-32603,"message":"original startup error"}}),
            );
            if case == 1 {
                h.send(json!({"id":models["id"],"error":{"code":-32603,"message":"network unavailable"}}));
            }
            if case == 2 {
                h.send(json!({"id":models["id"],"result":{"data":[],"nextCursor":null}}));
            }
            if case == 3 {
                let mut other = model();
                other["id"] = json!("m2");
                other["model"] = json!("model-b");
                h.send(
                    json!({"id":models["id"],"result":{"data":[model(),other],"nextCursor":null}}),
                );
            }
        }
        while let Some(event) = h.events.recv().await {
            assert!(!matches!(
                event,
                DriverEvent::UiRequest { .. } | DriverEvent::Ready(_)
            ));
        }
        assert!(matches!(h.run.await.unwrap(), DriverExit::StartFailed(_)));
    }
}
#[tokio::test(start_paused = true)]
async fn stop_closes_pending_startup_recovery_and_duplicate_answers_cannot_retry() {
    let mut h = Harness::start_with(configured(true)).await;
    let request = failed_open(&mut h).await;
    respond(&h, "wrong-request", Some(0));
    tokio::task::yield_now().await;
    assert!(h.end.from_client.try_recv().is_err());
    h.tx.send(DriverCommand::Shutdown {
        plan: StopPlan {
            cancel_grace: Duration::ZERO,
            process: ShutdownPlan::FORCE,
        },
    })
    .unwrap();
    let mut withdrawn = false;
    while let Some(event) = h.events.recv().await {
        if let DriverEvent::UiWithdrawn { request_id } = event {
            assert_eq!(request_id, request);
            withdrawn = true;
        }
    }
    assert!(
        withdrawn,
        "Stop withdraws the still-unanswered recovery card"
    );
    assert_eq!(h.run.await.unwrap(), DriverExit::Closed);
    let mut h = Harness::start_with(configured(true)).await;
    let request2 = failed_open(&mut h).await;
    respond(&h, &request2, Some(0));
    let retry = h.wire().await;
    respond(&h, &request2, Some(0));
    opened(&h, &retry, "model-a");
    loop {
        if matches!(h.event().await, DriverEvent::Ready(_)) {
            break;
        }
    }
    h.stop().await;
}
