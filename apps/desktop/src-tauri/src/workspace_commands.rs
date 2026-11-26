//! Typed UI bindings for Rust-owned project organization and folder preflight.

use std::time::Duration;

use ghost_core::session::{StopReport, StopScope};
use ghost_protocol::workspace::{
    ChatCreatePreview, ChatMove, ChatRole, ChatsStopped, FolderBlockers, FolderMatches,
    FolderScope, ProjectChatCreate, ProjectCreate, ProjectFolder, ProjectInput, ProjectSummary,
    RoleModel, WorkspaceChat, WorkspaceFilter, WorkspaceSnapshot,
};
use tauri::{State, Webview};

use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};

/// `chats_stop` waits this long for the agents to be gone; a stop is forced well before.
const STOP_WAIT: Duration = Duration::from_secs(30);

/// Stopping and listing other chats is for this window's current page only, never for a page
/// that is closing or replaced (and never for remote access, which has no such command).
fn current(webview: &Webview, state: &AppState) -> Result<(), AppError> {
    if state.quit.is_quitting() || !state.pages.for_view(webview.label()).is_current() {
        return Err(AppError::new(
            AppErrorCode::SessionNotReady,
            "This page is no longer active.",
        ));
    }
    Ok(())
}

fn stopped(report: StopReport) -> ChatsStopped {
    ChatsStopped {
        stopped: report.stopped,
        kept: report.kept,
        stopping: report.stopping,
    }
}

/// Stops these chats, a turn in progress included, and answers once their agents' processes
/// are gone: an action that waited for them can then go at once.
#[tauri::command]
#[specta::specta]
pub async fn chats_stop(
    webview: Webview,
    state: State<'_, AppState>,
    chat_ids: Vec<String>,
) -> Result<ChatsStopped, AppError> {
    current(&webview, &state)?;
    if chat_ids.len() > 256 || chat_ids.iter().any(|id| id.is_empty() || id.len() > 128) {
        return Err(AppError::invalid_argument(
            "Choose up to 256 chats to stop.",
        ));
    }
    let sessions = state.sessions()?;
    // A chat open in its terminal stops by that terminal closing: the person asked to stop it.
    crate::terminal::end_terminal_chats(&state, Some(&chat_ids));
    Ok(stopped(
        sessions
            .stop_all(StopScope::Chats(chat_ids), STOP_WAIT)
            .await,
    ))
}

/// Stops every idle agent (ready, unwatched, nothing open, queued or running in the background)
/// and answers once they are gone. Working, waiting and terminal chats are left as they are.
#[tauri::command]
#[specta::specta]
pub async fn agents_stop_idle(
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<ChatsStopped, AppError> {
    current(&webview, &state)?;
    let sessions = state.sessions()?;
    Ok(stopped(sessions.stop_idle(StopScope::All).await))
}

/// The chats with a live agent in the folders `scope` names, with what each does, and the
/// project terminals open there.
#[tauri::command]
#[specta::specta]
pub async fn folder_blockers(
    webview: Webview,
    state: State<'_, AppState>,
    scope: FolderScope,
) -> Result<FolderBlockers, AppError> {
    current(&webview, &state)?;
    Ok(state.sessions()?.folder_blockers(scope).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn workspace_snapshot(
    state: State<'_, AppState>,
    filter: WorkspaceFilter,
) -> Result<WorkspaceSnapshot, AppError> {
    // The sidebar reads the workspace whole; the next launch's first frame starts from that read.
    let whole = filter.include_archived && filter.project_id.is_none();
    let snapshot = state.sessions()?.workspace_snapshot(filter).await?;
    if whole {
        crate::launch_cache::keep(&state, ghost_core::launch_cache::Part::Workspace, &snapshot);
    }
    Ok(snapshot)
}

#[tauri::command]
#[specta::specta]
pub async fn project_create(
    state: State<'_, AppState>,
    request: ProjectCreate,
) -> Result<ProjectSummary, AppError> {
    Ok(state.sessions()?.project_create(request).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn project_update(
    state: State<'_, AppState>,
    project_id: String,
    input: ProjectInput,
) -> Result<ProjectSummary, AppError> {
    Ok(state.sessions()?.project_update(&project_id, input).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn project_add_folder(
    state: State<'_, AppState>,
    project_id: String,
    path: String,
) -> Result<ProjectFolder, AppError> {
    Ok(state
        .sessions()?
        .project_add_folder(&project_id, &path)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn chat_rename(
    state: State<'_, AppState>,
    chat_id: String,
    title: String,
) -> Result<WorkspaceChat, AppError> {
    Ok(state.sessions()?.chat_rename(&chat_id, &title).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn chat_pin(
    state: State<'_, AppState>,
    chat_id: String,
    pinned: bool,
) -> Result<WorkspaceChat, AppError> {
    Ok(state.sessions()?.chat_pin(&chat_id, pinned).await?)
}

/// The agent, model and thinking level a chat plans or builds with, or none to clear it.
#[tauri::command]
#[specta::specta]
pub async fn chat_set_role(
    state: State<'_, AppState>,
    chat_id: String,
    role: ChatRole,
    choice: Option<RoleModel>,
) -> Result<WorkspaceChat, AppError> {
    Ok(state
        .sessions()?
        .chat_set_role(&chat_id, role, choice)
        .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn chat_archive(
    state: State<'_, AppState>,
    chat_id: String,
    archived: bool,
) -> Result<WorkspaceChat, AppError> {
    Ok(state.sessions()?.chat_archive(&chat_id, archived).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn chat_move(
    state: State<'_, AppState>,
    chat_id: String,
    request: ChatMove,
) -> Result<WorkspaceChat, AppError> {
    Ok(state.sessions()?.chat_move(&chat_id, request).await?)
}

/// What a folder the person picked already is: its projects (compared canonical, so a link or
/// another spelling of the same folder finds them) and whether Ghost manages it as a worktree.
#[tauri::command]
#[specta::specta]
pub async fn workspace_folder_matches(
    state: State<'_, AppState>,
    path: String,
) -> Result<FolderMatches, AppError> {
    Ok(state.sessions()?.workspace_folder_matches(&path).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn chat_create_preview(
    state: State<'_, AppState>,
    folder_id: String,
) -> Result<ChatCreatePreview, AppError> {
    Ok(state.sessions()?.chat_create_preview(&folder_id).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn chat_project_create(
    state: State<'_, AppState>,
    request: ProjectChatCreate,
) -> Result<WorkspaceChat, AppError> {
    Ok(state.sessions()?.chat_project_create(request).await?)
}
