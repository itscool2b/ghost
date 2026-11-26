//! Coordinated projects (an experiment), as the project page reaches them. Every command here is
//! the person acting in the window: the session layer treats it as theirs, and an agent's own
//! changes come through its chat's project grant instead (`ghost_core::coordinator_service`).
//! Writes keep the active-profile lease until the writer has answered.
use crate::{error::AppError, state::AppState};
use ghost_protocol::coordinator::*;
use tauri::{Emitter, Manager, State};

/// Sent with a [`CoordinatorChanged`] after every committed change to a project's coordination,
/// whether the page or an agent made it, so an open project page reads again.
pub const COORDINATOR_CHANGED: &str = "auracode-coordinator-changed";

/// Forwards the active profile's coordination changes to the window. Call once a runtime is
/// published; the task ends with its store.
pub(crate) fn forward_changes(app: &tauri::AppHandle) {
    let Some(chats) = app.try_state::<AppState>().and_then(|state| state.chats()) else {
        return;
    };
    let mut changes = chats.manager.coordinator_changes();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match changes.recv().await {
                Ok(changed) => {
                    let _ = app.emit(COORDINATOR_CHANGED, changed);
                }
                // Changes were missed: an empty project means every open page reads again.
                Err(RecvError::Lagged(_)) => {
                    let _ = app.emit(
                        COORDINATOR_CHANGED,
                        CoordinatorChanged {
                            project_id: String::new(),
                        },
                    );
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

#[tauri::command]
#[specta::specta]
pub async fn coordinator_snapshot(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<CoordinatorSnapshot, AppError> {
    Ok(state.sessions()?.coordinator_snapshot(&project_id).await?)
}

/// Every coordinator and worker chat, for the sidebar's marks.
#[tauri::command]
#[specta::specta]
pub async fn coordinator_members(
    state: State<'_, AppState>,
) -> Result<Vec<CoordinationMember>, AppError> {
    Ok(state.sessions()?.coordination_members().await?)
}

/// One Context file: as it reads now, or at one of its kept revisions.
#[tauri::command]
#[specta::specta]
pub async fn coordinator_file(
    state: State<'_, AppState>,
    project_id: String,
    path: String,
    revision: Option<String>,
) -> Result<ContextFile, AppError> {
    Ok(state
        .sessions()?
        .coordinator_file(&project_id, &path, revision)
        .await?)
}

/// Starts a project's coordinator and sends it the brief.
#[tauri::command]
#[specta::specta]
pub async fn coordinator_start(
    state: State<'_, AppState>,
    input: CoordinatorStart,
) -> Result<CoordinatorSnapshot, AppError> {
    Ok(state.sessions()?.coordinator_start(input).await?)
}

/// One change from the project page; answers the project as it reads afterwards.
#[tauri::command]
#[specta::specta]
pub async fn coordinator_apply(
    state: State<'_, AppState>,
    project_id: String,
    action: CoordinatorAction,
) -> Result<CoordinatorSnapshot, AppError> {
    Ok(state
        .sessions()?
        .coordinator_apply(&project_id, action)
        .await?)
}
