//! The JSON-RPC peer: parsing stdout lines into requests, notifications and responses (batches in
//! order; `jsonrpc` not required), and [`RpcOut`], which writes requests with integer ids counting
//! from 1 per process, notifications and replies through the agent's never-blocking stdin.
//!
//! Ids from the agent are echoed exactly as received; invalid lines are logged, never answered. The
//! same peer serves Codex app-server in M1c, which omits the `jsonrpc` field.

use ghost_process::{LineSink, StdinClosed};
use serde_json::{Map, Value, json};

/// One message from the agent.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: Value,
        result: Result<Value, RpcError>,
    },
    /// Logged, never answered.
    Invalid {
        reason: String,
    },
}

/// A JSON-RPC error, from the agent or for it.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message} ({code})")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

/// The JSON-RPC error codes the ACP driver reads or sends.
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const REQUEST_CANCELLED: i64 = -32800;
    pub const AUTH_REQUIRED: i64 = -32000;
    pub const RESOURCE_NOT_FOUND: i64 = -32002;
}

/// One stdout line: a message, or a batch array (each entry in order). `jsonrpc` is not required.
pub fn parse_line(line: &str) -> Vec<Incoming> {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return vec![Incoming::Invalid {
                reason: format!(
                    "not JSON ({:?} at column {})",
                    error.classify(),
                    error.column()
                ),
            }];
        }
    };
    match value {
        Value::Array(entries) if entries.is_empty() => vec![Incoming::Invalid {
            reason: "an empty batch".into(),
        }],
        Value::Array(entries) => entries.into_iter().map(parse_message).collect(),
        other => vec![parse_message(other)],
    }
}

/// One message of a line or a batch.
fn parse_message(value: Value) -> Incoming {
    let Value::Object(mut fields) = value else {
        return Incoming::Invalid {
            reason: format!("a JSON {} instead of a message", type_name(&value)),
        };
    };
    let id = fields.remove("id");
    if let Some(id) = &id
        && !valid_id(id)
    {
        return Incoming::Invalid {
            reason: format!("an id that is a JSON {}", type_name(id)),
        };
    }
    match fields.remove("method") {
        Some(Value::String(method)) => {
            let params = fields.remove("params").unwrap_or(Value::Null);
            match id {
                Some(id) => Incoming::Request { id, method, params },
                None => Incoming::Notification { method, params },
            }
        }
        Some(other) => Incoming::Invalid {
            reason: format!("a method that is a JSON {}", type_name(&other)),
        },
        None => {
            let error = fields.remove("error").filter(|error| !error.is_null());
            let result = fields.remove("result");
            if error.is_none() && result.is_none() {
                return Incoming::Invalid {
                    reason: "neither a request, a notification nor a response".into(),
                };
            }
            let Some(id) = id else {
                return Incoming::Invalid {
                    reason: "a response without an id".into(),
                };
            };
            let result = match error {
                Some(error) => Err(parse_error(error)),
                None => Ok(result.unwrap_or(Value::Null)),
            };
            Incoming::Response { id, result }
        }
    }
}

/// A response's `error`, read leniently: a malformed one still fails its request.
fn parse_error(error: Value) -> RpcError {
    match error {
        Value::Object(mut fields) => {
            let code = fields
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or(codes::INTERNAL_ERROR);
            let message = match fields.remove("message") {
                Some(Value::String(message)) => message,
                _ => "The agent reported an error without a message.".into(),
            };
            let data = fields.remove("data").filter(|data| !data.is_null());
            RpcError {
                code,
                message,
                data,
            }
        }
        Value::String(message) => RpcError {
            code: codes::INTERNAL_ERROR,
            message,
            data: None,
        },
        other => RpcError {
            code: codes::INTERNAL_ERROR,
            message: "The agent reported an error without a message.".into(),
            data: Some(other),
        },
    }
}

/// JSON-RPC ids are strings, numbers or (discouraged) null.
fn valid_id(id: &Value) -> bool {
    matches!(id, Value::String(_) | Value::Number(_) | Value::Null)
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Our side of the connection: every message goes through the agent's stdin, which never blocks.
#[derive(Debug)]
pub struct RpcOut {
    sink: LineSink,
    next_id: i64,
    jsonrpc_field: bool,
}

impl RpcOut {
    /// `jsonrpc_field: false` omits `"jsonrpc":"2.0"` (Codex app-server).
    pub fn new(sink: LineSink, jsonrpc_field: bool) -> RpcOut {
        RpcOut {
            sink,
            next_id: 1,
            jsonrpc_field,
        }
    }

    /// Sends a request and returns its id: integers from 1, one per request, per process.
    pub fn request(&mut self, method: &str, params: Value) -> Result<i64, StdinClosed> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut message = self.envelope();
        message.insert("id".into(), json!(id));
        message.insert("method".into(), Value::String(method.to_owned()));
        message.insert("params".into(), params);
        self.send(message)?;
        Ok(id)
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), StdinClosed> {
        let mut message = self.envelope();
        message.insert("method".into(), Value::String(method.to_owned()));
        message.insert("params".into(), params);
        self.send(message)
    }

    /// Answers the agent's request `id` (echoed exactly as received).
    pub fn respond(&mut self, id: &Value, result: Value) -> Result<(), StdinClosed> {
        let mut message = self.envelope();
        message.insert("id".into(), id.clone());
        message.insert("result".into(), result);
        self.send(message)
    }

    pub fn respond_error(
        &mut self,
        id: &Value,
        code: i64,
        message: &str,
    ) -> Result<(), StdinClosed> {
        let mut reply = self.envelope();
        reply.insert("id".into(), id.clone());
        reply.insert("error".into(), json!({"code": code, "message": message}));
        self.send(reply)
    }

    /// Closes the agent's stdin after the lines queued so far.
    pub fn close(&self) {
        self.sink.close();
    }

    fn envelope(&self) -> Map<String, Value> {
        let mut message = Map::new();
        if self.jsonrpc_field {
            message.insert("jsonrpc".into(), Value::String("2.0".into()));
        }
        message
    }

    fn send(&self, message: Map<String, Value>) -> Result<(), StdinClosed> {
        self.sink.send(Value::Object(message).to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use ghost_process::testing::{FakeAgentEnd, FakeSpawner};
    use ghost_process::{
        EnvMap, EnvSource, LaunchSpec, MAX_LINE_BYTES, ProcessCwd, ProcessSpawner, ProtocolLog,
        ResolvedEnv, SpawnSpec, SpawnedProcess,
    };

    use super::*;

    fn request(id: Value, method: &str, params: Value) -> Incoming {
        Incoming::Request {
            id,
            method: method.into(),
            params,
        }
    }

    #[test]
    fn requests_notifications_and_results_parse() {
        assert_eq!(
            parse_line(
                r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"a":1}}"#
            ),
            [request(
                json!(7),
                "session/request_permission",
                json!({"a": 1})
            )]
        );
        assert_eq!(
            parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"b":2}}"#),
            [Incoming::Notification {
                method: "session/update".into(),
                params: json!({"b": 2})
            }]
        );
        assert_eq!(
            parse_line(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#),
            [Incoming::Response {
                id: json!(1),
                result: Ok(json!({"protocolVersion": 1}))
            }]
        );
        // A result of null is still a result.
        assert_eq!(
            parse_line(r#"{"id":2,"result":null}"#),
            [Incoming::Response {
                id: json!(2),
                result: Ok(Value::Null)
            }]
        );
        // A request without params gets null params.
        assert_eq!(
            parse_line(r#"{"id":"x","method":"ping"}"#),
            [request(json!("x"), "ping", Value::Null)]
        );
    }

    /// The error of a line holding one failed response.
    fn failure(line: &str) -> RpcError {
        match parse_line(line).as_slice() {
            [
                Incoming::Response {
                    result: Err(error), ..
                },
            ] => error.clone(),
            other => panic!("expected one failed response, got {other:?}"),
        }
    }

    #[test]
    fn errors_parse_with_code_message_and_data() {
        assert_eq!(
            parse_line(
                r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32000,"message":"Authentication required","data":{"x":1}}}"#
            ),
            [Incoming::Response {
                id: json!(3),
                result: Err(RpcError {
                    code: codes::AUTH_REQUIRED,
                    message: "Authentication required".into(),
                    data: Some(json!({"x": 1}))
                })
            }]
        );
        // A malformed error still fails its request.
        let error = failure(r#"{"id":4,"error":{"code":"bad"}}"#);
        assert_eq!(error.code, codes::INTERNAL_ERROR);
        assert!(!error.message.is_empty());
        assert_eq!(
            failure(r#"{"id":5,"error":"plain text"}"#).message,
            "plain text"
        );
        // `error: null` next to a result is a success.
        assert_eq!(
            parse_line(r#"{"id":6,"result":{},"error":null}"#),
            [Incoming::Response {
                id: json!(6),
                result: Ok(json!({}))
            }]
        );
        assert_eq!(
            RpcError {
                code: -32601,
                message: "Method not found".into(),
                data: None
            }
            .to_string(),
            "Method not found (-32601)"
        );
    }

    #[test]
    fn batches_dispatch_in_order() {
        let parsed = parse_line(
            r#"[{"jsonrpc":"2.0","method":"a","params":1},{"jsonrpc":"2.0","id":9,"result":true},{"jsonrpc":"2.0","id":10,"method":"b","params":{}},5]"#,
        );
        assert_eq!(parsed.len(), 4);
        assert!(matches!(&parsed[0], Incoming::Notification { method, .. } if method == "a"));
        assert!(
            matches!(&parsed[1], Incoming::Response { id, result: Ok(Value::Bool(true)) } if *id == json!(9))
        );
        assert!(matches!(&parsed[2], Incoming::Request { method, .. } if method == "b"));
        assert!(matches!(&parsed[3], Incoming::Invalid { .. }));
        assert!(matches!(
            parse_line("[]").as_slice(),
            [Incoming::Invalid { .. }]
        ));
    }

    #[test]
    fn string_numeric_and_null_ids_are_kept_exactly() {
        for (line, id) in [
            (r#"{"id":"abc-1","method":"m"}"#, json!("abc-1")),
            (
                r#"{"id":12345678901,"method":"m"}"#,
                json!(12_345_678_901_i64),
            ),
            (r#"{"id":-3,"method":"m"}"#, json!(-3)),
            (r#"{"id":1.5,"method":"m"}"#, json!(1.5)),
            (r#"{"id":null,"method":"m"}"#, Value::Null),
        ] {
            assert_eq!(
                parse_line(line),
                [request(id.clone(), "m", Value::Null)],
                "{line}"
            );
        }
        // An object or boolean id is not JSON-RPC.
        for line in [
            r#"{"id":{"a":1},"method":"m"}"#,
            r#"{"id":true,"result":1}"#,
        ] {
            assert!(
                matches!(parse_line(line).as_slice(), [Incoming::Invalid { .. }]),
                "{line}"
            );
        }
    }

    #[test]
    fn a_missing_jsonrpc_field_is_accepted() {
        assert_eq!(
            parse_line(r#"{"id":1,"method":"initialize","params":{}}"#),
            [request(json!(1), "initialize", json!({}))]
        );
        assert_eq!(
            parse_line(r#"{"method":"turn/started","params":{"turn":1}}"#),
            [Incoming::Notification {
                method: "turn/started".into(),
                params: json!({"turn": 1})
            }]
        );
        assert_eq!(
            parse_line(r#"{"id":1,"result":{}}"#),
            [Incoming::Response {
                id: json!(1),
                result: Ok(json!({}))
            }]
        );
    }

    #[test]
    fn invalid_lines_say_why() {
        for line in [
            "not json at all",
            "{\"unterminated\":",
            "42",
            "\"text\"",
            "null",
            r#"{"jsonrpc":"2.0"}"#,
            r#"{"method":5}"#,
            r#"{"result":{}}"#,
        ] {
            let parsed = parse_line(line);
            assert!(
                matches!(parsed.as_slice(), [Incoming::Invalid { reason }] if !reason.is_empty()),
                "{line}: {parsed:?}"
            );
        }
    }

    fn spec() -> SpawnSpec {
        SpawnSpec {
            launch: LaunchSpec {
                label: "fake".into(),
                program: PathBuf::from("/bin/fake"),
                args: Vec::new(),
                cwd: ProcessCwd::ChatFolder,
                set_env: Vec::new(),
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(60),
            },
            folder: PathBuf::from("/work"),
            env: Arc::new(ResolvedEnv {
                vars: EnvMap::new(),
                source: EnvSource::Fallback {
                    reason: "test".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            max_line_bytes: MAX_LINE_BYTES,
        }
    }

    async fn pipes() -> (SpawnedProcess, FakeAgentEnd) {
        let (spawner, mut ends) = FakeSpawner::new();
        let process = spawner
            .spawn(spec(), ProtocolLog::disabled())
            .await
            .expect("spawn");
        (process, ends.recv().await.expect("end"))
    }

    async fn next(end: &mut FakeAgentEnd) -> Value {
        let line = end.from_client.recv().await.expect("a line");
        serde_json::from_str(&line).expect("json")
    }

    #[tokio::test]
    async fn requests_count_from_one_and_messages_have_the_right_shape() {
        let (process, mut end) = pipes().await;
        let mut out = RpcOut::new(process.io.stdin.clone(), true);
        assert_eq!(
            out.request("initialize", json!({"protocolVersion": 1})),
            Ok(1)
        );
        assert_eq!(out.request("session/new", json!({})), Ok(2));
        out.notify("session/cancel", json!({"sessionId": "s"}))
            .expect("notify");
        out.respond(
            &json!("agent-7"),
            json!({"outcome": {"outcome": "cancelled"}}),
        )
        .expect("respond");
        out.respond_error(&json!(12), codes::METHOD_NOT_FOUND, "Method not found")
            .expect("respond");
        let first = end.from_client.recv().await.expect("line");
        // Keys keep their order on the wire.
        assert_eq!(
            first,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#
        );
        assert_eq!(
            next(&mut end).await,
            json!({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {}})
        );
        assert_eq!(
            next(&mut end).await,
            json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "s"}})
        );
        assert_eq!(
            next(&mut end).await,
            json!({"jsonrpc": "2.0", "id": "agent-7", "result": {"outcome": {"outcome": "cancelled"}}})
        );
        assert_eq!(
            next(&mut end).await,
            json!({"jsonrpc": "2.0", "id": 12, "error": {"code": -32601, "message": "Method not found"}})
        );
    }

    #[tokio::test]
    async fn without_the_jsonrpc_field_and_after_close() {
        let (process, mut end) = pipes().await;
        let mut out = RpcOut::new(process.io.stdin.clone(), false);
        out.request("turn/start", json!({})).expect("request");
        assert_eq!(
            next(&mut end).await,
            json!({"id": 1, "method": "turn/start", "params": {}})
        );
        out.close();
        assert_eq!(out.notify("x", Value::Null), Err(StdinClosed));
        assert_eq!(out.request("y", Value::Null), Err(StdinClosed));
        assert_eq!(end.from_client.recv().await, None, "stdin reached its end");
    }
}
