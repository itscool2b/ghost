//! What Ghost sends an ACP agent, exactly (spec 5.3.1): no `_meta`, no `systemPrompt`, no
//! `additionalDirectories`, `mcpServers` always empty, `fs` and `terminal` off, terminal sign-in
//! advertised. The conformance tests compare every builder with the schema crate's own.

use ghost_protocol::session::ConfigValue;
use serde_json::{Value, json};

use crate::driver::PermissionOutcome;

/// The only ACP protocol version Ghost speaks.
pub const PROTOCOL_VERSION: u64 = 1;

/// ACP method names.
pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const SESSION_NEW: &str = "session/new";
    pub const SESSION_LOAD: &str = "session/load";
    pub const SESSION_RESUME: &str = "session/resume";
    pub const SESSION_PROMPT: &str = "session/prompt";
    pub const SESSION_CANCEL: &str = "session/cancel";
    pub const SESSION_SET_CONFIG_OPTION: &str = "session/set_config_option";
    pub const SESSION_SET_MODE: &str = "session/set_mode";
    /// Legacy and untyped: the schema no longer has it; Hermes still takes it.
    pub const SESSION_SET_MODEL: &str = "session/set_model";
    pub const SESSION_UPDATE: &str = "session/update";
    pub const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";
    pub const CANCEL_REQUEST: &str = "$/cancel_request";
    pub const AUTH_STATUS_UPDATE: &str = "_auth/status_update";
}

/// `initialize`, with `version` as `clientInfo.version`.
pub fn initialize(version: &str) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
            "session": {"notices": {}, "configOptions": {"boolean": {}}},
            "auth": {"terminal": true},
            // Display-only terminals (terminals.rs): the agent runs a command and streams its output.
            "_meta": {"terminal_output": true},
        },
        "clientInfo": {"name": "ghost", "title": "Ghost ADE", "version": version},
    })
}

pub fn new_session(cwd: &str) -> Value {
    json!({"cwd": cwd, "mcpServers": []})
}

pub fn resume_session(session_id: &str, cwd: &str) -> Value {
    json!({"sessionId": session_id, "cwd": cwd, "mcpServers": []})
}

pub fn load_session(session_id: &str, cwd: &str) -> Value {
    json!({"sessionId": session_id, "cwd": cwd, "mcpServers": []})
}

/// `session/prompt` with one text block per text.
pub fn prompt<S: AsRef<str>>(session_id: &str, texts: &[S]) -> Value {
    let blocks: Vec<Value> = texts
        .iter()
        .map(|text| json!({"type": "text", "text": text.as_ref()}))
        .collect();
    json!({"sessionId": session_id, "prompt": blocks})
}

/// `session/cancel`, a notification.
pub fn cancel(session_id: &str) -> Value {
    json!({"sessionId": session_id})
}

/// `session/set_config_option`: a choice sends its value id, a toggle `"type": "boolean"` and the
/// flag.
pub fn set_config_option(session_id: &str, config_id: &str, value: &ConfigValue) -> Value {
    match value {
        ConfigValue::Choice { value } => {
            json!({"sessionId": session_id, "configId": config_id, "value": value})
        }
        ConfigValue::Toggle { value } => {
            json!({"sessionId": session_id, "configId": config_id, "type": "boolean", "value": value})
        }
    }
}

pub fn set_mode(session_id: &str, mode_id: &str) -> Value {
    json!({"sessionId": session_id, "modeId": mode_id})
}

/// The legacy, untyped `session/set_model`.
pub fn set_model(session_id: &str, model_id: &str) -> Value {
    json!({"sessionId": session_id, "modelId": model_id})
}

/// The reply to `session/request_permission`.
pub fn permission_reply(outcome: &PermissionOutcome) -> Value {
    match outcome {
        PermissionOutcome::Selected(option_id) => {
            json!({"outcome": {"outcome": "selected", "optionId": option_id}})
        }
        PermissionOutcome::Cancelled => json!({"outcome": {"outcome": "cancelled"}}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_the_spec_shape_in_order() {
        assert_eq!(
            initialize("0.1.0").to_string(),
            r#"{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false,"session":{"notices":{},"configOptions":{"boolean":{}}},"auth":{"terminal":true},"_meta":{"terminal_output":true}},"clientInfo":{"name":"ghost","title":"Ghost ADE","version":"0.1.0"}}"#
        );
        assert_eq!(
            new_session("/w").to_string(),
            r#"{"cwd":"/w","mcpServers":[]}"#
        );
        assert_eq!(
            resume_session("s", "/w").to_string(),
            r#"{"sessionId":"s","cwd":"/w","mcpServers":[]}"#
        );
        assert_eq!(
            load_session("s", "/w").to_string(),
            r#"{"sessionId":"s","cwd":"/w","mcpServers":[]}"#
        );
        assert_eq!(
            prompt("s", &["one", "two"]).to_string(),
            r#"{"sessionId":"s","prompt":[{"type":"text","text":"one"},{"type":"text","text":"two"}]}"#
        );
        assert_eq!(cancel("s").to_string(), r#"{"sessionId":"s"}"#);
        assert_eq!(
            set_config_option(
                "s",
                "model",
                &ConfigValue::Choice {
                    value: "fast".into()
                }
            )
            .to_string(),
            r#"{"sessionId":"s","configId":"model","value":"fast"}"#
        );
        assert_eq!(
            set_config_option("s", "fast", &ConfigValue::Toggle { value: true }).to_string(),
            r#"{"sessionId":"s","configId":"fast","type":"boolean","value":true}"#
        );
        assert_eq!(
            set_mode("s", "dont_ask").to_string(),
            r#"{"sessionId":"s","modeId":"dont_ask"}"#
        );
        assert_eq!(
            set_model("s", "openrouter:a").to_string(),
            r#"{"sessionId":"s","modelId":"openrouter:a"}"#
        );
        assert_eq!(
            permission_reply(&PermissionOutcome::Selected("allow".into())).to_string(),
            r#"{"outcome":{"outcome":"selected","optionId":"allow"}}"#
        );
        assert_eq!(
            permission_reply(&PermissionOutcome::Cancelled).to_string(),
            r#"{"outcome":{"outcome":"cancelled"}}"#
        );
    }
}
