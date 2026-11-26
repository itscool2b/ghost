//! The active profile controls native menus and window materials as well as the web UI.

use ghost_core::onboarding::AppearancePreference;
use tauri::{AppHandle, Manager, Theme};

use crate::state::AppState;

pub fn native_theme(preference: AppearancePreference) -> Option<Theme> {
    match preference {
        AppearancePreference::System => None,
        AppearancePreference::Light => Some(Theme::Light),
        AppearancePreference::Dark => Some(Theme::Dark),
    }
}

pub fn apply(app: &AppHandle, preference: AppearancePreference) {
    let theme = native_theme(preference);
    app.set_theme(theme);
    for window in app.webview_windows().values() {
        if let Err(error) = window.set_theme(theme) {
            tracing::warn!(%error, "native appearance could not be updated");
        }
        if let Some(resolved) = theme.or_else(|| window.theme().ok()) {
            repaint(app, window, resolved);
        }
    }
}

/// Locked and unavailable profiles use System without reading another profile's preference.
pub fn refresh(app: &AppHandle) {
    let preference = app
        .try_state::<AppState>()
        .and_then(|state| state.setup_service())
        .and_then(|service| service.preferences().ok())
        .map(|preferences| preferences.appearance)
        .unwrap_or_default();
    apply(app, preference);
}

/// ThemeChanged reports the effective theme, including automatic OS changes in System mode.
pub fn repaint(app: &AppHandle, window: &tauri::WebviewWindow, theme: Theme) {
    let opaque = app
        .try_state::<AppState>()
        .is_some_and(|state| state.app_info().glass == crate::glass::GlassEffect::None);
    crate::window::paint_appearance(window, theme, opaque);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_removes_the_native_override() {
        assert_eq!(native_theme(AppearancePreference::System), None);
        assert_eq!(
            native_theme(AppearancePreference::Light),
            Some(Theme::Light)
        );
        assert_eq!(native_theme(AppearancePreference::Dark), Some(Theme::Dark));
    }
}
