//! `ghost-cli agents`: the environment agents start with (how it was resolved, and the variable
//! names the scrub removes from it), then every agent and whether it can start here. Detection
//! waits for the env and every probe, so nothing reads `checking`; it never installs anything.

use ghost_protocol::session::{AgentEntry, Availability, EnvState, EnvStatus};

use crate::CliError;
use crate::headless::Headless;
use crate::out::Out;
use crate::report::Outcome;

pub async fn run(headless: &Headless, out: &Out) -> Result<Outcome, CliError> {
    // Every connection of every agent, so none reads `checking`. Every detection shares the env
    // resolution and the probes in flight.
    for connection in ghost_agents::catalog() {
        if let Some(spec) = headless.detector.spec(connection.id) {
            headless.detector.detect(&spec).await;
        }
    }
    let agents = headless.manager.agents(false).await;
    for line in render(&headless.manager.env_status(), &agents) {
        out.line(line);
    }
    Ok(Outcome::Passed)
}

/// The listing, line by line.
pub fn render(env: &EnvStatus, agents: &[AgentEntry]) -> Vec<String> {
    let mut lines = vec![
        format!("Environment: {}", env_text(&env.state)),
        format!(
            "Removed by the scrub: {}",
            if env.removed.is_empty() {
                "nothing".to_owned()
            } else {
                env.removed.join(", ")
            }
        ),
        String::new(),
    ];
    for agent in agents {
        lines.push(format!("{} ({})", agent.id, agent.name));
        let state = match &agent.availability {
            Availability::Checking => "checking".to_owned(),
            Availability::Ready { version, path } => {
                let mut text = String::from("ready");
                if let Some(version) = version {
                    text.push_str(&format!(": {version}"));
                }
                text.push_str(&format!(" ({path})"));
                if agent.needs_download {
                    text.push_str("; the first start downloads its adapter");
                }
                text
            }
            Availability::Missing { reason } => format!("missing: {reason}"),
            Availability::Broken { reason } => format!("broken: {reason}"),
        };
        lines.push(format!("  {state}"));
        if let Some(connections) = &agent.connections {
            let label = |id: &Option<String>| {
                id.as_ref().and_then(|id| {
                    connections
                        .options
                        .iter()
                        .find(|option| &option.id == id)
                        .map(|option| option.label.clone())
                })
            };
            lines.push(format!(
                "  connection: {}; new sessions use {}",
                label(&connections.preferred).unwrap_or_else(|| "Automatic".into()),
                label(&connections.active).unwrap_or_else(|| "none (none can run here)".into())
            ));
        }
        lines.push(format!("  starts as: {}", agent.launch));
        lines.push(format!("  last verified with: {}", agent.last_verified));
    }
    lines
}

fn env_text(state: &EnvState) -> String {
    match state {
        EnvState::Resolving => "still resolving".to_owned(),
        EnvState::Ready {
            source,
            took_ms,
            vars,
        } => format!("{source}, {vars} variables, resolved in {took_ms} ms"),
        EnvState::Fallback { reason } => format!("fallback, because {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_says_how_each_agent_stands() {
        let env = EnvStatus {
            state: EnvState::Ready {
                source: "zsh login shell".into(),
                took_ms: 812,
                vars: 64,
            },
            removed: vec!["CLAUDECODE".into(), "CLAUDE_CODE_ENTRYPOINT".into()],
        };
        let entry = |id: &str, availability: Availability, needs_download: bool| AgentEntry {
            id: id.into(),
            name: format!("{id} name"),
            description: String::new(),
            availability,
            launch: format!("{id} acp"),
            needs_download,
            last_verified: "1.0.0".into(),
            installation: None,
            latest_release: None,
            harness: None,
            connections: None,
            terminal: false,
        };
        let option = |id: &str, label: &str| ghost_protocol::session::AgentConnection {
            id: id.into(),
            label: label.into(),
            description: String::new(),
            availability: Availability::Checking,
            launch: String::new(),
            needs_download: false,
        };
        let claude = AgentEntry {
            connections: Some(ghost_protocol::session::AgentConnections {
                preferred: None,
                active: Some("claude-acp".into()),
                options: vec![
                    option("claude", "Direct"),
                    option("claude-acp", "ACP adapter"),
                ],
            }),
            ..entry(
                "claude",
                Availability::Ready {
                    version: Some("Claude Code 2.1.310 via claude-agent-acp 0.95.0".into()),
                    path: "/u/.local/bin/claude".into(),
                },
                true,
            )
        };
        let codex = AgentEntry {
            connections: Some(ghost_protocol::session::AgentConnections {
                preferred: Some("codex-acp".into()),
                active: None,
                options: vec![
                    option("codex", "Direct"),
                    option("codex-acp", "ACP adapter"),
                ],
            }),
            ..entry(
                "codex",
                Availability::Missing {
                    reason: "Node.js 22 or newer is required (found 20.11.0).".into(),
                },
                false,
            )
        };
        let lines = render(
            &env,
            &[
                claude,
                codex,
                entry(
                    "hermes",
                    Availability::Broken {
                        reason: "hermes --version failed".into(),
                    },
                    false,
                ),
            ],
        );
        assert_eq!(
            lines[0],
            "Environment: zsh login shell, 64 variables, resolved in 812 ms"
        );
        assert_eq!(
            lines[1],
            "Removed by the scrub: CLAUDECODE, CLAUDE_CODE_ENTRYPOINT"
        );
        assert!(lines.contains(&"  ready: Claude Code 2.1.310 via claude-agent-acp 0.95.0 (/u/.local/bin/claude); the first start downloads its adapter".to_owned()));
        assert!(
            lines.contains(
                &"  missing: Node.js 22 or newer is required (found 20.11.0).".to_owned()
            )
        );
        assert!(lines.contains(&"  broken: hermes --version failed".to_owned()));
        // One entry per agent, with the connection new sessions use.
        assert!(lines.contains(&"claude (claude name)".to_owned()));
        assert!(
            lines.contains(&"  connection: Automatic; new sessions use ACP adapter".to_owned())
        );
        assert!(lines.contains(
            &"  connection: ACP adapter; new sessions use none (none can run here)".to_owned()
        ));
        assert!(
            !lines
                .iter()
                .any(|line| line.starts_with("hermes") && line.contains("connection")),
            "one connection: no line"
        );
        assert!(lines.contains(&"  starts as: hermes acp".to_owned()));
        let fallback = render(
            &EnvStatus {
                state: EnvState::Fallback {
                    reason: "the login shell timed out".into(),
                },
                removed: Vec::new(),
            },
            &[],
        );
        assert_eq!(
            fallback[0],
            "Environment: fallback, because the login shell timed out"
        );
        assert_eq!(fallback[1], "Removed by the scrub: nothing");
    }
}
