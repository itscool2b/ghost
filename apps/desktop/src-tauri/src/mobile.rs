//! The Device pane (docs/superpowers/plans/2026-11-21-mobile.md): o7's own window's commands for
//! the devices `o7-devices` runs, the video they send to the pane, and the agents' bridge to them.
//!
//! Everything here is by chat id. The pane's commands are the person's: setting Android up and
//! accepting Google's licence, choosing a phone, and letting an agent use it exist only here, never
//! among the agents' tools.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine;
use o7_devices::manager::{Config, DEFAULT_RUNNING, DeviceKind, Manager, MobileStatus};
use o7_devices::scrcpy::{Touch, key};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody, IpcResponse};
use tauri::{AppHandle, Emitter, Manager as _, State};
use tokio::task::AbortHandle;

use crate::error::{AppError, AppErrorCode};

/// The event that carries every change: its payload is the whole [`MobileStatus`].
pub const EVENT: &str = "o7-mobile";

impl From<o7_devices::Error> for AppError {
    fn from(error: o7_devices::Error) -> Self {
        let code = match error {
            o7_devices::Error::Refused(_) => AppErrorCode::InvalidArgument,
            _ => AppErrorCode::Internal,
        };
        AppError::new(code, error.to_string())
    }
}

/// The video streams the pane is watching, one per chat, so a new one ends the one before it.
/// Each has a number: the pane stops the stream it started, and a stop that arrives late (the pane
/// closed and opened again meanwhile) cannot end the stream that replaced it.
#[derive(Default)]
pub struct Watches {
    next: AtomicU32,
    running: Mutex<HashMap<String, (u32, AbortHandle)>>,
}

impl Watches {
    /// Makes `task` the chat's stream, ending the one before it. Answers its number.
    fn start(&self, chat_id: &str, task: AbortHandle) -> u32 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, before)) = running.insert(chat_id.to_owned(), (id, task)) {
            before.abort();
        }
        id
    }

    /// Ends the chat's stream: the one numbered `id`, or whichever runs when none is named.
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

/// One packet of a device's video, sent to the page as raw bytes (the page reads an
/// `ArrayBuffer`): a byte of kind, then the payload (`o7_devices::manager::Viewer`).
#[derive(Debug, Clone, specta::Type)]
pub struct VideoPacket(Vec<u8>);

impl IpcResponse for VideoPacket {
    fn body(self) -> tauri::Result<InvokeResponseBody> {
        Ok(InvokeResponseBody::Raw(self.0))
    }
}

/// A finger's phase, from the pane's pointer events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

/// The device's buttons and the keys the pane forwards from the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKey {
    Back,
    Home,
    Recents,
    Power,
    VolumeUp,
    VolumeDown,
    Enter,
    Delete,
    ForwardDelete,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
}

/// What one of the agents' tools answered, for the development checks.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct MobileToolCheck {
    pub text: String,
    /// The screenshot's size in bytes, when the tool returned one.
    pub image_length: u32,
    pub error: bool,
}

/// What `chat_id`'s emulator or iPhone simulator shows now, as a PNG, for a bot that asked to
/// look (`crate::remote`). Never a phone or another program's emulator: those are the person's
/// own, with whatever else is on them.
pub(crate) async fn picture(app: &AppHandle, chat_id: &str) -> Result<Vec<u8>, String> {
    let manager = app
        .try_state::<Manager>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| "Mobile is not available in this run.".to_owned())?;
    let status = manager.status();
    let Some(device) = status.chats.iter().find(|device| device.chat_id == chat_id) else {
        return Err("This chat has no device.".into());
    };
    if device.kind == DeviceKind::Phone {
        return Err(
            "This chat's device is a phone or another program's emulator. o7 sends pictures of its own emulators and simulators only."
                .into(),
        );
    }
    if !device.live {
        return Err("This chat's device is not running.".into());
    }
    manager
        .screenshot(chat_id)
        .await
        .map_err(|error| error.to_string())
}

/// Agents' calls from o7-core's mobile grants, run on the chat's device.
struct AgentBridge(Manager);

impl o7_core::mobile_service::MobileBridge for AgentBridge {
    fn call(
        &self,
        chat_id: String,
        tool: String,
        arguments: serde_json::Value,
    ) -> o7_core::mobile_service::MobileFuture {
        let manager = self.0.clone();
        Box::pin(async move {
            let reply = manager.tool(&chat_id, &tool, &arguments).await;
            o7_core::mobile_service::MobileToolReply {
                text: reply.text,
                image: reply.image.map(|png| {
                    (
                        "image/png".to_owned(),
                        base64::engine::general_purpose::STANDARD.encode(png),
                    )
                }),
                error: reply.error,
            }
        })
    }
}

/// Creates the device manager during setup. Nothing is started and nothing is looked for until
/// the pane is opened or an agent asks.
pub fn setup(app: &AppHandle) {
    let Some(root) = app
        .try_state::<crate::state::AppState>()
        .and_then(|state| state.data_dir().map(|dir| dir.join("devices")))
    else {
        return;
    };
    let handle = app.clone();
    let manager = Manager::new(
        Config {
            root,
            places: o7_devices::sdk::Places::current(),
            adb: o7_devices::adb::Adb::from_env(),
            max_running: DEFAULT_RUNNING,
        },
        move || {
            if let Some(manager) = handle.try_state::<Manager>() {
                let _ = handle.emit_to(crate::window::MAIN_WINDOW, EVENT, manager.status());
            }
        },
    );
    app.manage(manager);
    app.manage(Watches::default());
    sync(app);
}

/// The active profile's sessions get their bridge to the devices (after setup and a profile
/// switch).
pub fn sync(app: &AppHandle) {
    let Some(manager) = app
        .try_state::<Manager>()
        .map(|state| state.inner().clone())
    else {
        return;
    };
    if let Some(chats) = app
        .try_state::<crate::state::AppState>()
        .and_then(|state| state.chats())
    {
        chats
            .manager
            .set_mobile_bridge(Some(Arc::new(AgentBridge(manager))));
    }
}

/// Settings' switch for Mobile. Off stops every device o7 started.
pub fn apply_preferences(app: &AppHandle, general: &o7_core::onboarding::GeneralPreferences) {
    sync(app);
    let Some(manager) = app
        .try_state::<Manager>()
        .map(|state| state.inner().clone())
    else {
        return;
    };
    if manager.enabled() == general.mobile {
        return;
    }
    if !general.mobile
        && let Some(watches) = app.try_state::<Watches>()
    {
        watches.clear();
    }
    let enabled = general.mobile;
    tauri::async_runtime::spawn(async move { manager.set_enabled(enabled).await });
}

/// o7 is quitting: its emulators save their state and stop before the app is gone.
pub fn shutdown(app: &AppHandle) {
    if let Some(watches) = app.try_state::<Watches>() {
        watches.clear();
    }
    if let Some(manager) = app
        .try_state::<Manager>()
        .map(|state| state.inner().clone())
    {
        tauri::async_runtime::block_on(manager.shutdown());
    }
}

type Devices<'a> = State<'a, Manager>;

#[tauri::command]
#[specta::specta]
pub async fn mobile_status(devices: Devices<'_>) -> Result<MobileStatus, AppError> {
    Ok(devices.status())
}

/// The pane was opened: look again at what is installed, and start watching for phones.
#[tauri::command]
#[specta::specta]
pub async fn mobile_refresh(devices: Devices<'_>) -> Result<MobileStatus, AppError> {
    devices.refresh().await;
    Ok(devices.status())
}

/// Reads Google's index and shows what would be downloaded, with its licence.
#[tauri::command]
#[specta::specta]
pub async fn mobile_fetch_offer(devices: Devices<'_>) -> Result<(), AppError> {
    let manager = devices.inner().clone();
    tauri::async_runtime::spawn(async move { manager.fetch_offer().await });
    Ok(())
}

/// The person read the offer and accepted Google's licence: download what it listed.
#[tauri::command]
#[specta::specta]
pub async fn mobile_fetch_start(devices: Devices<'_>) -> Result<(), AppError> {
    Ok(devices.fetch_start()?)
}

#[tauri::command]
#[specta::specta]
pub async fn mobile_fetch_cancel(devices: Devices<'_>) -> Result<(), AppError> {
    devices.fetch_cancel();
    Ok(())
}

/// Gives the chat a device and starts it: the one named, or one of o7's emulators.
#[tauri::command]
#[specta::specta]
pub async fn mobile_attach(
    devices: Devices<'_>,
    chat_id: String,
    device_id: Option<String>,
) -> Result<(), AppError> {
    Ok(devices.attach(&chat_id, device_id.as_deref())?)
}

#[tauri::command]
#[specta::specta]
pub async fn mobile_detach(
    devices: Devices<'_>,
    watches: State<'_, Watches>,
    chat_id: String,
) -> Result<(), AppError> {
    watches.stop(&chat_id, None);
    devices.detach(&chat_id);
    Ok(())
}

/// Stops one of o7's emulators.
#[tauri::command]
#[specta::specta]
pub async fn mobile_stop_device(devices: Devices<'_>, device_id: String) -> Result<(), AppError> {
    Ok(devices.stop_device(&device_id).await?)
}

/// The person's answer to "may the agent use this phone?", for the chat's device.
#[tauri::command]
#[specta::specta]
pub async fn mobile_allow_agent(
    devices: Devices<'_>,
    chat_id: String,
    allowed: bool,
) -> Result<(), AppError> {
    devices.allow_agent(&chat_id, allowed);
    Ok(())
}

/// Streams the chat's device to `channel` until [`mobile_unwatch`], a new watch for the chat, or
/// the stream's own end. Answers the stream's number, for `mobile_unwatch`.
#[tauri::command]
#[specta::specta]
pub async fn mobile_watch(
    devices: Devices<'_>,
    watches: State<'_, Watches>,
    chat_id: String,
    channel: Channel<VideoPacket>,
) -> Result<u32, AppError> {
    let mut viewer = devices.watch(&chat_id).await?;
    let task = tauri::async_runtime::spawn(async move {
        while let Some(packet) = viewer.next().await {
            if channel.send(VideoPacket(packet)).is_err() {
                break;
            }
        }
    });
    Ok(watches.start(&chat_id, task.inner().abort_handle()))
}

/// Ends the stream `mobile_watch` numbered `watch_id`, if it is still the chat's.
#[tauri::command]
#[specta::specta]
pub async fn mobile_unwatch(
    watches: State<'_, Watches>,
    chat_id: String,
    watch_id: u32,
) -> Result<(), AppError> {
    watches.stop(&chat_id, Some(watch_id));
    Ok(())
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
        assert!(
            first.await.unwrap_err().is_cancelled(),
            "a new stream ends the one before"
        );
        watches.stop("chat", Some(one));
        tokio::task::yield_now().await;
        assert!(!second.is_finished(), "the stale stop did nothing");
        watches.stop("other", Some(two));
        assert!(!second.is_finished(), "nor a stop for another chat");
        watches.stop("chat", Some(two));
        assert!(second.await.unwrap_err().is_cancelled());
        // Letting go of the device ends whichever stream runs.
        let (third, third_handle) = task();
        watches.start("chat", third_handle);
        watches.stop("chat", None);
        assert!(third.await.unwrap_err().is_cancelled());
        watches.stop("chat", None);
    }
}

/// The person's finger, at a point of the video they are watching.
#[tauri::command]
#[specta::specta]
pub async fn mobile_touch(
    devices: Devices<'_>,
    chat_id: String,
    phase: TouchPhase,
    x: u32,
    y: u32,
) -> Result<(), AppError> {
    let phase = match phase {
        TouchPhase::Down => Touch::Down,
        TouchPhase::Move => Touch::Move,
        TouchPhase::Up => Touch::Up,
    };
    Ok(devices.touch(&chat_id, phase, x, y).await?)
}

/// The person's wheel or trackpad, at a point of the video: notches right and down.
#[tauri::command]
#[specta::specta]
pub async fn mobile_scroll(
    devices: Devices<'_>,
    chat_id: String,
    x: u32,
    y: u32,
    horizontal: f32,
    vertical: f32,
) -> Result<(), AppError> {
    if !horizontal.is_finite() || !vertical.is_finite() {
        return Err(AppError::invalid_argument("A scroll is a number."));
    }
    Ok(devices.scroll(&chat_id, x, y, horizontal, vertical).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn mobile_key(
    devices: Devices<'_>,
    chat_id: String,
    key: DeviceKey,
) -> Result<(), AppError> {
    let code = match key {
        DeviceKey::Back => return Ok(devices.back(&chat_id).await?),
        DeviceKey::Home => key::HOME,
        DeviceKey::Recents => key::APP_SWITCH,
        DeviceKey::Power => key::POWER,
        DeviceKey::VolumeUp => key::VOLUME_UP,
        DeviceKey::VolumeDown => key::VOLUME_DOWN,
        DeviceKey::Enter => key::ENTER,
        DeviceKey::Delete => key::DELETE_BACK,
        DeviceKey::ForwardDelete => key::DELETE_FORWARD,
        DeviceKey::Tab => key::TAB,
        DeviceKey::Escape => key::ESCAPE,
        DeviceKey::Up => key::DPAD_UP,
        DeviceKey::Down => key::DPAD_DOWN,
        DeviceKey::Left => key::DPAD_LEFT,
        DeviceKey::Right => key::DPAD_RIGHT,
    };
    Ok(devices.press(&chat_id, code).await?)
}

/// Text the person typed or pasted into the pane.
#[tauri::command]
#[specta::specta]
pub async fn mobile_text(
    devices: Devices<'_>,
    chat_id: String,
    text: String,
) -> Result<(), AppError> {
    Ok(devices.text(&chat_id, &text).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn mobile_rotate(devices: Devices<'_>, chat_id: String) -> Result<(), AppError> {
    Ok(devices.rotate(&chat_id).await?)
}

/// Saves a picture of the chat's screen to the person's Pictures (or Downloads) folder and
/// answers where it is.
#[tauri::command]
#[specta::specta]
pub async fn mobile_screenshot(
    app: AppHandle,
    devices: Devices<'_>,
    chat_id: String,
) -> Result<String, AppError> {
    let png = devices.screenshot(&chat_id).await?;
    let folder = app
        .path()
        .picture_dir()
        .or_else(|_| app.path().download_dir())
        .map_err(|_| AppError::new(AppErrorCode::Internal, "There is no folder for pictures."))?;
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let path = folder.join(format!("o7-device-{seconds}.png"));
    tokio::fs::create_dir_all(&folder)
        .await
        .and(tokio::fs::write(&path, png).await)
        .map_err(|error| {
            AppError::new(
                AppErrorCode::Internal,
                format!("The picture could not be saved: {error}"),
            )
        })?;
    Ok(path.to_string_lossy().into_owned())
}

/// Runs one of the agents' device tools for a chat, as an agent's call would (`input` is its
/// arguments, as JSON). Development builds only: the checks drive every tool through it. A
/// release build refuses, so the window never acts as an agent there.
#[tauri::command]
#[specta::specta]
pub async fn mobile_agent_call(
    devices: Devices<'_>,
    chat_id: String,
    tool: String,
    input: String,
) -> Result<MobileToolCheck, AppError> {
    if !cfg!(debug_assertions) {
        return Err(AppError::invalid_argument(
            "Only development builds run the device tools from the window.",
        ));
    }
    let arguments: serde_json::Value = serde_json::from_str(&input)
        .map_err(|_| AppError::invalid_argument("The arguments are not JSON."))?;
    let reply = devices.tool(&chat_id, &tool, &arguments).await;
    Ok(MobileToolCheck {
        text: reply.text,
        image_length: reply
            .image
            .as_ref()
            .map_or(0, |png| u32::try_from(png.len()).unwrap_or(u32::MAX)),
        error: reply.error,
    })
}
