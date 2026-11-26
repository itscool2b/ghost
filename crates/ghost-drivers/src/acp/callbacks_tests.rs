use super::callbacks::*;
use super::dialect::DialectReply;
use ghost_protocol::{UiAnswer, UiPrompt, UiQuestionAnswer};
use serde_json::{Value, json};

#[test]
fn form_choice_labels_and_descriptions_preserve_native_answer_values() {
    for property in [
        json!({"type":"string", "oneOf":[{"const":"native_yes", "title":"Approve changes", "description":"Write the proposed files"}]}),
        json!({"type":"string", "enum":["native_yes"], "enumNames":["Approve changes"]}),
    ] {
        let has_description = property.get("oneOf").is_some();
        let call = call(
            "elicitation/create",
            json!({"mode":"form","sessionId":"s","message":"Review",
            "requestedSchema":{"type":"object","properties":{"choice":property}}}),
            false,
        );
        let UiPrompt::Questions { questions, .. } = &call.prompt else {
            panic!("questions");
        };
        assert_eq!(questions[0].options[0].label, "Approve changes");
        assert_eq!(
            questions[0].options[0].description,
            if has_description {
                "Write the proposed files"
            } else {
                ""
            }
        );
        assert_eq!(
            call.answer(&UiAnswer::Questions {
                answers: vec![UiQuestionAnswer {
                    question_id: "choice".into(),
                    selected: vec![0],
                    text: None,
                }]
            }),
            Some(json!({"action":"accept","content":{"choice":"native_yes"}}))
        );
    }
}

#[test]
fn plan_approval_displays_separate_tasks_and_phases_before_confirmation() {
    let call = call(
        "cursor/create_plan",
        json!({"toolCallId":"p", "plan":"Overview only",
        "todos":[{"id":"t","content":"Remove obsolete files","status":"pending"}],
        "phases":[{"name":"Verification", "todos":[{"id":"v","content":"Run tests","status":"pending"}]}]}),
        true,
    );
    let context = call.context.unwrap();
    assert!(context.contains("Remove obsolete files"));
    assert!(context.contains("Verification"));
    assert!(context.contains("Run tests"));
}

fn call(method: &str, params: Value, cursor: bool) -> UiCall {
    match request(method, &params, cursor) {
        DialectReply::Deferred(call) => *call,
        reply => panic!("expected deferred input, got {reply:?}"),
    }
}

#[test]
fn cursor_answers_keep_native_ids_and_require_valid_explicit_choices() {
    let call = call(
        "cursor/ask_question",
        json!({"toolCallId":"call1","questions":[
        {"id":"native-q","prompt":"Choose","options":[{"id":"native-option","label":"Visible"}]}]}),
        true,
    );
    let answer = |selected| UiAnswer::Questions {
        answers: vec![UiQuestionAnswer {
            question_id: "native-q".into(),
            selected,
            text: None,
        }],
    };
    assert_eq!(
        call.answer(&answer(vec![0])),
        Some(json!({"outcome":{"outcome":"answered",
        "answers":[{"questionId":"native-q","selectedOptionIds":["native-option"]}]}}))
    );
    assert_eq!(call.answer(&answer(vec![1])), None);
    assert_eq!(call.answer(&answer(vec![0, 0])), None);
    assert_eq!(call.answer(&answer(vec![])), None);
    assert_eq!(call.answer(&UiAnswer::Confirm { yes: true }), None);
    assert_eq!(call.cancelled(), json!({"outcome":{"outcome":"cancelled"}}));
}

#[test]
fn cursor_plan_preserves_the_full_markdown_and_never_approves_on_dismissal() {
    let plan = "A complete plan paragraph.\n".repeat(200);
    let call = call(
        "cursor/create_plan",
        json!({"toolCallId":"plan","name":"Work","plan":plan,"todos":[]}),
        true,
    );
    assert!(call.context.as_ref().unwrap().contains(&plan));
    assert_eq!(
        call.answer(&UiAnswer::Confirm { yes: true }),
        Some(json!({"outcome":{"outcome":"accepted"}}))
    );
    assert_eq!(
        call.answer(&UiAnswer::Confirm { yes: false }),
        Some(json!({"outcome":{"outcome":"rejected"}}))
    );
    assert_eq!(call.cancelled(), json!({"outcome":{"outcome":"cancelled"}}));
}

#[test]
fn omp_plan_choice_and_boolean_answers_have_their_native_types() {
    let call = call(
        "elicitation/create",
        json!({"mode":"form","sessionId":"s","message":"Approve?",
        "requestedSchema":{"type":"object","properties":{
            "value":{"type":"string","enum":["Approve","Refine"]}, "confirm":{"type":"boolean"}
        },"required":["value","confirm"]}}),
        false,
    );
    let answer = UiAnswer::Questions {
        answers: vec![
            UiQuestionAnswer {
                question_id: "confirm".into(),
                selected: vec![1],
                text: None,
            },
            UiQuestionAnswer {
                question_id: "value".into(),
                selected: vec![1],
                text: None,
            },
        ],
    };
    assert_eq!(
        call.answer(&answer),
        Some(json!({"action":"accept","content":{"value":"Refine","confirm":false}}))
    );
    assert_eq!(call.cancelled(), json!({"action":"cancel"}));
}

#[test]
fn omp_ask_dialog_pairs_other_text_and_preserves_multi_select_values() {
    let call = call(
        "elicitation/create",
        json!({"mode":"form","sessionId":"s","message":"Questions",
        "requestedSchema":{"type":"object","properties":{
            "q0":{"type":"array","items":{"anyOf":[{"const":"A","title":"A"},{"const":"B","title":"B"}]}},
            "q0__other":{"type":"string","title":"Other"},
            "q1":{"type":"string","oneOf":[{"const":"C"}]},"q1__other":{"type":"string"}
        }}}),
        false,
    );
    let answer = UiAnswer::Questions {
        answers: vec![
            UiQuestionAnswer {
                question_id: "q1".into(),
                selected: vec![],
                text: Some("Custom".into()),
            },
            UiQuestionAnswer {
                question_id: "q0".into(),
                selected: vec![0, 1],
                text: Some("Also".into()),
            },
        ],
    };
    assert_eq!(
        call.answer(&answer),
        Some(
            json!({"action":"accept","content":{"q0":["A","B"],"q0__other":"Also","q1__other":"Custom"}})
        )
    );
}

#[test]
fn unsupported_forms_and_constraints_return_errors_without_a_dialog() {
    for property in [
        json!({"type":"number"}),
        json!({"type":"object"}),
        json!({"type":"string","minLength":3}),
        json!({"type":"string","format":"email"}),
        json!({"type":"array","items":{"anyOf":[{"const":"A","not":{}}]}}),
        json!({"type":"string","enum":["A","A"]}),
    ] {
        assert!(matches!(
            request(
                "elicitation/create",
                &json!({"mode":"form","sessionId":"s","message":"Input",
            "requestedSchema":{"type":"object","properties":{"value":property}}}),
                false
            ),
            DialectReply::Error { .. }
        ));
    }
    assert!(matches!(
        request("elicitation/create", &json!({"mode":"url"}), false),
        DialectReply::Error { .. }
    ));
}
