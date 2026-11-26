use super::*;
use ghost_drivers::driver::{DriverCommand, LoggedCommand, SteerResult};
use ghost_drivers::testing::ScriptHandle;
use ghost_protocol::{
    PromptPart,
    composer::{ComposerInput, SteeringState},
};

fn driver(supported: bool) -> (ScriptedDriver, ScriptHandle) {
    let Step::Emit(DriverEvent::Ready(mut ready)) = ready_step("steering-native") else {
        unreachable!()
    };
    ready.capabilities.steer = supported;
    ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        Step::Emit(DriverEvent::Ready(ready)),
    ])
}

async fn open(rig: &Rig, handle: &ScriptHandle) -> (String, String) {
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(rig, &chat.id, SessionStatus::Ready).await;
    rig.manager
        .send(&chat.id, "Original prompt".into())
        .await
        .unwrap();
    let LoggedCommand::Prompt { turn_id, .. } = handle.wait_for(CommandMatch::Prompt).await else {
        panic!("prompt")
    };
    (chat.id, turn_id)
}

fn input(text: &str) -> ComposerInput {
    ComposerInput {
        text: text.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn steering_saves_exact_input_before_delivery_and_preserves_accepted_context() {
    let (driver, handle) = driver(true);
    handle.set_steer_reply(SteerResult::Accepted);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let (chat, turn) = open(&rig, &handle).await;
    let ack = rig
        .manager
        .composer_steer(&chat, turn.clone(), input("Focus on tests"))
        .await
        .unwrap();
    assert_eq!(ack.state, SteeringState::Accepted);
    let LoggedCommand::Steer {
        message_id,
        turn_id,
        parts,
    } = handle.wait_for(CommandMatch::Steer).await
    else {
        panic!("steer")
    };
    assert_eq!(turn_id, turn);
    assert_eq!(message_id, ack.message_id);
    assert_eq!(
        parts,
        vec![PromptPart::Text {
            text: "Focus on tests".into()
        }]
    );
    rig.store.with_store(move |store| {
        let (state,json): (String,String) = store.conn().query_row("SELECT state,parts_json FROM steering_inputs WHERE message_id=?1",[&ack.message_id],|row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(state,"accepted");
        assert_eq!(serde_json::from_str::<Vec<PromptPart>>(&json).unwrap(),parts);
        let pending: bool = store.conn().query_row("SELECT EXISTS(SELECT 1 FROM events WHERE kind='steering_status' AND json_extract(payload,'$.message_id')=?1 AND json_extract(payload,'$.state')='pending')",[&ack.message_id],|row| row.get(0)).unwrap();
        assert!(pending);
    });
    let context = rig
        .store
        .conversation(
            crate::chatstore::conversation::ConversationCommand::Snapshot {
                chat_id: chat.clone(),
                selected: Some(turn.clone()),
                target_agent: None,
                include_selected: true,
            },
        )
        .await
        .unwrap();
    let crate::chatstore::conversation::ConversationReply::Snapshot(context) = context else {
        panic!("snapshot")
    };
    assert_eq!(
        context.selected.as_ref().unwrap().prompt,
        "Original prompt\n\nFocus on tests"
    );
    assert_eq!(context.selected.as_ref().unwrap().parts.len(), 2);
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn steering_rejection_stays_visible_but_never_enters_retry_context() {
    let (driver, handle) = driver(true);
    handle.set_steer_reply(SteerResult::Rejected {
        message: "Native review cannot be steered.".into(),
    });
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let (chat, turn) = open(&rig, &handle).await;
    assert!(
        rig.manager
            .composer_steer(&chat, "stale-turn".into(), input("stale"))
            .await
            .is_err()
    );
    let ack = rig
        .manager
        .composer_steer(&chat, turn.clone(), input("Undelivered text"))
        .await
        .unwrap();
    assert_eq!(ack.state, SteeringState::Rejected);
    let context = rig
        .store
        .conversation(
            crate::chatstore::conversation::ConversationCommand::Snapshot {
                chat_id: chat.clone(),
                selected: Some(turn),
                target_agent: None,
                include_selected: true,
            },
        )
        .await
        .unwrap();
    let crate::chatstore::conversation::ConversationReply::Snapshot(context) = context else {
        panic!("snapshot")
    };
    assert_eq!(context.selected.as_ref().unwrap().prompt, "Original prompt");
    assert_eq!(context.selected.as_ref().unwrap().parts.len(), 1);
    assert_eq!(
        handle
            .commands()
            .iter()
            .filter(|cmd| matches!(cmd, LoggedCommand::Steer { .. }))
            .count(),
        1
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn unsupported_steering_never_becomes_a_queued_prompt() {
    let (driver, handle) = driver(false);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let (chat, turn) = open(&rig, &handle).await;
    assert!(
        rig.manager
            .composer_steer(&chat, turn, input("Do not queue"))
            .await
            .is_err()
    );
    assert_eq!(
        handle
            .commands()
            .iter()
            .filter(|cmd| matches!(cmd, LoggedCommand::Prompt { .. }))
            .count(),
        1
    );
    assert!(
        !handle
            .commands()
            .iter()
            .any(|cmd| matches!(cmd, LoggedCommand::Steer { .. }))
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

struct LateSteering {
    prompts: mpsc::UnboundedSender<String>,
    release: Arc<tokio::sync::Notify>,
}
impl AgentDriver for LateSteering {
    fn run(self: Box<Self>, mut ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        async move {
            let Step::Emit(DriverEvent::Ready(mut ready)) = ready_step("late-steering") else {
                unreachable!()
            };
            ready.capabilities.steer = true;
            let event = DriverEvent::Ready(ready);
            ctx.events
                .reserve(event.approx_bytes())
                .await
                .unwrap()
                .send(event);
            while let Some(command) = ctx.commands.recv().await {
                match command {
                    DriverCommand::Prompt { turn, .. } => {
                        let _ = self.prompts.send(turn.turn_id);
                    }
                    DriverCommand::Steer { turn_id, reply, .. } => {
                        let event = DriverEvent::TurnEnded {
                            turn_id,
                            stop_reason: StopReason::EndTurn,
                            usage: None,
                            error: None,
                        };
                        ctx.events
                            .reserve(event.approx_bytes())
                            .await
                            .unwrap()
                            .send(event);
                        self.release.notified().await;
                        let _ = reply.send(SteerResult::Accepted);
                    }
                    DriverCommand::SetAutonomy { autonomy, reply } => {
                        let _ = reply.send(Ok(ghost_protocol::AutonomyState {
                            requested: autonomy,
                            native_mode: None,
                            auto_answer: ghost_protocol::AutoAnswer::Never,
                            note: None,
                        }));
                    }
                    DriverCommand::Shutdown { .. } => break,
                    _ => {}
                }
            }
            DriverExit::Closed
        }
        .boxed()
    }
}

#[tokio::test]
async fn turn_compaction_waits_for_the_final_late_steering_event() {
    let (prompts, mut received) = mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let factory = Arc::new(OneDriver(Mutex::new(Some(Box::new(LateSteering {
        prompts,
        release: release.clone(),
    })))));
    let rig = rig_with(factory, WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.send(&chat.id, "Original".into()).await.unwrap();
    let turn = received.recv().await.unwrap();
    let manager = rig.manager.clone();
    let chat_id = chat.id.clone();
    let turn_id = turn.clone();
    let steering = tokio::spawn(async move {
        manager
            .composer_steer(&chat_id, turn_id, input("Late accepted input"))
            .await
    });
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager
        .history(&chat.id, u32::MAX, Some(64 * 1024))
        .await
        .unwrap();
    let check = turn.clone();
    rig.store.with_store(move |store| {
        let (state, compacted): (String, Option<i64>) = store
            .conn()
            .query_row(
                "SELECT status,compacted_at FROM turns WHERE id=?1",
                [check],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "completed");
        assert!(compacted.is_none());
    });
    release.notify_one();
    assert_eq!(
        steering.await.unwrap().unwrap().state,
        SteeringState::Accepted
    );
    rig.manager
        .history(&chat.id, u32::MAX, Some(64 * 1024))
        .await
        .unwrap();
    rig.store.with_store(move|store|{
        let (compacted,last):(Option<i64>,u32)=store.conn().query_row("SELECT compacted_at,last_event_seq FROM turns WHERE id=?1",[&turn],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        let final_seq:u32=store.conn().query_row("SELECT seq FROM events WHERE turn_id=?1 AND kind='steering_status' AND json_extract(payload,'$.state')='accepted'",[turn],|r|r.get(0)).unwrap();
        assert!(compacted.is_some());assert_eq!(last,final_seq);
    });
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}
