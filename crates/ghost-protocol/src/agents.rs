//! Agent installations and execution hosts. A provider, host and launch path are separate identities.
use serde::{Deserialize, Serialize};
use specta::Type;

/// Where the user's agent runs. Unknown future hosts must never fall back to a local launch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentHost {
    #[default]
    Native,
    Wsl {
        distro: String,
    },
    Unsupported,
}

impl<'de> Deserialize<'de> for AgentHost {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Wire {
            Native,
            Wsl {
                distro: String,
            },
            #[serde(other)]
            Unsupported,
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Native => Self::Native,
            Wire::Wsl { distro } => Self::Wsl { distro },
            Wire::Unsupported => Self::Unsupported,
        })
    }
}

/// How the installed binary is managed. Unknown and bundled installations cannot be updated here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstallChannel {
    VendorStandalone,
    Npm {
        package: String,
    },
    Homebrew {
        formula: String,
    },
    Bundled {
        application: String,
    },
    #[default]
    Unknown,
}

impl<'de> Deserialize<'de> for InstallChannel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Wire {
            VendorStandalone,
            Npm {
                package: String,
            },
            Homebrew {
                formula: String,
            },
            Bundled {
                application: String,
            },
            #[serde(other)]
            Unknown,
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::VendorStandalone => Self::VendorStandalone,
            Wire::Npm { package } => Self::Npm { package },
            Wire::Homebrew { formula } => Self::Homebrew { formula },
            Wire::Bundled { application } => Self::Bundled { application },
            Wire::Unknown => Self::Unknown,
        })
    }
}

/// Evidence about this exact installed version, independent of whether its binary can start.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VersionVerification {
    Verified {
        version: String,
        scope: String,
    },
    Untested {
        last_verified: Option<String>,
    },
    #[default]
    Unknown,
}

/// A successful version probe of one installation. `launch_path` remains the stable user entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentInstallation {
    pub agent_id: String,
    pub provider: String,
    pub host: AgentHost,
    pub launch_path: String,
    pub resolved_path: Option<String>,
    pub version: String,
    pub channel: InstallChannel,
    pub verification: VersionVerification,
}

/// One executable in discovery order. Failed version probes remain visible and cannot be selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentInstallationCandidate {
    pub path: String,
    pub availability: crate::session::Availability,
    pub installation: Option<AgentInstallation>,
    pub selected: bool,
    pub source: String,
}

/// Device-local executable preference. Paths never become portable chat metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentInstallations {
    pub agent_id: String,
    /// None follows the environment override, then PATH order. An explicit missing path stays selected.
    pub preferred_path: Option<String>,
    pub candidates: Vec<AgentInstallationCandidate>,
    pub truncated: bool,
    pub notice: Option<String>,
}

/// The vendor's own local authentication check. This does not promise a successful paid request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentAuthStatus {
    CredentialsPresent,
    SignInRequired,
    Unknown { reason: String },
    Unsupported,
}

/// Actions this exact detected installation can perform through reviewed vendor commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentMaintenance {
    pub auth_status: bool,
    /// A reviewed interactive vendor login command can run in the explicit terminal sheet.
    pub login_available: bool,
    /// `None` enables the explicit update action. Otherwise this explains the owning channel or
    /// missing helper; the UI must not offer an update for that installation.
    pub update_unavailable: Option<String>,
}

/// Public release metadata. Stale cached data remains visible during offline use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LatestRelease {
    pub version: Option<String>,
    pub source: String,
    pub checked_at: Option<crate::Millis>,
    pub stale: bool,
}

/// A sign-in added in Ghost, beside the agent's own. Device-local: its token stays in this
/// computer's secure storage and never becomes chat metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentAccount {
    pub id: String,
    /// The person's name for it ("Work").
    pub name: String,
    pub added_at: crate::Millis,
}

/// The accounts one agent can run on here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentAccounts {
    pub agent_id: String,
    /// Oldest first.
    pub accounts: Vec<AgentAccount>,
    /// The account agents start on from now; `None`: the agent's own sign-in. A running agent
    /// keeps the account it started on until it starts again.
    pub active: Option<String>,
    /// Why accounts cannot be added for this agent here, when they cannot.
    pub unavailable: Option<String>,
}

/// The usage limits one of an agent's accounts last reported: its windows (a session, a week, a
/// model's own) and when it reported them. Agents report them while a chat runs on the account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct AccountUsage {
    /// The added account; `None`: the agent's own sign-in.
    pub account_id: Option<String>,
    pub windows: Vec<crate::LimitWindow>,
    pub observed_at: crate::Millis,
}

/// What each of an agent's accounts last reported about its usage limits, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct AgentUsage {
    pub agent_id: String,
    pub accounts: Vec<AccountUsage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_hosts_never_become_native() {
        assert_eq!(
            serde_json::from_value::<AgentHost>(json!({"type":"remote","host":"example"})).unwrap(),
            AgentHost::Unsupported
        );
        assert_eq!(
            serde_json::to_value(AgentHost::Wsl {
                distro: "Ubuntu".into()
            })
            .unwrap(),
            json!({"type":"wsl","distro":"Ubuntu"})
        );
    }
}
