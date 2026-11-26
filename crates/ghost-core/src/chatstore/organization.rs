//! Project organization on the existing writer connection. File moves are never performed here.

use std::path::PathBuf;

use ghost_protocol::Millis;
use ghost_protocol::session::FolderState;
use ghost_protocol::workspace::{
    ChatRole, ChatRoles, FolderKind, ProjectFolder, ProjectInput, ProjectSummary, RoleModel,
    WorkspaceFilter,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::{ChatRecord, rows};
use crate::gitworkspaces::FolderIdentity;
use crate::ids::new_id;

#[derive(Debug, Clone)]
pub(crate) enum OrganizationCommand {
    Snapshot(WorkspaceFilter),
    CreateProject {
        id: String,
        input: ProjectInput,
        folder: PathBuf,
        identity: Option<FolderIdentity>,
    },
    UpdateProject {
        id: String,
        input: ProjectInput,
    },
    AddFolder {
        project_id: String,
        folder: PathBuf,
        identity: Option<FolderIdentity>,
    },
    PublishFolderIdentity {
        folder_id: String,
        expected_path: PathBuf,
        identity: FolderIdentity,
    },
    RenameChat {
        id: String,
        title: String,
    },
    PinChat {
        id: String,
        pinned: bool,
    },
    ArchiveChat {
        id: String,
        archived: bool,
    },
    /// Sets or clears the model a chat names for one kind of work. Validated by the caller.
    SetChatRole {
        id: String,
        role: ChatRole,
        choice: Option<RoleModel>,
    },
    MoveChat {
        id: String,
        project_id: String,
        folder_id: Option<String>,
        preserves_folder: bool,
    },
    CreateChat {
        id: String,
        folder_id: String,
        agent_id: String,
    },
    /// A chat whose primary folder is a new worktree of `source_folder_id`. The worktree path is
    /// registered in the same project, as a relocation into a worktree does.
    CreateWorktreeChat {
        setup_worktree_id: Option<String>,
        id: String,
        source_folder_id: String,
        path: PathBuf,
        agent_id: String,
        title: Option<String>,
    },
}

impl OrganizationCommand {
    pub(crate) fn is_read(&self) -> bool {
        matches!(self, Self::Snapshot(_))
    }

    pub(crate) fn bytes(&self) -> usize {
        let input_bytes = |input: &ProjectInput| {
            input.name.len()
                + input.color.as_ref().map_or(0, String::len)
                + input.default_agent.as_ref().map_or(0, String::len)
                + input.default_model.as_ref().map_or(0, String::len)
        };
        256 + match self {
            Self::Snapshot(filter) => filter.project_id.as_ref().map_or(0, String::len),
            Self::CreateProject {
                id,
                input,
                folder,
                identity,
            } => {
                id.len()
                    + input_bytes(input)
                    + folder.as_os_str().len()
                    + identity.as_ref().map_or(0, FolderIdentity::bytes)
            }
            Self::UpdateProject { id, input } => id.len() + input_bytes(input),
            Self::AddFolder {
                project_id,
                folder,
                identity,
            } => {
                project_id.len()
                    + folder.as_os_str().len()
                    + identity.as_ref().map_or(0, FolderIdentity::bytes)
            }
            Self::RenameChat { id, title } => id.len() + title.len(),
            Self::PublishFolderIdentity {
                folder_id,
                expected_path,
                identity,
            } => folder_id.len() + expected_path.as_os_str().len() + identity.bytes(),
            Self::PinChat { id, .. } | Self::ArchiveChat { id, .. } => id.len(),
            Self::SetChatRole { id, choice, .. } => {
                id.len()
                    + choice.as_ref().map_or(0, |choice| {
                        choice.agent.len()
                            + choice.model.as_ref().map_or(0, String::len)
                            + choice.effort.as_ref().map_or(0, String::len)
                    })
            }
            Self::MoveChat {
                id,
                project_id,
                folder_id,
                ..
            } => id.len() + project_id.len() + folder_id.as_ref().map_or(0, String::len),
            Self::CreateChat {
                id,
                folder_id,
                agent_id,
            } => id.len() + folder_id.len() + agent_id.len(),
            Self::CreateWorktreeChat {
                id,
                source_folder_id,
                path,
                agent_id,
                title,
                ..
            } => {
                id.len()
                    + source_folder_id.len()
                    + path.as_os_str().len()
                    + agent_id.len()
                    + title.as_ref().map_or(0, String::len)
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct OrganizedChat {
    pub(crate) chat: ChatRecord,
    pub(crate) project_id: String,
    pub(crate) folder_id: Option<String>,
    pub(crate) pinned: bool,
    pub(crate) archived: bool,
    pub(crate) roles: ChatRoles,
}

#[derive(Debug)]
pub(crate) struct Organization {
    pub(crate) projects: Vec<ProjectSummary>,
    pub(crate) chats: Vec<OrganizedChat>,
    pub(crate) affected_id: Option<String>,
}

pub(crate) fn execute(
    conn: &Connection,
    device: &str,
    now: Millis,
    command: OrganizationCommand,
) -> rusqlite::Result<Organization> {
    let affected_id = match command {
        OrganizationCommand::Snapshot(filter) => return snapshot(conn, device, &filter),
        OrganizationCommand::CreateProject {
            id,
            input,
            folder,
            identity,
        } => {
            conn.execute("INSERT INTO projects (id,name,color,default_agent,default_model,created_at,updated_at,device_id) VALUES (?1,?2,?3,?4,?5,?6,?6,?7)", params![id,input.name,input.color,input.default_agent,input.default_model,now,device])?;
            let folder_id = add_folder(conn, device, now, &id, &folder)?;
            update_identity(conn, device, now, &folder_id, identity.as_ref())?;
            id
        }
        OrganizationCommand::UpdateProject { id, input } => {
            changed(conn.execute("UPDATE projects SET name=?2,color=?3,default_agent=?4,default_model=?5,updated_at=?6,device_id=?7 WHERE id=?1 AND deleted=0", params![id,input.name,input.color,input.default_agent,input.default_model,now,device])?)?;
            id
        }
        OrganizationCommand::AddFolder {
            project_id,
            folder,
            identity,
        } => {
            project_exists(conn, &project_id)?;
            let id = add_folder(conn, device, now, &project_id, &folder)?;
            update_identity(conn, device, now, &id, identity.as_ref())?;
            id
        }
        OrganizationCommand::RenameChat { id, title } => {
            changed(conn.execute("UPDATE chats SET title=?2,title_source='manual',updated_at=?3,device_id=?4 WHERE id=?1 AND deleted=0", params![id,title,now,device])?)?;
            id
        }
        OrganizationCommand::PublishFolderIdentity {
            folder_id,
            expected_path,
            identity,
        } => {
            let _: i64=conn.query_row("SELECT 1 FROM project_folders pf JOIN projects p ON p.id=pf.project_id JOIN folder_locations fl ON fl.folder_id=pf.id AND fl.device_id=?2 WHERE pf.id=?1 AND pf.deleted=0 AND p.deleted=0 AND pf.kind='local' AND fl.path=?3",params![folder_id,device,super::stored_path(&expected_path)],|row|row.get(0))?;
            update_identity(conn, device, now, &folder_id, Some(&identity))?;
            folder_id
        }
        OrganizationCommand::PinChat { id, pinned } => {
            changed(conn.execute(
                "UPDATE chats SET pinned=?2,updated_at=?3,device_id=?4 WHERE id=?1 AND deleted=0",
                params![id, pinned, now, device],
            )?)?;
            id
        }
        OrganizationCommand::ArchiveChat { id, archived } => {
            changed(conn.execute(
                "UPDATE chats SET archived=?2,updated_at=?3,device_id=?4 WHERE id=?1 AND deleted=0",
                params![id, archived, now, device],
            )?)?;
            id
        }
        OrganizationCommand::SetChatRole { id, role, choice } => {
            // The chat must exist; a role choice does not reorder the list, so updated_at stays.
            let _: i64 = conn.query_row(
                "SELECT 1 FROM chats WHERE id=?1 AND deleted=0",
                params![id],
                |row| row.get(0),
            )?;
            match choice {
                Some(choice) => conn.execute(
                    "INSERT INTO chat_roles (chat_id,role,agent,model,effort,updated_at) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(chat_id,role) DO UPDATE SET agent=excluded.agent,model=excluded.model,effort=excluded.effort,updated_at=excluded.updated_at",
                    params![id, role.as_str(), choice.agent, choice.model, choice.effort, now],
                )?,
                None => conn.execute(
                    "DELETE FROM chat_roles WHERE chat_id=?1 AND role=?2",
                    params![id, role.as_str()],
                )?,
            };
            id
        }
        OrganizationCommand::MoveChat {
            id,
            project_id,
            folder_id,
            preserves_folder,
        } => {
            project_exists(conn, &project_id)?;
            let current =
                rows::chat(conn, device, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            let folder_id = match folder_id {
                Some(folder_id) => {
                    let actual: String = conn.query_row(
                        "SELECT project_id FROM project_folders WHERE id=?1 AND deleted=0",
                        [&folder_id],
                        |row| row.get(0),
                    )?;
                    if actual != project_id {
                        return Err(rusqlite::Error::InvalidParameterName(
                            "The folder does not belong to the destination project.".into(),
                        ));
                    }
                    let _: String = conn.query_row(
                        "SELECT path FROM folder_locations WHERE folder_id=?1 AND device_id=?2",
                        params![folder_id, device],
                        |row| row.get(0),
                    )?;
                    let has_session: bool = conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM agent_sessions WHERE chat_id=?1)",
                        [&id],
                        |row| row.get(0),
                    )?;
                    if has_session && !preserves_folder {
                        return Err(rusqlite::Error::InvalidParameterName(
                            "This chat has agent history in its current folder. Create a new chat in the destination folder until visible handoff is available.".into(),
                        ));
                    }
                    folder_id
                }
                None => {
                    if current.folder.as_os_str().is_empty() {
                        return Err(rusqlite::Error::InvalidParameterName(
                            "Locate the chat folder on this device before moving the chat.".into(),
                        ));
                    }
                    let destination = add_folder(conn, device, now, &project_id, &current.folder)?;
                    if let Some(source) = current.folder_id.as_ref() {
                        // Organization moves preserve the same local folder. Carry its qualified
                        // portable identity when the destination was newly registered as local.
                        conn.execute("UPDATE project_folders SET kind='git',git_remote_url=(SELECT git_remote_url FROM project_folders WHERE id=?2),git_default_branch=(SELECT git_default_branch FROM project_folders WHERE id=?2),origin_device_id=NULL,updated_at=?3,device_id=?4 WHERE id=?1 AND kind='local' AND EXISTS(SELECT 1 FROM project_folders WHERE id=?2 AND kind='git' AND deleted=0)", params![destination,source,now,device])?;
                    }
                    destination
                }
            };
            changed(conn.execute("UPDATE chats SET project_id=?2,folder_id=?3,updated_at=?4,device_id=?5 WHERE id=?1 AND deleted=0", params![id,project_id,folder_id,now,device])?)?;
            id
        }
        OrganizationCommand::CreateChat {
            id,
            folder_id,
            agent_id,
        } => {
            let project_id: String = conn.query_row("SELECT pf.project_id FROM project_folders pf JOIN projects p ON p.id=pf.project_id JOIN folder_locations fl ON fl.folder_id=pf.id AND fl.device_id=?2 WHERE pf.id=?1 AND pf.deleted=0 AND p.deleted=0", params![folder_id,device], |row| row.get(0))?;
            conn.execute("INSERT INTO chats (id,project_id,folder_id,active_agent,created_at,updated_at,device_id,agent_host) VALUES (?1,?2,?3,?4,?5,?5,?6,'{\"type\":\"native\"}')", params![id,project_id,folder_id,agent_id,now,device])?;
            id
        }
        OrganizationCommand::CreateWorktreeChat {
            setup_worktree_id,
            id,
            source_folder_id,
            path,
            agent_id,
            title,
        } => {
            let project_id: String = conn.query_row("SELECT pf.project_id FROM project_folders pf JOIN projects p ON p.id=pf.project_id JOIN folder_locations fl ON fl.folder_id=pf.id AND fl.device_id=?2 WHERE pf.id=?1 AND pf.deleted=0 AND p.deleted=0", params![source_folder_id,device], |row| row.get(0))?;
            let folder_id = add_folder(conn, device, now, &project_id, &path)?;
            conn.execute("INSERT INTO chats (id,project_id,folder_id,title,title_source,active_agent,created_at,updated_at,device_id,agent_host) VALUES (?1,?2,?3,?4,CASE WHEN ?4 IS NULL THEN NULL ELSE 'manual' END,?5,?6,?6,?7,'{\"type\":\"native\"}')", params![id,project_id,folder_id,title,agent_id,now,device])?;
            if let Some(worktree_id) = setup_worktree_id {
                conn.execute(
                    "INSERT INTO worktree_setup_attempts VALUES (?1,?2,?3,1,'awaiting',NULL)",
                    params![id, device, worktree_id],
                )?;
            }
            id
        }
    };
    let mut result = snapshot(
        conn,
        device,
        &WorkspaceFilter {
            include_archived: true,
            project_id: None,
        },
    )?;
    result.affected_id = Some(affected_id);
    Ok(result)
}

fn changed(count: usize) -> rusqlite::Result<()> {
    if count == 1 {
        Ok(())
    } else {
        Err(rusqlite::Error::QueryReturnedNoRows)
    }
}

fn project_exists(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.query_row(
        "SELECT 1 FROM projects WHERE id=?1 AND deleted=0",
        [id],
        |_| Ok(()),
    )
}

fn update_identity(
    conn: &Connection,
    device: &str,
    now: Millis,
    id: &str,
    identity: Option<&FolderIdentity>,
) -> rusqlite::Result<()> {
    let Some(identity) = identity else {
        return Ok(());
    };
    let (kind, remote, branch) = match identity {
        FolderIdentity::Local => ("local", None, None),
        FolderIdentity::Git {
            remote,
            default_branch,
        } => ("git", Some(remote.as_str()), default_branch.as_deref()),
    };
    conn.execute("UPDATE project_folders SET kind=?2,git_remote_url=?3,git_default_branch=?4,origin_device_id=CASE WHEN ?2='local' THEN ?5 ELSE NULL END,updated_at=?6,device_id=?5 WHERE id=?1 AND deleted=0", params![id,kind,remote,branch,device,now])?;
    Ok(())
}

pub(super) fn add_folder(
    conn: &Connection,
    device: &str,
    now: Millis,
    project: &str,
    path: &std::path::Path,
) -> rusqlite::Result<String> {
    let shown = super::stored_path(path);
    let existing = conn.query_row("SELECT pf.id FROM project_folders pf JOIN folder_locations fl ON fl.folder_id=pf.id AND fl.device_id=?2 WHERE pf.project_id=?1 AND pf.deleted=0 AND fl.path=?3 LIMIT 1", params![project,device,shown], |row| row.get::<_,String>(0)).optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = new_id();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| shown.as_str().into());
    conn.execute("INSERT INTO project_folders (id,project_id,display_name,kind,origin_device_id,created_at,updated_at,device_id) VALUES (?1,?2,?3,'local',?4,?5,?5,?4)", params![id,project,name,device,now])?;
    conn.execute(
        "INSERT INTO folder_locations (folder_id,device_id,path,last_seen_at) VALUES (?1,?2,?3,?4)",
        params![id, device, shown, now],
    )?;
    conn.execute(
        "UPDATE projects SET updated_at=?2,device_id=?3 WHERE id=?1",
        params![project, now, device],
    )?;
    Ok(id)
}

fn snapshot(
    conn: &Connection,
    device: &str,
    filter: &WorkspaceFilter,
) -> rusqlite::Result<Organization> {
    let mut statement = conn.prepare("SELECT id,name,color,default_agent,default_model,created_at,updated_at FROM projects WHERE deleted=0 ORDER BY sort_key,name,id")?;
    let mut projects = statement
        .query_map([], |row| {
            Ok(ProjectSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                // A default saved as a connection (`codex-acp`) names its agent.
                default_agent: row
                    .get::<_, Option<String>>(3)?
                    .map(|agent| ghost_agents::agent_of(&agent).to_owned()),
                default_model: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                folders: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut statement = conn.prepare("SELECT pf.id,pf.project_id,pf.display_name,fl.path,pf.kind,pf.git_remote_url,pf.git_default_branch FROM project_folders pf LEFT JOIN folder_locations fl ON fl.folder_id=pf.id AND fl.device_id=?1 WHERE pf.deleted=0 ORDER BY pf.sort_key,pf.created_at,pf.id")?;
    let folders = statement
        .query_map([device], |row| {
            let path: Option<String> = row.get(3)?;
            let absent = path.is_none();
            Ok(ProjectFolder {
                id: row.get(0)?,
                project_id: row.get(1)?,
                display_name: row.get(2)?,
                path,
                folder_state: if absent {
                    FolderState::Missing
                } else {
                    FolderState::Ok
                },
                unavailable_reason: absent.then(|| "Locate this folder on this device.".into()),
                kind: if row.get::<_, String>(4)? == "git" {
                    FolderKind::Git
                } else {
                    FolderKind::Local
                },
                git_remote_url: row.get(5)?,
                git_default_branch: row.get(6)?,
                // Set from the worktree registry when the workspace is shown.
                worktree_of: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for folder in folders {
        if let Some(project) = projects
            .iter_mut()
            .find(|project| project.id == folder.project_id)
        {
            project.folders.push(folder);
        }
    }
    let mut statement = conn.prepare("SELECT c.id,c.project_id,c.folder_id,c.pinned,c.archived FROM chats c JOIN projects p ON p.id=c.project_id WHERE c.deleted=0 AND p.deleted=0 AND (c.folder_id IS NULL OR EXISTS(SELECT 1 FROM project_folders f WHERE f.id=c.folder_id AND f.deleted=0)) AND (?1 OR c.archived=0) AND (?2 IS NULL OR c.project_id=?2) ORDER BY c.pinned DESC,c.updated_at DESC,c.id DESC")?;
    let metadata = statement
        .query_map(params![filter.include_archived, filter.project_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut chats = Vec::with_capacity(metadata.len());
    let mut roles =
        conn.prepare("SELECT role,agent,model,effort FROM chat_roles WHERE chat_id=?1")?;
    for (id, project_id, folder_id, pinned, archived) in metadata {
        let chat = rows::chat(conn, device, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        chats.push(OrganizedChat {
            chat,
            project_id,
            folder_id,
            pinned,
            archived,
            roles: chat_roles(&mut roles, &id)?,
        });
    }
    Ok(Organization {
        projects,
        chats,
        affected_id: None,
    })
}

/// A chat's role choices. A row naming a role this build does not know is left out.
fn chat_roles(
    statement: &mut rusqlite::Statement<'_>,
    chat_id: &str,
) -> rusqlite::Result<ChatRoles> {
    let mut roles = ChatRoles::default();
    for row in statement.query_map(params![chat_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            RoleModel {
                agent: row.get(1)?,
                model: row.get(2)?,
                effort: row.get(3)?,
            },
        ))
    })? {
        let (role, choice) = row?;
        match role.as_str() {
            "plan" => roles.set(ChatRole::Plan, Some(choice)),
            "build" => roles.set(ChatRole::Build, Some(choice)),
            _ => {}
        }
    }
    Ok(roles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatstore::test_support::TestDb;
    #[test]
    fn publication_identity_changes_only_the_reviewed_existing_device_mapping() {
        let db = TestDb::new();
        let chat = db.chat("/owned/repo");
        let id: String = db
            .conn()
            .query_row("SELECT folder_id FROM chats WHERE id=?1", [chat], |row| {
                row.get(0)
            })
            .unwrap();
        let command = |path: &str| OrganizationCommand::PublishFolderIdentity {
            folder_id: id.clone(),
            expected_path: path.into(),
            identity: FolderIdentity::Git {
                remote: "https://github.com/fixture/project".into(),
                default_branch: None,
            },
        };
        assert!(execute(db.conn(), "dev", 2, command("/wrong")).is_err());
        assert!(execute(db.conn(), "other", 2, command("/owned/repo")).is_err());
        let result = execute(db.conn(), "dev", 2, command("/owned/repo")).unwrap();
        assert_eq!(result.projects[0].folders[0].id, id);
        assert_eq!(result.projects[0].folders[0].kind, FolderKind::Git);
        assert_eq!(
            result.projects[0].folders[0].path.as_deref(),
            Some("/owned/repo")
        );
        assert!(execute(db.conn(), "dev", 3, command("/owned/repo")).is_err());
        let count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM project_folders", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn a_chat_keeps_its_own_roles_and_clearing_one_leaves_the_other() {
        let db = TestDb::new();
        let chat = db.chat("/owned/repo");
        let other = db.chat("/owned/other");
        let updated = |id: &str| -> i64 {
            db.conn()
                .query_row("SELECT updated_at FROM chats WHERE id=?1", [id], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        let before = updated(&chat);
        let set = |id: &str, role, choice| {
            execute(
                db.conn(),
                "dev",
                50,
                OrganizationCommand::SetChatRole {
                    id: id.into(),
                    role,
                    choice,
                },
            )
        };
        let fable = RoleModel {
            agent: "claude".into(),
            model: Some("claude-fable-5-2".into()),
            effort: Some("high".into()),
        };
        let codex = RoleModel {
            agent: "codex".into(),
            model: None,
            effort: None,
        };
        set(&chat, ChatRole::Plan, Some(fable.clone())).unwrap();
        set(&chat, ChatRole::Build, Some(codex.clone())).unwrap();
        // Choosing again replaces the choice instead of adding a second one.
        let view = set(&chat, ChatRole::Build, Some(codex.clone())).unwrap();
        let roles_of = |view: &Organization, id: &str| {
            view.chats
                .iter()
                .find(|entry| entry.chat.id == id)
                .unwrap()
                .roles
                .clone()
        };
        assert_eq!(
            roles_of(&view, &chat),
            ChatRoles {
                plan: Some(fable.clone()),
                build: Some(codex),
            }
        );
        // Another chat has none, and a role choice does not move the chat in the list.
        assert_eq!(roles_of(&view, &other), ChatRoles::default());
        assert_eq!(updated(&chat), before);
        let view = set(&chat, ChatRole::Build, None).unwrap();
        assert_eq!(
            roles_of(&view, &chat),
            ChatRoles {
                plan: Some(fable.clone()),
                build: None,
            }
        );
        // A chat that does not exist, or was deleted, takes none.
        assert!(set("missing", ChatRole::Plan, Some(fable.clone())).is_err());
        db.conn()
            .execute("UPDATE chats SET deleted=1 WHERE id=?1", [&other])
            .unwrap();
        assert!(set(&other, ChatRole::Plan, Some(fable)).is_err());
        let count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM chat_roles", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn portable_identity_updates_preserve_folder_id_and_device_local_paths() {
        let db = TestDb::new();
        let input = ProjectInput {
            name: "Project".into(),
            color: None,
            default_agent: None,
            default_model: None,
        };
        let view = execute(
            db.conn(),
            "dev",
            1,
            OrganizationCommand::CreateProject {
                id: "project".into(),
                input,
                folder: PathBuf::from("/owned/repo"),
                identity: None,
            },
        )
        .expect("project");
        let original_id = view.projects[0].folders[0].id.clone();
        let identity = FolderIdentity::Git {
            remote: "https://github.com/fixture/project".into(),
            default_branch: Some("trunk".into()),
        };
        let refresh = |identity| {
            execute(
                db.conn(),
                "dev",
                2,
                OrganizationCommand::AddFolder {
                    project_id: "project".into(),
                    folder: PathBuf::from("/owned/repo"),
                    identity,
                },
            )
            .expect("refresh")
        };
        let view = refresh(Some(identity));
        let folder = &view.projects[0].folders[0];
        assert_eq!(folder.id, original_id);
        assert_eq!(folder.kind, FolderKind::Git);
        assert_eq!(folder.path.as_deref(), Some("/owned/repo"));
        assert_eq!(folder.git_default_branch.as_deref(), Some("trunk"));
        assert_eq!(refresh(None).projects[0].folders[0].kind, FolderKind::Git);
        let foreign = execute(
            db.conn(),
            "other-device",
            3,
            OrganizationCommand::Snapshot(WorkspaceFilter::default()),
        )
        .expect("foreign view");
        assert!(foreign.projects[0].folders[0].path.is_none());
        assert_eq!(
            foreign.projects[0].folders[0].git_remote_url.as_deref(),
            Some("https://github.com/fixture/project")
        );
        let local = refresh(Some(FolderIdentity::Local));
        assert_eq!(local.projects[0].folders[0].id, original_id);
        assert_eq!(local.projects[0].folders[0].kind, FolderKind::Local);
        assert!(local.projects[0].folders[0].git_remote_url.is_none());
        assert!(local.projects[0].folders[0].git_default_branch.is_none());
    }

    #[test]
    fn portable_identity_follows_a_project_move_that_keeps_the_same_folder() {
        let db = TestDb::new();
        let chat = db.chat("/owned/repo");
        let source = rows::chat(db.conn(), "dev", &chat)
            .expect("read")
            .expect("chat");
        update_identity(
            db.conn(),
            "dev",
            2,
            source.folder_id.as_ref().expect("folder"),
            Some(&FolderIdentity::Git {
                remote: "https://github.com/fixture/project".into(),
                default_branch: Some("main".into()),
            }),
        )
        .expect("identity");
        execute(
            db.conn(),
            "dev",
            3,
            OrganizationCommand::CreateProject {
                id: "destination-project".into(),
                input: ProjectInput {
                    name: "Destination".into(),
                    color: None,
                    default_agent: None,
                    default_model: None,
                },
                folder: PathBuf::from("/other"),
                identity: None,
            },
        )
        .expect("destination");
        let view = execute(
            db.conn(),
            "dev",
            4,
            OrganizationCommand::MoveChat {
                id: chat,
                project_id: "destination-project".into(),
                folder_id: None,
                preserves_folder: true,
            },
        )
        .expect("move");
        let destination = view
            .projects
            .iter()
            .find(|project| project.id == "destination-project")
            .expect("project");
        let folder = destination
            .folders
            .iter()
            .find(|folder| folder.path.as_deref() == Some("/owned/repo"))
            .expect("folder");
        assert_eq!(folder.kind, FolderKind::Git);
        assert_eq!(
            folder.git_remote_url.as_deref(),
            Some("https://github.com/fixture/project")
        );
        assert_eq!(folder.git_default_branch.as_deref(), Some("main"));
        assert_ne!(Some(&folder.id), source.folder_id.as_ref());
    }

    #[test]
    fn foreign_device_paths_are_never_used_for_local_chats_or_folders() {
        let db = TestDb::new();
        let id = db.chat("/another-device/private-work");
        let view = execute(
            db.conn(),
            "different-device",
            2,
            OrganizationCommand::Snapshot(WorkspaceFilter::default()),
        )
        .expect("snapshot");
        assert_eq!(view.chats[0].chat.id, id);
        assert!(view.chats[0].chat.folder.as_os_str().is_empty());
        let folder = &view.projects[0].folders[0];
        assert!(folder.path.is_none());
        assert_eq!(folder.folder_state, FolderState::Missing);
        assert!(folder.unavailable_reason.is_some());
        assert!(
            rows::chat(db.conn(), "different-device", &id)
                .expect("chat")
                .expect("record")
                .folder
                .as_os_str()
                .is_empty()
        );
    }

    #[test]
    fn stored_manual_title_rejects_late_queued_automatic_write() {
        let db = TestDb::new();
        let id = db.chat("/work");
        execute(
            db.conn(),
            "dev",
            2,
            OrganizationCommand::RenameChat {
                id: id.clone(),
                title: "Chosen title".into(),
            },
        )
        .expect("rename");
        rows::set_title(
            db.conn(),
            &id,
            "Late title",
            super::super::TitleSource::Agent,
        )
        .expect("late write");
        let saved = rows::chat(db.conn(), "dev", &id)
            .expect("chat")
            .expect("saved");
        assert_eq!(saved.title.as_deref(), Some("Chosen title"));
        assert_eq!(saved.title_source, Some(super::super::TitleSource::Manual));
    }

    #[test]
    fn project_filter_keeps_destinations_and_archived_listing_is_explicit() {
        let db = TestDb::new();
        let first = db.chat("/one");
        let second = db.chat("/two");
        execute(
            db.conn(),
            "dev",
            2,
            OrganizationCommand::ArchiveChat {
                id: first,
                archived: true,
            },
        )
        .expect("archive");
        let active = execute(
            db.conn(),
            "dev",
            3,
            OrganizationCommand::Snapshot(WorkspaceFilter::default()),
        )
        .expect("active");
        assert_eq!(active.projects.len(), 2);
        assert_eq!(active.chats.len(), 1);
        assert_eq!(active.chats[0].chat.id, second);
        let project_id = active.chats[0].project_id.clone();
        let filtered = execute(
            db.conn(),
            "dev",
            3,
            OrganizationCommand::Snapshot(WorkspaceFilter {
                include_archived: true,
                project_id: Some(project_id),
            }),
        )
        .expect("filtered");
        assert_eq!(filtered.projects.len(), 2);
        assert_eq!(filtered.chats.len(), 1);
    }
}
