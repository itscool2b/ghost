//! A scripted agent: the UI demo, the streaming stress test, and later the replay harness.
//!
//! A [`MockScript`] lays [`MockStep`]s out on a virtual timeline: `Emit` takes no time, `Sleep` moves
//! the clock forward, and the stream steps hand out text one token every `1 / tokens_per_sec` seconds.
//! [`run_script`] plays that timeline against Tokio's clock and sends everything through a
//! [`FrameBatcher`], so the UI receives what a real driver produces: coalesced deltas in frames under
//! the size limit, flushed at most once per frame interval. [`play`] is the same timeline one level
//! down, event by event, for the demo agent's driver, whose session does its own batching. Because
//! pacing follows Tokio's clock, tests play whole turns instantly and deterministically with paused
//! time.

use std::time::Duration;

use ghost_protocol::{
    Cost, EventBatch, GhostEvent, PlanEntry, PlanPriority, PlanStatus, SessionInfo, StopReason,
    TokenUsage, ToolCall, ToolContent, ToolKind, ToolLocation, ToolStatus, ToolUpdate, UsageInfo,
    UsageScope,
};
use serde_json::json;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::batch::{DeltaKind, FrameBatcher};

/// Slowest stream rate. Zero would mean a stream that never finishes.
pub const MIN_TOKENS_PER_SEC: u32 = 1;

/// Fastest stream rate. Real agents peak at a few hundred tokens per second, so this is already a
/// stress case, and it bounds how much text one frame interval can produce.
pub const MAX_TOKENS_PER_SEC: u32 = 5_000;

/// Tokio timers have millisecond resolution, so a shorter frame interval cannot be honoured; it would
/// only add wakeups.
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(1);

/// About thirty years: stands in for "never" when a script sleeps absurdly long, without overflowing
/// `Instant` (which panics).
const FAR_FUTURE: Duration = Duration::from_secs(30 * 365 * 24 * 60 * 60);

/// One step of a [`MockScript`].
// `Emit` holds a whole event inline. A script is a short list built once per turn, so boxing it
// would only make scripts clumsier to write.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum MockStep {
    /// Emits one event at the current point of the timeline. Takes no time.
    Emit(GhostEvent),
    /// Moves the timeline forward without emitting anything: a tool running, a pause before answering.
    Sleep(Duration),
    /// Streams `text` as `MessageDelta`s for `message_id`, one token every `1 / tokens_per_sec` seconds
    /// (clamped to [`MIN_TOKENS_PER_SEC`]..=[`MAX_TOKENS_PER_SEC`]). A token is a word plus the
    /// whitespace after it, so the deltas concatenate back to `text` exactly. The step takes
    /// `tokens / tokens_per_sec` seconds; the first token goes out as soon as it starts.
    StreamText {
        message_id: String,
        text: String,
        tokens_per_sec: u32,
    },
    /// Like [`MockStep::StreamText`], as `ThoughtDelta`s for `thought_id`.
    StreamThought {
        thought_id: String,
        text: String,
        tokens_per_sec: u32,
    },
}

/// A scripted agent turn, played by [`run_script`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MockScript {
    pub steps: Vec<MockStep>,
}

/// How [`run_script`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// Every step played and every frame was delivered.
    Completed,
    /// The cancellation token fired and the turn was closed as cancelled.
    Cancelled,
    /// The sink reported its receiver gone; nothing more was sent.
    ReceiverGone,
}

impl MockScript {
    /// A realistic agent turn for the UI demo: think, read a file, edit it, update the plan, answer in
    /// markdown, report usage, end the turn. Every kind of transcript block shows up, with pauses where a
    /// real tool would take time. Ids derive from `turn_id`, so several demo turns can share a session.
    pub fn demo_turn(turn_id: &str, tokens_per_sec: u32) -> MockScript {
        let rate = clamp_rate(tokens_per_sec);
        let thought_id = format!("{turn_id}-thought");
        let read_id = format!("{turn_id}-read");
        let edit_id = format!("{turn_id}-edit");
        let message_id = format!("{turn_id}-answer");
        let thought_tokens = count_tokens(DEMO_THOUGHT);
        let answer_tokens = count_tokens(DEMO_ANSWER);
        let path = "src/main.rs";

        let steps = vec![
            MockStep::Emit(GhostEvent::TurnStarted {
                turn_id: turn_id.to_owned(),
                prompt_id: None,
            }),
            // Time to first token.
            MockStep::Sleep(Duration::from_millis(250)),
            MockStep::StreamThought {
                thought_id: thought_id.clone(),
                text: DEMO_THOUGHT.to_owned(),
                tokens_per_sec: rate,
            },
            MockStep::Emit(GhostEvent::ThoughtCompleted {
                thought_id,
                duration_ms: Some(stream_millis(thought_tokens, rate)),
            }),
            MockStep::Emit(GhostEvent::ToolCall(ToolCall {
                tool_call_id: read_id.clone(),
                kind: ToolKind::Read,
                title: format!("Read {path}"),
                status: ToolStatus::InProgress,
                locations: vec![ToolLocation {
                    path: path.to_owned(),
                    line: None,
                }],
                raw_input: Some(json!({ "path": path })),
                tool_name: Some("Read".to_owned()),
                content: Vec::new(),
                truncated: None,
            })),
            MockStep::Sleep(Duration::from_millis(350)),
            MockStep::Emit(GhostEvent::ToolUpdate(completed(
                read_id,
                ToolContent::Text {
                    text: DEMO_MAIN_BEFORE.to_owned(),
                    truncated: None,
                },
            ))),
            MockStep::Emit(GhostEvent::ToolCall(ToolCall {
                tool_call_id: edit_id.clone(),
                kind: ToolKind::Edit,
                title: format!("Edit {path}"),
                status: ToolStatus::InProgress,
                locations: vec![ToolLocation {
                    path: path.to_owned(),
                    line: Some(1),
                }],
                raw_input: Some(json!({
                    "path": path,
                    "old_string": DEMO_MAIN_BEFORE,
                    "new_string": DEMO_MAIN_AFTER,
                })),
                tool_name: Some("Edit".to_owned()),
                content: Vec::new(),
                truncated: None,
            })),
            MockStep::Sleep(Duration::from_millis(600)),
            MockStep::Emit(GhostEvent::ToolUpdate(completed(
                edit_id,
                ToolContent::Diff {
                    path: path.to_owned(),
                    old_text: Some(DEMO_MAIN_BEFORE.to_owned()),
                    new_text: DEMO_MAIN_AFTER.to_owned(),
                    truncated: None,
                },
            ))),
            MockStep::Emit(GhostEvent::PlanUpdated {
                entries: vec![
                    plan_entry(
                        "Read src/main.rs",
                        PlanStatus::Completed,
                        PlanPriority::High,
                    ),
                    plan_entry(
                        "Extract a greet helper",
                        PlanStatus::Completed,
                        PlanPriority::High,
                    ),
                    plan_entry(
                        "Explain the change",
                        PlanStatus::InProgress,
                        PlanPriority::Medium,
                    ),
                ],
            }),
            MockStep::Sleep(Duration::from_millis(200)),
            MockStep::StreamText {
                message_id: message_id.clone(),
                text: DEMO_ANSWER.to_owned(),
                tokens_per_sec: rate,
            },
            MockStep::Emit(GhostEvent::MessageCompleted { message_id }),
            MockStep::Emit(GhostEvent::UsageUpdated(UsageInfo {
                context_used: Some(
                    DEMO_PROMPT_TOKENS.saturating_add(to_u64(thought_tokens + answer_tokens)),
                ),
                context_size: Some(CONTEXT_SIZE),
                cost: Some(Cost {
                    amount: 0.0184,
                    currency: "USD".to_owned(),
                }),
            })),
            MockStep::Emit(GhostEvent::SessionUpdated(SessionInfo {
                title: Some("Extract a greet helper".to_owned()),
                model: Some("demo".to_owned()),
                ..SessionInfo::default()
            })),
            MockStep::Emit(GhostEvent::TurnEnded {
                turn_id: turn_id.to_owned(),
                stop_reason: StopReason::EndTurn,
                usage: Some(TokenUsage {
                    input_tokens: DEMO_PROMPT_TOKENS,
                    output_tokens: to_u64(thought_tokens + answer_tokens),
                    cache_read_tokens: Some(DEMO_CACHED_TOKENS),
                    cache_write_tokens: None,
                    reasoning_tokens: Some(to_u64(thought_tokens)),
                    scope: UsageScope::Turn,
                }),
                duration_ms: None,
                recovered: false,
            }),
        ];
        MockScript { steps }
    }

    /// A long, fast turn for the streaming stress test: a short thought, then varied markdown
    /// (headings, nested lists, a table, quotes, Unicode, and several code blocks in different
    /// languages), `total_tokens` tokens across both streams.
    pub fn stress(turn_id: &str, tokens_per_sec: u32, total_tokens: u32) -> MockScript {
        let rate = clamp_rate(tokens_per_sec);
        let total = total_tokens as usize;
        // A real long answer starts with a short thought; capped so the markdown gets nearly every token.
        let thought_tokens = (total / 20).min(200);
        let answer_tokens = total - thought_tokens;

        let mut steps = vec![MockStep::Emit(GhostEvent::TurnStarted {
            turn_id: turn_id.to_owned(),
            prompt_id: None,
        })];
        if thought_tokens > 0 {
            let thought_id = format!("{turn_id}-thought");
            steps.push(MockStep::StreamThought {
                thought_id: thought_id.clone(),
                text: cycle_tokens(STRESS_THOUGHT, thought_tokens),
                tokens_per_sec: rate,
            });
            steps.push(MockStep::Emit(GhostEvent::ThoughtCompleted {
                thought_id,
                duration_ms: Some(stream_millis(thought_tokens, rate)),
            }));
        }
        if answer_tokens > 0 {
            let message_id = format!("{turn_id}-answer");
            steps.push(MockStep::StreamText {
                message_id: message_id.clone(),
                text: stress_answer(answer_tokens),
                tokens_per_sec: rate,
            });
            steps.push(MockStep::Emit(GhostEvent::MessageCompleted { message_id }));
        }
        steps.push(MockStep::Emit(GhostEvent::UsageUpdated(UsageInfo {
            context_used: Some(STRESS_PROMPT_TOKENS.saturating_add(u64::from(total_tokens))),
            context_size: Some(CONTEXT_SIZE),
            cost: None,
        })));
        steps.push(MockStep::Emit(GhostEvent::TurnEnded {
            turn_id: turn_id.to_owned(),
            stop_reason: StopReason::EndTurn,
            usage: Some(TokenUsage {
                input_tokens: STRESS_PROMPT_TOKENS,
                output_tokens: u64::from(total_tokens),
                cache_read_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: Some(to_u64(thought_tokens)),
                scope: UsageScope::Turn,
            }),
            duration_ms: None,
            recovered: false,
        }));
        MockScript { steps }
    }

    /// The turn this script plays, which cancellation closes: the id of its first `TurnStarted` (or
    /// `TurnEnded`).
    pub fn turn_id(&self) -> Option<&str> {
        self.steps.iter().find_map(|step| match step {
            MockStep::Emit(
                GhostEvent::TurnStarted { turn_id, .. } | GhostEvent::TurnEnded { turn_id, .. },
            ) => Some(turn_id.as_str()),
            _ => None,
        })
    }

    /// Tokens across all stream steps, counted the same way they are paced.
    pub fn streamed_token_count(&self) -> usize {
        self.steps
            .iter()
            .map(|step| match step {
                MockStep::StreamText { text, .. } | MockStep::StreamThought { text, .. } => {
                    count_tokens(text)
                }
                MockStep::Emit(_) | MockStep::Sleep(_) => 0,
            })
            .sum()
    }
}

/// Plays `script` on Tokio's clock, sending `chat_id`'s frames to `sink`.
///
/// Events go through a [`FrameBatcher`] at their scripted times, and `sink` receives the flushed frames
/// at most once per `frame_interval` (clamped to at least 1 ms), plus one final flush when the script
/// ends or is cancelled. A flush happens as soon as the interval allows, so the first token after a
/// pause shows up at once. `sink` returning `false` means its receiver is gone: the run stops
/// immediately with [`RunOutcome::ReceiverGone`] and `sink` is not called again.
///
/// When `cancel` fires, the run stops at once (well within one frame interval), sends what is already
/// queued plus a `TurnEnded { stop_reason: Cancelled }` for the open turn, and returns
/// [`RunOutcome::Cancelled`]. If the script's turn had not started yet, it is reported as started and
/// cancelled so the UI never waits on it; if it had already ended, nothing is added.
///
/// Memory stays bounded however slow `sink` is: `sink` runs on this task, so nothing is produced while
/// it blocks, and whatever fell due meanwhile is coalesced into one delta per stream before the next
/// flush. Must run on a Tokio runtime with the time driver enabled.
pub async fn run_script<F>(
    script: MockScript,
    chat_id: &str,
    frame_interval: Duration,
    cancel: CancellationToken,
    mut sink: F,
) -> RunOutcome
where
    F: FnMut(EventBatch) -> bool,
{
    let frame_interval = frame_interval.max(MIN_FRAME_INTERVAL);
    let script_turn = script.turn_id().map(str::to_owned);
    let mut batcher = FrameBatcher::new(chat_id);
    let mut turns = TurnTracker::default();
    let mut timeline = Timeline::new(script.steps, Instant::now());
    let mut last_flush: Option<Instant> = None;

    loop {
        if cancel.is_cancelled() {
            tracing::debug!(chat_id, "mock run cancelled");
            for event in turns.closing_events(script_turn.as_deref()) {
                batcher.push(event);
            }
            return match deliver(&mut batcher, &mut sink) {
                Ok(()) => RunOutcome::Cancelled,
                Err(SinkClosed) => RunOutcome::ReceiverGone,
            };
        }

        let now = Instant::now();
        let due = timeline.advance(now, &mut |event| {
            turns.observe(&event);
            batcher.push(event);
        });
        let Some(due) = due else {
            return match deliver(&mut batcher, &mut sink) {
                Ok(()) => RunOutcome::Completed,
                Err(SinkClosed) => RunOutcome::ReceiverGone,
            };
        };

        let flush_at = last_flush.map(|at| later(at, frame_interval));
        if !batcher.is_empty() && flush_at.is_none_or(|at| at <= now) {
            last_flush = Some(now);
            if deliver(&mut batcher, &mut sink).is_err() {
                tracing::debug!(chat_id, "mock run stopped: receiver gone");
                return RunOutcome::ReceiverGone;
            }
        }

        // While a flush is pending, tokens falling due before it are invisible anyway: sleep until the
        // flush (or the end of the current stream, so the script's end is noticed on time) instead of
        // waking for every token.
        let wake = match flush_at {
            Some(at) if !batcher.is_empty() => at.min(due.boundary),
            _ => due.next,
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => {}
            () = tokio::time::sleep_until(wake) => {}
        }
    }
}

/// Plays `script` on Tokio's clock, handing every event to `emit` at its scripted time, without
/// batching: the event level beneath [`run_script`], for a driver whose session batches frames.
///
/// Tokens that fall due together (after a stall, or while `emit` was slow) arrive as one delta per
/// stream, exactly as [`run_script`] coalesces them. `emit` returning `false` means its receiver is
/// gone: the run stops with [`RunOutcome::ReceiverGone`] and `emit` is not called again. When
/// `cancel` fires, the run stops at once with [`RunOutcome::Cancelled`] and adds nothing: unlike
/// [`run_script`] it does not close the turn, because the driver ends turns itself. Must run on a
/// Tokio runtime with the time driver enabled.
pub async fn play<F>(script: MockScript, cancel: CancellationToken, mut emit: F) -> RunOutcome
where
    F: FnMut(GhostEvent) -> bool,
{
    let mut timeline = Timeline::new(script.steps, Instant::now());
    loop {
        if cancel.is_cancelled() {
            return RunOutcome::Cancelled;
        }
        let mut gone = false;
        let due = timeline.advance(Instant::now(), &mut |event| {
            if !gone && !emit(event) {
                gone = true;
            }
        });
        if gone {
            return RunOutcome::ReceiverGone;
        }
        let Some(due) = due else {
            return RunOutcome::Completed;
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => {}
            () = tokio::time::sleep_until(due.next) => {}
        }
    }
}

/// The sink said its receiver is gone.
struct SinkClosed;

/// Flushes the batcher into `sink`, stopping at the first frame the sink refuses.
fn deliver<F>(batcher: &mut FrameBatcher, sink: &mut F) -> Result<(), SinkClosed>
where
    F: FnMut(EventBatch) -> bool,
{
    for frame in batcher.flush() {
        if !sink(frame) {
            return Err(SinkClosed);
        }
    }
    Ok(())
}

/// Follows turn boundaries in what the script emitted, so a cancellation closes the right turn once.
#[derive(Debug, Default)]
struct TurnTracker {
    /// A turn that started and has not ended.
    open: Option<String>,
    /// Whether any turn start or end went out.
    seen: bool,
}

impl TurnTracker {
    fn observe(&mut self, event: &GhostEvent) {
        match event {
            GhostEvent::TurnStarted { turn_id, .. } => {
                self.open = Some(turn_id.clone());
                self.seen = true;
            }
            GhostEvent::TurnEnded { turn_id, .. } => {
                if self.open.as_ref() == Some(turn_id) {
                    self.open = None;
                }
                self.seen = true;
            }
            _ => {}
        }
    }

    /// What closes the run after a cancellation: end the open turn as cancelled; if the script's turn
    /// never started, report it as started and cancelled so the UI is not left waiting for it; if the
    /// turn already ended, nothing.
    fn closing_events(&self, script_turn: Option<&str>) -> Vec<GhostEvent> {
        let cancelled = |turn_id: String| GhostEvent::TurnEnded {
            turn_id,
            stop_reason: StopReason::Cancelled,
            usage: None,
            duration_ms: None,
            recovered: false,
        };
        match (&self.open, script_turn) {
            (Some(open), _) => vec![cancelled(open.clone())],
            (None, Some(turn)) if !self.seen => {
                vec![
                    GhostEvent::TurnStarted {
                        turn_id: turn.to_owned(),
                        prompt_id: None,
                    },
                    cancelled(turn.to_owned()),
                ]
            }
            _ => Vec::new(),
        }
    }
}

/// When the timeline next needs attention.
#[derive(Debug, Clone, Copy)]
struct Due {
    /// The next event, or the script's end, falls due.
    next: Instant,
    /// The current stream ends (or, with no stream running, the next step starts). Waking here instead
    /// of at `next` skips per-token wakeups while still noticing the end of the script on time.
    boundary: Instant,
}

/// Plays a script's steps against a clock. Purely computational: the runner owns all waiting.
struct Timeline {
    steps: std::vec::IntoIter<MockStep>,
    /// Virtual time at which the next step starts.
    cursor: Instant,
    stream: Option<ActiveStream>,
}

/// A stream step in progress.
struct ActiveStream {
    kind: DeltaKind,
    id: String,
    text: String,
    rate: u32,
    start: Instant,
    /// Tokens in `text`.
    total: u64,
    /// Tokens emitted so far.
    emitted: u64,
    /// Byte offset of the first token not emitted yet.
    pos: usize,
}

impl ActiveStream {
    fn new(kind: DeltaKind, id: String, text: String, tokens_per_sec: u32, start: Instant) -> Self {
        let total = count_tokens(&text) as u64;
        Self {
            kind,
            id,
            text,
            rate: clamp_rate(tokens_per_sec),
            start,
            total,
            emitted: 0,
            pos: 0,
        }
    }

    /// When token `index` falls due; `due(total)` is when the stream's time slot ends.
    fn due(&self, index: u64) -> Instant {
        later(self.start, token_offset(index, self.rate))
    }
}

impl Timeline {
    fn new(steps: Vec<MockStep>, start: Instant) -> Self {
        Self {
            steps: steps.into_iter(),
            cursor: start,
            stream: None,
        }
    }

    /// Emits, in order, everything due at or before `now`. Returns when the timeline next needs
    /// attention, or `None` once the whole script has played out.
    fn advance(&mut self, now: Instant, emit: &mut impl FnMut(GhostEvent)) -> Option<Due> {
        loop {
            if let Some(stream) = self.stream.as_mut() {
                let from = stream.pos;
                while stream.emitted < stream.total && stream.due(stream.emitted) <= now {
                    stream.pos = token_end(&stream.text, stream.pos);
                    stream.emitted += 1;
                }
                // Everything due goes out as one delta: tokens that piled up while the runner slept, or
                // while a slow sink held it up, cost one event instead of one per token.
                if stream.pos > from
                    && let Some(text) = stream.text.get(from..stream.pos)
                {
                    emit(stream.kind.event(stream.id.clone(), text.to_owned()));
                }
                if stream.emitted < stream.total {
                    return Some(Due {
                        next: stream.due(stream.emitted),
                        boundary: stream.due(stream.total),
                    });
                }
                self.cursor = stream.due(stream.total);
                self.stream = None;
            }
            if self.cursor > now {
                return Some(Due {
                    next: self.cursor,
                    boundary: self.cursor,
                });
            }
            match self.steps.next()? {
                MockStep::Emit(event) => emit(event),
                MockStep::Sleep(duration) => self.cursor = later(self.cursor, duration),
                MockStep::StreamText {
                    message_id,
                    text,
                    tokens_per_sec,
                } => {
                    self.stream = Some(ActiveStream::new(
                        DeltaKind::Message,
                        message_id,
                        text,
                        tokens_per_sec,
                        self.cursor,
                    ));
                }
                MockStep::StreamThought {
                    thought_id,
                    text,
                    tokens_per_sec,
                } => {
                    self.stream = Some(ActiveStream::new(
                        DeltaKind::Thought,
                        thought_id,
                        text,
                        tokens_per_sec,
                        self.cursor,
                    ));
                }
            }
        }
    }
}

fn clamp_rate(tokens_per_sec: u32) -> u32 {
    tokens_per_sec.clamp(MIN_TOKENS_PER_SEC, MAX_TOKENS_PER_SEC)
}

/// Offset of token `index` from its stream's start: `index / rate` seconds, in integer nanoseconds so
/// long streams do not drift.
fn token_offset(index: u64, rate: u32) -> Duration {
    let nanos = u128::from(index) * 1_000_000_000 / u128::from(rate.max(1));
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// `at + by`, capped at [`FAR_FUTURE`] instead of overflowing.
fn later(at: Instant, by: Duration) -> Instant {
    at.checked_add(by.min(FAR_FUTURE)).unwrap_or(at)
}

/// How long `tokens` take to stream at `rate`, in whole milliseconds.
fn stream_millis(tokens: usize, rate: u32) -> u32 {
    u32::try_from(token_offset(tokens as u64, clamp_rate(rate)).as_millis()).unwrap_or(u32::MAX)
}

fn to_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// A tool update that completes the tool with `content`, leaving every other field as it was.
fn completed(tool_call_id: String, content: ToolContent) -> ToolUpdate {
    ToolUpdate {
        tool_call_id,
        status: Some(ToolStatus::Completed),
        title: None,
        kind: None,
        locations: None,
        raw_input: None,
        tool_name: None,
        content: Some(vec![content]),
        truncated: None,
    }
}

/// Byte offset where the token starting at `from` ends. A token is a word plus the whitespace after it,
/// newlines included (so a blank line stays with the paragraph it ends), which makes tokens concatenate
/// back to the text exactly; whitespace before the first word belongs to the first token.
fn token_end(text: &str, from: usize) -> usize {
    let Some(rest) = text.get(from..) else {
        return text.len();
    };
    let mut in_word = false;
    let mut after_word = false;
    for (index, c) in rest.char_indices() {
        let space = c.is_whitespace();
        if after_word && !space {
            return from + index;
        }
        if !space {
            in_word = true;
        } else if in_word {
            after_word = true;
        }
    }
    text.len()
}

/// A text's tokens, as defined by [`token_end`].
#[derive(Debug, Clone)]
struct Tokens<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.pos >= self.text.len() {
            return None;
        }
        let end = token_end(self.text, self.pos);
        let token = self.text.get(self.pos..end)?;
        self.pos = end;
        Some(token)
    }
}

fn tokens(text: &str) -> Tokens<'_> {
    Tokens { text, pos: 0 }
}

fn count_tokens(text: &str) -> usize {
    tokens(text).count()
}

/// `count` tokens taken from `source` over and over. `source` ends in whitespace, so tokens never merge
/// across the seam and the count is exact.
fn cycle_tokens(source: &str, count: usize) -> String {
    tokens(source).cycle().take(count).collect()
}

/// Markdown of exactly `target` tokens: whole sections while they fit, topped up with a plain paragraph.
fn stress_answer(target: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    let mut part = 1;
    'fill: loop {
        for (title, body) in STRESS_SECTIONS {
            let section = format!("## {part}. {title}\n\n{body}\n\n");
            let section_tokens = count_tokens(&section);
            if count + section_tokens > target {
                break 'fill;
            }
            out.push_str(&section);
            count += section_tokens;
            part += 1;
        }
    }
    out.push_str(&cycle_tokens(STRESS_FILLER, target.saturating_sub(count)));
    out
}

fn plan_entry(content: &str, status: PlanStatus, priority: PlanPriority) -> PlanEntry {
    PlanEntry {
        content: content.to_owned(),
        status,
        priority: Some(priority),
    }
}

const CONTEXT_SIZE: u64 = 200_000;
const DEMO_PROMPT_TOKENS: u64 = 12_480;
const DEMO_CACHED_TOKENS: u64 = 9_216;
const STRESS_PROMPT_TOKENS: u64 = 4_096;

const DEMO_THOUGHT: &str = "The user wants the greeting pulled out of `main` so it can be reused. I'll read \
`src/main.rs` first to see how it is printed today, then extract a `greet` function and keep the output \
exactly the same.";

const DEMO_MAIN_BEFORE: &str = r#"fn main() {
    let name = "world";
    println!("Hello, {}!", name);
}
"#;

const DEMO_MAIN_AFTER: &str = r#"fn greet(name: &str) -> String {
    format!("Hello, {name}!")
}

fn main() {
    println!("{}", greet("world"));
}
"#;

const DEMO_ANSWER: &str = r#"## Extracted a `greet` helper

I read `src/main.rs` and moved the greeting into its own function, so it can be reused and tested without capturing stdout.

- `greet(name)` builds the message instead of formatting it inline
- `main` now prints `greet("world")`
- The output is unchanged: `Hello, world!`

```rust
fn greet(name: &str) -> String {
    format!("Hello, {name}!")
}
```

Run `cargo run` to check it, or add a unit test for `greet`.
"#;

const STRESS_THOUGHT: &str = "Long answer requested. I will cover parsing, batching, rendering and measuring, \
with a code sample per layer, a table of budgets, and steps to reproduce the numbers. ";

const STRESS_FILLER: &str = "The stream keeps flowing while the renderer catches up, one frame at a time, and \
every frame stays small enough for the fast path. ";

/// Sections the stress answer cycles through: every common markdown construct, five code blocks in
/// five languages, and a paragraph of mixed scripts and emoji.
const STRESS_SECTIONS: &[(&str, &str)] = &[
    (
        "Where the time goes",
        "Streaming a long answer touches every layer at once: the driver parses **thousands of deltas**, the \
batcher packs them into frames under 8 KiB, and the UI re-renders only the _last_ block on each frame. Keep \
`requestAnimationFrame` work under 4 ms; [the rendering notes](https://onghost.dev/docs/rendering) explain the \
budget.",
    ),
    (
        "Batching in Rust",
        r#"```rust
use std::time::Duration;

/// Flushes pending events once per frame until the UI goes away.
pub async fn pump(mut batcher: FrameBatcher, frame: Duration, mut send: impl FnMut(EventBatch) -> bool) {
    let mut ticker = tokio::time::interval(frame);
    loop {
        ticker.tick().await;
        for batch in batcher.flush() {
            if !send(batch) {
                return;
            }
        }
    }
}
```"#,
    ),
    (
        "Checklist",
        "- Parse the stream lazily
  - one `serde_json::from_str` per line
  - unknown events are logged, never fatal
- Coalesce deltas per message and per thought
- Split frames on **grapheme** boundaries
- Highlight finished code blocks in a worker",
    ),
    (
        "Applying a batch in TypeScript",
        r#"```ts
export function applyBatch(state: Transcript, batch: EventBatch): Transcript {
  if (batch.seq_start !== state.nextSeq) {
    return requestReplay(state, state.nextSeq);
  }
  for (const event of batch.events) {
    if (event.type === "message_delta") {
      state.messages.get(event.message_id)?.append(event.text);
    }
  }
  return { ...state, nextSeq: batch.seq_start + batch.events.length };
}
```"#,
    ),
    (
        "Budgets",
        "| Stage | Budget | Measured |
| :--- | ---: | ---: |
| Parse | 0.50 ms | 0.31 ms |
| Batch | 0.20 ms | 0.08 ms |
| Render | 4.00 ms | 3.70 ms |
| Total | 16.0 ms | 4.09 ms |",
    ),
    (
        "Reproducing it",
        "1. Start the mock driver at `5000` tokens per second.
2. Open the performance panel and start recording.
3. Scroll up and down while the answer streams.
4. Check that no frame takes longer than **16 ms**.",
    ),
    (
        "Unicode",
        "Frames never cut a character in half: déjà vu, naïve café, Ångström, 日本語のテキスト, 한국어 문장, \
Ελληνικά, עברית, and emoji such as 🚀, 👩‍💻, 👍🏽 and 🇸🇮 arrive intact even when a delta is split mid-sentence.",
    ),
    (
        "Commands",
        "```bash
pnpm --filter desktop dev
cargo test -p ghost-drivers -- --nocapture
RUST_LOG=ghost_drivers=debug cargo run -p ghost-cli -- replay fixtures/stress.jsonl
```",
    ),
    (
        "Principles",
        "> Every animation means something: the orb and the aura are live state, not decoration.
>
> Streaming should feel calm, even at five thousand tokens per second.",
    ),
    (
        "Settings",
        r#"```json
{
  "frameIntervalMs": 16,
  "maxFrameBytes": 8192,
  "tokensPerSec": 5000,
  "agents": ["claude", "codex", "pi"]
}
```"#,
    ),
    (
        "Reference coalescer in Python",
        r#"```python
def coalesce(events):
    merged = []
    for event in events:
        last = merged[-1] if merged else None
        same_message = last is not None and last.get("message_id") == event.get("message_id")
        if same_message and last["type"] == event["type"] == "message_delta":
            last["text"] += event["text"]
        else:
            merged.append(dict(event))
    return merged
```"#,
    ),
    (
        "Status",
        "- [x] Frames stay below the size limit
- [x] Sequence numbers never skip
- [ ] Highlight code while it streams

---",
    ),
    (
        "Why frames",
        "Without batching, a fast agent produces hundreds of tiny messages per second, each one a separate trip \
through the webview's message queue, a separate JSON parse, and a separate React update. Batching per animation \
frame turns that into one parse and one update per frame, however fast the tokens arrive, which keeps scrolling \
smooth and leaves the main thread free for input. The cost is at most one frame of extra latency, which nobody \
can see.",
    ),
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use ghost_protocol::{MAX_FRAME_BYTES, NoticeLevel};

    use super::*;

    const FRAME: Duration = Duration::from_millis(16);

    /// What the sink saw, each frame with the (paused) time since the run started.
    struct Run {
        outcome: RunOutcome,
        frames: Vec<(Duration, EventBatch)>,
        elapsed: Duration,
    }

    impl Run {
        fn events(&self) -> Vec<GhostEvent> {
            self.frames
                .iter()
                .flat_map(|(_, frame)| frame.events.iter().cloned())
                .collect()
        }

        fn text(&self, id: &str) -> String {
            self.events()
                .into_iter()
                .filter_map(|event| match event {
                    GhostEvent::MessageDelta { message_id, text } if message_id == id => Some(text),
                    _ => None,
                })
                .collect()
        }

        /// Times at which flushes happened; frames of one flush share a timestamp.
        fn flush_times(&self) -> Vec<Duration> {
            let mut times: Vec<Duration> = self.frames.iter().map(|(at, _)| *at).collect();
            times.dedup();
            times
        }

        /// Every frame is under the limit, numbered contiguously from zero, and coalesced.
        fn assert_frames_valid(&self) {
            let mut next = 0u32;
            for (_, frame) in &self.frames {
                assert_eq!(frame.chat_id, "session-1");
                assert_eq!(frame.seq_start, next, "sequence numbers must not skip");
                assert!(
                    frame.encoded_len() < MAX_FRAME_BYTES,
                    "frame of {} bytes",
                    frame.encoded_len()
                );
                next += u32::try_from(frame.events.len()).expect("frame length fits u32");
                for pair in frame.events.windows(2) {
                    match pair {
                        [
                            GhostEvent::MessageDelta { message_id: a, .. },
                            GhostEvent::MessageDelta { message_id: b, .. },
                        ]
                        | [
                            GhostEvent::ThoughtDelta { thought_id: a, .. },
                            GhostEvent::ThoughtDelta { thought_id: b, .. },
                        ] => {
                            assert_ne!(a, b, "adjacent deltas for one id must have been coalesced");
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    async fn play(script: MockScript, frame_interval: Duration, cancel: CancellationToken) -> Run {
        let start = Instant::now();
        let mut frames = Vec::new();
        let outcome = run_script(script, "session-1", frame_interval, cancel, |batch| {
            frames.push((start.elapsed(), batch));
            true
        })
        .await;
        Run {
            outcome,
            frames,
            elapsed: start.elapsed(),
        }
    }

    /// A script streaming `tokens` numbered words as one message.
    fn stream(tokens: usize, tokens_per_sec: u32) -> (MockScript, String) {
        let text: String = (0..tokens).map(|index| format!("w{index} ")).collect();
        let step = MockStep::StreamText {
            message_id: "m".into(),
            text: text.clone(),
            tokens_per_sec,
        };
        (MockScript { steps: vec![step] }, text)
    }

    /// The text of the script's first `StreamText` step.
    fn script_answer(script: &MockScript) -> String {
        script
            .steps
            .iter()
            .find_map(|step| match step {
                MockStep::StreamText { text, .. } => Some(text.clone()),
                _ => None,
            })
            .expect("script streams a message")
    }

    fn emitted(script: &MockScript) -> Vec<&GhostEvent> {
        script
            .steps
            .iter()
            .filter_map(|step| match step {
                MockStep::Emit(event) => Some(event),
                _ => None,
            })
            .collect()
    }

    fn cancel_after(delay: Duration) -> CancellationToken {
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            trigger.cancel();
        });
        cancel
    }

    fn turn_ended(turn_id: &str, stop_reason: StopReason) -> GhostEvent {
        GhostEvent::TurnEnded {
            turn_id: turn_id.into(),
            stop_reason,
            usage: None,
            duration_ms: None,
            recovered: false,
        }
    }

    fn turn_started(turn_id: &str) -> GhostEvent {
        GhostEvent::TurnStarted {
            turn_id: turn_id.into(),
            prompt_id: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn demo_turn_plays_out_in_order() {
        let run = play(
            MockScript::demo_turn("turn-1", 80),
            FRAME,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(run.outcome, RunOutcome::Completed);
        let events = run.events();
        assert!(
            matches!(events.first(), Some(GhostEvent::TurnStarted { turn_id, .. }) if turn_id == "turn-1")
        );
        assert!(matches!(
            events.last(),
            Some(GhostEvent::TurnEnded { turn_id, stop_reason: StopReason::EndTurn, usage: Some(_), .. }) if turn_id == "turn-1"
        ));
        let mut announced = HashSet::new();
        let mut updates = 0;
        for event in &events {
            match event {
                GhostEvent::ToolCall(call) => {
                    announced.insert(call.tool_call_id.clone());
                }
                GhostEvent::ToolUpdate(update) => {
                    assert!(
                        announced.contains(&update.tool_call_id),
                        "update before its call: {update:?}"
                    );
                    updates += 1;
                }
                _ => {}
            }
        }
        assert_eq!(updates, 2);
        assert_eq!(run.text("turn-1-answer"), DEMO_ANSWER);
        run.assert_frames_valid();
    }

    #[test]
    fn demo_turn_shows_every_kind_of_block() {
        let script = MockScript::demo_turn("turn-1", 80);
        let events = emitted(&script);
        let duration = stream_millis(count_tokens(DEMO_THOUGHT), 80);
        assert!(events.iter().any(|event| matches!(
            event,
            GhostEvent::ThoughtCompleted { duration_ms: Some(ms), .. } if *ms == duration && *ms > 0
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            GhostEvent::ToolCall(ToolCall { kind: ToolKind::Read, title, status: ToolStatus::InProgress, locations, .. })
                if title == "Read src/main.rs" && !locations.is_empty()
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            GhostEvent::ToolUpdate(ToolUpdate { status: Some(ToolStatus::Completed), content: Some(content), .. })
                if matches!(content.as_slice(), [ToolContent::Text { .. }])
        )));
        assert!(events.iter().any(
            |event| matches!(event, GhostEvent::ToolCall(call) if call.kind == ToolKind::Edit)
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            GhostEvent::ToolUpdate(ToolUpdate { content: Some(content), .. })
                if matches!(content.as_slice(), [ToolContent::Diff { old_text: Some(_), .. }])
        )));
        assert!(events.iter().any(
            |event| matches!(event, GhostEvent::PlanUpdated { entries } if entries.len() == 3)
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            GhostEvent::SessionUpdated(SessionInfo { model: Some(model), title: Some(_), .. }) if model == "demo"
        )));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, GhostEvent::UsageUpdated(_)))
        );
        let answer = script_answer(&script);
        assert!(answer.starts_with("## "));
        assert!(answer.contains("\n- "));
        assert!(answer.contains("```rust\n"));
        assert!(answer.contains("`cargo run`"));
        assert_eq!(script.turn_id(), Some("turn-1"));
    }

    #[tokio::test(start_paused = true)]
    async fn streams_at_the_requested_rate() {
        let tokens = 1_000;
        for rate in [7_u32, 60, 250, MAX_TOKENS_PER_SEC] {
            let (script, text) = stream(tokens, rate);
            let run = play(script, FRAME, CancellationToken::new()).await;
            assert_eq!(run.outcome, RunOutcome::Completed);
            assert_eq!(run.text("m"), text);

            let expected = tokens as f64 / f64::from(rate);
            let actual = run.elapsed.as_secs_f64();
            assert!(
                (actual - expected).abs() <= expected * 0.1,
                "{rate}/s: took {actual}s, expected {expected}s"
            );

            // Along the way, whatever has arrived by each flush matches the rate too.
            let mut delivered = String::new();
            let mut by_time: Vec<(Duration, usize)> = Vec::new();
            for (at, frame) in &run.frames {
                for event in &frame.events {
                    if let GhostEvent::MessageDelta { text, .. } = event {
                        delivered.push_str(text);
                    }
                }
                match by_time.last_mut() {
                    Some((last, len)) if last == at => *len = delivered.len(),
                    _ => by_time.push((*at, delivered.len())),
                }
            }
            for (at, len) in by_time {
                let arrived = count_tokens(&delivered[..len]);
                let due = (at.as_nanos() * u128::from(rate) / 1_000_000_000) as usize + 1;
                assert!(
                    arrived.abs_diff(due.min(tokens)) <= 1,
                    "{rate}/s at {at:?}: {arrived} tokens, {due} due"
                );
            }
            run.assert_frames_valid();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn tokens_per_sec_is_clamped() {
        assert_eq!(clamp_rate(0), MIN_TOKENS_PER_SEC);
        assert_eq!(clamp_rate(1_000_000), MAX_TOKENS_PER_SEC);

        // Zero streams at one token per second instead of never finishing.
        let (script, text) = stream(5, 0);
        let run = play(script, FRAME, CancellationToken::new()).await;
        assert_eq!(run.text("m"), text);
        assert!(
            (run.elapsed.as_secs_f64() - 5.0).abs() <= 0.5,
            "took {:?}",
            run.elapsed
        );

        // A million per second streams at the 5 000 cap.
        let (script, text) = stream(5_000, 1_000_000);
        let run = play(script, FRAME, CancellationToken::new()).await;
        assert_eq!(run.text("m"), text);
        assert!(
            (run.elapsed.as_secs_f64() - 1.0).abs() <= 0.1,
            "took {:?}",
            run.elapsed
        );

        // The builders store the clamped rate.
        for (script, rate) in [
            (MockScript::demo_turn("t", 0), MIN_TOKENS_PER_SEC),
            (MockScript::stress("t", 1_000_000, 500), MAX_TOKENS_PER_SEC),
        ] {
            for step in &script.steps {
                if let MockStep::StreamText { tokens_per_sec, .. }
                | MockStep::StreamThought { tokens_per_sec, .. } = step
                {
                    assert_eq!(*tokens_per_sec, rate);
                }
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn flushes_at_most_once_per_frame_interval() {
        let run = play(
            MockScript::stress("t", MAX_TOKENS_PER_SEC, 3_000),
            FRAME,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(run.outcome, RunOutcome::Completed);
        let times = run.flush_times();
        assert!(times.len() > 20);
        // Only the final flush may come sooner than one interval after the previous one.
        for pair in times[..times.len() - 1].windows(2) {
            assert!(
                pair[1] - pair[0] >= FRAME,
                "flushes at {:?} and {:?}",
                pair[0],
                pair[1]
            );
        }
        run.assert_frames_valid();
    }

    #[tokio::test(start_paused = true)]
    async fn first_token_after_a_pause_is_flushed_at_once() {
        let script = MockScript {
            steps: vec![
                MockStep::Emit(turn_started("t")),
                MockStep::Sleep(Duration::from_secs(1)),
                MockStep::StreamText {
                    message_id: "m".into(),
                    text: "one two three".into(),
                    tokens_per_sec: 1,
                },
            ],
        };
        let run = play(script, FRAME, CancellationToken::new()).await;
        let deliveries: Vec<(Duration, Vec<GhostEvent>)> = run
            .frames
            .iter()
            .map(|(at, frame)| (*at, frame.events.clone()))
            .collect();
        let delta = |text: &str| GhostEvent::MessageDelta {
            message_id: "m".into(),
            text: text.into(),
        };
        assert_eq!(
            deliveries,
            vec![
                (Duration::ZERO, vec![turn_started("t")]),
                (Duration::from_secs(1), vec![delta("one ")]),
                (Duration::from_secs(2), vec![delta("two ")]),
                (Duration::from_secs(3), vec![delta("three")]),
            ]
        );
        // The stream's slot ends one token after its last token.
        assert_eq!(run.elapsed, Duration::from_secs(4));
    }

    #[tokio::test]
    async fn plays_on_a_real_clock() {
        // 500 tokens at 5 000 per second: a tenth of a second of real streaming, through Tokio's real
        // millisecond timers instead of paused time.
        let (script, text) = stream(500, MAX_TOKENS_PER_SEC);
        let run = play(script, FRAME, CancellationToken::new()).await;
        assert_eq!(run.outcome, RunOutcome::Completed);
        assert_eq!(run.text("m"), text);
        assert!(
            run.elapsed >= Duration::from_millis(100),
            "finished early: {:?}",
            run.elapsed
        );
        assert!(
            run.elapsed < Duration::from_secs(5),
            "took {:?}",
            run.elapsed
        );
        // Throttled: about one frame per interval, not one per token.
        let max_frames = run.elapsed.as_millis() / FRAME.as_millis() + 2;
        assert!(
            run.frames.len() as u128 <= max_frames,
            "{} frames in {:?}",
            run.frames.len(),
            run.elapsed
        );
        run.assert_frames_valid();
    }

    #[tokio::test(start_paused = true)]
    async fn the_end_is_noticed_while_a_flush_is_pending() {
        // Ten tokens take 10 ms. With a one-second frame interval the run must still finish at 10 ms
        // instead of waiting for the next allowed flush.
        let (script, text) = stream(10, 1_000);
        let run = play(script, Duration::from_secs(1), CancellationToken::new()).await;
        assert_eq!(run.outcome, RunOutcome::Completed);
        assert_eq!(run.text("m"), text);
        assert_eq!(run.elapsed, Duration::from_millis(10));
        // The first token at once, then the final flush with the rest.
        assert_eq!(
            run.flush_times(),
            vec![Duration::ZERO, Duration::from_millis(10)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_mid_stream_ends_the_turn_as_cancelled() {
        // About twenty seconds of streaming, cancelled after one and a half.
        let script = MockScript::stress("turn-9", 100, 2_000);
        let answer = script_answer(&script);
        let cancel_at = Duration::from_millis(1_500);
        let run = play(script, FRAME, cancel_after(cancel_at)).await;
        assert_eq!(run.outcome, RunOutcome::Cancelled);
        assert!(
            run.elapsed >= cancel_at && run.elapsed <= cancel_at + FRAME,
            "stopped at {:?}",
            run.elapsed
        );

        let events = run.events();
        assert_eq!(
            events.last(),
            Some(&turn_ended("turn-9", StopReason::Cancelled))
        );
        let ends = events
            .iter()
            .filter(|event| matches!(event, GhostEvent::TurnEnded { .. }))
            .count();
        assert_eq!(ends, 1);
        // The answer was cut off cleanly: what arrived is a strict prefix.
        let streamed = run.text("turn-9-answer");
        assert!(
            !streamed.is_empty() && streamed.len() < answer.len() && answer.starts_with(&streamed)
        );
        run.assert_frames_valid();
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_from_the_sink_stops_at_once() {
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let mut frames = Vec::new();
        let outcome = run_script(
            MockScript::stress("turn-3", 1_000, 5_000),
            "s",
            FRAME,
            cancel,
            |batch| {
                frames.push(batch);
                if frames.len() == 3 {
                    trigger.cancel();
                }
                true
            },
        )
        .await;
        assert_eq!(outcome, RunOutcome::Cancelled);
        // Nothing from the script after the cancel: the only frame left closes the turn.
        assert_eq!(frames.len(), 4);
        assert_eq!(
            frames[3].events,
            vec![turn_ended("turn-3", StopReason::Cancelled)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_before_start_reports_a_cancelled_turn() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let run = play(MockScript::demo_turn("turn-0", 100), FRAME, cancel).await;
        assert_eq!(run.outcome, RunOutcome::Cancelled);
        assert_eq!(
            run.events(),
            vec![
                turn_started("turn-0"),
                turn_ended("turn-0", StopReason::Cancelled)
            ]
        );
        assert_eq!(run.elapsed, Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_after_the_turn_ended_adds_nothing() {
        let ended = turn_ended("t", StopReason::EndTurn);
        let script = MockScript {
            steps: vec![
                MockStep::Emit(turn_started("t")),
                MockStep::Emit(ended.clone()),
                MockStep::Sleep(Duration::from_secs(10)),
                MockStep::Emit(GhostEvent::Notice {
                    level: NoticeLevel::Info,
                    message: "never sent".into(),
                }),
            ],
        };
        let run = play(script, FRAME, cancel_after(Duration::from_secs(1))).await;
        assert_eq!(run.outcome, RunOutcome::Cancelled);
        assert_eq!(run.events(), vec![turn_started("t"), ended]);
        assert_eq!(run.elapsed, Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_without_a_turn_adds_nothing() {
        let hello = GhostEvent::Notice {
            level: NoticeLevel::Info,
            message: "hello".into(),
        };
        let script = MockScript {
            steps: vec![
                MockStep::Emit(hello.clone()),
                MockStep::Sleep(Duration::from_secs(10)),
            ],
        };
        let run = play(script, FRAME, cancel_after(Duration::from_secs(1))).await;
        assert_eq!(run.outcome, RunOutcome::Cancelled);
        assert_eq!(run.events(), vec![hello]);
    }

    #[tokio::test(start_paused = true)]
    async fn closed_receiver_stops_without_further_calls() {
        let mut calls = 0;
        let start = Instant::now();
        let outcome = run_script(
            MockScript::demo_turn("t", 200),
            "s",
            FRAME,
            CancellationToken::new(),
            |_| {
                calls += 1;
                calls < 3
            },
        )
        .await;
        assert_eq!(outcome, RunOutcome::ReceiverGone);
        assert_eq!(calls, 3);
        // It stopped right there instead of playing out the rest of the turn.
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "ran for {:?}",
            start.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn closed_receiver_on_a_closing_flush_is_reported() {
        // Refusing the final frame of a completed run.
        let mut calls = 0;
        let outcome = run_script(
            MockScript::demo_turn("t", 500),
            "s",
            FRAME,
            CancellationToken::new(),
            |batch| {
                calls += 1;
                !batch
                    .events
                    .iter()
                    .any(|event| matches!(event, GhostEvent::TurnEnded { .. }))
            },
        )
        .await;
        assert_eq!(outcome, RunOutcome::ReceiverGone);
        assert!(calls > 3);

        // Refusing the frame that closes a cancelled run.
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut calls = 0;
        let outcome = run_script(MockScript::demo_turn("t", 500), "s", FRAME, cancel, |_| {
            calls += 1;
            false
        })
        .await;
        assert_eq!(outcome, RunOutcome::ReceiverGone);
        assert_eq!(calls, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn long_frame_intervals_still_produce_small_frames() {
        // About 5 000 tokens (30 KB) fall due per one-second flush, so every flush needs several frames.
        let script = MockScript::stress("t", MAX_TOKENS_PER_SEC, 20_000);
        let answer = script_answer(&script);
        let run = play(script, Duration::from_secs(1), CancellationToken::new()).await;
        assert_eq!(run.outcome, RunOutcome::Completed);
        run.assert_frames_valid();
        let times = run.flush_times();
        assert!(
            run.frames.len() > times.len() * 2,
            "{} frames over {} flushes",
            run.frames.len(),
            times.len()
        );
        assert_eq!(run.text("t-answer"), answer);
        assert!(matches!(
            run.events().last(),
            Some(GhostEvent::TurnEnded {
                stop_reason: StopReason::EndTurn,
                ..
            })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn zero_frame_interval_is_clamped() {
        let (script, text) = stream(200, 1_000);
        let run = play(script, Duration::ZERO, CancellationToken::new()).await;
        assert_eq!(run.outcome, RunOutcome::Completed);
        assert_eq!(run.text("m"), text);
        let times = run.flush_times();
        for pair in times[..times.len() - 1].windows(2) {
            assert!(pair[1] - pair[0] >= MIN_FRAME_INTERVAL);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn absurd_sleeps_do_not_overflow() {
        let script = MockScript {
            steps: vec![
                MockStep::Emit(turn_started("t")),
                MockStep::Sleep(Duration::MAX),
                MockStep::Sleep(Duration::MAX),
                MockStep::Emit(GhostEvent::Notice {
                    level: NoticeLevel::Info,
                    message: "far away".into(),
                }),
            ],
        };
        let run = play(script, Duration::MAX, cancel_after(Duration::from_secs(1))).await;
        assert_eq!(run.outcome, RunOutcome::Cancelled);
        assert_eq!(
            run.events(),
            vec![turn_started("t"), turn_ended("t", StopReason::Cancelled)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn empty_script_completes_without_frames() {
        let run = play(MockScript::default(), FRAME, CancellationToken::new()).await;
        assert_eq!(run.outcome, RunOutcome::Completed);
        assert!(run.frames.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_long_stall_coalesces_into_one_delta_per_stream() {
        let start = Instant::now();
        let script = MockScript::stress("t", MAX_TOKENS_PER_SEC, 50_000);

        // Everything due at once, as after an hour-long stall: one event per run of tokens.
        let mut timeline = Timeline::new(script.steps.clone(), start);
        let mut batcher = FrameBatcher::new("s");
        let mut count = 0;
        let due = timeline.advance(start + Duration::from_secs(3_600), &mut |event| {
            count += 1;
            batcher.push(event);
        });
        assert!(due.is_none(), "the whole script is due");
        // TurnStarted, thought, ThoughtCompleted, answer, MessageCompleted, UsageUpdated, TurnEnded.
        assert_eq!(count, 7);
        assert_eq!(batcher.pending_len(), 7);

        // Several wakeups between two flushes still leave one queued delta.
        let mut timeline = Timeline::new(script.steps, start);
        let mut batcher = FrameBatcher::new("s");
        for second in [3, 4, 5] {
            timeline.advance(start + Duration::from_secs(second), &mut |event| {
                batcher.push(event)
            });
        }
        // TurnStarted, the whole thought, ThoughtCompleted, and the answer so far.
        assert_eq!(batcher.pending_len(), 4);
    }

    #[test]
    fn stress_script_has_the_requested_length() {
        for total in [0_u32, 1, 7, 50, 1_000, 10_000, 100_000] {
            let script = MockScript::stress("t", 1_000, total);
            let counted = script.streamed_token_count();
            let tolerance = total as usize / 10;
            assert!(
                counted.abs_diff(total as usize) <= tolerance,
                "{total} requested, {counted} streamed"
            );
            let events = emitted(&script);
            assert!(matches!(
                events.first(),
                Some(GhostEvent::TurnStarted { .. })
            ));
            assert!(matches!(
                events.last(),
                Some(GhostEvent::TurnEnded {
                    stop_reason: StopReason::EndTurn,
                    ..
                })
            ));
        }
    }

    #[test]
    fn stress_script_is_varied_markdown() {
        let answer = script_answer(&MockScript::stress("t", 1_000, 5_000));
        let fences = answer
            .lines()
            .filter(|line| line.starts_with("```"))
            .count();
        assert!(fences >= 10 && fences % 2 == 0, "{fences} fences");
        for marker in [
            "```rust",
            "```ts",
            "```bash",
            "```json",
            "```python",
            "| Stage |",
            "\n> ",
            "\n1. ",
            "- [x]",
        ] {
            assert!(answer.contains(marker), "missing {marker:?}");
        }
        assert!(answer.contains("日本語") && answer.contains("👩‍💻") && answer.contains("déjà"));
        // A small budget still ends on whole sections, never inside a code block.
        for total in [40_u32, 120, 300, 700] {
            let answer = script_answer(&MockScript::stress("t", 1_000, total));
            assert_eq!(
                answer
                    .lines()
                    .filter(|line| line.starts_with("```"))
                    .count()
                    % 2,
                0
            );
        }
    }

    #[test]
    fn tokens_are_words_with_trailing_whitespace() {
        assert_eq!(
            tokens("Hello world\n\n## Title\n").collect::<Vec<_>>(),
            ["Hello ", "world\n\n", "## ", "Title\n"]
        );
        assert_eq!(tokens("  lead in").collect::<Vec<_>>(), ["  lead ", "in"]);
        assert_eq!(tokens("\r\n\r\n").collect::<Vec<_>>(), ["\r\n\r\n"]);
        assert_eq!(
            tokens("日本語のテキスト 🚀").collect::<Vec<_>>(),
            ["日本語のテキスト ", "🚀"]
        );
        assert_eq!(tokens("").count(), 0);
        for text in [
            DEMO_ANSWER,
            DEMO_THOUGHT,
            DEMO_MAIN_AFTER,
            "a\tb\u{a0}c  ",
            " x ",
        ] {
            assert_eq!(tokens(text).collect::<String>(), text);
        }
        assert_eq!(cycle_tokens("a b ", 5), "a b a b a ");
        assert_eq!(cycle_tokens("", 5), "");
    }

    /// What `play` emitted, each event with the (paused) time since the run started.
    async fn play_events(
        script: MockScript,
        cancel: CancellationToken,
    ) -> (RunOutcome, Vec<(Duration, GhostEvent)>) {
        let start = Instant::now();
        let mut events = Vec::new();
        let outcome = super::play(script, cancel, |event| {
            events.push((start.elapsed(), event));
            true
        })
        .await;
        (outcome, events)
    }

    #[tokio::test(start_paused = true)]
    async fn play_emits_each_event_at_its_time() {
        let script = MockScript {
            steps: vec![
                MockStep::Emit(turn_started("t")),
                MockStep::Sleep(Duration::from_secs(1)),
                MockStep::StreamText {
                    message_id: "m".into(),
                    text: "one two three".into(),
                    tokens_per_sec: 2,
                },
                MockStep::Emit(turn_ended("t", StopReason::EndTurn)),
            ],
        };
        let (outcome, events) = play_events(script, CancellationToken::new()).await;
        assert_eq!(outcome, RunOutcome::Completed);
        let delta = |text: &str| GhostEvent::MessageDelta {
            message_id: "m".into(),
            text: text.into(),
        };
        assert_eq!(
            events,
            vec![
                (Duration::ZERO, turn_started("t")),
                (Duration::from_secs(1), delta("one ")),
                (Duration::from_millis(1_500), delta("two ")),
                (Duration::from_secs(2), delta("three")),
                (
                    Duration::from_millis(2_500),
                    turn_ended("t", StopReason::EndTurn)
                ),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn play_stops_on_cancel_without_closing_the_turn() {
        let script = MockScript::stress("t", 100, 2_000);
        let (outcome, events) = play_events(script, cancel_after(Duration::from_millis(500))).await;
        assert_eq!(outcome, RunOutcome::Cancelled);
        assert!(matches!(
            events.first(),
            Some((_, GhostEvent::TurnStarted { .. }))
        ));
        assert!(
            !events
                .iter()
                .any(|(_, event)| matches!(event, GhostEvent::TurnEnded { .. })),
            "the driver ends the turn, not play"
        );
        assert!(
            events
                .iter()
                .all(|(at, _)| *at <= Duration::from_millis(500))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn play_stops_when_the_receiver_is_gone() {
        let mut calls = 0;
        let outcome = super::play(
            MockScript::demo_turn("t", 200),
            CancellationToken::new(),
            |_| {
                calls += 1;
                calls < 2
            },
        )
        .await;
        assert_eq!(outcome, RunOutcome::ReceiverGone);
        assert_eq!(calls, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn play_coalesces_what_fell_due_together() {
        // Everything of a long turn is due at once when the clock jumps an hour ahead.
        let script = MockScript::stress("t", MAX_TOKENS_PER_SEC, 50_000);
        let cancel = CancellationToken::new();
        let mut events = Vec::new();
        let outcome = {
            let run = super::play(script, cancel, |event| {
                events.push(event);
                true
            });
            tokio::pin!(run);
            // The first poll emits what is due at the start and parks on the next token.
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(run.as_mut().poll(&mut context).is_pending());
            tokio::time::advance(Duration::from_secs(3_600)).await;
            run.await
        };
        assert_eq!(outcome, RunOutcome::Completed);
        // TurnStarted, thought, ThoughtCompleted, answer (two deltas: the first token at once, the
        // rest after the jump), MessageCompleted, UsageUpdated, TurnEnded.
        assert!(events.len() <= 9, "{} events", events.len());
        assert!(matches!(events.last(), Some(GhostEvent::TurnEnded { .. })));
    }

    #[test]
    fn play_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let future = super::play(MockScript::default(), CancellationToken::new(), |_| true);
        assert_send(&future);
    }

    #[test]
    fn run_script_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let future = run_script(
            MockScript::default(),
            "s",
            FRAME,
            CancellationToken::new(),
            |_| true,
        );
        assert_send(&future);
    }
}
