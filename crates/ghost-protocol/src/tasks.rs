//! Background work an agent starts inside a chat: a subagent it hands part of the job to, or a shell
//! command it leaves running in the background. Claude Code reports both as tasks; the chat shows
//! them above the composer and in a panel that follows each one's progress.
//!
//! [`GhostEvent::TaskUpdated`](crate::GhostEvent::TaskUpdated) carries a task's whole record except
//! its two long texts: the prompt is sent with the start and the summary with the end, and `None`
//! keeps the text already known. [`GhostEvent::TaskStep`](crate::GhostEvent::TaskStep) adds or
//! replaces one step of a task's work by its `step_id`.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::lenient::lenient_enum;
use crate::{Millis, ToolKind, ToolStatus, Truncation};

lenient_enum! {
    /// What a task runs.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum TaskKind {
        /// A subagent with its own conversation.
        Agent = "agent",
        /// A shell command.
        Command = "command",
        /// A kind Ghost does not know (an agent's newer kind, or a newer build's).
        Other = "other",
    }
    fallback = Other;
}

lenient_enum! {
    /// Where a task is in its life.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum TaskStatus {
        Running = "running",
        Completed = "completed",
        Failed = "failed",
        /// Stopped or killed, or its turn or its agent's process ended before it reported an end.
        Stopped = "stopped",
    }
    fallback = Stopped;
}

/// One background task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BackgroundTask {
    pub task_id: String,
    pub kind: TaskKind,
    /// What the agent said the task is for ("Map account switching code").
    pub title: String,
    /// A subagent's type ("Explore", "general-purpose").
    pub agent_type: Option<String>,
    /// The transcript's tool call that started it, when that call is shown in the transcript.
    pub tool_call_id: Option<String>,
    /// The task that started this one (a subagent's own subagent or command).
    pub parent_task_id: Option<String>,
    /// It runs in the background: the turn that started it can end first.
    pub backgrounded: bool,
    pub status: TaskStatus,
    /// What it does right now ("Reading docs/agent-accounts.md").
    pub activity: Option<String>,
    pub tool_uses: Option<u32>,
    pub tokens: Option<u64>,
    /// How long it ran, as the agent reported it.
    pub duration_ms: Option<u64>,
    /// When Ghost first heard of it.
    pub started_at: Option<Millis>,
    pub ended_at: Option<Millis>,
    /// What the task was asked to do, cut to 4 KiB. `None` keeps the prompt already known.
    pub prompt: Option<String>,
    /// The task's final report (markdown). `None` keeps the summary already known.
    pub summary: Option<String>,
    /// The summary was cut to fit the event budget; `blob` holds all of it.
    pub summary_truncated: Option<Truncation>,
}

lenient_enum! {
    /// What a [`TaskStep`] shows.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum TaskStepKind {
        /// A tool call the task made.
        Tool = "tool",
        /// Text the task's agent wrote.
        Text = "text",
    }
    fallback = Text;
}

/// One step of a task's work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TaskStep {
    pub step_id: String,
    pub kind: TaskStepKind,
    /// A tool's readable title ("Read main.rs"); empty for text.
    pub title: String,
    pub tool_kind: Option<ToolKind>,
    /// A tool's status.
    pub status: Option<ToolStatus>,
    /// A text step's markdown.
    pub text: Option<String>,
    /// The text was cut to fit the event budget; `blob` holds all of it.
    pub truncated: Option<Truncation>,
}
