//! IPC and session-state types: what the desktop commands and channels exchange about chats,
//! agents, options, history, storage and the environment.
//!
//! Same conventions as the crate root: `snake_case` names, data-carrying enums tagged with `"type"`
//! (the availability and state enums with `"state"`), timestamps as [`Millis`].

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::lenient::lenient_enum;
use crate::{
    AutonomyState, Capabilities, ErrorKind, GhostEvent, LimitWindow, Millis, SessionInfo,
    SessionStatus, SlashCommand, UsageInfo,
};

lenient_enum! {
    /// What an option controls, which decides where the UI shows it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
    #[serde(rename_all = "snake_case")]
    pub enum ConfigCategory {
        Model = "model",
        Effort = "effort",
        Mode = "mode",
        ModelConfig = "model_config",
        Other = "other",
    }
    fallback = Other;
}

/// One session option the agent offers (or the driver synthesized from its legacy lists).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ConfigOption {
    /// Opaque to the UI; the driver maps it back to the wire.
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: ConfigCategory,
    pub kind: ConfigKind,
    /// A change waiting for the turn to end (Hermes changes mode only while idle).
    pub pending: Option<ConfigValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigKind {
    Select {
        current: String,
        choices: Vec<ConfigChoice>,
    },
    Toggle {
        current: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ConfigChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
    /// The group the agent listed it under, when its choices were grouped.
    pub group: Option<String>,
}

/// A value for a [`ConfigOption`]: a choice for a select, a flag for a toggle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigValue {
    Choice { value: String },
    Toggle { value: bool },
}

/// What the header's pickers and the slash menu show, fetched with `chat_lists` whenever
/// [`SessionInfo::revision`] changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionLists {
    #[serde(default)]
    pub controls: SessionControls,
    pub revision: u32,
    /// `None` until the agent has started once.
    pub capabilities: Option<Capabilities>,
    pub options: Vec<ConfigOption>,
    pub commands: Vec<SlashCommand>,
    pub autonomy: AutonomyState,
    /// What the running session loaded, where its agent reports it.
    #[serde(default)]
    pub loaded: Option<SessionLoaded>,
}

/// What a running session loaded, as its agent reports it once it starts: MCP servers with
/// their state, skills, plugins and subagents. Each agent reports what it can: Claude all of it,
/// Codex its MCP servers, Pi its skills.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionLoaded {
    pub mcp_servers: Vec<LoadedMcpServer>,
    pub skills: Vec<String>,
    pub plugins: Vec<LoadedPlugin>,
    pub agents: Vec<String>,
    /// How many tools the agent offers, MCP tools included.
    pub tools: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LoadedMcpServer {
    pub name: String,
    pub state: McpLoadState,
    /// Where it was configured (user, project, a plugin…), as the agent names it.
    pub source: Option<String>,
}

/// An MCP server's state, in the few words every agent's states come to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum McpLoadState {
    Connected,
    Starting,
    NeedsSignIn,
    Failed,
    Off,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LoadedPlugin {
    pub name: String,
    pub version: Option<String>,
}

impl SessionLoaded {
    /// Bounded, so a large setup cannot swell the session's lists: 64 servers, 256 skills, 128
    /// plugins and 128 subagents, each name cut at 128 characters.
    pub fn bounded(mut self) -> Self {
        let cut = |text: &mut String| {
            if let Some((at, _)) = text.char_indices().nth(128) {
                text.truncate(at);
            }
        };
        self.mcp_servers.truncate(64);
        self.skills.truncate(256);
        self.plugins.truncate(128);
        self.agents.truncate(128);
        for server in &mut self.mcp_servers {
            cut(&mut server.name);
            if let Some(source) = &mut server.source {
                cut(source);
            }
        }
        for plugin in &mut self.plugins {
            cut(&mut plugin.name);
            if let Some(version) = &mut plugin.version {
                cut(version);
            }
        }
        self.skills.iter_mut().chain(&mut self.agents).for_each(cut);
        self
    }
}

/// Runtime native control choices, independent from legacy autonomy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct SessionControls {
    pub version: u32,
    pub access: SessionControl,
    pub workflow: SessionControl,
}
impl Default for SessionControls {
    fn default() -> Self {
        Self {
            version: 1,
            access: SessionControl::default(),
            workflow: SessionControl::default(),
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct SessionControl {
    pub requested: Option<String>,
    pub effective: Option<String>,
    pub pending: Option<String>,
    pub choices: Vec<SessionControlChoice>,
    pub native_mode: Option<String>,
    pub note: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionControlChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
    pub legacy_autonomy: Option<crate::Autonomy>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionControlChange {
    Access {
        value: String,
    },
    Workflow {
        value: String,
        #[serde(default)]
        implementation_approved: bool,
    },
}
/// Persisted separately for each native session; unknown versions fail closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionControlPreferences {
    pub version: u32,
    pub access: Option<String>,
    pub workflow: Option<String>,
}
impl Default for SessionControlPreferences {
    fn default() -> Self {
        Self {
            version: 1,
            access: None,
            workflow: None,
        }
    }
}

/// Whether a chat's folder can be used on this machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FolderState {
    #[default]
    Ok,
    Missing,
    /// The OS refused access (macOS privacy).
    NotAllowed,
}

/// A chat as the chat list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChatSummary {
    pub id: String,
    pub title: Option<String>,
    pub agent_id: String,
    #[serde(default)]
    pub agent_host: crate::agents::AgentHost,
    pub agent_name: String,
    pub folder: String,
    pub folder_state: FolderState,
    pub status: SessionStatus,
    /// A permission card or question waits for the user.
    pub needs_attention: bool,
    pub created_at: Millis,
    pub updated_at: Millis,
}

/// A change to one chat's row in the list, sent over the status channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ChatStatusUpdate {
    pub chat_id: String,
    pub status: SessionStatus,
    pub needs_attention: bool,
    pub title: Option<String>,
    pub updated_at: Millis,
}

/// What `chat_attach` returns: the snapshot and the newest history page, consistent with the frames
/// the subscription carries from `next_seq` on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ChatAttach {
    pub subscription_id: String,
    pub next_seq: u32,
    pub snapshot: ChatSnapshot,
    pub history: HistoryPage,
}

/// Everything about a chat that is not in its history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ChatSnapshot {
    pub chat: ChatSummary,
    pub session: SessionInfo,
    pub queue: Vec<QueuedPrompt>,
    pub open_turn_id: Option<String>,
    pub usage: Option<UsageInfo>,
    /// The last `LimitsUpdated` (never stored).
    pub limits: Option<Vec<LimitWindow>>,
    pub last_error: Option<ErrorInfo>,
    pub auth: Option<AuthPrompt>,
    pub storage: StorageStatus,
    /// The background tasks the live session knows: every running one, and those that ended since
    /// the current or last turn started. A running task in history that is not here ended with an
    /// earlier process.
    #[serde(default)]
    pub tasks: Vec<crate::tasks::BackgroundTask>,
}

/// One page of a chat's stored history, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct HistoryPage {
    pub chat_id: String,
    pub items: Vec<HistoryItem>,
    pub has_more: bool,
    /// Pass to `chat_history` for the next older page.
    pub before_seq: Option<u32>,
    /// The oldest items belong to this turn, whose earlier part is in the next older page.
    pub cut_turn_id: Option<String>,
    /// Saving is paused: rows from this seq up to the page's cursor are not saved yet and not in
    /// the page.
    pub unsaved_from: Option<u32>,
    /// Rows that did not parse (written by a newer build).
    pub skipped: u32,
    /// The agent that ran each turn with rows in the page, so an earlier turn keeps its agent
    /// after the chat switches agents. A turn missing here is the chat's current agent's.
    #[serde(default)]
    pub turn_agents: Vec<TurnAgent>,
}

/// The agent one turn ran on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TurnAgent {
    pub turn_id: String,
    pub agent_id: String,
    /// The agent's display name ("Claude Code").
    pub agent_name: String,
}

/// One stored event and where it sits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct HistoryItem {
    pub seq: u32,
    pub turn_id: Option<String>,
    pub at: Millis,
    pub event: GhostEvent,
}

/// What `chat_send` did with the prompt: sent at once, or queued behind a turn or a start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PromptAck {
    pub prompt_id: String,
    pub queued: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct QueuedPrompt {
    pub prompt_id: String,
    /// The prompt's first 256 characters.
    pub preview: String,
    /// Queued before a stop, error, crash or restart: sent only after the user says so.
    pub held: bool,
    pub created_at: Millis,
}

/// The complete persisted prompt currently saving its checkpoint before dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SendingPrompt {
    pub prompt_id: String,
    pub text: String,
    pub parts: Vec<crate::PromptPart>,
    pub created_at: Millis,
}

/// The error a chat last showed, for the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ErrorInfo {
    pub kind: ErrorKind,
    pub message: String,
    pub detail: Option<String>,
}

/// The agent needs the user to sign in with its own command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AuthPrompt {
    pub message: String,
    pub commands: Vec<AuthCommand>,
}

/// A sign-in command the user runs in a terminal with their own binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AuthCommand {
    pub label: String,
    pub argv: Vec<String>,
    /// Variables the method sets (ACP `AuthMethodTerminal.env`).
    pub env: Vec<(String, String)>,
}

/// An agent Ghost can start, as the New chat screen shows it: one entry per agent. For an
/// agent with several connections, every field but `id` and `name` describes the connection a
/// new session would use now (or the preferred one while none can run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub availability: Availability,
    /// How Ghost starts it, for display.
    pub launch: String,
    /// The first start downloads the agent's adapter.
    pub needs_download: bool,
    pub last_verified: String,
    /// Present only after a successful version probe of the user's installation.
    #[serde(default)]
    pub installation: Option<crate::agents::AgentInstallation>,
    #[serde(default)]
    pub latest_release: Option<crate::agents::LatestRelease>,
    /// The installed version against its latest known release, computed by detection.
    #[serde(default)]
    pub harness: Option<crate::plugins::HarnessStatus>,
    /// The ways Ghost can start this agent, when it has more than one (Claude Code's own
    /// protocol or its ACP adapter). `None` for an agent with one connection.
    #[serde(default)]
    pub connections: Option<AgentConnections>,
    /// A chat with this agent can open in its own interactive interface, in Ghost's
    /// terminal (Chat | Terminal).
    #[serde(default)]
    pub terminal: bool,
}

/// An agent's connections and the device's Connection setting for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentConnections {
    /// The connection chosen in the agent's details; `None` is Automatic: the first connection,
    /// in `options` order, that can run here.
    pub preferred: Option<String>,
    /// The connection a new session uses now. `None` while detection runs or when none can run.
    /// A chat's saved session keeps resuming through the connection that made it.
    pub active: Option<String>,
    pub options: Vec<AgentConnection>,
}

/// One way Ghost can start an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentConnection {
    /// The connection's catalog id (`claude-acp`), which saved native sessions record.
    pub id: String,
    /// "Direct", "ACP adapter".
    pub label: String,
    pub description: String,
    pub availability: Availability,
    pub launch: String,
    pub needs_download: bool,
}

/// Whether an agent can start here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Availability {
    /// Detection has not finished yet.
    #[default]
    Checking,
    /// `path` is the binary that will actually run (the user's own CLI behind an adapter).
    Ready {
        version: Option<String>,
        path: String,
    },
    Missing {
        reason: String,
    },
    Broken {
        reason: String,
    },
}

/// The environment agents start with, as the banner shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct EnvStatus {
    pub state: EnvState,
    /// Names the scrub removed; never values.
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EnvState {
    #[default]
    Resolving,
    Ready {
        source: String,
        took_ms: u32,
        vars: u32,
    },
    Fallback {
        reason: String,
    },
}

/// Whether the profile database is saving.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StorageStatus {
    pub state: StorageState,
    pub unsaved_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StorageState {
    #[default]
    Ok,
    Paused {
        reason: PauseReason,
        since: Millis,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    DiskFull,
    Busy,
}

/// One page of a blob's text, for "Load full output".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BlobText {
    pub sha256: String,
    pub offset: u64,
    /// Where the next page starts (a character boundary).
    pub next_offset: u64,
    pub total: u64,
    pub text: String,
    pub eof: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AutoAnswer, Autonomy, StopReason};
    use serde_json::json;

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + serde::de::DeserializeOwned,
    {
        let text = serde_json::to_string(value).expect("serialize");
        serde_json::from_str(&text).expect("deserialize")
    }

    fn summary() -> ChatSummary {
        ChatSummary {
            agent_host: Default::default(),
            id: "c".into(),
            title: None,
            agent_id: "hermes".into(),
            agent_name: "Hermes".into(),
            folder: "/work/repo".into(),
            folder_state: FolderState::NotAllowed,
            status: SessionStatus::Ready,
            needs_attention: true,
            created_at: 1,
            updated_at: 2,
        }
    }

    #[test]
    fn options_and_lists_round_trip() {
        let lists = SessionLists {
            loaded: None,
            controls: Default::default(),
            revision: 3,
            capabilities: Some(Capabilities {
                interrupt: true,
                autonomy_levels: vec![Autonomy::FullAuto, Autonomy::AutoEdit, Autonomy::Ask],
                ..Default::default()
            }),
            options: vec![
                ConfigOption {
                    id: "model".into(),
                    name: "Model".into(),
                    description: None,
                    category: ConfigCategory::Model,
                    kind: ConfigKind::Select {
                        current: "fast".into(),
                        choices: vec![ConfigChoice {
                            value: "fast".into(),
                            name: "Fast".into(),
                            description: Some("Quick".into()),
                            group: Some("Provider".into()),
                        }],
                    },
                    pending: Some(ConfigValue::Choice {
                        value: "smart".into(),
                    }),
                },
                ConfigOption {
                    id: "fast".into(),
                    name: "Fast mode".into(),
                    description: None,
                    category: ConfigCategory::Other,
                    kind: ConfigKind::Toggle { current: true },
                    pending: None,
                },
            ],
            commands: vec![SlashCommand {
                name: "review".into(),
                description: "Review".into(),
                input_hint: None,
            }],
            autonomy: AutonomyState {
                requested: Autonomy::FullAuto,
                native_mode: None,
                auto_answer: AutoAnswer::All,
                note: None,
            },
        };
        assert_eq!(round_trip(&lists), lists);
        let value = serde_json::to_value(&lists).expect("serialize");
        assert_eq!(value["options"][0]["kind"]["type"], "select");
        assert_eq!(value["options"][0]["pending"]["type"], "choice");
        assert_eq!(value["options"][1]["category"], "other");
        assert_eq!(
            serde_json::to_value(ConfigValue::Toggle { value: false }).expect("serialize"),
            json!({"type": "toggle", "value": false})
        );
    }

    #[test]
    fn an_option_category_from_a_newer_agent_reads_as_other() {
        let option: ConfigOption = serde_json::from_value(json!({
            "id": "x", "name": "X", "description": null, "category": "temperature",
            "kind": {"type": "toggle", "current": false}, "pending": null
        }))
        .expect("parse");
        assert_eq!(option.category, ConfigCategory::Other);
    }

    #[test]
    fn attach_and_history_round_trip() {
        let attach = ChatAttach {
            subscription_id: "s".into(),
            next_seq: 42,
            snapshot: ChatSnapshot {
                chat: summary(),
                session: SessionInfo::default(),
                queue: vec![QueuedPrompt {
                    prompt_id: "p".into(),
                    preview: "Fix it".into(),
                    held: true,
                    created_at: 5,
                }],
                open_turn_id: Some("t".into()),
                usage: Some(UsageInfo::default()),
                limits: Some(Vec::new()),
                last_error: Some(ErrorInfo {
                    kind: ErrorKind::StartTimeout,
                    message: "m".into(),
                    detail: None,
                }),
                auth: Some(AuthPrompt {
                    message: "Hermes needs you to sign in.".into(),
                    commands: vec![AuthCommand {
                        label: "Sign in".into(),
                        argv: vec!["hermes".into(), "acp".into(), "--setup".into()],
                        env: vec![("HERMES_HOME".into(), "/h".into())],
                    }],
                }),
                storage: StorageStatus {
                    state: StorageState::Paused {
                        reason: PauseReason::DiskFull,
                        since: 9,
                    },
                    unsaved_bytes: 1024,
                },
                tasks: Vec::new(),
            },
            history: HistoryPage {
                chat_id: "c".into(),
                items: vec![HistoryItem {
                    seq: 41,
                    turn_id: Some("t".into()),
                    at: 7,
                    event: GhostEvent::TurnEnded {
                        turn_id: "t".into(),
                        stop_reason: StopReason::EndTurn,
                        usage: None,
                        duration_ms: Some(10),
                        recovered: false,
                    },
                }],
                has_more: true,
                before_seq: Some(41),
                cut_turn_id: None,
                unsaved_from: Some(40),
                skipped: 1,
                turn_agents: vec![TurnAgent {
                    turn_id: "t".into(),
                    agent_id: "claude".into(),
                    agent_name: "Claude Code".into(),
                }],
            },
        };
        assert_eq!(round_trip(&attach), attach);
        let value = serde_json::to_value(&attach).expect("serialize");
        assert_eq!(value["snapshot"]["storage"]["state"]["state"], "paused");
        assert_eq!(
            value["snapshot"]["auth"]["commands"][0]["env"],
            json!([["HERMES_HOME", "/h"]])
        );
    }

    #[test]
    fn state_enums_are_tagged_with_state() {
        assert_eq!(
            serde_json::to_value(Availability::Ready {
                version: Some("Claude Code 2.1.310 via claude-agent-acp 0.95.0".into()),
                path: "/usr/local/bin/claude".into(),
            })
            .expect("serialize"),
            json!({
                "state": "ready",
                "version": "Claude Code 2.1.310 via claude-agent-acp 0.95.0",
                "path": "/usr/local/bin/claude"
            })
        );
        assert_eq!(
            serde_json::to_value(Availability::Checking).expect("serialize"),
            json!({"state": "checking"})
        );
        assert_eq!(
            serde_json::to_value(EnvState::Fallback {
                reason: "timeout".into()
            })
            .expect("serialize"),
            json!({"state": "fallback", "reason": "timeout"})
        );
        assert_eq!(
            serde_json::to_value(StorageStatus::default()).expect("serialize"),
            json!({"state": {"state": "ok"}, "unsaved_bytes": 0})
        );
        for availability in [
            Availability::Checking,
            Availability::Missing { reason: "r".into() },
            Availability::Broken { reason: "r".into() },
        ] {
            assert_eq!(round_trip(&availability), availability);
        }
        let entry = AgentEntry {
            id: "mock".into(),
            name: "Demo agent".into(),
            description: "d".into(),
            availability: Availability::Ready {
                version: None,
                path: "built in".into(),
            },
            launch: "built in".into(),
            needs_download: false,
            last_verified: "auracode".into(),
            installation: None,
            latest_release: None,
            harness: None,
            connections: None,
            terminal: false,
        };
        assert_eq!(round_trip(&entry), entry);
        let claude = AgentEntry {
            id: "claude".into(),
            name: "Claude Code".into(),
            connections: Some(AgentConnections {
                preferred: Some("claude-acp".into()),
                active: Some("claude-acp".into()),
                options: vec![AgentConnection {
                    id: "claude-acp".into(),
                    label: "ACP adapter".into(),
                    description: "d".into(),
                    availability: Availability::Checking,
                    launch: "l".into(),
                    needs_download: true,
                }],
            }),
            ..entry.clone()
        };
        assert_eq!(round_trip(&claude), claude);
        // An entry from a build before connections reads as an agent with one connection.
        let mut older = serde_json::to_value(&entry).expect("json");
        older.as_object_mut().expect("object").remove("connections");
        assert_eq!(
            serde_json::from_value::<AgentEntry>(older).expect("older entry"),
            entry
        );
        let env = EnvStatus {
            state: EnvState::Ready {
                source: "login shell".into(),
                took_ms: 120,
                vars: 80,
            },
            removed: vec!["CLAUDECODE".into()],
        };
        assert_eq!(round_trip(&env), env);
    }

    #[test]
    fn small_records_round_trip() {
        let update = ChatStatusUpdate {
            chat_id: "c".into(),
            status: SessionStatus::Cancelling,
            needs_attention: false,
            title: Some("Fix the build".into()),
            updated_at: 3,
        };
        assert_eq!(round_trip(&update), update);
        assert_eq!(
            serde_json::to_value(&update).expect("serialize")["status"],
            "cancelling"
        );
        let ack = PromptAck {
            prompt_id: "p".into(),
            queued: true,
        };
        assert_eq!(round_trip(&ack), ack);
        let blob = BlobText {
            sha256: "ab".repeat(32),
            offset: 0,
            next_offset: 262_144,
            total: 5_000_000_000,
            text: "x".into(),
            eof: false,
        };
        assert_eq!(round_trip(&blob), blob);
        assert_eq!(round_trip(&summary()), summary());
        assert_eq!(
            serde_json::to_value(FolderState::NotAllowed).expect("serialize"),
            json!("not_allowed")
        );
    }
}

#[cfg(test)]
mod native_controls_compatibility_tests {
    use super::*;
    #[test]
    fn old_lists_and_preferences_default_without_widening_native_modes() {
        let lists: SessionLists = serde_json::from_value(serde_json::json!({"revision":0,"capabilities":null,"options":[],"commands":[],"autonomy":{"requested":"ask","native_mode":null,"auto_answer":{"type":"never"},"note":null}})).unwrap();
        assert_eq!(lists.controls, SessionControls::default());
        let preferences: SessionControlPreferences = serde_json::from_str("{}").unwrap();
        assert_eq!(preferences.version, 1);
        assert_eq!(preferences.workflow, None);
        let future: SessionControlPreferences =
            serde_json::from_str(r#"{"version":7,"workflow":"future-mode"}"#).unwrap();
        assert_eq!(future.version, 7);
        assert_eq!(future.workflow.as_deref(), Some("future-mode"));
    }
}
