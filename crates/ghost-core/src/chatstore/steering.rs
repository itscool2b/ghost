//! Durable user inputs. Native request IDs and pending control handles never enter storage.
use ghost_protocol::{PromptPart, composer::SteeringState};
use rusqlite::{Connection, params};

pub(crate) enum Command {
    Begin {
        chat_id: String,
        turn_id: String,
        message_id: String,
        seq: u32,
        parts: Vec<PromptPart>,
    },
    Finish {
        chat_id: String,
        message_id: String,
        state: SteeringState,
        detail: Option<String>,
    },
}

impl Command {
    pub fn bytes(&self) -> usize {
        match self {
            Self::Begin { parts, .. } => {
                serde_json::to_vec(parts).map_or(4 * 1024 * 1024, |value| value.len()) + 1024
            }
            Self::Finish { detail, .. } => detail.as_ref().map_or(0, String::len) + 1024,
        }
    }
}

pub(crate) fn state_name(state: SteeringState) -> &'static str {
    match state {
        SteeringState::Pending => "pending",
        SteeringState::Accepted => "accepted",
        SteeringState::Rejected => "rejected",
        SteeringState::Unconfirmed => "unconfirmed",
    }
}

pub(crate) fn execute(conn: &Connection, now: i64, command: Command) -> rusqlite::Result<()> {
    match command {
        Command::Begin {
            chat_id,
            turn_id,
            message_id,
            seq,
            parts,
        } => {
            let json = serde_json::to_string(&parts)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            let changed = conn.execute("INSERT INTO steering_inputs (message_id,chat_id,turn_id,seq,parts_json,state,created_at,updated_at) SELECT ?1,?2,?3,?4,?5,'pending',?6,?6 WHERE EXISTS(SELECT 1 FROM turns WHERE id=?3 AND chat_id=?2)", params![message_id,chat_id,turn_id,seq,json,now])?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
        }
        Command::Finish {
            chat_id,
            message_id,
            state,
            detail,
        } => {
            if state == SteeringState::Pending {
                return Err(rusqlite::Error::InvalidParameterName(
                    "A final steering outcome is required.".into(),
                ));
            }
            let changed = conn.execute("UPDATE steering_inputs SET state=?3,detail=?4,updated_at=?5 WHERE message_id=?1 AND chat_id=?2 AND state='pending'", params![message_id,chat_id,state_name(state),detail,now])?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatstore::{
        compact::CompactLimits,
        recover::{Scope, recover},
        test_support::{TestDb, event_row},
    };
    use ghost_protocol::GhostEvent;

    #[test]
    fn steering_recovery_restores_saved_acknowledgements_and_marks_pending_unconfirmed() {
        let mut db = TestDb::new();
        let chat = db.chat("/project");
        db.turn(&chat, "turn", 1, 0);
        for (index, id) in ["accepted", "pending", "unsaved"].into_iter().enumerate() {
            let seq = index as u32;
            db.append(
                &chat,
                &[event_row(
                    seq,
                    Some("turn"),
                    &GhostEvent::SteeringStatus {
                        message_id: id.into(),
                        state: SteeringState::Pending,
                        detail: None,
                    },
                )],
            );
            if id != "unsaved" {
                execute(
                    db.conn(),
                    1,
                    Command::Begin {
                        chat_id: chat.clone(),
                        turn_id: "turn".into(),
                        message_id: id.into(),
                        seq,
                        parts: vec![PromptPart::Text {
                            text: "Visible input".into(),
                        }],
                    },
                )
                .unwrap();
            }
        }
        execute(
            db.conn(),
            2,
            Command::Finish {
                chat_id: chat.clone(),
                message_id: "accepted".into(),
                state: SteeringState::Accepted,
                detail: None,
            },
        )
        .unwrap();
        recover(
            db.store.conn_mut(),
            Scope::All,
            "dev",
            &CompactLimits::STANDARD,
            3,
        )
        .unwrap();
        let outcomes = db
            .events(&chat)
            .into_iter()
            .filter_map(|(_, _, event)| match event {
                GhostEvent::SteeringStatus {
                    message_id, state, ..
                } if state != SteeringState::Pending => Some((message_id, state)),
                _ => None,
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(outcomes.get("accepted"), Some(&SteeringState::Accepted));
        assert_eq!(outcomes.get("pending"), Some(&SteeringState::Unconfirmed));
        assert_eq!(outcomes.get("unsaved"), Some(&SteeringState::Unconfirmed));
        let count = db.events(&chat).len();
        recover(
            db.store.conn_mut(),
            Scope::All,
            "dev",
            &CompactLimits::STANDARD,
            4,
        )
        .unwrap();
        assert_eq!(db.events(&chat).len(), count, "recovery is idempotent");
    }

    #[test]
    fn steering_store_refuses_cross_chat_turns_and_repeated_final_outcomes() {
        let db = TestDb::new();
        let chat = db.chat("/project");
        let other = db.chat("/other");
        db.turn(&chat, "turn", 1, 0);
        let begin = |chat_id| Command::Begin {
            chat_id,
            turn_id: "turn".into(),
            message_id: "message".into(),
            seq: 0,
            parts: vec![],
        };
        assert!(execute(db.conn(), 1, begin(other)).is_err());
        execute(db.conn(), 1, begin(chat.clone())).unwrap();
        let finish = || Command::Finish {
            chat_id: chat.clone(),
            message_id: "message".into(),
            state: SteeringState::Rejected,
            detail: None,
        };
        execute(db.conn(), 2, finish()).unwrap();
        assert!(execute(db.conn(), 3, finish()).is_err());
    }
}
