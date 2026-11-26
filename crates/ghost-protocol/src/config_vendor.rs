//! Explicit vendor-managed integration actions. Targets are identifiers, never shell programs.
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VendorProvider {
    Claude,
    Codex,
    Pi,
    OpenCode,
    Hermes,
    Cursor,
    Grok,
    Antigravity,
}
impl VendorProvider {
    pub fn agent_id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
            Self::Hermes => "hermes",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
            Self::Antigravity => "antigravity",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VendorKind {
    Plugin,
    Marketplace,
    Package,
    Native,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VendorAction {
    Install,
    Update,
    Remove,
    Enable,
    Disable,
    Open,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct VendorRequest {
    pub provider: VendorProvider,
    pub kind: VendorKind,
    pub action: VendorAction,
    pub target: String,
    /// An optional Library item revision, checked against personal configuration only.
    pub item_revision: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct VendorReview {
    pub token: String,
    pub request: VendorRequest,
    pub program: String,
    pub arguments: Vec<String>,
    pub working_directory: String,
    pub config_directory: String,
    pub revision: String,
    pub timeout_seconds: u32,
    pub notices: Vec<String>,
}
