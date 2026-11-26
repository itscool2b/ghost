//! Settings' defaults for new chats, and the models agents report for Settings to offer.

use super::*;
use ghost_drivers::testing::ScriptHandle;
use ghost_protocol::Autonomy;
use ghost_protocol::session::{
    ConfigCategory, ConfigChoice, ConfigKind, ConfigOption, ConfigValue,
};

type Seen = Arc<Mutex<Option<(Autonomy, Option<ConfigValue>, Option<ConfigValue>)>>>;

/// Records the autonomy, model and thinking level a driver is started with.
struct ObserveStart {
    inner: ScriptedDriver,
    seen: Seen,
}

impl AgentDriver for ObserveStart {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        *lock(&self.seen) = Some((
            ctx.setup.autonomy,
            ctx.setup.desired.get("model").cloned(),
            ctx.setup.desired.get("effort").cloned(),
        ));
        Box::new(self.inner).run(ctx)
    }
}

fn observed(steps: Vec<Step>) -> (Rig, Seen, ScriptHandle) {
    let (inner, handle) = ScriptedDriver::new(steps);
    let seen: Seen = Arc::new(Mutex::new(None));
    let rig = rig_with(
        Arc::new(OneDriver(Mutex::new(Some(Box::new(ObserveStart {
            inner,
            seen: Arc::clone(&seen),
        }))))),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    (rig, seen, handle)
}

fn choice(value: &str) -> ConfigChoice {
    ConfigChoice {
        value: value.into(),
        name: value.to_uppercase(),
        description: None,
        group: None,
    }
}

#[tokio::test]
async fn a_new_chat_starts_with_the_default_autonomy_and_its_agents_default_model() {
    let (rig, seen, _handle) = observed(vec![
        ready_step("defaults"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    rig.manager.set_new_chat_defaults(NewChatDefaults {
        autonomy: Autonomy::Ask,
        agent: Some("mock".into()),
        model: Some("small".into()),
        effort: None,
    });
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert_eq!(
        *lock(&seen),
        Some((
            Autonomy::Ask,
            Some(ConfigValue::Choice {
                value: "small".into()
            }),
            None
        ))
    );
    assert_eq!(
        rig.manager
            .lists(&chat.id)
            .await
            .unwrap()
            .autonomy
            .requested,
        Autonomy::Ask
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn another_agents_default_model_never_reaches_a_chat() {
    let (rig, seen, _handle) = observed(vec![
        ready_step("other-agent"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    rig.manager.set_new_chat_defaults(NewChatDefaults {
        autonomy: Autonomy::FullAuto,
        agent: Some("codex".into()),
        model: Some("gpt-5.5-codex".into()),
        effort: Some("high".into()),
    });
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert_eq!(*lock(&seen), Some((Autonomy::FullAuto, None, None)));
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

/// One entry per agent: a default saved for "Codex (ACP)" is Codex's, whichever id names it.
#[test]
fn a_default_model_follows_the_agent_whichever_connection_named_it() {
    let defaults = NewChatDefaults {
        autonomy: Autonomy::FullAuto,
        agent: Some("codex-acp".into()),
        model: Some("gpt-6".into()),
        effort: Some("xhigh".into()),
    };
    assert_eq!(defaults.model_for("codex").as_deref(), Some("gpt-6"));
    assert_eq!(defaults.model_for("codex-acp").as_deref(), Some("gpt-6"));
    assert_eq!(defaults.model_for("claude"), None);
    // The thinking level was chosen for the default model: a project's own model leaves it out.
    assert_eq!(
        defaults.effort_for("codex", Some("gpt-6")).as_deref(),
        Some("xhigh")
    );
    assert_eq!(defaults.effort_for("codex", Some("gpt-5")), None);
    assert_eq!(defaults.effort_for("codex", None), None);
    assert_eq!(defaults.effort_for("claude", Some("gpt-6")), None);
    let agent_default = NewChatDefaults {
        model: None,
        ..defaults
    };
    assert_eq!(
        agent_default.effort_for("codex", None).as_deref(),
        Some("xhigh")
    );
}

#[tokio::test]
async fn a_new_chat_starts_on_the_default_thinking_level_with_the_default_model() {
    let (rig, seen, _handle) = observed(vec![
        ready_step("thinking"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    rig.manager.set_new_chat_defaults(NewChatDefaults {
        autonomy: Autonomy::FullAuto,
        agent: Some("mock".into()),
        model: Some("small".into()),
        effort: Some("high".into()),
    });
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert_eq!(
        *lock(&seen),
        Some((
            Autonomy::FullAuto,
            Some(ConfigValue::Choice {
                value: "small".into()
            }),
            Some(ConfigValue::Choice {
                value: "high".into()
            })
        ))
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn without_settings_a_chat_starts_in_full_auto() {
    let (rig, seen, _handle) = observed(vec![
        ready_step("full-auto"),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert_eq!(*lock(&seen), Some((Autonomy::FullAuto, None, None)));
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn the_models_an_agent_lists_are_kept_for_settings() {
    let models = ConfigOption {
        id: "model".into(),
        name: "Model".into(),
        description: None,
        category: ConfigCategory::Model,
        kind: ConfigKind::Select {
            current: "small".into(),
            choices: vec![choice("small"), choice("large")],
        },
        pending: None,
    };
    let effort = ConfigOption {
        id: "effort".into(),
        name: "Effort".into(),
        description: None,
        category: ConfigCategory::Effort,
        kind: ConfigKind::Select {
            current: "low".into(),
            choices: vec![choice("low"), choice("high")],
        },
        pending: None,
    };
    let (driver, _handle) = ScriptedDriver::new(vec![
        Step::Spawn(launch()),
        ready_step("models"),
        Step::Emit(DriverEvent::Options(vec![models, effort])),
        Step::Expect(CommandMatch::Shutdown),
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    assert!(rig.manager.reported_models("mock").is_empty());
    assert!(rig.manager.reported_thinking("mock", None).is_empty());
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    eventually("the agent lists its models", || {
        !rig.manager.reported_models("mock").is_empty()
    })
    .await;
    assert_eq!(
        rig.manager.reported_models("mock"),
        vec![choice("small"), choice("large")]
    );
    assert!(rig.manager.reported_models("codex").is_empty());
    // The levels of the model it ran, which is also the one it starts on: nothing was chosen.
    let levels = vec![choice("low"), choice("high")];
    assert_eq!(rig.manager.reported_thinking("mock", Some("small")), levels);
    assert_eq!(rig.manager.reported_thinking("mock", None), levels);
    assert!(
        rig.manager
            .reported_thinking("mock", Some("large"))
            .is_empty()
    );
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}
