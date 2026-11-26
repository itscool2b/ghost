//! Native resume aliases become one claimed, persisted transcript before the next prompt.
mod support;

use ghost_drivers::driver::{DriverEvent, LoggedCommand};
use ghost_drivers::testing::{ScriptedDriver, Step};
use ghost_protocol::{ErrorKind, GhostEvent, SessionStatus, StopReason};
use std::time::Duration;
use support::*;

const NATIVE: &str = "019a0000-0000-7000-8000-000000000011";
const SOURCE: &str = "/transcripts/resumed.jsonl";

fn prompt_ids(commands: &[LoggedCommand]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|command| match command {
            LoggedCommand::Prompt { turn_id, .. } => Some(turn_id.clone()),
            _ => None,
        })
        .collect()
}

// A real clock lets the SQLite writer's busy wait coexist with the held write transaction.
#[tokio::test]
async fn native_identity_is_committed_in_the_same_row_before_dispatch_resumes() {
    assert_identity_commit_gate(Some(SOURCE)).await;
}

#[tokio::test]
async fn initially_absent_identity_is_committed_before_dispatch_resumes() {
    assert_identity_commit_gate(None).await;
}

fn ready_with_identity(native: Option<&str>) -> Step {
    let mut info = ready_info("unused");
    info.native_session_id = native.map(str::to_owned);
    Step::Emit(DriverEvent::Ready(info))
}

#[tokio::test]
async fn absent_id_is_not_claimed_or_used_to_resume_an_uncreated_transcript() {
    let (first, handle) = ScriptedDriver::new(vec![ready_with_identity(None)]);
    let (second, _) = ScriptedDriver::new(vec![ready_with_identity(None)]);
    let h = Harness::new(vec![first, second]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let another = h.chat().await;
    h.status(&another.id, SessionStatus::Ready).await;
    h.manager
        .send(&chat.id, "fails before creation".into())
        .await
        .unwrap();
    eventually("first visible prompt", || {
        prompt_ids(&handle.commands()).len() == 1
    })
    .await;
    handle.emit(DriverEvent::TurnEnded {
        turn_id: prompt_ids(&handle.commands())[0].clone(),
        stop_reason: StopReason::Error,
        usage: None,
        error: None,
    });
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.stop(&chat.id).await.unwrap();
    h.status(&chat.id, SessionStatus::Stopped).await;
    h.stored(&chat.id).await;
    let conn = rusqlite::Connection::open(h.profile.db_path()).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM agent_sessions WHERE native_session_id IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "no placeholder identity was saved");
    let (next, _) = ScriptedDriver::new(vec![ready_with_identity(None)]);
    h.factory.push(next);
    h.manager.start(&chat.id, false).await.unwrap();
    eventually("restarted driver", || h.factory.setups().len() == 3).await;
    assert_eq!(h.factory.setups()[2].resume, None);
    h.status(&chat.id, SessionStatus::Ready).await;
}

#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "assertions shared by the two database regression tests"
)]
async fn assert_identity_commit_gate(native: Option<&str>) {
    let (driver, handle) = ScriptedDriver::new(vec![ready_with_identity(native)]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager
        .send(&chat.id, "first".into())
        .await
        .expect("first prompt");
    eventually("first prompt", || prompt_ids(&handle.commands()).len() == 1).await;
    h.stored(&chat.id).await;
    let conn = rusqlite::Connection::open(h.profile.db_path()).expect("raw database");
    let row_id: String = conn
        .query_row(
            "SELECT id FROM agent_sessions WHERE chat_id = ?1",
            [&chat.id],
            |row| row.get(0),
        )
        .expect("session row");
    let before: Option<String> = conn
        .query_row(
            "SELECT native_session_id FROM agent_sessions WHERE id = ?1",
            [&row_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(before.as_deref(), native);
    conn.execute_batch("BEGIN IMMEDIATE").expect("hold writes");
    h.manager
        .send(&chat.id, "queued".into())
        .await
        .expect("queued prompt");
    handle.emit(DriverEvent::NativeSessionId(NATIVE.into()));
    handle.emit(DriverEvent::TurnEnded {
        turn_id: prompt_ids(&handle.commands())[0].clone(),
        stop_reason: StopReason::EndTurn,
        usage: None,
        error: None,
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        prompt_ids(&handle.commands()).len(),
        1,
        "identity persistence blocks another prompt"
    );
    conn.execute_batch("ROLLBACK").expect("release writes");
    eventually("next prompt after identity is saved", || {
        prompt_ids(&handle.commands()).len() == 2
    })
    .await;
    let rows: Vec<(String, String)> = conn
        .prepare("SELECT id, native_session_id FROM agent_sessions WHERE chat_id = ?1")
        .expect("prepare")
        .query_map([&chat.id], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect();
    assert_eq!(rows, [(row_id, NATIVE.into())]);
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    let (next, _) = ScriptedDriver::new(vec![ready_step(NATIVE)]);
    h.factory.push(next);
    h.manager.start(&chat.id, false).await.expect("resume");
    eventually("restarted driver", || h.factory.setups().len() == 2).await;
    assert_eq!(h.factory.setups()[1].resume.as_deref(), Some(NATIVE));
}

#[tokio::test(start_paused = true)]
async fn resolved_identity_conflict_stops_the_turn_and_keeps_the_source_row() {
    let (first, _) = ScriptedDriver::new(vec![ready_step(NATIVE)]);
    let (second, handle) = ScriptedDriver::new(vec![ready_step(SOURCE)]);
    let h = Harness::new(vec![first, second]);
    let owner = h.chat().await;
    h.status(&owner.id, SessionStatus::Ready).await;
    let conflicting = h.chat().await;
    h.status(&conflicting.id, SessionStatus::Ready).await;
    h.manager
        .send(&conflicting.id, "first".into())
        .await
        .expect("first prompt");
    eventually("prompt sent", || prompt_ids(&handle.commands()).len() == 1).await;
    h.manager
        .send(&conflicting.id, "queued".into())
        .await
        .expect("queued prompt");
    handle.emit(DriverEvent::NativeSessionId(NATIVE.into()));
    h.status(&conflicting.id, SessionStatus::Failed).await;
    assert_eq!(prompt_ids(&handle.commands()).len(), 1);
    assert!(
        handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::Shutdown))
    );
    assert!(
        h.queue(&conflicting.id)
            .await
            .iter()
            .all(|prompt| prompt.held)
    );
    let events = h.stored(&conflicting.id).await;
    assert!(events.iter().any(|event| matches!(event, GhostEvent::Error { kind: ErrorKind::Internal, message, .. } if message.contains("already open"))));
    assert!(events.iter().any(|event| matches!(
        event,
        GhostEvent::TurnEnded {
            stop_reason: StopReason::Error,
            ..
        }
    )));
    let conn = rusqlite::Connection::open(h.profile.db_path()).expect("raw database");
    let stored: String = conn
        .query_row(
            "SELECT native_session_id FROM agent_sessions WHERE chat_id = ?1",
            [&conflicting.id],
            |row| row.get(0),
        )
        .expect("session row");
    assert_eq!(stored, SOURCE);
    h.status(&owner.id, SessionStatus::Ready).await;
}

#[tokio::test]
async fn rejected_identity_update_never_releases_the_queued_prompt() {
    let (driver, handle) = ScriptedDriver::new(vec![ready_step(SOURCE)]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "first".into()).await.unwrap();
    eventually("first prompt", || prompt_ids(&handle.commands()).len() == 1).await;
    h.stored(&chat.id).await;
    let conn = rusqlite::Connection::open(h.profile.db_path()).unwrap();
    conn.execute_batch(&format!(
        "CREATE TRIGGER reject_native_identity BEFORE UPDATE OF native_session_id ON agent_sessions \
         WHEN NEW.native_session_id = '{NATIVE}' BEGIN SELECT RAISE(ABORT, 'identity update rejected'); END;"
    )).unwrap();
    h.manager.send(&chat.id, "queued".into()).await.unwrap();
    handle.emit(DriverEvent::NativeSessionId(NATIVE.into()));
    handle.emit(DriverEvent::TurnEnded {
        turn_id: prompt_ids(&handle.commands())[0].clone(),
        stop_reason: StopReason::EndTurn,
        usage: None,
        error: None,
    });
    eventually("identity save resolved", || {
        prompt_ids(&handle.commands()).len() > 1
            || handle
                .commands()
                .iter()
                .any(|command| matches!(command, LoggedCommand::Shutdown))
    })
    .await;
    assert_eq!(prompt_ids(&handle.commands()).len(), 1);
    h.status(&chat.id, SessionStatus::Failed).await;
    assert!(h.queue(&chat.id).await.iter().all(|prompt| prompt.held));
    let stored: String = conn
        .query_row(
            "SELECT native_session_id FROM agent_sessions WHERE chat_id = ?1",
            [&chat.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, SOURCE);
    assert!(h.stored(&chat.id).await.iter().any(|event| matches!(
        event,
        GhostEvent::Error {
            kind: ErrorKind::Storage,
            ..
        }
    )));
}

#[tokio::test]
async fn stalled_identity_save_outlives_attach_deadline_across_restart() {
    let (driver, handle) = ScriptedDriver::new(vec![ready_step(SOURCE)]);
    let (next, next_handle) = ScriptedDriver::new(vec![ready_step(NATIVE)]);
    let mut options = Options::default();
    options.timings.attach_timeout = Duration::from_millis(100);
    let h = Harness::with(vec![driver, next], options);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.stored(&chat.id).await;
    let conn = rusqlite::Connection::open(h.profile.db_path()).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    handle.emit(DriverEvent::NativeSessionId(NATIVE.into()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::Shutdown)),
        "an attach deadline cannot cancel a write that may still commit"
    );
    h.manager.stop(&chat.id).await.unwrap();
    h.status(&chat.id, SessionStatus::Stopped).await;
    // Starting reads project folders through the same writer that is still waiting on the
    // identity write, so the new generation cannot become Ready before that write resolves.
    // Everything up to the release stays inside SQLite's busy wait, where the write may still
    // commit.
    h.manager.start(&chat.id, false).await.unwrap();
    h.manager
        .send(&chat.id, "after restart".into())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        prompt_ids(&next_handle.commands()).is_empty(),
        "a new driver generation cannot bypass the identity write"
    );
    assert!(
        h.statuses
            .last(&chat.id)
            .is_none_or(|update| update.status != SessionStatus::Ready),
        "the restart waits for the identity write"
    );
    let stored: String = conn
        .query_row(
            "SELECT native_session_id FROM agent_sessions WHERE chat_id = ?1",
            [&chat.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, SOURCE);
    conn.execute_batch("ROLLBACK").unwrap();
    eventually("prompt after the committed identity", || {
        prompt_ids(&next_handle.commands()).len() == 1
    })
    .await;
    assert_eq!(h.factory.setups()[1].resume.as_deref(), Some(NATIVE));
    let stored: String = conn
        .query_row(
            "SELECT native_session_id FROM agent_sessions WHERE chat_id = ?1",
            [&chat.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, NATIVE);
}
