//! Device-local recipes. Source folders must belong to the chat's project on this device.
use ghost_protocol::worktree_setup::{WorktreeSetup, WorktreeSetupInput};
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) struct Command {
    pub chat_id: String,
    pub source: String,
    pub save: Option<WorktreeSetupInput>,
    /// Reads the recipe of this project folder directly, for a chat that does not exist yet.
    /// `source` must still be this device's path of the folder. Saving this way needs
    /// `folder_save`, an explicit reviewed action (the Instructions surface adding one file).
    pub folder_id: Option<String>,
    pub folder_save: bool,
}

impl Command {
    pub fn bytes(&self) -> usize {
        self.save.as_ref().map_or(1024, |input| {
            serde_json::to_vec(input).map_or(65536, |bytes| bytes.len()) + 1024
        })
    }
}

pub(crate) fn execute(
    conn: &Connection,
    device: &str,
    now: i64,
    command: Command,
) -> rusqlite::Result<WorktreeSetup> {
    let invalid = || {
        rusqlite::Error::InvalidParameterName(
            "The setup source no longer belongs to this chat's project on this device.".into(),
        )
    };
    let source = super::stored_path_text(&command.source);
    // Membership first: the folder, or a folder of the chat's project, is `source` on this device.
    let project: String = match &command.folder_id {
        Some(_) if command.save.is_some() && !command.folder_save => return Err(invalid()),
        Some(folder_id) => conn.query_row(
            "SELECT pf.project_id FROM project_folders pf JOIN projects p ON p.id=pf.project_id JOIN folder_locations fl ON fl.folder_id=pf.id WHERE pf.id=?1 AND pf.deleted=0 AND p.deleted=0 AND fl.device_id=?2 AND fl.path=?3",
            params![folder_id, device, source], |row| row.get(0)
        ).optional()?,
        None => conn.query_row(
            "SELECT pf.project_id FROM project_folders pf JOIN folder_locations fl ON fl.folder_id=pf.id JOIN chats c ON c.project_id=pf.project_id WHERE c.id=?1 AND c.deleted=0 AND pf.deleted=0 AND fl.device_id=?2 AND fl.path=?3 LIMIT 1",
            params![command.chat_id, device, source], |row| row.get(0)
        ).optional()?,
    }.ok_or_else(invalid)?;
    // Then the recipe's row: of the project's folders at that path (one added twice, or moved in
    // from another project), the one that has a recipe on this device, else the oldest. Reading
    // and saving land on the same row, whichever folder ID or chat asked.
    let folder: String = conn.query_row(
        "SELECT pf.id FROM project_folders pf JOIN folder_locations fl ON fl.folder_id=pf.id LEFT JOIN worktree_setup ws ON ws.folder_id=pf.id AND ws.device_id=?2 WHERE pf.project_id=?1 AND pf.deleted=0 AND fl.device_id=?2 AND fl.path=?3 ORDER BY ws.folder_id IS NULL, pf.created_at, pf.id LIMIT 1",
        params![project, device, source], |row| row.get(0)
    ).optional()?.ok_or_else(invalid)?;
    let input = if let Some(input) = command.save {
        crate::gitworkspaces::setup::validate(&input).map_err(|_| {
            rusqlite::Error::InvalidParameterName("The setup recipe is invalid.".into())
        })?;
        let json = serde_json::to_string(&input).map_err(|_| invalid())?;
        conn.execute("INSERT INTO worktree_setup(folder_id,device_id,input_json,updated_at) VALUES (?1,?2,?3,?4) ON CONFLICT(folder_id,device_id) DO UPDATE SET input_json=excluded.input_json,updated_at=excluded.updated_at", params![folder,device,json,now])?;
        input
    } else {
        let json: Option<String> = conn
            .query_row(
                "SELECT input_json FROM worktree_setup WHERE folder_id=?1 AND device_id=?2",
                params![folder, device],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(json) => serde_json::from_str(&json).map_err(|_| invalid())?,
            None => WorktreeSetupInput::default(),
        }
    };
    crate::gitworkspaces::setup::validate(&input).map_err(|_| invalid())?;
    Ok(WorktreeSetup {
        source: command.source,
        input,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_settings_are_shared_by_folder_but_isolated_by_device_and_project() {
        let db = super::super::test_support::TestDb::new();
        let chat = db.chat("/workspace");
        let other = db.chat("/other");
        let input = WorktreeSetupInput {
            run: None,
            files: vec![".env".into()],
            command: Some(ghost_protocol::worktree_setup::SetupCommand {
                program: "node".into(),
                args: vec!["install.js".into()],
            }),
        };
        let request = |chat_id: &str, source: &str, save| Command {
            chat_id: chat_id.into(),
            source: source.into(),
            save,
            folder_id: None,
            folder_save: false,
        };
        execute(
            db.conn(),
            "dev",
            1,
            request(&chat, "/workspace", Some(input.clone())),
        )
        .unwrap();
        let second = db.chat("/workspace");
        assert_eq!(
            execute(db.conn(), "dev", 2, request(&second, "/workspace", None))
                .unwrap()
                .input,
            input
        );
        assert!(execute(db.conn(), "dev", 2, request(&other, "/workspace", None)).is_err());
        let folder: String = db
            .conn()
            .query_row("SELECT folder_id FROM chats WHERE id=?1", [&chat], |row| {
                row.get(0)
            })
            .unwrap();
        // A chat that does not exist yet reads its source folder's recipe by folder ID only.
        let by_folder = |source: &str, save| Command {
            chat_id: String::new(),
            source: source.into(),
            save,
            folder_id: Some(folder.clone()),
            folder_save: false,
        };
        assert_eq!(
            execute(db.conn(), "dev", 2, by_folder("/workspace", None))
                .unwrap()
                .input,
            input
        );
        assert!(execute(db.conn(), "dev", 2, by_folder("/other", None)).is_err());
        assert!(
            execute(
                db.conn(),
                "dev",
                2,
                by_folder("/workspace", Some(WorktreeSetupInput::default()))
            )
            .is_err()
        );
        // The Instructions surface saves by folder only with the explicit flag, still bound to
        // this device's path of the folder.
        let mut explicit = by_folder(
            "/workspace",
            Some(WorktreeSetupInput {
                run: None,
                files: vec![".env".into(), "AGENTS.md".into()],
                command: input.command.clone(),
            }),
        );
        explicit.folder_save = true;
        assert_eq!(
            execute(db.conn(), "dev", 3, explicit).unwrap().input.files,
            vec![".env", "AGENTS.md"]
        );
        let mut elsewhere = by_folder("/other", Some(WorktreeSetupInput::default()));
        elsewhere.folder_save = true;
        assert!(execute(db.conn(), "dev", 3, elsewhere).is_err());
        let mut back = by_folder("/workspace", Some(input.clone()));
        back.folder_save = true;
        execute(db.conn(), "dev", 4, back).unwrap();
        db.conn().execute("INSERT INTO folder_locations(folder_id,device_id,path) VALUES (?1,'second','/workspace')",[folder]).unwrap();
        assert_eq!(
            execute(db.conn(), "second", 2, request(&chat, "/workspace", None))
                .unwrap()
                .input,
            WorktreeSetupInput::default()
        );
        assert_eq!(
            db.conn()
                .query_row("SELECT count(*) FROM worktree_setup", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn a_folder_in_its_project_twice_keeps_one_recipe_on_the_row_that_has_it() {
        let db = super::super::test_support::TestDb::new();
        let chat = db.chat("/workspace");
        let (first, project): (String, String) = db
            .conn()
            .query_row(
                "SELECT folder_id, project_id FROM chats WHERE id=?1",
                [&chat],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        // The same path again in the same project, added later.
        db.conn()
            .execute(
                "INSERT INTO project_folders (id,project_id,display_name,kind,origin_device_id,created_at,updated_at,device_id) VALUES ('later',?1,'workspace','local','dev',5,5,'dev')",
                [&project],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO folder_locations (folder_id,device_id,path,last_seen_at) VALUES ('later','dev','/workspace',5)",
                [],
            )
            .unwrap();
        let recipe = |files: &[&str]| WorktreeSetupInput {
            run: None,
            files: files.iter().map(|file| (*file).into()).collect(),
            command: None,
        };
        let read = |folder_id: Option<&str>| {
            execute(
                db.conn(),
                "dev",
                6,
                Command {
                    chat_id: chat.clone(),
                    source: "/workspace".into(),
                    save: None,
                    folder_id: folder_id.map(Into::into),
                    folder_save: false,
                },
            )
            .unwrap()
            .input
        };
        // A recipe saved through the later row is the one every reader gets: by chat, and by
        // either folder's ID.
        execute(
            db.conn(),
            "dev",
            6,
            Command {
                chat_id: String::new(),
                source: "/workspace".into(),
                save: Some(recipe(&[".env"])),
                folder_id: Some("later".into()),
                folder_save: true,
            },
        )
        .unwrap();
        let saved_on = |id: &str| -> i64 {
            db.conn()
                .query_row(
                    "SELECT count(*) FROM worktree_setup WHERE folder_id=?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        // Neither row had one, so it went to the oldest.
        assert_eq!((saved_on(&first), saved_on("later")), (1, 0));
        assert_eq!(read(None), recipe(&[".env"]));
        assert_eq!(read(Some("later")), recipe(&[".env"]));
        assert_eq!(read(Some(&first)), recipe(&[".env"]));
        // A recipe only the later row holds (saved before this rule) is found and kept there.
        db.conn().execute("DELETE FROM worktree_setup", []).unwrap();
        db.conn()
            .execute(
                "INSERT INTO worktree_setup(folder_id,device_id,input_json,updated_at) VALUES ('later','dev',?1,7)",
                [serde_json::to_string(&recipe(&["AGENTS.md"])).unwrap()],
            )
            .unwrap();
        assert_eq!(read(None), recipe(&["AGENTS.md"]));
        assert_eq!(read(Some(&first)), recipe(&["AGENTS.md"]));
        execute(
            db.conn(),
            "dev",
            8,
            Command {
                chat_id: chat.clone(),
                source: "/workspace".into(),
                save: Some(recipe(&["AGENTS.md", ".env"])),
                folder_id: None,
                folder_save: false,
            },
        )
        .unwrap();
        assert_eq!((saved_on(&first), saved_on("later")), (0, 1));
    }
}
