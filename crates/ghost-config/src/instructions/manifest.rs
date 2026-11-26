//! Which files Ghost keeps as copies, so a hand edit to one can be told from a stale one.
//! The record lives in the app's own data folder, never in a project or an agent folder.

use std::{collections::BTreeMap, io::Write, path::Path};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const NAME: &str = "managed.json";
const ROOT_LIMIT: usize = 4096;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub version: u32,
    /// Keyed by the scope's identity: a canonical project path, or "user".
    pub roots: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    /// The universal source, as a file key.
    pub source: String,
    /// The source's content hash when the copies were last written.
    pub source_hash: String,
    /// Copy key to the hash Ghost last wrote there.
    pub copies: BTreeMap<String, String>,
}

impl Manifest {
    /// Recovery must distinguish an absent record from a damaged one.
    pub fn load_checked(dir: &Path) -> Result<Manifest> {
        let root = match cap_std::fs::Dir::open_ambient_dir(dir, cap_std::ambient_authority()) {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(_) => return Err(Error::Io),
        };
        let Some(bytes) = crate::safe_fs::read(&root, NAME)? else {
            return Ok(Self::default());
        };
        let record: Self = serde_json::from_slice(&bytes).map_err(|_| Error::Format)?;
        if record.version != 1 || record.roots.len() > ROOT_LIMIT {
            return Err(Error::Format);
        }
        Ok(record)
    }
    pub fn load(dir: &Path) -> Manifest {
        // A missing or unreadable record only loses drift detection; identical copies still read
        // as copies, so nothing is ever overwritten because of it.
        std::fs::read(dir.join(NAME))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
            .filter(|manifest| manifest.version == 1 && manifest.roots.len() <= ROOT_LIMIT)
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut copy = self.clone();
        copy.version = 1;
        let bytes = serde_json::to_vec_pretty(&copy).map_err(|_| Error::Format)?;
        let temp = dir.join(format!("{NAME}.{}.tmp", uuid::Uuid::now_v7()));
        {
            let mut file = std::fs::File::create_new(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&temp, dir.join(NAME)).inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_round_trips_and_a_damaged_one_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::load(dir.path()), Manifest::default());
        let mut manifest = Manifest::default();
        manifest.roots.insert(
            "user".into(),
            Entry {
                source: "codex:AGENTS.md".into(),
                source_hash: "a".repeat(64),
                copies: [("pi:AGENTS.md".to_owned(), "a".repeat(64))].into(),
            },
        );
        manifest.save(dir.path()).unwrap();
        let loaded = Manifest::load(dir.path());
        assert_eq!(loaded.roots, manifest.roots);
        std::fs::write(dir.path().join(NAME), b"{not json").unwrap();
        assert_eq!(Manifest::load(dir.path()), Manifest::default());
    }
}
