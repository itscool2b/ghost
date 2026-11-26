//! Admission, descriptors and per-native-session persistence for native controls.
use super::*;
use crate::session::catalogs::Catalog;
use ghost_drivers::controls::{ACCESS, WORKFLOW};
use ghost_protocol::session::{
    SessionControl, SessionControlChange, SessionControlChoice, SessionControlPreferences,
    SessionControls,
};

pub(super) fn is_control(id: &str) -> bool {
    matches!(id, ACCESS | WORKFLOW)
}
pub(super) fn preferences(session: Option<&AgentSessionRow>) -> SessionControlPreferences {
    match session.and_then(|s| s.controls_json.as_deref()) {
        None => SessionControlPreferences::default(),
        Some(json) => serde_json::from_str(json).unwrap_or(SessionControlPreferences {
            version: u32::MAX,
            access: None,
            workflow: Some("invalid_saved_controls".into()),
        }),
    }
}
fn legacy(value: Autonomy) -> (&'static str, &'static str) {
    match value {
        Autonomy::FullAuto => ("legacy:full_auto", "Full access"),
        Autonomy::AutoEdit => ("legacy:auto_edit", "Allow edits"),
        Autonomy::Ask => ("legacy:ask", "Ask for approval"),
    }
}
/// The level a `legacy:` access value names.
fn legacy_level(value: &str) -> Option<Autonomy> {
    [Autonomy::FullAuto, Autonomy::AutoEdit, Autonomy::Ask]
        .into_iter()
        .find(|level| legacy(*level).0 == value)
}
fn descriptor(options: &[ConfigOption], id: &str, requested: Option<String>) -> SessionControl {
    let mut result = SessionControl {
        requested,
        ..Default::default()
    };
    if let Some(option) = options.iter().find(|o| o.id == id) {
        if let ConfigKind::Select { current, choices } = &option.kind {
            result.effective = Some(current.clone());
            result.choices = choices
                .iter()
                .map(|c| SessionControlChoice {
                    value: c.value.clone(),
                    name: c.name.clone(),
                    description: c.description.clone(),
                    legacy_autonomy: None,
                })
                .collect();
        }
        result.pending = option.pending.as_ref().and_then(choice_value);
        result.note = option.description.clone();
    }
    if result.requested.is_none() {
        result.requested = result.effective.clone();
    }
    if result
        .requested
        .as_ref()
        .is_some_and(|v| !result.choices.iter().any(|c| &c.value == v))
    {
        result.note = Some(
            "The saved native setting is unavailable. Select an offered value before sending."
                .into(),
        );
    }
    if result.note.is_none() && result.requested.is_some() && result.requested != result.effective {
        result.note = Some("The requested native setting has not taken effect. Sending is paused until it is applied or you select another setting.".into());
    }
    result
}
fn choice_value(value: &ConfigValue) -> Option<String> {
    match value {
        ConfigValue::Choice { value } => Some(value.clone()),
        _ => None,
    }
}
/// A control as its connection last listed it, showing `current` when it offers it.
fn offered_as(option: &ConfigOption, current: &str) -> Option<ConfigOption> {
    let ConfigKind::Select { choices, .. } = &option.kind else {
        return None;
    };
    choices
        .iter()
        .any(|choice| choice.value == current)
        .then(|| ConfigOption {
            kind: ConfigKind::Select {
                current: current.to_owned(),
                choices: choices.clone(),
            },
            pending: None,
            ..option.clone()
        })
}
impl ChatActor {
    /// The access levels this chat can ask for: the running agent's, else the ones its saved
    /// session or its connection last reported (a chat loaded at launch has not started yet).
    fn known_levels(&self) -> Vec<Autonomy> {
        if let Some(capabilities) = &self.capabilities {
            return capabilities.autonomy_levels.clone();
        }
        self.agent_session
            .as_ref()
            .and_then(|session| session.capabilities.as_ref())
            .map(|capabilities| capabilities.autonomy_levels.clone())
            .filter(|levels| !levels.is_empty())
            .or_else(|| self.catalog().map(|catalog| catalog.autonomy_levels))
            .unwrap_or_default()
    }

    /// Whether this chat is in the agent's Plan workflow with Full access in effect as its
    /// execution access. An agent's Plan mode takes the place of its access mode, so the planning
    /// turn asks for every command although the person chose not to be asked (`policy::decide`).
    /// Only the access the agent reports counts: one that is asked for but pending, refused or
    /// not offered is not Full access.
    pub(super) fn planning_freely(&self) -> bool {
        let controls = self.control_lists();
        if controls.workflow.effective.as_deref() != Some("plan")
            || controls.access.pending.is_some()
        {
            return false;
        }
        let Some(effective) = controls.access.effective.as_deref() else {
            return false;
        };
        let full = ghost_drivers::controls::full_access(effective)
            || legacy_level(effective) == Some(Autonomy::FullAuto);
        full && controls
            .access
            .requested
            .as_deref()
            .is_none_or(|requested| requested == effective)
    }

    /// Control `id` as the agent last listed it in this run, else as its connection did.
    fn last_control<'a>(
        &'a self,
        catalog: Option<&'a Catalog>,
        id: &str,
    ) -> Option<&'a ConfigOption> {
        self.options
            .iter()
            .find(|option| option.id == id)
            .or_else(|| catalog.and_then(|catalog| catalog.control(id)))
    }

    pub(super) fn control_lists(&self) -> SessionControls {
        let live = self.options_live();
        // While the agent is not running (a chat loaded at launch, a new chat still starting),
        // the controls it or its connection last listed stand in for its own, showing what the
        // next start asks for rather than what an earlier process ran with.
        let catalog = if live { None } else { self.catalog() };
        let mut access = descriptor(&self.options, ACCESS, self.controls.access.clone());
        if !live
            && let Some(requested) = self.controls.access.as_deref()
            && let Some(option) = self
                .last_control(catalog.as_ref(), ACCESS)
                .and_then(|option| offered_as(option, requested))
        {
            access = descriptor(&[option], ACCESS, Some(requested.to_owned()));
        }
        if access.choices.is_empty() {
            access.requested = self
                .controls
                .access
                .clone()
                .or_else(|| Some(legacy(self.autonomy).0.into()));
            access.effective = if self
                .autonomy_state
                .note
                .as_ref()
                .is_some_and(|s| s.contains("refused"))
            {
                None
            } else {
                Some(legacy(self.autonomy_state.requested).0.into())
            };
            if self.controls.access.is_some() {
                access.effective = None;
            }
            access.choices = self.known_levels().iter().map(|v| { let (id,name) = legacy(*v); SessionControlChoice { value: id.into(), name: name.into(), description: Some(match v { Autonomy::FullAuto => "Use this connection full native access setting; native restrictions remain in effect.", Autonomy::AutoEdit => "Allow native file edits, with other operations governed by this connection approval policy.", Autonomy::Ask => "Use this connection approval policy for access requests." }.into()), legacy_autonomy: Some(*v) } }).collect();
        }
        access.native_mode = self.autonomy_state.native_mode.clone();
        access.note = access.note.or_else(|| self.autonomy_state.note.clone());
        let mut workflow = descriptor(&self.options, WORKFLOW, self.controls.workflow.clone());
        if !live
            && let Some(requested) = self
                .controls
                .workflow
                .as_deref()
                .or_else(|| workflow.choices.is_empty().then_some("execute"))
            && let Some(option) = self
                .last_control(catalog.as_ref(), WORKFLOW)
                .and_then(|option| offered_as(option, requested))
        {
            workflow = descriptor(&[option], WORKFLOW, self.controls.workflow.clone());
        }
        // Only a running agent can say it has no Plan control.
        if workflow.choices.is_empty() && workflow.note.is_none() && live {
            workflow.note =
                Some("This connection does not advertise a native Plan control.".into());
        }
        // A change on its way shows as pending where the choices name it: a level chosen from the
        // ones shown before the agent's own lists names none of them, and they show it once applied.
        if let Some((id, value)) = &self.control_pending {
            let target = if id == ACCESS {
                &mut access
            } else {
                &mut workflow
            };
            let value = choice_value(value);
            if value
                .as_ref()
                .is_some_and(|value| target.choices.iter().any(|choice| &choice.value == value))
            {
                target.pending = value;
                target.requested = target.pending.clone();
            }
        }
        SessionControls {
            version: 1,
            access,
            workflow,
        }
    }
    pub(super) fn native_desired(&self) -> BTreeMap<String, ConfigValue> {
        let mut desired = self.desired.clone();
        if let Some(value) = &self.controls.access {
            desired.insert(
                ACCESS.into(),
                ConfigValue::Choice {
                    value: value.clone(),
                },
            );
        }
        if let Some(value) = &self.controls.workflow {
            desired.insert(
                WORKFLOW.into(),
                ConfigValue::Choice {
                    value: value.clone(),
                },
            );
        }
        desired
    }
    pub(super) fn controls_valid(&self) -> bool {
        preferences_match(&self.controls, &self.options)
            && self.desired.iter().all(|(id, value)| {
                let mode = self
                    .options
                    .iter()
                    .find(|o| &o.id == id && o.category == ConfigCategory::Mode);
                if mode.is_none() && !matches!(id.as_str(), "mode" | "legacy-mode") {
                    return true;
                }
                mode.is_some_and(|option| match (&option.kind, value) {
                    (ConfigKind::Select { current, choices }, ConfigValue::Choice { value }) => {
                        current == value && choices.iter().any(|c| c.value == *value)
                    }
                    (ConfigKind::Toggle { current }, ConfigValue::Toggle { value }) => {
                        current == value
                    }
                    _ => false,
                })
            })
    }
    pub(super) fn controls_idle(&self) -> bool {
        self.status == SessionStatus::Ready
            && self.turn.is_none()
            && self.queue.is_empty()
            && !self.normalizer.has_open_requests()
            && self.control_pending.is_none()
            && !self.organizing
            && !self.mcp_pending
            && !self.rewind_pending
            && !self.steering_pending
            && self.git_reservation.is_none()
            && self.checkpointing.is_none()
            && self.native_identity_pending.is_none()
            && self.driver.as_ref().is_some_and(|d| {
                d.ready && d.stop == Stop::None && d.run_idle.is_none_or(|idle| idle)
            })
    }
    pub(super) fn set_control(
        &mut self,
        change: SessionControlChange,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
    ) {
        if !self.options_live()
            && self.terminal.is_none()
            && matches!(
                self.status,
                SessionStatus::Stopped
                    | SessionStatus::Failed
                    | SessionStatus::AuthRequired
                    | SessionStatus::Starting
            )
        {
            return self.keep_control_for_start(change, reply);
        }
        if !self.controls_idle() {
            let _ = reply.send(Err(SessionError::OptionInvalid("Wait until the agent is idle, clear queued messages and answer pending approvals before changing native controls.".into())));
            return;
        }
        self.set_control_admitted(change, reply);
    }

    /// The agent is not running, or its start is still being prepared: an access change is kept
    /// for the start, as a model change is, and the start's setup asks for it. Once the agent's
    /// process has its setup (the last moment of a start), a change waits for the agent to be
    /// ready: kept for later instead, it could let a message sent meanwhile run on the access the
    /// person had just left. The working mode (Plan) is kept the same way, among the modes the
    /// connection last listed: a chat opened after a restart can be put in Plan before its first
    /// message. Leaving Plan still takes the person's explicit approval.
    fn keep_control_for_start(
        &mut self,
        change: SessionControlChange,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
    ) {
        if self.driver.is_some() {
            let what = match change {
                SessionControlChange::Access { .. } => "access",
                SessionControlChange::Workflow { .. } => "mode",
            };
            let _ = reply.send(Err(SessionError::OptionInvalid(format!(
                "{} is starting. Choose the {what} again once it is ready.",
                self.agent_label()
            ))));
            return;
        }
        let controls = self.control_lists();
        let value = match change {
            SessionControlChange::Access { value } => value,
            SessionControlChange::Workflow {
                value,
                implementation_approved,
            } => {
                if !controls.workflow.choices.iter().any(|c| c.value == value) {
                    let _ = reply.send(Err(SessionError::OptionInvalid(
                        "Modes load when the agent starts. Choose the mode once it is ready."
                            .into(),
                    )));
                    return;
                }
                if value == "execute"
                    && controls.workflow.effective.as_deref() == Some("plan")
                    && !implementation_approved
                {
                    let _ = reply.send(Err(SessionError::OptionInvalid(
                        "Approve implementation explicitly before leaving Plan.".into(),
                    )));
                    return;
                }
                self.controls.version = 1;
                self.controls.workflow = Some(value);
                self.persist_controls();
                self.revision = self.revision.wrapping_add(1);
                self.info_dirty = true;
                self.schedule_flush();
                let _ = reply.send(Ok(self.lists()));
                return;
            }
        };
        let Some(choice) = controls
            .access
            .choices
            .iter()
            .find(|choice| choice.value == value)
            .cloned()
        else {
            let _ = reply.send(Err(SessionError::OptionInvalid(
                "This connection does not offer that access setting.".into(),
            )));
            return;
        };
        if controls.workflow.effective.as_deref() == Some("plan") {
            let _ = reply.send(Err(SessionError::OptionInvalid(
                "Leave Plan with implementation approval before changing execution access.".into(),
            )));
            return;
        }
        if let Some(autonomy) = choice.legacy_autonomy {
            self.store_autonomy(
                autonomy,
                stopped_autonomy(autonomy, Some("Applies when the agent starts.")),
            );
        } else {
            self.controls.version = 1;
            self.controls.access = Some(value.clone());
            self.persist_controls();
            self.revision = self.revision.wrapping_add(1);
            self.info_dirty = true;
            self.schedule_flush();
        }
        let _ = reply.send(Ok(self.lists()));
    }

    /// Saves the chat's native control choices with its session, when it has one yet.
    fn persist_controls(&mut self) {
        let Some(session) = &mut self.agent_session else {
            return;
        };
        let controls_json = serde_json::to_string(&self.controls).ok();
        session.controls_json = controls_json.clone();
        let id = session.id.clone();
        self.queue_op(Op::UpdateAgentSession {
            id,
            patch: AgentSessionPatch {
                capabilities: None,
                native_session_id: None,
                model: None,
                effort: None,
                autonomy: None,
                config_json: None,
                controls_json,
                updated_at: now_millis(),
            },
        });
    }

    /// A control change the agent is idle for.
    fn set_control_admitted(
        &mut self,
        change: SessionControlChange,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
    ) {
        let controls = self.control_lists();
        let (id, value) = match change {
            SessionControlChange::Access { value } => {
                let choice = controls.access.choices.iter().find(|c| c.value == value);
                // A level the agent supports, chosen from the levels the picker showed before the
                // agent's own lists arrived (a moment after it turns ready): it goes through the
                // agent's level, as on a connection without native access.
                let level = legacy_level(&value).filter(|level| {
                    self.capabilities
                        .as_ref()
                        .is_some_and(|capabilities| capabilities.autonomy_levels.contains(level))
                });
                let Some(legacy_autonomy) = choice.map(|c| c.legacy_autonomy).or(level.map(Some))
                else {
                    let _ = reply.send(Err(SessionError::OptionInvalid(
                        "This connection does not offer that access setting.".into(),
                    )));
                    return;
                };
                if controls.workflow.effective.as_deref() == Some("plan") {
                    let _ = reply.send(Err(SessionError::OptionInvalid(
                        "Leave Plan with implementation approval before changing execution access."
                            .into(),
                    )));
                    return;
                }
                if let Some(autonomy) = legacy_autonomy {
                    self.control_pending = Some((
                        ACCESS.into(),
                        ConfigValue::Choice {
                            value: value.clone(),
                        },
                    ));
                    self.info_dirty = true;
                    self.schedule_flush();
                    self.set_autonomy_inner(autonomy, reply, true);
                    return;
                }
                (ACCESS, value)
            }
            SessionControlChange::Workflow {
                value,
                implementation_approved,
            } => {
                if !controls.workflow.choices.iter().any(|c| c.value == value) {
                    let _ = reply.send(Err(SessionError::OptionInvalid(
                        "This connection does not offer that workflow.".into(),
                    )));
                    return;
                }
                if value == "execute"
                    && controls.workflow.effective.as_deref() != Some("execute")
                    && !implementation_approved
                {
                    let _ = reply.send(Err(SessionError::OptionInvalid(
                        "Approve implementation explicitly before leaving Plan.".into(),
                    )));
                    return;
                }
                (WORKFLOW, value)
            }
        };
        let value = ConfigValue::Choice { value };
        self.control_pending = Some((id.into(), value.clone()));
        self.revision = self.revision.wrapping_add(1);
        self.info_dirty = true;
        self.schedule_flush();
        self.set_option_inner(id.into(), value, reply, true);
    }
    pub(super) fn observe_native_controls(&mut self) {
        if self.controls.version != 1 || self.control_pending.is_some() {
            return;
        }
        let effective = self.control_lists().workflow.effective;
        if effective.as_deref() == Some("plan") && self.controls.workflow.is_none() {
            self.control_applied(
                WORKFLOW,
                &ConfigValue::Choice {
                    value: "plan".into(),
                },
            );
        } else if effective.as_deref() == Some("execute")
            && self.controls.workflow.as_deref() == Some("plan")
            && self.implementation_approved
        {
            self.implementation_approved = false;
            self.controls.access =
                self.options
                    .iter()
                    .find(|o| o.id == ACCESS)
                    .and_then(|o| match &o.kind {
                        ConfigKind::Select { current, choices }
                            if choices.iter().any(|c| c.value == *current) =>
                        {
                            Some(current.clone())
                        }
                        _ => None,
                    });
            self.control_applied(
                WORKFLOW,
                &ConfigValue::Choice {
                    value: "execute".into(),
                },
            );
        }
    }
    pub(super) fn control_applied(&mut self, id: &str, value: &ConfigValue) {
        let Some(value) = choice_value(value) else {
            return;
        };
        self.controls.version = 1;
        if id == ACCESS {
            self.controls.access = Some(value);
        } else {
            if value == "plan" && self.controls.access.is_none() {
                self.controls.access =
                    self.options
                        .iter()
                        .find(|o| o.id == ACCESS)
                        .and_then(|o| match &o.kind {
                            ConfigKind::Select { current, choices }
                                if !current.is_empty()
                                    && choices.iter().any(|c| c.value == *current) =>
                            {
                                Some(current.clone())
                            }
                            _ => None,
                        });
            }
            self.controls.workflow = Some(value);
        }
        let mut legacy_changed = false;
        for option in &self.options {
            if option.category == ConfigCategory::Mode && self.desired.contains_key(&option.id) {
                let value = match &option.kind {
                    ConfigKind::Select { current, .. } => ConfigValue::Choice {
                        value: current.clone(),
                    },
                    ConfigKind::Toggle { current } => ConfigValue::Toggle { value: *current },
                };
                self.desired.insert(option.id.clone(), value);
                legacy_changed = true;
            }
        }
        if legacy_changed {
            self.persist_desired();
        }
        self.persist_controls();
    }
}

fn preferences_match(preferences: &SessionControlPreferences, options: &[ConfigOption]) -> bool {
    preferences.version == 1 && [(ACCESS, &preferences.access), (WORKFLOW, &preferences.workflow)].iter().all(|(id,value)| {
        value.as_ref().is_none_or(|v| options.iter().any(|o| o.id == *id && matches!(&o.kind, ConfigKind::Select { current, choices } if current == v && choices.iter().any(|c| c.value == *v))))
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_legacy_preferences_are_compatible_but_unknown_or_unapplied_values_block_execution() {
        let options = vec![ghost_drivers::controls::workflow("execute")];
        let mut preferences = SessionControlPreferences::default();
        assert!(preferences_match(&preferences, &options));
        preferences.workflow = Some("future-mode".into());
        assert!(!preferences_match(&preferences, &options));
        preferences.workflow = Some("plan".into());
        assert!(!preferences_match(&preferences, &options));
        preferences.workflow = Some("execute".into());
        assert!(preferences_match(&preferences, &options));
        preferences.version = 2;
        assert!(!preferences_match(&preferences, &options));
        assert_eq!(
            self::super::preferences(None),
            SessionControlPreferences::default()
        );
    }
}
