//! The catalog: every agent Ghost knows, as data.
//!
//! Each entry is one *connection*: one way to start an agent. Most agents have one. Claude Code
//! and Codex have two (Ghost's own driver for the CLI, and an npm ACP adapter), Antigravity and
//! Grok Build have their ACP server and a restricted text fallback, and OpenCode's 1.x and 2.x
//! lines are qualified separately. People see one agent ([`AgentSpec::agent`]); which connection
//! a new session uses is the device's Connection setting, Automatic by default (the first
//! connection, in catalog order, that can run here). See `docs/plan.md`, "One entry per agent".
//!
//! The npm adapters' version floors are the CLI versions they bundle (claude-agent-acp 0.95.0
//! bundles the Agent SDK with Claude Code 2.1.309; codex-acp 2.2.2 depends on `@openai/codex`
//! ^0.160.1). Ghost always drives the user's own CLI behind an adapter, forced through the
//! adapter's variable ([`UserBinary::env`]), never the one the adapter bundles.

use std::sync::LazyLock;
use std::time::Duration;

use ghost_process::DaemonMatch;
use ghost_protocol::DriverKind;

/// One way to start an agent (a connection). Its `id` is what a saved native session records, so
/// a session always resumes through the connection that made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    pub id: &'static str,
    /// The agent people see and chats name. The agent's preferred connection has `id == agent`;
    /// its other connections follow it in the catalog.
    pub agent: &'static str,
    /// How this connection reaches the agent, for the Connection setting ("Direct", "ACP adapter").
    pub connection: &'static str,
    /// The agent's name, the same for every one of its connections.
    pub name: &'static str,
    pub description: &'static str,
    pub kind: DriverKind,
    /// The driver's dialect for this agent (`"hermes"`, `"claude-acp"`, ...).
    pub dialect: &'static str,
    pub recipe: Recipe,
    pub startup_timeout: Duration,
    /// Turns run in a daemon that outlives the agent process (Hermes' gateway): cancels wait longer,
    /// and resumes compare the replay with stored history.
    pub turns_survive_process: bool,
    /// Copied into the launch spec: spared by the kill rule.
    pub daemons: Vec<DaemonMatch>,
    /// The version Ghost was last verified with, for the "untested version" badge.
    pub last_verified: &'static str,
}

/// How to find and start an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipe {
    /// A binary on the user's `PATH`.
    Binary {
        program: &'static str,
        args: &'static [&'static str],
        version_args: &'static [&'static str],
        min_version: Option<&'static str>,
    },
    /// A vendor-named executable with a protocol baseline and an optional major line boundary.
    /// In particular, Cursor never probes an unrelated executable named `agent`.
    QualifiedBinary {
        program: &'static str,
        args: &'static [&'static str],
        version_args: &'static [&'static str],
        min_version: &'static str,
        major: Option<u64>,
    },
    /// The user's CLI directly, with an explicit executable override and a required version.
    /// Windows shell shims are rejected; no adapter or package installation is involved.
    NativeBinary {
        user_binary: UserBinary,
        /// Protocol transport flags. Session, model and permission flags belong to the driver.
        args: &'static [&'static str],
    },
    /// An npm adapter installed once per version into `<data root>/adapters`, launched as
    /// `node <entry>` from its own folder, driving the user's own CLI.
    NpmAdapter {
        package: &'static str,
        version: &'static str,
        /// The package's `bin` entry to run.
        bin: &'static str,
        min_node_major: u32,
        user_binary: UserBinary,
    },
    /// Runs inside Ghost (the demo agent); no process.
    Builtin,
}

/// The user's own CLI, run directly or by an adapter. An adapter always sets `env` to its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserBinary {
    /// "Claude Code", for messages.
    pub label: &'static str,
    pub program: &'static str,
    /// Executable override: `CLAUDE_CODE_EXECUTABLE`, `CODEX_PATH`.
    pub env: &'static str,
    pub version_args: &'static [&'static str],
    pub min_version: &'static str,
}

/// Every connection Ghost knows, its agents in the order the New chat screen lists them and
/// each agent's connections in preference order. Built once.
pub fn catalog() -> &'static [AgentSpec] {
    static CATALOG: LazyLock<Vec<AgentSpec>> = LazyLock::new(build_catalog);
    &CATALOG
}

/// The connection with this id.
pub fn get(id: &str) -> Option<&'static AgentSpec> {
    catalog().iter().find(|spec| spec.id == id)
}

/// The agent a stored id names: `"claude-acp"` (a chat or project saved before agents had one
/// entry each) reads as `"claude"`. An id the catalog does not know names itself, so data from a
/// newer build keeps its id.
pub fn agent_of(id: &str) -> &str {
    get(id).map_or(id, |spec| spec.agent)
}

/// The agent `id` names has an interactive interface of its own that a chat can open in
/// Ghost's terminal (`ghost_drivers::terminal::TerminalAgent` builds its launch).
pub fn has_terminal(id: &str) -> bool {
    matches!(
        agent_of(id),
        "claude"
            | "codex"
            | "grok"
            | "opencode"
            | "hermes"
            | "pi"
            | "omp"
            | "cursor"
            | "antigravity"
    )
}

/// The connections of the agent `id` names, preferred first; empty for an id the catalog does not
/// know.
pub fn connections(id: &str) -> Vec<&'static AgentSpec> {
    let agent = agent_of(id);
    catalog()
        .iter()
        .filter(|spec| spec.agent == agent)
        .collect()
}

const START_TIMEOUT: Duration = Duration::from_secs(60);

fn build_catalog() -> Vec<AgentSpec> {
    vec![
        AgentSpec {
            id: "hermes",
            agent: "hermes",
            connection: "ACP",
            name: "Hermes",
            description: "Hermes Agent through its ACP mode (hermes acp). Its turns run in the Hermes gateway.",
            kind: DriverKind::Acp,
            dialect: "hermes",
            recipe: Recipe::Binary {
                program: "hermes",
                args: &["acp"],
                version_args: &["--version"],
                min_version: None,
            },
            // The gateway may start first.
            startup_timeout: START_TIMEOUT,
            turns_survive_process: true,
            daemons: vec![DaemonMatch {
                program: None,
                args_in_order: vec!["hermes_cli.main".into(), "gateway".into(), "run".into()],
            }],
            last_verified: "Hermes 0.22.5",
        },
        AgentSpec {
            id: "claude",
            agent: "claude",
            connection: "Direct",
            name: "Claude Code",
            description: "Your Claude Code, driven directly through its native control protocol.",
            kind: DriverKind::Claude,
            dialect: "claude",
            recipe: Recipe::NativeBinary {
                user_binary: UserBinary {
                    label: "Claude Code",
                    program: "claude",
                    env: "CLAUDE_CODE_EXECUTABLE",
                    version_args: &["--version"],
                    min_version: "2.1.310",
                },
                args: &[
                    "-p",
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--include-partial-messages",
                    "--permission-prompt-tool",
                    "stdio",
                ],
            },
            startup_timeout: START_TIMEOUT,
            turns_survive_process: false,
            daemons: vec![DaemonMatch {
                program: Some("claude".into()),
                args_in_order: vec!["daemon".into(), "run".into()],
            }],
            last_verified: "Claude Code 2.1.310 (macOS)",
        },
        AgentSpec {
            id: "claude-acp",
            agent: "claude",
            connection: "ACP adapter",
            name: "Claude Code",
            description: "Your Claude Code, driven through the claude-agent-acp adapter.",
            kind: DriverKind::Acp,
            dialect: "claude-acp",
            recipe: Recipe::NpmAdapter {
                package: "@agentclientprotocol/claude-agent-acp",
                version: "0.95.0",
                bin: "claude-agent-acp",
                min_node_major: 22,
                user_binary: UserBinary {
                    label: "Claude Code",
                    program: "claude",
                    env: "CLAUDE_CODE_EXECUTABLE",
                    version_args: &["--version"],
                    min_version: "2.1.309",
                },
            },
            startup_timeout: START_TIMEOUT,
            turns_survive_process: false,
            // A precaution: seen as `claude daemon run --origin transient ...` on this Mac.
            daemons: vec![DaemonMatch {
                program: Some("claude".into()),
                args_in_order: vec!["daemon".into(), "run".into()],
            }],
            last_verified: "adapter 0.95.0 with Claude Code 2.1.310",
        },
        AgentSpec {
            id: "codex",
            agent: "codex",
            connection: "Direct",
            name: "Codex",
            description: "Your Codex CLI, driven directly through its native app-server.",
            kind: DriverKind::Codex,
            dialect: "codex",
            recipe: Recipe::NativeBinary {
                user_binary: UserBinary {
                    label: "Codex",
                    program: "codex",
                    env: "CODEX_PATH",
                    version_args: &["--version"],
                    min_version: "0.162.1",
                },
                args: &["app-server", "--listen", "stdio://"],
            },
            startup_timeout: START_TIMEOUT,
            turns_survive_process: false,
            daemons: vec![DaemonMatch {
                program: Some("codex".into()),
                args_in_order: vec!["app-server".into(), "daemon".into()],
            }],
            last_verified: "Codex 0.162.1 free controls and fixtures (Windows); prompted verification pending",
        },
        AgentSpec {
            id: "codex-acp",
            agent: "codex",
            connection: "ACP adapter",
            name: "Codex",
            description: "Your Codex CLI, driven through the codex-acp adapter.",
            kind: DriverKind::Acp,
            dialect: "codex-acp",
            recipe: Recipe::NpmAdapter {
                package: "@agentclientprotocol/codex-acp",
                version: "2.2.2",
                bin: "codex-acp",
                min_node_major: 22,
                user_binary: UserBinary {
                    label: "Codex",
                    program: "codex",
                    env: "CODEX_PATH",
                    version_args: &["--version"],
                    min_version: "0.160.1",
                },
            },
            startup_timeout: START_TIMEOUT,
            turns_survive_process: false,
            daemons: Vec::new(),
            last_verified: "adapter 2.2.2 with Codex 0.164.0",
        },
        acp_binary(
            "antigravity",
            "Antigravity",
            "Google's separate ACP server. Install its complete official distribution and add it to PATH.",
            "antigravity",
            if cfg!(windows) {
                "agy_acp_server"
            } else {
                "agy_acp_server.par"
            },
            if cfg!(target_os = "linux") {
                &["--uid="]
            } else {
                &[]
            },
            "1.4.3",
            Some(1),
        ),
        acp_binary(
            "cursor",
            "Cursor",
            "Cursor CLI through ACP. Sign in using your own cursor-agent login.",
            "cursor",
            "cursor-agent",
            &["acp"],
            "2026.10.22",
            None,
        ),
        acp_binary(
            "grok",
            "Grok Build",
            "Your Grok Build CLI through its ACP server.",
            "grok",
            "grok",
            &["agent", "stdio"],
            "1.0.70",
            Some(1),
        ),
        acp_binary(
            "omp",
            "oh-my-pi",
            "oh-my-pi through ACP, with explicit extension dialogs and plan approval.",
            "omp",
            "omp",
            &["acp"],
            "18.12.6",
            Some(18),
        ),
        AgentSpec {
            connection: "OpenCode 1.x",
            ..acp_binary(
                "opencode",
                "OpenCode",
                "OpenCode 1.x through ACP; qualified separately from 2.x.",
                "opencode",
                "opencode",
                &["acp"],
                "1.19.4",
                Some(1),
            )
        },
        AgentSpec {
            agent: "opencode",
            connection: "OpenCode 2.x",
            ..acp_binary(
                "opencode-v2",
                "OpenCode",
                "OpenCode 2.x through ACP; qualified separately from 1.x.",
                "opencode-v2",
                "opencode",
                &["acp"],
                "2.1.4",
                Some(2),
            )
        },
        AgentSpec {
            id: "agy-stream",
            agent: "antigravity",
            connection: "Text fallback",
            name: "Antigravity",
            description: "Antigravity's restricted text transport. No interactive approvals; Stop ends the owned process.",
            kind: DriverKind::Headless,
            dialect: "agy-stream",
            recipe: Recipe::QualifiedBinary {
                program: "agy",
                args: &[
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                ],
                version_args: &["--version"],
                min_version: "1.4.7",
                major: Some(1),
            },
            startup_timeout: START_TIMEOUT,
            turns_survive_process: false,
            daemons: Vec::new(),
            last_verified: "Unverified: documented stream contract only",
        },
        AgentSpec {
            id: "grok-stream",
            agent: "grok",
            connection: "Text fallback",
            name: "Grok Build",
            description: "Grok Build's restricted text transport: one owned process per visible prompt. No interactive approvals; Stop ends the process.",
            kind: DriverKind::Headless,
            dialect: "grok-stream",
            recipe: Recipe::NativeBinary {
                user_binary: UserBinary {
                    label: "Grok Build",
                    program: "grok",
                    env: "GROK_EXECUTABLE",
                    version_args: &["--version"],
                    min_version: "1.0.70",
                },
                args: &["--output-format", "streaming-json"],
            },
            startup_timeout: START_TIMEOUT,
            turns_survive_process: false,
            daemons: Vec::new(),
            last_verified: "Unverified: pinned source contract only",
        },
        AgentSpec {
            id: "pi",
            agent: "pi",
            connection: "Direct",
            name: "Pi",
            description: "Your Pi agent, driven directly through its native RPC protocol.",
            kind: DriverKind::Pi,
            dialect: "pi",
            recipe: Recipe::Binary {
                program: "pi",
                args: &[],
                version_args: &["--version"],
                min_version: Some("1.0.2"),
            },
            startup_timeout: START_TIMEOUT,
            turns_survive_process: false,
            daemons: Vec::new(),
            last_verified: "Pi 1.0.2 protocol (real-agent acceptance pending)",
        },
        AgentSpec {
            id: "mock",
            agent: "mock",
            connection: "Built in",
            name: "Demo agent (scripted, no AI)",
            description: "A scripted agent for trying Ghost without an AI: it streams a demo turn, asks for permissions and questions, and can fail on purpose.",
            kind: DriverKind::Mock,
            dialect: "mock",
            recipe: Recipe::Builtin,
            startup_timeout: Duration::from_secs(5),
            turns_survive_process: false,
            daemons: Vec::new(),
            last_verified: "auracode",
        },
    ]
}

#[allow(
    clippy::too_many_arguments,
    reason = "one declarative row per catalog entry"
)]
fn acp_binary(
    id: &'static str,
    name: &'static str,
    description: &'static str,
    dialect: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    min_version: &'static str,
    major: Option<u64>,
) -> AgentSpec {
    AgentSpec {
        id,
        agent: id,
        connection: "ACP server",
        name,
        description,
        kind: DriverKind::Acp,
        dialect,
        recipe: Recipe::QualifiedBinary {
            program,
            args,
            version_args: &["--version"],
            min_version,
            major,
        },
        startup_timeout: START_TIMEOUT,
        turns_survive_process: false,
        daemons: Vec::new(),
        last_verified: "Unverified: documentation and synthetic protocol tests only",
    }
}

/// The first `x.y.z` in `text` (a leading `v` is allowed and dropped): `"Claude Code 2.1.310"` gives
/// `"2.1.310"`, `"v26.0.0"` gives `"26.0.0"`.
pub fn parse_version(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        let starts_run =
            bytes[start].is_ascii_digit() && (start == 0 || !bytes[start - 1].is_ascii_digit());
        if !starts_run {
            continue;
        }
        if let Some(end) = version_end(bytes, start) {
            return text.get(start..end).map(str::to_owned);
        }
    }
    None
}

/// Where `digits.digits.digits` starting at `start` ends, if it is there.
fn version_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut at = start;
    for part in 0..3 {
        let digits = bytes[at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        at += digits;
        if part < 2 {
            if bytes.get(at) != Some(&b'.') {
                return None;
            }
            at += 1;
        }
    }
    Some(at)
}

/// Whether `found` is at least `floor`, comparing `x.y.z` numerically. A version that does not
/// parse never satisfies a floor.
pub fn version_at_least(found: &str, floor: &str) -> bool {
    match (numeric(found), numeric(floor)) {
        (Some(found), Some(floor)) => found >= floor,
        _ => false,
    }
}

fn numeric(version: &str) -> Option<(u64, u64, u64)> {
    let version = parse_version(version)?;
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn the_catalog_lists_each_connection_once_native_claude_before_its_adapter() {
        let ids: Vec<&str> = catalog().iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                "hermes",
                "claude",
                "claude-acp",
                "codex",
                "codex-acp",
                "antigravity",
                "cursor",
                "grok",
                "omp",
                "opencode",
                "opencode-v2",
                "agy-stream",
                "grok-stream",
                "pi",
                "mock"
            ]
        );
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert_eq!(get("codex-acp").map(|spec| spec.name), Some("Codex"));
        assert!(get("agent").is_none());
        assert!(std::ptr::eq(catalog(), catalog()), "built once");
    }

    #[test]
    fn each_agent_is_one_entry_whose_preferred_connection_comes_first() {
        let mut agents: Vec<&str> = Vec::new();
        for spec in catalog() {
            if !agents.contains(&spec.agent) {
                agents.push(spec.agent);
            }
        }
        assert_eq!(
            agents,
            [
                "hermes",
                "claude",
                "codex",
                "antigravity",
                "cursor",
                "grok",
                "omp",
                "opencode",
                "pi",
                "mock"
            ]
        );
        for agent in &agents {
            let connections = connections(agent);
            // The preferred connection carries the agent's own id, and its name is the agent's.
            assert_eq!(connections[0].id, *agent, "{agent}");
            let labels: HashSet<&str> = connections.iter().map(|spec| spec.connection).collect();
            assert_eq!(labels.len(), connections.len(), "{agent}: labels differ");
            assert!(
                connections
                    .iter()
                    .all(|spec| spec.name == connections[0].name),
                "{agent}: one name"
            );
        }
        let ids =
            |agent: &str| -> Vec<&str> { connections(agent).iter().map(|spec| spec.id).collect() };
        assert_eq!(ids("claude"), ["claude", "claude-acp"]);
        assert_eq!(ids("codex"), ["codex", "codex-acp"]);
        assert_eq!(ids("antigravity"), ["antigravity", "agy-stream"]);
        assert_eq!(ids("grok"), ["grok", "grok-stream"]);
        assert_eq!(ids("opencode"), ["opencode", "opencode-v2"]);
        assert_eq!(ids("pi"), ["pi"]);
        // A connection id names its agent, and so finds the same connections.
        assert_eq!(ids("claude-acp"), ["claude", "claude-acp"]);
        for (stored, agent) in [
            ("claude-acp", "claude"),
            ("codex-acp", "codex"),
            ("agy-stream", "antigravity"),
            ("grok-stream", "grok"),
            ("opencode-v2", "opencode"),
            ("claude", "claude"),
            ("mock", "mock"),
            // Data from a newer build keeps its id.
            ("gemini", "gemini"),
            ("", ""),
        ] {
            assert_eq!(agent_of(stored), agent, "{stored}");
        }
        assert!(connections("gemini").is_empty());
        assert_eq!(get("claude").map(|spec| spec.connection), Some("Direct"));
        assert_eq!(
            get("claude-acp").map(|spec| spec.connection),
            Some("ACP adapter")
        );
    }

    #[test]
    fn each_agent_has_its_drivers_and_limits() {
        let hermes = get("hermes").expect("hermes");
        assert_eq!(hermes.kind, DriverKind::Acp);
        assert!(hermes.turns_survive_process);
        assert_eq!(hermes.startup_timeout, Duration::from_secs(60));
        assert_eq!(
            hermes.daemons[0].args_in_order,
            ["hermes_cli.main", "gateway", "run"]
        );
        assert!(matches!(
            hermes.recipe,
            Recipe::Binary {
                program: "hermes",
                args: ["acp"],
                min_version: None,
                ..
            }
        ));

        let native = get("claude").expect("native Claude");
        assert_eq!(native.kind, DriverKind::Claude);
        assert_eq!(native.dialect, "claude");
        assert!(!native.turns_survive_process);
        assert!(matches!(
            native.recipe,
            Recipe::NativeBinary {
                user_binary: UserBinary {
                    program: "claude",
                    env: "CLAUDE_CODE_EXECUTABLE",
                    min_version: "2.1.310",
                    ..
                },
                ..
            }
        ));

        let claude = get("claude-acp").expect("claude");
        let Recipe::NpmAdapter {
            package,
            version,
            bin,
            min_node_major,
            user_binary,
        } = &claude.recipe
        else {
            panic!("claude-acp is an npm adapter");
        };
        assert_eq!(
            (*package, *version, *bin, *min_node_major),
            (
                "@agentclientprotocol/claude-agent-acp",
                "0.95.0",
                "claude-agent-acp",
                22
            )
        );
        assert_eq!(
            (
                user_binary.program,
                user_binary.env,
                user_binary.min_version
            ),
            ("claude", "CLAUDE_CODE_EXECUTABLE", "2.1.309")
        );
        assert_eq!(claude.daemons[0].program.as_deref(), Some("claude"));

        let codex = get("codex-acp").expect("codex");
        assert!(matches!(
            &codex.recipe,
            Recipe::NpmAdapter {
                version: "2.2.2",
                user_binary: UserBinary {
                    env: "CODEX_PATH",
                    min_version: "0.160.1",
                    ..
                },
                ..
            }
        ));
        assert!(codex.daemons.is_empty());

        let mock = get("mock").expect("mock");
        assert_eq!(mock.kind, DriverKind::Mock);
        assert_eq!(mock.recipe, Recipe::Builtin);
        assert_eq!(mock.startup_timeout, Duration::from_secs(5));
    }

    #[test]
    fn versions_parse_from_cli_output() {
        for (text, version) in [
            ("2.1.310 (Claude Code)", Some("2.1.310")),
            ("codex-cli 0.164.0", Some("0.164.0")),
            ("Hermes Agent v0.22.5", Some("0.22.5")),
            ("v26.0.0", Some("26.0.0")),
            ("11.12.1\n", Some("11.12.1")),
            ("node 22.1 then 1.2.3.4", Some("1.2.3")),
            ("build 20251024.1.2", Some("20251024.1.2")),
            ("1.2", None),
            ("1..2.3", None),
            ("", None),
            ("no digits", None),
        ] {
            assert_eq!(parse_version(text).as_deref(), version, "{text:?}");
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_at_least("2.1.310", "2.1.309"));
        assert!(version_at_least("2.1.309", "2.1.309"));
        assert!(!version_at_least("2.1.99", "2.1.309"));
        assert!(version_at_least("0.164.0", "0.160.1"));
        assert!(!version_at_least("0.9.9", "0.160.1"));
        assert!(version_at_least("v26.0.0", "22.0.0"));
        assert!(!version_at_least("garbage", "1.0.0"));
        assert!(!version_at_least("1.0.0", "garbage"));
        assert!(!version_at_least("99999999999999999999.0.0", "1.0.0"));
    }
}
