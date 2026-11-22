//! Per-turn compaction, pure: deltas concatenated into pieces of at most 1 MiB, each tool merged into
//! one call (or one update for another turn's tool), the last plan and usage kept, every other row
//! verbatim, original sequence numbers kept; plus the full-text rows.
//!
//! | Rows of one turn | Kept as |
//! |---|---|
//! | user, agent and thought deltas, per id | concatenated in seq order into pieces of at most 1 MiB, each piece at the seq of its first delta |
//! | a tool call and its updates, for an id minted in this turn (`<turn_id>.` prefix) or called in it | one call with the final state at the call's seq (no call stored: a synthesized one at the first update's seq) |
//! | updates for an id minted elsewhere | one merged update (the fields last set) at the first such update's seq |
//! | plans, usage | the last of each |
//! | a background task's updates, per task | its first row as it is (with the prompt), and its latest state at its latest row, the summary kept |
//! | a background task's steps, per task and step | the latest state at the step's first seq |
//! | everything else, and rows that do not parse | unchanged, the payload copied verbatim |
//!
//! Kept rows keep their original seqs, a subset of the inputs', so `UNIQUE (chat_id, seq)` holds and
//! order is preserved; `TurnEnded`, the turn's highest tagged seq, is always kept, so `MAX(seq)` never
//! drops. [`apply`] runs it in one transaction, once per turn (`compacted_at`), and writes the
//! full-text rows. A turn whose payload exceeds 32 MiB is not merged, so memory stays bounded: only
//! its full-text rows are written, read row by row.

use std::collections::{HashMap, HashSet};

use o7_protocol::tasks::{BackgroundTask, TaskStep};
use o7_protocol::{Millis, O7Event, ToolCall, ToolKind, ToolStatus, ToolUpdate, Truncation};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::ids::new_id;

/// What a tool block reads as before its call arrived: a synthesized call, and the live reducers'
/// view of an update to an unknown tool.
pub(crate) const DEFAULT_TOOL_TITLE: &str = "Tool";

/// The sizes compaction works with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompactLimits {
    /// The most text one merged delta carries.
    pub(crate) piece_bytes: usize,
    /// A turn with more payload than this is not merged.
    pub(crate) merge_limit: u64,
    /// Recovery compacts a turn with at most this much payload itself; a larger one waits for the
    /// store's pass after startup ([`super::ChatStore`]'s leftovers), so that opening the app or a
    /// chat never waits on it.
    pub(crate) inline_limit: u64,
    /// The most text one full-text row carries.
    pub(crate) fts_bytes: usize,
}

impl CompactLimits {
    /// 1 MiB pieces, no merge above 32 MiB, 1 MiB per full-text row; recovery compacts up to 1 MiB
    /// itself.
    pub(crate) const STANDARD: CompactLimits = CompactLimits {
        piece_bytes: 1024 * 1024,
        merge_limit: 32 * 1024 * 1024,
        inline_limit: 1024 * 1024,
        fts_bytes: 1024 * 1024,
    };
}

/// One stored row of the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InRow {
    pub(crate) seq: u32,
    pub(crate) kind: String,
    pub(crate) payload: String,
    pub(crate) created_at: Millis,
}

/// A row compaction keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeptRow {
    pub(crate) seq: u32,
    pub(crate) kind: String,
    pub(crate) payload: String,
    pub(crate) created_at: Millis,
}

/// One `message_fts` row: a user or agent message's text, cut to the limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FtsRow {
    pub(crate) message_id: String,
    pub(crate) text: String,
}

/// What compacting one turn gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Compacted {
    /// In seq order.
    pub(crate) rows: Vec<KeptRow>,
    pub(crate) fts: Vec<FtsRow>,
}

/// Which stream a delta belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum DeltaKind {
    User,
    Message,
    Thought,
}

impl DeltaKind {
    fn of(event: &O7Event) -> Option<(DeltaKind, &str, &str)> {
        match event {
            O7Event::UserMessageDelta { message_id, text } => {
                Some((DeltaKind::User, message_id, text))
            }
            O7Event::MessageDelta { message_id, text } => {
                Some((DeltaKind::Message, message_id, text))
            }
            O7Event::ThoughtDelta { thought_id, text } => {
                Some((DeltaKind::Thought, thought_id, text))
            }
            _ => None,
        }
    }

    fn event(self, id: String, text: String) -> O7Event {
        match self {
            DeltaKind::User => O7Event::UserMessageDelta {
                message_id: id,
                text,
            },
            DeltaKind::Message => O7Event::MessageDelta {
                message_id: id,
                text,
            },
            DeltaKind::Thought => O7Event::ThoughtDelta {
                thought_id: id,
                text,
            },
        }
    }
}

/// One delta of a merged stream.
struct Delta {
    seq: u32,
    created_at: Millis,
    text: String,
}

/// A background task's rows in this turn: its first row as it is (the start, with the prompt), its
/// latest state at its latest row, and the summary from whichever row carried it.
struct TaskRows<'a> {
    first: &'a InRow,
    summary: Option<(String, Option<Truncation>)>,
    latest: Option<(u32, Millis, BackgroundTask)>,
}

/// A tool's events in this turn.
enum ToolEvent {
    Call(ToolCall),
    Update(ToolUpdate),
}

/// Compacts one turn's rows (all tagged `turn_id`).
pub(crate) fn compact(turn_id: &str, mut rows: Vec<InRow>, limits: &CompactLimits) -> Compacted {
    rows.sort_by_key(|row| row.seq);
    let parsed: Vec<Option<O7Event>> = rows
        .iter()
        .map(|row| serde_json::from_str::<O7Event>(&row.payload).ok())
        .collect();
    let prefix = format!("{turn_id}.");
    let called: HashSet<String> = parsed
        .iter()
        .filter_map(|event| match event {
            Some(O7Event::ToolCall(call)) => Some(call.tool_call_id.clone()),
            _ => None,
        })
        .collect();

    let mut kept = Vec::new();
    let mut fts = FtsCollector::new(limits.fts_bytes);
    let mut deltas: Vec<((DeltaKind, String), Vec<Delta>)> = Vec::new();
    let mut delta_index: HashMap<(DeltaKind, String), usize> = HashMap::new();
    // Tools of this turn: anchor (seq, created_at) and events in order.
    let mut own_tools: Vec<(String, u32, Millis, Vec<ToolEvent>)> = Vec::new();
    let mut own_index: HashMap<String, usize> = HashMap::new();
    let mut other_tools: Vec<(String, u32, Millis, Vec<ToolUpdate>)> = Vec::new();
    let mut other_index: HashMap<String, usize> = HashMap::new();
    let mut last_plan: Option<&InRow> = None;
    let mut last_usage: Option<&InRow> = None;
    let mut tasks: Vec<TaskRows<'_>> = Vec::new();
    let mut task_index: HashMap<String, usize> = HashMap::new();
    // Per step: its latest state at its first row, so steps keep their order.
    let mut steps: Vec<(u32, Millis, String, TaskStep)> = Vec::new();
    let mut step_index: HashMap<(String, String), usize> = HashMap::new();

    for (row, event) in rows.iter().zip(parsed) {
        let Some(event) = event else {
            kept.push(verbatim(row));
            continue;
        };
        fts.push(&event);
        if let Some((kind, id, text)) = DeltaKind::of(&event) {
            let key = (kind, id.to_owned());
            let index = *delta_index.entry(key.clone()).or_insert_with(|| {
                deltas.push((key, Vec::new()));
                deltas.len() - 1
            });
            deltas[index].1.push(Delta {
                seq: row.seq,
                created_at: row.created_at,
                text: text.to_owned(),
            });
            continue;
        }
        match event {
            O7Event::ToolCall(call) => {
                let id = call.tool_call_id.clone();
                let index = *own_index.entry(id.clone()).or_insert_with(|| {
                    own_tools.push((id, row.seq, row.created_at, Vec::new()));
                    own_tools.len() - 1
                });
                let tool = &mut own_tools[index];
                // The call anchors the block, even after an update that came first.
                if !tool
                    .3
                    .iter()
                    .any(|event| matches!(event, ToolEvent::Call(_)))
                {
                    tool.1 = row.seq;
                    tool.2 = row.created_at;
                }
                tool.3.push(ToolEvent::Call(call));
            }
            O7Event::ToolUpdate(update) => {
                let id = update.tool_call_id.clone();
                if called.contains(&id) || id.starts_with(&prefix) {
                    let index = *own_index.entry(id.clone()).or_insert_with(|| {
                        own_tools.push((id, row.seq, row.created_at, Vec::new()));
                        own_tools.len() - 1
                    });
                    own_tools[index].3.push(ToolEvent::Update(update));
                } else {
                    let index = *other_index.entry(id.clone()).or_insert_with(|| {
                        other_tools.push((id, row.seq, row.created_at, Vec::new()));
                        other_tools.len() - 1
                    });
                    other_tools[index].3.push(update);
                }
            }
            O7Event::PlanUpdated { .. } => last_plan = Some(row),
            O7Event::UsageUpdated(_) => last_usage = Some(row),
            O7Event::TaskUpdated(task) => {
                let index = *task_index.entry(task.task_id.clone()).or_insert_with(|| {
                    tasks.push(TaskRows {
                        first: row,
                        summary: None,
                        latest: None,
                    });
                    tasks.len() - 1
                });
                let rows = &mut tasks[index];
                if let Some(summary) = &task.summary {
                    rows.summary = Some((summary.clone(), task.summary_truncated.clone()));
                }
                if !std::ptr::eq(rows.first, row) {
                    rows.latest = Some((row.seq, row.created_at, task));
                }
            }
            O7Event::TaskStep { task_id, step } => {
                let key = (task_id.clone(), step.step_id.clone());
                match step_index.get(&key) {
                    Some(&index) => steps[index].3 = step,
                    None => {
                        step_index.insert(key, steps.len());
                        steps.push((row.seq, row.created_at, task_id, step));
                    }
                }
            }
            _ => kept.push(verbatim(row)),
        }
    }

    for ((kind, id), parts) in deltas {
        for (seq, created_at, text) in pieces(parts, limits.piece_bytes) {
            kept.push(encoded(seq, created_at, &kind.event(id.clone(), text)));
        }
    }
    for (id, seq, created_at, events) in own_tools {
        let mut state: Option<ToolCall> = None;
        for event in events {
            state = Some(match (event, state) {
                (ToolEvent::Call(call), _) => call,
                (ToolEvent::Update(update), state) => {
                    apply_update(state.unwrap_or_else(|| default_tool(&id)), update)
                }
            });
        }
        if let Some(call) = state {
            kept.push(encoded(seq, created_at, &O7Event::ToolCall(call)));
        }
    }
    for (id, seq, created_at, updates) in other_tools {
        let merged = updates.into_iter().fold(empty_update(&id), merge_update);
        kept.push(encoded(seq, created_at, &O7Event::ToolUpdate(merged)));
    }
    for task in tasks {
        kept.push(verbatim(task.first));
        if let Some((seq, created_at, mut latest)) = task.latest {
            // The first row carries the prompt; the latest says everything else.
            latest.prompt = None;
            if latest.summary.is_none()
                && let Some((summary, truncated)) = task.summary
            {
                latest.summary = Some(summary);
                latest.summary_truncated = truncated;
            }
            kept.push(encoded(seq, created_at, &O7Event::TaskUpdated(latest)));
        }
    }
    for (seq, created_at, task_id, step) in steps {
        kept.push(encoded(
            seq,
            created_at,
            &O7Event::TaskStep { task_id, step },
        ));
    }
    kept.extend(last_plan.map(verbatim));
    kept.extend(last_usage.map(verbatim));
    kept.sort_by_key(|row| row.seq);
    Compacted {
        rows: kept,
        fts: fts.finish(),
    }
}

fn verbatim(row: &InRow) -> KeptRow {
    KeptRow {
        seq: row.seq,
        kind: row.kind.clone(),
        payload: row.payload.clone(),
        created_at: row.created_at,
    }
}

fn encoded(seq: u32, created_at: Millis, event: &O7Event) -> KeptRow {
    KeptRow {
        seq,
        kind: event.kind().to_owned(),
        // These types always serialize; an empty object would only ever be skipped on reading.
        payload: serde_json::to_string(event).unwrap_or_else(|_| "{}".to_owned()),
        created_at,
    }
}

/// Concatenates one stream's deltas into pieces of at most `limit` bytes, each starting at a delta
/// (at that delta's seq). A single delta longer than the limit stays one piece.
fn pieces(parts: Vec<Delta>, limit: usize) -> Vec<(u32, Millis, String)> {
    let mut out = Vec::new();
    let mut current: Option<(u32, Millis, String)> = None;
    for part in parts {
        match &mut current {
            Some((_, _, text)) if text.len().saturating_add(part.text.len()) <= limit => {
                text.push_str(&part.text);
            }
            _ => {
                if let Some(done) = current.take() {
                    out.push(done);
                }
                current = Some((part.seq, part.created_at, part.text));
            }
        }
    }
    out.extend(current);
    out
}

/// What an update to a tool nobody announced starts from.
pub(crate) fn default_tool(id: &str) -> ToolCall {
    ToolCall {
        tool_call_id: id.to_owned(),
        kind: ToolKind::Other,
        title: DEFAULT_TOOL_TITLE.to_owned(),
        status: ToolStatus::Pending,
        locations: Vec::new(),
        raw_input: None,
        tool_name: None,
        content: Vec::new(),
        truncated: None,
    }
}

/// `update` applied to `call`: every `Some` field replaces; `content: Some([])` clears.
pub(crate) fn apply_update(mut call: ToolCall, update: ToolUpdate) -> ToolCall {
    if let Some(status) = update.status {
        call.status = status;
    }
    if let Some(title) = update.title {
        call.title = title;
    }
    if let Some(kind) = update.kind {
        call.kind = kind;
    }
    if let Some(locations) = update.locations {
        call.locations = locations;
    }
    if let Some(raw_input) = update.raw_input {
        call.raw_input = Some(raw_input);
    }
    if let Some(tool_name) = update.tool_name {
        call.tool_name = Some(tool_name);
    }
    if let Some(content) = update.content {
        call.content = content;
    }
    if let Some(truncated) = update.truncated {
        call.truncated = Some(truncated);
    }
    call
}

fn empty_update(id: &str) -> ToolUpdate {
    ToolUpdate {
        tool_call_id: id.to_owned(),
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

/// The fields last set across two updates.
fn merge_update(into: ToolUpdate, next: ToolUpdate) -> ToolUpdate {
    ToolUpdate {
        tool_call_id: into.tool_call_id,
        status: next.status.or(into.status),
        title: next.title.or(into.title),
        kind: next.kind.or(into.kind),
        locations: next.locations.or(into.locations),
        raw_input: next.raw_input.or(into.raw_input),
        tool_name: next.tool_name.or(into.tool_name),
        content: next.content.or(into.content),
        truncated: next.truncated.or(into.truncated),
    }
}

/// Collects one full-text row per user and agent message, each cut to the limit, in the order the
/// messages started. A message is written once its `MessageCompleted` arrives (or at the end), so
/// streaming a huge turn holds only the open messages.
pub(crate) struct FtsCollector {
    limit: usize,
    open: Vec<(String, String)>,
    done: Vec<FtsRow>,
}

impl FtsCollector {
    pub(crate) fn new(limit: usize) -> FtsCollector {
        FtsCollector {
            limit,
            open: Vec::new(),
            done: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, event: &O7Event) {
        match event {
            O7Event::UserMessageDelta { message_id, text }
            | O7Event::MessageDelta { message_id, text } => {
                let limit = self.limit;
                let entry = match self.open.iter_mut().position(|(id, _)| id == message_id) {
                    Some(index) => &mut self.open[index].1,
                    None => {
                        self.open.push((message_id.clone(), String::new()));
                        let last = self.open.len() - 1;
                        &mut self.open[last].1
                    }
                };
                let room = limit.saturating_sub(entry.len());
                if room > 0 {
                    entry.push_str(prefix_within(text, room));
                }
            }
            O7Event::MessageCompleted { message_id } => {
                if let Some(index) = self.open.iter().position(|(id, _)| id == message_id) {
                    let (message_id, text) = self.open.remove(index);
                    self.done.push(FtsRow { message_id, text });
                }
            }
            _ => {}
        }
    }

    pub(crate) fn finish(mut self) -> Vec<FtsRow> {
        self.done.extend(
            self.open
                .into_iter()
                .map(|(message_id, text)| FtsRow { message_id, text }),
        );
        self.done
    }
}

/// The longest prefix of `text` of at most `bytes` bytes that ends on a character boundary.
pub(crate) fn prefix_within(text: &str, bytes: usize) -> &str {
    if text.len() <= bytes {
        return text;
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Compacts one ended turn in the caller's transaction, unless it is running or was compacted
/// already. Returns whether it compacted the turn.
pub(crate) fn apply(
    conn: &Connection,
    chat_id: &str,
    turn_id: &str,
    device_id: &str,
    limits: &CompactLimits,
    now: Millis,
) -> rusqlite::Result<bool> {
    if !waits(conn, chat_id, turn_id)? {
        return Ok(false);
    }
    let fts = if turn_bytes(conn, turn_id)? > limits.merge_limit {
        // Too big to merge in memory: only the full-text rows, read row by row.
        collect_text(conn, turn_id, limits)?
    } else {
        let mut stmt = conn.prepare_cached(
            "SELECT seq, kind, payload, created_at FROM events WHERE turn_id = ?1 ORDER BY seq",
        )?;
        let rows: Vec<InRow> = stmt
            .query_map(params![turn_id], |row| {
                Ok(InRow {
                    seq: row.get(0)?,
                    kind: row.get(1)?,
                    payload: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let compacted = compact(turn_id, rows, limits);
        conn.prepare_cached("DELETE FROM events WHERE turn_id = ?1")?
            .execute(params![turn_id])?;
        let mut insert = conn.prepare_cached(
            "INSERT INTO events (id, chat_id, turn_id, seq, kind, payload, created_at, device_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for row in &compacted.rows {
            insert.execute(params![
                new_id(),
                chat_id,
                turn_id,
                row.seq,
                row.kind,
                row.payload,
                row.created_at,
                device_id,
            ])?;
        }
        compacted.fts
    };
    finish(conn, chat_id, turn_id, &fts, now)?;
    Ok(true)
}

/// Whether `turn_id` ended and is not compacted yet.
fn waits(conn: &Connection, chat_id: &str, turn_id: &str) -> rusqlite::Result<bool> {
    let state: Option<(String, Option<i64>)> = conn
        .prepare_cached("SELECT status, compacted_at FROM turns WHERE id = ?1 AND chat_id = ?2")?
        .query_row(params![turn_id, chat_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    Ok(matches!(state, Some((status, None)) if status != "running"))
}

/// Writes a turn's full-text rows and marks it compacted.
fn finish(
    conn: &Connection,
    chat_id: &str,
    turn_id: &str,
    fts: &[FtsRow],
    now: Millis,
) -> rusqlite::Result<()> {
    let mut insert_fts = conn.prepare_cached(
        "INSERT INTO message_fts (chat_id, message_id, text) VALUES (?1, ?2, ?3)",
    )?;
    for row in fts.iter().filter(|row| !row.text.is_empty()) {
        insert_fts.execute(params![chat_id, row.message_id, row.text])?;
    }
    conn.prepare_cached("UPDATE turns SET compacted_at = ?2, last_event_seq = MAX(COALESCE(last_event_seq,0),COALESCE((SELECT MAX(seq) FROM events WHERE turn_id=?1),0)) WHERE id = ?1")?
        .execute(params![turn_id, now])?;
    Ok(())
}

/// How much payload a turn's events hold. SQLite answers `octet_length` from each row's header,
/// so no payload is read.
pub(crate) fn turn_bytes(conn: &Connection, turn_id: &str) -> rusqlite::Result<u64> {
    let size: i64 = conn
        .prepare_cached(
            "SELECT COALESCE(SUM(octet_length(payload)), 0) FROM events WHERE turn_id = ?1",
        )?
        .query_row(params![turn_id], |row| row.get(0))?;
    Ok(u64::try_from(size).unwrap_or(0))
}

/// The full-text rows of a turn too large to merge, read row by row. It only reads, so it also
/// runs on a connection of its own, off the writer ([`save_text`] then stores what it found).
pub(crate) fn collect_text(
    conn: &Connection,
    turn_id: &str,
    limits: &CompactLimits,
) -> rusqlite::Result<Vec<FtsRow>> {
    let mut collector = FtsCollector::new(limits.fts_bytes);
    let mut stmt =
        conn.prepare_cached("SELECT payload FROM events WHERE turn_id = ?1 ORDER BY seq")?;
    let mut rows = stmt.query(params![turn_id])?;
    while let Some(row) = rows.next()? {
        let payload: String = row.get(0)?;
        if let Ok(event) = serde_json::from_str::<O7Event>(&payload) {
            collector.push(&event);
        }
    }
    Ok(collector.finish())
}

/// [`collect_text`] on a read-only connection of its own to the profile's database, off the
/// writer, as line edits are counted.
pub(crate) fn collect_text_at(
    db: &std::path::Path,
    turn_id: &str,
    limits: &CompactLimits,
) -> rusqlite::Result<Vec<FtsRow>> {
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    collect_text(&conn, turn_id, limits)
}

/// Stores the full-text rows [`collect_text`] read off the writer and marks the turn compacted,
/// in the caller's transaction. The turn is checked again here: one that was compacted meanwhile,
/// or is gone, is left alone. Returns whether it compacted the turn.
pub(crate) fn save_text(
    conn: &Connection,
    chat_id: &str,
    turn_id: &str,
    fts: &[FtsRow],
    now: Millis,
) -> rusqlite::Result<bool> {
    if !waits(conn, chat_id, turn_id)? {
        return Ok(false);
    }
    finish(conn, chat_id, turn_id, fts, now)?;
    Ok(true)
}

/// An ended turn that is not compacted yet, and how much payload it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Leftover {
    pub(crate) chat_id: String,
    pub(crate) turn_id: String,
    pub(crate) bytes: u64,
}

/// Every ended turn not compacted yet, oldest first.
pub(crate) fn leftovers(conn: &Connection) -> rusqlite::Result<Vec<Leftover>> {
    let turns: Vec<(String, String)> = conn
        .prepare_cached(
            "SELECT chat_id, id FROM turns WHERE status <> 'running' AND compacted_at IS NULL \
             ORDER BY started_at, id",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    turns
        .into_iter()
        .map(|(chat_id, turn_id)| {
            let bytes = turn_bytes(conn, &turn_id)?;
            Ok(Leftover {
                chat_id,
                turn_id,
                bytes,
            })
        })
        .collect()
}

/// A test-only reducer over seq-ordered events: what the transcript shows. Messages and thoughts by
/// id, each tool's final state, the last plan and usage, permissions and questions with their
/// resolutions, the other events in order, and the order items first appeared in.
#[cfg(test)]
pub(crate) mod reducer {
    use std::collections::BTreeMap;

    use o7_protocol::{
        O7Event, PermissionDecision, PermissionRequest, PlanEntry, ResolvedBy, ToolCall, UiAnswer,
        UiRequest, UsageInfo,
    };

    use super::{DeltaKind, apply_update, default_tool};

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub(crate) enum Item {
        Text(DeltaKind, String),
        Tool(String),
        Request(String),
        Other(usize),
    }

    /// A permission card and its resolution, if any.
    pub(crate) type Card = (PermissionRequest, Option<(PermissionDecision, ResolvedBy)>);

    /// A question card and its resolution, if any.
    pub(crate) type Question = (UiRequest, Option<(Option<UiAnswer>, ResolvedBy)>);

    #[derive(Debug, Default, PartialEq)]
    pub(crate) struct Transcript {
        pub(crate) texts: BTreeMap<(DeltaKind, String), String>,
        pub(crate) completed: BTreeMap<String, Option<u32>>,
        pub(crate) tools: BTreeMap<String, ToolCall>,
        pub(crate) plan: Option<Vec<PlanEntry>>,
        pub(crate) usage: Option<UsageInfo>,
        pub(crate) permissions: BTreeMap<String, Card>,
        pub(crate) questions: BTreeMap<String, Question>,
        pub(crate) others: Vec<O7Event>,
        pub(crate) order: Vec<Item>,
    }

    impl Transcript {
        fn first_seen(&mut self, item: Item) {
            if !self.order.contains(&item) {
                self.order.push(item);
            }
        }

        pub(crate) fn apply(&mut self, event: &O7Event) {
            if let Some((kind, id, text)) = DeltaKind::of(event) {
                self.first_seen(Item::Text(kind, id.to_owned()));
                self.texts
                    .entry((kind, id.to_owned()))
                    .or_default()
                    .push_str(text);
                return;
            }
            match event {
                O7Event::MessageCompleted { message_id } => {
                    self.completed.insert(message_id.clone(), None);
                }
                O7Event::ThoughtCompleted {
                    thought_id,
                    duration_ms,
                } => {
                    self.completed.insert(thought_id.clone(), *duration_ms);
                }
                O7Event::ToolCall(call) => {
                    self.first_seen(Item::Tool(call.tool_call_id.clone()));
                    self.tools.insert(call.tool_call_id.clone(), call.clone());
                }
                O7Event::ToolUpdate(update) => {
                    self.first_seen(Item::Tool(update.tool_call_id.clone()));
                    let current = self
                        .tools
                        .remove(&update.tool_call_id)
                        .unwrap_or_else(|| default_tool(&update.tool_call_id));
                    self.tools.insert(
                        update.tool_call_id.clone(),
                        apply_update(current, update.clone()),
                    );
                }
                O7Event::PlanUpdated { entries } => self.plan = Some(entries.clone()),
                O7Event::UsageUpdated(usage) => self.usage = Some(usage.clone()),
                O7Event::PermissionRequested(request) => {
                    self.first_seen(Item::Request(request.request_id.clone()));
                    self.permissions
                        .insert(request.request_id.clone(), (request.clone(), None));
                }
                O7Event::PermissionResolved {
                    request_id,
                    decision,
                    by,
                } => {
                    if let Some(entry) = self.permissions.get_mut(request_id) {
                        entry.1 = Some((decision.clone(), *by));
                    }
                }
                O7Event::UiRequest(request) => {
                    self.first_seen(Item::Request(request.request_id.clone()));
                    self.questions
                        .insert(request.request_id.clone(), (request.clone(), None));
                }
                O7Event::UiResolved {
                    request_id,
                    answer,
                    by,
                } => {
                    if let Some(entry) = self.questions.get_mut(request_id) {
                        entry.1 = Some((answer.clone(), *by));
                    }
                }
                other => {
                    let index = self.others.len();
                    self.first_seen(Item::Other(index));
                    self.others.push(other.clone());
                }
            }
        }
    }

    pub(crate) fn reduce<'a>(events: impl IntoIterator<Item = &'a O7Event>) -> Transcript {
        let mut transcript = Transcript::default();
        for event in events {
            transcript.apply(event);
        }
        transcript
    }
}

#[cfg(test)]
mod tests {
    use o7_protocol::{
        NoticeLevel, PermissionDecision, PermissionOption, PermissionOptionKind, PermissionRequest,
        PlanEntry, PlanStatus, ResolvedBy, StopReason, ToolContent, ToolLocation, UiPrompt,
        UiRequest, UsageInfo,
    };

    use super::reducer::reduce;
    use super::*;
    use crate::chatstore::test_support::{TestDb, event_row};

    fn row(seq: u32, event: &O7Event) -> InRow {
        InRow {
            seq,
            kind: event.kind().to_owned(),
            payload: serde_json::to_string(event).expect("serialize"),
            created_at: i64::from(seq) * 10,
        }
    }

    fn events_of(rows: &[KeptRow]) -> Vec<O7Event> {
        rows.iter()
            .filter_map(|row| serde_json::from_str(&row.payload).ok())
            .collect()
    }

    fn delta(id: &str, text: &str) -> O7Event {
        O7Event::MessageDelta {
            message_id: id.into(),
            text: text.into(),
        }
    }

    fn update(id: &str) -> ToolUpdate {
        empty_update(id)
    }

    fn call(id: &str, title: &str) -> ToolCall {
        ToolCall {
            title: title.into(),
            ..default_tool(id)
        }
    }

    #[test]
    fn compaction_concatenates_deltas_per_id_at_the_first_seq() {
        let rows = vec![
            row(
                0,
                &O7Event::TurnStarted {
                    turn_id: "t".into(),
                    prompt_id: None,
                },
            ),
            row(
                1,
                &O7Event::UserMessageDelta {
                    message_id: "t.u".into(),
                    text: "hi ".into(),
                },
            ),
            row(
                2,
                &O7Event::UserMessageDelta {
                    message_id: "t.u".into(),
                    text: "there".into(),
                },
            ),
            row(3, &delta("t.m1", "Hel")),
            row(
                4,
                &O7Event::ThoughtDelta {
                    thought_id: "t.t1".into(),
                    text: "hmm".into(),
                },
            ),
            row(5, &delta("t.m1", "lo")),
            row(
                6,
                &O7Event::MessageCompleted {
                    message_id: "t.m1".into(),
                },
            ),
        ];
        let compacted = compact("t", rows, &CompactLimits::STANDARD);
        let seqs: Vec<u32> = compacted.rows.iter().map(|row| row.seq).collect();
        assert_eq!(seqs, [0, 1, 3, 4, 6]);
        assert_eq!(
            events_of(&compacted.rows)[2],
            delta("t.m1", "Hello"),
            "the message's text at its first delta"
        );
        assert_eq!(
            compacted.fts,
            [
                FtsRow {
                    message_id: "t.m1".into(),
                    text: "Hello".into()
                },
                FtsRow {
                    message_id: "t.u".into(),
                    text: "hi there".into()
                },
            ],
            "user and agent messages, no thoughts"
        );
    }

    #[test]
    fn compaction_splits_long_streams_into_pieces() {
        let limits = CompactLimits {
            piece_bytes: 10,
            ..CompactLimits::STANDARD
        };
        let rows: Vec<InRow> = (0..7).map(|i| row(i, &delta("t.m1", "abcd"))).collect();
        let compacted = compact("t", rows, &limits);
        let pieces: Vec<(u32, O7Event)> = compacted
            .rows
            .iter()
            .map(|row| (row.seq, serde_json::from_str(&row.payload).expect("parse")))
            .collect();
        assert_eq!(
            pieces,
            [
                (0, delta("t.m1", "abcdabcd")),
                (2, delta("t.m1", "abcdabcd")),
                (4, delta("t.m1", "abcdabcd")),
                (6, delta("t.m1", "abcd")),
            ]
        );
    }

    #[test]
    fn compaction_keeps_a_tasks_start_and_latest_state_and_each_steps_latest() {
        use o7_protocol::tasks::{TaskKind, TaskStatus, TaskStepKind};
        let task = |status: TaskStatus, activity: &str| BackgroundTask {
            task_id: "a".into(),
            kind: TaskKind::Agent,
            title: "Explore".into(),
            agent_type: None,
            tool_call_id: None,
            parent_task_id: None,
            backgrounded: false,
            status,
            activity: Some(activity.into()),
            tool_uses: None,
            tokens: None,
            duration_ms: None,
            started_at: Some(1),
            ended_at: None,
            prompt: None,
            summary: None,
            summary_truncated: None,
        };
        let start = BackgroundTask {
            prompt: Some("Map it".into()),
            ..task(TaskStatus::Running, "Starting")
        };
        let done = BackgroundTask {
            summary: Some("Found it".into()),
            ..task(TaskStatus::Completed, "Reading b.rs")
        };
        let step = |status: ToolStatus| TaskStep {
            step_id: "s1".into(),
            kind: TaskStepKind::Tool,
            title: "Read a.rs".into(),
            tool_kind: Some(ToolKind::Read),
            status: Some(status),
            text: None,
            truncated: None,
        };
        let rows = vec![
            row(1, &O7Event::TaskUpdated(start.clone())),
            row(
                2,
                &O7Event::TaskStep {
                    task_id: "a".into(),
                    step: step(ToolStatus::InProgress),
                },
            ),
            row(
                3,
                &O7Event::TaskUpdated(task(TaskStatus::Running, "Reading a.rs")),
            ),
            row(
                4,
                &O7Event::TaskStep {
                    task_id: "a".into(),
                    step: step(ToolStatus::Completed),
                },
            ),
            row(5, &O7Event::TaskUpdated(done.clone())),
            row(
                6,
                &O7Event::TaskUpdated(task(TaskStatus::Completed, "Reading b.rs")),
            ),
        ];
        let compacted = compact("t", rows, &CompactLimits::STANDARD);
        let kept: Vec<(u32, O7Event)> = compacted
            .rows
            .iter()
            .map(|row| (row.seq, serde_json::from_str(&row.payload).expect("parse")))
            .collect();
        assert_eq!(
            kept,
            [
                (1, O7Event::TaskUpdated(start)),
                (
                    2,
                    O7Event::TaskStep {
                        task_id: "a".into(),
                        step: step(ToolStatus::Completed)
                    }
                ),
                (6, O7Event::TaskUpdated(done)),
            ]
        );
    }

    #[test]
    fn compaction_merges_own_tools_into_one_call_and_others_into_one_update() {
        let mut first = update("t.a");
        first.status = Some(ToolStatus::InProgress);
        first.content = Some(vec![ToolContent::Text {
            text: "partial".into(),
            truncated: None,
        }]);
        let mut second = update("t.a");
        second.status = Some(ToolStatus::Completed);
        second.title = Some("Ran it".into());
        let mut orphan = update("t.b");
        orphan.kind = Some(ToolKind::Read);
        let mut earlier_a = update("prev.x");
        earlier_a.status = Some(ToolStatus::InProgress);
        earlier_a.title = Some("still going".into());
        let mut earlier_b = update("prev.x");
        earlier_b.status = Some(ToolStatus::Completed);
        let rows = vec![
            row(1, &O7Event::ToolCall(call("t.a", "Run"))),
            row(2, &O7Event::ToolUpdate(earlier_a)),
            row(3, &O7Event::ToolUpdate(first)),
            row(4, &O7Event::ToolUpdate(orphan)),
            row(5, &O7Event::ToolUpdate(second)),
            row(6, &O7Event::ToolUpdate(earlier_b)),
        ];
        let compacted = compact("t", rows, &CompactLimits::STANDARD);
        let kept: Vec<(u32, O7Event)> = compacted
            .rows
            .iter()
            .map(|row| (row.seq, serde_json::from_str(&row.payload).expect("parse")))
            .collect();
        let mut final_a = call("t.a", "Ran it");
        final_a.status = ToolStatus::Completed;
        final_a.content = vec![ToolContent::Text {
            text: "partial".into(),
            truncated: None,
        }];
        let mut synthesized_b = default_tool("t.b");
        synthesized_b.kind = ToolKind::Read;
        let mut merged = update("prev.x");
        merged.status = Some(ToolStatus::Completed);
        merged.title = Some("still going".into());
        assert_eq!(
            kept,
            [
                (1, O7Event::ToolCall(final_a)),
                (2, O7Event::ToolUpdate(merged)),
                (4, O7Event::ToolCall(synthesized_b)),
            ]
        );
    }

    #[test]
    fn compaction_keeps_the_last_plan_and_usage_and_everything_else_verbatim() {
        let plan = |text: &str| O7Event::PlanUpdated {
            entries: vec![PlanEntry {
                content: text.into(),
                status: PlanStatus::Pending,
                priority: None,
            }],
        };
        let usage = |used| {
            O7Event::UsageUpdated(UsageInfo {
                context_used: Some(used),
                context_size: None,
                cost: None,
            })
        };
        let notice = O7Event::Notice {
            level: NoticeLevel::Warning,
            message: "careful".into(),
        };
        let mut unparsable = row(5, &notice);
        unparsable.kind = "hologram".into();
        unparsable.payload = r#"{"type":"hologram","x":1}"#.into();
        let ended = O7Event::TurnEnded {
            turn_id: "t".into(),
            stop_reason: StopReason::EndTurn,
            usage: None,
            duration_ms: Some(5),
            recovered: false,
        };
        let rows = vec![
            row(1, &plan("a")),
            row(2, &usage(1)),
            row(3, &notice),
            row(4, &plan("b")),
            unparsable.clone(),
            row(6, &usage(2)),
            row(7, &ended),
        ];
        let compacted = compact("t", rows.clone(), &CompactLimits::STANDARD);
        let seqs: Vec<u32> = compacted.rows.iter().map(|row| row.seq).collect();
        assert_eq!(seqs, [3, 4, 5, 6, 7]);
        assert_eq!(
            compacted.rows[2].payload, unparsable.payload,
            "copied verbatim"
        );
        assert_eq!(compacted.rows[2].kind, "hologram");
        assert_eq!(
            compacted.rows[4].payload, rows[6].payload,
            "TurnEnded verbatim"
        );
    }

    /// S13 and A42 through SQL: compaction rewrites the turn's rows once, keeps rows outside the
    /// turn, writes the full-text rows, and an update to an earlier turn's tool stays an update.
    #[test]
    fn compaction_applies_once_and_writes_fts_rows() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        db.turn(&chat, "t1", 1, 0);
        db.turn(&chat, "t2", 2, 4);
        let mut late = update("t1.tool");
        late.status = Some(ToolStatus::Completed);
        db.append(
            &chat,
            &[
                event_row(
                    0,
                    Some("t1"),
                    &O7Event::TurnStarted {
                        turn_id: "t1".into(),
                        prompt_id: None,
                    },
                ),
                event_row(1, Some("t1"), &O7Event::ToolCall(call("t1.tool", "Build"))),
                event_row(2, Some("t1"), &delta("t1.m1", "Building ")),
                event_row(3, Some("t1"), &delta("t1.m1", "now")),
                event_row(
                    4,
                    Some("t2"),
                    &O7Event::TurnStarted {
                        turn_id: "t2".into(),
                        prompt_id: None,
                    },
                ),
                event_row(
                    5,
                    None,
                    &O7Event::Notice {
                        level: NoticeLevel::Info,
                        message: "late".into(),
                    },
                ),
                event_row(6, Some("t2"), &O7Event::ToolUpdate(late.clone())),
                event_row(7, Some("t2"), &O7Event::ToolUpdate(update("t1.tool"))),
            ],
        );
        db.conn()
            .execute("UPDATE turns SET status = 'completed'", [])
            .expect("end turns");
        let conn = db.conn();
        assert!(apply(conn, &chat, "t2", "dev", &CompactLimits::STANDARD, 99).expect("compact"));
        assert!(
            !apply(conn, &chat, "t2", "dev", &CompactLimits::STANDARD, 99).expect("again"),
            "once"
        );
        assert!(apply(conn, &chat, "t1", "dev", &CompactLimits::STANDARD, 99).expect("t1"));
        let stored = db.events(&chat);
        let seqs: Vec<u32> = stored.iter().map(|(seq, _, _)| *seq).collect();
        assert_eq!(seqs, [0, 1, 2, 4, 5, 6]);
        assert_eq!(stored[2].2, delta("t1.m1", "Building now"));
        assert_eq!(
            stored[4],
            (
                5,
                None,
                O7Event::Notice {
                    level: NoticeLevel::Info,
                    message: "late".into()
                }
            )
        );
        assert_eq!(
            stored[5].2,
            O7Event::ToolUpdate(late),
            "never a synthesized call"
        );
        let fts: Vec<(String, String)> = conn
            .prepare("SELECT message_id, text FROM message_fts WHERE chat_id = ?1")
            .expect("prepare")
            .query_map(params![chat], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(fts, [("t1.m1".to_owned(), "Building now".to_owned())]);
        let running: i64 = conn
            .query_row(
                "SELECT count(*) FROM turns WHERE compacted_at IS NULL",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(running, 0);
    }

    #[test]
    fn running_turns_are_never_compacted() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        db.turn(&chat, "t1", 1, 0);
        db.append(
            &chat,
            &[
                event_row(0, Some("t1"), &delta("t1.m1", "a")),
                event_row(1, Some("t1"), &delta("t1.m1", "b")),
            ],
        );
        assert!(
            !apply(db.conn(), &chat, "t1", "dev", &CompactLimits::STANDARD, 1).expect("compact")
        );
        assert_eq!(db.events(&chat).len(), 2);
    }

    /// S14 with a scaled-down limit: a turn over the merge limit keeps every row and gets only its
    /// full-text rows, each capped.
    #[test]
    fn huge_turn_skips_merge() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        db.turn(&chat, "t1", 1, 0);
        let rows: Vec<_> = (0..40)
            .map(|i| event_row(i, Some("t1"), &delta("t1.m1", &"z".repeat(100))))
            .collect();
        db.append(&chat, &rows);
        db.conn()
            .execute("UPDATE turns SET status = 'completed'", [])
            .expect("end");
        let limits = CompactLimits {
            piece_bytes: 1024,
            merge_limit: 2_000,
            inline_limit: 2_000,
            fts_bytes: 250,
        };
        assert!(apply(db.conn(), &chat, "t1", "dev", &limits, 5).expect("compact"));
        assert_eq!(db.events(&chat).len(), 40, "not merged");
        let text: String = db
            .conn()
            .query_row(
                "SELECT text FROM message_fts WHERE message_id = 't1.m1'",
                [],
                |row| row.get(0),
            )
            .expect("fts");
        assert_eq!(text.len(), 250);
        let compacted: Option<i64> = db
            .conn()
            .query_row(
                "SELECT compacted_at FROM turns WHERE id = 't1'",
                [],
                |row| row.get(0),
            )
            .expect("turn");
        assert_eq!(compacted, Some(5));
    }

    #[test]
    fn fts_text_is_capped_on_a_character_boundary() {
        let mut collector = FtsCollector::new(5);
        collector.push(&delta("m", "ab"));
        collector.push(&delta("m", "c😀d"));
        assert_eq!(
            collector.finish(),
            [FtsRow {
                message_id: "m".into(),
                text: "abc".into()
            }]
        );
    }

    /// A deterministic pseudo-random source (xorshift), so failures reproduce from the seed.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }

        fn chance(&mut self, percent: u64) -> bool {
            self.below(100) < percent
        }

        fn text(&mut self) -> String {
            const WORDS: [&str; 8] = ["alpha ", "beta", " gamma", "δέλτα ", "😀", "\n", "e", "  "];
            (0..1 + self.below(4))
                .map(|_| WORDS[self.below(WORDS.len() as u64) as usize])
                .collect()
        }
    }

    /// One generated chat: every event in seq order with its turn tag.
    struct Timeline {
        events: Vec<(u32, Option<String>, O7Event)>,
        turns: Vec<String>,
    }

    fn random_update(rng: &mut Rng, id: &str) -> ToolUpdate {
        let mut update = update(id);
        if rng.chance(50) {
            update.status = Some(
                [
                    ToolStatus::InProgress,
                    ToolStatus::Completed,
                    ToolStatus::Failed,
                ][rng.below(3) as usize],
            );
        }
        if rng.chance(30) {
            update.title = Some(rng.text());
        }
        if rng.chance(20) {
            update.kind = Some(ToolKind::Execute);
        }
        if rng.chance(20) {
            update.locations = Some(vec![ToolLocation {
                path: rng.text(),
                line: Some(rng.below(9) as u32),
            }]);
        }
        if rng.chance(20) {
            update.raw_input = Some(serde_json::json!({ "n": rng.below(100) }));
        }
        if rng.chance(15) {
            update.tool_name = Some(rng.text());
        }
        if rng.chance(40) {
            update.content = Some(if rng.chance(20) {
                Vec::new()
            } else {
                vec![ToolContent::Terminal {
                    output: rng.text(),
                    exit_code: Some(0),
                    truncated: None,
                }]
            });
        }
        update
    }

    fn generate(seed: u64) -> Timeline {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut events = Vec::new();
        let mut turns = Vec::new();
        let mut seq = 0u32;
        let mut push = |events: &mut Vec<(u32, Option<String>, O7Event)>,
                        tag: Option<&str>,
                        event: O7Event| {
            events.push((seq, tag.map(str::to_owned), event));
            seq += 1;
        };
        let mut known_tools: Vec<String> = Vec::new();
        let mut request_counter = 0;
        let spawn = "spawn1";
        for t in 0..1 + rng.below(5) {
            let turn = format!("turn{t}");
            turns.push(turn.clone());
            push(
                &mut events,
                Some(&turn),
                O7Event::TurnStarted {
                    turn_id: turn.clone(),
                    prompt_id: None,
                },
            );
            // Untagged events only directly after TurnStarted (rule 8: before the driver saw the prompt).
            for _ in 0..rng.below(2) {
                if let Some(id) = known_tools
                    .get(rng.below(known_tools.len() as u64 + 1) as usize)
                    .cloned()
                {
                    push(
                        &mut events,
                        None,
                        O7Event::ToolUpdate(random_update(&mut rng, &id)),
                    );
                }
            }
            push(
                &mut events,
                Some(&turn),
                O7Event::UserMessageDelta {
                    message_id: format!("{turn}.u"),
                    text: rng.text(),
                },
            );
            let mut messages = 0;
            let mut thoughts = 0;
            let mut tools = 0;
            for _ in 0..rng.below(30) {
                let event = match rng.below(12) {
                    0..=2 => {
                        if messages == 0 || rng.chance(20) {
                            messages += 1;
                        }
                        delta(&format!("{turn}.m{messages}"), &rng.text())
                    }
                    3 => {
                        if thoughts == 0 || rng.chance(30) {
                            thoughts += 1;
                        }
                        O7Event::ThoughtDelta {
                            thought_id: format!("{turn}.t{thoughts}"),
                            text: rng.text(),
                        }
                    }
                    4 => {
                        tools += 1;
                        let id = format!("{turn}.tool{tools}");
                        known_tools.push(id.clone());
                        O7Event::ToolCall(ToolCall {
                            status: ToolStatus::Pending,
                            ..call(&id, &rng.text())
                        })
                    }
                    5 | 6 => {
                        // An update to this turn's tool, an earlier turn's, a background one, or an
                        // orphan minted in this turn.
                        let id = if rng.chance(15) {
                            tools += 1;
                            format!("{turn}.tool{tools}")
                        } else {
                            match known_tools.get(rng.below(known_tools.len() as u64 + 1) as usize)
                            {
                                Some(id) => id.clone(),
                                None => continue,
                            }
                        };
                        if !known_tools.contains(&id) {
                            known_tools.push(id.clone());
                        }
                        O7Event::ToolUpdate(random_update(&mut rng, &id))
                    }
                    7 => O7Event::PlanUpdated {
                        entries: vec![PlanEntry {
                            content: rng.text(),
                            status: PlanStatus::InProgress,
                            priority: None,
                        }],
                    },
                    8 => O7Event::UsageUpdated(UsageInfo {
                        context_used: Some(rng.below(1000)),
                        context_size: Some(1000),
                        cost: None,
                    }),
                    9 => {
                        request_counter += 1;
                        let id = format!("{turn}.p{request_counter}");
                        push(
                            &mut events,
                            Some(&turn),
                            O7Event::PermissionRequested(PermissionRequest {
                                request_id: id.clone(),
                                tool_call_id: None,
                                title: rng.text(),
                                kind: Some(ToolKind::Edit),
                                raw_input: None,
                                content: Vec::new(),
                                locations: Vec::new(),
                                options: vec![PermissionOption {
                                    option_id: "allow".into(),
                                    kind: PermissionOptionKind::AllowOnce,
                                    label: "Allow".into(),
                                }],
                                truncated: None,
                            }),
                        );
                        O7Event::PermissionResolved {
                            request_id: id,
                            decision: PermissionDecision::Selected {
                                option_id: "allow".into(),
                                plan: None,
                            },
                            by: ResolvedBy::User,
                        }
                    }
                    10 => {
                        request_counter += 1;
                        O7Event::UiRequest(UiRequest {
                            request_id: format!("{turn}.p{request_counter}"),
                            prompt: UiPrompt::Confirm {
                                message: rng.text(),
                            },
                        })
                    }
                    _ => {
                        if messages > 0 && rng.chance(50) {
                            O7Event::MessageCompleted {
                                message_id: format!("{turn}.m{messages}"),
                            }
                        } else {
                            O7Event::Notice {
                                level: NoticeLevel::Info,
                                message: rng.text(),
                            }
                        }
                    }
                };
                push(&mut events, Some(&turn), event);
            }
            push(
                &mut events,
                Some(&turn),
                O7Event::TurnEnded {
                    turn_id: turn.clone(),
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                    duration_ms: None,
                    recovered: false,
                },
            );
            // Late output after the reply, stored outside any turn.
            for _ in 0..rng.below(4) {
                let event = match rng.below(3) {
                    0 => match known_tools.get(rng.below(known_tools.len() as u64 + 1) as usize) {
                        Some(id) => O7Event::ToolUpdate(random_update(&mut rng, id)),
                        None => continue,
                    },
                    1 => delta(&format!("{spawn}.m{t}"), &rng.text()),
                    _ => {
                        let id = format!("{spawn}.bg{t}");
                        known_tools.push(id.clone());
                        O7Event::ToolCall(call(&id, "background"))
                    }
                };
                push(&mut events, None, event);
            }
        }
        Timeline { events, turns }
    }

    /// The compaction property test: the reducer gives the same transcript for generated live
    /// sequences and for their compacted rows, across turns, updates to earlier turns' tools, and
    /// updates stored outside turns; seqs stay unique and `MAX(seq)` never drops.
    #[test]
    fn compaction_preserves_the_reduced_transcript() {
        for seed in 1..=400u64 {
            let timeline = generate(seed);
            let live = reduce(timeline.events.iter().map(|(_, _, event)| event));
            let mut stored: Vec<(u32, O7Event)> = timeline
                .events
                .iter()
                .filter(|(_, tag, _)| tag.is_none())
                .map(|(seq, _, event)| (*seq, event.clone()))
                .collect();
            for turn in &timeline.turns {
                let rows: Vec<InRow> = timeline
                    .events
                    .iter()
                    .filter(|(_, tag, _)| tag.as_deref() == Some(turn.as_str()))
                    .map(|(seq, _, event)| row(*seq, event))
                    .collect();
                let ended = rows.iter().map(|row| row.seq).max();
                let compacted = compact(
                    turn,
                    rows,
                    &CompactLimits {
                        piece_bytes: 24,
                        ..CompactLimits::STANDARD
                    },
                );
                assert_eq!(
                    compacted.rows.iter().map(|row| row.seq).max(),
                    ended,
                    "seed {seed}: TurnEnded kept"
                );
                for row in compacted.rows {
                    stored.push((
                        row.seq,
                        serde_json::from_str(&row.payload).expect("compacted rows parse"),
                    ));
                }
            }
            stored.sort_by_key(|(seq, _)| *seq);
            let seqs: Vec<u32> = stored.iter().map(|(seq, _)| *seq).collect();
            let mut unique = seqs.clone();
            unique.dedup();
            assert_eq!(seqs, unique, "seed {seed}: unique seqs");
            assert_eq!(
                seqs.last(),
                timeline.events.last().map(|(seq, _, _)| seq),
                "seed {seed}: MAX(seq) kept"
            );
            let compacted = reduce(stored.iter().map(|(_, event)| event));
            assert_eq!(compacted, live, "seed {seed}");
        }
    }
}
