//! Opt-in native Claude checks. Run with --help first. Unit tests use only FakeSpawner.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::{FutureExt, future::BoxFuture};
use ghost_agents::Detector;
use ghost_cli::logs;
use ghost_core::ids::new_id;
use ghost_drivers::claude::ClaudeDriver;
use ghost_drivers::driver::{
    AgentDriver, AgentSpawner, DriverCommand, DriverContext, DriverEvent, DriverExit,
    DriverTimings, EventReceiver, NativeStartOptions, PermissionOutcome, SessionSetup, StopPlan,
    TurnRef, event_channel,
};
use ghost_process::{
    EnvResolver, EnvResolverConfig, LeftoverRegistry, LogFactory, ProcessControl, ProcessSpawner,
    Redactor, ShutdownPlan, SpawnError, SpawnSpec, SpawnedProcess, Spawner,
};
use ghost_protocol::session::ConfigValue;
use ghost_protocol::{Autonomy, GhostEvent, PromptPart, StopReason};
use serde_json::json;
use tokio::sync::mpsc;

const START_LIMIT: Duration = Duration::from_secs(90);
const TURN_LIMIT: Duration = Duration::from_secs(180);
const STOP_LIMIT: Duration = Duration::from_secs(15);
const THINKING_PROMPT: &str = "Without using tools, find the smallest positive integer n such that n is 2 modulo 7, 5 modulo 11, and 7 modulo 13. Check the result mentally, then reply with only n.";
const HELP: &str = r#"Native Claude acceptance checks, using the production driver and redacted protocol logs.

cargo run -p ghost-cli --example claude_acceptance -- <case> [options]

Cases:
  resume      Continue a stopped smoke session from a different, fresh cwd.
  fork        Fork that session into a chosen UUID, then reopen the fork.
  resume-at   Fork through a native message UUID, then reopen that fork.
  thinking    One fresh Sonnet medium turn, only with --spend. No retries.

Required for every case:
  --out PATH           New output directory for redacted logs and summary.json.
  --cwd PATH           New or empty target directory, kept after the check.

Required for resume, fork, and resume-at:
  --source-session UUID  Session produced by smoke. Stop its process first.
  --source-cwd PATH      Existing original smoke directory, different from --cwd.

Required for fork and resume-at:
  --new-session UUID     Chosen new transcript UUID, different from the source.
Required for resume-at:
  --message UUID         Native message UUID from the source's recorded protocol log.
Required for thinking:
  --spend                Authorize exactly one fixed prompt on sonnet, medium effort.

The three free cases print and send only the visible local command /usage. They verify startup
identity, command completion, run-idle, and fork reopening. They do not claim a semantic memory
check or inspect vendor transcripts/configuration. Resume-at always forks to preserve the source.
Thinking prints its exact prompt, sets one turn and a $0.25 CLI budget, and requires a completed
thinking block. The agent may withhold its text; the summary records text bytes and reasoning tokens
separately. Any permission or question is denied and fails the check. No automatic retries.
Every owned process is stopped; logs and fresh directories are retained for fixture curation.
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Case {
    Resume,
    Fork,
    ResumeAt,
    Thinking,
}

#[derive(Debug)]
struct Args {
    case: Case,
    out: PathBuf,
    cwd: PathBuf,
    source: Option<String>,
    source_cwd: Option<PathBuf>,
    new_session: Option<String>,
    message: Option<String>,
}

fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

impl Args {
    fn parse(words: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut words = words.into_iter();
        let Some(case) = words.next() else {
            return Ok(None);
        };
        let case = match case.as_str() {
            "--help" | "-h" => return Ok(None),
            "resume" => Case::Resume,
            "fork" => Case::Fork,
            "resume-at" => Case::ResumeAt,
            "thinking" => Case::Thinking,
            _ => return Err("Choose resume, fork, resume-at, or thinking; see --help.".into()),
        };
        let mut flags = BTreeMap::new();
        let mut spend = false;
        while let Some(flag) = words.next() {
            if flag == "--spend" {
                if spend {
                    return Err("--spend was repeated.".into());
                }
                spend = true;
                continue;
            }
            if !matches!(
                flag.as_str(),
                "--out"
                    | "--cwd"
                    | "--source-session"
                    | "--source-cwd"
                    | "--new-session"
                    | "--message"
            ) {
                return Err(format!("Unknown option {flag}; see --help."));
            }
            let value = words
                .next()
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| format!("{flag} needs a value."))?;
            if flags.insert(flag.clone(), value).is_some() {
                return Err(format!("{flag} was repeated."));
            }
        }
        if (case == Case::Thinking) != spend {
            return Err("Only thinking accepts --spend, and thinking requires it.".into());
        }
        let mut take = |flag: &str| {
            flags
                .remove(flag)
                .ok_or_else(|| format!("{flag} is required."))
        };
        let out = take("--out")?.into();
        let cwd = take("--cwd")?.into();
        let source = if case != Case::Thinking {
            Some(take("--source-session")?.to_ascii_lowercase())
        } else {
            None
        };
        let source_cwd = if case != Case::Thinking {
            Some(PathBuf::from(take("--source-cwd")?))
        } else {
            None
        };
        let new_session = if matches!(case, Case::Fork | Case::ResumeAt) {
            Some(take("--new-session")?.to_ascii_lowercase())
        } else {
            None
        };
        let message = if case == Case::ResumeAt {
            Some(take("--message")?.to_ascii_lowercase())
        } else {
            None
        };
        if !flags.is_empty() {
            return Err("An option does not apply to the selected case.".into());
        }
        if [&source, &new_session, &message]
            .into_iter()
            .flatten()
            .any(|id| !uuid(id))
        {
            return Err("Session and message IDs must be hyphenated UUIDs.".into());
        }
        if source
            .as_deref()
            .zip(new_session.as_deref())
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Err("A fork must use a different UUID from its source.".into());
        }
        Ok(Some(Self {
            case,
            out,
            cwd,
            source,
            source_cwd,
            new_session,
            message,
        }))
    }

    fn native_start(&self) -> NativeStartOptions {
        NativeStartOptions {
            new_session_id: self.new_session.clone(),
            fork: matches!(self.case, Case::Fork | Case::ResumeAt),
            resume_at: self.message.clone(),
            ..Default::default()
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The same production spawner and logs as the app, with ownership retained for timeout cleanup.
struct OwnedSpawner {
    base: Arc<dyn ProcessSpawner>,
    logs: LogFactory,
    name: String,
    controls: Arc<Mutex<Vec<ProcessControl>>>,
    paths: Arc<Mutex<Vec<PathBuf>>>,
}

impl AgentSpawner for OwnedSpawner {
    fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        let base = Arc::clone(&self.base);
        let log = self.logs.open(&self.name, &new_id());
        let controls = Arc::clone(&self.controls);
        let paths = Arc::clone(&self.paths);
        async move {
            let process = base.spawn(spec, log).await?;
            lock(&controls).push(process.control.clone());
            if let Some(path) = process.log.path() {
                lock(&paths).push(path);
            }
            Ok(process)
        }
        .boxed()
    }
}

impl OwnedSpawner {
    fn kill(&self) {
        for control in lock(&self.controls).iter() {
            control.kill_tree();
        }
    }
}

impl Drop for OwnedSpawner {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug, serde::Serialize)]
struct Summary {
    native_session_id: String,
    prompt_id: String,
    prompt: String,
    thought_bytes: usize,
    thoughts_completed: usize,
    reasoning_tokens: Option<u64>,
    reply: String,
    logs: Vec<PathBuf>,
}

async fn next(
    events: &mut EventReceiver,
    deadline: tokio::time::Instant,
) -> Result<DriverEvent, String> {
    tokio::time::timeout_at(deadline, events.recv())
        .await
        .map_err(|_| "Claude did not reach the expected state before the deadline.".to_owned())?
        .ok_or_else(|| {
            "Claude exited before the expected state; inspect the redacted protocol log.".into()
        })
}

fn reject_request(
    command: &mpsc::UnboundedSender<DriverCommand>,
    event: &DriverEvent,
) -> Result<(), String> {
    match event {
        DriverEvent::Permission { request, .. } => {
            let _ = command.send(DriverCommand::RespondPermission {
                request_id: request.request_id.clone(),
                outcome: PermissionOutcome::Cancelled,
            });
            Err("An unexpected permission was denied. This check does not approve tools.".into())
        }
        DriverEvent::UiRequest { request, .. } => {
            let _ = command.send(DriverCommand::RespondUi {
                request_id: request.request_id.clone(),
                answer: None,
            });
            Err("An unexpected question was dismissed.".into())
        }
        DriverEvent::AuthRequired(_) => Err(
            "Claude requires sign-in; use its own CLI, then run this check explicitly again."
                .into(),
        ),
        _ => Ok(()),
    }
}

async fn exercise(
    commands: &mpsc::UnboundedSender<DriverCommand>,
    events: &mut EventReceiver,
    expected: Option<&str>,
    thinking: bool,
) -> Result<Summary, String> {
    let deadline = tokio::time::Instant::now() + START_LIMIT;
    let mut idle = false;
    let mut native = None;
    while native.is_none() || !idle {
        let event = next(events, deadline).await?;
        reject_request(commands, &event)?;
        match event {
            DriverEvent::RunState { idle: state } => idle = state,
            DriverEvent::Ready(info) => native = info.native_session_id,
            _ => {}
        }
    }
    let native = native.ok_or("Claude did not report a native identity.")?;
    if expected.is_some_and(|expected| expected != native) {
        return Err("Claude opened a different native session ID from the requested one.".into());
    }
    let prompt = if thinking { THINKING_PROMPT } else { "/usage" };
    println!(
        "SEND {}: {prompt}",
        if thinking {
            "one paid Sonnet medium prompt, CLI budget $0.25"
        } else {
            "visible local command, no model prompt"
        }
    );
    let prompt_id = new_id();
    commands
        .send(DriverCommand::Prompt {
            turn: TurnRef {
                turn_id: prompt_id.clone(),
            },
            parts: vec![PromptPart::Text {
                text: prompt.into(),
            }],
        })
        .map_err(|_| "Claude closed before the prompt was sent.".to_owned())?;
    idle = false;
    let mut ended = false;
    let mut thought_bytes = 0usize;
    let mut thoughts_completed = 0usize;
    let mut reasoning_tokens = None;
    let mut reply = String::new();
    let deadline = tokio::time::Instant::now() + TURN_LIMIT;
    while !ended || !idle {
        let event = next(events, deadline).await?;
        reject_request(commands, &event)?;
        match event {
            DriverEvent::RunState { idle: state } => idle = state,
            DriverEvent::Event {
                event: GhostEvent::ThoughtDelta { text, .. },
                ..
            } => thought_bytes = thought_bytes.saturating_add(text.len()),
            DriverEvent::Event {
                event: GhostEvent::ThoughtCompleted { .. },
                ..
            } => thoughts_completed = thoughts_completed.saturating_add(1),
            DriverEvent::Event {
                event: GhostEvent::MessageDelta { text, .. },
                ..
            } => {
                if reply.len().saturating_add(text.len()) <= 8192 {
                    reply.push_str(&text);
                }
            }
            DriverEvent::TurnEnded {
                turn_id,
                stop_reason,
                error,
                usage,
            } if turn_id == prompt_id => {
                if stop_reason != StopReason::EndTurn || error.is_some() {
                    return Err(format!(
                        "Claude ended this check with {stop_reason:?}; inspect the redacted log."
                    ));
                }
                reasoning_tokens = usage.and_then(|usage| usage.reasoning_tokens);
                ended = true;
            }
            _ => {}
        }
    }
    if thinking && thoughts_completed == 0 {
        return Err(
            "The single paid turn completed without a thinking block. No retry was sent.".into(),
        );
    }
    Ok(Summary {
        native_session_id: native,
        prompt_id,
        prompt: prompt.into(),
        thought_bytes,
        thoughts_completed,
        reasoning_tokens,
        reply,
        logs: Vec::new(),
    })
}

async fn session(
    setup: SessionSetup,
    owned: Arc<OwnedSpawner>,
    expected: Option<&str>,
    thinking: bool,
) -> Result<Summary, String> {
    let (commands, command_rx) = mpsc::unbounded_channel();
    let (events, mut receiver) = event_channel();
    let mut task = tokio::spawn(Box::new(ClaudeDriver::new()).run(DriverContext {
        commands: command_rx,
        events,
        spawner: owned.clone(),
        setup,
    }));
    let result = tokio::select! {
        result = exercise(&commands, &mut receiver, expected, thinking) => result,
        signal = tokio::signal::ctrl_c() => Err(if signal.is_ok() { "Interrupted; stopping the owned Claude process.".into() } else { "Cannot listen for interrupt signals.".into() }),
    };
    let _ = commands.send(DriverCommand::Shutdown {
        plan: StopPlan {
            cancel_grace: Duration::from_secs(2),
            process: ShutdownPlan::STANDARD,
        },
    });
    let stopped = tokio::time::timeout(STOP_LIMIT, async {
        let mut events_open = true;
        loop {
            tokio::select! {
                event = receiver.recv(), if events_open => match event {
                    Some(event) => { let _ = reject_request(&commands, &event); }
                    None => events_open = false,
                },
                exit = &mut task => break exit,
            }
        }
    })
    .await;
    if stopped.is_err() {
        owned.kill();
        task.abort();
        let _ = task.await;
        return Err(
            "Claude exceeded the shutdown deadline; its owned process tree was stopped.".into(),
        );
    }
    if !matches!(stopped, Ok(Ok(DriverExit::Closed))) && result.is_ok() {
        return Err("Claude did not close normally after the check.".into());
    }
    let controls = lock(&owned.controls).clone();
    for control in controls {
        let report = tokio::time::timeout(STOP_LIMIT, control.shutdown(ShutdownPlan::STANDARD))
            .await
            .map_err(|_| "Owned process cleanup exceeded its deadline.".to_owned())?;
        if !report.leftovers.is_empty() {
            return Err("Owned child processes remain after shutdown.".into());
        }
    }
    let paths = lock(&owned.paths).clone();
    for path in &paths {
        println!("LOG {}", path.display());
    }
    let mut summary = result?;
    summary.logs = paths;
    if summary.logs.is_empty() {
        return Err("No protocol log was created.".into());
    }
    for path in &summary.logs {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(read) = logs::read(path)
                    && read.unreadable == 0
                    && logs::note(&read.records, "exit").is_some()
                {
                    if logs::note(&read.records, "dropped").is_some() {
                        return Err(
                            "The protocol log dropped records; it cannot be curated reliably.",
                        );
                    }
                    let messages: Vec<_> = read.records.iter().filter_map(logs::message).collect();
                    if !messages.iter().any(|message| message["type"] == "result") {
                        return Err("The protocol log lacks the completed result.");
                    }
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_| "The redacted protocol log did not finish writing.".to_owned())??;
    }
    Ok(summary)
}

fn fresh(path: &Path) -> Result<PathBuf, String> {
    if path.exists()
        && std::fs::read_dir(path)
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
    {
        return Err(format!("{} must be new or empty.", path.display()));
    }
    std::fs::create_dir_all(path).map_err(|error| error.to_string())?;
    std::fs::canonicalize(path).map_err(|error| error.to_string())
}

async fn execute(mut args: Args) -> Result<(), String> {
    if args.out.exists() {
        return Err("--out must name a new directory.".into());
    }
    let source_cwd = args
        .source_cwd
        .as_ref()
        .map(std::fs::canonicalize)
        .transpose()
        .map_err(|error| error.to_string())?;
    if source_cwd.as_ref().is_some_and(|path| !path.is_dir()) {
        return Err("--source-cwd must be an existing directory.".into());
    }
    args.cwd = fresh(&args.cwd)?;
    if source_cwd.as_ref() == Some(&args.cwd) {
        return Err("--cwd must differ from --source-cwd.".into());
    }
    std::fs::create_dir_all(&args.out).map_err(|error| error.to_string())?;
    args.out = std::fs::canonicalize(&args.out).map_err(|error| error.to_string())?;
    let rt = tokio::runtime::Handle::current();
    let registry = LeftoverRegistry::open(
        &ghost_cli::headless::real_data_root()
            .map_err(|error| error.to_string())?
            .join("run"),
        rt.clone(),
    )
    .map_err(|error| error.to_string())?;
    let redactor = Arc::new(Redactor::standard());
    let env = EnvResolver::start(
        EnvResolverConfig {
            redactor: Some(redactor.clone()),
            ..EnvResolverConfig::standard()
        },
        registry.clone(),
        rt.clone(),
    );
    let detector = Detector::new(
        env.clone(),
        registry.clone(),
        args.out.join("adapters"),
        rt.clone(),
    );
    // Join the detector's free startup probes before any session or registry shutdown.
    for spec in ghost_agents::catalog() {
        detector.detect(spec).await;
    }
    let work = async {
        let spec = ghost_agents::get("claude").ok_or("Native Claude is not in the catalog.")?;
        let (progress, _) = mpsc::unbounded_channel();
        let mut launch = detector
            .prepare_launch(spec, progress)
            .await
            .map_err(|error| error.to_string())?;
        launch.args.extend([
            "--max-turns".into(),
            "1".into(),
            "--max-budget-usd".into(),
            "0.25".into(),
        ]);
        let mut desired = BTreeMap::new();
        desired.insert(
            "model".into(),
            ConfigValue::Choice {
                value: if args.case == Case::Thinking {
                    "sonnet"
                } else {
                    "haiku"
                }
                .into(),
            },
        );
        if args.case == Case::Thinking {
            desired.insert(
                "effort".into(),
                ConfigValue::Choice {
                    value: "medium".into(),
                },
            );
        }
        let mut setup = SessionSetup {
            mcp_servers: Vec::new(),
            attachment_dir: None,
            attachment_writer: None,
            chat_id: new_id(),
            agent_id: "claude".into(),
            dialect: "claude".into(),
            folder: args.cwd.clone(),
            agent_folder: args.cwd.to_string_lossy().into_owned(),
            add_dirs: Vec::new(),
            launch,
            env: env.get().await,
            resume: args.source.clone(),
            native_start: args.native_start(),
            autonomy: Autonomy::Ask,
            desired,
            timings: DriverTimings::STANDARD,
            client_version: env!("CARGO_PKG_VERSION").into(),
        };
        let base: Arc<dyn ProcessSpawner> = Spawner::new(registry.clone(), rt.clone());
        let logs = LogFactory::new(args.out.join("protocol"), redactor.clone(), rt.clone());
        let make_spawner = |name: &str| {
            Arc::new(OwnedSpawner {
                base: base.clone(),
                logs: logs.clone(),
                name: name.into(),
                controls: Arc::new(Mutex::new(Vec::new())),
                paths: Arc::new(Mutex::new(Vec::new())),
            })
        };
        let expected = args.new_session.as_deref().or(args.source.as_deref());
        let first = session(
            setup.clone(),
            make_spawner("acceptance"),
            expected,
            args.case == Case::Thinking,
        )
        .await?;
        println!(
            "PASS native ID {}, result and run-idle observed; thinking blocks {}, text bytes {}, reasoning tokens {:?}",
            first.native_session_id, first.thoughts_completed, first.thought_bytes, first.reasoning_tokens
        );
        let mut summaries = vec![first];
        if matches!(args.case, Case::Fork | Case::ResumeAt) {
            setup.resume = args.new_session.clone();
            setup.native_start = NativeStartOptions::default();
            let reopened = session(
                setup,
                make_spawner("reopen"),
                args.new_session.as_deref(),
                false,
            )
            .await?;
            println!(
                "PASS fork reopened with native ID {}",
                reopened.native_session_id
            );
            summaries.push(reopened);
        }
        let mut report = json!({
            "case": format!("{:?}",args.case),
            "source_cwd": source_cwd, "cwd": args.cwd, "runs": summaries,
        });
        redactor.redact_value(&mut report);
        let report = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
        std::fs::write(args.out.join("summary.json"), report).map_err(|error| error.to_string())?;
        Ok::<_, String>(())
    }
    .await;
    let closed = registry.close().map_err(|error| error.to_string());
    work.and(closed)
        .map_err(|error| redactor.redact_text(&error).into_owned())
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let result = match Args::parse(std::env::args().skip(1)) {
        Ok(Some(args)) => execute(args).await,
        Ok(None) => {
            print!("{HELP}");
            return std::process::ExitCode::SUCCESS;
        }
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FAIL {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "claude_acceptance/protocol_tests.rs"]
mod protocol_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Args>, String> {
        Args::parse(args.iter().map(|value| (*value).to_owned()))
    }

    #[test]
    fn thinking_requires_explicit_spend_and_cannot_reuse_a_source() {
        assert!(parse(&["thinking", "--out", "logs", "--cwd", "repo"]).is_err());
        let args = parse(&["thinking", "--spend", "--out", "logs", "--cwd", "repo"])
            .expect("parse")
            .expect("args");
        assert_eq!(args.case, Case::Thinking);
        assert!(args.source.is_none());
        assert!(
            parse(&[
                "thinking",
                "--spend",
                "--out",
                "logs",
                "--cwd",
                "repo",
                "--source-session",
                "019a0000-0000-7000-8000-000000000001"
            ])
            .is_err()
        );
    }

    #[test]
    fn resume_at_is_always_a_fork_with_explicit_source_target_and_message() {
        let words = [
            "resume-at",
            "--out",
            "logs",
            "--cwd",
            "repo",
            "--source-cwd",
            "original",
            "--source-session",
            "019a0000-0000-7000-8000-000000000001",
            "--new-session",
            "019a0000-0000-7000-8000-000000000002",
            "--message",
            "019a0000-0000-7000-8000-000000000003",
        ];
        let args = parse(&words).expect("parse").expect("args");
        let native = args.native_start();
        assert!(native.fork);
        assert_eq!(native.new_session_id, args.new_session);
        assert_eq!(native.resume_at, args.message);
        let mut invalid = words.to_vec();
        invalid[10] = invalid[8];
        assert!(parse(&invalid).is_err(), "fork cannot overwrite source");
        invalid = words.to_vec();
        invalid.push("--spend");
        assert!(
            parse(&invalid).is_err(),
            "local-only case rejects paid authorization"
        );
    }
}
