//! The per-profile SQLite store.
//!
//! Whatever is on disk, [`Store::open`] returns either a working, fully migrated database or an
//! error, and it never destroys data:
//!
//! 1. Opening is coordinated across processes through `<db>.lock` (see `store/lock.rs`): one
//!    opener at a time holds it exclusively while it probes, repairs, and migrates, and every open
//!    store keeps a shared hold. So two processes creating a profile at once can't both run
//!    migration 1, and the file is only ever repaired (moved) or migrated when no other o7
//!    process has it open.
//! 2. An existing file gets a cheap probe: `user_version` first, then a read of `sqlite_schema`,
//!    which catches a file that is not a database and a damaged header or schema page while reading
//!    only the schema. `PRAGMA quick_check` reads every page (about 2 ms per MB), far too slow for
//!    startup, so [`check_integrity`] runs it on its own connection, meant for a background thread
//!    (for example after the window is up, when [`StoreInfo::unclean_shutdown`] says the last
//!    session didn't close the database).
//! 3. Damage, whether the probe found it, [`check_integrity`] did, or a query failed with
//!    SQLITE_CORRUPT or SQLITE_NOTADB ([`Store::note_query_error`]), is repaired at the next open
//!    that holds the lock exclusively, instead of the whole history disappearing: everything that
//!    can still be read is copied into a new database (`VACUUM INTO`, or, when that fails, table by
//!    table and row by row into the schema the migrations give that version), the damaged file and
//!    its `-wal`, `-shm`, and `-journal` are moved to `backups/aura.db.corrupt-<millis>`, and the
//!    copy takes its place. A record next to the moved file reports the repair as
//!    [`OpenOutcome::RecoveredFromCorruption`] on every open until [`Store::acknowledge_recovery`],
//!    so the notice can't be lost, and a repair interrupted after the move is finished by the next
//!    open. Only "not a database" and "corrupt" count as damage: a locked or unreadable file is an
//!    error and stays where it is.
//! 4. A database from a newer o7 (`user_version` above [`SCHEMA_VERSION`]) is refused with
//!    [`CoreError::SchemaTooNew`] before anything is written or repaired. The version is read before
//!    anything else on purpose: a newer build may use schema syntax this SQLite can't parse, which
//!    would look like damage, and an older app must never quarantine a newer app's data.
//! 5. Connection settings: WAL (readers never block the writer), `synchronous=NORMAL`, a 5 s busy
//!    timeout, foreign keys on, and `checkpoint_fullfsync`. With WAL and NORMAL, a power loss can
//!    drop the latest commits but can't corrupt the file, provided the writes of a checkpoint reach
//!    the disk before the WAL is reset. On macOS fsync() doesn't flush the drive's write cache, so
//!    without F_FULLFSYNC the reset could survive a power loss while the checkpoint's writes don't.
//!    `checkpoint_fullfsync` makes each checkpoint end with one F_FULLFSYNC (cheap: checkpoints are
//!    rare next to commits); elsewhere it does nothing.
//! 6. An existing database is copied with SQLite's online backup API to
//!    `backups/aura.db.v<from>-<millis>.bak`, synced, before it is migrated, keeping the five newest
//!    backups and never the one just made. No backup, no migration.
//! 7. All pending migrations run in one `BEGIN IMMEDIATE` transaction with foreign keys off and a
//!    `foreign_key_check` before commit; any failure rolls everything back, `user_version` included.

mod encrypted;
mod lock;
mod recovery;
mod salvage;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::config::DbConfig;
use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior};
use rusqlite_migration::{M, Migrations};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, InUseReason, Result};
use crate::fsutil::{self, PARTIAL_SUFFIX, SQLITE_SIDECARS};
use crate::profile::Profile;
use crate::time::now_millis;

use lock::OpenLock;
use recovery::Recovery;

/// The schema version this build reads and writes; equals the number of migrations.
pub const SCHEMA_VERSION: u32 = 22;

/// Pre-migration backups kept per database. Enough to step back through several upgrades without
/// letting a long-lived install fill the disk with copies.
pub const BACKUPS_TO_KEEP: usize = 5;

/// How long a statement waits for another connection (the CLI, a second window) to release a lock
/// before failing with "database is locked", and how long an open waits for another process to
/// finish opening the same database.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const BACKUP_SUFFIX: &str = ".bak";
const DEFAULT_DB_NAME: &str = "aura.db";

/// Every schema change, in order: entry `i` takes the database from version `i` to `i + 1`. The array
/// length is [`SCHEMA_VERSION`], so adding a migration without bumping the version fails to compile.
/// Each migration ends with a foreign key check, because migrations run with enforcement off.
/// Lazy because a migration with a Rust hook is built at run time.
static MIGRATIONS: std::sync::LazyLock<[M<'static>; SCHEMA_VERSION as usize]> =
    std::sync::LazyLock::new(|| {
        [
            M::up(include_str!("../migrations/0001_initial.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0002_sessions.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0003_agent_hosts.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0004_composer.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0005_drafts.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0006_continuations.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0007_sync.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0008_worktree_setup.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0009_config_sync.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0010_steering.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0011_sync_projection.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0012_transcript_sync.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0013_local_import.sql")).foreign_key_check(),
            M::up_with_hook(
                include_str!("../migrations/0014_plain_folder_paths.sql"),
                plain_folder_paths,
            )
            .foreign_key_check(),
            M::up(include_str!(
                "../migrations/0015_worktree_setup_attempts.sql"
            ))
            .foreign_key_check(),
            M::up(include_str!("../migrations/0016_session_controls.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0017_project_memory.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0018_shared_mcp.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0019_coordinator.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0020_chat_roles.sql")).foreign_key_check(),
            M::up(include_str!("../migrations/0021_line_edits.sql")).foreign_key_check(),
            M::up_with_hook(
                include_str!("../migrations/0022_memory_notes.sql"),
                split_long_memory,
            )
            .foreign_key_check(),
        ]
    });

fn migrations() -> &'static [M<'static>] {
    &*MIGRATIONS
}

/// Migration 22: a project's memory that is too long for a brief is split into a brief and notes
/// at its headings, without losing a character ([`crate::chatstore::memory_notes::split`]).
fn split_long_memory(tx: &rusqlite::Transaction) -> rusqlite_migration::HookResult {
    let split = crate::chatstore::memory_notes::split_all(tx, now_millis())?;
    if split > 0 {
        tracing::info!(
            projects = split,
            "split project memory into a brief and notes"
        );
    }
    Ok(())
}

/// Migration 14: every saved folder location that is a Windows verbatim path takes its plain
/// spelling when that names the same folder ([`fsutil::plain_windows_path`]); any other path is
/// left as it is. Only `folder_locations.path` holds folder paths.
fn plain_folder_paths(tx: &rusqlite::Transaction) -> rusqlite_migration::HookResult {
    let rows: Vec<(String, String, String)> = {
        let mut statement = tx.prepare(
            "SELECT folder_id, device_id, path FROM folder_locations WHERE substr(path, 1, 4) = '\\\\?\\'",
        )?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (folder, device, path) in rows {
        if let Some(plain) = fsutil::plain_windows_path(&path) {
            tx.execute(
                "UPDATE folder_locations SET path = ?3 WHERE folder_id = ?1 AND device_id = ?2",
                rusqlite::params![folder, device, plain],
            )?;
        }
    }
    Ok(())
}

/// What [`Store::open`] had to do. Paths are strings so the UI can show them as they are.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpenOutcome {
    /// No database existed; a new one was created at the current schema.
    Created,
    /// The database was already at the current schema.
    Opened,
    /// The database was upgraded. `backup` is the copy taken first.
    Migrated {
        from: u32,
        to: u32,
        backup: Option<String>,
    },
    /// The database was damaged. It was moved to `quarantined`, where it is kept, and everything
    /// that could still be read from it was copied into the database that replaced it; `salvage`
    /// says how much that was. Reported on every open until [`Store::acknowledge_recovery`], so the
    /// UI can't miss it: whatever the copy lacks is only in the quarantined file.
    RecoveredFromCorruption {
        quarantined: String,
        salvage: Salvage,
    },
}

/// How much of a damaged database made it into the database that replaced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Salvage {
    /// Every table read back without an error, so nothing is missing: the damage was in pages no
    /// row lives in (unused pages, a broken index), which the copy rebuilt.
    Complete,
    /// Some rows could not be read, or were dropped because the row they belong to was lost (a
    /// chat's events, when the chat's page was unreadable), or lost a reference to such a row (a
    /// chat whose folder was lost stays in its project, outside any folder). `tables` names those
    /// tables, in schema order. It is empty when which ones isn't known: part of the schema itself
    /// was unreadable (and may have listed tables o7 doesn't know of), or the record of the
    /// repair could not be read.
    Partial { tables: Vec<String> },
    /// Nothing could be read, so the new database started out empty.
    Nothing,
}

/// Facts about an opened store.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreInfo {
    pub path: PathBuf,
    pub schema_version: u32,
    pub outcome: OpenOutcome,
    /// The previous session did not close the database: it crashed, was killed, or lost power, so
    /// its `-wal` still held data (or its `-journal` was left behind). A good reason to run
    /// [`check_integrity`] in the background. Only known when this open had the database to itself;
    /// false when another o7 process had it open.
    pub unclean_shutdown: bool,
}

/// What [`check_integrity`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Integrity {
    Ok,
    /// The check found damage. It was recorded next to the database, and the next [`Store::open`]
    /// that has the database to itself repairs it (dropping the store and opening it again does
    /// that right away).
    Damaged {
        problem: String,
    },
}

/// An open, migrated profile database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    path: PathBuf,
    backups_dir: PathBuf,
    /// The shared hold on `<db>.lock`, released when the store drops, which tells other processes
    /// the database is in use (see `store/lock.rs`). Declared after `conn` because fields drop in
    /// declaration order: the connection must finish closing (and checkpointing) before another
    /// process may repair or migrate the file.
    _lock: Option<lock::StoreHold>,
}

impl Store {
    /// Opens (creating, migrating, or repairing as needed) the profile's `aura.db`, with backups in
    /// the profile's `backups/` directory.
    pub fn open(profile: &Profile) -> Result<(Store, StoreInfo)> {
        Store::open_at(&profile.db_path(), &profile.backups_dir())
    }

    /// Call on a blocking worker because it reads all data pages.
    pub fn check_integrity(&self) -> Result<Integrity> {
        check_integrity(&self.path)
    }

    /// [`Store::open`] for an explicit database path and backups directory. `backups_dir` should be on
    /// the same filesystem as the database, because damaged files are moved there by renaming.
    pub fn open_at(db_path: &Path, backups_dir: &Path) -> Result<(Store, StoreInfo)> {
        open_with(db_path, backups_dir, migrations())
    }

    /// The schema version stored in the database (`PRAGMA user_version`).
    pub fn schema_version(&self) -> Result<u32> {
        stored_version(&self.conn)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Mutable access, for `Connection::transaction`.
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn backups_dir(&self) -> &Path {
        &self.backups_dir
    }

    /// Call with an error a query on this store returned. When it means the file is damaged
    /// (SQLITE_CORRUPT or SQLITE_NOTADB), that is recorded next to the database, so the next open
    /// repairs it, and this returns true. Any other error is the caller's to handle; this returns
    /// false. Encrypted damage returns an error without scheduling plaintext recovery.
    pub fn note_query_error(&self, error: &rusqlite::Error) -> Result<bool> {
        if !is_damage(error) {
            return Ok(false);
        }
        recovery::mark_damaged(&self.path, &error.to_string())?;
        Ok(true)
    }

    /// Stops reporting the repair that moved the damaged database to `quarantined` (the path from
    /// [`OpenOutcome::RecoveredFromCorruption`]), once the user has seen it. The quarantined file
    /// itself is kept. Returns whether such a repair was pending.
    pub fn acknowledge_recovery(&self, quarantined: &str) -> Result<bool> {
        recovery::acknowledge(&self.path, &self.backups_dir, quarantined)
    }
}

/// Runs `PRAGMA quick_check` on a separate read-only connection and records any damage it finds, so
/// the next [`Store::open`] repairs the database. It reads every page (about 2 ms per MB), so call
/// it from a background thread; in WAL mode it doesn't block the store, which keeps writing.
pub fn check_integrity(db_path: &Path) -> Result<Integrity> {
    check_integrity_with(db_path, SCHEMA_VERSION)
}

fn check_integrity_with(db_path: &Path, supported: u32) -> Result<Integrity> {
    encrypted::require_plain(db_path)?;
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let checked = quick_check(&conn, supported);
    close_without_checkpoint(conn);
    let Some(problem) = checked? else {
        return Ok(Integrity::Ok);
    };
    recovery::mark_damaged(db_path, &problem)?;
    Ok(Integrity::Damaged { problem })
}

/// The first problem `quick_check` reports, if any. Data from a newer o7 is not judged: its
/// schema may simply be beyond this SQLite.
fn quick_check(conn: &Connection, supported: u32) -> Result<Option<String>> {
    let found = match read_user_version(conn) {
        Ok(raw) => version_from_raw(raw)?,
        Err(error) if is_damage(&error) => return Ok(Some(error.to_string())),
        Err(error) => return Err(error.into()),
    };
    if found > supported {
        return Err(CoreError::SchemaTooNew { found, supported });
    }
    // quick_check(1) stops at the first problem; one is enough to decide. Damage can also surface as
    // an error instead of a result row.
    match conn.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0)) {
        Ok(result) if result.eq_ignore_ascii_case("ok") => Ok(None),
        Ok(problem) => Ok(Some(problem)),
        Err(error) if is_damage(&error) => Ok(Some(error.to_string())),
        Err(error) => Err(error.into()),
    }
}

/// Only these two codes mean the bytes themselves are bad. Busy, locked, permission, and I/O errors
/// are about the environment, and treating a healthy database as damaged over them would hide the
/// user's data for nothing.
pub fn is_damage(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
    )
}

/// Whether an open coordinates through the open lock. Off only in tests, to stand in for a process
/// that doesn't know about it (an older build, another tool).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Locking {
    On,
    #[cfg(test)]
    Off,
}

/// [`Store::open_at`] with an explicit migration list, so tests can run synthetic upgrades.
pub(crate) fn open_with(
    db_path: &Path,
    backups_dir: &Path,
    migrations: &[M<'_>],
) -> Result<(Store, StoreInfo)> {
    open_inner(db_path, backups_dir, migrations, BUSY_TIMEOUT, Locking::On)
}

fn open_inner(
    db_path: &Path,
    backups_dir: &Path,
    migrations: &[M<'_>],
    busy_timeout: Duration,
    locking: Locking,
) -> Result<(Store, StoreInfo)> {
    encrypted::require_plain(db_path)?;
    let supported = u32::try_from(migrations.len()).map_err(|_| {
        CoreError::Migration(format!(
            "{} migrations is more than SQLite can track",
            migrations.len()
        ))
    })?;
    fsutil::create_private_dir_all(fsutil::parent_dir(db_path))?;
    let lock = match locking {
        Locking::On => Some(OpenLock::acquire(db_path, busy_timeout)?),
        #[cfg(test)]
        Locking::Off => None,
    };
    // A keyed opener may have created its marker while this process waited for the open lock.
    encrypted::require_plain(db_path)?;
    let ctx = Ctx {
        db_path,
        backups_dir,
        migrations,
        supported,
        busy_timeout,
        may_change: lock.as_ref().is_none_or(OpenLock::may_change),
    };
    let (conn, info) = open_locked(&ctx)?;
    let hold = lock.and_then(|lock| lock.into_store_hold(busy_timeout));
    Ok((
        Store {
            conn,
            path: db_path.to_path_buf(),
            backups_dir: backups_dir.to_path_buf(),
            _lock: hold,
        },
        info,
    ))
}

/// Everything the steps of one open need.
struct Ctx<'a, 'm> {
    db_path: &'a Path,
    backups_dir: &'a Path,
    migrations: &'a [M<'m>],
    supported: u32,
    busy_timeout: Duration,
    /// This open holds the lock exclusively (or locking doesn't work here), so it may repair and
    /// migrate. False when another o7 process has the database open.
    may_change: bool,
}

/// Why a step of an open failed: damage, which a repair can fix, or anything else.
enum Failure {
    Damage(String),
    Error(CoreError),
}

impl From<CoreError> for Failure {
    fn from(error: CoreError) -> Failure {
        Failure::Error(error)
    }
}

/// Classifies a SQLite error from a step of an open.
fn sql<T>(result: rusqlite::Result<T>) -> std::result::Result<T, Failure> {
    result.map_err(|error| {
        if is_damage(&error) {
            Failure::Damage(error.to_string())
        } else {
            Failure::Error(error.into())
        }
    })
}

/// A database that passed every step.
struct Opened {
    conn: Connection,
    from: u32,
    schema_version: u32,
    backup: Option<PathBuf>,
}

fn open_locked(ctx: &Ctx<'_, '_>) -> Result<(Connection, StoreInfo)> {
    // Before anything opens the file, since an open connection keeps a -wal around.
    let unclean_shutdown = ctx.may_change && left_unclean(ctx.db_path);
    let mut repair = None;
    if ctx.may_change {
        remove_stale_backup_partials(ctx.backups_dir, &db_file_name(ctx.db_path));
        repair = recovery::resume(ctx.db_path, ctx.backups_dir)?;
    }
    loop {
        let failure = match attempt(ctx) {
            Ok(opened) => return Ok(finish(ctx, opened, repair, unclean_shutdown)),
            Err(failure) => failure,
        };
        match (failure, &repair) {
            (Failure::Damage(problem), None) if ctx.may_change => {
                repair = Some(recovery::recover(
                    ctx.db_path,
                    ctx.backups_dir,
                    &problem,
                    ctx.migrations,
                )?);
            }
            (Failure::Damage(problem), repaired) => {
                // Recorded either way: for the open that may repair it, or for another try after a
                // repair that didn't hold.
                if let Err(error) = recovery::mark_damaged(ctx.db_path, &problem) {
                    tracing::warn!(%error, "could not record the damage for the next open");
                }
                let path = ctx.db_path.to_path_buf();
                return Err(match repaired {
                    Some(repair) => recovery::incomplete(
                        ctx.db_path,
                        &repair.quarantined,
                        CoreError::Damaged { path, problem },
                    ),
                    None => CoreError::DatabaseInUse {
                        path,
                        reason: InUseReason::Repair,
                    },
                });
            }
            (Failure::Error(error), Some(repair)) => {
                return Err(recovery::incomplete(
                    ctx.db_path,
                    &repair.quarantined,
                    error,
                ));
            }
            (Failure::Error(error), None) => return Err(error),
        }
    }
}

/// One pass at opening: probe, settings, migrations.
fn attempt(ctx: &Ctx<'_, '_>) -> std::result::Result<Opened, Failure> {
    let recorded = recovery::recorded_damage(ctx.db_path);
    let mut conn = if is_non_empty_file(ctx.db_path) {
        let conn = probe(ctx)?;
        if let Some(problem) = recorded {
            // Found by a check or a query, in pages the probe doesn't read.
            close_without_checkpoint(conn);
            return Err(Failure::Damage(problem));
        }
        conn
    } else {
        if recorded.is_some() && ctx.may_change {
            // The damaged file is gone; nothing left to repair.
            recovery::clear_recorded_damage(ctx.db_path);
        }
        // Missing or empty. SQLite itself discards a leftover -wal or -journal when the main file is
        // missing or empty, so rows from a deleted database can't resurface here.
        connect(ctx.db_path, ctx.busy_timeout)?
    };
    match prepare(ctx, &mut conn) {
        Ok((from, schema_version, backup)) => Ok(Opened {
            conn,
            from,
            schema_version,
            backup,
        }),
        Err(failure) => {
            // A damaged file is about to be moved and a newer one must stay byte-identical, so this
            // connection must not checkpoint anything into the file on its way out.
            close_without_checkpoint(conn);
            Err(failure)
        }
    }
}

/// Connection settings and migrations. Returns the version found, the version now, and the
/// pre-migration backup.
fn prepare(
    ctx: &Ctx<'_, '_>,
    conn: &mut Connection,
) -> std::result::Result<(u32, u32, Option<PathBuf>), Failure> {
    enable_wal(conn, ctx)?;
    sql(conn.pragma_update(None, "synchronous", "NORMAL"))?;
    // Durability on macOS; see the module docs. Per connection, and a no-op elsewhere.
    sql(conn.pragma_update(None, "checkpoint_fullfsync", true))?;

    let from = version(conn)?;
    if from > ctx.supported {
        // Only reachable if a process ignoring the open lock migrated the file since the probe.
        return Err(CoreError::SchemaTooNew {
            found: from,
            supported: ctx.supported,
        }
        .into());
    }
    let mut backup = None;
    if from < ctx.supported {
        if !ctx.may_change {
            return Err(CoreError::DatabaseInUse {
                path: ctx.db_path.to_path_buf(),
                reason: InUseReason::Upgrade,
            }
            .into());
        }
        backup = upgrade(ctx, conn, from)?;
    }
    sql(conn.pragma_update(None, "foreign_keys", true))?;
    Ok((from, version(conn)?, backup))
}

/// Switches to WAL. That needs an exclusive lock for a moment, and when another connection holds a
/// shared lock and wants to write too, SQLite answers SQLITE_BUSY at once instead of waiting out the
/// busy timeout (both waiting would deadlock), so this retries until the timeout.
fn enable_wal(conn: &Connection, ctx: &Ctx<'_, '_>) -> std::result::Result<(), Failure> {
    let deadline = Instant::now() + ctx.busy_timeout;
    loop {
        match conn
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))
        {
            Ok(mode) => {
                if !mode.eq_ignore_ascii_case("wal") {
                    // Some network filesystems can't do WAL; SQLite then keeps its rollback journal,
                    // which still works, just with readers blocking the writer.
                    tracing::warn!(journal_mode = %mode, path = %ctx.db_path.display(), "SQLite could not enable WAL; continuing without it");
                }
                return Ok(());
            }
            Err(error) if is_busy(&error) && Instant::now() < deadline => {
                std::thread::sleep(lock::POLL);
            }
            Err(error) => return sql(Err(error)),
        }
    }
}

fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// Backs up, then migrates. The loop only repeats when a process that ignores the open lock
/// migrated the database between this open's version read and its migration transaction: the
/// migration then fails on work already done, and continues from where the other one got to.
fn upgrade(
    ctx: &Ctx<'_, '_>,
    conn: &mut Connection,
    from: u32,
) -> std::result::Result<Option<PathBuf>, Failure> {
    let mut current = from;
    loop {
        let backup = if current > 0 {
            Some(backup_database(conn, ctx, current)?)
        } else {
            None
        };
        let failure = match migrate(conn, ctx.migrations, current, ctx.supported) {
            Ok(()) => return Ok(backup),
            Err(failure) => failure,
        };
        let now = version(conn)?;
        if now <= current || now > ctx.supported {
            return Err(failure);
        }
        tracing::info!(
            from = current,
            now,
            "another process upgraded the database meanwhile"
        );
        if now == ctx.supported {
            return Ok(backup);
        }
        current = now;
    }
}

/// Runs every pending migration in one transaction (rusqlite_migration's `to_latest`).
fn migrate(
    conn: &mut Connection,
    migrations: &[M<'_>],
    from: u32,
    to: u32,
) -> std::result::Result<(), Failure> {
    // PRAGMA foreign_keys is a no-op inside a transaction, so it is switched here, around the migration
    // transaction. With enforcement on, rebuilding a referenced table (create, copy, drop, rename; the
    // procedure SQLite documents for schema changes ALTER TABLE can't do) fails at the DROP.
    sql(conn.pragma_update(None, "foreign_keys", false))?;
    // BEGIN IMMEDIATE takes the write lock before the first statement, so a concurrent migration
    // makes this one wait instead of interleaving with it.
    conn.set_transaction_behavior(TransactionBehavior::Immediate);
    let migrated = Migrations::from_slice(migrations).to_latest(conn);
    conn.set_transaction_behavior(TransactionBehavior::Deferred);
    let restored = conn.pragma_update(None, "foreign_keys", true);
    match migrated {
        Ok(()) => {
            sql(restored)?;
            tracing::info!(from, to, "migrated the database");
            Ok(())
        }
        Err(rusqlite_migration::Error::RusqliteError { err, .. }) if is_damage(&err) => {
            Err(Failure::Damage(err.to_string()))
        }
        Err(error) => {
            tracing::error!(from, to, %error, "a database migration failed and was rolled back");
            Err(CoreError::Migration(format!(
                "version {from} to {to}: {}",
                describe_migration_error(&error)
            ))
            .into())
        }
    }
}

/// A short reason for the UI. rusqlite_migration's own message embeds the whole migration SQL, which
/// goes to the log instead.
fn describe_migration_error(error: &rusqlite_migration::Error) -> String {
    match error {
        rusqlite_migration::Error::RusqliteError { err, .. } => err.to_string(),
        rusqlite_migration::Error::ForeignKeyCheck(violations) => match violations.first() {
            Some(first) => format!(
                "{} row(s) would break foreign keys; first: {first}",
                violations.len()
            ),
            None => "rows would break foreign keys".to_string(),
        },
        other => other.to_string(),
    }
}

/// The cheap probe of an existing file; see the module docs.
fn probe(ctx: &Ctx<'_, '_>) -> std::result::Result<Connection, Failure> {
    let conn = connect(ctx.db_path, ctx.busy_timeout)?;
    match check(&conn, ctx.supported) {
        Ok(()) => Ok(conn),
        Err(failure) => {
            // Damaged files are about to be quarantined and newer files must stay byte-identical, so
            // this connection must not checkpoint anything into the file on its way out.
            close_without_checkpoint(conn);
            Err(failure)
        }
    }
}

fn check(conn: &Connection, supported: u32) -> std::result::Result<(), Failure> {
    let found = version(conn)?;
    if found > supported {
        return Err(CoreError::SchemaTooNew { found, supported }.into());
    }
    // Preparing any statement makes SQLite parse every CREATE in the schema, and counting walks the
    // schema b-tree from page 1: work in proportion to the schema, not the data, that still catches
    // a damaged header or schema page and a malformed schema. Data pages are left to
    // `check_integrity` and to the queries that read them.
    sql(
        conn.query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
            row.get::<_, i64>(0)
        }),
    )?;
    Ok(())
}

fn finish(
    ctx: &Ctx<'_, '_>,
    opened: Opened,
    repair: Option<Recovery>,
    unclean_shutdown: bool,
) -> (Connection, StoreInfo) {
    let Opened {
        conn,
        from,
        schema_version,
        backup,
    } = opened;
    // A repair in this open matters most; a pending one from an earlier open shows once nothing
    // else has to (a migration notice only appears once, the repair keeps appearing until
    // acknowledged).
    let outcome = match repair {
        Some(repair) => repair.outcome(),
        None if from > 0 && from < ctx.supported => OpenOutcome::Migrated {
            from,
            to: schema_version,
            backup: backup.map(|path| path.display().to_string()),
        },
        None => match recovery::newest_pending(ctx.db_path, ctx.backups_dir) {
            Some(pending) => pending.outcome(),
            None if from == 0 => OpenOutcome::Created,
            None => OpenOutcome::Opened,
        },
    };
    tracing::info!(path = %ctx.db_path.display(), schema_version, ?outcome, unclean_shutdown, "opened the store");
    (
        conn,
        StoreInfo {
            path: ctx.db_path.to_path_buf(),
            schema_version,
            outcome,
            unclean_shutdown,
        },
    )
}

fn connect(db_path: &Path, busy_timeout: Duration) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(busy_timeout)?;
    Ok(conn)
}

fn close_without_checkpoint(conn: Connection) {
    if let Err(error) = conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true) {
        tracing::debug!(%error, "could not turn off checkpoint-on-close");
    }
    if let Err((_conn, error)) = conn.close() {
        tracing::warn!(%error, "closing the database connection failed");
    }
}

fn read_user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
}

fn stored_version(conn: &Connection) -> Result<u32> {
    version_from_raw(read_user_version(conn)?)
}

/// [`stored_version`] for the steps of an open.
fn version(conn: &Connection) -> std::result::Result<u32, Failure> {
    Ok(version_from_raw(sql(read_user_version(conn))?)?)
}

/// `user_version` is a signed 32-bit field; o7 only ever writes 0 or a positive migration count.
fn version_from_raw(raw: i64) -> Result<u32> {
    u32::try_from(raw).map_err(|_| {
        CoreError::Migration(format!(
            "the database reports schema version {raw}, which o7 never writes"
        ))
    })
}

fn is_non_empty_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

/// Whether the last session left the database open: closing the last connection checkpoints and
/// deletes the `-wal`, and no `-journal` outlives a transaction, so one still there means a crash, a
/// kill, or a power loss. An empty `-wal` doesn't count: a read-only connection (such as
/// [`check_integrity`] after the store closed) creates one it can't delete, and an empty WAL holds
/// nothing a crash could have cut short.
fn left_unclean(db_path: &Path) -> bool {
    let wal = fs::metadata(fsutil::with_suffix(db_path, "-wal")).is_ok_and(|meta| meta.len() > 0);
    let journal = fs::symlink_metadata(fsutil::with_suffix(db_path, "-journal")).is_ok();
    is_non_empty_file(db_path) && (wal || journal)
}

fn db_file_name(db_path: &Path) -> String {
    let name = fsutil::file_name_lossy(db_path);
    if name.is_empty() {
        DEFAULT_DB_NAME.to_string()
    } else {
        name
    }
}

/// Copies the database with the online backup API (a consistent snapshot even with a WAL) into a
/// `.partial` file, syncs it, then renames it, so an interrupted backup never looks like a complete
/// one and a power loss can't leave a complete-looking name over missing bytes.
fn backup_database(
    conn: &Connection,
    ctx: &Ctx<'_, '_>,
    from: u32,
) -> std::result::Result<PathBuf, Failure> {
    fsutil::create_private_dir_all(ctx.backups_dir)?;
    let db_name = db_file_name(ctx.db_path);
    let base = format!("{db_name}.v{from}-{}", now_millis());
    let target = fsutil::free_path(ctx.backups_dir, &base, BACKUP_SUFFIX);
    let partial = fsutil::with_suffix(&target, PARTIAL_SUFFIX);
    let backup = {
        conn.backup(rusqlite::MAIN_DB, &partial, None)
            .map_err(CoreError::from)
    };
    if let Err(error) = backup {
        remove_with_sidecars(&partial);
        let CoreError::Sqlite(source) = error else {
            return Err(error.into());
        };
        if is_damage(&source) {
            return Err(Failure::Damage(source.to_string()));
        }
        return Err(CoreError::Backup {
            path: target,
            source,
        }
        .into());
    }
    if let Err(source) = fsutil::sync_file(&partial).and_then(|()| fs::rename(&partial, &target)) {
        remove_with_sidecars(&partial);
        return Err(CoreError::Io {
            path: target,
            source,
        }
        .into());
    }
    fsutil::sync_dir(ctx.backups_dir);
    tracing::info!(backup = %target.display(), from, "backed up the database before migrating");
    prune_backups(ctx.backups_dir, &db_name, BACKUPS_TO_KEEP, &target);
    Ok(target)
}

fn remove_with_sidecars(path: &Path) {
    let _ = fs::remove_file(path);
    for suffix in SQLITE_SIDECARS {
        let _ = fs::remove_file(fsutil::with_suffix(path, suffix));
    }
}

/// Deletes the `.partial` files (and any sidecars next to them) that interrupted backups left: a
/// finished backup is renamed away from its `.partial` name, so one is never a valid backup, and
/// without this every interruption, likely for a large database since the backup runs before the
/// window appears, would leak a copy as big as the database. Only called while holding the open
/// lock exclusively, so no other o7 process is writing one.
fn remove_stale_backup_partials(dir: &Path, db_name: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let base = SQLITE_SIDECARS
            .iter()
            .find_map(|suffix| name.strip_suffix(suffix))
            .unwrap_or(&name);
        let is_partial_backup = base
            .strip_suffix(PARTIAL_SUFFIX)
            .is_some_and(|backup| parse_backup_name(backup, db_name).is_some());
        if !is_partial_backup {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => {
                tracing::info!(file = %entry.path().display(), "removed what an interrupted backup left")
            }
            Err(error) => {
                tracing::warn!(file = %entry.path().display(), %error, "could not remove what an interrupted backup left")
            }
        }
    }
}

/// Sort key of a backup file name; newer backups compare greater.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BackupKey {
    millis: i64,
    /// Breaks millisecond ties: within one run, versions only go up.
    from: u32,
    /// The `.<n>` collision counter from `fsutil::free_path`.
    counter: u32,
}

/// Parses `<db_name>.v<from>-<millis>[.<n>].bak`. Anything else, including files a user put in the
/// folder, yields `None` and is never deleted by pruning.
fn parse_backup_name(name: &str, db_name: &str) -> Option<BackupKey> {
    let rest = name
        .strip_prefix(db_name)?
        .strip_prefix(".v")?
        .strip_suffix(BACKUP_SUFFIX)?;
    let (from, rest) = rest.split_once('-')?;
    let (millis, counter) = match rest.split_once('.') {
        Some((millis, counter)) => (millis, counter.parse().ok()?),
        None => (rest, 0),
    };
    Some(BackupKey {
        millis: millis.parse().ok()?,
        from: from.parse().ok()?,
        counter,
    })
}

/// Keeps `keep` backups of `db_name`: `protect` (the one just made) and the newest others, deleting
/// the rest (and any sidecars left by opening them). The new backup is protected by path, not by its
/// timestamp: after the clock was set back, older backups can carry later timestamps, and sorting
/// would make the new one look oldest and delete it, leaving a migration without its backup.
/// Failures are logged: a leftover backup costs disk space, not correctness.
fn prune_backups(dir: &Path, db_name: &str, keep: usize, protect: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(dir = %dir.display(), %error, "could not list backups to prune");
            return;
        }
    };
    let mut others: Vec<(BackupKey, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path() != protect)
        .filter_map(|entry| {
            let key = parse_backup_name(entry.file_name().to_str()?, db_name)?;
            Some((key, entry.path()))
        })
        .collect();
    others.sort_by_key(|(key, _)| std::cmp::Reverse(*key));
    for (_, path) in others.into_iter().skip(keep.saturating_sub(1)) {
        match fs::remove_file(&path) {
            Ok(()) => {
                for suffix in SQLITE_SIDECARS {
                    let _ = fs::remove_file(fsutil::with_suffix(&path, suffix));
                }
            }
            Err(error) => {
                tracing::warn!(backup = %path.display(), %error, "could not delete an old backup")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashSet};

    const TABLES: [&str; 23] = [
        "settings",
        "projects",
        "project_folders",
        "folder_locations",
        "chats",
        "agent_sessions",
        "turns",
        "events",
        "attachments",
        "agent_cache",
        "queued_prompts",
        "worktree_setup",
        "worktree_setup_attempts",
        "sync_config_device",
        "sync_blobs",
        "sync_blob_refs",
        "steering_inputs",
        "sync_projection",
        "sync_turns",
        "sync_turn_parts",
        "sync_turn_files",
        "sync_turn_fragments",
        "local_imports",
    ];

    /// The current schema version as SQLite reports it.
    const CURRENT: i64 = SCHEMA_VERSION as i64;

    /// The version a test's extra migration takes the database to.
    const NEXT: u32 = SCHEMA_VERSION + 1;

    struct Paths {
        _dir: tempfile::TempDir,
        db: PathBuf,
        backups: PathBuf,
    }

    fn paths() -> Paths {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("profile").join("aura.db");
        let backups = dir.path().join("profile").join("backups");
        Paths {
            _dir: dir,
            db,
            backups,
        }
    }

    fn open(p: &Paths) -> (Store, StoreInfo) {
        Store::open_at(&p.db, &p.backups).expect("open store")
    }

    fn user_version(path: &Path) -> i64 {
        let conn = Connection::open(path).expect("raw open");
        read_user_version(&conn).expect("user_version")
    }

    fn names(conn: &Connection, sql: &str) -> BTreeSet<String> {
        let mut stmt = conn.prepare(sql).expect("prepare");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query");
        rows.map(|row| row.expect("row")).collect()
    }

    fn files_in(dir: &Path) -> BTreeSet<String> {
        match fs::read_dir(dir) {
            Ok(entries) => entries
                .map(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect(),
            Err(_) => BTreeSet::new(),
        }
    }

    fn insert_project(conn: &Connection, id: &str, name: &str) {
        conn.execute(
            "INSERT INTO projects (id, name, created_at, updated_at, device_id) VALUES (?1, ?2, 1, 1, 'dev')",
            [id, name],
        )
        .expect("insert project");
    }

    fn insert_chat(conn: &Connection, id: &str, project_id: &str) {
        conn.execute(
            "INSERT INTO chats (id, project_id, created_at, updated_at, device_id) VALUES (?1, ?2, 1, 1, 'dev')",
            [id, project_id],
        )
        .expect("insert chat");
    }

    fn insert_events(conn: &Connection, chat_id: &str, count: i64) {
        let tx = conn.unchecked_transaction().expect("tx");
        for seq in 0..count {
            tx.execute(
                "INSERT INTO events (id, chat_id, seq, kind, payload, created_at, device_id) \
                 VALUES (?1, ?2, ?3, 'message_delta', ?4, 1, 'd')",
                rusqlite::params![
                    format!("{chat_id}-e{seq:05}"),
                    chat_id,
                    seq,
                    format!("{{\"text\": \"padding padding {seq}\"}}")
                ],
            )
            .expect("event");
        }
        tx.commit().expect("commit");
    }

    fn project_names(conn: &Connection) -> Vec<String> {
        names(conn, "SELECT name FROM projects ORDER BY name")
            .into_iter()
            .collect()
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count")
    }

    /// Pseudo-random bytes without a rand dependency (xorshift).
    fn noise(len: usize) -> Vec<u8> {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect()
    }

    /// Overwrites the start of page `page` (1-based) so SQLite can't parse it.
    fn smash(path: &Path, page: i64) {
        let page_size: i64 = Connection::open(path)
            .expect("raw")
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .expect("page_size");
        let mut bytes = fs::read(path).expect("read");
        let start = usize::try_from((page - 1) * page_size).expect("offset");
        bytes[start..start + 16].fill(0xA5);
        fs::write(path, bytes).expect("write");
    }

    /// A page in the middle of the `events` table (in key order), from `dbstat`.
    fn middle_events_leaf(path: &Path) -> i64 {
        let conn = Connection::open(path).expect("raw");
        let leaves: Vec<i64> = conn
            .prepare(
                "SELECT pageno FROM dbstat WHERE name = 'events' AND pagetype = 'leaf' ORDER BY path",
            )
            .expect("dbstat")
            .query_map([], |row| row.get(0))
            .expect("pages")
            .map(|row| row.expect("page"))
            .collect();
        assert!(leaves.len() > 10, "{leaves:?}");
        leaves[leaves.len() / 2]
    }

    /// A v1 database whose `junk` table's pages were leaked: harmless, but `quick_check` reports
    /// "never used" pages, and before salvaging existed that quarantined everything.
    fn with_leaked_pages(p: &Paths) {
        {
            let (store, _) = open(p);
            insert_project(store.conn(), "p1", "Alpha");
        }
        let conn = Connection::open(&p.db).expect("raw");
        conn.execute_batch(
            "CREATE TABLE junk (x); WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 300) \
             INSERT INTO junk SELECT printf('%0200d', i) FROM n; \
             PRAGMA writable_schema = ON; DELETE FROM sqlite_schema WHERE name = 'junk'; PRAGMA writable_schema = OFF;",
        )
        .expect("leak the pages of a dropped table");
    }

    /// The real migrations followed by `extra`, as tests define later schema versions.
    fn with_current<'a>(extra: &[M<'a>]) -> Vec<M<'a>> {
        let mut all: Vec<M<'a>> = migrations().to_vec();
        all.extend_from_slice(extra);
        all
    }

    fn quarantined_path(outcome: &OpenOutcome) -> PathBuf {
        match outcome {
            OpenOutcome::RecoveredFromCorruption { quarantined, .. } => PathBuf::from(quarantined),
            other => panic!("expected RecoveredFromCorruption, got {other:?}"),
        }
    }

    #[test]
    fn migration_list_matches_schema_version() {
        assert_eq!(migrations().len(), SCHEMA_VERSION as usize);
        Migrations::from_slice(migrations())
            .validate()
            .expect("migrations apply to an empty database");
    }

    #[test]
    fn a_fresh_database_is_created_with_the_full_schema() {
        let p = paths();
        let (store, info) = open(&p);
        assert_eq!(info.outcome, OpenOutcome::Created);
        assert_eq!(info.schema_version, SCHEMA_VERSION);
        assert_eq!(info.path, p.db);
        assert!(!info.unclean_shutdown);
        assert_eq!(store.path(), p.db);
        assert_eq!(store.backups_dir(), p.backups);
        assert_eq!(store.schema_version().expect("version"), SCHEMA_VERSION);

        let conn = store.conn();
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal_mode");
        assert_eq!(journal, "wal");
        let foreign_keys: i64 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("foreign_keys");
        assert_eq!(foreign_keys, 1);
        let synchronous: i64 = conn
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .expect("synchronous");
        assert_eq!(synchronous, 1, "NORMAL");
        // On macOS a checkpoint must end with F_FULLFSYNC, or a power loss can keep the WAL reset
        // and lose the checkpoint's writes. The bundled SQLite defaults to off.
        let ckpt_fullfsync: i64 = conn
            .pragma_query_value(None, "checkpoint_fullfsync", |row| row.get(0))
            .expect("checkpoint_fullfsync");
        assert_eq!(ckpt_fullfsync, 1);

        let tables = names(conn, "SELECT name FROM sqlite_schema WHERE type = 'table'");
        for table in TABLES.iter().chain(["message_fts"].iter()) {
            assert!(tables.contains(*table), "missing table {table}: {tables:?}");
        }
        let fts_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name = 'message_fts'",
                [],
                |row| row.get(0),
            )
            .expect("fts");
        assert!(
            fts_sql.starts_with("CREATE VIRTUAL TABLE message_fts USING fts5"),
            "{fts_sql}"
        );
        // No backups for a brand-new database.
        assert!(files_in(&p.backups).is_empty());
    }

    #[test]
    fn full_text_search_folds_case_and_diacritics() {
        let p = paths();
        let (store, _) = open(&p);
        let conn = store.conn();
        conn.execute(
            "INSERT INTO message_fts (chat_id, message_id, text) VALUES ('c1', 'm1', 'Café crème brûlée in São Paulo')",
            [],
        )
        .expect("insert");
        conn.execute("INSERT INTO message_fts (chat_id, message_id, text) VALUES ('c1', 'm2', 'nothing relevant')", [])
            .expect("insert");
        for query in ["cafe", "creme", "brulee", "sao paulo", "CAFÉ", "Crème"] {
            assert_eq!(
                fts_hits(conn, query),
                BTreeSet::from(["m1".to_string()]),
                "query {query:?}"
            );
        }
        assert!(fts_hits(conn, "tokyo").is_empty());
    }

    fn fts_hits(conn: &Connection, query: &str) -> BTreeSet<String> {
        let mut stmt = conn
            .prepare("SELECT message_id FROM message_fts WHERE message_fts MATCH ?1")
            .expect("prepare");
        let rows = stmt
            .query_map([query], |row| row.get::<_, String>(0))
            .expect("match");
        rows.map(|row| row.expect("row")).collect()
    }

    #[test]
    fn synced_tables_carry_device_id_and_tombstones() {
        let p = paths();
        let (store, _) = open(&p);
        let conn = store.conn();
        let columns = |table: &str| {
            names(
                conn,
                &format!("SELECT name FROM pragma_table_info('{table}')"),
            )
        };
        for table in ["projects", "project_folders", "chats"] {
            let cols = columns(table);
            assert!(
                cols.contains("device_id") && cols.contains("deleted"),
                "{table}: {cols:?}"
            );
        }
        for table in ["agent_sessions", "turns", "events", "folder_locations"] {
            assert!(columns(table).contains("device_id"), "{table}");
        }
        insert_project(conn, "p1", "Alpha");
        let (deleted, sort_key): (i64, String) = conn
            .query_row(
                "SELECT deleted, sort_key FROM projects WHERE id = 'p1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("defaults");
        assert_eq!((deleted, sort_key.as_str()), (0, ""));
        insert_chat(conn, "c1", "p1");
        let (isolation, pinned): (String, i64) = conn
            .query_row(
                "SELECT isolation, pinned FROM chats WHERE id = 'c1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("defaults");
        assert_eq!((isolation.as_str(), pinned), ("direct", 0));
    }

    #[test]
    fn constraints_are_enforced() {
        let p = paths();
        let (store, _) = open(&p);
        let conn = store.conn();
        let is_constraint = |result: rusqlite::Result<usize>| {
            matches!(
                result.map_err(|e| e.sqlite_error_code()),
                Err(Some(ErrorCode::ConstraintViolation))
            )
        };
        // Foreign keys are on: a chat can't point at a missing project.
        assert!(is_constraint(conn.execute(
            "INSERT INTO chats (id, project_id, created_at, updated_at, device_id) VALUES ('c', 'missing', 1, 1, 'd')",
            []
        )));
        insert_project(conn, "p1", "Alpha");
        assert!(is_constraint(conn.execute(
            "INSERT INTO project_folders (id, project_id, display_name, kind, created_at, updated_at, device_id) \
             VALUES ('f', 'p1', 'x', 'svn', 1, 1, 'd')",
            []
        )));
        assert!(is_constraint(conn.execute(
            "INSERT INTO chats (id, project_id, isolation, created_at, updated_at, device_id) VALUES ('c', 'p1', 'sandbox', 1, 1, 'd')",
            []
        )));
        insert_chat(conn, "c1", "p1");
        assert!(is_constraint(conn.execute(
            "INSERT INTO turns (id, chat_id, seq, status, started_at, device_id) VALUES ('t', 'c1', 1, 'paused', 1, 'd')",
            []
        )));
        conn.execute(
            "INSERT INTO events (id, chat_id, seq, kind, payload, created_at, device_id) VALUES ('e1', 'c1', 1, 'k', '{}', 1, 'd')",
            [],
        )
        .expect("first event");
        assert!(is_constraint(conn.execute(
            "INSERT INTO events (id, chat_id, seq, kind, payload, created_at, device_id) VALUES ('e2', 'c1', 1, 'k', '{}', 1, 'd')",
            []
        )));
    }

    #[test]
    fn every_foreign_key_leads_an_index() {
        let p = paths();
        let (store, _) = open(&p);
        let conn = store.conn();
        let mut checked = 0;
        for table in TABLES {
            let foreign_keys = names(
                conn,
                &format!("SELECT \"from\" FROM pragma_foreign_key_list('{table}')"),
            );
            let mut leading = HashSet::new();
            for index in names(
                conn,
                &format!("SELECT name FROM pragma_index_list('{table}')"),
            ) {
                leading.extend(names(
                    conn,
                    &format!("SELECT name FROM pragma_index_info('{index}') WHERE seqno = 0"),
                ));
            }
            for column in foreign_keys {
                assert!(leading.contains(&column), "{table}.{column} has no index");
                checked += 1;
            }
        }
        assert_eq!(
            checked, 20,
            "every foreign key in the listed tables was checked"
        );
    }

    #[test]
    fn reopening_is_opened() {
        let p = paths();
        {
            let (store, _) = open(&p);
            insert_project(store.conn(), "p1", "Alpha");
        }
        let (store, info) = open(&p);
        assert_eq!(info.outcome, OpenOutcome::Opened);
        assert_eq!(info.schema_version, SCHEMA_VERSION);
        assert!(!info.unclean_shutdown, "the last store closed cleanly");
        assert_eq!(project_names(store.conn()), vec!["Alpha"]);
        assert!(
            files_in(&p.backups).is_empty(),
            "no backup when nothing migrates"
        );
    }

    #[test]
    fn a_session_that_never_closed_is_reported_as_an_unclean_shutdown() {
        let p = paths();
        {
            let (store, _) = open(&p);
            insert_project(store.conn(), "p1", "Alpha");
            // As after a crash: the connection goes away without checkpointing, leaving its -wal.
            store
                .conn()
                .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
                .expect("keep the wal");
        }
        assert!(fsutil::with_suffix(&p.db, "-wal").exists());
        let (store, info) = open(&p);
        assert!(info.unclean_shutdown);
        assert_eq!(info.outcome, OpenOutcome::Opened);
        assert_eq!(project_names(store.conn()), vec!["Alpha"]);
        drop(store);
        assert!(!open(&p).1.unclean_shutdown);
    }

    #[test]
    fn random_bytes_are_quarantined_and_replaced() {
        let p = paths();
        fs::create_dir_all(p.db.parent().expect("parent")).expect("mkdir");
        let garbage = noise(64 * 1024);
        fs::write(&p.db, &garbage).expect("write garbage");
        fs::write(fsutil::with_suffix(&p.db, "-wal"), b"stale wal").expect("wal");
        fs::write(fsutil::with_suffix(&p.db, "-shm"), b"stale shm").expect("shm");

        let (store, info) = open(&p);
        let OpenOutcome::RecoveredFromCorruption {
            quarantined,
            salvage,
        } = &info.outcome
        else {
            panic!("expected RecoveredFromCorruption, got {:?}", info.outcome);
        };
        assert_eq!(
            salvage,
            &Salvage::Nothing,
            "nothing in random bytes is readable"
        );
        let quarantined = PathBuf::from(quarantined);
        assert_eq!(quarantined.parent(), Some(p.backups.as_path()));
        assert!(fsutil::file_name_lossy(&quarantined).starts_with("aura.db.corrupt-"));
        assert_eq!(fs::read(&quarantined).expect("quarantined bytes"), garbage);
        assert_eq!(
            fs::read(fsutil::with_suffix(&quarantined, "-wal")).expect("wal moved"),
            b"stale wal"
        );
        // The -shm moves too. SQLite reinitialises it while probing (it is a derived index, not data),
        // so only its presence is checked.
        assert!(fsutil::with_suffix(&quarantined, "-shm").is_file());

        // The replacement works.
        assert_eq!(info.schema_version, SCHEMA_VERSION);
        insert_project(store.conn(), "p1", "Fresh");
        assert_eq!(project_names(store.conn()), vec!["Fresh"]);
        drop(store);
        // Still reported until the UI acknowledges it.
        assert_eq!(open(&p).1.outcome, info.outcome);
    }

    /// Finding: a startup `quick_check` read the whole file (seconds for a large history). The open
    /// now only probes the header and schema, so harmless leaked pages don't even slow it down;
    /// `check_integrity`, run in the background, is what finds them.
    #[test]
    fn opening_skips_the_full_check_and_check_integrity_finds_what_it_skipped() {
        let p = paths();
        with_leaked_pages(&p);
        let (store, info) = open(&p);
        assert_eq!(
            info.outcome,
            OpenOutcome::Opened,
            "the probe doesn't read data pages"
        );
        assert_eq!(project_names(store.conn()), vec!["Alpha"]);

        let Integrity::Damaged { problem } = check_integrity(store.path()).expect("check") else {
            panic!("the leaked pages should be found");
        };
        assert!(problem.contains("never used"), "{problem}");
        assert!(
            recovery::damage_marker(&p.db).is_file(),
            "recorded for the next open"
        );
        // The store keeps working meanwhile.
        insert_project(store.conn(), "p2", "Beta");
        drop(store);

        let before = fs::read(&p.db).expect("read");
        let (store, info) = open(&p);
        let OpenOutcome::RecoveredFromCorruption { salvage, .. } = &info.outcome else {
            panic!("expected a repair, got {:?}", info.outcome);
        };
        assert_eq!(salvage, &Salvage::Complete, "leaked pages cost no rows");
        assert_eq!(project_names(store.conn()), vec!["Alpha", "Beta"]);
        assert_eq!(
            fs::read(quarantined_path(&info.outcome)).expect("quarantined"),
            before,
            "the damaged file is kept byte for byte"
        );
        assert!(!recovery::damage_marker(&p.db).exists());
        assert_eq!(
            check_integrity(store.path()).expect("check"),
            Integrity::Ok,
            "the copy is clean"
        );
    }

    #[test]
    fn check_integrity_of_a_healthy_database_records_nothing() {
        let p = paths();
        let (store, _) = open(&p);
        insert_project(store.conn(), "p1", "Alpha");
        assert_eq!(check_integrity(store.path()).expect("check"), Integrity::Ok);
        assert!(!recovery::damage_marker(&p.db).exists());
        drop(store);
        // Also with no store open, as a CLI check would run it.
        assert_eq!(check_integrity(&p.db).expect("check"), Integrity::Ok);
        let (_, info) = open(&p);
        assert_eq!(info.outcome, OpenOutcome::Opened);
        assert!(
            !info.unclean_shutdown,
            "the read-only check doesn't leave anything that looks like a crash"
        );
    }

    #[test]
    fn check_integrity_leaves_newer_data_alone() {
        let p = paths();
        let future_version = SCHEMA_VERSION + 1;
        {
            let (store, _) = open(&p);
            store
                .conn()
                .pragma_update(None, "user_version", future_version)
                .expect("bump");
        }
        assert!(matches!(
            check_integrity(&p.db),
            Err(CoreError::SchemaTooNew { found, .. }) if found == future_version
        ));
        assert!(!recovery::damage_marker(&p.db).exists());
    }

    #[test]
    fn check_integrity_of_random_bytes_records_damage() {
        let p = paths();
        fs::create_dir_all(p.db.parent().expect("parent")).expect("mkdir");
        fs::write(&p.db, noise(8192)).expect("garbage");
        assert!(matches!(
            check_integrity(&p.db).expect("check"),
            Integrity::Damaged { .. }
        ));
        assert!(recovery::damage_marker(&p.db).is_file());
    }

    /// Finding: one bad page in one table used to cost the whole history. Now the open doesn't
    /// notice it, the query that reads the page reports it, and the next open copies out
    /// everything else.
    #[test]
    fn a_damaged_table_page_found_by_a_query_is_repaired_with_everything_readable() {
        let p = paths();
        {
            let (store, _) = open(&p);
            let conn = store.conn();
            insert_project(conn, "p1", "Alpha");
            insert_chat(conn, "c1", "p1");
            insert_events(conn, "c1", 3000);
        }
        // Closing the last connection checkpointed the WAL, so every page is in the main file.
        assert!(!fsutil::with_suffix(&p.db, "-wal").exists());
        smash(&p.db, middle_events_leaf(&p.db));
        let damaged = fs::read(&p.db).expect("read");

        let (store, info) = open(&p);
        assert_eq!(info.outcome, OpenOutcome::Opened);
        let error = store
            .conn()
            .query_row("SELECT sum(length(payload)) FROM events", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect_err("the bad page is unreadable");
        assert!(is_damage(&error), "{error}");
        assert!(store.note_query_error(&error).expect("record"));
        drop(store);

        let (store, info) = open(&p);
        let OpenOutcome::RecoveredFromCorruption { salvage, .. } = &info.outcome else {
            panic!("expected a repair, got {:?}", info.outcome);
        };
        assert_eq!(
            salvage,
            &Salvage::Partial {
                tables: vec!["events".into()]
            }
        );
        assert_eq!(
            fs::read(quarantined_path(&info.outcome)).expect("quarantined"),
            damaged
        );
        let conn = store.conn();
        assert_eq!(project_names(conn), vec!["Alpha"]);
        assert_eq!(count(conn, "chats"), 1);
        let events = count(conn, "events");
        assert!(
            (2800..3000).contains(&events),
            "{events} of 3000 events kept"
        );
        assert_eq!(info.schema_version, SCHEMA_VERSION);
        drop(store);
        assert_eq!(check_integrity(&p.db).expect("check"), Integrity::Ok);
    }

    /// Finding: losing the one page of `project_folders` cost the chats in that folder and, through
    /// them, their events (1 of 4 chats and 100 of 400 events were left), although the project and
    /// every chat and event page were intact.
    #[test]
    fn a_lost_folder_page_keeps_every_chat_and_event() {
        let p = paths();
        {
            let (store, _) = open(&p);
            let conn = store.conn();
            insert_project(conn, "p1", "Alpha");
            conn.execute(
                "INSERT INTO project_folders (id, project_id, display_name, kind, created_at, updated_at, device_id) \
                 VALUES ('f1', 'p1', 'repo', 'git', 1, 1, 'dev')",
                [],
            )
            .expect("folder");
            for chat in ["c1", "c2", "c3", "c4"] {
                insert_chat(conn, chat, "p1");
                insert_events(conn, chat, 100);
            }
            conn.execute(
                "UPDATE chats SET folder_id = 'f1' WHERE id IN ('c1', 'c2', 'c3')",
                [],
            )
            .expect("three chats into the folder");
        }
        let folders: i64 = Connection::open(&p.db)
            .expect("raw")
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE name = 'project_folders'",
                [],
                |row| row.get(0),
            )
            .expect("rootpage");
        smash(&p.db, folders);
        // Found by the background check, which records it for the next open.
        assert!(matches!(
            check_integrity(&p.db).expect("check"),
            Integrity::Damaged { .. }
        ));

        let (store, info) = open(&p);
        let OpenOutcome::RecoveredFromCorruption {
            salvage: Salvage::Partial { tables },
            ..
        } = &info.outcome
        else {
            panic!("expected a partial repair, got {:?}", info.outcome);
        };
        assert_eq!(tables.as_slice(), ["project_folders", "chats"]);
        let conn = store.conn();
        assert_eq!(project_names(conn), vec!["Alpha"]);
        assert_eq!(count(conn, "chats"), 4);
        assert_eq!(count(conn, "events"), 400);
    }

    /// Page numbers of `sqlite_schema`'s leaves, in key order.
    fn schema_leaves(path: &Path) -> Vec<i64> {
        let conn = Connection::open(path).expect("raw");
        let mut stmt = conn
            .prepare(
                "SELECT pageno FROM dbstat WHERE name = 'sqlite_schema' AND pagetype = 'leaf' ORDER BY path",
            )
            .expect("dbstat");
        stmt.query_map([], |row| row.get(0))
            .expect("pages")
            .map(|row| row.expect("page"))
            .collect()
    }

    /// The names in `sqlite_schema` a lookup by rowid can still read, after the damage.
    fn readable_schema_names(path: &Path) -> BTreeSet<String> {
        let conn =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("open");
        conn.pragma_update(None, "writable_schema", true)
            .expect("writable_schema");
        // Loads what it can of the schema; the next statements run against that.
        let _ = conn.prepare("SELECT 1 FROM sqlite_schema LIMIT 0");
        (1..=200)
            .filter_map(|rowid| {
                conn.query_row(
                    "SELECT name FROM sqlite_schema WHERE rowid = ?1",
                    [rowid],
                    |row| row.get::<_, String>(0),
                )
                .ok()
            })
            .collect()
    }

    /// Rows in projects, chats, events, message_fts, and agent_cache.
    fn seeded(p: &Paths) {
        let (store, _) = open(p);
        let conn = store.conn();
        insert_project(conn, "p1", "Alpha");
        insert_chat(conn, "c1", "p1");
        insert_events(conn, "c1", 200);
        conn.execute_batch(
            "INSERT INTO message_fts (chat_id, message_id, text) VALUES ('c1', 'm1', 'crème brûlée'); \
             INSERT INTO agent_cache (agent_id, detection, checked_at) VALUES ('claude', '{}', 1);",
        )
        .expect("search entry and cached agent");
    }

    /// Finding: with one leaf of `sqlite_schema` damaged, the repair reported "Nothing was lost"
    /// while the new database had no table for any entry on that leaf ("no such table: events" on
    /// every later query, and user_version 1, so no migration would ever create them); with the
    /// other leaf damaged it started over empty although every data page was intact.
    #[test]
    fn a_damaged_schema_leaf_is_repaired_with_every_table_in_place() {
        let leaves = {
            let p = paths();
            seeded(&p);
            schema_leaves(&p.db)
        };
        assert!(leaves.len() >= 2, "{leaves:?}");
        for (index, leaf) in leaves.into_iter().enumerate() {
            let p = paths();
            seeded(&p);
            smash(&p.db, leaf);
            let readable = readable_schema_names(&p.db);

            let (store, info) = open(&p);
            let OpenOutcome::RecoveredFromCorruption {
                salvage: Salvage::Partial { tables },
                ..
            } = &info.outcome
            else {
                panic!(
                    "leaf {index}: expected a partial repair, got {:?}",
                    info.outcome
                );
            };
            assert_eq!(info.schema_version, SCHEMA_VERSION);
            let conn = store.conn();
            for table in TABLES.iter().chain(["message_fts"].iter()) {
                // Fails with "no such table" if the repaired database lacks it.
                let rows = count(conn, table);
                if !readable.contains(*table) {
                    assert!(
                        tables.iter().any(|lost| lost == table),
                        "leaf {index}: {table} was unreadable but isn't reported: {tables:?}"
                    );
                    assert_eq!(rows, 0, "leaf {index}: {table}");
                }
            }
            if readable.contains("projects") {
                assert_eq!(project_names(conn), vec!["Alpha"], "leaf {index}");
            }
            if readable.contains("agent_cache") {
                assert_eq!(count(conn, "agent_cache"), 1, "leaf {index}");
            }
            // Every table takes new rows.
            insert_project(conn, "p2", "Beta");
            insert_chat(conn, "c2", "p2");
            insert_events(conn, "c2", 3);
            conn.execute_batch(
                "INSERT INTO agent_cache (agent_id) VALUES ('codex'); \
                 INSERT INTO message_fts (chat_id, message_id, text) VALUES ('c2', 'm2', 'fresh');",
            )
            .expect("new rows");
            drop(store);
            assert_eq!(check_integrity(&p.db).expect("check"), Integrity::Ok);
            let leftovers: Vec<String> = files_in(p.db.parent().expect("profile"))
                .into_iter()
                .filter(|name| name.contains("salvage"))
                .collect();
            assert!(leftovers.is_empty(), "leaf {index}: {leftovers:?}");
        }
    }

    /// A WAL database whose last row is only in its `-wal`, as after a crash.
    fn with_a_row_only_in_the_wal(path: &Path) {
        fs::create_dir_all(fsutil::parent_dir(path)).expect("mkdir");
        let conn = Connection::open(path).expect("open");
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; CREATE TABLE t (x); INSERT INTO t VALUES ('checkpointed'); \
             PRAGMA wal_checkpoint(TRUNCATE); INSERT INTO t VALUES ('only in the wal');",
        )
        .expect("rows");
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .expect("keep the wal");
    }

    fn rows_of_t(path: &Path) -> Vec<String> {
        let conn =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("open");
        names(&conn, "SELECT x FROM t").into_iter().collect()
    }

    fn write_record(quarantined: &Path, salvage: &Salvage) {
        let record = serde_json::json!({"problem": "test", "salvage": salvage, "recovered_at": 1});
        fs::write(
            fsutil::with_suffix(quarantined, ".json"),
            record.to_string(),
        )
        .expect("record");
    }

    /// Finding: a repair that stopped right after moving the damaged file left its `-wal` at the
    /// database path, and the next open deleted it (finishing the repair, or SQLite creating the
    /// new file when nothing was salvaged), with every transaction not yet in the main file.
    #[test]
    fn a_repair_stopped_right_after_the_move_keeps_the_wal_with_the_moved_file() {
        for salvaged in [true, false] {
            let p = paths();
            with_a_row_only_in_the_wal(&p.db);
            fs::create_dir_all(&p.backups).expect("mkdir");
            let target = p.backups.join("aura.db.corrupt-1000");
            // What a repair has done by the time it moves the file: its copy (when it salvaged
            // anything) and its record.
            if salvaged {
                let mut copy =
                    Connection::open(recovery::salvage_path(&p.db, &target)).expect("copy");
                Migrations::from_slice(migrations())
                    .to_latest(&mut copy)
                    .expect("schema");
                insert_project(&copy, "p1", "Salvaged");
                write_record(&target, &Salvage::Complete);
            } else {
                write_record(&target, &Salvage::Nothing);
            }
            recovery::quarantine(&p.db, &target).expect("move");
            // ...and stops there.

            let (store, info) = open(&p);
            assert_eq!(quarantined_path(&info.outcome), target);
            assert_eq!(
                rows_of_t(&target),
                ["checkpointed", "only in the wal"],
                "salvaged {salvaged}: the moved file kept the row that was only in its -wal"
            );
            let tables = names(
                store.conn(),
                "SELECT name FROM sqlite_schema WHERE type = 'table'",
            );
            assert!(
                !tables.contains("t"),
                "the old -wal was not replayed into the new database"
            );
            let expected: Vec<&str> = if salvaged { vec!["Salvaged"] } else { vec![] };
            assert_eq!(project_names(store.conn()), expected);
        }
    }

    /// Finding: an interrupted repair whose record this build couldn't read (torn, or written by a
    /// newer build) lost its salvaged copy, and the open reported a plain Created over an empty
    /// database, with no repair notice.
    #[test]
    fn an_interrupted_repair_with_an_unreadable_record_installs_its_copy_and_is_reported() {
        let p = paths();
        {
            let (store, _) = open(&p);
            insert_project(store.conn(), "p1", "Alpha");
        }
        fs::create_dir_all(&p.backups).expect("mkdir");
        let target = p.backups.join("aura.db.corrupt-1000");
        // The repair moved the damaged file and stopped with its copy waiting.
        fs::rename(&p.db, recovery::salvage_path(&p.db, &target)).expect("the copy");
        fs::write(&target, noise(8192)).expect("the moved file");
        fs::write(
            fsutil::with_suffix(&target, ".json"),
            br#"{"problem": "x", "salvage": {"type": "from_a_newer_build"}, "recovered_at": 1}"#,
        )
        .expect("record");

        let (store, info) = open(&p);
        assert_eq!(
            info.outcome,
            OpenOutcome::RecoveredFromCorruption {
                quarantined: target.display().to_string(),
                salvage: Salvage::Partial { tables: Vec::new() },
            }
        );
        assert_eq!(project_names(store.conn()), vec!["Alpha"]);
        drop(store);
        assert_eq!(
            open(&p).1.outcome,
            info.outcome,
            "reported until acknowledged"
        );
    }

    /// Finding: the damage marker went only after the copy was installed, so a repair that stopped
    /// in between left it next to the healthy copy, and the next open repaired that copy again
    /// (another quarantined copy, and a "Nothing was lost" notice in place of the first repair's).
    /// The marker now goes as soon as the file it describes has moved (`recovery`'s tests check
    /// that order), so stopping after the install leaves the copy and the record: the next open
    /// reports the first repair and repairs nothing.
    #[test]
    fn a_repair_stopped_after_installing_its_copy_is_not_repeated() {
        let p = paths();
        {
            let (store, _) = open(&p);
            insert_project(store.conn(), "p1", "Alpha");
        }
        fs::create_dir_all(&p.backups).expect("mkdir");
        let target = p.backups.join("aura.db.corrupt-1000");
        fs::write(&target, noise(8192)).expect("the moved file");
        let events_lost = Salvage::Partial {
            tables: vec!["events".into()],
        };
        write_record(&target, &events_lost);

        let (store, info) = open(&p);
        assert_eq!(
            info.outcome,
            OpenOutcome::RecoveredFromCorruption {
                quarantined: target.display().to_string(),
                salvage: events_lost,
            }
        );
        assert_eq!(project_names(store.conn()), vec!["Alpha"]);
        let quarantined: Vec<String> = files_in(&p.backups)
            .into_iter()
            .filter(|name| name.starts_with("aura.db.corrupt-") && !name.ends_with(".json"))
            .collect();
        assert_eq!(quarantined, ["aura.db.corrupt-1000"], "nothing else moved");
    }

    #[test]
    fn note_query_error_ignores_errors_that_are_not_damage() {
        let p = paths();
        let (store, _) = open(&p);
        let error = store
            .conn()
            .execute(
                "INSERT INTO chats (id, project_id, created_at, updated_at, device_id) VALUES ('c', 'missing', 1, 1, 'd')",
                [],
            )
            .expect_err("foreign key");
        assert!(!store.note_query_error(&error).expect("note"));
        assert!(
            !store
                .note_query_error(&rusqlite::Error::QueryReturnedNoRows)
                .expect("note")
        );
        assert!(!recovery::damage_marker(&p.db).exists());
    }

    #[test]
    fn a_damaged_schema_page_is_caught_by_the_probe_and_repaired() {
        let p = paths();
        {
            let (store, _) = open(&p);
            insert_project(store.conn(), "p1", "Alpha");
        }
        {
            let conn = Connection::open(&p.db).expect("raw");
            conn.execute_batch(
                "PRAGMA writable_schema = ON; \
                 INSERT INTO sqlite_schema (type, name, tbl_name, rootpage, sql) \
                 VALUES ('table', 'broken', 'broken', 0, 'CREATE TABLE broken (x MALFORMED ((');",
            )
            .expect("malformed entry");
        }
        let (store, info) = open(&p);
        let OpenOutcome::RecoveredFromCorruption { salvage, .. } = &info.outcome else {
            panic!("expected a repair, got {:?}", info.outcome);
        };
        assert_eq!(
            salvage,
            &Salvage::Partial {
                tables: vec!["broken".into()]
            }
        );
        assert_eq!(project_names(store.conn()), vec!["Alpha"]);
    }

    #[test]
    fn a_truncated_database_keeps_what_is_left() {
        let p = paths();
        {
            let (store, _) = open(&p);
            let conn = store.conn();
            for n in 0..500 {
                insert_project(
                    conn,
                    &format!("p{n}"),
                    &format!("project number {n} with a longer name"),
                );
            }
        }
        let bytes = fs::read(&p.db).expect("read");
        fs::write(&p.db, &bytes[..bytes.len() / 2]).expect("truncate");
        let (store, info) = open(&p);
        let OpenOutcome::RecoveredFromCorruption { salvage, .. } = &info.outcome else {
            panic!("expected a repair, got {:?}", info.outcome);
        };
        assert!(
            matches!(salvage, Salvage::Partial { .. } | Salvage::Nothing),
            "{salvage:?}"
        );
        let kept = count(store.conn(), "projects");
        assert!(kept < 500, "{kept}");
    }

    #[test]
    fn a_newer_schema_is_refused_without_writing() {
        let p = paths();
        {
            let (store, _) = open(&p);
            insert_project(store.conn(), "p1", "Alpha");
            store
                .conn()
                .pragma_update(None, "user_version", 99)
                .expect("bump version");
        }
        let before = fs::read(&p.db).expect("read");
        let result = Store::open_at(&p.db, &p.backups);
        assert!(
            matches!(
                result,
                Err(CoreError::SchemaTooNew {
                    found: 99,
                    supported: SCHEMA_VERSION
                })
            ),
            "{:?}",
            result.map(|(_, info)| info)
        );
        assert_eq!(
            fs::read(&p.db).expect("read"),
            before,
            "the file is byte-for-byte unchanged"
        );
        assert!(
            files_in(&p.backups).is_empty(),
            "nothing quarantined or backed up"
        );
    }

    #[test]
    fn a_newer_schema_is_never_repaired_even_when_damage_was_recorded() {
        let p = paths();
        {
            let (store, _) = open(&p);
            store
                .conn()
                .pragma_update(None, "user_version", 99)
                .expect("bump version");
        }
        recovery::mark_damaged(&p.db, "found by a newer build's check").expect("mark");
        let before = fs::read(&p.db).expect("read");
        assert!(matches!(
            Store::open_at(&p.db, &p.backups),
            Err(CoreError::SchemaTooNew { found: 99, .. })
        ));
        assert_eq!(fs::read(&p.db).expect("read"), before);
        assert!(files_in(&p.backups).is_empty());
    }

    #[test]
    fn a_newer_schema_this_sqlite_cannot_parse_is_refused_not_quarantined() {
        let p = paths();
        let future_version = SCHEMA_VERSION + 1;
        {
            let (store, _) = open(&p);
            let conn = store.conn();
            conn.execute_batch("PRAGMA writable_schema = ON;")
                .expect("writable_schema");
            conn.execute(
                "INSERT INTO sqlite_schema (type, name, tbl_name, rootpage, sql) \
                 VALUES ('table', 'future', 'future', 0, 'CREATE TABLE future (x FUTURE SYNTAX ((')",
                [],
            )
            .expect("future schema row");
            conn.execute_batch(&format!(
                "PRAGMA writable_schema = OFF; PRAGMA user_version = {future_version};"
            ))
            .expect("bump");
        }
        let before = fs::read(&p.db).expect("read");
        let result = Store::open_at(&p.db, &p.backups);
        assert!(
            matches!(
                result,
                Err(CoreError::SchemaTooNew {
                    found,
                    supported: SCHEMA_VERSION
                }) if found == future_version
            ),
            "{:?}",
            result.map(|(_, i)| i)
        );
        assert_eq!(fs::read(&p.db).expect("read"), before);
        assert!(files_in(&p.backups).is_empty());
    }

    #[test]
    fn a_negative_schema_version_is_refused_without_writing() {
        let p = paths();
        {
            let (store, _) = open(&p);
            store
                .conn()
                .pragma_update(None, "user_version", -5)
                .expect("set");
        }
        let before = fs::read(&p.db).expect("read");
        let result = Store::open_at(&p.db, &p.backups);
        assert!(
            matches!(result, Err(CoreError::Migration(ref msg)) if msg.contains("-5")),
            "{:?}",
            result.map(|(_, i)| i)
        );
        assert_eq!(fs::read(&p.db).expect("read"), before);
    }

    /// Finding: when a step after the quarantine failed, the error claimed the database "was left
    /// unchanged", and the next open reported the empty replacement as Created, so the user never
    /// heard that their history had been moved aside.
    #[test]
    fn a_failure_after_the_move_names_the_quarantine_and_the_next_open_reports_it() {
        let p = paths();
        fs::create_dir_all(p.db.parent().expect("parent")).expect("mkdir");
        fs::write(&p.db, noise(16 * 1024)).expect("garbage");
        let broken = with_current(&[M::up("INSERT INTO no_such_table VALUES (1);")]);
        let result = open_with(&p.db, &p.backups, &broken);
        let Err(error) = &result else {
            panic!("expected an error, got {:?}", result.map(|(_, info)| info));
        };
        let CoreError::RecoveryIncomplete { quarantined, .. } = error else {
            panic!("expected RecoveryIncomplete, got {error:?}");
        };
        assert!(
            quarantined.is_file(),
            "the damaged file is where the error says"
        );
        let text = error.to_string();
        assert!(text.contains(&quarantined.display().to_string()), "{text}");
        assert!(!text.contains("left unchanged"), "{text}");

        let (_, info) = open(&p);
        assert_eq!(quarantined_path(&info.outcome), *quarantined);
        assert!(
            matches!(
                info.outcome,
                OpenOutcome::RecoveredFromCorruption {
                    salvage: Salvage::Nothing,
                    ..
                }
            ),
            "{:?}",
            info.outcome
        );
    }

    #[test]
    fn repairs_are_reported_until_acknowledged_and_then_only_the_file_stays() {
        let p = paths();
        fs::create_dir_all(p.db.parent().expect("parent")).expect("mkdir");
        fs::write(&p.db, noise(8192)).expect("garbage");
        let (store, info) = open(&p);
        let quarantined = quarantined_path(&info.outcome);
        assert!(
            !store
                .acknowledge_recovery("/not/what/was/reported")
                .expect("ack")
        );
        drop(store);
        let (store, again) = open(&p);
        assert_eq!(again.outcome, info.outcome, "not acknowledged yet");
        assert!(
            store
                .acknowledge_recovery(&quarantined.display().to_string())
                .expect("ack")
        );
        drop(store);
        let (_, after) = open(&p);
        assert_eq!(after.outcome, OpenOutcome::Opened);
        assert!(
            quarantined.is_file(),
            "acknowledging keeps the damaged copy"
        );
    }

    #[test]
    fn a_migration_reports_itself_once_then_a_pending_repair_shows_again() {
        let p = paths();
        fs::create_dir_all(p.db.parent().expect("parent")).expect("mkdir");
        fs::write(&p.db, noise(8192)).expect("garbage");
        let (_, repaired) = open(&p);
        assert!(matches!(
            repaired.outcome,
            OpenOutcome::RecoveredFromCorruption { .. }
        ));
        let later = with_current(&[M::up("CREATE TABLE later (x);")]);
        let (_, migrated) = open_with(&p.db, &p.backups, &later).expect("migrate");
        assert!(
            matches!(
                migrated.outcome,
                OpenOutcome::Migrated {
                    from: SCHEMA_VERSION,
                    to: NEXT,
                    ..
                }
            ),
            "{:?}",
            migrated.outcome
        );
        let (_, next) = open_with(&p.db, &p.backups, &later).expect("reopen");
        assert_eq!(next.outcome, repaired.outcome);
    }

    /// A later version that rebuilds `projects` (create, copy, drop, rename) while `chats` references
    /// it: the procedure SQLite documents for schema changes ALTER TABLE can't express.
    const REBUILD_PROJECTS: &str = "
        CREATE TABLE projects_new (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            color TEXT,
            icon TEXT,
            default_agent TEXT,
            default_model TEXT,
            sort_key TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            device_id TEXT NOT NULL,
            deleted INTEGER NOT NULL DEFAULT 0,
            archived INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO projects_new (id, name, color, icon, default_agent, default_model, sort_key, created_at, updated_at, device_id, deleted)
            SELECT id, name, color, icon, default_agent, default_model, sort_key, created_at, updated_at, device_id, deleted FROM projects;
        DROP TABLE projects;
        ALTER TABLE projects_new RENAME TO projects;
        CREATE INDEX projects_updated_at ON projects (updated_at);
    ";

    #[test]
    fn a_migration_backs_up_first_and_keeps_data() {
        let p = paths();
        {
            let (store, info) = open_with(&p.db, &p.backups, migrations()).expect("current");
            assert_eq!(info.outcome, OpenOutcome::Created);
            insert_project(store.conn(), "p1", "Alpha");
            insert_chat(store.conn(), "c1", "p1");
        }

        let later = with_current(&[M::up(REBUILD_PROJECTS).foreign_key_check()]);
        let (store, info) = open_with(&p.db, &p.backups, &later).expect("one version up");
        let OpenOutcome::Migrated {
            from: SCHEMA_VERSION,
            to: NEXT,
            backup: Some(backup),
        } = &info.outcome
        else {
            panic!(
                "expected Migrated {{ from: {SCHEMA_VERSION}, to: {NEXT}, backup: Some }}, got {:?}",
                info.outcome
            );
        };
        assert_eq!(info.schema_version, NEXT);
        let conn = store.conn();
        assert_eq!(project_names(conn), vec!["Alpha"]);
        let archived: i64 = conn
            .query_row("SELECT archived FROM projects WHERE id = 'p1'", [], |row| {
                row.get(0)
            })
            .expect("new column");
        assert_eq!(archived, 0);
        let chat_project: String = conn
            .query_row("SELECT project_id FROM chats WHERE id = 'c1'", [], |row| {
                row.get(0)
            })
            .expect("chat kept");
        assert_eq!(chat_project, "p1");
        let foreign_keys: i64 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("fk");
        assert_eq!(foreign_keys, 1, "enforcement is back on after migrating");

        // The backup is a complete database at the version before, with the old data.
        let backup = PathBuf::from(backup);
        assert_eq!(backup.parent(), Some(p.backups.as_path()));
        let name = fsutil::file_name_lossy(&backup);
        assert!(
            name.starts_with(&format!("aura.db.v{SCHEMA_VERSION}-")) && name.ends_with(".bak"),
            "{name}"
        );
        assert!(!fsutil::with_suffix(&backup, PARTIAL_SUFFIX).exists());
        let old = Connection::open(&backup).expect("open backup");
        assert_eq!(read_user_version(&old).expect("version"), CURRENT);
        assert_eq!(project_names(&old), vec!["Alpha"]);
        let old_columns = names(&old, "SELECT name FROM pragma_table_info('projects')");
        assert!(!old_columns.contains("archived"));
    }

    /// Migration 14, after the backup: saved verbatim folder locations take their plain spelling
    /// when it names the same folder; unsafe verbatim paths and every other path stay as saved,
    /// and the backup keeps the old spellings.
    #[test]
    fn migration_14_gives_verbatim_folder_locations_their_plain_spelling() {
        let p = paths();
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        let rows = [
            ("f1", r"\\?\C:\Users\me\work", r"C:\Users\me\work"),
            ("f2", r"\\?\UNC\server\share\repo", r"\\server\share\repo"),
            ("f3", r"\\?\C:\work\CON", r"\\?\C:\work\CON"),
            ("f4", r"\\?\C:\work\trailing.", r"\\?\C:\work\trailing."),
            ("f5", long.as_str(), long.as_str()),
            ("f6", r"D:\already\plain", r"D:\already\plain"),
            ("f7", "/Users/me/work", "/Users/me/work"),
        ];
        {
            let (store, info) =
                open_with(&p.db, &p.backups, &migrations()[..13]).expect("version 13");
            assert_eq!(info.schema_version, 13);
            let conn = store.conn();
            insert_project(conn, "p1", "Alpha");
            for (folder, saved, _) in &rows {
                conn.execute(
                    "INSERT INTO project_folders (id, project_id, display_name, kind, created_at, updated_at, device_id) \
                     VALUES (?1, 'p1', ?1, 'local', 1, 1, 'dev')",
                    [folder],
                )
                .expect("folder");
                conn.execute(
                    "INSERT INTO folder_locations (folder_id, device_id, path) VALUES (?1, 'dev', ?2)",
                    [folder, saved],
                )
                .expect("location");
            }
        }
        let (store, info) = open_with(&p.db, &p.backups, migrations()).expect("current");
        let OpenOutcome::Migrated {
            from: 13,
            to: SCHEMA_VERSION,
            backup: Some(backup),
        } = &info.outcome
        else {
            panic!(
                "expected Migrated from 13 to the current schema with a backup, got {:?}",
                info.outcome
            );
        };
        let path_of = |conn: &Connection, folder: &str| -> String {
            conn.query_row(
                "SELECT path FROM folder_locations WHERE folder_id = ?1",
                [folder],
                |row| row.get(0),
            )
            .expect("path")
        };
        for (folder, _, expected) in &rows {
            assert_eq!(path_of(store.conn(), folder), *expected, "{folder}");
        }
        let old = Connection::open(backup).expect("open backup");
        assert_eq!(read_user_version(&old).expect("version"), 13);
        for (folder, saved, _) in &rows {
            assert_eq!(path_of(&old, folder), *saved, "{folder} in the backup");
        }
    }

    /// Migrations on a v1 database with data in every table they touch: a backup first, every
    /// row kept, the new columns `NULL`, the new table and indexes in place, and the foreign key
    /// check clean.
    #[test]
    fn upgrading_a_populated_v1_database_keeps_every_row() {
        let p = paths();
        {
            let (store, info) = open_with(&p.db, &p.backups, &migrations()[..1]).expect("v1");
            assert_eq!(info.schema_version, 1);
            store
                .conn()
                .execute_batch(
                    "INSERT INTO projects (id, name, created_at, updated_at, device_id) \
                         VALUES ('p1', 'Alpha', 1, 1, 'dev'); \
                     INSERT INTO project_folders (id, project_id, display_name, kind, created_at, updated_at, device_id) \
                         VALUES ('f1', 'p1', 'repo', 'local', 1, 1, 'dev'); \
                     INSERT INTO folder_locations (folder_id, device_id, path) VALUES ('f1', 'dev', '/work/repo'); \
                     INSERT INTO chats (id, project_id, folder_id, title, title_source, active_agent, created_at, updated_at, device_id) \
                         VALUES ('c1', 'p1', 'f1', 'Fix the build', 'first_line', 'hermes', 1, 2, 'dev'); \
                     INSERT INTO agent_sessions (id, chat_id, agent_id, native_session_id, model, autonomy, cwd, created_at, updated_at, device_id) \
                         VALUES ('a1', 'c1', 'hermes', 'native-1', 'gpt', 'full_auto', '/work/repo', 1, 1, 'dev'); \
                     INSERT INTO turns (id, chat_id, agent_session_id, seq, status, stop_reason, started_at, ended_at, device_id) \
                         VALUES ('t1', 'c1', 'a1', 1, 'completed', 'end_turn', 1, 2, 'dev'); \
                     INSERT INTO events (id, chat_id, turn_id, seq, kind, payload, created_at, device_id) \
                         VALUES ('e1', 'c1', 't1', 0, 'turn_started', '{}', 1, 'dev'), \
                                ('e2', 'c1', 't1', 1, 'permission_requested', '{}', 1, 'dev');",
                )
                .expect("v1 rows");
        }

        let (store, info) = Store::open_at(&p.db, &p.backups).expect("upgrade");
        let OpenOutcome::Migrated {
            from: 1,
            to: SCHEMA_VERSION,
            backup: Some(backup),
        } = &info.outcome
        else {
            panic!("expected a migration from v1, got {:?}", info.outcome);
        };
        assert_eq!(info.schema_version, SCHEMA_VERSION);
        let old = Connection::open(backup).expect("backup");
        assert_eq!(read_user_version(&old).expect("version"), 1);
        assert_eq!(count(&old, "events"), 2, "the backup holds the v1 data");
        drop(old);

        let conn = store.conn();
        for table in [
            "projects",
            "project_folders",
            "folder_locations",
            "chats",
            "agent_sessions",
            "turns",
        ] {
            assert_eq!(count(conn, table), 1, "{table}");
        }
        assert_eq!(count(conn, "events"), 2);
        let host: String = conn
            .query_row("SELECT agent_host FROM chats WHERE id = 'c1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(host, r#"{"type":"native"}"#);
        let host: String = conn
            .query_row(
                "SELECT host FROM agent_sessions WHERE id = 'a1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(host, r#"{"type":"native"}"#);
        let (native, model, config): (String, String, Option<String>) = conn
            .query_row(
                "SELECT native_session_id, model, config_json FROM agent_sessions WHERE id = 'a1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("agent session");
        assert_eq!(
            (native.as_str(), model.as_str(), config),
            ("native-1", "gpt", None)
        );
        let turn: (String, Option<String>, Option<i64>, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT status, prompt_id, first_event_seq, last_event_seq, compacted_at FROM turns WHERE id = 't1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .expect("turn");
        assert_eq!(turn, ("completed".to_owned(), None, None, None, None));
        let violations: i64 = conn
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("foreign key check");
        assert_eq!(violations, 0);

        let indexes = names(
            conn,
            "SELECT name FROM sqlite_schema WHERE type = 'index' AND sql IS NOT NULL",
        );
        for index in [
            "turns_chat_first_event_seq",
            "turns_running",
            "turns_uncompacted",
            "events_requests",
            "queued_prompts_chat_id",
        ] {
            assert!(indexes.contains(index), "{index}: {indexes:?}");
        }

        // The new queue: rows default to not held, and the chat must exist.
        conn.execute(
            "INSERT INTO queued_prompts (id, chat_id, text, created_at, device_id) VALUES ('q1', 'c1', 'next', 3, 'dev')",
            [],
        )
        .expect("queue a prompt");
        let held: i64 = conn
            .query_row(
                "SELECT held FROM queued_prompts WHERE id = 'q1'",
                [],
                |row| row.get(0),
            )
            .expect("held");
        assert_eq!(held, 0);
        let orphan = conn.execute(
            "INSERT INTO queued_prompts (id, chat_id, text, created_at, device_id) VALUES ('q2', 'gone', 'x', 3, 'dev')",
            [],
        );
        assert!(orphan.is_err(), "a queued prompt needs its chat");
        drop(store);

        let (_, again) = open(&p);
        assert_eq!(again.outcome, OpenOutcome::Opened, "migrated once");
    }

    #[test]
    fn host_migration_backs_up_v2_and_preserves_native_chats() {
        let p = paths();
        {
            let (store, _) = open_with(&p.db, &p.backups, &migrations()[..2]).unwrap();
            store.conn().execute_batch(
                "INSERT INTO projects (id, name, created_at, updated_at, device_id) VALUES ('p', 'Work', 1, 1, 'dev');
                 INSERT INTO chats (id, project_id, active_agent, title, created_at, updated_at, device_id) VALUES ('c', 'p', 'codex', 'Keep this chat', 1, 2, 'dev');"
            ).unwrap();
        }
        let (store, info) = Store::open_at(&p.db, &p.backups).unwrap();
        let OpenOutcome::Migrated {
            from: 2,
            to: SCHEMA_VERSION,
            backup: Some(backup),
        } = info.outcome
        else {
            panic!("expected a backed-up host migration");
        };
        let original = Connection::open(backup).unwrap();
        assert_eq!(read_user_version(&original).unwrap(), 2);
        assert_eq!(count(&original, "chats"), 1);
        assert!(
            !names(&original, "SELECT name FROM pragma_table_info('chats')").contains("agent_host")
        );
        let preserved: (String, String, String) = store
            .conn()
            .query_row(
                "SELECT active_agent, title, agent_host FROM chats WHERE id = 'c'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            preserved,
            (
                "codex".into(),
                "Keep this chat".into(),
                r#"{"type":"native"}"#.into()
            )
        );
    }

    #[test]
    fn the_rebuild_migration_needs_foreign_keys_off() {
        // Control for the test above: the same SQL with enforcement on fails at the DROP, so that test
        // really exercises the foreign_keys=OFF handling.
        let p = paths();
        let (store, _) = open(&p);
        let conn = store.conn();
        insert_project(conn, "p1", "Alpha");
        insert_chat(conn, "c1", "p1");
        let result = conn.execute_batch(&format!("BEGIN; {REBUILD_PROJECTS} COMMIT;"));
        assert!(
            result.is_err(),
            "dropping a referenced table with foreign keys on must fail"
        );
        let _ = conn.execute_batch("ROLLBACK;");
    }

    #[test]
    fn a_failing_migration_changes_nothing() {
        let p = paths();
        {
            let (store, _) = open(&p);
            insert_project(store.conn(), "p1", "Alpha");
        }
        let broken = with_current(&[M::up(
            "CREATE TABLE half_done (x); INSERT INTO no_such_table VALUES (1);",
        )]);
        let result = open_with(&p.db, &p.backups, &broken);
        let Err(CoreError::Migration(message)) = &result else {
            panic!(
                "expected a migration error, got {:?}",
                result.map(|(_, info)| info)
            );
        };
        assert!(message.contains("no_such_table"), "{message}");
        assert!(
            !message.contains("CREATE TABLE"),
            "the UI message leaves the SQL out: {message}"
        );

        assert_eq!(user_version(&p.db), CURRENT);
        let (store, info) = open(&p);
        assert_eq!(info.outcome, OpenOutcome::Opened);
        assert_eq!(project_names(store.conn()), vec!["Alpha"]);
        let tables = names(
            store.conn(),
            "SELECT name FROM sqlite_schema WHERE type = 'table'",
        );
        assert!(
            !tables.contains("half_done"),
            "the partial migration was rolled back"
        );
    }

    #[test]
    fn a_migration_leaving_dangling_references_is_rolled_back() {
        let p = paths();
        {
            let (store, _) = open(&p);
            insert_project(store.conn(), "p1", "Alpha");
        }
        let dangling = with_current(&[M::up(
            "INSERT INTO chats (id, project_id, created_at, updated_at, device_id) VALUES ('orphan', 'gone', 1, 1, 'd');",
        )
        .foreign_key_check()]);
        let result = open_with(&p.db, &p.backups, &dangling);
        assert!(
            matches!(result, Err(CoreError::Migration(ref msg)) if msg.contains("foreign keys")),
            "{:?}",
            result.map(|(_, i)| i)
        );
        assert_eq!(user_version(&p.db), CURRENT);
        let (store, _) = open(&p);
        let chats: i64 = store
            .conn()
            .query_row("SELECT count(*) FROM chats", [], |row| row.get(0))
            .expect("count");
        assert_eq!(chats, 0);
    }

    #[test]
    fn backup_retention_keeps_the_five_newest() {
        let p = paths();
        drop(open_with(&p.db, &p.backups, migrations()).expect("current"));
        let sql: Vec<String> = (1..=7)
            .map(|n| format!("CREATE TABLE extra_{n} (x);"))
            .collect();
        let mut set = migrations().to_vec();
        for (index, statement) in sql.iter().enumerate() {
            set.push(M::up(statement));
            let (_, info) = open_with(&p.db, &p.backups, &set).expect("migrate");
            let from = SCHEMA_VERSION + u32::try_from(index).expect("small");
            assert!(
                matches!(info.outcome, OpenOutcome::Migrated { from: f, .. } if f == from),
                "{:?}",
                info.outcome
            );
        }
        // Seven migrations made seven backups (from the current version up); the two oldest were
        // pruned.
        let kept: BTreeSet<u32> = files_in(&p.backups)
            .iter()
            .filter_map(|name| parse_backup_name(name, "aura.db"))
            .map(|key| key.from)
            .collect();
        assert_eq!(kept, (SCHEMA_VERSION + 2..SCHEMA_VERSION + 7).collect());
        let bak_files = files_in(&p.backups)
            .iter()
            .filter(|name| name.ends_with(".bak"))
            .count();
        assert_eq!(bak_files, 5);
    }

    /// Finding: after the clock was set back, existing backups could carry later timestamps than the
    /// new one, so pruning by timestamp deleted the backup just taken and the migration ran without
    /// one, reporting a path that didn't exist.
    #[test]
    fn the_backup_just_taken_survives_pruning_even_with_backups_dated_in_the_future() {
        let p = paths();
        {
            let (store, _) = open(&p);
            insert_project(store.conn(), "p1", "Alpha");
        }
        fs::create_dir_all(&p.backups).expect("mkdir");
        let future = now_millis() + 10 * 365 * 24 * 3600 * 1000;
        for n in 0..5 {
            fs::write(
                p.backups.join(format!("aura.db.v1-{}.bak", future + n)),
                b"from when the clock was ahead",
            )
            .expect("future backup");
        }
        let later = with_current(&[M::up("CREATE TABLE later (x);")]);
        let (_, info) = open_with(&p.db, &p.backups, &later).expect("migrate");
        let OpenOutcome::Migrated {
            backup: Some(backup),
            ..
        } = &info.outcome
        else {
            panic!("expected Migrated, got {:?}", info.outcome);
        };
        let backup = PathBuf::from(backup);
        assert!(backup.is_file(), "the reported backup exists");
        assert_eq!(
            project_names(&Connection::open(&backup).expect("backup")),
            vec!["Alpha"]
        );
        let bak: BTreeSet<String> = files_in(&p.backups)
            .into_iter()
            .filter(|name| name.ends_with(".bak"))
            .collect();
        assert_eq!(bak.len(), BACKUPS_TO_KEEP, "{bak:?}");
        assert!(
            !bak.contains(&format!("aura.db.v1-{future}.bak")),
            "the oldest of the others went: {bak:?}"
        );
    }

    #[test]
    fn pruning_orders_by_time_protects_the_new_backup_and_never_touches_unknown_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let names = [
            "aura.db.v1-100.bak",
            "aura.db.v1-300.bak",
            "aura.db.v2-200.bak",
            "aura.db.v2-200.1.bak",
            "aura.db.v3-50.bak",
            "aura.db.v3-400.bak",
            "aura.db.v3-400.bak-wal",
            "aura.db.vX-1.bak",
            "aura.db.corrupt-1",
            "other.db.v1-1.bak",
            "notes.txt",
        ];
        for name in names {
            fs::write(dir.path().join(name), b"x").expect("write");
        }
        // The protected one has the oldest timestamp; it still counts as one of the three kept.
        prune_backups(
            dir.path(),
            "aura.db",
            3,
            &dir.path().join("aura.db.v3-50.bak"),
        );
        let left = files_in(dir.path());
        let expected: BTreeSet<String> = [
            "aura.db.v3-50.bak",
            "aura.db.v3-400.bak",
            "aura.db.v3-400.bak-wal",
            "aura.db.v1-300.bak",
            "aura.db.vX-1.bak",
            "aura.db.corrupt-1",
            "other.db.v1-1.bak",
            "notes.txt",
        ]
        .iter()
        .map(|name| name.to_string())
        .collect();
        assert_eq!(left, expected);
    }

    /// Finding: a backup interrupted mid-copy left a `.bak.partial` as big as the database that
    /// nothing ever removed.
    #[test]
    fn what_an_interrupted_backup_left_is_removed() {
        let p = paths();
        drop(open(&p));
        fs::create_dir_all(&p.backups).expect("mkdir");
        let leftovers = [
            "aura.db.v1-123.bak.partial",
            "aura.db.v1-123.bak.partial-journal",
            "aura.db.v1-124.2.bak.partial",
        ];
        let unrelated = [
            "aura.db.corrupt-1.partial",
            "notes.partial",
            "other.db.v1-5.bak.partial",
            "aura.db.v1-5.bak",
        ];
        for name in leftovers.iter().chain(unrelated.iter()) {
            fs::write(p.backups.join(name), b"x").expect("write");
        }
        drop(open(&p));
        let left = files_in(&p.backups);
        for name in leftovers {
            assert!(!left.contains(name), "{name} should be gone: {left:?}");
        }
        for name in unrelated {
            assert!(left.contains(name), "{name} should stay: {left:?}");
        }
    }

    #[test]
    fn backup_names_parse() {
        let key = |name: &str| parse_backup_name(name, "aura.db");
        assert_eq!(
            key("aura.db.v1-1700.bak"),
            Some(BackupKey {
                millis: 1700,
                from: 1,
                counter: 0
            })
        );
        assert_eq!(
            key("aura.db.v12-1700.3.bak"),
            Some(BackupKey {
                millis: 1700,
                from: 12,
                counter: 3
            })
        );
        assert_eq!(key("aura.db.v1-1700.bak.partial"), None);
        assert_eq!(key("aura.db.v1-abc.bak"), None);
        assert_eq!(key("aura.db.corrupt-1700"), None);
        assert_eq!(key("aura.dbx.v1-1700.bak"), None);
    }

    #[test]
    fn paths_with_spaces_and_unicode_work() {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = dir.path().join("My Data").join("Ünïcödé 🚀 profile");
        let db = profile.join("aura.db");
        let backups = profile.join("back ups");
        {
            let (store, info) = Store::open_at(&db, &backups).expect("create");
            assert_eq!(info.outcome, OpenOutcome::Created);
            insert_project(store.conn(), "p1", "Ünïcödé");
        }
        let later = with_current(&[M::up("CREATE TABLE later (x);")]);
        let (store, info) = open_with(&db, &backups, &later).expect("migrate");
        let OpenOutcome::Migrated {
            backup: Some(backup),
            ..
        } = &info.outcome
        else {
            panic!("expected Migrated, got {:?}", info.outcome);
        };
        assert!(Path::new(backup).is_file());
        assert_eq!(project_names(store.conn()), vec!["Ünïcödé"]);
        drop(store);
        // A repair through the same odd path (VACUUM INTO gets the path as text).
        recovery::mark_damaged(&db, "test").expect("mark");
        let (store, info) = open_with(&db, &backups, &later).expect("repair");
        assert!(matches!(
            info.outcome,
            OpenOutcome::RecoveredFromCorruption {
                salvage: Salvage::Complete,
                ..
            }
        ));
        assert_eq!(project_names(store.conn()), vec!["Ünïcödé"]);
    }

    #[test]
    fn a_stale_wal_next_to_a_missing_database_does_not_resurface() {
        let p = paths();
        fs::create_dir_all(p.db.parent().expect("parent")).expect("mkdir");
        let wal = fsutil::with_suffix(&p.db, "-wal");
        {
            let conn = Connection::open(&p.db).expect("raw");
            conn.execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE ghost (x); INSERT INTO ghost VALUES ('old');")
                .expect("old data");
            // Close without checkpointing, so the committed rows exist only in the -wal, as after a
            // crash. (Copying the -wal while it is open would fail on Windows.)
            conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
                .expect("keep the wal");
        }
        assert!(fs::metadata(&wal).expect("wal kept").len() > 0);
        fs::remove_file(&p.db).expect("delete the database by hand, leaving its -wal");
        let _ = fs::remove_file(fsutil::with_suffix(&p.db, "-shm"));

        let (store, info) = open(&p);
        assert_eq!(info.outcome, OpenOutcome::Created);
        let tables = names(
            store.conn(),
            "SELECT name FROM sqlite_schema WHERE type = 'table'",
        );
        assert!(!tables.contains("ghost"), "{tables:?}");
        assert!(tables.contains("projects"));
    }

    #[test]
    fn a_locked_database_is_an_error_not_damage() {
        let p = paths();
        drop(open(&p));
        let locker = Connection::open(&p.db).expect("locker");
        locker
            .execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE; INSERT INTO settings VALUES ('k', 'v', 1);")
            .expect("hold an exclusive lock");

        let result = open_inner(
            &p.db,
            &p.backups,
            migrations(),
            Duration::from_millis(50),
            Locking::On,
        );
        let Err(CoreError::Sqlite(error)) = &result else {
            panic!(
                "expected a busy error, got {:?}",
                result.map(|(_, info)| info)
            );
        };
        assert_eq!(error.sqlite_error_code(), Some(ErrorCode::DatabaseBusy));
        assert!(p.db.is_file(), "the database stays in place");
        assert!(files_in(&p.backups).is_empty(), "nothing was quarantined");
        assert!(!recovery::damage_marker(&p.db).exists());

        locker.execute_batch("COMMIT;").expect("commit");
        drop(locker);
        let (store, info) = open(&p);
        assert_eq!(info.outcome, OpenOutcome::Opened);
        let value: String = store
            .conn()
            .query_row("SELECT value FROM settings WHERE key = 'k'", [], |row| {
                row.get(0)
            })
            .expect("kept");
        assert_eq!(value, "v");
    }

    /// Finding: two processes creating a profile database at the same moment failed about half the
    /// time ("database is locked", "table settings already exists"). The open lock serializes them.
    #[test]
    fn simultaneous_first_opens_of_one_database_all_succeed() {
        for trial in 0..25 {
            let p = paths();
            let outcomes: Vec<Result<OpenOutcome>> = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..4)
                    .map(|_| {
                        scope.spawn(|| {
                            Store::open_at(&p.db, &p.backups).map(|(store, info)| {
                                insert_project(
                                    store.conn(),
                                    &crate::ids::new_id(),
                                    "from one opener",
                                );
                                info.outcome
                            })
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| handle.join().expect("thread"))
                    .collect()
            });
            let mut created = 0;
            for outcome in outcomes {
                match outcome {
                    Ok(OpenOutcome::Created) => created += 1,
                    Ok(OpenOutcome::Opened) => {}
                    other => panic!("trial {trial}: {other:?}"),
                }
            }
            assert_eq!(created, 1, "trial {trial}: exactly one opener created it");
            let (store, _) = open(&p);
            assert_eq!(count(store.conn(), "projects"), 4, "trial {trial}");
        }
    }

    /// The same race between openers that don't take the open lock (an older build, a tool): the
    /// WAL switch retries and migrations run under BEGIN IMMEDIATE with the version read again, so
    /// they still all succeed.
    #[test]
    fn simultaneous_first_opens_succeed_even_without_the_open_lock() {
        for trial in 0..25 {
            let p = paths();
            let results: Vec<Result<OpenOutcome>> = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..2)
                    .map(|_| {
                        scope.spawn(|| {
                            open_inner(&p.db, &p.backups, migrations(), BUSY_TIMEOUT, Locking::Off)
                                .map(|(_, info)| info.outcome)
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| handle.join().expect("thread"))
                    .collect()
            });
            for result in results {
                assert!(
                    matches!(result, Ok(OpenOutcome::Created | OpenOutcome::Opened)),
                    "trial {trial}: {result:?}"
                );
            }
            assert_eq!(user_version(&p.db), CURRENT, "trial {trial}");
        }
    }

    #[test]
    fn an_open_gives_up_when_another_process_takes_too_long_to_open() {
        let p = paths();
        drop(open(&p));
        let other = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock::lock_path(&p.db))
            .expect("lock file");
        other.try_lock().expect("another process is opening");
        let result = open_inner(
            &p.db,
            &p.backups,
            migrations(),
            Duration::from_millis(100),
            Locking::On,
        );
        assert!(
            matches!(
                result,
                Err(CoreError::DatabaseInUse {
                    reason: InUseReason::Opening,
                    ..
                })
            ),
            "{:?}",
            result.map(|(_, info)| info)
        );
        drop(other);
        assert_eq!(open(&p).1.outcome, OpenOutcome::Opened);
    }

    /// With another process holding the database open, an open may use it but never repair (move)
    /// or migrate it underneath that process. Once it closes, both happen.
    #[test]
    fn a_database_in_use_elsewhere_is_neither_repaired_nor_migrated() {
        let p = paths();
        let (first, _) = open(&p);
        insert_project(first.conn(), "p1", "Alpha");

        let later = with_current(&[M::up("CREATE TABLE later (x);")]);
        let result = open_with(&p.db, &p.backups, &later);
        assert!(
            matches!(
                result,
                Err(CoreError::DatabaseInUse {
                    reason: InUseReason::Upgrade,
                    ..
                })
            ),
            "{:?}",
            result.map(|(_, info)| info)
        );
        assert_eq!(user_version(&p.db), CURRENT);

        let (second, info) = open(&p);
        assert_eq!(
            info.outcome,
            OpenOutcome::Opened,
            "a healthy, current database opens"
        );
        assert!(
            !info.unclean_shutdown,
            "the other process's -wal is not a crash"
        );
        assert_eq!(project_names(second.conn()), vec!["Alpha"]);
        drop(second);

        first
            .note_query_error(&rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                Some("database disk image is malformed".into()),
            ))
            .expect("record damage");
        let result = Store::open_at(&p.db, &p.backups);
        assert!(
            matches!(
                result,
                Err(CoreError::DatabaseInUse {
                    reason: InUseReason::Repair,
                    ..
                })
            ),
            "{:?}",
            result.map(|(_, info)| info)
        );
        assert!(p.db.is_file(), "not moved while in use");
        assert!(files_in(&p.backups).is_empty());
        assert!(recovery::damage_marker(&p.db).is_file(), "still recorded");

        drop(first);
        let (store, info) = open(&p);
        assert!(matches!(
            info.outcome,
            OpenOutcome::RecoveredFromCorruption {
                salvage: Salvage::Complete,
                ..
            }
        ));
        assert_eq!(project_names(store.conn()), vec!["Alpha"]);
        drop(store);
        let (_, migrated) = open_with(&p.db, &p.backups, &later).expect("migrate once alone");
        assert!(matches!(
            migrated.outcome,
            OpenOutcome::Migrated {
                from: SCHEMA_VERSION,
                to: NEXT,
                ..
            }
        ));
    }

    #[test]
    fn the_wal_switch_waits_for_a_reader_to_finish() {
        let p = paths();
        drop(open(&p));
        {
            let conn = Connection::open(&p.db).expect("raw");
            let mode: String = conn
                .pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))
                .expect("leave WAL");
            assert_eq!(mode, "delete");
        }
        let reader = Connection::open(&p.db).expect("reader");
        reader
            .execute_batch("BEGIN; SELECT count(*) FROM projects;")
            .expect("hold a shared lock");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            reader.execute_batch("COMMIT;").expect("release");
        });
        let (store, info) = open(&p);
        release.join().expect("thread");
        assert_eq!(info.outcome, OpenOutcome::Opened);
        let journal: String = store
            .conn()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal_mode");
        assert_eq!(journal, "wal");
    }

    #[test]
    fn a_directory_in_place_of_the_database_is_an_error_and_left_alone() {
        let p = paths();
        fs::create_dir_all(&p.db).expect("directory named aura.db");
        assert!(Store::open_at(&p.db, &p.backups).is_err());
        assert!(p.db.is_dir());
        assert!(files_in(&p.backups).is_empty());
    }

    #[test]
    fn open_uses_the_profile_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let profiles = crate::profile::Profiles::new(crate::paths::DataRoot::at(dir.path()));
        let profile = profiles.open_active().expect("profile");
        let (store, info) = Store::open(&profile).expect("open");
        assert_eq!(info.path, profile.db_path());
        assert_eq!(store.path(), profile.db_path());
        assert_eq!(store.backups_dir(), profile.backups_dir());
        assert_eq!(info.outcome, OpenOutcome::Created);
    }

    #[test]
    fn outcomes_serialize_for_the_ui() {
        let value = serde_json::to_value(OpenOutcome::Migrated {
            from: 1,
            to: 2,
            backup: Some("/b/aura.db.v1-5.bak".into()),
        })
        .expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({"type": "migrated", "from": 1, "to": 2, "backup": "/b/aura.db.v1-5.bak"})
        );
        assert_eq!(
            serde_json::to_value(OpenOutcome::Created).expect("serialize"),
            serde_json::json!({"type": "created"})
        );
        let value = serde_json::to_value(OpenOutcome::RecoveredFromCorruption {
            quarantined: "/q".into(),
            salvage: Salvage::Partial {
                tables: vec!["events".into()],
            },
        })
        .expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "type": "recovered_from_corruption",
                "quarantined": "/q",
                "salvage": {"type": "partial", "tables": ["events"]}
            })
        );
        for (salvage, json) in [
            (Salvage::Complete, serde_json::json!({"type": "complete"})),
            (Salvage::Nothing, serde_json::json!({"type": "nothing"})),
        ] {
            assert_eq!(serde_json::to_value(&salvage).expect("serialize"), json);
            let back: Salvage = serde_json::from_value(json).expect("deserialize");
            assert_eq!(back, salvage);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_directory_is_an_error_not_a_panic() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).expect("mkdir");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).expect("chmod");
        // Root (and some container setups) ignore directory permissions; there is nothing to test then.
        if fs::write(locked.join("probe"), b"x").is_ok() {
            eprintln!("skipping: directory permissions are not enforced for this user");
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("chmod back");
            return;
        }
        let result = Store::open_at(&locked.join("aura.db"), &locked.join("backups"));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("chmod back");
        assert!(result.is_err(), "{:?}", result.map(|(_, info)| info));
        assert!(!locked.join("aura.db").exists());
    }

    /// A repair that fails before the move (here: its record can't be written into `backups/`)
    /// changes nothing: the damaged file stays put, the error says so, the copy it made is gone, and
    /// the damage stays recorded for the next try.
    #[cfg(unix)]
    #[test]
    fn a_repair_that_fails_before_the_move_leaves_everything_in_place() {
        use std::os::unix::fs::PermissionsExt;
        let p = paths();
        {
            let (store, _) = open(&p);
            insert_project(store.conn(), "p1", "Alpha");
        }
        recovery::mark_damaged(&p.db, "found by a check").expect("mark");
        fs::create_dir_all(&p.backups).expect("mkdir");
        fs::set_permissions(&p.backups, fs::Permissions::from_mode(0o555)).expect("chmod");
        if fs::write(p.backups.join("probe"), b"x").is_ok() {
            eprintln!("skipping: directory permissions are not enforced for this user");
            fs::set_permissions(&p.backups, fs::Permissions::from_mode(0o700)).expect("chmod back");
            return;
        }
        let before = fs::read(&p.db).expect("read");
        let result = Store::open_at(&p.db, &p.backups);
        fs::set_permissions(&p.backups, fs::Permissions::from_mode(0o700)).expect("chmod back");
        let Err(error) = &result else {
            panic!("expected an error, got {:?}", result.map(|(_, info)| info));
        };
        assert!(matches!(error, CoreError::RepairFailed { .. }), "{error:?}");
        assert!(error.to_string().contains("left where it is"), "{error}");
        assert_eq!(fs::read(&p.db).expect("read"), before);
        assert!(recovery::damage_marker(&p.db).is_file());
        assert!(files_in(&p.backups).is_empty(), "nothing was moved");
        let leftovers: Vec<String> = files_in(p.db.parent().expect("profile"))
            .into_iter()
            .filter(|name| name.contains("salvage"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the copy was cleaned up: {leftovers:?}"
        );

        let (store, info) = open(&p);
        assert!(matches!(
            info.outcome,
            OpenOutcome::RecoveredFromCorruption { .. }
        ));
        assert_eq!(project_names(store.conn()), vec!["Alpha"]);
    }
}
