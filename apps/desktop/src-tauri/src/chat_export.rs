//! Explicit plaintext export through the native save dialog. Never replaces an existing file.
use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
    window,
};
use std::{io::Write, path::Path};
use tauri::{Manager, State, Webview};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
#[specta::specta]
pub async fn chat_debug_bundle(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<String, AppError> {
    let lease = state.sessions()?;
    // Keep the profile admitted even if the renderer closes during the bounded read.
    tokio::spawn(async move { lease.debug_bundle(&chat_id).await.map_err(AppError::from) })
        .await
        .map_err(|_| {
            AppError::new(
                AppErrorCode::Internal,
                "The diagnostics task could not finish.",
            )
        })?
}

#[tauri::command]
#[specta::specta]
pub async fn chat_export_markdown(
    webview: Webview,
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<Option<String>, AppError> {
    let page = state.pages.for_view(webview.label());
    if state.quit.is_quitting() || !page.is_current() {
        return Err(AppError::new(
            AppErrorCode::SessionNotReady,
            "This page is no longer active.",
        ));
    }
    let lease = state.sessions()?;
    let runtime = state.chats().ok_or_else(|| {
        AppError::new(
            AppErrorCode::StorageUnavailable,
            "The workspace is unavailable.",
        )
    })?;
    let app = webview.app_handle().clone();
    // Cancellation of IPC cannot release profile admission while publication is still running.
    tokio::spawn(async move {
        let _lease = lease;
        let markdown = runtime
            .store
            .export_markdown(&chat_id)
            .await
            .map_err(|error| AppError::new(AppErrorCode::StorageUnavailable, error.to_string()))?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Export saved chat as Markdown")
            .set_file_name("ghost-chat.md")
            .add_filter("Markdown", &["md"])
            .set_can_create_directories(true);
        if let Some(window) = app.get_webview_window(window::MAIN_WINDOW) {
            dialog = dialog.set_parent(&window);
        }
        dialog.save_file(move |picked| {
            let _ = tx.send(picked);
        });
        let Some(picked) = rx.await.map_err(|_| {
            AppError::new(
                AppErrorCode::DialogFailed,
                "The save dialog could not finish.",
            )
        })?
        else {
            return Ok(None);
        };
        if !page.is_current() || app.state::<AppState>().quit.is_quitting() {
            return Err(AppError::new(
                AppErrorCode::SessionNotReady,
                "The page closed before the export was saved. Nothing was exported.",
            ));
        }
        let path = picked
            .simplified()
            .into_path()
            .map_err(|_| AppError::invalid_argument("Choose a local file for the export."))?;
        tokio::task::spawn_blocking(move || {
            let _lease = _lease;
            publish(&path, markdown.as_bytes())?;
            Ok(Some(path.to_string_lossy().into_owned()))
        })
        .await
        .map_err(|_| AppError::new(AppErrorCode::Internal, "The export task could not finish."))?
    })
    .await
    .map_err(|_| AppError::new(AppErrorCode::Internal, "The export task could not finish."))?
}

fn publish(path: &Path, markdown: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .filter(|path| path.is_absolute())
        .ok_or_else(|| AppError::invalid_argument("Choose an absolute local file path."))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".auracode-export-")
        .tempfile_in(parent)
        .map_err(|_| save_error())?;
    staged
        .write_all(markdown)
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|_| save_error())?;
    staged.persist_noclobber(path).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            AppError::invalid_argument("A file already exists there. Choose a new name; exports never replace existing files.")
        } else { save_error() }
    })?;
    Ok(())
}

fn save_error() -> AppError {
    AppError::new(
        AppErrorCode::StorageUnavailable,
        "The export could not be saved. Check the folder permissions and free space, then choose a new file.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publishes_complete_file_without_replacing_existing_content() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chat.md");
        publish(&path, b"# Saved chat\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"# Saved chat\n");
        assert!(publish(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"# Saved chat\n");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
    #[test]
    fn failed_destination_leaves_no_partial_export() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("existing-directory");
        std::fs::create_dir(&destination).unwrap();
        assert!(publish(&destination, b"private chat").is_err());
        assert!(destination.is_dir());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(publish(Path::new("relative.md"), b"private chat").is_err());
    }
}
