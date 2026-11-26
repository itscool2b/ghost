//! Composer IPC is a thin binding over the Rust context service.
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};

#[tauri::command]
#[specta::specta]
pub async fn composer_draft(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<ghost_protocol::composer::ComposerDraft, AppError> {
    Ok(state.sessions()?.composer_draft(&chat_id).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn composer_save_draft(
    state: State<'_, AppState>,
    chat_id: String,
    input: ComposerInput,
) -> Result<(), AppError> {
    Ok(state
        .sessions()?
        .composer_save_draft(&chat_id, input)
        .await?)
}
use ghost_protocol::{
    composer::{ComposerAttachment, ComposerInput, FileMention},
    session::PromptAck,
};
use tauri::{AppHandle, Manager as _, State};

#[tauri::command]
#[specta::specta]
pub async fn chat_message_images(
    state: State<'_, AppState>,
    chat_id: String,
    context: ghost_protocol::composer::ChatImageContext,
) -> Result<Vec<ghost_protocol::composer::ChatImage>, AppError> {
    Ok(state
        .sessions()?
        .chat_message_images(&chat_id, context)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn chat_image_preview(
    state: State<'_, AppState>,
    chat_id: String,
    source: ghost_protocol::composer::ChatImageSource,
    full: bool,
) -> Result<ghost_protocol::composer::ChatImagePreview, AppError> {
    Ok(state
        .sessions()?
        .chat_image_preview(&chat_id, source, full)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn composer_steer(
    state: State<'_, AppState>,
    chat_id: String,
    expected_turn_id: String,
    input: ComposerInput,
) -> Result<ghost_protocol::composer::SteeringAck, AppError> {
    let sessions = state.sessions()?;
    Ok(sessions
        .composer_steer(&chat_id, expected_turn_id, input)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn project_file_preview(
    state: State<'_, AppState>,
    chat_id: String,
    path: String,
) -> Result<ghost_protocol::composer::ProjectFilePreview, AppError> {
    Ok(state
        .sessions()?
        .project_file_preview(&chat_id, path)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn composer_pick(
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<Option<ComposerAttachment>, AppError> {
    use tauri_plugin_dialog::DialogExt as _;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Attach an image or text file");
    if let Some(window) = app.get_webview_window(crate::window::MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_file(move |picked| {
        let _ = tx.send(picked);
    });
    let Some(picked) = rx.await.map_err(|_| {
        AppError::new(
            AppErrorCode::DialogFailed,
            "The file picker could not be shown.",
        )
    })?
    else {
        return Ok(None);
    };
    let path = picked
        .simplified()
        .into_path()
        .map_err(|_| AppError::new(AppErrorCode::DialogFailed, "Select a local file."))?;
    Ok(Some(
        state.sessions()?.composer_import(&chat_id, path).await?,
    ))
}

#[tauri::command]
#[specta::specta]
pub async fn composer_import(
    state: State<'_, AppState>,
    chat_id: String,
    path: String,
) -> Result<ComposerAttachment, AppError> {
    Ok(state
        .sessions()?
        .composer_import(&chat_id, path.into())
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn composer_paste(
    state: State<'_, AppState>,
    chat_id: String,
    bytes: Vec<u8>,
    name: Option<String>,
) -> Result<ComposerAttachment, AppError> {
    Ok(state
        .sessions()?
        .composer_paste(&chat_id, bytes, name)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn composer_mentions(
    state: State<'_, AppState>,
    chat_id: String,
    query: String,
) -> Result<Vec<FileMention>, AppError> {
    Ok(state.sessions()?.composer_mentions(&chat_id, query).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn composer_send(
    state: State<'_, AppState>,
    chat_id: String,
    input: ComposerInput,
) -> Result<PromptAck, AppError> {
    Ok(state.sessions()?.composer_send(&chat_id, input).await?)
}
