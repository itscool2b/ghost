//! Native drivers can complete a prompt before their background work becomes idle.

mod support;

use std::time::Duration;

use ghost_agents::FixedAgent;
use ghost_drivers::driver::{DriverEvent, DriverExit, LoggedCommand};
use ghost_drivers::testing::{CommandMatch, ScriptHandle, ScriptedDriver, Step};
use ghost_protocol::{GhostEvent, SessionStatus, StopReason};
use support::*;

fn idle(value: bool) -> Step {
    Step::Emit(DriverEvent::RunState { idle: value })
}

fn finish(handle: &ScriptHandle, turn_id: String) {
    handle.emit(DriverEvent::TurnEnded {
        turn_id,
        stop_reason: StopReason::EndTurn,
        usage: None,
        error: None,
    });
}

fn prompt_count(handle: &ScriptHandle) -> usize {
    handle
        .commands()
        .iter()
        .filter(|command| matches!(command, LoggedCommand::Prompt { .. }))
        .count()
}

#[tokio::test(start_paused = true)]
async fn result_ends_prompt_but_only_idle_dispatches_the_next_one() {
    let (driver, handle) = ScriptedDriver::new(vec![idle(true), ready_step("native")]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.manager
        .send(&chat.id, "first".into())
        .await
        .expect("first");
    let first = prompt_turn(&handle).await;
    assert!(
        h.manager
            .send(&chat.id, "second".into())
            .await
            .expect("second")
            .queued
    );
    // The initial idle report is invalidated locally on dispatch, so even a driver that sends
    // its result before a running report cannot dispatch the queued prompt early.
    finish(&handle, first);
    sink.wait_for("the first prompt ended", |event| {
        matches!(event, GhostEvent::TurnEnded { .. })
    })
    .await;
    assert_eq!(
        h.statuses.last(&chat.id).expect("status").status,
        SessionStatus::Running
    );
    assert_eq!(prompt_count(&handle), 1);
    assert_eq!(h.queue(&chat.id).await.len(), 1);
    handle.emit(DriverEvent::RunState { idle: true });
    prompt_turn(&handle).await;
    assert_eq!(prompt_count(&handle), 2);
    assert!(h.queue(&chat.id).await.is_empty());
    assert!(
        h.manager
            .send(&chat.id, "third".into())
            .await
            .expect("third")
            .queued
    );
}

#[tokio::test(start_paused = true)]
async fn idle_before_result_still_waits_for_the_prompt_to_end() {
    let (driver, handle) = ScriptedDriver::new(vec![idle(true), ready_step("native")]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.manager
        .send(&chat.id, "first".into())
        .await
        .expect("first");
    let first = prompt_turn(&handle).await;
    handle.emit(DriverEvent::RunState { idle: true });
    handle.emit(DriverEvent::Event {
        turn: Some(first.clone()),
        event: message("marker", "idle observed"),
    });
    sink.wait_for(
        "idle was processed",
        |event| matches!(event, GhostEvent::MessageDelta { text, .. } if text == "idle observed"),
    )
    .await;
    assert!(
        h.manager
            .send(&chat.id, "second".into())
            .await
            .expect("second")
            .queued
    );
    assert_eq!(prompt_count(&handle), 1);
    finish(&handle, first);
    prompt_turn(&handle).await;
    assert_eq!(prompt_count(&handle), 2);
}

#[tokio::test(start_paused = true)]
async fn background_permission_keeps_interrupt_meaningful_without_an_open_prompt() {
    let (driver, handle) = ScriptedDriver::new(vec![idle(true), ready_step("native")]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    handle.emit(DriverEvent::RunState { idle: false });
    handle.emit(DriverEvent::Permission {
        turn: None,
        request: permission("background", None),
    });
    sink.wait_for("background permission", |event| {
        matches!(event, GhostEvent::PermissionRequested(_))
    })
    .await;
    h.status(&chat.id, SessionStatus::Running).await;
    h.manager
        .send(&chat.id, "queued".into())
        .await
        .expect("queued");
    h.manager.interrupt(&chat.id).await.expect("interrupt");
    command(&handle, CommandMatch::Interrupt).await;
    h.status(&chat.id, SessionStatus::Cancelling).await;
    // Idle alone cannot discard a still-open background permission.
    handle.emit(DriverEvent::RunState { idle: true });
    handle.emit(DriverEvent::Event {
        turn: None,
        event: message("marker", "still awaiting withdrawal"),
    });
    sink.wait_for("idle was processed", |event| matches!(event, GhostEvent::MessageDelta { text, .. } if text == "still awaiting withdrawal")).await;
    assert_eq!(
        h.statuses.last(&chat.id).expect("status").status,
        SessionStatus::Cancelling
    );
    assert_eq!(prompt_count(&handle), 0);
    handle.emit(DriverEvent::PermissionWithdrawn {
        request_id: "background".into(),
    });
    h.status(&chat.id, SessionStatus::Ready).await;
    assert!(h.queue(&chat.id).await.iter().all(|prompt| prompt.held));
    assert!(
        !sink
            .events()
            .iter()
            .any(|(_, event)| matches!(event, GhostEvent::TurnEnded { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn background_work_is_not_idle_evicted_and_quit_still_stops_it() {
    let (driver, handle) = ScriptedDriver::new(vec![
        idle(false),
        ready_step("native"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Running).await;
    tokio::time::advance(Duration::from_secs(60 * 60)).await;
    // A mailbox round trip gives any expired eviction timer a chance to run.
    h.manager.lists(&chat.id).await.expect("lists");
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::Shutdown))
    );
    h.manager.shutdown_all(Duration::from_secs(8)).await;
    assert_eq!(handle.commands(), [LoggedCommand::Shutdown]);
    h.status(&chat.id, SessionStatus::Stopped).await;
}

#[tokio::test(start_paused = true)]
async fn native_and_acp_chats_cannot_own_the_same_claude_session() {
    let mut native = mock_spec();
    native.id = "claude";
    let mut acp = mock_spec();
    acp.id = "claude-acp";
    let options = Options {
        agents: vec![
            FixedAgent::new(native, ready(), None),
            FixedAgent::new(acp, ready(), None),
        ],
        ..Options::default()
    };
    let (first, _) = ScriptedDriver::new(vec![ready_step("shared")]);
    let (second, second_handle) = ScriptedDriver::new(vec![
        ready_step("shared"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let h = Harness::with(vec![first, second], options);
    let native = h
        .manager
        .create_chat("claude", &h.folder)
        .await
        .expect("native chat");
    h.status(&native.id, SessionStatus::Ready).await;
    // One Claude Code: the second chat reaches the adapter through the Connection setting.
    h.detector
        .set_connection("claude", Some("claude-acp"))
        .await
        .expect("the adapter is Claude's");
    let acp = h
        .manager
        .create_chat("claude", &h.folder)
        .await
        .expect("ACP chat");
    assert_eq!(
        (native.agent_id.as_str(), acp.agent_id.as_str()),
        ("claude", "claude")
    );
    h.status(&acp.id, SessionStatus::Failed).await;
    assert_eq!(second_handle.commands(), [LoggedCommand::Shutdown]);
    let connections: Vec<String> = h
        .factory
        .setups()
        .iter()
        .map(|setup| setup.agent_id.clone())
        .collect();
    assert_eq!(connections, ["claude", "claude-acp"]);
    // The same namespace applies to a persisted resume id, before its driver is created.
    h.manager.stop(&native.id).await.expect("stop native");
    h.status(&native.id, SessionStatus::Stopped).await;
    let (next, _) = ScriptedDriver::new(vec![ready_step("shared")]);
    h.factory.push(next);
    h.manager.start(&acp.id, false).await.expect("start ACP");
    h.status(&acp.id, SessionStatus::Ready).await;
    let count = h.factory.setups().len();
    h.manager
        .start(&native.id, false)
        .await
        .expect("resume native");
    h.status(&native.id, SessionStatus::Failed).await;
    assert_eq!(h.factory.setups().len(), count);
}

/// Work an agent runs on its own (background tasks it reports, with no prompt of Ghost's
/// open) keeps it from being stopped to make room, even once it counts as idle for the queue.
#[tokio::test(start_paused = true)]
async fn background_work_is_not_stopped_to_make_room() {
    let (background, background_handle) =
        ScriptedDriver::new(vec![idle(true), ready_step("background")]);
    let (quiet, quiet_handle) = ScriptedDriver::new(vec![idle(true), ready_step("quiet")]);
    let (third, _) = ScriptedDriver::new(vec![idle(true), ready_step("third")]);
    let options = Options {
        limits: ghost_core::session::ManagerLimits {
            max_live_agents: 2,
            ..ghost_core::session::ManagerLimits::STANDARD
        },
        ..Options::default()
    };
    let h = Harness::with(vec![background, quiet, third], options);
    let busy = h.chat().await;
    h.status(&busy.id, SessionStatus::Ready).await;
    background_handle.emit(DriverEvent::RunState { idle: false });
    h.status(&busy.id, SessionStatus::Running).await;
    // Past the idle grace the queue treats it as idle, yet its work still runs.
    tokio::time::advance(Duration::from_secs(20)).await;
    let quiet = h.chat().await;
    h.status(&quiet.id, SessionStatus::Ready).await;
    let third = h.chat().await;
    h.status(&third.id, SessionStatus::Ready).await;
    h.status(&quiet.id, SessionStatus::Stopped).await;
    assert_eq!(quiet_handle.commands(), [LoggedCommand::Shutdown]);
    assert!(
        !background_handle
            .commands()
            .contains(&LoggedCommand::Shutdown),
        "an agent working in the background is never stopped for room"
    );
}
