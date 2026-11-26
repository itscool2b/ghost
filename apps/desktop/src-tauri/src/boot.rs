//! The startup sequence: data root, device identity, active profile, the profile's owner lock,
//! profile database.
//!
//! [`boot`] never fails and never panics. Repairs ghost-core made on the way (a recreated device
//! identity, a quarantined database, a migration) become notices; the first step that cannot
//! complete becomes the fatal error the UI shows on its error screen, with the data path, instead
//! of the app crashing before a window exists.
//!
//! The data root is ghost-core's [`DataRoot::resolve`] and nothing else ([`startup`]): the folder in
//! `GHOST_DATA_DIR`, or `dev.onghost.desktop` in the local app data folder
//! (`dev.onghost.desktop.dev` for a development build, so a build from a working copy never opens
//! the installed app's data). The CLI resolves the same one, so both always open the same profile.
//! Tauri's path API is not consulted: its `app_data_dir()` is Roaming AppData on Windows, which
//! managed machines copy between PCs, and `device.json` and the databases must never travel. Its
//! `app_local_data_dir()` is an installed build's default root, because tauri.conf.json's
//! identifier is ghost-core's `APP_IDENTIFIER` (`config_checks.rs` holds them together), so the
//! WebView data Tauri keeps there on Windows and Linux sits beside ours. Any other folder gets the
//! WebView data inside it (`LaunchOptions::webview_data_dir`).
//!
//! One process runs a profile's sessions, so startup takes the profile's owner lock
//! ([`ProfileOwner`]) between opening the profile and opening its database. A second launch with the
//! same data folder gets the error screen, saying another Ghost runs with that profile.

use std::path::PathBuf;

use ghost_core::device::{DeviceIdentity, DeviceLoad};
use ghost_core::owner::ProfileOwner;
use ghost_core::paths::DataRoot;
use ghost_core::profile::{Profile, ProfileMeta, Profiles};
use ghost_core::store::{OpenOutcome, Salvage, Store};
use serde::Serialize;

use crate::integrity;

/// The profile database as the info card shows it.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct DbInfo {
    pub path: String,
    pub schema_version: u32,
    /// What opening the database had to do (create, migrate, recover).
    pub outcome: OpenOutcome,
    /// How long opening it took, in milliseconds: the backup and the migration too when it
    /// migrated, all before the window exists.
    pub open_ms: u32,
    /// How long startup recovery took, which every first read waits behind; `null` until it
    /// finished. Read live (`AppState::app_info`).
    pub recovery_ms: Option<u32>,
    /// How long building the indexes recovery reads through took, after startup; `null` when
    /// this launch built none.
    pub indexes_ms: Option<u32>,
}

/// A database repair whose notice the user has not dismissed yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// Where ghost-core moved the damaged database, exactly as
    /// [`OpenOutcome::RecoveredFromCorruption`] reported it: the only path the shell ever hands
    /// back to [`Store::acknowledge_recovery`]. The page cannot name one.
    pub quarantined: String,
    /// The sentence the UI shows ([`recovery_notice`]).
    pub notice: String,
}

/// Everything startup produced, complete or not.
#[derive(Debug)]
pub struct Booted {
    /// `None` only when no data folder could be determined at all.
    pub data_dir: Option<PathBuf>,
    pub device: Option<DeviceIdentity>,
    pub profile: Option<ProfileMeta>,
    /// The active profile's folder (`ProfileMeta` and this make a `ghost_core::profile::Profile`).
    pub profile_dir: Option<PathBuf>,
    pub store: Option<Store>,
    /// This process owns the profile's sessions while this lives. Taken before the store opened;
    /// it moves into the chat store with it. Declared after `store` because fields drop in
    /// declaration order: like the chat store's writer, a launch lets go of the profile only once
    /// its database has closed, so whoever still holds the database's lock (even a process spawned
    /// at that moment, which holds copies of open files until its exec) holds the profile too, and
    /// the next launch is told the profile is in use instead of finding the database busy.
    pub owner: Option<ProfileOwner>,
    pub db: Option<DbInfo>,
    /// Sentences the UI lists: repairs that happened, data that was moved aside.
    pub notices: Vec<String>,
    /// The database repair to report on every launch until the user dismisses it. Not in
    /// `notices`, which cannot be dismissed.
    pub recovery: Option<Recovery>,
    /// The database to check in the background once the window is up ([`crate::integrity`]).
    pub integrity_check: Option<PathBuf>,
    /// The error that stopped startup, as a sentence the UI shows as is.
    pub fatal: Option<String>,
}

/// Runs the startup sequence on the data root ghost-core resolves: the one the app uses.
pub fn startup() -> Booted {
    boot(DataRoot::resolve())
}

/// Runs the startup sequence on `root`.
pub fn boot(root: ghost_core::Result<DataRoot>) -> Booted {
    boot_selected(root, None)
}

/// Prepare an explicit profile without changing state.json. The caller publishes that choice only
/// after its store and runtime are ready. All IO runs on a blocking worker.
pub(crate) fn boot_profile(root: DataRoot, profile: Profile) -> Booted {
    boot_selected(Ok(root), Some(profile))
}

fn boot_selected(root: ghost_core::Result<DataRoot>, selected: Option<Profile>) -> Booted {
    let preparing = selected.is_some();
    let mut booted = Booted {
        data_dir: None,
        device: None,
        profile: None,
        profile_dir: None,
        owner: None,
        store: None,
        db: None,
        notices: Vec::new(),
        recovery: None,
        integrity_check: None,
        fatal: None,
    };
    let root = match root {
        Ok(root) => root,
        Err(error) => return booted.fail("find the data folder", &error),
    };
    booted.data_dir = Some(root.root().to_path_buf());

    if let Err(error) = root.ensure() {
        return booted.fail("create the data folder", &error);
    }

    match DeviceIdentity::load_or_create(&root) {
        Ok(load) => {
            if let DeviceLoad::Recreated { backup, .. } = &load {
                booted.notices.push(format!(
                    "This device's identity file was damaged, so Ghost created a new one. The old file was kept at {}.",
                    backup.display()
                ));
            }
            booted.device = Some(load.identity().clone());
        }
        Err(error) => return booted.fail("load this device's identity", &error),
    }

    let profile = match selected
        .map(Ok)
        .unwrap_or_else(|| Profiles::new(root).open_active())
    {
        Ok(profile) => profile,
        Err(error) => return booted.fail("open your profile", &error),
    };
    booted.profile = Some(profile.meta.clone());
    booted.profile_dir = Some(profile.dir.clone());

    match ProfileOwner::acquire(&profile) {
        Ok(owner) => booted.owner = Some(owner),
        Err(error) => return booted.fail("take ownership of your profile", &error),
    }

    let opening = std::time::Instant::now();
    match Store::open(&profile) {
        Ok((store, info)) => {
            let open_ms = u32::try_from(opening.elapsed().as_millis()).unwrap_or(u32::MAX);
            tracing::info!(open_ms, "opened the profile database");
            if preparing && info.unclean_shutdown {
                match store.check_integrity() {
                    Ok(ghost_core::store::Integrity::Ok) => {}
                    Ok(ghost_core::store::Integrity::Damaged { .. }) => {
                        booted.fatal = Some("The selected profile needs database recovery. Existing data was preserved.".into());
                        return booted;
                    }
                    Err(error) => return booted.fail("check the selected database", &error),
                }
            }
            match &info.outcome {
                OpenOutcome::Created | OpenOutcome::Opened => {}
                OpenOutcome::Migrated { from, to, backup } => {
                    booted.notices.push(match backup {
                        Some(backup) => format!(
                            "Your data was upgraded from version {from} to {to}. A copy of the old database was kept at {backup}."
                        ),
                        None => format!("Your data was upgraded from version {from} to {to}."),
                    });
                }
                OpenOutcome::RecoveredFromCorruption {
                    quarantined,
                    salvage,
                } => {
                    booted.recovery = Some(Recovery {
                        quarantined: quarantined.clone(),
                        notice: recovery_notice(quarantined, salvage),
                    });
                }
            }
            booted.integrity_check =
                integrity::target(store.path(), info.unclean_shutdown && !preparing);
            booted.db = Some(DbInfo {
                path: info.path.display().to_string(),
                schema_version: info.schema_version,
                outcome: info.outcome,
                open_ms,
                recovery_ms: None,
                indexes_ms: None,
            });
            booted.store = Some(store);
        }
        Err(error) => return booted.fail("open your database", &error),
    }

    booted
}

/// Boots `root` as the next launch does, once the last owner's lock is really free. A test that
/// drops an owner and boots again at once can find the lock still held for a moment: a process
/// another test spawns at that instant holds a copy of every open file until its exec closes the
/// close-on-exec ones. So this retries `ProfileInUse` for up to 5 s. The database's open lock can
/// be held over the same way, but never without the owner's (`Booted` lets go of the owner last),
/// so the same retry covers it.
#[cfg(test)]
pub(crate) fn boot_next(root: &DataRoot) -> Booted {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let booted = boot(Ok(root.clone()));
        let in_use = booted.fatal.as_deref().is_some_and(|fatal| {
            fatal.starts_with("Ghost is already running with the profile in ")
        });
        if !in_use || Instant::now() >= deadline {
            return booted;
        }
        drop(booted);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The notice for a database ghost-core had to repair: how much of it the new database holds, and
/// where the damaged original is, since whatever the copy lacks is only there.
pub fn recovery_notice(quarantined: &str, salvage: &Salvage) -> String {
    match salvage {
        Salvage::Complete => format!(
            "Your database was damaged, so Ghost rebuilt it. Nothing was lost; the damaged copy was kept at {quarantined}."
        ),
        Salvage::Partial { tables } if !tables.is_empty() => format!(
            "Your database was damaged, so Ghost rebuilt it from what it could still read. Some records could not be read ({}); the damaged copy was kept at {quarantined}.",
            tables.join(", ")
        ),
        Salvage::Partial { .. } => format!(
            "Your database was damaged, so Ghost rebuilt it from what it could still read. Some records could not be read; the damaged copy was kept at {quarantined}."
        ),
        Salvage::Nothing => format!(
            "Your database was damaged and nothing in it could be read, so Ghost started a fresh one. The damaged copy was kept at {quarantined}."
        ),
    }
}

impl Booted {
    fn fail(mut self, step: &str, error: &ghost_core::CoreError) -> Booted {
        tracing::error!(%error, step, "startup could not complete");
        // ghost-core's messages are complete sentences; keep them verbatim.
        self.fatal = Some(error.to_string());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    use ghost_core::paths::DATA_DIR_ENV;

    use crate::test_support::run_child;

    fn fresh() -> (tempfile::TempDir, DataRoot) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("data"));
        (dir, root)
    }

    /// `<db>-wal` and the like.
    fn sidecar(db: &Path, suffix: &str) -> PathBuf {
        PathBuf::from(format!("{}{suffix}", db.display()))
    }

    /// Ends the session the way a crash does: the database file as it was, and a write-ahead log
    /// that still holds the session's writes. Both are copied while the store is open and put back
    /// after it closed, since a clean close folds the log into the file and deletes it.
    fn crash(store: Store) {
        let db = store.path().to_path_buf();
        let wal = sidecar(&db, "-wal");
        let file = fs::read(&db).expect("read the database");
        let log = fs::read(&wal).expect("read the log");
        assert!(!log.is_empty(), "the session wrote to its log");
        drop(store);
        assert!(!wal.exists(), "a clean close folds the log back");
        fs::write(&db, file).expect("put the database back");
        fs::write(&wal, log).expect("put the log back");
    }

    #[test]
    fn first_launch_creates_everything_without_notices() {
        let (_dir, root) = fresh();
        let booted = boot(Ok(root.clone()));
        assert_eq!(booted.fatal, None);
        assert!(booted.notices.is_empty(), "{:?}", booted.notices);
        assert_eq!(booted.recovery, None);
        assert_eq!(booted.integrity_check, None, "nothing to check yet");
        assert_eq!(booted.data_dir.as_deref(), Some(root.root()));
        assert!(booted.device.is_some());
        assert_eq!(
            booted.profile.as_ref().map(|p| p.id.as_str()),
            Some(ghost_core::profile::LOCAL_PROFILE_ID)
        );
        let db = booted.db.expect("db info");
        assert_eq!(db.outcome, OpenOutcome::Created);
        assert_eq!(db.schema_version, ghost_core::store::SCHEMA_VERSION);
        assert!(booted.store.is_some());
    }

    #[test]
    fn second_launch_opens_the_same_data() {
        let (_dir, root) = fresh();
        let first = boot(Ok(root.clone()));
        let device_id = first.device.expect("device").device_id;
        drop(first.store);
        drop(first.owner);
        let second = boot_next(&root);
        assert_eq!(second.fatal, None);
        assert_eq!(second.device.expect("device").device_id, device_id);
        assert_eq!(second.db.expect("db").outcome, OpenOutcome::Opened);
        assert_eq!(
            second.integrity_check, None,
            "the first launch closed the database cleanly"
        );
    }

    #[test]
    fn a_launch_after_a_crash_asks_for_an_integrity_check() {
        let (_dir, root) = fresh();
        let first = boot(Ok(root.clone()));
        let db = first.store.as_ref().expect("store").path().to_path_buf();
        crash(first.store.expect("store"));
        drop(first.owner);

        let second = boot_next(&root);
        assert_eq!(second.fatal, None);
        assert_eq!(second.db.as_ref().expect("db").outcome, OpenOutcome::Opened);
        assert_eq!(second.integrity_check, Some(db), "the store's own file");
        drop(second);

        let third = boot_next(&root);
        assert_eq!(
            third.integrity_check, None,
            "the launch after the crash closed the database cleanly"
        );
    }

    #[test]
    fn a_damaged_device_file_becomes_a_notice() {
        let (_dir, root) = fresh();
        root.ensure().expect("ensure");
        fs::write(root.device_file(), b"{ not json").expect("write");
        let booted = boot(Ok(root));
        assert_eq!(booted.fatal, None);
        assert_eq!(booted.notices.len(), 1, "{:?}", booted.notices);
        assert!(booted.notices[0].contains("identity file was damaged"));
        assert!(booted.notices[0].contains("device.json.corrupt-"));
    }

    #[test]
    fn a_damaged_database_is_recovered_and_reported_until_dismissed() {
        let (_dir, root) = fresh();
        let first = boot(Ok(root.clone()));
        let db_path = PathBuf::from(first.db.expect("db").path);
        drop(first.store);
        drop(first.owner);
        fs::write(&db_path, vec![0x5a; 8192]).expect("corrupt the database");

        let booted = boot_next(&root);
        assert_eq!(booted.fatal, None);
        let db = booted.db.expect("db");
        let OpenOutcome::RecoveredFromCorruption {
            quarantined,
            salvage,
        } = &db.outcome
        else {
            panic!("expected a recovery, got {:?}", db.outcome);
        };
        assert!(PathBuf::from(quarantined).exists());
        assert_eq!(
            booted.recovery,
            Some(Recovery {
                quarantined: quarantined.clone(),
                notice: recovery_notice(quarantined, salvage),
            }),
            "one dismissible notice, naming where the damaged copy is"
        );
        assert!(
            booted.notices.is_empty(),
            "not among the notices that cannot be dismissed: {:?}",
            booted.notices
        );
        assert!(
            booted
                .recovery
                .as_ref()
                .is_some_and(|recovery| recovery.notice.contains(quarantined.as_str()))
        );
        drop(booted.store);
        drop(booted.owner);

        let again = boot_next(&root);
        assert_eq!(
            again.recovery.as_ref().map(|r| r.quarantined.as_str()),
            Some(quarantined.as_str()),
            "the next launch reports the same repair: nobody dismissed it"
        );
    }

    #[test]
    fn the_recovery_notice_says_how_much_survived() {
        let kept = "/data/backups/aura.db.corrupt-1";
        let complete = recovery_notice(kept, &Salvage::Complete);
        assert!(complete.contains("Nothing was lost"), "{complete}");
        let partial = recovery_notice(
            kept,
            &Salvage::Partial {
                tables: vec!["chats".into(), "events".into()],
            },
        );
        assert!(
            partial.contains("Some records could not be read (chats, events)"),
            "{partial}"
        );
        let unnamed = recovery_notice(kept, &Salvage::Partial { tables: Vec::new() });
        assert!(!unnamed.contains("()"), "{unnamed}");
        let nothing = recovery_notice(kept, &Salvage::Nothing);
        assert!(nothing.contains("started a fresh one"), "{nothing}");
        for notice in [complete, partial, unnamed, nothing] {
            assert!(notice.starts_with("Your database was damaged"), "{notice}");
            assert!(notice.contains(kept), "{notice}");
        }
    }

    #[test]
    fn data_from_a_newer_version_is_fatal_and_untouched() {
        let (_dir, root) = fresh();
        let first = boot(Ok(root.clone()));
        let store = first.store.expect("store");
        store
            .conn()
            .pragma_update(None, "user_version", 99)
            .expect("bump user_version");
        let db_path = store.path().to_path_buf();
        drop(store);
        drop(first.owner);
        let before = fs::read(&db_path).expect("read");

        let booted = boot_next(&root);
        let fatal = booted.fatal.expect("fatal error");
        assert!(fatal.contains("newer version of Ghost"), "{fatal}");
        assert!(booted.store.is_none());
        assert_eq!(
            booted.integrity_check, None,
            "no database, nothing to check"
        );
        assert!(booted.device.is_some(), "steps before the failure are kept");
        assert!(booted.data_dir.is_some());
        assert_eq!(
            fs::read(&db_path).expect("read"),
            before,
            "nothing was written"
        );
    }

    /// A second launch with the same data folder while the first still runs: the profile is the
    /// first one's, so the second stops before touching the database, and says why.
    #[test]
    fn boot_reports_profile_in_use() {
        let (_dir, root) = fresh();
        let first = boot(Ok(root.clone()));
        assert_eq!(first.fatal, None);
        assert!(first.owner.is_some(), "the first launch owns the profile");
        assert!(first.profile_dir.is_some());

        let second = boot(Ok(root.clone()));
        let fatal = second.fatal.clone().expect("refused");
        assert!(
            fatal.starts_with("Ghost is already running with the profile in "),
            "{fatal}"
        );
        assert!(fatal.contains("GHOST_DATA_DIR"), "{fatal}");
        assert!(second.store.is_none(), "the database was not opened");
        assert!(second.owner.is_none());
        assert!(
            second.profile.is_some(),
            "steps before the refusal are kept"
        );
        drop(second);

        drop(first);
        let third = boot_next(&root);
        assert_eq!(third.fatal, None, "free once the first launch ended");
        assert!(third.owner.is_some());
    }

    #[test]
    fn an_unusable_data_folder_is_fatal_not_a_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A file where the data folder should be: creating folders under it fails on every OS.
        let blocker = dir.path().join("data");
        fs::write(&blocker, b"not a folder").expect("write");
        let booted = boot(Ok(DataRoot::at(blocker.join("nested"))));
        let fatal = booted.fatal.expect("fatal error");
        assert!(fatal.starts_with("Could not access"), "{fatal}");
        assert!(booted.device.is_none());
        assert!(booted.data_dir.is_some(), "the path is still shown");
    }

    #[test]
    fn no_data_folder_at_all_is_fatal() {
        let booted = boot(Err(ghost_core::CoreError::NoDataDir));
        let fatal = booted.fatal.expect("fatal error");
        assert!(fatal.contains("GHOST_DATA_DIR"), "{fatal}");
        assert_eq!(booted.data_dir, None);
        assert_eq!(booted.recovery, None);
        assert_eq!(booted.integrity_check, None);
    }

    /// Whether `root` holds what a first launch creates.
    fn started_in(root: &Path) -> bool {
        root.join("device.json").is_file()
            && root
                .join("profiles")
                .join(ghost_core::profile::LOCAL_PROFILE_ID)
                .join("aura.db")
                .is_file()
    }

    /// Marks a re-executed copy of the test binary; holds the root that child should start in.
    const CHILD_EXPECTS: &str = "GHOST_DESKTOP_TEST_EXPECTED_ROOT";

    /// The app starts exactly where ghost-core (and so the CLI) resolves, `GHOST_DATA_DIR`
    /// included, which is what lets a smoke run start in a throwaway folder.
    #[test]
    fn startup_uses_the_data_root_aura_core_resolves() {
        if let Some(expected) = std::env::var_os(CHILD_EXPECTS) {
            let booted = startup();
            assert_eq!(booted.fatal, None);
            assert_eq!(booted.data_dir.as_deref(), Some(Path::new(&expected)));
            let root = DataRoot::resolve().expect("resolve in child");
            assert_eq!(root.root(), Path::new(&expected));
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let expected = dir.path().join("throwaway data");
        run_child(
            "boot::tests::startup_uses_the_data_root_aura_core_resolves",
            |child| {
                child
                    .env(CHILD_EXPECTS, &expected)
                    .env(DATA_DIR_ENV, &expected);
            },
        );
        assert!(started_in(&expected));
    }

    /// Marks the child of the next test; holds its throwaway home folder.
    #[cfg(not(windows))]
    const CHILD_HOME: &str = "GHOST_DESKTOP_TEST_THROWAWAY_HOME";

    /// Without `GHOST_DATA_DIR` the app starts in its default folder in the local app data
    /// folder: `dev.onghost.desktop`, the folder Tauri's `app_local_data_dir()` names, for an
    /// installed build, and `dev.onghost.desktop.dev` for this development build. The child gets a
    /// throwaway home folder, which is what the local app data folder is built from on macOS and
    /// Linux, so it boots there instead of in the user's real data. Windows looks its known
    /// folders up without the environment, so this cannot run there without touching real data;
    /// ghost-core's tests pin the local (never roaming) folder there.
    #[cfg(not(windows))]
    #[test]
    fn startup_without_an_override_uses_the_local_app_data_folder() {
        fn local_app_data(home: &Path) -> PathBuf {
            if cfg!(target_os = "macos") {
                home.join("Library").join("Application Support")
            } else {
                home.join(".local").join("share")
            }
        }

        if let Some(home) = std::env::var_os(CHILD_HOME) {
            let booted = startup();
            assert_eq!(booted.fatal, None);
            assert_eq!(
                booted.data_dir,
                Some(local_app_data(Path::new(&home)).join(ghost_core::paths::DEFAULT_FOLDER))
            );
            return;
        }

        let home = tempfile::tempdir().expect("tempdir");
        run_child(
            "boot::tests::startup_without_an_override_uses_the_local_app_data_folder",
            |child| {
                child
                    .env(CHILD_HOME, home.path())
                    .env("HOME", home.path())
                    .env_remove("XDG_DATA_HOME")
                    .env_remove(DATA_DIR_ENV);
                for name in ghost_core::paths::LEGACY_DATA_DIR_ENVS {
                    child.env_remove(name);
                }
            },
        );
        assert!(started_in(
            &local_app_data(home.path()).join(ghost_core::paths::DEFAULT_FOLDER)
        ));
    }
}
