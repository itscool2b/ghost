//! Restrict Git's batch durability strategy to the filesystems documented by Git.
//! Unknown filesystems or failed detection retain full per-object fsync.

use std::path::Path;

#[cfg(windows)]
#[allow(unsafe_code)] // Read-only native volume query, with the handle/buffer contract below.
pub(super) fn supports_batch(repository: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, GetVolumeInformationByHandleW,
    };

    let Ok(directory) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(repository)
    else {
        return false;
    };
    if !directory.metadata().is_ok_and(|metadata| metadata.is_dir()) {
        return false;
    }
    let mut name = [0u16; 32];
    // SAFETY: the live directory owns the handle; the only output buffer has the supplied
    // length, and all optional outputs are null. The handle identifies the actual volume,
    // including when the profile's path traverses a mount point or junction.
    let success = unsafe {
        GetVolumeInformationByHandleW(
            directory.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if success == 0 {
        return false;
    }
    let Some(end) = name.iter().position(|unit| *unit == 0) else {
        return false;
    };
    String::from_utf16(&name[..end]).is_ok_and(|name| recognized_filesystem(&name))
}

#[cfg(windows)]
fn recognized_filesystem(name: &str) -> bool {
    name.eq_ignore_ascii_case("NTFS") || name.eq_ignore_ascii_case("ReFS")
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // Read-only native volume query, with the descriptor/buffer contract below.
pub(super) fn supports_batch(repository: &Path) -> bool {
    use std::os::fd::AsRawFd;

    let Ok(directory) = std::fs::File::open(repository) else {
        return false;
    };
    if !directory.metadata().is_ok_and(|metadata| metadata.is_dir()) {
        return false;
    }
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs receives a live directory descriptor and enough writable storage for
    // statfs. The structure is read only after the system call reports successful initialization.
    if unsafe { libc::fstatfs(directory.as_raw_fd(), info.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: the successful fstatfs call initialized the structure above.
    let info = unsafe { info.assume_init() };
    let name: Vec<u8> = info.f_fstypename.iter().map(|byte| *byte as u8).collect();
    let Some(end) = name.iter().position(|byte| *byte == 0) else {
        return false;
    };
    std::str::from_utf8(&name[..end]).is_ok_and(recognized_filesystem)
}

#[cfg(target_os = "macos")]
fn recognized_filesystem(name: &str) -> bool {
    matches!(name, "apfs" | "hfs")
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn supports_batch(_repository: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_paths_and_files_never_enable_batch_durability() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!supports_batch(&temp.path().join("absent")));
        let file = temp.path().join("file");
        std::fs::write(&file, b"fixture").unwrap();
        assert!(!supports_batch(&file));
    }

    #[cfg(windows)]
    #[test]
    fn only_documented_windows_filesystems_enable_batch_durability() {
        for name in ["NTFS", "ntfs", "ReFS", "refs"] {
            assert!(recognized_filesystem(name));
        }
        for name in ["", "FAT32", "exFAT", "APFS", "ext4", "NTFS-like"] {
            assert!(!recognized_filesystem(name));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_documented_macos_filesystems_enable_batch_durability() {
        for name in ["apfs", "hfs"] {
            assert!(recognized_filesystem(name));
        }
        for name in ["", "msdos", "exfat", "ntfs", "nfs", "apfs-like"] {
            assert!(!recognized_filesystem(name));
        }
    }
}
