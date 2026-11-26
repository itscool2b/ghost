//! Where Ghost keeps its data, and the rules for naming things inside it.
//!
//! ```text
//! <data root>/                 dirs::data_local_dir()/dev.onghost.desktop (a development build:
//!                              dev.onghost.desktop.dev), or $GHOST_DATA_DIR
//!   device.json                this machine's identity, shared by every profile
//!   state.json                 which profile is active
//!   profiles/<profile id>/     one per profile (see crate::profile)
//! ```
//!
//! The root is the *local* app data folder on purpose. On Windows the other choice, Roaming AppData
//! (`dirs::data_dir()`), is copied to a server at logon and logoff on managed machines (roaming
//! profiles, Folder Redirection) or kept on an SMB share. That would carry `device.json` to every PC
//! the user signs into (one device id for many machines), let the machine that logs off last
//! overwrite the others' databases, put WAL-mode SQLite on a network share where its locking is
//! unreliable, and copy gigabytes of history and checkpoints at every logon. On macOS and Linux both
//! functions return the same folder. If something ever has to roam, it must be small settings kept
//! outside `profiles/`, and never `device.json`.
//!
//! A development build (debug assertions on: `cargo run`, `tauri dev`, `tauri build --debug`) has a
//! default folder of its own, [`DEVELOPMENT_FOLDER`], next to the installed app's. A build from a
//! working copy is newer code than the installed app, and the installed app is often the one running
//! the session that changes that code. On the installed app's data, a newer schema would migrate
//! the database, which the installed app then refuses ([`crate::CoreError::SchemaTooNew`]), and the
//! data cleanup would delete its chats and the worktrees inside its folder. `GHOST_DATA_DIR`
//! still points a development build anywhere, the installed app's folder included.
//!
//! The app was o7 from 19 to 26 November 2026, with the folder `codes.o7.desktop`, and auracode
//! before that, with `si.auracode.desktop` ([`LEGACY_IDENTIFIERS`]). A copy updated from either
//! keeps using the folder its data is in, because moving it would break every Git worktree inside
//! it (each records its full path, and so does its repository) and every agent's own history of
//! those folders, which agents key by path. So the default root is the new folder when it holds
//! data, else the newest earlier folder that does, else the new one ([`DataRoot::resolve`]).
//! Holding data means a `device.json` or a `profiles` folder: a folder the platform made empty
//! never counts, so it cannot hide an earlier one.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};
use crate::fsutil;

/// The Tauri bundle identifier. Tauri's `app_local_data_dir()` is `dirs::data_local_dir()` joined
/// with this on macOS, Windows, and Linux, so resolving it here gives the desktop app (which must ask
/// Tauri for `app_local_data_dir()`, not `app_data_dir()`) and the CLI the same folder without
/// ghost-core depending on Tauri. It is also the folder name of the installed app's data root.
pub const APP_IDENTIFIER: &str = "dev.onghost.desktop";

/// The folder name of a development build's default data root (see the module docs).
pub const DEVELOPMENT_FOLDER: &str = "dev.onghost.desktop.dev";

/// The identifiers the app had before, newest first: as o7, then as auracode. Each is the folder
/// a copy from then keeps its data in (see the module docs).
pub const LEGACY_IDENTIFIERS: [&str; 2] = ["codes.o7.desktop", "si.auracode.desktop"];

/// A development build's folders from then, in the same order.
pub const LEGACY_DEVELOPMENT_FOLDERS: [&str; 2] =
    ["codes.o7.desktop.dev", "si.auracode.desktop.dev"];

/// The folders earlier versions used where this build would use `folder`, newest first.
pub const fn legacy_folders(folder: &str) -> &'static [&'static str] {
    match folder.as_bytes() {
        b"dev.onghost.desktop" => &LEGACY_IDENTIFIERS,
        b"dev.onghost.desktop.dev" => &LEGACY_DEVELOPMENT_FOLDERS,
        _ => &[],
    }
}

/// Whether this is a development build: debug assertions are on, as in the `dev` profile. Its
/// default data root is [`DEVELOPMENT_FOLDER`], never the installed app's.
pub const DEVELOPMENT_BUILD: bool = cfg!(debug_assertions);

/// The folder name of this build's default data root, inside the local app data folder.
pub const DEFAULT_FOLDER: &str = default_folder(DEVELOPMENT_BUILD);

/// The default data root's folder name for a development build or an installed (release) one.
pub const fn default_folder(development: bool) -> &'static str {
    if development {
        DEVELOPMENT_FOLDER
    } else {
        APP_IDENTIFIER
    }
}

/// Overrides the data root (tests, a second dev instance, a portable install). Blank values are
/// ignored, so an exported-but-empty variable can't silently move data into the working directory.
pub const DATA_DIR_ENV: &str = "GHOST_DATA_DIR";

/// The same override under the app's earlier names, newest first, still honoured when
/// [`DATA_DIR_ENV`] is not set: scripts and shortcuts made before a rename keep their folder. A
/// process that clears one clears them all.
pub const LEGACY_DATA_DIR_ENVS: [&str; 2] = ["O7_DATA_DIR", "AURACODE_DATA_DIR"];

/// The data root override from the environment: [`DATA_DIR_ENV`], else the first of
/// [`LEGACY_DATA_DIR_ENVS`] that names a folder.
fn data_dir_override() -> Option<OsString> {
    std::iter::once(DATA_DIR_ENV)
        .chain(LEGACY_DATA_DIR_ENVS)
        .filter_map(std::env::var_os)
        .find(|value| override_path(value).is_some())
}

/// Longest accepted profile id. UUIDs need 36; the headroom allows other id schemes later while
/// keeping paths short for Windows' MAX_PATH.
pub const MAX_PROFILE_ID_LEN: usize = 64;

const PROFILES_DIR: &str = "profiles";
const DEVICE_FILE: &str = "device.json";
const STATE_FILE: &str = "state.json";

/// Names Windows treats as devices in every directory (`profiles\con` is the console, not a folder).
/// They pass the character rules, so they are rejected explicitly to keep profile ids portable.
const WINDOWS_RESERVED_NAMES: [&str; 24] = [
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The root folder of all Ghost data on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRoot {
    root: PathBuf,
}

impl DataRoot {
    /// `$GHOST_DATA_DIR` when set to something non-blank, otherwise [`DEFAULT_FOLDER`] in the
    /// local app data folder: for an installed build the same folder Tauri's
    /// `app_local_data_dir()` returns for [`APP_IDENTIFIER`] (see the module docs for why it is the
    /// local folder, and why a development build has its own). A relative override is made
    /// absolute against the current directory now, so a later `chdir` can't move the data.
    ///
    /// Without an override, a copy updated from o7 or from auracode finds its data in the folder
    /// it used then (see the module docs).
    pub fn resolve() -> Result<DataRoot> {
        Self::resolve_from(data_dir_override(), platform_data_dir(), DEFAULT_FOLDER)
    }

    /// [`DataRoot::resolve`] with its inputs passed in, so the rules are testable without touching the
    /// process environment (which is shared by every test thread), for either kind of build.
    fn resolve_from(
        env_value: Option<OsString>,
        platform_data_dir: Option<PathBuf>,
        default_folder: &str,
    ) -> Result<DataRoot> {
        if let Some(path) = env_value.as_deref().and_then(override_path) {
            let absolute = std::path::absolute(&path).map_err(CoreError::io(&path))?;
            return Ok(DataRoot::at(absolute));
        }
        let dir = platform_data_dir.ok_or(CoreError::NoDataDir)?;
        let current = dir.join(default_folder);
        if holds_data(&current) {
            return Ok(DataRoot::at(current));
        }
        let earlier = legacy_folders(default_folder)
            .iter()
            .map(|legacy| dir.join(legacy))
            .find(|legacy| holds_data(legacy));
        Ok(DataRoot::at(earlier.unwrap_or(current)))
    }

    /// A data root at an explicit path (the desktop shell's `app_local_data_dir()`, tests).
    pub fn at(path: impl Into<PathBuf>) -> DataRoot {
        DataRoot { root: path.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/profiles`
    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join(PROFILES_DIR)
    }

    /// `<root>/profiles/<id>`, after checking `id` with [`validate_profile_id`]. Ids come from
    /// `state.json` and the UI, so this is the one place that stops them escaping `profiles/`.
    pub fn profile_dir(&self, id: &str) -> Result<PathBuf> {
        validate_profile_id(id)?;
        Ok(self.profiles_dir().join(id))
    }

    /// `<root>/device.json`
    pub fn device_file(&self) -> PathBuf {
        self.root.join(DEVICE_FILE)
    }

    /// `<root>/state.json`
    pub fn state_file(&self) -> PathBuf {
        self.root.join(STATE_FILE)
    }

    /// Creates the root and `profiles/`. `profiles/` is made private to the current user even when it
    /// already exists (mode 0700 on unix; on Windows a protected ACL for the user, SYSTEM, and
    /// Administrators), since everything under it is Ghost's and `GHOST_DATA_DIR` may put it
    /// somewhere other accounts can read, like `C:\aura`. A file where `profiles/` should be is
    /// moved aside, so a stray file can't block startup. A link there is used, never moved: one that
    /// leads nowhere (a folder on a disk that isn't connected) is a [`CoreError::BrokenLink`]. The
    /// root is left as is when it already exists, because with `GHOST_DATA_DIR` it may be a
    /// folder the user chose.
    pub fn ensure(&self) -> Result<()> {
        fsutil::create_private_dir_all(&self.root)?;
        let profiles = self.profiles_dir();
        fsutil::ensure_private_dir(&profiles)?;
        fsutil::restrict_to_owner(&profiles);
        Ok(())
    }
}

/// The platform folder the default data root lives in: the local (never roaming) app data folder,
/// which is what Tauri's `app_local_data_dir()` builds on. See the module docs for why.
fn platform_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir()
}

/// Whether `folder` is a data root something wrote to: it has this machine's identity or a
/// profiles folder. An empty folder, or one holding only what the platform keeps for the app (a
/// WebView profile), is not.
pub(crate) fn holds_data(folder: &Path) -> bool {
    // `symlink_metadata`: a `profiles` that links to a disk not connected still counts, so the
    // app reports the broken link instead of starting over in the other folder.
    DATA_MARKERS
        .iter()
        .any(|marker| std::fs::symlink_metadata(folder.join(marker)).is_ok())
}

/// The window-state plugin's file, kept per bundle identifier in the platform's config folder.
const WINDOW_STATE_FILE: &str = ".window-state.json";

/// Copies the window's saved size and position from the folder an earlier name kept it in, once:
/// when this identifier has none yet and an earlier one has (the newest that has). The window
/// then opens where it was. Failing changes nothing but where the window opens, so failures are
/// ignored.
pub fn carry_window_state() {
    if let Some(config) = dirs::config_dir() {
        carry_window_state_in(&config);
    }
}

fn carry_window_state_in(config: &Path) {
    let to = config.join(APP_IDENTIFIER).join(WINDOW_STATE_FILE);
    if to.exists() {
        return;
    }
    let Some(from) = LEGACY_IDENTIFIERS
        .iter()
        .map(|legacy| config.join(legacy).join(WINDOW_STATE_FILE))
        .find(|from| from.is_file())
    else {
        return;
    };
    if let Some(folder) = to.parent()
        && std::fs::create_dir_all(folder).is_ok()
    {
        let _ = std::fs::copy(&from, &to);
    }
}

/// What makes a folder a data root that was used ([`holds_data`]).
const DATA_MARKERS: [&str; 2] = [DEVICE_FILE, PROFILES_DIR];

/// Checks that `id` is safe to use as a directory name on every OS: 1 to 64 characters, the first
/// `[a-z0-9]`, the rest `[a-z0-9-]`, and not a Windows device name. This rules out `..`, separators,
/// drive letters, uppercase (case-insensitive filesystems would merge `A` and `a`), and empty ids.
pub fn validate_profile_id(id: &str) -> Result<()> {
    let bytes = id.as_bytes();
    let valid = match bytes.split_first() {
        Some((first, rest)) => {
            bytes.len() <= MAX_PROFILE_ID_LEN
                && is_lower_alnum(*first)
                && rest.iter().all(|&b| is_lower_alnum(b) || b == b'-')
                && !WINDOWS_RESERVED_NAMES.contains(&id)
        }
        None => false,
    };
    if valid {
        Ok(())
    } else {
        Err(CoreError::InvalidProfileId(id.to_string()))
    }
}

fn is_lower_alnum(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}

/// The program an agent starts to run Ghost's own helpers (`--ghost-memory-helper`,
/// `--ghost-browser-helper`, `--ghost-mobile-helper`): this executable, or inside a Linux AppImage the image's `AppRun`.
/// Agents get an env without the AppImage's library path (`ghost_process::strip_launch_context`),
/// and the bundled binary needs it on a system lacking WebKitGTK; `AppRun` sets it again and
/// executes the binary with the same arguments. `None` when the executable is unknown.
pub fn helper_program() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let program = appimage_launcher(&exe, std::env::var_os("APPDIR").as_deref()).unwrap_or(exe);
    program.is_file().then_some(program)
}

/// `APPDIR/AppRun` when `exe` lies inside `appdir` and that launcher exists. An `APPDIR` naming
/// another image (one that merely started Ghost) does not contain `exe`, and is ignored.
fn appimage_launcher(exe: &Path, appdir: Option<&OsStr>) -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let appdir = Path::new(appdir?);
    if !appdir.is_absolute() || appdir.parent().is_none() || !exe.starts_with(appdir) {
        return None;
    }
    let launcher = appdir.join("AppRun");
    launcher.is_file().then_some(launcher)
}

/// The override path if the variable holds anything but whitespace. Valid UTF-8 is trimmed (a stray
/// space or newline from a shell profile is far likelier than a folder really named " data"); other
/// bytes are used as is, since they can't be blank.
fn override_path(value: &OsStr) -> Option<PathBuf> {
    match value.to_str() {
        Some(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
        }
        None => (!value.is_empty()).then(|| PathBuf::from(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn platform_dir() -> Option<PathBuf> {
        Some(PathBuf::from("/platform/data"))
    }

    #[test]
    fn the_window_opens_where_an_earlier_name_left_it_once() {
        let config = tempfile::tempdir().expect("config");
        let kept = |identifier: &str| config.path().join(identifier).join(WINDOW_STATE_FILE);
        let [o7, auracode] = LEGACY_IDENTIFIERS.map(kept);
        let to = kept(APP_IDENTIFIER);
        carry_window_state_in(config.path());
        assert!(!to.exists(), "nothing to carry");
        // Only auracode's: a copy that never ran as o7.
        std::fs::create_dir_all(auracode.parent().unwrap()).unwrap();
        std::fs::write(&auracode, "oldest").unwrap();
        carry_window_state_in(config.path());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "oldest");
        // Both: the newer of the two is where the window last was.
        std::fs::remove_file(&to).unwrap();
        std::fs::create_dir_all(o7.parent().unwrap()).unwrap();
        std::fs::write(&o7, "old").unwrap();
        carry_window_state_in(config.path());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "old");
        // This identifier's own file is never replaced.
        std::fs::write(&to, "new").unwrap();
        carry_window_state_in(config.path());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "new");
    }

    #[test]
    fn a_copy_updated_from_an_earlier_name_keeps_the_folder_its_data_is_in() {
        let local = tempfile::tempdir().expect("local");
        let dir = || Some(local.path().to_path_buf());
        let resolve = |folder| DataRoot::resolve_from(None, dir(), folder).expect("resolve");
        let new = local.path().join(APP_IDENTIFIER);
        let [o7, auracode] = LEGACY_IDENTIFIERS.map(|legacy| local.path().join(legacy));

        // Nothing anywhere: a fresh install gets the new folder.
        assert_eq!(resolve(APP_IDENTIFIER).root(), new);

        // auracode's data, and empty folders the platform made under the later names (a WebView
        // profile in each): the data wins, wherever it is.
        std::fs::create_dir_all(auracode.join("profiles/local")).expect("oldest profile");
        std::fs::write(auracode.join("device.json"), "{}").expect("oldest device");
        std::fs::create_dir_all(new.join("EBWebView")).expect("platform folder");
        std::fs::create_dir_all(o7.join("EBWebView")).expect("platform folder");
        assert_eq!(resolve(APP_IDENTIFIER).root(), auracode);
        // Either marker alone is enough.
        std::fs::remove_file(auracode.join("device.json")).expect("remove");
        assert_eq!(resolve(APP_IDENTIFIER).root(), auracode);

        // o7's folder holding data comes before auracode's, as it did while the app was o7.
        std::fs::write(o7.join("device.json"), "{}").expect("old device");
        assert_eq!(resolve(APP_IDENTIFIER).root(), o7);

        // Once the new folder holds data it is the one, the earlier ones notwithstanding.
        std::fs::write(new.join("device.json"), "{}").expect("new device");
        assert_eq!(resolve(APP_IDENTIFIER).root(), new);

        // A development build does the same with the development folders, never the installed ones.
        let [dev_o7, dev_auracode] =
            LEGACY_DEVELOPMENT_FOLDERS.map(|legacy| local.path().join(legacy));
        std::fs::create_dir_all(dev_auracode.join("profiles")).expect("oldest dev");
        assert_eq!(resolve(DEVELOPMENT_FOLDER).root(), dev_auracode);
        std::fs::create_dir_all(dev_o7.join("profiles")).expect("old dev");
        assert_eq!(resolve(DEVELOPMENT_FOLDER).root(), dev_o7);

        // An override is taken as given, even with data in the default folders.
        let chosen = local.path().join("chosen");
        let root = DataRoot::resolve_from(Some(chosen.clone().into()), dir(), APP_IDENTIFIER)
            .expect("override");
        assert_eq!(root.root(), chosen);

        // Only this app's own folders are related; anything else has no earlier name.
        assert_eq!(legacy_folders(APP_IDENTIFIER), LEGACY_IDENTIFIERS);
        assert_eq!(
            legacy_folders(DEVELOPMENT_FOLDER),
            LEGACY_DEVELOPMENT_FOLDERS
        );
        assert!(legacy_folders("elsewhere").is_empty());
        assert!(!LEGACY_IDENTIFIERS.contains(&APP_IDENTIFIER));
    }

    /// Absolute on every OS (`/custom` is drive-relative on Windows).
    fn custom_root() -> PathBuf {
        std::env::temp_dir().join("aura custom root")
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn helpers_start_through_the_appimage_launcher_only_from_inside_its_mount() {
        let mount = tempfile::tempdir().expect("mount");
        let exe = mount.path().join("usr/bin/auracode");
        std::fs::create_dir_all(exe.parent().unwrap()).expect("bin dir");
        std::fs::write(&exe, b"").expect("exe");
        let appdir = Some(mount.path().as_os_str());
        // No AppRun in the mount: the executable itself.
        assert_eq!(appimage_launcher(&exe, appdir), None);
        let launcher = mount.path().join("AppRun");
        std::fs::write(&launcher, b"").expect("AppRun");
        assert_eq!(appimage_launcher(&exe, appdir), Some(launcher));
        // An APPDIR that does not hold the executable belongs to some other image.
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        assert_eq!(
            appimage_launcher(&exe, Some(elsewhere.path().as_os_str())),
            None
        );
        for appdir in ["", "/", "relative"] {
            assert_eq!(appimage_launcher(&exe, Some(OsStr::new(appdir))), None);
        }
        assert_eq!(appimage_launcher(&exe, None), None);
    }

    #[test]
    fn env_override_is_honoured() {
        let custom = custom_root();
        let root = DataRoot::resolve_from(
            Some(custom.clone().into_os_string()),
            platform_dir(),
            APP_IDENTIFIER,
        )
        .expect("resolve");
        assert_eq!(root.root(), custom);
    }

    #[test]
    fn env_override_is_trimmed() {
        let custom = custom_root();
        let padded = format!("  {} \n", custom.display());
        let root = DataRoot::resolve_from(Some(padded.into()), platform_dir(), APP_IDENTIFIER)
            .expect("resolve");
        assert_eq!(root.root(), custom);
    }

    #[test]
    fn empty_or_whitespace_env_values_are_ignored() {
        for value in ["", " ", "\t\n "] {
            let root = DataRoot::resolve_from(Some(value.into()), platform_dir(), APP_IDENTIFIER)
                .expect("resolve");
            assert_eq!(
                root.root(),
                Path::new("/platform/data").join(APP_IDENTIFIER),
                "{value:?}"
            );
        }
        let root = DataRoot::resolve_from(None, platform_dir(), APP_IDENTIFIER).expect("resolve");
        assert_eq!(
            root.root(),
            Path::new("/platform/data").join(APP_IDENTIFIER)
        );
    }

    /// A build from a working copy opened, migrated and could delete the installed app's data,
    /// which is often the app running the session that changes that code.
    #[test]
    fn a_development_build_defaults_to_its_own_folder() {
        assert_eq!(default_folder(false), APP_IDENTIFIER);
        assert_eq!(default_folder(true), DEVELOPMENT_FOLDER);
        assert_ne!(DEVELOPMENT_FOLDER, APP_IDENTIFIER);
        assert_eq!(DEVELOPMENT_BUILD, cfg!(debug_assertions));
        assert_eq!(DEFAULT_FOLDER, default_folder(DEVELOPMENT_BUILD));

        let development =
            DataRoot::resolve_from(None, platform_dir(), DEVELOPMENT_FOLDER).expect("resolve");
        assert_eq!(
            development.root(),
            Path::new("/platform/data").join(DEVELOPMENT_FOLDER)
        );
        assert!(
            !development.root().ends_with(APP_IDENTIFIER),
            "a sibling of the installed app's folder, not inside or equal to it"
        );
        // The override still wins, the installed app's own folder included.
        let installed = Path::new("/platform/data").join(APP_IDENTIFIER);
        let chosen = std::env::temp_dir().join(APP_IDENTIFIER);
        let root = DataRoot::resolve_from(
            Some(chosen.clone().into_os_string()),
            platform_dir(),
            DEVELOPMENT_FOLDER,
        )
        .expect("resolve");
        assert_eq!(root.root(), chosen);
        assert_ne!(root.root(), installed);
    }

    #[test]
    fn a_relative_override_becomes_absolute() {
        let root =
            DataRoot::resolve_from(Some("relative/aura".into()), platform_dir(), APP_IDENTIFIER)
                .expect("resolve");
        assert!(root.root().is_absolute(), "{}", root.root().display());
        let cwd = std::env::current_dir().expect("cwd");
        assert_eq!(root.root(), cwd.join("relative").join("aura"));
    }

    #[test]
    fn no_platform_dir_and_no_override_is_no_data_dir() {
        assert!(matches!(
            DataRoot::resolve_from(None, None, APP_IDENTIFIER),
            Err(CoreError::NoDataDir)
        ));
        assert!(matches!(
            DataRoot::resolve_from(Some(" ".into()), None, APP_IDENTIFIER),
            Err(CoreError::NoDataDir)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_override_is_used_byte_for_byte() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let raw = OsString::from_vec(b"/data/\xff\xfe".to_vec());
        let root =
            DataRoot::resolve_from(Some(raw), platform_dir(), APP_IDENTIFIER).expect("resolve");
        assert_eq!(root.root().as_os_str().as_bytes(), b"/data/\xff\xfe");
    }

    /// Marks a re-executed copy of the test binary; holds the root that child should resolve.
    const CHILD_EXPECTS: &str = "GHOST_CORE_TEST_EXPECTED_ROOT";

    /// Runs `resolve()` for real in a child process with a controlled environment, because changing
    /// the environment in-process is unsafe in edition 2024 and races with other test threads.
    #[test]
    fn resolve_reads_the_real_environment() {
        if let Some(expected) = std::env::var_os(CHILD_EXPECTS) {
            let root = DataRoot::resolve().expect("resolve in child");
            assert_eq!(root.root(), Path::new(&expected));
            return;
        }

        let exe = std::env::current_exe().expect("test binary path");
        let dir = tempfile::tempdir().expect("tempdir");
        let custom = dir.path().join("custom root");
        let mut cases: Vec<(Option<OsString>, PathBuf)> = vec![
            (Some(custom.clone().into_os_string()), custom.clone()),
            (
                Some(format!("  {}  ", custom.display()).into()),
                custom.clone(),
            ),
        ];
        // Spelled out rather than through `platform_data_dir()`, so this pins the local folder too.
        // The test binary is a development build, so its default is the development folder.
        if let Some(platform) = dirs::data_local_dir() {
            // On a machine that ran a development build under an earlier name, that one's folder.
            let current = platform.join(DEVELOPMENT_FOLDER);
            let earlier = LEGACY_DEVELOPMENT_FOLDERS
                .iter()
                .map(|legacy| platform.join(legacy))
                .find(|legacy| holds_data(legacy));
            let default = match earlier {
                Some(earlier) if !holds_data(&current) => earlier,
                _ => current,
            };
            cases.push((Some("".into()), default.clone()));
            cases.push((Some("   ".into()), default.clone()));
            cases.push((None, default));
        }

        for (value, expected) in cases {
            let mut child = Command::new(&exe);
            child
                .args([
                    "--exact",
                    "paths::tests::resolve_reads_the_real_environment",
                    "--test-threads",
                    "1",
                ])
                .env(CHILD_EXPECTS, &expected);
            match &value {
                Some(value) => child.env(DATA_DIR_ENV, value),
                None => {
                    for name in LEGACY_DATA_DIR_ENVS {
                        child.env_remove(name);
                    }
                    child.env_remove(DATA_DIR_ENV)
                }
            };
            let output = child.output().expect("run child test");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success() && stdout.contains("1 passed"),
                "case {value:?}: stdout {stdout}\nstderr {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// The default root must be the local app data folder, never the roaming one: Roaming AppData is
    /// copied between machines on managed Windows PCs, which would hand `device.json` (and the
    /// databases) to every PC the user signs into. On macOS and Linux the two folders are the same;
    /// the Windows CI job is the one that tells them apart.
    #[test]
    fn the_default_root_is_the_local_not_the_roaming_app_data_folder() {
        assert_eq!(platform_data_dir(), dirs::data_local_dir());
        let Some(local) = dirs::data_local_dir() else {
            return;
        };
        let root =
            DataRoot::resolve_from(None, platform_data_dir(), DEFAULT_FOLDER).expect("resolve");
        assert_eq!(root.root(), local.join(DEFAULT_FOLDER));
        #[cfg(windows)]
        {
            let roaming = dirs::data_dir().expect("Roaming AppData");
            assert_ne!(
                local, roaming,
                "LocalAppData and Roaming AppData are different folders"
            );
            assert!(
                !root.root().starts_with(&roaming),
                "{} is under Roaming AppData",
                root.root().display()
            );
        }
        #[cfg(not(windows))]
        assert_eq!(dirs::data_local_dir(), dirs::data_dir());
    }

    #[test]
    fn layout_paths_hang_off_the_root() {
        let root = DataRoot::at("/r");
        assert_eq!(root.profiles_dir(), Path::new("/r/profiles"));
        assert_eq!(root.device_file(), Path::new("/r/device.json"));
        assert_eq!(root.state_file(), Path::new("/r/state.json"));
        assert_eq!(
            root.profile_dir("local").expect("valid"),
            Path::new("/r/profiles/local")
        );
    }

    #[test]
    fn profile_dir_rejects_unsafe_ids() {
        let root = DataRoot::at("/r");
        let too_long = "x".repeat(65);
        for id in [
            "",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "A",
            "Local",
            too_long.as_str(),
            "-x",
            ".",
            "a.b",
            "a b",
            "é",
            "local\0",
        ] {
            assert!(
                matches!(root.profile_dir(id), Err(CoreError::InvalidProfileId(ref got)) if got == id),
                "{id:?} should be rejected"
            );
        }
    }

    #[test]
    fn profile_dir_rejects_windows_device_names() {
        for id in ["con", "nul", "aux", "prn", "com1", "lpt9", "com0"] {
            assert!(validate_profile_id(id).is_err(), "{id}");
        }
        for id in ["console", "con-1", "nul0", "com10", "lpt"] {
            assert!(validate_profile_id(id).is_ok(), "{id}");
        }
    }

    #[test]
    fn profile_dir_accepts_local_and_uuid_v7() {
        let root = DataRoot::at("/r");
        assert!(root.profile_dir("local").is_ok());
        let id = crate::ids::new_id();
        assert_eq!(
            root.profile_dir(&id).expect("uuid is valid"),
            Path::new("/r/profiles").join(&id)
        );
        assert!(validate_profile_id(&"a".repeat(64)).is_ok());
        assert!(validate_profile_id("0-a").is_ok());
    }

    #[test]
    fn ensure_creates_the_root_and_profiles_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("nested").join("aura"));
        root.ensure().expect("ensure");
        root.ensure().expect("ensure is idempotent");
        assert!(root.root().is_dir());
        assert!(root.profiles_dir().is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [root.root().to_path_buf(), root.profiles_dir()] {
                let mode = std::fs::metadata(&path)
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o700, "{}", path.display());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn ensure_tightens_an_existing_profiles_dir_but_not_the_root() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("aura"));
        std::fs::create_dir_all(root.profiles_dir()).expect("mkdir");
        for path in [root.root().to_path_buf(), root.profiles_dir()] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        root.ensure().expect("ensure");
        let mode = |path: &Path| {
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(&root.profiles_dir()), 0o700);
        assert_eq!(mode(root.root()), 0o755);
    }

    #[test]
    fn a_file_where_profiles_should_be_is_moved_aside() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("aura"));
        std::fs::create_dir_all(root.root()).expect("mkdir");
        std::fs::write(root.profiles_dir(), b"not a folder").expect("write");
        root.ensure().expect("ensure");
        assert!(root.profiles_dir().is_dir());
        let kept: Vec<PathBuf> = std::fs::read_dir(root.root())
            .expect("read_dir")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| fsutil::file_name_lossy(path).starts_with("profiles.corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(std::fs::read(&kept[0]).expect("kept"), b"not a folder");
    }

    /// `GHOST_DATA_DIR` can point anywhere (`C:\aura` inherits an ACL that lets other accounts
    /// read it), so `profiles\` gets its own owner-only ACL wherever it is.
    #[cfg(windows)]
    #[test]
    fn ensure_makes_profiles_owner_only_on_windows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("portable aura"));
        root.ensure().expect("ensure");
        assert!(
            fsutil::is_owner_only(&root.profiles_dir()).expect("read the ACL"),
            "profiles\\ is not owner-only"
        );
        root.ensure().expect("ensure is idempotent");
        assert!(fsutil::is_owner_only(&root.profiles_dir()).expect("read the ACL"));
    }
}
