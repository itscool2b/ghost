//! What travels between this desktop, the relay and a browser.
//!
//! The relay speaks one-letter text frames (`apps/relay/src/index.ts`): `o<cid>` a browser
//! connected, `c<cid>` it left (or, sent by the desktop, drop it), `m<cid> <payload>` its message,
//! `a<cid>` the desktop accepted it, `b<json>` a bot's tool call or its answer, `l<json>` the
//! desktop's bot links, `x` this socket was replaced.
//!
//! A browser's payload is one letter and a body: `H` its hello, `W` the desktop's welcome, `R` a
//! refusal, all three in the clear and carrying no secret; and `E`, a message sealed by
//! [`crate::crypto::Channel`]. Sealed messages are JSON: a request (`id`, `op`), its reply (`re`),
//! or an event (`ev`).

use o7_protocol::{
    EventBatch, UiAnswer,
    session::{ChatAttach, ChatStatusUpdate, HistoryPage, PromptAck},
    workspace::{ProjectSummary, WorkspaceChat},
};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
/// The length of the id the relay gives each browser connection.
pub const CID_LENGTH: usize = 16;

/// A frame from the relay.
#[derive(Debug, PartialEq, Eq)]
pub enum FromRelay<'a> {
    Opened(&'a str),
    Closed(&'a str),
    Message { cid: &'a str, payload: &'a str },
    Bot(&'a str),
    Replaced,
    Pong,
}

pub fn from_relay(frame: &str) -> Option<FromRelay<'_>> {
    if frame == "pong" {
        return Some(FromRelay::Pong);
    }
    if frame == "x" {
        return Some(FromRelay::Replaced);
    }
    let (kind, rest) = frame.split_at_checked(1)?;
    let cid = || {
        let cid = rest.get(..CID_LENGTH)?;
        cid.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            .then_some(cid)
    };
    match kind {
        "o" if rest.len() == CID_LENGTH => Some(FromRelay::Opened(cid()?)),
        "c" if rest.len() == CID_LENGTH => Some(FromRelay::Closed(cid()?)),
        "m" => {
            let payload = rest.get(CID_LENGTH..)?.strip_prefix(' ')?;
            Some(FromRelay::Message {
                cid: cid()?,
                payload,
            })
        }
        "b" => Some(FromRelay::Bot(rest)),
        _ => None,
    }
}

/// A browser's first message: which credential it holds and its half of the session nonce.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Hello {
    pub v: u32,
    pub kind: CredentialKind,
    pub id: String,
    pub nonce: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    /// A pairing link, good once.
    Pair,
    /// A paired device.
    Device,
}

impl CredentialKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pair => "pair",
            Self::Device => "device",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Welcome {
    pub v: u32,
    pub nonce: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The link was used, expired or replaced, or the device was removed.
    Unknown,
    /// This build does not speak the browser's version.
    Version,
    /// Too many browsers are connected.
    Busy,
    /// The device is paired with another workspace of this computer than the one open now.
    Elsewhere,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Refused {
    pub reason: Refusal,
}

/// A sealed request from a browser.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: u32,
    #[serde(flatten)]
    pub op: Op,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Turns a pairing link into a device. The only request a pairing session may make.
    Pair {
        name: String,
    },
    /// Projects, chats and agents.
    Overview,
    /// Follows one chat: its recent history now, then its live events. Replaces the chat this
    /// connection followed before.
    Open {
        chat_id: String,
    },
    /// Stops following.
    Close,
    /// The page of history before `before_seq`.
    History {
        chat_id: String,
        before_seq: u32,
    },
    Send {
        chat_id: String,
        text: String,
    },
    Interrupt {
        chat_id: String,
    },
    Permission {
        chat_id: String,
        request_id: String,
        option_id: Option<String>,
    },
    Answer {
        chat_id: String,
        request_id: String,
        answer: Option<UiAnswer>,
    },
    /// A new chat in a project folder, with its first message.
    Create {
        folder_id: String,
        agent_id: Option<String>,
        text: String,
    },
    /// The models and thinking levels a chat offers, and what it is set to.
    Options {
        chat_id: String,
    },
    SetModel {
        chat_id: String,
        model: String,
    },
    SetThinking {
        chat_id: String,
        level: String,
    },
    /// Hands the chat to another agent, as the app's own handoff does. Answers with the chat to
    /// open next.
    SwitchAgent {
        chat_id: String,
        agent_id: String,
    },
    /// This browser forgets the desktop: its device is removed.
    Forget,
}

/// What this build offers a paired browser beyond the first version's requests, named in
/// [`Event::Ready`] so a page shows only the controls this desktop answers: a request this build
/// does not know ends the session, as anything outside the protocol does. `pickers`: a chat's
/// model, thinking level and agent ([`Op::Options`] and the three requests after it).
pub const FEATURES: &[&str] = &["pickers"];

/// How the desktop looks, so a paired page wears the same theme: the theme's saved name (one of
/// `o7_core::onboarding::ThemePreference`'s, such as `claude` or `oh_my_pi`) and the
/// appearance (`system`, `light` or `dark`). A page checks both against the names it knows and
/// falls back to its own default for anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Look {
    pub theme: String,
    pub appearance: String,
}

/// What an [`Op`] returns.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Data {
    None {},
    Paired(Paired),
    Overview(Box<Overview>),
    Attach(Box<ChatAttach>),
    History(Box<HistoryPage>),
    Sent(PromptAck),
    Created {
        chat: Box<WorkspaceChat>,
        sent: PromptAck,
    },
    Options(Choices),
    /// The chat a handoff to another agent left the person with.
    Switched {
        chat_id: String,
    },
}

/// What a chat lets a person choose from a paired browser: its model and its thinking level. A
/// chat's access level is not offered there.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Choices {
    pub models: Vec<Choice>,
    pub model: Option<String>,
    pub thinking: Vec<Choice>,
    pub thinking_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct Paired {
    pub device_id: String,
    /// The device's key, sent once, inside the pairing session.
    pub device_key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub host_name: String,
    pub projects: Vec<ProjectSummary>,
    pub chats: Vec<WorkspaceChat>,
    pub agents: Vec<RemoteAgent>,
    /// The agent new chats start with when a project names none.
    pub default_agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteAgent {
    pub id: String,
    pub name: String,
    /// Installed and ready to start a chat.
    pub available: bool,
}

#[derive(Debug, Serialize)]
pub struct Reply<'a> {
    pub re: u32,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<&'a Data>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'a ErrorBody>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
}

/// What the desktop tells a browser without being asked.
#[derive(Debug, Serialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum Event<'a> {
    /// The first sealed message of a session: opening it proves the desktop holds the key.
    /// `features` is [`FEATURES`]; a desktop older than the list sends none.
    Ready {
        host_name: &'a str,
        paired: bool,
        features: &'a [&'a str],
        /// The desktop's theme and appearance; absent from older desktops and from a backend that
        /// does not say.
        #[serde(skip_serializing_if = "Option::is_none")]
        look: Option<&'a Look>,
    },
    /// A chat's status, title or attention changed.
    Chat { update: &'a ChatStatusUpdate },
    /// Live events of the chat this connection follows.
    Frame { batch: &'a EventBatch },
    /// Live events were dropped (the connection fell behind): open the chat again.
    Lost { chat_id: &'a str },
    /// Everything changed (another profile was opened): start over from the overview.
    Reset,
}

/// A bot's tool call, as the relay hands it over.
#[derive(Debug, Deserialize)]
pub struct BotCall {
    pub rid: String,
    pub bot: String,
    pub token: String,
    pub tool: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct BotAnswer<'a> {
    pub rid: &'a str,
    pub text: &'a str,
    pub error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<&'a crate::backend::Picture>,
}

#[derive(Debug, Serialize)]
pub struct BotLink<'a> {
    pub id: &'a str,
    pub hash: &'a str,
    /// Whether the relay may offer this link the full-control tools. The computer checks each
    /// call itself; this only keeps the list a bot is shown honest.
    pub full: bool,
    /// Whether the relay may offer this link only the tools that read (`readOnlyHint`). The
    /// computer checks each call itself.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read: bool,
    /// Which set of tools this copy of the app answers ([`crate::bot::TOOLS_VERSION`]), for the
    /// relay to list that one. A relay from before the number ignores it.
    pub v: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_frames_parse_strictly() {
        let cid = "abcDEF012345_-xy";
        assert_eq!(from_relay(&format!("o{cid}")), Some(FromRelay::Opened(cid)));
        assert_eq!(from_relay(&format!("c{cid}")), Some(FromRelay::Closed(cid)));
        assert_eq!(
            from_relay(&format!("m{cid} E a b")),
            Some(FromRelay::Message {
                cid,
                payload: "E a b"
            })
        );
        assert_eq!(
            from_relay(&format!("m{cid} ")),
            Some(FromRelay::Message { cid, payload: "" })
        );
        assert_eq!(from_relay("b{}"), Some(FromRelay::Bot("{}")));
        assert_eq!(from_relay("x"), Some(FromRelay::Replaced));
        assert_eq!(from_relay("pong"), Some(FromRelay::Pong));
        for bad in [
            "",
            "o",
            "oshort",
            &format!("o{cid}x"),
            &format!("m{cid}"),
            &format!("m{cid}E"),
            "o abcDEF012345_-x",
            "zzz",
            "é",
        ] {
            assert_eq!(from_relay(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn requests_name_one_operation_and_nothing_else() {
        let request: Request =
            serde_json::from_str(r#"{"id":7,"op":"send","chat_id":"c1","text":"Hi"}"#)
                .expect("send");
        assert_eq!(request.id, 7);
        assert_eq!(
            request.op,
            Op::Send {
                chat_id: "c1".into(),
                text: "Hi".into()
            }
        );
        let request: Request =
            serde_json::from_str(r#"{"id":1,"op":"permission","chat_id":"c","request_id":"r"}"#)
                .expect("declined permission");
        assert_eq!(
            request.op,
            Op::Permission {
                chat_id: "c".into(),
                request_id: "r".into(),
                option_id: None
            }
        );
        for bad in [
            r#"{"op":"overview"}"#,
            r#"{"id":1}"#,
            r#"{"id":1,"op":"shell","command":"rm"}"#,
            // A paired browser has no request for a chat's access level or for project memory.
            r#"{"id":1,"op":"set_access","chat_id":"c1","access":"full_auto"}"#,
            r#"{"id":1,"op":"write_memory","project_id":"p1","markdown":"x"}"#,
            r#"{"id":1,"op":"send","chat_id":"c1"}"#,
            r#"{"id":-1,"op":"overview"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn replies_and_events_have_the_shape_the_browser_reads() {
        let reply = Reply {
            re: 3,
            ok: true,
            data: Some(&Data::None {}),
            error: None,
        };
        assert_eq!(
            serde_json::to_string(&reply).expect("reply"),
            r#"{"re":3,"ok":true,"data":{}}"#
        );
        let error = ErrorBody {
            code: "not_found",
            message: "No such chat.".into(),
        };
        let reply = Reply {
            re: 4,
            ok: false,
            data: None,
            error: Some(&error),
        };
        assert_eq!(
            serde_json::to_string(&reply).expect("reply"),
            r#"{"re":4,"ok":false,"error":{"code":"not_found","message":"No such chat."}}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::Ready {
                host_name: "Studio",
                paired: true,
                features: FEATURES,
                look: None,
            })
            .expect("event"),
            r#"{"ev":"ready","host_name":"Studio","paired":true,"features":["pickers"]}"#
        );
        let look = Look {
            theme: "claude".into(),
            appearance: "system".into(),
        };
        assert_eq!(
            serde_json::to_string(&Event::Ready {
                host_name: "Studio",
                paired: true,
                features: FEATURES,
                look: Some(&look),
            })
            .expect("event"),
            r#"{"ev":"ready","host_name":"Studio","paired":true,"features":["pickers"],"look":{"theme":"claude","appearance":"system"}}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::Reset).expect("event"),
            r#"{"ev":"reset"}"#
        );
    }
}
