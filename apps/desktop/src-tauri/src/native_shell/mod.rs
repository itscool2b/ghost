//! Native long-run shell. Session status feeds it even when no webview is attached.

mod model;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod power;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ghost_core::onboarding::GeneralPreferences;
use ghost_core::session::{SinkGone, StatusSink};
use ghost_protocol::session::ChatStatusUpdate;
use tauri::{Emitter, Manager};
use tokio::sync::{mpsc, watch};

use crate::state::AppState;
use crate::sync::lock;
use model::{Activity, Counts, Notice};

const OWNER: &str = "auracode-native-shell";
const TRAY: &str = "auracode-main";
const OPEN: &str = "auracode-open";
const QUIT: &str = "auracode-quit";
pub const OPEN_CHAT_EVENT: &str = "auracode-open-chat";
/// A chat finished, failed or needs the person, for Settings' Sounds; its payload names which.
pub const NOTICE_EVENT: &str = "auracode-chat-notice";

pub struct NativeShell {
    activity: Mutex<Activity>,
    counts: watch::Sender<Counts>,
    notices: mpsc::Sender<(u64, String, Notice)>,
    profile_epoch: AtomicU64,
    stopped: watch::Sender<bool>,
    tray_ready: AtomicBool,
    pending_chat: Mutex<Option<String>>,
    notification_id: AtomicU32,
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    power: Mutex<Option<power::Power>>,
    /// Settings' switches (all on by default) and whether any chat is working now.
    switches: Switches,
}

/// Settings' General switches as the shell reads them, and the latest work state.
struct Switches {
    notifications: AtomicBool,
    sounds: AtomicBool,
    keep_awake: AtomicBool,
    keep_in_tray: AtomicBool,
    working: AtomicBool,
    /// Remote access asks to keep the computer awake (Settings > Remote access), work or not.
    reachable: AtomicBool,
}

impl Default for Switches {
    fn default() -> Self {
        let general = GeneralPreferences::default();
        Self {
            notifications: AtomicBool::new(general.notifications),
            sounds: AtomicBool::new(general.sounds),
            keep_awake: AtomicBool::new(general.keep_awake),
            keep_in_tray: AtomicBool::new(general.close_to_tray),
            working: AtomicBool::new(false),
            reachable: AtomicBool::new(false),
        }
    }
}

impl NativeShell {
    pub fn active_work_count(&self) -> usize {
        lock(&self.activity).work_count()
    }

    /// Settings' switches take effect at once: turning keep-awake off while an agent works
    /// releases the assertion now; turning it on during work takes it.
    pub fn apply_preferences(&self, general: &GeneralPreferences) {
        let switches = &self.switches;
        switches
            .notifications
            .store(general.notifications, Ordering::Release);
        switches.sounds.store(general.sounds, Ordering::Release);
        switches
            .keep_in_tray
            .store(general.close_to_tray, Ordering::Release);
        switches
            .keep_awake
            .store(general.keep_awake, Ordering::Release);
        self.sync_power();
    }

    /// Records whether any chat works now and holds the keep-awake assertion only while it does
    /// and Settings allows it.
    fn set_working(&self, working: bool) {
        self.switches.working.store(working, Ordering::Release);
        self.sync_power();
    }

    /// Holds off idle sleep for remote access while `on`, whether or not a chat works.
    fn set_reachable(&self, on: bool) {
        self.switches.reachable.store(on, Ordering::Release);
        self.sync_power();
    }

    fn sync_power(&self) {
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        if let Some(power) = lock(&self.power).as_ref() {
            let switches = &self.switches;
            power.set(
                (switches.working.load(Ordering::Acquire)
                    && switches.keep_awake.load(Ordering::Acquire))
                    || switches.reachable.load(Ordering::Acquire),
            );
        }
    }

    fn notifications_enabled(&self) -> bool {
        self.switches.notifications.load(Ordering::Acquire)
    }

    pub fn shutdown(&self) {
        self.stopped.send_replace(true);
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        if let Some(mut power) = lock(&self.power).take() {
            power.stop();
        }
    }

    fn open_chat(&self, app: &tauri::AppHandle, chat: &str) {
        // IDs originate in our own notification payload, never a path or an executable command.
        if chat.is_empty() || chat.len() > 256 || chat.chars().any(char::is_control) {
            return;
        }
        *lock(&self.pending_chat) = Some(chat.to_owned());
        crate::window::focus_main(app);
        let _ = app.emit(OPEN_CHAT_EVENT, ());
    }
}

impl NativeShell {
    fn reset_profile(&self) -> u64 {
        let mut activity = lock(&self.activity);
        let epoch = self.profile_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        *activity = Activity::default();
        self.counts.send_replace(Counts::default());
        *lock(&self.pending_chat) = None;
        epoch
    }

    fn send_at(&self, epoch: u64, update: &ChatStatusUpdate) -> Result<(), SinkGone> {
        let mut activity = lock(&self.activity);
        if *self.stopped.borrow() || self.profile_epoch.load(Ordering::Acquire) != epoch {
            return Err(SinkGone);
        }
        let (counts, notice) = activity.update(update);
        self.counts.send_if_modified(|current| {
            if *current == counts {
                false
            } else {
                *current = counts;
                true
            }
        });
        if let Some(notice) = notice {
            // Never stall an agent's actor for an OS notification. The current attention badge
            // remains authoritative even if an extreme burst fills the notification queue.
            if self
                .notices
                .try_send((epoch, update.chat_id.clone(), notice))
                .is_err()
            {
                tracing::debug!("native notification queue is full; retaining the attention badge");
            }
        }
        Ok(())
    }
}

struct ProfileSink {
    shell: Arc<NativeShell>,
    epoch: u64,
}
impl StatusSink for ProfileSink {
    fn send(&self, update: &ChatStatusUpdate) -> Result<(), SinkGone> {
        self.shell.send_at(self.epoch, update)
    }
    fn owner(&self) -> &str {
        OWNER
    }
}

pub(crate) fn profile_changed(app: &tauri::AppHandle) {
    let Some(shell) = app.try_state::<Arc<NativeShell>>() else {
        return;
    };
    let epoch = shell.reset_profile();
    if let Some(chats) = app.state::<AppState>().chats() {
        chats.manager.status_subscribe(Arc::new(ProfileSink {
            shell: shell.inner().clone(),
            epoch,
        }));
    }
}

/// Called after AppState is managed and before a webview can start a chat. Smoke launches leave
/// real tray, power and notification services disabled.
pub fn setup(app: &tauri::AppHandle) {
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    {
        let state = app.state::<AppState>();
        if state.launch.is_smoke() {
            return;
        }
        let (counts, mut changes) = watch::channel(Counts::default());
        let (notices, mut notifications) = mpsc::channel(64);
        let (stopped, mut stopping) = watch::channel(false);
        let shell = Arc::new(NativeShell {
            activity: Mutex::new(Activity::default()),
            counts,
            notices,
            profile_epoch: AtomicU64::new(0),
            stopped,
            tray_ready: AtomicBool::new(false),
            pending_chat: Mutex::new(None),
            notification_id: AtomicU32::new(0),
            power: Mutex::new(Some(power::Power::start())),
            switches: Switches::default(),
        });
        if let Some(chats) = state.chats() {
            chats.manager.status_subscribe(Arc::new(ProfileSink {
                shell: shell.clone(),
                epoch: 0,
            }));
        }
        app.manage(Arc::clone(&shell));
        match install_tray(app) {
            Ok(()) => shell.tray_ready.store(true, Ordering::Release),
            Err(error) => {
                tracing::warn!(%error, "the tray is unavailable; closing the window will quit safely")
            }
        }
        install_notification_actions(app);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut was_active = false;
            loop {
                tokio::select! {
                    biased;
                    _ = async { let _ = stopping.wait_for(|stop| *stop).await; } => break,
                    changed = changes.changed() => {
                        if changed.is_err() { break; }
                        let counts = *changes.borrow_and_update();
                        let active = counts.active > 0;
                        if active != was_active {
                            shell.set_working(active);
                            was_active = active;
                        }
                        update_tray(&app, counts);
                    }
                    notice = notifications.recv() => match notice {
                        Some((epoch, chat, notice)) => show_notification(&app, epoch, &chat, notice),
                        None => break,
                    }
                }
            }
            shell.shutdown();
        });
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let _ = app;
}

/// Applies Settings' notification, keep-awake and close-to-tray switches. Without a shell (smoke
/// launches) there is nothing to switch.
pub fn apply_preferences(app: &tauri::AppHandle, general: &GeneralPreferences) {
    if let Some(shell) = app.try_state::<Arc<NativeShell>>() {
        shell.apply_preferences(general);
    }
}

/// Keeps the computer awake for remote access while `on` (`ghost_remote::Service::wants_awake`),
/// or stops. Without a shell (smoke launches) there is nothing to hold.
pub fn keep_reachable(app: &tauri::AppHandle, on: bool) {
    if let Some(shell) = app.try_state::<Arc<NativeShell>>() {
        shell.set_reachable(on);
    }
}

/// True only after the tray was successfully installed and while Settings keeps Ghost in the
/// tray. Otherwise the window uses the ordinary guarded quit path, never hiding an application
/// the person asked to close or could not reach again.
pub fn close_to_tray(app: &tauri::AppHandle) -> bool {
    let Some(shell) = app.try_state::<Arc<NativeShell>>() else {
        return false;
    };
    if !shell.tray_ready.load(Ordering::Acquire)
        || !shell.switches.keep_in_tray.load(Ordering::Acquire)
    {
        return false;
    }
    if app
        .try_state::<AppState>()
        .is_some_and(|state| state.quit.is_quitting())
    {
        return false;
    }
    let Some(window) = app.get_webview_window(crate::window::MAIN_WINDOW) else {
        return false;
    };
    match window.hide() {
        Ok(()) => {
            crate::window::set_webview_visible(app, false);
            true
        }
        Err(error) => {
            tracing::warn!(%error, "could not hide the main window to the tray");
            false
        }
    }
}

/// Drain-on-read makes notification navigation survive a suspended or reloaded webview. The UI
/// calls this on startup, focus, and `auracode-open-chat` and selects the returned local chat ID.
#[tauri::command]
#[specta::specta]
pub fn shell_take_navigation(app: tauri::AppHandle) -> Option<String> {
    app.try_state::<Arc<NativeShell>>()
        .and_then(|shell| lock(&shell.pending_chat).take())
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn install_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    // The first Linux tray loads the AppIndicator library and panics, ending Ghost, when it is
    // missing. The same lookup first, so such a system runs without a tray instead.
    #[cfg(target_os = "linux")]
    if !appindicator_available() {
        return Err(std::io::Error::other(
            "neither libayatana-appindicator3 nor libappindicator3 is installed",
        )
        .into());
    }
    let open = MenuItem::with_id(app, OPEN, "Open Ghost", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT, "Quit Ghost", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let tray = TrayIconBuilder::with_id(TRAY)
        .icon(tray_icon())
        .icon_as_template(cfg!(target_os = "macos"))
        .menu(&menu)
        .tooltip("Ghost")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            OPEN => crate::window::focus_main(app),
            QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                crate::window::focus_main(tray.app_handle());
            }
        });
    tray.build(app)?;
    Ok(())
}

/// The libraries libappindicator-sys opens, in its order: Ayatana's and then Canonical's, each by
/// its ABI name and then by the bare name some AppImages carry.
#[cfg(target_os = "linux")]
fn appindicator_available() -> bool {
    [
        "libayatana-appindicator3.so.1",
        "libappindicator3.so.1",
        "libayatana-appindicator3.so",
        "libappindicator3.so",
    ]
    .iter()
    .any(|name| dlopen2::raw::Library::open(name).is_ok())
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn update_tray(app: &tauri::AppHandle, counts: Counts) {
    if let Some(tray) = app.tray_by_id(TRAY) {
        let _ = tray.set_tooltip(Some(model::tooltip(counts)));
        let count = counts.attention.max(counts.active);
        let title = (count > 0).then(|| count.to_string());
        let _ = tray.set_title(title);
        let _ = tray.set_icon(Some(badged_icon(&tray_icon(), counts)));
    }
    #[cfg(target_os = "macos")]
    if let Some(window) = app.get_webview_window(crate::window::MAIN_WINDOW) {
        let count = counts.attention.max(counts.active);
        let _ = window.set_badge_count((count > 0).then_some(count as i64));
    }
}

/// The logo on a black tile for the Windows notification area, generated by scripts/app-icons.mjs.
#[cfg(windows)]
fn tray_icon() -> tauri::image::Image<'static> {
    tauri::include_image!("icons/tray.png")
}

/// The logo on a black tile, as on Windows: Linux status areas (AppIndicator) draw icons in colour.
#[cfg(target_os = "linux")]
fn tray_icon() -> tauri::image::Image<'static> {
    tauri::include_image!("icons/tray.png")
}

/// The logo as a macOS menu bar template image: only its alpha is drawn, in the bar's own colour.
#[cfg(target_os = "macos")]
fn tray_icon() -> tauri::image::Image<'static> {
    tauri::include_image!("icons/tray-template.png")
}

/// Graphite's volt for "needs you" and fg-1 for "working" (visual-language.md 2.1).
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
const BADGE_ATTENTION: [u8; 4] = [204, 255, 51, 255];
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
const BADGE_ACTIVE: [u8; 4] = [237, 237, 237, 255];

/// The badge is cut out of the logo by a transparent ring. Without it the fg-1 "working" dot
/// merged with the off-white logo it overlaps, and a macOS template image (alpha only) drew both
/// as one shape.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
const BADGE_CUTOUT: [u8; 4] = [0, 0, 0, 0];

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn badged_icon(base: &tauri::image::Image<'_>, counts: Counts) -> tauri::image::Image<'static> {
    let (width, height) = (base.width(), base.height());
    let mut pixels = base.rgba().to_vec();
    if (counts.active > 0 || counts.attention > 0) && width >= 8 && height >= 8 {
        let radius = width.min(height) / 5;
        let outer = radius + (width.min(height) / 16).max(1);
        let (cx, cy) = (width - radius - 1, height - radius - 1);
        let color = if counts.attention > 0 {
            BADGE_ATTENTION
        } else {
            BADGE_ACTIVE
        };
        for y in cy.saturating_sub(outer)..=(cy + outer).min(height - 1) {
            for x in cx.saturating_sub(outer)..=(cx + outer).min(width - 1) {
                let (dx, dy) = (i64::from(x) - i64::from(cx), i64::from(y) - i64::from(cy));
                let distance = dx * dx + dy * dy;
                let paint = if distance <= i64::from(radius).pow(2) {
                    color
                } else if distance <= i64::from(outer).pow(2) {
                    BADGE_CUTOUT
                } else {
                    continue;
                };
                let start = (u64::from(y) * u64::from(width) + u64::from(x)) * 4;
                if let Ok(start) = usize::try_from(start)
                    && let Some(pixel) = pixels.get_mut(start..start.saturating_add(4))
                {
                    pixel.copy_from_slice(&paint);
                }
            }
        }
    }
    tauri::image::Image::new_owned(pixels, width, height)
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn install_notification_actions(app: &tauri::AppHandle) {
    use tauri_plugin_notification::NotificationExt;
    let handler_app = app.clone();
    if let Err(error) = app.notification().on_action(move |action| {
        if action.action_id() != "tap" {
            return;
        }
        let chat = action
            .notification()
            .and_then(|notice| notice.extra().get("chat_id"))
            .and_then(serde_json::Value::as_str);
        if let Some(chat) = chat
            && let Some(shell) = handler_app.try_state::<Arc<NativeShell>>()
            && action
                .notification()
                .and_then(|notice| notice.extra().get("profile_epoch"))
                .and_then(serde_json::Value::as_u64)
                == Some(shell.profile_epoch.load(Ordering::Acquire))
            && action
                .notification()
                .and_then(|notice| notice.extra().get("profile_id"))
                .and_then(serde_json::Value::as_str)
                == handler_app
                    .state::<AppState>()
                    .app_info()
                    .profile
                    .as_ref()
                    .map(|profile| profile.id.as_str())
        {
            shell.open_chat(&handler_app, chat);
        }
    }) {
        tracing::warn!(%error, "notification click navigation is unavailable");
    }
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn show_notification(app: &tauri::AppHandle, epoch: u64, chat: &str, notice: Notice) {
    use tauri_plugin_notification::NotificationExt;
    if app
        .try_state::<AppState>()
        .is_some_and(|state| state.quit.is_quitting())
    {
        return;
    }
    let Some(shell) = app.try_state::<Arc<NativeShell>>() else {
        return;
    };
    let current = || {
        shell.profile_epoch.load(Ordering::Acquire) == epoch
            && lock(&shell.activity).notice_is_current(chat, notice)
    };
    // The sound plays in the window, whether or not it is in front; the notification only
    // for a chat the person is not looking at.
    if shell.switches.sounds.load(Ordering::Acquire) && current() {
        let _ = app.emit(NOTICE_EVENT, notice.sound());
    }
    let foreground = app
        .get_webview_window(crate::window::MAIN_WINDOW)
        .is_some_and(|window| {
            window.is_visible().unwrap_or(true)
                && !window.is_minimized().unwrap_or(false)
                && window.is_focused().unwrap_or(true)
        });
    if foreground {
        return;
    }
    if !shell.notifications_enabled() || !current() {
        return;
    }
    let Some(profile) = app.state::<AppState>().app_info().profile else {
        return;
    };
    let id = (shell.notification_id.fetch_add(1, Ordering::Relaxed) % i32::MAX as u32 + 1) as i32;
    if let Err(error) = app
        .notification()
        .builder()
        .id(id)
        .title(notice.title())
        .body("Open Ghost to view the chat.")
        .extra("chat_id", chat)
        .extra("profile_epoch", epoch)
        .extra("profile_id", profile.id)
        .show()
    {
        tracing::warn!(%error, "could not send a native notification");
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn switching_clears_navigation_and_refuses_old_profile_status_updates() {
        let (counts, _) = watch::channel(Counts::default());
        let (notices, _) = mpsc::channel(8);
        let (stopped, _) = watch::channel(false);
        let shell = NativeShell {
            activity: Mutex::default(),
            counts,
            notices,
            stopped,
            profile_epoch: AtomicU64::new(0),
            tray_ready: AtomicBool::new(false),
            pending_chat: Mutex::new(Some("old-chat".into())),
            notification_id: AtomicU32::new(0),
            #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
            power: Mutex::new(None),
            switches: Switches::default(),
        };
        let update = ChatStatusUpdate {
            chat_id: "old-chat".into(),
            status: ghost_protocol::SessionStatus::Running,
            needs_attention: false,
            title: None,
            updated_at: 1,
        };
        shell.send_at(0, &update).unwrap();
        assert_eq!(shell.active_work_count(), 1);
        let epoch = shell.reset_profile();
        assert_eq!(shell.active_work_count(), 0);
        assert!(lock(&shell.pending_chat).is_none());
        assert!(shell.send_at(0, &update).is_err());
        assert_eq!(shell.active_work_count(), 0);
        shell.send_at(epoch, &update).unwrap();
        assert_eq!(shell.active_work_count(), 1);
    }
}

#[cfg(all(test, any(windows, target_os = "macos", target_os = "linux")))]
mod switch_tests {
    use super::*;

    fn shell(power: Option<power::Power>) -> NativeShell {
        let (counts, _) = watch::channel(Counts::default());
        let (notices, _) = mpsc::channel(8);
        let (stopped, _) = watch::channel(false);
        NativeShell {
            activity: Mutex::default(),
            counts,
            notices,
            stopped,
            profile_epoch: AtomicU64::new(0),
            tray_ready: AtomicBool::new(true),
            pending_chat: Mutex::new(None),
            notification_id: AtomicU32::new(0),
            power: Mutex::new(power),
            switches: Switches::default(),
        }
    }

    /// The working badge is fg-1 on an off-white logo: a transparent ring keeps it a separate dot.
    #[test]
    fn the_tray_badge_is_cut_out_of_the_logo() {
        let white = [237, 237, 237, 255];
        let base = tauri::image::Image::new_owned(white.repeat(32 * 32), 32, 32);
        let pixel = |image: &tauri::image::Image<'_>, x: usize, y: usize| -> [u8; 4] {
            let start = (y * 32 + x) * 4;
            image.rgba()[start..start + 4].try_into().expect("pixel")
        };
        let idle = badged_icon(&base, Counts::default());
        assert_eq!(idle.rgba(), base.rgba(), "no badge without work");

        let working = badged_icon(
            &base,
            Counts {
                active: 1,
                ..Counts::default()
            },
        );
        // Radius 6 around (25, 25), then a ring of 2 cut out to the edge of the logo.
        assert_eq!(pixel(&working, 25, 25), BADGE_ACTIVE);
        assert_eq!(
            pixel(&working, 18, 25),
            BADGE_CUTOUT,
            "ring left of the dot"
        );
        assert_eq!(pixel(&working, 25, 18), BADGE_CUTOUT, "ring above the dot");
        assert_eq!(pixel(&working, 16, 25), white, "the logo beyond the ring");
        assert_eq!(pixel(&working, 0, 0), white);

        let attention = badged_icon(
            &base,
            Counts {
                active: 1,
                attention: 1,
            },
        );
        assert_eq!(pixel(&attention, 25, 25), BADGE_ATTENTION);
        assert_eq!(pixel(&attention, 18, 25), BADGE_CUTOUT);
    }

    #[test]
    fn notification_and_tray_switches_start_on_and_follow_settings() {
        let shell = shell(None);
        assert!(shell.notifications_enabled());
        assert!(shell.switches.keep_in_tray.load(Ordering::Acquire));
        shell.apply_preferences(&GeneralPreferences {
            notifications: false,
            close_to_tray: false,
            ..GeneralPreferences::default()
        });
        assert!(!shell.notifications_enabled());
        assert!(!shell.switches.keep_in_tray.load(Ordering::Acquire));
        shell.apply_preferences(&GeneralPreferences::default());
        assert!(shell.notifications_enabled());
        assert!(shell.switches.keep_in_tray.load(Ordering::Acquire));
    }

    /// Keep-awake holds the assertion only while work runs and Settings allows it, and a change
    /// in Settings applies during the work, not at the next turn.
    #[test]
    fn keep_awake_follows_work_and_the_settings_switch_at_once() {
        use std::sync::mpsc::{Sender, channel};
        use std::time::Duration;
        struct Held(Sender<&'static str>);
        impl Drop for Held {
            fn drop(&mut self) {
                let _ = self.0.send("release");
            }
        }
        let (record, events) = channel();
        let power = power::Power::start_with(move || {
            record.send("hold").expect("record hold");
            Some(Held(record.clone()))
        });
        let shell = shell(Some(power));
        let next = || {
            events
                .recv_timeout(Duration::from_secs(5))
                .expect("power event")
        };
        shell.set_working(true);
        assert_eq!(next(), "hold");
        shell.apply_preferences(&GeneralPreferences {
            keep_awake: false,
            ..GeneralPreferences::default()
        });
        assert_eq!(next(), "release", "turning it off releases during the work");
        shell.set_working(false);
        shell.set_working(true);
        assert!(
            events.recv_timeout(Duration::from_millis(200)).is_err(),
            "work never holds the assertion while the switch is off"
        );
        shell.apply_preferences(&GeneralPreferences::default());
        assert_eq!(next(), "hold", "turning it on during work holds it now");
        shell.set_working(false);
        assert_eq!(next(), "release");

        // Remote access keeps it awake with no work at all, and even with the work switch off;
        // the work ending leaves that hold alone.
        shell.apply_preferences(&GeneralPreferences {
            keep_awake: false,
            ..GeneralPreferences::default()
        });
        shell.set_reachable(true);
        assert_eq!(next(), "hold", "remote access holds it");
        shell.set_working(true);
        shell.set_working(false);
        assert!(
            events.recv_timeout(Duration::from_millis(200)).is_err(),
            "work ending does not release what remote access holds"
        );
        shell.set_reachable(false);
        assert_eq!(next(), "release");
    }
}
