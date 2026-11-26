//! Helpers for the chat store's unit tests: a migrated database in a temporary profile, and rows.

use std::path::PathBuf;

use ghost_protocol::GhostEvent;
use rusqlite::{Connection, params};

use super::{EventRow, NewChat, rows};
use crate::paths::DataRoot;
use crate::profile::Profiles;
use crate::store::Store;

/// A migrated store in a temporary profile.
pub(crate) struct TestDb {
    _dir: tempfile::TempDir,
    pub(crate) store: Store,
}

impl TestDb {
    pub(crate) fn new() -> TestDb {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = Profiles::new(DataRoot::at(dir.path().join("data")))
            .open_active()
            .expect("profile");
        let (store, _) = Store::open(&profile).expect("store");
        TestDb { _dir: dir, store }
    }

    pub(crate) fn conn(&self) -> &Connection {
        self.store.conn()
    }

    /// A new chat in `folder`; returns its id.
    pub(crate) fn chat(&self, folder: &str) -> String {
        rows::create_chat(
            self.conn(),
            &NewChat {
                agent_host: Default::default(),
                agent_id: "mock".into(),
                folder: PathBuf::from(folder),
                device_id: "dev".into(),
                now: 1,
            },
        )
        .expect("chat")
        .id
    }

    /// A running turn row (no events).
    pub(crate) fn turn(&self, chat: &str, turn_id: &str, ordinal: u32, first_event_seq: u32) {
        self.conn()
            .execute(
                "INSERT INTO turns (id, chat_id, seq, status, started_at, device_id, first_event_seq) \
                 VALUES (?1, ?2, ?3, 'running', 1, 'dev', ?4)",
                params![turn_id, chat, ordinal, first_event_seq],
            )
            .expect("turn");
    }

    pub(crate) fn append(&self, chat: &str, rows: &[EventRow]) {
        rows::append_events(self.conn(), "dev", chat, rows).expect("append");
    }

    /// Every stored row of the chat as `(seq, turn_id, event)`, in seq order.
    pub(crate) fn events(&self, chat: &str) -> Vec<(u32, Option<String>, GhostEvent)> {
        let mut stmt = self
            .conn()
            .prepare("SELECT seq, turn_id, payload FROM events WHERE chat_id = ?1 ORDER BY seq")
            .expect("prepare");
        stmt.query_map(params![chat], |row| {
            let payload: String = row.get(2)?;
            Ok((
                row.get(0)?,
                row.get(1)?,
                serde_json::from_str(&payload).expect("stored events parse"),
            ))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }
}

/// The JSON of an event, as stored.
pub(crate) fn payload(event: &GhostEvent) -> String {
    serde_json::to_string(event).expect("serialize")
}

/// A stored row for `event`.
pub(crate) fn event_row(seq: u32, turn_id: Option<&str>, event: &GhostEvent) -> EventRow {
    EventRow {
        seq,
        turn_id: turn_id.map(str::to_owned),
        kind: event.kind(),
        payload: payload(event),
        created_at: i64::from(seq),
    }
}
