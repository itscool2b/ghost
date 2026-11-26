//! The main window: created hidden with the chosen glass, shown once the UI has painted.
//!
//! The window is built in Rust (`"create": false` in tauri.conf.json) because its material and
//! transparency depend on the OS version, which config alone cannot express. It starts hidden so
//! the first thing on screen is a painted UI rather than an empty transparent frame; the page calls
//! `window_ready` after its first paint, and a fallback shows the window after
//! [`READY_FALLBACK`] anyway, so a broken frontend can never leave an invisible app running.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::webview::{PageLoadEvent, WebviewWindowBuilder};
use tauri::window::Color;
use tauri::{AppHandle, Manager, Url, WebviewWindow};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_window_state::StateFlags;

use crate::error::{AppError, AppErrorCode};
use crate::glass::GlassPlan;
use crate::state::AppState;
#[cfg(any(windows, test))]
use crate::sync::lock;

/// Last successful WebView2 visibility change. Window visibility alone does not update the
/// controller's IsVisible flag. Failed transitions remain retryable on the next native event.
#[derive(Default)]
#[cfg(any(windows, test))]
pub struct WebviewVisibility(std::sync::Mutex<Option<bool>>);

#[cfg(any(windows, test))]
impl WebviewVisibility {
    fn apply<E>(&self, visible: bool, set: impl FnOnce(bool) -> Result<(), E>) -> Result<(), E> {
        let mut current = lock(&self.0);
        if *current != Some(visible) {
            set(visible)?;
            *current = Some(visible);
        }
        Ok(())
    }
}

/// Keep native window controls and the taskbar alive while hiding only the rendering webview.
/// Session actors, persistence and notifications run in Rust and do not depend on webview timers.
pub fn sync_webview_visibility(app: &AppHandle) {
    #[cfg(windows)]
    {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        if state.launch.is_smoke() || !state.window_shown.is_claimed() {
            return;
        }
        let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
            return;
        };
        let visible = match (window.is_visible(), window.is_minimized()) {
            (Ok(visible), Ok(minimized)) => visible && !minimized,
            _ => return,
        };
        set_webview_visible(app, visible);
    }
    #[cfg(not(windows))]
    let _ = app;
}

pub fn set_webview_visible(app: &AppHandle, visible: bool) {
    #[cfg(windows)]
    {
        if app
            .try_state::<AppState>()
            .is_some_and(|state| state.launch.is_smoke())
        {
            return;
        }
        let (Some(window), Some(state)) = (
            app.get_webview_window(MAIN_WINDOW),
            app.try_state::<WebviewVisibility>(),
        ) else {
            return;
        };
        let view: &tauri::Webview = window.as_ref();
        if let Err(error) = state.apply(
            visible,
            |visible| if visible { view.show() } else { view.hide() },
        ) {
            tracing::warn!(%error, visible, "could not update webview visibility");
        }
        // The browser's page over the panel follows the window.
        crate::browser::window_shown(app, visible);
    }
    #[cfg(not(windows))]
    let _ = (app, visible);
}

/// Label of the one window M0 has.
pub const MAIN_WINDOW: &str = "main";

/// The main window's title in a development build, so it is never taken for the installed app's
/// window (`Ghost ADE`, from tauri.conf.json) by a person, Alt+Tab, or automation that finds
/// windows by title.
pub const DEVELOPMENT_TITLE: &str = "Ghost dev";

/// The main window's title: tauri.conf.json's for an installed build, [`DEVELOPMENT_TITLE`] for a
/// development one.
pub fn main_title(configured: &str, development: bool) -> &str {
    if development {
        DEVELOPMENT_TITLE
    } else {
        configured
    }
}

/// How long the page gets to report its first paint before the window is shown regardless.
pub const READY_FALLBACK: Duration = Duration::from_secs(3);

/// The light `--g-window`, behind opaque windows (and the webview before it paints). The panes sit
/// on this window colour, so the frame and the page start as one surface.
pub const BASE_COLOR: Color = Color(245, 245, 245, 255);

/// The dark `--g-window`, for opaque windows in the dark appearance.
const BASE_COLOR_DARK: Color = Color(14, 14, 14, 255);

/// What the window-state plugin restores: size, position, maximized, fullscreen. Never visibility
/// (that would show the window before first paint) and never decorations (code owns the chrome).
pub fn restored_state() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN
}

/// Shows the window at most once: the first of "page ready" and "fallback timer" wins.
#[derive(Debug, Default)]
pub struct ShowOnce(AtomicBool);

impl ShowOnce {
    /// True for the first caller only.
    pub fn claim(&self) -> bool {
        !self.0.swap(true, Ordering::AcqRel)
    }

    /// Gives the claim back after showing failed, so a later attempt can retry.
    pub fn release(&self) {
        self.0.store(false, Ordering::Release);
    }

    pub fn is_claimed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Builds the main window from its config entry with `plan`'s glass, hidden.
///
/// `keep_running_hidden` (smoke runs) keeps the page running at full speed while the window is
/// hidden, covered, or behind a locked screen. WebKit otherwise slows a hidden page's timers to one
/// a second and suspends the page after about five minutes, and a smoke run reads the DOM, not the
/// pixels, so it has no reason to wait for the screen. Only macOS 14 and later have the switch;
/// WebKitGTK and WebView2 offer none, and their smoke runs show the window.
///
/// `webview_data` moves the webview profile ([`crate::launch::LaunchOptions::webview_data_dir`]).
pub fn create_main(
    app: &tauri::App,
    plan: &GlassPlan,
    keep_running_hidden: bool,
    webview_data: Option<&Path>,
) -> Result<WebviewWindow, Box<dyn std::error::Error>> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN_WINDOW)
        .cloned()
        .ok_or("tauri.conf.json has no window labelled \"main\"")?;

    // In development the page comes from the Vite dev server; in a build it is served by Tauri.
    let dev_url = if tauri::is_dev() {
        app.config().build.dev_url.clone()
    } else {
        None
    };
    let origin = AppOrigin::current(config.use_https_scheme);
    let handle = app.handle().clone();

    let title = main_title(&config.title, ghost_core::paths::DEVELOPMENT_BUILD).to_owned();
    let mut builder = WebviewWindowBuilder::from_config(app.handle(), &config)?
        .title(title)
        .theme(None)
        .visible(false)
        .transparent(plan.transparent)
        .on_navigation(move |url| {
            let allowed = is_app_url(url, origin, dev_url.as_ref());
            if !allowed {
                leave_for_browser(&handle, url);
            }
            allowed
        })
        .on_page_load(|window, payload| {
            // A reload or navigation starts a new page whose channels are not the old page's: stop
            // the old page's streams instead of streaming into nothing until they finish.
            if payload.event() == PageLoadEvent::Started
                && let Some(state) = window.try_state::<AppState>()
            {
                let stopped = state.demos.cancel_all();
                if stopped > 0 {
                    tracing::debug!(stopped, "page reloaded; cancelled its demo streams");
                }
            }
        });

    if plan.transparent {
        if !plan.effects.is_empty() {
            builder = builder.effects(tauri::utils::config::WindowEffectsConfig {
                effects: plan.effects.clone(),
                ..Default::default()
            });
        }
    } else {
        builder = builder.background_color(BASE_COLOR);
    }

    if keep_running_hidden {
        builder = builder.background_throttling(BackgroundThrottlingPolicy::Disabled);
    }
    if let Some(folder) = webview_data {
        builder = builder.data_directory(folder.to_path_buf());
    }

    // Avoids the white flash a transparent window shows on Windows before its first paint.
    #[cfg(windows)]
    {
        builder = builder.no_redirection_bitmap(plan.transparent);
    }

    #[cfg(all(windows, debug_assertions))]
    if let Some(arguments) = automation_browser_args() {
        builder = builder.additional_browser_args(&arguments);
    }

    // Tells the page when WebKitGTK will paint it on the CPU, where large soft shadows are costly.
    #[cfg(target_os = "linux")]
    if paints_on_cpu(|name| std::env::var_os(name)) {
        builder = builder.initialization_script(CPU_PAINT_SCRIPT);
    }

    let window = builder.build()?;
    if let Ok(theme) = window.theme() {
        paint_appearance(&window, theme, !plan.transparent);
    }
    Ok(window)
}

/// What the page reads to learn it is painted on the CPU (`paintsOnCpu` in `src/lib/paint.ts`).
#[cfg(target_os = "linux")]
const CPU_PAINT_SCRIPT: &str = "window.__AURA_PAINT__ = \"cpu\";";

/// Whether the environment makes WebKitGTK paint on the CPU instead of the GPU: its GPU renderer
/// or compositing is switched off, or Mesa is told to render in software. On that path the web
/// process repaints a pane's soft shadow with every change inside the pane and each frame is
/// copied, so the page draws its panes more cheaply (`ui-tokens.css`). A value of `0` leaves the
/// switch off, as WebKitGTK and Mesa read it. `get` reads one variable.
#[cfg(any(target_os = "linux", test))]
fn paints_on_cpu(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    [
        "WEBKIT_DISABLE_DMABUF_RENDERER",
        "WEBKIT_DISABLE_COMPOSITING_MODE",
        "LIBGL_ALWAYS_SOFTWARE",
    ]
    .iter()
    .any(|name| {
        get(name).is_some_and(|value| !value.is_empty() && value != "0" && value != "false")
    })
}

/// The window colour (`--g-window`) and secondary text as Windows COLORREFs (0x00BBGGRR).
#[cfg(any(windows, test))]
fn caption_colors(theme: tauri::Theme) -> [u32; 2] {
    match theme {
        tauri::Theme::Dark => [0x000e_0e0e, 0x00ab_abab],
        _ => [0x00f5_f5f5, 0x0050_5050],
    }
}

/// Repaint native chrome and opaque fallback windows without altering glass or window geometry.
pub fn paint_appearance(window: &WebviewWindow, theme: tauri::Theme, opaque: bool) {
    if opaque {
        let color = if theme == tauri::Theme::Dark {
            BASE_COLOR_DARK
        } else {
            BASE_COLOR
        };
        if let Err(error) = window.set_background_color(Some(color)) {
            tracing::debug!(%error, "the window background could not follow appearance");
        }
    }
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        caption::paint(hwnd.0, caption_paint(theme, opaque));
    }
}

/// The caption's colours: solid on an opaque window, to match the page painted under it; on a
/// glass window the system's own, so the window's material runs up through the title bar and the
/// two read as one sheet of glass.
#[cfg(any(windows, test))]
fn caption_paint(theme: tauri::Theme, opaque: bool) -> [u32; 2] {
    if opaque {
        caption_colors(theme)
    } else {
        [SYSTEM_COLOR; 2]
    }
}

/// `DWMWA_COLOR_DEFAULT`: hands a caption colour back to the system.
#[cfg(any(windows, test))]
const SYSTEM_COLOR: u32 = 0xFFFF_FFFF;

/// Windows keeps its native title bar and caption buttons, including Snap Layouts. DWM paints
/// the caption to match the resolved app appearance, or leaves it to the window's material.
/// Windows 10 ignores both attributes and keeps its own caption.
#[cfg(windows)]
#[allow(unsafe_code)]
mod caption {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DwmSetWindowAttribute,
    };

    pub fn paint(hwnd: *mut core::ffi::c_void, [caption, text]: [u32; 2]) {
        for (attribute, color) in [(DWMWA_CAPTION_COLOR, caption), (DWMWA_TEXT_COLOR, text)] {
            // SAFETY: `hwnd` is this process's live main window, and DWM copies the 4-byte value
            // before returning; an older Windows answers with an error and nothing changes.
            let result = unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    attribute as u32,
                    (&raw const color).cast(),
                    size_of::<u32>() as u32,
                )
            };
            if result < 0 {
                tracing::debug!(
                    result,
                    attribute,
                    "the window caption keeps its system colour"
                );
            }
        }
    }
}

/// The switches wry gives every WebView2 environment unless the app sets its own.
#[cfg(all(windows, debug_assertions))]
const WRY_BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required";

/// Edge WebDriver enables remote debugging through `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, but
/// WebView2 ignores that variable once an app passes its own arguments, and wry always does. Only
/// debug builds launched for WebDriver automation keep wry's switches and add the driver's, so the
/// external Windows gate can attach. Release builds never read the variable.
#[cfg(all(windows, debug_assertions))]
fn automation_browser_args() -> Option<String> {
    automation_args(
        std::env::var("TAURI_WEBVIEW_AUTOMATION").ok().as_deref(),
        std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS")
            .ok()
            .as_deref(),
    )
}

#[cfg(all(windows, debug_assertions))]
fn automation_args(automation: Option<&str>, driver: Option<&str>) -> Option<String> {
    let driver = driver.map(str::trim).filter(|args| !args.is_empty())?;
    (automation == Some("true")).then(|| format!("{WRY_BROWSER_ARGS} {driver}"))
}

/// Shows and focuses the main window the first time it is called (later calls do nothing, so a
/// page reload does not steal focus again). Returns whether this call showed it.
pub fn show_main_once(app: &AppHandle, once: &ShowOnce) -> Result<bool, AppError> {
    if !once.claim() {
        return Ok(false);
    }
    match show_and_focus(app) {
        Ok(()) => Ok(true),
        Err(error) => {
            once.release();
            Err(error)
        }
    }
}

/// Brings the main window forward: a second launch of the app lands here.
pub fn focus_main(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.window_shown.claim();
    }
    if let Some(window) = app.get_webview_window(MAIN_WINDOW)
        && window.is_minimized().unwrap_or(false)
        && let Err(error) = window.unminimize()
    {
        tracing::warn!(%error, "could not restore the minimized window");
    }
    if let Err(error) = show_and_focus(app) {
        tracing::warn!(%error, "could not focus the main window for a second launch");
    }
}

/// Shows and focuses the main window. Every path that shows it comes through here (the page's
/// `window_ready`, the fallback timer, a second launch), so this is also where the work that waits
/// for the window to be on screen starts: the integrity check after a session that did not close
/// the database ([`crate::integrity`]), which starts only once.
pub fn show_and_focus(app: &AppHandle) -> Result<(), AppError> {
    let window = app.get_webview_window(MAIN_WINDOW).ok_or_else(|| {
        AppError::new(
            AppErrorCode::WindowUnavailable,
            "The main window is not available.",
        )
    })?;
    let unavailable =
        |error: tauri::Error| AppError::new(AppErrorCode::WindowUnavailable, error.to_string());
    window.show().map_err(unavailable)?;
    set_webview_visible(app, true);
    // On screen now, even if focusing fails below.
    if let Some(state) = app.try_state::<AppState>() {
        state.integrity.window_shown();
    }
    window.set_focus().map_err(unavailable)?;
    Ok(())
}

/// Waits `delay`, then claims `once` and calls `show`, unless something else showed the window
/// first. Returns whether the window was shown here. Generic over `show` so the timing is testable
/// without a window.
pub async fn show_after(
    delay: Duration,
    once: Arc<ShowOnce>,
    show: impl FnOnce() -> Result<(), AppError> + Send + 'static,
) -> bool {
    tokio::time::sleep(delay).await;
    if !once.claim() {
        return false;
    }
    match show() {
        Ok(()) => true,
        Err(error) => {
            once.release();
            tracing::error!(%error, "could not show the main window");
            false
        }
    }
}

/// Where Tauri serves the app's own pages in a build, which differs by platform and is exactly
/// one origin per window: `tauri://localhost` on macOS and Linux; on Windows (where WebView2 needs
/// an http(s) host for custom protocols) `http://tauri.localhost`, or `https://tauri.localhost`
/// when the window sets `useHttpsScheme`. Tauri serves nothing at the other three, so a navigation
/// there goes to whatever the name resolves to (loopback, under Chromium and systemd-resolved), and
/// a local server could then replace the whole window with a lookalike page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppOrigin {
    scheme: &'static str,
    host: &'static str,
}

impl AppOrigin {
    /// The origin for a window on Windows (`windows`) or elsewhere, with its `useHttpsScheme`.
    pub const fn new(windows: bool, use_https_scheme: bool) -> AppOrigin {
        match (windows, use_https_scheme) {
            (false, _) => AppOrigin {
                scheme: "tauri",
                host: "localhost",
            },
            (true, false) => AppOrigin {
                scheme: "http",
                host: "tauri.localhost",
            },
            (true, true) => AppOrigin {
                scheme: "https",
                host: "tauri.localhost",
            },
        }
    }

    /// The origin on this platform for a window configured with `use_https_scheme`.
    pub const fn current(use_https_scheme: bool) -> AppOrigin {
        AppOrigin::new(cfg!(windows), use_https_scheme)
    }

    /// Same scheme and host, on the default port (Tauri never serves on another one).
    fn serves(&self, url: &Url) -> bool {
        url.scheme() == self.scheme && url.host_str() == Some(self.host) && url.port().is_none()
    }
}

/// Whether the main webview may navigate to `url`: only the app's own pages, served from `app` in a
/// build or from the Vite dev server (`dev_url`) in development.
pub fn is_app_url(url: &Url, app: AppOrigin, dev_url: Option<&Url>) -> bool {
    match url.scheme() {
        "about" => url.as_str() == "about:blank",
        _ => app.serves(url) || dev_url.is_some_and(|dev| same_origin(url, dev)),
    }
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// A blocked navigation to the web opens in the browser instead; anything else (`file:`, custom
/// schemes) is only logged.
fn leave_for_browser(app: &AppHandle, url: &Url) {
    if matches!(url.scheme(), "http" | "https") {
        tracing::info!(%url, "opening an external link in the browser");
        if let Err(error) = app.opener().open_url(url.as_str(), None::<&str>) {
            tracing::warn!(%error, %url, "could not open the link in the browser");
        }
    } else {
        tracing::warn!(%url, "blocked navigation away from the app");
    }
}

#[cfg(test)]
mod tests {
    /// A development window was once titled like the installed app's.
    #[test]
    fn a_development_window_has_its_own_title() {
        assert_eq!(super::main_title("Ghost", false), "Ghost");
        assert_eq!(super::main_title("Ghost", true), super::DEVELOPMENT_TITLE);
        assert_ne!(super::DEVELOPMENT_TITLE, "Ghost");
    }

    /// Native chrome must track both CSS palettes so switching appearance leaves no dark band.
    #[test]
    fn the_caption_matches_both_appearance_palettes() {
        let tokens = include_str!("../../../../packages/ui/src/graphite/tokens.css");
        let colorref = |palette: &str, name: &str| {
            let line = palette
                .lines()
                .find(|line| line.trim_start().starts_with(&format!("{name}:")))
                .unwrap_or_else(|| panic!("{name} is a Graphite token"));
            let hex = line
                .split('#')
                .nth(1)
                .expect("a hex colour")
                .get(..6)
                .unwrap();
            let rgb = u32::from_str_radix(hex, 16).unwrap();
            // 0x00RRGGBB to Windows' 0x00BBGGRR.
            ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
        };
        let light = tokens
            .split("[data-theme=\"light\"]")
            .nth(1)
            .expect("light palette");
        for (theme, palette) in [(tauri::Theme::Dark, tokens), (tauri::Theme::Light, light)] {
            assert_eq!(
                super::caption_colors(theme),
                [
                    colorref(palette, "--g-window"),
                    colorref(palette, "--g-fg-2")
                ]
            );
            // Opaque windows paint the same colour behind the page.
            let base = if theme == tauri::Theme::Dark {
                super::BASE_COLOR_DARK
            } else {
                super::BASE_COLOR
            };
            let [caption, _] = super::caption_colors(theme);
            assert_eq!(
                u32::from(base.0) | (u32::from(base.1) << 8) | (u32::from(base.2) << 16),
                caption
            );
        }
    }

    /// A glass window gives its caption back to the system, so the material runs through it.
    #[test]
    fn a_glass_caption_is_the_systems_and_an_opaque_one_is_painted() {
        for theme in [tauri::Theme::Dark, tauri::Theme::Light] {
            assert_eq!(
                super::caption_paint(theme, true),
                super::caption_colors(theme)
            );
            assert_eq!(
                super::caption_paint(theme, false),
                [super::SYSTEM_COLOR, super::SYSTEM_COLOR]
            );
        }
    }

    #[cfg(all(windows, debug_assertions))]
    #[test]
    fn only_webdriver_automation_adds_the_drivers_switches_to_wrys_own() {
        use super::{WRY_BROWSER_ARGS, automation_args};
        let driver = Some("--remote-debugging-port=0 --enable-automation");
        assert_eq!(
            automation_args(Some("true"), driver).as_deref(),
            Some(
                format!("{WRY_BROWSER_ARGS} --remote-debugging-port=0 --enable-automation")
                    .as_str()
            )
        );
        assert_eq!(automation_args(None, driver), None);
        assert_eq!(automation_args(Some("1"), driver), None);
        assert_eq!(automation_args(Some("true"), None), None);
        assert_eq!(automation_args(Some("true"), Some("   ")), None);
    }

    #[test]
    fn webview_visibility_retries_failed_changes_and_restores_after_minimize() {
        let state = super::WebviewVisibility::default();
        assert!(state.apply(false, |_| Err::<(), _>("failed")).is_err());
        let mut changes = Vec::new();
        for visible in [false, false, true, true, false, true] {
            state
                .apply(visible, |value| {
                    changes.push(value);
                    Ok::<(), ()>(())
                })
                .expect("visibility");
        }
        assert_eq!(changes, [false, true, false, true]);
    }

    use super::*;

    #[test]
    fn cpu_painting_follows_the_switches_that_turn_the_gpu_renderer_off() {
        let with = |pairs: &'static [(&'static str, &'static str)]| {
            paints_on_cpu(|name| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| std::ffi::OsString::from(value))
            })
        };
        assert!(!with(&[]));
        assert!(with(&[("WEBKIT_DISABLE_DMABUF_RENDERER", "1")]));
        assert!(with(&[("WEBKIT_DISABLE_COMPOSITING_MODE", "1")]));
        assert!(with(&[("LIBGL_ALWAYS_SOFTWARE", "true")]));
        // Off, as WebKitGTK and Mesa read them.
        assert!(!with(&[("WEBKIT_DISABLE_DMABUF_RENDERER", "0")]));
        assert!(!with(&[
            ("LIBGL_ALWAYS_SOFTWARE", "false"),
            ("WEBKIT_DISABLE_COMPOSITING_MODE", "")
        ]));
        // The NVIDIA switch keeps the GPU renderer.
        assert!(!with(&[("__NV_DISABLE_EXPLICIT_SYNC", "1")]));
    }
    use std::sync::atomic::AtomicUsize;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }

    #[test]
    fn only_the_first_claim_wins() {
        let once = ShowOnce::default();
        assert!(!once.is_claimed());
        assert!(once.claim());
        assert!(!once.claim());
        assert!(once.is_claimed());
        once.release();
        assert!(once.claim(), "a released claim can be taken again");
    }

    #[tokio::test(start_paused = true)]
    async fn the_fallback_shows_a_window_the_page_never_reported() {
        let once = Arc::new(ShowOnce::default());
        let shown = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&shown);
        let started = tokio::time::Instant::now();
        let fired = show_after(READY_FALLBACK, Arc::clone(&once), move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert!(fired);
        assert_eq!(shown.load(Ordering::SeqCst), 1);
        assert_eq!(started.elapsed(), READY_FALLBACK);
        assert!(once.is_claimed(), "a later window_ready is a no-op");
    }

    #[tokio::test(start_paused = true)]
    async fn the_fallback_stays_quiet_after_the_page_reported() {
        let once = Arc::new(ShowOnce::default());
        let fallback = tokio::spawn(show_after(READY_FALLBACK, Arc::clone(&once), || {
            panic!("the window was already shown");
        }));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(once.claim(), "the page's window_ready claims first");
        assert!(!fallback.await.expect("join"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_fallback_releases_the_claim() {
        let once = Arc::new(ShowOnce::default());
        let fired = show_after(READY_FALLBACK, Arc::clone(&once), || {
            Err(AppError::new(AppErrorCode::WindowUnavailable, "gone"))
        })
        .await;
        assert!(!fired);
        assert!(!once.is_claimed(), "a later window_ready may still try");
    }

    /// The three origins Tauri can serve a window from, whatever platform the test runs on.
    const MACOS_LINUX: AppOrigin = AppOrigin::new(false, false);
    const WINDOWS: AppOrigin = AppOrigin::new(true, false);
    const WINDOWS_HTTPS: AppOrigin = AppOrigin::new(true, true);
    const ALL: [AppOrigin; 3] = [MACOS_LINUX, WINDOWS, WINDOWS_HTTPS];

    #[test]
    fn app_pages_are_allowed() {
        for (origin, page) in [
            (MACOS_LINUX, "tauri://localhost"),
            (MACOS_LINUX, "tauri://localhost/index.html"),
            (WINDOWS, "http://tauri.localhost/"),
            (WINDOWS, "http://tauri.localhost:80/assets/x.js"),
            (WINDOWS_HTTPS, "https://tauri.localhost/assets/x.js"),
        ] {
            assert!(is_app_url(&url(page), origin, None), "{page} on {origin:?}");
        }
        let dev = url("http://localhost:1420");
        for origin in ALL {
            assert!(is_app_url(
                &url("http://localhost:1420/?stress=1"),
                origin,
                Some(&dev)
            ));
            assert!(is_app_url(&url("about:blank"), origin, None));
        }
    }

    #[test]
    fn origins_tauri_does_not_serve_on_this_platform_are_blocked() {
        // One origin per platform and window: the others are not Tauri's, and a server listening
        // where they resolve could replace the whole window.
        for (origin, page) in [
            (MACOS_LINUX, "http://tauri.localhost/"),
            (MACOS_LINUX, "https://tauri.localhost/"),
            (MACOS_LINUX, "tauri://localhost:1/"),
            (WINDOWS, "https://tauri.localhost/"),
            (WINDOWS, "tauri://localhost/"),
            (WINDOWS, "http://tauri.localhost:8080/"),
            (WINDOWS_HTTPS, "http://tauri.localhost/"),
            (WINDOWS_HTTPS, "tauri://localhost/"),
            (WINDOWS_HTTPS, "https://tauri.localhost:8443/"),
        ] {
            assert!(
                !is_app_url(&url(page), origin, None),
                "{page} on {origin:?}"
            );
        }
    }

    #[test]
    fn this_platform_uses_its_own_origin() {
        let served = if cfg!(windows) {
            "http://tauri.localhost/"
        } else {
            "tauri://localhost/"
        };
        assert!(is_app_url(&url(served), AppOrigin::current(false), None));
        assert_eq!(AppOrigin::current(true) == WINDOWS_HTTPS, cfg!(windows));
    }

    #[test]
    fn everything_else_is_blocked() {
        let dev = url("http://localhost:1420");
        for blocked in [
            "https://onghost.dev/docs",
            "http://localhost:1421/",
            "https://localhost:1420/",
            "http://evil.tauri.localhost.example/",
            "http://tauri.localhost.evil.example/",
            "tauri://evil/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "about:srcdoc",
            "mailto:someone@example.com",
        ] {
            for origin in ALL {
                assert!(
                    !is_app_url(&url(blocked), origin, Some(&dev)),
                    "{blocked} on {origin:?}"
                );
            }
        }
        // Without a dev server (a release build), localhost is not the app.
        for origin in ALL {
            assert!(!is_app_url(&url("http://localhost:1420/"), origin, None));
        }
    }

    #[test]
    fn the_restored_state_never_includes_visibility() {
        let flags = restored_state();
        assert!(!flags.contains(StateFlags::VISIBLE));
        assert!(!flags.contains(StateFlags::DECORATIONS));
        assert!(flags.contains(StateFlags::SIZE | StateFlags::POSITION));
    }
}
