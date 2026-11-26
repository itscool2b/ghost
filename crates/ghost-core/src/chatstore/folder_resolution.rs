//! Device-only code roots and folder locations, on the existing writer connection.
use ghost_protocol::workspace::FolderKind;
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) enum Command {
    Roots(Option<Vec<String>>),
    Locate {
        folder_id: String,
        path: String,
        previous: Option<String>,
        kind: FolderKind,
        remote: Option<String>,
    },
}
pub(crate) enum Reply {
    Roots(Vec<String>),
    Located(super::organization::Organization),
}
impl Command {
    pub(crate) fn writes(&self) -> bool {
        !matches!(self, Self::Roots(None))
    }
    pub(crate) fn bytes(&self) -> usize {
        (match self {
            Self::Roots(roots) => roots
                .as_ref()
                .map_or(0, |roots| roots.iter().map(String::len).sum()),
            Self::Locate {
                folder_id,
                path,
                previous,
                remote,
                ..
            } => {
                folder_id.len()
                    + path.len()
                    + previous.as_ref().map_or(0, String::len)
                    + remote.as_ref().map_or(0, String::len)
            }
        }) + 256
    }
}
fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}
pub(crate) fn execute(
    conn: &Connection,
    device: &str,
    now: i64,
    command: Command,
) -> rusqlite::Result<Reply> {
    match command {
        Command::Roots(save) => {
            // Settings are an explicit local allowlist, not portable metadata.
            let key = format!("device/{device}/code-roots-v1");
            if let Some(roots) = save {
                if roots.len() > 16
                    || roots.iter().any(|path| {
                        path.is_empty()
                            || path.len() > 32768
                            || !std::path::Path::new(path).is_absolute()
                    })
                {
                    return Err(invalid("Choose at most 16 absolute code roots."));
                }
                let value = serde_json::to_string(&roots)
                    .map_err(|_| invalid("Code roots could not be saved."))?;
                conn.execute("INSERT INTO settings(key,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",params![key,value,now])?;
                return Ok(Reply::Roots(roots));
            }
            let value: Option<String> = conn
                .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                    row.get(0)
                })
                .optional()?;
            let roots: Vec<String> = value
                .map(|value| {
                    serde_json::from_str(&value)
                        .map_err(|_| invalid("Saved code roots could not be read."))
                })
                .transpose()?
                .unwrap_or_default();
            if roots.len() > 16
                || roots.iter().any(|path| {
                    path.len() > 32768
                        || !std::path::Path::new(path).is_absolute()
                        || path.chars().any(char::is_control)
                })
            {
                return Err(invalid("Saved code roots exceed their limit."));
            }
            Ok(Reply::Roots(roots))
        }
        Command::Locate {
            folder_id,
            path,
            previous,
            kind,
            remote,
        } => {
            let path = super::stored_path_text(&path);
            let current:(String,Option<String>,Option<String>)=conn.query_row("SELECT pf.kind,pf.git_remote_url,fl.path FROM project_folders pf JOIN projects p ON p.id=pf.project_id LEFT JOIN folder_locations fl ON fl.folder_id=pf.id AND fl.device_id=?2 WHERE pf.id=?1 AND pf.deleted=0 AND p.deleted=0",params![folder_id,device],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
            let expected = if kind == FolderKind::Git {
                "git"
            } else {
                "local"
            };
            if current != (expected.into(), remote, previous.clone()) {
                return Err(invalid("The folder changed. Review its location again."));
            }
            if previous.as_deref() != Some(&path) {
                let native:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_sessions s JOIN chats c ON c.id=s.chat_id WHERE c.folder_id=?1 AND s.device_id=?2)",params![folder_id,device],|row|row.get(0))?;
                if native {
                    return Err(invalid(
                        "This folder has native agent history on this device. Use a reviewed chat handoff or a new chat instead of changing its location.",
                    ));
                }
            }
            conn.execute("INSERT INTO folder_locations(folder_id,device_id,path,last_seen_at) VALUES(?1,?2,?3,?4) ON CONFLICT(folder_id,device_id) DO UPDATE SET path=excluded.path,last_seen_at=excluded.last_seen_at",params![folder_id,device,path,now])?;
            let organization = super::organization::execute(
                conn,
                device,
                now,
                super::organization::OrganizationCommand::Snapshot(
                    ghost_protocol::workspace::WorkspaceFilter {
                        include_archived: true,
                        project_id: None,
                    },
                ),
            )?;
            Ok(Reply::Located(organization))
        }
    }
}
