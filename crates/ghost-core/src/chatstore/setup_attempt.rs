//! Setup execution belongs to the profile writer, including generation comparisons.
use ghost_protocol::worktree_setup::{WorktreeSetupAttempt, WorktreeSetupState};
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) struct Command {
    pub chat_id: String,
    pub expected_generation: Option<u32>,
    pub state: WorktreeSetupState,
    pub detail: Option<String>,
}

pub(crate) fn read(
    conn: &Connection,
    device: &str,
    chat: &str,
) -> rusqlite::Result<Option<WorktreeSetupAttempt>> {
    conn.query_row("SELECT worktree_id,generation,state,detail FROM worktree_setup_attempts WHERE chat_id=?1 AND device_id=?2",params![chat,device],|row| {
        let text: String = row.get(2)?;
        let state = match text.as_str() {
            "awaiting" => WorktreeSetupState::Awaiting,
            "running" => WorktreeSetupState::Running,
            "failed" => WorktreeSetupState::Failed,
            "interrupted" => WorktreeSetupState::Interrupted,
            "succeeded" => WorktreeSetupState::Succeeded,
            _ => return Err(rusqlite::Error::InvalidQuery),
        };
        Ok(WorktreeSetupAttempt {worktree_id:row.get(0)?,generation:row.get(1)?,state,detail:row.get(3)?})
    }).optional()
}

pub(crate) fn execute(
    conn: &Connection,
    device: &str,
    command: Command,
) -> rusqlite::Result<Option<WorktreeSetupAttempt>> {
    use WorktreeSetupState::*;
    let Some(current) = read(conn, device, &command.chat_id)? else {
        return Ok(None);
    };
    // A missing expected generation means an explicitly reviewed retry. It cannot supersede a
    // running process. Completion can only close its own running generation once.
    let generation = match command.expected_generation {
        None if command.state == Awaiting && current.state != Running => current
            .generation
            .checked_add(1)
            .ok_or(rusqlite::Error::InvalidQuery)?,
        Some(expected)
            if current.generation == expected
                && matches!(
                    (current.state, command.state),
                    (Awaiting, Running | Failed | Succeeded)
                        | (Running, Failed | Interrupted | Succeeded)
                ) =>
        {
            expected
        }
        _ => return Ok(None),
    };
    let state = match command.state {
        Awaiting => "awaiting",
        Running => "running",
        Failed => "failed",
        Interrupted => "interrupted",
        Succeeded => "succeeded",
    };
    conn.execute("UPDATE worktree_setup_attempts SET generation=?3,state=?4,detail=?5 WHERE chat_id=?1 AND device_id=?2",params![command.chat_id,device,generation,state,command.detail])?;
    if matches!(command.state, Failed | Interrupted) {
        super::rows::queue_hold(conn, Some(&command.chat_id), true)?;
    }
    read(conn, device, &command.chat_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generations_and_terminal_completions_are_single_use_and_device_local() {
        let db = super::super::test_support::TestDb::new();
        let chat = db.chat("/workspace");
        db.conn()
            .execute(
                "INSERT INTO worktree_setup_attempts VALUES (?1,'dev','tree',1,'awaiting',NULL)",
                [&chat],
            )
            .unwrap();
        let update = |generation, state| Command {
            chat_id: chat.clone(),
            expected_generation: generation,
            state,
            detail: None,
        };
        assert!(
            execute(
                db.conn(),
                "other",
                update(Some(1), WorktreeSetupState::Running)
            )
            .unwrap()
            .is_none()
        );
        assert!(
            execute(
                db.conn(),
                "dev",
                update(Some(1), WorktreeSetupState::Running)
            )
            .unwrap()
            .is_some()
        );
        assert!(
            execute(db.conn(), "dev", update(None, WorktreeSetupState::Awaiting))
                .unwrap()
                .is_none()
        );
        assert!(
            execute(
                db.conn(),
                "dev",
                update(Some(1), WorktreeSetupState::Failed)
            )
            .unwrap()
            .is_some()
        );
        assert!(
            execute(
                db.conn(),
                "dev",
                update(Some(1), WorktreeSetupState::Succeeded)
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            execute(db.conn(), "dev", update(None, WorktreeSetupState::Awaiting))
                .unwrap()
                .unwrap()
                .generation,
            2
        );
        assert!(
            execute(
                db.conn(),
                "dev",
                update(Some(1), WorktreeSetupState::Succeeded)
            )
            .unwrap()
            .is_none()
        );
        execute(
            db.conn(),
            "dev",
            update(Some(2), WorktreeSetupState::Running),
        )
        .unwrap()
        .unwrap();
        execute(
            db.conn(),
            "dev",
            update(Some(2), WorktreeSetupState::Succeeded),
        )
        .unwrap()
        .unwrap();
        assert!(
            execute(
                db.conn(),
                "dev",
                update(Some(2), WorktreeSetupState::Succeeded)
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn restart_interrupts_only_unfinished_local_attempts_and_preserves_legacy_chats() {
        let mut db = super::super::test_support::TestDb::new();
        let chat = db.chat("/workspace");
        let legacy = db.chat("/legacy");
        super::super::rows::queue_put(
            db.conn(),
            "dev",
            &super::super::QueuedRow {
                id: "held-before-restart".into(),
                chat_id: chat.clone(),
                text: "already held".into(),
                parts: vec![],
                held: true,
                created_at: 0,
            },
        )
        .unwrap();
        super::super::rows::queue_put(
            db.conn(),
            "dev",
            &super::super::QueuedRow {
                id: "queued-before-restart".into(),
                chat_id: chat.clone(),
                text: "keep queued".into(),
                parts: vec![],
                held: false,
                created_at: 1,
            },
        )
        .unwrap();
        db.conn()
            .execute(
                "INSERT INTO worktree_setup_attempts VALUES (?1,'dev','tree',1,'running',NULL)",
                [&chat],
            )
            .unwrap();
        db.conn().execute("INSERT INTO worktree_setup_attempts VALUES (?1,'other','other-tree',2,'awaiting',NULL)",[&chat]).unwrap();
        super::super::recover::recover(
            db.store.conn_mut(),
            super::super::recover::Scope::All,
            "dev",
            &super::super::compact::CompactLimits::STANDARD,
            2,
        )
        .unwrap();
        assert_eq!(
            read(db.conn(), "dev", &chat).unwrap().unwrap().state,
            WorktreeSetupState::Interrupted
        );
        assert_eq!(
            read(db.conn(), "other", &chat).unwrap().unwrap().state,
            WorktreeSetupState::Awaiting
        );
        assert!(read(db.conn(), "dev", &legacy).unwrap().is_none());
        assert_eq!(
            super::super::rows::queued(db.conn(), &chat)
                .unwrap()
                .iter()
                .map(|row| (row.id.as_str(), row.held))
                .collect::<Vec<_>>(),
            [
                ("held-before-restart", true),
                ("queued-before-restart", true)
            ]
        );
    }
}
