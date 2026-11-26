use super::*;
use ghost_drivers::driver::LoggedCommand;
use ghost_protocol::{PromptPart, memory::SaveProjectMemory};

async fn project_for(rig: &Rig, chat: &str) -> String {
    let (reply, answer) = oneshot::channel();
    let chat = chat.to_owned();
    rig.store.with_store(move |store| {
        let id = store
            .conn()
            .query_row("SELECT project_id FROM chats WHERE id=?1", [chat], |r| {
                r.get::<_, String>(0)
            })
            .unwrap();
        let _ = reply.send(id);
    });
    answer.await.unwrap()
}
fn text(command: LoggedCommand) -> String {
    let LoggedCommand::Prompt { parts, .. } = command else {
        panic!("prompt")
    };
    parts
        .iter()
        .filter_map(|part| match part {
            PromptPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
#[tokio::test]
async fn visible_memory_attaches_to_real_turn_once_and_changed_revision_reaches_next_queued_turn() {
    let (driver, handle) = ScriptedDriver::new(vec![
        ready_step("memory-native"),
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Interrupt),
        Step::EndTurn {
            stop_reason: StopReason::Cancelled,
            usage: None,
            error: None,
        },
        Step::Expect(CommandMatch::Prompt),
        Step::Expect(CommandMatch::Interrupt),
        Step::EndTurn {
            stop_reason: StopReason::Cancelled,
            usage: None,
            error: None,
        },
        Step::Expect(CommandMatch::Prompt),
        Step::Hang,
    ]);
    let rig = rig(vec![driver], WriterConfig::STANDARD, Knobs::STANDARD);
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let project = project_for(&rig, &chat.id).await;
    let first = rig
        .manager
        .save_project_memory(SaveProjectMemory {
            project_id: project.clone(),
            expected_revision: None,
            markdown: "Accepted durable fact one".into(),
            reason: "Initial note".into(),
        })
        .await
        .unwrap();
    rig.manager
        .send(&chat.id, "First real request".into())
        .await
        .unwrap();
    let first_sent = text(handle.wait_for(CommandMatch::Prompt).await);
    assert!(first_sent.starts_with("First real request"));
    assert!(first_sent.contains("Accepted durable fact one"));
    assert!(first_sent.contains("instruction files take precedence"));
    // While the first real turn is running, the next request waits. Memory is chosen at dispatch.
    rig.manager
        .send(&chat.id, "Queued real request".into())
        .await
        .unwrap();
    let second = rig
        .manager
        .save_project_memory(SaveProjectMemory {
            project_id: project.clone(),
            expected_revision: first.revision,
            markdown: "Accepted durable fact two".into(),
            reason: "Updated note while turn runs".into(),
        })
        .await
        .unwrap();
    rig.manager.interrupt(&chat.id).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.resume_queue(&chat.id).await.unwrap();
    let second_sent = text(handle.wait_for(CommandMatch::Prompt).await);
    assert!(second_sent.contains("Queued real request"));
    assert!(second_sent.contains("Accepted durable fact two"));
    assert!(!second_sent.contains("Accepted durable fact one"));
    rig.manager
        .send(&chat.id, "Third real request".into())
        .await
        .unwrap();
    rig.manager.interrupt(&chat.id).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    rig.manager.resume_queue(&chat.id).await.unwrap();
    let third_sent = text(handle.wait_for(CommandMatch::Prompt).await);
    assert!(third_sent.contains("Third real request"));
    assert!(!third_sent.contains("Project memory ("));
    let (reply, answer) = oneshot::channel();
    rig.store.with_store(move |store| {
        let rows = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM project_memory_snapshots WHERE revision=?1",
                [second.revision],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        let _ = reply.send(rows);
    });
    assert_eq!(answer.await.unwrap(), 2);
    rig.manager.force_stop(&chat.id).await.unwrap();
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

struct ObserveMemoryGrant {
    inner: ScriptedDriver,
    seen: Arc<Mutex<Option<(String, String)>>>,
}
impl AgentDriver for ObserveMemoryGrant {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        *lock(&self.seen) = ctx
            .setup
            .mcp_servers
            .iter()
            .find(|server| server.definition.id == "ghost-project-memory")
            .and_then(|server| {
                Some((
                    server.credentials.env.get("GHOST_MEMORY_ENDPOINT")?.clone(),
                    server.credentials.env.get("GHOST_MEMORY_TOKEN")?.clone(),
                ))
            });
        Box::new(self.inner).run(ctx)
    }
}
#[tokio::test]
async fn stopping_native_generation_revokes_its_memory_helper_service() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("memory-stop"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let seen = Arc::new(Mutex::new(None));
    let rig = rig_with(
        Arc::new(OneDriver(Mutex::new(Some(Box::new(ObserveMemoryGrant {
            inner: driver,
            seen: seen.clone(),
        }))))),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let (endpoint, _token) = lock(&seen)
        .clone()
        .expect("managed memory helper is part of this native generation");
    assert!(tokio::net::TcpStream::connect(&endpoint).await.is_ok());
    rig.manager.stop(&chat.id).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Stopped).await;
    tokio::task::yield_now().await;
    assert!(tokio::net::TcpStream::connect(&endpoint).await.is_err());
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

async fn call_memory(
    endpoint: &str,
    token: &str,
    mut value: serde_json::Value,
) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(endpoint).await.unwrap();
    value["token"] = serde_json::json!(token);
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    stream.write_all(&bytes).await.unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(stream)
        .read_line(&mut line)
        .await
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn the_profile_switch_reaches_a_running_generations_memory_writes_at_once() {
    let (driver, _) = ScriptedDriver::new(vec![
        ready_step("memory-switch"),
        Step::Expect(CommandMatch::Shutdown),
        Step::Exit(DriverExit::Closed),
    ]);
    let seen = Arc::new(Mutex::new(None));
    let rig = rig_with(
        Arc::new(OneDriver(Mutex::new(Some(Box::new(ObserveMemoryGrant {
            inner: driver,
            seen: seen.clone(),
        }))))),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let project = project_for(&rig, &chat.id).await;
    let (endpoint, token) = lock(&seen).clone().unwrap();
    assert!(rig.manager.agent_memory_writes(), "on by default");
    let written = call_memory(
        &endpoint,
        &token,
        serde_json::json!({"operation":"write","base_revision":null,"markdown":"Agent fact","reason":"Learned it"}),
    )
    .await;
    assert_eq!(written["result"]["markdown"], "Agent fact");
    let history = rig.manager.project_memory_history(&project).await.unwrap();
    assert_eq!(history[0].source, format!("agent:{}", chat.id));
    rig.manager.set_agent_memory_writes(false);
    let refused = call_memory(
        &endpoint,
        &token,
        serde_json::json!({"operation":"write","base_revision":written["result"]["revision"],"markdown":"Second","reason":"Off now"}),
    )
    .await;
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("turned off agents' edits")
    );
    assert_eq!(
        rig.manager
            .read_project_memory(&project)
            .await
            .unwrap()
            .markdown,
        "Agent fact"
    );
    rig.manager.stop(&chat.id).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Stopped).await;
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn crashing_and_failed_native_generations_revoke_memory_credentials() {
    for (exit, wanted) in [
        (
            DriverExit::Crashed {
                exit: None,
                stderr_tail: String::new(),
            },
            SessionStatus::Stopped,
        ),
        (
            DriverExit::StartFailed(ghost_drivers::driver::DriverError {
                kind: ghost_protocol::ErrorKind::SpawnFailed,
                message: "Synthetic failure".into(),
                detail: None,
            }),
            SessionStatus::Failed,
        ),
    ] {
        let (driver, handle) = ScriptedDriver::new(vec![ready_step("memory-crash")]);
        let seen = Arc::new(Mutex::new(None));
        let rig = rig_with(
            Arc::new(OneDriver(Mutex::new(Some(Box::new(ObserveMemoryGrant {
                inner: driver,
                seen: seen.clone(),
            }))))),
            WriterConfig::STANDARD,
            Knobs::STANDARD,
        );
        let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
        status(&rig, &chat.id, SessionStatus::Ready).await;
        let (endpoint, token) = lock(&seen).clone().unwrap();
        assert!(!token.is_empty());
        assert!(tokio::net::TcpStream::connect(&endpoint).await.is_ok());
        handle.exit(exit);
        status(&rig, &chat.id, wanted).await;
        tokio::task::yield_now().await;
        assert!(tokio::net::TcpStream::connect(&endpoint).await.is_err());
        rig.manager.shutdown_all(Duration::from_secs(5)).await;
    }
}
#[tokio::test]
async fn cancelling_pre_dispatch_snapshot_keeps_live_generations_memory_tools() {
    let (driver, handle) = ScriptedDriver::new(vec![ready_step("memory-cancel-snapshot")]);
    let seen = Arc::new(Mutex::new(None));
    let rig = rig_with(
        Arc::new(OneDriver(Mutex::new(Some(Box::new(ObserveMemoryGrant {
            inner: driver,
            seen: seen.clone(),
        }))))),
        WriterConfig::STANDARD,
        Knobs::STANDARD,
    );
    let chat = rig.manager.create_chat("mock", &rig.folder).await.unwrap();
    status(&rig, &chat.id, SessionStatus::Ready).await;
    let (endpoint, _token) = lock(&seen).clone().unwrap();
    let (release, blocked) = std::sync::mpsc::channel::<()>();
    let (entered, waiting) = oneshot::channel();
    rig.store.with_store(move |_| {
        let _ = entered.send(());
        let _ = blocked.recv_timeout(Duration::from_secs(10));
    });
    waiting.await.unwrap();
    rig.manager
        .send(&chat.id, "Hold before native send".into())
        .await
        .unwrap();
    rig.manager.interrupt(&chat.id).await.unwrap();
    drop(release);
    status(&rig, &chat.id, SessionStatus::Ready).await;
    assert!(tokio::net::TcpStream::connect(&endpoint).await.is_ok());
    assert!(
        !handle
            .commands()
            .iter()
            .any(|command| matches!(command, LoggedCommand::Prompt { .. }))
    );
    rig.manager.force_stop(&chat.id).await.unwrap();
    rig.manager.shutdown_all(Duration::from_secs(5)).await;
}
