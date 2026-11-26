//! Pi's delta-only assistant updates and authoritative message/tool snapshots.
use super::{metadata, wire::text};
use crate::acp::{map::hex, options::truncate};
use ghost_protocol::{
    GhostEvent, NoticeLevel, StopReason, TokenUsage, ToolCall, ToolContent, ToolKind, ToolLocation,
    ToolStatus, ToolUpdate, UsageScope,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use tokio::time::Instant;

const MAX_BUFFER: usize = 32 * 1024;
const MAX_BLOCKS: usize = 256;
const MAX_TRACKED_ITEMS: usize = 16384;

pub fn id(native: &str) -> String {
    if native.chars().count() <= 96 {
        native.into()
    } else {
        format!("pi-{}", hex(&Sha256::digest(native.as_bytes())))
    }
}
#[derive(Debug)]
struct Block {
    id: String,
    thought: bool,
    text: String,
    overflow: bool,
    completed: bool,
    emitted: bool,
    started: Instant,
}
impl Block {
    fn delta(&mut self, value: &str, out: &mut Vec<GhostEvent>) {
        if self.completed || (value.is_empty() && self.emitted) {
            return;
        }
        if !self.overflow && self.text.len().saturating_add(value.len()) <= MAX_BUFFER {
            self.text.push_str(value);
        } else {
            self.overflow = true;
            self.text.clear();
        }
        self.emitted = true;
        out.push(if self.thought {
            GhostEvent::ThoughtDelta {
                thought_id: self.id.clone(),
                text: value.into(),
            }
        } else {
            GhostEvent::MessageDelta {
                message_id: self.id.clone(),
                text: value.into(),
            }
        });
    }
    fn snapshot(&mut self, value: &str, out: &mut Vec<GhostEvent>) {
        if self.completed || self.overflow {
            return;
        }
        if let Some(suffix) = value.strip_prefix(&self.text) {
            let suffix = suffix.to_owned();
            self.delta(&suffix, out);
        } else {
            out.push(GhostEvent::Notice {
                level: NoticeLevel::Warning,
                message: "Pi revised content already streamed; the streamed text is retained."
                    .into(),
            });
        }
    }
    fn complete(&mut self, out: &mut Vec<GhostEvent>) {
        if self.completed || !self.emitted {
            return;
        }
        self.completed = true;
        out.push(if self.thought {
            GhostEvent::ThoughtCompleted {
                thought_id: self.id.clone(),
                duration_ms: Some(
                    self.started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
                ),
            }
        } else {
            GhostEvent::MessageCompleted {
                message_id: self.id.clone(),
            }
        });
    }
}

#[derive(Debug)]
pub struct Translator {
    prefix: String,
    sequence: u64,
    blocks: BTreeMap<u64, Block>,
    tools: HashSet<String>,
    ended: HashSet<String>,
    usage: Option<TokenUsage>,
    reason: StopReason,
    exhausted: bool,
}
impl Translator {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: id(&prefix.into()),
            sequence: 0,
            blocks: BTreeMap::new(),
            tools: HashSet::new(),
            ended: HashSet::new(),
            usage: None,
            reason: StopReason::EndTurn,
            exhausted: false,
        }
    }
    pub fn begin_turn(&mut self) {
        self.blocks.clear();
        self.tools.clear();
        self.ended.clear();
        self.usage = None;
        self.reason = StopReason::EndTurn;
        self.exhausted = false;
    }
    pub fn is_exhausted(&self) -> bool {
        self.exhausted
    }
    pub fn usage(&self) -> Option<TokenUsage> {
        self.usage.clone()
    }
    pub fn reason(&self) -> StopReason {
        self.reason
    }
    pub fn close_open(&mut self) -> Vec<GhostEvent> {
        let mut out = Vec::new();
        for block in self.blocks.values_mut() {
            block.complete(&mut out);
        }
        out
    }
    fn block(&mut self, index: u64, thought: bool) -> Option<&mut Block> {
        if !self.blocks.contains_key(&index) && self.blocks.len() >= MAX_BLOCKS {
            self.exhausted = true;
            return None;
        }
        Some(self.blocks.entry(index).or_insert_with(|| Block {
            id: id(&format!("pi-{}-{}-{index}", self.prefix, self.sequence)),
            thought,
            text: String::new(),
            overflow: false,
            completed: false,
            emitted: false,
            started: Instant::now(),
        }))
    }
    fn tool(&mut self, native: &str, name: &str, args: Option<&Value>, out: &mut Vec<GhostEvent>) {
        let tool_id = id(native);
        let locations = args
            .and_then(|v| text(v, "path").or_else(|| text(v, "file_path")))
            .map(|path| {
                vec![ToolLocation {
                    path: path.into(),
                    line: None,
                }]
            })
            .unwrap_or_default();
        if self.tools.insert(tool_id.clone()) {
            out.push(GhostEvent::ToolCall(ToolCall {
                tool_call_id: tool_id,
                kind: tool_kind(name),
                title: truncate(name, 200),
                status: ToolStatus::Pending,
                locations,
                raw_input: args.cloned(),
                tool_name: Some(truncate(name, 100)),
                content: vec![],
                truncated: None,
            }));
        } else if let Some(args) = args {
            out.push(GhostEvent::ToolUpdate(ToolUpdate {
                tool_call_id: tool_id,
                status: None,
                title: None,
                kind: None,
                locations: Some(locations),
                raw_input: Some(args.clone()),
                tool_name: None,
                content: None,
                truncated: None,
            }));
        }
        // State is bounded independently of how long extensions keep an agent run alive.
        if self.tools.len() > MAX_TRACKED_ITEMS {
            self.exhausted = true;
        }
    }
    fn message(&mut self, message: &Value, out: &mut Vec<GhostEvent>) {
        if message["role"] != "assistant" {
            return;
        }
        let digest = hex(&Sha256::digest(message.to_string()));
        if !self.ended.insert(digest) {
            return;
        }
        if message["content"]
            .as_array()
            .is_some_and(|blocks| blocks.len() > MAX_BLOCKS)
        {
            self.exhausted = true;
        }
        for (index, content) in message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .take(MAX_BLOCKS)
            .enumerate()
        {
            match text(content, "type") {
                Some("text" | "thinking") => {
                    let thought = content["type"] == "thinking";
                    if let Some(block) = self.block(index as u64, thought) {
                        block.snapshot(
                            content[if thought { "thinking" } else { "text" }]
                                .as_str()
                                .unwrap_or_default(),
                            out,
                        );
                        block.complete(out);
                    }
                }
                Some("toolCall") => {
                    if let (Some(native), Some(name)) = (text(content, "id"), text(content, "name"))
                    {
                        self.tool(native, name, content.get("arguments"), out);
                    }
                }
                _ => {}
            }
        }
        if let Some(usage) = metadata::tokens(&message["usage"], UsageScope::Turn) {
            if let Some(total) = &mut self.usage {
                total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
                total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
                total.cache_read_tokens = total
                    .cache_read_tokens
                    .zip(usage.cache_read_tokens)
                    .map(|(a, b)| a.saturating_add(b));
                total.cache_write_tokens = total
                    .cache_write_tokens
                    .zip(usage.cache_write_tokens)
                    .map(|(a, b)| a.saturating_add(b));
            } else {
                self.usage = Some(usage);
            }
        }
        self.reason = match text(message, "stopReason") {
            Some("length") => StopReason::MaxTokens,
            Some("error") => StopReason::Error,
            Some("aborted") => StopReason::Cancelled,
            Some("stop" | "toolUse") => StopReason::EndTurn,
            _ => StopReason::Other,
        };
        if self.ended.len() > MAX_TRACKED_ITEMS {
            self.exhausted = true;
        }
    }
    pub fn ingest(&mut self, frame: &Value) -> Vec<GhostEvent> {
        let mut out = Vec::new();
        match text(frame, "type") {
            Some("message_start") if frame["message"]["role"] == "assistant" => {
                out.extend(self.close_open());
                self.blocks.clear();
                self.sequence += 1;
                // message_start can contain a snapshot, but message_end owns usage and completion.
            }
            Some("message_update") => {
                let event = &frame["assistantMessageEvent"];
                let index = event["contentIndex"].as_u64().unwrap_or(0);
                match text(event, "type") {
                    Some("text_start" | "thinking_start") => {
                        let thought = event["type"] == "thinking_start";
                        if let Some(block) = self.block(index, thought) {
                            block.delta("", &mut out);
                        }
                    }
                    Some("text_delta" | "thinking_delta") => {
                        let thought = event["type"] == "thinking_delta";
                        if let Some(block) = self.block(index, thought) {
                            block.delta(event["delta"].as_str().unwrap_or_default(), &mut out);
                        }
                    }
                    Some("text_end" | "thinking_end") => {
                        let thought = event["type"] == "thinking_end";
                        if let Some(block) = self.block(index, thought) {
                            block.snapshot(event["content"].as_str().unwrap_or_default(), &mut out);
                            block.complete(&mut out);
                        }
                    }
                    Some("toolcall_start") => {
                        if let (Some(native), Some(name)) =
                            (text(event, "id"), text(event, "toolName"))
                        {
                            self.tool(native, name, None, &mut out);
                        }
                    }
                    Some("toolcall_end") => {
                        let tool = &event["toolCall"];
                        if let (Some(native), Some(name)) = (text(tool, "id"), text(tool, "name")) {
                            self.tool(native, name, tool.get("arguments"), &mut out);
                        }
                    }
                    // Incomplete JSON is not valid tool input. toolcall_end provides the complete object.
                    _ => {}
                }
            }
            Some("message_end") => {
                self.message(&frame["message"], &mut out);
                out.extend(self.close_open());
            }
            Some("tool_execution_start" | "tool_execution_update" | "tool_execution_end") => {
                if let Some(native) = text(frame, "toolCallId") {
                    self.tool(
                        native,
                        text(frame, "toolName").unwrap_or("Tool"),
                        frame.get("args"),
                        &mut out,
                    );
                    let result = if frame["type"] == "tool_execution_update" {
                        frame.get("partialResult")
                    } else {
                        frame.get("result")
                    };
                    let status = if frame["type"] == "tool_execution_end" {
                        if frame["isError"] == false {
                            ToolStatus::Completed
                        } else {
                            ToolStatus::Failed
                        }
                    } else {
                        ToolStatus::InProgress
                    };
                    out.push(GhostEvent::ToolUpdate(ToolUpdate {
                        tool_call_id: id(native),
                        status: Some(status),
                        title: None,
                        kind: None,
                        locations: None,
                        raw_input: None,
                        tool_name: None,
                        content: result.map(tool_content),
                        truncated: None,
                    }));
                }
            }
            Some("agent_settled") => out.extend(self.close_open()),
            _ => {}
        }
        out
    }
}
fn tool_kind(name: &str) -> ToolKind {
    match name {
        "read" => ToolKind::Read,
        "edit" | "write" => ToolKind::Edit,
        "bash" => ToolKind::Execute,
        "grep" | "find" | "ls" => ToolKind::Search,
        _ => ToolKind::Other,
    }
}
fn tool_content(result: &Value) -> Vec<ToolContent> {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| {
            (part["type"] == "text")
                .then(|| part["text"].as_str())
                .flatten()
                .map(|text| ToolContent::Text {
                    text: text.into(),
                    truncated: None,
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn delta_final_snapshot_and_duplicate_end_emit_text_once_and_count_usage_once() {
        let mut t = Translator::new("test");
        t.ingest(&json!({"type":"message_start","message":{"role":"assistant"}}));
        let a=t.ingest(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"hello"}}));
        let end = json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hello world"}],"stopReason":"stop","usage":{"input":3,"output":2}}});
        let b = t.ingest(&end);
        let c = t.ingest(&end);
        let text: String = a
            .iter()
            .chain(&b)
            .chain(&c)
            .filter_map(|e| {
                if let GhostEvent::MessageDelta { text, .. } = e {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(text, "hello world");
        assert!(c.is_empty());
        assert_eq!(t.usage().unwrap().input_tokens, 3);
    }
    #[test]
    fn empty_thinking_closes_and_tool_snapshots_replace_partial_output() {
        let mut t = Translator::new("test");
        assert!(
            matches!(&t.ingest(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}))[0],GhostEvent::ThoughtDelta {text,..} if text.is_empty())
        );
        assert!(matches!(
            &t.close_open()[0],
            GhostEvent::ThoughtCompleted { .. }
        ));
        t.ingest(&json!({"type":"tool_execution_start","toolCallId":"tool","toolName":"bash","args":{"command":"echo hi"}}));
        let update=t.ingest(&json!({"type":"tool_execution_end","toolCallId":"tool","toolName":"bash","isError":false,"result":{"content":[{"type":"text","text":"hi"}]}}));
        assert!(
            matches!(&update[0],GhostEvent::ToolUpdate(v) if v.status==Some(ToolStatus::Completed))
        );
        assert!(id(&"x".repeat(500)).len() <= 96);
    }

    #[test]
    fn retained_state_is_bounded_without_silently_ignoring_excess_blocks() {
        let mut translator = Translator::new("test");
        for index in 0..=MAX_BLOCKS {
            translator.ingest(&json!({"type":"message_update",
            "assistantMessageEvent":{"type":"text_start","contentIndex":index}}));
        }
        assert!(translator.is_exhausted());
        assert_eq!(translator.blocks.len(), MAX_BLOCKS);
        translator.begin_turn();
        assert!(!translator.is_exhausted());
        let blocks = vec![json!({"type":"text","text":"snapshot"}); MAX_BLOCKS + 1];
        translator
            .ingest(&json!({"type":"message_end","message":{"role":"assistant","content":blocks}}));
        assert!(translator.is_exhausted());
        assert_eq!(translator.blocks.len(), MAX_BLOCKS);
    }
}
