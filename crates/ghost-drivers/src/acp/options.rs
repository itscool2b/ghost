//! The option model: ACP config options win; legacy `modes` and `models` become synthesized
//! options, each with its route (`set_config_option`, `session/set_mode`, the untyped
//! `session/set_model`).
//!
//! - `configOptions` entries: `select` becomes a select (grouped choices flattened, the group name
//!   kept), `boolean` a toggle, other types are skipped. Category `mode`, `model`, `model_config`
//!   map to Mode, Model, ModelConfig; `thought_level`, `effort`, `reasoning`, `reasoning_effort`
//!   and `thinking` to Effort, as does a select with one of those ids and no category; anything
//!   else to Other.
//! - Legacy `modes` become the option `mode` (category Mode) only when no real option has category
//!   Mode; legacy `models` the option `model` (category Model) only when none has category Model.
//! - Limits: 100 options, 500 choices each, names 200 characters, descriptions 300.

use ghost_protocol::session::{
    ConfigCategory, ConfigChoice, ConfigKind, ConfigOption, ConfigValue,
};
use serde_json::Value;

use super::requests::{self, methods};
use super::wire::{self, LegacyList, OpenReply};

pub const MAX_OPTIONS: usize = 100;
pub const MAX_CHOICES: usize = 500;
pub const MAX_NAME_CHARS: usize = 200;
pub const MAX_DESCRIPTION_CHARS: usize = 300;

/// How a change of one option reaches the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `session/set_config_option` with the option's id as `configId`.
    ConfigOption,
    /// `session/set_mode` (a synthesized option from the legacy `modes`).
    SetMode,
    /// The untyped `session/set_model` (a synthesized option from the legacy `models`).
    SetModel,
    NativeControl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    option: ConfigOption,
    route: Route,
}

/// Every option of a session, with current values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionModel {
    controls_dialect: String,
    execution_mode: Option<String>,
    controls: Vec<Entry>,
    real: Vec<Entry>,
    /// The legacy lists, kept so the synthesized options can be rebuilt when the real ones change.
    legacy_modes: Option<LegacyList>,
    legacy_models: Option<LegacyList>,
    synthesized: Vec<Entry>,
    /// The open reply had `configOptions` (Hermes' had none and answers changes with `[]`).
    open_had_options: bool,
}

impl OptionModel {
    pub fn from_open(reply: &OpenReply) -> OptionModel {
        let mut model = OptionModel {
            controls_dialect: String::new(),
            execution_mode: None,
            controls: Vec::new(),
            real: Vec::new(),
            legacy_modes: reply.modes.clone(),
            legacy_models: reply.models.clone(),
            synthesized: Vec::new(),
            open_had_options: reply.config_options.is_some(),
        };
        model.set_real(reply.config_options.as_deref().unwrap_or_default());
        model
    }

    /// Only verified adapter mode semantics become the dedicated controls. Unknown modes stay
    /// in the native options list without being labeled as execution or planning.
    pub fn enable_native_controls(&mut self, dialect: &str) {
        self.controls_dialect = dialect.into();
        self.rebuild_controls();
    }
    pub fn remember_execution_access(&mut self, value: &str) {
        if self.execution_choice(value) && self.offers_mode(value) {
            self.execution_mode = Some(value.into());
            self.rebuild_controls();
        }
    }
    fn execution_choice(&self, value: &str) -> bool {
        match self.controls_dialect.as_str() {
            "claude-acp" => matches!(value, "default" | "acceptEdits" | "bypassPermissions"),
            "codex-acp" => matches!(value, "read-only" | "auto" | "full-access"),
            "opencode" => value == "build",
            "cursor" => matches!(value, "agent" | "ask"),
            _ => false,
        }
    }
    fn rebuild_controls(&mut self) {
        self.controls.clear();
        let Some(mode) = self.mode().cloned() else {
            return;
        };
        let ConfigKind::Select { current, choices } = mode.kind else {
            return;
        };
        let access: Vec<_> = choices
            .iter()
            .filter(|c| self.execution_choice(&c.value))
            .cloned()
            .collect();
        if self.execution_choice(&current) {
            self.execution_mode = Some(current.clone());
        }
        if !access.is_empty() {
            self.controls.push(Entry {
                option: crate::controls::select(
                    crate::controls::ACCESS,
                    "Access",
                    self.execution_mode.as_deref().unwrap_or(""),
                    access.clone(),
                ),
                route: Route::NativeControl,
            });
        }
        if self.execution_mode.is_none()
            && current == "plan"
            && choices.iter().any(|c| c.value == "plan")
            && !access.is_empty()
        {
            let mut workflow = crate::controls::workflow("plan");
            if let ConfigKind::Select { choices, .. } = &mut workflow.kind {
                choices.retain(|c| c.value == "plan");
            }
            workflow.description = Some("Native Plan is active. The prior execution access is unknown; use the native harness to choose it explicitly.".into());
            self.controls.push(Entry {
                option: workflow,
                route: Route::NativeControl,
            });
        }
        if self.execution_mode.is_some()
            && choices.iter().any(|c| c.value == "plan")
            && !access.is_empty()
            && (current == "plan" || self.execution_choice(&current))
        {
            self.controls.push(Entry {
                option: crate::controls::workflow(if current == "plan" {
                    "plan"
                } else {
                    "execute"
                }),
                route: Route::NativeControl,
            });
        }
    }

    fn native_target(&self, id: &str, value: &ConfigValue) -> Option<(String, ConfigValue)> {
        let ConfigValue::Choice { value } = value else {
            return None;
        };
        let native = match id {
            crate::controls::ACCESS if self.execution_choice(value) => value.clone(),
            crate::controls::WORKFLOW if value == "plan" => "plan".into(),
            crate::controls::WORKFLOW if value == "execute" => self.execution_mode.clone()?,
            _ => return None,
        };
        Some((
            self.mode()?.id.clone(),
            ConfigValue::Choice { value: native },
        ))
    }

    /// The full list with current values: real options first, then the synthesized ones.
    pub fn options(&self) -> Vec<ConfigOption> {
        self.entries().map(|entry| entry.option.clone()).collect()
    }

    fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.real
            .iter()
            .chain(self.synthesized.iter())
            .chain(self.controls.iter())
    }

    fn entry(&self, option_id: &str) -> Option<&Entry> {
        self.entries().find(|entry| entry.option.id == option_id)
    }

    fn entry_mut(&mut self, option_id: &str) -> Option<&mut Entry> {
        self.real
            .iter_mut()
            .chain(self.synthesized.iter_mut())
            .chain(self.controls.iter_mut())
            .find(|entry| entry.option.id == option_id)
    }

    pub fn get(&self, option_id: &str) -> Option<&ConfigOption> {
        self.entry(option_id).map(|entry| &entry.option)
    }

    /// The first option of `category`.
    pub fn by_category(&self, category: ConfigCategory) -> Option<&ConfigOption> {
        self.entries()
            .map(|entry| &entry.option)
            .find(|option| option.category == category)
    }

    pub fn has_category(&self, category: ConfigCategory) -> bool {
        self.by_category(category).is_some()
    }

    /// The Mode option's id, current value and choices.
    pub fn mode(&self) -> Option<&ConfigOption> {
        self.by_category(ConfigCategory::Mode)
    }

    /// The current mode, when there is a Mode option.
    pub fn current_mode(&self) -> Option<&str> {
        match &self.mode()?.kind {
            ConfigKind::Select { current, .. } => Some(current),
            ConfigKind::Toggle { .. } => None,
        }
    }

    /// Whether the Mode option offers `mode`.
    pub fn offers_mode(&self, mode: &str) -> bool {
        self.mode().is_some_and(|option| offers(option, mode))
    }

    /// The display name of a mode, or the id.
    pub fn mode_name(&self, mode: &str) -> String {
        self.mode()
            .and_then(|option| choice_name(option, mode))
            .unwrap_or_else(|| mode.to_owned())
    }

    /// `Ok` when `value` is something `option_id` offers; else why not.
    pub fn validate(&self, option_id: &str, value: &ConfigValue) -> Result<(), String> {
        let Some(option) = self.get(option_id) else {
            return Err(format!("This agent has no option \"{option_id}\"."));
        };
        match (&option.kind, value) {
            (ConfigKind::Select { choices, .. }, ConfigValue::Choice { value }) => {
                if choices.iter().any(|choice| choice.value == *value) {
                    Ok(())
                } else {
                    Err(format!(
                        "\"{value}\" isn't one of the choices for {}.",
                        option.name
                    ))
                }
            }
            (ConfigKind::Toggle { .. }, ConfigValue::Toggle { .. }) => Ok(()),
            (ConfigKind::Select { .. }, ConfigValue::Toggle { .. }) => {
                Err(format!("{} takes a choice, not on or off.", option.name))
            }
            (ConfigKind::Toggle { .. }, ConfigValue::Choice { .. }) => {
                Err(format!("{} is on or off, not a choice.", option.name))
            }
        }
    }

    /// Whether `value` is the option's current value.
    pub fn is_current(&self, option_id: &str, value: &ConfigValue) -> bool {
        self.get(option_id)
            .is_some_and(|option| current_value(option) == *value)
    }

    /// The method and params that set `option_id` to `value`.
    pub fn request(
        &self,
        session_id: &str,
        option_id: &str,
        value: &ConfigValue,
    ) -> Option<(&'static str, Value)> {
        if let Some((id, value)) = self.native_target(option_id, value) {
            return self.request(session_id, &id, &value);
        }
        let entry = self.entry(option_id)?;
        Some(match (entry.route, value) {
            (Route::ConfigOption, value) => (
                methods::SESSION_SET_CONFIG_OPTION,
                requests::set_config_option(session_id, option_id, value),
            ),
            (Route::SetMode, ConfigValue::Choice { value }) => (
                methods::SESSION_SET_MODE,
                requests::set_mode(session_id, value),
            ),
            (Route::SetModel, ConfigValue::Choice { value }) => (
                methods::SESSION_SET_MODEL,
                requests::set_model(session_id, value),
            ),
            (Route::SetMode | Route::SetModel, ConfigValue::Toggle { .. })
            | (Route::NativeControl, _) => return None,
        })
    }

    /// The agent accepted `value` for `option_id`. A `set_config_option` reply's complete list
    /// replaces the real options, except an empty list from an agent whose open reply had none
    /// (Hermes).
    pub fn applied(&mut self, option_id: &str, value: &ConfigValue, reply: Option<&[Value]>) {
        if let Some((id, value)) = self.native_target(option_id, value) {
            self.applied(&id, &value, reply);
            return;
        }
        let route = self.entry(option_id).map(|entry| entry.route);
        if route == Some(Route::ConfigOption)
            && let Some(list) = reply
            && (!list.is_empty() || self.open_had_options)
        {
            self.set_real(list);
            return;
        }
        self.set_current(option_id, value);
        self.rebuild_controls();
    }

    /// `config_option_update`: replaces every real option; the synthesized ones stay unless a real
    /// option now covers their category.
    pub fn replace_real(&mut self, raw: &[Value]) {
        self.set_real(raw);
    }

    /// `current_mode_update`: the current value of the Mode option. Returns whether it changed.
    pub fn set_current_mode(&mut self, mode: &str) -> bool {
        let Some(id) = self.mode().map(|option| option.id.clone()) else {
            return false;
        };
        let before = self.current_mode().map(str::to_owned);
        self.set_current(
            &id,
            &ConfigValue::Choice {
                value: mode.to_owned(),
            },
        );
        self.rebuild_controls();
        before.as_deref() != Some(mode)
    }

    /// Marks a change waiting for the turn to end (or clears it).
    pub fn set_pending(&mut self, option_id: &str, pending: Option<ConfigValue>) {
        if let Some(entry) = self.entry_mut(option_id) {
            entry.option.pending = pending;
        }
    }

    pub fn pending(&self, option_id: &str) -> Option<&ConfigValue> {
        self.get(option_id)?.pending.as_ref()
    }

    fn set_current(&mut self, option_id: &str, value: &ConfigValue) {
        let Some(entry) = self.entry_mut(option_id) else {
            return;
        };
        match (&mut entry.option.kind, value) {
            (ConfigKind::Select { current, choices }, ConfigValue::Choice { value }) => {
                *current = value.clone();
                ensure_choice(choices, value);
            }
            (ConfigKind::Toggle { current }, ConfigValue::Toggle { value }) => *current = *value,
            _ => {}
        }
        let route = entry.route;
        // The legacy list follows, so rebuilding the synthesized option keeps the value.
        let legacy = match route {
            Route::SetMode => self.legacy_modes.as_mut(),
            Route::SetModel => self.legacy_models.as_mut(),
            Route::ConfigOption | Route::NativeControl => None,
        };
        if let Some(list) = legacy
            && let ConfigValue::Choice { value } = value
        {
            list.current = value.clone();
        }
    }

    fn set_real(&mut self, raw: &[Value]) {
        self.real = raw
            .iter()
            .filter_map(parse_config_option)
            .filter(|option| {
                !matches!(
                    option.id.as_str(),
                    crate::controls::ACCESS | crate::controls::WORKFLOW
                )
            })
            .take(MAX_OPTIONS)
            .map(|option| Entry {
                option,
                route: Route::ConfigOption,
            })
            .collect();
        self.synthesize();
    }

    fn synthesize(&mut self) {
        let pending: Vec<(String, ConfigValue)> = self
            .synthesized
            .iter()
            .filter_map(|entry| Some((entry.option.id.clone(), entry.option.pending.clone()?)))
            .collect();
        self.synthesized.clear();
        let real_has = |category| {
            self.real
                .iter()
                .any(|entry| entry.option.category == category)
        };
        let mut synthesized = Vec::new();
        if !real_has(ConfigCategory::Mode)
            && let Some(modes) = &self.legacy_modes
        {
            synthesized.push(Entry {
                option: legacy_option(self.free_id("mode"), "Mode", ConfigCategory::Mode, modes),
                route: Route::SetMode,
            });
        }
        if !real_has(ConfigCategory::Model)
            && let Some(models) = &self.legacy_models
        {
            synthesized.push(Entry {
                option: legacy_option(
                    self.free_id("model"),
                    "Model",
                    ConfigCategory::Model,
                    models,
                ),
                route: Route::SetModel,
            });
        }
        for entry in &mut synthesized {
            if let Some((_, value)) = pending.iter().find(|(id, _)| *id == entry.option.id) {
                entry.option.pending = Some(value.clone());
            }
        }
        self.synthesized = synthesized;
        self.rebuild_controls();
    }

    /// `id`, or a variant no real option uses.
    fn free_id(&self, id: &str) -> String {
        if self.real.iter().any(|entry| entry.option.id == id) {
            format!("legacy-{id}")
        } else {
            id.to_owned()
        }
    }
}

/// The current value of an option as a [`ConfigValue`].
pub fn current_value(option: &ConfigOption) -> ConfigValue {
    match &option.kind {
        ConfigKind::Select { current, .. } => ConfigValue::Choice {
            value: current.clone(),
        },
        ConfigKind::Toggle { current } => ConfigValue::Toggle { value: *current },
    }
}

/// How a value reads in a notice: a choice's name, or on and off.
pub fn value_label(option: &ConfigOption, value: &ConfigValue) -> String {
    match value {
        ConfigValue::Choice { value } => {
            choice_name(option, value).unwrap_or_else(|| value.clone())
        }
        ConfigValue::Toggle { value: true } => "on".into(),
        ConfigValue::Toggle { value: false } => "off".into(),
    }
}

fn offers(option: &ConfigOption, value: &str) -> bool {
    match &option.kind {
        ConfigKind::Select { choices, .. } => choices.iter().any(|choice| choice.value == value),
        ConfigKind::Toggle { .. } => false,
    }
}

fn choice_name(option: &ConfigOption, value: &str) -> Option<String> {
    match &option.kind {
        ConfigKind::Select { choices, .. } => choices
            .iter()
            .find(|choice| choice.value == value)
            .map(|choice| choice.name.clone()),
        ConfigKind::Toggle { .. } => None,
    }
}

/// A current value the agent did not list is added, so the select always shows it.
fn ensure_choice(choices: &mut Vec<ConfigChoice>, value: &str) {
    if !choices.iter().any(|choice| choice.value == value) {
        choices.push(ConfigChoice {
            value: value.to_owned(),
            name: value.to_owned(),
            description: None,
            group: None,
        });
    }
}

fn legacy_option(
    id: String,
    name: &str,
    category: ConfigCategory,
    list: &LegacyList,
) -> ConfigOption {
    let mut choices: Vec<ConfigChoice> = list
        .available
        .iter()
        .take(MAX_CHOICES)
        .map(|choice| ConfigChoice {
            value: choice.id.clone(),
            name: truncate(&choice.name, MAX_NAME_CHARS),
            description: choice
                .description
                .as_deref()
                .map(|text| truncate(text, MAX_DESCRIPTION_CHARS)),
            group: None,
        })
        .collect();
    ensure_choice(&mut choices, &list.current);
    ConfigOption {
        id,
        name: name.to_owned(),
        description: None,
        category,
        kind: ConfigKind::Select {
            current: list.current.clone(),
            choices,
        },
        pending: None,
    }
}

/// One `configOptions` entry; `None` for an entry Ghost cannot use (no id, another type).
pub fn parse_config_option(raw: &Value) -> Option<ConfigOption> {
    let id = wire::string_at(raw, "id")?;
    let kind = match wire::str_at(raw, "type")? {
        "select" => {
            let current = wire::string_at(raw, "currentValue")?;
            let mut choices = Vec::new();
            for entry in raw.get("options")?.as_array()? {
                if let Some(group_options) = entry.get("options").and_then(Value::as_array) {
                    let group = wire::string_at(entry, "name")
                        .or_else(|| wire::string_at(entry, "group"))
                        .map(|name| truncate(&name, MAX_NAME_CHARS));
                    choices.extend(
                        group_options
                            .iter()
                            .filter_map(|choice| select_choice(choice, group.clone())),
                    );
                } else if let Some(choice) = select_choice(entry, None) {
                    choices.push(choice);
                }
            }
            choices.truncate(MAX_CHOICES);
            ensure_choice(&mut choices, &current);
            ConfigKind::Select { current, choices }
        }
        "boolean" => ConfigKind::Toggle {
            current: raw.get("currentValue")?.as_bool()?,
        },
        _ => return None,
    };
    let category = match wire::str_at(raw, "category") {
        Some("mode") => ConfigCategory::Mode,
        Some("model") => ConfigCategory::Model,
        Some(category) if names_effort(category) => ConfigCategory::Effort,
        Some("model_config") => ConfigCategory::ModelConfig,
        // An agent that leaves the category out may still name its thinking level plainly.
        None | Some("") if matches!(kind, ConfigKind::Select { .. }) && names_effort(&id) => {
            ConfigCategory::Effort
        }
        _ => ConfigCategory::Other,
    };
    Some(ConfigOption {
        name: truncate(
            &wire::string_at(raw, "name").unwrap_or_else(|| id.clone()),
            MAX_NAME_CHARS,
        ),
        id,
        description: wire::str_at(raw, "description")
            .map(|text| truncate(text, MAX_DESCRIPTION_CHARS)),
        category,
        kind,
        pending: None,
    })
}

/// Whether a category or an option id names a thinking level: `thought_level` (ACP's own),
/// `effort`, `reasoning`, `reasoning_effort` or `thinking`, in any case and with `-`, `_` or no
/// separator. Exact names only, so `reasoning_summary` or `thinking_budget` stay where they are.
fn names_effort(name: &str) -> bool {
    let plain: String = name
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        plain.as_str(),
        "thoughtlevel" | "effort" | "reasoning" | "reasoningeffort" | "thinking"
    )
}

fn select_choice(raw: &Value, group: Option<String>) -> Option<ConfigChoice> {
    let value = wire::string_at(raw, "value")?;
    Some(ConfigChoice {
        name: truncate(
            &wire::string_at(raw, "name").unwrap_or_else(|| value.clone()),
            MAX_NAME_CHARS,
        ),
        value,
        description: wire::str_at(raw, "description")
            .map(|text| truncate(text, MAX_DESCRIPTION_CHARS)),
        group,
    })
}

/// The first `max` characters of `text`.
pub fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => text[..end].to_owned(),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// claude-acp's shape: real options with categories, plus legacy modes it also sends.
    fn claude_reply() -> OpenReply {
        OpenReply::parse(&json!({
            "sessionId": "s",
            "modes": {"currentModeId": "default", "availableModes": [{"id": "default", "name": "Default"}]},
            "configOptions": [
                {"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "default",
                 "options": [{"value": "default", "name": "Default"}, {"value": "acceptEdits", "name": "Accept Edits"},
                             {"value": "bypassPermissions", "name": "Bypass Permissions"}]},
                {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "sonnet",
                 "options": [{"group": "anthropic", "name": "Anthropic", "options": [
                     {"value": "sonnet", "name": "Sonnet"}, {"value": "opus", "name": "Opus", "description": "Most capable"}]}]},
                {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "high",
                 "options": [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]},
                {"id": "fast", "name": "Fast mode", "category": "model_config", "type": "boolean", "currentValue": false},
                {"id": "agent", "name": "Agent", "type": "select", "currentValue": "default",
                 "options": [{"value": "default", "name": "Default"}]},
                {"id": "slider", "name": "Temperature", "type": "number", "currentValue": 0.5},
                {"name": "no id", "type": "boolean", "currentValue": true},
            ],
        }))
    }

    /// Hermes' shape: no config options, legacy modes and models.
    fn hermes_reply() -> OpenReply {
        OpenReply::parse(&json!({
            "sessionId": "h",
            "modes": {"currentModeId": "default", "availableModes": [
                {"id": "default", "name": "Default"}, {"id": "accept_edits", "name": "Accept Edits"},
                {"id": "dont_ask", "name": "Don't Ask"}]},
            "models": {"currentModelId": "openrouter:a", "availableModels": [
                {"modelId": "openrouter:a", "name": "A"}, {"modelId": "openrouter:b", "name": "B"}]},
        }))
    }

    #[test]
    fn config_options_win_and_map_kinds_and_categories() {
        let model = OptionModel::from_open(&claude_reply());
        let options = model.options();
        let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
        assert_eq!(
            ids,
            ["mode", "model", "effort", "fast", "agent"],
            "no synthesized mode"
        );
        let categories: Vec<ConfigCategory> =
            options.iter().map(|option| option.category).collect();
        assert_eq!(
            categories,
            [
                ConfigCategory::Mode,
                ConfigCategory::Model,
                ConfigCategory::Effort,
                ConfigCategory::ModelConfig,
                ConfigCategory::Other
            ]
        );
        let ConfigKind::Select { choices, current } = &options[1].kind else {
            panic!("model is a select");
        };
        assert_eq!(current, "sonnet");
        assert_eq!(choices.len(), 2, "grouped choices are flattened");
        assert_eq!(choices[1].group.as_deref(), Some("Anthropic"));
        assert_eq!(choices[1].description.as_deref(), Some("Most capable"));
        assert_eq!(options[3].kind, ConfigKind::Toggle { current: false });
        assert_eq!(model.current_mode(), Some("default"));
        assert!(model.offers_mode("bypassPermissions"));
        assert_eq!(model.mode_name("acceptEdits"), "Accept Edits");
        assert_eq!(
            model.request("s", "fast", &ConfigValue::Toggle { value: true }),
            Some((
                methods::SESSION_SET_CONFIG_OPTION,
                json!({"sessionId": "s", "configId": "fast", "type": "boolean", "value": true})
            ))
        );
    }

    /// Agents name their thinking level in several ways; each lands in Effort, so the picker shows
    /// it as Thinking instead of under More options. Look-alike names keep their own place.
    #[test]
    fn thinking_levels_are_effort_whatever_the_agent_calls_them() {
        let levels = json!([{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]);
        let category = |id: &str, category: Option<&str>, kind: &str| {
            let mut raw = if kind == "boolean" {
                json!({"id": id, "name": id, "type": "boolean", "currentValue": false})
            } else {
                json!({"id": id, "name": id, "type": "select", "currentValue": "low", "options": levels})
            };
            if let Some(category) = category {
                raw["category"] = json!(category);
            }
            parse_config_option(&raw).expect("parses").category
        };
        for named in [
            "effort",
            "reasoning",
            "reasoning_effort",
            "thinking",
            "thought_level",
            "Reasoning-Effort",
            "reasoningEffort",
        ] {
            assert_eq!(
                category("level", Some(named), "select"),
                ConfigCategory::Effort,
                "category {named}"
            );
            assert_eq!(
                category(named, None, "select"),
                ConfigCategory::Effort,
                "id {named}"
            );
            assert_eq!(
                category(named, Some(""), "select"),
                ConfigCategory::Effort,
                "id {named} with an empty category"
            );
        }
        // A category the agent chose wins over the id, and an id alone never makes a switch a
        // level; names that only resemble a level stay Other.
        assert_eq!(
            category("thinking", Some("model_config"), "select"),
            ConfigCategory::ModelConfig
        );
        assert_eq!(
            category("thinking", Some("mode"), "select"),
            ConfigCategory::Mode
        );
        assert_eq!(category("thinking", None, "boolean"), ConfigCategory::Other);
        assert_eq!(
            category("thinking", Some("thinking"), "boolean"),
            ConfigCategory::Effort
        );
        for other in [
            "reasoning_summary",
            "thinking_budget",
            "effort_mode",
            "verbosity",
            "model",
        ] {
            assert_eq!(
                category(other, None, "select"),
                ConfigCategory::Other,
                "{other}"
            );
        }
        assert_eq!(
            category("agent", Some("other"), "select"),
            ConfigCategory::Other
        );
    }

    #[test]
    fn legacy_modes_and_models_are_synthesized_with_their_routes() {
        let model = OptionModel::from_open(&hermes_reply());
        let options = model.options();
        assert_eq!(options.len(), 2);
        assert_eq!(
            (options[0].id.as_str(), options[0].category),
            ("mode", ConfigCategory::Mode)
        );
        assert_eq!(
            (options[1].id.as_str(), options[1].category),
            ("model", ConfigCategory::Model)
        );
        let choice = |value: &str| ConfigValue::Choice {
            value: value.into(),
        };
        assert_eq!(
            model.request("h", "mode", &choice("dont_ask")),
            Some((
                methods::SESSION_SET_MODE,
                json!({"sessionId": "h", "modeId": "dont_ask"})
            ))
        );
        assert_eq!(
            model.request("h", "model", &choice("openrouter:b")),
            Some((
                methods::SESSION_SET_MODEL,
                json!({"sessionId": "h", "modelId": "openrouter:b"})
            ))
        );
        assert_eq!(
            model.request("h", "mode", &ConfigValue::Toggle { value: true }),
            None
        );
        assert_eq!(model.request("h", "nope", &choice("x")), None);
    }

    #[test]
    fn validation_happens_without_the_agent() {
        let model = OptionModel::from_open(&claude_reply());
        let choice = |value: &str| ConfigValue::Choice {
            value: value.into(),
        };
        assert_eq!(model.validate("model", &choice("opus")), Ok(()));
        assert!(model.validate("model", &choice("gpt")).is_err());
        assert!(
            model
                .validate("model", &ConfigValue::Toggle { value: true })
                .is_err()
        );
        assert!(model.validate("fast", &choice("on")).is_err());
        assert_eq!(
            model.validate("fast", &ConfigValue::Toggle { value: true }),
            Ok(())
        );
        assert!(model.validate("missing", &choice("x")).is_err());
        assert!(model.is_current("effort", &choice("high")));
        assert!(!model.is_current("effort", &choice("low")));
    }

    #[test]
    fn replies_and_updates_replace_real_options_but_hermes_empty_replies_are_ignored() {
        let mut claude = OptionModel::from_open(&claude_reply());
        let choice = |value: &str| ConfigValue::Choice {
            value: value.into(),
        };
        // The reply's complete list replaces the real options.
        claude.applied(
            "model",
            &choice("opus"),
            Some(&[
                json!({"id": "model", "name": "Model", "category": "model", "type": "select",
                          "currentValue": "opus", "options": [{"value": "opus", "name": "Opus"}]}),
            ]),
        );
        assert_eq!(
            claude.options().len(),
            2,
            "the reply's list, plus the synthesized mode"
        );
        assert!(claude.get("effort").is_none());
        assert_eq!(
            claude.get("mode").map(|option| option.category),
            Some(ConfigCategory::Mode)
        );

        // Hermes answers `set_config_option` with `[]`: ignored, the value set locally.
        let mut hermes = OptionModel::from_open(&hermes_reply());
        hermes.applied("mode", &choice("dont_ask"), Some(&[]));
        assert_eq!(hermes.options().len(), 2);
        assert_eq!(hermes.current_mode(), Some("dont_ask"));

        // config_option_update replaces every real option; the synthesized ones stay.
        hermes.replace_real(&[
            json!({"id": "edit_approval_policy", "name": "Edits", "type": "select",
                                     "currentValue": "ask", "options": [{"value": "ask"}]}),
        ]);
        assert_eq!(hermes.options().len(), 3);
        assert_eq!(
            hermes.current_mode(),
            Some("dont_ask"),
            "the synthesized mode keeps its value"
        );
    }

    #[test]
    fn current_mode_updates_and_pending_changes() {
        let mut model = OptionModel::from_open(&hermes_reply());
        assert!(model.set_current_mode("accept_edits"));
        assert!(!model.set_current_mode("accept_edits"));
        assert_eq!(model.current_mode(), Some("accept_edits"));
        // A mode the agent did not list still shows.
        assert!(model.set_current_mode("plan"));
        assert!(model.offers_mode("plan"));
        model.set_pending(
            "mode",
            Some(ConfigValue::Choice {
                value: "default".into(),
            }),
        );
        assert_eq!(
            model.options()[0].pending,
            Some(ConfigValue::Choice {
                value: "default".into()
            })
        );
        model.replace_real(&[]);
        assert!(
            model.pending("mode").is_some(),
            "rebuilding keeps a pending change"
        );
        model.set_pending("mode", None);
        assert_eq!(model.pending("mode"), None);
        assert!(
            !OptionModel::default().set_current_mode("x"),
            "no Mode option"
        );
    }

    #[test]
    fn limits_apply_to_names_descriptions_choices_and_options() {
        let long = "n".repeat(250);
        let choices: Vec<Value> = (0..600)
            .map(|index| json!({"value": format!("v{index}"), "name": long, "description": "d".repeat(400)}))
            .collect();
        let raw: Vec<Value> = (0..120)
            .map(|index| {
                json!({"id": format!("o{index}"), "name": long, "type": "select",
                                "currentValue": "v0", "options": choices})
            })
            .collect();
        let reply = OpenReply {
            config_options: Some(raw),
            ..OpenReply::default()
        };
        let options = OptionModel::from_open(&reply).options();
        assert_eq!(options.len(), MAX_OPTIONS);
        assert_eq!(options[0].name.chars().count(), MAX_NAME_CHARS);
        let ConfigKind::Select { choices, .. } = &options[0].kind else {
            panic!("a select");
        };
        assert_eq!(choices.len(), MAX_CHOICES);
        assert_eq!(choices[0].name.chars().count(), MAX_NAME_CHARS);
        assert_eq!(
            choices[0]
                .description
                .as_deref()
                .map(|text| text.chars().count()),
            Some(MAX_DESCRIPTION_CHARS)
        );
        assert_eq!(truncate("héllo", 2), "hé");
        assert_eq!(truncate("hé", 5), "hé");
    }

    #[test]
    fn synthesized_ids_never_collide_with_real_ones() {
        let mut reply = hermes_reply();
        reply.config_options = Some(vec![
            json!({"id": "mode", "name": "Something else", "type": "boolean", "currentValue": true}),
        ]);
        let model = OptionModel::from_open(&reply);
        let ids: Vec<String> = model
            .options()
            .into_iter()
            .map(|option| option.id)
            .collect();
        assert_eq!(ids, ["mode", "legacy-mode", "model"]);
        assert_eq!(
            model.mode().map(|option| option.id.as_str()),
            Some("legacy-mode")
        );
    }

    #[test]
    fn value_labels() {
        let model = OptionModel::from_open(&claude_reply());
        let effort = model.get("effort").expect("effort");
        assert_eq!(
            value_label(
                effort,
                &ConfigValue::Choice {
                    value: "low".into()
                }
            ),
            "Low"
        );
        assert_eq!(
            value_label(
                effort,
                &ConfigValue::Choice {
                    value: "max".into()
                }
            ),
            "max"
        );
        let fast = model.get("fast").expect("fast");
        assert_eq!(
            value_label(fast, &ConfigValue::Toggle { value: true }),
            "on"
        );
        assert_eq!(current_value(fast), ConfigValue::Toggle { value: false });
    }
}

#[cfg(test)]
mod native_control_tests {
    use super::*;
    use serde_json::json;
    fn model(config: bool) -> OptionModel {
        let raw = if config {
            json!({"configOptions":[{"id":"permission","name":"Mode","category":"mode","type":"select","currentValue":"acceptEdits","options":[{"value":"acceptEdits","name":"Accept edits"},{"value":"plan","name":"Plan"},{"value":"future","name":"Unknown"}]}]})
        } else {
            json!({"modes":{"currentModeId":"acceptEdits","availableModes":[{"id":"acceptEdits","name":"Accept edits"},{"id":"plan","name":"Plan"}]}})
        };
        let mut model = OptionModel::from_open(&OpenReply::parse(&raw));
        model.enable_native_controls("claude-acp");
        model
    }
    #[test]
    fn native_plan_keeps_config_and_legacy_routes_and_remembers_execution_access() {
        for config in [true, false] {
            let mut model = model(config);
            let plan = ConfigValue::Choice {
                value: "plan".into(),
            };
            let execute = ConfigValue::Choice {
                value: "execute".into(),
            };
            let (method, params) = model
                .request("s", crate::controls::WORKFLOW, &plan)
                .unwrap();
            assert_eq!(
                method,
                if config {
                    "session/set_config_option"
                } else {
                    "session/set_mode"
                }
            );
            assert_eq!(params[if config { "value" } else { "modeId" }], "plan");
            model.applied(crate::controls::WORKFLOW, &plan, None);
            let (_, params) = model
                .request("s", crate::controls::WORKFLOW, &execute)
                .unwrap();
            assert_eq!(
                params[if config { "value" } else { "modeId" }],
                "acceptEdits"
            );
            assert!(
                model
                    .validate(
                        crate::controls::WORKFLOW,
                        &ConfigValue::Choice {
                            value: "future".into()
                        }
                    )
                    .is_err()
            );
        }
    }
    #[test]
    fn unclassified_generic_modes_do_not_claim_plan_semantics() {
        let mut model = model(false);
        model.enable_native_controls("generic");
        assert!(model.get(crate::controls::WORKFLOW).is_none());
        assert!(model.mode().is_some());
    }
}
