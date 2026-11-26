//! Native item streams mapped to the shared transcript vocabulary.
use ghost_protocol::{
    GhostEvent, PermissionRequest, PlanEntry, PlanStatus, ToolCall, ToolContent, ToolKind,
    ToolStatus, ToolUpdate, Truncation, UsageInfo,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Default)]
struct Item {
    emitted: usize,
    complete: bool,
    tool: bool,
    thought_raw: Option<bool>,
    thought_index: Option<u64>,
    output: String,
    output_bytes: usize,
    approval: Option<ToolContent>,
}

pub(super) struct Translator {
    prefix: String,
    items: BTreeMap<String, Item>,
    overflow: bool,
    approval_bytes: usize,
}
impl Translator {
    pub fn new(prefix: String) -> Self {
        Self {
            prefix,
            items: BTreeMap::new(),
            overflow: false,
            approval_bytes: 0,
        }
    }
    fn id(&self, native: &str) -> String {
        if native.len() <= 96 {
            native.into()
        } else {
            format!(
                "codex.{}",
                hex(&Sha256::digest(
                    format!("{}:{native}", self.prefix).as_bytes()
                ))
            )
        }
    }
    pub fn finish_turn(&mut self) {
        self.items.clear();
        self.overflow = false;
        self.approval_bytes = 0;
    }
    pub fn overflowed(&self) -> bool {
        self.overflow
    }
    pub fn enrich_permission(&self, native: &str, request: &mut PermissionRequest) {
        let id = self.id(native);
        if let Some(item) = self.items.get(&id).filter(|item| item.tool) {
            request.tool_call_id = Some(id);
            if let Some(content) = &item.approval {
                request.content.push(content.clone());
            }
        }
    }
    pub fn notification(&mut self, method: &str, params: &Value) -> Vec<GhostEvent> {
        let mut events = Vec::new();
        if self.items.len() >= 4096 {
            self.overflow = true;
            return events;
        }
        match method {
            "item/agentMessage/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/textDelta" => {
                let (Some(native), Some(text)) =
                    (params["itemId"].as_str(), params["delta"].as_str())
                else {
                    return events;
                };
                let thought = method != "item/agentMessage/delta";
                let id = self.id(native);
                let state = self.items.entry(id.clone()).or_default();
                if state.complete {
                    return events;
                }
                if thought {
                    let raw = method == "item/reasoning/textDelta";
                    if state.thought_raw.is_some_and(|selected| selected != raw) {
                        return events;
                    }
                    state.thought_raw = Some(raw);
                    let index = params[if raw { "contentIndex" } else { "summaryIndex" }]
                        .as_u64()
                        .unwrap_or(0);
                    if state
                        .thought_index
                        .is_some_and(|previous| previous != index)
                    {
                        events.push(GhostEvent::ThoughtDelta {
                            thought_id: id.clone(),
                            text: "\n".into(),
                        });
                        state.emitted = state.emitted.saturating_add(1);
                    }
                    state.thought_index = Some(index);
                }
                if !state.complete {
                    state.emitted = state.emitted.saturating_add(text.len());
                    events.push(if thought {
                        GhostEvent::ThoughtDelta {
                            thought_id: id,
                            text: text.into(),
                        }
                    } else {
                        GhostEvent::MessageDelta {
                            message_id: id,
                            text: text.into(),
                        }
                    });
                }
            }
            "item/started" | "item/completed" => {
                let item = &params["item"];
                let Some(native) = item["id"].as_str().filter(|id| !id.is_empty()) else {
                    return events;
                };
                let id = self.id(native);
                let finished = method == "item/completed";
                let state = self.items.entry(id.clone()).or_default();
                if state.complete {
                    return events;
                }
                match item["type"].as_str() {
                    Some("agentMessage" | "reasoning") => {
                        let thought = item["type"] == "reasoning";
                        let text = if thought {
                            let summary = item["summary"]
                                .as_array()
                                .filter(|values| !values.is_empty());
                            let parts = if state.thought_raw == Some(true) {
                                item["content"].as_array()
                            } else {
                                summary.or_else(|| item["content"].as_array())
                            };
                            parts
                                .map(|values| {
                                    values
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                })
                                .unwrap_or_default()
                        } else {
                            item["text"].as_str().unwrap_or("").into()
                        };
                        if let Some(suffix) = text.get(state.emitted..)
                            && !suffix.is_empty()
                        {
                            events.push(if thought {
                                GhostEvent::ThoughtDelta {
                                    thought_id: id.clone(),
                                    text: suffix.into(),
                                }
                            } else {
                                GhostEvent::MessageDelta {
                                    message_id: id.clone(),
                                    text: suffix.into(),
                                }
                            });
                            state.emitted = text.len();
                        }
                        if finished {
                            events.push(if thought {
                                GhostEvent::ThoughtCompleted {
                                    thought_id: id,
                                    duration_ms: None,
                                }
                            } else {
                                GhostEvent::MessageCompleted { message_id: id }
                            });
                        }
                    }
                    Some(
                        "commandExecution"
                        | "fileChange"
                        | "mcpToolCall"
                        | "dynamicToolCall"
                        | "webSearch"
                        | "imageView"
                        | "imageGeneration"
                        | "collabAgentToolCall"
                        | "contextCompaction",
                    ) => {
                        let (kind, title) = tool_identity(item);
                        let status = if finished {
                            match item["status"].as_str() {
                                Some("failed" | "declined") => ToolStatus::Failed,
                                Some("completed" | "succeeded") => ToolStatus::Completed,
                                None if matches!(
                                    item["type"].as_str(),
                                    Some("webSearch" | "imageView" | "contextCompaction")
                                ) =>
                                {
                                    ToolStatus::Completed
                                }
                                _ => ToolStatus::Pending,
                            }
                        } else {
                            ToolStatus::InProgress
                        };
                        let mut content = content(item);
                        if item["type"] == "fileChange" && !content.is_empty() {
                            if let Some(ToolContent::Text { text, .. }) = &state.approval {
                                self.approval_bytes =
                                    self.approval_bytes.saturating_sub(text.len());
                            }
                            let budget = (64 * 1024)
                                .min((4 * 1024 * 1024_usize).saturating_sub(self.approval_bytes));
                            let mut preview = String::new();
                            let mut total_bytes = 0usize;
                            for part in &content {
                                if let ToolContent::Text { text, .. } = part {
                                    if total_bytes > 0 {
                                        total_bytes = total_bytes.saturating_add(1);
                                        if preview.len() < budget {
                                            preview.push('\n');
                                        }
                                    }
                                    total_bytes = total_bytes.saturating_add(text.len());
                                    let mut end =
                                        budget.saturating_sub(preview.len()).min(text.len());
                                    while !text.is_char_boundary(end) {
                                        end -= 1;
                                    }
                                    preview.push_str(&text[..end]);
                                }
                            }
                            self.approval_bytes += preview.len();
                            state.approval = Some(ToolContent::Text {
                                truncated: (preview.len() < total_bytes).then_some(Truncation {
                                    total_bytes: total_bytes as u64,
                                    blob: None,
                                }),
                                text: preview,
                            });
                        }
                        if content.is_empty() && state.output_bytes > 0 {
                            let truncated =
                                (state.output_bytes > state.output.len()).then_some(Truncation {
                                    total_bytes: state.output_bytes as u64,
                                    blob: None,
                                });
                            content.push(if item["type"] == "commandExecution" {
                                ToolContent::Terminal {
                                    output: state.output.clone(),
                                    exit_code: item["exitCode"]
                                        .as_i64()
                                        .and_then(|code| i32::try_from(code).ok()),
                                    truncated,
                                }
                            } else {
                                ToolContent::Text {
                                    text: state.output.clone(),
                                    truncated,
                                }
                            });
                        }
                        if !state.tool {
                            state.tool = true;
                            events.push(GhostEvent::ToolCall(ToolCall {
                                tool_call_id: id.clone(),
                                kind,
                                title: title.clone(),
                                status,
                                locations: Vec::new(),
                                raw_input: (item["type"] != "imageGeneration")
                                    .then(|| item.clone()),
                                tool_name: item["tool"].as_str().map(str::to_owned),
                                content: content.clone(),
                                truncated: None,
                            }));
                        } else {
                            events.push(GhostEvent::ToolUpdate(ToolUpdate {
                                tool_call_id: id,
                                status: Some(status),
                                title: Some(title),
                                kind: Some(kind),
                                locations: None,
                                raw_input: None,
                                tool_name: None,
                                content: Some(content),
                                truncated: None,
                            }));
                        }
                    }
                    _ => {}
                }
                state.complete = finished;
            }
            "turn/diff/updated" => {
                if let (Some(turn), Some(diff)) =
                    (params["turnId"].as_str(), params["diff"].as_str())
                {
                    let id = self.id(&format!("{turn}.diff"));
                    let state = self.items.entry(id.clone()).or_default();
                    let content = vec![ToolContent::Text {
                        text: diff.into(),
                        truncated: None,
                    }];
                    if state.tool {
                        events.push(GhostEvent::ToolUpdate(ToolUpdate {
                            tool_call_id: id,
                            status: None,
                            title: None,
                            kind: None,
                            locations: None,
                            raw_input: None,
                            tool_name: None,
                            content: Some(content),
                            truncated: None,
                        }));
                    } else {
                        state.tool = true;
                        events.push(GhostEvent::ToolCall(ToolCall {
                            tool_call_id: id,
                            kind: ToolKind::Edit,
                            title: "Turn changes".into(),
                            status: ToolStatus::Completed,
                            locations: Vec::new(),
                            raw_input: None,
                            tool_name: None,
                            content,
                            truncated: None,
                        }));
                    }
                }
            }
            "turn/plan/updated" => {
                if let Some(plan) = params["plan"].as_array() {
                    events.push(GhostEvent::PlanUpdated {
                        entries: plan
                            .iter()
                            .filter_map(|entry| {
                                Some(PlanEntry {
                                    content: entry["step"].as_str()?.into(),
                                    status: match entry["status"].as_str() {
                                        Some("completed") => PlanStatus::Completed,
                                        Some("inProgress") => PlanStatus::InProgress,
                                        _ => PlanStatus::Pending,
                                    },
                                    priority: None,
                                })
                            })
                            .collect(),
                    });
                }
            }
            "thread/tokenUsage/updated" => {
                let usage = &params["tokenUsage"];
                events.push(GhostEvent::UsageUpdated(UsageInfo {
                    context_used: usage["last"]["totalTokens"].as_u64(),
                    context_size: usage["modelContextWindow"].as_u64(),
                    cost: None,
                }));
            }
            // Accumulate a bounded preview because ToolUpdate content replaces earlier content.
            // Final native snapshots still pass through the normalizer for complete storage.
            "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
                if let (Some(native), Some(delta)) =
                    (params["itemId"].as_str(), params["delta"].as_str())
                {
                    let id = self.id(native);
                    if let Some(state) = self
                        .items
                        .get_mut(&id)
                        .filter(|item| item.tool && !item.complete)
                    {
                        state.output_bytes = state.output_bytes.saturating_add(delta.len());
                        let remaining = (256 * 1024_usize).saturating_sub(state.output.len());
                        let mut end = remaining.min(delta.len());
                        while !delta.is_char_boundary(end) {
                            end -= 1;
                        }
                        state.output.push_str(&delta[..end]);
                        events.push(GhostEvent::ToolUpdate(ToolUpdate {
                            tool_call_id: id,
                            status: None,
                            title: None,
                            kind: None,
                            locations: None,
                            raw_input: None,
                            tool_name: None,
                            content: Some(vec![ToolContent::Text {
                                text: state.output.clone(),
                                truncated: (state.output_bytes > state.output.len()).then_some(
                                    Truncation {
                                        total_bytes: state.output_bytes as u64,
                                        blob: None,
                                    },
                                ),
                            }]),
                            truncated: None,
                        }));
                    }
                }
            }
            _ => {}
        }
        events
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn tool_identity(item: &Value) -> (ToolKind, String) {
    match item["type"].as_str() {
        Some("commandExecution") => (
            ToolKind::Execute,
            item["command"].as_str().unwrap_or("Command").into(),
        ),
        Some("fileChange") => (ToolKind::Edit, "File changes".into()),
        Some("webSearch") => (ToolKind::Search, "Web search".into()),
        Some("imageView") => (ToolKind::Read, "View image".into()),
        Some("imageGeneration") => (ToolKind::Other, "Generate image".into()),
        Some("contextCompaction") => (ToolKind::Think, "Compact context".into()),
        _ => (
            ToolKind::Other,
            item["tool"].as_str().unwrap_or("Codex tool").into(),
        ),
    }
}
fn content(item: &Value) -> Vec<ToolContent> {
    let image_path = match item["type"].as_str() {
        Some("imageView") => item["path"].as_str(),
        Some("imageGeneration") => item["savedPath"].as_str(),
        _ => None,
    };
    if let Some(path) = image_path {
        return vec![ToolContent::Text {
            text: crate::image_reference("Agent image", path),
            truncated: None,
        }];
    }
    if item["type"] == "imageGeneration" {
        // `result` can contain a very large base64 image. This event model carries local
        // references, not binary payloads; never put those bytes in the text transcript.
        return vec![ToolContent::Text {
            text: "Image generation returned no saved image path.".into(),
            truncated: None,
        }];
    }
    if item["type"] == "commandExecution" {
        return item["aggregatedOutput"]
            .as_str()
            .map(|output| {
                vec![ToolContent::Terminal {
                    output: output.into(),
                    exit_code: item["exitCode"]
                        .as_i64()
                        .and_then(|code| i32::try_from(code).ok()),
                    truncated: None,
                }]
            })
            .unwrap_or_default();
    }
    if item["type"] == "fileChange" {
        return item["changes"]
            .as_array()
            .map(|changes| {
                changes
                    .iter()
                    .filter_map(|change| {
                        Some(ToolContent::Text {
                            text: format!(
                                "{}\n{}",
                                change["path"].as_str()?,
                                change["diff"].as_str().unwrap_or("")
                            ),
                            truncated: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    item.get("result")
        .filter(|result| !result.is_null())
        .map(|result| {
            vec![ToolContent::Text {
                text: result.to_string(),
                truncated: None,
            }]
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_image_paths_survive_without_base64_in_the_transcript() {
        for item in [
            json!({"type":"imageView","path":"C:/work/image (1).png"}),
            json!({"type":"imageGeneration","savedPath":"C:/work/image (1).png","result":"base64"}),
        ] {
            assert_eq!(
                content(&item),
                vec![ToolContent::Text {
                    text: "![Agent image](<C:/work/image (1).png>)".into(),
                    truncated: None
                }]
            );
        }
        let mut translator = Translator::new("spawn".into());
        let output = translator.notification("item/completed", &json!({"item":{"id":"image","type":"imageGeneration","status":"completed","savedPath":"out.png","result":"base64"}}));
        assert!(output.iter().any(|event| matches!(event, GhostEvent::ToolCall(call) if call.content.len() == 1 && call.raw_input.is_none())));
        assert!(
            !format!(
                "{:?}",
                content(&json!({"type":"imageGeneration","result":"base64"}))
            )
            .contains("base64")
        );
    }

    #[test]
    fn snapshots_complete_partial_text_without_repeating_it() {
        let mut translator = Translator::new("spawn".into());
        assert_eq!(
            translator.notification(
                "item/agentMessage/delta",
                &json!({"itemId":"m","delta":"hel"})
            ),
            vec![GhostEvent::MessageDelta {
                message_id: "m".into(),
                text: "hel".into()
            }]
        );
        let complete = json!({"item":{"type":"agentMessage","id":"m","text":"hello"}});
        assert_eq!(
            translator.notification("item/completed", &complete),
            vec![
                GhostEvent::MessageDelta {
                    message_id: "m".into(),
                    text: "lo".into()
                },
                GhostEvent::MessageCompleted {
                    message_id: "m".into()
                }
            ]
        );
        assert!(
            translator
                .notification("item/completed", &complete)
                .is_empty()
        );
    }

    #[test]
    fn tool_delta_previews_accumulate_with_bounded_honest_truncation() {
        let mut translator = Translator::new("spawn".into());
        translator.notification("item/started",&json!({"item":{"type":"commandExecution","id":"cmd","command":"test","status":"inProgress"}}));
        translator.notification(
            "item/commandExecution/outputDelta",
            &json!({"itemId":"cmd","delta":"one"}),
        );
        let events = translator.notification(
            "item/commandExecution/outputDelta",
            &json!({"itemId":"cmd","delta":"two"}),
        );
        let GhostEvent::ToolUpdate(update) = &events[0] else {
            panic!("tool update");
        };
        assert!(
            matches!(&update.content.as_ref().unwrap()[0],ToolContent::Text {text,..} if text=="onetwo")
        );
        let events = translator.notification(
            "item/commandExecution/outputDelta",
            &json!({"itemId":"cmd","delta":"x".repeat(300_000)}),
        );
        let GhostEvent::ToolUpdate(update) = &events[0] else {
            panic!("tool update");
        };
        assert!(
            matches!(&update.content.as_ref().unwrap()[0],ToolContent::Text {text,truncated:Some(info)} if text.len()==256*1024 && info.total_bytes==300_006)
        );
        translator.finish_turn();
        assert!(translator.items.is_empty());
    }

    #[test]
    fn reasoning_channels_are_not_duplicated_and_long_ids_are_bounded() {
        let mut translator = Translator::new("spawn".repeat(30));
        let long = "id".repeat(100);
        let first = translator.notification(
            "item/reasoning/summaryTextDelta",
            &json!({"itemId":long,"summaryIndex":0,"delta":"summary"}),
        );
        let GhostEvent::ThoughtDelta { thought_id, .. } = &first[0] else {
            panic!("thought");
        };
        assert!(thought_id.len() <= 96);
        assert!(
            translator
                .notification(
                    "item/reasoning/textDelta",
                    &json!({"itemId":long,"delta":"raw duplicate"})
                )
                .is_empty()
        );
        let completed=translator.notification("item/completed",&json!({"item":{"type":"reasoning","id":long,"summary":["summary"],"content":["raw duplicate"]}}));
        assert_eq!(completed.len(), 1);
        assert!(matches!(completed[0], GhostEvent::ThoughtCompleted { .. }));
    }

    #[test]
    fn unknown_tool_status_never_claims_success() {
        let mut translator = Translator::new("spawn".into());
        let events=translator.notification("item/completed",&json!({"item":{"type":"commandExecution","id":"cmd","command":"test","status":"future"}}));
        assert!(
            matches!(&events[0],GhostEvent::ToolCall(call) if call.status==ToolStatus::Pending)
        );
    }

    #[test]
    fn raw_reasoning_block_boundaries_match_the_completed_snapshot() {
        let mut translator = Translator::new("spawn".into());
        let mut events = translator.notification(
            "item/reasoning/textDelta",
            &json!({"itemId":"r","contentIndex":0,"delta":"ab"}),
        );
        events.extend(translator.notification(
            "item/reasoning/textDelta",
            &json!({"itemId":"r","contentIndex":1,"delta":"cd"}),
        ));
        events.extend(translator.notification(
            "item/completed",
            &json!({"item":{"type":"reasoning","id":"r","summary":[],"content":["ab","cd"]}}),
        ));
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::ThoughtDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "ab\ncd");
        assert!(matches!(
            events.last(),
            Some(GhostEvent::ThoughtCompleted { .. })
        ));
        assert!(
            translator
                .notification(
                    "item/reasoning/textDelta",
                    &json!({"itemId":"r","contentIndex":2,"delta":"late"})
                )
                .is_empty()
        );
    }

    #[test]
    fn completion_without_output_retains_the_streamed_preview() {
        for (kind, delta_method) in [
            ("commandExecution", "item/commandExecution/outputDelta"),
            ("fileChange", "item/fileChange/outputDelta"),
        ] {
            let mut translator = Translator::new("spawn".into());
            translator.notification(
                "item/started",
                &json!({"item":{"id":"tool","type":kind,"status":"inProgress"}}),
            );
            translator.notification(
                delta_method,
                &json!({"itemId":"tool","delta":"important output"}),
            );
            let events = translator.notification("item/completed", &json!({"item":{"id":"tool","type":kind,"status":"completed","aggregatedOutput":null,"exitCode":0,"changes":[]}}));
            let GhostEvent::ToolUpdate(update) = &events[0] else {
                panic!("tool update")
            };
            let content = update.content.as_ref().unwrap();
            if kind == "commandExecution" {
                assert!(
                    matches!(&content[0], ToolContent::Terminal {output,exit_code:Some(0),..} if output == "important output")
                );
            } else {
                assert!(
                    matches!(&content[0], ToolContent::Text {text,..} if text == "important output")
                );
            }
        }
    }
}
