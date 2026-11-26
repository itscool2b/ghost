//! Claude Code's tasks: the subagents and background shell commands an agent starts, and the
//! subagents' own messages.
//!
//! Claude reports a task in `system` frames: `task_started` (for every Bash call, foreground ones
//! too), `task_progress`, `task_updated` (a `patch`), `task_notification` (its end and its report)
//! and `background_tasks_changed` (the tasks running in the background now). A subagent's own
//! messages arrive as ordinary `assistant`, `user` and `stream_event` frames whose
//! `parent_tool_use_id` names the call that started it and whose `agent_id` is its task id. They
//! become the task's steps, never transcript content: its tool calls (titled like the transcript's)
//! and its text. Streamed partial frames are skipped; the committed ones say everything.
//!
//! Every subagent shows. A command shows only when it runs in the background: a foreground command
//! is its tool call in the transcript already. One moved to the background later shows from then
//! on, so the hidden ones are kept until they end.

use std::collections::HashMap;

use ghost_protocol::tasks::{BackgroundTask, TaskKind, TaskStatus, TaskStep, TaskStepKind};
use ghost_protocol::{GhostEvent, ToolStatus};
use serde_json::Value;

use super::translate::{bounded_id, string, tool_kind, tool_title};
use crate::acp::options::{MAX_DESCRIPTION_CHARS, truncate};

/// A task's prompt, cut with an ellipsis.
pub(super) const MAX_PROMPT_BYTES: usize = 4 * 1024;
/// A task's final report.
pub(super) const MAX_SUMMARY_BYTES: usize = 32 * 1024;
/// One text step; the session's normalizer fits it to the event budget with a blob of the rest.
pub(super) const MAX_STEP_TEXT_BYTES: usize = 32 * 1024;
/// Tasks remembered at most; the oldest ended ones go first.
const MAX_TASKS: usize = 256;

#[derive(Debug)]
struct Task {
    record: BackgroundTask,
    visible: bool,
    ended: bool,
    prompt_sent: bool,
    summary_sent: bool,
    /// When it was first heard of, among this process's tasks.
    order: u64,
}

/// One process's tasks.
#[derive(Debug, Default)]
pub(super) struct Tasks {
    tasks: HashMap<String, Task>,
    /// Each started task by the native id of the call that started it.
    by_tool: HashMap<String, String>,
    /// A subagent's tool calls by their native id: the task and the step as last shown.
    steps: HashMap<String, (String, TaskStep)>,
    next_order: u64,
    next_frame: u64,
}

impl Tasks {
    /// Reduces one `system` frame. `tool_call` answers the transcript's id of a call by its native
    /// id, when the call is shown in the transcript.
    pub(super) fn system(
        &mut self,
        frame: &Value,
        tool_call: impl Fn(&str) -> Option<String>,
        events: &mut Vec<GhostEvent>,
    ) {
        match string(frame, "subtype").unwrap_or_default() {
            "task_started" => self.started(frame, &tool_call, events),
            "task_progress" => self.progress(frame, &tool_call, events),
            "task_updated" => self.updated(frame, events),
            "task_notification" => self.notification(frame, &tool_call, events),
            "background_tasks_changed" => self.background(frame, events),
            _ => {}
        }
    }

    /// Reduces a subagent's own frame into its task's steps. Answers whether `frame` was one (its
    /// `parent_tool_use_id` is set), so the transcript never sees it.
    pub(super) fn subagent(
        &mut self,
        frame: &Value,
        tool_call: impl Fn(&str) -> Option<String>,
        events: &mut Vec<GhostEvent>,
    ) -> bool {
        let Some(parent) = string(frame, "parent_tool_use_id").filter(|id| !id.is_empty()) else {
            return false;
        };
        let family = string(frame, "type").unwrap_or_default();
        if !matches!(family, "assistant" | "user" | "stream_event") {
            return false;
        }
        // Claude Code's own usage-limit message is no step: the driver reports the limit.
        if family == "stream_event" || frame["api_error"] == "usage_limit_reached" {
            return true;
        }
        let task_id = string(frame, "agent_id")
            .filter(|id| !id.is_empty())
            .map(bounded_id)
            .or_else(|| self.by_tool.get(parent).cloned())
            .unwrap_or_else(|| bounded_id(parent));
        if !self.tasks.contains_key(&task_id) {
            // Its messages came before its start: show it from what they say.
            let task = self.task(&task_id, TaskKind::Agent);
            task.visible = true;
            task.record.title = title(string(frame, "task_description"), TaskKind::Agent);
            task.record.tool_call_id = tool_call(parent);
            self.by_tool.insert(parent.to_owned(), task_id.clone());
            self.emit(&task_id, events);
        }
        if let Some(task) = self.tasks.get_mut(&task_id)
            && task.record.agent_type.is_none()
        {
            task.record.agent_type = text(frame, "subagent_type", MAX_DESCRIPTION_CHARS);
        }
        let Some(content) = frame
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            return true;
        };
        if family == "assistant" {
            self.next_frame += 1;
            let key = string(frame, "uuid")
                .filter(|id| !id.is_empty())
                .map_or_else(|| format!("f{}", self.next_frame), bounded_id);
            for (position, block) in content.iter().enumerate() {
                match string(block, "type").unwrap_or_default() {
                    "text" => {
                        let Some(body) =
                            string(block, "text").filter(|text| !text.trim().is_empty())
                        else {
                            continue;
                        };
                        let step = TaskStep {
                            step_id: format!("{key}.{position}"),
                            kind: TaskStepKind::Text,
                            title: String::new(),
                            tool_kind: None,
                            status: None,
                            text: Some(cut(body, MAX_STEP_TEXT_BYTES)),
                            truncated: None,
                        };
                        push_step(&task_id, step, events);
                    }
                    "tool_use" | "server_tool_use" => {
                        let Some(native) = string(block, "id").filter(|id| !id.is_empty()) else {
                            continue;
                        };
                        let name = string(block, "name").unwrap_or("Tool");
                        let input = block.get("input");
                        let status = self
                            .steps
                            .get(native)
                            .and_then(|(_, step)| step.status)
                            .unwrap_or(ToolStatus::InProgress);
                        let step = TaskStep {
                            step_id: bounded_id(native),
                            kind: TaskStepKind::Tool,
                            title: tool_title(name, input),
                            tool_kind: Some(tool_kind(name)),
                            status: Some(status),
                            text: None,
                            truncated: None,
                        };
                        self.steps
                            .insert(native.to_owned(), (task_id.clone(), step.clone()));
                        push_step(&task_id, step, events);
                    }
                    _ => {}
                }
            }
        } else {
            for block in content {
                if string(block, "type") != Some("tool_result") {
                    continue;
                }
                let Some((owner, step)) =
                    string(block, "tool_use_id").and_then(|native| self.steps.get_mut(native))
                else {
                    continue;
                };
                let failed = block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                step.status = Some(if failed {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Completed
                });
                let (owner, step) = (owner.clone(), step.clone());
                push_step(&owner, step, events);
            }
        }
        true
    }

    fn started(
        &mut self,
        frame: &Value,
        tool_call: &impl Fn(&str) -> Option<String>,
        events: &mut Vec<GhostEvent>,
    ) {
        let Some(id) = task_id(frame) else {
            return;
        };
        let kind = kind(frame);
        let spawned_by = string(frame, "tool_use_id").filter(|id| !id.is_empty());
        let task = self.task(&id, kind);
        task.record.kind = kind;
        task.record.title = title(string(frame, "description"), kind);
        if let Some(agent_type) = text(frame, "subagent_type", MAX_DESCRIPTION_CHARS) {
            task.record.agent_type = Some(agent_type);
        }
        if let Some(native) = spawned_by
            && task.record.tool_call_id.is_none()
        {
            task.record.tool_call_id = tool_call(native);
        }
        if let Some(parent) = string(frame, "parent_task_id").filter(|id| !id.is_empty()) {
            task.record.parent_task_id = Some(bounded_id(parent));
        }
        let backgrounded = frame["is_backgrounded"].as_bool().unwrap_or(false);
        task.record.backgrounded |= backgrounded;
        task.record.prompt = string(frame, "prompt")
            .filter(|prompt| !prompt.trim().is_empty())
            .map(|prompt| cut(prompt, MAX_PROMPT_BYTES));
        task.visible |= kind == TaskKind::Agent || task.record.backgrounded;
        if let Some(native) = spawned_by {
            self.by_tool.insert(native.to_owned(), id.clone());
        }
        self.emit(&id, events);
    }

    fn progress(
        &mut self,
        frame: &Value,
        tool_call: &impl Fn(&str) -> Option<String>,
        events: &mut Vec<GhostEvent>,
    ) {
        let Some(id) = task_id(frame) else {
            return;
        };
        let known = self.tasks.contains_key(&id);
        let kind = if frame.get("subagent_type").is_some() {
            TaskKind::Agent
        } else {
            TaskKind::Other
        };
        let task = self.task(&id, kind);
        if !known {
            task.record.title = title(None, kind);
            task.visible = kind == TaskKind::Agent;
            if let Some(native) = string(frame, "tool_use_id") {
                task.record.tool_call_id = tool_call(native);
            }
        }
        if task.ended {
            return;
        }
        if let Some(activity) = text(frame, "description", MAX_DESCRIPTION_CHARS) {
            task.record.activity = Some(activity);
        }
        if let Some(agent_type) = text(frame, "subagent_type", MAX_DESCRIPTION_CHARS) {
            task.record.agent_type = Some(agent_type);
        }
        usage(&mut task.record, frame.get("usage"));
        self.emit(&id, events);
    }

    fn updated(&mut self, frame: &Value, events: &mut Vec<GhostEvent>) {
        let Some(id) = task_id(frame) else {
            return;
        };
        let Some(patch) = frame.get("patch") else {
            return;
        };
        let Some(task) = self.tasks.get_mut(&id) else {
            return;
        };
        if let Some(status) = string(patch, "status").and_then(status) {
            task.record.status = status;
            task.ended = status != TaskStatus::Running;
        }
        if let Some(end) = patch.get("end_time").and_then(Value::as_i64) {
            task.record.ended_at = Some(end);
        }
        if let Some(backgrounded) = patch.get("is_backgrounded").and_then(Value::as_bool) {
            task.record.backgrounded = backgrounded;
            task.visible |= backgrounded;
        }
        self.emit(&id, events);
        if self.tasks.get(&id).is_some_and(|task| task.ended) {
            self.end(&id);
        }
    }

    fn notification(
        &mut self,
        frame: &Value,
        tool_call: &impl Fn(&str) -> Option<String>,
        events: &mut Vec<GhostEvent>,
    ) {
        let Some(id) = task_id(frame) else {
            return;
        };
        let Some(task) = self.tasks.get_mut(&id) else {
            return;
        };
        // A notification is the end, whatever it calls it.
        task.record.status = string(frame, "status")
            .and_then(status)
            .filter(|status| *status != TaskStatus::Running)
            .unwrap_or(TaskStatus::Stopped);
        task.ended = true;
        task.record.summary = string(frame, "summary")
            .filter(|summary| !summary.trim().is_empty())
            .map(|summary| cut(summary, MAX_SUMMARY_BYTES));
        task.summary_sent = false;
        if task.record.tool_call_id.is_none()
            && let Some(native) = string(frame, "tool_use_id")
        {
            task.record.tool_call_id = tool_call(native);
        }
        usage(&mut task.record, frame.get("usage"));
        self.emit(&id, events);
        self.end(&id);
    }

    /// The tasks running in the background now: one not heard of yet shows from what it says, and
    /// a known one is in the background from now on.
    fn background(&mut self, frame: &Value, events: &mut Vec<GhostEvent>) {
        let Some(listed) = frame.get("tasks").and_then(Value::as_array) else {
            return;
        };
        for entry in listed {
            let Some(id) = task_id(entry) else {
                continue;
            };
            let known = self.tasks.contains_key(&id);
            let kind = kind(entry);
            let task = self.task(&id, kind);
            if task.ended || known && task.record.backgrounded && task.visible {
                continue;
            }
            if !known {
                task.record.title = title(string(entry, "description"), kind);
                task.record.agent_type = text(entry, "subagent_type", MAX_DESCRIPTION_CHARS);
            }
            task.record.backgrounded = true;
            task.visible = true;
            self.emit(&id, events);
        }
    }

    /// The task `id`, new and running when not heard of yet.
    fn task(&mut self, id: &str, kind: TaskKind) -> &mut Task {
        if !self.tasks.contains_key(id) {
            self.prune();
            self.next_order += 1;
        }
        let order = self.next_order;
        self.tasks.entry(id.to_owned()).or_insert_with(|| Task {
            record: BackgroundTask {
                task_id: id.to_owned(),
                kind,
                title: String::new(),
                agent_type: None,
                tool_call_id: None,
                parent_task_id: None,
                backgrounded: false,
                status: TaskStatus::Running,
                activity: None,
                tool_uses: None,
                tokens: None,
                duration_ms: None,
                started_at: None,
                ended_at: None,
                prompt: None,
                summary: None,
                summary_truncated: None,
            },
            visible: false,
            ended: false,
            prompt_sent: false,
            summary_sent: false,
            order,
        })
    }

    /// Shows the task's record when it shows: its prompt and summary once each.
    fn emit(&mut self, id: &str, events: &mut Vec<GhostEvent>) {
        let Some(task) = self.tasks.get_mut(id) else {
            return;
        };
        if !task.visible {
            return;
        }
        let mut record = task.record.clone();
        if task.prompt_sent {
            record.prompt = None;
        } else if record.prompt.is_some() {
            task.prompt_sent = true;
        }
        if task.summary_sent {
            record.summary = None;
        } else if record.summary.is_some() {
            task.summary_sent = true;
        }
        events.push(GhostEvent::TaskUpdated(record));
    }

    /// A task ended: its steps are done, and a hidden one is forgotten.
    fn end(&mut self, id: &str) {
        self.steps.retain(|_, (owner, _)| owner != id);
        if self.tasks.get(id).is_some_and(|task| !task.visible) {
            self.tasks.remove(id);
            self.by_tool.retain(|_, task| task != id);
        }
    }

    /// Room for one more task: the oldest ended one goes when the limit is reached.
    fn prune(&mut self) {
        if self.tasks.len() < MAX_TASKS {
            return;
        }
        let oldest = self
            .tasks
            .iter()
            .filter(|(_, task)| task.ended)
            .min_by_key(|(_, task)| task.order)
            .map(|(id, _)| id.clone());
        if let Some(id) = oldest {
            self.tasks.remove(&id);
            self.by_tool.retain(|_, task| *task != id);
        }
    }
}

fn push_step(task_id: &str, step: TaskStep, events: &mut Vec<GhostEvent>) {
    events.push(GhostEvent::TaskStep {
        task_id: task_id.to_owned(),
        step,
    });
}

fn task_id(frame: &Value) -> Option<String> {
    string(frame, "task_id")
        .filter(|id| !id.is_empty())
        .map(bounded_id)
}

/// `local_agent` (and Claude's other agent kinds) is a subagent, `local_bash` a command.
fn kind(frame: &Value) -> TaskKind {
    match string(frame, "task_type").unwrap_or_default() {
        "local_agent" | "remote_agent" | "in_process_teammate" => TaskKind::Agent,
        "local_bash" => TaskKind::Command,
        _ if frame.get("subagent_type").is_some() => TaskKind::Agent,
        _ => TaskKind::Other,
    }
}

fn title(description: Option<&str>, kind: TaskKind) -> String {
    description
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map_or_else(
            || {
                match kind {
                    TaskKind::Agent => "Subagent",
                    TaskKind::Command => "Background command",
                    TaskKind::Other => "Background task",
                }
                .to_owned()
            },
            |text| truncate(text, MAX_DESCRIPTION_CHARS),
        )
}

/// A one-line field, trimmed and cut to `chars`; `None` when blank.
fn text(frame: &Value, key: &str, chars: usize) -> Option<String> {
    string(frame, key)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| truncate(text, chars))
}

fn status(native: &str) -> Option<TaskStatus> {
    Some(match native {
        "running" | "pending" => TaskStatus::Running,
        "completed" => TaskStatus::Completed,
        "failed" | "error" => TaskStatus::Failed,
        "stopped" | "killed" | "cancelled" => TaskStatus::Stopped,
        _ => return None,
    })
}

fn usage(record: &mut BackgroundTask, usage: Option<&Value>) {
    let Some(usage) = usage else {
        return;
    };
    if let Some(tokens) = usage.get("total_tokens").and_then(Value::as_u64) {
        record.tokens = Some(tokens);
    }
    if let Some(uses) = usage
        .get("tool_uses")
        .and_then(Value::as_u64)
        .and_then(|uses| u32::try_from(uses).ok())
    {
        record.tool_uses = Some(uses);
    }
    if let Some(duration) = usage.get("duration_ms").and_then(Value::as_u64) {
        record.duration_ms = Some(duration);
    }
}

/// `text` cut to at most `bytes` on a character boundary, with an ellipsis when cut.
fn cut(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes.saturating_sub('…'.len_utf8());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use ghost_protocol::ToolKind;
    use serde_json::json;

    use super::super::translate::Translator;
    use super::*;

    const SPAWN: &str = "toolu_01TkJsynthetic";
    const TASK: &str = "a3c89c21b19145224";

    fn feed(translator: &mut Translator, frames: &[Value]) -> Vec<GhostEvent> {
        frames
            .iter()
            .flat_map(|frame| translator.ingest(frame))
            .collect()
    }

    fn tasks(events: &[GhostEvent]) -> Vec<&BackgroundTask> {
        events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::TaskUpdated(task) => Some(task),
                _ => None,
            })
            .collect()
    }

    fn steps(events: &[GhostEvent]) -> Vec<(&str, &TaskStep)> {
        events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::TaskStep { task_id, step } => Some((task_id.as_str(), step)),
                _ => None,
            })
            .collect()
    }

    /// The main agent's Agent call, as Claude commits it.
    fn agent_call() -> Value {
        json!({"type":"assistant","message":{"id":"msg_main","content":[{"type":"tool_use","id":SPAWN,"name":"Agent","input":{"description":"Map account switching code","subagent_type":"Explore","prompt":"In the repo, find where accounts switch.","run_in_background":true}}]},"parent_tool_use_id":null,"uuid":"u-main"})
    }

    fn started() -> Value {
        json!({"type":"system","subtype":"task_started","task_id":TASK,"run_id":"0mvztuxnl-1412051c","tool_use_id":SPAWN,"description":"Map account switching code","subagent_type":"Explore","is_backgrounded":true,"spawn_depth":1,"task_type":"local_agent","prompt":"In the repo, find where accounts switch.","uuid":"u-started"})
    }

    fn sub(kind: &str, uuid: &str, content: Value) -> Value {
        json!({"type":kind,"message":{"id":"msg_sub","role":kind,"content":content},"parent_tool_use_id":SPAWN,"agent_id":TASK,"subagent_type":"Explore","task_description":"Map account switching code","uuid":uuid})
    }

    #[test]
    fn a_subagent_becomes_a_task_whose_messages_are_its_steps() {
        let mut translator = Translator::with_prefix("spawn");
        let events = feed(
            &mut translator,
            &[
                agent_call(),
                json!({"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":TASK,"run_id":"0mvztuxnl-1412051c","task_type":"local_agent","subagent_type":"Explore","description":"Map account switching code"}],"uuid":"u-bg"}),
                started(),
                sub(
                    "assistant",
                    "u-s1",
                    json!([{"type":"thinking","thinking":"","signature":"private"}]),
                ),
                sub(
                    "assistant",
                    "u-s2",
                    json!([{"type":"text","text":"I'll start with the **docs**."}]),
                ),
                json!({"type":"stream_event","parent_tool_use_id":SPAWN,"event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"streamed"}}}),
                sub(
                    "assistant",
                    "u-s3",
                    json!([{"type":"tool_use","id":"toolu_sub_read","name":"Read","input":{"file_path":"C:\\repo\\docs\\agent-accounts.md"}}]),
                ),
                json!({"type":"system","subtype":"task_progress","task_id":TASK,"tool_use_id":SPAWN,"description":"Reading docs\\agent-accounts.md","subagent_type":"Explore","usage":{"total_tokens":50284,"tool_uses":1,"duration_ms":8422},"last_tool_name":"Read","uuid":"u-p1"}),
                sub(
                    "user",
                    "u-s4",
                    json!([{"tool_use_id":"toolu_sub_read","type":"tool_result","content":"# Agent accounts"}]),
                ),
                json!({"type":"system","subtype":"task_updated","task_id":TASK,"patch":{"status":"completed","end_time":1_793_658_537_330_i64},"uuid":"u-up"}),
                json!({"type":"system","subtype":"task_notification","task_id":TASK,"tool_use_id":SPAWN,"status":"completed","output_file":"C:\\Temp\\tasks\\a3c.output","summary":"## Findings\n\nAccounts switch in `accounts.rs`.","usage":{"total_tokens":61000,"tool_uses":4,"duration_ms":60100},"uuid":"u-note"}),
                json!({"type":"system","subtype":"background_tasks_changed","tasks":[],"uuid":"u-bg2"}),
            ],
        );
        // The subagent's own words and calls never reach the transcript.
        assert!(!events.iter().any(|event| matches!(
            event,
            GhostEvent::MessageDelta { .. } | GhostEvent::ThoughtDelta { .. }
        )));
        let calls: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::ToolCall(call) => Some(call.tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(calls, [SPAWN]);

        let records = tasks(&events);
        let first = records[0];
        assert_eq!(
            (
                first.task_id.as_str(),
                first.kind,
                first.status,
                first.backgrounded
            ),
            (TASK, TaskKind::Agent, TaskStatus::Running, true)
        );
        assert_eq!(first.title, "Map account switching code");
        assert_eq!(first.agent_type.as_deref(), Some("Explore"));
        // The start names the transcript's call and carries the prompt, once.
        let start = records
            .iter()
            .find(|task| task.prompt.is_some())
            .expect("the prompt");
        assert_eq!(start.tool_call_id.as_deref(), Some(SPAWN));
        assert_eq!(
            records.iter().filter(|task| task.prompt.is_some()).count(),
            1
        );
        let progress = records
            .iter()
            .find(|task| task.activity.is_some())
            .expect("progress");
        assert_eq!(
            progress.activity.as_deref(),
            Some("Reading docs\\agent-accounts.md")
        );
        assert_eq!(
            (progress.tokens, progress.tool_uses, progress.duration_ms),
            (Some(50284), Some(1), Some(8422))
        );
        let last = records.last().expect("the end");
        assert_eq!(last.status, TaskStatus::Completed);
        assert_eq!(last.ended_at, Some(1_793_658_537_330));
        assert_eq!(
            last.summary.as_deref(),
            Some("## Findings\n\nAccounts switch in `accounts.rs`.")
        );
        assert_eq!((last.tokens, last.tool_uses), (Some(61000), Some(4)));
        assert_eq!(
            records.iter().filter(|task| task.summary.is_some()).count(),
            1
        );
        let serialized = serde_json::to_string(&events).expect("serialize");
        assert!(
            !serialized.contains("a3c.output"),
            "the output file stays private"
        );
        assert!(!serialized.contains("private"), "no signatures");

        let steps = steps(&events);
        assert!(steps.iter().all(|(task, _)| *task == TASK));
        let shown: Vec<_> = steps
            .iter()
            .map(|(_, step)| (step.kind, step.title.as_str(), step.tool_kind, step.status))
            .collect();
        assert_eq!(
            shown,
            [
                (TaskStepKind::Text, "", None, None),
                (
                    TaskStepKind::Tool,
                    "Read agent-accounts.md",
                    Some(ToolKind::Read),
                    Some(ToolStatus::InProgress)
                ),
                (
                    TaskStepKind::Tool,
                    "Read agent-accounts.md",
                    Some(ToolKind::Read),
                    Some(ToolStatus::Completed)
                ),
            ]
        );
        assert_eq!(
            steps[0].1.text.as_deref(),
            Some("I'll start with the **docs**.")
        );
        assert_eq!(steps[1].1.step_id, steps[2].1.step_id, "upserted by id");
    }

    #[test]
    fn foreground_commands_stay_hidden_until_moved_to_the_background() {
        let mut translator = Translator::with_prefix("spawn");
        let foreground = |id: &str| json!({"type":"system","subtype":"task_started","task_id":id,"run_id":"r","tool_use_id":format!("toolu_{id}"),"description":"Run the tests","is_backgrounded":false,"task_type":"local_bash","uuid":format!("s-{id}")});
        let events = feed(
            &mut translator,
            &[
                foreground("quick"),
                json!({"type":"system","subtype":"task_notification","task_id":"quick","tool_use_id":"toolu_quick","status":"completed","output_file":"","summary":"Run the tests"}),
                foreground("slow"),
            ],
        );
        assert!(events.is_empty(), "{events:?}");
        let events = feed(
            &mut translator,
            &[
                json!({"type":"system","subtype":"task_updated","task_id":"slow","patch":{"is_backgrounded":true}}),
                json!({"type":"system","subtype":"task_updated","task_id":"slow","patch":{"status":"killed","end_time":5}}),
                json!({"type":"system","subtype":"task_notification","task_id":"slow","status":"stopped","output_file":"C:\\x.output","summary":"Background command \"Run the tests\" was stopped"}),
            ],
        );
        let records = tasks(&events);
        assert_eq!(records.len(), 3, "{records:?}");
        assert_eq!(
            (records[0].kind, records[0].status, records[0].backgrounded),
            (TaskKind::Command, TaskStatus::Running, true)
        );
        assert_eq!(records[0].tool_call_id, None, "no call of that id is shown");
        assert_eq!(
            (records[1].status, records[1].ended_at),
            (TaskStatus::Stopped, Some(5))
        );
        assert_eq!(records[2].status, TaskStatus::Stopped);
        assert!(records[2].summary.is_some());
    }

    #[test]
    fn a_subagents_own_tasks_name_their_parent_and_failures_show() {
        let mut translator = Translator::with_prefix("spawn");
        let events = feed(
            &mut translator,
            &[
                agent_call(),
                started(),
                sub(
                    "assistant",
                    "u-s1",
                    json!([{"type":"tool_use","id":"toolu_sub_bash","name":"Bash","input":{"command":"pnpm test","run_in_background":true}}]),
                ),
                json!({"type":"system","subtype":"task_started","task_id":"bnested","tool_use_id":"toolu_sub_bash","description":"Run the browser suite","is_backgrounded":true,"task_type":"local_bash","parent_task_id":TASK,"owned_by_subagent":true}),
                json!({"type":"system","subtype":"task_notification","task_id":"bnested","tool_use_id":"toolu_sub_bash","status":"failed","output_file":"","summary":"Background command \"Run the browser suite\" failed with exit code 1"}),
                sub(
                    "user",
                    "u-s2",
                    json!([{"tool_use_id":"toolu_sub_bash","type":"tool_result","is_error":true,"content":"exit 1"}]),
                ),
            ],
        );
        let nested: Vec<_> = tasks(&events)
            .into_iter()
            .filter(|task| task.task_id == "bnested")
            .collect();
        assert_eq!(nested.len(), 2);
        assert_eq!(nested[0].parent_task_id.as_deref(), Some(TASK));
        assert_eq!(
            nested[0].tool_call_id, None,
            "its call is a step, not a transcript call"
        );
        assert_eq!(nested[1].status, TaskStatus::Failed);
        let statuses: Vec<_> = steps(&events).iter().map(|(_, step)| step.status).collect();
        assert_eq!(
            statuses,
            [Some(ToolStatus::InProgress), Some(ToolStatus::Failed)]
        );
    }

    #[test]
    fn messages_before_the_start_show_the_task_from_what_they_say() {
        let mut translator = Translator::with_prefix("spawn");
        let events = feed(
            &mut translator,
            &[
                agent_call(),
                sub(
                    "assistant",
                    "u-s1",
                    json!([{"type":"text","text":"Looking."}]),
                ),
                started(),
            ],
        );
        let records = tasks(&events);
        assert_eq!(records[0].title, "Map account switching code");
        assert_eq!(records[0].tool_call_id.as_deref(), Some(SPAWN));
        assert!(records.iter().all(|task| task.task_id == TASK));
        assert_eq!(
            records.last().and_then(|task| task.prompt.as_deref()),
            Some("In the repo, find where accounts switch.")
        );
    }

    #[test]
    fn long_texts_are_cut_on_a_character_boundary() {
        let text = "é".repeat(MAX_PROMPT_BYTES);
        let shortened = cut(&text, MAX_PROMPT_BYTES);
        assert!(shortened.len() <= MAX_PROMPT_BYTES);
        assert!(shortened.ends_with('…'));
        assert_eq!(cut("short", 10), "short");
    }
}
