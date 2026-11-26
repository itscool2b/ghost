//! The shared vocabulary of Ghost.
//!
//! Every agent driver (Claude Code, Codex, Pi, any ACP agent) translates its native stream into
//! [`GhostEvent`]s, the UI renders only `GhostEvent`s, and the chat store saves them. The shape follows
//! the Agent Client Protocol's `session/update` vocabulary because that is already the cross-agent
//! standard; native-only extras (rate-limit windows, effort levels) are additive. [`session`] holds
//! the IPC and session-state types the desktop commands exchange.
//!
//! Wire format: JSON, internally tagged with `"type"` in `snake_case`. Timestamps are Unix
//! milliseconds (`i64`), exported to TypeScript as `number` (safe up to 2^53).
//!
//! Stored events outlive the build that wrote them. Any field added to a stored type from M1a on is
//! an `Option` with `#[serde(default)]`, and the enums that reach storage or come from agents read a
//! name they do not know as their fallback (`lenient.rs`).

pub mod activity;
pub mod agents;
pub mod changes;
pub mod commands;
pub mod composer;
pub mod config_vendor;
pub mod conversation;
pub mod coordinator;
pub mod folder_resolution;
pub mod forms;
pub mod git;
mod lenient;
pub mod mcp;
pub mod memory;
pub mod plugins;
pub mod project_tools;
pub mod remote;
pub mod repository_publication;
pub mod search;
pub mod session;
pub mod setup_actions;
pub mod tasks;
pub mod workspace;
pub mod workspace_removal;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::lenient::lenient_enum;

/// Unix time in milliseconds.
pub type Millis = i64;

/// Upper bound for one serialized [`EventBatch`] frame sent to the UI. Tauri evaluates JSON payloads
/// under 8 KiB inline and falls back to a slower fetch path above that, so producers split batches to
/// stay below this.
pub const MAX_FRAME_BYTES: usize = 8 * 1024;

/// What every non-delta event serializes within once the session's normalizer has cut it, leaving
/// room for a frame's own fields. Text deltas have no cap: the frame batcher splits them.
pub const EVENT_BUDGET: usize = MAX_FRAME_BYTES - 1024;

/// One normalized event in a chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GhostEvent {
    /// The session actor started a turn for the queued prompt `prompt_id`.
    TurnStarted {
        turn_id: String,
        prompt_id: Option<String>,
    },
    TurnEnded {
        turn_id: String,
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
        duration_ms: Option<u64>,
        /// Ended by recovery at startup: Ghost quit or crashed while the turn ran.
        recovered: bool,
    },
    /// The user's prompt text, as sent. The session actor emits it; drivers never do.
    UserMessageDelta {
        message_id: String,
        text: String,
    },
    // Delivery state for an explicit user message directed to an already running turn.
    SteeringStatus {
        message_id: String,
        state: composer::SteeringState,
        detail: Option<String>,
    },
    MessageDelta {
        message_id: String,
        text: String,
    },
    MessageCompleted {
        message_id: String,
    },
    ThoughtDelta {
        thought_id: String,
        text: String,
    },
    ThoughtCompleted {
        thought_id: String,
        duration_ms: Option<u32>,
    },
    ToolCall(ToolCall),
    ToolUpdate(ToolUpdate),
    PermissionRequested(PermissionRequest),
    PermissionResolved {
        request_id: String,
        decision: PermissionDecision,
        by: ResolvedBy,
    },
    UiRequest(UiRequest),
    /// The answer to a [`UiRequest`]; `None` when it was dismissed, withdrawn or closed.
    UiResolved {
        request_id: String,
        answer: Option<UiAnswer>,
        by: ResolvedBy,
    },
    PlanUpdated {
        entries: Vec<PlanEntry>,
    },
    /// A background task's record: a subagent or a background command ([`tasks`]).
    TaskUpdated(tasks::BackgroundTask),
    /// One step of a background task's work, added or replaced by its `step_id`.
    TaskStep {
        task_id: String,
        step: tasks::TaskStep,
    },
    UsageUpdated(UsageInfo),
    LimitsUpdated {
        windows: Vec<LimitWindow>,
    },
    /// Live header state; never stored.
    SessionUpdated(SessionInfo),
    /// What happened to a queued prompt (the composer's queue); never stored.
    PromptLifecycle {
        prompt_id: String,
        state: PromptState,
        /// The prompt's first 256 characters, sent with `Queued`.
        preview: Option<String>,
        /// Exact hold when queued or when a queued row's hold changes; absent for other states
        /// and older producers.
        #[serde(default)]
        held: Option<bool>,
    },
    Notice {
        level: NoticeLevel,
        message: String,
    },
    Error {
        kind: ErrorKind,
        message: String,
        /// More context, such as the agent's redacted stderr tail.
        detail: Option<String>,
        recoverable: bool,
    },
}

impl GhostEvent {
    /// The serde tag (`"message_delta"`, ...), stored in `events.kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            GhostEvent::TurnStarted { .. } => "turn_started",
            GhostEvent::TurnEnded { .. } => "turn_ended",
            GhostEvent::UserMessageDelta { .. } => "user_message_delta",
            GhostEvent::SteeringStatus { .. } => "steering_status",
            GhostEvent::MessageDelta { .. } => "message_delta",
            GhostEvent::MessageCompleted { .. } => "message_completed",
            GhostEvent::ThoughtDelta { .. } => "thought_delta",
            GhostEvent::ThoughtCompleted { .. } => "thought_completed",
            GhostEvent::ToolCall(_) => "tool_call",
            GhostEvent::ToolUpdate(_) => "tool_update",
            GhostEvent::PermissionRequested(_) => "permission_requested",
            GhostEvent::PermissionResolved { .. } => "permission_resolved",
            GhostEvent::UiRequest(_) => "ui_request",
            GhostEvent::UiResolved { .. } => "ui_resolved",
            GhostEvent::PlanUpdated { .. } => "plan_updated",
            GhostEvent::TaskUpdated(_) => "task_updated",
            GhostEvent::TaskStep { .. } => "task_step",
            GhostEvent::UsageUpdated(_) => "usage_updated",
            GhostEvent::LimitsUpdated { .. } => "limits_updated",
            GhostEvent::SessionUpdated(_) => "session_updated",
            GhostEvent::PromptLifecycle { .. } => "prompt_lifecycle",
            GhostEvent::Notice { .. } => "notice",
            GhostEvent::Error { .. } => "error",
        }
    }

    /// Whether the event is stored as a history row. `in_turn`: the event is tagged with a turn.
    ///
    /// Live state belongs in snapshots, so `SessionUpdated`, `PromptLifecycle` and `LimitsUpdated`
    /// are never stored, and `UsageUpdated` only inside a turn. Everything else is, including agent
    /// output that arrives outside any turn.
    pub fn is_stored(&self, in_turn: bool) -> bool {
        match self {
            GhostEvent::SessionUpdated(_)
            | GhostEvent::PromptLifecycle { .. }
            | GhostEvent::LimitsUpdated { .. } => false,
            GhostEvent::UsageUpdated(_) => in_turn,
            GhostEvent::TurnStarted { .. }
            | GhostEvent::TurnEnded { .. }
            | GhostEvent::UserMessageDelta { .. }
            | GhostEvent::SteeringStatus { .. }
            | GhostEvent::MessageDelta { .. }
            | GhostEvent::MessageCompleted { .. }
            | GhostEvent::ThoughtDelta { .. }
            | GhostEvent::ThoughtCompleted { .. }
            | GhostEvent::ToolCall(_)
            | GhostEvent::ToolUpdate(_)
            | GhostEvent::PermissionRequested(_)
            | GhostEvent::PermissionResolved { .. }
            | GhostEvent::UiRequest(_)
            | GhostEvent::UiResolved { .. }
            | GhostEvent::PlanUpdated { .. }
            | GhostEvent::TaskUpdated(_)
            | GhostEvent::TaskStep { .. }
            | GhostEvent::Notice { .. }
            | GhostEvent::Error { .. } => true,
        }
    }
}

lenient_enum! {
    /// Why a turn ended.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum StopReason {
        EndTurn = "end_turn",
        MaxTokens = "max_tokens",
        MaxTurnRequests = "max_turn_requests",
        Refusal = "refusal",
        /// The agent confirmed a cancel.
        Cancelled = "cancelled",
        /// Ghost ended the turn without the agent: a force stop, a crash, a quit.
        Interrupted = "interrupted",
        Error = "error",
        /// A reason Ghost does not know (an agent's, or a newer build's).
        Other = "other",
    }
    fallback = Other;
}

/// Token accounting for a turn, when the agent reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Tokens read from the prompt cache (ACP `cachedReadTokens`, Claude `cache_read_input_tokens`).
    pub cache_read_tokens: Option<u64>,
    /// Tokens written to the prompt cache (ACP `cachedWriteTokens`, Claude
    /// `cache_creation_input_tokens`).
    pub cache_write_tokens: Option<u64>,
    /// ACP `thoughtTokens`.
    pub reasoning_tokens: Option<u64>,
    pub scope: UsageScope,
}

lenient_enum! {
    /// What a [`TokenUsage`] counts.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum UsageScope {
        /// This turn alone.
        #[default]
        Turn = "turn",
        /// The whole native session so far ("this session" in the UI).
        Session = "session",
    }
    fallback = Session;
}

/// A tool invocation started by the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub kind: ToolKind,
    pub title: String,
    pub status: ToolStatus,
    pub locations: Vec<ToolLocation>,
    /// The tool's raw input as the agent sent it, for the expanded view.
    pub raw_input: Option<serde_json::Value>,
    /// The tool's own name (ACP `ToolCall.name`).
    pub tool_name: Option<String>,
    /// ACP tool calls carry content from the start.
    pub content: Vec<ToolContent>,
    /// More than entry text was cut to fit the event budget; `blob` holds the original event's JSON.
    pub truncated: Option<Truncation>,
}

/// Progress or result for a previously announced [`ToolCall`]. Mirrors ACP's update: `None` keeps a
/// field; `content: Some(v)` replaces the content, and `Some([])` clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ToolUpdate {
    pub tool_call_id: String,
    pub status: Option<ToolStatus>,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub locations: Option<Vec<ToolLocation>>,
    pub raw_input: Option<serde_json::Value>,
    /// ACP `ToolCallUpdateFields.name`.
    pub tool_name: Option<String>,
    pub content: Option<Vec<ToolContent>>,
    /// As on [`ToolCall`].
    pub truncated: Option<Truncation>,
}

lenient_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum ToolKind {
        Read = "read",
        Edit = "edit",
        Delete = "delete",
        Move = "move",
        Search = "search",
        Execute = "execute",
        Think = "think",
        Fetch = "fetch",
        SwitchMode = "switch_mode",
        Other = "other",
    }
    fallback = Other;
}

lenient_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum ToolStatus {
        Pending = "pending",
        InProgress = "in_progress",
        Completed = "completed",
        Failed = "failed",
    }
    fallback = Pending;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ToolLocation {
    pub path: String,
    pub line: Option<u32>,
}

/// Rich content attached to a tool call or update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContent {
    Text {
        text: String,
        truncated: Option<Truncation>,
    },
    Diff {
        path: String,
        /// `None` when the file is new.
        old_text: Option<String>,
        new_text: String,
        truncated: Option<Truncation>,
    },
    Terminal {
        output: String,
        exit_code: Option<i32>,
        truncated: Option<Truncation>,
    },
}

/// Inline content was cut; `blob` is the sha256 of the full content in the profile's attachments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Truncation {
    pub total_bytes: u64,
    pub blob: Option<String>,
}

/// The agent asks before running a tool, or asks the user a question through a permission request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PermissionRequest {
    pub request_id: String,
    /// Set only when it names a tool block already shown.
    pub tool_call_id: Option<String>,
    pub title: String,
    /// What the permission policy decides on; `None` is never auto-answered.
    pub kind: Option<ToolKind>,
    /// Some agents' permission requests never become tool blocks, so the card shows the input.
    pub raw_input: Option<serde_json::Value>,
    pub content: Vec<ToolContent>,
    pub locations: Vec<ToolLocation>,
    pub options: Vec<PermissionOption>,
    /// As on [`ToolCall`].
    pub truncated: Option<Truncation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PermissionOption {
    pub option_id: String,
    pub kind: PermissionOptionKind,
    pub label: String,
}

lenient_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum PermissionOptionKind {
        AllowOnce = "allow_once",
        AllowAlways = "allow_always",
        RejectOnce = "reject_once",
        RejectAlways = "reject_always",
        /// Shown, never auto-selected.
        Other = "other",
    }
    fallback = Other;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionDecision {
    Selected {
        option_id: String,
        /// The plan as the person edited it before accepting it: only on a plan approval that
        /// was accepted with edits ([`Capabilities::plan_edits`]). `None` for every other answer,
        /// and absent from what is stored and sent then, so older records read as they did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
    Cancelled,
}

lenient_enum! {
    /// Who answered a permission request or a question.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum ResolvedBy {
        User = "user",
        /// The permission policy, under the chat's autonomy.
        Auto = "auto",
        /// The agent withdrew it.
        Agent = "agent",
        /// Its turn ended or was cancelled.
        Turn = "turn",
        /// Recovery, or the agent's process ended.
        System = "system",
        Other = "other",
    }
    fallback = Other;
}

/// An agent asks the UI for input that isn't a tool permission (Claude or Cursor questions,
/// Pi extension UI, ACP elicitation, MCP forms). Never auto-answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct UiRequest {
    pub request_id: String,
    pub prompt: UiPrompt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiPrompt {
    Confirm {
        message: String,
    },
    Select {
        message: String,
        options: Vec<String>,
    },
    Input {
        message: String,
        placeholder: Option<String>,
    },
    // A group of independent questions, answered together and never by the permission policy.
    Questions {
        message: String,
        questions: Vec<UiQuestion>,
    },
    Notify {
        message: String,
    },
    /// An MCP server's form (MCP elicitation, form mode), answered with [`UiAnswer::Form`].
    /// `source` names the server.
    Form {
        message: String,
        source: Option<String>,
        fields: Vec<forms::UiField>,
    },
    /// An MCP server asks the user to finish something in the browser (URL mode). Answered
    /// with [`UiAnswer::Confirm`]: yes once the page is opened, no to decline.
    Link {
        message: String,
        source: Option<String>,
        /// http or https only.
        url: String,
    },
}

/// One question in a grouped prompt. The driver keeps the original native question and maps it by
/// `id`, so display normalization never changes the key sent back to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UiQuestion {
    /// Stable within the request and unique among its questions.
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<UiQuestionOption>,
    pub multi_select: bool,
    pub allow_free_text: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UiQuestionOption {
    pub label: String,
    pub description: String,
}

/// The answer to a [`UiRequest`], matching its [`UiPrompt`] (`Notify` takes none).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiAnswer {
    Confirm {
        yes: bool,
    },
    Select {
        index: u32,
    },
    Input {
        text: String,
    },
    Questions {
        answers: Vec<UiQuestionAnswer>,
    },
    /// A form submitted (`accept`) with its answered fields, or declined with none.
    Form {
        accept: bool,
        values: Vec<forms::UiFieldValue>,
    },
}

/// One answer per question. A single-select question accepts one option or free text; a
/// multi-select question accepts multiple options and optional free text. Nothing is preselected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UiQuestionAnswer {
    pub question_id: String,
    /// Distinct indices into the question's options, in their displayed order.
    pub selected: Vec<u32>,
    /// Nonblank when present, and accepted only when the question allows free text.
    pub text: Option<String>,
}

/// One piece of a prompt. M1a sends `Text` only; M2c adds images and mentions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptPart {
    Text {
        text: String,
    },
    /// `blob` is the sha256 of the image in the profile's attachments.
    Image {
        blob: String,
        mime: String,
    },
    Mention {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PlanEntry {
    pub content: String,
    pub status: PlanStatus,
    pub priority: Option<PlanPriority>,
}

lenient_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum PlanStatus {
        Pending = "pending",
        InProgress = "in_progress",
        Completed = "completed",
    }
    fallback = Pending;
}

lenient_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum PlanPriority {
        High = "high",
        Medium = "medium",
        Low = "low",
    }
    fallback = Medium;
}

/// Context-window and spend information for the session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct UsageInfo {
    pub context_used: Option<u64>,
    pub context_size: Option<u64>,
    /// The session's cumulative cost.
    pub cost: Option<Cost>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Cost {
    pub amount: f64,
    pub currency: String,
}

/// One account rate-limit window (Claude's five-hour/weekly windows, Codex's primary/secondary).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct LimitWindow {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<Millis>,
}

/// Live header state of a chat. Never stored; coalesced to at most one per frame.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionInfo {
    #[serde(default)]
    pub worktree_setup: Option<worktree_setup::WorktreeSetupAttempt>,
    pub status: SessionStatus,
    /// What is happening now: "Starting Hermes", "Stopping the turn; press Esc again to force stop".
    pub detail: Option<String>,
    pub native_session_id: Option<String>,
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
    pub title: Option<String>,
    /// Display labels of the current Model, Effort and Mode choices.
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub autonomy: Autonomy,
    /// Some queued prompts are held ("Send queued" shows).
    pub queue_held: bool,
    /// Increments when capabilities, options, slash commands or the autonomy state change: the UI
    /// refetches `chat_lists`.
    pub revision: u32,
    /// Why the queued prompts are not going to the agent yet, while any are queued. `None` with
    /// prompts queued means the next one is on its way.
    pub queue_wait: Option<QueueWait>,
    /// The prompt on its way to the agent while its folder checkpoint saves: shown as sending in
    /// the transcript, not in the queue. `None` once it went, or when a stop or a failed
    /// checkpoint put it back in the queue, held.
    #[serde(default)]
    pub sending: Option<session::SendingPrompt>,
    /// The added account ([`agents::AgentAccount`]) the current or last start signed in with.
    /// `None`: the agent's own sign-in, or no start yet.
    #[serde(default)]
    pub account_id: Option<String>,
    /// The agent said its usage limit is reached. Cleared when a turn completes or the agent
    /// starts again, on another account or after the reset.
    #[serde(default)]
    pub usage_limit: Option<UsageLimit>,
}

/// An agent's own report that its account's usage limit is reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UsageLimit {
    /// The agent's own words ("You've hit your 5-hour limit · resets 5:30pm (America/New_York)").
    pub message: String,
    pub resets_at: Option<Millis>,
}

/// Why queued prompts wait. Prompts go in order: a held prompt waits for the person to send or
/// discard it, and every prompt queued behind it waits too ([`QueueWait::Held`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum QueueWait {
    Setup {
        state: worktree_setup::WorktreeSetupState,
        detail: Option<String>,
    },
    /// The agent is starting or resuming its session.
    Starting,
    /// The agent is working on a turn.
    Working,
    /// The turn ended, but the agent still reports work in progress (its Stop hooks, background
    /// tasks) since `since`. The next prompt goes when it reports idle, or at `sends_at` without
    /// that report.
    FinishingTurn {
        since: Option<Millis>,
        sends_at: Option<Millis>,
    },
    /// A permission request (or, with `question`, a question) waits for the person's answer.
    NeedsAnswer { question: bool },
    /// A checkpoint of the folder is being saved before the next prompt goes.
    SavingCheckpoint,
    /// The turn is being cancelled, or the agent is stopping.
    Stopping,
    /// A change to this chat (a move, a revert, a Git action or a worktree setup) is in progress.
    ChatChanging,
    /// The agent could not start; its error card says why.
    StartFailed,
    /// The agent needs its own sign-in.
    SignIn,
    /// The agent is stopped.
    Stopped,
    /// The chat's folder is missing.
    FolderMissing,
    /// The chat's project or folder was removed.
    WorkspaceRemoved,
    /// The next prompt is held, since `after`, and waits for the person with any behind it.
    Held { after: HoldCause },
}

/// What held the queued prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum HoldCause {
    /// Ghost restarted (or reopened the chat) with prompts still queued.
    Restart,
    /// The person stopped the turn or the agent.
    Stop,
    /// A turn failed, the agent crashed, or a checkpoint could not be saved.
    Error,
    /// The agent asked for its own sign-in.
    SignIn,
    /// The chat's project or folder was removed.
    WorkspaceRemoved,
    /// The agent reached its usage limit. Another account, or the reset, continues the chat.
    UsageLimit,
}

/// Where a chat's agent is in its life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// No process.
    #[default]
    Stopped,
    Starting,
    /// The process is up and no turn runs.
    Ready,
    /// Exactly one turn is in flight.
    Running,
    /// The agent was asked to cancel; waiting for its reply.
    Cancelling,
    Stopping,
    /// Could not start; no process.
    Failed,
    /// The agent needs its own sign-in; never a live process.
    AuthRequired,
}

/// What happened to a queued prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PromptState {
    Queued,
    /// On its way: its folder checkpoint is being saved, and it goes to the agent right after.
    Sending,
    Started,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub input_hint: Option<String>,
}

lenient_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum NoticeLevel {
        Info = "info",
        Warning = "warning",
        Error = "error",
    }
    fallback = Info;
}

lenient_enum! {
    /// What kind of failure an [`GhostEvent::Error`] reports, which decides the error card's actions.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum ErrorKind {
        AgentUnavailable = "agent_unavailable",
        FolderMissing = "folder_missing",
        FolderNotAllowed = "folder_not_allowed",
        SpawnFailed = "spawn_failed",
        StartTimeout = "start_timeout",
        ProtocolMismatch = "protocol_mismatch",
        AuthRequired = "auth_required",
        /// The account the agent runs on reached its usage limit.
        UsageLimit = "usage_limit",
        AgentError = "agent_error",
        AgentExited = "agent_exited",
        ProtocolError = "protocol_error",
        Storage = "storage",
        Internal = "internal",
        Other = "other",
    }
    fallback = Other;
}

/// How much an agent may do without asking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Autonomy {
    #[default]
    FullAuto,
    AutoEdit,
    Ask,
}

/// What the agent's dialect made of the requested autonomy. The session's permission policy reads
/// `auto_answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AutonomyState {
    pub requested: Autonomy,
    /// The agent mode in effect, when Ghost knows it.
    pub native_mode: Option<String>,
    pub auto_answer: AutoAnswer,
    /// Shown next to the autonomy picker.
    pub note: Option<String>,
}

/// Which permission requests the session answers on the user's behalf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutoAnswer {
    Never,
    /// Requests of the eight tool kinds; never `switch_mode`, `other` or a request without a kind.
    All,
    Kinds {
        kinds: Vec<ToolKind>,
    },
}

/// Which driver runs an agent. M1c and M1d add Codex app-server and Pi RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DriverKind {
    Acp,
    Claude,
    Codex,
    Headless,
    Pi,
    Mock,
}

/// What a driver supports. The UI shows or hides features from this, never from agent names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Capabilities {
    pub images: bool,
    pub steer: bool,
    pub interrupt: bool,
    pub resume: bool,
    pub fork: bool,
    pub rewind: bool,
    pub set_model: bool,
    pub set_effort: bool,
    pub permissions: bool,
    pub plans: bool,
    pub usage: bool,
    pub limits: bool,
    pub slash_commands: bool,
    pub add_dirs: bool,
    /// A plan waiting for approval can be accepted as the person edited it: the agent is told
    /// the edited text is the approved plan. Written only when true, so what an agent without
    /// it reports, and every record made before it existed, reads the same as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plan_edits: bool,
    pub autonomy_levels: Vec<Autonomy>,
}

/// A frame of events for one chat, sent from the core to the UI over a Tauri channel.
///
/// `seq_start` is the chat-wide sequence number of the first event, so the UI can detect gaps and
/// drop duplicates after a reconnect. Stored rows carry the same numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct EventBatch {
    pub chat_id: String,
    pub seq_start: u32,
    pub events: Vec<GhostEvent>,
}

impl EventBatch {
    /// Serialized size in bytes, used to keep frames under [`MAX_FRAME_BYTES`].
    pub fn encoded_len(&self) -> usize {
        serde_json::to_vec(self)
            .map(|v| v.len())
            .unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer is stored and sent as it always was; only a plan accepted with edits adds the
    /// plan, and an answer written before plans could be edited still reads.
    #[test]
    fn a_decision_keeps_an_edited_plan_and_older_answers_still_read() {
        let plain = PermissionDecision::Selected {
            option_id: "allow".into(),
            plan: None,
        };
        let stored = serde_json::to_value(&plain).unwrap();
        assert_eq!(
            stored,
            serde_json::json!({"type": "selected", "option_id": "allow"})
        );
        assert_eq!(
            serde_json::from_value::<PermissionDecision>(stored).unwrap(),
            plain
        );
        let edited = PermissionDecision::Selected {
            option_id: "allow".into(),
            plan: Some("1. Build\n2. Test".into()),
        };
        let stored = serde_json::to_value(&edited).unwrap();
        assert_eq!(stored["plan"], "1. Build\n2. Test");
        assert_eq!(
            serde_json::from_value::<PermissionDecision>(stored).unwrap(),
            edited
        );
    }
    use serde_json::json;

    #[test]
    fn queued_lifecycle_retains_exact_hold_and_accepts_legacy_missing_hold() {
        let event: GhostEvent = serde_json::from_value(json!({
            "type": "prompt_lifecycle", "prompt_id": "held", "state": "queued",
            "preview": "new held message", "held": true
        }))
        .unwrap();
        assert!(matches!(
            event,
            GhostEvent::PromptLifecycle {
                held: Some(true),
                ..
            }
        ));
        assert_eq!(round_trip(&event), event);
        let legacy: GhostEvent = serde_json::from_value(json!({
            "type": "prompt_lifecycle", "prompt_id": "legacy", "state": "queued",
            "preview": "older producer"
        }))
        .unwrap();
        assert!(matches!(
            legacy,
            GhostEvent::PromptLifecycle { held: None, .. }
        ));
    }

    fn round_trip(event: &GhostEvent) -> GhostEvent {
        let text = serde_json::to_string(event).expect("serialize");
        serde_json::from_str(&text).expect("deserialize")
    }

    /// Position of the variant in the declaration. Exhaustive, so a new variant fails to compile
    /// here until the samples below cover it.
    fn variant_index(event: &GhostEvent) -> usize {
        match event {
            GhostEvent::TurnStarted { .. } => 0,
            GhostEvent::TurnEnded { .. } => 1,
            GhostEvent::UserMessageDelta { .. } => 2,
            GhostEvent::MessageDelta { .. } => 3,
            GhostEvent::MessageCompleted { .. } => 4,
            GhostEvent::ThoughtDelta { .. } => 5,
            GhostEvent::ThoughtCompleted { .. } => 6,
            GhostEvent::ToolCall(_) => 7,
            GhostEvent::ToolUpdate(_) => 8,
            GhostEvent::PermissionRequested(_) => 9,
            GhostEvent::PermissionResolved { .. } => 10,
            GhostEvent::UiRequest(_) => 11,
            GhostEvent::UiResolved { .. } => 12,
            GhostEvent::PlanUpdated { .. } => 13,
            GhostEvent::UsageUpdated(_) => 14,
            GhostEvent::LimitsUpdated { .. } => 15,
            GhostEvent::SessionUpdated(_) => 16,
            GhostEvent::PromptLifecycle { .. } => 17,
            GhostEvent::Notice { .. } => 18,
            GhostEvent::Error { .. } => 19,
            GhostEvent::SteeringStatus { .. } => 20,
            GhostEvent::TaskUpdated(_) => 21,
            GhostEvent::TaskStep { .. } => 22,
        }
    }

    const VARIANTS: usize = 23;

    fn truncation() -> Option<Truncation> {
        Some(Truncation {
            total_bytes: 200_000,
            blob: Some("ab".repeat(32)),
        })
    }

    /// Every variant at least once, with every nested type and the optional fields both set and
    /// unset somewhere.
    fn samples() -> Vec<GhostEvent> {
        vec![
            GhostEvent::SteeringStatus {
                message_id: "steer".into(),
                state: composer::SteeringState::Accepted,
                detail: None,
            },
            GhostEvent::TurnStarted {
                turn_id: "t".into(),
                prompt_id: Some("p".into()),
            },
            GhostEvent::TurnStarted {
                turn_id: "t".into(),
                prompt_id: None,
            },
            GhostEvent::TurnEnded {
                turn_id: "t".into(),
                stop_reason: StopReason::Interrupted,
                usage: Some(TokenUsage {
                    input_tokens: 5_000_000_000,
                    output_tokens: 3,
                    cache_read_tokens: Some(9_216),
                    cache_write_tokens: Some(512),
                    reasoning_tokens: None,
                    scope: UsageScope::Session,
                }),
                duration_ms: Some(1_234),
                recovered: true,
            },
            GhostEvent::TurnEnded {
                turn_id: "t".into(),
                stop_reason: StopReason::Other,
                usage: None,
                duration_ms: None,
                recovered: false,
            },
            GhostEvent::UserMessageDelta {
                message_id: "t.u".into(),
                text: "Fix the build 🚀".into(),
            },
            GhostEvent::MessageDelta {
                message_id: "m".into(),
                text: "é 🚀".into(),
            },
            GhostEvent::MessageCompleted {
                message_id: "m".into(),
            },
            GhostEvent::ThoughtDelta {
                thought_id: "th".into(),
                text: "hmm".into(),
            },
            GhostEvent::ThoughtCompleted {
                thought_id: "th".into(),
                duration_ms: Some(1200),
            },
            GhostEvent::ToolCall(ToolCall {
                tool_call_id: "tc".into(),
                kind: ToolKind::Execute,
                title: "Ran npm test".into(),
                status: ToolStatus::InProgress,
                locations: vec![ToolLocation {
                    path: "package.json".into(),
                    line: Some(3),
                }],
                raw_input: Some(json!({"command": "npm test", "z": 1, "a": 2})),
                tool_name: Some("Bash".into()),
                content: vec![ToolContent::Terminal {
                    output: "ok".into(),
                    exit_code: None,
                    truncated: None,
                }],
                truncated: truncation(),
            }),
            GhostEvent::ToolUpdate(ToolUpdate {
                tool_call_id: "tc".into(),
                status: Some(ToolStatus::Completed),
                title: None,
                kind: Some(ToolKind::SwitchMode),
                locations: Some(Vec::new()),
                raw_input: None,
                tool_name: None,
                content: Some(vec![
                    ToolContent::Text {
                        text: "ok".into(),
                        truncated: None,
                    },
                    ToolContent::Diff {
                        path: "a.rs".into(),
                        old_text: None,
                        new_text: "fn x(){}".into(),
                        truncated: truncation(),
                    },
                    ToolContent::Terminal {
                        output: "done".into(),
                        exit_code: Some(0),
                        truncated: truncation(),
                    },
                ]),
                truncated: None,
            }),
            GhostEvent::ToolUpdate(ToolUpdate {
                tool_call_id: "tc".into(),
                status: None,
                title: Some("Read".into()),
                kind: None,
                locations: None,
                raw_input: Some(json!(["argv", 1])),
                tool_name: Some("Read".into()),
                content: Some(Vec::new()),
                truncated: None,
            }),
            GhostEvent::PermissionRequested(PermissionRequest {
                request_id: "p".into(),
                tool_call_id: Some("tc".into()),
                title: "Run rm -rf build?".into(),
                kind: Some(ToolKind::Delete),
                raw_input: Some(json!({"path": "build"})),
                content: vec![ToolContent::Text {
                    text: "rm -rf build".into(),
                    truncated: None,
                }],
                locations: vec![ToolLocation {
                    path: "build".into(),
                    line: None,
                }],
                options: vec![
                    PermissionOption {
                        option_id: "allow".into(),
                        kind: PermissionOptionKind::AllowOnce,
                        label: "Allow".into(),
                    },
                    PermissionOption {
                        option_id: "always".into(),
                        kind: PermissionOptionKind::Other,
                        label: "Always".into(),
                    },
                ],
                truncated: None,
            }),
            GhostEvent::PermissionResolved {
                request_id: "p".into(),
                decision: PermissionDecision::Selected {
                    option_id: "allow".into(),
                    plan: None,
                },
                by: ResolvedBy::Auto,
            },
            GhostEvent::PermissionResolved {
                request_id: "p".into(),
                decision: PermissionDecision::Cancelled,
                by: ResolvedBy::System,
            },
            GhostEvent::UiRequest(UiRequest {
                request_id: "u".into(),
                prompt: UiPrompt::Select {
                    message: "Pick".into(),
                    options: vec!["a".into(), "b".into()],
                },
            }),
            GhostEvent::UiResolved {
                request_id: "u".into(),
                answer: Some(UiAnswer::Select { index: 1 }),
                by: ResolvedBy::User,
            },
            GhostEvent::UiResolved {
                request_id: "u".into(),
                answer: None,
                by: ResolvedBy::Agent,
            },
            GhostEvent::PlanUpdated {
                entries: vec![PlanEntry {
                    content: "Write tests".into(),
                    status: PlanStatus::InProgress,
                    priority: Some(PlanPriority::High),
                }],
            },
            GhostEvent::TaskUpdated(tasks::BackgroundTask {
                task_id: "a3c89c21b19145224".into(),
                kind: tasks::TaskKind::Agent,
                title: "Map account switching code".into(),
                agent_type: Some("Explore".into()),
                tool_call_id: Some("toolu_01".into()),
                parent_task_id: None,
                backgrounded: true,
                status: tasks::TaskStatus::Completed,
                activity: Some("Reading docs/agent-accounts.md".into()),
                tool_uses: Some(12),
                tokens: Some(48_210),
                duration_ms: Some(61_000),
                started_at: Some(1_793_658_476_000),
                ended_at: Some(1_793_658_537_330),
                prompt: Some("Find where accounts switch".into()),
                summary: Some("## Findings\n\nThe switch lives in `accounts.rs`.".into()),
                summary_truncated: truncation(),
            }),
            GhostEvent::TaskUpdated(tasks::BackgroundTask {
                task_id: "b1".into(),
                kind: tasks::TaskKind::Command,
                title: "Run website browser tests".into(),
                agent_type: None,
                tool_call_id: None,
                parent_task_id: Some("a3c89c21b19145224".into()),
                backgrounded: true,
                status: tasks::TaskStatus::Running,
                activity: None,
                tool_uses: None,
                tokens: None,
                duration_ms: None,
                started_at: None,
                ended_at: None,
                prompt: None,
                summary: None,
                summary_truncated: None,
            }),
            GhostEvent::TaskStep {
                task_id: "a3c89c21b19145224".into(),
                step: tasks::TaskStep {
                    step_id: "toolu_02".into(),
                    kind: tasks::TaskStepKind::Tool,
                    title: "Read accounts.rs".into(),
                    tool_kind: Some(ToolKind::Read),
                    status: Some(ToolStatus::Completed),
                    text: None,
                    truncated: None,
                },
            },
            GhostEvent::TaskStep {
                task_id: "a3c89c21b19145224".into(),
                step: tasks::TaskStep {
                    step_id: "u1.0".into(),
                    kind: tasks::TaskStepKind::Text,
                    title: String::new(),
                    tool_kind: None,
                    status: None,
                    text: Some("Now the **driver**.".into()),
                    truncated: truncation(),
                },
            },
            GhostEvent::UsageUpdated(UsageInfo {
                context_used: Some(1000),
                context_size: Some(200_000),
                cost: Some(Cost {
                    amount: 0.42,
                    currency: "USD".into(),
                }),
            }),
            GhostEvent::LimitsUpdated {
                windows: vec![LimitWindow {
                    label: "5h".into(),
                    used_percent: 37.5,
                    resets_at: Some(1_761_300_000_000),
                }],
            },
            GhostEvent::SessionUpdated(SessionInfo {
                status: SessionStatus::AuthRequired,
                detail: Some("Starting Hermes".into()),
                native_session_id: Some("abc".into()),
                agent_name: Some("Hermes".into()),
                agent_version: Some("0.22.5".into()),
                autonomy: Autonomy::Ask,
                queue_held: true,
                revision: 7,
                ..Default::default()
            }),
            GhostEvent::PromptLifecycle {
                prompt_id: "p".into(),
                state: PromptState::Queued,
                preview: Some("Fix the build".into()),
                held: Some(false),
            },
            GhostEvent::Notice {
                level: NoticeLevel::Error,
                message: "untested version".into(),
            },
            GhostEvent::Error {
                kind: ErrorKind::AgentExited,
                message: "The agent stopped unexpectedly (exit code 1).".into(),
                detail: Some("panic: boom".into()),
                recoverable: true,
            },
        ]
    }

    #[test]
    fn the_samples_cover_every_variant() {
        let mut seen = [false; VARIANTS];
        for event in samples() {
            seen[variant_index(&event)] = true;
        }
        assert!(seen.iter().all(|covered| *covered), "{seen:?}");
    }

    #[test]
    fn every_variant_round_trips() {
        for event in &samples() {
            assert_eq!(&round_trip(event), event);
        }
    }

    #[test]
    fn kind_is_the_serde_tag() {
        for event in samples() {
            let value = serde_json::to_value(&event).expect("serialize");
            assert_eq!(value["type"], event.kind(), "{event:?}");
        }
    }

    #[test]
    fn is_stored_for_every_variant() {
        for event in samples() {
            let (in_turn, outside) = match variant_index(&event) {
                // SessionUpdated, LimitsUpdated, PromptLifecycle: live state, never stored.
                15..=17 => (false, false),
                // UsageUpdated: only inside a turn.
                14 => (true, false),
                _ => (true, true),
            };
            assert_eq!(event.is_stored(true), in_turn, "{event:?} in a turn");
            assert_eq!(event.is_stored(false), outside, "{event:?} outside a turn");
        }
    }

    #[test]
    fn tags_are_snake_case() {
        let value = serde_json::to_value(GhostEvent::MessageDelta {
            message_id: "m1".into(),
            text: "hi".into(),
        })
        .expect("serialize");
        assert_eq!(
            value,
            json!({"type": "message_delta", "message_id": "m1", "text": "hi"})
        );
    }

    #[test]
    fn newtype_variants_flatten_their_struct() {
        let value = serde_json::to_value(GhostEvent::ToolCall(ToolCall {
            tool_call_id: "t1".into(),
            kind: ToolKind::Execute,
            title: "Ran npm test".into(),
            status: ToolStatus::InProgress,
            locations: vec![],
            raw_input: Some(json!({"command": "npm test"})),
            tool_name: None,
            content: vec![],
            truncated: None,
        }))
        .expect("serialize");
        assert_eq!(value["type"], "tool_call");
        assert_eq!(value["kind"], "execute");
        assert_eq!(value["status"], "in_progress");
        assert_eq!(value["raw_input"]["command"], "npm test");
    }

    #[test]
    fn json_objects_keep_their_key_order() {
        let event = GhostEvent::ToolCall(ToolCall {
            tool_call_id: "t".into(),
            kind: ToolKind::Other,
            title: "x".into(),
            status: ToolStatus::Pending,
            locations: vec![],
            raw_input: Some(serde_json::from_str(r#"{"zeta": 1, "alpha": 2}"#).expect("json")),
            tool_name: None,
            content: vec![],
            truncated: None,
        });
        let text = serde_json::to_string(&event).expect("serialize");
        assert!(
            text.contains(r#""raw_input":{"zeta":1,"alpha":2}"#),
            "{text}"
        );
    }

    #[test]
    fn tool_update_content_tells_keep_from_clear() {
        let keep: ToolUpdate = serde_json::from_value(json!({
            "tool_call_id": "t", "status": null, "title": null, "kind": null, "locations": null,
            "raw_input": null, "tool_name": null, "content": null, "truncated": null
        }))
        .expect("keep");
        assert_eq!(keep.content, None);
        let clear: ToolUpdate = serde_json::from_value(json!({
            "tool_call_id": "t", "status": null, "title": null, "kind": null, "locations": null,
            "raw_input": null, "tool_name": null, "content": [], "truncated": null
        }))
        .expect("clear");
        assert_eq!(clear.content, Some(Vec::new()));
    }

    #[test]
    fn unknown_event_type_is_an_error_not_a_panic() {
        let parsed: Result<GhostEvent, _> = serde_json::from_str(r#"{"type":"teleport","x":1}"#);
        assert!(parsed.is_err());
    }

    /// A row a newer build wrote: unknown names in lenient enums still parse.
    #[test]
    fn stored_rows_with_unknown_names_read_their_fallbacks() {
        let ended: GhostEvent = serde_json::from_value(json!({
            "type": "turn_ended", "turn_id": "t", "stop_reason": "paused_for_lunch",
            "usage": {"input_tokens": 1, "output_tokens": 2, "cache_read_tokens": null,
                      "cache_write_tokens": null, "reasoning_tokens": null, "scope": "lifetime"},
            "duration_ms": null, "recovered": false
        }))
        .expect("a newer build's turn_ended");
        let GhostEvent::TurnEnded {
            stop_reason, usage, ..
        } = ended
        else {
            panic!("expected turn_ended, got {ended:?}");
        };
        assert_eq!(stop_reason, StopReason::Other);
        assert_eq!(usage.map(|usage| usage.scope), Some(UsageScope::Session));

        let notice: GhostEvent =
            serde_json::from_value(json!({"type": "notice", "level": "fatal", "message": "m"}))
                .expect("notice");
        assert_eq!(
            notice,
            GhostEvent::Notice {
                level: NoticeLevel::Info,
                message: "m".into()
            }
        );
    }

    /// Checks one lenient enum: every variant's wire name is what serde writes and reads back, an
    /// unknown name reads as the fallback, and a value that is not a string is still an error.
    fn check_lenient<T>(all: &[T], fallback: T, as_str: fn(T) -> &'static str)
    where
        T: Copy + PartialEq + std::fmt::Debug + Serialize + serde::de::DeserializeOwned,
    {
        assert!(all.contains(&fallback));
        for &variant in all {
            let written = serde_json::to_value(variant).expect("serialize");
            assert_eq!(written, json!(as_str(variant)), "{variant:?}");
            let read: T = serde_json::from_value(written).expect("deserialize");
            assert_eq!(read, variant);
        }
        let unknown: T = serde_json::from_value(json!("from_a_newer_build")).expect("unknown");
        assert_eq!(unknown, fallback);
        let empty: T = serde_json::from_value(json!("")).expect("empty");
        assert_eq!(empty, fallback);
        for not_a_string in [json!(3), json!(null), json!(true), json!({}), json!(["x"])] {
            assert!(
                serde_json::from_value::<T>(not_a_string.clone()).is_err(),
                "{not_a_string} must not parse as {fallback:?}"
            );
        }
        // Through a borrowed &str too (from_str borrows where it can).
        let text = format!("\"{}\"", as_str(all[0]));
        assert_eq!(serde_json::from_str::<T>(&text).expect("from_str"), all[0]);
    }

    #[test]
    fn every_lenient_enum_reads_unknown_names_as_its_fallback() {
        check_lenient(StopReason::ALL, StopReason::Other, StopReason::as_str);
        check_lenient(ToolKind::ALL, ToolKind::Other, ToolKind::as_str);
        check_lenient(ToolStatus::ALL, ToolStatus::Pending, ToolStatus::as_str);
        check_lenient(
            PermissionOptionKind::ALL,
            PermissionOptionKind::Other,
            PermissionOptionKind::as_str,
        );
        check_lenient(ResolvedBy::ALL, ResolvedBy::Other, ResolvedBy::as_str);
        check_lenient(PlanStatus::ALL, PlanStatus::Pending, PlanStatus::as_str);
        check_lenient(
            PlanPriority::ALL,
            PlanPriority::Medium,
            PlanPriority::as_str,
        );
        check_lenient(NoticeLevel::ALL, NoticeLevel::Info, NoticeLevel::as_str);
        check_lenient(ErrorKind::ALL, ErrorKind::Other, ErrorKind::as_str);
        check_lenient(UsageScope::ALL, UsageScope::Session, UsageScope::as_str);
        check_lenient(
            session::ConfigCategory::ALL,
            session::ConfigCategory::Other,
            session::ConfigCategory::as_str,
        );
        check_lenient(
            tasks::TaskKind::ALL,
            tasks::TaskKind::Other,
            tasks::TaskKind::as_str,
        );
        check_lenient(
            tasks::TaskStatus::ALL,
            tasks::TaskStatus::Stopped,
            tasks::TaskStatus::as_str,
        );
        check_lenient(
            tasks::TaskStepKind::ALL,
            tasks::TaskStepKind::Text,
            tasks::TaskStepKind::as_str,
        );
        check_lenient(
            remote::RemoteDeviceKind::ALL,
            remote::RemoteDeviceKind::Browser,
            remote::RemoteDeviceKind::as_str,
        );
    }

    #[test]
    fn fallbacks_are_the_documented_ones() {
        assert_eq!(StopReason::FALLBACK, StopReason::Other);
        assert_eq!(ToolKind::FALLBACK, ToolKind::Other);
        assert_eq!(ToolStatus::FALLBACK, ToolStatus::Pending);
        assert_eq!(PermissionOptionKind::FALLBACK, PermissionOptionKind::Other);
        assert_eq!(ResolvedBy::FALLBACK, ResolvedBy::Other);
        assert_eq!(PlanStatus::FALLBACK, PlanStatus::Pending);
        assert_eq!(PlanPriority::FALLBACK, PlanPriority::Medium);
        assert_eq!(NoticeLevel::FALLBACK, NoticeLevel::Info);
        assert_eq!(ErrorKind::FALLBACK, ErrorKind::Other);
        assert_eq!(UsageScope::FALLBACK, UsageScope::Session);
        assert_eq!(
            session::ConfigCategory::FALLBACK,
            session::ConfigCategory::Other
        );
        // A device of a kind this build does not know is offered what a browser is.
        assert_eq!(
            remote::RemoteDeviceKind::FALLBACK,
            remote::RemoteDeviceKind::Browser
        );
    }

    #[test]
    fn data_carrying_enums_round_trip() {
        for answer in [
            UiAnswer::Confirm { yes: false },
            UiAnswer::Select { index: 2 },
            UiAnswer::Input { text: "x".into() },
            UiAnswer::Questions {
                answers: vec![UiQuestionAnswer {
                    question_id: "q0".into(),
                    selected: vec![0, 2],
                    text: Some("Keep the existing behavior too".into()),
                }],
            },
        ] {
            let value = serde_json::to_value(&answer).expect("serialize");
            assert!(value["type"].is_string());
            assert_eq!(
                serde_json::from_value::<UiAnswer>(value).expect("deserialize"),
                answer
            );
        }
        for part in [
            PromptPart::Text { text: "hi".into() },
            PromptPart::Image {
                blob: "ab".repeat(32),
                mime: "image/png".into(),
            },
            PromptPart::Mention {
                path: "src/main.rs".into(),
            },
        ] {
            let value = serde_json::to_value(&part).expect("serialize");
            assert_eq!(
                serde_json::from_value::<PromptPart>(value).expect("deserialize"),
                part
            );
        }
        for auto in [
            AutoAnswer::Never,
            AutoAnswer::All,
            AutoAnswer::Kinds {
                kinds: vec![ToolKind::Execute],
            },
        ] {
            let state = AutonomyState {
                requested: Autonomy::FullAuto,
                native_mode: Some("dont_ask".into()),
                auto_answer: auto,
                note: None,
            };
            let value = serde_json::to_value(&state).expect("serialize");
            assert_eq!(
                serde_json::from_value::<AutonomyState>(value).expect("deserialize"),
                state
            );
        }
        assert_eq!(
            serde_json::to_value(AutoAnswer::Kinds {
                kinds: vec![ToolKind::Read]
            })
            .expect("serialize"),
            json!({"type": "kinds", "kinds": ["read"]})
        );
        assert_eq!(
            serde_json::to_value(DriverKind::Acp).expect("serialize"),
            json!("acp")
        );
        assert_eq!(
            serde_json::to_value(DriverKind::Claude).expect("serialize"),
            json!("claude")
        );
        assert_eq!(
            serde_json::from_value::<DriverKind>(json!("claude")).expect("deserialize"),
            DriverKind::Claude
        );
    }

    #[test]
    fn grouped_questions_keep_identity_and_selection_rules_on_the_wire() {
        let prompt = UiPrompt::Questions {
            message: "Choose how to proceed".into(),
            questions: vec![
                UiQuestion {
                    id: "q0".into(),
                    header: "Format".into(),
                    question: "Which format should I use?".into(),
                    options: vec![UiQuestionOption {
                        label: "Summary".into(),
                        description: "A brief overview".into(),
                    }],
                    multi_select: false,
                    allow_free_text: true,
                },
                UiQuestion {
                    id: "q1".into(),
                    header: "Sections".into(),
                    question: "Which sections should I include?".into(),
                    options: vec![UiQuestionOption {
                        label: "Tests".into(),
                        description: "Show the verification results".into(),
                    }],
                    multi_select: true,
                    allow_free_text: false,
                },
            ],
        };
        let value = serde_json::to_value(&prompt).expect("serialize");
        assert_eq!(value["type"], "questions");
        assert_eq!(value["questions"][0]["id"], "q0");
        assert_eq!(value["questions"][1]["id"], "q1");
        assert_eq!(
            value["questions"][0]["question"],
            "Which format should I use?"
        );
        assert_eq!(value["questions"][0]["multi_select"], false);
        assert_eq!(value["questions"][1]["multi_select"], true);
        assert_eq!(value["questions"][1]["allow_free_text"], false);
        assert_eq!(
            serde_json::from_value::<UiPrompt>(value).expect("deserialize"),
            prompt
        );
    }

    #[test]
    fn autonomy_defaults_to_full_auto() {
        assert_eq!(Autonomy::default(), Autonomy::FullAuto);
        assert_eq!(
            serde_json::to_value(Autonomy::AutoEdit).expect("serialize"),
            json!("auto_edit")
        );
        assert_eq!(SessionInfo::default().status, SessionStatus::Stopped);
        assert_eq!(SessionInfo::default().autonomy, Autonomy::FullAuto);
    }

    #[test]
    fn batch_encoded_len_matches_serialization() {
        let batch = EventBatch {
            chat_id: "c".into(),
            seq_start: 7,
            events: vec![GhostEvent::MessageDelta {
                message_id: "m".into(),
                text: "x".repeat(100),
            }],
        };
        assert_eq!(
            batch.encoded_len(),
            serde_json::to_vec(&batch).expect("serialize").len()
        );
        assert!(batch.encoded_len() < MAX_FRAME_BYTES);
        assert_eq!(
            serde_json::to_value(&batch).expect("serialize")["chat_id"],
            "c"
        );
    }

    #[test]
    fn the_event_budget_leaves_room_for_a_frame() {
        assert_eq!(EVENT_BUDGET, 7 * 1024);
        const { assert!(EVENT_BUDGET < MAX_FRAME_BYTES) };
    }
}

pub mod worktree_setup;
