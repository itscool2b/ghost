//! A fake machine for detection and install tests: a `PATH` folder of fake tools (this crate's
//! test binary copied as `node`, `npm`, `claude`, `codex` and `hermes`), a home, an adapters folder,
//! and the folder the fakes read their behaviour from and log into. Nothing real ever runs.

#![allow(dead_code, reason = "each test file uses its own part of the helpers")]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ghost_agents::{AgentSpec, Detector, get};
use ghost_process::{EnvKey, EnvMap, EnvResolver, EnvSource, LeftoverRegistry, ResolvedEnv};
use tokio::runtime::Handle;

pub const TOOLS: &[&str] = &["node", "npm", "claude", "codex", "hermes"];

pub struct Machine {
    pub root: tempfile::TempDir,
    pub bin: PathBuf,
    pub home: PathBuf,
    pub adapters: PathBuf,
    pub fake: PathBuf,
}

impl Machine {
    /// A machine with every fake tool on `PATH`.
    pub fn new() -> Machine {
        Machine::with_tools(TOOLS)
    }

    pub fn with_tools(tools: &[&str]) -> Machine {
        let root = tempfile::tempdir().expect("tempdir");
        let bin = root.path().join("bin");
        let home = root.path().join("home");
        let adapters = root.path().join("data").join("adapters");
        let fake = root.path().join("fake");
        for dir in [&bin, &home, &fake] {
            std::fs::create_dir_all(dir).expect("mkdir");
        }
        let machine = Machine {
            root,
            bin,
            home,
            adapters,
            fake,
        };
        for tool in tools {
            machine.install_tool(&machine.bin, tool);
        }
        machine
    }

    /// Copies the fake under `name` into `dir`.
    pub fn install_tool(&self, dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).expect("mkdir");
        let path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let binary = option_env!("CARGO_BIN_EXE_ghost-agents-testbin")
            .or(option_env!("CARGO_BIN_EXE_ghost-plugin-testbin"))
            .expect("the package's fake CLI binary");
        std::fs::copy(binary, &path).expect("copy");
        path
    }

    pub fn tool(&self, name: &str) -> PathBuf {
        self.bin
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }

    /// Sets what a fake does (`ok`, `fail`, `hang`, ...).
    pub fn mode(&self, tool: &str, mode: &str) {
        std::fs::write(self.fake.join(format!("{tool}.mode")), mode).expect("write");
    }

    /// Sets what a fake's `--version` prints.
    pub fn version(&self, tool: &str, printed: &str) {
        std::fs::write(self.fake.join(format!("{tool}.version")), printed).expect("write");
    }

    /// Every run of a fake: its cwd, argv and npm's update-notifier setting.
    pub fn runs(&self, tool: &str) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.fake.join(format!("{tool}.log")))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("a run"))
            .collect()
    }

    /// The env: the test's own, with this machine's `PATH` and home and no user binary variables.
    pub fn env(&self, extra: &[(&str, &str)]) -> ResolvedEnv {
        self.env_with_path(&[&self.bin], extra)
    }

    pub fn env_with_path(&self, path: &[&Path], extra: &[(&str, &str)]) -> ResolvedEnv {
        // Keep only OS launch requirements. Tests never inherit agent homes, executable
        // overrides, package-manager roots or credentials from the owner's environment.
        let system = ghost_process::process_env();
        let mut vars = EnvMap::new();
        for name in ["SystemRoot", "WINDIR", "COMSPEC", "PATHEXT", "SystemDrive"] {
            if let Some(value) = system.get(&EnvKey::from(name)) {
                vars.insert(EnvKey::from(name), value.clone());
            }
        }
        for name in [
            "TEMP",
            "TMP",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
        ] {
            let path = self.root.path().join(name);
            std::fs::create_dir_all(&path).expect("isolated environment folder");
            vars.insert(EnvKey::from(name), path.into_os_string());
        }
        vars.insert(
            EnvKey::from("PATH"),
            std::env::join_paths(path).expect("join"),
        );
        vars.insert(EnvKey::from("HOME"), self.home.clone().into_os_string());
        vars.insert(
            EnvKey::from("USERPROFILE"),
            self.home.clone().into_os_string(),
        );
        vars.insert(
            EnvKey::from("GHOST_FAKE_DIR"),
            self.fake.clone().into_os_string(),
        );
        for (name, value) in extra {
            vars.insert(EnvKey::from(*name), OsString::from(*value));
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

    pub fn detector(&self, env: ResolvedEnv) -> Arc<Detector> {
        Detector::new(
            EnvResolver::fixed(env),
            LeftoverRegistry::disabled(),
            self.adapters.clone(),
            Handle::current(),
        )
    }

    /// The install folder of an adapter.
    pub fn adapter_folder(&self, name: &str) -> PathBuf {
        self.adapters.join(name)
    }

    /// The pids a hanging fake wrote, once they are there.
    pub async fn pids(&self, tool: &str) -> Vec<u32> {
        let path = self.fake.join(format!("{tool}.pids"));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(text) = std::fs::read_to_string(&path) {
                return text
                    .split_whitespace()
                    .map(|pid| pid.parse().expect("pid"))
                    .collect();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{} never appeared",
                path.display()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

pub fn spec(id: &str) -> AgentSpec {
    get(id).expect("a catalog agent").clone()
}

/// Whether `pid` runs (a zombie has exited already).
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    let state = String::from_utf8_lossy(&output.stdout);
    let state = state.trim();
    !state.is_empty() && !state.starts_with('Z')
}

#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

/// A process a test started, identified by its pid and, on Unix, its start time, so a recycled
/// pid is never taken for it.
#[derive(Debug, Clone)]
pub struct Held {
    pid: u32,
    /// `None`: it had exited already when it was held (Windows keeps no start times).
    #[cfg(unix)]
    start: Option<ghost_process::ptable::StartTime>,
}

impl Held {
    pub fn new(pid: u32) -> Held {
        Held {
            pid,
            #[cfg(unix)]
            start: ghost_process::ptable::start_time(pid).ok(),
        }
    }

    /// Whether the process still runs as itself.
    pub fn running(&self) -> bool {
        #[cfg(unix)]
        {
            self.start.as_ref().is_some_and(|start| {
                alive(self.pid)
                    && ghost_process::ptable::start_time(self.pid).is_ok_and(|now| now == *start)
            })
        }
        #[cfg(windows)]
        {
            alive(self.pid)
        }
    }

    /// Kills it, only while it still runs as itself.
    fn kill(&self) {
        if !self.running() {
            return;
        }
        #[cfg(unix)]
        {
            let _ = std::process::Command::new("kill")
                .args(["-9", &self.pid.to_string()])
                .output();
        }
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &self.pid.to_string()])
                .output();
        }
    }
}

/// Polls until `process` is gone (or its pid belongs to another process); whether that happened
/// within 5 s.
pub async fn gone(process: &Held) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if !process.running() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Holds the processes a test started and kills, when dropped, those still running as
/// themselves, so a failing test leaves nothing behind and never hits a recycled pid.
#[derive(Debug, Default)]
pub struct Reaper(Vec<Held>);

impl Reaper {
    /// Holds `pids`, read now with their start times; returns what it holds, for [`gone`].
    pub fn hold(pids: &[u32]) -> (Reaper, Vec<Held>) {
        let held: Vec<Held> = pids.iter().map(|pid| Held::new(*pid)).collect();
        (Reaper(held.clone()), held)
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        for process in &self.0 {
            process.kill();
        }
    }
}
