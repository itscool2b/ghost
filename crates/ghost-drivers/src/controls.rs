//! Native control option identities shared by the adapters and session actor.
use ghost_protocol::session::{ConfigCategory, ConfigChoice, ConfigKind, ConfigOption};
pub const ACCESS: &str = "aura.access";
pub const WORKFLOW: &str = "aura.workflow";
/// Whether a native access value is an agent's widest level, the one that never asks: Claude
/// Code's `bypassPermissions` (its ACP adapter's too) and Codex's ACP full access, under either
/// of the two ids its adapter has used.
pub fn full_access(value: &str) -> bool {
    matches!(
        value,
        "bypassPermissions" | "full-access" | "agent-full-access"
    )
}
pub fn select(id: &str, name: &str, current: &str, choices: Vec<ConfigChoice>) -> ConfigOption {
    ConfigOption {
        id: id.into(),
        name: name.into(),
        description: None,
        category: ConfigCategory::Other,
        kind: ConfigKind::Select {
            current: current.into(),
            choices,
        },
        pending: None,
    }
}
pub fn choice(value: &str, name: &str) -> ConfigChoice {
    ConfigChoice {
        value: value.into(),
        name: name.into(),
        description: None,
        group: None,
    }
}
pub fn workflow(current: &str) -> ConfigOption {
    let mut execute = choice("execute", "Execute");
    execute.description = Some("Run with the remembered native access setting. Leaving Plan requires implementation approval.".into());
    let mut plan = choice("plan", "Plan");
    plan.description = Some("Use the harness native planning workflow. Access remains separate; implementation requires your approval.".into());
    select(WORKFLOW, "Workflow", current, vec![execute, plan])
}
