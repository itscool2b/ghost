//! The replay harness (feature `replay`): real ACP and Claude drivers over `FakeSpawner` pipes against a
//! fake agent playing a curated protocol log, on paused time, with the collected events and replies
//! compared with a golden file.
//!
//! [`run`] takes the `SessionSetup` from the fixture's `start` note (folder `<CWD>`, standard
//! timings) and spawns through `FakeSpawner` with `ProtocolLog::disabled_with_id(<the start note's
//! spawn_id>)`, so the ids the driver mints outside a turn match the recording. Then it walks the
//! records: `cmd` sends the matching `DriverCommand` (replies collected); `in` writes the line to the
//! driver, a response's `id` rewritten to the live id of the request it answers; `out` reads the
//! next line the driver wrote and compares it; `err` goes to the fake stderr; `agent` records act.
//! After each record, a 1 ms sleep lets every task go idle, so interleaving is deterministic. When
//! the records end, the command channel closes and the fake agent exits once its stdin closes.
//!
//! Call it on a current-thread runtime with paused time: `#[tokio::test(start_paused = true)]`, or
//! a runtime built with `start_paused(true)`. A long replay then takes no wall-clock time.

pub mod curate;
pub mod fake_agent;
pub mod fixture;
pub mod golden;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use ghost_process::testing::FakeSpawner;
use ghost_process::{
    EnvMap, EnvSource, LaunchSpec, ProcessCwd, ProcessSpawner, ProtocolLog, ResolvedEnv,
    ShutdownPlan, SpawnError, SpawnSpec, SpawnedProcess,
};
use ghost_protocol::{Autonomy, GhostEvent};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

pub use curate::{CurateError, curate, scan_clean};
pub use fixture::{AgentAction, Fixture, FixtureError, FixtureRecord};
pub use golden::{assert_golden, golden_path};

use crate::acp::AcpDriver;
use crate::claude::ClaudeDriver;
use crate::codex::CodexDriver;
use crate::driver::{
    AgentDriver, AgentSpawner, DriverCommand, DriverContext, DriverEvent, DriverTimings,
    EventReceiver, LoggedCommand, NativeStartOptions, PermissionOutcome, SessionSetup, StopPlan,
    TurnRef, event_channel,
};
use fake_agent::FakeAgent;

/// The chat folder replays run in, as curated fixtures spell it.
pub const REPLAY_FOLDER: &str = "<CWD>";

/// The client version replays advertise.
pub const REPLAY_CLIENT_VERSION: &str = "0.0.0-replay";

/// Stable new-session identity when curation replaced the recorded Claude UUID with a placeholder.
const REPLAY_NATIVE_SESSION_ID: &str = "019a0000-0000-7000-8000-000000000001";

fn curated_session_id(id: &str) -> bool {
    id.strip_prefix("<SESSION_")
        .and_then(|rest| rest.strip_suffix('>'))
        .is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}
fn curated_session_uuid(id: &str) -> Option<String> {
    let number = id
        .strip_prefix("<SESSION_")?
        .strip_suffix('>')?
        .parse::<u64>()
        .ok()?;
    (number <= 0xffff_ffff_ffff).then(|| format!("019a0000-0000-7000-8000-{number:012x}"))
}

/// How long, in virtual time, a replay may take after its last record before it is given up.
pub const REPLAY_END_LIMIT: Duration = Duration::from_secs(30 * 60);

/// What a replay produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplayReport {
    /// Every `DriverEvent`, as JSON, thought durations blanked.
    pub events: Vec<Value>,
    /// The replies to `set_option` and `set_autonomy` commands, in command order.
    pub replies: Vec<Value>,
    /// How `run` ended, exit times blanked.
    pub exit: Value,
    /// Where the driver's output differed from the fixture; empty when it matched.
    pub mismatches: Vec<String>,
}

/// Spawns through a `FakeSpawner` with the recorded spawn id.
struct ReplaySpawner {
    base: Arc<FakeSpawner>,
    spawn_id: String,
}

impl AgentSpawner for ReplaySpawner {
    fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        self.base
            .spawn(spec, ProtocolLog::disabled_with_id(self.spawn_id.clone()))
    }
}

/// A launch that looks like the catalog's for the agents Ghost knows: the dialect's sign-in
/// commands read it.
fn replay_launch(agent_id: &str) -> LaunchSpec {
    let (program, args, set_env): (&str, &[&str], &[(&str, &str)]) = match agent_id {
        "claude" => ("/opt/agents/bin/claude", &[], &[]),
        "codex" => (
            "/opt/agents/bin/codex",
            &["app-server", "--listen", "stdio://"],
            &[],
        ),
        "pi" => ("/opt/agents/bin/pi", &[], &[]),
        "hermes" => ("/opt/agents/bin/hermes", &["acp"], &[]),
        "claude-acp" => (
            "/opt/agents/bin/node",
            &["/opt/agents/adapters/claude-agent-acp/dist/index.js"],
            &[("CLAUDE_CODE_EXECUTABLE", "/opt/agents/bin/claude")],
        ),
        "codex-acp" => (
            "/opt/agents/bin/node",
            &["/opt/agents/adapters/codex-acp/dist/index.js"],
            &[("CODEX_PATH", "/opt/agents/bin/codex")],
        ),
        _ => ("/opt/agents/bin/acp-agent", &[], &[]),
    };
    LaunchSpec {
        label: agent_id.to_owned(),
        program: PathBuf::from(program),
        args: args.iter().map(Into::into).collect(),
        cwd: ProcessCwd::ChatFolder,
        set_env: set_env
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect(),
        daemons: Vec::new(),
        startup_timeout: Duration::from_secs(60),
    }
}

/// The session setup a `start` note describes.
pub fn setup_from_start(start: &Value) -> Result<SessionSetup, String> {
    let text = |key: &str| {
        start
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("the start note has no \"{key}\""))
    };
    let agent_id = text("agent_id")?;
    let pi = agent_id == "pi" || start["dialect"] == "pi";
    let native = pi || agent_id == "claude" || start["dialect"] == "claude";
    let autonomy: Autonomy = match start.get("autonomy") {
        Some(value) => {
            serde_json::from_value(value.clone()).map_err(|error| format!("autonomy: {error}"))?
        }
        None => Autonomy::default(),
    };
    let desired = match start.get("desired") {
        Some(value) if !value.is_null() => {
            serde_json::from_value(value.clone()).map_err(|error| format!("desired: {error}"))?
        }
        _ => BTreeMap::new(),
    };
    let resume = start.get("resume").and_then(Value::as_str).map(|id| {
        if native {
            curated_session_uuid(id).unwrap_or_else(|| id.to_owned())
        } else {
            id.to_owned()
        }
    });
    let mut native_start: NativeStartOptions = match start.get("native_start") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("native_start: {error}"))?,
        None => NativeStartOptions::default(),
    };
    if native
        && (resume.is_none() || native_start.fork)
        && native_start
            .new_session_id
            .as_deref()
            .is_none_or(curated_session_id)
    {
        let recorded = native_start
            .new_session_id
            .as_deref()
            .or_else(|| start["native_session_id"].as_str());
        native_start.new_session_id = Some(
            recorded
                .and_then(|id| {
                    uuid::Uuid::parse_str(id)
                        .ok()
                        .map(|id| id.to_string())
                        .or_else(|| curated_session_uuid(id))
                })
                .unwrap_or_else(|| REPLAY_NATIVE_SESSION_ID.into()),
        );
    }
    Ok(SessionSetup {
        mcp_servers: Vec::new(),
        attachment_dir: None,
        attachment_writer: None,
        chat_id: "replay".into(),
        launch: replay_launch(if pi {
            "pi"
        } else if native {
            "claude"
        } else {
            &agent_id
        }),
        dialect: text("dialect")?,
        agent_id,
        folder: PathBuf::from(REPLAY_FOLDER),
        agent_folder: REPLAY_FOLDER.into(),
        add_dirs: Vec::new(),
        env: Arc::new(ResolvedEnv {
            vars: EnvMap::new(),
            source: EnvSource::Fallback {
                reason: "replay".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        }),
        resume,
        native_start,
        autonomy,
        desired,
        timings: DriverTimings::STANDARD,
        client_version: REPLAY_CLIENT_VERSION.into(),
    })
}

/// A typed control reply on its way.
enum ReplyWait {
    Steer(String, oneshot::Receiver<crate::driver::SteerResult>),
    Option(
        String,
        oneshot::Receiver<Result<(), crate::driver::DriverError>>,
    ),
    Autonomy(
        Autonomy,
        oneshot::Receiver<Result<ghost_protocol::AutonomyState, crate::driver::DriverError>>,
    ),
    Rewind(
        String,
        bool,
        oneshot::Receiver<Result<crate::driver::FileRewindResult, crate::driver::DriverError>>,
    ),
}

/// Runs the fixture's real driver over fake pipes. The start's agent or dialect selects Claude;
/// existing ACP fixtures retain their original driver and comparison rules.
pub async fn run(fixture: &Fixture) -> ReplayReport {
    let mut report = ReplayReport::default();
    let Some(start) = fixture.start() else {
        report
            .mismatches
            .push("the fixture has no start note".into());
        return report;
    };
    let mut setup = match setup_from_start(start) {
        Ok(setup) => setup,
        Err(reason) => {
            report.mismatches.push(reason);
            return report;
        }
    };
    if setup.agent_id == "codex" || setup.dialect == "codex" {
        // Keep strict native payload comparison while reproducing the recorded client identity.
        if let Some(version) = fixture.records.iter().find_map(|record| match record {
            FixtureRecord::Out(message) if message["method"] == "initialize" => message
                .pointer("/params/clientInfo/version")
                .and_then(Value::as_str),
            _ => None,
        }) {
            setup.client_version = version.into();
        }
    }
    let spawn_id = start
        .get("spawn_id")
        .and_then(Value::as_str)
        .unwrap_or("replay-spawn")
        .to_owned();
    let (spawner, mut ends) = FakeSpawner::new();
    let (commands, receiver) = mpsc::unbounded_channel();
    let (sender, events) = event_channel();
    let pi = setup.agent_id == "pi" || setup.dialect == "pi";
    let native = pi || setup.agent_id == "claude" || setup.dialect == "claude";
    let native_session = setup
        .native_start
        .new_session_id
        .clone()
        .map(|id| uuid::Uuid::parse_str(&id).map_or(id, |uuid| uuid.to_string()))
        .or_else(|| setup.resume.clone())
        .unwrap_or_default();
    let ctx = DriverContext {
        commands: receiver,
        events: sender,
        spawner: Arc::new(ReplaySpawner {
            base: spawner,
            spawn_id,
        }),
        setup,
    };
    let codex = ctx.setup.agent_id == "codex" || ctx.setup.dialect == "codex";
    let driver: Box<dyn AgentDriver> = if codex {
        Box::new(CodexDriver::new())
    } else if pi {
        Box::new(crate::pi::PiDriver::new())
    } else if native {
        Box::new(ClaudeDriver::new())
    } else {
        Box::new(AcpDriver::new())
    };
    let driver = tokio::spawn(driver.run(ctx));
    let collected = Arc::new(Mutex::new(Vec::new()));
    let collector = tokio::spawn(collect(events, Arc::clone(&collected)));
    let end = match timeout(Duration::from_secs(5), ends.recv()).await {
        Ok(Some(end)) => end,
        _ => {
            report
                .mismatches
                .push("the driver never spawned its agent".into());
            driver.abort();
            return report;
        }
    };
    let mut agent = FakeAgent::new(end);
    agent.strict_jsonrpc(codex);
    if native && let Some(recorded) = start["native_session_id"].as_str() {
        agent.native_session(recorded, &native_session);
    }
    let mut commands = Some(commands);
    let mut replies = Vec::new();
    for (index, record) in fixture.records.iter().enumerate() {
        match record {
            FixtureRecord::Start(_) => {}
            FixtureRecord::Note(note) => {
                // A recorded exit is the agent's exit when no agent record said so.
                if let Some(exit) = note.get("exit")
                    && !agent.exited()
                {
                    let code = exit
                        .get("code")
                        .and_then(Value::as_i64)
                        .and_then(|code| i32::try_from(code).ok());
                    agent.exit(code);
                }
            }
            FixtureRecord::Out(expected) => agent.expect_out(index, expected).await,
            FixtureRecord::In(message) => agent.write_in(index, message),
            FixtureRecord::InRaw(raw) => agent.write_raw(raw),
            FixtureRecord::Err(line) => agent.stderr(line),
            FixtureRecord::Agent(action) => agent.act(action).await,
            FixtureRecord::Cmd(logged) => {
                let Some(sender) = &commands else {
                    agent.mismatches.push(format!(
                        "record {}: a command after the session ended",
                        index + 1
                    ));
                    continue;
                };
                let (command, wait) = command_for(logged);
                if let Some(wait) = wait {
                    replies.push(wait);
                }
                let closes = matches!(command, DriverCommand::Shutdown { .. });
                if sender.send(command).is_err() {
                    agent.mismatches.push(format!(
                        "record {}: the driver no longer reads commands",
                        index + 1
                    ));
                }
                if closes {
                    commands = None;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    agent.unexpected_lines();
    drop(commands);
    agent.serve_until_closed(REPLAY_END_LIMIT).await;
    let exit = match timeout(REPLAY_END_LIMIT, driver).await {
        Ok(Ok(exit)) => blank_exit_times(serde_json::to_value(exit).unwrap_or(Value::Null)),
        Ok(Err(error)) => {
            agent
                .mismatches
                .push(format!("the driver task failed: {error}"));
            Value::Null
        }
        Err(_) => {
            agent.mismatches.push("the driver did not return".into());
            Value::Null
        }
    };
    let _ = collector.await;
    agent.mismatches.extend(agent.unanswered());
    report.events = collected
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain(..)
        .collect();
    for wait in replies {
        report.replies.push(reply_json(wait).await);
    }
    report.exit = exit;
    report.mismatches = agent.mismatches;
    report
}

/// The command a `cmd` record stands for, with its reply receiver.
fn command_for(logged: &LoggedCommand) -> (DriverCommand, Option<ReplyWait>) {
    match logged.clone() {
        LoggedCommand::Steer {
            turn_id,
            message_id,
            parts,
        } => {
            let (reply, wait) = oneshot::channel();
            (
                DriverCommand::Steer {
                    turn_id,
                    message_id: message_id.clone(),
                    parts,
                    reply,
                },
                Some(ReplyWait::Steer(message_id, wait)),
            )
        }
        LoggedCommand::Prompt { turn_id, parts } => (
            DriverCommand::Prompt {
                turn: TurnRef { turn_id },
                parts,
            },
            None,
        ),
        LoggedCommand::Interrupt => (DriverCommand::Interrupt, None),
        LoggedCommand::EditPlan { request_id, plan } => {
            (DriverCommand::EditPlan { request_id, plan }, None)
        }
        LoggedCommand::RespondPermission {
            request_id,
            option_id,
        } => (
            DriverCommand::RespondPermission {
                request_id,
                outcome: option_id
                    .map_or(PermissionOutcome::Cancelled, PermissionOutcome::Selected),
            },
            None,
        ),
        LoggedCommand::RespondUi { request_id, answer } => {
            (DriverCommand::RespondUi { request_id, answer }, None)
        }
        LoggedCommand::SetOption { option_id, value } => {
            let (reply, wait) = oneshot::channel();
            (
                DriverCommand::SetOption {
                    option_id: option_id.clone(),
                    value,
                    reply,
                },
                Some(ReplyWait::Option(option_id, wait)),
            )
        }
        LoggedCommand::SetAutonomy { autonomy } => {
            let (reply, wait) = oneshot::channel();
            (
                DriverCommand::SetAutonomy { autonomy, reply },
                Some(ReplyWait::Autonomy(autonomy, wait)),
            )
        }
        LoggedCommand::McpAction {
            server_name,
            action,
        } => {
            let (reply, _wait) = oneshot::channel();
            (
                DriverCommand::McpAction {
                    server_name,
                    action,
                    reply,
                },
                None,
            )
        }
        LoggedCommand::RewindFiles {
            user_message_id,
            dry_run,
        } => {
            let (reply, wait) = oneshot::channel();
            (
                DriverCommand::RewindFiles {
                    user_message_id: user_message_id.clone(),
                    dry_run,
                    reply,
                },
                Some(ReplyWait::Rewind(user_message_id, dry_run, wait)),
            )
        }
        // The session's Stop: 3 s for a running turn's cancel, then the standard chain.
        LoggedCommand::Shutdown => (
            DriverCommand::Shutdown {
                plan: StopPlan {
                    cancel_grace: Duration::from_secs(3),
                    process: ShutdownPlan::STANDARD,
                },
            },
            None,
        ),
    }
}

async fn reply_json(wait: ReplyWait) -> Value {
    match wait {
        ReplyWait::Steer(message_id, wait) => {
            let reply = wait
                .await
                .map_or(json!("dropped"), |outcome| json!(outcome));
            json!({"steer": message_id, "reply": reply})
        }
        ReplyWait::Option(option_id, wait) => {
            let reply = match wait.await {
                Ok(Ok(())) => json!({"ok": null}),
                Ok(Err(error)) => json!({"err": error}),
                Err(_) => json!("dropped"),
            };
            json!({"set_option": option_id, "reply": reply})
        }
        ReplyWait::Autonomy(autonomy, wait) => {
            let reply = match wait.await {
                Ok(Ok(state)) => json!({"ok": state}),
                Ok(Err(error)) => json!({"err": error}),
                Err(_) => json!("dropped"),
            };
            json!({"set_autonomy": autonomy, "reply": reply})
        }
        ReplyWait::Rewind(user_message_id, dry_run, wait) => {
            let reply = match wait.await {
                Ok(Ok(result)) => json!({"ok": result}),
                Ok(Err(error)) => json!({"err": error}),
                Err(_) => json!("dropped"),
            };
            json!({"rewind_files": user_message_id, "dry_run": dry_run, "reply": reply})
        }
    }
}

/// Collects every event until the driver drops its sender.
async fn collect(mut events: EventReceiver, into: Arc<Mutex<Vec<Value>>>) {
    while let Some(event) = events.recv().await {
        let value = serde_json::to_value(blank_durations(event)).unwrap_or(Value::Null);
        into.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(value);
    }
}

/// Thought durations depend on timing alone; goldens leave them out.
fn blank_durations(event: DriverEvent) -> DriverEvent {
    match event {
        DriverEvent::Event {
            turn,
            event: GhostEvent::ThoughtCompleted { thought_id, .. },
        } => DriverEvent::Event {
            turn,
            event: GhostEvent::ThoughtCompleted {
                thought_id,
                duration_ms: None,
            },
        },
        other => other,
    }
}

/// Exit times are wall-clock; goldens leave them out.
fn blank_exit_times(mut exit: Value) -> Value {
    if let Some(at) = exit.pointer_mut("/data/exit/at_ms") {
        *at = json!(0);
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = r#"{"d":"note","note":{"start":{"agent_id":"acp-agent","dialect":"generic","resume":null,"autonomy":"full_auto","desired":{},"spawn_id":"019a0000-0000-7000-8000-00000000aaaa"}}}"#;

    fn fixture(lines: &[&str]) -> Fixture {
        Fixture::parse("test", &lines.join("\n")).expect("parses")
    }

    #[test]
    fn native_start_reuses_a_recorded_uuid_and_has_a_stable_curated_fallback() {
        let mut start = json!({"agent_id":"claude","dialect":"claude","native_session_id":"019a0000-0000-7000-8000-000000000009"});
        let setup = setup_from_start(&start).expect("native setup");
        assert_eq!(
            setup.native_start.new_session_id.as_deref(),
            start["native_session_id"].as_str()
        );
        assert_eq!(setup.chat_id, "replay");
        assert_eq!(
            setup.launch.program,
            PathBuf::from("/opt/agents/bin/claude")
        );
        start["native_session_id"] = json!("<SESSION_1>");
        assert_eq!(
            setup_from_start(&start)
                .expect("curated setup")
                .native_start
                .new_session_id
                .as_deref(),
            Some(REPLAY_NATIVE_SESSION_ID)
        );
        start["agent_id"] = json!("custom-claude");
        assert_eq!(
            setup_from_start(&start)
                .expect("dialect selects native")
                .launch
                .program,
            PathBuf::from("/opt/agents/bin/claude")
        );
    }

    #[test]
    fn the_start_note_becomes_the_setup() {
        let start = serde_json::json!({"agent_id": "hermes", "dialect": "hermes", "resume": "s-1",
            "autonomy": "ask", "desired": {"model": {"type": "choice", "value": "p:a"}}, "spawn_id": "x"});
        let setup = setup_from_start(&start).expect("setup");
        assert_eq!(
            (setup.agent_id.as_str(), setup.dialect.as_str()),
            ("hermes", "hermes")
        );
        assert_eq!(setup.resume.as_deref(), Some("s-1"));
        assert_eq!(setup.autonomy, Autonomy::Ask);
        assert_eq!(setup.desired.len(), 1);
        assert_eq!(setup.agent_folder, REPLAY_FOLDER);
        assert_eq!(setup.launch.args, ["acp"]);
        assert_eq!(setup.native_start, NativeStartOptions::default());
        assert!(setup_from_start(&serde_json::json!({"dialect": "x"})).is_err());
    }

    #[test]
    fn native_fork_replay_seeds_the_new_id_and_preserves_the_source() {
        let source = "019a0000-0000-7000-8000-000000000009";
        let fork = "019a0000-0000-7000-8000-000000000008";
        let message = "019a0000-0000-7000-8000-000000000007";
        let mut start = json!({"agent_id":"claude","dialect":"claude","resume":source,"native_session_id":fork,
            "native_start":{"fork":true,"resume_at":message,"file_checkpointing":true}});
        let setup = setup_from_start(&start).expect("fork setup");
        assert_eq!(setup.resume.as_deref(), Some(source));
        assert_eq!(setup.native_start.new_session_id.as_deref(), Some(fork));
        assert_eq!(setup.native_start.resume_at.as_deref(), Some(message));
        assert!(setup.native_start.fork && setup.native_start.file_checkpointing);
        assert_eq!(
            setup_from_start(&setup.start_note("replay"))
                .expect("round trip")
                .native_start,
            setup.native_start
        );
        start
            .as_object_mut()
            .expect("start object")
            .remove("native_start");
        start["native_session_id"] = json!(source);
        let resumed = setup_from_start(&start).expect("resume setup");
        assert_eq!(resumed.resume.as_deref(), Some(source));
        assert_eq!(resumed.native_start.new_session_id, None);
        start["resume"] = json!("<SESSION_2>");
        start["native_session_id"] = json!("<SESSION_2>");
        let resumed = setup_from_start(&start).expect("curated resume setup");
        assert_eq!(
            resumed.resume.as_deref(),
            Some("019a0000-0000-7000-8000-000000000002")
        );
        start["native_start"] = json!({"fork":true,"new_session_id":"<SESSION_3>"});
        let fork = setup_from_start(&start).expect("curated fork setup");
        assert_eq!(
            fork.native_start.new_session_id.as_deref(),
            Some("019a0000-0000-7000-8000-000000000003")
        );
        assert_ne!(fork.native_start.new_session_id, fork.resume);
    }

    #[tokio::test(start_paused = true)]
    async fn a_curated_native_fork_replays_with_deterministic_ids() {
        let mut fixture = Fixture::parse(
            "fork",
            include_str!("../../tests/fixtures/claude/basic.jsonl"),
        )
        .expect("fixture");
        for record in &mut fixture.records {
            if let FixtureRecord::Start(start) = record {
                start["resume"] = json!("<SESSION_2>");
                start["native_start"] = json!({"fork":true,"new_session_id":"<SESSION_1>","resume_at":"019a0000-0000-7000-8000-000000000007"});
            }
        }
        let first = run(&fixture).await;
        let second = run(&fixture).await;
        assert!(first.mismatches.is_empty(), "{:?}", first.mismatches);
        assert!(second.mismatches.is_empty(), "{:?}", second.mismatches);
        assert_eq!(first.events, second.events);
        assert_eq!(first.exit, second.exit);
        assert!(first.events.iter().any(|event| matches!(serde_json::from_value::<DriverEvent>(event.clone()),
            Ok(DriverEvent::Ready(info)) if info.native_session_id.as_deref() == Some(REPLAY_NATIVE_SESSION_ID) && info.resumed == crate::driver::ResumeOutcome::New)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_short_session_replays_without_mismatches() {
        let report = run(&fixture(&[
            START,
            r#"{"d":"out","msg":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}}"#,
            r#"{"d":"in","msg":{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}}"#,
            r#"{"d":"out","msg":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}}"#,
            r#"{"d":"in","msg":{"jsonrpc":"2.0","id":2,"result":{"sessionId":"<SESSION_1>"}}}"#,
            r#"{"d":"cmd","cmd":{"type":"prompt","turn_id":"T1","parts":[{"type":"text","text":"hi"}]}}"#,
            r#"{"d":"out","msg":{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"<SESSION_1>","prompt":[{"type":"text","text":"hi"}]}}}"#,
            r#"{"d":"in","msg":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"<SESSION_1>","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ok"}}}}}"#,
            r#"{"d":"in","msg":{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}}"#,
        ]))
        .await;
        assert!(report.mismatches.is_empty(), "{:?}", report.mismatches);
        assert_eq!(report.exit, serde_json::json!({"type": "closed"}));
        let events: Vec<DriverEvent> = report
            .events
            .iter()
            .map(|event| serde_json::from_value(event.clone()).expect("an event"))
            .collect();
        assert!(events.iter().any(
            |event| matches!(event, DriverEvent::TurnEnded { turn_id, .. } if turn_id == "T1")
        ));
        assert!(events.iter().any(|event| matches!(event, DriverEvent::Ready(info) if info.spawn_id == "019a0000-0000-7000-8000-00000000aaaa")));
    }

    #[tokio::test(start_paused = true)]
    async fn differences_are_reported_record_by_record() {
        let report = run(&fixture(&[
            START,
            r#"{"d":"out","msg":{"jsonrpc":"2.0","id":1,"method":"session/new","params":{}}}"#,
            r#"{"d":"in","msg":{"jsonrpc":"2.0","id":9,"result":{}}}"#,
            r#"{"d":"out","msg":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}}"#,
        ]))
        .await;
        assert_eq!(report.mismatches.len(), 3, "{:?}", report.mismatches);
        assert!(report.mismatches[0].starts_with("record 2: expected"));
        assert!(report.mismatches[1].contains("never sent"));
        assert!(report.mismatches[2].contains("the driver wrote nothing"));
    }

    #[tokio::test(start_paused = true)]
    async fn an_unanswered_agent_request_breaks_rule_three_unless_the_agent_died() {
        // The driver answers everything it forwards, so a missing answer shows only when the
        // harness itself never lets the reply out: here the agent dies first, which is allowed.
        let report = run(&fixture(&[
            START,
            r#"{"d":"out","msg":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}}"#,
            r#"{"d":"in","msg":{"jsonrpc":"2.0","id":"q1","method":"session/request_permission","params":{"toolCall":{"toolCallId":"t"},"options":[]}}}"#,
            r#"{"d":"agent","agent":{"exit":{"code":1}}}"#,
            r#"{"d":"agent","agent":"eof"}"#,
        ]))
        .await;
        assert!(report.mismatches.is_empty(), "{:?}", report.mismatches);
        assert!(matches!(
            report.exit.get("type").and_then(Value::as_str),
            Some("start_failed")
        ));
    }

    #[test]
    fn exits_lose_their_times() {
        let exit = blank_exit_times(
            serde_json::json!({"type": "crashed", "data": {"exit": {"code": 1, "signal": null, "at_ms": 1234}, "stderr_tail": ""}}),
        );
        assert_eq!(exit["data"]["exit"]["at_ms"], 0);
    }
}
