use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};
use ghost_protocol::agents::AgentInstallations;
use tauri::{State, Webview};

fn current(view: &Webview, state: &AppState) -> Result<(), AppError> {
    if state.quit.is_quitting() || !state.pages.for_view(view.label()).is_current() {
        return Err(AppError::new(
            AppErrorCode::SessionNotReady,
            "This page is no longer active.",
        ));
    }
    Ok(())
}
#[tauri::command]
#[specta::specta]
pub async fn agent_installations(
    webview: Webview,
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<AgentInstallations, AppError> {
    current(&webview, &state)?;
    Ok(state.sessions()?.agent_installations(&agent_id).await?)
}
#[tauri::command]
#[specta::specta]
pub async fn agent_select_installation(
    webview: Webview,
    state: State<'_, AppState>,
    agent_id: String,
    path: Option<String>,
) -> Result<AgentInstallations, AppError> {
    current(&webview, &state)?;
    let sessions = state.sessions()?;
    // Saving and publishing the preference continues if the initiating page closes.
    tokio::spawn(async move { Ok(sessions.select_agent_installation(&agent_id, path).await?) })
        .await
        .map_err(|_| {
            AppError::new(
                AppErrorCode::Internal,
                "The executable choice could not be saved.",
            )
        })?
}
