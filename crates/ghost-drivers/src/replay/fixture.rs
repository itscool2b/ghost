//! Fixtures: a curated protocol log (the log record format) plus fixture-only `{"d":"agent", ...}`
//! records.
//!
//! ```text
//! {"d":"note","note":{"start":{"agent_id":"hermes","dialect":"hermes","resume":null,"autonomy":"full_auto","desired":{},"spawn_id":"019a..."}}}
//! {"d":"out","msg":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{...}}}
//! {"d":"in","msg":{"jsonrpc":"2.0","id":1,"result":{...}}}
//! {"d":"in","raw":"Debugger listening on ..."}
//! {"d":"err","line":"INFO gateway ready"}
//! {"d":"cmd","cmd":{"type":"prompt","turn_id":"019a...","parts":[{"type":"text","text":"hi"}]}}
//! {"d":"agent","agent":"eof"}
//! {"d":"agent","agent":{"exit":{"code":1}}}
//! {"d":"agent","agent":{"oversized":{"bytes":70000000,"keys":{"method":"session/update"}}}}
//! {"d":"agent","agent":{"wait":{"ms":5000}}}
//! ```
//!
//! `t` is for humans and may be missing; replay uses line order. Other notes (`spawn`, `exit`,
//! `text` for a comment) are kept as [`FixtureRecord::Note`].
//!
//! A record the log capped (spec 3.8: `{"t","d","raw","truncated"}`, whatever its `d`) holds only
//! the first half of its payload, so nothing in it can be replayed: the parser refuses it, naming
//! the line, instead of replaying a capped `in` record as half a line.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::driver::LoggedCommand;

/// A fixture: its name (the file stem) and its records in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Fixture {
    pub name: String,
    pub records: Vec<FixtureRecord>,
}

/// One record of a fixture.
#[derive(Debug, Clone, PartialEq)]
pub enum FixtureRecord {
    /// The `start` note: the session setup, without the env.
    Start(Value),
    /// A message the driver wrote to the agent, compared when replayed.
    Out(Value),
    /// A message the agent wrote.
    In(Value),
    /// A line of stdout that was not JSON.
    InRaw(String),
    /// A stderr line.
    Err(String),
    /// A command the session sent the driver.
    Cmd(LoggedCommand),
    /// What the fake agent does: end stdout, exit, write an oversized line, wait.
    Agent(AgentAction),
    /// Any other note (`spawn`, `exit`, `shutdown`, `text`).
    Note(Value),
}

/// A fixture-only action of the fake agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAction {
    /// stdout reaches its end.
    Eof,
    /// The process exits with `code`.
    Exit { code: i32 },
    /// A line over the cap, as the framer reports one: `keys` holds `id`, `method`, `type`.
    Oversized { bytes: u64, keys: Value },
    /// Virtual time passes.
    Wait { ms: u64 },
}

/// Why a fixture did not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{name}, line {line}: {reason}")]
pub struct FixtureError {
    pub name: String,
    pub line: usize,
    pub reason: String,
}

impl Fixture {
    /// Reads a fixture file; its name is the file stem.
    pub fn load(path: &Path) -> io::Result<Fixture> {
        let text = std::fs::read_to_string(path)?;
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        Fixture::parse(&name, &text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn parse(name: &str, text: &str) -> Result<Fixture, FixtureError> {
        let mut records = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let error = |reason: String| FixtureError {
                name: name.to_owned(),
                line: index + 1,
                reason,
            };
            let value: Value = serde_json::from_str(line)
                .map_err(|error_| error(format!("not JSON: {error_}")))?;
            records.push(record(value).map_err(error)?);
        }
        Ok(Fixture {
            name: name.to_owned(),
            records,
        })
    }

    /// The `start` note's fields.
    pub fn start(&self) -> Option<&Value> {
        self.records.iter().find_map(|record| match record {
            FixtureRecord::Start(start) => Some(start),
            _ => None,
        })
    }
}

fn record(mut value: Value) -> Result<FixtureRecord, String> {
    let direction = value
        .get("d")
        .and_then(Value::as_str)
        .ok_or("no \"d\"")?
        .to_owned();
    if let Some(cut) = value.get("truncated") {
        return Err(format!(
            "a capped \"{direction}\" record ({cut} bytes cut), which cannot be replayed"
        ));
    }
    let mut take = |key: &str| {
        value
            .get_mut(key)
            .map(Value::take)
            .ok_or_else(|| format!("a \"{direction}\" record without \"{key}\""))
    };
    match direction.as_str() {
        "out" => Ok(FixtureRecord::Out(take("msg")?)),
        "in" => match take("msg") {
            Ok(message) => Ok(FixtureRecord::In(message)),
            Err(_) => match take("raw")? {
                Value::String(raw) => Ok(FixtureRecord::InRaw(raw)),
                _ => Err("a raw line that is not a string".into()),
            },
        },
        "err" => match take("line")? {
            Value::String(line) => Ok(FixtureRecord::Err(line)),
            _ => Err("a stderr line that is not a string".into()),
        },
        "cmd" => serde_json::from_value(take("cmd")?)
            .map(FixtureRecord::Cmd)
            .map_err(|error| format!("a command Ghost does not know: {error}")),
        "agent" => serde_json::from_value(take("agent")?)
            .map(FixtureRecord::Agent)
            .map_err(|error| format!("an agent action Ghost does not know: {error}")),
        "note" => {
            let mut note = take("note")?;
            match note.get_mut("start") {
                Some(start) => Ok(FixtureRecord::Start(start.take())),
                None => Ok(FixtureRecord::Note(note)),
            }
        }
        other => Err(format!("an unknown direction \"{other}\"")),
    }
}

#[cfg(test)]
mod tests {
    use ghost_protocol::PromptPart;
    use serde_json::json;

    use super::*;

    #[test]
    fn every_record_kind_parses() {
        let text = r#"
{"t":0,"d":"note","note":{"spawn":{"label":"hermes acp","pid":5001}}}
{"t":1,"d":"note","note":{"start":{"agent_id":"hermes","dialect":"hermes","resume":null,"autonomy":"full_auto","desired":{},"spawn_id":"s"}}}
{"t":2,"d":"out","msg":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}}
{"t":3,"d":"in","msg":{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}}

{"t":4,"d":"in","raw":"Debugger listening"}
{"t":5,"d":"err","line":"INFO gateway ready"}
{"t":6,"d":"cmd","cmd":{"type":"prompt","turn_id":"T","parts":[{"type":"text","text":"hi"}]}}
{"d":"agent","agent":"eof"}
{"d":"agent","agent":{"exit":{"code":1}}}
{"d":"agent","agent":{"oversized":{"bytes":70000000,"keys":{"method":"session/update"}}}}
{"d":"agent","agent":{"wait":{"ms":5000}}}
{"d":"note","note":{"text":"a comment"}}
"#;
        let fixture = Fixture::parse("all", text).expect("parses");
        assert_eq!(fixture.records.len(), 12);
        assert!(
            matches!(&fixture.records[0], FixtureRecord::Note(note) if note.get("spawn").is_some())
        );
        assert_eq!(
            fixture.start().and_then(|start| start.get("dialect")),
            Some(&json!("hermes"))
        );
        assert!(
            matches!(&fixture.records[2], FixtureRecord::Out(message) if message["method"] == "initialize")
        );
        assert!(matches!(&fixture.records[3], FixtureRecord::In(message) if message["id"] == 1));
        assert_eq!(
            fixture.records[4],
            FixtureRecord::InRaw("Debugger listening".into())
        );
        assert_eq!(
            fixture.records[5],
            FixtureRecord::Err("INFO gateway ready".into())
        );
        assert_eq!(
            fixture.records[6],
            FixtureRecord::Cmd(LoggedCommand::Prompt {
                turn_id: "T".into(),
                parts: vec![PromptPart::Text { text: "hi".into() }]
            })
        );
        assert_eq!(fixture.records[7], FixtureRecord::Agent(AgentAction::Eof));
        assert_eq!(
            fixture.records[8],
            FixtureRecord::Agent(AgentAction::Exit { code: 1 })
        );
        assert!(matches!(
            &fixture.records[9],
            FixtureRecord::Agent(AgentAction::Oversized {
                bytes: 70_000_000,
                ..
            })
        ));
        assert_eq!(
            fixture.records[10],
            FixtureRecord::Agent(AgentAction::Wait { ms: 5000 })
        );
    }

    #[test]
    fn bad_lines_name_the_fixture_and_the_line() {
        for (text, needle) in [
            ("{\"d\":\"out\"}", "without \"msg\""),
            ("not json", "not JSON"),
            ("{\"msg\":{}}", "no \"d\""),
            ("{\"d\":\"sideways\",\"msg\":{}}", "unknown direction"),
            ("{\"d\":\"cmd\",\"cmd\":{\"type\":\"teleport\"}}", "command"),
            ("{\"d\":\"agent\",\"agent\":\"explode\"}", "agent action"),
            ("{\"d\":\"err\",\"line\":5}", "not a string"),
        ] {
            let error = Fixture::parse("broken", &format!("\n{text}")).expect_err("fails");
            assert_eq!((error.name.as_str(), error.line), ("broken", 2), "{text}");
            assert!(error.reason.contains(needle), "{text}: {}", error.reason);
        }
    }

    /// The capped shape of spec 3.8, for every `d` the log writes: the record's own `d`, the first
    /// half of its payload as text in `raw`, and the bytes cut. An `in` record with `raw` and no
    /// `truncated` is still a line that was not JSON.
    #[test]
    fn capped_records_are_refused_for_every_direction() {
        for direction in ["out", "in", "err", "cmd", "note"] {
            let first = json!({"t": 1, "d": "note", "note": {"text": "first"}});
            let capped = json!({"t": 2, "d": direction,
                "raw": "{\"jsonrpc\":\"2.0\",\"method\":\"session/upd", "truncated": 131_072});
            let text = format!("{first}\n{capped}\n");
            let error = Fixture::parse("capped", &text).expect_err("refused");
            assert_eq!(
                (error.name.as_str(), error.line),
                ("capped", 2),
                "{direction}"
            );
            assert!(
                error
                    .reason
                    .contains(&format!("capped \"{direction}\" record"))
                    && error.reason.contains("131072 bytes cut")
                    && error.reason.contains("cannot be replayed"),
                "{direction}: {}",
                error.reason
            );
        }
        let fixture = Fixture::parse("raw", "{\"d\":\"in\",\"raw\":\"Debugger listening\"}\n")
            .expect("a raw line without truncated still parses");
        assert_eq!(
            fixture.records,
            [FixtureRecord::InRaw("Debugger listening".into())]
        );
    }

    #[test]
    fn loading_names_the_fixture_after_its_file() {
        let dir = std::env::temp_dir().join(format!("aura-fixture-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("turn-basic.jsonl");
        std::fs::write(&path, "{\"d\":\"agent\",\"agent\":\"eof\"}\n").expect("write");
        let fixture = Fixture::load(&path).expect("loads");
        assert_eq!(fixture.name, "turn-basic");
        std::fs::write(&path, "nope\n").expect("write");
        assert_eq!(
            Fixture::load(&path).expect_err("invalid").kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
