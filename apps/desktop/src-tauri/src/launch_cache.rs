//! The sidebar's last lists, for the window's first frame (`o7_core::launch_cache`).
//!
//! `chats_list` and the unfiltered `workspace_snapshot` keep what they answered; the page reads
//! both back before it first renders, shows them at once, and replaces them when the database
//! answers. Only the open profile's lists are ever read or kept: both go through the same access
//! check as the chat commands, and the files live in that profile's folder.

use std::path::PathBuf;

use o7_core::launch_cache::{self, Part};
use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::error::AppError;
use crate::state::AppState;

/// What the page starts from: each list as the JSON text kept for it, or `null` when none is kept.
/// The page checks each text's version and shape itself, and ignores what it does not know.
#[derive(Debug, Clone, Default, Serialize, Type)]
pub struct LaunchCache {
    pub chats: Option<String>,
    pub workspace: Option<String>,
}

/// The open profile's folder, only while the chat commands would answer for it.
fn profile_dir(state: &AppState) -> Result<Option<PathBuf>, AppError> {
    // The lease is the access check, and holds a profile switch off until this returns.
    let _lease = state.sessions()?;
    Ok(state.chats().map(|chats| chats.profile_dir.clone()))
}

/// The lists the sidebar last showed in this profile. It reads two small files and never the
/// database, so it answers while the database is still busy with startup.
#[tauri::command]
#[specta::specta]
pub async fn launch_cache(state: State<'_, AppState>) -> Result<LaunchCache, AppError> {
    let Some(dir) = profile_dir(&state)? else {
        return Ok(LaunchCache::default());
    };
    Ok(tauri::async_runtime::spawn_blocking(move || LaunchCache {
        chats: launch_cache::read(&dir, Part::Chats),
        workspace: launch_cache::read(&dir, Part::Workspace),
    })
    .await
    .unwrap_or_default())
}

/// Keeps `list` as `part` of the open profile, on a blocking thread and without holding the
/// command that answered it. A list that cannot be kept only costs the next launch its first
/// frame, so a failure is logged and nothing more.
pub fn keep<T: Serialize>(state: &AppState, part: Part, list: &T) {
    let Ok(Some(dir)) = profile_dir(state) else {
        return;
    };
    let Ok(data) = serde_json::to_vec(list) else {
        return;
    };
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = launch_cache::save(&dir, part, &data) {
            tracing::warn!(%error, "could not keep the sidebar for the next launch");
        }
    });
}
