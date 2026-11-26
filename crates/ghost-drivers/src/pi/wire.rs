//! Pi RPC is a JSONL protocol, not JSON-RPC. Errors deliberately omit the input.
use serde_json::Value;

#[derive(Debug, PartialEq)]
pub enum Incoming {
    Response {
        id: String,
        command: String,
        success: bool,
        data: Value,
        error: Option<String>,
    },
    Ui(Value),
    Event(Value),
    Invalid(&'static str),
}

pub fn parse_line(line: &str) -> Incoming {
    let Ok(mut value) = serde_json::from_str::<Value>(line) else {
        return Incoming::Invalid("not JSON");
    };
    match text(&value, "type") {
        Some("response") => {
            let (Some(id), Some(command), Some(success)) = (
                text(&value, "id"),
                text(&value, "command"),
                value["success"].as_bool(),
            ) else {
                return Incoming::Invalid("invalid response envelope");
            };
            let (id, command) = (id.to_owned(), command.to_owned());
            Incoming::Response {
                id,
                command,
                success,
                data: value["data"].take(),
                error: value["error"].as_str().map(str::to_owned),
            }
        }
        Some("extension_ui_request") => {
            if text(&value, "id").is_none() || text(&value, "method").is_none() {
                Incoming::Invalid("invalid extension UI envelope")
            } else {
                Incoming::Ui(value)
            }
        }
        Some(_) => Incoming::Event(value),
        None => Incoming::Invalid("missing message type"),
    }
}

pub fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responses_require_correlation_and_boolean_success() {
        for input in [
            "secret",
            "null",
            "[]",
            "{}",
            r#"{"type":"response","success":true}"#,
            r#"{"type":"response","id":"x","command":"prompt","success":"true"}"#,
        ] {
            assert!(matches!(parse_line(input), Incoming::Invalid(_)));
        }
        assert!(matches!(
            parse_line(
                r#"{"type":"response","id":"p1","command":"prompt","success":true,"data":{"disposition":"started"}}"#
            ),
            Incoming::Response { success: true, .. }
        ));
    }
    #[test]
    fn future_events_are_ignored_by_the_consumer_not_parser() {
        assert!(matches!(
            parse_line(r#"{"type":"future_event","content":"a\u2028b"}"#),
            Incoming::Event(_)
        ));
    }
}
