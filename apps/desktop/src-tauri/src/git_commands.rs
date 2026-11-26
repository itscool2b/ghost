//! Thin Git workspace IPC. The core owns review tokens, folder admission and mutation journals.

use ghost_protocol::git::{
    GitWorkspace, IsolationInput, IsolationPreview, IsolationResult, ManagedWorktree,
    WorktreeAction, WorktreeActionPreview, WorktreeActionResult, WorktreeDestination,
    WorktreeRecoveryAction, WorktreeRecoveryPreview, WorktreeRelocationPreview,
};
use tauri::State;

use crate::{error::AppError, state::AppState};

#[tauri::command]
#[specta::specta]
pub async fn profile_worktrees(
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> Result<Vec<ManagedWorktree>, AppError> {
    Ok(state
        .sessions()?
        .profile_worktrees(project_id.as_deref())
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn profile_worktree_recovery_preview(
    state: State<'_, AppState>,
    worktree_id: String,
    action: WorktreeRecoveryAction,
) -> Result<WorktreeRecoveryPreview, AppError> {
    Ok(state
        .sessions()?
        .profile_worktree_recovery_preview(&worktree_id, action)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn profile_worktree_recreate_preview(
    state: State<'_, AppState>,
    worktree_id: String,
) -> Result<ghost_protocol::git::WorktreeRecreatePreview, AppError> {
    Ok(state
        .sessions()?
        .profile_worktree_recreate_preview(&worktree_id)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn profile_worktree_recovery_apply(
    state: State<'_, AppState>,
    worktree_id: String,
    token: String,
    recreate: bool,
) -> Result<WorktreeActionResult, AppError> {
    Ok(state
        .sessions()?
        .profile_worktree_recovery_apply(&worktree_id, &token, recreate)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn profile_worktree_open_folder(
    state: State<'_, AppState>,
    worktree_id: String,
) -> Result<(), AppError> {
    let folder = state
        .sessions()?
        .profile_worktree_folder(&worktree_id)
        .await?;
    tauri::async_runtime::spawn_blocking(move || {
        tauri_plugin_opener::open_path(folder, None::<&str>)
    })
    .await
    .map_err(|_| {
        AppError::new(
            crate::error::AppErrorCode::Internal,
            "The folder could not be opened.",
        )
    })?
    .map_err(|_| {
        AppError::new(
            crate::error::AppErrorCode::Internal,
            "The folder could not be opened.",
        )
    })?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn git_workspace(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<GitWorkspace, AppError> {
    Ok(state.sessions()?.git_workspace(&chat_id).await?)
}

/// Reviews committing chosen files of the chat's folder as they are now.
#[tauri::command]
#[specta::specta]
pub async fn git_commit_preview(
    state: State<'_, AppState>,
    chat_id: String,
    input: ghost_protocol::git::CommitInput,
) -> Result<ghost_protocol::git::CommitPreview, AppError> {
    Ok(state
        .sessions()?
        .git_commit_preview(&chat_id, input)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn git_commit_apply(
    state: State<'_, AppState>,
    chat_id: String,
    token: String,
) -> Result<ghost_protocol::git::CommitResult, AppError> {
    Ok(state.sessions()?.git_commit_apply(&chat_id, &token).await?)
}

/// Reviews a fast-forward push of the chat folder's branch.
#[tauri::command]
#[specta::specta]
pub async fn git_push_preview(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<ghost_protocol::git::PushPreview, AppError> {
    Ok(state.sessions()?.git_push_preview(&chat_id).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn git_push_apply(
    state: State<'_, AppState>,
    chat_id: String,
    token: String,
) -> Result<ghost_protocol::git::PushResult, AppError> {
    Ok(state.sessions()?.git_push_apply(&chat_id, &token).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn managed_worktrees(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<Vec<ManagedWorktree>, AppError> {
    Ok(state.sessions()?.managed_worktrees(&chat_id).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn isolation_preview(
    state: State<'_, AppState>,
    chat_id: String,
    input: IsolationInput,
) -> Result<IsolationPreview, AppError> {
    Ok(state.sessions()?.isolation_preview(&chat_id, input).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn isolation_apply(
    state: State<'_, AppState>,
    chat_id: String,
    token: String,
) -> Result<IsolationResult, AppError> {
    Ok(state.sessions()?.isolation_apply(&chat_id, &token).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_action_preview(
    state: State<'_, AppState>,
    chat_id: String,
    worktree_id: String,
    action: WorktreeAction,
) -> Result<WorktreeActionPreview, AppError> {
    Ok(state
        .sessions()?
        .worktree_action_preview(&chat_id, &worktree_id, action)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_action_apply(
    state: State<'_, AppState>,
    chat_id: String,
    token: String,
) -> Result<WorktreeActionResult, AppError> {
    Ok(state
        .sessions()?
        .worktree_action_apply(&chat_id, &token)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_recovery_preview(
    state: State<'_, AppState>,
    chat_id: String,
    worktree_id: String,
    action: WorktreeRecoveryAction,
) -> Result<WorktreeRecoveryPreview, AppError> {
    Ok(state
        .sessions()?
        .worktree_recovery_preview(&chat_id, &worktree_id, action)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_recovery_apply(
    state: State<'_, AppState>,
    chat_id: String,
    worktree_id: String,
    token: String,
) -> Result<WorktreeActionResult, AppError> {
    Ok(state
        .sessions()?
        .worktree_recovery_apply(&chat_id, &worktree_id, &token)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_relocation_preview(
    state: State<'_, AppState>,
    chat_id: String,
    worktree_id: String,
    destination: WorktreeDestination,
) -> Result<WorktreeRelocationPreview, AppError> {
    Ok(state
        .sessions()?
        .worktree_relocation_preview(&chat_id, &worktree_id, destination)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_relocation_apply(
    state: State<'_, AppState>,
    chat_id: String,
    token: String,
) -> Result<ghost_protocol::conversation::ContinuationResult, AppError> {
    Ok(state
        .sessions()?
        .worktree_relocation_apply(&chat_id, &token)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_pr_preview(
    state: State<'_, AppState>,
    chat_id: String,
    worktree_id: String,
    input: ghost_protocol::git::PullRequestInput,
) -> Result<ghost_protocol::git::PullRequestPreview, AppError> {
    Ok(state
        .sessions()?
        .worktree_pr_preview(&chat_id, &worktree_id, input)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_pr_apply(
    state: State<'_, AppState>,
    chat_id: String,
    token: String,
) -> Result<ghost_protocol::git::PullRequestResult, AppError> {
    Ok(state
        .sessions()?
        .worktree_pr_apply(&chat_id, &token)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_chat_preview(
    state: State<'_, AppState>,
    input: ghost_protocol::git::WorktreeChatInput,
) -> Result<ghost_protocol::git::WorktreeChatPreview, AppError> {
    Ok(state.sessions()?.worktree_chat_preview(input).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_chat_apply(
    state: State<'_, AppState>,
    token: String,
) -> Result<ghost_protocol::git::WorktreeChatResult, AppError> {
    Ok(state.sessions()?.worktree_chat_apply(&token).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_recreate_preview(
    state: State<'_, AppState>,
    chat_id: String,
    worktree_id: String,
) -> Result<ghost_protocol::git::WorktreeRecreatePreview, AppError> {
    Ok(state
        .sessions()?
        .worktree_recreate_preview(&chat_id, &worktree_id)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn worktree_recreate_apply(
    state: State<'_, AppState>,
    chat_id: String,
    worktree_id: String,
    token: String,
) -> Result<WorktreeActionResult, AppError> {
    Ok(state
        .sessions()?
        .worktree_recreate_apply(&chat_id, &worktree_id, &token)
        .await?)
}
