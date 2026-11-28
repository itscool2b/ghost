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
use crate::detect::{Engine, Project};
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
pub const TURNED_OFF: &str =
    "Game is turned off in Ghost (Settings > Game). Ask the person to turn it on.";

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
    pub summons: Vec<Summon>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct Summon {
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
    pub fetch: Fetch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Fetch {
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
    pub project: Option<Project>,
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
}

/// Input from the pane, in the shape the game's bridge takes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum InputEvent {
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
}

impl InputEvent {
    /// Whether it is input a person could have made: short names, real numbers.
    fn sane(&self) -> bool {
        let finite = |numbers: &[f32]| numbers.iter().all(|n| n.is_finite() && n.abs() < 1.0e6);
        match self {
            InputEvent::Key { code, text, .. } => {
                !code.is_empty()
                    && code.len() <= 32
                    && code.chars().all(|ch| ch.is_ascii_alphanumeric())
                    && text.chars().count() <= 2
            }
            InputEvent::Mouse { x, y, button, .. } => {
                finite(&[*x, *y]) && matches!(button.as_str(), "left" | "right" | "middle")
            }
            InputEvent::Move { x, y, dx, dy } => finite(&[*x, *y, *dx, *dy]),
            InputEvent::Wheel { x, y, dy } => finite(&[*x, *y, *dy]),
        }
    }
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

/// A running game.
struct Run {
    bridge: Arc<Bridge>,
    process: Running,
    hello: Hello,
    frozen: bool,
    speed: f32,
    fps: f32,
    scene: String,
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
    project: Option<Project>,
    looked: bool,
    starting: bool,
    run: Option<Run>,
    ended: Option<String>,
    output: Output,
    used: Instant,
    agent: Option<String>,
    sound: bool,
    viewers: usize,
    /// The newest frame, kept across runs so a pane that stays open sees the next one.
    frames: watch::Sender<Option<Arc<Frame>>>,
    /// Counts this chat's starts, so what an earlier one left behind cannot touch a later one.
    generation: u64,
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
            frames: watch::channel(None).0,
            generation: 0,
        }
    }
}

struct State {
    enabled: bool,
    found: Vec<Found>,
    looked: bool,
    fetch: Fetch,
    fetch_cancel: Arc<AtomicBool>,
    chats: HashMap<String, Chat>,
    summons: HashMap<String, u32>,
    summon_seq: u32,
    owner_window: Option<i64>,
}

struct Inner {
    config: Config,
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
                state: Mutex::new(State {
                    enabled: true,
                    found: Vec::new(),
                    looked: false,
                    fetch: Fetch::Idle,
                    fetch_cancel: Arc::default(),
                    chats: HashMap::new(),
                    summons: HashMap::new(),
                    summon_seq: 0,
                    owner_window: None,
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
    pub fn set_owner_window(&self, window: Option<i64>) {
        self.state().owner_window = window;
    }

    pub fn status(&self) -> GameStatus {
        let state = self.state();
        let release = godot::release();
        let mut chats: Vec<ChatGame> = state
            .chats
            .iter()
            .map(|(chat_id, chat)| chat_status(chat_id, chat))
            .collect();
        chats.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
        let mut summons: Vec<Summon> = state
            .summons
            .iter()
            .map(|(chat_id, seq)| Summon {
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
    pub async fn look(&self, chat_id: &str, folder: &Path) -> Option<Project> {
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
            if matches!(state.fetch, Fetch::Running { .. } | Fetch::Unpacking) {
                return;
            }
            state.fetch = Fetch::Running {
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
                    Fetch::Unpacking
                } else {
                    Fetch::Running {
                        received: u32::try_from(progress.received).unwrap_or(u32::MAX),
                        total: u32::try_from(progress.total).unwrap_or(u32::MAX),
                    }
                };
                reporter.changed();
            };
            let result = godot::fetch(&manager.inner.config.root, &report, &cancel).await;
            manager.state().fetch = match result {
                Ok(_) | Err(Error::Cancelled) => Fetch::Idle,
                Err(error) => Fetch::Failed {
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
            Engine::Godot => {}
            Engine::Web => {
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
            .start(chat_id, folder, scene, &program, generation, owner_window)
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
        generation: u64,
        owner_window: Option<i64>,
    ) -> Result<Run, Error> {
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
            bridge,
            process,
            hello,
            frozen: false,
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
                    let same = (run.fps - fps).abs() < 0.5
                        && run.frozen == frozen
                        && (run.speed - speed).abs() < 0.01
                        && run.scene == scene;
                    if same {
                        return;
                    }
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
    pub fn input(&self, chat_id: &str, events: &[InputEvent]) {
        let Ok(bridge) = self.bridge(chat_id) else {
            return;
        };
        let events: Vec<&InputEvent> = events
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

    /// One chat's game as the pane shows it.
    pub fn chat(&self, chat_id: &str) -> Option<ChatGame> {
        let state = self.state();
        state
            .chats
            .get(chat_id)
            .map(|chat| chat_status(chat_id, chat))
    }

    /// The Godot a project would run on, for a tool to tell an agent where it is.
    pub(crate) fn godot_for(&self, project: &Project) -> Result<PathBuf, String> {
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

fn chat_status(chat_id: &str, chat: &Chat) -> ChatGame {
    let run = chat.run.as_ref();
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
    }
}
