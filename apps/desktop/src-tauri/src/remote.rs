//! Remote access: paired browsers, Ghost for iPhone and linked bots reaching this computer's chats
//! through the relay (`ghost-remote`, `docs/remote-sessions.md`). Off until the person turns it on
//! in Settings.
//!
//! The service belongs to the installation: one connection to the relay, whichever profile is
//! open. A device or a bot link belongs to the profile it was made in and reaches no other
//! (`ghost_remote::Registry`): when another profile opens, browsers are told to start over, the
//! other profile's devices are ended, and the relay is given only this profile's bot links.

use std::sync::Arc;

use ghost_core::session::{FrameSink, SinkGone, StatusSink};
use ghost_protocol::{
    EventBatch, UiAnswer,
    remote::{RemoteBotLink, RemotePairing, RemoteStatus},
    search::ChatSearchHit,
    session::{Availability, ChatAttach, ChatStatusUpdate, HistoryPage, PromptAck},
    workspace::{ProjectChatCreate, WorkspaceChat, WorkspaceFilter},
};
use ghost_remote::{
    Backend, BackendError, BoxFuture, Frames, Overview, RegistryError, RemoteAgent, Service,
    Statuses,
};
use tauri::{Emitter, Manager, State};

use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};

/// Sent with a [`RemoteStatus`] whenever what Settings shows about remote access changed.
pub const REMOTE_CHANGED: &str = "auracode-remote-changed";
/// History pages a browser asks for after the first: small enough to seal into one message.
const PAGE_BYTES: u32 = 256 * 1024;
/// The longer side of a picture a bot is shown, and its JPEG quality: small enough for the
/// relay's messages (2 MiB) and a bot's own limits, large enough to read a page.
const PICTURE_SIDE: u32 = 1600;
const PICTURE_QUALITY: u8 = 75;
/// The most a picture may take on the wire, in base64.
const PICTURE_BYTES: usize = 1536 * 1024;

/// A pull request as GitHub CLI read it, as the remote service takes one.
fn pull_request(view: ghost_core::gitworkspaces::PullRequestView) -> ghost_remote::PullRequest {
    use ghost_core::gitworkspaces::Checks;
    ghost_remote::PullRequest {
        number: view.number,
        url: view.url,
        state: view.state,
        checks: match view.checks {
            Checks::None => ghost_remote::PrChecks::None,
            Checks::Pending => ghost_remote::PrChecks::Pending,
            Checks::Passed => ghost_remote::PrChecks::Passed,
            Checks::Failed(names) => ghost_remote::PrChecks::Failed(names),
        },
        notes: view
            .notes
            .into_iter()
            .map(|note| ghost_remote::PrNote {
                author: note.author,
                kind: note.kind,
                body: note.body,
                trusted: note.trusted,
            })
            .collect(),
    }
}

/// How long a reading reuses what GitHub CLI said of a chat's pull request: a bot reads often,
/// and each look is a call to GitHub.
const PR_FRESH: std::time::Duration = std::time::Duration::from_secs(120);
/// How long what GitHub CLI said is kept for Ghost for iPhone's Inbox, which shows it beside a chat
/// that finished in the last day and never asks GitHub itself.
const PR_KEPT: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// What GitHub CLI last said of each chat's pull request, and when, by chat: a bot's reading
/// names it in one line, and Ghost for iPhone shows its parts, from the same look.
type PrSaid = std::sync::Mutex<
    std::collections::HashMap<String, (std::time::Instant, Option<ghost_remote::PullRequest>)>,
>;

fn pr_said() -> &'static PrSaid {
    static SAID: std::sync::OnceLock<PrSaid> = std::sync::OnceLock::new();
    SAID.get_or_init(Default::default)
}

/// The pull request open from `chat_id`'s branch: what GitHub CLI said within [`PR_FRESH`], or
/// what it says now, kept for the next reading.
async fn pull_request_of(
    sessions: &ghost_core::session::SessionManager,
    chat_id: &str,
) -> Result<Option<ghost_remote::PullRequest>, AppError> {
    let fresh = pr_said()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(chat_id)
        .filter(|(at, _)| at.elapsed() < PR_FRESH)
        .map(|(_, said)| said.clone());
    if let Some(said) = fresh {
        return Ok(said);
    }
    let said = sessions.pull_request(chat_id).await?.map(pull_request);
    keep_pull_request(chat_id, said.clone());
    Ok(said)
}

/// Keeps what GitHub CLI said of `chat_id`'s pull request. What is a day old goes, so the cache
/// holds only the chats looked at in the last day.
fn keep_pull_request(chat_id: &str, said: Option<ghost_remote::PullRequest>) {
    let mut kept = pr_said()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    kept.retain(|_, (at, _)| at.elapsed() < PR_KEPT);
    kept.insert(chat_id.to_owned(), (std::time::Instant::now(), said));
}

/// What GitHub CLI last said of `chat_id`'s pull request in the last day, without asking again.
fn known_pull_request(chat_id: &str) -> Option<ghost_remote::PullRequest> {
    pr_said()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(chat_id)
        .filter(|(at, _)| at.elapsed() < PR_KEPT)
        .and_then(|(_, said)| said.clone())
}

/// The words the remote service uses for how a file changed.
fn change_word(kind: ghost_protocol::changes::ChangeKind) -> &'static str {
    match kind {
        ghost_protocol::changes::ChangeKind::Added => "added",
        ghost_protocol::changes::ChangeKind::Modified => "modified",
        ghost_protocol::changes::ChangeKind::Deleted => "deleted",
    }
}

/// The base64 length of `bytes` bytes, padded.
fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

/// The longer sides and JPEG qualities a picture for a paired device steps down through until
/// its base64 fits the answer: the first is a bot's own picture.
const STEPS: [(u32, u8); 5] = [
    (PICTURE_SIDE, PICTURE_QUALITY),
    (1280, 70),
    (1024, 65),
    (800, 60),
    (640, 55),
];

/// `picture` as a JPEG whose base64 is at most `max_base64`, stepping down through [`STEPS`];
/// none when even the smallest is larger. A picture with transparency is laid on white.
fn jpeg_within(
    picture: &image::DynamicImage,
    max_base64: usize,
) -> Result<Option<(Vec<u8>, u32, u32)>, String> {
    for (side, quality) in STEPS {
        let sized = if picture.width().max(picture.height()) > side {
            picture.resize(side, side, image::imageops::FilterType::Triangle)
        } else {
            picture.clone()
        };
        let mut out = Vec::new();
        on_white(&sized)
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut out, quality,
            ))
            .map_err(|error| format!("The picture could not be made: {error}"))?;
        if base64_len(out.len()) <= max_base64 {
            return Ok(Some((out, sized.width(), sized.height())));
        }
    }
    Ok(None)
}

/// `picture` with any transparency laid on white, as a JPEG holds it.
fn on_white(picture: &image::DynamicImage) -> image::RgbImage {
    if !picture.color().has_alpha() {
        return picture.to_rgb8();
    }
    let rgba = picture.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let over = |channel: u8| {
            let alpha = u32::from(a);
            let laid = (u32::from(channel) * alpha + 255 * (255 - alpha)) / 255;
            u8::try_from(laid).unwrap_or(u8::MAX)
        };
        image::Rgb([over(r), over(g), over(b)])
    })
}

/// The hour on the computer's own clock, 0 to 23, for work bots leave for tonight.
#[allow(unsafe_code)] // Read-only clock queries; the contract is stated at each call.
fn local_hour() -> Option<u8> {
    #[cfg(windows)]
    {
        // SAFETY: GetLocalTime only writes the SYSTEMTIME it is given.
        let mut now: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut now) };
        u8::try_from(now.wHour).ok().filter(|hour| *hour < 24)
    }
    #[cfg(unix)]
    {
        // SAFETY: localtime_r is the reentrant form: it writes only the tm it is given.
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        let mut parts: libc::tm = unsafe { std::mem::zeroed() };
        let read = unsafe { libc::localtime_r(&now, &mut parts) };
        (!read.is_null())
            .then(|| u8::try_from(parts.tm_hour).ok())
            .flatten()
            .filter(|hour| *hour < 24)
    }
    #[cfg(not(any(windows, unix)))]
    None
}

/// `bytes` (a PNG or a JPEG) as a JPEG no larger than [`PICTURE_SIDE`] on its longer side.
fn shrink(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let picture = image::load_from_memory(bytes)
        .map_err(|error| format!("The picture could not be read: {error}"))?;
    let picture = if picture.width().max(picture.height()) > PICTURE_SIDE {
        picture.resize(
            PICTURE_SIDE,
            PICTURE_SIDE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        picture
    };
    let mut out = Vec::new();
    picture
        .to_rgb8()
        .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut out,
            PICTURE_QUALITY,
        ))
        .map_err(|error| format!("The picture could not be made: {error}"))?;
    Ok(out)
}

/// The relay and the website it belongs to. Release builds reach only Ghost's own; a
/// development build may name a relay on this machine. The site was auracode.si until
/// 19 November 2026: the relay still answers there for copies from before the move
/// (docs/remote-sessions.md), and a copy from after it never goes there.
fn origins() -> (String, String) {
    if cfg!(debug_assertions)
        && let (Ok(relay), Ok(site)) = (
            std::env::var("GHOST_REMOTE_RELAY"),
            std::env::var("GHOST_REMOTE_SITE"),
        )
    {
        return (relay, site);
    }
    let site = match option_env!("GHOST_API_ORIGIN") {
        Some("https://preview.onghost.dev") => "https://preview.onghost.dev",
        _ => "https://onghost.dev",
    };
    (site.replacen("https://", "wss://", 1), site.to_owned())
}

/// Starts the service once the app's state exists. Smoke runs never get one.
pub(crate) fn setup(app: &tauri::AppHandle, runtime: &tokio::runtime::Handle) {
    let state = app.state::<AppState>();
    if state.launch.is_smoke() {
        return;
    }
    let Some(folder) = state.data_dir() else {
        return;
    };
    let (relay, site) = origins();
    let host_name = state
        .app_info()
        .device
        .map(|device| device.name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "This computer".into());
    let notify = app.clone();
    let config = ghost_remote::Config {
        path: folder.join("remote.json"),
        relay,
        site,
        host_name,
        profile: open_profile(&state),
        backend: Arc::new(AppBackend {
            app: app.clone(),
            profile: String::new(),
        }),
        changed: Arc::new(move || {
            if let Some(service) = notify.try_state::<Service>() {
                crate::native_shell::keep_reachable(&notify, service.wants_awake());
                let _ = notify.emit(REMOTE_CHANGED, service.status());
            }
        }),
        // A chat a bot acted on finished or asks: its link's address is told, off this thread.
        deliver: {
            let app = app.clone();
            let runtime = runtime.clone();
            Arc::new(move |delivery| {
                let app = app.clone();
                runtime.spawn(async move {
                    let bot = delivery.bot_id.clone();
                    let told = crate::remote_callback::send(delivery).await;
                    if !told.ok {
                        // Why, never where: the address and the key stay out of the log.
                        tracing::warn!(detail = %told.detail, "a bot's call-back was not delivered");
                    }
                    if let Some(service) = app.try_state::<Service>() {
                        service.note_told(&bot, told.ok, told.detail);
                    }
                });
            })
        },
    };
    match Service::start(config, runtime) {
        Ok(service) => {
            crate::native_shell::keep_reachable(app, service.wants_awake());
            app.manage(service);
            // A link that expires unused changes nothing else, so the hold is looked at again
            // now and then.
            let app = app.clone();
            runtime.spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
                    let Some(service) = app.try_state::<Service>() else {
                        return;
                    };
                    crate::native_shell::keep_reachable(&app, service.wants_awake());
                }
            });
        }
        // Settings shows why; nothing else depends on remote access.
        Err(error) => {
            tracing::warn!(%error, "remote access is unavailable");
            app.manage(Unavailable(error.to_string()));
        }
    }
}

/// Why the service could not start: its saved settings could not be read.
struct Unavailable(String);

/// The open profile's id, which a device or a bot link is bound to; empty while none is open.
fn open_profile(state: &AppState) -> String {
    state
        .app_info()
        .profile
        .map(|profile| profile.id)
        .unwrap_or_default()
}

pub(crate) fn profile_changed(app: &tauri::AppHandle) {
    if let Some(service) = app.try_state::<Service>() {
        service.profile_changed(&open_profile(&app.state::<AppState>()));
    }
}

pub(crate) fn shutdown(app: &tauri::AppHandle) {
    if let Some(service) = app.try_state::<Service>() {
        service.shutdown();
    }
}

fn service(app: &tauri::AppHandle) -> Result<Service, AppError> {
    if let Some(service) = app.try_state::<Service>() {
        return Ok(service.inner().clone());
    }
    Err(AppError::new(
        AppErrorCode::StorageUnavailable,
        match app.try_state::<Unavailable>() {
            Some(reason) => reason.0.clone(),
            None => "Remote access is not available in this run.".into(),
        },
    ))
}

fn refused(error: RegistryError) -> AppError {
    let code = match error {
        RegistryError::TooManyDevices | RegistryError::TooManyBots | RegistryError::NoWorkspace => {
            AppErrorCode::InvalidArgument
        }
        _ => AppErrorCode::StorageUnavailable,
    };
    AppError::new(code, error.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn remote_status(app: tauri::AppHandle) -> Result<RemoteStatus, AppError> {
    Ok(service(&app)?.status())
}

/// Turns remote access on or off. Off closes the connection at once; paired devices and bot
/// links stay saved, and work again when it is back on.
#[tauri::command]
#[specta::specta]
pub fn remote_set_enabled(app: tauri::AppHandle, enabled: bool) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.set_enabled(enabled).map_err(refused)?;
    Ok(service.status())
}

/// Keeps the computer awake for remote access, or stops: it does not sleep on its own while
/// remote access is on and a paired device or a bot link can reach it. The screen still turns
/// off.
#[tauri::command]
#[specta::specta]
pub fn remote_set_keep_awake(app: tauri::AppHandle, on: bool) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.set_keep_awake(on).map_err(refused)?;
    Ok(service.status())
}

/// Opens a pairing link, good once and for ten minutes. It replaces the link before it.
#[tauri::command]
#[specta::specta]
pub fn remote_pair(app: tauri::AppHandle) -> Result<RemotePairing, AppError> {
    let service = service(&app)?;
    if !service.status().enabled {
        return Err(AppError::invalid_argument(
            "Turn remote access on before pairing a device.",
        ));
    }
    service.pair().map_err(refused)
}

#[tauri::command]
#[specta::specta]
pub fn remote_cancel_pairing(app: tauri::AppHandle) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.cancel_pairing();
    Ok(service.status())
}

/// Removes a paired device. Its open sessions end and its key stops working.
#[tauri::command]
#[specta::specta]
pub fn remote_remove_device(app: tauri::AppHandle, id: String) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.remove_device(&id).map_err(refused)?;
    Ok(service.status())
}

/// Creates a bot link. Its address carries the bot's token and is returned only here. `level`
/// is `read`, `chats` or `full`; `approve` lets its bot answer what an agent asks permission
/// for, and `full_access` lets it turn on full access (full control only). Both are off unless
/// the person chose them.
#[tauri::command]
#[specta::specta]
pub fn remote_create_bot(
    app: tauri::AppHandle,
    name: String,
    level: String,
    approve: bool,
    full_access: bool,
) -> Result<RemoteBotLink, AppError> {
    let (full, read_only) = match level.as_str() {
        "read" => (false, true),
        "chats" => (false, false),
        "full" => (true, false),
        _ => return Err(AppError::invalid_argument("Choose what the bot can do.")),
    };
    service(&app)?
        .create_bot(
            &name,
            ghost_remote::bot::Rights {
                full,
                read_only,
                approve,
                full_access,
            },
        )
        .map_err(refused)
}

/// This program's path, for an assistant on this computer that starts Ghost's helper
/// (`--ghost-bot-helper`) to reach its door without the relay.
#[tauri::command]
#[specta::specta]
pub fn remote_local_program() -> Result<String, AppError> {
    std::env::current_exe()
        .map(|program| program.display().to_string())
        .map_err(|error| AppError::new(AppErrorCode::StorageUnavailable, error.to_string()))
}

/// Makes an automation: an address that starts a task in `project` (one of the open
/// workspace's) with `instruction`, from what is sent to it, which its agent is given as data.
/// The address is returned only here.
#[tauri::command]
#[specta::specta]
pub async fn remote_create_automation(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    name: String,
    project: String,
    instruction: String,
) -> Result<RemoteBotLink, AppError> {
    if instruction.trim().is_empty() {
        return Err(AppError::invalid_argument(
            "Say what its agent should do with what is sent.",
        ));
    }
    let service = service(&app)?;
    let snapshot = state
        .sessions()?
        .workspace_snapshot(WorkspaceFilter {
            include_archived: false,
            project_id: None,
        })
        .await?;
    if !snapshot.projects.iter().any(|known| known.id == project) {
        return Err(AppError::invalid_argument(
            "Choose one of this workspace's projects.",
        ));
    }
    service
        .create_automation(&name, &project, &instruction)
        .map_err(refused)
}

/// A new address for a bot link, shown once: the old one stops working at once, and a link that
/// expired unused works again. What the link may do stays.
#[tauri::command]
#[specta::specta]
pub fn remote_renew_bot(app: tauri::AppHandle, id: String) -> Result<RemoteBotLink, AppError> {
    service(&app)?
        .renew_bot(&id)
        .map_err(refused)?
        .ok_or_else(|| AppError::invalid_argument("That bot link is gone."))
}

/// Removes a bot link. Its address stops working, and the key of its call-back, if it had
/// one, leaves this computer's secure storage.
#[tauri::command]
#[specta::specta]
pub async fn remote_remove_bot(
    app: tauri::AppHandle,
    id: String,
) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.remove_bot(&id).map_err(refused)?;
    let _ = tokio::task::spawn_blocking(move || crate::remote_callback::remove_key(&id)).await;
    Ok(service.status())
}

fn callback_kind(kind: &str) -> Result<ghost_remote::CallbackKind, AppError> {
    match kind {
        "grok" => Ok(ghost_remote::CallbackKind::Grok),
        "hermes" => Ok(ghost_remote::CallbackKind::Hermes),
        "openclaw" => Ok(ghost_remote::CallbackKind::Openclaw),
        "webhook" => Ok(ghost_remote::CallbackKind::Webhook),
        _ => Err(AppError::invalid_argument(
            "Choose which assistant this address is.",
        )),
    }
}

fn stored(error: crate::agent_tokens::TokenError) -> AppError {
    match error {
        crate::agent_tokens::TokenError::Invalid => {
            AppError::invalid_argument("That key cannot be kept: it is empty or too long.")
        }
        other => AppError::new(AppErrorCode::StorageUnavailable, other.to_string()),
    }
}

/// One short field of a call-back: a line of ordinary text, or nothing.
fn short_field(value: Option<String>, what: &str) -> Result<Option<String>, AppError> {
    match value.map(|value| value.trim().to_owned()) {
        None => Ok(None),
        Some(value) if value.is_empty() => Ok(None),
        Some(value) if value.len() > 200 || value.chars().any(char::is_control) => Err(
            AppError::invalid_argument(format!("That {what} is not one short line.")),
        ),
        Some(value) => Ok(Some(value)),
    }
}

/// Sets where the bot link `id` is told that a chat it acted on finished or asks for an answer:
/// which assistant the address is, the address, and the key it takes. The key goes to this
/// computer's secure storage and is never returned; without a new one the kept one stays, so
/// an address can be corrected without typing the key again. Only the window calls this: a bot
/// has no tool that reads or changes its own link.
#[tauri::command]
#[specta::specta]
pub async fn remote_set_bot_callback(
    app: tauri::AppHandle,
    id: String,
    kind: String,
    url: String,
    key: Option<String>,
    channel: Option<String>,
    to: Option<String>,
) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    let kind = callback_kind(&kind)?;
    // Kept as the parser wrote it: what is sent to is exactly what was checked.
    let url = ghost_remote::callback::check_url(url.trim())
        .map_err(|refusal| AppError::invalid_argument(refusal.0))?;
    let (channel, to) = (
        short_field(channel, "channel")?,
        short_field(to, "recipient")?,
    );
    let (channel, to) = match (kind, channel, to) {
        (ghost_remote::CallbackKind::Openclaw, Some(channel), Some(to)) => {
            (Some(channel), Some(to))
        }
        (ghost_remote::CallbackKind::Openclaw, None, None) => (None, None),
        (ghost_remote::CallbackKind::Openclaw, ..) => {
            return Err(AppError::invalid_argument(
                "Give both the channel and who to send to, or neither.",
            ));
        }
        _ => (None, None),
    };
    if !service.status().bots.iter().any(|bot| bot.id == id) {
        return Err(AppError::invalid_argument("That bot link is gone."));
    }
    let key = key.filter(|key| !key.trim().is_empty());
    let bot = id.clone();
    let kept = tokio::task::spawn_blocking(move || match key {
        Some(key) => crate::remote_callback::save_key(&bot, &key).map(|()| true),
        None => Ok(crate::remote_callback::has_key(&bot)),
    })
    .await
    .map_err(|_| {
        AppError::new(
            AppErrorCode::StorageUnavailable,
            "Secure storage did not answer.",
        )
    })?
    .map_err(stored)?;
    if !kept {
        return Err(AppError::invalid_argument(
            "Give the key this address takes.",
        ));
    }
    service
        .set_bot_callback(
            &id,
            Some(ghost_remote::Callback {
                kind,
                url,
                channel,
                to,
            }),
        )
        .map_err(refused)?;
    Ok(service.status())
}

/// Stops telling the bot link `id` anything, and forgets its key.
#[tauri::command]
#[specta::specta]
pub async fn remote_clear_bot_callback(
    app: tauri::AppHandle,
    id: String,
) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.set_bot_callback(&id, None).map_err(refused)?;
    let _ = tokio::task::spawn_blocking(move || crate::remote_callback::remove_key(&id)).await;
    Ok(service.status())
}

/// Sends the bot link `id`'s address a test message now and answers with how it went.
#[tauri::command]
#[specta::specta]
pub async fn remote_test_bot_callback(
    app: tauri::AppHandle,
    id: String,
) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    let delivery = service
        .test_delivery(&id)
        .ok_or_else(|| AppError::invalid_argument("Set where to tell this bot first."))?;
    let told = crate::remote_callback::send(delivery).await;
    service.note_told(&id, told.ok, told.detail);
    Ok(service.status())
}

/// Sends Ghost for iPhone `id` a test notification now, through the relay and Apple, as its row's
/// "Send test notification" does: at most one every ten seconds, and only through a relay that
/// sends notifications. The status says when Apple last took one for it.
#[tauri::command]
#[specta::specta]
pub fn remote_test_notification(
    app: tauri::AppHandle,
    id: String,
) -> Result<RemoteStatus, AppError> {
    let service = service(&app)?;
    service.test_push(&id).map_err(AppError::invalid_argument)?;
    Ok(service.status())
}

/// One profile's chats, as remote sessions reach them. Every call takes the same lease a window's
/// command does, and takes it only while `profile` is the open profile, both read under one lock:
/// a session accepted for one profile is never served another's chats, wherever a switch lands.
/// The service binds a profile before every call; unbound (`profile` empty), nothing is served.
struct AppBackend {
    app: tauri::AppHandle,
    profile: String,
}

fn failed(error: AppError) -> BackendError {
    // The same codes the window gets, in the same words.
    let code = match error.code {
        AppErrorCode::ChatNotFound => "not_found",
        AppErrorCode::RequestNotPending => "request_not_pending",
        AppErrorCode::OptionInvalid => "option_invalid",
        AppErrorCode::PromptEmpty => "empty",
        AppErrorCode::PromptTooLarge => "too_large",
        AppErrorCode::QueueFull => "queue_full",
        AppErrorCode::UnknownAgent | AppErrorCode::AgentUnavailable => "agent_unavailable",
        AppErrorCode::InvalidFolder | AppErrorCode::InvalidArgument => "invalid",
        AppErrorCode::TooManyAgents
        | AppErrorCode::TooManySubscriptions
        | AppErrorCode::FolderBusy => "busy",
        AppErrorCode::StorageUnavailable | AppErrorCode::ShuttingDown => "unavailable",
        _ => "failed",
    };
    BackendError::new(code, error.message)
}

struct FrameAdapter {
    owner: String,
    frames: Frames,
}

impl FrameSink for FrameAdapter {
    fn send(&self, frame: &EventBatch) -> Result<(), SinkGone> {
        if (self.frames)(frame) {
            Ok(())
        } else {
            Err(SinkGone)
        }
    }

    fn owner(&self) -> &str {
        &self.owner
    }
}

struct StatusAdapter {
    owner: String,
    statuses: Statuses,
}

impl StatusSink for StatusAdapter {
    fn send(&self, update: &ChatStatusUpdate) -> Result<(), SinkGone> {
        if (self.statuses)(update) {
            Ok(())
        } else {
            Err(SinkGone)
        }
    }

    fn owner(&self) -> &str {
        &self.owner
    }
}

impl AppBackend {
    fn state(&self) -> State<'_, AppState> {
        self.app.state::<AppState>()
    }

    /// The bound profile's session manager, under a command lease.
    fn sessions(
        &self,
    ) -> Result<crate::state::ProfileLease<ghost_core::session::SessionManager>, AppError> {
        self.state().sessions_of(&self.profile)
    }

    /// The agent a new chat starts with: the one asked for, else the project's, else the one
    /// Settings names, else the first that is ready.
    async fn agent_for(&self, folder_id: &str, asked: Option<&str>) -> Result<String, AppError> {
        if let Some(agent) = asked {
            return Ok(agent.to_owned());
        }
        let sessions = self.sessions()?;
        let snapshot = sessions
            .workspace_snapshot(WorkspaceFilter {
                include_archived: false,
                project_id: None,
            })
            .await?;
        let project = snapshot
            .projects
            .iter()
            .find(|project| project.folders.iter().any(|folder| folder.id == folder_id))
            .ok_or_else(|| AppError::invalid_argument("That folder is no longer here."))?;
        if let Some(agent) = &project.default_agent {
            return Ok(agent.clone());
        }
        if let Some(agent) = default_agent(&self.state()) {
            return Ok(agent);
        }
        sessions
            .agents(false)
            .await
            .into_iter()
            .find(|agent| matches!(agent.availability, Availability::Ready { .. }))
            .map(|agent| agent.id)
            .ok_or_else(|| {
                AppError::new(
                    AppErrorCode::AgentUnavailable,
                    "No agent is ready on this computer.",
                )
            })
    }
}

/// The open profile's theme and appearance, by their saved names, for a paired page to wear.
fn look(state: &AppState) -> Option<ghost_remote::Look> {
    let preferences = state.setup_service()?.preferences().ok()?;
    let name = |value: serde_json::Value| value.as_str().map(str::to_owned);
    Some(ghost_remote::Look {
        theme: name(serde_json::to_value(preferences.theme).ok()?)?,
        appearance: name(serde_json::to_value(preferences.appearance).ok()?)?,
    })
}

fn default_agent(state: &AppState) -> Option<String> {
    state
        .setup_service()
        .and_then(|setup| setup.preferences().ok())
        .and_then(|preferences| preferences.general.default_agent)
}

/// Whether `path` names a folder by a full path on this computer's own drives. A network share
/// in any spelling (`\\server\share`, `//server/share`, mixed slashes, the `\\?\UNC\` and device
/// forms) is not: opening one makes Windows sign in to that machine. The path is parsed as the
/// system parses it, after its separators are made one kind, so no spelling slips past.
///
/// A drive letter mapped to a network share is the person's own setup and passes: it reaches
/// nothing they have not already connected.
fn on_this_computer(path: &str) -> bool {
    use std::path::{Component, Path, Prefix};
    if cfg!(windows) {
        let unified = path.replace('/', "\\");
        if unified.starts_with("\\\\") {
            return false;
        }
        let mut parts = Path::new(&unified).components();
        matches!(
            (parts.next(), parts.next()),
            (Some(Component::Prefix(prefix)), Some(Component::RootDir))
                if matches!(prefix.kind(), Prefix::Disk(_))
        )
    } else {
        !path.starts_with("//") && Path::new(path).is_absolute()
    }
}

/// The words a bot uses for the three access levels.
fn access_word(autonomy: ghost_protocol::Autonomy) -> &'static str {
    match autonomy {
        ghost_protocol::Autonomy::Ask => "ask",
        ghost_protocol::Autonomy::AutoEdit => "auto_edit",
        ghost_protocol::Autonomy::FullAuto => "full_auto",
    }
}

/// A chat's model and thinking choices as its pickers show them, and its access levels.
fn chat_options(lists: &ghost_protocol::session::SessionLists) -> ghost_remote::ChatOptions {
    use ghost_protocol::session::{ConfigCategory, ConfigKind};
    use ghost_remote::Choice;
    let select = |category: ConfigCategory| {
        lists
            .options
            .iter()
            .filter(|option| !matches!(option.id.as_str(), "aura.access" | "aura.workflow"))
            .find(|option| option.category == category)
            .and_then(|option| match &option.kind {
                ConfigKind::Select { current, choices } => Some((
                    choices
                        .iter()
                        .map(|choice| Choice {
                            value: choice.value.clone(),
                            name: choice.name.clone(),
                        })
                        .collect::<Vec<_>>(),
                    Some(current.clone()),
                )),
                ConfigKind::Toggle { .. } => None,
            })
            .unwrap_or_default()
    };
    let (models, model) = select(ConfigCategory::Model);
    let (thinking, thinking_level) = select(ConfigCategory::Effort);
    let native = &lists.controls.access.choices;
    let access = if native.is_empty() {
        [
            ghost_protocol::Autonomy::Ask,
            ghost_protocol::Autonomy::AutoEdit,
            ghost_protocol::Autonomy::FullAuto,
        ]
        .into_iter()
        .map(|level| Choice {
            value: access_word(level).into(),
            name: access_word(level).replace('_', " "),
        })
        .collect()
    } else {
        native
            .iter()
            .filter_map(|choice| {
                choice.legacy_autonomy.map(|level| Choice {
                    value: access_word(level).into(),
                    name: choice.name.clone(),
                })
            })
            .collect()
    };
    ghost_remote::ChatOptions {
        models,
        model,
        thinking,
        thinking_level,
        access,
        access_mode: Some(access_word(lists.autonomy.requested).into()),
    }
}

impl Backend for AppBackend {
    fn bound(self: Arc<Self>, profile: &str) -> Arc<dyn Backend> {
        Arc::new(Self {
            app: self.app.clone(),
            profile: profile.to_owned(),
        })
    }

    fn look(&self) -> Option<ghost_remote::Look> {
        look(&self.state())
    }

    /// A full-control link's actions, each the call the window itself makes, under the same
    /// lease and only while this backend's profile is the open one.
    fn act(
        &self,
        action: ghost_remote::Action,
    ) -> BoxFuture<'_, Result<ghost_remote::Outcome, BackendError>> {
        use ghost_protocol::session::{ConfigCategory, ConfigValue, SessionControlChange};
        use ghost_remote::{Action, Outcome};
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            // A model or a thinking level, by the value its picker would send.
            let choose = |chat_id: String,
                          category: ConfigCategory,
                          what: &'static str,
                          value: String| {
                let sessions = &sessions;
                async move {
                    let lists = sessions
                        .lists(&chat_id)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    let offered = chat_options(&lists);
                    let choices = if category == ConfigCategory::Model {
                        &offered.models
                    } else {
                        &offered.thinking
                    };
                    if !choices.iter().any(|choice| choice.value == value) {
                        return Err(BackendError::new(
                            "invalid",
                            format!(
                                "This chat offers no such {what}. Read the chat's options for the list."
                            ),
                        ));
                    }
                    let option = lists
                        .options
                        .iter()
                        .filter(|option| {
                            !matches!(option.id.as_str(), "aura.access" | "aura.workflow")
                        })
                        .find(|option| option.category == category)
                        .map(|option| option.id.clone())
                        .ok_or_else(|| {
                            BackendError::new(
                                "invalid",
                                format!("This agent has no {what} to choose."),
                            )
                        })?;
                    sessions
                        .set_option(
                            &chat_id,
                            &option,
                            ConfigValue::Choice {
                                value: value.clone(),
                            },
                        )
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Done(format!("The {what} is now {value}.")))
                }
            };
            match action {
                Action::CreateProject { path, name } => {
                    // A folder on this computer, by its full path: not a network share, whose
                    // name alone makes Windows reach out to another machine.
                    if !on_this_computer(&path) {
                        return Err(BackendError::new(
                            "invalid",
                            "Give a folder on the computer itself, not a network path.",
                        ));
                    }
                    let name = name.unwrap_or_else(|| {
                        std::path::Path::new(&path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| "Project".into())
                    });
                    let project = sessions
                        .project_create(ghost_protocol::workspace::ProjectCreate {
                            project: ghost_protocol::workspace::ProjectInput {
                                name,
                                color: None,
                                default_agent: None,
                                default_model: None,
                            },
                            folder: path,
                        })
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Project {
                        folder_id: project.folders.first().map(|folder| folder.id.clone()),
                        id: project.id,
                        name: project.name,
                    })
                }
                Action::Options { chat_id } => Ok(Outcome::Options(chat_options(
                    &sessions
                        .lists(&chat_id)
                        .await
                        .map_err(|error| failed(error.into()))?,
                ))),
                Action::SetModel { chat_id, model } => {
                    choose(chat_id, ConfigCategory::Model, "model", model).await
                }
                Action::SetThinking { chat_id, level } => {
                    choose(chat_id, ConfigCategory::Effort, "thinking level", level).await
                }
                Action::SetAccess { chat_id, access } => {
                    let wanted = match access.as_str() {
                        "ask" => ghost_protocol::Autonomy::Ask,
                        "auto_edit" => ghost_protocol::Autonomy::AutoEdit,
                        "full_auto" => ghost_protocol::Autonomy::FullAuto,
                        _ => return Err(BackendError::new("invalid", "No such access level.")),
                    };
                    let lists = sessions
                        .lists(&chat_id)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    let native = &lists.controls.access.choices;
                    // As the window's picker does: the agent's own level where it has them.
                    if native.is_empty() {
                        sessions
                            .set_autonomy(&chat_id, wanted)
                            .await
                            .map_err(|error| failed(error.into()))?;
                    } else {
                        let choice = native
                            .iter()
                            .find(|choice| choice.legacy_autonomy == Some(wanted))
                            .ok_or_else(|| {
                                BackendError::new(
                                    "invalid",
                                    "This agent does not offer that access level. Read the chat's options for its levels.",
                                )
                            })?;
                        sessions
                            .set_control(
                                &chat_id,
                                SessionControlChange::Access {
                                    value: choice.value.clone(),
                                },
                            )
                            .await
                            .map_err(|error| failed(error.into()))?;
                    }
                    Ok(Outcome::Done(format!("Access is now {access}.")))
                }
                Action::SwitchAgent { chat_id, agent_id } => {
                    // As the window does: the agent stops, then the app's own handoff is prepared
                    // and applied. A chat that is already stopped answers the stop with an error
                    // that does not matter here; the handoff itself refuses a chat still working.
                    let _ = sessions.stop(&chat_id).await;
                    let preview = sessions
                        .continuation_preview(
                            &chat_id,
                            ghost_protocol::conversation::ContinuationAction::SwitchAgent {
                                agent_id,
                            },
                        )
                        .await
                        .map_err(|error| failed(error.into()))?;
                    let result = sessions
                        .continuation_apply(&chat_id, &preview.token)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Chat { id: result.chat_id })
                }
                Action::Rename { chat_id, title } => {
                    sessions
                        .chat_rename(&chat_id, &title)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Done("Renamed.".into()))
                }
                Action::Archive { chat_id, archived } => {
                    sessions
                        .chat_archive(&chat_id, archived)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Done(
                        if archived {
                            "Archived."
                        } else {
                            "Restored from the archive."
                        }
                        .into(),
                    ))
                }
                Action::StartAgent { chat_id } => {
                    sessions
                        .start(&chat_id, false)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Done(
                        "The agent is starting. Wait for the chat, or send it a message.".into(),
                    ))
                }
                Action::Changes { chat_id } => {
                    let changes = sessions
                        .chat_changes(&chat_id, None)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Changes {
                        files: changes
                            .files
                            .iter()
                            .map(|file| (file.path.clone(), change_word(file.kind).to_owned()))
                            .collect(),
                        unavailable: changes.unavailable_reason,
                    })
                }
                Action::Diff { chat_id, path } => {
                    let changes = sessions
                        .chat_changes(&chat_id, None)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    let checkpoint = changes.checkpoint.ok_or_else(|| {
                        BackendError::new("not_found", "This chat has no saved changes to read.")
                    })?;
                    // Only a file the last turn changed: the path is looked up among them, never
                    // opened from the disk by name.
                    if !changes.files.iter().any(|file| file.path == path) {
                        return Err(BackendError::new(
                            "not_found",
                            "The last turn did not change that file. List the changes for its files.",
                        ));
                    }
                    let diff = sessions
                        .chat_change_diff(&chat_id, &checkpoint.id, &path)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Diff {
                        before: diff.before,
                        after: diff.after,
                        binary: diff.binary,
                        truncated: diff.truncated,
                    })
                }
                Action::ReadMemory { project_id } => {
                    let memory = sessions
                        .read_project_memory(&project_id)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Memory {
                        markdown: memory.markdown,
                        revision: memory.revision,
                    })
                }
                Action::SaveMemory {
                    project_id,
                    markdown,
                    base_revision,
                    reason,
                } => {
                    // The person's own edit from a paired device: saved as theirs, as the
                    // window's editor saves, whatever the switch for agents' edits says.
                    let conflict = || {
                        BackendError::new(
                            "conflict",
                            "The brief changed on the computer since it was read. Read it again and make the edit there.",
                        )
                    };
                    let current = sessions
                        .read_project_memory(&project_id)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    // `Some(None)`: the device read no brief, so it saves only while there is
                    // none yet.
                    if base_revision.is_some_and(|base| base != current.revision) {
                        return Err(conflict());
                    }
                    let saved = sessions
                        .save_project_memory(ghost_protocol::memory::SaveProjectMemory {
                            project_id: project_id.clone(),
                            expected_revision: current.revision.clone(),
                            markdown,
                            reason,
                        })
                        .await;
                    match saved {
                        Ok(saved) => Ok(Outcome::Memory {
                            markdown: saved.markdown,
                            revision: saved.revision,
                        }),
                        Err(error) => {
                            // Saved over in the moment between the reading and the save.
                            let moved = sessions
                                .read_project_memory(&project_id)
                                .await
                                .is_ok_and(|now| now.revision != current.revision);
                            Err(if moved {
                                conflict()
                            } else {
                                failed(error.into())
                            })
                        }
                    }
                }
                Action::WriteMemory {
                    project_id,
                    markdown,
                } => {
                    // Memory reaches every later chat of the project: the person's own switch
                    // for agents' edits covers a bot's as well.
                    if !sessions.agent_memory_writes() {
                        return Err(BackendError::new(
                            "refused",
                            "The owner has turned off edits to project memory. Tell them what you would have saved.",
                        ));
                    }
                    let current = sessions
                        .read_project_memory(&project_id)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    // Saved as a bot's, not as the person's: the window shows the edit and
                    // offers to undo it, as it does for an agent's.
                    sessions
                        .bot_save_project_memory(ghost_protocol::memory::SaveProjectMemory {
                            project_id,
                            expected_revision: current.revision,
                            markdown,
                            reason: "Edited by a linked bot".into(),
                        })
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(Outcome::Done("Saved the project's memory.".into()))
                }
            }
        })
    }

    fn overview(&self) -> BoxFuture<'_, Result<Overview, BackendError>> {
        Box::pin(async move {
            let state = self.state();
            let sessions = self.sessions().map_err(failed)?;
            let snapshot = sessions
                .workspace_snapshot(WorkspaceFilter {
                    include_archived: false,
                    project_id: None,
                })
                .await
                .map_err(|error| failed(error.into()))?;
            let agents = sessions
                .agents(false)
                .await
                .into_iter()
                .map(|agent| RemoteAgent {
                    available: matches!(agent.availability, Availability::Ready { .. }),
                    id: agent.id,
                    name: agent.name,
                })
                .collect();
            Ok(Overview {
                host_name: state
                    .app_info()
                    .device
                    .map(|device| device.name)
                    .unwrap_or_default(),
                projects: snapshot.projects,
                chats: snapshot.chats,
                agents,
                default_agent: default_agent(&state),
            })
        })
    }

    fn watch(&self, owner: &str, statuses: Statuses) -> Result<(), BackendError> {
        // No lease: the subscription belongs to the manager, and ends with it.
        let chats = self.state().chats_of(&self.profile).map_err(failed)?;
        chats.manager.status_subscribe(Arc::new(StatusAdapter {
            owner: owner.to_owned(),
            statuses,
        }));
        Ok(())
    }

    fn attach<'a>(
        &'a self,
        chat_id: &'a str,
        owner: &'a str,
        frames: Frames,
    ) -> BoxFuture<'a, Result<ChatAttach, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            // One chat per browser: what it followed before is let go first.
            sessions.drop_owner(owner);
            sessions
                .attach(
                    chat_id,
                    Arc::new(FrameAdapter {
                        owner: owner.to_owned(),
                        frames,
                    }),
                )
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn release(&self, owner: &str) {
        if let Some(chats) = self.state().chats() {
            chats.manager.drop_owner(owner);
        }
    }

    fn history<'a>(
        &'a self,
        chat_id: &'a str,
        before_seq: u32,
    ) -> BoxFuture<'a, Result<HistoryPage, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .history(chat_id, before_seq, Some(PAGE_BYTES))
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn send<'a>(
        &'a self,
        chat_id: &'a str,
        text: String,
    ) -> BoxFuture<'a, Result<PromptAck, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .send(chat_id, text)
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn interrupt<'a>(&'a self, chat_id: &'a str) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .interrupt(chat_id)
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn permission<'a>(
        &'a self,
        chat_id: &'a str,
        request_id: &'a str,
        option_id: Option<String>,
    ) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .respond_permission(chat_id, request_id, option_id)
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn answer<'a>(
        &'a self,
        chat_id: &'a str,
        request_id: &'a str,
        answer: Option<UiAnswer>,
    ) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .respond_ui(chat_id, request_id, answer)
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    fn create<'a>(
        &'a self,
        folder_id: &'a str,
        agent_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<WorkspaceChat, BackendError>> {
        Box::pin(async move {
            let agent_id = self.agent_for(folder_id, agent_id).await.map_err(failed)?;
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .chat_project_create(ProjectChatCreate {
                    agent_id,
                    folder_id: folder_id.to_owned(),
                    // Asked for from afar, there is nobody to show the busy-folder warning to.
                    allow_busy_folder: true,
                })
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    /// A task's chat in a new worktree of the folder, as the window's "new chat in a worktree"
    /// makes it, without the saved setup (it would wait for a terminal nobody watches): the agent
    /// is told the checkout is fresh. A folder that is not a repository gets an ordinary chat.
    fn create_task_chat<'a>(
        &'a self,
        folder_id: &'a str,
        agent_id: Option<&'a str>,
        title: &'a str,
    ) -> BoxFuture<'a, Result<ghost_remote::TaskChat, BackendError>> {
        use ghost_protocol::git::{WorktreeChatInput, WorktreeChatPreview};
        Box::pin(async move {
            let agent_id = self.agent_for(folder_id, agent_id).await.map_err(failed)?;
            let sessions = self.sessions().map_err(failed)?;
            let preview = sessions
                .worktree_chat_preview(WorktreeChatInput {
                    folder_id: folder_id.to_owned(),
                    agent_id: agent_id.clone(),
                    start_branch: None,
                    branch: None,
                    title: Some(title.to_owned()),
                    bring_changes: false,
                    run_setup: false,
                })
                .await
                .map_err(|error| failed(error.into()))?;
            match preview {
                WorktreeChatPreview::Worktree { plan } => {
                    let made = sessions
                        .worktree_chat_apply(&plan.token)
                        .await
                        .map_err(|error| failed(error.into()))?;
                    Ok(ghost_remote::TaskChat {
                        chat: made.chat,
                        branch: Some(made.worktree.branch),
                        path: Some(made.worktree.path),
                        base: Some(made.worktree.base_commit),
                    })
                }
                WorktreeChatPreview::NotGitRepository { .. } => Ok(ghost_remote::TaskChat {
                    chat: sessions
                        .chat_project_create(ProjectChatCreate {
                            agent_id,
                            folder_id: folder_id.to_owned(),
                            allow_busy_folder: true,
                        })
                        .await
                        .map_err(|error| failed(error.into()))?,
                    branch: None,
                    path: None,
                    base: None,
                }),
            }
        })
    }

    /// A chat's branch and what is not committed in its folder, and its agent's usage as last
    /// reported. A part that cannot be read is left out, never guessed.
    fn local_hour(&self) -> Option<u8> {
        local_hour()
    }

    /// The pull request open from the chat's branch, read with GitHub CLI as the person is
    /// signed in, or as it said within [`PR_FRESH`].
    fn pull_request<'a>(
        &'a self,
        chat_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ghost_remote::PullRequest>, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            pull_request_of(&sessions, chat_id).await.map_err(failed)
        })
    }

    /// What GitHub CLI said of the chat's pull request in the last day, only while this
    /// backend's profile is the open one. It asks GitHub nothing.
    fn known_pull_request(&self, chat_id: &str) -> Option<ghost_remote::PullRequest> {
        drop(self.sessions().ok()?);
        known_pull_request(chat_id)
    }

    /// The chat's browser tab or emulator as a JPEG, only while this backend's profile is the
    /// open one. The chat is one of its own: the tool found it in the overview.
    fn picture<'a>(
        &'a self,
        chat_id: &'a str,
        of: ghost_remote::PictureOf,
    ) -> BoxFuture<'a, Result<ghost_remote::Picture, BackendError>> {
        use base64::Engine as _;
        Box::pin(async move {
            drop(self.sessions().map_err(failed)?);
            let taken = match of {
                ghost_remote::PictureOf::Browser => {
                    crate::browser::picture(&self.app, chat_id).await
                }
                ghost_remote::PictureOf::Device => crate::mobile::picture(&self.app, chat_id).await,
            }
            .map_err(|message| BackendError::new("unavailable", message))?;
            let jpeg = tokio::task::spawn_blocking(move || shrink(&taken))
                .await
                .map_err(|error| BackendError::new("failed", error.to_string()))?
                .map_err(|message| BackendError::new("failed", message))?;
            let data = base64::engine::general_purpose::STANDARD.encode(jpeg);
            if data.len() > PICTURE_BYTES {
                return Err(BackendError::new(
                    "too_large",
                    "The picture is too large to send.",
                ));
            }
            Ok(ghost_remote::Picture {
                mime: "image/jpeg".into(),
                data,
            })
        })
    }

    fn facts<'a>(
        &'a self,
        chat_id: &'a str,
        agent_id: &'a str,
    ) -> BoxFuture<'a, Result<ghost_remote::ChatFacts, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            let git = sessions.git_workspace(chat_id).await.ok();
            let worktree = sessions
                .managed_worktrees(chat_id)
                .await
                .is_ok_and(|worktrees| worktrees.iter().any(|tree| tree.chat_id == chat_id));
            let usage = sessions
                .agent_usage(agent_id)
                .accounts
                .first()
                .map(|account| {
                    account
                        .windows
                        .iter()
                        .map(|window| (window.label.clone(), window.used_percent))
                        .collect()
                })
                .unwrap_or_default();
            // A chat on a branch of its own may have a pull request; the main branch has none.
            let branch = git.as_ref().and_then(|git| git.branch.clone());
            let own = branch
                .as_deref()
                .is_some_and(|branch| !matches!(branch, "main" | "master" | "trunk" | "develop"));
            let pull_request = if own {
                pull_request_of(&sessions, chat_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|pr| ghost_remote::render::pull_request(&pr))
            } else {
                None
            };
            Ok(ghost_remote::ChatFacts {
                pull_request,
                branch,
                worktree,
                uncommitted: git.as_ref().map(|git| git.files.len()),
                usage,
            })
        })
    }

    /// The profile's archived chats, as the window's archive lists them.
    fn archived(&self) -> BoxFuture<'_, Result<Vec<WorkspaceChat>, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            let snapshot = sessions
                .workspace_snapshot(WorkspaceFilter {
                    include_archived: true,
                    project_id: None,
                })
                .await
                .map_err(|error| failed(error.into()))?;
            Ok(snapshot
                .chats
                .into_iter()
                .filter(|entry| entry.archived)
                .collect())
        })
    }

    /// The chat's browser tab or emulator as a JPEG, made smaller step by step until its base64
    /// is at most `max_bytes` (a phone's answer is smaller than a bot's picture may be).
    fn picture_within<'a>(
        &'a self,
        chat_id: &'a str,
        of: ghost_remote::PictureOf,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<ghost_remote::Picture, BackendError>> {
        use base64::Engine as _;
        Box::pin(async move {
            drop(self.sessions().map_err(failed)?);
            let taken = match of {
                ghost_remote::PictureOf::Browser => {
                    crate::browser::picture(&self.app, chat_id).await
                }
                ghost_remote::PictureOf::Device => crate::mobile::picture(&self.app, chat_id).await,
            }
            .map_err(|message| BackendError::new("unavailable", message))?;
            let jpeg = tokio::task::spawn_blocking(move || {
                let picture = image::load_from_memory(&taken)
                    .map_err(|error| format!("The picture could not be read: {error}"))?;
                jpeg_within(&picture, max_bytes)
            })
            .await
            .map_err(|error| BackendError::new("failed", error.to_string()))?
            .map_err(|message| BackendError::new("failed", message))?
            .ok_or_else(|| {
                BackendError::new(
                    "too_large",
                    "The picture is too large to send to this device.",
                )
            })?;
            Ok(ghost_remote::Picture {
                mime: "image/jpeg".into(),
                data: base64::engine::general_purpose::STANDARD.encode(jpeg.0),
            })
        })
    }

    /// A photo from a paired device as the chat's message box keeps it (`prepare_photo`): a
    /// JPEG stays a JPEG and anything else becomes a PNG, made again with nothing but its
    /// pixels, turned upright, and under 4 MiB.
    fn prepare_image<'a>(
        &'a self,
        bytes: Vec<u8>,
        name: &'a str,
        _mime: &'a str,
    ) -> BoxFuture<'a, Result<ghost_remote::PreparedImage, BackendError>> {
        Box::pin(async move {
            let name = name.to_owned();
            let photo = tokio::task::spawn_blocking(move || {
                ghost_core::session::prepare_photo(&bytes, &name)
            })
            .await
            .map_err(|error| BackendError::new("failed", error.to_string()))?
            .map_err(|error| failed(error.into()))?;
            Ok(ghost_remote::PreparedImage {
                bytes: photo.bytes,
                name: photo.name,
                mime: photo.mime.into(),
            })
        })
    }

    /// A prepared photo into the chat's message box (`composer_photo`), kept with the chat. One
    /// that is not what `prepare_photo` makes is prepared there first.
    fn attach_image<'a>(
        &'a self,
        chat_id: &'a str,
        bytes: Vec<u8>,
        name: &'a str,
        _mime: &'a str,
    ) -> BoxFuture<'a, Result<String, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            let attached = sessions
                .composer_photo(chat_id, bytes, name.to_owned())
                .await
                .map_err(|error| failed(error.into()))?;
            Ok(attached.id)
        })
    }

    /// The message with its attachments, as the window's message box sends it.
    fn send_input<'a>(
        &'a self,
        chat_id: &'a str,
        text: String,
        attachments: Vec<String>,
    ) -> BoxFuture<'a, Result<PromptAck, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            sessions
                .composer_send(
                    chat_id,
                    ghost_protocol::composer::ComposerInput {
                        text,
                        attachments,
                        mentions: Vec::new(),
                    },
                )
                .await
                .map_err(|error| failed(error.into()))
        })
    }

    /// An image the chat holds, as the window's preview reads it (a PNG), or as a JPEG made
    /// smaller step by step when the PNG would not fit `max_bytes` of base64. Never a project file
    /// by its path: the service refuses that source, and so does this.
    fn image<'a>(
        &'a self,
        chat_id: &'a str,
        source: ghost_protocol::composer::ChatImageSource,
        full: bool,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<ghost_remote::ImagePreview, BackendError>> {
        use base64::Engine as _;
        Box::pin(async move {
            if matches!(
                source,
                ghost_protocol::composer::ChatImageSource::Project { .. }
            ) {
                return Err(BackendError::new(
                    "invalid",
                    "Images are read from the chat's own messages and attachments only.",
                ));
            }
            let sessions = self.sessions().map_err(failed)?;
            let preview = sessions
                .chat_image_preview(chat_id, source, full)
                .await
                .map_err(|error| failed(error.into()))?;
            if preview.png_base64.len() <= max_bytes {
                return Ok(ghost_remote::ImagePreview {
                    mime: "image/png".into(),
                    data: preview.png_base64,
                    width: preview.width,
                    height: preview.height,
                });
            }
            let (jpeg, width, height) = tokio::task::spawn_blocking(move || {
                let png = base64::engine::general_purpose::STANDARD
                    .decode(preview.png_base64)
                    .map_err(|_| "The image could not be read.".to_owned())?;
                let picture = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                    .map_err(|error| format!("The image could not be read: {error}"))?;
                jpeg_within(&picture, max_bytes)
            })
            .await
            .map_err(|error| BackendError::new("failed", error.to_string()))?
            .map_err(|message| BackendError::new("failed", message))?
            .ok_or_else(|| {
                BackendError::new(
                    "too_large",
                    "The image is too large to send to this device.",
                )
            })?;
            Ok(ghost_remote::ImagePreview {
                mime: "image/jpeg".into(),
                data: base64::engine::general_purpose::STANDARD.encode(jpeg),
                width,
                height,
            })
        })
    }

    /// What the chat's own branch changed against its base, committed or not, read with the
    /// app's own bounded Git: its task's base when the service names one, else its worktree's,
    /// else where its branch left the folder's default branch. A chat on no branch of its own,
    /// or one Git cannot read, says why.
    fn branch_changes<'a>(
        &'a self,
        chat_id: &'a str,
        base: Option<&'a str>,
        max_files: usize,
    ) -> BoxFuture<'a, Result<ghost_remote::BranchChanges, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            match sessions.chat_branch_changes(chat_id, base, max_files).await {
                Ok(Some(changes)) => Ok(ghost_remote::BranchChanges {
                    files: changes
                        .files
                        .into_iter()
                        .map(|file| (file.path, change_word(file.kind).to_owned()))
                        .collect(),
                    base: Some(changes.base),
                    unavailable: None,
                }),
                Ok(None) => Ok(ghost_remote::BranchChanges {
                    unavailable: Some(
                        "This chat has no branch of its own: it works in the project folder."
                            .into(),
                    ),
                    ..ghost_remote::BranchChanges::default()
                }),
                Err(error) => {
                    let error = failed(error.into());
                    if error.code == "not_found" {
                        return Err(error);
                    }
                    Ok(ghost_remote::BranchChanges {
                        unavailable: Some(error.message),
                        ..ghost_remote::BranchChanges::default()
                    })
                }
            }
        })
    }

    /// One file of the chat's branch changes, before (at the base) and after (in its folder),
    /// only when it is among them: nothing else is read.
    fn branch_diff<'a>(
        &'a self,
        chat_id: &'a str,
        base: Option<&'a str>,
        path: &'a str,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<ghost_remote::Diff, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            let diff = sessions
                .chat_branch_diff(chat_id, base, path, max_bytes)
                .await
                .map_err(|error| failed(error.into()))?
                .ok_or_else(|| {
                    BackendError::new("not_found", "This chat's branch did not change that file.")
                })?;
            Ok(ghost_remote::Diff {
                before: diff.before,
                after: diff.after,
                binary: diff.binary,
                truncated: diff.truncated,
            })
        })
    }

    /// The window's own search of the profile's chats (`workspace_search`), archived chats left
    /// out, under the same lease and only while this backend's profile is the open one.
    fn search<'a>(
        &'a self,
        query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<ChatSearchHit>, BackendError>> {
        Box::pin(async move {
            let sessions = self.sessions().map_err(failed)?;
            let results = sessions
                .search(query.to_owned(), false)
                .await
                .map_err(|error| failed(error.into()))?;
            Ok(results.chats)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::on_this_computer;

    /// What GitHub CLI said of a chat's pull request is kept for the iPhone app's Inbox, and read
    /// back without a new look; a chat it never looked at has none, and neither has one whose
    /// branch had no pull request.
    #[test]
    fn a_pull_request_once_read_is_known_without_asking_again() {
        let said = ghost_remote::PullRequest {
            number: 12,
            url: "https://github.com/o/r/pull/12".into(),
            state: "open".into(),
            checks: ghost_remote::PrChecks::Passed,
            notes: vec![],
        };
        super::keep_pull_request("known-chat", Some(said.clone()));
        super::keep_pull_request("no-pull-request", None);
        assert_eq!(super::known_pull_request("known-chat"), Some(said));
        assert_eq!(super::known_pull_request("no-pull-request"), None);
        assert_eq!(super::known_pull_request("never-read"), None);
    }

    /// The clock's hour is one of the day's.
    #[test]
    fn the_computers_hour_is_read() {
        let hour = super::local_hour().expect("an hour");
        assert!(hour < 24);
    }

    /// A bot is shown a JPEG whose longer side is at most 1600 pixels; a smaller picture keeps
    /// its size, and bytes that are no picture are refused.
    #[test]
    fn a_picture_for_a_bot_is_a_jpeg_no_larger_than_its_side() {
        let encoded = |width: u32, height: u32| {
            let mut png = Vec::new();
            image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 30]))
                .write_with_encoder(image::codecs::png::PngEncoder::new(&mut png))
                .expect("png");
            png
        };
        let size = |jpeg: &[u8]| {
            let back = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
                .expect("a jpeg");
            (back.width(), back.height())
        };
        assert_eq!(
            size(&super::shrink(&encoded(3200, 1000)).expect("wide")),
            (1600, 500)
        );
        assert_eq!(
            size(&super::shrink(&encoded(1080, 2400)).expect("tall")),
            (720, 1600)
        );
        assert_eq!(
            size(&super::shrink(&encoded(400, 300)).expect("small")),
            (400, 300)
        );
        assert!(super::shrink(b"not a picture").is_err());
    }

    /// A picture for a paired device steps down in size until its base64 fits the answer, lays
    /// transparency on white, and is refused (none) when even its smallest step does not fit.
    #[test]
    fn a_picture_for_the_phone_steps_down_until_it_fits() {
        let mut seed = 7u32;
        let noise = image::RgbImage::from_fn(3000, 2000, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let [a, b, c, _] = seed.to_le_bytes();
            image::Rgb([a, b, c])
        });
        let picture = image::DynamicImage::ImageRgb8(noise);
        let budget = 420 * 1024 - 1024;
        let (jpeg, width, height) = super::jpeg_within(&picture, budget)
            .expect("made")
            .expect("fits");
        assert!(super::base64_len(jpeg.len()) <= budget);
        assert!(width.max(height) <= 1600, "{width}x{height}");
        assert!(
            width.max(height) < 1600,
            "noise does not fit at the first step"
        );
        let back =
            image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).expect("a jpeg");
        assert_eq!((back.width(), back.height()), (width, height));
        assert!(super::jpeg_within(&picture, 100).expect("made").is_none());
        // A small picture keeps its size; its clear pixels come out white.
        let mut clear = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]));
        clear.put_pixel(0, 0, image::Rgba([200, 0, 0, 255]));
        let flat = super::on_white(&image::DynamicImage::ImageRgba8(clear));
        assert_eq!(flat.get_pixel(3, 3).0, [255, 255, 255]);
        assert_eq!(flat.get_pixel(0, 0).0, [200, 0, 0]);
        let (_, width, height) = super::jpeg_within(&image::DynamicImage::ImageRgb8(flat), budget)
            .expect("made")
            .expect("fits");
        assert_eq!((width, height), (4, 4));
    }

    #[test]
    fn a_project_folder_is_a_full_path_on_this_computers_own_drives() {
        #[cfg(windows)]
        {
            for local in [
                r"C:\work\site",
                "C:/work/site",
                r"d:\",
                r"C:\work/mixed\path",
            ] {
                assert!(on_this_computer(local), "{local}");
            }
            // A network share, however it is spelled, and the forms that reach one.
            for remote in [
                r"\server\share",
                "//server/share",
                r"\/server\share",
                r"/\server/share",
                r"\?\UNC\server\share",
                r"\.\pipe\name",
                r"\?\C:\work",
                "//?/UNC/server/share",
            ] {
                assert!(!on_this_computer(remote), "{remote}");
            }
            // Not a full path: relative, drive-relative, or rooted with no drive.
            for partial in ["work", r"C:work", r"\work", "/work", "", "C:"] {
                assert!(!on_this_computer(partial), "{partial}");
            }
        }
        #[cfg(not(windows))]
        {
            assert!(on_this_computer("/home/someone/site"));
            for other in ["//server/share", "site", ""] {
                assert!(!on_this_computer(other), "{other}");
            }
        }
    }
}
