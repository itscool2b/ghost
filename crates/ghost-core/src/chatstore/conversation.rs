//! Saved conversation reads and atomic, explicitly confirmed continuation changes.

use std::collections::BTreeMap;

use ghost_drivers::driver::NativeStartOptions;
use ghost_protocol::{
    GhostEvent, PromptPart,
    conversation::{ConversationTurn, ConversationTurns},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{AgentSessionRow, ChatRecord, rows};
use crate::conversation::{CommandRun, ContextTurn, cut};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingContinuation {
    pub handoff: String,
    pub force_fresh: bool,
    pub resume_source: Option<String>,
    pub native_start: NativeStartOptions,
    /// The connection a native fork resumes `resume_source` through: the source session's, so
    /// a fork never crosses from an adapter's session to the native driver or back. Absent in
    /// continuations saved before agents had one entry each.
    #[serde(default)]
    pub connection: Option<String>,
}

impl PendingContinuation {
    /// Nothing left for the next prompt to carry: the handoff went out with the continuation's
    /// first turn and its session has started. The row stays as the chat's lineage.
    fn is_spent(&self) -> bool {
        self.handoff.is_empty()
            && !self.force_fresh
            && self.resume_source.is_none()
            && self.connection.is_none()
            && self.native_start == NativeStartOptions::default()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ConversationSnapshot {
    pub remote_history: bool,
    pub chat: ChatRecord,
    pub next_seq: u32,
    pub latest_ordinal: u32,
    pub turns: Vec<ContextTurn>,
    pub selected: Option<ContextTurn>,
    pub target_session: Option<AgentSessionRow>,
    pub target_last_turn: Option<u32>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ApplyContinuation {
    pub source_chat: String,
    pub source_turn: Option<String>,
    pub expected_next_seq: u32,
    pub expected_updated_at: i64,
    pub expected_folder: std::path::PathBuf,
    pub expected_active_agent: String,
    pub expected_target_session: Option<(String, Option<String>)>,
    pub target_agent: String,
    pub destination_chat: Option<String>,
    pub destination_folder: Option<std::path::PathBuf>,
    pub pending: PendingContinuation,
    pub parts: Vec<PromptPart>,
}

#[derive(Clone)]
pub(crate) enum ConversationCommand {
    Pending {
        chat_id: String,
    },
    Turns {
        chat_id: String,
        before: Option<u32>,
    },
    Snapshot {
        chat_id: String,
        selected: Option<String>,
        target_agent: Option<String>,
        include_selected: bool,
    },
    Apply(Box<ApplyContinuation>),
}

pub(crate) enum ConversationReply {
    Pending(Option<ghost_protocol::conversation::PendingHandoff>),
    Turns(ConversationTurns),
    Snapshot(Box<ConversationSnapshot>),
    Applied(Box<super::ChatLoad>),
}

impl ConversationCommand {
    pub fn writes(&self) -> bool {
        matches!(self, Self::Apply(_))
    }
    pub fn bytes(&self) -> usize {
        match self {
            Self::Apply(apply) => {
                4096 + apply.pending.handoff.len()
                    + serde_json::to_string(&apply.parts).map_or(0, |value| value.len())
            }
            _ => 4096,
        }
    }
}

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

pub(crate) fn pending(
    conn: &Connection,
    chat_id: &str,
) -> rusqlite::Result<Option<PendingContinuation>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT pending_json FROM chat_continuations WHERE chat_id=?1 AND deleted=0",
            [chat_id],
            |row| row.get(0),
        )
        .optional()?;
    json.map(|json| serde_json::from_str(&json).map_err(|_| invalid("The saved continuation is damaged. Keep the source chat and create a new continuation."))).transpose()
}

pub(crate) fn execute(
    conn: &Connection,
    device: &str,
    now: i64,
    command: ConversationCommand,
) -> rusqlite::Result<ConversationReply> {
    match command {
        ConversationCommand::Pending { chat_id } => {
            let chat =
                rows::chat(conn, device, &chat_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            // A spent continuation is not a handoff: the chat would announce one forever.
            Ok(ConversationReply::Pending(
                pending(conn, &chat_id)?
                    .filter(|value| !value.is_spent())
                    .map(|value| ghost_protocol::conversation::PendingHandoff {
                        agent_id: chat.agent_id,
                        handoff: value.handoff,
                        uses_native_fork: value.native_start.fork,
                    }),
            ))
        }
        ConversationCommand::Turns { chat_id, before } => {
            let _ =
                rows::chat(conn, device, &chat_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            let mut statement = conn.prepare("SELECT t.id,t.seq,COALESCE(t.source_agent,s.agent_id,''),t.status,t.started_at FROM turns t LEFT JOIN agent_sessions s ON s.id=t.agent_session_id WHERE t.chat_id=?1 AND (?2 IS NULL OR t.seq<?2) ORDER BY t.seq DESC LIMIT 51")?;
            let mut turns = statement
                .query_map(params![chat_id, before], |row| {
                    Ok(ConversationTurn {
                        turn_id: row.get(0)?,
                        ordinal: row.get(1)?,
                        agent_id: ghost_agents::agent_of(&row.get::<_, String>(2)?).to_owned(),
                        status: row.get(3)?,
                        started_at: row.get(4)?,
                        prompt_preview: String::new(),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let has_more = turns.len() > 50;
            turns.truncate(50);
            for turn in &mut turns {
                turn.prompt_preview =
                    cut(&read_turn(conn, &chat_id, &turn.turn_id)?.prompt, 256).to_owned();
            }
            let before_ordinal = turns.last().map(|turn| turn.ordinal);
            Ok(ConversationReply::Turns(ConversationTurns {
                can_continue_here: rows::remote_history(conn, &chat_id)?,
                turns,
                before_ordinal,
                has_more,
            }))
        }
        ConversationCommand::Snapshot {
            chat_id,
            selected,
            target_agent,
            include_selected,
        } => {
            let chat =
                rows::chat(conn, device, &chat_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            let selected = selected
                .map(|id| read_turn(conn, &chat_id, &id))
                .transpose()?;
            let latest_ordinal = conn.query_row(
                "SELECT COALESCE(MAX(seq),0) FROM turns WHERE chat_id=?1",
                [&chat_id],
                |row| row.get(0),
            )?;
            let through = selected
                .as_ref()
                .map(|turn| turn.ordinal.saturating_sub(u32::from(!include_selected)))
                .unwrap_or(latest_ordinal);
            let target = target_agent.unwrap_or_else(|| {
                selected
                    .as_ref()
                    .map_or_else(|| chat.agent_id.clone(), |turn| turn.agent_id.clone())
            });
            let target_session = rows::agent_session_for(conn, &chat_id, &target)?;
            let target_last_turn = conn.query_row(
                "SELECT MAX(seq) FROM turns WHERE chat_id=?1 AND agent_session_id=?2",
                params![chat_id, target_session.as_ref().map(|session| &session.id)],
                |row| row.get(0),
            )?;
            let ids = conn
                .prepare(
                    "SELECT id FROM turns WHERE chat_id=?1 AND seq<=?2 ORDER BY seq DESC LIMIT 201",
                )?
                .query_map(params![chat_id, through], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut turns = Vec::new();
            let mut bytes = 0;
            let mut truncated = ids.len() > 200;
            for (index, id) in ids.into_iter().take(200).enumerate() {
                let mut turn = read_turn(conn, &chat_id, &id)?;
                // The handoff lists the newest commands, so only the newest turns' are read.
                if index < COMMAND_TURNS {
                    turn.commands = read_commands(conn, &chat_id, &id)?;
                }
                bytes += turn.prompt.len() + turn.answer.len();
                if bytes > 4 * 1024 * 1024 {
                    truncated = true;
                    break;
                }
                turns.push(turn);
            }
            turns.reverse();
            Ok(ConversationReply::Snapshot(Box::new(
                ConversationSnapshot {
                    remote_history: rows::remote_history(conn, &chat_id)?,
                    next_seq: next_seq(conn, &chat_id)?,
                    chat,
                    latest_ordinal,
                    turns,
                    selected,
                    target_session,
                    target_last_turn,
                    truncated,
                },
            )))
        }
        ConversationCommand::Apply(apply) => {
            let id = apply_change(conn, device, now, *apply)?;
            let load = rows::load_chat(conn, device, &id, 0)?
                .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok(ConversationReply::Applied(Box::new(load)))
        }
    }
}

fn next_seq(conn: &Connection, chat: &str) -> rusqlite::Result<u32> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq)+1,0) FROM events WHERE chat_id=?1",
        [chat],
        |row| row.get(0),
    )
}

/// How many of the newest turns the handoff reads commands from.
const COMMAND_TURNS: usize = 20;
/// The most of one turn's tool events read for its commands.
const COMMAND_EVENT_BYTES: usize = 4 * 1024 * 1024;

// A turn's rows are read through the turn's own index. `+chat_id` keeps the planner off the
// chat's `(chat_id, seq)` index, which `ORDER BY seq` would otherwise make it pick: that walks
// every row of the chat once per turn read. A 34-turn chat of 130,000 rows took 5 s to list its
// turns that way, past the store's answer limit, so the composer could not send.

/// One turn's tool rows, for its commands.
const TURN_COMMANDS_SQL: &str = "SELECT payload FROM events WHERE +chat_id=?1 AND turn_id=?2 \
     AND kind IN ('tool_call','tool_update') ORDER BY seq";
/// One turn's message rows, for its prompt and answer; one past the limit shows it was reached.
const TURN_MESSAGES_SQL: &str = "SELECT payload FROM events WHERE +chat_id=?1 AND turn_id=?2 \
     AND kind IN ('user_message_delta','message_delta','message_completed') ORDER BY seq LIMIT 10001";

/// The shell commands a turn ran, in the order they started: its tool calls folded with their
/// updates. A row that is not JSON (a salvaged one) is skipped, never the turn.
fn read_commands(
    conn: &Connection,
    chat_id: &str,
    turn_id: &str,
) -> rusqlite::Result<Vec<CommandRun>> {
    let mut calls: Vec<ghost_protocol::ToolCall> = Vec::new();
    let mut bytes = 0;
    let mut statement = conn.prepare_cached(TURN_COMMANDS_SQL)?;
    let mut rows = statement.query(params![chat_id, turn_id])?;
    while let Some(row) = rows.next()? {
        let payload: String = row.get(0)?;
        bytes += payload.len();
        if bytes > COMMAND_EVENT_BYTES {
            break;
        }
        match serde_json::from_str::<GhostEvent>(&payload) {
            Ok(GhostEvent::ToolCall(call)) => {
                match calls
                    .iter_mut()
                    .find(|known| known.tool_call_id == call.tool_call_id)
                {
                    Some(known) => *known = call,
                    None => calls.push(call),
                }
            }
            Ok(GhostEvent::ToolUpdate(update)) => {
                match calls
                    .iter()
                    .position(|known| known.tool_call_id == update.tool_call_id)
                {
                    Some(index) => {
                        let known = calls.remove(index);
                        calls.insert(index, super::compact::apply_update(known, update));
                    }
                    None => {
                        let call = super::compact::default_tool(&update.tool_call_id);
                        calls.push(super::compact::apply_update(call, update));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(calls.iter().filter_map(CommandRun::of).collect())
}

fn read_turn(conn: &Connection, chat_id: &str, turn_id: &str) -> rusqlite::Result<ContextTurn> {
    let (ordinal,agent_id,status,json,prefix): (u32,String,String,String,Option<String>) = conn.query_row("SELECT t.seq,COALESCE(t.source_agent,s.agent_id,c.active_agent,''),t.status,t.prompt_parts_json,t.continuation_prefix FROM turns t JOIN chats c ON c.id=t.chat_id LEFT JOIN agent_sessions s ON s.id=t.agent_session_id WHERE t.id=?1 AND t.chat_id=?2", params![turn_id,chat_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
    if json.len() > 2 * 1024 * 1024 {
        return Err(invalid(
            "The original prompt exceeds the continuation limit.",
        ));
    }
    let mut parts: Vec<PromptPart> = serde_json::from_str(&json)
        .map_err(|_| invalid("The original prompt is not supported by this version."))?;
    let mut prompt = String::new();
    let steering_bytes: i64 = conn.query_row("SELECT COALESCE(SUM(octet_length(parts_json)),0) FROM steering_inputs WHERE chat_id=?1 AND turn_id=?2",params![chat_id,turn_id],|row| row.get(0))?;
    if steering_bytes > 4 * 1024 * 1024 {
        return Err(invalid(
            "This turn's steering inputs exceed the continuation limit. Its original transcript is still available.",
        ));
    }
    let steering = conn.prepare("SELECT message_id,state,parts_json FROM steering_inputs WHERE chat_id=?1 AND turn_id=?2 ORDER BY seq")?
        .query_map(params![chat_id,turn_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut steering_ids = std::collections::BTreeSet::new();
    let mut accepted = std::collections::BTreeSet::new();
    // Status-only attempts may precede an unsuccessful durable input write. They are still
    // excluded from context unless the confirmed input row exists.
    for id in conn.prepare("SELECT DISTINCT json_extract(payload,'$.message_id') FROM events WHERE chat_id=?1 AND turn_id=?2 AND kind='steering_status'")?
        .query_map(params![chat_id,turn_id], |row| row.get::<_,String>(0))? {
        steering_ids.insert(id?);
    }
    for (id, state, json) in steering {
        steering_ids.insert(id.clone());
        if state == "accepted" {
            let added: Vec<PromptPart> = serde_json::from_str(&json)
                .map_err(|_| invalid("The saved steering context is damaged."))?;
            parts.extend(added);
            accepted.insert(id);
        }
    }
    let mut steering_text = BTreeMap::<String, String>::new();
    let mut messages = BTreeMap::<String, String>::new();
    let mut last_message = None;
    let mut statement = conn.prepare_cached(TURN_MESSAGES_SQL)?;
    let values = statement.query_map(params![chat_id, turn_id], |row| row.get::<_, String>(0))?;
    let mut bytes = 0;
    for (index, value) in values.enumerate() {
        let value = value?;
        bytes += value.len();
        if bytes > 4 * 1024 * 1024 || index >= 10_000 {
            return Err(invalid(
                "This turn exceeds the continuation limit. Its original transcript is still available.",
            ));
        }
        match serde_json::from_str::<GhostEvent>(&value) {
            Ok(GhostEvent::UserMessageDelta { message_id, text }) => {
                if !steering_ids.contains(&message_id) {
                    prompt.push_str(&text);
                } else if accepted.contains(&message_id) {
                    steering_text.entry(message_id).or_default().push_str(&text);
                }
            }
            Ok(GhostEvent::MessageDelta { message_id, text }) => {
                messages.entry(message_id).or_default().push_str(&text)
            }
            Ok(GhostEvent::MessageCompleted { message_id }) => last_message = Some(message_id),
            _ => {}
        }
    }
    if let Some(prefix) = prefix {
        if let Some(rest) = prompt.strip_prefix(&prefix) {
            prompt = rest.to_owned();
        }
        if matches!(parts.first(),Some(PromptPart::Text { text }) if text == &prefix) {
            parts.remove(0);
        }
    }
    // Preserve steering order from its original transcript sequence, not UUID sort order.
    for id in conn.prepare("SELECT message_id FROM steering_inputs WHERE chat_id=?1 AND turn_id=?2 AND state='accepted' ORDER BY seq")?
        .query_map(params![chat_id,turn_id], |row| row.get::<_,String>(0))? {
        if let Some(text) = steering_text.remove(&id?) { prompt.push_str("\n\n"); prompt.push_str(&text); }
    }
    let answer = if status == "completed" {
        last_message
            .and_then(|id| messages.remove(&id))
            .unwrap_or_default()
    } else {
        String::new()
    };
    Ok(ContextTurn {
        id: turn_id.into(),
        ordinal,
        agent_id: ghost_agents::agent_of(&agent_id).to_owned(),
        status,
        prompt,
        parts,
        answer,
        commands: Vec::new(),
    })
}

fn apply_change(
    conn: &Connection,
    device: &str,
    now: i64,
    apply: ApplyContinuation,
) -> rusqlite::Result<String> {
    let source = rows::chat(conn, device, &apply.source_chat)?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    if source.updated_at != apply.expected_updated_at
        || next_seq(conn, &apply.source_chat)? != apply.expected_next_seq
        || source.folder != apply.expected_folder
        || source.agent_id != apply.expected_active_agent
        || rows::agent_session_for(conn, &apply.source_chat, &apply.target_agent)?
            .map(|session| (session.id, session.native_session_id))
            != apply.expected_target_session
    {
        return Err(invalid(
            "The conversation changed after the preview. Review a new preview.",
        ));
    }
    let destination = apply
        .destination_chat
        .as_deref()
        .unwrap_or(&apply.source_chat);
    if apply.destination_chat.is_some() {
        conn.execute("INSERT INTO chats (id,project_id,folder_id,title,title_source,active_agent,created_at,updated_at,device_id,agent_host) SELECT ?2,project_id,folder_id,COALESCE(title,'Chat') || ' (fork)','manual',?3,?4,?4,?5,agent_host FROM chats WHERE id=?1 AND deleted=0", params![apply.source_chat,destination,apply.target_agent,now,device])?;
        // A fork plans and builds with what the chat it came from chose.
        conn.execute("INSERT INTO chat_roles (chat_id,role,agent,model,effort,updated_at) SELECT ?2,role,agent,model,effort,?3 FROM chat_roles WHERE chat_id=?1", params![apply.source_chat,destination,now])?;
    } else {
        conn.execute(
            "UPDATE chats SET active_agent=?2,updated_at=?3,device_id=?4 WHERE id=?1 AND deleted=0",
            params![destination, apply.target_agent, now, device],
        )?;
    }
    if let Some(folder) = &apply.destination_folder {
        if apply.destination_chat.is_some() || !apply.pending.force_fresh {
            return Err(invalid(
                "Folder relocation requires a fresh session in the same chat.",
            ));
        }
        let project: String = conn.query_row(
            "SELECT project_id FROM chats WHERE id=?1 AND deleted=0",
            [destination],
            |row| row.get(0),
        )?;
        let folder_id = super::organization::add_folder(conn, device, now, &project, folder)?;
        conn.execute(
            "UPDATE chats SET folder_id=?2 WHERE id=?1",
            params![destination, folder_id],
        )?;
    }
    // Only references selected from a stored source turn can copy chat-scoped attachment metadata.
    for part in &apply.parts {
        if let PromptPart::Image { blob, mime } = part {
            let item: Option<(String,String,u32)> = conn.query_row("SELECT display_name,mime,byte_length FROM composer_attachments WHERE chat_id=?1 AND sha256=?2 AND mime=?3 LIMIT 1", params![apply.source_chat,blob,mime], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            let (name, mime, bytes) = item.ok_or_else(|| {
                invalid("A saved image is unavailable in the source chat. Attach it again.")
            })?;
            if destination != apply.source_chat {
                conn.execute("INSERT INTO composer_attachments (id,chat_id,sha256,display_name,mime,byte_length,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(chat_id,sha256,display_name) DO NOTHING", params![crate::ids::new_id(),destination,blob,name,mime,bytes,now])?;
            }
        }
    }
    if destination != apply.source_chat {
        let text = std::iter::once(apply.pending.handoff.as_str()).chain(
            apply.parts.iter().filter_map(|part| {
                if let PromptPart::Text { text } = part {
                    Some(text.as_str())
                } else {
                    None
                }
            }),
        );
        let hashes: std::collections::BTreeSet<_> =
            text.flat_map(super::image_refs::hashes).collect();
        for hash in hashes {
            // A guessed URI is just text. Only metadata already owned by the source chat moves.
            conn.execute("INSERT INTO composer_attachments (id,chat_id,sha256,display_name,mime,byte_length,created_at) SELECT ?1,?2,sha256,display_name,mime,byte_length,?3 FROM composer_attachments WHERE chat_id=?4 AND sha256=?5 AND mime LIKE 'image/%' LIMIT 1 ON CONFLICT(chat_id,sha256,display_name) DO NOTHING", params![crate::ids::new_id(),destination,now,apply.source_chat,hash])?;
        }
    }
    let json = serde_json::to_string(&apply.pending)
        .map_err(|_| invalid("The continuation could not be saved."))?;
    conn.execute("INSERT INTO chat_continuations (chat_id,source_chat_id,source_turn_id,pending_json,created_at,device_id) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(chat_id) DO UPDATE SET source_chat_id=excluded.source_chat_id,source_turn_id=excluded.source_turn_id,pending_json=excluded.pending_json,created_at=excluded.created_at,device_id=excluded.device_id,deleted=0", params![destination,apply.source_chat,apply.source_turn,json,now,device])?;
    Ok(destination.into())
}

#[cfg(test)]
mod image_tests {
    use super::*;
    use crate::{paths::DataRoot, profile::Profiles, store::Store};

    #[test]
    fn fork_retains_owned_inline_images_without_adopting_guessed_hashes() {
        let scratch = tempfile::tempdir().unwrap();
        let profile = Profiles::new(DataRoot::at(scratch.path()))
            .create("Local")
            .unwrap();
        let (store, _) = Store::open(&profile).unwrap();
        let create = |folder: &str| {
            rows::create_chat(
                store.conn(),
                &super::super::NewChat {
                    folder: folder.into(),
                    agent_id: "codex".into(),
                    agent_host: ghost_protocol::agents::AgentHost::Native,
                    device_id: "device".into(),
                    now: 1,
                },
            )
            .unwrap()
        };
        let source = create("/source");
        let other = create("/other");
        let owned = "a".repeat(64);
        let foreign = "b".repeat(64);
        for (chat, hash) in [(&source.id, &owned), (&other.id, &foreign)] {
            store.conn().execute("INSERT INTO composer_attachments(id,chat_id,sha256,display_name,mime,byte_length,created_at) VALUES(?1,?2,?3,'Agent image','image/png',20,1)",params![crate::ids::new_id(),chat,hash]).unwrap();
        }
        store.conn().execute("INSERT INTO chat_roles(chat_id,role,agent,model,effort,updated_at) VALUES(?1,'build','claude','claude-fable-5-2','high',1)", params![source.id]).unwrap();
        let destination = crate::ids::new_id();
        apply_change(
            store.conn(),
            "device",
            2,
            ApplyContinuation {
                source_chat: source.id.clone(),
                source_turn: None,
                expected_next_seq: 0,
                expected_updated_at: source.updated_at,
                expected_folder: source.folder,
                expected_active_agent: "codex".into(),
                expected_target_session: None,
                target_agent: "codex".into(),
                destination_chat: Some(destination.clone()),
                destination_folder: None,
                pending: PendingContinuation {
                    handoff: format!(
                        "![Owned](<aura-image:{owned}>) ![Guessed](<aura-image:{foreign}>)"
                    ),
                    ..Default::default()
                },
                parts: vec![PromptPart::Text {
                    text: format!("aura-image:{foreign}"),
                }],
            },
        )
        .unwrap();
        let hashes: Vec<String> = store
            .conn()
            .prepare("SELECT sha256 FROM composer_attachments WHERE chat_id=?1")
            .unwrap()
            .query_map([&destination], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(hashes, vec![owned]);
        let role: (String, String, Option<String>) = store
            .conn()
            .query_row(
                "SELECT role,agent,model FROM chat_roles WHERE chat_id=?1",
                [&destination],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            role,
            (
                "build".into(),
                "claude".into(),
                Some("claude-fable-5-2".into())
            )
        );
    }
}

#[cfg(test)]
mod turn_read_tests {
    use super::*;
    use crate::{paths::DataRoot, profile::Profiles, store::Store};

    /// A turn's rows come through the turn's index, never a walk through the whole chat: listing a
    /// long chat's turns ran past the store's 5 s answer limit that way.
    #[test]
    fn turn_rows_are_read_through_the_turn_index() {
        let scratch = tempfile::tempdir().unwrap();
        let profile = Profiles::new(DataRoot::at(scratch.path()))
            .create("Local")
            .unwrap();
        let (store, _) = Store::open(&profile).unwrap();
        for sql in [TURN_MESSAGES_SQL, TURN_COMMANDS_SQL] {
            let plan = store
                .conn()
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(params!["chat", "turn"], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join("; ");
            assert!(plan.contains("USING INDEX events_turn_id"), "{sql}: {plan}");
        }
    }
}
