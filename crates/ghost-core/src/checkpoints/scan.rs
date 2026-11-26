use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{CheckpointError, Result};

pub(super) const FILE_LIMIT: usize = 10 * 1024 * 1024;
pub(super) const TOTAL_LIMIT: usize = 100 * 1024 * 1024;
const COUNT_LIMIT: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Fingerprint {
    pub(super) sha256: String,
    pub(crate) bytes: u32,
    pub(super) mode: u32,
    pub(super) binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedFile {
    pub(super) fingerprint: Fingerprint,
    pub(super) oid: String,
}

pub(super) struct Scan {
    pub(super) files: BTreeMap<String, Fingerprint>,
    pub(super) copies: Vec<(String, PathBuf)>,
    pub(super) warnings: Vec<String>,
    pub(super) excluded: BTreeSet<String>,
}

pub(super) fn paths(bytes: &[u8]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for path in bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        if paths.len() >= COUNT_LIMIT {
            return Err(CheckpointError::Message(
                "This folder exceeds the checkpoint limit of 20,000 files.".into(),
            ));
        }
        let path = std::str::from_utf8(path).map_err(|_| {
            CheckpointError::Message(
                "A filename cannot be represented in this app. No checkpoint was created.".into(),
            )
        })?;
        let path = path.trim_end_matches('/');
        validate(path)?;
        paths.push(path.to_owned());
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub(crate) fn validate(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains(['\\', ':'])
        || path.chars().any(char::is_control)
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || path.split('/').any(|part| {
            [
                ".git",
                ".claude",
                ".codex",
                ".pi",
                ".omp",
                ".grok",
                ".hermes",
                ".ssh",
                ".gnupg",
                ".aws",
                ".azure",
                "keychains",
                "credentials",
                "credentials.json",
                "auth.json",
                "auth.toml",
                ".npmrc",
                ".netrc",
                "id_rsa",
                "id_ed25519",
                ".env",
            ]
            .iter()
            .any(|name| part.eq_ignore_ascii_case(name))
                || part.starts_with(".auracode-revert-")
                || part.starts_with(".auracode-transfer-")
                || part.to_ascii_lowercase().starts_with(".env.")
                || [".key", ".pem", ".p12", ".keychain-db"]
                    .iter()
                    .any(|suffix| part.to_ascii_lowercase().ends_with(suffix))
        })
    {
        return Err(CheckpointError::Message(
            "The checkpoint contains an unsupported or unsafe file path.".into(),
        ));
    }
    Ok(())
}

/// All parents must still be real directories. Symlinks and Windows reparse points are excluded.
pub(super) fn checked_path(root: &Path, relative: &str) -> Result<PathBuf> {
    validate(relative)?;
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(meta) if is_link(&meta) => {
                return Err(CheckpointError::Message(format!(
                    "{relative}: links and reparse points are excluded from checkpoints."
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

pub(crate) fn read(root: &Path, relative: &str) -> Result<Option<(Fingerprint, Vec<u8>)>> {
    let path = checked_path(root, relative)?;
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !meta.is_file() {
        return Err(CheckpointError::Message(format!(
            "{relative}: directories, submodules and special files are excluded."
        )));
    }
    if meta.len() > FILE_LIMIT as u64 {
        return Err(CheckpointError::Message(format!(
            "{relative}: exceeds the 10 MiB per-file checkpoint limit."
        )));
    }
    // Even if a directory changes after the metadata check, capability-relative open cannot
    // traverse beyond root. Metadata from the opened handle is authoritative for mode/size.
    let mut directory = cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    let mut components = Path::new(relative).components().peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            break;
        }
        directory = directory.open_dir_nofollow(component.as_os_str())?;
    }
    let name = Path::new(relative)
        .file_name()
        .ok_or_else(|| CheckpointError::Message("The checkpoint path has no file name.".into()))?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = directory.open_with(name, &options)?.into_std();
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > FILE_LIMIT as u64 {
        return Err(CheckpointError::Message(format!(
            "{relative}: changed type or exceeded the checkpoint limit."
        )));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > FILE_LIMIT {
        return Err(CheckpointError::Message(format!(
            "{relative}: grew beyond the checkpoint file limit."
        )));
    }
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            0o100755
        } else {
            0o100644
        }
    };
    #[cfg(not(unix))]
    let mode = 0o100644;
    let fingerprint = Fingerprint {
        sha256: crate::chatstore::rows::hex(&Sha256::digest(&bytes)),
        bytes: bytes.len() as u32,
        mode,
        binary: bytes.contains(&0) || std::str::from_utf8(&bytes).is_err(),
    };
    Ok(Some((fingerprint, bytes)))
}

#[cfg(test)]
pub(super) fn collect(root: &Path, paths: &[String], copy_to: Option<&Path>) -> Result<Scan> {
    collect_with(root, paths, copy_to, None, None)
}

/// Always read and hash current bytes. Metadata alone never authorizes object reuse.
pub(super) fn collect_with(
    root: &Path,
    paths: &[String],
    copy_to: Option<&Path>,
    previous: Option<&BTreeMap<String, SavedFile>>,
    context: Option<&super::coordination::Context>,
) -> Result<Scan> {
    let mut result = Scan {
        files: BTreeMap::new(),
        copies: Vec::new(),
        warnings: Vec::new(),
        excluded: BTreeSet::new(),
    };
    let mut total = 0usize;
    let mut omitted = 0usize;
    for path in paths {
        if let Some(context) = context {
            context.check()?;
        }
        match read(root, path) {
            Ok(Some((fingerprint, bytes))) => {
                total += bytes.len();
                if total > TOTAL_LIMIT {
                    return Err(CheckpointError::Message(
                        "This folder exceeds the 100 MiB checkpoint limit.".into(),
                    ));
                }
                let reused = previous
                    .and_then(|files| files.get(path))
                    .is_some_and(|saved| {
                        saved.fingerprint.sha256 == fingerprint.sha256
                            && saved.fingerprint.bytes == fingerprint.bytes
                    });
                if let Some(copy_to) = copy_to.filter(|_| !reused) {
                    if let Some(context) = context {
                        context.check()?;
                    }
                    let copy = copy_to.join(result.files.len().to_string());
                    fs::write(&copy, bytes)?;
                    result.copies.push((path.clone(), copy));
                }
                result.files.insert(path.clone(), fingerprint);
            }
            Ok(None) => {}
            Err(error) => {
                result.excluded.insert(path.clone());
                omitted += 1;
                if result.warnings.len() < 100 {
                    result.warnings.push(error.to_string());
                }
            }
        }
    }
    if let Some(context) = context {
        context.check()?;
    }
    if omitted > 100 {
        result
            .warnings
            .push(format!("{} more paths were excluded.", omitted - 100));
    }
    Ok(result)
}
