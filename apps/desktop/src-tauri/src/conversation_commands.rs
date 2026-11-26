use crate::{error::AppError, state::AppState};
use ghost_protocol::conversation::{
    ContinuationAction, ContinuationPreview, ContinuationResult, ConversationTurns, EditablePrompt,
};
use tauri::State;

#[tauri::command]
#[specta::specta]
pub async fn conversation_pending(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<Option<ghost_protocol::conversation::PendingHandoff>, AppError> {
    Ok(state.sessions()?.conversation_pending(&chat_id).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn conversation_turns(
    state: State<'_, AppState>,
    chat_id: String,
    before_ordinal: Option<u32>,
) -> Result<ConversationTurns, AppError> {
    Ok(state
        .sessions()?
        .conversation_turns(&chat_id, before_ordinal)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn conversation_editable(
    state: State<'_, AppState>,
    chat_id: String,
    turn_id: String,
) -> Result<EditablePrompt, AppError> {
    Ok(state
        .sessions()?
        .conversation_editable(&chat_id, &turn_id)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn continuation_preview(
    state: State<'_, AppState>,
    chat_id: String,
    action: ContinuationAction,
) -> Result<ContinuationPreview, AppError> {
    Ok(state
        .sessions()?
        .continuation_preview(&chat_id, action)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn continuation_apply(
    state: State<'_, AppState>,
    chat_id: String,
    token: String,
) -> Result<ContinuationResult, AppError> {
    Ok(state
        .sessions()?
        .continuation_apply(&chat_id, &token)
        .await?)
}
