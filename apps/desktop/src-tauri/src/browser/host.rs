//! The pages themselves, on the main thread: one wry webview per live tab, a child of the main
//! window, shown over the panel's page area or hidden.
//!
//! Every function here runs on the main thread. None holds a borrow of the host or the browser's
//! lock while wry builds a webview, because WebView2 pumps window messages while it builds and
//! another page's events (or a popup's) can arrive then.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tauri::Manager;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{NewWindowResponse, PageLoadEvent, WebViewBuilder};

use super::{Browser, Shot};
use super::{
    BrowserError, BrowserEvent, BrowserRect, ClearData, DownloadState, GoAction, LIVE_LIMIT,
    PAGE_SCRIPT, address,
};
use crate::sync::lock;
use crate::window::MAIN_WINDOW;

/// Where a page that is not shown but must keep drawing (an agent works in it) waits: out of the
/// window's client area, at the panel's last size.
const PARKED_AT: f64 = -24_000.0;

/// The size a page gets before the panel has ever been placed.
const DEFAULT_SIZE: (f64, f64) = (1280.0, 800.0);

struct View {
    webview: wry::WebView,
    /// Shown off-window so an agent's page keeps drawing.
    parked: bool,
}

#[derive(Default)]
struct Host {
    views: HashMap<String, View>,
    /// The tab over the panel, where, and whether it shows.
    placed: Option<(String, BrowserRect, bool)>,
    /// The panel's last size, for pages that open hidden.
    size: Option<(f64, f64)>,
    /// The window is minimized or hidden: nothing shows.
    window_hidden: bool,
    #[cfg(windows)]
    environment: Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment>,
}

thread_local! {
    static HOST: RefCell<Host> = RefCell::new(Host::default());
    /// The profile the pages keep their data in (Linux and the first Windows page build with it).
    static CONTEXT: RefCell<Option<(PathBuf, wry::WebContext)>> = const { RefCell::new(None) };
}

static UNAVAILABLE: OnceLock<Option<String>> = OnceLock::new();

/// Decides once, on the main thread at startup, whether this system can show the browser.
pub(super) fn probe(app: &tauri::AppHandle) {
    let _ = UNAVAILABLE.set(probe_window(app));
}

#[cfg(target_os = "linux")]
fn probe_window(app: &tauri::AppHandle) -> Option<String> {
    let window = app.get_webview_window(MAIN_WINDOW)?;
    // The window's own page, before its first document and before the overlay moves it: its
    // sound and its process (`media`).
    super::media::guard_window(&window);
    // Asked of GDK, not of the window's handle: the window is still hidden here, and a Wayland
    // window has no handle to give until it shows.
    match super::gtk_layer::display_kind().as_deref() {
        // X11: each page is a child window of the main one.
        Some("GdkX11Display") => None,
        // Wayland has no child windows: the pages are widgets in a layer over the app's page.
        Some("GdkWaylandDisplay") => super::gtk_layer::install(&window)
            .err()
            .map(|error| format!("The browser could not join the window: {error}")),
        Some(other) => Some(format!("The browser does not know this display ({other}).")),
        None => Some("The browser found no display.".into()),
    }
}

#[cfg(not(target_os = "linux"))]
fn probe_window(_app: &tauri::AppHandle) -> Option<String> {
    None
}

/// Why the browser cannot run here, when it cannot.
pub(super) fn unavailable() -> Option<String> {
    UNAVAILABLE.get().cloned().flatten()
}

fn rect(rect: &BrowserRect) -> wry::Rect {
    wry::Rect {
        position: LogicalPosition::new(rect.x, rect.y).into(),
        size: LogicalSize::new(rect.width, rect.height).into(),
    }
}

fn parked_rect() -> BrowserRect {
    let (width, height) = HOST.with_borrow(|host| host.size.unwrap_or(DEFAULT_SIZE));
    BrowserRect {
        x: PARKED_AT,
        y: PARKED_AT,
        width,
        height,
    }
}

fn with_view<T>(tab: &str, work: impl FnOnce(&View) -> T) -> Result<T, BrowserError> {
    HOST.with_borrow(|host| host.views.get(tab).map(work))
        .ok_or_else(|| BrowserError::NoTab(tab.to_owned()))
}

/// Builds a tab's page if it has none. `url` loads in it; `None` loads the tab's own address.
fn ensure(browser: &Arc<Browser>, tab_id: &str) -> Result<(), BrowserError> {
    if HOST.with_borrow(|host| host.views.contains_key(tab_id)) {
        return Ok(());
    }
    let (chat_id, url, zoom) = {
        let inner = lock(&browser.inner);
        let tab = inner
            .tabs
            .get(tab_id)
            .ok_or_else(|| BrowserError::NoTab(tab_id.to_owned()))?;
        (tab.chat_id.clone(), tab.url.clone(), tab.zoom)
    };
    let webview = build(browser, tab_id, &chat_id, url.as_deref())?;
    if (zoom - 1.0).abs() > f64::EPSILON {
        let _ = webview.zoom(zoom);
    }
    HOST.with_borrow_mut(|host| {
        host.views.insert(
            tab_id.to_owned(),
            View {
                webview,
                parked: false,
            },
        );
    });
    browser.page_state(tab_id, |tab| tab.live = true);
    Ok(())
}

/// Puts the least recently used hidden pages to sleep until fewer than [`LIVE_LIMIT`] are live.
fn evict(browser: &Arc<Browser>) {
    loop {
        let (count, placed, candidates) = HOST.with_borrow(|host| {
            (
                host.views.len(),
                host.placed.as_ref().map(|(tab, _, _)| tab.clone()),
                host.views.keys().cloned().collect::<Vec<_>>(),
            )
        });
        if count < LIVE_LIMIT {
            return;
        }
        let victim = {
            let inner = lock(&browser.inner);
            candidates
                .iter()
                .filter(|id| Some(*id) != placed.as_ref())
                .filter_map(|id| inner.tabs.get(id))
                .filter(|tab| !inner.agents.get(&tab.chat_id).is_some_and(|n| *n > 0))
                // A page with a download running reports its end; it sleeps once that comes.
                .filter(|tab| tab.downloads.is_empty())
                .min_by_key(|tab| tab.used)
                .map(|tab| tab.id.clone())
        };
        let Some(victim) = victim else {
            return;
        };
        sleep(browser, &victim);
    }
}

/// Closes a tab's page but keeps the tab: it loads again when shown.
fn sleep(browser: &Arc<Browser>, tab_id: &str) {
    let view = HOST.with_borrow_mut(|host| {
        if host
            .placed
            .as_ref()
            .is_some_and(|(tab, _, _)| tab == tab_id)
        {
            host.placed = None;
        }
        host.views.remove(tab_id)
    });
    drop(view);
    #[cfg(windows)]
    super::webview2::forget(tab_id);
    browser.page_state(tab_id, |tab| {
        tab.live = false;
        tab.loading = false;
        tab.annotating = false;
        // A sleeping page reports no download's end.
        tab.downloads.clear();
    });
}

fn build(
    browser: &Arc<Browser>,
    tab_id: &str,
    chat_id: &str,
    url: Option<&str>,
) -> Result<wry::WebView, BrowserError> {
    if let Some(why) = unavailable() {
        return Err(BrowserError::Unavailable(why));
    }
    let profile = lock(&browser.inner).profile.clone().ok_or_else(|| {
        BrowserError::Unavailable("Open a profile before using the browser.".into())
    })?;
    let window = browser
        .app
        .get_webview_window(MAIN_WINDOW)
        .ok_or_else(|| BrowserError::Unavailable("The window is not open.".into()))?;
    evict(browser);
    let bounds = parked_rect();
    let weak = Arc::downgrade(browser);
    let passwords = browser.passwords();

    #[cfg(windows)]
    {
        let environment = HOST.with_borrow(|host| host.environment.clone());
        let webview = match environment {
            Some(environment) => {
                use wry::WebViewBuilderExtWindows;
                configure(WebViewBuilder::new(), &weak, tab_id, chat_id, url, &bounds)
                    .with_environment(environment)
                    .build_as_child(&window)
            }
            None => with_context(&profile, |context| {
                configure(
                    WebViewBuilder::new_with_web_context(context),
                    &weak,
                    tab_id,
                    chat_id,
                    url,
                    &bounds,
                )
                .build_as_child(&window)
            }),
        }
        .map_err(|error| BrowserError::Unavailable(format!("The page could not open: {error}")))?;
        {
            use wry::WebViewExtWindows;
            let environment = webview.environment();
            HOST.with_borrow_mut(|host| {
                host.environment.get_or_insert(environment);
            });
        }
        super::webview2::attach(&webview, &weak, tab_id, passwords);
        Ok(webview)
    }

    #[cfg(target_os = "macos")]
    {
        // WebKit keeps the profile's data by the store identifier below, not by the profile.
        let _ = (passwords, profile);
        let builder = configure(WebViewBuilder::new(), &weak, tab_id, chat_id, url, &bounds);
        use wry::WebViewBuilderExtDarwin;
        let ended = weak.clone();
        let tab = tab_id.to_owned();
        builder
            .with_data_store_identifier(*b"auracode-browser")
            .with_back_forward_navigation_gestures(true)
            // A page whose content process ended shows Reload, as on Windows and Linux. Never
            // from inside WebKit's own callback: the page closes on a later turn.
            .with_on_web_content_process_terminate_handler(move || {
                let Some(browser) = ended.upgrade() else {
                    return;
                };
                let tab = tab.clone();
                let app = browser.app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = app.run_on_main_thread(move || crashed(&browser, &tab, false));
                });
            })
            .build_as_child(&window)
            .map_err(|error| BrowserError::Unavailable(format!("The page could not open: {error}")))
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = passwords;
        // On Linux the page loads only once its sound and video are settled, so no document of
        // it plays what this system cannot (`media`).
        #[cfg(target_os = "linux")]
        let media = super::media::available()
            && !lock(&browser.inner)
                .tabs
                .get(tab_id)
                .is_some_and(|tab| tab.media_off);
        let first = if cfg!(target_os = "linux") { None } else { url };
        let webview = build_in_window(&profile, &window, &weak, tab_id, chat_id, first, &bounds)
            .map_err(|error| {
                BrowserError::Unavailable(format!("The page could not open: {error}"))
            })?;
        #[cfg(target_os = "linux")]
        {
            use wry::WebViewExtUnix;
            let page = webview.webview();
            if !media {
                super::media::turn_off(&page);
            }
            let ended = weak.clone();
            let tab = tab_id.to_owned();
            super::media::watch(&page, move || {
                if let Some(browser) = ended.upgrade() {
                    crashed(&browser, &tab, false);
                }
            });
            if let Some(url) = url {
                webview.load_url(url).map_err(|error| {
                    BrowserError::Page(format!("The page could not load: {error}"))
                })?;
            }
        }
        Ok(webview)
    }
}

/// Builds a page in the window off Windows and macOS: on Wayland a widget in a holder over the
/// app's page, placed by the holder; on X11 a child window, placed by its own bounds.
#[cfg(not(any(windows, target_os = "macos")))]
fn build_in_window(
    profile: &super::Profile,
    window: &tauri::WebviewWindow,
    weak: &Weak<Browser>,
    tab_id: &str,
    chat_id: &str,
    url: Option<&str>,
    bounds: &BrowserRect,
) -> wry::Result<wry::WebView> {
    #[cfg(target_os = "linux")]
    let holder = super::gtk_layer::installed()
        .then(|| super::gtk_layer::holder((bounds.width, bounds.height)))
        .flatten();
    #[cfg(not(target_os = "linux"))]
    let holder: Option<()> = None;
    let inside = BrowserRect {
        x: 0.0,
        y: 0.0,
        ..*bounds
    };
    let built = with_context(profile, |context| {
        let builder = configure(
            WebViewBuilder::new_with_web_context(context),
            weak,
            tab_id,
            chat_id,
            url,
            if holder.is_some() { &inside } else { bounds },
        );
        #[cfg(target_os = "linux")]
        if let Some(holder) = &holder {
            use wry::WebViewBuilderExtUnix;
            return builder.build_gtk(holder);
        }
        builder.build_as_child(window)
    });
    #[cfg(target_os = "linux")]
    if let Some(holder) = &holder {
        super::gtk_layer::adopt(holder, built.as_ref().ok());
    }
    built
}

/// Moves a page to `at`.
fn set_bounds(view: &View, at: &BrowserRect) {
    #[cfg(target_os = "linux")]
    if super::gtk_layer::place(&view.webview, at) {
        return;
    }
    let _ = view.webview.set_bounds(rect(at));
}

/// Shows or hides a page.
fn set_visible(view: &View, visible: bool) {
    #[cfg(target_os = "linux")]
    if super::gtk_layer::set_visible(&view.webview, visible) {
        return;
    }
    let _ = view.webview.set_visible(visible);
}

#[cfg(not(target_os = "macos"))]
fn with_context<T>(profile: &super::Profile, build: impl FnOnce(&mut wry::WebContext) -> T) -> T {
    let folder = profile.dir.join("webview");
    CONTEXT.with_borrow_mut(|context| {
        if !matches!(context, Some((dir, _)) if *dir == folder) {
            *context = None;
        }
        let (_, web) = context.get_or_insert_with(|| {
            let _ = std::fs::create_dir_all(&folder);
            (folder.clone(), wry::WebContext::new(Some(folder.clone())))
        });
        build(web)
    })
}

/// The switches the browser's engine starts with on Windows: wry's own, without turning off
/// SmartScreen, and pages keep running while hidden or covered so an agent's page keeps working.
#[cfg(windows)]
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows --disable-renderer-backgrounding";

/// The engine's switches. A development build launched for automation may also open the
/// browser's DevTools port (`GHOST_BROWSER_DEBUG_PORT`), so checks can drive real pages;
/// release builds never read it.
#[cfg(windows)]
fn browser_args() -> String {
    #[cfg(debug_assertions)]
    if let Some(port) = std::env::var("GHOST_BROWSER_DEBUG_PORT")
        .ok()
        .and_then(|port| port.trim().parse::<u16>().ok())
        .filter(|port| *port >= 1024)
    {
        return format!("{BROWSER_ARGS} --remote-debugging-port={port}");
    }
    BROWSER_ARGS.to_owned()
}

fn configure<'a>(
    builder: WebViewBuilder<'a>,
    weak: &Weak<Browser>,
    tab_id: &str,
    chat_id: &str,
    url: Option<&str>,
    bounds: &BrowserRect,
) -> WebViewBuilder<'a> {
    let mut builder = builder
        .with_bounds(rect(bounds))
        .with_visible(false)
        .with_focused(false)
        .with_devtools(true)
        .with_hotkeys_zoom(true)
        .with_clipboard(true)
        .with_initialization_script(PAGE_SCRIPT)
        .with_general_autofill_enabled(true);
    #[cfg(windows)]
    {
        use wry::WebViewBuilderExtWindows;
        builder = builder
            .with_additional_browser_args(browser_args())
            .with_browser_accelerator_keys(true)
            .with_default_context_menus(true)
            .with_scroll_bar_style(wry::ScrollBarStyle::FluentOverlay);
    }
    if let Some(url) = url {
        builder = builder.with_url(url);
    }

    let browser = weak.clone();
    let tab = tab_id.to_owned();
    builder = builder.with_ipc_handler(move |request| {
        if let Some(browser) = browser.upgrade() {
            browser.page_message(&tab, &request.uri().to_string(), request.body());
        }
    });

    let browser = weak.clone();
    let tab = tab_id.to_owned();
    let chat = chat_id.to_owned();
    builder = builder.with_navigation_handler(move |url| {
        let Some(browser) = browser.upgrade() else {
            return false;
        };
        match address::policy(&url) {
            address::Policy::Open => true,
            address::Policy::Outside => {
                browser.emit(BrowserEvent::Outside {
                    chat_id: chat.clone(),
                    tab_id: tab.clone(),
                    url,
                });
                false
            }
            address::Policy::Refuse => false,
        }
    });

    let browser = weak.clone();
    let tab = tab_id.to_owned();
    builder = builder.with_document_title_changed_handler(move |title| {
        if let Some(browser) = browser.upgrade() {
            browser.page_titled(&tab, &title);
        }
    });

    let browser = weak.clone();
    let tab = tab_id.to_owned();
    builder = builder.with_on_page_load_handler(move |event, url| {
        let Some(browser) = browser.upgrade() else {
            return;
        };
        match event {
            PageLoadEvent::Started => browser.page_started(&tab, &url),
            // On Windows the engine's own completion event says more (whether a document came, and
            // why a load failed), so `webview2` reports the finish there.
            PageLoadEvent::Finished => {
                #[cfg(not(windows))]
                {
                    browser.page_finished(&tab, Some(&url), super::Finished::loaded());
                    browser.page_state(&tab, |tab| tab.can_back = tab.loads > 1);
                }
            }
        }
    });

    let browser = weak.clone();
    let tab = tab_id.to_owned();
    let chat = chat_id.to_owned();
    builder = builder.with_new_window_req_handler(move |url, _features| {
        // Only a person's click or key opens a window, as a browser's pop-up blocker has it; the
        // person may still open a blocked one.
        #[cfg(windows)]
        let person = super::webview2::asked_by_person(&tab, &url);
        #[cfg(not(windows))]
        let person = true;
        let Some(browser) = browser.upgrade() else {
            return NewWindowResponse::Deny;
        };
        match address::policy(&url) {
            address::Policy::Outside => {
                browser.emit(BrowserEvent::Outside {
                    chat_id: chat.clone(),
                    tab_id: tab.clone(),
                    url,
                });
                NewWindowResponse::Deny
            }
            address::Policy::Refuse => NewWindowResponse::Deny,
            address::Policy::Open if !person => {
                browser.emit(BrowserEvent::Blocked {
                    chat_id: chat.clone(),
                    tab_id: tab.clone(),
                    url,
                });
                NewWindowResponse::Deny
            }
            address::Policy::Open => popup(&browser, &chat, &tab, &url),
        }
    });

    let browser = weak.clone();
    let tab = tab_id.to_owned();
    let chat = chat_id.to_owned();
    builder = builder.with_download_started_handler(move |url, path| {
        if let Some(browser) = browser.upgrade() {
            browser.download_began(&tab, &url, path);
            browser.emit(BrowserEvent::Download {
                chat_id: chat.clone(),
                tab_id: tab.clone(),
                url,
                file: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
                state: DownloadState::Started,
            });
        }
        true
    });

    let browser = weak.clone();
    let tab = tab_id.to_owned();
    let chat = chat_id.to_owned();
    builder.with_download_completed_handler(move |url, path, success| {
        if let Some(browser) = browser.upgrade() {
            browser.download_ended(&tab, &url);
            browser.emit(BrowserEvent::Download {
                chat_id: chat.clone(),
                tab_id: tab.clone(),
                url,
                file: path.map(|path| path.to_string_lossy().into_owned()),
                state: if success {
                    DownloadState::Done
                } else {
                    DownloadState::Failed
                },
            });
        }
    })
}

/// A page asked for a new window (a link with `target=_blank`, `window.open`, a sign-in popup):
/// it opens as a tab of the same chat. On Windows the tab is the window the page asked for, so a
/// sign-in popup can still talk to its opener and close itself.
fn popup(browser: &Arc<Browser>, chat_id: &str, opener: &str, url: &str) -> NewWindowResponse {
    #[cfg(windows)]
    {
        let Ok(tab) = browser.open_tab(chat_id, Some(url.to_owned()), true) else {
            return NewWindowResponse::Deny;
        };
        browser.opened_by(&tab.id, opener);
        let weak = Arc::downgrade(browser);
        let environment = HOST.with_borrow(|host| host.environment.clone());
        // A popup needs an open profile; the environment above already carries its data.
        let (Some(environment), Some(window), Some(_)) = (
            environment,
            browser.app.get_webview_window(MAIN_WINDOW),
            lock(&browser.inner).profile.clone(),
        ) else {
            return NewWindowResponse::Deny;
        };
        evict(browser);
        let bounds = parked_rect();
        use wry::WebViewBuilderExtWindows;
        // No address of its own: the engine hands it the one the page asked for.
        let built = configure(
            WebViewBuilder::new(),
            &weak,
            &tab.id,
            chat_id,
            None,
            &bounds,
        )
        .with_environment(environment)
        .build_as_child(&window);
        match built {
            Ok(webview) => {
                use wry::WebViewExtWindows;
                super::webview2::attach(&webview, &weak, &tab.id, browser.passwords());
                let core = webview.webview();
                HOST.with_borrow_mut(|host| {
                    host.views.insert(
                        tab.id.clone(),
                        View {
                            webview,
                            parked: false,
                        },
                    );
                });
                browser.page_state(&tab.id, |tab| tab.live = true);
                NewWindowResponse::Create { webview: core }
            }
            Err(error) => {
                tracing::warn!(%error, "a popup's tab could not open");
                let _ = browser.close_tab(&tab.id);
                NewWindowResponse::Deny
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(tab) = browser.open_tab(chat_id, Some(url.to_owned()), true) {
            browser.opened_by(&tab.id, opener);
        }
        NewWindowResponse::Deny
    }
}

// -------------------------------------------------------------------------------- operations

pub(super) fn load(browser: &Arc<Browser>, tab_id: &str, url: &str) -> Result<(), BrowserError> {
    let fresh = !HOST.with_borrow(|host| host.views.contains_key(tab_id));
    // A new page loads its address as it is built.
    ensure(browser, tab_id)?;
    if fresh {
        restore_placement(tab_id);
        return Ok(());
    }
    with_view(tab_id, |view| view.webview.load_url(url))?
        .map_err(|error| BrowserError::Page(format!("The page could not load: {error}")))
}

pub(super) fn go(
    browser: &Arc<Browser>,
    tab_id: &str,
    action: GoAction,
) -> Result<(), BrowserError> {
    if action == GoAction::Reload && !HOST.with_borrow(|host| host.views.contains_key(tab_id)) {
        // A sleeping or crashed page reloads by being built again.
        ensure(browser, tab_id)?;
        restore_placement(tab_id);
        return Ok(());
    }
    with_view(tab_id, |view| {
        #[cfg(windows)]
        {
            super::webview2::go(&view.webview, action);
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let script = match action {
                GoAction::Back => "history.back()",
                GoAction::Forward => "history.forward()",
                GoAction::Reload => "location.reload()",
                GoAction::Stop => "window.stop()",
            };
            view.webview
                .evaluate_script(script)
                .map_err(|error| BrowserError::Page(error.to_string()))
        }
    })?
}

/// A page built after the panel placed its tab takes the panel's place.
fn restore_placement(tab_id: &str) {
    let placed = HOST.with_borrow(|host| host.placed.clone());
    if let Some((tab, rect, visible)) = placed
        && tab == tab_id
    {
        show(tab_id, Some(rect), visible);
    }
}

fn show(tab_id: &str, at: Option<BrowserRect>, visible: bool) {
    let hidden = HOST.with_borrow(|host| host.window_hidden);
    let _ = with_view(tab_id, |view| {
        if let Some(at) = at {
            set_bounds(view, &at);
        }
        set_visible(view, visible && !hidden && at.is_some());
    });
}

pub(super) fn place(
    browser: &Arc<Browser>,
    tab_id: Option<&str>,
    at: Option<BrowserRect>,
    visible: bool,
) -> Result<(), BrowserError> {
    let previous = HOST.with_borrow(|host| host.placed.as_ref().map(|(tab, _, _)| tab.clone()));
    let Some(tab_id) = tab_id else {
        // Nothing over the panel at all.
        HOST.with_borrow_mut(|host| host.placed = None);
        if let Some(previous) = previous {
            hide_or_park(browser, &previous);
        }
        return Ok(());
    };
    let Some(at) = at else {
        // Hiding names its own page: a chat switching away never hides the chat switched to,
        // whichever of the two calls arrives first.
        HOST.with_borrow_mut(|host| {
            if host
                .placed
                .as_ref()
                .is_some_and(|(tab, _, _)| tab == tab_id)
            {
                host.placed = None;
            }
        });
        hide_or_park(browser, tab_id);
        return Ok(());
    };
    HOST.with_borrow_mut(|host| host.size = Some((at.width, at.height)));
    if let Some(previous) = previous
        && previous != tab_id
    {
        hide_or_park(browser, &previous);
    }
    HOST.with_borrow_mut(|host| host.placed = Some((tab_id.to_owned(), at, visible)));
    let has_page = lock(&browser.inner)
        .tabs
        .get(tab_id)
        .is_some_and(|tab| tab.url.is_some() && !tab.crashed);
    if has_page {
        ensure(browser, tab_id)?;
    }
    if HOST.with_borrow(|host| host.views.contains_key(tab_id)) {
        HOST.with_borrow_mut(|host| {
            if let Some(view) = host.views.get_mut(tab_id) {
                view.parked = false;
            }
        });
        show(tab_id, Some(at), visible);
    }
    Ok(())
}

/// A page that is no longer over the panel hides, unless an agent works in it: then it keeps
/// drawing off-window.
fn hide_or_park(browser: &Arc<Browser>, tab_id: &str) {
    let busy = browser
        .tab_chat(tab_id)
        .is_some_and(|chat| browser.agent_busy(&chat));
    if busy {
        park(tab_id);
    } else {
        let _ = with_view(tab_id, |view| {
            set_visible(view, false);
        });
        HOST.with_borrow_mut(|host| {
            if let Some(view) = host.views.get_mut(tab_id) {
                view.parked = false;
            }
        });
    }
}

/// Keeps a page drawing while it is not over the panel, for an agent's screenshots and clicks.
pub(super) fn park(tab_id: &str) {
    let placed = HOST.with_borrow(|host| {
        host.placed
            .as_ref()
            .is_some_and(|(tab, _, visible)| tab == tab_id && *visible)
    });
    if placed {
        return;
    }
    let at = parked_rect();
    let _ = with_view(tab_id, |view| {
        set_bounds(view, &at);
        set_visible(view, true);
    });
    HOST.with_borrow_mut(|host| {
        if let Some(view) = host.views.get_mut(tab_id) {
            view.parked = true;
        }
    });
}

/// An agent finished: pages it kept drawing off-window hide again.
pub(super) fn unpark_idle(browser: &Arc<Browser>) {
    let parked: Vec<String> = HOST.with_borrow(|host| {
        host.views
            .iter()
            .filter(|(_, view)| view.parked)
            .map(|(id, _)| id.clone())
            .collect()
    });
    for tab in parked {
        let busy = browser
            .tab_chat(&tab)
            .is_some_and(|chat| browser.agent_busy(&chat));
        if !busy {
            hide_or_park(browser, &tab);
        }
    }
}

/// Builds the page an agent needs and keeps it drawing.
pub(super) fn prepare_for_agent(browser: &Arc<Browser>, tab_id: &str) -> Result<(), BrowserError> {
    ensure(browser, tab_id)?;
    park(tab_id);
    Ok(())
}

/// The window was minimized or restored: the page over the panel follows it.
pub(crate) fn window_shown(shown: bool) {
    let placed = HOST.with_borrow_mut(|host| {
        // Windows reports this on every resize and focus change: only a real change moves a page.
        if host.window_hidden != shown {
            return None;
        }
        host.window_hidden = !shown;
        host.placed.clone()
    });
    if let Some((tab, at, visible)) = placed {
        show(&tab, Some(at), visible);
    }
}

pub(super) fn eval(
    _browser: &Arc<Browser>,
    tab_id: &str,
    script: &str,
    sender: tokio::sync::oneshot::Sender<String>,
) -> Result<(), BrowserError> {
    let sender = Mutex::new(Some(sender));
    with_view(tab_id, |view| {
        view.webview
            .evaluate_script_with_callback(script, move |result| {
                if let Some(sender) = lock(&sender).take() {
                    let _ = sender.send(result);
                }
            })
            .map_err(|error| BrowserError::Page(error.to_string()))
    })?
}

pub(super) fn zoom(_browser: &Arc<Browser>, tab_id: &str, zoom: f64) -> Result<(), BrowserError> {
    match with_view(tab_id, |view| view.webview.zoom(zoom)) {
        Ok(result) => result.map_err(|error| BrowserError::Page(error.to_string())),
        // A sleeping page takes its zoom when it is built.
        Err(BrowserError::NoTab(_)) => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn devtools(_browser: &Arc<Browser>, tab_id: &str) -> Result<(), BrowserError> {
    with_view(tab_id, |view| view.webview.open_devtools())
}

pub(super) fn focus(_browser: &Arc<Browser>, tab_id: &str) -> Result<(), BrowserError> {
    with_view(tab_id, |view| view.webview.focus())?
        .map_err(|error| BrowserError::Page(error.to_string()))
}

/// The engine's own word on where a page is, after a single-page app moved.
pub(super) fn refresh_location(browser: &Arc<Browser>, tab_id: &str) {
    let Ok(Ok(url)) = with_view(tab_id, |view| view.webview.url()) else {
        return;
    };
    if address::policy(&url) == address::Policy::Open && url != "about:blank" {
        browser.page_state(tab_id, |tab| tab.url = Some(url));
    }
    #[cfg(windows)]
    let _ = with_view(tab_id, |view| {
        super::webview2::refresh(browser, tab_id, &view.webview)
    });
}

pub(super) fn close(browser: &Arc<Browser>, tab_id: &str) {
    let view = HOST.with_borrow_mut(|host| {
        if host
            .placed
            .as_ref()
            .is_some_and(|(tab, _, _)| tab == tab_id)
        {
            host.placed = None;
        }
        host.views.remove(tab_id)
    });
    drop(view);
    #[cfg(windows)]
    super::webview2::forget(tab_id);
    let _ = browser;
}

/// Every page closes (the profile changes, the plugin turns off, Ghost quits). The engine
/// itself goes with the last page.
pub(super) fn close_all(browser: &Arc<Browser>) {
    let views: Vec<View> = HOST.with_borrow_mut(|host| {
        host.placed = None;
        host.views.drain().map(|(_, view)| view).collect()
    });
    let ids: Vec<String> = lock(&browser.inner).tabs.keys().cloned().collect();
    drop(views);
    for id in ids {
        #[cfg(windows)]
        super::webview2::forget(&id);
        browser.page_state(&id, |tab| {
            tab.live = false;
            tab.loading = false;
            tab.annotating = false;
            tab.downloads.clear();
        });
    }
    HOST.with_borrow_mut(|host| {
        #[cfg(windows)]
        {
            host.environment = None;
        }
        let _ = host;
    });
    CONTEXT.with_borrow_mut(|context| *context = None);
}

pub(super) fn apply_passwords(browser: &Arc<Browser>, enabled: bool) {
    #[cfg(windows)]
    HOST.with_borrow(|host| {
        for view in host.views.values() {
            super::webview2::passwords(&view.webview, enabled);
        }
    });
    let _ = (browser, enabled);
}

pub(super) fn clear(browser: &Arc<Browser>, data: ClearData) -> Result<(), BrowserError> {
    // The engine clears through a page of the profile; one is built for it when none is open.
    let any = HOST.with_borrow(|host| host.views.keys().next().cloned());
    let temporary = if any.is_none() {
        let id = "clearing".to_owned();
        let profile = lock(&browser.inner).profile.clone();
        let Some(profile) = profile else {
            return Ok(());
        };
        let window = browser
            .app
            .get_webview_window(MAIN_WINDOW)
            .ok_or_else(|| BrowserError::Unavailable("The window is not open.".into()))?;
        let bounds = parked_rect();
        let weak = Arc::downgrade(browser);
        #[cfg(windows)]
        let built = {
            let environment = HOST.with_borrow(|host| host.environment.clone());
            match environment {
                Some(environment) => {
                    use wry::WebViewBuilderExtWindows;
                    configure(WebViewBuilder::new(), &weak, &id, "", None, &bounds)
                        .with_environment(environment)
                        .build_as_child(&window)
                }
                None => with_context(&profile, |context| {
                    configure(
                        WebViewBuilder::new_with_web_context(context),
                        &weak,
                        &id,
                        "",
                        None,
                        &bounds,
                    )
                    .build_as_child(&window)
                }),
            }
        };
        #[cfg(target_os = "macos")]
        let built = {
            // WebKit keeps the profile's data by the store identifier below, not by the profile.
            let _ = profile;
            use wry::WebViewBuilderExtDarwin;
            configure(WebViewBuilder::new(), &weak, &id, "", None, &bounds)
                .with_data_store_identifier(*b"auracode-browser")
                .build_as_child(&window)
        };
        // On Wayland a hidden widget in the overlay: Wayland has no child windows.
        #[cfg(not(any(windows, target_os = "macos")))]
        let built = build_in_window(&profile, &window, &weak, &id, "", None, &bounds);
        Some(built.map_err(|error| BrowserError::Unavailable(error.to_string()))?)
    } else {
        None
    };
    let result = match (&temporary, any) {
        (Some(webview), _) => clear_with(webview, data),
        (None, Some(tab)) => with_view(&tab, |view| clear_with(&view.webview, data))?,
        (None, None) => Ok(()),
    };
    drop(temporary);
    result
}

fn clear_with(webview: &wry::WebView, data: ClearData) -> Result<(), BrowserError> {
    #[cfg(windows)]
    {
        super::webview2::clear(webview, data);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        // WebKit clears a profile's data all at once.
        if data.cookies || data.cache {
            webview
                .clear_all_browsing_data()
                .map_err(|error| BrowserError::Page(error.to_string()))?;
        }
        Ok(())
    }
}

/// A screenshot of a tab's page, base64 (JPEG for the view, PNG otherwise), where the engine can
/// take one (Windows, Linux); `None` elsewhere.
pub(super) async fn screenshot(
    browser: &Arc<Browser>,
    tab_id: &str,
    shot: Shot,
) -> Result<Option<String>, BrowserError> {
    #[cfg(windows)]
    {
        super::webview2::screenshot(browser, tab_id, shot)
            .await
            .map(Some)
    }
    #[cfg(target_os = "linux")]
    {
        super::snapshot::screenshot(browser, tab_id, shot)
            .await
            .map(Some)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (browser, tab_id, shot);
        Ok(None)
    }
}

/// Asks the engine for a picture of a tab's page (Linux); `sender` gets it.
#[cfg(target_os = "linux")]
pub(super) fn snapshot(
    tab_id: &str,
    region: super::snapshot::Region,
    sender: tokio::sync::oneshot::Sender<Result<super::snapshot::Picture, BrowserError>>,
) -> Result<(), BrowserError> {
    with_view(tab_id, |view| {
        use wry::WebViewExtUnix;
        super::snapshot::take(&view.webview.webview(), region, sender);
    })
}

/// Runs a DevTools protocol method in a tab's page (Windows).
#[cfg(windows)]
pub(super) fn devtools_call(
    tab_id: &str,
    method: &str,
    params: &str,
    sender: tokio::sync::oneshot::Sender<Result<String, String>>,
) -> Result<(), BrowserError> {
    with_view(tab_id, |view| {
        super::webview2::call(&view.webview, method, params, sender);
    })
}

/// A popup asked to close itself (`window.close()` after a sign-in): its tab closes.
#[cfg(windows)]
pub(super) fn close_requested(browser: &Arc<Browser>, tab_id: &str) {
    browser.close_requested(tab_id);
}

/// A page's process ended. Its page closes but keeps its place over the panel, so Reload builds
/// it again where it was; when the whole engine ended (`whole`, Windows), every page goes and the
/// next one starts a new engine. On Linux a tab whose page ends again within
/// [`super::media::CRASH_WINDOW`] loads with sound and video off from then on.
pub(super) fn crashed(browser: &Arc<Browser>, tab_id: &str, whole: bool) {
    let gone: Vec<(String, View)> = HOST.with_borrow_mut(|host| {
        if whole {
            #[cfg(windows)]
            {
                host.environment = None;
            }
            host.views.drain().collect()
        } else {
            host.views
                .remove(tab_id)
                .map(|view| vec![(tab_id.to_owned(), view)])
                .unwrap_or_default()
        }
    });
    if whole {
        CONTEXT.with_borrow_mut(|context| *context = None);
    }
    let ids: Vec<String> = gone.iter().map(|(id, _)| id.clone()).collect();
    drop(gone);
    for id in ids {
        #[cfg(windows)]
        super::webview2::forget(&id);
        browser.page_state(&id, |tab| {
            tab.live = false;
            tab.loading = false;
            tab.annotating = false;
            tab.crashed = tab.url.is_some();
            tab.downloads.clear();
            #[cfg(target_os = "linux")]
            {
                let now = std::time::Instant::now();
                if tab
                    .ended_at
                    .is_some_and(|at| now.duration_since(at) < super::media::CRASH_WINDOW)
                {
                    tab.media_off = true;
                }
                tab.ended_at = Some(now);
            }
        });
    }
}
