use super::*;
use futures::{StreamExt, stream};
use ghost_protocol::agents::{AgentInstallationCandidate, AgentInstallations};
use std::path::Path;

impl Detector {
    /// Explicit inspection probes bounded candidates without changing the chosen executable.
    pub async fn installations(&self, id: &str) -> Result<AgentInstallations, String> {
        let Mode::Live(live) = &self.mode else {
            return Err("This catalog has no selectable executables.".into());
        };
        let spec = self
            .connection_for(id)
            .ok_or("The agent is not in the catalog.")?;
        let _selection = live.selection_gate.lock().await;
        inventory(live, &spec).await
    }

    /// An explicit choice applies to new sessions only. Aliases of one provider share it.
    /// Null restores the environment override/PATH default. A disappeared choice never falls back.
    pub async fn select_installation(
        &self,
        id: &str,
        selected: Option<String>,
    ) -> Result<AgentInstallations, String> {
        let Mode::Live(live) = &self.mode else {
            return Err("This catalog has no selectable executables.".into());
        };
        let spec = self
            .connection_for(id)
            .ok_or("The agent is not in the catalog.")?;
        if matches!(spec.recipe, Recipe::Builtin) {
            return Err("The built-in agent has no executable choice.".into());
        }
        let path = live
            .preferences
            .clone()
            .ok_or("This detector does not persist executable choices.")?;
        let _selection = live.selection_gate.lock().await;
        let env = live.env.get().await;
        let selected = selected.map(PathBuf::from);
        if let Some(selected) = &selected {
            let (choices, _) = crate::candidates::paths(&spec, &env.vars, cfg!(windows));
            let known = choices
                .iter()
                .any(|choice| same_path(&choice.path, selected))
                || lock(&live.state)
                    .selections
                    .get(crate::installation::provider_namespace(id))
                    .is_some_and(|saved| same_path(saved, selected));
            if !known {
                return Err(
                    "This executable is not in the discovered list. Refresh installations first."
                        .into(),
                );
            }
            let detected = probe::detect_at(
                &spec,
                &env,
                &live.registry,
                &live.adapters_dir,
                Some(selected),
            )
            .await;
            if !matches!(detected.availability, Availability::Ready { .. }) {
                return Err(match detected.availability {
                    Availability::Missing { reason } | Availability::Broken { reason } => reason,
                    _ => "The executable is not ready.".into(),
                });
            }
        }
        let provider = crate::installation::provider_namespace(id).to_owned();
        if lock(&live.state).maintenance.contains(&provider) {
            return Err("This agent is being updated. Wait before changing its executable.".into());
        }
        let selections = tokio::task::spawn_blocking(move || {
            crate::preferences::save(&path, &provider, selected.as_deref())
        })
        .await
        .map_err(|_| "The executable preference could not be saved.")??;
        replace_selections(live, Ok(selections));
        // Invalidated results are never returned as readiness for a new start.
        live.detect_every(true).await;
        inventory(live, &spec).await
    }

    /// Re-read the environment and installation metadata. Probes run only when metadata changed.
    pub async fn refresh_changed_installations(&self) {
        if let Mode::Live(live) = &self.mode {
            refresh_metadata(live).await;
        }
    }
}

async fn inventory(live: &Arc<Live>, spec: &AgentSpec) -> Result<AgentInstallations, String> {
    let env = live.env.get().await;
    let (preferred, notice) = {
        let state = lock(&live.state);
        (
            state
                .selections
                .get(crate::installation::provider_namespace(spec.id))
                .cloned(),
            state.preference_error.clone(),
        )
    };
    let (mut paths, truncated) = crate::candidates::paths(spec, &env.vars, cfg!(windows));
    let default = match &spec.recipe {
        Recipe::NativeBinary { user_binary, .. } | Recipe::NpmAdapter { user_binary, .. } => {
            match probe::locate_user_binary(user_binary, &env.vars, cfg!(windows)) {
                probe::UserBinaryLookup::Found(path) | probe::UserBinaryLookup::ShimOnly(path) => {
                    Some(path)
                }
                _ => None,
            }
        }
        Recipe::Binary { program, .. } | Recipe::QualifiedBinary { program, .. } => {
            find_program(program, &env.vars)
        }
        Recipe::Builtin => None,
    };
    let selected = preferred.as_ref().or(default.as_ref());
    if let Some(preferred) = &preferred
        && !paths
            .iter()
            .any(|candidate| same_path(&candidate.path, preferred))
    {
        paths.push(crate::candidates::Candidate {
            path: preferred.clone(),
            source: "Saved choice",
        });
    }
    let candidates = stream::iter(paths.into_iter().map(|candidate| {
        let env = Arc::clone(&env);
        async move {
            let detected = probe::detect_at(
                spec,
                &env,
                &live.registry,
                &live.adapters_dir,
                Some(&candidate.path),
            )
            .await;
            AgentInstallationCandidate {
                selected: selected.is_some_and(|selected| same_path(selected, &candidate.path)),
                path: candidate.path.display().to_string(),
                source: candidate.source.into(),
                availability: detected.availability,
                installation: detected.installation,
            }
        }
    }))
    .buffered(4)
    .collect()
    .await;
    Ok(AgentInstallations {
        agent_id: spec.id.into(),
        preferred_path: preferred.map(|p| p.display().to_string()),
        candidates,
        truncated,
        notice,
    })
}

fn same_path(a: &Path, b: &Path) -> bool {
    crate::candidates::key(a, cfg!(windows)) == crate::candidates::key(b, cfg!(windows))
}

pub(super) fn replace_selections(
    live: &Live,
    choices: Result<crate::preferences::Selections, String>,
) -> bool {
    let (choices, error) = match choices {
        Ok(choices) => (choices, None),
        Err(error) => (Default::default(), Some(error)),
    };
    let mut state = lock(&live.state);
    if state.selections == choices && state.preference_error == error {
        return false;
    }
    state.selections = choices;
    state.preference_error = error;
    invalidate_installations(live, &mut state);
    true
}

/// A changed executable or preference invalidates every derived launch and listing together.
/// The generation floor also prevents an older probe from restoring a stale result afterwards.
fn invalidate_installations(live: &Live, state: &mut LiveState) {
    state.configuration_revision += 1;
    state.listings.clear();
    for job in state.listing_jobs.values() {
        job.task.cancel();
    }
    state.generation += 1;
    let floor = state.generation;
    for spec in &live.specs {
        state.minimum_generation.insert(
            crate::installation::provider_namespace(spec.id).into(),
            floor,
        );
    }
    state.results.clear();
    state.probes.clear();
    state.releases.clear();
}

pub(super) async fn refresh_preferences(live: &Arc<Live>) -> bool {
    let _selection = live.selection_gate.lock().await;
    if let Some(path) = live.preferences.clone() {
        let result = tokio::task::spawn_blocking(move || crate::preferences::read(&path))
            .await
            .unwrap_or_else(|_| Err("Executable preferences could not be read.".into()));
        return replace_selections(live, result);
    }
    false
}

async fn refresh_metadata(live: &Arc<Live>) {
    let preference_changed = refresh_preferences(live).await;
    // Installers can create a known tools directory or update the registry/login-shell PATH
    // after Ghost starts. Looking only at the cached PATH would never discover those locations.
    // The resolver shares an in-flight refresh; existing sessions retain their original env.
    live.env.refresh();
    let env = live.env.get().await;
    let selected = lock(&live.state).selections.clone();
    let specs = live.specs.clone();
    let observed = tokio::task::spawn_blocking(move || {
        crate::candidates::fingerprint(&specs, &env.vars, &selected)
    })
    .await
    .ok();
    let changed = {
        let mut state = lock(&live.state);
        let changed =
            state.metadata_fingerprint.is_some() && observed != state.metadata_fingerprint;
        state.metadata_fingerprint = observed;
        if changed {
            invalidate_installations(live, &mut state);
        }
        changed
    };
    if changed || preference_changed {
        live.detect_every(true).await;
    }
}

pub(super) fn observe(live: &Arc<Live>) {
    let weak = Arc::downgrade(live);
    live.rt.spawn(async move {
        loop {
            {
                let Some(live) = weak.upgrade() else {
                    break;
                };
                refresh_metadata(&live).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_process::{EnvKey, EnvMap, EnvResolverConfig, EnvSource, ResolvedEnv};

    #[tokio::test]
    async fn invalidation_drops_cached_launches_and_rejects_older_probes() {
        let scratch = tempfile::tempdir().unwrap();
        let spec = crate::get("cursor").unwrap().clone();
        let live = Arc::new(Live {
            specs: vec![spec.clone()],
            env: EnvResolver::fixed(ResolvedEnv {
                // Empty PATH ensures no vendor process can be launched by this test.
                vars: EnvMap::new(),
                source: EnvSource::Fallback {
                    reason: "isolated fixture".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            registry: LeftoverRegistry::disabled(),
            adapters_dir: scratch.path().join("adapters"),
            rt: Handle::current(),
            state: Mutex::new(LiveState::default()),
            preferences: None,
            selection_gate: tokio::sync::Mutex::new(()),
            listing_slots: tokio::sync::Semaphore::new(2),
            listing_admission_gate: Mutex::new(None),
        });
        let old_path = scratch.path().join("removed-node.exe");
        lock(&live.state).results.insert(
            spec.id.into(),
            (
                0,
                Detection {
                    installation: None,
                    availability: Availability::Ready {
                        version: None,
                        path: old_path.display().to_string(),
                    },
                    needs_download: false,
                    located: Some(crate::probe::Located {
                        program: old_path,
                        prefix_args: vec!["old-index.js".into()],
                        node: None,
                        set_env: Vec::new(),
                    }),
                },
            ),
        );
        // This current-thread task cannot run before the invalidation below. Keep a waiter so
        // it completes afterwards, exercising the production generation check in Live::probe.
        let old_probe = live.probe(&spec, true);
        {
            let mut state = lock(&live.state);
            invalidate_installations(&live, &mut state);
            assert_eq!(state.configuration_revision, 1);
            assert!(state.probes.is_empty());
        }
        assert!(
            live.cached(&spec).is_none(),
            "a new launch must not use the removed runtime"
        );
        assert!(old_probe.wait().await.is_some());
        assert!(
            live.cached(&spec).is_none(),
            "an older probe restored an invalidated result"
        );
        let (progress, _) = mpsc::unbounded_channel();
        assert!(matches!(
            live.prepare_launch(&spec, progress).await,
            Err(LaunchError::Missing { .. })
        ));
        assert!(matches!(
            live.cached(&spec).unwrap().availability,
            Availability::Missing { .. }
        ));
    }

    #[tokio::test]
    async fn metadata_refresh_discovers_a_tools_directory_created_after_startup() {
        let scratch = tempfile::tempdir().unwrap();
        let added = scratch.path().join("installed-later");
        let env = EnvResolver::start(
            EnvResolverConfig {
                // No user shell, overrides or agent process is involved. Windows only reads
                // its environment registry; Unix uses the isolated fallback after this miss.
                shell: Some(scratch.path().join("no-shell")),
                base: Some(EnvMap::new()),
                home: Some(scratch.path().into()),
                known_dirs: Some(vec![added.clone()]),
                ..EnvResolverConfig::standard()
            },
            LeftoverRegistry::disabled(),
            Handle::current(),
        );
        let live = Arc::new(Live {
            specs: Vec::new(), // Never probe executables on the owner's machine.
            env: Arc::clone(&env),
            registry: LeftoverRegistry::disabled(),
            adapters_dir: scratch.path().join("adapters"),
            rt: Handle::current(),
            state: Mutex::new(LiveState::default()),
            preferences: None,
            selection_gate: tokio::sync::Mutex::new(()),
            listing_slots: tokio::sync::Semaphore::new(2),
            listing_admission_gate: Mutex::new(None),
        });
        let contains = |resolved: &ghost_process::ResolvedEnv| {
            resolved
                .vars
                .get(&EnvKey::from("PATH"))
                .is_some_and(|path| std::env::split_paths(path).any(|entry| entry == added))
        };
        refresh_metadata(&live).await;
        let original = env.get().await;
        assert!(!contains(&original));
        let fingerprint = lock(&live.state).metadata_fingerprint;
        std::fs::create_dir(&added).unwrap();
        assert!(
            !contains(env.get().await.as_ref()),
            "the cached environment remains unchanged"
        );
        refresh_metadata(&live).await;
        assert!(
            contains(env.get().await.as_ref()),
            "the next observation includes the new directory"
        );
        assert_ne!(lock(&live.state).metadata_fingerprint, fingerprint);
        assert_eq!(lock(&live.state).configuration_revision, 1);
        assert!(
            !contains(&original),
            "running sessions keep their original environment"
        );
    }
}
