//! Clone lifecycle only. Git output, progress, environment and credentials never enter this file.
//! Recovery reads destinations but never removes or repairs them. Dismissal removes one record.
use super::*;
use cap_fs_ext::{DirExt, OpenOptionsFollowExt, OpenOptionsSyncExt};
use ghost_protocol::setup_actions::CloneRecovery;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

const MAX_RECORD: u64 = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    volume: u64,
    file: u64,
}

impl Identity {
    // Stable Windows directory identities require the handle API. The two unsafe operations
    // below only initialize a plain output struct and borrow this function's owned handle.
    #[cfg_attr(windows, allow(unsafe_code))]
    fn read(dir: &Dir) -> std::io::Result<Self> {
        let file = dir.try_clone()?.into_std_file();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = file.metadata()?;
            Ok(Self {
                volume: meta.dev(),
                file: meta.ino(),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
            };
            // SAFETY: BY_HANDLE_FILE_INFORMATION consists entirely of integer fields and FILETIMEs.
            let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            // The retained directory handle is valid for the duration of this call.
            if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut info) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self {
                volume: u64::from(info.dwVolumeSerialNumber),
                file: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(std::io::Error::other(
                "Folder identity is unavailable on this platform.",
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Planned,
    Created,
    GitFinished,
    Ready,
    Failed,
    Canceled,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    id: String,
    remote: String,
    repository: String,
    parent: PathBuf,
    name: String,
    parent_identity: Identity,
    destination_identity: Option<Identity>,
    git_identity: Option<Identity>,
    phase: Phase,
}

pub(super) struct Journal {
    directory: Dir,
    path: PathBuf,
    record: Mutex<Record>,
}

fn fail(path: &Path, reason: impl std::fmt::Display) -> CloneError {
    CloneError::Message(format!(
        "Clone recovery could not be saved at {}: {reason}. Check the record and destination before retrying.",
        path.display()
    ))
}

fn sync(dir: &Dir) -> std::io::Result<()> {
    #[cfg(unix)]
    crate::fsutil::fsync_dir(dir)?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn directory(root: &Path, create: bool) -> Result<Option<Dir>> {
    let parent = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    if create {
        match parent.create_dir("clones") {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        // Retry the parent sync even if a previous creation succeeded but its sync failed.
        sync(&parent)?;
    }
    match parent.open_dir_nofollow("clones") {
        Ok(dir) => Ok(Some(dir)),
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn filename(id: &str) -> Result<String> {
    if uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id) {
        Ok(format!("{id}.json"))
    } else {
        Err(CloneError::message(
            "This clone recovery identifier is invalid.",
        ))
    }
}

fn bytes(dir: &Dir, name: &str) -> Result<Vec<u8>> {
    let metadata = dir.symlink_metadata(name)?;
    if !metadata.is_file() || metadata.len() > MAX_RECORD {
        return Err(CloneError::message(
            "The recovery record is not a bounded regular file.",
        ));
    }
    let mut file = dir.open_with(
        name,
        cap_std::fs::OpenOptions::new()
            .read(true)
            .follow(cap_fs_ext::FollowSymlinks::No)
            .nonblock(true),
    )?;
    if !file.metadata()?.is_file() {
        return Err(CloneError::message(
            "The recovery record changed to a non-file.",
        ));
    }
    let mut body = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_RECORD + 1)
        .read_to_end(&mut body)?;
    if body.len() as u64 > MAX_RECORD {
        return Err(CloneError::message("The recovery record is too large."));
    }
    Ok(body)
}

fn encode(record: &Record) -> Result<Vec<u8>> {
    serde_json::to_vec(record)
        .map_err(|_| CloneError::message("The clone record could not be encoded."))
}

impl Journal {
    pub(super) fn begin(root: &Path, prepared: &Prepared) -> Result<Arc<Self>> {
        let path = root.join("clones").join(filename(&prepared.view.id)?);
        let directory = directory(root, true)?.ok_or_else(|| fail(&path, "storage unavailable"))?;
        let record = Record {
            version: 1,
            id: prepared.view.id.clone(),
            remote: prepared.view.remote.clone(),
            repository: prepared.view.repository.clone(),
            parent: prepared.parent.clone(),
            name: prepared.name.clone(),
            parent_identity: Identity::read(&prepared.parent_handle)?,
            destination_identity: None,
            git_identity: None,
            phase: Phase::Planned,
        };
        let journal = Arc::new(Self {
            directory,
            path,
            record: Mutex::new(record.clone()),
        });
        journal.publish(&record, None)?;
        Ok(journal)
    }

    fn publish(&self, record: &Record, previous: Option<&Record>) -> Result<()> {
        let name = filename(&record.id)?;
        let body = encode(record)?;
        let temporary = format!("{}.{}.partial", record.id, uuid::Uuid::now_v7());
        let write = || -> Result<()> {
            let mut options = cap_std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = self.directory.open_with(&temporary, &options)?;
            file.write_all(&body)?;
            file.sync_all()?;
            if let Some(previous) = previous {
                if bytes(&self.directory, &name)? != encode(previous)? {
                    return Err(CloneError::message(
                        "The recovery record changed outside this clone.",
                    ));
                }
                self.directory.rename(&temporary, &self.directory, &name)?;
            } else {
                self.directory
                    .hard_link(&temporary, &self.directory, &name)?;
                self.directory.remove_file(&temporary)?;
            }
            sync(&self.directory)?;
            if bytes(&self.directory, &name)? != body {
                return Err(CloneError::message(
                    "The saved recovery record could not be verified.",
                ));
            }
            Ok(())
        };
        let result = write().map_err(|error| fail(&self.path, error));
        let _ = self.directory.remove_file(&temporary);
        result
    }

    fn change(&self, change: impl FnOnce(&mut Record)) -> Result<()> {
        let mut current = lock(&self.record);
        let mut next = current.clone();
        change(&mut next);
        self.publish(&next, Some(&current))?;
        *current = next;
        Ok(())
    }

    pub(super) fn created(&self, destination: &files::Destination) -> Result<()> {
        let identity = Identity::read(&destination.directory)?;
        self.change(|record| {
            record.phase = Phase::Created;
            record.destination_identity = Some(identity);
        })
    }

    pub(super) fn git_finished(&self, destination: &files::Destination) -> Result<()> {
        files::verify(destination).map_err(|error| fail(&self.path, error))?;
        let git = destination.directory.open_dir_nofollow(".git")?;
        let identity = Identity::read(&git)?;
        self.change(|record| {
            record.phase = Phase::GitFinished;
            record.git_identity = Some(identity);
        })
    }

    pub(super) fn finish(&self, state: CloneState) -> Result<()> {
        let phase = match state {
            CloneState::Ready => Phase::Ready,
            CloneState::Failed => Phase::Failed,
            CloneState::Canceled => Phase::Canceled,
            _ => return Ok(()),
        };
        self.change(|record| record.phase = phase)
    }
}

fn load(dir: &Dir, id: &str) -> Result<Record> {
    let record: Record = serde_json::from_slice(&bytes(dir, &filename(id)?)?).map_err(|_| {
        CloneError::message("The clone recovery record is unreadable or unsupported.")
    })?;
    let remote = validate::remote(&record.remote)
        .map_err(|_| CloneError::message("The saved repository address is invalid."))?;
    if record.version != 1
        || record.id != id
        || !record.parent.is_absolute()
        || validate::name(&record.name).is_err()
        || record.repository != remote.repository
        || record.remote != remote.url
        || (record.phase == Phase::Planned && record.destination_identity.is_some())
        || (matches!(
            record.phase,
            Phase::Created | Phase::GitFinished | Phase::Ready
        ) && record.destination_identity.is_none())
        || (matches!(record.phase, Phase::GitFinished | Phase::Ready)
            && record.git_identity.is_none())
    {
        return Err(CloneError::message(
            "The clone recovery record is inconsistent.",
        ));
    }
    Ok(record)
}

fn inspect(record: &Record) -> CloneRecovery {
    let destination = plain_path(&record.parent.join(&record.name))
        .to_string_lossy()
        .into_owned();
    let mut view = CloneRecovery {
        id: record.id.clone(),
        state: CloneState::Interrupted,
        repository: Some(record.repository.clone()),
        remote: Some(record.remote.clone()),
        destination: Some(destination),
        detail: "This clone was interrupted. Its folder has been kept as it is.".into(),
        can_add: false,
        can_open: false,
    };
    let checked = || -> Result<Option<Dir>> {
        // Canonical-parent and retained identity checks reject replaced parents and redirects.
        if std::fs::canonicalize(&record.parent)? != record.parent {
            return Err(CloneError::message("The parent path changed."));
        }
        let parent = Dir::open_ambient_dir(&record.parent, cap_std::ambient_authority())?;
        if Identity::read(&parent)? != record.parent_identity {
            return Err(CloneError::message("The parent folder was replaced."));
        }
        if files::absent(&parent, &record.name)? {
            return Ok(None);
        }
        let folder = parent.open_dir_nofollow(&record.name)?;
        if record.destination_identity.as_ref() != Some(&Identity::read(&folder)?) {
            return Err(CloneError::message(
                "The clone folder cannot be identified.",
            ));
        }
        Ok(Some(folder))
    };
    match checked() {
        Ok(None) => {
            view.state = CloneState::Failed;
            view.detail = "The clone folder is missing. No folder was changed. You can dismiss this record or retry elsewhere.".into();
        }
        Ok(Some(folder)) => {
            view.can_open = true;
            if matches!(record.phase, Phase::Ready | Phase::GitFinished)
                && folder
                    .open_dir_nofollow(".git")
                    .ok()
                    .and_then(|git| Identity::read(&git).ok())
                    .as_ref()
                    == record.git_identity.as_ref()
            {
                view.state = CloneState::Ready;
                view.can_add = true;
                view.detail =
                    "Git completed this clone. You can add the existing folder as a project."
                        .into();
            } else if record.phase == Phase::Failed || record.phase == Phase::Canceled {
                view.detail = "The clone stopped and its folder was retained. Review it before retrying in another folder.".into();
            }
        }
        Err(_) => {
            view.detail = "The destination or its parent changed, is linked, or is unavailable. It has been preserved. Inspect the displayed path yourself.".into();
        }
    }
    view
}

pub(super) fn list(root: &Path, active: &[String]) -> Result<Vec<CloneRecovery>> {
    let Some(dir) = directory(root, false)? else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for entry in dir.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
            continue;
        };
        if filename(id).is_err() || active.iter().any(|active| active == id) {
            continue;
        }
        let view = match load(&dir, id) {
            Ok(record) => inspect(&record),
            Err(_) => CloneRecovery {
                id: id.into(),
                state: CloneState::Interrupted,
                repository: None,
                remote: None,
                destination: None,
                can_add: false,
                can_open: false,
                detail: format!(
                    "The clone recovery record could not be read. It is preserved at {}.",
                    root.join("clones").join(name).display()
                ),
            },
        };
        records.push(view);
    }
    records.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(records)
}

pub(super) fn get(root: &Path, id: &str) -> Result<CloneRecovery> {
    let dir = directory(root, false)?
        .ok_or_else(|| CloneError::message("This recovery record is no longer available."))?;
    Ok(inspect(&load(&dir, id)?))
}

pub(super) fn dismiss(root: &Path, id: &str) -> Result<()> {
    let name = filename(id)?;
    let Some(dir) = directory(root, false)? else {
        return Ok(());
    };
    match dir.symlink_metadata(&name) {
        Ok(meta) if meta.is_file() => {
            dir.remove_file(&name)?;
            sync(&dir)?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(CloneError::message(
            "This recovery entry is not a regular record and has been preserved.",
        )),
    }
}
