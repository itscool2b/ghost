//! Composer metadata on the profile writer. Image hashes require matching chat-owned metadata.
use super::{StoreError, files::Attachments, rows};
use ghost_protocol::{
    PromptPart,
    composer::{ChatImageContext, ComposerAttachment, ComposerInput},
};
use rusqlite::{Connection, OptionalExtension as _, params};
use std::sync::Arc;

pub(crate) enum ComposerCommand {
    ResolveImage {
        chat_id: String,
        sha256: String,
    },
    ImageParts {
        chat_id: String,
        context: ChatImageContext,
    },
    ReadDraft {
        chat_id: String,
    },
    SaveDraft {
        chat_id: String,
        input: ComposerInput,
    },
    Import {
        chat_id: String,
        name: String,
        mime: String,
        sha256: String,
        bytes: Arc<[u8]>,
    },
    Resolve {
        chat_id: String,
        ids: Vec<String>,
    },
}

pub(crate) enum ComposerReply {
    ImageParts(Vec<PromptPart>),
    Attachments(Vec<StoredAttachment>),
    Draft(ComposerInput),
}

pub(crate) struct StoredAttachment {
    pub meta: ComposerAttachment,
    pub sha256: String,
}

impl ComposerCommand {
    pub fn bytes(&self) -> usize {
        match self {
            Self::ResolveImage { .. } => 1024,
            Self::ImageParts { .. } => 1024,
            Self::ReadDraft { .. } => 512,
            Self::SaveDraft { input, .. } => {
                input.text.len()
                    + input
                        .attachments
                        .iter()
                        .chain(&input.mentions)
                        .map(String::len)
                        .sum::<usize>()
                    + 512
            }
            Self::Import { bytes, .. } => bytes.len() + 2048,
            Self::Resolve { ids, .. } => ids.iter().map(String::len).sum::<usize>() + 512,
        }
    }
}

pub(crate) fn execute(
    conn: &Connection,
    files: &Attachments,
    now: i64,
    command: ComposerCommand,
) -> Result<ComposerReply, StoreError> {
    let sql = |error: rusqlite::Error| {
        StoreError::Other(format!("Could not save attachment metadata: {error}"))
    };
    match command {
        ComposerCommand::ResolveImage { chat_id, sha256 } => {
            let item = conn.query_row("SELECT id,display_name,mime,byte_length,sha256 FROM composer_attachments WHERE chat_id=?1 AND sha256=?2 AND mime IN ('image/png','image/jpeg') LIMIT 1", params![chat_id,sha256], |row| {
                Ok(StoredAttachment { meta: ComposerAttachment { id: row.get(0)?, name: row.get(1)?, mime: row.get(2)?, bytes: row.get(3)?, image: true }, sha256: row.get(4)? })
            }).map_err(|_| StoreError::Other("This image is unavailable in this chat.".into()))?;
            Ok(ComposerReply::Attachments(vec![item]))
        }
        ComposerCommand::ImageParts { chat_id, context } => {
            let (query, id) = match context {
                ChatImageContext::Turn { turn_id } => (
                    "SELECT prompt_parts_json FROM turns WHERE id=?1 AND chat_id=?2",
                    turn_id,
                ),
                ChatImageContext::Steering { message_id } => (
                    "SELECT parts_json FROM steering_inputs WHERE message_id=?1 AND chat_id=?2",
                    message_id,
                ),
                ChatImageContext::Queued { prompt_id } => (
                    "SELECT parts_json FROM queued_prompts WHERE id=?1 AND chat_id=?2",
                    prompt_id,
                ),
            };
            let json: String = conn
                .query_row(query, params![id, chat_id], |row| row.get(0))
                .map_err(|_| {
                    StoreError::Other("The message images are unavailable in this chat.".into())
                })?;
            if json.len() > 4 * 1024 * 1024 {
                return Err(StoreError::Other(
                    "The saved message exceeds the image preview limit.".into(),
                ));
            }
            let parts: Vec<PromptPart> = serde_json::from_str(&json).map_err(|_| {
                StoreError::Other("The saved message images could not be read.".into())
            })?;
            // Text and mentions never leave this query through the image API.
            Ok(ComposerReply::ImageParts(
                parts
                    .into_iter()
                    .filter(|part| matches!(part, PromptPart::Image { .. }))
                    .take(8)
                    .collect(),
            ))
        }
        ComposerCommand::ReadDraft { chat_id } => {
            let json: Option<String> = conn
                .query_row(
                    "SELECT input_json FROM composer_drafts WHERE chat_id=?1",
                    [&chat_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sql)?;
            let input = match json {
                Some(json) => serde_json::from_str(&json)
                    .map_err(|_| StoreError::Other("The saved draft could not be read.".into()))?,
                None => ComposerInput::default(),
            };
            Ok(ComposerReply::Draft(input))
        }
        ComposerCommand::SaveDraft { chat_id, input } => {
            let json = serde_json::to_string(&input)
                .map_err(|_| StoreError::Other("The draft could not be encoded.".into()))?;
            conn.execute("INSERT INTO composer_drafts(chat_id,input_json,updated_at) VALUES (?1,?2,?3) ON CONFLICT(chat_id) DO UPDATE SET input_json=excluded.input_json,updated_at=excluded.updated_at", params![chat_id,json,now]).map_err(sql)?;
            Ok(ComposerReply::Draft(input))
        }
        ComposerCommand::Import {
            chat_id,
            name,
            mime,
            sha256,
            bytes,
        } => {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1 AND deleted=0)",
                    [&chat_id],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            if !exists {
                return Err(StoreError::Other("This chat no longer exists.".into()));
            }
            let size = files.write(&sha256, &bytes).map_err(|error| {
                StoreError::Other(format!("Could not save attachment: {error}"))
            })?;
            rows::insert_attachment(conn, &sha256, &mime, size, now).map_err(sql)?;
            let id = crate::ids::new_id();
            conn.execute("INSERT INTO composer_attachments (id,chat_id,sha256,display_name,mime,byte_length,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(chat_id,sha256,display_name) DO NOTHING", params![id, chat_id, sha256, name, mime, size as i64, now]).map_err(sql)?;
            let actual: String = conn.query_row("SELECT id FROM composer_attachments WHERE chat_id=?1 AND sha256=?2 AND display_name=?3", params![chat_id,sha256,name], |row| row.get(0)).map_err(sql)?;
            Ok(ComposerReply::Attachments(vec![StoredAttachment {
                meta: ComposerAttachment {
                    id: actual,
                    name,
                    image: mime.starts_with("image/"),
                    mime,
                    bytes: size as u32,
                },
                sha256,
            }]))
        }
        ComposerCommand::Resolve { chat_id, ids } => {
            let mut result = Vec::with_capacity(ids.len());
            for id in ids {
                let item = conn.query_row("SELECT id,display_name,mime,byte_length,sha256 FROM composer_attachments WHERE id=?1 AND chat_id=?2", params![id,chat_id], |row| {
                    let mime: String = row.get(2)?;
                    Ok(StoredAttachment { meta: ComposerAttachment { id: row.get(0)?, name: row.get(1)?, image: mime.starts_with("image/"), mime, bytes: row.get(3)? }, sha256: row.get(4)? })
                }).map_err(|_| StoreError::Other("An attachment is unavailable in this chat. Attach it again.".into()))?;
                result.push(item);
            }
            Ok(ComposerReply::Attachments(result))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatstore::test_support::TestDb;

    #[test]
    fn message_images_resolve_only_from_the_owning_chat_and_never_return_text() {
        let db = TestDb::new();
        let chat = db.chat("work");
        let other = db.chat("other");
        db.turn(&chat, "turn", 1, 1);
        let json = r#"[{"type":"text","text":"private words"},{"type":"image","blob":"abc","mime":"image/png"}]"#;
        db.conn()
            .execute(
                "UPDATE turns SET prompt_parts_json=?1 WHERE id='turn'",
                [json],
            )
            .unwrap();
        db.conn().execute("INSERT INTO queued_prompts (id,chat_id,text,parts_json,created_at,device_id) VALUES ('queue',?1,'text',?2,1,'dev')", params![chat,json]).unwrap();
        db.conn().execute("INSERT INTO steering_inputs (message_id,chat_id,turn_id,seq,parts_json,state,created_at,updated_at) VALUES ('steer',?1,'turn',1,?2,'pending',1,1)", params![chat,json]).unwrap();
        let files = Attachments::new(std::path::PathBuf::from("unused"));
        for context in [
            ChatImageContext::Turn {
                turn_id: "turn".into(),
            },
            ChatImageContext::Queued {
                prompt_id: "queue".into(),
            },
            ChatImageContext::Steering {
                message_id: "steer".into(),
            },
        ] {
            let ComposerReply::ImageParts(parts) = execute(
                db.conn(),
                &files,
                1,
                ComposerCommand::ImageParts {
                    chat_id: chat.clone(),
                    context: context.clone(),
                },
            )
            .unwrap() else {
                panic!("wrong reply")
            };
            assert_eq!(parts.len(), 1);
            assert!(matches!(&parts[0], PromptPart::Image { blob, .. } if blob == "abc"));
            assert!(
                execute(
                    db.conn(),
                    &files,
                    1,
                    ComposerCommand::ImageParts {
                        chat_id: other.clone(),
                        context
                    }
                )
                .is_err()
            );
        }
    }
}
