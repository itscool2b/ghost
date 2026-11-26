//! `ghost-cli smoke <agent> --scenario <names> --spend [--model <id>] [--effort <id>]`: the paid
//! scenarios of spec 10.3, in one chat in a fresh repository, in the order given, stopping at the
//! first failure.
//!
//! Each scenario runs in an agent process of its own, so each leaves one protocol log, the one its
//! fixture is curated from: the chat's first start (with the `--model` and `--effort` switches)
//! serves the first scenario, and every later one starts the agent again, which resumes the native
//! session. That makes `resume` exactly its spec ("stop the session, `chat_start`, then the
//! prompt"), and lets `after-interrupt` see whether an interrupted Hermes turn kept running in its
//! gateway (the replay at the next start says so).
//!
//! | Scenario | Prompt | Checks |
//! |---|---|---|
//! | basic | `Reply with exactly: ok` | streaming, `end_turn`, usage with the dialect's scope, the switched model |
//! | permission | `Create an empty file named aura-perm.txt ...`, then `Create it now.` (Ask) | a card; Reject: no file, the turn ends normally; Allow once: the file exists |
//! | interrupt | `Count from 1 to 300, one number per line.` | interrupt after the first text: `Cancelled`, the partial answer kept, a prompt queued before the interrupt held (then removed, never sent) |
//! | after-interrupt | `What was the last number you wrote? ...` | the reply is the last number of the interrupted answer |
//! | full-auto | `Create a file named aura-auto.txt containing hi.` | no card waits for the user; the file holds hi |
//! | resume | `Which files did you create in this chat? ...` | names both files; every turn stored once |
//!
//! The demo agent (`mock`) keeps no files and no memory, so it gets its magic prompts instead and
//! checks that hold for it: the scenarios still exercise the CLI and the session manager end to
//! end, without a real agent.

use std::path::Path;
use std::time::Duration;

use ghost_agents::AgentSpec;
use ghost_drivers::acp::dialect::dialect_for;
use ghost_protocol::session::{ConfigCategory, ConfigKind, ConfigValue, HistoryItem};
use ghost_protocol::{
    Autonomy, DriverKind, GhostEvent, PermissionOptionKind, PermissionRequest, ResolvedBy,
    SessionStatus, StopReason, ToolStatus, UsageScope,
};

use crate::CliError;
use crate::args::{Scenario, SmokeArgs};
use crate::chat::{ChatRun, StartEnd, TurnNext};
use crate::headless::Headless;
use crate::out::Out;
use crate::probe::{cheapest, status_name};
use crate::report::{Outcome, Report};
use crate::watch::{TurnView, Watch};

/// How long the smoke run waits for each thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmokeTimings {
    /// A start, a first adapter install included.
    pub start: Duration,
    pub stop: Duration,
    /// One turn, or the wait for its next card.
    pub turn: Duration,
}

impl SmokeTimings {
    pub const STANDARD: SmokeTimings = SmokeTimings {
        start: Duration::from_secs(12 * 60),
        stop: Duration::from_secs(30),
        turn: Duration::from_secs(5 * 60),
    };
}

/// A turn that keeps asking stops being answered after this many cards.
const MAX_CARDS: usize = 5;

/// The prompt queued during the interrupted turn: held, then removed, never sent.
const QUEUED: &str = "This prompt waits behind the interrupted turn and is never sent.";

/// The prompts of spec 10.3, and the demo agent's stand-ins.
struct Prompts {
    basic: &'static str,
    permission: &'static str,
    permission_again: &'static str,
    count: &'static str,
    last_number: &'static str,
    full_auto: &'static str,
    files: &'static str,
}

const REAL: Prompts = Prompts {
    basic: "Reply with exactly: ok",
    permission: "Create an empty file named aura-perm.txt in the current folder, then reply done.",
    permission_again: "Create it now.",
    count: "Count from 1 to 300, one number per line.",
    last_number: "What was the last number you wrote? Reply with only the number.",
    full_auto: "Create a file named aura-auto.txt containing hi.",
    files: "Which files did you create in this chat? Reply with the file names only.",
};

const DEMO: Prompts = Prompts {
    basic: "/echo ok",
    permission: "/permission",
    permission_again: "/permission",
    count: "/slow",
    last_number: "/echo after the interrupt",
    full_auto: "/permission",
    files: "/echo resumed",
};

/// Hermes' Ask (`default`) asks before edits made with its file tools; a shell command such as
/// `touch` goes to its own approval model instead, which lets harmless ones run. So its
/// permission prompts name the file tool.
const HERMES: Prompts = Prompts {
    permission: "Create an empty file named aura-perm.txt in the current folder using your write_file tool, not the terminal, then reply done.",
    permission_again: "Create it now with write_file.",
    ..REAL
};

/// Codex's Ask (codex-acp's `read-only`, approvals `on-request`) asks only when the model requests
/// more access; a small model told the folder is read-only may just say so. So its permission
/// prompts tell it to request approval.
const CODEX: Prompts = Prompts {
    permission: "Run the shell command `touch aura-perm.txt` in the current folder. The sandbox is read-only, so request approval to run it with write access. Then reply done.",
    permission_again: "Run `touch aura-perm.txt` again now, and request approval to run it with write access.",
    ..REAL
};

/// Dialects whose agent drops an interrupted turn's partial answer from its own context: recorded
/// with Codex 0.164.0 through codex-acp 2.2.2, whose resumed thread holds every completed turn
/// (the resume check passes) but not the interrupted one's output.
const FORGETS_INTERRUPTED: &[&str] = &["codex-acp"];

/// The files the real scenarios ask for.
const PERM_FILE: &str = "aura-perm.txt";
const AUTO_FILE: &str = "aura-auto.txt";

pub async fn run(
    headless: &Headless,
    args: &SmokeArgs,
    folder: &Path,
    timings: SmokeTimings,
    out: &Out,
) -> Result<Outcome, CliError> {
    let spec = headless
        .detector
        .spec(&args.agent)
        .ok_or_else(|| crate::unknown_agent(headless, &args.agent))?;
    let mut report = Report::new(out);
    report.info(
        "agent",
        format!("{} ({}), in {}", spec.name, spec.id, folder.display()),
    );
    let availability = headless.detector.detect(&spec).await;
    if let ghost_protocol::session::Availability::Missing { reason }
    | ghost_protocol::session::Availability::Broken { reason } = &availability
    {
        report.fail("detect", reason);
        report.note(report.summary());
        return Ok(report.outcome());
    }
    let watch = Watch::new();
    headless.manager.status_subscribe(watch.status_sink());
    let chat = match ChatRun::create(headless, &watch, &spec, folder).await {
        Ok(chat) => chat,
        Err(error) => {
            report.fail("start", error);
            report.note(report.summary());
            return Ok(report.outcome());
        }
    };
    let mut smoke = Smoke {
        headless,
        chat,
        report,
        timings,
        demo: spec.kind == DriverKind::Mock,
        spec,
        last_number: None,
        prompts: Vec::new(),
        queued: Vec::new(),
        native: None,
        logs: Vec::new(),
    };
    smoke.run(args).await;
    smoke.end().await;
    smoke.report.note(smoke.report.summary());
    Ok(smoke.report.outcome())
}

struct Smoke<'a> {
    headless: &'a Headless,
    chat: ChatRun<'a>,
    report: Report,
    timings: SmokeTimings,
    demo: bool,
    spec: AgentSpec,
    /// The last number of the interrupted answer, for after-interrupt.
    last_number: Option<u64>,
    /// The prompt ids this run started, in order.
    prompts: Vec<String>,
    /// Prompts this smoke owns that were queued, including the interrupt sentinel.
    queued: Vec<String>,
    /// The native session the last start opened.
    native: Option<String>,
    /// Each scenario's protocol logs.
    logs: Vec<(std::path::PathBuf, &'static str)>,
}

impl Smoke<'_> {
    fn prompts(&self) -> &'static Prompts {
        if self.demo {
            &DEMO
        } else if self.spec.dialect == "hermes" {
            &HERMES
        } else if self.spec.dialect == "codex-acp" {
            &CODEX
        } else {
            &REAL
        }
    }

    async fn run(&mut self, args: &SmokeArgs) {
        let (end, asked) = self.chat.settle(self.timings.start).await;
        for card in &asked {
            self.report.info(
                "start",
                format!("asked before ready: \"{}\"; answered cancelled", card.title),
            );
        }
        if end != StartEnd::Ready {
            self.start_failed("start", &end).await;
            return;
        }
        self.native = self
            .chat
            .ready_info(self.timings.stop)
            .await
            .and_then(|info| info.native_session_id);
        self.report.pass("start", "the agent is ready");
        if !self.settings(args).await {
            return;
        }
        for (index, scenario) in args.scenarios.iter().copied().enumerate() {
            let logs_before: Vec<_> = if index == 0 {
                Vec::new()
            } else {
                self.chat.logs()
            };
            if index > 0 && !self.restart(scenario).await {
                return;
            }
            let failures = self.report.failures();
            self.scenario(scenario).await;
            let stopped = matches!(self.chat.stop(self.timings.stop).await, Ok(true));
            self.cancel_owned_queue().await;
            for log in self
                .chat
                .logs()
                .into_iter()
                .filter(|log| !logs_before.contains(log))
            {
                self.logs.push((log, scenario.name()));
            }
            if !stopped {
                self.report.fail(
                    scenario.name(),
                    format!(
                        "the agent did not stop after the scenario ({})",
                        status_name(self.chat.status())
                    ),
                );
            }
            if self.report.failures() > failures {
                if index + 1 < args.scenarios.len() {
                    self.report.note(format!(
                        "Stopped at the first failure: {} did not run.",
                        args.scenarios[index + 1..]
                            .iter()
                            .map(|scenario| scenario.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                return;
            }
        }
    }

    /// `--model` and `--effort`, before the first prompt; without them, what is in use.
    async fn settings(&mut self, args: &SmokeArgs) -> bool {
        for (category, wanted, flag) in [
            (ConfigCategory::Model, args.model.as_deref(), "--model"),
            (ConfigCategory::Effort, args.effort.as_deref(), "--effort"),
        ] {
            let lists = match self.chat.lists().await {
                Ok(lists) => lists,
                Err(error) => {
                    self.report.fail("settings", error);
                    return false;
                }
            };
            let option = lists
                .options
                .iter()
                .find(|option| option.category == category);
            let label = format!("{category:?}").to_lowercase();
            let Some(option) = option else {
                if wanted.is_some() {
                    self.report.fail(
                        "settings",
                        format!("{flag}: the agent offers no {label} option"),
                    );
                    return false;
                }
                continue;
            };
            let ConfigKind::Select { current, choices } = &option.kind else {
                continue;
            };
            let Some(wanted) = wanted else {
                let name = choices
                    .iter()
                    .find(|choice| &choice.value == current)
                    .map_or(current.as_str(), |choice| choice.name.as_str());
                let mut text = format!("{label}: {name} ({current}), the agent's own choice");
                if category == ConfigCategory::Model
                    && let Some(cheap) = cheapest(option)
                    && &cheap != current
                {
                    text.push_str(&format!(
                        "; the cheapest-looking model offered is {cheap} (pass --model {cheap})"
                    ));
                }
                self.report.info("settings", text);
                continue;
            };
            let Some(choice) = choices
                .iter()
                .find(|choice| choice.value == wanted)
                .or_else(|| {
                    choices
                        .iter()
                        .find(|choice| choice.name.eq_ignore_ascii_case(wanted))
                })
            else {
                self.report.fail(
                    "settings",
                    format!(
                        "{flag} {wanted} is not offered; the choices are {}",
                        choices
                            .iter()
                            .map(|choice| choice.value.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                return false;
            };
            let (result, cards) = self
                .chat
                .answering(self.headless.manager.set_option(
                    &self.chat.chat_id,
                    &option.id,
                    ConfigValue::Choice {
                        value: choice.value.clone(),
                    },
                ))
                .await;
            for card in cards {
                self.report.info(
                    "settings",
                    format!(
                        "the agent asked \"{}\" while {label} changed; answered allow once",
                        card.title
                    ),
                );
            }
            match result {
                Ok(_) => self.report.pass(
                    "settings",
                    format!("{label} switched to {} ({})", choice.name, choice.value),
                ),
                Err(error) => {
                    self.report
                        .fail("settings", format!("{flag} {wanted}: {error}"));
                    return false;
                }
            }
        }
        true
    }

    /// Starts the agent again for the next scenario: a new process resuming the session.
    async fn restart(&mut self, scenario: Scenario) -> bool {
        let step = scenario.name();
        match self.chat.start(self.timings.start).await {
            Ok((StartEnd::Ready, _)) => {
                let now = self
                    .chat
                    .ready_info(self.timings.stop)
                    .await
                    .and_then(|info| info.native_session_id);
                if now != self.native && !self.demo {
                    self.report.info(
                        step,
                        format!(
                            "the agent opened a new session ({}) instead of continuing {}",
                            now.as_deref().unwrap_or("none"),
                            self.native.as_deref().unwrap_or("none")
                        ),
                    );
                }
                self.native = now;
                true
            }
            Ok((end, _)) => {
                self.start_failed(step, &end).await;
                false
            }
            Err(error) => {
                self.report.fail(step, error);
                false
            }
        }
    }

    async fn start_failed(&mut self, step: &str, end: &StartEnd) {
        let detail = match self.chat.snapshot().await {
            Ok(snapshot) => snapshot
                .last_error
                .map(|error| format!("{:?}: {}", error.kind, error.message))
                .or_else(|| snapshot.auth.map(|auth| auth.message))
                .unwrap_or_else(|| "no error card".into()),
            Err(error) => error.to_string(),
        };
        self.report.fail(
            step,
            format!("the agent did not become ready ({end:?}): {detail}"),
        );
        if *end == StartEnd::TimedOut {
            let _ = self.headless.manager.force_stop(&self.chat.chat_id).await;
        }
    }

    async fn scenario(&mut self, scenario: Scenario) {
        if scenario == Scenario::Permission {
            match self.chat.lists().await {
                Ok(lists)
                    if lists.capabilities.as_ref().is_some_and(|capabilities| {
                        !capabilities.permissions
                            || !capabilities.autonomy_levels.contains(&Autonomy::Ask)
                    }) =>
                {
                    self.report.skip(
                        scenario.name(),
                        "The agent does not offer an enforceable Ask permission mode; no permission prompts were sent.",
                    );
                    return;
                }
                Err(error) => {
                    self.report
                        .fail(scenario.name(), format!("capabilities: {error}"));
                    return;
                }
                _ => {}
            }
        }
        let autonomy = if scenario == Scenario::Permission {
            Autonomy::Ask
        } else {
            Autonomy::FullAuto
        };
        let (result, cards) = self
            .chat
            .answering(
                self.headless
                    .manager
                    .set_autonomy(&self.chat.chat_id, autonomy),
            )
            .await;
        for card in cards {
            self.report.info(
                scenario.name(),
                format!(
                    "the agent asked \"{}\" while autonomy changed; answered allow once",
                    card.title
                ),
            );
        }
        match result {
            Ok(lists) => {
                let state = lists.autonomy;
                let mut text = format!(
                    "autonomy {autonomy:?}: mode {}",
                    state.native_mode.as_deref().unwrap_or("unchanged")
                );
                if let Some(note) = state.note {
                    text.push_str(&format!(" (note: {note})"));
                }
                self.report.info(scenario.name(), text);
            }
            Err(error) => {
                self.report
                    .fail(scenario.name(), format!("autonomy: {error}"));
                return;
            }
        }
        match scenario {
            Scenario::Basic => self.basic().await,
            Scenario::Permission => self.permission().await,
            Scenario::Interrupt => self.interrupt().await,
            Scenario::AfterInterrupt => self.after_interrupt().await,
            Scenario::FullAuto => self.full_auto().await,
            Scenario::Resume => self.resume().await,
        }
    }

    /// Sends a prompt and answers its cards with `answer` until the turn ends.
    async fn turn(&mut self, step: &str, text: &str, answer: Answer) -> Option<(TurnView, usize)> {
        let ack = match self.chat.send(text).await {
            Ok(ack) => ack,
            Err(error) => {
                self.report
                    .fail(step, format!("the prompt was not sent: {error}"));
                return None;
            }
        };
        // A native result can precede run-idle. Keep tracking an accepted prompt while the
        // session manager waits for that idle report, rather than abandoning queued work.
        if ack.queued {
            self.queued.push(ack.prompt_id.clone());
        }
        self.prompts.push(ack.prompt_id.clone());
        let Some(turn_id) = self
            .chat
            .turn_started(&ack.prompt_id, self.timings.turn)
            .await
        else {
            self.report.fail(step, "the prompt's turn never started");
            let _ = self.headless.manager.force_stop(&self.chat.chat_id).await;
            return None;
        };
        let mut cards = 0;
        let mut answered: Vec<String> = Vec::new();
        loop {
            match self
                .chat
                .turn_next(&turn_id, &answered, self.timings.turn)
                .await
            {
                TurnNext::Card(card) => {
                    cards += 1;
                    answered.push(card.request_id.clone());
                    if cards > MAX_CARDS {
                        self.report
                            .fail(step, format!("the agent asked more than {MAX_CARDS} times"));
                        let _ = self.headless.manager.interrupt(&self.chat.chat_id).await;
                        return self
                            .chat
                            .turn_ended(&turn_id, self.timings.turn)
                            .await
                            .map(|view| (view, cards));
                    }
                    let option = answer.option(&card);
                    self.report.info(
                        step,
                        format!(
                            "card \"{}\" (kind {:?}): answered {}",
                            card.title,
                            card.kind,
                            option.as_deref().unwrap_or("cancelled")
                        ),
                    );
                    if let Err(error) = self
                        .headless
                        .manager
                        .respond_permission(&self.chat.chat_id, &card.request_id, option)
                        .await
                    {
                        self.report
                            .fail(step, format!("answering the card failed: {error}"));
                    }
                }
                TurnNext::Ended(view) => return Some((view, cards)),
                TurnNext::TimedOut => {
                    self.report.fail(
                        step,
                        format!(
                            "the turn did not end within {} s",
                            self.timings.turn.as_secs()
                        ),
                    );
                    let _ = self.headless.manager.force_stop(&self.chat.chat_id).await;
                    return None;
                }
            }
        }
    }

    /// The turn ended without an error.
    fn ended_well(&mut self, step: &str, view: &TurnView) -> bool {
        match &view.ended {
            Some((StopReason::Error, _)) => {
                let message = view.errors.last().map_or_else(
                    || "no error card".into(),
                    |(kind, message)| format!("{kind:?}: {message}"),
                );
                self.report
                    .fail(step, format!("the turn failed: {message}"));
                false
            }
            Some(_) => true,
            None => {
                self.report.fail(step, "the turn did not end");
                false
            }
        }
    }

    async fn basic(&mut self) {
        let step = "basic";
        let Some((view, cards)) = self.turn(step, self.prompts().basic, Answer::Reject).await
        else {
            return;
        };
        if !self.ended_well(step, &view) {
            return;
        }
        self.report
            .check(cards == 0, step, format!("{cards} card(s)"));
        self.report.check(
            view.delta_frames >= 1,
            step,
            format!(
                "the reply streamed in {} frame(s): {:?}",
                view.delta_frames,
                view.reply.trim()
            ),
        );
        let stop = view.ended.as_ref().map(|(stop, _)| *stop);
        self.report.check(
            stop == Some(StopReason::EndTurn),
            step,
            format!("stop reason {stop:?}"),
        );
        let scope = expected_scope(&self.spec, true);
        match view.ended.as_ref().and_then(|(_, usage)| usage.as_ref()) {
            Some(usage) => self.report.check(
                usage.scope == scope,
                step,
                format!(
                    "usage: {} in, {} out, scope {:?} (expected {scope:?})",
                    usage.input_tokens, usage.output_tokens, usage.scope
                ),
            ),
            None => self.report.fail(step, "the turn reported no usage"),
        }
        let model = self.chat.info().and_then(|info| info.model);
        self.report.info(
            step,
            format!(
                "the turn ran on the model {}",
                model.as_deref().unwrap_or("the agent did not name")
            ),
        );
    }

    async fn permission(&mut self) {
        let step = "permission";
        let prompts = self.prompts();
        let Some((view, cards)) = self.turn(step, prompts.permission, Answer::Reject).await else {
            return;
        };
        self.report.check(
            cards >= 1,
            step,
            format!("a card appeared ({cards} in all), rejected"),
        );
        if !self.ended_well(step, &view) {
            return;
        }
        if self.demo {
            self.report.check(
                view.tools
                    .values()
                    .all(|status| *status == ToolStatus::Failed),
                step,
                format!(
                    "after Reject the demo tool failed and the agent carried on: {:?}",
                    view.reply.trim()
                ),
            );
        } else {
            let exists = self.chat.folder.join(PERM_FILE).exists();
            self.report.check(
                !exists,
                step,
                format!(
                    "after Reject there is no {PERM_FILE}, and the agent carried on: {:?}",
                    view.reply.trim()
                ),
            );
        }
        let Some((view, cards)) = self
            .turn(step, prompts.permission_again, Answer::AllowOnce)
            .await
        else {
            return;
        };
        self.report.check(
            cards >= 1,
            step,
            format!("a card appeared ({cards} in all), allowed once"),
        );
        if !self.ended_well(step, &view) {
            return;
        }
        if self.demo {
            self.report.check(
                !view.tools.is_empty()
                    && view
                        .tools
                        .values()
                        .all(|status| *status == ToolStatus::Completed),
                step,
                format!(
                    "after Allow once the demo tool completed: {:?}",
                    view.reply.trim()
                ),
            );
        } else {
            let exists = self.chat.folder.join(PERM_FILE).exists();
            self.report
                .check(exists, step, format!("after Allow once {PERM_FILE} exists"));
        }
    }

    async fn interrupt(&mut self) {
        let step = "interrupt";
        let ack = match self.chat.send(self.prompts().count).await {
            Ok(ack) => ack,
            Err(error) => {
                self.report
                    .fail(step, format!("the prompt was not sent: {error}"));
                return;
            }
        };
        if ack.queued {
            self.queued.push(ack.prompt_id.clone());
        }
        self.prompts.push(ack.prompt_id.clone());
        let Some(turn_id) = self
            .chat
            .turn_started(&ack.prompt_id, self.timings.turn)
            .await
        else {
            self.report.fail(step, "the prompt's turn never started");
            let _ = self.headless.manager.force_stop(&self.chat.chat_id).await;
            return;
        };
        let Some(begun) = self.chat.first_delta(&turn_id, self.timings.turn).await else {
            self.report.fail(step, "no reply began");
            let _ = self.headless.manager.force_stop(&self.chat.chat_id).await;
            return;
        };
        if begun.ended.is_some() {
            self.report
                .fail(step, "the turn ended before the interrupt could be sent");
            return;
        }
        // Queued before the interrupt, so the interrupt holds it (decision 11).
        let queued = match self.chat.send(QUEUED).await {
            Ok(ack) => ack,
            Err(error) => {
                self.report
                    .fail(step, format!("queueing a prompt failed: {error}"));
                return;
            }
        };
        if queued.queued {
            self.queued.push(queued.prompt_id.clone());
        }
        if let Err(error) = self.headless.manager.interrupt(&self.chat.chat_id).await {
            self.report
                .fail(step, format!("the interrupt failed: {error}"));
            return;
        }
        let Some(view) = self.chat.turn_ended(&turn_id, self.timings.turn).await else {
            self.report.fail(step, "the interrupted turn did not end");
            let _ = self.headless.manager.force_stop(&self.chat.chat_id).await;
            return;
        };
        let stop = view.ended.as_ref().map(|(stop, _)| *stop);
        self.report.check(
            stop == Some(StopReason::Cancelled),
            step,
            format!("interrupted after the first text: stop reason {stop:?}"),
        );
        self.report.check(
            !view.reply.is_empty(),
            step,
            format!(
                "the partial answer is kept ({} characters)",
                view.reply.chars().count()
            ),
        );
        self.last_number = last_number(&view.reply);
        if !queued.queued {
            self.report
                .fail(step, "the second prompt was sent at once instead of queued");
        } else {
            let held = self
                .chat
                .watch
                .until(Duration::from_secs(5), |state| {
                    state
                        .info(&self.chat.chat_id)
                        .filter(|info| info.queue_held)
                        .map(|_| ())
                })
                .await
                .is_some();
            let started = self.chat.watch.with(|state| {
                state
                    .turn_of_prompt(&self.chat.chat_id, &queued.prompt_id)
                    .is_some()
            });
            self.report.check(
                held && !started,
                step,
                "the prompt queued before the interrupt is held, not sent",
            );
            match self
                .headless
                .manager
                .cancel_queued(&self.chat.chat_id, &queued.prompt_id)
                .await
            {
                Ok(()) => self.report.pass(step, "the held prompt was removed"),
                Err(error) => self
                    .report
                    .fail(step, format!("removing the held prompt failed: {error}")),
            }
        }
        if dialect_for(self.spec.dialect).replay_may_hold_unseen_work() {
            self.report.info(
                step,
                "whether the turn kept running in the gateway shows when the next scenario starts",
            );
        }
    }

    async fn after_interrupt(&mut self) {
        let step = "after-interrupt";
        let continued = self.chat.watch.with(|state| {
            state
                .notices_since(&self.chat.chat_id, 0)
                .into_iter()
                .any(|notice| notice.contains("continued this session while Ghost was away"))
        });
        if continued {
            self.report.fail(
                step,
                "the interrupted turn kept running: the agent's replay holds work done after the interrupt",
            );
        }
        let Some((view, _)) = self
            .turn(step, self.prompts().last_number, Answer::Reject)
            .await
        else {
            return;
        };
        if !self.ended_well(step, &view) {
            return;
        }
        let reply = view.reply.trim();
        if self.demo {
            self.report.check(
                reply == "after the interrupt",
                step,
                format!("the turn after the interrupt ended normally: {reply:?} (the demo agent keeps no context)"),
            );
            return;
        }
        let answered = reply
            .trim_matches(|c: char| !c.is_ascii_digit())
            .parse::<u64>()
            .ok();
        if FORGETS_INTERRUPTED.contains(&self.spec.dialect) && answered != self.last_number {
            // The agent's own behaviour: Ghost kept the partial answer (interrupt checked it).
            self.report.info(
                step,
                format!(
                    "{} keeps no interrupted answer across a restart: {reply:?}",
                    self.spec.name
                ),
            );
            return;
        }
        match (self.last_number, answered) {
            (Some(last), Some(answered)) => self.report.check(
                last == answered,
                step,
                format!("the agent says its last number was {answered}; the interrupted answer ended at {last}"),
            ),
            (None, _) => self.report.fail(step, "the interrupted answer held no number"),
            (_, None) => self.report.fail(step, format!("the reply is not a number: {reply:?}")),
        }
    }

    async fn full_auto(&mut self) {
        let step = "full-auto";
        let Some((view, cards)) = self
            .turn(step, self.prompts().full_auto, Answer::Reject)
            .await
        else {
            return;
        };
        self.report.check(
            cards == 0,
            step,
            format!("{cards} card(s) waited for the user"),
        );
        let auto: Vec<String> = view
            .requests
            .iter()
            .filter(|request| view.resolved_by(&request.request_id) == Some(ResolvedBy::Auto))
            .map(|request| format!("\"{}\" ({:?})", request.title, request.kind))
            .collect();
        if !auto.is_empty() {
            self.report
                .info(step, format!("auto-approved: {}", auto.join(", ")));
        }
        if !self.ended_well(step, &view) {
            return;
        }
        if self.demo {
            self.report.check(
                !view.tools.is_empty()
                    && view
                        .tools
                        .values()
                        .all(|status| *status == ToolStatus::Completed),
                step,
                "the demo tool ran without a card",
            );
        } else {
            let path = self.chat.folder.join(AUTO_FILE);
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            self.report.check(
                text.contains("hi"),
                step,
                format!("{AUTO_FILE} holds {:?}", text.trim()),
            );
        }
    }

    async fn resume(&mut self) {
        let step = "resume";
        let Some((view, _)) = self.turn(step, self.prompts().files, Answer::Reject).await else {
            return;
        };
        if !self.ended_well(step, &view) {
            return;
        }
        let reply = view.reply.trim().to_owned();
        if self.demo {
            self.report.check(
                reply == "resumed",
                step,
                format!("the turn after the restart ended normally: {reply:?}"),
            );
        } else {
            self.report.check(
                reply.contains(PERM_FILE) && reply.contains(AUTO_FILE),
                step,
                format!("the reply names {PERM_FILE} and {AUTO_FILE}: {reply:?}"),
            );
        }
        match self.history().await {
            Ok(items) => {
                let problems = duplicated_turns(&items, &self.prompts);
                self.report.check(
                    problems.is_empty(),
                    step,
                    if problems.is_empty() {
                        format!(
                            "every one of the {} turns is stored once",
                            self.prompts.len()
                        )
                    } else {
                        problems.join("; ")
                    },
                );
            }
            Err(error) => self.report.fail(step, error),
        }
    }

    /// The chat's whole stored history, oldest first.
    async fn history(&self) -> Result<Vec<HistoryItem>, ghost_core::session::SessionError> {
        let mut items: Vec<HistoryItem> = Vec::new();
        let mut before = u32::MAX;
        loop {
            let page = self
                .headless
                .manager
                .history(&self.chat.chat_id, before, Some(4 * 1024 * 1024))
                .await?;
            items.splice(0..0, page.items);
            match page.before_seq {
                Some(next) if page.has_more && next > 0 && next < before => before = next,
                _ => return Ok(items),
            }
        }
    }

    async fn end(&mut self) {
        if !matches!(self.chat.stop(self.timings.stop).await, Ok(true)) {
            let _ = self.headless.manager.force_stop(&self.chat.chat_id).await;
            let _ = self
                .chat
                .wait_status(SessionStatus::Stopped, self.timings.stop)
                .await;
        }
        self.cancel_owned_queue().await;
        if self.logs.is_empty() {
            if self.demo {
                self.report.note("The demo agent writes no protocol log.");
            }
            return;
        }
        if self.spec.kind == DriverKind::Headless {
            let mut lines =
                vec!["Protocol logs (text fallback replay is not supported yet):".to_owned()];
            lines.extend(
                self.logs
                    .iter()
                    .map(|(log, name)| format!("  {name}: {}", log.display())),
            );
            self.report.note(lines.join("\n"));
            return;
        }
        let mut lines = vec!["Protocol logs, and the fixture each can become:".to_owned()];
        let fixture_folder = if matches!(
            self.spec.kind,
            DriverKind::Claude | DriverKind::Codex | DriverKind::Pi
        ) {
            self.spec.id.to_owned()
        } else {
            format!("acp/{}", self.spec.id)
        };
        for (log, name) in &self.logs {
            lines.push(format!(
                "  ghost-cli fixture {} --scenario {name} --out crates/ghost-drivers/tests/fixtures/{fixture_folder}/{name}.jsonl",
                log.display()
            ));
        }
        self.report.note(lines.join("\n"));
    }

    /// Called after stop has held the queue, so a timed-out smoke cannot run later on restart.
    async fn cancel_owned_queue(&mut self) {
        for prompt_id in std::mem::take(&mut self.queued) {
            if let Err(error) = self
                .headless
                .manager
                .cancel_queued(&self.chat.chat_id, &prompt_id)
                .await
                && !matches!(error, ghost_core::session::SessionError::RequestNotPending)
            {
                self.report.fail(
                    "end",
                    format!("removing queued smoke prompt failed: {error}"),
                );
            }
        }
    }
}

/// How a scenario answers the cards of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// Reject once when offered, else cancelled.
    Reject,
    /// Allow once when offered (never "always"), else cancelled.
    AllowOnce,
}

impl Answer {
    fn option(self, card: &PermissionRequest) -> Option<String> {
        let kind = match self {
            Answer::Reject => PermissionOptionKind::RejectOnce,
            Answer::AllowOnce => PermissionOptionKind::AllowOnce,
        };
        card.options
            .iter()
            .find(|option| option.kind == kind)
            .map(|option| option.option_id.clone())
    }
}

/// The usage scope a turn reports: per turn for turn-scope dialects (and the demo agent); for
/// session-scope ones (Hermes), the session total on a spawn's first turn and deltas after it
/// (spec 5.5.4).
pub fn expected_scope(spec: &AgentSpec, first_of_spawn: bool) -> UsageScope {
    if spec.kind == DriverKind::Headless && spec.dialect == "grok-stream" {
        return UsageScope::Turn;
    }
    if matches!(
        spec.kind,
        DriverKind::Mock | DriverKind::Claude | DriverKind::Codex | DriverKind::Pi
    ) {
        return UsageScope::Turn;
    }
    match dialect_for(spec.dialect).prompt_usage_scope() {
        UsageScope::Turn => UsageScope::Turn,
        UsageScope::Session if first_of_spawn => UsageScope::Session,
        UsageScope::Session => UsageScope::Turn,
    }
}

/// The last whole number in a text.
pub fn last_number(text: &str) -> Option<u64> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|word| !word.is_empty())
        .filter_map(|word| word.parse().ok())
        .next_back()
}

/// What breaks "every turn stored once": a turn started or ended twice, or a turn count that
/// differs from the prompts the run started.
pub fn duplicated_turns(items: &[HistoryItem], prompts: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut started: Vec<String> = Vec::new();
    let mut ended: Vec<String> = Vec::new();
    for item in items {
        match &item.event {
            GhostEvent::TurnStarted { turn_id, .. } => {
                if started.contains(turn_id) {
                    problems.push(format!("the turn {turn_id} is stored twice"));
                }
                started.push(turn_id.clone());
            }
            GhostEvent::TurnEnded { turn_id, .. } => {
                if ended.contains(turn_id) {
                    problems.push(format!("the turn {turn_id} ended twice"));
                }
                ended.push(turn_id.clone());
            }
            _ => {}
        }
    }
    if started.len() != prompts.len() {
        problems.push(format!(
            "{} turns are stored for {} prompts",
            started.len(),
            prompts.len()
        ));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_number_is_the_last_whole_number() {
        assert_eq!(last_number("1\n2\n3\n47"), Some(47));
        assert_eq!(last_number("1\n2\n3\n4"), Some(4));
        assert_eq!(last_number("Counting: 1, 2, 3, 12"), Some(12));
        assert_eq!(last_number("no digits"), None);
    }

    #[test]
    fn usage_scopes_follow_the_dialect() {
        let claude = ghost_agents::get("claude").expect("native Claude");
        assert_eq!(expected_scope(claude, true), UsageScope::Turn);
        assert_eq!(expected_scope(claude, false), UsageScope::Turn);
        let hermes = ghost_agents::get("hermes").expect("hermes").clone();
        assert_eq!(expected_scope(&hermes, true), UsageScope::Session);
        assert_eq!(expected_scope(&hermes, false), UsageScope::Turn);
        let codex = ghost_agents::get("codex-acp").expect("codex-acp").clone();
        assert_eq!(expected_scope(&codex, true), UsageScope::Turn);
        let mock = ghost_agents::get("mock").expect("mock").clone();
        assert_eq!(expected_scope(&mock, true), UsageScope::Turn);
        for first in [true, false] {
            assert_eq!(
                expected_scope(ghost_agents::get("grok-stream").unwrap(), first),
                UsageScope::Turn
            );
        }
        assert_eq!(
            expected_scope(ghost_agents::get("agy-stream").unwrap(), true),
            UsageScope::Session
        );
    }

    #[test]
    fn duplicated_turns_are_found() {
        let item = |seq: u32, event: GhostEvent| HistoryItem {
            seq,
            turn_id: None,
            at: 0,
            event,
        };
        let started = |id: &str| GhostEvent::TurnStarted {
            turn_id: id.into(),
            prompt_id: None,
        };
        let ended = |id: &str| GhostEvent::TurnEnded {
            turn_id: id.into(),
            stop_reason: StopReason::EndTurn,
            usage: None,
            duration_ms: None,
            recovered: false,
        };
        let good = vec![
            item(0, started("a")),
            item(1, ended("a")),
            item(2, started("b")),
            item(3, ended("b")),
        ];
        let prompts = vec!["p1".to_owned(), "p2".to_owned()];
        assert!(duplicated_turns(&good, &prompts).is_empty());
        let mut twice = good.clone();
        twice.push(item(4, started("a")));
        let problems = duplicated_turns(&twice, &prompts);
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("stored twice")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("3 turns are stored for 2 prompts"))
        );
    }

    #[test]
    fn cards_are_answered_once_never_always() {
        let card = PermissionRequest {
            request_id: "r".into(),
            tool_call_id: None,
            title: "Write".into(),
            kind: None,
            raw_input: None,
            content: Vec::new(),
            locations: Vec::new(),
            options: vec![
                ghost_protocol::PermissionOption {
                    option_id: "always".into(),
                    kind: PermissionOptionKind::AllowAlways,
                    label: "Always".into(),
                },
                ghost_protocol::PermissionOption {
                    option_id: "once".into(),
                    kind: PermissionOptionKind::AllowOnce,
                    label: "Once".into(),
                },
                ghost_protocol::PermissionOption {
                    option_id: "no".into(),
                    kind: PermissionOptionKind::RejectOnce,
                    label: "No".into(),
                },
            ],
            truncated: None,
        };
        assert_eq!(Answer::AllowOnce.option(&card).as_deref(), Some("once"));
        assert_eq!(Answer::Reject.option(&card).as_deref(), Some("no"));
        let bare = PermissionRequest {
            options: Vec::new(),
            ..card
        };
        assert_eq!(Answer::AllowOnce.option(&bare), None);
    }
}
