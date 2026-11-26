//! The one-time npm adapter install, inside `Detector::prepare_launch`.
//!
//! ```text
//! folder   <data root>/adapters/<package without "@", "/" as "-">-<version>, created private
//! valid    folder/installed.json names this package and version, and its entry file exists
//! install  only when not valid; one attempt per package at a time in this process, holding
//!          folder/install.lock exclusively (so the CLI and the app never install at once):
//!          1. remove folder/node_modules; write folder/package.json and folder/package-lock.json
//!             from the copies committed for this version (crates/ghost-agents/adapters/<folder>/)
//!          2. <npm> ci --omit=optional --ignore-scripts --no-audit --no-fund, in the folder, with
//!             the resolved and scrubbed env plus npm_config_update_notifier=false, 10 minutes
//!          3. node_modules/<package>/package.json: the version matches, and `bin` resolves to a
//!             file inside the folder
//!          4. <node> <entry> --version within 30 s prints the version
//!          5. write folder/installed.json atomically
//! ```
//!
//! `--omit=optional` drops the CLIs the adapters bundle, so they can only ever drive the user's own;
//! `--ignore-scripts` runs nothing from the tree. Dropping the future kills npm, and an unmarked
//! folder is cleaned at the next attempt, so an interrupted download never leaves a broken install
//! in use. The committed lockfile pins every transitive version and its integrity hash, so the
//! code that runs with the user's login is exactly the reviewed tree.

use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ghost_process::{
    HelperEnv, HelperSpec, LeftoverRegistry, Redactor, ResolvedEnv, run_helper, secret_values,
};
use serde::{Deserialize, Serialize};

use crate::launch::LaunchError;
use crate::{INSTALL_CHECK_TIMEOUT, INSTALL_TIMEOUT};

/// The marker of a complete install.
const MARKER: &str = "installed.json";

/// Held exclusively while an install runs, across processes.
const LOCK: &str = "install.lock";

/// How long a waiting install sleeps between tries of another process's lock.
const LOCK_RETRY: Duration = Duration::from_millis(200);

/// npm's output kept for the error message.
const NPM_OUTPUT: usize = 64 * 1024;

/// Install scripts a lockfile may list, reviewed (none so far: neither adapter's tree has one,
/// and `--ignore-scripts` keeps any from running in any case).
#[cfg(test)]
pub(crate) const INSTALL_SCRIPT_ALLOWLIST: &[&str] = &[];

/// The `package.json` and `package-lock.json` committed for one adapter version.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AdapterFiles {
    pub(crate) package_json: &'static str,
    pub(crate) lockfile: &'static str,
}

/// The committed files of `package` at `version`, embedded in the binary.
pub(crate) fn committed(package: &str, version: &str) -> Option<AdapterFiles> {
    match (package, version) {
        ("@agentclientprotocol/claude-agent-acp", "0.95.0") => Some(AdapterFiles {
            package_json: include_str!(
                "../adapters/agentclientprotocol-claude-agent-acp-0.95.0/package.json"
            ),
            lockfile: include_str!(
                "../adapters/agentclientprotocol-claude-agent-acp-0.95.0/package-lock.json"
            ),
        }),
        ("@agentclientprotocol/codex-acp", "2.2.2") => Some(AdapterFiles {
            package_json: include_str!(
                "../adapters/agentclientprotocol-codex-acp-2.2.2/package.json"
            ),
            lockfile: include_str!(
                "../adapters/agentclientprotocol-codex-acp-2.2.2/package-lock.json"
            ),
        }),
        _ => None,
    }
}

/// `agentclientprotocol-claude-agent-acp-0.95.0` for `@agentclientprotocol/claude-agent-acp`
/// 0.95.0: the install folder's name, and the committed files' folder.
pub(crate) fn folder_name(package: &str, version: &str) -> String {
    format!(
        "{}-{version}",
        package.trim_start_matches('@').replace('/', "-")
    )
}

/// `folder/installed.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Marker {
    package: String,
    version: String,
    /// Relative to the folder.
    entry: String,
    installed_at_ms: i64,
}

/// The entry file of a valid install of `package` at `version` in `folder`.
pub(crate) fn installed_entry(folder: &Path, package: &str, version: &str) -> Option<PathBuf> {
    let marker: Marker = serde_json::from_slice(&std::fs::read(folder.join(MARKER)).ok()?).ok()?;
    if marker.package != package || marker.version != version {
        return None;
    }
    let entry = inside(folder, Path::new(&marker.entry))?;
    entry.is_file().then_some(entry)
}

/// Deletes the marker, so the next start installs again.
pub(crate) fn forget(folder: &Path) -> io::Result<()> {
    match std::fs::remove_file(folder.join(MARKER)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// `relative` below `base`, if it never leaves it: plain names only, no `..`, no root.
fn inside(base: &Path, relative: &Path) -> Option<PathBuf> {
    let mut path = base.to_path_buf();
    let mut any = false;
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                path.push(part);
                any = true;
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    any.then_some(path)
}

/// One install: everything it needs, owned, so the future can be shared and outlive its caller.
#[derive(Debug, Clone)]
pub(crate) struct InstallJob {
    pub(crate) package: &'static str,
    pub(crate) version: &'static str,
    pub(crate) bin: &'static str,
    /// "Claude", for messages.
    pub(crate) short_name: String,
    pub(crate) folder: PathBuf,
    pub(crate) npm: PathBuf,
    pub(crate) node: PathBuf,
    pub(crate) env: Arc<ResolvedEnv>,
    pub(crate) registry: Arc<LeftoverRegistry>,
}

/// Installs the adapter (see the module docs); the absolute entry file when it is complete.
pub(crate) async fn install(job: InstallJob) -> Result<PathBuf, LaunchError> {
    let short = job.short_name.clone();
    let failed = move |detail: String| LaunchError::InstallFailed {
        reason: format!("Could not download the {short} adapter: {detail}"),
    };
    let files = committed(job.package, job.version)
        .ok_or_else(|| failed("this build has no lockfile for it.".into()))?;
    let folder = job.folder.clone();
    blocking(move || create_private(&folder))
        .await
        .map_err(|error| failed(format!("{error}.")))?;
    let _lock = lock(&job.folder)
        .await
        .map_err(|error| failed(format!("{error}.")))?;
    // Another process may have finished it while this one waited for the lock.
    if let Some(entry) = installed_entry(&job.folder, job.package, job.version) {
        return Ok(entry);
    }
    let folder = job.folder.clone();
    blocking(move || prepare(&folder, files))
        .await
        .map_err(|error| failed(format!("{error}.")))?;

    let redactor = Redactor::standard();
    redactor.set_secrets(secret_values(&job.env.vars));
    tracing::info!(
        package = job.package,
        version = job.version,
        "installing an adapter"
    );
    let npm = run_helper(
        &job.registry,
        HelperSpec {
            program: job.npm.clone(),
            args: [
                "ci",
                "--omit=optional",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
            ]
            .iter()
            .map(OsString::from)
            .collect(),
            cwd: job.folder.clone(),
            env: HelperEnv::Resolved(Arc::clone(&job.env)),
            set: vec![("npm_config_update_notifier".into(), "false".into())],
            new_session: false,
            timeout: INSTALL_TIMEOUT,
            max_stdout: NPM_OUTPUT,
        },
    )
    .await
    .map_err(|error| failed(format!("{error}.")))?;
    if npm.timed_out {
        return Err(failed("npm did not finish within 10 minutes.".into()));
    }
    let code = npm.exit.and_then(|exit| exit.code);
    tracing::info!(
        package = job.package,
        version = job.version,
        code,
        "npm ci finished"
    );
    if code != Some(0) {
        let line = npm_error_line(&npm.stderr_tail).map_or_else(
            || {
                code.map_or_else(
                    || "npm was stopped.".to_owned(),
                    |code| format!("npm exited with code {code}."),
                )
            },
            |line| redactor.redact_text(&line).into_owned(),
        );
        return Err(failed(line));
    }

    let (folder, package, version, bin) = (job.folder.clone(), job.package, job.version, job.bin);
    let entry = blocking(move || verify(&folder, package, version, bin))
        .await
        .map_err(failed.clone())?;

    let check = run_helper(
        &job.registry,
        HelperSpec {
            program: job.node.clone(),
            args: vec![entry.clone().into_os_string(), "--version".into()],
            cwd: job.folder.clone(),
            env: HelperEnv::Resolved(Arc::clone(&job.env)),
            set: Vec::new(),
            new_session: false,
            timeout: INSTALL_CHECK_TIMEOUT,
            max_stdout: NPM_OUTPUT,
        },
    )
    .await
    .map_err(|error| failed(format!("{error}.")))?;
    let printed = String::from_utf8_lossy(&check.stdout);
    if check.timed_out
        || check.exit.and_then(|exit| exit.code) != Some(0)
        || !printed.contains(job.version)
    {
        let line = first_line(&check.stderr_tail)
            .or_else(|| first_line(&printed))
            .map_or_else(
                || "it printed no version".to_owned(),
                |line| redactor.redact_text(&line).into_owned(),
            );
        return Err(failed(format!("the adapter did not start ({line}).")));
    }

    let (folder, marked) = (job.folder.clone(), entry.clone());
    let relative = marked
        .strip_prefix(&job.folder)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let marker = Marker {
        package: job.package.to_owned(),
        version: job.version.to_owned(),
        entry: relative,
        installed_at_ms: now_millis(),
    };
    blocking(move || write_marker(&folder, &marker))
        .await
        .map_err(|error| failed(format!("{error}.")))?;
    tracing::info!(
        package = job.package,
        version = job.version,
        "adapter installed"
    );
    Ok(entry)
}

/// Runs blocking file work on the blocking pool.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| Err(error.to_string()))
}

/// The folder, readable by this user only.
fn create_private(folder: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(folder)
        .map_err(|error| format!("{} could not be created: {error}", folder.display()))
}

/// Takes `folder/install.lock` exclusively, waiting for another process's install; the file is
/// unlocked when the returned handle drops (with the future, if the start is interrupted).
async fn lock(folder: &Path) -> Result<ghost_process::locked_file::LockedFile, String> {
    let path = folder.join(LOCK);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| format!("{} could not be opened: {error}", path.display()))?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(ghost_process::locked_file::LockedFile::new(file)),
            Err(std::fs::TryLockError::WouldBlock) => tokio::time::sleep(LOCK_RETRY).await,
            // A file system without locks: one install per process still holds.
            Err(std::fs::TryLockError::Error(error))
                if error.kind() == io::ErrorKind::Unsupported =>
            {
                return Ok(ghost_process::locked_file::LockedFile::new(file));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(format!("{} could not be locked: {error}", path.display()));
            }
        }
    }
}

/// Cleans what an earlier, unmarked attempt left, and writes the committed files.
fn prepare(folder: &Path, files: AdapterFiles) -> Result<(), String> {
    let modules = folder.join("node_modules");
    match std::fs::remove_dir_all(&modules) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(format!(
                "{} could not be removed: {error}",
                modules.display()
            ));
        }
        _ => {}
    }
    forget(folder).map_err(|error| error.to_string())?;
    for (name, text) in [
        ("package.json", files.package_json),
        ("package-lock.json", files.lockfile),
    ] {
        std::fs::write(folder.join(name), text)
            .map_err(|error| format!("{name} could not be written: {error}"))?;
    }
    Ok(())
}

/// The installed package's version and entry file (step 3).
fn verify(folder: &Path, package: &str, version: &str, bin: &str) -> Result<PathBuf, String> {
    let package_dir = inside(folder, &Path::new("node_modules").join(package))
        .ok_or_else(|| "its package name is not a plain path.".to_owned())?;
    let manifest: serde_json::Value = std::fs::read(package_dir.join("package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| format!("npm left no readable package.json for {package}."))?;
    let found = manifest["version"].as_str().unwrap_or_default();
    if found != version {
        return Err(format!("npm installed {package} {found}, not {version}."));
    }
    let program = match &manifest["bin"] {
        serde_json::Value::String(path) => Some(path.as_str()),
        serde_json::Value::Object(bins) => bins.get(bin).and_then(serde_json::Value::as_str),
        _ => None,
    }
    .ok_or_else(|| format!("{package} names no {bin} program."))?;
    let entry = inside(&package_dir, Path::new(program))
        .ok_or_else(|| format!("{package}'s program lies outside its folder."))?;
    if !entry.is_file() {
        return Err(format!("{package}'s program file is missing."));
    }
    // A symlink must not lead out of the install either.
    let real = std::fs::canonicalize(&entry).map_err(|error| error.to_string())?;
    let base = std::fs::canonicalize(folder).map_err(|error| error.to_string())?;
    if !real.starts_with(&base) {
        return Err(format!("{package}'s program lies outside its folder."));
    }
    Ok(entry)
}

/// Temp file and rename, so a marker is whole or absent.
fn write_marker(folder: &Path, marker: &Marker) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(marker).map_err(|error| error.to_string())?;
    let temp = folder.join(format!("{MARKER}.tmp"));
    std::fs::write(&temp, text)
        .map_err(|error| format!("the install marker could not be written: {error}"))?;
    std::fs::rename(&temp, folder.join(MARKER))
        .map_err(|error| format!("the install marker could not be written: {error}"))
}

/// The first line npm marks as an error (`npm error code E404`, or `npm ERR!` from older npm),
/// else the first line at all.
fn npm_error_line(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("npm error") || line.starts_with("npm ERR!"))
        .map(str::to_owned)
        .or_else(|| first_line(stderr))
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(300).collect())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::catalog::{Recipe, catalog};

    #[test]
    fn every_catalog_adapter_has_a_reviewed_lockfile() {
        let mut adapters = 0;
        for spec in catalog() {
            let Recipe::NpmAdapter {
                package,
                version,
                bin,
                ..
            } = &spec.recipe
            else {
                continue;
            };
            adapters += 1;
            let files = committed(package, version)
                .unwrap_or_else(|| panic!("{} has no committed lockfile", spec.id));
            let manifest: Value = serde_json::from_str(files.package_json).expect("package.json");
            assert_eq!(
                manifest,
                json!({"name": "auracode-adapter", "private": true, "dependencies": {*package: *version}}),
                "{}: the root depends on exactly the catalog version",
                spec.id
            );
            let lock: Value = serde_json::from_str(files.lockfile).expect("package-lock.json");
            assert_eq!(lock["lockfileVersion"], json!(3), "{}", spec.id);
            assert_eq!(lock["name"], json!("auracode-adapter"));
            let packages = lock["packages"].as_object().expect("packages");
            assert_eq!(
                packages[""]["dependencies"],
                json!({*package: *version}),
                "{}: the lockfile's root matches package.json",
                spec.id
            );
            let adapter = &packages[&format!("node_modules/{package}")];
            assert_eq!(adapter["version"], json!(*version), "{}", spec.id);
            assert!(
                adapter["integrity"]
                    .as_str()
                    .is_some_and(|hash| hash.starts_with("sha512-"))
            );
            assert!(
                adapter["bin"][*bin].is_string(),
                "{}: the bin the catalog runs",
                spec.id
            );
            for (path, entry) in packages {
                if path.is_empty() {
                    continue;
                }
                assert!(
                    entry["integrity"].as_str().is_some() && entry["resolved"].as_str().is_some(),
                    "{}: {path} is pinned with its integrity",
                    spec.id
                );
                if entry["hasInstallScript"] == json!(true) {
                    assert!(
                        INSTALL_SCRIPT_ALLOWLIST.contains(&path.as_str()),
                        "{}: {path} has an install script nobody reviewed",
                        spec.id
                    );
                }
            }
        }
        assert_eq!(adapters, 2, "claude-acp and codex-acp");
        assert!(committed("@agentclientprotocol/claude-agent-acp", "0.94.0").is_none());
    }

    #[test]
    fn folder_names_flatten_the_package_name() {
        assert_eq!(
            folder_name("@agentclientprotocol/claude-agent-acp", "0.95.0"),
            "agentclientprotocol-claude-agent-acp-0.95.0"
        );
        assert_eq!(folder_name("plain", "1.0.0"), "plain-1.0.0");
    }

    #[test]
    fn paths_never_leave_their_base() {
        let base = Path::new("/data/adapters/x");
        assert_eq!(
            inside(base, Path::new("dist/index.js")),
            Some(PathBuf::from("/data/adapters/x/dist/index.js"))
        );
        assert_eq!(
            inside(base, Path::new("./dist/index.js")),
            Some(PathBuf::from("/data/adapters/x/dist/index.js"))
        );
        assert_eq!(inside(base, Path::new("../../evil.js")), None);
        assert_eq!(inside(base, Path::new("dist/../../evil.js")), None);
        assert_eq!(inside(base, Path::new("/etc/passwd")), None);
        assert_eq!(inside(base, Path::new("")), None);
    }

    #[test]
    fn the_marker_names_the_package_version_and_an_existing_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path();
        let package = "@agentclientprotocol/codex-acp";
        assert_eq!(installed_entry(folder, package, "2.2.2"), None, "no marker");
        let entry = folder.join("node_modules/@agentclientprotocol/codex-acp/dist/index.js");
        std::fs::create_dir_all(entry.parent().expect("parent")).expect("mkdir");
        std::fs::write(&entry, b"// adapter").expect("write");
        let marker = Marker {
            package: package.into(),
            version: "2.2.2".into(),
            entry: "node_modules/@agentclientprotocol/codex-acp/dist/index.js".into(),
            installed_at_ms: 1,
        };
        write_marker(folder, &marker).expect("marker");
        assert_eq!(
            installed_entry(folder, package, "2.2.2"),
            Some(entry.clone())
        );
        assert_eq!(
            installed_entry(folder, package, "2.2.3"),
            None,
            "another version"
        );
        assert_eq!(
            installed_entry(folder, "@other/pkg", "2.2.2"),
            None,
            "another package"
        );
        std::fs::remove_file(&entry).expect("remove");
        assert_eq!(
            installed_entry(folder, package, "2.2.2"),
            None,
            "the entry is gone"
        );
        let escaping = Marker {
            entry: "../outside.js".into(),
            ..marker
        };
        write_marker(folder, &escaping).expect("marker");
        assert_eq!(installed_entry(folder, package, "2.2.2"), None);
        forget(folder).expect("forget");
        forget(folder).expect("forgetting twice is fine");
        assert!(!folder.join(MARKER).exists());
    }

    #[test]
    fn preparing_cleans_an_unmarked_attempt_and_writes_the_committed_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path();
        std::fs::create_dir_all(folder.join("node_modules/partial")).expect("mkdir");
        std::fs::write(folder.join(MARKER), b"{}").expect("write");
        let files = committed("@agentclientprotocol/codex-acp", "2.2.2").expect("files");
        prepare(folder, files).expect("prepare");
        assert!(!folder.join("node_modules").exists());
        assert!(!folder.join(MARKER).exists());
        assert_eq!(
            std::fs::read_to_string(folder.join("package.json")).expect("read"),
            files.package_json
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("package-lock.json")).expect("read"),
            files.lockfile
        );
    }

    #[test]
    fn npm_errors_come_from_the_first_marked_line() {
        let stderr = "npm warn config something\nnpm error code E404\nnpm error 404 Not Found - GET https://registry.npmjs.org/x\n";
        assert_eq!(
            npm_error_line(stderr).as_deref(),
            Some("npm error code E404")
        );
        assert_eq!(
            npm_error_line("npm ERR! code EINTEGRITY\n").as_deref(),
            Some("npm ERR! code EINTEGRITY")
        );
        assert_eq!(
            npm_error_line("\n  something broke\n").as_deref(),
            Some("something broke")
        );
        assert_eq!(npm_error_line(""), None);
    }
}
