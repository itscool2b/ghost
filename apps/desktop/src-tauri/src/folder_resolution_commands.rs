//! Profile-bound device-local folder resolution. Mutations retain admission to completion.
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};
use ghost_protocol::{
    folder_resolution::{FolderLocationReview, FolderResolution},
    workspace::ProjectFolder,
};
use tauri::{State, Webview};
fn current(webview: &Webview, state: &AppState) -> Result<(), AppError> {
    if state.quit.is_quitting() || !state.pages.for_view(webview.label()).is_current() {
        return Err(AppError::new(
            AppErrorCode::SessionNotReady,
            "This page is no longer active.",
        ));
    }
    Ok(())
}
#[tauri::command]
#[specta::specta]
pub async fn folder_code_roots(
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<Vec<String>, AppError> {
    current(&webview, &state)?;
    Ok(state.sessions()?.folder_code_roots().await?)
}
#[tauri::command]
#[specta::specta]
pub async fn folder_set_code_roots(
    webview: Webview,
    state: State<'_, AppState>,
    roots: Vec<String>,
) -> Result<Vec<String>, AppError> {
    current(&webview, &state)?;
    let sessions = state.sessions()?;
    tokio::spawn(async move { Ok(sessions.folder_set_code_roots(roots).await?) })
        .await
        .map_err(|_| AppError::new(AppErrorCode::Internal, "Code roots could not be saved."))?
}
#[tauri::command]
#[specta::specta]
pub async fn folder_resolution(
    webview: Webview,
    state: State<'_, AppState>,
    folder_id: String,
    search: bool,
) -> Result<FolderResolution, AppError> {
    current(&webview, &state)?;
    Ok(state
        .sessions()?
        .folder_resolution(&folder_id, search)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn folder_locate_review(
    webview: Webview,
    state: State<'_, AppState>,
    folder_id: String,
    path: String,
) -> Result<FolderLocationReview, AppError> {
    current(&webview, &state)?;
    Ok(state
        .sessions()?
        .folder_locate_review(&folder_id, &path)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn folder_locate_apply(
    webview: Webview,
    state: State<'_, AppState>,
    review: FolderLocationReview,
) -> Result<ProjectFolder, AppError> {
    current(&webview, &state)?;
    let sessions = state.sessions()?;
    tokio::spawn(async move { Ok(sessions.folder_locate_apply(review).await?) })
        .await
        .map_err(|_| {
            AppError::new(
                AppErrorCode::Internal,
                "The folder mapping task could not finish.",
            )
        })?
}
