//! The Processes page's listing against the real OS: a spawned agent and what it started are
//! listed with their facts, even a program whose parent already exited; a child stops on request,
//! and the agent itself and processes outside its tree are refused.

#[cfg(test)]
mod common;

use std::sync::Arc;

use common::{GONE_WITHIN, Reaper, launch, next_json, read_pids, spec, wait_gone};
use ghost_process::tree::{Snapshot, StopError, TreeProcess, stop};
use ghost_process::{
    LeftoverRegistry, ProcessControl, ProcessSpawner, ProtocolLog, ShutdownPlan, SpawnedProcess,
    Spawner,
};
use serde_json::json;
use tokio::runtime::Handle;

#[cfg(test)]
async fn spawned(mode: &str, folder: &std::path::Path, extra: &[&str]) -> (SpawnedProcess, u32) {
    let pidfile = folder.join(format!("{mode}.pid"));
    let pidfile_arg = pidfile.to_string_lossy().into_owned();
    let mut args = vec![mode, pidfile_arg.as_str()];
    args.extend_from_slice(extra);
    let spawner: Arc<Spawner> = Spawner::new(LeftoverRegistry::disabled(), Handle::current());
    let mut process = spawner
        .spawn(spec(launch(&args), folder), ProtocolLog::disabled())
        .await
        .expect("spawn");
    assert_eq!(
        next_json(&mut process.io.stdout).await,
        json!({"ready": true})
    );
    let pid = read_pids(&pidfile).await[0];
    (process, pid)
}

#[cfg(test)]
async fn tree(control: &ProcessControl) -> Vec<TreeProcess> {
    let control = control.clone();
    tokio::task::spawn_blocking(move || Snapshot::take().map(|snapshot| snapshot.tree(&control)))
        .await
        .expect("join")
        .expect("snapshot")
}

#[tokio::test]
async fn an_agent_and_its_child_are_listed_and_the_child_stops_on_request() {
    let folder = tempfile::tempdir().expect("tempdir");
    let (process, child) = spawned("spawn-group-child", folder.path(), &["hang"]).await;
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    reaper.hold(child);

    let listed = tree(&process.control).await;
    assert_eq!(
        listed.first().map(|process| process.pid),
        Some(process.control.pid()),
        "the agent comes first: {listed:?}"
    );
    assert!(
        listed[0].name.contains("ghost-testbin") && listed[0].parent.is_none(),
        "the agent's own row: {:?}",
        listed[0]
    );
    let entry = listed
        .iter()
        .find(|entry| entry.pid == child)
        .unwrap_or_else(|| panic!("the child is listed: {listed:?}"));
    assert_eq!(entry.parent, Some(process.control.pid()));
    assert!(entry.name.contains("ghost-testbin"), "{entry:?}");
    assert!(
        entry
            .command
            .as_deref()
            .is_some_and(|command| command.contains("hang")),
        "{entry:?}"
    );
    #[cfg(windows)]
    assert!(
        entry.started_ms.is_some() && entry.cpu_ms.is_some() && entry.memory_bytes.is_some(),
        "{entry:?}"
    );

    assert_eq!(
        stop(&process.control, process.control.pid()),
        Err(StopError::Agent)
    );
    assert_eq!(
        stop(&process.control, std::process::id()),
        Err(StopError::NotInTree)
    );
    let control = process.control.clone();
    let stopped = tokio::task::spawn_blocking(move || stop(&control, child))
        .await
        .expect("join");
    assert_eq!(stopped, Ok(1));
    assert!(wait_gone(child, GONE_WITHIN).await, "the child was stopped");
    let after = tree(&process.control).await;
    assert!(after.iter().all(|entry| entry.pid != child), "{after:?}");

    let _ = process.control.shutdown(ShutdownPlan::STANDARD).await;
    reaper.assert_gone().await;
    assert!(
        tree(&process.control).await.is_empty(),
        "an exited agent lists nothing"
    );
}

/// The case the owner hit on 2026-11-03: a program started through a parent that exited at once
/// (`Start-Process`) still counts as the agent's.
#[tokio::test]
async fn a_program_whose_parent_exited_still_counts_as_the_agents() {
    let folder = tempfile::tempdir().expect("tempdir");
    let (process, orphan) = spawned("spawn-orphan", folder.path(), &[]).await;
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    reaper.hold(orphan);
    // The parent writes the pid file just before it exits, so the listing waits for its exit.
    let parent = read_pids(&folder.path().join("spawn-orphan.pid")).await[1];
    assert!(
        wait_gone(parent, GONE_WITHIN).await,
        "the orphan's parent exited"
    );

    let listed = tree(&process.control).await;
    let entry = listed
        .iter()
        .find(|entry| entry.pid == orphan)
        .unwrap_or_else(|| panic!("the orphan is listed: {listed:?}"));
    assert_eq!(entry.parent, None, "its parent is gone: {entry:?}");

    let _ = process.control.shutdown(ShutdownPlan::STANDARD).await;
    reaper.assert_gone().await;
}
