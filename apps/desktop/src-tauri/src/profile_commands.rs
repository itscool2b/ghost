//! Explicit switches between the profiles of this data folder. Preparation keeps the current
//! runtime usable on failure; publication closes its subscriptions and swaps the whole runtime
//! without merging the two profiles' rows.
use crate::{
    boot,
    error::{AppError, AppErrorCode},
    onboarding::SetupService,
    sessions::{self, ChatRuntime, Outside},
    state::{AppInfo, AppState},
    sync::lock,
};
use ghost_core::{
    paths::DataRoot,
    profile::{Profile, ProfileMeta, Profiles},
};
use std::{sync::Arc, time::Duration};
use tauri::{Emitter, Manager, State};

pub const PROFILE_CHANGED: &str = "profile-changed";
#[cfg(test)]
mod tests;

struct Switching<'a> {
    state: &'a AppState,
    _serial: Option<tokio::sync::OwnedMutexGuard<()>>,
}
impl Drop for Switching<'_> {
    fn drop(&mut self) {
        lock(&self.state.runtime).info.profile_switching = false;
    }
}
fn unavailable(message: &str) -> AppError {
    AppError::new(AppErrorCode::StorageUnavailable, message)
}
fn root(state: &AppState) -> Result<DataRoot, AppError> {
    state
        .data_dir()
        .map(DataRoot::at)
        .ok_or_else(|| unavailable("The local data folder is unavailable."))
}

#[tauri::command]
#[specta::specta]
pub async fn profiles_list(state: State<'_, AppState>) -> Result<Vec<ProfileMeta>, AppError> {
    let profiles = Profiles::new(root(&state)?);
    tokio::task::spawn_blocking(move || profiles.list())
        .await
        .map_err(|_| unavailable("Profiles could not be listed."))?
        .map_err(|_| unavailable("Profiles could not be listed. Existing data was preserved."))
}
#[tauri::command]
#[specta::specta]
pub async fn profile_activate(
    app: tauri::AppHandle,
    profile_id: String,
) -> Result<AppInfo, AppError> {
    run(app, profile_id).await
}

async fn resolve(state: &AppState, id: String) -> Result<Profile, AppError> {
    let profiles = Profiles::new(root(state)?);
    tokio::task::spawn_blocking(move || profiles.get(&id))
        .await
        .map_err(|_| unavailable("The profile could not be read."))?
        .map_err(|_| {
            unavailable("The selected profile is unavailable. Existing data was preserved.")
        })
}

async fn run(app: tauri::AppHandle, profile_id: String) -> Result<AppInfo, AppError> {
    // Completion belongs to the app, not the invoking page. A reload cannot abandon a committed
    // active pointer while leaving the old runtime in memory.
    tokio::spawn(async move {
        let state = app.state::<AppState>();
        let serial = state
            .transition
            .clone()
            .try_lock_owned()
            .map_err(|_| unavailable("Another profile operation is in progress."))?;
        let selected = resolve(&state, profile_id).await?;
        let mutation = app
            .state::<Arc<crate::config_library::LibraryService>>()
            .mutation
            .clone();
        let result =
            activate_serial(&state, selected, Outside::real, mutation, Some(&serial)).await?;
        crate::memory_commands::forward_changes(&app);
        crate::coordinator_commands::forward_changes(&app);
        if let Some(library) = app.try_state::<Arc<crate::config_library::LibraryService>>() {
            library.reset_profile();
        }
        if let Some(vendor) = app.try_state::<Arc<crate::config_vendor::VendorService>>() {
            vendor.reset_profile();
        }
        if let Some(instructions) =
            app.try_state::<Arc<crate::instructions_commands::InstructionsService>>()
        {
            instructions.reset_profile();
        }
        if let Some(clones) = app.try_state::<Arc<ghost_core::cloning::Clones>>() {
            clones.cancel_all();
        }
        crate::native_shell::profile_changed(&app);
        crate::remote::profile_changed(&app);
        crate::general_preferences::refresh(&app);
        crate::appearance::refresh(&app);
        let _ = app.emit(PROFILE_CHANGED, &result);
        Ok(result)
    })
    .await
    .map_err(|_| {
        unavailable("The profile switch could not finish. Restart Ghost before retrying.")
    })?
}

type OutsideFactory = fn(
    &Arc<ghost_process::LeftoverRegistry>,
    &Arc<ghost_process::Redactor>,
    &std::path::Path,
    &tokio::runtime::Handle,
) -> Outside;

#[cfg(test)]
async fn activate(
    state: &AppState,
    selected: Profile,
    outside: OutsideFactory,
) -> Result<AppInfo, AppError> {
    activate_serial(state, selected, outside, Arc::default(), None).await
}

async fn activate_serial(
    state: &AppState,
    selected: Profile,
    outside: OutsideFactory,
    mutation: Arc<tokio::sync::Mutex<()>>,
    serial: Option<&tokio::sync::OwnedMutexGuard<()>>,
) -> Result<AppInfo, AppError> {
    let serial = match serial {
        Some(_) => None,
        None => Some(
            state
                .transition
                .clone()
                .try_lock_owned()
                .map_err(|_| unavailable("Another profile transition is in progress."))?,
        ),
    };
    if state.quit.is_quitting() {
        return Err(AppError::new(
            AppErrorCode::ShuttingDown,
            "Ghost is closing.",
        ));
    }
    {
        let mut runtime = lock(&state.runtime);
        if runtime.info.profile.is_none() {
            return Err(unavailable("The current profile is unavailable."));
        }
        if runtime.info.profile.as_ref().map(|profile| &profile.id) == Some(&selected.meta.id)
            && runtime.chats.is_some()
        {
            return Ok(runtime.info.clone());
        }
        runtime.info.profile_switching = true;
    }
    let _switching = Switching {
        state,
        _serial: serial,
    };
    let _plugin_refresh =
        tokio::time::timeout(Duration::from_secs(5), state.wait_profile_commands())
            .await
            .map_err(|_| {
                unavailable("Wait for current profile commands to finish, then retry switching.")
            })?;
    let _mutation = mutation.lock_owned().await;
    // An open terminal runs the person's own process (a shell, a terminal chat, a setup command,
    // a sign-in or an install), which a switch never ends for them: it asks first, before
    // stopping anything.
    let _terminals = state.terminals.try_quiesce_when_idle().ok_or_else(|| {
        unavailable(
            "Close every open terminal (shells, terminal chats, setup commands and sign-ins) before switching profiles.",
        )
    })?;
    // Switching closes this profile: its chats' agents stop here, so the switch never asks the
    // person to stop them. Each chat resumes its session when this profile opens again.
    if let Some(chats) = state.chats() {
        let report = chats
            .manager
            .stop_all(ghost_core::session::StopScope::All, Duration::from_secs(30))
            .await;
        // A chat still on its way out is named, not mistaken for a Git change or an update.
        if !report.stopping.is_empty() {
            let names = chats.manager.chat_names(report.stopping).await;
            return Err(unavailable(&ghost_core::session::still_stopping(
                &names,
                "Switch profiles again in a moment.",
            )));
        }
    }
    activate_one(state, selected, outside).await
}

async fn activate_one(
    state: &AppState,
    selected: Profile,
    outside: OutsideFactory,
) -> Result<AppInfo, AppError> {
    let from_profile = state
        .app_info()
        .profile
        .map(|profile| profile.id)
        .ok_or_else(|| unavailable("The current profile is unavailable."))?;
    let old = state.chats();
    let _sessions = match &old {
        Some(old) => Some(old.manager.try_quiesce_when_idle().ok_or_else(|| {
            unavailable(
                "Wait for Git changes and agent updates to finish, then switch profiles again.",
            )
        })?),
        None => None,
    };
    if let Some(old) = &old {
        if !matches!(
            old.manager.storage_status().state,
            ghost_protocol::session::StorageState::Ok
        ) {
            return Err(unavailable(
                "Resolve the current profile's storage error before switching.",
            ));
        }
        old.store.flush(Duration::from_secs(5)).await.map_err(|_| {
            unavailable(
                "The current profile could not confirm its pending writes. Nothing was switched.",
            )
        })?;
    }
    let data_root = root(state)?;
    let profile_id = selected.meta.id.clone();
    let rt = tokio::runtime::Handle::current();
    let version = state.app_info().app_version;
    let reuse = old.as_ref().map(|old| old.services.clone());
    let (mut prepared, onboarding, next) = tokio::task::spawn_blocking(move || {
        let mut prepared = boot::boot_profile(data_root, selected.clone());
        if let Some(message) = &prepared.fatal {
            return Err(unavailable(message));
        }
        let onboarding = Arc::new(SetupService::new(selected.dir.clone()));
        let next = sessions::start_profile_chats(&mut prepared, &version, &rt, outside, reuse)
            .ok_or_else(|| {
                unavailable(
                    prepared
                        .fatal
                        .as_deref()
                        .unwrap_or("The selected profile could not start."),
                )
            })?;
        Ok::<_, AppError>((prepared, onboarding, Arc::new(next)))
    })
    .await
    .map_err(|_| {
        unavailable("The selected profile could not be prepared. Nothing was switched.")
    })??;
    let recovered = tokio::time::timeout(Duration::from_secs(5), next.store.recovered()).await;
    if recovered.is_err()
        || next.store.flush(Duration::from_secs(5)).await.is_err()
        || state.quit.is_quitting()
    {
        discard(&next).await;
        return Err(unavailable(
            "The selected profile could not confirm its storage, or Ghost is closing. Nothing was switched.",
        ));
    }
    let profiles = Profiles::new(root(state)?);
    let target = profile_id.clone();
    let (published, saved) = tokio::task::spawn_blocking(move || {
        let published = profiles.set_active(&target);
        // Read back what is on disk: a write that reported an error may still have landed.
        (published.is_ok(), profiles.saved_active())
    })
    .await
    .unwrap_or((false, None));
    if !published && saved.as_deref() != Some(profile_id.as_str()) {
        discard(&next).await;
        return Err(unavailable(
            if saved.as_deref() == Some(from_profile.as_str()) {
                "The profile choice could not be saved. Nothing was switched."
            } else {
                "The active profile could not be confirmed. Existing data was preserved."
            },
        ));
    }
    // No fallible preparation remains after publication. Old data was flushed before this point;
    // a close failure is reported honestly while retaining its WAL/recovery files.
    state.integrity.close();
    if let Some(old) = &old {
        for owner in state.pages.invalidate_all() {
            old.manager.drop_owner(&owner);
        }
        old.manager.shutdown_all(Duration::from_secs(5)).await;
        if !old.store.shutdown(Duration::from_secs(5)).await {
            prepared.notices.push("The previous profile did not confirm a clean close. Its saved data and recovery files remain available.".into());
        }
    }
    state.install_profile(prepared, next, onboarding);
    let mut info = state.app_info();
    info.profile_switching = false;
    Ok(info)
}
async fn discard(runtime: &ChatRuntime) {
    runtime.manager.shutdown_all(Duration::from_secs(2)).await;
    runtime.store.shutdown(Duration::from_secs(5)).await;
}
