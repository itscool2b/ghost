//! Remote access as Settings shows it: whether this computer can be reached from a paired
//! browser or a linked bot, and who is paired and linked.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::Millis;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemoteStatus {
    /// The person turned remote access on. Nothing connects anywhere while it is off.
    pub enabled: bool,
    pub link: RemoteLink,
    /// The name paired browsers show for this computer.
    pub host_name: String,
    pub devices: Vec<RemoteDevice>,
    pub bots: Vec<RemoteBot>,
    /// A pairing link is open: its address is not repeated here, only when it stops opening.
    pub pairing_expires_at: Option<Millis>,
}

/// This computer's connection to the relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RemoteLink {
    Off,
    Connecting,
    Online,
    /// Not reachable right now; it keeps trying. `reason` says why in a sentence.
    Offline {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemoteDevice {
    pub id: String,
    pub name: String,
    pub created_at: Millis,
    pub last_seen_at: Millis,
    /// It has a session open right now.
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemoteBot {
    pub id: String,
    pub name: String,
    pub created_at: Millis,
    pub last_used_at: Option<Millis>,
    /// Made with full control: every tool, where a link without it has the chat tools only.
    pub full: bool,
    /// Tool calls the link has made, and how many were answered with an error. Counted on this
    /// computer only.
    pub calls: u32,
    pub failed: u32,
}

/// A pairing link, shown once: whoever opens it first within its time becomes a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemotePairing {
    pub url: String,
    pub expires_at: Millis,
}

/// A new bot link, shown once: the MCP address a bot connects to. It carries the bot's token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemoteBotLink {
    pub id: String,
    pub name: String,
    pub url: String,
}
