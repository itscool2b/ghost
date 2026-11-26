//! The sealed channel between a paired browser and this desktop.
//!
//! Both sides hold one 32-byte secret: a pairing link's key, or the device key a pairing produced.
//! A session derives two AES-256-GCM keys from it and from a fresh nonce of each side, one per
//! direction, and numbers its messages from zero. The relay between them carries only what this
//! seals: it can drop a message, never read, change, reorder or replay one.
//!
//! `apps/web/app/remote/channel.ts` is the browser's half; both are held to
//! `tests/vectors.json`.

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const KEY_BYTES: usize = 32;
pub const NONCE_BYTES: usize = 16;

const TO_HOST: &[u8] = b"auracode remote v1 client to host";
const TO_CLIENT: &[u8] = b"auracode remote v1 host to client";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("The system's random source is unavailable.")]
    Random,
    #[error("The message could not be opened.")]
    Open,
    #[error("The message could not be sealed.")]
    Seal,
}

pub fn random<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|_| CryptoError::Random)?;
    Ok(bytes)
}

/// Unpadded base64url, the encoding of every id, key and sealed message on the wire.
/// The door's proof that it holds this run's key (`door.json`): HMAC-SHA256 under the
/// challenge, of the key (HKDF's extract step), in base64url. Whoever answers a challenge with
/// it read the key, which only Ghost and its person's account can.
pub fn door_proof(key: &[u8], challenge: &[u8]) -> String {
    let (proof, _) = Hkdf::<Sha256>::extract(Some(challenge), key);
    encode(&proof)
}

/// Whether `proof` answers `challenge` under `key`, compared in constant time.
pub fn door_proven(key: &[u8], challenge: &[u8], proof: &str) -> bool {
    same(&door_proof(key, challenge), proof)
}

pub fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Exactly `N` bytes of canonical unpadded base64url, or nothing.
pub fn decode<const N: usize>(text: &str) -> Option<[u8; N]> {
    // Bound the input before decoding: four characters carry three bytes.
    if text.len() != (N * 4).div_ceil(3) {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
    (encode(&bytes) == text).then_some(())?;
    bytes.try_into().ok()
}

/// SHA-256 of a secret's text form, as the relay stores it to recognise the secret again.
/// A computer's id at the relay: the start of a hash of its key, so only the key can claim the
/// id. The relay makes the same check before a request reaches the computer's room.
pub fn host_id(key: &[u8; KEY_BYTES]) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::new()
        .chain_update(b"auracode remote v1 host id")
        .chain_update(key)
        .finalize();
    let mut id = [0_u8; 16];
    id.copy_from_slice(&digest[..16]);
    id
}

pub fn hash(text: &str) -> String {
    encode(&Sha256::digest(text.as_bytes()))
}

/// Whether two hashes are equal, without stopping at the first difference.
pub fn same(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// Which end of a session a [`Channel`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Host,
    Client,
}

/// One session's two keys and message counters.
pub struct Channel {
    seal: Aes256Gcm,
    open: Aes256Gcm,
    sent: u64,
    received: u64,
    context: Vec<u8>,
}

impl Channel {
    /// `context` names what the secret is (`pair` or `device`) and its id, so a message sealed
    /// for one credential never opens under another.
    pub fn new(
        side: Side,
        secret: &[u8; KEY_BYTES],
        client_nonce: &[u8; NONCE_BYTES],
        host_nonce: &[u8; NONCE_BYTES],
        context: &str,
    ) -> Result<Self, CryptoError> {
        let mut salt = [0u8; NONCE_BYTES * 2];
        salt[..NONCE_BYTES].copy_from_slice(client_nonce);
        salt[NONCE_BYTES..].copy_from_slice(host_nonce);
        let kdf = Hkdf::<Sha256>::new(Some(&salt), secret);
        let key = |info: &[u8]| -> Result<Aes256Gcm, CryptoError> {
            let mut bytes = Zeroizing::new([0u8; KEY_BYTES]);
            kdf.expand(info, bytes.as_mut())
                .map_err(|_| CryptoError::Seal)?;
            Aes256Gcm::new_from_slice(bytes.as_ref()).map_err(|_| CryptoError::Seal)
        };
        let (seal, open) = match side {
            Side::Host => (key(TO_CLIENT)?, key(TO_HOST)?),
            Side::Client => (key(TO_HOST)?, key(TO_CLIENT)?),
        };
        Ok(Self {
            seal,
            open,
            sent: 0,
            received: 0,
            context: format!("auracode remote v1|{context}").into_bytes(),
        })
    }

    /// Seals the next message this side sends.
    pub fn seal(&mut self, message: &[u8]) -> Result<String, CryptoError> {
        let sealed = self
            .seal
            .encrypt(
                &counter(self.sent),
                Payload {
                    msg: message,
                    aad: &self.context,
                },
            )
            .map_err(|_| CryptoError::Seal)?;
        // A counter never repeats under one key: a session ends long before 2^64 messages.
        self.sent = self.sent.checked_add(1).ok_or(CryptoError::Seal)?;
        Ok(encode(&sealed))
    }

    /// Opens the next message the other side sent. Anything else, in any other order, fails, and
    /// the session is over: the counter does not move on a failure.
    pub fn open(&mut self, sealed: &str) -> Result<Vec<u8>, CryptoError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(sealed)
            .map_err(|_| CryptoError::Open)?;
        let message = self
            .open
            .decrypt(
                &counter(self.received),
                Payload {
                    msg: &bytes,
                    aad: &self.context,
                },
            )
            .map_err(|_| CryptoError::Open)?;
        self.received = self.received.checked_add(1).ok_or(CryptoError::Open)?;
        Ok(message)
    }
}

/// The 96-bit nonce of a direction's `n`th message: the count, big-endian, after four zero bytes.
fn counter(n: u64) -> Nonce<aes_gcm::aes::cipher::typenum::U12> {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&n.to_be_bytes());
    Nonce::from(nonce)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(context: &str) -> (Channel, Channel) {
        let secret = [7u8; KEY_BYTES];
        let (cn, hn) = ([1u8; NONCE_BYTES], [2u8; NONCE_BYTES]);
        (
            Channel::new(Side::Host, &secret, &cn, &hn, context).expect("host"),
            Channel::new(Side::Client, &secret, &cn, &hn, context).expect("client"),
        )
    }

    #[test]
    fn each_side_opens_only_what_the_other_sealed_in_order() {
        let (mut host, mut client) = pair("device|d1");
        let first = client.seal(b"one").expect("seal");
        let second = client.seal(b"two").expect("seal");
        assert_ne!(first, second);
        // Out of order, replayed or echoed back to its sender: all refused, and nothing is lost.
        assert_eq!(host.open(&second), Err(CryptoError::Open));
        assert_eq!(client.open(&first), Err(CryptoError::Open));
        assert_eq!(host.open(&first).expect("open"), b"one");
        assert_eq!(host.open(&first), Err(CryptoError::Open));
        assert_eq!(host.open(&second).expect("open"), b"two");
        let reply = host.seal(b"three").expect("seal");
        assert_eq!(client.open(&reply).expect("open"), b"three");
    }

    #[test]
    fn a_changed_message_secret_nonce_or_context_does_not_open() {
        let (_, mut client) = pair("device|d1");
        let sealed = client.seal(b"secret words").expect("seal");
        let mut bytes = URL_SAFE_NO_PAD.decode(&sealed).expect("decode");
        bytes[0] ^= 1;
        let (mut host, _) = pair("device|d1");
        assert_eq!(host.open(&encode(&bytes)), Err(CryptoError::Open));
        assert_eq!(host.open("not base64!"), Err(CryptoError::Open));
        assert_eq!(host.open(""), Err(CryptoError::Open));
        // The untouched message still opens: a failure did not move the counter.
        assert_eq!(host.open(&sealed).expect("open"), b"secret words");

        let (mut other_context, _) = pair("device|d2");
        assert_eq!(other_context.open(&sealed), Err(CryptoError::Open));
        let (cn, hn) = ([1u8; NONCE_BYTES], [2u8; NONCE_BYTES]);
        let mut other_secret =
            Channel::new(Side::Host, &[8u8; KEY_BYTES], &cn, &hn, "device|d1").expect("host");
        assert_eq!(other_secret.open(&sealed), Err(CryptoError::Open));
        let mut other_nonce = Channel::new(
            Side::Host,
            &[7u8; KEY_BYTES],
            &cn,
            &[3u8; NONCE_BYTES],
            "device|d1",
        )
        .expect("host");
        assert_eq!(other_nonce.open(&sealed), Err(CryptoError::Open));
    }

    #[test]
    fn ids_and_keys_decode_only_in_their_exact_canonical_form() {
        let key: [u8; 32] = random().expect("random");
        let text = encode(&key);
        assert_eq!(text.len(), 43);
        assert_eq!(decode::<32>(&text), Some(key));
        assert_eq!(decode::<16>(&text), None);
        assert_eq!(decode::<32>(&text[..42]), None);
        assert_eq!(decode::<32>(&format!("{text}=")), None);
        assert_eq!(
            decode::<32>(&text.replace(|c: char| c.is_ascii(), "+")),
            None
        );
        // Two spellings of the same bytes: only the canonical one is an id.
        assert_eq!(decode::<1>("AA"), Some([0]));
        assert_eq!(decode::<1>("AB"), None);
        assert_ne!(
            random::<16>().expect("random"),
            random::<16>().expect("random")
        );
    }

    #[test]
    fn hashes_match_the_relays() {
        // `sha256("abc")`, base64url: what the relay computes for the same text.
        assert_eq!(hash("abc"), "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0");
        assert!(same(&hash("abc"), &hash("abc")));
        assert!(!same(&hash("abc"), &hash("abd")));
        assert!(!same("a", "ab"));
    }
}

/// The vectors both halves are held to (`tests/vectors.json`): the browser's tests read the same
/// file, so the two cannot drift apart.
#[cfg(test)]
mod vectors {
    use super::*;

    #[derive(serde::Deserialize, serde::Serialize)]
    struct Vectors {
        secret: String,
        client_nonce: String,
        host_nonce: String,
        context: String,
        /// Each message in plain words and as the named side seals it, in order.
        client_to_host: Vec<(String, String)>,
        host_to_client: Vec<(String, String)>,
    }

    const FILE: &str = include_str!("../tests/vectors.json");

    fn channels(vectors: &Vectors) -> (Channel, Channel) {
        let secret = decode::<KEY_BYTES>(&vectors.secret).expect("secret");
        let client = decode::<NONCE_BYTES>(&vectors.client_nonce).expect("client nonce");
        let host = decode::<NONCE_BYTES>(&vectors.host_nonce).expect("host nonce");
        (
            Channel::new(Side::Host, &secret, &client, &host, &vectors.context).expect("host"),
            Channel::new(Side::Client, &secret, &client, &host, &vectors.context).expect("client"),
        )
    }

    #[test]
    fn seals_and_opens_the_shared_vectors() {
        let vectors: Vectors = serde_json::from_str(FILE).expect("vectors");
        let (mut host, mut client) = channels(&vectors);
        assert!(vectors.client_to_host.len() >= 3 && vectors.host_to_client.len() >= 3);
        for (plain, sealed) in &vectors.client_to_host {
            assert_eq!(&client.seal(plain.as_bytes()).expect("seal"), sealed);
            assert_eq!(host.open(sealed).expect("open"), plain.as_bytes());
        }
        for (plain, sealed) in &vectors.host_to_client {
            assert_eq!(&host.seal(plain.as_bytes()).expect("seal"), sealed);
            assert_eq!(client.open(sealed).expect("open"), plain.as_bytes());
        }
    }

    /// Writes the vectors anew: `GHOST_WRITE_VECTORS=1 cargo test -p ghost-remote vectors`.
    #[test]
    fn writes_the_shared_vectors_when_asked() {
        if std::env::var_os("GHOST_WRITE_VECTORS").is_none() {
            return;
        }
        let mut vectors = Vectors {
            secret: encode(&[0x42; KEY_BYTES]),
            client_nonce: encode(&(0u8..16).collect::<Vec<_>>()),
            host_nonce: encode(&(16u8..32).collect::<Vec<_>>()),
            context: "device|AAECAwQFBgc".into(),
            client_to_host: Vec::new(),
            host_to_client: Vec::new(),
        };
        let (mut host, mut client) = channels(&vectors);
        for plain in ["", r#"{"id":1,"op":"overview"}"#, "Prüfe das, bitte ✓"] {
            let sealed = client.seal(plain.as_bytes()).expect("seal");
            vectors.client_to_host.push((plain.into(), sealed));
        }
        for plain in [
            r#"{"ev":"ready","host_name":"Studio","paired":true}"#,
            "",
            "日本語",
        ] {
            let sealed = host.seal(plain.as_bytes()).expect("seal");
            vectors.host_to_client.push((plain.into(), sealed));
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors.json");
        let json = serde_json::to_string_pretty(&vectors).expect("json");
        std::fs::write(path, format!("{json}\n")).expect("write");
    }
}
