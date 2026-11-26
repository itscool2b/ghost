//! Dialect hooks: `AcpDialect` and the generic, claude-acp, codex-acp and hermes dialects:
//! autonomy candidates, usage scope, idle-only mode changes, replay summaries, sign-in commands,
//! start failure hints, and agent-specific requests and notifications.
//!
//! | | `hermes` | `claude-acp` | `codex-acp` | `generic` |
//! |---|---|---|---|---|
//! | usage scope | Session | Turn | Turn | Session |
//! | mode change only while idle | yes | no | no | no |
//! | replay may hold unseen work | yes | no | no | no |
//! | auth commands | launch command plus the method's args | `--cli` methods as the user's `claude` | fallback `codex login` | launch command plus the method's args |
//! | start failure hint | `windows_breakaway_unavailable` | none | none | none |
//! | open not found | none | none | -32603 "no rollout found" | none |

use std::ffi::OsStr;

use ghost_process::{EnvMap, LaunchSpec};
use ghost_protocol::session::AuthCommand;
use ghost_protocol::{AutoAnswer, Autonomy, ToolKind, UsageScope};

use super::callbacks::{self, UiCall};
use super::wire;
use crate::driver::DriverEvent;
use crate::jsonrpc::{RpcError, codes};

/// How one agent family speaks ACP beyond the schema.
pub trait AcpDialect: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    /// The requested level as ordered candidates; the driver applies the first that works and
    /// appends a last resort itself.
    fn autonomy_candidates(&self, requested: Autonomy) -> Vec<AutonomyCandidate>;
    fn prompt_usage_scope(&self) -> UsageScope;
    fn prompt_usage(&self, result: &serde_json::Value) -> Option<wire::WireUsage> {
        result.get("usage").and_then(wire::WireUsage::parse)
    }
    fn form_elicitation(&self) -> bool {
        false
    }
    fn autonomy_levels(&self) -> Vec<Autonomy> {
        vec![Autonomy::FullAuto, Autonomy::AutoEdit, Autonomy::Ask]
    }
    fn mode_change_requires_idle(&self) -> bool {
        false
    }
    /// Whether a refused mode change is worth sending again after a short wait: the agent was
    /// still applying an earlier change.
    fn mode_change_retryable(&self, _error: &RpcError) -> bool {
        false
    }
    /// Keep a `ReplaySummary` while load or resume replays.
    fn replay_may_hold_unseen_work(&self) -> bool {
        false
    }
    /// A sign-in command the user runs with their own binary, for one advertised auth method.
    fn auth_command(
        &self,
        method: &wire::AuthMethod,
        launch: &LaunchSpec,
        env: &EnvMap,
    ) -> Option<AuthCommand>;
    /// Fallback commands when the agent advertises none (codex-acp: `codex login`).
    fn fallback_auth_commands(&self, _launch: &LaunchSpec, _env: &EnvMap) -> Vec<AuthCommand> {
        Vec::new()
    }
    /// A sentence added to a start failure whose stderr tail matches (Hermes on Windows).
    fn start_failure_hint(&self, _stderr_tail: &str) -> Option<String> {
        None
    }
    /// An error answer to `session/resume` or `session/load` that says the session does not exist,
    /// beyond -32002, -32601 and -32602: the reason the fallback's notice gives, or `None`.
    fn open_not_found(&self, _error: &RpcError) -> Option<String> {
        None
    }
    fn on_request(&self, _method: &str, _params: &serde_json::Value) -> DialectReply {
        DialectReply::NotMine
    }
    fn on_notification(&self, _method: &str, _params: &serde_json::Value) -> Vec<DriverEvent> {
        Vec::new()
    }
    /// How notes and notices name the agent; `None` uses its `agentInfo` or the agent id.
    fn agent_label(&self) -> Option<&'static str> {
        None
    }
}

/// One way to reach a requested autonomy level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomyCandidate {
    /// The agent mode to switch to; `None` changes no mode.
    pub mode: Option<String>,
    pub auto_answer: AutoAnswer,
    pub note: Option<String>,
}

/// What a dialect makes of an agent request Ghost does not handle itself.
#[derive(Debug, Clone, PartialEq)]
pub enum DialectReply {
    NotMine,
    Respond(serde_json::Value),
    Error { code: i64, message: String },
    Deferred(Box<UiCall>),
}

/// The dialect for `id`; unknown ids get the generic dialect.
pub fn dialect_for(id: &str) -> Box<dyn AcpDialect> {
    match id {
        "hermes" => Box::new(Hermes),
        "claude-acp" => Box::new(ClaudeAcp),
        "codex-acp" => Box::new(CodexAcp),
        "cursor" => Box::new(Vendor("cursor")),
        "omp" => Box::new(Vendor("omp")),
        "antigravity" => Box::new(Vendor("antigravity")),
        "grok" => Box::new(Vendor("grok")),
        "opencode" | "opencode-v2" => Box::new(Vendor("opencode")),
        _ => Box::new(Generic),
    }
}

/// ACP v1 families whose protocol remains the generic client's, with explicit UI extensions.
#[derive(Debug, Clone, Copy)]
pub struct Vendor(&'static str);

impl AcpDialect for Vendor {
    fn id(&self) -> &'static str {
        self.0
    }
    fn autonomy_candidates(&self, requested: Autonomy) -> Vec<AutonomyCandidate> {
        if self.0 == "omp" {
            return vec![candidate(
                None,
                AutoAnswer::All,
                Some(
                    "oh-my-pi controls tool execution. Its ACP interface does not provide a general Ask or Auto-edit boundary; extension questions and plan approval always wait for you.",
                ),
            )];
        }
        Generic.autonomy_candidates(requested)
    }
    fn autonomy_levels(&self) -> Vec<Autonomy> {
        if self.0 == "omp" {
            vec![Autonomy::FullAuto]
        } else {
            Generic.autonomy_levels()
        }
    }
    fn prompt_usage_scope(&self) -> UsageScope {
        if matches!(self.0, "omp" | "grok") {
            UsageScope::Turn
        } else {
            UsageScope::Session
        }
    }
    fn form_elicitation(&self) -> bool {
        self.0 == "omp"
    }
    fn prompt_usage(&self, result: &serde_json::Value) -> Option<wire::WireUsage> {
        if self.0 != "grok" {
            return result.get("usage").and_then(wire::WireUsage::parse);
        }
        // Grok's pinned PromptUsage is a prompt-wide ledger under _meta.usage. Its input
        // includes both cache buckets; sibling _meta token fields describe only the last call.
        let value = result.pointer("/_meta/usage")?;
        let read = value
            .get("cachedReadTokens")
            .map(serde_json::Value::as_u64)
            .unwrap_or(Some(0))?;
        let write = value
            .get("cacheCreationTokens")
            .map(serde_json::Value::as_u64)
            .unwrap_or(Some(0))?;
        Some(wire::WireUsage {
            input: value
                .get("inputTokens")?
                .as_u64()?
                .checked_sub(read.checked_add(write)?)?,
            output: value.get("outputTokens")?.as_u64()?,
            cached_read: Some(read),
            cached_write: Some(write),
            thought: value
                .get("reasoningTokens")
                .and_then(serde_json::Value::as_u64),
        })
    }
    fn on_request(&self, method: &str, params: &serde_json::Value) -> DialectReply {
        match self.0 {
            "cursor" => callbacks::request(method, params, true),
            "omp" => callbacks::request(method, params, false),
            _ => DialectReply::NotMine,
        }
    }
    fn auth_command(
        &self,
        method: &wire::AuthMethod,
        launch: &LaunchSpec,
        env: &EnvMap,
    ) -> Option<AuthCommand> {
        if self.0 == "cursor" && method.id == "cursor_login" {
            return Some(command(
                method,
                vec![
                    launch.program.to_string_lossy().into_owned(),
                    "login".into(),
                ],
            ));
        }
        Generic.auth_command(method, launch, env)
    }
    fn agent_label(&self) -> Option<&'static str> {
        Some(match self.0 {
            "cursor" => "Cursor",
            "omp" => "oh-my-pi",
            "antigravity" => "Antigravity",
            "grok" => "Grok Build",
            _ => "OpenCode",
        })
    }
}

/// The note claude-acp's Full auto carries when the user's settings removed bypass mode.
pub const CLAUDE_NO_BYPASS_NOTE: &str = "Claude Code's settings turn off bypass mode, so Full auto runs as Accept edits and Ghost asks before other actions.";

/// The note Hermes' Ask carries (recorded with Hermes 0.22.5): its `default` mode asks before
/// edits made with its file tools, while shell commands go to its own approval model first.
pub const HERMES_ASK_NOTE: &str = "Hermes asks before it edits files. Shell commands go to its own safety check first, which can run harmless ones without asking.";

/// The note Codex's Ask carries (recorded with Codex 0.164.0 through codex-acp 2.2.2): the
/// read-only preset asks only when the model requests more access.
pub const CODEX_ASK_NOTE: &str = "Codex works read-only and asks when it needs to write or go online. A model may stop and say so instead of asking.";

/// What codex-acp's answer to the resume of a thread that has no rollout yet says (Codex 0.164.0).
pub const CODEX_NO_ROLLOUT: &str = "no rollout found";

/// The hint for a Hermes start that failed because Windows refused its gateway's breakaway.
pub const HERMES_BREAKAWAY_HINT: &str = "On Windows, Ghost can't let Hermes start its gateway yet. Run `hermes gateway ensure` in a terminal, then press Retry.";

/// What generic Auto-edit and the demo agent's Auto-edit answer on their own.
pub const EDIT_KINDS: [ToolKind; 6] = [
    ToolKind::Read,
    ToolKind::Search,
    ToolKind::Think,
    ToolKind::Edit,
    ToolKind::Delete,
    ToolKind::Move,
];

fn candidate(mode: Option<&str>, auto_answer: AutoAnswer, note: Option<&str>) -> AutonomyCandidate {
    AutonomyCandidate {
        mode: mode.map(str::to_owned),
        auto_answer,
        note: note.map(str::to_owned),
    }
}

/// The launch command plus `args`: the agent's own binary and arguments.
fn launch_command(launch: &LaunchSpec, args: &[String]) -> Vec<String> {
    let mut argv = vec![launch.program.to_string_lossy().into_owned()];
    argv.extend(
        launch
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned()),
    );
    argv.extend(args.iter().cloned());
    argv
}

/// The user's own binary an adapter drives: the launch's `set_env` value for `variable` (what
/// detection chose), else the resolved env's, else `program` on `PATH`.
fn user_binary(launch: &LaunchSpec, env: &EnvMap, variable: &str, program: &str) -> String {
    let from_launch = launch
        .set_env
        .iter()
        .find(|(name, _)| name.as_os_str() == OsStr::new(variable))
        .map(|(_, value)| value.to_string_lossy().into_owned());
    let from_env = || {
        env.get(&ghost_process::EnvKey::from(variable))
            .map(|value| value.to_string_lossy().into_owned())
    };
    from_launch
        .or_else(from_env)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| program.to_owned())
}

fn command(method: &wire::AuthMethod, argv: Vec<String>) -> AuthCommand {
    AuthCommand {
        label: method.name.clone(),
        argv,
        env: method.env.clone(),
    }
}

/// Any ACP agent: no mode changes; Full auto answers the eight tool kinds.
#[derive(Debug, Clone, Copy, Default)]
pub struct Generic;

impl AcpDialect for Generic {
    fn id(&self) -> &'static str {
        "generic"
    }

    fn autonomy_candidates(&self, requested: Autonomy) -> Vec<AutonomyCandidate> {
        vec![match requested {
            Autonomy::FullAuto => candidate(None, AutoAnswer::All, None),
            Autonomy::AutoEdit => candidate(
                None,
                AutoAnswer::Kinds {
                    kinds: EDIT_KINDS.to_vec(),
                },
                None,
            ),
            Autonomy::Ask => candidate(None, AutoAnswer::Never, None),
        }]
    }

    /// The schema documents session totals.
    fn prompt_usage_scope(&self) -> UsageScope {
        UsageScope::Session
    }

    fn auth_command(
        &self,
        method: &wire::AuthMethod,
        launch: &LaunchSpec,
        _env: &EnvMap,
    ) -> Option<AuthCommand> {
        terminal_method_command(method, launch)
    }
}

/// A terminal method as the launch command plus its args.
fn terminal_method_command(method: &wire::AuthMethod, launch: &LaunchSpec) -> Option<AuthCommand> {
    (method.kind == wire::AuthMethodKind::Terminal)
        .then(|| command(method, launch_command(launch, &method.args)))
}

/// Hermes: a viewer of its gateway daemon.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hermes;

impl AcpDialect for Hermes {
    fn id(&self) -> &'static str {
        "hermes"
    }

    fn autonomy_candidates(&self, requested: Autonomy) -> Vec<AutonomyCandidate> {
        vec![match requested {
            // Hermes' dangerous-command approvals, the set its own `--yolo` bypasses; sensitive-path
            // edit asks reach the user.
            Autonomy::FullAuto => candidate(
                Some("dont_ask"),
                AutoAnswer::Kinds {
                    kinds: vec![ToolKind::Execute],
                },
                None,
            ),
            Autonomy::AutoEdit => candidate(Some("accept_edits"), AutoAnswer::Never, None),
            Autonomy::Ask => candidate(Some("default"), AutoAnswer::Never, Some(HERMES_ASK_NOTE)),
        }]
    }

    fn prompt_usage_scope(&self) -> UsageScope {
        UsageScope::Session
    }

    /// Hermes answers -32603 and posts an agent message when its mode changes mid-turn.
    fn mode_change_requires_idle(&self) -> bool {
        true
    }

    /// Hermes 0.22.5 refuses a mode change that reaches its gateway before the previous one has
    /// landed (-32603 with `details: "stale_generation"`); the same change goes through once it
    /// has.
    fn mode_change_retryable(&self, error: &RpcError) -> bool {
        error.code == codes::INTERNAL_ERROR
            && error
                .data
                .as_ref()
                .and_then(|data| data.get("details"))
                .and_then(serde_json::Value::as_str)
                == Some("stale_generation")
    }

    /// Its gateway outlives the viewer, so a turn can finish while Ghost is away.
    fn replay_may_hold_unseen_work(&self) -> bool {
        true
    }

    /// `hermes-setup` `["--setup"]` becomes `hermes acp --setup`.
    fn auth_command(
        &self,
        method: &wire::AuthMethod,
        launch: &LaunchSpec,
        _env: &EnvMap,
    ) -> Option<AuthCommand> {
        terminal_method_command(method, launch)
    }

    fn start_failure_hint(&self, stderr_tail: &str) -> Option<String> {
        stderr_tail
            .contains("windows_breakaway_unavailable")
            .then(|| HERMES_BREAKAWAY_HINT.to_owned())
    }

    fn agent_label(&self) -> Option<&'static str> {
        Some("Hermes")
    }
}

/// claude-agent-acp, driving the user's own Claude Code.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeAcp;

impl AcpDialect for ClaudeAcp {
    fn id(&self) -> &'static str {
        "claude-acp"
    }

    fn autonomy_candidates(&self, requested: Autonomy) -> Vec<AutonomyCandidate> {
        match requested {
            Autonomy::FullAuto => vec![
                candidate(Some("bypassPermissions"), AutoAnswer::Never, None),
                candidate(
                    Some("acceptEdits"),
                    AutoAnswer::Never,
                    Some(CLAUDE_NO_BYPASS_NOTE),
                ),
            ],
            Autonomy::AutoEdit => vec![candidate(Some("acceptEdits"), AutoAnswer::Never, None)],
            Autonomy::Ask => vec![candidate(Some("default"), AutoAnswer::Never, None)],
        }
    }

    /// claude-acp resets its accumulated usage at each user turn.
    fn prompt_usage_scope(&self) -> UsageScope {
        UsageScope::Turn
    }

    /// Terminal methods whose args start with `--cli` run the user's `claude` with the rest:
    /// `claude auth login --claudeai`, `claude auth login --console`.
    fn auth_command(
        &self,
        method: &wire::AuthMethod,
        launch: &LaunchSpec,
        env: &EnvMap,
    ) -> Option<AuthCommand> {
        if method.kind != wire::AuthMethodKind::Terminal {
            return None;
        }
        let (first, rest) = method.args.split_first()?;
        if first != "--cli" {
            return None;
        }
        let mut argv = vec![user_binary(launch, env, "CLAUDE_CODE_EXECUTABLE", "claude")];
        argv.extend(rest.iter().cloned());
        Some(command(method, argv))
    }

    fn agent_label(&self) -> Option<&'static str> {
        Some("Claude Code")
    }
}

/// codex-acp, driving the user's own Codex.
#[derive(Debug, Clone, Copy, Default)]
pub struct CodexAcp;

impl AcpDialect for CodexAcp {
    fn id(&self) -> &'static str {
        "codex-acp"
    }

    fn autonomy_candidates(&self, requested: Autonomy) -> Vec<AutonomyCandidate> {
        vec![match requested {
            // Approval never, full access.
            Autonomy::FullAuto => candidate(Some("agent-full-access"), AutoAnswer::Never, None),
            Autonomy::AutoEdit => candidate(Some("workspace-write"), AutoAnswer::Never, None),
            Autonomy::Ask => candidate(Some("read-only"), AutoAnswer::Never, Some(CODEX_ASK_NOTE)),
        }]
    }

    fn prompt_usage_scope(&self) -> UsageScope {
        UsageScope::Turn
    }

    fn auth_command(
        &self,
        method: &wire::AuthMethod,
        launch: &LaunchSpec,
        _env: &EnvMap,
    ) -> Option<AuthCommand> {
        terminal_method_command(method, launch)
    }

    /// codex-acp advertises no terminal method: `codex login` with the user's binary.
    fn fallback_auth_commands(&self, launch: &LaunchSpec, env: &EnvMap) -> Vec<AuthCommand> {
        vec![AuthCommand {
            label: "Sign in to Codex".into(),
            argv: vec![
                user_binary(launch, env, "CODEX_PATH", "codex"),
                "login".into(),
            ],
            env: Vec::new(),
        }]
    }

    /// Codex writes a thread's rollout at its first turn, so it can't resume a thread never
    /// prompted: codex-acp answers -32603 "Internal error" with "no rollout found for thread id
    /// <id>" in `data.details`.
    fn open_not_found(&self, error: &RpcError) -> Option<String> {
        let details = error.data.as_ref()?.get("details")?.as_str()?;
        (error.code == codes::INTERNAL_ERROR && details.contains(CODEX_NO_ROLLOUT))
            .then(|| details.to_owned())
    }

    fn agent_label(&self) -> Option<&'static str> {
        Some("Codex")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use ghost_process::ProcessCwd;

    use super::*;

    fn launch(program: &str, args: &[&str], set_env: &[(&str, &str)]) -> LaunchSpec {
        LaunchSpec {
            label: "agent".into(),
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

    fn terminal(id: &str, args: &[&str]) -> wire::AuthMethod {
        wire::AuthMethod {
            id: id.into(),
            name: format!("{id} name"),
            description: None,
            kind: wire::AuthMethodKind::Terminal,
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: vec![("HERMES_SETUP".into(), "1".into())],
        }
    }

    fn modes(dialect: &dyn AcpDialect, level: Autonomy) -> Vec<(Option<String>, AutoAnswer)> {
        dialect
            .autonomy_candidates(level)
            .into_iter()
            .map(|candidate| (candidate.mode, candidate.auto_answer))
            .collect()
    }

    #[test]
    fn autonomy_candidates_per_dialect() {
        let some = |mode: &str| Some(mode.to_owned());
        assert_eq!(
            modes(&ClaudeAcp, Autonomy::FullAuto),
            [
                (some("bypassPermissions"), AutoAnswer::Never),
                (some("acceptEdits"), AutoAnswer::Never)
            ]
        );
        assert_eq!(
            ClaudeAcp.autonomy_candidates(Autonomy::FullAuto)[1]
                .note
                .as_deref(),
            Some(CLAUDE_NO_BYPASS_NOTE)
        );
        assert_eq!(
            modes(&ClaudeAcp, Autonomy::AutoEdit),
            [(some("acceptEdits"), AutoAnswer::Never)]
        );
        assert_eq!(
            modes(&ClaudeAcp, Autonomy::Ask),
            [(some("default"), AutoAnswer::Never)]
        );
        assert_eq!(
            modes(&CodexAcp, Autonomy::FullAuto),
            [(some("agent-full-access"), AutoAnswer::Never)]
        );
        assert_eq!(
            modes(&CodexAcp, Autonomy::AutoEdit),
            [(some("workspace-write"), AutoAnswer::Never)]
        );
        assert_eq!(
            modes(&CodexAcp, Autonomy::Ask),
            [(some("read-only"), AutoAnswer::Never)]
        );
        assert_eq!(
            modes(&Hermes, Autonomy::FullAuto),
            [(
                some("dont_ask"),
                AutoAnswer::Kinds {
                    kinds: vec![ToolKind::Execute]
                }
            )]
        );
        assert_eq!(
            modes(&Hermes, Autonomy::AutoEdit),
            [(some("accept_edits"), AutoAnswer::Never)]
        );
        assert_eq!(
            modes(&Hermes, Autonomy::Ask),
            [(some("default"), AutoAnswer::Never)]
        );
        // Both recorded in the paid scenarios: what each agent's Ask really covers.
        assert_eq!(
            Hermes.autonomy_candidates(Autonomy::Ask)[0].note.as_deref(),
            Some(HERMES_ASK_NOTE)
        );
        assert_eq!(
            CodexAcp.autonomy_candidates(Autonomy::Ask)[0]
                .note
                .as_deref(),
            Some(CODEX_ASK_NOTE)
        );
        assert_eq!(
            modes(&Generic, Autonomy::FullAuto),
            [(None, AutoAnswer::All)]
        );
        assert_eq!(
            modes(&Generic, Autonomy::AutoEdit),
            [(
                None,
                AutoAnswer::Kinds {
                    kinds: EDIT_KINDS.to_vec()
                }
            )]
        );
        assert_eq!(modes(&Generic, Autonomy::Ask), [(None, AutoAnswer::Never)]);
        // No dialect auto-answers through allow_always: auto answers are kinds, never options.
    }

    #[test]
    fn dialect_table() {
        for (id, scope, idle, replay) in [
            ("hermes", UsageScope::Session, true, true),
            ("claude-acp", UsageScope::Turn, false, false),
            ("codex-acp", UsageScope::Turn, false, false),
            ("omp", UsageScope::Turn, false, false),
            ("grok", UsageScope::Turn, false, false),
            ("cursor", UsageScope::Session, false, false),
            ("antigravity", UsageScope::Session, false, false),
            ("opencode-v2", UsageScope::Session, false, false),
            ("generic", UsageScope::Session, false, false),
            ("some-new-agent", UsageScope::Session, false, false),
        ] {
            let dialect = dialect_for(id);
            assert_eq!(dialect.prompt_usage_scope(), scope, "{id}");
            assert_eq!(dialect.mode_change_requires_idle(), idle, "{id}");
            assert_eq!(dialect.replay_may_hold_unseen_work(), replay, "{id}");
        }
        assert_eq!(dialect_for("some-new-agent").id(), "generic");
        assert_eq!(dialect_for("mock").id(), "generic");
    }

    #[test]
    fn grok_usage_ignores_last_call_fields_and_rejects_invalid_cache_totals() {
        let grok = dialect_for("grok");
        assert!(
            grok.prompt_usage(&serde_json::json!({"_meta":{"inputTokens":99,"outputTokens":4}}))
                .is_none()
        );
        for usage in [
            serde_json::json!({"inputTokens":1,"outputTokens":4,"cachedReadTokens":2}),
            serde_json::json!({"inputTokens":10,"outputTokens":4,"cacheCreationTokens":"bad"}),
        ] {
            assert!(
                grok.prompt_usage(&serde_json::json!({"_meta":{"usage":usage}}))
                    .is_none()
            );
        }
    }

    #[test]
    fn hermes_setup_runs_the_launch_command_with_the_methods_args() {
        let launch = launch("/opt/bin/hermes", &["acp"], &[]);
        let command = Hermes
            .auth_command(
                &terminal("hermes-setup", &["--setup"]),
                &launch,
                &EnvMap::new(),
            )
            .expect("a command");
        assert_eq!(command.argv, ["/opt/bin/hermes", "acp", "--setup"]);
        assert_eq!(command.label, "hermes-setup name");
        assert_eq!(command.env, [("HERMES_SETUP".to_owned(), "1".to_owned())]);
        let agent_method = wire::AuthMethod {
            kind: wire::AuthMethodKind::Agent,
            ..terminal("openrouter", &[])
        };
        assert_eq!(
            Hermes.auth_command(&agent_method, &launch, &EnvMap::new()),
            None,
            "agent methods are never called in M1a"
        );
    }

    #[test]
    fn claude_cli_methods_run_the_users_claude() {
        let launch = launch(
            "/opt/bin/node",
            &["/adapters/claude/dist/index.js"],
            &[("CLAUDE_CODE_EXECUTABLE", "/opt/claude/bin/claude")],
        );
        let env = EnvMap::new();
        let login = ClaudeAcp
            .auth_command(
                &terminal("claude-ai-login", &["--cli", "auth", "login", "--claudeai"]),
                &launch,
                &env,
            )
            .expect("a command");
        assert_eq!(
            login.argv,
            ["/opt/claude/bin/claude", "auth", "login", "--claudeai"]
        );
        let remote = ClaudeAcp
            .auth_command(&terminal("claude-login", &["--cli"]), &launch, &env)
            .expect("a command");
        assert_eq!(remote.argv, ["/opt/claude/bin/claude"]);
        assert_eq!(
            ClaudeAcp.auth_command(&terminal("other", &["--tui"]), &launch, &env),
            None
        );
        // Without the launch variable: the env's, then plain `claude`.
        let bare = launch_without_env();
        let mut env = EnvMap::new();
        env.insert("CLAUDE_CODE_EXECUTABLE".into(), "/env/claude".into());
        assert_eq!(
            ClaudeAcp
                .auth_command(&terminal("c", &["--cli", "x"]), &bare, &env)
                .expect("command")
                .argv,
            ["/env/claude", "x"]
        );
        assert_eq!(
            ClaudeAcp
                .auth_command(&terminal("c", &["--cli"]), &bare, &EnvMap::new())
                .expect("command")
                .argv,
            ["claude"]
        );
    }

    fn launch_without_env() -> LaunchSpec {
        launch("/opt/bin/node", &["entry.js"], &[])
    }

    #[test]
    fn codex_falls_back_to_codex_login() {
        let launch = launch(
            "/opt/bin/node",
            &["/adapters/codex/dist/index.js"],
            &[("CODEX_PATH", "/opt/codex/bin/codex")],
        );
        let commands = CodexAcp.fallback_auth_commands(&launch, &EnvMap::new());
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].argv, ["/opt/codex/bin/codex", "login"]);
        assert!(
            Generic
                .fallback_auth_commands(&launch, &EnvMap::new())
                .is_empty()
        );
        assert!(
            Hermes
                .fallback_auth_commands(&launch, &EnvMap::new())
                .is_empty()
        );
    }

    #[test]
    fn generic_terminal_methods_run_the_launch_command() {
        let launch = launch("/opt/bin/agent", &["--acp"], &[]);
        let command = Generic
            .auth_command(&terminal("login", &["login"]), &launch, &EnvMap::new())
            .expect("a command");
        assert_eq!(command.argv, ["/opt/bin/agent", "--acp", "login"]);
    }

    #[test]
    fn only_hermes_stale_generation_refusals_are_retried() {
        let error = |code: i64, data: Option<serde_json::Value>| RpcError {
            code,
            message: "Internal error".into(),
            data,
        };
        let stale = error(
            codes::INTERNAL_ERROR,
            Some(serde_json::json!({"details": "stale_generation"})),
        );
        assert!(Hermes.mode_change_retryable(&stale));
        assert!(!Hermes.mode_change_retryable(&error(codes::INTERNAL_ERROR, None)));
        assert!(!Hermes.mode_change_retryable(&error(
            codes::INVALID_PARAMS,
            Some(serde_json::json!({"details": "stale_generation"}))
        )));
        assert!(!Generic.mode_change_retryable(&stale));
    }

    #[test]
    fn hermes_breakaway_hint() {
        assert_eq!(
            Hermes.start_failure_hint(
                "ERROR gateway start failed: windows_breakaway_unavailable (job object)"
            ),
            Some(HERMES_BREAKAWAY_HINT.to_owned())
        );
        assert_eq!(Hermes.start_failure_hint("some other failure"), None);
        for dialect in [
            dialect_for("claude-acp"),
            dialect_for("codex-acp"),
            dialect_for("generic"),
        ] {
            assert_eq!(
                dialect.start_failure_hint("windows_breakaway_unavailable"),
                None
            );
        }
    }

    #[test]
    fn default_hooks_leave_requests_and_notifications_alone() {
        for dialect in ["hermes", "claude-acp", "codex-acp", "generic"].map(dialect_for) {
            assert_eq!(
                dialect.on_request("x/y", &serde_json::Value::Null),
                DialectReply::NotMine
            );
            assert!(
                dialect
                    .on_notification("x/y", &serde_json::Value::Null)
                    .is_empty()
            );
        }
    }
}
