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
    /// Addresses that start a task from what is sent to them, in one project with one
    /// instruction.
    pub automations: Vec<RemoteAutomation>,
    /// A pairing link is open: its address is not repeated here, only when it stops opening.
    pub pairing_expires_at: Option<Millis>,
    /// The computer does not sleep on its own while remote access is on and a paired device or a
    /// bot link can reach it. Off unless the person turns it on.
    pub keep_awake: bool,
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
    /// Reads only: nothing that sends, answers, stops or changes.
    pub read_only: bool,
    /// May approve what an agent asks permission for.
    pub approve: bool,
    /// May turn on full access in a chat.
    pub full_access: bool,
    /// When the link stops working unless it is used before then (90 days after its last use).
    pub expires_at: Millis,
    /// Tool calls the link has made, and how many were answered with an error. Counted on this
    /// computer only.
    pub calls: u32,
    pub failed: u32,
    /// Where the link's bot is told that a chat it acted on finished or asks for an answer.
    pub callback: Option<RemoteBotCallback>,
}

/// A bot link's call-back as Settings shows it. The key its address takes is never here, and
/// the address is shown without its path, which for some assistants is a secret too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemoteBotCallback {
    /// `grok`, `hermes`, `openclaw` or `webhook`.
    pub kind: String,
    /// The address's scheme and host.
    pub address: String,
    /// OpenClaw only: where its answer is delivered.
    pub channel: Option<String>,
    pub to: Option<String>,
    /// How the last message sent there went, since the app started.
    pub last: Option<RemoteTold>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemoteTold {
    pub at: Millis,
    pub ok: bool,
    /// In words, without the address or the key.
    pub detail: String,
}

/// A pairing link, shown once: whoever opens it first within its time becomes a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemotePairing {
    pub url: String,
    pub expires_at: Millis,
}

/// An automation, as Settings lists it. Its address is shown once, when it is made or renewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemoteAutomation {
    pub id: String,
    pub name: String,
    /// The project its tasks start in.
    pub project: String,
    pub instruction: String,
    pub created_at: Millis,
    pub last_used_at: Option<Millis>,
    /// When it stops working unless it is used before then (90 days idle).
    pub expires_at: Millis,
    /// What was sent to it, and how much of that was refused.
    pub calls: u32,
    pub failed: u32,
    /// Why what was last sent could not start, until one starts again.
    pub last_error: Option<String>,
}

/// A new bot link, shown once: the MCP address a bot connects to. It carries the bot's token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RemoteBotLink {
    pub id: String,
    pub name: String,
    pub url: String,
}
