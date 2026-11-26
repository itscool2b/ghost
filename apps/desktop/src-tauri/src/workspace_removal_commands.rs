//! Reviewed parent tombstones. The owned task retains the active profile lease until commit.
use crate::{error::AppError, state::AppState};
use ghost_protocol::workspace_removal::{
    RemovedWorkspace, WorkspaceRemovalAction, WorkspaceRemovalPreview, WorkspaceRemovalTarget,
};
use tauri::State;

#[tauri::command]
#[specta::specta]
pub async fn workspace_removed(
    state: State<'_, AppState>,
) -> Result<Vec<RemovedWorkspace>, AppError> {
    Ok(state.sessions()?.workspace_removed().await?)
}
#[tauri::command]
#[specta::specta]
pub async fn workspace_removal_preview(
    state: State<'_, AppState>,
    target: WorkspaceRemovalTarget,
    action: WorkspaceRemovalAction,
) -> Result<WorkspaceRemovalPreview, AppError> {
    Ok(state
        .sessions()?
        .workspace_removal_preview(target, action)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn workspace_removal_apply(
    state: State<'_, AppState>,
    browser: State<'_, std::sync::Arc<crate::browser::Browser>>,
    token: String,
) -> Result<(), AppError> {
    let lease = state.sessions()?;
    let manager = (*lease).clone();
    manager
        .workspace_removal_apply(&token, Box::new(lease))
        .await?;
    // The removed chats' browsers close with them.
    if let Ok(sessions) = state.sessions()
        && let Ok(snapshot) = sessions
            .workspace_snapshot(ghost_protocol::workspace::WorkspaceFilter {
                include_archived: true,
                project_id: None,
            })
            .await
    {
        let existing = snapshot
            .chats
            .into_iter()
            .map(|chat| chat.chat.id)
            .collect();
        browser.prune(&existing);
    }
    Ok(())
}
