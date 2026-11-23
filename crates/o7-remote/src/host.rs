//! The desktop's end of remote sessions: one task that keeps the socket to the relay, and behind
//! it a session per connected browser.
//!
//! The task owns every session's keys and counters, so each message is opened and sealed in
//! order. Work that waits (a call into the app) runs beside it and hands back a finished answer.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::{SinkExt, StreamExt};
use o7_protocol::{
    EventBatch,
    remote::{
        RemoteBot, RemoteBotCallback, RemoteBotLink, RemoteDevice, RemoteLink, RemotePairing,
        RemoteStatus, RemoteTold,
    },
    session::{ChatStatusUpdate, HistoryPage},
};
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    time::Instant,
};
use tokio_tungstenite::tungstenite::Message;

use crate::{
    backend::{Backend, BackendError, Frames, Statuses},
    bot,
    callback::{self, Callback, CallbackEvent, Delivery},
    crypto::{self, Channel, NONCE_BYTES, Side},
    registry::{Registry, RegistryError},
    relay,
    wire::{
        self, BotAnswer, BotCall, BotLink, CredentialKind, Data, ErrorBody, Event, FromRelay,
        Hello, Op, Paired, Refusal, Refused, Reply, Request, Welcome,
    },
};

mod session;
use session::{Received, bot_answer, bot_links, closed, receive, refusal, seal};

/// Browsers connected at once, proven or not.
const MAX_CONNECTIONS: usize = 32;
/// How long a browser has to prove it holds a key.
const PROVE_WITHIN: Duration = Duration::from_secs(20);
/// Calls one browser may have running at once.
const MAX_CALLS: usize = 16;
/// Bot calls running at once.
const MAX_BOT_CALLS: usize = 8;
/// Live events queued for one browser before it is told to open its chat again.
const MAX_BACKLOG: usize = 4 * 1024 * 1024;
/// The most one sealed message carries: under the relay's limit once sealed and encoded.
const MAX_PLAINTEXT: usize = 600 * 1024;
/// The most one merged frame carries. Frames queued for one browser while the socket is busy go
/// out together, so a fast stream costs a phone a few messages a second, not one per word.
const MERGED_FRAME: usize = 256 * 1024;
/// Finished work taken at once before writing: enough to merge a burst, never a whole backlog.
const DRAIN: usize = 256;
const MAX_TEXT: usize = 64 * 1024;
const PING_EVERY: Duration = Duration::from_secs(25);
const SWEEP_EVERY: Duration = Duration::from_secs(5);
/// No answer to a keep-alive for this long: the socket is dead.
const SILENCE: Duration = Duration::from_secs(70);
const WATCHER: &str = "auracode-remote";
/// The owner the call-back watches chats under.
const TELLER: &str = "auracode-remote-callbacks";
/// How long after a chat settles its links' bots are told. A bot that was waiting on the chat
/// gets the result in its own call's answer within this time, and is then not told again.
const TELL_AFTER: Duration = Duration::from_secs(2);
/// How often the call-back looks again at whether anything is to be watched at all.
const TELL_CHECK: Duration = Duration::from_secs(5);

pub struct Config {
    /// `remote.json` in the data folder.
    pub path: PathBuf,
    /// The relay's WebSocket origin, such as `wss://o7.codes`.
    pub relay: String,
    /// The website's origin, which pairing links and bot addresses start with.
    pub site: String,
    /// What paired browsers call this computer.
    pub host_name: String,
    /// The open workspace's id; empty while none is open. Devices and bot links reach only the
    /// workspace they were made in.
    pub profile: String,
    pub backend: Arc<dyn Backend>,
    /// Called after anything Settings shows has changed.
    pub changed: Arc<dyn Fn() + Send + Sync>,
    /// Sends one message to a bot link's call-back address. The app does the sending: the key
    /// the address takes is in its secure storage. Never called for a link without an address.
    pub deliver: Arc<dyn Fn(Delivery) + Send + Sync>,
}

enum Control {
    /// Settings changed: remote access was turned on or off, or a device or bot link was removed.
    Refresh,
    /// Another profile was opened: every followed chat is gone.
    Reset,
    Shutdown,
}

struct Shared {
    registry: Mutex<Registry>,
    link: Mutex<RemoteLink>,
    /// Sessions open per device.
    connected: Mutex<HashMap<String, usize>>,
    /// When each link's bot last heard how a chat stands from one of its own calls, by link and
    /// chat. A state it has heard is not told again.
    heard: Mutex<HashMap<(String, String), i64>>,
    /// How the last message to each link's call-back address went, by link.
    told: Mutex<HashMap<String, RemoteTold>>,
    /// The service was shut down: the call-back stops watching.
    closing: AtomicBool,
    config: Config,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not take remote access down with it; the data is plain state.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Remote access for one installation. Cloning shares it.
#[derive(Clone)]
pub struct Service {
    shared: Arc<Shared>,
    control: UnboundedSender<Control>,
}

impl Service {
    /// Reads the saved settings and starts the connection task on `runtime`. Nothing connects
    /// until remote access is on.
    pub fn start(config: Config, runtime: &tokio::runtime::Handle) -> Result<Self, RegistryError> {
        let mut registry = Registry::load(&config.path)?;
        // Devices and bot links issued under another site are not this copy's to accept.
        registry.set_site(&config.site);
        registry.set_scope(&config.profile);
        let shared = Arc::new(Shared {
            link: Mutex::new(if registry.enabled() {
                RemoteLink::Connecting
            } else {
                RemoteLink::Off
            }),
            registry: Mutex::new(registry),
            connected: Mutex::new(HashMap::new()),
            heard: Mutex::new(HashMap::new()),
            told: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            config,
        });
        let (control, receiver) = unbounded_channel();
        runtime.spawn(run(shared.clone(), receiver));
        runtime.spawn(tell(shared.clone()));
        Ok(Self { shared, control })
    }

    pub fn status(&self) -> RemoteStatus {
        let mut registry = lock(&self.shared.registry);
        let connected = lock(&self.shared.connected);
        let told = lock(&self.shared.told);
        RemoteStatus {
            enabled: registry.enabled(),
            link: lock(&self.shared.link).clone(),
            host_name: self.shared.config.host_name.clone(),
            devices: registry
                .devices()
                .iter()
                .map(|device| RemoteDevice {
                    id: device.id.clone(),
                    name: device.name.clone(),
                    created_at: device.created_at,
                    last_seen_at: device.last_seen_at,
                    connected: connected.get(&device.id).is_some_and(|count| *count > 0),
                })
                .collect(),
            bots: registry
                .bots()
                .iter()
                .map(|bot| RemoteBot {
                    id: bot.id.clone(),
                    name: bot.name.clone(),
                    created_at: bot.created_at,
                    last_used_at: bot.last_used_at,
                    full: bot.full,
                    calls: u32::try_from(bot.calls).unwrap_or(u32::MAX),
                    failed: u32::try_from(bot.failed).unwrap_or(u32::MAX),
                    callback: bot.callback.as_ref().map(|callback| RemoteBotCallback {
                        kind: serde_json::to_value(callback.kind)
                            .ok()
                            .and_then(|kind| kind.as_str().map(str::to_owned))
                            .unwrap_or_default(),
                        address: callback::shown(&callback.url),
                        channel: callback.channel.clone(),
                        to: callback.to.clone(),
                        last: told.get(&bot.id).cloned(),
                    }),
                })
                .collect(),
            pairing_expires_at: registry.pairing(now()).map(|pairing| pairing.expires_at),
        }
    }

    fn refresh(&self) {
        let _ = self.control.send(Control::Refresh);
        (self.shared.config.changed)();
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), RegistryError> {
        lock(&self.shared.registry).set_enabled(enabled)?;
        if !enabled {
            *lock(&self.shared.link) = RemoteLink::Off;
        }
        self.refresh();
        Ok(())
    }

    /// Opens a new pairing link, closing the one before it.
    pub fn pair(&self) -> Result<RemotePairing, RegistryError> {
        let mut registry = lock(&self.shared.registry);
        let pairing = registry.start_pairing(now())?;
        let url = format!(
            "{}/remote#{}.{}.{}",
            self.shared.config.site,
            registry.host_id(),
            pairing.id,
            crypto::encode(&pairing.key)
        );
        drop(registry);
        (self.shared.config.changed)();
        Ok(RemotePairing {
            url,
            expires_at: pairing.expires_at,
        })
    }

    pub fn cancel_pairing(&self) {
        lock(&self.shared.registry).cancel_pairing();
        (self.shared.config.changed)();
    }

    pub fn remove_device(&self, id: &str) -> Result<(), RegistryError> {
        lock(&self.shared.registry).remove_device(id)?;
        self.refresh();
        Ok(())
    }

    /// `full`: every tool, where a link without it has the chat tools only.
    pub fn create_bot(&self, name: &str, full: bool) -> Result<RemoteBotLink, RegistryError> {
        let mut registry = lock(&self.shared.registry);
        let new = registry.add_bot(name, full, now())?;
        let link = RemoteBotLink {
            name: registry
                .bots()
                .iter()
                .find(|bot| bot.id == new.id)
                .map_or_else(String::new, |bot| bot.name.clone()),
            url: format!(
                "{}/api/remote/v1/mcp/{}/{}.{}",
                self.shared.config.site,
                registry.host_id(),
                new.id,
                new.token
            ),
            id: new.id,
        };
        drop(registry);
        self.refresh();
        Ok(link)
    }

    pub fn remove_bot(&self, id: &str) -> Result<(), RegistryError> {
        lock(&self.shared.registry).remove_bot(id)?;
        self.refresh();
        Ok(())
    }

    /// Sets or clears where the open workspace's bot link `id` is told. `false`: no such link.
    /// The caller has checked the address (`callback::check_url`) and keeps its key.
    pub fn set_bot_callback(
        &self,
        id: &str,
        callback: Option<Callback>,
    ) -> Result<bool, RegistryError> {
        let found = lock(&self.shared.registry).set_bot_callback(id, callback)?;
        // What was said of the address before is not about this one.
        lock(&self.shared.told).remove(id);
        (self.shared.config.changed)();
        Ok(found)
    }

    /// Where the open workspace's bot link `id` is told, if anywhere.
    pub fn bot_callback(&self, id: &str) -> Option<Callback> {
        lock(&self.shared.registry)
            .bots()
            .into_iter()
            .find(|bot| bot.id == id)
            .and_then(|bot| bot.callback.clone())
    }

    /// Notes how a message to the bot link `id`'s address went, for Settings to show.
    pub fn note_told(&self, id: &str, ok: bool, detail: String) {
        lock(&self.shared.told).insert(
            id.to_owned(),
            RemoteTold {
                at: now(),
                ok,
                detail,
            },
        );
        (self.shared.config.changed)();
    }

    /// The message Settings sends to check a link's address. `None`: no such link or address.
    pub fn test_delivery(&self, id: &str) -> Option<Delivery> {
        let registry = lock(&self.shared.registry);
        let bot = registry.bots().into_iter().find(|bot| bot.id == id)?;
        Some(Delivery {
            bot_id: bot.id.clone(),
            bot_name: bot.name.clone(),
            callback: bot.callback.clone()?,
            event: CallbackEvent::Test,
            id: crypto::encode(&crypto::random::<12>().ok()?),
            chat_id: String::new(),
            title: String::new(),
            text: callback::test_text(),
            at: now(),
        })
    }

    /// Another profile was opened: browsers start over from its chats.
    pub fn profile_changed(&self, profile: &str) {
        lock(&self.shared.registry).set_scope(profile);
        let _ = self.control.send(Control::Reset);
        (self.shared.config.changed)();
    }

    /// Ends the connection task. Paired devices and bot links stay saved.
    pub fn shutdown(&self) {
        self.shared.closing.store(true, Ordering::Relaxed);
        let _ = self.control.send(Control::Shutdown);
    }
}

/// How a chat stands, as far as telling a bot goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Its agent is working.
    Busy,
    /// It waits for an answer or a permission.
    Asking,
    /// Its agent failed, or needs its owner to sign in.
    Failed,
    /// Between turns.
    Rest,
}

impl Stage {
    fn of(status: o7_protocol::SessionStatus, needs_attention: bool) -> Self {
        use o7_protocol::SessionStatus as S;
        if needs_attention {
            return Self::Asking;
        }
        match status {
            S::Running | S::Starting | S::Cancelling => Self::Busy,
            S::Failed | S::AuthRequired => Self::Failed,
            S::Ready | S::Stopped | S::Stopping => Self::Rest,
        }
    }
}

/// The call-back's task: watches the open workspace's chats while any of its bot links names an
/// address, and tells a link's bot when a chat that link acted on stops working: it finished,
/// failed or asks for an answer. Only a change from working (or from asking) is told, so nothing
/// is sent for how chats stood when the watching began.
async fn tell(shared: Arc<Shared>) {
    while !shared.closing.load(Ordering::Relaxed) {
        let scope = {
            let registry = lock(&shared.registry);
            (registry.enabled() && registry.any_callback() && !registry.scope().is_empty())
                .then(|| registry.scope().to_owned())
        };
        let Some(scope) = scope else {
            tokio::time::sleep(TELL_CHECK).await;
            continue;
        };
        let backend = shared.config.backend.clone().bound(&scope);
        let (updates, mut received) = unbounded_channel::<ChatStatusUpdate>();
        let statuses: Statuses = Arc::new(move |update| updates.send(update.clone()).is_ok());
        let mut stages: HashMap<String, Stage> = HashMap::new();
        let watching = backend.watch(TELLER, statuses).is_ok();
        if let (true, Ok(overview)) = (watching, backend.overview().await) {
            for entry in &overview.chats {
                stages.insert(
                    entry.chat.id.clone(),
                    Stage::of(entry.chat.status, entry.chat.needs_attention),
                );
            }
        } else {
            backend.release(TELLER);
            tokio::time::sleep(TELL_CHECK).await;
            continue;
        }
        loop {
            let update = match tokio::time::timeout(TELL_CHECK, received.recv()).await {
                Ok(Some(update)) => update,
                // The watch ended: another workspace opened, or the app is closing.
                Ok(None) => break,
                Err(_) => {
                    let registry = lock(&shared.registry);
                    if shared.closing.load(Ordering::Relaxed)
                        || !registry.enabled()
                        || !registry.any_callback()
                        || registry.scope() != scope
                    {
                        break;
                    }
                    continue;
                }
            };
            let stage = Stage::of(update.status, update.needs_attention);
            let before = stages.insert(update.chat_id.clone(), stage);
            let event = match (before, stage) {
                (Some(Stage::Busy), Stage::Asking) => CallbackEvent::Asking,
                (Some(Stage::Busy | Stage::Asking), Stage::Rest) => CallbackEvent::Finished,
                (Some(Stage::Busy | Stage::Asking), Stage::Failed) => CallbackEvent::Failed,
                _ => continue,
            };
            if lock(&shared.registry).told_of(&update.chat_id).is_empty() {
                continue;
            }
            tokio::spawn(tell_later(
                shared.clone(),
                backend.clone(),
                update.chat_id,
                stage,
                event,
                now(),
            ));
        }
        backend.release(TELLER);
    }
}

/// Tells the links that follow `chat_id` of `event`, a moment after it happened: unless the chat
/// has moved on by then (a later change tells of that), and not a link whose bot has heard of
/// this state from a call of its own meanwhile.
async fn tell_later(
    shared: Arc<Shared>,
    backend: Arc<dyn Backend>,
    chat_id: String,
    stage: Stage,
    event: CallbackEvent,
    settled_at: i64,
) {
    tokio::time::sleep(TELL_AFTER).await;
    let Ok(overview) = backend.overview().await else {
        return;
    };
    let Some(entry) = overview.chats.iter().find(|entry| entry.chat.id == chat_id) else {
        return;
    };
    if Stage::of(entry.chat.status, entry.chat.needs_attention) != stage {
        return;
    }
    let told: Vec<_> = {
        let heard = lock(&shared.heard);
        lock(&shared.registry)
            .told_of(&chat_id)
            .into_iter()
            .filter(|(bot, _, _)| {
                heard
                    .get(&(bot.clone(), chat_id.clone()))
                    // Heard in the same millisecond it settled: told anyway. Twice is harmless;
                    // never is a bot left waiting.
                    .is_none_or(|at| *at <= settled_at)
            })
            .collect()
    };
    if told.is_empty() {
        return;
    }
    let Ok(page) = backend.history(&chat_id, u32::MAX).await else {
        return;
    };
    let project = overview
        .projects
        .iter()
        .find(|project| project.id == entry.project_id)
        .map_or("no project", |project| project.name.as_str());
    let text = callback::text(event, &entry.chat, project, &page.items);
    for (bot_id, bot_name, callback) in told {
        let Ok(id) = crypto::random::<12>() else {
            continue;
        };
        (shared.config.deliver)(Delivery {
            bot_id,
            bot_name,
            callback,
            event,
            id: crypto::encode(&id),
            chat_id: chat_id.clone(),
            title: crate::render::title(&entry.chat),
            text: text.clone(),
            at: now(),
        });
    }
}

/// A browser's session once it said hello.
struct Session {
    channel: Channel,
    /// The pairing link the session was opened with, until it pairs.
    pairing: Option<String>,
    /// The device, once known: at once for a paired device, after `pair` for a pairing link.
    device: Option<String>,
    /// The workspace its key was accepted under. Everything it asks for is asked of that one.
    scope: String,
    /// A sealed message from the browser opened: it holds the key.
    proven: bool,
    calls: Arc<AtomicUsize>,
    backlog: Arc<AtomicUsize>,
    /// Which chat this browser follows is decided by its newest `open` or `close`. Each takes the
    /// next number and runs under `following`; one that finds a newer number has been overtaken
    /// and does nothing, so a slow open can never replace the chat asked for after it.
    follow: Arc<AtomicU64>,
    following: Arc<tokio::sync::Mutex<()>>,
    /// The session is over: an open still waiting its turn must not start following for it.
    ended: Arc<AtomicBool>,
}

struct Connection {
    since: Instant,
    session: Option<Session>,
}

/// Finished work on its way back to the socket.
enum Out {
    /// A sealed message for one browser. `backlog` is given back once it is written.
    Sealed {
        cid: String,
        json: String,
        backlog: usize,
    },
    /// Live events of the chat a browser follows: `json` is the frame as it would be sent alone
    /// (`None` once merged), and frames that follow each other are merged before they are sealed
    /// ([`merge`]). `backlog` is also the frame's size.
    Frame {
        cid: String,
        batch: EventBatch,
        json: Option<String>,
        backlog: usize,
    },
    /// End this browser's session, after what was queued before it.
    Drop {
        cid: String,
    },
    Status(ChatStatusUpdate),
    Bot {
        rid: String,
        answer: bot::Answer,
    },
}

enum Ended {
    /// Remote access was turned off.
    Disabled,
    Shutdown,
    /// The socket closed or failed: connect again.
    Lost(String),
}

async fn run(shared: Arc<Shared>, mut control: UnboundedReceiver<Control>) {
    let mut failures: u32 = 0;
    loop {
        if !lock(&shared.registry).enabled() {
            set_link(&shared, RemoteLink::Off);
            match control.recv().await {
                Some(Control::Shutdown) | None => return,
                Some(_) => continue,
            }
        }
        if failures == 0 {
            set_link(&shared, RemoteLink::Connecting);
        }
        let (id, key) = {
            let registry = lock(&shared.registry);
            (
                registry.host_id().to_owned(),
                registry.host_key().to_owned(),
            )
        };
        let reason = match relay::connect(&shared.config.relay, &id, &key).await {
            Ok(socket) => {
                failures = 0;
                set_link(&shared, RemoteLink::Online);
                let ended = serve(&shared, socket, &mut control).await;
                lock(&shared.connected).clear();
                match ended {
                    Ended::Shutdown => return,
                    Ended::Disabled => continue,
                    Ended::Lost(reason) => reason,
                }
            }
            Err(reason) => reason,
        };
        failures = failures.saturating_add(1);
        set_link(&shared, RemoteLink::Offline { reason });
        // 2, 4, 8 ... 60 seconds, and at once when Settings changes.
        let wait = Duration::from_secs(2u64.saturating_pow(failures.min(6)).min(60));
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            message = control.recv() => match message {
                Some(Control::Shutdown) | None => return,
                Some(_) => {}
            },
        }
    }
}

fn set_link(shared: &Shared, link: RemoteLink) {
    let changed = {
        let mut current = lock(&shared.link);
        let changed = *current != link;
        *current = link;
        changed
    };
    if changed {
        (shared.config.changed)();
    }
}

fn owner(cid: &str) -> String {
    format!("{WATCHER}:{cid}")
}

/// One connection to the relay, until it ends.
async fn serve(
    shared: &Arc<Shared>,
    socket: relay::Socket,
    control: &mut UnboundedReceiver<Control>,
) -> Ended {
    let (mut sink, mut stream) = socket.split();
    let (out, mut finished) = unbounded_channel::<Out>();
    let backend = shared.config.backend.clone();
    let mut state = Serving {
        shared: shared.clone(),
        backend: backend.clone(),
        out,
        connections: HashMap::new(),
        bot_calls: Arc::new(AtomicUsize::new(0)),
        links: bot_links(shared),
        outbox: Vec::new(),
    };
    let mut ping = tokio::time::interval(PING_EVERY);
    let mut sweep = tokio::time::interval(SWEEP_EVERY);
    let mut heard = Instant::now();
    state.watch();
    // This socket learns the bot links before anything else.
    state.outbox.push(state.links.clone());

    let ended = loop {
        // Everything a step decided to say goes out before the next step is taken.
        let mut failed = None;
        if !state.outbox.is_empty() {
            for frame in state.outbox.drain(..) {
                if let Err(error) = sink.feed(Message::text(frame)).await {
                    failed = Some(error.to_string());
                    break;
                }
            }
            // One flush for the step's messages, not one per message.
            if failed.is_none()
                && let Err(error) = sink.flush().await
            {
                failed = Some(error.to_string());
            }
        }
        if let Some(reason) = failed {
            break Ended::Lost(reason);
        }
        let ended = tokio::select! {
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    heard = Instant::now();
                    state.relay_frame(text.as_str())
                }
                Some(Ok(Message::Close(_))) | None => {
                    Some(Ended::Lost("The relay closed the connection.".into()))
                }
                Some(Ok(_)) => None,
                Some(Err(error)) => Some(Ended::Lost(error.to_string())),
            },
            Some(done) = finished.recv() => {
                // Whatever else finished meanwhile goes out in this step, frames merged.
                let mut batch = vec![done];
                while batch.len() < DRAIN {
                    match finished.try_recv() {
                        Ok(more) => batch.push(more),
                        Err(_) => break,
                    }
                }
                for done in merge(batch) {
                    state.finished(done);
                }
                None
            }
            message = control.recv() => match message {
                Some(Control::Shutdown) | None => Some(Ended::Shutdown),
                Some(Control::Reset) => {
                    // The browsers are told first; then the other workspace's devices are ended
                    // and the relay learns which bot links are this workspace's.
                    state.reset();
                    let ended = state.refresh();
                    state.watch();
                    ended
                }
                Some(Control::Refresh) => state.refresh(),
            },
            _ = ping.tick() => {
                if heard.elapsed() > SILENCE {
                    Some(Ended::Lost("The relay stopped answering.".into()))
                } else {
                    state.outbox.push("ping".into());
                    None
                }
            }
            _ = sweep.tick() => {
                state.sweep();
                None
            }
        };
        if let Some(ended) = ended {
            break ended;
        }
    };
    for (cid, connection) in state.connections.drain() {
        closed(shared, &*backend, &cid, connection);
    }
    backend.release(WATCHER);
    let _ = sink.close().await;
    ended
}

/// Merges frames that follow each other for the same browser and chat into one, in order: the
/// events of a burst read the same as they would one frame at a time. Anything else between two
/// frames keeps them apart, so nothing is reordered. A merged frame is written once, when sealed.
fn merge(items: Vec<Out>) -> Vec<Out> {
    let mut merged: Vec<Out> = Vec::with_capacity(items.len());
    for item in items {
        if let Out::Frame {
            cid,
            batch,
            json,
            backlog,
        } = item
        {
            if let Some(Out::Frame {
                cid: last_cid,
                batch: last,
                json: last_json,
                backlog: last_backlog,
            }) = merged.last_mut()
                && *last_cid == cid
                && last.chat_id == batch.chat_id
                && last.seq_start as usize + last.events.len() == batch.seq_start as usize
                && *last_backlog + backlog <= MERGED_FRAME
            {
                last.events.extend(batch.events);
                *last_backlog += backlog;
                *last_json = None;
                continue;
            }
            merged.push(Out::Frame {
                cid,
                batch,
                json,
                backlog,
            });
        } else {
            merged.push(item);
        }
    }
    merged
}

/// The state of one connection to the relay. Each method is one step: it never waits, and it
/// leaves what it wants written in `outbox`.
struct Serving {
    shared: Arc<Shared>,
    backend: Arc<dyn Backend>,
    out: UnboundedSender<Out>,
    connections: HashMap<String, Connection>,
    bot_calls: Arc<AtomicUsize>,
    /// The bot links as the relay last heard them.
    links: String,
    outbox: Vec<String>,
}

impl Serving {
    /// Follows every chat's status on behalf of all browsers.
    fn watch(&self) {
        let out = self.out.clone();
        let statuses: Statuses =
            Arc::new(move |update| out.send(Out::Status(update.clone())).is_ok());
        // Before a profile is open there is nothing to watch; a reset watches again.
        let scope = lock(&self.shared.registry).scope().to_owned();
        let _ = self.backend.clone().bound(&scope).watch(WATCHER, statuses);
    }

    /// Ends a browser's session and tells the relay to drop its socket.
    fn drop_connection(&mut self, cid: &str) {
        if let Some(connection) = self.connections.remove(cid) {
            closed(&self.shared, &*self.backend, cid, connection);
        }
        self.outbox.push(format!("c{cid}"));
    }

    fn relay_frame(&mut self, text: &str) -> Option<Ended> {
        match wire::from_relay(text)? {
            FromRelay::Pong => {}
            FromRelay::Replaced => {
                return Some(Ended::Lost(
                    "Another o7 took over this computer's remote access.".into(),
                ));
            }
            FromRelay::Opened(cid) => {
                if self.connections.len() >= MAX_CONNECTIONS {
                    self.outbox
                        .push(format!("m{cid} R{}", refusal(Refusal::Busy)));
                    self.outbox.push(format!("c{cid}"));
                } else {
                    self.connections.insert(
                        cid.to_owned(),
                        Connection {
                            since: Instant::now(),
                            session: None,
                        },
                    );
                }
            }
            FromRelay::Closed(cid) => {
                if let Some(connection) = self.connections.remove(cid) {
                    closed(&self.shared, &*self.backend, cid, connection);
                }
            }
            FromRelay::Bot(body) => self.bot(body),
            FromRelay::Message { cid, payload } => {
                let Some(connection) = self.connections.get_mut(cid) else {
                    self.outbox.push(format!("c{cid}"));
                    return None;
                };
                match receive(
                    &self.shared,
                    &self.backend,
                    &self.out,
                    cid,
                    connection,
                    payload,
                ) {
                    Received::Send(frames) => self.outbox.extend(frames),
                    Received::Refuse(last) => {
                        self.outbox.extend(last);
                        self.drop_connection(cid);
                    }
                }
            }
        }
        None
    }

    /// A bot's tool call: checked against the link's token here, whatever the relay checked.
    fn bot(&mut self, body: &str) {
        let Ok(call) = serde_json::from_str::<BotCall>(body) else {
            return;
        };
        // The token and the workspace it is good for, read together.
        let (verified, scope, full, first) = {
            let mut registry = lock(&self.shared.registry);
            let unused = registry
                .bots()
                .iter()
                .any(|bot| bot.id == call.bot && bot.last_used_at.is_none());
            let verified = registry.verify_bot(&call.bot, &call.token, now());
            (
                verified,
                registry.scope().to_owned(),
                registry.bot_full(&call.bot),
                verified && unused,
            )
        };
        // A link's first call: Settings, which may still show the link's address, says so.
        if first {
            (self.shared.config.changed)();
        }
        let refused = if !verified {
            Some("This bot link was removed. Its owner can create a new one in o7.")
        } else if !bot::known(&call.tool) {
            Some("There is no such tool.")
        } else if !bot::allowed(&call.tool, full) {
            // Whatever the relay listed or let through: the link's own level decides, here.
            Some(bot::NOT_ALLOWED)
        } else if self.bot_calls.load(Ordering::Relaxed) >= MAX_BOT_CALLS {
            Some("Too many calls are running. Wait for one to finish.")
        } else {
            None
        };
        if let Some(text) = refused {
            self.outbox.push(bot_answer(&call.rid, text, true));
            return;
        }
        self.bot_calls.fetch_add(1, Ordering::Relaxed);
        let (backend, out, running, shared) = (
            self.backend.clone().bound(&scope),
            self.out.clone(),
            self.bot_calls.clone(),
            self.shared.clone(),
        );
        tokio::spawn(async move {
            let owner = format!("{WATCHER}-bot:{}", call.rid);
            let answer = bot::call(&backend, &owner, &call.tool, &call.args, full).await;
            running.fetch_sub(1, Ordering::Relaxed);
            if answer.error {
                lock(&shared.registry).bot_failed(&call.bot);
            } else if let Some(chat) = &answer.chat {
                // The bot has heard how this chat stands, and a chat it acted on is its to hear
                // about from now on.
                if answer.acted {
                    lock(&shared.registry).follow(&call.bot, chat);
                }
                lock(&shared.heard).insert((call.bot.clone(), chat.clone()), now());
            }
            let _ = out.send(Out::Bot {
                rid: call.rid,
                answer,
            });
        });
    }

    /// Work that finished beside the connection task: seal it in order and send it.
    fn finished(&mut self, done: Out) {
        match done {
            Out::Sealed { cid, json, backlog } => {
                let Some(session) = self
                    .connections
                    .get_mut(&cid)
                    .and_then(|connection| connection.session.as_mut())
                else {
                    return;
                };
                let held = session.backlog.load(Ordering::Relaxed);
                session
                    .backlog
                    .fetch_sub(backlog.min(held), Ordering::Relaxed);
                match session.channel.seal(json.as_bytes()) {
                    Ok(sealed) => self.outbox.push(format!("m{cid} E{sealed}")),
                    Err(_) => self.drop_connection(&cid),
                }
            }
            Out::Frame {
                cid,
                batch,
                json,
                backlog,
            } => {
                let json =
                    json.or_else(|| serde_json::to_string(&Event::Frame { batch: &batch }).ok());
                // A frame that cannot be written is a gap: the browser reads the chat again.
                let json = json.unwrap_or_else(|| {
                    serde_json::to_string(&Event::Lost {
                        chat_id: &batch.chat_id,
                    })
                    .unwrap_or_default()
                });
                self.finished(Out::Sealed { cid, json, backlog });
            }
            Out::Drop { cid } => {
                if self.connections.contains_key(&cid) {
                    self.drop_connection(&cid);
                }
                // A device that removed itself is gone everywhere, not only where it asked.
                self.end_removed_devices();
            }
            Out::Status(update) => {
                let Ok(json) = serde_json::to_string(&Event::Chat { update: &update }) else {
                    return;
                };
                let mut failed = Vec::new();
                for (cid, connection) in &mut self.connections {
                    let Some(session) = connection
                        .session
                        .as_mut()
                        .filter(|session| session.proven && session.device.is_some())
                    else {
                        continue;
                    };
                    match session.channel.seal(json.as_bytes()) {
                        Ok(sealed) => self.outbox.push(format!("m{cid} E{sealed}")),
                        Err(_) => failed.push(cid.clone()),
                    }
                }
                for cid in failed {
                    self.drop_connection(&cid);
                }
            }
            Out::Bot { rid, answer } => {
                self.outbox
                    .push(bot_answer(&rid, &answer.text, answer.error));
            }
        }
    }

    /// Another profile was opened: nothing a browser followed exists any more. The registry is
    /// already scoped to it, so the caller's refresh ends the devices that are not its own, and
    /// only then are the new profile's chats watched.
    fn reset(&mut self) {
        for (cid, connection) in &mut self.connections {
            self.backend.release(&owner(cid));
            let Some(session) = connection
                .session
                .as_mut()
                .filter(|session| session.proven && session.device.is_some())
            else {
                continue;
            };
            session.backlog.store(0, Ordering::Relaxed);
            if let Ok(sealed) = seal(&mut session.channel, &Event::Reset) {
                self.outbox.push(format!("m{cid} E{sealed}"));
            }
        }
    }

    /// Settings changed: stop if remote access is off, tell the relay the bot links, and end the
    /// sessions of removed devices now rather than at their next message.
    fn refresh(&mut self) -> Option<Ended> {
        if !lock(&self.shared.registry).enabled() {
            return Some(Ended::Disabled);
        }
        let links = bot_links(&self.shared);
        if links != self.links {
            self.outbox.push(links.clone());
            self.links = links;
        }
        self.end_removed_devices();
        None
    }

    /// Ends every session whose device is no longer paired.
    fn end_removed_devices(&mut self) {
        let gone: Vec<String> = {
            let registry = lock(&self.shared.registry);
            self.connections
                .iter()
                .filter(|(_, connection)| {
                    connection
                        .session
                        .as_ref()
                        .and_then(|session| session.device.as_deref())
                        .is_some_and(|device| registry.device_key(device).is_none())
                })
                .map(|(cid, _)| cid.clone())
                .collect()
        };
        for cid in gone {
            self.drop_connection(&cid);
        }
    }

    /// Ends connections that never proved they hold a key.
    fn sweep(&mut self) {
        let late: Vec<String> = self
            .connections
            .iter()
            .filter(|(_, connection)| {
                connection.since.elapsed() > PROVE_WITHIN
                    && !connection
                        .session
                        .as_ref()
                        .is_some_and(|session| session.proven)
            })
            .map(|(cid, _)| cid.clone())
            .collect();
        for cid in late {
            self.drop_connection(&cid);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use o7_protocol::{EventBatch, O7Event};

    use super::{MERGED_FRAME, Out, merge};

    fn frame(cid: &str, chat: &str, seq_start: u32, texts: &[&str], backlog: usize) -> Out {
        Out::Frame {
            cid: cid.into(),
            batch: EventBatch {
                chat_id: chat.into(),
                seq_start,
                events: texts
                    .iter()
                    .map(|text| O7Event::MessageDelta {
                        message_id: "m1".into(),
                        text: (*text).into(),
                    })
                    .collect(),
            },
            json: Some("alone".into()),
            backlog,
        }
    }

    /// Each item as (browser, chat, first seq, texts, written alone) or a marker for the rest.
    fn read(items: &[Out]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                Out::Frame {
                    cid,
                    batch,
                    json,
                    backlog,
                } => {
                    let texts: Vec<String> = batch
                        .events
                        .iter()
                        .map(|event| match event {
                            O7Event::MessageDelta { text, .. } => text.clone(),
                            _ => "?".into(),
                        })
                        .collect();
                    format!(
                        "{cid} {} {} {} {backlog}{}",
                        batch.chat_id,
                        batch.seq_start,
                        texts.join(""),
                        if json.is_some() { " alone" } else { "" }
                    )
                }
                Out::Sealed { cid, json, .. } => format!("sealed {cid} {json}"),
                Out::Drop { cid } => format!("drop {cid}"),
                _ => "other".into(),
            })
            .collect()
    }

    #[test]
    fn a_burst_of_frames_reads_as_one_in_order() {
        let merged = merge(vec![
            frame("a", "c1", 5, &["Hel"], 10),
            frame("a", "c1", 6, &["lo", " th"], 10),
            frame("a", "c1", 8, &["ere"], 10),
        ]);
        assert_eq!(read(&merged), ["a c1 5 Hello there 30"]);
    }

    #[test]
    fn anything_between_frames_keeps_them_apart_and_in_order() {
        let merged = merge(vec![
            frame("a", "c1", 5, &["one"], 10),
            Out::Sealed {
                cid: "a".into(),
                json: "reply".into(),
                backlog: 0,
            },
            frame("a", "c1", 6, &["two"], 10),
            // Another browser, another chat, a gap in the events: none of these join.
            frame("b", "c1", 7, &["three"], 10),
            frame("b", "c2", 8, &["four"], 10),
            frame("b", "c2", 10, &["five"], 10),
            Out::Drop { cid: "b".into() },
        ]);
        assert_eq!(
            read(&merged),
            [
                "a c1 5 one 10 alone",
                "sealed a reply",
                "a c1 6 two 10 alone",
                "b c1 7 three 10 alone",
                "b c2 8 four 10 alone",
                "b c2 10 five 10 alone",
                "drop b",
            ]
        );
    }

    #[test]
    fn a_merged_frame_stays_under_its_size() {
        let half = MERGED_FRAME / 2;
        let merged = merge(vec![
            frame("a", "c1", 1, &["x"], half),
            frame("a", "c1", 2, &["y"], half),
            frame("a", "c1", 3, &["z"], 1),
        ]);
        assert_eq!(
            read(&merged),
            [
                format!("a c1 1 xy {MERGED_FRAME}"),
                "a c1 3 z 1 alone".into()
            ]
        );
    }
}
