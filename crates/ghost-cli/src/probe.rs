//! `ghost-cli probe <agent> [--env NAME=VALUE]... [--folder <dir>]`: the free probe steps of spec
//! 10.2 through the headless session manager. No prompt is ever sent.
//!
//! | Step | What it does |
//! |---|---|
//! | 0 install | npm adapters: the one-time install's time and size, no bundled platform CLI, `npm prefix` in the adapter folder; a first install is interrupted once (step 9's second half), then done cleanly |
//! | 1 initialize | from the protocol log: the agent's name and version, protocol version, capabilities, auth methods, time to answer; nothing used the hostile chat folder's npm files |
//! | 2 session | the options, the legacy models and modes, the current model, anything asked before ready |
//! | 3 commands | after 3 s: the slash commands and the largest commands update |
//! | 4 options, autonomy | every option set to its current value; the model switched to the cheapest and back (what else the switch changed; Hermes: what a second session reports meanwhile); Full auto, Auto-edit, Ask and back, each mode reaching the agent, against the dialect's candidates |
//! | 5 cancel | an interrupt while idle changes nothing |
//! | 6 close | Stop: stdin closed, the exit and the shutdown steps; nothing left in the process table (Unix), once a stop's chain is over |
//! | 7 resume | ACP starts again: the session continues (or the agent says why not: claude-acp and codex-acp keep no session before its first prompt), no transcript comes back, and chosen options return. Native Claude is skipped because this free probe has no persisted prompted transcript. |
//! | 8 auth | an agent that needs a sign-in: its card (codex-acp with `--env CODEX_HOME=<empty folder>`) |
//! | 9 interrupt | an interrupt right after initialize stops the start with nothing left, and the next start works |
//!
//! Step 10 (a Finder launch of a built app) stays manual. Each spawn leaves a protocol log, and the
//! probe ends by naming each log with the `probe-*` fixture it can become.
//! Restricted text fallbacks expose no control RPC; their free probe checks executable detection
//! only and explicitly leaves native startup, auth, output and resume to an authorized smoke run.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ghost_agents::{AgentSpec, Recipe};
use ghost_drivers::acp::dialect::dialect_for;
use ghost_process::ptable::ProcInfo;
use ghost_process::{HelperEnv, HelperSpec, find_program, run_helper};
use ghost_protocol::session::{
    Availability, ConfigCategory, ConfigChoice, ConfigKind, ConfigOption, ConfigValue, SessionLists,
};
use ghost_protocol::{AutoAnswer, Autonomy, AutonomyState, DriverKind, GhostEvent, SessionStatus};
use serde_json::Value;

use crate::CliError;
use crate::args::ProbeArgs;
use ghost_core::session::SessionError;

use crate::chat::{ChatRun, StartEnd};
use crate::headless::Headless;
use crate::logs;
use crate::out::Out;
use crate::ps;
use crate::repo::HOSTILE_MARK;
use crate::report::{Outcome, Report};
use crate::watch::Watch;

/// How long the probe waits for each thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeTimings {
    /// A start: an adapter's first install (10 min) and the agent's own start (60 s), with margin.
    pub start: Duration,
    /// A stop: the chain's 10 s and the session's forced stop at 15 s, with margin.
    pub stop: Duration,
    /// Step 3's wait for the slash commands.
    pub commands: Duration,
    /// How long the session must stay ready after step 5's interrupt.
    pub settle: Duration,
    /// How long a written record may take to reach the log file.
    pub log: Duration,
    /// How long step 9 waits for a first install's npm to run before it interrupts the install
    /// anyway (at once once npm shows in the process table).
    pub install_run: Duration,
}

impl ProbeTimings {
    pub const STANDARD: ProbeTimings = ProbeTimings {
        start: Duration::from_secs(12 * 60),
        stop: Duration::from_secs(30),
        commands: Duration::from_secs(3),
        settle: Duration::from_millis(500),
        log: Duration::from_secs(5),
        install_run: Duration::from_secs(10),
    };
}

/// Why a model switch did not show.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SetFailure {
    /// The agent answered the change with an error.
    Refused(String),
    /// The change went through, but the session reports another value.
    NotReported(String),
    /// The session could not ask (the agent stopped, say).
    Failed(String),
}

impl SetFailure {
    fn text(&self, value: &str) -> String {
        match self {
            SetFailure::Refused(error) | SetFailure::Failed(error) => {
                format!("switching the model to {value} failed: {error}")
            }
            SetFailure::NotReported(reported) => {
                format!("the model was set to {value}, but the session reports {reported}")
            }
        }
    }
}

/// How long a mode change may take to reach the agent: the driver's option request timeout (30 s,
/// spec appendix A) with a margin.
const MODE_CHANGE_LIMIT: Duration = Duration::from_secs(35);

/// How long the process table check waits for what a stop is still ending: the FORCE chain's
/// SIGTERM grace and kill confirmation (2 s each, spec appendix A), with a margin.
const LEFTOVER_GRACE: Duration = Duration::from_secs(6);

/// Words cheaper models carry in their ids or names, cheapest first. Hermes names its free tier
/// so (`Nous · free tier · nous/welcome`).
const CHEAP_MARKERS: [&str; 9] = [
    "free",
    "haiku",
    "nano",
    "mini",
    "flash-lite",
    "lite",
    "flash",
    "small",
    "fast",
];

/// Words the descriptions of cheaper models use, cheapest first, read when no id or name carries a
/// marker: codex-acp describes `gpt-6-luna` as "Fast and affordable model for easier tasks".
const CHEAP_DESCRIPTIONS: [&str; 4] = ["cheapest", "cheap", "affordable", "fast"];

/// The names `--env` sets, for messages; values are never printed.
fn env_names(args: &ProbeArgs) -> String {
    args.env
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn run(
    headless: &Headless,
    args: &ProbeArgs,
    folder: &Path,
    hostile: bool,
    timings: ProbeTimings,
    out: &Out,
) -> Result<Outcome, CliError> {
    let spec = headless
        .detector
        .spec(&args.agent)
        .ok_or_else(|| crate::unknown_agent(headless, &args.agent))?;
    let mut probe = Probe {
        headless,
        report: Report::new(out),
        spec,
        timings,
        hostile,
        folder: folder.to_path_buf(),
        before: None,
        user_binary: None,
        main_log: None,
        pids: Vec::new(),
        named_logs: Vec::new(),
    };
    probe.run(args).await;
    probe.report.note(probe.report.summary());
    Ok(probe.report.outcome())
}

struct Probe<'a> {
    headless: &'a Headless,
    report: Report,
    spec: AgentSpec,
    timings: ProbeTimings,
    /// Whether the chat folder is the probe's hostile repository (not `--folder`).
    hostile: bool,
    folder: PathBuf,
    /// The process table before the first start (Unix).
    before: Option<Vec<ProcInfo>>,
    /// The user's binary detection found (an adapter's), or the agent's program.
    user_binary: Option<String>,
    /// The protocol log of the session steps 1 to 6 probe.
    main_log: Option<PathBuf>,
    /// Every agent process the probe started, from the spawn notes.
    pids: Vec<u32>,
    /// Each protocol log with the fixture name it can become.
    named_logs: Vec<(PathBuf, &'static str)>,
}

impl Probe<'_> {
    fn demo(&self) -> bool {
        self.spec.kind == DriverKind::Mock
    }

    fn native_claude(&self) -> bool {
        self.spec.kind == DriverKind::Claude
    }

    fn native_codex(&self) -> bool {
        self.spec.kind == DriverKind::Codex
    }

    fn native_pi(&self) -> bool {
        self.spec.kind == DriverKind::Pi
    }

    /// Cards the agent showed while a setting changed, and how the probe answered them.
    fn report_cards(&mut self, step: &str, cards: &[ghost_protocol::PermissionRequest]) {
        for card in cards {
            let allowed = card
                .options
                .iter()
                .any(|option| option.kind == ghost_protocol::PermissionOptionKind::AllowOnce);
            self.report.info(
                step,
                format!(
                    "the agent asked \"{}\" while a setting changed; the probe answered {}",
                    card.title,
                    if allowed { "allow once" } else { "cancelled" }
                ),
            );
        }
    }

    /// The log steps 1 to 6 read: the session's own, or (before it is ready) the newest.
    fn session_log(&self, chat: &ChatRun<'_>) -> Option<PathBuf> {
        self.main_log
            .clone()
            .or_else(|| chat.logs().last().cloned())
    }

    /// Notes the agent pid of a log's spawn, for the process table checks.
    fn note_pid(&mut self, log: &Path) {
        if let Some(pid) = logs::read(log)
            .ok()
            .and_then(|read| logs::spawn_pid(&read.records))
            && !self.pids.contains(&pid)
        {
            self.pids.push(pid);
        }
    }

    /// Names every log of `chat` not in `before` as the fixture `name` can become, and notes its
    /// pid.
    fn name_new_logs(&mut self, chat: &ChatRun<'_>, before: &[PathBuf], name: &'static str) {
        for log in chat.logs().into_iter().filter(|log| !before.contains(log)) {
            self.note_pid(&log);
            self.named_logs.push((log, name));
        }
    }

    async fn run(&mut self, args: &ProbeArgs) {
        let spec = self.spec.clone();
        self.report.info(
            "agent",
            format!("{} ({}), dialect {}", spec.name, spec.id, spec.dialect),
        );
        match self.headless.detector.detect(&spec).await {
            Availability::Ready { version, path } => {
                self.report.pass(
                    "detect",
                    format!("{} ({path})", version.as_deref().unwrap_or("ready")),
                );
                self.user_binary = Some(path);
            }
            Availability::Missing { reason } | Availability::Broken { reason } => {
                self.report.fail("detect", reason);
                return;
            }
            Availability::Checking => {
                self.report.fail("detect", "detection did not finish");
                return;
            }
        }
        if spec.kind == DriverKind::Headless {
            self.report.skip(
                "1-9 controls",
                "This text fallback has no control RPC. Only executable detection was checked; native startup, authentication, output and resume remain unverified. No chat or prompt was started.",
            );
            return;
        }
        if !args.env.is_empty() {
            self.report.info(
                "env",
                format!(
                    "set on every agent process, after the scrub: {}",
                    env_names(args)
                ),
            );
        }
        self.before = ps::snapshot();
        let watch = Watch::new();
        self.headless.manager.status_subscribe(watch.status_sink());
        let adapter = adapter_folder(self.headless, &spec);
        let installed_before = adapter.as_deref().is_some_and(is_installed);
        let started_at = Instant::now();
        let chat = match ChatRun::create(self.headless, &watch, &spec, &self.folder).await {
            Ok(chat) => chat,
            Err(error) => {
                self.report.fail("start", error);
                return;
            }
        };
        let mut details_mark = 0;
        let mut start_mark = 0;
        if let Some(adapter) = adapter.as_deref()
            && !installed_before
        {
            self.interrupt_install(&chat, adapter).await;
            details_mark = watch.with(|state| state.details.len());
            start_mark = chat.mark();
            if let Err(error) = self.headless.manager.start(&chat.chat_id, false).await {
                self.report.fail("start", error);
                self.end(&chat).await;
                return;
            }
        }
        let (end, asked) = chat.settle(self.timings.start).await;
        let took = started_at.elapsed();
        self.install_step(&chat, adapter.as_deref(), installed_before, details_mark)
            .await;
        match end {
            StartEnd::Ready => {
                self.report
                    .pass("start", format!("ready in {:.1} s", took.as_secs_f64()));
            }
            StartEnd::AuthRequired => {
                self.main_log = chat.logs().last().cloned();
                if let Some(log) = self.main_log.clone() {
                    self.named_logs.push((log, "probe-auth"));
                }
                self.initialize_step(&chat).await;
                self.auth_step(&chat).await;
                self.end(&chat).await;
                return;
            }
            other => {
                self.start_failed(&chat, &other, start_mark).await;
                self.initialize_step(&chat).await;
                self.end(&chat).await;
                return;
            }
        }
        self.main_log = chat.logs().last().cloned();
        if let Some(log) = self.main_log.clone() {
            self.named_logs.push((log, "probe-session"));
        }
        self.initialize_step(&chat).await;
        let lists = self.session_step(&chat, &asked).await;
        self.commands_step(&chat).await;
        let chosen = self.options_step(&chat, lists.as_ref(), &watch).await;
        self.autonomy_step(&chat).await;
        self.cancel_step(&chat).await;
        let native = chat.info().and_then(|info| info.native_session_id);
        if self.close_step(&chat).await {
            // Step 4 chose every option's value as it was at the start, then switched the model,
            // so a restart, which re-applies what was chosen (5.3.2), must bring back those values
            // with the model and thinking level the switch left.
            self.resume_step(&chat, chosen.as_ref(), native.as_deref())
                .await;
            self.interrupted_start_step(&chat).await;
        }
        self.end(&chat).await;
    }

    /// Spec 10.2 step 9's second half, for an adapter not installed yet: the first start's
    /// download is interrupted once, which must leave no npm running and no install marked.
    async fn interrupt_install(&mut self, chat: &ChatRun<'_>, adapter: &Path) {
        let step = "9 interrupt";
        let downloading = chat
            .watch
            .until(self.timings.start, |state| {
                let detail = state.info(&chat.chat_id)?.detail.clone()?;
                detail.starts_with("Downloading").then_some(detail)
            })
            .await;
        if downloading.is_none() {
            self.report
                .fail(step, "the first start never showed the adapter's download");
            return;
        }
        // A small adapter installs within a second or two, so the interrupt goes as soon as npm
        // runs (or once the wait is over where no process table can show it).
        let deadline = Instant::now() + self.timings.install_run;
        while Instant::now() < deadline && !is_installed(adapter) {
            match ps::snapshot() {
                Some(table) if npm_install_running(&table) => break,
                Some(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                None => {
                    tokio::time::sleep_until(deadline.into()).await;
                    break;
                }
            }
        }
        if let Err(error) = self.headless.manager.interrupt(&chat.chat_id).await {
            self.report.fail(step, error);
            return;
        }
        if !chat
            .wait_status(SessionStatus::Stopped, self.timings.stop)
            .await
        {
            self.report.fail(
                step,
                format!(
                    "an interrupt during the first install left the chat {}",
                    status_name(chat.status())
                ),
            );
            return;
        }
        if is_installed(adapter) {
            self.report.info(
                step,
                "the install finished before the interrupt landed; the next start used it",
            );
        } else {
            self.report.pass(
                step,
                "an interrupt during the first install stopped it; nothing is marked installed",
            );
        }
        self.leftovers(
            step,
            &[
                adapter.display().to_string(),
                "--omit=optional --ignore-scripts".into(),
            ],
        )
        .await;
    }

    async fn install_step(
        &mut self,
        chat: &ChatRun<'_>,
        adapter: Option<&Path>,
        installed_before: bool,
        details_mark: usize,
    ) {
        let step = "0 install";
        let Some(adapter) = adapter else {
            self.report
                .skip(step, format!("{} is not an npm adapter", self.spec.name));
            return;
        };
        if installed_before {
            self.report
                .info(step, format!("already installed in {}", adapter.display()));
        } else if let Some(took) = download_time(chat, details_mark) {
            self.report.pass(
                step,
                format!(
                    "installed in {:.1} s into {}",
                    took.as_secs_f64(),
                    adapter.display()
                ),
            );
        } else if is_installed(adapter) {
            self.report
                .info(step, format!("installed into {}", adapter.display()));
        }
        if !is_installed(adapter) {
            self.report
                .fail(step, "the adapter is not marked installed");
            return;
        }
        self.report.info(
            step,
            format!("the folder holds {}", megabytes(folder_size(adapter))),
        );
        let bundled = bundled_clis(adapter);
        if bundled.is_empty() {
            self.report.pass(
                step,
                "no bundled CLI: no @anthropic-ai/claude-agent-sdk-<platform> or @openai/codex-<platform> package",
            );
        } else {
            self.report.fail(
                step,
                format!("bundled CLI packages are installed: {}", bundled.join(", ")),
            );
        }
        self.npm_prefix(adapter).await;
    }

    /// `npm prefix` in the adapter folder must print that folder: npm treats it as its own
    /// project, so no folder above it decides what runs.
    async fn npm_prefix(&mut self, adapter: &Path) {
        let step = "0 install";
        let env = self.headless.env.get().await;
        let Some(npm) = find_program("npm", &env.vars) else {
            self.report
                .skip(step, "npm is not on PATH, so npm prefix was not checked");
            return;
        };
        let output = run_helper(
            &self.headless.registry,
            HelperSpec {
                program: npm,
                args: vec!["prefix".into()],
                cwd: adapter.to_path_buf(),
                env: HelperEnv::Resolved(env),
                set: vec![("npm_config_update_notifier".into(), "false".into())],
                new_session: false,
                timeout: Duration::from_secs(30),
                max_stdout: 64 * 1024,
            },
        )
        .await;
        match output {
            Ok(output) if output.exit.is_some_and(|exit| exit.code == Some(0)) => {
                let printed = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                let same = prefix_names_folder(&printed, adapter);
                self.report.check(
                    same,
                    step,
                    format!("npm prefix in the adapter folder prints {printed}"),
                );
            }
            Ok(output) => self.report.fail(
                step,
                format!("npm prefix failed: {}", output.stderr_tail.trim()),
            ),
            Err(error) => self
                .report
                .fail(step, format!("npm prefix failed: {error}")),
        }
    }

    async fn start_failed(&mut self, chat: &ChatRun<'_>, end: &StartEnd, mark: usize) {
        let errors = chat
            .watch
            .with(|state| state.errors_since(&chat.chat_id, mark));
        let detail = errors
            .last()
            .map(|(kind, message, detail)| {
                let mut text = format!("{kind:?}: {message}");
                if let Some(detail) = detail {
                    text.push('\n');
                    text.push_str(detail);
                }
                text
            })
            .unwrap_or_else(|| "no error card".into());
        let what = match end {
            StartEnd::TimedOut => format!(
                "the start did not end within {} s",
                self.timings.start.as_secs()
            ),
            StartEnd::Stopped => "the agent stopped while it started".into(),
            _ => "the start failed".into(),
        };
        self.report.fail("start", format!("{what}: {detail}"));
        if *end == StartEnd::TimedOut {
            let _ = self.headless.manager.force_stop(&chat.chat_id).await;
        }
    }

    /// The main log's records, once `ready` holds for them (the log writer runs behind).
    async fn log_records(&self, log: &Path, ready: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + self.timings.log;
        loop {
            let records = logs::read(log).map(|read| read.records).unwrap_or_default();
            if ready(&records) || Instant::now() >= deadline {
                return records;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn initialize_step(&mut self, chat: &ChatRun<'_>) {
        let step = "1 initialize";
        if self.demo() {
            let info = chat.info();
            self.report.skip(
                step,
                format!(
                    "the demo agent runs inside Ghost: no process and no protocol log ({} {})",
                    info.as_ref()
                        .and_then(|info| info.agent_name.clone())
                        .unwrap_or_default(),
                    info.and_then(|info| info.agent_version).unwrap_or_default()
                ),
            );
            return;
        }
        let Some(log) = self.session_log(chat) else {
            self.report.fail(step, "the start left no protocol log");
            return;
        };
        let initialize = if self.native_pi() {
            "get_state"
        } else {
            "initialize"
        };
        let records = self
            .log_records(&log, |records| {
                logs::exchange(records, initialize).is_some()
            })
            .await;
        self.note_pid(&log);
        let Some((sent, answer)) = logs::exchange(&records, initialize) else {
            self.report
                .fail(step, format!("no reply to initialize in {}", log.display()));
            return;
        };
        let msg = logs::message(answer).cloned().unwrap_or(Value::Null);
        let took = match (logs::t(sent), logs::t(answer)) {
            (Some(sent), Some(answered)) => format!(" in {} ms", answered - sent),
            _ => String::new(),
        };
        if self.native_codex() {
            match codex_initialize(&records) {
                Ok(()) => self.report.pass(step, format!("Codex answered initialize{took}; initialized was sent before opening the thread")),
                Err(reason) => self.report.fail(step, reason),
            }
            self.hostile_check(&records);
            return;
        }
        if self.native_pi() {
            match pi_state_result(&msg) {
                Ok(_) => self
                    .report
                    .pass(step, format!("Pi answered native get_state{took}")),
                Err(reason) => self.report.fail(step, reason),
            }
            self.hostile_check(&records);
            return;
        }
        if self.native_claude() {
            match native_initialize_result(&msg) {
                Ok(result) => {
                    self.report.pass(
                        step,
                        format!(
                            "Claude Code {} answered native initialize{took}; session state {}",
                            text_at(result, "/claude_code_version"),
                            text_at(result, "/session_state")
                        ),
                    );
                    self.report.info(
                        step,
                        format!(
                            "native permission mode {}; capabilities: {}",
                            text_at(result, "/current_permission_mode"),
                            result
                                .get("capabilities")
                                .map_or_else(|| "none".into(), Value::to_string)
                        ),
                    );
                }
                Err(reason) => self.report.fail(step, reason),
            }
            self.hostile_check(&records);
            return;
        }
        match msg.get("result") {
            Some(result) => {
                let name = text_at(result, "/agentInfo/name");
                let version = text_at(result, "/agentInfo/version");
                self.report.pass(
                    step,
                    format!(
                        "{name} {version} answered{took}; protocol version {}",
                        result
                            .get("protocolVersion")
                            .map_or_else(|| "?".into(), Value::to_string)
                    ),
                );
                self.report.info(
                    step,
                    format!(
                        "capabilities: {}; auth methods: {}",
                        result
                            .get("agentCapabilities")
                            .map_or_else(|| "none".into(), Value::to_string),
                        auth_method_ids(result)
                    ),
                );
            }
            None => self.report.fail(
                step,
                format!(
                    "initialize failed: {}",
                    msg.get("error")
                        .map_or_else(|| "?".into(), Value::to_string)
                ),
            ),
        }
        self.hostile_check(&records);
    }

    /// Nothing may run the chat folder's npm files: the mark of its fake adapters never shows,
    /// and an adapter runs in its own folder.
    fn hostile_check(&mut self, records: &[Value]) {
        let step = "1 initialize";
        if !self.hostile {
            self.report.skip(
                step,
                "the chat folder is your own (--folder), not the hostile one",
            );
            return;
        }
        if logs::mentions(records, HOSTILE_MARK) {
            self.report.fail(
                step,
                "the log shows HOSTILE: something ran the chat folder's fake adapter",
            );
            return;
        }
        let cwd = logs::note(records, "spawn")
            .and_then(|spawn| spawn.get("cwd"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        match &self.spec.recipe {
            Recipe::NpmAdapter { .. } => {
                let adapter = adapter_folder(self.headless, &self.spec).unwrap_or_default();
                self.report.check(
                    same_folder(Path::new(&cwd), &adapter),
                    step,
                    format!(
                        "nothing used the hostile chat folder's npm files; the adapter ran in {cwd}"
                    ),
                );
            }
            Recipe::Binary { .. }
            | Recipe::QualifiedBinary { .. }
            | Recipe::NativeBinary { .. }
            | Recipe::Builtin => self
                .report
                .pass(step, "nothing ran the hostile chat folder's fake adapters"),
        }
    }

    async fn auth_step(&mut self, chat: &ChatRun<'_>) {
        let step = "8 auth";
        match chat.snapshot().await {
            Ok(snapshot) => match snapshot.auth {
                Some(auth) => {
                    let commands = auth
                        .commands
                        .iter()
                        .map(|command| {
                            let mut text = command.argv.join(" ");
                            if !command.env.is_empty() {
                                let names: Vec<&str> =
                                    command.env.iter().map(|(name, _)| name.as_str()).collect();
                                text.push_str(&format!(" (sets {})", names.join(", ")));
                            }
                            format!("{}: {text}", command.label)
                        })
                        .collect::<Vec<_>>();
                    self.report.info(
                        step,
                        format!(
                            "the agent needs a sign-in: {}\n{}",
                            auth.message,
                            if commands.is_empty() {
                                "no sign-in command".to_owned()
                            } else {
                                commands.join("\n")
                            }
                        ),
                    );
                    self.report
                        .skip("2-9", "the other steps need an agent that is signed in");
                }
                None => self
                    .report
                    .fail(step, "the chat reads AuthRequired without an auth card"),
            },
            Err(error) => self.report.fail(step, error),
        }
    }

    async fn session_step(
        &mut self,
        chat: &ChatRun<'_>,
        asked: &[ghost_protocol::PermissionRequest],
    ) -> Option<SessionLists> {
        let step = "2 session";
        let lists = match chat.lists().await {
            Ok(lists) => lists,
            Err(error) => {
                self.report.fail(step, error);
                return None;
            }
        };
        let native = chat
            .ready_info(self.timings.stop)
            .await
            .and_then(|info| info.native_session_id)
            .unwrap_or_default();
        self.report.pass(step, format!("opened session {native}"));
        self.report.info(step, options_text(&lists.options));
        if let Some(model) = lists
            .options
            .iter()
            .find(|option| option.category == ConfigCategory::Model)
        {
            self.report
                .info(step, format!("current model: {}", current_text(model)));
        }
        if !self.demo()
            && let Some(log) = self.session_log(chat)
        {
            if self.native_codex() {
                let records = self.log_records(&log, open_settled).await;
                match codex_open(&records, &native) {
                    Ok(description) => self.report.pass(step, description),
                    Err(reason) => self.report.fail(step, reason),
                }
            } else if self.native_pi() {
                let records = self
                    .log_records(&log, |records| {
                        logs::exchange(records, "get_state").is_some()
                    })
                    .await;
                match pi_open(&records, &native) {
                    Ok(description) => self.report.pass(step, description),
                    Err(reason) => self.report.fail(step, reason),
                }
            } else if self.native_claude() {
                let records = self
                    .log_records(&log, |records| {
                        logs::exchange(records, "initialize").is_some()
                    })
                    .await;
                match native_open(&records, &native, None) {
                    Ok(description) => self.report.pass(step, description),
                    Err(reason) => self.report.fail(step, reason),
                }
            } else {
                let records = self
                    .log_records(&log, |records| {
                        logs::first_open(records).is_some_and(|open| {
                            logs::message(open)
                                .and_then(|msg| msg.get("id"))
                                .is_some_and(|id| logs::reply(records, id).is_some())
                        })
                    })
                    .await;
                if let Some(result) = open_result(&records) {
                    let models = result
                        .pointer("/models/availableModels")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    let modes: Vec<&str> = result
                        .pointer("/modes/availableModes")
                        .and_then(Value::as_array)
                        .map(|modes| {
                            modes
                                .iter()
                                .filter_map(|mode| mode.get("id").and_then(Value::as_str))
                                .collect()
                        })
                        .unwrap_or_default();
                    self.report.info(
                    step,
                    format!(
                        "the open's reply: {models} legacy model(s) (current {}); modes {} (current {})",
                        text_at(result, "/models/currentModelId"),
                        if modes.is_empty() {
                            "none".to_owned()
                        } else {
                            modes.join(", ")
                        },
                        text_at(result, "/modes/currentModeId"),
                    ),
                );
                }
            }
        }
        if asked.is_empty() {
            self.report.info(
                step,
                "nothing was asked before the session was ready (no folder trust question)",
            );
        } else {
            for card in asked {
                self.report.info(
                    step,
                    format!(
                        "asked before ready: \"{}\" (kind {:?}); the probe answered cancelled",
                        card.title, card.kind
                    ),
                );
            }
        }
        Some(lists)
    }

    async fn commands_step(&mut self, chat: &ChatRun<'_>) {
        let step = "3 commands";
        if self.native_codex() {
            self.report.skip(
                step,
                "the native Codex driver does not advertise slash commands",
            );
            return;
        }
        tokio::time::sleep(self.timings.commands).await;
        let count = match chat.lists().await {
            Ok(lists) => lists.commands.len(),
            Err(error) => {
                self.report.fail(step, error);
                return;
            }
        };
        let largest = if self.demo() {
            None
        } else {
            self.session_log(chat).and_then(|log| {
                let records = logs::read(&log).ok()?.records;
                largest_commands_update(&records, self.native_claude())
            })
        };
        self.report.info(
            step,
            match largest {
                Some(bytes) => format!(
                    "{count} slash command(s) after {:.1} s; the largest commands update was {bytes} bytes",
                    self.timings.commands.as_secs_f64()
                ),
                None => format!(
                    "{count} slash command(s) after {:.1} s",
                    self.timings.commands.as_secs_f64()
                ),
            },
        );
    }

    /// Sets every option to its current value, then switches the model and back. Returns the
    /// options as chosen: their values at the start, with the model the agent took last and the
    /// thinking level it showed with that model, since a model change hands the level to the agent
    /// and the next start restores the level the agent showed.
    async fn options_step(
        &mut self,
        chat: &ChatRun<'_>,
        lists: Option<&SessionLists>,
        watch: &std::sync::Arc<Watch>,
    ) -> Option<SessionLists> {
        let step = "4 options";
        if self.native_codex() {
            self.report.info(step, "Codex model and effort changes are staged for the next turn; this free probe verifies local choices only");
        }
        let Some(lists) = lists else {
            self.report.skip(step, "no options to set");
            return None;
        };
        if self.native_pi() {
            let valid = lists.options.iter().all(|option| match &option.kind {
                ConfigKind::Select { current, choices } => {
                    choices.iter().any(|choice| choice.value == *current)
                }
                ConfigKind::Toggle { .. } => false,
            });
            self.report.check(
                valid,
                step,
                "Pi model and thinking metadata contain their current choices",
            );
            self.report.skip(step,"Pi development probes leave settings unchanged; live setter behavior is covered by protocol tests");
            return Some(lists.clone());
        }
        let mut failed = false;
        for option in &lists.options {
            let value = current_value(option);
            let (result, cards) = chat
                .answering(
                    self.headless
                        .manager
                        .set_option(&chat.chat_id, &option.id, value),
                )
                .await;
            self.report_cards(step, &cards);
            if let Err(error) = result {
                self.report.fail(
                    step,
                    format!("setting {} to its current value failed: {error}", option.id),
                );
                failed = true;
            }
        }
        if !failed {
            match chat.lists().await {
                Ok(after) => {
                    let changed = changed_options(&lists.options, &after.options);
                    self.report.check(
                        changed.is_empty(),
                        step,
                        if changed.is_empty() {
                            format!(
                                "every option set to its current value ({}); nothing changed",
                                lists.options.len()
                            )
                        } else {
                            format!("setting current values changed {}", changed.join(", "))
                        },
                    );
                }
                Err(error) => self.report.fail(step, error),
            }
        }
        let mut chosen = lists.clone();
        if let Some((option_id, value)) = self.model_switch(chat, lists, watch).await {
            if let Some(option) = chosen
                .options
                .iter_mut()
                .find(|option| option.id == option_id)
                && let ConfigKind::Select { current, .. } = &mut option.kind
            {
                *current = value;
            }
            if let Ok(now) = chat.lists().await {
                for option in &mut chosen.options {
                    if option.category == ConfigCategory::Effort
                        && let Some(after) = now.options.iter().find(|after| after.id == option.id)
                    {
                        option.kind = after.kind.clone();
                    }
                }
            }
        }
        Some(chosen)
    }

    /// Switches the model to the cheapest-looking choice and back; both changes must show. Returns
    /// the model option's id and the value the agent took last, when they changed hands.
    async fn model_switch(
        &mut self,
        chat: &ChatRun<'_>,
        lists: &SessionLists,
        watch: &std::sync::Arc<Watch>,
    ) -> Option<(String, String)> {
        let step = "4 options";
        let Some(model) = lists
            .options
            .iter()
            .find(|option| option.category == ConfigCategory::Model)
        else {
            self.report.skip(step, "the agent offers no model option");
            return None;
        };
        let ConfigKind::Select { current, choices } = &model.kind else {
            self.report.skip(step, "the model option is not a list");
            return None;
        };
        let Some(target) = switch_target(model) else {
            self.report.skip(step, "only one model is offered");
            return None;
        };
        let original = current.clone();
        let name_of = |value: &str| {
            choices
                .iter()
                .find(|choice| choice.value == value)
                .map_or_else(|| value.to_owned(), |choice| choice.name.clone())
        };
        match self.set_model(chat, &model.id, &target).await {
            Ok(()) => {
                self.report.pass(
                    step,
                    format!("the model switched to {} ({target})", name_of(&target)),
                );
                if self.spec.dialect == "hermes" {
                    self.second_session(chat, watch, &original, &target).await;
                }
            }
            Err(failure) => {
                self.report.fail(step, failure.text(&target));
                return None;
            }
        }
        match self.set_model(chat, &model.id, &original).await {
            // Hermes 0.22.5 can't take a session back to its configured `auto:` model: from a
            // provider that checks the model (its `nous`) it refuses the id it offered itself.
            Err(SetFailure::Refused(error)) => {
                self.report.info(
                    step,
                    format!(
                        "the agent refused to switch back to {} ({original}), a model it offers itself: {error}",
                        name_of(&original)
                    ),
                );
                return Some((model.id.clone(), target));
            }
            Err(failure) => {
                self.report.fail(step, failure.text(&original));
                return Some((model.id.clone(), target));
            }
            Ok(()) => {}
        }
        self.report.pass(
            step,
            format!(
                "the model switched back to {} ({original})",
                name_of(&original)
            ),
        );
        // An agent may change other options with the model (codex-acp: a model that offers
        // no `ultra` sets `medium`) and keep them when it switches back.
        if let Ok(now) = chat.lists().await {
            let others = |options: &[ConfigOption]| -> Vec<ConfigOption> {
                options
                    .iter()
                    .filter(|option| option.id != model.id)
                    .cloned()
                    .collect()
            };
            for text in changed_options(&others(&lists.options), &others(&now.options)) {
                self.report.info(
                    step,
                    format!("switching the model changed {text}, and switching back left it"),
                );
            }
        }
        Some((model.id.clone(), original))
    }

    /// Sets the model and checks that the session reports it.
    async fn set_model(
        &mut self,
        chat: &ChatRun<'_>,
        option_id: &str,
        value: &str,
    ) -> Result<(), SetFailure> {
        let step = "4 options";
        let (result, cards) = chat
            .answering(self.headless.manager.set_option(
                &chat.chat_id,
                option_id,
                ConfigValue::Choice {
                    value: value.to_owned(),
                },
            ))
            .await;
        self.report_cards(step, &cards);
        match result {
            Ok(_) => {
                // The reply can come before the session applied the agent's new option list, so
                // the change is read back until it shows.
                let wanted = ConfigValue::Choice {
                    value: value.to_owned(),
                };
                let deadline = Instant::now() + self.timings.log;
                let reported = loop {
                    let reported = chat.lists().await.ok().and_then(|lists| {
                        lists
                            .options
                            .iter()
                            .find(|option| option.id == option_id)
                            .map(current_value)
                    });
                    if reported.as_ref() == Some(&wanted) || Instant::now() >= deadline {
                        break reported;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                };
                if reported.as_ref() == Some(&wanted) {
                    Ok(())
                } else {
                    Err(SetFailure::NotReported(format!("{reported:?}")))
                }
            }
            Err(SessionError::OptionInvalid(message)) => Err(SetFailure::Refused(message)),
            Err(error) => Err(SetFailure::Failed(error.to_string())),
        }
    }

    /// Hermes: while the first session is switched, what model a second `session/new` reports
    /// says whether the switch stays inside the session (spec 10.2 step 4).
    async fn second_session(
        &mut self,
        chat: &ChatRun<'_>,
        watch: &std::sync::Arc<Watch>,
        original: &str,
        target: &str,
    ) {
        let step = "4 options";
        let second = match ChatRun::create(self.headless, watch, &self.spec, &chat.folder).await {
            Ok(second) => second,
            Err(error) => {
                self.report
                    .fail(step, format!("a second session did not start: {error}"));
                return;
            }
        };
        let (end, _) = second.settle(self.timings.start).await;
        if end == StartEnd::Ready {
            self.name_new_logs(&second, &[], "probe-second-session");
            let reported = second.lists().await.ok().and_then(|lists| {
                lists
                    .options
                    .iter()
                    .find(|option| option.category == ConfigCategory::Model)
                    .map(current_value)
            });
            let reported = match reported {
                Some(ConfigValue::Choice { value }) => value,
                _ => "nothing".into(),
            };
            let meaning = if reported == original {
                "the switch stays inside the session"
            } else if reported == target {
                "the switch changed the agent's configured model"
            } else {
                "neither model"
            };
            self.report.info(
                step,
                format!("while switched, a second session reports {reported}: {meaning}"),
            );
        } else {
            self.report.fail(
                step,
                format!("a second session did not become ready ({end:?})"),
            );
        }
        if !matches!(second.stop(self.timings.stop).await, Ok(true)) {
            let _ = self.headless.manager.force_stop(&second.chat_id).await;
        }
    }

    /// Full auto, Auto-edit, Ask and back, each against the dialect's candidates (spec 5.5.2).
    async fn autonomy_step(&mut self, chat: &ChatRun<'_>) {
        let step = "4 autonomy";
        if self.native_codex() {
            self.report.info(step, "Codex permission settings are sent on the next turn; this free probe verifies local policy staging only");
        }
        if self.native_pi() {
            match chat.lists().await {
                Ok(lists) => self.report.check(
                    lists.autonomy.requested == Autonomy::FullAuto
                        && lists.autonomy.auto_answer == AutoAnswer::Never
                        && lists
                            .capabilities
                            .as_ref()
                            .is_some_and(|c| c.autonomy_levels == [Autonomy::FullAuto]),
                    step,
                    "Pi offers Full auto and never auto-answers extension dialogs",
                ),
                Err(error) => self.report.fail(step, error),
            }
            return;
        }
        let original = match chat.lists().await {
            Ok(lists) => lists.autonomy.requested,
            Err(error) => {
                self.report.fail(step, error);
                return;
            }
        };
        for level in [
            Autonomy::FullAuto,
            Autonomy::AutoEdit,
            Autonomy::Ask,
            original,
        ] {
            let (result, cards) = chat
                .answering(self.headless.manager.set_autonomy(&chat.chat_id, level))
                .await;
            self.report_cards(step, &cards);
            let state = match result {
                Ok(lists) => lists.autonomy,
                Err(error) => {
                    self.report
                        .fail(step, format!("{}: {error}", level_name(level)));
                    continue;
                }
            };
            // The policy applies at once and the mode change follows (spec 5.5.2); a later change
            // replaces one still on its way, so the walk waits for each mode to reach the agent.
            let state = match self.mode_lands(chat, state).await {
                Ok(state) => state,
                Err(why) => {
                    self.report
                        .fail(step, format!("{}: {why}", level_name(level)));
                    continue;
                }
            };
            let expected = expected_modes(&self.spec, level);
            let text = autonomy_text(level, &state);
            if state.requested != level {
                self.report
                    .fail(step, format!("asked for {}, got {text}", level_name(level)));
            } else if expected.contains(&state.native_mode) {
                self.report.pass(step, text);
            } else {
                self.report.fail(
                    step,
                    format!(
                        "{text}; expected the mode {}",
                        expected
                            .iter()
                            .map(|mode| mode.clone().unwrap_or_else(|| "unchanged".into()))
                            .collect::<Vec<_>>()
                            .join(" or ")
                    ),
                );
            }
        }
    }

    /// Waits until the agent runs the mode `state` names: ACP reports its Mode option with nothing
    /// pending, while native Claude acknowledges the control. Returns the autonomy in effect then,
    /// including a fallback after a candidate's mode change failed.
    async fn mode_lands(
        &self,
        chat: &ChatRun<'_>,
        state: AutonomyState,
    ) -> Result<AutonomyState, String> {
        let Some(mode) = state.native_mode.clone() else {
            return Ok(state);
        };
        let deadline = Instant::now() + MODE_CHANGE_LIMIT;
        loop {
            let lists = chat.lists().await.map_err(|error| error.to_string())?;
            let landed = if self.native_codex() {
                lists.autonomy == state
            } else if self.native_claude() {
                self.session_log(chat)
                    .and_then(|log| logs::read(&log).ok())
                    .is_some_and(|read| native_effective_mode(&read.records) == Some(mode.as_str()))
            } else {
                lists
                .options
                .iter()
                .find(|option| option.category == ConfigCategory::Mode)
                .is_some_and(|option| {
                    option.pending.is_none()
                        && matches!(&option.kind, ConfigKind::Select { current, .. } if *current == mode)
                })
            };
            if landed || lists.autonomy.native_mode.as_deref() != Some(mode.as_str()) {
                return Ok(lists.autonomy);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the agent did not switch to the mode {mode} within {} s",
                    MODE_CHANGE_LIMIT.as_secs()
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// An interrupt while no prompt runs: nothing happens, and no `session/cancel` is sent.
    async fn cancel_step(&mut self, chat: &ChatRun<'_>) {
        let step = "5 cancel";
        if let Err(error) = self.headless.manager.interrupt(&chat.chat_id).await {
            self.report.fail(step, error);
            return;
        }
        tokio::time::sleep(self.timings.settle).await;
        let status = chat.status();
        let method = if self.native_codex() {
            "turn/interrupt"
        } else if self.native_pi() {
            "abort"
        } else if self.native_claude() {
            "interrupt"
        } else {
            "session/cancel"
        };
        let cancels = if self.demo() {
            0
        } else {
            self.session_log(chat)
                .and_then(|log| logs::read(&log).ok())
                .map_or(0, |read| {
                    logs::requests_of(&read.records, &[method]).count()
                })
        };
        self.report.check(
            status == Some(SessionStatus::Ready) && cancels == 0,
            step,
            format!(
                "an interrupt while idle left the session {} and sent {cancels} {method} (Ghost sends none while no prompt runs)",
                status_name(status)
            ),
        );
    }

    /// Stop: stdin closes and the agent exits; nothing it started may be left. Returns whether
    /// the chat stopped.
    async fn close_step(&mut self, chat: &ChatRun<'_>) -> bool {
        let step = "6 close";
        match chat.stop(self.timings.stop).await {
            Ok(true) => {}
            Ok(false) => {
                self.report.fail(
                    step,
                    format!(
                        "the chat did not stop within {} s ({})",
                        self.timings.stop.as_secs(),
                        status_name(chat.status())
                    ),
                );
                return false;
            }
            Err(error) => {
                self.report.fail(step, error);
                return false;
            }
        }
        if self.demo() {
            self.report.pass(step, "the demo agent stopped");
            return true;
        }
        let Some(log) = self.session_log(chat) else {
            self.report.fail(step, "no protocol log");
            return true;
        };
        let records = self
            .log_records(&log, |records| logs::note(records, "shutdown").is_some())
            .await;
        self.report_exit(step, &records);
        self.leftovers(step, &[]).await;
        true
    }

    fn report_exit(&mut self, step: &str, records: &[Value]) {
        let exit = logs::note(records, "exit");
        let shutdown = logs::note(records, "shutdown");
        let steps: Vec<String> = shutdown
            .and_then(|shutdown| shutdown.get("steps"))
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .filter_map(|step| step.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let took = shutdown
            .and_then(|shutdown| shutdown.get("took_ms"))
            .and_then(Value::as_u64);
        let code = exit
            .and_then(|exit| exit.get("code"))
            .cloned()
            .unwrap_or(Value::Null);
        let signal = exit
            .and_then(|exit| exit.get("signal"))
            .cloned()
            .unwrap_or(Value::Null);
        let on_eof = steps.iter().any(|step| step == "ExitedAfterEof")
            && !steps
                .iter()
                .any(|step| step.starts_with("SentSigterm") || step.starts_with("Killed"));
        let text = format!(
            "steps {}, {}exit code {code}, signal {signal}",
            if steps.is_empty() {
                "none logged".to_owned()
            } else {
                steps.join(", ")
            },
            took.map(|took| format!("{took} ms, ")).unwrap_or_default(),
        );
        if on_eof {
            self.report
                .pass(step, format!("the agent exited when stdin closed: {text}"));
        } else {
            self.report.info(
                step,
                format!("the agent did not exit on stdin's end alone: {text}"),
            );
        }
    }

    /// The `ps` diff: nothing of any agent the probe started is left; declared daemons are listed.
    /// What a stop is still ending gets [`LEFTOVER_GRACE`] to go: a force stop reads `Stopped` at
    /// once while its FORCE chain runs, and an interrupted install's npm dies with its tree.
    async fn leftovers(&mut self, step: &str, extra: &[String]) {
        let Some(before) = self.before.clone() else {
            self.report
                .skip(step, "no process table here; the job object ends the tree");
            return;
        };
        let mut needles: Vec<String> = extra.to_vec();
        needles.push(self.folder.display().to_string());
        if let Some(binary) = &self.user_binary
            && binary != "built in"
        {
            needles.push(binary.clone());
        }
        if let Some(adapter) = adapter_folder(self.headless, &self.spec) {
            needles.push(adapter.display().to_string());
        }
        let deadline = Instant::now() + LEFTOVER_GRACE;
        let found = loop {
            let Some(after) = ps::snapshot() else {
                self.report
                    .skip(step, "the process table could not be read");
                return;
            };
            let found = ps::leftovers(
                &before,
                &after,
                &self.pids,
                &needles,
                &self.spec.daemons,
                std::process::id(),
            );
            if found.is_clean() || Instant::now() >= deadline {
                break found;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        for spared in &found.spared {
            self.report.info(
                step,
                format!("spared (a declared daemon): {}", ps::describe(spared)),
            );
        }
        if found.is_clean() {
            self.report.pass(
                step,
                format!(
                    "nothing left running: no process in the group of agent(s) {:?}, no new process naming them",
                    self.pids
                ),
            );
        } else {
            let mut lines = vec!["processes left running:".to_owned()];
            lines.extend(
                found
                    .group
                    .iter()
                    .map(|info| format!("in the agent's group: {}", ps::describe(info))),
            );
            lines.extend(
                found
                    .related
                    .iter()
                    .map(|info| format!("started during the run: {}", ps::describe(info))),
            );
            self.report.fail(step, lines.join("\n"));
        }
    }

    /// Start again: the native session continues (or the agent says why it can't), no transcript
    /// comes back, and the options are back as `chosen` (what step 4 set them to).
    async fn resume_step(
        &mut self,
        chat: &ChatRun<'_>,
        chosen: Option<&SessionLists>,
        native: Option<&str>,
    ) {
        let step = "7 resume";
        if self.native_pi() {
            self.report.skip(step,"Pi resume recall requires a prompted transcript and is not exercised by this read-only probe");
            return;
        }
        if self.native_claude() || self.native_codex() {
            self.report.skip(
                step,
                "native resume was not exercised: this free probe sent no prompt; a prompted smoke test must verify persisted transcript resume and restored settings",
            );
            return;
        }
        let mark = chat.mark();
        let logs_before = chat.logs();
        let (end, _) = match chat.start(self.timings.start).await {
            Ok(started) => started,
            Err(error) => {
                self.report.fail(step, error);
                return;
            }
        };
        if end != StartEnd::Ready {
            self.start_failed(chat, &end, mark).await;
            return;
        }
        let new_log = chat
            .logs()
            .into_iter()
            .find(|log| !logs_before.contains(log));
        self.name_new_logs(chat, &logs_before, "probe-resume");
        let now = chat
            .ready_info(self.timings.stop)
            .await
            .and_then(|info| info.native_session_id);
        let resumable = chat
            .lists()
            .await
            .ok()
            .and_then(|lists| lists.capabilities)
            .is_some_and(|capabilities| capabilities.resume);
        let records = match &new_log {
            Some(log) => self.log_records(log, open_settled).await,
            None => Vec::new(),
        };
        match (native, now.as_deref()) {
            (Some(was), Some(is)) if was == is => {
                self.report
                    .pass(step, format!("the session {is} continued"));
            }
            (was, is) if !resumable || self.demo() => self.report.info(
                step,
                format!(
                    "a new session ({}) replaced {}: this agent can't continue earlier sessions",
                    is.unwrap_or("none"),
                    was.unwrap_or("none")
                ),
            ),
            (was, is) => {
                // claude-acp and codex-acp keep a session only from its first prompt, and the probe
                // never prompts: the agent refuses the resume, and Ghost falls back (5.3.2).
                let notice = chat
                    .watch
                    .with(|state| state.notices_since(&chat.chat_id, mark))
                    .into_iter()
                    .find(|notice| notice.starts_with("Could not continue the earlier session"));
                match (refused_open(&records), notice) {
                    (Some(answer), Some(notice)) => self.report.info(
                        step,
                        format!(
                            "the agent keeps no session it was never prompted in: it answered the resume {answer}, so Ghost started a new session ({}) and said so: \"{notice}\"",
                            is.unwrap_or("none")
                        ),
                    ),
                    _ => self.report.fail(
                        step,
                        format!(
                            "a new session ({}) replaced {}, although the agent can continue sessions",
                            is.unwrap_or("none"),
                            was.unwrap_or("none")
                        ),
                    ),
                }
            }
        }
        if new_log.is_some() {
            let method = logs::first_open(&records)
                .and_then(logs::message)
                .and_then(|msg| msg.get("method"))
                .and_then(Value::as_str)
                .unwrap_or("nothing");
            let reapplied: Vec<String> = logs::requests_of(
                &records,
                &[
                    "session/set_config_option",
                    "session/set_mode",
                    "session/set_model",
                ],
            )
            .filter_map(logs::message)
            .map(|msg| {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                format!(
                    "{} {}",
                    msg.get("method").and_then(Value::as_str).unwrap_or("?"),
                    ["configId", "modeId", "modelId", "value"]
                        .iter()
                        .filter_map(|key| params.get(key).map(|value| format!("{key}={value}")))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            })
            .collect();
            self.report.info(
                step,
                format!(
                    "opened with {method}; settings sent after it: {}",
                    if reapplied.is_empty() {
                        "none".to_owned()
                    } else {
                        reapplied.join("; ")
                    }
                ),
            );
        }
        let replayed: Vec<&'static str> = chat.watch.with(|state| {
            state
                .since(&chat.chat_id, mark)
                .filter_map(|seen| transcript_kind(&seen.event))
                .collect()
        });
        self.report.check(
            replayed.is_empty(),
            step,
            if replayed.is_empty() {
                "no transcript came back while the session opened".to_owned()
            } else {
                format!(
                    "transcript came back while the session opened: {}",
                    replayed.join(", ")
                )
            },
        );
        let continued = chat
            .watch
            .with(|state| state.notices_since(&chat.chat_id, mark))
            .into_iter()
            .find(|notice| notice.contains("continued this session while Ghost was away"));
        if let Some(notice) = continued {
            self.report.fail(
                step,
                format!("the replay differs from what was stored: {notice}"),
            );
        }
        if let Some(chosen) = chosen {
            match chat.lists().await {
                Ok(after) => {
                    let notices = chat
                        .watch
                        .with(|state| state.notices_since(&chat.chat_id, mark));
                    let (gone, changed) = resume_changes(&chosen.options, &after.options, &notices);
                    for text in gone {
                        self.report.info(step, text);
                    }
                    self.report.check(
                        changed.is_empty(),
                        step,
                        if changed.is_empty() {
                            "the options are back as they were".to_owned()
                        } else {
                            format!("options differ after the resume: {}", changed.join(", "))
                        },
                    );
                }
                Err(error) => self.report.fail(step, error),
            }
        }
    }

    /// An interrupt right after initialize ends the start with nothing left; the next start works.
    async fn interrupted_start_step(&mut self, chat: &ChatRun<'_>) {
        let step = "9 interrupt";
        if !matches!(chat.stop(self.timings.stop).await, Ok(true)) {
            self.report
                .fail(step, "the chat did not stop before the interrupted start");
            return;
        }
        let logs_before = chat.logs();
        // Native Claude has no persisted transcript before a prompt. Fresh starts still exercise
        // startup interruption without claiming that an unprompted native session can resume.
        let fresh = self.native_claude() || self.native_codex() || self.native_pi();
        if fresh {
            self.report.info(
                step,
                "native interruption and restart use fresh sessions because this free probe has no persisted transcript",
            );
        }
        if let Err(error) = self.headless.manager.start(&chat.chat_id, fresh).await {
            self.report.fail(step, error);
            return;
        }
        if !self.demo() {
            self.wait_for_initialize(chat, &logs_before).await;
        }
        if let Err(error) = self.headless.manager.interrupt(&chat.chat_id).await {
            self.report.fail(step, error);
            return;
        }
        let ended = chat
            .watch
            .until(self.timings.stop, |state| {
                match state.status(&chat.chat_id) {
                    Some(
                        status @ (SessionStatus::Stopped
                        | SessionStatus::Ready
                        | SessionStatus::Failed),
                    ) => Some(status),
                    _ => None,
                }
            })
            .await;
        match ended {
            Some(SessionStatus::Stopped) => self.report.pass(
                step,
                if self.demo() {
                    "an interrupt during the start stopped it"
                } else {
                    "an interrupt after initialize stopped the start"
                },
            ),
            Some(SessionStatus::Ready) => {
                self.report.info(
                    step,
                    "the start finished before the interrupt landed, so this was not checked",
                );
                let _ = chat.stop(self.timings.stop).await;
            }
            other => {
                self.report.fail(
                    step,
                    format!("the interrupted start left the chat {}", status_name(other)),
                );
                let _ = self.headless.manager.force_stop(&chat.chat_id).await;
            }
        }
        self.name_new_logs(chat, &logs_before, "probe-interrupted-start");
        if !self.demo() {
            self.leftovers(step, &[]).await;
        }
        let logs_before = chat.logs();
        let mark = chat.mark();
        let started = if fresh {
            match self.headless.manager.start(&chat.chat_id, true).await {
                Ok(()) => Ok(chat.settle(self.timings.start).await),
                Err(error) => Err(error),
            }
        } else {
            chat.start(self.timings.start).await
        };
        match started {
            Ok((StartEnd::Ready, _)) => {
                self.report.pass(step, "the next start was ready");
                self.name_new_logs(chat, &logs_before, "probe-restart");
            }
            Ok((end, _)) => self.start_failed(chat, &end, mark).await,
            Err(error) => self.report.fail(step, error),
        }
    }

    /// Waits until the new spawn's log holds the initialize reply (or the start ended).
    async fn wait_for_initialize(&self, chat: &ChatRun<'_>, logs_before: &[PathBuf]) {
        let deadline = Instant::now() + self.timings.start;
        while Instant::now() < deadline {
            if matches!(
                chat.status(),
                Some(SessionStatus::Ready | SessionStatus::Failed | SessionStatus::Stopped)
            ) {
                return;
            }
            let answered = chat
                .logs()
                .into_iter()
                .filter(|log| !logs_before.contains(log))
                .filter_map(|log| logs::read(&log).ok())
                .any(|read| {
                    logs::exchange(
                        &read.records,
                        if self.native_pi() {
                            "get_state"
                        } else {
                            "initialize"
                        },
                    )
                    .is_some()
                });
            if answered {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Stops what still runs, checks the table once more, and names the logs.
    async fn end(&mut self, chat: &ChatRun<'_>) {
        if !matches!(chat.stop(self.timings.stop).await, Ok(true)) {
            let _ = self.headless.manager.force_stop(&chat.chat_id).await;
            let _ = chat
                .wait_status(SessionStatus::Stopped, self.timings.stop)
                .await;
        }
        if !self.demo() {
            for log in chat.logs() {
                self.note_pid(&log);
                match logs::read(&log) {
                    Ok(read) => {
                        let complete = read.unreadable == 0
                            && !logs::notes(&read.records, "dropped")
                                .any(|count| count.as_u64().unwrap_or(0) > 0)
                            && !read.records.iter().any(|record| {
                                logs::direction(record) == Some("out")
                                    && record.get("truncated").is_some()
                            });
                        let free = complete && !logs::has_prompt(&read.records);
                        self.report.check(
                            free,
                            "free",
                            if free {
                                format!("no prompt was sent in {}", log.display())
                            } else {
                                format!(
                                    "a prompt was sent or its absence could not be verified in {}",
                                    log.display()
                                )
                            },
                        );
                    }
                    Err(error) => self.report.fail(
                        "free",
                        format!("could not verify {}: {error}", log.display()),
                    ),
                }
            }
            self.leftovers("end", &[]).await;
        }
        if self.named_logs.is_empty() {
            if self.demo() {
                self.report.note("The demo agent writes no protocol log.");
            }
            return;
        }
        let mut lines = vec!["Protocol logs, and the fixture each can become:".to_owned()];
        let fixture_dir = if self.native_codex() {
            "codex".into()
        } else if self.native_pi() {
            "pi".into()
        } else if self.native_claude() {
            "claude".into()
        } else {
            format!("acp/{}", self.spec.id)
        };
        for (log, name) in &self.named_logs {
            lines.push(format!(
                "  ghost-cli fixture {} --scenario {name} --out crates/ghost-drivers/tests/fixtures/{fixture_dir}/{name}.jsonl",
                log.display(),
            ));
        }
        self.report.note(lines.join("\n"));
    }
}

/// The install folder of an npm adapter: `<adapters>/<package without "@", "/" as "-">-<version>`
/// (spec 4).
pub fn adapter_folder(headless: &Headless, spec: &AgentSpec) -> Option<PathBuf> {
    match &spec.recipe {
        Recipe::NpmAdapter {
            package, version, ..
        } => Some(headless.adapters_dir().join(format!(
            "{}-{version}",
            package.trim_start_matches('@').replace('/', "-")
        ))),
        Recipe::Binary { .. }
        | Recipe::QualifiedBinary { .. }
        | Recipe::NativeBinary { .. }
        | Recipe::Builtin => None,
    }
}

/// Whether the adapter install's npm runs: `npm ci` with the flags the install gives it (spec 4).
fn npm_install_running(table: &[ProcInfo]) -> bool {
    table.iter().any(|info| {
        let has = |word: &str| info.argv.iter().any(|arg| arg == word);
        has("ci") && has("--omit=optional") && has("--ignore-scripts")
    })
}

/// Whether an adapter folder holds a complete install (its marker).
fn is_installed(folder: &Path) -> bool {
    folder.join("installed.json").is_file()
}

/// How long the start showed the download detail: from the first such detail after the first
/// `details_mark` details to the next detail.
fn download_time(chat: &ChatRun<'_>, details_mark: usize) -> Option<Duration> {
    chat.watch.with(|state| {
        let mut started: Option<Instant> = None;
        for (chat_id, at, detail) in state.details.iter().skip(details_mark) {
            if chat_id != &chat.chat_id {
                continue;
            }
            let downloading = detail
                .as_deref()
                .is_some_and(|detail| detail.starts_with("Downloading"));
            match (started, downloading) {
                (None, true) => started = Some(*at),
                (Some(begun), false) => return Some(at.duration_since(begun)),
                _ => {}
            }
        }
        None
    })
}

/// The bytes of every file under `folder` (links are not followed).
fn folder_size(folder: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() {
                total += entry.metadata().map_or(0, |meta| meta.len());
            }
        }
    }
    total
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// The platform CLI packages the adapters bundle as optional dependencies, wherever they sit in
/// `node_modules`: `@anthropic-ai/claude-agent-sdk-<platform>` and `@openai/codex-<platform>`.
pub fn bundled_clis(folder: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![folder.join("node_modules")];
    while let Some(modules) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&modules) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if name.starts_with('@') {
                let Ok(scoped) = std::fs::read_dir(&path) else {
                    continue;
                };
                for package in scoped.filter_map(Result::ok) {
                    let package_name = package.file_name().to_string_lossy().into_owned();
                    let bundled = (name == "@anthropic-ai"
                        && package_name.starts_with("claude-agent-sdk-"))
                        || (name == "@openai" && package_name.starts_with("codex-"));
                    if bundled {
                        found.push(format!("{name}/{package_name}"));
                    }
                    stack.push(package.path().join("node_modules"));
                }
            } else {
                stack.push(path.join("node_modules"));
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Whether what `npm prefix` printed names `folder`. npm masks every UUID it prints, so a folder
/// whose path holds one (a data root in a folder named after a session id) prints with `***`
/// there.
fn prefix_names_folder(printed: &str, folder: &Path) -> bool {
    if same_folder(Path::new(printed), folder) {
        return true;
    }
    let real = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    [real.as_path(), folder]
        .iter()
        .any(|path| mask_uuids(&path.to_string_lossy()) == printed)
}

/// `text` with every UUID as npm prints it: `***` (`@npmcli/redact` masks them, any case, as whole
/// words, since legacy npm tokens look like UUIDs).
fn mask_uuids(text: &str) -> String {
    let bytes = text.as_bytes();
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut masked = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while at < bytes.len() {
        let word_starts = at == 0 || !is_word(bytes[at - 1]);
        match uuid_end(bytes, at) {
            Some(end) if word_starts && bytes.get(end).is_none_or(|&next| !is_word(next)) => {
                // A UUID is ASCII, so both ends are character boundaries.
                masked.push_str(text.get(copied..at).unwrap_or_default());
                masked.push_str("***");
                copied = end;
                at = end;
            }
            _ => at += 1,
        }
    }
    masked.push_str(text.get(copied..).unwrap_or_default());
    masked
}

/// Where a UUID (8-4-4-4-12 hex digits) starting at `start` ends.
fn uuid_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut at = start;
    for (index, digits) in [8, 4, 4, 4, 12].into_iter().enumerate() {
        if index > 0 {
            if bytes.get(at) != Some(&b'-') {
                return None;
            }
            at += 1;
        }
        let group = bytes.get(at..at + digits)?;
        if !group.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        at += digits;
    }
    Some(at)
}

/// Whether two paths name the same folder (through links, as `npm prefix` prints the real one).
fn same_folder(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn text_at<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or("?")
}

fn auth_method_ids(result: &Value) -> String {
    let ids: Vec<&str> = result
        .get("authMethods")
        .and_then(Value::as_array)
        .map(|methods| {
            methods
                .iter()
                .filter_map(|method| method.get("id").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        "none".into()
    } else {
        ids.join(", ")
    }
}

/// Check the app-server handshake without reporting its home, account or host metadata.
fn codex_initialize(records: &[Value]) -> Result<(), String> {
    let (request, response) =
        logs::exchange(records, "initialize").ok_or("Codex did not answer initialize")?;
    if logs::message(response)
        .and_then(|message| message.pointer("/result/userAgent"))
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("Codex initialize did not return its user agent".into());
    }
    let position = |record: &Value| {
        records
            .iter()
            .position(|candidate| std::ptr::eq(candidate, record))
    };
    let initialized = records
        .iter()
        .position(|record| {
            logs::direction(record) == Some("out")
                && logs::message(record).is_some_and(|message| {
                    message["method"] == "initialized" && message.get("id").is_none()
                })
        })
        .ok_or("Codex was not sent the initialized notification")?;
    let request_pos = position(request).ok_or("Codex initialize request is missing")?;
    let response_pos = position(response).ok_or("Codex initialize reply is missing")?;
    if request_pos >= response_pos
        || response_pos >= initialized
        || logs::first_open(records)
            .and_then(position)
            .is_some_and(|open| open <= initialized)
    {
        return Err("Codex initialize, initialized and thread open are out of order".into());
    }
    Ok(())
}

fn codex_open(records: &[Value], native_id: &str) -> Result<String, String> {
    codex_initialize(records)?;
    let args = logs::note(records, "spawn")
        .and_then(|spawn| spawn["args"].as_array())
        .ok_or("Codex has no logged launch arguments")?;
    if args
        != &vec![
            Value::from("app-server"),
            Value::from("--listen"),
            Value::from("stdio://"),
        ]
    {
        return Err("Codex did not use the expected owned stdio app-server launch".into());
    }
    let request = logs::first_open(records)
        .and_then(logs::message)
        .ok_or("Codex sent no thread open")?;
    if request["method"] != "thread/start"
        || request
            .pointer("/params/cwd")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("Codex did not start a fresh thread in the selected folder".into());
    }
    let result = open_result(records).ok_or("Codex thread start did not succeed")?;
    if native_id.is_empty()
        || result.pointer("/thread/id").and_then(Value::as_str) != Some(native_id)
    {
        return Err("Codex persisted identity is not the returned thread ID".into());
    }
    let policy = result
        .get("approvalPolicy")
        .filter(|value| value.is_string() || value.is_object())
        .ok_or("Codex returned no effective approval policy")?;
    let sandbox = result
        .pointer("/sandbox/type")
        .and_then(Value::as_str)
        .ok_or("Codex returned no effective sandbox policy")?;
    if logs::exchange(records, "model/list")
        .and_then(|(_, answer)| logs::message(answer))
        .and_then(|message| message.pointer("/result/data"))
        .and_then(Value::as_array)
        .is_none()
    {
        return Err("Codex did not return its native model catalog".into());
    }
    Ok(format!(
        "owned stdio app-server opened thread {native_id}; effective approval policy {policy}, sandbox {sandbox}; native model catalog received"
    ))
}

/// Native initialize is a control response, not JSON-RPC. Inspect only fields needed by the
/// probe; its account and other opaque payloads never belong in the report.
fn pi_state_result(message: &Value) -> Result<&Value, String> {
    if message["type"] != "response"
        || message["command"] != "get_state"
        || message["success"] != true
    {
        return Err("Pi did not acknowledge get_state successfully".into());
    }
    let state = &message["data"];
    if state["sessionId"].as_str().is_none_or(str::is_empty)
        || !state["isStreaming"].is_boolean()
        || !state["isCompacting"].is_boolean()
        || !state["pendingMessageCount"].is_u64()
    {
        return Err("Pi get_state omitted required session/run-state fields".into());
    }
    Ok(state)
}

fn pi_open(records: &[Value], native_id: &str) -> Result<String, String> {
    let reply = logs::exchange(records, "get_state")
        .and_then(|(_, answer)| logs::message(answer))
        .ok_or("Pi session has no get_state response")?;
    let state = pi_state_result(reply)?;
    if state["sessionId"] != native_id {
        return Err("Pi state and Ready session ids differ".into());
    }
    let args = logs::note(records, "spawn")
        .and_then(|spawn| spawn["args"].as_array())
        .ok_or("Pi spawn args missing")?;
    if !args
        .windows(2)
        .any(|pair| pair[0] == "--mode" && pair[1] == "rpc")
        || !args
            .windows(2)
            .any(|pair| pair[0] == "--session-id" && pair[1] == native_id)
    {
        return Err("Pi did not start in RPC mode with its requested fresh session id".into());
    }
    Ok("Pi opened the requested native session through RPC".into())
}

fn native_initialize_result(message: &Value) -> Result<&Value, String> {
    if message["type"] != "control_response" || message["response"]["subtype"] != "success" {
        return Err(format!(
            "native initialize failed: {}",
            message
                .pointer("/response/error")
                .and_then(Value::as_str)
                .unwrap_or("no success response")
        ));
    }
    let result = message
        .pointer("/response/response")
        .filter(|value| value.is_object())
        .ok_or("native initialize has no response payload")?;
    if result["claude_code_version"]
        .as_str()
        .is_none_or(str::is_empty)
        || result["current_permission_mode"]
            .as_str()
            .is_none_or(str::is_empty)
        || !matches!(
            result["session_state"].as_str(),
            Some("idle" | "running" | "requires_action")
        )
    {
        return Err("native initialize did not report its version, permission mode and recognized session state".into());
    }
    Ok(result)
}

/// Prove a native session was opened through its expected launch argument and initialized. This
/// checks the open handshake only; a free probe cannot prove the model recalls a prior turn.
fn native_open(records: &[Value], native_id: &str, resume: Option<&str>) -> Result<String, String> {
    let initialized = logs::exchange(records, "initialize")
        .and_then(|(_, answer)| logs::message(answer))
        .ok_or("native session has no initialize response")?;
    native_initialize_result(initialized)?;
    let start = logs::note(records, "start").ok_or("native session has no start note")?;
    let resume_matches = match resume {
        Some(id) => start["resume"].as_str() == Some(id),
        None => start.get("resume").is_some_and(Value::is_null),
    };
    if native_id.is_empty()
        || start["native_session_id"].as_str() != Some(native_id)
        || !resume_matches
    {
        return Err("native session identity does not match its start note".into());
    }
    let args = logs::note(records, "spawn")
        .and_then(|spawn| spawn["args"].as_array())
        .ok_or("native session has no logged launch arguments")?;
    let args: Vec<&str> = args
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()
        .ok_or("native session has malformed launch arguments")?;
    let flag = resume.map_or_else(
        || format!("--session-id={native_id}"),
        |id| format!("--resume={id}"),
    );
    let identity: Vec<&str> = args
        .iter()
        .copied()
        .filter(|arg| arg.starts_with("--session-id=") || arg.starts_with("--resume="))
        .collect();
    if identity != [flag.as_str()] {
        return Err(format!(
            "native session was not launched with exactly {flag}"
        ));
    }
    let value = |name: &str| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1])
    };
    if value("--input-format") != Some("stream-json")
        || value("--output-format") != Some("stream-json")
        || value("--permission-prompt-tool") != Some("stdio")
        || !matches!(
            value("--permission-mode"),
            Some("default" | "acceptEdits" | "bypassPermissions")
        )
        || !["-p", "--verbose", "--include-partial-messages"]
            .iter()
            .all(|flag| args.contains(flag))
        || args
            .iter()
            .any(|arg| *arg == "--bare" || arg.starts_with("--system-prompt"))
    {
        return Err("native launch did not preserve the required stream transport and explicit permission mode".into());
    }
    Ok(format!(
        "native session {native_id} initialized through {flag} with explicit permission mode {}",
        value("--permission-mode").unwrap_or_default()
    ))
}

fn largest_commands_update(records: &[Value], native: bool) -> Option<usize> {
    if native {
        logs::native_events(records, "commands_changed")
            .filter_map(logs::message)
            .map(|message| message.to_string().len())
            .max()
    } else {
        logs::updates(records, "available_commands_update")
            .filter_map(logs::message)
            .map(|message| message.to_string().len())
            .max()
    }
}

/// The last mode the native CLI acknowledged. Unlike ACP, Claude need not publish a Mode option.
fn native_effective_mode(records: &[Value]) -> Option<&str> {
    let mut mode = logs::exchange(records, "initialize")
        .and_then(|(_, answer)| logs::message(answer))
        .and_then(|message| native_initialize_result(message).ok())
        .and_then(|result| result["current_permission_mode"].as_str());
    for request in logs::requests_of(records, &["set_permission_mode"]).filter_map(logs::message) {
        let Some(reply) = request
            .get("request_id")
            .and_then(|id| logs::reply(records, id))
            .and_then(logs::message)
        else {
            continue;
        };
        if reply["response"]["subtype"] == "success" {
            mode = reply
                .pointer("/response/response/mode")
                .and_then(Value::as_str)
                .or_else(|| request.pointer("/request/mode").and_then(Value::as_str));
        }
    }
    mode
}

/// Whether a log holds the first session open's outcome: its reply, and after an error reply the
/// `session/new` the driver falls back to.
fn open_settled(records: &[Value]) -> bool {
    let Some(reply) = logs::first_open(records)
        .and_then(logs::message)
        .and_then(|open| open.get("id"))
        .and_then(|id| logs::reply(records, id))
    else {
        return false;
    };
    let failed = logs::message(reply).is_some_and(|msg| msg.get("error").is_some());
    !failed || logs::request(records, "session/new").is_some()
}

/// The error the agent answered the first session open with, when the driver opened a new session
/// after it (spec 5.3.2): `-32002 (Resource not found: <id>)`, with the error's `data.details`
/// when it has them.
fn refused_open(records: &[Value]) -> Option<String> {
    let open = logs::message(logs::first_open(records)?)?;
    if open.get("method").and_then(Value::as_str) == Some("session/new") {
        return None;
    }
    let reply = logs::reply(records, open.get("id")?)?;
    let error = logs::message(reply)?.get("error")?;
    logs::request(records, "session/new")?;
    let code = error
        .get("code")
        .map_or_else(|| "?".to_owned(), Value::to_string);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(
        match error.pointer("/data/details").and_then(Value::as_str) {
            Some(details) => format!("{code} ({message}: {details})"),
            None => format!("{code} ({message})"),
        },
    )
}

/// The result of the first session open, when the agent answered it.
fn open_result(records: &[Value]) -> Option<&Value> {
    let open = logs::first_open(records)?;
    let id = logs::message(open)?.get("id")?;
    logs::message(logs::reply(records, id)?)?.get("result")
}

/// The option's current value, as `set_option` takes it.
fn current_value(option: &ConfigOption) -> ConfigValue {
    match &option.kind {
        ConfigKind::Select { current, .. } => ConfigValue::Choice {
            value: current.clone(),
        },
        ConfigKind::Toggle { current } => ConfigValue::Toggle { value: *current },
    }
}

fn current_text(option: &ConfigOption) -> String {
    match &option.kind {
        ConfigKind::Select { current, choices } => choices
            .iter()
            .find(|choice| &choice.value == current)
            .map_or_else(
                || current.clone(),
                |choice| format!("{} ({current})", choice.name),
            ),
        ConfigKind::Toggle { current } => current.to_string(),
    }
}

/// `3 options: mode (Mode, 3 choices), fast (ModelConfig, toggle)`.
fn options_text(options: &[ConfigOption]) -> String {
    if options.is_empty() {
        return "no options".into();
    }
    let parts: Vec<String> = options
        .iter()
        .map(|option| {
            let kind = match &option.kind {
                ConfigKind::Select { choices, .. } => format!("{} choices", choices.len()),
                ConfigKind::Toggle { .. } => "toggle".into(),
            };
            format!("{} ({:?}, {kind})", option.id, option.category)
        })
        .collect();
    format!("{} option(s): {}", options.len(), parts.join(", "))
}

/// The ids of options whose current value differs between two lists (or that one lacks).
fn changed_options(before: &[ConfigOption], after: &[ConfigOption]) -> Vec<String> {
    resume_changes(before, after, &[]).1
}

/// What a restart changed in the options: the earlier values the agent no longer offers, each with
/// Ghost's notice about it (5.3.2: Hermes 0.22.5 can't switch back to an `auto:` model, so the
/// session reopens on another), then every other change, which Ghost should not have made.
fn resume_changes(
    before: &[ConfigOption],
    after: &[ConfigOption],
    notices: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut gone = Vec::new();
    let mut changed = Vec::new();
    for option in before {
        let Some(now) = after.iter().find(|other| other.id == option.id) else {
            changed.push(format!("{} (gone)", option.id));
            continue;
        };
        if current_value(now) == current_value(option) {
            continue;
        }
        match no_longer_offered(option, now, notices) {
            Some(text) => gone.push(text),
            None => changed.push(format!(
                "{} ({} -> {})",
                option.id,
                current_text(option),
                current_text(now)
            )),
        }
    }
    for option in after {
        if !before.iter().any(|other| other.id == option.id) {
            changed.push(format!("{} (new)", option.id));
        }
    }
    (gone, changed)
}

/// When `before`'s value is not among `after`'s choices and Ghost said so in a notice: the line
/// that says it.
fn no_longer_offered(
    before: &ConfigOption,
    after: &ConfigOption,
    notices: &[String],
) -> Option<String> {
    let (
        ConfigKind::Select {
            current: earlier, ..
        },
        ConfigKind::Select { choices, .. },
    ) = (&before.kind, &after.kind)
    else {
        return None;
    };
    if choices.iter().any(|choice| &choice.value == earlier) {
        return None;
    }
    let notice = notices.iter().find(|notice| {
        notice.contains("isn't offered anymore") && notice.contains(&format!("({earlier})"))
    })?;
    Some(format!(
        "{}: the agent no longer offers {earlier} and runs {}; Ghost said so: \"{notice}\"",
        before.id,
        current_text(after)
    ))
}

/// The model to switch to: the cheapest-looking choice that is not current, else any other.
pub fn switch_target(model: &ConfigOption) -> Option<String> {
    let ConfigKind::Select { current, choices } = &model.kind else {
        return None;
    };
    let others: Vec<&ConfigChoice> = choices
        .iter()
        .filter(|choice| &choice.value != current)
        .collect();
    cheapest_of(&others)
        .or_else(|| others.first().copied())
        .map(|choice| choice.value.clone())
}

/// The cheapest-looking choice of a model option, whether or not it is current.
pub fn cheapest(model: &ConfigOption) -> Option<String> {
    let ConfigKind::Select { choices, .. } = &model.kind else {
        return None;
    };
    let choices: Vec<&ConfigChoice> = choices.iter().collect();
    cheapest_of(&choices).map(|choice| choice.value.clone())
}

/// The first choice carrying the cheapest marker any of them carries in its id or name, else the
/// first whose description says it is cheap (codex-acp's models name no size).
fn cheapest_of<'a>(choices: &[&'a ConfigChoice]) -> Option<&'a ConfigChoice> {
    let by_name = CHEAP_MARKERS.iter().find_map(|marker| {
        choices.iter().copied().find(|choice| {
            choice.value.to_lowercase().contains(marker)
                || choice.name.to_lowercase().contains(marker)
        })
    });
    by_name.or_else(|| {
        CHEAP_DESCRIPTIONS.iter().find_map(|marker| {
            choices.iter().copied().find(|choice| {
                choice
                    .description
                    .as_deref()
                    .is_some_and(|description| description.to_lowercase().contains(marker))
            })
        })
    })
}

/// The native modes an autonomy level may land on: the dialect's candidates (spec 5.5.2), the
/// demo agent's own table for the demo agent.
pub fn expected_modes(spec: &AgentSpec, level: Autonomy) -> Vec<Option<String>> {
    if spec.kind == DriverKind::Codex {
        return vec![Some(
            match level {
                Autonomy::FullAuto => "never",
                Autonomy::AutoEdit => "on-request",
                Autonomy::Ask => "untrusted",
            }
            .into(),
        )];
    }
    if spec.kind == DriverKind::Pi {
        return vec![None];
    }
    if spec.kind == DriverKind::Claude {
        return match level {
            Autonomy::FullAuto => {
                vec![Some("bypassPermissions".into()), Some("acceptEdits".into())]
            }
            Autonomy::AutoEdit => vec![Some("acceptEdits".into())],
            Autonomy::Ask => vec![Some("default".into())],
        };
    }
    if spec.kind == DriverKind::Mock {
        let mode = match level {
            Autonomy::FullAuto => "full_auto",
            Autonomy::AutoEdit => "auto_edit",
            Autonomy::Ask => "ask",
        };
        return vec![Some(mode.to_owned())];
    }
    dialect_for(spec.dialect)
        .autonomy_candidates(level)
        .into_iter()
        .map(|candidate| candidate.mode)
        .collect()
}

fn level_name(level: Autonomy) -> &'static str {
    match level {
        Autonomy::FullAuto => "Full auto",
        Autonomy::AutoEdit => "Auto-edit",
        Autonomy::Ask => "Ask",
    }
}

fn autonomy_text(level: Autonomy, state: &AutonomyState) -> String {
    let answers = match &state.auto_answer {
        AutoAnswer::Never => "Ghost does not auto-answer remaining permission requests".to_owned(),
        AutoAnswer::All => "answers every tool kind itself".to_owned(),
        AutoAnswer::Kinds { kinds } => format!(
            "answers {} itself",
            kinds
                .iter()
                .map(|kind| format!("{kind:?}").to_lowercase())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let mut text = format!(
        "{}: mode {}, {answers}",
        level_name(level),
        state.native_mode.as_deref().unwrap_or("unchanged")
    );
    if let Some(note) = &state.note {
        text.push_str(&format!(" (note: {note})"));
    }
    text
}

/// The kind of a transcript event (what a replayed open must not bring back).
fn transcript_kind(event: &GhostEvent) -> Option<&'static str> {
    match event {
        GhostEvent::UserMessageDelta { .. }
        | GhostEvent::MessageDelta { .. }
        | GhostEvent::ThoughtDelta { .. }
        | GhostEvent::ToolCall(_)
        | GhostEvent::ToolUpdate(_)
        | GhostEvent::PlanUpdated { .. } => Some(event.kind()),
        _ => None,
    }
}

pub(crate) fn status_name(status: Option<SessionStatus>) -> String {
    status.map_or_else(
        || "unknown".into(),
        |status| format!("{status:?}").to_lowercase(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(current: &str, choices: &[(&str, &str)]) -> ConfigOption {
        ConfigOption {
            id: "model".into(),
            name: "Model".into(),
            description: None,
            category: ConfigCategory::Model,
            kind: ConfigKind::Select {
                current: current.into(),
                choices: choices
                    .iter()
                    .map(|(value, name)| ConfigChoice {
                        value: (*value).into(),
                        name: (*name).into(),
                        description: None,
                        group: None,
                    })
                    .collect(),
            },
            pending: None,
        }
    }

    #[test]
    fn the_switch_goes_to_the_cheapest_model_that_is_not_current() {
        let claude = model(
            "default",
            &[
                ("default", "Default (Opus)"),
                ("sonnet", "Sonnet"),
                ("haiku", "Haiku"),
            ],
        );
        assert_eq!(switch_target(&claude).as_deref(), Some("haiku"));
        assert_eq!(cheapest(&claude).as_deref(), Some("haiku"));
        let codex = model(
            "gpt-5.1-codex",
            &[
                ("gpt-5.1-codex", "gpt-5.1-codex"),
                ("gpt-5.1-codex-mini", "gpt-5.1-codex-mini"),
            ],
        );
        assert_eq!(switch_target(&codex).as_deref(), Some("gpt-5.1-codex-mini"));
        // The cheapest is current already: any other model shows the switch.
        let mock = model(
            "mock-fast",
            &[("mock-fast", "Mock Fast"), ("mock-smart", "Mock Smart")],
        );
        assert_eq!(switch_target(&mock).as_deref(), Some("mock-smart"));
        assert_eq!(cheapest(&mock).as_deref(), Some("mock-fast"));
        assert_eq!(switch_target(&model("only", &[("only", "Only")])), None);
        // Hermes 0.22.5 offers its free tier among paid models (probe 2): free is cheapest.
        let hermes = model(
            "auto:nous/welcome",
            &[
                ("auto:nous/welcome", "Auto · nous/welcome"),
                ("nous:nous/welcome", "Nous · free tier · nous/welcome"),
                ("anthropic:claude-opus-5-5", "Anthropic · claude-opus-5-5"),
                (
                    "anthropic:claude-haiku-4-5-20251001",
                    "Anthropic · claude-haiku-4-5-20251001",
                ),
            ],
        );
        assert_eq!(cheapest(&hermes).as_deref(), Some("nous:nous/welcome"));
        assert_eq!(switch_target(&hermes).as_deref(), Some("nous:nous/welcome"));
    }

    /// The interrupt during a first install lands while npm runs: codex-acp's install (15 MB) was
    /// over before a fixed two-second wait ended, so step 9 never interrupted it.
    #[test]
    fn the_install_interrupt_waits_for_npm_to_run() {
        let proc = |pid: u32, argv: &str| ProcInfo {
            pid,
            ppid: 1,
            pgid: pid,
            sid: None,
            start: ghost_process::ptable::StartTime::Unknown,
            argv: argv.split(' ').map(str::to_owned).collect(),
        };
        let quiet = vec![
            proc(10, "/bin/zsh"),
            proc(11, "node /opt/homebrew/bin/npm ci"),
            proc(12, "node /opt/homebrew/bin/npm install --omit=optional"),
        ];
        assert!(!npm_install_running(&quiet));
        let mut installing = quiet;
        installing.push(proc(
            13,
            "node /opt/homebrew/bin/npm ci --omit=optional --ignore-scripts --no-audit --no-fund",
        ));
        assert!(npm_install_running(&installing));
    }

    /// codex-acp's models (codex 0.164.0) carry no size in their ids or names; their descriptions
    /// say which is the cheap one, as the real-agent probe showed.
    #[test]
    fn the_cheapest_model_is_found_by_its_description_when_its_name_says_nothing() {
        let mut codex = model(
            "gpt-6-astra",
            &[
                ("gpt-6.1-sol", "6.1 Sol"),
                ("gpt-6-astra", "6 Astra"),
                ("gpt-6-sol", "6 Sol"),
                ("gpt-6-luna", "6 Luna"),
                ("gpt-5.6-sol", "5.6 Sol"),
                ("gpt-5.6-terra", "5.6 Terra"),
                ("gpt-5.6-luna", "5.6 Luna"),
            ],
        );
        let descriptions = [
            "Latest workhorse model for coding and everyday work.",
            "Frontier intelligence for the most demanding work.",
            "Previous generation workhorse model.",
            "Fast and affordable model for easier tasks.",
            "Older generation workhorse model.",
            "Older balanced model for straightforward work.",
            "Older fast and efficient model.",
        ];
        if let ConfigKind::Select { choices, .. } = &mut codex.kind {
            for (choice, description) in choices.iter_mut().zip(descriptions) {
                choice.description = Some(description.into());
            }
        }
        assert_eq!(cheapest(&codex).as_deref(), Some("gpt-6-luna"));
        assert_eq!(switch_target(&codex).as_deref(), Some("gpt-6-luna"));
        // A name that says it wins over any description.
        let mut claude = model(
            "opus",
            &[
                ("opus", "Opus 5.5"),
                ("sonnet", "Sonnet 5.5"),
                ("haiku", "Haiku 5.5"),
            ],
        );
        if let ConfigKind::Select { choices, .. } = &mut claude.kind {
            choices[1].description = Some("Most efficient and affordable".into());
            choices[2].description = Some("Fastest for quick answers".into());
        }
        assert_eq!(cheapest(&claude).as_deref(), Some("haiku"));
    }

    #[test]
    fn expected_modes_follow_the_dialect_tables() {
        let hermes = ghost_agents::get("hermes").expect("hermes").clone();
        assert_eq!(
            expected_modes(&hermes, Autonomy::FullAuto),
            [Some("dont_ask".to_owned())]
        );
        assert_eq!(
            expected_modes(&hermes, Autonomy::Ask),
            [Some("default".to_owned())]
        );
        let claude = ghost_agents::get("claude-acp").expect("claude-acp").clone();
        assert_eq!(
            expected_modes(&claude, Autonomy::FullAuto),
            [
                Some("bypassPermissions".to_owned()),
                Some("acceptEdits".to_owned())
            ]
        );
        let mock = ghost_agents::get("mock").expect("mock").clone();
        assert_eq!(
            expected_modes(&mock, Autonomy::AutoEdit),
            [Some("auto_edit".to_owned())]
        );
    }

    #[test]
    fn changed_options_name_what_moved() {
        let before = vec![model("a", &[("a", "A"), ("b", "B")])];
        let after = vec![model("b", &[("a", "A"), ("b", "B")])];
        assert_eq!(changed_options(&before, &before), Vec::<String>::new());
        assert_eq!(changed_options(&before, &after), ["model (A (a) -> B (b))"]);
        assert_eq!(changed_options(&before, &[]), ["model (gone)"]);
    }

    #[test]
    fn bundled_clis_are_found_wherever_npm_put_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let modules = dir.path().join("node_modules");
        for package in [
            "@anthropic-ai/claude-agent-sdk",
            "@agentclientprotocol/claude-agent-acp/node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64",
            "@openai/codex",
            "@openai/codex-linux-x64",
            "zod",
        ] {
            std::fs::create_dir_all(modules.join(package)).expect("package folder");
        }
        assert_eq!(
            bundled_clis(dir.path()),
            [
                "@anthropic-ai/claude-agent-sdk-darwin-arm64",
                "@openai/codex-linux-x64"
            ]
        );
        assert!(bundled_clis(&dir.path().join("missing")).is_empty());
        std::fs::write(modules.join("zod/index.js"), vec![0u8; 1500]).expect("a file");
        assert_eq!(folder_size(dir.path()), 1500);
        assert_eq!(megabytes(1_500_000), "1.5 MB");
    }

    /// npm masks every UUID it prints (`@npmcli/redact`), so `npm prefix` in a folder whose path
    /// holds one prints `***` there: seen with a data root in a folder named after a session id.
    #[test]
    fn npm_prefix_names_the_folder_through_npms_uuid_mask() {
        let dir = tempfile::tempdir().expect("tempdir");
        let uuid = "d8fa469e-d838-43CE-8a75-2cd4fce641be";
        let folder = dir.path().join(uuid).join("adapters");
        std::fs::create_dir_all(&folder).expect("the folder");
        let real = std::fs::canonicalize(&folder)
            .expect("canonical")
            .display()
            .to_string();
        assert!(prefix_names_folder(&real, &folder));
        assert!(prefix_names_folder(&real.replace(uuid, "***"), &folder));
        assert!(!prefix_names_folder(&real.replace(uuid, "x***"), &folder));
        assert!(!prefix_names_folder(
            &real.replace("adapters", "***"),
            &folder
        ));
        // A UUID inside a longer word is not one npm masks.
        assert_eq!(
            mask_uuids(&format!("/a/{uuid}/b-{uuid}/c_{uuid}x/{uuid}")),
            format!("/a/***/b-***/c_{uuid}x/***")
        );
    }

    #[test]
    fn the_open_reply_and_auth_methods_read_from_a_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ghost-drivers/tests/fixtures/acp/hermes/legacy-models-modes.jsonl");
        let records = logs::read(&path).expect("fixture").records;
        let result = open_result(&records).expect("the open's result");
        assert_eq!(text_at(result, "/modes/currentModeId"), "default");
        let (_, answer) = logs::exchange(&records, "initialize").expect("initialize");
        let init = logs::message(answer)
            .and_then(|msg| msg.get("result"))
            .expect("result");
        assert_eq!(auth_method_ids(init), "openrouter, hermes-setup");
    }

    fn native_records(resume: Option<&str>) -> Vec<Value> {
        use serde_json::json;
        vec![
            json!({"d":"note","note":{"start":{"agent_id":"claude","dialect":"claude","native_session_id":"native-1","resume":resume}}}),
            json!({"d":"note","note":{"spawn":{"args":["-p","--input-format","stream-json","--output-format","stream-json","--verbose","--include-partial-messages","--permission-prompt-tool","stdio","--permission-mode","acceptEdits",if let Some(id) = resume {format!("--resume={id}")} else {"--session-id=native-1".into()}]}}}),
            json!({"d":"out","msg":{"type":"control_request","request_id":"init","request":{"subtype":"initialize"}}}),
            json!({"d":"in","msg":{"type":"control_response","response":{"subtype":"success","request_id":"init","response":{"claude_code_version":"2.1.310","session_state":"idle","current_permission_mode":"acceptEdits","capabilities":[]}}}}),
        ]
    }

    #[test]
    fn native_open_checks_transport_permissions_and_identity_instead_of_acp_opens() {
        let fresh = native_records(None);
        assert!(
            native_open(&fresh, "native-1", None)
                .expect("native open")
                .contains("--session-id=native-1")
        );
        let resumed = native_records(Some("native-1"));
        assert!(
            native_open(&resumed, "native-1", Some("native-1"))
                .expect("native resume")
                .contains("--resume=native-1")
        );
        assert!(native_open(&fresh, "another", None).is_err());
        assert!(native_open(&fresh, "native-1", Some("native-1")).is_err());
        let mut malformed_resume = fresh.clone();
        malformed_resume[0]["note"]["start"]["resume"] = serde_json::json!(false);
        assert!(native_open(&malformed_resume, "native-1", None).is_err());
        let mut missing_mode = fresh.clone();
        missing_mode[1]["note"]["spawn"]["args"][9] = serde_json::json!("--another-flag");
        assert!(native_open(&missing_mode, "native-1", None).is_err());
        let mut hidden_prompt = fresh.clone();
        hidden_prompt[1]["note"]["spawn"]["args"]
            .as_array_mut()
            .expect("args")
            .push(serde_json::json!("--system-prompt=hidden"));
        assert!(native_open(&hidden_prompt, "native-1", None).is_err());
        let mut malformed = fresh;
        malformed[3]["msg"]["response"]["response"]["session_state"] =
            serde_json::json!({"state":"idle"});
        assert!(native_open(&malformed, "native-1", None).is_err());
    }

    #[test]
    fn native_modes_follow_successful_controls_and_preserve_mode_on_refusal() {
        use serde_json::json;
        let mut records = native_records(None);
        assert_eq!(native_effective_mode(&records), Some("acceptEdits"));
        records.push(json!({"d":"out","msg":{"type":"control_request","request_id":"mode","request":{"subtype":"set_permission_mode","mode":"bypassPermissions"}}}));
        assert_eq!(
            native_effective_mode(&records),
            Some("acceptEdits"),
            "pending requests do not confirm a mode"
        );
        records.push(json!({"d":"in","msg":{"type":"control_response","response":{"subtype":"error","request_id":"mode","error":"policy"}}}));
        assert_eq!(native_effective_mode(&records), Some("acceptEdits"));
        records.push(json!({"d":"out","msg":{"type":"control_request","request_id":"ask","request":{"subtype":"set_permission_mode","mode":"default"}}}));
        records.push(json!({"d":"in","msg":{"type":"control_response","response":{"subtype":"success","request_id":"ask","response":{}}}}));
        assert_eq!(native_effective_mode(&records), Some("default"));
        let spec = ghost_agents::get("claude").expect("native Claude");
        assert_eq!(
            expected_modes(spec, Autonomy::Ask),
            [Some("default".into())]
        );
        assert_eq!(
            expected_modes(spec, Autonomy::AutoEdit),
            [Some("acceptEdits".into())]
        );
        assert_eq!(
            expected_modes(spec, Autonomy::FullAuto),
            [Some("bypassPermissions".into()), Some("acceptEdits".into())]
        );
    }

    #[test]
    fn command_update_measurement_uses_the_selected_protocol() {
        let native = serde_json::json!({"d":"in","msg":{"type":"system","subtype":"commands_changed","commands":[{"name":"usage"}]}});
        let acp = serde_json::json!({"d":"in","msg":{"method":"session/update","params":{"update":{"sessionUpdate":"available_commands_update","availableCommands":[]}}}});
        let native_size = native["msg"].to_string().len();
        let acp_size = acp["msg"].to_string().len();
        let records = vec![native, acp];
        assert_eq!(largest_commands_update(&records, true), Some(native_size));
        assert_eq!(largest_commands_update(&records, false), Some(acp_size));
    }

    #[test]
    fn codex_probe_checks_handshake_identity_and_owned_transport() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ghost-drivers/tests/fixtures/codex/basic.jsonl");
        let mut records = logs::read(&path).expect("fixture").records;
        records.insert(0, serde_json::json!({"d":"note","note":{"spawn":{"args":["app-server","--listen","stdio://"]}}}));
        assert!(codex_open(&records, "thread-1").is_ok());
        assert!(
            codex_open(&records, "tree-1").is_err(),
            "session tree IDs are not native thread IDs"
        );
        assert_eq!(logs::session_cwd(&records), Some("<CWD>"));
        let init = records
            .iter()
            .position(|record| record["msg"]["method"] == "initialized")
            .expect("initialized");
        let mut early_open = records.clone();
        early_open.swap(init, init + 1);
        assert!(codex_initialize(&early_open).is_err());
        records[0]["note"]["spawn"]["args"] =
            serde_json::json!(["app-server", "--listen", "ws://localhost:1234"]);
        assert!(codex_open(&records, "thread-1").is_err());
    }

    #[test]
    fn codex_turn_start_and_steer_cannot_be_reported_as_free() {
        for method in ["turn/start", "turn/steer"] {
            assert!(logs::has_prompt(&[
                serde_json::json!({"d":"out","msg":{"id":3,"method":method,"params":{"input":[]}}})
            ]));
        }
        assert!(!logs::has_prompt(&[
            serde_json::json!({"d":"out","msg":{"id":3,"method":"thread/start","params":{}}})
        ]));
    }

    #[test]
    fn pi_open_checks_native_state_and_launch_without_exposing_model_internals() {
        let records = vec![
            serde_json::json!({"d":"note","note":{"spawn":{"args":["--mode","rpc","--session-id","session"]}}}),
            serde_json::json!({"d":"out","msg":{"type":"get_state","id":"s"}}),
            serde_json::json!({"d":"in","msg":{"type":"response","command":"get_state","id":"s","success":true,"data":{
                "sessionId":"session","isStreaming":false,"isCompacting":false,"pendingMessageCount":0,
                "model":{"id":"model","provider":"test","baseUrl":"private"}}}}),
        ];
        assert!(pi_open(&records, "session").unwrap().contains("RPC"));
        assert!(pi_open(&records, "other").is_err());
        assert!(!pi_open(&records, "session").unwrap().contains("private"));
        let mut malformed = records[2]["msg"].clone();
        malformed["data"]["isStreaming"] = Value::Null;
        assert!(pi_state_result(&malformed).is_err());
    }
}
