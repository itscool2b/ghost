use super::*;
use ghost_protocol::forms::{UiFieldValue, UiValue};
use ghost_protocol::{GhostEvent, UiAnswer, UiPrompt};

#[tokio::test(start_paused = true)]
async fn an_mcp_form_shows_in_claudes_order_and_goes_back_as_the_mcp_result() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    h.wire().await;
    h.send(
        json!({"type":"control_request","request_id":"e1","request":{
        "subtype":"elicitation","mcp_server_name":"deploy","message":"Where should it deploy?",
        "mode":"form","property_order":["region","confirm"],
        "requested_schema":{"type":"object","properties":{
            "confirm":{"type":"boolean","title":"Deploy now"},
            "region":{"type":"string","enum":["eu","us"],"enumNames":["Europe","United States"]}
        },"required":["region"]}}}),
    )
    .await;
    let request = loop {
        if let DriverEvent::UiRequest { request, .. } = h.event().await {
            break request;
        }
    };
    let UiPrompt::Form {
        message,
        source,
        fields,
    } = &request.prompt
    else {
        panic!("form")
    };
    assert_eq!(message, "Where should it deploy?");
    assert_eq!(source.as_deref(), Some("deploy"));
    assert_eq!(
        fields
            .iter()
            .map(|field| field.id.as_str())
            .collect::<Vec<_>>(),
        ["region", "confirm"]
    );
    // An answer without the required field keeps the form open: nothing goes back.
    h.tx.send(DriverCommand::RespondUi {
        request_id: "e1".into(),
        answer: Some(UiAnswer::Form {
            accept: true,
            values: vec![],
        }),
    })
    .unwrap();
    assert!(
        timeout(Duration::from_millis(5), h.end.from_client.recv())
            .await
            .is_err()
    );
    h.tx.send(DriverCommand::RespondUi {
        request_id: "e1".into(),
        answer: Some(UiAnswer::Form {
            accept: true,
            values: vec![
                UiFieldValue {
                    id: "region".into(),
                    value: UiValue::Choices { selected: vec![1] },
                },
                UiFieldValue {
                    id: "confirm".into(),
                    value: UiValue::Boolean { value: true },
                },
            ],
        }),
    })
    .unwrap();
    let reply = h.wire().await;
    assert_eq!(reply["response"]["request_id"], "e1");
    assert_eq!(
        reply["response"]["response"],
        json!({"action":"accept","content":{"region":"us","confirm":true}})
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_browser_step_opens_and_a_form_auracode_cannot_show_is_cancelled_with_a_notice() {
    let mut h = Harness::start().await;
    h.ready().await;
    h.send(
        json!({"type":"control_request","request_id":"u1","request":{
        "subtype":"elicitation","mcp_server_name":"github","message":"Sign in to GitHub",
        "mode":"url","url":"https://github.com/login/device","elicitation_id":"x1"}}),
    )
    .await;
    loop {
        if let DriverEvent::UiRequest { request, .. } = h.event().await {
            assert!(matches!(request.prompt, UiPrompt::Link { .. }));
            break;
        }
    }
    h.tx.send(DriverCommand::RespondUi {
        request_id: "u1".into(),
        answer: Some(UiAnswer::Confirm { yes: true }),
    })
    .unwrap();
    assert_eq!(
        h.wire().await["response"]["response"],
        json!({"action":"accept"})
    );
    h.send(
        json!({"type":"control_request","request_id":"f2","request":{
        "subtype":"elicitation","mcp_server_name":"x","message":"Nested","mode":"form",
        "requested_schema":{"type":"object","properties":{"a":{"type":"object"}}}}}),
    )
    .await;
    assert_eq!(
        h.wire().await["response"]["response"],
        json!({"action":"cancel"})
    );
    loop {
        match h.event().await {
            DriverEvent::UiRequest { .. } => panic!("an unshowable form was shown"),
            DriverEvent::Event {
                event: GhostEvent::Notice { message, .. },
                ..
            } => {
                assert!(message.contains("cannot show"), "{message}");
                break;
            }
            _ => {}
        }
    }
    h.stop().await;
}

/// Claude names what the session loaded in the first turn's init frame; the report follows it.
#[tokio::test(start_paused = true)]
async fn the_first_turns_init_reports_what_the_session_loaded() {
    use ghost_protocol::session::McpLoadState;
    let mut h = Harness::start().await;
    h.ready().await;
    h.prompt();
    h.wire().await;
    let session = h.native_id().to_owned();
    h.send(
        json!({"type":"system","subtype":"init","session_id":session,
        "mcp_servers":[{"name":"docs","status":"connected","source":"user"}],
        "skills":["deploy"],"tools":["Bash","Read"]}),
    )
    .await;
    loop {
        if let DriverEvent::Loaded(loaded) = h.event().await
            && !loaded.mcp_servers.is_empty()
        {
            assert_eq!(loaded.mcp_servers[0].name, "docs");
            assert_eq!(loaded.mcp_servers[0].state, McpLoadState::Connected);
            assert_eq!(loaded.skills, ["deploy"]);
            assert_eq!(loaded.tools, Some(2));
            break;
        }
    }
    h.stop().await;
}
