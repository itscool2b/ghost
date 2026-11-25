//! A chat as plain text, for a bot that reads it through a tool. A summary, not a record: the
//! person's words, the steps the agent took by title, what it said, and what it is waiting on.

use std::collections::HashMap;

use o7_protocol::{
    O7Event, PermissionRequest, SessionStatus, StopReason, ToolCall, ToolStatus, UiPrompt,
    UiRequest,
    commands::{CommandRun, first_error, hide_secret_flags},
    session::{ChatSummary, HistoryItem},
    workspace::WorkspaceChat,
};

/// The most of a prompt, of an agent's words and of a turn's steps one reading carries. A bot
/// pays for every character it reads, on every later step of its own turn, so the tools a
/// current relay lists read less unless more is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub prompt: usize,
    pub answer: usize,
    pub steps: usize,
    /// How many of a turn's commands are listed with how each ended, newest last. None: the
    /// commands stay among the steps, by title.
    pub commands: usize,
    /// The short tool names (`answer`, `wait`) in what a reading says to do next.
    pub lean: bool,
}

impl Limits {
    /// What the first tools read (`read_chat`, `wait_for_chat`).
    pub const FIRST: Self = Self {
        prompt: 2_000,
        answer: 6_000,
        steps: 12,
        commands: 0,
        lean: false,
    };
    /// A reading for a bot that follows a chat: the end of the answer, the last few steps.
    pub const LEAN: Self = Self {
        prompt: 400,
        answer: 3_000,
        steps: 5,
        commands: 6,
        lean: true,
    };
    /// The same reading when the bot asks for all of it.
    pub const WIDE: Self = Self {
        prompt: 2_000,
        answer: 12_000,
        steps: 20,
        commands: 12,
        lean: true,
    };
}
/// How o7 begins the project memory it attaches after a prompt
/// (`crates/o7-core/src/chatstore/memory.rs`). It is reference context, not the person's words.
const MEMORY: &str = "\n\nProject memory (Aura reference context, revision ";

#[derive(Debug, Default, PartialEq)]
pub struct Turn {
    pub prompt: String,
    /// Messages the person sent while the turn ran.
    pub steers: Vec<String>,
    /// The agent's messages, in order.
    pub said: Vec<String>,
    /// Each step's title and how it ended.
    pub steps: Vec<(String, ToolStatus)>,
    /// Each step's tool call as its updates left it, in the order of `steps`: what a command
    /// was, how it ended and what it printed are read from here.
    pub calls: Vec<ToolCall>,
    pub errors: Vec<String>,
    /// `None` while the turn runs.
    pub ended: Option<StopReason>,
}

/// A request the agent is waiting on.
#[derive(Debug, PartialEq)]
pub enum Waiting {
    Permission(PermissionRequest),
    Question(UiRequest),
}

#[derive(Debug, Default, PartialEq)]
pub struct Digest {
    pub turns: Vec<Turn>,
    pub waiting: Vec<Waiting>,
}

/// Folds one page of history, oldest first, into its turns and open requests.
pub fn digest(items: &[HistoryItem]) -> Digest {
    let mut turns: Vec<Turn> = Vec::new();
    let mut open = false;
    let mut users: HashMap<&str, (usize, Option<usize>)> = HashMap::new();
    let mut messages: HashMap<&str, (usize, usize)> = HashMap::new();
    let mut tools: HashMap<&str, (usize, usize)> = HashMap::new();
    let mut waiting: Vec<Waiting> = Vec::new();
    // Events before the page's first turn start belong to a turn cut by the page.
    fn current(turns: &mut Vec<Turn>, open: &mut bool) -> usize {
        if !*open {
            turns.push(Turn::default());
            *open = true;
        }
        turns.len() - 1
    }
    for item in items {
        match &item.event {
            O7Event::TurnStarted { .. } => {
                turns.push(Turn::default());
                open = true;
            }
            O7Event::TurnEnded { stop_reason, .. } => {
                let at = current(&mut turns, &mut open);
                turns[at].ended = Some(*stop_reason);
                open = false;
            }
            O7Event::UserMessageDelta { message_id, text } => {
                let at = current(&mut turns, &mut open);
                let place = *users.entry(message_id).or_insert_with(|| {
                    if turns[at].prompt.is_empty() && turns[at].steers.is_empty() {
                        (at, None)
                    } else {
                        turns[at].steers.push(String::new());
                        (at, Some(turns[at].steers.len() - 1))
                    }
                });
                match place {
                    (turn, None) => turns[turn].prompt.push_str(text),
                    (turn, Some(steer)) => turns[turn].steers[steer].push_str(text),
                }
            }
            O7Event::MessageDelta { message_id, text } => {
                let at = current(&mut turns, &mut open);
                let (turn, message) = *messages.entry(message_id).or_insert_with(|| {
                    turns[at].said.push(String::new());
                    (at, turns[at].said.len() - 1)
                });
                turns[turn].said[message].push_str(text);
            }
            O7Event::ToolCall(call) => {
                let at = current(&mut turns, &mut open);
                let title = if call.title.trim().is_empty() {
                    call.tool_name.clone().unwrap_or_else(|| "Tool".into())
                } else {
                    call.title.clone()
                };
                turns[at].steps.push((title, call.status));
                turns[at].calls.push(call.clone());
                tools.insert(&call.tool_call_id, (at, turns[at].steps.len() - 1));
            }
            O7Event::ToolUpdate(update) => {
                if let Some(&(turn, step)) = tools.get(update.tool_call_id.as_str()) {
                    let entry = &mut turns[turn].steps[step];
                    if let Some(title) = update.title.as_ref().filter(|t| !t.trim().is_empty()) {
                        entry.0.clone_from(title);
                    }
                    if let Some(status) = update.status {
                        entry.1 = status;
                    }
                    let call = &mut turns[turn].calls[step];
                    if let Some(status) = update.status {
                        call.status = status;
                    }
                    if let Some(kind) = update.kind {
                        call.kind = kind;
                    }
                    if let Some(input) = &update.raw_input {
                        call.raw_input = Some(input.clone());
                    }
                    if let Some(content) = &update.content {
                        call.content.clone_from(content);
                    }
                    if let Some(locations) = &update.locations {
                        call.locations.clone_from(locations);
                    }
                }
            }
            O7Event::PermissionRequested(request) => {
                waiting.push(Waiting::Permission(request.clone()));
            }
            O7Event::UiRequest(request) => {
                // A notice asks nothing.
                if !matches!(request.prompt, UiPrompt::Notify { .. }) {
                    waiting.push(Waiting::Question(request.clone()));
                }
            }
            O7Event::PermissionResolved { request_id, .. }
            | O7Event::UiResolved { request_id, .. } => {
                waiting.retain(|request| request.id() != request_id);
            }
            O7Event::Error { message, .. } => {
                let at = current(&mut turns, &mut open);
                turns[at].errors.push(message.clone());
            }
            _ => {}
        }
    }
    Digest { turns, waiting }
}

impl Waiting {
    pub fn id(&self) -> &str {
        match self {
            Self::Permission(request) => &request.request_id,
            Self::Question(request) => &request.request_id,
        }
    }
}

/// A chat's status in the words the bot tools use.
pub fn status(chat: &ChatSummary) -> String {
    let status = match chat.status {
        SessionStatus::Running => "working",
        SessionStatus::Ready => "idle",
        SessionStatus::Stopped => "stopped (a message starts it again)",
        SessionStatus::Starting => "starting",
        SessionStatus::Cancelling | SessionStatus::Stopping => "stopping",
        SessionStatus::Failed => "failed",
        SessionStatus::AuthRequired => "needs its owner to sign in on the computer",
    };
    if chat.needs_attention {
        format!("{status}, waiting for an answer")
    } else {
        status.to_owned()
    }
}

/// Whether a chat is between turns with nothing to answer: waiting for it would wait for nothing.
pub fn settled(chat: &ChatSummary) -> bool {
    chat.needs_attention
        || !matches!(
            chat.status,
            SessionStatus::Running | SessionStatus::Starting | SessionStatus::Cancelling
        )
}

pub fn title(chat: &ChatSummary) -> String {
    one_line(chat.title.as_deref().unwrap_or("Untitled chat"), 120)
}

/// The last `turns` turns of a chat and what it waits on.
pub fn chat(chat: &ChatSummary, items: &[HistoryItem], turns: usize) -> String {
    chat_with(chat, items, turns, &Limits::FIRST)
}

/// A turn in progress in two lines: how far it is, not what it has said so far. A bot that waits
/// reads this each time it asks, so it is short; the whole turn is read once, when it ends.
pub fn progress(chat: &ChatSummary, items: &[HistoryItem]) -> String {
    let digest = digest(items);
    let mut out = format!("Chat {}: {}.", chat.id, status(chat));
    if let Some(turn) = digest.turns.last() {
        let done = turn.steps.len();
        if done > 0 {
            let latest: Vec<String> = turn.steps[done.saturating_sub(3)..]
                .iter()
                .map(|(title, _)| one_line(title, 80))
                .collect();
            out.push_str(&format!(
                " {done} step{} so far; latest: {}.",
                if done == 1 { "" } else { "s" },
                latest.join("; ")
            ));
        }
    }
    out
}

/// The request a chat waits on, by id.
pub fn request(items: &[HistoryItem], request_id: &str) -> Option<Waiting> {
    digest(items)
        .waiting
        .into_iter()
        .find(|request| request.id() == request_id)
}

/// One changed file as the lines that differ, with two lines around them: what is the same at
/// the file's start and end is left out, so a small edit to a large file reads small.
pub fn change(before: Option<&str>, after: Option<&str>, chars: usize) -> String {
    const AROUND: usize = 2;
    let old: Vec<&str> = before
        .map(|text| text.lines().collect())
        .unwrap_or_default();
    let new: Vec<&str> = after.map(|text| text.lines().collect()).unwrap_or_default();
    let same_start = old
        .iter()
        .zip(&new)
        .take_while(|(old, new)| old == new)
        .count();
    let same_end = old[same_start..]
        .iter()
        .rev()
        .zip(new[same_start..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    if same_start == old.len() && same_start == new.len() {
        return "The file reads the same before and after.\n".into();
    }
    let from = same_start.saturating_sub(AROUND);
    let mut out = match (before, after) {
        (None, _) => "New file:\n".to_owned(),
        (_, None) => "Deleted file:\n".to_owned(),
        _ => format!("From line {}:\n", from + 1),
    };
    for line in &old[from..same_start] {
        out.push_str(&format!("  {line}\n"));
    }
    for line in &old[same_start..old.len() - same_end] {
        out.push_str(&format!("- {line}\n"));
    }
    for line in &new[same_start..new.len() - same_end] {
        out.push_str(&format!("+ {line}\n"));
    }
    let after_end = new.len() - same_end;
    for line in &new[after_end..(after_end + AROUND).min(new.len())] {
        out.push_str(&format!("  {line}\n"));
    }
    head(&out, chars)
}

/// The last `turns` turns of a chat and what it waits on, within `limits`.
pub fn chat_with(
    chat: &ChatSummary,
    items: &[HistoryItem],
    turns: usize,
    limits: &Limits,
) -> String {
    let digest = digest(items);
    let mut out = format!(
        "Chat {} \"{}\" with {}: {}.\n",
        chat.id,
        title(chat),
        chat.agent_name,
        status(chat)
    );
    let total = digest.turns.len();
    if total == 0 {
        out.push_str("\nNo turns yet.\n");
    }
    for turn in digest.turns.iter().skip(total.saturating_sub(turns.max(1))) {
        out.push('\n');
        if !turn.prompt.is_empty() {
            out.push_str(&format!(
                "User: {}\n",
                head(person(&turn.prompt), limits.prompt)
            ));
        }
        for steer in &turn.steers {
            out.push_str(&format!(
                "User, during the turn: {}\n",
                head(steer, limits.prompt)
            ));
        }
        // The commands are told apart from the other steps where the reading has room for how
        // each ended: "did the tests pass" is then read, not inferred from a title.
        let runs: Vec<Option<CommandRun>> = if limits.commands == 0 {
            turn.steps.iter().map(|_| None).collect()
        } else {
            turn.calls.iter().map(CommandRun::of).collect()
        };
        let others: Vec<&(String, ToolStatus)> = turn
            .steps
            .iter()
            .zip(&runs)
            .filter(|(_, run)| run.is_none())
            .map(|(step, _)| step)
            .collect();
        if !others.is_empty() {
            let shown = others.len().min(limits.steps);
            let earlier = others.len() - shown;
            let steps: Vec<String> = others[earlier..]
                .iter()
                .map(|(title, status)| match status {
                    ToolStatus::Failed => format!("{} (failed)", one_line(title, 120)),
                    ToolStatus::Completed => one_line(title, 120),
                    _ => format!("{} (running)", one_line(title, 120)),
                })
                .collect();
            out.push_str(&format!(
                "Steps: {}{}\n",
                if earlier > 0 {
                    format!("{earlier} earlier, then ")
                } else {
                    String::new()
                },
                steps.join("; ")
            ));
        }
        out.push_str(&ran(
            &runs.into_iter().flatten().collect::<Vec<_>>(),
            limits.commands,
        ));
        let said = turn.said.join("\n\n");
        if !said.trim().is_empty() {
            out.push_str(&format!("Agent: {}\n", tail(said.trim(), limits.answer)));
        }
        for error in &turn.errors {
            out.push_str(&format!("Error: {}\n", head(error, 500)));
        }
        out.push_str(match turn.ended {
            None => "The turn is still running.\n",
            Some(StopReason::EndTurn) => "The turn finished.\n",
            Some(StopReason::Cancelled | StopReason::Interrupted) => "The turn was stopped.\n",
            Some(StopReason::Refusal) => "The agent refused.\n",
            Some(StopReason::MaxTokens | StopReason::MaxTurnRequests) => {
                "The turn ended at a limit.\n"
            }
            Some(StopReason::Error) => "The turn ended with an error.\n",
            Some(StopReason::Other) => "The turn ended.\n",
        });
    }
    for request in &digest.waiting {
        out.push('\n');
        out.push_str(&waiting(request, limits.lean));
    }
    out
}

/// What a chat waits on, as the bare facts of each request: whether it is a permission or a
/// question, its id, and what it asks as the agent wrote it. For a message that must set the
/// agent's words apart itself (the call-back).
pub fn asked(items: &[HistoryItem]) -> Vec<(&'static str, String, String)> {
    digest(items)
        .waiting
        .iter()
        .map(|request| match request {
            Waiting::Permission(request) => (
                "permission",
                request.request_id.clone(),
                request.title.clone(),
            ),
            Waiting::Question(request) => (
                if matches!(request.prompt, UiPrompt::Confirm { .. }) {
                    "confirmation"
                } else {
                    "question"
                },
                request.request_id.clone(),
                question(&request.prompt),
            ),
        })
        .collect()
}

/// What a question asks, as the agent wrote it: its message, or a grouped prompt's first
/// question.
pub fn question(prompt: &UiPrompt) -> String {
    match prompt {
        UiPrompt::Confirm { message }
        | UiPrompt::Input { message, .. }
        | UiPrompt::Notify { message }
        | UiPrompt::Form { message, .. }
        | UiPrompt::Link { message, .. }
        | UiPrompt::Select { message, .. } => message.clone(),
        UiPrompt::Questions { message, questions } => questions
            .first()
            .map_or_else(|| message.clone(), |first| first.question.clone()),
    }
}

/// How long a finished chat is news: an inbox lists the chats that finished in the last day.
pub const RECENT_MS: i64 = 24 * 60 * 60 * 1000;

/// The open chats as an inbox sorts them. The bot's `inbox` and the iPhone app's read the same.
pub struct Sorted<'a> {
    /// Waiting for an answer.
    pub asking: Vec<&'a WorkspaceChat>,
    pub working: Vec<&'a WorkspaceChat>,
    /// Neither, and changed in the last day ([`RECENT_MS`]), newest first.
    pub finished: Vec<&'a WorkspaceChat>,
}

pub fn sorted(chats: &[WorkspaceChat], now: i64) -> Sorted<'_> {
    let open: Vec<&WorkspaceChat> = chats.iter().filter(|entry| !entry.archived).collect();
    let busy = |chat: &ChatSummary| {
        matches!(
            chat.status,
            SessionStatus::Running | SessionStatus::Starting | SessionStatus::Cancelling
        )
    };
    let asking = open
        .iter()
        .copied()
        .filter(|entry| entry.chat.needs_attention)
        .collect();
    let working = open
        .iter()
        .copied()
        .filter(|entry| !entry.chat.needs_attention && busy(&entry.chat))
        .collect();
    let mut finished: Vec<&WorkspaceChat> = open
        .iter()
        .copied()
        .filter(|entry| {
            !entry.chat.needs_attention
                && !busy(&entry.chat)
                && now.saturating_sub(entry.chat.updated_at) <= RECENT_MS
        })
        .collect();
    finished.sort_by_key(|entry| std::cmp::Reverse(entry.chat.updated_at));
    Sorted {
        asking,
        working,
        finished,
    }
}

/// When a page's last turn started and, once it ended, when and after how long. A page that
/// begins inside a turn has that turn start at its first event, the earliest known.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LastTurn {
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub duration_ms: Option<u64>,
}

pub fn last_turn(items: &[HistoryItem]) -> LastTurn {
    let mut last = LastTurn::default();
    for item in items {
        match &item.event {
            O7Event::TurnStarted { .. } => {
                last = LastTurn {
                    started_at: Some(item.at),
                    ..LastTurn::default()
                };
            }
            O7Event::TurnEnded { duration_ms, .. } => {
                last.started_at.get_or_insert(item.at);
                last.ended_at = Some(item.at);
                last.duration_ms = *duration_ms;
            }
            _ => {
                last.started_at.get_or_insert(item.at);
            }
        }
    }
    last
}

/// What a page's last turn changed with its edits, for an inbox: the files, as the window's card
/// under the answer lists them (the paths of each edit, deletion or move that did not fail), and
/// the lines those edits added and removed, as each edit's own +/− counts them. The lines are
/// unknown (`None`) when the turn made no diff to count, or one too large to count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Edits {
    pub files: usize,
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

/// The longest stretch of a diff, on either side, compared line by line, as the window's own
/// diff compares it (`apps/desktop/src/lib/diff.ts`).
const MAX_DIFF_LINES: usize = 2_000;
/// How many line comparisons one page's edits may cost in all.
const DIFF_BUDGET: usize = 4_000_000;

pub fn last_turn_edits(items: &[HistoryItem]) -> Edits {
    let digest = digest(items);
    let Some(turn) = digest.turns.last() else {
        return Edits::default();
    };
    let mut paths: Vec<&str> = Vec::new();
    let (mut added, mut removed) = (0_u64, 0_u64);
    let (mut counted, mut whole) = (false, true);
    let mut budget = DIFF_BUDGET;
    for call in &turn.calls {
        if call.status == ToolStatus::Failed
            || !matches!(
                call.kind,
                o7_protocol::ToolKind::Edit
                    | o7_protocol::ToolKind::Delete
                    | o7_protocol::ToolKind::Move
            )
        {
            continue;
        }
        for location in &call.locations {
            if !location.path.is_empty() && !paths.contains(&location.path.as_str()) {
                paths.push(&location.path);
            }
        }
        for content in &call.content {
            let o7_protocol::ToolContent::Diff {
                old_text,
                new_text,
                truncated,
                ..
            } = content
            else {
                continue;
            };
            // A diff too large for the event keeps empty texts until it is loaded: nothing to
            // count, as in the window.
            if truncated.is_some()
                && new_text.is_empty()
                && old_text.as_deref().is_none_or(str::is_empty)
            {
                continue;
            }
            match line_delta(old_text.as_deref(), new_text, &mut budget) {
                Some((plus, minus)) => {
                    added += plus;
                    removed += minus;
                    counted = true;
                }
                None => whole = false,
            }
        }
    }
    let known = counted && whole;
    Edits {
        files: paths.len(),
        added: known.then_some(added),
        removed: known.then_some(removed),
    }
}

/// Lines of `text`, without the empty one a final newline would add.
fn lines_of(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

/// The lines `new` adds to `old` and removes from it, as the window's diff counts them: the
/// lines both start and end with are set aside, and the rest compared line by line. `None` when
/// that rest is longer on either side than the window compares, or costs more than `budget`
/// still allows.
pub fn line_delta(old: Option<&str>, new: &str, budget: &mut usize) -> Option<(u64, u64)> {
    let before = lines_of(old.unwrap_or(""));
    let after = lines_of(new);
    let head = before
        .iter()
        .zip(&after)
        .take_while(|(old, new)| old == new)
        .count();
    let tail = before[head..]
        .iter()
        .rev()
        .zip(after[head..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let old = &before[head..before.len() - tail];
    let new = &after[head..after.len() - tail];
    let count = |lines: usize| u64::try_from(lines).unwrap_or(u64::MAX);
    // Lines only added or only removed are counted without comparing anything.
    if old.is_empty() || new.is_empty() {
        return Some((count(new.len()), count(old.len())));
    }
    let cells = old.len().saturating_mul(new.len());
    if old.len() > MAX_DIFF_LINES || new.len() > MAX_DIFF_LINES || cells > *budget {
        return None;
    }
    *budget -= cells;
    // The longest run of lines both keep in order, one row of the table at a time.
    let mut row = vec![0_usize; new.len() + 1];
    for line in old {
        let mut diagonal = 0;
        for (at, other) in new.iter().enumerate() {
            let above = row[at + 1];
            row[at + 1] = if line == other {
                diagonal + 1
            } else {
                above.max(row[at])
            };
            diagonal = above;
        }
    }
    let kept = row[new.len()];
    Some((count(new.len() - kept), count(old.len() - kept)))
}

/// What a working chat is doing, in one line: the latest step of its turn, else how the agent's
/// last message begins, else its status.
pub fn doing(chat: &ChatSummary, items: &[HistoryItem]) -> String {
    digest(items)
        .turns
        .last()
        .filter(|turn| turn.ended.is_none())
        .and_then(|turn| {
            turn.steps
                .last()
                .map(|(title, _)| one_line(title, 160))
                .or_else(|| {
                    turn.said
                        .last()
                        .filter(|said| !said.trim().is_empty())
                        .map(|said| one_line(said, 160))
                })
        })
        .unwrap_or_else(|| status(chat))
}

/// What a chat is asking, each request as `read` shows it, for a reading of several chats.
pub fn asking(items: &[HistoryItem]) -> String {
    digest(items)
        .waiting
        .iter()
        .map(|request| {
            let text = waiting(request, true);
            text.lines()
                .map(|line| format!("  {line}\n"))
                .collect::<String>()
        })
        .collect()
}

/// What a chat's last turn came to, in one line: how it ended, whether a command still fails,
/// and how the agent's last message begins.
pub fn brief(chat: &ChatSummary, items: &[HistoryItem]) -> String {
    let parts = brief_parts(chat, items);
    let mut out = parts.ended;
    if let Some(failing) = parts.failing {
        out.push_str(&format!(", still failing: {failing}"));
    }
    if let Some(said) = parts.said {
        out.push_str(&format!(". {said}"));
    }
    out
}

/// What [`brief`] says, apart: a message that must keep the agent's own words visibly the
/// agent's (the call-back) sets them out itself.
pub struct Brief {
    /// How the last turn ended, or the chat's status when it has not.
    pub ended: String,
    /// The last command still failing, as one line.
    pub failing: Option<String>,
    /// How the agent's last message begins, or the turn's last error.
    pub said: Option<String>,
}

pub fn brief_parts(chat: &ChatSummary, items: &[HistoryItem]) -> Brief {
    let digest = digest(items);
    let Some(turn) = digest.turns.last() else {
        return Brief {
            ended: status(chat),
            failing: None,
            said: None,
        };
    };
    let ended = match turn.ended {
        None => status(chat),
        Some(StopReason::EndTurn) => "finished".into(),
        Some(StopReason::Cancelled | StopReason::Interrupted) => "stopped".into(),
        Some(StopReason::Refusal) => "refused".into(),
        Some(StopReason::MaxTokens | StopReason::MaxTurnRequests) => "ended at a limit".into(),
        Some(StopReason::Error) => "ended with an error".into(),
        Some(StopReason::Other) => "ended".into(),
    };
    let runs: Vec<CommandRun> = turn.calls.iter().filter_map(CommandRun::of).collect();
    let failing = runs.iter().enumerate().rev().find(|(index, run)| {
        run.failed()
            && !runs[index + 1..]
                .iter()
                .any(|later| later.command == run.command && later.succeeded())
    });
    Brief {
        ended,
        failing: failing.map(|(_, run)| run_line(run)),
        said: turn
            .said
            .last()
            .filter(|said| !said.trim().is_empty())
            .map(|said| one_line(said.trim(), 200))
            .or_else(|| turn.errors.last().map(|error| one_line(error, 200))),
    }
}

/// What a chat stands on, in a line or two after its reading: its branch and what is not
/// committed, and its agent's usage as last reported. Nothing where nothing is known.
/// A pull request in one line: `#12 <address>, open; checks failed: test, lint`.
pub fn pull_request(pr: &crate::backend::PullRequest) -> String {
    use crate::backend::PrChecks;
    let checks = match &pr.checks {
        PrChecks::None => "no checks".to_owned(),
        PrChecks::Pending => "checks running".to_owned(),
        PrChecks::Passed => "checks passed".to_owned(),
        PrChecks::Failed(names) => format!(
            "checks failed: {}",
            names
                .iter()
                .take(5)
                .map(|name| one_line(name, 60))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let notes = match pr.notes.len() {
        0 => String::new(),
        1 => "; 1 review or comment".to_owned(),
        n => format!("; {n} reviews and comments"),
    };
    format!(
        "#{} {}, {}; {checks}{notes}",
        pr.number,
        one_line(&pr.url, 200),
        one_line(&pr.state, 20)
    )
}

pub fn facts(facts: &crate::backend::ChatFacts) -> String {
    let mut out = String::new();
    if let Some(pull_request) = &facts.pull_request {
        out.push_str(&format!("Pull request: {}.\n", one_line(pull_request, 300)));
    }
    if let Some(branch) = &facts.branch {
        out.push_str(&format!(
            "Branch: {}{}",
            one_line(branch, 120),
            if facts.worktree {
                " (its own worktree)"
            } else {
                ""
            }
        ));
        match facts.uncommitted {
            Some(0) => out.push_str(", nothing uncommitted.\n"),
            Some(1) => out.push_str(", 1 file changed and not committed.\n"),
            Some(n) => out.push_str(&format!(", {n} files changed and not committed.\n")),
            None => out.push_str(".\n"),
        }
    }
    if !facts.usage.is_empty() {
        let windows: Vec<String> = facts
            .usage
            .iter()
            .take(3)
            .map(|(label, used)| format!("{} {:.0}%", one_line(label, 40), used.clamp(0.0, 100.0)))
            .collect();
        out.push_str(&format!(
            "Agent usage, as last reported: {}.\n",
            windows.join(", ")
        ));
    }
    out
}

/// A command as one line: how it ended, then the command with secret-looking values hidden.
fn run_line(run: &CommandRun) -> String {
    format!("{} | {}", run.outcome(), hide_secret_flags(&run.line()))
}

/// The commands a turn ran, newest last, and the last one still failing with its first error. A
/// failure is "still failing" unless the same command later ran and succeeded, as in a handoff
/// (`o7-core`'s conversation.rs).
fn ran(runs: &[CommandRun], shown: usize) -> String {
    if runs.is_empty() || shown == 0 {
        return String::new();
    }
    let earlier = runs.len().saturating_sub(shown);
    let mut out = String::from("Ran");
    if earlier > 0 {
        out.push_str(&format!(" ({earlier} earlier not listed)"));
    }
    out.push_str(
        ":
",
    );
    for run in &runs[earlier..] {
        out.push_str(&format!(
            "- {}
",
            run_line(run)
        ));
    }
    let failing = runs.iter().enumerate().rev().find(|(index, run)| {
        run.failed()
            && !runs[index + 1..]
                .iter()
                .any(|later| later.command == run.command && later.succeeded())
    });
    if let Some((_, run)) = failing {
        out.push_str(&format!(
            "Still failing: {}
",
            run_line(run)
        ));
        let error = first_error(&run.output);
        if !error.trim().is_empty() {
            out.push_str(&format!(
                "First error:
{}
",
                head(&error, 700)
            ));
        }
    }
    out
}

fn waiting(request: &Waiting, lean: bool) -> String {
    match request {
        Waiting::Permission(request) => {
            let options: Vec<String> = request
                .options
                .iter()
                .map(|option| format!("{} = {}", option.option_id, one_line(&option.label, 80)))
                .collect();
            format!(
                "Waiting request {} (permission): {}\n{}: {}\n",
                request.request_id,
                one_line(&request.title, 300),
                if lean {
                    "Options for answer"
                } else {
                    "Options for respond_to_request"
                },
                options.join("; ")
            )
        }
        Waiting::Question(request) => {
            let text = match &request.prompt {
                UiPrompt::Confirm { message }
                | UiPrompt::Input { message, .. }
                | UiPrompt::Notify { message }
                | UiPrompt::Form { message, .. }
                | UiPrompt::Link { message, .. } => message.clone(),
                UiPrompt::Select { message, options } => {
                    format!("{message} Choices: {}", options.join("; "))
                }
                UiPrompt::Questions { message, questions } => {
                    let asked: Vec<String> = questions
                        .iter()
                        .map(|question| {
                            let choices: Vec<&str> = question
                                .options
                                .iter()
                                .map(|option| option.label.as_str())
                                .collect();
                            if choices.is_empty() {
                                question.question.clone()
                            } else {
                                format!("{} Choices: {}", question.question, choices.join("; "))
                            }
                        })
                        .collect();
                    format!("{message} {}", asked.join(" "))
                }
            };
            format!(
                "Waiting request {} (question): {}\n{}\n",
                request.request_id,
                head(text.trim(), 1_500),
                if lean {
                    "Answer it with answer and text."
                } else {
                    "To answer, dismiss it with respond_to_request (no option_id), then send the answer with send_message."
                }
            )
        }
    }
}

/// A prompt without the project memory o7 attached after it.
fn person(prompt: &str) -> &str {
    match prompt.find(MEMORY) {
        Some(at) if prompt[at..].contains("\nUse this as project background.") => &prompt[..at],
        _ => prompt,
    }
}

fn one_line(text: &str, chars: usize) -> String {
    head(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        chars,
    )
}

/// The start of a long text, marked where it was cut.
pub fn cut(text: &str, chars: usize) -> String {
    head(text, chars)
}

fn head(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// The end of a long text: an agent's answer finishes with what matters.
fn tail(text: &str, chars: usize) -> String {
    let count = text.chars().count();
    if count <= chars {
        return text.to_owned();
    }
    let at = text
        .char_indices()
        .nth(count - chars)
        .map_or(0, |(at, _)| at);
    format!("…{}", &text[at..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use o7_protocol::{
        PermissionDecision, PermissionOption, PermissionOptionKind, ResolvedBy, ToolCall, ToolKind,
        ToolUpdate,
    };

    fn items(events: Vec<O7Event>) -> Vec<HistoryItem> {
        events
            .into_iter()
            .enumerate()
            .map(|(index, event)| HistoryItem {
                seq: index as u32,
                turn_id: None,
                at: 0,
                event,
            })
            .collect()
    }

    fn user(id: &str, text: &str) -> O7Event {
        O7Event::UserMessageDelta {
            message_id: id.into(),
            text: text.into(),
        }
    }

    fn said(id: &str, text: &str) -> O7Event {
        O7Event::MessageDelta {
            message_id: id.into(),
            text: text.into(),
        }
    }

    fn tool(id: &str, title: &str, status: ToolStatus) -> O7Event {
        O7Event::ToolCall(ToolCall {
            tool_call_id: id.into(),
            kind: ToolKind::Execute,
            title: title.into(),
            status,
            locations: vec![],
            raw_input: None,
            tool_name: Some("Bash".into()),
            content: vec![],
            truncated: None,
        })
    }

    /// A step that is not a command: a file read.
    fn looked(id: &str, title: &str) -> O7Event {
        let O7Event::ToolCall(mut call) = tool(id, title, ToolStatus::Completed) else {
            unreachable!()
        };
        call.kind = ToolKind::Read;
        call.tool_name = Some("Read".into());
        O7Event::ToolCall(call)
    }

    /// A command with what it printed and how it ended.
    fn ran_command(id: &str, command: &str, exit: i32, output: &str) -> O7Event {
        let O7Event::ToolCall(mut call) = tool(
            id,
            command,
            if exit == 0 {
                ToolStatus::Completed
            } else {
                ToolStatus::Failed
            },
        ) else {
            unreachable!()
        };
        call.raw_input = Some(serde_json::json!({ "command": command }));
        call.content = vec![o7_protocol::ToolContent::Terminal {
            output: output.into(),
            exit_code: Some(exit),
            truncated: None,
        }];
        O7Event::ToolCall(call)
    }

    fn started(id: &str) -> O7Event {
        O7Event::TurnStarted {
            turn_id: id.into(),
            prompt_id: None,
        }
    }

    fn ended(id: &str, stop_reason: StopReason) -> O7Event {
        O7Event::TurnEnded {
            turn_id: id.into(),
            stop_reason,
            usage: None,
            duration_ms: None,
            recovered: false,
        }
    }

    fn permission(id: &str) -> O7Event {
        O7Event::PermissionRequested(PermissionRequest {
            request_id: id.into(),
            tool_call_id: None,
            title: "Run `rm -rf build`".into(),
            kind: Some(ToolKind::Execute),
            raw_input: None,
            content: vec![],
            locations: vec![],
            options: vec![
                PermissionOption {
                    option_id: "allow".into(),
                    kind: PermissionOptionKind::AllowOnce,
                    label: "Allow once".into(),
                },
                PermissionOption {
                    option_id: "reject".into(),
                    kind: PermissionOptionKind::RejectOnce,
                    label: "Reject".into(),
                },
            ],
            truncated: None,
        })
    }

    fn summary(status: SessionStatus, needs_attention: bool) -> ChatSummary {
        serde_json::from_value(serde_json::json!({
            "id": "c1",
            "title": "Fix the\nlogin form",
            "agent_id": "claude-code",
            "agent_host": { "type": "native" },
            "agent_name": "Claude Code",
            "folder": "/work/app",
            "folder_state": "ok",
            "status": status,
            "needs_attention": needs_attention,
            "created_at": 0,
            "updated_at": 0,
        }))
        .expect("summary")
    }

    #[test]
    fn folds_turns_steps_answers_and_open_requests() {
        let memory = format!(
            "{MEMORY}r1)\nUse this as project background. More.\n\n# Notes\nsecret notes\n"
        );
        let digest = digest(&items(vec![
            // The page begins inside a turn whose start is on an older page.
            said("m0", "Earlier answer"),
            ended("t0", StopReason::EndTurn),
            started("t1"),
            user("u1", "Run the "),
            user("u1", &format!("tests{memory}")),
            tool("x1", "npm test", ToolStatus::InProgress),
            said("m1", "Running "),
            O7Event::ToolUpdate(ToolUpdate {
                tool_call_id: "x1".into(),
                status: Some(ToolStatus::Failed),
                title: None,
                kind: None,
                locations: None,
                raw_input: None,
                tool_name: None,
                content: None,
                truncated: None,
            }),
            said("m1", "them."),
            user("u2", "Use pnpm"),
            said("m2", "Two failed."),
            permission("p1"),
            permission("p2"),
            O7Event::PermissionResolved {
                request_id: "p1".into(),
                decision: PermissionDecision::Cancelled,
                by: ResolvedBy::User,
            },
        ]));
        assert_eq!(digest.turns.len(), 2);
        assert_eq!(digest.turns[0].said, ["Earlier answer"]);
        assert_eq!(digest.turns[0].ended, Some(StopReason::EndTurn));
        let turn = &digest.turns[1];
        assert!(turn.prompt.starts_with("Run the tests\n\nProject memory"));
        assert_eq!(turn.steers, ["Use pnpm"]);
        assert_eq!(turn.said, ["Running them.", "Two failed."]);
        assert_eq!(turn.steps, [("npm test".to_owned(), ToolStatus::Failed)]);
        assert_eq!(turn.ended, None);
        assert_eq!(digest.waiting.len(), 1);
        assert_eq!(digest.waiting[0].id(), "p2");
    }

    #[test]
    fn reads_as_text_without_attached_memory_and_names_what_to_answer() {
        let memory = format!("{MEMORY}r1)\nUse this as project background.\n\nsecret notes\n");
        let history = items(vec![
            started("t0"),
            user("u0", "First"),
            said("m0", "Done first."),
            ended("t0", StopReason::EndTurn),
            started("t1"),
            user("u1", &format!("Clean the build{memory}")),
            tool("x1", "Read package.json", ToolStatus::Completed),
            tool("x2", "", ToolStatus::InProgress),
            said("m1", "I need to delete the build folder."),
            permission("p1"),
        ]);
        let text = chat(&summary(SessionStatus::Running, true), &history, 1);
        assert_eq!(
            text,
            "Chat c1 \"Fix the login form\" with Claude Code: working, waiting for an answer.\n\
             \n\
             User: Clean the build\n\
             Steps: Read package.json; Bash (running)\n\
             Agent: I need to delete the build folder.\n\
             The turn is still running.\n\
             \n\
             Waiting request p1 (permission): Run `rm -rf build`\n\
             Options for respond_to_request: allow = Allow once; reject = Reject\n"
        );
        assert!(!text.contains("secret notes"));
        let both = chat(&summary(SessionStatus::Ready, false), &history, 5);
        assert!(both.contains("User: First\nAgent: Done first.\nThe turn finished.\n"));
        assert!(both.starts_with("Chat c1 \"Fix the login form\" with Claude Code: idle.\n"));
        assert_eq!(
            chat(&summary(SessionStatus::Stopped, false), &[], 2),
            "Chat c1 \"Fix the login form\" with Claude Code: stopped (a message starts it again).\n\nNo turns yet.\n"
        );
    }

    #[test]
    fn long_text_is_cut_on_characters_and_keeps_an_answers_end() {
        let history = items(vec![
            started("t"),
            user("u", &"é".repeat(Limits::FIRST.prompt + 10)),
            said("m", &format!("{}the end", "ü".repeat(Limits::FIRST.answer))),
            ended("t", StopReason::Cancelled),
        ]);
        let text = chat(&summary(SessionStatus::Ready, false), &history, 1);
        let prompt = text
            .lines()
            .find(|line| line.starts_with("User: "))
            .expect("prompt");
        assert_eq!(
            prompt.chars().count(),
            "User: ".len() + Limits::FIRST.prompt + 1
        );
        let answer = text
            .lines()
            .find(|line| line.starts_with("Agent: "))
            .expect("answer");
        assert!(answer.starts_with("Agent: …ü") && answer.ends_with("the end"));
        assert_eq!(
            answer.chars().count(),
            "Agent: ".len() + Limits::FIRST.answer + 1
        );
        assert!(text.ends_with("The turn was stopped.\n"));
        let many: Vec<O7Event> = std::iter::once(started("t"))
            .chain((0..20).map(|n| {
                tool(
                    &format!("x{n}"),
                    &format!("Step {n}"),
                    ToolStatus::Completed,
                )
            }))
            .collect();
        let text = chat(&summary(SessionStatus::Running, false), &items(many), 1);
        assert!(text.contains("Steps: 8 earlier, then Step 8; Step 9;"));
        assert!(text.contains("Step 19\n"));
    }

    /// "Did the tests pass" is read from the result, not inferred from a step's title: the
    /// commands with how each ended, the one still failing and the first lines of its error.
    #[test]
    fn a_lean_reading_says_what_ran_and_what_still_fails() {
        let history = items(vec![
            started("t1"),
            user("u", "Fix the login test"),
            looked("r", "Read login.test.ts"),
            ran_command(
                "1",
                "npm test",
                1,
                "PASS a.test.ts\nFAIL login.test.ts\n  Error: Cannot find module 'x'\n    at load\n",
            ),
            ran_command("2", "npm run lint --token abc", 0, "ok"),
            ran_command(
                "3",
                "cargo build",
                101,
                "error[E0425]: cannot find value `y`\n",
            ),
            ran_command("4", "npm test", 0, "PASS login.test.ts"),
            said("m", "Fixed the import. The build still fails on y."),
            ended("t1", StopReason::EndTurn),
        ]);
        let chat = summary(SessionStatus::Ready, false);
        let lean = chat_with(&chat, &history, 1, &Limits::LEAN);
        assert!(lean.contains("Steps: Read login.test.ts\n"), "{lean}");
        assert!(
            lean.contains(
                "Ran:\n- exit 1 | npm test\n- exit 0 | npm run lint --token <SECRET>\n- exit 101 | cargo build\n- exit 0 | npm test\n"
            ),
            "{lean}"
        );
        // npm test failed and later passed: it is not what still fails.
        assert!(
            lean.contains("Still failing: exit 101 | cargo build\nFirst error:\nerror[E0425]: cannot find value `y`\n"),
            "{lean}"
        );
        assert!(!lean.contains("Still failing: exit 1 | npm test"), "{lean}");
        // One line for a reading of several chats.
        assert_eq!(
            brief(&chat, &history),
            "finished, still failing: exit 101 | cargo build. Fixed the import. The build still fails on y."
        );
        // A turn whose every command passed says nothing fails.
        let clean = items(vec![
            started("t1"),
            ran_command("1", "npm test", 0, "PASS"),
            said("m", "All green."),
            ended("t1", StopReason::EndTurn),
        ]);
        let lean = chat_with(&chat, &clean, 1, &Limits::LEAN);
        assert!(lean.contains("Ran:\n- exit 0 | npm test\n") && !lean.contains("Still failing"));
        assert_eq!(brief(&chat, &clean), "finished. All green.");
        // More commands than a reading lists: the newest, and how many are left out.
        let many: Vec<O7Event> = std::iter::once(started("t"))
            .chain((0..9).map(|n| ran_command(&format!("c{n}"), &format!("step {n}"), 0, "")))
            .collect();
        let lean = chat_with(&chat, &items(many), 1, &Limits::LEAN);
        assert!(
            lean.contains("Ran (3 earlier not listed):\n- exit 0 | step 3\n"),
            "{lean}"
        );
    }

    #[test]
    fn what_chats_ask_reads_as_in_read_for_a_reading_of_several() {
        let history = items(vec![started("t1"), permission("p1")]);
        let asks = asking(&history);
        assert!(asks.starts_with("  "), "{asks}");
        assert!(asks.contains("p1") && asks.contains("answer"), "{asks}");
        assert_eq!(asking(&items(vec![started("t1")])), "");
    }

    #[test]
    fn a_lean_reading_is_shorter_and_names_the_current_tools() {
        let history = items(vec![
            started("t1"),
            user("u", &"a".repeat(1_000)),
            looked("0", "Read z.rs"),
            looked("1", "Read a.rs"),
            looked("2", "Read b.rs"),
            looked("3", "Read c.rs"),
            looked("4", "Read d.rs"),
            looked("5", "Read e.rs"),
            tool("6", "Run tests", ToolStatus::InProgress),
            said("m", &"b".repeat(5_000)),
            permission("p1"),
        ]);
        let chat = summary(SessionStatus::Running, true);
        let first = super::chat(&chat, &history, 1);
        let lean = chat_with(&chat, &history, 1, &Limits::LEAN);
        assert!(
            lean.len() < first.len() * 3 / 4,
            "{} of {}",
            lean.len(),
            first.len()
        );
        // The steps that are not commands by title; the command with how it stands.
        assert!(lean.contains("Steps: 1 earlier, then Read a.rs;"), "{lean}");
        assert!(lean.contains("Ran:\n- running | Run tests\n"), "{lean}");
        assert!(!first.contains("Ran:"), "the first tools read as they did");
        assert!(first.contains("Run tests (running)"), "{first}");
        assert!(lean.contains("Options for answer: "), "{lean}");
        assert!(first.contains("Options for respond_to_request: "));
        // A turn in progress is a line, whatever it has said so far.
        assert_eq!(
            progress(&chat, &history),
            format!(
                "Chat {}: working, waiting for an answer. 7 steps so far; latest: Read d.rs; Read e.rs; Run tests.",
                chat.id
            )
        );
        assert!(request(&history, "p1").is_some());
        assert!(request(&history, "p2").is_none());
    }

    #[test]
    fn a_change_is_its_differing_lines_with_two_around_them() {
        let before = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
        let after = "one\ntwo\nthree\nFOUR\nfour and a half\nfive\nsix\nseven\n";
        assert_eq!(
            change(Some(before), Some(after), 1_000),
            "From line 2:\n  two\n  three\n- four\n+ FOUR\n+ four and a half\n  five\n  six\n"
        );
        assert_eq!(change(None, Some("a\nb\n"), 1_000), "New file:\n+ a\n+ b\n");
        assert_eq!(change(Some("a\n"), None, 1_000), "Deleted file:\n- a\n");
        assert_eq!(
            change(Some("a\n"), Some("a\n"), 1_000),
            "The file reads the same before and after.\n"
        );
        // Lines repeated around an insertion are not counted twice.
        assert_eq!(
            change(Some("x\nx\n"), Some("x\nx\nx\n"), 1_000),
            "From line 1:\n  x\n  x\n+ x\n"
        );
        assert!(change(None, Some(&"long line\n".repeat(500)), 100).ends_with('…'));
    }

    #[test]
    fn waiting_is_over_when_a_chat_stops_working_or_asks() {
        assert!(!settled(&summary(SessionStatus::Running, false)));
        assert!(!settled(&summary(SessionStatus::Starting, false)));
        assert!(settled(&summary(SessionStatus::Running, true)));
        for status in [
            SessionStatus::Ready,
            SessionStatus::Stopped,
            SessionStatus::Failed,
            SessionStatus::AuthRequired,
        ] {
            assert!(settled(&summary(status, false)));
        }
    }

    /// When a page's last turn began and ended, for an inbox: a page cut inside a turn starts it
    /// at its first event, and a new turn forgets the last one's end.
    #[test]
    fn a_pages_last_turn_is_timed_from_its_own_events() {
        let at = |times: &[i64], events: Vec<O7Event>| {
            let mut page = items(events);
            for (item, time) in page.iter_mut().zip(times) {
                item.at = *time;
            }
            page
        };
        assert_eq!(last_turn(&[]), LastTurn::default());
        // Cut inside a turn that is still going: its first event is the earliest known.
        let cut = at(
            &[5, 7],
            vec![
                said("m1", "Looking"),
                tool("x1", "ls", ToolStatus::InProgress),
            ],
        );
        assert_eq!(
            last_turn(&cut),
            LastTurn {
                started_at: Some(5),
                ended_at: None,
                duration_ms: None
            }
        );
        // Two turns: the second's start, and the end only once it came.
        let mut done = ended("t2", StopReason::EndTurn);
        if let O7Event::TurnEnded { duration_ms, .. } = &mut done {
            *duration_ms = Some(40);
        }
        let two = at(
            &[1, 2, 3, 10, 11, 50],
            vec![
                started("t1"),
                said("m1", "a"),
                ended("t1", StopReason::EndTurn),
                started("t2"),
                said("m2", "b"),
                done,
            ],
        );
        assert_eq!(
            last_turn(&two),
            LastTurn {
                started_at: Some(10),
                ended_at: Some(50),
                duration_ms: Some(40)
            }
        );
        assert_eq!(
            last_turn(&two[..5]).ended_at,
            None,
            "a new turn forgets the last end"
        );
    }

    /// A working chat's line is its open turn's latest step, else its agent's words, else how it
    /// stands; a turn that already ended says nothing about what the chat does now.
    #[test]
    fn what_a_working_chat_does_is_its_open_turns_latest_step() {
        let chat = summary(SessionStatus::Running, false);
        let page = items(vec![
            started("t1"),
            said("m1", "I will run\n the tests."),
            tool("x1", "Read src/login.ts", ToolStatus::Completed),
            tool("x2", "Run   cargo\ntest", ToolStatus::InProgress),
        ]);
        assert_eq!(doing(&chat, &page), "Run cargo test");
        assert_eq!(doing(&chat, &page[..2]), "I will run the tests.");
        assert_eq!(doing(&chat, &page[..1]), "working");
        let finished = items(vec![
            started("t1"),
            tool("x1", "ls", ToolStatus::Completed),
            ended("t1", StopReason::EndTurn),
        ]);
        assert_eq!(
            doing(&summary(SessionStatus::Starting, false), &finished),
            "starting"
        );
        assert_eq!(doing(&chat, &[]), "working");
    }

    fn edit(id: &str, path: &str, old: Option<&str>, new: &str, status: ToolStatus) -> O7Event {
        O7Event::ToolCall(ToolCall {
            tool_call_id: id.into(),
            kind: ToolKind::Edit,
            title: format!("Edit {path}"),
            status,
            locations: vec![o7_protocol::ToolLocation {
                path: path.into(),
                line: None,
            }],
            raw_input: None,
            tool_name: Some("Edit".into()),
            content: vec![o7_protocol::ToolContent::Diff {
                path: path.into(),
                old_text: old.map(str::to_owned),
                new_text: new.into(),
                truncated: None,
            }],
            truncated: None,
        })
    }

    /// A finished turn's edits as an inbox counts them: the files the window's card under the
    /// answer lists, and the lines as each edit's own +/− counts them. A failed edit changed
    /// nothing; an earlier turn's is not this one's.
    #[test]
    fn a_turns_edits_count_their_files_and_lines() {
        let page = items(vec![
            started("t0"),
            edit("e0", "old.rs", None, "a\n", ToolStatus::Completed),
            ended("t0", StopReason::EndTurn),
            started("t1"),
            looked("r1", "Read src/login.ts"),
            edit(
                "e1",
                "src/login.ts",
                Some("a\nb\nc\n"),
                "a\nB\nc\nd\n",
                ToolStatus::Completed,
            ),
            edit(
                "e2",
                "src/login.test.ts",
                None,
                "x\ny\n",
                ToolStatus::Completed,
            ),
            edit(
                "e3",
                "src/login.ts",
                Some("a\nB\nc\nd\n"),
                "a\nB\nc\n",
                ToolStatus::Completed,
            ),
            edit(
                "e4",
                "src/broken.ts",
                Some("q\n"),
                "r\n",
                ToolStatus::Failed,
            ),
            ended("t1", StopReason::EndTurn),
        ]);
        assert_eq!(
            last_turn_edits(&page),
            Edits {
                files: 2,
                added: Some(4),
                removed: Some(2)
            }
        );
        assert_eq!(last_turn_edits(&[]), Edits::default());

        // A path an update names counts; a diff too large to compare makes the lines unknown
        // rather than wrong.
        let big_old: String = (0..2_100).map(|n| format!("o{n}\n")).collect();
        let big_new: String = (0..2_100).map(|n| format!("n{n}\n")).collect();
        let page = items(vec![
            started("t1"),
            O7Event::ToolCall(ToolCall {
                tool_call_id: "m1".into(),
                kind: ToolKind::Move,
                title: "Move".into(),
                status: ToolStatus::InProgress,
                locations: vec![],
                raw_input: None,
                tool_name: None,
                content: vec![],
                truncated: None,
            }),
            O7Event::ToolUpdate(ToolUpdate {
                tool_call_id: "m1".into(),
                status: Some(ToolStatus::Completed),
                title: None,
                kind: None,
                locations: Some(vec![o7_protocol::ToolLocation {
                    path: "b.rs".into(),
                    line: None,
                }]),
                raw_input: None,
                tool_name: None,
                content: None,
                truncated: None,
            }),
            edit(
                "e1",
                "big.rs",
                Some(&big_old),
                &big_new,
                ToolStatus::Completed,
            ),
        ]);
        assert_eq!(
            last_turn_edits(&page),
            Edits {
                files: 2,
                added: None,
                removed: None
            }
        );

        // Lines only added or only removed are counted however many; a comparison costs its
        // lines times lines, and one past the budget is not made.
        let mut budget = 4;
        assert_eq!(
            line_delta(Some("a\nb\n"), "a\nb\n", &mut budget),
            Some((0, 0))
        );
        assert_eq!(
            line_delta(Some("x\ny\nz\n"), "x\n", &mut budget),
            Some((0, 2))
        );
        assert_eq!(line_delta(None, &big_new, &mut budget), Some((2_100, 0)));
        assert_eq!(budget, 4);
        assert_eq!(
            line_delta(Some("a\nb\n"), "c\nd\n", &mut budget),
            Some((2, 2))
        );
        assert_eq!(budget, 0);
        assert_eq!(line_delta(Some("a\n"), "b\n", &mut budget), None);
    }
}
