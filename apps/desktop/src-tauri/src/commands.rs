//! The typed commands the UI calls. Each is a thin wrapper: the logic lives in the modules it calls,
//! where it is tested without a running app.
//!
//! The TypeScript side (`apps/desktop/src/bindings.ts`) is generated from these signatures; see
//! [`crate::bindings`].
//!
//! The chat commands wrap o7-core's `SessionManager` ([`AppState::sessions`]); every
//! `SessionError` reaches the UI as one `AppErrorCode` with the error's own sentence. Commands that
//! belong to a webview (a chat view, the status channel) take the injected `Webview`, whose label
//! owns the subscription: a reloaded page drops every subscription of its webview.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use o7_core::session::SessionError;
use o7_protocol::session::{
    AgentEntry, BlobText, ChatAttach, ChatStatusUpdate, ChatSummary, ConfigChoice, ConfigValue,
    EnvStatus, HistoryPage, PromptAck, QueuedPrompt, SessionLists, StorageStatus,
};
use o7_protocol::{Autonomy, EventBatch, UiAnswer};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, Webview};

use crate::demo::DemoKind;
use crate::error::{AppError, AppErrorCode};
use crate::reveal;
use crate::sessions::{self, ChannelSink, StatusChannelSink};
use crate::smoke::{self, SmokeReport};
use crate::state::{AppInfo, AppState};
use crate::window;

/// Everything about this launch: versions, OS, device, profile, database, window glass, launch
/// mode, notices, and the fatal startup error if there was one. Never fails.
#[tauri::command]
#[specta::specta]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    state.app_info()
}

/// Starts a scripted mock-agent turn and streams its frames over `channel` (one `EventBatch` at
/// most every 16 ms, each under 8 KiB). Returns the session id the frames carry, for
/// `cancel_demo_stream`. The stream stops by itself when the channel's page goes away.
#[tauri::command]
#[specta::specta]
pub fn start_demo_stream(
    state: State<'_, AppState>,
    kind: DemoKind,
    tokens_per_sec: u32,
    channel: Channel<EventBatch>,
) -> Result<String, AppError> {
    let (session_id, run) = state.demos.start(kind, tokens_per_sec, move |batch| {
        channel.send(batch).is_ok()
    })?;
    tauri::async_runtime::spawn(run);
    Ok(session_id)
}

/// Stops a demo stream; its channel then receives a final `turn_ended` with `stop_reason:
/// "cancelled"`. Unknown or already finished ids are ignored, so calling this twice is harmless.
#[tauri::command]
#[specta::specta]
pub fn cancel_demo_stream(state: State<'_, AppState>, session_id: String) {
    state.demos.cancel(&session_id);
}

/// The page has painted: show and focus the main window. Only the first call does anything, so a
/// reloaded page does not steal focus again.
#[tauri::command]
#[specta::specta]
pub fn window_ready(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    window::show_main_once(&app, &state.window_shown).map(|_shown| ())
}

/// Shows the data folder in the system file manager (Finder, Explorer, ...), or its nearest
/// existing parent when the folder is missing. Takes no path: the page can reveal only the folder
/// this launch uses, never one it names.
#[tauri::command]
#[specta::specta]
pub async fn reveal_data_dir(state: State<'_, AppState>) -> Result<(), AppError> {
    let data_dir = state.data_dir().map(Path::to_path_buf);
    // The file manager calls block (D-Bus on Linux, the shell on Windows), so they run on a
    // blocking thread, as in the opener plugin's own command.
    tauri::async_runtime::spawn_blocking(move || {
        reveal::reveal_data_dir(data_dir.as_deref(), |path| {
            tauri_plugin_opener::reveal_item_in_dir(path)
        })
    })
    .await
    .map_err(|error| {
        AppError::new(
            AppErrorCode::RevealFailed,
            format!("Could not show the data folder: {error}"),
        )
    })??;
    Ok(())
}

/// Dismisses the database repair notice (`recovery_notice` in `app_info`) for good: later launches
/// no longer show it. Takes no path: the shell acknowledges exactly the repair startup reported,
/// never one the page names. Calling it again, or with no notice, does nothing; when it fails the
/// notice stays.
#[tauri::command]
#[specta::specta]
pub async fn acknowledge_recovery(state: State<'_, AppState>) -> Result<(), AppError> {
    state.acknowledge_recovery().await
}

/// Smoke mode only: prints `O7_SMOKE_RESULT <json>` on stdout and, with
/// `O7_SMOKE_EXIT=1`, exits with 0 when the run passed and 1 otherwise, through the quit
/// flow like any other exit. Only the first report counts.
#[tauri::command]
#[specta::specta]
pub fn smoke_report(
    app: AppHandle,
    state: State<'_, AppState>,
    report: SmokeReport,
) -> Result<(), AppError> {
    let Some(plan) = state.launch.smoke_plan() else {
        return Err(AppError::new(
            AppErrorCode::NotInSmokeMode,
            "Smoke reports are only accepted while O7_SMOKE runs a smoke check.",
        ));
    };
    if state.smoke_reported.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let report = smoke::finalize(report, plan.scenario);
    smoke::print(&report);
    if state.launch.smoke_exit {
        app.exit(smoke::exit_code(&report));
    }
    Ok(())
}

/// The most `chat_blob` returns per call.
pub const MAX_BLOB_PAGE: u32 = 256 * 1024;

/// How long `agents_refresh` waits for the env and the probes before it answers with what is known.
pub const AGENTS_REFRESH_BOUND: Duration = Duration::from_secs(15);

/// A blob is named by the 64 lowercase hex digits of its sha256, and read at most
/// [`MAX_BLOB_PAGE`] bytes at a time.
fn check_blob_request(sha256: &str, len: u32) -> Result<(), AppError> {
    let hex = sha256.len() == 64
        && sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !hex {
        return Err(AppError::invalid_argument(
            "A blob is named by the 64 lowercase hex digits of its sha256.",
        ));
    }
    if len == 0 || len > MAX_BLOB_PAGE {
        return Err(AppError::invalid_argument(format!(
            "A blob page is 1 to {MAX_BLOB_PAGE} bytes, not {len}."
        )));
    }
    Ok(())
}

/// Every agent o7 knows, with its cached availability. Never waits: detection runs in the
/// background, and an agent still being checked reads `checking`.
#[tauri::command]
#[specta::specta]
pub async fn agents_list(app: AppHandle) -> Vec<AgentEntry> {
    let Some(state) = app.try_state::<AppState>() else {
        return Vec::new();
    };
    match state.sessions() {
        Ok(manager) => manager.agents(false).await,
        Err(_) => Vec::new(),
    }
}

/// Resolves the environment again and probes every agent, in parallel, within 15 s; agents whose
/// probe has not ended by then keep their last known availability.
#[tauri::command]
#[specta::specta]
pub async fn agents_refresh(app: AppHandle) -> Vec<AgentEntry> {
    let Some(state) = app.try_state::<AppState>() else {
        return Vec::new();
    };
    let Ok(manager) = state.sessions() else {
        return Vec::new();
    };
    match tokio::time::timeout(AGENTS_REFRESH_BOUND, manager.agents(true)).await {
        Ok(agents) => agents,
        Err(_) => {
            tracing::warn!("refreshing the agents took longer than 15 s; answering what is known");
            manager.agents(false).await
        }
    }
}

/// The environment agents start with: resolving, resolved (and how), or a fallback, with the
/// names of the variables no agent inherits. Never waits.
#[tauri::command]
#[specta::specta]
pub fn env_status(state: State<'_, AppState>) -> EnvStatus {
    state
        .sessions()
        .map(|manager| manager.env_status())
        .unwrap_or_default()
}

/// Starts resolving the environment again and returns the status at once.
#[tauri::command]
#[specta::specta]
pub fn env_refresh(state: State<'_, AppState>) -> EnvStatus {
    state
        .sessions()
        .map(|manager| manager.env_refresh())
        .unwrap_or_default()
}

/// Shows the system folder picker and returns the chosen folder, or `null` when the user cancelled.
/// Resolves when the dialog closes. A chat smoke run's picker answers its folder
/// (`O7_SMOKE_FOLDER`) at once: automation cannot click through a system dialog.
#[tauri::command]
#[specta::specta]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, AppError> {
    use tauri_plugin_dialog::DialogExt as _;
    if let Some(state) = app.try_state::<AppState>()
        && let Some(folder) = state.launch.smoke_folder()
    {
        tracing::info!(
            folder,
            "the smoke run's folder picker answers its chat folder"
        );
        return Ok(Some(folder.to_owned()));
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose the chat's folder")
        .set_can_create_directories(true);
    if let Some(window) = app.get_webview_window(window::MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_folder(move |picked| {
        // The command stopped waiting only if its page went away.
        let _ = tx.send(picked);
    });
    let picked = rx.await.map_err(|_| {
        AppError::new(
            AppErrorCode::DialogFailed,
            "The folder picker could not be shown.",
        )
    })?;
    sessions::picked_folder(picked)
}

/// Every chat, newest activity first.
#[tauri::command]
#[specta::specta]
pub async fn chats_list(state: State<'_, AppState>) -> Result<Vec<ChatSummary>, AppError> {
    let chats = state.sessions()?.chats().await?;
    // The next launch's first frame starts from this list.
    crate::launch_cache::keep(&state, o7_core::launch_cache::Part::Chats, &chats);
    Ok(chats)
}

/// Creates a chat with `agent_id` in `folder` (an absolute path) and starts its agent. Creating a
/// chat whose agent `needs_download` is the consent to download its adapter.
#[tauri::command]
#[specta::specta]
pub async fn chat_create(
    state: State<'_, AppState>,
    agent_id: String,
    folder: String,
) -> Result<ChatSummary, AppError> {
    Ok(state
        .sessions()?
        .create_chat(&agent_id, Path::new(&folder))
        .await?)
}

/// Creates a chat on an explicitly selected execution host. Unsupported hosts are refused before
/// writing the chat or starting any native process.
#[tauri::command]
#[specta::specta]
pub async fn chat_create_on_host(
    state: State<'_, AppState>,
    agent_id: String,
    agent_host: o7_protocol::agents::AgentHost,
    folder: String,
) -> Result<ChatSummary, AppError> {
    Ok(state
        .sessions()?
        .create_chat_on_host(&agent_id, agent_host, Path::new(&folder))
        .await?)
}

/// Reviewed capabilities for the detected install, without running authentication or updates.
#[tauri::command]
#[specta::specta]
pub async fn agent_maintenance(
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<o7_protocol::agents::AgentMaintenance, AppError> {
    Ok(state.sessions()?.agent_maintenance(&agent_id).await)
}

/// Ask the vendor CLI for local auth status, retaining no account details or raw output.
#[tauri::command]
#[specta::specta]
pub async fn agent_auth_status(
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<o7_protocol::agents::AgentAuthStatus, AppError> {
    Ok(state.sessions()?.agent_auth_status(&agent_id).await)
}

/// Explicit update action. No updates run as part of detection or release checks.
#[tauri::command]
#[specta::specta]
pub async fn agent_update(state: State<'_, AppState>, agent_id: String) -> Result<(), AppError> {
    Ok(state.sessions()?.update_agent(&agent_id).await?)
}

/// What the agent's last update printed in this run of o7: its last lines as plain text,
/// for the person who asked for the update. Kept in memory only, never stored, logged or sent on.
#[tauri::command]
#[specta::specta]
pub fn agent_update_output(
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<Option<String>, AppError> {
    Ok(state.sessions()?.agent_update_output(&agent_id))
}

/// An agent's Connection setting on this device: one of its connection ids, or `null` for
/// Automatic. New sessions use it; a chat's saved session keeps its own connection. Answers every
/// agent as `agents_list` reports them, without probing again.
#[tauri::command]
#[specta::specta]
pub async fn agent_set_connection(
    state: State<'_, AppState>,
    agent_id: String,
    connection: Option<String>,
) -> Result<Vec<AgentEntry>, AppError> {
    Ok(state
        .sessions()?
        .set_agent_connection(&agent_id, connection.as_deref())
        .await?)
}

/// Opens a view of a chat: the snapshot and the newest history page, consistent with the frames
/// `channel` carries from `next_seq` on. Opening a chat never starts its agent. A webview has one
/// view per chat; attaching again replaces the earlier one.
#[tauri::command]
#[specta::specta]
pub async fn chat_attach(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
    channel: Channel<EventBatch>,
) -> Result<ChatAttach, AppError> {
    let manager = state.sessions()?;
    let owner = webview.label().to_owned();
    let page = state.pages.for_view(&owner);
    let sink = Arc::new(ChannelSink::new(channel, owner, page));
    Ok(manager.attach(&chat_id, sink).await?)
}

/// Closes a view: its channel ends. Unknown ids are ignored.
#[tauri::command]
#[specta::specta]
pub fn chat_detach(state: State<'_, AppState>, subscription_id: String) {
    if let Ok(manager) = state.sessions() {
        manager.detach(&subscription_id);
    }
}

/// The history page before `before_seq` (the page's own `before_seq` from the last call), at most
/// about `max_bytes` (default 512 KiB).
#[tauri::command]
#[specta::specta]
pub async fn chat_history(
    state: State<'_, AppState>,
    chat_id: String,
    before_seq: u32,
    max_bytes: Option<u32>,
) -> Result<HistoryPage, AppError> {
    Ok(state
        .sessions()?
        .history(&chat_id, before_seq, max_bytes)
        .await?)
}

/// The chat's options, slash commands, capabilities and autonomy state. Fetch again whenever the
/// session's `revision` changes.
#[tauri::command]
#[specta::specta]
pub async fn chat_lists(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<SessionLists, AppError> {
    Ok(state.sessions()?.lists(&chat_id).await?)
}

/// Exact device-local folders supplied to a live driver, or a preflight for its next start.
#[tauri::command]
#[specta::specta]
pub async fn chat_folder_context(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<o7_protocol::workspace::ChatFolderContext, AppError> {
    Ok(state.sessions()?.chat_folder_context(&chat_id).await?)
}

/// Sends `text` as typed, or queues it behind the running turn (or the start). A stopped chat starts
/// its agent.
#[tauri::command]
#[specta::specta]
pub async fn chat_send(
    state: State<'_, AppState>,
    chat_id: String,
    text: String,
) -> Result<PromptAck, AppError> {
    Ok(state.sessions()?.send(&chat_id, text).await?)
}

/// Removes a queued prompt.
#[tauri::command]
#[specta::specta]
pub async fn chat_cancel_queued(
    state: State<'_, AppState>,
    chat_id: String,
    prompt_id: String,
) -> Result<(), AppError> {
    Ok(state
        .sessions()?
        .cancel_queued(&chat_id, &prompt_id)
        .await?)
}

/// The models `agent_id` listed the last time one of its chats ran on this device, for Settings'
/// default model. Empty until then: agents list their models only from a running session.
#[tauri::command]
#[specta::specta]
pub async fn agent_models(
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<Vec<ConfigChoice>, AppError> {
    Ok(state.sessions()?.reported_models(&agent_id))
}

/// The thinking levels `model` of `agent_id` offered the last time it ran on this device (`None`:
/// the model the agent starts on), for Settings' default thinking level. Empty when unknown or
/// when the model offers none.
#[tauri::command]
#[specta::specta]
pub async fn agent_thinking_levels(
    state: State<'_, AppState>,
    agent_id: String,
    model: Option<String>,
) -> Result<Vec<ConfigChoice>, AppError> {
    Ok(state
        .sessions()?
        .reported_thinking(&agent_id, model.as_deref()))
}

/// The words of a queued prompt as typed, for editing it in place.
#[tauri::command]
#[specta::specta]
pub async fn chat_queued_text(
    state: State<'_, AppState>,
    chat_id: String,
    prompt_id: String,
) -> Result<String, AppError> {
    Ok(state.sessions()?.queued_text(&chat_id, &prompt_id).await?)
}

/// Replaces a queued prompt's words (its attachments, mentions, place and hold stay) and returns
/// the queue after the change.
#[tauri::command]
#[specta::specta]
pub async fn chat_edit_queued(
    state: State<'_, AppState>,
    chat_id: String,
    prompt_id: String,
    text: String,
) -> Result<Vec<QueuedPrompt>, AppError> {
    Ok(state
        .sessions()?
        .edit_queued(&chat_id, &prompt_id, text)
        .await?)
}

/// Moves a queued prompt to `index` (0 goes next) and returns the queue after the change.
#[tauri::command]
#[specta::specta]
pub async fn chat_move_queued(
    state: State<'_, AppState>,
    chat_id: String,
    prompt_id: String,
    index: u32,
) -> Result<Vec<QueuedPrompt>, AppError> {
    Ok(state
        .sessions()?
        .move_queued(&chat_id, &prompt_id, index as usize)
        .await?)
}

/// Releases the held prompts ("Send queued"), starting the agent when it is stopped.
#[tauri::command]
#[specta::specta]
pub async fn chat_resume_queue(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<(), AppError> {
    Ok(state.sessions()?.resume_queue(&chat_id).await?)
}

/// "Send now": the queued prompt goes next, released if held. A running turn is interrupted
/// first when the agent can interrupt it; a stopped agent starts. Returns the queue after the
/// change.
#[tauri::command]
#[specta::specta]
pub async fn chat_send_now(
    state: State<'_, AppState>,
    chat_id: String,
    prompt_id: String,
) -> Result<Vec<QueuedPrompt>, AppError> {
    Ok(state.sessions()?.send_now(&chat_id, &prompt_id).await?)
}

/// Asks the agent to stop the running turn (Esc). While it is already cancelling, this force-stops.
#[tauri::command]
#[specta::specta]
pub async fn chat_interrupt(state: State<'_, AppState>, chat_id: String) -> Result<(), AppError> {
    Ok(state.sessions()?.interrupt(&chat_id).await?)
}

/// Ends the agent's processes at once (the second Esc).
#[tauri::command]
#[specta::specta]
pub async fn chat_force_stop(state: State<'_, AppState>, chat_id: String) -> Result<(), AppError> {
    Ok(state.sessions()?.force_stop(&chat_id).await?)
}

/// Starts the chat's agent, continuing its native session unless `fresh`. Failures arrive as cards.
#[tauri::command]
#[specta::specta]
pub async fn chat_start(
    state: State<'_, AppState>,
    chat_id: String,
    fresh: bool,
) -> Result<(), AppError> {
    Ok(state.sessions()?.start(&chat_id, fresh).await?)
}

/// Stops the chat's agent gracefully.
#[tauri::command]
#[specta::specta]
pub async fn chat_stop(state: State<'_, AppState>, chat_id: String) -> Result<(), AppError> {
    Ok(state.sessions()?.stop(&chat_id).await?)
}

/// Answers a permission card with one of its options; `null` cancels it.
#[tauri::command]
#[specta::specta]
pub async fn chat_respond_permission(
    state: State<'_, AppState>,
    chat_id: String,
    request_id: String,
    option_id: Option<String>,
) -> Result<(), AppError> {
    Ok(state
        .sessions()?
        .respond_permission(&chat_id, &request_id, option_id)
        .await?)
}

/// Accepts a plan waiting for approval as the person edited it. The core refuses it unless the
/// request is a plan approval, the option is its allow-once answer and the agent takes edited
/// plans.
#[tauri::command]
#[specta::specta]
pub async fn chat_accept_plan(
    state: State<'_, AppState>,
    chat_id: String,
    request_id: String,
    option_id: String,
    plan: String,
) -> Result<(), AppError> {
    Ok(state
        .sessions()?
        .accept_plan(&chat_id, &request_id, &option_id, &plan)
        .await?)
}

/// Answers a question card; `null` dismisses it.
#[tauri::command]
#[specta::specta]
pub async fn chat_respond_ui(
    state: State<'_, AppState>,
    chat_id: String,
    request_id: String,
    answer: Option<UiAnswer>,
) -> Result<(), AppError> {
    Ok(state
        .sessions()?
        .respond_ui(&chat_id, &request_id, answer)
        .await?)
}

/// Apply a supported native access or workflow choice after actor admission.
#[tauri::command]
#[specta::specta]
pub async fn chat_set_control(
    state: State<'_, AppState>,
    chat_id: String,
    change: o7_protocol::session::SessionControlChange,
) -> Result<SessionLists, AppError> {
    Ok(state.sessions()?.set_control(&chat_id, change).await?)
}

/// Changes one of the agent's options and returns the lists after the change (35 s at most).
#[tauri::command]
#[specta::specta]
pub async fn chat_set_option(
    state: State<'_, AppState>,
    chat_id: String,
    option_id: String,
    value: ConfigValue,
) -> Result<SessionLists, AppError> {
    Ok(state
        .sessions()?
        .set_option(&chat_id, &option_id, value)
        .await?)
}

/// Changes the chat's autonomy and returns the lists after the change (35 s at most). Works while
/// the agent is stopped: it applies when the agent starts.
#[tauri::command]
#[specta::specta]
pub async fn chat_set_autonomy(
    state: State<'_, AppState>,
    chat_id: String,
    autonomy: Autonomy,
) -> Result<SessionLists, AppError> {
    Ok(state.sessions()?.set_autonomy(&chat_id, autonomy).await?)
}

/// One page of a full output: at most `len` bytes (up to 256 KiB) from `offset`, ending on a
/// character boundary; continue from `next_offset`. `sha256` is the 64 lowercase hex digits of a
/// truncation's blob.
#[tauri::command]
#[specta::specta]
pub async fn chat_blob(
    state: State<'_, AppState>,
    chat_id: String,
    sha256: String,
    offset: u64,
    len: u32,
) -> Result<BlobText, AppError> {
    check_blob_request(&sha256, len)?;
    Ok(state
        .sessions()?
        .blob(&chat_id, &sha256, offset, len)
        .await?)
}

/// Shows the chat's newest protocol log in the system file manager. Takes no path: the shell
/// chooses the file.
#[tauri::command]
#[specta::specta]
pub async fn chat_reveal_log(state: State<'_, AppState>, chat_id: String) -> Result<(), AppError> {
    let manager = state.sessions()?;
    // The id joins a path below, so only a chat this profile has gets that far.
    let known = sessions::is_plain_id(&chat_id)
        && manager
            .chats()
            .await
            .map_err(|error| {
                AppError::new(
                    AppErrorCode::RevealFailed,
                    format!("Could not show the chat's log: {error}"),
                )
            })?
            .iter()
            .any(|chat| chat.id == chat_id);
    if !known {
        return Err(SessionError::ChatNotFound(chat_id).into());
    }
    let dir = manager.protocol_log_dir(&chat_id);
    tauri::async_runtime::spawn_blocking(move || {
        sessions::reveal_newest_log(&dir, |path| tauri_plugin_opener::reveal_item_in_dir(path))
    })
    .await
    .map_err(|error| {
        AppError::new(
            AppErrorCode::RevealFailed,
            format!("Could not show the chat's log: {error}"),
        )
    })??;
    Ok(())
}

/// Subscribes the calling webview to chat-list updates (status, attention, title) over `channel`,
/// replacing its earlier subscription. Returns the subscription id.
#[tauri::command]
#[specta::specta]
pub fn status_subscribe(
    webview: Webview,
    state: State<'_, AppState>,
    channel: Channel<ChatStatusUpdate>,
) -> Result<String, AppError> {
    let manager = state.sessions()?;
    let owner = webview.label().to_owned();
    let page = state.pages.for_status(&owner);
    Ok(manager.status_subscribe(Arc::new(StatusChannelSink::new(channel, owner, page))))
}

/// Whether the profile database is saving, paused (with how much is unsaved), or failed.
#[tauri::command]
#[specta::specta]
pub fn storage_status(state: State<'_, AppState>) -> StorageStatus {
    state
        .sessions()
        .map(|manager| manager.storage_status())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_requests_name_a_sha256_and_a_bounded_page() {
        let sha = "0123456789abcdef".repeat(4);
        assert!(check_blob_request(&sha, 1).is_ok());
        assert!(check_blob_request(&sha, MAX_BLOB_PAGE).is_ok());
        for (name, len) in [
            (sha.to_uppercase(), 1),
            (sha[..63].to_owned(), 1),
            (format!("{}g", &sha[..63]), 1),
            ("../../etc/passwd".to_owned(), 1),
            (sha.clone(), 0),
            (sha.clone(), MAX_BLOB_PAGE + 1),
        ] {
            let error = check_blob_request(&name, len).expect_err("refused");
            assert_eq!(error.code, AppErrorCode::InvalidArgument, "{name} {len}");
        }
    }
}
