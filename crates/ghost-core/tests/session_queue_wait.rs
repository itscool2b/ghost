//! Why queued prompts wait (`SessionInfo::queue_wait`), the idle grace that keeps a missing idle
//! report from stalling the queue, and Send now, through the session manager with scripted
//! drivers on paused time. The drivers here take their events from the test, so nothing moves
//! until a test says so.

mod support;

use std::sync::Arc;
use std::time::Duration;

use ghost_drivers::driver::{DriverError, DriverEvent, DriverExit, LoggedCommand};
use ghost_drivers::testing::{CommandMatch, ScriptHandle, ScriptedDriver, Step};
use ghost_protocol::{
    Capabilities, ErrorKind, GhostEvent, HoldCause, NoticeLevel, PromptPart, QueueWait,
    SessionStatus, StopReason, UsageInfo,
};
use support::*;

use helpers::*;

/// What these tests share; test code, so failing loudly with `expect` is right here.
#[cfg(test)]
mod helpers {
    use super::*;

    /// Ready with interrupt support, as every real driver reports it.
    pub fn ready_interruptible(native: &str) -> DriverEvent {
        let mut info = ready_info(native);
        info.capabilities = Capabilities {
            interrupt: true,
            ..Capabilities::default()
        };
        DriverEvent::Ready(info)
    }

    pub fn ended(turn_id: &str, stop_reason: StopReason) -> DriverEvent {
        DriverEvent::TurnEnded {
            turn_id: turn_id.to_owned(),
            stop_reason,
            usage: None,
            error: None,
        }
    }

    /// What a view that joins now is told about the queue's wait.
    pub async fn wait_now(h: &Harness, chat: &str) -> Option<QueueWait> {
        let attach = h
            .manager
            .attach(chat, Sink::new("wait-probe"))
            .await
            .expect("attach");
        h.manager.detach(&attach.subscription_id);
        attach.snapshot.session.queue_wait
    }

    /// Waits until a live frame says the queue's wait is `wanted`.
    pub async fn live_wait(sink: &Arc<Sink>, wanted: Option<QueueWait>) {
        let what = format!("a live header saying the queue waits for {wanted:?}");
        sink.wait_for(
            &what,
            |event| matches!(event, GhostEvent::SessionUpdated(info) if info.queue_wait == wanted),
        )
        .await;
    }

    /// The words of every prompt the driver received, in order.
    pub fn prompts(handle: &ScriptHandle) -> Vec<String> {
        handle
            .commands()
            .into_iter()
            .filter_map(|command| match command {
                LoggedCommand::Prompt { parts, .. } => match parts.as_slice() {
                    [PromptPart::Text { text }] => Some(text.clone()),
                    other => panic!("one text part, got {other:?}"),
                },
                _ => None,
            })
            .collect()
    }

    /// A chat whose driver has reported `Ready` (with `ready`), attached to a recording view.
    pub async fn ready_chat(
        h: &Harness,
        handle: &ScriptHandle,
        ready: DriverEvent,
    ) -> (String, Arc<Sink>) {
        let chat = h.chat().await;
        handle.emit(ready);
        h.status(&chat.id, SessionStatus::Ready).await;
        let sink = Sink::new("main");
        h.manager
            .attach(&chat.id, sink.clone())
            .await
            .expect("attach");
        (chat.id, sink)
    }
}

/// A prompt sent while the agent starts says "Starting", not just "queued", and goes once the
/// agent is ready.
#[tokio::test(start_paused = true)]
async fn a_prompt_sent_while_the_agent_starts_waits_for_it_then_goes() {
    let (driver, handle) = ScriptedDriver::new(Vec::new());
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Starting).await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    let ack = h
        .manager
        .send(&chat.id, "hello".into())
        .await
        .expect("send");
    assert!(ack.queued);
    live_wait(&sink, Some(QueueWait::Starting)).await;
    assert_eq!(wait_now(&h, &chat.id).await, Some(QueueWait::Starting));
    handle.emit(DriverEvent::Ready(ready_info("n1")));
    prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["hello"]);
    assert!(h.queue(&chat.id).await.is_empty());
    assert_eq!(wait_now(&h, &chat.id).await, None);
}

/// A prompt behind a running turn says the agent is working; Send now interrupts that turn and
/// sends it next, ahead of an older one, which keeps its place and is not held.
#[tokio::test(start_paused = true)]
async fn send_now_interrupts_the_running_turn_and_goes_next() {
    let (driver, handle) = ScriptedDriver::new(Vec::new());
    let h = Harness::new(vec![driver]);
    let (chat, _sink) = ready_chat(&h, &handle, ready_interruptible("n1")).await;
    h.manager.send(&chat, "first".into()).await.expect("first");
    let first = prompt_turn(&handle).await;
    let second = h
        .manager
        .send(&chat, "second".into())
        .await
        .expect("second");
    let third = h.manager.send(&chat, "third".into()).await.expect("third");
    assert!(second.queued && third.queued);
    assert_eq!(wait_now(&h, &chat).await, Some(QueueWait::Working));

    let queue = h
        .manager
        .send_now(&chat, &third.prompt_id)
        .await
        .expect("send now");
    let order: Vec<(&str, bool)> = queue
        .iter()
        .map(|row| (row.prompt_id.as_str(), row.held))
        .collect();
    assert_eq!(
        order,
        [
            (third.prompt_id.as_str(), false),
            (second.prompt_id.as_str(), false)
        ]
    );
    command(&handle, CommandMatch::Interrupt).await;
    h.status(&chat, SessionStatus::Cancelling).await;
    assert_eq!(wait_now(&h, &chat).await, Some(QueueWait::Stopping));

    handle.emit(ended(&first, StopReason::Cancelled));
    let next = prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["first", "third"]);
    let queue = h.queue(&chat).await;
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].prompt_id, second.prompt_id);
    assert!(!queue[0].held, "Send now holds nothing");
    assert_eq!(wait_now(&h, &chat).await, Some(QueueWait::Working));
    handle.emit(ended(&next, StopReason::EndTurn));
    prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["first", "third", "second"]);
}

/// An agent that never reports idle after its turn (a Stop hook or a background task that keeps
/// it busy) holds the queue only for the idle grace: the wait says so with when it sends, the
/// prompt then goes, and the log says why.
#[tokio::test(start_paused = true)]
async fn a_missing_idle_report_holds_the_queue_only_for_the_grace() {
    let (driver, handle) =
        ScriptedDriver::new(vec![Step::Emit(DriverEvent::RunState { idle: true })]);
    let h = Harness::new(vec![driver]);
    let (chat, sink) = ready_chat(&h, &handle, DriverEvent::Ready(ready_info("n1"))).await;
    h.manager.send(&chat, "first".into()).await.expect("first");
    let first = prompt_turn(&handle).await;
    assert!(
        h.manager
            .send(&chat, "second".into())
            .await
            .expect("second")
            .queued
    );
    handle.emit(ended(&first, StopReason::EndTurn));
    sink.wait_for("the first turn ended", |event| {
        matches!(event, GhostEvent::TurnEnded { .. })
    })
    .await;
    let Some(QueueWait::FinishingTurn {
        since: Some(since),
        sends_at: Some(sends_at),
    }) = wait_now(&h, &chat).await
    else {
        panic!("the queue waits for the agent to finish its turn, with when it sends");
    };
    assert_eq!(sends_at - since, 15_000, "the standard idle grace is 15 s");
    sink.wait_for("a live header with the same wait", |event| {
        matches!(
            event,
            GhostEvent::SessionUpdated(info)
                if matches!(info.queue_wait, Some(QueueWait::FinishingTurn { .. }))
        )
    })
    .await;
    assert_eq!(
        h.statuses.last(&chat).expect("status").status,
        SessionStatus::Running
    );
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(prompts(&handle), ["first"], "still inside the grace");
    tokio::time::advance(Duration::from_secs(5)).await;
    prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["first", "second"]);
    assert!(h.queue(&chat).await.is_empty());
}

/// An idle report inside the grace sends at once, and the grace never runs while a request waits
/// for the person: an open permission says so, and holds the queue past the grace.
#[tokio::test(start_paused = true)]
async fn an_open_request_holds_the_queue_and_idle_sends_at_once() {
    let (driver, handle) =
        ScriptedDriver::new(vec![Step::Emit(DriverEvent::RunState { idle: true })]);
    let h = Harness::new(vec![driver]);
    let (chat, sink) = ready_chat(&h, &handle, DriverEvent::Ready(ready_info("n1"))).await;
    h.manager.send(&chat, "first".into()).await.expect("first");
    let first = prompt_turn(&handle).await;
    handle.emit(DriverEvent::Permission {
        turn: Some(first.clone()),
        request: permission("p1", None),
    });
    sink.wait_for("the card", |event| {
        matches!(event, GhostEvent::PermissionRequested(_))
    })
    .await;
    h.manager
        .send(&chat, "second".into())
        .await
        .expect("second");
    assert_eq!(
        wait_now(&h, &chat).await,
        Some(QueueWait::NeedsAnswer { question: false })
    );
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(prompts(&handle), ["first"], "a card is never waited out");
    h.manager
        .respond_permission(&chat, "p1", Some("once".into()))
        .await
        .expect("answer");
    command(
        &handle,
        CommandMatch::RespondPermission {
            request_id: Some("p1".into()),
        },
    )
    .await;
    assert_eq!(wait_now(&h, &chat).await, Some(QueueWait::Working));
    handle.emit(ended(&first, StopReason::EndTurn));
    sink.wait_for("the first turn ended", |event| {
        matches!(event, GhostEvent::TurnEnded { .. })
    })
    .await;
    assert!(matches!(
        wait_now(&h, &chat).await,
        Some(QueueWait::FinishingTurn { .. })
    ));
    handle.emit(DriverEvent::RunState { idle: true });
    prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["first", "second"]);
}

/// Once the grace let the queue go, a late busy report with no prompt open cannot make the chat
/// busy again or hold the next prompt; a late idle report changes nothing either.
#[tokio::test(start_paused = true)]
async fn late_reports_after_the_grace_never_make_the_chat_busy_again() {
    let (driver, handle) =
        ScriptedDriver::new(vec![Step::Emit(DriverEvent::RunState { idle: true })]);
    let h = Harness::new(vec![driver]);
    let (chat, sink) = ready_chat(&h, &handle, DriverEvent::Ready(ready_info("n1"))).await;
    h.manager.send(&chat, "first".into()).await.expect("first");
    let first = prompt_turn(&handle).await;
    handle.emit(ended(&first, StopReason::EndTurn));
    sink.wait_for("the first turn ended", |event| {
        matches!(event, GhostEvent::TurnEnded { .. })
    })
    .await;
    h.status(&chat, SessionStatus::Running).await;
    // Nothing queued: the grace still ends the wait, so the next message simply sends.
    h.status(&chat, SessionStatus::Ready).await;
    handle.emit(DriverEvent::RunState { idle: false });
    handle.emit(DriverEvent::Event {
        turn: Some(first.clone()),
        event: message("late", "a late update for the ended turn"),
    });
    sink.wait_for("the late update", |event| {
        matches!(event, GhostEvent::MessageDelta { text, .. } if text == "a late update for the ended turn")
    })
    .await;
    assert_eq!(
        h.statuses.last(&chat).expect("status").status,
        SessionStatus::Ready
    );
    let ack = h
        .manager
        .send(&chat, "second".into())
        .await
        .expect("second");
    assert!(!ack.queued, "the late busy report does not hold the prompt");
    let second = prompt_turn(&handle).await;
    handle.emit(ended(&second, StopReason::EndTurn));
    handle.emit(DriverEvent::RunState { idle: true });
    h.status(&chat, SessionStatus::Ready).await;
    assert!(
        !h.manager
            .send(&chat, "third".into())
            .await
            .expect("third")
            .queued
    );
}

/// A turn-based agent (an ACP agent such as Grok Build) that sends notifications after its turn
/// ended (a title, usage, an update for the ended turn) stays ready: the next prompt goes at once.
#[tokio::test(start_paused = true)]
async fn late_notifications_after_end_turn_keep_a_turn_based_agent_ready() {
    let (driver, handle) = ScriptedDriver::new(Vec::new());
    let h = Harness::new(vec![driver]);
    let (chat, sink) = ready_chat(&h, &handle, DriverEvent::Ready(ready_info("n1"))).await;
    h.manager.send(&chat, "first".into()).await.expect("first");
    let first = prompt_turn(&handle).await;
    handle.emit(ended(&first, StopReason::EndTurn));
    handle.emit(DriverEvent::Title(Some("Fix the login flow".into())));
    handle.emit(DriverEvent::Event {
        turn: None,
        event: GhostEvent::UsageUpdated(UsageInfo {
            context_used: Some(1200),
            context_size: Some(200_000),
            cost: None,
        }),
    });
    handle.emit(DriverEvent::Event {
        turn: Some(first),
        event: GhostEvent::Notice {
            level: NoticeLevel::Info,
            message: "session info for the ended turn".into(),
        },
    });
    sink.wait_for("the late notice", |event| {
        matches!(event, GhostEvent::Notice { message, .. } if message == "session info for the ended turn")
    })
    .await;
    assert_eq!(
        h.statuses.last(&chat).expect("status").status,
        SessionStatus::Ready
    );
    let ack = h
        .manager
        .send(&chat, "second".into())
        .await
        .expect("second");
    assert!(!ack.queued);
    prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["first", "second"]);
}

/// Esc holds what is queued ("Held after you stopped"); Send on one held prompt sends just that
/// one at once, and the rest stay held.
#[tokio::test(start_paused = true)]
async fn a_held_prompt_is_sent_by_the_person_alone() {
    let (driver, handle) = ScriptedDriver::new(Vec::new());
    let h = Harness::new(vec![driver]);
    let (chat, _sink) = ready_chat(&h, &handle, ready_interruptible("n1")).await;
    h.manager.send(&chat, "first".into()).await.expect("first");
    let first = prompt_turn(&handle).await;
    let second = h
        .manager
        .send(&chat, "second".into())
        .await
        .expect("second");
    let third = h.manager.send(&chat, "third".into()).await.expect("third");
    h.manager.interrupt(&chat).await.expect("Esc");
    command(&handle, CommandMatch::Interrupt).await;
    handle.emit(ended(&first, StopReason::Cancelled));
    h.status(&chat, SessionStatus::Ready).await;
    assert_eq!(
        wait_now(&h, &chat).await,
        Some(QueueWait::Held {
            after: HoldCause::Stop
        })
    );
    let queue = h
        .manager
        .send_now(&chat, &third.prompt_id)
        .await
        .expect("send");
    assert_eq!(queue.len(), 1, "the third went at once");
    assert_eq!(queue[0].prompt_id, second.prompt_id);
    assert!(queue[0].held);
    prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["first", "third"]);
    assert_eq!(
        wait_now(&h, &chat).await,
        Some(QueueWait::Held {
            after: HoldCause::Stop
        })
    );
}

/// Prompts still queued at a restart come back held ("Held after a restart"); Send starts the
/// agent and sends that one.
#[tokio::test(start_paused = true)]
async fn a_prompt_held_by_a_restart_is_released_and_starts_the_agent() {
    let (first, first_handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let h = Harness::new(vec![first]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "work".into()).await.expect("send");
    prompt_turn(&first_handle).await;
    let later = h
        .manager
        .send(&chat.id, "later".into())
        .await
        .expect("send");
    assert!(later.queued);
    let (second, handle) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = h.restart(vec![second], Options::default()).await;
    assert_eq!(
        wait_now(&h, &chat.id).await,
        Some(QueueWait::Held {
            after: HoldCause::Restart
        })
    );
    let queue = h
        .manager
        .send_now(&chat.id, &later.prompt_id)
        .await
        .expect("send");
    assert!(!queue[0].held);
    prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["later"]);
}

/// After a restart a new prompt queues behind the held one, which keeps "Held after a restart":
/// nothing starts the agent or sends either of them until the person decides. Send it now sends
/// the held one first, then the new one.
#[tokio::test(start_paused = true)]
async fn a_new_prompt_waits_behind_a_prompt_held_by_a_restart() {
    let (first, first_handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let h = Harness::new(vec![first]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "work".into()).await.expect("send");
    prompt_turn(&first_handle).await;
    let held = h
        .manager
        .send(&chat.id, "older".into())
        .await
        .expect("send");
    let (second, handle) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = h.restart(vec![second], Options::default()).await;
    let newer = h
        .manager
        .send(&chat.id, "newer".into())
        .await
        .expect("send");
    assert!(newer.queued);
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(h.factory.setups().is_empty(), "nothing started the agent");
    assert!(prompts(&handle).is_empty());
    assert_eq!(
        wait_now(&h, &chat.id).await,
        Some(QueueWait::Held {
            after: HoldCause::Restart
        })
    );
    let rows: Vec<(String, bool)> = h
        .queue(&chat.id)
        .await
        .into_iter()
        .map(|row| (row.prompt_id, row.held))
        .collect();
    assert_eq!(
        rows,
        [
            (held.prompt_id.clone(), true),
            (newer.prompt_id.clone(), false)
        ]
    );
    h.manager
        .send_now(&chat.id, &held.prompt_id)
        .await
        .expect("Send it now");
    let turn = prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["older"]);
    assert_eq!(wait_now(&h, &chat.id).await, Some(QueueWait::Working));
    handle.emit(ended(&turn, StopReason::EndTurn));
    prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["older", "newer"]);
}

/// Discarding the held prompt lets the one queued behind it go, starting the agent as a send would.
#[tokio::test(start_paused = true)]
async fn discarding_the_held_prompt_lets_the_next_one_go() {
    let (first, first_handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let h = Harness::new(vec![first]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "work".into()).await.expect("send");
    prompt_turn(&first_handle).await;
    let held = h
        .manager
        .send(&chat.id, "older".into())
        .await
        .expect("send");
    let (second, handle) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = h.restart(vec![second], Options::default()).await;
    h.manager
        .send(&chat.id, "newer".into())
        .await
        .expect("send");
    h.manager
        .cancel_queued(&chat.id, &held.prompt_id)
        .await
        .expect("Discard");
    prompt_turn(&handle).await;
    assert_eq!(prompts(&handle), ["newer"]);
    assert!(h.queue(&chat.id).await.is_empty());
}

/// A sign-in error puts the prompt back held until the agent is signed in.
#[tokio::test(start_paused = true)]
async fn a_sign_in_error_holds_the_prompt_for_the_sign_in() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::EndTurn {
            stop_reason: StopReason::Error,
            usage: None,
            error: Some(DriverError {
                kind: ErrorKind::AuthRequired,
                message: "Authentication required".into(),
                detail: None,
            }),
        },
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager
        .send(&chat.id, "hello".into())
        .await
        .expect("send");
    h.status(&chat.id, SessionStatus::AuthRequired).await;
    assert_eq!(
        wait_now(&h, &chat.id).await,
        Some(QueueWait::Held {
            after: HoldCause::SignIn
        })
    );
}

/// An agent that cannot start keeps the prompt waiting with "could not start"; a folder that is
/// gone says so instead.
#[tokio::test(start_paused = true)]
async fn a_failed_start_and_a_missing_folder_say_why_the_prompt_waits() {
    // No driver left: every start fails.
    let h = Harness::new(Vec::new());
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Failed).await;
    let before = h.statuses.all(&chat.id).len();
    let ack = h
        .manager
        .send(&chat.id, "hello".into())
        .await
        .expect("send");
    assert!(ack.queued);
    // The start the prompt asks for fails at once, so Starting can come and go between two looks
    // at the latest status: read every update since the send instead.
    let statuses = Arc::clone(&h.statuses);
    let id = chat.id.clone();
    eventually("the prompt's start to begin and fail", move || {
        let since: Vec<SessionStatus> = statuses
            .all(&id)
            .into_iter()
            .skip(before)
            .map(|update| update.status)
            .collect();
        since
            .iter()
            .position(|status| *status == SessionStatus::Starting)
            .is_some_and(|start| since[start..].contains(&SessionStatus::Failed))
    })
    .await;
    assert_eq!(wait_now(&h, &chat.id).await, Some(QueueWait::StartFailed));

    let (driver, _) = ScriptedDriver::new(vec![ready_step("n2")]);
    h.factory.push(driver);
    let folder = h.dir.path().join("project");
    std::fs::create_dir_all(&folder).expect("folder");
    let chat = h.manager.create_chat(MOCK, &folder).await.expect("create");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    std::fs::remove_dir_all(&folder).expect("remove the folder");
    h.manager
        .send(&chat.id, "hello".into())
        .await
        .expect("send");
    h.status(&chat.id, SessionStatus::Failed).await;
    assert_eq!(wait_now(&h, &chat.id).await, Some(QueueWait::FolderMissing));
}

/// Chats share a folder: a second chat in the folder of a working chat sends at once, with no
/// admission wait behind the first.
#[tokio::test(start_paused = true)]
async fn a_chat_never_waits_for_another_chat_in_the_same_folder() {
    let (a, a_handle) = ScriptedDriver::new(Vec::new());
    let (b, b_handle) = ScriptedDriver::new(Vec::new());
    let h = Harness::new(vec![a, b]);
    let (first, _) = ready_chat(&h, &a_handle, DriverEvent::Ready(ready_info("a"))).await;
    h.manager.send(&first, "work".into()).await.expect("work");
    prompt_turn(&a_handle).await;
    let (second, _) = ready_chat(&h, &b_handle, DriverEvent::Ready(ready_info("b"))).await;
    let ack = h.manager.send(&second, "also".into()).await.expect("also");
    assert!(!ack.queued);
    prompt_turn(&b_handle).await;
    assert_eq!(prompts(&b_handle), ["also"]);
}
