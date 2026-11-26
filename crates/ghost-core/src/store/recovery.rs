//! Repairing a damaged database, and remembering that it happened.
//!
//! Two small files carry the state from one open to the next, so neither a repair nor its notice
//! can get lost:
//! - `<db>.damaged`: damage that [`check_integrity`](super::check_integrity) or a failed query
//!   found while the database was in use. The next open that has the database to itself repairs it.
//! - `backups/<db>.corrupt-<millis>.json`, next to the moved file: a repair happened. Every open
//!   reports it as [`OpenOutcome::RecoveredFromCorruption`] until
//!   [`Store::acknowledge_recovery`](super::Store::acknowledge_recovery) removes it. It is written
//!   before the damaged file is moved, so the file is never moved without it. One this build can't
//!   read (torn, or written by a newer build) still reports the repair, without saying how much
//!   of the database it kept.
//!
//! A repair copies what can be read into `<db>.salvage-<millis>.partial` (the damaged file is only
//! read), writes the record, gives the damaged file's `-wal` and `-journal` their names in
//! `backups/`, moves the damaged file there, and renames the copy into place. Until the move, a
//! failure changes nothing. From the move on, the moved file has its `-wal` whatever happens, and
//! a failure leaves the record and the copy in place: the next open installs the copy and reports
//! the repair.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite_migration::M;
use serde::{Deserialize, Serialize};

use super::{OpenOutcome, Salvage, db_file_name, is_non_empty_file, remove_with_sidecars, salvage};
use crate::error::{CoreError, Result};
use crate::fsutil::{self, JsonFile, PARTIAL_SUFFIX, SQLITE_SIDECARS};
use crate::time::now_millis;

const DAMAGE_SUFFIX: &str = ".damaged";
const SALVAGE_INFIX: &str = ".salvage-";
const RECORD_SUFFIX: &str = ".json";
const QUARANTINE_INFIX: &str = ".corrupt-";

/// A repair that moved a damaged database aside.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Recovery {
    pub(super) quarantined: PathBuf,
    pub(super) salvage: Salvage,
}

impl Recovery {
    pub(super) fn outcome(&self) -> OpenOutcome {
        OpenOutcome::RecoveredFromCorruption {
            quarantined: self.quarantined.display().to_string(),
            salvage: self.salvage.clone(),
        }
    }
}

/// The contents of `<db>.damaged`.
#[derive(Debug, Serialize, Deserialize)]
struct DamageMarker {
    problem: String,
    /// Unix ms.
    found_at: i64,
}

/// The contents of `<quarantined>.json`.
#[derive(Debug, Serialize, Deserialize)]
struct RecoveryRecord {
    /// What the probe, check, or query reported.
    problem: String,
    salvage: Salvage,
    /// Unix ms.
    recovered_at: i64,
}

/// How much a repair kept when its record doesn't say: some records were lost, it isn't known which.
fn unknown_salvage() -> Salvage {
    Salvage::Partial { tables: Vec::new() }
}

/// `<db>.damaged`
pub(super) fn damage_marker(db_path: &Path) -> PathBuf {
    fsutil::with_suffix(db_path, DAMAGE_SUFFIX)
}

/// Where the repair that moves the database to `quarantined` builds its copy:
/// `<db>.salvage-<millis>.partial`, next to the database so installing it is a rename within one
/// folder, and named after that repair so a copy left waiting is never taken for another repair's.
pub(super) fn salvage_path(db_path: &Path, quarantined: &Path) -> PathBuf {
    let name = fsutil::file_name_lossy(quarantined);
    let key = name
        .rsplit_once(QUARANTINE_INFIX)
        .map_or(name.as_str(), |(_, key)| key);
    fsutil::with_suffix(db_path, &format!("{SALVAGE_INFIX}{key}{PARTIAL_SUFFIX}"))
}

/// The quarantined file of the repair that built `copy` ([`salvage_path`] the other way round).
fn quarantined_for(copy: &Path, db_path: &Path, backups_dir: &Path) -> Option<PathBuf> {
    let db_name = db_file_name(db_path);
    let name = fsutil::file_name_lossy(copy);
    let key = name
        .strip_prefix(&format!("{db_name}{SALVAGE_INFIX}"))?
        .strip_suffix(PARTIAL_SUFFIX)?;
    Some(backups_dir.join(format!("{db_name}{QUARANTINE_INFIX}{key}")))
}

/// Every salvage copy of this database, with sidecars folded into the file they belong to.
fn salvage_copies(db_path: &Path) -> Vec<PathBuf> {
    let dir = fsutil::parent_dir(db_path);
    let prefix = format!("{}{SALVAGE_INFIX}", db_file_name(db_path));
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut copies: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let base = SQLITE_SIDECARS
                .iter()
                .find_map(|suffix| name.strip_suffix(suffix))
                .unwrap_or(&name);
            (base.starts_with(prefix.as_str()) && base.ends_with(PARTIAL_SUFFIX))
                .then(|| dir.join(base))
        })
        .collect();
    copies.sort();
    copies.dedup();
    copies
}

/// Records damage found while the database was in use, for the next exclusive open to repair. The
/// first problem found is kept, since it is the closest to the cause.
pub(super) fn mark_damaged(db_path: &Path, problem: &str) -> Result<()> {
    let marker = damage_marker(db_path);
    if fs::symlink_metadata(&marker).is_ok() {
        return Ok(());
    }
    fsutil::write_json_atomic(
        &marker,
        &DamageMarker {
            problem: problem.to_string(),
            found_at: now_millis(),
        },
    )?;
    tracing::warn!(%problem, db = %db_path.display(), "recorded damage; the next open repairs the database");
    Ok(())
}

/// The damage recorded by [`mark_damaged`], if any.
pub(super) fn recorded_damage(db_path: &Path) -> Option<String> {
    match fsutil::read_json::<DamageMarker>(&damage_marker(db_path)) {
        JsonFile::Missing => None,
        JsonFile::Valid(marker) => Some(marker.problem),
        // Something is there, so damage was recorded even if the details are lost.
        JsonFile::Unusable(error) | JsonFile::Unreadable(error) => {
            Some(format!("damage was recorded earlier ({error})"))
        }
    }
}

pub(super) fn clear_recorded_damage(db_path: &Path) {
    let marker = damage_marker(db_path);
    match fs::remove_file(&marker) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(marker = %marker.display(), %error, "could not remove the damage record")
        }
    }
}

/// A recovery record in `backups/`.
#[derive(Debug)]
struct RecordFile {
    quarantined: PathBuf,
    path: PathBuf,
    /// The quarantine's millisecond, then `free_path`'s collision counter: newer sorts greater.
    key: (i64, u32),
}

/// This database's recovery records, newest first.
fn record_files(db_path: &Path, backups_dir: &Path) -> Vec<RecordFile> {
    let prefix = format!("{}{QUARANTINE_INFIX}", db_file_name(db_path));
    let Ok(entries) = fs::read_dir(backups_dir) else {
        return Vec::new();
    };
    let mut files: Vec<RecordFile> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(RECORD_SUFFIX)?;
            let rest = stem.strip_prefix(prefix.as_str())?;
            let (millis, counter) = match rest.split_once('.') {
                Some((millis, counter)) => (millis, counter.parse().unwrap_or(0)),
                None => (rest, 0),
            };
            Some(RecordFile {
                quarantined: backups_dir.join(stem),
                path: entry.path(),
                key: (millis.parse().ok()?, counter),
            })
        })
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.key));
    files
}

/// What the record at `path` says the repair kept: `None` when there is no record (any more), and
/// [`unknown_salvage`] when there is one this build can't read (torn, written by a newer build with
/// a salvage this one doesn't know, or unreadable just now), since it still proves the repair.
fn recorded_salvage(path: &Path) -> Option<Salvage> {
    match fsutil::read_json::<RecoveryRecord>(path) {
        JsonFile::Valid(record) => Some(record.salvage),
        JsonFile::Missing => None,
        JsonFile::Unusable(error) | JsonFile::Unreadable(error) => {
            tracing::warn!(%error, "a repair record can't be read; reporting the repair without its details");
            Some(unknown_salvage())
        }
    }
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// The newest repair the UI hasn't acknowledged yet, whose damaged copy is still there.
pub(super) fn newest_pending(db_path: &Path, backups_dir: &Path) -> Option<Recovery> {
    record_files(db_path, backups_dir)
        .into_iter()
        .filter(|file| exists(&file.quarantined))
        .find_map(|file| {
            recorded_salvage(&file.path).map(|salvage| Recovery {
                quarantined: file.quarantined,
                salvage,
            })
        })
}

/// Finishes a repair an earlier open couldn't complete (the damaged file was moved, but its copy
/// never went in), drops records whose damaged copy is gone, and deletes what repairs that never
/// moved the database left. Returns the repair it finished. Only called while holding the open
/// lock exclusively.
///
/// A copy is waiting when the database is missing and the file its repair moved is in `backups/`.
/// A repair only starts on a database that exists, so at most one copy can be waiting. It is
/// matched to its repair by name, which a clock set back between two repairs can't confuse, and
/// installed whatever its record says, or even without one: the copy is what the repair salvaged.
/// A copy whose moved file is still there is never deleted.
pub(super) fn resume(db_path: &Path, backups_dir: &Path) -> Result<Option<Recovery>> {
    for file in record_files(db_path, backups_dir) {
        if !exists(&file.quarantined) {
            // The repair stopped between writing the record and moving the file (which is still in
            // place), or the user deleted the damaged copy: nothing left to report. The sidecar
            // names a stopped repair made next to it go too; they only ever pointed at the live
            // database's.
            remove_sidecars(&file.quarantined);
            if let Err(error) = fs::remove_file(&file.path) {
                tracing::warn!(record = %file.path.display(), %error, "could not remove a stale repair record");
            }
        }
    }
    let database_missing = !is_non_empty_file(db_path);
    let mut waiting = None;
    for copy in salvage_copies(db_path) {
        match quarantined_for(&copy, db_path, backups_dir) {
            Some(quarantined) if exists(&quarantined) => {
                if database_missing && waiting.is_none() {
                    waiting = Some((copy, quarantined));
                } else {
                    tracing::warn!(copy = %copy.display(), "keeping a salvaged copy whose damaged database is still kept");
                }
            }
            // A repair that never moved the database (or the scratch copy of one).
            _ => {
                tracing::info!(copy = %copy.display(), "removing what an interrupted repair left");
                remove_with_sidecars(&copy);
            }
        }
    }
    let Some((copy, quarantined)) = waiting else {
        return Ok(None);
    };
    let salvage = match recorded_salvage(&fsutil::with_suffix(&quarantined, RECORD_SUFFIX)) {
        // No record, or one saying nothing was salvaged although a copy is waiting: how much the
        // copy holds is unknown.
        Some(Salvage::Nothing) | None => unknown_salvage(),
        Some(salvage) => salvage,
    };
    tracing::info!(quarantined = %quarantined.display(), "finishing an interrupted repair");
    finish(db_path, Some(&copy), quarantined, salvage).map(Some)
}

/// Repairs the damaged database at `db_path` (the steps are in the module docs). `migrations` are
/// the store's, for the salvage. Only called while holding the open lock exclusively, so no other
/// Ghost process has the file open.
pub(super) fn recover(
    db_path: &Path,
    backups_dir: &Path,
    problem: &str,
    migrations: &[M<'_>],
) -> Result<Recovery> {
    let failed = |source: CoreError| CoreError::RepairFailed {
        path: db_path.to_path_buf(),
        source: Box::new(source),
    };
    fsutil::create_private_dir_all(backups_dir).map_err(failed)?;
    let base = format!(
        "{}{QUARANTINE_INFIX}{}",
        db_file_name(db_path),
        now_millis()
    );
    let target = fsutil::free_path(backups_dir, &base, "");
    let record_path = fsutil::with_suffix(&target, RECORD_SUFFIX);
    let copy = salvage_path(db_path, &target);

    tracing::warn!(%problem, db = %db_path.display(), "the database is damaged; copying out what can be read");
    let salvage = salvage::salvage(db_path, &copy, migrations).map_err(failed)?;
    let record = RecoveryRecord {
        problem: problem.to_string(),
        salvage: salvage.clone(),
        recovered_at: now_millis(),
    };
    // Nothing has changed until the move succeeds, so a failure before it only removes what the
    // repair made.
    let undo = |error: CoreError| {
        match fs::remove_file(&record_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(record = %record_path.display(), %error, "could not remove the record of a repair that didn't happen")
            }
        }
        remove_with_sidecars(&copy);
        failed(error)
    };
    fsutil::write_json_atomic(&record_path, &record).map_err(undo)?;
    quarantine(db_path, &target).map_err(undo)?;
    tracing::warn!(quarantined = %target.display(), ?salvage, "moved the damaged database aside");
    let copy = (salvage != Salvage::Nothing).then_some(copy.as_path());
    finish(db_path, copy, target, salvage)
}

/// Moves the damaged database to `target`. Its `-wal` and `-journal` get their names there first,
/// keeping their suffixes so SQLite can still open the set (a hard link, or a synced copy where the
/// filesystem has no hard links): the moved file is never without its `-wal`, which holds the
/// transactions not yet written to the main file (usual after the crash that caused the damage),
/// even if the repair stops right after the move. The names at the database path, and the `-shm`,
/// go in [`finish`]. On failure nothing changed.
pub(super) fn quarantine(db_path: &Path, target: &Path) -> Result<()> {
    if let Err(error) = link_sidecars(db_path, target) {
        remove_sidecars(target);
        return Err(error);
    }
    fsutil::sync_dir(fsutil::parent_dir(target));
    if let Err(source) = fs::rename(db_path, target) {
        remove_sidecars(target);
        return Err(CoreError::Io {
            path: db_path.to_path_buf(),
            source,
        });
    }
    fsutil::sync_dir(fsutil::parent_dir(target));
    fsutil::sync_dir(fsutil::parent_dir(db_path));
    Ok(())
}

/// The first half of [`quarantine`]'s sidecar handling. Never the `-shm`: it is only an index SQLite
/// rebuilds, and it is shared memory, matched to whoever opens it by its file. Two names for it
/// would let the new database and the moved one share an index if both were opened.
fn link_sidecars(db_path: &Path, target: &Path) -> Result<()> {
    for suffix in SQLITE_SIDECARS
        .into_iter()
        .filter(|suffix| *suffix != "-shm")
    {
        let sidecar = fsutil::with_suffix(db_path, suffix);
        if !exists(&sidecar) {
            continue;
        }
        let kept = fsutil::with_suffix(target, suffix);
        fs::hard_link(&sidecar, &kept)
            .or_else(|error| {
                tracing::debug!(%error, sidecar = %sidecar.display(), "could not hard-link a sidecar of the damaged database; copying it");
                fs::copy(&sidecar, &kept).and_then(|_| fsutil::sync_file(&kept))
            })
            .map_err(CoreError::io(&sidecar))?;
    }
    Ok(())
}

/// Removes the sidecar names next to `path`, not `path` itself.
fn remove_sidecars(path: &Path) {
    for suffix in SQLITE_SIDECARS {
        let sidecar = fsutil::with_suffix(path, suffix);
        match fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(sidecar = %sidecar.display(), %error, "could not remove a sidecar left by a repair")
            }
        }
    }
}

/// What follows the move of the damaged database to `quarantined`, in a repair and when the next
/// open finishes one: forget the damage it was moved for, clear the database path of its sidecars,
/// and put the salvaged copy (if any) in its place.
fn finish(
    db_path: &Path,
    copy: Option<&Path>,
    quarantined: PathBuf,
    salvage: Salvage,
) -> Result<Recovery> {
    // The marker describes the file that just moved. Were it removed only after the install, a
    // crash in between would leave it next to the healthy copy, which the next open would then
    // repair all over again.
    clear_recorded_damage(db_path);
    let incomplete = |error| incomplete(db_path, &quarantined, error);
    settle_sidecars(db_path, &quarantined).map_err(incomplete)?;
    if let Some(copy) = copy {
        install(copy, db_path).map_err(incomplete)?;
    }
    Ok(Recovery {
        quarantined,
        salvage,
    })
}

/// Clears the database path of the moved database's sidecars: left there, SQLite would replay an
/// old `-wal` into the new file. One the moved file already has next to it ([`quarantine`]) is
/// removed; one it lacks (the `-shm`, or any sidecar of a repair stopped before it linked them) is
/// moved there. Only called after the move, or with a copy waiting, which proves the files at the
/// database path are the moved database's.
fn settle_sidecars(db_path: &Path, quarantined: &Path) -> Result<()> {
    for suffix in SQLITE_SIDECARS {
        let sidecar = fsutil::with_suffix(db_path, suffix);
        if !exists(&sidecar) {
            continue;
        }
        let kept = fsutil::with_suffix(quarantined, suffix);
        let settled = if exists(&kept) {
            fs::remove_file(&sidecar)
        } else {
            fs::rename(&sidecar, &kept)
        };
        settled.map_err(CoreError::io(&sidecar))?;
    }
    Ok(())
}

/// Renames a salvaged copy into place. Stale sidecars go first: SQLite can't tell which database a
/// `-wal` belongs to, and would replay an old one into the new file.
fn install(copy: &Path, db_path: &Path) -> Result<()> {
    for suffix in SQLITE_SIDECARS {
        let sidecar = fsutil::with_suffix(db_path, suffix);
        match fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(CoreError::Io {
                    path: sidecar,
                    source,
                });
            }
        }
    }
    fs::rename(copy, db_path).map_err(CoreError::io(db_path))?;
    fsutil::sync_dir(fsutil::parent_dir(db_path));
    Ok(())
}

/// The error for a failure after the damaged file was moved. It names where the file went, and
/// rephrases errors whose own message would claim the database "was left unchanged".
pub(super) fn incomplete(db_path: &Path, quarantined: &Path, error: CoreError) -> CoreError {
    let reason = match &error {
        CoreError::Migration(detail) => format!("upgrading it failed: {detail}"),
        CoreError::Backup { path, source } => format!(
            "backing it up to {} before upgrading it failed: {source}",
            path.display()
        ),
        other => other.to_string(),
    };
    CoreError::RecoveryIncomplete {
        path: db_path.to_path_buf(),
        quarantined: quarantined.to_path_buf(),
        reason,
        source: Box::new(error),
    }
}

/// Removes the record of the repair that moved the database to `quarantined` (as reported in
/// [`OpenOutcome::RecoveredFromCorruption`]), so opens stop reporting it. Only records of this
/// database in its own backups folder can match; the damaged copy itself is kept. Returns whether
/// a record was removed.
pub(super) fn acknowledge(db_path: &Path, backups_dir: &Path, quarantined: &str) -> Result<bool> {
    let Some(file) = record_files(db_path, backups_dir)
        .into_iter()
        .find(|file| file.quarantined.display().to_string() == quarantined)
    else {
        return Ok(false);
    };
    match fs::remove_file(&file.path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(CoreError::Io {
            path: file.path,
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::config::DbConfig;
    use rusqlite::{Connection, OpenFlags};

    struct Paths {
        _dir: tempfile::TempDir,
        db: PathBuf,
        backups: PathBuf,
    }

    fn paths() -> Paths {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("aura.db");
        let backups = dir.path().join("backups");
        fs::create_dir_all(&backups).expect("mkdir");
        Paths {
            _dir: dir,
            db,
            backups,
        }
    }

    fn write_record(p: &Paths, stem: &str, salvage: Salvage) -> PathBuf {
        let quarantined = p.backups.join(stem);
        fsutil::write_json_atomic(
            &fsutil::with_suffix(&quarantined, RECORD_SUFFIX),
            &RecoveryRecord {
                problem: "test".into(),
                salvage,
                recovered_at: 1,
            },
        )
        .expect("record");
        quarantined
    }

    /// A WAL database at `path` whose last row is only in its `-wal`, as after a crash; returns
    /// the `-wal`'s bytes.
    fn with_a_row_only_in_the_wal(path: &Path) -> Vec<u8> {
        let conn = Connection::open(path).expect("open");
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; CREATE TABLE t (x); INSERT INTO t VALUES ('checkpointed'); \
             PRAGMA wal_checkpoint(TRUNCATE); INSERT INTO t VALUES ('only in the wal');",
        )
        .expect("rows");
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .expect("keep the wal");
        drop(conn);
        let wal = fs::read(fsutil::with_suffix(path, "-wal")).expect("wal");
        assert!(!wal.is_empty());
        wal
    }

    fn rows_of(path: &Path) -> Vec<String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read-only open");
        let mut stmt = conn
            .prepare("SELECT x FROM t ORDER BY rowid")
            .expect("select");
        stmt.query_map([], |row| row.get(0))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect()
    }

    #[test]
    fn damage_records_keep_the_first_problem_and_clear() {
        let p = paths();
        assert_eq!(recorded_damage(&p.db), None);
        mark_damaged(&p.db, "Page 39: never used").expect("mark");
        mark_damaged(&p.db, "a later problem").expect("mark again");
        assert_eq!(
            recorded_damage(&p.db).as_deref(),
            Some("Page 39: never used")
        );
        clear_recorded_damage(&p.db);
        assert_eq!(recorded_damage(&p.db), None);
        clear_recorded_damage(&p.db);
    }

    #[test]
    fn an_unparsable_damage_record_still_counts_as_damage() {
        let p = paths();
        fs::write(damage_marker(&p.db), b"{ torn").expect("write");
        assert!(recorded_damage(&p.db).is_some());
    }

    #[test]
    fn records_sort_newest_first_and_ignore_other_files() {
        let p = paths();
        for stem in [
            "aura.db.corrupt-100",
            "aura.db.corrupt-300",
            "aura.db.corrupt-300.1",
            "aura.db.corrupt-200",
        ] {
            write_record(&p, stem, Salvage::Complete);
        }
        for other in [
            "other.db.corrupt-999.json",
            "aura.db.corrupt-abc.json",
            "aura.db.v1-5.bak",
            "notes.json",
        ] {
            fs::write(p.backups.join(other), b"{}").expect("write");
        }
        let names: Vec<String> = record_files(&p.db, &p.backups)
            .iter()
            .map(|file| fsutil::file_name_lossy(&file.quarantined))
            .collect();
        assert_eq!(
            names,
            [
                "aura.db.corrupt-300.1",
                "aura.db.corrupt-300",
                "aura.db.corrupt-200",
                "aura.db.corrupt-100"
            ]
        );
    }

    #[test]
    fn pending_needs_the_damaged_copy_to_still_exist() {
        let p = paths();
        let gone = write_record(&p, "aura.db.corrupt-200", Salvage::Complete);
        let kept = write_record(
            &p,
            "aura.db.corrupt-100",
            Salvage::Partial {
                tables: vec!["events".into()],
            },
        );
        fs::write(&kept, b"damaged bytes").expect("quarantined file");
        let pending = newest_pending(&p.db, &p.backups).expect("pending");
        assert_eq!(pending.quarantined, kept);
        assert!(!gone.exists());
    }

    /// Finding: a record that couldn't be parsed was skipped, so its repair stopped being reported.
    #[test]
    fn a_record_this_build_cant_read_is_still_reported() {
        for body in [
            &b"{ torn"[..],
            br#"{"problem": "x", "salvage": {"type": "from_a_newer_build"}, "recovered_at": 1}"#,
        ] {
            let p = paths();
            let quarantined = p.backups.join("aura.db.corrupt-100");
            fs::write(&quarantined, b"damaged bytes").expect("quarantined file");
            fs::write(fsutil::with_suffix(&quarantined, RECORD_SUFFIX), body).expect("record");
            assert_eq!(
                newest_pending(&p.db, &p.backups),
                Some(Recovery {
                    quarantined: quarantined.clone(),
                    salvage: unknown_salvage(),
                })
            );
            assert!(
                acknowledge(&p.db, &p.backups, &quarantined.display().to_string()).expect("ack")
            );
            assert_eq!(newest_pending(&p.db, &p.backups), None);
        }
    }

    #[test]
    fn salvage_copies_are_named_after_their_repair() {
        let p = paths();
        let quarantined = p.backups.join("aura.db.corrupt-1730000000000.2");
        let copy = salvage_path(&p.db, &quarantined);
        assert_eq!(
            copy,
            p.db.with_file_name("aura.db.salvage-1730000000000.2.partial")
        );
        assert_eq!(quarantined_for(&copy, &p.db, &p.backups), Some(quarantined));
        assert_eq!(
            quarantined_for(&p.db.with_file_name("notes.partial"), &p.db, &p.backups),
            None
        );
    }

    #[test]
    fn resume_drops_records_whose_copy_is_gone_and_stale_salvage_copies() {
        let p = paths();
        let stale = write_record(&p, "aura.db.corrupt-5", Salvage::Complete);
        fs::write(&p.db, b"the database is in place").expect("db");
        let copy = salvage_path(&p.db, &stale);
        fs::write(&copy, b"half a copy").expect("stale copy");
        fs::write(fsutil::with_suffix(&copy, "-journal"), b"journal").expect("stale journal");
        let orphan = salvage_path(&p.db, &p.backups.join("aura.db.corrupt-9"));
        fs::write(&orphan, b"from a repair interrupted mid-copy").expect("orphan");
        let scratch = p.db.with_file_name("aura.db.salvage-9.schema.partial");
        fs::write(
            &scratch,
            b"the schema copy of a repair interrupted mid-copy",
        )
        .expect("scratch");
        // Sidecar names a repair gave the file before it stopped short of moving it.
        fs::write(
            fsutil::with_suffix(&stale, "-wal"),
            b"a link to the live -wal",
        )
        .expect("link");
        let unrelated = p.db.with_file_name("notes.salvage-1.partial");
        fs::write(&unrelated, b"not ours").expect("unrelated");

        assert_eq!(resume(&p.db, &p.backups).expect("resume"), None);
        assert!(!fsutil::with_suffix(&stale, RECORD_SUFFIX).exists());
        assert!(!fsutil::with_suffix(&stale, "-wal").exists());
        assert!(!copy.exists());
        assert!(!fsutil::with_suffix(&copy, "-journal").exists());
        assert!(!orphan.exists());
        assert!(!scratch.exists());
        assert!(unrelated.exists());
        assert_eq!(fs::read(&p.db).expect("db"), b"the database is in place");
    }

    #[test]
    fn resume_installs_a_copy_the_last_repair_left_waiting() {
        let p = paths();
        let quarantined = write_record(&p, "aura.db.corrupt-7", Salvage::Complete);
        fs::write(&quarantined, b"damaged").expect("quarantined");
        let copy = salvage_path(&p.db, &quarantined);
        fs::write(&copy, b"salvaged database").expect("copy");
        fs::write(fsutil::with_suffix(&p.db, "-shm"), b"stale shm").expect("stale shm");
        mark_damaged(&p.db, "found earlier").expect("mark");

        let finished = resume(&p.db, &p.backups)
            .expect("resume")
            .expect("finished the repair");
        assert_eq!(finished.quarantined, quarantined);
        assert_eq!(finished.salvage, Salvage::Complete);
        assert_eq!(fs::read(&p.db).expect("installed"), b"salvaged database");
        assert!(!copy.exists());
        assert!(
            !fsutil::with_suffix(&p.db, "-shm").exists(),
            "a stale sidecar must never meet the new file"
        );
        assert_eq!(recorded_damage(&p.db), None);
        // Still reported until acknowledged.
        assert_eq!(newest_pending(&p.db, &p.backups), Some(finished));
    }

    /// With the clock set back between two repairs, the repair whose copy is waiting can have the
    /// older timestamp. Its copy is found by name, not by assuming the newest record owns it.
    #[test]
    fn resume_installs_the_waiting_copy_even_when_it_is_not_the_newest_record() {
        let p = paths();
        let earlier_clock = write_record(&p, "aura.db.corrupt-100", Salvage::Complete);
        let later_clock = write_record(&p, "aura.db.corrupt-900", Salvage::Nothing);
        for quarantined in [&earlier_clock, &later_clock] {
            fs::write(quarantined, b"damaged").expect("quarantined");
        }
        fs::write(salvage_path(&p.db, &earlier_clock), b"the waiting copy").expect("copy");
        let finished = resume(&p.db, &p.backups)
            .expect("resume")
            .expect("finished");
        assert_eq!(finished.quarantined, earlier_clock);
        assert_eq!(fs::read(&p.db).expect("installed"), b"the waiting copy");
    }

    /// Finding: a waiting copy was installed only when its record could be read; a torn record, or
    /// one from a newer build with a salvage this one doesn't know, got the copy deleted and the
    /// open reported a plain Created over an empty database.
    #[test]
    fn a_waiting_copy_is_installed_even_when_its_record_cant_be_read() {
        for body in [
            Some(&b"{ torn"[..]),
            Some(br#"{"problem": "x", "salvage": {"type": "from_a_newer_build"}, "recovered_at": 1}"#),
            None,
        ] {
            let p = paths();
            let quarantined = p.backups.join("aura.db.corrupt-1000");
            fs::write(&quarantined, b"damaged").expect("quarantined");
            if let Some(body) = body {
                fs::write(fsutil::with_suffix(&quarantined, RECORD_SUFFIX), body).expect("record");
            }
            let copy = salvage_path(&p.db, &quarantined);
            fs::write(&copy, b"salvaged database").expect("copy");

            let finished = resume(&p.db, &p.backups)
                .expect("resume")
                .expect("finished the repair");
            assert_eq!(
                finished,
                Recovery {
                    quarantined: quarantined.clone(),
                    salvage: unknown_salvage(),
                },
                "{body:?}"
            );
            assert_eq!(fs::read(&p.db).expect("installed"), b"salvaged database");
            assert!(!copy.exists());
        }
    }

    #[test]
    fn a_salvaged_copy_is_never_deleted_while_its_damaged_database_is_kept() {
        let p = paths();
        let quarantined = write_record(&p, "aura.db.corrupt-7", Salvage::Complete);
        fs::write(&quarantined, b"damaged").expect("quarantined");
        let copy = salvage_path(&p.db, &quarantined);
        fs::write(&copy, b"salvaged database").expect("copy");
        // Something put a database back in place by hand meanwhile.
        fs::write(&p.db, b"a database put back by hand").expect("db");
        assert_eq!(resume(&p.db, &p.backups).expect("resume"), None);
        assert_eq!(fs::read(&copy).expect("kept"), b"salvaged database");
        assert_eq!(fs::read(&p.db).expect("db"), b"a database put back by hand");
    }

    #[test]
    fn a_repair_that_salvaged_nothing_leaves_no_copy_to_install() {
        let p = paths();
        let quarantined = write_record(&p, "aura.db.corrupt-7", Salvage::Nothing);
        fs::write(&quarantined, b"damaged").expect("quarantined");
        assert_eq!(resume(&p.db, &p.backups).expect("resume"), None);
        assert!(!p.db.exists(), "the open creates the new database");
        assert_eq!(
            newest_pending(&p.db, &p.backups).map(|pending| pending.salvage),
            Some(Salvage::Nothing),
            "and the repair is still reported"
        );
    }

    #[test]
    fn a_database_removed_by_hand_after_a_finished_repair_is_not_resurrected() {
        let p = paths();
        let quarantined = write_record(&p, "aura.db.corrupt-7", Salvage::Complete);
        fs::write(&quarantined, b"damaged").expect("quarantined");
        assert_eq!(resume(&p.db, &p.backups).expect("resume"), None);
        assert!(!p.db.exists());
        assert!(newest_pending(&p.db, &p.backups).is_some());
    }

    /// Finding: the move took the damaged file first and its `-wal` afterwards, so a repair that
    /// stopped in between left the `-wal` at the database path, where finishing the repair deleted
    /// it to install the copy: every transaction not yet in the main file was gone.
    #[test]
    fn finishing_a_repair_keeps_the_wal_the_move_left_behind() {
        let p = paths();
        let wal = with_a_row_only_in_the_wal(&p.db);
        // As a repair left it when it stopped right after moving the main file: record written,
        // copy waiting, the -wal (and -shm) still at the database path.
        let quarantined = write_record(&p, "aura.db.corrupt-1000", Salvage::Complete);
        fs::rename(&p.db, &quarantined).expect("move the main file");
        fs::write(salvage_path(&p.db, &quarantined), b"salvaged database").expect("copy");

        resume(&p.db, &p.backups)
            .expect("resume")
            .expect("finished the repair");
        assert_eq!(
            fs::read(fsutil::with_suffix(&quarantined, "-wal")).expect("the -wal moved along"),
            wal
        );
        assert!(!fsutil::with_suffix(&p.db, "-wal").exists());
        assert_eq!(
            rows_of(&quarantined),
            ["checkpointed", "only in the wal"],
            "the moved file still has the row that was only in its -wal"
        );
        assert_eq!(fs::read(&p.db).expect("installed"), b"salvaged database");
    }

    /// The move gives each sidecar its name next to the moved file before the file itself moves,
    /// so stopping right after it loses nothing.
    #[test]
    fn quarantine_gives_the_sidecars_their_names_before_the_move() {
        let p = paths();
        let wal = with_a_row_only_in_the_wal(&p.db);
        let target = p.backups.join("aura.db.corrupt-1000");
        quarantine(&p.db, &target).expect("move");
        assert!(!p.db.exists());
        assert_eq!(
            fs::read(fsutil::with_suffix(&target, "-wal")).expect("kept"),
            wal
        );
        assert!(
            fsutil::with_suffix(&p.db, "-wal").exists(),
            "the old name goes only once the move is done"
        );
        assert!(
            !fsutil::with_suffix(&target, "-shm").exists(),
            "shared memory never gets a second name"
        );
        assert_eq!(rows_of(&target), ["checkpointed", "only in the wal"]);
    }

    #[test]
    fn a_move_that_fails_changes_nothing() {
        let p = paths();
        let wal = with_a_row_only_in_the_wal(&p.db);
        let before = fs::read(&p.db).expect("db");
        // A folder with something in it where the database should go.
        let target = p.backups.join("aura.db.corrupt-1000");
        fs::create_dir_all(target.join("in the way")).expect("mkdir");
        assert!(matches!(
            quarantine(&p.db, &target),
            Err(CoreError::Io { .. })
        ));
        assert_eq!(fs::read(&p.db).expect("db"), before);
        assert_eq!(
            fs::read(fsutil::with_suffix(&p.db, "-wal")).expect("wal"),
            wal
        );
        for suffix in SQLITE_SIDECARS {
            assert!(
                !fsutil::with_suffix(&target, suffix).exists(),
                "{suffix} left next to the target"
            );
        }
    }

    /// Finding: the damage marker went only after the copy was installed, so a crash in between
    /// left it next to the healthy copy, and the next open repaired that copy again (another
    /// quarantined copy, and a second notice in place of the first repair's).
    #[test]
    fn the_damage_marker_goes_before_the_copy_is_installed() {
        let p = paths();
        let events_lost = Salvage::Partial {
            tables: vec!["events".into()],
        };
        let quarantined = write_record(&p, "aura.db.corrupt-7", events_lost.clone());
        fs::write(&quarantined, b"damaged").expect("quarantined");
        let copy = salvage_path(&p.db, &quarantined);
        fs::write(&copy, b"salvaged database").expect("copy");
        mark_damaged(&p.db, "found by a check").expect("mark");
        // Something in the way of the install: a folder where the database goes.
        fs::create_dir_all(p.db.join("in the way")).expect("mkdir");

        let error = resume(&p.db, &p.backups).expect_err("the install fails");
        assert!(
            matches!(error, CoreError::RecoveryIncomplete { .. }),
            "{error:?}"
        );
        assert_eq!(
            recorded_damage(&p.db),
            None,
            "cleared before the install was tried"
        );
        assert!(copy.exists(), "the copy still waits");

        fs::remove_dir_all(&p.db).expect("clear the way");
        let finished = resume(&p.db, &p.backups)
            .expect("resume")
            .expect("finished");
        assert_eq!(finished.salvage, events_lost);
        assert_eq!(fs::read(&p.db).expect("installed"), b"salvaged database");
    }

    #[test]
    fn acknowledging_removes_only_the_matching_record() {
        let p = paths();
        let first = write_record(&p, "aura.db.corrupt-1", Salvage::Complete);
        let second = write_record(&p, "aura.db.corrupt-2", Salvage::Nothing);
        for quarantined in [&first, &second] {
            fs::write(quarantined, b"damaged").expect("quarantined");
        }
        assert!(!acknowledge(&p.db, &p.backups, "/somewhere/else").expect("ack"));
        assert!(
            !acknowledge(&p.db, &p.backups, "aura.db.corrupt-2").expect("ack"),
            "a bare name is not the path the outcome reported"
        );
        assert!(acknowledge(&p.db, &p.backups, &second.display().to_string()).expect("ack"));
        assert!(second.exists(), "the damaged copy itself is kept");
        assert_eq!(
            newest_pending(&p.db, &p.backups).map(|pending| pending.quarantined),
            Some(first.clone())
        );
        assert!(!acknowledge(&p.db, &p.backups, &second.display().to_string()).expect("again"));
    }

    #[test]
    fn incomplete_errors_name_the_quarantine_and_drop_left_unchanged() {
        let error = incomplete(
            Path::new("/p/aura.db"),
            Path::new("/p/backups/aura.db.corrupt-1"),
            CoreError::Migration("version 1 to 2: no such table: x".into()),
        );
        let text = error.to_string();
        assert!(text.contains("/p/backups/aura.db.corrupt-1"), "{text}");
        assert!(text.contains("no such table: x"), "{text}");
        assert!(!text.contains("left unchanged"), "{text}");
    }
}
