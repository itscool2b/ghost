//! Permission cards, questions, options and autonomy, through the session manager with scripted
//! drivers on paused time.

mod support;

use std::time::Duration;

use ghost_core::session::SessionError;
use ghost_drivers::driver::{DriverError, DriverEvent, DriverExit, LoggedCommand};
use ghost_drivers::testing::{CommandMatch, ScriptedDriver, Step};
use ghost_protocol::session::{
    ConfigCategory, ConfigChoice, ConfigKind, ConfigOption, ConfigValue,
};
use ghost_protocol::{
    AutoAnswer, Autonomy, AutonomyState, ErrorKind, GhostEvent, NoticeLevel, PermissionDecision,
    ResolvedBy, SessionStatus, StopReason, ToolKind, UiAnswer, UiPrompt, UiRequest,
};
use support::*;

#[tokio::test(start_paused = true)]
async fn grouped_questions_are_bounded_validated_and_never_auto_answered() {
    use ghost_protocol::{UiQuestion, UiQuestionAnswer, UiQuestionOption};

    let questions: Vec<_> = ["single", "multiple"]
        .into_iter()
        .map(|id| UiQuestion {
            id: id.into(),
            header: "header ".repeat(200),
            question: "question \"\\🦀 ".repeat(1000),
            options: (0..4)
                .map(|index| UiQuestionOption {
                    label: format!("{index}: {}", "choice ".repeat(100)),
                    description: "description \"\\🦀 ".repeat(1000),
                })
                .collect(),
            multi_select: id == "multiple",
            allow_free_text: id == "multiple",
        })
        .collect();
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Autonomy(full_auto())),
        ready_step("n1"),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    handle.emit(DriverEvent::UiRequest {
        turn: None,
        request: UiRequest {
            request_id: "questions".into(),
            prompt: UiPrompt::Questions {
                message: "Please choose".repeat(1000),
                questions,
            },
        },
    });
    sink.wait_for("grouped questions", |event| {
        matches!(event, GhostEvent::UiRequest(_))
    })
    .await;
    let history = h.stored(&chat.id).await;
    let request = history
        .iter()
        .find_map(|event| match event {
            GhostEvent::UiRequest(request) if request.request_id == "questions" => Some(request),
            _ => None,
        })
        .expect("grouped request");
    let UiPrompt::Questions { questions, .. } = &request.prompt else {
        panic!("questions");
    };
    assert_eq!(
        questions
            .iter()
            .map(|question| question.id.as_str())
            .collect::<Vec<_>>(),
        ["single", "multiple"]
    );
    for question in questions {
        for (index, option) in question.options.iter().enumerate() {
            assert!(
                option.label.starts_with(&format!("{index}:")),
                "option order stays exact"
            );
        }
    }
    assert!(
        serde_json::to_vec(&GhostEvent::UiRequest(request.clone()))
            .expect("JSON")
            .len()
            <= ghost_protocol::EVENT_BUDGET
    );
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::RespondUi { .. }))
    );
    let single = UiQuestionAnswer {
        question_id: "single".into(),
        selected: vec![1],
        text: None,
    };
    let multiple = UiQuestionAnswer {
        question_id: "multiple".into(),
        selected: vec![0, 3],
        text: Some("another choice \"\\🦀 ".repeat(1000)),
    };
    let mut invalid = vec![
        vec![],
        vec![single.clone()],
        vec![single.clone(), single.clone()],
    ];
    for changed in [
        UiQuestionAnswer {
            question_id: "unknown".into(),
            ..multiple.clone()
        },
        UiQuestionAnswer {
            selected: vec![4],
            ..multiple.clone()
        },
        UiQuestionAnswer {
            selected: vec![1, 1],
            ..multiple.clone()
        },
        UiQuestionAnswer {
            selected: vec![],
            text: None,
            ..multiple.clone()
        },
        UiQuestionAnswer {
            text: Some(" \t".into()),
            ..multiple.clone()
        },
    ] {
        invalid.push(vec![single.clone(), changed]);
    }
    for changed in [
        UiQuestionAnswer {
            selected: vec![0, 1],
            ..single.clone()
        },
        UiQuestionAnswer {
            selected: vec![],
            text: Some("not allowed".into()),
            ..single.clone()
        },
    ] {
        invalid.push(vec![changed, multiple.clone()]);
    }
    for answers in invalid {
        assert!(matches!(
            h.manager
                .respond_ui(&chat.id, "questions", Some(UiAnswer::Questions { answers }))
                .await,
            Err(SessionError::OptionInvalid(_))
        ));
    }
    let answer = UiAnswer::Questions {
        answers: vec![multiple, single],
    };
    h.manager
        .respond_ui(&chat.id, "questions", Some(answer.clone()))
        .await
        .expect("valid grouped answer");
    assert_eq!(
        command(
            &handle,
            CommandMatch::RespondUi {
                request_id: Some("questions".into())
            }
        )
        .await,
        LoggedCommand::RespondUi {
            request_id: "questions".into(),
            answer: Some(answer)
        }
    );
    for event in h.stored(&chat.id).await {
        if matches!(event, GhostEvent::UiResolved { .. }) {
            assert!(
                serde_json::to_vec(&event).expect("JSON").len() <= ghost_protocol::EVENT_BUDGET
            );
        }
    }
}

fn end_turn(stop_reason: StopReason) -> Step {
    Step::EndTurn {
        stop_reason,
        usage: None,
        error: None,
    }
}

fn card(id: &str, kind: Option<ToolKind>) -> Step {
    Step::EmitInTurn(DriverEvent::Permission {
        turn: None,
        request: permission(id, kind),
    })
}

fn resolution(events: &[(u32, GhostEvent)], id: &str) -> Vec<(PermissionDecision, ResolvedBy)> {
    events
        .iter()
        .filter_map(|(_, event)| match event {
            GhostEvent::PermissionResolved {
                request_id,
                decision,
                by,
            } if request_id == id => Some((decision.clone(), *by)),
            _ => None,
        })
        .collect()
}

fn full_auto() -> AutonomyState {
    AutonomyState {
        requested: Autonomy::FullAuto,
        native_mode: Some("full_auto".into()),
        auto_answer: AutoAnswer::All,
        note: None,
    }
}

/// A9: an answer that comes after the agent withdrew the request is not pending, and nothing
/// reaches the agent.
#[tokio::test(start_paused = true)]
async fn late_answer_after_withdrawal() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        card("p1", Some(ToolKind::Edit)),
        Step::Wait(Duration::from_secs(1)),
        Step::Emit(DriverEvent::PermissionWithdrawn {
            request_id: "p1".into(),
        }),
        Step::Hang,
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "edit".into()).await.expect("send");
    sink.wait_for("the withdrawal", |event| {
        matches!(
            event,
            GhostEvent::PermissionResolved {
                by: ResolvedBy::Agent,
                ..
            }
        )
    })
    .await;
    assert!(matches!(
        h.manager
            .respond_permission(&chat.id, "p1", Some("once".into()))
            .await,
        Err(SessionError::RequestNotPending)
    ));
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::RespondPermission { .. }))
    );
}

/// S38: the user's answer crossing the agent's withdrawal gives one resolution (the user's) and
/// an info notice.
#[tokio::test(start_paused = true)]
async fn answer_crossing_withdrawal() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        card("p1", Some(ToolKind::Edit)),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("p1".into()),
        }),
        Step::Emit(DriverEvent::PermissionWithdrawn {
            request_id: "p1".into(),
        }),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "edit".into()).await.expect("send");
    sink.wait_for("the card", |event| {
        matches!(event, GhostEvent::PermissionRequested(_))
    })
    .await;
    h.manager
        .respond_permission(&chat.id, "p1", Some("once".into()))
        .await
        .expect("answer");
    sink.wait_for("the turn ends", |event| {
        matches!(event, GhostEvent::TurnEnded { .. })
    })
    .await;
    let events = sink.events();
    assert_eq!(
        resolution(&events, "p1"),
        [(
            PermissionDecision::Selected {
                option_id: "once".into(),
                plan: None,
            },
            ResolvedBy::User
        )]
    );
    assert!(events.iter().any(|(_, event)| matches!(
        event,
        GhostEvent::Notice { level: NoticeLevel::Info, message } if message == "The agent withdrew \"May I p1?\" before your answer reached it."
    )));
}

/// A33: two requests at once are two independent cards: answering one leaves the other open.
#[tokio::test(start_paused = true)]
async fn concurrent_permissions() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        card("p1", Some(ToolKind::Edit)),
        card("p2", Some(ToolKind::Execute)),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("p2".into()),
        }),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("p1".into()),
        }),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "go".into()).await.expect("send");
    let cards = sink.clone();
    eventually("both cards show", || {
        cards
            .events()
            .iter()
            .filter(|(_, event)| matches!(event, GhostEvent::PermissionRequested(_)))
            .count()
            == 2
    })
    .await;
    h.manager
        .respond_permission(&chat.id, "p2", Some("no".into()))
        .await
        .expect("answer p2");
    assert!(
        resolution(&sink.events(), "p1").is_empty(),
        "p1 is still open"
    );
    assert!(matches!(
        h.manager
            .respond_permission(&chat.id, "p1", Some("bogus".into()))
            .await,
        Err(SessionError::OptionInvalid(_))
    ));
    h.manager
        .respond_permission(&chat.id, "p1", None)
        .await
        .expect("cancel p1");
    sink.wait_for("the turn ends", |event| {
        matches!(event, GhostEvent::TurnEnded { .. })
    })
    .await;
    assert_eq!(
        handle
            .commands()
            .iter()
            .filter_map(|command| match command {
                LoggedCommand::RespondPermission {
                    request_id,
                    option_id,
                } => Some((request_id.clone(), option_id.clone())),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [
            ("p2".to_owned(), Some("no".to_owned())),
            ("p1".to_owned(), None)
        ]
    );
    let events = sink.events();
    assert_eq!(
        resolution(&events, "p2"),
        [(
            PermissionDecision::Selected {
                option_id: "no".into(),
                plan: None,
            },
            ResolvedBy::User
        )]
    );
    assert_eq!(
        resolution(&events, "p1"),
        [(PermissionDecision::Cancelled, ResolvedBy::User)]
    );
}

/// S11: two views get identical frames, and the first answer to a card wins.
#[tokio::test(start_paused = true)]
async fn two_subscribers() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        in_turn(message("m1", "Shall I?")),
        card("p1", Some(ToolKind::Edit)),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("p1".into()),
        }),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let first = Sink::new("window-1");
    let second = Sink::new("window-2");
    h.manager
        .attach(&chat.id, first.clone())
        .await
        .expect("attach");
    let b = h
        .manager
        .attach(&chat.id, second.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "go".into()).await.expect("send");
    second
        .wait_for("the card", |event| {
            matches!(event, GhostEvent::PermissionRequested(_))
        })
        .await;
    h.manager
        .respond_permission(&chat.id, "p1", Some("once".into()))
        .await
        .expect("the first answer");
    assert!(
        matches!(
            h.manager
                .respond_permission(&chat.id, "p1", Some("no".into()))
                .await,
            Err(SessionError::RequestNotPending)
        ),
        "the second view's answer comes too late"
    );
    first
        .wait_for("the turn ends", |event| {
            matches!(event, GhostEvent::TurnEnded { .. })
        })
        .await;
    second
        .wait_for("the turn ends", |event| {
            matches!(event, GhostEvent::TurnEnded { .. })
        })
        .await;
    let from_both: Vec<(u32, GhostEvent)> = first
        .events()
        .into_iter()
        .filter(|(seq, _)| *seq >= b.next_seq)
        .collect();
    assert_eq!(
        from_both,
        second.events(),
        "identical frames once both are attached"
    );
    assert_eq!(
        resolution(&second.events(), "p1"),
        [(
            PermissionDecision::Selected {
                option_id: "once".into(),
                plan: None,
            },
            ResolvedBy::User
        )]
    );
}

/// A question that only notifies is never open, so nothing answers it; a stopped chat's autonomy
/// change that the agent refused asks before every action.
#[tokio::test(start_paused = true)]
async fn notifications_are_never_open_and_a_refused_autonomy_asks() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::SetAutonomy),
        Step::Hang,
    ]);
    handle.set_autonomy_result(Err(DriverError {
        kind: ErrorKind::AgentError,
        message: "mode not found".into(),
        detail: None,
    }));
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    // The sink only receives live events after attach. Do not race the initial history snapshot.
    handle.emit(DriverEvent::UiRequest {
        turn: None,
        request: UiRequest {
            request_id: "n".into(),
            prompt: UiPrompt::Notify {
                message: "FYI".into(),
            },
        },
    });
    sink.wait_for("the notification", |event| {
        matches!(event, GhostEvent::UiRequest(_))
    })
    .await;
    assert!(matches!(
        h.manager.respond_ui(&chat.id, "n", None).await,
        Err(SessionError::RequestNotPending)
    ));
    assert!(
        !h.statuses
            .last(&chat.id)
            .is_some_and(|update| update.needs_attention)
    );
    let lists = h
        .manager
        .set_autonomy(&chat.id, Autonomy::FullAuto)
        .await
        .expect("autonomy");
    assert_eq!(lists.autonomy.auto_answer, AutoAnswer::Never);
    assert!(
        lists
            .autonomy
            .note
            .as_deref()
            .is_some_and(|note| note.contains("mode not found")),
        "{:?}",
        lists.autonomy.note
    );
}

/// S31: a card waiting while nobody views the chat sets `needs_attention` on the status channel.
#[tokio::test(start_paused = true)]
async fn needs_attention_on_permission() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        card("p1", Some(ToolKind::Delete)),
        Step::Expect(CommandMatch::RespondPermission { request_id: None }),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager
        .send(&chat.id, "clean up".into())
        .await
        .expect("send");
    let statuses = h.statuses.clone();
    let id = chat.id.clone();
    eventually("the chat needs attention", || {
        statuses
            .last(&id)
            .is_some_and(|update| update.needs_attention)
    })
    .await;
    let listed = h.manager.chats().await.expect("chats");
    assert!(listed[0].needs_attention);
    h.manager
        .respond_permission(&chat.id, "p1", Some("once".into()))
        .await
        .expect("answer");
    let statuses = h.statuses.clone();
    eventually("attention is cleared", || {
        statuses
            .last(&id)
            .is_some_and(|update| !update.needs_attention)
    })
    .await;
}

/// A36: a request before `Ready` is forwarded untagged, shows while `Starting`, and the answer
/// reaches the agent.
#[tokio::test(start_paused = true)]
async fn card_before_ready() {
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Permission {
            turn: None,
            request: permission("guarded-model", Some(ToolKind::Other)),
        }),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("guarded-model".into()),
        }),
        ready_step("n1"),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    let attach = h
        .manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    let is_card = |event: &GhostEvent| matches!(event, GhostEvent::PermissionRequested(request) if request.request_id == "guarded-model");
    // The card is in the attach's page when it reached the store first, else it arrives live.
    if !attach.history.items.iter().any(|item| is_card(&item.event)) {
        sink.wait_for("the card", is_card).await;
    }
    assert_eq!(attach.snapshot.session.status, SessionStatus::Starting);
    assert_eq!(
        h.statuses.last(&chat.id).map(|update| update.status),
        Some(SessionStatus::Starting)
    );
    h.manager
        .respond_permission(&chat.id, "guarded-model", Some("once".into()))
        .await
        .expect("answer");
    h.status(&chat.id, SessionStatus::Ready).await;
    assert!(
        handle
            .commands()
            .contains(&LoggedCommand::RespondPermission {
                request_id: "guarded-model".into(),
                option_id: Some("once".into()),
            })
    );
}

/// Native policy reports update covered open cards; user access changes remain idle-only.
#[tokio::test(start_paused = true)]
async fn native_autonomy_report_answers_open_cards() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        card("edit", Some(ToolKind::Edit)),
        card("question", Some(ToolKind::Other)),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("edit".into()),
        }),
        Step::Hang,
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "go".into()).await.expect("send");
    let cards = sink.clone();
    eventually("both cards show", || {
        cards
            .events()
            .iter()
            .filter(|(_, event)| matches!(event, GhostEvent::PermissionRequested(_)))
            .count()
            == 2
    })
    .await;
    assert!(matches!(
        h.manager.set_autonomy(&chat.id, Autonomy::FullAuto).await,
        Err(SessionError::OptionInvalid(_))
    ));
    handle.emit(DriverEvent::Autonomy(full_auto()));
    command(
        &handle,
        CommandMatch::RespondPermission {
            request_id: Some("edit".into()),
        },
    )
    .await;
    sink.wait_for("the auto answer", |event| {
        matches!(
            event,
            GhostEvent::PermissionResolved {
                by: ResolvedBy::Auto,
                ..
            }
        )
    })
    .await;
    let events = sink.events();
    assert_eq!(
        resolution(&events, "edit"),
        [(
            PermissionDecision::Selected {
                option_id: "once".into(),
                plan: None,
            },
            ResolvedBy::Auto
        )]
    );
    assert!(
        resolution(&events, "question").is_empty(),
        "questions are never auto-answered"
    );
}

/// Native policy changes recompute pending decisions; kind other still requires the user.
#[tokio::test(start_paused = true)]
async fn native_autonomy_report_recomputes_pending_decisions() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        card("asked", Some(ToolKind::Execute)),
        card("auto", Some(ToolKind::Execute)),
        card("other", Some(ToolKind::Other)),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("asked".into()),
        }),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("auto".into()),
        }),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "go".into()).await.expect("send");
    sink.wait_for("the first card", |event| {
        matches!(event, GhostEvent::PermissionRequested(_))
    })
    .await;
    assert!(matches!(
        h.manager.set_autonomy(&chat.id, Autonomy::FullAuto).await,
        Err(SessionError::OptionInvalid(_))
    ));
    handle.emit(DriverEvent::Autonomy(full_auto()));
    let probe = sink.clone();
    eventually("the third card shows", || {
        probe
            .events()
            .iter()
            .filter(|(_, event)| matches!(event, GhostEvent::PermissionRequested(_)))
            .count()
            == 3
    })
    .await;
    command(
        &handle,
        CommandMatch::RespondPermission {
            request_id: Some("auto".into()),
        },
    )
    .await;
    sink.wait_for("the native policy auto answer", |event| matches!(event,GhostEvent::PermissionResolved {request_id,by:ResolvedBy::Auto,..} if request_id == "auto")).await;
    let events = sink.events();
    assert_eq!(
        resolution(&events, "auto"),
        [(
            PermissionDecision::Selected {
                option_id: "once".into(),
                plan: None,
            },
            ResolvedBy::Auto
        )]
    );
    assert_eq!(
        resolution(&events, "asked"),
        [(
            PermissionDecision::Selected {
                option_id: "once".into(),
                plan: None,
            },
            ResolvedBy::Auto
        )],
        "answered when the state changed"
    );
    assert!(resolution(&events, "other").is_empty());
    assert!(
        handle
            .commands()
            .contains(&LoggedCommand::RespondPermission {
                request_id: "auto".into(),
                option_id: Some("once".into()),
            })
    );
}

/// The agent's own autonomy report (a mode picked by hand turns auto-answers off) applies too.
#[tokio::test(start_paused = true)]
async fn an_autonomy_report_from_the_agent_applies() {
    let (driver, _) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Autonomy(full_auto())),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        card("auto", Some(ToolKind::Read)),
        Step::Emit(DriverEvent::Autonomy(AutonomyState {
            requested: Autonomy::FullAuto,
            native_mode: Some("ask".into()),
            auto_answer: AutoAnswer::Never,
            note: Some(
                "You picked the Ask mode by hand, so Ghost asks before every action.".into(),
            ),
        })),
        card("asked", Some(ToolKind::Read)),
        Step::Hang,
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "read".into()).await.expect("send");
    let probe = sink.clone();
    eventually("both cards show", || {
        probe
            .events()
            .iter()
            .filter(|(_, event)| matches!(event, GhostEvent::PermissionRequested(_)))
            .count()
            == 2
    })
    .await;
    let events = sink.events();
    assert_eq!(resolution(&events, "auto").len(), 1);
    assert!(resolution(&events, "asked").is_empty());
    let lists = h.manager.lists(&chat.id).await.expect("lists");
    assert_eq!(lists.autonomy.native_mode.as_deref(), Some("ask"));
}

/// 7.1: autonomy set while stopped is stored and applied at the next start.
#[tokio::test(start_paused = true)]
async fn autonomy_while_stopped_applies_at_the_next_start() {
    let (first, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let (second, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = Harness::new(vec![first, second]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    let lists = h
        .manager
        .set_autonomy(&chat.id, Autonomy::Ask)
        .await
        .expect("autonomy");
    assert_eq!(lists.autonomy.requested, Autonomy::Ask);
    assert_eq!(
        lists.autonomy.note.as_deref(),
        Some("Applies when the agent starts.")
    );
    assert_eq!(lists.autonomy.auto_answer, AutoAnswer::Never);
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Ready).await;
    assert_eq!(h.factory.setups()[1].autonomy, Autonomy::Ask);
}

fn model_option() -> ConfigOption {
    ConfigOption {
        id: "model".into(),
        name: "Model".into(),
        description: None,
        category: ConfigCategory::Model,
        kind: ConfigKind::Select {
            current: "fast".into(),
            choices: vec![
                ConfigChoice {
                    value: "fast".into(),
                    name: "Fast".into(),
                    description: None,
                    group: None,
                },
                ConfigChoice {
                    value: "smart".into(),
                    name: "Smart".into(),
                    description: None,
                    group: None,
                },
            ],
        },
        pending: None,
    }
}

/// The current value of the option `id` in `lists`.
fn shown(lists: &ghost_protocol::session::SessionLists, id: &str) -> Option<String> {
    lists.options.iter().find_map(|option| match &option.kind {
        ConfigKind::Select { current, .. } if option.id == id => Some(current.clone()),
        _ => None,
    })
}

fn select(id: &str, category: ConfigCategory, current: &str, values: &[&str]) -> ConfigOption {
    ConfigOption {
        id: id.into(),
        name: id.into(),
        description: None,
        category,
        kind: ConfigKind::Select {
            current: current.into(),
            choices: values
                .iter()
                .map(|value| ConfigChoice {
                    value: (*value).into(),
                    name: (*value).into(),
                    description: None,
                    group: None,
                })
                .collect(),
        },
        pending: None,
    }
}

fn choice(value: &str) -> ConfigValue {
    ConfigValue::Choice {
        value: value.into(),
    }
}

/// S18: an option change during a turn is sent at once, and its answer shows the choice although
/// the driver's own lists have not arrived; the choice is remembered for the next start; invalid
/// values and an agent's refusal read as `OptionInvalid`; a stopped chat keeps a choice for its
/// next start.
// Memory preparation runs on the profile writer thread before the native Prompt command.
#[tokio::test]
async fn option_change_during_turn() {
    let (first, handle) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Options(vec![model_option()])),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::SetOption),
        Step::Expect(CommandMatch::SetOption),
        end_turn(StopReason::EndTurn),
    ]);
    let (second, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = Harness::new(vec![first, second]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let lists = h.manager.lists(&chat.id).await.expect("lists");
    assert_eq!(lists.options, [model_option()]);
    let attach = h
        .manager
        .attach(&chat.id, Sink::new("main"))
        .await
        .expect("attach");
    assert_eq!(attach.snapshot.session.model.as_deref(), Some("Fast"));
    h.manager.send(&chat.id, "work".into()).await.expect("send");
    command(&handle, CommandMatch::Prompt).await;
    h.status(&chat.id, SessionStatus::Running).await;
    let smart = ConfigValue::Choice {
        value: "smart".into(),
    };
    // The scripted driver answers without sending lists, as every driver answers before its lists
    // arrive: the answer itself shows the new model, so the picker never snaps back.
    let lists = h
        .manager
        .set_option(&chat.id, "model", smart.clone())
        .await
        .expect("set during the turn");
    assert_eq!(shown(&lists, "model").as_deref(), Some("smart"));
    assert!(lists.revision > attach.snapshot.session.revision);
    assert_eq!(
        shown(&h.manager.lists(&chat.id).await.expect("lists"), "model").as_deref(),
        Some("smart")
    );
    assert!(matches!(
        h.manager
            .set_option(
                &chat.id,
                "model",
                ConfigValue::Choice {
                    value: "huge".into()
                }
            )
            .await,
        Err(SessionError::OptionInvalid(_))
    ));
    assert!(matches!(
        h.manager.set_option(&chat.id, "nope", smart.clone()).await,
        Err(SessionError::OptionInvalid(_))
    ));
    handle.set_option_reply(Err(DriverError {
        kind: ErrorKind::AgentError,
        message: "That model isn't available on your plan.".into(),
        detail: None,
    }));
    assert!(matches!(
        h.manager.set_option(&chat.id, "model", ConfigValue::Choice { value: "fast".into() }).await,
        Err(SessionError::OptionInvalid(message)) if message == "That model isn't available on your plan."
    ));
    assert_eq!(
        shown(&h.manager.lists(&chat.id).await.expect("lists"), "model").as_deref(),
        Some("smart"),
        "a refused change shows nothing new"
    );
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    let stopped = h.manager.lists(&chat.id).await.expect("lists");
    assert_eq!(
        shown(&stopped, "model").as_deref(),
        Some("smart"),
        "a stopped chat shows what its next start asks for"
    );
    let fast = choice("fast");
    let lists = h
        .manager
        .set_option(&chat.id, "model", fast.clone())
        .await
        .expect("kept for the next start");
    assert_eq!(shown(&lists, "model").as_deref(), Some("fast"));
    assert!(lists.revision > stopped.revision);
    assert!(matches!(
        h.manager
            .set_option(&chat.id, "model", choice("huge"))
            .await,
        Err(SessionError::OptionInvalid(_))
    ));
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Ready).await;
    assert_eq!(
        h.factory.setups()[1].desired.get("model"),
        Some(&fast),
        "the stopped chat's choice applies at the next start"
    );
}

/// A chat whose agent is not running offers its connection's models and each model's thinking
/// levels, with what its next start asks for; a level the newly chosen model does not offer is
/// forgotten, one its catalog does not know waits for the agent, and mode changes wait for an idle
/// agent. At Ready, a saved level the agent does not offer is forgotten.
#[tokio::test]
async fn a_stopped_chat_keeps_model_and_thinking_choices_for_its_next_start() {
    let models = || {
        select(
            "model",
            ConfigCategory::Model,
            "fast",
            &["fast", "smart", "tiny"],
        )
    };
    let mode = select("mode", ConfigCategory::Mode, "ask", &["ask", "auto"]);
    let (first, _) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Options(vec![
            models(),
            select("effort", ConfigCategory::Effort, "low", &["low", "high"]),
            mode.clone(),
        ])),
        ready_step("n1"),
    ]);
    // The second start runs `smart`, which offers no `high`.
    let mut smart = models();
    if let ConfigKind::Select { current, .. } = &mut smart.kind {
        *current = "smart".into();
    }
    let (second, _) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Options(vec![
            smart,
            select("effort", ConfigCategory::Effort, "low", &["low", "medium"]),
        ])),
        ready_step("n1"),
    ]);
    let (third, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = Harness::new(vec![first, second, third]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    let lists = h.manager.lists(&chat.id).await.expect("lists");
    assert_eq!(shown(&lists, "model").as_deref(), Some("fast"));
    assert_eq!(
        shown(&lists, "effort").as_deref(),
        Some("low"),
        "the level the session ran with"
    );
    let lists = h
        .manager
        .set_option(&chat.id, "effort", choice("high"))
        .await
        .expect("level");
    assert_eq!(shown(&lists, "effort").as_deref(), Some("high"));
    assert!(matches!(
        h.manager
            .set_option(&chat.id, "effort", choice("max"))
            .await,
        Err(SessionError::OptionInvalid(_))
    ));
    assert!(matches!(
        h.manager.set_option(&chat.id, "mode", choice("auto")).await,
        Err(SessionError::OptionInvalid(message)) if message.contains("running and idle")
    ));
    // `smart` never ran: its levels are unknown, so the chosen level waits for the agent.
    let lists = h
        .manager
        .set_option(&chat.id, "model", choice("smart"))
        .await
        .expect("model");
    assert_eq!(shown(&lists, "model").as_deref(), Some("smart"));
    assert_eq!(shown(&lists, "effort"), None);
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Ready).await;
    let setup = &h.factory.setups()[1];
    assert_eq!(setup.desired.get("model"), Some(&choice("smart")));
    assert_eq!(setup.desired.get("effort"), Some(&choice("high")));
    // `smart` offers no `high`: the next start no longer asks for it.
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Ready).await;
    let setup = &h.factory.setups()[2];
    assert_eq!(setup.desired.get("model"), Some(&choice("smart")));
    assert_eq!(setup.desired.get("effort"), None);
}

/// A known model without a level forgets the level chosen for another one.
#[tokio::test]
async fn choosing_a_model_without_levels_forgets_the_level() {
    let (first, _) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Options(vec![select(
            "model",
            ConfigCategory::Model,
            "tiny",
            &["fast", "tiny"],
        )])),
        Step::Emit(DriverEvent::Options(vec![
            select("model", ConfigCategory::Model, "fast", &["fast", "tiny"]),
            select("effort", ConfigCategory::Effort, "low", &["low", "high"]),
        ])),
        ready_step("n1"),
    ]);
    let (second, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = Harness::new(vec![first, second]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    h.manager
        .set_option(&chat.id, "effort", choice("high"))
        .await
        .expect("level");
    let lists = h
        .manager
        .set_option(&chat.id, "model", choice("tiny"))
        .await
        .expect("model");
    assert_eq!(shown(&lists, "effort"), None, "tiny offers no level");
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Ready).await;
    assert_eq!(h.factory.setups()[1].desired.get("effort"), None);
}

/// The catalog survives a restart: a reopened chat offers its models and thinking levels before
/// its agent starts, and a choice made then applies at the start.
#[tokio::test]
async fn the_model_catalog_survives_a_restart() {
    let (first, _) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Options(vec![
            select("model", ConfigCategory::Model, "fast", &["fast", "smart"]),
            select("effort", ConfigCategory::Effort, "low", &["low", "high"]),
        ])),
        ready_step("n1"),
    ]);
    let h = Harness::new(vec![first]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let (second, _) = ScriptedDriver::new(vec![ready_step("n1")]);
    let h = h.restart(vec![second], Options::default()).await;
    let mut lists = h.manager.lists(&chat.id).await.expect("lists");
    for _ in 0..200 {
        if !lists.options.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        lists = h.manager.lists(&chat.id).await.expect("lists");
    }
    assert_eq!(shown(&lists, "model").as_deref(), Some("fast"));
    assert_eq!(shown(&lists, "effort").as_deref(), Some("low"));
    let lists = h
        .manager
        .set_option(&chat.id, "effort", choice("high"))
        .await
        .expect("level before the start");
    assert_eq!(shown(&lists, "effort").as_deref(), Some("high"));
    h.manager.start(&chat.id, false).await.expect("start");
    h.status(&chat.id, SessionStatus::Ready).await;
    assert_eq!(
        h.factory.setups()[0].desired.get("effort"),
        Some(&choice("high"))
    );
}

fn color_question() -> UiRequest {
    UiRequest {
        request_id: "q1".into(),
        prompt: UiPrompt::Select {
            message: "Which color?".into(),
            options: vec!["Red".into(), "Green".into(), "Blue".into()],
        },
    }
}

/// A41: the question round trip: answers that do not fit are refused, an answer reaches the
/// agent and resolves the card `User`; a dismissal; a withdrawal; a question open when the agent
/// stops is closed `System`.
#[tokio::test(start_paused = true)]
async fn question_round_trip() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        Step::EmitInTurn(DriverEvent::UiRequest {
            turn: None,
            request: color_question(),
        }),
        Step::Expect(CommandMatch::RespondUi {
            request_id: Some("q1".into()),
        }),
        Step::EmitInTurn(DriverEvent::UiRequest {
            turn: None,
            request: UiRequest {
                request_id: "q2".into(),
                prompt: UiPrompt::Confirm {
                    message: "Sure?".into(),
                },
            },
        }),
        Step::Expect(CommandMatch::RespondUi {
            request_id: Some("q2".into()),
        }),
        Step::EmitInTurn(DriverEvent::UiRequest {
            turn: None,
            request: UiRequest {
                request_id: "q3".into(),
                prompt: UiPrompt::Input {
                    message: "Name?".into(),
                    placeholder: None,
                },
            },
        }),
        Step::Emit(DriverEvent::UiWithdrawn {
            request_id: "q3".into(),
        }),
        end_turn(StopReason::EndTurn),
        Step::Emit(DriverEvent::UiRequest {
            turn: None,
            request: UiRequest {
                request_id: "q4".into(),
                prompt: UiPrompt::Confirm {
                    message: "Later?".into(),
                },
            },
        }),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager.send(&chat.id, "/ask".into()).await.expect("send");
    sink.wait_for(
        "the question",
        |event| matches!(event, GhostEvent::UiRequest(request) if request.request_id == "q1"),
    )
    .await;
    for wrong in [
        UiAnswer::Select { index: 3 },
        UiAnswer::Confirm { yes: true },
    ] {
        assert!(matches!(
            h.manager.respond_ui(&chat.id, "q1", Some(wrong)).await,
            Err(SessionError::OptionInvalid(_))
        ));
    }
    h.manager
        .respond_ui(&chat.id, "q1", Some(UiAnswer::Select { index: 1 }))
        .await
        .expect("answer");
    assert!(matches!(
        h.manager.respond_ui(&chat.id, "q1", None).await,
        Err(SessionError::RequestNotPending)
    ));
    sink.wait_for(
        "the second question",
        |event| matches!(event, GhostEvent::UiRequest(request) if request.request_id == "q2"),
    )
    .await;
    h.manager
        .respond_ui(&chat.id, "q2", None)
        .await
        .expect("dismiss");
    sink.wait_for(
        "the late question",
        |event| matches!(event, GhostEvent::UiRequest(request) if request.request_id == "q4"),
    )
    .await;
    h.manager.stop(&chat.id).await.expect("stop");
    h.status(&chat.id, SessionStatus::Stopped).await;
    let resolutions: Vec<(String, Option<UiAnswer>, ResolvedBy)> = h
        .stored(&chat.id)
        .await
        .into_iter()
        .filter_map(|event| match event {
            GhostEvent::UiResolved {
                request_id,
                answer,
                by,
            } => Some((request_id, answer, by)),
            _ => None,
        })
        .collect();
    assert_eq!(
        resolutions,
        [
            (
                "q1".into(),
                Some(UiAnswer::Select { index: 1 }),
                ResolvedBy::User
            ),
            ("q2".into(), None, ResolvedBy::User),
            ("q3".into(), None, ResolvedBy::Agent),
            ("q4".into(), None, ResolvedBy::System),
        ]
    );
    assert!(handle.commands().contains(&LoggedCommand::RespondUi {
        request_id: "q1".into(),
        answer: Some(UiAnswer::Select { index: 1 }),
    }));
}

/// A26 through the actor: under Full auto an `execute` request is answered and recorded as
/// requested and resolved `Auto`; `allow_always` is never chosen.
// Memory snapshot preparation uses the profile writer thread; keep this acknowledgement deadline on real time.
#[tokio::test]
async fn full_auto_records_auto_answers() {
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Autonomy(full_auto())),
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        card("run", Some(ToolKind::Execute)),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("run".into()),
        }),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager
        .send(&chat.id, "run the tests".into())
        .await
        .expect("send");
    command(
        &handle,
        CommandMatch::RespondPermission {
            request_id: Some("run".into()),
        },
    )
    .await;
    h.status(&chat.id, SessionStatus::Ready).await;
    let stored = h.stored(&chat.id).await;
    let request = stored
        .iter()
        .position(|event| matches!(event, GhostEvent::PermissionRequested(_)))
        .expect("requested");
    assert_eq!(
        stored[request + 1],
        GhostEvent::PermissionResolved {
            request_id: "run".into(),
            decision: PermissionDecision::Selected {
                option_id: "once".into(),
                plan: None,
            },
            by: ResolvedBy::Auto,
        }
    );
}

/// A plan waiting for approval, as Claude Code's ExitPlanMode asks for it.
fn plan_card(id: &str, plan: &str) -> Step {
    let mut request = permission(id, Some(ToolKind::SwitchMode));
    request.raw_input = Some(serde_json::json!({ "plan": plan }));
    Step::EmitInTurn(DriverEvent::Permission {
        turn: None,
        request,
    })
}

fn edits(commands: &[LoggedCommand]) -> usize {
    commands
        .iter()
        .filter(|command| matches!(command, LoggedCommand::EditPlan { .. }))
        .count()
}

/// A plan accepted with edits: the edited plan reaches the agent before the answer does, and the
/// record of the answer keeps it. Every other use of an edited plan is refused, and a refusal
/// sends the agent nothing.
#[tokio::test(start_paused = true)]
async fn a_plan_accepted_with_edits_reaches_the_agent_before_the_answer() {
    let mut ready = ready_info("n1");
    ready.capabilities.plan_edits = true;
    let (driver, handle) = ScriptedDriver::new(vec![
        Step::Emit(DriverEvent::Ready(ready)),
        Step::Expect(CommandMatch::Prompt),
        card("edit", Some(ToolKind::Edit)),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("edit".into()),
        }),
        plan_card("plan", "1. Build"),
        Step::Expect(CommandMatch::Any),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("plan".into()),
        }),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager
        .send(&chat.id, "plan it".into())
        .await
        .expect("send");
    sink.wait_for("the first card", |event| {
        matches!(event, GhostEvent::PermissionRequested(request) if request.request_id == "edit")
    })
    .await;
    let edited = "1. Build\n2. Test";
    // An ordinary request is not a plan: it takes no edits.
    let refused = h
        .manager
        .accept_plan(&chat.id, "edit", "once", edited)
        .await;
    assert!(
        matches!(&refused, Err(SessionError::OptionInvalid(why)) if why.contains("Only a plan")),
        "{refused:?}"
    );
    h.manager
        .respond_permission(&chat.id, "edit", Some("once".into()))
        .await
        .expect("answer");
    sink.wait_for("the plan", |event| {
        matches!(event, GhostEvent::PermissionRequested(request) if request.request_id == "plan")
    })
    .await;
    let long = "x".repeat(64 * 1024 + 1);
    for (option, plan, why) in [
        ("no", edited, "only be accepted"),
        ("always", edited, "only be accepted"),
        ("missing", edited, "isn't one of this request's options"),
        ("once", "  \n ", "empty"),
        ("once", long.as_str(), "too long"),
        ("once", "1. Build\0", "cannot be sent"),
    ] {
        let refused = h.manager.accept_plan(&chat.id, "plan", option, plan).await;
        assert!(
            matches!(&refused, Err(SessionError::OptionInvalid(text)) if text.contains(why)),
            "{option}: {refused:?}"
        );
    }
    assert_eq!(edits(&handle.commands()), 0, "a refusal sends nothing");
    assert!(resolution(&sink.events(), "plan").is_empty());
    h.manager
        .accept_plan(&chat.id, "plan", "once", edited)
        .await
        .expect("accept with edits");
    sink.wait_for("the turn ends", |event| {
        matches!(event, GhostEvent::TurnEnded { .. })
    })
    .await;
    let commands = handle.commands();
    let edit = commands
        .iter()
        .position(|command| {
            matches!(command, LoggedCommand::EditPlan { request_id, plan }
                if request_id == "plan" && plan == edited)
        })
        .expect("the edited plan was sent");
    let answer = commands
        .iter()
        .position(|command| {
            matches!(command, LoggedCommand::RespondPermission { request_id, option_id }
                if request_id == "plan" && option_id.as_deref() == Some("once"))
        })
        .expect("the answer was sent");
    assert!(edit < answer, "the plan goes first");
    assert_eq!(edits(&commands), 1);
    assert_eq!(
        resolution(&sink.events(), "plan"),
        [(
            PermissionDecision::Selected {
                option_id: "once".into(),
                plan: Some(edited.into()),
            },
            ResolvedBy::User
        )]
    );
}

/// An agent that does not say it takes edited plans is never sent one: the edit is refused, the
/// plan stays waiting, and a plain answer still goes through.
#[tokio::test(start_paused = true)]
async fn an_agent_that_takes_no_edited_plans_is_never_sent_one() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("n1"),
        Step::Expect(CommandMatch::Prompt),
        plan_card("plan", "1. Build"),
        Step::Expect(CommandMatch::RespondPermission {
            request_id: Some("plan".into()),
        }),
        end_turn(StopReason::EndTurn),
    ]);
    let h = Harness::new(vec![driver]);
    let chat = h.chat().await;
    let sink = Sink::new("main");
    h.manager
        .attach(&chat.id, sink.clone())
        .await
        .expect("attach");
    h.status(&chat.id, SessionStatus::Ready).await;
    h.manager
        .send(&chat.id, "plan it".into())
        .await
        .expect("send");
    sink.wait_for("the plan", |event| {
        matches!(event, GhostEvent::PermissionRequested(_))
    })
    .await;
    let refused = h
        .manager
        .accept_plan(&chat.id, "plan", "once", "1. Build\n2. Test")
        .await;
    assert!(
        matches!(&refused, Err(SessionError::OptionInvalid(why)) if why.contains("cannot take an edited plan")),
        "{refused:?}"
    );
    assert!(
        resolution(&sink.events(), "plan").is_empty(),
        "still waiting"
    );
    h.manager
        .respond_permission(&chat.id, "plan", Some("once".into()))
        .await
        .expect("a plain answer");
    sink.wait_for("the turn ends", |event| {
        matches!(event, GhostEvent::TurnEnded { .. })
    })
    .await;
    assert_eq!(edits(&handle.commands()), 0);
    assert_eq!(
        resolution(&sink.events(), "plan"),
        [(
            PermissionDecision::Selected {
                option_id: "once".into(),
                plan: None,
            },
            ResolvedBy::User
        )]
    );
}
