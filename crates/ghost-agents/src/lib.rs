//! The agents Ghost can start, and what it takes to start each one.
//!
//! - [`catalog`](mod@catalog): the data. Each [`AgentSpec`] names its driver, its dialect, how to find and run it
//!   ([`Recipe`]), its start timeout, the vendor daemons the kill rule spares, and the version last
//!   verified with Ghost.
//! - [`Detector`]: whether each agent can start here ([`Availability`]), probed with the resolved,
//!   scrubbed env and cached; and the launch spec of one start, which installs an npm adapter the
//!   first time ([`Detector::prepare_launch`]). Other crates' tests use [`Detector::fixed`], whose
//!   [`FixedAgent`]s answer as given.
//!
//! The catalog lists connections; people see one entry per agent. Claude Code and Codex each have
//! Ghost's own driver and an npm ACP adapter, Antigravity and Grok Build an ACP server and a
//! restricted text fallback, and OpenCode its 1.x and 2.x lines: [`Detector::cached`] shows each
//! as one agent, and [`Detector::choose_connection`] picks the connection of a new session
//! (Automatic, or the agent's Connection setting on this device).
//! Detection never installs or runs an adapter. Installation has its own timeout and the user's
//! consent (creating the chat after the download note).
//!
//! [`Availability`]: ghost_protocol::session::Availability

mod candidates;
pub mod catalog;
pub mod config_vendor;
mod connection_choices;
mod cursor_entry;
mod detect;
pub mod harness;
pub mod hosts;
mod install;
pub mod installation;
pub mod installer;
mod launch;
pub mod maintenance;
mod npm_entry;
pub mod plugins;
mod preferences;
mod probe;
pub mod releases;
mod shared;

use std::time::Duration;

pub use catalog::{
    AgentSpec, Recipe, UserBinary, agent_of, catalog, get, has_terminal, parse_version,
    version_at_least,
};
pub use detect::{Detector, FixedAgent, Gate};
pub use launch::LaunchError;

/// A detection probe (`<program> --version`) is killed with its tree after this.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Probes keep at most this much output.
pub const PROBE_MAX_STDOUT: usize = 64 * 1024;

/// The one-time adapter install (`npm ci`) may take this long.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The installed adapter's `--version` check, which loads every module, may take this long.
pub const INSTALL_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
