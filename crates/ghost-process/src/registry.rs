//! The leftover registry and the startup sweep (Unix).
//!
//! Each running Ghost process (the app or the CLI) keeps one file, `<data root>/run/<instance
//! pid>-<instance start key>.json`, listing its running agents: their group, their declared daemons
//! and, after every turn, the descendants that left the group. The file is written atomically on
//! every change and rewritten every [`REGISTRY_HEARTBEAT`](crate::REGISTRY_HEARTBEAT) while it has
//! entries, so its modification time is the instance's heartbeat, and deleted at a clean exit. At
//! the next start, [`sweep_stale`] stops what dead instances left running and touches nothing a live
//! instance owns.
//!
//! ```text
//! {"version":1,"instance":{"pid":4242,"start":{...},"sid":4242,"boot":"<marker>"},
//!  "entries":[{"pid":5001,"pgid":5001,"sid":4242,"start":{...},"label":"hermes acp",
//!              "spawned_at_ms":1792840000000,"daemons":[...],"escaped":[{"pid":5120,"start":{...}}]}]}
//! ```
//!
//! Windows keeps no registry: an agent's job object dies with its owner. There `open` returns a
//! disabled registry and `sweep_stale` does nothing.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;

use crate::ptable::StartTime;
use crate::spawn::DaemonMatch;

#[cfg(any(unix, test))]
mod sweep;

/// The file format's version.
#[cfg(any(unix, test))]
const VERSION: u32 = 1;

/// The Ghost process a registry file belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceRecord {
    pub pid: u32,
    pub start: StartTime,
    pub sid: Option<u32>,
    /// The boot the instance ran in: a different marker means the machine rebooted since.
    pub boot: Option<String>,
}

/// A descendant that left the agent's group, identified by pid and start time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscapedProc {
    pub pid: u32,
    pub start: StartTime,
}

/// One running agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftoverEntry {
    pub pid: u32,
    pub pgid: u32,
    pub sid: Option<u32>,
    pub start: StartTime,
    pub label: String,
    pub spawned_at_ms: i64,
    pub daemons: Vec<DaemonMatch>,
    pub escaped: Vec<EscapedProc>,
}

/// A registry file's contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileBody {
    version: u32,
    instance: InstanceRecord,
    entries: Vec<LeftoverEntry>,
}

/// This instance's registry file.
#[derive(Debug)]
pub struct LeftoverRegistry {
    own: InstanceRecord,
    /// `None` for a disabled registry.
    file: Option<Arc<RegistryFile>>,
}

/// The file and the entries it holds. Every change rewrites the file under the lock, so writes
/// land in the order the changes were made.
#[derive(Debug)]
pub(crate) struct RegistryFile {
    path: PathBuf,
    state: Mutex<FileState>,
    heartbeat: Mutex<Option<tokio::task::AbortHandle>>,
}

#[derive(Debug)]
struct FileState {
    body: FileBody,
    closed: bool,
    /// Only failed shutdown records survive an otherwise clean instance close.
    retain_at_close: std::collections::HashSet<u32>,
}

impl LeftoverRegistry {
    /// Opens this instance's file under `run_dir` and starts its heartbeat on `rt`.
    pub fn open(run_dir: &Path, rt: Handle) -> io::Result<Arc<LeftoverRegistry>> {
        #[cfg(unix)]
        {
            let own = own_instance();
            let file = RegistryFile::create(run_dir, own.clone())?;
            let weak = Arc::downgrade(&file);
            let task = rt.spawn(heartbeat(weak));
            *lock(&file.heartbeat) = Some(task.abort_handle());
            Ok(Arc::new(LeftoverRegistry {
                own,
                file: Some(file),
            }))
        }
        #[cfg(not(unix))]
        {
            let _ = (run_dir, rt);
            Ok(LeftoverRegistry::disabled())
        }
    }

    /// A registry that records nothing (Windows, tests, and a run directory that can't be written).
    pub fn disabled() -> Arc<LeftoverRegistry> {
        Arc::new(LeftoverRegistry {
            own: InstanceRecord {
                pid: std::process::id(),
                start: StartTime::Unknown,
                sid: None,
                boot: None,
            },
            file: None,
        })
    }

    /// This Ghost process.
    pub fn own(&self) -> &InstanceRecord {
        &self.own
    }

    /// Records a running agent (an entry with the same pid is replaced). A failure is a warning for
    /// the caller, never a failed spawn; the entry is still written with the next change.
    pub fn register(&self, entry: LeftoverEntry) -> io::Result<()> {
        self.handle().register(entry)
    }

    /// Replaces the escaped descendants recorded for the agent `pid`.
    pub fn set_escaped(&self, pid: u32, escaped: Vec<EscapedProc>) {
        self.handle().set_escaped(pid, escaped);
    }

    pub fn unregister(&self, pid: u32) {
        self.handle().unregister(pid);
    }

    /// Closes this instance's file. Failed shutdown records survive for recovery; otherwise the
    /// file is deleted. Later changes write nothing.
    pub fn close(&self) -> io::Result<()> {
        match &self.file {
            Some(file) => file.close(),
            None => Ok(()),
        }
    }

    /// A cheap handle for blocking threads, which need an owned value.
    pub(crate) fn handle(&self) -> RegistryHandle {
        RegistryHandle(self.file.clone())
    }

    /// The registry file's path, when there is one.
    #[cfg(test)]
    pub(crate) fn path(&self) -> Option<&Path> {
        self.file.as_ref().map(|file| file.path.as_path())
    }
}

/// What a blocking task holds to change the registry.
#[derive(Debug, Clone)]
pub(crate) struct RegistryHandle(Option<Arc<RegistryFile>>);

impl RegistryHandle {
    /// Shutdown owns the decision: an unconfirmed cleanup retains exact escaped identities for
    /// the next instance instead of converting a failed kill into lost recovery metadata.
    #[cfg(unix)]
    pub(crate) fn finish_shutdown(&self, pid: u32, escaped: Vec<EscapedProc>, complete: bool) {
        if complete {
            self.unregister(pid);
        } else if let Some(file) = &self.0 {
            let mut state = lock(&file.state);
            if state.closed {
                return;
            }
            if let Some(entry) = state.body.entries.iter_mut().find(|entry| entry.pid == pid) {
                entry.escaped = escaped;
                state.retain_at_close.insert(pid);
                file.write_or_warn(&state.body);
            }
        }
    }
    /// Whether changes are recorded at all (not a disabled registry).
    #[cfg(unix)]
    pub(crate) fn is_enabled(&self) -> bool {
        self.0.is_some()
    }

    pub(crate) fn register(&self, entry: LeftoverEntry) -> io::Result<()> {
        match &self.0 {
            Some(file) => file.register(entry),
            None => Ok(()),
        }
    }

    pub(crate) fn set_escaped(&self, pid: u32, escaped: Vec<EscapedProc>) {
        if let Some(file) = &self.0 {
            file.set_escaped(pid, escaped);
        }
    }

    pub(crate) fn unregister(&self, pid: u32) {
        if let Some(file) = &self.0 {
            file.unregister(pid);
        }
    }
}

impl RegistryFile {
    /// Creates the run directory (private) and writes an empty file for `own`.
    #[cfg(unix)]
    fn create(run_dir: &Path, own: InstanceRecord) -> io::Result<Arc<RegistryFile>> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(run_dir)?;
        let name = format!("{}-{}.json", own.pid, start_key(&own.start));
        let file = Arc::new(RegistryFile {
            path: run_dir.join(name),
            state: Mutex::new(FileState {
                body: FileBody {
                    version: VERSION,
                    instance: own,
                    entries: Vec::new(),
                },
                closed: false,
                retain_at_close: Default::default(),
            }),
            heartbeat: Mutex::new(None),
        });
        {
            let state = lock(&file.state);
            file.write(&state.body)?;
        }
        Ok(file)
    }

    fn register(&self, entry: LeftoverEntry) -> io::Result<()> {
        let mut state = lock(&self.state);
        if state.closed {
            return Err(io::Error::other("the leftover registry is closed"));
        }
        state.body.entries.retain(|known| known.pid != entry.pid);
        state.retain_at_close.remove(&entry.pid);
        state.body.entries.push(entry);
        self.write(&state.body)
    }

    fn set_escaped(&self, pid: u32, escaped: Vec<EscapedProc>) {
        let mut state = lock(&self.state);
        if state.closed {
            return;
        }
        let Some(entry) = state.body.entries.iter_mut().find(|entry| entry.pid == pid) else {
            return;
        };
        if entry.escaped == escaped {
            return;
        }
        entry.escaped = escaped;
        self.write_or_warn(&state.body);
    }

    fn unregister(&self, pid: u32) {
        let mut state = lock(&self.state);
        if state.closed {
            return;
        }
        let before = state.body.entries.len();
        state.body.entries.retain(|entry| entry.pid != pid);
        state.retain_at_close.remove(&pid);
        if state.body.entries.len() != before {
            self.write_or_warn(&state.body);
        }
    }

    /// Rewrites the file while it has entries: its modification time is the heartbeat.
    #[cfg(unix)]
    fn beat(&self) {
        let state = lock(&self.state);
        if !state.closed && !state.body.entries.is_empty() {
            self.write_or_warn(&state.body);
        }
    }

    fn close(&self) -> io::Result<()> {
        if let Some(task) = lock(&self.heartbeat).take() {
            task.abort();
        }
        let mut state = lock(&self.state);
        state.closed = true;
        let retained = state.retain_at_close.clone();
        state
            .body
            .entries
            .retain(|entry| retained.contains(&entry.pid));
        if !state.body.entries.is_empty() {
            return self.write(&state.body);
        }
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn write_or_warn(&self, body: &FileBody) {
        if let Err(error) = self.write(body) {
            tracing::warn!(path = %self.path.display(), %error, "could not update the leftover registry");
        }
    }

    /// Temp file, fsync, rename: a reader sees the old file or the new one, never half of one.
    fn write(&self, body: &FileBody) -> io::Result<()> {
        use std::io::Write;
        let text = serde_json::to_vec(body).map_err(io::Error::other)?;
        let mut temp_name = self.path.clone().into_os_string();
        temp_name.push(".tmp");
        let temp = PathBuf::from(temp_name);
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&text)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, &self.path)
    }
}

/// Rewrites the file every heartbeat interval while the registry lives.
#[cfg(unix)]
async fn heartbeat(file: std::sync::Weak<RegistryFile>) {
    loop {
        tokio::time::sleep(crate::REGISTRY_HEARTBEAT).await;
        let Some(file) = file.upgrade() else {
            return;
        };
        if tokio::task::spawn_blocking(move || file.beat())
            .await
            .is_err()
        {
            return;
        }
    }
}

/// This process, as its registry file records it.
#[cfg(unix)]
fn own_instance() -> InstanceRecord {
    let pid = std::process::id();
    InstanceRecord {
        pid,
        start: crate::ptable::start_time(pid).unwrap_or(StartTime::Unknown),
        sid: nix::unistd::getsid(None)
            .ok()
            .and_then(|sid| u32::try_from(sid.as_raw()).ok()),
        boot: crate::ptable::boot_marker(),
    }
}

/// A file-name-safe key for a start time, so a recycled instance pid gets a file of its own.
#[cfg(unix)]
fn start_key(start: &StartTime) -> String {
    match start {
        StartTime::LinuxTicks { ticks, .. } => ticks.to_string(),
        StartTime::MacLstart(lstart) => crate::ptable::start_epoch(start).map_or_else(
            || {
                lstart
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect()
            },
            |epoch| epoch.to_string(),
        ),
        StartTime::Unknown => format!("unknown-{}", crate::log::now_millis()),
    }
}

/// Stops what dead instances left running: every registry file in `run_dir` that is not `own`'s.
/// Runs once at boot on a background task.
pub async fn sweep_stale(run_dir: &Path, own: &InstanceRecord) -> SweepReport {
    #[cfg(unix)]
    {
        let run_dir = run_dir.to_path_buf();
        let own = own.clone();
        match tokio::task::spawn_blocking(move || sweep::sweep(&run_dir, &own, &sweep::RealSystem))
            .await
        {
            Ok(report) => report,
            Err(error) => SweepReport {
                errors: vec![format!("the sweep stopped: {error}")],
                ..SweepReport::default()
            },
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (run_dir, own);
        SweepReport::default()
    }
}

/// What a sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub files_removed: u32,
    /// Each killed process's pid and label.
    pub killed: Vec<(u32, String)>,
    /// Files whose instance is still running.
    pub skipped_live_owner: u32,
    /// Candidates whose pid now belongs to another process.
    pub skipped_reused: u32,
    pub errors: Vec<String>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: u32) -> LeftoverEntry {
        LeftoverEntry {
            pid,
            pgid: pid,
            sid: Some(1),
            start: StartTime::MacLstart("Sat Oct 24 09:03:00 2026".into()),
            label: format!("agent {pid}"),
            spawned_at_ms: 0,
            daemons: vec![DaemonMatch {
                program: None,
                args_in_order: vec!["gateway".into()],
            }],
            escaped: Vec::new(),
        }
    }

    #[cfg(unix)]
    fn body(path: &Path) -> FileBody {
        serde_json::from_slice(&std::fs::read(path).expect("read")).expect("parse")
    }

    #[test]
    fn a_disabled_registry_names_this_process_and_accepts_everything() {
        let registry = LeftoverRegistry::disabled();
        assert_eq!(registry.own().pid, std::process::id());
        let entry = entry(1);
        registry.register(entry.clone()).expect("register");
        registry.set_escaped(1, Vec::new());
        registry.unregister(1);
        registry.close().expect("close");
        assert!(registry.path().is_none());
        let text = serde_json::to_string(&entry).expect("serialize");
        assert_eq!(
            serde_json::from_str::<LeftoverEntry>(&text).expect("deserialize"),
            entry
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unconfirmed_shutdown_keeps_recovery_identities_until_cleanup_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry =
            LeftoverRegistry::open(&dir.path().join("run"), Handle::current()).expect("open");
        let path = registry.path().expect("registry path").to_path_buf();
        registry.register(entry(5001)).expect("register");
        let escaped = vec![EscapedProc {
            pid: 5120,
            start: StartTime::MacLstart("Sat Oct 24 09:04:00 2026".into()),
        }];
        registry
            .handle()
            .finish_shutdown(5001, escaped.clone(), false);
        let stored = body(&path);
        assert_eq!(stored.entries.len(), 1);
        assert_eq!(stored.entries[0].escaped, escaped);
        registry.handle().finish_shutdown(5001, Vec::new(), true);
        assert!(body(&path).entries.is_empty());
        registry.close().expect("close");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_shutdown_survives_an_otherwise_clean_registry_close() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry =
            LeftoverRegistry::open(&dir.path().join("run"), Handle::current()).expect("open");
        let path = registry.path().expect("registry path").to_path_buf();
        registry.register(entry(5001)).expect("failed process");
        registry.register(entry(5002)).expect("clean process");
        let escaped = vec![EscapedProc {
            pid: 5120,
            start: StartTime::MacLstart("same process".into()),
        }];
        registry
            .handle()
            .finish_shutdown(5001, escaped.clone(), false);
        registry.close().expect("close");
        let stored = body(&path);
        assert_eq!(stored.entries.len(), 1);
        assert_eq!(stored.entries[0].pid, 5001);
        assert_eq!(stored.entries[0].escaped, escaped);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_file_follows_every_change_and_goes_at_close() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = dir.path().join("run");
        let registry = LeftoverRegistry::open(&run, Handle::current()).expect("open");
        let own = registry.own().clone();
        assert_eq!(own.pid, std::process::id());
        assert_ne!(own.start, StartTime::Unknown);
        assert!(own.boot.is_some());
        let path = registry.path().expect("a file").to_path_buf();
        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!("{}-", own.pid)) && name.ends_with(".json")
                })
        );
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&run).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "the run folder is private");
        }
        assert_eq!(body(&path).entries, []);
        assert_eq!(body(&path).instance, own);

        registry.register(entry(5001)).expect("register");
        registry.register(entry(5002)).expect("register");
        let escaped = vec![EscapedProc {
            pid: 5120,
            start: StartTime::MacLstart("Sat Oct 24 09:04:00 2026".into()),
        }];
        registry.set_escaped(5001, escaped.clone());
        let written = body(&path);
        assert_eq!(written.version, 1);
        assert_eq!(written.entries.len(), 2);
        assert_eq!(written.entries[0].escaped, escaped);
        registry.unregister(5001);
        assert_eq!(body(&path).entries, [entry(5002)]);
        // Registering a pid again replaces its entry.
        let mut renamed = entry(5002);
        renamed.label = "again".into();
        registry.register(renamed.clone()).expect("register");
        assert_eq!(body(&path).entries, [renamed]);

        registry.close().expect("close");
        assert!(!path.exists());
        assert!(registry.register(entry(1)).is_err(), "closed");
        registry.unregister(5002);
        assert!(!path.exists(), "nothing writes it back");
        let leftovers: Vec<_> = std::fs::read_dir(&run).expect("dir").collect();
        assert!(leftovers.is_empty(), "no temp files left");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_registers_and_unregisters_leave_a_consistent_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry =
            LeftoverRegistry::open(&dir.path().join("run"), Handle::current()).expect("open");
        let path = registry.path().expect("file").to_path_buf();
        let mut threads = Vec::new();
        for thread in 0..8u32 {
            let registry = Arc::clone(&registry);
            let path = path.clone();
            threads.push(std::thread::spawn(move || {
                for round in 0..25u32 {
                    let pid = 10_000 + thread * 100 + round;
                    registry.register(entry(pid)).expect("register");
                    // Every read in between sees a whole file.
                    let seen = body(&path);
                    assert_eq!(seen.version, 1);
                    if round % 2 == 0 {
                        registry.unregister(pid);
                    }
                }
            }));
        }
        for thread in threads {
            thread.join().expect("join");
        }
        let mut pids: Vec<u32> = body(&path).entries.iter().map(|entry| entry.pid).collect();
        pids.sort_unstable();
        let mut expected: Vec<u32> = (0..8u32)
            .flat_map(|thread| {
                (0..25u32)
                    .filter(|round| round % 2 == 1)
                    .map(move |round| 10_000 + thread * 100 + round)
            })
            .collect();
        expected.sort_unstable();
        assert_eq!(pids, expected);
        registry.close().expect("close");
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn the_heartbeat_rewrites_the_file_while_it_has_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry =
            LeftoverRegistry::open(&dir.path().join("run"), Handle::current()).expect("open");
        let path = registry.path().expect("file").to_path_buf();
        let modified = || {
            std::fs::metadata(&path)
                .expect("meta")
                .modified()
                .expect("mtime")
        };
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let backdate = || {
            std::fs::File::options()
                .write(true)
                .open(&path)
                .expect("open")
                .set_modified(old)
                .expect("set mtime");
        };

        // Without entries the heartbeat leaves the file alone.
        backdate();
        tokio::time::sleep(crate::REGISTRY_HEARTBEAT + std::time::Duration::from_secs(1)).await;
        settle().await;
        assert_eq!(modified(), old);

        registry.register(entry(7)).expect("register");
        backdate();
        tokio::time::sleep(crate::REGISTRY_HEARTBEAT).await;
        settle().await;
        assert!(modified() > old, "rewritten as a heartbeat");
        registry.close().expect("close");
    }

    /// Lets the heartbeat's blocking write finish.
    #[cfg(unix)]
    async fn settle() {
        for _ in 0..50 {
            tokio::task::yield_now().await;
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}
