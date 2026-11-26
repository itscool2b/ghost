use super::*;
use ghost_drivers::driver::DriverCommand;
use ghost_protocol::session::{ConfigValue, SessionControlChange};
use tokio::sync::Semaphore;

struct DelayedCustomization {
    seen: mpsc::UnboundedSender<&'static str>,
    release: Arc<Semaphore>,
    native: bool,
}
impl AgentDriver for DelayedCustomization {
    fn run(self: Box<Self>, mut ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        async move {
            let options = |value: &str| {
                DriverEvent::Options(vec![ghost_drivers::controls::select(
                    ghost_drivers::controls::ACCESS,
                    "Access",
                    value,
                    vec![
                        ghost_drivers::controls::choice("default", "Ask"),
                        ghost_drivers::controls::choice("future", "Future access"),
                    ],
                )])
            };
            if self.native {
                let event = options("default");
                ctx.events
                    .reserve(event.approx_bytes())
                    .await
                    .unwrap()
                    .send(event);
            }
            let Step::Emit(mut event) = ready_step("delayed-customization") else {
                unreachable!()
            };
            if let DriverEvent::Ready(info) = &mut event {
                info.capabilities.autonomy_levels = vec![
                    ghost_protocol::Autonomy::Ask,
                    ghost_protocol::Autonomy::FullAuto,
                ];
            }
            ctx.events
                .reserve(event.approx_bytes())
                .await
                .unwrap()
                .send(event);
            while let Some(command) = ctx.commands.recv().await {
                match command {
                    DriverCommand::McpAction { reply, .. } => {
                        self.seen.send("pending").unwrap();
                        let _permit = self.release.acquire().await.unwrap();
                        let _ = reply.send(Ok(ghost_protocol::mcp::McpActionResult {
                            state: "observed".into(),
                            message: "No later event follows this reply".into(),
                            authorization_url: None,
                            servers: Vec::new(),
                        }));
                    }
                    DriverCommand::SetOption { value, reply, .. } => {
                        self.seen.send("pending").unwrap();
                        let _permit = self.release.acquire().await.unwrap();
                        let ConfigValue::Choice { value } = value else {
                            panic!()
                        };
                        let event = options(&value);
                        ctx.events
                            .reserve(event.approx_bytes())
                            .await
                            .unwrap()
                            .send(event);
                        let _ = reply.send(Ok(()));
                    }
                    DriverCommand::SetAutonomy { autonomy, reply } => {
                        self.seen.send("pending").unwrap();
                        let _permit = self.release.acquire().await.unwrap();
                        let _ = reply.send(Ok(ghost_protocol::AutonomyState {
                            requested: autonomy,
                            native_mode: None,
                            auto_answer: ghost_protocol::AutoAnswer::Never,
                            note: None,
                        }));
                    }
                    DriverCommand::Prompt { .. } => {
                        self.seen.send("prompt").unwrap();
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
async fn queued_prompt_dispatches_after_mcp_native_control_and_legacy_control_acknowledgements_without_further_events()
 {
    for kind in ["mcp", "native", "legacy"] {
        let (seen, mut observations) = mpsc::unbounded_channel();
        let release = Arc::new(Semaphore::new(0));
        let rig = rig_with(
            Arc::new(OneDriver(Mutex::new(Some(Box::new(
                DelayedCustomization {
                    seen,
                    release: release.clone(),
                    native: kind == "native",
                },
            ))))),
            WriterConfig::STANDARD,
            Knobs::STANDARD,
        );
        let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
        status(&rig, &chat.id, SessionStatus::Ready).await;
        let manager = rig.manager.clone();
        let id = chat.id.clone();
        let target = kind.to_owned();
        let operation = tokio::spawn(async move {
            if target == "mcp" {
                manager
                    .mcp_session_action(
                        &id,
                        "docs".into(),
                        ghost_protocol::mcp::McpSessionAction::Status,
                    )
                    .await
                    .map(|_| ())
            } else {
                manager
                    .set_control(
                        &id,
                        SessionControlChange::Access {
                            value: if target == "native" {
                                "future"
                            } else {
                                "legacy:full_auto"
                            }
                            .into(),
                        },
                    )
                    .await
                    .map(|_| ())
            }
        });
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), observations.recv())
                .await
                .unwrap(),
            Some("pending")
        );
        rig.manager
            .send(&chat.id, "queued while customization waits".into())
            .await
            .unwrap();
        release.add_permits(1);
        operation.await.unwrap().unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), observations.recv())
                .await
                .unwrap(),
            Some("prompt"),
            "{kind} acknowledgement must reopen dispatch"
        );
        rig.manager.shutdown_all(Duration::from_secs(5)).await;
    }
}
