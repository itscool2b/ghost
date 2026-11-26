//! Helpers for ghost-process's real-process tests: the test child, specs, waiting, and the reaper
//! every test kills its processes with. Each test file declares this module under `#[cfg(test)]`.

#![allow(dead_code, reason = "each test file uses its own part of the helpers")]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ghost_process::{
    EnvKey, EnvMap, EnvSource, LaunchSpec, LineSource, ProcessCwd, ResolvedEnv, SpawnSpec,
    StdoutItem,
};

/// Generous: a loaded CI machine with antivirus scanning can take seconds to start a process.
pub const STARTUP: Duration = Duration::from_secs(30);

/// The spike's bar for "the tree is dead".
pub const GONE_WITHIN: Duration = Duration::from_secs(5);

pub fn testbin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ghost-testbin"))
}

/// The test process's own env, as a resolved env.
pub fn env() -> Arc<ResolvedEnv> {
    env_with(&[])
}

/// The test process's env plus `extra`.
pub fn env_with(extra: &[(&str, &str)]) -> Arc<ResolvedEnv> {
    let mut vars: EnvMap = ghost_process::process_env();
    for (name, value) in extra {
        vars.insert(EnvKey::from(*name), OsString::from(*value));
    }
    Arc::new(ResolvedEnv {
        vars,
        source: EnvSource::Fallback {
            reason: "test".into(),
        },
        warnings: Vec::new(),
        resolved_at_ms: 0,
    })
}

/// The test child in `mode`, run in the chat folder.
pub fn launch(args: &[&str]) -> LaunchSpec {
    LaunchSpec {
        label: format!(
            "ghost-testbin {}",
            args.first().copied().unwrap_or_default()
        ),
        program: testbin(),
        args: args.iter().map(OsString::from).collect(),
        cwd: ProcessCwd::ChatFolder,
        set_env: Vec::new(),
        daemons: Vec::new(),
        startup_timeout: STARTUP,
    }
}

pub fn spec(launch: LaunchSpec, folder: &Path) -> SpawnSpec {
    SpawnSpec {
        launch,
        folder: folder.to_path_buf(),
        env: env(),
        max_line_bytes: ghost_process::MAX_LINE_BYTES,
    }
}

/// The next stdout line, parsed.
pub async fn next_json(stdout: &mut LineSource) -> serde_json::Value {
    match tokio::time::timeout(STARTUP, stdout.next())
        .await
        .expect("a line in time")
    {
        Some(StdoutItem::Line(line)) => serde_json::from_str(&line).expect("JSON"),
        other => panic!("expected a line, got {other:?}"),
    }
}

/// The pids a test child wrote to `path`, once it exists.
pub async fn read_pids(path: &Path) -> Vec<u32> {
    let deadline = tokio::time::Instant::now() + STARTUP;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text
                .split_whitespace()
                .map(|pid| pid.parse().expect("a pid"))
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

/// Whether `pid` runs (a zombie has exited already).
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    match kill(Pid::from_raw(i32::try_from(pid).expect("pid")), None) {
        Ok(()) | Err(Errno::EPERM) => {}
        Err(_) => return false,
    }
    !zombie(pid)
}

#[cfg(target_os = "linux")]
fn zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state == "Z")
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn zombie(pid: u32) -> bool {
    let output = std::process::Command::new("/bin/ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    let state = String::from_utf8_lossy(&output.stdout);
    let state = state.trim();
    state.is_empty() || state.starts_with('Z')
}

/// Whether `pid` runs, through `tasklist` (no unsafe Windows calls in this crate).
#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

/// Polls until `pid` is gone; whether that happened within `limit`.
pub async fn wait_gone(pid: u32, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if !alive(pid) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Kills, when dropped, every process it holds that still runs as itself, so a failed assertion
/// never leaves a sleeper behind; [`Reaper::assert_gone`] is each test's leftover check.
#[derive(Default)]
pub struct Reaper {
    #[cfg(unix)]
    held: Vec<(u32, ghost_process::ptable::StartTime)>,
    #[cfg(windows)]
    held: Vec<u32>,
}

impl Reaper {
    pub fn new() -> Reaper {
        Reaper::default()
    }

    /// Holds `pid`, identified by its start time where the OS has one.
    pub fn hold(&mut self, pid: u32) {
        #[cfg(unix)]
        if let Ok(start) = ghost_process::ptable::start_time(pid) {
            self.held.push((pid, start));
        }
        #[cfg(windows)]
        self.held.push(pid);
    }

    pub fn hold_all(&mut self, pids: &[u32]) {
        for pid in pids {
            self.hold(*pid);
        }
    }

    /// Asserts that nothing held runs any more, by pid and start time, within the spike's bar: a
    /// recycled pid is not the process that was held.
    pub async fn assert_gone(&self) {
        #[cfg(unix)]
        for (pid, start) in &self.held {
            let deadline = tokio::time::Instant::now() + GONE_WITHIN;
            while running_as(*pid, start) {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "process {pid} was left running"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        #[cfg(windows)]
        for pid in &self.held {
            assert!(
                wait_gone(*pid, GONE_WITHIN).await,
                "process {pid} was left running"
            );
        }
    }

    /// Releases `pid`: a test that expects it to live (a spared daemon) kills it itself.
    pub fn pids(&self) -> Vec<u32> {
        #[cfg(unix)]
        {
            self.held.iter().map(|(pid, _)| *pid).collect()
        }
        #[cfg(windows)]
        {
            self.held.clone()
        }
    }

    pub fn release(&mut self, pid: u32) {
        #[cfg(unix)]
        self.held.retain(|(held, _)| *held != pid);
        #[cfg(windows)]
        self.held.retain(|held| *held != pid);
    }
}

/// Whether `pid` still runs as the process that started at `start` (a zombie has exited).
#[cfg(unix)]
fn running_as(pid: u32, start: &ghost_process::ptable::StartTime) -> bool {
    alive(pid) && ghost_process::ptable::start_time(pid).is_ok_and(|now| now == *start)
}

impl Drop for Reaper {
    fn drop(&mut self) {
        #[cfg(unix)]
        for (pid, start) in &self.held {
            let same = ghost_process::ptable::start_time(*pid).is_ok_and(|now| now == *start);
            if same {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(i32::try_from(*pid).expect("pid")),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
        #[cfg(windows)]
        for pid in &self.held {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
    }
}

/// Kills `pid` now if it still runs (a spared daemon a test is done with).
pub fn kill_now(pid: u32) {
    #[cfg(unix)]
    {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid")),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
    }
}
