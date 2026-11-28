//! The Game pane (docs/game.md): Ghost's own window's commands for the games `ghost-game` runs,
//! the frames they send to the pane, and the agents' bridge to them.
//!
//! Everything here is by chat id, and a chat's game is the one in the chat's own folder: the
//! folder is read from the chat's record, never taken from the page. The pane's commands are the
//! person's: fetching Godot, turning Game on for a chat, a window of the game's own and Godot's
//! editor exist only here, never among the agents' tools.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine as _;
use ghost_game::{
    Config, DEFAULT_RUNNING, EngineProbe, GameEngine, GameInput, GameLog, GameStatus,
    Manager as Games, Starter,
};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody, IpcResponse};
use tauri::{AppHandle, Emitter, Manager as _, State};
use tokio::task::AbortHandle;

use crate::error::{AppError, AppErrorCode};
use crate::state::AppState;

/// The event that carries every change: its payload is the whole [`GameStatus`].
pub const EVENT: &str = "ghost-game";

/// The most chats whose "Game is on here" choice is kept; the oldest go first.
const MAX_CHOSEN: usize = 400;

impl From<ghost_game::Error> for AppError {
    fn from(error: ghost_game::Error) -> Self {
        let code = match error {
            ghost_game::Error::Refused(_) => AppErrorCode::InvalidArgument,
            _ => AppErrorCode::Internal,
        };
        AppError::new(code, error.to_string())
    }
}

/// The frame streams the pane is watching, one per chat, so a new one ends the one before it.
/// Each has a number: the pane stops the stream it started, and a stop that arrives late (the pane
/// closed and opened again meanwhile) cannot end the stream that replaced it.
#[derive(Default)]
pub struct Watches {
    next: AtomicU32,
    running: Mutex<HashMap<String, (u32, AbortHandle)>>,
}

impl Watches {
    fn start(&self, chat_id: &str, task: AbortHandle) -> u32 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, before)) = running.insert(chat_id.to_owned(), (id, task)) {
            before.abort();
        }
        id
    }

    fn stop(&self, chat_id: &str, id: Option<u32>) {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        let current = running.get(chat_id).map(|(current, _)| *current);
        if current.is_some()
            && id.is_none_or(|id| Some(id) == current)
            && let Some((_, task)) = running.remove(chat_id)
        {
            task.abort();
        }
    }

    fn clear(&self) {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        for (_, (_, task)) in running.drain() {
            task.abort();
        }
    }
}

/// The chats the person turned Game on for although their folder holds no game Ghost knows (a
/// folder a game is about to be made in). Kept in a small file of chat ids beside Ghost's Godot.
pub struct Chosen {
    file: PathBuf,
    chats: Mutex<Vec<String>>,
}

impl Chosen {
    fn open(folder: &Path) -> Self {
        let file = folder.join("chats.json");
        let chats = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|chat| chat_id_like(chat))
            .take(MAX_CHOSEN)
            .collect();
        Self {
            file,
            chats: Mutex::new(chats),
        }
    }

    fn has(&self, chat_id: &str) -> bool {
        self.chats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|chat| chat == chat_id)
    }

    /// Remembers or forgets the chat, and writes the file. A write that fails keeps the choice
    /// for this run of the app.
    fn set(&self, chat_id: &str, on: bool) {
        let saved = {
            let mut chats = self.chats.lock().unwrap_or_else(PoisonError::into_inner);
            chats.retain(|chat| chat != chat_id);
            if on {
                chats.push(chat_id.to_owned());
                let over = chats.len().saturating_sub(MAX_CHOSEN);
                chats.drain(..over);
            }
            chats.clone()
        };
        let write = || -> std::io::Result<()> {
            if let Some(folder) = self.file.parent() {
                std::fs::create_dir_all(folder)?;
            }
            let scratch = self.file.with_extension("json.new");
            std::fs::write(&scratch, serde_json::to_vec(&saved)?)?;
            std::fs::rename(&scratch, &self.file)
        };
        if let Err(error) = write() {
            tracing::warn!(%error, "the chats with Game turned on could not be saved");
        }
    }
}

/// Whether `text` could be one of Ghost's chat ids: what the file may hold and nothing else.
fn chat_id_like(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// One packet of a game's picture stream, sent to the page as raw bytes (the page reads an
/// `ArrayBuffer`): a byte of kind, then the payload (`ghost_game::Viewer`).
#[derive(Debug, Clone, specta::Type)]
pub struct GameFrame(Vec<u8>);

impl IpcResponse for GameFrame {
    fn body(self) -> tauri::Result<InvokeResponseBody> {
        Ok(InvokeResponseBody::Raw(self.0))
    }
}

/// What one of the agents' tools answered, for the development checks.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct GameToolCheck {
    pub text: String,
    /// How many pictures came with it, and the size of them all in bytes.
    pub images: u32,
    pub image_length: u32,
    pub error: bool,
}

/// Agents' calls from ghost-core's game grants, run on the chat's own game.
struct AgentBridge {
    app: AppHandle,
    games: Games,
    chosen: Arc<Chosen>,
}

impl ghost_core::game_service::GameBridge for AgentBridge {
    fn wanted(&self, chat_id: String, folder: PathBuf) -> ghost_core::game_service::WantedFuture {
        let (games, chosen) = (self.games.clone(), Arc::clone(&self.chosen));
        Box::pin(async move {
            if !games.enabled() {
                return false;
            }
            chosen.has(&chat_id) || games.look(&chat_id, &folder).await.is_some()
        })
    }

    fn call(
        &self,
        chat_id: String,
        folder: PathBuf,
        tool: String,
        arguments: serde_json::Value,
    ) -> ghost_core::game_service::GameFuture {
        let games = self.games.clone();
        // A game an agent starts sits out of sight too, owned by Ghost's window.
        games.set_owner_window(owner_window(&self.app));
        Box::pin(async move {
            let reply = games.tool(&chat_id, &folder, &tool, &arguments).await;
            ghost_core::game_service::GameToolReply {
                text: reply.text,
                images: reply
                    .images
                    .into_iter()
                    .map(|jpeg| {
                        (
                            "image/jpeg".to_owned(),
                            base64::engine::general_purpose::STANDARD.encode(jpeg),
                        )
                    })
                    .collect(),
                error: reply.error,
            }
        })
    }

    fn granted(&self, chat_id: &str, granted: bool) {
        self.games.tools_granted(chat_id, granted);
    }
}

/// Web games, for the tools: the app's browser, where a game that is a page runs
/// (`crate::browser::game`). Looked up at each call, since the browser is the profile's.
struct WebRunner(AppHandle);

impl ghost_game::WebGames for WebRunner {
    fn call(
        &self,
        chat_id: String,
        tool: String,
        arguments: serde_json::Value,
    ) -> ghost_game::WebFuture {
        let app = self.0.clone();
        Box::pin(async move {
            let Some(browser) = app
                .try_state::<Arc<crate::browser::Browser>>()
                .map(|state| Arc::clone(state.inner()))
            else {
                return ghost_game::ToolReply::failed(
                    "Ghost's browser is not available in this run, so a web game cannot be opened here.",
                );
            };
            crate::browser::game::call(&browser, &chat_id, &tool, &arguments).await
        })
    }
}

/// Creates the game manager during setup. Nothing is started and nothing is looked for until a
/// pane is opened or an agent asks.
pub fn setup(app: &AppHandle) {
    let Some(root) = app
        .try_state::<AppState>()
        .and_then(|state| state.data_dir().map(|dir| dir.join("game")))
    else {
        return;
    };
    let handle = app.clone();
    let chosen = Arc::new(Chosen::open(&root));
    let games = Games::new(
        Config {
            root,
            max_running: DEFAULT_RUNNING,
        },
        move || {
            if let Some(games) = handle.try_state::<Games>() {
                let _ = handle.emit_to(crate::window::MAIN_WINDOW, EVENT, games.status());
            }
        },
    );
    // A game nobody watched or used for ten minutes is stopped.
    let idle = games.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            idle.stop_idle().await;
        }
    });
    games.set_web(Some(Arc::new(WebRunner(app.clone()))));
    app.manage(games);
    app.manage(chosen);
    app.manage(Watches::default());
    sync(app);
}

/// Ghost's own window, as the number Godot takes for the window a game's window is owned by, so
/// that the game has no taskbar entry and can sit off-screen. Windows only: elsewhere a game's
/// window shows as any window does.
fn owner_window(app: &AppHandle) -> Option<i64> {
    #[cfg(windows)]
    {
        let window = app.get_webview_window(crate::window::MAIN_WINDOW)?;
        let hwnd = window.hwnd().ok()?;
        Some(hwnd.0 as isize as i64)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        None
    }
}

/// The active profile's sessions get their bridge to the games (after setup and a profile
/// switch).
pub fn sync(app: &AppHandle) {
    let (Some(games), Some(chosen)) = (
        app.try_state::<Games>().map(|state| state.inner().clone()),
        app.try_state::<Arc<Chosen>>()
            .map(|state| Arc::clone(state.inner())),
    ) else {
        return;
    };
    if let Some(chats) = app.try_state::<AppState>().and_then(|state| state.chats()) {
        chats.manager.set_game_bridge(Some(Arc::new(AgentBridge {
            app: app.clone(),
            games,
            chosen,
        })));
    }
}

/// Settings' switch for Game. Off stops every game Ghost started.
pub fn apply_preferences(app: &AppHandle, general: &ghost_core::onboarding::GeneralPreferences) {
    sync(app);
    let Some(games) = app.try_state::<Games>() else {
        return;
    };
    if games.enabled() == general.game {
        return;
    }
    if !general.game
        && let Some(watches) = app.try_state::<Watches>()
    {
        watches.clear();
    }
    games.set_enabled(general.game);
}

/// Ghost is quitting: no game outlives the app.
pub fn shutdown(app: &AppHandle) {
    if let Some(watches) = app.try_state::<Watches>() {
        watches.clear();
    }
    if let Some(games) = app.try_state::<Games>().map(|state| state.inner().clone()) {
        tauri::async_runtime::block_on(games.shutdown());
    }
}

/// The folder `chat_id` works in: the chat's own record says, never the page.
async fn folder_of(state: &State<'_, AppState>, chat_id: &str) -> Result<PathBuf, AppError> {
    Ok(state.sessions()?.chat_folder(chat_id).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn game_status(games: State<'_, Games>) -> Result<GameStatus, AppError> {
    Ok(games.status())
}

/// A chat is shown, or its turn ended: what its folder holds now. Cheap (a few files are read),
/// and it starts nothing.
#[tauri::command]
#[specta::specta]
pub async fn game_look(
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chat_id: String,
) -> Result<GameStatus, AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    games.look(&chat_id, &folder).await;
    Ok(games.status())
}

/// The pane was opened: look over the computer for Godot again, and at the chat's folder.
#[tauri::command]
#[specta::specta]
pub async fn game_refresh(
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chat_id: String,
) -> Result<GameStatus, AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    games.refresh().await;
    games.look(&chat_id, &folder).await;
    Ok(games.status())
}

/// The person pressed Get Godot: download the pinned release from Godot's own releases, check it
/// against the hash Godot publishes, and unpack it into Ghost's folder.
#[tauri::command]
#[specta::specta]
pub async fn game_fetch_start(games: State<'_, Games>) -> Result<(), AppError> {
    games.fetch_start();
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn game_fetch_cancel(games: State<'_, Games>) -> Result<(), AppError> {
    games.fetch_cancel();
    Ok(())
}

/// The person pressed Play: start the chat's game (again), and let it run live.
#[tauri::command]
#[specta::specta]
pub async fn game_run(
    app: AppHandle,
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chat_id: String,
    scene: Option<String>,
) -> Result<(), AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    games.set_owner_window(owner_window(&app));
    games.run(&chat_id, &folder, scene.as_deref()).await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn game_stop(games: State<'_, Games>, chat_id: String) -> Result<(), AppError> {
    games.stop(&chat_id).await;
    Ok(())
}

/// Pause or resume the chat's game, or change how fast it runs.
#[tauri::command]
#[specta::specta]
pub async fn game_time(
    games: State<'_, Games>,
    chat_id: String,
    frozen: Option<bool>,
    speed: Option<f32>,
) -> Result<(), AppError> {
    if speed.is_some_and(|speed| !speed.is_finite()) {
        return Err(AppError::invalid_argument("A speed is a number."));
    }
    games.time(&chat_id, frozen, speed).await?;
    Ok(())
}

/// One frame of game time while the game is paused.
#[tauri::command]
#[specta::specta]
pub async fn game_step(games: State<'_, Games>, chat_id: String) -> Result<(), AppError> {
    Ok(games.step(&chat_id).await?)
}

/// Whether the chat's game is heard.
#[tauri::command]
#[specta::specta]
pub async fn game_sound(
    games: State<'_, Games>,
    chat_id: String,
    on: bool,
) -> Result<(), AppError> {
    games.sound(&chat_id, on);
    Ok(())
}

/// The person's keys and mouse, at places of the picture they are watching.
#[tauri::command]
#[specta::specta]
pub async fn game_input(
    games: State<'_, Games>,
    chat_id: String,
    events: Vec<GameInput>,
) -> Result<(), AppError> {
    games.input(&chat_id, &events);
    Ok(())
}

/// What is under a point of the game's picture, in the game's own pixels, for the pane's Point:
/// the person freezes the game and clicks what is wrong. Nothing there answers `None`.
#[tauri::command]
#[specta::specta]
pub async fn game_pick(
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chat_id: String,
    x: f32,
    y: f32,
) -> Result<Option<ghost_game::GamePicked>, AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    Ok(games.pick(&chat_id, &folder, x, y).await?)
}

/// The values of a node the person may turn while the game runs, for the pane's Tune.
#[tauri::command]
#[specta::specta]
pub async fn game_tunables(
    games: State<'_, Games>,
    chat_id: String,
    node: String,
) -> Result<ghost_game::GameTunables, AppError> {
    Ok(games.tunables(&chat_id, &node).await?)
}

/// Turns one value of the running game. It changes this run only: the person hands what they
/// keep to the agent, which writes it into the project.
#[tauri::command]
#[specta::specta]
pub async fn game_set_value(
    games: State<'_, Games>,
    chat_id: String,
    node: String,
    property: String,
    kind: String,
    value: f64,
) -> Result<(), AppError> {
    Ok(games
        .set_value(&chat_id, &node, &property, &kind, value)
        .await?)
}

/// The last seconds of the chat's run as small pictures, and the run kept for the agent to play
/// again (`game_replay`). The person's action: what they just played goes to the agent.
#[tauri::command]
#[specta::specta]
pub async fn game_recent(
    games: State<'_, Games>,
    chat_id: String,
    seconds: f32,
) -> Result<ghost_game::GameRecent, AppError> {
    Ok(games.recent(&chat_id, seconds).await?)
}

/// The pictures, sounds and models of the chat's folder, the newest first, for the pane's
/// Assets view. Links are not followed and hidden folders, dependencies and builds are not
/// entered.
#[tauri::command]
#[specta::specta]
pub async fn game_assets(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<ghost_game::assets::GameAssets, AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    tokio::task::spawn_blocking(move || ghost_game::assets::list(&folder))
        .await
        .map_err(|_| AppError::new(AppErrorCode::Internal, "The folder could not be read."))
}

/// One asset of the chat's folder, for the person to look at or hear.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct GameAssetData {
    pub kind: ghost_game::assets::AssetKind,
    /// What the file is, by its name ("image/png", "audio/ogg", "model/gltf-binary").
    pub mime: String,
    pub base64: String,
}

/// What a browser calls a file with this name.
fn asset_mime(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        _ => "application/octet-stream",
    }
}

/// Reads one asset by the path [`game_assets`] listed it under. Only a picture, a sound or a
/// model inside the chat's own folder is read, through no link, up to the size its kind allows.
#[tauri::command]
#[specta::specta]
pub async fn game_asset(
    state: State<'_, AppState>,
    chat_id: String,
    path: String,
) -> Result<GameAssetData, AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    let mime = asset_mime(&path).to_owned();
    let (kind, bytes) =
        tokio::task::spawn_blocking(move || ghost_game::assets::read(&folder, &path))
            .await
            .map_err(|_| AppError::new(AppErrorCode::Internal, "The asset could not be read."))??;
    Ok(GameAssetData {
        kind,
        mime,
        base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Packs the chat's game for the web and puts the zip in the person's Downloads folder. A Godot
/// project is exported by Ghost's own Godot (its web template is fetched first, ten megabytes,
/// when it is not here yet); a web game is packed from its built folder. Nothing is uploaded.
#[tauri::command]
#[specta::specta]
pub async fn game_export(
    app: AppHandle,
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chat_id: String,
) -> Result<ghost_game::export::GameExport, AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    let downloads = app
        .path()
        .download_dir()
        .map_err(|_| AppError::new(AppErrorCode::Internal, "There is no Downloads folder."))?;
    Ok(games.export(&chat_id, &folder, &downloads).await?)
}

/// Shows the zip the chat's last export made, in its folder. The page names no path: only the
/// zip this app just wrote is ever shown.
#[tauri::command]
#[specta::specta]
pub async fn game_export_show(games: State<'_, Games>, chat_id: String) -> Result<(), AppError> {
    let zip = games.exported(&chat_id).ok_or_else(|| {
        AppError::new(
            AppErrorCode::RevealFailed,
            "The exported zip is no longer there.",
        )
    })?;
    tauri::async_runtime::spawn_blocking(move || tauri_plugin_opener::reveal_item_in_dir(zip))
        .await
        .map_err(|_| AppError::new(AppErrorCode::RevealFailed, "The folder could not be shown."))?
        .map_err(|error| {
            AppError::new(
                AppErrorCode::RevealFailed,
                format!("The folder could not be shown: {error}"),
            )
        })
}

/// Streams the chat's game to `channel` until [`game_unwatch`], a new watch for the chat, or the
/// stream's own end. Answers the stream's number, for `game_unwatch`.
#[tauri::command]
#[specta::specta]
pub async fn game_watch(
    state: State<'_, AppState>,
    games: State<'_, Games>,
    watches: State<'_, Watches>,
    chat_id: String,
    channel: Channel<GameFrame>,
) -> Result<u32, AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    let mut viewer = games.watch(&chat_id, &folder);
    let task = tauri::async_runtime::spawn(async move {
        while let Some(packet) = viewer.next().await {
            if channel.send(GameFrame(packet)).is_err() {
                break;
            }
        }
    });
    Ok(watches.start(&chat_id, task.inner().abort_handle()))
}

/// Ends the stream `game_watch` numbered `watch_id`, if it is still the chat's.
#[tauri::command]
#[specta::specta]
pub async fn game_unwatch(
    watches: State<'_, Watches>,
    chat_id: String,
    watch_id: u32,
) -> Result<(), AppError> {
    watches.stop(&chat_id, Some(watch_id));
    Ok(())
}

/// The run's errors and what the game printed, for the pane's log.
#[tauri::command]
#[specta::specta]
pub async fn game_log(games: State<'_, Games>, chat_id: String) -> Result<GameLog, AppError> {
    Ok(games.pane_log(&chat_id, 200))
}

/// The person wants the game in a window of its own (or back out of sight): its next start does
/// that. A running game is started again, since a window cannot change hands while it runs.
#[tauri::command]
#[specta::specta]
pub async fn game_window(
    app: AppHandle,
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chat_id: String,
    windowed: bool,
) -> Result<(), AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    games.look(&chat_id, &folder).await;
    games.set_windowed(&chat_id, windowed);
    let running = games
        .chat(&chat_id)
        .is_some_and(|game| game.state == ghost_game::GameState::Running);
    if running {
        games.set_owner_window(owner_window(&app));
        games.run(&chat_id, &folder, None).await?;
        games.time(&chat_id, Some(false), None).await?;
    }
    Ok(())
}

/// Opens the chat's project in Godot's own editor.
#[tauri::command]
#[specta::specta]
pub async fn game_open_editor(
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chat_id: String,
) -> Result<(), AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    Ok(games.open_editor(&chat_id, &folder).await?)
}

/// Saves a picture of the chat's game to the person's Pictures (or Downloads) folder and answers
/// where it is.
#[tauri::command]
#[specta::specta]
pub async fn game_picture(
    app: AppHandle,
    games: State<'_, Games>,
    chat_id: String,
) -> Result<String, AppError> {
    let jpeg = games.picture(&chat_id).await?;
    let folder = app
        .path()
        .picture_dir()
        .or_else(|_| app.path().download_dir())
        .map_err(|_| AppError::new(AppErrorCode::Internal, "There is no folder for pictures."))?;
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let path = folder.join(format!("ghost-game-{seconds}.jpg"));
    tokio::fs::create_dir_all(&folder)
        .await
        .and(tokio::fs::write(&path, jpeg).await)
        .map_err(|error| {
            AppError::new(
                AppErrorCode::Internal,
                format!("The picture could not be saved: {error}"),
            )
        })?;
    Ok(path.to_string_lossy().into_owned())
}

/// What turning Game on for a chat did to its agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum GameToolsChange {
    /// The agent's session is starting again with the game tools.
    Restarting,
    /// The agent is busy or stopped: it gets the tools the next time its session starts.
    NextStart,
}

/// The person turned Game on for this chat (a folder a game is about to be made in), or asked
/// for the game tools in a chat whose folder became a game after its agent started. The chat's
/// idle session starts again, since an agent's tools are fixed when its session starts.
#[tauri::command]
#[specta::specta]
pub async fn game_turn_on(
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chosen: State<'_, Arc<Chosen>>,
    chat_id: String,
    on: bool,
) -> Result<GameToolsChange, AppError> {
    if !chat_id_like(&chat_id) {
        return Err(AppError::invalid_argument("That is not a chat."));
    }
    let folder = folder_of(&state, &chat_id).await?;
    chosen.set(&chat_id, on);
    games.look(&chat_id, &folder).await;
    let restarted = state
        .sessions()?
        .mcp_session_action(
            &chat_id,
            ghost_core::game_service::SERVER_NAME.to_owned(),
            ghost_protocol::mcp::McpSessionAction::Restart,
        )
        .await
        .is_ok();
    Ok(if restarted {
        GameToolsChange::Restarting
    } else {
        GameToolsChange::NextStart
    })
}

/// The person pressed Start a game: one of Ghost's own small projects is written into the
/// chat's folder, which must hold no game and none of its files yet. Game is turned on for the
/// chat, and its idle session starts again with the game tools.
#[tauri::command]
#[specta::specta]
pub async fn game_start_project(
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chosen: State<'_, Arc<Chosen>>,
    chat_id: String,
    starter: Starter,
) -> Result<GameToolsChange, AppError> {
    if !chat_id_like(&chat_id) {
        return Err(AppError::invalid_argument("That is not a chat."));
    }
    let folder = folder_of(&state, &chat_id).await?;
    games.start_project(&chat_id, &folder, starter).await?;
    chosen.set(&chat_id, true);
    let restarted = state
        .sessions()?
        .mcp_session_action(
            &chat_id,
            ghost_core::game_service::SERVER_NAME.to_owned(),
            ghost_protocol::mcp::McpSessionAction::Restart,
        )
        .await
        .is_ok();
    Ok(if restarted {
        GameToolsChange::Restarting
    } else {
        GameToolsChange::NextStart
    })
}

/// Whether the agent tools of the engine this chat's project is made with (Unity, Unreal,
/// Roblox) are within reach on this computer now, for the pane's Connect card.
#[tauri::command]
#[specta::specta]
pub async fn game_probe(
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chat_id: String,
) -> Result<Option<EngineProbe>, AppError> {
    let folder = folder_of(&state, &chat_id).await?;
    let Some(project) = games.look(&chat_id, &folder).await else {
        return Ok(None);
    };
    Ok(Some(ghost_game::probe::probe(project.engine).await))
}

/// What connecting an engine's own agent tools to a project did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum GameConnected {
    /// The engine's server is one of this project's MCP servers now.
    Added,
    /// It already was.
    Already,
}

/// The MCP server an engine's maker gives for it, as its own guide writes it. `None` for
/// engines that are connected another way (Unity, from its own CLI) or run by Ghost.
fn vendor_server(engine: GameEngine) -> Option<(&'static str, ghost_protocol::mcp::McpTransport)> {
    use ghost_protocol::mcp::McpTransport;
    match engine {
        GameEngine::Unreal => Some((
            "unreal-mcp",
            McpTransport::Http {
                url: ghost_game::probe::UNREAL_MCP.to_owned(),
            },
        )),
        GameEngine::Roblox if cfg!(windows) => Some((
            "Roblox_Studio",
            McpTransport::Stdio {
                command: "cmd.exe".to_owned(),
                args: vec![
                    "/c".to_owned(),
                    "%LOCALAPPDATA%\\Roblox\\mcp.bat".to_owned(),
                ],
            },
        )),
        GameEngine::Roblox if cfg!(target_os = "macos") => Some((
            "Roblox_Studio",
            McpTransport::Stdio {
                command: "/Applications/RobloxStudio.app/Contents/MacOS/StudioMCP".to_owned(),
                args: Vec::new(),
            },
        )),
        _ => None,
    }
}

/// The person pressed "Add to this project" on a Connect card: the engine's own MCP server, as
/// its maker's guide gives it, becomes one of the MCP servers of the chat's project, for the
/// agents that can take it. The person's, from the pane: no tool calls this, and the address or
/// program is Ghost's own constant for that engine, never the page's.
#[tauri::command]
#[specta::specta]
pub async fn game_connect(
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chat_id: String,
) -> Result<GameConnected, AppError> {
    use ghost_core::mcp::Command;
    let folder = folder_of(&state, &chat_id).await?;
    let project = games.look(&chat_id, &folder).await.ok_or_else(|| {
        AppError::invalid_argument("This chat's folder holds no game Ghost knows.")
    })?;
    let (name, transport) = vendor_server(project.engine).ok_or_else(|| {
        AppError::invalid_argument(format!(
            "{} is not connected through an MCP server Ghost can add.",
            project.engine.name()
        ))
    })?;
    let sessions = state.sessions()?;
    let workspace = sessions
        .workspace_snapshot(ghost_protocol::workspace::WorkspaceFilter {
            include_archived: true,
            project_id: None,
        })
        .await?;
    let project_id = workspace
        .chats
        .iter()
        .find(|entry| entry.chat.id == chat_id)
        .map(|entry| entry.project_id.clone())
        .ok_or_else(|| AppError::invalid_argument("That chat is in no project."))?;
    let snapshot = sessions.shared_mcp(Command::Snapshot).await?;
    let taken = snapshot.servers.iter().any(|server| {
        server.definition.name == name
            && server
                .definition
                .project_id
                .as_deref()
                .is_none_or(|scope| scope == project_id)
    });
    if taken {
        return Ok(GameConnected::Already);
    }
    sessions
        .shared_mcp(Command::Save {
            expected: snapshot.revision,
            definition: ghost_protocol::mcp::McpDefinition {
                id: uuid::Uuid::now_v7().to_string(),
                name: name.to_owned(),
                project_id: Some(project_id),
                enabled: true,
                transport,
                env_keys: Vec::new(),
                header_keys: Vec::new(),
                agents: None,
            },
        })
        .await?;
    Ok(GameConnected::Added)
}

/// Whether the person turned Game on for this chat themselves.
#[tauri::command]
#[specta::specta]
pub async fn game_turned_on(
    chosen: State<'_, Arc<Chosen>>,
    chat_id: String,
) -> Result<bool, AppError> {
    Ok(chosen.has(&chat_id))
}

/// Runs one of the agents' game tools for a chat, as an agent's call would (`input` is its
/// arguments, as JSON). Development builds only: the checks drive every tool through it. A
/// release build refuses, so the window never acts as an agent there.
#[tauri::command]
#[specta::specta]
pub async fn game_agent_call(
    state: State<'_, AppState>,
    games: State<'_, Games>,
    chat_id: String,
    tool: String,
    input: String,
) -> Result<GameToolCheck, AppError> {
    if !cfg!(debug_assertions) {
        return Err(AppError::invalid_argument(
            "Only development builds run the game tools from the window.",
        ));
    }
    let arguments: serde_json::Value = serde_json::from_str(&input)
        .map_err(|_| AppError::invalid_argument("The arguments are not JSON."))?;
    let folder = folder_of(&state, &chat_id).await?;
    let reply = games.tool(&chat_id, &folder, &tool, &arguments).await;
    Ok(GameToolCheck {
        text: reply.text,
        images: u32::try_from(reply.images.len()).unwrap_or(u32::MAX),
        image_length: u32::try_from(reply.images.iter().map(Vec::len).sum::<usize>())
            .unwrap_or(u32::MAX),
        error: reply.error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> (tokio::task::JoinHandle<()>, AbortHandle) {
        let task = tokio::spawn(std::future::pending::<()>());
        let handle = task.abort_handle();
        (task, handle)
    }

    /// The pane closed and opened again: the first stream's late stop must not end the second.
    #[tokio::test]
    async fn a_late_stop_cannot_end_the_stream_that_replaced_it() {
        let watches = Watches::default();
        let (first, first_handle) = task();
        let (second, second_handle) = task();
        let one = watches.start("chat", first_handle);
        let two = watches.start("chat", second_handle);
        assert_ne!(one, two);
        assert!(first.await.unwrap_err().is_cancelled());
        watches.stop("chat", Some(one));
        tokio::task::yield_now().await;
        assert!(!second.is_finished(), "the stale stop did nothing");
        watches.stop("chat", Some(two));
        assert!(second.await.unwrap_err().is_cancelled());
    }

    /// The chats the person turned Game on for survive a restart of the app, and the file can
    /// hold nothing but chat ids however it was written.
    #[test]
    fn the_chats_game_is_on_for_are_kept_and_read_back_carefully() {
        let folder = tempfile::tempdir().unwrap();
        let chosen = Chosen::open(folder.path());
        assert!(!chosen.has("chat-1"));
        chosen.set("chat-1", true);
        chosen.set("chat-2", true);
        chosen.set("chat-1", true);
        chosen.set("chat-2", false);
        assert!(chosen.has("chat-1") && !chosen.has("chat-2"));
        let reopened = Chosen::open(folder.path());
        assert!(reopened.has("chat-1") && !reopened.has("chat-2"));

        // The oldest go once there are more than the file keeps.
        for number in 0..MAX_CHOSEN + 3 {
            reopened.set(&format!("chat-n{number}"), true);
        }
        let reopened = Chosen::open(folder.path());
        assert!(!reopened.has("chat-1") && !reopened.has("chat-n2"));
        assert!(reopened.has("chat-n3") && reopened.has(&format!("chat-n{}", MAX_CHOSEN + 2)));

        // A file somebody else wrote: only what could be a chat id is read, and junk reads as none.
        std::fs::write(
            folder.path().join("chats.json"),
            br#"["good-1", "../../etc", "", "has space", 7]"#,
        )
        .unwrap();
        assert!(
            !Chosen::open(folder.path()).has("good-1"),
            "one wrong entry voids the file"
        );
        std::fs::write(
            folder.path().join("chats.json"),
            br#"["good-1", "../../etc", "", "has space"]"#,
        )
        .unwrap();
        let careful = Chosen::open(folder.path());
        assert!(careful.has("good-1"));
        assert!(!careful.has("../../etc") && !careful.has("has space") && !careful.has(""));
        std::fs::write(folder.path().join("chats.json"), b"not json").unwrap();
        assert!(!Chosen::open(folder.path()).has("good-1"));
        assert!(chat_id_like("01a2202f-9358-74f8-9459-64b2c9774603"));
        assert!(!chat_id_like(&"a".repeat(65)));
    }
}
