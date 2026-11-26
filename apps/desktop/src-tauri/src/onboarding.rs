//! Setup commands touch only Ghost's profile preferences. Discovery and explicit vendor
//! maintenance keep their existing, separately reviewed command paths.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ghost_core::onboarding::{
    OnboardingStore, SetupAction, SetupError, SetupPreferences, SetupSnapshot,
};
use tauri::{AppHandle, State};

use crate::error::{AppError, AppErrorCode};
use crate::state::{AppState, ProfileLease};

pub struct SetupService {
    profile_dir: PathBuf,
    store: Mutex<Option<OnboardingStore>>,
}

impl SetupService {
    pub fn new(profile_dir: PathBuf) -> Self {
        Self {
            profile_dir,
            store: Mutex::new(None),
        }
    }

    fn run(&self, change: Option<(u32, SetupAction)>) -> Result<SetupSnapshot, AppError> {
        self.with_store(|store| {
            match change {
                Some((revision, action)) => store.apply(revision, action),
                None => store.snapshot(),
            }
            .map_err(map_error)
        })
    }

    /// The saved preferences, without marking the boot seen (that is the interface's to do).
    pub fn preferences(&self) -> Result<SetupPreferences, AppError> {
        self.with_store(|store| Ok(store.preferences()))
    }

    /// Serialize applying runtime settings with writes, and always use the latest saved values.
    /// Concurrent command replies may resume out of order after their blocking file work.
    pub(crate) fn apply_current_preferences(
        &self,
        apply: impl FnOnce(&SetupPreferences),
    ) -> Result<(), AppError> {
        self.with_store(|store| {
            apply(&store.preferences());
            Ok(())
        })
    }

    fn with_store<T>(
        &self,
        run: impl FnOnce(&OnboardingStore) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut store = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if store.is_none() {
            *store = Some(OnboardingStore::open(&self.profile_dir).map_err(map_error)?);
        }
        let store = store
            .as_ref()
            .ok_or_else(|| AppError::new(AppErrorCode::Internal, "Setup is unavailable."))?;
        run(store)
    }
}

fn map_error(error: SetupError) -> AppError {
    let code = match error {
        SetupError::Invalid(_) | SetupError::Stale => AppErrorCode::InvalidArgument,
        SetupError::NewerVersion | SetupError::Storage(_) => AppErrorCode::StorageUnavailable,
    };
    AppError::new(code, error.to_string())
}

async fn run(
    state: &AppState,
    change: Option<(u32, SetupAction)>,
) -> Result<(SetupSnapshot, ProfileLease<Arc<SetupService>>), AppError> {
    let service = state.onboarding()?;
    tauri::async_runtime::spawn_blocking(move || {
        let snapshot = service.run(change)?;
        // A profile switch must wait through applying these preferences to the native runtime,
        // not just until the file operation finishes.
        Ok((snapshot, service))
    })
    .await
    .map_err(|_| AppError::new(AppErrorCode::Internal, "Setup could not be loaded."))?
}

#[tauri::command]
#[specta::specta]
pub async fn setup_snapshot(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SetupSnapshot, AppError> {
    let (snapshot, lease) = run(&state, None).await?;
    lease.apply_current_preferences(|preferences| {
        crate::general_preferences::apply(&app, &preferences.general);
    })?;
    Ok(snapshot)
}

#[tauri::command]
#[specta::specta]
pub async fn setup_apply(
    app: AppHandle,
    state: State<'_, AppState>,
    revision: u32,
    action: SetupAction,
) -> Result<SetupSnapshot, AppError> {
    let appearance_changed = matches!(&action, SetupAction::Appearance { .. });
    let (snapshot, lease) = run(&state, Some((revision, action))).await?;
    // Settings' switches take effect at once: new chats, notifications, keep-awake, closing.
    lease.apply_current_preferences(|preferences| {
        crate::general_preferences::apply(&app, &preferences.general);
        if appearance_changed {
            crate::appearance::apply(&app, preferences.appearance);
        }
    })?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn setup_result_keeps_profile_switch_waiting_until_runtime_preferences_are_applied() {
        let dir = tempfile::tempdir().unwrap();
        let mut booted =
            crate::boot::boot(Ok(ghost_core::paths::DataRoot::at(dir.path().join("data"))));
        let chats = crate::sessions::testing::start(&mut booted);
        let state = AppState::new(
            booted,
            chats,
            &crate::os::OsInfo {
                os: "windows",
                version: None,
                display: "test".into(),
            },
            crate::glass::GlassEffect::None,
            crate::launch::LaunchOptions::from_lookup(|_| None),
            "0.1.0".into(),
        );
        let (older, older_lease) = run(&state, None).await.unwrap();
        assert!(!older.preferences.general.automatic_checkpoints);
        let (_, lease) = run(
            &state,
            Some((0, SetupAction::AutomaticCheckpoints { enabled: true })),
        )
        .await
        .unwrap();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(10),
                state.wait_profile_commands()
            )
            .await
            .is_err()
        );
        let manager = state.chats().unwrap().manager.clone();
        lease
            .apply_current_preferences(|preferences| {
                crate::general_preferences::apply_to_manager(&manager, &preferences.general);
            })
            .unwrap();
        // A snapshot command that read before the toggle may finish after it. It must not
        // restore the older policy when updating the runtime.
        older_lease
            .apply_current_preferences(|preferences| {
                crate::general_preferences::apply_to_manager(&manager, &preferences.general);
            })
            .unwrap();
        assert!(manager.automatic_checkpoints());
        drop(older_lease);
        drop(lease);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            state.wait_profile_commands(),
        )
        .await
        .unwrap();
        state.at_exit();
    }
}
