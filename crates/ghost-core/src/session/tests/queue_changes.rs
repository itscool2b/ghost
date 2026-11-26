//! Editing and reordering queued prompts: the actor's queue, its stored rows and the order the
//! agent receives them in.

use super::*;
use crate::chatstore::QueuedRow;
use ghost_drivers::driver::LoggedCommand;
use ghost_drivers::testing::ScriptHandle;
use ghost_protocol::{PromptPart, composer::ComposerInput};

fn driver() -> (ScriptedDriver, ScriptHandle) {
    ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("queue-changes"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Interrupt),
        Step::EndTurn {
            stop_reason: StopReason::Cancelled,
            usage: None,
            error: None,
        },
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ])
}

/// A chat whose first turn is running, so every later prompt waits in the queue.
async fn busy_chat(rig: &Rig, handle: &ScriptHandle) -> String {
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.send(&chat.id, "work".into()).await.unwrap();
    handle.wait_for(CommandMatch::Prompt).await;
    chat.id
}

async fn queue(rig: &Rig, chat: &str, text: &str) -> String {
    let ack = rig.manager.send(chat, text.into()).await.unwrap();
    assert!(ack.queued);
    ack.prompt_id
}

/// The stored queue, in the order a reopened chat loads it.
async fn stored(rig: &Rig, chat: &str) -> Vec<QueuedRow> {
    let (reply, answer) = tokio::sync::oneshot::channel();
    let chat = chat.to_owned();
    rig.store.with_store(move |store| {
        let rows = crate::chatstore::rows::queued(store.conn(), &chat).expect("queued rows");
        let _ = reply.send(rows);
    });
    answer.await.expect("stored queue")
}

async fn stored_until(
    rig: &Rig,
    chat: &str,
    done: impl Fn(&[QueuedRow]) -> bool,
) -> Vec<QueuedRow> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let rows = stored(rig, chat).await;
        if done(&rows) {
            return rows;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the queue was not saved: {rows:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn first_text(command: &LoggedCommand) -> String {
    match command {
        LoggedCommand::Prompt { parts, .. } => match parts.first() {
            Some(PromptPart::Text { text }) => text.clone(),
            other => panic!("a text part first, got {other:?}"),
        },
        other => panic!("a prompt, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_moved_prompt_keeps_its_place_in_storage_and_goes_first() {
    let (driver, handle) = driver();
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = busy_chat(&rig, &handle).await;
    let a = queue(&rig, &chat, "first").await;
    let b = queue(&rig, &chat, "second").await;
    let c = queue(&rig, &chat, "third").await;

    let moved = rig.manager.move_queued(&chat, &c, 0).await.unwrap();
    let order: Vec<_> = moved.iter().map(|row| row.prompt_id.clone()).collect();
    assert_eq!(order, [c.clone(), a.clone(), b.clone()]);
    // Past the end clamps to the last place; moving back restores nothing else.
    let moved = rig.manager.move_queued(&chat, &a, 99).await.unwrap();
    let order: Vec<_> = moved.iter().map(|row| row.prompt_id.clone()).collect();
    assert_eq!(order, [c.clone(), b.clone(), a.clone()]);

    let wanted = [c.clone(), b.clone(), a.clone()];
    let rows = stored_until(&rig, &chat, |rows| {
        rows.iter()
            .map(|row| row.id.as_str())
            .eq(wanted.iter().map(String::as_str))
    })
    .await;
    assert!(
        rows.windows(2)
            .all(|pair| pair[0].created_at < pair[1].created_at),
        "stored times follow the new order: {rows:?}"
    );

    assert!(matches!(
        rig.manager.move_queued(&chat, "missing", 0).await,
        Err(SessionError::RequestNotPending)
    ));

    // Held by the interrupt, then released: the moved prompt is sent first.
    rig.manager.interrupt(&chat).await.unwrap();
    status(&rig, &chat, SessionStatus::Ready).await;
    rig.manager.resume_queue(&chat).await.unwrap();
    assert_eq!(
        first_text(&handle.wait_for(CommandMatch::Prompt).await),
        "third"
    );
}

#[tokio::test(start_paused = true)]
async fn an_edited_prompt_is_saved_and_sent_with_its_new_words() {
    let (driver, handle) = driver();
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = busy_chat(&rig, &handle).await;
    let a = queue(&rig, &chat, "frist draft").await;
    let b = queue(&rig, &chat, "later").await;

    let edited = rig
        .manager
        .edit_queued(&chat, &a, "first draft".into())
        .await
        .unwrap();
    assert_eq!(edited[0].prompt_id, a);
    assert_eq!(edited[0].preview, "first draft");
    assert_eq!(edited[1].prompt_id, b, "the edit keeps the prompt's place");
    assert_eq!(
        rig.manager.queued_text(&chat, &a).await.unwrap(),
        "first draft"
    );

    assert!(matches!(
        rig.manager.edit_queued(&chat, &a, "   ".into()).await,
        Err(SessionError::PromptEmpty)
    ));
    let limit = ManagerLimits::STANDARD.max_prompt_bytes;
    assert!(matches!(
        rig.manager
            .edit_queued(&chat, &a, "x".repeat(limit + 1))
            .await,
        Err(SessionError::PromptTooLarge { .. })
    ));
    assert!(matches!(
        rig.manager.edit_queued(&chat, "missing", "x".into()).await,
        Err(SessionError::RequestNotPending)
    ));
    assert!(matches!(
        rig.manager.queued_text(&chat, "missing").await,
        Err(SessionError::RequestNotPending)
    ));

    stored_until(&rig, &chat, |rows| {
        rows.first()
            .is_some_and(|row| row.id == a && row.text == "first draft")
    })
    .await;

    rig.manager.interrupt(&chat).await.unwrap();
    status(&rig, &chat, SessionStatus::Ready).await;
    rig.manager.resume_queue(&chat).await.unwrap();
    assert_eq!(
        first_text(&handle.wait_for(CommandMatch::Prompt).await),
        "first draft"
    );
}

#[tokio::test(start_paused = true)]
async fn editing_keeps_a_prompts_mentions_and_their_lines() {
    let (driver, handle) = driver();
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    std::fs::write(rig.folder.join("notes.md"), "notes").unwrap();
    let chat = busy_chat(&rig, &handle).await;
    let ack = rig
        .manager
        .composer_send(
            &chat,
            ComposerInput {
                text: "check this".into(),
                mentions: vec!["notes.md".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(ack.queued);
    assert_eq!(
        rig.manager
            .queued_text(&chat, &ack.prompt_id)
            .await
            .unwrap(),
        "check this",
        "only the typed words are edited"
    );
    rig.manager
        .edit_queued(&chat, &ack.prompt_id, "check that".into())
        .await
        .unwrap();
    let rows = stored_until(&rig, &chat, |rows| {
        rows.first()
            .is_some_and(|row| row.text.starts_with("check that"))
    })
    .await;
    assert_eq!(rows[0].text, "check that\n@notes.md");
    assert!(matches!(
        rows[0].parts.as_slice(),
        [PromptPart::Text { text }, PromptPart::Mention { path }]
            if text == "check that" && path.ends_with("notes.md")
    ));
    // With a mention, the words alone may be cleared: the prompt still says something.
    rig.manager
        .edit_queued(&chat, &ack.prompt_id, String::new())
        .await
        .unwrap();
}
