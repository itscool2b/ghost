//! Detection: whether each agent can start here, and the launch spec of one start.
//!
//! Probes run through `run_helper` with the resolved, scrubbed env, 10 s and 64 KiB of output;
//! binaries run in `$HOME`, adapter probes in the adapter folder. Results are cached, and every
//! caller shares the env resolution and the probe in flight. Detection never installs and never
//! runs an adapter: the install happens in [`Detector::prepare_launch`].
//!
//! - `Binary`: found on the env's `PATH`, its version command runs, and it meets its floor.
//! - `NativeBinary`: the user's executable override or CLI on `PATH` runs and meets its floor,
//!   with the same executable rules as adapters but without Node.js or npm.
//! - `NpmAdapter`: Node of the required major or newer; the user's binary (the adapter's variable
//!   when it names an executable file, else the one on `PATH`; `.exe` only on Windows) runs and
//!   meets its floor; `npm` is needed only until the adapter is installed. `Ready` names the user's
//!   binary, the one that will actually run.
//!
//! Other crates' tests use [`Detector::fixed`]: [`FixedAgent`]s answer as given, can read
//! `Checking` until a detection ends, send download details, and hold detection or a launch
//! preparation at a [`Gate`]; the detector records every [`Detector::forget_install`].

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use ghost_process::{EnvResolver, LaunchSpec, LeftoverRegistry, find_program};
use ghost_protocol::session::{AgentEntry, Availability};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};

use crate::catalog::{AgentSpec, Recipe, catalog};
use crate::install::{self, InstallJob};
use crate::launch::{self, LaunchError};
use crate::probe::{self, BUILT_IN, Detection};
use crate::shared::SharedTask;

mod agents;
mod selections;

/// Detects agents and prepares their launches.
#[derive(Debug)]
pub struct Detector {
    mode: Mode,
    /// The agent of every `forget_install`, in order.
    forgotten: Mutex<Vec<String>>,
    /// Each agent's Connection setting on this device (agent id to connection id); an agent
    /// without one is Automatic.
    connection_choices: Mutex<crate::connection_choices::Choices>,
    /// Where they are saved: beside the executable choices, for a detector that persists those.
    connection_path: Option<PathBuf>,
}

#[derive(Debug)]
enum Mode {
    Live(Arc<Live>),
    Fixed { agents: Vec<Fixed> },
}

/// A probe of one agent, shared by everyone waiting for it.
type Probe = SharedTask<Detection>;

/// An install of one adapter version, shared by every start waiting for it: when the last of them
/// stops waiting, the install is cancelled and npm is killed.
type Install = SharedTask<Result<PathBuf, LaunchError>>;

/// How often a probe or an install is started again when the one joined was cancelled under it.
const ATTEMPTS: usize = 3;

/// The real detector: the catalog's agents, probed with the resolved env.
struct Live {
    specs: Vec<AgentSpec>,
    env: Arc<EnvResolver>,
    registry: Arc<LeftoverRegistry>,
    adapters_dir: PathBuf,
    rt: Handle,
    state: Mutex<LiveState>,
    preferences: Option<PathBuf>,
    selection_gate: tokio::sync::Mutex<()>,
    /// At most two providers run listings, each provider runs its commands in turn.
    listing_slots: tokio::sync::Semaphore,
    #[cfg(test)]
    listing_admission_gate: Mutex<Option<Gate>>,
}

#[derive(Default)]
struct LiveState {
    metadata_fingerprint: Option<u64>,
    selections: crate::preferences::Selections,
    preference_error: Option<String>,
    configuration_revision: u64,
    release_checks_enabled: bool,
    releases: HashMap<String, ghost_protocol::agents::LatestRelease>,
    /// The last Plugin library listing of each provider, with the version it was read from.
    listings: HashMap<String, crate::plugins::AgentListings>,
    listing_jobs: HashMap<String, ListingJob>,
    listing_generation: u64,
    maintenance: HashSet<String>,
    /// What each provider's last update printed, cleaned for reading. Memory only: it is shown to
    /// the person who ran the update and is gone when Ghost quits.
    update_output: HashMap<String, String>,
    minimum_generation: HashMap<String, u64>,
    /// Each agent's latest detection, with the generation of the probe that made it.
    results: HashMap<String, (u64, Detection)>,
    probes: HashMap<String, Probe>,
    installs: HashMap<String, Install>,
    generation: u64,
}

struct ListingJob {
    version: String,
    revision: u64,
    generation: u64,
    minimum: Option<u64>,
    task: SharedTask<Option<crate::plugins::AgentListings>>,
    gate: Arc<tokio::sync::Mutex<()>>,
}

fn provider_detection_generations(
    state: &LiveState,
    provider: &str,
) -> std::collections::BTreeMap<String, u64> {
    state
        .results
        .iter()
        .filter(|(id, _)| crate::installation::provider_namespace(id) == provider)
        .map(|(id, (generation, _))| (id.clone(), *generation))
        .collect()
}

struct MaintenanceGuard {
    live: Arc<Live>,
    provider: String,
    active: bool,
}
impl MaintenanceGuard {
    fn complete(mut self, spec: &AgentSpec, detection: Detection) {
        let mut state = lock(&self.live.state);
        state.generation += 1;
        let generation = state.generation;
        state
            .minimum_generation
            .insert(self.provider.clone(), generation);
        state
            .probes
            .retain(|id, _| crate::installation::provider_namespace(id) != self.provider);
        state
            .results
            .insert(spec.id.to_owned(), (generation, detection));
        state.maintenance.remove(&self.provider);
        self.active = false;
    }
}
impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = lock(&self.live.state);
        state.maintenance.remove(&self.provider);
        state
            .probes
            .retain(|id, _| crate::installation::provider_namespace(id) != self.provider);
    }
}

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = lock(&self.state);
        f.debug_struct("Live")
            .field("agents", &self.specs.len())
            .field("detected", &state.results.len())
            .field("adapters_dir", &self.adapters_dir)
            .finish_non_exhaustive()
    }
}

impl Live {
    /// The probe of `spec` in flight, or a new one (always a new one with `fresh`).
    fn probe(self: &Arc<Self>, spec: &AgentSpec, fresh: bool) -> Probe {
        let mut state = lock(&self.state);
        if !fresh && let Some(running) = state.probes.get(spec.id).filter(|probe| probe.is_live()) {
            return running.clone();
        }
        state.generation += 1;
        let generation = state.generation;
        let live = Arc::clone(self);
        let id = spec.id;
        let spec = spec.clone();
        let selected = state
            .selections
            .get(crate::installation::provider_namespace(spec.id))
            .cloned();
        let preference_error = state.preference_error.clone();
        let probe = SharedTask::spawn(&self.rt, async move {
            let env = live.env.get().await;
            let detection = match preference_error {
                Some(reason) if !matches!(spec.recipe, Recipe::Builtin) => {
                    Detection::missing(reason)
                }
                _ => {
                    probe::detect_at(
                        &spec,
                        &env,
                        &live.registry,
                        &live.adapters_dir,
                        selected.as_deref(),
                    )
                    .await
                }
            };
            let mut state = lock(&live.state);
            let newer = state
                .results
                .get(spec.id)
                .is_some_and(|(known, _)| *known > generation);
            let provider = crate::installation::provider_namespace(spec.id);
            let invalidated = state
                .minimum_generation
                .get(provider)
                .is_some_and(|minimum| generation < *minimum)
                || state.maintenance.contains(provider);
            if !newer && !invalidated {
                state
                    .results
                    .insert(spec.id.to_owned(), (generation, detection.clone()));
            }
            detection
        });
        state.probes.insert(id.to_owned(), probe.clone());
        probe
    }

    fn cached(&self, spec: &AgentSpec) -> Option<Detection> {
        lock(&self.state)
            .results
            .get(spec.id)
            .map(|(_, detection)| detection.clone())
    }

    /// The cached detection, or the one in flight, or a new one.
    async fn detect(self: &Arc<Self>, spec: &AgentSpec) -> Detection {
        if let Some(detection) = self.cached(spec) {
            return detection;
        }
        for _ in 0..ATTEMPTS {
            let revision = lock(&self.state).configuration_revision;
            if let Some(detection) = self.probe(spec, false).wait().await {
                if lock(&self.state).configuration_revision != revision {
                    continue;
                }
                return detection;
            }
        }
        Detection::missing(format!(
            "Checking {} was interrupted; try again.",
            spec.name
        ))
    }

    /// Probes every agent at once and waits for all of them.
    async fn detect_every(self: &Arc<Self>, fresh: bool) {
        let probes: Vec<Probe> = self
            .specs
            .iter()
            .map(|spec| self.probe(spec, fresh))
            .collect();
        // Held together, so one start leaving a probe early never cancels it under this wait.
        let _holds: Vec<_> = probes.iter().map(SharedTask::hold).collect();
        for probe in &probes {
            probe.wait().await;
        }
    }

    /// Updates the cached download state of an adapter.
    fn set_needs_download(&self, spec: &AgentSpec, needs_download: bool) {
        if let Some((_, detection)) = lock(&self.state).results.get_mut(spec.id) {
            detection.needs_download = needs_download;
        }
    }

    fn entry(&self, spec: &AgentSpec) -> AgentEntry {
        if matches!(spec.recipe, Recipe::Builtin) {
            return entry(
                spec,
                Availability::Ready {
                    version: None,
                    path: BUILT_IN.into(),
                },
                false,
            );
        }
        match self.cached(spec) {
            Some(detection) => {
                let mut entry = entry(spec, detection.availability, detection.needs_download);
                entry.installation = detection.installation;
                let state = lock(&self.state);
                entry.latest_release = state.releases.get(spec.id).cloned();
                entry.harness = Some(crate::harness::harness_status(
                    &crate::harness::HarnessFacts {
                        availability: &entry.availability,
                        installation: entry.installation.as_ref(),
                        latest: entry.latest_release.as_ref(),
                        has_source: entry
                            .installation
                            .as_ref()
                            .and_then(crate::releases::ReleaseSource::for_installation)
                            .is_some(),
                        checks_enabled: state.release_checks_enabled,
                    },
                ));
                drop(state);
                entry
            }
            None => entry(spec, Availability::Checking, false),
        }
    }

    async fn prepare_launch(
        self: &Arc<Self>,
        spec: &AgentSpec,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<LaunchSpec, LaunchError> {
        let detection = self.detect(spec).await;
        let located = match (&detection.availability, detection.located) {
            (Availability::Ready { .. }, located) => located,
            (availability, _) => return Err(unavailable(availability)),
        };
        match &spec.recipe {
            Recipe::Builtin => Ok(launch::builtin(spec)),
            Recipe::Binary { program, args, .. }
            | Recipe::QualifiedBinary { program, args, .. } => {
                let located = located.ok_or_else(|| LaunchError::Missing {
                    reason: format!("{program} was not found on your PATH."),
                })?;
                let mut launch = launch::binary(spec, &located.program, program, args);
                launch.args.splice(0..0, located.prefix_args);
                launch.set_env.extend(located.set_env);
                Ok(launch)
            }
            Recipe::NativeBinary { user_binary, args } => {
                let located = located.ok_or_else(|| LaunchError::Missing {
                    reason: format!("{} was not found on your PATH.", user_binary.program),
                })?;
                Ok(launch::binary(
                    spec,
                    &located.program,
                    user_binary.program,
                    args,
                ))
            }
            Recipe::NpmAdapter {
                package,
                version,
                bin,
                user_binary,
                ..
            } => {
                let short = probe::short_name(user_binary);
                let Some(located) = located else {
                    return Err(LaunchError::Missing {
                        reason: format!("The {short} adapter can't start here."),
                    });
                };
                let node = located.node.clone().ok_or_else(|| LaunchError::Missing {
                    reason: "Node.js was not found on your PATH.".into(),
                })?;
                let folder = self
                    .adapters_dir
                    .join(install::folder_name(package, version));
                let entry = match install::installed_entry(&folder, package, version) {
                    Some(entry) => entry,
                    None => {
                        let _ = progress.send(format!(
                            "Downloading the {short} adapter ({package} {version}). This happens once and can take a few minutes."
                        ));
                        let env = self.env.get().await;
                        let npm =
                            find_program("npm", &env.vars).ok_or_else(|| LaunchError::Missing {
                                reason:
                                    "npm was not found; Ghost needs it once to download the adapter."
                                        .into(),
                            })?;
                        let job = InstallJob {
                            package,
                            version,
                            bin,
                            short_name: short.to_owned(),
                            folder: folder.clone(),
                            npm,
                            node: node.clone(),
                            env,
                            registry: Arc::clone(&self.registry),
                        };
                        let entry = self.install(job).await?;
                        self.set_needs_download(spec, false);
                        entry
                    }
                };
                Ok(launch::adapter(
                    spec,
                    bin,
                    &node,
                    &entry,
                    &folder,
                    user_binary,
                    &located.program,
                ))
            }
        }
    }

    /// The install of this adapter version in flight, or a new one; waits for it.
    async fn install(&self, job: InstallJob) -> Result<PathBuf, LaunchError> {
        let key = format!("{}@{}", job.package, job.version);
        for _ in 0..ATTEMPTS {
            let install: Install = {
                let mut state = lock(&self.state);
                match state.installs.get(&key) {
                    // A finished install is never joined: the next start checks the marker itself.
                    Some(running) if running.is_running() => running.clone(),
                    _ => {
                        let install = SharedTask::spawn(&self.rt, install::install(job.clone()));
                        state.installs.insert(key.clone(), install.clone());
                        install
                    }
                }
            };
            if let Some(result) = install.wait().await {
                return result;
            }
        }
        Err(LaunchError::InstallFailed {
            reason: format!(
                "Could not download the {} adapter: the download was interrupted.",
                job.short_name
            ),
        })
    }

    fn forget_install(&self, spec: &AgentSpec) {
        let Recipe::NpmAdapter {
            package, version, ..
        } = &spec.recipe
        else {
            return;
        };
        let folder = self
            .adapters_dir
            .join(install::folder_name(package, version));
        if let Err(error) = install::forget(&folder) {
            tracing::warn!(agent = spec.id, %error, "could not delete an adapter's install marker");
        }
        self.set_needs_download(spec, true);
    }
}

/// An agent with fixed answers, for tests: no probes, no env, no installs.
///
/// Build one with [`FixedAgent::new`] and the `with_*` methods: the struct is `non_exhaustive`, so
/// later fields never break a test. By default `detect` answers `availability` at once,
/// `prepare_launch` answers `launch` at once, and nothing needs a download.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FixedAgent {
    pub spec: AgentSpec,
    /// What [`Detector::cached`] reports until a [`Detector::detect`] of this agent ends, and what
    /// `detect` answers unless `detected` is set.
    pub availability: Availability,
    /// What `detect` answers, and `cached` reports from then on: an agent that reads `Checking`
    /// at first and that detection finds `Ready`, `Missing` or `Broken`.
    pub detected: Option<Availability>,
    /// `None` for a built-in agent, or to make `prepare_launch` fail with the availability's reason.
    pub launch: Option<LaunchSpec>,
    /// `prepare_launch` fails with this instead, after its download details and its gate (an
    /// `InstallFailed` download, say).
    pub launch_error: Option<LaunchError>,
    /// The adapter is not installed yet: `AgentEntry::needs_download` reads true, and the next
    /// `prepare_launch` sends `progress` before it answers. A successful `prepare_launch` installs
    /// it, and `forget_install` on an npm adapter makes the download needed again.
    pub needs_download: bool,
    /// The status details a download sends through `prepare_launch`'s `progress`, in order.
    pub progress: Vec<String>,
    /// Holds every `detect` of this agent until the gate opens.
    pub detect_gate: Option<Gate>,
    /// Holds every `prepare_launch` of this agent, after its download details, until the gate
    /// opens.
    pub launch_gate: Option<Gate>,
}

impl FixedAgent {
    /// An agent that reports `availability` and starts with `launch` (`None` for a built-in agent,
    /// or to make `prepare_launch` fail with the availability's reason).
    pub fn new(
        spec: AgentSpec,
        availability: Availability,
        launch: Option<LaunchSpec>,
    ) -> FixedAgent {
        FixedAgent {
            spec,
            availability,
            detected: None,
            launch,
            launch_error: None,
            needs_download: false,
            progress: Vec::new(),
            detect_gate: None,
            launch_gate: None,
        }
    }

    /// `detect` answers `availability`, and `cached` reports it from then on.
    pub fn with_detected(mut self, availability: Availability) -> FixedAgent {
        self.detected = Some(availability);
        self
    }

    /// The adapter is not installed: the next `prepare_launch` sends `progress` first.
    pub fn with_download(mut self, progress: Vec<String>) -> FixedAgent {
        self.needs_download = true;
        self.progress = progress;
        self
    }

    /// `prepare_launch` fails with `error`.
    pub fn with_launch_error(mut self, error: LaunchError) -> FixedAgent {
        self.launch_error = Some(error);
        self
    }

    /// `detect` waits until `gate` opens.
    pub fn with_detect_gate(mut self, gate: Gate) -> FixedAgent {
        self.detect_gate = Some(gate);
        self
    }

    /// `prepare_launch` waits until `gate` opens, after its download details.
    pub fn with_launch_gate(mut self, gate: Gate) -> FixedAgent {
        self.launch_gate = Some(gate);
        self
    }
}

/// Holds a fixed agent's detection or launch preparation until the test opens it, so a test can
/// act while a start is still preparing (an Esc during a download, say). Clones share one gate, and
/// it stays open once opened.
#[derive(Debug, Clone)]
pub struct Gate {
    inner: Arc<GateInner>,
}

#[derive(Debug)]
struct GateInner {
    open: watch::Sender<bool>,
    /// Calls waiting at the gate now.
    waiting: watch::Sender<usize>,
}

impl Gate {
    pub fn new() -> Gate {
        Gate {
            inner: Arc::new(GateInner {
                open: watch::Sender::new(false),
                waiting: watch::Sender::new(0),
            }),
        }
    }

    /// Lets every waiting call through, and every later one.
    pub fn open(&self) {
        self.inner.open.send_replace(true);
    }

    /// How many calls wait at the gate now. A call whose future was dropped while it waited (a
    /// start that was interrupted) no longer counts.
    pub fn waiting(&self) -> usize {
        *self.inner.waiting.borrow()
    }

    /// Resolves once exactly `count` calls wait at the gate: 1 when a preparation reached it, 0 once
    /// it passed or was dropped.
    pub async fn until_waiting(&self, count: usize) {
        let mut waiting = self.inner.waiting.subscribe();
        // The gate holds the sender, so the channel stays open while `self` lives.
        let _ = waiting.wait_for(|now| *now == count).await;
    }

    /// Waits until the gate is open, counted as waiting meanwhile.
    async fn pass(&self) {
        let mut open = self.inner.open.subscribe();
        if *open.borrow() {
            return;
        }
        self.inner.waiting.send_modify(|now| *now += 1);
        let _leave = Leave(&self.inner.waiting);
        let _ = open.wait_for(|open| *open).await;
    }
}

impl Default for Gate {
    fn default() -> Gate {
        Gate::new()
    }
}

/// Stops counting a call as waiting once it passes the gate or its future is dropped.
struct Leave<'a>(&'a watch::Sender<usize>);

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|now| *now = now.saturating_sub(1));
    }
}

/// A fixed agent and what its answers have done so far.
#[derive(Debug)]
struct Fixed {
    agent: FixedAgent,
    state: Mutex<FixedState>,
}

#[derive(Debug, Clone, Copy)]
struct FixedState {
    /// A `detect` ended: `cached` reports the detected availability.
    detected: bool,
    needs_download: bool,
}

impl Fixed {
    fn new(agent: FixedAgent) -> Fixed {
        let state = FixedState {
            detected: false,
            needs_download: agent.needs_download,
        };
        Fixed {
            agent,
            state: Mutex::new(state),
        }
    }

    /// The availability detection ends with.
    fn detected(&self) -> &Availability {
        self.agent
            .detected
            .as_ref()
            .unwrap_or(&self.agent.availability)
    }

    fn entry(&self) -> AgentEntry {
        let state = *lock(&self.state);
        let availability = if state.detected {
            self.detected()
        } else {
            &self.agent.availability
        };
        entry(&self.agent.spec, availability.clone(), state.needs_download)
    }

    async fn detect(&self) -> Availability {
        if let Some(gate) = &self.agent.detect_gate {
            gate.pass().await;
        }
        lock(&self.state).detected = true;
        self.detected().clone()
    }

    async fn prepare_launch(
        &self,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<LaunchSpec, LaunchError> {
        let downloading = lock(&self.state).needs_download;
        if downloading {
            for detail in &self.agent.progress {
                // A start that no longer listens drops this future next.
                let _ = progress.send(detail.clone());
            }
        }
        if let Some(gate) = &self.agent.launch_gate {
            gate.pass().await;
        }
        let launch = self.launch()?;
        if downloading {
            lock(&self.state).needs_download = false;
        }
        Ok(launch)
    }

    fn launch(&self) -> Result<LaunchSpec, LaunchError> {
        if let Some(error) = &self.agent.launch_error {
            return Err(error.clone());
        }
        match (&self.agent.launch, &self.agent.spec.recipe) {
            (Some(launch), _) => Ok(launch.clone()),
            (None, Recipe::Builtin) => Ok(launch::builtin(&self.agent.spec)),
            (None, _) => Err(unavailable(self.detected())),
        }
    }
}

impl Detector {
    /// Desktop opt-in at real startup. Fixed detectors and test machines never start network work.
    /// Release checks are independent of launch readiness and never install anything.
    pub fn enable_release_checks(&self, cache_path: PathBuf) {
        let Mode::Live(live) = &self.mode else {
            return;
        };
        let Ok(cache) = crate::releases::ReleaseCache::new(cache_path) else {
            return;
        };
        {
            let mut state = lock(&live.state);
            if state.release_checks_enabled {
                return;
            }
            state.release_checks_enabled = true;
        }
        let weak = Arc::downgrade(live);
        live.rt.spawn(async move {
            cache.load().await;
            loop {
                {
                    let Some(live) = weak.upgrade() else {
                        break;
                    };
                    live.detect_every(false).await;
                    let installations: Vec<_> = lock(&live.state)
                        .results
                        .iter()
                        .filter_map(|(id, (_, detection))| {
                            Some((id.clone(), detection.installation.clone()?))
                        })
                        .collect();
                    for (id, installation) in installations {
                        let Some(source) =
                            crate::releases::ReleaseSource::for_installation(&installation)
                        else {
                            continue;
                        };
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |time| {
                                i64::try_from(time.as_millis()).unwrap_or(i64::MAX)
                            });
                        let latest = cache.refresh(&source, now).await;
                        lock(&live.state).releases.insert(id, latest);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
            }
        });
    }
    /// Reviewed actions for the currently detected installation, without running those actions.
    pub async fn maintenance(&self, id: &str) -> ghost_protocol::agents::AgentMaintenance {
        use ghost_protocol::agents::AgentMaintenance;
        let unavailable = |reason: String| AgentMaintenance {
            auth_status: false,
            login_available: false,
            update_unavailable: Some(reason),
        };
        let Mode::Live(live) = &self.mode else {
            return unavailable("This agent has no vendor maintenance commands.".into());
        };
        let Some(spec) = self.connection_for(id) else {
            return unavailable("This agent is not in the catalog.".into());
        };
        let Some(installation) = live.detect(&spec).await.installation else {
            return unavailable("The agent installation is not ready.".into());
        };
        let env = live.env.get().await;
        let update_unavailable = if lock(&live.state)
            .maintenance
            .contains(&installation.provider)
        {
            Some(crate::maintenance::MaintenanceError::AlreadyRunning.to_string())
        } else {
            crate::maintenance::update_command(&installation, &env.vars)
                .err()
                .map(|error| error.to_string())
        };
        AgentMaintenance {
            auth_status: crate::maintenance::auth_command(&installation).is_ok(),
            login_available: crate::maintenance::login_command(&installation).is_ok(),
            update_unavailable,
        }
    }

    /// Reload Ghost's choices and environment without waiting for catalog-wide detection.
    pub async fn prepare_plugin_refresh(&self) {
        self.reload_connections().await;
        if let Mode::Live(live) = &self.mode {
            selections::refresh_preferences(live).await;
            live.env.refresh();
        }
    }

    /// Reprobe one supported provider inside the core's bounded refresh stream. Other catalog
    /// providers cannot delay its listings. Connections of one provider are checked in turn.
    pub async fn refresh_plugin_provider(&self, id: &str) {
        if crate::plugins::recipes(id).is_empty()
            && !matches!(
                id,
                "opencode" | "hermes" | "grok" | "cursor" | "antigravity" | "omp"
            )
        {
            return;
        }
        match &self.mode {
            Mode::Live(live) => {
                let _slot = live.listing_slots.acquire().await;
                for spec in &live.specs {
                    if crate::installation::provider_namespace(spec.id) == id {
                        live.probe(spec, true).wait().await;
                    }
                }
            }
            Mode::Fixed { agents } => {
                for fixed in agents {
                    if crate::installation::provider_namespace(fixed.agent.spec.id) == id {
                        fixed.detect().await;
                    }
                }
            }
        }
    }

    /// Listings read with the provider's reviewed commands, shared with any refresh in flight.
    /// A changed installation version invalidates the cache and reports the version change.
    /// None means not installed or no reviewed listing commands.
    pub async fn plugin_listings(
        &self,
        id: &str,
        refresh: bool,
        now: ghost_protocol::Millis,
    ) -> Option<crate::plugins::AgentListings> {
        let Mode::Live(live) = &self.mode else {
            return None;
        };
        let provider = crate::installation::provider_namespace(id).to_owned();
        if crate::plugins::recipes(&provider).is_empty() {
            return None;
        }
        for _ in 0..ATTEMPTS {
            let (revision, minimum, detections) = {
                let state = lock(&live.state);
                (
                    state.configuration_revision,
                    state.minimum_generation.get(&provider).copied(),
                    provider_detection_generations(&state, &provider),
                )
            };
            let installation = match self.connection_for(id) {
                Some(spec) => live.detect(&spec).await.installation,
                None => None,
            };
            #[cfg(test)]
            {
                let gate = lock(&live.listing_admission_gate).take();
                if let Some(gate) = gate {
                    gate.pass().await;
                }
            }
            let (job, job_generation) = {
                let mut state = lock(&live.state);
                // Resolution can await a probe. Never label an old executable with a selection
                // revision published while that resolution was in progress.
                if state.configuration_revision != revision
                    || state.minimum_generation.get(&provider).copied() != minimum
                    || provider_detection_generations(&state, &provider) != detections
                {
                    continue;
                }
                let Some(installation) = installation else {
                    state.listings.remove(&provider);
                    state.listing_generation += 1;
                    let generation = state.listing_generation;
                    if let Some(job) = state.listing_jobs.get_mut(&provider) {
                        job.task.cancel();
                        // Retain its sequencing gate for a later reinstall, while making both
                        // admission and publication of this obsolete job impossible.
                        job.version.clear();
                        job.generation = generation;
                    }
                    return None;
                };
                if let Some(job) = state.listing_jobs.get(&provider)
                    && job.version == installation.version
                    && job.revision == revision
                    && job.minimum == minimum
                    && job.task.is_running()
                {
                    (job.task.clone(), job.generation)
                } else {
                    let cached = state.listings.get(&provider).cloned();
                    if crate::plugins::cache_answers(
                        cached.as_ref(),
                        &installation.version,
                        refresh,
                    ) {
                        return cached.map(|cached| crate::plugins::AgentListings {
                            version_change: None,
                            ..cached
                        });
                    }
                    let gate = state
                        .listing_jobs
                        .get(&provider)
                        .map(|previous| {
                            previous.task.cancel();
                            previous.gate.clone()
                        })
                        .unwrap_or_default();
                    state.listing_generation += 1;
                    let generation = state.listing_generation;
                    let owned = Arc::clone(live);
                    let namespace = provider.clone();
                    let installation = installation.clone();
                    let version = installation.version.clone();
                    let provider_gate = Arc::clone(&gate);
                    let task = SharedTask::spawn(&live.rt, async move {
                        let _provider = provider_gate.lock().await;
                        let _slot = owned.listing_slots.acquire().await.ok()?;
                        let mut listings = crate::plugins::run_listings(
                            &installation,
                            owned.env.get().await,
                            &owned.registry,
                        )
                        .await;
                        crate::plugins::retain_failed_entries(&mut listings, cached.as_ref());
                        let fresh = crate::plugins::AgentListings {
                            version_change: cached
                                .filter(|cached| cached.version != installation.version)
                                .map(|cached| ghost_protocol::plugins::VersionChange {
                                    from: cached.version,
                                    to: installation.version.clone(),
                                }),
                            version: installation.version,
                            updated_at: now,
                            listings,
                        };
                        let mut state = lock(&owned.state);
                        if state.configuration_revision != revision
                            || state.minimum_generation.get(&namespace).copied() != minimum
                            || state
                                .listing_jobs
                                .get(&namespace)
                                .is_none_or(|job| job.generation != generation)
                        {
                            return None;
                        }
                        state.listings.insert(namespace, fresh.clone());
                        Some(fresh)
                    });
                    state.listing_jobs.insert(
                        provider.clone(),
                        ListingJob {
                            version,
                            revision,
                            generation,
                            minimum,
                            task: task.clone(),
                            gate,
                        },
                    );
                    (task, generation)
                }
            };
            match job.wait().await {
                Some(result) => return result,
                // A replaced request may have just cancelled its last waiter. Do not join
                // that cancelled job permanently; the next attempt creates fresh work.
                None => {
                    let state = lock(&live.state);
                    if state.listing_jobs.get(&provider).is_none_or(|job| {
                        job.generation != job_generation
                            || job.revision != state.configuration_revision
                            || job.minimum != state.minimum_generation.get(&provider).copied()
                    }) {
                        return None;
                    }
                    continue;
                }
            }
        }
        None
    }

    /// Last public listing facts, without detection or CLI work.
    pub fn cached_plugin_listings(
        &self,
    ) -> std::collections::BTreeMap<String, crate::plugins::AgentListings> {
        match &self.mode {
            Mode::Live(live) => lock(&live.state)
                .listings
                .iter()
                .map(|(id, listing)| (id.clone(), listing.clone()))
                .collect(),
            Mode::Fixed { .. } => Default::default(),
        }
    }

    /// Runs the vendor's local status command and retains only a coarse status, never account data.
    pub async fn auth_status(&self, id: &str) -> ghost_protocol::agents::AgentAuthStatus {
        use ghost_protocol::agents::AgentAuthStatus;
        let Mode::Live(live) = &self.mode else {
            return AgentAuthStatus::Unsupported;
        };
        let Some(spec) = self.connection_for(id) else {
            return AgentAuthStatus::Unsupported;
        };
        let Some(installation) = live.detect(&spec).await.installation else {
            return AgentAuthStatus::Unknown {
                reason: "The agent installation is not ready.".into(),
            };
        };
        let command = match crate::maintenance::auth_command(&installation) {
            Ok(command) => command,
            Err(_) => return AgentAuthStatus::Unsupported,
        };
        if crate::maintenance::check_unchanged(&installation).is_err() {
            return AgentAuthStatus::Unknown {
                reason: "The installation changed. Refresh agents.".into(),
            };
        }
        match crate::maintenance::run(
            command,
            live.env.get().await,
            &live.registry,
            crate::PROBE_TIMEOUT,
        )
        .await
        {
            Ok(output) => crate::maintenance::auth_status(&installation.provider, &output),
            Err(error) => AgentAuthStatus::Unknown {
                reason: error.to_string(),
            },
        }
    }

    /// The installation `id` runs, detected again now, with the agent's name. A vendor terminal
    /// (sign-in, account token, update) starts from what is on disk: a failed update or a change
    /// of executable settings drops the cached detection on purpose, and a terminal that waited for
    /// someone to press Refresh never started (owner, 7 November 2026: "it doesn't even boot up
    /// the terminal"). The new detection is kept, so the agent shows as ready again too.
    pub async fn fresh_installation(
        &self,
        id: &str,
    ) -> Result<(String, ghost_protocol::agents::AgentInstallation), String> {
        let Mode::Live(live) = &self.mode else {
            return Err("This agent has no vendor commands.".into());
        };
        let spec = self
            .connection_for(id)
            .ok_or_else(|| format!("Ghost doesn't know an agent called \"{id}\"."))?;
        // Two updaters replacing one binary at once can leave it broken, and a sign-in started
        // mid-update runs a binary that is being replaced.
        if lock(&live.state)
            .maintenance
            .contains(crate::installation::provider_namespace(id))
        {
            return Err(format!(
                "{} is updating; try again when the update finishes.",
                spec.name
            ));
        }
        let interrupted = || format!("Checking {} was interrupted; try again.", spec.name);
        let detection = live
            .probe(&spec, true)
            .wait()
            .await
            .ok_or_else(interrupted)?;
        match detection.availability {
            Availability::Ready { .. } => detection
                .installation
                .map(|installation| (spec.name.to_owned(), installation))
                .ok_or_else(|| format!("Ghost can't run {}'s own commands.", spec.name)),
            Availability::Missing { reason } | Availability::Broken { reason } => Err(reason),
            Availability::Checking => Err(interrupted()),
        }
    }

    /// Explicit user action only. Serialize aliases of one provider and invalidate detection even
    /// on failure, because a vendor installer may have partially changed its files; a failed
    /// update then detects the agent again at once, so it does not sit unchecked until a refresh.
    pub async fn update(&self, id: &str) -> Result<(), crate::maintenance::MaintenanceError> {
        let result = self.update_once(id).await;
        if result.is_err()
            && let Mode::Live(live) = &self.mode
            && let Some(spec) = self.connection_for(id)
        {
            // The guard is gone: this probe's result is kept.
            drop(live.probe(&spec, true));
        }
        result
    }

    /// What the last update of `id`'s provider printed in this run of Ghost, when it printed
    /// anything. For the person who asked for the update; never stored or sent on.
    pub fn update_output(&self, id: &str) -> Option<String> {
        let Mode::Live(live) = &self.mode else {
            return None;
        };
        lock(&live.state)
            .update_output
            .get(crate::installation::provider_namespace(id))
            .filter(|text| !text.is_empty())
            .cloned()
    }

    async fn update_once(&self, id: &str) -> Result<(), crate::maintenance::MaintenanceError> {
        use crate::maintenance::{self, MaintenanceError};
        let Mode::Live(live) = &self.mode else {
            return Err(MaintenanceError::Unsupported);
        };
        let _selection = live
            .selection_gate
            .try_lock()
            .map_err(|_| MaintenanceError::AlreadyRunning)?;
        let spec = self
            .connection_for(id)
            .ok_or(MaintenanceError::Unsupported)?;
        let provider = crate::installation::provider_namespace(id).to_owned();
        if !lock(&live.state).maintenance.insert(provider.clone()) {
            return Err(MaintenanceError::AlreadyRunning);
        }
        let guard = MaintenanceGuard {
            live: Arc::clone(live),
            provider: provider.clone(),
            active: true,
        };
        // An earlier update's output says nothing about this one.
        lock(&live.state).update_output.remove(&provider);
        let installation = live
            .probe(&spec, true)
            .wait()
            .await
            .and_then(|detected| detected.installation)
            .ok_or(MaintenanceError::Changed)?;
        let env = live.env.get().await;
        let command = maintenance::update_command(&installation, &env.vars)?;
        maintenance::check_unchanged(&installation)?;
        // Drop cached aliases before starting so cancellation cannot leave stale metadata.
        {
            let mut state = lock(&live.state);
            state.generation += 1;
            let floor = state.generation;
            state.minimum_generation.insert(provider.clone(), floor);
            state
                .results
                .retain(|id, _| crate::installation::provider_namespace(id) != provider);
            state
                .probes
                .retain(|id, _| crate::installation::provider_namespace(id) != provider);
        }
        let output = maintenance::run(command, env, &live.registry, crate::INSTALL_TIMEOUT).await?;
        // Kept whatever the result: a failure's reason, and a "success" that changed nothing, are
        // both in what the command said.
        lock(&live.state)
            .update_output
            .insert(provider.clone(), maintenance::readable_output(&output));
        if output.timed_out {
            return Err(MaintenanceError::TimedOut);
        }
        let code = output.exit.and_then(|exit| exit.code);
        if code != Some(0) {
            return Err(MaintenanceError::Failed(code));
        }
        // Keep the provider lock through verification. Releasing it first would let another
        // updater replace the binary while this check is still deciding whether it succeeded.
        let detection = live.probe(&spec, true).wait().await;
        let Some(detection) = detection
            .filter(|detected| matches!(detected.availability, Availability::Ready { .. }))
        else {
            return Err(MaintenanceError::VerificationFailed);
        };
        guard.complete(&spec, detection);
        Ok(())
    }
    /// Detects the catalog's agents. Detection starts in the background on `rt` at once and probes
    /// once the env resolves; `adapters_dir` is `<data root>/adapters`.
    pub fn new(
        env: Arc<EnvResolver>,
        registry: Arc<LeftoverRegistry>,
        adapters_dir: PathBuf,
        rt: Handle,
    ) -> Arc<Detector> {
        Self::create(env, registry, adapters_dir, rt, None)
    }

    /// Device-local app preferences are loaded before any startup probe. Tests and CLI callers
    /// using `new` retain an isolated, non-persistent detector.
    pub fn with_preferences(
        env: Arc<EnvResolver>,
        registry: Arc<LeftoverRegistry>,
        adapters_dir: PathBuf,
        rt: Handle,
        preferences: PathBuf,
    ) -> Arc<Detector> {
        Self::create(env, registry, adapters_dir, rt, Some(preferences))
    }

    fn create(
        env: Arc<EnvResolver>,
        registry: Arc<LeftoverRegistry>,
        adapters_dir: PathBuf,
        rt: Handle,
        preferences: Option<PathBuf>,
    ) -> Arc<Detector> {
        let mut state = LiveState::default();
        if let Some(path) = &preferences {
            match crate::preferences::read(path) {
                Ok(selections) => state.selections = selections,
                Err(error) => state.preference_error = Some(error),
            }
        }
        let connection_path = preferences
            .as_ref()
            .map(|path| path.with_file_name(crate::connection_choices::FILE));
        let connection_choices = connection_path
            .as_deref()
            .map(|path| {
                crate::connection_choices::read(path).unwrap_or_else(|error| {
                    tracing::warn!(%error, "connection choices unreadable; every agent uses Automatic");
                    Default::default()
                })
            })
            .unwrap_or_default();
        let live = Arc::new(Live {
            specs: catalog().to_vec(),
            env,
            registry,
            adapters_dir,
            rt: rt.clone(),
            state: Mutex::new(state),
            preferences,
            selection_gate: tokio::sync::Mutex::new(()),
            listing_slots: tokio::sync::Semaphore::new(2),
            #[cfg(test)]
            listing_admission_gate: Mutex::new(None),
        });
        let background = Arc::clone(&live);
        rt.spawn(async move { background.detect_every(false).await });
        if live.preferences.is_some() {
            selections::observe(&live);
        }
        Arc::new(Detector {
            mode: Mode::Live(live),
            forgotten: Mutex::new(Vec::new()),
            connection_choices: Mutex::new(connection_choices),
            connection_path,
        })
    }

    /// Tests: these agents, in this order, with fixed answers (see [`FixedAgent`]); no probes, no
    /// env, no installs. Connection settings live in memory only.
    pub fn fixed(agents: Vec<FixedAgent>) -> Arc<Detector> {
        Arc::new(Detector {
            mode: Mode::Fixed {
                agents: agents.into_iter().map(Fixed::new).collect(),
            },
            forgotten: Mutex::new(Vec::new()),
            connection_choices: Mutex::new(Default::default()),
            connection_path: None,
        })
    }

    /// The connection with exactly this id (what a saved native session records). The session
    /// manager looks connections and agents up here ([`agent`](Self::agent)), never in
    /// `catalog()`, so tests can inject their own.
    pub fn spec(&self, id: &str) -> Option<AgentSpec> {
        match &self.mode {
            Mode::Live(live) => live.specs.iter().find(|spec| spec.id == id).cloned(),
            Mode::Fixed { agents } => find(agents, id).map(|fixed| fixed.agent.spec.clone()),
        }
    }

    /// Whether `spec` can start here. Cached; waits for the env resolution and the probe in flight,
    /// which it shares with every other caller, so it answers `Ready`, `Missing` or `Broken`, never
    /// `Checking`. A fixed agent answers its `detected` availability, or `availability` without one.
    pub async fn detect(&self, spec: &AgentSpec) -> Availability {
        match &self.mode {
            Mode::Live(live) => live.detect(spec).await.availability,
            Mode::Fixed { agents } => match find(agents, spec.id) {
                Some(fixed) => fixed.detect().await,
                None => Availability::Missing {
                    reason: unknown(spec),
                },
            },
        }
    }

    /// Every agent, one entry each, in catalog order (a fixed detector's in the order given).
    ///
    /// `refresh: false` answers at once with [`cached`](Self::cached), never waiting: an agent whose
    /// first detection has not ended reads `Checking`, and detection runs in the background from
    /// [`new`](Self::new) on, once the env resolves. `refresh: true` resolves the env again, probes
    /// every connection, and waits for the results; a fixed detector runs `detect` for each agent in
    /// turn.
    pub async fn detect_all(&self, refresh: bool) -> Vec<AgentEntry> {
        if refresh {
            self.reload_connections().await;
            match &self.mode {
                Mode::Fixed { agents } => {
                    for fixed in agents {
                        fixed.detect().await;
                    }
                }
                Mode::Live(live) => {
                    selections::refresh_preferences(live).await;
                    live.env.refresh();
                    live.detect_every(true).await;
                }
            }
        }
        self.cached()
    }

    /// The last results, one entry per agent, without waiting: `Checking` until an agent's first
    /// detection ends. An agent with several connections shows the one a new session uses (see
    /// the `agents` module for the rule) and lists them all in `connections`.
    pub fn cached(&self) -> Vec<AgentEntry> {
        self.agent_entries()
    }

    /// Installs an npm adapter when it is not installed (one shared attempt per package; `progress`
    /// receives status details), then builds the launch spec. Dropping the future kills a running
    /// install.
    pub async fn prepare_launch(
        &self,
        spec: &AgentSpec,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<LaunchSpec, LaunchError> {
        match &self.mode {
            Mode::Fixed { agents } => match find(agents, spec.id) {
                Some(fixed) => fixed.prepare_launch(progress).await,
                None => Err(LaunchError::Missing {
                    reason: unknown(spec),
                }),
            },
            Mode::Live(live) => live.prepare_launch(spec, progress).await,
        }
    }

    /// Deletes an adapter's install marker, so the next start installs it again (a start failed on
    /// a missing module). Every call is recorded ([`forgotten`](Self::forgotten)); a fixed npm
    /// adapter needs its download again.
    pub fn forget_install(&self, spec: &AgentSpec) {
        lock(&self.forgotten).push(spec.id.to_owned());
        match &self.mode {
            Mode::Live(live) => live.forget_install(spec),
            Mode::Fixed { agents } => {
                if let Some(fixed) = find(agents, spec.id)
                    && matches!(fixed.agent.spec.recipe, Recipe::NpmAdapter { .. })
                {
                    lock(&fixed.state).needs_download = true;
                }
            }
        }
    }

    /// Tests: the agent of every [`forget_install`](Self::forget_install) so far, in order.
    pub fn forgotten(&self) -> Vec<String> {
        lock(&self.forgotten).clone()
    }
}

fn find<'a>(agents: &'a [Fixed], id: &str) -> Option<&'a Fixed> {
    agents.iter().find(|fixed| fixed.agent.spec.id == id)
}

/// The reason an agent the detector does not know cannot start.
fn unknown(spec: &AgentSpec) -> String {
    format!("Ghost doesn't know an agent called \"{}\".", spec.id)
}

/// The error for an agent that cannot start, from its availability.
fn unavailable(availability: &Availability) -> LaunchError {
    match availability {
        Availability::Missing { reason } => LaunchError::Missing {
            reason: reason.clone(),
        },
        Availability::Broken { reason } => LaunchError::Broken {
            reason: reason.clone(),
        },
        Availability::Checking | Availability::Ready { .. } => LaunchError::Missing {
            reason: "this test agent has no launch spec".into(),
        },
    }
}

/// An agent as the New chat screen lists it.
fn entry(spec: &AgentSpec, availability: Availability, needs_download: bool) -> AgentEntry {
    AgentEntry {
        id: spec.id.to_owned(),
        name: spec.name.to_owned(),
        description: spec.description.to_owned(),
        availability,
        launch: launch_label(spec),
        needs_download,
        last_verified: spec.last_verified.to_owned(),
        installation: None,
        latest_release: None,
        harness: None,
        connections: None,
        terminal: crate::catalog::has_terminal(spec.agent),
    }
}

/// How the agent starts, for display.
fn launch_label(spec: &AgentSpec) -> String {
    match &spec.recipe {
        Recipe::Binary { program, args, .. } | Recipe::QualifiedBinary { program, args, .. } => {
            std::iter::once(*program)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ")
        }
        Recipe::NativeBinary { user_binary, args } => std::iter::once(user_binary.program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" "),
        Recipe::NpmAdapter {
            package,
            version,
            user_binary,
            ..
        } => format!("{package} {version} with your {}", user_binary.program),
        Recipe::Builtin => BUILT_IN.into(),
    }
}

/// Locks `mutex`, recovering the data of a thread that panicked while holding it: a test double
/// must keep answering after a test's own panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ghost_process::{EnvMap, EnvSource, ProcessCwd, ResolvedEnv};

    use super::*;
    use crate::catalog::get;

    fn live() -> Arc<Detector> {
        let env = EnvResolver::fixed(ResolvedEnv {
            vars: EnvMap::new(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        });
        Detector::new(
            env,
            LeftoverRegistry::disabled(),
            PathBuf::from("/data/adapters"),
            Handle::current(),
        )
    }

    #[tokio::test]
    async fn listing_admission_retries_an_installation_resolved_before_selection_changed() {
        let detector = live();
        detector.detect_all(true).await;
        let Mode::Live(live) = &detector.mode else {
            panic!("live detector");
        };
        let temp = tempfile::tempdir().unwrap();
        let old_path = temp.path().join("old-claude");
        let new_path = temp.path().join("new-claude");
        let old =
            crate::installation::native("claude", "2.1.310", old_path.to_str().unwrap()).unwrap();
        let mut new =
            crate::installation::native("claude", "2.1.312", new_path.to_str().unwrap()).unwrap();
        // An unsupported host gives the new result a distinct, deterministic listing failure
        // before any helper starts. These synthetic installations never execute a real CLI.
        new.host = ghost_protocol::agents::AgentHost::Unsupported;
        let detection = |installation: ghost_protocol::agents::AgentInstallation| Detection {
            availability: Availability::Ready {
                version: Some(installation.version.clone()),
                path: installation.launch_path.clone(),
            },
            installation: Some(installation),
            needs_download: false,
            located: None,
        };
        {
            let mut state = lock(&live.state);
            let generation = state.generation;
            state
                .results
                .insert("claude".into(), (generation, detection(old)));
        }
        let gate = Gate::new();
        *lock(&live.listing_admission_gate) = Some(gate.clone());
        let listing = tokio::spawn({
            let detector = detector.clone();
            async move { detector.plugin_listings("claude", true, 1).await.unwrap() }
        });
        gate.until_waiting(1).await;
        assert!(selections::replace_selections(
            live,
            Ok([("claude".into(), new_path)].into())
        ));
        {
            let mut state = lock(&live.state);
            let generation = state.generation;
            state
                .results
                .insert("claude".into(), (generation, detection(new)));
        }
        gate.open();
        let result = listing.await.unwrap();
        assert_eq!(
            result.version, "2.1.312",
            "the old installation cannot be admitted under the new revision"
        );
        assert!(
            result
                .listings
                .iter()
                .all(|listing| matches!(&listing.state,
            ghost_protocol::plugins::ListingState::Failed { reason }
                if reason == "Ghost lists plugins only for agents installed on this computer."))
        );
        assert_eq!(detector.cached_plugin_listings()["claude"], result);
    }

    #[tokio::test]
    async fn an_older_missing_resolution_cannot_erase_a_reinstalled_provider_cache() {
        let detector = live();
        detector.detect_all(true).await;
        let Mode::Live(live) = &detector.mode else {
            panic!("live detector");
        };
        let gate = Gate::new();
        *lock(&live.listing_admission_gate) = Some(gate.clone());
        let older = tokio::spawn({
            let detector = detector.clone();
            async move { detector.plugin_listings("claude", false, 1).await }
        });
        gate.until_waiting(1).await;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("new-claude");
        let mut installation =
            crate::installation::native("claude", "2.1.312", path.to_str().unwrap()).unwrap();
        installation.host = ghost_protocol::agents::AgentHost::Unsupported;
        {
            let mut state = lock(&live.state);
            state.generation += 1;
            let generation = state.generation;
            state.results.insert(
                "claude".into(),
                (
                    generation,
                    Detection {
                        availability: Availability::Ready {
                            version: Some(installation.version.clone()),
                            path: installation.launch_path.clone(),
                        },
                        installation: Some(installation),
                        needs_download: false,
                        located: None,
                    },
                ),
            );
        }
        // A newer reprobe and listing complete while the older missing result is paused. The
        // synthetic unsupported host finishes without launching any CLI or reading agent files.
        let fresh = detector.plugin_listings("claude", true, 2).await.unwrap();
        assert_eq!(fresh.version, "2.1.312");
        gate.open();
        assert_eq!(
            older.await.unwrap(),
            Some(fresh.clone()),
            "the stale None must retry current facts instead of clearing the newer result"
        );
        assert_eq!(detector.cached_plugin_listings()["claude"], fresh);
    }

    fn progress() -> mpsc::UnboundedSender<String> {
        mpsc::unbounded_channel().0
    }

    fn ready(path: &str) -> Availability {
        Availability::Ready {
            version: Some("1.0.0".into()),
            path: path.into(),
        }
    }

    fn claude() -> AgentSpec {
        get("claude-acp").expect("claude").clone()
    }

    fn claude_launch() -> LaunchSpec {
        LaunchSpec {
            label: "claude-agent-acp".into(),
            program: PathBuf::from("/usr/local/bin/node"),
            args: vec!["/data/adapters/claude/entry.js".into()],
            cwd: ProcessCwd::Fixed(PathBuf::from("/data/adapters/claude")),
            set_env: Vec::new(),
            daemons: Vec::new(),
            startup_timeout: Duration::from_secs(60),
        }
    }

    const DOWNLOADING: &str = "Downloading the Claude adapter (@agentclientprotocol/claude-agent-acp 0.95.0). This happens once and can take a few minutes.";

    fn needs_download(detector: &Detector) -> bool {
        detector.cached()[0].needs_download
    }

    /// With nothing on `PATH`, detection finds nothing to run and probes nothing.
    #[tokio::test]
    async fn a_live_detector_with_an_empty_path_reports_why_each_agent_is_missing() {
        let detector = live();
        let entries = detector.detect_all(true).await;
        // One entry per agent: the adapters, text fallbacks and OpenCode's 2.x line are
        // connections of their agents, not agents of their own.
        let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "hermes",
                "claude",
                "codex",
                "antigravity",
                "cursor",
                "grok",
                "omp",
                "opencode",
                "pi",
                "mock"
            ]
        );
        for entry in entries.iter().filter(|entry| {
            matches!(
                entry.id.as_str(),
                "antigravity" | "cursor" | "grok" | "omp" | "opencode"
            )
        }) {
            assert!(
                matches!(&entry.availability, Availability::Missing { reason } if reason.contains("official"))
            );
        }
        let reasons: Vec<Availability> = entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.id.as_str(),
                    "hermes" | "claude" | "codex" | "pi" | "mock"
                )
            })
            .map(|entry| entry.availability.clone())
            .collect();
        // Nothing is installed: each agent says so with its preferred connection's reason.
        assert_eq!(
            reasons,
            [
                Availability::Missing {
                    reason: "hermes was not found on your PATH.".into()
                },
                Availability::Missing {
                    reason: "claude was not found on your PATH.".into()
                },
                Availability::Missing {
                    reason: "codex was not found on your PATH.".into()
                },
                Availability::Missing {
                    reason: "pi was not found on your PATH.".into()
                },
                Availability::Ready {
                    version: None,
                    path: BUILT_IN.into()
                },
            ]
        );
        assert!(entries.iter().all(|entry| !entry.needs_download));
        assert_eq!(entries[0].launch, "hermes acp");
        assert_eq!(entries[0].connections, None, "one connection");
        let agent = &entries[1];
        assert_eq!(agent.name, "Claude Code");
        let connections = agent.connections.as_ref().expect("Claude has two");
        assert_eq!(
            (
                connections.preferred.as_deref(),
                connections.active.as_deref()
            ),
            (None, None)
        );
        let options: Vec<(&str, &str, &str)> = connections
            .options
            .iter()
            .map(|option| {
                (
                    option.id.as_str(),
                    option.label.as_str(),
                    option.launch.as_str(),
                )
            })
            .collect();
        assert_eq!(
            options,
            [
                ("claude", "Direct", agent.launch.as_str()),
                (
                    "claude-acp",
                    "ACP adapter",
                    "@agentclientprotocol/claude-agent-acp 0.95.0 with your claude"
                ),
            ]
        );
        assert_eq!(
            connections.options[1].availability,
            Availability::Missing {
                reason: "Node.js 22 or newer is required (found none).".into()
            }
        );
        assert_eq!(detector.cached(), entries);

        let mock = detector.spec("mock").expect("mock");
        let launch = detector
            .prepare_launch(&mock, progress())
            .await
            .expect("the demo agent starts");
        assert_eq!(launch.label, "mock");
        assert_eq!(launch.program, PathBuf::new());
        assert_eq!(launch.cwd, ProcessCwd::ChatFolder);
        assert_eq!(launch.startup_timeout, Duration::from_secs(5));

        let hermes = detector.spec("hermes").expect("hermes");
        assert_eq!(
            detector.prepare_launch(&hermes, progress()).await,
            Err(LaunchError::Missing {
                reason: "hermes was not found on your PATH.".into()
            })
        );
        assert!(matches!(
            detector.detect(&hermes).await,
            Availability::Missing { .. }
        ));
        assert!(detector.spec("cursor").is_some());
        assert!(detector.spec("agent").is_none());
        detector.forget_install(&claude());
        assert_eq!(detector.forgotten(), ["claude-acp"]);
    }

    #[tokio::test]
    async fn the_demo_agent_is_ready_before_any_detection() {
        let detector = live();
        let entries = detector.cached();
        assert!(matches!(
            entries
                .iter()
                .find(|entry| entry.id == "mock")
                .expect("demo agent")
                .availability,
            Availability::Ready { .. }
        ));
    }

    #[tokio::test]
    async fn fixed_agents_answer_as_given() {
        let hermes = get("hermes").expect("hermes").clone();
        let launch = LaunchSpec {
            label: "hermes acp".into(),
            program: PathBuf::from("/opt/bin/hermes"),
            args: vec!["acp".into()],
            cwd: ProcessCwd::ChatFolder,
            set_env: Vec::new(),
            daemons: hermes.daemons.clone(),
            startup_timeout: hermes.startup_timeout,
        };
        let broken = AgentSpec {
            id: "broken",
            ..get("codex-acp").expect("codex").clone()
        };
        let detector = Detector::fixed(vec![
            FixedAgent::new(
                hermes.clone(),
                Availability::Ready {
                    version: Some("0.22.5".into()),
                    path: "/opt/bin/hermes".into(),
                },
                Some(launch.clone()),
            ),
            FixedAgent::new(
                broken.clone(),
                Availability::Broken {
                    reason: "Ghost needs Codex 0.160.1 or newer (found 0.1.0).".into(),
                },
                None,
            ),
            FixedAgent::new(
                get("mock").expect("mock").clone(),
                Availability::Checking,
                None,
            ),
        ]);
        assert_eq!(detector.spec("hermes"), Some(hermes.clone()));
        assert!(
            detector.spec("claude-acp").is_none(),
            "only the given agents"
        );
        assert_eq!(
            detector.prepare_launch(&hermes, progress()).await,
            Ok(launch)
        );
        assert_eq!(
            detector.prepare_launch(&broken, progress()).await,
            Err(LaunchError::Broken {
                reason: "Ghost needs Codex 0.160.1 or newer (found 0.1.0).".into()
            })
        );
        let mock = detector.spec("mock").expect("mock");
        assert_eq!(
            detector
                .prepare_launch(&mock, progress())
                .await
                .map(|launch| launch.label),
            Ok("mock".to_owned())
        );
        let entries = detector.detect_all(true).await;
        let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["hermes", "broken", "mock"]);
        assert!(entries.iter().all(|entry| !entry.needs_download));
        assert_eq!(detector.detect(&mock).await, Availability::Checking);
        let stranger = AgentSpec {
            id: "stranger",
            ..mock
        };
        assert!(matches!(
            detector.prepare_launch(&stranger, progress()).await,
            Err(LaunchError::Missing { .. })
        ));
        assert!(matches!(
            detector.detect(&stranger).await,
            Availability::Missing { .. }
        ));
        detector.forget_install(&broken);
        detector.forget_install(&hermes);
        assert_eq!(detector.forgotten(), ["broken", "hermes"]);
        let downloads: Vec<(String, bool)> = detector
            .cached()
            .into_iter()
            .map(|entry| (entry.id, entry.needs_download))
            .collect();
        assert_eq!(
            downloads,
            [
                ("hermes".to_owned(), false),
                ("broken".to_owned(), true),
                ("mock".to_owned(), false)
            ],
            "only an npm adapter has an install to forget"
        );
    }

    /// `Checking` proceeds at create time, and the start waits for detection, which may find the
    /// agent ready or missing.
    #[tokio::test]
    async fn a_fixed_agent_reads_checking_until_detection_ends() {
        let detector = Detector::fixed(vec![
            FixedAgent::new(claude(), Availability::Checking, Some(claude_launch()))
                .with_detected(ready("/usr/local/bin/claude")),
        ]);
        assert_eq!(detector.cached()[0].availability, Availability::Checking);
        assert_eq!(detector.detect_all(false).await, detector.cached());
        assert_eq!(
            detector.detect(&claude()).await,
            ready("/usr/local/bin/claude")
        );
        assert_eq!(
            detector.cached()[0].availability,
            ready("/usr/local/bin/claude")
        );

        let missing = Availability::Missing {
            reason: "The Claude adapter drives your own Claude Code, and claude was not found on your PATH.".into(),
        };
        let detector = Detector::fixed(vec![
            FixedAgent::new(claude(), Availability::Checking, None).with_detected(missing.clone()),
        ]);
        let entries = detector.detect_all(true).await;
        assert_eq!(entries[0].availability, missing);
        assert_eq!(
            detector.prepare_launch(&claude(), progress()).await,
            Err(LaunchError::Missing {
                reason: "The Claude adapter drives your own Claude Code, and claude was not found on your PATH.".into()
            }),
            "a failed launch reports the detected reason"
        );
    }

    #[tokio::test]
    async fn a_download_sends_its_details_once_and_forget_install_needs_it_again() {
        let detector = Detector::fixed(vec![
            FixedAgent::new(
                claude(),
                ready("/usr/local/bin/claude"),
                Some(claude_launch()),
            )
            .with_download(vec![DOWNLOADING.into()]),
        ]);
        assert!(needs_download(&detector));

        let (details, mut received) = mpsc::unbounded_channel();
        assert_eq!(
            detector.prepare_launch(&claude(), details.clone()).await,
            Ok(claude_launch())
        );
        assert_eq!(received.try_recv().as_deref(), Ok(DOWNLOADING));
        assert!(received.try_recv().is_err(), "one detail");
        assert!(!needs_download(&detector), "installed");

        assert_eq!(
            detector.prepare_launch(&claude(), details.clone()).await,
            Ok(claude_launch())
        );
        assert!(received.try_recv().is_err(), "no second download");

        detector.forget_install(&claude());
        assert_eq!(detector.forgotten(), ["claude-acp"]);
        assert!(needs_download(&detector));
        assert_eq!(
            detector.prepare_launch(&claude(), details).await,
            Ok(claude_launch())
        );
        assert_eq!(
            received.try_recv().as_deref(),
            Ok(DOWNLOADING),
            "Retry downloads again"
        );
    }

    #[tokio::test]
    async fn a_failed_download_installs_nothing() {
        let failure = LaunchError::InstallFailed {
            reason: "Could not download the Claude adapter: npm error code E404".into(),
        };
        let detector = Detector::fixed(vec![
            FixedAgent::new(
                claude(),
                ready("/usr/local/bin/claude"),
                Some(claude_launch()),
            )
            .with_download(vec![DOWNLOADING.into()])
            .with_launch_error(failure.clone()),
        ]);
        let (details, mut received) = mpsc::unbounded_channel();
        assert_eq!(
            detector.prepare_launch(&claude(), details).await,
            Err(failure)
        );
        assert_eq!(received.try_recv().as_deref(), Ok(DOWNLOADING));
        assert!(needs_download(&detector), "still not installed");
    }

    #[tokio::test]
    async fn gates_hold_detection_and_preparation_until_opened() {
        let detect_gate = Gate::new();
        let launch_gate = Gate::default();
        let detector = Detector::fixed(vec![
            FixedAgent::new(claude(), Availability::Checking, Some(claude_launch()))
                .with_detected(ready("/usr/local/bin/claude"))
                .with_download(vec![DOWNLOADING.into()])
                .with_detect_gate(detect_gate.clone())
                .with_launch_gate(launch_gate.clone()),
        ]);

        let detecting = tokio::spawn({
            let detector = Arc::clone(&detector);
            async move { detector.detect(&claude()).await }
        });
        detect_gate.until_waiting(1).await;
        assert_eq!(detector.cached()[0].availability, Availability::Checking);
        assert!(!detecting.is_finished());
        detect_gate.open();
        assert_eq!(
            detecting.await.expect("join"),
            ready("/usr/local/bin/claude")
        );
        assert_eq!(detect_gate.waiting(), 0);
        assert_eq!(
            detector.detect(&claude()).await,
            ready("/usr/local/bin/claude"),
            "an open gate stays open"
        );

        // An Esc during the download drops the preparation at the gate: nothing is installed.
        let (details, mut received) = mpsc::unbounded_channel();
        let preparing = tokio::spawn({
            let detector = Arc::clone(&detector);
            let details = details.clone();
            async move { detector.prepare_launch(&claude(), details).await }
        });
        launch_gate.until_waiting(1).await;
        assert_eq!(received.recv().await.as_deref(), Some(DOWNLOADING));
        preparing.abort();
        launch_gate.until_waiting(0).await;
        assert!(preparing.await.expect_err("aborted").is_cancelled());
        assert!(needs_download(&detector));

        // Retry, released: the download runs again and completes.
        let preparing = tokio::spawn({
            let detector = Arc::clone(&detector);
            async move { detector.prepare_launch(&claude(), details).await }
        });
        launch_gate.until_waiting(1).await;
        assert_eq!(received.recv().await.as_deref(), Some(DOWNLOADING));
        launch_gate.open();
        assert_eq!(preparing.await.expect("join"), Ok(claude_launch()));
        assert_eq!(launch_gate.waiting(), 0);
        assert!(!needs_download(&detector));
    }
}
