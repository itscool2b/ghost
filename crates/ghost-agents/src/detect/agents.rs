//! One entry per agent. The catalog's connections of one agent (Claude Code's own protocol and its
//! ACP adapter, say) are shown and chosen as that agent:
//!
//! - **Automatic** (the default): a new session uses the first connection, in catalog order, that
//!   can run here. While the preferred one is still being checked, the agent reads `Checking`.
//! - **An explicit connection** (the agent's Connection setting on this device): new sessions use
//!   it, and the agent shows its availability.
//! - **None can run**: the agent shows its preferred connection's reason, followed by each other
//!   connection's (named by its label) when either is installed but unusable (`Broken`), so the
//!   reason says what is missing for every way in.
//!
//! A chat's saved native session always resumes through the connection that made it; the session
//! manager asks for a choice only for a new session.
use super::*;
use crate::catalog::agent_of;
use ghost_protocol::session::{AgentConnection, AgentConnections};

/// Which connection an agent uses, from its connections' availabilities in catalog order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    /// The connection a new session uses now.
    pub(crate) active: Option<usize>,
    /// The connection whose details the agent's entry shows: the active one, the explicit choice,
    /// the one still being checked, or the preferred one when none can run.
    pub(crate) shown: usize,
    pub(crate) availability: Availability,
}

/// The selection rule (see the module comment). `connections` holds each connection's label and
/// availability in catalog order; `preferred` is the index of an explicit choice. Never empty.
pub(crate) fn choose(preferred: Option<usize>, connections: &[(&str, &Availability)]) -> Choice {
    if let Some(index) = preferred.filter(|index| *index < connections.len()) {
        let availability = connections[index].1.clone();
        return Choice {
            active: matches!(availability, Availability::Ready { .. }).then_some(index),
            shown: index,
            availability,
        };
    }
    for (index, (_, availability)) in connections.iter().enumerate() {
        match availability {
            Availability::Ready { .. } => {
                return Choice {
                    active: Some(index),
                    shown: index,
                    availability: (*availability).clone(),
                };
            }
            Availability::Checking => {
                return Choice {
                    active: None,
                    shown: index,
                    availability: Availability::Checking,
                };
            }
            Availability::Missing { .. } | Availability::Broken { .. } => {}
        }
    }
    Choice {
        active: None,
        shown: 0,
        availability: unavailable_everywhere(connections),
    }
}

fn reason(availability: &Availability) -> &str {
    match availability {
        Availability::Missing { reason } | Availability::Broken { reason } => reason,
        Availability::Checking | Availability::Ready { .. } => "",
    }
}

/// Why none of an agent's connections can run: the preferred one's reason, then each other one's
/// when either is installed but unusable.
fn unavailable_everywhere(connections: &[(&str, &Availability)]) -> Availability {
    let (_, first) = connections[0];
    let first_broken = matches!(first, Availability::Broken { .. });
    let mut text = reason(first).to_owned();
    for (label, availability) in &connections[1..] {
        let broken = matches!(availability, Availability::Broken { .. });
        let other = reason(availability);
        if (first_broken || broken) && !other.is_empty() && other != reason(first) {
            text.push_str(&format!(" {label}: {other}"));
        }
    }
    if first_broken {
        Availability::Broken { reason: text }
    } else {
        Availability::Missing { reason: text }
    }
}

impl Detector {
    /// Every connection this detector knows, in order.
    pub(super) fn specs(&self) -> Vec<&AgentSpec> {
        match &self.mode {
            Mode::Live(live) => live.specs.iter().collect(),
            Mode::Fixed { agents } => agents.iter().map(|fixed| &fixed.agent.spec).collect(),
        }
    }

    /// The connections of the agent `id` names (its own id or one of its connections'), preferred
    /// first. The catalog says which agent a connection belongs to, so a test double keeps its
    /// catalog agent and an id the catalog does not know is an agent of its own.
    pub(super) fn group(&self, id: &str) -> Vec<&AgentSpec> {
        let agent = agent_of(id);
        self.specs()
            .into_iter()
            .filter(|spec| agent_of(spec.id) == agent)
            .collect()
    }

    /// The agent `id` names, as its preferred connection this detector knows. Chats, projects and
    /// defaults name agents; `None` for an agent this detector does not know.
    pub fn agent(&self, id: &str) -> Option<AgentSpec> {
        let agent = agent_of(id);
        let group = self.group(id);
        group
            .iter()
            .find(|spec| spec.id == agent)
            .or(group.first())
            .map(|spec| (*spec).clone())
    }

    /// The agent's explicit connection on this device: its index in `group`.
    fn preferred(&self, agent: &str, group: &[&AgentSpec]) -> Option<usize> {
        let choices = lock(&self.connection_choices);
        let chosen = choices.get(agent)?;
        group.iter().position(|spec| spec.id == chosen)
    }

    /// The connection `id` stands for now, from the last detections, without waiting: a connection
    /// id other than an agent's own names that connection, and an agent's id names the connection
    /// its entry shows (the one a new session would use, or the preferred one while none can run).
    /// Actions on an agent (an update, a sign-in check, an executable choice, the Plugin library's
    /// listing) act on it, and a chat with no saved session shows it until its start chooses.
    pub fn connection_for(&self, id: &str) -> Option<AgentSpec> {
        if agent_of(id) != id {
            return self.spec(id);
        }
        let group = self.group(id);
        if group.len() < 2 {
            return group.first().map(|spec| (*spec).clone());
        }
        let entries: Vec<AgentEntry> = group
            .iter()
            .map(|spec| self.connection_entry(spec))
            .collect();
        let shown = self.choice(id, &group, &entries).shown;
        Some(group[shown].clone())
    }

    fn choice(&self, agent: &str, group: &[&AgentSpec], entries: &[AgentEntry]) -> Choice {
        let connections: Vec<(&str, &Availability)> = group
            .iter()
            .zip(entries)
            .map(|(spec, entry)| (spec.connection, &entry.availability))
            .collect();
        choose(self.preferred(agent, group), &connections)
    }

    /// One connection's own entry, from its last detection.
    fn connection_entry(&self, spec: &AgentSpec) -> AgentEntry {
        match &self.mode {
            Mode::Live(live) => live.entry(spec),
            Mode::Fixed { agents } => find(agents, spec.id)
                .map(Fixed::entry)
                .unwrap_or_else(|| entry(spec, Availability::Checking, false)),
        }
    }

    /// Every agent, one entry each, in catalog order, from the last detections.
    pub(super) fn agent_entries(&self) -> Vec<AgentEntry> {
        let mut seen: Vec<&str> = Vec::new();
        let mut entries = Vec::new();
        for spec in self.specs() {
            let agent = agent_of(spec.id);
            if seen.contains(&agent) {
                continue;
            }
            seen.push(agent);
            entries.push(self.agent_entry(agent));
        }
        entries
    }

    fn agent_entry(&self, agent: &str) -> AgentEntry {
        let group = self.group(agent);
        let own: Vec<AgentEntry> = group
            .iter()
            .map(|spec| self.connection_entry(spec))
            .collect();
        let name = self
            .agent(agent)
            .map_or_else(|| agent.to_owned(), |spec| spec.name.to_owned());
        if group.len() < 2 {
            let mut entry = own
                .into_iter()
                .next()
                .unwrap_or_else(|| entry(group[0], Availability::Checking, false));
            entry.id = agent.to_owned();
            entry.name = name;
            return entry;
        }
        let choice = self.choice(agent, &group, &own);
        let preferred = self
            .preferred(agent, &group)
            .map(|index| group[index].id.to_owned());
        let options = group
            .iter()
            .zip(&own)
            .map(|(spec, own)| AgentConnection {
                id: spec.id.to_owned(),
                label: spec.connection.to_owned(),
                description: own.description.clone(),
                availability: own.availability.clone(),
                launch: own.launch.clone(),
                needs_download: own.needs_download,
            })
            .collect();
        let mut entry = own[choice.shown].clone();
        entry.id = agent.to_owned();
        entry.name = name;
        entry.availability = choice.availability;
        entry.connections = Some(AgentConnections {
            preferred,
            active: choice.active.map(|index| group[index].id.to_owned()),
            options,
        });
        entry
    }

    /// The connection a new session of the agent `id` names starts with: its explicit connection,
    /// else the first that can run here, each detected in turn (sharing the probes in flight).
    /// Fails with why none can run.
    pub async fn choose_connection(&self, id: &str) -> Result<AgentSpec, LaunchError> {
        let group: Vec<AgentSpec> = self.group(id).into_iter().cloned().collect();
        let Some(first) = group.first() else {
            return Err(LaunchError::Missing {
                reason: format!("Ghost doesn't know an agent called \"{id}\"."),
            });
        };
        if group.len() == 1 {
            return Ok(first.clone());
        }
        let refs: Vec<&AgentSpec> = group.iter().collect();
        if let Some(index) = self.preferred(agent_of(id), &refs) {
            return Ok(group[index].clone());
        }
        let mut found = Vec::new();
        for spec in &group {
            let availability = self.detect(spec).await;
            if matches!(
                availability,
                Availability::Ready { .. } | Availability::Checking
            ) {
                return Ok(spec.clone());
            }
            found.push(availability);
        }
        let connections: Vec<(&str, &Availability)> = group
            .iter()
            .zip(&found)
            .map(|(spec, availability)| (spec.connection, availability))
            .collect();
        Err(unavailable(&unavailable_everywhere(&connections)))
    }

    /// The agent's Connection setting on this device: `None` is Automatic, else one of its
    /// connection ids. Applies to new sessions only: a saved session keeps its connection. Saved
    /// in the data root when this detector persists preferences.
    pub async fn set_connection(&self, id: &str, connection: Option<&str>) -> Result<(), String> {
        let agent = agent_of(id).to_owned();
        let group = self.group(&agent);
        if group.len() < 2 {
            return Err("This agent has one connection.".into());
        }
        if let Some(connection) = connection
            && !group.iter().any(|spec| spec.id == connection)
        {
            return Err("This connection does not belong to the agent.".into());
        }
        let saved = match &self.connection_path {
            Some(path) => {
                let path = path.clone();
                let connection = connection.map(str::to_owned);
                let agent = agent.clone();
                tokio::task::spawn_blocking(move || {
                    crate::connection_choices::save(&path, &agent, connection.as_deref())
                })
                .await
                .map_err(|_| "The connection choice could not be saved.".to_owned())??
            }
            None => {
                let mut choices = lock(&self.connection_choices).clone();
                match connection {
                    Some(connection) => {
                        choices.insert(agent, connection.to_owned());
                    }
                    None => {
                        choices.remove(&agent);
                    }
                }
                choices
            }
        };
        *lock(&self.connection_choices) = saved;
        Ok(())
    }

    /// Reads the saved Connection settings again (another window may have changed them). An
    /// unreadable file leaves every agent Automatic.
    pub(super) async fn reload_connections(&self) {
        let Some(path) = self.connection_path.clone() else {
            return;
        };
        let choices = tokio::task::spawn_blocking(move || crate::connection_choices::read(&path))
            .await
            .unwrap_or_else(|_| Err("The connection choices could not be read.".into()));
        *lock(&self.connection_choices) = choices.unwrap_or_else(|error| {
            tracing::warn!(%error, "connection choices unreadable; every agent uses Automatic");
            Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::get;

    fn fixed(id: &str, availability: Availability) -> FixedAgent {
        FixedAgent::new(get(id).expect("catalog").clone(), availability, None)
    }

    fn ids(entries: &[AgentEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.id.as_str()).collect()
    }

    #[tokio::test]
    async fn claude_is_one_agent_that_uses_native_when_it_runs_and_the_adapter_otherwise() {
        let both = Detector::fixed(vec![
            fixed("hermes", missing("hermes was not found on your PATH.")),
            fixed("claude", ready()),
            fixed("claude-acp", ready()).with_download(Vec::new()),
            fixed("mock", ready()),
        ]);
        let entries = both.cached();
        assert_eq!(ids(&entries), ["hermes", "claude", "mock"]);
        let claude = &entries[1];
        assert_eq!(claude.name, "Claude Code");
        assert_eq!(claude.availability, ready());
        assert!(!claude.needs_download, "native needs no download");
        let connections = claude.connections.as_ref().expect("two connections");
        assert_eq!(connections.active.as_deref(), Some("claude"));
        assert_eq!(connections.preferred, None);
        assert!(connections.options[1].needs_download);
        assert_eq!(
            both.choose_connection("claude").await.map(|spec| spec.id),
            Ok("claude")
        );
        // A stored adapter id names the same agent.
        assert_eq!(both.agent("claude-acp").map(|spec| spec.id), Some("claude"));
        assert_eq!(
            both.connection_for("claude").map(|spec| spec.id),
            Some("claude")
        );
        assert_eq!(
            both.connection_for("claude-acp").map(|spec| spec.id),
            Some("claude-acp")
        );

        // Claude Code is too old for the native driver: the adapter runs it, with its download.
        let old = Detector::fixed(vec![
            fixed(
                "claude",
                broken("Ghost needs Claude Code 2.1.310 or newer (found 2.1.309)."),
            ),
            fixed("claude-acp", ready()).with_download(Vec::new()),
        ]);
        let claude = &old.cached()[0];
        assert_eq!(claude.availability, ready());
        assert!(claude.needs_download);
        assert_eq!(
            claude.launch,
            "@agentclientprotocol/claude-agent-acp 0.95.0 with your claude"
        );
        assert_eq!(
            claude
                .connections
                .as_ref()
                .and_then(|value| value.active.as_deref()),
            Some("claude-acp")
        );
        assert_eq!(
            old.choose_connection("claude").await.map(|spec| spec.id),
            Ok("claude-acp")
        );
        // Updating, signing in and the Plugin library act on the installation that runs.
        assert_eq!(
            old.connection_for("claude").map(|spec| spec.id),
            Some("claude-acp")
        );

        // Neither can run: one reason for both ways in.
        let none = Detector::fixed(vec![
            fixed(
                "claude",
                broken("Ghost needs Claude Code 2.1.310 or newer (found 2.1.309)."),
            ),
            fixed(
                "claude-acp",
                missing("Node.js 22 or newer is required (found 20.11.0)."),
            ),
        ]);
        let reason = "Ghost needs Claude Code 2.1.310 or newer (found 2.1.309). ACP adapter: Node.js 22 or newer is required (found 20.11.0).";
        assert_eq!(none.cached()[0].availability, broken(reason));
        assert_eq!(
            none.choose_connection("claude").await,
            Err(LaunchError::Broken {
                reason: reason.into()
            })
        );
    }

    #[tokio::test]
    async fn the_connection_setting_applies_to_new_sessions_and_is_checked() {
        let detector = Detector::fixed(vec![
            fixed("claude", ready()),
            fixed("claude-acp", ready()),
            fixed("codex", ready()),
            fixed(
                "codex-acp",
                missing("Node.js 22 or newer is required (found none)."),
            ),
            fixed("hermes", ready()),
        ]);
        detector
            .set_connection("claude", Some("claude-acp"))
            .await
            .expect("the adapter is Claude's");
        let claude = &detector.cached()[0];
        let connections = claude.connections.as_ref().expect("two");
        assert_eq!(
            (
                connections.preferred.as_deref(),
                connections.active.as_deref()
            ),
            (Some("claude-acp"), Some("claude-acp"))
        );
        assert_eq!(
            detector
                .choose_connection("claude")
                .await
                .map(|spec| spec.id),
            Ok("claude-acp")
        );
        // An explicit connection that cannot run is used anyway, and the agent says why.
        detector
            .set_connection("codex", Some("codex-acp"))
            .await
            .expect("set");
        let codex = &detector.cached()[1];
        assert_eq!(
            codex.availability,
            missing("Node.js 22 or newer is required (found none).")
        );
        assert_eq!(
            codex
                .connections
                .as_ref()
                .and_then(|value| value.active.as_deref()),
            None
        );
        assert_eq!(
            detector
                .choose_connection("codex")
                .await
                .map(|spec| spec.id),
            Ok("codex-acp")
        );
        // Back to Automatic.
        detector
            .set_connection("codex-acp", None)
            .await
            .expect("automatic");
        assert_eq!(detector.cached()[1].availability, ready());
        assert!(
            detector
                .set_connection("claude", Some("codex"))
                .await
                .is_err()
        );
        assert!(
            detector
                .set_connection("hermes", Some("hermes"))
                .await
                .is_err()
        );
        assert!(detector.set_connection("gemini", None).await.is_err());
    }

    #[tokio::test]
    async fn a_lone_connection_is_still_shown_as_its_agent() {
        let detector = Detector::fixed(vec![fixed("claude-acp", ready())]);
        let entries = detector.cached();
        assert_eq!(ids(&entries), ["claude"]);
        assert_eq!(entries[0].connections, None);
        assert_eq!(
            detector.agent("claude").map(|spec| spec.id),
            Some("claude-acp")
        );
        assert_eq!(
            detector
                .choose_connection("claude")
                .await
                .map(|spec| spec.id),
            Ok("claude-acp")
        );
        assert!(detector.agent("codex").is_none());
        assert!(detector.choose_connection("codex").await.is_err());
    }

    fn ready() -> Availability {
        Availability::Ready {
            version: Some("1.0.0".into()),
            path: "/bin/x".into(),
        }
    }

    fn missing(reason: &str) -> Availability {
        Availability::Missing {
            reason: reason.into(),
        }
    }

    fn broken(reason: &str) -> Availability {
        Availability::Broken {
            reason: reason.into(),
        }
    }

    #[test]
    fn automatic_takes_the_first_connection_that_runs_and_waits_for_the_preferred_one() {
        let pick = |native: Availability, adapter: Availability| {
            choose(None, &[("Direct", &native), ("ACP adapter", &adapter)])
        };
        assert_eq!(pick(ready(), ready()).active, Some(0), "native first");
        let fallback = pick(broken("Too old."), ready());
        assert_eq!((fallback.active, fallback.shown), (Some(1), 1));
        assert_eq!(fallback.availability, ready());
        // Still checking the preferred one: the agent is checking, whatever the adapter says.
        let checking = pick(Availability::Checking, ready());
        assert_eq!(
            (checking.active, checking.availability),
            (None, Availability::Checking)
        );
        let later = pick(missing("Not found."), Availability::Checking);
        assert_eq!((later.active, later.shown), (None, 1));
        assert_eq!(later.availability, Availability::Checking);
    }

    #[test]
    fn an_explicit_connection_is_used_even_when_another_would_run() {
        let native = ready();
        let adapter = missing("Node.js 22 or newer is required (found none).");
        let connections = [("Direct", &native), ("ACP adapter", &adapter)];
        let explicit = choose(Some(1), &connections);
        assert_eq!((explicit.active, explicit.shown), (None, 1));
        assert_eq!(explicit.availability, adapter);
        assert_eq!(choose(Some(0), &connections).active, Some(0));
        // An index the agent does not have is Automatic.
        assert_eq!(choose(Some(7), &connections).active, Some(0));
    }

    #[test]
    fn when_nothing_runs_the_reason_names_what_each_connection_lacks() {
        let reason = |native: Availability, adapter: Availability| {
            choose(None, &[("Direct", &native), ("ACP adapter", &adapter)]).availability
        };
        // Not installed at all: the preferred connection's reason says it.
        assert_eq!(
            reason(
                missing("claude was not found on your PATH."),
                missing("Node.js 22 or newer is required (found none).")
            ),
            missing("claude was not found on your PATH.")
        );
        // Installed but too old for the native driver, and no Node.js for the adapter.
        assert_eq!(
            reason(
                broken("Ghost needs Claude Code 2.1.310 or newer (found 2.1.309)."),
                missing("Node.js 22 or newer is required (found 20.11.0).")
            ),
            broken(
                "Ghost needs Claude Code 2.1.310 or newer (found 2.1.309). ACP adapter: Node.js 22 or newer is required (found 20.11.0)."
            )
        );
        // The preferred one is absent, but the other is installed and unusable.
        assert_eq!(
            reason(
                missing("agy_acp_server was not found on your PATH."),
                broken(
                    "Antigravity: this catalog entry requires the 1.x release line; found 2.0.0"
                )
            ),
            missing(
                "agy_acp_server was not found on your PATH. ACP adapter: Antigravity: this catalog entry requires the 1.x release line; found 2.0.0"
            )
        );
        // The same reason is not repeated.
        assert_eq!(
            reason(broken("Damaged."), broken("Damaged.")),
            broken("Damaged.")
        );
    }
}
