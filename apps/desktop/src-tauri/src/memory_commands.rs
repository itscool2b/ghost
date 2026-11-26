//! Memory writes retain the active-profile lease until the writer acknowledges the exact revision.
use crate::{error::AppError, state::AppState};
use ghost_protocol::memory::*;
use tauri::{Emitter, Manager, State};

/// Sent with a [`ProjectMemoryChanged`] after every committed change to a project's memory,
/// history or proposals, whether the editor or an agent made it. An agent's direct edit is named,
/// so the window can say so and offer to undo it.
pub const MEMORY_CHANGED: &str = "auracode-project-memory-changed";

/// Forwards the active profile's memory changes to the window, so an open Project memory page
/// reflects an agent's edit. Call once a runtime is published; the task ends with its store.
pub(crate) fn forward_changes(app: &tauri::AppHandle) {
    let Some(chats) = app.try_state::<AppState>().and_then(|state| state.chats()) else {
        return;
    };
    let changes = chats.store.memory_changes();
    let app = app.clone();
    tauri::async_runtime::spawn(forward(changes, move |changed| {
        let _ = app.emit(MEMORY_CHANGED, changed);
    }));
}

/// Sends every change on, until the store closes. A receiver that fell behind sends one change
/// without a project, so every open editor reads again.
async fn forward(
    mut changes: tokio::sync::broadcast::Receiver<ProjectMemoryChanged>,
    send: impl Fn(ProjectMemoryChanged),
) {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match changes.recv().await {
            Ok(changed) => send(changed),
            Err(RecvError::Lagged(_)) => send(ProjectMemoryChanged {
                project_id: None,
                agent_edit: None,
            }),
            Err(RecvError::Closed) => break,
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn read_project_memory(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ProjectMemory, AppError> {
    Ok(state.sessions()?.read_project_memory(&project_id).await?)
}
#[tauri::command]
#[specta::specta]
pub async fn save_project_memory(
    state: State<'_, AppState>,
    input: SaveProjectMemory,
) -> Result<ProjectMemory, AppError> {
    Ok(state.sessions()?.save_project_memory(input).await?)
}
#[tauri::command]
#[specta::specta]
pub async fn propose_project_memory(
    state: State<'_, AppState>,
    input: ProposeProjectMemory,
) -> Result<MemoryProposal, AppError> {
    Ok(state.sessions()?.propose_project_memory(input).await?)
}
#[tauri::command]
#[specta::specta]
pub async fn accept_memory_proposal(
    state: State<'_, AppState>,
    input: AcceptMemoryProposal,
) -> Result<ProjectMemory, AppError> {
    Ok(state.sessions()?.accept_memory_proposal(input).await?)
}
#[tauri::command]
#[specta::specta]
pub async fn reject_memory_proposal(
    state: State<'_, AppState>,
    project_id: String,
    proposal_id: String,
) -> Result<Vec<MemoryProposal>, AppError> {
    Ok(state
        .sessions()?
        .reject_memory_proposal(&project_id, &proposal_id)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn project_memory_history(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<MemoryRevision>, AppError> {
    Ok(state
        .sessions()?
        .project_memory_history(&project_id)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn project_memory_proposals(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<MemoryProposal>, AppError> {
    Ok(state
        .sessions()?
        .project_memory_proposals(&project_id)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn export_project_memory(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<ProjectMemoryBundle, AppError> {
    Ok(state.sessions()?.export_project_memory(&project_id).await?)
}
#[tauri::command]
#[specta::specta]
pub async fn import_project_memory(
    state: State<'_, AppState>,
    input: ImportProjectMemory,
) -> Result<ProjectMemory, AppError> {
    Ok(state.sessions()?.import_project_memory(input).await?)
}
/// One note of a project's memory, with its text. The page lists notes from
/// `read_project_memory` and reads one when it is opened.
#[tauri::command]
#[specta::specta]
pub async fn read_memory_note(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
) -> Result<MemoryNote, AppError> {
    Ok(state
        .sessions()?
        .read_memory_note(&project_id, &name)
        .await?)
}
#[tauri::command]
#[specta::specta]
pub async fn save_memory_note(
    state: State<'_, AppState>,
    input: SaveMemoryNote,
) -> Result<MemoryNote, AppError> {
    Ok(state.sessions()?.save_memory_note(input).await?)
}
#[tauri::command]
#[specta::specta]
pub async fn delete_memory_note(
    state: State<'_, AppState>,
    input: DeleteMemoryNote,
) -> Result<ProjectMemory, AppError> {
    Ok(state.sessions()?.delete_memory_note(input).await?)
}
/// Takes back a run of edits an agent or a linked bot made to the brief or to one note: the
/// notice's Undo. `first` is the earliest edit of the run and `last` the latest. Rust decides
/// whether they can still be undone, so a later edit is never taken with them.
#[tauri::command]
#[specta::specta]
pub async fn undo_memory_edit(
    state: State<'_, AppState>,
    project_id: String,
    first: String,
    last: String,
) -> Result<ProjectMemory, AppError> {
    Ok(state
        .sessions()?
        .undo_memory_edit(&project_id, &first, &last)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn forwarding_names_agent_edits_rereads_after_a_lag_and_ends_with_the_store() {
        let (sender, changes) = tokio::sync::broadcast::channel(2);
        let edit = |revision: &str| ProjectMemoryChanged {
            project_id: Some("project".into()),
            agent_edit: Some(AgentMemoryEdit {
                revision: revision.into(),
                parent_revision: None,
                chat_id: "chat".into(),
                note: None,
                deleted: false,
            }),
        };
        for revision in ["one", "two", "three"] {
            sender.send(edit(revision)).unwrap();
        }
        drop(sender);
        let sent = std::sync::Mutex::new(Vec::new());
        forward(changes, |changed| sent.lock().unwrap().push(changed)).await;
        assert_eq!(
            sent.into_inner().unwrap(),
            [
                ProjectMemoryChanged {
                    project_id: None,
                    agent_edit: None
                },
                edit("two"),
                edit("three"),
            ]
        );
    }
}
