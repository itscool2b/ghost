//! App-owned per-device executable selections. This file contains paths, never vendor credentials.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub(crate) type Selections = BTreeMap<String, PathBuf>;
const MAX_BYTES: u64 = 64 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    selections: Selections,
}

pub(crate) fn read(path: &Path) -> Result<Selections, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Selections::new()),
        Err(_) => {
            return Err(
                "The saved executable choices could not be read. No default was substituted."
                    .into(),
            );
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The saved executable choices could not be read.")?;
    let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| "The saved executable choices are invalid. Restore this Ghost preference file before choosing an installation.")?;
    if bytes.len() as u64 > MAX_BYTES
        || saved.version != 1
        || saved.selections.len() > 64
        || saved.selections.iter().any(|(id, path)| !valid(id, path))
    {
        return Err("The saved executable choices have an unsupported format.".into());
    }
    Ok(saved.selections)
}

fn valid(id: &str, path: &Path) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        && path.is_absolute()
        && path
            .to_str()
            .is_some_and(|p| p.len() <= 8192 && !p.chars().any(char::is_control))
}

pub(crate) fn save(
    path: &Path,
    provider: &str,
    selected: Option<&Path>,
) -> Result<Selections, String> {
    if selected.is_some_and(|path| !valid(provider, path)) {
        return Err("The executable choice is invalid.".into());
    }
    let parent = path
        .parent()
        .ok_or("The executable preferences folder is unavailable.")?;
    std::fs::create_dir_all(parent)
        .map_err(|_| "The executable preferences folder could not be created.")?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))
        .map_err(|_| "The executable preference lock could not be opened.")?;
    lock.try_lock()
        .map_err(|_| "Another Ghost window is changing executable choices. Try again.")?;
    let _lock = ghost_process::locked_file::LockedFile::new(lock);
    let mut selections = read(path)?;
    match selected {
        Some(value) => {
            selections.insert(provider.into(), value.to_owned());
        }
        None => {
            selections.remove(provider);
        }
    }
    let bytes = serde_json::to_vec(&Saved {
        version: 1,
        selections: selections.clone(),
    })
    .map_err(|_| "The executable preferences could not be encoded.")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("There are too many saved executable choices.".into());
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "The executable preferences could not be saved.")?;
    temp.write_all(&bytes)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|_| "The executable preferences could not be saved.")?;
    temp.persist(path)
        .map_err(|_| "The executable preferences could not be replaced.")?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|_| "The executable preference directory could not be saved.")?;
    Ok(selections)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn choices_round_trip_keep_other_providers_and_refuse_corrupt_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("choices.json");
        let a = dir.path().join("claude.exe");
        let b = dir.path().join("codex.exe");
        save(&path, "claude", Some(&a)).unwrap();
        save(&path, "codex", Some(&b)).unwrap();
        assert_eq!(read(&path).unwrap().get("claude"), Some(&a));
        save(&path, "claude", None).unwrap();
        assert_eq!(read(&path).unwrap().len(), 1);
        std::fs::write(&path, "broken").unwrap();
        assert!(save(&path, "claude", None).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken");
    }
}
