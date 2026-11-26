//! Linux: whether the browser's engine can play sound and video here, and the pages that must not
//! try when it cannot.
//!
//! WebKitGTK plays media through GStreamer, and a page process that needs an element GStreamer
//! cannot find aborts. On 17 November 2026 every page with sound or video took its process down in
//! the installed AppImage ("GStreamer element autoaudiosink not found", then SIGABRT inside
//! WebKit): the image carried GStreamer's libraries but none of its plugins, and its launcher hid
//! the system's. The window's own page runs the same risk, since Settings' Sounds play through Web
//! Audio.
//!
//! So Ghost decides once, at startup, whether the plugin files WebKit's media needs are in the
//! folders GStreamer will search, by GStreamer's own rules ([`plugin_dirs`]). It never starts
//! GStreamer in its own process for that: the first registry scan is slow and writes a registry.
//! When the plugins are missing, every page starts with audio, video, Web Audio, Media Source,
//! cameras and microphones, WebRTC and encrypted media off, and Settings and the browser say why
//! ([`notice`]). A browser tab whose page process still ends twice within [`CRASH_WINDOW`] loads
//! again with them off (`host::crashed`), and the window's own page loads again with them off after
//! its process ends ([`guard_window`]).
//!
//! An AppImage built without plugins runs with the system's instead: `main` points GStreamer at
//! them before anything starts ([`launch_env`]).

use std::cell::Cell;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use webkit2gtk::{SettingsExt, WebProcessTerminationReason, WebViewExt};

use crate::os::os_release_field;

/// A page whose process ends twice within this loads again with sound and video off.
pub const CRASH_WINDOW: Duration = Duration::from_secs(30);

/// The plugins WebKit's media cannot do without: GStreamer's core elements; the base plugins' app
/// source and sink, audio conversion and resampling, playback and type finding; and the good
/// plugins' automatic audio output. A page that plays anything without one of them aborts.
pub const REQUIRED: &[&str] = &[
    "coreelements",
    "app",
    "audioconvert",
    "audioresample",
    "autodetect",
    "playback",
    "typefindfunctions",
];

/// GStreamer looks this many folders deep into a plugin folder.
const DEPTH: u32 = 10;

/// Folder entries the search reads at most, so a plugin path naming something huge cannot hold up
/// startup.
const BUDGET: usize = 20_000;

/// The largest GStreamer library the version check reads.
const LIBRARY_LIMIT: u64 = 32 * 1024 * 1024;

/// What Settings says once the window's page has lost its process.
const WINDOW_RESTARTED: &str =
    "Sounds are off until Ghost restarts, because the window's page stopped working once.";

/// What startup found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// The folders GStreamer searches for plugins, in its order.
    pub dirs: Vec<PathBuf>,
    /// The [`REQUIRED`] plugins none of them holds.
    pub missing: Vec<&'static str>,
    /// An AppImage without plugins of its own loads the system's with its own GStreamer, which
    /// refuses a plugin built for a newer minor version than itself: both minor versions (the
    /// image's, the system's) when the system's is newer.
    pub older: Option<(u32, u32)>,
}

impl Detection {
    pub fn available(&self) -> bool {
        self.missing.is_empty() && self.older.is_none()
    }
}

/// Decides what this system can play. `var` reads the environment; `root` is where the system's
/// folders are (`/`, or a stand-in in tests).
pub fn detect(var: &dyn Fn(&str) -> Option<OsString>, root: &Path) -> Detection {
    let dirs = plugin_dirs(var, root);
    let missing = missing(&dirs, REQUIRED);
    let older = if missing.is_empty() {
        older(var, root)
    } else {
        None
    };
    Detection {
        dirs,
        missing,
        older,
    }
}

/// The folders GStreamer searches for plugins, as its registry decides them: `GST_PLUGIN_PATH_1_0`
/// (else `GST_PLUGIN_PATH`) first, then `GST_PLUGIN_SYSTEM_PATH_1_0` (else
/// `GST_PLUGIN_SYSTEM_PATH`), or, when neither system variable is set, the person's own plugin
/// folder and the system's. A system variable that is set replaces the defaults, even when empty.
pub fn plugin_dirs(var: &dyn Fn(&str) -> Option<OsString>, root: &Path) -> Vec<PathBuf> {
    let mut dirs = var("GST_PLUGIN_PATH_1_0")
        .or_else(|| var("GST_PLUGIN_PATH"))
        .map(|list| entries(&list))
        .unwrap_or_default();
    match var("GST_PLUGIN_SYSTEM_PATH_1_0").or_else(|| var("GST_PLUGIN_SYSTEM_PATH")) {
        Some(list) => dirs.extend(entries(&list)),
        None => {
            dirs.extend(user_plugin_dir(var));
            dirs.extend(system_plugin_dir(root));
        }
    }
    dirs
}

/// A search path's folders. GStreamer skips empty entries, as an AppImage launcher leaves them.
fn entries(list: &OsStr) -> Vec<PathBuf> {
    std::env::split_paths(list)
        .filter(|path| !path.as_os_str().is_empty())
        .collect()
}

/// `~/.local/share/gstreamer-1.0/plugins` (by `XDG_DATA_HOME`), searched before the system's.
fn user_plugin_dir(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let data = var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            var("HOME")
                .map(PathBuf::from)
                .filter(|home| home.is_absolute())
                .map(|home| home.join(".local").join("share"))
        })?;
    Some(data.join("gstreamer-1.0").join("plugins"))
}

/// The system's plugin folder as a GStreamer built for this system has it: Debian's multiarch
/// folder, else `lib64` (Fedora, openSUSE), else `lib` (Arch, Alpine), whichever exists first.
pub fn system_plugin_dir(root: &Path) -> Option<PathBuf> {
    lib_dirs(root)
        .into_iter()
        .map(|lib| lib.join("gstreamer-1.0"))
        .find(|dir| dir.is_dir())
}

/// The system's library folders, in the order [`system_plugin_dir`] tries them.
fn lib_dirs(root: &Path) -> Vec<PathBuf> {
    let usr = root.join("usr");
    let mut dirs = Vec::with_capacity(3);
    if let Some(triple) = multiarch() {
        dirs.push(usr.join("lib").join(triple));
    }
    dirs.push(usr.join("lib64"));
    dirs.push(usr.join("lib"));
    dirs
}

/// Debian's multiarch name for this build's architecture.
fn multiarch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64-linux-gnu"),
        "aarch64" => Some("aarch64-linux-gnu"),
        "x86" => Some("i386-linux-gnu"),
        "arm" => Some("arm-linux-gnueabihf"),
        "riscv64" => Some("riscv64-linux-gnu"),
        _ => None,
    }
}

/// Which of `names` no folder in `dirs` holds as `libgst<name>.so`, looking into subfolders as
/// GStreamer does (ten deep, never into `.debug`), within a budget of folder entries.
pub fn missing(dirs: &[PathBuf], names: &[&'static str]) -> Vec<&'static str> {
    let mut found = HashSet::new();
    let mut budget = BUDGET;
    for dir in dirs {
        collect(dir, DEPTH, &mut budget, &mut found);
    }
    names
        .iter()
        .copied()
        .filter(|name| !found.contains(OsStr::new(&format!("libgst{name}.so"))))
        .collect()
}

fn collect(dir: &Path, depth: u32, budget: &mut usize, found: &mut HashSet<OsString>) {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in listing.flatten() {
        let Some(left) = budget.checked_sub(1) else {
            return;
        };
        *budget = left;
        let Ok(mut kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_symlink() {
            // A link counts as what it points at, as GStreamer has it.
            let Ok(target) = std::fs::metadata(&path) else {
                continue;
            };
            kind = target.file_type();
        }
        if kind.is_file() {
            found.insert(entry.file_name());
        } else if kind.is_dir() && depth > 0 && entry.file_name() != ".debug" {
            collect(&path, depth - 1, budget, found);
        }
    }
}

/// The AppImage's mount, when Ghost runs from one.
fn appdir(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    var("APPDIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute() && dir.parent().is_some())
}

/// Whether an AppImage carries GStreamer plugins of its own.
fn carries_plugins(appdir: &Path) -> bool {
    appdir.join("usr/lib/gstreamer-1.0").is_dir()
}

/// For an AppImage without plugins of its own: the minor versions of its GStreamer and the
/// system's, when the system's is newer (see [`Detection::older`]). `None` when either version
/// cannot be told, and the page process's own guard (`CRASH_WINDOW`) covers that case.
fn older(var: &dyn Fn(&str) -> Option<OsString>, root: &Path) -> Option<(u32, u32)> {
    let appdir = appdir(var)?;
    if carries_plugins(&appdir) {
        return None;
    }
    let image = read_library(&appdir.join("usr/lib/libgstreamer-1.0.so.0"))
        .and_then(|bytes| named_minor(&bytes))?;
    let system = lib_dirs(root).into_iter().find_map(|lib| {
        let library = lib.join("libgstreamer-1.0.so.0");
        let real = std::fs::canonicalize(&library).ok()?;
        real.file_name()
            .and_then(OsStr::to_str)
            .and_then(soname_minor)
            .or_else(|| read_library(&real).and_then(|bytes| named_minor(&bytes)))
    })?;
    (system > image).then_some((image, system))
}

fn read_library(path: &Path) -> Option<Vec<u8>> {
    let size = std::fs::metadata(path).ok()?.len();
    (size <= LIBRARY_LIMIT)
        .then(|| std::fs::read(path).ok())
        .flatten()
}

/// GStreamer's minor version from its library's file name: `libgstreamer-1.0.so.0.2808.0` is
/// 1.28.8 (libtool's current is a hundred times the minor version plus the micro).
pub fn soname_minor(name: &str) -> Option<u32> {
    let (current, _) = name
        .strip_prefix("libgstreamer-1.0.so.0.")?
        .split_once('.')?;
    current.parse::<u32>().ok().map(|current| current / 100)
}

/// The newest `1.<minor>.<micro>` a GStreamer library names among its strings: its own version.
/// The others it names are older, the versions that brought something in.
pub fn named_minor(bytes: &[u8]) -> Option<u32> {
    bytes
        .split(|byte| *byte == 0)
        .filter_map(|text| {
            let mut parts = text.strip_prefix(b"1.")?.split(|byte| *byte == b'.');
            let minor = number(parts.next()?)?;
            number(parts.next()?)?;
            parts.next().is_none().then_some(minor)
        })
        .max()
}

fn number(digits: &[u8]) -> Option<u32> {
    (!digits.is_empty() && digits.len() <= 3 && digits.iter().all(u8::is_ascii_digit)).then(|| {
        digits
            .iter()
            .fold(0, |value, digit| value * 10 + u32::from(digit - b'0'))
    })
}

/// What `main` sets, before anything starts, when Ghost runs from an AppImage:
///
/// - GStreamer's registry in Ghost's own cache folder, unless the person chose one. The image's
///   plugins sit at a mount that moves with every launch, so in the shared registry they would make
///   the person's other GStreamer apps scan everything again after each launch.
/// - When the image carries no plugins: the system's plugin folders. The image's launcher puts its
///   own (missing) folder first in `GST_PLUGIN_SYSTEM_PATH_1_0`, which hides the system's; what
///   follows it is the person's own setting and stays.
///
/// `var` reads the environment; `root` is where the system's folders are (`/`, or a stand-in).
pub fn launch_env(
    var: &dyn Fn(&str) -> Option<OsString>,
    root: &Path,
) -> Vec<(&'static str, OsString)> {
    let Some(appdir) = appdir(var) else {
        return Vec::new();
    };
    let bundled = carries_plugins(&appdir);
    let mut set = Vec::new();
    if var("GST_REGISTRY_1_0").is_none()
        && var("GST_REGISTRY").is_none()
        && let Some(cache) = cache_dir(var)
    {
        let kind = if bundled { "image" } else { "system" };
        let file = format!("{kind}-{}.bin", std::env::consts::ARCH);
        set.push((
            "GST_REGISTRY_1_0",
            cache
                .join(ghost_core::paths::DEFAULT_FOLDER)
                .join("gstreamer-1.0")
                .join(file)
                .into_os_string(),
        ));
    }
    if !bundled {
        let theirs: Vec<PathBuf> = var("GST_PLUGIN_SYSTEM_PATH_1_0")
            .map(|list| entries(&list))
            .unwrap_or_default()
            .into_iter()
            .filter(|dir| !dir.starts_with(&appdir))
            .collect();
        let dirs: Vec<PathBuf> = if theirs.is_empty() {
            user_plugin_dir(var)
                .into_iter()
                .chain(system_plugin_dir(root))
                .collect()
        } else {
            theirs
        };
        if !dirs.is_empty()
            && let Ok(list) = std::env::join_paths(&dirs)
        {
            set.push(("GST_PLUGIN_SYSTEM_PATH_1_0", list));
        }
    }
    set
}

/// `$XDG_CACHE_HOME`, else `~/.cache`.
fn cache_dir(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            var("HOME")
                .map(PathBuf::from)
                .filter(|home| home.is_absolute())
                .map(|home| home.join(".cache"))
        })
}

/// What Settings and the browser say while sound and video are off, naming the packages to
/// install on this distribution (by its `os-release`) where Ghost knows them.
pub fn notice_for(found: &Detection, os_release: Option<&str>) -> Option<String> {
    if found.available() {
        return None;
    }
    if found.missing.is_empty() {
        return Some(
            "Audio and video are off here: this copy of Ghost was built without its own GStreamer plugins, and the system's are newer than it can load.".into(),
        );
    }
    let install = os_release.and_then(packages).map_or_else(
        || "Install them with your package manager, then restart Ghost.".to_owned(),
        |names| format!("Install {names}, then restart Ghost."),
    );
    Some(format!(
        "Audio and video are off here: GStreamer's base and good plugins are missing. {install}"
    ))
}

/// The packages that hold GStreamer's base and good plugins on a distribution, by its
/// `os-release` (`ID`, then the families in `ID_LIKE`).
pub fn packages(os_release: &str) -> Option<&'static str> {
    let id = os_release_field(os_release, "ID").unwrap_or_default();
    let like = os_release_field(os_release, "ID_LIKE").unwrap_or_default();
    let is = |family: &str| id == family || like.split_whitespace().any(|name| name == family);
    if is("arch") {
        Some("gst-plugins-base and gst-plugins-good")
    } else if is("debian") || is("ubuntu") {
        Some("gstreamer1.0-plugins-base and gstreamer1.0-plugins-good")
    } else if is("fedora") || is("rhel") || is("centos") {
        Some("gstreamer1-plugins-base and gstreamer1-plugins-good")
    } else if id.starts_with("opensuse") || is("suse") || is("opensuse") {
        Some("gstreamer-plugins-base and gstreamer-plugins-good")
    } else {
        None
    }
}

// ------------------------------------------------------------------------------- the pages

static DETECTION: OnceLock<Detection> = OnceLock::new();
static NOTICE: OnceLock<Option<String>> = OnceLock::new();

/// The window's own page lost its process once, and plays no sound from then on.
static WINDOW_OFF: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// When the window's own page last lost its process (the main thread's).
    static WINDOW_ENDED: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// What this system can play, decided on first use (at startup) and logged.
pub fn detection() -> &'static Detection {
    DETECTION.get_or_init(|| {
        let found = detect(&|name| std::env::var_os(name), Path::new("/"));
        if found.available() {
            tracing::info!(dirs = ?found.dirs, "GStreamer's plugins are here: pages may play sound and video");
        } else {
            tracing::warn!(
                missing = ?found.missing,
                older = ?found.older,
                dirs = ?found.dirs,
                "GStreamer's plugins are not usable here: every page starts with sound and video off"
            );
        }
        found
    })
}

/// Whether pages may play sound and video here.
pub fn available() -> bool {
    detection().available()
}

/// The sentence Settings and the browser show while sound and video are off here.
pub fn notice() -> Option<String> {
    NOTICE
        .get_or_init(|| {
            let release = ["/etc/os-release", "/usr/lib/os-release"]
                .iter()
                .find_map(|path| std::fs::read_to_string(path).ok());
            notice_for(detection(), release.as_deref())
        })
        .clone()
}

/// Whether the window's own page plays sound (Settings' Sounds), and why not.
pub fn window() -> (bool, Option<String>) {
    let off = WINDOW_OFF.load(Ordering::Acquire);
    let why = notice().or_else(|| off.then(|| WINDOW_RESTARTED.to_owned()));
    (available() && !off, why)
}

/// Turns a page's sound and video off: before its first load, so no document of it has them.
/// Where media works, pages keep WebKit's own defaults.
pub fn turn_off(page: &webkit2gtk::WebView) {
    let Some(settings) = WebViewExt::settings(page) else {
        return;
    };
    settings.set_enable_webaudio(false);
    settings.set_enable_media(false);
    settings.set_enable_media_stream(false);
    settings.set_enable_mediasource(false);
    settings.set_enable_webrtc(false);
    settings.set_enable_encrypted_media(false);
    settings.set_enable_media_capabilities(false);
}

/// Calls `ended` when a page's process ends (it crashed or ran out of memory), on the main loop's
/// next turn: never inside WebKit's own signal, since `ended` drops the page.
pub fn watch(page: &webkit2gtk::WebView, ended: impl Fn() + 'static) {
    let ended = Rc::new(ended);
    page.connect_web_process_terminated(move |_, reason| {
        if reason == WebProcessTerminationReason::TerminatedByApi {
            return;
        }
        tracing::warn!(?reason, "a browser page's process ended");
        let ended = Rc::clone(&ended);
        gtk::glib::idle_add_local_once(move || ended());
    });
}

/// The window's own page, at startup on the main thread and before its first document: sound and
/// video off where this system cannot play them, and a page whose process ends loads again
/// ([`window_ended`]).
pub fn guard_window(window: &tauri::WebviewWindow) {
    let Some(page) = window_page(window) else {
        tracing::warn!("the window's page was not found: its sound and its process go unwatched");
        return;
    };
    if !available() {
        turn_off(&page);
    }
    page.connect_web_process_terminated(|page, reason| {
        if reason == WebProcessTerminationReason::TerminatedByApi {
            return;
        }
        let page = page.clone();
        gtk::glib::idle_add_local_once(move || window_ended(&page, reason));
    });
}

/// The window's page among its column's children, before the browser's overlay moves it.
fn window_page(window: &tauri::WebviewWindow) -> Option<webkit2gtk::WebView> {
    use gtk::prelude::*;
    let column = window.default_vbox().ok()?;
    column
        .children()
        .into_iter()
        .find_map(|child| child.downcast::<webkit2gtk::WebView>().ok())
}

/// The window's page lost its process: it loads again with sound and video off (Settings says
/// so), unless it already did within [`CRASH_WINDOW`], which would only repeat.
fn window_ended(page: &webkit2gtk::WebView, reason: WebProcessTerminationReason) {
    let now = Instant::now();
    let before = WINDOW_ENDED.replace(Some(now));
    if before.is_some_and(|at| now.duration_since(at) < CRASH_WINDOW) {
        tracing::error!(
            ?reason,
            "the window's page stopped working again within 30 seconds; it is not loaded again"
        );
        return;
    }
    turn_off(page);
    WINDOW_OFF.store(true, Ordering::Release);
    tracing::warn!(
        ?reason,
        "the window's page stopped working; it loads again with sound and video off"
    );
    page.reload();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Env(HashMap<&'static str, OsString>);

    impl Env {
        fn new(pairs: &[(&'static str, &str)]) -> Env {
            Env(pairs
                .iter()
                .map(|(name, value)| (*name, OsString::from(value)))
                .collect())
        }

        fn var(&self) -> impl Fn(&str) -> Option<OsString> + '_ {
            |name| self.0.get(name).cloned()
        }
    }

    fn plugins(dir: &Path, names: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        for name in names {
            std::fs::write(dir.join(format!("libgst{name}.so")), b"").unwrap();
        }
    }

    const BASE_ONLY: &[&str] = &[
        "coreelements",
        "app",
        "audioconvert",
        "audioresample",
        "playback",
        "typefindfunctions",
    ];

    #[test]
    fn the_system_folder_is_the_first_of_multiarch_lib64_and_lib() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(system_plugin_dir(root.path()), None);
        let lib = root.path().join("usr/lib/gstreamer-1.0");
        std::fs::create_dir_all(&lib).unwrap();
        assert_eq!(system_plugin_dir(root.path()), Some(lib));
        let lib64 = root.path().join("usr/lib64/gstreamer-1.0");
        std::fs::create_dir_all(&lib64).unwrap();
        assert_eq!(system_plugin_dir(root.path()), Some(lib64));
        if let Some(triple) = multiarch() {
            let debian = root
                .path()
                .join("usr/lib")
                .join(triple)
                .join("gstreamer-1.0");
            std::fs::create_dir_all(&debian).unwrap();
            assert_eq!(system_plugin_dir(root.path()), Some(debian));
        }
    }

    #[test]
    fn plugin_dirs_follow_gstreamers_variables() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("usr/lib/gstreamer-1.0");
        std::fs::create_dir_all(&system).unwrap();

        // Nothing set: the person's folder, then the system's.
        let env = Env::new(&[("HOME", "/home/someone")]);
        assert_eq!(
            plugin_dirs(&env.var(), root.path()),
            [
                PathBuf::from("/home/someone/.local/share/gstreamer-1.0/plugins"),
                system.clone()
            ]
        );
        let env = Env::new(&[("HOME", "/home/someone"), ("XDG_DATA_HOME", "/data")]);
        assert_eq!(
            plugin_dirs(&env.var(), root.path())[0],
            PathBuf::from("/data/gstreamer-1.0/plugins")
        );

        // The extra path comes first; a system path replaces the defaults; empty entries go.
        let env = Env::new(&[
            ("HOME", "/home/someone"),
            ("GST_PLUGIN_PATH_1_0", "/extra::/more"),
            ("GST_PLUGIN_PATH", "/ignored"),
            (
                "GST_PLUGIN_SYSTEM_PATH_1_0",
                "/mnt/app/usr/lib/gstreamer-1.0:",
            ),
            ("GST_PLUGIN_SYSTEM_PATH", "/ignored-too"),
        ]);
        assert_eq!(
            plugin_dirs(&env.var(), root.path()),
            [
                PathBuf::from("/extra"),
                PathBuf::from("/more"),
                PathBuf::from("/mnt/app/usr/lib/gstreamer-1.0")
            ]
        );

        // The unversioned names count when the versioned ones are unset.
        let env = Env::new(&[
            ("GST_PLUGIN_PATH", "/extra"),
            ("GST_PLUGIN_SYSTEM_PATH", "/system"),
        ]);
        assert_eq!(
            plugin_dirs(&env.var(), root.path()),
            [PathBuf::from("/extra"), PathBuf::from("/system")]
        );

        // Set but empty: no system plugins at all, as GStreamer has it.
        let env = Env::new(&[
            ("HOME", "/home/someone"),
            ("GST_PLUGIN_SYSTEM_PATH_1_0", ""),
        ]);
        assert!(plugin_dirs(&env.var(), root.path()).is_empty());
    }

    #[test]
    fn detection_names_the_missing_plugins() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("usr/lib/gstreamer-1.0");
        // This machine on 17 November 2026: base's libraries, no good plugins.
        plugins(&system, BASE_ONLY);
        let env = Env::new(&[("HOME", "/nonexistent-home")]);
        let found = detect(&env.var(), root.path());
        assert_eq!(found.missing, ["autodetect"]);
        assert!(!found.available());

        plugins(&system, &["autodetect"]);
        let found = detect(&env.var(), root.path());
        assert!(found.available(), "{found:?}");

        // An AppImage's launcher hides the system's plugins behind its own missing folder.
        let image = tempfile::tempdir().unwrap();
        let hidden = format!("{}/usr/lib/gstreamer-1.0:", image.path().display());
        let env = Env::new(&[("GST_PLUGIN_SYSTEM_PATH_1_0", &hidden)]);
        assert_eq!(detect(&env.var(), root.path()).missing, REQUIRED);
    }

    #[test]
    fn plugins_count_in_subfolders_and_through_links_but_not_in_debug_folders() {
        let dir = tempfile::tempdir().unwrap();
        plugins(&dir.path().join("a/b"), &["app"]);
        plugins(&dir.path().join(".debug"), &["playback"]);
        plugins(&dir.path().join("elsewhere"), &["autodetect"]);
        std::os::unix::fs::symlink(
            dir.path().join("elsewhere/libgstautodetect.so"),
            dir.path().join("a/libgstautodetect.so"),
        )
        .unwrap();
        let found = missing(
            &[dir.path().join("a"), dir.path().to_path_buf()],
            &["app", "playback", "autodetect", "typefindfunctions"],
        );
        assert_eq!(found, ["playback", "typefindfunctions"]);
    }

    #[test]
    fn versions_read_from_names_and_strings() {
        assert_eq!(soname_minor("libgstreamer-1.0.so.0.2808.0"), Some(28));
        assert_eq!(soname_minor("libgstreamer-1.0.so.0.2003.0"), Some(20));
        assert_eq!(soname_minor("libgstreamer-1.0.so.0"), None);
        assert_eq!(soname_minor("libgstbase-1.0.so.0.2808.0"), None);
        let library = b"\x7fELF\0GStreamer %d.%d.%d\x001.23.90\0Initializing\x001.28.8\x001.4\x001.2.3.4\x00x1.99.0\0";
        assert_eq!(named_minor(library), Some(28));
        assert_eq!(named_minor(b"no versions here\0"), None);
    }

    /// An AppImage at `image`, with plugins of its own or none, carrying GStreamer `1.<minor>`.
    fn appimage(image: &Path, bundled: bool, minor: u32) {
        let lib = image.join("usr/lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(
            lib.join("libgstreamer-1.0.so.0"),
            format!("\0junk\x001.{minor}.2\0").as_bytes(),
        )
        .unwrap();
        if bundled {
            plugins(&lib.join("gstreamer-1.0"), REQUIRED);
        }
    }

    #[test]
    fn an_image_without_plugins_is_not_trusted_with_newer_system_plugins() {
        let root = tempfile::tempdir().unwrap();
        let lib = root.path().join("usr/lib");
        plugins(&lib.join("gstreamer-1.0"), REQUIRED);
        std::fs::write(lib.join("libgstreamer-1.0.so.0.2808.0"), b"").unwrap();
        std::os::unix::fs::symlink(
            "libgstreamer-1.0.so.0.2808.0",
            lib.join("libgstreamer-1.0.so.0"),
        )
        .unwrap();
        let system = lib.join("gstreamer-1.0").display().to_string();

        let older = tempfile::tempdir().unwrap();
        appimage(older.path(), false, 20);
        let appdir = older.path().display().to_string();
        let env = Env::new(&[("APPDIR", &appdir), ("GST_PLUGIN_SYSTEM_PATH_1_0", &system)]);
        let found = detect(&env.var(), root.path());
        assert_eq!(found.older, Some((20, 28)));
        assert!(!found.available());
        assert!(
            notice_for(&found, None)
                .unwrap()
                .contains("built without its own GStreamer plugins")
        );

        let same = tempfile::tempdir().unwrap();
        appimage(same.path(), false, 28);
        let appdir = same.path().display().to_string();
        let env = Env::new(&[("APPDIR", &appdir), ("GST_PLUGIN_SYSTEM_PATH_1_0", &system)]);
        assert!(detect(&env.var(), root.path()).available());

        // An image with its own plugins loads only those: no comparison.
        let bundled = tempfile::tempdir().unwrap();
        appimage(bundled.path(), true, 20);
        let appdir = bundled.path().display().to_string();
        let own = format!("{appdir}/usr/lib/gstreamer-1.0");
        let env = Env::new(&[("APPDIR", &appdir), ("GST_PLUGIN_SYSTEM_PATH_1_0", &own)]);
        assert!(detect(&env.var(), root.path()).available());
    }

    #[test]
    fn launch_env_points_an_image_without_plugins_at_the_systems() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("usr/lib/gstreamer-1.0");
        std::fs::create_dir_all(&system).unwrap();
        let image = tempfile::tempdir().unwrap();
        appimage(image.path(), false, 28);
        let appdir = image.path().display().to_string();
        let launcher = format!("{appdir}/usr/lib/gstreamer-1.0:");
        let env = Env::new(&[
            ("APPDIR", &appdir),
            ("HOME", "/home/someone"),
            ("GST_PLUGIN_SYSTEM_PATH_1_0", &launcher),
        ]);
        let set = launch_env(&env.var(), root.path());
        let registry = PathBuf::from("/home/someone/.cache")
            .join(ghost_core::paths::DEFAULT_FOLDER)
            .join(format!(
                "gstreamer-1.0/system-{}.bin",
                std::env::consts::ARCH
            ));
        assert_eq!(
            set,
            [
                ("GST_REGISTRY_1_0", registry.into_os_string()),
                (
                    "GST_PLUGIN_SYSTEM_PATH_1_0",
                    std::env::join_paths([
                        PathBuf::from("/home/someone/.local/share/gstreamer-1.0/plugins"),
                        system.clone()
                    ])
                    .unwrap()
                ),
            ]
        );

        // The person's own system path, after the launcher's, is kept; their registry too.
        let theirs = format!("{appdir}/usr/lib/gstreamer-1.0:/opt/gst/plugins");
        let env = Env::new(&[
            ("APPDIR", &appdir),
            ("HOME", "/home/someone"),
            ("GST_PLUGIN_SYSTEM_PATH_1_0", &theirs),
            ("GST_REGISTRY", "/tmp/their-registry.bin"),
        ]);
        assert_eq!(
            launch_env(&env.var(), root.path()),
            [(
                "GST_PLUGIN_SYSTEM_PATH_1_0",
                OsString::from("/opt/gst/plugins")
            )]
        );
    }

    #[test]
    fn launch_env_leaves_an_image_with_plugins_its_own_and_others_alone() {
        let root = tempfile::tempdir().unwrap();
        let image = tempfile::tempdir().unwrap();
        appimage(image.path(), true, 28);
        let appdir = image.path().display().to_string();
        let env = Env::new(&[("APPDIR", &appdir), ("XDG_CACHE_HOME", "/cache")]);
        let set = launch_env(&env.var(), root.path());
        assert_eq!(set.len(), 1, "{set:?}");
        assert_eq!(set[0].0, "GST_REGISTRY_1_0");
        assert!(
            PathBuf::from(&set[0].1).starts_with("/cache"),
            "{:?}",
            set[0].1
        );
        let env = Env::new(&[
            ("APPDIR", &appdir),
            ("XDG_CACHE_HOME", "/cache"),
            ("GST_REGISTRY_1_0", "/mine.bin"),
        ]);
        assert!(launch_env(&env.var(), root.path()).is_empty());

        // Not an AppImage (a deb, an rpm, a development build): nothing changes.
        let env = Env::new(&[("HOME", "/home/someone")]);
        assert!(launch_env(&env.var(), root.path()).is_empty());
        for odd in ["/", "relative/mount"] {
            let env = Env::new(&[("APPDIR", odd), ("HOME", "/home/someone")]);
            assert!(launch_env(&env.var(), root.path()).is_empty(), "{odd}");
        }
    }

    #[test]
    fn the_notice_names_this_distributions_packages() {
        let found = Detection {
            dirs: vec![],
            missing: vec!["autodetect"],
            older: None,
        };
        assert_eq!(
            notice_for(&found, Some("NAME=\"Omarchy\"\nID=omarchy\nID_LIKE=arch\n")).as_deref(),
            Some(
                "Audio and video are off here: GStreamer's base and good plugins are missing. Install gst-plugins-base and gst-plugins-good, then restart Ghost."
            )
        );
        for (release, names) in [
            (
                "ID=ubuntu\nID_LIKE=debian\n",
                "gstreamer1.0-plugins-base and",
            ),
            (
                "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n",
                "gstreamer1.0-plugins-good",
            ),
            (
                "ID=fedora\n",
                "gstreamer1-plugins-base and gstreamer1-plugins-good",
            ),
            (
                "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n",
                "gstreamer1-plugins-good",
            ),
            (
                "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n",
                "gstreamer-plugins-good",
            ),
            ("ID=gentoo\n", "with your package manager"),
        ] {
            let notice = notice_for(&found, Some(release)).unwrap();
            assert!(notice.contains(names), "{release:?}: {notice}");
        }
        assert!(
            notice_for(&found, None)
                .unwrap()
                .ends_with("Install them with your package manager, then restart Ghost.")
        );
        let fine = Detection {
            missing: vec![],
            ..found
        };
        assert_eq!(notice_for(&fine, Some("ID=arch\n")), None);
    }
}
