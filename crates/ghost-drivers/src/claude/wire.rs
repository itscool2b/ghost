//! Claude's control envelopes are not JSON-RPC. Keep unsolicited stream frames and control replies
//! distinct, and never include input text in parser errors (it may contain account data).

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Request { id: String, request: Value },
    Response { id: String, response: Value },
    Cancel { id: String },
    Event(Value),
    Invalid(&'static str),
}

pub fn parse_line(line: &str) -> Incoming {
    let Ok(mut value) = serde_json::from_str::<Value>(line) else {
        return Incoming::Invalid("not JSON");
    };
    let Some(kind) = nonempty_string(&value, "type") else {
        return Incoming::Invalid("missing message type");
    };
    match kind {
        "control_request" => {
            let Some(id) = nonempty_string(&value, "request_id").map(str::to_owned) else {
                return Incoming::Invalid("invalid control request id");
            };
            if nonempty_string(&value["request"], "subtype").is_none() {
                return Incoming::Invalid("invalid control request body");
            }
            Incoming::Request {
                id,
                request: value["request"].take(),
            }
        }
        "control_response" => {
            let response = &value["response"];
            let Some(id) = nonempty_string(response, "request_id").map(str::to_owned) else {
                return Incoming::Invalid("invalid control response id");
            };
            if !matches!(
                nonempty_string(response, "subtype"),
                Some("success" | "error")
            ) {
                return Incoming::Invalid("invalid control response subtype");
            }
            Incoming::Response {
                id,
                response: value["response"].take(),
            }
        }
        "control_cancel_request" => match nonempty_string(&value, "request_id") {
            Some(id) => Incoming::Cancel { id: id.to_owned() },
            None => Incoming::Invalid("invalid control cancellation id"),
        },
        _ => Incoming::Event(value),
    }
}

fn nonempty_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn control_requests_keep_the_original_input() {
        let value = json!({"type":"control_request","request_id":"permission-9",
            "request":{"subtype":"can_use_tool","input":{"path":"a.txt"},
                "requires_user_interaction":true,"future_field":42}});
        assert_eq!(
            parse_line(&value.to_string()),
            Incoming::Request {
                id: "permission-9".into(),
                request: value["request"].clone()
            }
        );
    }

    #[test]
    fn nested_responses_keep_reconnect_prompts_and_errors() {
        for response in [
            json!({"subtype":"success","request_id":"r1","response":{"models":[]},
                "pending_permission_requests":[{"request_id":"p1"}]}),
            json!({"subtype":"error","request_id":"r1","error":"unsupported"}),
        ] {
            assert_eq!(
                parse_line(&json!({"type":"control_response","response":response}).to_string()),
                Incoming::Response {
                    id: "r1".into(),
                    response
                }
            );
        }
    }

    #[test]
    fn cancellations_are_separate_from_responses() {
        assert_eq!(
            parse_line(r#"{"type":"control_cancel_request","request_id":"p1"}"#),
            Incoming::Cancel { id: "p1".into() }
        );
    }

    #[test]
    fn streaming_hooks_and_future_events_pass_without_reinterpretation() {
        for value in [
            json!({"type":"system","subtype":"hook_started"}),
            json!({"type":"stream_event","event":{"type":"message_start"}}),
            json!({"type":"future_event","request_id":"unrelated"}),
        ] {
            assert_eq!(parse_line(&value.to_string()), Incoming::Event(value));
        }
    }

    #[test]
    fn malformed_control_frames_never_become_events_or_expose_input() {
        for line in [
            "private-account-data",
            "[]",
            "null",
            "{}",
            r#"{"type":3}"#,
            r#"{"type":"control_request","request_id":7,"request":{}}"#,
            r#"{"type":"control_request","request_id":"","request":{}}"#,
            r#"{"type":"control_request","request_id":"p1","request":null}"#,
            r#"{"type":"control_request","request_id":"p1","request":{"subtype":4}}"#,
            r#"{"type":"control_response","response":{"request_id":"r1"}}"#,
            r#"{"type":"control_response","response":{"request_id":"r1","subtype":"future"}}"#,
            r#"{"type":"control_cancel_request"}"#,
        ] {
            let Incoming::Invalid(reason) = parse_line(line) else {
                panic!("accepted {line}")
            };
            assert!(!reason.contains("private-account-data"));
        }
    }

    #[test]
    fn recorded_native_streams_parse_without_rejecting_vendor_frames() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/spikes/claude-control/fixtures");
        let mut count = 0;
        for file in [
            "turn-basic.jsonl",
            "turn-permission.jsonl",
            "turn-interrupt-permission.jsonl",
            "controls.jsonl",
        ] {
            let text = std::fs::read_to_string(root.join(file)).expect("recorded fixture");
            for (index, line) in text.lines().enumerate() {
                assert!(
                    !matches!(parse_line(line), Incoming::Invalid(_)),
                    "{file}:{}",
                    index + 1
                );
                count += 1;
            }
        }
        assert!(count > 20);
    }
}
