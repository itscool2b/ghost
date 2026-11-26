//! Agent processes: everything between Ghost and an agent's operating-system process.
//!
//! Drivers, the environment resolver, detection probes and sessions all spawn, so this crate sits
//! below every one of them and knows no `GhostEvent`; it is tested against plain child processes.
//!
//! - [`spawn`]: [`LaunchSpec`] says what to run; a [`ProcessSpawner`] starts it with all three pipes
//!   and the complete, scrubbed environment ([`env`](mod@env)), in its own process group (Unix) or job object
//!   (Windows). [`run_helper`] runs short-lived helpers (the login shell, `--version` probes, the
//!   adapter install).
//! - [`io`]: stdin is a never-blocking [`LineSink`]; stdout a [`LineSource`] of bounded lines, so a
//!   slow reader stops the agent instead of growing memory.
//! - [`control`] and [`shutdown`]: a [`ProcessControl`] closes stdin, waits, signals, and kills the
//!   whole tree except the vendor daemons an agent declares; the last handle dropped kills the tree.
//! - [`registry`] and [`ptable`]: a per-instance record of running agents, so the next start can stop
//!   what a crashed Ghost left behind (Unix; Windows jobs die with their owner).
//! - [`log`]: one JSON Lines protocol log per agent process, redacted before anything reaches disk.
//! - `testing` (feature `testing`): in-memory processes for other crates' tests.
//!
//! The constants are the limits of appendix A of the M1a spec.

pub mod control;
pub mod env;
pub mod host;
pub mod io;
pub mod locked_file;
pub mod log;
pub mod ptable;
pub mod pty;
pub mod raw;
pub mod registry;
pub mod shutdown;
pub mod spawn;
pub mod supervisor;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tree;

mod framing;
mod sniff;
mod stderr;

use std::time::Duration;

pub use control::{ControlImpl, ExitInfo, ProcessControl};
pub use env::{
    DenyReason, EnvKey, EnvMap, EnvResolver, EnvResolverConfig, EnvSource, LAUNCH_SET_ENV,
    ResolvedEnv, ResolverStatus, ScrubReport, child_env, deny_reason, expand_env_strings, find_exe,
    find_program, merge_windows_path, node_options_has_debugger_hook, parse_env0, process_env,
    scrub, secret_values, strip_launch_context,
};
pub use host::{HostAvailability, HostExecution, HostSpawner};
pub use io::{AgentIo, LineSink, LineSource, StdinClosed, StdoutItem, TopKeys};
pub use log::{
    Direction, LogFactory, LogLimits, LogNote, ProtocolLog, Redactor, RetentionPolicy,
    RetentionReport,
};
pub use raw::{ByteSink, RawProcess};
pub use registry::{
    EscapedProc, InstanceRecord, LeftoverEntry, LeftoverRegistry, SweepReport, sweep_stale,
};
pub use shutdown::{ShutdownPlan, ShutdownReport, ShutdownStep};
pub use spawn::{
    DaemonMatch, HelperEnv, HelperOutput, HelperSpec, LaunchSpec, ProcessCwd, ProcessSpawner,
    SpawnError, SpawnSpec, SpawnedProcess, Spawner, run_helper, run_helper_input, spawn_command,
};

/// The longest stdout line an agent may write (Hermes' own limit is 50 MiB). A longer line is
/// skipped, and only its top-level `id`, `method` and `type` reach the driver
/// ([`StdoutItem::Oversized`]).
pub const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// How many stdout lines wait for the driver before the reader stops reading the pipe.
pub const STDOUT_CHANNEL_LINES: usize = 16;

/// How many bytes of stdout lines wait for the driver before the reader stops; one longer line
/// passes alone.
pub const STDOUT_CHANNEL_BYTES: usize = 8 * 1024 * 1024;

/// How long stdout is still read after the agent exited: a descendant may hold the pipe open.
pub const PIPE_EOF_GRACE: Duration = Duration::from_secs(2);

/// The ring of recent stderr kept for error cards ([`ControlImpl::stderr_tail`]).
pub const STDERR_TAIL_BYTES: usize = 16 * 1024;

/// How much of the stderr tail an error card shows.
pub const STDERR_TAIL_SHOWN_BYTES: usize = 2 * 1024;

/// stderr is split into log lines of at most this many bytes.
pub const STDERR_LINE_BYTES: usize = 4 * 1024;

/// stderr logged per second; the rest is counted, so a flood never fills the disk.
pub const STDERR_LOG_BYTES_PER_SEC: usize = 256 * 1024;

/// How often a leftover registry file with entries is rewritten: its modification time is its
/// instance's heartbeat.
pub const REGISTRY_HEARTBEAT: Duration = Duration::from_secs(60);

/// A dead instance's group members count as its own if they started before its last heartbeat
/// plus this much.
pub const SWEEP_WINDOW_SLACK: Duration = Duration::from_secs(60);

/// The startup sweep waits this long between SIGTERM and SIGKILL.
pub const SWEEP_SIGTERM_GRACE: Duration = Duration::from_secs(2);
