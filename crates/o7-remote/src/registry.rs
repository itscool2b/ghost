//! Who may reach this desktop: its own identity at the relay, the browsers paired with it and the
//! bots linked to it. Kept in `remote.json` in the data folder, readable by the user only.
//!
//! A pairing link is the exception: it lives in memory, opens once and expires in minutes, so a
//! link left in a screenshot or a browser's history is worth nothing later.
//!
//! A device or a bot link belongs to the workspace (profile) that was open when it was made, and
//! reaches that workspace only. The file is the installation's, but nothing in it opens another
//! workspace's chats: a phone paired in one profile does not reach another profile's chats. With
//! no workspace open, nothing is accepted.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub use o7_protocol::remote::RemoteDeviceKind as DeviceKind;
use serde::{Deserialize, Serialize};

use crate::crypto::{self, KEY_BYTES};

pub const MAX_DEVICES: usize = 16;
pub const MAX_BOTS: usize = 16;
pub const MAX_NAME: usize = 48;
/// The longest app version or device model a device gives when it pairs, in characters.
pub const MAX_CLIENT_FIELD: usize = 32;
/// How long a pairing link opens.
pub const PAIRING_MS: i64 = 10 * 60 * 1000;
/// Wrong attempts a pairing link survives. Its key cannot be guessed; this only bounds the noise.
const PAIRING_ATTEMPTS: u32 = 8;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("Remote access settings could not be saved: {0}")]
    Save(String),
    #[error("Remote access settings could not be read: {0}")]
    Read(String),
    #[error("This computer already has {MAX_DEVICES} paired devices. Remove one first.")]
    TooManyDevices,
    #[error("This computer already has {MAX_BOTS} bot links. Remove one first.")]
    TooManyBots,
    #[error("The system's random source is unavailable.")]
    Random,
    #[error("Open a workspace before pairing a device or linking a bot.")]
    NoWorkspace,
    #[error("This link has {MAX_QUEUED} tasks queued for tonight already. Take one off first.")]
    TooManyQueued,
}

impl From<crypto::CryptoError> for RegistryError {
    fn from(_: crypto::CryptoError) -> Self {
        Self::Random
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    /// The device's 32-byte key, base64url.
    pub key: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    /// The workspace it was paired in, and the only one it reaches. A record without one (none
    /// was ever written by a release) reaches nothing.
    #[serde(default)]
    pub profile: String,
    /// The website the pairing link pointed at: the origin the browser keeps this key under.
    /// Empty on a record from before sites were recorded, when the site was auracode.si.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub site: String,
    /// A browser, or o7 for iPhone. Only a pairing on the app's own path, which the relay marks
    /// (`k`), is recorded as the app: what a session says about itself is only its word. Left
    /// out for a browser, so its record reads as it did before kinds were recorded.
    #[serde(default, skip_serializing_if = "DeviceKind::is_browser")]
    pub kind: DeviceKind,
    /// The app's version as o7 for iPhone gave it when it paired. None for a browser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bot {
    pub id: String,
    /// SHA-256 of the bot's token. The token itself is shown once and never stored.
    pub token_hash: String,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    /// The workspace it was made in, and the only one it reaches.
    #[serde(default)]
    pub profile: String,
    /// Full control: every tool, where a link without it has the chat tools only. Chosen when the
    /// link is made and never changed after: a wider link is a new link.
    #[serde(default)]
    pub full: bool,
    /// Reads only: the chats, the projects and what each chat did, and nothing that sends,
    /// answers, stops or changes anything. Chosen when the link is made, as `full` is.
    #[serde(default)]
    pub read_only: bool,
    /// May answer what an agent asks (a permission request or a question). Off unless its owner
    /// turned it on when making the link: the bot reads what agents write, and what it reads must
    /// not be able to approve what an agent does (23 November 2026). Without it a bot may only
    /// decline.
    #[serde(default)]
    pub approve: bool,
    /// May set a chat's access to full access (`full_auto`). Only with `full`.
    #[serde(default)]
    pub full_access: bool,
    /// The website whose address the link's token was shown in. Empty as for a device.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub site: String,
    /// How many tool calls this link has made, and how many of them were answered with an error.
    /// Kept on this computer only, for its owner to see whether a link is used and works; saved
    /// with the time of last use, so the last minute's calls may be lost with the app.
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub failed: u64,
    /// Where this link's bot is told that a chat it acted on finished or asks for an answer
    /// (`crate::callback`). The key its address takes is in the computer's secure storage,
    /// never here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback: Option<crate::callback::Callback>,
    /// The chats this link started, sent to or answered, newest last: the ones it is told
    /// about. A link is never told of a chat it did not touch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub following: Vec<String>,
    /// An automation: this link only starts tasks, in one project with one instruction, from
    /// what is sent to its address (Settings > Remote access > Automations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook: Option<Hook>,
}

/// What an automation does with what it is sent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hook {
    /// The project its tasks start in.
    pub project: String,
    /// What its agent is told to do with each thing sent: the person's own words.
    pub instruction: String,
    /// The tasks it started in the last day: its own log, which nothing else can push out, so
    /// the day's limit holds whatever else starts tasks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub starts: Vec<HookStart>,
    /// Why what was last sent could not start, for its owner in Settings; the sender is told
    /// only that it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// A task an automation started.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookStart {
    pub at: i64,
    pub chat: String,
}

/// The longest instruction an automation keeps, in characters.
pub const MAX_INSTRUCTION: usize = 4_000;
/// How many of an automation's tasks may work at once, and start in a day.
pub const HOOK_RUNNING: usize = 3;
pub const HOOK_DAILY: usize = 20;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// How many chats a link is told about at most; the oldest is forgotten first.
const MAX_FOLLOWING: usize = 40;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Stored {
    version: u32,
    enabled: bool,
    /// This desktop's id at the relay (16 bytes) and the key that claims it (32 bytes).
    host_id: String,
    host_key: String,
    devices: Vec<Device>,
    bots: Vec<Bot>,
    /// Tasks bots started (`task`), newest last: what their review needs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tasks: Vec<Task>,
    /// Hold off the computer's idle sleep while remote access is on and something can reach it
    /// (Settings > Remote access). Off unless the person turns it on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    keep_awake: bool,
    /// Tasks bots left for tonight (`task` with `when: "tonight"`), oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    queued: Vec<Queued>,
    /// The tasks started from the queue lately: when, and their chat. The night's budget and
    /// "one at a time" are read from it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    night: Vec<NightStart>,
    /// What was last seen of each task's pull request, by the task's chat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    prs: Vec<PrSeen>,
}

/// What a task's watcher last saw of its pull request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrSeen {
    /// The task's chat.
    pub chat: String,
    pub number: u64,
    pub url: String,
    pub state: String,
    /// [`crate::backend::PrChecks::word`].
    pub checks: String,
    /// How many reviews and comments it had.
    pub notes: usize,
}

/// How long after a task starts its pull request is watched.
pub const PR_WATCHED_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// A task a bot link left for tonight.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Queued {
    /// `q` and a few letters, what the bot is told and cancels it by.
    pub id: String,
    /// The bot link that queued it; set by the service.
    pub bot: String,
    pub goal: String,
    /// The project's id, found when it was queued.
    pub project: Option<String>,
    pub agent: Option<String>,
    pub review: Option<bool>,
    pub created_at: i64,
    /// It could not start, and why: kept a day so the bot can read it in `inbox`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<i64>,
}

/// A task started from the queue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NightStart {
    pub at: i64,
    pub chat: String,
}

/// The most tasks one link may leave queued.
pub const MAX_QUEUED: usize = 10;
/// How long a night lasts, for its budget: a task started this long ago was another night's.
pub const NIGHT_MS: i64 = 12 * 60 * 60 * 1000;
/// How long a queued task that could not start stays listed.
const FAILED_KEPT_MS: i64 = 24 * 60 * 60 * 1000;

/// A task a bot link started: its chat, where its work is, and its review.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    /// The bot link that started it; set by the service.
    pub bot: String,
    /// The chat doing the work.
    pub builder: String,
    pub folder_id: String,
    pub goal: String,
    pub branch: Option<String>,
    pub path: Option<String>,
    pub base: Option<String>,
    /// The agent that reviews the work when it is done; none, no review.
    pub review_agent: Option<String>,
    /// The review's chat, once it started.
    pub reviewer: Option<String>,
    pub created_at: i64,
}

/// How many tasks are kept; the oldest goes first.
const MAX_TASKS: usize = 40;

/// A pairing link's secret half, to be shown once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub id: String,
    pub key: [u8; KEY_BYTES],
    pub expires_at: i64,
    failures: u32,
}

/// A new bot link's secret, to be shown once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBot {
    pub id: String,
    pub token: String,
}

pub struct Registry {
    path: PathBuf,
    stored: Stored,
    pairing: Option<Pairing>,
    /// The open workspace's id. Empty while none is open.
    scope: String,
    /// The website new pairing links and bot addresses point at.
    site: String,
}

impl Registry {
    /// Reads the saved settings, or starts with remote access off and a new identity. A file that
    /// cannot be read is an error, never silently replaced: it holds every pairing.
    pub fn load(path: &Path) -> Result<Self, RegistryError> {
        let stored = match std::fs::read(path) {
            Ok(bytes) => {
                let stored: Stored = serde_json::from_slice(&bytes)
                    .map_err(|error| RegistryError::Read(error.to_string()))?;
                let mut stored = stored;
                let key = crypto::decode::<KEY_BYTES>(&stored.host_key);
                if stored.version != 1 || crypto::decode::<16>(&stored.host_id).is_none() {
                    return Err(RegistryError::Read("unsupported contents".into()));
                }
                let Some(key) = key else {
                    return Err(RegistryError::Read("unsupported contents".into()));
                };
                // An id from before ids were made from the key (development builds only): the
                // relay no longer takes it, so the computer gets the id its key makes. Browsers
                // paired under the old id look for it there and are paired again.
                stored.host_id = crypto::encode(&crypto::host_id(&key));
                stored
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let key = crypto::random::<KEY_BYTES>()?;
                Stored {
                    version: 1,
                    enabled: false,
                    host_id: crypto::encode(&crypto::host_id(&key)),
                    host_key: crypto::encode(&key),
                    devices: Vec::new(),
                    bots: Vec::new(),
                    tasks: Vec::new(),
                    keep_awake: false,
                    queued: Vec::new(),
                    night: Vec::new(),
                    prs: Vec::new(),
                }
            }
            Err(error) => return Err(RegistryError::Read(error.to_string())),
        };
        Ok(Self {
            path: path.to_owned(),
            stored,
            pairing: None,
            scope: String::new(),
            site: String::new(),
        })
    }

    /// Names the website this copy pairs through, and removes every browser device and bot link
    /// issued under another one, in every workspace. Returns how many went.
    ///
    /// A browser keeps a device's key under the site's origin, and a bot's token is part of an
    /// address at the site. When the site moves, whoever holds the old address later holds a
    /// page that can use those keys and receives those tokens, so none of them may stay good:
    /// the phone pairs again and the bot is linked again, at the new address. o7 for iPhone
    /// keeps its key in the iPhone's Keychain, under no web origin, so it stays; its `ready`
    /// names the new site.
    pub fn set_site(&mut self, site: &str) -> usize {
        self.site = site.to_owned();
        let before = self.stored.devices.len() + self.stored.bots.len();
        self.stored
            .devices
            .retain(|device| device.kind == DeviceKind::Ios || device.site == site);
        self.stored.bots.retain(|bot| bot.site == site);
        let gone = before - self.stored.devices.len() - self.stored.bots.len();
        if gone > 0 {
            self.pairing = None;
            // A save that fails leaves them in the file and out of this run: they are refused
            // now all the same, and removed again at the next start.
            let _ = self.save();
        }
        gone
    }

    /// Names the open workspace. Devices and bot links of any other stop being accepted, and an
    /// open pairing link closes: it was shown for the workspace being left.
    pub fn set_scope(&mut self, profile: &str) {
        if self.scope != profile {
            self.scope = profile.to_owned();
            self.pairing = None;
        }
    }

    /// The open workspace's id; empty while none is open.
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// Whether a record made in `profile` belongs to the open workspace.
    fn open(&self, profile: &str) -> bool {
        !self.scope.is_empty() && self.scope == profile
    }

    fn save(&self) -> Result<(), RegistryError> {
        let save = |error: std::io::Error| RegistryError::Save(error.to_string());
        let folder = self
            .path
            .parent()
            .ok_or_else(|| RegistryError::Save("no folder".into()))?;
        // A temporary file beside it is private to the user on Unix, and replaces the old file
        // whole: a crash leaves one or the other.
        let mut file = tempfile::NamedTempFile::new_in(folder).map_err(save)?;
        let bytes = serde_json::to_vec_pretty(&self.stored)
            .map_err(|error| RegistryError::Save(error.to_string()))?;
        file.write_all(&bytes).map_err(save)?;
        file.as_file().sync_all().map_err(save)?;
        file.persist(&self.path)
            .map_err(|error| RegistryError::Save(error.error.to_string()))?;
        Ok(())
    }

    pub fn enabled(&self) -> bool {
        self.stored.enabled
    }

    pub fn host_id(&self) -> &str {
        &self.stored.host_id
    }

    pub fn host_key(&self) -> &str {
        &self.stored.host_key
    }

    /// The open workspace's devices.
    pub fn devices(&self) -> Vec<&Device> {
        self.stored
            .devices
            .iter()
            .filter(|device| self.open(&device.profile))
            .collect()
    }

    /// The open workspace's bot links.
    pub fn bots(&self) -> Vec<&Bot> {
        self.stored
            .bots
            .iter()
            .filter(|bot| self.open(&bot.profile))
            .collect()
    }

    /// Turning remote access off also ends any open pairing link.
    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), RegistryError> {
        if !enabled {
            self.pairing = None;
        }
        if self.stored.enabled != enabled {
            self.stored.enabled = enabled;
            self.save()?;
        }
        Ok(())
    }

    /// Queues `item` for tonight. A link may leave at most [`MAX_QUEUED`].
    pub fn queue(&mut self, item: Queued) -> Result<(), RegistryError> {
        let waiting = self
            .stored
            .queued
            .iter()
            .filter(|queued| queued.bot == item.bot && queued.failed.is_none())
            .count();
        if waiting >= MAX_QUEUED {
            return Err(RegistryError::TooManyQueued);
        }
        self.stored.queued.push(item);
        self.save()
    }

    /// Takes `bot`'s queued `id` off the queue; false when it has none such.
    pub fn cancel_queued(&mut self, bot: &str, id: &str) -> bool {
        let before = self.stored.queued.len();
        self.stored
            .queued
            .retain(|queued| !(queued.bot == bot && queued.id == id));
        let gone = self.stored.queued.len() < before;
        if gone {
            let _ = self.save();
        }
        gone
    }

    /// What `bot` left for tonight, oldest first, with what could not start in the last day.
    pub fn queued_of(&self, bot: &str, now: i64) -> Vec<Queued> {
        self.stored
            .queued
            .iter()
            .filter(|queued| queued.bot == bot)
            .filter(|queued| {
                queued
                    .failed_at
                    .is_none_or(|at| now.saturating_sub(at) <= FAILED_KEPT_MS)
            })
            .cloned()
            .collect()
    }

    /// The next queued task that may start: the oldest whose link belongs to the open workspace
    /// and still works. A link that is gone takes its queue with it, and what failed a day ago
    /// is forgotten.
    pub fn next_queued(&mut self, now: i64) -> Option<Queued> {
        let before = self.stored.queued.len();
        let links: Vec<String> = self.stored.bots.iter().map(|bot| bot.id.clone()).collect();
        self.stored.queued.retain(|queued| {
            links.contains(&queued.bot)
                && queued
                    .failed_at
                    .is_none_or(|at| now.saturating_sub(at) <= FAILED_KEPT_MS)
        });
        if self.stored.queued.len() < before {
            let _ = self.save();
        }
        let open: Vec<&Bot> = self
            .bots()
            .into_iter()
            .filter(|bot| !expired(bot, now))
            .collect();
        self.stored
            .queued
            .iter()
            .find(|queued| queued.failed.is_none() && open.iter().any(|bot| bot.id == queued.bot))
            .cloned()
    }

    /// `id` started as `chat`: off the queue, and counted against the night.
    pub fn started_queued(&mut self, id: &str, chat: &str, now: i64) {
        self.stored.queued.retain(|queued| queued.id != id);
        self.stored
            .night
            .retain(|start| now.saturating_sub(start.at) <= NIGHT_MS);
        self.stored.night.push(NightStart {
            at: now,
            chat: chat.to_owned(),
        });
        let _ = self.save();
    }

    /// `id` could not start: it stays listed a day, with why, and is not tried again.
    pub fn failed_queued(&mut self, id: &str, why: &str, now: i64) {
        for queued in &mut self.stored.queued {
            if queued.id == id {
                queued.failed = Some(why.to_owned());
                queued.failed_at = Some(now);
            }
        }
        let _ = self.save();
    }

    /// How many tasks started from the queue this night.
    pub fn started_tonight(&self, now: i64) -> usize {
        self.stored
            .night
            .iter()
            .filter(|start| now.saturating_sub(start.at) <= NIGHT_MS)
            .count()
    }

    /// The chats of this night's tasks and of their reviews: while one works, the next waits.
    pub fn night_chats(&self, now: i64) -> Vec<String> {
        let mut chats = Vec::new();
        for start in &self.stored.night {
            if now.saturating_sub(start.at) > NIGHT_MS {
                continue;
            }
            chats.push(start.chat.clone());
            if let Some(reviewer) = self
                .stored
                .tasks
                .iter()
                .find(|task| task.builder == start.chat)
                .and_then(|task| task.reviewer.clone())
            {
                chats.push(reviewer);
            }
        }
        chats
    }

    /// The tasks whose pull request is watched: the open workspace's, younger than a week,
    /// with a branch, started by a link that has a call-back, and not merged or closed. Each
    /// with what was last seen of it.
    pub fn watched_tasks(&self, now: i64) -> Vec<(Task, Option<PrSeen>)> {
        let links: Vec<&Bot> = self
            .bots()
            .into_iter()
            .filter(|bot| bot.callback.is_some() && !expired(bot, now))
            .collect();
        self.stored
            .tasks
            .iter()
            .filter(|task| task.branch.is_some())
            .filter(|task| now.saturating_sub(task.created_at) <= PR_WATCHED_MS)
            .filter(|task| links.iter().any(|bot| bot.id == task.bot))
            .map(|task| {
                let seen = self
                    .stored
                    .prs
                    .iter()
                    .find(|seen| seen.chat == task.builder)
                    .cloned();
                (task.clone(), seen)
            })
            .filter(|(_, seen)| {
                seen.as_ref()
                    .is_none_or(|seen| !matches!(seen.state.as_str(), "merged" | "closed"))
            })
            .collect()
    }

    /// Keeps what was seen of a task's pull request.
    pub fn saw_pr(&mut self, seen: PrSeen) {
        self.stored.prs.retain(|kept| kept.chat != seen.chat);
        self.stored.prs.push(seen);
        if self.stored.prs.len() > MAX_TASKS {
            let extra = self.stored.prs.len() - MAX_TASKS;
            self.stored.prs.drain(..extra);
        }
        let _ = self.save();
    }

    /// A new automation: a link that only starts tasks, in `hook`'s project with its
    /// instruction, from what is sent to its address.
    pub fn add_hook(&mut self, name: &str, hook: Hook, now: i64) -> Result<NewBot, RegistryError> {
        let hook = Hook {
            project: hook.project,
            instruction: hook
                .instruction
                .trim()
                .chars()
                .take(MAX_INSTRUCTION)
                .collect(),
            starts: Vec::new(),
            last_error: None,
        };
        let new = self.add_bot(&name_of(name, "Automation"), crate::bot::Rights::CHATS, now)?;
        if let Some(bot) = self.stored.bots.iter_mut().find(|bot| bot.id == new.id) {
            bot.hook = Some(hook);
        }
        if let Err(error) = self.save() {
            self.stored.bots.retain(|bot| bot.id != new.id);
            return Err(error);
        }
        Ok(new)
    }

    /// The open workspace's automation `id`, when it is one.
    pub fn hook_of(&self, id: &str) -> Option<Hook> {
        self.bots()
            .into_iter()
            .find(|bot| bot.id == id)
            .and_then(|bot| bot.hook.clone())
    }

    /// The chats of the tasks automation `id` started in the last day, from its own log: how
    /// many started, and which may still work.
    pub fn hook_chats(&self, id: &str, now: i64) -> Vec<String> {
        self.stored
            .bots
            .iter()
            .find(|bot| bot.id == id)
            .and_then(|bot| bot.hook.as_ref())
            .map(|hook| {
                hook.starts
                    .iter()
                    .filter(|start| now.saturating_sub(start.at) <= DAY_MS)
                    .map(|start| start.chat.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Automation `id` started a task in `chat`: logged against its day, and its last error
    /// cleared.
    pub fn hook_started(&mut self, id: &str, chat: &str, now: i64) {
        if let Some(hook) = self
            .stored
            .bots
            .iter_mut()
            .find(|bot| bot.id == id)
            .and_then(|bot| bot.hook.as_mut())
        {
            hook.starts
                .retain(|start| now.saturating_sub(start.at) <= DAY_MS);
            hook.starts.push(HookStart {
                at: now,
                chat: chat.to_owned(),
            });
            hook.last_error = None;
        }
        let _ = self.save();
    }

    /// What was sent to automation `id` could not start, and why.
    pub fn hook_failed(&mut self, id: &str, why: &str) {
        if let Some(hook) = self
            .stored
            .bots
            .iter_mut()
            .find(|bot| bot.id == id)
            .and_then(|bot| bot.hook.as_mut())
        {
            hook.last_error = Some(why.chars().take(300).collect());
        }
        let _ = self.save();
    }

    /// A link's name and call-back, when it has one.
    pub fn callback_of(&self, bot: &str) -> Option<(String, crate::callback::Callback)> {
        self.stored
            .bots
            .iter()
            .find(|link| link.id == bot)
            .and_then(|link| Some((link.name.clone(), link.callback.clone()?)))
    }

    /// Whether the person asked to keep the computer awake for remote access.
    pub fn keep_awake(&self) -> bool {
        self.stored.keep_awake
    }

    pub fn set_keep_awake(&mut self, on: bool) -> Result<(), RegistryError> {
        if self.stored.keep_awake != on {
            self.stored.keep_awake = on;
            self.save()?;
        }
        Ok(())
    }

    /// Whether to hold off idle sleep now: the person asked for it, remote access is on, and a
    /// paired device or a link that has not expired can reach the open workspace. With nothing
    /// that could reach it, staying awake would only use power.
    pub fn wants_awake(&self, now: i64) -> bool {
        self.stored.enabled
            && self.stored.keep_awake
            && (!self.devices().is_empty() || self.bots().iter().any(|bot| !expired(bot, now)))
    }

    /// A new pairing link. It replaces the one before it: only the newest link opens.
    pub fn start_pairing(&mut self, now: i64) -> Result<Pairing, RegistryError> {
        if self.scope.is_empty() {
            return Err(RegistryError::NoWorkspace);
        }
        let pairing = Pairing {
            id: crypto::encode(&crypto::random::<8>()?),
            key: crypto::random()?,
            expires_at: now + PAIRING_MS,
            failures: 0,
        };
        self.pairing = Some(pairing.clone());
        Ok(pairing)
    }

    pub fn cancel_pairing(&mut self) {
        self.pairing = None;
    }

    /// The open pairing link, while it is still good.
    pub fn pairing(&mut self, now: i64) -> Option<&Pairing> {
        if self
            .pairing
            .as_ref()
            .is_some_and(|pairing| pairing.expires_at <= now)
        {
            self.pairing = None;
        }
        self.pairing.as_ref()
    }

    /// The key of the pairing link `id`, if that link is the open one.
    pub fn pairing_key(&mut self, id: &str, now: i64) -> Option<[u8; KEY_BYTES]> {
        self.pairing(now)
            .filter(|pairing| crypto::same(&pairing.id, id))
            .map(|pairing| pairing.key)
    }

    /// A session under the pairing link `id` sent something its key did not seal.
    pub fn pairing_failed(&mut self, id: &str) {
        if let Some(pairing) = self.pairing.as_mut().filter(|pairing| pairing.id == id) {
            pairing.failures += 1;
            if pairing.failures >= PAIRING_ATTEMPTS {
                self.pairing = None;
            }
        }
    }

    /// Spends the pairing link `id` on a new browser device. The link never opens again.
    pub fn pair(
        &mut self,
        id: &str,
        name: &str,
        now: i64,
    ) -> Result<Option<Device>, RegistryError> {
        self.pair_as(id, name, DeviceKind::Browser, None, now)
    }

    /// Spends the pairing link `id` on a new device of `kind`: a browser, or o7 for iPhone with
    /// the version it gave (cut to [`MAX_CLIENT_FIELD`]). The link never opens again. A browser
    /// keeps no version.
    pub fn pair_as(
        &mut self,
        id: &str,
        name: &str,
        kind: DeviceKind,
        app_version: Option<&str>,
        now: i64,
    ) -> Result<Option<Device>, RegistryError> {
        if self.pairing_key(id, now).is_none() {
            return Ok(None);
        }
        if self.devices().len() >= MAX_DEVICES {
            return Err(RegistryError::TooManyDevices);
        }
        let (fallback, app_version) = match kind {
            DeviceKind::Browser => ("Browser", None),
            DeviceKind::Ios => (
                "iPhone",
                app_version
                    .map(|version| one_line(version, MAX_CLIENT_FIELD))
                    .filter(|version| !version.is_empty()),
            ),
        };
        let device = Device {
            id: crypto::encode(&crypto::random::<8>()?),
            key: crypto::encode(&crypto::random::<KEY_BYTES>()?),
            name: name_of(name, fallback),
            created_at: now,
            last_seen_at: now,
            profile: self.scope.clone(),
            site: self.site.clone(),
            kind,
            app_version,
        };
        self.stored.devices.push(device.clone());
        if let Err(error) = self.save() {
            self.stored.devices.pop();
            return Err(error);
        }
        self.pairing = None;
        Ok(Some(device))
    }

    /// The key of the device `id`, if it was paired in the open workspace.
    pub fn device_key(&self, id: &str) -> Option<[u8; KEY_BYTES]> {
        self.stored
            .devices
            .iter()
            .find(|device| crypto::same(&device.id, id) && self.open(&device.profile))
            .and_then(|device| crypto::decode(&device.key))
    }

    /// What the device `id` is, if it was paired in the open workspace.
    pub fn device_kind(&self, id: &str) -> Option<DeviceKind> {
        self.stored
            .devices
            .iter()
            .find(|device| crypto::same(&device.id, id) && self.open(&device.profile))
            .map(|device| device.kind)
    }

    /// Whether the device `id` is paired, but in a workspace that is not the open one.
    pub fn device_elsewhere(&self, id: &str) -> bool {
        self.stored
            .devices
            .iter()
            .any(|device| crypto::same(&device.id, id) && !self.open(&device.profile))
    }

    /// Notes that a device connected. A failed save only loses the time.
    pub fn seen(&mut self, id: &str, now: i64) {
        let scope = self.scope.clone();
        if let Some(device) = self
            .stored
            .devices
            .iter_mut()
            .find(|d| d.id == id && !scope.is_empty() && d.profile == scope)
        {
            // Reconnects are frequent; a minute's precision is enough to be worth a write.
            if now - device.last_seen_at >= 60_000 {
                device.last_seen_at = now;
                let _ = self.save();
            }
        }
    }

    pub fn remove_device(&mut self, id: &str) -> Result<bool, RegistryError> {
        let before = self.stored.devices.clone();
        let scope = self.scope.clone();
        self.stored
            .devices
            .retain(|device| !(device.id == id && !scope.is_empty() && device.profile == scope));
        if self.stored.devices.len() == before.len() {
            return Ok(false);
        }
        if let Err(error) = self.save() {
            self.stored.devices = before;
            return Err(error);
        }
        Ok(true)
    }

    pub fn add_bot(
        &mut self,
        name: &str,
        rights: crate::bot::Rights,
        now: i64,
    ) -> Result<NewBot, RegistryError> {
        let rights = rights.coherent();
        if self.scope.is_empty() {
            return Err(RegistryError::NoWorkspace);
        }
        if self.bots().len() >= MAX_BOTS {
            return Err(RegistryError::TooManyBots);
        }
        let new = NewBot {
            id: crypto::encode(&crypto::random::<8>()?),
            token: crypto::encode(&crypto::random::<KEY_BYTES>()?),
        };
        self.stored.bots.push(Bot {
            id: new.id.clone(),
            token_hash: crypto::hash(&new.token),
            name: name_of(name, "Bot"),
            created_at: now,
            last_used_at: None,
            profile: self.scope.clone(),
            full: rights.full,
            read_only: rights.read_only,
            approve: rights.approve,
            full_access: rights.full_access,
            site: self.site.clone(),
            calls: 0,
            failed: 0,
            callback: None,
            following: Vec::new(),
            hook: None,
        });
        if let Err(error) = self.save() {
            self.stored.bots.pop();
            return Err(error);
        }
        Ok(new)
    }

    /// Whether the open workspace's bot link `id` has gone unused too long to be accepted.
    pub fn bot_expired(&self, id: &str, now: i64) -> bool {
        self.bots()
            .iter()
            .any(|bot| crypto::same(&bot.id, id) && expired(bot, now))
    }

    /// A new address for the open workspace's bot link `id`: a new token, the old one refused
    /// from now on. Everything else about the link stays, and an expired link is in use again.
    pub fn renew_bot(&mut self, id: &str, now: i64) -> Result<Option<NewBot>, RegistryError> {
        let scope = self.scope.clone();
        let Some(index) = self
            .stored
            .bots
            .iter()
            .position(|bot| bot.id == id && !scope.is_empty() && bot.profile == scope)
        else {
            return Ok(None);
        };
        let token = crypto::encode(&crypto::random::<KEY_BYTES>()?);
        let before = self.stored.bots[index].clone();
        let bot = &mut self.stored.bots[index];
        bot.token_hash = crypto::hash(&token);
        bot.created_at = now;
        bot.last_used_at = None;
        bot.site = self.site.clone();
        if let Err(error) = self.save() {
            self.stored.bots[index] = before;
            return Err(error);
        }
        Ok(Some(NewBot {
            id: id.to_owned(),
            token,
        }))
    }

    /// Whether `token` is the bot `id`'s, and that bot was linked in the open workspace and has
    /// not expired. Notes the use.
    /// What link `id` may do, when `token` is its token and it still works in the open
    /// workspace; none otherwise. Nothing is counted: a listing, not a call.
    pub fn check_bot(&self, id: &str, token: &str, now: i64) -> Option<crate::bot::Rights> {
        let hash = crypto::hash(token);
        self.bots()
            .into_iter()
            .find(|bot| crypto::same(&bot.id, id))
            .filter(|bot| crypto::same(&bot.token_hash, &hash) && !expired(bot, now))
            .map(crate::bot::Rights::of)
    }

    pub fn verify_bot(&mut self, id: &str, token: &str, now: i64) -> bool {
        let hash = crypto::hash(token);
        let scope = self.scope.clone();
        let Some(bot) = self
            .stored
            .bots
            .iter_mut()
            .find(|bot| crypto::same(&bot.id, id) && !scope.is_empty() && bot.profile == scope)
        else {
            return false;
        };
        if !crypto::same(&bot.token_hash, &hash) || expired(bot, now) {
            return false;
        }
        bot.calls = bot.calls.saturating_add(1);
        if bot.last_used_at.is_none_or(|last| now - last >= 60_000) {
            bot.last_used_at = Some(now);
            let _ = self.save();
        }
        true
    }

    /// Sets or clears where the open workspace's bot link `id` is told. `false`: no such link.
    pub fn set_bot_callback(
        &mut self,
        id: &str,
        callback: Option<crate::callback::Callback>,
    ) -> Result<bool, RegistryError> {
        let scope = self.scope.clone();
        let Some(index) = self
            .stored
            .bots
            .iter()
            .position(|bot| bot.id == id && !scope.is_empty() && bot.profile == scope)
        else {
            return Ok(false);
        };
        let before = self.stored.bots[index].callback.clone();
        self.stored.bots[index].callback = callback;
        if let Err(error) = self.save() {
            self.stored.bots[index].callback = before;
            return Err(error);
        }
        Ok(true)
    }

    /// Notes that the open workspace's bot link `id` acted on `chat`: it is told about that
    /// chat from now on. Saved, so a restart in the middle of a task does not lose it.
    pub fn follow(&mut self, id: &str, chat: &str) {
        let scope = self.scope.clone();
        let Some(bot) = self
            .stored
            .bots
            .iter_mut()
            .find(|bot| bot.id == id && !scope.is_empty() && bot.profile == scope)
        else {
            return;
        };
        if bot.following.last().is_some_and(|last| last == chat) {
            return;
        }
        bot.following.retain(|followed| followed != chat);
        bot.following.push(chat.to_owned());
        let extra = bot.following.len().saturating_sub(MAX_FOLLOWING);
        bot.following.drain(..extra);
        let _ = self.save();
    }

    /// The open workspace's bot links that are told about `chat`: each one's id, name, where,
    /// and what it may do (the message says whether its bot may approve what is asked).
    pub fn told_of(
        &self,
        chat: &str,
    ) -> Vec<(
        String,
        String,
        crate::callback::Callback,
        crate::bot::Rights,
    )> {
        self.bots()
            .into_iter()
            .filter(|bot| bot.following.iter().any(|followed| followed == chat))
            .filter_map(|bot| {
                bot.callback.clone().map(|callback| {
                    (
                        bot.id.clone(),
                        bot.name.clone(),
                        callback,
                        crate::bot::Rights::of(bot),
                    )
                })
            })
            .collect()
    }

    /// Whether any link of the open workspace is told anything.
    pub fn any_callback(&self) -> bool {
        self.bots().iter().any(|bot| bot.callback.is_some())
    }

    /// Keeps a task a bot link of the open workspace started.
    pub fn add_task(&mut self, task: Task) {
        if !self.bots().iter().any(|bot| bot.id == task.bot) {
            return;
        }
        self.stored
            .tasks
            .retain(|kept| kept.builder != task.builder);
        self.stored.tasks.push(task);
        let extra = self.stored.tasks.len().saturating_sub(MAX_TASKS);
        self.stored.tasks.drain(..extra);
        let _ = self.save();
    }

    /// The task `chat` works on or reviews, of a bot link of the open workspace.
    pub fn task_of(&self, chat: &str) -> Option<Task> {
        let bots: Vec<&str> = self.bots().iter().map(|bot| bot.id.as_str()).collect();
        self.stored
            .tasks
            .iter()
            .rev()
            .find(|task| {
                bots.contains(&task.bot.as_str())
                    && (task.builder == chat || task.reviewer.as_deref() == Some(chat))
            })
            .cloned()
    }

    /// Notes the review chat of the task `builder` works on.
    pub fn set_reviewer(&mut self, builder: &str, reviewer: &str) {
        if let Some(task) = self
            .stored
            .tasks
            .iter_mut()
            .rev()
            .find(|task| task.builder == builder)
        {
            task.reviewer = Some(reviewer.to_owned());
            let _ = self.save();
        }
    }

    /// Whether a task of the open workspace still waits for its review to start.
    pub fn reviews_pending(&self) -> bool {
        let bots: Vec<&str> = self.bots().iter().map(|bot| bot.id.as_str()).collect();
        self.stored.tasks.iter().any(|task| {
            bots.contains(&task.bot.as_str())
                && task.review_agent.is_some()
                && task.reviewer.is_none()
        })
    }

    /// Notes that a call of the open workspace's bot link `id` was answered with an error.
    pub fn bot_failed(&mut self, id: &str) {
        let scope = self.scope.clone();
        if let Some(bot) = self
            .stored
            .bots
            .iter_mut()
            .find(|bot| crypto::same(&bot.id, id) && !scope.is_empty() && bot.profile == scope)
        {
            bot.failed = bot.failed.saturating_add(1);
        }
    }

    /// Whether the open workspace's bot link `id` was made with full control.
    pub fn bot_full(&self, id: &str) -> bool {
        self.bot_rights(id).full
    }

    /// What the open workspace's bot link `id` may do; nothing for a link it does not have.
    pub fn bot_rights(&self, id: &str) -> crate::bot::Rights {
        self.bots()
            .iter()
            .find(|bot| crypto::same(&bot.id, id))
            .map_or(
                crate::bot::Rights {
                    read_only: true,
                    ..Default::default()
                },
                |bot| crate::bot::Rights::of(bot),
            )
    }

    pub fn remove_bot(&mut self, id: &str) -> Result<bool, RegistryError> {
        let before = self.stored.bots.clone();
        let scope = self.scope.clone();
        self.stored
            .bots
            .retain(|bot| !(bot.id == id && !scope.is_empty() && bot.profile == scope));
        if self.stored.bots.len() == before.len() {
            return Ok(false);
        }
        if let Err(error) = self.save() {
            self.stored.bots = before;
            return Err(error);
        }
        Ok(true)
    }
}

/// How long a bot link may go unused before it is refused: its address lives in another
/// service, so one nobody uses any more should not work for ever. Renewing it gives a new one.
pub const BOT_IDLE_EXPIRY_MS: i64 = 90 * 24 * 60 * 60 * 1000;

/// Whether `bot` has gone unused (or, never used, unrenewed) for longer than
/// [`BOT_IDLE_EXPIRY_MS`].
pub fn expired(bot: &Bot, now: i64) -> bool {
    now.saturating_sub(bot.last_used_at.unwrap_or(bot.created_at)) > BOT_IDLE_EXPIRY_MS
}

/// A name a person or a browser supplied, as one short line; `fallback` when nothing is left.
fn name_of(name: &str, fallback: &str) -> String {
    let name = one_line(name, MAX_NAME);
    if name.is_empty() {
        fallback.to_owned()
    } else {
        name
    }
}

/// What a device supplied, as one line of at most `max` characters: control characters become
/// spaces and runs of space become one.
pub fn one_line(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(id: &str, bot: &str) -> Queued {
        Queued {
            id: id.to_owned(),
            bot: bot.to_owned(),
            goal: format!("Do {id}"),
            project: None,
            agent: None,
            review: None,
            created_at: 1,
            failed: None,
            failed_at: None,
        }
    }

    /// An automation is a link that remembers its project and instruction, cut to length, and
    /// counts the tasks it started in the last day.
    #[test]
    fn an_automation_keeps_its_project_and_instruction() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("remote.json");
        let mut registry = Registry::load(&path).expect("load");
        registry.set_scope("local");
        let hook = registry
            .add_hook(
                "",
                Hook {
                    project: "p1".into(),
                    instruction: format!("  Fix what CI reports.{}", "x".repeat(MAX_INSTRUCTION)),
                    starts: Vec::new(),
                    last_error: None,
                },
                1,
            )
            .expect("hook");
        let link = registry
            .add_bot("Grok", crate::bot::Rights::CHATS, 1)
            .expect("bot");
        let again = Registry::load(&path).expect("load");
        let mut again = again;
        again.set_scope("local");
        let kept = again.hook_of(&hook.id).expect("kept");
        assert_eq!(kept.project, "p1");
        assert!(kept.instruction.starts_with("Fix what CI reports."));
        assert_eq!(kept.instruction.chars().count(), MAX_INSTRUCTION);
        assert_eq!(
            again
                .bots()
                .iter()
                .find(|bot| bot.id == hook.id)
                .expect("named")
                .name,
            "Automation"
        );
        assert!(
            again.hook_of(&link.id).is_none(),
            "a bot link is no automation"
        );
        again.set_scope("other");
        assert!(again.hook_of(&hook.id).is_none(), "another workspace's");
        registry.hook_started(&hook.id, "c5", 10);
        assert_eq!(registry.hook_chats(&hook.id, 11), ["c5"]);
        assert!(
            registry.hook_chats(&hook.id, 10 + DAY_MS + 1).is_empty(),
            "a day later"
        );
        // Its log is its own: tasks pushed out of the shared list do not empty it.
        for n in 0..MAX_TASKS + 5 {
            registry.add_task(Task {
                bot: link.id.clone(),
                builder: format!("x{n}"),
                folder_id: "f1".into(),
                goal: "Other".into(),
                branch: None,
                path: None,
                base: None,
                review_agent: None,
                reviewer: None,
                created_at: 11,
            });
        }
        assert_eq!(registry.hook_chats(&hook.id, 12), ["c5"]);
        registry.hook_failed(&hook.id, "There is no project \"p1\".");
        let failed = Registry::load(&path).expect("load");
        let mut failed = failed;
        failed.set_scope("local");
        assert_eq!(
            failed
                .bots()
                .iter()
                .find(|bot| bot.id == hook.id)
                .and_then(|bot| bot.hook.as_ref())
                .and_then(|hook| hook.last_error.clone())
                .as_deref(),
            Some("There is no project \"p1\".")
        );
    }

    /// A link's queue: at most ten each, cancelled only by its own link, taken oldest first and
    /// only for a link of the open workspace, counted against the night once started, kept a
    /// day with why when it could not start, and gone with its link.
    #[test]
    fn work_left_for_tonight_is_queued_started_and_counted() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("remote.json");
        let mut registry = Registry::load(&path).expect("load");
        registry.set_scope("local");
        let grok = registry
            .add_bot("Grok", crate::bot::Rights::CHATS, 1)
            .expect("bot")
            .id;
        let muse = registry
            .add_bot("Muse", crate::bot::Rights::CHATS, 1)
            .expect("bot")
            .id;
        for n in 0..MAX_QUEUED {
            registry
                .queue(queued(&format!("q{n}"), &grok))
                .expect("queued");
        }
        assert!(matches!(
            registry.queue(queued("q99", &grok)),
            Err(RegistryError::TooManyQueued)
        ));
        registry.queue(queued("m1", &muse)).expect("another link's");
        assert!(!registry.cancel_queued(&muse, "q0"), "not its own");
        assert!(registry.cancel_queued(&grok, "q0"));
        assert_eq!(registry.queued_of(&grok, 2).len(), MAX_QUEUED - 1);
        assert_eq!(registry.next_queued(2).expect("next").id, "q1");

        registry.started_queued("q1", "c7", 10);
        assert_eq!(registry.started_tonight(11), 1);
        assert_eq!(
            registry.started_tonight(10 + NIGHT_MS + 1),
            0,
            "another night"
        );
        assert_eq!(registry.night_chats(11), ["c7"]);
        registry.failed_queued("q2", "No agent \"codex\" is ready here.", 12);
        assert_eq!(
            registry.next_queued(13).expect("next").id,
            "q3",
            "a failed one is not tried again"
        );
        let failed = registry
            .queued_of(&grok, 13)
            .into_iter()
            .find(|queued| queued.id == "q2")
            .expect("listed");
        assert_eq!(
            failed.failed.as_deref(),
            Some("No agent \"codex\" is ready here.")
        );
        assert!(
            registry
                .queued_of(&grok, 12 + FAILED_KEPT_MS + 1)
                .iter()
                .all(|queued| queued.id != "q2"),
            "forgotten after a day"
        );

        let mut again = Registry::load(&path).expect("load");
        again.set_scope("other");
        assert!(again.next_queued(13).is_none(), "another workspace's links");
        again.set_scope("local");
        assert_eq!(again.next_queued(13).expect("kept").id, "q3");
        assert!(again.remove_bot(&grok).expect("removed"));
        assert_eq!(
            again.next_queued(14).expect("muse's").id,
            "m1",
            "gone with its link"
        );
    }

    /// Kept awake only when asked, with remote access on, and while a link that has not expired
    /// (or a paired device) can reach the open workspace. The choice is kept across restarts, and
    /// a file from before it reads as off.
    #[test]
    fn the_computer_is_kept_awake_only_while_something_can_reach_it() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("remote.json");
        let mut registry = Registry::load(&path).expect("load");
        registry.set_scope("local");
        registry.set_enabled(true).expect("on");
        assert!(!registry.keep_awake(), "off unless asked");
        let bot = registry
            .add_bot("Grok", crate::bot::Rights::CHATS, 1)
            .expect("bot");
        assert!(!registry.wants_awake(2), "not asked");
        let saved = std::fs::read_to_string(&path).expect("saved");
        assert!(!saved.contains("keep_awake"), "nothing written while off");
        registry.set_keep_awake(true).expect("keep awake");
        assert!(registry.wants_awake(2));
        assert!(
            !registry.wants_awake(1 + BOT_IDLE_EXPIRY_MS + 1),
            "the only link expired"
        );
        let mut again = Registry::load(&path).expect("load");
        again.set_scope("local");
        assert!(again.keep_awake() && again.wants_awake(2), "kept");
        again.set_scope("other");
        assert!(
            !again.wants_awake(2),
            "another workspace's link reaches nothing here"
        );
        registry.set_enabled(false).expect("off");
        assert!(!registry.wants_awake(2), "remote access is off");
        registry.set_enabled(true).expect("on");
        assert!(registry.remove_bot(&bot.id).expect("remove"));
        assert!(!registry.wants_awake(2), "nothing can reach it");
        registry.set_keep_awake(false).expect("off");
        assert!(!Registry::load(&path).expect("load").keep_awake());
    }

    /// What a link may do is chosen when it is made, and the parts that only mean something
    /// together are made to fit: a read-only link approves nothing and has no full control.
    /// A task is kept for its review, found by either of its chats, and only for a link of the
    /// open workspace.
    #[test]
    fn a_task_is_kept_for_its_review() {
        let folder = tempfile::tempdir().expect("folder");
        let mut registry = Registry::load(&folder.path().join("remote.json")).expect("load");
        registry.set_scope("local");
        let bot = registry
            .add_bot("Grok", crate::bot::Rights::CHATS, 1)
            .expect("bot");
        let task = |bot: &str, builder: &str| Task {
            bot: bot.to_owned(),
            builder: builder.to_owned(),
            folder_id: "f1".into(),
            goal: "Fix it".into(),
            branch: Some("aura/fix-it".into()),
            path: Some("/work/wt".into()),
            base: Some("abc".into()),
            review_agent: Some("codex".into()),
            reviewer: None,
            created_at: 1,
        };
        registry.add_task(task("nobody", "c0"));
        assert!(
            registry.task_of("c0").is_none(),
            "not a link of this workspace"
        );
        assert!(!registry.reviews_pending());
        registry.add_task(task(&bot.id, "c1"));
        assert!(registry.reviews_pending());
        assert_eq!(registry.task_of("c1").expect("task").builder, "c1");
        registry.set_reviewer("c1", "c2");
        assert!(!registry.reviews_pending());
        assert_eq!(registry.task_of("c2").expect("task").builder, "c1");
        let mut again = Registry::load(&folder.path().join("remote.json")).expect("load");
        again.set_scope("local");
        assert_eq!(
            again.task_of("c2").expect("task").reviewer.as_deref(),
            Some("c2")
        );
        again.set_scope("other");
        assert!(again.task_of("c1").is_none(), "another workspace's");
        for n in 0..MAX_TASKS + 3 {
            registry.add_task(task(&bot.id, &format!("x{n}")));
        }
        assert!(registry.task_of("c1").is_none(), "the oldest go");
    }

    #[test]
    fn a_links_rights_are_kept_and_made_coherent() {
        let folder = tempfile::tempdir().expect("folder");
        let mut registry = Registry::load(&folder.path().join("remote.json")).expect("load");
        registry.set_scope("local");
        let reader = registry
            .add_bot(
                "Reader",
                crate::bot::Rights {
                    read_only: true,
                    full: true,
                    approve: true,
                    full_access: true,
                },
                1,
            )
            .expect("bot");
        let chats = registry
            .add_bot(
                "Chats",
                crate::bot::Rights {
                    approve: true,
                    full_access: true,
                    ..crate::bot::Rights::CHATS
                },
                1,
            )
            .expect("bot");
        let full = registry
            .add_bot(
                "Full",
                crate::bot::Rights {
                    full_access: true,
                    ..crate::bot::Rights::FULL
                },
                1,
            )
            .expect("bot");
        assert_eq!(
            registry.bot_rights(&reader.id),
            crate::bot::Rights {
                read_only: true,
                ..Default::default()
            }
        );
        assert_eq!(
            registry.bot_rights(&chats.id),
            crate::bot::Rights {
                approve: true,
                ..crate::bot::Rights::CHATS
            },
            "full access needs full control"
        );
        assert_eq!(
            registry.bot_rights(&full.id),
            crate::bot::Rights {
                full_access: true,
                ..crate::bot::Rights::FULL
            }
        );
        assert!(
            registry.bot_rights("nobody").read_only,
            "an unknown link reads nothing more"
        );
        // Saved and read back the same.
        let mut again = Registry::load(&folder.path().join("remote.json")).expect("load");
        again.set_scope("local");
        assert_eq!(again.bot_rights(&chats.id), registry.bot_rights(&chats.id));
    }

    /// A link unused for ninety days is refused; renewing it gives a new address, refuses the
    /// old one at once, and keeps everything else.
    #[test]
    fn an_unused_link_expires_and_a_renewed_one_takes_a_new_address() {
        let day = 24 * 60 * 60 * 1000;
        let folder = tempfile::tempdir().expect("folder");
        let mut registry = Registry::load(&folder.path().join("remote.json")).expect("load");
        registry.set_scope("local");
        let bot = registry
            .add_bot("Grok", crate::bot::Rights::FULL, 0)
            .expect("bot");
        assert!(registry.verify_bot(&bot.id, &bot.token, 89 * day));
        assert!(!registry.bot_expired(&bot.id, 89 * day + 89 * day));
        assert!(
            registry.verify_bot(&bot.id, &bot.token, 178 * day),
            "used within 90 days"
        );
        assert!(registry.bot_expired(&bot.id, 178 * day + 91 * day));
        assert!(!registry.verify_bot(&bot.id, &bot.token, 178 * day + 91 * day));
        let renewed = registry
            .renew_bot(&bot.id, 300 * day)
            .expect("saved")
            .expect("renewed");
        assert_eq!(renewed.id, bot.id);
        assert_ne!(renewed.token, bot.token);
        assert!(
            !registry.verify_bot(&bot.id, &bot.token, 300 * day),
            "the old address"
        );
        assert!(registry.verify_bot(&bot.id, &renewed.token, 300 * day));
        assert!(registry.bot_full(&bot.id), "the rest of the link stays");
        assert!(registry.renew_bot("nobody", 1).expect("saved").is_none());
    }

    #[test]
    fn a_device_or_bot_reaches_only_the_workspace_it_was_made_in() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("remote.json");
        let mut registry = Registry::load(&path).expect("load");
        // No workspace open: nothing can be made, and nothing is accepted.
        assert!(matches!(
            registry.start_pairing(0),
            Err(RegistryError::NoWorkspace)
        ));
        assert!(matches!(
            registry.add_bot("Grok", crate::bot::Rights::CHATS, 0),
            Err(RegistryError::NoWorkspace)
        ));

        registry.set_scope("local");
        let pairing = registry.start_pairing(0).expect("pairing");
        let phone = registry
            .pair(&pairing.id, "Phone", 1)
            .expect("pair")
            .expect("device");
        let bot = registry
            .add_bot("Grok", crate::bot::Rights::CHATS, 1)
            .expect("bot");
        let open = registry.start_pairing(2).expect("pairing");

        // Another workspace opens: Local's phone, bot and open link are not its own.
        registry.set_scope("other");
        assert_eq!(registry.device_key(&phone.id), None);
        assert!(registry.device_elsewhere(&phone.id));
        assert!(!registry.device_elsewhere("no-such-one"));
        assert!(!registry.verify_bot(&bot.id, &bot.token, 3));
        assert_eq!(
            registry.pairing_key(&open.id, 3),
            None,
            "a link shown for Local"
        );
        assert!(registry.devices().is_empty() && registry.bots().is_empty());
        // Nor can they be removed from here, or counted against this workspace.
        assert!(!registry.remove_device(&phone.id).expect("remove"));
        assert!(!registry.remove_bot(&bot.id).expect("remove"));
        registry.seen(&phone.id, 10_000_000);
        let pairing = registry.start_pairing(4).expect("pairing");
        let tablet = registry
            .pair(&pairing.id, "Tablet", 5)
            .expect("pair")
            .expect("device");
        assert_eq!(tablet.profile, "other");
        assert_eq!(registry.devices().len(), 1);

        // Back in Local, and after a restart: each workspace has its own again.
        let mut again = Registry::load(&path).expect("reload");
        again.set_scope("local");
        assert_eq!(again.device_key(&phone.id), crypto::decode(&phone.key));
        assert_eq!(
            again.devices()[0].last_seen_at,
            1,
            "untouched from the other workspace"
        );
        assert!(again.verify_bot(&bot.id, &bot.token, 6));
        assert_eq!(again.device_key(&tablet.id), None);
        assert!(again.device_elsewhere(&tablet.id));

        // A record with no workspace (never written by a release) reaches nothing, anywhere.
        let text = std::fs::read_to_string(&path).expect("file");
        std::fs::write(
            &path,
            text.replace(r#""profile": "local""#, r#""profile": """#),
        )
        .expect("write");
        let mut legacy = Registry::load(&path).expect("reload");
        for scope in ["local", "other", ""] {
            legacy.set_scope(scope);
            assert_eq!(legacy.device_key(&phone.id), None, "{scope}");
            assert!(!legacy.verify_bot(&bot.id, &bot.token, 7), "{scope}");
        }
    }

    #[test]
    fn a_device_or_bot_issued_under_another_site_is_removed() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("remote.json");
        let mut registry = Registry::load(&path).expect("load");
        assert_eq!(registry.set_site("https://old.test"), 0);
        registry.set_scope("local");
        let pairing = registry.start_pairing(1).expect("pairing");
        let phone = registry
            .pair(&pairing.id, "Phone", 2)
            .expect("pair")
            .expect("device");
        let bot = registry
            .add_bot("Bot", crate::bot::Rights::FULL, 3)
            .expect("bot");
        // o7 for iPhone, paired under the same site: its key is in the iPhone's Keychain.
        let pairing = registry.start_pairing(3).expect("pairing");
        let iphone = registry
            .pair_as(&pairing.id, "", DeviceKind::Ios, Some("1.0"), 3)
            .expect("pair")
            .expect("device");
        // Another workspace's records go too: the site is the same for all of them.
        registry.set_scope("other");
        let pairing = registry.start_pairing(4).expect("pairing");
        let tablet = registry
            .pair(&pairing.id, "Tablet", 5)
            .expect("pair")
            .expect("device");
        assert_eq!(phone.site, "https://old.test");
        assert_eq!(iphone.site, "https://old.test");

        // The same site again keeps everything.
        let mut same = Registry::load(&path).expect("reload");
        assert_eq!(same.set_site("https://old.test"), 0);
        same.set_scope("local");
        assert!(same.device_key(&phone.id).is_some());
        assert!(same.verify_bot(&bot.id, &bot.token, 6));

        // The site moved: nothing a browser or a bot was issued under the old one is good, here
        // or after a restart. The iPhone app's key never lived under the site, so it stays.
        for _ in 0..2 {
            let mut moved = Registry::load(&path).expect("reload");
            let gone = moved.set_site("https://new.test");
            for scope in ["local", "other"] {
                moved.set_scope(scope);
                assert!(moved.bots().is_empty(), "{scope}");
                assert_eq!(moved.device_key(&phone.id), None);
                assert_eq!(moved.device_key(&tablet.id), None);
                assert!(!moved.device_elsewhere(&tablet.id));
                assert!(!moved.verify_bot(&bot.id, &bot.token, 7));
            }
            moved.set_scope("other");
            assert!(moved.devices().is_empty());
            moved.set_scope("local");
            let kept: Vec<&str> = moved.devices().iter().map(|d| d.id.as_str()).collect();
            assert_eq!(kept, [iphone.id.as_str()]);
            assert_eq!(moved.device_kind(&iphone.id), Some(DeviceKind::Ios));
            assert!(moved.device_key(&iphone.id).is_some());
            // The second time round they are already gone from the file.
            assert!(gone == 3 || gone == 0, "{gone}");
        }
        let saved = std::fs::read_to_string(&path).expect("file");
        assert!(!saved.contains(&phone.key) && !saved.contains(&phone.id));
        assert!(saved.contains(&iphone.key));

        // What is issued now is kept, and says where it was issued.
        let mut moved = Registry::load(&path).expect("reload");
        assert_eq!(moved.set_site("https://new.test"), 0);
        moved.set_scope("local");
        let pairing = moved.start_pairing(8).expect("pairing");
        let again = moved
            .pair(&pairing.id, "Phone", 9)
            .expect("pair")
            .expect("device");
        assert_eq!(again.site, "https://new.test");
        let mut kept = Registry::load(&path).expect("reload");
        assert_eq!(kept.set_site("https://new.test"), 0);
        kept.set_scope("local");
        assert!(kept.device_key(&again.id).is_some());

        // A record from before sites were recorded names none: it was issued under the first
        // address, so it goes when a site is named.
        let text = std::fs::read_to_string(&path).expect("file");
        assert!(text.contains(r#""site": "https://new.test""#));
        std::fs::write(
            &path,
            text.replace(r#""site": "https://new.test""#, r#""site": """#),
        )
        .expect("write");
        let mut early = Registry::load(&path).expect("reload");
        assert_eq!(early.set_site("https://new.test"), 1);
        early.set_scope("local");
        assert_eq!(early.device_key(&again.id), None);
    }

    /// o7 for iPhone is recorded as what it is, with its version, and a browser exactly as
    /// before kinds were recorded.
    #[test]
    fn an_iphone_is_recorded_as_the_app_and_a_browser_as_before() {
        let (folder, mut registry) = fresh();
        let path = folder.path().join("remote.json");
        let pairing = registry.start_pairing(1).expect("pairing");
        let iphone = registry
            .pair_as(
                &pairing.id,
                "  ",
                DeviceKind::Ios,
                Some(" 1.0\n(12)\u{7} "),
                2,
            )
            .expect("pair")
            .expect("device");
        assert_eq!(iphone.name, "iPhone", "the app's name when none is given");
        assert_eq!(iphone.kind, DeviceKind::Ios);
        assert_eq!(iphone.app_version.as_deref(), Some("1.0 (12)"));
        let pairing = registry.start_pairing(3).expect("pairing");
        let long = registry
            .pair_as(
                &pairing.id,
                "Work",
                DeviceKind::Ios,
                Some(&"9".repeat(40)),
                4,
            )
            .expect("pair")
            .expect("device");
        assert_eq!(long.app_version, Some("9".repeat(MAX_CLIENT_FIELD)));
        // A browser keeps no version, whatever it is given, and is named as before.
        let pairing = registry.start_pairing(5).expect("pairing");
        let browser = registry
            .pair_as(&pairing.id, "", DeviceKind::Browser, Some("1.0"), 6)
            .expect("pair")
            .expect("device");
        assert_eq!(
            (browser.name.as_str(), browser.app_version.as_deref()),
            ("Browser", None)
        );
        let pairing = registry.start_pairing(7).expect("pairing");
        let old = registry
            .pair(&pairing.id, "Phone", 8)
            .expect("pair")
            .expect("device");
        assert_eq!(old.kind, DeviceKind::Browser);
        assert_eq!(registry.device_kind(&iphone.id), Some(DeviceKind::Ios));
        assert_eq!(registry.device_kind(&old.id), Some(DeviceKind::Browser));
        assert_eq!(registry.device_kind("nobody"), None);

        // Both survive a restart.
        let mut again = Registry::load(&path).expect("reload");
        again.set_scope("local");
        let found = |registry: &Registry, id: &str| {
            registry
                .devices()
                .into_iter()
                .find(|device| device.id == id)
                .cloned()
                .expect("device")
        };
        assert_eq!(found(&again, &iphone.id), iphone);
        assert_eq!(found(&again, &old.id), old);
        // Another workspace does not see what either is.
        again.set_scope("other");
        assert_eq!(again.device_kind(&iphone.id), None);

        // A browser's record has neither field, so it reads as it did before; the app's has both.
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("file")).expect("json");
        let record = |id: &str| {
            saved["devices"]
                .as_array()
                .expect("devices")
                .iter()
                .find(|device| device["id"] == id)
                .cloned()
                .expect("record")
        };
        for id in [&browser.id, &old.id] {
            let record = record(id);
            assert!(record.get("kind").is_none(), "{record}");
            assert!(record.get("app_version").is_none(), "{record}");
        }
        assert_eq!(record(&iphone.id)["kind"], "ios");
        assert_eq!(record(&iphone.id)["app_version"], "1.0 (12)");

        // A kind a newer build wrote reads as a browser, the device offered least, and the file
        // still loads.
        let text = std::fs::read_to_string(&path).expect("file");
        std::fs::write(
            &path,
            text.replace(r#""kind": "ios""#, r#""kind": "android""#),
        )
        .expect("write");
        let mut newer = Registry::load(&path).expect("a kind it does not know");
        newer.set_scope("local");
        assert_eq!(newer.device_kind(&iphone.id), Some(DeviceKind::Browser));
    }

    fn fresh() -> (tempfile::TempDir, Registry) {
        let folder = tempfile::tempdir().expect("folder");
        let mut registry = Registry::load(&folder.path().join("remote.json")).expect("load");
        registry.set_scope("local");
        (folder, registry)
    }

    #[test]
    fn starts_off_with_an_identity_and_keeps_it_across_restarts() {
        let (folder, mut registry) = fresh();
        let path = folder.path().join("remote.json");
        assert!(!registry.enabled());
        assert!(!path.exists(), "nothing is written until something changes");
        assert_eq!(registry.host_id().len(), 22);
        assert_eq!(registry.host_key().len(), 43);
        // The id is made from the key: nobody else can claim it at the relay.
        let key = crypto::decode::<KEY_BYTES>(registry.host_key()).expect("key");
        assert_eq!(registry.host_id(), crypto::encode(&crypto::host_id(&key)));
        registry.set_enabled(true).expect("enable");
        let (id, key) = (
            registry.host_id().to_owned(),
            registry.host_key().to_owned(),
        );
        let again = Registry::load(&path).expect("reload");
        assert!(again.enabled());
        assert_eq!(
            (again.host_id(), again.host_key()),
            (id.as_str(), key.as_str())
        );
    }

    #[test]
    fn unreadable_settings_are_an_error_not_a_fresh_start() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("remote.json");
        for contents in [
            "{",
            "{}",
            r#"{"version":2,"enabled":true,"host_id":"a","host_key":"b","devices":[],"bots":[]}"#,
        ] {
            std::fs::write(&path, contents).expect("write");
            assert!(
                matches!(Registry::load(&path), Err(RegistryError::Read(_))),
                "{contents}"
            );
            assert_eq!(std::fs::read_to_string(&path).expect("kept"), contents);
        }
    }

    #[test]
    fn a_pairing_link_opens_once_within_its_time() {
        let (_folder, mut registry) = fresh();
        let first = registry.start_pairing(1_000).expect("pairing");
        assert_eq!(first.id.len(), 11);
        assert_eq!(first.expires_at, 1_000 + PAIRING_MS);
        assert_eq!(registry.pairing_key(&first.id, 2_000), Some(first.key));
        assert_eq!(registry.pairing_key("another-id!", 2_000), None);

        // A newer link replaces it.
        let second = registry.start_pairing(3_000).expect("pairing");
        assert_ne!(first.key, second.key);
        assert_eq!(registry.pairing_key(&first.id, 3_500), None);
        assert_eq!(
            registry.pair(&first.id, "Phone", 3_500).expect("pair"),
            None
        );

        let device = registry
            .pair(&second.id, "  iPhone\n(Safari)\u{7} ", 4_000)
            .expect("pair")
            .expect("device");
        assert_eq!(device.name, "iPhone (Safari)");
        assert_eq!(registry.device_key(&device.id), crypto::decode(&device.key));
        // Spent: the same link pairs nothing more.
        assert_eq!(registry.pairing_key(&second.id, 4_001), None);
        assert_eq!(
            registry.pair(&second.id, "Again", 4_001).expect("pair"),
            None
        );
        assert_eq!(registry.devices().len(), 1);

        // Expired, and cancelled.
        let third = registry.start_pairing(10_000).expect("pairing");
        assert_eq!(registry.pairing_key(&third.id, 10_000 + PAIRING_MS), None);
        let fourth = registry.start_pairing(20_000).expect("pairing");
        registry.cancel_pairing();
        assert_eq!(registry.pairing_key(&fourth.id, 20_001), None);
        // Turning remote access off closes an open link too.
        registry.set_enabled(true).expect("on");
        let fifth = registry.start_pairing(30_000).expect("pairing");
        registry.set_enabled(false).expect("off");
        assert_eq!(registry.pairing_key(&fifth.id, 30_001), None);
    }

    #[test]
    fn a_pairing_link_closes_after_repeated_wrong_keys() {
        let (_folder, mut registry) = fresh();
        let pairing = registry.start_pairing(0).expect("pairing");
        for _ in 0..PAIRING_ATTEMPTS - 1 {
            registry.pairing_failed(&pairing.id);
            registry.pairing_failed("someone-else");
        }
        assert!(registry.pairing_key(&pairing.id, 1).is_some());
        registry.pairing_failed(&pairing.id);
        assert!(registry.pairing_key(&pairing.id, 1).is_none());
    }

    #[test]
    fn devices_and_bots_persist_are_bounded_and_can_be_removed() {
        let (folder, mut registry) = fresh();
        let path = folder.path().join("remote.json");
        for count in 0..MAX_DEVICES {
            let pairing = registry.start_pairing(0).expect("pairing");
            registry
                .pair(&pairing.id, &format!("Device {count}"), 5)
                .expect("pair")
                .expect("device");
        }
        let pairing = registry.start_pairing(0).expect("pairing");
        assert!(matches!(
            registry.pair(&pairing.id, "One more", 5),
            Err(RegistryError::TooManyDevices)
        ));
        // The refused pairing did not spend the link.
        assert!(registry.pairing_key(&pairing.id, 6).is_some());
        let first = registry.devices()[0].id.clone();
        assert!(registry.remove_device(&first).expect("remove"));
        assert!(!registry.remove_device(&first).expect("remove again"));
        assert_eq!(registry.device_key(&first), None);

        let bot = registry
            .add_bot("", crate::bot::Rights::CHATS, 7)
            .expect("bot");
        assert_eq!((bot.id.len(), bot.token.len()), (11, 43));
        assert_eq!(registry.bots()[0].name, "Bot");
        assert_ne!(
            registry.bots()[0].token_hash,
            bot.token,
            "the token is never stored"
        );
        assert!(!registry.verify_bot(&bot.id, "wrong", 8));
        assert!(!registry.verify_bot("wrong", &bot.token, 8));
        assert_eq!(registry.bots()[0].last_used_at, None);
        assert_eq!(
            registry.bots()[0].calls,
            0,
            "a refused call is not the link's"
        );
        assert!(registry.verify_bot(&bot.id, &bot.token, 9));
        assert_eq!(registry.bots()[0].last_used_at, Some(9));
        assert!(registry.verify_bot(&bot.id, &bot.token, 10));
        registry.bot_failed(&bot.id);
        registry.bot_failed("another");
        assert_eq!(
            (registry.bots()[0].calls, registry.bots()[0].failed),
            (2, 1)
        );
        // A link is told only about chats it acted on, and only once it names where.
        let place = crate::callback::Callback {
            kind: crate::callback::CallbackKind::Webhook,
            url: "https://example.com/hook".into(),
            channel: None,
            to: None,
        };
        registry.follow(&bot.id, "c1");
        registry.follow("another", "c1");
        assert!(registry.told_of("c1").is_empty(), "no address yet");
        assert!(!registry.any_callback());
        assert!(matches!(
            registry.set_bot_callback("another", Some(place.clone())),
            Ok(false)
        ));
        assert!(matches!(
            registry.set_bot_callback(&bot.id, Some(place.clone())),
            Ok(true)
        ));
        assert!(registry.any_callback());
        assert_eq!(
            registry.told_of("c1"),
            [(
                bot.id.clone(),
                "Bot".to_owned(),
                place,
                crate::bot::Rights::CHATS
            )]
        );
        assert!(registry.told_of("c2").is_empty(), "a chat it never touched");
        for n in 0..MAX_FOLLOWING + 5 {
            registry.follow(&bot.id, &format!("x{n}"));
        }
        assert_eq!(registry.bots()[0].following.len(), MAX_FOLLOWING);
        assert!(
            registry.told_of("c1").is_empty(),
            "the oldest are forgotten"
        );
        assert!(matches!(registry.set_bot_callback(&bot.id, None), Ok(true)));
        assert!(!registry.any_callback());
        for _ in 1..MAX_BOTS {
            registry
                .add_bot("Grok", crate::bot::Rights::FULL, 7)
                .expect("bot");
        }
        assert!(matches!(
            registry.add_bot("Grok", crate::bot::Rights::CHATS, 7),
            Err(RegistryError::TooManyBots)
        ));

        let mut again = Registry::load(&path).expect("reload");
        assert!(again.devices().is_empty(), "no workspace is open yet");
        again.set_scope("local");
        assert_eq!(again.devices().len(), MAX_DEVICES - 1);
        assert_eq!(again.bots().len(), MAX_BOTS);
        assert!(again.verify_bot(&bot.id, &bot.token, 10));
        // The level a link was made with is kept, and is each link's own.
        assert!(!again.bot_full(&bot.id));
        let full = again.bots()[1].id.clone();
        assert!(again.bot_full(&full));
        assert!(!again.bot_full("no-such-bot"));
        assert!(again.remove_bot(&bot.id).expect("remove"));
        assert!(!again.verify_bot(&bot.id, &bot.token, 11));
        assert!(
            !std::fs::read_to_string(&path)
                .expect("file")
                .contains(&bot.token)
        );
    }
}
