use super::*;

const ID: &str = "019a0000-0000-7000-8000-000000000011";
const OTHER: &str = "019a0000-0000-7000-8000-000000000022";

#[tokio::test(start_paused = true)]
async fn path_resume_resolves_once_and_subsequent_prompts_use_the_actual_uuid() {
    let mut settings = setup();
    settings.resume = Some("C:\\transcripts\\resumed.jsonl".into());
    let mut h = Harness::start_with(settings).await;
    h.ready().await;
    h.send(json!({"type":"system","subtype":"init","session_id":ID}))
        .await;
    loop {
        if let DriverEvent::NativeSessionId(id) = h.event().await {
            assert_eq!(id, ID);
            break;
        }
    }
    h.send(json!({"type":"system","subtype":"init","session_id":ID}))
        .await;
    h.prompt();
    assert_eq!(h.wire().await["session_id"], ID);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn early_path_resolution_is_reported_in_ready_without_a_second_identity_event() {
    let mut settings = setup();
    settings.resume = Some("/transcripts/resumed.jsonl".into());
    let mut h = Harness::start_with(settings).await;
    let init = h.wire().await;
    h.send(json!({"type":"system","subtype":"init","session_id":ID}))
        .await;
    h.send(json!({"type":"control_response","response":{"subtype":"success","request_id":init["request_id"],"response":{"session_state":"idle","current_permission_mode":"default","models":[]}}})).await;
    loop {
        match h.event().await {
            DriverEvent::Ready(info) => {
                assert_eq!(info.native_session_id.as_deref(), Some(ID));
                assert_eq!(info.resumed, crate::driver::ResumeOutcome::Resumed);
                break;
            }
            DriverEvent::NativeSessionId(_) => panic!("early identity belongs in Ready"),
            _ => {}
        }
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn requested_uuids_cannot_silently_switch_and_invalid_ids_close_the_session() {
    for kind in [
        "fresh",
        "resume",
        "fork",
        "resolved-path",
        "missing",
        "malformed",
    ] {
        let mut settings = setup();
        if kind == "resolved-path" {
            settings.resume = Some("/transcripts/resumed.jsonl".into());
        } else if kind == "resume" {
            settings.resume = Some(ID.into());
        } else {
            settings.native_start.new_session_id = Some(ID.into());
            if kind == "fork" {
                settings.resume = Some(OTHER.into());
                settings.native_start.fork = true;
            }
        }
        let mut h = Harness::start_with(settings).await;
        h.ready().await;
        if kind == "resolved-path" {
            h.send(json!({"type":"system","subtype":"init","session_id":ID}))
                .await;
            loop {
                if matches!(h.event().await, DriverEvent::NativeSessionId(_)) {
                    break;
                }
            }
        }
        let mut frame = json!({"type":"system","subtype":"init","session_id":OTHER});
        if kind == "missing" {
            frame.as_object_mut().expect("frame").remove("session_id");
        }
        if kind == "malformed" {
            frame["session_id"] = json!("invalid");
        }
        h.send(frame).await;
        let mut noticed = false;
        while let Some(event) = h.events.recv().await {
            assert!(
                !matches!(event, DriverEvent::NativeSessionId(_)),
                "mismatches cannot rename transcripts"
            );
            if let DriverEvent::Event {
                event: ghost_protocol::GhostEvent::Notice { message, .. },
                ..
            } = event
            {
                noticed |= message.contains("native session id");
            }
        }
        assert!(noticed, "identity failure must be visible");
        assert!(matches!(
            h.run.await.expect("driver task"),
            DriverExit::Crashed { .. }
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn malformed_identity_before_ready_fails_startup_without_opening_a_session() {
    let mut h = Harness::start().await;
    let _init = h.wire().await;
    h.send(json!({"type":"system","subtype":"init","session_id":"not-a-uuid"}))
        .await;
    while let Some(event) = h.events.recv().await {
        assert!(!matches!(event, DriverEvent::Ready(_)));
    }
    assert!(
        matches!(h.run.await.expect("driver task"), DriverExit::StartFailed(error) if error.kind == ghost_protocol::ErrorKind::ProtocolError)
    );
}

#[tokio::test(start_paused = true)]
async fn rejected_identity_cannot_append_its_subsequent_content_to_the_original_chat() {
    let mut settings = setup();
    settings.native_start.new_session_id = Some(ID.into());
    let mut h = Harness::start_with(settings).await;
    h.ready().await;
    h.prompt();
    h.wire().await;
    h.send(json!({"type":"system","subtype":"init","session_id":OTHER}))
        .await;
    h.send(json!({"type":"assistant","session_id":OTHER,"message":{"id":"foreign","content":[{"type":"text","text":"Wrong transcript"}]}})).await;
    h.send(
        json!({"type":"result","subtype":"success","session_id":OTHER,"user_message_uuid":"t1"}),
    )
    .await;
    h.end.to_client.send(FakeLine::Eof).expect("EOF");
    while let Some(event) = h.events.recv().await {
        assert!(
            !matches!(
                event,
                DriverEvent::Event {
                    event: ghost_protocol::GhostEvent::MessageDelta { .. },
                    ..
                } | DriverEvent::TurnEnded { .. }
            ),
            "rejected session cannot contribute transcript events"
        );
    }
    assert!(matches!(
        h.run.await.expect("driver"),
        DriverExit::Crashed { .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn uuid_resume_identity_is_canonical_even_when_the_launch_source_is_uppercase() {
    let source = "019A0000-0000-7000-8000-000000000011";
    let mut settings = setup();
    settings.resume = Some(source.into());
    let mut h = Harness::start_with(settings).await;
    let initialize = h.wire().await;
    h.send(json!({"type":"control_response","response":{"subtype":"success","request_id":initialize["request_id"],"response":{"session_state":"idle","current_permission_mode":"default","models":[]}}})).await;
    loop {
        if let DriverEvent::Ready(info) = h.event().await {
            assert_eq!(info.native_session_id.as_deref(), Some(ID));
            break;
        }
    }
    assert!(
        h.end
            .spec
            .launch
            .args
            .iter()
            .any(|arg| arg == format!("--resume={source}").as_str())
    );
    h.send(json!({"type":"system","subtype":"init","session_id":ID}))
        .await;
    h.prompt();
    assert_eq!(h.wire().await["session_id"], ID);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn subagent_init_does_not_validate_or_change_the_main_identity() {
    let mut settings = setup();
    settings.native_start.new_session_id = Some(ID.into());
    let mut h = Harness::start_with(settings).await;
    h.ready().await;
    h.send(json!({"type":"system","subtype":"init","session_id":OTHER,"parent_tool_use_id":"subagent"})).await;
    h.send(json!({"type":"system","subtype":"init","session_id":ID}))
        .await;
    h.prompt();
    assert_eq!(h.wire().await["session_id"], ID);
    h.stop().await;
}
