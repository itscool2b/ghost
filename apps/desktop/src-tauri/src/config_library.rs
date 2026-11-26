//! Thin Library commands. Rust owns root selection, revisions, mutation admission and recovery.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use ghost_config::{
    CopyPreview, CopyReceipt, CopyResult, Inventory, ItemPreview, ItemRef, Library, PreparedCopy,
    Source,
};
use tauri::State;

use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};

struct Pending {
    generation: crate::state::ProfileGeneration,
    _slot: tokio::sync::OwnedSemaphorePermit,
    library: Arc<Library>,
    sources: Vec<Source>,
    folder_id: Option<String>,
    prepared: PreparedCopy,
}

pub struct LibraryService {
    pending: Mutex<HashMap<String, Pending>>,
    pub(crate) mutation: Arc<tokio::sync::Mutex<()>>,
    previews: Arc<tokio::sync::Semaphore>,
}

impl LibraryService {
    pub(crate) fn reset_profile(&self) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

impl Default for LibraryService {
    fn default() -> Self {
        Self {
            pending: Mutex::default(),
            mutation: Arc::default(),
            previews: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
}

fn config_error(error: ghost_config::Error) -> AppError {
    AppError::new(AppErrorCode::InvalidArgument, error.to_string())
}

fn task_error() -> AppError {
    AppError::new(
        AppErrorCode::Internal,
        "The Library operation stopped unexpectedly. Retained recovery files were not removed.",
    )
}

#[tauri::command]
#[specta::specta]
pub async fn library_copy_history(
    state: State<'_, AppState>,
    folder_id: Option<String>,
) -> Result<Vec<CopyReceipt>, AppError> {
    let roots = state
        .sessions()?
        .config_library_roots(folder_id.as_deref())
        .await?;
    tauri::async_runtime::spawn_blocking(move || {
        Library::new(roots.sources)
            .map_err(config_error)?
            .copy_history()
            .map_err(config_error)
    })
    .await
    .map_err(|_| task_error())?
}

#[tauri::command]
#[specta::specta]
pub async fn library_snapshot(
    state: State<'_, AppState>,
    folder_id: Option<String>,
) -> Result<Inventory, AppError> {
    let roots = state
        .sessions()?
        .config_library_roots(folder_id.as_deref())
        .await?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut inventory = Library::new(roots.sources).map_err(config_error)?.inspect();
        inventory.notices.extend(roots.notices);
        Ok(inventory)
    })
    .await
    .map_err(|_| task_error())?
}

#[tauri::command]
#[specta::specta]
pub async fn library_item_preview(
    state: State<'_, AppState>,
    folder_id: Option<String>,
    item: ItemRef,
    revision: String,
) -> Result<ItemPreview, AppError> {
    let roots = state
        .sessions()?
        .config_library_roots(folder_id.as_deref())
        .await?;
    tauri::async_runtime::spawn_blocking(move || {
        Library::new(roots.sources)
            .map_err(config_error)?
            .preview(&item, &revision)
            .map_err(config_error)
    })
    .await
    .map_err(|_| task_error())?
}

#[tauri::command]
#[specta::specta]
pub async fn library_copy_preview(
    state: State<'_, AppState>,
    service: State<'_, Arc<LibraryService>>,
    folder_id: Option<String>,
    item: ItemRef,
    revision: String,
    to_source: String,
) -> Result<CopyPreview, AppError> {
    let manager = state.sessions()?;
    let roots = manager.config_library_roots(folder_id.as_deref()).await?;
    let service = Arc::clone(&service);
    service
        .pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|_, pending| !pending.prepared.is_expired());
    let slot = Arc::clone(&service.previews)
        .try_acquire_owned()
        .map_err(|_| {
            AppError::new(
                AppErrorCode::InvalidArgument,
                "Close another copy preview before preparing a new one.",
            )
        })?;
    tauri::async_runtime::spawn_blocking(move || {
        let _profile = manager;
        let library = Arc::new(Library::new(roots.sources.clone()).map_err(config_error)?);
        let prepared = library
            .prepare_copy(&item, &revision, &to_source)
            .map_err(config_error)?;
        let preview = prepared.preview().clone();
        let mut pending = service
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|_, pending| !pending.prepared.is_expired());
        if pending.len() >= 4 {
            return Err(AppError::new(
                AppErrorCode::InvalidArgument,
                "Close another copy preview before preparing a new one.",
            ));
        }
        pending.insert(
            preview.id.clone(),
            Pending {
                generation: _profile.generation(),
                _slot: slot,
                library,
                sources: roots.sources,
                folder_id,
                prepared,
            },
        );
        Ok(preview)
    })
    .await
    .map_err(|_| task_error())?
}

#[tauri::command]
#[specta::specta]
pub fn library_copy_cancel(service: State<'_, Arc<LibraryService>>, id: String) {
    service
        .pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id);
}

#[tauri::command]
#[specta::specta]
pub async fn library_copy_apply(
    state: State<'_, AppState>,
    service: State<'_, Arc<LibraryService>>,
    id: String,
) -> Result<CopyResult, AppError> {
    let manager = state.sessions()?;
    let pending = service
        .pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id)
        .ok_or_else(|| config_error(ghost_config::Error::Stale))?;
    if !pending.generation.is_current() {
        return Err(config_error(ghost_config::Error::Stale));
    }
    let gate = Arc::clone(&service.mutation);
    // Explicit apply outlives the initiating webview. Queued work still rechecks shutdown before
    // acquiring maintenance ownership, so a completed quit cannot release a deferred file write.
    tauri::async_runtime::spawn(async move {
        let _serial = gate.lock_owned().await;
        let current = manager
            .config_library_roots(pending.folder_id.as_deref())
            .await?;
        if current.sources != pending.sources {
            return Err(config_error(ghost_config::Error::Stale));
        }
        manager
            .config_library_apply(pending.library, pending.prepared)
            .await
            .map_err(AppError::from)
    })
    .await
    .map_err(|_| task_error())?
}

#[tauri::command]
#[specta::specta]
pub async fn library_copy_rollback(
    state: State<'_, AppState>,
    service: State<'_, Arc<LibraryService>>,
    folder_id: Option<String>,
    source_id: String,
    id: String,
) -> Result<CopyResult, AppError> {
    let manager = state.sessions()?;
    let gate = Arc::clone(&service.mutation);
    tauri::async_runtime::spawn(async move {
        let _serial = gate.lock_owned().await;
        let roots = manager.config_library_roots(folder_id.as_deref()).await?;
        let library = Arc::new(Library::new(roots.sources).map_err(config_error)?);
        manager
            .config_library_rollback(library, source_id, id)
            .await
            .map_err(AppError::from)
    })
    .await
    .map_err(|_| task_error())?
}

/// Shares an agent's MCP server with every agent: its entry, checked against the revision the
/// person reviewed, becomes a shared definition for all projects through the same import as pasted
/// native JSON. Values from its `env` and `headers` stay on this device as local credentials.
#[tauri::command]
#[specta::specta]
pub async fn library_mcp_share(
    state: State<'_, AppState>,
    folder_id: Option<String>,
    item: ItemRef,
    revision: String,
    expected: u32,
) -> Result<ghost_protocol::mcp::McpSnapshot, AppError> {
    let manager = state.sessions()?;
    let roots = manager.config_library_roots(folder_id.as_deref()).await?;
    let key = item.key.clone();
    let server = tauri::async_runtime::spawn_blocking(move || {
        Library::new(roots.sources)
            .map_err(config_error)?
            .mcp_share(&item, &revision)
            .map_err(config_error)
    })
    .await
    .map_err(|_| task_error())??;
    let mut servers = serde_json::Map::new();
    servers.insert(key, server);
    let document = serde_json::json!({ "mcpServers": servers }).to_string();
    Ok(manager
        .shared_mcp(ghost_core::mcp::Command::ImportNative {
            expected,
            project_id: None,
            document,
        })
        .await?)
}
