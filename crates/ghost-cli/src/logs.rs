//! Reading protocol logs back (spec 3.8): one JSON Lines file per agent process,
//! `<profile>/logs/protocol/<chat_id>/<spawn_id>.jsonl`, rotated to `.1.jsonl` and `.2.jsonl`.
//!
//! Probe and smoke read their own runs' logs (the spawn note's pid, the initialize reply, the exit
//! and shutdown notes); `fixture` reads the log it curates. Everything here is a plain function of
//! the records, so it is tested on records written by hand and on package B's fixtures, which
//! have the same shape.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The methods that open a session: their `params.cwd` is the chat folder.
pub const OPEN_METHODS: [&str; 6] = [
    "session/new",
    "session/load",
    "session/resume",
    "thread/start",
    "thread/resume",
    "thread/fork",
];

/// The current log files in a chat's log folder, oldest spawn first: logs are named after UUIDv7
/// spawn ids, which sort by time. Rotated parts are left out ([`read`] reads them with their log).
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_suffix(".jsonl"))
                    .is_some_and(|stem| !stem.is_empty() && !stem.contains('.'))
        })
        .collect();
    logs.sort();
    logs
}

/// The files of one log, oldest first: `<spawn>.2.jsonl`, `<spawn>.1.jsonl`, then `<spawn>.jsonl`
/// (those that exist). A rotated part itself is refused: the log is named by its current file.
pub fn parts(log: &Path) -> Result<Vec<PathBuf>, String> {
    let Some(stem) = log
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".jsonl"))
    else {
        return Ok(vec![log.to_path_buf()]);
    };
    if let Some((base, generation)) = stem.rsplit_once('.')
        && !base.is_empty()
        && !generation.is_empty()
        && generation.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!(
            "{} is an older part of the log {base}.jsonl; pass {base}.jsonl, and its older parts are read with it",
            log.display()
        ));
    }
    let dir = log.parent().unwrap_or_else(|| Path::new(""));
    let mut parts: Vec<PathBuf> = (1..=9)
        .rev()
        .map(|generation| dir.join(format!("{stem}.{generation}.jsonl")))
        .filter(|part| part.is_file())
        .collect();
    parts.push(log.to_path_buf());
    Ok(parts)
}

/// A log's records, and how many lines were not JSON objects (a log cut short while written).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Records {
    pub records: Vec<Value>,
    pub unreadable: usize,
}

/// Reads a log with its rotated parts.
pub fn read(log: &Path) -> io::Result<Records> {
    let parts = parts(log).map_err(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason))?;
    let mut text = String::new();
    for part in parts {
        text.push_str(&std::fs::read_to_string(part)?);
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    Ok(parse(&text))
}

/// The records of a log's text.
pub fn parse(text: &str) -> Records {
    let mut parsed = Records::default();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<Value>(line) {
            Ok(record) if record.is_object() => parsed.records.push(record),
            _ => parsed.unreadable += 1,
        }
    }
    parsed
}

/// The record's direction: `out`, `in`, `err`, `cmd` or `note`.
pub fn direction(record: &Value) -> Option<&str> {
    record.get("d").and_then(Value::as_str)
}

/// The record's time, Unix ms (curated fixtures count from their first record).
pub fn t(record: &Value) -> Option<i64> {
    record.get("t").and_then(Value::as_i64)
}

/// The protocol message of an `out` or `in` record (ACP JSON-RPC or Claude stream-json).
pub fn message(record: &Value) -> Option<&Value> {
    record.get("msg")
}

/// The first note of this kind (`spawn`, `start`, `exit`, `shutdown`, `dropped`, `text`).
pub fn note<'a>(records: &'a [Value], kind: &str) -> Option<&'a Value> {
    records
        .iter()
        .filter(|record| direction(record) == Some("note"))
        .find_map(|record| record.get("note")?.get(kind))
}

/// Every note of this kind, in order.
pub fn notes<'a>(records: &'a [Value], kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
    records
        .iter()
        .filter(|record| direction(record) == Some("note"))
        .filter_map(move |record| record.get("note")?.get(kind))
}

/// An ACP request/notification method or a native Claude control subtype.
pub fn request_method(message: &Value) -> Option<&str> {
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        Some(method)
    } else if message["type"] == "control_request" && message["request_id"].is_string() {
        message.pointer("/request/subtype").and_then(Value::as_str)
    } else if message["id"].is_string()
        && matches!(
            message["type"].as_str(),
            Some(
                "prompt"
                    | "abort"
                    | "clear_queue"
                    | "get_state"
                    | "get_available_models"
                    | "get_available_thinking_levels"
                    | "get_session_stats"
                    | "get_commands"
                    | "set_model"
                    | "set_thinking_level"
            )
        )
    {
        message["type"].as_str()
    } else {
        None
    }
}

/// The first request the driver sent with this method or native control subtype.
pub fn request<'a>(records: &'a [Value], method: &str) -> Option<&'a Value> {
    records.iter().find(|record| {
        direction(record) == Some("out")
            && message(record).is_some_and(|msg| {
                request_method(msg) == Some(method)
                    && (msg.get("id").is_some()
                        || (msg["type"] == "control_request" && msg["request_id"].is_string()))
            })
    })
}

/// The agent's reply to the request with this id: its first `in` record with the id and a result
/// or an error.
pub fn reply<'a>(records: &'a [Value], id: &Value) -> Option<&'a Value> {
    records.iter().find(|record| {
        direction(record) == Some("in")
            && message(record).is_some_and(|msg| {
                (msg.get("id") == Some(id)
                    && msg.get("method").is_none()
                    && (msg.get("result").is_some() || msg.get("error").is_some()))
                    || (msg["type"] == "control_response"
                        && msg.pointer("/response/request_id") == Some(id)
                        && matches!(
                            msg.pointer("/response/subtype").and_then(Value::as_str),
                            Some("success" | "error")
                        ))
                    || (msg["type"] == "response"
                        && msg.get("id") == Some(id)
                        && msg["success"].is_boolean())
            })
    })
}

/// A request with this method and its reply, when both are in the log.
pub fn exchange<'a>(records: &'a [Value], method: &str) -> Option<(&'a Value, &'a Value)> {
    let sent = request(records, method)?;
    let request = message(sent)?;
    let id = request.get("id").or_else(|| request.get("request_id"))?;
    Some((sent, reply(records, id)?))
}

/// The first session open the driver sent (`session/new`, `session/load` or `session/resume`).
pub fn first_open(records: &[Value]) -> Option<&Value> {
    records.iter().find(|record| {
        direction(record) == Some("out")
            && message(record)
                .and_then(|msg| msg.get("method"))
                .and_then(Value::as_str)
                .is_some_and(|method| OPEN_METHODS.contains(&method))
    })
}

/// The chat folder as the agent got it: the first session open's `cwd`.
pub fn session_cwd(records: &[Value]) -> Option<&str> {
    first_open(records)
        .and_then(message)
        .and_then(|msg| msg.pointer("/params/cwd"))
        .and_then(Value::as_str)
        .or_else(|| {
            // Native Claude opens its session through launch arguments, in the process cwd.
            let start = note(records, "start")?;
            (matches!(start["agent_id"].as_str(), Some("claude" | "pi"))
                || matches!(start["dialect"].as_str(), Some("claude" | "pi")))
            .then(|| note(records, "spawn")?.get("cwd")?.as_str())
            .flatten()
        })
}

/// The `session/update` notifications the agent sent with this `sessionUpdate` tag.
pub fn updates<'a>(records: &'a [Value], tag: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
    records.iter().filter(move |record| {
        direction(record) == Some("in")
            && message(record).is_some_and(|msg| {
                msg.get("method").and_then(Value::as_str) == Some("session/update")
                    && msg
                        .pointer("/params/update/sessionUpdate")
                        .and_then(Value::as_str)
                        == Some(tag)
            })
    })
}

/// Requests the driver sent with one of these methods, in order.
pub fn requests_of<'a>(
    records: &'a [Value],
    methods: &'a [&'a str],
) -> impl Iterator<Item = &'a Value> + 'a {
    records.iter().filter(move |record| {
        direction(record) == Some("out")
            && message(record)
                .and_then(request_method)
                .is_some_and(|method| methods.contains(&method))
    })
}

/// Incoming native system events with this subtype.
pub fn native_events<'a>(
    records: &'a [Value],
    subtype: &'a str,
) -> impl Iterator<Item = &'a Value> + 'a {
    records.iter().filter(move |record| {
        direction(record) == Some("in")
            && message(record)
                .is_some_and(|msg| msg["type"] == "system" && msg["subtype"] == subtype)
    })
}

/// Evidence that a run sent or attempted a prompt, including native Claude user frames.
pub fn has_prompt(records: &[Value]) -> bool {
    commands(records).any(|command| command["type"] == "prompt")
        || records.iter().any(|record| {
            direction(record) == Some("out")
                && message(record).is_some_and(|msg| {
                    msg["type"] == "user"
                        || matches!(
                            msg["method"].as_str(),
                            Some("session/prompt" | "turn/start" | "turn/steer")
                        )
                })
        })
}

/// The `cmd` records, in order.
pub fn commands(records: &[Value]) -> impl Iterator<Item = &Value> {
    records
        .iter()
        .filter(|record| direction(record) == Some("cmd"))
        .filter_map(|record| record.get("cmd"))
}

/// Whether any string in the records holds `needle`: a stderr line, a raw line, a message field.
pub fn mentions(records: &[Value], needle: &str) -> bool {
    fn walk(value: &Value, needle: &str) -> bool {
        match value {
            Value::String(text) => text.contains(needle),
            Value::Array(items) => items.iter().any(|item| walk(item, needle)),
            Value::Object(object) => object.values().any(|item| walk(item, needle)),
            Value::Null | Value::Bool(_) | Value::Number(_) => false,
        }
    }
    records.iter().any(|record| walk(record, needle))
}

/// The pid in the spawn note.
pub fn spawn_pid(records: &[Value]) -> Option<u32> {
    note(records, "spawn")?
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn records(lines: &[Value]) -> Vec<Value> {
        lines.to_vec()
    }

    fn fixture(agent: &str, name: &str) -> Vec<Value> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ghost-drivers/tests/fixtures/acp")
            .join(agent)
            .join(format!("{name}.jsonl"));
        read(&path).expect("a fixture of package B").records
    }

    #[test]
    fn exchanges_and_opens_are_found_in_a_real_fixture() {
        let records = fixture("hermes", "legacy-models-modes");
        let (sent, answer) = exchange(&records, "initialize").expect("initialize");
        assert_eq!(message(sent).and_then(|msg| msg.get("id")), Some(&json!(1)));
        assert_eq!(
            message(answer).and_then(|msg| msg.pointer("/result/agentInfo/name")),
            Some(&json!("hermes-agent"))
        );
        let (_, opened) = exchange(&records, "session/new").expect("session/new");
        assert_eq!(
            message(opened).and_then(|msg| msg.pointer("/result/models/currentModelId")),
            Some(&json!("openrouter:anthropic/claude-sonnet-4.5"))
        );
        assert_eq!(session_cwd(&records), Some("<CWD>"));
        assert_eq!(
            note(&records, "start").and_then(|start| start.get("dialect")),
            Some(&json!("hermes"))
        );
        assert!(requests_of(&records, &["session/set_mode", "session/set_model"]).count() >= 2);
        assert!(commands(&records).count() >= 3);
        assert!(exchange(&records, "session/resume").is_none());
    }

    #[test]
    fn replies_match_ids_and_skip_requests_from_the_agent() {
        // The agent's own request with id 2 is not the reply to our request 2.
        let records = records(&[
            json!({"d":"out","msg":{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{}}}),
            json!({"d":"in","msg":{"jsonrpc":"2.0","id":2,"method":"session/request_permission","params":{}}}),
            json!({"d":"in","msg":{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"available_commands_update","availableCommands":[]}}}}),
            json!({"d":"in","msg":{"jsonrpc":"2.0","id":2,"result":{"stopReason":"end_turn"}}}),
            json!({"t":5,"d":"note","note":{"spawn":{"pid":5001}}}),
            json!({"d":"err","line":"a HOSTILE line"}),
        ]);
        let (_, answer) = exchange(&records, "session/prompt").expect("the prompt's reply");
        assert_eq!(
            message(answer).and_then(|msg| msg.pointer("/result/stopReason")),
            Some(&json!("end_turn"))
        );
        assert_eq!(updates(&records, "available_commands_update").count(), 1);
        assert_eq!(spawn_pid(&records), Some(5001));
        assert_eq!(t(&records[4]), Some(5));
        assert!(mentions(&records, "HOSTILE"));
        assert!(!mentions(&records, "nowhere"));
        assert_eq!(session_cwd(&records), None);
    }

    #[test]
    fn native_controls_match_nested_reply_ids_without_matching_agent_requests() {
        let records = records(&[
            json!({"d":"out","msg":{"type":"control_request","request_id":"init","request":{"subtype":"initialize"}}}),
            json!({"d":"in","msg":{"type":"control_request","request_id":"init","request":{"subtype":"can_use_tool"}}}),
            json!({"d":"in","msg":{"type":"control_response","response":{"subtype":"success","request_id":"other","response":{}}}}),
            json!({"d":"in","msg":{"type":"control_response","response":{"subtype":"success","request_id":"init","response":{"claude_code_version":"2.1.310"}}}}),
            json!({"d":"out","msg":{"type":"control_request","request_id":"mode","request":{"subtype":"set_permission_mode","mode":"default"}}}),
            json!({"d":"in","msg":{"type":"control_response","response":{"subtype":"error","request_id":"mode","error":"policy"}}}),
            json!({"d":"in","msg":{"type":"system","subtype":"commands_changed","commands":[]}}),
            json!({"d":"out","msg":{"type":"system","subtype":"commands_changed","commands":[]}}),
            json!({"d":"note","note":{"start":{"agent_id":"claude","dialect":"claude"}}}),
            json!({"d":"note","note":{"spawn":{"cwd":"/work/project"}}}),
        ]);
        let (_, initialized) = exchange(&records, "initialize").expect("native initialize");
        assert_eq!(
            message(initialized).expect("message")["response"]["response"]["claude_code_version"],
            "2.1.310"
        );
        let (_, refused) = exchange(&records, "set_permission_mode").expect("native error");
        assert_eq!(
            message(refused).expect("message")["response"]["error"],
            "policy"
        );
        assert_eq!(requests_of(&records, &["set_permission_mode"]).count(), 1);
        assert_eq!(native_events(&records, "commands_changed").count(), 1);
        assert_eq!(session_cwd(&records), Some("/work/project"));
        assert!(!has_prompt(&records));
    }

    #[test]
    fn notifications_still_count_as_cancels_and_both_prompt_protocols_are_detected() {
        let cancel = vec![json!({"d":"out","msg":{"method":"session/cancel","params":{}}})];
        assert_eq!(requests_of(&cancel, &["session/cancel"]).count(), 1);
        assert!(
            request(&cancel, "session/cancel").is_none(),
            "a notification has no correlated reply"
        );
        assert!(!has_prompt(&cancel));
        for prompt in [
            json!({"d":"out","msg":{"method":"session/prompt","id":1,"params":{}}}),
            json!({"d":"out","msg":{"type":"user","message":{"role":"user","content":"/usage"}}}),
            json!({"d":"cmd","cmd":{"type":"prompt","parts":[]}}),
        ] {
            assert!(has_prompt(&[prompt]));
        }
        assert!(!has_prompt(&[
            json!({"d":"in","msg":{"type":"user","message":{"content":[]}}})
        ]));
    }

    #[test]
    fn rotated_parts_are_read_oldest_first_and_named_by_the_current_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let current = dir.path().join("019a-spawn.jsonl");
        std::fs::write(
            dir.path().join("019a-spawn.2.jsonl"),
            "{\"d\":\"note\",\"n\":1}\n",
        )
        .expect("write");
        std::fs::write(
            dir.path().join("019a-spawn.1.jsonl"),
            "{\"d\":\"note\",\"n\":2}",
        )
        .expect("write");
        std::fs::write(&current, "{\"d\":\"note\",\"n\":3}\nnot json\n").expect("write");
        std::fs::write(dir.path().join("019b-other.jsonl"), "").expect("write");
        let read = read(&current).expect("read");
        let order: Vec<i64> = read
            .records
            .iter()
            .filter_map(|record| record.get("n").and_then(Value::as_i64))
            .collect();
        assert_eq!(order, [1, 2, 3]);
        assert_eq!(read.unreadable, 1);
        let refused = parts(&dir.path().join("019a-spawn.1.jsonl")).expect_err("a part");
        assert!(refused.contains("pass 019a-spawn.jsonl"), "{refused}");
        assert_eq!(
            list(dir.path()),
            [current, dir.path().join("019b-other.jsonl")],
            "current files only, by spawn"
        );
        assert_eq!(list(&dir.path().join("missing")), Vec::<PathBuf>::new());
        // A file that is not named like a log is read alone.
        assert_eq!(
            parts(Path::new("/x/copied.log")),
            Ok(vec![PathBuf::from("/x/copied.log")])
        );
    }
}
