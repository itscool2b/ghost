//! A fake ACP agent for ghost-cli's tests. Tests copy it as `hermes` (with `.exe` on Windows) into
//! a folder on the test env's `PATH`: it answers `--version` the way Hermes does, and under `acp`
//! speaks enough ACP for every probe step and every smoke scenario, so the CLI runs end to end
//! through a real process, the real ACP driver and real protocol logs, with no real agent. It
//! lives in this crate because a test can only find the binaries of its own package.
//!
//! - `initialize`: protocol version 1, load and resume, one terminal auth method.
//! - `session/new`, `session/load`, `session/resume`: Hermes' legacy `models` (`fake:large`,
//!   `fake:mini`) and `modes` (`default`, `accept_edits`, `dont_ask`), then three slash commands.
//!   Sessions persist in `$GHOST_FAKE_STATE`, one JSON file each, so a resume in a new process
//!   continues them; an unknown session answers -32002. `GHOST_FAKE_OPEN_DELAY_MS` delays every
//!   open's reply, which gives an interrupted start its window.
//! - `session/set_mode`, `session/set_model`, `session/set_config_option` (`[]`, as Hermes).
//! - `session/prompt`, by its text: the smoke scenarios' prompts (`Reply with exactly: ok`, the two
//!   files, counting to 300 until cancelled, the last number, the files created); anything else
//!   gets `fake reply`. In the `default` mode a file is written only after the client allows it.
//!   Usage is the session's running total, as Hermes reports it.
//! - `session/cancel` ends a running prompt `cancelled`; stdin's end ends the process.
//!
//! Variables turn on what real agents did in the probes (spec 10.2), so the CLI's handling of it
//! is tested here:
//!
//! - `GHOST_FAKE_SET_DELAY_MS`: settings changes answer this late (a real agent takes tens of
//!   milliseconds, long enough for a later change to arrive meanwhile).
//! - `GHOST_FAKE_HERMES_MODELS`: Hermes 0.22.5's models. The configured model is `auto:large`, and
//!   a model id names its provider only when the provider is one Hermes knows (`fake`), so a
//!   switch to `fake:mini` and back to `auto:large` leaves `fake:auto:large`, and `auto:large` is
//!   offered only while the session's provider is `auto`. `GHOST_FAKE_HERMES_MODELS=refuse`
//!   answers that switch back -32603 instead, as Hermes does from its `nous` provider, which
//!   checks the model.
//! - `GHOST_FAKE_FORGET_UNPROMPTED`: a session exists only from its first prompt, as claude-acp's
//!   and codex-acp's do: resuming or loading one never prompted answers -32002.
//! - `GHOST_FAKE_SLOW_CHILD_MS` (Unix): `initialize` starts a child in the agent's process group
//!   that exits this long after a SIGTERM, as claude-agent-acp's `claude auth status` does.
//! - `GHOST_FAKE_AUTH_REQUIRED`: no one is signed in: `session/new` answers -32000, as codex-acp
//!   does with an empty `CODEX_HOME`.
//! - `GHOST_FAKE_EFFORT`: an `effort` config option (`thought_level`) that follows the model, as
//!   codex-acp's does: `fake:mini` offers no `high`, so a switch to it from `high` sets `medium`
//!   (told in a `config_option_update`), and switching back keeps `medium`.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Pacing of the counting prompt: one number per tick.
const TICK: Duration = Duration::from_millis(15);

/// Starts the arguments of the slow child (`GHOST_FAKE_SLOW_CHILD_MS`).
const LINGER: &str = "--linger";

/// How long the slow child lives when nothing signals it, so a failed test leaves nothing behind
/// for long.
#[cfg(unix)]
const LINGER_LIFETIME: Duration = Duration::from_secs(60);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(LINGER) {
        return linger(args.get(2).and_then(|ms| ms.parse().ok()).unwrap_or(0));
    }
    if args.iter().any(|arg| arg == "--version") {
        println!(
            "{}",
            std::env::var("GHOST_FAKE_VERSION")
                .unwrap_or_else(|_| "Hermes Agent v0.22.5 (ghost-cli fake)".into())
        );
        return ExitCode::SUCCESS;
    }
    if args.get(1).map(String::as_str) != Some("acp") {
        eprintln!("usage: hermes acp");
        return ExitCode::FAILURE;
    }
    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fake agent: {error}");
            ExitCode::FAILURE
        }
    }
}

/// What the stdin reader hands over: a message, or the end of stdin.
type Incoming = Option<Value>;

fn serve() -> Result<()> {
    let (sender, receiver) = mpsc::channel::<Incoming>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(message) = serde_json::from_str::<Value>(&line)
                && sender.send(Some(message)).is_err()
            {
                return;
            }
        }
        let _ = sender.send(None);
    });
    eprintln!("INFO fake agent ready");
    let mut agent = Agent::new()?;
    while let Ok(Some(message)) = receiver.recv() {
        if agent.handle(message, &receiver)? == Flow::Ended {
            break;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Going,
    /// stdin ended.
    Ended,
}

/// One session, as stored between processes.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Session {
    id: String,
    cwd: PathBuf,
    mode: String,
    model: String,
    files: Vec<String>,
    last_number: u64,
    input_tokens: u64,
    output_tokens: u64,
    /// Prompts the session ran.
    #[serde(default)]
    prompts: u64,
    /// The `effort` option's value (`GHOST_FAKE_EFFORT`).
    #[serde(default)]
    effort: String,
}

/// What real agents did in the probes, each turned on by a variable (see the module docs).
#[derive(Debug, Default)]
struct Quirks {
    set_delay: Duration,
    hermes_models: bool,
    /// `GHOST_FAKE_HERMES_MODELS=refuse`.
    refuse_switch_back: bool,
    forget_unprompted: bool,
    slow_child: Option<u64>,
    effort: bool,
    auth_required: bool,
}

impl Quirks {
    fn from_env() -> Quirks {
        let millis = |name: &str| -> Option<u64> { std::env::var(name).ok()?.parse().ok() };
        let on = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        Quirks {
            set_delay: millis("GHOST_FAKE_SET_DELAY_MS")
                .map_or(Duration::ZERO, Duration::from_millis),
            hermes_models: on("GHOST_FAKE_HERMES_MODELS"),
            refuse_switch_back: std::env::var("GHOST_FAKE_HERMES_MODELS")
                .is_ok_and(|value| value == "refuse"),
            forget_unprompted: on("GHOST_FAKE_FORGET_UNPROMPTED"),
            slow_child: millis("GHOST_FAKE_SLOW_CHILD_MS"),
            effort: on("GHOST_FAKE_EFFORT"),
            auth_required: on("GHOST_FAKE_AUTH_REQUIRED"),
        }
    }
}

struct Agent {
    state: PathBuf,
    open_delay: Duration,
    quirks: Quirks,
    sessions: BTreeMap<String, Session>,
    next_request: u64,
}

impl Agent {
    fn new() -> Result<Agent> {
        let state = std::env::var_os("GHOST_FAKE_STATE")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("ghost-cli-fake-agent-state"));
        std::fs::create_dir_all(&state)?;
        let open_delay = std::env::var("GHOST_FAKE_OPEN_DELAY_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .map_or(Duration::ZERO, Duration::from_millis);
        Ok(Agent {
            state,
            open_delay,
            quirks: Quirks::from_env(),
            sessions: BTreeMap::new(),
            next_request: 1,
        })
    }

    fn handle(&mut self, message: Value, incoming: &Receiver<Incoming>) -> Result<Flow> {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (method, id) {
            (Some("session/prompt"), Some(id)) => self.prompt(&id, &params, incoming),
            (Some(method), Some(id)) => {
                self.request(method, &id, &params)?;
                Ok(Flow::Going)
            }
            // A cancel while no prompt runs, or another notification: nothing to do.
            (Some(_), None) | (None, _) => Ok(Flow::Going),
        }
    }

    fn request(&mut self, method: &str, id: &Value, params: &Value) -> Result<()> {
        match method {
            "initialize" => {
                if let Some(millis) = self.quirks.slow_child {
                    slow_child(millis)?;
                }
                reply(
                    id,
                    json!({
                        "protocolVersion": 1,
                        "agentCapabilities": {
                            "loadSession": true,
                            "promptCapabilities": {"image": false},
                            "sessionCapabilities": {"resume": {}}
                        },
                        "authMethods": [{
                            "id": "hermes-setup",
                            "name": "Configure the fake",
                            "type": "terminal",
                            "args": ["--setup"]
                        }],
                        "agentInfo": {"name": "fake-hermes", "version": "0.22.5"}
                    }),
                )
            }
            "session/new" => {
                std::thread::sleep(self.open_delay);
                if self.quirks.auth_required {
                    return fail(id, -32000, "Authentication required");
                }
                let session = Session {
                    id: format!("fake-{}-{}", std::process::id(), self.sessions.len() + 1),
                    cwd: PathBuf::from(text(params, "cwd")),
                    mode: "default".into(),
                    model: if self.quirks.hermes_models {
                        "auto:large".into()
                    } else {
                        "fake:large".into()
                    },
                    effort: if self.quirks.effort {
                        "high".into()
                    } else {
                        String::new()
                    },
                    ..Session::default()
                };
                self.save(&session)?;
                let mut result = self.offers(&session);
                result["sessionId"] = json!(session.id);
                reply(id, result)?;
                commands(&session.id)?;
                self.sessions.insert(session.id.clone(), session);
                Ok(())
            }
            "session/load" | "session/resume" => {
                std::thread::sleep(self.open_delay);
                let session_id = text(params, "sessionId");
                let found = self
                    .load(&session_id)
                    .filter(|session| !self.quirks.forget_unprompted || session.prompts > 0);
                let Some(mut session) = found else {
                    return fail(id, -32002, &format!("Resource not found: {session_id}"));
                };
                session.cwd = PathBuf::from(text(params, "cwd"));
                self.save(&session)?;
                reply(id, self.offers(&session))?;
                commands(&session.id)?;
                self.sessions.insert(session.id.clone(), session);
                Ok(())
            }
            "session/set_mode" => {
                std::thread::sleep(self.quirks.set_delay);
                self.change(id, params, |session, params| {
                    session.mode = text(params, "modeId");
                    json!({})
                })
            }
            "session/set_model" => {
                std::thread::sleep(self.quirks.set_delay);
                let (hermes_models, effort) = (self.quirks.hermes_models, self.quirks.effort);
                let Some(session) = self.sessions.get_mut(&text(params, "sessionId")) else {
                    return fail(id, -32602, "unknown session");
                };
                let picked = text(params, "modelId");
                let names_no_provider = !picked.starts_with("fake:");
                if self.quirks.refuse_switch_back
                    && names_no_provider
                    && !session.model.starts_with("auto:")
                {
                    return fail(id, -32603, "Internal error");
                }
                session.model = if hermes_models {
                    hermes_model_switch(&session.model, &picked)
                } else {
                    picked
                };
                let follows =
                    effort && !effort_choices(&session.model).contains(&session.effort.as_str());
                if follows {
                    session.effort = "medium".into();
                }
                let session = session.clone();
                self.save(&session)?;
                if effort {
                    update(
                        &session.id,
                        json!({"sessionUpdate": "config_option_update",
                               "configOptions": [effort_option(&session)]}),
                    )?;
                }
                reply(id, json!({}))
            }
            "session/set_config_option" => {
                std::thread::sleep(self.quirks.set_delay);
                if self.quirks.effort && text(params, "configId") == "effort" {
                    return self.change(id, params, |session, params| {
                        session.effort = text(params, "value");
                        json!({"configOptions": [effort_option(session)]})
                    });
                }
                reply(id, json!({"configOptions": []}))
            }
            other => fail(id, -32601, &format!("method not found: {other}")),
        }
    }

    /// The legacy `models` and `modes` an open answers with.
    fn offers(&self, session: &Session) -> Value {
        let models = if self.quirks.hermes_models {
            // The configured model, offered while the session runs on its provider, then every
            // model of the providers Hermes knows, the current one kept in any case.
            let mut ids: Vec<&str> = Vec::new();
            if session.model.starts_with("auto:") {
                ids.push(&session.model);
            }
            ids.push("fake:mini");
            if !ids.contains(&session.model.as_str()) {
                ids.push(&session.model);
            }
            // Named as Hermes names them: the provider, then the model.
            ids.iter()
                .map(|id| {
                    let name = id.split_once(':').map_or_else(
                        || (*id).to_owned(),
                        |(provider, model)| format!("{provider} · {model}"),
                    );
                    json!({"modelId": id, "name": name})
                })
                .collect::<Vec<_>>()
        } else {
            vec![
                json!({"modelId": "fake:large", "name": "fake/large"}),
                json!({"modelId": "fake:mini", "name": "fake/mini"}),
            ]
        };
        let mut offers = json!({
            "models": {
                "availableModels": models,
                "currentModelId": session.model
            },
            "modes": {
                "currentModeId": session.mode,
                "availableModes": [
                    {"id": "default", "name": "Default"},
                    {"id": "accept_edits", "name": "Accept Edits"},
                    {"id": "dont_ask", "name": "Don't Ask"}
                ]
            }
        });
        if self.quirks.effort {
            offers["configOptions"] = json!([effort_option(session)]);
        }
        offers
    }

    /// A settings change of the session `params` names.
    fn change(
        &mut self,
        id: &Value,
        params: &Value,
        apply: impl FnOnce(&mut Session, &Value) -> Value,
    ) -> Result<()> {
        let session_id = text(params, "sessionId");
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return fail(id, -32602, "unknown session");
        };
        let result = apply(session, params);
        let session = session.clone();
        self.save(&session)?;
        reply(id, result)
    }

    fn prompt(
        &mut self,
        id: &Value,
        params: &Value,
        incoming: &Receiver<Incoming>,
    ) -> Result<Flow> {
        let session_id = text(params, "sessionId");
        let Some(mut session) = self.sessions.get(&session_id).cloned() else {
            fail(id, -32602, "unknown session")?;
            return Ok(Flow::Going);
        };
        let prompt: String = params
            .get("prompt")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        session.input_tokens += 10;
        session.prompts += 1;
        let mut stop = "end_turn";
        let mut said = String::new();
        if prompt.contains("Reply with exactly: ok") {
            for piece in ["o", "k"] {
                chunk(&session.id, piece)?;
                said.push_str(piece);
            }
        } else if prompt.contains("Count from 1 to 300") {
            for number in 1..=300 {
                let piece = format!("{number}\n");
                chunk(&session.id, &piece)?;
                said.push_str(&piece);
                session.last_number = number;
                match incoming.recv_timeout(TICK) {
                    Ok(Some(message)) if is_cancel(&message) => {
                        stop = "cancelled";
                        break;
                    }
                    Ok(Some(message)) => {
                        if self.side(message)? == Flow::Ended {
                            return Ok(Flow::Ended);
                        }
                    }
                    Ok(None) | Err(RecvTimeoutError::Disconnected) => return Ok(Flow::Ended),
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
        } else if prompt.contains("What was the last number you wrote") {
            said = session.last_number.to_string();
            chunk(&session.id, &said)?;
        } else if prompt.contains("aura-perm.txt") || prompt.contains("Create it now") {
            match self.write_file(&mut session, "aura-perm.txt", "", incoming)? {
                Written::Yes => said = "done.".into(),
                Written::Refused => said = "I did not create aura-perm.txt.".into(),
                Written::Cancelled => stop = "cancelled",
                Written::Ended => return Ok(Flow::Ended),
            }
            if !said.is_empty() {
                chunk(&session.id, &said)?;
            }
        } else if prompt.contains("aura-auto.txt") {
            match self.write_file(&mut session, "aura-auto.txt", "hi\n", incoming)? {
                Written::Yes => said = "Created aura-auto.txt.".into(),
                Written::Refused => said = "I did not create aura-auto.txt.".into(),
                Written::Cancelled => stop = "cancelled",
                Written::Ended => return Ok(Flow::Ended),
            }
            if !said.is_empty() {
                chunk(&session.id, &said)?;
            }
        } else if prompt.contains("Which files did you create") {
            said = session.files.join("\n");
            chunk(&session.id, &said)?;
        } else {
            said = "fake reply".into();
            chunk(&session.id, &said)?;
        }
        session.output_tokens += said.split_whitespace().count().max(1) as u64;
        self.save(&session)?;
        let usage =
            json!({"inputTokens": session.input_tokens, "outputTokens": session.output_tokens});
        self.sessions.insert(session.id.clone(), session);
        reply(id, json!({"stopReason": stop, "usage": usage}))?;
        Ok(Flow::Going)
    }

    /// Writes a file in the session's folder; in the `default` mode only once the client allows
    /// it.
    fn write_file(
        &mut self,
        session: &mut Session,
        name: &str,
        contents: &str,
        incoming: &Receiver<Incoming>,
    ) -> Result<Written> {
        let call = format!("call-{}", self.next_request);
        update(
            &session.id,
            json!({"sessionUpdate": "tool_call", "toolCallId": call, "title": format!("Write {name}"),
                   "kind": "edit", "status": "pending", "rawInput": {"path": name}}),
        )?;
        if session.mode == "default" {
            let request = format!("perm-{}", self.next_request);
            self.next_request += 1;
            send(&json!({
                "jsonrpc": "2.0",
                "id": request,
                "method": "session/request_permission",
                "params": {
                    "sessionId": session.id,
                    "toolCall": {"toolCallId": call, "title": format!("Write {name}"), "kind": "edit"},
                    "options": [
                        {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
                        {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
                    ]
                }
            }))?;
            let mut cancelled = false;
            let answer = loop {
                match incoming.recv() {
                    Ok(Some(message))
                        if message.get("method").is_none()
                            && message.get("id") == Some(&json!(request)) =>
                    {
                        break message;
                    }
                    Ok(Some(message)) if is_cancel(&message) => cancelled = true,
                    Ok(Some(message)) => {
                        if self.side(message)? == Flow::Ended {
                            return Ok(Written::Ended);
                        }
                    }
                    Ok(None) | Err(_) => return Ok(Written::Ended),
                }
            };
            let allowed = answer
                .pointer("/result/outcome/optionId")
                .and_then(Value::as_str)
                == Some("allow");
            if !allowed {
                update(
                    &session.id,
                    json!({"sessionUpdate": "tool_call_update", "toolCallId": call, "status": "failed"}),
                )?;
                return Ok(if cancelled {
                    Written::Cancelled
                } else {
                    Written::Refused
                });
            }
        } else {
            self.next_request += 1;
        }
        std::fs::write(session.cwd.join(name), contents)?;
        if !session.files.iter().any(|file| file == name) {
            session.files.push(name.to_owned());
        }
        update(
            &session.id,
            json!({"sessionUpdate": "tool_call_update", "toolCallId": call, "status": "completed"}),
        )?;
        Ok(Written::Yes)
    }

    /// A message that arrives while a prompt runs: settings changes are answered, the rest is
    /// ignored.
    fn side(&mut self, message: Value) -> Result<Flow> {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let (Some(method), Some(id)) = (method, message.get("id").cloned()) {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            if method == "session/prompt" {
                fail(&id, -32603, "a prompt is already running")?;
            } else {
                self.request(&method, &id, &params)?;
            }
        }
        Ok(Flow::Going)
    }

    fn path(&self, session_id: &str) -> PathBuf {
        let safe: String = session_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.state.join(format!("{safe}.json"))
    }

    fn save(&self, session: &Session) -> Result<()> {
        std::fs::write(self.path(&session.id), serde_json::to_string(session)?)?;
        Ok(())
    }

    fn load(&self, session_id: &str) -> Option<Session> {
        let text = std::fs::read_to_string(self.path(session_id)).ok()?;
        serde_json::from_str(&text).ok()
    }
}

enum Written {
    Yes,
    Refused,
    Cancelled,
    Ended,
}

/// The efforts a model offers (`GHOST_FAKE_EFFORT`): `fake:mini` stops at `medium`.
fn effort_choices(model: &str) -> &'static [&'static str] {
    if model == "fake:mini" {
        &["low", "medium"]
    } else {
        &["low", "medium", "high"]
    }
}

/// The `effort` config option of a session (`GHOST_FAKE_EFFORT`).
fn effort_option(session: &Session) -> Value {
    let options: Vec<Value> = effort_choices(&session.model)
        .iter()
        .map(|value| {
            let mut name = (*value).to_owned();
            if let Some(first) = name.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            json!({"value": value, "name": name})
        })
        .collect();
    json!({"id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
           "currentValue": session.effort, "options": options})
}

/// Hermes 0.22.5's `session/set_model`: the part before the first colon names the provider only
/// when it is one Hermes knows; otherwise the whole id is a model of the current provider.
fn hermes_model_switch(current: &str, picked: &str) -> String {
    match picked.split_once(':') {
        Some(("fake", _)) => picked.to_owned(),
        _ => {
            let provider = current
                .split_once(':')
                .map_or("auto", |(provider, _)| provider);
            format!("{provider}:{picked}")
        }
    }
}

/// Starts the slow child (`GHOST_FAKE_SLOW_CHILD_MS`) in this process's group, with no pipes of
/// ours, so it outlives this process until the group is signalled.
#[cfg(unix)]
fn slow_child(millis: u64) -> Result<()> {
    std::process::Command::new(std::env::current_exe()?)
        .args([LINGER, &millis.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(not(unix))]
fn slow_child(_millis: u64) -> Result<()> {
    Ok(())
}

/// The slow child: exits `millis` after a SIGTERM, or after [`LINGER_LIFETIME`].
#[cfg(unix)]
fn linger(millis: u64) -> ExitCode {
    use tokio::signal::unix::{SignalKind, signal};
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::FAILURE;
    };
    runtime.block_on(async move {
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            return;
        };
        tokio::select! {
            _ = terminate.recv() => tokio::time::sleep(Duration::from_millis(millis)).await,
            () = tokio::time::sleep(LINGER_LIFETIME) => {}
        }
    });
    ExitCode::SUCCESS
}

#[cfg(not(unix))]
fn linger(_millis: u64) -> ExitCode {
    ExitCode::SUCCESS
}

fn commands(session_id: &str) -> Result<()> {
    update(
        session_id,
        json!({"sessionUpdate": "available_commands_update", "availableCommands": [
            {"name": "help", "description": "Show what the fake can do"},
            {"name": "model", "description": "Switch the model", "input": {"hint": "model id"}},
            {"name": "reset", "description": "Start over"}
        ]}),
    )
}

fn is_cancel(message: &Value) -> bool {
    message.get("method").and_then(Value::as_str) == Some("session/cancel")
}

fn text(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn chunk(session_id: &str, piece: &str) -> Result<()> {
    update(
        session_id,
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": piece}}),
    )
}

fn update(session_id: &str, update: Value) -> Result<()> {
    send(&json!({"jsonrpc": "2.0", "method": "session/update",
                 "params": {"sessionId": session_id, "update": update}}))
}

fn reply(id: &Value, result: Value) -> Result<()> {
    send(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn fail(id: &Value, code: i64, message: &str) -> Result<()> {
    send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
}

fn send(message: &Value) -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{message}")?;
    out.flush()?;
    Ok(())
}
