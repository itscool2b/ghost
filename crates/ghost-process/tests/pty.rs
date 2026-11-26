//! Native terminal tests use only the owned test binary, never a vendor login or shell profile.
#[cfg(test)]
mod common;

use common::{GONE_WITHIN, Reaper, STARTUP, launch, read_pids, spec, wait_gone};
use ghost_process::{
    LeftoverRegistry, Spawner,
    pty::{PtyOutput, PtySize},
};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;

fn spawner() -> Arc<Spawner> {
    Spawner::new(LeftoverRegistry::disabled(), Handle::current())
}
const SIZE: PtySize = PtySize { rows: 24, cols: 80 };

#[cfg(test)]
async fn until(output: &mut PtyOutput, needle: &str) -> Vec<u8> {
    tokio::time::timeout(STARTUP, async {
        let mut bytes = Vec::new();
        loop {
            let next = output
                .next()
                .await
                .expect("terminal output ended before marker");
            bytes.extend(next);
            assert!(bytes.len() < 1024 * 1024, "missing terminal marker");
            if String::from_utf8_lossy(&bytes).contains(needle) {
                return bytes;
            }
        }
    })
    .await
    .expect("terminal marker deadline")
}

#[tokio::test]
async fn native_terminal_accepts_input_preserves_arguments_and_resizes() {
    let folder = tempfile::tempdir().unwrap();
    let mut terminal = spawner()
        .spawn_pty(
            spec(
                launch(&["terminal-echo", "space and quote\"", "trailing\\", "界"]),
                folder.path(),
            ),
            PtySize {
                rows: 24,
                cols: 160,
            },
        )
        .await
        .unwrap();
    let mut reaper = Reaper::new();
    reaper.hold(terminal.control.pid());
    let ready = until(&mut terminal.output, "界\"]").await;
    let ready = String::from_utf8_lossy(&ready);
    assert!(
        ready.contains("terminal-ready stdin=true stdout=true"),
        "{ready}"
    );
    assert!(ready.contains("space and quote\\\""), "{ready}");
    terminal.input.write(b"hello\r".to_vec()).await.unwrap();
    until(&mut terminal.output, "terminal-received hello").await;
    terminal
        .control
        .resize(PtySize {
            rows: 35,
            cols: 110,
        })
        .unwrap();
    terminal.input.write(b"size\r".to_vec()).await.unwrap();
    until(&mut terminal.output, "terminal-size 110x35").await;
    terminal.input.write(b"quit\r".to_vec()).await.unwrap();
    let exit = tokio::time::timeout(STARTUP, terminal.control.exited())
        .await
        .unwrap();
    assert_eq!(exit.code, Some(0));
    tokio::time::timeout(STARTUP, async {
        while terminal.output.next().await.is_some() {}
    })
    .await
    .unwrap();
    assert!(!terminal.output.truncated());
    reaper.assert_gone().await;
}

#[tokio::test]
async fn dropping_terminal_control_stops_its_descendants_but_not_a_sibling() {
    let folder = tempfile::tempdir().unwrap();
    let pids = folder.path().join("child.pid");
    let sibling = spawner()
        .spawn_pty(spec(launch(&["hang"]), folder.path()), SIZE)
        .await
        .unwrap();
    let mut terminal = spawner()
        .spawn_pty(
            spec(
                launch(&["spawn-group-child", pids.to_str().unwrap(), "hang"]),
                folder.path(),
            ),
            SIZE,
        )
        .await
        .unwrap();
    let mut reaper = Reaper::new();
    let parent = terminal.control.pid();
    reaper.hold(parent);
    reaper.hold(sibling.control.pid());
    until(&mut terminal.output, "ready").await;
    let children = read_pids(&pids).await;
    for pid in &children {
        reaper.hold(*pid);
    }
    drop(terminal.control);
    assert!(wait_gone(parent, GONE_WITHIN).await);
    for child in children {
        assert!(wait_gone(child, GONE_WITHIN).await);
    }
    assert!(sibling.control.try_exited().is_none());
    sibling.control.shutdown().await;
    reaper.assert_gone().await;
}

#[tokio::test]
async fn stalled_terminal_output_does_not_block_force_cleanup() {
    let folder = tempfile::tempdir().unwrap();
    let terminal = spawner()
        .spawn_pty(
            spec(launch(&["json-lines", "200000", "1024"]), folder.path()),
            SIZE,
        )
        .await
        .unwrap();
    let mut reaper = Reaper::new();
    reaper.hold(terminal.control.pid());
    tokio::time::sleep(Duration::from_millis(50)).await;
    tokio::time::timeout(STARTUP, terminal.control.shutdown())
        .await
        .unwrap();
    assert!(wait_gone(terminal.control.pid(), GONE_WITHIN).await);
    reaper.assert_gone().await;
}

#[tokio::test]
async fn invalid_size_fails_before_start() {
    let folder = tempfile::tempdir().unwrap();
    assert!(
        spawner()
            .spawn_pty(
                spec(launch(&["hang"]), folder.path()),
                PtySize { rows: 0, cols: 80 }
            )
            .await
            .is_err()
    );
}
