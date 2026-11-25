//! Notifications for o7 for iPhone, sealed on this computer for one phone.
//!
//! The relay and Apple carry each notification without reading it. What it says is sealed with a
//! key only this computer and the phone hold: HKDF-SHA256 over the device's key, with no salt and
//! the label `o7 push v1`. The seal is AES-256-GCM under a fresh 12-byte nonce, bound to this
//! computer and the device (`o7 push v1|<host id>|<device id>`), and travels as the unpadded
//! base64url of the nonce, the ciphertext and the tag. The relay wraps it in fixed words for
//! Apple, and the phone's notification extension opens it (`apps/ios`, `PushSeal`).
//!
//! A chat's notifications share one collapse id, so a newer one replaces the older on the phone.
//! It is a keyed hash of the chat's id, so Apple and the relay never see which chat it is.
//!
//! This module seals, words and limits. `host/notify.rs` decides who hears what and sends it
//! through the relay. See docs/remote-sessions.md ("Notifications on the phone").

use std::collections::{HashMap, VecDeque};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload as Sealed},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::crypto::{self, CryptoError, KEY_BYTES};

/// What the push key is derived with, and what every seal is bound to.
pub const LABEL: &str = "o7 push v1";
/// What a collapse id hashes before the chat's id.
const COLLAPSE_LABEL: &str = "o7 push collapse v1|";
/// The most a payload takes once encoded, in bytes. Sealed and encoded it stays well under the
/// 3,400 characters the relay passes on.
pub const MAX_PAYLOAD: usize = 2_400;
pub const NONCE_BYTES: usize = 12;
/// AES-GCM's tag.
const TAG_BYTES: usize = 16;
/// How much of the keyed hash a collapse id keeps.
const COLLAPSE_BYTES: usize = 12;

/// The key a device's notifications are sealed with. The phone's notification extension holds
/// only this key, never the device key itself.
pub fn key(device_key: &[u8; KEY_BYTES]) -> Result<Zeroizing<[u8; KEY_BYTES]>, CryptoError> {
    let mut key = Zeroizing::new([0_u8; KEY_BYTES]);
    // An empty salt is HKDF's own default, as CryptoKit's `salt: Data()` and WebCrypto's empty
    // salt are.
    Hkdf::<Sha256>::new(Some(&[]), device_key)
        .expand(LABEL.as_bytes(), key.as_mut())
        .map_err(|_| CryptoError::Seal)?;
    Ok(key)
}

/// What a seal is bound to: this computer and the device it is for.
fn associated(host_id: &str, device_id: &str) -> String {
    format!("{LABEL}|{host_id}|{device_id}")
}

fn cipher(key: &[u8; KEY_BYTES]) -> Result<Aes256Gcm, CryptoError> {
    Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::Seal)
}

/// Seals `plaintext` for the device `device_id` of this computer (`host_id`), under a fresh
/// random nonce.
pub fn seal(
    key: &[u8; KEY_BYTES],
    host_id: &str,
    device_id: &str,
    plaintext: &[u8],
) -> Result<String, CryptoError> {
    let nonce = crypto::random::<NONCE_BYTES>()?;
    seal_with(key, host_id, device_id, &nonce, plaintext)
}

/// [`seal`] under a given nonce: for the shared vectors only. A nonce is never used twice.
pub(crate) fn seal_with(
    key: &[u8; KEY_BYTES],
    host_id: &str,
    device_id: &str,
    nonce: &[u8; NONCE_BYTES],
    plaintext: &[u8],
) -> Result<String, CryptoError> {
    let sealed = cipher(key)?
        .encrypt(
            &Nonce::<aes_gcm::aes::cipher::typenum::U12>::from(*nonce),
            Sealed {
                msg: plaintext,
                aad: associated(host_id, device_id).as_bytes(),
            },
        )
        .map_err(|_| CryptoError::Seal)?;
    let mut blob = Vec::with_capacity(NONCE_BYTES + sealed.len());
    blob.extend_from_slice(nonce);
    blob.extend_from_slice(&sealed);
    Ok(crypto::encode(&blob))
}

/// Opens what [`seal`] sealed for this computer and device, as the phone does. Anything else, or
/// anything changed on the way, does not open.
pub fn open(
    key: &[u8; KEY_BYTES],
    host_id: &str,
    device_id: &str,
    blob: &str,
) -> Result<Vec<u8>, CryptoError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(blob)
        .map_err(|_| CryptoError::Open)?;
    if bytes.len() < NONCE_BYTES + TAG_BYTES {
        return Err(CryptoError::Open);
    }
    let (nonce, sealed) = bytes.split_at(NONCE_BYTES);
    let nonce: [u8; NONCE_BYTES] = nonce.try_into().map_err(|_| CryptoError::Open)?;
    cipher(key)
        .map_err(|_| CryptoError::Open)?
        .decrypt(
            &Nonce::<aes_gcm::aes::cipher::typenum::U12>::from(nonce),
            Sealed {
                msg: sealed,
                aad: associated(host_id, device_id).as_bytes(),
            },
        )
        .map_err(|_| CryptoError::Open)
}

/// The collapse id of a chat's notifications on one device: the first twelve bytes of
/// HMAC-SHA256 under the device key of `o7 push collapse v1|` and the chat's id, unpadded
/// base64url. A newer notification with it replaces the older one on the phone.
pub fn collapse(device_key: &[u8; KEY_BYTES], chat: &str) -> String {
    // HKDF's extract step is HMAC-SHA256 keyed with its salt.
    let (mac, _) = Hkdf::<Sha256>::extract(
        Some(device_key),
        format!("{COLLAPSE_LABEL}{chat}").as_bytes(),
    );
    crypto::encode(&mac[..COLLAPSE_BYTES])
}

/// Whether `token` is a phone's address at Apple as the app gives it: 64 to 200 hex characters.
pub fn token_ok(token: &str) -> bool {
    (64..=200).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Why a notification was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A chat started waiting on the person: a permission or a question.
    Asking,
    /// A chat's turn ended.
    Finished,
    /// A chat's agent failed or needs its owner to sign in.
    Failed,
    /// A task from the phone was reviewed.
    Reviewed,
    /// A task from the phone has news on its pull request.
    PullRequest,
    /// A task the phone queued for tonight could not start.
    QueuedFailed,
    /// What a chat asked was answered elsewhere: the notification goes quietly.
    Resolved,
    /// Asked for, to see that notifications arrive.
    Test,
}

impl Kind {
    /// Whether a device that chose `prefs` wants this kind.
    pub fn wanted(self, prefs: &Prefs) -> bool {
        match self {
            Self::Asking | Self::Resolved => prefs.asking,
            Self::Finished => prefs.finished,
            Self::Failed => prefs.failed,
            Self::Reviewed | Self::PullRequest | Self::QueuedFailed => prefs.tasks,
            Self::Test => true,
        }
    }

    /// The rule of one per chat and kind every fifteen seconds does not hold these: a resolved
    /// notification takes back what was asked, and a test was asked for.
    fn exempt(self) -> bool {
        matches!(self, Self::Resolved | Self::Test)
    }
}

/// A request the notification can answer, as `permission` or `answer` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Request {
    pub id: String,
    /// `permission`, or the question's kind: `confirm`, `select`, `input`, `form` or `questions`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// The option of a permission that allows once. Never one that allows always.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<String>,
    /// The option of a permission that rejects once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<String>,
}

/// What a notification says once the phone opens it (`PushPayload` in `apps/ios`). What a field
/// does not know is left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Payload {
    /// The payload's version: 1.
    pub v: u32,
    /// Unique to this notification: the phone drops a repeat.
    pub id: String,
    /// When it was sent, in Unix milliseconds: the phone drops one older than the last it showed.
    pub at: i64,
    /// This computer's name.
    pub host: String,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    /// The chat's title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// What is asked, or what happened, in one line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// How many chats on this computer wait on the person now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asking_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<Request>,
    /// For a review: the reviewer's chat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    /// For queued work that could not start: its id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued: Option<String>,
}

impl Payload {
    /// A notification of `kind` from the computer `host`, with a fresh id and the time now.
    pub fn new(kind: Kind, host: &str, at: i64) -> Result<Self, CryptoError> {
        Ok(Self {
            v: 1,
            id: crypto::encode(&crypto::random::<12>()?),
            at,
            host: crate::render::cut(host, 80),
            kind,
            chat: None,
            title: None,
            project: None,
            agent: None,
            line: None,
            asking_count: None,
            request: None,
            review: None,
            queued: None,
        })
    }

    /// The payload as it is sealed, at most [`MAX_PAYLOAD`] bytes. One that is longer loses the
    /// end of its line (all of it, if need be), then of its title, then its optional parts one at
    /// a time. Its version, id, time, computer, kind and chat always stay; `None` only if even
    /// they are too long.
    pub fn encoded(mut self) -> Option<Vec<u8>> {
        if let Some(bytes) = fits(&self) {
            return Some(bytes);
        }
        let texts: [fn(&mut Self) -> &mut Option<String>; 2] =
            [|payload| &mut payload.line, |payload| &mut payload.title];
        for text in texts {
            if let Some(bytes) = trim(&mut self, text) {
                return Some(bytes);
            }
            // Not even its start fits: it goes.
            *text(&mut self) = None;
            if let Some(bytes) = fits(&self) {
                return Some(bytes);
            }
        }
        let parts: [fn(&mut Self); 6] = [
            |payload| payload.project = None,
            |payload| payload.agent = None,
            |payload| payload.review = None,
            |payload| payload.queued = None,
            |payload| payload.asking_count = None,
            |payload| payload.request = None,
        ];
        for drop in parts {
            drop(&mut self);
            if let Some(bytes) = fits(&self) {
                return Some(bytes);
            }
        }
        None
    }
}

/// `payload` encoded, if it fits.
fn fits(payload: &Payload) -> Option<Vec<u8>> {
    serde_json::to_vec(payload)
        .ok()
        .filter(|bytes| bytes.len() <= MAX_PAYLOAD)
}

/// Cuts the text `field` names to the most of its start that lets `payload` fit, marked where it
/// was cut, and gives the payload as it then is; `None` when no start of it fits.
fn trim(payload: &mut Payload, field: fn(&mut Payload) -> &mut Option<String>) -> Option<Vec<u8>> {
    let text = field(payload).clone()?;
    let mut best = None;
    let (mut low, mut high) = (0, text.chars().count().saturating_sub(1));
    while low <= high {
        let keep = low + (high - low) / 2;
        *field(payload) = Some(crate::render::cut(&text, keep));
        if fits(payload).is_some() {
            best = Some(keep);
            low = keep + 1;
        } else if keep == 0 {
            break;
        } else {
            high = keep - 1;
        }
    }
    *field(payload) = Some(crate::render::cut(&text, best?));
    fits(payload)
}

/// What a computer's Live Activity shows, sealed as a notification is: how many chats wait on
/// the person and how many work, and the one that leads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Activity {
    pub asking: usize,
    pub working: usize,
    /// The chat that leads, in plain words: the Lock Screen draws no Markdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    /// When the leading work started, in Unix milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
}

/// Words without Markdown's marks, on one line of at most `chars` characters.
pub fn plain(text: &str, chars: usize) -> String {
    let words: Vec<&str> = text
        .split_whitespace()
        .map(|word| word.trim_start_matches('#'))
        .filter(|word| !word.is_empty())
        .collect();
    let flat = words.join(" ").replace(['`', '*'], "");
    crate::render::cut(&flat, chars)
}

/// Which chats a device hears of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Every chat of the workspace.
    #[default]
    All,
    /// Only those the device started, sent to or answered.
    Mine,
}

impl Scope {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "all" => Some(Self::All),
            "mine" => Some(Self::Mine),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for Scope {
    /// A scope a newer build wrote reads as `mine`, the one that tells least.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "all" => Self::All,
            _ => Self::Mine,
        })
    }
}

fn on() -> bool {
    true
}

/// What a device wants to hear of. Everything, of every chat, unless it says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// A chat waits on the person, and the notification goes once it was answered elsewhere.
    #[serde(default = "on")]
    pub asking: bool,
    #[serde(default = "on")]
    pub finished: bool,
    #[serde(default = "on")]
    pub failed: bool,
    /// The device's tasks: a review is in, queued work could not start, a pull request changed.
    #[serde(default = "on")]
    pub tasks: bool,
    #[serde(default)]
    pub scope: Scope,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            asking: true,
            finished: true,
            failed: true,
            tasks: true,
            scope: Scope::All,
        }
    }
}

/// Which of Apple's services a token belongs to: a development build's, or the App Store's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Env {
    Sandbox,
    Production,
}

impl Env {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "sandbox" => Some(Self::Sandbox),
            "production" => Some(Self::Production),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Production => "production",
        }
    }
}

impl<'de> Deserialize<'de> for Env {
    /// A service a newer build wrote reads as Apple's own: a wrong one is refused there, and the
    /// token is dropped.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "sandbox" => Self::Sandbox,
            _ => Self::Production,
        })
    }
}

/// One notification per chat and kind in this long, to a device.
pub const PER_CHAT_MS: i64 = 15_000;
/// At most this many notifications to a device in [`WINDOW_MS`], and [`PER_DAY`] a day.
pub const PER_WINDOW: usize = 30;
pub const WINDOW_MS: i64 = 10 * 60_000;
pub const PER_DAY: usize = 300;
const DAY_MS: i64 = 24 * 60 * 60_000;

/// How many notifications one device has been sent, to keep within its limits: one per chat and
/// kind every fifteen seconds (a resolved one and a test aside), thirty every ten minutes and
/// three hundred a day.
#[derive(Debug, Default)]
pub struct Limiter {
    /// When each notification of the last day went, oldest first.
    sent: VecDeque<i64>,
    /// When each chat last had a notification of each kind, for the last fifteen seconds.
    last: HashMap<(String, Kind), i64>,
}

impl Limiter {
    /// Whether one more of `kind`, about `chat`, may go at `now`; counted when it may.
    pub fn allow(&mut self, chat: Option<&str>, kind: Kind, now: i64) -> bool {
        while self
            .sent
            .front()
            .is_some_and(|at| now.saturating_sub(*at) >= DAY_MS)
        {
            self.sent.pop_front();
        }
        self.last
            .retain(|_, at| now.saturating_sub(*at) < PER_CHAT_MS);
        let chat = chat.filter(|_| !kind.exempt());
        if chat.is_some_and(|chat| self.last.contains_key(&(chat.to_owned(), kind))) {
            return false;
        }
        let recent = self
            .sent
            .iter()
            .filter(|at| now.saturating_sub(**at) < WINDOW_MS)
            .count();
        if recent >= PER_WINDOW || self.sent.len() >= PER_DAY {
            return false;
        }
        self.sent.push_back(now);
        if let Some(chat) = chat {
            self.last.insert((chat.to_owned(), kind), now);
        }
        true
    }

    /// Whether nothing was sent in the last day: the limiter can be forgotten.
    pub fn idle(&self, now: i64) -> bool {
        self.sent
            .back()
            .is_none_or(|at| now.saturating_sub(*at) >= DAY_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device_key() -> [u8; KEY_BYTES] {
        std::array::from_fn(|index| index as u8)
    }

    #[test]
    fn a_seal_opens_only_for_its_computer_and_device_and_unchanged() {
        let key = key(&device_key()).expect("key");
        let blob = seal(&key, "HOST", "DEVICE", b"{\"v\":1}").expect("seal");
        assert_eq!(
            open(&key, "HOST", "DEVICE", &blob).expect("open"),
            b"{\"v\":1}"
        );
        // A fresh nonce each time: the same words never seal the same.
        assert_ne!(
            blob,
            seal(&key, "HOST", "DEVICE", b"{\"v\":1}").expect("seal")
        );
        assert_eq!(open(&key, "OTHER", "DEVICE", &blob), Err(CryptoError::Open));
        assert_eq!(open(&key, "HOST", "OTHER", &blob), Err(CryptoError::Open));
        let other = super::key(&[9; KEY_BYTES]).expect("key");
        assert_eq!(
            open(&other, "HOST", "DEVICE", &blob),
            Err(CryptoError::Open)
        );
        let mut bytes = URL_SAFE_NO_PAD.decode(&blob).expect("base64");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(
            open(&key, "HOST", "DEVICE", &crypto::encode(&bytes)),
            Err(CryptoError::Open)
        );
        assert_eq!(
            open(&key, "HOST", "DEVICE", "short"),
            Err(CryptoError::Open)
        );
        assert_eq!(
            open(&key, "HOST", "DEVICE", "not base64!"),
            Err(CryptoError::Open)
        );
        // The blob is the nonce, the ciphertext and the tag.
        assert_eq!(
            URL_SAFE_NO_PAD.decode(&blob).expect("base64").len(),
            NONCE_BYTES + 7 + TAG_BYTES
        );
    }

    #[test]
    fn the_push_key_is_not_the_device_key_and_collapse_ids_say_nothing() {
        let device = device_key();
        let key = key(&device).expect("key");
        assert_ne!(*key, device);
        assert_eq!(
            *key,
            *super::key(&device).expect("again"),
            "derived the same"
        );
        let one = collapse(&device, "c1");
        assert_eq!(one.len(), 16);
        assert_eq!(one, collapse(&device, "c1"), "one per chat");
        assert_ne!(one, collapse(&device, "c2"));
        assert_ne!(one, collapse(&[7; KEY_BYTES], "c1"), "each device's own");
        assert!(!one.contains("c1"));
        assert!(token_ok(&"a".repeat(64)));
        assert!(token_ok(&"AbCd09".repeat(20)));
        assert!(token_ok(&"f".repeat(200)));
        for bad in [
            "a".repeat(63),
            "a".repeat(201),
            "g".repeat(64),
            " ".repeat(64),
        ] {
            assert!(!token_ok(&bad), "{bad}");
        }
    }

    fn payload(kind: Kind) -> Payload {
        let mut payload = Payload::new(kind, "Studio", 1_000).expect("payload");
        payload.chat = Some("c1".into());
        payload
    }

    #[test]
    fn a_payload_reads_as_the_phone_reads_it_and_leaves_out_what_it_does_not_know() {
        let mut asking = payload(Kind::Asking);
        asking.id = "n1".into();
        asking.title = Some("Fix the login form".into());
        asking.asking_count = Some(2);
        asking.request = Some(Request {
            id: "p1".into(),
            kind: "permission",
            allow: Some("allow".into()),
            deny: None,
        });
        assert_eq!(
            String::from_utf8(asking.encoded().expect("fits")).expect("utf-8"),
            r#"{"v":1,"id":"n1","at":1000,"host":"Studio","kind":"asking","chat":"c1","title":"Fix the login form","asking_count":2,"request":{"id":"p1","type":"permission","allow":"allow"}}"#
        );
        let mut test = Payload::new(Kind::Test, "Studio", 5).expect("payload");
        test.id = "t".into();
        assert_eq!(
            String::from_utf8(test.encoded().expect("fits")).expect("utf-8"),
            r#"{"v":1,"id":"t","at":5,"host":"Studio","kind":"test"}"#
        );
        for (kind, word) in [
            (Kind::Finished, "finished"),
            (Kind::Failed, "failed"),
            (Kind::Reviewed, "reviewed"),
            (Kind::PullRequest, "pull_request"),
            (Kind::QueuedFailed, "queued_failed"),
            (Kind::Resolved, "resolved"),
        ] {
            assert_eq!(serde_json::to_value(kind).expect("kind"), word);
        }
        // A fresh id for each.
        assert_ne!(payload(Kind::Test).id, payload(Kind::Test).id);
    }

    #[test]
    fn a_long_payload_loses_its_line_then_its_title_then_its_extras() {
        let mut long = payload(Kind::Asking);
        long.title = Some("Title ".repeat(50));
        long.line = Some("é".repeat(3_000));
        long.project = Some("app".into());
        let bytes = long.clone().encoded().expect("fits");
        assert!(
            bytes.len() <= MAX_PAYLOAD && bytes.len() > MAX_PAYLOAD - 8,
            "{}",
            bytes.len()
        );
        let read: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(
            read["title"],
            long.title.clone().expect("title"),
            "the title is whole"
        );
        let line = read["line"].as_str().expect("line");
        assert!(line.ends_with('…') && line.starts_with("éé"), "{line}");

        // A title too long for any line: the line goes, then the title is cut.
        long.title = Some("t".repeat(4_000));
        let read: serde_json::Value =
            serde_json::from_slice(&long.clone().encoded().expect("fits")).expect("json");
        assert!(read.get("line").is_none(), "{read}");
        assert!(read["title"].as_str().expect("title").ends_with('…'));
        assert_eq!(read["project"], "app", "kept while the title could be cut");

        // Parts the computer does not cut (an agent's option ids) go one by one, never the
        // version, id, time, computer, kind or chat.
        let mut wild = payload(Kind::Asking);
        wild.request = Some(Request {
            id: "r".repeat(3_000),
            kind: "permission",
            allow: None,
            deny: None,
        });
        wild.project = Some("app".into());
        let read: serde_json::Value =
            serde_json::from_slice(&wild.encoded().expect("fits")).expect("json");
        assert!(
            read.get("request").is_none() && read.get("project").is_none(),
            "{read}"
        );
        assert_eq!(
            (&read["v"], &read["kind"], &read["chat"], &read["host"]),
            (
                &serde_json::json!(1),
                &serde_json::json!("asking"),
                &serde_json::json!("c1"),
                &serde_json::json!("Studio")
            )
        );
        let mut hopeless = payload(Kind::Finished);
        hopeless.chat = Some("c".repeat(3_000));
        assert_eq!(hopeless.encoded(), None, "the chat is never cut");
    }

    #[test]
    fn a_device_hears_what_it_chose() {
        let all = Prefs::default();
        let quiet = Prefs {
            asking: false,
            finished: false,
            failed: true,
            tasks: false,
            scope: Scope::Mine,
        };
        for kind in [
            Kind::Asking,
            Kind::Resolved,
            Kind::Finished,
            Kind::Failed,
            Kind::Reviewed,
            Kind::PullRequest,
            Kind::QueuedFailed,
            Kind::Test,
        ] {
            assert!(kind.wanted(&all), "{kind:?}");
        }
        assert!(!Kind::Asking.wanted(&quiet) && !Kind::Resolved.wanted(&quiet));
        assert!(!Kind::Finished.wanted(&quiet));
        assert!(Kind::Failed.wanted(&quiet));
        assert!(!Kind::Reviewed.wanted(&quiet) && !Kind::QueuedFailed.wanted(&quiet));
        assert!(!Kind::PullRequest.wanted(&quiet));
        assert!(Kind::Test.wanted(&quiet), "a test was asked for");
        // As kept: every part, read back; a missing part is on, and a scope a newer build wrote
        // tells least.
        let kept = serde_json::to_string(&quiet).expect("json");
        assert_eq!(
            kept,
            r#"{"asking":false,"finished":false,"failed":true,"tasks":false,"scope":"mine"}"#
        );
        assert_eq!(serde_json::from_str::<Prefs>(&kept).expect("prefs"), quiet);
        assert_eq!(serde_json::from_str::<Prefs>("{}").expect("prefs"), all);
        assert_eq!(
            serde_json::from_str::<Prefs>(r#"{"scope":"starred"}"#)
                .expect("prefs")
                .scope,
            Scope::Mine
        );
        assert_eq!(Scope::parse("mine"), Some(Scope::Mine));
        assert_eq!(Scope::parse("everything"), None);
        assert_eq!(Env::parse("sandbox"), Some(Env::Sandbox));
        assert_eq!(Env::parse("prod"), None);
        assert_eq!(
            serde_json::from_str::<Env>(r#""moon""#).expect("env"),
            Env::Production
        );
    }

    #[test]
    fn a_device_is_sent_one_per_chat_and_kind_a_while_and_so_many_an_hour_and_a_day() {
        let mut limits = Limiter::default();
        assert!(limits.allow(Some("c1"), Kind::Asking, 0));
        assert!(
            !limits.allow(Some("c1"), Kind::Asking, 14_999),
            "the same chat and kind"
        );
        assert!(limits.allow(Some("c1"), Kind::Finished, 1), "another kind");
        assert!(limits.allow(Some("c2"), Kind::Asking, 2), "another chat");
        assert!(limits.allow(Some("c1"), Kind::Resolved, 3));
        assert!(
            limits.allow(Some("c1"), Kind::Resolved, 4),
            "resolved is exempt"
        );
        assert!(limits.allow(None, Kind::Test, 5));
        assert!(limits.allow(None, Kind::Test, 6), "a test too");
        assert!(
            limits.allow(Some("c1"), Kind::Asking, 15_000),
            "fifteen seconds on"
        );

        // Thirty in ten minutes, whatever they are about.
        let mut limits = Limiter::default();
        for n in 0..PER_WINDOW {
            assert!(limits.allow(Some(&format!("c{n}")), Kind::Finished, n as i64));
        }
        assert!(!limits.allow(Some("other"), Kind::Finished, 100));
        assert!(!limits.allow(None, Kind::Test, 100), "a test counts too");
        assert!(limits.allow(Some("other"), Kind::Finished, WINDOW_MS));

        // Three hundred a day.
        let mut limits = Limiter::default();
        let mut at = 0;
        for n in 0..PER_DAY {
            assert!(
                limits.allow(Some(&format!("c{n}")), Kind::Finished, at),
                "{n}"
            );
            at += WINDOW_MS / PER_WINDOW as i64;
        }
        assert!(!limits.allow(Some("late"), Kind::Finished, at + WINDOW_MS));
        assert!(!limits.idle(at));
        assert!(limits.allow(Some("late"), Kind::Finished, DAY_MS + 1));
        assert!(limits.idle(3 * DAY_MS));
    }

    #[test]
    fn plain_words_drop_markdowns_marks() {
        assert_eq!(
            plain("## Run `npm test` **now**\nplease", 100),
            "Run npm test now please"
        );
        assert_eq!(plain("abcdef", 3), "abc…");
    }
}

/// The vectors the phone is held to (`tests/push-vectors.json`): `apps/ios`'s `O7PushTests` read
/// the same file, so the two cannot drift apart.
#[cfg(test)]
mod vectors {
    use super::*;

    #[derive(Debug, serde::Deserialize, serde::Serialize)]
    struct File {
        note: String,
        vectors: Vec<Vector>,
    }

    #[derive(Debug, serde::Deserialize, serde::Serialize)]
    struct Vector {
        device_key: String,
        push_key: String,
        host_id: String,
        device_id: String,
        nonce: String,
        plaintext: String,
        blob: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collapse: Option<String>,
    }

    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/push-vectors.json");

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn unhex<const N: usize>(text: &str) -> [u8; N] {
        let bytes: Vec<u8> = (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex"))
            .collect();
        bytes.try_into().expect("length")
    }

    /// What the vectors seal: a permission asked, a turn finished in words beyond ASCII, a test
    /// with no chat, and a Live Activity's state.
    fn written() -> File {
        let host = "TW9jayBjb21wdXRlciAxMg";
        let device = "TW9ja0RldjE";
        let device_key: [u8; KEY_BYTES] = std::array::from_fn(|index| index as u8);
        let at = 1_764_043_200_000;
        let mut asking = Payload::new(Kind::Asking, "Studio", at).expect("payload");
        asking.id = "AAECAwQFBgcICQoL".into();
        asking.chat = Some("c1".into());
        asking.title = Some("Fix the login form".into());
        asking.project = Some("app".into());
        asking.agent = Some("Claude Code".into());
        asking.line = Some("Run `npm test`".into());
        asking.asking_count = Some(1);
        asking.request = Some(Request {
            id: "p1".into(),
            kind: "permission",
            allow: Some("allow".into()),
            deny: Some("reject".into()),
        });
        let mut finished = Payload::new(Kind::Finished, "Studio Ü", at + 1).expect("payload");
        finished.id = "DA0ODxAREhMUFRYX".into();
        finished.chat = Some("c2".into());
        finished.title = Some("Résumé page".into());
        finished.line = Some("All 12 passed. Prüfe das, bitte ✓".into());
        finished.asking_count = Some(0);
        let mut test = Payload::new(Kind::Test, "Studio", at + 2).expect("payload");
        test.id = "GBkaGxwdHh8gISIj".into();
        test.line = Some("Notifications from Studio arrive here.".into());
        let activity = Activity {
            asking: 1,
            working: 2,
            lead: Some("Fix the login form: Run npm test".into()),
            since: Some(at - 60_000),
        };
        let push_key = key(&device_key).expect("key");
        let sealed = |plaintext: Vec<u8>, nonce: [u8; NONCE_BYTES], chat: Option<&str>| Vector {
            device_key: hex(&device_key),
            push_key: hex(push_key.as_ref()),
            host_id: host.into(),
            device_id: device.into(),
            nonce: hex(&nonce),
            blob: seal_with(&push_key, host, device, &nonce, &plaintext).expect("seal"),
            plaintext: String::from_utf8(plaintext).expect("utf-8"),
            chat: chat.map(str::to_owned),
            collapse: chat.map(|chat| collapse(&device_key, chat)),
        };
        File {
            note: "Notifications o7 seals for o7 for iPhone (crates/o7-remote/src/push.rs). Each plaintext is sealed with push_key, HKDF-SHA256 of device_key with no salt and the info \"o7 push v1\", under AES-256-GCM with the given nonce and the associated data \"o7 push v1|<host_id>|<device_id>\". blob is the unpadded base64url of nonce, ciphertext and tag. collapse is the unpadded base64url of the first 12 bytes of HMAC-SHA256 under device_key of \"o7 push collapse v1|<chat>\". The last vector is a Live Activity's content-state, sealed the same way. Rewrite with O7_WRITE_VECTORS=1 cargo test -p o7-remote push::vectors.".into(),
            vectors: vec![
                sealed(asking.encoded().expect("fits"), [0x11; NONCE_BYTES], Some("c1")),
                sealed(finished.encoded().expect("fits"), [0x22; NONCE_BYTES], Some("c2")),
                sealed(test.encoded().expect("fits"), [0x33; NONCE_BYTES], None),
                sealed(
                    serde_json::to_vec(&activity).expect("json"),
                    [0x44; NONCE_BYTES],
                    None,
                ),
            ],
        }
    }

    /// Writes the vectors anew: `O7_WRITE_VECTORS=1 cargo test -p o7-remote push::vectors`.
    /// Without it, checks the file against what this code seals.
    #[test]
    fn seals_and_opens_the_shared_vectors() {
        let made = written();
        if std::env::var_os("O7_WRITE_VECTORS").is_some() {
            let json = serde_json::to_string_pretty(&made).expect("json");
            std::fs::write(PATH, format!("{json}\n")).expect("write");
        }
        let file: File =
            serde_json::from_str(&std::fs::read_to_string(PATH).expect("push-vectors.json"))
                .expect("vectors");
        assert!(file.vectors.len() >= 3);
        assert_eq!(
            serde_json::to_value(&file.vectors).expect("json"),
            serde_json::to_value(&made.vectors).expect("json"),
            "the file is what this code seals"
        );
        for vector in &file.vectors {
            let device_key = unhex::<KEY_BYTES>(&vector.device_key);
            let push_key = key(&device_key).expect("key");
            assert_eq!(hex(push_key.as_ref()), vector.push_key);
            let nonce = unhex::<NONCE_BYTES>(&vector.nonce);
            assert_eq!(
                seal_with(
                    &push_key,
                    &vector.host_id,
                    &vector.device_id,
                    &nonce,
                    vector.plaintext.as_bytes()
                )
                .expect("seal"),
                vector.blob
            );
            assert_eq!(
                open(&push_key, &vector.host_id, &vector.device_id, &vector.blob).expect("open"),
                vector.plaintext.as_bytes()
            );
            if let (Some(chat), Some(id)) = (&vector.chat, &vector.collapse) {
                assert_eq!(&collapse(&device_key, chat), id);
            }
        }
    }
}
