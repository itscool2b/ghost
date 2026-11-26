//! The process table around a run (Unix): what an agent left running once it stopped, the
//! `ps` diff of spec 9.5's acceptance. Windows has no process table in M1a; its job objects end
//! every agent's tree.
//!
//! Three lists, from a snapshot taken before the first agent started and one after it stopped:
//!
//! - `group`: processes still in an agent's process group (agents lead their own group). Always
//!   a leftover: the kill rule ends every member, a matching daemon included.
//! - `related`: processes that started during the run, whose command line names the agent's
//!   program, its adapter folder, the user's binary or the chat folder, and that are orphans (their
//!   parent is init, is gone, or is another leftover), except declared daemons. An agent's escaped
//!   descendants are orphans once it exited; a process the user's own tools start meanwhile has a
//!   living parent and is never counted. (On Linux a subreaper adopts orphans instead of init,
//!   and those are missed.)
//! - `spared`: processes matching the agent's declared daemons (Hermes' gateway), which the kill
//!   rule leaves running, whenever they started.

use ghost_process::DaemonMatch;
use ghost_process::ptable::{ProcInfo, StartTime, matches_daemon};

/// What the table shows after a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Leftovers {
    pub group: Vec<ProcInfo>,
    pub related: Vec<ProcInfo>,
    pub spared: Vec<ProcInfo>,
}

impl Leftovers {
    /// Whether nothing the run started is still running (spared daemons aside).
    pub fn is_clean(&self) -> bool {
        self.group.is_empty() && self.related.is_empty()
    }
}

/// Every process now; `None` on Windows or when the table can't be read.
pub fn snapshot() -> Option<Vec<ProcInfo>> {
    #[cfg(unix)]
    {
        ghost_process::ptable::snapshot()
            .map_err(|error| tracing::warn!(%error, "could not read the process table"))
            .ok()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The lists, from the tables `before` and `after` a run, the pids of its agents, the words that
/// name what the agent runs (`needles`), and the agent's declared daemons. `own_pid` (this
/// process) is never a leftover.
pub fn leftovers(
    before: &[ProcInfo],
    after: &[ProcInfo],
    agents: &[u32],
    needles: &[String],
    daemons: &[DaemonMatch],
    own_pid: u32,
) -> Leftovers {
    let mut found = Leftovers::default();
    let existed = |info: &ProcInfo| {
        before
            .iter()
            .any(|old| old.pid == info.pid && same_start(&old.start, &info.start))
    };
    let needles: Vec<&String> = needles.iter().filter(|needle| !needle.is_empty()).collect();
    let mut candidates: Vec<&ProcInfo> = Vec::new();
    for info in after {
        if info.pid == own_pid || info.ppid == own_pid {
            continue;
        }
        if agents.contains(&info.pgid) {
            found.group.push(info.clone());
        } else if matches_daemon(info, daemons) {
            found.spared.push(info.clone());
        } else if !existed(info) {
            let command = info.argv.join(" ");
            if needles
                .iter()
                .any(|needle| command.contains(needle.as_str()))
            {
                candidates.push(info);
            }
        }
    }
    // Orphans first, then whatever descends from a leftover, until nothing changes.
    let mut leftover: Vec<u32> = found.group.iter().map(|info| info.pid).collect();
    leftover.extend_from_slice(agents);
    loop {
        let counted = found.related.len();
        candidates.retain(|info| {
            let orphan = info.ppid <= 1
                || !after.iter().any(|parent| parent.pid == info.ppid)
                || leftover.contains(&info.ppid);
            if orphan {
                found.related.push((*info).clone());
                leftover.push(info.pid);
            }
            !orphan
        });
        if found.related.len() == counted {
            break;
        }
    }
    found.related.sort_by_key(|info| info.pid);
    found
}

/// Start times compare as equal unless both are known and differ.
fn same_start(a: &StartTime, b: &StartTime) -> bool {
    matches!(a, StartTime::Unknown) || matches!(b, StartTime::Unknown) || a == b
}

/// One process for a report line: `5120 node /path/to/entry.js`.
pub fn describe(info: &ProcInfo) -> String {
    let mut command = info.argv.join(" ");
    if command.chars().count() > 160 {
        command = command.chars().take(160).collect::<String>() + "...";
    }
    format!("{} {command}", info.pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, ppid: u32, pgid: u32, start: &str, argv: &str) -> ProcInfo {
        ProcInfo {
            pid,
            ppid,
            pgid,
            sid: None,
            start: StartTime::MacLstart(start.into()),
            argv: argv.split(' ').map(str::to_owned).collect(),
        }
    }

    #[test]
    fn group_members_related_newcomers_and_spared_daemons_are_told_apart() {
        let gateway = DaemonMatch {
            program: None,
            args_in_order: vec!["hermes_cli.main".into(), "gateway".into(), "run".into()],
        };
        let before = vec![
            proc(10, 1, 10, "a", "/usr/bin/zsh"),
            proc(
                20,
                1,
                20,
                "a",
                "/opt/hermes/bin/python -m hermes_cli.main gateway run --quiet",
            ),
            // An unrelated process whose pid is recycled during the run.
            proc(30, 1, 30, "old", "/usr/bin/vim notes.txt"),
        ];
        let after = vec![
            proc(10, 1, 10, "a", "/usr/bin/zsh"),
            proc(
                20,
                1,
                20,
                "a",
                "/opt/hermes/bin/python -m hermes_cli.main gateway run --quiet",
            ),
            proc(30, 1, 30, "new", "/tmp/run/repo/node_modules/.bin/tool"),
            // A child of the orphan above: a leftover too.
            proc(
                31,
                30,
                31,
                "new",
                "/tmp/run/repo/node_modules/.bin/tool --child",
            ),
            // The user's own shell (it existed before) runs a tool in the chat folder meanwhile.
            proc(32, 10, 32, "new", "/usr/bin/grep -r x /tmp/run/repo"),
            proc(5001, 1, 5001, "b", "/x/.local/bin/hermes acp"),
            proc(5002, 1, 5001, "b", "/bin/sh -c sleep 100"),
            proc(5003, 1, 5003, "b", "/usr/bin/caffeinate"),
            proc(77, 1, 77, "b", "/x/ghost-cli probe hermes"),
            proc(78, 77, 78, "b", "ps -axo pid=,ppid=,pgid=,lstart=,command="),
        ];
        let found = leftovers(
            &before,
            &after,
            &[5001],
            &[
                "/x/.local/bin/hermes".into(),
                "/tmp/run/repo".into(),
                String::new(),
            ],
            std::slice::from_ref(&gateway),
            77,
        );
        let pids = |list: &[ProcInfo]| list.iter().map(|info| info.pid).collect::<Vec<_>>();
        assert_eq!(pids(&found.group), [5001, 5002]);
        assert_eq!(
            pids(&found.related),
            [30, 31],
            "the recycled pid is a newcomer, its child too; the user's own grep is not"
        );
        assert_eq!(pids(&found.spared), [20]);
        assert!(!found.is_clean());
        assert!(describe(&found.group[0]).starts_with("5001 /x/.local/bin/hermes acp"));

        let clean = leftovers(&before, &before, &[5001], &[], &[gateway], 77);
        assert!(clean.is_clean());
        assert_eq!(pids(&clean.spared), [20]);
    }
}
