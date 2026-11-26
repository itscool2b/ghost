//! The model, the thinking level and the other options a chat shows and changes.
//!
//! While its agent runs, a chat shows the driver's own lists. A change the driver confirms shows at
//! once: its answer comes before the lists that follow it, so the actor marks the value applied
//! itself, and the driver's lists replace that when they arrive. When the agent is not running
//! (stopped, failed, signed out), the chat shows its connection's catalog (`session::catalogs`)
//! with the choices the next start asks for, and a change is kept for that start, as a change of
//! access is. Mode changes still wait for an idle, running agent.
use std::collections::BTreeSet;

use super::*;
use crate::session::catalogs::Catalog;

impl ChatActor {
    /// The agent runs and reads its options.
    pub(super) fn options_live(&self) -> bool {
        matches!(
            self.status,
            SessionStatus::Ready | SessionStatus::Running | SessionStatus::Cancelling
        ) && self
            .driver
            .as_ref()
            .is_some_and(|driver| driver.events.is_some())
    }

    /// The agent is not running and nothing is starting or stopping it: a change waits for its
    /// next start.
    fn options_waiting(&self) -> bool {
        self.driver.is_none()
            && self.preparing.is_none()
            && matches!(
                self.status,
                SessionStatus::Stopped | SessionStatus::Failed | SessionStatus::AuthRequired
            )
    }

    /// The connection whose catalog the chat shows: its running or saved session's, else the one
    /// a new session of its agent would use now.
    fn catalog_key(&self) -> Option<&'static str> {
        if self.driver.is_some() || self.agent_session.is_some() {
            self.spec.as_ref().map(|spec| spec.id)
        } else {
            chat_connection(&self.shared.deps.detector, &self.chat, None).map(|spec| spec.id)
        }
    }

    pub(super) fn catalog(&self) -> Option<Catalog> {
        self.catalog_key()
            .and_then(|connection| self.shared.catalogs.get(connection))
    }

    /// The options the lists show.
    pub(super) fn shown_options(&self) -> Vec<ConfigOption> {
        if self.options_live() {
            self.options.clone()
        } else {
            self.waiting_options()
        }
    }

    /// What a chat whose agent is not running shows: the model and its thinking level from the
    /// catalog (the last lists of this run when there is none), with the values the next start
    /// asks for; the other options as the agent last listed them.
    fn waiting_options(&self) -> Vec<ConfigOption> {
        let catalog = self.catalog();
        let last = |category| self.options.iter().find(|o| o.category == category);
        let session = self.agent_session.as_ref();
        let mut shown = Vec::new();
        let model = catalog
            .as_ref()
            .map(|catalog| catalog.model.clone())
            .or_else(|| last(ConfigCategory::Model).cloned());
        if let Some(mut model) = model {
            if let Some(value) =
                self.waiting_value(&model, session.and_then(|s| s.model.as_deref()))
            {
                show_value(&mut model, &value);
            }
            let selected = select_current(&model).to_owned();
            let effort = match &catalog {
                Some(catalog) => catalog.effort(&selected).flatten().cloned(),
                None => last(ConfigCategory::Effort)
                    .filter(|_| {
                        last(ConfigCategory::Model)
                            .is_some_and(|model| select_current(model) == selected)
                    })
                    .cloned(),
            };
            shown.push(model);
            if let Some(mut effort) = effort {
                let ran = session
                    .filter(|session| session.model.as_deref() == Some(selected.as_str()))
                    .and_then(|session| session.effort.as_deref());
                let value = self.waiting_value(&effort, ran).or_else(|| {
                    Some(ConfigValue::Choice {
                        value: "default".into(),
                    })
                    .filter(|value| offers(&effort, value))
                });
                if let Some(value) = value {
                    show_value(&mut effort, &value);
                }
                effort.pending = None;
                shown.push(effort);
            }
        }
        for option in &self.options {
            if matches!(
                option.category,
                ConfigCategory::Model | ConfigCategory::Effort
            ) {
                continue;
            }
            let mut option = option.clone();
            if let Some(value) = self
                .desired
                .get(&option.id)
                .filter(|value| offers(&option, value))
            {
                show_value(&mut option, value);
            }
            shown.push(option);
        }
        shown
    }

    /// The value a chat whose agent is not running shows for `option`: the one chosen for the next
    /// start, else the one its session last ran with, when the option offers it. A chosen model
    /// shows even unlisted (the agent may still run it); a level only when it is offered.
    fn waiting_value(&self, option: &ConfigOption, ran: Option<&str>) -> Option<ConfigValue> {
        if let Some(value) = self.desired.get(&option.id)
            && (option.category != ConfigCategory::Effort || offers(option, value))
        {
            return Some(value.clone());
        }
        ran.map(|value| ConfigValue::Choice {
            value: value.to_owned(),
        })
        .filter(|value| offers(option, value))
    }

    /// Keeps a change for the agent's next start when it is not running; refuses one while it
    /// starts or stops.
    pub(super) fn set_option_waiting(
        &mut self,
        option_id: String,
        value: ConfigValue,
        reply: oneshot::Sender<Result<SessionLists, SessionError>>,
    ) {
        if !self.options_waiting() {
            let _ = reply.send(Err(SessionError::OptionInvalid(
                "Wait for the agent to finish starting or stopping, then choose again.".into(),
            )));
            return;
        }
        let options = self.waiting_options();
        if let Err(error) = check_option(&options, &option_id, &value) {
            let _ = reply.send(Err(error));
            return;
        }
        let category = options
            .iter()
            .find(|option| option.id == option_id)
            .map(|option| option.category);
        if controls::is_control(&option_id) || category == Some(ConfigCategory::Mode) {
            let _ = reply.send(Err(SessionError::OptionInvalid(
                "Change the mode once the agent is running and idle.".into(),
            )));
            return;
        }
        if category == Some(ConfigCategory::Model) {
            self.fit_effort_to(&value);
        }
        self.desired.insert(option_id, value);
        self.persist_desired();
        self.revision = self.revision.wrapping_add(1);
        self.info_dirty = true;
        self.schedule_flush();
        let _ = reply.send(Ok(self.lists()));
    }

    /// A thinking level the newly chosen model does not offer is forgotten, so the next start does
    /// not ask for it. One the catalog does not know for that model stays for the driver to check.
    fn fit_effort_to(&mut self, model: &ConfigValue) {
        let ConfigValue::Choice { value: model } = model else {
            return;
        };
        let Some(catalog) = self.catalog() else {
            return;
        };
        let Some(effort) = catalog.effort(model) else {
            return;
        };
        for id in effort_ids(&catalog, &self.options) {
            let fits = effort.is_some_and(|option| {
                option.id == id
                    && self
                        .desired
                        .get(&id)
                        .is_some_and(|value| offers(option, value))
            });
            if !fits {
                self.desired.remove(&id);
            }
        }
    }

    /// The driver confirmed `value` for `option_id`: the lists show it at once. Its answer comes
    /// before the lists that follow it, and those replace these when they arrive.
    pub(super) fn show_applied(&mut self, option_id: &str, value: &ConfigValue) {
        let Some(option) = self.options.iter_mut().find(|o| o.id == option_id) else {
            return;
        };
        // A change the agent holds for the turn's end shows as pending, not applied.
        if !offers(option, value)
            || current_value(option) == *value
            || option.pending.as_ref() == Some(value)
        {
            return;
        }
        show_value(option, value);
        self.revision = self.revision.wrapping_add(1);
        self.info_dirty = true;
        self.schedule_flush();
    }

    /// A model change is about to go to the driver.
    pub(super) fn note_model_change(&mut self, option_id: &str, value: &ConfigValue) {
        let model = self
            .options
            .iter()
            .any(|o| o.id == option_id && o.category == ConfigCategory::Model);
        self.model_change = match value {
            ConfigValue::Choice { value } if model => {
                Some((self.generation, option_id.to_owned(), value.clone()))
            }
            _ => self.model_change.take(),
        };
    }

    /// The driver's lists: its connection's catalog learns them, and the lists that show a model
    /// change decide the thinking level the next start restores (drivers may reset the level when
    /// the model changes).
    pub(super) fn take_options(&mut self, options: Vec<ConfigOption>) {
        let first = self.options_generation != self.generation;
        self.options_generation = self.generation;
        if let Some(spec) = &self.spec {
            self.shared
                .catalogs
                .learn(spec.id, spec.agent, &options, first && self.default_shown);
        }
        if let Some((generation, id, model)) = &self.model_change
            && *generation == self.generation
            && options
                .iter()
                .any(|option| option.id == *id && select_current(option) == model)
        {
            self.model_change = None;
            let catalog = self.catalog();
            let mut ids: BTreeSet<String> = self
                .options
                .iter()
                .chain(&options)
                .filter(|option| option.category == ConfigCategory::Effort)
                .map(|option| option.id.clone())
                .collect();
            ids.insert("effort".into());
            if let Some(catalog) = &catalog {
                ids.extend(effort_ids(catalog, &[]));
            }
            let before = self.desired.clone();
            for id in &ids {
                self.desired.remove(id);
            }
            if let Some(effort) = options
                .iter()
                .find(|option| option.category == ConfigCategory::Effort)
            {
                let value = current_value(effort);
                let explicit = !matches!(&value, ConfigValue::Choice { value } if value.is_empty() || value == "default");
                if explicit {
                    self.desired.insert(effort.id.clone(), value);
                }
            }
            if self.desired != before {
                self.persist_desired();
            }
        }
        self.options = options;
    }

    /// At Ready: a saved thinking level the agent does not offer now (chosen for another model, or
    /// before the agent changed its levels) is forgotten; every start would ask for it again.
    pub(super) fn forget_unoffered_effort(&mut self) {
        if self.options_generation != self.generation {
            return;
        }
        let mut ids: BTreeSet<String> = self
            .options
            .iter()
            .filter(|option| option.category == ConfigCategory::Effort)
            .map(|option| option.id.clone())
            .collect();
        ids.insert("effort".into());
        for id in ids {
            let Some(value) = self.desired.get(&id) else {
                continue;
            };
            let offered = self.options.iter().any(|option| {
                option.id == id
                    && option.category == ConfigCategory::Effort
                    && offers(option, value)
            });
            if !offered {
                self.desired.remove(&id);
            }
        }
    }

    /// A catalog changed or the saved ones arrived: a chat someone views and whose agent is not
    /// running fetches its lists again.
    pub(super) fn catalogs_changed(&mut self) {
        if !self.options_live() && !self.subscribers.is_empty() {
            self.revision = self.revision.wrapping_add(1);
            self.info_dirty = true;
            self.schedule_flush();
        }
    }
}

/// The ids the catalog and `options` use for a thinking level, and `effort`.
fn effort_ids(catalog: &Catalog, options: &[ConfigOption]) -> BTreeSet<String> {
    let mut ids: BTreeSet<String> = catalog
        .efforts
        .values()
        .flatten()
        .chain(
            options
                .iter()
                .filter(|option| option.category == ConfigCategory::Effort),
        )
        .map(|option| option.id.clone())
        .collect();
    ids.insert("effort".into());
    ids
}

/// A select's current value; empty for a toggle.
fn select_current(option: &ConfigOption) -> &str {
    match &option.kind {
        ConfigKind::Select { current, .. } => current,
        ConfigKind::Toggle { .. } => "",
    }
}
