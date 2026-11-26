//! Linux ownership checks use only Ghost's test child, never installed agent binaries.
#![cfg(target_os = "linux")]
// The rig's helpers are not tests themselves; a failed expectation there fails the test, as below.
#![allow(clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Reaper, STARTUP, alive, read_pids, testbin};
use ghost_process::supervisor::{
    SupervisorContext, serve,
    wire::{self, Ended, Frame, LaunchRequest, Ready, Tag},
};
use ghost_process::{DaemonMatch, LeftoverRegistry, ProtocolLog, Redactor, Spawner};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

struct Rig {
    input: WriteHalf<DuplexStream>,
    output: ReadHalf<DuplexStream>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    reaper: Reaper,
    descendants: Vec<u32>,
    _folder: tempfile::TempDir,
}

async fn start(mode: &str, spare_daemon: bool) -> Rig {
    let folder = tempfile::tempdir().expect("folder");
    let pidfile = folder.path().join("children.pid");
    let (client, server) = tokio::io::duplex(8192);
    let (reader, writer) = tokio::io::split(server);
    let rt = tokio::runtime::Handle::current();
    let context = SupervisorContext {
        spawner: Spawner::new(LeftoverRegistry::disabled(), rt),
        env: common::env(),
        redactor: Arc::new(Redactor::standard()),
        log: ProtocolLog::disabled(),
        shutdown: None,
    };
    let task = tokio::spawn(serve(reader, writer, context));
    let (mut output, mut input) = tokio::io::split(client);
    let request = LaunchRequest {
        protocol: wire::PROTOCOL_VERSION,
        program: testbin().to_string_lossy().into_owned(),
        args: vec![mode.into(), pidfile.to_string_lossy().into_owned()],
        cwd: folder.path().to_string_lossy().into_owned(),
        preparation: None,
        set_env: Default::default(),
        daemons: if spare_daemon {
            vec![DaemonMatch {
                program: Some("ghost-testbin".into()),
                args_in_order: vec!["daemon".into(), "run".into()],
            }]
        } else {
            Vec::new()
        },
        max_line_bytes: 1024 * 1024,
    };
    wire::write_frame(
        &mut input,
        &Frame::json(Tag::Launch, &request).expect("frame"),
    )
    .await
    .expect("launch");
    let ready = tokio::time::timeout(STARTUP, wire::read_frame(&mut output))
        .await
        .expect("startup")
        .expect("read")
        .expect("ready");
    assert_eq!(ready.tag, Tag::Ready);
    let ready: Ready = ready.decode().expect("ready body");
    let mut reaper = Reaper::new();
    reaper.hold(ready.pid);
    let descendants = read_pids(&pidfile).await;
    reaper.hold_all(&descendants);
    Rig {
        input,
        output,
        task,
        reaper,
        descendants,
        _folder: folder,
    }
}

async fn end(rig: &mut Rig) -> Ended {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let frame = wire::read_frame(&mut rig.output)
                .await
                .expect("read")
                .expect("end");
            if frame.tag == Tag::Ended {
                return frame.decode().expect("ended body");
            }
        }
    })
    .await
    .expect("bounded shutdown")
}

#[tokio::test]
async fn a_stopped_supervisor_kills_its_owned_group_and_escaped_children() {
    for mode in ["spawn-group-child", "spawn-pty-child"] {
        let mut rig = start(mode, false).await;
        wire::write_frame(&mut rig.input, &Frame::empty(Tag::Stop))
            .await
            .expect("stop");
        let ended = end(&mut rig).await;
        assert!(ended.leftovers.is_empty());
        assert!(ended.spared.is_empty());
        rig.task.await.expect("join").expect("serve");
        rig.reaper.assert_gone().await;
    }
}

#[tokio::test]
async fn a_supervisor_preserves_the_declared_daemon_subtree() {
    let mut rig = start("spawn-setsid-daemon", true).await;
    wire::write_frame(&mut rig.input, &Frame::empty(Tag::Stop))
        .await
        .expect("stop");
    let ended = end(&mut rig).await;
    assert!(ended.leftovers.is_empty());
    for pid in &rig.descendants {
        assert!(ended.spared.contains(pid));
        assert!(alive(*pid), "declared test daemon stays running");
    }
    rig.task.await.expect("join").expect("serve");
    // Reaper kills only the test processes, after the preservation assertion.
}

#[tokio::test]
async fn losing_the_windows_side_of_the_transport_stops_linux_descendants() {
    let rig = start("spawn-pty-child", false).await;
    drop(rig.input);
    drop(rig.output);
    tokio::time::timeout(Duration::from_secs(12), rig.task)
        .await
        .expect("bounded loss cleanup")
        .expect("join")
        .expect("serve");
    rig.reaper.assert_gone().await;
}

#[test]
fn endpoint_version_is_available_without_a_launch_or_environment_resolution() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ghost-host-supervisor"))
        .arg("--version")
        .env_clear()
        .output()
        .expect("version");
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("protocol {}", wire::PROTOCOL_VERSION))
    );
    assert!(output.stderr.is_empty());
}
