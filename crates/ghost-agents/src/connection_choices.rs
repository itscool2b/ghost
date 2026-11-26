//! App-owned per-device Connection settings: the connection new sessions of an agent use. An agent
//! without a saved choice is Automatic. The file names catalog ids only, never paths, accounts or
//! credentials, and lives beside the executable choices in the data root.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

/// Agent id to connection id.
pub(crate) type Choices = BTreeMap<String, String>;

/// The file's name in the data root.
pub(crate) const FILE: &str = "agent-connections.json";

const MAX_BYTES: u64 = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    choices: Choices,
}

/// The saved choices. A missing file is Automatic for every agent.
pub(crate) fn read(path: &Path) -> Result<Choices, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Choices::new()),
        Err(_) => return Err("The saved connection choices could not be read.".into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The saved connection choices could not be read.")?;
    let saved: Saved = serde_json::from_slice(&bytes)
        .map_err(|_| "The saved connection choices are invalid. Every agent uses Automatic.")?;
    if bytes.len() as u64 > MAX_BYTES
        || saved.version != 1
        || saved.choices.len() > 64
        || saved
            .choices
            .iter()
            .any(|(agent, connection)| !valid(agent) || !valid(connection))
    {
        return Err("The saved connection choices have an unsupported format.".into());
    }
    Ok(saved.choices)
}

fn valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

/// A file written by a newer build (a later `version`), which this build must not replace.
fn newer(path: &Path) -> bool {
    std::fs::read(path)
        .ok()
        .filter(|bytes| bytes.len() as u64 <= MAX_BYTES)
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.get("version").and_then(serde_json::Value::as_u64))
        .is_some_and(|version| version > 1)
}

/// Saves `agent`'s choice (`None`: Automatic) and returns every saved choice. A damaged file holds
/// nothing but these choices, so it is replaced; one from a newer build is left unchanged.
pub(crate) fn save(path: &Path, agent: &str, connection: Option<&str>) -> Result<Choices, String> {
    if !valid(agent) || connection.is_some_and(|connection| !valid(connection)) {
        return Err("The connection choice is invalid.".into());
    }
    let parent = path
        .parent()
        .ok_or("The connection preferences folder is unavailable.")?;
    std::fs::create_dir_all(parent)
        .map_err(|_| "The connection preferences folder could not be created.")?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))
        .map_err(|_| "The connection preference lock could not be opened.")?;
    lock.try_lock()
        .map_err(|_| "Another Ghost window is changing connection choices. Try again.")?;
    let _lock = ghost_process::locked_file::LockedFile::new(lock);
    let mut choices = match read(path) {
        Ok(choices) => choices,
        Err(_) if newer(path) => {
            return Err(
                "The connection choices were saved by a newer Ghost. They were left unchanged."
                    .into(),
            );
        }
        Err(_) => Choices::new(),
    };
    match connection {
        Some(connection) => {
            choices.insert(agent.into(), connection.into());
        }
        None => {
            choices.remove(agent);
        }
    }
    let bytes = serde_json::to_vec(&Saved {
        version: 1,
        choices: choices.clone(),
    })
    .map_err(|_| "The connection preferences could not be encoded.")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("There are too many saved connection choices.".into());
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "The connection preferences could not be saved.")?;
    temp.write_all(&bytes)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|_| "The connection preferences could not be saved.")?;
    temp.persist(path)
        .map_err(|_| "The connection preferences could not be replaced.")?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|_| "The connection preference directory could not be saved.")?;
    Ok(choices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_round_trip_keep_other_agents_and_replace_an_unreadable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert!(read(&path).unwrap().is_empty(), "no file is Automatic");
        save(&path, "claude", Some("claude-acp")).unwrap();
        save(&path, "codex", Some("codex")).unwrap();
        let saved = read(&path).unwrap();
        assert_eq!(saved.get("claude").map(String::as_str), Some("claude-acp"));
        assert_eq!(saved.get("codex").map(String::as_str), Some("codex"));
        save(&path, "claude", None).unwrap();
        assert_eq!(read(&path).unwrap().len(), 1);
        // A damaged file reads as an error (every agent Automatic) and the next choice replaces it.
        std::fs::write(&path, "broken").unwrap();
        assert!(read(&path).is_err());
        save(&path, "claude", Some("claude-acp")).unwrap();
        assert_eq!(read(&path).unwrap().len(), 1);
        // Paths, separators and empty ids are never saved.
        assert!(save(&path, "claude", Some("../x")).is_err());
        assert!(save(&path, "", None).is_err());
        let newer = r#"{"version":2,"choices":{},"later":true}"#;
        std::fs::write(&path, newer).unwrap();
        assert!(read(&path).is_err(), "a newer format is not guessed at");
        assert!(save(&path, "claude", None).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            newer,
            "left unchanged"
        );
    }
}
