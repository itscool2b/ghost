//! History pages: whole turns below a cursor within a byte budget, or the newest rows of a turn too
//! large for one page (with `cut_turn_id`); rows that do not parse are skipped and counted, except
//! `turn_ended` rows.
//!
//! ```text
//! inputs   chat_id, cursor (exclusive upper bound), max_bytes (default 512 KiB, clamped to 64 KiB..4 MiB)
//! 1  turn ranges below the cursor, newest first (64 at most); a range is [a turn's first_event_seq, the
//!    next newer turn's first_event_seq or the cursor); events outside turns belong to the range they
//!    fall in, and events before the chat's first turn to that turn's range
//! 2  whole ranges are added while the total stays within max_bytes
//! 3  when not even the newest range fits, the page is its newest part: its rows newest first while
//!    the total stays within max_bytes, at least one row; cut_turn_id = that turn
//! 4  lo = the oldest added range's start, or the oldest added row's seq
//! 5  the page: every event from lo up to the cursor; has_more = an event below lo exists
//! ```
//!
//! A page holds at most `max_bytes` plus one row, and a row is at most 1 MiB (a compacted text
//! piece), so a 50 MB turn arrives in pages and the next page continues inside a cut turn the same
//! way. Sizes are payload bytes (`octet_length`), which SQLite reads from the record header.

use ghost_protocol::session::{HistoryItem, HistoryPage, TurnAgent};
use ghost_protocol::{GhostEvent, StopReason};
use rusqlite::{Connection, OptionalExtension, params};

/// `chat_history`'s default page size.
pub(crate) const DEFAULT_PAGE_BYTES: usize = 512 * 1024;
const MIN_PAGE_BYTES: usize = 64 * 1024;
const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;

/// Turn ranges one page considers at most.
const RANGES_PER_PAGE: i64 = 64;

/// `max_bytes` (512 KiB when not given) within 64 KiB..4 MiB.
pub(crate) fn clamp_page_bytes(max_bytes: Option<usize>) -> usize {
    max_bytes
        .unwrap_or(DEFAULT_PAGE_BYTES)
        .clamp(MIN_PAGE_BYTES, MAX_PAGE_BYTES)
}

/// One turn's part of the history below the cursor.
struct Range {
    turn_id: Option<String>,
    start: u32,
    end: u32,
}

/// The page of `chat_id`'s history that ends just below `cursor`. `unsaved_from` is left `None`:
/// the caller knows what is unsaved.
pub(crate) fn page(
    conn: &Connection,
    chat_id: &str,
    cursor: u32,
    max_bytes: usize,
) -> rusqlite::Result<HistoryPage> {
    let max_bytes = clamp_page_bytes(Some(max_bytes)) as u64;
    let ranges = ranges(conn, chat_id, cursor)?;

    let mut total = 0u64;
    let mut lo = None;
    let mut cut_turn_id = None;
    for range in &ranges {
        let size = range_bytes(conn, chat_id, range.start, range.end)?;
        if total.saturating_add(size) > max_bytes {
            break;
        }
        total = total.saturating_add(size);
        lo = Some(range.start);
    }
    if lo.is_none()
        && let Some(newest) = ranges.first()
    {
        // Not even the newest range fits: its newest rows, at least one.
        let mut stmt = conn.prepare_cached(
            "SELECT seq, octet_length(payload) FROM events WHERE chat_id = ?1 AND seq >= ?2 AND seq < ?3 \
             ORDER BY seq DESC",
        )?;
        let mut rows = stmt.query(params![chat_id, newest.start, newest.end])?;
        while let Some(row) = rows.next()? {
            let seq: u32 = row.get(0)?;
            let size = row.get::<_, Option<i64>>(1)?.unwrap_or(0).max(0) as u64;
            if lo.is_some() && total.saturating_add(size) > max_bytes {
                break;
            }
            total = total.saturating_add(size);
            lo = Some(seq);
        }
        cut_turn_id = newest.turn_id.clone();
    }
    let lo = lo.unwrap_or(0);

    let mut items = Vec::new();
    let mut skipped = 0u32;
    let mut stmt = conn.prepare_cached(
        "SELECT seq, turn_id, kind, payload, created_at FROM events WHERE chat_id = ?1 AND seq >= ?2 \
         AND seq < ?3 ORDER BY seq",
    )?;
    let mut rows = stmt.query(params![chat_id, lo, cursor])?;
    while let Some(row) = rows.next()? {
        let seq: u32 = row.get(0)?;
        let turn_id: Option<String> = row.get(1)?;
        let kind: String = row.get(2)?;
        let payload: String = row.get(3)?;
        let at: i64 = row.get(4)?;
        match parse_row(&kind, &payload, turn_id.as_deref()) {
            Some(event) => items.push(HistoryItem {
                seq,
                turn_id,
                at,
                event,
            }),
            None => skipped = skipped.saturating_add(1),
        }
    }
    let has_more = lo > 0
        && conn
            .prepare_cached("SELECT 1 FROM events WHERE chat_id = ?1 AND seq < ?2 LIMIT 1")?
            .query_row(params![chat_id, lo], |_| Ok(()))
            .optional()?
            .is_some();
    let turn_agents = turn_agents(conn, chat_id, lo, cursor)?;
    Ok(HistoryPage {
        chat_id: chat_id.to_owned(),
        items,
        has_more,
        before_seq: Some(lo),
        cut_turn_id,
        unsaved_from: None,
        skipped,
        turn_agents,
    })
}

/// The agent of every turn with rows in `lo..cursor`, by the rule conversation uses: the agent
/// the turn itself names (one an earlier version brought from another device), else its agent
/// session's, else the chat's. A turn that resolves to no agent is left out (the interface then
/// shows the chat's agent).
fn turn_agents(
    conn: &Connection,
    chat_id: &str,
    lo: u32,
    cursor: u32,
) -> rusqlite::Result<Vec<TurnAgent>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, COALESCE(t.source_agent, s.agent_id, c.active_agent, '') FROM turns t \
         JOIN chats c ON c.id = t.chat_id LEFT JOIN agent_sessions s ON s.id = t.agent_session_id \
         WHERE t.chat_id = ?1 AND t.id IN (SELECT DISTINCT turn_id FROM events WHERE chat_id = ?1 \
         AND seq >= ?2 AND seq < ?3 AND turn_id IS NOT NULL) ORDER BY t.seq",
    )?;
    let rows = stmt.query_map(params![chat_id, lo, cursor], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut agents = Vec::new();
    for row in rows {
        let (turn_id, agent_id) = row?;
        if agent_id.is_empty() {
            continue;
        }
        // A turn names its agent, not the connection its session ran through.
        let agent_id = ghost_agents::agent_of(&agent_id).to_owned();
        let agent_name = ghost_agents::get(&agent_id)
            .map_or_else(|| agent_id.clone(), |spec| spec.name.to_owned());
        agents.push(TurnAgent {
            turn_id,
            agent_id,
            agent_name,
        });
    }
    Ok(agents)
}

/// Step 1: the turn ranges below the cursor, newest first.
fn ranges(conn: &Connection, chat_id: &str, cursor: u32) -> rusqlite::Result<Vec<Range>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, first_event_seq FROM turns WHERE chat_id = ?1 AND first_event_seq < ?2 \
         ORDER BY first_event_seq DESC LIMIT ?3",
    )?;
    let turns: Vec<(String, u32)> = stmt
        .query_map(params![chat_id, cursor, RANGES_PER_PAGE], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if turns.is_empty() {
        // No turn below the cursor: everything there is one range.
        return Ok(vec![Range {
            turn_id: None,
            start: 0,
            end: cursor,
        }]);
    }
    let oldest_start: Option<u32> = conn
        .prepare_cached("SELECT MIN(first_event_seq) FROM turns WHERE chat_id = ?1")?
        .query_row(params![chat_id], |row| row.get(0))?;
    let mut ranges = Vec::with_capacity(turns.len());
    let mut end = cursor;
    for (turn_id, start) in turns {
        // Events before the chat's first turn come along with it.
        let start = if Some(start) == oldest_start {
            0
        } else {
            start
        };
        ranges.push(Range {
            turn_id: Some(turn_id),
            start,
            end,
        });
        end = start;
    }
    Ok(ranges)
}

fn range_bytes(conn: &Connection, chat_id: &str, start: u32, end: u32) -> rusqlite::Result<u64> {
    let total: i64 = conn
        .prepare_cached(
            "SELECT COALESCE(SUM(octet_length(payload)), 0) FROM events WHERE chat_id = ?1 AND seq >= ?2 \
             AND seq < ?3",
        )?
        .query_row(params![chat_id, start, end], |row| row.get(0))?;
    Ok(total.max(0) as u64)
}

/// A stored row as an event. Unknown enum names read leniently; a row that still does not parse is
/// skipped (`None`), except a `turn_ended` row, which reads as its turn ending with `Other` so the
/// turn still renders as ended.
pub(crate) fn parse_row(kind: &str, payload: &str, turn_id: Option<&str>) -> Option<GhostEvent> {
    if let Ok(event) = serde_json::from_str::<GhostEvent>(payload) {
        return Some(event);
    }
    if kind != "turn_ended" {
        return None;
    }
    let from_payload = serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| {
            value
                .get("turn_id")
                .and_then(|id| id.as_str())
                .map(str::to_owned)
        });
    let turn_id = from_payload.or_else(|| turn_id.map(str::to_owned))?;
    Some(GhostEvent::TurnEnded {
        turn_id,
        stop_reason: StopReason::Other,
        usage: None,
        duration_ms: None,
        recovered: false,
    })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ghost_protocol::{NoticeLevel, ToolCall, ToolKind, ToolStatus};
    use rusqlite::params;

    use super::*;
    use crate::chatstore::test_support::{TestDb, event_row, payload};

    /// Stores a turn: TurnStarted at `start`, `deltas` message deltas of `delta_len` bytes, TurnEnded.
    /// Returns the next free seq.
    fn turn(
        db: &TestDb,
        chat: &str,
        ordinal: u32,
        start: u32,
        deltas: u32,
        delta_len: usize,
    ) -> u32 {
        let turn_id = format!("turn-{ordinal:05}");
        db.turn(chat, &turn_id, ordinal, start);
        let mut seq = start + 1;
        let text = "x".repeat(delta_len);
        let mut rows = vec![event_row(
            start,
            Some(&turn_id),
            &GhostEvent::TurnStarted {
                turn_id: turn_id.clone(),
                prompt_id: None,
            },
        )];
        rows.extend((0..deltas).map(|i| {
            event_row(
                seq + i,
                Some(&turn_id),
                &GhostEvent::MessageDelta {
                    message_id: format!("{turn_id}.m1"),
                    text: text.clone(),
                },
            )
        }));
        db.append(chat, &rows);
        seq += deltas;
        db.append(
            chat,
            &[event_row(
                seq,
                Some(&turn_id),
                &GhostEvent::TurnEnded {
                    turn_id: turn_id.clone(),
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                    duration_ms: None,
                    recovered: false,
                },
            )],
        );
        seq + 1
    }

    fn notice(seq: u32, text: &str) -> crate::chatstore::EventRow {
        event_row(
            seq,
            None,
            &GhostEvent::Notice {
                level: NoticeLevel::Info,
                message: text.into(),
            },
        )
    }

    fn seqs(page: &HistoryPage) -> Vec<u32> {
        page.items.iter().map(|item| item.seq).collect()
    }

    /// Each turn keeps the agent that ran it after the chat switches agents: its agent session's,
    /// else the agent the turn itself names, else the chat's current agent.
    #[test]
    fn history_pages_name_the_agent_of_each_turn() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        let mut next = 0;
        for ordinal in 1..=3 {
            next = turn(&db, &chat, ordinal, next, 1, 10);
        }
        db.conn()
            .execute_batch(&format!(
                "INSERT INTO agent_sessions (id, chat_id, agent_id, cwd, created_at, updated_at, device_id) \
                 VALUES ('s-claude', '{chat}', 'claude', '/work', 1, 1, 'dev'); \
                 UPDATE turns SET agent_session_id = 's-claude' WHERE id = 'turn-00001'; \
                 UPDATE turns SET source_agent = 'pi' WHERE id = 'turn-00002'; \
                 UPDATE chats SET active_agent = 'codex' WHERE id = '{chat}';"
            ))
            .expect("agents");
        let page = page(db.conn(), &chat, next, DEFAULT_PAGE_BYTES).expect("page");
        let agents: Vec<(&str, &str, &str)> = page
            .turn_agents
            .iter()
            .map(|agent| {
                (
                    agent.turn_id.as_str(),
                    agent.agent_id.as_str(),
                    agent.agent_name.as_str(),
                )
            })
            .collect();
        assert_eq!(
            agents,
            [
                ("turn-00001", "claude", "Claude Code"),
                ("turn-00002", "pi", "Pi"),
                ("turn-00003", "codex", "Codex"),
            ]
        );
        // Only turns with rows in the page are named.
        let older = page_before(&db, &chat, 3);
        assert_eq!(older.len(), 1);
    }

    fn page_before(db: &TestDb, chat: &str, cursor: u32) -> Vec<TurnAgent> {
        page(db.conn(), chat, cursor, DEFAULT_PAGE_BYTES)
            .expect("page")
            .turn_agents
    }

    #[test]
    fn history_pages_an_empty_chat() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        let page = page(db.conn(), &chat, 0, DEFAULT_PAGE_BYTES).expect("page");
        assert!(page.items.is_empty());
        assert!(!page.has_more);
        assert_eq!(page.cut_turn_id, None);
        assert_eq!(page.skipped, 0);
    }

    #[test]
    fn history_pages_whole_turns_newest_first_within_the_budget() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        // Five turns of about 20 KB each.
        let mut next = 0;
        for ordinal in 1..=5 {
            next = turn(&db, &chat, ordinal, next, 10, 2_000);
        }
        // A 64 KiB page (the floor) holds three whole turns.
        let first = page(db.conn(), &chat, next, 1).expect("page");
        let turns: Vec<_> = first
            .items
            .iter()
            .filter_map(|item| match &item.event {
                GhostEvent::TurnStarted { turn_id, .. } => Some(turn_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(turns, ["turn-00003", "turn-00004", "turn-00005"]);
        assert!(first.has_more);
        assert_eq!(first.cut_turn_id, None);
        let lo = first.before_seq.expect("cursor");
        assert_eq!(first.items.first().map(|item| item.seq), Some(lo));

        let second = page(db.conn(), &chat, lo, 1).expect("older page");
        assert_eq!(second.items.len(), 24, "turns 1 and 2, whole");
        assert!(!second.has_more);
        assert_eq!(second.items.last().map(|item| item.seq), Some(lo - 1));
    }

    #[test]
    fn history_pages_keep_events_outside_turns_with_the_range_they_fall_in() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        // An error before the first turn, a late notice between turns, one after the last.
        db.append(&chat, &[notice(0, "before")]);
        let next = turn(&db, &chat, 1, 1, 2, 10);
        db.append(&chat, &[notice(next, "between")]);
        let next = turn(&db, &chat, 2, next + 1, 2, 10);
        db.append(&chat, &[notice(next, "after")]);
        let page = page(db.conn(), &chat, next + 1, DEFAULT_PAGE_BYTES).expect("page");
        assert_eq!(seqs(&page), (0..=next).collect::<Vec<_>>());
        assert!(!page.has_more);
        let untagged: Vec<_> = page
            .items
            .iter()
            .filter(|item| item.turn_id.is_none())
            .map(|item| item.seq)
            .collect();
        assert_eq!(untagged, [0, 5, next]);
    }

    #[test]
    fn history_pages_events_when_no_turn_exists() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        db.append(&chat, &[notice(0, "a"), notice(3, "b")]);
        let page = page(db.conn(), &chat, 10, DEFAULT_PAGE_BYTES).expect("page");
        assert_eq!(seqs(&page), [0, 3]);
        assert!(!page.has_more);
    }

    #[test]
    fn history_pages_skip_rows_that_do_not_parse_but_keep_turn_ended() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        db.turn(&chat, "t1", 1, 0);
        let started = event_row(
            0,
            Some("t1"),
            &GhostEvent::TurnStarted {
                turn_id: "t1".into(),
                prompt_id: None,
            },
        );
        let mut future = started.clone();
        future.seq = 1;
        future.kind = "hologram";
        future.payload = r#"{"type":"hologram","depth":3}"#.into();
        let mut ended = started.clone();
        ended.seq = 2;
        ended.kind = "turn_ended";
        ended.payload =
            r#"{"type":"turn_ended","turn_id":"t1","stop_reason":"teleported","usage":7}"#.into();
        let mut lenient = started.clone();
        lenient.seq = 3;
        lenient.turn_id = None;
        lenient.kind = "notice";
        lenient.payload =
            r#"{"type":"notice","level":"critical","message":"hi","extra":true}"#.into();
        db.append(&chat, &[started, future, ended, lenient]);
        let page = page(db.conn(), &chat, 4, DEFAULT_PAGE_BYTES).expect("page");
        assert_eq!(page.skipped, 1);
        assert_eq!(seqs(&page), [0, 2, 3]);
        assert_eq!(
            page.items[1].event,
            GhostEvent::TurnEnded {
                turn_id: "t1".into(),
                stop_reason: StopReason::Other,
                usage: None,
                duration_ms: None,
                recovered: false,
            }
        );
        assert_eq!(
            page.items[2].event,
            GhostEvent::Notice {
                level: NoticeLevel::Info,
                message: "hi".into()
            }
        );
    }

    /// S12: a generated 50 MB turn, uncompacted (frame-sized rows), is paged by rows with
    /// `cut_turn_id`, and the next page continues inside it.
    #[test]
    fn huge_turn_pages_by_rows() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        let small_end = turn(&db, &chat, 1, 0, 3, 100);
        // About 50 MB in 7 KB rows.
        let rows = 7_200;
        let end = turn(&db, &chat, 2, small_end, rows, 7_000);
        let mut cursor = end;
        let mut pages = 0;
        let mut seen = Vec::new();
        loop {
            let page = page(db.conn(), &chat, cursor, DEFAULT_PAGE_BYTES).expect("page");
            pages += 1;
            let bytes: usize = page
                .items
                .iter()
                .map(|item| payload(&item.event).len())
                .sum();
            assert!(bytes <= DEFAULT_PAGE_BYTES + 8 * 1024, "{bytes}");
            seen.extend(seqs(&page).into_iter().rev());
            if page.cut_turn_id.is_none() {
                // The small turn and the start of the big one fit once the rest is paged.
                assert!(!page.has_more);
                break;
            }
            assert_eq!(page.cut_turn_id.as_deref(), Some("turn-00002"));
            assert!(page.has_more);
            cursor = page.before_seq.expect("cursor");
            assert!(pages < 200, "paging must make progress");
        }
        seen.reverse();
        assert_eq!(seen, (0..end).collect::<Vec<_>>(), "no gap, no duplicate");
        assert!(pages > 90, "{pages}");
    }

    /// S12: the same 50 MB turn compacted into 1 MiB pieces pages one or two rows at a time.
    #[test]
    fn huge_compacted_turn_pages_by_rows() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        db.turn(&chat, "big", 1, 0);
        let piece = "y".repeat(1024 * 1024 - 64);
        let mut rows = vec![event_row(
            0,
            Some("big"),
            &GhostEvent::TurnStarted {
                turn_id: "big".into(),
                prompt_id: None,
            },
        )];
        for i in 0..50u32 {
            rows.push(event_row(
                1 + i * 100,
                Some("big"),
                &GhostEvent::MessageDelta {
                    message_id: "big.m1".into(),
                    text: piece.clone(),
                },
            ));
        }
        db.append(&chat, &rows);
        let mut cursor = 6_000;
        let mut seen = 0;
        loop {
            let page = page(db.conn(), &chat, cursor, DEFAULT_PAGE_BYTES).expect("page");
            assert!(!page.items.is_empty());
            assert!(page.items.len() <= 2, "{}", page.items.len());
            seen += page.items.len();
            if !page.has_more {
                break;
            }
            assert_eq!(page.cut_turn_id.as_deref(), Some("big"));
            cursor = page.before_seq.expect("cursor");
        }
        assert_eq!(seen, 51);
    }

    /// History pages for a generated 10,000-turn chat cost the same at any depth and typically
    /// take under 50 ms in optimized builds.
    #[test]
    fn history_pages_of_a_10000_turn_chat_are_fast() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        let conn = db.conn();
        // Compacted-looking turns: started, user text, a tool call, an agent message, ended.
        let tx = conn.unchecked_transaction().expect("tx");
        let mut seq = 0u32;
        for ordinal in 1..=10_000u32 {
            let turn_id = format!("turn-{ordinal:05}");
            tx.execute(
                "INSERT INTO turns (id, chat_id, seq, status, started_at, device_id, first_event_seq, \
                 last_event_seq, compacted_at) VALUES (?1, ?2, ?3, 'completed', 1, 'dev', ?4, ?5, 2)",
                params![turn_id, chat, ordinal, seq, seq + 4],
            )
            .expect("turn");
            let events = [
                GhostEvent::TurnStarted {
                    turn_id: turn_id.clone(),
                    prompt_id: Some(format!("p{ordinal}")),
                },
                GhostEvent::UserMessageDelta {
                    message_id: format!("{turn_id}.u"),
                    text: "Please change the thing in the file and run the tests again.".into(),
                },
                GhostEvent::ToolCall(ToolCall {
                    tool_call_id: format!("{turn_id}.tool-1"),
                    kind: ToolKind::Edit,
                    title: "Edit src/lib.rs".into(),
                    status: ToolStatus::Completed,
                    locations: Vec::new(),
                    raw_input: Some(serde_json::json!({"path": "src/lib.rs"})),
                    tool_name: Some("edit".into()),
                    content: Vec::new(),
                    truncated: None,
                }),
                GhostEvent::MessageDelta {
                    message_id: format!("{turn_id}.m1"),
                    text: "Done. ".repeat(80),
                },
                GhostEvent::TurnEnded {
                    turn_id: turn_id.clone(),
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                    duration_ms: Some(1200),
                    recovered: false,
                },
            ];
            for event in events {
                let row = event_row(seq, Some(&turn_id), &event);
                tx.execute(
                    "INSERT INTO events (id, chat_id, turn_id, seq, kind, payload, created_at, device_id) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 'dev')",
                    params![format!("e{seq}"), chat, turn_id, seq, row.kind, row.payload],
                )
                .expect("event");
                seq += 1;
            }
        }
        tx.commit().expect("commit");

        // Walk the history once for its page cursors, newest first.
        let mut cursor = seq;
        let mut cursors = Vec::new();
        loop {
            cursors.push(cursor);
            let page = page(conn, &chat, cursor, DEFAULT_PAGE_BYTES).expect("page");
            if !page.has_more {
                break;
            }
            cursor = page.before_seq.expect("cursor");
        }
        // Time each page twice, alternating the newest and oldest ends, and keep its best time: a
        // busy moment on a shared runner then slows both ends alike instead of one quarter.
        let mut times = vec![std::time::Duration::MAX; cursors.len()];
        for _ in 0..2 {
            for step in 0..cursors.len() {
                let index = if step % 2 == 0 {
                    step / 2
                } else {
                    cursors.len() - 1 - step / 2
                };
                let started = Instant::now();
                page(conn, &chat, cursors[index], DEFAULT_PAGE_BYTES).expect("page");
                times[index] = times[index].min(started.elapsed());
            }
        }
        assert!(times.len() >= 10_000 / 64, "{}", times.len());
        // A page that scans from either end of the chat makes one end of the history slower than
        // the other, whatever the machine, so that ratio is the portable check. The absolute 50 ms budget is for optimized builds; unoptimized test
        // builds on shared CI runners get headroom. A single outlier is scheduling noise.
        let quarter = times.len() / 4;
        let median = |mut part: Vec<std::time::Duration>| {
            part.sort();
            part[part.len() / 2]
        };
        let newest = median(times[..quarter].to_vec());
        let oldest = median(times[times.len() - quarter..].to_vec());
        assert!(
            oldest.max(newest) <= oldest.min(newest) * 3 + std::time::Duration::from_millis(5),
            "the oldest pages took {oldest:?}, the newest {newest:?}"
        );
        let budget = if cfg!(debug_assertions) { 250 } else { 50 };
        let typical = median(times.clone());
        assert!(
            typical < std::time::Duration::from_millis(budget),
            "the median page took {typical:?}"
        );
        let slowest = times.iter().copied().max().unwrap_or_default();
        assert!(
            slowest < std::time::Duration::from_secs(1),
            "the slowest page took {slowest:?}"
        );
    }
}
