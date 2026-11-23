//! The browser beside the chat (docs/superpowers/plans/2026-11-09-browser.md): each chat's tabs,
//! their pages, history, downloads and Annotate, and the browser tools agents use.
//!
//! A tab's page is a native webview of its own (wry: WebView2, WKWebView or WebKitGTK), a child of
//! the main window placed over the panel's page area. It is not a Tauri webview: no o7
//! command, protocol or IPC key reaches a guest page. Its one channel back is wry's message
//! handler, which takes a few typed messages from o7's page script ([`PAGE_SCRIPT`]) and
//! checks each against the page the engine says sent it.
//!
//! The pages live on the main thread ([`host`]); everything else (the tab list, history, what the
//! panel shows) is here, behind one lock that no main-thread call is made under.

mod address;
pub mod agent;
pub mod commands;
#[cfg(target_os = "linux")]
mod gtk_layer;
mod history;
mod host;
#[cfg(target_os = "linux")]
pub mod media;
#[cfg(target_os = "linux")]
mod snapshot;
#[cfg(windows)]
mod webview2;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::sync::lock;
use crate::window::MAIN_WINDOW;

pub use history::HistoryEntry;

/// The window event every change of the browser goes out on.
pub const EVENT: &str = "auracode-browser";

/// o7's page script, injected into every top document before the page's own scripts.
pub const PAGE_SCRIPT: &str = include_str!("page.js");

/// Tabs one chat may have open.
pub const TAB_LIMIT: usize = 24;

/// Pages loaded at once across every chat; the least recently used hidden ones sleep beyond it.
pub const LIVE_LIMIT: usize = 10;

/// How long a page script call may take before o7 gives up on it (a hung page, an `alert`).
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(10);

/// One tab of a chat's browser.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct BrowserTab {
    pub id: String,
    /// `None`: a new tab, which shows o7's start page instead of a web page.
    pub url: Option<String>,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// The page's process ended (it crashed or ran out of memory); Reload brings it back.
    pub crashed: bool,
    /// Why the last load failed, when the engine says (Windows).
    pub error: Option<String>,
    pub zoom: f64,
    /// No page is loaded for it until it is shown: restored after a restart, or put to sleep while
    /// hidden to save memory.
    pub asleep: bool,
    /// Annotate's picker runs in its page.
    pub annotating: bool,
    /// Its page may play sound and video. False on Linux where GStreamer's plugins are missing,
    /// and for a tab whose page process ended twice in a short time (it loads with them off).
    pub media: bool,
}

/// A chat's tabs, in their order, and the one in front.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct ChatBrowser {
    pub chat_id: String,
    pub tabs: Vec<BrowserTab>,
    pub active: Option<String>,
}

/// Whether the browser can be used, and how it keeps its data.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct BrowserStatus {
    /// The Browser plugin is on (Settings > Plugins & MCP).
    pub enabled: bool,
    /// Why the browser cannot run on this system, when it cannot.
    pub unavailable: Option<String>,
    /// The engine offers to save sign-ins (Windows).
    pub passwords: bool,
    /// Why pages play no sound or video on this system, when they cannot (Linux without
    /// GStreamer's plugins), naming what to install.
    pub media_notice: Option<String>,
}

/// Where the panel's page area is, in the window's CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct BrowserRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BrowserRect {
    fn usable(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite())
            && self.width >= 1.0
            && self.height >= 1.0
            && self.width <= 16_384.0
            && self.height <= 16_384.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Started,
    Done,
    Failed,
}

/// A part of a page Annotate picked, as the page script read it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct PickedElement {
    pub tag: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub classes: Vec<String>,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
    #[serde(default)]
    pub selector: String,
    #[serde(default)]
    pub text: String,
    pub rect: PickRect,
    #[serde(default)]
    pub styles: BTreeMap<String, String>,
    #[serde(default)]
    pub component: Option<PickedComponent>,
    #[serde(default)]
    pub html: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct PickRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The React, Vue or Svelte component that rendered a picked element, and the ones around it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct PickedComponent {
    pub name: String,
    #[serde(default)]
    pub stack: Vec<String>,
    #[serde(default)]
    pub framework: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct PickViewport {
    pub width: f64,
    pub height: f64,
    #[serde(default = "one")]
    pub dpr: f64,
}

fn one() -> f64 {
    1.0
}

/// What Annotate picked: one element, or an area and the element at its centre.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct BrowserPick {
    /// `element` or `area`.
    pub mode: String,
    /// The address and title the engine reports for the page (never the page's own word).
    pub url: String,
    pub title: String,
    /// Shift was held: add it to the note open now.
    #[serde(default)]
    pub add: bool,
    pub viewport: PickViewport,
    #[serde(default)]
    pub area: Option<PickRect>,
    #[serde(default)]
    pub elements: Vec<PickedElement>,
    /// The page as it was shown when the pick was made, as a JPEG data URL.
    #[serde(default)]
    pub screenshot: Option<String>,
}

impl BrowserPick {
    /// Holds a pick to sizes the panel can show and an agent can read; a page could send anything.
    fn bounded(mut self) -> Option<BrowserPick> {
        let finite = |rect: &PickRect| {
            [rect.x, rect.y, rect.width, rect.height]
                .iter()
                .all(|value| value.is_finite() && value.abs() < 1.0e7)
        };
        if !matches!(self.mode.as_str(), "element" | "area")
            || !self.viewport.width.is_finite()
            || !self.viewport.height.is_finite()
            || self.area.as_ref().is_some_and(|area| !finite(area))
        {
            return None;
        }
        self.elements.truncate(8);
        self.elements.retain(|element| finite(&element.rect));
        for element in &mut self.elements {
            clip(&mut element.tag, 40);
            clip(&mut element.selector, 400);
            clip(&mut element.text, 400);
            clip(&mut element.html, 1200);
            if let Some(id) = &mut element.id {
                clip(id, 200);
            }
            element.classes.truncate(16);
            for class in &mut element.classes {
                clip(class, 120);
            }
            element.attributes = std::mem::take(&mut element.attributes)
                .into_iter()
                .take(16)
                .map(|(mut name, mut value)| {
                    clip(&mut name, 40);
                    clip(&mut value, 300);
                    (name, value)
                })
                .collect();
            element.styles = std::mem::take(&mut element.styles)
                .into_iter()
                .take(12)
                .map(|(mut name, mut value)| {
                    clip(&mut name, 40);
                    clip(&mut value, 200);
                    (name, value)
                })
                .collect();
            if let Some(component) = &mut element.component {
                clip(&mut component.name, 120);
                clip(&mut component.framework, 20);
                component.stack.truncate(10);
                for name in &mut component.stack {
                    clip(name, 120);
                }
            }
        }
        if self.mode == "element" && self.elements.is_empty() {
            return None;
        }
        Some(self)
    }
}

fn clip(text: &mut String, limit: usize) {
    if text.chars().count() > limit {
        *text = text.chars().take(limit).collect();
    }
}

/// Everything the browser tells the window.
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserEvent {
    /// A chat's tabs changed (opened, closed, moved front, loading, titled, crashed).
    Chat {
        browser: ChatBrowser,
    },
    /// Annotate picked part of a page.
    Pick {
        chat_id: String,
        tab_id: String,
        pick: BrowserPick,
    },
    /// Annotate ended from the page (Esc).
    AnnotateEnded {
        chat_id: String,
        tab_id: String,
    },
    /// One of the browser's own shortcuts, pressed while a page had the keyboard.
    Shortcut {
        chat_id: String,
        tab_id: String,
        action: String,
    },
    /// An agent works in a chat's browser (`label` says what), or stopped (`None`). `opened`: it
    /// opened a page, so the panel should show it.
    Agent {
        chat_id: String,
        tab_id: Option<String>,
        label: Option<String>,
        opened: bool,
    },
    Download {
        chat_id: String,
        tab_id: String,
        url: String,
        file: Option<String>,
        state: DownloadState,
    },
    /// A page tried to open another program's link (`mailto:`, `vscode:`); the person may.
    Outside {
        chat_id: String,
        tab_id: String,
        url: String,
    },
    /// A page tried to open a window without a person's click or key (a pop-up); the person may
    /// open it as a tab.
    Blocked {
        chat_id: String,
        tab_id: String,
        url: String,
    },
    Status {
        status: BrowserStatus,
    },
}

/// Why a browser call failed, in words for a person or an agent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrowserError {
    #[error(
        "Browser use is turned off. Turn it on in Settings > Plugins & MCP > Universal plugins."
    )]
    Disabled,
    #[error("{0}")]
    Unavailable(String),
    #[error("No tab {0} is open.")]
    NoTab(String),
    #[error("{0}")]
    Invalid(String),
    #[error("The page did not answer in time. It may be busy or showing a dialog.")]
    Timeout,
    #[error("{0}")]
    Page(String),
}

impl From<BrowserError> for crate::error::AppError {
    fn from(error: BrowserError) -> Self {
        use crate::error::{AppError, AppErrorCode};
        let code = match error {
            BrowserError::Invalid(_) | BrowserError::NoTab(_) => AppErrorCode::InvalidArgument,
            BrowserError::Disabled | BrowserError::Unavailable(_) => AppErrorCode::Internal,
            BrowserError::Timeout | BrowserError::Page(_) => AppErrorCode::Internal,
        };
        AppError::new(code, error.to_string())
    }
}

#[derive(Debug, Clone)]
struct Tab {
    id: String,
    chat_id: String,
    url: Option<String>,
    title: String,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    crashed: bool,
    error: Option<String>,
    zoom: f64,
    live: bool,
    annotating: bool,
    /// When it was last shown or used, for choosing which page sleeps.
    used: u64,
    /// Finished loads, so a wait can tell a new page from the old one.
    loads: u64,
    /// The tab whose page opened this one (a link to a new tab, a sign-in popup), until the person
    /// picks another tab.
    opener: Option<String>,
    /// A document has loaded in it. A tab a page opened only to download a file never has one.
    landed: bool,
    /// A page opened it only to download a file: it closes once its downloads end.
    download_only: bool,
    /// Downloads it started that have not ended: each one's address and the file it saves to.
    downloads: Vec<(String, PathBuf)>,
    /// When its page's process last ended, to tell a page that keeps ending from a one-off.
    #[cfg(target_os = "linux")]
    ended_at: Option<std::time::Instant>,
    /// Its page loads with sound and video off: its process ended twice within
    /// [`media::CRASH_WINDOW`].
    #[cfg(target_os = "linux")]
    media_off: bool,
}

impl Tab {
    fn new(id: String, chat_id: String, url: Option<String>, title: String) -> Tab {
        Tab {
            id,
            chat_id,
            url,
            title,
            loading: false,
            can_back: false,
            can_forward: false,
            crashed: false,
            error: None,
            zoom: 1.0,
            live: false,
            annotating: false,
            used: 0,
            loads: 0,
            opener: None,
            landed: false,
            download_only: false,
            downloads: Vec::new(),
            #[cfg(target_os = "linux")]
            ended_at: None,
            #[cfg(target_os = "linux")]
            media_off: false,
        }
    }

    /// Whether its page may play sound and video (see [`BrowserTab::media`]).
    fn media(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            !self.media_off && media::available()
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }

    fn view(&self) -> BrowserTab {
        BrowserTab {
            id: self.id.clone(),
            url: self.url.clone(),
            title: self.title.clone(),
            loading: self.loading,
            can_go_back: self.can_back,
            can_go_forward: self.can_forward,
            crashed: self.crashed,
            error: self.error.clone(),
            zoom: self.zoom,
            asleep: !self.live && self.url.is_some(),
            annotating: self.annotating,
            media: self.media(),
        }
    }
}

/// What a navigation that ended came to.
struct Finished {
    /// It is the latest the page started (an earlier one ends when a newer one replaces it).
    latest: bool,
    /// It brought a document. A download, a stop or an empty answer brings none and leaves the
    /// page as it was.
    landed: bool,
    success: bool,
    /// Why the document it brought is the engine's error page.
    error: Option<String>,
}

impl Finished {
    /// A load that finished, where the engine says no more than that.
    #[cfg(not(windows))]
    fn loaded() -> Finished {
        Finished {
            latest: true,
            landed: true,
            success: true,
            error: None,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ChatTabs {
    tabs: Vec<String>,
    active: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Profile {
    /// `<profile>/browser`.
    dir: PathBuf,
}

#[derive(Default)]
struct Inner {
    profile: Option<Profile>,
    chats: BTreeMap<String, ChatTabs>,
    tabs: HashMap<String, Tab>,
    history: history::History,
    sessions_dirty: bool,
    clock: u64,
    /// Agent calls running per chat.
    agents: HashMap<String, u32>,
}

/// What `sessions.json` holds: each chat's tabs and the one in front.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Sessions {
    version: u32,
    chats: BTreeMap<String, SavedChat>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedChat {
    tabs: Vec<SavedTab>,
    active: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedTab {
    id: String,
    url: Option<String>,
    #[serde(default)]
    title: String,
}

/// The browser service: one per app, following the active profile.
pub struct Browser {
    app: AppHandle,
    inner: Mutex<Inner>,
    enabled: AtomicBool,
    passwords: AtomicBool,
    next: AtomicU64,
}

impl Browser {
    pub fn new(app: AppHandle) -> Arc<Browser> {
        let browser = Arc::new(Browser {
            app,
            inner: Mutex::new(Inner::default()),
            enabled: AtomicBool::new(true),
            passwords: AtomicBool::new(true),
            next: AtomicU64::new(1),
        });
        // History and the tab lists reach the disk a moment after they change.
        let saver = Arc::downgrade(&browser);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(browser) = saver.upgrade() else {
                    return;
                };
                browser.save();
            }
        });
        browser
    }

    pub fn status(&self) -> BrowserStatus {
        BrowserStatus {
            enabled: self.enabled(),
            unavailable: host::unavailable(),
            passwords: cfg!(windows) && self.passwords.load(Ordering::Acquire),
            #[cfg(target_os = "linux")]
            media_notice: media::notice(),
            #[cfg(not(target_os = "linux"))]
            media_notice: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// The Browser plugin's switch. Off closes every page; the tab lists stay for when it is on.
    pub fn set_enabled(self: &Arc<Self>, enabled: bool) {
        let was = self.enabled.swap(enabled, Ordering::AcqRel);
        if was == enabled {
            return;
        }
        if !enabled {
            self.sleep_all();
        }
        self.emit(BrowserEvent::Status {
            status: self.status(),
        });
    }

    /// Whether the engine offers to save sign-ins. Applies to pages loaded after it changes.
    pub fn set_passwords(self: &Arc<Self>, enabled: bool) {
        if self.passwords.swap(enabled, Ordering::AcqRel) != enabled {
            let this = Arc::clone(self);
            let _ = self
                .app
                .run_on_main_thread(move || host::apply_passwords(&this, enabled));
            self.emit(BrowserEvent::Status {
                status: self.status(),
            });
        }
    }

    fn passwords(&self) -> bool {
        self.passwords.load(Ordering::Acquire)
    }

    /// The profile changed (startup, a switch): every page closes, and the new profile's tabs and
    /// history are read. `dir` is the profile's folder.
    pub fn set_profile(self: &Arc<Self>, dir: Option<&Path>) {
        let next = dir.map(|dir| Profile {
            dir: dir.join("browser"),
        });
        {
            let inner = lock(&self.inner);
            if inner.profile == next {
                return;
            }
        }
        // The old profile's pages go before its data does.
        self.save();
        let this = Arc::clone(self);
        let _ = self.app.run_on_main_thread(move || host::close_all(&this));
        let mut fresh = Inner {
            profile: next.clone(),
            ..Inner::default()
        };
        if let Some(profile) = &next {
            fresh.history = history::History::load(&profile.dir.join("history.json"));
            let sessions: Sessions = std::fs::read(profile.dir.join("sessions.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            for (chat_id, saved) in sessions.chats {
                let mut chat = ChatTabs::default();
                for tab in saved.tabs.into_iter().take(TAB_LIMIT) {
                    if tab.id.is_empty() || tab.id.len() > 64 || fresh.tabs.contains_key(&tab.id) {
                        continue;
                    }
                    let url = tab
                        .url
                        .filter(|url| address::policy(url) == address::Policy::Open);
                    chat.tabs.push(tab.id.clone());
                    fresh.tabs.insert(
                        tab.id.clone(),
                        Tab::new(tab.id, chat_id.clone(), url, tab.title),
                    );
                }
                chat.active = saved
                    .active
                    .filter(|active| chat.tabs.contains(active))
                    .or_else(|| chat.tabs.last().cloned());
                if !chat.tabs.is_empty() {
                    fresh.chats.insert(chat_id, chat);
                }
            }
        }
        let chats: Vec<String> = fresh.chats.keys().cloned().collect();
        *lock(&self.inner) = fresh;
        for chat_id in chats {
            self.emit_chat(&chat_id);
        }
        self.emit(BrowserEvent::Status {
            status: self.status(),
        });
    }

    /// Writes history and the tab lists when they changed.
    pub fn save(&self) {
        let (dir, history, sessions) = {
            let mut inner = lock(&self.inner);
            let Some(profile) = inner.profile.clone() else {
                return;
            };
            let history = inner.history.take_changes();
            let sessions = if inner.sessions_dirty {
                inner.sessions_dirty = false;
                let saved = Sessions {
                    version: 1,
                    chats: inner
                        .chats
                        .iter()
                        .map(|(chat_id, chat)| {
                            (
                                chat_id.clone(),
                                SavedChat {
                                    tabs: chat
                                        .tabs
                                        .iter()
                                        .filter_map(|id| inner.tabs.get(id))
                                        .map(|tab| SavedTab {
                                            id: tab.id.clone(),
                                            url: tab.url.clone(),
                                            title: tab.title.clone(),
                                        })
                                        .collect(),
                                    active: chat.active.clone(),
                                },
                            )
                        })
                        .collect(),
                };
                serde_json::to_vec(&saved).ok()
            } else {
                None
            };
            (profile.dir, history, sessions)
        };
        if history.is_none() && sessions.is_none() {
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&dir) {
            tracing::warn!(%error, "the browser's folder could not be made");
            return;
        }
        for (name, bytes) in [("history.json", history), ("sessions.json", sessions)] {
            if let Some(bytes) = bytes
                && let Err(error) = write_atomically(&dir.join(name), &bytes)
            {
                tracing::warn!(%error, name, "the browser could not save");
            }
        }
    }

    /// Quitting: every page closes and what changed is saved. On the main thread.
    pub fn shutdown(self: &Arc<Self>) {
        host::close_all(self);
        self.save();
    }

    fn emit(&self, event: BrowserEvent) {
        if let Err(error) = self.app.emit_to(MAIN_WINDOW, EVENT, &event) {
            tracing::debug!(%error, "the browser event could not reach the window");
        }
    }

    fn emit_chat(&self, chat_id: &str) {
        let browser = self.chat(chat_id);
        self.emit(BrowserEvent::Chat { browser });
    }

    /// A chat's tabs as the panel shows them.
    pub fn chat(&self, chat_id: &str) -> ChatBrowser {
        let inner = lock(&self.inner);
        chat_view(&inner, chat_id)
    }

    fn tab(&self, tab_id: &str) -> Result<Tab, BrowserError> {
        lock(&self.inner)
            .tabs
            .get(tab_id)
            .cloned()
            .ok_or_else(|| BrowserError::NoTab(tab_id.to_owned()))
    }

    fn check(&self) -> Result<(), BrowserError> {
        if !self.enabled() {
            return Err(BrowserError::Disabled);
        }
        if let Some(why) = host::unavailable() {
            return Err(BrowserError::Unavailable(why));
        }
        if lock(&self.inner).profile.is_none() {
            return Err(BrowserError::Unavailable(
                "Open a profile before using the browser.".into(),
            ));
        }
        Ok(())
    }

    fn new_id(&self) -> String {
        let serial = self.next.fetch_add(1, Ordering::Relaxed);
        let random = uuid::Uuid::new_v4().simple().to_string();
        format!("t{serial}{}", &random[..10])
    }

    /// Opens a tab in `chat_id` with `url` (a new tab's start page when `None`), in front when
    /// `activate`. The page loads when the tab is shown, or now for an agent.
    pub fn open_tab(
        self: &Arc<Self>,
        chat_id: &str,
        url: Option<String>,
        activate: bool,
    ) -> Result<BrowserTab, BrowserError> {
        self.check()?;
        valid_chat(chat_id)?;
        if let Some(url) = &url
            && address::policy(url) != address::Policy::Open
        {
            return Err(BrowserError::Invalid(format!(
                "The browser does not open {url}."
            )));
        }
        let id = self.new_id();
        let view = {
            let mut inner = lock(&self.inner);
            let count = inner.chats.get(chat_id).map_or(0, |chat| chat.tabs.len());
            if count >= TAB_LIMIT {
                return Err(BrowserError::Invalid(format!(
                    "A chat can have {TAB_LIMIT} tabs open. Close one first."
                )));
            }
            let title = url.as_deref().map(address::site).unwrap_or_default();
            let mut tab = Tab::new(id.clone(), chat_id.to_owned(), url, title);
            inner.clock += 1;
            tab.used = inner.clock;
            let view = tab.view();
            inner.tabs.insert(id.clone(), tab);
            let chat = inner.chats.entry(chat_id.to_owned()).or_default();
            chat.tabs.push(id.clone());
            if activate || chat.active.is_none() {
                chat.active = Some(id);
            }
            inner.sessions_dirty = true;
            view
        };
        self.emit_chat(chat_id);
        Ok(view)
    }

    /// Closes a tab and its page; the tab beside it comes to the front.
    pub fn close_tab(self: &Arc<Self>, tab_id: &str) -> Result<(), BrowserError> {
        let chat_id = {
            let mut inner = lock(&self.inner);
            let Some(tab) = inner.tabs.remove(tab_id) else {
                return Ok(());
            };
            if let Some(chat) = inner.chats.get_mut(&tab.chat_id) {
                let index = chat.tabs.iter().position(|id| id == tab_id).unwrap_or(0);
                chat.tabs.retain(|id| id != tab_id);
                if chat.active.as_deref() == Some(tab_id) {
                    // A popup that closes hands back to the page that opened it.
                    let opener = tab
                        .opener
                        .as_ref()
                        .filter(|opener| chat.tabs.contains(opener));
                    chat.active = opener
                        .or_else(|| chat.tabs.get(index))
                        .or_else(|| chat.tabs.get(index.saturating_sub(1)))
                        .cloned();
                }
                if chat.tabs.is_empty() {
                    inner.chats.remove(&tab.chat_id);
                }
            }
            inner.sessions_dirty = true;
            tab.chat_id
        };
        let this = Arc::clone(self);
        let tab = tab_id.to_owned();
        let _ = self
            .app
            .run_on_main_thread(move || host::close(&this, &tab));
        self.emit_chat(&chat_id);
        Ok(())
    }

    /// Puts `tab_id` in front of its chat's browser.
    pub fn activate(&self, chat_id: &str, tab_id: &str) -> Result<(), BrowserError> {
        {
            let mut inner = lock(&self.inner);
            let Some(chat) = inner.chats.get_mut(chat_id) else {
                return Err(BrowserError::NoTab(tab_id.to_owned()));
            };
            if !chat.tabs.iter().any(|id| id == tab_id) {
                return Err(BrowserError::NoTab(tab_id.to_owned()));
            }
            let before = chat.active.replace(tab_id.to_owned());
            // Picking another tab ends a popup's claim to hand back to its opener.
            if let Some(before) = before.filter(|before| before != tab_id)
                && let Some(tab) = inner.tabs.get_mut(&before)
            {
                tab.opener = None;
            }
            inner.sessions_dirty = true;
        }
        self.emit_chat(chat_id);
        Ok(())
    }

    /// A page asked to close its tab (`window.close()`, or the engine closing a window it opened
    /// only for a download). The engine reports a download's end through the page that started it,
    /// so the files of any still running tell when they are done instead; then the tab closes.
    #[cfg(windows)]
    fn close_requested(self: &Arc<Self>, tab_id: &str) {
        let running = {
            let mut inner = lock(&self.inner);
            inner
                .tabs
                .get_mut(tab_id)
                .map(|tab| (tab.chat_id.clone(), std::mem::take(&mut tab.downloads)))
        };
        if let Some((chat_id, downloads)) = running {
            for (url, file) in downloads {
                self.watch_download(&chat_id, tab_id, url, file);
            }
        }
        let _ = self.close_tab(tab_id);
    }

    /// Reports a download done once its file is in place (the engine writes elsewhere until it
    /// finishes), for up to half an hour.
    #[cfg(windows)]
    fn watch_download(self: &Arc<Self>, chat_id: &str, tab_id: &str, url: String, file: PathBuf) {
        let this = Arc::clone(self);
        let chat_id = chat_id.to_owned();
        let tab_id = tab_id.to_owned();
        tauri::async_runtime::spawn(async move {
            let deadline = std::time::Instant::now() + Duration::from_secs(30 * 60);
            while std::time::Instant::now() < deadline {
                if file.is_file() {
                    this.emit(BrowserEvent::Download {
                        chat_id,
                        tab_id,
                        url,
                        file: Some(file.to_string_lossy().into_owned()),
                        state: DownloadState::Done,
                    });
                    return;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });
    }

    /// Notes that a page in `opener` opened `tab_id`.
    fn opened_by(&self, tab_id: &str, opener: &str) {
        if let Some(tab) = lock(&self.inner).tabs.get_mut(tab_id) {
            tab.opener = Some(opener.to_owned());
        }
    }

    /// A download began in a tab, saving `url` to `file`. A tab with no page of its own (a link
    /// to a file opened in a new tab, or a file's address typed into an empty tab) shows its start
    /// page again, and one a page opened only for the download hands back to that page and closes
    /// once the download ends.
    fn download_began(&self, tab_id: &str, url: &str, file: &Path) {
        let chat_id = {
            let mut inner = lock(&self.inner);
            let Some(tab) = inner.tabs.get_mut(tab_id) else {
                return;
            };
            tab.downloads.push((url.to_owned(), file.to_owned()));
            if tab.landed {
                return;
            }
            tab.url = None;
            tab.loading = false;
            tab.title.clear();
            let opener = tab.opener.clone();
            tab.download_only = opener.is_some();
            let chat_id = tab.chat_id.clone();
            if let Some(opener) = opener
                && let Some(chat) = inner.chats.get_mut(&chat_id)
                && chat.active.as_deref() == Some(tab_id)
                && chat.tabs.contains(&opener)
            {
                chat.active = Some(opener);
            }
            inner.sessions_dirty = true;
            chat_id
        };
        self.emit_chat(&chat_id);
    }

    /// A download of `url` ended: a tab opened only for downloads closes once its last one does,
    /// unless it is its chat's only tab.
    fn download_ended(self: &Arc<Self>, tab_id: &str, url: &str) {
        let close = {
            let mut inner = lock(&self.inner);
            let Some(tab) = inner.tabs.get_mut(tab_id) else {
                return;
            };
            if let Some(at) = tab.downloads.iter().position(|(started, _)| started == url) {
                tab.downloads.remove(at);
            }
            let ready = tab.download_only && tab.downloads.is_empty();
            let chat_id = tab.chat_id.clone();
            ready
                && inner
                    .chats
                    .get(&chat_id)
                    .is_some_and(|chat| chat.tabs.len() > 1)
        };
        if close {
            let _ = self.close_tab(tab_id);
        }
    }

    /// Moves a tab to `index` in its chat's strip.
    pub fn move_tab(&self, tab_id: &str, index: usize) -> Result<(), BrowserError> {
        let chat_id = {
            let mut inner = lock(&self.inner);
            let chat_id = inner
                .tabs
                .get(tab_id)
                .map(|tab| tab.chat_id.clone())
                .ok_or_else(|| BrowserError::NoTab(tab_id.to_owned()))?;
            if let Some(chat) = inner.chats.get_mut(&chat_id) {
                chat.tabs.retain(|id| id != tab_id);
                let at = index.min(chat.tabs.len());
                chat.tabs.insert(at, tab_id.to_owned());
            }
            inner.sessions_dirty = true;
            chat_id
        };
        self.emit_chat(&chat_id);
        Ok(())
    }

    /// Every tab of a chat closes (the chat was deleted).
    pub fn forget_chat(self: &Arc<Self>, chat_id: &str) {
        let tabs = {
            let mut inner = lock(&self.inner);
            let tabs = inner
                .chats
                .remove(chat_id)
                .map(|chat| chat.tabs)
                .unwrap_or_default();
            for tab in &tabs {
                inner.tabs.remove(tab);
            }
            inner.sessions_dirty = true;
            tabs
        };
        if tabs.is_empty() {
            return;
        }
        let this = Arc::clone(self);
        let _ = self.app.run_on_main_thread(move || {
            for tab in tabs {
                host::close(&this, &tab);
            }
        });
        self.emit_chat(chat_id);
    }

    /// Chats that no longer exist (their project or folder was removed) lose their browsers.
    pub fn prune(self: &Arc<Self>, existing: &std::collections::HashSet<String>) {
        let gone: Vec<String> = lock(&self.inner)
            .chats
            .keys()
            .filter(|chat| !existing.contains(*chat))
            .cloned()
            .collect();
        for chat in gone {
            self.forget_chat(&chat);
        }
    }

    /// Loads `input` (an address, a host, a file path or words to search for) in a tab.
    pub async fn navigate(
        self: &Arc<Self>,
        tab_id: &str,
        input: &str,
    ) -> Result<String, BrowserError> {
        self.check()?;
        let url = address::resolve(input).ok_or_else(|| {
            BrowserError::Invalid("That is not an address the browser can open.".into())
        })?;
        let chat_id = {
            let mut inner = lock(&self.inner);
            let tab = inner
                .tabs
                .get_mut(tab_id)
                .ok_or_else(|| BrowserError::NoTab(tab_id.to_owned()))?;
            tab.url = Some(url.clone());
            tab.loading = true;
            tab.error = None;
            tab.crashed = false;
            if tab.title.is_empty() {
                tab.title = address::site(&url);
            }
            inner.sessions_dirty = true;
            inner.tabs[tab_id].chat_id.clone()
        };
        self.emit_chat(&chat_id);
        let tab = tab_id.to_owned();
        let target = url.clone();
        self.on_main(move |browser| host::load(browser, &tab, &target))
            .await??;
        Ok(url)
    }

    /// Back, forward, reload or stop.
    pub async fn go(self: &Arc<Self>, tab_id: &str, action: GoAction) -> Result<(), BrowserError> {
        self.check()?;
        let tab = self.tab(tab_id)?;
        if tab.url.is_none() {
            return Ok(());
        }
        let id = tab_id.to_owned();
        self.on_main(move |browser| host::go(browser, &id, action))
            .await?
    }

    /// Shows `tab_id`'s page over the panel at `rect` (hidden when `visible` is false or there is
    /// no rect), and hides whatever page was shown before.
    pub async fn place(
        self: &Arc<Self>,
        tab_id: Option<String>,
        rect: Option<BrowserRect>,
        visible: bool,
    ) -> Result<(), BrowserError> {
        if tab_id.is_some() {
            self.check()?;
        }
        let rect = rect.filter(BrowserRect::usable);
        if let Some(tab) = &tab_id {
            let mut inner = lock(&self.inner);
            inner.clock += 1;
            let clock = inner.clock;
            if let Some(tab) = inner.tabs.get_mut(tab) {
                tab.used = clock;
            }
        }
        self.on_main(move |browser| host::place(browser, tab_id.as_deref(), rect, visible))
            .await?
    }

    /// The page as shown, as a JPEG data URL (`None` where the engine cannot take one).
    pub async fn capture(self: &Arc<Self>, tab_id: &str) -> Result<Option<String>, BrowserError> {
        self.check()?;
        if !self.tab(tab_id)?.live {
            return Ok(None);
        }
        let shot = host::screenshot(self, tab_id, Shot::Viewport { quality: 82 }).await?;
        Ok(shot.map(|bytes| format!("data:image/jpeg;base64,{bytes}")))
    }

    pub async fn set_zoom(self: &Arc<Self>, tab_id: &str, zoom: f64) -> Result<f64, BrowserError> {
        self.check()?;
        let zoom = if zoom.is_finite() {
            zoom.clamp(0.25, 5.0)
        } else {
            1.0
        };
        let chat_id = {
            let mut inner = lock(&self.inner);
            let tab = inner
                .tabs
                .get_mut(tab_id)
                .ok_or_else(|| BrowserError::NoTab(tab_id.to_owned()))?;
            tab.zoom = zoom;
            tab.chat_id.clone()
        };
        let id = tab_id.to_owned();
        self.on_main(move |browser| host::zoom(browser, &id, zoom))
            .await??;
        self.emit_chat(&chat_id);
        Ok(zoom)
    }

    pub async fn devtools(self: &Arc<Self>, tab_id: &str) -> Result<(), BrowserError> {
        self.check()?;
        let id = tab_id.to_owned();
        self.on_main(move |browser| host::devtools(browser, &id))
            .await?
    }

    /// Focuses the page itself (after the address bar lets go).
    pub async fn focus(self: &Arc<Self>, tab_id: &str) -> Result<(), BrowserError> {
        let id = tab_id.to_owned();
        self.on_main(move |browser| host::focus(browser, &id))
            .await?
    }

    /// Turns Annotate's picker on or off in a tab's page. `accent` and `ink` colour its highlight.
    pub async fn annotate(
        self: &Arc<Self>,
        tab_id: &str,
        on: bool,
        accent: &str,
        ink: &str,
    ) -> Result<bool, BrowserError> {
        self.check()?;
        let tab = self.tab(tab_id)?;
        if on && !tab.live {
            return Err(BrowserError::Invalid(
                "Open a page in this tab before annotating it.".into(),
            ));
        }
        let options = serde_json::json!({ "accent": color(accent), "ink": color(ink) });
        let result = if tab.live {
            self.call(tab_id, &format!("api.annotate({on}, {options})"))
                .await
                .map(|value| value.as_bool().unwrap_or(false))
        } else {
            Ok(false)
        };
        {
            let mut inner = lock(&self.inner);
            if let Some(tab) = inner.tabs.get_mut(tab_id) {
                tab.annotating = on && result.is_ok();
            }
        }
        self.emit_chat(&tab.chat_id);
        result
    }

    /// The element at a point of a tab's page (Shift-click on the frozen page).
    pub async fn pick_at(
        self: &Arc<Self>,
        tab_id: &str,
        x: f64,
        y: f64,
    ) -> Result<Option<PickedElement>, BrowserError> {
        self.check()?;
        if !x.is_finite() || !y.is_finite() {
            return Err(BrowserError::Invalid(
                "The point is not on the page.".into(),
            ));
        }
        let value = self.call(tab_id, &format!("api.pickAt({x}, {y})")).await?;
        if value.is_null() {
            return Ok(None);
        }
        let element: PickedElement = serde_json::from_value(value).map_err(|_| {
            BrowserError::Page("The page answered with something unreadable.".into())
        })?;
        let pick = BrowserPick {
            mode: "element".into(),
            url: String::new(),
            title: String::new(),
            add: true,
            viewport: PickViewport {
                width: 1.0,
                height: 1.0,
                dpr: 1.0,
            },
            area: None,
            elements: vec![element],
            screenshot: None,
        }
        .bounded();
        Ok(pick.and_then(|pick| pick.elements.into_iter().next()))
    }

    /// Calls the page script in a tab: `api` is o7's page agent. Answers what the call
    /// returned, or the page's error.
    pub(crate) async fn call(
        self: &Arc<Self>,
        tab_id: &str,
        expression: &str,
    ) -> Result<Value, BrowserError> {
        let script = format!(
            "(() => {{ try {{ const api = window.__auraBrowser; if (!api) return JSON.stringify({{ missing: true }}); const value = ({expression}); return JSON.stringify({{ ok: true, value: value === undefined ? null : value }}); }} catch (error) {{ return JSON.stringify({{ error: String((error && error.message) || error) }}); }} }})()"
        );
        let answer = self.eval(tab_id, script, SCRIPT_TIMEOUT).await?;
        if answer.get("missing").is_some() {
            return Err(BrowserError::Page(
                "o7's page script is not running in this page (it may be a browser error page or a file o7 cannot script). Reload it, or open another address.".into(),
            ));
        }
        if let Some(error) = answer.get("error").and_then(Value::as_str) {
            return Err(BrowserError::Page(error.to_owned()));
        }
        Ok(answer.get("value").cloned().unwrap_or(Value::Null))
    }

    /// Runs `script` in a tab's page and decodes the JSON string it returns.
    pub(crate) async fn eval(
        self: &Arc<Self>,
        tab_id: &str,
        script: String,
        timeout: Duration,
    ) -> Result<Value, BrowserError> {
        let (sender, receiver) = tokio::sync::oneshot::channel::<String>();
        let id = tab_id.to_owned();
        self.on_main(move |browser| host::eval(browser, &id, &script, sender))
            .await??;
        let raw = tokio::time::timeout(timeout, receiver)
            .await
            .map_err(|_| BrowserError::Timeout)?
            .map_err(|_| BrowserError::Page("The page went away.".into()))?;
        Ok(decode(&raw))
    }

    /// Runs `work` on the main thread, where the pages live, and answers its result.
    async fn on_main<T: Send + 'static>(
        self: &Arc<Self>,
        work: impl FnOnce(&Arc<Browser>) -> T + Send + 'static,
    ) -> Result<T, BrowserError> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let this = Arc::clone(self);
        self.app
            .run_on_main_thread(move || {
                let _ = sender.send(work(&this));
            })
            .map_err(|error| BrowserError::Unavailable(error.to_string()))?;
        receiver
            .await
            .map_err(|_| BrowserError::Unavailable("o7 is closing.".into()))
    }

    /// Every page goes to sleep (the plugin was turned off, or the profile is closing).
    fn sleep_all(self: &Arc<Self>) {
        let this = Arc::clone(self);
        let _ = self.app.run_on_main_thread(move || host::close_all(&this));
        let chats: Vec<String> = {
            let mut inner = lock(&self.inner);
            for tab in inner.tabs.values_mut() {
                tab.live = false;
                tab.loading = false;
                tab.annotating = false;
            }
            inner.chats.keys().cloned().collect()
        };
        for chat in chats {
            self.emit_chat(&chat);
        }
    }

    // ------------------------------------------------------------ history

    pub fn history(&self, query: &str, limit: usize, before: Option<f64>) -> Vec<HistoryEntry> {
        lock(&self.inner)
            .history
            .list(query, limit.clamp(1, 500), before)
    }

    pub fn suggest(&self, input: &str, limit: usize) -> Vec<HistoryEntry> {
        lock(&self.inner)
            .history
            .suggest(input, limit.clamp(1, 20), now_ms())
    }

    pub fn remove_history(&self, urls: &[String]) -> usize {
        lock(&self.inner).history.remove(urls)
    }

    /// Clears what `data` names: history, cookies and site data, the cache, saved passwords.
    pub async fn clear(self: &Arc<Self>, data: ClearData) -> Result<(), BrowserError> {
        if data.history {
            lock(&self.inner).history.clear();
        }
        if data.cookies || data.cache || data.passwords {
            self.on_main(move |browser| host::clear(browser, data))
                .await??;
        }
        self.save();
        Ok(())
    }

    // ------------------------------------------------------------ events from the pages

    fn page_started(&self, tab_id: &str, url: &str) {
        let chat = {
            let mut inner = lock(&self.inner);
            let Some(tab) = inner.tabs.get_mut(tab_id) else {
                return;
            };
            tab.loading = true;
            tab.crashed = false;
            tab.error = None;
            tab.landed = true;
            tab.download_only = false;
            if address::policy(url) == address::Policy::Open && url != "about:blank" {
                tab.url = Some(url.to_owned());
            }
            if tab.annotating {
                // A new document has no picker.
                tab.annotating = false;
                self.emit(BrowserEvent::AnnotateEnded {
                    chat_id: tab.chat_id.clone(),
                    tab_id: tab_id.to_owned(),
                });
            }
            inner.sessions_dirty = true;
            inner.tabs[tab_id].chat_id.clone()
        };
        self.emit_chat(&chat);
    }

    fn page_finished(&self, tab_id: &str, url: Option<&str>, finished: Finished) {
        let chat = {
            let mut inner = lock(&self.inner);
            let Some(tab) = inner.tabs.get_mut(tab_id) else {
                return;
            };
            if finished.latest {
                tab.loading = false;
            }
            tab.loads += 1;
            if finished.landed {
                tab.error = finished.error;
            }
            let url = url.filter(|url| {
                address::policy(url) == address::Policy::Open && *url != "about:blank"
            });
            if let Some(url) = url {
                tab.url = Some(url.to_owned());
            }
            let title = tab.title.clone();
            let chat = tab.chat_id.clone();
            if finished.success
                && let Some(url) = url
            {
                inner.history.visit(url, &title, now_ms());
            }
            inner.sessions_dirty = true;
            chat
        };
        self.emit_chat(&chat);
    }

    fn page_titled(&self, tab_id: &str, title: &str) {
        let chat = {
            let mut inner = lock(&self.inner);
            let Some(tab) = inner.tabs.get_mut(tab_id) else {
                return;
            };
            let mut title = title.trim().to_owned();
            clip(&mut title, 300);
            if tab.title == title {
                return;
            }
            tab.title = title.clone();
            let url = tab.url.clone();
            let chat = tab.chat_id.clone();
            if let Some(url) = url {
                inner.history.retitle(&url, &title);
            }
            inner.sessions_dirty = true;
            chat
        };
        self.emit_chat(&chat);
    }

    /// The engine's own word on where a tab is and whether it can go back (Windows reports these
    /// directly; elsewhere they follow the page's loads and moves).
    fn page_state(&self, tab_id: &str, update: impl FnOnce(&mut Tab)) {
        let chat = {
            let mut inner = lock(&self.inner);
            let Some(tab) = inner.tabs.get_mut(tab_id) else {
                return;
            };
            let before = tab.view();
            update(tab);
            if tab.view() == before {
                return;
            }
            let chat = tab.chat_id.clone();
            inner.sessions_dirty = true;
            chat
        };
        self.emit_chat(&chat);
    }

    fn tab_chat(&self, tab_id: &str) -> Option<String> {
        lock(&self.inner)
            .tabs
            .get(tab_id)
            .map(|tab| tab.chat_id.clone())
    }

    fn page_message(self: &Arc<Self>, tab_id: &str, page_url: &str, body: &str) {
        if body.len() > 256 * 1024 {
            return;
        }
        let Ok(message) = serde_json::from_str::<Value>(body) else {
            return;
        };
        let Some(chat_id) = self.tab_chat(tab_id) else {
            return;
        };
        match message.get("kind").and_then(Value::as_str) {
            Some("key") => {
                let Some(action) = message.get("action").and_then(Value::as_str) else {
                    return;
                };
                if matches!(
                    action,
                    "address"
                        | "new-tab"
                        | "close-tab"
                        | "toggle"
                        | "annotate"
                        | "next-tab"
                        | "previous-tab"
                ) {
                    self.emit(BrowserEvent::Shortcut {
                        chat_id,
                        tab_id: tab_id.to_owned(),
                        action: action.to_owned(),
                    });
                }
            }
            Some("location") => {
                let this = Arc::clone(self);
                let tab = tab_id.to_owned();
                // The engine says where the page is; the message only says it moved.
                let _ = self
                    .app
                    .run_on_main_thread(move || host::refresh_location(&this, &tab));
            }
            Some("annotate-exit") => {
                let annotating = lock(&self.inner)
                    .tabs
                    .get(tab_id)
                    .is_some_and(|tab| tab.annotating);
                if annotating {
                    // Esc in the page ends Annotate as the button does: the picker goes and the
                    // tab says it is not annotating.
                    let this = Arc::clone(self);
                    let tab = tab_id.to_owned();
                    tauri::async_runtime::spawn(async move {
                        let _ = this.annotate(&tab, false, "", "").await;
                    });
                    self.emit(BrowserEvent::AnnotateEnded {
                        chat_id,
                        tab_id: tab_id.to_owned(),
                    });
                }
            }
            Some("pick") => {
                let annotating = lock(&self.inner)
                    .tabs
                    .get(tab_id)
                    .is_some_and(|tab| tab.annotating);
                if !annotating {
                    return;
                }
                let Ok(mut pick) = serde_json::from_value::<BrowserPick>(message) else {
                    return;
                };
                pick.url = page_url.to_owned();
                pick.title = lock(&self.inner)
                    .tabs
                    .get(tab_id)
                    .map(|tab| tab.title.clone())
                    .unwrap_or_default();
                pick.screenshot = None;
                let Some(pick) = pick.bounded() else {
                    return;
                };
                let this = Arc::clone(self);
                let tab = tab_id.to_owned();
                tauri::async_runtime::spawn(async move {
                    let mut pick = pick;
                    pick.screenshot = this.capture(&tab).await.ok().flatten();
                    this.emit(BrowserEvent::Pick {
                        chat_id,
                        tab_id: tab,
                        pick,
                    });
                });
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ agents

    /// An agent's call starts (`label` says what it does) or ends (`None`) in a chat's browser.
    pub(crate) fn agent_activity(
        &self,
        chat_id: &str,
        tab_id: Option<&str>,
        label: Option<&str>,
        opened: bool,
    ) {
        {
            let mut inner = lock(&self.inner);
            let count = inner.agents.entry(chat_id.to_owned()).or_insert(0);
            if label.is_some() {
                *count += 1;
            } else {
                *count = count.saturating_sub(1);
            }
        }
        self.emit(BrowserEvent::Agent {
            chat_id: chat_id.to_owned(),
            tab_id: tab_id.map(str::to_owned),
            label: label.map(str::to_owned),
            opened,
        });
    }

    /// Whether an agent call runs in a chat's browser now.
    fn agent_busy(&self, chat_id: &str) -> bool {
        lock(&self.inner)
            .agents
            .get(chat_id)
            .is_some_and(|count| *count > 0)
    }
}

/// What Clear browsing data removes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
pub struct ClearData {
    pub history: bool,
    pub cookies: bool,
    pub cache: bool,
    pub passwords: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum GoAction {
    Back,
    Forward,
    Reload,
    Stop,
}

/// What a screenshot covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Shot {
    /// What the panel shows, as a JPEG.
    Viewport { quality: u8 },
    /// The whole page, as a PNG.
    FullPage,
    /// One part of the page (CSS pixels of the viewport), as a PNG.
    Clip {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
}

/// Agents' calls from o7-core's browser grants, run in the app's browser.
struct AgentBridge(std::sync::Weak<Browser>);

impl o7_core::browser_service::BrowserBridge for AgentBridge {
    fn call(
        &self,
        chat_id: String,
        tool: String,
        arguments: Value,
    ) -> o7_core::browser_service::BrowserFuture {
        let browser = self.0.upgrade();
        Box::pin(async move {
            let Some(browser) = browser else {
                return o7_core::browser_service::BrowserToolReply {
                    text: "o7 is closing.".into(),
                    image: None,
                    error: true,
                };
            };
            let reply = agent::call(&browser, &chat_id, &tool, &arguments).await;
            o7_core::browser_service::BrowserToolReply {
                text: reply.text,
                image: reply.image,
                error: reply.error,
            }
        })
    }
}

/// What `chat_id`'s browser tab shows now, taken as the agent's screenshot is, for a bot that
/// asked to look (`crate::remote`): the image's bytes, or why there are none, said for a bot.
pub(crate) async fn picture(app: &AppHandle, chat_id: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    use tauri::Manager as _;
    let browser = app
        .try_state::<Arc<Browser>>()
        .map(|browser| Arc::clone(&browser))
        .ok_or_else(|| "The browser is not available in this run.".to_owned())?;
    let reply = agent::call(
        &browser,
        chat_id,
        "browser_screenshot",
        &serde_json::json!({}),
    )
    .await;
    if reply.error {
        // Words meant for an agent, which has the browser's tools; a bot has not.
        let empty = reply.text.starts_with("No page is open")
            || reply.text.starts_with("The tab in front has no page");
        return Err(if empty {
            "This chat's browser shows no page.".to_owned()
        } else {
            reply.text
        });
    }
    let (_, data) = reply
        .image
        .ok_or_else(|| "The browser gave no picture.".to_owned())?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| error.to_string())
}

/// Starts the browser during setup, on the main thread: what this system allows, the service,
/// and the active profile.
pub fn setup(app: &AppHandle) {
    use tauri::Manager;
    host::probe(app);
    let browser = Browser::new(app.clone());
    app.manage(Arc::clone(&browser));
    sync(app);
}

/// The active profile reaches the browser (after setup and a profile switch): its folder, whether
/// it keeps browser data on disk, and its sessions' bridge to the browser.
pub fn sync(app: &AppHandle) {
    use tauri::Manager;
    let Some(browser) = app
        .try_state::<Arc<Browser>>()
        .map(|state| Arc::clone(&state))
    else {
        return;
    };
    let chats = app
        .try_state::<crate::state::AppState>()
        .and_then(|state| state.chats());
    match chats {
        Some(chats) => {
            browser.set_profile(Some(&chats.profile_dir));
            chats
                .manager
                .set_browser_bridge(Some(Arc::new(AgentBridge(Arc::downgrade(&browser)))));
        }
        None => browser.set_profile(None),
    }
}

/// Settings' switches for the browser: the plugin itself and saved sign-ins.
pub fn apply_preferences(app: &AppHandle, general: &o7_core::onboarding::GeneralPreferences) {
    use tauri::Manager;
    sync(app);
    if let Some(browser) = app
        .try_state::<Arc<Browser>>()
        .map(|state| Arc::clone(&state))
    {
        browser.set_enabled(general.browser);
        browser.set_passwords(general.browser_passwords);
    }
}

/// Whether the window's own page may play sound (Settings' Sounds), and why not. Linux decides at
/// startup ([`media`]); Windows and macOS always can.
pub fn window_media() -> (bool, Option<String>) {
    #[cfg(target_os = "linux")]
    {
        media::window()
    }
    #[cfg(not(target_os = "linux"))]
    {
        (true, None)
    }
}

/// The window was minimized or restored.
pub fn window_shown(app: &AppHandle, shown: bool) {
    let _ = app.run_on_main_thread(move || host::window_shown(shown));
}

/// o7 quits: every page closes and what changed is saved. On the main thread.
pub fn shutdown(app: &AppHandle) {
    use tauri::Manager;
    if let Some(browser) = app.try_state::<Arc<Browser>>() {
        browser.shutdown();
    }
}

fn chat_view(inner: &Inner, chat_id: &str) -> ChatBrowser {
    let chat = inner.chats.get(chat_id);
    ChatBrowser {
        chat_id: chat_id.to_owned(),
        tabs: chat
            .map(|chat| {
                chat.tabs
                    .iter()
                    .filter_map(|id| inner.tabs.get(id))
                    .map(Tab::view)
                    .collect()
            })
            .unwrap_or_default(),
        active: chat.and_then(|chat| chat.active.clone()),
    }
}

fn valid_chat(chat_id: &str) -> Result<(), BrowserError> {
    if chat_id.is_empty() || chat_id.len() > 128 || chat_id.chars().any(char::is_control) {
        return Err(BrowserError::Invalid("The chat id is invalid.".into()));
    }
    Ok(())
}

/// A colour for the picker's highlight: `#rrggbb` or `rgb(...)`, else nothing (the page script's
/// own default).
fn color(value: &str) -> String {
    let value = value.trim();
    let hex = value.len() == 7
        && value.starts_with('#')
        && value[1..].chars().all(|c| c.is_ascii_hexdigit());
    let rgb = value.len() <= 40
        && (value.starts_with("rgb(") || value.starts_with("rgba("))
        && value.ends_with(')')
        && value[value.find('(').unwrap_or(0) + 1..value.len() - 1]
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | ' ' | '/' | '%'));
    if hex || rgb {
        value.to_owned()
    } else {
        String::new()
    }
}

/// The value a script returned: engines hand back JSON, and o7's scripts return a JSON
/// string inside it.
fn decode(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::String(inner)) => serde_json::from_str(&inner).unwrap_or(Value::String(inner)),
        Ok(value) => value,
        Err(_) => Value::String(raw.to_owned()),
    }
}

pub(crate) fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_millis() as f64)
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let folder = path.parent().unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(folder)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_answers_decode_once_or_twice() {
        assert_eq!(
            decode(r#""{\"ok\":true}""#),
            serde_json::json!({ "ok": true })
        );
        assert_eq!(decode(r#"{"a":1}"#), serde_json::json!({ "a": 1 }));
        assert_eq!(decode(r#""plain""#), Value::String("plain".into()));
        assert_eq!(decode("null"), Value::Null);
        assert_eq!(decode("not json"), Value::String("not json".into()));
    }

    #[test]
    fn only_plain_colours_reach_the_page() {
        assert_eq!(color("#d97757"), "#d97757");
        assert_eq!(color("rgb(217, 119, 87)"), "rgb(217, 119, 87)");
        assert_eq!(color("rgba(0 0 0 / 50%)"), "rgba(0 0 0 / 50%)");
        assert_eq!(color("red"), "");
        assert_eq!(color("#fff\"; alert(1)"), "");
        assert_eq!(color("rgb(1,2,3)); x("), "");
    }

    fn element(tag: &str) -> PickedElement {
        PickedElement {
            tag: tag.into(),
            id: None,
            classes: vec!["a".repeat(500); 40],
            attributes: (0..40)
                .map(|i| (format!("data-{i}"), "v".repeat(900)))
                .collect(),
            selector: "s".repeat(2000),
            text: "t".repeat(2000),
            rect: PickRect {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            },
            styles: BTreeMap::new(),
            component: None,
            html: "h".repeat(5000),
        }
    }

    #[test]
    fn a_pick_from_a_page_is_held_to_its_sizes() {
        let pick = BrowserPick {
            mode: "element".into(),
            url: String::new(),
            title: String::new(),
            add: false,
            viewport: PickViewport {
                width: 800.0,
                height: 600.0,
                dpr: 1.0,
            },
            area: None,
            elements: (0..20).map(|_| element("button")).collect(),
            screenshot: None,
        }
        .bounded()
        .expect("a pick");
        assert_eq!(pick.elements.len(), 8);
        let first = &pick.elements[0];
        assert_eq!(first.classes.len(), 16);
        assert_eq!(first.classes[0].len(), 120);
        assert_eq!(first.attributes.len(), 16);
        assert!(first.attributes.values().all(|value| value.len() == 300));
        assert_eq!(first.selector.len(), 400);
        assert_eq!(first.html.len(), 1200);
    }

    #[test]
    fn a_broken_pick_is_dropped() {
        let base = BrowserPick {
            mode: "element".into(),
            url: String::new(),
            title: String::new(),
            add: false,
            viewport: PickViewport {
                width: 800.0,
                height: 600.0,
                dpr: 1.0,
            },
            area: None,
            elements: vec![],
            screenshot: None,
        };
        assert!(
            base.clone().bounded().is_none(),
            "an element pick needs an element"
        );
        assert!(
            BrowserPick {
                mode: "script".into(),
                ..base.clone()
            }
            .bounded()
            .is_none()
        );
        let mut nan = element("div");
        nan.rect.x = f64::NAN;
        assert!(
            BrowserPick {
                elements: vec![nan],
                ..base.clone()
            }
            .bounded()
            .is_none()
        );
        assert!(
            BrowserPick {
                mode: "area".into(),
                area: Some(PickRect {
                    x: 0.0,
                    y: 0.0,
                    width: 10.0,
                    height: 10.0
                }),
                ..base
            }
            .bounded()
            .is_some(),
            "an area may hold no element"
        );
    }

    #[test]
    fn panel_rects_must_be_sane() {
        let rect = |x: f64, y: f64, width: f64, height: f64| BrowserRect {
            x,
            y,
            width,
            height,
        };
        assert!(rect(10.0, 20.0, 600.0, 400.0).usable());
        assert!(!rect(10.0, 20.0, 0.0, 400.0).usable());
        assert!(!rect(f64::NAN, 20.0, 600.0, 400.0).usable());
        assert!(!rect(0.0, 0.0, 1.0e9, 400.0).usable());
    }
}
