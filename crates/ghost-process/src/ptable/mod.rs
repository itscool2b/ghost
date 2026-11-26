//! The process table: snapshots, start times, descendants, and daemon matching.
//!
//! Unix only for anything that reads the system (Windows needs no process table in M1a: the job
//! tracks the tree and no sweep runs there). The types and the pure functions (matching and the
//! parsers, tested on captured text) build on every OS, so the leftover registry's records do too.
//!
//! macOS reads `ps -axo pid=,ppid=,pgid=,lstart=,command=` with `LC_ALL=C TZ=UTC0` (`lstart` is five
//! fixed tokens, one-second resolution) and session ids through `getsid` (macOS `ps -o sess` prints
//! 0); Linux reads `/proc/<pid>/stat` (start time in clock ticks since boot, field 22) with the boot
//! id. Other Unix systems take the `ps` path.

mod linux;
mod macos;

#[cfg(unix)]
use std::collections::HashSet;
#[cfg(any(unix, test))]
use std::io;

use serde::{Deserialize, Serialize};

use crate::spawn::DaemonMatch;

/// One process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub sid: Option<u32>,
    pub start: StartTime,
    /// macOS: `ps -o command=` split on spaces; Linux: `/proc/<pid>/cmdline` split on NUL.
    pub argv: Vec<String>,
}

/// When a process started, in a form that tells a recycled pid from the original.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartTime {
    /// Clock ticks since boot (`/proc/<pid>/stat` field 22), with the boot it counts from.
    LinuxTicks {
        ticks: u64,
        boot_id: String,
    },
    /// `ps -o lstart=` as printed with `LC_ALL=C TZ=UTC0`, its words joined by single spaces.
    MacLstart(String),
    Unknown,
}

/// Every process, read once. Zombies are left out: they have exited already.
#[cfg(unix)]
pub fn snapshot() -> io::Result<Vec<ProcInfo>> {
    #[cfg(target_os = "linux")]
    {
        linux::snapshot()
    }
    #[cfg(not(target_os = "linux"))]
    {
        macos::snapshot()
    }
}

/// One process's start time. A pid that is not running is `NotFound`.
#[cfg(unix)]
pub fn start_time(pid: u32) -> io::Result<StartTime> {
    #[cfg(target_os = "linux")]
    {
        linux::start_time(pid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        macos::start_time(pid)
    }
}

/// Every process whose parent chain in `table` leads to `root`, `root` excluded, nearest first.
pub fn descendants(table: &[ProcInfo], root: u32) -> Vec<ProcInfo> {
    let mut found: Vec<ProcInfo> = Vec::new();
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::from([root]);
    let mut frontier = vec![root];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for parent in frontier {
            for info in table.iter().filter(|info| info.ppid == parent) {
                if seen.insert(info.pid) {
                    found.push(info.clone());
                    next.push(info.pid);
                }
            }
        }
        frontier = next;
    }
    found
}

/// Whether `info` is one of `daemons`: the file name of `argv[0]` equals `program` when set, and
/// the words of `args_in_order` appear in argv in that order, not necessarily adjacent. Pure.
pub fn matches_daemon(info: &ProcInfo, daemons: &[DaemonMatch]) -> bool {
    daemons.iter().any(|daemon| {
        let program_ok = daemon.program.as_deref().is_none_or(|program| {
            info.argv.first().is_some_and(|argv0| {
                std::path::Path::new(argv0)
                    .file_name()
                    .is_some_and(|name| name == program)
            })
        });
        let mut words = info.argv.iter();
        let args_ok = daemon
            .args_in_order
            .iter()
            .all(|wanted| words.any(|word| word == wanted));
        program_ok && args_ok
    })
}

/// What the kill rule spares in `table`: every declared daemon that left the agent's group
/// `group`, and every process below it outside that group. A daemon still in the group is only a
/// member: it dies with the group, and its subtree is not spared.
#[cfg(any(unix, test))]
pub(crate) fn spared_daemons(
    table: &[ProcInfo],
    daemons: &[DaemonMatch],
    group: u32,
) -> std::collections::HashSet<u32> {
    let mut spared = std::collections::HashSet::new();
    if daemons.is_empty() {
        return spared;
    }
    for info in table
        .iter()
        .filter(|info| info.pgid != group && matches_daemon(info, daemons))
    {
        spared.insert(info.pid);
        spared.extend(
            descendants(table, info.pid)
                .iter()
                .filter(|below| below.pgid != group)
                .map(|below| below.pid),
        );
    }
    spared
}

/// What identifies this boot: Linux's boot id, macOS's `kern.boottime`.
#[cfg(unix)]
pub fn boot_marker() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        linux::boot_id()
    }
    #[cfg(not(target_os = "linux"))]
    {
        macos::boot_marker()
    }
}

/// Parses `ps -axo pid=,ppid=,pgid=,lstart=,command=` output. Pure, tested with captured output.
pub fn parse_ps(text: &str) -> Vec<ProcInfo> {
    macos::parse_ps(text)
}

/// Parses one process's `/proc/<pid>/stat` and `cmdline`. Pure.
pub fn parse_proc_stat(pid: u32, stat: &str, cmdline: &[u8], boot_id: &str) -> Option<ProcInfo> {
    linux::parse_proc_stat(pid, stat, cmdline, boot_id)
}

/// The start time of `pid` if it is running (not a zombie), for identity checks.
#[cfg(unix)]
pub(crate) fn running_start(pid: u32) -> Option<StartTime> {
    #[cfg(target_os = "linux")]
    {
        linux::running_start(pid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        macos::running_start(pid)
    }
}

/// Which of `processes` still run as themselves (pid and start time), from one read of the table.
#[cfg(unix)]
pub(crate) fn still_running<'a, I>(processes: I) -> Vec<(u32, StartTime)>
where
    I: IntoIterator<Item = (u32, &'a StartTime)>,
{
    let wanted: Vec<(u32, &StartTime)> = processes
        .into_iter()
        .filter(|(_, start)| **start != StartTime::Unknown)
        .collect();
    #[cfg(target_os = "linux")]
    {
        wanted
            .into_iter()
            .filter(|(pid, start)| linux::running_start(*pid).is_some_and(|now| now == **start))
            .map(|(pid, start)| (pid, start.clone()))
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let pids: Vec<u32> = wanted.iter().map(|(pid, _)| *pid).collect();
        let now = macos::running_starts(&pids);
        wanted
            .into_iter()
            .filter(|(pid, start)| now.get(pid).is_some_and(|found| found == *start))
            .map(|(pid, start)| (pid, start.clone()))
            .collect()
    }
}

/// Final confirmation retains unreadable identities, which never authorize a signal.
#[cfg(unix)]
pub(crate) fn not_confirmed_gone<'a>(
    processes: impl IntoIterator<Item = (u32, &'a StartTime)>,
) -> Vec<(u32, StartTime)> {
    retain_unconfirmed(processes, |pid| {
        #[cfg(target_os = "linux")]
        {
            linux::checked_running_start(pid)
        }
        #[cfg(not(target_os = "linux"))]
        {
            macos::checked_running_start(pid)
        }
    })
}

#[cfg(any(unix, test))]
fn retain_unconfirmed<'a>(
    processes: impl IntoIterator<Item = (u32, &'a StartTime)>,
    mut observe: impl FnMut(u32) -> io::Result<Option<StartTime>>,
) -> Vec<(u32, StartTime)> {
    processes
        .into_iter()
        .filter_map(|(pid, expected)| {
            let retain = match observe(pid) {
                Ok(None) => false,
                Ok(Some(found)) => *expected == StartTime::Unknown || found == *expected,
                Err(_) => true,
            };
            retain.then(|| (pid, expected.clone()))
        })
        .collect()
}

/// A missing row is conclusive only when the kernel also reports no such pid.
#[cfg(unix)]
pub(super) fn confirm_absent(pid: u32) -> io::Result<Option<StartTime>> {
    let raw = i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid process id"))?;
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(raw), None) {
        Err(nix::errno::Errno::ESRCH) => Ok(None),
        _ => Err(io::Error::other("process identity could not be confirmed")),
    }
}

/// An unreadable process table is an error, never an empty group.
#[cfg(unix)]
pub(crate) fn checked_group_members(pgid: u32) -> io::Result<Vec<u32>> {
    #[cfg(target_os = "linux")]
    {
        linux::checked_group_members(pgid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        macos::checked_group_members(pgid)
    }
}

/// Whether a process that is not a zombie is in group `pgid` (Linux, from `/proc`; elsewhere a
/// group probe answers this).
#[cfg(unix)]
pub(crate) fn group_has_live_member(pgid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::checked_group_members(pgid).map_or(true, |members| !members.is_empty())
    }
    #[cfg(not(target_os = "linux"))]
    {
        macos::checked_group_members(pgid).map_or(true, |members| !members.is_empty())
    }
}

/// The processes needed to apply the kill rule to an agent: the descendants of `root` and the
/// members of group `pgid`, with full detail. Cheaper than [`snapshot`] on macOS, where reading
/// every command line is slow.
#[cfg(unix)]
pub(crate) fn tree_of(root: u32, pgid: u32) -> io::Result<Vec<ProcInfo>> {
    #[cfg(target_os = "linux")]
    {
        let table = linux::snapshot()?;
        let keep = wanted(&table, root, pgid);
        Ok(table
            .into_iter()
            .filter(|info| keep.contains(&info.pid))
            .collect())
    }
    #[cfg(not(target_os = "linux"))]
    {
        macos::tree_of(root, pgid)
    }
}

/// The pids of `root`, its descendants and the members of `pgid`, in `table`.
#[cfg(unix)]
fn wanted(table: &[ProcInfo], root: u32, pgid: u32) -> HashSet<u32> {
    let mut keep: HashSet<u32> = descendants(table, root)
        .into_iter()
        .map(|info| info.pid)
        .collect();
    keep.insert(root);
    keep.extend(
        table
            .iter()
            .filter(|info| info.pgid == pgid)
            .map(|info| info.pid),
    );
    keep
}

/// Seconds since the Unix epoch of a macOS `lstart` (UTC). Pure.
#[cfg(test)]
pub(crate) fn lstart_epoch(lstart: &str) -> Option<i64> {
    macos::lstart_epoch(lstart)
}

/// Seconds since the Unix epoch when a process started, when its start time can say (a Linux start
/// time only counts in the boot it was read in).
#[cfg(unix)]
pub(crate) fn start_epoch(start: &StartTime) -> Option<i64> {
    match start {
        StartTime::MacLstart(lstart) => macos::lstart_epoch(lstart),
        StartTime::LinuxTicks { ticks, boot_id } => linux::ticks_epoch(*ticks, boot_id),
        StartTime::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_identity_reads_remain_recoverable_but_recycled_pids_do_not() {
        let known = StartTime::MacLstart("old".into());
        let retained = retain_unconfirmed(
            [(1, &known), (2, &known), (3, &known), (4, &known)],
            |pid| match pid {
                1 => Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unreadable",
                )),
                2 => Ok(None),
                3 => Ok(Some(StartTime::MacLstart("replacement".into()))),
                _ => Ok(Some(known.clone())),
            },
        );
        assert_eq!(retained, [(1, known.clone()), (4, known)]);
    }

    fn proc(pid: u32, ppid: u32, pgid: u32, argv: &[&str]) -> ProcInfo {
        ProcInfo {
            pid,
            ppid,
            pgid,
            sid: Some(1),
            start: StartTime::Unknown,
            argv: argv.iter().map(|word| (*word).to_owned()).collect(),
        }
    }

    #[test]
    fn start_times_serialize_for_the_registry() {
        let linux = StartTime::LinuxTicks {
            ticks: 123,
            boot_id: "b".into(),
        };
        assert_eq!(
            serde_json::to_value(&linux).expect("serialize"),
            serde_json::json!({"linux_ticks": {"ticks": 123, "boot_id": "b"}})
        );
        for start in [
            linux,
            StartTime::MacLstart("Sat Oct 24 09:03:00 2026".into()),
            StartTime::Unknown,
        ] {
            let text = serde_json::to_string(&start).expect("serialize");
            assert_eq!(
                serde_json::from_str::<StartTime>(&text).expect("deserialize"),
                start
            );
        }
    }

    #[test]
    fn descendants_follow_parent_links_through_generations() {
        let table = [
            proc(1, 0, 1, &["/sbin/launchd"]),
            proc(200, 1, 200, &["agent"]),
            proc(201, 200, 200, &["/bin/sh", "-c", "hook"]),
            proc(202, 201, 200, &["node", "mcp.js"]),
            proc(300, 200, 300, &["daemon"]),
            proc(301, 300, 300, &["helper"]),
            proc(400, 1, 400, &["unrelated"]),
        ];
        let pids: Vec<u32> = descendants(&table, 200)
            .iter()
            .map(|info| info.pid)
            .collect();
        assert_eq!(pids, [201, 300, 202, 301], "nearest first");
        assert!(descendants(&table, 202).is_empty());
        assert!(descendants(&table, 999).is_empty());
        // A cycle in a racy snapshot never loops.
        let cycle = [proc(10, 11, 10, &[]), proc(11, 10, 10, &[])];
        assert_eq!(descendants(&cycle, 10).len(), 1);
    }

    #[test]
    fn daemons_match_by_program_and_words_in_order() {
        let gateway = proc(
            5120,
            5001,
            5120,
            &[
                "/Users/me/.hermes/venv/bin/python3",
                "-m",
                "hermes_cli.main",
                "--profile",
                "default",
                "gateway",
                "run",
                "--quiet",
            ],
        );
        let hermes = DaemonMatch {
            program: None,
            args_in_order: vec!["hermes_cli.main".into(), "gateway".into(), "run".into()],
        };
        assert!(matches_daemon(&gateway, std::slice::from_ref(&hermes)));
        let claude_daemon = proc(
            6000,
            5001,
            6000,
            &["claude", "daemon", "run", "--origin", "transient"],
        );
        let claude = DaemonMatch {
            program: Some("claude".into()),
            args_in_order: vec!["daemon".into(), "run".into()],
        };
        assert!(matches_daemon(
            &claude_daemon,
            &[hermes.clone(), claude.clone()]
        ));
        let by_path = proc(
            6001,
            1,
            6001,
            &["/Users/me/.local/bin/claude", "daemon", "run"],
        );
        assert!(matches_daemon(&by_path, std::slice::from_ref(&claude)));
        for other in [
            proc(
                1,
                0,
                1,
                &["python3", "-m", "hermes_cli.main", "run", "gateway"],
            ),
            proc(1, 0, 1, &["python3", "-m", "hermes_cli.main", "chat"]),
            proc(1, 0, 1, &["claude", "run", "daemon"]),
            proc(1, 0, 1, &["not-claude", "daemon", "run"]),
            proc(1, 0, 1, &[]),
        ] {
            assert!(
                !matches_daemon(&other, &[hermes.clone(), claude.clone()]),
                "{other:?}"
            );
        }
        assert!(!matches_daemon(&gateway, &[]));
    }
}
