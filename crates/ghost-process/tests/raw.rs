//! Native raw transport checks use only the owned harmless test binary.

#[cfg(test)]
mod common;

use common::{GONE_WITHIN, Reaper, STARTUP, launch, read_pids, spec, wait_gone};
use ghost_process::{LeftoverRegistry, ShutdownPlan, Spawner};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::runtime::Handle;

fn spawner() -> Arc<Spawner> {
    Spawner::new(LeftoverRegistry::disabled(), Handle::current())
}

#[tokio::test]
async fn raw_transport_preserves_bytes_without_newlines_and_control_closes_stdin() {
    let folder = tempfile::tempdir().unwrap();
    let mut process = spawner()
        .spawn_raw(spec(launch(&["raw-echo"]), folder.path()))
        .await
        .unwrap();
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    let payload = vec![0, 255, 254, 10, 13, 27, 91, 48, 109, 0];
    process.stdin.write(payload.clone()).await.unwrap();
    let mut echoed = vec![0; payload.len()];
    tokio::time::timeout(STARTUP, process.stdout.read_exact(&mut echoed))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(echoed, payload);
    process.control.close_stdin();
    assert_eq!(
        tokio::time::timeout(STARTUP, process.control.exited())
            .await
            .unwrap()
            .code,
        Some(0)
    );
    assert!(process.stdin.is_closed());
    let mut remainder = Vec::new();
    process.stdout.read_to_end(&mut remainder).await.unwrap();
    assert!(
        remainder.is_empty(),
        "raw stdin adds no newline or terminator"
    );
    process.stderr.read_to_end(&mut remainder).await.unwrap();
    assert!(remainder.is_empty());
    assert_eq!(process.control.stderr_tail(1024), "");
    reaper.assert_gone().await;
}

#[tokio::test]
async fn raw_stdout_and_stderr_can_be_drained_concurrently() {
    let folder = tempfile::tempdir().unwrap();
    let mut process = spawner()
        .spawn_raw(spec(launch(&["flood-stderr", "131072"]), folder.path()))
        .await
        .unwrap();
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let read = async {
        tokio::try_join!(
            process.stdout.read_to_end(&mut output),
            process.stderr.read_to_end(&mut errors)
        )
    };
    tokio::time::timeout(STARTUP, read).await.unwrap().unwrap();
    assert_eq!(errors.len(), 131072);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output).unwrap()["done"],
        true
    );
    assert_eq!(process.control.exited().await.code, Some(0));
    assert_eq!(
        process.control.stderr_tail(1024),
        "",
        "the raw stream has no retained stderr copy"
    );
    reaper.assert_gone().await;
}

#[tokio::test]
async fn dropping_raw_control_kills_only_its_owned_tree() {
    use tokio::io::AsyncBufReadExt;
    let folder = tempfile::tempdir().unwrap();
    let pids = folder.path().join("child.pid");
    let sibling = spawner()
        .spawn_raw(spec(launch(&["hang"]), folder.path()))
        .await
        .unwrap();
    let process = spawner()
        .spawn_raw(spec(
            launch(&["spawn-group-child", pids.to_str().unwrap(), "hang"]),
            folder.path(),
        ))
        .await
        .unwrap();
    let mut reaper = Reaper::new();
    reaper.hold(process.control.pid());
    reaper.hold(sibling.control.pid());
    let mut reader = BufReader::new(process.stdout);
    let mut ready = String::new();
    tokio::time::timeout(STARTUP, reader.read_line(&mut ready))
        .await
        .unwrap()
        .unwrap();
    let children = read_pids(&pids).await;
    for pid in &children {
        reaper.hold(*pid);
    }
    let parent = process.control.pid();
    drop(process.control);
    assert!(wait_gone(parent, GONE_WITHIN).await);
    for child in children {
        assert!(wait_gone(child, GONE_WITHIN).await);
    }
    assert!(
        sibling.control.try_exited().is_none(),
        "another owned session was not stopped"
    );
    sibling.control.shutdown(ShutdownPlan::FORCE).await;
    reaper.assert_gone().await;
}
