//! The Plugin library, one section per agent: what that agent supports, read from its own CLI
//! where it has a reviewed machine-readable listing (ghost-agents) and from its files otherwise
//! (ghost-config), with the harness status detection computed.
//! Assembly is pure; [`SessionManager::plugin_library`] gathers the facts.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ghost_agents::plugins::AgentListings;
use ghost_config::{Inventory, Item, Kind, Provider, extensions::Extensions};
use ghost_process::EnvKey;
use ghost_protocol::plugins::{
    AgentPlugins, FeatureListing, HarnessStatus, ListingState, PluginEntry, PluginFeature,
    PluginLibrary, PluginLibraryRef, PluginSystem,
};
use ghost_protocol::session::{AgentEntry, Availability};

use super::{SessionError, SessionManager};

#[derive(Clone, Copy, Default)]
pub(super) struct RefreshState {
    pub(super) generation: u64,
    pauses: usize,
}

/// Holds library admission closed while a profile transition drains its command leases.
/// Dropping an abandoned transition reopens the current manager; shutdown remains permanent.
pub struct PluginRefreshPause {
    shared: Arc<super::Shared>,
}

impl Drop for PluginRefreshPause {
    fn drop(&mut self) {
        self.shared.plugin_refresh.send_modify(|state| {
            state.pauses -= 1;
            state.generation += 1;
        });
    }
}

/// Everything one Plugin library answer is built from.
#[derive(Default)]
pub struct PluginFacts {
    pub agents: Vec<AgentEntry>,
    /// Per provider namespace; absent when the agent is not installed or has no listing commands.
    pub listings: BTreeMap<String, AgentListings>,
    pub inventory: Inventory,
    /// Per provider namespace, including `opencode`.
    pub extensions: BTreeMap<String, Extensions>,
}

const SKILLS: &str = "From the agent's skills folder.";
const FOLDER: &str = "From the agent's folder.";

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "skill",
        Kind::Instructions => "instructions",
        Kind::McpServer => "mcp_server",
        Kind::Plugin => "plugin",
        Kind::Package => "package",
    }
}

fn from_item(item: &Item) -> PluginEntry {
    PluginEntry {
        key: item.item.key.clone(),
        label: item.label.clone(),
        description: None,
        version: None,
        marketplace: item
            .item
            .key
            .split_once('@')
            .map(|(_, market)| market.to_owned())
            .filter(|_| item.item.kind == Kind::Plugin),
        scope: Some(
            if item.provider == Provider::Shared {
                "shared"
            } else {
                "user"
            }
            .into(),
        ),
        enabled: None,
        installed: true,
        kind: None,
        local_fields: Vec::new(),
        library: Some(PluginLibraryRef {
            source_id: item.item.source_id.clone(),
            kind: kind_name(item.item.kind).into(),
            key: item.item.key.clone(),
            revision: item.revision.clone(),
        }),
    }
}

fn named(key: &str) -> PluginEntry {
    PluginEntry {
        key: key.into(),
        label: key.into(),
        description: None,
        version: None,
        marketplace: None,
        scope: Some("user".into()),
        enabled: None,
        installed: true,
        kind: None,
        local_fields: Vec::new(),
        library: None,
    }
}

fn items(inventory: &Inventory, providers: &[Provider], kind: Kind) -> Vec<PluginEntry> {
    inventory
        .items
        .iter()
        .filter(|item| {
            item.item.kind == kind
                && providers.contains(&item.provider)
                && item.scope == ghost_config::Scope::User
        })
        .map(from_item)
        .collect()
}

fn files(feature: PluginFeature, reason: &str, entries: Vec<PluginEntry>) -> FeatureListing {
    FeatureListing {
        feature,
        state: ListingState::Files {
            reason: reason.into(),
        },
        command: None,
        entries,
        available: Vec::new(),
        available_total: 0,
    }
}

/// The CLI's listing when it ran; its files when it failed (keeping the failure) or never ran.
fn live_or_files(
    listings: Option<&AgentListings>,
    feature: PluginFeature,
    fallback: Vec<PluginEntry>,
    not_run: &str,
) -> FeatureListing {
    let Some(listing) = listings.and_then(|listings| {
        listings
            .listings
            .iter()
            .find(|listing| listing.feature == feature)
    }) else {
        return files(feature, not_run, fallback);
    };
    let mut listing = listing.clone();
    if matches!(listing.state, ListingState::Failed { .. }) {
        for entry in &fallback {
            if !listing
                .entries
                .iter()
                .any(|retained| retained.key == entry.key)
            {
                listing.entries.push(entry.clone());
            }
        }
    }
    // Link live rows to their file items so they can open their preview and copy review.
    for entry in &mut listing.entries {
        if entry.library.is_none() {
            entry.library = fallback
                .iter()
                .find(|item| item.key == entry.key)
                .and_then(|item| item.library.clone());
        }
    }
    listing
}

pub fn assemble(facts: &PluginFacts) -> PluginLibrary {
    let entry = |id: &str| facts.agents.iter().find(|agent| agent.id == id);
    let ext = |id: &str| facts.extensions.get(id).cloned().unwrap_or_default();
    let mut library = PluginLibrary::default();
    for (id, name) in [
        ("claude", "Claude Code"),
        ("codex", "Codex"),
        ("pi", "Pi"),
        ("opencode", "OpenCode"),
        ("hermes", "Hermes"),
        ("grok", "Grok Build"),
        ("cursor", "Cursor"),
        ("antigravity", "Antigravity"),
        ("omp", "oh-my-pi"),
    ] {
        let agent = entry(id);
        let installed =
            agent.is_some_and(|agent| matches!(agent.availability, Availability::Ready { .. }));
        let version = agent
            .and_then(|agent| agent.installation.as_ref())
            .map(|installation| installation.version.clone());
        let harness = agent
            .and_then(|agent| agent.harness.clone())
            .unwrap_or(if agent.is_some() {
                HarnessStatus::Checking
            } else {
                HarnessStatus::NotInstalled
            });
        let listings = facts.listings.get(id).filter(|_| {
            installed
                || agent.is_some_and(|agent| matches!(agent.availability, Availability::Checking))
        });
        let extensions = ext(id);
        let not_run =
            if agent.is_some_and(|agent| matches!(agent.availability, Availability::Checking)) {
                "Detection is still running. These come from its folder."
            } else if installed {
                "The agent's listing did not run. These come from its folder."
            } else {
                "Not installed on this device. These come from its folder, if any."
            };
        let (system, summary, features) = match id {
            "claude" => (
                PluginSystem::Marketplace,
                "Plugins from marketplaces, plus skills, MCP servers, slash commands, subagents and hooks.",
                vec![
                    live_or_files(
                        listings,
                        PluginFeature::Plugins,
                        items(&facts.inventory, &[Provider::Claude], Kind::Plugin),
                        not_run,
                    ),
                    live_or_files(
                        listings,
                        PluginFeature::Marketplaces,
                        extensions
                            .marketplaces
                            .iter()
                            .map(|name| named(name))
                            .collect(),
                        not_run,
                    ),
                    files(
                        PluginFeature::Skills,
                        SKILLS,
                        items(&facts.inventory, &[Provider::Claude], Kind::Skill),
                    ),
                    files(
                        PluginFeature::McpServers,
                        "`claude mcp list` connects to every server to check it, so these come from ~/.claude.json.",
                        items(&facts.inventory, &[Provider::Claude], Kind::McpServer),
                    ),
                    files(
                        PluginFeature::Commands,
                        FOLDER,
                        extensions.commands.iter().map(|name| named(name)).collect(),
                    ),
                    files(
                        PluginFeature::Subagents,
                        FOLDER,
                        extensions
                            .subagents
                            .iter()
                            .map(|name| named(name))
                            .collect(),
                    ),
                    files(
                        PluginFeature::Hooks,
                        FOLDER,
                        extensions.hooks.iter().map(|name| named(name)).collect(),
                    ),
                ],
            ),
            "codex" => (
                PluginSystem::Marketplace,
                "Plugins from marketplaces, plus MCP servers, skills, subagents and hooks.",
                vec![
                    live_or_files(
                        listings,
                        PluginFeature::Plugins,
                        items(&facts.inventory, &[Provider::Codex], Kind::Plugin),
                        not_run,
                    ),
                    live_or_files(
                        listings,
                        PluginFeature::Marketplaces,
                        extensions
                            .marketplaces
                            .iter()
                            .map(|name| named(name))
                            .collect(),
                        not_run,
                    ),
                    live_or_files(
                        listings,
                        PluginFeature::McpServers,
                        items(&facts.inventory, &[Provider::Codex], Kind::McpServer),
                        not_run,
                    ),
                    files(
                        PluginFeature::Skills,
                        SKILLS,
                        items(
                            &facts.inventory,
                            &[Provider::Codex, Provider::Shared],
                            Kind::Skill,
                        ),
                    ),
                    files(
                        PluginFeature::Subagents,
                        FOLDER,
                        extensions
                            .subagents
                            .iter()
                            .map(|name| named(name))
                            .collect(),
                    ),
                    files(
                        PluginFeature::Hooks,
                        FOLDER,
                        extensions.hooks.iter().map(|name| named(name)).collect(),
                    ),
                ],
            ),
            "pi" => (
                PluginSystem::Packages,
                "Pi has no plugin marketplace. It installs packages from npm or git, and reads skills, MCP servers and prompt templates from its folder.",
                vec![
                    live_or_files(
                        listings,
                        PluginFeature::Packages,
                        items(&facts.inventory, &[Provider::Pi], Kind::Package),
                        not_run,
                    ),
                    files(
                        PluginFeature::Skills,
                        SKILLS,
                        items(&facts.inventory, &[Provider::Pi], Kind::Skill),
                    ),
                    files(
                        PluginFeature::McpServers,
                        "`pi mcp list` starts every server to check it, so these come from mcp.json.",
                        items(&facts.inventory, &[Provider::Pi], Kind::McpServer),
                    ),
                    files(
                        PluginFeature::Commands,
                        FOLDER,
                        extensions.commands.iter().map(|name| named(name)).collect(),
                    ),
                ],
            ),
            "opencode" => {
                const OPENCODE: &str = "OpenCode installs its plugins and checks its servers whenever it runs, so Ghost reads ~/.config/opencode instead.";
                let servers = extensions
                    .servers
                    .iter()
                    .map(|server| PluginEntry {
                        kind: server.kind.clone(),
                        enabled: server.enabled,
                        local_fields: server.local_fields.clone(),
                        ..named(&server.name)
                    })
                    .collect();
                (
                    PluginSystem::Declared,
                    "OpenCode loads npm plugins named in its config and local plugin files, plus MCP servers, skills, commands and agents.",
                    vec![
                        files(
                            PluginFeature::Plugins,
                            OPENCODE,
                            extensions.plugins.iter().map(|name| named(name)).collect(),
                        ),
                        files(PluginFeature::McpServers, OPENCODE, servers),
                        files(
                            PluginFeature::Skills,
                            OPENCODE,
                            extensions.skills.iter().map(|name| named(name)).collect(),
                        ),
                        files(
                            PluginFeature::Commands,
                            OPENCODE,
                            extensions.commands.iter().map(|name| named(name)).collect(),
                        ),
                        files(
                            PluginFeature::Subagents,
                            OPENCODE,
                            extensions
                                .subagents
                                .iter()
                                .map(|name| named(name))
                                .collect(),
                        ),
                    ],
                )
            }
            // Grok Build lists its plugins and marketplace sources as JSON; Hermes lists its
            // plugins, and its skill sources (taps) are read from the file it writes. Both keep
            // the reviewed file inventory for MCP servers and skills.
            "grok" | "hermes" => {
                let reason = "Read from reviewed native configuration paths. This inventory does not start the agent or connect servers.";
                let servers = extensions
                    .servers
                    .iter()
                    .map(|s| PluginEntry {
                        kind: s.kind.clone(),
                        enabled: s.enabled,
                        local_fields: s.local_fields.clone(),
                        ..named(&s.name)
                    })
                    .collect();
                let skills = extensions.skills.iter().map(|s| named(s)).collect();
                let marketplaces = if id == "grok" {
                    live_or_files(listings, PluginFeature::Marketplaces, Vec::new(), not_run)
                } else {
                    files(
                        PluginFeature::Marketplaces,
                        "Hermes skill sources (taps), read from skills/.hub/taps.json.",
                        extensions
                            .marketplaces
                            .iter()
                            .map(|name| named(name))
                            .collect(),
                    )
                };
                (
                    PluginSystem::Marketplace,
                    if id == "grok" {
                        "Plugins from marketplace sources, plus skills and MCP servers."
                    } else {
                        "Plugins from Hermes's catalog or GitHub, skill sources (taps), skills and MCP servers."
                    },
                    vec![
                        live_or_files(listings, PluginFeature::Plugins, Vec::new(), not_run),
                        marketplaces,
                        files(PluginFeature::McpServers, reason, servers),
                        files(PluginFeature::Skills, reason, skills),
                    ],
                )
            }
            _ => {
                let reason = "Read from reviewed native configuration paths. This inventory does not start the agent or connect servers.";
                let servers = extensions
                    .servers
                    .iter()
                    .map(|s| PluginEntry {
                        kind: s.kind.clone(),
                        enabled: s.enabled,
                        local_fields: s.local_fields.clone(),
                        ..named(&s.name)
                    })
                    .collect();
                let mut features = vec![
                    files(PluginFeature::McpServers, reason, servers),
                    files(
                        PluginFeature::Skills,
                        reason,
                        extensions.skills.iter().map(|s| named(s)).collect(),
                    ),
                ];
                for (feature, names) in [
                    (PluginFeature::Commands, &extensions.commands),
                    (PluginFeature::Subagents, &extensions.subagents),
                    (PluginFeature::Hooks, &extensions.hooks),
                ] {
                    if !names.is_empty() {
                        features.push(files(
                            feature,
                            reason,
                            names.iter().map(|s| named(s)).collect(),
                        ));
                    }
                }
                features.push(FeatureListing{feature:PluginFeature::Plugins,state:ListingState::Unavailable{reason:"This agent has no qualified structured plugin-management interface in Ghost. Open its native customization route for supported plugins, hooks, rules and memory.".into()},command:None,entries:Vec::new(),available:Vec::new(),available_total:0});
                (
                    PluginSystem::Declared,
                    "Native customization capabilities vary by agent and installed version. Reviewed file inventory and named native routes are shown where available.",
                    features,
                )
            }
        };
        library.agents.push(AgentPlugins {
            management: ghost_agents::plugins::management(id, version.as_deref()),
            agent_id: id.into(),
            name: name.into(),
            installed,
            version,
            system,
            summary: summary.into(),
            features,
            harness,
            updated_at: listings.map(|listings| listings.updated_at),
            version_change: listings.and_then(|listings| listings.version_change.clone()),
            notices: extensions.notices.clone(),
        });
    }
    library
        .notices
        .extend(facts.inventory.notices.iter().cloned());
    library
}

/// OpenCode's global folder: `$XDG_CONFIG_HOME/opencode`, else `~/.config/opencode`, on every OS.
pub(super) fn opencode_root(env: &ghost_process::EnvMap) -> Option<PathBuf> {
    let variable = |key: &str| {
        env.get(&EnvKey::from(key))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    variable("XDG_CONFIG_HOME")
        .map(|config| config.join("opencode"))
        .or_else(|| {
            if cfg!(windows) {
                variable("USERPROFILE").or_else(|| variable("HOME"))
            } else {
                variable("HOME")
            }
            .map(|home| home.join(".config").join("opencode"))
        })
}

impl SessionManager {
    /// The Plugin library. `refresh` probes the agents again and reruns every listing; otherwise
    /// a listing is reused unless its agent's installed version changed since it was read.
    pub async fn plugin_library(&self, refresh: bool) -> Result<PluginLibrary, SessionError> {
        self.plugin_library_progressive(refresh, |_| true).await
    }

    /// Cancels the current read so profile switches can drain their admitted commands.
    pub fn cancel_plugin_refresh(&self) {
        self.shared()
            .plugin_refresh
            .send_modify(|state| state.generation += 1);
    }

    /// Cancels current reads and rejects later registrations until the returned guard drops.
    pub fn pause_plugin_refresh(&self) -> PluginRefreshPause {
        self.shared().plugin_refresh.send_modify(|state| {
            state.pauses += 1;
            state.generation += 1;
        });
        PluginRefreshPause {
            shared: self.shared().clone(),
        }
    }

    /// Publishes all cached/file sections first, then each completed provider independently.
    /// A newer request or shutdown drops this request's pending helper waiters.
    pub async fn plugin_library_progressive(
        &self,
        refresh: bool,
        mut publish: impl FnMut(PluginLibrary) -> bool + Send,
    ) -> Result<PluginLibrary, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let updates = &self.shared().plugin_refresh;
        let mut cancelled = updates.subscribe();
        let mut generation = None;
        updates.send_modify(|state| {
            if state.pauses == 0 && !self.shared().is_shutting_down() {
                state.generation += 1;
                generation = Some(state.generation);
            }
        });
        let generation = generation.ok_or_else(|| {
            SessionError::Internal("Plugins & MCP is paused during a profile change.".into())
        })?;
        let mut publish_current = |library| {
            let current = updates.borrow();
            current.generation == generation
                && current.pauses == 0
                && !self.shared().is_shutting_down()
                && publish(library)
        };
        if cancelled.borrow_and_update().generation != generation {
            return Err(SessionError::Internal(
                "The Plugins & MCP refresh was replaced.".into(),
            ));
        }
        tokio::select! {
            biased;
            _ = cancelled.changed() => Err(SessionError::Internal("The Plugins & MCP refresh was replaced.".into())),
            result = self.plugin_library_read(refresh, &mut publish_current) => result,
        }
    }

    async fn plugin_library_read(
        &self,
        refresh: bool,
        publish: &mut (impl FnMut(PluginLibrary) -> bool + Send),
    ) -> Result<PluginLibrary, SessionError> {
        let agents = self.deps().detector.cached();
        let listings = self.deps().detector.cached_plugin_listings();
        let roots = self.config_library_roots(None).await?;
        let env = self.deps().env.get().await;
        let opencode = opencode_root(&env.vars);
        let extra_roots = [
            ("hermes", "HERMES_HOME", ".hermes"),
            ("cursor", "", ".cursor"),
            ("grok", "", ".grok"),
        ]
        .into_iter()
        .filter_map(|(id, key, suffix)| {
            let variable = |name: &str| {
                env.vars
                    .get(&EnvKey::from(name))
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
            };
            let path = if key.is_empty() { None } else { variable(key) }.or_else(|| {
                variable(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|p| p.join(suffix))
            })?;
            Some((id, path))
        })
        .collect::<Vec<_>>();
        let (inventory, extensions) = self
            .deps()
            .rt
            .spawn_blocking(move || {
                let inventory = ghost_config::Library::new(roots.sources.clone())
                    .map(|library| library.inspect())
                    .unwrap_or_default();
                let mut extensions = BTreeMap::new();
                for source in &roots.sources {
                    if source.scope != ghost_config::Scope::User || source.claude_state_only {
                        continue;
                    }
                    let id = match source.provider {
                        Provider::Claude => "claude",
                        Provider::Codex => "codex",
                        Provider::Pi => "pi",
                        Provider::Shared => continue,
                    };
                    extensions.insert(
                        id.to_owned(),
                        ghost_config::extensions::inspect(source.provider, &source.root),
                    );
                }
                if let Some(root) = opencode {
                    extensions.insert(
                        "opencode".to_owned(),
                        ghost_config::extensions::inspect_opencode(Path::new(&root)),
                    );
                }
                for (id, root) in extra_roots {
                    extensions.insert(
                        id.into(),
                        ghost_config::extensions::inspect_extra(id, &root),
                    );
                }
                (inventory, extensions)
            })
            .await
            .map_err(|_| SessionError::Internal("Plugins & MCP could not be read.".into()))?;
        let mut facts = PluginFacts {
            agents,
            listings,
            inventory,
            extensions,
        };
        let mut library = assemble(&facts);
        if !publish(library.clone()) {
            return Ok(library);
        }
        if refresh {
            self.deps().detector.prepare_plugin_refresh().await;
        }
        use futures::{StreamExt, stream::FuturesUnordered};
        let detector = self.deps().detector.clone();
        let mut pending = [
            "claude",
            "codex",
            "pi",
            "opencode",
            "hermes",
            "grok",
            "cursor",
            "antigravity",
            "omp",
        ]
        .into_iter();
        let mut jobs = FuturesUnordered::new();
        for id in pending.by_ref().take(2) {
            jobs.push(refresh_provider(detector.clone(), id, refresh));
        }
        while let Some((id, listing)) = jobs.next().await {
            if let Some(listing) = listing {
                facts.listings.insert(id.into(), listing);
            } else {
                facts.listings.remove(id);
            }
            facts.agents = self.deps().detector.cached();
            library = assemble(&facts);
            if !publish(library.clone()) {
                break;
            }
            if let Some(id) = pending.next() {
                jobs.push(refresh_provider(detector.clone(), id, refresh));
            }
        }
        Ok(library)
    }
}

async fn refresh_provider(
    detector: Arc<ghost_agents::Detector>,
    id: &'static str,
    refresh: bool,
) -> (&'static str, Option<AgentListings>) {
    if refresh {
        detector.refresh_plugin_provider(id).await;
    }
    let listing = if id == "opencode" {
        None
    } else {
        detector
            .plugin_listings(id, refresh, crate::time::now_millis())
            .await
    };
    (id, listing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_protocol::agents::{
        AgentHost, AgentInstallation, InstallChannel, VersionVerification,
    };

    fn agent(id: &str, version: Option<&str>, harness: Option<HarnessStatus>) -> AgentEntry {
        AgentEntry {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            availability: match version {
                Some(version) => Availability::Ready {
                    version: Some(version.into()),
                    path: format!("/bin/{id}"),
                },
                None => Availability::Missing {
                    reason: "not found".into(),
                },
            },
            launch: String::new(),
            needs_download: false,
            last_verified: String::new(),
            installation: version.map(|version| AgentInstallation {
                agent_id: id.into(),
                provider: id.into(),
                host: AgentHost::Native,
                launch_path: format!("/bin/{id}"),
                resolved_path: None,
                version: version.into(),
                channel: InstallChannel::VendorStandalone,
                verification: VersionVerification::Unknown,
            }),
            latest_release: None,
            harness,
            connections: None,
            terminal: false,
        }
    }

    fn listing(feature: PluginFeature, state: ListingState, keys: &[&str]) -> FeatureListing {
        FeatureListing {
            feature,
            state,
            command: Some("claude plugin list --json --available".into()),
            entries: keys.iter().map(|key| named(key)).collect(),
            available: Vec::new(),
            available_total: 0,
        }
    }

    #[test]
    fn a_failed_refresh_keeps_cached_rows_and_file_only_rows_with_their_links() {
        let cached = AgentListings {
            version: "1".into(),
            updated_at: 1,
            version_change: None,
            listings: vec![listing(
                PluginFeature::Plugins,
                ListingState::Failed {
                    reason: "fixture failure".into(),
                },
                &["cached@team"],
            )],
        };
        let mut fallback = named("file-only@team");
        fallback.library = Some(PluginLibraryRef {
            source_id: "user:claude".into(),
            kind: "plugin".into(),
            key: fallback.key.clone(),
            revision: "1".into(),
        });
        let result = live_or_files(
            Some(&cached),
            PluginFeature::Plugins,
            vec![fallback],
            "not run",
        );
        assert!(matches!(result.state, ListingState::Failed { .. }));
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            ["cached@team", "file-only@team"]
        );
        assert!(result.entries[1].library.is_some());
    }

    #[test]
    fn initial_detection_is_checking_and_unsupported_inventory_is_explicit() {
        let facts = PluginFacts {
            agents: vec![AgentEntry {
                availability: Availability::Checking,
                ..agent("claude", None, None)
            }],
            ..Default::default()
        };
        let initial = assemble(&facts);
        assert_eq!(initial.agents.len(), 9);
        assert_eq!(initial.agents[0].harness, HarnessStatus::Checking);
        assert!(!initial.agents[0].installed);
        assert!(matches!(&initial.agents[0].features[0].state,
            ListingState::Files { reason } if reason.contains("Detection is still running")));
    }

    #[test]
    fn a_known_missing_provider_never_assembles_cached_live_rows_or_timestamp() {
        let facts = PluginFacts {
            agents: vec![agent("claude", None, Some(HarnessStatus::NotInstalled))],
            listings: [(
                "claude".into(),
                AgentListings {
                    version: "2.1.310".into(),
                    updated_at: 42,
                    version_change: None,
                    listings: vec![listing(
                        PluginFeature::Plugins,
                        ListingState::Live,
                        &["old@team"],
                    )],
                },
            )]
            .into(),
            ..Default::default()
        };
        let result = assemble(&facts);
        let claude = &result.agents[0];
        assert!(!claude.installed);
        assert!(claude.updated_at.is_none());
        assert!(
            claude
                .features
                .iter()
                .all(|listing| !matches!(listing.state, ListingState::Live))
        );
        assert!(
            claude
                .features
                .iter()
                .all(|listing| listing.entries.is_empty())
        );
    }

    /// One section per agent, from the same one-entry-per-agent list New chat reads: Claude Code
    /// running through its ACP adapter (its CLI too old for the native driver) is installed, with
    /// one section, and no adapter or OpenCode line gets its own.
    #[test]
    fn an_agent_running_through_its_adapter_is_one_installed_section() {
        let adapter = AgentEntry {
            connections: Some(ghost_protocol::session::AgentConnections {
                preferred: None,
                active: Some("claude-acp".into()),
                options: Vec::new(),
            }),
            ..agent("claude", Some("2.1.309"), None)
        };
        let facts = PluginFacts {
            agents: vec![adapter, agent("opencode", Some("2.1.4"), None)],
            ..Default::default()
        };
        let library = assemble(&facts);
        let sections: Vec<(&str, bool)> = library
            .agents
            .iter()
            .map(|agent| (agent.agent_id.as_str(), agent.installed))
            .collect();
        assert_eq!(
            sections,
            [
                ("claude", true),
                ("codex", false),
                ("pi", false),
                ("opencode", true),
                ("hermes", false),
                ("grok", false),
                ("cursor", false),
                ("antigravity", false),
                ("omp", false)
            ]
        );
        assert_eq!(library.agents[0].version.as_deref(), Some("2.1.309"));
    }

    #[test]
    fn each_agent_gets_its_own_section_with_what_it_supports() {
        let mut facts = PluginFacts {
            agents: vec![
                agent(
                    "claude",
                    Some("2.1.312"),
                    Some(HarnessStatus::UpdateAvailable {
                        version: "2.1.312".into(),
                        latest: "2.1.314".into(),
                        source: "npm latest".into(),
                        checked_at: Some(1),
                    }),
                ),
                agent("codex", Some("0.162.1"), None),
                agent("pi", None, Some(HarnessStatus::NotInstalled)),
            ],
            ..Default::default()
        };
        facts.listings.insert(
            "claude".into(),
            AgentListings {
                version: "2.1.312".into(),
                updated_at: 42,
                listings: vec![
                    listing(PluginFeature::Plugins, ListingState::Live, &["tools@team"]),
                    listing(
                        PluginFeature::Marketplaces,
                        ListingState::Failed {
                            reason: "`claude plugin marketplace list --json` failed.".into(),
                        },
                        &[],
                    ),
                ],
                version_change: Some(ghost_protocol::plugins::VersionChange {
                    from: "2.1.310".into(),
                    to: "2.1.312".into(),
                }),
            },
        );
        facts.extensions.insert(
            "claude".into(),
            Extensions {
                marketplaces: vec!["team".into()],
                hooks: vec!["PreToolUse".into()],
                ..Default::default()
            },
        );
        let library = assemble(&facts);
        let ids: Vec<_> = library
            .agents
            .iter()
            .map(|agent| agent.agent_id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "claude",
                "codex",
                "pi",
                "opencode",
                "hermes",
                "grok",
                "cursor",
                "antigravity",
                "omp"
            ]
        );
        let claude = &library.agents[0];
        assert_eq!(claude.updated_at, Some(42));
        assert!(claude.version_change.is_some());
        assert!(matches!(
            claude.harness,
            HarnessStatus::UpdateAvailable { .. }
        ));
        let feature = |agent: &AgentPlugins, feature| {
            agent
                .features
                .iter()
                .find(|listing| listing.feature == feature)
                .unwrap()
                .clone()
        };
        assert_eq!(
            feature(claude, PluginFeature::Plugins).state,
            ListingState::Live
        );
        // A failed listing keeps its reason and falls back to the declared names.
        let markets = feature(claude, PluginFeature::Marketplaces);
        assert!(matches!(markets.state, ListingState::Failed { .. }));
        assert_eq!(markets.entries[0].key, "team");
        assert!(matches!(
            feature(claude, PluginFeature::McpServers).state,
            ListingState::Files { .. }
        ));
        assert_eq!(
            feature(claude, PluginFeature::Hooks).entries[0].key,
            "PreToolUse"
        );
        let codex = &library.agents[1];
        assert_eq!(codex.harness, HarnessStatus::Checking);
        let pi = &library.agents[2];
        assert_eq!(pi.system, PluginSystem::Packages);
        assert!(pi.summary.starts_with("Pi has no plugin marketplace"));
        assert!(!pi.installed);
        assert!(
            pi.features
                .iter()
                .all(|listing| !matches!(listing.state, ListingState::Live))
        );
        let opencode = &library.agents[3];
        assert_eq!(opencode.harness, HarnessStatus::NotInstalled);
    }
}
