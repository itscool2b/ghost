//! The bridge to a running game: Ghost's side of the connection the game's own bridge script
//! opens (`resources/godot/bridge.gd`).
//!
//! Ghost listens on loopback for one run. The game connects out, and the first thing it sends
//! must be the run's token: a connection that says anything else is dropped. After that Ghost
//! asks and the game answers. The game is not trusted: every message is bounded, an answer is
//! matched to the question it names and to nothing else, and whatever it says is data.
//!
//! Messages are frames of `[u32 length, little-endian][u8 kind][payload]`; the length counts the
//! kind byte. Kind 1 is JSON, 2 a JPEG frame for the pane, 3 a picture that answers a request.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot, watch};

use crate::Error;

const KIND_JSON: u8 = 1;
const KIND_FRAME: u8 = 2;
const KIND_IMAGE: u8 = 3;

/// The most one message from the game may hold: a picture at full size with room to spare.
pub const MAX_MESSAGE: usize = 8 * 1024 * 1024;
/// The most pictures one answer may carry.
const MAX_PICTURES: usize = 4;
/// How long a connection has to say hello.
const HELLO_PATIENCE: Duration = Duration::from_secs(5);

/// What the game says about itself when it connects.
#[derive(Debug, Clone, PartialEq)]
pub struct Hello {
    pub engine: String,
    pub version: String,
    pub width: u32,
    pub height: u32,
    pub pid: u32,
    /// The game window's native handle, where the engine gives one.
    pub window: i64,
    pub project: String,
    pub renderer: String,
    /// Controllers the game itself sees on this computer.
    pub joypads: u32,
}

/// One picture of the game for the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub number: u32,
    pub width: u16,
    pub height: u16,
    /// The game was frozen when it was taken.
    pub frozen: bool,
    pub jpeg: Vec<u8>,
}

/// A picture that answers a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u16,
    pub height: u16,
    pub jpeg: Vec<u8>,
}

/// The game's answer to one request.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub value: Value,
    pub pictures: Vec<Picture>,
}

/// What the game says unasked.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Twice a second: its frame rate, whether it is frozen, its scene.
    Stats(Value),
    /// An error or warning of its own, with its file and line where it has them.
    Error(Value),
    /// The connection ended: the game quit or crashed.
    Closed,
}

fn random_token() -> String {
    // Two time-ordered ids hold 148 random bits between them, as the other grants' tokens do.
    format!(
        "{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    )
}

/// Compares in time that does not depend on where the two differ.
fn same_token(given: &str, expected: &str) -> bool {
    given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// Waiting for one run's game to connect.
pub struct Listening {
    listener: TcpListener,
    port: u16,
    token: String,
}

impl Listening {
    /// Listens on a loopback port of the system's choosing, with a fresh token.
    pub async fn bind() -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener,
            port,
            token: random_token(),
        })
    }

    /// What the game is given to find its way back: `GHOST_GAME_BRIDGE`.
    pub fn address(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// What the game must say first: `GHOST_GAME_TOKEN`.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Waits for the game. A connection that does not say hello with the token within five
    /// seconds is dropped and the wait goes on, until `patience` runs out. `on_event` hears what
    /// the game says unasked from then on, and once that the connection ended.
    pub async fn accept(
        self,
        patience: Duration,
        on_event: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<(Bridge, Hello), Error> {
        let deadline = tokio::time::Instant::now() + patience;
        loop {
            let (stream, peer) = tokio::time::timeout_at(deadline, self.listener.accept())
                .await
                .map_err(|_| Error::Timeout)??;
            if !peer.ip().is_loopback() {
                continue;
            }
            let _ = stream.set_nodelay(true);
            let (mut read, write) = stream.into_split();
            let first = tokio::time::timeout(HELLO_PATIENCE, read_frame(&mut read)).await;
            let Ok(Ok((KIND_JSON, payload))) = first else {
                continue;
            };
            let Ok(message) = serde_json::from_slice::<Value>(&payload) else {
                continue;
            };
            if !message["hello"]
                .as_str()
                .is_some_and(|given| same_token(given, &self.token))
            {
                continue;
            }
            let text = |key: &str| {
                message[key]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(120)
                    .collect::<String>()
            };
            let number = |key: &str| whole(&message[key]).unwrap_or(0);
            let hello = Hello {
                engine: text("engine"),
                version: text("version"),
                width: number("width").clamp(1, 16384),
                height: number("height").clamp(1, 16384),
                pid: number("pid"),
                window: message["window"].as_i64().unwrap_or(0),
                project: text("project"),
                renderer: text("renderer"),
                joypads: number("joypads").min(16),
            };
            return Ok((Bridge::start(read, write, Arc::new(on_event)), hello));
        }
    }
}

async fn read_frame(read: &mut OwnedReadHalf) -> std::io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    read.read_exact(&mut head).await?;
    let length = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
    if length == 0 || length > MAX_MESSAGE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "a message of a size the bridge does not allow",
        ));
    }
    let mut payload = vec![0u8; length - 1];
    read.read_exact(&mut payload).await?;
    Ok((head[4], payload))
}

fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.extend_from_slice(&(payload.len() as u32 + 1).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

fn json_frame(message: &Value) -> Vec<u8> {
    frame(KIND_JSON, message.to_string().as_bytes())
}

struct Pending {
    answer: oneshot::Sender<Result<Reply, Error>>,
    /// The JSON answer, held while its pictures are still to come.
    value: Option<Value>,
    wanted: usize,
    pictures: Vec<Picture>,
}

type Waiting = Arc<Mutex<HashMap<u32, Pending>>>;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The connection to one running game.
pub struct Bridge {
    out: mpsc::Sender<Vec<u8>>,
    waiting: Waiting,
    next: AtomicU32,
    frames: watch::Receiver<Option<Arc<Frame>>>,
    closed: Arc<AtomicBool>,
    tasks: [tokio::task::JoinHandle<()>; 2],
}

impl Drop for Bridge {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Bridge {
    fn start(
        read: OwnedReadHalf,
        mut write: OwnedWriteHalf,
        on_event: Arc<dyn Fn(Event) + Send + Sync>,
    ) -> Self {
        let (out, mut outbox) = mpsc::channel::<Vec<u8>>(64);
        let waiting: Waiting = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (frame_tx, frames) = watch::channel(None);
        let writer = tokio::spawn(async move {
            while let Some(bytes) = outbox.recv().await {
                if write.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        });
        let reader = tokio::spawn(read_loop(
            read,
            out.clone(),
            Arc::clone(&waiting),
            frame_tx,
            Arc::clone(&closed),
            on_event,
        ));
        Self {
            out,
            waiting,
            next: AtomicU32::new(1),
            frames,
            closed,
            tasks: [writer, reader],
        }
    }

    /// Whether the game has gone.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// The newest frame for the pane, replaced as each arrives.
    pub fn frames(&self) -> watch::Receiver<Option<Arc<Frame>>> {
        self.frames.clone()
    }

    /// Asks the game to do `what` and waits for its answer and the pictures it names.
    pub async fn ask(&self, what: &str, fields: Value, patience: Duration) -> Result<Reply, Error> {
        if self.is_closed() {
            return Err(Error::Bridge("The game is not running any more.".into()));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut message = match fields {
            Value::Object(map) => Value::Object(map),
            _ => json!({}),
        };
        message["id"] = json!(id);
        message["do"] = json!(what);
        let (answer, answered) = oneshot::channel();
        lock(&self.waiting).insert(
            id,
            Pending {
                answer,
                value: None,
                wanted: 0,
                pictures: Vec::new(),
            },
        );
        if self.out.send(json_frame(&message)).await.is_err() {
            lock(&self.waiting).remove(&id);
            return Err(Error::Bridge("The game is not running any more.".into()));
        }
        match tokio::time::timeout(patience, answered).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Error::Bridge("The game ended before it answered.".into())),
            Err(_) => {
                lock(&self.waiting).remove(&id);
                Err(Error::Timeout)
            }
        }
    }

    /// Tells the game something it does not answer (input from the pane). Dropped when the game
    /// is not keeping up: late input is worse than none.
    pub fn tell(&self, what: &str, fields: Value) {
        let mut message = match fields {
            Value::Object(map) => Value::Object(map),
            _ => json!({}),
        };
        message["do"] = json!(what);
        let _ = self.out.try_send(json_frame(&message));
    }
}

/// A whole number the game sent, which an engine's JSON may have written as `7` or as `7.0`.
fn whole(value: &Value) -> Option<u32> {
    if let Some(number) = value.as_u64() {
        return u32::try_from(number).ok();
    }
    let number = value.as_f64()?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (number >= 0.0 && number <= f64::from(u32::MAX) && number.fract() == 0.0)
        .then_some(number as u32)
}

fn settle(waiting: &Waiting, id: u32) {
    let mut waiting = lock(waiting);
    let ready = waiting
        .get(&id)
        .is_some_and(|pending| pending.value.is_some() && pending.pictures.len() >= pending.wanted);
    if ready && let Some(pending) = waiting.remove(&id) {
        let _ = pending.answer.send(Ok(Reply {
            value: pending.value.unwrap_or(Value::Null),
            pictures: pending.pictures,
        }));
    }
}

async fn read_loop(
    mut read: OwnedReadHalf,
    out: mpsc::Sender<Vec<u8>>,
    waiting: Waiting,
    frames: watch::Sender<Option<Arc<Frame>>>,
    closed: Arc<AtomicBool>,
    on_event: Arc<dyn Fn(Event) + Send + Sync>,
) {
    while let Ok((kind, payload)) = read_frame(&mut read).await {
        match kind {
            KIND_JSON => {
                let Ok(message) = serde_json::from_slice::<Value>(&payload) else {
                    break;
                };
                if let Some(event) = message["event"].as_str() {
                    match event {
                        "stats" => on_event(Event::Stats(message)),
                        "error" => on_event(Event::Error(message)),
                        _ => {}
                    }
                    continue;
                }
                let Some(id) = whole(&message["id"]) else {
                    continue;
                };
                if let Some(why) = message["error"].as_str() {
                    if let Some(pending) = lock(&waiting).remove(&id) {
                        let why: String = why.chars().take(600).collect();
                        let _ = pending.answer.send(Err(Error::Game(why)));
                    }
                    continue;
                }
                if let Some(pending) = lock(&waiting).get_mut(&id) {
                    pending.wanted =
                        (whole(&message["images"]).unwrap_or(0) as usize).min(MAX_PICTURES);
                    pending.value = Some(message);
                }
                settle(&waiting, id);
            }
            KIND_FRAME if payload.len() > 9 => {
                let number = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let frame = Frame {
                    number,
                    width: u16::from_le_bytes([payload[4], payload[5]]),
                    height: u16::from_le_bytes([payload[6], payload[7]]),
                    frozen: payload[8] != 0,
                    jpeg: payload[9..].to_vec(),
                };
                let _ = frames.send(Some(Arc::new(frame)));
                // The game sends its next frame only once this one is acknowledged, so a slow
                // pane slows the pictures and never the game.
                let _ = out.try_send(json_frame(&json!({"do": "ack", "frame": number})));
            }
            KIND_IMAGE if payload.len() > 8 => {
                let id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                if let Some(pending) = lock(&waiting).get_mut(&id)
                    && pending.pictures.len() < MAX_PICTURES
                {
                    pending.pictures.push(Picture {
                        width: u16::from_le_bytes([payload[4], payload[5]]),
                        height: u16::from_le_bytes([payload[6], payload[7]]),
                        jpeg: payload[8..].to_vec(),
                    });
                }
                settle(&waiting, id);
            }
            _ => {}
        }
    }
    closed.store(true, Ordering::Release);
    for (_, pending) in lock(&waiting).drain() {
        let _ = pending.answer.send(Err(Error::Bridge(
            "The game ended before it answered.".into(),
        )));
    }
    on_event(Event::Closed);
}

#[cfg(test)]
pub(crate) mod testing {
    //! A stand-in for a game: the other end of the bridge, for this crate's tests.
    use super::*;
    use tokio::net::TcpStream;

    pub struct FakeGame {
        pub read: OwnedReadHalf,
        pub write: OwnedWriteHalf,
    }

    impl FakeGame {
        pub async fn connect(address: &str) -> Self {
            let stream = TcpStream::connect(address).await.unwrap();
            let (read, write) = stream.into_split();
            Self { read, write }
        }

        pub async fn send_json(&mut self, message: Value) {
            self.write.write_all(&json_frame(&message)).await.unwrap();
        }

        pub async fn send(&mut self, kind: u8, payload: &[u8]) {
            self.write.write_all(&frame(kind, payload)).await.unwrap();
        }

        pub async fn hello(&mut self, token: &str) {
            self.send_json(json!({
                "hello": token, "engine": "godot", "version": "4.7.2-stable (official)",
                "width": 960, "height": 540, "pid": 4242, "window": 99, "project": "Fixture",
                "renderer": "opengl3",
            }))
            .await;
        }

        /// The next JSON message Ghost sent.
        pub async fn next(&mut self) -> Value {
            loop {
                let (kind, payload) = read_frame(&mut self.read).await.unwrap();
                if kind == KIND_JSON {
                    return serde_json::from_slice(&payload).unwrap();
                }
            }
        }

        /// A picture of `bytes` answering request `id`.
        pub async fn picture(&mut self, id: u64, bytes: &[u8]) {
            let mut payload = (id as u32).to_le_bytes().to_vec();
            payload.extend_from_slice(&64u16.to_le_bytes());
            payload.extend_from_slice(&36u16.to_le_bytes());
            payload.extend_from_slice(bytes);
            self.send(KIND_IMAGE, &payload).await;
        }

        /// Frame `number` for the pane.
        pub async fn frame(&mut self, number: u32, frozen: bool) {
            let mut payload = number.to_le_bytes().to_vec();
            payload.extend_from_slice(&960u16.to_le_bytes());
            payload.extend_from_slice(&540u16.to_le_bytes());
            payload.push(u8::from(frozen));
            payload.extend_from_slice(b"jpeg");
            self.send(KIND_FRAME, &payload).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeGame;
    use super::*;

    async fn connected() -> (Bridge, Hello, FakeGame, Arc<Mutex<Vec<Event>>>) {
        let listening = Listening::bind().await.unwrap();
        let address = listening.address();
        let token = listening.token().to_owned();
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let heard = Arc::clone(&events);
        let accepting = tokio::spawn(listening.accept(Duration::from_secs(5), move |event| {
            lock(&heard).push(event);
        }));
        let mut game = FakeGame::connect(&address).await;
        game.hello(&token).await;
        let (bridge, hello) = accepting.await.unwrap().unwrap();
        (bridge, hello, game, events)
    }

    #[tokio::test]
    async fn only_a_game_that_shows_the_runs_token_is_taken() {
        let listening = Listening::bind().await.unwrap();
        let address = listening.address();
        assert!(address.starts_with("127.0.0.1:"));
        let token = listening.token().to_owned();
        assert!(token.len() >= 60, "a long random token");
        let accepting = tokio::spawn(listening.accept(Duration::from_secs(5), |_| {}));
        // The wrong token, something that is not JSON, and no token at all: each is dropped.
        let mut wrong = FakeGame::connect(&address).await;
        wrong.hello("not-the-token").await;
        let mut junk = FakeGame::connect(&address).await;
        junk.send(KIND_JSON, b"{ not json").await;
        let mut silent = FakeGame::connect(&address).await;
        silent.send_json(json!({"engine": "godot"})).await;
        let mut right = FakeGame::connect(&address).await;
        right.hello(&token).await;
        let (bridge, hello) = accepting.await.unwrap().unwrap();
        assert_eq!(hello.engine, "godot");
        assert_eq!((hello.width, hello.height, hello.pid), (960, 540, 4242));
        assert_eq!(hello.project, "Fixture");
        assert!(!bridge.is_closed());
        assert!(
            !same_token("abc", "abd") && !same_token("abc", "abcd") && same_token("abc", "abc")
        );
    }

    #[tokio::test]
    async fn nobody_connecting_runs_out_of_patience() {
        let listening = Listening::bind().await.unwrap();
        let waited = listening.accept(Duration::from_millis(50), |_| {}).await;
        assert!(matches!(waited, Err(Error::Timeout)));
    }

    #[tokio::test]
    async fn an_answer_goes_to_its_own_question_with_its_pictures() {
        let (bridge, _, mut game, _) = connected().await;
        let asking = tokio::spawn(async move {
            let reply = bridge
                .ask("act", json!({"steps": []}), Duration::from_secs(5))
                .await;
            (bridge, reply)
        });
        let request = game.next().await;
        assert_eq!(request["do"], "act");
        let id = request["id"].as_u64().unwrap();
        // An answer to a question nobody asked, and a picture for it, go nowhere.
        game.send_json(json!({"id": id + 100, "ok": true})).await;
        game.picture(id + 100, b"stray").await;
        // Godot writes whole numbers that came from JSON as floats: an id of 7 comes back 7.0.
        assert_eq!(whole(&json!(7.0)), Some(7));
        assert_eq!(whole(&json!(7)), Some(7));
        for not_whole in [json!(7.5), json!(-1), json!("7"), json!(null), json!(1e12)] {
            assert_eq!(whole(&not_whole), None, "{not_whole}");
        }
        #[allow(clippy::cast_precision_loss)]
        let as_float = id as f64;
        game.send_json(json!({"id": as_float, "ok": true, "images": 2.0, "frames": 30}))
            .await;
        game.picture(id, b"strip").await;
        game.picture(id, b"final").await;
        let (bridge, reply) = asking.await.unwrap();
        let reply = reply.unwrap();
        assert_eq!(reply.value["frames"], 30);
        assert_eq!(reply.pictures.len(), 2);
        assert_eq!(reply.pictures[0].jpeg, b"strip");
        assert_eq!(reply.pictures[1].jpeg, b"final");
        assert_eq!(
            (reply.pictures[0].width, reply.pictures[0].height),
            (64, 36)
        );

        // A refusal comes back as the game's own words.
        let asking = tokio::spawn(async move {
            bridge
                .ask("inspect", json!({"node": "g9"}), Duration::from_secs(5))
                .await
        });
        let request = game.next().await;
        game.send_json(json!({"id": request["id"], "error": "No such node now."}))
            .await;
        let refused = asking.await.unwrap().unwrap_err();
        assert_eq!(refused.to_string(), "No such node now.");
    }

    #[tokio::test]
    async fn a_frame_replaces_the_last_and_is_acknowledged() {
        let (bridge, _, mut game, events) = connected().await;
        let mut frames = bridge.frames();
        assert!(frames.borrow().is_none());
        game.frame(1, false).await;
        game.frame(2, true).await;
        assert_eq!(game.next().await, json!({"do": "ack", "frame": 1}));
        assert_eq!(game.next().await, json!({"do": "ack", "frame": 2}));
        frames.changed().await.unwrap();
        let newest = frames.borrow_and_update().clone().unwrap();
        assert_eq!((newest.number, newest.frozen), (2, true));
        assert_eq!((newest.width, newest.height), (960, 540));
        assert_eq!(newest.jpeg, b"jpeg");

        game.send_json(json!({"event": "stats", "fps": 60})).await;
        game.send_json(json!({"event": "error", "text": "boom"}))
            .await;
        game.send_json(json!({"event": "made-up"})).await;
        bridge.tell("input", json!({"events": []}));
        assert_eq!(game.next().await["do"], "input");
        // The reader hears them in its own time.
        for _ in 0..100 {
            if lock(&events).len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        let heard = lock(&events).clone();
        assert!(matches!(&heard[0], Event::Stats(value) if value["fps"] == 60));
        assert!(matches!(&heard[1], Event::Error(value) if value["text"] == "boom"));
        assert_eq!(
            heard.len(),
            2,
            "an event Ghost does not know is not passed on"
        );
    }

    #[tokio::test]
    async fn a_game_that_goes_fails_what_was_waiting_and_says_so_once() {
        let (bridge, _, mut game, events) = connected().await;
        let bridge = Arc::new(bridge);
        let asker = Arc::clone(&bridge);
        let asking =
            tokio::spawn(async move { asker.ask("tree", json!({}), Duration::from_secs(5)).await });
        let _ = game.next().await;
        drop(game);
        let failed = asking.await.unwrap().unwrap_err();
        assert_eq!(failed.to_string(), "The game ended before it answered.");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(bridge.is_closed());
        assert_eq!(lock(&events).as_slice(), [Event::Closed]);
        let after = bridge.ask("tree", json!({}), Duration::from_secs(1)).await;
        assert_eq!(
            after.unwrap_err().to_string(),
            "The game is not running any more."
        );
    }

    #[tokio::test]
    async fn a_message_of_a_size_the_bridge_does_not_allow_ends_the_connection() {
        let (bridge, _, mut game, _) = connected().await;
        let mut head = ((MAX_MESSAGE as u32) + 2).to_le_bytes().to_vec();
        head.push(KIND_JSON);
        game.write.write_all(&head).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(bridge.is_closed());
    }

    #[tokio::test]
    async fn an_unanswered_question_times_out_and_is_forgotten() {
        let (bridge, _, mut game, _) = connected().await;
        let waited = bridge
            .ask("tree", json!({}), Duration::from_millis(30))
            .await;
        assert!(matches!(waited, Err(Error::Timeout)));
        // Its late answer is for nobody.
        let request = game.next().await;
        game.send_json(json!({"id": request["id"], "ok": true}))
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(lock(&bridge.waiting).is_empty());
    }
}
