//! Sending a bot link's call-back (`ghost_remote::callback`): the message that tells a linked
//! assistant a chat it acted on has finished, failed or asks for an answer.
//!
//! `ghost-remote` decides when and what; this sends it, in the form the assistant's own way of
//! being woken takes, with the key that address needs. The key is kept in the computer's secure
//! storage, one entry per bot link, and is read only here, at the moment of sending. Neither the
//! key nor the address is ever written to a log: an address's path is itself a secret for some
//! assistants.
//!
//! Who may do what: only the window sets, tests or clears a link's address (the commands in
//! `remote.rs`); a bot has no tool that reads or changes it. Ghost sends only to the address its
//! owner typed, follows no redirect from it, and sends the text `ghost-remote` wrote and nothing
//! else from the computer.

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ghost_remote::{CallbackKind, Delivery};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::agent_tokens::{NativeTokens, TokenEntry, TokenError};

/// The longest key accepted, in bytes.
pub const MAX_KEY: usize = 1024;
/// How long one attempt may take.
const ATTEMPT: Duration = Duration::from_secs(10);
/// A failed attempt is made once more, after this long.
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// A bot link's entry in secure storage. Its name is a digest: it names no link or assistant.
fn entry(bot_id: &str) -> Result<TokenEntry, TokenError> {
    TokenEntry::new(&format!("remote-callback:{bot_id}"))
}

/// Keeps `key` for the bot link `bot_id`. Call on a blocking worker.
pub fn save_key(bot_id: &str, key: &str) -> Result<(), TokenError> {
    let key = key.trim();
    if key.is_empty() || key.len() > MAX_KEY || key.chars().any(char::is_control) {
        return Err(TokenError::Invalid);
    }
    NativeTokens::new()?.save(&entry(bot_id)?, key.as_bytes())
}

/// Forgets the bot link's key, if it has one. Call on a blocking worker.
pub fn remove_key(bot_id: &str) -> Result<(), TokenError> {
    NativeTokens::new()?.remove(&entry(bot_id)?)
}

/// Whether the bot link has a key kept. Call on a blocking worker.
pub fn has_key(bot_id: &str) -> bool {
    load_key(bot_id).is_ok_and(|key| key.is_some())
}

fn load_key(bot_id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, TokenError> {
    NativeTokens::new()?.load(&entry(bot_id)?)
}

/// One request, ready to send.
#[derive(Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

/// HMAC-SHA256 (RFC 2104), with the hash this crate already has.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = Zeroizing::new([0u8; BLOCK]);
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    let mut outer = Sha256::new();
    let mut pad = Zeroizing::new([0u8; BLOCK]);
    for (place, byte) in pad.iter_mut().zip(block.iter()) {
        *place = byte ^ 0x36;
    }
    inner.update(&pad[..]);
    inner.update(message);
    for (place, byte) in pad.iter_mut().zip(block.iter()) {
        *place = byte ^ 0x5c;
    }
    outer.update(&pad[..]);
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The request that carries `delivery`, signed or authorized with `key`, at `now` (Unix
/// seconds). Each assistant is spoken to as its own documentation says (read 23 November 2026):
///
/// - Grok Bot, a routine's webhook: `Authorization: Bearer <key>` and a JSON body, which the
///   Bot is given with the routine's instruction.
/// - Hermes, a webhook route: `X-Webhook-Timestamp` and `X-Webhook-Signature-V2`, the hex
///   HMAC-SHA256 of `<timestamp>.<body>` under the route's secret.
/// - OpenClaw, `POST /hooks/agent`: `Authorization: Bearer <token>`, `message` as the agent's
///   input (OpenClaw wraps it as outside content), and where to deliver its answer.
/// - Anything else, Standard Webhooks: `webhook-id`, `webhook-timestamp` and
///   `webhook-signature: v1,<base64 HMAC-SHA256 of id.timestamp.body>`.
pub fn request(delivery: &Delivery, key: &[u8], now: i64) -> Outgoing {
    let callback = &delivery.callback;
    let event = serde_json::json!({
        "source": "ghost",
        "event": delivery.event.word(),
        "id": delivery.id,
        "chat": delivery.chat_id,
        "title": delivery.title,
        "message": delivery.text,
        "at": delivery.at,
    });
    let json = ("Content-Type", "application/json".to_owned());
    let bearer = || {
        (
            "Authorization",
            format!("Bearer {}", String::from_utf8_lossy(key)),
        )
    };
    match callback.kind {
        CallbackKind::Grok => Outgoing {
            url: callback.url.clone(),
            headers: vec![json, bearer()],
            body: event.to_string(),
        },
        CallbackKind::Hermes => {
            let body = event.to_string();
            let signature = hmac_sha256(key, format!("{now}.{body}").as_bytes());
            Outgoing {
                url: callback.url.clone(),
                headers: vec![
                    json,
                    ("X-Webhook-Timestamp", now.to_string()),
                    ("X-Webhook-Signature-V2", hex(&signature)),
                ],
                body,
            }
        }
        CallbackKind::Openclaw => {
            let mut body = serde_json::json!({
                "message": delivery.text,
                "name": "Ghost",
                "deliver": true,
            });
            if let (Some(channel), Some(to)) = (&callback.channel, &callback.to) {
                body["channel"] = channel.clone().into();
                body["to"] = to.clone().into();
            }
            Outgoing {
                url: callback.url.clone(),
                headers: vec![json, bearer(), ("Idempotency-Key", delivery.id.clone())],
                body: body.to_string(),
            }
        }
        CallbackKind::Webhook => {
            let body = event.to_string();
            // A Standard Webhooks secret is base64 after `whsec_`; any other key is used as typed.
            let text = String::from_utf8_lossy(key);
            let secret = text
                .strip_prefix("whsec_")
                .and_then(|encoded| STANDARD.decode(encoded).ok())
                .map(Zeroizing::new);
            let signature = hmac_sha256(
                secret.as_deref().map_or(key, Vec::as_slice),
                format!("{}.{now}.{body}", delivery.id).as_bytes(),
            );
            Outgoing {
                url: callback.url.clone(),
                headers: vec![
                    json,
                    ("webhook-id", delivery.id.clone()),
                    ("webhook-timestamp", now.to_string()),
                    (
                        "webhook-signature",
                        format!("v1,{}", STANDARD.encode(signature)),
                    ),
                ],
                body,
            }
        }
    }
}

/// How one delivery ended, in words Settings shows. They never carry the address or the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    pub ok: bool,
    pub detail: String,
}

fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        // The key goes to the address its owner typed and nowhere that address points on to.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(ATTEMPT)
        .user_agent("ghost")
        .build()
        .ok()
}

async fn attempt(client: &reqwest::Client, outgoing: &Outgoing) -> Told {
    let mut request = client.post(&outgoing.url).body(outgoing.body.clone());
    for (name, value) in &outgoing.headers {
        request = request.header(*name, value);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => Told {
            ok: true,
            detail: "Delivered.".into(),
        },
        Ok(response) => {
            let status = response.status();
            Told {
                ok: false,
                detail: match status.as_u16() {
                    401 | 403 => "The assistant refused the key.".into(),
                    404 => "The assistant has nothing at that address (or refused the signature)."
                        .into(),
                    429 => "The assistant is taking no more for now.".into(),
                    300..=399 => {
                        "The address answered with a redirect, which Ghost does not follow.".into()
                    }
                    code => format!("The assistant answered {code}."),
                },
            }
        }
        Err(error) if error.is_timeout() => Told {
            ok: false,
            detail: "The assistant did not answer in time.".into(),
        },
        Err(error) if error.is_connect() => Told {
            ok: false,
            detail: "Ghost could not connect to that address.".into(),
        },
        Err(_) => Told {
            ok: false,
            detail: "The message could not be sent.".into(),
        },
    }
}

/// Whether trying once more could help: the assistant was busy or unreachable, not unwilling.
fn worth_retrying(told: &Told) -> bool {
    !told.ok
        && !told.detail.contains("refused")
        && !told.detail.contains("nothing at that address")
        && !told.detail.contains("redirect")
}

/// Sends `delivery` and says how it went. The key is read from secure storage here and nowhere
/// kept; a link whose key is gone is told nothing.
pub async fn send(delivery: Delivery) -> Told {
    let bot = delivery.bot_id.clone();
    let key = match tokio::task::spawn_blocking(move || load_key(&bot)).await {
        Ok(Ok(Some(key))) => key,
        Ok(Ok(None)) => {
            return Told {
                ok: false,
                detail: "Its key is no longer in this computer's secure storage. Set it again."
                    .into(),
            };
        }
        _ => {
            return Told {
                ok: false,
                detail: "Secure storage is unavailable or locked.".into(),
            };
        }
    };
    let Some(client) = client() else {
        return Told {
            ok: false,
            detail: "The message could not be sent.".into(),
        };
    };
    let seconds = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
            })
    };
    let first = attempt(&client, &request(&delivery, &key, seconds())).await;
    if !worth_retrying(&first) {
        return first;
    }
    tokio::time::sleep(RETRY_AFTER).await;
    // Signed again: a signature carries its time.
    attempt(&client, &request(&delivery, &key, seconds())).await
}

/// The entry names of every key this module may have kept for `bot_ids`, for removal with the
/// app's data.
pub fn remove_keys(bot_ids: &[String]) -> usize {
    let Ok(store) = NativeTokens::new() else {
        return 0;
    };
    bot_ids
        .iter()
        .filter(|id| entry(id).and_then(|entry| store.remove(&entry)).is_err())
        .count()
}

#[cfg(test)]
mod tests {
    use ghost_remote::{Callback, CallbackEvent};

    use super::*;

    fn delivery(kind: CallbackKind) -> Delivery {
        Delivery {
            bot_id: "bot1".into(),
            bot_name: "Grok Bot".into(),
            callback: Callback {
                kind,
                url: "https://assistant.example/hook/abc".into(),
                channel: None,
                to: None,
            },
            event: CallbackEvent::Finished,
            id: "evt_1".into(),
            chat_id: "c7".into(),
            title: "Fix the login form".into(),
            text: "Ghost: chat c7 \"Fix the login form\" (web, Claude Code) has finished.".into(),
            at: 1_700_000_000_000,
        }
    }

    fn header<'a>(outgoing: &'a Outgoing, name: &str) -> &'a str {
        outgoing
            .headers
            .iter()
            .find(|(header, _)| *header == name)
            .map_or("", |(_, value)| value.as_str())
    }

    #[test]
    fn hmac_matches_the_rfc_4231_vectors() {
        // Test case 1 and test case 2 of RFC 4231, and a key longer than a block (case 6).
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn each_assistant_is_spoken_to_its_own_way() {
        let key = b"s3cret-key";
        let grok = request(&delivery(CallbackKind::Grok), key, 1_700_000_000);
        assert_eq!(grok.url, "https://assistant.example/hook/abc");
        assert_eq!(header(&grok, "Authorization"), "Bearer s3cret-key");
        let body: serde_json::Value = serde_json::from_str(&grok.body).expect("json");
        assert_eq!(body["source"], "ghost");
        assert_eq!(body["event"], "finished");
        assert_eq!(body["chat"], "c7");
        assert!(
            body["message"]
                .as_str()
                .expect("message")
                .starts_with("Ghost: chat c7")
        );
        assert!(!grok.body.contains("s3cret"), "the key is never in a body");

        let hermes = request(&delivery(CallbackKind::Hermes), key, 1_700_000_000);
        assert_eq!(header(&hermes, "X-Webhook-Timestamp"), "1700000000");
        assert_eq!(
            header(&hermes, "X-Webhook-Signature-V2"),
            hex(&hmac_sha256(
                key,
                format!("1700000000.{}", hermes.body).as_bytes()
            ))
        );
        assert_eq!(header(&hermes, "Authorization"), "", "signed, not carried");

        let mut to_chat = delivery(CallbackKind::Openclaw);
        let plain = request(&to_chat, key, 1);
        let body: serde_json::Value = serde_json::from_str(&plain.body).expect("json");
        assert_eq!(body["name"], "Ghost");
        assert_eq!(body["deliver"], true);
        assert!(body.get("channel").is_none() && body.get("to").is_none());
        assert_eq!(header(&plain, "Idempotency-Key"), "evt_1");
        assert_eq!(header(&plain, "Authorization"), "Bearer s3cret-key");
        to_chat.callback.channel = Some("telegram".into());
        to_chat.callback.to = Some("12345".into());
        let body: serde_json::Value =
            serde_json::from_str(&request(&to_chat, key, 1).body).expect("json");
        assert_eq!(
            (body["channel"].as_str(), body["to"].as_str()),
            (Some("telegram"), Some("12345"))
        );

        let webhook = request(&delivery(CallbackKind::Webhook), key, 1_700_000_000);
        assert_eq!(header(&webhook, "webhook-id"), "evt_1");
        assert_eq!(header(&webhook, "webhook-timestamp"), "1700000000");
        let expected = STANDARD.encode(hmac_sha256(
            key,
            format!("evt_1.1700000000.{}", webhook.body).as_bytes(),
        ));
        assert_eq!(
            header(&webhook, "webhook-signature"),
            format!("v1,{expected}")
        );
        // A Standard Webhooks secret is the bytes its base64 names.
        let secret = format!("whsec_{}", STANDARD.encode(b"raw-secret-bytes"));
        let signed = request(&delivery(CallbackKind::Webhook), secret.as_bytes(), 5);
        let expected = STANDARD.encode(hmac_sha256(
            b"raw-secret-bytes",
            format!("evt_1.5.{}", signed.body).as_bytes(),
        ));
        assert_eq!(
            header(&signed, "webhook-signature"),
            format!("v1,{expected}")
        );
    }

    #[test]
    fn a_refusal_is_not_tried_again_and_a_busy_assistant_is() {
        let told = |detail: &str| Told {
            ok: false,
            detail: detail.into(),
        };
        assert!(!worth_retrying(&told("The assistant refused the key.")));
        assert!(!worth_retrying(&told(
            "The assistant has nothing at that address (or refused the signature)."
        )));
        assert!(!worth_retrying(&told(
            "The address answered with a redirect, which Ghost does not follow."
        )));
        assert!(worth_retrying(&told("The assistant answered 503.")));
        assert!(worth_retrying(&told(
            "The assistant did not answer in time."
        )));
        assert!(!worth_retrying(&Told {
            ok: true,
            detail: "Delivered.".into()
        }));
    }

    #[test]
    fn a_links_entry_names_nothing() {
        let name = entry("bot1").expect("entry").target();
        assert!(
            !name.contains("bot1") && !name.contains("callback"),
            "{name}"
        );
        assert_ne!(name, entry("bot2").expect("entry").target());
    }
}
