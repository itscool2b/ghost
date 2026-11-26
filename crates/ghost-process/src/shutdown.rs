//! The shutdown chain and the kill rule.
//!
//! The chain, on Unix: snapshot the agent's descendants (orphans reparent to init, so this comes
//! first), close stdin, wait `stdin_eof_grace`, `killpg(SIGTERM)` plus SIGTERM to every escaped
//! descendant, wait `sigterm_grace`, then SIGKILL to the group and to the escaped descendants still
//! alive, and wait `kill_confirm`. Windows skips SIGTERM (no signals across consoles) and terminates
//! the job.
//!
//! The kill rule: every member of the agent's process group dies, and so does every descendant the
//! parent walk finds in another group or session, unless it matches one of the agent's declared
//! daemons ([`DaemonMatch`](crate::spawn::DaemonMatch)), which is spared with its whole subtree.

use std::sync::Arc;
use std::time::Duration;

#[cfg(windows)]
use futures::FutureExt;
#[cfg(windows)]
use futures::future::BoxFuture;

use crate::control::{ControlState, ExitInfo};
use crate::log::LogNote;

/// How long each step of the chain waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownPlan {
    /// Zero skips closing stdin and goes straight to the signals.
    pub stdin_eof_grace: Duration,
    pub sigterm_grace: Duration,
    pub kill_confirm: Duration,
}

impl ShutdownPlan {
    /// A Stop: codex-acp needs more than 2 s after EOF, and Claude's SDKs wait 5 s.
    pub const STANDARD: ShutdownPlan = ShutdownPlan {
        stdin_eof_grace: Duration::from_secs(5),
        sigterm_grace: Duration::from_secs(3),
        kill_confirm: Duration::from_secs(2),
    };

    /// Quitting Ghost.
    pub const QUIT: ShutdownPlan = ShutdownPlan {
        stdin_eof_grace: Duration::from_secs(3),
        sigterm_grace: Duration::from_secs(1),
        kill_confirm: Duration::from_secs(2),
    };

    /// A force stop: no EOF, SIGTERM at once.
    pub const FORCE: ShutdownPlan = ShutdownPlan {
        stdin_eof_grace: Duration::ZERO,
        sigterm_grace: Duration::from_secs(2),
        kill_confirm: Duration::from_secs(2),
    };
}

/// What the chain did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    pub exit: Option<ExitInfo>,
    pub steps: Vec<ShutdownStep>,
    /// Processes still alive after the chain.
    pub leftovers: Vec<u32>,
    /// Declared daemons and their subtrees, left running on purpose.
    pub spared: Vec<u32>,
    pub took: Duration,
}

/// One step of the chain, in the order taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownStep {
    AlreadyExited,
    ClosedStdin,
    ExitedAfterEof,
    SentSigterm { processes: u32 },
    ExitedAfterSigterm,
    Killed { processes: u32 },
    TerminatedJob,
}

/// How often the chain checks whether what it signalled is gone.
#[cfg(unix)]
const POLL: Duration = Duration::from_millis(50);

/// How long `kill_tree` waits for its walk before it sends the stopped group SIGKILL itself. The
/// walk takes a few milliseconds on Linux and some tens on macOS, where it runs `ps`.
#[cfg(unix)]
pub(crate) const KILL_WALK_LIMIT: Duration = Duration::from_secs(1);

/// The shutdown chain of a real agent (see the module docs). Runs as a task of its own.
pub(crate) async fn run(state: Arc<ControlState>, plan: ShutdownPlan) -> ShutdownReport {
    let started = tokio::time::Instant::now();
    let mut report = chain(&state, plan).await;
    report.took = started.elapsed();
    state.log.note(LogNote::Shutdown {
        steps: report
            .steps
            .iter()
            .map(|step| format!("{step:?}"))
            .collect(),
        took_ms: u64::try_from(report.took.as_millis()).unwrap_or(u64::MAX),
    });
    report
}

/// Waits up to `limit` for the agent's own exit.
async fn wait_exit(state: &ControlState, limit: Duration) -> Option<ExitInfo> {
    let mut exit = state.exit.clone();
    let found = tokio::time::timeout(limit, exit.wait_for(Option::is_some)).await;
    match found {
        Ok(Ok(found)) => *found,
        _ => state.exited_now(),
    }
}

#[cfg(windows)]
async fn chain(state: &Arc<ControlState>, plan: ShutdownPlan) -> ShutdownReport {
    let mut steps = Vec::new();
    let mut exit = state.exited_now();
    if exit.is_some() {
        steps.push(ShutdownStep::AlreadyExited);
    } else if !plan.stdin_eof_grace.is_zero() {
        state.stdin.close();
        steps.push(ShutdownStep::ClosedStdin);
        exit = wait_exit(state, plan.stdin_eof_grace).await;
        if exit.is_some() {
            steps.push(ShutdownStep::ExitedAfterEof);
        }
    }
    // No SIGTERM across consoles: the job ends whatever is left, the agent included.
    let _ = state.kill.send(());
    steps.push(ShutdownStep::TerminatedJob);
    if exit.is_none() {
        exit = wait_exit(state, plan.kill_confirm).await;
    }
    state.registry.unregister(state.pid);
    ShutdownReport {
        exit,
        steps,
        leftovers: Vec::new(),
        spared: Vec::new(),
        took: Duration::ZERO,
    }
}

#[cfg(unix)]
use unix::chain;
#[cfg(unix)]
pub(crate) use unix::{kill_session_blocking, kill_tree, note_reaped, record_escaped};

#[cfg(windows)]
pub(crate) fn kill_tree(state: &Arc<ControlState>) {
    // A non-blocking request: the waiter owns the job. The job dies with Ghost in any case.
    let _ = state.kill.send(());
}

#[cfg(windows)]
pub(crate) fn record_escaped(_state: Arc<ControlState>) -> BoxFuture<'static, ()> {
    async {}.boxed()
}

#[cfg(unix)]
mod unix {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::Duration;

    use futures::FutureExt;
    use futures::future::BoxFuture;
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill, killpg};
    use nix::unistd::{Pid, getpgid};

    use super::{KILL_WALK_LIMIT, POLL, ShutdownPlan, ShutdownReport, ShutdownStep, wait_exit};
    use crate::control::ControlState;
    use crate::ptable::{self, ProcInfo, StartTime, descendants, spared_daemons};
    use crate::registry::EscapedProc;
    use crate::spawn::DaemonMatch;

    /// The agent's tree, sorted by the kill rule.
    #[derive(Debug, Default)]
    pub(crate) struct Tree {
        /// Members of the agent's group (killpg reaches them).
        pub(crate) members: Vec<u32>,
        /// Descendants outside the group, minus declared daemons: signalled one by one.
        pub(crate) escaped: Vec<EscapedProc>,
        /// Declared daemons and their subtrees.
        pub(crate) spared: Vec<u32>,
    }

    /// Sorts `table` (an agent's descendants and group) by the kill rule. Only a declared daemon
    /// that left the group is spared, with its subtree: one still in the group is a member like
    /// any other, and its descendants elsewhere are escaped descendants.
    pub(crate) fn classify(
        table: &[ProcInfo],
        root: u32,
        pgid: u32,
        daemons: &[DaemonMatch],
    ) -> Tree {
        let below = descendants(table, root);
        let in_group = |info: &ProcInfo| info.pgid == pgid;
        let spared = spared_daemons(&below, daemons, pgid);
        let mut tree = Tree {
            members: table
                .iter()
                .filter(|info| in_group(info))
                .map(|info| info.pid)
                .collect(),
            escaped: below
                .iter()
                .filter(|info| !in_group(info) && !spared.contains(&info.pid))
                .map(|info| EscapedProc {
                    pid: info.pid,
                    start: info.start.clone(),
                })
                .collect(),
            spared: spared.into_iter().collect(),
        };
        tree.spared.sort_unstable();
        tree
    }

    fn pid(raw: u32) -> Option<Pid> {
        i32::try_from(raw)
            .ok()
            .filter(|raw| *raw > 0)
            .map(Pid::from_raw)
    }

    /// Right after the waiter reaped the agent: a group that is empty now is empty for good, and
    /// its id is free for a new process to take, so nothing may signal it again. Permission errors
    /// are inconclusive; final cleanup inspects live group members before clearing the record.
    pub(crate) fn note_reaped(pgid: u32, gone: &AtomicBool) {
        let Some(group) = pid(pgid) else {
            return;
        };
        if matches!(killpg(group, None), Err(Errno::ESRCH)) {
            gone.store(true, Ordering::Release);
        }
    }

    /// Whether a signal to the agent's group reaches only the agent's own processes. The group's
    /// id is the agent's pid, which nothing else can take while the agent is not reaped. After
    /// that, the group is the agent's until it is seen empty, or until a new process holds that
    /// pid: the kernel hands out a pid only while no group has it as its id, so the agent's group
    /// had emptied first.
    fn group_is_ours(state: &ControlState) -> nix::Result<bool> {
        if state.group_gone() {
            return Ok(false);
        }
        if state.exited_now().is_none() {
            return Ok(true);
        }
        let Some(leader) = pid(state.pgid) else {
            return Err(Errno::EINVAL);
        };
        match getpgid(Some(leader)) {
            Err(Errno::ESRCH) => Ok(true),
            Ok(_) => {
                state.mark_group_gone();
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// Sends `signal` (`None` only probes) to the agent's group while it is still the agent's.
    /// `ESRCH` means it is empty for good.
    fn signal_group(state: &ControlState, signal: Option<Signal>) -> nix::Result<()> {
        let group = match pid(state.pgid) {
            Some(group) if group_is_ours(state)? => group,
            _ => return Err(Errno::ESRCH),
        };
        let sent = killpg(group, signal);
        if sent == Err(Errno::ESRCH) {
            state.mark_group_gone();
        }
        sent
    }

    /// Whether cleanup is still unconfirmed. Permission/read failures never prove absence.
    fn group_alive(state: &ControlState) -> bool {
        let exited = state.exited_now().is_some();
        match signal_group(state, None) {
            // Kernels can still report a group containing only zombies: inspect its live rows.
            Ok(()) | Err(Errno::EPERM) => {
                let live = ptable::group_has_live_member(state.pgid);
                if !live && exited {
                    state.mark_group_gone();
                }
                live
            }
            Err(Errno::ESRCH) => false,
            Err(_) => true,
        }
    }

    /// The escaped processes that still run as themselves (blocking: one read of the process
    /// table, however many there are).
    fn alive_escaped(escaped: &[EscapedProc]) -> Vec<EscapedProc> {
        ptable::still_running(escaped.iter().map(|proc| (proc.pid, &proc.start)))
            .into_iter()
            .map(|(pid, start)| EscapedProc { pid, start })
            .collect()
    }

    fn unconfirmed_escaped(escaped: &[EscapedProc]) -> Vec<EscapedProc> {
        ptable::not_confirmed_gone(escaped.iter().map(|proc| (proc.pid, &proc.start)))
            .into_iter()
            .map(|(pid, start)| EscapedProc { pid, start })
            .collect()
    }

    /// Signals the group and the escaped processes still running as themselves; returns how many.
    fn signal_all(state: &ControlState, tree: &Tree, signal: Signal) -> u32 {
        let mut count = 0;
        if signal_group(state, Some(signal)).is_ok() {
            count += u32::try_from(tree.members.len().max(1)).unwrap_or(u32::MAX);
        }
        for proc in alive_escaped(&tree.escaped) {
            if let Some(target) = pid(proc.pid)
                && kill(target, signal).is_ok()
            {
                count += 1;
            }
        }
        count
    }

    /// Reads the agent's tree on the blocking pool; an empty tree when it cannot be read.
    async fn snapshot(state: &Arc<ControlState>) -> Tree {
        let reader = Arc::clone(state);
        state
            .rt
            .spawn_blocking(move || {
                ptable::tree_of(reader.pid, reader.pgid)
                    .map(|table| classify(&table, reader.pid, reader.pgid, &reader.daemons))
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default()
    }

    /// Adds `more`'s escaped processes and spared daemons to `tree`.
    fn merge(tree: &mut Tree, more: Tree) {
        for proc in more.escaped {
            if !tree.escaped.iter().any(|known| known.pid == proc.pid) {
                tree.escaped.push(proc);
            }
        }
        for member in more.members {
            if !tree.members.contains(&member) {
                tree.members.push(member);
            }
        }
        for spared in more.spared {
            if !tree.spared.contains(&spared) {
                tree.spared.push(spared);
            }
        }
        tree.escaped.retain(|proc| !tree.spared.contains(&proc.pid));
    }

    /// Whether anything the kill rule covers still runs.
    async fn remaining(state: &Arc<ControlState>, tree: &Tree) -> bool {
        if state.exited_now().is_none() || group_alive(state) {
            return true;
        }
        if tree.escaped.is_empty() {
            return false;
        }
        let escaped = tree.escaped.clone();
        state
            .rt
            .spawn_blocking(move || !alive_escaped(&escaped).is_empty())
            .await
            .unwrap_or(false)
    }

    /// Polls until nothing the kill rule covers runs, or `limit` passes. Whether it all went.
    async fn wait_gone(state: &Arc<ControlState>, tree: &Tree, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if !remaining(state, tree).await {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(POLL).await;
        }
    }

    async fn signal(state: &Arc<ControlState>, tree: &Tree, signal: Signal) -> u32 {
        let target = Arc::clone(state);
        let members = tree.members.clone();
        let escaped = tree.escaped.clone();
        state
            .rt
            .spawn_blocking(move || {
                let tree = Tree {
                    members,
                    escaped,
                    spared: Vec::new(),
                };
                signal_all(&target, &tree, signal)
            })
            .await
            .unwrap_or(0)
    }

    pub(crate) async fn chain(state: &Arc<ControlState>, plan: ShutdownPlan) -> ShutdownReport {
        let mut steps = Vec::new();
        let mut exit = state.exited_now();
        // 0. Orphans reparent to init, so the tree is read before anything can die.
        let mut tree = if exit.is_none() {
            snapshot(state).await
        } else {
            Tree::default()
        };
        let recorded = lock(&state.escaped).clone();
        merge(
            &mut tree,
            Tree {
                escaped: recorded,
                ..Tree::default()
            },
        );
        if exit.is_some() {
            steps.push(ShutdownStep::AlreadyExited);
        }
        // 1, 2. End of file, and time to exit on it.
        if exit.is_none() && !plan.stdin_eof_grace.is_zero() {
            state.stdin.close();
            steps.push(ShutdownStep::ClosedStdin);
            exit = wait_exit(state, plan.stdin_eof_grace).await;
            if exit.is_some() {
                steps.push(ShutdownStep::ExitedAfterEof);
            }
        }
        // 3, 4. SIGTERM to the group and the escaped descendants, re-read first.
        if remaining(state, &tree).await {
            if state.exited_now().is_none() {
                let again = snapshot(state).await;
                merge(&mut tree, again);
            }
            let agent_alive = state.exited_now().is_none();
            let processes = signal(state, &tree, Signal::SIGTERM).await;
            steps.push(ShutdownStep::SentSigterm { processes });
            wait_gone(state, &tree, plan.sigterm_grace).await;
            exit = state.exited_now();
            if agent_alive && exit.is_some() {
                steps.push(ShutdownStep::ExitedAfterSigterm);
            }
        }
        // 5, 6. SIGKILL to whatever is left, and time for it to land.
        if remaining(state, &tree).await {
            let processes = signal(state, &tree, Signal::SIGKILL).await;
            steps.push(ShutdownStep::Killed { processes });
            wait_gone(state, &tree, plan.kill_confirm).await;
            exit = match exit {
                Some(exit) => Some(exit),
                None => wait_exit(state, Duration::ZERO).await,
            };
        }
        // The agent can end on its own, or from an earlier kill, after the first read above.
        // The later steps then find nothing to signal and never read the exit again.
        exit = exit.or_else(|| state.exited_now());
        // 7. Remove recovery metadata only after cleanup is confirmed. A failed process-table
        // read or a surviving descendant must remain recoverable after this instance exits.
        state.retain_registry.store(true, Ordering::Release);
        let registry = state.registry.clone();
        let agent = state.pid;
        let escaped = tree.escaped.clone();
        let group = Arc::clone(state);
        let leftovers = state
            .rt
            .spawn_blocking(move || {
                let alive = unconfirmed_escaped(&escaped);
                let mut leftovers: Vec<u32> = alive.iter().map(|proc| proc.pid).collect();
                if group_alive(&group) {
                    // Who is left in the group, if the table can say.
                    let members = ptable::checked_group_members(group.pgid)
                        // The group was still alive but could not be enumerated. Report its
                        // identity as unconfirmed, rather than claiming an empty cleanup.
                        .unwrap_or_else(|_| vec![group.pid]);
                    leftovers.extend(members);
                }
                leftovers.sort_unstable();
                leftovers.dedup();
                let complete = leftovers.is_empty() && group.exited_now().is_some();
                registry.finish_shutdown(agent, alive, complete);
                group.retain_registry.store(!complete, Ordering::Release);
                leftovers
            })
            .await
            .unwrap_or_else(|_| vec![state.pid]);
        ShutdownReport {
            exit,
            steps,
            leftovers,
            spared: tree.spared,
            took: Duration::ZERO,
        }
    }

    /// Immediate and callable from any thread, with or without a runtime. The group is stopped
    /// inline: a stopped process can neither exit nor fork, so the agent's children stay its
    /// children (an orphan reparents to init, out of the walk's reach) until a walk on a thread of
    /// its own has read them. That thread kills the escaped descendants it finds, the group, and
    /// the descendants recorded after earlier turns that still run as themselves. This waits for
    /// it up to [`KILL_WALK_LIMIT`], then sends the group SIGKILL itself, so the group is dead or
    /// dying when this returns.
    pub(crate) fn kill_tree(state: &Arc<ControlState>) {
        let stopped = signal_group(state, Some(Signal::SIGSTOP)).is_ok();
        if !stopped && lock(&state.escaped).is_empty() {
            return;
        }
        let target = Arc::clone(state);
        let (done, finished) = std::sync::mpsc::channel::<()>();
        let walker = std::thread::Builder::new()
            .name("aura-kill-tree".into())
            .spawn(move || {
                kill_walked(&target, stopped);
                let _ = done.send(());
            });
        let finished = match walker {
            Ok(_) => finished.recv_timeout(KILL_WALK_LIMIT).is_ok(),
            Err(error) => {
                tracing::warn!(pid = state.pid, %error, "could not start the walk for an agent's escaped processes");
                false
            }
        };
        if stopped && !finished {
            // Without the walk the escaped descendants it would find may survive; the group never
            // does. A walk still running kills what it finds when it gets there.
            let _ = signal_group(state, Some(Signal::SIGKILL));
        }
    }

    /// [`kill_tree`]'s walk: the escaped descendants of the stopped agent (`stopped`), then its
    /// group, then the recorded escaped descendants that still run as themselves.
    fn kill_walked(state: &ControlState, stopped: bool) {
        let mut found: Vec<EscapedProc> = Vec::new();
        if stopped
            && state.start != StartTime::Unknown
            && let Ok(table) = ptable::tree_of(state.pid, state.pgid)
            // Only while the agent is still itself: a recycled pid's children are not ours.
            && table
                .iter()
                .any(|info| info.pid == state.pid && info.start == state.start)
        {
            found = classify(&table, state.pid, state.pgid, &state.daemons).escaped;
        }
        // Read moments ago, so signalled as found: a start time read again now would leave the
        // same gap before the signal.
        for proc in &found {
            if let Some(target) = pid(proc.pid) {
                let _ = kill(target, Signal::SIGKILL);
            }
        }
        if stopped {
            let _ = signal_group(state, Some(Signal::SIGKILL));
        }
        let recorded: Vec<EscapedProc> = lock(&state.escaped)
            .iter()
            .filter(|proc| !found.iter().any(|known| known.pid == proc.pid))
            .cloned()
            .collect();
        for proc in alive_escaped(&recorded) {
            if let Some(target) = pid(proc.pid) {
                let _ = kill(target, Signal::SIGKILL);
            }
        }
    }

    /// Records, in the leftover registry, the descendants that left the agent's group, minus
    /// declared daemons; earlier records that still run stay, though the walk no longer reaches
    /// them once their parent exited.
    pub(crate) fn record_escaped(state: Arc<ControlState>) -> BoxFuture<'static, ()> {
        async move {
            if state.exited_now().is_some() {
                return;
            }
            let worker = Arc::clone(&state);
            let _ = state
                .rt
                .spawn_blocking(move || {
                    let Ok(table) = ptable::tree_of(worker.pid, worker.pgid) else {
                        return;
                    };
                    let mut escaped =
                        classify(&table, worker.pid, worker.pgid, &worker.daemons).escaped;
                    let earlier = lock(&worker.escaped).clone();
                    for proc in unconfirmed_escaped(&earlier) {
                        if !escaped.iter().any(|known| known.pid == proc.pid) {
                            escaped.push(proc);
                        }
                    }
                    *lock(&worker.escaped) = escaped.clone();
                    worker.registry.set_escaped(worker.pid, escaped);
                })
                .await;
        }
        .boxed()
    }

    /// Kills every process of session `sid` (a helper that started a session of its own), from one
    /// snapshot of the process table.
    pub(crate) fn kill_session_blocking(sid: u32) {
        let Ok(table) = ptable::snapshot() else {
            return;
        };
        for info in table.iter().filter(|info| info.sid == Some(sid)) {
            if let Some(target) = pid(info.pid) {
                let _ = kill(target, Signal::SIGKILL);
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
        use crate::ptable::StartTime;

        fn proc(pid: u32, ppid: u32, pgid: u32, argv: &[&str]) -> ProcInfo {
            ProcInfo {
                pid,
                ppid,
                pgid,
                sid: Some(1),
                start: StartTime::MacLstart(format!("Sat Oct 24 09:00:{:02} 2026", pid % 60)),
                argv: argv.iter().map(|word| (*word).to_owned()).collect(),
            }
        }

        #[test]
        fn the_kill_rule_sorts_members_escapees_and_spared_daemons() {
            let daemons = [DaemonMatch {
                program: Some("claude".into()),
                args_in_order: vec!["daemon".into(), "run".into()],
            }];
            let table = [
                proc(500, 1, 500, &["node", "claude-agent-acp"]),
                proc(501, 500, 500, &["bash"]),
                proc(502, 501, 600, &["vite", "--port", "5173"]),
                proc(
                    503,
                    500,
                    700,
                    &["claude", "daemon", "run", "--origin", "transient"],
                ),
                proc(504, 503, 700, &["bg-pty-host"]),
                proc(505, 504, 800, &["zsh"]),
                proc(506, 1, 500, &["orphaned-member"]),
                // A daemon match inside the group is only a member: it dies with the group, and
                // what it started in a group of its own is an escaped descendant like any other.
                proc(507, 500, 500, &["claude", "daemon", "run"]),
                proc(508, 507, 900, &["bg-pty-host"]),
                proc(509, 508, 900, &["zsh"]),
            ];
            let tree = classify(&table, 500, 500, &daemons);
            let mut members = tree.members.clone();
            members.sort_unstable();
            assert_eq!(members, [500, 501, 506, 507]);
            let mut escaped: Vec<u32> = tree.escaped.iter().map(|proc| proc.pid).collect();
            escaped.sort_unstable();
            assert_eq!(escaped, [502, 508, 509]);
            assert_eq!(tree.spared, [503, 504, 505]);
            let vite = tree.escaped.iter().find(|proc| proc.pid == 502);
            assert_eq!(vite.map(|proc| &proc.start), Some(&table[2].start));
        }

        /// A control state for `pid`, as `spawn` builds one, and the sender of its exit.
        fn state_for(
            pid: u32,
        ) -> (
            Arc<ControlState>,
            tokio::sync::watch::Sender<Option<crate::control::ExitInfo>>,
        ) {
            let (exit_tx, exit_rx) = tokio::sync::watch::channel(None);
            let (stdin, _stdin_rx) = crate::io::LineSink::channel();
            let state = ControlState::new(
                pid,
                StartTime::Unknown,
                stdin,
                exit_rx,
                Arc::new(crate::stderr::StderrTail::default()),
                crate::log::ProtocolLog::disabled(),
                tokio::runtime::Handle::current(),
                crate::registry::LeftoverRegistry::disabled().handle(),
                Vec::new(),
                Arc::new(AtomicBool::new(false)),
            );
            (Arc::new(state), exit_tx)
        }

        #[test]
        fn the_waiter_marks_a_group_gone_only_when_it_is_empty() {
            let gone = AtomicBool::new(false);
            let own = u32::try_from(nix::unistd::getpgrp().as_raw()).expect("pgid");
            note_reaped(own, &gone);
            assert!(
                !gone.load(Ordering::Acquire),
                "this test's own group has members"
            );
            // No process can have this id: it is above every pid Linux or macOS hands out.
            note_reaped(4_194_305, &gone);
            assert!(gone.load(Ordering::Acquire));
        }

        /// A child killed and reaped when dropped, so a failed assertion leaves nothing behind (an
        /// unreaped child's pid cannot be recycled, so the kill can only reach it).
        struct Sleeper(std::process::Child);

        impl Drop for Sleeper {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        /// The agent's group id is its pid: once the agent is reaped, that id is free, so the
        /// group is signalled only while it is still the agent's.
        #[tokio::test]
        async fn a_reaped_agents_group_is_signalled_only_while_it_is_its_own() {
            use std::os::unix::process::CommandExt;

            // A process leading a group of its own, standing in for the agent.
            let mut sleeper = Sleeper(
                std::process::Command::new("/bin/sleep")
                    .arg("30")
                    .process_group(0)
                    .spawn()
                    .expect("sleep"),
            );
            let pid = sleeper.0.id();
            let (state, exit) = state_for(pid);
            assert!(
                group_is_ours(&state).expect("ownership"),
                "not reaped: nothing else can have its id"
            );
            assert_eq!(signal_group(&state, None), Ok(()));
            assert!(group_alive(&state));
            // The waiter published the agent's exit, so it was reaped, yet a process holds its
            // pid: the agent's group had emptied first, and nothing signals this one.
            exit.send_replace(Some(crate::control::ExitInfo::unknown()));
            assert!(!group_is_ours(&state).expect("ownership"));
            assert!(state.group_gone());
            assert_eq!(
                signal_group(&state, Some(Signal::SIGKILL)),
                Err(Errno::ESRCH)
            );
            kill_tree(&state);
            assert!(!group_alive(&state));
            assert!(
                sleeper.0.try_wait().expect("try_wait").is_none(),
                "never signalled"
            );
            drop(sleeper);

            // Reaped, and no process holds the pid: the group stays the agent's until it is seen
            // empty, and from then on it is gone for good.
            let (state, exit) = state_for(pid);
            exit.send_replace(Some(crate::control::ExitInfo::unknown()));
            assert!(group_is_ours(&state).expect("ownership"));
            assert!(!group_alive(&state));
            assert!(state.group_gone());
            assert!(!group_is_ours(&state).expect("ownership"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_match_the_spec() {
        assert_eq!(
            ShutdownPlan::STANDARD.stdin_eof_grace,
            Duration::from_secs(5)
        );
        assert_eq!(ShutdownPlan::QUIT.sigterm_grace, Duration::from_secs(1));
        assert!(ShutdownPlan::FORCE.stdin_eof_grace.is_zero());
        assert_eq!(ShutdownPlan::FORCE.kill_confirm, Duration::from_secs(2));
    }
}
