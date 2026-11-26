//! Native metadata evidence and regressions. All sessions use fake pipes.
use super::super::tests::Harness;
use super::*;
use ghost_protocol::session::{ConfigKind, ConfigOption};

fn real_frames(source: &str) -> impl Iterator<Item = Value> + '_ {
    source
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
}
fn real_initialize() -> Value {
    real_frames(include_str!(
        "../../../../docs/spikes/claude-control/fixtures/turn-basic.jsonl"
    ))
    .find(|frame| frame["response"]["response"]["models"].is_array())
    .expect("recorded initialize")["response"]["response"]
        .clone()
}
async fn opened(payload: Value, early: Option<Value>) -> (Harness, Vec<ConfigOption>) {
    let (harness, options, _) = opened_with(super::super::tests::setup(), payload, early).await;
    (harness, options)
}
/// Opens a session with `setup`; answers the options and the notices sent before Ready.
async fn opened_with(
    setup: crate::driver::SessionSetup,
    payload: Value,
    early: Option<Value>,
) -> (Harness, Vec<ConfigOption>, Vec<String>) {
    let mut harness = Harness::start_with(setup).await;
    let initialize = harness.wire().await;
    if let Some(frame) = early {
        send_metadata(&harness, frame).await;
    }
    harness.send(json!({"type":"control_response","response":{"subtype":"success","request_id":initialize["request_id"],"response":payload}})).await;
    let mut options = Vec::new();
    let mut notices = Vec::new();
    loop {
        match harness.event().await {
            DriverEvent::Options(next) => options = next,
            DriverEvent::Event {
                event: GhostEvent::Notice { message, .. },
                ..
            } => notices.push(message),
            DriverEvent::Ready(_) => break,
            _ => {}
        }
    }
    (harness, options, notices)
}
/// Chooses `model`; Claude answers with the id it resolves to. Answers the options sent after.
async fn choose_model(harness: &mut Harness, model: &str, resolved: &str) -> Vec<ConfigOption> {
    let (reply, receive) = oneshot::channel();
    harness
        .tx
        .send(DriverCommand::SetOption {
            option_id: "model".into(),
            value: ConfigValue::Choice {
                value: model.into(),
            },
            reply,
        })
        .expect("set model");
    let request = harness.wire().await;
    assert_eq!(
        request["request"],
        json!({"subtype":"set_model","model":model})
    );
    harness.send(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{"model":resolved}}})).await;
    assert!(receive.await.expect("model reply").is_ok());
    next_options(harness).await
}
async fn send_metadata(harness: &Harness, mut frame: Value) {
    if frame["subtype"] == "init" {
        let id = harness
            .end
            .spec
            .launch
            .args
            .iter()
            .find_map(|arg| arg.to_str()?.strip_prefix("--session-id="))
            .expect("fake native id");
        frame["session_id"] = json!(id);
    }
    harness.send(frame).await;
}
async fn next_options(harness: &mut Harness) -> Vec<ConfigOption> {
    loop {
        if let DriverEvent::Options(options) = harness.event().await {
            return options;
        }
    }
}
fn current<'a>(options: &'a [ConfigOption], id: &str) -> Option<&'a str> {
    options.iter().find_map(|option| {
        if option.id != id {
            return None;
        }
        let ConfigKind::Select { current, choices } = &option.kind else {
            panic!("select option")
        };
        assert!(
            choices.iter().any(|choice| choice.value == *current),
            "current must be offered"
        );
        Some(current.as_str())
    })
}
async fn set_effort(harness: &mut Harness, effort: &str) {
    let (reply, receive) = oneshot::channel();
    harness
        .tx
        .send(DriverCommand::SetOption {
            option_id: "effort".into(),
            value: ConfigValue::Choice {
                value: effort.into(),
            },
            reply,
        })
        .expect("set effort");
    let request = harness.wire().await;
    assert_eq!(request["request"]["settings"]["effortLevel"], effort);
    harness.send(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{}}})).await;
    assert!(receive.await.expect("reply").is_ok());
    assert_eq!(
        current(&next_options(harness).await, "effort"),
        Some(effort)
    );
}

#[tokio::test(start_paused = true)]
async fn recorded_init_and_status_frames_refresh_model_and_native_mode() {
    let (mut harness, _) = opened(real_initialize(), None).await;
    let init = real_frames(include_str!(
        "../../../../docs/spikes/claude-control/fixtures/turn-basic.jsonl"
    ))
    .find(|frame| frame["type"] == "system" && frame["subtype"] == "init")
    .expect("recorded system init");
    send_metadata(&harness, init).await;
    assert_eq!(
        current(&next_options(&mut harness).await, "model"),
        Some("haiku")
    );
    let statuses = real_frames(include_str!(
        "../../../../docs/spikes/claude-control/fixtures/controls.jsonl"
    ))
    .filter(|frame| {
        frame["type"] == "system"
            && frame["subtype"] == "status"
            && frame["permissionMode"].is_string()
    });
    let mut count = 0;
    for status in statuses {
        let expected = status["permissionMode"]
            .as_str()
            .expect("native mode")
            .to_owned();
        harness.send(status).await;
        loop {
            if let DriverEvent::Autonomy(state) = harness.event().await {
                assert_eq!(state.requested, Autonomy::Ask);
                assert_eq!(state.native_mode.as_deref(), Some(expected.as_str()));
                assert_eq!(state.auto_answer, AutoAnswer::Never);
                assert_eq!(state.note.is_some(), expected != "default");
                break;
            }
        }
        count += 1;
    }
    assert_eq!(count, 2, "recorded native mode changes");
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn resolving_an_alias_keeps_explicit_effort_but_a_different_model_revalidates_it() {
    let payload = json!({"model":"alias","current_permission_mode":"default","session_state":"idle","commands":[],"models":[
        {"value":"alias","resolvedModel":"actual-a","supportsEffort":true,"supportedEffortLevels":["low","high"]},
        {"value":"other","resolvedModel":"actual-b","supportsEffort":true,"supportedEffortLevels":["low"]}
    ]});
    let (mut harness, _) = opened(payload, None).await;
    set_effort(&mut harness, "high").await;
    // The alias's own resolved id changes nothing, so no list goes out for it.
    send_metadata(
        &harness,
        json!({"type":"system","subtype":"init","model":"actual-a"}),
    )
    .await;
    let (reply, receive) = oneshot::channel();
    harness
        .tx
        .send(DriverCommand::SetOption {
            option_id: "model".into(),
            value: ConfigValue::Choice {
                value: "alias".into(),
            },
            reply,
        })
        .expect("set equivalent model alias");
    let request = harness.wire().await;
    assert_eq!(request["request"]["subtype"], "set_model");
    harness.send(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{}}})).await;
    assert!(receive.await.expect("model reply").is_ok());
    let options = next_options(&mut harness).await;
    assert_eq!(current(&options, "model"), Some("alias"));
    assert_eq!(current(&options, "effort"), Some("high"));
    send_metadata(
        &harness,
        json!({"type":"system","subtype":"init","model":"actual-b"}),
    )
    .await;
    let options = next_options(&mut harness).await;
    assert_eq!(current(&options, "model"), Some("other"));
    assert_eq!(current(&options, "effort"), Some(translate::DEFAULT_EFFORT));
    let (reply, receive) = oneshot::channel();
    harness
        .tx
        .send(DriverCommand::SetOption {
            option_id: "effort".into(),
            value: ConfigValue::Choice {
                value: "high".into(),
            },
            reply,
        })
        .expect("set effort");
    assert!(receive.await.expect("reply").is_err());
    set_effort(&mut harness, "low").await;
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn early_native_metadata_is_not_overwritten_by_initialize_defaults() {
    let payload = real_initialize();
    let (mut harness, options) = opened(payload, Some(json!({"type":"system","subtype":"init","model":"claude-haiku-5-5","permissionMode":"default"}))).await;
    assert_eq!(current(&options, "model"), Some("haiku"));
    send_metadata(&harness, json!({"type":"system","subtype":"init","parent_tool_use_id":"subagent-tool","model":"subagent-model","permissionMode":"bypassPermissions"})).await;
    send_metadata(&harness, json!({"type":"system","subtype":"init","model":"future-native-model","permissionMode":"vendorPolicy"})).await;
    let options = next_options(&mut harness).await;
    assert_eq!(current(&options, "model"), Some("future-native-model"));
    assert_eq!(current(&options, "effort"), None);
    loop {
        if let DriverEvent::Autonomy(state) = harness.event().await {
            assert_eq!(state.requested, Autonomy::Ask);
            assert_eq!(state.native_mode.as_deref(), Some("vendorPolicy"));
            assert_eq!(state.auto_answer, AutoAnswer::Never);
            assert!(state.note.is_some());
            break;
        }
    }
    harness.stop().await;
}

/// `default` and `opus` both resolve to `claude-opus-5-5` (recorded in 2.1.310). Claude answers a
/// switch with the resolved id and names it again at each turn; the chosen alias stays selected.
#[tokio::test(start_paused = true)]
async fn choosing_opus_keeps_opus_although_default_resolves_to_the_same_model() {
    let (mut harness, options) = opened(real_initialize(), None).await;
    assert_eq!(current(&options, "model"), Some("default"));
    let options = choose_model(&mut harness, "opus", "claude-opus-5-5").await;
    assert_eq!(current(&options, "model"), Some("opus"));
    send_metadata(
        &harness,
        json!({"type":"system","subtype":"init","model":"claude-opus-5-5"}),
    )
    .await;
    set_effort(&mut harness, "high").await;
    let options = choose_model(&mut harness, "default", "claude-opus-5-5").await;
    assert_eq!(current(&options, "model"), Some("default"));
    assert_eq!(
        current(&options, "effort"),
        Some("high"),
        "the same model keeps its effort"
    );
    let options = choose_model(&mut harness, "opus", "claude-opus-5-5").await;
    assert_eq!(current(&options, "model"), Some("opus"));
    let options = choose_model(&mut harness, "sonnet", "claude-sonnet-5-5").await;
    assert_eq!(current(&options, "model"), Some("sonnet"));
    assert_eq!(
        current(&options, "effort"),
        Some(translate::DEFAULT_EFFORT),
        "another model drops the effort chosen for the last one"
    );
    // A switch Claude makes itself (its own /model) names only the resolved id, which shows as
    // the first alias that resolves to it.
    send_metadata(
        &harness,
        json!({"type":"system","subtype":"init","model":"claude-opus-5-5"}),
    )
    .await;
    assert_eq!(
        current(&next_options(&mut harness).await, "model"),
        Some("default")
    );
    harness.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_saved_alias_survives_an_early_resolved_init() {
    let mut setup = super::super::tests::setup();
    setup.desired.insert(
        "model".into(),
        ConfigValue::Choice {
            value: "opus".into(),
        },
    );
    let (harness, options, _) = opened_with(
        setup,
        real_initialize(),
        Some(json!({"type":"system","subtype":"init","model":"claude-opus-5-5"})),
    )
    .await;
    assert_eq!(current(&options, "model"), Some("opus"));
    harness.stop().await;
}

/// A thinking level saved for another model, or one Claude no longer offers, never fails the
/// start: the session opens at the model's default and says why.
#[tokio::test(start_paused = true)]
async fn a_saved_effort_the_model_lacks_starts_at_its_default_with_a_notice() {
    let payload = json!({"current_permission_mode":"default","session_state":"idle","commands":[],"models":[
        {"value":"few","resolvedModel":"few","supportsEffort":true,"supportedEffortLevels":["low"]},
        {"value":"none","resolvedModel":"none"}
    ]});
    for (model, offered) in [("few", Some(translate::DEFAULT_EFFORT)), ("none", None)] {
        let mut setup = super::super::tests::setup();
        for (id, value) in [("model", model), ("effort", "high")] {
            setup.desired.insert(
                id.into(),
                ConfigValue::Choice {
                    value: value.into(),
                },
            );
        }
        let (harness, options, notices) = opened_with(setup, payload.clone(), None).await;
        assert_eq!(current(&options, "model"), Some(model));
        assert_eq!(current(&options, "effort"), offered);
        assert!(
            notices
                .iter()
                .any(|notice| notice.contains("thinking level chosen earlier")),
            "{notices:?}"
        );
        harness.stop().await;
    }
}

#[test]
fn current_model_is_present_even_when_missing_or_beyond_the_choice_limit() {
    let models = (0..505)
        .map(|index| json!({"value":format!("model-{index}")}))
        .collect::<Vec<_>>();
    for model in ["future-model", "model-504"] {
        let options = translate::options(&json!({"models":models}), model, None);
        assert_eq!(current(&options, "model"), Some(model));
        let ConfigKind::Select { choices, .. } = &options[0].kind else {
            panic!("model select")
        };
        assert_eq!(choices.len(), crate::acp::options::MAX_CHOICES);
    }
    assert_eq!(
        current(
            &translate::options(&json!({}), "known-from-native", None),
            "model"
        ),
        Some("known-from-native")
    );
}

#[test]
fn main_stop_reason_resets_at_turn_and_message_boundaries_and_ignores_subagents() {
    let mut translator = translate::Translator::with_prefix("spawn");
    let delta = |reason: &str, parent: Value| json!({"type":"stream_event","parent_tool_use_id":parent,"event":{"type":"message_delta","delta":{"stop_reason":reason}}});
    translator.ingest(&delta("max_tokens", Value::Null));
    assert_eq!(translator.main_stop_reason(), Some("max_tokens"));
    translator.ingest(&delta("refusal", json!("subagent")));
    translator.ingest(&json!({"type":"stream_event","parent_tool_use_id":"subagent","event":{"type":"message_start","message":{"id":"sub-message"}}}));
    assert_eq!(translator.main_stop_reason(), Some("max_tokens"));
    translator.ingest(&json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_start","message":{"id":"next-main-message"}}}));
    assert_eq!(translator.main_stop_reason(), None);
    translator.ingest(&delta("refusal", Value::Null));
    translator.ingest(&json!({"type":"result","subtype":"success"}));
    assert_eq!(translator.main_stop_reason(), Some("refusal"));
    translator.begin_turn();
    assert_eq!(translator.main_stop_reason(), None);
}
