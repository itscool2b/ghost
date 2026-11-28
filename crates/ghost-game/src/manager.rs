//! Every chat's game: which engine its folder holds, the Godot that runs it, the running game and
//! its bridge, and what the pane shows of it all.
//!
//! One game a chat, and a few running at once. A game belongs to the chat that started it: its
//! folder is the chat's own, given by whoever owns the chat and never by a tool's arguments. A
//! game nobody has used for ten minutes is stopped, and every game ends when Ghost does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::bridge::{Bridge, Event, Frame, Hello, Listening, Reply};
use crate::detect::{GameEngine, GameProject};
use crate::godot::{self, Found};
use crate::output::Output;
use crate::run::{self, Launch, Running};
use crate::{Error, tools};

pub use crate::output::GameError;

/// How many games run at once unless the configuration says otherwise.
pub const DEFAULT_RUNNING: usize = 2;
/// A game nobody watched or used for this long is stopped.
pub const IDLE: Duration = Duration::from_secs(600);
/// How long a starting game has to connect. A project's first start imports its assets.
const START_PATIENCE: Duration = Duration::from_secs(120);
/// What a tool is told when the person has turned Game off.
pub const TURNED_OFF: &str = "Game is turned off in Ghost (Settings > Plugins & MCP > Universal plugins > Game). Ask the person to turn it on, or work without it.";

pub struct Config {
    /// Ghost's folder for games: Godot when it is fetched, and Ghost's own scripts.
    pub root: PathBuf,
    pub max_running: usize,
}

/// What the pane shows. Sent whole on every change.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct GameStatus {
    pub enabled: bool,
    pub godot: GodotStatus,
    pub chats: Vec<ChatGame>,
    /// Chats an agent's tool asked to be shown, newest count each: the pane opens for them.
    pub summons: Vec<GameSummon>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct GameSummon {
    pub chat_id: String,
    pub seq: u32,
}

/// Godot on this computer.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct GodotStatus {
    /// The release Ghost fetches.
    pub pinned: String,
    /// Its download, in bytes; 0 where Godot publishes none for this computer.
    pub size: u32,
    /// Ghost's own copy is here.
    pub fetched: bool,
    /// Versions the person installed themselves that Ghost can use.
    pub found: Vec<String>,
    /// The computer has been looked over at least once.
    pub looked: bool,
    pub fetch: GodotFetch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GodotFetch {
    Idle,
    Running { received: u32, total: u32 },
    Unpacking,
    Failed { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum GameState {
    /// Nothing runs.
    Idle,
    Starting,
    Running,
    /// It ran and ended; `ended` says how.
    Ended,
}

/// One chat's game.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct ChatGame {
    pub chat_id: String,
    /// What the chat's folder holds, once it has been looked at.
    pub project: Option<GameProject>,
    pub state: GameState,
    /// Frozen by Ghost: an agent is between actions, or the person paused it.
    pub frozen: bool,
    pub speed: f32,
    pub fps: f32,
    /// The scene that runs, as a path inside the project.
    pub scene: String,
    pub width: u32,
    pub height: u32,
    /// The engine that runs it, as it says ("4.7.2-stable (official)").
    pub engine_version: String,
    /// Errors of this run, repeats counted; warnings apart.
    pub errors: u32,
    pub last_error: Option<GameError>,
    /// Why it ended, in words, when it has.
    pub ended: Option<String>,
    /// What an agent is doing with it now ("Holding ui_right"), for the pane.
    pub agent: Option<String>,
    pub sound: bool,
    pub pid: Option<u32>,
    /// Why no Godot on this computer runs the project, in words for the person: there is none
    /// yet, or the project is newer than every one here. `None` when one does, and for projects
    /// Ghost does not run itself.
    pub godot_missing: Option<String>,
    /// The person asked for the game in a window of its own; its next start shows one.
    pub windowed: bool,
    /// The chat's agent has the game tools in the session that runs now. Without them the pane
    /// offers to restart the session with them.
    pub tools: bool,
    /// Controllers the game itself reads on this computer. When it reads one, the pane sends it
    /// none of its own.
    pub joypads: u32,
}

/// A run's errors and what it printed, for the pane's log.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct GameLog {
    pub errors: Vec<GameError>,
    /// The last lines the game printed, oldest first.
    pub lines: Vec<String>,
    /// How many older lines are gone.
    pub dropped: u32,
}

/// Input from the pane, in the shape the game's bridge takes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum GameInput {
    /// A key by its browser `code` ("KeyA", "ArrowLeft").
    Key {
        code: String,
        down: bool,
        #[serde(default)]
        text: String,
        #[serde(default)]
        shift: bool,
        #[serde(default)]
        ctrl: bool,
        #[serde(default)]
        alt: bool,
    },
    /// A mouse button, at a place in the game's own pixels.
    Mouse {
        x: f32,
        y: f32,
        button: String,
        down: bool,
    },
    Move {
        x: f32,
        y: f32,
        #[serde(default)]
        dx: f32,
        #[serde(default)]
        dy: f32,
    },
    Wheel {
        x: f32,
        y: f32,
        dy: f32,
    },
    /// A controller's button, by Godot's number for it (0 is the bottom face button).
    Pad {
        button: u8,
        down: bool,
    },
    /// A controller's stick or trigger: Godot's axis number and where it stands, -1 to 1.
    Axis {
        axis: u8,
        value: f32,
    },
}

impl GameInput {
    /// Whether it is input a person could have made: short names, real numbers.
    fn sane(&self) -> bool {
        let finite = |numbers: &[f32]| numbers.iter().all(|n| n.is_finite() && n.abs() < 1.0e6);
        match self {
            GameInput::Key { code, text, .. } => {
                !code.is_empty()
                    && code.len() <= 32
                    && code.chars().all(|ch| ch.is_ascii_alphanumeric())
                    && text.chars().count() <= 2
            }
            GameInput::Mouse { x, y, button, .. } => {
                finite(&[*x, *y]) && matches!(button.as_str(), "left" | "right" | "middle")
            }
            GameInput::Move { x, y, dx, dy } => finite(&[*x, *y, *dx, *dy]),
            GameInput::Wheel { x, y, dy } => finite(&[*x, *y, *dy]),
            GameInput::Pad { button, .. } => *button < 32,
            GameInput::Axis { axis, value } => {
                *axis < 8 && value.is_finite() && (-1.0..=1.0).contains(value)
            }
        }
    }
}

/// What is under a point of the game's picture, for a person who points at what is wrong.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct GamePicked {
    /// The node's path in the running game ("/root/Main/Player/Sprite2D").
    pub path: String,
    pub name: String,
    pub class: String,
    /// What it says, when it shows text.
    pub text: String,
    /// The scene file it is defined in ("res://player.tscn"), its path inside that scene, and
    /// the line of the file where it begins.
    pub scene: String,
    pub in_scene: String,
    pub scene_line: Option<u32>,
    /// The script that drives it, its own or the nearest above it, and the node that script
    /// is on.
    pub script: String,
    pub script_on: String,
    /// Where it is in the picture, in the game's own pixels: x, y, width, height.
    pub area: [f32; 4],
}

/// One value a person may turn while the game runs.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct GameTunable {
    pub name: String,
    /// "float", "int" or "bool" (a switch: 0 or 1).
    pub kind: String,
    pub value: f64,
    /// What the script starts it at, when it says.
    pub initial: Option<f64>,
    /// The range and step its author gave, when they gave one.
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
}

/// A node's tunable values.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct GameTunables {
    pub path: String,
    pub name: String,
    pub class: String,
    pub script: String,
    pub values: Vec<GameTunable>,
}

/// The last seconds of a run, as the person just played them.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct GameRecent {
    /// The name an agent gives `game_replay` to play the run again ("r3"). `None` when the
    /// run cannot be played again; `unrepeatable` says why.
    pub replay: Option<String>,
    pub unrepeatable: Option<String>,
    /// Small pictures of the stretch, left to right and top to bottom (JPEG).
    pub picture: Vec<u8>,
    pub pictures: u32,
    /// How long the stretch is, and how long the whole run is, in seconds of game time.
    pub seconds: f32,
    pub run_seconds: f32,
    /// How many inputs the run was given from its start.
    pub inputs: u32,
}

/// A run kept to be played again: what it began from and every input it was given.
#[derive(Debug, Clone)]
struct Replay {
    id: String,
    seed: u32,
    scene: Option<String>,
    events: Value,
    ticks: u64,
}

/// How one start begins: which of the chat's starts it is, the window that owns the game's,
/// and the seed of the run it repeats, when it repeats one.
struct Begin {
    generation: u64,
    owner_window: Option<i64>,
    replayed: Option<u32>,
}

/// How many runs a chat keeps to be played again.
const REPLAYS_KEPT: usize = 8;

/// A seed for a run's random numbers.
fn fresh_seed() -> u32 {
    // The low bits of a version 7 id are random.
    (uuid::Uuid::now_v7().as_u128() & 0xffff_ffff) as u32
}

/// The line of a scene file where a node begins: `[node name="Sprite" parent="Player"]`.
fn scene_line(text: &str, in_scene: &str) -> Option<u32> {
    let (parent, name) = match in_scene.rsplit_once('/') {
        Some((parent, name)) => (Some(parent), name),
        None => (None, in_scene),
    };
    let named = |line: &str, key: &str, value: &str| line.contains(&format!("{key}=\"{value}\""));
    let mut root = None;
    for (index, line) in text.lines().enumerate() {
        if !line.starts_with("[node ") {
            continue;
        }
        let number = u32::try_from(index + 1).ok()?;
        let top = !line.contains(" parent=\"");
        if top && root.is_none() {
            root = Some(number);
        }
        match (in_scene, parent) {
            // The scene's own root is its first node, whatever the running game calls it.
            (".", _) if top => return Some(number),
            (_, None) if named(line, "name", name) && named(line, "parent", ".") => {
                return Some(number);
            }
            (_, Some(parent)) if named(line, "name", name) && named(line, "parent", parent) => {
                return Some(number);
            }
            _ => {}
        }
    }
    None.or(root.filter(|_| in_scene == "."))
}

/// What a tool answers: words, and the pictures that go with them (JPEG).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolReply {
    pub text: String,
    pub images: Vec<Vec<u8>>,
    pub error: bool,
}

impl ToolReply {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    pub fn failed(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
            error: true,
        }
    }
}

/// A tool call that goes to a web game.
pub type WebFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ToolReply> + Send>>;

/// Web games, as the tools reach them: the app's browser, where a game that is a page runs. The
/// chat is the caller's own; the browser acts in that chat's tab and no other.
pub trait WebGames: Send + Sync {
    fn call(&self, chat_id: String, tool: String, arguments: Value) -> WebFuture;
}

/// A running game.
struct Run {
    bridge: Arc<Bridge>,
    process: Running,
    hello: Hello,
    frozen: bool,
    speed: f32,
    fps: f32,
    scene: String,
    /// What the run's random numbers began from, and the scene it was asked to run.
    seed: u32,
    asked_scene: Option<String>,
    /// Controllers the game itself sees on this computer.
    joypads: u32,
    /// Copies the bridge's frames to the chat's viewers; ends with the run.
    forward: tokio::task::JoinHandle<()>,
}

impl Drop for Run {
    fn drop(&mut self) {
        self.forward.abort();
        self.process.kill();
    }
}

struct Chat {
    folder: PathBuf,
    project: Option<GameProject>,
    looked: bool,
    starting: bool,
    run: Option<Run>,
    ended: Option<String>,
    output: Output,
    used: Instant,
    agent: Option<String>,
    sound: bool,
    viewers: usize,
    windowed: bool,
    /// The newest frame, kept across runs so a pane that stays open sees the next one.
    frames: watch::Sender<Option<Arc<Frame>>>,
    /// Counts this chat's starts, so what an earlier one left behind cannot touch a later one.
    generation: u64,
    /// Runs kept to be played again, oldest first, and how many were ever kept.
    replays: Vec<Replay>,
    replay_seq: u32,
}

impl Chat {
    fn new(folder: &Path) -> Self {
        Self {
            folder: folder.to_owned(),
            project: None,
            looked: false,
            starting: false,
            run: None,
            ended: None,
            output: Output::default(),
            used: Instant::now(),
            agent: None,
            sound: false,
            viewers: 0,
            windowed: false,
            frames: watch::channel(None).0,
            generation: 0,
            replays: Vec::new(),
            replay_seq: 0,
        }
    }
}

struct State {
    enabled: bool,
    found: Vec<Found>,
    looked: bool,
    fetch: GodotFetch,
    fetch_cancel: Arc<AtomicBool>,
    chats: HashMap<String, Chat>,
    summons: HashMap<String, u32>,
    summon_seq: u32,
    owner_window: Option<i64>,
    /// How many live sessions of each chat hold the game tools: counted, because a restarted
    /// session may be given its tools before the one it replaces lets go of them.
    tools: HashMap<String, u32>,
}

struct Inner {
    config: Config,
    web: Mutex<Option<Arc<dyn WebGames>>>,
    state: Mutex<State>,
    notify: Box<dyn Fn() + Send + Sync>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Every chat's game. Cloning it gives another handle on the same games.
#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

/// A pane watching one chat's game: the frames as they come, across restarts of the game.
pub struct Viewer {
    manager: Manager,
    chat_id: String,
    frames: watch::Receiver<Option<Arc<Frame>>>,
}

impl Viewer {
    /// The next frame for the pane: one kind byte (6, a whole JPEG, as the Device pane's
    /// pictures are) and the JPEG. `None` when the chat is gone.
    pub async fn next(&mut self) -> Option<Vec<u8>> {
        loop {
            self.frames.changed().await.ok()?;
            let frame = self.frames.borrow_and_update().clone();
            if let Some(frame) = frame {
                let mut packet = Vec::with_capacity(frame.jpeg.len() + 1);
                packet.push(6);
                packet.extend_from_slice(&frame.jpeg);
                return Some(packet);
            }
        }
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.manager.unwatch(&self.chat_id);
    }
}

impl Manager {
    /// `notify` is called on every change of [`Manager::status`]. It must not call back in.
    pub fn new(config: Config, notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                web: Mutex::new(None),
                state: Mutex::new(State {
                    enabled: true,
                    found: Vec::new(),
                    looked: false,
                    fetch: GodotFetch::Idle,
                    fetch_cancel: Arc::default(),
                    chats: HashMap::new(),
                    summons: HashMap::new(),
                    summon_seq: 0,
                    owner_window: None,
                    tools: HashMap::new(),
                }),
                notify: Box::new(notify),
            }),
        }
    }

    /// Ghost's folder for games.
    pub(crate) fn root(&self) -> &Path {
        &self.inner.config.root
    }

    fn changed(&self) {
        (self.inner.notify)();
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        lock(&self.inner.state)
    }

    /// Ghost's own window, which a game's window is owned by to stay out of sight.
    /// Where web games run: the app's browser. Without one (tests, a build with no browser) a
    /// web game's tools say where it does run.
    pub fn set_web(&self, web: Option<Arc<dyn WebGames>>) {
        *lock(&self.inner.web) = web;
    }

    pub(crate) fn web(&self) -> Option<Arc<dyn WebGames>> {
        lock(&self.inner.web).clone()
    }

    pub fn set_owner_window(&self, window: Option<i64>) {
        self.state().owner_window = window;
    }

    pub fn status(&self) -> GameStatus {
        let state = self.state();
        let release = godot::release();
        let mut chats: Vec<ChatGame> = state
            .chats
            .iter()
            .map(|(chat_id, chat)| {
                chat_status(
                    chat_id,
                    chat,
                    state.tools.contains_key(chat_id),
                    &state.found,
                )
            })
            .collect();
        chats.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
        let mut summons: Vec<GameSummon> = state
            .summons
            .iter()
            .map(|(chat_id, seq)| GameSummon {
                chat_id: chat_id.clone(),
                seq: *seq,
            })
            .collect();
        summons.sort_by_key(|summon| summon.seq);
        GameStatus {
            enabled: state.enabled,
            godot: GodotStatus {
                pinned: godot::VERSION.to_owned(),
                size: release.map_or(0, |release| u32::try_from(release.size).unwrap_or(u32::MAX)),
                fetched: state.found.iter().any(|godot| godot.managed),
                found: state
                    .found
                    .iter()
                    .filter(|godot| !godot.managed)
                    .map(|godot| godot.version.clone())
                    .collect(),
                looked: state.looked,
                fetch: state.fetch.clone(),
            },
            chats,
            summons,
        }
    }

    /// Turning Game off stops every game; nothing starts while it is off.
    pub fn set_enabled(&self, enabled: bool) {
        let stopped: Vec<Run> = {
            let mut state = self.state();
            if state.enabled == enabled {
                return;
            }
            state.enabled = enabled;
            if enabled {
                Vec::new()
            } else {
                state
                    .chats
                    .values_mut()
                    .filter_map(|chat| {
                        chat.ended = chat
                            .run
                            .is_some()
                            .then(|| "Game was turned off.".to_owned());
                        chat.run.take()
                    })
                    .collect()
            }
        };
        drop(stopped);
        self.changed();
    }

    pub fn enabled(&self) -> bool {
        self.state().enabled
    }

    /// Looks over the computer for Godot again.
    pub async fn refresh(&self) {
        let found = godot::find(&self.inner.config.root).await;
        {
            let mut state = self.state();
            state.found = found;
            state.looked = true;
        }
        self.changed();
    }

    async fn look_once(&self) {
        if !self.state().looked {
            self.refresh().await;
        }
    }

    /// What `folder` holds, for the chat. The folder is the chat's own: the caller owns the chat.
    pub async fn look(&self, chat_id: &str, folder: &Path) -> Option<GameProject> {
        let at = folder.to_owned();
        let project = tokio::task::spawn_blocking(move || crate::detect(&at))
            .await
            .ok()
            .flatten();
        {
            let mut state = self.state();
            let chat = state
                .chats
                .entry(chat_id.to_owned())
                .or_insert_with(|| Chat::new(folder));
            if chat.folder != folder {
                // The chat moved to another folder: what ran in the old one is not its game.
                *chat = Chat::new(folder);
            }
            if chat.looked && chat.project == project {
                return project;
            }
            chat.project = project.clone();
            chat.looked = true;
        }
        self.changed();
        project
    }

    /// Starts a game from nothing in the chat's folder: one of Ghost's own small projects. The
    /// person's, from the pane: no tool calls this. A Godot starter is marked as made with the
    /// Godot that will run it.
    pub async fn start_project(
        &self,
        chat_id: &str,
        folder: &Path,
        starter: crate::starter::Starter,
    ) -> Result<GameProject, Error> {
        if !self.enabled() {
            return Err(Error::Refused(TURNED_OFF.into()));
        }
        self.look_once().await;
        let version = {
            let state = self.state();
            godot::choose(None, &state.found)
                .map(|godot| godot.version.clone())
                .unwrap_or_else(|_| godot::VERSION.to_owned())
        };
        // "4.7.2" as "4.7": a project names the release line it was made with.
        let line = version.split('.').take(2).collect::<Vec<_>>().join(".");
        let at = folder.to_owned();
        tokio::task::spawn_blocking(move || crate::starter::write(&at, starter, &line))
            .await
            .map_err(|_| Error::Game("The project could not be written.".into()))??;
        self.look(chat_id, folder).await.ok_or_else(|| {
            Error::Game("The project was written, and Ghost cannot read it back.".into())
        })
    }

    /// Forgets a chat that is gone, and stops its game.
    pub fn forget(&self, chat_id: &str) {
        let chat = {
            let mut state = self.state();
            state.summons.remove(chat_id);
            state.chats.remove(chat_id)
        };
        if chat.is_some() {
            drop(chat);
            self.changed();
        }
    }

    // ---------- Godot, fetched by the person ----------

    /// Starts fetching Ghost's own Godot. The person's, from the pane: no tool calls this.
    pub fn fetch_start(&self) {
        let cancel = {
            let mut state = self.state();
            if matches!(
                state.fetch,
                GodotFetch::Running { .. } | GodotFetch::Unpacking
            ) {
                return;
            }
            state.fetch = GodotFetch::Running {
                received: 0,
                total: godot::release()
                    .map_or(0, |release| u32::try_from(release.size).unwrap_or(u32::MAX)),
            };
            state.fetch_cancel = Arc::default();
            Arc::clone(&state.fetch_cancel)
        };
        self.changed();
        let manager = self.clone();
        tokio::spawn(async move {
            let reporter = manager.clone();
            let report = move |progress: ghost_fetch::Progress| {
                let whole = progress.received >= progress.total;
                reporter.state().fetch = if whole {
                    GodotFetch::Unpacking
                } else {
                    GodotFetch::Running {
                        received: u32::try_from(progress.received).unwrap_or(u32::MAX),
                        total: u32::try_from(progress.total).unwrap_or(u32::MAX),
                    }
                };
                reporter.changed();
            };
            let result = godot::fetch(&manager.inner.config.root, &report, &cancel).await;
            manager.state().fetch = match result {
                Ok(_) | Err(Error::Cancelled) => GodotFetch::Idle,
                Err(error) => GodotFetch::Failed {
                    message: error.to_string(),
                },
            };
            manager.refresh().await;
        });
    }

    pub fn fetch_cancel(&self) {
        self.state().fetch_cancel.store(true, Ordering::Release);
    }

    // ---------- running ----------

    /// The pane opens for this chat: an agent's tool is about to show something.
    pub(crate) fn summon(&self, chat_id: &str) {
        {
            let mut state = self.state();
            state.summon_seq = state.summon_seq.wrapping_add(1);
            let seq = state.summon_seq;
            state.summons.insert(chat_id.to_owned(), seq);
        }
        self.changed();
    }

    /// What an agent is doing with the chat's game now, for the pane; `None` when it is done.
    pub(crate) fn agent_doing(&self, chat_id: &str, doing: Option<String>) {
        {
            let mut state = self.state();
            let Some(chat) = state.chats.get_mut(chat_id) else {
                return;
            };
            if chat.agent == doing {
                return;
            }
            chat.agent = doing;
            chat.used = Instant::now();
        }
        self.changed();
    }

    /// The chat's running game, to ask it something.
    pub(crate) fn bridge(&self, chat_id: &str) -> Result<Arc<Bridge>, Error> {
        let mut state = self.state();
        let chat = state.chats.get_mut(chat_id);
        let run = chat.and_then(|chat| {
            chat.used = Instant::now();
            chat.run.as_ref()
        });
        match run {
            Some(run) if !run.bridge.is_closed() => Ok(Arc::clone(&run.bridge)),
            _ => Err(Error::Refused(
                "This chat's game is not running. Start it with game_run.".into(),
            )),
        }
    }

    /// Starts the chat's game, stopping the one it had. `scene` runs a scene other than the
    /// project's main one.
    pub async fn run(
        &self,
        chat_id: &str,
        folder: &Path,
        scene: Option<&str>,
    ) -> Result<(), Error> {
        self.run_from(chat_id, folder, scene, None).await
    }

    /// [`Manager::run`], and with `replayed` a run that begins from that seed and stands still
    /// before its first step, to be given a replay.
    async fn run_from(
        &self,
        chat_id: &str,
        folder: &Path,
        scene: Option<&str>,
        replayed: Option<u32>,
    ) -> Result<(), Error> {
        if !self.enabled() {
            return Err(Error::Refused(TURNED_OFF.into()));
        }
        if let Some(scene) = scene {
            run::checked_scene(scene)?;
        }
        let project = self.look(chat_id, folder).await.ok_or_else(|| {
            Error::Refused(
                "This chat's folder does not hold a game Ghost knows (a Godot project has a project.godot at its top)."
                    .into(),
            )
        })?;
        if let Some(why) = &project.unsupported {
            return Err(Error::Refused(why.clone()));
        }
        match project.engine {
            GameEngine::Godot => {}
            GameEngine::Web => {
                return Err(Error::Refused(
                    "This is a web game. Start its dev server and open it in Ghost's browser (browser_navigate)."
                        .into(),
                ));
            }
            other => {
                return Err(Error::Refused(format!(
                    "Ghost does not run {} itself. Use {}'s own tools for it.",
                    other.name(),
                    other.name()
                )));
            }
        }
        self.look_once().await;
        let (program, generation, owner_window) = {
            let mut state = self.state();
            let program = godot::choose(project.version.as_deref(), &state.found)
                .map_err(Error::Refused)?
                .program
                .clone();
            let running = state
                .chats
                .iter()
                .filter(|(id, chat)| {
                    id.as_str() != chat_id && (chat.run.is_some() || chat.starting)
                })
                .count();
            if running >= self.inner.config.max_running {
                return Err(Error::Refused(format!(
                    "{running} games are running in other chats already. Stop one first."
                )));
            }
            let owner_window = state.owner_window;
            let chat = state
                .chats
                .get_mut(chat_id)
                .ok_or_else(|| Error::Refused("This chat is gone.".into()))?;
            // In a window of its own the game has no owner and sits where windows open.
            let owner_window = owner_window.filter(|_| !chat.windowed);
            if chat.starting {
                return Err(Error::Refused(
                    "This chat's game is already starting.".into(),
                ));
            }
            chat.run = None;
            chat.starting = true;
            chat.ended = None;
            chat.output = Output::default();
            chat.used = Instant::now();
            chat.generation += 1;
            (program, chat.generation, owner_window)
        };
        self.changed();
        let result = self
            .start(
                chat_id,
                folder,
                scene,
                &program,
                Begin {
                    generation,
                    owner_window,
                    replayed,
                },
            )
            .await;
        // A start that was overtaken (the chat stopped it, or started again) leaves nothing: its
        // game is let go, which ends it.
        let outcome = {
            let mut state = self.state();
            let chat = state
                .chats
                .get_mut(chat_id)
                .filter(|chat| chat.generation == generation);
            match (result, chat) {
                (Ok(run), Some(chat)) => {
                    chat.starting = false;
                    chat.run = Some(run);
                    self.apply_viewers(chat);
                    Ok(())
                }
                (Ok(_), None) => Err(Error::Cancelled),
                (Err(error), Some(chat)) => {
                    chat.starting = false;
                    chat.ended = Some(error.to_string());
                    Err(error)
                }
                (Err(error), None) => Err(error),
            }
        };
        self.changed();
        outcome
    }

    async fn start(
        &self,
        chat_id: &str,
        folder: &Path,
        scene: Option<&str>,
        program: &Path,
        begin: Begin,
    ) -> Result<Run, Error> {
        let Begin {
            generation,
            owner_window,
            replayed,
        } = begin;
        let seed = replayed.unwrap_or_else(fresh_seed);
        let root = self.inner.config.root.clone();
        let main_loop = tokio::task::spawn_blocking(move || run::write_scripts(&root))
            .await
            .map_err(|_| Error::Game("Ghost's game scripts could not be written.".into()))??;
        let listening = Listening::bind().await?;
        let printer = self.clone();
        let printed_for = chat_id.to_owned();
        let mut process = run::start(
            &Launch {
                godot: program,
                project: folder,
                main_loop: &main_loop,
                bridge_address: &listening.address(),
                token: listening.token(),
                scene,
                owner_window,
                seed,
                held: replayed.is_some(),
            },
            move |line| {
                let mut state = printer.state();
                if let Some(chat) = state.chats.get_mut(&printed_for)
                    && chat.generation == generation
                {
                    chat.output.printed(&line);
                }
            },
        )?;
        let listener = self.clone();
        let heard_for = chat_id.to_owned();
        let accepting = listening.accept(START_PATIENCE, move |event| {
            listener.heard(&heard_for, generation, event);
        });
        tokio::pin!(accepting);
        // A game that ends before it connects failed to start: say what it printed last.
        let (bridge, hello) = loop {
            tokio::select! {
                accepted = &mut accepting => break accepted?,
                () = tokio::time::sleep(Duration::from_millis(200)) => {
                    if let Some(code) = process.ended() {
                        return Err(Error::Game(self.start_failure(chat_id, code)));
                    }
                }
            }
        };
        process.settled();
        let bridge = Arc::new(bridge);
        let frames_out = {
            let state = self.state();
            state
                .chats
                .get(chat_id)
                .map(|chat| chat.frames.clone())
                .ok_or_else(|| Error::Refused("This chat is gone.".into()))?
        };
        let mut frames_in = bridge.frames();
        let forward = tokio::spawn(async move {
            while frames_in.changed().await.is_ok() {
                let frame = frames_in.borrow_and_update().clone();
                if frames_out.send(frame).is_err() {
                    break;
                }
            }
        });
        Ok(Run {
            scene: scene.unwrap_or_default().to_owned(),
            seed,
            asked_scene: scene.map(str::to_owned),
            joypads: hello.joypads,
            bridge,
            process,
            hello,
            frozen: replayed.is_some(),
            speed: 1.0,
            fps: 0.0,
            forward,
        })
    }

    fn start_failure(&self, chat_id: &str, code: Option<i32>) -> String {
        let state = self.state();
        let chat = state.chats.get(chat_id);
        let said = chat
            .and_then(|chat| chat.output.errors().last().map(GameError::line))
            .or_else(|| {
                chat.and_then(|chat| chat.output.tail(3).last().map(|line| (*line).to_owned()))
            });
        match (said, code) {
            (Some(said), _) => format!("The game did not start: {said}"),
            (None, Some(code)) => format!("The game did not start (Godot exited with {code})."),
            (None, None) => "The game did not start.".to_owned(),
        }
    }

    /// What a running game says unasked.
    fn heard(&self, chat_id: &str, generation: u64, event: Event) {
        {
            let mut state = self.state();
            let Some(chat) = state.chats.get_mut(chat_id) else {
                return;
            };
            if chat.generation != generation {
                return;
            }
            match event {
                Event::Stats(stats) => {
                    let Some(run) = chat.run.as_mut() else {
                        return;
                    };
                    let fps = stats["fps"].as_f64().unwrap_or(0.0) as f32;
                    let frozen = stats["frozen"].as_bool().unwrap_or(false);
                    let speed = stats["speed"].as_f64().unwrap_or(1.0) as f32;
                    let scene: String = stats["scene"]
                        .as_str()
                        .unwrap_or_default()
                        .chars()
                        .take(300)
                        .collect();
                    let joypads = stats["joypads"].as_u64().unwrap_or(0).min(16) as u32;
                    let same = (run.fps - fps).abs() < 0.5
                        && run.frozen == frozen
                        && (run.speed - speed).abs() < 0.01
                        && run.scene == scene
                        && run.joypads == joypads;
                    if same {
                        return;
                    }
                    run.joypads = joypads;
                    run.fps = fps;
                    run.frozen = frozen;
                    run.speed = speed;
                    run.scene = scene;
                }
                Event::Error(event) => match GameError::from_event(&event) {
                    Some(error) => chat.output.failed(error),
                    None => return,
                },
                Event::Closed => {
                    let Some(mut run) = chat.run.take() else {
                        return;
                    };
                    let code = run.process.ended().flatten();
                    chat.agent = None;
                    chat.ended = Some(match code {
                        Some(0) | None => "The game ended.".to_owned(),
                        Some(code) => match chat.output.errors().last() {
                            Some(error) => format!("The game crashed: {}", error.line()),
                            None => format!("The game ended with code {code}."),
                        },
                    });
                }
            }
        }
        self.changed();
    }

    /// Stops the chat's game.
    pub async fn stop(&self, chat_id: &str) {
        let run = {
            let mut state = self.state();
            let Some(chat) = state.chats.get_mut(chat_id) else {
                return;
            };
            chat.generation += 1;
            chat.starting = false;
            chat.agent = None;
            let run = chat.run.take();
            if run.is_some() {
                chat.ended = Some("Stopped.".to_owned());
            }
            run
        };
        if let Some(run) = run {
            // Asked first, so the game can end as it would for a player; then ended regardless.
            let _ = run
                .bridge
                .ask("quit", json!({}), Duration::from_millis(800))
                .await;
            drop(run);
            self.changed();
        }
    }

    /// Stops every game. Called when Ghost quits.
    pub async fn shutdown(&self) {
        let chats: Vec<String> = self.state().chats.keys().cloned().collect();
        for chat_id in chats {
            self.stop(&chat_id).await;
        }
    }

    /// Stops the games nobody has watched or used for [`IDLE`]. The desktop calls it now and then.
    pub async fn stop_idle(&self) {
        let idle: Vec<String> = {
            let state = self.state();
            state
                .chats
                .iter()
                .filter(|(_, chat)| {
                    chat.run.is_some() && chat.viewers == 0 && chat.used.elapsed() >= IDLE
                })
                .map(|(chat_id, _)| chat_id.clone())
                .collect()
        };
        for chat_id in idle {
            self.stop(&chat_id).await;
            if let Some(chat) = self.state().chats.get_mut(&chat_id) {
                chat.ended = Some("Stopped after ten minutes unused.".to_owned());
            }
            self.changed();
        }
    }

    // ---------- the pane ----------

    /// Frames go to the game's viewers only while there are any, and sound plays only then.
    fn apply_viewers(&self, chat: &Chat) {
        if let Some(run) = &chat.run {
            run.bridge.tell(
                "watch",
                json!({"on": chat.viewers > 0, "max": 960, "every": 2}),
            );
            run.bridge
                .tell("audio", json!({"on": chat.sound && chat.viewers > 0}));
        }
    }

    /// The pane starts watching the chat's game.
    pub fn watch(&self, chat_id: &str, folder: &Path) -> Viewer {
        let frames = {
            let mut state = self.state();
            let chat = state
                .chats
                .entry(chat_id.to_owned())
                .or_insert_with(|| Chat::new(folder));
            chat.viewers += 1;
            chat.used = Instant::now();
            self.apply_viewers(chat);
            chat.frames.subscribe()
        };
        Viewer {
            manager: self.clone(),
            chat_id: chat_id.to_owned(),
            frames,
        }
    }

    fn unwatch(&self, chat_id: &str) {
        let mut state = self.state();
        if let Some(chat) = state.chats.get_mut(chat_id) {
            chat.viewers = chat.viewers.saturating_sub(1);
            chat.used = Instant::now();
            self.apply_viewers(chat);
        }
    }

    /// Input from the pane. Nothing is sent to a game that is not running, and input a person
    /// could not have made is dropped.
    pub fn input(&self, chat_id: &str, events: &[GameInput]) {
        let Ok(bridge) = self.bridge(chat_id) else {
            return;
        };
        let events: Vec<&GameInput> = events
            .iter()
            .filter(|event| event.sane())
            .take(64)
            .collect();
        if !events.is_empty() {
            bridge.tell("input", json!({ "events": events }));
        }
    }

    /// Freezes or resumes the chat's game, or changes how fast it runs.
    pub async fn time(
        &self,
        chat_id: &str,
        frozen: Option<bool>,
        speed: Option<f32>,
    ) -> Result<(bool, f32), Error> {
        let bridge = self.bridge(chat_id)?;
        let mut fields = json!({});
        if let Some(frozen) = frozen {
            fields["frozen"] = json!(frozen);
        }
        if let Some(speed) = speed.filter(|speed| speed.is_finite()) {
            fields["speed"] = json!(speed.clamp(0.1, 4.0));
        }
        let reply = bridge.ask("time", fields, Duration::from_secs(5)).await?;
        let frozen = reply.value["frozen"].as_bool().unwrap_or(false);
        let speed = reply.value["speed"].as_f64().unwrap_or(1.0) as f32;
        {
            let mut state = self.state();
            if let Some(run) = state
                .chats
                .get_mut(chat_id)
                .and_then(|chat| chat.run.as_mut())
            {
                run.frozen = frozen;
                run.speed = speed;
            }
        }
        self.changed();
        Ok((frozen, speed))
    }

    /// Whether the chat's game is heard. It plays only while the pane is open.
    pub fn sound(&self, chat_id: &str, on: bool) {
        {
            let mut state = self.state();
            let Some(chat) = state.chats.get_mut(chat_id) else {
                return;
            };
            chat.sound = on;
            self.apply_viewers(chat);
        }
        self.changed();
    }

    // ---------- tools ----------

    /// Asks the chat's running game, for a tool.
    pub(crate) async fn ask(
        &self,
        chat_id: &str,
        what: &str,
        fields: Value,
        patience: Duration,
    ) -> Result<Reply, Error> {
        self.bridge(chat_id)?.ask(what, fields, patience).await
    }

    /// The errors a tool's action just caused, as lines, and forgets that they are new.
    pub(crate) fn fresh_errors(&self, chat_id: &str) -> Vec<String> {
        let mut state = self.state();
        let Some(chat) = state.chats.get_mut(chat_id) else {
            return Vec::new();
        };
        let fresh = chat.output.take_fresh() as usize;
        if fresh == 0 {
            return Vec::new();
        }
        let errors = chat.output.errors();
        errors
            .iter()
            .rev()
            .filter(|error| error.kind != "warning")
            .take(fresh.min(5))
            .map(GameError::line)
            .collect()
    }

    /// The run's errors and the last lines it printed, for a tool.
    pub(crate) fn log(&self, chat_id: &str, lines: usize) -> (Vec<GameError>, Vec<String>, u32) {
        let state = self.state();
        match state.chats.get(chat_id) {
            Some(chat) => (
                chat.output.errors().to_vec(),
                chat.output
                    .tail(lines)
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                chat.output.dropped(),
            ),
            None => (Vec::new(), Vec::new(), 0),
        }
    }

    /// The run's errors and its last `lines` printed lines, for the pane's log.
    pub fn pane_log(&self, chat_id: &str, lines: usize) -> GameLog {
        let (errors, lines, dropped) = self.log(chat_id, lines.min(600));
        GameLog {
            errors,
            lines,
            dropped,
        }
    }

    /// One frame of game time while the game is frozen: the pane's Step.
    pub async fn step(&self, chat_id: &str) -> Result<(), Error> {
        self.ask(
            chat_id,
            "act",
            json!({"steps": [{"ms": 16}], "shots": 0, "max": 160}),
            Duration::from_secs(10),
        )
        .await
        .map(|_| ())
    }

    /// The game as it is drawn now, as a JPEG, for the person to keep.
    pub async fn picture(&self, chat_id: &str) -> Result<Vec<u8>, Error> {
        let reply = self
            .ask(
                chat_id,
                "shot",
                json!({"max": 1920}),
                Duration::from_secs(10),
            )
            .await?;
        reply
            .pictures
            .into_iter()
            .next()
            .map(|picture| picture.jpeg)
            .ok_or_else(|| Error::Bridge("The game sent no picture.".into()))
    }

    /// What is under a point of the game's picture, in the game's own pixels: the person points
    /// at what is wrong. `None` when nothing is there.
    pub async fn pick(
        &self,
        chat_id: &str,
        folder: &Path,
        x: f32,
        y: f32,
    ) -> Result<Option<GamePicked>, Error> {
        if !(x.is_finite() && y.is_finite()) {
            return Err(Error::Refused("That is not a place in the picture.".into()));
        }
        let reply = self
            .ask(
                chat_id,
                "pick",
                json!({ "x": x, "y": y }),
                Duration::from_secs(5),
            )
            .await?;
        let value = &reply.value;
        if value["found"].as_bool() != Some(true) {
            return Ok(None);
        }
        // What the game says of itself is data: cut to size, and shown as text only.
        let word = |key: &str| -> String {
            value[key]
                .as_str()
                .unwrap_or_default()
                .chars()
                .filter(|ch| !ch.is_control())
                .take(300)
                .collect()
        };
        let mut area = [0.0f32; 4];
        for (index, slot) in area.iter_mut().enumerate() {
            let number = value["box"][index].as_f64().unwrap_or(0.0) as f32;
            *slot = if number.is_finite() {
                number.clamp(-100_000.0, 100_000.0)
            } else {
                0.0
            };
        }
        let scene = word("scene");
        let in_scene = word("in_scene");
        // The line is read from the project's own file, never from what the game said beyond
        // which file: a scene outside the project is not opened.
        let scene_line = match run::checked_scene(&scene) {
            Ok(inside) if !in_scene.is_empty() => {
                let file = folder.join(inside.trim_start_matches("res://"));
                let wanted = in_scene.clone();
                tokio::task::spawn_blocking(move || {
                    let size = std::fs::metadata(&file).ok()?.len();
                    if size > 4 * 1024 * 1024 {
                        return None;
                    }
                    scene_line(&std::fs::read_to_string(&file).ok()?, &wanted)
                })
                .await
                .ok()
                .flatten()
            }
            _ => None,
        };
        Ok(Some(GamePicked {
            path: word("path"),
            name: word("name"),
            class: word("class"),
            text: word("text"),
            scene,
            in_scene,
            scene_line,
            script: word("script"),
            script_on: word("script_on"),
            area,
        }))
    }

    /// The values of a node a person may turn while the game runs.
    pub async fn tunables(&self, chat_id: &str, node: &str) -> Result<GameTunables, Error> {
        let reply = self
            .ask(
                chat_id,
                "tune",
                json!({ "node": node }),
                Duration::from_secs(5),
            )
            .await?;
        let value = &reply.value;
        let word = |value: &Value| -> String {
            value
                .as_str()
                .unwrap_or_default()
                .chars()
                .filter(|ch| !ch.is_control())
                .take(300)
                .collect()
        };
        let number = |value: &Value| match value {
            Value::Bool(on) => Some(f64::from(u8::from(*on))),
            other => other.as_f64().filter(|number| number.is_finite()),
        };
        let values = value["values"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .take(24)
                    .filter_map(|entry| {
                        let kind = word(&entry["type"]);
                        if !matches!(kind.as_str(), "float" | "int" | "bool") {
                            return None;
                        }
                        Some(GameTunable {
                            name: word(&entry["name"]),
                            kind,
                            value: number(&entry["value"])?,
                            initial: number(&entry["default"]),
                            min: number(&entry["min"]),
                            max: number(&entry["max"]),
                            step: number(&entry["step"]).filter(|step| *step > 0.0),
                        })
                    })
                    .filter(|tunable| !tunable.name.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Ok(GameTunables {
            path: word(&value["path"]),
            name: word(&value["name"]),
            class: word(&value["class"]),
            script: word(&value["script"]),
            values,
        })
    }

    /// Sets one tunable value of the running game, from the pane's slider. `kind` is the
    /// tunable's own.
    pub async fn set_value(
        &self,
        chat_id: &str,
        node: &str,
        property: &str,
        kind: &str,
        value: f64,
    ) -> Result<(), Error> {
        let named = !property.is_empty()
            && property.len() <= 64
            && property
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        if !named || !value.is_finite() {
            return Err(Error::Refused("That is not a value of this node.".into()));
        }
        let text = match kind {
            "bool" => (value != 0.0).to_string(),
            "int" => format!("{}", value.round() as i64),
            "float" => format!("{value}"),
            _ => return Err(Error::Refused("That value cannot be turned here.".into())),
        };
        self.ask(
            chat_id,
            "set",
            json!({ "node": node, "property": property, "value": text }),
            Duration::from_secs(5),
        )
        .await
        .map(|_| ())
    }

    /// The last `seconds` of the chat's run as small pictures, and the whole run kept to be
    /// played again: what the person hands the agent when something just went wrong.
    pub async fn recent(&self, chat_id: &str, seconds: f32) -> Result<GameRecent, Error> {
        let seconds = if seconds.is_finite() {
            seconds.clamp(1.0, 15.0)
        } else {
            15.0
        };
        let reply = self
            .ask(
                chat_id,
                "recent",
                json!({ "seconds": seconds }),
                Duration::from_secs(10),
            )
            .await?;
        let value = reply.value;
        let picture = reply
            .pictures
            .into_iter()
            .next()
            .map(|picture| picture.jpeg)
            .ok_or_else(|| Error::Bridge("The game sent no picture.".into()))?;
        // Whole numbers, however the game wrote them.
        let whole = |value: &Value| value.as_f64().filter(|n| *n >= 0.0).map(|n| n as u64);
        let ticks = whole(&value["tick"]).unwrap_or(0);
        let events = value["events"].clone();
        // What a replay will give back: what was done up to the step the run stands at. Something
        // done while it stood still after that step (a look at its state) is not part of it.
        let inputs = events.as_array().map_or(0, |events| {
            events
                .iter()
                .filter(|event| whole(&event[0]).is_some_and(|tick| tick <= ticks))
                .count()
        });
        let full = value["full"].as_bool().unwrap_or(false);
        let mut state = self.state();
        let chat = state
            .chats
            .get_mut(chat_id)
            .ok_or_else(|| Error::Refused("This chat is gone.".into()))?;
        let Some(run) = chat.run.as_ref() else {
            return Err(Error::Refused("This chat's game is not running.".into()));
        };
        let (replay, unrepeatable) = if full {
            (
                None,
                Some(
                    "This run was given more input than Ghost keeps, so it cannot be played again. The pictures are what it looked like."
                        .to_owned(),
                ),
            )
        } else if ticks == 0 {
            (None, Some("The game has not taken a step yet.".to_owned()))
        } else {
            chat.replay_seq += 1;
            let id = format!("r{}", chat.replay_seq);
            let kept = Replay {
                id: id.clone(),
                seed: run.seed,
                scene: run.asked_scene.clone(),
                events,
                ticks,
            };
            chat.replays.push(kept);
            if chat.replays.len() > REPLAYS_KEPT {
                chat.replays.remove(0);
            }
            (Some(id), None)
        };
        Ok(GameRecent {
            replay,
            unrepeatable,
            picture,
            pictures: value["pictures"].as_u64().unwrap_or(1).min(12) as u32,
            seconds: value["seconds"].as_f64().unwrap_or(0.0) as f32,
            run_seconds: ticks as f32 / 60.0,
            inputs: u32::try_from(inputs).unwrap_or(u32::MAX),
        })
    }

    /// Plays a kept run again: the game starts afresh from the same seed, standing still, is
    /// given the same inputs at the same steps as fast as it can go, and stands still again at
    /// `until_seconds` of game time (the kept run's end when not given).
    pub(crate) async fn replay(
        &self,
        chat_id: &str,
        folder: &Path,
        id: &str,
        until_seconds: Option<f64>,
    ) -> Result<Reply, Error> {
        let kept = {
            let state = self.state();
            let chat = state
                .chats
                .get(chat_id)
                .ok_or_else(|| Error::Refused("This chat is gone.".into()))?;
            chat.replays
                .iter()
                .find(|replay| replay.id == id)
                .cloned()
                .ok_or_else(|| {
                    let known: Vec<&str> =
                        chat.replays.iter().map(|replay| replay.id.as_str()).collect();
                    Error::Refused(if known.is_empty() {
                        "No run has been kept to replay. The person keeps one with \"Send the last 15 seconds\" in the Game pane.".to_owned()
                    } else {
                        format!("No replay \"{id}\" is kept. Kept: {}.", known.join(", "))
                    })
                })?
        };
        let until = match until_seconds {
            Some(seconds) if seconds.is_finite() && seconds > 0.0 => {
                ((seconds * 60.0).round() as u64).clamp(1, kept.ticks)
            }
            _ => kept.ticks,
        };
        self.run_from(chat_id, folder, kept.scene.as_deref(), Some(kept.seed))
            .await?;
        // As fast as the game can go; a heavy game may not go faster than it plays.
        let patience = Duration::from_secs(30 + until / 30);
        self.ask(
            chat_id,
            "replay",
            json!({
                "events": kept.events, "until": until, "window": 900, "shots": 12,
                "max": 1568,
            }),
            patience,
        )
        .await
    }

    /// Whether the chat's game gets a window of its own from its next start on. The person's
    /// choice, from the pane.
    pub fn set_windowed(&self, chat_id: &str, windowed: bool) {
        {
            let mut state = self.state();
            let Some(chat) = state.chats.get_mut(chat_id) else {
                return;
            };
            if chat.windowed == windowed {
                return;
            }
            chat.windowed = windowed;
        }
        self.changed();
    }

    /// Opens the chat's project in Godot's own editor, as a program of the person's: it is not
    /// Ghost's to stop, and it outlives the chat. The person's, from the pane: no tool calls this.
    pub async fn open_editor(&self, chat_id: &str, folder: &Path) -> Result<(), Error> {
        let project = self
            .look(chat_id, folder)
            .await
            .filter(|project| project.engine == GameEngine::Godot)
            .ok_or_else(|| Error::Refused("This chat's folder holds no Godot project.".into()))?;
        self.look_once().await;
        let program = self.godot_for(&project).map_err(Error::Refused)?;
        let mut command = crate::command(&program);
        command
            .arg("--path")
            .arg(folder)
            .arg("--editor")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        command
            .spawn()
            .map(drop)
            .map_err(|error| Error::Game(format!("Godot could not be opened: {error}")))
    }

    /// One chat's game as the pane shows it.
    pub fn chat(&self, chat_id: &str) -> Option<ChatGame> {
        let state = self.state();
        state.chats.get(chat_id).map(|chat| {
            chat_status(
                chat_id,
                chat,
                state.tools.contains_key(chat_id),
                &state.found,
            )
        })
    }

    /// A session of `chat_id` was given the game tools, or let go of them. Said by the app's
    /// sessions, never by a tool.
    pub fn tools_granted(&self, chat_id: &str, granted: bool) {
        {
            let mut state = self.state();
            let held = state.tools.get(chat_id).copied().unwrap_or(0);
            let now = if granted {
                held.saturating_add(1)
            } else {
                held.saturating_sub(1)
            };
            if now == 0 {
                state.tools.remove(chat_id);
            } else {
                state.tools.insert(chat_id.to_owned(), now);
            }
            if (held == 0) == (now == 0) {
                return;
            }
        }
        self.changed();
    }

    /// The Godot a project would run on, for a tool to tell an agent where it is.
    pub(crate) fn godot_for(&self, project: &GameProject) -> Result<PathBuf, String> {
        let state = self.state();
        godot::choose(project.version.as_deref(), &state.found).map(|godot| godot.program.clone())
    }

    pub(crate) async fn looked(&self) {
        self.look_once().await;
    }

    /// One tool call from an agent in `chat_id`, whose folder is `folder`. Both come from the
    /// session's grant: a tool's arguments never name a chat or a folder.
    pub async fn tool(
        &self,
        chat_id: &str,
        folder: &Path,
        tool: &str,
        arguments: &Value,
    ) -> ToolReply {
        tools::call(self, chat_id, folder, tool, arguments).await
    }
}

fn chat_status(chat_id: &str, chat: &Chat, tools: bool, found: &[Found]) -> ChatGame {
    let run = chat.run.as_ref();
    let godot_missing = chat
        .project
        .as_ref()
        .filter(|project| project.engine == GameEngine::Godot && project.unsupported.is_none())
        .and_then(|project| godot::choose(project.version.as_deref(), found).err());
    ChatGame {
        chat_id: chat_id.to_owned(),
        project: chat.project.clone(),
        state: if chat.starting {
            GameState::Starting
        } else if run.is_some() {
            GameState::Running
        } else if chat.ended.is_some() {
            GameState::Ended
        } else {
            GameState::Idle
        },
        frozen: run.is_some_and(|run| run.frozen),
        speed: run.map_or(1.0, |run| run.speed),
        fps: run.map_or(0.0, |run| run.fps),
        scene: run.map(|run| run.scene.clone()).unwrap_or_default(),
        width: run.map_or(0, |run| run.hello.width),
        height: run.map_or(0, |run| run.hello.height),
        engine_version: run.map(|run| run.hello.version.clone()).unwrap_or_default(),
        errors: chat.output.error_count(),
        last_error: chat.output.errors().last().cloned(),
        ended: chat.ended.clone(),
        agent: chat.agent.clone(),
        sound: chat.sound,
        pid: run.and_then(|run| run.process.pid()),
        godot_missing,
        windowed: chat.windowed,
        tools,
        joypads: run.map_or(0, |run| run.joypads),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picked_node_is_found_in_its_scene_file() {
        let scene = "[gd_scene load_steps=2 format=3]\n\n[ext_resource type=\"Script\" path=\"res://main.gd\" id=\"1\"]\n\n[node name=\"Main\" type=\"Node2D\"]\nscript = ExtResource(\"1\")\n\n[node name=\"Player\" type=\"Polygon2D\" parent=\".\"]\n\n[node name=\"Hud\" type=\"Label\" parent=\".\"]\n\n[node name=\"Sprite\" type=\"Sprite2D\" parent=\"Player\"]\n\n[node name=\"Sprite\" type=\"Sprite2D\" parent=\"Enemy/Body\"]\n";
        // The scene's own top node, whatever it is called where it runs.
        assert_eq!(scene_line(scene, "."), Some(5));
        assert_eq!(scene_line(scene, "Player"), Some(8));
        assert_eq!(scene_line(scene, "Hud"), Some(10));
        // Two nodes of one name are told apart by what they hang from.
        assert_eq!(scene_line(scene, "Player/Sprite"), Some(12));
        assert_eq!(scene_line(scene, "Enemy/Body/Sprite"), Some(14));
        // A node made by a script is in no file.
        assert_eq!(scene_line(scene, "Bullet"), None);
        assert_eq!(scene_line(scene, "Player/Bullet"), None);
        assert_eq!(scene_line("", "."), None);
    }

    #[test]
    fn a_controllers_input_is_a_button_or_a_stick_within_its_range() {
        assert!(
            GameInput::Pad {
                button: 0,
                down: true
            }
            .sane()
        );
        assert!(
            GameInput::Pad {
                button: 14,
                down: false
            }
            .sane()
        );
        assert!(
            !GameInput::Pad {
                button: 200,
                down: true
            }
            .sane()
        );
        assert!(
            GameInput::Axis {
                axis: 1,
                value: -1.0
            }
            .sane()
        );
        assert!(
            GameInput::Axis {
                axis: 5,
                value: 0.25
            }
            .sane()
        );
        assert!(
            !GameInput::Axis {
                axis: 1,
                value: 1.5
            }
            .sane()
        );
        assert!(
            !GameInput::Axis {
                axis: 1,
                value: f32::NAN
            }
            .sane()
        );
        assert!(
            !GameInput::Axis {
                axis: 9,
                value: 0.0
            }
            .sane()
        );
        // As the pane sends them.
        let pad: GameInput =
            serde_json::from_value(json!({ "t": "pad", "button": 3, "down": true })).unwrap();
        assert_eq!(
            pad,
            GameInput::Pad {
                button: 3,
                down: true
            }
        );
        let axis: GameInput =
            serde_json::from_value(json!({ "t": "axis", "axis": 0, "value": 0.5 })).unwrap();
        assert_eq!(
            axis,
            GameInput::Axis {
                axis: 0,
                value: 0.5
            }
        );
    }

    #[test]
    fn every_run_draws_its_own_seed() {
        let seeds: std::collections::HashSet<u32> = (0..64).map(|_| fresh_seed()).collect();
        assert!(seeds.len() > 48, "{} different seeds of 64", seeds.len());
    }
}
