//! Wayland: the browser's pages as GTK widgets over the app's own page.
//!
//! wry's child webviews are X11 child windows, and Wayland has no counterpart: a window there is
//! one surface the toolkit draws. So on Wayland the pages join the window's widget tree. The app's
//! own page moves into a `gtk::Overlay`, and each live tab's WebKitGTK view is an overlay child,
//! in a holder of exactly the panel's rectangle. X11 keeps wry's child windows.
//!
//! The holder is the page's size on purpose. GDK draws the app's page with OpenGL and has to
//! upload, every frame, whatever GTK draws over it. One layer across the whole window made that
//! the whole window: the main process took 75 to 95 % of a core to show a small animation at 60
//! frames a second, against 13 % without the layer (4K at 2x, 16 November 2026). A holder the page
//! fills costs nothing beyond the page, and with no page shown nothing lies over the app at all.
//!
//! Everything here runs on the main thread, as the rest of the host does.
//!
//! The overlay also keeps keyboard focus on a page (see [`keep_focus`]).

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::prelude::*;

use super::BrowserRect;

thread_local! {
    static OVERLAY: RefCell<Option<gtk::Overlay>> = const { RefCell::new(None) };
    /// The app's own page.
    static PAGE: RefCell<Option<gtk::Widget>> = const { RefCell::new(None) };
    /// The page that owns keyboard focus: the app's own, or the browser page last clicked.
    static OWNER: RefCell<Option<gtk::Widget>> = const { RefCell::new(None) };
    /// Where each holder goes, in the overlay's own coordinates (logical pixels).
    static PLACES: RefCell<HashMap<gtk::Widget, gtk::gdk::Rectangle>> = RefCell::new(HashMap::new());
}

/// GDK's name for the display the app runs on: `GdkWaylandDisplay` or `GdkX11Display`.
pub(super) fn display_kind() -> Option<String> {
    gtk::gdk::Display::default().map(|display| display.type_().name().to_owned())
}

/// Moves the window's page into an overlay the browser's pages can join. Done once, before the
/// window shows, so the page is moved while nothing is drawn yet.
pub(super) fn install(window: &tauri::WebviewWindow) -> Result<(), String> {
    if OVERLAY.with_borrow(Option::is_some) {
        return Ok(());
    }
    let column = window.default_vbox().map_err(|error| error.to_string())?;
    let children = column.children();
    let index = children
        .iter()
        .position(|child| child.type_().name() == "WebKitWebView")
        .ok_or("the window has no page to lay the browser over")?;
    let page = children[index].clone();
    let overlay = gtk::Overlay::new();
    column.remove(&page);
    overlay.add(&page);
    overlay.connect_get_child_position(|_, holder| {
        PLACES.with_borrow(|places| places.get(holder).copied())
    });
    column.pack_start(&overlay, true, true, 0);
    column.reorder_child(&overlay, i32::try_from(index).unwrap_or(0));
    overlay.show();
    OVERLAY.set(Some(overlay));
    PAGE.set(Some(page.clone()));
    if let Ok(toplevel) = window.gtk_window() {
        keep_focus(toplevel.upcast_ref());
    }
    // Moving the page out of the window's tree left the window with no focus to give it back.
    page.grab_focus();
    Ok(())
}

/// Below this x a page is parked off the window ([`super::host`]): it draws, but is not shown.
const PARKED_BEFORE: i32 = -10_000;

/// Keyboard focus stays on a page (owner, 17 November 2026: after a key in a terminal, typing
/// stopped until a click). The window holds the app's page and, over it, each browser page in its
/// holder. GTK's own focus moves (its arrow and Tab key bindings, and a page passing focus on past
/// its last control) would carry focus to a browser page, shown or parked off the window, or to
/// nothing. So focus moves between pages only by a click (or a page's own focus call) into a page
/// on screen, and focus that ends on nothing, or on a page hidden or parked, returns to the page
/// that held it. `GHOST_FOCUS_PROBE=1` logs every focus change and key the window sees.
fn keep_focus(window: &gtk::Window) {
    // GTK moving focus between the window's widgets by keyboard: never. A page moves focus among
    // its own controls itself.
    window.connect_focus(|_, direction| {
        probe(|| format!("window focus move {direction:?} stopped"));
        gtk::glib::Propagation::Stop
    });
    window.connect_set_focus(|window, widget| {
        probe(|| format!("set-focus {}", describe(widget)));
        if let Some(widget) = widget
            && takes_focus(widget)
        {
            OWNER.set(Some(widget.clone()));
            return;
        }
        let window = window.clone();
        gtk::glib::idle_add_local_once(move || refocus(&window));
    });
    window.connect_is_active_notify(|window| {
        probe(|| {
            format!(
                "window {} with focus on {}",
                if window.is_active() {
                    "active"
                } else {
                    "inactive"
                },
                describe(window.focused_widget().as_ref())
            )
        });
        if window.is_active() {
            refocus(window);
        }
    });
    if probing() {
        window.connect_key_press_event(|window, event| {
            probe(|| {
                format!(
                    "key {:?} state {:#x} on {}",
                    event.keyval().name(),
                    event.state().bits(),
                    describe(window.focused_widget().as_ref())
                )
            });
            gtk::glib::Propagation::Proceed
        });
    }
}

/// The app's page, or a browser page whose holder is shown at a place in the window.
fn takes_focus(widget: &gtk::Widget) -> bool {
    if PAGE.with_borrow(|page| page.as_ref() == Some(widget)) {
        return true;
    }
    let Some(holder) = widget.parent() else {
        return false;
    };
    holder.is_visible()
        && PLACES.with_borrow(|places| {
            places
                .get(&holder)
                .is_some_and(|place| place.x() > PARKED_BEFORE)
        })
}

/// Gives focus back to the page that owns it when it is on nothing or on a page not shown.
fn refocus(window: &gtk::Window) {
    let current = window.focused_widget();
    if current.as_ref().is_some_and(takes_focus) {
        return;
    }
    let owner = OWNER
        .with_borrow(Clone::clone)
        .filter(takes_focus)
        .or_else(|| PAGE.with_borrow(Clone::clone));
    if let Some(owner) = owner {
        probe(|| format!("focus returned to {}", describe(Some(&owner))));
        owner.grab_focus();
    }
}

/// Before a browser page hides or goes, the app's page takes focus if that page held it.
fn release_focus(holder: &gtk::Widget) {
    let Some(window) = holder.toplevel().and_downcast::<gtk::Window>() else {
        return;
    };
    let held = window
        .focused_widget()
        .is_some_and(|focus| &focus == holder || focus.is_ancestor(holder));
    if held && let Some(page) = PAGE.with_borrow(Clone::clone) {
        OWNER.set(Some(page.clone()));
        page.grab_focus();
    }
}

fn probing() -> bool {
    static PROBING: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBING.get_or_init(|| std::env::var_os("GHOST_FOCUS_PROBE").is_some_and(|v| v == "1"))
}

fn probe(message: impl FnOnce() -> String) {
    if probing() {
        tracing::info!(target: "ghost::focus", "{}", message());
    }
}

fn describe(widget: Option<&gtk::Widget>) -> String {
    match widget {
        None => "nothing".into(),
        Some(widget) if PAGE.with_borrow(|page| page.as_ref() == Some(widget)) => {
            "the app's page".into()
        }
        Some(widget) => match widget.parent() {
            Some(holder) if PLACES.with_borrow(|places| places.contains_key(&holder)) => {
                let shown = takes_focus(widget);
                format!(
                    "a browser page ({})",
                    if shown { "shown" } else { "hidden or parked" }
                )
            }
            _ => widget.type_().name().to_owned(),
        },
    }
}

/// Whether the pages are widgets here (Wayland) rather than child windows (X11).
pub(super) fn installed() -> bool {
    OVERLAY.with_borrow(Option::is_some)
}

/// A hidden holder for one page of `size`, over the app's page. wry builds the page inside it
/// (`build_gtk`); [`adopt`] then ties the holder's life to the page's.
pub(super) fn holder(size: (f64, f64)) -> Option<gtk::Fixed> {
    let overlay = OVERLAY.with_borrow(Clone::clone)?;
    let holder = gtk::Fixed::new();
    let (width, height) = (whole(size.0).max(1), whole(size.1).max(1));
    PLACES.with_borrow_mut(|places| {
        places.insert(
            holder.clone().upcast(),
            gtk::gdk::Rectangle::new(0, 0, width, height),
        );
    });
    overlay.add_overlay(&holder);
    Some(holder)
}

/// Removes a page's holder when the page is destroyed (wry destroys it when the tab sleeps or
/// closes). Without a page there is nothing to remove, and the holder goes at once.
pub(super) fn adopt(holder: &gtk::Fixed, webview: Option<&wry::WebView>) {
    let Some(webview) = webview else {
        discard(holder.upcast_ref());
        return;
    };
    let holder = holder.clone();
    widget_of(webview).connect_destroy(move |_| discard(holder.upcast_ref()));
}

fn discard(holder: &gtk::Widget) {
    release_focus(holder);
    PLACES.with_borrow_mut(|places| places.remove(holder));
    if let Some(overlay) = OVERLAY.with_borrow(Clone::clone) {
        overlay.remove(holder);
    }
}

/// The page's holder, when the page lives in the overlay.
fn holder_of(webview: &wry::WebView) -> Option<(gtk::Widget, gtk::Widget)> {
    let widget = widget_of(webview);
    let holder = widget.parent()?;
    PLACES
        .with_borrow(|places| places.contains_key(&holder))
        .then_some((holder, widget))
}

fn widget_of(webview: &wry::WebView) -> gtk::Widget {
    use wry::WebViewExtUnix;
    webview.webview().upcast()
}

/// Moves a page to `at` (logical pixels in the window's page). `false` when the page is not in
/// the overlay (X11), where wry moves it.
pub(super) fn place(webview: &wry::WebView, at: &BrowserRect) -> bool {
    let Some((holder, widget)) = holder_of(webview) else {
        return false;
    };
    let (width, height) = (whole(at.width).max(1), whole(at.height).max(1));
    if whole(at.x) <= PARKED_BEFORE {
        release_focus(&holder);
    }
    PLACES.with_borrow_mut(|places| {
        places.insert(
            holder.clone(),
            gtk::gdk::Rectangle::new(whole(at.x), whole(at.y), width, height),
        );
    });
    widget.set_size_request(width, height);
    holder.queue_resize();
    true
}

/// Shows or hides a page with its holder. `false` when the page is not in the overlay.
pub(super) fn set_visible(webview: &wry::WebView, visible: bool) -> bool {
    let Some((holder, _)) = holder_of(webview) else {
        return false;
    };
    if visible {
        holder.show_all();
    } else {
        release_focus(&holder);
        holder.hide();
    }
    true
}

/// GTK takes whole pixels. `as` saturates a float out of range.
fn whole(value: f64) -> i32 {
    value.round() as i32
}
