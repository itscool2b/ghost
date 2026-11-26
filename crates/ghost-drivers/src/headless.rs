//! Restricted text fallbacks: agy has a persistent JSONL stdin; Grok starts one owned process per
//! visible prompt. Neither transport supports interactive approvals. Interrupt stops the owned
//! process and the session. See docs/spikes/m1d-acp.md for pinned wire evidence.

use std::{collections::VecDeque, sync::Arc, time::Duration};

use futures::{FutureExt, future::BoxFuture};
use ghost_process::{LogNote, MAX_LINE_BYTES, PIPE_EOF_GRACE, ShutdownPlan, SpawnSpec, StdoutItem};
use ghost_protocol::{
    AutoAnswer, Autonomy, AutonomyState, Capabilities, ErrorKind, GhostEvent, PromptPart,
    StopReason, TokenUsage, UsageScope,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{sync::mpsc, time::Instant};

use crate::driver::{
    AgentDriver, AgentSpawner, DriverCommand, DriverContext, DriverError, DriverEvent, DriverExit,
    EventSender, LoggedCommand, ReadyInfo, ResumeOutcome, SessionSetup,
};

#[derive(Debug, Default)]
pub struct HeadlessDriver;

impl AgentDriver for HeadlessDriver {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        run(ctx).boxed()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Agy,
    Grok,
}

fn failure(message: impl Into<String>) -> DriverError {
    DriverError {
        kind: ErrorKind::ProtocolError,
        message: message.into(),
        detail: None,
    }
}

fn autonomy() -> AutonomyState {
    AutonomyState {
        requested: Autonomy::FullAuto, native_mode: None, auto_answer: AutoAnswer::Never,
        note: Some("This fallback uses the CLI's own permission policy. It cannot ask for approval here; actions requiring approval may be denied. Stop ends the process. Use ACP for interactive approvals.".into()),
    }
}

fn unsupported(command: DriverCommand) {
    match command {
        DriverCommand::McpAction { reply, .. } => crate::mcp::unsupported(
            reply,
            "This restricted fallback does not support shared MCP or session authentication.",
        ),
        DriverCommand::SetOption { reply, .. } => {
            let _ = reply.send(Err(failure(
                "Options cannot be changed through this text fallback. Use ACP.",
            )));
        }
        DriverCommand::SetAutonomy {
            autonomy: requested,
            reply,
        } => {
            let _ = reply.send(if requested == Autonomy::FullAuto {
                Ok(autonomy())
            } else {
                Err(failure(
                    "This text fallback cannot enforce Ask or Auto-edit. Use ACP.",
                ))
            });
        }
        DriverCommand::Steer { reply, .. } => {
            let _ = reply.send(crate::driver::SteerResult::Rejected {
                message: "This agent cannot steer an exact running turn.".into(),
            });
        }
        DriverCommand::RewindFiles { reply, .. } => {
            let _ = reply.send(Err(failure("This text fallback cannot rewind files.")));
        }
        _ => {}
    }
}

async fn emit(events: &EventSender, event: DriverEvent) -> bool {
    match events.reserve(event.approx_bytes()).await {
        Ok(permit) => {
            permit.send(event);
            true
        }
        Err(_) => false,
    }
}

fn ready(native_id: Option<&str>, spawn_id: &str, family: Family, resumed: bool) -> DriverEvent {
    DriverEvent::Ready(ReadyInfo {
        native_session_id: native_id.map(str::to_owned),
        spawn_id: spawn_id.into(),
        agent_name: Some(
            if family == Family::Agy {
                "Antigravity (text fallback)"
            } else {
                "Grok Build (text fallback)"
            }
            .into(),
        ),
        agent_version: None,
        capabilities: Capabilities {
            interrupt: true,
            resume: true,
            usage: true,
            autonomy_levels: vec![Autonomy::FullAuto],
            ..Capabilities::default()
        },
        resumed: if resumed {
            ResumeOutcome::Resumed
        } else {
            ResumeOutcome::New
        },
        replay: None,
    })
}

fn text_prompt(parts: &[PromptPart]) -> Result<String, DriverError> {
    let mut texts = Vec::new();
    for part in parts {
        match part {
            PromptPart::Text { text } => texts.push(text.as_str()),
            _ => return Err(failure("This fallback accepts text only.")),
        }
    }
    let text = texts.join("\n\n");
    if text.trim().is_empty() {
        return Err(failure("An empty prompt was not sent."));
    }
    Ok(text)
}

fn ended(turn: String, error: DriverError) -> DriverEvent {
    DriverEvent::TurnEnded {
        turn_id: turn,
        stop_reason: StopReason::Error,
        usage: None,
        error: Some(error),
    }
}

async fn run(ctx: DriverContext) -> DriverExit {
    let DriverContext {
        mut commands,
        events,
        spawner,
        setup,
    } = ctx;
    let family = match setup.dialect.as_str() {
        "agy-stream" => Family::Agy,
        "grok-stream" => Family::Grok,
        _ => return DriverExit::StartFailed(failure("Unknown text fallback dialect.")),
    };
    if setup.autonomy != Autonomy::FullAuto
        || !setup.desired.is_empty()
        || !setup.add_dirs.is_empty()
        || setup.native_start != Default::default()
    {
        return DriverExit::StartFailed(failure(
            "This fallback supports only Full auto with the CLI's own settings. Options, extra folders, fork, resume-at and checkpoints require another driver.",
        ));
    }
    if setup
        .resume
        .as_ref()
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return DriverExit::StartFailed(failure("The fallback resume id must be a UUID."));
    }
    let mut native_id = setup.resume.clone();
    if family == Family::Agy {
        return run_process(
            &setup,
            family,
            &mut native_id,
            &mut commands,
            &events,
            &spawner,
            None,
        )
        .await
        .exit();
    }
    // Grok has no no-prompt handshake in headless mode. This supervisor owns its chosen native
    // UUID before any model process is started, but publishes it only after the vendor confirms
    // that a transcript exists. The process is started only by a visible Prompt.
    let id = native_id
        .get_or_insert_with(|| uuid::Uuid::now_v7().to_string())
        .clone();
    let supervisor = format!("grok-fallback-{id}");
    if !emit(&events, DriverEvent::Autonomy(autonomy())).await
        || !emit(
            &events,
            ready(
                setup.resume.as_deref(),
                &supervisor,
                family,
                setup.resume.is_some(),
            ),
        )
        .await
    {
        return DriverExit::Closed;
    }
    let mut existing = setup.resume.is_some();
    loop {
        let Some(command) = commands.recv().await else {
            return DriverExit::Closed;
        };
        match command {
            DriverCommand::Shutdown { .. } | DriverCommand::Interrupt => return DriverExit::Closed,
            command @ DriverCommand::Prompt { .. } => {
                let mut turn_setup = setup.clone();
                turn_setup.resume = existing.then(|| id.clone());
                match run_process(
                    &turn_setup,
                    family,
                    &mut native_id,
                    &mut commands,
                    &events,
                    &spawner,
                    Some(command),
                )
                .await
                {
                    ProcessOutcome::TurnFinished { created } => existing |= created,
                    ProcessOutcome::Exit(exit) => return exit,
                }
            }
            command => unsupported(command),
        }
    }
}

enum ProcessOutcome {
    TurnFinished { created: bool },
    Exit(DriverExit),
}
impl ProcessOutcome {
    fn exit(self) -> DriverExit {
        match self {
            Self::Exit(exit) => exit,
            Self::TurnFinished { .. } => DriverExit::Closed,
        }
    }
}

struct Turn {
    id: String,
    bytes: usize,
    digest: Sha256,
    terminal: Option<(StopReason, Option<TokenUsage>, Option<DriverError>)>,
}
impl Turn {
    fn new(id: String) -> Self {
        Self {
            id,
            bytes: 0,
            digest: Sha256::new(),
            terminal: None,
        }
    }
    fn delta(&mut self, text: &str, out: &mut VecDeque<DriverEvent>) {
        if text.is_empty() {
            return;
        }
        self.bytes = self.bytes.saturating_add(text.len());
        self.digest.update(text);
        out.push_back(DriverEvent::Event {
            turn: Some(self.id.clone()),
            event: GhostEvent::MessageDelta {
                message_id: format!("{}.m1", self.id),
                text: text.into(),
            },
        });
    }
    fn normalize_usage(&mut self, previous: &mut Option<TokenUsage>) {
        let Some((_, Some(total), _)) = self.terminal.as_mut() else {
            // A later difference would span this unreported turn, not just the next turn.
            *previous = None;
            return;
        };
        if total.scope != UsageScope::Session {
            return;
        }
        let current = total.clone();
        if let Some(before) = previous.as_ref()
            && total.input_tokens >= before.input_tokens
            && total.output_tokens >= before.output_tokens
        {
            total.input_tokens -= before.input_tokens;
            total.output_tokens -= before.output_tokens;
            let difference = |now: Option<u64>, before: Option<u64>| {
                now.zip(before)
                    .and_then(|(now, before)| now.checked_sub(before))
            };
            total.cache_read_tokens = difference(total.cache_read_tokens, before.cache_read_tokens);
            total.cache_write_tokens =
                difference(total.cache_write_tokens, before.cache_write_tokens);
            total.reasoning_tokens = difference(total.reasoning_tokens, before.reasoning_tokens);
            total.scope = UsageScope::Turn;
        }
        *previous = Some(current);
    }
    fn finish(self, out: &mut VecDeque<DriverEvent>) {
        if self.bytes > 0 {
            out.push_back(DriverEvent::Event {
                turn: Some(self.id.clone()),
                event: GhostEvent::MessageCompleted {
                    message_id: format!("{}.m1", self.id),
                },
            });
        }
        let (stop_reason, usage, error) = self.terminal.unwrap_or((
            StopReason::Error,
            None,
            Some(failure("The fallback exited without a terminal result.")),
        ));
        out.push_back(DriverEvent::TurnEnded {
            turn_id: self.id,
            stop_reason,
            usage,
            error,
        });
    }
}

fn usage(value: &Value, family: Family) -> Option<TokenUsage> {
    let value = value.get("usage")?;
    Some(TokenUsage {
        input_tokens: value.get("input_tokens")?.as_u64()?,
        output_tokens: value.get("output_tokens")?.as_u64()?,
        cache_read_tokens: value
            .get(if family == Family::Agy {
                "cache_read_tokens"
            } else {
                "cache_read_input_tokens"
            })
            .and_then(Value::as_u64),
        cache_write_tokens: value
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64),
        reasoning_tokens: value
            .get(if family == Family::Agy {
                "thinking_tokens"
            } else {
                "reasoning_tokens"
            })
            .and_then(Value::as_u64),
        scope: if family == Family::Agy {
            UsageScope::Session
        } else {
            UsageScope::Turn
        },
    })
}

fn frame(
    turn: &mut Turn,
    value: &Value,
    family: Family,
    native_id: &str,
    out: &mut VecDeque<DriverEvent>,
) -> Result<bool, DriverError> {
    if turn.terminal.is_some() {
        return Ok(false);
    }
    let tag = value
        .get(if family == Family::Agy {
            "event"
        } else {
            "type"
        })
        .and_then(Value::as_str)
        .unwrap_or("");
    if family == Family::Agy && tag == "step_update" {
        let step = &value["step_update"];
        if step
            .get("conversation_id")
            .and_then(Value::as_str)
            .is_some_and(|id| id != native_id)
        {
            return Err(failure(
                "Antigravity reported output from another conversation.",
            ));
        }
        if step["step_type"] == "agent_response"
            && let Some(text) = step["text_delta"].as_str()
        {
            turn.delta(text, out);
        }
    } else if family == Family::Grok && tag == "text" {
        if let Some(text) = value["data"].as_str() {
            turn.delta(text, out);
        } else {
            return Err(failure("Grok returned a text event without text."));
        }
    } else if family == Family::Agy && tag == "result" {
        let result = &value["result"];
        if result["conversation_id"].as_str() != Some(native_id) {
            return Err(failure(
                "Antigravity returned a result for another conversation.",
            ));
        }
        let text = result["response"]
            .as_str()
            .ok_or_else(|| failure("Antigravity returned an invalid result."))?;
        let prefix = text.get(..turn.bytes).ok_or_else(|| {
            failure("Antigravity's final response differs from its streamed text.")
        })?;
        if Sha256::digest(prefix) != turn.digest.clone().finalize() {
            return Err(failure(
                "Antigravity's final response differs from its streamed text.",
            ));
        }
        let suffix = &text[turn.bytes..];
        turn.delta(suffix, out);
        let (reason, error) = match result["status"].as_str() {
            Some("SUCCESS") => (StopReason::EndTurn, None),
            Some("CANCELLED") => (StopReason::Cancelled, None),
            Some(_) => (
                StopReason::Error,
                Some(failure(
                    result["error"]
                        .as_str()
                        .unwrap_or("Antigravity reported an unsuccessful result."),
                )),
            ),
            None => return Err(failure("Antigravity's result has no status.")),
        };
        turn.terminal = Some((reason, usage(result, family), error));
        return Ok(true);
    } else if family == Family::Grok && matches!(tag, "end" | "error") {
        if (tag == "end" || value.get("sessionId").is_some())
            && value["sessionId"].as_str() != Some(native_id)
        {
            return Err(failure("Grok returned a result for another session."));
        }
        let reason = match value["stopReason"].as_str() {
            Some("end_turn") => StopReason::EndTurn,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("max_turn_requests") => StopReason::MaxTurnRequests,
            Some("refusal") => StopReason::Refusal,
            Some("cancelled") => StopReason::Cancelled,
            _ => StopReason::Error,
        };
        let error = (tag == "error" || reason == StopReason::Error).then(|| {
            failure(
                value["message"]
                    .as_str()
                    .unwrap_or("Grok returned an invalid terminal result."),
            )
        });
        turn.terminal = Some((
            if tag == "error" {
                StopReason::Error
            } else {
                reason
            },
            usage(value, family),
            error,
        ));
        return Ok(true);
    }
    Ok(false)
}

#[allow(
    clippy::too_many_arguments,
    reason = "the per-process loop shares the supervisor's owned channels"
)]
async fn run_process(
    setup: &SessionSetup,
    family: Family,
    native_id: &mut Option<String>,
    commands: &mut mpsc::UnboundedReceiver<DriverCommand>,
    events: &EventSender,
    spawner: &Arc<dyn AgentSpawner>,
    first: Option<DriverCommand>,
) -> ProcessOutcome {
    let mut launch = setup.launch.clone();
    let mut turn = None;
    if let Some(DriverCommand::Prompt {
        turn: requested,
        parts,
    }) = &first
    {
        let text = match text_prompt(parts) {
            Ok(text) if text.encode_utf16().count() <= 12_000 => text,
            Ok(_) => {
                emit(
                    events,
                    ended(
                        requested.turn_id.clone(),
                        failure(
                            "This prompt is too large for Grok's command-line fallback. Use ACP.",
                        ),
                    ),
                )
                .await;
                return ProcessOutcome::TurnFinished { created: false };
            }
            Err(error) => {
                emit(events, ended(requested.turn_id.clone(), error)).await;
                return ProcessOutcome::TurnFinished { created: false };
            }
        };
        launch.args.extend(["-p".into(), text.into()]);
        if let Some(id) = native_id.as_ref() {
            launch.args.extend([
                if setup.resume.is_some() {
                    "--resume"
                } else {
                    "--session-id"
                }
                .into(),
                id.into(),
            ]);
        }
        turn = Some(Turn::new(requested.turn_id.clone()));
    } else if let Some(id) = &setup.resume {
        launch.args.extend(["--conversation".into(), id.into()]);
    }
    let spec = SpawnSpec {
        launch,
        folder: setup.folder.clone(),
        env: Arc::clone(&setup.env),
        max_line_bytes: MAX_LINE_BYTES,
    };
    let mut spawning = spawner.spawn(spec);
    let process = loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                None | Some(DriverCommand::Shutdown { .. } | DriverCommand::Interrupt) => return ProcessOutcome::Exit(DriverExit::Closed),
                Some(DriverCommand::Prompt { turn, .. }) => { emit(events, ended(turn.turn_id, failure("A fallback prompt is already starting."))).await; }
                Some(command) => unsupported(command),
            },
            result = &mut spawning => match result { Ok(process) => break process, Err(error) => return ProcessOutcome::Exit(DriverExit::StartFailed(crate::acp::spawn_failure(&error))) },
        }
    };
    process
        .log
        .note(LogNote::Start(setup.start_note(&process.spawn_id)));
    if let Some(command) = &first
        && let Ok(value) = serde_json::to_value(LoggedCommand::from(command))
    {
        process.log.command(value);
    }
    let sink = process.io.stdin;
    let mut stdout = process.io.stdout;
    let control = process.control;
    let mut exit_wait = control.exited();
    let mut exit_info = None;
    let mut shutdown: Option<BoxFuture<'static, ghost_process::ShutdownReport>> = None;
    let mut close_at = None;
    let mut stopped = false;
    let mut process_done = false;
    let mut eof = false;
    let mut ready_sent = family == Family::Grok;
    let mut failed: Option<DriverError> = None;
    let mut out = VecDeque::new();
    let startup = Instant::now() + setup.launch.startup_timeout;
    let mut drain_used = Duration::ZERO;
    let mut drain_since = None;
    let mut terminal_received = false;
    let mut identity_announced = setup.resume.is_some();
    // Agy's totals are cumulative over its session, but the baseline belongs to this process.
    let mut usage_total = None;
    loop {
        if failed.is_some() && shutdown.is_none() && !process_done {
            sink.close();
            shutdown = Some(control.shutdown(ShutdownPlan::FORCE));
        }
        if close_at.is_some_and(|due| Instant::now() >= due) && shutdown.is_none() && !process_done
        {
            sink.close();
            shutdown = Some(control.shutdown(ShutdownPlan::FORCE));
            close_at = None;
        }
        if eof && process_done {
            if let Some(mut finished) = turn.take() {
                terminal_received = finished.terminal.is_some();
                if stopped {
                    finished.terminal = Some((StopReason::Cancelled, None, None));
                } else if let Some(error) = &failed {
                    finished.terminal = Some((StopReason::Error, None, Some(error.clone())));
                }
                finished.finish(&mut out);
            }
            if out.is_empty() {
                break;
            }
        }
        let bytes = out.front().map_or(0, DriverEvent::approx_bytes);
        let full = out.len() >= 64
            || out.iter().map(DriverEvent::approx_bytes).sum::<usize>() >= 8 * 1024 * 1024;
        match (process_done && !eof && !full, drain_since) {
            (true, None) => drain_since = Some(Instant::now()),
            (false, Some(since)) => {
                drain_used += since.elapsed();
                drain_since = None;
            }
            _ => {}
        }
        let drain_due = drain_since.map(|since| since + PIPE_EOF_GRACE.saturating_sub(drain_used));
        let deadline = (!ready_sent && failed.is_none() && !stopped)
            .then_some(startup)
            .into_iter()
            .chain(close_at.filter(|_| !process_done && shutdown.is_none()))
            .chain(drain_due.filter(|_| !full && !eof))
            .min()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400));
        tokio::select! {
            biased;
            command = commands.recv(), if !stopped && failed.is_none() && !process_done => {
                if let Some(command) = &command && let Ok(value) = serde_json::to_value(LoggedCommand::from(command)) { process.log.command(value); }
                match command {
                    None | Some(DriverCommand::Shutdown { .. } | DriverCommand::Interrupt) => {
                        stopped = true; sink.close(); shutdown = Some(control.shutdown(ShutdownPlan::FORCE));
                    }
                    Some(DriverCommand::Prompt { turn: requested, parts }) => {
                        if !ready_sent || turn.is_some() { out.push_back(ended(requested.turn_id, failure("The fallback is not idle."))); }
                        else {
                            match text_prompt(&parts) {
                                Ok(text) => { turn = Some(Turn::new(requested.turn_id)); if sink.send(json!({"event":"user","message":{"content":text}}).to_string()).is_err() { failed = Some(failure("Antigravity stopped accepting input.")); } }
                                Err(error) => out.push_back(ended(requested.turn_id, error)),
                            }
                        }
                    }
                    Some(command) => unsupported(command),
                }
            }
            permit = events.reserve(bytes), if !out.is_empty() => {
                let Ok(permit) = permit else { control.kill_tree(); return ProcessOutcome::Exit(DriverExit::Closed); };
                if let Some(event) = out.pop_front() { permit.send(event); }
            }
            item = stdout.next(), if !eof && !full => match item {
                Some(StdoutItem::Line(line)) if failed.is_none() => match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        if family == Family::Agy && value["event"] == "init" {
                            match value["conversation_id"].as_str().filter(|id| uuid::Uuid::parse_str(id).is_ok()) {
                                Some(id) if native_id.as_deref().is_none_or(|expected| expected == id) => {
                                    *native_id = Some(id.into());
                                    if !ready_sent {
                                        ready_sent = true;
                                        out.push_back(DriverEvent::Autonomy(autonomy()));
                                        out.push_back(ready(Some(id), &process.spawn_id, family, setup.resume.is_some()));
                                    }
                                }
                                _ => failed = Some(failure("Antigravity returned an invalid or conflicting conversation id.")),
                            }
                        } else if let (Some(turn_ref), Some(id)) = (&mut turn, native_id.as_deref()) {
                            match frame(turn_ref, &value, family, id, &mut out) {
                                Ok(true) if family == Family::Agy => {
                                    if let Some(mut finished) = turn.take() {
                                        finished.normalize_usage(&mut usage_total);
                                        finished.finish(&mut out);
                                    }
                                }
                                Ok(true) => {
                                    if !identity_announced && value["sessionId"].as_str() == Some(id) {
                                        out.push_back(DriverEvent::NativeSessionId(id.into()));
                                        identity_announced = true;
                                    }
                                    close_at = Some(Instant::now() + Duration::from_secs(2));
                                },
                                Ok(false) => {}, Err(error) => failed = Some(error),
                            }
                        }
                    }
                    Err(_) => failed = Some(failure("The fallback returned malformed JSON.")),
                },
                Some(StdoutItem::Oversized { .. }) => failed = Some(failure("The fallback returned an oversized frame; the turn cannot be completed safely.")),
                Some(_) => {},
                None => { eof = true; if !process_done { close_at = Some(Instant::now() + Duration::from_secs(2)); } },
            },
            info = &mut exit_wait, if !process_done => { exit_info = Some(info); process_done = true; },
            report = async { match &mut shutdown { Some(task) => Some(task.await), None => None } }, if shutdown.is_some() => {
                shutdown = None; process_done = true; if let Some(report) = report { exit_info = exit_info.or(report.exit); }
            }
            () = sink.writer_closed(), if family == Family::Agy && !stopped && failed.is_none() && !process_done && shutdown.is_none() => {
                if let Some(info) = control.try_exited() { exit_info = Some(info); process_done = true; }
                else { failed = Some(failure("Antigravity stopped accepting input.")); }
            }
            _ = tokio::time::sleep_until(deadline) => {
                if !ready_sent && Instant::now() >= startup { failed = Some(failure("Antigravity did not identify its session before the startup deadline.")); ready_sent = false; }
                if drain_due.is_some_and(|due| Instant::now() >= due) && !full { eof = true; }
            }
        }
    }
    if stopped {
        ProcessOutcome::Exit(DriverExit::Closed)
    } else if family == Family::Grok && failed.is_none() && terminal_received {
        control.kill_tree();
        ProcessOutcome::TurnFinished {
            created: identity_announced,
        }
    } else if !ready_sent {
        ProcessOutcome::Exit(DriverExit::StartFailed(failed.unwrap_or_else(|| {
            failure("The fallback exited before identifying its session.")
        })))
    } else {
        ProcessOutcome::Exit(DriverExit::Crashed {
            exit: exit_info,
            stderr_tail: control.stderr_tail(ghost_process::STDERR_TAIL_BYTES),
        })
    }
}

#[cfg(test)]
#[path = "headless_tests.rs"]
mod tests;
