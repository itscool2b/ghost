//! Native Claude content and bounded metadata reduction. Control and turn lifecycle stay in
//! the driver. Account, hooks, signatures and other opaque protocol fields never enter events.

use std::collections::{BTreeMap, HashMap, HashSet};

use ghost_protocol::session::{ConfigCategory, ConfigChoice, ConfigKind, ConfigOption};
use ghost_protocol::{
    Cost, GhostEvent, LimitWindow, NoticeLevel, PlanEntry, PlanStatus, SlashCommand, TokenUsage,
    ToolCall, ToolContent, ToolKind, ToolLocation, ToolStatus, ToolUpdate, UsageInfo, UsageScope,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::time::Instant;

use super::tasks::Tasks;
use crate::acp::map::{hex, reduce_commands as reduce_acp_commands};
use crate::acp::options::{MAX_CHOICES, MAX_DESCRIPTION_CHARS, MAX_NAME_CHARS, truncate};
use crate::acp::wire::CommandEntry;

const MAX_PARTIAL_JSON_BYTES: usize = 4 * 1024 * 1024;
/// UI choice that removes an explicit effort override in Claude's settings for this process.
pub const DEFAULT_EFFORT: &str = "default";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thought,
    Tool,
}

#[derive(Debug)]
struct Block {
    id: String,
    kind: BlockKind,
    text: String,
    partial_json: String,
    json_overflow: bool,
    started: Instant,
    emitted: bool,
    completed: bool,
    committed: bool,
    committed_text: Option<String>,
}

impl Block {
    fn new(id: String, kind: BlockKind) -> Self {
        Self {
            id,
            kind,
            text: String::new(),
            partial_json: String::new(),
            json_overflow: false,
            started: Instant::now(),
            emitted: false,
            completed: false,
            committed: false,
            committed_text: None,
        }
    }

    fn delta(&mut self, text: &str, events: &mut Vec<GhostEvent>) {
        // Native Claude can expose a thinking block and token counts without exposing its text.
        // Emit one empty delta to open the activity, then preserve its completion and duration.
        if self.committed || text.is_empty() && (self.kind != BlockKind::Thought || self.emitted) {
            return;
        }
        self.text.push_str(text);
        self.emitted = true;
        match self.kind {
            BlockKind::Text => events.push(GhostEvent::MessageDelta {
                message_id: self.id.clone(),
                text: text.to_owned(),
            }),
            BlockKind::Thought => events.push(GhostEvent::ThoughtDelta {
                thought_id: self.id.clone(),
                text: text.to_owned(),
            }),
            BlockKind::Tool => {}
        }
    }

    fn complete(&mut self, events: &mut Vec<GhostEvent>) {
        if self.completed || !self.emitted {
            return;
        }
        self.completed = true;
        match self.kind {
            BlockKind::Text => events.push(GhostEvent::MessageCompleted {
                message_id: self.id.clone(),
            }),
            BlockKind::Thought => events.push(GhostEvent::ThoughtCompleted {
                thought_id: self.id.clone(),
                duration_ms: Some(
                    u32::try_from(self.started.elapsed().as_millis()).unwrap_or(u32::MAX),
                ),
            }),
            BlockKind::Tool => {}
        }
    }
}

#[derive(Debug)]
struct Tool {
    id: String,
    name: String,
    input: Option<Value>,
    input_dirty: bool,
    terminal: bool,
}

/// Translates one process's content. Each native message and content-block index identifies a
/// separate block, including when subagent streams interleave. Native tool ids survive turns.
#[derive(Debug)]
pub struct Translator {
    prefix: String,
    next_message: u64,
    current: HashMap<String, String>,
    messages: BTreeMap<String, BTreeMap<u64, Block>>,
    tools: HashMap<String, Tool>,
    seen_frames: HashSet<String>,
    main_stop_reason: Option<String>,
    /// Subagents and background commands, which outlive turns.
    tasks: Tasks,
}

impl Default for Translator {
    fn default() -> Self {
        Self::new()
    }
}

impl Translator {
    pub fn new() -> Self {
        Self::with_prefix(uuid::Uuid::now_v7().to_string())
    }

    /// Use the process's recorded spawn id so fallback ids are deterministic during replay.
    pub fn with_prefix(prefix: impl Into<String>) -> Self {
        Self {
            prefix: bounded_id(&prefix.into()),
            next_message: 0,
            current: HashMap::new(),
            messages: BTreeMap::new(),
            tools: HashMap::new(),
            seen_frames: HashSet::new(),
            main_stop_reason: None,
            tasks: Tasks::default(),
        }
    }

    /// Call after closing the preceding turn's blocks. Tools are kept for late tool results.
    pub fn begin_turn(&mut self) {
        self.current.clear();
        self.messages.clear();
        self.seen_frames.clear();
        self.main_stop_reason = None;
    }

    /// The latest main message's API stop reason when the terminal result omits it.
    pub fn main_stop_reason(&self) -> Option<&str> {
        self.main_stop_reason.as_deref()
    }

    /// The already-shown tool id, suitable for a permission card's association.
    pub fn tool_call_id(&self, native: &str) -> Option<&str> {
        self.tools.get(native).map(|tool| tool.id.as_str())
    }

    /// Close unfinished content before a forced end. A native result closes it automatically.
    pub fn close_open(&mut self) -> Vec<GhostEvent> {
        let mut events = Vec::new();
        for blocks in self.messages.values_mut() {
            for block in blocks.values_mut() {
                block.complete(&mut events);
            }
        }
        events
    }

    /// Convert only content, usage and background tasks. Commands and options use the reducers
    /// below; the driver owns `system/init`, statuses, control requests and result stop reasons. A
    /// subagent's own messages become its task's steps ([`super::tasks`]), never transcript content.
    pub fn ingest(&mut self, frame: &Value) -> Vec<GhostEvent> {
        let mut events = Vec::new();
        let family = string(frame, "type").unwrap_or_default();
        let tools = &self.tools;
        let shown = |native: &str| tools.get(native).map(|tool| tool.id.clone());
        if family == "system" {
            // Claude Code summarized the conversation to free context: say so where it happened.
            if string(frame, "subtype") == Some("compact_boundary") {
                events.push(compacted(frame));
                return events;
            }
            self.tasks.system(frame, shown, &mut events);
            return events;
        }
        if !matches!(
            family,
            "stream_event" | "assistant" | "user" | "result" | "rate_limit_event"
        ) {
            return events;
        }
        if let Some(id) = string(frame, "uuid")
            && !self.seen_frames.insert(id.to_owned())
        {
            return events;
        }
        if self.tasks.subagent(frame, shown, &mut events) {
            return events;
        }
        match family {
            "stream_event" => self.stream(frame, &mut events),
            // Claude Code's own usage-limit message is not the model speaking: the driver reports
            // it as the limit, and the failed turn's error card carries its words.
            "assistant" if frame["api_error"] == "usage_limit_reached" => {}
            "assistant" => self.assistant(frame, &mut events),
            "user" => self.tool_results(frame, &mut events),
            "rate_limit_event" => {
                let windows = rate_limits(frame);
                if !windows.is_empty() {
                    events.push(GhostEvent::LimitsUpdated { windows });
                }
            }
            "result" => {
                events.extend(self.close_open());
                let usage = result_usage(frame);
                if usage != UsageInfo::default() {
                    events.push(GhostEvent::UsageUpdated(usage));
                }
            }
            _ => {}
        }
        events
    }

    fn message_id(&mut self, frame: &Value, message: Option<&Value>, new: bool) -> String {
        let stream = string(frame, "parent_tool_use_id").unwrap_or_default();
        if let Some(id) = message
            .and_then(|message| string(message, "id"))
            .or_else(|| string(frame, "api_message_id"))
        {
            let id = bounded_id(id);
            self.current.insert(stream.to_owned(), id.clone());
            return id;
        }
        if !new && let Some(id) = self.current.get(stream) {
            return id.clone();
        }
        self.next_message = self.next_message.saturating_add(1);
        let id = format!("{}.m{}", self.prefix, self.next_message);
        self.current.insert(stream.to_owned(), id.clone());
        id
    }

    fn stream(&mut self, frame: &Value, events: &mut Vec<GhostEvent>) {
        let Some(event) = frame.get("event") else {
            return;
        };
        let family = string(event, "type").unwrap_or_default();
        if matches!(frame.get("parent_tool_use_id"), None | Some(Value::Null)) {
            if family == "message_start" {
                self.main_stop_reason = None;
            } else if family == "message_delta"
                && let Some(reason) = string(&event["delta"], "stop_reason")
            {
                self.main_stop_reason = Some(reason.into());
            }
        }
        let message = self.message_id(frame, event.get("message"), family == "message_start");
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
        match family {
            "content_block_start" => {
                if let Some(content) = event.get("content_block") {
                    self.start_block(&message, index, content, events);
                }
            }
            "content_block_delta" => {
                let Some(delta) = event.get("delta") else {
                    return;
                };
                match string(delta, "type").unwrap_or_default() {
                    "text_delta" | "thinking_delta" => {
                        let (kind, field) = if string(delta, "type") == Some("thinking_delta") {
                            (BlockKind::Thought, "thinking")
                        } else {
                            (BlockKind::Text, "text")
                        };
                        if let Some(text) = string(delta, field) {
                            self.block(&message, index, kind).delta(text, events);
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = string(delta, "partial_json") {
                            self.input_delta(&message, index, partial, events);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(block) = self
                    .messages
                    .get_mut(&message)
                    .and_then(|blocks| blocks.get_mut(&index))
                {
                    block.complete(events);
                }
            }
            "message_stop" => {
                if let Some(blocks) = self.messages.get_mut(&message) {
                    for block in blocks.values_mut() {
                        block.complete(events);
                    }
                }
            }
            _ => {}
        }
    }

    fn block(&mut self, message: &str, index: u64, kind: BlockKind) -> &mut Block {
        self.messages
            .entry(message.to_owned())
            .or_default()
            .entry(index)
            .or_insert_with(|| Block::new(format!("{message}.b{index}"), kind))
    }

    fn start_block(
        &mut self,
        message: &str,
        index: u64,
        content: &Value,
        events: &mut Vec<GhostEvent>,
    ) {
        match string(content, "type").unwrap_or_default() {
            "text" => self
                .block(message, index, BlockKind::Text)
                .delta(string(content, "text").unwrap_or_default(), events),
            "thinking" => self
                .block(message, index, BlockKind::Thought)
                .delta(string(content, "thinking").unwrap_or_default(), events),
            "tool_use" | "server_tool_use" => {
                let Some(native) = string(content, "id").filter(|id| !id.is_empty()) else {
                    return;
                };
                let id = self.tool_start(
                    native,
                    string(content, "name").unwrap_or("Tool"),
                    content.get("input"),
                    events,
                );
                let block = self.block(message, index, BlockKind::Tool);
                block.id = id;
            }
            _ => {}
        }
    }

    fn input_delta(
        &mut self,
        message: &str,
        index: u64,
        partial: &str,
        events: &mut Vec<GhostEvent>,
    ) {
        let Some(block) = self
            .messages
            .get_mut(message)
            .and_then(|blocks| blocks.get_mut(&index))
        else {
            return;
        };
        if block.kind != BlockKind::Tool || block.committed || block.json_overflow {
            return;
        }
        if block.partial_json.len().saturating_add(partial.len()) > MAX_PARTIAL_JSON_BYTES {
            block.partial_json.clear();
            block.json_overflow = true;
            return;
        }
        block.partial_json.push_str(partial);
        // Keep incomplete JSON visible as a string; replace it with the object as soon as it parses.
        let input = serde_json::from_str(&block.partial_json)
            .unwrap_or_else(|_| Value::String(block.partial_json.clone()));
        if let Some(tool) = self.tools.values_mut().find(|tool| tool.id == block.id) {
            tool.input_dirty = true;
        }
        let mut update = empty_update(block.id.clone());
        update.raw_input = Some(input);
        events.push(GhostEvent::ToolUpdate(update));
    }

    fn assistant(&mut self, frame: &Value, events: &mut Vec<GhostEvent>) {
        let Some(message) = frame.get("message") else {
            return;
        };
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            return;
        };
        let id = self.message_id(frame, Some(message), false);
        let count = content.len();
        for (position, content) in content.iter().enumerate() {
            let (kind, field) = match string(content, "type").unwrap_or_default() {
                "text" => (BlockKind::Text, "text"),
                "thinking" => (BlockKind::Thought, "thinking"),
                "tool_use" | "server_tool_use" => (BlockKind::Tool, "id"),
                _ => continue,
            };
            let text = string(content, field).unwrap_or_default();
            let index = self.committed_index(&id, position as u64, count, kind, text);
            if kind == BlockKind::Tool {
                if text.is_empty() {
                    continue;
                }
                let tool_id = self.tool_start(
                    text,
                    string(content, "name").unwrap_or("Tool"),
                    content.get("input"),
                    events,
                );
                let block = self.block(&id, index, kind);
                block.id = tool_id;
                block.committed = true;
                block.partial_json.clear();
            } else {
                let block = self.block(&id, index, kind);
                if !block.committed {
                    // A committed copy normally equals the deltas. A missing tail is appended;
                    // divergent text cannot replace an append-only event already displayed.
                    let suffix = text
                        .strip_prefix(&block.text)
                        .unwrap_or_default()
                        .to_owned();
                    block.delta(&suffix, events);
                    block.committed = true;
                    block.committed_text = Some(text.to_owned());
                    block.complete(events);
                }
            }
        }
    }

    fn committed_index(
        &self,
        message: &str,
        position: u64,
        count: usize,
        kind: BlockKind,
        text: &str,
    ) -> u64 {
        let Some(blocks) = self.messages.get(message) else {
            return position;
        };
        if count > 1 {
            return position;
        }
        // Claude emits one committed assistant frame per block, rather than always sending the
        // complete content array. Find the matching streamed block instead of assuming index 0.
        if let Some((index, _)) = blocks.iter().find(|(_, block)| {
            block.kind == kind
                && !block.committed
                && (kind == BlockKind::Tool && block.id == bounded_id(text)
                    || kind != BlockKind::Tool && text.starts_with(&block.text))
        }) {
            return *index;
        }
        if let Some((index, _)) = blocks.iter().find(|(_, block)| {
            block.kind == kind
                && (kind == BlockKind::Tool && block.id == bounded_id(text)
                    || kind != BlockKind::Tool
                        && (block.text == text || block.committed_text.as_deref() == Some(text)))
        }) {
            return *index;
        }
        if kind != BlockKind::Tool
            && let Some((index, _)) = blocks
                .iter()
                .find(|(_, block)| block.kind == kind && !block.committed)
        {
            return *index;
        }
        blocks
            .keys()
            .next_back()
            .map_or(0, |index| index.saturating_add(1))
    }

    /// Starts or updates the call for `native`. Claude's to-do list (TodoWrite) also becomes the
    /// turn's plan, the checklist Codex's plan updates fill, whenever its list arrives or changes.
    fn tool_start(
        &mut self,
        native: &str,
        name: &str,
        input: Option<&Value>,
        events: &mut Vec<GhostEvent>,
    ) -> String {
        let before = events.len();
        let id = self.call_start(native, name, input, events);
        if events.len() > before
            && let Some(entries) = todo_plan(name, input)
        {
            events.push(GhostEvent::PlanUpdated { entries });
        }
        id
    }

    fn call_start(
        &mut self,
        native: &str,
        name: &str,
        input: Option<&Value>,
        events: &mut Vec<GhostEvent>,
    ) -> String {
        if let Some(tool) = self.tools.get_mut(native) {
            if tool.input_dirty
                || tool.name != name
                || input.is_some_and(|input| tool.input.as_ref() != Some(input))
            {
                let mut update = empty_update(tool.id.clone());
                update.title = Some(tool_title(name, input));
                update.kind = Some(tool_kind(name));
                update.tool_name = Some(name.to_owned());
                update.raw_input = input.cloned();
                update.locations = Some(tool_locations(input));
                tool.name = name.to_owned();
                if let Some(input) = input {
                    tool.input = Some(input.clone());
                }
                tool.input_dirty = false;
                events.push(GhostEvent::ToolUpdate(update));
            }
            return tool.id.clone();
        }
        let id = bounded_id(native);
        events.push(GhostEvent::ToolCall(ToolCall {
            tool_call_id: id.clone(),
            kind: tool_kind(name),
            title: tool_title(name, input),
            status: ToolStatus::InProgress,
            locations: tool_locations(input),
            raw_input: input.cloned(),
            tool_name: Some(name.to_owned()),
            content: Vec::new(),
            truncated: None,
        }));
        self.tools.insert(
            native.to_owned(),
            Tool {
                id: id.clone(),
                name: name.to_owned(),
                input: input.cloned(),
                input_dirty: false,
                terminal: false,
            },
        );
        id
    }

    fn tool_results(&mut self, frame: &Value, events: &mut Vec<GhostEvent>) {
        let Some(content) = frame
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            return;
        };
        let result_count = content
            .iter()
            .filter(|block| string(block, "type") == Some("tool_result"))
            .count();
        for block in content {
            if string(block, "type") != Some("tool_result") {
                continue;
            }
            let Some(native) = string(block, "tool_use_id").filter(|id| !id.is_empty()) else {
                continue;
            };
            if !self.tools.contains_key(native) {
                self.tool_start(native, "Tool", None, events);
            }
            let Some(tool) = self.tools.get_mut(native) else {
                continue;
            };
            if tool.terminal {
                continue;
            }
            tool.terminal = true;
            let mut update = empty_update(tool.id.clone());
            update.status = Some(
                if block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Completed
                },
            );
            update.content = Some(tool_output(
                block,
                (result_count == 1)
                    .then(|| frame.get("tool_use_result"))
                    .flatten(),
            ));
            events.push(GhostEvent::ToolUpdate(update));
        }
    }
}

/// Claude's to-do list as plan entries: `None` unless `name` is TodoWrite with a `todos` list.
fn todo_plan(name: &str, input: Option<&Value>) -> Option<Vec<PlanEntry>> {
    if name != "TodoWrite" {
        return None;
    }
    let todos = input?.get("todos")?.as_array()?;
    Some(
        todos
            .iter()
            .filter_map(|todo| {
                let content = string(todo, "content")?.trim();
                (!content.is_empty()).then(|| PlanEntry {
                    content: content.chars().take(500).collect(),
                    status: match string(todo, "status") {
                        Some("completed") => PlanStatus::Completed,
                        Some("in_progress") => PlanStatus::InProgress,
                        _ => PlanStatus::Pending,
                    },
                    priority: None,
                })
            })
            .take(100)
            .collect(),
    )
}

/// Claude Code's `compact_boundary`: the conversation so far was summarized, by request or on
/// its own as the context filled, from `pre_tokens` tokens.
fn compacted(frame: &Value) -> GhostEvent {
    let metadata = &frame["compact_metadata"];
    let before = metadata["pre_tokens"]
        .as_u64()
        .filter(|tokens| *tokens > 0)
        .map(|tokens| {
            if tokens >= 1000 {
                format!(" from {}k tokens", tokens.div_ceil(1000))
            } else {
                format!(" from {tokens} tokens")
            }
        })
        .unwrap_or_default();
    let message = if string(metadata, "trigger") == Some("manual") {
        format!("Compacted the conversation{before}. Claude Code continues from its summary.")
    } else {
        format!(
            "Claude Code compacted the conversation{before} as its context filled, and continues from its summary."
        )
    };
    GhostEvent::Notice {
        level: NoticeLevel::Info,
        message,
    }
}

pub(super) fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

pub(super) fn bounded_id(id: &str) -> String {
    if id.chars().count() <= 96 {
        id.to_owned()
    } else {
        hex(&Sha256::digest(id.as_bytes()))
    }
}

fn empty_update(tool_call_id: String) -> ToolUpdate {
    ToolUpdate {
        tool_call_id,
        status: None,
        title: None,
        kind: None,
        locations: None,
        raw_input: None,
        tool_name: None,
        content: None,
        truncated: None,
    }
}

/// Only exact known built-in tool names affect policy classification. MCP names stay `Other`.
pub fn tool_kind(name: &str) -> ToolKind {
    match name {
        "Read" => ToolKind::Read,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ToolKind::Edit,
        "Glob" | "Grep" | "ToolSearch" => ToolKind::Search,
        "Bash" => ToolKind::Execute,
        "WebFetch" | "WebSearch" => ToolKind::Fetch,
        "EnterPlanMode" | "ExitPlanMode" => ToolKind::SwitchMode,
        _ => ToolKind::Other,
    }
}

/// The longest pattern or query a title quotes, in characters; the input keeps the whole of it.
const MAX_SUBJECT_CHARS: usize = 120;

/// How a call reads in the transcript. The agent's own description wins; Claude's built-in tools
/// that carry none are titled from their input ("Read main.rs", "Search for useState"), and any
/// other tool keeps its name. The full path stays in the call's locations and raw input.
pub(super) fn tool_title(name: &str, input: Option<&Value>) -> String {
    if let Some(description) = input
        .and_then(|input| string(input, "description"))
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        return truncate(description, MAX_DESCRIPTION_CHARS);
    }
    builtin_title(name, input).map_or_else(
        || truncate(name, MAX_NAME_CHARS),
        |title| truncate(&title, MAX_DESCRIPTION_CHARS),
    )
}

/// A built-in tool's title from its input, or `None` when the input does not say enough yet (a
/// call streams `{}` first and its input later).
fn builtin_title(name: &str, input: Option<&Value>) -> Option<String> {
    let field = |key: &str| {
        input
            .and_then(|input| string(input, key))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    Some(match name {
        "Read" => format!("Read {}", file_name(field("file_path")?)),
        "Edit" | "MultiEdit" => format!("Edit {}", file_name(field("file_path")?)),
        "Write" => format!("Write {}", file_name(field("file_path")?)),
        "NotebookEdit" => format!("Edit {}", file_name(field("notebook_path")?)),
        "Grep" => format!("Search for {}", subject(field("pattern")?)),
        "Glob" => format!("Find {}", subject(field("pattern")?)),
        "LS" => format!("List {}", file_name(field("path")?)),
        "WebFetch" => format!("Fetch {}", host(field("url")?)?),
        "WebSearch" => format!("Search the web for {}", subject(field("query")?)),
        "ToolSearch" => format!("Search tools for {}", subject(field("query")?)),
        "Bash" => command_line(field("command")?),
        "TodoWrite" => "Update the to-do list".to_owned(),
        "EnterPlanMode" => "Enter plan mode".to_owned(),
        "ExitPlanMode" => "Exit plan mode".to_owned(),
        _ => return None,
    })
}

/// The last component of a Unix or Windows path ("src/main.rs" reads "main.rs").
fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rsplit(['/', '\\']).next() {
        Some(name) if !name.is_empty() => name,
        _ => path,
    }
}

/// A pattern or query on one line, cut to `MAX_SUBJECT_CHARS` with an ellipsis.
fn subject(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let more = line.len() < text.trim().len();
    let cut = truncate(line, MAX_SUBJECT_CHARS);
    if more || cut.len() < line.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

/// A command's first line, with an ellipsis when more lines follow (a script, a heredoc).
fn command_line(command: &str) -> String {
    let mut lines = command
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or_default();
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first.to_owned()
    }
}

/// The host of a URL, without its scheme, credentials, port, path, query or fragment.
fn host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next().unwrap_or(authority)
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    (!host.is_empty()).then_some(host)
}

fn tool_locations(input: Option<&Value>) -> Vec<ToolLocation> {
    let Some(input) = input else {
        return Vec::new();
    };
    string(input, "file_path")
        .or_else(|| string(input, "notebook_path"))
        .map(|path| ToolLocation {
            path: path.to_owned(),
            line: input
                .get("offset")
                .and_then(Value::as_u64)
                .and_then(|line| u32::try_from(line).ok()),
        })
        .into_iter()
        .collect()
}

fn tool_output(block: &Value, detail: Option<&Value>) -> Vec<ToolContent> {
    if let Some(detail) = detail
        && (detail.get("stdout").is_some() || detail.get("stderr").is_some())
    {
        let mut output = string(detail, "stdout").unwrap_or_default().to_owned();
        let stderr = string(detail, "stderr").unwrap_or_default();
        if !output.is_empty() && !stderr.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(stderr);
        // Preserve explanatory tool_result content when a command intentionally had no stdout.
        if output.is_empty() {
            output = content_text(block.get("content"));
        }
        return vec![ToolContent::Terminal {
            output,
            exit_code: detail
                .get("exitCode")
                .or_else(|| detail.get("exit_code"))
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok()),
            truncated: None,
        }];
    }
    let text = content_text(block.get("content"));
    if text.is_empty() {
        Vec::new()
    } else {
        vec![ToolContent::Text {
            text,
            truncated: None,
        }]
    }
}

fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                (string(block, "type") == Some("text"))
                    .then(|| string(block, "text"))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// What the session loaded, from Claude's `system/init`: MCP servers with their status and
/// source, skills, plugins with their versions, subagents, and how many tools it offers.
pub fn loaded(init: &Value) -> ghost_protocol::session::SessionLoaded {
    use ghost_protocol::session::{LoadedMcpServer, LoadedPlugin, McpLoadState, SessionLoaded};
    let names = |key: &str| -> Vec<String> {
        init[key]
            .as_array()
            .into_iter()
            .flatten()
            // Names, or (in the initialize reply) entries that carry one.
            .filter_map(|item| item.as_str().or(item["name"].as_str()).map(str::to_owned))
            .collect()
    };
    SessionLoaded {
        mcp_servers: init["mcp_servers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|server| {
                Some(LoadedMcpServer {
                    name: server["name"].as_str()?.to_owned(),
                    state: match server["status"].as_str() {
                        Some("connected") => McpLoadState::Connected,
                        Some("pending") => McpLoadState::Starting,
                        Some("needs-auth") => McpLoadState::NeedsSignIn,
                        Some("failed") => McpLoadState::Failed,
                        Some("disabled") => McpLoadState::Off,
                        _ => McpLoadState::Unknown,
                    },
                    source: server["source"].as_str().map(str::to_owned),
                })
            })
            .collect(),
        skills: names("skills"),
        plugins: init["plugins"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|plugin| {
                Some(LoadedPlugin {
                    name: plugin["name"].as_str()?.to_owned(),
                    version: plugin["version"].as_str().map(str::to_owned),
                })
            })
            .collect(),
        agents: names("agents"),
        tools: init["tools"]
            .as_array()
            .map(|tools| u32::try_from(tools.len()).unwrap_or(u32::MAX)),
    }
    .bounded()
}

/// Reduce an initialize/commands-changed payload or its command array using the shared limits.
pub fn reduce_commands(value: &Value) -> Vec<SlashCommand> {
    let Some(commands) = value
        .as_array()
        .or_else(|| value.get("commands").and_then(Value::as_array))
    else {
        return Vec::new();
    };
    reduce_acp_commands(
        commands
            .iter()
            .filter_map(|command| {
                let name = command
                    .as_str()
                    .or_else(|| string(command, "name"))?
                    .trim_start_matches('/');
                if name.is_empty() {
                    return None;
                }
                Some(CommandEntry {
                    name: name.to_owned(),
                    description: string(command, "description")
                        .unwrap_or_default()
                        .to_owned(),
                    hint: string(command, "argumentHint")
                        .filter(|hint| !hint.is_empty())
                        .map(str::to_owned),
                })
            })
            .collect(),
    )
}

/// Model and effort choices from the initialize inner response. Raw account data is ignored.
pub fn options(
    init: &Value,
    current_model: &str,
    current_effort: Option<&str>,
) -> Vec<ConfigOption> {
    let models = init
        .get("models")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let selected = models
        .iter()
        .find(|model| string(model, "value") == Some(current_model))
        .or_else(|| {
            models
                .iter()
                .find(|model| string(model, "resolvedModel") == Some(current_model))
        });
    let current = selected
        .and_then(|model| string(model, "value"))
        .unwrap_or(current_model);
    let mut seen = HashSet::new();
    let mut choices = models
        .iter()
        .filter_map(|model| {
            let value = truncate(
                string(model, "value").filter(|value| !value.is_empty())?,
                MAX_NAME_CHARS,
            );
            if !seen.insert(value.clone()) {
                return None;
            }
            Some(ConfigChoice {
                name: truncate(
                    string(model, "displayName").unwrap_or(&value),
                    MAX_NAME_CHARS,
                ),
                value,
                description: string(model, "description")
                    .map(|text| truncate(text, MAX_DESCRIPTION_CHARS)),
                group: None,
            })
        })
        .take(MAX_CHOICES)
        .collect::<Vec<_>>();
    let current = truncate(current, MAX_NAME_CHARS);
    if !current.is_empty() && !choices.iter().any(|choice| choice.value == current) {
        if choices.len() == MAX_CHOICES {
            choices.pop();
        }
        choices.push(ConfigChoice {
            value: current.clone(),
            name: selected
                .and_then(|model| string(model, "displayName"))
                .map_or_else(|| current.clone(), |name| truncate(name, MAX_NAME_CHARS)),
            description: Some("Current model reported by Claude.".into()),
            group: None,
        });
    }
    let mut options = Vec::new();
    if !choices.is_empty() {
        options.push(ConfigOption {
            id: "model".to_owned(),
            name: "Model".to_owned(),
            description: None,
            category: ConfigCategory::Model,
            kind: ConfigKind::Select { current, choices },
            pending: None,
        });
    }
    if let Some(model) = selected
        && model.get("supportsEffort").and_then(Value::as_bool) == Some(true)
        && let Some(levels) = model.get("supportedEffortLevels").and_then(Value::as_array)
    {
        let mut seen = HashSet::new();
        let mut choices = levels
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 128 && *value != DEFAULT_EFFORT)
            .filter(|value| seen.insert((*value).to_owned()))
            .take(MAX_CHOICES.saturating_sub(1))
            .map(|value| ConfigChoice {
                value: value.to_owned(),
                name: value.to_owned(),
                description: None,
                group: None,
            })
            .collect::<Vec<_>>();
        if !choices.is_empty() {
            choices.insert(
                0,
                ConfigChoice {
                    value: DEFAULT_EFFORT.to_owned(),
                    name: "Default".to_owned(),
                    description: None,
                    group: None,
                },
            );
            let current = current_effort
                .filter(|value| choices.iter().any(|choice| choice.value == *value))
                .unwrap_or(DEFAULT_EFFORT);
            options.push(ConfigOption {
                id: "effort".to_owned(),
                name: "Effort".to_owned(),
                description: None,
                category: ConfigCategory::Effort,
                kind: ConfigKind::Select {
                    current: current.to_owned(),
                    choices,
                },
                pending: None,
            });
        }
    }
    options
}

/// Main-loop token accounting for this result only. `modelUsage` is session cumulative and must
/// not be substituted for the turn's `usage`.
pub fn token_usage(result: &Value) -> Option<TokenUsage> {
    let usage = result.get("usage")?.as_object()?;
    Some(TokenUsage {
        input_tokens: usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_read_tokens: usage.get("cache_read_input_tokens").and_then(Value::as_u64),
        cache_write_tokens: usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64),
        reasoning_tokens: usage
            .get("output_tokens_details")
            .and_then(|detail| detail.get("thinking_tokens"))
            .and_then(Value::as_u64),
        scope: UsageScope::Turn,
    })
}

/// Cumulative session spend. Model token totals are not the current context occupancy.
pub fn result_usage(result: &Value) -> UsageInfo {
    UsageInfo {
        context_used: None,
        context_size: None,
        cost: result
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .filter(|amount| amount.is_finite() && *amount >= 0.0)
            .map(|amount| Cost {
                amount,
                currency: "USD".to_owned(),
            }),
    }
}

/// Reduce `get_context_usage`'s inner response without leaking its large detail lists.
pub fn context_usage(value: &Value) -> UsageInfo {
    UsageInfo {
        context_used: value.get("totalTokens").and_then(Value::as_u64),
        context_size: value.get("maxTokens").and_then(Value::as_u64),
        cost: None,
    }
}

/// Stream limits use fractions and Unix seconds, unlike the `get_usage` control response.
pub fn rate_limits(frame: &Value) -> Vec<LimitWindow> {
    let info = frame.get("rate_limit_info").unwrap_or(frame);
    let Some(windows) = info.get("unifiedWindows").and_then(Value::as_object) else {
        return Vec::new();
    };
    windows
        .iter()
        .filter_map(|(name, value)| {
            let fraction = value
                .get("utilization")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())?;
            Some(LimitWindow {
                label: match name.as_str() {
                    "five_hour" => "5 hours".to_owned(),
                    "seven_day" => "7 days".to_owned(),
                    _ => truncate(name, 48),
                },
                used_percent: (fraction * 100.0).clamp(0.0, 100.0),
                resets_at: value
                    .get("resetsAt")
                    .and_then(Value::as_i64)
                    .and_then(|seconds| seconds.checked_mul(1000)),
            })
        })
        .take(16)
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const BASIC: &str =
        include_str!("../../../../docs/spikes/claude-control/fixtures/turn-basic.jsonl");
    const PERMISSION: &str =
        include_str!("../../../../docs/spikes/claude-control/fixtures/turn-permission.jsonl");
    const INTERRUPT: &str =
        include_str!("../../../../docs/spikes/claude-control/fixtures/turn-interrupt.jsonl");

    fn fixture(source: &str) -> Vec<Value> {
        source
            .lines()
            .map(|line| serde_json::from_str(line).expect("redacted fixture JSON"))
            .collect()
    }

    fn stream(id: &str, event: Value) -> Value {
        json!({"type":"stream_event", "api_message_id":id, "event":event})
    }

    #[test]
    fn claudes_to_do_list_becomes_the_turns_plan_whenever_it_changes() {
        let mut translator = Translator::default();
        let todos = json!({"todos":[
            {"content":"Read the failing test","status":"completed","activeForm":"Reading"},
            {"content":"Fix the parser","status":"in_progress","activeForm":"Fixing"},
            {"content":"Run the suite","status":"pending","activeForm":"Running"},
            {"content":"   ","status":"pending"}
        ]});
        let events = translator.ingest(&committed(
            "m",
            json!([{"type":"tool_use","id":"todo","name":"TodoWrite","input":todos}]),
        ));
        let plans: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::PlanUpdated { entries } => Some(entries.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            plans,
            vec![vec![
                PlanEntry {
                    content: "Read the failing test".into(),
                    status: PlanStatus::Completed,
                    priority: None,
                },
                PlanEntry {
                    content: "Fix the parser".into(),
                    status: PlanStatus::InProgress,
                    priority: None,
                },
                PlanEntry {
                    content: "Run the suite".into(),
                    status: PlanStatus::Pending,
                    priority: None,
                },
            ]]
        );
        // The call itself still shows, and the same list again changes nothing.
        assert!(events.iter().any(|event| matches!(event, GhostEvent::ToolCall(call) if call.title == "Update the to-do list")));
        assert!(
            translator
                .ingest(&committed(
                    "m2",
                    json!([{"type":"tool_use","id":"todo","name":"TodoWrite","input":todos}])
                ))
                .iter()
                .all(|event| !matches!(event, GhostEvent::PlanUpdated { .. }))
        );
        // Other tools never touch the plan.
        assert_eq!(todo_plan("Bash", Some(&json!({"todos":[]}))), None);
    }

    #[test]
    fn a_compacted_conversation_is_a_notice_where_it_happened() {
        let mut translator = Translator::default();
        let mut notice = |frame: Value| match translator.ingest(&frame).as_slice() {
            [
                GhostEvent::Notice {
                    level: NoticeLevel::Info,
                    message,
                },
            ] => message.clone(),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(
            notice(
                json!({"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":152_340}})
            ),
            "Claude Code compacted the conversation from 153k tokens as its context filled, and continues from its summary."
        );
        assert_eq!(
            notice(
                json!({"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"manual"}})
            ),
            "Compacted the conversation. Claude Code continues from its summary."
        );
    }

    fn committed(id: &str, content: Value) -> Value {
        json!({"type":"assistant", "message":{"id":id,"content":content}})
    }

    fn text(events: &[GhostEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::MessageDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn basic_fixture_emits_text_once_and_keeps_turn_and_session_usage_distinct() {
        let mut translator = Translator::new();
        let frames = fixture(BASIC);
        let events = frames
            .iter()
            .flat_map(|frame| translator.ingest(frame))
            .collect::<Vec<_>>();
        assert_eq!(text(&events), "ok");
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, GhostEvent::MessageCompleted { .. }))
                .count(),
            1
        );
        let result = frames
            .iter()
            .find(|frame| string(frame, "type") == Some("result"))
            .expect("result");
        let usage = token_usage(result).expect("usage");
        assert_eq!((usage.input_tokens, usage.output_tokens), (2, 4));
        assert_eq!(
            (usage.cache_read_tokens, usage.cache_write_tokens),
            (Some(12799), Some(16736))
        );
        assert_eq!(usage.reasoning_tokens, Some(0));
        assert_eq!(usage.scope, UsageScope::Turn);
        assert_eq!(result_usage(result).context_used, None);
        let limits = events
            .iter()
            .find_map(|event| match event {
                GhostEvent::LimitsUpdated { windows } => Some(windows),
                _ => None,
            })
            .expect("limits");
        assert_eq!(limits[0].used_percent, 30.0);
        assert_eq!(limits[0].resets_at, Some(1792834800000));
    }

    #[test]
    fn permission_fixture_has_one_call_per_tool_with_final_inputs_and_results() {
        let mut translator = Translator::new();
        let events = fixture(PERMISSION)
            .iter()
            .flat_map(|frame| translator.ingest(frame))
            .collect::<Vec<_>>();
        let calls = events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::ToolCall(call) => Some(call),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|call| call.kind == ToolKind::Execute));
        let statuses = events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::ToolUpdate(update) => update.status,
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            [
                ToolStatus::Completed,
                ToolStatus::Completed,
                ToolStatus::Failed
            ]
        );
        assert!(events.iter().any(|event| matches!(event, GhostEvent::ToolUpdate(update) if update.raw_input.as_ref().and_then(|input| string(input, "command")) == Some("echo auracode-spike"))));
        assert!(events.iter().any(|event| matches!(event, GhostEvent::ToolUpdate(update) if matches!(update.content.as_deref(), Some([ToolContent::Terminal { output, .. }]) if output == "auracode-spike"))));
    }

    #[test]
    fn interrupted_fixture_retains_partial_output_without_replaying_it() {
        let frames = fixture(INTERRUPT);
        let mut translator = Translator::new();
        let mut expected = String::new();
        let mut events = Vec::new();
        for frame in &frames {
            if string(frame, "type") == Some("assistant")
                && let Some(content) = frame["message"]["content"].as_array()
            {
                for block in content {
                    if string(block, "type") == Some("text") {
                        expected.push_str(string(block, "text").unwrap_or_default());
                    }
                }
            }
            events.extend(translator.ingest(frame));
            if string(frame, "type") == Some("result") {
                translator.begin_turn();
            }
        }
        assert_eq!(text(&events), expected);
        assert!(text(&events).contains("37"));
    }

    #[test]
    fn thinking_and_text_commits_reconcile_individual_blocks_and_ignore_signatures() {
        let mut translator = Translator::new();
        let mut events = Vec::new();
        for frame in [
            stream(
                "m",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            ),
            stream(
                "m",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Check "}}),
            ),
            stream(
                "m",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"private-signature"}}),
            ),
            committed(
                "m",
                json!([{"type":"thinking","thinking":"Check carefully","signature":"private-signature"}]),
            ),
            stream(
                "m",
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            ),
            stream(
                "m",
                json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Ans"}}),
            ),
            committed("m", json!([{"type":"text","text":"Answer"}])),
            stream("m", json!({"type":"content_block_stop","index":1})),
            committed("m", json!([{"type":"text","text":"Answer"}])),
        ] {
            events.extend(translator.ingest(&frame));
        }
        assert_eq!(text(&events), "Answer");
        let thoughts = events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::ThoughtDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(thoughts, "Check carefully");
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, GhostEvent::ThoughtCompleted { .. }))
                .count(),
            1
        );
        assert!(
            !serde_json::to_string(&events)
                .expect("serialize")
                .contains("private-signature")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn withheld_thinking_text_keeps_one_activity_and_its_duration() {
        let mut translator = Translator::new();
        let mut events = translator.ingest(&stream(
            "m",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        ));
        tokio::time::advance(std::time::Duration::from_millis(120)).await;
        for frame in [
            stream(
                "m",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"","estimated_tokens":302}}),
            ),
            stream(
                "m",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"private-signature"}}),
            ),
            stream("m", json!({"type":"content_block_stop","index":0})),
            committed(
                "m",
                json!([{"type":"thinking","thinking":"","signature":"private-signature"}]),
            ),
            stream("m", json!({"type":"message_stop"})),
        ] {
            events.extend(translator.ingest(&frame));
        }
        assert!(matches!(events.as_slice(), [
            GhostEvent::ThoughtDelta { text, thought_id },
            GhostEvent::ThoughtCompleted { thought_id: completed_id, duration_ms: Some(120) }
        ] if text.is_empty() && thought_id == completed_id));
        assert!(
            !serde_json::to_string(&events)
                .expect("events")
                .contains("private-signature")
        );
    }

    #[test]
    fn committed_only_withheld_thinking_has_a_complete_empty_activity() {
        let mut translator = Translator::new();
        let frame = committed("m", json!([{"type":"thinking","thinking":""}]));
        let events = translator.ingest(&frame);
        assert!(matches!(events.as_slice(), [
            GhostEvent::ThoughtDelta { text, .. }, GhostEvent::ThoughtCompleted { .. }
        ] if text.is_empty()));
        assert!(translator.ingest(&frame).is_empty());
    }

    #[test]
    fn tool_json_stream_is_replaced_by_final_input_and_unknown_results_are_preserved() {
        let mut translator = Translator::new();
        let mut events = Vec::new();
        for frame in [
            stream(
                "m",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"Read","input":{}}}),
            ),
            stream(
                "m",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"file_"}}),
            ),
            stream(
                "m",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"path\":\"a.rs\"}"}}),
            ),
            committed(
                "m",
                json!([{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"a.rs","offset":10}}]),
            ),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"late","is_error":true,"content":[{"type":"text","text":"not found"}]}]}}),
        ] {
            events.extend(translator.ingest(&frame));
        }
        assert_eq!(translator.tool_call_id("t"), Some("t"));
        assert!(events.iter().any(|event| matches!(event, GhostEvent::ToolUpdate(update) if update.raw_input == Some(Value::String("{\"file_".to_owned())))));
        assert!(events.iter().any(|event| matches!(event, GhostEvent::ToolUpdate(update) if update.locations == Some(vec![ToolLocation { path: "a.rs".to_owned(), line: Some(10) }]))));
        assert_eq!(
            events
                .iter()
                .filter(
                    |event| matches!(event, GhostEvent::ToolCall(call) if call.tool_call_id == "t")
                )
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(event, GhostEvent::ToolUpdate(update) if update.tool_call_id == "late" && update.status == Some(ToolStatus::Failed))));
    }

    #[test]
    fn builtin_tools_are_titled_from_their_input() {
        let long = "x".repeat(MAX_SUBJECT_CHARS + 10);
        for (name, input, title) in [
            (
                "Read",
                json!({"file_path":"C:\\repo\\src\\main.rs"}),
                "Read main.rs",
            ),
            (
                "Read",
                json!({"file_path":"/repo/src/lib.rs","offset":10,"limit":40}),
                "Read lib.rs",
            ),
            (
                "Grep",
                json!({"pattern":"useState","path":"src","glob":"*.tsx"}),
                "Search for useState",
            ),
            ("Glob", json!({"pattern":"**/*.tsx"}), "Find **/*.tsx"),
            ("LS", json!({"path":"/repo/src/"}), "List src"),
            (
                "WebFetch",
                json!({"url":"https://me@docs.rs:443/serde/latest?q=1#top","prompt":"Summarize"}),
                "Fetch docs.rs",
            ),
            (
                "WebFetch",
                json!({"url":"http://[::1]:8080/health"}),
                "Fetch [::1]",
            ),
            (
                "WebSearch",
                json!({"query":"tauri window effects"}),
                "Search the web for tauri window effects",
            ),
            (
                "Edit",
                json!({"file_path":"src/app.ts","old_string":"a","new_string":"b"}),
                "Edit app.ts",
            ),
            (
                "MultiEdit",
                json!({"file_path":"src/app.ts","edits":[]}),
                "Edit app.ts",
            ),
            (
                "Write",
                json!({"file_path":"notes\\todo.md","content":""}),
                "Write todo.md",
            ),
            (
                "NotebookEdit",
                json!({"notebook_path":"nb/analysis.ipynb"}),
                "Edit analysis.ipynb",
            ),
            (
                "Bash",
                json!({"command":"pnpm test -- sidebar"}),
                "pnpm test -- sidebar",
            ),
            (
                "Bash",
                json!({"command":"cat <<'EOF' > a.txt\nhello\nEOF"}),
                "cat <<'EOF' > a.txt …",
            ),
            ("TodoWrite", json!({"todos":[]}), "Update the to-do list"),
            ("ExitPlanMode", json!({"plan":"Ship it"}), "Exit plan mode"),
        ] {
            assert_eq!(tool_title(name, Some(&input)), title, "{name} {input}");
        }
        // The agent's own words win over the derived title.
        assert_eq!(
            tool_title(
                "Bash",
                Some(&json!({"command":"ls -la","description":"List the project files"}))
            ),
            "List the project files"
        );
        assert_eq!(
            tool_title(
                "Task",
                Some(&json!({"description":"Explore the repo","prompt":"..."}))
            ),
            "Explore the repo"
        );
        // A long pattern is cut on one line; the input keeps the whole of it.
        let cut = tool_title("Grep", Some(&json!({ "pattern": long })));
        assert_eq!(
            cut,
            format!("Search for {}…", "x".repeat(MAX_SUBJECT_CHARS))
        );
        assert_eq!(
            tool_title("WebSearch", Some(&json!({"query":"first\nsecond"}))),
            "Search the web for first…"
        );
        // Input still streaming ({}), missing or blank fields, and other tools keep the name.
        for (name, input) in [
            ("Read", Some(json!({}))),
            ("Grep", Some(json!({"pattern":"  "}))),
            ("WebFetch", Some(json!({"url":"https:///path"}))),
            ("Glob", None),
            ("mcp__github__create_issue", Some(json!({"title":"Bug"}))),
        ] {
            assert_eq!(tool_title(name, input.as_ref()), name);
        }
    }

    #[test]
    fn a_streamed_call_is_retitled_once_its_input_commits() {
        let mut translator = Translator::new();
        let mut events = Vec::new();
        for frame in [
            stream(
                "m",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"g","name":"Grep","input":{}}}),
            ),
            stream(
                "m",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"pattern\":\"useState\"}"}}),
            ),
            committed(
                "m",
                json!([{"type":"tool_use","id":"g","name":"Grep","input":{"pattern":"useState"}}]),
            ),
        ] {
            events.extend(translator.ingest(&frame));
        }
        let titles = events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::ToolCall(call) => Some(call.title.as_str()),
                GhostEvent::ToolUpdate(update) => update.title.as_deref(),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(titles, ["Grep", "Search for useState"]);
    }

    #[test]
    fn nested_streams_and_committed_only_messages_keep_separate_ids() {
        let mut translator = Translator::new();
        let frames = [
            stream(
                "main",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"one"}}),
            ),
            stream(
                "child",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"two"}}),
            ),
            committed("main", json!([{"type":"text","text":"one"}])),
            committed("child", json!([{"type":"text","text":"two"}])),
            committed("local", json!([{"type":"text","text":"local output"}])),
        ];
        let events = frames
            .iter()
            .flat_map(|frame| translator.ingest(frame))
            .collect::<Vec<_>>();
        assert_eq!(text(&events), "onetwolocal output");
        let ids = events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::MessageDelta { message_id, .. } => Some(message_id),
                _ => None,
            })
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn real_initialize_commands_and_options_are_reduced_without_account() {
        let frames = fixture(BASIC);
        let init = frames
            .iter()
            .find_map(|frame| {
                frame
                    .get("response")
                    .and_then(|response| response.get("response"))
                    .filter(|response| response.get("models").is_some())
            })
            .expect("initialize");
        let commands = reduce_commands(init);
        assert!(commands.len() >= 100);
        assert!(commands.iter().any(|command| command.name == "usage"));
        assert!(
            commands
                .iter()
                .all(|command| command.description.chars().count() <= 300)
        );
        let choices = options(init, "default", Some("medium"));
        assert_eq!(choices.len(), 2);
        let serialized = serde_json::to_string(&choices).expect("serialize");
        assert!(!serialized.contains("<EMAIL>"));
        assert!(!serialized.contains("<ORG_NAME>"));
        let changed = frames
            .iter()
            .find(|frame| string(frame, "subtype") == Some("commands_changed"))
            .expect("commands changed");
        assert!(!reduce_commands(changed).is_empty());
    }

    #[test]
    fn real_initialize_effort_always_selects_an_offered_choice() {
        let frames = fixture(BASIC);
        let init = frames
            .iter()
            .find_map(|frame| {
                frame
                    .get("response")
                    .and_then(|response| response.get("response"))
                    .filter(|response| response.get("models").is_some())
            })
            .expect("initialize");
        for model in init["models"].as_array().expect("models") {
            let model = model["value"].as_str().expect("model value");
            for effort in [
                None,
                Some(DEFAULT_EFFORT),
                Some("medium"),
                Some("not-supported"),
            ] {
                for option in options(init, model, effort) {
                    if option.id == "effort" {
                        let ConfigKind::Select { current, choices } = option.kind else {
                            panic!("effort is a select")
                        };
                        assert!(
                            choices.iter().any(|choice| choice.value == current),
                            "{model}: {effort:?}"
                        );
                        assert!(choices.iter().any(
                            |choice| choice.value == DEFAULT_EFFORT && choice.name == "Default"
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_metadata_and_unknown_frames_are_safe() {
        let mut translator = Translator::new();
        for frame in [
            json!({"type":"future","account":{"email":"private"}}),
            json!({"type":"system","subtype":"init","account":{"email":"private"}}),
            json!({"type":"assistant","message":null}),
            json!({"type":"user","message":{"content":"hello"}}),
        ] {
            assert!(translator.ingest(&frame).is_empty());
        }
        assert_eq!(
            reduce_commands(
                &json!([{"name":"/x","description":"one"},{"name":"x","description":"duplicate"},null,{"name":""}])
            ),
            vec![SlashCommand {
                name: "x".to_owned(),
                description: "one".to_owned(),
                input_hint: None
            }]
        );
        assert!(token_usage(&json!({"modelUsage":{"m":{"inputTokens":500}}})).is_none());
        assert_eq!(
            context_usage(&json!({"totalTokens":25,"maxTokens":100,"account":"private"})),
            UsageInfo {
                context_used: Some(25),
                context_size: Some(100),
                cost: None
            }
        );
        assert!(
            rate_limits(
                &json!({"rate_limit_info":{"unifiedWindows":{"bad":{"utilization":"no"}}}})
            )
            .is_empty()
        );
    }

    #[test]
    fn result_closes_partial_blocks_and_next_turn_does_not_reuse_fallback_ids() {
        let mut translator = Translator::new();
        let delta = json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}});
        let first = translator.ingest(&delta);
        assert_eq!(translator.ingest(&json!({"type":"result"})).len(), 1);
        assert!(translator.close_open().is_empty());
        translator.begin_turn();
        let next = translator.ingest(&delta);
        assert_ne!(first, next);
    }

    #[test]
    fn oversized_tool_json_can_still_commit_and_finish_after_a_turn_boundary() {
        let mut translator = Translator::new();
        translator.ingest(&stream("m", json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"Read","input":{}}})));
        assert!(translator.ingest(&stream("m", json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"x".repeat(MAX_PARTIAL_JSON_BYTES + 1)}}))).is_empty());
        let commit = translator.ingest(&committed(
            "m",
            json!([{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"a"}}]),
        ));
        assert!(
            matches!(commit.as_slice(), [GhostEvent::ToolUpdate(update)] if update.raw_input == Some(json!({"file_path":"a"})))
        );
        translator.begin_turn();
        let result = json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"done"}]}});
        assert!(
            matches!(translator.ingest(&result).as_slice(), [GhostEvent::ToolUpdate(update)] if update.status == Some(ToolStatus::Completed))
        );
        assert!(translator.ingest(&result).is_empty());
    }

    #[test]
    fn duplicate_stream_frames_and_divergent_commits_do_not_duplicate_visible_text() {
        let mut translator = Translator::new();
        let mut delta = stream(
            "m",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"visible"}}),
        );
        delta["uuid"] = json!("unique-frame");
        assert_eq!(text(&translator.ingest(&delta)), "visible");
        assert!(translator.ingest(&delta).is_empty());
        let commit = translator.ingest(&committed(
            "m",
            json!([{"type":"text","text":"changed final"}]),
        ));
        assert!(text(&commit).is_empty());
        assert!(matches!(
            commit.as_slice(),
            [GhostEvent::MessageCompleted { .. }]
        ));
    }

    #[test]
    fn resolved_model_is_an_advertised_choice_and_limits_are_bounded() {
        let choices = options(
            &json!({"models":[{"value":"haiku","resolvedModel":"resolved","displayName":"Haiku","supportsEffort":true,"supportedEffortLevels":["medium","future","medium"]}]}),
            "resolved",
            Some("medium"),
        );
        assert!(
            matches!(&choices[0].kind, ConfigKind::Select { current, .. } if current == "haiku")
        );
        assert!(
            matches!(&choices[1].kind, ConfigKind::Select { choices, .. } if choices.len() == 3 && choices.iter().any(|c| c.value == "future"))
        );
        let limits = rate_limits(
            &json!({"unifiedWindows":{"five_hour":{"utilization":1.5,"resetsAt":i64::MAX},"seven_day":{"utilization":-0.1,"resetsAt":42}}}),
        );
        assert_eq!(limits[0].used_percent, 100.0);
        assert_eq!(limits[0].resets_at, None);
        assert_eq!(limits[1].used_percent, 0.0);
    }

    #[test]
    fn large_text_reaches_the_shared_batcher_losslessly_and_limits_fit_one_event() {
        let mut translator = Translator::new();
        let expected = "Rust 🦀 \"newline\"\n".repeat(3000);
        let events = translator.ingest(&committed(
            "large",
            json!([{"type":"text","text":expected}]),
        ));
        let mut batcher = crate::batch::FrameBatcher::new("chat");
        for event in events {
            batcher.push(event);
        }
        let frames = batcher.flush();
        assert!(frames.len() > 1);
        assert!(
            frames
                .iter()
                .all(|frame| frame.encoded_len() <= ghost_protocol::MAX_FRAME_BYTES)
        );
        let delivered = frames
            .into_iter()
            .flat_map(|frame| frame.events)
            .collect::<Vec<_>>();
        assert_eq!(text(&delivered), expected);
        let windows = (0..100)
            .map(|index| {
                (
                    format!("{index}{}", "\u{0001}".repeat(200)),
                    json!({"utilization":0.5,"resetsAt":i64::MAX/1000}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let event = GhostEvent::LimitsUpdated {
            windows: rate_limits(&json!({"unifiedWindows": windows})),
        };
        assert!(
            serde_json::to_vec(&event).expect("serialize").len() <= ghost_protocol::EVENT_BUDGET
        );
    }
}
