//! This machine's identity.
//!
//! Rows that depend on this machine (a folder's location, a queued prompt) record the `device_id`
//! that wrote them. The identity lives in `<data root>/device.json`, outside any profile, so all
//! profiles on a machine agree on which device they are.

use std::ffi::OsStr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::fsutil::{self, JsonFile};
use crate::ids::new_id;
use crate::paths::DataRoot;
use crate::time::now_millis;

/// Shown when the OS gives no usable host name.
pub const FALLBACK_DEVICE_NAME: &str = "This device";

/// Device names are display labels (device list, "Continue on ..."); longer host names get cut.
pub const MAX_DEVICE_NAME_CHARS: usize = 64;

/// Who this machine is, as stored in `device.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct DeviceIdentity {
    /// Lowercase hyphenated UUIDv7, generated once per machine.
    pub device_id: String,
    pub name: String,
    /// `"macos"`, `"windows"`, `"linux"`, or Rust's `std::env::consts::OS` elsewhere.
    pub platform: String,
    /// Unix ms. Exported to TypeScript as `number` (specta refuses `i64` by default); exact until
    /// 2^53 ms, hundreds of millennia away.
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
}

/// How [`DeviceIdentity::load_or_create`] got the identity, so the shell can log or surface it.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceLoad {
    /// `device.json` was present and valid.
    Existing(DeviceIdentity),
    /// First run on this machine (or this data root).
    Created(DeviceIdentity),
    /// `device.json` was unusable; it was moved to `backup` and a new identity was created. Rows this
    /// machine wrote earlier keep the old id, which is why the old file is kept rather than deleted.
    Recreated {
        identity: DeviceIdentity,
        backup: PathBuf,
    },
}

impl DeviceLoad {
    pub fn identity(&self) -> &DeviceIdentity {
        match self {
            DeviceLoad::Existing(identity)
            | DeviceLoad::Created(identity)
            | DeviceLoad::Recreated { identity, .. } => identity,
        }
    }
}

impl DeviceIdentity {
    /// Reads `device.json`, creating it on first run.
    ///
    /// When the file was read but can't be used (not JSON, missing fields, an id that isn't a
    /// canonical UUID), or something other than a file sits at that path, it is renamed to
    /// `device.json.corrupt-<millis>` next to it and replaced by a fresh identity: the app must always
    /// start, and the old bytes stay available.
    ///
    /// A file that could not be *read* (permission denied, an I/O error, too many open files, a
    /// Windows sharing violation) is an error instead, and stays where it is. Its contents may be
    /// perfectly good, and every folder location is tied to this id, so replacing it over a
    /// passing fault would change the machine's identity for good.
    pub fn load_or_create(root: &DataRoot) -> Result<DeviceLoad> {
        root.ensure()?;
        let path = root.device_file();
        let problem = match fsutil::read_json::<DeviceIdentity>(&path) {
            JsonFile::Valid(identity) if is_canonical_uuid(&identity.device_id) => {
                return Ok(DeviceLoad::Existing(identity));
            }
            JsonFile::Valid(identity) => {
                format!("device_id {:?} is not a canonical UUID", identity.device_id)
            }
            JsonFile::Missing => {
                let identity = DeviceIdentity::generate();
                fsutil::write_json_atomic(&path, &identity)?;
                tracing::info!(device_id = %identity.device_id, "created device identity");
                return Ok(DeviceLoad::Created(identity));
            }
            JsonFile::Unusable(error) => error.to_string(),
            JsonFile::Unreadable(error) => {
                tracing::error!(%error, "device.json could not be read; keeping it as it is");
                return Err(error);
            }
        };

        let backup = fsutil::quarantine_file(&path)?;
        let identity = DeviceIdentity::generate();
        fsutil::write_json_atomic(&path, &identity)?;
        tracing::warn!(
            %problem,
            backup = %backup.display(),
            device_id = %identity.device_id,
            "device.json was unusable; moved it aside and created a new device identity"
        );
        Ok(DeviceLoad::Recreated { identity, backup })
    }

    fn generate() -> DeviceIdentity {
        DeviceIdentity {
            device_id: new_id(),
            name: device_name_from(hostname::get().ok().as_deref()),
            platform: current_platform().to_string(),
            created_at: now_millis(),
        }
    }
}

/// The platform string stored with the identity. Rust's own OS names are already `"macos"`,
/// `"windows"`, and `"linux"`, so whatever shows the device can switch on them without a
/// translation table, and any other OS still gets a stable, meaningful value.
pub fn current_platform() -> &'static str {
    std::env::consts::OS
}

/// Turns the raw host name into a display name: control characters dropped, whitespace trimmed, the
/// mDNS `.local` suffix macOS adds removed, and the result capped at [`MAX_DEVICE_NAME_CHARS`]
/// characters. Falls back to [`FALLBACK_DEVICE_NAME`] when nothing usable is left or the OS gave no
/// name at all.
fn device_name_from(raw: Option<&OsStr>) -> String {
    let Some(raw) = raw else {
        return FALLBACK_DEVICE_NAME.to_string();
    };
    let printable: String = raw
        .to_string_lossy()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let mut name = printable.trim();
    if let Some(cut) = name.len().checked_sub(".local".len())
        && let Some(suffix) = name.get(cut..)
        && suffix.eq_ignore_ascii_case(".local")
    {
        name = name[..cut].trim_end();
    }
    let capped: String = name.chars().take(MAX_DEVICE_NAME_CHARS).collect();
    let capped = capped.trim_end();
    if capped.is_empty() {
        FALLBACK_DEVICE_NAME.to_string()
    } else {
        capped.to_string()
    }
}

/// Every row this machine writes carries the id, so anything but the exact form Ghost generates
/// (lowercase, hyphenated) is treated as a damaged file rather than copied into the database.
fn is_canonical_uuid(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.hyphenated().to_string() == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn root() -> (tempfile::TempDir, DataRoot) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("aura"));
        (dir, root)
    }

    #[test]
    fn first_run_creates_device_json() {
        let (_dir, root) = root();
        let load = DeviceIdentity::load_or_create(&root).expect("load");
        let DeviceLoad::Created(identity) = &load else {
            panic!("expected Created, got {load:?}");
        };
        assert!(root.device_file().is_file());
        assert_eq!(
            uuid::Uuid::parse_str(&identity.device_id)
                .expect("uuid")
                .get_version_num(),
            7
        );
        assert!(is_canonical_uuid(&identity.device_id));
        assert_eq!(identity.platform, std::env::consts::OS);
        assert!(!identity.name.is_empty());
        assert!(identity.name.chars().count() <= MAX_DEVICE_NAME_CHARS);
        assert!(identity.created_at > 0);
        assert_eq!(load.identity(), identity);
    }

    #[test]
    fn second_run_returns_the_same_identity() {
        let (_dir, root) = root();
        let first = DeviceIdentity::load_or_create(&root)
            .expect("first")
            .identity()
            .clone();
        let second = DeviceIdentity::load_or_create(&root).expect("second");
        assert_eq!(second, DeviceLoad::Existing(first));
    }

    #[test]
    fn corrupt_json_is_moved_aside_and_recreated() {
        let (_dir, root) = root();
        let old = DeviceIdentity::load_or_create(&root)
            .expect("first")
            .identity()
            .clone();
        let garbage = b"{\"device_id\": \"truncated";
        fs::write(root.device_file(), garbage).expect("corrupt");

        let load = DeviceIdentity::load_or_create(&root).expect("recover");
        let DeviceLoad::Recreated { identity, backup } = &load else {
            panic!("expected Recreated, got {load:?}");
        };
        assert_ne!(identity.device_id, old.device_id);
        assert!(backup.is_file());
        assert_eq!(backup.parent(), Some(root.root()));
        assert!(fsutil::file_name_lossy(backup).starts_with("device.json.corrupt-"));
        assert_eq!(fs::read(backup).expect("backup bytes"), garbage);
        // The replacement is stable from now on.
        assert_eq!(
            DeviceIdentity::load_or_create(&root).expect("again"),
            DeviceLoad::Existing(identity.clone())
        );
    }

    #[test]
    fn json_missing_fields_is_recreated() {
        let (_dir, root) = root();
        root.ensure().expect("ensure");
        let partial = br#"{"device_id": "0192f0c1-7b1a-7cc0-8a4e-1f2d3c4b5a69", "name": "box"}"#;
        fs::write(root.device_file(), partial).expect("write");
        let load = DeviceIdentity::load_or_create(&root).expect("recover");
        let DeviceLoad::Recreated { identity, backup } = &load else {
            panic!("expected Recreated, got {load:?}");
        };
        assert_ne!(identity.device_id, "0192f0c1-7b1a-7cc0-8a4e-1f2d3c4b5a69");
        assert_eq!(fs::read(backup).expect("backup bytes"), partial);
    }

    /// A read error says nothing about the contents, so the identity must survive it: an error now,
    /// the same device id once the file is readable again, and nothing moved aside in between.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_device_json_is_an_error_and_keeps_the_identity() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, root) = root();
        let old = DeviceIdentity::load_or_create(&root)
            .expect("first")
            .identity()
            .clone();
        let before = fs::read(root.device_file()).expect("device.json");
        fs::set_permissions(root.device_file(), fs::Permissions::from_mode(0o000)).expect("chmod");
        if fs::read(root.device_file()).is_ok() {
            eprintln!("skipping: file permissions are not enforced for this user");
            return;
        }
        let result = DeviceIdentity::load_or_create(&root);
        fs::set_permissions(root.device_file(), fs::Permissions::from_mode(0o600))
            .expect("chmod back");
        let Err(crate::CoreError::Io { path, source }) = &result else {
            panic!("expected an I/O error, got {result:?}");
        };
        assert_eq!(path, &root.device_file());
        assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(root.device_file()).expect("kept"), before);
        let moved_aside = fs::read_dir(root.root()).expect("read_dir").any(|entry| {
            fsutil::file_name_lossy(&entry.expect("entry").path())
                .starts_with("device.json.corrupt-")
        });
        assert!(!moved_aside, "nothing was quarantined");
        assert_eq!(
            DeviceIdentity::load_or_create(&root).expect("readable again"),
            DeviceLoad::Existing(old)
        );
    }

    /// Something other than a file (here a socket) can't hold an identity, so it is moved aside like
    /// a directory would be.
    #[cfg(unix)]
    #[test]
    fn a_socket_in_place_of_device_json_is_moved_aside() {
        use std::os::unix::fs::FileTypeExt;
        let (_dir, root) = root();
        root.ensure().expect("ensure");
        let _listener =
            std::os::unix::net::UnixListener::bind(root.device_file()).expect("bind a socket");
        let load = DeviceIdentity::load_or_create(&root).expect("recover");
        let DeviceLoad::Recreated { backup, .. } = &load else {
            panic!("expected Recreated, got {load:?}");
        };
        assert!(
            fs::symlink_metadata(backup)
                .expect("moved socket")
                .file_type()
                .is_socket()
        );
        assert!(root.device_file().is_file());
    }

    #[test]
    fn a_directory_in_place_of_device_json_is_moved_aside() {
        let (_dir, root) = root();
        root.ensure().expect("ensure");
        fs::create_dir(root.device_file()).expect("directory named device.json");
        let load = DeviceIdentity::load_or_create(&root).expect("recover");
        let DeviceLoad::Recreated { backup, .. } = &load else {
            panic!("expected Recreated, got {load:?}");
        };
        assert!(backup.is_dir());
        assert!(root.device_file().is_file());
    }

    #[test]
    fn a_non_uuid_device_id_is_recreated() {
        let (_dir, root) = root();
        root.ensure().expect("ensure");
        for bad in ["", "my-mac", "0192F0C1-7B1A-7CC0-8A4E-1F2D3C4B5A69"] {
            let body = serde_json::json!({"device_id": bad, "name": "box", "platform": "linux", "created_at": 1});
            fs::write(root.device_file(), body.to_string()).expect("write");
            let load = DeviceIdentity::load_or_create(&root).expect("recover");
            assert!(
                matches!(load, DeviceLoad::Recreated { .. }),
                "{bad:?}: {load:?}"
            );
        }
    }

    #[test]
    fn unknown_fields_from_a_newer_version_are_accepted() {
        let (_dir, root) = root();
        root.ensure().expect("ensure");
        let body = serde_json::json!({
            "device_id": "0192f0c1-7b1a-7cc0-8a4e-1f2d3c4b5a69",
            "name": "box",
            "platform": "linux",
            "created_at": 5,
            "public_key": "added later"
        });
        fs::write(root.device_file(), body.to_string()).expect("write");
        let load = DeviceIdentity::load_or_create(&root).expect("load");
        assert!(
            matches!(load, DeviceLoad::Existing(ref identity) if identity.created_at == 5),
            "{load:?}"
        );
    }

    #[test]
    fn device_name_falls_back_when_unusable() {
        assert_eq!(device_name_from(None), FALLBACK_DEVICE_NAME);
        for raw in ["", "   ", "\n\t", ".local", " .LOCAL ", "\u{7}"] {
            assert_eq!(
                device_name_from(Some(OsStr::new(raw))),
                FALLBACK_DEVICE_NAME,
                "{raw:?}"
            );
        }
    }

    #[test]
    fn device_name_is_cleaned_up() {
        let name = |raw: &str| device_name_from(Some(OsStr::new(raw)));
        assert_eq!(name("  studio-box \n"), "studio-box");
        assert_eq!(name("Someones-MacBook-Pro.local"), "Someones-MacBook-Pro");
        assert_eq!(name("Someones-MacBook-Pro.LOCAL"), "Someones-MacBook-Pro");
        assert_eq!(name("build.example.com"), "build.example.com");
        assert_eq!(name("tab\tbed\u{0}name"), "tabbedname");
        assert_eq!(name("Ünïcödé-🚀"), "Ünïcödé-🚀");
    }

    #[test]
    fn device_name_is_capped_at_64_characters_on_char_boundaries() {
        let long_ascii = "a".repeat(200);
        assert_eq!(
            device_name_from(Some(OsStr::new(&long_ascii)))
                .chars()
                .count(),
            64
        );
        let long_multibyte = "é".repeat(100);
        let capped = device_name_from(Some(OsStr::new(&long_multibyte)));
        assert_eq!(capped, "é".repeat(64));
        // A cut that lands right after a space doesn't leave trailing whitespace.
        let spaced = format!("{} tail", "b".repeat(63));
        assert_eq!(device_name_from(Some(OsStr::new(&spaced))), "b".repeat(63));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_host_name_is_decoded_lossily() {
        use std::os::unix::ffi::OsStrExt;
        let name = device_name_from(Some(OsStr::from_bytes(b"box-\xff")));
        assert_eq!(name, "box-\u{fffd}");
    }

    #[test]
    fn platform_uses_rust_os_names() {
        let platform = current_platform();
        #[cfg(target_os = "macos")]
        assert_eq!(platform, "macos");
        #[cfg(target_os = "windows")]
        assert_eq!(platform, "windows");
        #[cfg(target_os = "linux")]
        assert_eq!(platform, "linux");
        assert!(!platform.is_empty());
    }
}
