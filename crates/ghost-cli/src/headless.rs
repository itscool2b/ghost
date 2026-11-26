//! The headless runtime: the session manager the desktop builds at startup (spec 7.2), with no
//! window, on a temporary data root unless `--data-dir` names one.
//!
//! Two data roots are in play. The *profile root* holds the profile the chats run on: a fresh
//! temporary one per run (device identity, `profiles/local/aura.db`, protocol logs), or the
//! `--data-dir` one, whose owner lock is taken first, so the CLI refuses while the app runs on it
//! (spec 6.1). The *machine root* is where agent processes register (`run/`, spec 3.5) and where
//! the npm adapters are installed (`adapters/`, shared with the app under the install lock): the
//! real data root (`GHOST_DATA_DIR` or the platform's), or the `--data-dir` one. So the app's next
//! sweep cleans up after a crashed CLI run, and an adapter is downloaded once for both.
//!
//! The order is the app's: profile, owner lock, store, chat store; the leftover registry and the
//! sweep of dead instances' records (awaited here, so its report comes before the command's
//! output); one redactor shared by the env resolver, the protocol logs and the manager; the
//! detector; the spawner, which also sets the `--env` pairs; the shipped drivers.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use ghost_agents::Detector;
use ghost_core::chatstore::ChatStore;
use ghost_core::device::DeviceIdentity;
use ghost_core::owner::ProfileOwner;
use ghost_core::paths::DataRoot;
use ghost_core::profile::Profiles;
use ghost_core::session::{
    ManagerDeps, ManagerLimits, SessionManager, SessionTimings, ShutdownSummary,
};
use ghost_core::store::Store;
use ghost_drivers::factory::StandardDriverFactory;
use ghost_process::{
    EnvKey, EnvResolver, EnvResolverConfig, LeftoverRegistry, LogFactory, ProcessSpawner,
    ProtocolLog, Redactor, ResolvedEnv, SpawnError, SpawnSpec, SpawnedProcess, Spawner,
    SweepReport, secret_values, sweep_stale,
};
use tokio::runtime::Handle;

use crate::CliError;

/// `<machine root>/run`: one leftover registry file per running Ghost process.
pub const RUN_DIR: &str = "run";

/// `<machine root>/adapters`: the npm adapters, one folder per package version.
pub const ADAPTERS_DIR: &str = "adapters";

/// How long the agents get to stop at the end of a run, as when the app quits.
pub const SESSIONS_DEADLINE: Duration = Duration::from_secs(8);

/// How long the store gets to save and close after them.
pub const STORE_DEADLINE: Duration = Duration::from_secs(5);

/// How much longer than its own deadline a step of the end is waited for.
const STEP_MARGIN: Duration = Duration::from_secs(1);

/// The two data roots of a run (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    /// `run/` and `adapters/` live here.
    pub machine: PathBuf,
    /// The chats' profile lives here.
    pub profile: PathBuf,
}

/// The env agents start with.
#[derive(Debug, Clone)]
pub enum EnvChoice {
    /// The user's login shell, as the app resolves it (spec 3.6).
    LoginShell,
    /// This env as it is (tests: a `PATH` that finds no agent).
    Fixed(ResolvedEnv),
}

/// Everything a headless runtime is built from.
#[derive(Debug, Clone)]
pub struct HeadlessConfig {
    pub roots: Roots,
    pub env: EnvChoice,
    /// Set on every agent process after the launch's own variables (`probe --env`).
    pub set_env: Vec<(OsString, OsString)>,
    /// What the agents see as the client's version.
    pub app_version: String,
    /// Provide workspace and checkpoint storage. Automatic snapshots follow the profile's
    /// explicit opt-in; isolated protocol fixtures may omit these services entirely.
    pub checkpoints: bool,
}

/// A running headless session manager and what ending it needs.
pub struct Headless {
    pub manager: SessionManager,
    pub store: Arc<ChatStore>,
    pub registry: Arc<LeftoverRegistry>,
    pub detector: Arc<Detector>,
    pub env: Arc<EnvResolver>,
    pub roots: Roots,
    /// The profile's folder: `aura.db`, `logs/protocol/<chat>/<spawn>.jsonl`.
    pub profile_dir: PathBuf,
    /// What the startup sweep found of dead instances' agents.
    pub sweep: SweepReport,
}

impl std::fmt::Debug for Headless {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Headless")
            .field("roots", &self.roots)
            .field("profile_dir", &self.profile_dir)
            .finish_non_exhaustive()
    }
}

/// How the end of a run went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended {
    /// `None` when the agents did not stop within their deadline plus a margin.
    pub sessions: Option<ShutdownSummary>,
    /// Whether the store saved everything before it closed.
    pub saved: bool,
}

impl Headless {
    /// Builds the runtime on the current Tokio runtime.
    pub async fn start(config: HeadlessConfig) -> Result<Headless, CliError> {
        let rt = Handle::current();
        let HeadlessConfig {
            roots,
            env,
            set_env,
            app_version,
            checkpoints,
        } = config;

        // The profile, as the app's boot opens it: the owner lock before the store.
        let profile_root = DataRoot::at(&roots.profile);
        profile_root.ensure()?;
        let device = DeviceIdentity::load_or_create(&profile_root)?
            .identity()
            .clone();
        let profile = Profiles::new(profile_root).open_active()?;
        let owner = ProfileOwner::acquire(&profile)?;
        let (store, _) = Store::open(&profile)?;
        let store = ChatStore::start(store, owner, &profile, device.device_id.clone(), rt.clone())?;

        let run_dir = roots.machine.join(RUN_DIR);
        let registry = match LeftoverRegistry::open(&run_dir, rt.clone()) {
            Ok(registry) => registry,
            Err(error) => {
                tracing::warn!(%error, path = %run_dir.display(), "the leftover registry is unavailable; agents run without it");
                LeftoverRegistry::disabled()
            }
        };
        let sweep = sweep_stale(&run_dir, registry.own()).await;

        let redactor = Arc::new(Redactor::standard());
        let env = match env {
            EnvChoice::LoginShell => EnvResolver::start(
                EnvResolverConfig {
                    redactor: Some(Arc::clone(&redactor)),
                    ..EnvResolverConfig::standard()
                },
                Arc::clone(&registry),
                rt.clone(),
            ),
            EnvChoice::Fixed(fixed) => {
                redactor.set_secrets(secret_values(&fixed.vars));
                EnvResolver::fixed(fixed)
            }
        };
        let detector = Detector::new(
            Arc::clone(&env),
            Arc::clone(&registry),
            roots.machine.join(ADAPTERS_DIR),
            rt.clone(),
        );
        let spawner: Arc<dyn ProcessSpawner> = Arc::new(SetEnvSpawner {
            base: Spawner::new(Arc::clone(&registry), rt.clone()),
            set: set_env,
        });
        let logs = LogFactory::new(
            profile.logs_dir().join("protocol"),
            Arc::clone(&redactor),
            rt.clone(),
        );
        let manager = SessionManager::new(ManagerDeps {
            worktrees: checkpoints.then(|| {
                ghost_core::gitworkspaces::GitWorkspaces::new(
                    &profile,
                    Arc::clone(&env),
                    Arc::clone(&registry),
                )
            }),
            checkpoints: checkpoints.then(|| {
                ghost_core::checkpoints::Checkpoints::new(
                    &profile,
                    Arc::clone(&env),
                    Arc::clone(&registry),
                )
            }),
            store: Arc::clone(&store),
            env: Arc::clone(&env),
            spawner,
            detector: Arc::clone(&detector),
            drivers: StandardDriverFactory::new(),
            logs,
            redactor,
            device_id: device.device_id,
            app_version,
            limits: ManagerLimits::STANDARD,
            timings: SessionTimings::STANDARD,
            rt,
        });
        if checkpoints {
            let enabled = match ghost_core::onboarding::OnboardingStore::open(&profile.dir) {
                Ok(settings) => settings.preferences().general.automatic_checkpoints,
                Err(error) => {
                    tracing::warn!(%error, "settings could not be read; automatic checkpoints stay off");
                    false
                }
            };
            manager.set_automatic_checkpoints(enabled);
        }
        Ok(Headless {
            manager,
            store,
            registry,
            detector,
            env,
            roots,
            profile_dir: profile.dir,
            sweep,
        })
    }

    /// `<machine root>/adapters`.
    pub fn adapters_dir(&self) -> PathBuf {
        self.roots.machine.join(ADAPTERS_DIR)
    }

    /// The folder of a chat's protocol logs.
    pub fn log_dir(&self, chat_id: &str) -> PathBuf {
        self.manager.protocol_log_dir(chat_id)
    }

    /// Ends the run as the app quits: every agent stops within 8 s (then the rest is killed), the
    /// store saves and closes within 5 s, and this process's registry file goes.
    pub async fn shutdown(&self) -> Ended {
        let sessions = tokio::time::timeout(
            SESSIONS_DEADLINE + STEP_MARGIN,
            self.manager.shutdown_all(SESSIONS_DEADLINE),
        )
        .await
        .ok();
        if sessions.is_none() {
            self.manager.kill_all_now();
        }
        let saved = tokio::time::timeout(
            STORE_DEADLINE + STEP_MARGIN,
            self.store.shutdown(STORE_DEADLINE),
        )
        .await
        .unwrap_or(false);
        self.close_registry();
        Ended { sessions, saved }
    }

    /// The last resort, after a second signal: kills every agent process at once and gives the
    /// store a second to close.
    pub fn kill_now(&self) {
        self.manager.kill_all_now();
        if !self.store.shutdown_blocking(Duration::from_secs(1)) {
            tracing::warn!("the store could not save everything before ghost-cli exited");
        }
        self.close_registry();
    }

    fn close_registry(&self) {
        if let Err(error) = self.registry.close() {
            tracing::warn!(%error, "could not remove this process's leftover registry file");
        }
    }
}

/// The startup notice when the sweep stopped processes a crashed Ghost left running, as the app
/// words it.
pub fn sweep_notice(report: &SweepReport) -> Option<String> {
    match report.killed.len() {
        0 => None,
        1 => Some(
            "Ghost stopped 1 agent process left running by an earlier session that ended unexpectedly."
                .into(),
        ),
        n => Some(format!(
            "Ghost stopped {n} agent processes left running by an earlier session that ended unexpectedly."
        )),
    }
}

/// Sets the `--env` pairs on every agent process, after the launch's own variables: a pair
/// replaces a launch variable of the same name (`CLAUDE_CODE_EXECUTABLE`, say) and, like every
/// `set_env` entry, wins over the user's own value after the scrub.
struct SetEnvSpawner {
    base: Arc<dyn ProcessSpawner>,
    set: Vec<(OsString, OsString)>,
}

impl ProcessSpawner for SetEnvSpawner {
    fn spawn(
        &self,
        mut spec: SpawnSpec,
        log: ProtocolLog,
    ) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        apply_set_env(&mut spec.launch.set_env, &self.set);
        self.base.spawn(spec, log)
    }
}

/// `set` after `launch`: a name in both keeps `set`'s value, in `set`'s place.
fn apply_set_env(launch: &mut Vec<(OsString, OsString)>, set: &[(OsString, OsString)]) {
    for (name, value) in set {
        let key = EnvKey::new(name.clone());
        launch.retain(|(existing, _)| EnvKey::new(existing.clone()) != key);
        launch.push((name.clone(), value.clone()));
    }
}

/// The data root a run without `--data-dir` registers in and installs adapters into: the one the
/// app uses.
pub fn real_data_root() -> Result<PathBuf, CliError> {
    Ok(DataRoot::resolve()?.root().to_path_buf())
}

/// A path as given, made absolute against the current folder.
pub fn absolute(path: &Path) -> Result<PathBuf, CliError> {
    std::path::absolute(path)
        .map_err(|error| CliError::io(format!("resolve {}", path.display()), error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_env_pairs_win_over_launch_variables_and_keep_their_order() {
        let mut launch: Vec<(OsString, OsString)> = vec![
            ("CODEX_PATH".into(), "/usr/local/bin/codex".into()),
            ("KEEP".into(), "1".into()),
        ];
        apply_set_env(
            &mut launch,
            &[
                ("CODEX_HOME".into(), "/tmp/empty".into()),
                ("CODEX_PATH".into(), "/opt/codex".into()),
            ],
        );
        assert_eq!(
            launch,
            [
                (OsString::from("KEEP"), OsString::from("1")),
                (OsString::from("CODEX_HOME"), OsString::from("/tmp/empty")),
                (OsString::from("CODEX_PATH"), OsString::from("/opt/codex")),
            ]
        );
    }

    #[test]
    fn the_sweep_notice_counts_what_it_stopped() {
        let mut report = SweepReport::default();
        assert_eq!(sweep_notice(&report), None);
        report.killed.push((5001, "hermes acp".into()));
        assert!(
            sweep_notice(&report).is_some_and(|notice| notice.contains("stopped 1 agent process "))
        );
        report.killed.push((5002, "codex-acp".into()));
        assert!(
            sweep_notice(&report)
                .is_some_and(|notice| notice.contains("stopped 2 agent processes"))
        );
    }
}
