//! ghost-cli: Ghost's headless dev tool (M1a spec 9.5).
//!
//! It runs agents the way the app does, through a headless [`SessionManager`] on a temporary data
//! root, with a fresh temporary git repository as the chat folder, and records what they do:
//!
//! - `agents`: the env agents start with, the names the scrub removes, and each agent's
//!   availability ([`agents`]).
//! - `probe <agent>`: the free probe steps of spec 10.2 ([`probe`]).
//! - `smoke <agent> --scenario <names> --spend`: the paid scenarios of spec 10.3 ([`smoke`]).
//! - `fixture <log>`: a protocol log into a replay fixture ([`fixture`]).
//! - `replay <fixture>`: a fixture through the real ACP driver, against its golden ([`replay`]).
//!
//! Its agent processes register in the real data root's `run/` (spec 3.5), so the app's next
//! sweep cleans up after a crashed run, and it shares the app's `adapters/`. The binary only calls
//! [`main_with`]; tests drive the same functions with a fixed env that finds no real agent.
//!
//! [`SessionManager`]: ghost_core::session::SessionManager

pub mod agents;
pub mod args;
pub mod chat;
pub mod fixture;
pub mod headless;
pub mod logs;
pub mod out;
pub mod probe;
pub mod ps;
pub mod replay;
pub mod repo;
pub mod report;
pub mod smoke;
pub mod watch;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use ghost_core::CoreError;
use ghost_core::session::SessionError;

use crate::args::{Command, Invocation, UsageError};
use crate::fixture::CurateContext;
use crate::headless::{EnvChoice, Headless, HeadlessConfig, Roots};
use crate::out::Out;
use crate::repo::{RepoKind, RunFolder};
use crate::report::Outcome;

/// A check or the command failed.
pub const EXIT_FAILED: u8 = 1;
/// The command line could not be used.
pub const EXIT_USAGE: u8 = 2;
/// A signal stopped the command (128 plus SIGINT's number, as shells report it).
pub const EXIT_INTERRUPTED: u8 = 130;

/// Why a command could not run. The message is shown as it is.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("could not {what}: {source}")]
    Io {
        what: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Core(#[from] CoreError),
    #[error("{0}")]
    Session(#[from] SessionError),
    #[error("{0}")]
    Message(String),
}

impl CliError {
    pub fn io(what: impl Into<String>, source: std::io::Error) -> CliError {
        CliError::Io {
            what: what.into(),
            source,
        }
    }
}

/// The error for an agent id the detector does not know, listing the ones it does. The CLI
/// probes and smoke-tests each connection on its own, so it lists connection ids (`claude-acp`).
pub(crate) fn unknown_agent(headless: &Headless, id: &str) -> CliError {
    let known: Vec<&str> = ghost_agents::catalog()
        .iter()
        .map(|spec| spec.id)
        .filter(|id| headless.detector.spec(id).is_some())
        .collect();
    CliError::Message(format!(
        "Ghost doesn't know an agent called \"{id}\"; the agents are {}",
        known.join(", ")
    ))
}

/// Runs one command line (without the program name) and returns the process's exit status.
pub fn main_with(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    init_tracing();
    let invocation = match args::parse(args) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("ghost-cli: {error}");
            if matches!(error, UsageError::Invalid(_)) {
                eprintln!("Run ghost-cli help for the commands and options.");
            }
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let out = Out::stdout();
    let result = match &invocation.command {
        Command::Help => {
            out.line(args::USAGE.trim_end());
            return ExitCode::SUCCESS;
        }
        Command::Fixture(fixture_args) => paused_runtime()
            .and_then(|rt| rt.block_on(fixture::run(fixture_args, &CurateContext::here(), &out))),
        Command::Replay(replay_args) => {
            paused_runtime().and_then(|rt| rt.block_on(replay::run(replay_args, &out)))
        }
        Command::Agents
        | Command::AgentAuth(_)
        | Command::AgentUpdate(_)
        | Command::Probe(_)
        | Command::Smoke(_) => {
            runtime().and_then(|rt| rt.block_on(run_headless(&invocation, &out)))
        }
    };
    match result {
        Ok(Outcome::Passed) => ExitCode::SUCCESS,
        Ok(Outcome::Failed) => ExitCode::from(EXIT_FAILED),
        Ok(Outcome::Interrupted) => ExitCode::from(EXIT_INTERRUPTED),
        Err(error) => {
            eprintln!("ghost-cli: {error}");
            ExitCode::from(EXIT_FAILED)
        }
    }
}

/// Logs go to stderr, filtered by `RUST_LOG` (default: warnings only), so stdout holds the
/// command's own lines.
fn init_tracing() {
    use std::io::IsTerminal as _;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .try_init();
}

/// The runtime of the commands that run agents.
fn runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::io("start the async runtime", error))
}

/// A replay's runtime: one thread, paused time, so a fixture's waits take no time.
fn paused_runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .map_err(|error| CliError::io("start the async runtime", error))
}

/// `agents`, `probe` and `smoke`: the run folder, the headless runtime, the chat folder, the
/// command, and the end, with signals handled throughout.
async fn run_headless(invocation: &Invocation, out: &Out) -> Result<Outcome, CliError> {
    // First: from here on a signal no longer ends the process at once, so a Ctrl+C during startup
    // waits for startup (which is bounded), then stops the agents like any other.
    let mut signals = Signals::new()?;
    let (label, keep) = match &invocation.command {
        Command::Probe(probe) => (format!("probe-{}", probe.agent), true),
        Command::Smoke(smoke) => (format!("smoke-{}", smoke.agent), true),
        _ => ("agents".to_owned(), false),
    };
    let run_folder = RunFolder::create(&label)?;
    let roots = match &invocation.data_dir {
        Some(dir) => {
            let dir = headless::absolute(dir)?;
            Roots {
                machine: dir.clone(),
                profile: dir,
            }
        }
        None => Roots {
            machine: headless::real_data_root()?,
            profile: run_folder.path().join("data"),
        },
    };
    let set_env = match &invocation.command {
        Command::Probe(probe) => probe
            .env
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect(),
        _ => Vec::new(),
    };
    let started = Headless::start(HeadlessConfig {
        roots,
        env: EnvChoice::LoginShell,
        set_env,
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        checkpoints: true,
    })
    .await;
    let headless = match started {
        Ok(headless) => headless,
        Err(CliError::Core(error @ CoreError::ProfileInUse { .. })) => {
            return Err(CliError::Message(format!(
                "{error}\naura-cli refuses to share a profile; run it without --data-dir to use a temporary one."
            )));
        }
        Err(error) => return Err(error),
    };
    if let Some(notice) = headless::sweep_notice(&headless.sweep) {
        out.line(notice);
    }
    let logs = headless.profile_dir.join("logs").join("protocol");
    let outcome = tokio::select! {
        result = command(&headless, invocation, run_folder.path(), out) => Some(result),
        () = signals.next() => {
            out.line("ghost-cli: stopping the agents; a second Ctrl+C kills them at once");
            None
        }
    };
    tokio::select! {
        ended = headless.shutdown() => {
            if ended.sessions.is_some_and(|summary| summary.forced > 0) {
                out.line("Some agents did not stop in time and were killed.");
            }
        }
        () = signals.next() => headless.kill_now(),
    }
    drop(headless);
    if keep {
        let kept = run_folder.keep();
        out.line(format!(
            "This run's protocol logs are in {}, its chat folder in {}.",
            logs.display(),
            kept.join("repo").display()
        ));
        out.line(format!(
            "Delete {} when you are done with them{}.",
            kept.display(),
            if invocation.data_dir.is_some() {
                " (the logs stay in the --data-dir profile)"
            } else {
                ""
            }
        ));
    }
    outcome.unwrap_or(Ok(Outcome::Interrupted))
}

/// One headless command on a running runtime.
async fn command(
    headless: &Headless,
    invocation: &Invocation,
    run_folder: &std::path::Path,
    out: &Out,
) -> Result<Outcome, CliError> {
    match &invocation.command {
        Command::Agents => agents::run(headless, out).await,
        Command::AgentAuth(id) => {
            if headless.detector.spec(id).is_none() {
                return Err(unknown_agent(headless, id));
            }
            let status = headless.detector.auth_status(id).await;
            let (message, outcome) = match status {
                ghost_protocol::agents::AgentAuthStatus::CredentialsPresent => ("The vendor reports local credentials present; request authorization has not been checked.".to_owned(), Outcome::Passed),
                ghost_protocol::agents::AgentAuthStatus::SignInRequired => ("The vendor reports sign-in is required.".to_owned(), Outcome::Failed),
                ghost_protocol::agents::AgentAuthStatus::Unknown { reason } => (reason, Outcome::Failed),
                ghost_protocol::agents::AgentAuthStatus::Unsupported => ("This agent has no supported standalone authentication check.".to_owned(), Outcome::Failed),
            };
            out.line(message);
            Ok(outcome)
        }
        Command::AgentUpdate(id) => {
            if headless.detector.spec(id).is_none() {
                return Err(unknown_agent(headless, id));
            }
            headless
                .detector
                .update(id)
                .await
                .map_err(|error| CliError::Message(error.to_string()))?;
            out.line("The vendor update completed. Agent detection has been refreshed.");
            Ok(Outcome::Passed)
        }
        Command::Probe(args) => {
            let (folder, hostile) = match &args.folder {
                Some(folder) => (headless::absolute(folder)?, false),
                None => (
                    repo::create(
                        &run_folder.join("repo"),
                        RepoKind::Hostile,
                        &headless.registry,
                    )
                    .await?,
                    true,
                ),
            };
            probe::run(
                headless,
                args,
                &folder,
                hostile,
                probe::ProbeTimings::STANDARD,
                out,
            )
            .await
        }
        Command::Smoke(args) => {
            let folder: PathBuf = repo::create(
                &run_folder.join("repo"),
                RepoKind::Plain,
                &headless.registry,
            )
            .await?;
            smoke::run(headless, args, &folder, smoke::SmokeTimings::STANDARD, out).await
        }
        Command::Help | Command::Fixture(_) | Command::Replay(_) => Ok(Outcome::Passed),
    }
}

/// SIGINT, SIGTERM and SIGHUP (Unix) or Ctrl+C (Windows), each delivered once per [`next`].
///
/// [`next`]: Signals::next
struct Signals {
    #[cfg(unix)]
    streams: [tokio::signal::unix::Signal; 3],
}

impl Signals {
    fn new() -> Result<Signals, CliError> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let install = |kind: SignalKind| {
                signal(kind).map_err(|error| CliError::io("listen for signals", error))
            };
            Ok(Signals {
                streams: [
                    install(SignalKind::interrupt())?,
                    install(SignalKind::terminate())?,
                    install(SignalKind::hangup())?,
                ],
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Signals {})
        }
    }

    async fn next(&mut self) {
        #[cfg(unix)]
        {
            let [interrupt, terminate, hangup] = &mut self.streams;
            tokio::select! {
                Some(()) = interrupt.recv() => {}
                Some(()) = terminate.recv() => {}
                Some(()) = hangup.recv() => {}
                else => std::future::pending::<()>().await,
            }
        }
        #[cfg(not(unix))]
        {
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}
