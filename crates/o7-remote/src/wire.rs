//! What travels between this desktop, the relay and a browser or o7 for iPhone.
//!
//! The relay speaks one-letter text frames (`apps/relay/src/index.ts`): `o<cid>` a browser
//! connected, `k<cid>` that connection came in on the iPhone app's path (sent right after its
//! `o`), `c<cid>` it left (or, sent by the desktop, drop it), `m<cid> <payload>` its message,
//! `a<cid>` the desktop accepted it, `b<json>` a bot's tool call or its answer, `l<json>` the
//! desktop's bot links, `x` this socket was replaced.
//!
//! A browser's payload is one letter and a body: `H` its hello, `W` the desktop's welcome, `R` a
//! refusal, all three in the clear and carrying no secret; and `E`, a message sealed by
//! [`crate::crypto::Channel`]. Sealed messages are JSON: a request (`id`, `op`), its reply (`re`),
//! or an event (`ev`). The iPhone app speaks the same, and is offered more ([`APP_FEATURES`]).

use o7_protocol::{
    EventBatch, PermissionOption, UiAnswer, UiPrompt,
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
    /// The connection just opened came in on the iPhone app's path, which no page can open.
    Native(&'a str),
    Closed(&'a str),
    Message {
        cid: &'a str,
        payload: &'a str,
    },
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
        "k" if rest.len() == CID_LENGTH => Some(FromRelay::Native(cid()?)),
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
    /// `client` is what the device says it is (o7 for iPhone names itself); a browser says
    /// nothing.
    Pair {
        name: String,
        #[serde(default)]
        client: Option<Client>,
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
    /// What needs the person across the open chats, in one reading: the requests chats wait
    /// on, the chats working and the chats that finished in the last day ([`Inbox`]).
    Inbox,
    /// The chats whose title, project, agent or messages match `query` (1 to
    /// [`MAX_QUERY`] characters), at most `limit` of them ([`MAX_HITS`]).
    Search {
        query: String,
        limit: Option<u32>,
    },
}

/// What a device says about itself when it pairs. It is only its word: the computer records o7
/// for iPhone only from a connection the relay marked as the app's ([`FromRelay::Native`]).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Client {
    /// `ios` from o7 for iPhone.
    #[serde(default)]
    pub kind: Option<String>,
    /// The app's version, kept with the device (at most
    /// [`crate::registry::MAX_CLIENT_FIELD`] characters).
    #[serde(default)]
    pub version: Option<String>,
    /// The device's model, such as `iPhone17,1`. Read, and not kept yet.
    #[serde(default)]
    pub model: Option<String>,
}

impl Client {
    /// Whether it says it is o7 for iPhone.
    pub fn ios(&self) -> bool {
        self.kind.as_deref() == Some("ios")
    }
}

/// A chat's model, thinking level and agent: [`Op::Options`] and the three requests after it.
pub const PICKERS: &str = "pickers";
/// [`Op::Inbox`].
pub const INBOX: &str = "inbox";
/// [`Op::Search`].
pub const SEARCH: &str = "search";

/// What this build offers a paired browser beyond the first version's requests, named in
/// [`Event::Ready`] so a page shows only the controls this desktop answers. A request the build
/// does not know ends the session, as anything outside the protocol does; one it knows but did
/// not offer is answered `not_offered`. A browser's list stays as it is, so what it is sent
/// does not change.
pub const FEATURES: &[&str] = &[PICKERS];
/// What o7 for iPhone is offered: a session on the app's path (the relay's `k`) with a pairing
/// link, or with a device recorded as the app. Each later part of the app adds its name here and
/// the requests that need it to [`Op::feature`].
pub const APP_FEATURES: &[&str] = &[PICKERS, INBOX, SEARCH];

/// The longest search, in characters, and the most hits one returns.
pub const MAX_QUERY: usize = 200;
pub const MAX_HITS: usize = 50;
/// How many hits a search returns when it names no limit.
pub const DEFAULT_HITS: usize = 20;

impl Op {
    /// The feature a request needs, or `None` for the first version's requests, which every
    /// session may make. A session is answered `not_offered` for a feature it was not offered.
    pub fn feature(&self) -> Option<&'static str> {
        match self {
            Self::Options { .. }
            | Self::SetModel { .. }
            | Self::SetThinking { .. }
            | Self::SwitchAgent { .. } => Some(PICKERS),
            Self::Inbox => Some(INBOX),
            Self::Search { .. } => Some(SEARCH),
            Self::Pair { .. }
            | Self::Overview
            | Self::Open { .. }
            | Self::Close
            | Self::History { .. }
            | Self::Send { .. }
            | Self::Interrupt { .. }
            | Self::Permission { .. }
            | Self::Answer { .. }
            | Self::Create { .. }
            | Self::Forget => None,
        }
    }
}

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
    Inbox(Box<Inbox>),
    Search(SearchHits),
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
    /// What o7 for iPhone is offered from now on, as its next `ready` names it. Only for the
    /// app: a browser's answer is as it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<&'static [&'static str]>,
}

/// What needs the person across the open chats ([`Op::Inbox`]), at most [`MAX_INBOX`] of each.
/// It reads the overview and the newest page of history of each chat it lists.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Inbox {
    /// The requests chats wait on, one entry each.
    pub asking: Vec<Asking>,
    pub working: Vec<Working>,
    /// Chats whose last turn ended in the last day, newest first.
    pub recent: Vec<Recent>,
}

/// The most entries of each part of [`Inbox`].
pub const MAX_INBOX: usize = 40;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Asking {
    pub chat_id: String,
    /// What it asks. None when the chat waits on something its newest history does not show
    /// yet: the chat itself says.
    pub request: Option<Asked>,
}

/// A request a chat waits on, as `permission` or `answer` answers it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Asked {
    pub id: String,
    /// `permission` or `question`.
    pub kind: &'static str,
    /// What it asks, as the agent wrote it: a permission's title, a question's message (a
    /// grouped prompt's first question).
    pub title: String,
    /// A permission's options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<PermissionOption>>,
    /// A question's prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<UiPrompt>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Working {
    pub chat_id: String,
    /// What it is doing, in one line: its latest step's title, else how the agent's last
    /// message begins, else its status.
    pub line: String,
    /// When its turn started, in Unix milliseconds.
    pub since: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Recent {
    pub chat_id: String,
    /// When its last turn ended, in Unix milliseconds.
    pub ended_at: i64,
    pub duration_ms: Option<u64>,
    /// What the turn came to, in one line: how it ended, a command still failing, how the
    /// agent's last message begins.
    pub line: String,
}

/// What [`Op::Search`] found, best first.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SearchHits {
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchHit {
    pub chat_id: String,
    pub title: String,
    /// The words around a match in the chat's messages; None when only its title, project or
    /// agent matched.
    pub snippet: Option<String>,
    /// When the chat last changed, in Unix milliseconds.
    pub at: i64,
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
    /// `features` is [`FEATURES`], or [`APP_FEATURES`] for o7 for iPhone; a desktop older than
    /// the list sends none.
    Ready {
        host_name: &'a str,
        paired: bool,
        features: &'a [&'a str],
        /// The desktop's theme and appearance; absent from older desktops and from a backend that
        /// does not say.
        #[serde(skip_serializing_if = "Option::is_none")]
        look: Option<&'a Look>,
        /// The website this computer pairs through (`https://o7.codes`), so the iPhone app
        /// follows it when it moves. Only on the app's path; a browser is on the site already.
        #[serde(skip_serializing_if = "Option::is_none")]
        site: Option<&'a str>,
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
    /// An automation: its address takes what is sent to it (`/hook/`) and nothing else, and it
    /// is no MCP server. The computer refuses anything but a trigger from it all the same.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hook: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_frames_parse_strictly() {
        let cid = "abcDEF012345_-xy";
        assert_eq!(from_relay(&format!("o{cid}")), Some(FromRelay::Opened(cid)));
        assert_eq!(from_relay(&format!("k{cid}")), Some(FromRelay::Native(cid)));
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
            // The app's mark takes exactly one connection id, as `o` does.
            "k",
            "kshort",
            &format!("k{cid}x"),
            "k abcDEF012345_-x",
            "kabcDEF012345_-x!",
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
        // A browser's pairing says nothing about itself; o7 for iPhone names itself.
        let op = |text: &str| serde_json::from_str::<Request>(text).expect(text).op;
        assert_eq!(
            op(r#"{"id":1,"op":"pair","name":"Phone"}"#),
            Op::Pair {
                name: "Phone".into(),
                client: None
            }
        );
        assert_eq!(
            op(
                r#"{"id":1,"op":"pair","name":"","client":{"kind":"ios","version":"1.0","model":"iPhone17,1"}}"#
            ),
            Op::Pair {
                name: String::new(),
                client: Some(Client {
                    kind: Some("ios".into()),
                    version: Some("1.0".into()),
                    model: Some("iPhone17,1".into()),
                })
            }
        );
        assert_eq!(op(r#"{"id":2,"op":"inbox"}"#), Op::Inbox);
        assert_eq!(
            op(r#"{"id":3,"op":"search","query":"login"}"#),
            Op::Search {
                query: "login".into(),
                limit: None
            }
        );
        assert_eq!(
            op(r#"{"id":4,"op":"search","query":"login","limit":5}"#),
            Op::Search {
                query: "login".into(),
                limit: Some(5)
            }
        );
        // Whether a session was offered a request is the computer's to say: a browser asking
        // for one it was not offered is answered `not_offered` (tests/host.rs).
        for bad in [
            r#"{"op":"overview"}"#,
            r#"{"id":1}"#,
            r#"{"id":1,"op":"shell","command":"rm"}"#,
            r#"{"id":1,"op":"send","chat_id":"c1"}"#,
            r#"{"id":-1,"op":"overview"}"#,
            r#"{"id":1,"op":"search"}"#,
            r#"{"id":1,"op":"search","query":"x","limit":-1}"#,
            r#"{"id":1,"op":"pair","name":"A","client":{"kind":7}}"#,
        ] {
            assert!(serde_json::from_str::<Request>(bad).is_err(), "{bad}");
        }
    }

    /// Every request names the feature it needs; a browser is offered what it always was, and
    /// the iPhone app everything a browser is and more.
    #[test]
    fn each_request_belongs_to_a_feature_offered_by_name() {
        assert_eq!(FEATURES, ["pickers"]);
        assert_eq!(APP_FEATURES, ["pickers", "inbox", "search"]);
        assert!(FEATURES.iter().all(|name| APP_FEATURES.contains(name)));
        let op = |text: &str| serde_json::from_str::<Request>(text).expect(text).op;
        for (request, feature) in [
            (r#"{"id":1,"op":"pair","name":"A"}"#, None),
            (r#"{"id":1,"op":"overview"}"#, None),
            (r#"{"id":1,"op":"open","chat_id":"c"}"#, None),
            (r#"{"id":1,"op":"close"}"#, None),
            (
                r#"{"id":1,"op":"history","chat_id":"c","before_seq":1}"#,
                None,
            ),
            (r#"{"id":1,"op":"send","chat_id":"c","text":"x"}"#, None),
            (r#"{"id":1,"op":"interrupt","chat_id":"c"}"#, None),
            (
                r#"{"id":1,"op":"permission","chat_id":"c","request_id":"r"}"#,
                None,
            ),
            (
                r#"{"id":1,"op":"answer","chat_id":"c","request_id":"r"}"#,
                None,
            ),
            (r#"{"id":1,"op":"create","folder_id":"f","text":"x"}"#, None),
            (r#"{"id":1,"op":"forget"}"#, None),
            (r#"{"id":1,"op":"options","chat_id":"c"}"#, Some("pickers")),
            (
                r#"{"id":1,"op":"set_model","chat_id":"c","model":"m"}"#,
                Some("pickers"),
            ),
            (
                r#"{"id":1,"op":"set_thinking","chat_id":"c","level":"l"}"#,
                Some("pickers"),
            ),
            (
                r#"{"id":1,"op":"switch_agent","chat_id":"c","agent_id":"a"}"#,
                Some("pickers"),
            ),
            (r#"{"id":1,"op":"inbox"}"#, Some("inbox")),
            (r#"{"id":1,"op":"search","query":"x"}"#, Some("search")),
        ] {
            let feature_of = op(request).feature();
            assert_eq!(feature_of, feature, "{request}");
            // Whatever a request needs, the app is offered.
            assert!(
                feature_of.is_none_or(|name| APP_FEATURES.contains(&name)),
                "{request}"
            );
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
        // A browser's `ready` is what it was before the iPhone app.
        assert_eq!(
            serde_json::to_string(&Event::Ready {
                host_name: "Studio",
                paired: true,
                features: FEATURES,
                look: None,
                site: None,
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
                site: None,
            })
            .expect("event"),
            r#"{"ev":"ready","host_name":"Studio","paired":true,"features":["pickers"],"look":{"theme":"claude","appearance":"system"}}"#
        );
        // The app's names its features and the site.
        assert_eq!(
            serde_json::to_string(&Event::Ready {
                host_name: "Studio",
                paired: true,
                features: APP_FEATURES,
                look: Some(&look),
                site: Some("https://o7.codes"),
            })
            .expect("event"),
            r#"{"ev":"ready","host_name":"Studio","paired":true,"features":["pickers","inbox","search"],"look":{"theme":"claude","appearance":"system"},"site":"https://o7.codes"}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::Reset).expect("event"),
            r#"{"ev":"reset"}"#
        );
        // A browser's pairing answer and its pickers are what they were; the app's pairing
        // answer adds its features.
        let paired = |features| {
            serde_json::to_string(&Data::Paired(Paired {
                device_id: "d1".into(),
                device_key: "k1".into(),
                features,
            }))
            .expect("paired")
        };
        assert_eq!(paired(None), r#"{"device_id":"d1","device_key":"k1"}"#);
        assert_eq!(
            paired(Some(APP_FEATURES)),
            r#"{"device_id":"d1","device_key":"k1","features":["pickers","inbox","search"]}"#
        );
        assert_eq!(
            serde_json::to_string(&Data::Options(Choices {
                models: vec![Choice {
                    value: "opus".into(),
                    name: "Opus 5.5".into(),
                }],
                model: Some("opus".into()),
                thinking: Vec::new(),
                thinking_level: None,
            }))
            .expect("choices"),
            r#"{"models":[{"value":"opus","name":"Opus 5.5"}],"model":"opus","thinking":[],"thinking_level":null}"#
        );
    }

    /// What the iPhone app's Inbox and Search read, field for field.
    #[test]
    fn the_inbox_and_search_have_the_shape_the_app_reads() {
        use o7_protocol::PermissionOptionKind;
        let inbox = Inbox {
            asking: vec![
                Asking {
                    chat_id: "c1".into(),
                    request: Some(Asked {
                        id: "r1".into(),
                        kind: "permission",
                        title: "Run `npm test`".into(),
                        options: Some(vec![PermissionOption {
                            option_id: "allow".into(),
                            kind: PermissionOptionKind::AllowOnce,
                            label: "Allow once".into(),
                        }]),
                        prompt: None,
                    }),
                },
                Asking {
                    chat_id: "c2".into(),
                    request: Some(Asked {
                        id: "q1".into(),
                        kind: "question",
                        title: "Deploy now?".into(),
                        options: None,
                        prompt: Some(UiPrompt::Confirm {
                            message: "Deploy now?".into(),
                        }),
                    }),
                },
                Asking {
                    chat_id: "c3".into(),
                    request: None,
                },
            ],
            working: vec![Working {
                chat_id: "c4".into(),
                line: "Running cargo test".into(),
                since: 1_000,
            }],
            recent: vec![Recent {
                chat_id: "c5".into(),
                ended_at: 2_000,
                duration_ms: Some(900),
                line: "finished. Done.".into(),
            }],
        };
        assert_eq!(
            serde_json::to_value(Data::Inbox(Box::new(inbox))).expect("inbox"),
            serde_json::json!({
                "asking": [
                    { "chat_id": "c1", "request": {
                        "id": "r1", "kind": "permission", "title": "Run `npm test`",
                        "options": [{ "option_id": "allow", "kind": "allow_once", "label": "Allow once" }],
                    } },
                    { "chat_id": "c2", "request": {
                        "id": "q1", "kind": "question", "title": "Deploy now?",
                        "prompt": { "type": "confirm", "message": "Deploy now?" },
                    } },
                    { "chat_id": "c3", "request": null },
                ],
                "working": [{ "chat_id": "c4", "line": "Running cargo test", "since": 1000 }],
                "recent": [{ "chat_id": "c5", "ended_at": 2000, "duration_ms": 900, "line": "finished. Done." }],
            })
        );
        assert_eq!(
            serde_json::to_value(Data::Search(SearchHits {
                hits: vec![
                    SearchHit {
                        chat_id: "c1".into(),
                        title: "Fix the login form".into(),
                        snippet: Some("…the login form…".into()),
                        at: 5,
                    },
                    SearchHit {
                        chat_id: "c2".into(),
                        title: "Login".into(),
                        snippet: None,
                        at: 4,
                    },
                ],
            }))
            .expect("hits"),
            serde_json::json!({ "hits": [
                { "chat_id": "c1", "title": "Fix the login form", "snippet": "…the login form…", "at": 5 },
                { "chat_id": "c2", "title": "Login", "snippet": null, "at": 4 },
            ] })
        );
    }
}
