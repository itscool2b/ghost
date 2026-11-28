//! Settings' General page reaches Rust here: the defaults for new chats, automatic checkpoints,
//! Instructions' worktree copies and Project memory's agent edits go to the session manager, the
//! notification, keep-awake and close-to-tray switches to the native shell, and the automatic
//! update check to the updater. The values live in the profile's setup preferences
//! (`ghost_core::onboarding`).

use ghost_core::onboarding::GeneralPreferences;
use ghost_core::session::NewChatDefaults;
use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// Applies saved General preferences to the running app.
pub fn apply(app: &AppHandle, general: &GeneralPreferences) {
    if let Some(chats) = app.try_state::<AppState>().and_then(|state| state.chats()) {
        apply_to_manager(&chats.manager, general);
    }
    crate::native_shell::apply_preferences(app, general);
    crate::browser::apply_preferences(app, general);
    crate::mobile::apply_preferences(app, general);
    crate::game::apply_preferences(app, general);
    if let Some(updates) = app.try_state::<crate::updater::Updates>() {
        updates.set_automatic(general.automatic_updates);
    }
}

pub(crate) fn apply_to_manager(
    manager: &ghost_core::session::SessionManager,
    general: &GeneralPreferences,
) {
    manager.set_new_chat_defaults(defaults(general));
    manager.set_automatic_checkpoints(general.automatic_checkpoints);
    manager.set_worktree_instruction_copies(general.copy_instructions_to_worktrees);
    manager.set_agent_memory_writes(general.agent_memory_writes);
    manager.set_browser_tools(general.browser);
    manager.set_mobile_tools(general.mobile);
    manager.set_game_tools(general.game);
    manager.set_coordination(general.coordinated_projects);
}

/// Initialize before publishing a runtime, so its first actor sees the saved profile policy.
pub(crate) fn initialize(
    manager: &ghost_core::session::SessionManager,
    service: &crate::onboarding::SetupService,
) {
    let general = match service.preferences() {
        Ok(preferences) => preferences.general,
        Err(error) => {
            tracing::warn!(%error, "settings could not be read; using the defaults");
            GeneralPreferences::default()
        }
    };
    apply_to_manager(manager, &general);
}

/// Applies the active profile's saved preferences, or the defaults when there is no readable
/// profile (a failed startup). Smoke launches keep the defaults.
pub fn refresh(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if state.launch.is_smoke() {
        return;
    }
    if let Some(service) = state.setup_service() {
        match service.apply_current_preferences(|preferences| apply(app, &preferences.general)) {
            Ok(()) => return,
            Err(error) => {
                tracing::warn!(%error, "settings could not be read; using the defaults");
            }
        }
    }
    apply(app, &GeneralPreferences::default());
}

fn defaults(general: &GeneralPreferences) -> NewChatDefaults {
    NewChatDefaults {
        autonomy: general.default_autonomy,
        agent: general.default_agent.clone(),
        model: general.default_model.clone(),
        effort: general.default_effort.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_protocol::Autonomy;

    #[test]
    fn full_auto_stays_the_default_and_a_model_keeps_its_agent() {
        assert_eq!(
            defaults(&GeneralPreferences::default()),
            NewChatDefaults::default()
        );
        assert_eq!(NewChatDefaults::default().autonomy, Autonomy::FullAuto);
        let chosen = defaults(&GeneralPreferences {
            default_agent: Some("codex".into()),
            default_model: Some("gpt-5.5-codex".into()),
            default_effort: Some("high".into()),
            default_autonomy: Autonomy::AutoEdit,
            ..GeneralPreferences::default()
        });
        assert_eq!(chosen.autonomy, Autonomy::AutoEdit);
        assert_eq!(chosen.agent.as_deref(), Some("codex"));
        assert_eq!(chosen.model.as_deref(), Some("gpt-5.5-codex"));
        assert_eq!(chosen.effort.as_deref(), Some("high"));
    }

    #[tokio::test]
    async fn project_memory_switch_reaches_running_sessions_through_the_manager() {
        let dir = tempfile::tempdir().unwrap();
        let mut booted =
            crate::boot::boot(Ok(ghost_core::paths::DataRoot::at(dir.path().join("data"))));
        let chats = crate::sessions::testing::start(&mut booted).unwrap();
        assert!(chats.manager.agent_memory_writes(), "on by default");
        apply_to_manager(
            &chats.manager,
            &GeneralPreferences {
                agent_memory_writes: false,
                ..GeneralPreferences::default()
            },
        );
        assert!(!chats.manager.agent_memory_writes());
        apply_to_manager(&chats.manager, &GeneralPreferences::default());
        assert!(chats.manager.agent_memory_writes());
        chats
            .store
            .shutdown(std::time::Duration::from_secs(5))
            .await;
    }
}
