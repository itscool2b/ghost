//! What to run and how it starts: launch specs, the spawner seam, and short-lived helpers.
//!
//! Every spawn gets an absolute program path, the complete environment (`env_clear` plus
//! [`child_env`](crate::env::child_env): resolved, scrubbed, then `set_env`), its own process group on
//! Unix (a new session for helpers that ask for one) or a kill-on-close job object with
//! `CREATE_NO_WINDOW` on Windows, and all three pipes. The process cwd is checked first, on the
//! blocking pool, because Unix reports a missing cwd as a missing program and a macOS privacy prompt
//! can hold the call until the user answers.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::control::{
    ChildControl, ControlState, ExitInfo, ProcessControl, native_child, wait_child,
};
use crate::env::{EnvMap, EnvSource, ResolvedEnv, child_env, process_env, strip_launch_context};
use crate::io::{AgentIo, LineSink, stdout_channel};
use crate::log::{LogNote, ProtocolLog, now_millis};
use crate::registry::{LeftoverRegistry, RegistryHandle};
use crate::stderr::StderrTail;
use crate::{PIPE_EOF_GRACE, STDERR_TAIL_BYTES};

/// The longest working folder Windows accepts for a new process, in UTF-16 units.
#[cfg(any(windows, test))]
const WINDOWS_MAX_CWD: usize = 259;

/// Where the agent process runs. An ACP agent always gets the chat folder as the protocol's `cwd`,
/// wherever its process runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessCwd {
    /// The chat's folder ([`SpawnSpec::folder`]).
    ChatFolder,
    /// A folder of its own, such as an npm adapter's install folder, so nothing in the chat folder
    /// (a `.npmrc`, a committed `node_modules`) decides what runs.
    Fixed(PathBuf),
}

/// A descendant that is a vendor daemon and must survive shutdown (Hermes' gateway, Claude's
/// daemon): the file name of `argv[0]` equals `program` when set, and the words of `args_in_order`
/// appear in argv in that order (not necessarily adjacent). A match is spared with its whole subtree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonMatch {
    pub program: Option<String>,
    pub args_in_order: Vec<String>,
}

/// How to start one agent.
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    /// "hermes acp", for logs and status text.
    pub label: String,
    /// Absolute; symlinks are not resolved (agents launch through their stable symlink or shim).
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: ProcessCwd,
    /// Always set, after the scrub. Names are logged, values never.
    pub set_env: Vec<(OsString, OsString)>,
    /// Spared by the kill rule.
    pub daemons: Vec<DaemonMatch>,
    pub startup_timeout: Duration,
}

/// Prints `set_env` names only: values can be credentials.
impl fmt::Debug for LaunchSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let set_env: Vec<_> = self.set_env.iter().map(|(name, _)| name).collect();
        f.debug_struct("LaunchSpec")
            .field("label", &self.label)
            .field("program", &self.program)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("set_env", &set_env)
            .field("daemons", &self.daemons)
            .field("startup_timeout", &self.startup_timeout)
            .finish()
    }
}

/// One spawn: the launch, the chat folder ([`ProcessCwd::ChatFolder`] resolves to it), the env the
/// session started with, and the stdout line cap ([`crate::MAX_LINE_BYTES`] for agents).
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub launch: LaunchSpec,
    pub folder: PathBuf,
    pub env: Arc<ResolvedEnv>,
    pub max_line_bytes: usize,
}

/// Starts agent processes. [`Spawner`] is the real one; tests use `testing::FakeSpawner`.
pub trait ProcessSpawner: Send + Sync + 'static {
    /// Spawns an agent with all three pipes; `log` receives every stdin, stdout and stderr line.
    fn spawn(
        &self,
        spec: SpawnSpec,
        log: ProtocolLog,
    ) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>>;
}

/// Spawns real processes and records each agent in the leftover registry.
#[derive(Debug)]
pub struct Spawner {
    registry: Arc<LeftoverRegistry>,
    /// Where the pipe tasks and the waiter run: constructors never assume a runtime context.
    rt: Handle,
}

impl Spawner {
    pub fn new(registry: Arc<LeftoverRegistry>, rt: Handle) -> Arc<Spawner> {
        Arc::new(Spawner { registry, rt })
    }

    pub fn registry(&self) -> &Arc<LeftoverRegistry> {
        &self.registry
    }

    /// An interactive, owned terminal. The caller supplies a native executable and argument vector.
    pub async fn spawn_pty(
        &self,
        spec: SpawnSpec,
        size: crate::pty::PtySize,
    ) -> Result<crate::pty::PtyProcess, SpawnError> {
        crate::pty::spawn(Arc::clone(&self.registry), self.rt.clone(), spec, size).await
    }

    /// Starts a native binary transport with the ordinary environment and tree-ownership rules.
    /// No stdin/stdout/stderr content is logged. Callers must consume both output streams.
    pub async fn spawn_raw(&self, spec: SpawnSpec) -> Result<crate::raw::RawProcess, SpawnError> {
        let cwd = match &spec.launch.cwd {
            ProcessCwd::ChatFolder => spec.folder,
            ProcessCwd::Fixed(dir) => dir.clone(),
        };
        let (vars, _) = child_env(&spec.env, &spec.launch.set_env);
        check(&self.rt, &cwd, &spec.launch.program, &vars).await?;
        let command = command(
            &spec.launch.program,
            &spec.launch.args,
            &cwd,
            &vars,
            Stdio::piped(),
        );
        let mut child = start(&self.rt, command, false, &spec.launch.program)?;
        let missing_pipe = || SpawnError::Spawn {
            program: spec.launch.program.display().to_string(),
            source: io::Error::other("the owned transport is missing a pipe"),
        };
        let input = child.stdin().take().ok_or_else(missing_pipe)?;
        let stdout = child.stdout().take().ok_or_else(missing_pipe)?;
        let stderr = child.stderr().take().ok_or_else(missing_pipe)?;
        let (stdin, receiver) = crate::raw::ByteSink::channel();
        let control = own_child(
            Arc::clone(&self.registry),
            self.rt.clone(),
            child,
            &spec.launch,
            stdin.clone(),
            ProtocolLog::disabled(),
            Arc::new(StderrTail::default()),
        )
        .await?;
        self.rt.spawn(receiver.forward(input));
        Ok(crate::raw::RawProcess {
            stdin,
            stdout,
            stderr,
            control,
        })
    }
}

impl ProcessSpawner for Spawner {
    fn spawn(
        &self,
        spec: SpawnSpec,
        log: ProtocolLog,
    ) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        let registry = Arc::clone(&self.registry);
        let rt = self.rt.clone();
        spawn_agent(registry, rt, spec, log).boxed()
    }
}

/// Starts one agent: checks, the complete env, the wrappers, the registry entry, the pipe tasks
/// and the waiter.
async fn spawn_agent(
    registry: Arc<LeftoverRegistry>,
    rt: Handle,
    spec: SpawnSpec,
    log: ProtocolLog,
) -> Result<SpawnedProcess, SpawnError> {
    let SpawnSpec {
        launch,
        folder,
        env,
        max_line_bytes,
    } = spec;
    let cwd = match &launch.cwd {
        ProcessCwd::ChatFolder => folder,
        ProcessCwd::Fixed(dir) => dir.clone(),
    };
    let (vars, scrubbed) = child_env(&env, &launch.set_env);
    check(&rt, &cwd, &launch.program, &vars).await?;
    let command = command(&launch.program, &launch.args, &cwd, &vars, Stdio::piped());
    let mut child = start(&rt, command, false, &launch.program)?;
    let pid = child_pid(child.as_ref(), &launch.program)?;
    let stdin_pipe = child.stdin().take();
    let stdout_pipe = child.stdout().take();
    let stderr_pipe = child.stderr().take();

    log.note(LogNote::Spawn {
        label: launch.label.clone(),
        program: launch.program.display().to_string(),
        args: launch
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        cwd: cwd.display().to_string(),
        removed: scrubbed.removed,
        set: launch
            .set_env
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect(),
        pid,
    });

    let tail = Arc::new(StderrTail::default());
    let (sink, stdin_rx) = LineSink::channel();
    let (stdout_tx, source) = stdout_channel();
    let control = own_child(
        registry,
        rt.clone(),
        child,
        &launch,
        sink.clone(),
        log.clone(),
        Arc::clone(&tail),
    )
    .await?;
    if let Some(pipe) = stdin_pipe {
        rt.spawn(crate::io::write_stdin(pipe, stdin_rx, log.clone()));
    }
    if let Some(pipe) = stdout_pipe {
        rt.spawn(crate::io::read_stdout(
            pipe,
            stdout_tx,
            log.clone(),
            max_line_bytes,
        ));
    }
    if let Some(pipe) = stderr_pipe {
        rt.spawn(crate::stderr::drain(pipe, Arc::clone(&tail), log.clone()));
    }
    Ok(SpawnedProcess {
        spawn_id: log.spawn_id().to_owned(),
        io: AgentIo {
            stdin: sink,
            stdout: source,
        },
        control,
        log,
    })
}

/// Attach registry, direct-child exit observation and existing tree cleanup to a prepared child.
/// The guard covers cancellation while registration is written on the blocking pool.
pub(crate) async fn own_child(
    registry: Arc<LeftoverRegistry>,
    rt: Handle,
    child: Box<dyn ChildWrapper>,
    launch: &LaunchSpec,
    stdin: impl Into<crate::control::StdinControl>,
    log: ProtocolLog,
    tail: Arc<StderrTail>,
) -> Result<ProcessControl, SpawnError> {
    let pid = child_pid(child.as_ref(), &launch.program)?;
    let mut guard = SpawnGuard {
        #[cfg(unix)]
        pid,
        rt: rt.clone(),
        registration: Registration::new(registry.handle(), pid),
        armed: true,
    };
    #[cfg(unix)]
    let start_time = {
        let registration = guard.registration.clone();
        let label = launch.label.clone();
        let daemons = launch.daemons.clone();
        rt.spawn_blocking(move || register_agent(&registration, pid, label, daemons))
            .await
            .unwrap_or(crate::ptable::StartTime::Unknown)
    };
    let (exit_tx, exit_rx) = watch::channel(None);
    #[cfg(windows)]
    let (kill_tx, kill_rx) = tokio::sync::mpsc::unbounded_channel();
    #[cfg(unix)]
    let group_gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
    rt.spawn(wait_child(
        child,
        exit_tx,
        log.clone(),
        #[cfg(unix)]
        (pid, Arc::clone(&group_gone)),
        #[cfg(windows)]
        kill_rx,
    ));
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut state = ControlState::new(
        pid,
        #[cfg(unix)]
        start_time,
        stdin,
        exit_rx,
        tail,
        log.clone(),
        rt,
        registry.handle(),
        #[cfg(unix)]
        launch.daemons.clone(),
        #[cfg(unix)]
        group_gone,
        #[cfg(windows)]
        kill_tx,
    );
    // A job of its own, nested in the one that ends it, lists what it starts even after their
    // parent exits (crate::tree). Without it the parent chain still lists most of the tree.
    #[cfg(windows)]
    {
        state.tree = crate::tree::TreeJob::attach(pid);
    }
    guard.armed = false;
    Ok(ProcessControl::new(Arc::new(ChildControl::new(state))))
}

/// Reads a process's start time and records it in the registry (blocking).
#[cfg(unix)]
fn register_agent(
    registration: &Registration,
    pid: u32,
    label: String,
    daemons: Vec<DaemonMatch>,
) -> crate::ptable::StartTime {
    let start = crate::ptable::start_time(pid).unwrap_or(crate::ptable::StartTime::Unknown);
    registration.register(crate::registry::LeftoverEntry {
        pid,
        pgid: pid,
        sid: session_of(pid),
        start: start.clone(),
        label,
        spawned_at_ms: now_millis(),
        daemons,
        escaped: Vec::new(),
    });
    start
}

/// A process's registry entry, which its spawn may abandon while the entry is still being written:
/// whichever comes second (the write or the abandonment) removes it, so a dropped spawn never
/// leaves a stale entry.
#[derive(Debug, Clone)]
struct Registration {
    registry: RegistryHandle,
    pid: u32,
    state: Arc<Mutex<RegistrationState>>,
}

#[derive(Debug, Default)]
struct RegistrationState {
    written: bool,
    abandoned: bool,
}

impl Registration {
    fn new(registry: RegistryHandle, pid: u32) -> Registration {
        Registration {
            registry,
            pid,
            state: Arc::new(Mutex::new(RegistrationState::default())),
        }
    }

    /// Blocking: writes the entry, unless the spawn was abandoned first. A failure is a warning.
    #[cfg(unix)]
    fn register(&self, entry: crate::registry::LeftoverEntry) {
        let mut state = lock(&self.state);
        if state.abandoned {
            return;
        }
        if let Err(error) = self.registry.register(entry) {
            tracing::warn!(pid = self.pid, %error, "could not record a process in the leftover registry");
        }
        state.written = true;
    }

    /// Blocking: removes the entry now, or keeps it from being written.
    fn abandon(&self) {
        let mut state = lock(&self.state);
        state.abandoned = true;
        if state.written {
            self.registry.unregister(self.pid);
        }
    }
}

/// Kills a half-started agent's tree if its spawn is dropped before the control takes over.
struct SpawnGuard {
    #[cfg(unix)]
    pid: u32,
    rt: Handle,
    registration: Registration,
    armed: bool,
}

impl Drop for SpawnGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        #[cfg(unix)]
        kill_group(self.pid);
        // Windows: the job closes with the child, which drops next.
        let registration = self.registration.clone();
        drop(self.rt.spawn_blocking(move || registration.abandon()));
    }
}

#[cfg(unix)]
fn session_of(pid: u32) -> Option<u32> {
    let raw = i32::try_from(pid).ok()?;
    nix::unistd::getsid(Some(nix::unistd::Pid::from_raw(raw)))
        .ok()
        .and_then(|sid| u32::try_from(sid.as_raw()).ok())
}

/// The cwd and program checks, on the blocking pool: a macOS privacy prompt can hold the call
/// until the user answers, and Unix reports a missing cwd as a missing program, so the cwd goes
/// first.
pub(crate) async fn check(
    rt: &Handle,
    cwd: &Path,
    program: &Path,
    vars: &EnvMap,
) -> Result<(), SpawnError> {
    let name = program.display().to_string();
    let (cwd, program) = (cwd.to_path_buf(), program.to_path_buf());
    #[cfg(windows)]
    let extensions = crate::env::pathext(vars);
    #[cfg(not(windows))]
    let _ = vars;
    rt.spawn_blocking(move || {
        check_cwd(&cwd)?;
        #[cfg(windows)]
        check_program(&program, &extensions)?;
        #[cfg(not(windows))]
        check_program(&program)?;
        Ok(())
    })
    .await
    .unwrap_or_else(|error| {
        Err(SpawnError::Spawn {
            program: name,
            source: io::Error::other(error),
        })
    })
}

/// The process cwd: it exists, is a folder, and may be read. Folders are never canonicalized (a
/// Windows result carries a `\\?\` prefix Node agents mishandle).
fn check_cwd(cwd: &Path) -> Result<(), SpawnError> {
    #[cfg(windows)]
    check_cwd_length(cwd)?;
    let invalid = |reason: &str| SpawnError::InvalidCwd {
        path: cwd.to_path_buf(),
        reason: reason.to_owned(),
    };
    match std::fs::metadata(cwd) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(invalid("it is not a folder")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(invalid("it does not exist"));
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            return Err(SpawnError::CwdNotAllowed {
                path: cwd.to_path_buf(),
            });
        }
        Err(error) => return Err(invalid(&error.to_string())),
    }
    match std::fs::read_dir(cwd) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            Err(SpawnError::CwdNotAllowed {
                path: cwd.to_path_buf(),
            })
        }
        _ => Ok(()),
    }
}

/// Windows refuses a working folder longer than 259 UTF-16 units. Pure, so it is tested
/// everywhere.
#[cfg(any(windows, test))]
pub(crate) fn check_cwd_length(cwd: &Path) -> Result<(), SpawnError> {
    if cwd.to_string_lossy().encode_utf16().count() > WINDOWS_MAX_CWD {
        return Err(SpawnError::InvalidCwd {
            path: cwd.to_path_buf(),
            reason: "it is longer than Windows allows for a program's working folder".into(),
        });
    }
    Ok(())
}

#[cfg(not(windows))]
fn check_program(program: &Path) -> Result<(), SpawnError> {
    let name = || program.display().to_string();
    if !program.is_absolute() {
        return Err(SpawnError::ProgramNotFound { program: name() });
    }
    match std::fs::metadata(program) {
        Ok(meta) if meta.is_file() => {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 == 0 {
                return Err(SpawnError::PermissionDenied { program: name() });
            }
            Ok(())
        }
        Ok(_) => Err(SpawnError::ProgramNotFound { program: name() }),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            Err(SpawnError::PermissionDenied { program: name() })
        }
        Err(_) => Err(SpawnError::ProgramNotFound { program: name() }),
    }
}

#[cfg(windows)]
fn check_program(program: &Path, extensions: &[String]) -> Result<(), SpawnError> {
    let name = || program.display().to_string();
    if !program.is_absolute() || !program.is_file() {
        return Err(SpawnError::ProgramNotFound { program: name() });
    }
    let runnable = program.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy();
        extensions.iter().any(|known| {
            known
                .trim_start_matches('.')
                .eq_ignore_ascii_case(&extension)
        })
    });
    if !runnable {
        return Err(SpawnError::Spawn {
            program: name(),
            source: io::Error::other("it has no extension Windows runs (PATHEXT)"),
        });
    }
    Ok(())
}

/// A command with exactly `vars` as its env and its stdout and stderr piped.
fn command(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    vars: &EnvMap,
    stdin: Stdio,
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(vars.iter().map(|(name, value)| (name.as_os_str(), value)))
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// The spike's wrapper stack: `KillOnDrop` everywhere; Unix adds `ResetSigmask` and a group of its
/// own (a session of its own for helpers that ask for one); Windows adds `CREATE_NO_WINDOW` and a
/// kill-on-close job object, for every agent and helper.
/// Some descriptors cannot be created close-on-exec atomically, such as a PTY pair from `openpty`.
/// Creating and marking them and every spawn are mutually exclusive. On macOS, std also creates
/// subprocess pipes before setting close-on-exec, including the fork/exec status pipe. Another
/// concurrent spawn must not inherit those writers and delay startup until its child exits.
static INHERITANCE: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Held only while spawning a child, never while waiting for its exit or output.
pub(crate) fn inheritable_marked() -> std::sync::RwLockWriteGuard<'static, ()> {
    INHERITANCE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Starts a standard command without racing another owned spawn's descriptor setup.
/// The caller owns the returned child. No guard is held while it runs or while the caller waits.
pub fn spawn_command(command: &mut std::process::Command) -> io::Result<std::process::Child> {
    let _marked = inheritable_marked();
    command.spawn()
}

/// Held while creating descriptors that are not yet close-on-exec.
#[cfg(unix)]
pub(crate) fn creating_inheritable() -> std::sync::RwLockWriteGuard<'static, ()> {
    INHERITANCE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn wrap(command: tokio::process::Command, new_session: bool) -> CommandWrap {
    let mut wrapped = CommandWrap::from(command);
    wrapped.wrap(KillOnDrop);
    #[cfg(unix)]
    {
        use process_wrap::tokio::{ProcessGroup, ProcessSession, ResetSigmask};
        wrapped.wrap(ResetSigmask);
        if new_session {
            wrapped.wrap(ProcessSession);
        } else {
            wrapped.wrap(ProcessGroup::leader());
        }
    }
    #[cfg(windows)]
    {
        use process_wrap::tokio::{CreationFlags, JobObject};
        let _ = new_session;
        wrapped.wrap(CreationFlags(
            windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ));
        wrapped.wrap(JobObject);
    }
    wrapped
}

/// How often a start is tried again while the binary is busy, waiting `BUSY_WAIT` times the attempt
/// before each.
const BUSY_RETRIES: u32 = 3;
const BUSY_WAIT: Duration = Duration::from_millis(10);

/// Spawns inside `rt`, so the child's pipes and reaping belong to it wherever this is polled.
///
/// Linux refuses to run a file someone holds open for writing ("text file busy"). Right after a
/// binary is written, a child that another thread of this process is forking holds it open until
/// its own exec: an agent that just updated itself, or a test's fake binary. That clears within
/// milliseconds, so the start is tried again a few times before it fails.
fn start(
    rt: &Handle,
    command: tokio::process::Command,
    new_session: bool,
    program: &Path,
) -> Result<Box<dyn ChildWrapper>, SpawnError> {
    let _context = rt.enter();
    let mut wrapped = wrap(command, new_session);
    let mut attempt = 0;
    loop {
        let spawned = {
            let _marked = inheritable_marked();
            wrapped.spawn()
        };
        match spawned {
            Ok(child) => return Ok(child),
            Err(source)
                if source.kind() == io::ErrorKind::ExecutableFileBusy && attempt < BUSY_RETRIES =>
            {
                attempt += 1;
                std::thread::sleep(BUSY_WAIT * attempt);
            }
            Err(source) => return Err(spawn_error(program, source)),
        }
    }
}

pub(crate) fn spawn_error(program: &Path, source: io::Error) -> SpawnError {
    let program = program.display().to_string();
    match source.kind() {
        io::ErrorKind::NotFound => SpawnError::ProgramNotFound { program },
        io::ErrorKind::PermissionDenied => SpawnError::PermissionDenied { program },
        _ => SpawnError::Spawn { program, source },
    }
}

fn child_pid(child: &dyn ChildWrapper, program: &Path) -> Result<u32, SpawnError> {
    child.id().ok_or_else(|| SpawnError::Spawn {
        program: program.display().to_string(),
        source: io::Error::other("the process exited before its pid could be read"),
    })
}

/// A running agent: its pipes, its control, and its protocol log.
#[derive(Debug)]
pub struct SpawnedProcess {
    /// Copied from [`ProtocolLog::spawn_id`]: names the log file and prefixes ids minted outside a
    /// turn.
    pub spawn_id: String,
    pub io: AgentIo,
    pub control: ProcessControl,
    pub log: ProtocolLog,
}

/// Why a spawn failed. Every message is a sentence the UI can show.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("{host} can't run agents: {reason}")]
    HostUnavailable { host: String, reason: String },
    #[error("{program} was not found")]
    ProgramNotFound { program: String },
    #[error("{program} can't be run: permission denied")]
    PermissionDenied { program: String },
    #[error("the folder {} can't be used: {reason}", path.display())]
    InvalidCwd { path: PathBuf, reason: String },
    #[error("Ghost isn't allowed to open {}", path.display())]
    CwdNotAllowed { path: PathBuf },
    #[error("spawning needs a Tokio runtime")]
    NoRuntime,
    #[error("could not start {program}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
}

/// The environment a helper starts with. Both are scrubbed.
#[derive(Debug, Clone)]
pub enum HelperEnv {
    /// Ghost's own env minus `strip_launch_context`: the env resolver's shell, which must not
    /// wait for the resolver.
    ProcessEnv,
    Resolved(Arc<ResolvedEnv>),
}

/// A short-lived helper: the env resolver's shell, a `--version` probe, the adapter install.
#[derive(Clone)]
pub struct HelperSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: HelperEnv,
    /// Always set, after the scrub.
    pub set: Vec<(OsString, OsString)>,
    /// Starts the helper in a new session (Unix), so an interactive shell never takes the
    /// foreground of the terminal Ghost was started from.
    pub new_session: bool,
    pub timeout: Duration,
    /// stdout beyond this is dropped (`stdout_truncated`).
    pub max_stdout: usize,
}

/// Prints `set` names only, as [`LaunchSpec`] does: values can be credentials.
impl fmt::Debug for HelperSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let set: Vec<_> = self.set.iter().map(|(name, _)| name).collect();
        f.debug_struct("HelperSpec")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("env", &self.env)
            .field("set", &set)
            .field("new_session", &self.new_session)
            .field("timeout", &self.timeout)
            .field("max_stdout", &self.max_stdout)
            .finish()
    }
}

/// What a helper produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelperOutput {
    /// `None` when it was killed on timeout before it exited.
    pub exit: Option<ExitInfo>,
    pub stdout: Vec<u8>,
    pub stdout_truncated: bool,
    /// Lossy and not redacted.
    pub stderr_tail: String,
    pub timed_out: bool,
}

/// Runs a short-lived helper: stdin `/dev/null`, registered for the sweep, the whole tree killed on
/// timeout and when the returned future is dropped. A helper leaves nothing behind: what its tree
/// still runs after it exits is killed too, and its pipes get [`PIPE_EOF_GRACE`] to end.
pub async fn run_helper(
    registry: &LeftoverRegistry,
    spec: HelperSpec,
) -> Result<HelperOutput, SpawnError> {
    run_helper_inner(registry, spec, None).await
}

/// Runs an owned helper with at most 1 MiB of unlogged stdin, closing the pipe after all bytes are
/// written. Input never enters argv or a temporary file. Output capture and cancellation retain the
/// same bounds and tree ownership as [`run_helper`].
/// Successful pipe delivery is not an application acknowledgment; callers must validate the result.
pub async fn run_helper_input(
    registry: &LeftoverRegistry,
    spec: HelperSpec,
    input: Vec<u8>,
) -> Result<HelperOutput, SpawnError> {
    if input.len() > 1024 * 1024 {
        return Err(SpawnError::Spawn {
            program: spec.program.display().to_string(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "helper input exceeds 1 MiB"),
        });
    }
    run_helper_inner(registry, spec, Some(input)).await
}

async fn run_helper_inner(
    registry: &LeftoverRegistry,
    spec: HelperSpec,
    input: Option<Vec<u8>>,
) -> Result<HelperOutput, SpawnError> {
    let rt = Handle::try_current().map_err(|_| SpawnError::NoRuntime)?;
    let HelperSpec {
        program,
        args,
        cwd,
        env,
        set,
        new_session,
        timeout,
        max_stdout,
    } = spec;
    let env = match env {
        HelperEnv::Resolved(env) => env,
        HelperEnv::ProcessEnv => {
            let mut vars = process_env();
            strip_launch_context(&mut vars);
            Arc::new(ResolvedEnv {
                vars,
                source: EnvSource::Fallback {
                    reason: "Ghost's own environment".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: now_millis(),
            })
        }
    };
    let (vars, _) = child_env(&env, &set);
    check(&rt, &cwd, &program, &vars).await?;
    let mut child = start(
        &rt,
        command(
            &program,
            &args,
            &cwd,
            &vars,
            if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            },
        ),
        new_session,
        &program,
    )?;
    let pid = child_pid(child.as_ref(), &program)?;
    let handle = registry.handle();
    #[cfg(unix)]
    let enabled = handle.is_enabled();
    let mut guard = HelperGuard {
        #[cfg(unix)]
        pid,
        #[cfg(unix)]
        new_session,
        rt: rt.clone(),
        registration: Registration::new(handle, pid),
        readers: Vec::new(),
        done: false,
    };
    #[cfg(unix)]
    if enabled {
        let registration = guard.registration.clone();
        let label = format!(
            "helper {}",
            program.file_name().map_or_else(
                || program.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            )
        );
        let _ = rt
            .spawn_blocking(move || register_agent(&registration, pid, label, Vec::new()))
            .await;
    }

    let stdout = Arc::new(Mutex::new(Captured::default()));
    let tail = Arc::new(StderrTail::default());
    let readers = [
        child
            .stdout()
            .take()
            .map(|pipe| rt.spawn(capture(pipe, Arc::clone(&stdout), max_stdout))),
        child
            .stderr()
            .take()
            .map(|pipe| rt.spawn(tail_into(pipe, Arc::clone(&tail)))),
    ];
    guard.readers = readers
        .iter()
        .flatten()
        .map(|task| task.abort_handle())
        .collect();
    let writer = if let Some(input) = input {
        let mut pipe = child.stdin().take().ok_or_else(|| SpawnError::Spawn {
            program: program.display().to_string(),
            source: io::Error::other("helper stdin pipe is unavailable"),
        })?;
        Some(rt.spawn(async move {
            pipe.write_all(&input).await?;
            pipe.shutdown().await
        }))
    } else {
        None
    };
    if let Some(writer) = &writer {
        guard.readers.push(writer.abort_handle());
    }

    let (exit, timed_out) = match tokio::time::timeout(timeout, wait_direct(child.as_mut())).await {
        Ok(status) => (status.ok().map(ExitInfo::from_status), false),
        Err(_) => {
            guard.kill_tree(child.as_mut(), true).await;
            let _ = tokio::time::timeout(Duration::from_secs(2), wait_direct(child.as_mut())).await;
            (None, true)
        }
    };
    // Whatever its group still runs (a background job of an rc file, say) goes too.
    guard.kill_tree(child.as_mut(), false).await;
    for reader in readers.into_iter().flatten() {
        let _ = tokio::time::timeout(PIPE_EOF_GRACE, reader).await;
    }
    let input_written = if let Some(mut writer) = writer {
        let result = tokio::time::timeout(PIPE_EOF_GRACE, &mut writer).await;
        writer.abort();
        matches!(result, Ok(Ok(Ok(()))))
    } else {
        true
    };
    guard.finish().await;
    if !input_written && !timed_out {
        return Err(SpawnError::Spawn {
            program: program.display().to_string(),
            source: io::Error::new(
                io::ErrorKind::BrokenPipe,
                "helper did not accept its complete input",
            ),
        });
    }
    let captured = std::mem::take(&mut *lock(&stdout));
    Ok(HelperOutput {
        exit,
        stdout: captured.bytes,
        stdout_truncated: captured.truncated,
        stderr_tail: tail.tail(STDERR_TAIL_BYTES),
        timed_out,
    })
}

/// The direct child's exit, never a group-wide or job-wide wait.
async fn wait_direct(child: &mut dyn ChildWrapper) -> io::Result<std::process::ExitStatus> {
    match native_child(child) {
        Some(native) => native.wait().await,
        None => Err(io::Error::other("no process to wait for")),
    }
}

/// Kills a helper's tree when its future is dropped, and unregisters it.
struct HelperGuard {
    #[cfg(unix)]
    pid: u32,
    #[cfg(unix)]
    new_session: bool,
    rt: Handle,
    registration: Registration,
    readers: Vec<tokio::task::AbortHandle>,
    done: bool,
}

impl HelperGuard {
    /// Kills the helper's group, or its job. With `whole_session`, a helper that started a
    /// session of its own loses every process of that session too (one snapshot of the table).
    async fn kill_tree(&self, child: &mut dyn ChildWrapper, whole_session: bool) {
        #[cfg(unix)]
        {
            let _ = child;
            kill_group(self.pid);
            if whole_session && self.new_session {
                let sid = self.pid;
                let _ = self
                    .rt
                    .spawn_blocking(move || crate::shutdown::kill_session_blocking(sid))
                    .await;
            }
        }
        #[cfg(windows)]
        {
            let _ = whole_session;
            let _ = child.start_kill();
        }
    }

    async fn finish(&mut self) {
        self.done = true;
        let registration = self.registration.clone();
        let _ = self.rt.spawn_blocking(move || registration.abandon()).await;
    }
}

impl Drop for HelperGuard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        // Dropped mid-run: the tree dies now (on Windows the job closes with the child next).
        #[cfg(unix)]
        {
            kill_group(self.pid);
            if self.new_session {
                // On a thread of its own, not the blocking pool: a runtime that is shutting down
                // drops this future and takes no more blocking work.
                let sid = self.pid;
                let killer = std::thread::Builder::new()
                    .name("aura-kill-session".into())
                    .spawn(move || crate::shutdown::kill_session_blocking(sid));
                if killer.is_err() {
                    crate::shutdown::kill_session_blocking(sid);
                }
            }
        }
        for reader in &self.readers {
            reader.abort();
        }
        let registration = self.registration.clone();
        drop(self.rt.spawn_blocking(move || registration.abandon()));
    }
}

#[cfg(unix)]
fn kill_group(pgid: u32) {
    if let Ok(raw) = i32::try_from(pgid)
        && raw > 0
    {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(raw),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

/// A helper's stdout, up to its limit.
#[derive(Debug, Default)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Reads to the end, keeping at most `max` bytes: the rest is still read, so the helper never
/// blocks on a full pipe.
async fn capture(mut pipe: impl AsyncRead + Unpin, into: Arc<Mutex<Captured>>, max: usize) {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = match pipe.read(&mut chunk).await {
            Ok(0) => return,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        let mut captured = lock(&into);
        let room = max.saturating_sub(captured.bytes.len());
        let bytes = chunk.get(..read).unwrap_or_default();
        if bytes.len() > room {
            captured.truncated = true;
        }
        captured
            .bytes
            .extend_from_slice(bytes.get(..room.min(bytes.len())).unwrap_or_default());
    }
}

/// Reads stderr to the end into the tail.
async fn tail_into(mut pipe: impl AsyncRead + Unpin, tail: Arc<StderrTail>) {
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) => return,
            Ok(read) => tail.push(chunk.get(..read).unwrap_or_default()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_command_spawn_joins_owned_spawn_exclusion() {
        use std::sync::mpsc;

        let missing = tempfile::tempdir().expect("scratch command folder");
        let program = missing.path().join("missing-program");
        let (held, holding) = mpsc::sync_channel(0);
        let (release, released) = mpsc::sync_channel(0);
        let first = std::thread::spawn(move || {
            let _guard = inheritable_marked();
            held.send(()).expect("report owned spawn setup");
            released.recv().expect("release owned spawn setup");
        });
        holding
            .recv_timeout(Duration::from_secs(5))
            .expect("owned spawn setup entered");
        let (attempt, attempting) = mpsc::sync_channel(0);
        let (entered, entering) = mpsc::sync_channel(0);
        let second = std::thread::spawn(move || {
            attempt.send(()).expect("report standard command attempt");
            let error = spawn_command(&mut std::process::Command::new(program))
                .expect_err("scratch command is absent");
            entered.send(error.kind()).expect("report command result");
        });
        attempting
            .recv_timeout(Duration::from_secs(5))
            .expect("standard command attempted");
        let early = entering.recv_timeout(Duration::from_millis(100)).ok();
        release.send(()).expect("release owned spawn setup");
        first.join().expect("owned spawn thread");
        let result = early.unwrap_or_else(|| {
            entering
                .recv_timeout(Duration::from_secs(5))
                .expect("standard command proceeds after owned setup")
        });
        second.join().expect("standard command thread");
        assert_eq!(result, io::ErrorKind::NotFound);
        assert!(
            early.is_none(),
            "standard command bypassed owned spawn setup"
        );
    }

    #[test]
    fn child_spawn_setup_excludes_other_child_spawns() {
        use std::sync::mpsc;

        let (held, holding) = mpsc::sync_channel(0);
        let (release, released) = mpsc::sync_channel(0);
        let first = std::thread::spawn(move || {
            let _guard = inheritable_marked();
            held.send(()).expect("report first spawn setup");
            released.recv().expect("release first spawn setup");
        });
        holding
            .recv_timeout(Duration::from_secs(5))
            .expect("first spawn setup entered");
        let (attempt, attempting) = mpsc::sync_channel(0);
        let (entered, entering) = mpsc::sync_channel(0);
        let second = std::thread::spawn(move || {
            attempt.send(()).expect("report second spawn attempt");
            let _guard = inheritable_marked();
            entered.send(()).expect("report second spawn setup");
        });
        attempting
            .recv_timeout(Duration::from_secs(5))
            .expect("second spawn attempted");
        let overlapped = entering.recv_timeout(Duration::from_millis(100)).is_ok();
        release.send(()).expect("release first spawn setup");
        first.join().expect("first spawn thread");
        if !overlapped {
            entering
                .recv_timeout(Duration::from_secs(5))
                .expect("second spawn enters after first finishes");
        }
        second.join().expect("second spawn thread");
        assert!(
            !overlapped,
            "child spawn setup must not overlap another fork"
        );
    }

    #[test]
    fn launch_spec_debug_shows_variable_names_but_never_values() {
        let launch = LaunchSpec {
            label: "claude-agent-acp".into(),
            program: PathBuf::from("/usr/local/bin/node"),
            args: vec!["entry.js".into()],
            cwd: ProcessCwd::Fixed(PathBuf::from("/data/adapters/x")),
            set_env: vec![("ANTHROPIC_API_KEY".into(), "sk-ant-secret-value".into())],
            daemons: Vec::new(),
            startup_timeout: Duration::from_secs(60),
        };
        let text = format!("{launch:?}");
        assert!(text.contains("ANTHROPIC_API_KEY"), "{text}");
        assert!(!text.contains("sk-ant-secret-value"), "{text}");
    }

    #[test]
    fn helper_spec_debug_shows_variable_names_but_never_values() {
        let resolved = ResolvedEnv {
            vars: [(
                crate::env::EnvKey::from("OPENAI_API_KEY"),
                OsString::from("sk-proj-resolved-value"),
            )]
            .into_iter()
            .collect(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        };
        let helper = HelperSpec {
            program: PathBuf::from("/usr/local/bin/npm"),
            args: vec!["ci".into()],
            cwd: PathBuf::from("/data/adapters/x"),
            env: HelperEnv::Resolved(Arc::new(resolved)),
            set: vec![("NPM_TOKEN".into(), "npm_secret-set-value".into())],
            new_session: false,
            timeout: Duration::from_secs(600),
            max_stdout: 65_536,
        };
        let text = format!("{helper:?}");
        for name in ["NPM_TOKEN", "OPENAI_API_KEY", "/usr/local/bin/npm", "ci"] {
            assert!(text.contains(name), "{name}: {text}");
        }
        for value in ["npm_secret-set-value", "sk-proj-resolved-value"] {
            assert!(!text.contains(value), "{value}: {text}");
        }
    }

    #[test]
    fn daemon_matches_serialize_for_the_registry() {
        let daemon = DaemonMatch {
            program: None,
            args_in_order: vec!["hermes_cli.main".into(), "gateway".into(), "run".into()],
        };
        assert_eq!(
            serde_json::to_value(&daemon).expect("serialize"),
            serde_json::json!({"program": null, "args_in_order": ["hermes_cli.main", "gateway", "run"]})
        );
    }

    #[test]
    fn windows_long_cwd_rejected() {
        let short = PathBuf::from(format!("C:\\{}", "a".repeat(WINDOWS_MAX_CWD - 3)));
        assert!(check_cwd_length(&short).is_ok());
        let long = PathBuf::from(format!("C:\\{}", "a".repeat(WINDOWS_MAX_CWD - 2)));
        let error = check_cwd_length(&long).expect_err("too long");
        assert!(matches!(error, SpawnError::InvalidCwd { .. }));
        assert!(
            error
                .to_string()
                .ends_with("it is longer than Windows allows for a program's working folder"),
            "{error}"
        );
        // UTF-16 units, not bytes: an emoji counts two, an accented letter one.
        let units = PathBuf::from(format!("C:\\{}", "é".repeat(WINDOWS_MAX_CWD - 3)));
        assert!(check_cwd_length(&units).is_ok());
        let emoji = PathBuf::from(format!("C:\\{}", "🚀".repeat(129)));
        assert!(check_cwd_length(&emoji).is_err());
    }

    #[test]
    fn windows_wrappers_per_agent() {
        use process_wrap::tokio::KillOnDrop;
        for new_session in [false, true] {
            let wrapped = wrap(tokio::process::Command::new("agent"), new_session);
            assert!(wrapped.has_wrap::<KillOnDrop>());
            #[cfg(windows)]
            {
                use process_wrap::tokio::{CreationFlags, JobObject};
                assert!(
                    wrapped.has_wrap::<JobObject>(),
                    "every agent and helper gets a job"
                );
                let flags = wrapped
                    .get_wrap::<CreationFlags>()
                    .expect("creation flags")
                    .0;
                assert_eq!(flags, windows::Win32::System::Threading::CREATE_NO_WINDOW);
            }
            #[cfg(unix)]
            {
                use process_wrap::tokio::{ProcessGroup, ProcessSession, ResetSigmask};
                assert!(wrapped.has_wrap::<ResetSigmask>());
                assert_eq!(wrapped.has_wrap::<ProcessGroup>(), !new_session);
                assert_eq!(wrapped.has_wrap::<ProcessSession>(), new_session);
            }
        }
    }

    /// Whichever comes second, the entry's write or its abandonment, leaves no entry behind.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_abandoned_registration_leaves_no_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry =
            LeftoverRegistry::open(&dir.path().join("run"), Handle::current()).expect("registry");
        let path = registry.path().expect("file").to_path_buf();
        let entries = || -> usize {
            let body: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
            body["entries"].as_array().map_or(0, Vec::len)
        };
        let entry = |pid| crate::registry::LeftoverEntry {
            pid,
            pgid: pid,
            sid: None,
            start: crate::ptable::StartTime::Unknown,
            label: "test".into(),
            spawned_at_ms: 0,
            daemons: Vec::new(),
            escaped: Vec::new(),
        };
        let written_first = Registration::new(registry.handle(), 11);
        written_first.register(entry(11));
        assert_eq!(entries(), 1);
        written_first.abandon();
        assert_eq!(entries(), 0, "abandoned after the write: removed");
        let abandoned_first = Registration::new(registry.handle(), 12);
        abandoned_first.abandon();
        abandoned_first.register(entry(12));
        assert_eq!(entries(), 0, "abandoned before the write: never written");
        registry.close().expect("close");
    }

    #[test]
    fn spawn_errors_read_as_sentences() {
        let missing = SpawnError::InvalidCwd {
            path: PathBuf::from("/gone"),
            reason: "it does not exist".into(),
        };
        assert_eq!(
            missing.to_string(),
            "the folder /gone can't be used: it does not exist"
        );
        assert_eq!(
            SpawnError::CwdNotAllowed {
                path: PathBuf::from("/Users/me/Desktop/x")
            }
            .to_string(),
            "Ghost isn't allowed to open /Users/me/Desktop/x"
        );
    }
}
