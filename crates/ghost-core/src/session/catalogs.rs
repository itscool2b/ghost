//! The model catalog each agent connection last reported: its models, the thinking levels each
//! model offered while it was current, the model it starts on when nothing is chosen, and its
//! access: the native access and workflow controls it listed and the access levels it supports. A
//! chat whose agent is not running offers these, so a model, a thinking level or an access level
//! can be chosen before the agent starts (a new chat's is still starting); the choice applies when
//! it does.
//!
//! Every list a running agent reports refreshes its connection's catalog, and the profile's
//! database keeps the catalogs across restarts as one device-only setting (another device runs
//! other agent versions). A catalog can go stale: the agent updates, an account loses a model. The
//! running agent's own lists always win once it starts, models it no longer lists lose their
//! levels, and a saved choice it no longer offers is left out by its driver with a notice.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use ghost_protocol::session::{ConfigCategory, ConfigChoice, ConfigKind, ConfigOption};
use ghost_protocol::{Autonomy, Millis};
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::watch;

use super::lock;
use crate::chatstore::ChatStore;
use crate::chatstore::agent_catalogs::{Command, MAX_BYTES};
use crate::time::now_millis;

/// The saved record's layout.
const VERSION: u32 = 1;

/// Connections kept at most; the one updated longest ago goes first.
const MAX_CATALOGS: usize = 64;

/// Models kept per connection at most (the drivers' own choice limit).
const MAX_MODELS: usize = 500;

/// A save waits this long for more changes.
const SAVE_DELAY: Duration = Duration::from_millis(300);

/// One connection's catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Catalog {
    /// The product agent (`claude` for both `claude` and `claude-acp`), for Settings.
    pub(crate) agent: String,
    pub(crate) updated_at: Millis,
    /// The Model option as last reported. Its `current` is the model the agent starts on when
    /// nothing is chosen, empty until a start without a chosen model showed it.
    pub(crate) model: ConfigOption,
    /// By model value: the thinking level option the agent offered while that model was current
    /// (its `current` means nothing), or `None` when that model offered none.
    pub(crate) efforts: BTreeMap<String, Option<ConfigOption>>,
    /// The native access and workflow controls (`aura.access`, `aura.workflow`) as last listed,
    /// their choices only: which one is chosen belongs to each chat. Older records have none.
    #[serde(default)]
    pub(crate) controls: Vec<ConfigOption>,
    /// The access levels the connection said it supports when it last started.
    #[serde(default)]
    pub(crate) autonomy_levels: Vec<Autonomy>,
}

impl Catalog {
    /// The thinking level option of `model`: `None` when unknown, `Some(None)` when it has none.
    pub(crate) fn effort(&self, model: &str) -> Option<Option<&ConfigOption>> {
        self.efforts.get(model).map(Option::as_ref)
    }

    /// The model the agent starts on when nothing is chosen, when known.
    pub(crate) fn default_model(&self) -> Option<&str> {
        match &self.model.kind {
            ConfigKind::Select { current, .. } if !current.is_empty() => Some(current),
            _ => None,
        }
    }

    pub(crate) fn models(&self) -> &[ConfigChoice] {
        match &self.model.kind {
            ConfigKind::Select { choices, .. } => choices,
            ConfigKind::Toggle { .. } => &[],
        }
    }

    /// The native control `id` as last listed, its `current` empty.
    pub(crate) fn control(&self, id: &str) -> Option<&ConfigOption> {
        self.controls.iter().find(|option| option.id == id)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    version: u32,
    #[serde(default)]
    catalogs: BTreeMap<String, Catalog>,
}

#[derive(Debug, Default)]
struct State {
    catalogs: BTreeMap<String, Catalog>,
    /// Changed since the last save.
    dirty: bool,
    /// A newer Ghost wrote the saved record: it is read, never overwritten.
    keep_saved: bool,
    /// The saved record was read (or there was none): until then nothing is saved over it.
    loaded: bool,
}

/// Every connection's catalog, shared by the chats of one manager.
#[derive(Debug)]
pub(crate) struct Catalogs {
    state: Mutex<State>,
    /// Advances whenever a catalog changes or the saved ones arrive.
    changes: watch::Sender<u64>,
}

impl Catalogs {
    pub(crate) fn new() -> Arc<Catalogs> {
        Arc::new(Catalogs {
            state: Mutex::new(State::default()),
            changes: watch::Sender::new(0),
        })
    }

    /// Loads the saved catalogs, then saves each change shortly after it, until the manager is
    /// gone. A store that is closed or paused only delays a save until the next change, and a
    /// record that could not be read is never saved over.
    pub(crate) fn keep(self: &Arc<Self>, store: Arc<ChatStore>, rt: &Handle) {
        let catalogs = Arc::downgrade(self);
        let changes = self.changes.subscribe();
        rt.spawn(save_changes(catalogs, changes, store));
    }

    /// Changes from now on.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub(crate) fn get(&self, connection: &str) -> Option<Catalog> {
        lock(&self.state).catalogs.get(connection).cloned()
    }

    /// The catalog of `agent`: the one of `connection` when there is one, else the one of its
    /// connections updated last.
    pub(crate) fn for_agent(&self, agent: &str, connection: Option<&str>) -> Option<Catalog> {
        let state = lock(&self.state);
        connection
            .and_then(|connection| state.catalogs.get(connection))
            .or_else(|| {
                state
                    .catalogs
                    .values()
                    .filter(|catalog| catalog.agent == agent)
                    .max_by_key(|catalog| catalog.updated_at)
            })
            .cloned()
    }

    /// Records what a running agent of `connection` listed. `shows_default`: these are the first
    /// lists of a start that chose no model, so their model is the one the agent starts on.
    pub(crate) fn learn(
        &self,
        connection: &str,
        agent: &str,
        options: &[ConfigOption],
        shows_default: bool,
    ) {
        let Some(model) = options.iter().find(|option| {
            option.category == ConfigCategory::Model
                && matches!(&option.kind, ConfigKind::Select { choices, .. } if !choices.is_empty())
        }) else {
            return;
        };
        let ConfigKind::Select { current, choices } = &model.kind else {
            return;
        };
        let effort = options
            .iter()
            .find(|option| option.category == ConfigCategory::Effort)
            .map(levels_only);
        let mut state = lock(&self.state);
        let known = state.catalogs.get(connection);
        let default = if shows_default {
            current.clone()
        } else {
            known
                .and_then(Catalog::default_model)
                .unwrap_or_default()
                .to_owned()
        };
        let mut efforts = known.map(|known| known.efforts.clone()).unwrap_or_default();
        if !current.is_empty() {
            efforts.insert(current.clone(), effort);
        }
        // A model the agent no longer lists keeps no levels.
        efforts.retain(|value, _| choices.iter().any(|choice| choice.value == *value));
        // Every list a driver sends is whole: one without native controls means it has none now.
        let controls = options
            .iter()
            .filter(|option| is_control(&option.id))
            .map(levels_only)
            .collect();
        let next = Catalog {
            agent: agent.to_owned(),
            updated_at: now_millis(),
            model: ConfigOption {
                pending: None,
                kind: ConfigKind::Select {
                    current: default,
                    choices: choices.iter().take(MAX_MODELS).cloned().collect(),
                },
                ..model.clone()
            },
            efforts,
            controls,
            autonomy_levels: known
                .map(|known| known.autonomy_levels.clone())
                .unwrap_or_default(),
        };
        if known.is_some_and(|known| same_but_time(known, &next)) {
            return;
        }
        state.catalogs.insert(connection.to_owned(), next);
        while state.catalogs.len() > MAX_CATALOGS {
            let Some(oldest) = state
                .catalogs
                .iter()
                .min_by_key(|(_, catalog)| catalog.updated_at)
                .map(|(connection, _)| connection.clone())
            else {
                break;
            };
            state.catalogs.remove(&oldest);
        }
        state.dirty = true;
        drop(state);
        self.changes
            .send_modify(|count| *count = count.wrapping_add(1));
    }

    /// Records the access levels a started agent of `connection` said it supports. A connection
    /// without a catalog (it listed no models) keeps none.
    pub(crate) fn learn_levels(&self, connection: &str, levels: &[Autonomy]) {
        let mut state = lock(&self.state);
        let Some(catalog) = state.catalogs.get_mut(connection) else {
            return;
        };
        if catalog.autonomy_levels == levels {
            return;
        }
        catalog.autonomy_levels = levels.to_vec();
        catalog.updated_at = now_millis();
        state.dirty = true;
        drop(state);
        self.changes
            .send_modify(|count| *count = count.wrapping_add(1));
    }

    /// The saved catalogs join the ones learned in this run, which are newer.
    fn merge_saved(&self, json: &str) {
        let value: serde_json::Value = match serde_json::from_str(json) {
            Ok(value) => value,
            Err(_) => {
                tracing::warn!("the agents' saved model lists were unreadable and start over");
                return;
            }
        };
        let mut state = lock(&self.state);
        if value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|version| version > u64::from(VERSION))
        {
            state.keep_saved = true;
            return;
        }
        let Ok(record) = serde_json::from_value::<Record>(value) else {
            tracing::warn!("the agents' saved model lists were unreadable and start over");
            return;
        };
        let mut added = false;
        for (connection, catalog) in record.catalogs.into_iter().take(MAX_CATALOGS) {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                state.catalogs.entry(connection)
            {
                entry.insert(catalog);
                added = true;
            }
        }
        drop(state);
        if added {
            self.changes
                .send_modify(|count| *count = count.wrapping_add(1));
        }
    }

    /// The record to save, when something changed since the last save and the saved one was read.
    fn take_dirty(&self) -> Option<String> {
        let mut state = lock(&self.state);
        if !state.dirty || state.keep_saved || !state.loaded {
            return None;
        }
        state.dirty = false;
        let record = Record {
            version: VERSION,
            catalogs: state.catalogs.clone(),
        };
        serde_json::to_string(&record)
            .ok()
            .filter(|json| json.len() <= MAX_BYTES)
    }

    fn mark_dirty(&self) {
        lock(&self.state).dirty = true;
    }

    fn loaded(&self) -> bool {
        lock(&self.state).loaded
    }

    /// Saves what changed since the last save at once: a quit does not wait for the next save.
    pub(crate) async fn save_now(&self, store: &ChatStore) {
        let Some(json) = self.take_dirty() else {
            return;
        };
        if store.agent_catalogs(Command::Save(json)).await.is_err() {
            self.mark_dirty();
        }
    }
}

async fn save_changes(
    catalogs: Weak<Catalogs>,
    mut changes: watch::Receiver<u64>,
    store: Arc<ChatStore>,
) {
    load(&catalogs, &store).await;
    // The sender lives in the catalogs: once the manager is gone, this ends.
    while changes.changed().await.is_ok() {
        tokio::time::sleep(SAVE_DELAY).await;
        changes.mark_unchanged();
        let Some(book) = catalogs.upgrade() else {
            return;
        };
        if !book.loaded() {
            drop(book);
            if !load(&catalogs, &store).await {
                continue;
            }
        } else {
            drop(book);
        }
        let Some(json) = catalogs
            .upgrade()
            .and_then(|catalogs| catalogs.take_dirty())
        else {
            continue;
        };
        if let Err(error) = store.agent_catalogs(Command::Save(json)).await {
            tracing::debug!(%error, "the agents' model lists could not be saved yet");
            if let Some(catalogs) = catalogs.upgrade() {
                catalogs.mark_dirty();
            }
        }
    }
}

/// Reads the saved catalogs into `catalogs`; whether the record could be read.
async fn load(catalogs: &Weak<Catalogs>, store: &ChatStore) -> bool {
    match store.agent_catalogs(Command::Load).await {
        Ok(saved) => {
            if let Some(catalogs) = catalogs.upgrade() {
                if let Some(json) = saved {
                    catalogs.merge_saved(&json);
                }
                lock(&catalogs.state).loaded = true;
            }
            true
        }
        Err(error) => {
            tracing::debug!(%error, "the agents' saved model lists could not be read yet");
            false
        }
    }
}

/// The option with its levels only: which one is chosen belongs to each chat.
fn levels_only(option: &ConfigOption) -> ConfigOption {
    ConfigOption {
        pending: None,
        kind: match &option.kind {
            ConfigKind::Select { choices, .. } => ConfigKind::Select {
                current: String::new(),
                choices: choices.iter().take(MAX_MODELS).cloned().collect(),
            },
            ConfigKind::Toggle { .. } => ConfigKind::Toggle { current: false },
        },
        ..option.clone()
    }
}

fn same_but_time(a: &Catalog, b: &Catalog) -> bool {
    a.agent == b.agent
        && a.model == b.model
        && a.efforts == b.efforts
        && a.controls == b.controls
        && a.autonomy_levels == b.autonomy_levels
}

/// The native access and workflow controls, which every chat chooses for itself.
fn is_control(id: &str) -> bool {
    matches!(
        id,
        ghost_drivers::controls::ACCESS | ghost_drivers::controls::WORKFLOW
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_protocol::session::ConfigChoice;

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
                        name: value.to_uppercase(),
                        description: None,
                        group: None,
                    })
                    .collect(),
            },
            pending: None,
        }
    }

    fn lists(model: &str, effort: Option<(&str, &[&str])>) -> Vec<ConfigOption> {
        let mut options = vec![select(
            "model",
            ConfigCategory::Model,
            model,
            &["fast", "smart", "tiny"],
        )];
        if let Some((current, levels)) = effort {
            options.push(select("effort", ConfigCategory::Effort, current, levels));
        }
        options.push(select("mode", ConfigCategory::Mode, "ask", &["ask"]));
        options
    }

    #[test]
    fn learns_each_models_levels_and_the_default_only_from_a_start_without_a_choice() {
        let catalogs = Catalogs::new();
        let mut changes = catalogs.subscribe();
        catalogs.learn(
            "mock",
            "mock",
            &lists("smart", Some(("high", &["low", "high"]))),
            false,
        );
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();
        let catalog = catalogs.get("mock").expect("learned");
        assert_eq!(
            catalog.default_model(),
            None,
            "a chosen model is no default"
        );
        let effort = catalog.effort("smart").flatten().expect("smart's levels");
        assert!(
            matches!(&effort.kind, ConfigKind::Select { current, choices } if current.is_empty() && choices.len() == 2)
        );
        assert_eq!(catalog.effort("tiny"), None, "unknown");
        catalogs.learn("mock", "mock", &lists("tiny", None), false);
        assert_eq!(catalogs.get("mock").unwrap().effort("tiny"), Some(None));
        catalogs.learn(
            "mock",
            "mock",
            &lists("fast", Some(("low", &["low"]))),
            true,
        );
        let catalog = catalogs.get("mock").unwrap();
        assert_eq!(catalog.default_model(), Some("fast"));
        assert!(catalog.effort("smart").is_some(), "earlier levels stay");
        // Only the chosen level differs: nothing changes, nothing is saved.
        changes.mark_unchanged();
        catalogs.learn(
            "mock",
            "mock",
            &lists("fast", Some(("high", &["low"]))),
            false,
        );
        assert!(!changes.has_changed().unwrap());
        assert_eq!(catalogs.get("mock").unwrap().default_model(), Some("fast"));
        // A model the agent stopped listing loses its levels.
        let mut fewer = lists("fast", Some(("low", &["low"])));
        fewer[0] = select("model", ConfigCategory::Model, "fast", &["fast", "tiny"]);
        catalogs.learn("mock", "mock", &fewer, false);
        assert_eq!(catalogs.get("mock").unwrap().effort("smart"), None);
        // Lists without models teach nothing.
        catalogs.learn(
            "other",
            "other",
            &[select("mode", ConfigCategory::Mode, "a", &["a"])],
            true,
        );
        assert!(catalogs.get("other").is_none());
    }

    #[test]
    fn learns_the_access_controls_and_levels_without_any_chats_choice() {
        let catalogs = read();
        let mut options = lists("smart", None);
        options.push(select(
            ghost_drivers::controls::ACCESS,
            ConfigCategory::Other,
            "bypassPermissions",
            &["default", "bypassPermissions"],
        ));
        catalogs.learn_levels("mock", &[Autonomy::Ask]);
        assert!(
            catalogs.get("mock").is_none(),
            "levels alone make no catalog"
        );
        catalogs.learn("mock", "mock", &options, true);
        let catalog = catalogs.get("mock").unwrap();
        let access = catalog
            .control(ghost_drivers::controls::ACCESS)
            .expect("access");
        assert!(
            matches!(&access.kind, ConfigKind::Select { current, choices } if current.is_empty() && choices.len() == 2)
        );
        assert!(catalog.control("mode").is_none(), "only native controls");
        catalogs.take_dirty();
        catalogs.learn_levels("mock", &[Autonomy::FullAuto, Autonomy::Ask]);
        let saved = catalogs.take_dirty().expect("the levels are saved");
        catalogs.learn_levels("mock", &[Autonomy::FullAuto, Autonomy::Ask]);
        assert!(
            catalogs.take_dirty().is_none(),
            "the same levels change nothing"
        );
        // A later list keeps the levels; one without controls drops them.
        catalogs.learn("mock", "mock", &lists("smart", None), false);
        let catalog = catalogs.get("mock").unwrap();
        assert!(catalog.controls.is_empty());
        assert_eq!(catalog.autonomy_levels, [Autonomy::FullAuto, Autonomy::Ask]);
        let restarted = read();
        restarted.merge_saved(&saved);
        assert_eq!(
            restarted.get("mock").unwrap().autonomy_levels,
            [Autonomy::FullAuto, Autonomy::Ask]
        );
        // A record saved before access was kept reads with none.
        let mut older: serde_json::Value = serde_json::from_str(&saved).unwrap();
        let entry = older["catalogs"]["mock"].as_object_mut().unwrap();
        entry.remove("controls");
        entry.remove("autonomy_levels");
        let restarted = read();
        restarted.merge_saved(&older.to_string());
        let catalog = restarted.get("mock").expect("an older record still loads");
        assert!(catalog.controls.is_empty() && catalog.autonomy_levels.is_empty());
    }

    /// A book whose saved record was read.
    fn read() -> Arc<Catalogs> {
        let catalogs = Catalogs::new();
        lock(&catalogs.state).loaded = true;
        catalogs
    }

    #[test]
    fn saved_catalogs_round_trip_and_never_replace_newer_ones_or_a_newer_record() {
        let unread = Catalogs::new();
        unread.learn("mock", "mock", &lists("smart", None), true);
        assert!(
            unread.take_dirty().is_none(),
            "nothing is saved over an unread record"
        );
        let catalogs = read();
        catalogs.learn("mock", "mock", &lists("smart", None), true);
        let saved = catalogs.take_dirty().expect("dirty");
        assert!(catalogs.take_dirty().is_none(), "saved once");
        let restarted = read();
        restarted.merge_saved(&saved);
        assert_eq!(restarted.get("mock"), catalogs.get("mock"));
        assert!(restarted.take_dirty().is_none(), "nothing new to save");
        // A list learned before the saved ones arrived wins over them.
        let fresh = Catalogs::new();
        fresh.learn("mock", "mock", &lists("tiny", None), true);
        fresh.merge_saved(&saved);
        assert_eq!(fresh.get("mock").unwrap().default_model(), Some("tiny"));
        assert_eq!(
            fresh
                .for_agent("mock", None)
                .map(|catalog| catalog.default_model().map(str::to_owned)),
            Some(Some("tiny".into()))
        );
        // Unreadable records start over; a newer layout is read-only.
        let broken = Catalogs::new();
        broken.merge_saved("{not json");
        assert!(broken.get("mock").is_none());
        let newer = read();
        newer.merge_saved(r#"{"version":99,"catalogs":{}}"#);
        newer.learn("mock", "mock", &lists("smart", None), true);
        assert!(newer.take_dirty().is_none(), "a newer record is kept");
    }
}
