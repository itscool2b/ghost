//! Reversible parent tombstones, written in the profile writer's transaction.
use ghost_protocol::workspace_removal::{
    RemovedWorkspace, WorkspaceRemovalAction, WorkspaceRemovalTarget,
};
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub target: WorkspaceRemovalTarget,
    pub name: String,
    pub project_name: String,
    pub deleted: bool,
    pub parent_deleted: bool,
    pub updated_at: i64,
    pub folders: Vec<(String, Option<String>, i64, bool)>,
    pub chats: Vec<(String, i64)>,
}
pub(crate) enum Command {
    List,
    Inspect(WorkspaceRemovalTarget),
    Apply {
        before: Snapshot,
        action: WorkspaceRemovalAction,
    },
}
pub(crate) enum Reply {
    List(Vec<RemovedWorkspace>),
    Snapshot(Snapshot),
    Applied,
}
impl Command {
    pub fn writes(&self) -> bool {
        matches!(self, Self::Apply { .. })
    }
    pub fn bytes(&self) -> usize {
        match self {
            Self::List => 256,
            Self::Inspect(target) => 256 + target_id(target).len(),
            Self::Apply { before, .. } => {
                1024 + before
                    .folders
                    .iter()
                    .map(|(id, path, _, _)| id.len() + path.as_ref().map_or(0, String::len))
                    .sum::<usize>()
                    + before
                        .chats
                        .iter()
                        .map(|(id, _)| id.len() + 32)
                        .sum::<usize>()
            }
        }
    }
}
pub(crate) fn target_id(target: &WorkspaceRemovalTarget) -> &str {
    match target {
        WorkspaceRemovalTarget::Project { id } | WorkspaceRemovalTarget::Folder { id } => id,
    }
}
fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}
pub(crate) fn inspect(
    conn: &Connection,
    device: &str,
    target: WorkspaceRemovalTarget,
) -> rusqlite::Result<Snapshot> {
    let id = target_id(&target);
    let project = matches!(target, WorkspaceRemovalTarget::Project { .. });
    let (name, project_name, deleted, parent_deleted, updated_at): (
        String,
        String,
        bool,
        bool,
        i64,
    ) = if project {
        conn.query_row(
            "SELECT name,name,deleted,0,updated_at FROM projects WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?
    } else {
        conn.query_row("SELECT f.display_name,p.name,f.deleted,p.deleted,f.updated_at FROM project_folders f JOIN projects p ON p.id=f.project_id WHERE f.id=?1",[id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?
    };
    let folders = conn.prepare("SELECT f.id,l.path,f.updated_at,f.deleted FROM project_folders f LEFT JOIN folder_locations l ON l.folder_id=f.id AND l.device_id=?2 WHERE (?3 AND f.project_id=?1) OR (NOT ?3 AND f.id=?1) ORDER BY f.id LIMIT 1025")?
        .query_map(params![id,device,project],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let chats = conn.prepare("SELECT id,updated_at FROM chats WHERE deleted=0 AND ((?2 AND project_id=?1) OR (NOT ?2 AND folder_id=?1)) ORDER BY id LIMIT 10001")?
        .query_map(params![id,project],|r| Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if folders.len() > 1024 || chats.len() > 10000 {
        return Err(invalid(
            "This removal exceeds the review limit of 1,024 folders or 10,000 chats.",
        ));
    }
    Ok(Snapshot {
        target,
        name,
        project_name,
        deleted,
        parent_deleted,
        updated_at,
        folders,
        chats,
    })
}
pub(crate) fn execute(
    conn: &Connection,
    device: &str,
    now: i64,
    command: Command,
) -> rusqlite::Result<Reply> {
    match command {
        Command::Inspect(target) => Ok(Reply::Snapshot(inspect(conn, device, target)?)),
        Command::List => {
            let mut removed = Vec::new();
            let mut query=conn.prepare("SELECT id,name,name,updated_at,0,(SELECT COUNT(*) FROM chats WHERE project_id=p.id AND deleted=0) FROM projects p WHERE deleted=1 ORDER BY updated_at DESC,id LIMIT 1000")?;
            for row in query.query_map([], |r| {
                Ok(RemovedWorkspace {
                    target: WorkspaceRemovalTarget::Project { id: r.get(0)? },
                    name: r.get(1)?,
                    project_name: r.get(2)?,
                    removed_at: r.get(3)?,
                    restore_unavailable: None,
                    chat_count: r.get(5)?,
                })
            })? {
                removed.push(row?);
            }
            let mut query=conn.prepare("SELECT f.id,f.display_name,p.name,f.updated_at,p.deleted,(SELECT COUNT(*) FROM chats WHERE folder_id=f.id AND deleted=0) FROM project_folders f JOIN projects p ON p.id=f.project_id WHERE f.deleted=1 ORDER BY f.updated_at DESC,f.id LIMIT 1000")?;
            for row in query.query_map([], |r| {
                Ok(RemovedWorkspace {
                    target: WorkspaceRemovalTarget::Folder { id: r.get(0)? },
                    name: r.get(1)?,
                    project_name: r.get(2)?,
                    removed_at: r.get(3)?,
                    restore_unavailable: r
                        .get::<_, bool>(4)?
                        .then(|| "Restore this folder's project first.".into()),
                    chat_count: r.get(5)?,
                })
            })? {
                removed.push(row?);
            }
            Ok(Reply::List(removed))
        }
        Command::Apply { before, action } => {
            let current = inspect(conn, device, before.target.clone())?;
            if current != before {
                return Err(invalid(
                    "The project or its chats changed. Review removal again.",
                ));
            }
            let remove = action == WorkspaceRemovalAction::Remove;
            if current.deleted == remove || current.parent_deleted {
                return Err(invalid(
                    "This item's removal state changed. Refresh the removed-items list.",
                ));
            }
            for (chat, _) in &current.chats {
                let busy:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM turns WHERE chat_id=?1 AND status='running') OR (?2 AND EXISTS(SELECT 1 FROM queued_prompts WHERE chat_id=?1)) OR EXISTS(SELECT 1 FROM steering_inputs WHERE chat_id=?1 AND state='pending')",params![chat,remove],|r|r.get(0))?;
                if busy {
                    return Err(invalid(
                        "Stop affected chats and clear queued prompts before changing this workspace.",
                    ));
                }
            }
            let table = match current.target {
                WorkspaceRemovalTarget::Project { .. } => "projects",
                WorkspaceRemovalTarget::Folder { .. } => "project_folders",
            };
            conn.execute(
                &format!("UPDATE {table} SET deleted=?2,updated_at=?3,device_id=?4 WHERE id=?1"),
                params![target_id(&current.target), remove, now, device],
            )?;
            Ok(Reply::Applied)
        }
    }
}

/// Used by launch/dispatch admission. Reading history remains possible after removal.
pub(crate) fn available(conn: &Connection, chat_id: &str) -> rusqlite::Result<bool> {
    Ok(conn.query_row("SELECT p.deleted=0 AND (c.folder_id IS NULL OR f.deleted=0) FROM chats c JOIN projects p ON p.id=c.project_id LEFT JOIN project_folders f ON f.id=c.folder_id WHERE c.id=?1 AND c.deleted=0",[chat_id],|r| r.get(0)).optional()?.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatstore::{rows, test_support::TestDb};
    fn target(db: &TestDb, chat: &str, project: bool) -> WorkspaceRemovalTarget {
        let (p, f): (String, String) = db
            .conn()
            .query_row(
                "SELECT project_id,folder_id FROM chats WHERE id=?1",
                [chat],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        if project {
            WorkspaceRemovalTarget::Project { id: p }
        } else {
            WorkspaceRemovalTarget::Folder { id: f }
        }
    }
    fn change(db: &TestDb, target: WorkspaceRemovalTarget, action: WorkspaceRemovalAction) {
        let before = inspect(db.conn(), "dev", target).unwrap();
        execute(db.conn(), "dev", 10, Command::Apply { before, action }).unwrap();
    }
    #[test]
    fn removal_hides_and_restores_same_history_and_location_without_changing_archive_state() {
        for project in [false, true] {
            let db = TestDb::new();
            let chat = db.chat("/owned/project");
            let target = target(&db, &chat, project);
            db.turn(&chat, "retained-turn", 1, 0);
            db.conn()
                .execute(
                    "UPDATE turns SET status='completed' WHERE id='retained-turn'",
                    [],
                )
                .unwrap();
            change(&db, target.clone(), WorkspaceRemovalAction::Remove);
            assert!(!available(db.conn(), &chat).unwrap());
            assert!(rows::chats(db.conn(), "dev").unwrap().is_empty());
            assert_eq!(
                rows::chat(db.conn(), "dev", &chat).unwrap().unwrap().folder,
                std::path::PathBuf::from("/owned/project")
            );
            assert_eq!(
                db.conn()
                    .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                1
            );
            let Reply::List(list) = execute(db.conn(), "dev", 11, Command::List).unwrap() else {
                panic!()
            };
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].chat_count, 1);
            change(&db, target, WorkspaceRemovalAction::Restore);
            assert!(available(db.conn(), &chat).unwrap());
            assert_eq!(rows::chats(db.conn(), "dev").unwrap().len(), 1);
        }
    }
    #[test]
    fn stale_review_and_active_turn_are_refused_without_a_tombstone() {
        let db = TestDb::new();
        let chat = db.chat("/owned/project");
        let target = target(&db, &chat, true);
        let before = inspect(db.conn(), "dev", target.clone()).unwrap();
        db.chat("/owned/project");
        assert!(
            execute(
                db.conn(),
                "dev",
                2,
                Command::Apply {
                    before,
                    action: WorkspaceRemovalAction::Remove
                }
            )
            .is_err()
        );
        let before = inspect(db.conn(), "dev", target).unwrap();
        db.turn(&chat, "running", 1, 0);
        assert!(
            execute(
                db.conn(),
                "dev",
                2,
                Command::Apply {
                    before,
                    action: WorkspaceRemovalAction::Remove
                }
            )
            .is_err()
        );
        assert!(available(db.conn(), &chat).unwrap());
    }
    #[test]
    fn restoring_project_preserves_previously_removed_folder_and_archived_chat() {
        let db = TestDb::new();
        let chat = db.chat("/owned/project");
        db.conn()
            .execute("UPDATE chats SET archived=1 WHERE id=?1", [&chat])
            .unwrap();
        let folder = target(&db, &chat, false);
        let project = target(&db, &chat, true);
        change(&db, folder.clone(), WorkspaceRemovalAction::Remove);
        change(&db, project.clone(), WorkspaceRemovalAction::Remove);
        let before = inspect(db.conn(), "dev", folder.clone()).unwrap();
        assert!(
            execute(
                db.conn(),
                "dev",
                10,
                Command::Apply {
                    before,
                    action: WorkspaceRemovalAction::Restore
                }
            )
            .is_err()
        );
        change(&db, project, WorkspaceRemovalAction::Restore);
        assert!(!available(db.conn(), &chat).unwrap());
        change(&db, folder, WorkspaceRemovalAction::Restore);
        assert!(available(db.conn(), &chat).unwrap());
        assert!(rows::chats(db.conn(), "dev").unwrap().is_empty());
    }
}
