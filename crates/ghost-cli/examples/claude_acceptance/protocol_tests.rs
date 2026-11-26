use super::*;
use ghost_process::testing::{FakeAgentEnd, FakeLine, FakeSpawner};
use ghost_process::{EnvMap, EnvSource, ExitInfo, LaunchSpec, ProcessCwd, ResolvedEnv};
use serde_json::Value;
use tokio::time::timeout;

const NATIVE: &str = "019a0000-0000-7000-8000-000000000001";

fn setup(folder: &Path) -> SessionSetup {
    SessionSetup {
        mcp_servers: Vec::new(),
        attachment_dir: None,
        attachment_writer: None,
        chat_id: new_id(),
        agent_id: "claude".into(),
        dialect: "claude".into(),
        folder: folder.into(),
        agent_folder: folder.to_string_lossy().into_owned(),
        add_dirs: Vec::new(),
        launch: LaunchSpec {
            label: "claude".into(),
            program: "/fake/claude".into(),
            args: Vec::new(),
            cwd: ProcessCwd::ChatFolder,
            set_env: Vec::new(),
            daemons: Vec::new(),
            startup_timeout: Duration::from_secs(60),
        },
        env: Arc::new(ResolvedEnv {
            vars: EnvMap::new(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        }),
        resume: Some(NATIVE.into()),
        native_start: NativeStartOptions::default(),
        autonomy: Autonomy::Ask,
        desired: BTreeMap::new(),
        timings: DriverTimings::STANDARD,
        client_version: "test".into(),
    }
}

struct FakeCheck {
    task: tokio::task::JoinHandle<Result<Summary, String>>,
    end: FakeAgentEnd,
    owned: Arc<OwnedSpawner>,
}

fn send(end: &FakeAgentEnd, frame: Value) {
    end.to_client
        .send(FakeLine::Line(frame.to_string()))
        .expect("stdout");
}

async fn wire(end: &mut FakeAgentEnd) -> Option<Value> {
    loop {
        let text = timeout(Duration::from_secs(2), end.from_client.recv())
            .await
            .expect("wire deadline")?;
        let frame: Value = serde_json::from_str(&text).expect("wire JSON");
        if matches!(
            frame["request"]["subtype"].as_str(),
            Some("get_usage" | "get_context_usage")
        ) {
            send(
                end,
                json!({"type":"control_response","response":{"subtype":"success","request_id":frame["request_id"],"response":{}}}),
            );
        } else {
            return Some(frame);
        }
    }
}

impl FakeCheck {
    async fn start(folder: &Path, thinking: bool) -> Self {
        let (base, mut ends) = FakeSpawner::new();
        let owned = Arc::new(OwnedSpawner {
            base,
            logs: LogFactory::new(
                folder.join("logs"),
                Arc::new(Redactor::standard()),
                tokio::runtime::Handle::current(),
            ),
            name: "fake-acceptance".into(),
            controls: Arc::new(Mutex::new(Vec::new())),
            paths: Arc::new(Mutex::new(Vec::new())),
        });
        let task = tokio::spawn(session(
            setup(folder),
            owned.clone(),
            Some(NATIVE),
            thinking,
        ));
        let mut end = timeout(Duration::from_secs(2), ends.recv())
            .await
            .expect("spawn deadline")
            .expect("spawn");
        let init = wire(&mut end).await.expect("initialize");
        assert_eq!(init["request"]["subtype"], "initialize");
        send(
            &end,
            json!({"type":"control_response","response":{"subtype":"success","request_id":init["request_id"],"response":{"current_permission_mode":"default","session_state":"idle","models":[],"commands":[]}}}),
        );
        let prompt = wire(&mut end).await.expect("prompt");
        assert_eq!(prompt["type"], "user");
        assert_eq!(
            prompt["message"]["content"],
            if thinking { THINKING_PROMPT } else { "/usage" }
        );
        Self { task, end, owned }
    }

    fn result(&self) {
        send(
            &self.end,
            json!({"type":"result","subtype":"success","is_error":false,"result":"local result","session_id":NATIVE,"stop_reason":"end_turn"}),
        );
    }

    fn idle(&self) {
        send(
            &self.end,
            json!({"type":"system","subtype":"session_state_changed","state":"idle"}),
        );
    }

    async fn finish(mut self) -> Result<Summary, String> {
        // Completion must close stdin, never issue another user prompt.
        assert!(
            wire(&mut self.end).await.is_none(),
            "unexpected additional command"
        );
        self.end.to_client.send(FakeLine::Eof).expect("EOF");
        self.end
            .exit
            .send(ExitInfo {
                code: Some(0),
                signal: None,
                at_ms: 0,
            })
            .expect("exit");
        timeout(Duration::from_secs(3), self.task)
            .await
            .expect("check deadline")
            .expect("check task")
    }
}

#[tokio::test]
async fn local_command_waits_for_run_idle_and_retains_curatable_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut check = FakeCheck::start(dir.path(), false).await;
    check.result();
    assert!(
        timeout(Duration::from_millis(40), wire(&mut check.end))
            .await
            .is_err(),
        "result alone must not shut down the still-busy run"
    );
    assert!(!check.task.is_finished());
    check.idle();
    let summary = check.finish().await.expect("local check");
    assert_eq!(summary.native_session_id, NATIVE);
    assert_eq!(summary.prompt, "/usage");
    assert_eq!(summary.logs.len(), 1);
    let log = logs::read(&summary.logs[0]).expect("redacted log");
    assert!(logs::exchange(&log.records, "initialize").is_some());
    assert!(logs::note(&log.records, "exit").is_some());
    assert_eq!(
        log.records
            .iter()
            .filter_map(logs::message)
            .filter(|message| message["type"] == "user")
            .count(),
        1
    );
}

#[tokio::test]
async fn thinking_without_delta_fails_after_one_prompt_and_stops() {
    let dir = tempfile::tempdir().expect("tempdir");
    let check = FakeCheck::start(dir.path(), true).await;
    let owned = check.owned.clone();
    check.result();
    check.idle();
    let error = check.finish().await.expect_err("missing thinking");
    assert!(error.contains("No retry was sent"));
    assert!(
        lock(&owned.controls)
            .iter()
            .all(|control| control.try_exited().is_some())
    );
    assert_eq!(lock(&owned.paths).len(), 1);
}

#[tokio::test]
async fn thinking_delta_is_recorded_without_a_second_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let check = FakeCheck::start(dir.path(), true).await;
    send(
        &check.end,
        json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}),
    );
    send(
        &check.end,
        json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Check the remainders."}}}),
    );
    check.result();
    check.idle();
    let summary = check.finish().await.expect("thinking check");
    assert!(summary.thought_bytes > 0);
    let log = logs::read(&summary.logs[0]).expect("log");
    assert!(
        log.records
            .iter()
            .filter_map(logs::message)
            .any(|frame| frame["event"]["delta"]["type"] == "thinking_delta")
    );
}

#[tokio::test]
async fn withheld_thinking_text_passes_with_lifecycle_and_reports_token_evidence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let check = FakeCheck::start(dir.path(), true).await;
    for event in [
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"","estimated_tokens":302}}),
        json!({"type":"content_block_stop","index":0}),
    ] {
        send(&check.end, json!({"type":"stream_event","event":event}));
    }
    send(
        &check.end,
        json!({
            "type":"result","subtype":"success","is_error":false,"result":"local result",
            "session_id":NATIVE,"stop_reason":"end_turn",
            "usage":{"input_tokens":12,"output_tokens":305,"output_tokens_details":{"thinking_tokens":302}}
        }),
    );
    check.idle();
    let summary = check.finish().await.expect("withheld thinking check");
    assert_eq!(summary.thought_bytes, 0);
    assert_eq!(summary.thoughts_completed, 1);
    assert_eq!(summary.reasoning_tokens, Some(302));
}
