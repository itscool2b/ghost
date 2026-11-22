//! The app-wide state Tauri manages: startup results, the chat runtime (the chat store and the
//! session manager), the database repair notice, the quit flow's state, each webview's page, the
//! background integrity check, running demo streams, and the launch switches.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use o7_core::device::DeviceIdentity;
use o7_core::profile::ProfileMeta;
use o7_core::session::{SessionError, SessionManager};
use serde::Serialize;

use crate::boot::{Booted, DbInfo, Recovery};
use crate::demo::DemoSessions;
use crate::error::{AppError, AppErrorCode};
use crate::glass::GlassEffect;
use crate::integrity::IntegrityCheck;
use crate::launch::{LaunchMode, LaunchOptions};
use crate::os::OsInfo;
use crate::sessions::{self, ChatRuntime, Pages, Quit};
use crate::sync::lock;
use crate::window::ShowOnce;

/// Everything the UI needs to know about this launch, from `app_info`.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct AppInfo {
    /// o7's version (the Cargo workspace version).
    pub app_version: String,
    pub tauri_version: String,
    /// `"macos"`, `"windows"`, `"linux"`, or Rust's name for another OS.
    pub os: String,
    /// The release as the OS reports it (`"26.6.2"`, `"10.0.22631"`, a Linux distribution name).
    pub os_version: String,
    /// `"aarch64"`, `"x86_64"`, ...
    pub arch: String,
    /// The data root. `null` only when no data folder could be determined at all.
    pub data_dir: Option<String>,
    /// `null` when startup failed before the identity was loaded (see `fatal`).
    pub device: Option<DeviceIdentity>,
    /// The active profile; `null` when startup failed before opening it.
    pub profile: Option<ProfileMeta>,
    /// Profile commands are temporarily unavailable while a reviewed switch is prepared.
    pub profile_switching: bool,
    /// The profile database; `null` when startup failed before opening it.
    pub db: Option<DbInfo>,
    /// The native material behind the window. `none` means the UI must paint an opaque background.
    pub glass: GlassEffect,
    /// What to open into: the normal screen, the smoke run, or the stress view.
    pub launch: LaunchMode,
    /// Sentences to show: repairs startup made, launch settings that were ignored. None of them
    /// can be dismissed.
    pub notices: Vec<String>,
    /// The notice that the database was damaged and repaired, shown on every launch until the user
    /// dismisses it (`acknowledge_recovery`): whatever the repair could not save is only in the
    /// damaged copy it names. `null` when there is none, and once it has been dismissed.
    pub recovery_notice: Option<String>,
    /// Why startup could not complete, as a sentence to show on the error screen.
    pub fatal: Option<String>,
    /// The window's page may play sound (Settings' Sounds). False on Linux where GStreamer's
    /// plugins are missing, and after the page stopped working once.
    pub media: bool,
    /// Why it may not, as a sentence for Settings and the browser.
    pub media_notice: Option<String>,
}

/// State managed by Tauri for the whole app.
pub struct AppState {
    pub(crate) runtime: Mutex<ProfileRuntime>,
    pub(crate) transition: Arc<tokio::sync::Mutex<()>>,
    access: Arc<ProfileAccess>,
    /// The data root as a path (`info.data_dir` is its display string): the one folder the page
    /// may reveal, through `reveal_data_dir`.
    data_dir: Option<PathBuf>,
    /// The chat store (which owns the profile database and its owner lock) and the session
    /// manager on top of it. `None` when startup failed before they could start.
    /// The repair whose notice is still shown, until `acknowledge_recovery` succeeds.
    recovery: Mutex<Option<Recovery>>,
    /// Where quitting stands; shared with the quit and signal tasks.
    pub quit: Arc<Quit>,
    /// Each webview's current page, for the sinks of its channels.
    pub pages: Arc<Pages>,
    pub catalog_refresh: Arc<crate::catalog_refresh::RefreshGate>,
    pub dictation: Arc<crate::dictation::Dictation>,
    /// Reading aloud: the one voice that may be speaking.
    pub speech: Arc<crate::speech::Speech>,
    /// Unlogged terminal sessions, owned by the window that explicitly opened them.
    pub terminals: o7_core::terminal::Terminals,
    /// Which chat each terminal chat's terminal holds, so a page that goes ends its own.
    pub terminal_chats: crate::terminal::TerminalChats,
    /// Lazily loaded setup preferences, available only while this process owns the profile.
    /// The background check after a session that did not close the database. Started once the
    /// window is shown, closed at exit before the store.
    pub integrity: IntegrityCheck,
    pub demos: DemoSessions,
    pub launch: LaunchOptions,
    /// Shared with the startup fallback timer, so whichever shows the window first wins.
    pub window_shown: Arc<ShowOnce>,
    /// Set by the first smoke report (or the watchdog); later ones are ignored.
    pub smoke_reported: Arc<AtomicBool>,
}

pub(crate) struct ProfileRuntime {
    pub info: AppInfo,
    pub chats: Option<Arc<ChatRuntime>>,
    pub onboarding: Option<Arc<crate::onboarding::SetupService>>,
}

#[derive(Default)]
struct ProfileAccess {
    active: AtomicUsize,
    changed: tokio::sync::Notify,
    generation: AtomicU64,
}
/// A cached operation is valid only for the profile generation that admitted it. A lease, rather
/// than this token alone, prevents an in-flight switch.
#[derive(Clone)]
pub struct ProfileGeneration {
    access: Arc<ProfileAccess>,
    generation: u64,
}
impl ProfileGeneration {
    pub fn is_current(&self) -> bool {
        self.access.generation.load(Ordering::Acquire) == self.generation
    }
}
/// Keeps commands admitted to the old profile visible to a switch until their await completes.
/// Do not clone the underlying service into a detached task; use its own reviewed task ownership.
pub struct ProfileLease<T> {
    value: T,
    access: Arc<ProfileAccess>,
    generation: u64,
}
impl<T> ProfileLease<T> {
    pub fn generation(&self) -> ProfileGeneration {
        ProfileGeneration {
            access: self.access.clone(),
            generation: self.generation,
        }
    }
}
impl<T> std::fmt::Debug for ProfileLease<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProfileLease(..)")
    }
}
impl<T> std::ops::Deref for ProfileLease<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}
impl<T> Drop for ProfileLease<T> {
    fn drop(&mut self) {
        self.access.active.fetch_sub(1, Ordering::AcqRel);
        self.access.changed.notify_one();
    }
}

impl AppState {
    /// `booted` is what startup produced; `chats` what [`sessions::start_chats`] built from it (the
    /// store and the owner lock moved there).
    pub fn new(
        booted: Booted,
        chats: Option<ChatRuntime>,
        os: &OsInfo,
        glass: GlassEffect,
        launch: LaunchOptions,
        app_version: String,
    ) -> AppState {
        let onboarding = chats
            .as_ref()
            .and(booted.profile_dir.clone())
            .map(|dir| Arc::new(crate::onboarding::SetupService::new(dir)));
        if let (Some(chats), Some(onboarding)) = (&chats, &onboarding) {
            crate::general_preferences::initialize(&chats.manager, onboarding);
        }
        let mut notices = booted.notices;
        notices.extend(launch.notices());
        if crate::updater::staging() {
            notices.push(format!("Staging build {app_version}. Updates use the staging release channel. This build is for release qualification."));
        }
        let info = AppInfo {
            app_version,
            tauri_version: tauri::VERSION.to_string(),
            os: os.os.to_string(),
            os_version: os.display.clone(),
            arch: std::env::consts::ARCH.to_string(),
            data_dir: booted
                .data_dir
                .as_ref()
                .map(|dir| dir.display().to_string()),
            device: booted.device,
            profile_switching: false,
            profile: booted.profile,
            db: booted.db,
            glass,
            launch: launch.mode(),
            notices,
            // Filled in live by `app_info`, since dismissing it changes it.
            recovery_notice: None,
            fatal: booted.fatal,
            // Filled in live by `app_info`: the window's page can lose its sound.
            media: true,
            media_notice: None,
        };
        AppState {
            runtime: Mutex::new(ProfileRuntime {
                info,
                chats: chats.map(Arc::new),
                onboarding,
            }),
            transition: Arc::new(tokio::sync::Mutex::new(())),
            access: Arc::default(),
            data_dir: booted.data_dir,
            recovery: Mutex::new(booted.recovery),
            quit: Arc::default(),
            pages: Arc::default(),
            catalog_refresh: Arc::default(),
            dictation: Arc::default(),
            speech: Arc::default(),
            terminals: Default::default(),
            terminal_chats: Default::default(),
            integrity: IntegrityCheck::new(booted.integrity_check),
            demos: DemoSessions::default(),
            launch,
            window_shown: Arc::default(),
            smoke_reported: Arc::default(),
        }
    }

    /// The startup snapshot (the schema version is the one startup opened), with what can change
    /// since read live: notices that arrived after startup (the sweep's), and the repair notice,
    /// which goes once dismissed (a reloaded page must not show it again).
    pub fn app_info(&self) -> AppInfo {
        let runtime = lock(&self.runtime);
        let mut info = runtime.info.clone();
        if let Some(root) = &self.data_dir {
            info.notices
                .extend(o7_config::instructions::recovery_notices(
                    &root.join("instructions"),
                ));
        }
        if let Some(chats) = &runtime.chats {
            info.notices
                .extend(lock(&chats.late_notices).iter().cloned());
            if let Some(db) = &mut info.db {
                let timings = chats.store.startup_timings();
                let ms = |ms: u64| u32::try_from(ms).unwrap_or(u32::MAX);
                db.recovery_ms = timings.recovery_ms.map(ms);
                db.indexes_ms = timings.indexes_ms.map(ms);
            }
        }
        info.recovery_notice = lock(&self.recovery)
            .as_ref()
            .map(|recovery| recovery.notice.clone());
        (info.media, info.media_notice) = crate::browser::window_media();
        info
    }

    /// The data root startup chose, whether or not it could be created. `None` only when no data
    /// folder could be determined at all.
    pub fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    /// The chat store and the session manager, when startup got that far.
    pub fn chats(&self) -> Option<Arc<ChatRuntime>> {
        lock(&self.runtime).chats.clone()
    }

    /// The session manager every chat command wraps. Without one (startup failed, and the UI
    /// shows its error screen) a command answers `storage_unavailable`.
    pub fn sessions(&self) -> Result<ProfileLease<SessionManager>, AppError> {
        let runtime = lock(&self.runtime);
        self.check_profile_access(&runtime)?;
        let manager = runtime
            .chats
            .as_ref()
            .map(|chats| chats.manager.clone())
            .ok_or_else(|| {
                SessionError::StorageUnavailable(
                    "Unlock or select a profile before opening chats.".into(),
                )
            })?;
        Ok(self.lease(manager))
    }

    /// As [`Self::sessions`], and only while `profile` is the open profile. The profile and its
    /// chats are read under one lock, so a caller admitted for one profile is never handed
    /// another's chats, whenever a switch lands (remote access binds every call this way).
    pub(crate) fn sessions_of(
        &self,
        profile: &str,
    ) -> Result<ProfileLease<SessionManager>, AppError> {
        let runtime = lock(&self.runtime);
        self.check_profile_access(&runtime)?;
        let manager = Self::chats_of_open(&runtime, profile)?.manager.clone();
        Ok(self.lease(manager))
    }

    /// The open profile's chats without a lease, only while `profile` is that profile.
    pub(crate) fn chats_of(&self, profile: &str) -> Result<Arc<ChatRuntime>, AppError> {
        Self::chats_of_open(&lock(&self.runtime), profile).cloned()
    }

    fn chats_of_open<'a>(
        runtime: &'a ProfileRuntime,
        profile: &str,
    ) -> Result<&'a Arc<ChatRuntime>, AppError> {
        let open = runtime.info.profile.as_ref().map(|open| open.id.as_str());
        match (&runtime.chats, open) {
            (Some(chats), Some(open)) if !profile.is_empty() && open == profile => Ok(chats),
            _ => Err(AppError::new(
                AppErrorCode::StorageUnavailable,
                "Another workspace is open on this computer.",
            )),
        }
    }

    /// The active profile's setup preferences without a command lease, for reads the app makes on
    /// its own (startup and profile switches apply Settings' General page). Never for changes.
    pub(crate) fn setup_service(&self) -> Option<Arc<crate::onboarding::SetupService>> {
        lock(&self.runtime).onboarding.clone()
    }

    pub fn onboarding(
        &self,
    ) -> Result<ProfileLease<Arc<crate::onboarding::SetupService>>, AppError> {
        let runtime = lock(&self.runtime);
        self.check_profile_access(&runtime)?;
        let service = runtime.onboarding.clone().ok_or_else(|| {
            AppError::new(
                AppErrorCode::StorageUnavailable,
                "Select a profile before changing setup.",
            )
        })?;
        Ok(self.lease(service))
    }
    fn check_profile_access(&self, runtime: &ProfileRuntime) -> Result<(), AppError> {
        if runtime.info.profile_switching {
            Err(AppError::new(
                AppErrorCode::StorageUnavailable,
                "Wait for the profile switch to finish.",
            ))
        } else {
            Ok(())
        }
    }
    fn lease<T>(&self, value: T) -> ProfileLease<T> {
        self.access.active.fetch_add(1, Ordering::AcqRel);
        ProfileLease {
            value,
            access: self.access.clone(),
            generation: self.access.generation.load(Ordering::Acquire),
        }
    }
    pub(crate) async fn wait_profile_commands(
        &self,
    ) -> Option<o7_core::session::PluginRefreshPause> {
        let library_pause = lock(&self.runtime)
            .chats
            .as_ref()
            .map(|chats| chats.manager.pause_plugin_refresh());
        loop {
            let changed = self.access.changed.notified();
            if self.access.active.load(Ordering::Acquire) == 0 {
                return library_pause;
            }
            changed.await;
        }
    }
    pub(crate) fn install_profile(
        &self,
        booted: Booted,
        chats: Arc<ChatRuntime>,
        onboarding: Arc<crate::onboarding::SetupService>,
    ) {
        crate::general_preferences::initialize(&chats.manager, &onboarding);
        let mut runtime = lock(&self.runtime);
        self.access.generation.fetch_add(1, Ordering::AcqRel);
        self.dictation.cancel_all();
        self.speech.stop_all();
        runtime.info.profile = booted.profile;
        runtime.info.db = booted.db;
        runtime.info.fatal = None;
        runtime.info.notices = booted.notices;
        runtime.info.notices.extend(self.launch.notices());
        runtime.chats = Some(chats);
        runtime.onboarding = Some(onboarding);
        *lock(&self.recovery) = booted.recovery;
    }

    /// Dismisses the repair notice for good: o7-core removes the repair's record (keeping the
    /// damaged copy), so no later launch reports it. It acknowledges exactly the repair startup
    /// reported; nothing can name another path. With no notice (there was none, or it was already
    /// dismissed) this does nothing, so calling it twice is harmless. On failure the notice stays,
    /// in `app_info` and on the next launch.
    pub async fn acknowledge_recovery(&self) -> Result<(), AppError> {
        let Some(pending) = lock(&self.recovery).clone() else {
            return Ok(());
        };
        let Some(chats) = self.chats() else {
            return Err(AppError::new(
                AppErrorCode::AcknowledgeFailed,
                "Could not dismiss the notice: the database is closed.",
            ));
        };
        match chats
            .store
            .acknowledge_recovery(pending.quarantined.clone())
            .await
        {
            Ok(removed) => {
                if !removed {
                    // Another o7 process (the CLI) acknowledged it, or its record was
                    // removed by hand: either way there is nothing left to report.
                    tracing::info!(quarantined = %pending.quarantined, "the database repair was already acknowledged");
                }
                let mut recovery = lock(&self.recovery);
                if recovery.as_ref() == Some(&pending) {
                    *recovery = None;
                }
                Ok(())
            }
            Err(error) => {
                tracing::warn!(%error, quarantined = %pending.quarantined, "could not acknowledge the database repair");
                Err(AppError::new(
                    AppErrorCode::AcknowledgeFailed,
                    format!("Could not dismiss the notice: {error}"),
                ))
            }
        }
    }

    /// `RunEvent::Exit`: the integrity check closes first, so one that has not begun never starts
    /// after the store closes (a running one reads through its own connection and ends with the
    /// process); then every agent process is killed and the store closes within 1 s unless the quit
    /// flow closed it ([`sessions::exit_last_resort`]); then the demo streams stop. Only a closed
    /// connection folds SQLite's write-ahead log back into the file; a log left behind tells the
    /// next launch this one did not end cleanly.
    pub fn at_exit(&self) {
        self.dictation.cancel_all();
        self.speech.stop_all();
        self.terminals.stop_all();
        if self.integrity.close() {
            tracing::info!("the integrity check is still running; it ends with the process");
        }
        if let Some(chats) = self.chats() {
            sessions::exit_last_resort(&self.quit, &chats);
        }
        self.demos.cancel_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::{boot, boot_next};
    use crate::launch::{NO_GLASS_ENV, STRESS_ENV};
    use o7_core::paths::DataRoot;
    use o7_core::store::OpenOutcome;

    fn os() -> OsInfo {
        OsInfo {
            os: "macos",
            version: None,
            display: "26.6.2".into(),
        }
    }

    /// The state of a launch, with its chats started on fakes (needs a Tokio runtime).
    fn state(mut booted: Booted) -> AppState {
        let chats = sessions::testing::start(&mut booted);
        let launch = LaunchOptions::from_lookup(|_| None);
        AppState::new(
            booted,
            chats,
            &os(),
            GlassEffect::None,
            launch,
            "0.1.0".into(),
        )
    }

    /// A data root whose database the last launch found damaged and repaired. Returns the folder,
    /// the root, and the path the repair moved the damaged database to.
    fn repaired() -> (tempfile::TempDir, DataRoot, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("data"));
        let quarantined = damage_and_repair(&root);
        (dir, root, quarantined)
    }

    /// Damages `root`'s database and boots, which repairs it. Returns where the damaged copy went.
    fn damage_and_repair(root: &DataRoot) -> String {
        let first = boot_next(root);
        let db = PathBuf::from(&first.db.as_ref().expect("db").path);
        drop(first);
        std::fs::write(&db, vec![0x5a; 8192]).expect("damage the database");
        let repaired = boot_next(root);
        let quarantined = repaired
            .recovery
            .as_ref()
            .expect("a repair to report")
            .quarantined
            .clone();
        drop(repaired);
        quarantined
    }

    /// `<quarantined>.json`, o7-core's record of the repair: removing it is what acknowledging
    /// does.
    fn record(quarantined: &str) -> PathBuf {
        PathBuf::from(format!("{quarantined}.json"))
    }

    /// Whether `notice` is about the repair that moved the damaged copy to exactly `quarantined`
    /// (every notice ends with "kept at <path>."; a plain `contains` would also match a later
    /// repair's `<path>.1`).
    fn names(notice: &str, quarantined: &str) -> bool {
        notice.ends_with(&format!(" {quarantined}."))
    }

    /// The state of the next launch, after `state` exited.
    fn state_after_exit(state: AppState, root: &DataRoot) -> AppState {
        state.at_exit();
        drop(state);
        self::state(boot_next(root))
    }

    #[tokio::test]
    async fn checkpoint_policy_is_initialized_before_publishing_each_profile() {
        use o7_core::onboarding::{OnboardingStore, SetupAction};
        use o7_core::profile::Profiles;

        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::at(dir.path().join("data"));
        let first = boot(Ok(root.clone()));
        OnboardingStore::open(first.profile_dir.as_ref().unwrap())
            .unwrap()
            .apply(0, SetupAction::AutomaticCheckpoints { enabled: true })
            .unwrap();
        let state = state(first);
        let previous = state.chats().unwrap();
        assert!(previous.manager.automatic_checkpoints());

        let next = Profiles::new(root.clone()).create("Other profile").unwrap();
        let onboarding = Arc::new(crate::onboarding::SetupService::new(next.dir.clone()));
        let mut booted = crate::boot::boot_profile(root, next);
        let chats = Arc::new(sessions::testing::start(&mut booted).unwrap());
        // Make the new runtime disagree with its saved preference to prove installation reads
        // that profile, rather than keeping either manager's previous policy.
        chats.manager.set_automatic_checkpoints(true);
        state.install_profile(booted, chats, onboarding);
        assert!(!state.sessions().unwrap().automatic_checkpoints());
        assert!(previous.manager.automatic_checkpoints());
        sessions::exit_last_resort(&Quit::default(), &previous);
        state.at_exit();
    }

    #[tokio::test]
    async fn app_info_combines_startup_os_glass_and_launch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut booted = boot(Ok(DataRoot::at(dir.path().join("data"))));
        let chats = sessions::testing::start(&mut booted);
        assert!(chats.is_some());
        let launch = LaunchOptions::from_lookup(|name| match name {
            STRESS_ENV => Some("orbs=0".into()),
            NO_GLASS_ENV => Some("1".into()),
            _ => None,
        });
        let state = AppState::new(
            booted,
            chats,
            &os(),
            GlassEffect::None,
            launch,
            "0.1.0".into(),
        );
        let info = state.app_info();

        assert_eq!(info.app_version, "0.1.0");
        assert_eq!(info.tauri_version, tauri::VERSION);
        assert_eq!(info.os, "macos");
        assert_eq!(info.os_version, "26.6.2");
        assert_eq!(info.arch, std::env::consts::ARCH);
        assert_eq!(info.glass, GlassEffect::None);
        assert_eq!(info.launch, LaunchMode::Normal);
        assert_eq!(info.fatal, None);
        assert_eq!(info.recovery_notice, None);
        // Linux decides by the plugins this machine has; elsewhere the window always plays sound.
        assert_eq!(
            (info.media, info.media_notice.clone()),
            crate::browser::window_media()
        );
        if !cfg!(target_os = "linux") {
            assert!(info.media && info.media_notice.is_none());
        }
        assert!(info.device.is_some() && info.profile.is_some());
        assert_eq!(
            info.db.as_ref().map(|db| db.schema_version),
            Some(o7_core::store::SCHEMA_VERSION)
        );
        assert_eq!(info.notices.len(), 2, "{:?}", info.notices);
        assert!(info.notices[0].starts_with("O7_STRESS was ignored"));
        assert!(state.sessions().is_ok());

        let data = dir.path().join("data");
        assert_eq!(state.data_dir(), Some(data.as_path()));
        assert_eq!(info.data_dir, Some(data.display().to_string()));
        assert_eq!(
            state.integrity.pending(),
            None,
            "a first launch checks nothing"
        );
        state.at_exit();
    }

    /// Retained journals are reported both at startup and when a later write leaves one behind.
    #[tokio::test]
    async fn initial_notice_lists_retained_instructions_journal_without_touching_targets() {
        for before_startup in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let data = dir.path().join("data");
            let target = dir.path().join("project").join("AGENTS.md");
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, b"Keep the user's instructions.").unwrap();
            let journal = data
                .join("instructions")
                .join("backups")
                .join(uuid::Uuid::now_v7().to_string());
            let retain_journal = || {
                std::fs::create_dir_all(&journal).unwrap();
                std::fs::write(journal.join("journal.json"), b"{broken").unwrap();
            };
            if before_startup {
                retain_journal();
            }
            let state = state(boot(Ok(DataRoot::at(data))));
            if !before_startup {
                retain_journal();
            }
            let info = state.app_info();
            assert!(
                info.notices.iter().any(|notice| {
                    notice.contains("Instructions change needs recovery")
                        && notice.contains(&journal.to_string_lossy().to_string())
                }),
                "before_startup={before_startup}, journal={}, notices={:?}",
                journal.display(),
                info.notices
            );
            assert_eq!(
                std::fs::read(journal.join("journal.json")).unwrap(),
                b"{broken"
            );
            assert_eq!(std::fs::read_dir(&journal).unwrap().count(), 1);
            assert_eq!(
                std::fs::read(&target).unwrap(),
                b"Keep the user's instructions."
            );
            state.at_exit();
        }
    }

    /// Remote access asks for one profile's chats by name: only the open profile's are handed out.
    #[tokio::test]
    async fn chats_are_handed_out_by_name_only_for_the_open_profile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(boot(Ok(DataRoot::at(dir.path().join("data")))));
        let open = state.app_info().profile.expect("profile").id;
        assert!(state.sessions_of(&open).is_ok());
        assert!(state.chats_of(&open).is_ok());
        for other in ["", "another-profile", &format!("{open} ")] {
            let Err(refused) = state.sessions_of(other) else {
                panic!("{other:?} was handed chats");
            };
            assert_eq!(refused.code, AppErrorCode::StorageUnavailable, "{other:?}");
            assert!(state.chats_of(other).is_err(), "{other:?}");
        }
        // While a switch is under way nothing is handed out, whatever the name.
        lock(&state.runtime).info.profile_switching = true;
        assert!(state.sessions_of(&open).is_err());
        lock(&state.runtime).info.profile_switching = false;
        // With no profile open, an empty name is not a match.
        lock(&state.runtime).info.profile = None;
        assert!(state.sessions_of("").is_err() && state.chats_of("").is_err());
        state.at_exit();
    }

    /// The sweep's notice arrives after startup and shows with the others from then on.
    #[tokio::test]
    async fn notices_that_arrive_after_startup_are_listed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(boot(Ok(DataRoot::at(dir.path().join("data")))));
        assert!(state.app_info().notices.is_empty());
        let chats = state.chats().expect("chats");
        lock(&chats.late_notices).push("o7 stopped 1 agent process.".into());
        assert_eq!(state.app_info().notices, ["o7 stopped 1 agent process."]);
        state.at_exit();
    }

    #[tokio::test]
    async fn exiting_closes_the_store_and_leaves_no_write_ahead_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let booted = boot(Ok(DataRoot::at(dir.path().join("data"))));
        let db = PathBuf::from(&booted.db.as_ref().expect("db").path);
        let wal = PathBuf::from(format!("{}-wal", db.display()));
        let state = state(booted);
        assert!(
            std::fs::metadata(&wal).is_ok_and(|meta| meta.len() > 0),
            "creating the database wrote to its log"
        );

        state.at_exit();
        assert!(!wal.exists(), "the log was folded back into the database");
        assert!(state.quit.store_closed());
        state.at_exit();
        // The startup facts are still there for a late app_info.
        assert!(state.app_info().db.is_some());
    }

    #[tokio::test]
    async fn a_failed_startup_still_answers_app_info() {
        let booted = boot(Err(o7_core::CoreError::NoDataDir));
        let launch = LaunchOptions::from_lookup(|_| None);
        let state = AppState::new(
            booted,
            None,
            &os(),
            GlassEffect::Vibrancy,
            launch,
            "0.1.0".into(),
        );
        let info = state.app_info();
        assert!(info.fatal.is_some());
        assert_eq!(info.data_dir, None);
        assert_eq!(state.data_dir(), None, "nothing to reveal");
        assert_eq!(info.device, None);
        assert_eq!(info.db, None);
        assert_eq!(info.recovery_notice, None);
        let json = serde_json::to_value(&info).expect("serialize");
        assert_eq!(json["glass"], "vibrancy");
        assert_eq!(json["launch"]["type"], "normal");
        assert!(json["device"].is_null());
        assert!(json["recovery_notice"].is_null());

        let error = state.sessions().expect_err("no sessions");
        assert_eq!(error.code, AppErrorCode::StorageUnavailable);
        state.at_exit();
    }

    #[tokio::test]
    async fn a_dismissed_repair_notice_is_gone_now_and_on_the_next_launch() {
        let (_dir, root, quarantined) = repaired();
        let state = state(boot_next(&root));
        let info = state.app_info();
        let notice = info.recovery_notice.expect("the repair is still reported");
        assert!(names(&notice, &quarantined), "{notice}");
        assert!(info.notices.is_empty(), "{:?}", info.notices);

        state.acknowledge_recovery().await.expect("acknowledge");
        assert_eq!(
            state.app_info().recovery_notice,
            None,
            "a reloaded page no longer shows it"
        );
        assert!(
            !record(&quarantined).exists(),
            "the repair's record is gone"
        );
        assert!(
            Path::new(&quarantined).exists(),
            "the damaged copy itself is kept"
        );

        let next = state_after_exit(state, &root);
        assert_eq!(next.app_info().recovery_notice, None);
        assert_eq!(next.app_info().db.expect("db").outcome, OpenOutcome::Opened);
        next.at_exit();
    }

    #[tokio::test]
    async fn acknowledging_twice_is_harmless() {
        let (_dir, root, quarantined) = repaired();
        let state = state(boot_next(&root));
        state.acknowledge_recovery().await.expect("acknowledge");
        state
            .acknowledge_recovery()
            .await
            .expect("a second acknowledge does nothing");
        assert_eq!(state.app_info().recovery_notice, None);
        assert!(Path::new(&quarantined).exists());
        let next = state_after_exit(state, &root);
        assert_eq!(next.app_info().recovery_notice, None);
        next.at_exit();
    }

    #[tokio::test]
    async fn acknowledging_without_a_notice_does_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(boot(Ok(DataRoot::at(dir.path().join("data")))));
        assert_eq!(state.app_info().recovery_notice, None);
        state
            .acknowledge_recovery()
            .await
            .expect("nothing to dismiss");
        // Not even a store is needed when there is nothing to acknowledge.
        state.at_exit();
        state
            .acknowledge_recovery()
            .await
            .expect("still nothing to dismiss");
    }

    /// The page passes no path, and the shell passes on only the one startup recorded: with two
    /// repairs on record, dismissing the reported (newest) one leaves the older one, which the next
    /// launch reports in turn.
    #[tokio::test]
    async fn only_the_repair_startup_reported_is_acknowledged() {
        let (_dir, root, older) = repaired();
        let newer = damage_and_repair(&root);
        assert_ne!(older, newer);

        let state = state(boot_next(&root));
        let notice = state.app_info().recovery_notice.expect("a notice");
        assert!(names(&notice, &newer), "the newest repair first: {notice}");
        state.acknowledge_recovery().await.expect("acknowledge");
        assert!(!record(&newer).exists());
        assert!(record(&older).exists(), "the other repair was not touched");

        let next = state_after_exit(state, &root);
        let notice = next.app_info().recovery_notice.expect("the older repair");
        assert!(names(&notice, &older), "{notice}");
        next.at_exit();
    }

    #[tokio::test]
    async fn acknowledging_with_the_store_closed_fails_and_keeps_the_notice() {
        let (_dir, root, quarantined) = repaired();
        let state = state(boot_next(&root));
        state.at_exit();
        let error = state
            .acknowledge_recovery()
            .await
            .expect_err("the store is closed");
        assert_eq!(error.code, AppErrorCode::AcknowledgeFailed);
        assert!(
            error.message.starts_with("Could not dismiss the notice: "),
            "{}",
            error.message
        );
        assert!(
            state.app_info().recovery_notice.is_some(),
            "the notice stays"
        );
        assert!(record(&quarantined).exists());
        let next = state_after_exit(state, &root);
        assert!(
            next.app_info().recovery_notice.is_some(),
            "and shows again on the next launch"
        );
        next.at_exit();
    }

    /// Without chats at all (startup failed after the repair was found), dismissing fails and the
    /// notice stays.
    #[tokio::test]
    async fn acknowledging_without_chats_fails_and_keeps_the_notice() {
        let (_dir, root, _quarantined) = repaired();
        let booted = boot_next(&root);
        let launch = LaunchOptions::from_lookup(|_| None);
        let state = AppState::new(
            booted,
            None,
            &os(),
            GlassEffect::None,
            launch,
            "0.1.0".into(),
        );
        let error = state
            .acknowledge_recovery()
            .await
            .expect_err("no store to ask");
        assert_eq!(error.code, AppErrorCode::AcknowledgeFailed);
        assert!(state.app_info().recovery_notice.is_some());
    }

    /// An I/O failure (here: no permission to delete in the backups folder) keeps the notice, and
    /// a later attempt that can write succeeds.
    #[cfg(unix)]
    #[tokio::test]
    async fn acknowledging_that_cannot_remove_the_record_fails_and_keeps_the_notice() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, root, quarantined) = repaired();
        let state = state(boot_next(&root));
        let backups = Path::new(&quarantined).parent().expect("backups folder");
        let mode = std::fs::metadata(backups).expect("metadata").permissions();
        std::fs::set_permissions(backups, std::fs::Permissions::from_mode(0o500)).expect("chmod");
        // Root ignores permissions, so there would be nothing to fail.
        if std::fs::write(backups.join("probe"), b"").is_ok() {
            std::fs::set_permissions(backups, mode).expect("restore");
            eprintln!("skipped: this user can write to a read-only folder");
            state.at_exit();
            return;
        }

        let result = state.acknowledge_recovery().await;
        std::fs::set_permissions(backups, mode).expect("restore");
        let error = result.expect_err("the record could not be removed");
        assert_eq!(error.code, AppErrorCode::AcknowledgeFailed);
        assert!(
            error.message.starts_with("Could not dismiss the notice: "),
            "{}",
            error.message
        );
        assert!(error.message.contains(&quarantined), "{}", error.message);
        assert!(
            state.app_info().recovery_notice.is_some(),
            "the notice stays"
        );
        assert!(record(&quarantined).exists());

        state
            .acknowledge_recovery()
            .await
            .expect("works once it can write");
        assert_eq!(state.app_info().recovery_notice, None);
        state.at_exit();
    }

    #[tokio::test]
    async fn a_launch_after_a_crash_holds_its_integrity_check_until_the_window_shows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("data"));
        let first = boot(Ok(root.clone()));
        let db = first.store.as_ref().expect("store").path().to_path_buf();
        let wal = PathBuf::from(format!("{}-wal", db.display()));
        let (file, log) = (
            std::fs::read(&db).expect("read"),
            std::fs::read(&wal).expect("read the log"),
        );
        drop(first);
        // Put back what a crash would have left: the file and its unfolded log.
        std::fs::write(&db, file).expect("restore");
        std::fs::write(&wal, log).expect("restore the log");

        let state = state(boot_next(&root));
        assert_eq!(state.integrity.pending(), Some(db));
        assert!(!state.integrity.close(), "nothing running yet");
        assert_eq!(state.integrity.pending(), None, "and now nothing ever will");
        state.at_exit();
    }
}
