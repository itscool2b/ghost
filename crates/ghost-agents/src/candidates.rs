//! Bounded executable discovery and metadata observation. Never walks agent configuration folders.
use crate::{AgentSpec, Recipe};
use ghost_process::{EnvKey, EnvMap};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(crate) const MAX_CANDIDATES: usize = 8;
const MAX_DIRS: usize = 256;

#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub path: PathBuf,
    pub source: &'static str,
}

pub(crate) fn paths(spec: &AgentSpec, env: &EnvMap, windows: bool) -> (Vec<Candidate>, bool) {
    let (program, variable, direct) = match &spec.recipe {
        Recipe::Builtin => return (Vec::new(), false),
        Recipe::NativeBinary { user_binary, .. } | Recipe::NpmAdapter { user_binary, .. } => {
            (user_binary.program, Some(user_binary.env), true)
        }
        Recipe::Binary { program, .. } | Recipe::QualifiedBinary { program, .. } => {
            (*program, None, false)
        }
    };
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |path: PathBuf, source| {
        // Keep the stable entry, including symlinks. Distinct entries may share a target today but
        // change independently at the next update. Only identical paths are duplicates.
        if seen.insert(key(&path, windows)) {
            result.push(Candidate { path, source });
        }
    };
    if let Some(variable) = variable
        && let Some(path) = env.get(&EnvKey::from(variable)).map(PathBuf::from)
        && path.is_absolute()
        && crate::probe::runnable(&path, windows)
    {
        push(path, "Environment override");
    }
    let dirs = directories(env, windows);
    let mut truncated = dirs.len() > MAX_DIRS;
    let extensions: Vec<String> = if !windows {
        vec![String::new()]
    } else if direct {
        vec![".exe".into()]
    } else {
        env.get(&EnvKey::from("PATHEXT"))
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|ext| {
                [".com", ".exe", ".bat", ".cmd", ".ps1"]
                    .iter()
                    .any(|known| ext.eq_ignore_ascii_case(known))
            })
            .map(str::to_ascii_lowercase)
            .collect()
    };
    for dir in dirs.into_iter().take(MAX_DIRS) {
        for extension in &extensions {
            let path = dir.join(format!("{program}{extension}"));
            if if windows {
                path.is_file()
            } else {
                crate::probe::runnable(&path, false)
            } {
                push(path, "PATH");
            }
        }
    }
    // Native transports cannot execute Windows shell shims. Show the first shim as a broken
    // candidate only when no executable exists, preserving their established fallback behavior.
    if result.is_empty() && windows && direct {
        for dir in directories(env, true).into_iter().take(MAX_DIRS) {
            for extension in ["cmd", "bat", "ps1"] {
                let path = dir.join(format!("{program}.{extension}"));
                if path.is_file() {
                    result.push(Candidate {
                        path,
                        source: "Unsupported shell shim",
                    });
                    break;
                }
            }
            if !result.is_empty() {
                break;
            }
        }
    }
    truncated |= result.len() > MAX_CANDIDATES;
    result.truncate(MAX_CANDIDATES);
    (result, truncated)
}

pub(crate) fn directories(env: &EnvMap, windows: bool) -> Vec<PathBuf> {
    if windows {
        return crate::probe::windows_path_dirs(env);
    }
    env.get(&EnvKey::from("PATH"))
        .map(|value| {
            std::env::split_paths(value)
                .filter(|p| p.is_absolute())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn key(path: &Path, windows: bool) -> String {
    let text = path.to_string_lossy();
    if windows {
        text.replace('/', "\\").to_ascii_lowercase()
    } else {
        text.into_owned()
    }
}

/// Detect additions/removals, in-place updates, symlink retargeting and reviewed npm bin changes.
/// Only file metadata and package manifests already used by launch qualification are inspected.
pub(crate) fn fingerprint(
    specs: &[AgentSpec],
    env: &EnvMap,
    selected: &std::collections::BTreeMap<String, PathBuf>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let mut observe = |path: &Path| {
        path.hash(&mut hash);
        std::fs::canonicalize(path).ok().hash(&mut hash);
        for meta in [std::fs::symlink_metadata(path), std::fs::metadata(path)] {
            meta.ok()
                .map(|m| (m.len(), m.modified().ok(), m.permissions().readonly()))
                .hash(&mut hash);
        }
    };
    for dir in directories(env, cfg!(windows)).into_iter().take(MAX_DIRS) {
        observe(&dir);
    }
    for spec in specs {
        let (mut candidates, _) = paths(spec, env, cfg!(windows));
        if let Some(path) = selected.get(crate::installation::provider_namespace(spec.id)) {
            candidates.push(Candidate {
                path: path.clone(),
                source: "Selected",
            });
        }
        for candidate in candidates {
            observe(&candidate.path);
            if let Some(entry) = crate::npm_entry::entry(spec.id, &candidate.path) {
                observe(&entry);
            }
            if spec.id == "cursor"
                && let Some((node, entry)) = crate::cursor_entry::command(&candidate.path)
            {
                observe(&node);
                if let Some(folder) = entry.parent() {
                    observe(&folder.join("package.json"));
                }
            }
        }
    }
    hash.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_discovery_preserves_path_order_and_deduplicates_entries() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        for path in [&first, &second] {
            std::fs::create_dir(path).unwrap();
        }
        for path in [
            first.join("pi.cmd"),
            second.join("pi.exe"),
            second.join("pi.cmd"),
        ] {
            std::fs::write(path, "fixture").unwrap();
        }
        let mut env = EnvMap::new();
        env.insert(
            EnvKey::from("PATH"),
            format!(
                "{};{};{}",
                first.display(),
                second.display(),
                first.display()
            )
            .into(),
        );
        let (paths, truncated) = paths(crate::get("pi").unwrap(), &env, true);
        assert!(!truncated);
        assert_eq!(
            paths.iter().map(|c| c.path.clone()).collect::<Vec<_>>(),
            [
                first.join("pi.cmd"),
                second.join("pi.exe"),
                second.join("pi.cmd")
            ]
        );
    }
    #[test]
    fn native_override_precedes_executables_and_shell_shims_cannot_displace_them() {
        let dir = tempfile::tempdir().unwrap();
        let custom = dir.path().join("custom.exe");
        for path in [
            &custom,
            &dir.path().join("claude.exe"),
            &dir.path().join("claude.cmd"),
        ] {
            std::fs::write(path, "fixture").unwrap();
        }
        let mut env = EnvMap::new();
        env.insert(EnvKey::from("PATH"), dir.path().into());
        env.insert(
            EnvKey::from("CLAUDE_CODE_EXECUTABLE"),
            custom.clone().into(),
        );
        let (paths, _) = paths(crate::get("claude").unwrap(), &env, true);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].path, custom);
        assert_eq!(paths[0].source, "Environment override");
    }
    #[test]
    fn metadata_observation_detects_new_files_and_in_place_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = EnvMap::new();
        env.insert(EnvKey::from("PATH"), dir.path().into());
        let selected = Default::default();
        let specs = [crate::get("pi").unwrap().clone()];
        let before = fingerprint(&specs, &env, &selected);
        let path = dir.path().join(if cfg!(windows) { "pi.exe" } else { "pi" });
        std::fs::write(&path, "one").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let added = fingerprint(&specs, &env, &selected);
        assert_ne!(before, added);
        std::fs::write(path, "two with different length").unwrap();
        assert_ne!(added, fingerprint(&specs, &env, &selected));
    }

    #[test]
    fn excessive_discovery_is_bounded_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut dirs = Vec::new();
        for index in 0..12 {
            let path = dir.path().join(index.to_string());
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("pi.exe"), "fixture").unwrap();
            dirs.push(path.display().to_string());
        }
        let mut env = EnvMap::new();
        env.insert(EnvKey::from("PATH"), dirs.join(";").into());
        let (found, truncated) = paths(crate::get("pi").unwrap(), &env, true);
        assert_eq!(found.len(), MAX_CANDIDATES);
        assert!(truncated);
    }

    #[cfg(unix)]
    #[test]
    fn retargeted_symlink_changes_metadata_without_changing_the_stable_entry() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        for path in [&first, &second] {
            std::fs::write(path, "fixture").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let entry = dir.path().join("pi");
        symlink(&first, &entry).unwrap();
        let mut env = EnvMap::new();
        env.insert(EnvKey::from("PATH"), dir.path().into());
        let specs = [crate::get("pi").unwrap().clone()];
        let before = fingerprint(&specs, &env, &Default::default());
        std::fs::remove_file(&entry).unwrap();
        symlink(second, &entry).unwrap();
        assert_ne!(before, fingerprint(&specs, &env, &Default::default()));
        assert_eq!(paths(&specs[0], &env, false).0[0].path, entry);
    }
}
