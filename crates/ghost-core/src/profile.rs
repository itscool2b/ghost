//! Profiles: separate sets of projects and chats on one machine.
//!
//! Every profile is Local: its data lives only on this machine. A fresh install has one, `local`.
//! A data folder may hold more (Ghost once kept an account's workspace as a second Local
//! profile), and the profile switcher lists them all. Each profile is a directory
//! `<data root>/profiles/<id>/`:
//!
//! ```text
//! profile.json     ProfileMeta; written last when creating, so its presence means "complete"
//! aura.db          SQLite store
//! attachments/     content-addressed files referenced from chats
//! checkpoints/     shadow git repos for per-turn snapshots
//! logs/            raw protocol logs per session
//! backups/         pre-migration backups and quarantined databases
//! ```
//!
//! `<data root>/state.json` records the active profile. Whatever state the files are in, opening the
//! active profile yields a usable profile: a missing, corrupt, or dangling `state.json` falls back to
//! Local (recreating it if needed) with a warning, and a stray file where a profile folder belongs is
//! moved aside, never an error that blocks startup. A link where a folder belongs is the exception:
//! one that leads nowhere (the folder is on a disk that isn't connected) is an error, because a new
//! profile in its place would leave the real one behind.
//!
//! Ghost accounts were removed. A folder one of them left behind (its `profile.json` says
//! `"kind": "account"`, and its database is encrypted) is never opened, listed, changed or deleted:
//! [`Profiles::list`] skips it, and when `state.json` names it, Local opens instead.

use std::fs;
use std::path::{Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{CoreError, Result};
use crate::fsutil::{self, JsonFile};
use crate::ids::new_id;
use crate::paths::DataRoot;
use crate::time::now_millis;

/// Id of the profile every install starts with.
pub const LOCAL_PROFILE_ID: &str = "local";

const PROFILE_FILE: &str = "profile.json";
const DB_FILE: &str = "aura.db";
const ATTACHMENTS_DIR: &str = "attachments";
const CHECKPOINTS_DIR: &str = "checkpoints";
const LOGS_DIR: &str = "logs";
const BACKUPS_DIR: &str = "backups";
const WORKTREES_DIR: &str = "worktrees";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// Data that lives only on this machine.
    Local,
}

/// The name of a profile created without one.
const DEFAULT_DISPLAY_NAME: &str = "Local";

/// What a `profile.json` on disk may say its kind is. Wider than [`ProfileKind`]: a folder left
/// by a removed Ghost account still says `account`, and reading that as its own case (rather
/// than as a damaged file) is what lets the folder be recognised and left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredKind {
    Local,
    Account,
}

/// `profile.json` as stored, before its kind is checked.
#[derive(Debug, Deserialize)]
struct StoredMeta {
    id: String,
    kind: StoredKind,
    display_name: String,
    created_at: i64,
}

/// The contents of `profile.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProfileMeta {
    pub id: String,
    pub kind: ProfileKind,
    pub display_name: String,
    /// Unix ms, exported to TypeScript as `number` (see `DeviceIdentity::created_at`).
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
}

/// A profile and the directory holding its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub meta: ProfileMeta,
    pub dir: PathBuf,
}

impl Profile {
    /// `<dir>/aura.db`
    pub fn db_path(&self) -> PathBuf {
        self.dir.join(DB_FILE)
    }

    /// `<dir>/attachments`
    pub fn attachments_dir(&self) -> PathBuf {
        self.dir.join(ATTACHMENTS_DIR)
    }

    /// `<dir>/checkpoints`
    pub fn checkpoints_dir(&self) -> PathBuf {
        self.dir.join(CHECKPOINTS_DIR)
    }

    /// `<dir>/worktrees`: the Git worktrees Ghost creates for isolated chats, and their
    /// records.
    pub fn worktrees_dir(&self) -> PathBuf {
        self.dir.join(WORKTREES_DIR)
    }

    /// `<dir>/logs`
    pub fn logs_dir(&self) -> PathBuf {
        self.dir.join(LOGS_DIR)
    }

    /// `<dir>/backups`
    pub fn backups_dir(&self) -> PathBuf {
        self.dir.join(BACKUPS_DIR)
    }

    /// `<dir>/profile.json`
    pub fn meta_path(&self) -> PathBuf {
        self.dir.join(PROFILE_FILE)
    }

    /// Recreates any missing subdirectory (a user may delete `logs/`), so callers can rely on them.
    /// Something other than a folder where one belongs (a stray file named `logs`) is moved aside
    /// first, since failing here would fail every launch.
    fn ensure_layout(&self) -> Result<()> {
        fsutil::ensure_private_dir(&self.dir)?;
        for dir in [
            self.attachments_dir(),
            self.checkpoints_dir(),
            self.logs_dir(),
            self.backups_dir(),
        ] {
            fsutil::ensure_private_dir(&dir)?;
        }
        Ok(())
    }
}

/// The contents of `state.json`. Fields written by a newer Ghost are kept in `extra` and written
/// back untouched, so running an older build doesn't erase them.
#[derive(Debug, Serialize, Deserialize)]
struct AppState {
    active_profile: String,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// What reading one profile directory produced.
#[derive(Debug)]
enum ProfileRead {
    Missing,
    Valid(Profile),
    /// A folder left by a removed Ghost account. It is never opened, changed or deleted.
    RetiredAccount,
    Invalid(CoreError),
}

/// Access to the profiles under one data root.
#[derive(Debug, Clone)]
pub struct Profiles {
    root: DataRoot,
}

impl Profiles {
    pub fn new(root: DataRoot) -> Profiles {
        Profiles { root }
    }

    pub fn root(&self) -> &DataRoot {
        &self.root
    }

    /// Opens the profile named in `state.json`, bootstrapping Local on first run.
    ///
    /// Never fails because of what's in the files: a missing or corrupt `state.json`, or one naming a
    /// profile that is missing, invalid, unreadable, or left by a removed Ghost account, falls
    /// back to Local (recreating it if needed), rewrites `state.json`, and logs a warning. The
    /// profile it named is left exactly as it was. A `state.json` that can't be read also falls back
    /// to Local, but is left alone, so a passing fault doesn't forget the user's choice. A file where
    /// a profile folder or one of its subfolders belongs is moved aside. Only real I/O failures (disk
    /// full, no permission to the data root) are errors, and a link to one of these folders that
    /// leads nowhere ([`CoreError::BrokenLink`]): falling back to a new Local profile there would
    /// leave the real one behind for when its disk is back.
    pub fn open_active(&self) -> Result<Profile> {
        self.root.ensure()?;
        let state_path = self.root.state_file();
        let mut extra = Map::new();
        match fsutil::read_json::<AppState>(&state_path) {
            JsonFile::Valid(state) => {
                let read = match self.read_profile(&state.active_profile) {
                    Ok(read) => read,
                    // Its folder is on a disk that isn't connected. Falling back would make Local
                    // the active profile for good, as if the user had switched.
                    Err(error @ CoreError::BrokenLink { .. }) => return Err(error),
                    Err(error) => ProfileRead::Invalid(error),
                };
                match read {
                    ProfileRead::Valid(profile) => {
                        profile.ensure_layout()?;
                        return Ok(profile);
                    }
                    ProfileRead::Missing => tracing::warn!(
                        profile = %state.active_profile,
                        "the active profile no longer exists; falling back to Local"
                    ),
                    ProfileRead::RetiredAccount => tracing::warn!(
                        profile = %state.active_profile,
                        "the active profile belongs to a Ghost account, which this version no longer opens; leaving its folder as it is and opening Local"
                    ),
                    ProfileRead::Invalid(error) => tracing::warn!(
                        profile = %state.active_profile,
                        %error,
                        "the active profile can't be opened; falling back to Local"
                    ),
                }
                extra = state.extra;
            }
            JsonFile::Missing if self.profiles_dir_is_empty() => {
                tracing::info!("first run: creating the Local profile");
            }
            JsonFile::Missing => tracing::warn!("state.json is missing; falling back to Local"),
            JsonFile::Unusable(error) => {
                let backup = fsutil::quarantine_file(&state_path)?;
                tracing::warn!(%error, backup = %backup.display(), "state.json is unusable; falling back to Local");
            }
            JsonFile::Unreadable(error) => {
                // A read error says nothing about the contents (a passing I/O fault, a permissions
                // mix-up), so the file is left as it is: this run uses Local, and the next one reads
                // the user's choice again.
                tracing::warn!(%error, "state.json could not be read; using Local for now and leaving it alone");
                return self.ensure_local();
            }
        }
        let local = self.ensure_local()?;
        self.write_state(LOCAL_PROFILE_ID, extra)?;
        Ok(local)
    }

    /// Every readable profile: `local` first, then the others, oldest first (a stable order for the
    /// profile switcher even when two share a creation millisecond). A directory that isn't a valid
    /// profile (bad name, missing or corrupt `profile.json`, id mismatch) is skipped with a warning
    /// so one damaged profile can't hide the others. So is a folder left by a removed Ghost
    /// account, which stays on disk untouched.
    pub fn list(&self) -> Result<Vec<ProfileMeta>> {
        let profiles_dir = self.root.profiles_dir();
        let entries = match fs::read_dir(&profiles_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(CoreError::Io {
                    path: profiles_dir,
                    source,
                });
            }
        };

        let mut profiles = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    tracing::warn!(dir = %profiles_dir.display(), %error, "skipping an unreadable profiles entry");
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_dir() {
                // Stray files such as .DS_Store are not profiles.
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                tracing::warn!(path = %path.display(), "skipping a profile folder with a non-UTF-8 name");
                continue;
            };
            match self.read_profile_or_invalid(&name) {
                ProfileRead::Valid(profile) => profiles.push(profile.meta),
                ProfileRead::Missing => {
                    tracing::warn!(path = %path.display(), "skipping a profile folder without profile.json");
                }
                ProfileRead::RetiredAccount => {
                    tracing::info!(path = %path.display(), "skipping the profile of a Ghost account, which this version no longer opens; its folder is left as it is");
                }
                ProfileRead::Invalid(error) => {
                    tracing::warn!(path = %path.display(), %error, "skipping an unreadable profile");
                }
            }
        }
        profiles.sort_by(|a, b| list_order(a).cmp(&list_order(b)));
        Ok(profiles)
    }

    /// Creates a profile. It gets the id `local` while no `local` profile exists; otherwise the id
    /// is a new UUIDv7. A blank display name becomes "Local".
    ///
    /// No screen creates a second profile today: [`Profiles::open_active`] makes `local`, and the
    /// others a data folder holds were made by earlier versions.
    pub fn create(&self, display_name: &str) -> Result<Profile> {
        self.root.ensure()?;
        let id = if matches!(self.read_profile(LOCAL_PROFILE_ID)?, ProfileRead::Missing) {
            LOCAL_PROFILE_ID.to_string()
        } else {
            new_id()
        };
        let trimmed = display_name.trim();
        let display_name = if trimmed.is_empty() {
            DEFAULT_DISPLAY_NAME
        } else {
            trimmed
        };
        self.write_new_profile(id, display_name.to_string())
    }

    /// Loads an existing profile by id. A folder left by a removed Ghost account is not a
    /// profile this version has: it answers as not found.
    pub fn get(&self, id: &str) -> Result<Profile> {
        match self.read_profile(id)? {
            ProfileRead::Valid(profile) => Ok(profile),
            ProfileRead::Missing | ProfileRead::RetiredAccount => {
                Err(CoreError::ProfileNotFound(id.to_string()))
            }
            ProfileRead::Invalid(error) => Err(error),
        }
    }

    /// Makes `id` the active profile and persists the choice. The id is validated before it touches
    /// the filesystem, and the profile must exist and be readable.
    pub fn set_active(&self, id: &str) -> Result<Profile> {
        let profile = self.get(id)?;
        self.root.ensure()?;
        profile.ensure_layout()?;
        let extra = match fsutil::read_json::<AppState>(&self.root.state_file()) {
            JsonFile::Valid(state) => state.extra,
            JsonFile::Missing | JsonFile::Unusable(_) | JsonFile::Unreadable(_) => Map::new(),
        };
        self.write_state(id, extra)?;
        Ok(profile)
    }

    /// The id `state.json` names right now, read as it is: nothing is repaired and nothing falls
    /// back to Local. A profile switch uses it to tell whether its choice was saved when the write
    /// itself reported an error.
    pub fn saved_active(&self) -> Option<String> {
        match fsutil::read_json::<AppState>(&self.root.state_file()) {
            JsonFile::Valid(state) => Some(state.active_profile),
            JsonFile::Missing | JsonFile::Unusable(_) | JsonFile::Unreadable(_) => None,
        }
    }

    /// The Local profile, created or repaired as needed. A corrupt `profile.json` is moved aside and
    /// rewritten; nothing else in the directory (the database above all) is touched. A file where the
    /// profile folder belongs is moved aside first: every read below would fail with "not a
    /// directory", and so would every launch.
    fn ensure_local(&self) -> Result<Profile> {
        fsutil::ensure_private_dir(&self.root.profile_dir(LOCAL_PROFILE_ID)?)?;
        match self.read_profile(LOCAL_PROFILE_ID)? {
            ProfileRead::Valid(profile) => {
                profile.ensure_layout()?;
                Ok(profile)
            }
            ProfileRead::Missing => self.write_new_profile(
                LOCAL_PROFILE_ID.to_string(),
                DEFAULT_DISPLAY_NAME.to_string(),
            ),
            // An account's profile never had the id `local`, so a `local` whose file says
            // `account` was edited by hand. Like any other unusable file it is moved aside, never
            // deleted, and nothing else in the folder is touched.
            ProfileRead::RetiredAccount => self.recreate_local_meta(
                "the Local profile.json names a Ghost account, which this version no longer opens",
            ),
            ProfileRead::Invalid(error) => self.recreate_local_meta(&error.to_string()),
        }
    }

    /// Moves the unusable `profiles/local/profile.json` aside and writes a new one.
    fn recreate_local_meta(&self, reason: &str) -> Result<Profile> {
        let meta_path = self.root.profile_dir(LOCAL_PROFILE_ID)?.join(PROFILE_FILE);
        let backup = fsutil::quarantine_file(&meta_path)?;
        tracing::warn!(reason, backup = %backup.display(), "the Local profile.json was unusable; recreated it");
        self.write_new_profile(
            LOCAL_PROFILE_ID.to_string(),
            DEFAULT_DISPLAY_NAME.to_string(),
        )
    }

    /// Creates the directory and subdirectories, then writes `profile.json` last, so a crash midway
    /// leaves a folder `list()` skips rather than a half-made profile it shows.
    fn write_new_profile(&self, id: String, display_name: String) -> Result<Profile> {
        let dir = self.root.profile_dir(&id)?;
        let profile = Profile {
            meta: ProfileMeta {
                id,
                kind: ProfileKind::Local,
                display_name,
                created_at: now_millis(),
            },
            dir,
        };
        profile.ensure_layout()?;
        fsutil::write_json_atomic(&profile.meta_path(), &profile.meta)?;
        tracing::info!(profile = %profile.meta.id, kind = ?profile.meta.kind, "created profile");
        Ok(profile)
    }

    fn write_state(&self, active_profile: &str, extra: Map<String, Value>) -> Result<()> {
        let state = AppState {
            active_profile: active_profile.to_string(),
            extra,
        };
        fsutil::write_json_atomic(&self.root.state_file(), &state)
    }

    /// Reads `profiles/<id>/profile.json`. An invalid id is an `Err`, so callers handling user input
    /// get `InvalidProfileId` before any path is built. So is a profile folder linked to one that
    /// can't be found (`BrokenLink`), which is unavailable, not missing.
    fn read_profile(&self, id: &str) -> Result<ProfileRead> {
        let dir = self.root.profile_dir(id)?;
        fsutil::check_folder_link(&dir)?;
        let meta_path = dir.join(PROFILE_FILE);
        Ok(match fsutil::read_json::<StoredMeta>(&meta_path) {
            JsonFile::Missing => ProfileRead::Missing,
            JsonFile::Unusable(error) | JsonFile::Unreadable(error) => ProfileRead::Invalid(error),
            JsonFile::Valid(meta) if meta.id != id => {
                ProfileRead::Invalid(id_mismatch(&meta_path, &meta.id, id))
            }
            JsonFile::Valid(meta) => match meta.kind {
                StoredKind::Account => ProfileRead::RetiredAccount,
                StoredKind::Local => ProfileRead::Valid(Profile {
                    meta: ProfileMeta {
                        id: meta.id,
                        kind: ProfileKind::Local,
                        display_name: meta.display_name,
                        created_at: meta.created_at,
                    },
                    dir,
                }),
            },
        })
    }

    /// [`Profiles::read_profile`] for ids from files, where an invalid id is just another broken
    /// reference to fall back from.
    fn read_profile_or_invalid(&self, id: &str) -> ProfileRead {
        self.read_profile(id).unwrap_or_else(ProfileRead::Invalid)
    }

    fn profiles_dir_is_empty(&self) -> bool {
        fs::read_dir(self.root.profiles_dir())
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true)
    }
}

fn list_order(meta: &ProfileMeta) -> (bool, i64, &str) {
    (
        meta.id != LOCAL_PROFILE_ID,
        meta.created_at,
        meta.id.as_str(),
    )
}

/// A `profile.json` copied into another profile's folder would make two folders claim one id.
fn id_mismatch(meta_path: &Path, found: &str, folder: &str) -> CoreError {
    CoreError::Json {
        path: meta_path.to_path_buf(),
        source: serde_json::Error::custom(format!(
            "its id {found:?} does not match its folder {folder:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::error::LinkProblem;
    use crate::paths::validate_profile_id;

    fn setup() -> (tempfile::TempDir, Profiles) {
        let dir = tempfile::tempdir().expect("tempdir");
        let profiles = Profiles::new(DataRoot::at(dir.path().join("aura")));
        (dir, profiles)
    }

    fn state_json(profiles: &Profiles) -> Value {
        let bytes = fs::read(profiles.root().state_file()).expect("state.json");
        serde_json::from_slice(&bytes).expect("state.json is JSON")
    }

    fn write_state_raw(profiles: &Profiles, body: &str) {
        profiles.root().ensure().expect("ensure");
        fs::write(profiles.root().state_file(), body).expect("write state");
    }

    #[test]
    fn first_open_creates_local_and_state() {
        let (_dir, profiles) = setup();
        let profile = profiles.open_active().expect("open");
        assert_eq!(profile.meta.id, LOCAL_PROFILE_ID);
        assert_eq!(profile.meta.kind, ProfileKind::Local);
        assert_eq!(profile.meta.display_name, "Local");
        assert_eq!(profile.dir, profiles.root().profiles_dir().join("local"));
        for dir in [
            profile.attachments_dir(),
            profile.checkpoints_dir(),
            profile.logs_dir(),
            profile.backups_dir(),
        ] {
            assert!(dir.is_dir(), "{}", dir.display());
        }
        assert_eq!(profile.db_path(), profile.dir.join("aura.db"));
        let meta: ProfileMeta =
            serde_json::from_slice(&fs::read(profile.meta_path()).expect("profile.json"))
                .expect("meta");
        assert_eq!(meta, profile.meta);
        assert_eq!(state_json(&profiles)["active_profile"], "local");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&profile.dir)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
        // Opening again changes nothing.
        assert_eq!(profiles.open_active().expect("reopen"), profile);
    }

    #[test]
    fn json_formats_are_snake_case() {
        let meta = ProfileMeta {
            id: "local".into(),
            kind: ProfileKind::Local,
            display_name: "A".into(),
            created_at: 1,
        };
        assert_eq!(
            serde_json::to_value(&meta).expect("serialize"),
            serde_json::json!({"id": "local", "kind": "local", "display_name": "A", "created_at": 1})
        );
    }

    #[test]
    fn state_naming_a_missing_profile_falls_back_to_local_and_is_rewritten() {
        let (_dir, profiles) = setup();
        let ghost = new_id();
        write_state_raw(&profiles, &format!(r#"{{"active_profile": "{ghost}"}}"#));
        let profile = profiles.open_active().expect("open");
        assert_eq!(profile.meta.id, LOCAL_PROFILE_ID);
        assert_eq!(state_json(&profiles)["active_profile"], "local");
    }

    #[test]
    fn state_naming_an_invalid_id_falls_back_to_local() {
        let (_dir, profiles) = setup();
        write_state_raw(&profiles, r#"{"active_profile": "../../etc"}"#);
        let profile = profiles.open_active().expect("open");
        assert_eq!(profile.meta.id, LOCAL_PROFILE_ID);
        assert_eq!(state_json(&profiles)["active_profile"], "local");
    }

    #[test]
    fn state_naming_a_corrupt_profile_falls_back_to_local_without_touching_it() {
        let (_dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let other = profiles.create("me@example.com").expect("create");
        profiles.set_active(&other.meta.id).expect("activate");
        fs::write(other.meta_path(), b"{ broken").expect("corrupt");
        let profile = profiles.open_active().expect("open");
        assert_eq!(profile.meta.id, LOCAL_PROFILE_ID);
        assert_eq!(
            fs::read(other.meta_path()).expect("left alone"),
            b"{ broken"
        );
    }

    #[test]
    fn corrupt_state_falls_back_to_local_and_is_kept_aside() {
        let (_dir, profiles) = setup();
        write_state_raw(&profiles, "not json at all");
        let profile = profiles.open_active().expect("open");
        assert_eq!(profile.meta.id, LOCAL_PROFILE_ID);
        assert_eq!(state_json(&profiles)["active_profile"], "local");
        let kept: Vec<PathBuf> = fs::read_dir(profiles.root().root())
            .expect("read_dir")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| fsutil::file_name_lossy(path).starts_with("state.json.corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read(&kept[0]).expect("bytes"), b"not json at all");
    }

    #[test]
    fn missing_state_with_existing_profiles_reopens_local() {
        let (_dir, profiles) = setup();
        let first = profiles.open_active().expect("first");
        fs::remove_file(profiles.root().state_file()).expect("delete state");
        assert_eq!(profiles.open_active().expect("open"), first);
        assert_eq!(state_json(&profiles)["active_profile"], "local");
    }

    #[test]
    fn a_corrupt_local_profile_json_is_recreated_without_touching_the_data() {
        let (_dir, profiles) = setup();
        let local = profiles.open_active().expect("first");
        fs::write(local.db_path(), b"pretend database").expect("db");
        fs::write(local.meta_path(), b"{{{{").expect("corrupt");
        let repaired = profiles.open_active().expect("open");
        assert_eq!(repaired.meta.id, LOCAL_PROFILE_ID);
        assert_eq!(
            fs::read(repaired.db_path()).expect("db kept"),
            b"pretend database"
        );
        let kept = fs::read_dir(&local.dir)
            .expect("read_dir")
            .map(|entry| entry.expect("entry").path())
            .any(|path| fsutil::file_name_lossy(&path).starts_with("profile.json.corrupt-"));
        assert!(kept, "the corrupt profile.json is kept aside");
    }

    fn corrupt_copies(dir: &Path, prefix: &str) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .expect("read_dir")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| fsutil::file_name_lossy(path).starts_with(prefix))
            .collect()
    }

    #[test]
    fn a_file_where_the_local_profile_folder_should_be_is_moved_aside() {
        for with_state in [false, true] {
            let (_dir, profiles) = setup();
            profiles.root().ensure().expect("ensure");
            if with_state {
                write_state_raw(&profiles, r#"{"active_profile": "local"}"#);
            }
            let local_dir = profiles.root().profiles_dir().join(LOCAL_PROFILE_ID);
            fs::write(&local_dir, b"not a folder").expect("write");

            let profile = profiles.open_active().expect("open");
            assert_eq!(profile.meta.id, LOCAL_PROFILE_ID);
            assert!(profile.dir.is_dir());
            assert!(profile.logs_dir().is_dir());
            let kept = corrupt_copies(&profiles.root().profiles_dir(), "local.corrupt-");
            assert_eq!(kept.len(), 1, "with_state {with_state}: {kept:?}");
            assert_eq!(fs::read(&kept[0]).expect("kept"), b"not a folder");
            // The moved file is not mistaken for a profile.
            let ids: Vec<String> = profiles
                .list()
                .expect("list")
                .into_iter()
                .map(|meta| meta.id)
                .collect();
            assert_eq!(ids, vec![LOCAL_PROFILE_ID.to_string()]);
        }
    }

    #[test]
    fn files_where_profile_subfolders_should_be_are_moved_aside() {
        let (_dir, profiles) = setup();
        let local = profiles.open_active().expect("first");
        fs::write(local.db_path(), b"the database").expect("db");
        let subdirs = [
            local.attachments_dir(),
            local.checkpoints_dir(),
            local.logs_dir(),
            local.backups_dir(),
        ];
        for dir in &subdirs {
            fs::remove_dir_all(dir).expect("remove");
            fs::write(dir, b"stray").expect("write a file in its place");
        }
        let reopened = profiles.open_active().expect("open");
        assert_eq!(reopened, local);
        for dir in &subdirs {
            assert!(dir.is_dir(), "{}", dir.display());
            let name = format!("{}.corrupt-", fsutil::file_name_lossy(dir));
            let kept = corrupt_copies(&local.dir, &name);
            assert_eq!(kept.len(), 1, "{name}: {kept:?}");
            assert_eq!(fs::read(&kept[0]).expect("kept"), b"stray");
        }
        assert_eq!(
            fs::read(local.db_path()).expect("db untouched"),
            b"the database"
        );
    }

    #[cfg(unix)]
    fn is_link(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    }

    /// Finding: with `profiles/` linked to a folder on a disk that wasn't connected, the link was
    /// moved aside and a new, empty Local profile created in its place, which the app kept using
    /// once the disk was back, with the history stranded behind `profiles.corrupt-<ms>`.
    #[cfg(unix)]
    #[test]
    fn an_unplugged_profiles_link_is_an_error_and_starts_no_new_profile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let disk = dir.path().join("external disk");
        let elsewhere = disk.join("aura profiles");
        fs::create_dir_all(&elsewhere).expect("mkdir");
        let root = DataRoot::at(dir.path().join("aura"));
        fs::create_dir_all(root.root()).expect("mkdir");
        std::os::unix::fs::symlink(&elsewhere, root.profiles_dir()).expect("symlink");
        let profiles = Profiles::new(root.clone());
        let local = profiles.open_active().expect("open through the link");
        fs::write(local.db_path(), b"the history").expect("data on the other disk");

        let unplugged = dir.path().join("unplugged");
        fs::rename(&disk, &unplugged).expect("unplug the disk");
        let error = profiles
            .open_active()
            .expect_err("the profiles are unavailable");
        assert!(
            matches!(
                &error,
                CoreError::BrokenLink { link, target, problem: LinkProblem::Missing }
                    if *link == root.profiles_dir() && *target == elsewhere
            ),
            "{error:?}"
        );
        assert!(is_link(&root.profiles_dir()), "the link is left in place");
        assert!(!disk.exists(), "nothing was created where the disk was");
        assert!(corrupt_copies(root.root(), "profiles.corrupt-").is_empty());
        assert!(matches!(
            profiles.create(""),
            Err(CoreError::BrokenLink { .. })
        ));

        fs::rename(&unplugged, &disk).expect("plug it back in");
        let again = profiles.open_active().expect("open");
        assert_eq!(again, local);
        assert_eq!(fs::read(again.db_path()).expect("db"), b"the history");
    }

    /// The active profile's own folder on that disk: an error too, and `state.json` keeps naming
    /// it, where falling back to Local would have made Local the active profile for good.
    #[cfg(unix)]
    #[test]
    fn an_unplugged_profile_folder_link_is_an_error_and_stays_the_active_profile() {
        let (dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let other = profiles.create("work").expect("create");
        let disk = dir.path().join("external disk");
        fs::create_dir_all(&disk).expect("mkdir");
        let moved = disk.join(&other.meta.id);
        fs::rename(&other.dir, &moved).expect("move the profile to the other disk");
        std::os::unix::fs::symlink(&moved, &other.dir).expect("symlink");
        assert_eq!(
            profiles.set_active(&other.meta.id).expect("activate"),
            other
        );
        let state = fs::read(profiles.root().state_file()).expect("state.json");

        let unplugged = dir.path().join("unplugged");
        fs::rename(&disk, &unplugged).expect("unplug the disk");
        assert!(matches!(
            profiles.open_active(),
            Err(CoreError::BrokenLink { ref link, .. }) if *link == other.dir
        ));
        assert!(matches!(
            profiles.get(&other.meta.id),
            Err(CoreError::BrokenLink { .. })
        ));
        assert_eq!(
            fs::read(profiles.root().state_file()).expect("state.json"),
            state,
            "still the active profile"
        );
        assert!(is_link(&other.dir));

        fs::rename(&unplugged, &disk).expect("plug it back in");
        assert_eq!(profiles.open_active().expect("open"), other);
    }

    #[cfg(unix)]
    #[test]
    fn an_unplugged_subfolder_link_is_an_error_and_nothing_takes_its_place() {
        let (dir, profiles) = setup();
        let local = profiles.open_active().expect("bootstrap");
        let disk = dir.path().join("external disk");
        let elsewhere = disk.join("checkpoints");
        fs::create_dir_all(&elsewhere).expect("mkdir");
        fs::remove_dir(local.checkpoints_dir()).expect("rmdir");
        std::os::unix::fs::symlink(&elsewhere, local.checkpoints_dir()).expect("symlink");
        assert_eq!(profiles.open_active().expect("open"), local);

        let unplugged = dir.path().join("unplugged");
        fs::rename(&disk, &unplugged).expect("unplug the disk");
        assert!(matches!(
            profiles.open_active(),
            Err(CoreError::BrokenLink { ref link, .. }) if *link == local.checkpoints_dir()
        ));
        assert!(is_link(&local.checkpoints_dir()));
        assert!(corrupt_copies(&local.dir, "checkpoints.corrupt-").is_empty());
        assert!(!disk.exists());
    }

    #[test]
    fn a_file_in_place_of_a_subfolder_is_moved_aside_when_activating_a_profile() {
        let (_dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let other = profiles.create("work").expect("create");
        fs::remove_dir_all(other.attachments_dir()).expect("remove");
        fs::write(other.attachments_dir(), b"stray").expect("write");
        let activated = profiles.set_active(&other.meta.id).expect("activate");
        assert!(activated.attachments_dir().is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_state_json_falls_back_to_local_and_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let other = profiles.create("work").expect("create");
        profiles.set_active(&other.meta.id).expect("activate");
        let state = profiles.root().state_file();
        let before = fs::read(&state).expect("state.json");
        fs::set_permissions(&state, fs::Permissions::from_mode(0o000)).expect("chmod");
        if fs::read(&state).is_ok() {
            eprintln!("skipping: file permissions are not enforced for this user");
            return;
        }
        let opened = profiles.open_active();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o600)).expect("chmod back");
        assert_eq!(opened.expect("open").meta.id, LOCAL_PROFILE_ID);
        assert_eq!(fs::read(&state).expect("kept"), before, "left as it was");
        assert!(corrupt_copies(profiles.root().root(), "state.json.corrupt-").is_empty());
        // Readable again: the user's choice is back.
        assert_eq!(profiles.open_active().expect("open"), other);
    }

    #[test]
    fn missing_subdirectories_are_recreated_on_open() {
        let (_dir, profiles) = setup();
        let local = profiles.open_active().expect("first");
        fs::remove_dir_all(local.logs_dir()).expect("remove logs");
        let reopened = profiles.open_active().expect("open");
        assert!(reopened.logs_dir().is_dir());
    }

    #[test]
    fn created_profiles_are_listed() {
        let (_dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let other = profiles.create("  Side project  ").expect("create");
        assert_eq!(other.meta.kind, ProfileKind::Local);
        assert_eq!(other.meta.display_name, "Side project");
        assert!(validate_profile_id(&other.meta.id).is_ok());
        assert_eq!(
            uuid::Uuid::parse_str(&other.meta.id)
                .expect("uuid")
                .get_version_num(),
            7
        );
        assert!(other.backups_dir().is_dir());

        let listed = profiles.list().expect("list");
        let ids: Vec<&str> = listed.iter().map(|meta| meta.id.as_str()).collect();
        assert_eq!(ids, vec![LOCAL_PROFILE_ID, other.meta.id.as_str()]);
        assert_eq!(listed[1], other.meta);
    }

    #[test]
    fn create_local_uses_local_once_then_uuids() {
        let (_dir, profiles) = setup();
        let first = profiles.create("").expect("first");
        assert_eq!(first.meta.id, LOCAL_PROFILE_ID);
        assert_eq!(first.meta.display_name, "Local");
        let second = profiles.create("Kept workspace").expect("second");
        assert_ne!(second.meta.id, LOCAL_PROFILE_ID);
        assert_eq!(second.meta.kind, ProfileKind::Local);
        assert_eq!(second.meta.display_name, "Kept workspace");
        let blank = profiles.create("   ").expect("blank");
        assert_eq!(blank.meta.display_name, "Local");
    }

    #[test]
    fn set_active_persists_across_instances() {
        let (dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let other = profiles.create("work").expect("create");
        let activated = profiles.set_active(&other.meta.id).expect("activate");
        assert_eq!(activated, other);

        let fresh = Profiles::new(DataRoot::at(dir.path().join("aura")));
        assert_eq!(fresh.open_active().expect("open"), other);
    }

    #[test]
    fn saved_active_reads_the_file_without_repairing_it() {
        let (_dir, profiles) = setup();
        assert_eq!(profiles.saved_active(), None, "no state.json yet");
        profiles.open_active().expect("bootstrap");
        assert_eq!(profiles.saved_active().as_deref(), Some(LOCAL_PROFILE_ID));
        let ghost = new_id();
        let body = format!(r#"{{"active_profile": "{ghost}"}}"#);
        write_state_raw(&profiles, &body);
        assert_eq!(
            profiles.saved_active(),
            Some(ghost),
            "as written, not Local"
        );
        assert_eq!(
            fs::read(profiles.root().state_file()).expect("state"),
            body.as_bytes()
        );
        write_state_raw(&profiles, "not json");
        assert_eq!(profiles.saved_active(), None);
        assert_eq!(
            fs::read(profiles.root().state_file()).expect("state"),
            b"not json",
            "left alone"
        );
    }

    #[test]
    fn set_active_keeps_unknown_state_fields() {
        let (_dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let other = profiles.create("work").expect("create");
        write_state_raw(
            &profiles,
            r#"{"active_profile": "local", "window": {"w": 900}}"#,
        );
        profiles.set_active(&other.meta.id).expect("activate");
        let state = state_json(&profiles);
        assert_eq!(state["active_profile"], other.meta.id.as_str());
        assert_eq!(state["window"]["w"], 900);
    }

    #[test]
    fn set_active_rejects_traversal() {
        let (_dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let before = fs::read(profiles.root().state_file()).expect("state");
        assert!(
            matches!(profiles.set_active("../evil"), Err(CoreError::InvalidProfileId(ref id)) if id == "../evil")
        );
        assert_eq!(
            fs::read(profiles.root().state_file()).expect("state"),
            before
        );
    }

    #[test]
    fn set_active_on_a_missing_profile_is_not_found() {
        let (_dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let ghost = new_id();
        assert!(
            matches!(profiles.set_active(&ghost), Err(CoreError::ProfileNotFound(ref id)) if *id == ghost)
        );
        assert_eq!(state_json(&profiles)["active_profile"], "local");
    }

    #[test]
    fn list_skips_unusable_profile_dirs() {
        let (_dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let good = profiles.create("good").expect("good");
        let bad = profiles.create("bad").expect("bad");
        fs::write(bad.meta_path(), b"\x00\x01 not json").expect("corrupt");

        let profiles_dir = profiles.root().profiles_dir();
        fs::create_dir(profiles_dir.join("Not-Valid")).expect("bad name");
        fs::create_dir(profiles_dir.join("emptyprofile")).expect("no profile.json");
        fs::write(profiles_dir.join(".DS_Store"), b"x").expect("stray file");
        // A profile.json copied into the wrong folder.
        let copied = profiles_dir.join("copied");
        fs::create_dir(&copied).expect("mkdir");
        fs::copy(good.meta_path(), copied.join("profile.json")).expect("copy");

        let ids: Vec<String> = profiles
            .list()
            .expect("list")
            .into_iter()
            .map(|meta| meta.id)
            .collect();
        assert_eq!(ids, vec![LOCAL_PROFILE_ID.to_string(), good.meta.id]);
    }

    #[test]
    fn list_orders_local_first_then_the_others_oldest_first() {
        let (_dir, profiles) = setup();
        // Fast enough that creation times collide: the ids (UUIDv7, so in creation order) decide.
        let local = profiles.create("").expect("local");
        let second = profiles.create("kept").expect("second");
        let third = profiles.create("later").expect("third");
        assert_eq!(
            local.meta.id, LOCAL_PROFILE_ID,
            "the first profile takes the id `local`"
        );
        // `local` leads even when it is the newest by its own timestamp.
        let mut meta = local.meta.clone();
        meta.created_at = third.meta.created_at + 1_000;
        fsutil::write_json_atomic(&local.meta_path(), &meta).expect("rewrite");
        let ids: Vec<String> = profiles
            .list()
            .expect("list")
            .into_iter()
            .map(|meta| meta.id)
            .collect();
        assert_eq!(ids, vec![local.meta.id, second.meta.id, third.meta.id]);
    }

    /// What a Ghost account left in a data folder: its own profile folder, with an encrypted
    /// database and the files beside it. Returns the folder and every file's bytes.
    fn account_leftover(profiles: &Profiles) -> (PathBuf, Vec<(PathBuf, Vec<u8>)>) {
        let id = new_id();
        let dir = profiles.root().profile_dir(&id).expect("dir");
        fs::create_dir_all(dir.join("attachments")).expect("mkdir");
        let files = [
            (
                "profile.json",
                format!(
                    r#"{{"id":"{id}","kind":"account","display_name":"me@example.com","created_at":1}}"#
                )
                .into_bytes(),
            ),
            ("aura.encrypted.db", b"ciphertext, not SQLite".to_vec()),
            ("aura.encrypted.db.encryption", b"{\"version\":1}".to_vec()),
            ("attachments/blob", b"encrypted attachment".to_vec()),
        ];
        let written = files
            .into_iter()
            .map(|(name, bytes)| {
                let path = dir.join(name);
                fs::write(&path, &bytes).expect("write");
                (path, bytes)
            })
            .collect();
        (dir, written)
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read_dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    /// Ghost accounts were removed. The folder one left behind is not listed, cannot be
    /// selected, and stays on disk byte for byte; when it was the last-used profile, Local opens
    /// (and is created if the account was the only profile the data folder had).
    #[test]
    fn a_leftover_account_profile_is_skipped_and_left_untouched_and_local_opens() {
        let (_dir, profiles) = setup();
        profiles.root().ensure().expect("ensure");
        let (account_dir, files) = account_leftover(&profiles);
        let id = account_dir
            .file_name()
            .and_then(|name| name.to_str())
            .expect("id")
            .to_owned();
        let before = entries(&account_dir);
        // The last-used profile was the account's, and no Local profile exists yet.
        write_state_raw(
            &profiles,
            &format!(r#"{{"active_profile": "{id}", "window": {{"w": 900}}}}"#),
        );

        let opened = profiles.open_active().expect("the app still opens");
        assert_eq!(
            opened.meta.id, LOCAL_PROFILE_ID,
            "Local is created and opened"
        );
        assert_eq!(opened.meta.kind, ProfileKind::Local);
        let state = state_json(&profiles);
        assert_eq!(state["active_profile"], "local");
        assert_eq!(state["window"]["w"], 900, "other saved state is kept");

        let listed: Vec<String> = profiles
            .list()
            .expect("list")
            .into_iter()
            .map(|meta| meta.id)
            .collect();
        assert_eq!(
            listed,
            vec![LOCAL_PROFILE_ID.to_string()],
            "only Local profiles"
        );
        assert!(matches!(
            profiles.get(&id),
            Err(CoreError::ProfileNotFound(ref missing)) if *missing == id
        ));
        assert!(matches!(
            profiles.set_active(&id),
            Err(CoreError::ProfileNotFound(_))
        ));
        assert_eq!(state_json(&profiles)["active_profile"], "local");

        // Opening again (a second launch) changes nothing either.
        assert_eq!(profiles.open_active().expect("reopen"), opened);
        assert_eq!(
            entries(&account_dir),
            before,
            "nothing added, moved or removed"
        );
        for (path, bytes) in &files {
            assert_eq!(
                &fs::read(path).expect("still there"),
                bytes,
                "{}",
                path.display()
            );
        }
        assert!(
            corrupt_copies(&account_dir, "profile.json.corrupt-").is_empty(),
            "its profile.json is not treated as damage"
        );
    }

    /// With a Local profile already there, the leftover folder changes nothing about it.
    #[test]
    fn a_leftover_account_profile_beside_local_profiles_changes_nothing_for_them() {
        let (_dir, profiles) = setup();
        let local = profiles.open_active().expect("bootstrap");
        fs::write(local.db_path(), b"the history").expect("db");
        let kept = profiles.create("Kept workspace").expect("second local");
        let (account_dir, files) = account_leftover(&profiles);
        let id = account_dir
            .file_name()
            .and_then(|name| name.to_str())
            .expect("id")
            .to_owned();
        write_state_raw(&profiles, &format!(r#"{{"active_profile": "{id}"}}"#));

        assert_eq!(profiles.open_active().expect("open"), local);
        assert_eq!(fs::read(local.db_path()).expect("db"), b"the history");
        let listed: Vec<String> = profiles
            .list()
            .expect("list")
            .into_iter()
            .map(|meta| meta.id)
            .collect();
        assert_eq!(listed, vec![local.meta.id.clone(), kept.meta.id.clone()]);
        for (path, bytes) in &files {
            assert_eq!(
                &fs::read(path).expect("still there"),
                bytes,
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn list_of_a_fresh_root_is_empty() {
        let (_dir, profiles) = setup();
        assert!(profiles.list().expect("list").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn list_skips_an_unreadable_profile_dir() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let locked = profiles.create("locked").expect("create");
        fs::set_permissions(&locked.dir, fs::Permissions::from_mode(0o000)).expect("chmod");
        let enforced = fs::read(locked.meta_path()).is_err();
        let listed = profiles.list();
        // Restore before asserting so the temp dir can always be cleaned up.
        fs::set_permissions(&locked.dir, fs::Permissions::from_mode(0o700)).expect("chmod back");
        if !enforced {
            eprintln!("skipping: directory permissions are not enforced for this user");
            return;
        }
        let ids: Vec<String> = listed
            .expect("list")
            .into_iter()
            .map(|meta| meta.id)
            .collect();
        assert_eq!(ids, vec![LOCAL_PROFILE_ID.to_string()]);
    }

    #[test]
    fn get_distinguishes_missing_invalid_and_corrupt() {
        let (_dir, profiles) = setup();
        let local = profiles.open_active().expect("bootstrap");
        assert_eq!(profiles.get(LOCAL_PROFILE_ID).expect("get"), local);
        assert!(matches!(
            profiles.get("nope"),
            Err(CoreError::ProfileNotFound(_))
        ));
        assert!(matches!(
            profiles.get("NOPE"),
            Err(CoreError::InvalidProfileId(_))
        ));
        fs::write(local.meta_path(), b"[]").expect("corrupt");
        assert!(matches!(
            profiles.get(LOCAL_PROFILE_ID),
            Err(CoreError::Json { .. })
        ));
    }

    #[test]
    fn state_json_with_a_bom_is_accepted() {
        let (_dir, profiles) = setup();
        profiles.open_active().expect("bootstrap");
        let other = profiles.create("work").expect("create");
        write_state_raw(
            &profiles,
            &format!("\u{feff}{{\"active_profile\": \"{}\"}}", other.meta.id),
        );
        assert_eq!(profiles.open_active().expect("open"), other);
    }
}
