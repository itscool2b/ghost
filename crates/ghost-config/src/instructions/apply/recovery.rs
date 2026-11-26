//! Restart recovery uses only journal-named files and exact reviewed states. Unknown bytes stay.
use super::*;

#[cfg(test)]
mod tests;

pub(crate) struct Pending {
    pub path: PathBuf,
    pub targets: Vec<PathBuf>,
    pub identity: Option<String>,
}

fn read_journal(path: &Path) -> Result<Journal> {
    let dir = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
    let bytes = safe_fs::read(&dir, "journal.json")?.ok_or(Error::Format)?;
    let journal: Journal = serde_json::from_slice(&bytes).map_err(|_| Error::Format)?;
    if !matches!(journal.version, 1 | 2)
        || uuid::Uuid::parse_str(&journal.id).is_err()
        || path.file_name().and_then(|name| name.to_str()) != Some(&journal.id)
        || journal.files.len() > safe_fs::FILE_COUNT
        || (journal.version == 2 && journal.canonical_roots.len() != journal.roots.len())
        || journal
            .roots
            .iter()
            .any(|root| !Path::new(root).is_absolute())
    {
        return Err(Error::Format);
    }
    for file in &journal.files {
        safe_fs::validate(&file.path)?;
        if file.root >= journal.roots.len() || (file.before.is_some() && file.link.is_some()) {
            return Err(Error::Format);
        }
        if let Some(from) = &file.rename_from {
            safe_fs::validate(from)?;
            if !from.eq_ignore_ascii_case(&file.path) || from == &file.path {
                return Err(Error::Format);
            }
        }
    }
    Ok(journal)
}

fn marked(path: &Path, name: &str) -> bool {
    Dir::open_ambient_dir(path, cap_std::ambient_authority())
        .ok()
        .and_then(|dir| safe_fs::read(&dir, name).ok().flatten())
        .as_deref()
        == Some(b"1")
}

pub(super) fn mark(path: &Path, name: &str) -> Result<()> {
    if marked(path, name) {
        return Ok(());
    }
    write_backup(path, name, b"1")
}

pub(super) fn prunable(path: &Path) -> bool {
    read_journal(path)
        .and_then(|journal| phase(path, &journal))
        .is_ok_and(|phase| phase.terminal)
}

struct Phase {
    ready: bool,
    committed: bool,
    terminal: bool,
}

/// Recovery and retention must agree about valid and terminal evidence. A malformed marker is
/// not an absent marker, and rollback and commit evidence can never coexist in version 2.
fn phase(path: &Path, journal: &Journal) -> Result<Phase> {
    let dir = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
    let marker = |name| match safe_fs::read(&dir, name)? {
        None => Ok(false),
        Some(bytes) if bytes == b"1" => Ok(true),
        Some(_) => Err(Error::Format),
    };
    let ready = marker("backups-ready")?;
    let committed = marker("files-committed")?;
    let metadata = marker("metadata-committed")?;
    let complete = marker("complete")?;
    let rolled_back = marker("rolled-back")?;
    let settled = marker("settled")?;
    if journal.version == 1 {
        // v1 complete did not record metadata intent. A recovery marker is cleared logically
        // only by verified rollback and the new settlement marker, never by guessing.
        if complete || committed || metadata {
            return Err(Error::Format);
        }
        return Ok(Phase {
            ready,
            committed: false,
            terminal: rolled_back && (!marker("recovery")? || settled),
        });
    }
    if journal.version != 2
        || (rolled_back && (committed || metadata || complete))
        || (committed && !ready)
        || (metadata && !committed)
        || (complete && !metadata)
        || (settled && !complete && !rolled_back)
    {
        return Err(Error::Format);
    }
    Ok(Phase {
        ready,
        committed,
        terminal: settled && (complete || rolled_back),
    })
}

fn folders(state: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(state.join("backups")) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    paths
}

// Even a malformed journal can contain usable target hints. These are used only to refuse an
// overlapping new write, never as authority to read or mutate a path.
fn target_hints(path: &Path) -> Vec<PathBuf> {
    let value = Dir::open_ambient_dir(path, cap_std::ambient_authority())
        .ok()
        .and_then(|dir| safe_fs::read(&dir, "journal.json").ok().flatten())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(roots) = value["roots"].as_array() else {
        return Vec::new();
    };
    let Some(files) = value["files"].as_array() else {
        return Vec::new();
    };
    files
        .iter()
        .flat_map(|file| {
            let root = file["root"]
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| roots.get(index))
                .and_then(|root| root.as_str());
            ["path", "rename_from"].into_iter().filter_map(move |key| {
                let root = Path::new(root?);
                let name = file[key].as_str()?;
                (root.is_absolute() && safe_fs::validate(name).is_ok()).then(|| root.join(name))
            })
        })
        .collect()
}

fn identity_hint(path: &Path) -> Option<String> {
    let dir = Dir::open_ambient_dir(path, cap_std::ambient_authority()).ok()?;
    let bytes = safe_fs::read(&dir, "journal.json").ok()??;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let identity = value["metadata"]["identity"].as_str()?;
    // This is only an exact map-key hint for refusing overlap, never filesystem authority.
    (identity.len() <= 32_768
        && !identity.contains('\0')
        && (identity == "user" || Path::new(identity).is_absolute()))
    .then(|| identity.to_owned())
}

/// Read-only startup/UI notice. This never opens an agent root or changes a file.
pub(crate) fn notices(state: &Path) -> Vec<String> {
    folders(state).into_iter().filter_map(|path| {
        if marked(&path, "recovered") && prunable(&path) {
            Some(format!("An interrupted Instructions change was recovered. Its backup is kept at {}.", path.display()))
        } else if !prunable(&path) {
            Some(format!("An interrupted Instructions change needs recovery. Unrecognized files will be preserved. Its journal and backups are kept at {}.", path.display()))
        } else { None }
    }).collect()
}

pub(crate) fn recover(state: &Path) -> Vec<Pending> {
    folders(state)
        .into_iter()
        .filter_map(|path| {
            if prunable(&path) {
                return None;
            }
            let journal = read_journal(&path);
            if let Ok(journal) = &journal {
                // Mark before resolving, so a crash after settlement still leaves the outcome notice.
                if phase(&path, journal).is_ok()
                    && mark(&path, "recovered").is_ok()
                    && resolve(&path, journal).is_ok()
                    && prunable(&path)
                {
                    return None;
                }
            }
            let targets = journal
                .as_ref()
                .map(|journal| {
                    journal
                        .files
                        .iter()
                        .flat_map(|file| {
                            let root = PathBuf::from(&journal.roots[file.root]);
                            std::iter::once(root.join(&file.path))
                                .chain(file.rename_from.as_ref().map(|from| root.join(from)))
                        })
                        .collect()
                })
                .unwrap_or_else(|_| target_hints(&path));
            let identity = journal
                .ok()
                .and_then(|journal| journal.metadata.map(|intent| intent.identity))
                .or_else(|| identity_hint(&path));
            Some(Pending {
                path,
                targets,
                identity,
            })
        })
        .collect()
}

pub(crate) fn overlaps(left: &Path, right: &Path) -> bool {
    // Conservative on case-sensitive volumes too: a case-only rename owns both spellings.
    let key = |path: &Path| {
        let resolved = path
            .parent()
            .and_then(|parent| std::fs::canonicalize(parent).ok())
            .zip(path.file_name())
            .map(|(parent, name)| parent.join(name))
            .unwrap_or_else(|| path.to_path_buf());
        resolved.to_string_lossy().replace('\\', "/").to_lowercase()
    };
    key(left) == key(right)
}

fn matches(value: &Expect, hash: Option<&String>, link: Option<&String>) -> bool {
    match value {
        Expect::Missing => hash.is_none() && link.is_none(),
        Expect::File(bytes) => {
            link.is_none() && hash.is_some_and(|hash| safe_fs::hash(bytes) == *hash)
        }
        Expect::Link(target) => {
            hash.is_none() && link.is_some_and(|link| target == Path::new(link))
        }
    }
}

fn cleanup(dir: &Dir, name: &str, hash: Option<&String>, link: Option<&String>) -> Result<()> {
    let value = current(dir, name)?;
    if value == Expect::Missing {
        return Ok(());
    }
    if !matches(&value, hash, link) {
        return Err(Error::Stale);
    }
    dir.remove_file(name)?;
    Ok(())
}

pub(super) fn resolve(path: &Path, journal: &Journal) -> Result<()> {
    let phase = phase(path, journal)?;
    let committed = phase.committed;
    let ready = phase.ready;
    let backup_dir = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
    for (index, file) in journal.files.iter().enumerate().rev() {
        if journal.version == 2 {
            let canonical = std::fs::canonicalize(&journal.roots[file.root])?;
            if journal.canonical_roots[file.root].as_deref() != canonical.to_str() {
                return Err(Error::Stale);
            }
        }
        let root = open_root(Path::new(&journal.roots[file.root]))?.ok_or(Error::Unavailable)?;
        let (parent, name) = match parent(&root, &file.path, false) {
            Ok(value) => value,
            Err(_)
                if !committed
                    && file.before.is_none()
                    && file.link.is_none()
                    && root
                        .symlink_metadata(&file.path)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let now = current(&parent, &name)?;
        let before = |value: &Expect| matches(value, file.before.as_ref(), file.link.as_ref());
        let after = |value: &Expect| matches(value, file.after.as_ref(), None);
        if ready && let Some(hash) = &file.before {
            let bytes =
                safe_fs::read(&backup_dir, &format!("before-{index}"))?.ok_or(Error::Format)?;
            if safe_fs::hash(&bytes) != *hash {
                return Err(Error::Format);
            }
        }
        if let Some(from) = &file.rename_from {
            let (from_parent, from_name) = super::parent(&root, from, false)?;
            let names = safe_fs::children(&parent, "")?;
            let target_spelling = names.iter().any(|value| value == &name);
            let source_spelling = names.iter().any(|value| value == &from_name);
            if journal.version == 2 && !ready && target_spelling {
                return Err(Error::Stale);
            }
            if committed {
                // Rename mutations carry no replacement bytes; the original content stays.
                if !target_spelling || source_spelling || !before(&now) {
                    return Err(Error::Stale);
                }
            } else if target_spelling && !source_spelling && before(&now) {
                parent.rename(&name, &from_parent, &from_name)?;
            } else if !source_spelling
                || target_spelling
                || !before(&current(&from_parent, &from_name)?)
            {
                return Err(Error::Stale);
            }
            continue;
        }
        let old_name = temp_name(&name, &journal.id, "old");
        let tmp_name = temp_name(&name, &journal.id, "tmp");
        let old = current(&parent, &old_name)?;
        let temp = current(&parent, &tmp_name)?;
        if journal.version == 2
            && !ready
            && (!before(&now) || old != Expect::Missing || temp != Expect::Missing)
        {
            return Err(Error::Stale);
        }
        // Validate all siblings before changing anything for this file.
        if (old != Expect::Missing && !before(&old)) || (temp != Expect::Missing && !after(&temp)) {
            return Err(Error::Stale);
        }
        if committed {
            if !after(&now) {
                return Err(Error::Stale);
            }
        } else if !before(&now) {
            if now != Expect::Missing && !after(&now) {
                return Err(Error::Stale);
            }
            // A missing original without its staged sibling is ambiguous. Preserve it.
            if (file.link.is_some() || file.before.is_some()) && old == Expect::Missing {
                return Err(Error::Stale);
            }
            if journal.version == 1
                && file.before.is_none()
                && file.link.is_none()
                && temp == Expect::Missing
            {
                return Err(Error::Stale);
            }
            let bytes = if let Some(hash) = &file.before {
                let bytes =
                    safe_fs::read(&backup_dir, &format!("before-{index}"))?.ok_or(Error::Format)?;
                if safe_fs::hash(&bytes) != *hash {
                    return Err(Error::Format);
                }
                Some(bytes)
            } else {
                None
            };
            if now != Expect::Missing {
                parent.remove_file(&name)?;
            }
            if old != Expect::Missing {
                restore(&parent, &old_name, &name, &old)?;
            } else if let Some(bytes) = bytes {
                write_new(&parent, &name, &bytes)?;
            }
        }
        cleanup(&parent, &old_name, file.before.as_ref(), file.link.as_ref())?;
        cleanup(&parent, &tmp_name, file.after.as_ref(), None)?;
    }
    if committed {
        if let Some(intent) = &journal.metadata {
            let state = path
                .parent()
                .and_then(Path::parent)
                .ok_or(Error::UnsafePath)?;
            let mut record = super::super::manifest::Manifest::load_checked(state)?;
            let existing = record.roots.get(&intent.identity);
            if existing != intent.after.as_ref() {
                if existing != intent.before.as_ref() {
                    return Err(Error::Stale);
                }
                match &intent.after {
                    Some(entry) => {
                        record.roots.insert(intent.identity.clone(), entry.clone());
                    }
                    None => {
                        record.roots.remove(&intent.identity);
                    }
                }
                record.save(state)?;
            }
        }
        mark(path, "metadata-committed")?;
        mark(path, "complete")?;
    } else {
        mark(path, "rolled-back")?;
    }
    mark(path, "settled")?;
    if self::phase(path, journal)?.terminal {
        Ok(())
    } else {
        Err(Error::Format)
    }
}
