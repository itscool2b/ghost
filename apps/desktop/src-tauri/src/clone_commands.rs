//! Clone IPC carries the person's form and reviewed identifiers, never an executable or command.
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};
use ghost_core::cloning::Clones;
use ghost_protocol::setup_actions::{CloneCheck, CloneInput, CloneRecord, CloneRecovery};
use std::sync::Arc;
use tauri::{State, Webview};

fn error(value: ghost_core::cloning::CloneError) -> AppError {
    AppError::new(AppErrorCode::InvalidArgument, value.to_string())
}
fn current(webview: &Webview, state: &AppState) -> Result<(), AppError> {
    if state.quit.is_quitting() {
        return Err(AppError::new(
            AppErrorCode::ShuttingDown,
            "Ghost is quitting.",
        ));
    }
    if !state.pages.for_view(webview.label()).is_current() {
        return Err(AppError::new(
            AppErrorCode::SessionNotReady,
            "This page is no longer active.",
        ));
    }
    Ok(())
}
/// Reads the clone form. A complete form comes back with the exact command Start will run.
#[tauri::command]
#[specta::specta]
pub async fn clone_check(
    webview: Webview,
    state: State<'_, AppState>,
    service: State<'_, Arc<Clones>>,
    input: CloneInput,
) -> Result<CloneCheck, AppError> {
    current(&webview, &state)?;
    let page = state.pages.for_view(webview.label());
    let check = state
        .sessions()?
        .clone_check(&service, webview.label(), input)
        .await?;
    if !page.is_current() || state.quit.is_quitting() {
        if let Some(review) = &check.review {
            service.cancel_review(webview.label(), &review.id);
        }
        return Err(AppError::new(
            AppErrorCode::SessionNotReady,
            "This clone form was closed.",
        ));
    }
    Ok(check)
}
#[tauri::command]
#[specta::specta]
pub fn clone_cancel_review(webview: Webview, service: State<'_, Arc<Clones>>, id: String) {
    service.cancel_review(webview.label(), &id);
}
#[tauri::command]
#[specta::specta]
pub fn clone_start(
    webview: Webview,
    state: State<'_, AppState>,
    service: State<'_, Arc<Clones>>,
    id: String,
) -> Result<CloneRecord, AppError> {
    current(&webview, &state)?;
    let admission = state.sessions()?;
    let manager = (*admission).clone();
    let chats = state.chats().ok_or_else(|| {
        AppError::new(
            AppErrorCode::SessionNotReady,
            "The local workspace is unavailable.",
        )
    })?;
    Ok(manager.clone_start(
        &service,
        webview.label(),
        &id,
        Arc::clone(&chats.registry),
        Box::new(admission),
    )?)
}
#[tauri::command]
#[specta::specta]
pub fn clone_cancel(webview: Webview, service: State<'_, Arc<Clones>>, id: String) {
    service.cancel(webview.label(), &id);
}
/// The clone's record; with `after`, the next revision once Git reports progress (or within ten
/// seconds), so the window follows the clone without polling.
#[tauri::command]
#[specta::specta]
pub async fn clone_status(
    webview: Webview,
    service: State<'_, Arc<Clones>>,
    id: String,
    after: Option<u32>,
) -> Result<CloneRecord, AppError> {
    service
        .status(webview.label(), &id, after)
        .await
        .map_err(error)
}

#[tauri::command]
#[specta::specta]
pub async fn clone_recovery_list(
    webview: Webview,
    state: State<'_, AppState>,
    service: State<'_, Arc<Clones>>,
) -> Result<Vec<CloneRecovery>, AppError> {
    current(&webview, &state)?;
    let service = Arc::clone(service.inner());
    tauri::async_runtime::spawn_blocking(move || service.recovery_list())
        .await
        .map_err(|_| AppError::new(AppErrorCode::Internal, "Clone recovery could not be read."))?
        .map_err(error)
}

/// Revalidates the stored destination before handing it to the normal project form.
#[tauri::command]
#[specta::specta]
pub async fn clone_recovery_ready(
    webview: Webview,
    state: State<'_, AppState>,
    service: State<'_, Arc<Clones>>,
    id: String,
) -> Result<CloneRecovery, AppError> {
    current(&webview, &state)?;
    let service = Arc::clone(service.inner());
    let record = tauri::async_runtime::spawn_blocking(move || service.recovery_get(&id))
        .await
        .map_err(|_| {
            AppError::new(
                AppErrorCode::Internal,
                "The cloned folder could not be checked.",
            )
        })?
        .map_err(error)?;
    if !record.can_add {
        return Err(AppError::new(AppErrorCode::InvalidArgument, record.detail));
    }
    Ok(record)
}

#[tauri::command]
#[specta::specta]
pub async fn clone_recovery_open(
    webview: Webview,
    state: State<'_, AppState>,
    service: State<'_, Arc<Clones>>,
    id: String,
) -> Result<(), AppError> {
    current(&webview, &state)?;
    let service = Arc::clone(service.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let record = service.recovery_get(&id).map_err(error)?;
        let path = record
            .destination
            .filter(|_| record.can_open)
            .ok_or_else(|| {
                AppError::new(
                    AppErrorCode::InvalidArgument,
                    "The saved clone folder cannot be safely opened.",
                )
            })?;
        tauri_plugin_opener::reveal_item_in_dir(path).map_err(|_| {
            AppError::new(
                AppErrorCode::RevealFailed,
                "The file manager could not show this folder.",
            )
        })
    })
    .await
    .map_err(|_| {
        AppError::new(
            AppErrorCode::Internal,
            "The clone folder could not be opened.",
        )
    })?
}

#[tauri::command]
#[specta::specta]
pub async fn clone_recovery_dismiss(
    webview: Webview,
    state: State<'_, AppState>,
    service: State<'_, Arc<Clones>>,
    id: String,
) -> Result<(), AppError> {
    current(&webview, &state)?;
    let service = Arc::clone(service.inner());
    tauri::async_runtime::spawn_blocking(move || service.recovery_dismiss(&id))
        .await
        .map_err(|_| {
            AppError::new(
                AppErrorCode::Internal,
                "The recovery notice could not be dismissed.",
            )
        })?
        .map_err(error)
}
