//! Cursor's official Windows distribution: a stable cursor-agent shim beside versioned Node
//! runtimes. Resolve the reviewed layout directly, without interpreting or executing a shell shim.
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_VERSIONS: usize = 128;
const MAX_MANIFEST: u64 = 64 * 1024;

/// Newer releases include a build time before the commit hash; older releases contain a date.
fn version_key(name: &str) -> Option<([u32; 6], &str)> {
    let pieces: Vec<_> = name.split('-').collect();
    if !matches!(pieces.len(), 2 | 5) {
        return None;
    }
    let date: Vec<_> = pieces[0].split('.').collect();
    if date.len() != 3 {
        return None;
    }
    let mut values = [0; 6];
    for (index, part) in date.into_iter().enumerate() {
        values[index] = part.parse().ok()?;
    }
    if values[0] < 2025 || !(1..=12).contains(&values[1]) || !(1..=31).contains(&values[2]) {
        return None;
    }
    if pieces.len() == 5 {
        for index in 0..3 {
            values[index + 3] = pieces[index + 1].parse().ok()?;
        }
        if values[3] > 23 || values[4] > 59 || values[5] > 59 {
            return None;
        }
    }
    let commit = *pieces.last()?;
    if !(7..=64).contains(&commit.len()) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some((values, commit))
}

fn runtime(folder: &Path, root: &Path) -> Option<(PathBuf, PathBuf)> {
    let folder = folder.canonicalize().ok()?;
    if !folder.starts_with(root) {
        return None;
    }
    let manifest = folder.join("package.json").canonicalize().ok()?;
    if !manifest.starts_with(&folder) {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(manifest)
        .ok()?
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return None;
    }
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if manifest["name"].as_str()? != "@anysphere/agent-cli-runtime" {
        return None;
    }
    let node = folder.join("node.exe").canonicalize().ok()?;
    let entry = folder.join("index.js").canonicalize().ok()?;
    (node.starts_with(&folder) && entry.starts_with(&folder) && node.is_file() && entry.is_file())
        .then_some((node, entry))
}

/// Only the vendor-named command is accepted; an unrelated `agent` on PATH is never probed.
pub(crate) fn command(shim: &Path) -> Option<(PathBuf, PathBuf)> {
    if !crate::npm_entry::is_shim(shim)
        || !shim.file_stem()?.eq_ignore_ascii_case("cursor-agent")
        || !shim.is_file()
    {
        return None;
    }
    let root = shim.parent()?.canonicalize().ok()?;
    if let Some(found) = runtime(&root, &root) {
        return Some(found);
    }
    let versions = root.join("versions").canonicalize().ok()?;
    if !versions.starts_with(&root) {
        return None;
    }
    let mut entries = Vec::new();
    for (count, entry) in std::fs::read_dir(&versions).ok()?.enumerate() {
        if count >= MAX_VERSIONS {
            return None;
        }
        let entry = entry.ok()?;
        let name = entry.file_name().into_string().ok()?;
        if version_key(&name).is_some() && entry.path().is_dir() {
            entries.push((name, entry.path()));
        }
    }
    entries.sort_by(|a, b| version_key(&b.0).cmp(&version_key(&a.0)));
    entries
        .iter()
        .find_map(|(_, folder)| runtime(folder, &versions))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(root: &Path, version: &str) -> PathBuf {
        let path = root.join("versions").join(version);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("node.exe"), "fixture, never executed").unwrap();
        std::fs::write(path.join("index.js"), "fixture, never executed").unwrap();
        std::fs::write(
            path.join("package.json"),
            r#"{"name":"@anysphere/agent-cli-runtime"}"#,
        )
        .unwrap();
        path
    }

    #[test]
    fn resolves_only_complete_vendor_runtimes_in_date_and_time_order() {
        let scratch = tempfile::tempdir().unwrap();
        let shim = scratch.path().join("cursor-agent.cmd");
        std::fs::write(&shim, "untrusted contents never interpreted").unwrap();
        install(scratch.path(), "2026.9.30-abcdef0");
        install(scratch.path(), "2026.10.22-abcdef0");
        let newest = install(scratch.path(), "2026.10.22-11-30-00-abcdef0");
        install(scratch.path(), "2026.10.22-09-59-59-fffffff");
        let pending = install(scratch.path(), "2026.11.02-abcdef0");
        std::fs::remove_file(pending.join("index.js")).unwrap();
        let staged = install(scratch.path(), "2026.12.01-abcdef0");
        std::fs::write(staged.join("package.json"), r#"{"name":"unrelated"}"#).unwrap();
        assert_eq!(
            command(&shim),
            Some((
                newest.join("node.exe").canonicalize().unwrap(),
                newest.join("index.js").canonicalize().unwrap(),
            ))
        );
        let other = scratch.path().join("agent.cmd");
        std::fs::write(&other, "fixture").unwrap();
        assert_eq!(command(&other), None);
        assert_eq!(version_key("staged"), None);
        assert_eq!(version_key("2026.10.22-24-00-00-abcdef0"), None);
        assert_eq!(version_key("2026.10.22-other"), None);
    }

    #[test]
    fn changed_runtime_changes_the_installation_fingerprint() {
        let scratch = tempfile::tempdir().unwrap();
        let shim = scratch.path().join("cursor-agent.cmd");
        std::fs::write(&shim, "fixture").unwrap();
        let original = install(scratch.path(), "2026.10.22-abcdef0");
        let mut env = ghost_process::EnvMap::new();
        env.insert(ghost_process::EnvKey::from("PATH"), scratch.path().into());
        // A saved selection is observed on every platform, independent of executable suffix.
        let selected = [("cursor".into(), shim)].into_iter().collect();
        let specs = [crate::get("cursor").unwrap().clone()];
        for (file, contents) in [
            ("node.exe", "changed runtime, never executed"),
            ("index.js", "changed entry point, never executed"),
            (
                "package.json",
                r#"{"name":"@anysphere/agent-cli-runtime","version":"2026.10.22"}"#,
            ),
        ] {
            let previous = crate::candidates::fingerprint(&specs, &env, &selected);
            std::fs::write(original.join(file), contents).unwrap();
            assert_ne!(
                crate::candidates::fingerprint(&specs, &env, &selected),
                previous,
                "{file}"
            );
        }
        let before = crate::candidates::fingerprint(&specs, &env, &selected);
        install(scratch.path(), "2026.11.02-abcdef0");
        assert_ne!(
            crate::candidates::fingerprint(&specs, &env, &selected),
            before
        );
    }

    #[test]
    fn directory_enumeration_and_manifest_reads_are_bounded() {
        let scratch = tempfile::tempdir().unwrap();
        let shim = scratch.path().join("cursor-agent.cmd");
        std::fs::write(&shim, "fixture").unwrap();
        let older = install(scratch.path(), "2026.10.22-abcdef0");
        let oversized = install(scratch.path(), "2026.11.02-abcdef0");
        std::fs::write(
            oversized.join("package.json"),
            vec![b' '; MAX_MANIFEST as usize + 1],
        )
        .unwrap();
        assert_eq!(
            command(&shim).unwrap().1,
            older.join("index.js").canonicalize().unwrap()
        );
        for index in 2..MAX_VERSIONS {
            std::fs::write(
                scratch
                    .path()
                    .join("versions")
                    .join(format!("unrelated-{index}")),
                "fixture",
            )
            .unwrap();
        }
        assert!(
            command(&shim).is_some(),
            "exactly the enumeration limit is supported"
        );
        std::fs::write(scratch.path().join("versions/one-too-many"), "fixture").unwrap();
        assert_eq!(
            command(&shim),
            None,
            "an incomplete directory inventory must not choose a runtime"
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_links_cannot_escape_the_distribution() {
        for file in ["node.exe", "index.js", "package.json"] {
            let scratch = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let shim = scratch.path().join("cursor-agent.cmd");
            std::fs::write(&shim, "fixture").unwrap();
            let folder = install(scratch.path(), "2026.10.22-abcdef0");
            std::fs::rename(folder.join(file), outside.path().join(file)).unwrap();
            std::os::unix::fs::symlink(outside.path().join(file), folder.join(file)).unwrap();
            assert_eq!(command(&shim), None, "{file}");
        }
        let scratch = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let shim = scratch.path().join("cursor-agent.cmd");
        std::fs::write(&shim, "fixture").unwrap();
        std::fs::create_dir(scratch.path().join("versions")).unwrap();
        let target = install(outside.path(), "2026.10.22-abcdef0");
        std::os::unix::fs::symlink(target, scratch.path().join("versions/2026.10.22-abcdef0"))
            .unwrap();
        assert_eq!(
            command(&shim),
            None,
            "a version directory cannot leave the distribution"
        );
    }
}
