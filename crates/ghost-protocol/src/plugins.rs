//! The Plugin library: what each agent supports, read from the agent's own CLI where it has a
//! machine-readable listing and from its files otherwise. Entries carry names and public facts
//! only. Secret values (MCP env, headers, plugin options) never enter this model; only the names of
//! fields kept on the device do.
use serde::{Deserialize, Serialize};
use specta::Type;

/// One kind of extension an agent can have.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type,
)]
#[serde(rename_all = "snake_case")]
pub enum PluginFeature {
    Plugins,
    Marketplaces,
    Packages,
    Skills,
    McpServers,
    Commands,
    Subagents,
    Hooks,
}

/// Where one feature's entries came from this time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ListingState {
    /// Read from the agent's own command (`command`).
    Live,
    /// The agent's command failed. Entries, if any, come from its files instead.
    Failed { reason: String },
    /// The agent has no machine-readable listing for this feature. Entries come from its files.
    Files { reason: String },
    /// Nothing can be listed on this device, for the reason given.
    Unavailable { reason: String },
}

/// A file inventory item a row corresponds to, so the row can open its preview and copy review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PluginLibraryRef {
    pub source_id: String,
    /// The Library item kind (`skill`, `mcp_server`, `plugin`, `package`).
    pub kind: String,
    pub key: String,
    pub revision: String,
}

/// One installed, configured or available entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PluginEntry {
    /// The agent's own identifier: `name@marketplace`, a server or skill name, a package source.
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub marketplace: Option<String>,
    /// `user`, `project` or `local` when the agent reports it.
    pub scope: Option<String>,
    pub enabled: Option<bool>,
    /// False for an entry a marketplace offers that is not installed here.
    pub installed: bool,
    /// Transport or source kind, such as `stdio`, `http`, `github`, `directory` or `npm`.
    pub kind: Option<String>,
    /// Names of the fields this entry keeps on the device (MCP env and headers). Never values.
    pub local_fields: Vec<String>,
    pub library: Option<PluginLibraryRef>,
}

/// One feature of one agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FeatureListing {
    pub feature: PluginFeature,
    pub state: ListingState,
    /// The exact command that fed this listing, as a person would type it.
    pub command: Option<String>,
    pub entries: Vec<PluginEntry>,
    /// What the configured marketplaces offer that is not installed, capped.
    pub available: Vec<PluginEntry>,
    /// How many available entries the agent reported before the cap.
    pub available_total: u32,
}

/// Whether the agent has a plugin system of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PluginSystem {
    /// Plugins installed from marketplaces.
    Marketplace,
    /// Packages installed from npm or git, without a marketplace.
    Packages,
    /// Plugins declared in its configuration and loaded by the agent itself.
    Declared,
}

/// The installed harness against its latest known release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HarnessStatus {
    NotInstalled,
    /// Detection or the release check has not finished.
    Checking,
    UpToDate {
        version: String,
        checked_at: Option<crate::Millis>,
    },
    UpdateAvailable {
        version: String,
        latest: String,
        source: String,
        checked_at: Option<crate::Millis>,
    },
    /// Releases could not be checked (offline, no release source for this channel).
    Unknown {
        version: Option<String>,
        reason: String,
    },
    /// The installed binary did not pass its version check.
    Failed {
        reason: String,
    },
}

/// The installed version changed since the previous listing, so the listing was read again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct VersionChange {
    pub from: String,
    pub to: String,
}

/// One agent's section of the Plugin library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentPlugins {
    #[serde(default)]
    pub management: Option<PluginManagement>,
    /// The provider namespace (`claude`, `codex`, `pi`, `opencode`), not a launch dialect.
    pub agent_id: String,
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub system: PluginSystem,
    /// One plain sentence about what this agent supports.
    pub summary: String,
    pub features: Vec<FeatureListing>,
    pub harness: HarnessStatus,
    /// When the listing was last read. None: never on this run.
    pub updated_at: Option<crate::Millis>,
    pub version_change: Option<VersionChange>,
    pub notices: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PluginManagement {
    pub version: u32,
    pub provider: crate::config_vendor::VendorProvider,
    pub actions: Vec<PluginManagementAction>,
    pub native_routes: Vec<NativeCustomizationRoute>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PluginManagementAction {
    pub feature: PluginFeature,
    pub kind: crate::config_vendor::VendorKind,
    pub actions: Vec<crate::config_vendor::VendorAction>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct NativeCustomizationRoute {
    pub id: String,
    pub label: String,
    pub instructions: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PluginLibrary {
    pub agents: Vec<AgentPlugins>,
    pub notices: Vec<String>,
}
