//! The one folder a clone owns. It is created atomically, so an existing folder is never reused,
//! and it is removed only while it is still that same folder.
use super::*;
use cap_fs_ext::DirExt;

pub(super) struct Destination {
    pub(super) prepared: Prepared,
    pub(super) directory: Dir,
}

/// Whether `name` is free in `parent`. A link or file of that name counts as taken.
pub(super) fn absent(parent: &Dir, name: &str) -> std::io::Result<bool> {
    match parent.symlink_metadata(name) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
        Ok(_) => Ok(false),
    }
}

pub(super) fn matches(directory: &Dir, path: &Path) -> Result<()> {
    let retained = same_file::Handle::from_file(directory.try_clone()?.into_std_file())?;
    let current = same_file::Handle::from_file(
        Dir::open_ambient_dir(path, cap_std::ambient_authority())?.into_std_file(),
    )?;
    if retained != current {
        return Err(CloneError::message(
            "The chosen folder was moved or replaced. Choose it again.",
        ));
    }
    Ok(())
}

/// Creates the destination. An existing path of that name, even an empty folder, is refused.
pub(super) fn begin(prepared: Prepared) -> Result<Destination> {
    matches(&prepared.parent_handle, &prepared.parent)?;
    match prepared.parent_handle.create_dir(&prepared.name) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(CloneError::Message(format!(
                "{} already has a {} folder. Choose another name.",
                prepared.view.parent, prepared.name
            )));
        }
        Err(error) => return Err(error.into()),
    }
    match prepared.parent_handle.open_dir_nofollow(&prepared.name) {
        Ok(directory) => Ok(Destination {
            prepared,
            directory,
        }),
        Err(error) => {
            // Only an empty folder is removed this way; anything put in it stays.
            let _ = prepared.parent_handle.remove_dir(&prepared.name);
            Err(error.into())
        }
    }
}

/// Removes the unfinished folder this clone created, and nothing else: the parent must still be
/// the chosen folder and the name must still be the folder created at start. Returns whether it
/// was removed.
pub(super) fn remove(destination: Destination) -> bool {
    let Destination {
        prepared,
        directory,
    } = destination;
    if matches(&prepared.parent_handle, &prepared.parent).is_err() {
        return false;
    }
    let same = (|| -> Result<bool> {
        let retained = same_file::Handle::from_file(directory.try_clone()?.into_std_file())?;
        let current = same_file::Handle::from_file(
            prepared
                .parent_handle
                .open_dir_nofollow(&prepared.name)?
                .into_std_file(),
        )?;
        Ok(retained == current)
    })();
    // Windows keeps a folder while a handle to it is open.
    drop(directory);
    if !matches!(same, Ok(true)) {
        return false;
    }
    // std's removal never follows links out of the folder and clears Git's read-only pack files.
    std::fs::remove_dir_all(prepared.parent.join(&prepared.name)).is_ok()
}

/// Git finished: the folder must still be ours and hold a repository.
pub(super) fn verify(destination: &Destination) -> Result<()> {
    matches(
        &destination.prepared.parent_handle,
        &destination.prepared.parent,
    )?;
    let current = destination
        .prepared
        .parent_handle
        .open_dir_nofollow(&destination.prepared.name)?;
    let retained =
        same_file::Handle::from_file(destination.directory.try_clone()?.into_std_file())?;
    if retained != same_file::Handle::from_file(current.into_std_file())? {
        return Err(CloneError::message(
            "The new folder was moved or replaced while Git was cloning.",
        ));
    }
    destination.directory.open_dir_nofollow(".git")?;
    Ok(())
}
