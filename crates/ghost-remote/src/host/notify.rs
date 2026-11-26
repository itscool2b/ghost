//! Telling Ghost for iPhone what needs it, through the relay and Apple. [`crate::push`] seals what
//! a notification says; this decides who hears what, and sends it.
//!
//! A device with notifications on hears, as it chose: a chat that starts waiting on the person,
//! a chat whose turn finished or failed, and of its own tasks, a review that is in, work queued
//! for tonight that could not start, and news on a pull request. A request answered elsewhere is
//! taken back quietly (`resolved`). A device with a Live Activity is sent its counts as they
//! change. The service always sends; the phone hides what is about the chat on its screen.
//!
//! Frames go out only on a socket whose relay said it sends notifications (`f`): an older relay
//! ends the socket on a frame it does not know, every browser with it. Until then, and while
//! there is no socket, up to 32 wait for two minutes. The relay answers each one (`P`), on
//! whichever socket is open by then, so what was sent is kept here, not with the socket.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Instant,
};

use tokio::sync::mpsc::error::SendError;

use super::*;
use crate::{
    push::{self, Kind, Payload, Scope},
    registry::{ActivityToken, PushTarget},
    render::{self, Waiting},
    wire::{Overview, PushAnswer, PushFrame},
};

/// How many frames wait for a socket that sends notifications, and for how long.
const WAITING: usize = 32;
const WAIT: Duration = Duration::from_secs(120);
/// How long the relay's answer to a frame is waited for, and the most frames waiting for one.
const ANSWER_WITHIN: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING: usize = 256;
/// How long Apple keeps trying an alert, and one that asks: what a chat asks is answered soon or
/// not at all.
const TTL: u32 = 3_600;
const TTL_ASKING: u32 = 900;
/// How long Apple keeps trying a Live Activity's update.
const TTL_ACTIVITY: u32 = 3_600;
/// One test per device in this long.
const TEST_EVERY: Duration = Duration::from_secs(10);
/// At most one Live Activity update per device in this long, unless more chats need the person.
const ACTIVITY_EVERY: Duration = Duration::from_secs(10);
/// A Live Activity ends once nothing worked or waited on the person for this long.
const ACTIVITY_IDLE: Duration = Duration::from_secs(5 * 60);
/// How long a phone's own answer to what a chat asked is remembered.
const ANSWERED_FOR_MS: i64 = 10 * 60_000;
/// The longest line, title, project and agent name a notification carries, in characters.
const LINE: usize = 400;
const NAME: usize = 80;

/// What notifications stand on, for the whole service: the socket they go out on, what waits
/// and what was sent, and what each device was sent.
#[derive(Default)]
pub(super) struct State {
    /// The current socket's way out, while there is one.
    out: Option<UnboundedSender<Out>>,
    /// Its relay said it sends notifications.
    relay: bool,
    /// Frames waiting for a socket that sends them, oldest first.
    waiting: VecDeque<(Instant, Sending)>,
    /// Frames written and not yet answered, by their id.
    pending: HashMap<String, Sent>,
    /// What each device was sent, for its limits.
    limits: HashMap<String, push::Limiter>,
    /// The chats each device has a notification standing for that asks it something.
    asked: HashMap<String, HashSet<String>>,
    /// Which device last answered what each chat asked, and when.
    answered: HashMap<String, (String, i64)>,
    /// When each device was last sent a test.
    tested: HashMap<String, Instant>,
    /// Each device's Live Activity, as it was last sent.
    activities: HashMap<String, Live>,
    /// The Live Activity's state is being read.
    reading: bool,
}

/// A frame for the relay, and what its answer is about.
pub(super) struct Sending {
    id: String,
    device: String,
    token: String,
    /// An alert, where a Live Activity's update is not.
    alert: bool,
    /// `p` and the frame.
    text: String,
}

/// A frame written, waiting for its answer.
struct Sent {
    device: String,
    token: String,
    alert: bool,
    at: Instant,
}

/// A device's Live Activity as this computer last sent it.
struct Live {
    token: String,
    sent: Option<(Instant, push::Activity)>,
    /// Since when nothing works or waits.
    idle_since: Option<Instant>,
    /// It was sent `end`: nothing more goes to this token.
    ended: bool,
}

impl State {
    /// Keeps `sending` for a socket that sends notifications.
    fn wait(&mut self, sending: Sending) {
        self.wait_at(sending, Instant::now());
    }

    /// [`State::wait`] as of `now`: what waited two minutes goes, and the oldest goes past 32.
    fn wait_at(&mut self, sending: Sending, now: Instant) {
        self.waiting
            .retain(|(at, _)| now.saturating_duration_since(*at) <= WAIT);
        self.waiting.push_back((now, sending));
        while self.waiting.len() > WAITING {
            self.waiting.pop_front();
        }
    }

    /// Notes a frame written, for its answer.
    fn written(&mut self, sending: Sending) {
        let now = Instant::now();
        self.pending
            .retain(|_, sent| now.saturating_duration_since(sent.at) <= ANSWER_WITHIN);
        if self.pending.len() >= MAX_PENDING
            && let Some(oldest) = self
                .pending
                .iter()
                .min_by_key(|(_, sent)| sent.at)
                .map(|(id, _)| id.clone())
        {
            self.pending.remove(&oldest);
        }
        self.pending.insert(
            sending.id,
            Sent {
                device: sending.device,
                token: sending.token,
                alert: sending.alert,
                at: now,
            },
        );
    }
}

/// A socket to the relay opened: what is sent waits until its relay says it sends notifications.
pub(super) fn opened(shared: &Shared, out: UnboundedSender<Out>) {
    let mut state = lock(&shared.push);
    state.out = Some(out);
    state.relay = false;
}

/// The socket closed. The frames it was given and never wrote wait for the next one.
pub(super) fn closed(shared: &Shared, finished: &mut UnboundedReceiver<Out>) {
    let mut state = lock(&shared.push);
    state.out = None;
    state.relay = false;
    let mut unwritten = Vec::new();
    while let Ok(done) = finished.try_recv() {
        if let Out::Push(sending) = done {
            unwritten.push(sending);
        }
    }
    for sending in unwritten {
        state.wait(sending);
    }
}

/// The relay said what it does (`f`). When it sends notifications, what waited goes now: the
/// frames to write, oldest first.
pub(super) fn features(shared: &Shared, body: &str) -> Vec<String> {
    let mut state = lock(&shared.push);
    state.relay = crate::wire::relay_pushes(body);
    if !state.relay {
        return Vec::new();
    }
    let now = Instant::now();
    let fresh: Vec<Sending> = state
        .waiting
        .drain(..)
        .filter(|(at, _)| now.saturating_duration_since(*at) <= WAIT)
        .map(|(_, sending)| sending)
        .collect();
    fresh
        .into_iter()
        .map(|sending| {
            let text = sending.text.clone();
            state.written(sending);
            text
        })
        .collect()
}

/// A frame is written to the relay now: its answer is waited for.
pub(super) fn written(shared: &Shared, sending: Sending) -> String {
    let text = sending.text.clone();
    lock(&shared.push).written(sending);
    text
}

/// Whether the relay in front of this computer sends notifications now.
pub(super) fn relays(shared: &Shared) -> bool {
    lock(&shared.push).relay
}

/// How a frame went, as the relay answered it (`P`). An answer to a frame this run did not send
/// is no answer. Apple took an alert: the device's last notification is now. Apple said the
/// address is gone: the device forgets it, unless it gave another since.
pub(super) fn answered(shared: &Shared, body: &str) {
    let Ok(answer) = serde_json::from_str::<PushAnswer>(body) else {
        return;
    };
    let Some(sent) = lock(&shared.push).pending.remove(&answer.id) else {
        return;
    };
    let changed = {
        let mut registry = lock(&shared.registry);
        if answer.ok && sent.alert {
            registry.pushed(&sent.device, now());
            true
        } else if answer.drop {
            registry.drop_device_push(&sent.device, &sent.token)
        } else {
            false
        }
    };
    if answer.drop {
        let mut state = lock(&shared.push);
        if state
            .activities
            .get(&sent.device)
            .is_some_and(|live| live.token == sent.token)
        {
            state.activities.remove(&sent.device);
        }
    }
    if !answer.ok {
        // Why, in Apple's or the relay's word; never what it said or where it went. Not tried
        // again: a refusal is the same next time.
        tracing::debug!(
            status = ?answer.status,
            reason = answer.reason.as_deref().unwrap_or(""),
            "a notification was not delivered"
        );
    }
    if changed {
        (shared.config.changed)();
    }
}

/// Forgets what expired: frames that waited two minutes, sends whose answer never came, and
/// what is kept for devices no longer paired here. `now` is the time as the sweep sees it.
pub(super) fn sweep(shared: &Shared, now: Instant) {
    let paired: HashSet<String> = lock(&shared.registry)
        .devices()
        .iter()
        .map(|device| device.id.clone())
        .collect();
    let at = super::now();
    let mut state = lock(&shared.push);
    state
        .waiting
        .retain(|(queued, _)| now.saturating_duration_since(*queued) <= WAIT);
    state
        .pending
        .retain(|_, sent| now.saturating_duration_since(sent.at) <= ANSWER_WITHIN);
    state.tested.retain(|device, tested| {
        paired.contains(device) && now.saturating_duration_since(*tested) < TEST_EVERY
    });
    state
        .answered
        .retain(|_, (_, when)| at.saturating_sub(*when) < ANSWERED_FOR_MS);
    state
        .limits
        .retain(|device, limits| paired.contains(device) && !limits.idle(at));
    state
        .asked
        .retain(|device, chats| paired.contains(device) && !chats.is_empty());
    state.activities.retain(|device, _| paired.contains(device));
}

/// Sends a frame on a socket that sends notifications, or keeps it for one.
fn send(shared: &Shared, sending: Sending) {
    let mut state = lock(&shared.push);
    let out = state.out.clone().filter(|_| state.relay);
    match out {
        Some(out) => {
            if let Err(SendError(Out::Push(sending))) = out.send(Out::Push(sending)) {
                state.wait(sending);
            }
        }
        None => state.wait(sending),
    }
}

/// The frame id, the frame and what its answer is about, for `blob` sealed for `target`.
fn frame(target: &PushTarget, frame: PushFrame, alert: bool) -> Option<Sending> {
    let json = serde_json::to_string(&frame).ok()?;
    Some(Sending {
        id: frame.id,
        device: target.device.clone(),
        token: frame.to,
        alert,
        text: format!("p{json}"),
    })
}

/// A fresh id for a frame.
fn frame_id() -> Option<String> {
    crypto::random::<12>().ok().map(|id| crypto::encode(&id))
}

/// Seals `payload` for `target` and sends it as an alert, within the device's limits. Whether
/// it went.
fn alert(shared: &Shared, host_id: &str, target: &PushTarget, payload: Payload, ttl: u32) -> bool {
    let Some(push) = &target.push else {
        return false;
    };
    let (chat, kind) = (payload.chat.clone(), payload.kind);
    if !lock(&shared.push)
        .limits
        .entry(target.device.clone())
        .or_default()
        .allow(chat.as_deref(), kind, now())
    {
        return false;
    }
    let Some(plain) = payload.encoded() else {
        return false;
    };
    let (Ok(key), Some(id)) = (push::key(&target.key), frame_id()) else {
        return false;
    };
    let Ok(blob) = push::seal(&key, host_id, &target.device, &plain) else {
        return false;
    };
    let Some(sending) = frame(
        target,
        PushFrame {
            id,
            to: push.token.clone(),
            env: push.env.word(),
            kind: "alert",
            collapse: chat
                .as_deref()
                .map(|chat| push::collapse(&target.key, chat)),
            d: target.device.clone(),
            b: blob,
            ttl,
            // A resolved notification replaces the one that asked, without a sound.
            sound: Some(kind != Kind::Resolved),
            priority: None,
            event: None,
        },
        true,
    ) else {
        return false;
    };
    send(shared, sending);
    true
}

/// What a notification says of the chat it is about.
pub(super) struct About {
    pub(super) chat: String,
    pub(super) title: String,
    pub(super) project: Option<String>,
    pub(super) agent: Option<String>,
    /// How many chats on this computer wait on the person now, when known.
    pub(super) asking: Option<usize>,
}

impl About {
    /// What `entry` is, from the overview it is in.
    pub(super) fn of(
        overview: &Overview,
        entry: &ghost_protocol::workspace::WorkspaceChat,
    ) -> Self {
        Self {
            chat: entry.chat.id.clone(),
            title: render::title(&entry.chat),
            project: overview
                .projects
                .iter()
                .find(|project| project.id == entry.project_id)
                .map(|project| render::cut(&project.name, NAME)),
            agent: Some(render::cut(&entry.chat.agent_name, NAME)),
            asking: Some(asking(overview)),
        }
    }

    /// A notification of `kind` from this computer about this chat.
    fn payload(&self, shared: &Shared, kind: Kind, line: Option<&str>) -> Option<Payload> {
        let mut payload = Payload::new(kind, &shared.config.host_name, now()).ok()?;
        payload.chat = Some(self.chat.clone());
        payload.title = Some(self.title.clone());
        payload.project.clone_from(&self.project);
        payload.agent.clone_from(&self.agent);
        payload.line = line.map(|line| one_line(line, LINE));
        payload.asking_count = self.asking;
        Some(payload)
    }
}

/// How many chats of `overview` wait on the person.
pub(super) fn asking(overview: &Overview) -> usize {
    overview
        .chats
        .iter()
        .filter(|entry| !entry.archived && entry.chat.needs_attention)
        .count()
}

/// Words on one line, cut to `chars` characters.
fn one_line(text: &str, chars: usize) -> String {
    render::cut(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        chars,
    )
}

/// This computer's id at the relay, and the open workspace's devices that hear `kind` about
/// `chat`: those with notifications on that chose that kind, of every chat or of this one among
/// those they acted on.
fn audience(shared: &Shared, kind: Kind, chat: &str) -> (String, Vec<PushTarget>) {
    let registry = lock(&shared.registry);
    let targets = registry
        .push_targets()
        .into_iter()
        .filter(|target| {
            target.push.as_ref().is_some_and(|push| {
                kind.wanted(&push.prefs)
                    && (push.prefs.scope == Scope::All
                        || target.following.iter().any(|followed| followed == chat))
            })
        })
        .collect();
    (registry.host_id().to_owned(), targets)
}

/// The open workspace's device `owner`, when it hears `kind` of its own tasks.
fn owner(shared: &Shared, owner: &str, kind: Kind) -> Option<(String, PushTarget)> {
    let registry = lock(&shared.registry);
    let target = registry.push_target(owner).filter(|target| {
        target
            .push
            .as_ref()
            .is_some_and(|push| kind.wanted(&push.prefs))
    })?;
    Some((registry.host_id().to_owned(), target))
}

/// What a chat waits on, as a notification says it and answers it: its words, and the request
/// when the phone can answer it (a permission, or a question of a kind it knows). A permission
/// is allowed only once from a notification: `allow` is never an option that allows always.
pub(super) fn request_of(waiting: &Waiting) -> (String, Option<push::Request>) {
    match waiting {
        Waiting::Permission(request) => {
            let option = |kind: ghost_protocol::PermissionOptionKind| {
                request
                    .options
                    .iter()
                    .find(|option| option.kind == kind)
                    .map(|option| option.option_id.clone())
            };
            (
                request.title.clone(),
                Some(push::Request {
                    id: request.request_id.clone(),
                    kind: "permission",
                    allow: option(ghost_protocol::PermissionOptionKind::AllowOnce),
                    deny: option(ghost_protocol::PermissionOptionKind::RejectOnce),
                }),
            )
        }
        Waiting::Question(request) => {
            use ghost_protocol::UiPrompt;
            let kind = match &request.prompt {
                UiPrompt::Confirm { .. } => Some("confirm"),
                UiPrompt::Select { .. } => Some("select"),
                UiPrompt::Input { .. } => Some("input"),
                UiPrompt::Form { .. } => Some("form"),
                UiPrompt::Questions { .. } => Some("questions"),
                // A page to open, or a notice: nothing a notification answers.
                UiPrompt::Link { .. } | UiPrompt::Notify { .. } => None,
            };
            (
                render::question(&request.prompt),
                kind.map(|kind| push::Request {
                    id: request.request_id.clone(),
                    kind,
                    allow: None,
                    deny: None,
                }),
            )
        }
    }
}

/// What a finished turn came to, in a line: how it ended when it did not simply finish, the
/// agent's last words, and a command still failing.
pub(super) fn finished_line(brief: render::Brief) -> Option<String> {
    let mut parts = Vec::new();
    match brief.ended.as_str() {
        "stopped" => parts.push("Stopped.".to_owned()),
        "refused" => parts.push("The agent refused.".to_owned()),
        "ended at a limit" => parts.push("Ended at a limit.".to_owned()),
        "ended with an error" => parts.push("Ended with an error.".to_owned()),
        _ => {}
    }
    parts.extend(brief.said);
    if let Some(failing) = brief.failing {
        parts.push(format!("Still failing: {failing}."));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Why a chat's agent stopped, in a line.
pub(super) fn failed_line(
    chat: &ghost_protocol::session::ChatSummary,
    brief: render::Brief,
) -> String {
    if chat.status == ghost_protocol::SessionStatus::AuthRequired {
        "Its agent needs you to sign in on the computer.".into()
    } else {
        brief.said.unwrap_or_else(|| "Its agent stopped.".into())
    }
}

/// A chat started waiting on the person, its turn finished, or its agent failed: each device
/// that chose to hear it is told, but `except`. A device told that its chat asks keeps that
/// notification until the chat moves on; one that does not hear how it moved on (it chose not
/// to, or its limits) is quietly told that the chat no longer waits on it.
pub(super) fn chat_event(
    shared: &Shared,
    about: &About,
    kind: Kind,
    line: Option<&str>,
    request: Option<push::Request>,
    except: Option<&str>,
) {
    let (host_id, targets) = audience(shared, kind, &about.chat);
    if kind == Kind::Asking {
        // A new request: whoever answered the last one has not answered this one.
        lock(&shared.push).answered.remove(&about.chat);
    }
    let ttl = if kind == Kind::Asking && request.is_some() {
        TTL_ASKING
    } else {
        TTL
    };
    let mut told = HashSet::new();
    for target in targets
        .iter()
        .filter(|target| Some(target.device.as_str()) != except)
    {
        let Some(mut payload) = about.payload(shared, kind, line) else {
            continue;
        };
        payload.request.clone_from(&request);
        if alert(shared, &host_id, target, payload, ttl) {
            told.insert(target.device.clone());
        }
    }
    let mut state = lock(&shared.push);
    let stale: Vec<String> = state
        .asked
        .iter()
        .filter(|(device, chats)| chats.contains(&about.chat) && !told.contains(*device))
        .map(|(device, _)| device.clone())
        .collect();
    for device in &told {
        let chats = state.asked.entry(device.clone()).or_default();
        if kind == Kind::Asking {
            chats.insert(about.chat.clone());
        } else {
            chats.remove(&about.chat);
        }
    }
    drop(state);
    if kind != Kind::Asking && !stale.is_empty() {
        take_back(shared, about, &stale, "No longer waiting on you.");
    }
}

/// Whether any device has a notification standing that `chat` asks it something.
pub(super) fn asked_anywhere(shared: &Shared, chat: &str) -> bool {
    lock(&shared.push)
        .asked
        .values()
        .any(|chats| chats.contains(chat))
}

/// What `chat` asked was answered, and it works again: each device told it asks is told,
/// quietly, that it was answered elsewhere, but one that answered it itself (the phone took its
/// own notification away).
pub(super) fn resolved(shared: &Shared, about: &About) {
    let at = now();
    let (devices, by) = {
        let state = lock(&shared.push);
        let devices: Vec<String> = state
            .asked
            .iter()
            .filter(|(_, chats)| chats.contains(&about.chat))
            .map(|(device, _)| device.clone())
            .collect();
        let by = state
            .answered
            .get(&about.chat)
            .filter(|(_, when)| at.saturating_sub(*when) < ANSWERED_FOR_MS)
            .map(|(device, _)| device.clone());
        (devices, by)
    };
    let others: Vec<String> = devices
        .into_iter()
        .filter(|device| Some(device) != by.as_ref())
        .collect();
    if let Some(answering) = &by
        && let Some(chats) = lock(&shared.push).asked.get_mut(answering)
    {
        chats.remove(&about.chat);
    }
    let line = if by.is_some() {
        "Answered on another device."
    } else {
        "Answered on the computer."
    };
    take_back(shared, about, &others, line);
}

/// Replaces what `devices` were told `about` asks with a quiet notification that says `line`.
fn take_back(shared: &Shared, about: &About, devices: &[String], line: &str) {
    let (host_id, targets) = {
        let registry = lock(&shared.registry);
        let targets: Vec<PushTarget> = devices
            .iter()
            .filter_map(|device| registry.push_target(device))
            .filter(|target| target.push.is_some())
            .collect();
        (registry.host_id().to_owned(), targets)
    };
    for target in &targets {
        if let Some(payload) = about.payload(shared, Kind::Resolved, Some(line)) {
            alert(shared, &host_id, target, payload, TTL);
        }
    }
    let mut state = lock(&shared.push);
    for device in devices {
        if let Some(chats) = state.asked.get_mut(device) {
            chats.remove(&about.chat);
        }
    }
}

/// The review of a task from the phone is in: its device is told, about the task's own chat,
/// with the reviewer's chat beside it.
pub(super) fn reviewed(
    shared: &Shared,
    device: &str,
    about: &About,
    reviewer: &str,
    line: Option<&str>,
) {
    let Some((host_id, target)) = owner(shared, device, Kind::Reviewed) else {
        return;
    };
    if let Some(mut payload) = about.payload(shared, Kind::Reviewed, line) {
        payload.review = Some(reviewer.to_owned());
        alert(shared, &host_id, &target, payload, TTL);
    }
}

/// A task from the phone has news on its pull request: its device is told.
pub(super) fn pull_request(shared: &Shared, device: &str, about: &About, line: &str) {
    let Some((host_id, target)) = owner(shared, device, Kind::PullRequest) else {
        return;
    };
    if let Some(payload) = about.payload(shared, Kind::PullRequest, Some(line)) {
        alert(shared, &host_id, &target, payload, TTL);
    }
}

/// The pull request's news in a line for the person's own phone. Reviewers' and commenters'
/// words go only when they are the repository's own people, as `facts` shows them.
pub(super) fn pull_request_line(number: u64, news: &[crate::callback::PrNews<'_>]) -> String {
    use crate::callback::PrNews;
    let said: Vec<String> = news
        .iter()
        .map(|item| match item {
            PrNews::Merged => "It was merged.".to_owned(),
            PrNews::Closed => "It was closed without merging.".to_owned(),
            PrNews::ChecksFailed(names) => format!(
                "Its checks failed: {}.",
                names
                    .iter()
                    .take(5)
                    .map(|name| one_line(name, 60))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            PrNews::ChecksPassed => "Its checks passed.".to_owned(),
            PrNews::Note(note) => {
                let what = if note.kind == "comment" {
                    "A new comment".to_owned()
                } else {
                    format!("A new review ({})", one_line(&note.kind, 30))
                };
                let author = one_line(&note.author, 40);
                if note.trusted && !note.body.trim().is_empty() {
                    format!("{what} by {author}: {}", one_line(&note.body, 200))
                } else {
                    format!("{what} by {author}.")
                }
            }
            PrNews::More(more) => format!("And {more} more."),
        })
        .collect();
    format!("Pull request #{number}. {}", said.join(" "))
}

/// A task the phone queued for tonight could not start: its device is told why.
pub(super) fn queued_failed(
    shared: &Shared,
    device: &str,
    queued: &str,
    goal: &str,
    why: &str,
    asking: Option<usize>,
) {
    let Some((host_id, target)) = owner(shared, device, Kind::QueuedFailed) else {
        return;
    };
    let Ok(mut payload) = Payload::new(Kind::QueuedFailed, &shared.config.host_name, now()) else {
        return;
    };
    payload.title = Some(one_line(goal.lines().next().unwrap_or("Task"), 120));
    payload.line = Some(one_line(why, LINE));
    payload.queued = Some(queued.to_owned());
    payload.asking_count = asking;
    alert(shared, &host_id, &target, payload, TTL);
}

/// Why a test notification was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Untested {
    /// The device has not turned notifications on here.
    Off,
    /// The relay in front of this computer does not send notifications.
    NoRelay,
    /// One went less than ten seconds ago.
    TooSoon,
    /// The device had all the notifications it may have for now.
    Limited,
}

impl Untested {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Off => {
                "Notifications are not on for this device here. Turn them on in Ghost for iPhone."
            }
            Self::NoRelay => "The relay this computer uses does not send notifications yet.",
            Self::TooSoon => "A test went a moment ago. Wait a few seconds before sending another.",
            Self::Limited => {
                "This device was sent all the notifications it may have for now. Try again later."
            }
        }
    }

    pub(super) fn code(self) -> &'static str {
        match self {
            Self::Off => "not_found",
            Self::NoRelay => "unavailable",
            Self::TooSoon | Self::Limited => "busy",
        }
    }
}

/// Sends the open workspace's device `device` a test notification now: at most one every ten
/// seconds, and only through a relay that sends notifications.
pub(super) fn test(shared: &Shared, device: &str) -> Result<(), Untested> {
    let (host_id, target) = {
        let registry = lock(&shared.registry);
        (registry.host_id().to_owned(), registry.push_target(device))
    };
    let Some(target) = target.filter(|target| target.push.is_some()) else {
        return Err(Untested::Off);
    };
    {
        let mut state = lock(&shared.push);
        if !state.relay {
            return Err(Untested::NoRelay);
        }
        let now = Instant::now();
        if state
            .tested
            .get(device)
            .is_some_and(|at| now.saturating_duration_since(*at) < TEST_EVERY)
        {
            return Err(Untested::TooSoon);
        }
        state.tested.insert(device.to_owned(), now);
    }
    let host = &shared.config.host_name;
    let mut payload = Payload::new(Kind::Test, host, now()).map_err(|_| Untested::Limited)?;
    payload.line = Some(format!(
        "Notifications from {} arrive here.",
        one_line(host, NAME)
    ));
    if alert(shared, &host_id, &target, payload, TTL) {
        Ok(())
    } else {
        Err(Untested::Limited)
    }
}

/// The app on `device` started, sent to or answered `chat` while `scope` was the open
/// workspace (`answered`: it answered what the chat asked). A device that chose "mine" hears of
/// that chat from now on, and nothing is taken back from the phone that answered.
pub(super) fn acted(shared: &Shared, device: &str, scope: &str, chat: &str, answered: bool) {
    {
        let mut registry = lock(&shared.registry);
        if registry.scope() == scope {
            registry.follow_device(device, chat);
        }
    }
    if answered {
        let mut state = lock(&shared.push);
        state
            .answered
            .insert(chat.to_owned(), (device.to_owned(), now()));
        if let Some(chats) = state.asked.get_mut(device) {
            chats.remove(chat);
        }
    }
}

/// The device hears nothing more: what was kept for it goes.
pub(super) fn forget(shared: &Shared, device: &str) {
    let mut state = lock(&shared.push);
    state.asked.remove(device);
    state.activities.remove(device);
}

/// Whether a Live Activity is to be read and sent now, given how many chats ask and work as
/// the call-back's watch counts them: one was never sent, more chats ask than it shows, ten
/// seconds passed while something works or the counts changed, or nothing worked or waited for
/// five minutes and it ends. Starts the reading when it says so: [`activity`] ends it.
pub(super) fn activity_due(shared: &Shared, asking: usize, working: usize) -> bool {
    let tokens: Vec<(String, ActivityToken)> = lock(&shared.registry)
        .push_targets()
        .into_iter()
        .filter_map(|target| Some((target.device, target.activity?)))
        .collect();
    let mut state = lock(&shared.push);
    // In step with the tokens: a new one starts afresh, one gone takes its state with it.
    state.activities.retain(|device, live| {
        tokens
            .iter()
            .any(|(known, token)| known == device && token.token == live.token)
    });
    for (device, token) in tokens {
        state.activities.entry(device).or_insert_with(|| Live {
            token: token.token,
            sent: None,
            idle_since: None,
            ended: false,
        });
    }
    if state.reading {
        return false;
    }
    let now = Instant::now();
    let due = state.activities.values().any(|live| {
        if live.ended {
            return false;
        }
        let Some((at, last)) = &live.sent else {
            return true;
        };
        let waited = now.saturating_duration_since(*at) >= ACTIVITY_EVERY;
        asking > last.asking
            || (waited && ((asking, working) != (last.asking, last.working) || working > 0))
            || (asking + working == 0
                && live
                    .idle_since
                    .is_some_and(|since| now.saturating_duration_since(since) >= ACTIVITY_IDLE))
    });
    state.reading = due;
    due
}

/// Reads what the Live Activity shows and sends it to each device whose activity is behind:
/// how many chats ask and work, the one that leads and since when. At most one update every ten
/// seconds per device, at once when more chats ask than it shows (and then at Apple's high
/// priority), and `end` once nothing worked or waited for five minutes.
pub(super) async fn activity(shared: Arc<Shared>, backend: Arc<dyn Backend>) {
    let read = read_activity(&backend).await;
    let (host_id, targets) = {
        let registry = lock(&shared.registry);
        (registry.host_id().to_owned(), registry.push_targets())
    };
    let mut sends = Vec::new();
    {
        let mut state = lock(&shared.push);
        state.reading = false;
        let Some(read) = read else {
            return;
        };
        let now = Instant::now();
        for target in targets {
            let Some(token) = target.activity.clone() else {
                continue;
            };
            let Some(live) = state
                .activities
                .get_mut(&target.device)
                .filter(|live| live.token == token.token)
            else {
                continue;
            };
            if let Some((priority, ending)) = decide(live, &read, now) {
                sends.push((target, token, priority, ending));
            }
        }
        drop(state);
        for (target, token, priority, ending) in sends {
            send_activity(&shared, &host_id, &target, &token, &read, priority, ending);
        }
    }
}

/// What a device's Live Activity is sent, now that it was read as `read` at `now`: nothing, an
/// update at Apple's priority (10 when more chats ask than it showed, else 5), or its end, once
/// nothing worked or waited for five minutes. Notes what it sends.
fn decide(live: &mut Live, read: &push::Activity, now: Instant) -> Option<(u8, bool)> {
    if live.ended {
        return None;
    }
    if read.asking + read.working == 0 {
        live.idle_since.get_or_insert(now);
    } else {
        live.idle_since = None;
    }
    let ending = live
        .idle_since
        .is_some_and(|since| now.saturating_duration_since(since) >= ACTIVITY_IDLE);
    let (send, priority) = match &live.sent {
        _ if ending => (true, 5),
        None => (true, 5),
        Some((_, last)) if last == read => (false, 5),
        Some((_, last)) if read.asking > last.asking => (true, 10),
        Some((at, _)) => (now.saturating_duration_since(*at) >= ACTIVITY_EVERY, 5),
    };
    if !send {
        return None;
    }
    live.sent = Some((now, read.clone()));
    live.ended = ending;
    Some((priority, ending))
}

/// Seals a Live Activity's state for `target` and sends it.
fn send_activity(
    shared: &Shared,
    host_id: &str,
    target: &PushTarget,
    token: &ActivityToken,
    state: &push::Activity,
    priority: u8,
    ending: bool,
) {
    let Ok(plain) = serde_json::to_vec(state) else {
        return;
    };
    let (Ok(key), Some(id)) = (push::key(&target.key), frame_id()) else {
        return;
    };
    let Ok(blob) = push::seal(&key, host_id, &target.device, &plain) else {
        return;
    };
    if let Some(sending) = frame(
        target,
        PushFrame {
            id,
            to: token.token.clone(),
            env: token.env.word(),
            kind: "liveactivity",
            collapse: None,
            d: target.device.clone(),
            b: blob,
            ttl: TTL_ACTIVITY,
            sound: None,
            priority: Some(priority),
            event: Some(if ending { "end" } else { "update" }),
        },
        false,
    ) {
        send(shared, sending);
    }
}

/// What the Live Activity shows now: how many chats ask and work, as the Inbox sorts them, and
/// the one that leads. The first that asks leads, by its title and what it asks; else the first
/// that works, by its agent and what it does, with when its turn began.
async fn read_activity(backend: &Arc<dyn Backend>) -> Option<push::Activity> {
    let overview = backend.overview().await.ok()?;
    let sorted = render::sorted(&overview.chats, now());
    let (lead, since) = if let Some(entry) = sorted.asking.first() {
        let asked = backend
            .history(&entry.chat.id, u32::MAX)
            .await
            .ok()
            .and_then(|page| {
                render::digest(&page.items)
                    .waiting
                    .last()
                    .map(|waiting| request_of(waiting).0)
            });
        let words = match asked {
            Some(asked) => format!("{}: {asked}", render::title(&entry.chat)),
            None => render::title(&entry.chat),
        };
        (Some(push::plain(&words, 120)), None)
    } else if let Some(entry) = sorted.working.first() {
        let page = backend.history(&entry.chat.id, u32::MAX).await.ok();
        let doing = page.as_ref().map_or_else(
            || render::status(&entry.chat),
            |page| render::doing(&entry.chat, &page.items),
        );
        let since = page.as_ref().and_then(|page| {
            let turn = render::last_turn(&page.items);
            turn.started_at.filter(|_| turn.ended_at.is_none())
        });
        (
            Some(push::plain(
                &format!("{}: {doing}", entry.chat.agent_name),
                120,
            )),
            since,
        )
    } else {
        (None, None)
    };
    Some(push::Activity {
        asking: sorted.asking.len(),
        working: sorted.working.len(),
        lead,
        since,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sending(id: &str) -> Sending {
        Sending {
            id: id.into(),
            device: "d1".into(),
            token: "ab".repeat(32),
            alert: true,
            text: format!("p{{\"id\":\"{id}\"}}"),
        }
    }

    /// At most 32 frames wait for a socket that sends them, the newest kept, and none longer
    /// than two minutes.
    #[test]
    fn frames_wait_a_while_and_only_so_many() {
        let mut state = State::default();
        let start = Instant::now();
        for n in 0..WAITING + 8 {
            state.wait_at(sending(&format!("s{n}")), start);
        }
        assert_eq!(state.waiting.len(), WAITING);
        assert_eq!(
            state.waiting.front().map(|(_, sent)| sent.id.as_str()),
            Some("s8")
        );
        state.wait_at(sending("late"), start + WAIT + Duration::from_secs(1));
        let ids: Vec<&str> = state
            .waiting
            .iter()
            .map(|(_, sent)| sent.id.as_str())
            .collect();
        assert_eq!(ids, ["late"], "the others waited too long");
    }

    fn read(asking: usize, working: usize) -> push::Activity {
        push::Activity {
            asking,
            working,
            lead: None,
            since: None,
        }
    }

    /// A Live Activity is sent what changed at most every ten seconds, at once and at Apple's
    /// high priority when more chats ask, and its end once nothing worked or waited for five
    /// minutes; nothing after.
    #[test]
    fn a_live_activity_follows_the_counts_within_its_rate_and_ends_when_idle() {
        let mut live = Live {
            token: "t".into(),
            sent: None,
            idle_since: None,
            ended: false,
        };
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        assert_eq!(
            decide(&mut live, &read(0, 1), at(0)),
            Some((5, false)),
            "the first"
        );
        assert_eq!(
            decide(&mut live, &read(0, 1), at(20)),
            None,
            "nothing changed"
        );
        assert_eq!(decide(&mut live, &read(0, 2), at(25)), Some((5, false)));
        assert_eq!(
            decide(&mut live, &read(0, 3), at(30)),
            None,
            "within ten seconds"
        );
        assert_eq!(
            decide(&mut live, &read(1, 3), at(31)),
            Some((10, false)),
            "more chats ask: at once"
        );
        assert_eq!(decide(&mut live, &read(1, 2), at(41)), Some((5, false)));
        assert_eq!(decide(&mut live, &read(0, 0), at(60)), Some((5, false)));
        assert_eq!(
            decide(&mut live, &read(0, 0), at(200)),
            None,
            "idle, not yet long"
        );
        assert_eq!(
            decide(&mut live, &read(0, 0), at(60) + ACTIVITY_IDLE),
            Some((5, true)),
            "the end"
        );
        assert!(live.ended);
        assert_eq!(
            decide(&mut live, &read(2, 0), at(400)),
            None,
            "ended for good"
        );

        // Work again before five minutes keeps it going.
        let mut live = Live {
            token: "t".into(),
            sent: None,
            idle_since: None,
            ended: false,
        };
        decide(&mut live, &read(0, 0), at(0));
        assert_eq!(decide(&mut live, &read(0, 1), at(200)), Some((5, false)));
        assert_eq!(decide(&mut live, &read(0, 0), at(290)), Some((5, false)));
        assert_eq!(
            decide(&mut live, &read(0, 0), at(450)),
            None,
            "idle anew from 290"
        );
    }

    #[test]
    fn a_request_is_offered_only_as_the_phone_can_answer_it() {
        use ghost_protocol::{
            PermissionOption, PermissionOptionKind, PermissionRequest, UiPrompt, UiRequest,
        };
        let option = |id: &str, kind| PermissionOption {
            option_id: id.into(),
            kind,
            label: id.into(),
        };
        let permission = Waiting::Permission(PermissionRequest {
            request_id: "p1".into(),
            tool_call_id: None,
            title: "Run `npm test`".into(),
            kind: None,
            raw_input: None,
            content: vec![],
            locations: vec![],
            options: vec![
                option("always", PermissionOptionKind::AllowAlways),
                option("once", PermissionOptionKind::AllowOnce),
                option("never", PermissionOptionKind::RejectAlways),
                option("no", PermissionOptionKind::RejectOnce),
            ],
            truncated: None,
        });
        let (line, request) = request_of(&permission);
        assert_eq!(line, "Run `npm test`");
        assert_eq!(
            request,
            Some(push::Request {
                id: "p1".into(),
                kind: "permission",
                allow: Some("once".into()),
                deny: Some("no".into()),
            })
        );
        // Only "always" to allow: Allow is not offered at all.
        let Waiting::Permission(mut only_always) = permission else {
            unreachable!()
        };
        only_always.options = vec![option("always", PermissionOptionKind::AllowAlways)];
        let (_, request) = request_of(&Waiting::Permission(only_always));
        assert_eq!(request.and_then(|request| request.allow), None);
        let question = |prompt| {
            Waiting::Question(UiRequest {
                request_id: "q1".into(),
                prompt,
            })
        };
        for (prompt, kind) in [
            (
                UiPrompt::Confirm {
                    message: "Deploy?".into(),
                },
                Some("confirm"),
            ),
            (
                UiPrompt::Input {
                    message: "Name it".into(),
                    placeholder: None,
                },
                Some("input"),
            ),
            (
                UiPrompt::Select {
                    message: "Which?".into(),
                    options: vec!["a".into()],
                },
                Some("select"),
            ),
            (
                UiPrompt::Link {
                    message: "Sign in".into(),
                    source: None,
                    url: "https://example.com".into(),
                },
                None,
            ),
        ] {
            let (_, request) = request_of(&question(prompt));
            assert_eq!(request.map(|request| request.kind), kind);
        }
    }

    #[test]
    fn a_turns_end_reads_as_a_line() {
        let brief = |ended: &str, failing: Option<&str>, said: Option<&str>| render::Brief {
            ended: ended.into(),
            failing: failing.map(str::to_owned),
            said: said.map(str::to_owned),
        };
        assert_eq!(
            finished_line(brief("finished", None, Some("Done."))).as_deref(),
            Some("Done.")
        );
        assert_eq!(finished_line(brief("finished", None, None)), None);
        assert_eq!(
            finished_line(brief(
                "stopped",
                Some("exit 1 | npm test"),
                Some("Halfway.")
            ))
            .as_deref(),
            Some("Stopped. Halfway. Still failing: exit 1 | npm test.")
        );
        assert_eq!(
            finished_line(brief("idle", None, None)),
            None,
            "a status is no ending"
        );
    }
}
