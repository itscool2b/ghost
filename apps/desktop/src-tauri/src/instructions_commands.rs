//! Thin Instructions commands. The webview names a target and a change; Rust resolves every folder,
//! reviews exact bytes, and applies by review id. Writes share the configuration mutation gate
//! with Library copies, so they never interleave.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ghost_config::instructions::{self, ApplyResult, Change, Prepared, Review, Snapshot, Target};
use ghost_protocol::worktree_setup::WorktreeSetup;
use tauri::State;

use crate::{
    config_library::LibraryService,
    error::{AppError, AppErrorCode},
    state::AppState,
};

/// At most this many reviews wait for Apply at once.
const PENDING: usize = 8;

struct Pending {
    generation: crate::state::ProfileGeneration,
    prepared: Prepared,
}

#[derive(Default)]
pub struct InstructionsService {
    pending: Mutex<HashMap<String, Pending>>,
}

impl InstructionsService {
    pub(crate) fn reset_profile(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn config_error(error: ghost_config::Error) -> AppError {
    AppError::new(AppErrorCode::InvalidArgument, error.to_string())
}

fn task_error() -> AppError {
    AppError::new(
        AppErrorCode::Internal,
        "The instructions task stopped unexpectedly. Its journal and backups were kept. Open Instructions again to check recovery.",
    )
}

/// The app-owned folder for the managed-copy record and backups: never a project or agent folder.
fn state_dir(state: &AppState) -> Result<PathBuf, AppError> {
    state
        .data_dir()
        .map(|dir| dir.join("instructions"))
        .ok_or_else(|| {
            AppError::new(
                AppErrorCode::DataDirUnknown,
                "Ghost's data folder is unavailable, so instruction backups have nowhere to go.",
            )
        })
}

#[tauri::command]
#[specta::specta]
pub async fn instructions_snapshot(
    state: State<'_, AppState>,
    library: State<'_, Arc<LibraryService>>,
    target: Target,
) -> Result<Snapshot, AppError> {
    let manager = state.sessions()?;
    let context = manager
        .instructions_context(&target, state_dir(&state)?)
        .await?;
    let gate = Arc::clone(&library.mutation);
    let _serial = gate.lock_owned().await;
    tauri::async_runtime::spawn_blocking(move || {
        let _serial = _serial;
        instructions::recover(&context.state_dir);
        instructions::snapshot(&context).map_err(config_error)
    })
    .await
    .map_err(|_| task_error())?
}

#[tauri::command]
#[specta::specta]
pub async fn instructions_review(
    state: State<'_, AppState>,
    service: State<'_, Arc<InstructionsService>>,
    library: State<'_, Arc<LibraryService>>,
    target: Target,
    change: Change,
) -> Result<Review, AppError> {
    let manager = state.sessions()?;
    let generation = manager.generation();
    let context = manager
        .instructions_context(&target, state_dir(&state)?)
        .await?;
    let gate = Arc::clone(&library.mutation);
    let _serial = gate.lock_owned().await;
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        let _serial = _serial;
        instructions::recover(&context.state_dir);
        instructions::prepare(&context, &change).map_err(config_error)
    })
    .await
    .map_err(|_| task_error())??;
    let review = prepared.review().clone();
    let mut pending = service.lock();
    pending.retain(|_, item| !item.prepared.is_expired() && item.generation.is_current());
    if pending.len() >= PENDING {
        return Err(AppError::new(
            AppErrorCode::InvalidArgument,
            "Close another instructions review before starting a new one.",
        ));
    }
    pending.insert(
        review.id.clone(),
        Pending {
            generation,
            prepared,
        },
    );
    Ok(review)
}

#[tauri::command]
#[specta::specta]
pub fn instructions_cancel(service: State<'_, Arc<InstructionsService>>, id: String) {
    service.lock().remove(&id);
}

#[tauri::command]
#[specta::specta]
pub async fn instructions_apply(
    state: State<'_, AppState>,
    service: State<'_, Arc<InstructionsService>>,
    library: State<'_, Arc<LibraryService>>,
    id: String,
) -> Result<ApplyResult, AppError> {
    let manager = state.sessions()?;
    let pending = service
        .lock()
        .remove(&id)
        .ok_or_else(|| config_error(ghost_config::Error::Stale))?;
    if !pending.generation.is_current() || pending.prepared.is_expired() {
        return Err(config_error(ghost_config::Error::Stale));
    }
    let gate = Arc::clone(&library.mutation);
    // Apply outlives the webview that asked: the task owns the gate and the transaction.
    tauri::async_runtime::spawn(async move {
        let _serial = gate.lock_owned().await;
        manager
            .instructions_apply(pending.prepared)
            .await
            .map_err(AppError::from)
    })
    .await
    .map_err(|_| task_error())?
}

#[tauri::command]
#[specta::specta]
pub async fn instructions_include_in_worktrees(
    state: State<'_, AppState>,
    target: Target,
    path: String,
) -> Result<WorktreeSetup, AppError> {
    Ok(state
        .sessions()?
        .instructions_include_in_worktrees(&target, &path)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn instructions_commit(
    state: State<'_, AppState>,
    target: Target,
    path: String,
) -> Result<String, AppError> {
    Ok(state
        .sessions()?
        .instructions_commit(&target, &path)
        .await?)
}
