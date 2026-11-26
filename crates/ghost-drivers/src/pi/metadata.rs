//! Only display fields cross from native model/configuration responses into the product.
use super::wire::text;
use crate::acp::{
    map::{hex, reduce_commands},
    options::truncate,
    wire::CommandEntry,
};
use ghost_protocol::session::{
    ConfigCategory, ConfigChoice, ConfigKind, ConfigOption, ConfigValue,
};
use ghost_protocol::{Cost, SlashCommand, TokenUsage, UsageInfo, UsageScope};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAX_MODELS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub provider: String,
    pub id: String,
    pub name: String,
}
impl Model {
    pub fn parse(value: &Value) -> Option<Self> {
        let provider = text(value, "provider")?;
        let id = text(value, "id")?;
        if provider.len() > 1024 || id.len() > 1024 {
            return None;
        }
        Some(Self {
            provider: provider.into(),
            id: id.into(),
            name: truncate(text(value, "name").unwrap_or(id), 100),
        })
    }
    pub fn key(&self) -> String {
        // Separating with a JSON tuple avoids collisions when provider/model contain slashes.
        format!(
            "pi-model-{}",
            hex(&Sha256::digest(json!([self.provider, self.id]).to_string()))
        )
    }
}

#[derive(Debug, Default)]
pub struct Metadata {
    pub models: BTreeMap<String, Model>,
    pub model: Option<Model>,
    pub level: Option<String>,
    pub levels: Vec<String>,
}
impl Metadata {
    pub fn models(&mut self, data: &Value) -> bool {
        let Some(models) = data["models"].as_array() else {
            return false;
        };
        self.models = models
            .iter()
            .filter_map(Model::parse)
            .take(MAX_MODELS)
            .map(|m| (m.key(), m))
            .collect();
        true
    }
    pub fn levels(&mut self, data: &Value) -> bool {
        let Some(levels) = data["levels"].as_array() else {
            return false;
        };
        let mut seen = std::collections::BTreeSet::new();
        self.levels = levels
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .filter(|s| seen.insert((*s).to_owned()))
            .take(100)
            .map(str::to_owned)
            .collect();
        true
    }
    pub fn state(&mut self, state: &Value) {
        self.model = Model::parse(&state["model"]);
        self.level = text(state, "thinkingLevel")
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .map(str::to_owned);
    }
    pub fn options(&self) -> Vec<ConfigOption> {
        let mut options = Vec::new();
        if let Some(current) = &self.model {
            let mut models = self.models.clone();
            models.insert(current.key(), current.clone());
            options.push(select(
                "model",
                "Model",
                ConfigCategory::Model,
                current.key(),
                models
                    .into_iter()
                    .map(|(key, model)| ConfigChoice {
                        value: key,
                        name: model.name,
                        description: None,
                        group: Some(truncate(&model.provider, 100)),
                    })
                    .collect(),
            ));
        }
        if let Some(current) = &self.level {
            let mut levels = self.levels.clone();
            if !levels.contains(current) {
                levels.push(current.clone());
            }
            options.push(select(
                "effort",
                "Thinking",
                ConfigCategory::Effort,
                current.clone(),
                levels
                    .into_iter()
                    .map(|level| ConfigChoice {
                        name: level.clone(),
                        value: level,
                        description: None,
                        group: None,
                    })
                    .collect(),
            ));
        }
        options
    }
    pub fn request(&self, id: &str, value: &ConfigValue) -> Option<Value> {
        match (id, value) {
            ("model", ConfigValue::Choice { value }) => {
                let model = self
                    .models
                    .get(value)
                    .or_else(|| self.model.as_ref().filter(|m| m.key() == *value))?;
                Some(json!({"type":"set_model","provider":model.provider,"modelId":model.id}))
            }
            ("effort", ConfigValue::Choice { value }) if self.levels.contains(value) => {
                Some(json!({"type":"set_thinking_level","level":value}))
            }
            _ => None,
        }
    }
}
fn select(
    id: &str,
    name: &str,
    category: ConfigCategory,
    current: String,
    choices: Vec<ConfigChoice>,
) -> ConfigOption {
    ConfigOption {
        id: id.into(),
        name: name.into(),
        description: None,
        category,
        kind: ConfigKind::Select { current, choices },
        pending: None,
    }
}

pub fn commands(value: &Value) -> Vec<SlashCommand> {
    reduce_commands(
        value["commands"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                Some(CommandEntry {
                    name: text(c, "name")?.into(),
                    description: c["description"].as_str().unwrap_or_default().into(),
                    hint: None,
                })
            })
            .collect(),
    )
}

/// The skills among Pi's commands (`source: "skill"`), named without their `skill:` prefix.
pub fn skills(value: &Value) -> Vec<String> {
    value["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|command| command["source"] == "skill")
        .filter_map(|command| text(command, "name"))
        .map(|name| name.strip_prefix("skill:").unwrap_or(name).to_owned())
        .collect()
}

/// Context occupancy may explicitly become unknown after compaction. Do not substitute cumulative
/// session token counts for occupancy or preserve an outdated pre-compaction count.
pub fn usage(value: &Value) -> UsageInfo {
    UsageInfo {
        context_used: value["contextUsage"]["tokens"].as_u64(),
        context_size: value["contextUsage"]["contextWindow"]
            .as_u64()
            .filter(|n| *n > 0),
        cost: value["cost"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .map(|amount| Cost {
                amount,
                currency: "USD".into(),
            }),
    }
}
pub fn tokens(value: &Value, scope: UsageScope) -> Option<TokenUsage> {
    Some(TokenUsage {
        input_tokens: value["input"].as_u64()?,
        output_tokens: value["output"].as_u64()?,
        cache_read_tokens: value["cacheRead"].as_u64(),
        cache_write_tokens: value["cacheWrite"].as_u64(),
        reasoning_tokens: None,
        scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_are_the_commands_pi_marks_as_skills() {
        let commands = serde_json::json!({"commands": [
            {"name": "skill:deploy", "source": "skill"},
            {"name": "fix-tests", "source": "prompt"},
            {"name": "review", "source": "extension"}
        ]});
        assert_eq!(skills(&commands), ["deploy"]);
    }
    #[test]
    fn model_choices_keep_current_valid_and_never_export_auth_or_source_fields() {
        let mut m = Metadata::default();
        m.models(&json!({"models":[{"id":"m","provider":"p","name":"Model","apiKey":"secret","baseUrl":"private"}]}));
        m.state(&json!({"model":{"provider":"custom","id":"current"},"thinkingLevel":"off","autoCompactionEnabled":true}));
        for option in m.options() {
            if let ConfigKind::Select { current, choices } = option.kind {
                assert!(choices.iter().any(|c| c.value == current));
            }
        }
        let rendered = serde_json::to_string(&m.options()).unwrap();
        assert!(!rendered.contains("secret") && !rendered.contains("private"));
        let key = m.models.keys().next().unwrap().clone();
        assert_eq!(
            m.request("model", &ConfigValue::Choice { value: key }),
            Some(json!({"type":"set_model","provider":"p","modelId":"m"}))
        );
        assert!(
            m.request(
                "effort",
                &ConfigValue::Choice {
                    value: "high".into()
                }
            )
            .is_none()
        );
    }
    #[test]
    fn context_is_occupancy_and_null_clears_it_without_inventing_tokens() {
        let data = json!({"tokens":{"input":9000,"output":20,"cacheRead":10,"cacheWrite":4},
            "cost":0.02,"contextUsage":{"tokens":null,"contextWindow":100000}});
        assert_eq!(usage(&data).context_used, None);
        assert_eq!(usage(&data).context_size, Some(100000));
        assert_eq!(
            tokens(&data["tokens"], UsageScope::Session).unwrap().scope,
            UsageScope::Session
        );
    }

    #[test]
    fn max_thinking_is_valid_but_global_compaction_settings_are_never_offered() {
        let mut metadata = Metadata::default();
        metadata.state(&json!({"thinkingLevel":"max","autoCompactionEnabled":true}));
        metadata.levels(&json!({"levels":["off","high","max"]}));
        assert_eq!(
            metadata.request(
                "effort",
                &ConfigValue::Choice {
                    value: "max".into()
                }
            ),
            Some(json!({"type":"set_thinking_level","level":"max"}))
        );
        assert!(
            metadata
                .options()
                .iter()
                .all(|option| option.id != "auto_compaction")
        );
        assert!(
            metadata
                .request("auto_compaction", &ConfigValue::Toggle { value: false })
                .is_none()
        );
    }
}

#[cfg(test)]
mod future_effort_tests {
    use super::*;
    #[test]
    fn future_advertised_effort_reaches_the_native_request() {
        let mut metadata = Metadata::default();
        metadata.levels(&json!({"levels":["future-effort", "high"]}));
        metadata.state(&json!({"thinkingLevel":"future-effort"}));
        assert_eq!(
            metadata.request(
                "effort",
                &ConfigValue::Choice {
                    value: "future-effort".into()
                }
            ),
            Some(json!({"type":"set_thinking_level","level":"future-effort"}))
        );
        assert!(
            matches!(&metadata.options()[0].kind, ConfigKind::Select {current,choices} if current == "future-effort" && choices.iter().any(|c|c.value=="future-effort"))
        );
    }
}
