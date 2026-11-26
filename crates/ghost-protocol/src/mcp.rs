//! Portable shared MCP definitions. Credential values exist only in the local runtime type.
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Http { url: String },
    Sse { url: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct McpDefinition {
    /// Stable Ghost ID, shared by an all-project definition and its project override.
    pub id: String,
    pub name: String,
    /// None means all projects. A project UUID means every folder and worktree in that project.
    pub project_id: Option<String>,
    pub enabled: bool,
    pub transport: McpTransport,
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
    /// The agents that load this server, by agent ID (`claude`, `codex`). None means every
    /// agent that can take shared servers, including ones installed later. Left out of the JSON
    /// when None, so a definition for every agent reads exactly as before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<String>>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct McpRecord {
    pub definition: McpDefinition,
    pub revision: u32,
    pub missing_credentials: Vec<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct McpSnapshot {
    pub revision: u32,
    pub servers: Vec<McpRecord>,
}
/// This type never enters UI snapshots or protocol logs.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct McpCredentials {
    pub env: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
}
impl std::fmt::Debug for McpCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpCredentials")
            .field("env_keys", &self.env.keys())
            .field("header_keys", &self.headers.keys())
            .finish()
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedMcpServer {
    pub definition: McpDefinition,
    pub revision: u32,
    pub credentials: McpCredentials,
}
impl std::fmt::Debug for ResolvedMcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedMcpServer")
            .field("id", &self.definition.id)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct McpExport {
    pub version: u32,
    pub definitions: Vec<McpDefinition>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct McpCompatibility {
    pub agent_id: String,
    pub supported: bool,
    pub reason: String,
    pub restart_required: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct McpTestResult {
    pub state: String,
    pub message: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum McpSessionAction {
    Status,
    Login,
    Restart,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct McpActionResult {
    pub state: String,
    pub message: String,
    pub authorization_url: Option<String>,
    pub servers: Vec<McpRuntimeServer>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct McpRuntimeServer {
    pub name: String,
    pub state: String,
    pub authentication: String,
    pub tool_count: u32,
}
