//! The agents' model catalogs (`session::catalogs`) as one device-only setting, on the existing
//! writer connection. Like the code roots, the row is this device's own: another device runs other
//! agent versions.
use rusqlite::{Connection, OptionalExtension, params};

/// The saved record is at most this large; a larger one is not written.
pub(crate) const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(crate) enum Command {
    Load,
    Save(String),
}

impl Command {
    pub(crate) fn writes(&self) -> bool {
        matches!(self, Self::Save(_))
    }
    pub(crate) fn bytes(&self) -> usize {
        match self {
            Self::Load => 256,
            Self::Save(json) => json.len() + 256,
        }
    }
}

fn key(device: &str) -> String {
    format!("device/{device}/agent-catalogs-v1")
}

/// `Load` answers the saved JSON, if any; `Save` answers `None`.
pub(crate) fn execute(
    conn: &Connection,
    device: &str,
    now: i64,
    command: Command,
) -> rusqlite::Result<Option<String>> {
    match command {
        Command::Load => conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                [key(device)],
                |row| row.get(0),
            )
            .optional(),
        Command::Save(json) => {
            if json.len() > MAX_BYTES {
                return Err(rusqlite::Error::InvalidParameterName(
                    "The agents' model lists are too large to save.".into(),
                ));
            }
            conn.execute(
                "INSERT INTO settings(key,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
                params![key(device), json, now],
            )?;
            Ok(None)
        }
    }
}
