use crate::{error::AppError, state::AppState};
use ghost_protocol::changes::{
    ChangeDiff, ChangesSnapshot, HunkRevert, NativeRewind, RevertPreview, RevertResult,
};
use tauri::State;

#[tauri::command]
#[specta::specta]
pub async fn chat_changes(
    state: State<'_, AppState>,
    chat_id: String,
    turn_id: Option<String>,
) -> Result<ChangesSnapshot, AppError> {
    Ok(state.sessions()?.chat_changes(&chat_id, turn_id).await?)
}
/// A sent message's Rewind through the agent's own checkpoints; `dry_run` only lists the files.
#[tauri::command]
#[specta::specta]
pub async fn chat_rewind_files(
    state: State<'_, AppState>,
    chat_id: String,
    turn_id: String,
    dry_run: bool,
) -> Result<NativeRewind, AppError> {
    Ok(state
        .sessions()?
        .chat_rewind_files(&chat_id, turn_id, dry_run)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn chat_change_diff(
    state: State<'_, AppState>,
    chat_id: String,
    checkpoint_id: String,
    path: String,
) -> Result<ChangeDiff, AppError> {
    Ok(state
        .sessions()?
        .chat_change_diff(&chat_id, &checkpoint_id, &path)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn checkpoint_revert_preview(
    state: State<'_, AppState>,
    chat_id: String,
    checkpoint_id: String,
    paths: Vec<String>,
) -> Result<RevertPreview, AppError> {
    Ok(state
        .sessions()?
        .checkpoint_revert_preview(&chat_id, &checkpoint_id, paths)
        .await?)
}
/// A revert of some hunks of one modified file; `checkpoint_revert` applies it.
#[tauri::command]
#[specta::specta]
pub async fn checkpoint_hunk_revert_preview(
    state: State<'_, AppState>,
    chat_id: String,
    checkpoint_id: String,
    selection: HunkRevert,
) -> Result<RevertPreview, AppError> {
    Ok(state
        .sessions()?
        .checkpoint_hunk_revert_preview(&chat_id, &checkpoint_id, selection)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn checkpoint_revert(
    state: State<'_, AppState>,
    chat_id: String,
    token: String,
) -> Result<RevertResult, AppError> {
    Ok(state
        .sessions()?
        .checkpoint_revert(&chat_id, &token)
        .await?)
}
