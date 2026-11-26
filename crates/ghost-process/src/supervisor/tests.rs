use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};

use super::*;
use crate::{
    EnvSource, ExitInfo, TopKeys,
    testing::{FakeAgentEnd, FakeLine, FakeSpawner},
};

fn request() -> LaunchRequest {
    LaunchRequest {
        protocol: PROTOCOL_VERSION,
        program: "/usr/bin/agent".into(),
        args: vec!["stdio".into()],
        cwd: "/workspace".into(),
        preparation: None,
        set_env: environment::LaunchEnvironment::default(),
        daemons: Vec::new(),
        max_line_bytes: 1024,
    }
}

fn context(spawner: Arc<dyn ProcessSpawner>) -> SupervisorContext {
    SupervisorContext {
        spawner,
        env: Arc::new(ResolvedEnv {
            vars: Default::default(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        }),
        redactor: Arc::new(Redactor::standard()),
        log: ProtocolLog::disabled(),
        shutdown: None,
    }
}

struct Rig {
    input: WriteHalf<DuplexStream>,
    output: ReadHalf<DuplexStream>,
    task: JoinHandle<io::Result<()>>,
    agent: FakeAgentEnd,
    spawner: Arc<FakeSpawner>,
}

async fn start() -> Rig {
    start_with_shutdown(None).await
}

async fn start_with_shutdown(shutdown: Option<tokio::sync::watch::Receiver<bool>>) -> Rig {
    start_with_request(request(), shutdown).await
}

async fn start_with_request(
    request: LaunchRequest,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
) -> Rig {
    let (spawner, mut agents) = FakeSpawner::new();
    let (client, server) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(server);
    let mut context = context(spawner.clone());
    context.shutdown = shutdown;
    let task = tokio::spawn(serve_with_lease(
        reader,
        writer,
        context,
        Duration::from_secs(3),
    ));
    let (mut output, mut input) = tokio::io::split(client);
    wire::write_frame(
        &mut input,
        &Frame::json(Tag::Launch, &request).expect("frame"),
    )
    .await
    .expect("launch");
    let ready = wire::read_frame(&mut output)
        .await
        .expect("read")
        .expect("ready");
    assert_eq!(ready.tag, Tag::Ready);
    let ready: Ready = ready.decode().expect("ready body");
    assert_eq!(ready.protocol, PROTOCOL_VERSION);
    let agent = agents.recv().await.expect("agent");
    Rig {
        input,
        output,
        task,
        agent,
        spawner,
    }
}

#[tokio::test(start_paused = true)]
async fn launch_preserves_overrides_and_redacts_their_secret_values() {
    let mut request = request();
    request.set_env = environment::LaunchEnvironment(vec![
        ("CLAUDE_CODE_SDK_READS_SESSION_STATE".into(), "1".into()),
        ("CLAUDE_CODE_EXECUTABLE".into(), "/home/test/agent".into()),
        (
            "PRIVATE_TOKEN".into(),
            "a-private-test-override-value".into(),
        ),
    ]);
    let expected = request.set_env.clone();
    let mut rig = start_with_request(request, None).await;
    assert_eq!(
        environment::LaunchEnvironment::try_from(&rig.agent.spec.launch).expect("overrides"),
        expected
    );
    rig.agent
        .stderr
        .send("a-private-test-override-value".into())
        .expect("stderr");
    // Let the fake stderr pump feed its tail before requesting cleanup.
    tokio::task::yield_now().await;
    wire::write_frame(&mut rig.input, &Frame::empty(Tag::Stop))
        .await
        .expect("stop");
    let ended = finish(&mut rig).await;
    assert!(!ended.stderr_tail.contains("a-private-test-override-value"));
    assert!(
        ended.stderr_tail.contains("<SECRET>"),
        "{}",
        ended.stderr_tail
    );
    rig.task.await.expect("join").expect("serve");
}

#[tokio::test(start_paused = true)]
async fn endpoint_termination_uses_the_owned_shutdown_chain() {
    let (stop, signal) = tokio::sync::watch::channel(false);
    let mut rig = start_with_shutdown(Some(signal)).await;
    stop.send_replace(true);
    finish(&mut rig).await;
    assert!(
        rig.spawner.controls()[0]
            .calls()
            .iter()
            .any(|call| matches!(call, crate::testing::FakeCall::Shutdown(_)))
    );
    rig.task.await.expect("join").expect("serve");
}

#[tokio::test(start_paused = true)]
async fn stalled_output_does_not_block_parent_lease_cleanup() {
    let rig = start().await;
    for _ in 0..100 {
        rig.agent
            .to_client
            .send(FakeLine::Line(format!(
                "{{\"text\":\"{}\"}}",
                "x".repeat(1024)
            )))
            .expect("stdout");
    }
    tokio::time::advance(Duration::from_secs(4)).await;
    // Keep the read side open but deliberately do not drain it.
    tokio::time::timeout(Duration::from_secs(12), rig.task)
        .await
        .expect("bounded cleanup")
        .expect("join")
        .expect("serve");
    assert!(rig.spawner.controls()[0].was_killed());
}

async fn finish(rig: &mut Rig) -> Ended {
    loop {
        let frame = wire::read_frame(&mut rig.output)
            .await
            .expect("read")
            .expect("ended");
        if frame.tag == Tag::Ended {
            return frame.decode().expect("ended body");
        }
    }
}

#[tokio::test(start_paused = true)]
async fn forwards_lines_and_oversized_mandatory_keys_without_json_reencoding() {
    let mut rig = start().await;
    let line = r#"{"request":"visible"}"#;
    wire::write_frame(
        &mut rig.input,
        &Frame {
            tag: Tag::Stdin,
            payload: line.as_bytes().to_vec(),
        },
    )
    .await
    .expect("stdin");
    assert_eq!(rig.agent.from_client.recv().await.as_deref(), Some(line));
    rig.agent
        .to_client
        .send(FakeLine::Line(line.into()))
        .expect("stdout");
    let echoed = wire::read_frame(&mut rig.output)
        .await
        .expect("read")
        .expect("line");
    assert_eq!(echoed.tag, Tag::Stdout);
    assert_eq!(echoed.payload, line.as_bytes());
    rig.agent
        .to_client
        .send(FakeLine::Oversized {
            bytes: 9999,
            keys: TopKeys {
                id: Some(serde_json::json!(7)),
                method: Some("permission".into()),
                kind: None,
            },
        })
        .expect("oversized");
    let frame = wire::read_frame(&mut rig.output)
        .await
        .expect("read")
        .expect("oversized");
    assert_eq!(frame.tag, Tag::Oversized);
    let body: Oversized = frame.decode().expect("body");
    assert_eq!(body.id, Some(serde_json::json!(7)));
    assert_eq!(body.method.as_deref(), Some("permission"));
    wire::write_frame(&mut rig.input, &Frame::empty(Tag::Stop))
        .await
        .expect("stop");
    finish(&mut rig).await;
    rig.task.await.expect("join").expect("serve");
}

#[tokio::test(start_paused = true)]
async fn a_missing_parent_heartbeat_stops_the_owned_process() {
    let mut rig = start().await;
    tokio::time::advance(Duration::from_secs(4)).await;
    let ended = finish(&mut rig).await;
    assert_eq!(ended.exit.and_then(|exit| exit.signal), Some(9));
    assert!(rig.spawner.controls()[0].was_killed());
    rig.task.await.expect("join").expect("serve");
}

#[tokio::test(start_paused = true)]
async fn parent_eof_stops_the_agent_even_while_its_stdin_is_open() {
    let mut rig = start().await;
    rig.input.shutdown().await.expect("disconnect");
    finish(&mut rig).await;
    assert!(rig.spawner.controls()[0].was_killed());
    rig.task.await.expect("join").expect("serve");
}

#[tokio::test(start_paused = true)]
async fn stop_discards_following_prompts_and_never_launches_a_second_process() {
    let mut rig = start().await;
    wire::write_frame(&mut rig.input, &Frame::empty(Tag::Stop))
        .await
        .expect("stop");
    wire::write_frame(
        &mut rig.input,
        &Frame {
            tag: Tag::Stdin,
            payload: b"must not run".to_vec(),
        },
    )
    .await
    .expect("queued line");
    finish(&mut rig).await;
    assert!(rig.agent.from_client.try_recv().is_err());
    assert_eq!(rig.spawner.controls().len(), 1);
    rig.task.await.expect("join").expect("serve");
}

#[tokio::test(start_paused = true)]
async fn malformed_and_duplicate_launch_frames_fail_closed() {
    for duplicate in [false, true] {
        let mut rig = start().await;
        if duplicate {
            wire::write_frame(
                &mut rig.input,
                &Frame::json(Tag::Launch, &request()).expect("frame"),
            )
            .await
            .expect("duplicate");
        } else {
            rig.input
                .write_all(&[255, 0, 0, 0, 0])
                .await
                .expect("invalid tag");
        }
        finish(&mut rig).await;
        assert_eq!(rig.spawner.controls().len(), 1);
        assert!(rig.spawner.controls()[0].was_killed());
        rig.task.await.expect("join").expect("serve");
    }
}

#[tokio::test(start_paused = true)]
async fn buffered_output_precedes_exit_and_stderr_is_redacted() {
    let mut rig = start().await;
    rig.agent
        .stderr
        .send("Authorization: Bearer abcdefghijklmnopqrstuvwxyz123456".into())
        .expect("stderr");
    rig.agent
        .to_client
        .send(FakeLine::Line(r#"{"last":true}"#.into()))
        .expect("last");
    rig.agent.to_client.send(FakeLine::Eof).expect("eof");
    let (replacement, _) = tokio::sync::oneshot::channel();
    let exit = std::mem::replace(&mut rig.agent.exit, replacement);
    exit.send(ExitInfo {
        code: Some(0),
        signal: None,
        at_ms: 1,
    })
    .expect("exit");
    let last = wire::read_frame(&mut rig.output)
        .await
        .expect("read")
        .expect("last");
    assert_eq!(last.tag, Tag::Stdout);
    let ended = finish(&mut rig).await;
    assert_eq!(ended.exit.and_then(|exit| exit.code), Some(0));
    assert!(
        !ended
            .stderr_tail
            .contains("abcdefghijklmnopqrstuvwxyz123456")
    );
    rig.task.await.expect("join").expect("serve");
}

#[tokio::test]
async fn oversized_control_header_is_refused_without_reading_its_payload() {
    let (mut tx, mut rx) = tokio::io::duplex(5);
    tx.write_all(&[Tag::Launch as u8, 0x7f, 0xff, 0xff, 0xff])
        .await
        .expect("header");
    assert!(wire::read_frame(&mut rx).await.is_err());
}

#[test]
fn launch_validation_rejects_shell_relative_paths_and_unbounded_daemon_match() {
    let valid = request();
    valid.validate().expect("valid");
    let mut invalid = valid.clone();
    invalid.program = "agent".into();
    assert!(invalid.validate().is_err());
    let mut invalid = valid.clone();
    invalid.cwd = "C:\\project".into();
    assert!(invalid.validate().is_err());
    let mut invalid = valid.clone();
    invalid.protocol += 1;
    assert!(invalid.validate().is_err());
    let mut invalid = valid;
    invalid.daemons.push(crate::DaemonMatch {
        program: None,
        args_in_order: Vec::new(),
    });
    assert!(invalid.validate().is_err());
}

#[tokio::test(start_paused = true)]
async fn preparation_token_from_another_connection_never_spawns() {
    let (spawner, mut agents) = FakeSpawner::new();
    let (mut client, server) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(server);
    let task = tokio::spawn(serve(reader, writer, context(spawner)));
    let mut request = request();
    request.preparation = Some(uuid::Uuid::now_v7().to_string());
    wire::write_frame(
        &mut client,
        &Frame::json(Tag::Launch, &request).expect("frame"),
    )
    .await
    .expect("write");
    assert!(task.await.expect("join").is_err());
    assert!(agents.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn heartbeat_and_stop_before_launch_never_spawn() {
    let (spawner, mut agents) = FakeSpawner::new();
    let (mut client, server) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(server);
    let task = tokio::spawn(serve(reader, writer, context(spawner)));
    wire::write_frame(&mut client, &Frame::empty(Tag::Heartbeat))
        .await
        .expect("heartbeat");
    wire::write_frame(&mut client, &Frame::empty(Tag::Stop))
        .await
        .expect("stop");
    task.await.expect("join").expect("stopped");
    assert!(agents.try_recv().is_err());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn inspected_launch_uses_one_environment_and_rejects_a_changed_realm() {
    use inspect::{InspectRequest, Prepared};
    for change in ["none", "realm", "program"] {
        let folder = tempfile::tempdir().expect("folder");
        let (spawner, mut agents) = FakeSpawner::new();
        let (mut client, server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(server);
        let mut context = context(spawner);
        Arc::make_mut(&mut context.env).vars.insert(
            crate::EnvKey::from("HOME"),
            folder.path().as_os_str().to_owned(),
        );
        Arc::make_mut(&mut context.env)
            .vars
            .insert(crate::EnvKey::from("PATH"), "/usr/bin:/bin".into());
        let task = tokio::spawn(serve(reader, writer, context));
        let inspection = InspectRequest {
            protocol: PROTOCOL_VERSION,
            cwd: folder.path().to_string_lossy().into_owned(),
            set_env: Default::default(),
            executables: vec!["sh".into()],
        };
        wire::write_frame(
            &mut client,
            &Frame::json(Tag::Inspect, &inspection).expect("frame"),
        )
        .await
        .expect("inspect");
        let frame = wire::read_frame(&mut client)
            .await
            .expect("read")
            .expect("inspection");
        assert_eq!(frame.tag, Tag::Inspection);
        let prepared: Prepared = frame.decode().expect("prepared");
        let mut launch = request();
        launch.program = prepared.inspection.executables[0].launch_path.clone();
        launch.cwd = prepared.inspection.canonical_cwd.clone();
        launch.preparation = Some(prepared.preparation);
        launch
            .set_env
            .0
            .push(("CLAUDE_CODE_SDK_READS_SESSION_STATE".into(), "1".into()));
        if change == "realm" {
            launch
                .set_env
                .0
                .push(("CLAUDE_CONFIG_DIR".into(), "/other-config".into()));
        }
        if change == "program" {
            launch.program = "/uninspected/agent".into();
        }
        wire::write_frame(
            &mut client,
            &Frame::json(Tag::Launch, &launch).expect("frame"),
        )
        .await
        .expect("launch");
        if change != "none" {
            assert!(task.await.expect("join").is_err());
            assert!(agents.try_recv().is_err());
        } else {
            let ready = wire::read_frame(&mut client)
                .await
                .expect("read")
                .expect("ready");
            assert_eq!(ready.tag, Tag::Ready);
            let agent = agents.recv().await.expect("agent");
            assert_eq!(
                agent.spec.env.vars[&crate::EnvKey::from("HOME")],
                folder.path().as_os_str()
            );
            wire::write_frame(&mut client, &Frame::empty(Tag::Stop))
                .await
                .expect("stop");
            while wire::read_frame(&mut client).await.expect("read").is_some() {}
            task.await.expect("join").expect("serve");
        }
    }
}
