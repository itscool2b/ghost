//! Extension dialogs are explicit user interactions, never permissions that Full auto can answer.
use super::wire::text;
use ghost_protocol::{UiAnswer, UiPrompt};
use serde_json::{Value, json};

pub fn prompt(frame: &Value) -> Option<UiPrompt> {
    let title = text(frame, "title").unwrap_or("Pi extension");
    let prompt = match text(frame, "method")? {
        "confirm" => Some(UiPrompt::Confirm {
            message: match text(frame, "message") {
                Some(message) => format!("{title}\n\n{message}"),
                None => title.into(),
            },
        }),
        "select" => {
            let options = frame["options"].as_array()?;
            if options.is_empty() || options.len() > 1000 {
                return None;
            }
            let options: Option<Vec<_>> = options
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect();
            Some(UiPrompt::Select {
                message: title.into(),
                options: options?,
            })
        }
        "input" => Some(UiPrompt::Input {
            message: title.into(),
            placeholder: frame["placeholder"].as_str().map(str::to_owned),
        }),
        // The current product input has no editable prefill/multiline contract. Cancelling is
        // honest; presenting an empty input would silently lose the editor's original content.
        _ => None,
    }?;
    fits_display(&prompt).then_some(prompt)
}

fn fits_display(prompt: &UiPrompt) -> bool {
    // Core caps question messages and labels before display. Never ask for an answer to a
    // shortened dialog; leave room for the request id and event envelope within its 7 KiB cap.
    let message_fits =
        |message: &str| serde_json::to_string(message).is_ok_and(|json| json.len() <= 2048);
    if !serde_json::to_vec(prompt).is_ok_and(|json| json.len() <= 6000) {
        return false;
    }
    match prompt {
        UiPrompt::Confirm { message } => message_fits(message),
        UiPrompt::Select { message, options } => {
            message_fits(message) && options.iter().all(|option| option.chars().count() <= 64)
        }
        UiPrompt::Input {
            message,
            placeholder,
        } => {
            message_fits(message)
                && placeholder
                    .as_ref()
                    .is_none_or(|text| text.chars().count() <= 256)
        }
        _ => false,
    }
}
pub fn response(frame: &Value, answer: Option<UiAnswer>) -> Value {
    let mut response = json!({"type":"extension_ui_response","id":frame["id"],"cancelled":true});
    let field = match (text(frame, "method"), answer) {
        (Some("confirm"), Some(UiAnswer::Confirm { yes })) => Some(("confirmed", json!(yes))),
        (Some("input"), Some(UiAnswer::Input { text })) => Some(("value", json!(text))),
        (Some("select"), Some(UiAnswer::Select { index })) => frame["options"]
            .get(index as usize)
            .and_then(Value::as_str)
            .map(|s| ("value", json!(s))),
        _ => None,
    };
    if let Some((key, value)) = field {
        if let Some(object) = response.as_object_mut() {
            object.remove("cancelled");
        }
        response[key] = value;
    }
    response
}
pub fn blocking(method: &str) -> bool {
    matches!(method, "select" | "confirm" | "input" | "editor")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_returns_the_original_value_and_bad_answers_cancel() {
        let frame = json!({"type":"extension_ui_request","id":"q1","method":"select","title":"Choose","options":["A","B"]});
        assert_eq!(
            response(&frame, Some(UiAnswer::Select { index: 1 }))["value"],
            "B"
        );
        assert_eq!(
            response(&frame, Some(UiAnswer::Select { index: 10 }))["cancelled"],
            true
        );
        assert_eq!(
            response(&frame, Some(UiAnswer::Confirm { yes: true }))["cancelled"],
            true
        );
        assert!(prompt(&json!({"method":"editor","prefill":"important"})).is_none());
    }

    #[test]
    fn dialogs_that_core_would_truncate_are_rejected_before_display() {
        for frame in [
            json!({"method":"confirm","title":"Approve","message":"x".repeat(2048)}),
            json!({"method":"confirm","title":"\\\"".repeat(600)}),
            json!({"method":"select","title":"Choose","options":["x".repeat(65)]}),
            json!({"method":"input","title":"Name","placeholder":"x".repeat(257)}),
            json!({"method":"select","title":"Choose","options":vec!["x".repeat(64); 100]}),
        ] {
            assert!(prompt(&frame).is_none(), "oversized dialog was accepted");
        }
    }

    #[test]
    fn display_limits_preserve_unicode_labels_and_native_answers() {
        let label = "界".repeat(64);
        let frame = json!({"method":"select","title":"Choose","options":[label]});
        let Some(UiPrompt::Select { options, .. }) = prompt(&frame) else {
            panic!("a complete 64-character label fits");
        };
        assert_eq!(options, vec![label.clone()]);
        assert_eq!(
            response(&frame, Some(UiAnswer::Select { index: 0 }))["value"],
            label
        );
        assert!(
            prompt(&json!({"method":"input","title":"Name","placeholder":"界".repeat(256)}))
                .is_some()
        );
    }
}
