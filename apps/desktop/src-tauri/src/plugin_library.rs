//! Thin Plugin library commands. Rust owns which CLI commands run, the cache and the harness
//! status; the renderer only asks for a snapshot or a refresh.
use ghost_protocol::plugins::PluginLibrary;
use tauri::{State, ipc::Channel};

use crate::{error::AppError, state::AppState};
#[tauri::command]
#[specta::specta]
pub async fn plugin_management_capabilities(
    state: State<'_, AppState>,
) -> Result<Vec<ghost_protocol::plugins::PluginManagement>, AppError> {
    let sessions = state.sessions()?;
    Ok(sessions
        .agents(false)
        .await
        .into_iter()
        .filter_map(|agent| {
            ghost_agents::plugins::management(
                &agent.id,
                agent.installation.as_ref().map(|i| i.version.as_str()),
            )
        })
        .collect())
}

/// One section per agent. `refresh` is the explicit Refresh: agents are probed again and every
/// listing reruns. Otherwise a listing is reused until its agent's installed version changes.
#[tauri::command]
#[specta::specta]
pub async fn plugin_library_snapshot(
    state: State<'_, AppState>,
    refresh: bool,
    channel: Channel<PluginLibrary>,
) -> Result<PluginLibrary, AppError> {
    let sessions = state.sessions()?;
    let generation = sessions.generation();
    Ok(sessions
        .plugin_library_progressive(refresh, |library| {
            generation.is_current() && channel.send(library).is_ok()
        })
        .await?)
}
