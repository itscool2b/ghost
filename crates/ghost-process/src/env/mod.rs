//! The environment agents start with.
//!
//! Finder and the Dock start apps with launchd's bare `PATH`, so on macOS and Linux the env comes
//! from the user's login shell (`$SHELL -i -l -c` with nonce markers and `env -0`, in a new session,
//! 10 s; then `-l -c`, 5 s; then a fallback), and on Windows from the registry (HKLM, then HKCU). It
//! is resolved in the background at boot, shared by every caller, and kept in memory only: it holds
//! API keys, so it never reaches disk.
//!
//! Every child then gets [`child_env`]: the resolved env, minus the scrub's denylist ([`scrub`]),
//! plus the spawn's own `set` entries. Values are never logged; reports carry names only, and the
//! values of secret-looking variables ([`secret_values`]) go to the shared [`Redactor`] after each
//! resolution, so an agent that echoes a key never writes it into a protocol log.

mod scrub;
mod shell;
mod which;
mod windows;

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::runtime::Handle;

use crate::log::Redactor;
use crate::registry::LeftoverRegistry;

pub use scrub::{DenyReason, ScrubReport, deny_reason, node_options_has_debugger_hook, scrub};
#[cfg(unix)]
pub use shell::user_shell;
pub use shell::{parse_env0, terminal_args};
#[cfg(unix)]
pub(crate) use which::is_program;
#[cfg(windows)]
pub(crate) use which::pathext;
pub use which::{find_exe, find_program};
pub use windows::{expand_env_strings, merge_windows_path, policy_key_exists};

/// An environment variable name. `Eq`, `Ord` and `Hash` ignore ASCII case on Windows, where
/// `Path` and `PATH` are one variable; elsewhere names compare exactly.
#[derive(Clone)]
pub struct EnvKey(OsString);

impl EnvKey {
    pub fn new(name: impl Into<OsString>) -> EnvKey {
        EnvKey(name.into())
    }

    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }

    pub fn into_os_string(self) -> OsString {
        self.0
    }

    /// The name as compared: ASCII-uppercased on Windows.
    fn folded(&self) -> impl Iterator<Item = u8> + '_ {
        self.0.as_encoded_bytes().iter().map(|byte| {
            if cfg!(windows) {
                byte.to_ascii_uppercase()
            } else {
                *byte
            }
        })
    }
}

impl PartialEq for EnvKey {
    fn eq(&self, other: &EnvKey) -> bool {
        self.folded().eq(other.folded())
    }
}

impl Eq for EnvKey {}

impl PartialOrd for EnvKey {
    fn partial_cmp(&self, other: &EnvKey) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EnvKey {
    fn cmp(&self, other: &EnvKey) -> Ordering {
        self.folded().cmp(other.folded())
    }
}

impl Hash for EnvKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for byte in self.folded() {
            state.write_u8(byte);
        }
    }
}

impl fmt::Debug for EnvKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl From<&str> for EnvKey {
    fn from(name: &str) -> EnvKey {
        EnvKey(name.into())
    }
}

impl From<String> for EnvKey {
    fn from(name: String) -> EnvKey {
        EnvKey(name.into())
    }
}

impl From<OsString> for EnvKey {
    fn from(name: OsString) -> EnvKey {
        EnvKey(name)
    }
}

impl From<&OsStr> for EnvKey {
    fn from(name: &OsStr) -> EnvKey {
        EnvKey(name.to_os_string())
    }
}

/// A complete environment, by name.
pub type EnvMap = BTreeMap<EnvKey, OsString>;

/// A resolved environment. `Debug` prints names only: values hold API keys.
#[derive(Clone)]
pub struct ResolvedEnv {
    pub vars: EnvMap,
    pub source: EnvSource,
    pub warnings: Vec<String>,
    pub resolved_at_ms: i64,
}

impl fmt::Debug for ResolvedEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedEnv")
            .field("vars", &self.vars.keys().collect::<Vec<_>>())
            .field("source", &self.source)
            .field("warnings", &self.warnings)
            .field("resolved_at_ms", &self.resolved_at_ms)
            .finish()
    }
}

/// Where a [`ResolvedEnv`] came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvSource {
    LoginShell {
        shell: PathBuf,
        /// `-i -l` worked; false after the `-l` retry.
        interactive: bool,
        took_ms: u64,
    },
    WindowsRegistry,
    /// Ghost's own env plus the known dirs, because the shell failed.
    Fallback {
        reason: String,
    },
}

/// How the resolver runs. `None` fields take the user's real values (tests set them).
#[derive(Clone)]
pub struct EnvResolverConfig {
    /// The interactive login shell's limit.
    pub timeout: Duration,
    /// The non-interactive retry's limit.
    pub retry_timeout: Duration,
    /// A failed attempt older than this is retried on the next `get`.
    pub retry_after: Duration,
    pub shell: Option<PathBuf>,
    /// The shell's input env instead of Ghost's own.
    pub base: Option<EnvMap>,
    pub home: Option<PathBuf>,
    pub known_dirs: Option<Vec<PathBuf>>,
    /// Gets the secret literals of every env the resolver produces ([`secret_values`]), before
    /// any caller sees that env: the shared redactor of protocol logs and sessions. `None` leaves
    /// redaction to the caller (tests).
    pub redactor: Option<Arc<Redactor>>,
}

/// Prints `base` names only: values hold API keys.
impl fmt::Debug for EnvResolverConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvResolverConfig")
            .field("timeout", &self.timeout)
            .field("retry_timeout", &self.retry_timeout)
            .field("retry_after", &self.retry_after)
            .field("shell", &self.shell)
            .field(
                "base",
                &self
                    .base
                    .as_ref()
                    .map(|base| base.keys().collect::<Vec<_>>()),
            )
            .field("home", &self.home)
            .field("known_dirs", &self.known_dirs)
            .field("redactor", &self.redactor)
            .finish()
    }
}

impl EnvResolverConfig {
    /// 10 s, 5 s, 60 s, the user's real shell, env, home and known dirs, and no redactor.
    pub fn standard() -> EnvResolverConfig {
        EnvResolverConfig {
            timeout: Duration::from_secs(10),
            retry_timeout: Duration::from_secs(5),
            retry_after: Duration::from_secs(60),
            shell: None,
            base: None,
            home: None,
            known_dirs: None,
            redactor: None,
        }
    }
}

/// Resolves the env in the background and hands every caller the same result.
pub struct EnvResolver {
    inner: Arc<ResolverInner>,
}

struct ResolverInner {
    /// `None` for a fixed resolver.
    live: Option<Live>,
    state: Mutex<ResolverState>,
}

struct Live {
    config: EnvResolverConfig,
    registry: Arc<LeftoverRegistry>,
    rt: Handle,
}

/// One resolution, shared by everyone waiting for it.
type Attempt = Shared<BoxFuture<'static, Arc<ResolvedEnv>>>;

#[derive(Default)]
struct ResolverState {
    current: Option<Arc<ResolvedEnv>>,
    /// How long the attempt that produced `current` took.
    took_ms: u64,
    /// When that attempt finished.
    finished: Option<tokio::time::Instant>,
    in_flight: Option<Attempt>,
}

impl EnvResolver {
    /// Starts resolving on `rt`; boot never waits for it. The first attempt starts at once.
    pub fn start(
        config: EnvResolverConfig,
        registry: Arc<LeftoverRegistry>,
        rt: Handle,
    ) -> Arc<EnvResolver> {
        let resolver = Arc::new(EnvResolver {
            inner: Arc::new(ResolverInner {
                live: Some(Live {
                    config,
                    registry,
                    rt,
                }),
                state: Mutex::new(ResolverState::default()),
            }),
        });
        {
            let mut state = lock(&resolver.inner.state);
            resolver.begin(&mut state);
        }
        resolver
    }

    /// Tests: no shell; [`get`](Self::get) returns `env` at once. No redactor gets its secrets.
    pub fn fixed(env: ResolvedEnv) -> Arc<EnvResolver> {
        let took_ms = match &env.source {
            EnvSource::LoginShell { took_ms, .. } => *took_ms,
            EnvSource::WindowsRegistry | EnvSource::Fallback { .. } => 0,
        };
        Arc::new(EnvResolver {
            inner: Arc::new(ResolverInner {
                live: None,
                state: Mutex::new(ResolverState {
                    current: Some(Arc::new(env)),
                    took_ms,
                    finished: None,
                    in_flight: None,
                }),
            }),
        })
    }

    /// The resolved env, sharing the attempt in flight. A fallback older than
    /// [`EnvResolverConfig::retry_after`] is retried first.
    pub async fn get(&self) -> Arc<ResolvedEnv> {
        let attempt = {
            let mut state = lock(&self.inner.state);
            if let Some(attempt) = &state.in_flight {
                attempt.clone()
            } else if let Some(current) = &state.current
                && !self.stale(&state)
            {
                return Arc::clone(current);
            } else {
                match self.begin(&mut state) {
                    Some(attempt) => attempt,
                    None => return Arc::new(empty_env()),
                }
            }
        };
        attempt.await
    }

    /// The last resolved env, if any attempt finished.
    pub fn current(&self) -> Option<Arc<ResolvedEnv>> {
        lock(&self.inner.state).current.clone()
    }

    /// Starts a new attempt, unless one is running. Running sessions keep the env they started
    /// with.
    pub fn refresh(&self) {
        let mut state = lock(&self.inner.state);
        if state.in_flight.is_none() {
            self.begin(&mut state);
        }
    }

    pub fn status(&self) -> ResolverStatus {
        let state = lock(&self.inner.state);
        if state.in_flight.is_some() {
            return ResolverStatus::Resolving;
        }
        let Some(env) = &state.current else {
            return ResolverStatus::Resolving;
        };
        match &env.source {
            EnvSource::LoginShell { took_ms, .. } => ResolverStatus::Ready {
                source: env.source.clone(),
                took_ms: *took_ms,
                vars: env.vars.len(),
            },
            EnvSource::WindowsRegistry => ResolverStatus::Ready {
                source: env.source.clone(),
                took_ms: state.took_ms,
                vars: env.vars.len(),
            },
            EnvSource::Fallback { reason } => ResolverStatus::Fallback {
                reason: reason.clone(),
            },
        }
    }

    /// Whether the current env is a fallback old enough to try again.
    fn stale(&self, state: &ResolverState) -> bool {
        let Some(live) = &self.inner.live else {
            return false;
        };
        let failed = state
            .current
            .as_ref()
            .is_some_and(|env| matches!(env.source, EnvSource::Fallback { .. }));
        failed
            && state
                .finished
                .is_some_and(|finished| finished.elapsed() >= live.config.retry_after)
    }

    /// Starts an attempt on the runtime and records it as in flight. `None` for a fixed resolver.
    fn begin(&self, state: &mut ResolverState) -> Option<Attempt> {
        let live = self.inner.live.as_ref()?;
        let inner = Arc::clone(&self.inner);
        let task = live.rt.spawn(async move {
            let Some(live) = &inner.live else {
                return Arc::new(empty_env());
            };
            let started = tokio::time::Instant::now();
            let env = resolve(&live.config, &live.registry).await;
            if let Some(redactor) = &live.config.redactor {
                redactor.set_secrets(secret_values(&env.vars));
            }
            let env = Arc::new(env);
            let mut state = lock(&inner.state);
            state.current = Some(Arc::clone(&env));
            state.took_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            state.finished = Some(tokio::time::Instant::now());
            state.in_flight = None;
            env
        });
        let inner = Arc::clone(&self.inner);
        let attempt = async move {
            match task.await {
                Ok(env) => env,
                Err(_) => failed_attempt(&inner),
            }
        }
        .boxed()
        .shared();
        state.in_flight = Some(attempt.clone());
        Some(attempt)
    }
}

impl fmt::Debug for EnvResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = lock(&self.inner.state);
        f.debug_struct("EnvResolver")
            .field("fixed", &self.inner.live.is_none())
            .field("current", &state.current)
            .field("resolving", &state.in_flight.is_some())
            .finish()
    }
}

/// The answer when an attempt's task ended without one (the runtime is shutting down, or the
/// attempt panicked), so no caller waits forever: the last env, or else the fallback, `base` or
/// Ghost's own env without its launch context. A new fallback is the current env from then on,
/// and its secrets reach the redactor before any caller gets it, as every resolution's do.
fn failed_attempt(inner: &ResolverInner) -> Arc<ResolvedEnv> {
    let mut state = lock(&inner.state);
    state.in_flight = None;
    if let Some(current) = &state.current {
        return Arc::clone(current);
    }
    let config = inner.live.as_ref().map(|live| &live.config);
    let mut vars = config
        .and_then(|config| config.base.clone())
        .unwrap_or_else(process_env);
    strip_launch_context(&mut vars);
    if let Some(redactor) = config.and_then(|config| config.redactor.as_ref()) {
        redactor.set_secrets(secret_values(&vars));
    }
    let env = Arc::new(fallback(
        vars,
        "the environment could not be read".into(),
        Vec::new(),
    ));
    state.current = Some(Arc::clone(&env));
    state.finished = Some(tokio::time::Instant::now());
    env
}

/// An env with nothing in it, for a resolver with nothing to resolve.
fn empty_env() -> ResolvedEnv {
    ResolvedEnv {
        vars: EnvMap::new(),
        source: EnvSource::Fallback {
            reason: "no environment".into(),
        },
        warnings: Vec::new(),
        resolved_at_ms: now_millis(),
    }
}

/// One resolution: the login shell (macOS and Linux) or the registry (Windows), with the known
/// dirs appended and the shell's own bookkeeping variables removed.
async fn resolve(config: &EnvResolverConfig, registry: &LeftoverRegistry) -> ResolvedEnv {
    let mut base = config.base.clone().unwrap_or_else(process_env);
    strip_launch_context(&mut base);
    #[cfg(windows)]
    {
        let _ = registry;
        let input = base.clone();
        let read = tokio::task::spawn_blocking(move || windows::registry_env(&input)).await;
        let mut env = match read {
            Ok(Ok(vars)) => ResolvedEnv {
                vars,
                source: EnvSource::WindowsRegistry,
                warnings: Vec::new(),
                resolved_at_ms: now_millis(),
            },
            Ok(Err(error)) => fallback(
                base,
                format!("the registry could not be read: {error}"),
                Vec::new(),
            ),
            Err(error) => fallback(
                base,
                format!("the registry could not be read: {error}"),
                Vec::new(),
            ),
        };
        let known = config.known_dirs.clone().unwrap_or_else(|| {
            let mut known = config
                .home
                .clone()
                .or_else(|| {
                    env.vars
                        .get(&EnvKey::from("USERPROFILE"))
                        .map(PathBuf::from)
                })
                .filter(|home| home.is_absolute())
                .map(|home| standard_known_dirs(&home))
                .unwrap_or_default();
            for (variable, relative) in [
                ("APPDATA", "npm"),
                ("LOCALAPPDATA", "Microsoft/WinGet/Links"),
                ("LOCALAPPDATA", "Programs/OpenAI/Codex/bin"),
                ("LOCALAPPDATA", "cursor-agent"),
            ] {
                if let Some(base) = env
                    .vars
                    .get(&EnvKey::from(variable))
                    .map(PathBuf::from)
                    .filter(|base| base.is_absolute())
                {
                    known.push(base.join(relative));
                }
            }
            known
        });
        append_known_dirs(&mut env.vars, &known);
        env
    }
    #[cfg(unix)]
    {
        let home = home_dir(config, &base);
        let known = config
            .known_dirs
            .clone()
            .unwrap_or_else(|| standard_known_dirs(&home));
        let default = shell::platform_default();
        let mut env = match shell::login_shell(config, registry, &base, &home, &default).await {
            Ok(env) => env,
            Err((reason, warnings)) => fallback(base, reason, warnings),
        };
        for name in ["PWD", "OLDPWD", "SHLVL", "_", "GHOST_RESOLVING_ENV"] {
            env.vars.remove(&EnvKey::from(name));
        }
        append_known_dirs(&mut env.vars, &known);
        env
    }
}

/// Ghost's own env, without its launch context, as the env of last resort.
fn fallback(vars: EnvMap, reason: String, warnings: Vec<String>) -> ResolvedEnv {
    ResolvedEnv {
        vars,
        source: EnvSource::Fallback { reason },
        warnings,
        resolved_at_ms: now_millis(),
    }
}

/// The user's home: the config's, the env's `HOME`, the passwd entry's, or `/`.
#[cfg(unix)]
fn home_dir(config: &EnvResolverConfig, base: &EnvMap) -> PathBuf {
    if let Some(home) = &config.home {
        return home.clone();
    }
    if let Some(home) = base.get(&EnvKey::from("HOME")).map(PathBuf::from)
        && home.is_absolute()
    {
        return home;
    }
    nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.dir)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The folders agents install into, whose absence from a launchd `PATH` would hide them:
/// `~/.local/bin`, the opencode, grok, pi, bun and volta bins, the asdf and mise shims, the
/// `prefix=` of `~/.npmrc` (read, never by running npm) plus `/bin`, Homebrew's and `/usr/local/bin`.
/// nvm is left to the login shell: it is a shell function the fallback cannot resolve reliably.
pub(crate) fn standard_known_dirs(home: &std::path::Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        ".local/bin",
        ".opencode/bin",
        ".grok/bin",
        ".pi/agent/bin",
        ".bun/bin",
        ".volta/bin",
        ".asdf/shims",
        ".local/share/mise/shims",
    ]
    .iter()
    .map(|relative| home.join(relative))
    .collect();
    if let Some(prefix) = read_npmrc_prefix(home) {
        #[cfg(unix)]
        dirs.push(prefix.join("bin"));
        #[cfg(windows)]
        dirs.push(prefix);
    }
    #[cfg(unix)]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    dirs
}

/// Read only a bounded configuration input for its prefix. Other fields are never exposed.
fn read_npmrc_prefix(home: &std::path::Path) -> Option<PathBuf> {
    use std::io::Read;
    const LIMIT: u64 = 64 * 1024;
    let file = std::fs::File::open(home.join(".npmrc")).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut input = String::new();
    file.take(LIMIT + 1).read_to_string(&mut input).ok()?;
    (input.len() as u64 <= LIMIT)
        .then(|| npmrc_prefix(&input, home))
        .flatten()
}

/// The `prefix` of an `.npmrc`: `prefix=/path` or `prefix = "~/path"`; a relative prefix is ignored.
pub(crate) fn npmrc_prefix(npmrc: &str, home: &std::path::Path) -> Option<PathBuf> {
    let value = npmrc.lines().rev().find_map(|line| {
        let line = line.trim();
        if line.starts_with(';') || line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        (key.trim() == "prefix").then(|| value.trim().trim_matches(|c| c == '"' || c == '\''))
    })?;
    if value.chars().any(char::is_control) {
        return None;
    }
    let path = match value.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(value),
    };
    path.is_absolute().then_some(path)
}

/// Appends each existing dir that `PATH` lacks, in order.
pub(crate) fn append_known_dirs(vars: &mut EnvMap, known: &[PathBuf]) {
    let key = EnvKey::from("PATH");
    let mut dirs: Vec<PathBuf> = vars
        .get(&key)
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    let mut changed = false;
    for dir in known {
        let exists = dirs.iter().any(|present| {
            if cfg!(windows) {
                present.as_os_str().eq_ignore_ascii_case(dir.as_os_str())
            } else {
                present == dir
            }
        });
        if !exists && dir.is_absolute() && dir.is_dir() {
            dirs.push(dir.clone());
            changed = true;
        }
    }
    if changed && let Ok(path) = std::env::join_paths(&dirs) {
        vars.insert(key, path);
    }
}

/// Where the resolver is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolverStatus {
    Resolving,
    Ready {
        source: EnvSource,
        took_ms: u64,
        vars: usize,
    },
    Fallback {
        reason: String,
    },
}

/// Variables that describe how Ghost itself was launched, which the resolver's shell must not
/// inherit: a dev launch from Apple Terminal would replay its session restore against the user's
/// tab history, and a `pnpm dev` launch would leak `npm_config_*` into every agent. Whatever the
/// user's own rc files export comes back through the shell's output.
///
/// The terminal app Ghost was started from names itself too, and none of it is true for an
/// agent: Ghost's own terminal is not kitty, so a child that read `KITTY_WINDOW_ID` or
/// `KITTY_LISTEN_ON` would send kitty remote-control commands to the person's other window, and
/// `GHOSTTY_SHELL_FEATURES` or `VTE_VERSION` would load shell integration that writes another
/// terminal's escape codes. A terminal inside Ghost says what it is itself (`pty::spawn`).
const LAUNCH_CONTEXT_NAMES: &[&str] = &[
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "INIT_CWD",
    // The launching terminal and multiplexer.
    "VTE_VERSION",
    "WINDOWID",
    "WT_SESSION",
    "WT_PROFILE_ID",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "COLORFGBG",
    "TMUX",
    "TMUX_PANE",
    "STY",
];

/// Names, separated by `:`, of variables Ghost set for itself when it started (the NVIDIA
/// switch in the desktop's `main`). They and this variable are launch context: the app needs
/// them, an agent's shell must not inherit them.
pub const LAUNCH_SET_ENV: &str = "GHOST_LAUNCH_SET";

/// The same marker under the app's earlier names: a copy updated from then may start with it.
pub const LEGACY_LAUNCH_SET_ENVS: [&str; 2] = ["O7_LAUNCH_SET", "AURACODE_LAUNCH_SET"];

/// Prefix families of launch-context variables (see [`LAUNCH_CONTEXT_NAMES`]), the terminal apps'
/// own families among them.
const LAUNCH_CONTEXT_PREFIXES: &[&str] = &[
    "SHELL_SESSION_",
    "npm_",
    "PNPM_",
    "pnpm_",
    "TAURI_",
    "KITTY_",
    "GHOSTTY_",
    "ALACRITTY_",
    "WEZTERM_",
    "KONSOLE_",
    "ITERM_",
    "TERMINATOR_",
    "ZELLIJ",
];

/// Removes the variables that describe how Ghost itself was launched; returns their names,
/// sorted. Applies to the resolver shell's input and to the fallback.
///
/// Names and prefixes compare case-sensitively on every OS, unlike the scrub's: npm, pnpm and Tauri
/// set these names in a fixed case, and a user's own `NPM_TOKEN` or `NPM_CONFIG_REGISTRY` must
/// reach the shell on Windows too.
///
/// What Ghost set for itself goes too ([`LAUNCH_SET_ENV`]), and on Unix a Linux AppImage's
/// launch context ([`strip_appimage`]); their names are reported with the rest.
pub fn strip_launch_context(vars: &mut EnvMap) -> Vec<String> {
    let doomed: Vec<EnvKey> = vars
        .keys()
        .filter(|key| is_launch_context(key.as_os_str()))
        .cloned()
        .collect();
    let mut removed: Vec<String> = doomed
        .into_iter()
        .filter_map(|key| {
            vars.remove(&key)?;
            Some(key.as_os_str().to_string_lossy().into_owned())
        })
        .collect();
    for marker in std::iter::once(LAUNCH_SET_ENV).chain(LEGACY_LAUNCH_SET_ENVS) {
        let Some(set) = vars.remove(&EnvKey::from(marker)) else {
            continue;
        };
        removed.push(marker.to_owned());
        for name in set
            .to_string_lossy()
            .split(':')
            .filter(|name| !name.is_empty())
        {
            if vars.remove(&EnvKey::from(name)).is_some() {
                removed.push(name.to_owned());
            }
        }
    }
    #[cfg(unix)]
    removed.extend(strip_appimage(vars));
    removed.sort();
    removed.dedup();
    removed
}

/// The AppImage runtime's own variables, naming the image and how it was started.
#[cfg(unix)]
const APPIMAGE_RUNTIME_NAMES: &[&str] = &["APPDIR", "APPIMAGE", "ARGV0", "OWD"];

/// Variables an AppImage's launch hooks set to a fixed value, overwriting the session's, with the
/// variable whose pointing into the image shows that hook ran: linuxdeploy's GTK hook forces
/// `GTK_THEME` to Adwaita, its Python hook sets `PYTHONDONTWRITEBYTECODE`, its GStreamer hook
/// `GST_REGISTRY_REUSE_PLUGIN_SCANNER`.
#[cfg(unix)]
const APPIMAGE_HOOK_CONSTANTS: &[(&str, &str)] = &[
    ("GTK_THEME", "GTK_DATA_PREFIX"),
    ("PYTHONDONTWRITEBYTECODE", "PYTHONHOME"),
    (
        "GST_REGISTRY_REUSE_PLUGIN_SCANNER",
        "GST_PLUGIN_SCANNER_1_0",
    ),
];

/// Removes what a Linux AppImage's launcher put in the env, when `APPDIR` names its mount; returns
/// the names it removed or rewrote. The launcher points `LD_LIBRARY_PATH`, `PATH`,
/// `XDG_DATA_DIRS`, `PYTHONHOME`, `PERLLIB`, `GIO_MODULE_DIR` and others into the image, so the
/// user's own `node`, `git` or `python` loads the image's libraries and fails (`node`: "version
/// `LIBFFI_CALL_PLAN_8.4' not found"). Each path-list entry inside the mount is dropped, with empty
/// entries and repeats; a variable left with nothing is removed. A value with nothing in the
/// mount is kept as it is, whatever its name. A user's own `LD_LIBRARY_PATH` from their rc files
/// comes back through the login shell's output.
#[cfg(unix)]
fn strip_appimage(vars: &mut EnvMap) -> Vec<String> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let Some(appdir) = vars.get(&EnvKey::from("APPDIR")).map(|dir| {
        let bytes = dir.as_bytes();
        let end = bytes
            .iter()
            .rposition(|byte| *byte != b'/')
            .map_or(0, |at| at + 1);
        bytes[..end].to_vec()
    }) else {
        return Vec::new();
    };
    let mut touched = Vec::new();
    for name in APPIMAGE_RUNTIME_NAMES {
        if vars.remove(&EnvKey::from(*name)).is_some() {
            touched.push((*name).to_owned());
        }
    }
    // `/` or a relative path would match too much: then this is not an AppImage mount.
    if appdir.first() != Some(&b'/') || appdir.len() < 2 {
        return touched;
    }
    let inside = |entry: &[u8]| {
        entry.starts_with(&appdir) && matches!(entry.get(appdir.len()), None | Some(b'/'))
    };
    for (name, hint) in APPIMAGE_HOOK_CONSTANTS {
        let hook_ran = vars
            .get(&EnvKey::from(*hint))
            .is_some_and(|value| value.as_bytes().split(|b| *b == b':').any(inside));
        if hook_ran && vars.remove(&EnvKey::from(*name)).is_some() {
            touched.push((*name).to_owned());
        }
    }
    let names: Vec<EnvKey> = vars.keys().cloned().collect();
    for name in names {
        let Some(value) = vars.get(&name) else {
            continue;
        };
        if !value.as_bytes().split(|b| *b == b':').any(inside) {
            continue;
        }
        let mut kept: Vec<&[u8]> = Vec::new();
        for entry in value.as_bytes().split(|b| *b == b':') {
            if !entry.is_empty() && !inside(entry) && !kept.contains(&entry) {
                kept.push(entry);
            }
        }
        let rewritten = (!kept.is_empty()).then(|| OsString::from_vec(kept.join(&b':')));
        touched.push(name.as_os_str().to_string_lossy().into_owned());
        match rewritten {
            Some(value) => {
                vars.insert(name, value);
            }
            None => {
                vars.remove(&name);
            }
        }
    }
    touched
}

fn is_launch_context(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    LAUNCH_CONTEXT_NAMES.contains(&name)
        || LAUNCH_CONTEXT_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// Words that make a variable's value a secret, compared ignoring ASCII case.
const SECRET_NAME_WORDS: &[&str] = &["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL"];

/// The values the redactor must hide: those of variables whose names contain `KEY`, `TOKEN`,
/// `SECRET`, `PASSWORD` or `CREDENTIAL`, ignoring ASCII case (so npm's
/// `npm_config_//registry.npmjs.org/:_authToken` counts). [`Redactor::set_secrets`] drops the
/// ones shorter than 12 characters. Names or values that are not Unicode are skipped.
pub fn secret_values(vars: &EnvMap) -> Vec<String> {
    vars.iter()
        .filter_map(|(name, value)| {
            let name = name.as_os_str().to_str()?.to_ascii_uppercase();
            if !SECRET_NAME_WORDS.iter().any(|word| name.contains(word)) {
                return None;
            }
            value.to_str().map(str::to_owned)
        })
        .collect()
}

/// The complete env of one child: resolved, scrubbed, then every `set` entry.
pub fn child_env(resolved: &ResolvedEnv, set: &[(OsString, OsString)]) -> (EnvMap, ScrubReport) {
    let mut vars = resolved.vars.clone();
    let report = scrub(&mut vars);
    for (name, value) in set {
        vars.insert(EnvKey::from(name.clone()), value.clone());
    }
    (vars, report)
}

/// Ghost's own environment.
pub fn process_env() -> EnvMap {
    std::env::vars_os()
        .map(|(name, value)| (EnvKey::from(name), value))
        .collect()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> EnvMap {
        pairs
            .iter()
            .map(|(name, value)| (EnvKey::from(*name), OsString::from(*value)))
            .collect()
    }

    fn names(vars: &EnvMap) -> Vec<String> {
        vars.keys()
            .map(|key| key.as_os_str().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn env_keys_ignore_case_only_on_windows() {
        let path = EnvKey::from("PATH");
        let mixed = EnvKey::from("Path");
        assert_eq!(path == mixed, cfg!(windows));
        let mut vars = EnvMap::new();
        vars.insert(path, "a".into());
        vars.insert(mixed, "b".into());
        assert_eq!(vars.len(), if cfg!(windows) { 1 } else { 2 });
    }

    #[test]
    fn launch_context_is_stripped_and_named() {
        let mut vars = map(&[
            ("PATH", "/usr/bin"),
            ("HOME", "/Users/me"),
            ("TERM_PROGRAM", "Apple_Terminal"),
            ("TERM_PROGRAM_VERSION", "455"),
            ("TERM_SESSION_ID", "w0t0p0"),
            ("SHELL_SESSION_HISTFILE", "/x"),
            ("npm_config_user_agent", "pnpm/12"),
            ("npm_lifecycle_event", "dev"),
            ("npm_execpath", "/opt/pnpm"),
            ("INIT_CWD", "/repo"),
            ("PNPM_HOME", "/pnpm"),
            ("pnpm_config_verify_deps_before_run", "false"),
            ("TAURI_ENV_PLATFORM", "darwin"),
            ("TERM", "xterm-256color"),
            ("SHELL", "/bin/zsh"),
        ]);
        let removed = strip_launch_context(&mut vars);
        assert_eq!(
            removed,
            [
                "INIT_CWD",
                "PNPM_HOME",
                "SHELL_SESSION_HISTFILE",
                "TAURI_ENV_PLATFORM",
                "TERM_PROGRAM",
                "TERM_PROGRAM_VERSION",
                "TERM_SESSION_ID",
                "npm_config_user_agent",
                "npm_execpath",
                "npm_lifecycle_event",
                "pnpm_config_verify_deps_before_run",
            ]
        );
        assert_eq!(names(&vars), ["HOME", "PATH", "SHELL", "TERM"]);
    }

    /// A launch from kitty, Ghostty, Windows Terminal or tmux: none of their names reach an agent,
    /// while the person's own settings that only look alike stay.
    #[test]
    fn the_launching_terminal_is_launch_context() {
        let mut vars = map(&[
            ("PATH", "/usr/bin"),
            ("KITTY_WINDOW_ID", "1"),
            ("KITTY_LISTEN_ON", "unix:/tmp/kitty-1234"),
            ("KITTY_PUBLIC_KEY", "1:abc"),
            ("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty"),
            ("GHOSTTY_SHELL_FEATURES", "cursor,title"),
            ("ALACRITTY_SOCKET", "/run/user/1000/Alacritty.sock"),
            ("WEZTERM_PANE", "0"),
            ("VTE_VERSION", "7600"),
            ("WINDOWID", "4194316"),
            ("WT_SESSION", "f3a1"),
            ("LC_TERMINAL", "iTerm2"),
            ("TMUX", "/tmp/tmux-1000/default,1,0"),
            ("TMUX_PANE", "%0"),
            ("ZELLIJ_SESSION_NAME", "dev"),
            ("EDITOR", "nvim"),
            ("TERMINAL", "kitty"),
            ("LC_ALL", "en_US.UTF-8"),
        ]);
        let removed = strip_launch_context(&mut vars);
        assert_eq!(
            removed,
            [
                "ALACRITTY_SOCKET",
                "GHOSTTY_RESOURCES_DIR",
                "GHOSTTY_SHELL_FEATURES",
                "KITTY_LISTEN_ON",
                "KITTY_PUBLIC_KEY",
                "KITTY_WINDOW_ID",
                "LC_TERMINAL",
                "TMUX",
                "TMUX_PANE",
                "VTE_VERSION",
                "WEZTERM_PANE",
                "WINDOWID",
                "WT_SESSION",
                "ZELLIJ_SESSION_NAME",
            ]
        );
        assert_eq!(names(&vars), ["EDITOR", "LC_ALL", "PATH", "TERMINAL"]);
    }

    /// What linuxdeploy's AppRun and its GTK and Python hooks set, as a real AppImage of Ghost
    /// left it: every entry inside the mount goes, the session's own entries stay in order.
    #[cfg(unix)]
    #[test]
    fn an_appimage_launch_context_is_stripped() {
        let mount = "/tmp/.mount_auracoXyZ12";
        let mut vars = map(&[
            ("APPDIR", mount),
            (
                "APPIMAGE",
                "/home/me/Applications/Ghost_0.1.0_amd64.AppImage",
            ),
            ("ARGV0", "/home/me/Applications/auracode.AppImage"),
            ("OWD", "/home/me"),
            (
                "PATH",
                "/tmp/.mount_auracoXyZ12/usr/bin/:/tmp/.mount_auracoXyZ12/bin/:/usr/local/bin:/usr/bin",
            ),
            (
                "LD_LIBRARY_PATH",
                "/tmp/.mount_auracoXyZ12/usr/lib/:/tmp/.mount_auracoXyZ12/lib64/:",
            ),
            (
                "XDG_DATA_DIRS",
                "/tmp/.mount_auracoXyZ12/usr/share/:/usr/share:/usr/local/share:/usr/share",
            ),
            ("PYTHONHOME", "/tmp/.mount_auracoXyZ12/usr/"),
            ("PYTHONDONTWRITEBYTECODE", "1"),
            ("GTK_DATA_PREFIX", mount),
            ("GTK_EXE_PREFIX", "/tmp/.mount_auracoXyZ12//usr"),
            ("GTK_THEME", "Adwaita:dark"),
            (
                "GIO_MODULE_DIR",
                "/tmp/.mount_auracoXyZ12//usr/lib/gio/modules",
            ),
            ("HOME", "/home/me"),
            ("SHELL", "/bin/bash"),
            // Shares the mount's prefix without being inside it.
            ("NOT_INSIDE", "/tmp/.mount_auracoXyZ12-other/lib"),
        ]);
        let removed = strip_launch_context(&mut vars);
        assert_eq!(
            removed,
            [
                "APPDIR",
                "APPIMAGE",
                "ARGV0",
                "GIO_MODULE_DIR",
                "GTK_DATA_PREFIX",
                "GTK_EXE_PREFIX",
                "GTK_THEME",
                "LD_LIBRARY_PATH",
                "OWD",
                "PATH",
                "PYTHONDONTWRITEBYTECODE",
                "PYTHONHOME",
                "XDG_DATA_DIRS",
            ]
        );
        assert_eq!(
            names(&vars),
            ["HOME", "NOT_INSIDE", "PATH", "SHELL", "XDG_DATA_DIRS"]
        );
        let value = |name: &str| vars[&EnvKey::from(name)].to_string_lossy().into_owned();
        assert_eq!(value("PATH"), "/usr/local/bin:/usr/bin");
        assert_eq!(value("XDG_DATA_DIRS"), "/usr/share:/usr/local/share");
        assert_eq!(value("NOT_INSIDE"), "/tmp/.mount_auracoXyZ12-other/lib");
    }

    /// What an image with GStreamer's plugins adds: its launcher's and its GStreamer hook's
    /// variables, and the ones Ghost's own `main` set for the image, all go.
    #[cfg(unix)]
    #[test]
    fn an_appimages_gstreamer_context_is_stripped() {
        let mount = "/tmp/.mount_auracoAbC34";
        let mut vars = map(&[
            ("APPDIR", mount),
            (
                "GST_PLUGIN_SYSTEM_PATH",
                "/tmp/.mount_auracoAbC34/usr/lib/gstreamer:",
            ),
            (
                "GST_PLUGIN_SYSTEM_PATH_1_0",
                "/tmp/.mount_auracoAbC34/usr/lib/gstreamer-1.0:/tmp/.mount_auracoAbC34/usr/lib/gstreamer-1.0",
            ),
            (
                "GST_PLUGIN_PATH_1_0",
                "/tmp/.mount_auracoAbC34/usr/lib/gstreamer-1.0",
            ),
            (
                "GST_PLUGIN_SCANNER_1_0",
                "/tmp/.mount_auracoAbC34/usr/lib/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner",
            ),
            (
                "GST_PTP_HELPER_1_0",
                "/tmp/.mount_auracoAbC34/usr/lib/gstreamer1.0/gstreamer-1.0/gst-ptp-helper",
            ),
            ("GST_REGISTRY_REUSE_PLUGIN_SCANNER", "no"),
            ("GHOST_LAUNCH_SET", "GST_REGISTRY_1_0"),
            (
                "GST_REGISTRY_1_0",
                "/home/me/.cache/si.auracode.desktop/gstreamer-1.0/image-x86_64.bin",
            ),
            ("HOME", "/home/me"),
        ]);
        assert_eq!(
            strip_launch_context(&mut vars),
            [
                "APPDIR",
                "GHOST_LAUNCH_SET",
                "GST_PLUGIN_PATH_1_0",
                "GST_PLUGIN_SCANNER_1_0",
                "GST_PLUGIN_SYSTEM_PATH",
                "GST_PLUGIN_SYSTEM_PATH_1_0",
                "GST_PTP_HELPER_1_0",
                "GST_REGISTRY_1_0",
                "GST_REGISTRY_REUSE_PLUGIN_SCANNER",
            ]
        );
        assert_eq!(names(&vars), ["HOME"]);

        // An image without plugins: Ghost pointed GStreamer at the system's folders itself.
        let mut vars = map(&[
            ("APPDIR", mount),
            ("GST_PLUGIN_SYSTEM_PATH_1_0", "/usr/lib/gstreamer-1.0"),
            (
                "GHOST_LAUNCH_SET",
                "__NV_DISABLE_EXPLICIT_SYNC:GST_REGISTRY_1_0:GST_PLUGIN_SYSTEM_PATH_1_0",
            ),
            ("__NV_DISABLE_EXPLICIT_SYNC", "1"),
            ("GST_REGISTRY_1_0", "/home/me/.cache/x/system-x86_64.bin"),
            // The person's own, without the GStreamer hook's evidence, stays.
            ("GST_REGISTRY_REUSE_PLUGIN_SCANNER", "no"),
            ("HOME", "/home/me"),
        ]);
        strip_launch_context(&mut vars);
        assert_eq!(names(&vars), ["GST_REGISTRY_REUSE_PLUGIN_SCANNER", "HOME"]);
    }

    /// Without the GTK hook's evidence a `GTK_THEME` is the person's own; without `APPDIR`, or
    /// with one that is not an absolute folder, nothing is taken for an AppImage's.
    #[cfg(unix)]
    #[test]
    fn without_an_appimage_mount_nothing_extra_is_stripped() {
        let mut vars = map(&[
            ("APPDIR", "/opt/someapp"),
            ("GTK_THEME", "Adwaita:dark"),
            ("PYTHONDONTWRITEBYTECODE", "1"),
            ("PATH", "/usr/bin"),
        ]);
        assert_eq!(strip_launch_context(&mut vars), ["APPDIR"]);
        assert_eq!(
            names(&vars),
            ["GTK_THEME", "PATH", "PYTHONDONTWRITEBYTECODE"]
        );
        for appdir in ["/", "relative/dir", ""] {
            let mut vars = map(&[
                ("APPDIR", appdir),
                ("PATH", "/usr/bin:relative/dir/bin"),
                ("LD_LIBRARY_PATH", "/usr/lib"),
            ]);
            assert_eq!(strip_launch_context(&mut vars), ["APPDIR"], "{appdir:?}");
            assert_eq!(
                vars[&EnvKey::from("PATH")],
                OsString::from("/usr/bin:relative/dir/bin")
            );
            assert!(vars.contains_key(&EnvKey::from("LD_LIBRARY_PATH")));
        }
        let mut plain = map(&[("PATH", "/usr/bin"), ("GTK_THEME", "Adwaita:dark")]);
        assert!(strip_launch_context(&mut plain).is_empty());
    }

    /// What Ghost set for itself at launch goes; the same variable set by the person stays.
    #[test]
    fn variables_auracode_set_at_launch_are_stripped() {
        let mut vars = map(&[
            (
                "GHOST_LAUNCH_SET",
                "__NV_DISABLE_EXPLICIT_SYNC::NOT_PRESENT",
            ),
            ("__NV_DISABLE_EXPLICIT_SYNC", "1"),
            ("PATH", "/usr/bin"),
        ]);
        assert_eq!(
            strip_launch_context(&mut vars),
            ["GHOST_LAUNCH_SET", "__NV_DISABLE_EXPLICIT_SYNC"]
        );
        assert_eq!(names(&vars), ["PATH"]);
        let mut own = map(&[("__NV_DISABLE_EXPLICIT_SYNC", "1"), ("PATH", "/usr/bin")]);
        assert!(strip_launch_context(&mut own).is_empty());
        assert_eq!(names(&own), ["PATH", "__NV_DISABLE_EXPLICIT_SYNC"]);
    }

    /// On Windows too: the scrub ignores case there, launch context never does.
    #[test]
    fn launch_context_names_match_case_sensitively_on_every_os() {
        let mut vars = map(&[
            ("npm_config_user_agent", "pnpm/12"),
            ("NPM_TOKEN", "npm_0123456789abcdef"),
            ("NPM_CONFIG_REGISTRY", "https://registry.example.com/"),
            ("Pnpm_Store", "/x"),
            ("tauri_cli_flag", "1"),
            ("Term_Program", "vscode"),
            ("init_cwd", "/repo"),
            ("shell_session_dir", "/y"),
        ]);
        let removed = strip_launch_context(&mut vars);
        assert_eq!(removed, ["npm_config_user_agent"]);
        // Sorted here: a Windows map orders its keys ignoring case.
        let mut kept = names(&vars);
        kept.sort();
        assert_eq!(
            kept,
            [
                "NPM_CONFIG_REGISTRY",
                "NPM_TOKEN",
                "Pnpm_Store",
                "Term_Program",
                "init_cwd",
                "shell_session_dir",
                "tauri_cli_flag",
            ]
        );
    }

    #[test]
    fn child_env_scrubs_then_sets() {
        let resolved = ResolvedEnv {
            vars: map(&[
                ("PATH", "/usr/bin"),
                ("CLAUDECODE", "1"),
                ("CLAUDE_CODE_EXECUTABLE", ""),
            ]),
            source: EnvSource::WindowsRegistry,
            warnings: Vec::new(),
            resolved_at_ms: 0,
        };
        // A `set` entry wins even for a name the scrub removes.
        let set = vec![
            (
                OsString::from("CLAUDE_CODE_EXECUTABLE"),
                OsString::from("/usr/local/bin/claude"),
            ),
            (OsString::from("CLAUDECODE"), OsString::from("set-by-us")),
        ];
        let (vars, report) = child_env(&resolved, &set);
        assert_eq!(report.removed, ["CLAUDECODE"]);
        assert_eq!(
            vars.get(&EnvKey::from("CLAUDE_CODE_EXECUTABLE")),
            Some(&OsString::from("/usr/local/bin/claude"))
        );
        assert_eq!(
            vars.get(&EnvKey::from("CLAUDECODE")),
            Some(&OsString::from("set-by-us"))
        );
        assert_eq!(
            vars.get(&EnvKey::from("PATH")),
            Some(&OsString::from("/usr/bin"))
        );
        // The resolved env itself is untouched.
        assert!(resolved.vars.contains_key(&EnvKey::from("CLAUDECODE")));
    }

    #[test]
    fn resolved_env_debug_never_prints_values() {
        let env = ResolvedEnv {
            // Joined by the compiler: written whole, a secret scanner takes it for a real one.
            vars: map(&[("ANTHROPIC_API_KEY", concat!("sk-ant-", "api03-secret"))]),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        };
        let text = format!("{env:?}");
        assert!(text.contains("ANTHROPIC_API_KEY"), "{text}");
        assert!(!text.contains("secret"), "{text}");
    }

    #[test]
    fn secret_values_are_those_of_secret_looking_names() {
        let vars = map(&[
            ("ANTHROPIC_API_KEY", "sk-ant-api03-abcdefghijklmnop"),
            ("GITHUB_TOKEN", "ghp_0123456789abcdefghij"),
            (
                "npm_config_//registry.npmjs.org/:_authToken",
                "npm_0123456789abcdef",
            ),
            (
                "AWS_SECRET_ACCESS_KEY",
                concat!("wJalrXUtn", "FEMI/K7MDENG"),
            ),
            ("DB_PASSWORD", "hunter2hunter2"),
            ("GOOGLE_APPLICATION_CREDENTIALS", "/Users/me/gcp.json"),
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/Users/me"),
        ]);
        let mut values = secret_values(&vars);
        values.sort();
        assert_eq!(
            values,
            [
                "/Users/me/gcp.json",
                "ghp_0123456789abcdefghij",
                "hunter2hunter2",
                "npm_0123456789abcdef",
                "sk-ant-api03-abcdefghijklmnop",
                "wJalrXUtnFEMI/K7MDENG",
            ]
        );
    }

    /// A config whose shell cannot start: every attempt falls back, and nothing of the user's
    /// own runs.
    fn unstartable(base: EnvMap) -> EnvResolverConfig {
        EnvResolverConfig {
            base: Some(base),
            shell: Some(PathBuf::from("/nonexistent/aura-test/zsh")),
            home: Some(std::env::temp_dir()),
            known_dirs: Some(Vec::new()),
            ..EnvResolverConfig::standard()
        }
    }

    #[tokio::test]
    async fn the_resolver_hands_each_envs_secrets_to_the_redactor() {
        let redactor = Arc::new(Redactor::standard());
        let config = EnvResolverConfig {
            redactor: Some(Arc::clone(&redactor)),
            ..unstartable(map(&[
                ("PATH", "/usr/bin"),
                ("OPENAI_API_KEY", "sk-proj-0123456789abcdef"),
            ]))
        };
        let printed = format!("{config:?}");
        assert!(printed.contains("OPENAI_API_KEY"), "{printed}");
        assert!(!printed.contains("sk-proj"), "no values: {printed}");
        let resolver = EnvResolver::start(config, LeftoverRegistry::disabled(), Handle::current());
        let env = resolver.get().await;
        // Unix falls back to the base env; Windows adds the registry's variables to it.
        assert!(env.vars.contains_key(&EnvKey::from("OPENAI_API_KEY")));
        assert_eq!(
            redactor.redact_text("401: bad key sk-proj-0123456789abcdef"),
            "401: bad key <SECRET>"
        );
        assert!(!format!("{resolver:?}").contains("sk-proj"));
    }

    /// An attempt whose task never answers (its runtime shut down first) still hands its env's
    /// secrets to the redactor before anyone gets that env, and its fallback becomes current.
    #[test]
    fn a_failed_attempt_still_hands_its_secrets_to_the_redactor() {
        let doomed = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let redactor = Arc::new(Redactor::standard());
        let config = EnvResolverConfig {
            redactor: Some(Arc::clone(&redactor)),
            ..unstartable(map(&[
                ("PATH", "/usr/bin"),
                ("OPENAI_API_KEY", "sk-proj-fallback-0123456789"),
                ("TERM_PROGRAM", "vscode"),
            ]))
        };
        let resolver = EnvResolver::start(
            config,
            LeftoverRegistry::disabled(),
            doomed.handle().clone(),
        );
        // A current-thread runtime runs nothing until driven: its attempt never started.
        drop(doomed);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let env = runtime.block_on(resolver.get());
        assert_eq!(
            env.source,
            EnvSource::Fallback {
                reason: "the environment could not be read".into()
            }
        );
        assert_eq!(
            names(&env.vars),
            ["OPENAI_API_KEY", "PATH"],
            "the configured base, without its launch context"
        );
        assert_eq!(
            redactor.redact_text("401: bad key sk-proj-fallback-0123456789"),
            "401: bad key <SECRET>"
        );
        assert!(
            resolver
                .current()
                .is_some_and(|current| Arc::ptr_eq(&current, &env))
        );
        assert!(matches!(resolver.status(), ResolverStatus::Fallback { .. }));
        assert!(Arc::ptr_eq(&runtime.block_on(resolver.get()), &env));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_shell_that_cannot_start_falls_back_to_the_env_without_launch_context() {
        let config = unstartable(map(&[("PATH", "/usr/bin"), ("TERM_PROGRAM", "vscode")]));
        let resolver = EnvResolver::start(config, LeftoverRegistry::disabled(), Handle::current());
        assert_eq!(resolver.status(), ResolverStatus::Resolving);
        assert!(resolver.current().is_none());
        let env = resolver.get().await;
        assert_eq!(names(&env.vars), ["PATH"]);
        let EnvSource::Fallback { reason } = &env.source else {
            panic!("a fallback: {:?}", env.source);
        };
        assert!(reason.contains("/nonexistent/aura-test/zsh"), "{reason}");
        assert!(matches!(resolver.status(), ResolverStatus::Fallback { .. }));
        assert!(resolver.current().is_some());
        // A failed attempt is not retried before `retry_after`; `refresh` starts one at once.
        assert!(Arc::ptr_eq(&resolver.get().await, &env));
        resolver.refresh();
        assert_eq!(resolver.status(), ResolverStatus::Resolving);
        assert!(!Arc::ptr_eq(&resolver.get().await, &env));
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn a_failed_attempt_is_retried_after_a_minute() {
        let resolver = EnvResolver::start(
            unstartable(map(&[("PATH", "/usr/bin")])),
            LeftoverRegistry::disabled(),
            Handle::current(),
        );
        let first = resolver.get().await;
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(Arc::ptr_eq(&resolver.get().await, &first), "too early");
        tokio::time::advance(Duration::from_secs(2)).await;
        let second = resolver.get().await;
        assert!(!Arc::ptr_eq(&second, &first), "retried");
        assert!(Arc::ptr_eq(&resolver.get().await, &second));
    }

    #[cfg(unix)]
    #[test]
    fn known_dirs_include_the_npmrc_prefix() {
        let home = std::path::Path::new("/Users/me");
        assert_eq!(
            npmrc_prefix(
                "registry=https://r.example\n; prefix=/ignored\nprefix = \"~/.npm-global\"\n",
                home
            ),
            Some(PathBuf::from("/Users/me/.npm-global"))
        );
        assert_eq!(
            npmrc_prefix("prefix=/opt/npm\n", home),
            Some(PathBuf::from("/opt/npm"))
        );
        assert_eq!(npmrc_prefix("prefix=relative/dir\n", home), None);
        assert_eq!(npmrc_prefix("# prefix=/x\n", home), None);
        let dirs = standard_known_dirs(home);
        for expected in [
            "/Users/me/.local/bin",
            "/Users/me/.asdf/shims",
            "/Users/me/.local/share/mise/shims",
            "/opt/homebrew/bin",
            "/usr/local/bin",
        ] {
            assert!(dirs.contains(&PathBuf::from(expected)), "{expected}");
        }
    }

    #[test]
    fn npm_prefix_reads_are_bounded_and_other_fields_never_become_paths() {
        let root = tempfile::tempdir().expect("temp");
        let prefix = root.path().join("global");
        let config = root.path().join(".npmrc");
        std::fs::write(
            &config,
            format!(
                "//registry.example/:_authToken=private\nprefix={}\n",
                prefix.display()
            ),
        )
        .unwrap();
        assert_eq!(read_npmrc_prefix(root.path()), Some(prefix));
        std::fs::write(&config, "x".repeat(64 * 1024 + 1)).unwrap();
        assert_eq!(read_npmrc_prefix(root.path()), None);
        assert_eq!(npmrc_prefix("prefix=relative\n", root.path()), None);
        assert_eq!(npmrc_prefix("prefix=~/bad\0path\n", root.path()), None);
    }

    #[test]
    fn known_dirs_preserve_path_priority_and_ignore_missing_or_relative_entries() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let mut vars = EnvMap::new();
        vars.insert(
            EnvKey::from("PATH"),
            std::env::join_paths([&first]).unwrap(),
        );
        append_known_dirs(
            &mut vars,
            &[
                second.clone(),
                first.clone(),
                root.path().join("missing"),
                PathBuf::from("."),
            ],
        );
        assert_eq!(
            std::env::split_paths(&vars[&EnvKey::from("PATH")]).collect::<Vec<_>>(),
            [first, second]
        );
    }

    #[cfg(unix)]
    #[test]
    fn known_dirs_appended() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        for relative in [
            ".local/bin",
            ".asdf/shims",
            ".local/share/mise/shims",
            "npm-prefix/bin",
        ] {
            std::fs::create_dir_all(home.join(relative)).expect("mkdir");
        }
        std::fs::write(
            home.join(".npmrc"),
            format!("prefix={}\n", home.join("npm-prefix").display()),
        )
        .expect("npmrc");
        let mut known = standard_known_dirs(home);
        known.retain(|dir| dir.starts_with(home));
        let present = home.join(".local/bin");
        let mut vars = map(&[("HOME", "/h")]);
        vars.insert(
            EnvKey::from("PATH"),
            std::env::join_paths(["/usr/bin", present.to_str().expect("utf-8")]).expect("join"),
        );
        append_known_dirs(&mut vars, &known);
        let path: Vec<PathBuf> =
            std::env::split_paths(vars.get(&EnvKey::from("PATH")).expect("PATH")).collect();
        assert_eq!(
            path,
            [
                PathBuf::from("/usr/bin"),
                present,
                home.join(".asdf/shims"),
                home.join(".local/share/mise/shims"),
                home.join("npm-prefix/bin"),
            ],
            "existing dirs missing from PATH are appended in order; missing ones are not"
        );
        // Nothing to add leaves PATH as it was.
        let before = vars.clone();
        append_known_dirs(&mut vars, &known);
        assert_eq!(
            vars.get(&EnvKey::from("PATH")),
            before.get(&EnvKey::from("PATH"))
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_reads_the_registry_over_the_env_without_launch_context() {
        let config = unstartable(map(&[
            ("PATH", "C:\\from-the-process"),
            ("TERM_PROGRAM", "vscode"),
            ("GHOST_TEST_BASE", "kept"),
        ]));
        let resolver = EnvResolver::start(config, LeftoverRegistry::disabled(), Handle::current());
        let env = resolver.get().await;
        assert_eq!(env.source, EnvSource::WindowsRegistry);
        assert!(!env.vars.contains_key(&EnvKey::from("TERM_PROGRAM")));
        assert_eq!(
            env.vars.get(&EnvKey::from("GHOST_TEST_BASE")),
            Some(&OsString::from("kept"))
        );
        let path = env
            .vars
            .get(&EnvKey::from("PATH"))
            .expect("PATH")
            .to_string_lossy()
            .into_owned();
        assert!(path.to_ascii_lowercase().contains("system32"), "{path}");
        assert!(
            path.ends_with("C:\\from-the-process"),
            "process entries come last: {path}"
        );
        assert!(matches!(resolver.status(), ResolverStatus::Ready { .. }));
    }

    #[tokio::test]
    async fn a_fixed_resolver_returns_its_env() {
        let env = ResolvedEnv {
            vars: map(&[("PATH", "/bin")]),
            source: EnvSource::LoginShell {
                shell: PathBuf::from("/bin/zsh"),
                interactive: true,
                took_ms: 120,
            },
            warnings: Vec::new(),
            resolved_at_ms: 5,
        };
        let resolver = EnvResolver::fixed(env);
        assert_eq!(resolver.get().await.resolved_at_ms, 5);
        assert_eq!(
            resolver.status(),
            ResolverStatus::Ready {
                source: EnvSource::LoginShell {
                    shell: PathBuf::from("/bin/zsh"),
                    interactive: true,
                    took_ms: 120,
                },
                took_ms: 120,
                vars: 1,
            }
        );
    }
}
