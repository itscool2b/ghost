//! What ghost-cli's tests share: a fake machine whose env finds no real agent (or only the fake
//! one, installed as `hermes`), a headless runtime on temporary roots, and package B's fixtures.
//! Nothing here runs a real agent or a login shell.

#![allow(dead_code, reason = "each test file uses its own part of the helpers")]

use std::path::{Path, PathBuf};

use ghost_cli::headless::{EnvChoice, Headless, HeadlessConfig, Roots};
use ghost_process::{EnvKey, EnvMap, EnvSource, ResolvedEnv};

/// A machine for one test: a `PATH` folder, a home, the fake agent's state, the machine root
/// (`run/`, `adapters/`) and the profile root, all in one temporary folder.
pub struct Machine {
    pub dir: tempfile::TempDir,
    pub bin: PathBuf,
    pub home: PathBuf,
    pub state: PathBuf,
    pub machine_root: PathBuf,
    pub profile_root: PathBuf,
}

impl Machine {
    /// A machine whose `PATH` finds nothing: only the built-in demo agent can start.
    pub fn new() -> Machine {
        let dir = tempfile::tempdir().expect("tempdir");
        // Canonical on Unix, as the CLI's run folders are: agents report the real path (macOS
        // reaches /var/folders through a link). Windows' canonical form has a `\\?\` prefix.
        let root = if cfg!(unix) {
            std::fs::canonicalize(dir.path()).expect("canonical tempdir")
        } else {
            dir.path().to_path_buf()
        };
        let machine = Machine {
            bin: root.join("bin"),
            home: root.join("home"),
            state: root.join("fake-state"),
            machine_root: root.join("machine"),
            profile_root: root.join("profile"),
            dir,
        };
        for folder in [&machine.bin, &machine.home, &machine.state] {
            std::fs::create_dir_all(folder).expect("a folder");
        }
        machine
    }

    /// A machine with the fake agent on `PATH` as `hermes`.
    pub fn with_fake_hermes() -> Machine {
        let machine = Machine::new();
        let name = if cfg!(windows) {
            "hermes.exe"
        } else {
            "hermes"
        };
        std::fs::copy(fake_agent(), machine.bin.join(name)).expect("install the fake agent");
        machine
    }

    /// The folder for anything else a test makes.
    pub fn path(&self, name: &str) -> PathBuf {
        self.machine_root
            .parent()
            .expect("the machine's folder")
            .join(name)
    }

    /// The fixed env agents start with here, plus `extra`.
    pub fn env(&self, extra: &[(&str, &str)]) -> ResolvedEnv {
        let mut vars = EnvMap::new();
        vars.insert(EnvKey::from("PATH"), self.bin.clone().into_os_string());
        vars.insert(EnvKey::from("HOME"), self.home.clone().into_os_string());
        vars.insert(
            EnvKey::from("USERPROFILE"),
            self.home.clone().into_os_string(),
        );
        vars.insert(
            EnvKey::from("GHOST_FAKE_STATE"),
            self.state.clone().into_os_string(),
        );
        // A Windows process needs its system folder.
        for name in ["SystemRoot", "SYSTEMROOT", "TEMP", "TMP"] {
            if cfg!(windows)
                && let Some(value) = std::env::var_os(name)
            {
                vars.insert(EnvKey::from(name), value);
            }
        }
        for (name, value) in extra {
            vars.insert(EnvKey::from(*name), (*value).into());
        }
        ResolvedEnv {
            vars,
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        }
    }

    /// A headless runtime on this machine's roots, with the fixed env plus `extra`.
    pub async fn headless(&self, extra: &[(&str, &str)]) -> Headless {
        self.headless_setting(extra, &[]).await
    }

    /// The same, with `set` on every agent process (what `probe --env` does).
    pub async fn headless_setting(&self, extra: &[(&str, &str)], set: &[(&str, &str)]) -> Headless {
        Headless::start(HeadlessConfig {
            roots: Roots {
                machine: self.machine_root.clone(),
                profile: self.profile_root.clone(),
            },
            env: EnvChoice::Fixed(self.env(extra)),
            set_env: set
                .iter()
                .map(|(name, value)| ((*name).into(), (*value).into()))
                .collect(),
            app_version: "0.1.0".into(),
            checkpoints: false,
        })
        .await
        .expect("the headless runtime starts")
    }

    /// This process's leftover registry files in the machine root's `run/`.
    pub fn registry_files(&self) -> Vec<PathBuf> {
        let prefix = format!("{}-", std::process::id());
        std::fs::read_dir(self.machine_root.join("run"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The fake ACP agent this package builds.
pub fn fake_agent() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ghost-cli-fake-agent"))
}

/// Package B's fixture folder.
pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ghost-drivers/tests/fixtures/acp")
}

/// Every committed fixture, sorted.
pub fn fixtures() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![fixture_root()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).expect("a fixture folder") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                folders.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The lines of the output that start with `label` (`PASS`, `FAIL`, ...).
pub fn lines<'a>(output: &'a str, label: &str) -> Vec<&'a str> {
    output
        .lines()
        .filter(|line| line.starts_with(&format!("{label}  ")))
        .collect()
}

/// Fails with the whole output when a line starts with FAIL.
pub fn assert_no_failures(output: &str) {
    let failures = lines(output, "FAIL");
    assert!(
        failures.is_empty(),
        "failures: {failures:#?}\n\nthe whole output:\n{output}"
    );
}

/// Fails with the whole output unless a PASS line holds `needle`.
pub fn assert_passed(output: &str, needle: &str) {
    assert!(
        lines(output, "PASS")
            .iter()
            .any(|line| line.contains(needle)),
        "no PASS line holds {needle:?}:\n{output}"
    );
}

/// The protocol logs a probe or smoke run names at its end, with the fixture name each can become.
pub fn named_logs(output: &str) -> Vec<(PathBuf, String)> {
    output
        .lines()
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["ghost-cli", "fixture", log, "--scenario", name, "--out", _] => {
                    Some((PathBuf::from(log), (*name).to_owned()))
                }
                _ => None,
            }
        })
        .collect()
}
