//! A bounded, readable snapshot of saved rows. It never opens agent files or attachment payloads.
use std::collections::HashMap;

use ghost_protocol::GhostEvent;
use rusqlite::{Connection, OptionalExtension};

const MAX_SOURCE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROWS: u64 = 100_000;

struct Block {
    label: String,
    body: String,
    literal: bool,
}

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(std::io::Error::other(message).into())
}

fn fenced(text: &str) -> String {
    let longest = text
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}\n{text}\n{fence}\n")
}

pub(crate) fn markdown(conn: &Connection, chat_id: &str) -> rusqlite::Result<String> {
    read(conn, chat_id, MAX_SOURCE_BYTES)
}

fn read(conn: &Connection, chat_id: &str, max_source: u64) -> rusqlite::Result<String> {
    let title: String = conn
        .query_row(
            "SELECT COALESCE(title,'New chat') FROM chats WHERE id=?1 AND deleted=0",
            [chat_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let (bytes, count, last): (i64, i64, Option<u32>) = conn.query_row(
        "SELECT COALESCE(SUM(octet_length(payload)),0),COUNT(*),MAX(seq) FROM events WHERE chat_id=?1",
        [chat_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    if bytes < 0
        || count < 0
        || bytes as u64 > max_source
        || count as u64 > MAX_ROWS
        || title.len() > 8192
    {
        return Err(invalid(
            "This conversation exceeds the readable export limit. No partial export was created.",
        ));
    }
    let mut blocks = Vec::<Block>::new();
    let mut messages = HashMap::<(Option<String>, &'static str, String), usize>::new();
    let mut unknown = 0usize;
    let mut query =
        conn.prepare("SELECT turn_id,kind,payload FROM events WHERE chat_id=?1 ORDER BY seq")?;
    let mut rows = query.query([chat_id])?;
    while let Some(row) = rows.next()? {
        let turn: Option<String> = row.get(0)?;
        let kind: String = row.get(1)?;
        let payload: String = row.get(2)?;
        let Some(event) = super::history::parse_row(&kind, &payload, turn.as_deref()) else {
            unknown += 1;
            blocks.push(Block {
                label: "Unsupported saved event".into(),
                body: payload,
                literal: true,
            });
            continue;
        };
        let message = match &event {
            GhostEvent::UserMessageDelta { message_id, text } => Some(("You", message_id, text)),
            GhostEvent::MessageDelta { message_id, text } => Some(("Assistant", message_id, text)),
            GhostEvent::ThoughtDelta { thought_id, text } => Some(("Reasoning", thought_id, text)),
            _ => None,
        };
        if let Some((label, id, text)) = message {
            let index = *messages
                .entry((turn, label, id.clone()))
                .or_insert_with(|| {
                    blocks.push(Block {
                        label: label.into(),
                        body: String::new(),
                        literal: false,
                    });
                    blocks.len() - 1
                });
            blocks[index].body.push_str(text);
            continue;
        }
        let label = match &event {
            GhostEvent::TurnStarted { .. } => "Turn started",
            GhostEvent::TurnEnded { .. } => "Turn ended",
            GhostEvent::ToolCall(_) => "Tool call",
            GhostEvent::ToolUpdate(_) => "Tool update",
            GhostEvent::PermissionRequested(_) => "Permission requested",
            GhostEvent::PermissionResolved { .. } => "Permission resolved",
            GhostEvent::UiRequest(_) => "Question",
            GhostEvent::UiResolved { .. } => "Answer",
            GhostEvent::PlanUpdated { .. } => "Plan",
            // A task's report: subagent work no longer shows in the transcript itself.
            GhostEvent::TaskUpdated(task) if task.summary.is_some() => "Background task",
            GhostEvent::SteeringStatus { .. } => "Steering delivery",
            GhostEvent::Notice { .. } => "Notice",
            GhostEvent::Error { .. } => "Error",
            GhostEvent::UsageUpdated(_) | GhostEvent::LimitsUpdated { .. } => "Usage",
            GhostEvent::MessageCompleted { .. }
            | GhostEvent::ThoughtCompleted { .. }
            | GhostEvent::SessionUpdated(_)
            | GhostEvent::PromptLifecycle { .. } => continue,
            _ => continue,
        };
        let body = match &event {
            GhostEvent::Notice { message, .. } => message.clone(),
            GhostEvent::TaskUpdated(task) => format!(
                "{} ({})\n\n{}",
                task.title,
                task.status.as_str(),
                task.summary.as_deref().unwrap_or("")
            ),
            GhostEvent::Error {
                message, detail, ..
            } => format!("{message}\n{}", detail.as_deref().unwrap_or("")),
            _ => {
                serde_json::to_string_pretty(&event).map_err(|error| invalid(&error.to_string()))?
            }
        };
        blocks.push(Block {
            label: label.into(),
            body,
            literal: true,
        });
    }
    let mut output = format!(
        "# Saved Ghost conversation\n\n{}\nSaved through event {}. Running turns may still be incomplete. This snapshot excludes queued drafts, binary attachments and external tool blobs; their visible text references remain.\n\n",
        fenced(&title),
        last.map_or_else(|| "none".into(), |seq| seq.to_string())
    );
    if unknown > 0 {
        output.push_str(&format!(
            "{unknown} unrecognized saved events are included verbatim below.\n\n"
        ));
    }
    for block in blocks {
        output.push_str(&format!("## {}\n\n", block.label));
        output.push_str(&if block.literal {
            fenced(&block.body)
        } else {
            block.body
        });
        output.push_str("\n\n");
        if output.len() > MAX_OUTPUT_BYTES {
            return Err(invalid(
                "This conversation exceeds the readable export limit. No partial export was created.",
            ));
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatstore::test_support::TestDb;

    fn event(db: &TestDb, chat: &str, seq: u32, turn: &str, event: GhostEvent) {
        let payload = serde_json::to_string(&event).unwrap();
        db.conn().execute("INSERT INTO events(chat_id,seq,turn_id,kind,payload,created_at,device_id) VALUES(?1,?2,?3,?4,?5,1,'dev')", rusqlite::params![chat,seq,turn,event.kind(),payload]).unwrap();
    }

    #[test]
    fn deltas_join_without_merging_reused_ids_across_turns_or_chats() {
        let db = TestDb::new();
        let chat = db.chat("/fixture");
        let other = db.chat("/other");
        db.turn(&chat, "first", 1, 0);
        db.turn(&chat, "second", 2, 2);
        db.turn(&other, "private", 1, 0);
        for (seq, turn, text) in [
            (0, "first", "Hello "),
            (1, "first", "world"),
            (2, "second", "Second answer"),
        ] {
            event(
                &db,
                &chat,
                seq,
                turn,
                GhostEvent::MessageDelta {
                    message_id: "shared-id".into(),
                    text: text.into(),
                },
            );
        }
        event(
            &db,
            &other,
            0,
            "private",
            GhostEvent::MessageDelta {
                message_id: "shared-id".into(),
                text: "Other private chat".into(),
            },
        );
        let result = markdown(db.conn(), &chat).unwrap();
        assert!(result.contains("Hello world"));
        assert!(result.contains("Second answer"));
        assert_eq!(result.matches("## Assistant").count(), 2);
        assert!(!result.contains("Other private chat"));
        assert!(read(db.conn(), &chat, 1).is_err());
        assert!(markdown(db.conn(), "missing").is_err());
    }

    #[test]
    fn unknown_saved_content_is_preserved_and_cannot_close_its_fence() {
        let db = TestDb::new();
        let chat = db.chat("/fixture");
        let raw = "{\"type\":\"future_event\",\"text\":\"``` visible unknown\"}";
        db.conn().execute("INSERT INTO events(chat_id,seq,kind,payload,created_at,device_id) VALUES(?1,0,'future_event',?2,1,'dev')", rusqlite::params![chat,raw]).unwrap();
        let result = markdown(db.conn(), &chat).unwrap();
        assert!(result.contains(raw));
        assert!(result.contains("1 unrecognized saved events"));
        assert!(result.contains("````\n{"));
        assert!(result.contains("binary attachments"));
    }
}
