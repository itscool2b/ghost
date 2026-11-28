//! A checked zip archive, unpacked so that nothing lands outside its folder. Moved here from
//! ghost-devices, where it unpacks Android's SDK packages, so that every tool Ghost fetches is
//! unpacked by the same code.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

/// Why an archive was not unpacked. The sentences are for the person as they are.
#[derive(Debug, thiserror::Error)]
pub enum UnzipError {
    /// The archive is not what an honest one looks like.
    #[error("{0}")]
    Archive(&'static str),
    /// Unpacked, it would pass the limit the caller set.
    #[error("The downloaded archive is larger than Ghost accepts.")]
    TooLarge,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

const BAD: &str = "The downloaded archive could not be unpacked.";
const PATH_LEAVES: &str = "The downloaded archive holds a path that leaves its folder.";
const LINK_LEAVES: &str = "The downloaded archive holds a link that leaves its folder.";
const LINK_IN_LINK: &str = "The downloaded archive holds a link inside a link.";

/// Unpacks a checked archive into the empty folder `into`, writing at most `limit` bytes. No
/// entry may land outside it: not by its own path, and not through a link.
///
/// Folders and files are written first, while no link exists, so no write can pass through one.
/// Links are made afterwards, each beside real folders only, and every one is then resolved for
/// real: a link that ends up anywhere outside `into` fails the whole unpack, and one that points
/// at nothing is dropped.
///
/// It blocks: call it from a thread that may.
pub fn unzip(archive: &Path, into: &Path, limit: u64) -> Result<(), UnzipError> {
    let bad = |_| UnzipError::Archive(BAD);
    std::fs::create_dir_all(into)?;
    let file = std::io::BufReader::new(std::fs::File::open(archive)?);
    let mut zip = zip::ZipArchive::new(file).map_err(bad)?;
    let mut written = 0u64;
    let mut links: Vec<(PathBuf, String)> = Vec::new();
    for number in 0..zip.len() {
        let mut entry = zip.by_index(number).map_err(bad)?;
        let relative = entry
            .enclosed_name()
            .ok_or(UnzipError::Archive(PATH_LEAVES))?;
        let target = into.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if entry.is_symlink() {
            let mut link = String::new();
            entry
                .by_ref()
                .take(4096)
                .read_to_string(&mut link)
                .map_err(|_| UnzipError::Archive(BAD))?;
            links.push((relative, link));
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // The size an entry declares is only a claim: what is really written is what counts,
        // against the entry's own claim and against the limit for the whole archive.
        let declared = entry.size();
        let room = limit.saturating_sub(written);
        if declared > room {
            return Err(UnzipError::TooLarge);
        }
        let mode = entry.unix_mode();
        // `create_new`: a second entry of the same name never overwrites the first.
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        let copied = std::io::copy(
            &mut entry.by_ref().take(declared.saturating_add(1)),
            &mut out,
        )?;
        written = written.saturating_add(copied);
        if copied != declared {
            return Err(UnzipError::Archive(BAD));
        }
        out.flush()?;
        drop(out);
        make_runnable(&target, mode)?;
    }
    make_links(into, &links)
}

/// Makes the archive's links, then checks where each really leads.
fn make_links(into: &Path, links: &[(PathBuf, String)]) -> Result<(), UnzipError> {
    if links.is_empty() {
        return Ok(());
    }
    let mut made = Vec::new();
    for (relative, link) in links {
        check_link(relative, link)?;
        let target = into.join(relative);
        real_folders_only(into, relative)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if create_link(Path::new(link), &target)? {
            made.push(target);
        }
    }
    let top = std::fs::canonicalize(into)?;
    for link in made {
        match std::fs::canonicalize(&link) {
            Ok(resolved) if resolved.starts_with(&top) => {}
            Ok(_) => return Err(UnzipError::Archive(LINK_LEAVES)),
            // It points at nothing: of no use, so it does not stay.
            Err(_) => {
                let _ = std::fs::remove_file(&link);
            }
        }
    }
    Ok(())
}

/// Every folder above `relative` must be a real folder, not a link made a moment ago: a link
/// placed "inside" another link would really be placed wherever that one leads.
fn real_folders_only(into: &Path, relative: &Path) -> Result<(), UnzipError> {
    let mut path = into.to_owned();
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    for part in parent.components() {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(found) if found.file_type().is_symlink() => {
                return Err(UnzipError::Archive(LINK_IN_LINK));
            }
            _ => {}
        }
    }
    Ok(())
}

/// A link's text must be relative and, read as plain path parts from the entry's own folder,
/// must not climb above the archive's top. Where it really leads is checked after it is made.
fn check_link(relative: &Path, link: &str) -> Result<(), UnzipError> {
    let escapes = || UnzipError::Archive(LINK_LEAVES);
    let link_path = Path::new(link);
    if link.is_empty() || link_path.is_absolute() {
        return Err(escapes());
    }
    let mut depth = relative
        .parent()
        .map_or(0, |parent| parent.components().count()) as i64;
    for part in link_path.components() {
        match part {
            Component::Normal(_) => depth += 1,
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(escapes());
                }
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return Err(escapes()),
        }
    }
    Ok(())
}

/// Makes the link, and says whether one was made. Windows archives hold no links; one there is
/// left out.
#[cfg(unix)]
fn create_link(link: &Path, target: &Path) -> Result<bool, UnzipError> {
    std::os::unix::fs::symlink(link, target)?;
    Ok(true)
}

#[cfg(not(unix))]
fn create_link(_link: &Path, _target: &Path) -> Result<bool, UnzipError> {
    Ok(false)
}

#[cfg(unix)]
fn make_runnable(target: &Path, mode: Option<u32>) -> Result<(), UnzipError> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        // The owner can always read and write what Ghost fetched; nothing is set-id.
        let mode = (mode & 0o777) | 0o600;
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_runnable(_target: &Path, _mode: Option<u32>) -> Result<(), UnzipError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn archive(folder: &Path, build: impl FnOnce(&mut zip::ZipWriter<std::fs::File>)) -> PathBuf {
        let path = folder.join("archive.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        build(&mut writer);
        writer.finish().unwrap();
        path
    }

    #[test]
    fn files_and_folders_land_inside_and_nowhere_else() {
        let folder = tempfile::tempdir().unwrap();
        let zip = archive(folder.path(), |writer| {
            writer
                .add_directory("tool/", SimpleFileOptions::default())
                .unwrap();
            writer
                .start_file(
                    "tool/program",
                    SimpleFileOptions::default().unix_permissions(0o755),
                )
                .unwrap();
            writer.write_all(b"#!run").unwrap();
            writer
                .start_file("tool/notes/readme.txt", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"hello").unwrap();
        });
        let into = folder.path().join("out");
        unzip(&zip, &into, 1 << 20).unwrap();
        assert_eq!(std::fs::read(into.join("tool/program")).unwrap(), b"#!run");
        assert_eq!(
            std::fs::read(into.join("tool/notes/readme.txt")).unwrap(),
            b"hello"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(into.join("tool/program"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn more_than_the_limit_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let zip = archive(folder.path(), |writer| {
            writer
                .start_file("big", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&[7u8; 4096]).unwrap();
        });
        let refused = unzip(&zip, &folder.path().join("out"), 4095);
        assert!(matches!(refused, Err(UnzipError::TooLarge)), "{refused:?}");
        unzip(&zip, &folder.path().join("fits"), 4096).unwrap();
    }

    #[test]
    fn a_second_entry_of_the_same_name_never_overwrites_the_first() {
        let folder = tempfile::tempdir().unwrap();
        let zip = archive(folder.path(), |writer| {
            writer
                .start_file("same", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"first").unwrap();
            writer
                .start_file("./same", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"second").unwrap();
        });
        let into = folder.path().join("out");
        assert!(unzip(&zip, &into, 1 << 20).is_err());
        assert_eq!(std::fs::read(into.join("same")).unwrap(), b"first");
    }

    #[test]
    fn something_that_is_not_an_archive_says_so() {
        let folder = tempfile::tempdir().unwrap();
        let not = folder.path().join("not.zip");
        std::fs::write(&not, b"this is not a zip file").unwrap();
        let refused = unzip(&not, &folder.path().join("out"), 1 << 20).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "The downloaded archive could not be unpacked."
        );
    }

    #[test]
    fn a_links_text_may_not_climb_out_of_the_archive() {
        let inside = Path::new("tool/bin/link");
        assert!(check_link(inside, "../lib/real").is_ok());
        assert!(check_link(inside, "./beside").is_ok());
        assert!(check_link(inside, "../../top").is_ok());
        for escapes in ["../../../out", "/etc/passwd", ""] {
            assert!(check_link(inside, escapes).is_err(), "{escapes:?}");
        }
        assert!(check_link(Path::new("top-link"), "../out").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_that_leaves_the_folder_fails_the_whole_unpack() {
        let folder = tempfile::tempdir().unwrap();
        let zip = archive(folder.path(), |writer| {
            writer
                .start_file("real", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"x").unwrap();
            writer
                .add_symlink("good", "real", SimpleFileOptions::default())
                .unwrap();
            writer
                .add_symlink("dangling", "nothing-here", SimpleFileOptions::default())
                .unwrap();
        });
        let into = folder.path().join("out");
        unzip(&zip, &into, 1 << 20).unwrap();
        assert_eq!(std::fs::read(into.join("good")).unwrap(), b"x");
        assert!(
            std::fs::symlink_metadata(into.join("dangling")).is_err(),
            "a link to nothing does not stay"
        );

        let escaping = archive(folder.path(), |writer| {
            writer
                .add_symlink("out", "../", SimpleFileOptions::default())
                .unwrap();
        });
        let refused = unzip(&escaping, &folder.path().join("out2"), 1 << 20).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "The downloaded archive holds a link that leaves its folder."
        );
    }
}
