//! The shutdown chain and the kill rule against the real OS: group members, job-control children,
//! PTY hosts and detached children die; declared daemons and their subtrees live. Plus the spike's
//! process tests (`docs/spikes/windows-spawn/tests/spawn.rs`), ported to this crate's API. Every
//! test kills what it spawned and checks nothing is left, by pid and start time.

#[cfg(test)]
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{GONE_WITHIN, Reaper, STARTUP, alive, launch, next_json, read_pids, spec, wait_gone};
use ghost_process::{
    DaemonMatch, LeftoverRegistry, ProcessSpawner, ProtocolLog, ShutdownPlan, ShutdownStep,
    SpawnedProcess, Spawner,
};
use serde_json::json;
use tokio::runtime::Handle;

/// Short graces, so a test that has to wait them out stays quick.
const QUICK: ShutdownPlan = ShutdownPlan {
    stdin_eof_grace: Duration::from_millis(1500),
    sigterm_grace: Duration::from_millis(1500),
    kill_confirm: Duration::from_secs(2),
};

#[cfg(test)]
fn spawner() -> Arc<Spawner> {
    Spawner::new(LeftoverRegistry::disabled(), Handle::current())
}

/// Spawns `mode <pidfile> [extra]` and returns the process, its descendants' pids, and a reaper
/// holding all of them.
#[cfg(test)]
async fn with_children(
    spawner: &Spawner,
    folder: &std::path::Path,
    mode: &str,
    extra: &[&str],
    daemons: Vec<DaemonMatch>,
) -> (SpawnedProcess, Vec<u32>, Reaper) {
    let pidfile = folder.join(format!("{mode}.pid"));
    let pidfile_arg = pidfile.to_string_lossy().into_owned();
    let mut args = vec![mode, pidfile_arg.as_str()];
    args.extend_from_slice(extra);
    let mut launch = launch(&args);
    launch.daemons = daemons;
    let mut process = spawner
        .spawn(spec(launch, folder), ProtocolLog::disabled())
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    assert_eq!(
        next_json(&mut process.io.stdout).await,
        json!({"ready": true})
    );
    let pids = read_pids(&pidfile).await;
    reaper.hold_all(&pids);
    (process, pids, reaper)
}

#[cfg(unix)]
#[cfg(test)]
fn testbin_daemon() -> Vec<DaemonMatch> {
    vec![DaemonMatch {
        program: Some("ghost-testbin".into()),
        args_in_order: vec!["daemon".into(), "run".into()],
    }]
}

#[tokio::test]
async fn group_member_is_killed() {
    let folder = tempfile::tempdir().expect("tempdir");
    let (process, pids, reaper) = with_children(
        &spawner(),
        folder.path(),
        "spawn-group-child",
        &[],
        Vec::new(),
    )
    .await;
    let report = process.control.shutdown(ShutdownPlan::STANDARD).await;
    assert_eq!(
        report.exit.and_then(|exit| exit.code),
        Some(0),
        "{report:?}"
    );
    #[cfg(unix)]
    assert!(
        matches!(
            report.steps.as_slice(),
            [
                ShutdownStep::ClosedStdin,
                ShutdownStep::ExitedAfterEof,
                ShutdownStep::SentSigterm { .. }
            ]
        ),
        "the agent left on EOF, its group member got SIGTERM: {report:?}"
    );
    #[cfg(windows)]
    assert_eq!(
        report.steps,
        [
            ShutdownStep::ClosedStdin,
            ShutdownStep::ExitedAfterEof,
            ShutdownStep::TerminatedJob
        ]
    );
    assert!(wait_gone(pids[0], GONE_WITHIN).await, "the grandchild died");
    assert!(report.leftovers.is_empty(), "{report:?}");
    reaper.assert_gone().await;
}

#[tokio::test]
async fn clean_eof_sends_no_signal() {
    let folder = tempfile::tempdir().expect("tempdir");
    let process = spawner()
        .spawn(
            spec(launch(&["echo"]), folder.path()),
            ProtocolLog::disabled(),
        )
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    let report = process.control.shutdown(ShutdownPlan::STANDARD).await;
    let exit = report.exit.expect("exited");
    assert_eq!((exit.code, exit.signal), (Some(0), None));
    #[cfg(unix)]
    assert_eq!(
        report.steps,
        [ShutdownStep::ClosedStdin, ShutdownStep::ExitedAfterEof]
    );
    #[cfg(windows)]
    assert_eq!(
        report.steps,
        [
            ShutdownStep::ClosedStdin,
            ShutdownStep::ExitedAfterEof,
            ShutdownStep::TerminatedJob
        ]
    );
    assert!(report.took < ShutdownPlan::STANDARD.stdin_eof_grace);
    // A second call joins the same chain, even after it finished.
    assert_eq!(process.control.shutdown(ShutdownPlan::FORCE).await, report);
    reaper.assert_gone().await;
}

#[tokio::test]
async fn sigterm_ignored_then_killed() {
    let folder = tempfile::tempdir().expect("tempdir");
    let mut process = spawner()
        .spawn(
            spec(launch(&["ignore-sigterm"]), folder.path()),
            ProtocolLog::disabled(),
        )
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    assert_eq!(
        next_json(&mut process.io.stdout).await,
        json!({"ready": true})
    );
    let report = process.control.shutdown(QUICK).await;
    #[cfg(unix)]
    {
        assert_eq!(
            report.steps,
            [
                ShutdownStep::ClosedStdin,
                ShutdownStep::SentSigterm { processes: 1 },
                ShutdownStep::Killed { processes: 1 }
            ],
            "{report:?}"
        );
        assert_eq!(report.exit.and_then(|exit| exit.signal), Some(9));
    }
    #[cfg(windows)]
    assert_eq!(
        report.steps,
        [ShutdownStep::ClosedStdin, ShutdownStep::TerminatedJob]
    );
    assert!(report.exit.is_some());
    assert!(
        process
            .control
            .stderr_tail(4096)
            .contains("ignoring SIGTERM")
            || cfg!(windows)
    );
    reaper.assert_gone().await;
}

#[tokio::test]
async fn a_force_stop_skips_end_of_file() {
    let folder = tempfile::tempdir().expect("tempdir");
    let process = spawner()
        .spawn(
            spec(launch(&["echo"]), folder.path()),
            ProtocolLog::disabled(),
        )
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    let report = process.control.shutdown(ShutdownPlan::FORCE).await;
    assert!(
        !report.steps.contains(&ShutdownStep::ClosedStdin),
        "{report:?}"
    );
    #[cfg(unix)]
    assert_eq!(
        report.steps,
        [
            ShutdownStep::SentSigterm { processes: 1 },
            ShutdownStep::ExitedAfterSigterm
        ]
    );
    reaper.assert_gone().await;
}

/// A kill that lands after shutdown's first look leaves its later steps nothing to signal. The
/// exit still has to be reported, or a caller treats a finished cleanup as unconfirmed.
#[tokio::test]
async fn an_exit_during_shutdown_is_still_reported() {
    let folder = tempfile::tempdir().expect("tempdir");
    for _ in 0..5 {
        let process = spawner()
            .spawn(
                spec(launch(&["hang"]), folder.path()),
                ProtocolLog::disabled(),
            )
            .await
            .expect("spawn");
        let mut reaper = Reaper::new();
        reaper.hold(process.control.pid());
        process.control.kill_tree();
        let report = process.control.shutdown(ShutdownPlan::FORCE).await;
        assert!(report.exit.is_some(), "{report:?}");
        assert!(report.leftovers.is_empty(), "{report:?}");
        reaper.assert_gone().await;
    }
}

#[tokio::test]
async fn kill_tree_after_exit() {
    let folder = tempfile::tempdir().expect("tempdir");
    let process = spawner()
        .spawn(
            spec(launch(&["exit-before-read", "3"]), folder.path()),
            ProtocolLog::disabled(),
        )
        .await
        .expect("spawn");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    let exit = process.control.exited().await;
    assert_eq!(exit.code, Some(3));
    // Reaped already: killing the empty group (macOS answers EPERM or ESRCH) is not an error.
    process.control.kill_tree();
    process.control.kill_tree();
    assert_eq!(
        process.control.try_exited(),
        Some(exit),
        "the original exit stays"
    );
    let report = process.control.shutdown(ShutdownPlan::STANDARD).await;
    #[cfg(unix)]
    assert_eq!(report.steps, [ShutdownStep::AlreadyExited]);
    assert_eq!(report.exit, Some(exit));
    reaper.assert_gone().await;
}

#[tokio::test]
async fn dropping_control_kills_tree() {
    let folder = tempfile::tempdir().expect("tempdir");
    let (process, pids, reaper) = with_children(
        &spawner(),
        folder.path(),
        "spawn-group-child",
        &["hang"],
        Vec::new(),
    )
    .await;
    let agent = process.control.pid();
    drop(process);
    assert!(
        wait_gone(pids[0], GONE_WITHIN).await,
        "the grandchild died with the last handle"
    );
    assert!(wait_gone(agent, GONE_WITHIN).await);
    reaper.assert_gone().await;
}

#[tokio::test]
async fn kill_tree_kills_the_grandchild() {
    let folder = tempfile::tempdir().expect("tempdir");
    let (process, pids, reaper) = with_children(
        &spawner(),
        folder.path(),
        "spawn-group-child",
        &["hang"],
        Vec::new(),
    )
    .await;
    assert!(alive(pids[0]));
    process.control.kill_tree();
    let exit = tokio::time::timeout(STARTUP, process.control.exited())
        .await
        .expect("killed");
    assert_ne!(
        exit.code,
        Some(0),
        "killed, so it cannot have succeeded: {exit:?}"
    );
    assert!(
        wait_gone(pids[0], GONE_WITHIN).await,
        "grandchild {} survived kill_tree",
        pids[0]
    );
    reaper.assert_gone().await;
}

#[tokio::test]
async fn kill_tree_reaches_grandchildren_that_outlive_their_parent() {
    let folder = tempfile::tempdir().expect("tempdir");
    let (process, pids, reaper) = with_children(
        &spawner(),
        folder.path(),
        "spawn-group-child",
        &["exit"],
        Vec::new(),
    )
    .await;
    let exit = process.control.exited().await;
    assert_eq!(exit.code, Some(0), "the agent exits on its own");
    assert!(alive(pids[0]), "the grandchild outlives it");
    process.control.kill_tree();
    assert!(
        wait_gone(pids[0], GONE_WITHIN).await,
        "grandchild {} outlived kill_tree",
        pids[0]
    );
    assert_eq!(
        process.control.try_exited(),
        Some(exit),
        "kill_tree keeps the agent's own exit"
    );
    reaper.assert_gone().await;
}

#[tokio::test]
async fn kill_tree_without_runtime() {
    let folder = tempfile::tempdir().expect("tempdir");
    let (process, pids, reaper) = with_children(
        &spawner(),
        folder.path(),
        "spawn-group-child",
        &["hang"],
        Vec::new(),
    )
    .await;
    let control = process.control.clone();
    // A run-event callback at quit has no Tokio context: a plain thread stands in for it.
    std::thread::spawn(move || control.kill_tree())
        .join()
        .expect("join");
    assert!(wait_gone(pids[0], GONE_WITHIN).await);
    assert!(wait_gone(process.control.pid(), GONE_WITHIN).await);
    reaper.assert_gone().await;
}

#[cfg(unix)]
#[tokio::test]
async fn declared_daemon_survives() {
    let folder = tempfile::tempdir().expect("tempdir");
    let (process, pids, mut reaper) = with_children(
        &spawner(),
        folder.path(),
        "spawn-setsid-daemon",
        &[],
        testbin_daemon(),
    )
    .await;
    let (daemon, daemon_child) = (pids[0], pids[1]);
    let report = process.control.shutdown(QUICK).await;
    assert_eq!(
        report.exit.and_then(|exit| exit.code),
        Some(0),
        "{report:?}"
    );
    let mut spared = report.spared.clone();
    spared.sort_unstable();
    let mut expected = vec![daemon, daemon_child];
    expected.sort_unstable();
    assert_eq!(
        spared, expected,
        "the daemon and its subtree, listed as spared"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        alive(daemon) && alive(daemon_child),
        "both survive shutdown"
    );
    // The test is done with them.
    for pid in [daemon, daemon_child] {
        common::kill_now(pid);
        assert!(wait_gone(pid, GONE_WITHIN).await);
        reaper.release(pid);
    }
    reaper.assert_gone().await;
}

#[cfg(unix)]
#[tokio::test]
async fn daemon_subtree_is_spared() {
    let folder = tempfile::tempdir().expect("tempdir");
    // A vendor daemon in a group of its own, still parented under the agent.
    let (process, pids, mut reaper) = with_children(
        &spawner(),
        folder.path(),
        "spawn-group-daemon",
        &[],
        testbin_daemon(),
    )
    .await;
    let report = process.control.shutdown(QUICK).await;
    let mut spared = report.spared.clone();
    spared.sort_unstable();
    let mut expected = pids.clone();
    expected.sort_unstable();
    assert_eq!(spared, expected, "{report:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        pids.iter().all(|pid| alive(*pid)),
        "spared by its declaration, not its group"
    );
    for pid in pids {
        common::kill_now(pid);
        assert!(wait_gone(pid, GONE_WITHIN).await);
        reaper.release(pid);
    }
    reaper.assert_gone().await;
}

#[cfg(unix)]
#[tokio::test]
async fn job_control_child_is_killed() {
    let folder = tempfile::tempdir().expect("tempdir");
    let (process, pids, reaper) = with_children(
        &spawner(),
        folder.path(),
        "spawn-job-child",
        &[],
        Vec::new(),
    )
    .await;
    let report = process.control.shutdown(QUICK).await;
    assert!(
        matches!(report.steps.get(2), Some(ShutdownStep::SentSigterm { processes }) if *processes >= 1),
        "the escaped child got SIGTERM: {report:?}"
    );
    assert!(
        wait_gone(pids[0], GONE_WITHIN).await,
        "killed by the descendant walk"
    );
    assert!(report.spared.is_empty());
    reaper.assert_gone().await;
}

#[cfg(unix)]
#[tokio::test]
async fn setsid_child_without_daemon_match_is_killed() {
    let folder = tempfile::tempdir().expect("tempdir");
    let spawner = spawner();
    // At shutdown: found by the walk, though it left the group and the session.
    let (process, pids, reaper) = with_children(
        &spawner,
        folder.path(),
        "spawn-pty-child",
        &[],
        testbin_daemon(),
    )
    .await;
    let report = process.control.shutdown(QUICK).await;
    assert!(wait_gone(pids[0], GONE_WITHIN).await, "{report:?}");
    assert!(report.spared.is_empty(), "it matches no daemon");
    reaper.assert_gone().await;
    drop(process);

    // By kill_tree, for a child started during the current turn, so nothing recorded it yet;
    // from a thread with no runtime, as a run-event callback calls it.
    let sub = folder.path().join("kill-tree");
    std::fs::create_dir(&sub).expect("mkdir");
    let (process, pids, reaper) =
        with_children(&spawner, &sub, "spawn-pty-child", &[], testbin_daemon()).await;
    let control = process.control.clone();
    std::thread::spawn(move || control.kill_tree())
        .join()
        .expect("join");
    assert!(
        wait_gone(pids[0], GONE_WITHIN).await,
        "kill_tree walked to the escaped child before the agent died"
    );
    reaper.assert_gone().await;
    drop(process);

    // By the last control's drop, the same way.
    let sub = folder.path().join("last-drop");
    std::fs::create_dir(&sub).expect("mkdir");
    let (process, pids, reaper) =
        with_children(&spawner, &sub, "spawn-pty-child", &[], testbin_daemon()).await;
    drop(process);
    assert!(
        wait_gone(pids[0], GONE_WITHIN).await,
        "the last drop walked to the escaped child before the agent died"
    );
    reaper.assert_gone().await;
}

/// A child an earlier turn recorded is killed by kill_tree even after its parent exited, when no
/// walk can reach it any more.
#[cfg(unix)]
#[tokio::test]
async fn kill_tree_kills_recorded_escapees_whose_parent_is_gone() {
    let folder = tempfile::tempdir().expect("tempdir");
    let (process, pids, reaper) = with_children(
        &spawner(),
        folder.path(),
        "spawn-pty-child",
        &[],
        Vec::new(),
    )
    .await;
    process.control.record_escaped().await;
    // The agent leaves on EOF; its escaped child lives on, orphaned.
    process.control.close_stdin();
    let exit = tokio::time::timeout(STARTUP, process.control.exited())
        .await
        .expect("exited");
    assert_eq!(exit.code, Some(0));
    assert!(alive(pids[0]));
    process.control.kill_tree();
    assert!(
        wait_gone(pids[0], GONE_WITHIN).await,
        "kill_tree reached the recorded escapee"
    );
    reaper.assert_gone().await;
}

#[cfg(unix)]
#[tokio::test]
async fn the_registry_follows_an_agent_and_its_escapees() {
    let folder = tempfile::tempdir().expect("tempdir");
    let run = folder.path().join("run");
    let registry = LeftoverRegistry::open(&run, Handle::current()).expect("registry");
    let spawner = Spawner::new(Arc::clone(&registry), Handle::current());
    assert!(Arc::ptr_eq(spawner.registry(), &registry));
    let (process, pids, reaper) =
        with_children(&spawner, folder.path(), "spawn-pty-child", &[], Vec::new()).await;
    let file = std::fs::read_dir(&run)
        .expect("run dir")
        .map(|entry| entry.expect("entry").path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("the registry file");
    let entries = |file: &std::path::Path| -> Vec<serde_json::Value> {
        let body: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file).expect("read")).expect("json");
        body["entries"].as_array().expect("entries").clone()
    };
    let written = entries(&file);
    assert_eq!(written.len(), 1);
    assert_eq!(written[0]["pid"], json!(process.control.pid()));
    assert_eq!(written[0]["pgid"], json!(process.control.pid()));
    assert_eq!(written[0]["label"], json!("ghost-testbin spawn-pty-child"));
    assert_eq!(written[0]["escaped"], json!([]));
    process.control.record_escaped().await;
    let written = entries(&file);
    assert_eq!(
        written[0]["escaped"][0]["pid"],
        json!(pids[0]),
        "{written:?}"
    );
    let report = process.control.shutdown(QUICK).await;
    assert!(report.exit.is_some());
    assert_eq!(
        entries(&file),
        Vec::<serde_json::Value>::new(),
        "unregistered"
    );
    reaper.assert_gone().await;
    registry.close().expect("close");
    assert!(!file.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn registry_failure_is_a_warning() {
    let folder = tempfile::tempdir().expect("tempdir");
    let run = folder.path().join("run");
    let registry = LeftoverRegistry::open(&run, Handle::current()).expect("registry");
    // The run folder vanishes under it: every write fails from now on.
    std::fs::remove_dir_all(&run).expect("remove");
    let spawner = Spawner::new(Arc::clone(&registry), Handle::current());
    let mut process = spawner
        .spawn(
            spec(launch(&["echo"]), folder.path()),
            ProtocolLog::disabled(),
        )
        .await
        .expect("the agent runs anyway");
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    process
        .io
        .stdin
        .send(r#"{"still":"works"}"#.into())
        .expect("stdin");
    assert_eq!(
        next_json(&mut process.io.stdout).await,
        json!({"still": "works"})
    );
    process.control.shutdown(ShutdownPlan::STANDARD).await;
    reaper.assert_gone().await;
}

#[cfg(unix)]
#[tokio::test]
async fn the_sweep_stops_a_dead_instances_agent_and_spares_its_daemon() {
    use ghost_process::ptable::{StartTime, start_time};
    use ghost_process::{InstanceRecord, sweep_stale};

    let folder = tempfile::tempdir().expect("tempdir");
    let run = folder.path().join("run");
    std::fs::create_dir(&run).expect("mkdir");
    // An agent this test owns, left running as if its Ghost had crashed.
    let (process, pids, mut reaper) = with_children(
        &spawner(),
        folder.path(),
        "spawn-setsid-daemon",
        &[],
        testbin_daemon(),
    )
    .await;
    let agent = process.control.pid();
    let agent_start = start_time(agent).expect("start");
    let sid = nix::unistd::getsid(Some(nix::unistd::Pid::from_raw(
        i32::try_from(agent).expect("pid"),
    )))
    .expect("sid")
    .as_raw();
    let boot = ghost_process::ptable::boot_marker();
    let dead = InstanceRecord {
        pid: 4_000_000_000,
        start: agent_start.clone(),
        sid: Some(1),
        boot: boot.clone(),
    };
    let body = json!({
        "version": 1,
        "instance": dead,
        "entries": [{
            "pid": agent, "pgid": agent, "sid": sid, "start": agent_start,
            "label": "ghost-testbin spawn-setsid-daemon", "spawned_at_ms": 0,
            "daemons": testbin_daemon(), "escaped": [],
        }],
    });
    std::fs::write(run.join("4000000000-1.json"), body.to_string()).expect("write");
    let own = InstanceRecord {
        pid: std::process::id(),
        start: start_time(std::process::id()).unwrap_or(StartTime::Unknown),
        sid: None,
        boot,
    };
    let report = sweep_stale(&run, &own).await;
    assert_eq!(report.killed.len(), 1, "{report:?}");
    assert_eq!(report.killed[0].0, agent);
    assert_eq!(report.files_removed, 1);
    assert!(std::fs::read_dir(&run).expect("dir").next().is_none());
    assert!(wait_gone(agent, GONE_WITHIN).await);
    assert!(
        pids.iter().all(|pid| alive(*pid)),
        "the daemon and its child survive the sweep"
    );
    for pid in pids {
        common::kill_now(pid);
        assert!(wait_gone(pid, GONE_WITHIN).await);
        reaper.release(pid);
    }
    drop(process);
    reaper.assert_gone().await;
}

/// Starts `ghost-testbin orphan 60`, which owns a sleeper through the real spawner, and returns the
/// owner and the sleeper's pid. The owner exits without cleanup once it reads a line.
#[cfg(test)]
async fn crashing_owner(reaper: &mut Reaper) -> (tokio::process::Child, u32) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    // A plain spawn on purpose: the owner must not be in a group or job of ours.
    let mut owner = tokio::process::Command::new(common::testbin())
        .args(["orphan", "60"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("owner");
    reaper.hold(owner.id().expect("pid"));
    let stdout = owner.stdout.take().expect("piped");
    let line = tokio::time::timeout(STARTUP, BufReader::new(stdout).lines().next_line())
        .await
        .expect("in time")
        .expect("read")
        .expect("the sleeper's pid");
    let sleeper: u32 = line.trim().parse().expect("a pid");
    reaper.hold(sleeper);
    let mut stdin = owner.stdin.take().expect("piped");
    stdin.write_all(b"go\n").await.expect("write");
    stdin.flush().await.expect("flush");
    (owner, sleeper)
}

#[cfg(windows)]
#[tokio::test]
async fn the_job_kills_the_tree_when_its_owner_dies_without_cleanup() {
    let mut reaper = Reaper::new();
    let (mut owner, sleeper) = crashing_owner(&mut reaper).await;
    let status = tokio::time::timeout(STARTUP, owner.wait())
        .await
        .expect("in time")
        .expect("wait");
    assert!(status.success(), "owner: {status:?}");
    assert!(
        wait_gone(sleeper, GONE_WITHIN).await,
        "sleeper {sleeper} outlived its owner, so the job did not close"
    );
    reaper.assert_gone().await;
}

#[cfg(unix)]
#[tokio::test]
async fn unix_has_no_kernel_cleanup_when_the_owner_crashes() {
    // The documented gap: nothing ties a process group to Ghost's lifetime, so a crash leaves
    // the tree running, and the startup sweep of recorded agents has to find it.
    let mut reaper = Reaper::new();
    let (mut owner, sleeper) = crashing_owner(&mut reaper).await;
    let status = tokio::time::timeout(STARTUP, owner.wait())
        .await
        .expect("in time")
        .expect("wait");
    assert!(status.success(), "owner: {status:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        alive(sleeper),
        "expected the sleeper to survive its owner on Unix"
    );
    common::kill_now(sleeper);
    reaper.assert_gone().await;
}
