//! Which native material sits behind the window, chosen from the OS and its version.
//!
//! | OS | Material | Window |
//! |---|---|---|
//! | macOS 26+ | Liquid Glass (regular), vibrancy listed as Tauri's fallback | transparent |
//! | macOS 13.3 (the oldest the bundle installs on) to 15 | vibrancy (`UnderWindowBackground`, following system appearance) | transparent |
//! | Windows 11 22H2 and later (build 22523+) | Acrylic, the system backdrop (following the window's appearance) | transparent |
//! | Windows 11 (builds 22000 to 22522) | Mica (following system appearance) | transparent |
//! | Windows 10, Linux, anything else | none: the UI paints its opaque frame | opaque |
//! | `GHOST_NO_GLASS=1` | none (the power spike's baseline) | opaque |
//!
//! Opaque is the fallback because a transparent window with no material behind it shows whatever
//! is on the desktop, and on Windows 10 Tauri drops Mica's "unsupported" error silently.
//!
//! Acrylic is see-through glass: the desktop and the windows behind blur through it, which is the
//! Halo look (docs/design/visual-language.md). From build 22523 window-vibrancy applies it as a
//! DWM system backdrop (`DWMSBT_TRANSIENTWINDOW`), composited by DWM like Mica. The resize and
//! drag lag Tauri documents belongs to the older blur-behind path, which builds before 22523 would
//! take; those keep Mica, and the legacy `Blur` effect is never used.
//! See docs/spikes/windows-titlebar.md for the Windows reasoning.

use serde::Serialize;
use tauri::window::Effect;

use crate::os::OsVersion;

/// The first macOS with Liquid Glass (`NSGlassEffectView`).
pub const LIQUID_GLASS_MACOS: u32 = 26;
/// macOS 26 reports itself as 16.0 to binaries linked against an older SDK (the same shim that made
/// Big Sur report 10.16). No real macOS 16 exists, so 16 means 26.
pub const LIQUID_GLASS_MACOS_COMPAT: u32 = 16;
/// The first Windows 11 build; Mica needs it.
pub const MICA_WINDOWS_BUILD: u32 = 22_000;
/// The first build with system backdrops (`DWMWA_SYSTEMBACKDROP_TYPE`), where Acrylic is DWM's.
pub const ACRYLIC_WINDOWS_BUILD: u32 = 22_523;

/// The material the window got, reported to the UI in `app_info`. `none` tells the UI to paint
/// its own opaque background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum GlassEffect {
    LiquidGlass,
    Vibrancy,
    Mica,
    Acrylic,
    None,
}

/// How to build the main window for a [`GlassEffect`].
#[derive(Debug, Clone, PartialEq)]
pub struct GlassPlan {
    pub effect: GlassEffect,
    /// Whether the window (and its webview) are transparent. Only with a material behind them.
    pub transparent: bool,
    /// Passed to Tauri's window effects, in priority order.
    pub effects: Vec<Effect>,
}

impl GlassPlan {
    /// No material, opaque window.
    pub fn opaque() -> GlassPlan {
        GlassPlan {
            effect: GlassEffect::None,
            transparent: false,
            effects: Vec::new(),
        }
    }

    fn with(effect: GlassEffect, effects: Vec<Effect>) -> GlassPlan {
        GlassPlan {
            effect,
            transparent: true,
            effects,
        }
    }

    /// What to try after creating the window with this plan failed: an opaque window, when that
    /// could succeed. Only a transparent plan has a fallback, and only while a webview runtime is
    /// installed (`webview_runtime`, from `tauri::webview_version`). Without one (WebView2 missing
    /// on Windows) the second attempt fails the same way, after Tauri shows its "Could not find
    /// the WebView2 Runtime" dialog a second time.
    pub fn opaque_fallback(&self, webview_runtime: bool) -> Option<GlassPlan> {
        (self.transparent && webview_runtime).then(GlassPlan::opaque)
    }
}

/// Chooses the material for `os` (`std::env::consts::OS`) at `version`. An unknown version gets the
/// conservative choice for its OS (vibrancy on macOS, opaque on Windows), so the material reported
/// to the UI is always one that is really there.
pub fn plan(os: &str, version: Option<OsVersion>, disabled: bool) -> GlassPlan {
    if disabled {
        return GlassPlan::opaque();
    }
    match os {
        "macos" => match version {
            Some(v) if v.major >= LIQUID_GLASS_MACOS || v.major == LIQUID_GLASS_MACOS_COMPAT => {
                // Tauri tries Liquid Glass first and falls back to the next entry if AppKit lacks it,
                // so a wrong version guess still gets a material.
                GlassPlan::with(
                    GlassEffect::LiquidGlass,
                    vec![Effect::LiquidGlassRegular, Effect::UnderWindowBackground],
                )
            }
            _ => GlassPlan::with(GlassEffect::Vibrancy, vec![Effect::UnderWindowBackground]),
        },
        "windows" => match version {
            Some(v) if v.major >= 10 && v.patch >= ACRYLIC_WINDOWS_BUILD => {
                GlassPlan::with(GlassEffect::Acrylic, vec![Effect::Acrylic])
            }
            Some(v) if v.major >= 10 && v.patch >= MICA_WINDOWS_BUILD => {
                GlassPlan::with(GlassEffect::Mica, vec![Effect::Mica])
            }
            _ => GlassPlan::opaque(),
        },
        _ => GlassPlan::opaque(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u32, minor: u32, patch: u32) -> Option<OsVersion> {
        Some(OsVersion::new(major, minor, patch))
    }

    #[test]
    fn macos_26_and_later_get_liquid_glass_with_a_vibrancy_fallback() {
        for version in [v(26, 0, 0), v(26, 6, 2), v(27, 1, 0)] {
            let plan = plan("macos", version, false);
            assert_eq!(plan.effect, GlassEffect::LiquidGlass);
            assert!(plan.transparent);
            assert_eq!(
                plan.effects,
                vec![Effect::LiquidGlassRegular, Effect::UnderWindowBackground]
            );
        }
    }

    #[test]
    fn macos_reporting_the_16_compat_version_is_26() {
        assert_eq!(
            plan("macos", v(16, 0, 0), false).effect,
            GlassEffect::LiquidGlass
        );
    }

    #[test]
    fn older_macos_and_unknown_versions_get_vibrancy() {
        for version in [v(11, 0, 0), v(14, 7, 1), v(15, 6, 0), None] {
            let plan = plan("macos", version, false);
            assert_eq!(plan.effect, GlassEffect::Vibrancy, "{version:?}");
            assert!(plan.transparent);
            assert_eq!(plan.effects, vec![Effect::UnderWindowBackground]);
        }
    }

    #[test]
    fn windows_11_with_system_backdrops_gets_acrylic() {
        for version in [
            v(10, 0, 22523),
            v(10, 0, 22631),
            v(10, 0, 26100),
            v(10, 0, 26300),
        ] {
            let plan = plan("windows", version, false);
            assert_eq!(plan.effect, GlassEffect::Acrylic, "{version:?}");
            assert!(plan.transparent);
            assert_eq!(plan.effects, vec![Effect::Acrylic]);
        }
    }

    #[test]
    fn early_windows_11_keeps_system_appearance_mica() {
        for version in [v(10, 0, 22000), v(10, 0, 22522)] {
            let plan = plan("windows", version, false);
            assert_eq!(plan.effect, GlassEffect::Mica, "{version:?}");
            assert!(plan.transparent);
            assert_eq!(plan.effects, vec![Effect::Mica]);
        }
    }

    #[test]
    fn windows_10_and_unknown_windows_are_opaque() {
        for version in [v(10, 0, 19045), v(10, 0, 21999), v(6, 3, 9600), None] {
            assert_eq!(
                plan("windows", version, false),
                GlassPlan::opaque(),
                "{version:?}"
            );
        }
    }

    #[test]
    fn linux_and_other_systems_are_opaque() {
        for os in ["linux", "freebsd", "openbsd", "android"] {
            assert_eq!(plan(os, v(6, 8, 0), false), GlassPlan::opaque(), "{os}");
        }
    }

    #[test]
    fn disabling_glass_always_gives_an_opaque_window() {
        for (os, version) in [
            ("macos", v(26, 6, 2)),
            ("macos", v(14, 0, 0)),
            ("windows", v(10, 0, 26100)),
            ("linux", None),
        ] {
            let plan = plan(os, version, true);
            assert_eq!(plan, GlassPlan::opaque(), "{os}");
            assert!(!plan.transparent);
            assert!(plan.effects.is_empty());
        }
    }

    #[test]
    fn a_failed_glass_window_retries_opaque_while_a_webview_runtime_exists() {
        for glass in [
            plan("macos", v(26, 6, 2), false),
            plan("macos", v(14, 0, 0), false),
            plan("windows", v(10, 0, 26100), false),
        ] {
            assert!(glass.transparent);
            assert_eq!(glass.opaque_fallback(true), Some(GlassPlan::opaque()));
        }
    }

    #[test]
    fn a_missing_webview_runtime_is_not_retried() {
        // The retry would fail the same way, and in a Windows release build Tauri shows its
        // "Could not find the WebView2 Runtime" dialog on every attempt.
        let glass = plan("windows", v(10, 0, 22631), false);
        assert_eq!(glass.opaque_fallback(false), None);
    }

    #[test]
    fn an_opaque_window_has_nothing_to_fall_back_to() {
        for webview_runtime in [true, false] {
            assert_eq!(GlassPlan::opaque().opaque_fallback(webview_runtime), None);
            assert_eq!(
                plan("linux", None, false).opaque_fallback(webview_runtime),
                None
            );
        }
    }

    #[test]
    fn effect_names_are_snake_case_on_the_wire() {
        assert_eq!(
            serde_json::to_value(GlassEffect::LiquidGlass).expect("serialize"),
            "liquid_glass"
        );
        assert_eq!(
            serde_json::to_value(GlassEffect::Acrylic).expect("serialize"),
            "acrylic"
        );
        assert_eq!(
            serde_json::to_value(GlassEffect::None).expect("serialize"),
            "none"
        );
    }
}
