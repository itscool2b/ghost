use super::{DictationEvent, DictationModel, Failure, Operation, audio, model};
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Manager, State, Webview, ipc::Channel};

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
pub fn dictation_model(webview: Webview) -> DictationModel {
    model::status(models(&webview).as_deref())
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
            let engine = model::engine(&folder)?;
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
