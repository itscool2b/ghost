//! Filesystem helpers shared by the device, profile, and store modules.
//!
//! Two policies live here so every module follows them the same way:
//! - o7's own files are written atomically (temp file, fsync, rename), so a crash or power loss
//!   leaves either the old content or the new content, never a torn file;
//! - a file o7 cannot understand is moved aside (`<name>.corrupt-<millis>`), never deleted or
//!   silently overwritten, so nothing a user might want back is destroyed.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{CoreError, LinkProblem, Result};
use crate::ids::new_id;
use crate::time::now_millis;

/// Companion files SQLite keeps next to a database. Paths handed out by [`free_path`] are kept free
/// of these too, so a quarantined or backed-up database never collides with another's sidecars.
pub(crate) const SQLITE_SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// Suffix of a file still being written; see [`free_path`].
pub(crate) const PARTIAL_SUFFIX: &str = ".partial";

/// Upper bound on the `.<n>` collision counter before falling back to a random suffix.
const MAX_COLLISION_COUNTER: u32 = 999;

/// Creates `path` and any missing parents, readable only by the current user on unix.
///
/// Profiles hold chats that contain code and secrets, so directories o7 creates start out
/// private (subject to the umask, which can only remove bits). Existing directories are left as is;
/// see [`restrict_to_owner`] for tightening one o7 fully owns.
pub(crate) fn create_private_dir_all(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(CoreError::io(path))
}

/// Makes `path` a directory o7 can use: moves aside a file (or anything else that is neither
/// a folder nor a link) sitting there with [`quarantine_file`], then creates it with
/// [`create_private_dir_all`]. A stray file where a folder belongs must not make startup fail on
/// every launch. A link is never moved, since someone put the folder elsewhere on purpose (on
/// another disk, say): one that leads to a folder is used, and one that doesn't is a
/// [`CoreError::BrokenLink`]. Replacing a link whose disk is unplugged would start the data over on
/// this disk, and the history would stay behind the link once the disk is back. Returns where the
/// obstacle went, if there was one.
pub(crate) fn ensure_private_dir(path: &Path) -> Result<Option<PathBuf>> {
    let moved = match fs::symlink_metadata(path) {
        // A link's own metadata is never a directory, on Windows (junctions too) as on unix.
        Ok(meta) if meta.is_dir() => None,
        Ok(meta) if meta.file_type().is_symlink() => {
            check_link_to_dir(path)?;
            None
        }
        Ok(_) => {
            let moved = quarantine_file(path)?;
            tracing::warn!(
                path = %path.display(),
                moved = %moved.display(),
                "something other than a folder was in the way; moved it aside"
            );
            Some(moved)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(CoreError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    create_private_dir_all(path)?;
    Ok(moved)
}

/// A [`CoreError::BrokenLink`] when `path` is a link that doesn't lead to a folder, for callers that
/// would otherwise take such a folder for missing; anything else at `path` is left to them.
pub(crate) fn check_folder_link(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => check_link_to_dir(path),
        _ => Ok(()),
    }
}

/// Checks that the link at `link` leads to a folder; see [`ensure_private_dir`].
fn check_link_to_dir(link: &Path) -> Result<()> {
    let problem = match fs::metadata(link) {
        Ok(meta) if meta.is_dir() => return Ok(()),
        Ok(_) => LinkProblem::NotAFolder,
        Err(error) if error.kind() == io::ErrorKind::NotFound => LinkProblem::Missing,
        // A loop, or no permission to look: an error about the link, which stays as it is.
        Err(source) => {
            return Err(CoreError::Io {
                path: link.to_path_buf(),
                source,
            });
        }
    };
    let target = fs::read_link(link).map_err(CoreError::io(link))?;
    tracing::error!(
        link = %link.display(),
        target = %target.display(),
        ?problem,
        "a link to a data folder doesn't lead to a folder; leaving it as it is"
    );
    Err(CoreError::BrokenLink {
        link: link.to_path_buf(),
        target,
        problem,
    })
}

/// Best effort: removes group and other permissions from a directory that already existed (created
/// by an older build or by hand). Failure is logged, not fatal: the data is still usable.
#[cfg(unix)]
pub(crate) fn restrict_to_owner(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(path) {
        Ok(meta) if meta.permissions().mode() & 0o077 != 0 => {
            if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o700)) {
                tracing::warn!(path = %path.display(), %error, "could not make the directory private");
            }
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "could not read directory permissions")
        }
    }
}

/// Best effort: gives the directory a protected DACL (nothing inherited from its parent) that grants
/// full control to the current user, SYSTEM, and Administrators only, inherited by everything inside.
/// AppData is private already, but `O7_DATA_DIR` can put the data anywhere: `C:\aura` inherits
/// an ACL that lets every account on the PC read it and Authenticated Users change it. The DACL is
/// only written when it isn't owner-only yet, because writing one makes Windows walk every file below
/// to re-apply inheritance. Failure is logged, not fatal, as on unix.
#[cfg(windows)]
pub(crate) fn restrict_to_owner(path: &Path) {
    match is_owner_only(path) {
        Ok(true) => {}
        Ok(false) => {
            if let Err(error) = win_acl::set_owner_only(path) {
                tracing::warn!(path = %path.display(), %error, "could not make the directory private");
            }
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "could not read directory permissions")
        }
    }
}

/// Neither unix permissions nor Windows ACLs: nothing to tighten.
#[cfg(not(any(unix, windows)))]
pub(crate) fn restrict_to_owner(_path: &Path) {}

/// Whether `path` already has exactly the DACL [`restrict_to_owner`] writes on Windows.
#[cfg(windows)]
pub(crate) fn is_owner_only(path: &Path) -> io::Result<bool> {
    win_acl::read_dacl(path).map(|dacl| dacl.is_owner_only())
}

/// Result of reading one of o7's JSON files.
#[derive(Debug)]
pub(crate) enum JsonFile<T> {
    Missing,
    Valid(T),
    /// The path holds something o7 can't use: bytes that aren't the expected JSON, or something
    /// other than a regular file (a directory, a socket). Moving it aside loses nothing, since nothing
    /// better can be read from it.
    Unusable(CoreError),
    /// Reading failed: permission denied, an I/O error, too many open files, a Windows sharing
    /// violation. The file itself may be fine, so callers must not treat it as damaged.
    Unreadable(CoreError),
}

/// Reads and parses a JSON file, telling "missing", "unusable", and "unreadable" apart.
///
/// The type of the path is checked before reading: a FIFO would block the read forever, and on
/// Windows a directory fails to open with "access denied", which would pass for a permissions problem
/// instead of something in the way.
///
/// A UTF-8 byte order mark is skipped: some Windows editors add one when a user hand-edits a file, and
/// rejecting it would turn a harmless edit into "corrupt".
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> JsonFile<T> {
    let io_error = |source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    };
    match fs::metadata(path) {
        Ok(meta) if !meta.is_file() => {
            return JsonFile::Unusable(io_error(io::Error::other("it is not a regular file")));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return JsonFile::Missing,
        Err(source) => return JsonFile::Unreadable(io_error(source)),
    }
    match fs::read(path) {
        Ok(bytes) => {
            let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
            match serde_json::from_slice(body) {
                Ok(value) => JsonFile::Valid(value),
                Err(source) => JsonFile::Unusable(CoreError::Json {
                    path: path.to_path_buf(),
                    source,
                }),
            }
        }
        // Deleted between the two calls.
        Err(error) if error.kind() == io::ErrorKind::NotFound => JsonFile::Missing,
        Err(source) => JsonFile::Unreadable(io_error(source)),
    }
}

/// Writes `value` as pretty JSON to `path` atomically.
///
/// The bytes go to a temp file in the same directory (same filesystem, so the rename is atomic),
/// which is fsynced and then renamed over the target; `std::fs::rename` replaces an existing file on
/// Windows too. The directory is fsynced afterwards on unix so the rename itself survives power loss.
/// Files are created owner-only on unix.
pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| CoreError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Writes `bytes` to `path` atomically, as [`write_json_atomic`] writes its JSON.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let file_name = path.file_name().ok_or_else(|| CoreError::Io {
        path: path.to_path_buf(),
        source: io::Error::from(io::ErrorKind::InvalidInput),
    })?;
    let dir = parent_dir(path);
    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".tmp-{}", new_id()));
    let temp = dir.join(temp_name);

    if let Err(source) = write_synced_then_rename(&temp, path, bytes) {
        let _ = fs::remove_file(&temp);
        return Err(CoreError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    sync_dir(dir);
    Ok(())
}

fn write_synced_then_rename(temp: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = create_new_private(temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp, target)
}

/// Moves a file o7 could not use to `<name>.corrupt-<millis>` in the same directory and returns
/// the new path.
pub(crate) fn quarantine_file(path: &Path) -> Result<PathBuf> {
    let base = format!("{}.corrupt-{}", file_name_lossy(path), now_millis());
    let target = free_path(parent_dir(path), &base, "");
    fs::rename(path, &target).map_err(CoreError::io(path))?;
    Ok(target)
}

/// `dir/<base><suffix>`, or `dir/<base>.<n><suffix>` for the first free `n`, so two events in the same
/// millisecond never overwrite each other's files. "Free" also means no SQLite sidecar or partial
/// file of that name exists.
pub(crate) fn free_path(dir: &Path, base: &str, suffix: &str) -> PathBuf {
    let first = dir.join(format!("{base}{suffix}"));
    if is_free(&first) {
        return first;
    }
    for n in 1..=MAX_COLLISION_COUNTER {
        let candidate = dir.join(format!("{base}.{n}{suffix}"));
        if is_free(&candidate) {
            return candidate;
        }
    }
    dir.join(format!("{base}.{}{suffix}", new_id()))
}

/// `path` with `suffix` appended to its file name (`aura.db` + `-wal` = `aura.db-wal`). Works on the
/// raw OS string so non-UTF-8 names keep their exact bytes.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// The file name for building derived names (backups, quarantine). Lossy is fine here: the result
/// only has to be a recognisable, valid new name, never to match the original bytes.
pub(crate) fn file_name_lossy(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Win32's MAX_PATH less the terminating NUL: a plain path longer than this needs the verbatim
/// prefix to stay reachable for programs that are not long-path aware.
const MAX_PLAIN_PATH: usize = 259;

/// The plain spelling of a Windows verbatim path, which `std::fs::canonicalize` returns:
/// `\\?\C:\work` is `C:\work` and `\\?\UNC\server\share\work` is `\\server\share\work`. The prefix
/// goes only when the plain spelling names the same folder: it fits MAX_PATH and no part needs
/// the prefix (`.` or `..`, a trailing dot or space, a DOS device name such as `CON` or `COM1`, a
/// character Win32 parsing rejects, an empty part). Anything else, a volume GUID path for one, and
/// every non-verbatim path (all of them off Windows) comes back unchanged.
pub(crate) fn plain_path(path: &Path) -> PathBuf {
    match path.to_str().and_then(plain_windows_path) {
        Some(plain) => PathBuf::from(plain),
        None => path.to_path_buf(),
    }
}

/// [`plain_path`] on text: `Some` only when `text` is a verbatim path whose plain spelling is safe.
pub(crate) fn plain_windows_path(text: &str) -> Option<String> {
    let rest = text.strip_prefix(r"\\?\")?;
    let (root, tail) = match rest.get(..4) {
        Some(unc) if unc.eq_ignore_ascii_case(r"UNC\") => {
            let mut parts = rest[4..].splitn(3, '\\');
            let server = parts.next()?;
            let share = parts.next()?;
            if !plain_part(server) || !plain_part(share) {
                return None;
            }
            (format!(r"\\{server}\{share}"), parts.next().unwrap_or(""))
        }
        _ => {
            let bytes = rest.as_bytes();
            if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
                return None;
            }
            let tail = &rest[2..];
            if !tail.is_empty() && !tail.starts_with('\\') {
                return None;
            }
            (
                rest[..2].to_owned(),
                tail.strip_prefix('\\').unwrap_or(tail),
            )
        }
    };
    let mut plain = root;
    if tail.is_empty() {
        if !plain.starts_with(r"\\") {
            plain.push('\\');
        }
    } else {
        for part in tail.split('\\') {
            if !plain_part(part) {
                return None;
            }
            plain.push('\\');
            plain.push_str(part);
        }
    }
    (plain.encode_utf16().count() <= MAX_PLAIN_PATH).then_some(plain)
}

/// Whether one path part reads the same with and without the verbatim prefix.
fn plain_part(part: &str) -> bool {
    if part.is_empty()
        || part == "."
        || part == ".."
        || part.ends_with(['.', ' '])
        || part
            .chars()
            .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*'))
    {
        return false;
    }
    // A DOS device name stays one with an extension or trailing spaces before it: `CON.txt`.
    let stem = part.split('.').next().unwrap_or(part).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    let device = matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((upper.starts_with("COM") || upper.starts_with("LPT"))
        && upper.chars().count() == 4
        && upper
            .chars()
            .nth(3)
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³')));
    !device
}

/// The directory containing `path`; `.` for a bare file name.
pub(crate) fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn is_free(path: &Path) -> bool {
    let taken = |candidate: &Path| fs::symlink_metadata(candidate).is_ok();
    !taken(path)
        && !taken(&with_suffix(path, PARTIAL_SUFFIX))
        && SQLITE_SIDECARS
            .iter()
            .all(|suffix| !taken(&with_suffix(path, suffix)))
}

fn create_new_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Flushes a finished file to stable storage before it is renamed into place, so a power loss can't
/// leave a complete-looking name over incomplete bytes. On macOS `sync_all` is F_FULLFSYNC, which
/// also flushes the drive's write cache. Opened for writing because Windows' FlushFileBuffers needs a
/// handle with write access.
pub(crate) fn sync_file(path: &Path) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_all()
}

/// Best effort: some filesystems refuse to fsync a directory, and the rename already happened.
#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) {
    if let Err(error) = File::open(dir).and_then(|handle| handle.sync_all()) {
        tracing::debug!(dir = %dir.display(), %error, "could not fsync directory");
    }
}

/// Windows has no portable directory fsync through std; NTFS journals the rename metadata.
#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) {}

/// Flushes an open directory's entries (a rename, link or new file in it) to stable storage. cap-std
/// holds directories as `O_PATH` descriptors on Linux, which `fsync` refuses with EBADF, so this
/// syncs a readable descriptor of the same directory rather than the `Dir` itself.
#[cfg(unix)]
pub(crate) fn fsync_dir(dir: &cap_std::fs::Dir) -> io::Result<()> {
    dir.open(".")?.into_std().sync_all()
}

/// The Windows ACL calls behind [`restrict_to_owner`]. The workspace denies unsafe code, so the FFI
/// is confined to this module: each function that needs it carries `#[allow(unsafe_code)]` and one
/// unsafe block with its SAFETY comment.
#[cfg(windows)]
mod win_acl {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS,
        SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        CONTAINER_INHERIT_ACE, CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
        GetAclInformation, GetSecurityDescriptorControl, GetTokenInformation, OBJECT_INHERIT_ACE,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY, TOKEN_USER, TokenUser,
        WinBuiltinAdministratorsSid, WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Who an ACE is for, as far as an owner-only folder cares.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Who {
        User,
        System,
        Administrators,
        Other,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Ace {
        pub(crate) who: Who,
        /// An access-allowed ACE (anything else, deny ACEs included, is not what o7 writes).
        pub(crate) allow: bool,
        pub(crate) mask: u32,
        pub(crate) flags: u8,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct Dacl {
        /// Not inheriting anything from the parent folder.
        pub(crate) protected: bool,
        /// A null DACL grants everyone full access.
        pub(crate) null: bool,
        pub(crate) aces: Vec<Ace>,
    }

    impl Dacl {
        /// Exactly what [`set_owner_only`] writes: protected, only the three trustees, and the user
        /// with full control that folders and files below inherit.
        pub(crate) fn is_owner_only(&self) -> bool {
            let inherit = (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8;
            self.protected
                && !self.null
                && self
                    .aces
                    .iter()
                    .all(|ace| ace.allow && ace.who != Who::Other)
                && self.aces.iter().any(|ace| {
                    ace.who == Who::User
                        && ace.mask == FILE_ALL_ACCESS
                        && (ace.flags & inherit) == inherit
                })
        }
    }

    /// The SIDs of the three trustees, each in a buffer this struct owns, so the pointers handed to
    /// Windows stay valid for as long as it lives.
    struct Trustees {
        /// A `TOKEN_USER` (u64s for its pointer's alignment); the user's SID lives inside it.
        user: Vec<u64>,
        /// `SECURITY_MAX_SID_SIZE` (68) bytes each, as u32s for the SID's alignment.
        system: [u32; 17],
        administrators: [u32; 17],
    }

    impl Trustees {
        /// The SIDs are only read through these pointers (EqualSid, SetEntriesInAclW), never written,
        /// so handing out `*mut` pointers to the immutable buffers is sound.
        #[allow(unsafe_code)]
        fn sids(&self) -> [(Who, PSID); 3] {
            // SAFETY: `trustees()` only returns a `Trustees` whose `user` buffer holds a TOKEN_USER
            // written by GetTokenInformation, u64-aligned and at least `size_of::<TOKEN_USER>()`
            // bytes, so this read stays in bounds. The SID it points at lives in the same buffer.
            let user = unsafe { (*self.user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
            [
                (Who::User, user),
                (Who::System, self.system.as_ptr().cast_mut().cast()),
                (
                    Who::Administrators,
                    self.administrators.as_ptr().cast_mut().cast(),
                ),
            ]
        }
    }

    /// The process user's SID and the well-known SYSTEM and Administrators SIDs.
    #[allow(unsafe_code)]
    fn trustees() -> io::Result<Trustees> {
        let mut trustees = Trustees {
            user: Vec::new(),
            system: [0; 17],
            administrators: [0; 17],
        };
        // SAFETY: every out-pointer points at a live local or at a buffer whose size in bytes is
        // passed alongside it, and the token handle is closed on every path once it was opened.
        unsafe {
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0u32;
            // The sizing call fails with ERROR_INSUFFICIENT_BUFFER and sets `len`.
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
            trustees.user = vec![0u64; (len as usize).div_ceil(8)];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                trustees.user.as_mut_ptr().cast(),
                len,
                &mut len,
            );
            let error = io::Error::last_os_error();
            CloseHandle(token);
            if ok == 0 {
                return Err(error);
            }
            if trustees.user.len() * 8 < size_of::<TOKEN_USER>() {
                return Err(io::Error::other("the process token returned no user"));
            }
            for (kind, buffer) in [
                (WinLocalSystemSid, &mut trustees.system),
                (WinBuiltinAdministratorsSid, &mut trustees.administrators),
            ] {
                let mut size = size_of_val(buffer) as u32;
                if CreateWellKnownSid(kind, null_mut(), buffer.as_mut_ptr().cast(), &mut size) == 0
                {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        Ok(trustees)
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn win32(status: u32) -> io::Result<()> {
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status as i32))
        }
    }

    /// Replaces the DACL of `path` with a protected one granting full control to the user, SYSTEM,
    /// and Administrators, inherited by every folder and file below.
    #[allow(unsafe_code)]
    pub(crate) fn set_owner_only(path: &Path) -> io::Result<()> {
        let trustees = trustees()?;
        let entries = trustees.sids().map(|(_, sid)| EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS,
            grfAccessMode: SET_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                // With TRUSTEE_IS_SID the name field carries the SID pointer.
                ptstrName: sid.cast(),
            },
        });
        let wide = wide(path);
        // SAFETY: `entries` and the SIDs they point at (owned by `trustees`) outlive both calls;
        // `wide` is a NUL-terminated UTF-16 path; `acl` is allocated by SetEntriesInAclW and freed
        // with LocalFree exactly once, after its last use.
        unsafe {
            let mut acl: *mut ACL = null_mut();
            win32(SetEntriesInAclW(
                entries.len() as u32,
                entries.as_ptr(),
                null(),
                &mut acl,
            ))?;
            let status = SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl,
                null(),
            );
            LocalFree(acl.cast());
            win32(status)
        }
    }

    /// Reads the DACL of `path` back, naming the trustees o7 cares about.
    #[allow(unsafe_code)]
    pub(crate) fn read_dacl(path: &Path) -> io::Result<Dacl> {
        let trustees = trustees()?;
        let sids = trustees.sids();
        let wide = wide(path);
        // SAFETY: the out-pointers point at live locals; `descriptor` is allocated by
        // GetNamedSecurityInfoW and freed with LocalFree exactly once, after the last use of `dacl`
        // (which points into it); GetAce is only asked for indexes below the ACE count that
        // GetAclInformation reported, and every ACE starts with an ACE_HEADER, and an access-allowed
        // ACE is an ACCESS_ALLOWED_ACE whose SID starts at `SidStart`.
        unsafe {
            let mut dacl: *mut ACL = null_mut();
            let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
            win32(GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            ))?;
            let result: io::Result<Dacl> = 'read: {
                let mut control = 0u16;
                let mut revision = 0u32;
                if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0 {
                    break 'read Err(io::Error::last_os_error());
                }
                let mut read = Dacl {
                    protected: control & SE_DACL_PROTECTED != 0,
                    null: dacl.is_null(),
                    aces: Vec::new(),
                };
                if dacl.is_null() {
                    break 'read Ok(read);
                }
                let mut info = ACL_SIZE_INFORMATION::default();
                if GetAclInformation(
                    dacl,
                    (&raw mut info).cast(),
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                ) == 0
                {
                    break 'read Err(io::Error::last_os_error());
                }
                for index in 0..info.AceCount {
                    let mut ace: *mut c_void = null_mut();
                    if GetAce(dacl, index, &mut ace) == 0 {
                        break 'read Err(io::Error::last_os_error());
                    }
                    let header = *ace.cast::<ACE_HEADER>();
                    let allow = u32::from(header.AceType) == ACCESS_ALLOWED_ACE_TYPE;
                    let (who, mask) = if allow {
                        let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
                        let sid: PSID = (&raw mut (*allowed).SidStart).cast();
                        let who = sids
                            .iter()
                            .find(|(_, trustee)| EqualSid(sid, *trustee) != 0)
                            .map_or(Who::Other, |(who, _)| *who);
                        (who, (*allowed).Mask)
                    } else {
                        (Who::Other, 0)
                    };
                    read.aces.push(Ace {
                        who,
                        allow,
                        mask,
                        flags: header.AceFlags,
                    });
                }
                Ok(read)
            };
            LocalFree(descriptor.cast());
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// On Linux the `Dir` itself is an `O_PATH` descriptor that `fsync` refuses, which failed
    /// every encrypted profile write, Revert's recovery copies and clone journals there.
    #[cfg(unix)]
    #[test]
    fn an_open_directory_syncs_through_a_readable_descriptor() {
        let temp = tempfile::tempdir().unwrap();
        let dir =
            cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        dir.write("entry", b"fixture").unwrap();
        fsync_dir(&dir).unwrap();
        let nested = {
            dir.create_dir("nested").unwrap();
            dir.open_dir("nested").unwrap()
        };
        fsync_dir(&nested).unwrap();
    }

    #[test]
    fn verbatim_paths_take_their_plain_spelling_when_it_names_the_same_folder() {
        let plain = |text: &str| plain_windows_path(text);
        assert_eq!(
            plain(r"\\?\C:\Users\me\work").as_deref(),
            Some(r"C:\Users\me\work")
        );
        assert_eq!(plain(r"\\?\d:\").as_deref(), Some(r"d:\"));
        assert_eq!(plain(r"\\?\C:").as_deref(), Some(r"C:\"));
        assert_eq!(
            plain(r"\\?\UNC\server\share\repo").as_deref(),
            Some(r"\\server\share\repo")
        );
        assert_eq!(
            plain(r"\\?\unc\server\share").as_deref(),
            Some(r"\\server\share")
        );
        assert_eq!(
            plain(r"\\?\C:\Users\me\café 1.2\x").as_deref(),
            Some(r"C:\Users\me\café 1.2\x"),
            "dots and spaces inside a name are fine"
        );
        assert_eq!(
            plain(r"\\?\C:\console\COM10\lpt\NULL").as_deref(),
            Some(r"C:\console\COM10\lpt\NULL"),
            "names that only start like a device are fine"
        );
    }

    #[test]
    fn verbatim_paths_keep_their_prefix_when_the_plain_spelling_would_differ() {
        for text in [
            r"\\?\C:\work\.",
            r"\\?\C:\work\..\other",
            r"\\?\C:\work\trailing.",
            r"\\?\C:\work\trailing ",
            r"\\?\C:\work\CON",
            r"\\?\C:\work\con.txt",
            r"\\?\C:\work\nul .log",
            r"\\?\C:\work\COM1",
            r"\\?\C:\work\lpt9\x",
            r"\\?\C:\work\COM¹",
            r"\\?\C:\work\CONOUT$",
            r"\\?\C:\work\a:b",
            r"\\?\C:\work\a/b",
            r"\\?\C:\work\a*b",
            r"\\?\C:\work\\double",
            r"\\?\C:\work\",
            r"\\?\Volume{0b1c2d3e-0000-0000-0000-100000000000}\work",
            r"\\?\GLOBALROOT\Device\HarddiskVolume3\work",
            r"\\?\UNC\server",
            r"\\?\UNC\\share\repo",
            r"\\?\1:\work",
            r"\\?\C:work",
        ] {
            assert_eq!(plain_windows_path(text), None, "{text}");
        }
        let long = format!(r"\\?\C:\{}", "a".repeat(MAX_PLAIN_PATH - 3));
        assert_eq!(
            plain_windows_path(&long).map(|plain| plain.len()),
            Some(MAX_PLAIN_PATH),
            "exactly MAX_PATH less the NUL still fits"
        );
        let longer = format!(r"\\?\C:\{}", "a".repeat(MAX_PLAIN_PATH - 2));
        assert_eq!(plain_windows_path(&longer), None, "longer keeps the prefix");
        let wide = format!(r"\\?\C:\{}", "😀".repeat(129));
        assert_eq!(
            plain_windows_path(&wide),
            None,
            "the limit counts UTF-16 units"
        );
    }

    #[test]
    fn plain_paths_and_other_platforms_pass_through() {
        for text in [
            r"C:\Users\me\work",
            r"\\server\share\repo",
            "/Users/me/work",
            "relative",
            r"\\.\C:\work",
        ] {
            assert_eq!(plain_path(Path::new(text)), PathBuf::from(text), "{text}");
            assert_eq!(plain_windows_path(text), None, "{text}");
        }
        assert_eq!(
            plain_path(Path::new(r"\\?\C:\work")),
            PathBuf::from(r"C:\work")
        );
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Sample {
        name: String,
        count: u32,
    }

    fn sample(count: u32) -> Sample {
        Sample {
            name: "ü 🚀".into(),
            count,
        }
    }

    #[test]
    fn atomic_write_round_trips_and_replaces() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        write_json_atomic(&path, &sample(1)).expect("first write");
        write_json_atomic(&path, &sample(2)).expect("second write");
        match read_json::<Sample>(&path) {
            JsonFile::Valid(value) => assert_eq!(value, sample(2)),
            other => panic!("expected valid JSON, got {other:?}"),
        }
        // No temp files are left behind.
        let names: Vec<_> = fs::read_dir(dir.path())
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(names, vec![OsString::from("state.json")]);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_creates_owner_only_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("device.json");
        write_json_atomic(&path, &sample(1)).expect("write");
        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn atomic_write_into_a_missing_directory_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing").join("state.json");
        assert!(matches!(
            write_json_atomic(&path, &sample(1)),
            Err(CoreError::Io { .. })
        ));
    }

    #[test]
    fn read_json_distinguishes_missing_from_broken() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("x.json");
        assert!(matches!(read_json::<Sample>(&path), JsonFile::Missing));
        fs::write(&path, b"{not json").expect("write");
        assert!(matches!(
            read_json::<Sample>(&path),
            JsonFile::Unusable(CoreError::Json { .. })
        ));
        fs::write(&path, br#"{"name":"x"}"#).expect("write");
        assert!(matches!(
            read_json::<Sample>(&path),
            JsonFile::Unusable(CoreError::Json { .. })
        ));
        // A directory where the file should be is unusable (safe to move aside), not "missing" and
        // not a read error. On Windows opening it would fail with "access denied", which would pass
        // for a permissions problem if the type weren't checked first.
        fs::remove_file(&path).expect("remove");
        fs::create_dir(&path).expect("mkdir");
        assert!(matches!(
            read_json::<Sample>(&path),
            JsonFile::Unusable(CoreError::Io { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn read_json_reports_a_read_error_as_unreadable_not_unusable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("x.json");
        fs::write(&path, br#"{"name":"a","count":1}"#).expect("write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod");
        if fs::read(&path).is_ok() {
            eprintln!("skipping: file permissions are not enforced for this user");
            return;
        }
        let read = read_json::<Sample>(&path);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod back");
        assert!(
            matches!(read, JsonFile::Unreadable(CoreError::Io { ref source, .. }) if source.kind() == io::ErrorKind::PermissionDenied),
            "{read:?}"
        );
    }

    /// A socket (like a FIFO, which would block a read forever) is something other than a file.
    #[cfg(unix)]
    #[test]
    fn read_json_treats_a_socket_as_unusable_without_reading_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("x.json");
        let _listener = std::os::unix::net::UnixListener::bind(&path).expect("bind a socket");
        assert!(matches!(
            read_json::<Sample>(&path),
            JsonFile::Unusable(CoreError::Io { .. })
        ));
    }

    #[test]
    fn ensure_private_dir_moves_a_file_aside_and_keeps_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocked = dir.path().join("logs");
        fs::write(&blocked, b"stray").expect("write");
        let moved = ensure_private_dir(&blocked)
            .expect("ensure")
            .expect("the file was moved");
        assert!(blocked.is_dir());
        assert_eq!(moved.parent(), Some(dir.path()));
        assert!(file_name_lossy(&moved).starts_with("logs.corrupt-"));
        assert_eq!(fs::read(&moved).expect("kept"), b"stray");
        // A directory, existing or new, is left alone.
        fs::write(blocked.join("inside"), b"x").expect("write");
        assert_eq!(ensure_private_dir(&blocked).expect("ensure"), None);
        assert!(blocked.join("inside").is_file());
        let fresh = dir.path().join("a").join("b");
        assert_eq!(ensure_private_dir(&fresh).expect("ensure"), None);
        assert!(fresh.is_dir());
    }

    #[cfg(unix)]
    fn is_link_to(path: &Path, target: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
            && fs::read_link(path).is_ok_and(|read| read == target)
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_dir_uses_a_symlinked_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let elsewhere = dir.path().join("on another disk");
        fs::create_dir(&elsewhere).expect("mkdir");
        let linked = dir.path().join("checkpoints");
        std::os::unix::fs::symlink(&elsewhere, &linked).expect("symlink");
        assert_eq!(ensure_private_dir(&linked).expect("ensure"), None);
        assert!(is_link_to(&linked, &elsewhere));
    }

    /// Finding: a link whose target was missing (its disk unplugged) was moved aside and replaced
    /// by an empty folder, so the data forked onto this disk and the history stayed behind the
    /// link once the disk was back.
    #[cfg(unix)]
    #[test]
    fn ensure_private_dir_reports_a_dangling_link_and_leaves_it_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gone = dir.path().join("unplugged disk").join("attachments");
        let dangling = dir.path().join("attachments");
        std::os::unix::fs::symlink(&gone, &dangling).expect("symlink");
        let error = ensure_private_dir(&dangling).expect_err("a dangling link is an error");
        assert!(
            matches!(
                &error,
                CoreError::BrokenLink { link, target, problem: LinkProblem::Missing }
                    if *link == dangling && *target == gone
            ),
            "{error:?}"
        );
        assert!(is_link_to(&dangling, &gone), "the link is left as it was");
        assert!(!gone.exists(), "nothing is created where the link leads");
        let names: Vec<OsString> = fs::read_dir(dir.path())
            .expect("read_dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(
            names,
            vec![OsString::from("attachments")],
            "nothing moved aside"
        );

        // Plugged back in: used as it is.
        fs::create_dir_all(&gone).expect("the disk is back");
        assert_eq!(ensure_private_dir(&dangling).expect("ensure"), None);
        assert!(is_link_to(&dangling, &gone));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_dir_reports_a_link_to_a_file_and_leaves_it_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("notes.txt");
        fs::write(&file, b"not a folder").expect("write");
        let linked = dir.path().join("logs");
        std::os::unix::fs::symlink(&file, &linked).expect("symlink");
        assert!(matches!(
            ensure_private_dir(&linked),
            Err(CoreError::BrokenLink {
                problem: LinkProblem::NotAFolder,
                ..
            })
        ));
        assert!(is_link_to(&linked, &file));
        assert_eq!(fs::read(&file).expect("read"), b"not a folder");
    }

    /// A link that loops can't be followed; that is an error about the link, which stays.
    #[cfg(unix)]
    #[test]
    fn ensure_private_dir_reports_a_link_loop_without_touching_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let looped = dir.path().join("backups");
        std::os::unix::fs::symlink(&looped, &looped).expect("symlink");
        assert!(matches!(
            ensure_private_dir(&looped),
            Err(CoreError::Io { ref path, .. }) if *path == looped
        ));
        assert!(is_link_to(&looped, &looped));
    }

    #[test]
    fn sync_file_flushes_an_existing_file_and_fails_on_a_missing_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aura.db.v1-5.bak.partial");
        fs::write(&path, b"backup bytes").expect("write");
        sync_file(&path).expect("sync");
        assert_eq!(fs::read(&path).expect("read"), b"backup bytes");
        assert!(sync_file(&dir.path().join("missing")).is_err());
    }

    /// Reads the ACL back after `restrict_to_owner`: protected, only the user, SYSTEM, and
    /// Administrators, and inherited by what is created inside afterwards.
    #[cfg(windows)]
    #[test]
    fn restrict_to_owner_sets_a_protected_owner_only_acl_on_windows() {
        use windows_sys::Win32::Security::INHERITED_ACE;
        use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
        let dir = tempfile::tempdir().expect("tempdir");
        let private = dir.path().join("profiles");
        create_private_dir_all(&private).expect("mkdir");
        let before = win_acl::read_dacl(&private).expect("read the ACL");
        assert!(
            !before.protected,
            "a new folder inherits its parent's ACL: {before:?}"
        );
        assert!(!before.is_owner_only());

        restrict_to_owner(&private);
        let after = win_acl::read_dacl(&private).expect("read the ACL");
        assert!(after.protected, "{after:?}");
        assert!(after.is_owner_only(), "{after:?}");
        let mut who: Vec<win_acl::Who> = after.aces.iter().map(|ace| ace.who).collect();
        who.sort_by_key(|who| *who as u8);
        who.dedup();
        assert_eq!(
            who,
            vec![
                win_acl::Who::User,
                win_acl::Who::System,
                win_acl::Who::Administrators
            ],
            "{after:?}"
        );
        assert!(
            after
                .aces
                .iter()
                .all(|ace| ace.allow && ace.mask == FILE_ALL_ACCESS),
            "{after:?}"
        );

        // Created afterwards: inherits exactly those entries.
        let child = private.join("aura.db");
        fs::write(&child, b"x").expect("write");
        let inherited = win_acl::read_dacl(&child).expect("read the child's ACL");
        assert!(!inherited.aces.is_empty(), "{inherited:?}");
        assert!(
            inherited.aces.iter().all(|ace| ace.allow
                && ace.who != win_acl::Who::Other
                && (ace.flags & (INHERITED_ACE as u8)) != 0),
            "{inherited:?}"
        );

        // Already owner-only: left as is (no second write that would walk the folder again).
        restrict_to_owner(&private);
        assert_eq!(win_acl::read_dacl(&private).expect("read the ACL"), after);
    }

    #[test]
    fn read_json_skips_a_utf8_bom() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("x.json");
        fs::write(&path, b"\xEF\xBB\xBF{\"name\":\"a\",\"count\":3}").expect("write");
        match read_json::<Sample>(&path) {
            JsonFile::Valid(value) => assert_eq!(value.count, 3),
            other => panic!("expected valid JSON, got {other:?}"),
        }
    }

    #[test]
    fn quarantine_moves_the_file_aside_with_its_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("device.json");
        fs::write(&path, b"garbage").expect("write");
        let moved = quarantine_file(&path).expect("quarantine");
        assert!(!path.exists());
        assert_eq!(moved.parent(), Some(dir.path()));
        let name = file_name_lossy(&moved);
        assert!(name.starts_with("device.json.corrupt-"), "{name}");
        assert_eq!(fs::read(&moved).expect("read"), b"garbage");
    }

    #[test]
    fn free_path_never_reuses_a_taken_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = free_path(dir.path(), "aura.db.corrupt-5", "");
        assert_eq!(first, dir.path().join("aura.db.corrupt-5"));
        fs::write(&first, b"x").expect("write");
        let second = free_path(dir.path(), "aura.db.corrupt-5", "");
        assert_eq!(second, dir.path().join("aura.db.corrupt-5.1"));
        // A sidecar alone also makes a name taken.
        fs::write(with_suffix(&second, "-wal"), b"x").expect("write");
        let third = free_path(dir.path(), "aura.db.corrupt-5", "");
        assert_eq!(third, dir.path().join("aura.db.corrupt-5.2"));
        // Suffixes go after the counter.
        fs::write(dir.path().join("aura.db.v1-7.bak"), b"x").expect("write");
        assert_eq!(
            free_path(dir.path(), "aura.db.v1-7", ".bak"),
            dir.path().join("aura.db.v1-7.1.bak")
        );
    }

    #[test]
    fn helpers_handle_bare_names() {
        assert_eq!(parent_dir(Path::new("aura.db")), Path::new("."));
        assert_eq!(parent_dir(Path::new("/a/aura.db")), Path::new("/a"));
        assert_eq!(
            with_suffix(Path::new("/a/aura.db"), "-wal"),
            PathBuf::from("/a/aura.db-wal")
        );
        assert_eq!(file_name_lossy(Path::new("/a/aura.db")), "aura.db");
    }

    #[test]
    fn private_dirs_are_created_recursively() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b");
        create_private_dir_all(&nested).expect("create");
        create_private_dir_all(&nested).expect("idempotent");
        assert!(nested.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&nested)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn a_file_in_the_way_of_a_directory_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("profiles");
        fs::write(&blocker, b"x").expect("write");
        assert!(matches!(
            create_private_dir_all(&blocker),
            Err(CoreError::Io { .. })
        ));
    }
}
