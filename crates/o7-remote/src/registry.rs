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

use serde::{Deserialize, Serialize};

use crate::crypto::{self, KEY_BYTES};

pub const MAX_DEVICES: usize = 16;
pub const MAX_BOTS: usize = 16;
pub const MAX_NAME: usize = 48;
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Stored {
    version: u32,
    enabled: bool,
    /// This desktop's id at the relay (16 bytes) and the key that claims it (32 bytes).
    host_id: String,
    host_key: String,
    devices: Vec<Device>,
    bots: Vec<Bot>,
}

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

    /// Names the website this copy pairs through, and removes every device and bot link issued
    /// under another one, in every workspace. Returns how many went.
    ///
    /// A browser keeps a device's key under the site's origin, and a bot's token is part of an
    /// address at the site. When the site moves, whoever holds the old address later holds a
    /// page that can use those keys and receives those tokens, so none of them may stay good:
    /// the phone pairs again and the bot is linked again, at the new address.
    pub fn set_site(&mut self, site: &str) -> usize {
        self.site = site.to_owned();
        let before = self.stored.devices.len() + self.stored.bots.len();
        self.stored.devices.retain(|device| device.site == site);
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

    /// Spends the pairing link `id` on a new device. The link never opens again.
    pub fn pair(
        &mut self,
        id: &str,
        name: &str,
        now: i64,
    ) -> Result<Option<Device>, RegistryError> {
        if self.pairing_key(id, now).is_none() {
            return Ok(None);
        }
        if self.devices().len() >= MAX_DEVICES {
            return Err(RegistryError::TooManyDevices);
        }
        let device = Device {
            id: crypto::encode(&crypto::random::<8>()?),
            key: crypto::encode(&crypto::random::<KEY_BYTES>()?),
            name: name_of(name, "Browser"),
            created_at: now,
            last_seen_at: now,
            profile: self.scope.clone(),
            site: self.site.clone(),
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

    pub fn add_bot(&mut self, name: &str, full: bool, now: i64) -> Result<NewBot, RegistryError> {
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
            full,
            site: self.site.clone(),
            calls: 0,
            failed: 0,
        });
        if let Err(error) = self.save() {
            self.stored.bots.pop();
            return Err(error);
        }
        Ok(new)
    }

    /// Whether `token` is the bot `id`'s, and that bot was linked in the open workspace. Notes
    /// the use.
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
        if !crypto::same(&bot.token_hash, &hash) {
            return false;
        }
        bot.calls = bot.calls.saturating_add(1);
        if bot.last_used_at.is_none_or(|last| now - last >= 60_000) {
            bot.last_used_at = Some(now);
            let _ = self.save();
        }
        true
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
        self.bots()
            .iter()
            .any(|bot| crypto::same(&bot.id, id) && bot.full)
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

/// A name a person or a browser supplied, as one short line; `fallback` when nothing is left.
fn name_of(name: &str, fallback: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME)
        .collect();
    if name.is_empty() {
        fallback.to_owned()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            registry.add_bot("Grok", false, 0),
            Err(RegistryError::NoWorkspace)
        ));

        registry.set_scope("local");
        let pairing = registry.start_pairing(0).expect("pairing");
        let phone = registry
            .pair(&pairing.id, "Phone", 1)
            .expect("pair")
            .expect("device");
        let bot = registry.add_bot("Grok", false, 1).expect("bot");
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
        let bot = registry.add_bot("Bot", true, 3).expect("bot");
        // Another workspace's records go too: the site is the same for all of them.
        registry.set_scope("other");
        let pairing = registry.start_pairing(4).expect("pairing");
        let tablet = registry
            .pair(&pairing.id, "Tablet", 5)
            .expect("pair")
            .expect("device");
        assert_eq!(phone.site, "https://old.test");

        // The same site again keeps everything.
        let mut same = Registry::load(&path).expect("reload");
        assert_eq!(same.set_site("https://old.test"), 0);
        same.set_scope("local");
        assert!(same.device_key(&phone.id).is_some());
        assert!(same.verify_bot(&bot.id, &bot.token, 6));

        // The site moved: nothing issued under the old one is good, here or after a restart.
        for _ in 0..2 {
            let mut moved = Registry::load(&path).expect("reload");
            let gone = moved.set_site("https://new.test");
            for scope in ["local", "other"] {
                moved.set_scope(scope);
                assert!(
                    moved.devices().is_empty() && moved.bots().is_empty(),
                    "{scope}"
                );
                assert_eq!(moved.device_key(&phone.id), None);
                assert_eq!(moved.device_key(&tablet.id), None);
                assert!(!moved.device_elsewhere(&tablet.id));
                assert!(!moved.verify_bot(&bot.id, &bot.token, 7));
            }
            // The second time round they are already gone from the file.
            assert!(gone == 3 || gone == 0);
        }
        let saved = std::fs::read_to_string(&path).expect("file");
        assert!(!saved.contains(&phone.key) && !saved.contains(&phone.id));

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

        let bot = registry.add_bot("", false, 7).expect("bot");
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
        for _ in 1..MAX_BOTS {
            registry.add_bot("Grok", true, 7).expect("bot");
        }
        assert!(matches!(
            registry.add_bot("Grok", false, 7),
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
