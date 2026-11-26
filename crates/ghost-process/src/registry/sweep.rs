//! The startup sweep, over a [`System`] so its rules are tested against a fake process table.
//!
//! For every registry file that is not this instance's:
//!
//! 1. Its instance is alive (its pid runs with the recorded start time): skip; another Ghost
//!    owns those processes.
//! 2. The boot marker differs: the machine rebooted; delete the file, kill nothing.
//! 3. The instance is dead. Candidates are (a) processes whose `pgid` and `sid` equal an entry's and
//!    whose start time lies between the entry's start and the file's modification time plus
//!    [`SWEEP_WINDOW_SLACK`] (a process started after that window cannot belong to the dead
//!    instance, so a terminal job that later reused the group id is never touched); (b) the
//!    entry's escaped processes whose pid and start time still match; (c) when the entry's leader
//!    is alive with its recorded start time, its descendants. Declared daemons outside the entry's
//!    group are spared with their subtrees (one in the group is a member like any other).
//!    SIGTERM, [`SWEEP_SIGTERM_GRACE`], SIGKILL, start times re-checked before each signal.
//! 4. The file is deleted.
//!
//! Every skipped candidate is logged with its reason.

use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::{FileBody, InstanceRecord, LeftoverEntry, SweepReport, VERSION};
use crate::ptable::{ProcInfo, StartTime, descendants, spared_daemons};
use crate::{SWEEP_SIGTERM_GRACE, SWEEP_WINDOW_SLACK};

/// What the sweep reads and signals.
pub(crate) trait System {
    fn snapshot(&self) -> io::Result<Vec<ProcInfo>>;
    /// The start time of `pid` if it runs (zombies do not).
    fn running_start(&self, pid: u32) -> Option<StartTime>;
    /// Unix seconds of a start time, when it can say.
    fn start_epoch(&self, start: &StartTime) -> Option<i64>;
    /// SIGTERM, or SIGKILL with `kill`.
    fn signal(&self, pid: u32, kill: bool);
    fn sleep(&self, duration: Duration);
}

/// The real process table and signals.
#[cfg(unix)]
pub(crate) struct RealSystem;

#[cfg(unix)]
impl System for RealSystem {
    fn snapshot(&self) -> io::Result<Vec<ProcInfo>> {
        crate::ptable::snapshot()
    }

    fn running_start(&self, pid: u32) -> Option<StartTime> {
        crate::ptable::running_start(pid)
    }

    fn start_epoch(&self, start: &StartTime) -> Option<i64> {
        crate::ptable::start_epoch(start)
    }

    fn signal(&self, pid: u32, kill: bool) {
        use nix::sys::signal::Signal;
        let signal = if kill {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        if let Ok(raw) = i32::try_from(pid) {
            let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(raw), signal);
        }
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// A process the sweep will stop.
#[derive(Debug, Clone)]
struct Candidate {
    pid: u32,
    start: StartTime,
    label: String,
}

pub(crate) fn sweep(run_dir: &Path, own: &InstanceRecord, system: &dyn System) -> SweepReport {
    let mut report = SweepReport::default();
    let listing = match std::fs::read_dir(run_dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return report,
        Err(error) => {
            report
                .errors
                .push(format!("could not read {}: {error}", run_dir.display()));
            return report;
        }
    };
    let mut table: Option<Vec<ProcInfo>> = None;
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut doomed = Vec::new();
    for item in listing.flatten() {
        let path = item.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let name = path.display().to_string();
        let body: FileBody = match std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        {
            Ok(body) => body,
            Err(error) => {
                // An atomic write never leaves half a file, so this one is garbage.
                tracing::warn!(file = %name, %error, "removing an unreadable leftover registry file");
                report.errors.push(format!("{name}: unreadable ({error})"));
                if std::fs::remove_file(&path).is_ok() {
                    report.files_removed += 1;
                }
                continue;
            }
        };
        if body.version != VERSION {
            report.errors.push(format!(
                "{name}: version {} is not one this build reads",
                body.version
            ));
            continue;
        }
        let instance = &body.instance;
        if instance.pid == own.pid && instance.start == own.start {
            continue;
        }
        let alive = match &instance.start {
            StartTime::Unknown => system.running_start(instance.pid).is_some(),
            start => system.running_start(instance.pid).as_ref() == Some(start),
        };
        if alive {
            tracing::debug!(file = %name, pid = instance.pid, "skipping the records of a running instance");
            report.skipped_live_owner += 1;
            continue;
        }
        if let (Some(then), Some(now)) = (&instance.boot, &own.boot)
            && then != now
        {
            tracing::info!(file = %name, "removing a leftover registry file from before a reboot");
            if std::fs::remove_file(&path).is_ok() {
                report.files_removed += 1;
            }
            continue;
        }
        if !body.entries.is_empty() && table.is_none() {
            match system.snapshot() {
                Ok(snapshot) => table = Some(snapshot),
                Err(error) => {
                    report
                        .errors
                        .push(format!("could not read the process table: {error}"));
                    return report;
                }
            }
        }
        let heartbeat = item
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
            .and_then(|since| i64::try_from(since.as_secs()).ok());
        let table = table.as_deref().unwrap_or_default();
        for entry in &body.entries {
            candidates.extend(plan(entry, heartbeat, table, system, &mut report));
        }
        doomed.push(path);
    }

    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.pid));
    let mut signalled = Vec::new();
    for candidate in candidates {
        match system.running_start(candidate.pid) {
            Some(start) if start == candidate.start => {
                system.signal(candidate.pid, false);
                signalled.push(candidate);
            }
            Some(_) => {
                tracing::info!(pid = candidate.pid, "not signalling a reused pid");
                report.skipped_reused += 1;
            }
            None => {}
        }
    }
    if !signalled.is_empty() {
        system.sleep(SWEEP_SIGTERM_GRACE);
    }
    for candidate in &signalled {
        if system.running_start(candidate.pid).as_ref() == Some(&candidate.start) {
            system.signal(candidate.pid, true);
        }
    }
    report.killed = signalled
        .into_iter()
        .map(|candidate| (candidate.pid, candidate.label))
        .collect();
    for path in doomed {
        match std::fs::remove_file(&path) {
            Ok(()) => report.files_removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => report
                .errors
                .push(format!("could not delete {}: {error}", path.display())),
        }
    }
    if !report.killed.is_empty() {
        tracing::info!(
            killed = report.killed.len(),
            "stopped agent processes an earlier Ghost left running"
        );
    }
    report
}

/// The candidates of one entry of a dead instance.
fn plan(
    entry: &LeftoverEntry,
    heartbeat: Option<i64>,
    table: &[ProcInfo],
    system: &dyn System,
    report: &mut SweepReport,
) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    let candidate = |info: &ProcInfo| Candidate {
        pid: info.pid,
        start: info.start.clone(),
        label: entry.label.clone(),
    };

    // (a) Group members inside the heartbeat window.
    let low = system.start_epoch(&entry.start);
    let high =
        heartbeat.map(|beat| beat + i64::try_from(SWEEP_WINDOW_SLACK.as_secs()).unwrap_or(0));
    match (low, high, entry.sid) {
        (Some(low), Some(high), Some(sid)) => {
            for info in table
                .iter()
                .filter(|info| info.pgid == entry.pgid && info.sid == Some(sid))
            {
                match system.start_epoch(&info.start) {
                    Some(start) if (low..=high).contains(&start) => found.push(candidate(info)),
                    Some(_) => tracing::info!(
                        pid = info.pid,
                        pgid = entry.pgid,
                        "skipping a group member that started outside the dead instance's window"
                    ),
                    None => tracing::info!(
                        pid = info.pid,
                        "skipping a group member without a start time"
                    ),
                }
            }
        }
        _ => tracing::info!(
            pid = entry.pid,
            "skipping the group of an entry whose window or session is unknown"
        ),
    }

    // (b) Recorded escaped descendants, by pid and start time.
    for escaped in &entry.escaped {
        match table.iter().find(|info| info.pid == escaped.pid) {
            Some(info) if info.start == escaped.start => found.push(candidate(info)),
            Some(_) => {
                tracing::info!(
                    pid = escaped.pid,
                    "skipping an escaped process whose pid was reused"
                );
                report.skipped_reused += 1;
            }
            None => {}
        }
    }

    // (c) The leader's descendants, when the leader itself still runs.
    let leader_alive = table
        .iter()
        .any(|info| info.pid == entry.pid && info.start == entry.start);
    if leader_alive {
        found.extend(descendants(table, entry.pid).iter().map(candidate));
    }

    // As at shutdown: only a daemon that left the agent's group is spared, with its subtree.
    let spared = spared_daemons(table, &entry.daemons, entry.pgid);
    found.retain(|candidate| {
        let keep = !spared.contains(&candidate.pid);
        if !keep {
            tracing::info!(pid = candidate.pid, "sparing a declared daemon");
        }
        keep
    });
    found
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::registry::EscapedProc;
    use crate::spawn::DaemonMatch;

    const BOOT: &str = "boot-1";

    fn at(epoch: i64) -> StartTime {
        // Seconds since the Unix epoch, written the way macOS ps does.
        let days = epoch.div_euclid(86_400);
        let secs = epoch.rem_euclid(86_400);
        // 2026-10-24 is day 20_750; the tests stay inside that month.
        let day = 24 + (days - 20_750);
        StartTime::MacLstart(format!(
            "Sat Oct {day} {:02}:{:02}:{:02} 2026",
            secs / 3600,
            secs % 3600 / 60,
            secs % 60
        ))
    }

    /// 2026-10-24 09:00:00 UTC.
    const T0: i64 = 1_792_832_400;

    #[derive(Default)]
    struct Fake {
        table: Vec<ProcInfo>,
        /// pid to its start; a signalled process leaves it (unless it ignores SIGTERM).
        alive: Mutex<HashMap<u32, StartTime>>,
        ignores_term: HashSet<u32>,
        signals: Mutex<Vec<(u32, bool)>>,
        slept: Mutex<Duration>,
    }

    impl Fake {
        fn new(table: Vec<ProcInfo>) -> Fake {
            let alive = table
                .iter()
                .map(|info| (info.pid, info.start.clone()))
                .collect();
            Fake {
                table,
                alive: Mutex::new(alive),
                ..Fake::default()
            }
        }

        fn signals(&self) -> Vec<(u32, bool)> {
            self.signals.lock().expect("lock").clone()
        }
    }

    impl System for Fake {
        fn snapshot(&self) -> io::Result<Vec<ProcInfo>> {
            Ok(self.table.clone())
        }
        fn running_start(&self, pid: u32) -> Option<StartTime> {
            self.alive.lock().expect("lock").get(&pid).cloned()
        }
        fn start_epoch(&self, start: &StartTime) -> Option<i64> {
            match start {
                StartTime::MacLstart(lstart) => crate::ptable::lstart_epoch(lstart),
                _ => None,
            }
        }
        fn signal(&self, pid: u32, kill: bool) {
            self.signals.lock().expect("lock").push((pid, kill));
            if kill || !self.ignores_term.contains(&pid) {
                self.alive.lock().expect("lock").remove(&pid);
            }
        }
        fn sleep(&self, duration: Duration) {
            *self.slept.lock().expect("lock") += duration;
        }
    }

    fn proc(pid: u32, ppid: u32, pgid: u32, sid: u32, start: i64, argv: &[&str]) -> ProcInfo {
        ProcInfo {
            pid,
            ppid,
            pgid,
            sid: Some(sid),
            start: at(start),
            argv: argv.iter().map(|word| (*word).to_owned()).collect(),
        }
    }

    fn own() -> InstanceRecord {
        InstanceRecord {
            pid: 100,
            start: at(T0 + 3600),
            sid: Some(100),
            boot: Some(BOOT.into()),
        }
    }

    fn dead_instance() -> InstanceRecord {
        InstanceRecord {
            pid: 42,
            start: at(T0 - 60),
            sid: Some(42),
            boot: Some(BOOT.into()),
        }
    }

    fn entry(pid: u32, start: i64, escaped: Vec<EscapedProc>) -> LeftoverEntry {
        LeftoverEntry {
            pid,
            pgid: pid,
            sid: Some(42),
            start: at(start),
            label: "hermes acp".into(),
            spawned_at_ms: start * 1000,
            daemons: vec![DaemonMatch {
                program: None,
                args_in_order: vec!["hermes_cli.main".into(), "gateway".into(), "run".into()],
            }],
            escaped,
        }
    }

    /// Writes a registry file whose modification time (the last heartbeat) is `heartbeat`.
    fn write_file(
        dir: &Path,
        instance: InstanceRecord,
        entries: Vec<LeftoverEntry>,
        heartbeat: i64,
    ) {
        let path = dir.join(format!("{}-1.json", instance.pid));
        let body = FileBody {
            version: VERSION,
            instance,
            entries,
        };
        std::fs::write(&path, serde_json::to_vec(&body).expect("json")).expect("write");
        let mtime = SystemTime::UNIX_EPOCH
            + Duration::from_secs(u64::try_from(heartbeat).expect("positive"));
        std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_modified(mtime)
            .expect("mtime");
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("dir")
            .map(|item| {
                item.expect("item")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn sweep_kills_dead_instance_leftovers() {
        let dir = tempfile::tempdir().expect("tempdir");
        // The dead instance's agent 5001 started at T0; its last heartbeat was T0 + 600.
        let escaped_start = T0 + 100;
        write_file(
            dir.path(),
            dead_instance(),
            vec![entry(
                5001,
                T0,
                vec![EscapedProc {
                    pid: 5300,
                    start: at(escaped_start),
                }],
            )],
            T0 + 600,
        );
        let fake = Fake::new(vec![
            proc(1, 0, 1, 1, T0 - 86_000, &["/sbin/launchd"]),
            // The leader still runs, with a group member and a descendant in another group.
            proc(5001, 1, 5001, 42, T0, &["hermes", "acp"]),
            proc(5002, 5001, 5001, 42, T0 + 5, &["bash"]),
            proc(5003, 1, 5001, 42, T0 + 650, &["dev-server"]),
            proc(5200, 5001, 5200, 5200, T0 + 30, &["pty-host"]),
            // The recorded escaped process, orphaned.
            proc(5300, 1, 5300, 5300, escaped_start, &["watcher"]),
            // Hermes' gateway with a child: spared.
            proc(
                5400,
                5001,
                5400,
                5400,
                T0 + 2,
                &["python3", "-m", "hermes_cli.main", "gateway", "run"],
            ),
            proc(5401, 5400, 5400, 5400, T0 + 3, &["gateway-worker"]),
        ]);
        let report = sweep(dir.path(), &own(), &fake);
        let mut killed: Vec<u32> = report.killed.iter().map(|(pid, _)| *pid).collect();
        killed.sort_unstable();
        assert_eq!(killed, [5001, 5002, 5003, 5200, 5300], "{report:?}");
        assert!(report.killed.iter().all(|(_, label)| label == "hermes acp"));
        assert_eq!(report.files_removed, 1);
        assert_eq!(report.skipped_live_owner, 0);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        // SIGTERM to each, then the grace; nothing was left for SIGKILL.
        assert!(fake.signals().iter().all(|(_, kill)| !kill));
        assert_eq!(*fake.slept.lock().expect("lock"), SWEEP_SIGTERM_GRACE);
        assert!(files(dir.path()).is_empty());
        let alive = fake.alive.lock().expect("lock");
        assert!(
            alive.contains_key(&5400) && alive.contains_key(&5401),
            "the daemon's subtree survives"
        );
    }

    /// The sweep and shutdown agree: a daemon match still in the agent's group is a member, so it
    /// and what it started elsewhere are stopped; one that left the group is spared.
    #[test]
    fn a_daemon_match_in_the_group_is_stopped_with_its_subtree() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(
            dir.path(),
            dead_instance(),
            vec![entry(5001, T0, Vec::new())],
            T0 + 600,
        );
        let gateway = ["python3", "-m", "hermes_cli.main", "gateway", "run"];
        let fake = Fake::new(vec![
            proc(5001, 1, 5001, 42, T0, &["hermes", "acp"]),
            // In the agent's group: a member, with a PTY host that left the group below it.
            proc(5500, 5001, 5001, 42, T0 + 1, &gateway),
            proc(5501, 5500, 5501, 5501, T0 + 2, &["pty-host"]),
            // Outside the group: spared with its child.
            proc(5600, 5001, 5600, 5600, T0 + 3, &gateway),
            proc(5601, 5600, 5600, 5600, T0 + 4, &["gateway-worker"]),
        ]);
        let report = sweep(dir.path(), &own(), &fake);
        let mut killed: Vec<u32> = report.killed.iter().map(|(pid, _)| *pid).collect();
        killed.sort_unstable();
        assert_eq!(killed, [5001, 5500, 5501], "{report:?}");
        let alive = fake.alive.lock().expect("lock");
        assert!(alive.contains_key(&5600) && alive.contains_key(&5601));
    }

    #[test]
    fn a_process_that_ignores_sigterm_gets_sigkill_after_the_grace() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(
            dir.path(),
            dead_instance(),
            vec![entry(5001, T0, Vec::new())],
            T0 + 60,
        );
        let mut fake = Fake::new(vec![proc(5001, 1, 5001, 42, T0, &["agent"])]);
        fake.ignores_term.insert(5001);
        let report = sweep(dir.path(), &own(), &fake);
        assert_eq!(fake.signals(), [(5001, false), (5001, true)]);
        assert_eq!(report.killed, [(5001, "hermes acp".to_owned())]);
    }

    #[test]
    fn sweep_skips_recycled_pid() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(
            dir.path(),
            dead_instance(),
            vec![entry(
                5001,
                T0,
                vec![EscapedProc {
                    pid: 5300,
                    start: at(T0 + 100),
                }],
            )],
            T0 + 600,
        );
        // Both pids now belong to other processes: other start times, other groups.
        let fake = Fake::new(vec![
            proc(5001, 1, 5001, 9000, T0 + 7200, &["someone-else"]),
            proc(5300, 1, 5300, 9000, T0 + 7300, &["also-someone-else"]),
        ]);
        let report = sweep(dir.path(), &own(), &fake);
        assert!(fake.signals().is_empty(), "{report:?}");
        assert!(report.killed.is_empty());
        assert_eq!(report.skipped_reused, 1, "the escaped pid");
        assert_eq!(report.files_removed, 1);
    }

    #[test]
    fn sweep_skips_live_instance() {
        let dir = tempfile::tempdir().expect("tempdir");
        let live = dead_instance();
        write_file(
            dir.path(),
            live.clone(),
            vec![entry(5001, T0, Vec::new())],
            T0 + 600,
        );
        let fake = Fake::new(vec![
            proc(42, 1, 42, 42, T0 - 60, &["auracode"]),
            proc(5001, 42, 5001, 42, T0, &["hermes", "acp"]),
        ]);
        let report = sweep(dir.path(), &own(), &fake);
        assert!(fake.signals().is_empty());
        assert_eq!(report.skipped_live_owner, 1);
        assert_eq!(report.files_removed, 0);
        assert_eq!(files(dir.path()), ["42-1.json"]);
        // This instance's own file is never touched either.
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(
            dir.path(),
            own(),
            vec![entry(5001, T0, Vec::new())],
            T0 + 600,
        );
        let report = sweep(dir.path(), &own(), &fake);
        assert_eq!(report, SweepReport::default());
        assert_eq!(files(dir.path()).len(), 1);
    }

    #[test]
    fn sweep_after_reboot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut before = dead_instance();
        before.boot = Some("boot-0".into());
        write_file(
            dir.path(),
            before,
            vec![entry(5001, T0, Vec::new())],
            T0 + 600,
        );
        // A process with the same pid, group and even start shows up: it is not ours.
        let fake = Fake::new(vec![proc(5001, 1, 5001, 42, T0, &["hermes", "acp"])]);
        let report = sweep(dir.path(), &own(), &fake);
        assert!(fake.signals().is_empty());
        assert_eq!(report.files_removed, 1);
        assert!(report.killed.is_empty());
        assert!(files(dir.path()).is_empty());
    }

    #[test]
    fn sweep_ignores_processes_after_heartbeat_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(
            dir.path(),
            dead_instance(),
            vec![entry(5001, T0, Vec::new())],
            T0 + 600,
        );
        // The leader exited; a terminal job later reused the group id in the same session.
        let fake = Fake::new(vec![
            proc(5001, 1, 5001, 42, T0 + 600 + 61, &["vim"]),
            proc(5002, 5001, 5001, 42, T0 + 700, &["sh"]),
            proc(4999, 1, 5001, 42, T0 - 1, &["before-the-agent"]),
        ]);
        let report = sweep(dir.path(), &own(), &fake);
        assert!(fake.signals().is_empty(), "{report:?}");
        assert!(report.killed.is_empty());
        assert_eq!(report.files_removed, 1);
        // Inside the window (the heartbeat plus 60 s), the member is the dead instance's.
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(
            dir.path(),
            dead_instance(),
            vec![entry(5001, T0, Vec::new())],
            T0 + 600,
        );
        let fake = Fake::new(vec![proc(5002, 1, 5001, 42, T0 + 660, &["late-member"])]);
        let report = sweep(dir.path(), &own(), &fake);
        assert_eq!(report.killed, [(5002, "hermes acp".to_owned())]);
    }

    #[test]
    fn unreadable_and_foreign_files_are_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("1-1.json"), b"{ not json").expect("write");
        std::fs::write(dir.path().join("2-1.json.tmp"), b"half").expect("write");
        let newer = serde_json::json!({"version": 2, "instance": dead_instance(), "entries": []});
        std::fs::write(dir.path().join("3-1.json"), newer.to_string()).expect("write");
        let fake = Fake::new(Vec::new());
        let report = sweep(dir.path(), &own(), &fake);
        assert_eq!(report.files_removed, 1);
        assert_eq!(report.errors.len(), 2, "{:?}", report.errors);
        assert_eq!(files(dir.path()), ["2-1.json.tmp", "3-1.json"]);
        let none = sweep(&dir.path().join("missing"), &own(), &fake);
        assert_eq!(none, SweepReport::default());
    }
}
