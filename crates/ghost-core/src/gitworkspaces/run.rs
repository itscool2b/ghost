//! A worktree's ports and its Run command.
//!
//! Each managed worktree gets ten consecutive local ports, kept in its registry entry from the
//! first time one is needed, so several worktrees of one project can serve at once without
//! choosing ports by hand. Its setup command, Run command and project terminals get the first as
//! `GHOST_PORT`; Run, which usually starts a dev server, also gets it as `PORT`. A block is
//! taken only where no other live worktree of the profile holds it and every one of its ports can
//! be bound on this machine right now (Windows reserves some ranges for Hyper-V). Run is the
//! folder recipe's `run` command, started like the setup command: the same executable rules and
//! environment, in the worktree itself.
use std::collections::HashSet;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ghost_protocol::git::{ManagedWorktree, WorktreeState};
use ghost_protocol::worktree_setup::SetupCommand;

use super::setup::{executable, invalid};
use super::{GitWorkspaces, Result};

/// How many ports a worktree gets: the first is `GHOST_PORT`, and the next nine are its own.
pub(crate) const PORT_SPAN: u16 = 10;
/// Blocks start between these: below the ephemeral ranges Windows, macOS and Linux hand out to
/// outgoing connections (from 32768 or 49152), and away from the usual dev servers' defaults.
const FIRST_BLOCK: u16 = 25_000;
const LAST_BLOCK: u16 = 31_990;
// Every block stays below the ephemeral ranges.
const _: () = assert!(LAST_BLOCK + PORT_SPAN - 1 < 32_768);

/// Whether every port of the block starting at `start` can be bound on this machine now.
fn bindable(start: u16) -> bool {
    (0..PORT_SPAN).all(|offset| TcpListener::bind((Ipv4Addr::LOCALHOST, start + offset)).is_ok())
}

/// The first block no other worktree holds and this machine can bind.
pub(super) fn free_block(taken: &HashSet<u16>, bindable: impl Fn(u16) -> bool) -> Option<u16> {
    (FIRST_BLOCK..=LAST_BLOCK)
        .step_by(usize::from(PORT_SPAN))
        .find(|start| !taken.contains(start) && bindable(*start))
}

/// A worktree still in use keeps its block; a retired one gives it back.
fn holds_ports(record: &ManagedWorktree) -> bool {
    record.state != WorktreeState::Retired
}

impl GitWorkspaces {
    /// The worktree `chat_id` works in, ready, and the first of its ports, given once and kept.
    /// A block another live worktree took meanwhile (a restored worktree's, say) is given anew.
    /// One short read-modify-write of the registry, so it never waits for a Git change.
    pub(super) fn port_under_gate(&self, chat_id: &str) -> Result<(ManagedWorktree, u16)> {
        self.update_records(|records| {
            let index = records
                .iter()
                .position(|record| {
                    record.chat_id == chat_id && record.state == WorktreeState::Ready
                })
                .ok_or_else(|| invalid("This chat has no ready worktree."))?;
            let id = records[index].id.clone();
            let taken: HashSet<u16> = records
                .iter()
                .filter(|record| record.id != id && holds_ports(record))
                .filter_map(|record| record.port)
                .collect();
            if let Some(port) = records[index].port.filter(|port| !taken.contains(port)) {
                return Ok((records[index].clone(), port));
            }
            let port = free_block(&taken, bindable).ok_or_else(|| {
                invalid(
                    "No block of ten free local ports is left. Retire a worktree you no longer use.",
                )
            })?;
            records[index].port = Some(port);
            Ok((records[index].clone(), port))
        })
    }

    /// [`Self::port_under_gate`], for the session.
    pub(crate) async fn worktree_port(&self, chat_id: &str) -> Result<(ManagedWorktree, u16)> {
        self.port_under_gate(chat_id)
    }

    /// The process Run starts: `command` in the chat's worktree, as the setup command runs, with
    /// the worktree's first port as `GHOST_PORT` and `PORT`.
    pub(crate) async fn run_command(
        &self,
        chat_id: &str,
        command: &SetupCommand,
    ) -> Result<ghost_process::SpawnSpec> {
        let (record, port) = self.worktree_port(chat_id).await?;
        let program = executable(self, &Some(command.clone()))
            .await?
            .ok_or_else(|| invalid("Save a Run command in this folder's setup first."))?
            .0;
        let mut env = (*self.env.get().await).clone();
        env.vars.retain(|key, _| {
            !key.as_os_str()
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
        });
        let folder = PathBuf::from(&record.path);
        Ok(ghost_process::SpawnSpec {
            launch: ghost_process::LaunchSpec {
                label: "Worktree run".into(),
                program,
                args: command.args.iter().map(Into::into).collect(),
                cwd: ghost_process::ProcessCwd::Fixed(crate::fsutil::plain_path(&folder)),
                set_env: port_env(port, true),
                daemons: vec![],
                startup_timeout: Duration::from_secs(15),
            },
            folder,
            env: Arc::new(env),
            max_line_bytes: ghost_process::MAX_LINE_BYTES,
        })
    }
}

/// `GHOST_PORT`, and `PORT` too for Run. `O7_PORT` and `AURACODE_PORT` too: Setup and Run
/// commands saved while the app had those names use them, as the app's own suggestion did then.
pub(crate) fn port_env(port: u16, run: bool) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let value = std::ffi::OsString::from(port.to_string());
    let mut env = vec![
        ("GHOST_PORT".into(), value.clone()),
        ("O7_PORT".into(), value.clone()),
        ("AURACODE_PORT".into(), value.clone()),
    ];
    if run {
        env.push(("PORT".into(), value));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_skip_taken_and_unbindable_ports_and_end_at_the_last() {
        let taken: HashSet<u16> = [25_000, 25_010].into();
        assert_eq!(free_block(&taken, |_| true), Some(25_020));
        // A block with a port the machine cannot bind (in use, or reserved) is passed over.
        assert_eq!(free_block(&taken, |start| start != 25_020), Some(25_030));
        assert_eq!(free_block(&HashSet::new(), |_| false), None);
        let all: HashSet<u16> = (FIRST_BLOCK..=LAST_BLOCK)
            .step_by(usize::from(PORT_SPAN))
            .collect();
        assert_eq!(all.len(), 700);
        assert_eq!(free_block(&all, |_| true), None);
    }

    #[test]
    fn run_gets_port_too_and_others_only_the_app_port_under_every_name() {
        let names = |run| {
            port_env(25_040, run)
                .into_iter()
                .map(|(key, value)| {
                    format!("{}={}", key.to_string_lossy(), value.to_string_lossy())
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(true),
            [
                "GHOST_PORT=25040",
                "O7_PORT=25040",
                "AURACODE_PORT=25040",
                "PORT=25040"
            ]
        );
        assert_eq!(
            names(false),
            ["GHOST_PORT=25040", "O7_PORT=25040", "AURACODE_PORT=25040"]
        );
    }
}
