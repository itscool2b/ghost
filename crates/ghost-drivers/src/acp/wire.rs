//! Lenient inbound ACP types: the tag of an update is read as a string first, then its fields
//! leniently, so an unknown update or a malformed one costs only itself.
//!
//! Nothing here deserializes through serde's derive: one field of an unexpected type (a number
//! where a title belongs) drops that field, not the whole message, and enums agents extend read
//! unknown names as their fallback.

use ghost_protocol::{
    Cost, NoticeLevel, PermissionOption, PermissionOptionKind, PlanEntry, PlanPriority, PlanStatus,
    StopReason, ToolContent, ToolKind, ToolLocation, ToolStatus, UsageInfo,
};
use serde_json::{Map, Value};

/// The string at `key`, when it is one.
pub fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The string at `key`, owned.
pub fn string_at(value: &Value, key: &str) -> Option<String> {
    str_at(value, key).map(str::to_owned)
}

/// Whether `key` holds anything but `null` (an empty object counts: ACP capabilities are
/// advertised by presence).
fn present(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(|value| !value.is_null())
}

fn bool_at(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn u64_at(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

/// What `initialize` answered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitializeReply {
    pub mcp_stdio: bool,
    pub mcp_http: bool,
    pub mcp_sse: bool,
    /// The raw value, so a mismatch can say what the agent speaks.
    pub protocol_version: Option<Value>,
    pub load_session: bool,
    /// `sessionCapabilities.resume` is advertised.
    pub resume: bool,
    /// `sessionCapabilities.additionalDirectories` is advertised.
    pub additional_directories: bool,
    /// `promptCapabilities.image`.
    pub image: bool,
    pub auth_methods: Vec<AuthMethod>,
    pub agent_name: Option<String>,
    pub agent_title: Option<String>,
    pub agent_version: Option<String>,
}

impl InitializeReply {
    pub fn parse(result: &Value) -> InitializeReply {
        let capabilities = result.get("agentCapabilities").unwrap_or(&Value::Null);
        let session = capabilities
            .get("sessionCapabilities")
            .unwrap_or(&Value::Null);
        let prompt = capabilities
            .get("promptCapabilities")
            .unwrap_or(&Value::Null);
        let info = result.get("agentInfo").unwrap_or(&Value::Null);
        let auth_methods = result
            .get("authMethods")
            .and_then(Value::as_array)
            .map(|methods| methods.iter().filter_map(AuthMethod::parse).collect())
            .unwrap_or_default();
        InitializeReply {
            mcp_stdio: capabilities["mcpCapabilities"]["stdio"]
                .as_bool()
                .unwrap_or(true),
            mcp_http: capabilities["mcpCapabilities"]["http"]
                .as_bool()
                .unwrap_or(false),
            mcp_sse: capabilities["mcpCapabilities"]["sse"]
                .as_bool()
                .unwrap_or(false),
            protocol_version: result
                .get("protocolVersion")
                .filter(|version| !version.is_null())
                .cloned(),
            load_session: bool_at(capabilities, "loadSession"),
            resume: present(session, "resume"),
            additional_directories: present(session, "additionalDirectories"),
            image: bool_at(prompt, "image"),
            auth_methods,
            agent_name: string_at(info, "name"),
            agent_title: string_at(info, "title"),
            agent_version: string_at(info, "version"),
        }
    }

    /// The protocol version is 1, the only one Ghost speaks.
    pub fn speaks_version_1(&self) -> bool {
        self.protocol_version
            .as_ref()
            .and_then(Value::as_u64)
            .is_some_and(|version| version == 1)
    }

    /// How the version reads in the mismatch message.
    pub fn version_text(&self) -> String {
        match &self.protocol_version {
            Some(Value::Number(number)) => number.to_string(),
            Some(Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
            None => "no".into(),
        }
    }
}

/// One advertised sign-in method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: AuthMethodKind,
    /// A terminal method's extra arguments for the agent's own command.
    pub args: Vec<String>,
    /// A terminal method's variables, sorted by name.
    pub env: Vec<(String, String)>,
}

/// The `type` of an auth method. No `type` means `agent` (schema 1.12.0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethodKind {
    Agent,
    Terminal,
    Other(String),
}

impl AuthMethod {
    pub fn parse(value: &Value) -> Option<AuthMethod> {
        let id = string_at(value, "id")?;
        let kind = match str_at(value, "type") {
            None | Some("agent") => AuthMethodKind::Agent,
            Some("terminal") => AuthMethodKind::Terminal,
            Some(other) => AuthMethodKind::Other(other.to_owned()),
        };
        let args = value
            .get("args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let mut env: Vec<(String, String)> = value
            .get("env")
            .and_then(Value::as_object)
            .map(|env| {
                env.iter()
                    .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        env.sort();
        Some(AuthMethod {
            name: string_at(value, "name").unwrap_or_else(|| id.clone()),
            id,
            description: string_at(value, "description"),
            kind,
            args,
            env,
        })
    }
}

/// What `session/new`, `session/load` or `session/resume` answered.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenReply {
    /// `session/new` names the session; load and resume replies need not.
    pub session_id: Option<String>,
    /// The raw `configOptions` entries; `None` when the reply had none.
    pub config_options: Option<Vec<Value>>,
    pub modes: Option<LegacyList>,
    /// The legacy `models` list, read from the raw reply (the schema type drops it).
    pub models: Option<LegacyList>,
}

impl OpenReply {
    pub fn parse(result: &Value) -> OpenReply {
        OpenReply {
            session_id: string_at(result, "sessionId"),
            config_options: result
                .get("configOptions")
                .and_then(Value::as_array)
                .cloned(),
            modes: result.get("modes").and_then(|modes| {
                LegacyList::parse(modes, "currentModeId", "availableModes", "id")
            }),
            models: result.get("models").and_then(|models| {
                LegacyList::parse(models, "currentModelId", "availableModels", "modelId")
            }),
        }
    }
}

/// A legacy `modes` or `models` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyList {
    pub current: String,
    pub available: Vec<LegacyChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyChoice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

impl LegacyList {
    fn parse(value: &Value, current: &str, available: &str, id_key: &str) -> Option<LegacyList> {
        let current = string_at(value, current)?;
        let available = value
            .get(available)?
            .as_array()?
            .iter()
            .filter_map(|entry| {
                let id = string_at(entry, id_key)?;
                Some(LegacyChoice {
                    name: string_at(entry, "name").unwrap_or_else(|| id.clone()),
                    id,
                    description: string_at(entry, "description"),
                })
            })
            .collect();
        Some(LegacyList { current, available })
    }
}

/// A `session/update` notification's own fields.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateNotification<'a> {
    pub session_id: Option<&'a str>,
    pub update: &'a Value,
}

impl<'a> UpdateNotification<'a> {
    /// `None` when the params hold no `update` object.
    pub fn parse(params: &'a Value) -> Option<UpdateNotification<'a>> {
        let update = params.get("update").filter(|update| update.is_object())?;
        Some(UpdateNotification {
            session_id: str_at(params, "sessionId"),
            update,
        })
    }
}

/// One `session/update`, by `update.sessionUpdate`.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    UserMessageChunk(Chunk),
    AgentMessageChunk(Chunk),
    AgentThoughtChunk(Chunk),
    ToolCall(ToolFields),
    ToolCallUpdate(ToolFields),
    Plan(Vec<PlanEntry>),
    AvailableCommands(Vec<CommandEntry>),
    CurrentMode(String),
    ConfigOptions(Vec<Value>),
    SessionInfo {
        title: TitleChange,
    },
    Usage(UsageInfo),
    Notice {
        level: NoticeLevel,
        message: String,
    },
    /// A tag Ghost knows and does not map (compaction, unstable kinds it does not advertise).
    Unmapped(&'static str),
    /// A tag Ghost does not know.
    Unknown(String),
}

/// An update Ghost could not read: dropped, logged, and the stream goes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed {
    /// The tag, when there was one.
    pub tag: Option<String>,
    pub reason: &'static str,
}

/// The tags Ghost knows and leaves unmapped: not advertised, or unstable.
pub const UNMAPPED_TAGS: &[&str] = &[
    "compaction_update",
    "compaction_summary_chunk",
    "plan_update",
    "plan_removed",
    "subagent_update",
    "session_message",
    "session_message_chunk",
];

impl Update {
    pub fn parse(update: &Value) -> Result<Update, Malformed> {
        let Some(tag) = str_at(update, "sessionUpdate") else {
            return Err(Malformed {
                tag: None,
                reason: "no sessionUpdate tag",
            });
        };
        let malformed = |reason| Malformed {
            tag: Some(tag.to_owned()),
            reason,
        };
        let parsed = match tag {
            "user_message_chunk" => Update::UserMessageChunk(
                Chunk::parse(update).ok_or_else(|| malformed("no content"))?,
            ),
            "agent_message_chunk" => Update::AgentMessageChunk(
                Chunk::parse(update).ok_or_else(|| malformed("no content"))?,
            ),
            "agent_thought_chunk" => Update::AgentThoughtChunk(
                Chunk::parse(update).ok_or_else(|| malformed("no content"))?,
            ),
            "tool_call" => Update::ToolCall(
                ToolFields::parse(update).ok_or_else(|| malformed("no toolCallId"))?,
            ),
            "tool_call_update" => Update::ToolCallUpdate(
                ToolFields::parse(update).ok_or_else(|| malformed("no toolCallId"))?,
            ),
            "plan" => {
                let entries = update
                    .get("entries")
                    .and_then(Value::as_array)
                    .ok_or_else(|| malformed("no entries"))?;
                Update::Plan(entries.iter().filter_map(plan_entry).collect())
            }
            "available_commands_update" => {
                let commands = update
                    .get("availableCommands")
                    .and_then(Value::as_array)
                    .ok_or_else(|| malformed("no availableCommands"))?;
                Update::AvailableCommands(commands.iter().filter_map(CommandEntry::parse).collect())
            }
            "current_mode_update" => Update::CurrentMode(
                string_at(update, "currentModeId").ok_or_else(|| malformed("no currentModeId"))?,
            ),
            "config_option_update" => Update::ConfigOptions(
                update
                    .get("configOptions")
                    .and_then(Value::as_array)
                    .cloned()
                    .ok_or_else(|| malformed("no configOptions"))?,
            ),
            "session_info_update" => Update::SessionInfo {
                title: match update.get("title") {
                    None => TitleChange::Keep,
                    Some(Value::Null) => TitleChange::Clear,
                    Some(Value::String(title)) => TitleChange::Set(title.clone()),
                    Some(_) => return Err(malformed("a title that is not a string")),
                },
            },
            "usage_update" => {
                let used = u64_at(update, "used");
                let size = u64_at(update, "size");
                if used.is_none() && size.is_none() {
                    return Err(malformed("no used or size"));
                }
                Update::Usage(UsageInfo {
                    context_used: used,
                    context_size: size,
                    cost: update.get("cost").and_then(cost),
                })
            }
            "notice" => {
                let title = str_at(update, "title").ok_or_else(|| malformed("no title"))?;
                let message = match str_at(update, "description") {
                    Some(description) if !description.is_empty() => {
                        format!("{title}: {description}")
                    }
                    _ => title.to_owned(),
                };
                Update::Notice {
                    level: str_at(update, "severity")
                        .map_or(NoticeLevel::Info, NoticeLevel::from_wire),
                    message,
                }
            }
            other => match UNMAPPED_TAGS.iter().find(|known| **known == other) {
                Some(known) => Update::Unmapped(known),
                None => Update::Unknown(other.to_owned()),
            },
        };
        Ok(parsed)
    }

    /// Transcript kinds close an open message or thought and are replay during a load or resume;
    /// state kinds do neither.
    pub fn is_transcript(&self) -> bool {
        matches!(
            self,
            Update::UserMessageChunk(_)
                | Update::AgentMessageChunk(_)
                | Update::AgentThoughtChunk(_)
                | Update::ToolCall(_)
                | Update::ToolCallUpdate(_)
                | Update::Plan(_)
        )
    }
}

/// A message or thought chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// The content block as text (see [`content_text`]).
    pub text: String,
    pub message_id: Option<String>,
}

impl Chunk {
    fn parse(update: &Value) -> Option<Chunk> {
        let content = update
            .get("content")
            .filter(|content| content.is_object())?;
        Some(Chunk {
            text: content_text(content),
            message_id: string_at(update, "messageId"),
        })
    }
}

/// `session_info_update`'s title: a string sets it, `null` clears it, absent keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TitleChange {
    Keep,
    Set(String),
    Clear,
}

/// One entry of `available_commands_update`, before the limits apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    pub name: String,
    pub description: String,
    pub hint: Option<String>,
}

impl CommandEntry {
    fn parse(value: &Value) -> Option<CommandEntry> {
        let name = string_at(value, "name").filter(|name| !name.is_empty())?;
        Some(CommandEntry {
            name,
            description: string_at(value, "description").unwrap_or_default(),
            hint: value
                .get("input")
                .and_then(|input| string_at(input, "hint")),
        })
    }
}

/// The fields of a `tool_call`, a `tool_call_update`, or a permission request's `toolCall`.
/// `None` means absent (an update keeps that field).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolFields {
    pub tool_call_id: String,
    pub title: Option<String>,
    pub name: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    pub content: Option<Vec<ToolContent>>,
    pub locations: Option<Vec<ToolLocation>>,
    pub raw_input: Option<Value>,
    /// Content entries of a type Ghost does not show, dropped.
    pub dropped_content: usize,
    /// The content entries that are display terminals: their position and terminal id.
    pub terminals: Vec<(usize, String)>,
    /// The update's terminal `_meta` (terminals.rs).
    pub terminal_meta: Vec<super::terminals::TerminalMeta>,
}

impl ToolFields {
    pub fn parse(value: &Value) -> Option<ToolFields> {
        let tool_call_id = string_at(value, "toolCallId")?;
        let mut dropped_content = 0;
        let mut terminals = Vec::new();
        let content = value
            .get("content")
            .and_then(Value::as_array)
            .map(|entries| {
                let mut content = Vec::with_capacity(entries.len());
                for entry in entries {
                    // A display terminal: its output arrives on later updates' `_meta`.
                    if str_at(entry, "type") == Some("terminal") {
                        match str_at(entry, "terminalId").filter(|id| !id.is_empty()) {
                            Some(id) => {
                                terminals.push((content.len(), id.to_owned()));
                                content.push(ToolContent::Terminal {
                                    output: String::new(),
                                    exit_code: None,
                                    truncated: None,
                                });
                            }
                            None => dropped_content += 1,
                        }
                        continue;
                    }
                    match tool_content(entry) {
                        Some(item) => content.push(item),
                        None => dropped_content += 1,
                    }
                }
                content
            });
        Some(ToolFields {
            tool_call_id,
            title: string_at(value, "title"),
            name: string_at(value, "name"),
            kind: str_at(value, "kind").map(ToolKind::from_wire),
            status: str_at(value, "status").map(ToolStatus::from_wire),
            content,
            locations: value
                .get("locations")
                .and_then(Value::as_array)
                .map(|locations| locations.iter().filter_map(location).collect()),
            raw_input: value
                .get("rawInput")
                .filter(|input| !input.is_null())
                .cloned(),
            dropped_content,
            terminals,
            terminal_meta: super::terminals::meta(value),
        })
    }
}

/// One tool content entry; `None` for a type Ghost does not show.
fn tool_content(entry: &Value) -> Option<ToolContent> {
    match str_at(entry, "type")? {
        "content" => Some(ToolContent::Text {
            text: content_text(entry.get("content")?),
            truncated: None,
        }),
        "diff" => Some(ToolContent::Diff {
            path: string_at(entry, "path")?,
            old_text: string_at(entry, "oldText"),
            new_text: string_at(entry, "newText")?,
            truncated: None,
        }),
        _ => None,
    }
}

fn location(value: &Value) -> Option<ToolLocation> {
    Some(ToolLocation {
        path: string_at(value, "path")?,
        line: u64_at(value, "line").and_then(|line| u32::try_from(line).ok()),
    })
}

fn plan_entry(value: &Value) -> Option<PlanEntry> {
    Some(PlanEntry {
        content: string_at(value, "content")?,
        status: str_at(value, "status").map_or(PlanStatus::Pending, PlanStatus::from_wire),
        priority: match str_at(value, "priority") {
            Some("high") => Some(PlanPriority::High),
            Some("medium") => Some(PlanPriority::Medium),
            Some("low") => Some(PlanPriority::Low),
            _ => None,
        },
    })
}

fn cost(value: &Value) -> Option<Cost> {
    Some(Cost {
        amount: value.get("amount")?.as_f64()?,
        currency: string_at(value, "currency")?,
    })
}

/// A content block as text: `text` its text; `image` and `audio` a placeholder with the type and
/// size; `resource_link` a markdown link; `resource` its text, or a placeholder for a blob; any
/// other type `[<type>]`.
pub fn content_text(block: &Value) -> String {
    let kind = str_at(block, "type").unwrap_or("content");
    match kind {
        "text" => string_at(block, "text").unwrap_or_default(),
        "image" if str_at(block, "uri").is_some() => {
            crate::image_reference("Agent image", str_at(block, "uri").unwrap_or_default())
        }
        "image" | "audio" => format!(
            "[{kind}: {}, {} KB]",
            str_at(block, "mimeType").unwrap_or("unknown type"),
            kilobytes(str_at(block, "data").unwrap_or_default())
        ),
        "resource_link" => {
            let label = str_at(block, "title")
                .filter(|title| !title.is_empty())
                .or_else(|| str_at(block, "name"))
                .unwrap_or("resource");
            let uri = str_at(block, "uri").unwrap_or_default();
            if str_at(block, "mimeType").is_some_and(|mime| mime.starts_with("image/")) {
                crate::image_reference(label, uri)
            } else {
                format!("[{label}]({uri})")
            }
        }
        "resource" => {
            let resource = block.get("resource").unwrap_or(&Value::Null);
            match str_at(resource, "text") {
                Some(text) => text.to_owned(),
                None if str_at(resource, "mimeType")
                    .is_some_and(|mime| mime.starts_with("image/")) =>
                {
                    crate::image_reference(
                        "Agent image",
                        str_at(resource, "uri").unwrap_or_default(),
                    )
                }
                None => format!("[resource {}]", str_at(resource, "uri").unwrap_or_default()),
            }
        }
        other => format!("[{other}]"),
    }
}

/// The decoded size of base64 `data`, in KB rounded up.
fn kilobytes(data: &str) -> u64 {
    let padding = data.bytes().rev().take_while(|byte| *byte == b'=').count();
    let bytes = (data.len() / 4 * 3 + data.len() % 4 * 3 / 4).saturating_sub(padding);
    u64::try_from(bytes.div_ceil(1024)).unwrap_or(u64::MAX)
}

/// A `session/request_permission` request's params.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionParams {
    pub session_id: Option<String>,
    pub tool: ToolFields,
    pub options: Vec<PermissionOption>,
}

impl PermissionParams {
    /// `Err` with the reason when the params do not parse (answered -32602).
    pub fn parse(params: &Value) -> Result<PermissionParams, &'static str> {
        if !params.is_object() {
            return Err("the params are not an object");
        }
        let tool = params
            .get("toolCall")
            .filter(|tool| tool.is_object())
            .ok_or("no toolCall object")?;
        let options = params
            .get("options")
            .and_then(Value::as_array)
            .ok_or("no options list")?;
        let tool = ToolFields::parse(tool).unwrap_or_else(|| {
            // A toolCall without an id still asks; it names no block.
            let mut fields = ToolFields::parse(&with_id(tool)).unwrap_or_default();
            fields.tool_call_id.clear();
            fields
        });
        let options = options
            .iter()
            .filter_map(|option| {
                let option_id = string_at(option, "optionId")?;
                Some(PermissionOption {
                    label: string_at(option, "name").unwrap_or_else(|| option_id.clone()),
                    kind: str_at(option, "kind")
                        .map_or(PermissionOptionKind::Other, PermissionOptionKind::from_wire),
                    option_id,
                })
            })
            .collect();
        Ok(PermissionParams {
            session_id: string_at(params, "sessionId"),
            tool,
            options,
        })
    }
}

/// `tool` with an empty `toolCallId`, so its other fields parse.
fn with_id(tool: &Value) -> Value {
    let mut fields = tool.as_object().cloned().unwrap_or_else(Map::new);
    fields.insert("toolCallId".into(), Value::String(String::new()));
    Value::Object(fields)
}

/// What `session/prompt` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptReply {
    pub stop_reason: StopReason,
    /// The agent's own name for a reason Ghost reads as `Other`, for the log.
    pub unknown_reason: Option<String>,
    pub usage: Option<WireUsage>,
}

impl PromptReply {
    pub fn parse(result: &Value) -> PromptReply {
        let (stop_reason, unknown_reason) = match str_at(result, "stopReason") {
            Some("end_turn") => (StopReason::EndTurn, None),
            Some("max_tokens") => (StopReason::MaxTokens, None),
            Some("max_turn_requests") => (StopReason::MaxTurnRequests, None),
            Some("refusal") => (StopReason::Refusal, None),
            Some("cancelled") => (StopReason::Cancelled, None),
            Some(other) => (StopReason::Other, Some(other.to_owned())),
            None => (StopReason::Other, Some("(none)".into())),
        };
        PromptReply {
            stop_reason,
            unknown_reason,
            usage: result.get("usage").and_then(WireUsage::parse),
        }
    }
}

/// `PromptResponse.usage` (an unstable field, read raw).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WireUsage {
    pub input: u64,
    pub output: u64,
    pub cached_read: Option<u64>,
    pub cached_write: Option<u64>,
    pub thought: Option<u64>,
}

impl WireUsage {
    pub fn parse(value: &Value) -> Option<WireUsage> {
        let input = u64_at(value, "inputTokens");
        let output = u64_at(value, "outputTokens");
        if input.is_none() && output.is_none() {
            return None;
        }
        Some(WireUsage {
            input: input.unwrap_or(0),
            output: output.unwrap_or(0),
            cached_read: u64_at(value, "cachedReadTokens"),
            cached_write: u64_at(value, "cachedWriteTokens"),
            thought: u64_at(value, "thoughtTokens"),
        })
    }
}

/// What `session/set_config_option` answered: the complete list, when the reply had one.
pub fn config_options_reply(result: &Value) -> Option<Vec<Value>> {
    result
        .get("configOptions")
        .and_then(Value::as_array)
        .cloned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn initialize_replies_are_read_leniently() {
        let reply = InitializeReply::parse(&json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {"image": true, "embeddedContext": true},
                "sessionCapabilities": {"resume": {}, "additionalDirectories": {}, "list": {}},
            },
            "authMethods": [
                {"id": "claude-ai-login", "name": "Claude Subscription", "type": "terminal",
                 "args": ["--cli", "auth", "login", "--claudeai"], "env": {"B": "2", "A": "1"}},
                {"id": "api-key", "name": "API Key"},
                {"id": "future", "name": "Future", "type": "browser"},
                {"name": "no id"},
            ],
            "agentInfo": {"name": "hermes-agent", "title": "Hermes", "version": "0.22.5"},
        }));
        assert!(reply.speaks_version_1());
        assert!(reply.load_session && reply.resume && reply.additional_directories && reply.image);
        assert_eq!(reply.agent_name.as_deref(), Some("hermes-agent"));
        assert_eq!(reply.agent_version.as_deref(), Some("0.22.5"));
        assert_eq!(reply.auth_methods.len(), 3);
        assert_eq!(reply.auth_methods[0].kind, AuthMethodKind::Terminal);
        assert_eq!(
            reply.auth_methods[0].env,
            [
                ("A".to_owned(), "1".to_owned()),
                ("B".to_owned(), "2".to_owned())
            ]
        );
        assert_eq!(reply.auth_methods[1].kind, AuthMethodKind::Agent);
        assert_eq!(
            reply.auth_methods[2].kind,
            AuthMethodKind::Other("browser".into())
        );

        // Everything optional may be missing or of the wrong type.
        let bare = InitializeReply::parse(&json!({"protocolVersion": "2", "agentCapabilities": 7}));
        assert!(!bare.speaks_version_1());
        assert_eq!(bare.version_text(), "2");
        assert!(!bare.load_session && !bare.resume && bare.auth_methods.is_empty());
        assert_eq!(InitializeReply::parse(&json!({})).version_text(), "no");
        // `resume: null` is not an advertisement.
        assert!(
            !InitializeReply::parse(
                &json!({"agentCapabilities": {"sessionCapabilities": {"resume": null}}})
            )
            .resume
        );
    }

    #[test]
    fn open_replies_keep_options_and_legacy_lists() {
        let reply = OpenReply::parse(&json!({
            "sessionId": "s1",
            "configOptions": [{"id": "model"}],
            "modes": {"currentModeId": "default", "availableModes": [
                {"id": "default", "name": "Default", "description": "Ask before edits."},
                {"id": "dont_ask"},
                {"name": "no id"},
            ]},
            "models": {"currentModelId": "openrouter:a", "availableModels": [
                {"modelId": "openrouter:a", "name": "A"},
            ]},
        }));
        assert_eq!(reply.session_id.as_deref(), Some("s1"));
        assert_eq!(reply.config_options.as_ref().map(Vec::len), Some(1));
        let modes = reply.modes.expect("modes");
        assert_eq!(modes.current, "default");
        assert_eq!(modes.available.len(), 2);
        assert_eq!(
            modes.available[1].name, "dont_ask",
            "the id names a nameless mode"
        );
        assert_eq!(
            reply.models.expect("models").available[0].id,
            "openrouter:a"
        );
        let empty = OpenReply::parse(&json!({"modes": {"availableModes": []}}));
        assert_eq!(empty, OpenReply::default());
    }

    #[test]
    fn content_blocks_become_text() {
        assert_eq!(content_text(&json!({"type": "text", "text": "hi"})), "hi");
        assert_eq!(
            content_text(
                &json!({"type": "image", "mimeType": "image/png", "data": "A".repeat(4096)})
            ),
            "[image: image/png, 3 KB]"
        );
        assert_eq!(
            content_text(&json!({"type": "audio", "mimeType": "audio/wav", "data": "AAA="})),
            "[audio: audio/wav, 1 KB]"
        );
        assert_eq!(
            content_text(
                &json!({"type": "resource_link", "name": "main.rs", "uri": "file:///w/main.rs"})
            ),
            "[main.rs](file:///w/main.rs)"
        );
        assert_eq!(
            content_text(
                &json!({"type": "resource_link", "name": "n", "title": "Title", "uri": "u"})
            ),
            "[Title](u)"
        );
        assert_eq!(
            content_text(
                &json!({"type": "resource", "resource": {"uri": "file:///a", "text": "body"}})
            ),
            "body"
        );
        assert_eq!(
            content_text(
                &json!({"type": "resource", "resource": {"uri": "file:///b", "blob": "AAAA"}})
            ),
            "[resource file:///b]"
        );
        assert_eq!(content_text(&json!({"type": "hologram"})), "[hologram]");
        assert_eq!(kilobytes(""), 0);
        assert_eq!(kilobytes("AA=="), 1);
    }

    #[test]
    fn tool_fields_drop_unknown_content_and_bad_locations() {
        let fields = ToolFields::parse(&json!({
            "toolCallId": "t1", "title": "Edit", "kind": "teleport", "status": "weird",
            "content": [
                {"type": "content", "content": {"type": "text", "text": "out"}},
                {"type": "diff", "path": "/w/a.rs", "newText": "new"},
                {"type": "diff", "path": "/w/b.rs", "oldText": "old", "newText": "new"},
                {"type": "terminal", "terminalId": "term-1"},
                {"type": "video"},
                {"type": "diff", "path": "/w/c.rs"},
            ],
            "locations": [{"path": "/w/a.rs", "line": 3}, {"line": 4}, {"path": "/w/b.rs", "line": -1}],
            "rawInput": {"cmd": "ls"},
            "rawOutput": {"ignored": true},
        }))
        .expect("fields");
        assert_eq!(fields.kind, Some(ToolKind::Other));
        assert_eq!(fields.status, Some(ToolStatus::Pending));
        let content = fields.content.expect("content");
        assert_eq!(content.len(), 4);
        assert_eq!(fields.dropped_content, 2);
        assert!(matches!(
            &content[1],
            ToolContent::Diff { old_text: None, .. }
        ));
        // A display terminal: its output comes on later updates (terminals.rs).
        assert!(matches!(&content[3], ToolContent::Terminal { output, .. } if output.is_empty()));
        assert_eq!(fields.terminals, [(3, "term-1".to_owned())]);
        assert_eq!(
            fields.locations.expect("locations"),
            [
                ToolLocation {
                    path: "/w/a.rs".into(),
                    line: Some(3)
                },
                ToolLocation {
                    path: "/w/b.rs".into(),
                    line: None
                },
            ]
        );
        assert_eq!(fields.raw_input, Some(json!({"cmd": "ls"})));
        assert!(ToolFields::parse(&json!({"title": "no id"})).is_none());
    }

    #[test]
    fn updates_read_their_tag_first() {
        assert!(matches!(
            Update::parse(&json!({"sessionUpdate": "brand_new_kind", "x": 1})),
            Ok(Update::Unknown(tag)) if tag == "brand_new_kind"
        ));
        assert_eq!(
            Update::parse(&json!({"sessionUpdate": "compaction_update"})),
            Ok(Update::Unmapped("compaction_update"))
        );
        assert_eq!(
            Update::parse(&json!({"no": "tag"})),
            Err(Malformed {
                tag: None,
                reason: "no sessionUpdate tag"
            })
        );
        assert_eq!(
            Update::parse(&json!({"sessionUpdate": "agent_message_chunk", "content": "text"})),
            Err(Malformed {
                tag: Some("agent_message_chunk".into()),
                reason: "no content"
            })
        );
        assert!(Update::parse(&json!({"sessionUpdate": "usage_update"})).is_err());
        assert!(
            Update::parse(&json!({"sessionUpdate": "session_info_update", "title": 3})).is_err()
        );
        assert_eq!(
            Update::parse(&json!({"sessionUpdate": "session_info_update", "updatedAt": "x"})),
            Ok(Update::SessionInfo {
                title: TitleChange::Keep
            })
        );
        assert_eq!(
            Update::parse(
                &json!({"sessionUpdate": "notice", "severity": "fatal", "title": "Low disk"})
            ),
            Ok(Update::Notice {
                level: NoticeLevel::Info,
                message: "Low disk".into()
            })
        );
    }

    #[test]
    fn permission_params_parse_leniently_or_fail_with_a_reason() {
        let parsed = PermissionParams::parse(&json!({
            "sessionId": "s",
            "toolCall": {"toolCallId": "t", "title": "Run ls", "kind": "execute"},
            "options": [
                {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "x", "kind": "allow_forever"},
                {"name": "no id"},
            ],
        }))
        .expect("parses");
        assert_eq!(parsed.options.len(), 2);
        assert_eq!(parsed.options[1].kind, PermissionOptionKind::Other);
        assert_eq!(parsed.options[1].label, "x");
        let no_id = PermissionParams::parse(&json!({
            "toolCall": {"title": "Question"}, "options": []
        }))
        .expect("parses");
        assert_eq!(no_id.tool.tool_call_id, "");
        assert_eq!(no_id.tool.title.as_deref(), Some("Question"));
        for bad in [
            json!(3),
            json!({"options": []}),
            json!({"toolCall": {}, "options": 5}),
        ] {
            assert!(PermissionParams::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn prompt_replies_map_stop_reasons_and_usage() {
        for (wire, reason) in [
            ("end_turn", StopReason::EndTurn),
            ("max_tokens", StopReason::MaxTokens),
            ("max_turn_requests", StopReason::MaxTurnRequests),
            ("refusal", StopReason::Refusal),
            ("cancelled", StopReason::Cancelled),
        ] {
            let reply = PromptReply::parse(&json!({"stopReason": wire}));
            assert_eq!((reply.stop_reason, reply.unknown_reason), (reason, None));
        }
        let other = PromptReply::parse(&json!({"stopReason": "budget_exhausted"}));
        assert_eq!(other.stop_reason, StopReason::Other);
        assert_eq!(other.unknown_reason.as_deref(), Some("budget_exhausted"));
        let usage = PromptReply::parse(&json!({"stopReason": "end_turn", "usage": {
            "totalTokens": 30, "inputTokens": 20, "outputTokens": 10,
            "cachedReadTokens": 5, "thoughtTokens": 2,
        }}))
        .usage
        .expect("usage");
        assert_eq!(
            usage,
            WireUsage {
                input: 20,
                output: 10,
                cached_read: Some(5),
                cached_write: None,
                thought: Some(2)
            }
        );
        assert_eq!(WireUsage::parse(&json!({"totalTokens": 3})), None);
    }
}
