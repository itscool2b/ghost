//! The background database check after a session that did not close the database.
//!
//! Opening the store only probes the schema: `PRAGMA quick_check` reads every page (about 2 ms per
//! MB), far too slow for startup (see ghost-core's store docs). So when the last session crashed,
//! was killed, or lost power ([`StoreInfo::unclean_shutdown`]), the shell runs ghost-core's
//! [`check_integrity`] once the window has been shown, on a thread of its own: never on the main
//! thread, never in the way of startup or the first paint. ghost-core records any damage it finds
//! next to the database, and the next launch repairs it and tells the user; here the outcome is
//! only logged.
//!
//! The check reads through its own read-only connection on the database path and never uses the
//! app's [`Store`], so closing the store at exit neither waits for it nor pulls anything from under
//! it. At exit ([`IntegrityCheck::close`], before the store closes) a check that has not begun
//! never will, and one still running is left to end with the process: its thread is never joined,
//! Tauri ends the process with `std::process::exit`, which does not wait for other threads, and a
//! read-only connection cut off mid-read changes nothing on disk. While that connection is open,
//! though, the store's close cannot fold the write-ahead log back into the file, so the next launch
//! sees an unclean shutdown again and checks once more.
//!
//! [`StoreInfo::unclean_shutdown`]: ghost_core::store::StoreInfo::unclean_shutdown
//! [`check_integrity`]: ghost_core::store::check_integrity
//! [`Store`]: ghost_core::store::Store

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use ghost_core::store::Integrity;

use crate::sync::lock;

/// The check's thread, as debuggers and crash reports name it.
pub const THREAD_NAME: &str = "integrity-check";

/// Which database to check once the window is up, if any: the store's file, but only after a
/// session that did not close it. A clean close leaves nothing a crash could have cut short, and
/// checking on every launch would read the whole file each time.
pub fn target(db_path: &Path, unclean_shutdown: bool) -> Option<PathBuf> {
    unclean_shutdown.then(|| db_path.to_path_buf())
}

/// Where this launch's check stands. Shared by the window, which releases it, the check's thread,
/// and the exit.
#[derive(Debug, PartialEq, Eq)]
enum Phase {
    /// Check this database once the window has been shown.
    Pending(PathBuf),
    /// Handed to its thread, which has not begun the check yet.
    Released,
    /// The check is running.
    Running,
    /// Nothing (more) to do: there was nothing to check, the check is over, or the app is exiting.
    Over,
}

/// This launch's background check: at most one, started once the window is on screen.
#[derive(Debug)]
pub struct IntegrityCheck {
    phase: Arc<Mutex<Phase>>,
}

impl IntegrityCheck {
    /// A check of `target` (see [`target`]), or none.
    pub fn new(target: Option<PathBuf>) -> IntegrityCheck {
        IntegrityCheck {
            phase: Arc::new(Mutex::new(target.map_or(Phase::Over, Phase::Pending))),
        }
    }

    /// The window is on screen: starts the pending check on its own thread. Only the first call
    /// does anything, so everything that shows the window may call it. Returns whether a check
    /// started. The thread is detached on purpose (see the module docs).
    pub fn window_shown(&self) -> bool {
        let Some(job) = self.release() else {
            return false;
        };
        match spawn(job, ghost_core::store::check_integrity) {
            Ok(_detached) => true,
            Err(error) => {
                tracing::warn!(%error, "could not start the integrity check");
                false
            }
        }
    }

    /// At exit, before the store closes: a check that has not begun never will. Never waits for
    /// one that is running; returns whether one is (it ends with the process).
    pub fn close(&self) -> bool {
        let mut phase = lock(&self.phase);
        if *phase == Phase::Running {
            return true;
        }
        *phase = Phase::Over;
        false
    }

    /// The database a check is waiting to read, before the window has been shown.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> Option<PathBuf> {
        match &*lock(&self.phase) {
            Phase::Pending(path) => Some(path.clone()),
            _ => None,
        }
    }

    /// Hands the pending check to whoever runs it, once.
    fn release(&self) -> Option<Job> {
        let mut phase = lock(&self.phase);
        match std::mem::replace(&mut *phase, Phase::Released) {
            Phase::Pending(path) => Some(Job {
                path,
                phase: Arc::clone(&self.phase),
            }),
            other => {
                *phase = other;
                None
            }
        }
    }
}

/// A released check, for its thread to run.
#[derive(Debug)]
struct Job {
    path: PathBuf,
    phase: Arc<Mutex<Phase>>,
}

impl Job {
    /// Runs `check` on the database, unless the exit closed the check first. Returns the outcome,
    /// or `None` when the check did not run.
    fn run(
        self,
        check: impl FnOnce(&Path) -> ghost_core::Result<Integrity>,
    ) -> Option<ghost_core::Result<Integrity>> {
        {
            let mut phase = lock(&self.phase);
            if *phase != Phase::Released {
                return None;
            }
            *phase = Phase::Running;
        }
        // Over however the check ends, a panic included, so a check that died never counts as
        // running.
        let _over = MarkOver(&self.phase);
        Some(check(&self.path))
    }
}

/// Marks the check over when dropped.
struct MarkOver<'a>(&'a Mutex<Phase>);

impl Drop for MarkOver<'_> {
    fn drop(&mut self) {
        *lock(self.0) = Phase::Over;
    }
}

/// Runs `job` with `check` on a thread of its own and logs the outcome. Generic over `check` so
/// tests can stand in for the real one.
fn spawn(
    job: Job,
    check: impl FnOnce(&Path) -> ghost_core::Result<Integrity> + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(THREAD_NAME.to_string())
        .spawn(move || {
            let db = job.path.clone();
            let outcome = job.run(|path| {
                tracing::info!(
                    db = %path.display(),
                    "the last session did not close the database; checking it in the background"
                );
                check(path)
            });
            if let Some(outcome) = outcome {
                log_outcome(&db, &outcome);
            }
        })
}

/// Logs what the check found. Damage is a warning rather than an error, since ghost-core recorded
/// it and the next launch repairs it; a check that could not run is an error, since nothing was
/// checked.
fn log_outcome(db: &Path, outcome: &ghost_core::Result<Integrity>) {
    match outcome {
        Ok(Integrity::Ok) => {
            tracing::info!(db = %db.display(), "the integrity check found no damage");
        }
        Ok(Integrity::Damaged { problem }) => {
            tracing::warn!(
                db = %db.display(),
                %problem,
                "the integrity check found damage; the next launch repairs the database"
            );
        }
        Err(error) => {
            tracing::error!(db = %db.display(), %error, "the integrity check could not run");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use ghost_core::paths::DataRoot;
    use ghost_core::store::OpenOutcome;

    use crate::boot::{Booted, boot, boot_next};

    /// Generous: the machines that run these tests can be heavily loaded.
    const PATIENCE: Duration = Duration::from_secs(60);

    fn db() -> PathBuf {
        PathBuf::from("/data/profiles/local/aura.db")
    }

    fn fresh() -> (tempfile::TempDir, DataRoot) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = DataRoot::at(dir.path().join("data"));
        (dir, root)
    }

    fn db_path(booted: &Booted) -> PathBuf {
        booted.store.as_ref().expect("store").path().to_path_buf()
    }

    fn is_over(check: &IntegrityCheck) -> bool {
        *lock(&check.phase) == Phase::Over
    }

    /// A log destination the test can read back.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            lock(&self.0).extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Everything logged while `f` runs on this thread, without colours or timestamps.
    fn logs(f: impl FnOnce()) -> String {
        let capture = Capture::default();
        let writer = capture.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::TRACE)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        String::from_utf8_lossy(&lock(&capture.0)).into_owned()
    }

    #[test]
    fn only_a_session_that_did_not_close_the_database_is_checked() {
        assert_eq!(target(&db(), true), Some(db()));
        assert_eq!(target(&db(), false), None);
    }

    #[test]
    fn nothing_to_check_starts_nothing() {
        let check = IntegrityCheck::new(None);
        assert_eq!(check.pending(), None);
        assert!(check.release().is_none());
        assert!(!check.window_shown());
        assert!(!check.close());
    }

    #[test]
    fn the_check_is_released_once() {
        let check = IntegrityCheck::new(Some(db()));
        assert_eq!(check.pending(), Some(db()));
        let job = check.release().expect("released");
        assert_eq!(check.pending(), None);
        assert!(
            check.release().is_none(),
            "showing the window again starts nothing"
        );
        let outcome = job.run(|path| {
            assert_eq!(path, db(), "the check reads the store's file");
            Ok(Integrity::Ok)
        });
        assert!(matches!(outcome, Some(Ok(Integrity::Ok))), "{outcome:?}");
        assert!(is_over(&check));
        assert!(!check.close(), "nothing is running any more");
    }

    #[test]
    fn a_check_closed_before_it_began_never_runs() {
        let check = IntegrityCheck::new(Some(db()));
        let job = check.release().expect("released");
        assert!(!check.close(), "released, but not running yet");
        assert!(
            job.run(|_| panic!("the check ran after the exit closed it"))
                .is_none()
        );
        assert!(is_over(&check));
    }

    #[test]
    fn a_closed_check_never_starts_later() {
        let check = IntegrityCheck::new(Some(db()));
        assert!(!check.close());
        assert!(check.release().is_none());
        assert!(
            !check.window_shown(),
            "a window shown during the exit starts nothing"
        );
    }

    #[test]
    fn closing_does_not_wait_for_a_running_check() {
        let check = Arc::new(IntegrityCheck::new(Some(db())));
        let job = check.release().expect("released");
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel::<()>();
        let thread = spawn(job, move |_| {
            started_tx.send(()).expect("report the start");
            // Blocks until the test lets go: a close that waited would never return.
            finish_rx.recv().expect("wait for the test");
            Ok(Integrity::Ok)
        })
        .expect("spawn");
        started_rx
            .recv_timeout(PATIENCE)
            .expect("the check started");

        let (closed_tx, closed_rx) = mpsc::channel();
        let closer = Arc::clone(&check);
        std::thread::spawn(move || closed_tx.send(closer.close()));
        let still_running = closed_rx
            .recv_timeout(PATIENCE)
            .expect("close returned while the check was still running");
        assert!(still_running, "close reports the running check");

        finish_tx.send(()).expect("let the check finish");
        thread.join().expect("the check's thread ended");
        assert!(is_over(&check));
        assert!(!check.close());
    }

    #[test]
    fn a_check_that_panics_does_not_count_as_running() {
        let check = IntegrityCheck::new(Some(db()));
        let job = check.release().expect("released");
        let thread = spawn(job, |_| panic!("a bug in the check")).expect("spawn");
        assert!(thread.join().is_err(), "the panic ended the thread");
        assert!(is_over(&check));
        assert!(!check.close());
    }

    #[test]
    fn damage_is_a_warning_and_a_check_that_could_not_run_is_an_error() {
        let db = db();
        let clean = logs(|| log_outcome(&db, &Ok(Integrity::Ok)));
        assert!(
            clean.contains("INFO") && clean.contains("found no damage"),
            "{clean}"
        );
        assert!(
            !clean.contains("WARN") && !clean.contains("ERROR"),
            "{clean}"
        );

        let problem = "Page 7: btreeInitPage() returns error code 11";
        let damaged = logs(|| {
            log_outcome(
                &db,
                &Ok(Integrity::Damaged {
                    problem: problem.into(),
                }),
            );
        });
        assert!(
            damaged.contains("WARN") && damaged.contains(problem),
            "{damaged}"
        );
        assert!(damaged.contains(&db.display().to_string()), "{damaged}");

        let failed = logs(|| log_outcome(&db, &Err(ghost_core::CoreError::NoDataDir)));
        assert!(
            failed.contains("ERROR") && failed.contains("GHOST_DATA_DIR"),
            "{failed}"
        );
    }

    #[test]
    fn showing_the_window_starts_the_real_check_once() {
        let (_dir, root) = fresh();
        let booted = boot(Ok(root.clone()));
        let db = db_path(&booted);
        let check = IntegrityCheck::new(Some(db));
        assert!(check.window_shown());
        assert!(!check.window_shown(), "only the first call starts it");
        let started = Instant::now();
        while !is_over(&check) {
            assert!(started.elapsed() < PATIENCE, "the check never finished");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(booted);
        let next = boot_next(&root);
        assert_eq!(
            next.db.expect("db").outcome,
            OpenOutcome::Opened,
            "a healthy database is left alone"
        );
    }

    /// End to end, with damage the opening probe cannot see: the check runs on its own thread,
    /// reads the store's file while the store is open, and the next launch repairs what it found.
    #[test]
    fn the_check_records_damage_the_probe_missed_for_the_next_launch_to_repair() {
        let (_dir, root) = fresh();
        let first = boot(Ok(root.clone()));
        let db = db_path(&first);
        // A table's root page: the probe reads only the schema, quick_check reads every page.
        let (page, page_size) = {
            let conn = first.store.as_ref().expect("store").conn();
            let page: i64 = conn
                .query_row(
                    "SELECT max(rootpage) FROM sqlite_schema WHERE type = 'table'",
                    [],
                    |row| row.get(0),
                )
                .expect("a table's root page");
            let page_size: i64 = conn
                .query_row("PRAGMA page_size", [], |row| row.get(0))
                .expect("page size");
            (page, page_size)
        };
        assert!(page > 1, "not the schema page");
        drop(first);
        let mut bytes = std::fs::read(&db).expect("read");
        let start = usize::try_from((page - 1) * page_size).expect("offset");
        bytes[start..start + 16].fill(0xA5);
        std::fs::write(&db, bytes).expect("damage a page");

        let second = boot_next(&root);
        assert_eq!(
            second.db.as_ref().expect("db").outcome,
            OpenOutcome::Opened,
            "the probe does not read that page"
        );
        let check = IntegrityCheck::new(Some(db_path(&second)));
        let job = check.release().expect("released");
        let caller = std::thread::current().id();
        let (seen_tx, seen_rx) = mpsc::channel();
        spawn(job, move |path| {
            let thread = std::thread::current();
            let outcome = ghost_core::store::check_integrity(path);
            let damaged = matches!(outcome, Ok(Integrity::Damaged { .. }));
            seen_tx
                .send((thread.id(), thread.name().map(str::to_string), damaged))
                .expect("report");
            outcome
        })
        .expect("spawn")
        .join()
        .expect("the check's thread ended");
        let (thread, name, damaged) = seen_rx.recv().expect("the check ran");
        assert_ne!(thread, caller, "never on the thread that showed the window");
        assert_eq!(name.as_deref(), Some(THREAD_NAME));
        assert!(damaged, "the check found the damaged page");
        drop(second);

        let third = boot_next(&root);
        assert_eq!(third.fatal, None);
        assert!(
            matches!(
                third.db.expect("db").outcome,
                OpenOutcome::RecoveredFromCorruption { .. }
            ),
            "the next launch repaired what the check recorded"
        );
        assert!(third.recovery.is_some(), "and reports it until dismissed");
    }
}
