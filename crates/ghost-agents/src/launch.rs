//! Launch specs: how one start of an agent runs.
//!
//! A `Binary` or `NativeBinary` agent runs its program with its args from the chat folder. An
//! `NpmAdapter` runs as `node <absolute entry>` from its own install folder, never the chat folder
//! (which reaches the adapter only as the ACP `cwd`), with the user's binary forced through the
//! adapter's variable. A `Builtin` agent has no process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ghost_process::{LaunchSpec, ProcessCwd};

use crate::catalog::{AgentSpec, UserBinary};

/// Why an agent cannot start. Every reason is a sentence the error card shows.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchError {
    #[error("{reason}")]
    Missing { reason: String },
    #[error("{reason}")]
    Broken { reason: String },
    #[error("{reason}")]
    InstallFailed { reason: String },
}

/// The launch spec of a built-in agent: it runs inside Ghost, so it names no program and is never
/// spawned.
pub(crate) fn builtin(spec: &AgentSpec) -> LaunchSpec {
    LaunchSpec {
        label: spec.id.to_owned(),
        program: PathBuf::new(),
        args: Vec::new(),
        cwd: ProcessCwd::ChatFolder,
        set_env: Vec::new(),
        daemons: spec.daemons.clone(),
        startup_timeout: spec.startup_timeout,
    }
}

/// A binary on the user's `PATH`, run from the chat folder: `hermes acp`.
pub(crate) fn binary(spec: &AgentSpec, program: &Path, name: &str, args: &[&str]) -> LaunchSpec {
    LaunchSpec {
        label: std::iter::once(name)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" "),
        program: program.to_path_buf(),
        args: args.iter().map(OsString::from).collect(),
        cwd: ProcessCwd::ChatFolder,
        set_env: Vec::new(),
        daemons: spec.daemons.clone(),
        startup_timeout: spec.startup_timeout,
    }
}

/// An installed npm adapter: `node <entry>` in its own folder, the user's binary always set.
pub(crate) fn adapter(
    spec: &AgentSpec,
    bin: &str,
    node: &Path,
    entry: &Path,
    folder: &Path,
    user_binary: &UserBinary,
    user_binary_path: &Path,
) -> LaunchSpec {
    LaunchSpec {
        label: bin.to_owned(),
        program: node.to_path_buf(),
        args: vec![entry.as_os_str().to_os_string()],
        cwd: ProcessCwd::Fixed(folder.to_path_buf()),
        set_env: vec![(
            OsString::from(user_binary.env),
            user_binary_path.as_os_str().to_os_string(),
        )],
        daemons: spec.daemons.clone(),
        startup_timeout: spec.startup_timeout,
    }
}
