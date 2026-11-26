//! Explicit publication only. The owned apply task keeps the active profile admitted to completion.
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};
use ghost_protocol::repository_publication::{
    RepositoryPublicationInput, RepositoryPublicationResult, RepositoryPublicationReview,
};
use tauri::{State, Webview};
#[tauri::command]
#[specta::specta]
pub async fn repository_publication_draft(
    webview: Webview,
    state: State<'_, AppState>,
    folder_id: String,
) -> Result<Option<ghost_protocol::repository_publication::RepositoryPublicationDraft>, AppError> {
    current(&webview, &state)?;
    Ok(state
        .sessions()?
        .repository_publication_draft(&folder_id)
        .await?)
}
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
pub async fn repository_publication_review(
    webview: Webview,
    state: State<'_, AppState>,
    folder_id: String,
    input: RepositoryPublicationInput,
) -> Result<RepositoryPublicationReview, AppError> {
    current(&webview, &state)?;
    Ok(state
        .sessions()?
        .repository_publication_review(&folder_id, input)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn repository_publication_apply(
    webview: Webview,
    state: State<'_, AppState>,
    folder_id: String,
    token: String,
) -> Result<RepositoryPublicationResult, AppError> {
    current(&webview, &state)?;
    let sessions = state.sessions()?;
    // The task owns the publication, so a page that reloads cannot abandon it halfway.
    tokio::spawn(async move {
        sessions
            .repository_publication_apply(folder_id, token)
            .await
            .map_err(AppError::from)
    })
    .await
    .map_err(|_| {
        AppError::new(
            AppErrorCode::Internal,
            "Publication stopped unexpectedly. Review its saved state before trying again.",
        )
    })?
}
