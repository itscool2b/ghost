//! Callback payloads are kept separate from their displayed, normalized copies.
use ghost_protocol::{
    PermissionOption, PermissionOptionKind, PermissionRequest, ToolContent, ToolKind, UiAnswer,
    UiQuestion, UiQuestionOption,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug)]
pub(super) enum Callback {
    Approval {
        permissions: Option<Value>,
        decisions: BTreeSet<String>,
    },
    Questions(Vec<UiQuestion>),
    /// An MCP server's form or browser step (`mcpServer/elicitation/request`).
    Elicitation(crate::elicitation::Elicitation),
}

impl Callback {
    pub fn cancel(&self) -> Value {
        match self {
            Self::Approval {
                permissions: Some(_),
                ..
            } => json!({"permissions":{},"scope":"turn"}),
            Self::Approval {
                permissions: None, ..
            } => json!({"decision":"cancel"}),
            Self::Questions(_) => json!({"answers":{}}),
            Self::Elicitation(_) => json!({"action":"cancel","content":null}),
        }
    }

    pub fn permission_answer(&self, decision: &str) -> Option<Value> {
        let Self::Approval {
            permissions,
            decisions,
        } = self
        else {
            return None;
        };
        if !decisions.contains(decision) {
            return None;
        }
        Some(match permissions {
            Some(permissions) if matches!(decision, "accept" | "acceptForSession") => {
                json!({"permissions":permissions,"scope":if decision == "acceptForSession" {"session"} else {"turn"}})
            }
            Some(_) => json!({"permissions":{},"scope":"turn"}),
            None => json!({"decision":decision}),
        })
    }

    pub fn question_answer(&self, answer: &UiAnswer) -> Option<Value> {
        if let Self::Elicitation(spec) = self {
            return spec.result(Some(answer));
        }
        let (Self::Questions(questions), UiAnswer::Questions { answers }) = (self, answer) else {
            return None;
        };
        if questions.len() != answers.len() {
            return None;
        }
        let mut result = serde_json::Map::new();
        for question in questions {
            let mut matches = answers.iter().filter(|a| a.question_id == question.id);
            let answer = matches.next()?;
            if matches.next().is_some()
                || answer.selected.len() + usize::from(answer.text.is_some()) != 1
            {
                return None;
            }
            let text = if let Some(text) = &answer.text {
                if !question.allow_free_text || text.trim().is_empty() {
                    return None;
                }
                text.clone()
            } else {
                question
                    .options
                    .get(*answer.selected.first()? as usize)?
                    .label
                    .clone()
            };
            result.insert(question.id.clone(), json!({"answers":[text]}));
        }
        Some(json!({"answers":result}))
    }
}

pub(super) fn questions(params: &Value) -> Result<Vec<UiQuestion>, &'static str> {
    let Some(items) = params["questions"].as_array() else {
        return Err("Codex sent malformed questions.");
    };
    if items.is_empty() || items.len() > 8 {
        return Err("Codex sent an unsupported number of questions.");
    }
    let mut ids = BTreeSet::new();
    let mut result = Vec::new();
    for item in items {
        if ["isSecret", "isOther"]
            .iter()
            .any(|field| item.get(field).is_some_and(|value| !value.is_boolean()))
        {
            return Err("Codex sent malformed question flags.");
        }
        if item["isSecret"].as_bool().unwrap_or(false) {
            return Err(
                "Secret input is not supported here. Continue this request in the Codex CLI.",
            );
        }
        let id = item["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .ok_or("Codex sent a question without a valid ID.")?;
        if !ids.insert(id) {
            return Err("Codex sent duplicate question IDs.");
        }
        let text = item["question"]
            .as_str()
            .ok_or("Codex sent a question without text.")?;
        let mut options = Vec::new();
        if let Some(choices) = item["options"].as_array() {
            if choices.len() > 16 {
                return Err("Codex sent too many choices.");
            }
            for choice in choices {
                options.push(UiQuestionOption {
                    label: choice["label"]
                        .as_str()
                        .ok_or("Codex sent a choice without a label.")?
                        .into(),
                    description: choice["description"].as_str().unwrap_or("").into(),
                });
            }
        } else if !item["options"].is_null() {
            return Err("Codex sent malformed choices.");
        }
        let allow_free_text = options.is_empty() || item["isOther"].as_bool().unwrap_or(false);
        result.push(UiQuestion {
            id: id.into(),
            header: item["header"].as_str().unwrap_or("").into(),
            question: text.into(),
            options,
            multi_select: false,
            allow_free_text,
        });
    }
    Ok(result)
}

pub(super) fn approval(
    method: &str,
    params: &Value,
    request_id: String,
) -> Option<(Callback, PermissionRequest)> {
    let (title, kind, permissions) = match method {
        "item/commandExecution/requestApproval" => (
            params["command"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if params["networkApprovalContext"].is_object() {
                        format!(
                            "Allow network access to {}",
                            params["networkApprovalContext"]["host"]
                                .as_str()
                                .unwrap_or("this host")
                        )
                    } else {
                        "Allow command execution".into()
                    }
                }),
            ToolKind::Execute,
            None,
        ),
        "item/fileChange/requestApproval" => ("Allow file changes".into(), ToolKind::Edit, None),
        "item/permissions/requestApproval" if params["permissions"].is_object() => (
            "Allow additional permissions".into(),
            ToolKind::Other,
            Some(params["permissions"].clone()),
        ),
        _ => return None,
    };
    // Newer native builds advertise a narrower choice set than the stable schema.
    // Persistent policy amendments require a separate UI and are not offered here.
    let advertised = match params.get("availableDecisions") {
        None | Some(Value::Null) => None,
        Some(Value::Array(choices)) => Some(choices),
        Some(_) => return None,
    };
    let options: Vec<_> = [
        ("accept", "Allow once", PermissionOptionKind::AllowOnce),
        (
            "acceptForSession",
            "Allow for this session",
            PermissionOptionKind::AllowAlways,
        ),
        ("decline", "Decline", PermissionOptionKind::RejectOnce),
        (
            "cancel",
            if permissions.is_some() {
                "Cancel request"
            } else {
                "Cancel turn"
            },
            PermissionOptionKind::Other,
        ),
    ]
    .into_iter()
    .filter(|(id, _, _)| {
        advertised.is_none_or(|choices| choices.iter().any(|choice| choice.as_str() == Some(id)))
    })
    .map(|(id, label, kind)| PermissionOption {
        option_id: id.into(),
        label: label.into(),
        kind,
    })
    .collect();
    if options.is_empty() {
        return None;
    }
    let decisions = options
        .iter()
        .map(|option| option.option_id.clone())
        .collect();
    let content = ["reason", "grantRoot"]
        .into_iter()
        .filter_map(|field| {
            params[field]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(|text| ToolContent::Text {
                    text: format!("{field}: {text}"),
                    truncated: None,
                })
        })
        .collect();
    Some((
        Callback::Approval {
            permissions,
            decisions,
        },
        PermissionRequest {
            request_id,
            tool_call_id: None,
            title,
            kind: Some(kind),
            raw_input: Some(params.clone()),
            content,
            locations: Vec::new(),
            options,
            truncated: None,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_protocol::UiQuestionAnswer;

    #[test]
    fn grouped_answers_use_original_ids_and_labels() {
        let questions = questions(&json!({"questions":[{"id":"q.original","question":"Which?","options":[{"label":"One","description":"first"}],"isOther":true}]})).unwrap();
        let callback = Callback::Questions(questions);
        assert_eq!(
            callback.question_answer(&UiAnswer::Questions {
                answers: vec![UiQuestionAnswer {
                    question_id: "q.original".into(),
                    selected: vec![0],
                    text: None
                }]
            }),
            Some(json!({"answers":{"q.original":{"answers":["One"]}}}))
        );
        assert_eq!(
            callback.question_answer(&UiAnswer::Questions {
                answers: vec![UiQuestionAnswer {
                    question_id: "q.other".into(),
                    selected: vec![0],
                    text: None
                }]
            }),
            None
        );
    }

    #[test]
    fn secret_and_ambiguous_questions_never_become_persisted_ui() {
        assert!(
            questions(&json!({"questions":[{"id":"q","question":"Secret?","isSecret":true}]}))
                .is_err()
        );
        assert!(
            questions(
                &json!({"questions":[{"id":"q","question":"One"},{"id":"q","question":"Two"}]})
            )
            .is_err()
        );
        for field in ["isSecret", "isOther"] {
            let mut params = json!({"questions":[{"id":"q","question":"Value"}]});
            params["questions"][0][field] = json!("true");
            assert!(questions(&params).is_err());
        }
    }

    #[test]
    fn permission_grants_cannot_add_unrequested_rights() {
        let callback = Callback::Approval {
            permissions: Some(json!({"network":{"enabled":true}})),
            decisions: ["accept", "decline"].map(str::to_owned).into(),
        };
        assert_eq!(
            callback.permission_answer("accept"),
            Some(json!({"permissions":{"network":{"enabled":true}},"scope":"turn"}))
        );
        assert_eq!(
            callback.permission_answer("decline"),
            Some(json!({"permissions":{},"scope":"turn"}))
        );
        assert_eq!(callback.permission_answer("invented"), None);
    }

    #[test]
    fn advertised_decisions_bound_both_visible_choices_and_wire_answers() {
        let params = json!({"command":"echo test","availableDecisions":["accept",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["echo"]}},"cancel"]});
        let (callback, request) =
            approval("item/commandExecution/requestApproval", &params, "r".into()).unwrap();
        assert_eq!(
            request
                .options
                .iter()
                .map(|option| option.option_id.as_str())
                .collect::<Vec<_>>(),
            ["accept", "cancel"]
        );
        assert_eq!(
            callback.permission_answer("accept"),
            Some(json!({"decision":"accept"}))
        );
        assert_eq!(callback.permission_answer("acceptForSession"), None);
        assert_eq!(callback.permission_answer("decline"), None);
        for choices in [json!([]), json!(["unknown"]), json!("accept")] {
            assert!(
                approval(
                    "item/commandExecution/requestApproval",
                    &json!({"availableDecisions":choices}),
                    "r".into()
                )
                .is_none()
            );
        }
    }
}
