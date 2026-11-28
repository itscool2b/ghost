//! A terminal chat's launch: the agent's own interactive interface, started with what the chat's
//! driver would get (program, environment, account, shared MCP, project memory, the session to
//! continue) but the CLI's interactive arguments instead of its protocol. Credential values travel
//! only in the environment, never on the command line, where other programs and logs can read them.
use std::path::{Path, PathBuf};

use ghost_process::{EnvKey, EnvMap, LaunchSpec};
use ghost_protocol::{
    Autonomy,
    mcp::{McpTransport, ResolvedMcpServer},
};
use serde_json::{Value, json};

/// The vendor session a terminal opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalSession {
    /// Continue this session (Claude Code's session ID, Codex's thread ID, ...).
    Resume(String),
    /// A new session under this ID, chosen up front so Chat can resume it later. Only for agents
    /// that take one ([`TerminalAgent::names_new_sessions`]).
    Named(String),
    /// A new session the agent names itself, learned after the terminal ends where the agent
    /// records it.
    New,
}

pub struct TerminalOptions<'a> {
    pub servers: &'a [ResolvedMcpServer],
    pub autonomy: Autonomy,
    pub session: TerminalSession,
    /// The model the chat chose, if it chose one.
    pub model: Option<String>,
    pub add_dirs: &'a [PathBuf],
    /// Pi's shared-MCP extension file ([`pi_extension`]), which the caller keeps while the
    /// terminal runs.
    pub extension: Option<&'a Path>,
    /// The chat's resolved environment, for a value the person set themselves
    /// (`OPENCODE_CONFIG_CONTENT`).
    pub env: Option<&'a EnvMap>,
}

/// The launch, and the shared servers it had to leave out, with why, for the chat to say.
#[derive(Debug, Clone)]
pub struct TerminalLaunch {
    pub launch: LaunchSpec,
    pub omitted: Vec<String>,
}

/// Claude Code's interactive interface. Its MCP descriptor names environment variables
/// (`${GHOST_MCP_0_env_0}`) whose values the launch sets, as the protocol launch does.
pub fn claude(base: &LaunchSpec, options: &TerminalOptions<'_>) -> TerminalLaunch {
    let mut launch = interactive(base);
    crate::mcp::claude(options.servers, &mut launch);
    let mode = match options.autonomy {
        Autonomy::Ask => "default",
        Autonomy::AutoEdit => "acceptEdits",
        Autonomy::FullAuto => "bypassPermissions",
    };
    launch
        .args
        .extend(["--permission-mode".into(), mode.into()]);
    if options.autonomy == Autonomy::FullAuto {
        launch
            .args
            .push("--allow-dangerously-skip-permissions".into());
    }
    match &options.session {
        TerminalSession::Resume(id) => launch.args.push(format!("--resume={id}").into()),
        TerminalSession::Named(id) => launch.args.push(format!("--session-id={id}").into()),
        TerminalSession::New => {}
    }
    if let Some(model) = &options.model {
        launch.args.push(format!("--model={model}").into());
    }
    for dir in options.add_dirs {
        launch
            .args
            .extend(["--add-dir".into(), dir.as_os_str().to_owned()]);
    }
    TerminalLaunch {
        launch,
        omitted: Vec::new(),
    }
}

/// Text safe to type into a terminal: every control character but line breaks and tabs is
/// dropped (escape included, so text cannot end a bracketed paste and run as keystrokes, nor move
/// the cursor or rewrite the screen), and CRLF becomes LF.
pub fn typed_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

/// A key Codex's `-c` path can carry without quoting: its parser splits the path at dots.
fn plain_key(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A TOML value for `-c`: JSON strings, arrays and inline tables of strings are valid TOML.
fn toml(value: &Value) -> String {
    match value {
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}={}", Value::String(k.clone()), toml(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Array(items) => {
            format!("[{}]", items.iter().map(toml).collect::<Vec<_>>().join(","))
        }
        other => other.to_string(),
    }
}

/// Codex's interactive interface. Shared servers go in `-c mcp_servers.<name>.*` overrides; a
/// command's environment credentials are set on Codex's process and forwarded by name
/// (`env_vars`), and an address's header credentials through `env_http_headers`.
pub fn codex(base: &LaunchSpec, options: &TerminalOptions<'_>) -> TerminalLaunch {
    let mut launch = interactive(base);
    let mut omitted = Vec::new();
    if let TerminalSession::Resume(id) = &options.session {
        launch.args.extend(["resume".into(), id.into()]);
    }
    let (approval, sandbox) = match options.autonomy {
        Autonomy::FullAuto => ("never", "danger-full-access"),
        Autonomy::AutoEdit => ("on-request", "workspace-write"),
        Autonomy::Ask => ("untrusted", "read-only"),
    };
    launch.args.extend([
        "--ask-for-approval".into(),
        approval.into(),
        "--sandbox".into(),
        sandbox.into(),
    ]);
    if let Some(model) = &options.model {
        launch.args.extend(["--model".into(), model.into()]);
    }
    for dir in options.add_dirs {
        launch
            .args
            .extend(["--add-dir".into(), dir.as_os_str().to_owned()]);
    }
    // Names compare as the OS compares them (TOKEN and token are one variable on Windows). A
    // variable forwarded to one server must not carry another's value, nor replace one the launch
    // already sets (the account's, say); Ghost's own names, GHOST_ and the earlier O7_ and AURA_,
    // are reserved.
    let launch_names: std::collections::BTreeSet<EnvKey> = launch
        .set_env
        .iter()
        .map(|(name, _)| EnvKey::new(name.clone()))
        .collect();
    let mut forwarded = std::collections::BTreeMap::<EnvKey, String>::new();
    let mut headers = 0usize;
    for server in options.servers {
        let name = &server.definition.name;
        if !plain_key(name) {
            omitted.push(format!(
                "{name}: Codex's terminal cannot take a server name with dots"
            ));
            continue;
        }
        let mut overrides: Vec<(String, Value)> = Vec::new();
        let mut env = Vec::new();
        match &server.definition.transport {
            McpTransport::Stdio { command, args } => {
                // Ghost's own project-memory server uses the reserved names by design.
                let own_memory = crate::mcp::managed(server);
                let reserved = !own_memory
                    && server.credentials.env.keys().any(|key| {
                        // The app's own launch variables, under its name now and before.
                        ["GHOST_", "O7_", "AURA_"].iter().any(|own| {
                            key.get(..own.len())
                                .is_some_and(|p| p.eq_ignore_ascii_case(own))
                        }) || launch_names.contains(&EnvKey::new(key))
                    });
                if reserved {
                    omitted.push(format!(
                        "{name}: it needs a variable Ghost sets for the launch itself"
                    ));
                    continue;
                }
                let mut own = std::collections::BTreeMap::<EnvKey, &String>::new();
                let clash = server.credentials.env.iter().any(|(key, value)| {
                    let name = EnvKey::new(key);
                    own.insert(name.clone(), value)
                        .is_some_and(|other| other != value)
                        || forwarded
                            .get(&name)
                            .is_some_and(|existing| existing != value)
                });
                if clash {
                    omitted.push(format!(
                        "{name}: another shared server needs a different value in the same variable"
                    ));
                    continue;
                }
                overrides.push(("command".into(), json!(command)));
                overrides.push(("args".into(), json!(args)));
                if !server.credentials.env.is_empty() {
                    overrides.push((
                        "env_vars".into(),
                        json!(server.credentials.env.keys().collect::<Vec<_>>()),
                    ));
                    for (key, value) in &server.credentials.env {
                        env.push((key.clone(), value.clone()));
                    }
                }
            }
            McpTransport::Http { url } => {
                overrides.push(("url".into(), json!(url)));
                if !server.credentials.headers.is_empty() {
                    let mut names = serde_json::Map::new();
                    for (header, value) in &server.credentials.headers {
                        let mut variable = format!("GHOST_CODEX_MCP_HEADER_{headers}");
                        while launch_names.contains(&EnvKey::new(&variable)) {
                            headers += 1;
                            variable = format!("GHOST_CODEX_MCP_HEADER_{headers}");
                        }
                        headers += 1;
                        names.insert(header.clone(), json!(variable));
                        env.push((variable, value.clone()));
                    }
                    overrides.push(("env_http_headers".into(), Value::Object(names)));
                }
            }
            McpTransport::Sse { .. } => {
                omitted.push(format!("{name}: Codex does not take legacy SSE servers"));
                continue;
            }
        }
        overrides.push(("enabled".into(), json!(true)));
        for (key, value) in overrides {
            launch.args.extend([
                "-c".into(),
                format!("mcp_servers.{name}.{key}={}", toml(&value)).into(),
            ]);
        }
        for (key, value) in env {
            let name = EnvKey::new(&key);
            forwarded.insert(name.clone(), value.clone());
            launch
                .set_env
                .retain(|(existing, _)| EnvKey::new(existing.clone()) != name);
            launch.set_env.push((key.into(), value.into()));
        }
    }
    TerminalLaunch { launch, omitted }
}

/// The agents whose own interactive interface a chat can open in Ghost's terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalAgent {
    Claude,
    Codex,
    Grok,
    OpenCode,
    Hermes,
    Pi,
    OhMyPi,
    Cursor,
    Antigravity,
}

impl TerminalAgent {
    /// The agent a catalog agent ID names (`ghost_agents::agent_of`), when it has an interactive
    /// interface Ghost can open.
    pub fn of(agent: &str) -> Option<TerminalAgent> {
        Some(match agent {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "grok" => Self::Grok,
            "opencode" => Self::OpenCode,
            "hermes" => Self::Hermes,
            "pi" => Self::Pi,
            "omp" => Self::OhMyPi,
            "cursor" => Self::Cursor,
            "antigravity" => Self::Antigravity,
            _ => return None,
        })
    }

    /// Chat (its protocol) and its interface keep one store of sessions, so either continues the
    /// other's. Two do not (found natively on 8 November 2026): Cursor 2026.11.05 keeps its ACP
    /// sessions apart from its interface's chats (`~/.cursor/acp-sessions`, `~/.cursor/chats`), and
    /// Hermes 0.22.5's ACP server refuses to resume a session its CLI began. Their terminal keeps
    /// a session of its own, and the visible handoff carries its turns to Chat.
    pub fn shares_chat_sessions(self) -> bool {
        !matches!(self, Self::Cursor | Self::Hermes)
    }

    /// A new session can start under an ID Ghost chooses, so Chat continues it afterwards.
    pub fn names_new_sessions(self) -> bool {
        matches!(self, Self::Claude | Self::Grok | Self::Pi)
    }

    /// What the terminal types to ask the agent's interface to exit, so it finishes its session's
    /// record. Its processes close after the grace period either way.
    pub fn exit_input(self) -> &'static str {
        match self {
            // Hermes' prompt ends on end of input; Ctrl+C first clears what was typed.
            Self::Hermes => "\u{3}\u{4}",
            _ => "\u{3}\u{3}",
        }
    }

    /// The interface's program name, for finding it on `PATH`.
    pub fn program_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::Hermes => "hermes",
            Self::Pi => "pi",
            Self::OhMyPi => "omp",
            Self::Cursor => "cursor-agent",
            Self::Antigravity => "agy",
        }
    }

    /// The agent's name in messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Grok => "Grok Build",
            Self::OpenCode => "OpenCode",
            Self::Hermes => "Hermes",
            Self::Pi => "Pi",
            Self::OhMyPi => "oh-my-pi",
            Self::Cursor => "Cursor",
            Self::Antigravity => "Antigravity",
        }
    }
}

/// The program the agent's interface runs as. Usually the connection's own, once its protocol
/// arguments are dropped (`grok agent stdio` becomes `grok`). Through an ACP adapter, the CLI the
/// adapter drives, whose path the adapter reads from `adapter_env` (`CLAUDE_CODE_EXECUTABLE`).
/// Antigravity's ACP server is a program of its own, so its interface is `agy` beside it or on
/// `PATH`.
pub fn interactive_program(
    agent: TerminalAgent,
    base: &LaunchSpec,
    adapter_env: Option<&str>,
    env: &EnvMap,
) -> Option<PathBuf> {
    if let Some(name) = adapter_env {
        let key = EnvKey::from(name);
        return base
            .set_env
            .iter()
            .rev()
            .find(|(existing, _)| EnvKey::new(existing.clone()) == key)
            .map(|(_, value)| PathBuf::from(value))
            .or_else(|| env.get(&key).map(PathBuf::from))
            .filter(|path| path.is_absolute())
            .or_else(|| ghost_process::find_program(agent.program_name(), env));
    }
    if agent == TerminalAgent::Antigravity {
        let named = |path: &Path| {
            path.file_stem()
                .is_some_and(|stem| stem.eq_ignore_ascii_case(agent.program_name()))
        };
        if named(&base.program) {
            return Some(base.program.clone());
        }
        let beside = base
            .program
            .parent()
            .map(|dir| dir.join(if cfg!(windows) { "agy.exe" } else { "agy" }))
            .filter(|path| path.is_file());
        return beside.or_else(|| ghost_process::find_program(agent.program_name(), env));
    }
    Some(base.program.clone())
}

/// The connection's arguments the interface keeps: what comes before its protocol arguments (an
/// npm shim's entry script: `node.exe <entry> acp` keeps `<entry>`). None when the interface is
/// another program than the connection's (`replaced`), or when the arguments do not end with the
/// protocol's as expected.
pub fn interactive_args(
    args: &[std::ffi::OsString],
    protocol: &[&str],
    replaced: bool,
) -> Vec<std::ffi::OsString> {
    if replaced || args.len() < protocol.len() {
        return Vec::new();
    }
    let (prefix, tail) = args.split_at(args.len() - protocol.len());
    if tail
        .iter()
        .zip(protocol)
        .all(|(arg, expected)| arg == expected)
    {
        prefix.to_vec()
    } else {
        Vec::new()
    }
}

/// The launch for `agent`'s interface: `base` is the connection's launch with `program` already
/// the interface's ([`interactive_program`]) and `args` only what it keeps
/// ([`interactive_args`]).
pub fn build(
    agent: TerminalAgent,
    base: &LaunchSpec,
    options: &TerminalOptions<'_>,
) -> TerminalLaunch {
    let mut made = match agent {
        TerminalAgent::Claude => claude(base, options),
        TerminalAgent::Codex => codex(base, options),
        TerminalAgent::Grok => grok(base, options),
        TerminalAgent::OpenCode => opencode(base, options),
        TerminalAgent::Hermes => hermes(base, options),
        TerminalAgent::Pi => pi(base, options),
        TerminalAgent::OhMyPi => oh_my_pi(base, options),
        TerminalAgent::Cursor => cursor(base, options),
        TerminalAgent::Antigravity => antigravity(base, options),
    };
    // An adapter starts in its own folder; the agent's interface works in the chat's.
    made.launch.cwd = ghost_process::ProcessCwd::ChatFolder;
    made
}

/// The connection's launch as an interactive one (its protocol arguments already dropped).
fn interactive(base: &LaunchSpec) -> LaunchSpec {
    let mut launch = base.clone();
    launch.label = format!("{} (terminal)", base.label);
    launch
}

/// A shared server as the chat names it: Ghost's own memory server by what it is.
fn server_label(server: &ResolvedMcpServer) -> String {
    if crate::mcp::managed_memory(server) {
        "Project memory tools".to_owned()
    } else if crate::mcp::managed_browser(server) {
        "Browser tools".to_owned()
    } else if crate::mcp::managed_mobile(server) {
        "Device tools".to_owned()
    } else if crate::mcp::managed_game(server) {
        "Game tools".to_owned()
    } else if crate::mcp::managed_project(server) {
        "Project tools".to_owned()
    } else {
        server.definition.name.clone()
    }
}

/// Every shared server, left out because the agent's interface takes MCP servers only from its
/// own settings.
fn own_settings(
    agent: TerminalAgent,
    servers: &[ResolvedMcpServer],
    settings: &str,
) -> Vec<String> {
    servers
        .iter()
        .map(|server| {
            format!(
                "{}: {}'s terminal loads MCP servers only from {settings}",
                server_label(server),
                agent.label()
            )
        })
        .collect()
}

/// The chat's model and extra folders, when the agent's interface cannot take them.
fn unsupported(
    agent: TerminalAgent,
    options: &TerminalOptions<'_>,
    model: bool,
    folders: bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    if !model && let Some(chosen) = &options.model {
        notes.push(format!(
            "the chosen model ({chosen}): {}'s terminal uses its own model setting",
            agent.label()
        ));
    }
    if !folders && !options.add_dirs.is_empty() {
        notes.push(format!(
            "the chat's other folders: {}'s terminal works in the chat's folder only",
            agent.label()
        ));
    }
    notes
}

fn add_dirs(launch: &mut LaunchSpec, options: &TerminalOptions<'_>) {
    for dir in options.add_dirs {
        launch
            .args
            .extend(["--add-dir".into(), dir.as_os_str().to_owned()]);
    }
}

/// Grok Build's interface. A new session takes the UUID Ghost chose (`--session-id`).
pub fn grok(base: &LaunchSpec, options: &TerminalOptions<'_>) -> TerminalLaunch {
    let agent = TerminalAgent::Grok;
    let mut launch = interactive(base);
    match &options.session {
        TerminalSession::Resume(id) => launch.args.extend(["--resume".into(), id.into()]),
        TerminalSession::Named(id) => launch.args.extend(["--session-id".into(), id.into()]),
        TerminalSession::New => {}
    }
    let mode = match options.autonomy {
        Autonomy::Ask => "default",
        Autonomy::AutoEdit => "acceptEdits",
        Autonomy::FullAuto => "bypassPermissions",
    };
    launch
        .args
        .extend(["--permission-mode".into(), mode.into()]);
    if let Some(model) = &options.model {
        launch.args.extend(["--model".into(), model.into()]);
    }
    add_dirs(&mut launch, options);
    TerminalLaunch {
        launch,
        omitted: own_settings(agent, options.servers, "its own settings (grok mcp)"),
    }
}

/// OpenCode's interface. Shared servers and the chat's access level travel in
/// `OPENCODE_CONFIG_CONTENT`, an environment variable OpenCode merges over its own configuration
/// (and over the person's own value of it), so no credential reaches the command line.
pub fn opencode(base: &LaunchSpec, options: &TerminalOptions<'_>) -> TerminalLaunch {
    const NAME: &str = "OPENCODE_CONFIG_CONTENT";
    let agent = TerminalAgent::OpenCode;
    let mut launch = interactive(base);
    let mut omitted = Vec::new();
    if let TerminalSession::Resume(id) = &options.session {
        launch.args.extend(["--session".into(), id.into()]);
    }
    match &options.model {
        // OpenCode names a model by its provider and ID.
        Some(model) if model.contains('/') => {
            launch.args.extend(["--model".into(), model.into()]);
        }
        Some(model) => omitted.push(format!(
            "the chosen model ({model}): OpenCode's terminal names models as provider/model"
        )),
        None => {}
    }
    let mut config = serde_json::Map::new();
    match options.autonomy {
        Autonomy::Ask => {
            config.insert(
                "permission".into(),
                json!({"edit": "ask", "bash": "ask", "webfetch": "ask"}),
            );
        }
        Autonomy::AutoEdit => {
            config.insert(
                "permission".into(),
                json!({"edit": "allow", "bash": "ask", "webfetch": "ask"}),
            );
        }
        // Everything not explicitly denied is approved.
        Autonomy::FullAuto => launch.args.push("--auto".into()),
    }
    let mut servers = serde_json::Map::new();
    for server in options.servers {
        let entry = match &server.definition.transport {
            McpTransport::Stdio { command, args } => {
                let mut line = vec![command.clone()];
                line.extend(args.iter().cloned());
                let mut entry = json!({"type": "local", "command": line, "enabled": true});
                if !server.credentials.env.is_empty() {
                    entry["environment"] = json!(server.credentials.env);
                }
                entry
            }
            McpTransport::Http { url } => {
                let mut entry = json!({"type": "remote", "url": url, "enabled": true});
                if !server.credentials.headers.is_empty() {
                    entry["headers"] = json!(server.credentials.headers);
                }
                entry
            }
            McpTransport::Sse { .. } => {
                omitted.push(format!(
                    "{}: OpenCode's terminal does not take legacy SSE servers",
                    server_label(server)
                ));
                continue;
            }
        };
        servers.insert(server.definition.name.clone(), entry);
    }
    if !servers.is_empty() {
        config.insert("mcp".into(), Value::Object(servers));
    }
    if !config.is_empty() {
        let key = EnvKey::from(NAME);
        let own = launch
            .set_env
            .iter()
            .rev()
            .find(|(existing, _)| EnvKey::new(existing.clone()) == key)
            .map(|(_, value)| value.to_string_lossy().into_owned())
            .or_else(|| {
                options
                    .env
                    .and_then(|env| env.get(&key))
                    .map(|value| value.to_string_lossy().into_owned())
            });
        let mut merged = own
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        merge(&mut merged, Value::Object(config));
        launch
            .set_env
            .retain(|(existing, _)| EnvKey::new(existing.clone()) != key);
        launch
            .set_env
            .push((NAME.into(), merged.to_string().into()));
    }
    omitted.extend(unsupported(agent, options, true, false));
    TerminalLaunch { launch, omitted }
}

/// `overlay` over `base`, objects merged key by key.
fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// Hermes' interface. A resumed session stays in the chat's folder (`--no-restore-cwd`), whatever
/// folder it was recorded in.
pub fn hermes(base: &LaunchSpec, options: &TerminalOptions<'_>) -> TerminalLaunch {
    let agent = TerminalAgent::Hermes;
    let mut launch = interactive(base);
    if let TerminalSession::Resume(id) = &options.session {
        launch
            .args
            .extend(["--resume".into(), id.into(), "--no-restore-cwd".into()]);
    }
    if options.autonomy == Autonomy::FullAuto {
        launch.args.push("--yolo".into());
    }
    let mut omitted = own_settings(agent, options.servers, "its own config");
    omitted.extend(unsupported(agent, options, false, false));
    TerminalLaunch { launch, omitted }
}

/// Pi's interface. Shared servers load through the extension Pi's Chat driver uses
/// ([`pi_extension`]), their configuration in `GHOST_SHARED_MCP`.
pub fn pi(base: &LaunchSpec, options: &TerminalOptions<'_>) -> TerminalLaunch {
    let agent = TerminalAgent::Pi;
    let mut launch = interactive(base);
    let mut omitted = Vec::new();
    match &options.session {
        TerminalSession::Resume(id) => launch.args.extend(["--session".into(), id.into()]),
        TerminalSession::Named(id) => launch.args.extend(["--session-id".into(), id.into()]),
        TerminalSession::New => {}
    }
    if let Some(model) = &options.model {
        launch.args.extend(["--model".into(), model.into()]);
    }
    let (servers, left): (Vec<_>, Vec<_>) = options
        .servers
        .iter()
        .cloned()
        .partition(crate::pi::shared_mcp_supported);
    omitted.extend(left.iter().map(|server| {
        format!(
            "{}: Pi takes stdio and HTTP servers named with letters, numbers, underscores and hyphens",
            server_label(server)
        )
    }));
    if !servers.is_empty() {
        match options.extension {
            Some(extension) => {
                launch
                    .args
                    .extend(["--extension".into(), extension.as_os_str().to_owned()]);
                launch.set_env.push((
                    "GHOST_SHARED_MCP".into(),
                    crate::pi::shared_mcp_json(&servers).into(),
                ));
            }
            None => omitted.extend(servers.iter().map(|server| {
                format!(
                    "{}: Ghost could not prepare Pi's MCP extension",
                    server_label(server)
                )
            })),
        }
    }
    omitted.extend(unsupported(agent, options, true, false));
    TerminalLaunch { launch, omitted }
}

/// Pi's shared-MCP extension as a file for its interface to load: the caller keeps the handle
/// until the terminal ends (the file goes with it).
pub fn pi_extension() -> std::io::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut file = tempfile::Builder::new()
        .prefix("ghost-shared-mcp-")
        .suffix(".ts")
        .tempfile()?;
    file.write_all(crate::pi::SHARED_MCP_EXTENSION)?;
    file.flush()?;
    Ok(file)
}

/// oh-my-pi's interface.
pub fn oh_my_pi(base: &LaunchSpec, options: &TerminalOptions<'_>) -> TerminalLaunch {
    let agent = TerminalAgent::OhMyPi;
    let mut launch = interactive(base);
    if let TerminalSession::Resume(id) = &options.session {
        launch.args.extend(["--resume".into(), id.into()]);
    }
    let mut omitted = own_settings(agent, options.servers, "its own config");
    omitted.extend(unsupported(agent, options, false, false));
    TerminalLaunch { launch, omitted }
}

/// Cursor's interface.
pub fn cursor(base: &LaunchSpec, options: &TerminalOptions<'_>) -> TerminalLaunch {
    let agent = TerminalAgent::Cursor;
    let mut launch = interactive(base);
    if let TerminalSession::Resume(id) = &options.session {
        launch.args.push(format!("--resume={id}").into());
    }
    if options.autonomy == Autonomy::FullAuto {
        launch.args.push("--force".into());
    }
    let mut omitted = own_settings(agent, options.servers, "its own mcp.json");
    omitted.extend(unsupported(agent, options, false, false));
    TerminalLaunch { launch, omitted }
}

/// Antigravity's interface (`agy`).
pub fn antigravity(base: &LaunchSpec, options: &TerminalOptions<'_>) -> TerminalLaunch {
    let agent = TerminalAgent::Antigravity;
    let mut launch = interactive(base);
    if let TerminalSession::Resume(id) = &options.session {
        launch.args.extend(["--conversation".into(), id.into()]);
    }
    match options.autonomy {
        Autonomy::Ask => {}
        Autonomy::AutoEdit => launch.args.extend(["--mode".into(), "accept-edits".into()]),
        Autonomy::FullAuto => launch.args.push("--dangerously-skip-permissions".into()),
    }
    add_dirs(&mut launch, options);
    let mut omitted = own_settings(agent, options.servers, "its own settings (agy mcp)");
    omitted.extend(unsupported(agent, options, false, true));
    TerminalLaunch { launch, omitted }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_protocol::mcp::{McpCredentials, McpDefinition};
    use std::collections::BTreeMap;

    /// A connection's launch as the terminal gets it: its protocol arguments already dropped
    /// (`interactive_args`).
    fn base() -> LaunchSpec {
        LaunchSpec {
            label: "claude".into(),
            program: PathBuf::from("/bin/claude"),
            args: Vec::new(),
            cwd: ghost_process::ProcessCwd::ChatFolder,
            set_env: vec![("CLAUDE_CONFIG_DIR".into(), "/accounts/two".into())],
            daemons: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(30),
        }
    }

    fn server(name: &str, transport: McpTransport, env: &[(&str, String)]) -> ResolvedMcpServer {
        let values: BTreeMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        let http = matches!(transport, McpTransport::Http { .. });
        ResolvedMcpServer {
            definition: McpDefinition {
                id: "61a6ce00-b99f-4f8a-9148-8f880f3c735f".into(),
                name: name.into(),
                project_id: None,
                enabled: true,
                transport,
                env_keys: if http {
                    vec![]
                } else {
                    values.keys().cloned().collect()
                },
                header_keys: if http {
                    values.keys().cloned().collect()
                } else {
                    vec![]
                },
                agents: None,
            },
            revision: 1,
            credentials: McpCredentials {
                env: if http {
                    BTreeMap::new()
                } else {
                    values.clone()
                },
                headers: if http { values } else { BTreeMap::new() },
            },
        }
    }

    fn args(launch: &LaunchSpec) -> Vec<String> {
        launch
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn options(servers: &[ResolvedMcpServer], session: TerminalSession) -> TerminalOptions<'_> {
        TerminalOptions {
            servers,
            autonomy: Autonomy::AutoEdit,
            session,
            model: Some("opus".into()),
            add_dirs: &[],
            extension: None,
            env: None,
        }
    }

    #[test]
    fn claude_drops_the_protocol_resumes_the_chats_session_and_keeps_the_account() {
        let made = claude(&base(), &options(&[], TerminalSession::Resume("s1".into())));
        let argv = args(&made.launch);
        assert!(
            !argv.iter().any(|a| a == "-p" || a.contains("stream-json")),
            "{argv:?}"
        );
        assert!(argv.contains(&"--resume=s1".to_owned()));
        assert!(argv.contains(&"--model=opus".to_owned()));
        assert_eq!(
            argv[..2],
            ["--permission-mode".to_owned(), "acceptEdits".to_owned()]
        );
        assert!(
            made.launch
                .set_env
                .iter()
                .any(|(k, _)| k == "CLAUDE_CONFIG_DIR")
        );
        let new = claude(&base(), &options(&[], TerminalSession::Named("n1".into())));
        assert!(args(&new.launch).contains(&"--session-id=n1".to_owned()));
    }

    #[test]
    fn typed_text_cannot_end_a_paste_or_send_keys() {
        let hostile = "answer\u{1b}[201~\r\nrm -rf ~\r\u{9b}2J\u{7}done\tok";
        let safe = typed_text(hostile);
        assert!(
            !safe
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t'),
            "{safe:?}"
        );
        assert_eq!(safe, "answer[201~\nrm -rf ~2Jdone\tok");
    }

    #[test]
    fn credentials_never_reach_either_command_line() {
        let secret = format!("value-{}", 41);
        let servers = [
            server(
                "docs",
                McpTransport::Stdio {
                    command: "node".into(),
                    args: vec!["s.js".into()],
                },
                &[("TOKEN", secret.clone())],
            ),
            server(
                "remote",
                McpTransport::Http {
                    url: "https://example.test/mcp".into(),
                },
                &[("X-Key", secret.clone())],
            ),
        ];
        for made in [
            claude(&base(), &options(&servers, TerminalSession::New)),
            codex(&base(), &options(&servers, TerminalSession::New)),
        ] {
            assert!(!args(&made.launch).iter().any(|a| a.contains(&secret)));
            assert!(
                made.launch
                    .set_env
                    .iter()
                    .any(|(_, v)| v.to_string_lossy() == secret)
            );
        }
    }

    #[test]
    fn codex_forwards_a_commands_variables_by_name_and_headers_through_the_environment() {
        let secret = format!("value-{}", 7);
        let servers = [
            server(
                "docs",
                McpTransport::Stdio {
                    command: "node".into(),
                    args: vec!["s.js".into()],
                },
                &[("TOKEN", secret.clone())],
            ),
            server(
                "remote",
                McpTransport::Http {
                    url: "https://example.test/mcp".into(),
                },
                &[("X-Key", secret.clone())],
            ),
        ];
        let made = codex(
            &base(),
            &options(&servers, TerminalSession::Resume("t1".into())),
        );
        let args = args(&made.launch);
        assert_eq!(args[..2], ["resume".to_owned(), "t1".to_owned()]);
        assert!(
            args.contains(&r#"mcp_servers.docs.command="node""#.to_owned()),
            "{args:?}"
        );
        assert!(args.contains(&r#"mcp_servers.docs.args=["s.js"]"#.to_owned()));
        assert!(args.contains(&r#"mcp_servers.docs.env_vars=["TOKEN"]"#.to_owned()));
        assert!(
            args.contains(
                &r#"mcp_servers.remote.env_http_headers={"X-Key"="GHOST_CODEX_MCP_HEADER_0"}"#
                    .to_owned()
            )
        );
        assert!(
            made.launch
                .set_env
                .iter()
                .any(|(k, v)| k == "TOKEN" && v.to_string_lossy() == secret)
        );
        assert!(made.omitted.is_empty());
    }

    #[test]
    fn codex_never_lets_a_server_variable_replace_the_launchs_or_auracodes_own() {
        let mut launch = base();
        launch
            .set_env
            .push(("GHOST_CODEX_MCP_HEADER_0".into(), "taken".into()));
        let servers = [
            server(
                "account",
                McpTransport::Stdio {
                    command: "a".into(),
                    args: vec![],
                },
                &[("CLAUDE_CONFIG_DIR", format!("v{}", 1))],
            ),
            server(
                "reserved",
                McpTransport::Stdio {
                    command: "b".into(),
                    args: vec![],
                },
                &[("aura_codex_mcp_header_9", format!("v{}", 2))],
            ),
            server(
                "remote",
                McpTransport::Http {
                    url: "https://example.test/mcp".into(),
                },
                &[("X-Key", format!("v{}", 3))],
            ),
        ];
        let made = codex(&launch, &options(&servers, TerminalSession::New));
        assert_eq!(made.omitted.len(), 2, "{:?}", made.omitted);
        // The header takes the next free name, leaving the launch's own value alone.
        assert!(
            args(&made.launch)
                .iter()
                .any(|a| a.contains("GHOST_CODEX_MCP_HEADER_1"))
        );
        let taken = made
            .launch
            .set_env
            .iter()
            .find(|(k, _)| k == "GHOST_CODEX_MCP_HEADER_0");
        assert_eq!(
            taken.map(|(_, v)| v.to_string_lossy().into_owned()),
            Some("taken".into())
        );
    }

    #[test]
    fn codex_keeps_project_memory_whose_variables_are_auracodes_own() {
        let mut memory = server(
            "ghost_project_memory",
            McpTransport::Stdio {
                command: "auracode".into(),
                args: vec!["--ghost-memory-helper".into()],
            },
            &[("GHOST_MEMORY_TOKEN", format!("t{}", 1))],
        );
        memory.definition.id = "ghost-project-memory".into();
        let made = codex(
            &base(),
            &options(std::slice::from_ref(&memory), TerminalSession::New),
        );
        assert!(made.omitted.is_empty(), "{:?}", made.omitted);
        assert!(
            args(&made.launch)
                .iter()
                .any(|a| a.starts_with("mcp_servers.ghost_project_memory."))
        );
        // Another server may still not take Ghost's names.
        let mut other = memory.clone();
        other.definition.id = "61a6ce00-b99f-4f8a-9148-8f880f3c735f".into();
        other.definition.name = "imposter".into();
        assert_eq!(
            codex(&base(), &options(&[other], TerminalSession::New))
                .omitted
                .len(),
            1
        );
    }

    #[test]
    fn codex_compares_variable_names_as_the_os_does() {
        let servers = [
            server(
                "one",
                McpTransport::Stdio {
                    command: "a".into(),
                    args: vec![],
                },
                &[("TOKEN", format!("v{}", 1))],
            ),
            server(
                "two",
                McpTransport::Stdio {
                    command: "b".into(),
                    args: vec![],
                },
                &[("token", format!("v{}", 2))],
            ),
        ];
        let made = codex(&base(), &options(&servers, TerminalSession::New));
        // One variable on Windows, two elsewhere.
        assert_eq!(
            made.omitted.len(),
            usize::from(cfg!(windows)),
            "{:?}",
            made.omitted
        );
    }

    #[test]
    fn codex_leaves_out_what_it_cannot_take_and_says_why() {
        let servers = [
            server(
                "legacy",
                McpTransport::Sse {
                    url: "https://example.test/sse".into(),
                },
                &[],
            ),
            server(
                "dotted.name",
                McpTransport::Http {
                    url: "https://example.test/mcp".into(),
                },
                &[],
            ),
            server(
                "one",
                McpTransport::Stdio {
                    command: "a".into(),
                    args: vec![],
                },
                &[("TOKEN", format!("v{}", 1))],
            ),
            server(
                "two",
                McpTransport::Stdio {
                    command: "b".into(),
                    args: vec![],
                },
                &[("TOKEN", format!("v{}", 2))],
            ),
        ];
        let made = codex(&base(), &options(&servers, TerminalSession::New));
        assert_eq!(made.omitted.len(), 3, "{:?}", made.omitted);
        assert!(
            args(&made.launch)
                .iter()
                .any(|a| a.starts_with("mcp_servers.one."))
        );
        assert!(
            !args(&made.launch)
                .iter()
                .any(|a| a.starts_with("mcp_servers.two."))
        );
    }

    fn every_agent() -> [TerminalAgent; 9] {
        [
            TerminalAgent::Claude,
            TerminalAgent::Codex,
            TerminalAgent::Grok,
            TerminalAgent::OpenCode,
            TerminalAgent::Hermes,
            TerminalAgent::Pi,
            TerminalAgent::OhMyPi,
            TerminalAgent::Cursor,
            TerminalAgent::Antigravity,
        ]
    }

    #[test]
    fn every_catalog_agent_with_an_interface_has_a_terminal() {
        for (id, agent) in [
            ("claude", TerminalAgent::Claude),
            ("codex", TerminalAgent::Codex),
            ("grok", TerminalAgent::Grok),
            ("opencode", TerminalAgent::OpenCode),
            ("hermes", TerminalAgent::Hermes),
            ("pi", TerminalAgent::Pi),
            ("omp", TerminalAgent::OhMyPi),
            ("cursor", TerminalAgent::Cursor),
            ("antigravity", TerminalAgent::Antigravity),
        ] {
            assert_eq!(TerminalAgent::of(id), Some(agent), "{id}");
        }
        assert_eq!(TerminalAgent::of("mock"), None);
        assert_eq!(
            TerminalAgent::of("claude-acp"),
            None,
            "connections map to their agent first"
        );
        let named: Vec<_> = every_agent()
            .into_iter()
            .filter(|agent| agent.names_new_sessions())
            .collect();
        assert_eq!(
            named,
            [
                TerminalAgent::Claude,
                TerminalAgent::Grok,
                TerminalAgent::Pi
            ]
        );
    }

    #[test]
    fn each_agent_drops_the_protocol_and_resumes_with_its_own_flag() {
        let expected: [(TerminalAgent, &[&str]); 7] = [
            (TerminalAgent::Grok, &["--resume", "s1"]),
            (TerminalAgent::OpenCode, &["--session", "s1"]),
            (
                TerminalAgent::Hermes,
                &["--resume", "s1", "--no-restore-cwd"],
            ),
            (TerminalAgent::Pi, &["--session", "s1"]),
            (TerminalAgent::OhMyPi, &["--resume", "s1"]),
            (TerminalAgent::Cursor, &["--resume=s1"]),
            (TerminalAgent::Antigravity, &["--conversation", "s1"]),
        ];
        for (agent, resume) in expected {
            let mut chosen = options(&[], TerminalSession::Resume("s1".into()));
            chosen.model = None;
            let made = build(agent, &base(), &chosen);
            let argv = args(&made.launch);
            assert!(
                !argv.iter().any(|a| a == "-p" || a.contains("stream-json")),
                "{agent:?}: {argv:?}"
            );
            assert!(
                argv.windows(resume.len()).any(|window| window == resume),
                "{agent:?}: {argv:?}"
            );
            assert_eq!(made.launch.label, "claude (terminal)");
            assert_eq!(made.launch.cwd, ghost_process::ProcessCwd::ChatFolder);
            // The account's environment stays.
            assert!(
                made.launch
                    .set_env
                    .iter()
                    .any(|(k, _)| k == "CLAUDE_CONFIG_DIR")
            );
        }
        for agent in [TerminalAgent::Grok, TerminalAgent::Pi] {
            let made = build(
                agent,
                &base(),
                &options(&[], TerminalSession::Named("n1".into())),
            );
            let argv = args(&made.launch);
            assert!(
                argv.windows(2).any(|w| w == ["--session-id", "n1"]),
                "{agent:?}: {argv:?}"
            );
        }
    }

    #[test]
    fn access_levels_map_to_each_agents_own_approvals() {
        let with = |agent, autonomy| {
            let mut chosen = options(&[], TerminalSession::New);
            chosen.autonomy = autonomy;
            args(&build(agent, &base(), &chosen).launch)
        };
        assert!(
            with(TerminalAgent::Grok, Autonomy::Ask)
                .windows(2)
                .any(|w| w == ["--permission-mode", "default"])
        );
        assert!(
            with(TerminalAgent::Grok, Autonomy::FullAuto)
                .windows(2)
                .any(|w| w == ["--permission-mode", "bypassPermissions"])
        );
        assert!(with(TerminalAgent::OpenCode, Autonomy::FullAuto).contains(&"--auto".to_owned()));
        assert!(!with(TerminalAgent::OpenCode, Autonomy::Ask).contains(&"--auto".to_owned()));
        assert!(with(TerminalAgent::Hermes, Autonomy::FullAuto).contains(&"--yolo".to_owned()));
        assert!(!with(TerminalAgent::Hermes, Autonomy::AutoEdit).contains(&"--yolo".to_owned()));
        assert!(with(TerminalAgent::Cursor, Autonomy::FullAuto).contains(&"--force".to_owned()));
        assert!(
            with(TerminalAgent::Antigravity, Autonomy::AutoEdit)
                .windows(2)
                .any(|w| w == ["--mode", "accept-edits"])
        );
        assert!(
            with(TerminalAgent::Antigravity, Autonomy::FullAuto)
                .contains(&"--dangerously-skip-permissions".to_owned())
        );
        assert!(
            !with(TerminalAgent::Antigravity, Autonomy::Ask)
                .iter()
                .any(|a| a.contains("dangerously") || a == "--mode")
        );
    }

    #[test]
    fn opencode_takes_shared_servers_and_access_in_its_config_variable_merged_over_the_persons() {
        let secret = format!("value-{}", 3);
        let servers = [
            server(
                "docs",
                McpTransport::Stdio {
                    command: "node".into(),
                    args: vec!["s.js".into()],
                },
                &[("TOKEN", secret.clone())],
            ),
            server(
                "remote",
                McpTransport::Http {
                    url: "https://example.test/mcp".into(),
                },
                &[("X-Key", secret.clone())],
            ),
            server(
                "old",
                McpTransport::Sse {
                    url: "https://example.test/sse".into(),
                },
                &[],
            ),
        ];
        let mut env = EnvMap::new();
        env.insert(
            EnvKey::from("OPENCODE_CONFIG_CONTENT"),
            r#"{"theme":"mine","permission":{"read":"allow"}}"#.into(),
        );
        let mut chosen = options(&servers, TerminalSession::New);
        chosen.autonomy = Autonomy::Ask;
        chosen.model = Some("anthropic/claude-sonnet".into());
        chosen.env = Some(&env);
        let made = opencode(&base(), &chosen);
        let argv = args(&made.launch);
        assert!(!argv.iter().any(|a| a.contains(&secret)), "{argv:?}");
        assert!(
            argv.windows(2)
                .any(|w| w == ["--model", "anthropic/claude-sonnet"])
        );
        let config: Value = made
            .launch
            .set_env
            .iter()
            .find(|(k, _)| k == "OPENCODE_CONFIG_CONTENT")
            .map(|(_, v)| serde_json::from_str(&v.to_string_lossy()).unwrap())
            .expect("config");
        assert_eq!(config["theme"], "mine", "the person's own settings stay");
        assert_eq!(config["permission"]["read"], "allow");
        assert_eq!(config["permission"]["edit"], "ask");
        assert_eq!(config["mcp"]["docs"]["command"], json!(["node", "s.js"]));
        assert_eq!(config["mcp"]["docs"]["environment"]["TOKEN"], json!(secret));
        assert_eq!(config["mcp"]["remote"]["headers"]["X-Key"], json!(secret));
        assert!(config["mcp"].get("old").is_none());
        assert_eq!(made.omitted.len(), 1, "{:?}", made.omitted);
        assert!(made.omitted[0].starts_with("old: "));
        // A model OpenCode cannot name is left out, and said.
        chosen.model = Some("sonnet".into());
        let made = opencode(&base(), &chosen);
        assert!(!args(&made.launch).contains(&"--model".to_owned()));
        assert!(made.omitted.iter().any(|note| note.contains("sonnet")));
    }

    #[test]
    fn pi_loads_shared_servers_through_its_extension_without_credentials_on_the_command_line() {
        let secret = format!("value-{}", 5);
        let servers = [
            server(
                "docs",
                McpTransport::Stdio {
                    command: "node".into(),
                    args: vec!["s.js".into()],
                },
                &[("TOKEN", secret.clone())],
            ),
            server(
                "dotted.name",
                McpTransport::Http {
                    url: "https://example.test/mcp".into(),
                },
                &[],
            ),
        ];
        let extension = PathBuf::from("/tmp/ghost-shared-mcp-1.ts");
        let mut chosen = options(&servers, TerminalSession::Named("n1".into()));
        chosen.extension = Some(&extension);
        let made = pi(&base(), &chosen);
        let argv = args(&made.launch);
        assert!(!argv.iter().any(|a| a.contains(&secret)), "{argv:?}");
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--extension" && w[1].ends_with("ghost-shared-mcp-1.ts"))
        );
        let shared: Value = made
            .launch
            .set_env
            .iter()
            .find(|(k, _)| k == "GHOST_SHARED_MCP")
            .map(|(_, v)| serde_json::from_str(&v.to_string_lossy()).unwrap())
            .expect("servers");
        assert!(shared.get("docs").is_some());
        assert!(shared.get("dotted.name").is_none());
        assert!(
            made.omitted
                .iter()
                .any(|note| note.starts_with("dotted.name: "))
        );
        // Without the extension file nothing is passed, and the chat says so.
        chosen.extension = None;
        let made = pi(&base(), &chosen);
        assert!(!args(&made.launch).contains(&"--extension".to_owned()));
        assert!(made.omitted.iter().any(|note| note.starts_with("docs: ")));
        let file = pi_extension().expect("extension");
        assert_eq!(
            std::fs::read(file.path()).unwrap(),
            crate::pi::SHARED_MCP_EXTENSION
        );
    }

    #[test]
    fn agents_with_their_own_mcp_settings_say_what_their_terminal_leaves_out() {
        let servers = [server(
            "docs",
            McpTransport::Stdio {
                command: "node".into(),
                args: vec![],
            },
            &[],
        )];
        let dirs = [PathBuf::from("/other")];
        for agent in [
            TerminalAgent::Grok,
            TerminalAgent::Hermes,
            TerminalAgent::OhMyPi,
            TerminalAgent::Cursor,
            TerminalAgent::Antigravity,
        ] {
            let mut chosen = options(&servers, TerminalSession::New);
            chosen.add_dirs = &dirs;
            let made = build(agent, &base(), &chosen);
            assert!(
                made.omitted
                    .iter()
                    .any(|note| note.starts_with("docs: ") && note.contains(agent.label())),
                "{agent:?}: {:?}",
                made.omitted
            );
            let takes_folders = matches!(agent, TerminalAgent::Grok | TerminalAgent::Antigravity);
            assert_eq!(
                made.omitted
                    .iter()
                    .any(|note| note.contains("other folders")),
                !takes_folders,
                "{agent:?}"
            );
            assert_eq!(
                args(&made.launch).contains(&"--add-dir".to_owned()),
                takes_folders,
                "{agent:?}"
            );
            let takes_model = agent == TerminalAgent::Grok;
            assert_eq!(
                made.omitted.iter().any(|note| note.contains("(opus)")),
                !takes_model,
                "{agent:?}"
            );
        }
    }

    #[test]
    fn the_interface_program_is_the_connections_the_adapters_cli_or_agy() {
        let env = EnvMap::new();
        let plain = base();
        assert_eq!(
            interactive_program(TerminalAgent::Grok, &plain, None, &env),
            Some(PathBuf::from("/bin/claude"))
        );
        let mut adapter = base();
        adapter.program = PathBuf::from("/usr/bin/node");
        let cli = std::env::temp_dir().join("claude-cli");
        adapter
            .set_env
            .push(("CLAUDE_CODE_EXECUTABLE".into(), cli.clone().into()));
        assert_eq!(
            interactive_program(
                TerminalAgent::Claude,
                &adapter,
                Some("CLAUDE_CODE_EXECUTABLE"),
                &env
            ),
            Some(cli)
        );
        let folder = tempfile::tempdir().unwrap();
        let agy = folder
            .path()
            .join(if cfg!(windows) { "agy.exe" } else { "agy" });
        std::fs::write(&agy, b"").unwrap();
        let mut server = base();
        server.program = folder.path().join("agy_acp_server");
        assert_eq!(
            interactive_program(TerminalAgent::Antigravity, &server, None, &env),
            Some(agy.clone())
        );
        let mut direct = base();
        direct.program = agy.clone();
        assert_eq!(
            interactive_program(TerminalAgent::Antigravity, &direct, None, &env),
            Some(agy)
        );
    }

    #[test]
    fn hermes_exits_on_end_of_input_and_the_others_on_ctrl_c_twice() {
        for agent in every_agent() {
            let input = agent.exit_input();
            assert!(input.starts_with('\u{3}'), "{agent:?}");
            assert_eq!(input.ends_with('\u{4}'), agent == TerminalAgent::Hermes);
        }
    }

    #[test]
    fn the_interface_keeps_a_shims_entry_script_and_drops_only_the_protocol() {
        let os = |items: &[&str]| -> Vec<std::ffi::OsString> {
            items.iter().map(std::ffi::OsString::from).collect()
        };
        // Cursor through its npm shim: node.exe <entry> acp.
        assert_eq!(
            interactive_args(&os(&["C:/cursor/index.js", "acp"]), &["acp"], false),
            os(&["C:/cursor/index.js"])
        );
        assert_eq!(
            interactive_args(&os(&["agent", "stdio"]), &["agent", "stdio"], false),
            os(&[])
        );
        // Another program than the connection's keeps none of its arguments.
        assert_eq!(
            interactive_args(&os(&["C:/adapter/index.js"]), &[], true),
            os(&[])
        );
        // Arguments that do not end with the protocol's are not guessed at.
        assert_eq!(
            interactive_args(&os(&["C:/x/index.js", "serve"]), &["acp"], false),
            os(&[])
        );
    }
}
