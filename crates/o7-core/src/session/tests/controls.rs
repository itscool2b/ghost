use super::*;
use o7_drivers::driver::{DriverCommand, LoggedCommand};
use o7_protocol::session::{
    ConfigCategory, ConfigKind, ConfigOption, ConfigValue, SessionControlChange,
    SessionControlPreferences, SessionLists,
};
use o7_protocol::{Autonomy, AutonomyState};

struct NativeControls;
impl AgentDriver for NativeControls {
    fn run(self: Box<Self>, mut ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        async move {
            let mut access = "default".to_owned();
            let mut workflow = "execute".to_owned();
            let mut active = None;
            let options = |access: &str, workflow: &str| {
                DriverEvent::Options(vec![
                    o7_drivers::controls::select(
                        o7_drivers::controls::ACCESS,
                        "Access",
                        access,
                        vec![
                            o7_drivers::controls::choice("default", "Ask"),
                            o7_drivers::controls::choice("future-access", "Future native access"),
                        ],
                    ),
                    o7_drivers::controls::workflow(workflow),
                ])
            };
            let event = options(&access, &workflow);
            ctx.events
                .reserve(event.approx_bytes())
                .await
                .unwrap()
                .send(event);
            let Step::Emit(event) = ready_step("control-native") else {
                unreachable!()
            };
            ctx.events
                .reserve(event.approx_bytes())
                .await
                .unwrap()
                .send(event);
            while let Some(command) = ctx.commands.recv().await {
                match command {
                    DriverCommand::SetOption {
                        option_id,
                        value,
                        reply,
                    } => {
                        if let ConfigValue::Choice { value } = value {
                            if option_id == o7_drivers::controls::ACCESS {
                                access = value;
                            } else {
                                workflow = value;
                            }
                        }
                        let event = options(&access, &workflow);
                        ctx.events
                            .reserve(event.approx_bytes())
                            .await
                            .unwrap()
                            .send(event);
                        let _ = reply.send(Ok(()));
                    }
                    DriverCommand::Prompt { turn, .. } => {
                        active = Some(turn.turn_id);
                    }
                    DriverCommand::Interrupt => {
                        if let Some(turn_id) = active.take() {
                            let event = DriverEvent::TurnEnded {
                                turn_id,
                                stop_reason: StopReason::Cancelled,
                                usage: None,
                                error: None,
                            };
                            ctx.events
                                .reserve(event.approx_bytes())
                                .await
                                .unwrap()
                                .send(event);
                        }
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
fn native_rig() -> Rig {
    rig_with(
        Arc::new(OneDriver(Mutex::new(Some(Box::new(NativeControls))))),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    )
}
#[tokio::test]
async fn native_controls_keep_custom_access_and_plan_persistence_separate_from_flat_options() {
    let rig = native_rig();
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let lists = rig
        .manager
        .set_control(
            &chat.id,
            SessionControlChange::Access {
                value: "future-access".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        lists.controls.access.effective.as_deref(),
        Some("future-access")
    );
    assert!(
        rig.manager
            .set_control(
                &chat.id,
                SessionControlChange::Access {
                    value: "not-offered".into()
                }
            )
            .await
            .is_err()
    );
    let lists = rig
        .manager
        .set_control(
            &chat.id,
            SessionControlChange::Workflow {
                value: "plan".into(),
                implementation_approved: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(lists.controls.workflow.effective.as_deref(), Some("plan"));
    assert!(
        rig.manager
            .set_control(
                &chat.id,
                SessionControlChange::Workflow {
                    value: "execute".into(),
                    implementation_approved: false
                }
            )
            .await
            .is_err()
    );
    assert!(
        rig.manager
            .set_option(
                &chat.id,
                o7_drivers::controls::WORKFLOW,
                ConfigValue::Choice {
                    value: "execute".into()
                }
            )
            .await
            .is_err()
    );
    let lists = rig
        .manager
        .set_control(
            &chat.id,
            SessionControlChange::Workflow {
                value: "execute".into(),
                implementation_approved: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        lists.controls.workflow.effective.as_deref(),
        Some("execute")
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
    let loaded = rig.store.load_chat(&chat.id).await.unwrap();
    let saved = loaded.agent_session.unwrap();
    let controls: SessionControlPreferences =
        serde_json::from_str(saved.controls_json.as_deref().unwrap()).unwrap();
    assert_eq!(controls.access.as_deref(), Some("future-access"));
    assert_eq!(controls.workflow.as_deref(), Some("execute"));
    let flat: serde_json::Value =
        serde_json::from_str(saved.config_json.as_deref().unwrap()).unwrap();
    assert!(flat.get(o7_drivers::controls::WORKFLOW).is_none());
    assert!(flat.get(o7_drivers::controls::ACCESS).is_none());
}
#[tokio::test]
async fn queued_or_running_work_cannot_cross_a_native_plan_transition() {
    let rig = native_rig();
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.send(&chat.id, "first".into()).await.unwrap();
    assert!(
        rig.manager
            .send(&chat.id, "queued".into())
            .await
            .unwrap()
            .queued
    );
    assert!(
        rig.manager
            .set_control(
                &chat.id,
                SessionControlChange::Workflow {
                    value: "plan".into(),
                    implementation_approved: false
                }
            )
            .await
            .is_err()
    );
    rig.manager.interrupt(&chat.id).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert!(
        rig.manager
            .set_control(
                &chat.id,
                SessionControlChange::Workflow {
                    value: "plan".into(),
                    implementation_approved: false
                }
            )
            .await
            .is_err(),
        "a held queue also blocks idle transitions"
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}
#[tokio::test]
async fn unsupported_access_is_not_presented_as_available() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("limited"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert!(
        rig.manager
            .lists(&chat.id)
            .await
            .unwrap()
            .controls
            .access
            .choices
            .is_empty()
    );
    assert!(
        rig.manager
            .set_control(
                &chat.id,
                SessionControlChange::Access {
                    value: "legacy:full_auto".into()
                }
            )
            .await
            .is_err()
    );
    assert!(
        !handle
            .commands()
            .iter()
            .any(|c| matches!(c, LoggedCommand::SetAutonomy { .. }))
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn only_explicit_native_plan_approval_allows_a_reported_execution_transition() {
    for approved in [false, true] {
        let options = |workflow: &str| {
            DriverEvent::Options(vec![
                o7_drivers::controls::select(
                    o7_drivers::controls::ACCESS,
                    "Access",
                    "default",
                    vec![o7_drivers::controls::choice("default", "Ask")],
                ),
                o7_drivers::controls::workflow(workflow),
            ])
        };
        let (driver, handle) =
            ScriptedDriver::new(vec![Step::Emit(options("plan")), ready_step("native-plan")]);
        let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
        let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
        status(&rig, &chat.id, SessionStatus::Ready).await;
        if approved {
            handle.emit(DriverEvent::Permission {
                turn: None,
                request: o7_protocol::PermissionRequest {
                    request_id: "implement".into(),
                    tool_call_id: None,
                    title: "Implement the plan?".into(),
                    kind: Some(o7_protocol::ToolKind::SwitchMode),
                    raw_input: None,
                    content: Vec::new(),
                    locations: Vec::new(),
                    options: vec![o7_protocol::PermissionOption {
                        option_id: "yes".into(),
                        kind: o7_protocol::PermissionOptionKind::AllowOnce,
                        label: "Approve".into(),
                    }],
                    truncated: None,
                },
            });
            eventually("native approval card", || {
                lock(&rig.statuses.updates)
                    .iter()
                    .rev()
                    .find(|u| u.chat_id == chat.id)
                    .is_some_and(|u| u.needs_attention)
            })
            .await;
            rig.manager
                .respond_permission(&chat.id, "implement", Some("yes".into()))
                .await
                .unwrap();
        }
        handle.emit(options("execute"));
        tokio::time::sleep(Duration::from_millis(20)).await;
        let lists = rig.manager.lists(&chat.id).await.unwrap();
        assert_eq!(
            lists.controls.workflow.requested.as_deref(),
            Some(if approved { "execute" } else { "plan" })
        );
        if !approved {
            assert!(rig.manager.send(&chat.id, "do work".into()).await.is_err());
        }
        rig.manager.shutdown_all(Duration::from_secs(5)).await;
    }
}

/// The owner's report: a plan approved during a turn, the turn stopped, and Send refused until
/// Execute was chosen again. The approval must carry the session out of Plan by itself.
#[tokio::test]
async fn a_plan_approved_mid_turn_still_sends_after_stop() {
    let options = |workflow: &str| {
        DriverEvent::Options(vec![
            o7_drivers::controls::select(
                o7_drivers::controls::ACCESS,
                "Access",
                "bypassPermissions",
                vec![o7_drivers::controls::choice(
                    "bypassPermissions",
                    "Full access",
                )],
            ),
            o7_drivers::controls::workflow(workflow),
        ])
    };
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Emit(options("plan")),
        ready_step("plan-then-stop"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("exit-plan".into()),
        }),
        Step::Expect(CommandMatch::Interrupt),
        Step::EndTurn {
            stop_reason: StopReason::Cancelled,
            usage: None,
            error: None,
        },
        Step::Expect(CommandMatch::Prompt),
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.send(&chat.id, "plan it".into()).await.unwrap();
    handle.wait_for(CommandMatch::Prompt).await;
    handle.emit(DriverEvent::Permission {
        turn: None,
        request: o7_protocol::PermissionRequest {
            request_id: "exit-plan".into(),
            tool_call_id: None,
            title: "ExitPlanMode".into(),
            kind: Some(o7_protocol::ToolKind::SwitchMode),
            raw_input: Some(serde_json::json!({"plan": "# Plan"})),
            content: Vec::new(),
            locations: Vec::new(),
            options: vec![o7_protocol::PermissionOption {
                option_id: "allow".into(),
                kind: o7_protocol::PermissionOptionKind::AllowOnce,
                label: "Allow once".into(),
            }],
            truncated: None,
        },
    });
    eventually("plan approval card", || {
        lock(&rig.statuses.updates)
            .iter()
            .rev()
            .find(|u| u.chat_id == chat.id)
            .is_some_and(|u| u.needs_attention)
    })
    .await;
    rig.manager
        .respond_permission(&chat.id, "exit-plan", Some("allow".into()))
        .await
        .unwrap();
    // Claude leaves Plan on its own and reports it while the turn goes on.
    handle.emit(options("execute"));
    tokio::time::sleep(Duration::from_millis(20)).await;
    rig.manager.interrupt(&chat.id).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let lists = rig.manager.lists(&chat.id).await.unwrap();
    assert_eq!(
        lists.controls.workflow.requested.as_deref(),
        Some("execute")
    );
    rig.manager
        .send(&chat.id, "keep going".into())
        .await
        .expect("a message after Stop sends without choosing Execute again");
    handle.wait_for(CommandMatch::Prompt).await;
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn legacy_live_access_obeys_native_idle_queue_and_approval_admission_but_stopped_defaults_remain_editable()
 {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("legacy-admission"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Interrupt),
        Step::EndTurn {
            stop_reason: StopReason::Cancelled,
            usage: None,
            error: None,
        },
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.send(&chat.id, "running".into()).await.unwrap();
    handle.wait_for(CommandMatch::Prompt).await;
    assert!(
        rig.manager
            .set_autonomy(&chat.id, Autonomy::Ask)
            .await
            .is_err()
    );
    let queued = rig.manager.send(&chat.id, "queued".into()).await.unwrap();
    assert!(queued.queued);
    rig.manager.interrupt(&chat.id).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert!(
        rig.manager
            .set_autonomy(&chat.id, Autonomy::Ask)
            .await
            .is_err(),
        "held queue blocks legacy access changes"
    );
    rig.manager
        .cancel_queued(&chat.id, &queued.prompt_id)
        .await
        .unwrap();
    rig.manager
        .set_autonomy(&chat.id, Autonomy::Ask)
        .await
        .unwrap();
    handle.emit(DriverEvent::Permission {
        turn: None,
        request: o7_protocol::PermissionRequest {
            request_id: "access-approval".into(),
            tool_call_id: None,
            title: "Allow read?".into(),
            kind: Some(o7_protocol::ToolKind::Read),
            raw_input: None,
            content: Vec::new(),
            locations: Vec::new(),
            options: vec![o7_protocol::PermissionOption {
                option_id: "allow".into(),
                kind: o7_protocol::PermissionOptionKind::AllowOnce,
                label: "Allow".into(),
            }],
            truncated: None,
        },
    });
    eventually("approval shown", || {
        lock(&rig.statuses.updates)
            .iter()
            .rev()
            .find(|u| u.chat_id == chat.id)
            .is_some_and(|u| u.needs_attention)
    })
    .await;
    assert!(
        rig.manager
            .set_autonomy(&chat.id, Autonomy::FullAuto)
            .await
            .is_err()
    );
    rig.manager
        .respond_permission(&chat.id, "access-approval", None)
        .await
        .unwrap();
    rig.manager.stop(&chat.id).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Stopped).await;
    let lists = rig
        .manager
        .set_autonomy(&chat.id, Autonomy::FullAuto)
        .await
        .unwrap();
    assert_eq!(lists.autonomy.requested, Autonomy::FullAuto);
    assert_eq!(
        handle
            .commands()
            .iter()
            .filter(|c| matches!(c, LoggedCommand::SetAutonomy { .. }))
            .count(),
        1,
        "only the idle live choice reached the native driver"
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

/// One start's level and the native access its setup asked for.
type Start = (Autonomy, Option<String>);

/// What a Claude-like agent did: each start, then the changes and prompts it received, in order.
#[derive(Clone, Default)]
struct Seen {
    starts: Arc<Mutex<Vec<Start>>>,
    commands: Arc<Mutex<Vec<String>>>,
    /// The working mode each start's setup asked for.
    workflows: Arc<Mutex<Vec<Option<String>>>>,
}

/// Starts like Claude Code: it lists a model, native access and workflow, starts on the access
/// its setup asks for (else the one its level maps to), and reports Ready once `ready` has a
/// permit (a test holds a start by leaving it empty).
struct ClaudeLike {
    seen: Seen,
    ready: Arc<tokio::sync::Semaphore>,
}

fn native_access(autonomy: Autonomy) -> &'static str {
    match autonomy {
        Autonomy::FullAuto => "bypassPermissions",
        Autonomy::AutoEdit => "acceptEdits",
        Autonomy::Ask => "default",
    }
}

fn claude_options(access: &str, workflow: &str) -> DriverEvent {
    let model = ConfigOption {
        id: "model".into(),
        name: "Model".into(),
        description: None,
        category: ConfigCategory::Model,
        kind: ConfigKind::Select {
            current: "opus".into(),
            choices: vec![o7_drivers::controls::choice("opus", "Opus")],
        },
        pending: None,
    };
    DriverEvent::Options(vec![
        model,
        o7_drivers::controls::select(
            o7_drivers::controls::ACCESS,
            "Access",
            access,
            vec![
                o7_drivers::controls::choice("default", "Ask for approval"),
                o7_drivers::controls::choice("acceptEdits", "Allow edits"),
                o7_drivers::controls::choice("bypassPermissions", "Full access"),
            ],
        ),
        o7_drivers::controls::workflow(workflow),
    ])
}

async fn emit(events: &o7_drivers::driver::EventSender, event: DriverEvent) {
    events
        .reserve(event.approx_bytes())
        .await
        .unwrap()
        .send(event);
}

impl AgentDriver for ClaudeLike {
    fn run(self: Box<Self>, mut ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        async move {
            let requested = match ctx.setup.desired.get(o7_drivers::controls::ACCESS) {
                Some(ConfigValue::Choice { value }) => Some(value.clone()),
                _ => None,
            };
            lock(&self.seen.starts).push((ctx.setup.autonomy, requested.clone()));
            let asked = match ctx.setup.desired.get(o7_drivers::controls::WORKFLOW) {
                Some(ConfigValue::Choice { value }) => Some(value.clone()),
                _ => None,
            };
            lock(&self.seen.workflows).push(asked.clone());
            let workflow = asked.unwrap_or_else(|| "execute".to_owned());
            let mut access =
                requested.unwrap_or_else(|| native_access(ctx.setup.autonomy).to_owned());
            let Ok(permit) = self.ready.acquire().await else {
                return DriverExit::Closed;
            };
            permit.forget();
            emit(&ctx.events, claude_options(&access, &workflow)).await;
            emit(
                &ctx.events,
                DriverEvent::Autonomy(AutonomyState {
                    requested: ctx.setup.autonomy,
                    native_mode: Some(access.clone()),
                    auto_answer: o7_protocol::AutoAnswer::Never,
                    note: None,
                }),
            )
            .await;
            let Step::Emit(DriverEvent::Ready(mut info)) =
                ready_step(&format!("native-{}", ctx.setup.chat_id))
            else {
                unreachable!()
            };
            info.capabilities.autonomy_levels =
                vec![Autonomy::FullAuto, Autonomy::AutoEdit, Autonomy::Ask];
            info.capabilities.resume = true;
            if ctx.setup.resume.is_some() {
                info.resumed = ResumeOutcome::Resumed;
            }
            emit(&ctx.events, DriverEvent::Ready(info)).await;
            while let Some(command) = ctx.commands.recv().await {
                match command {
                    DriverCommand::SetOption {
                        option_id,
                        value: ConfigValue::Choice { value },
                        reply,
                    } if option_id == o7_drivers::controls::ACCESS => {
                        lock(&self.seen.commands).push(format!("access:{value}"));
                        access = value;
                        emit(&ctx.events, claude_options(&access, &workflow)).await;
                        let _ = reply.send(Ok(()));
                    }
                    DriverCommand::SetAutonomy { autonomy, reply } => {
                        access = native_access(autonomy).to_owned();
                        lock(&self.seen.commands).push(format!("level:{access}"));
                        emit(&ctx.events, claude_options(&access, &workflow)).await;
                        let _ = reply.send(Ok(AutonomyState {
                            requested: autonomy,
                            native_mode: Some(access.clone()),
                            auto_answer: o7_protocol::AutoAnswer::Never,
                            note: None,
                        }));
                    }
                    DriverCommand::Prompt { turn, .. } => {
                        lock(&self.seen.commands).push(format!("prompt with {access}"));
                        emit(
                            &ctx.events,
                            DriverEvent::TurnEnded {
                                turn_id: turn.turn_id,
                                stop_reason: StopReason::EndTurn,
                                usage: None,
                                error: None,
                            },
                        )
                        .await;
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

struct ClaudeLikeFactory {
    seen: Seen,
    ready: Arc<tokio::sync::Semaphore>,
}

impl DriverFactory for ClaudeLikeFactory {
    fn create(&self, _kind: o7_protocol::DriverKind) -> Box<dyn AgentDriver> {
        Box::new(ClaudeLike {
            seen: self.seen.clone(),
            ready: Arc::clone(&self.ready),
        })
    }
}

/// Starts report Ready at once unless `held`, when each waits for `ready.add_permits(1)`.
fn claude_rig(held: bool) -> (Rig, Seen, Arc<tokio::sync::Semaphore>) {
    let seen = Seen::default();
    let ready = Arc::new(tokio::sync::Semaphore::new(if held { 0 } else { 1000 }));
    let rig = rig_with(
        Arc::new(ClaudeLikeFactory {
            seen: seen.clone(),
            ready: Arc::clone(&ready),
        }),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    (rig, seen, ready)
}

fn access_values(lists: &SessionLists) -> Vec<String> {
    lists
        .controls
        .access
        .choices
        .iter()
        .map(|choice| choice.value.clone())
        .collect()
}

/// The owner's report, 6 November 2026: a chat opened after a restart said "Access unavailable"
/// until a message started its agent. Its access is named and changeable before the agent
/// starts, and the next start asks for the change.
#[tokio::test]
async fn a_reopened_chat_names_its_access_and_keeps_a_change_for_its_next_start() {
    let (mut rig, seen, _ready) = claude_rig(false);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager
        .set_control(
            &chat.id,
            SessionControlChange::Access {
                value: "acceptEdits".into(),
            },
        )
        .await
        .unwrap();
    super::conversation::reopen_manager(&mut rig).await;
    // The saved catalogs load as the manager opens.
    let mut lists = rig.manager.lists(&chat.id).await.unwrap();
    for _ in 0..600 {
        if !lists.controls.access.choices.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        lists = rig.manager.lists(&chat.id).await.unwrap();
    }
    assert_eq!(
        access_values(&lists),
        ["default", "acceptEdits", "bypassPermissions"]
    );
    assert_eq!(
        lists.controls.access.effective.as_deref(),
        Some("acceptEdits")
    );
    assert_eq!(
        lists.controls.access.requested.as_deref(),
        Some("acceptEdits")
    );
    assert_eq!(lists.controls.access.note, None, "nothing waits to apply");
    assert_eq!(
        lists.controls.workflow.effective.as_deref(),
        Some("execute")
    );
    assert_eq!(lists.controls.workflow.note, None);
    // Chosen while stopped: kept for the start, which asks for it.
    let lists = rig
        .manager
        .set_control(
            &chat.id,
            SessionControlChange::Access {
                value: "default".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(lists.controls.access.effective.as_deref(), Some("default"));
    // The owner's report, 23 November 2026: a chat opened after a restart could not be put in
    // Plan until a message had started its agent. The mode is kept for the start as access is.
    assert!(
        rig.manager
            .set_control(
                &chat.id,
                SessionControlChange::Workflow {
                    value: "review".into(),
                    implementation_approved: false,
                },
            )
            .await
            .is_err(),
        "only a mode the connection listed"
    );
    let lists = rig
        .manager
        .set_control(
            &chat.id,
            SessionControlChange::Workflow {
                value: "plan".into(),
                implementation_approved: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(lists.controls.workflow.effective.as_deref(), Some("plan"));
    assert_eq!(lists.controls.workflow.requested.as_deref(), Some("plan"));
    assert!(
        rig.manager
            .set_control(
                &chat.id,
                SessionControlChange::Workflow {
                    value: "execute".into(),
                    implementation_approved: false,
                },
            )
            .await
            .is_err(),
        "leaving Plan takes the person's approval, running or not"
    );
    rig.manager.send(&chat.id, "go".into()).await.unwrap();
    eventually("the prompt", || {
        lock(&seen.commands).iter().any(|c| c.starts_with("prompt"))
    })
    .await;
    assert_eq!(
        lock(&seen.starts).last().cloned(),
        Some((Autonomy::FullAuto, Some("default".into())))
    );
    assert_eq!(
        lock(&seen.commands).as_slice(),
        ["access:acceptEdits", "prompt with default"],
        "the start applied it; nothing changed it again"
    );
    assert_eq!(
        lock(&seen.workflows).last().cloned(),
        Some(Some("plan".into())),
        "the start asked for Plan"
    );
    let lists = rig.manager.lists(&chat.id).await.unwrap();
    assert_eq!(lists.controls.workflow.effective.as_deref(), Some("plan"));
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

/// A new chat's agent takes a few seconds to start. Its access levels are named meanwhile. Once
/// the agent's process has its setup, a change is refused rather than kept for later: kept, a
/// message sent before it applied could run on the access the person had just left.
#[tokio::test]
async fn a_new_chat_names_its_access_while_its_agent_starts_and_never_runs_a_stale_choice() {
    let (rig, seen, ready) = claude_rig(true);
    // An earlier chat taught the connection's levels.
    ready.add_permits(1);
    let first = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &first.id, SessionStatus::Ready).await;
    let folder = rig.folder.join("second");
    std::fs::create_dir_all(&folder).unwrap();
    let chat = rig.manager.create_chat("mock", &folder).await.unwrap();
    eventually("the second agent spawned", || lock(&seen.starts).len() == 2).await;
    let lists = rig.manager.lists(&chat.id).await.unwrap();
    assert_eq!(
        access_values(&lists),
        ["legacy:full_auto", "legacy:auto_edit", "legacy:ask"],
        "the levels the connection reported, not \"Access unavailable\""
    );
    assert_eq!(
        lists.controls.access.effective.as_deref(),
        Some("legacy:full_auto")
    );
    let refused = rig
        .manager
        .set_control(
            &chat.id,
            SessionControlChange::Access {
                value: "legacy:ask".into(),
            },
        )
        .await
        .expect_err("the agent already has its setup");
    assert!(refused.to_string().contains("is starting"), "{refused}");
    let sent = rig.manager.send(&chat.id, "first words".into()).await;
    assert!(sent.unwrap().queued);
    ready.add_permits(1);
    eventually("the prompt", || {
        lock(&seen.commands).iter().any(|c| c.starts_with("prompt"))
    })
    .await;
    assert_eq!(lock(&seen.starts)[1], (Autonomy::FullAuto, None));
    assert_eq!(
        lock(&seen.commands).as_slice(),
        ["prompt with bypassPermissions"],
        "the message ran on the access the picker showed"
    );
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let lists = rig.manager.lists(&chat.id).await.unwrap();
    assert_eq!(
        lists.controls.access.effective.as_deref(),
        Some("bypassPermissions")
    );
    assert_eq!(lists.autonomy.requested, Autonomy::FullAuto);
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

/// With nothing known about its agent yet, a starting chat offers no invented choices, and says
/// the level it starts on.
#[tokio::test]
async fn a_first_chat_offers_no_access_choices_until_its_agent_reports_them() {
    let (rig, seen, ready) = claude_rig(true);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    let lists = rig.manager.lists(&chat.id).await.unwrap();
    assert!(lists.controls.access.choices.is_empty());
    assert_eq!(
        lists.controls.access.requested.as_deref(),
        Some("legacy:full_auto"),
        "the level it starts on, for the picker to name"
    );
    assert_eq!(
        lists.controls.workflow.note, None,
        "only a running agent can say it has no Plan control"
    );
    ready.add_permits(1);
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let lists = rig.manager.lists(&chat.id).await.unwrap();
    assert_eq!(
        lists.controls.access.effective.as_deref(),
        Some("bypassPermissions")
    );
    // A level the picker showed before the agent's own lists arrived still applies, through the
    // agent's level.
    rig.manager
        .set_control(
            &chat.id,
            SessionControlChange::Access {
                value: "legacy:ask".into(),
            },
        )
        .await
        .expect("a level the agent supports");
    let lists = rig.manager.lists(&chat.id).await.unwrap();
    assert_eq!(lists.controls.access.effective.as_deref(), Some("default"));
    assert_eq!(lists.controls.access.pending, None);
    assert_eq!(lock(&seen.commands).as_slice(), ["level:default"]);
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}
