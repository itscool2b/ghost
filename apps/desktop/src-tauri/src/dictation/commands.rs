use super::larger::{self, LargerEvent};
use super::{DictationEvent, DictationModel, Failure, Operation, audio, model};
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State, Webview, ipc::Channel};

fn error(message: String) -> AppError {
    AppError::new(AppErrorCode::InvalidArgument, message)
}

/// Where the bundled models are: `dictation/` in the app's resources (tauri.conf.json).
fn models(webview: &Webview) -> Option<PathBuf> {
    webview
        .app_handle()
        .path()
        .resource_dir()
        .ok()
        .map(|resources| resources.join("dictation"))
}

/// Where a model fetched after installation is kept: `models/dictation` in the data folder,
/// shared by every profile, since a model is nobody's data.
fn downloads(state: &AppState) -> Option<PathBuf> {
    state
        .data_dir()
        .map(|root| root.join("models").join("dictation"))
}

fn operation(
    view: &Webview,
    state: &AppState,
    id: String,
    channel: Channel<DictationEvent>,
) -> Result<Operation, AppError> {
    let page = state.pages.for_view(view.label());
    if state.quit.is_quitting() || !page.is_current() {
        return Err(error("This page is no longer active.".into()));
    }
    let profile = state.sessions()?.generation();
    state
        .dictation
        .begin(
            view.label(),
            id,
            Arc::new(move |event| channel.send(event).is_ok()),
            Box::new(move || page.is_current() && profile.is_current()),
        )
        .map_err(error)
}

#[tauri::command]
#[specta::specta]
pub fn dictation_model(webview: Webview, state: State<'_, AppState>) -> DictationModel {
    model::status(models(&webview).as_deref(), downloads(&state).as_deref())
}

/// Fetches the larger speech model by itself (`larger.rs`): a first look a minute after launch,
/// then again every quarter of an hour for as long as it is wanted and not here, so a computer
/// that was offline or short of room gets it once it can. A person's Cancel or Remove ends that.
pub fn start_automatic(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut wait = larger::FIRST;
        loop {
            tokio::time::sleep(wait).await;
            wait = larger::AGAIN;
            let folder = {
                let Some(state) = app.try_state::<AppState>() else {
                    return;
                };
                if state.quit.is_quitting() {
                    return;
                }
                // Dictation that cannot run here has no use for a better model.
                let bundled = app.path().resource_dir().ok();
                if !bundled.is_some_and(|resources| model::present(&resources.join("dictation"))) {
                    return;
                }
                let Some(folder) = downloads(&state) else {
                    return;
                };
                folder
            };
            if larger::attempt(folder).await == larger::Attempt::Done {
                return;
            }
        }
    });
}

/// Fetches the larger speech model, once, from its pinned address (`larger.rs`): the page supplies
/// nothing but the wish, which also takes back an earlier Cancel or Remove. Progress arrives on
/// `channel`; the download goes on if the page leaves.
#[tauri::command]
#[specta::specta]
pub fn dictation_larger_download(
    webview: Webview,
    state: State<'_, AppState>,
    channel: Channel<LargerEvent>,
) -> Result<(), AppError> {
    if state.quit.is_quitting() || !state.pages.for_view(webview.label()).is_current() {
        return Err(error("This page is no longer active.".into()));
    }
    if !audio::cpu_supported() {
        return Err(error(Failure::unsupported().message));
    }
    let folder = downloads(&state)
        .ok_or_else(|| error("This copy of o7 has nowhere to keep a downloaded model.".into()))?;
    if model::larger(Some(&folder)).is_some() {
        return Err(error("The larger model is already here.".into()));
    }
    larger::accept(&folder).map_err(error)?;
    larger::start(folder, move |event| {
        let _ = channel.send(event);
    })
    .map_err(error)
}

/// Stops the download, or says no to one o7 has not started yet. What arrived is kept, and the
/// next download continues from it; o7 does not start one by itself again.
#[tauri::command]
#[specta::specta]
pub fn dictation_larger_cancel(
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    if !state.pages.for_view(webview.label()).is_current() {
        return Err(error("This page is no longer active.".into()));
    }
    // The no is written before the download hears it, so a page told that it stopped reads the
    // no with it; the download is stopped whether or not the writing worked.
    let saved = downloads(&state).map_or(Ok(()), |folder| larger::decline(&folder));
    larger::cancel();
    saved.map_err(error)
}

/// Deletes the larger model; dictation returns to the built-in one, and o7 does not fetch the
/// model by itself again.
#[tauri::command]
#[specta::specta]
pub fn dictation_larger_remove(
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    if !state.pages.for_view(webview.label()).is_current() {
        return Err(error("This page is no longer active.".into()));
    }
    let Some(folder) = downloads(&state) else {
        return Ok(());
    };
    // The no first: a model that is gone and fetched again at the next launch would be worse
    // than one that is still here.
    larger::decline(&folder).map_err(error)?;
    larger::remove(&folder).map_err(error)
}

/// Opens the microphone at once and streams the live text until `dictation_stop` (finish) or
/// `dictation_cancel`. There is no time limit.
#[tauri::command]
#[specta::specta]
pub fn dictation_start(
    webview: Webview,
    state: State<'_, AppState>,
    id: String,
    channel: Channel<DictationEvent>,
) -> Result<(), AppError> {
    let folder = models(&webview);
    let fetched = downloads(&state);
    let operation = operation(&webview, &state, id, channel)?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !audio::cpu_supported() {
                return Err(Failure::unsupported());
            }
            let folder = folder.ok_or_else(|| {
                Failure::model("o7 could not find its dictation models. Reinstall o7.")
            })?;
            if !operation.emit(DictationEvent::Preparing) {
                return Err(Failure::cancelled());
            }
            let engine = model::engine(&folder, fetched.as_deref())?;
            audio::run(&engine, &operation)
        }))
        .unwrap_or_else(|_| {
            Err(Failure::other(
                "Dictation stopped unexpectedly. Your draft is unchanged. Try again.",
            ))
        });
        operation.finish(result);
    });
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn dictation_stop(webview: Webview, state: State<'_, AppState>, id: String) {
    state.dictation.stop(webview.label(), &id);
}

#[tauri::command]
#[specta::specta]
pub fn dictation_cancel(webview: Webview, state: State<'_, AppState>, id: String) {
    state.dictation.cancel(webview.label(), &id);
}
