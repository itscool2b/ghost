use super::wire::Model;
use ghost_protocol::session::{
    ConfigCategory, ConfigChoice, ConfigKind, ConfigOption, ConfigValue,
};
use ghost_protocol::{AutoAnswer, Autonomy, AutonomyState};
use serde_json::{Value, json};

pub(super) fn policies(autonomy: Autonomy) -> (&'static str, &'static str, Value) {
    match autonomy {
        Autonomy::FullAuto => (
            "never",
            "danger-full-access",
            json!({"type":"dangerFullAccess"}),
        ),
        Autonomy::AutoEdit => (
            "on-request",
            "workspace-write",
            json!({"type":"workspaceWrite"}),
        ),
        Autonomy::Ask => ("untrusted", "read-only", json!({"type":"readOnly"})),
    }
}

pub(super) fn autonomy(requested: Autonomy) -> AutonomyState {
    AutonomyState {
        requested,
        native_mode: Some(policies(requested).0.into()),
        auto_answer: AutoAnswer::Never,
        note: Some("Applied to each new turn; Codex restrictions still apply.".into()),
    }
}

pub(super) fn options(models: &[Model], model: &str, effort: Option<&str>) -> Vec<ConfigOption> {
    if models.is_empty() {
        return Vec::new();
    }
    let mut choices: Vec<_> = models
        .iter()
        .map(|m| ConfigChoice {
            value: m.model.clone(),
            name: m.display_name.clone(),
            description: Some(m.description.clone()),
            group: None,
        })
        .collect();
    if !choices.iter().any(|choice| choice.value == model) {
        choices.push(ConfigChoice {
            value: model.into(),
            name: model.into(),
            description: None,
            group: None,
        });
    }
    let mut result = vec![ConfigOption {
        id: "model".into(),
        name: "Model".into(),
        description: Some("Model for the next turn.".into()),
        category: ConfigCategory::Model,
        kind: ConfigKind::Select {
            current: model.into(),
            choices,
        },
        pending: None,
    }];
    if let Some(selected) = models.iter().find(|m| m.model == model) {
        let mut choices = vec![ConfigChoice {
            value: "default".into(),
            name: "Default".into(),
            description: Some(format!("Use {}", selected.default_reasoning_effort)),
            group: None,
        }];
        choices.extend(
            selected
                .supported_reasoning_efforts
                .iter()
                .map(|level| ConfigChoice {
                    value: level.reasoning_effort.clone(),
                    name: level.reasoning_effort.clone(),
                    description: Some(level.description.clone()),
                    group: None,
                }),
        );
        result.push(ConfigOption {
            id: "effort".into(),
            name: "Effort".into(),
            description: Some("Reasoning effort for the next turn.".into()),
            category: ConfigCategory::Effort,
            kind: ConfigKind::Select {
                current: effort.unwrap_or("default").into(),
                choices,
            },
            pending: None,
        });
    }
    result
}

pub(super) fn validate(models: &[Model], model: &str, id: &str, value: &ConfigValue) -> bool {
    let ConfigValue::Choice { value } = value else {
        return false;
    };
    match id {
        "model" => models.iter().any(|m| m.model == *value),
        "effort" => models.iter().find(|m| m.model == model).is_some_and(|m| {
            value == "default"
                || m.supported_reasoning_efforts
                    .iter()
                    .any(|e| e.reasoning_effort == *value)
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_does_not_pregrant_file_writes_and_vendor_callbacks_always_reach_the_user() {
        assert_eq!(
            policies(Autonomy::Ask),
            ("untrusted", "read-only", json!({"type":"readOnly"}))
        );
        assert_eq!(
            policies(Autonomy::AutoEdit),
            (
                "on-request",
                "workspace-write",
                json!({"type":"workspaceWrite"})
            )
        );
        for level in [Autonomy::Ask, Autonomy::AutoEdit, Autonomy::FullAuto] {
            assert_eq!(autonomy(level).auto_answer, AutoAnswer::Never);
        }
    }
}
