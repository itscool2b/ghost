//! Copying whatever can still be read out of a damaged database into a new one.
//!
//! First `VACUUM INTO`: it rebuilds every table and index from what their b-trees hold, so unused
//! ("leaked") pages and broken free lists disappear, and it keeps `user_version`. It gives up at the
//! first page it can't read, though. Then the new database gets Ghost's own schema for the
//! damaged file's `user_version`, from the migrations, so every table of that version exists even
//! when its entry in the damaged schema is lost, and the tables are copied into it one by one, row
//! by row: a read error skips ahead to the next readable rowid instead of abandoning the table.
//! `sqlite_schema` is read the same way, and the tables whose entries come after an unreadable part
//! of it are read through a copy of the file whose schema leaves that part out
//! ([`rebuild_schema`]). References left dangling by a lost parent row are settled at the end
//! ([`settle_orphans`]), so the copy passes the foreign key check every later migration runs.
//!
//! The damaged file is only ever read (read-only connection, no checkpoint on close), so it can be
//! quarantined byte for byte afterwards.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rusqlite::config::DbConfig;
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, ErrorCode, OpenFlags, Statement};
use rusqlite_migration::{M, Migrations};

use super::{
    BUSY_TIMEOUT, Salvage, close_without_checkpoint, describe_migration_error, remove_with_sidecars,
};
use crate::error::{CoreError, Result};
use crate::fsutil::{self, PARTIAL_SUFFIX};

/// Passes of the orphan cleanup. Each pass settles the rows whose parent the previous pass dropped,
/// so this bounds the depth of foreign key chains it untangles (the schema's deepest is 4).
const MAX_ORPHAN_PASSES: usize = 16;

/// Copies everything readable from the database at `damaged` into a new database at `out` and says
/// how much made it. `migrations` are the store's: when the damaged file can't be copied whole, the
/// copy's schema comes from them. Errors only mean `out` could not be written (disk full, no
/// permission): the caller then leaves everything as it was. Nothing is left at `out` when nothing
/// could be read.
pub(super) fn salvage(damaged: &Path, out: &Path, migrations: &[M<'_>]) -> Result<Salvage> {
    let scratch = scratch_path(out);
    remove_with_sidecars(out);
    remove_with_sidecars(&scratch);
    let source = open_source(damaged)?;
    let copied = match vacuum_into(&source, out) {
        Ok(()) => Ok(Salvage::Complete),
        Err(error) => {
            tracing::info!(%error, "VACUUM INTO could not copy the damaged database; copying it table by table");
            // A failed VACUUM INTO leaves an empty or partial file behind.
            remove_with_sidecars(out);
            copy_tables(&source, out, &scratch, migrations)
        }
    };
    close_without_checkpoint(source);
    remove_with_sidecars(&scratch);
    let salvage = match copied {
        Ok(salvage) => salvage,
        Err(error) => {
            remove_with_sidecars(out);
            return Err(error);
        }
    };
    if salvage == Salvage::Nothing {
        remove_with_sidecars(out);
    } else if let Err(source) = fsutil::sync_file(out) {
        // SQLite doesn't sync a VACUUM INTO target, and the copy below ran with synchronous=OFF.
        remove_with_sidecars(out);
        return Err(CoreError::Io {
            path: out.to_path_buf(),
            source,
        });
    }
    Ok(salvage)
}

/// Where [`rebuild_schema`] puts its copy of the damaged file: next to `out` and named after it
/// (`<db>.salvage-<millis>.schema.partial`), so the next open removes it if a repair stops midway.
fn scratch_path(out: &Path) -> PathBuf {
    let name = fsutil::file_name_lossy(out);
    match name.strip_suffix(PARTIAL_SUFFIX) {
        Some(stem) => out.with_file_name(format!("{stem}.schema{PARTIAL_SUFFIX}")),
        None => fsutil::with_suffix(out, ".schema"),
    }
}

/// Read-only, so salvaging never writes to the damaged file, and no checkpoint on close, so its
/// `-wal` stays as it was. The path is used as is (no URI parsing).
fn open_source(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

fn vacuum_into(source: &Connection, out: &Path) -> rusqlite::Result<()> {
    // Absolute, so a relative name can never be taken for a "file:" URI.
    let invalid = || rusqlite::Error::InvalidPath(out.to_path_buf());
    let target = std::path::absolute(out).map_err(|_| invalid())?;
    let target = target.to_str().ok_or_else(invalid)?;
    source.execute("VACUUM INTO ?1", [target]).map(|_| ())
}

/// Errors that mean the new database can't be written (disk full, no permission, I/O), which stop
/// the salvage. Anything else, such as a statement the new database rejects, costs only the table,
/// index, or row it was about.
fn is_write_failure(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(
            ErrorCode::DiskFull
                | ErrorCode::SystemIoFailure
                | ErrorCode::CannotOpen
                | ErrorCode::ReadOnly
                | ErrorCode::PermissionDenied
                | ErrorCode::OutOfMemory
                | ErrorCode::NoLargeFileSupport
                | ErrorCode::DatabaseBusy
                | ErrorCode::DatabaseLocked
                | ErrorCode::DatabaseCorrupt
                | ErrorCode::NotADatabase
        )
    )
}

/// One row of `sqlite_schema`.
struct SchemaEntry {
    rowid: i64,
    kind: String,
    name: String,
    tbl_name: String,
    rootpage: Option<i64>,
    /// `None` for the indexes behind PRIMARY KEY and UNIQUE constraints.
    sql: Option<String>,
}

/// What could be read of `sqlite_schema`.
struct Schema {
    /// In rowid order, which is the order things were created in.
    entries: Vec<SchemaEntry>,
    /// Part of it could not be read, so it may have listed tables nothing else knows of (the
    /// migrations only know Ghost's own).
    damaged: bool,
    /// Entries were read past an unreadable part. SQLite loads the schema in rowid order and stops
    /// at the first page it can't read, so it knows none of them: their tables can only be read
    /// through [`rebuild_schema`].
    hidden: bool,
}

/// What `PRAGMA table_list` says about a table.
struct TableKind {
    /// `table`, `virtual`, or `shadow` (a virtual table's storage).
    kind: String,
    without_rowid: bool,
}

/// Rows of a table copied, and whether any could not be read or written.
struct TableCopy {
    rows: u64,
    complete: bool,
}

fn copy_tables(
    source: &Connection,
    out: &Path,
    scratch: &Path,
    migrations: &[M<'_>],
) -> Result<Salvage> {
    relax_schema_parsing(source);
    let Ok(version) = source.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
    else {
        // The header itself is unreadable, and with it everything else.
        return Ok(Salvage::Nothing);
    };
    let schema = read_schema(source);
    let rebuilt = if schema.hidden {
        rebuild_schema(source, scratch, &schema.entries)
    } else {
        None
    };
    let copied = copy_into(
        rebuilt.as_ref().unwrap_or(source),
        &schema,
        out,
        version,
        migrations,
    );
    if let Some(conn) = rebuilt {
        close_without_checkpoint(conn);
    }
    copied
}

/// With writable_schema on, SQLite skips schema entries it can't parse instead of failing every
/// statement, so one malformed CREATE doesn't hide the other tables. The statement that loads the
/// schema still fails when something in it is bad (the ones after it run against what did load),
/// so one is prepared here and its error set aside.
fn relax_schema_parsing(conn: &Connection) {
    if let Err(error) = conn.pragma_update(None, "writable_schema", true) {
        tracing::debug!(%error, "could not relax schema parsing for the salvage");
    }
    if let Err(error) = conn.prepare("SELECT 1 FROM sqlite_schema LIMIT 0") {
        tracing::debug!(%error, "the damaged database's schema did not load cleanly");
    }
}

/// Reads `sqlite_schema` as it would any rowid table ([`scan_by_rowid`]), so an unreadable page
/// costs only the entries on it.
fn read_schema(source: &Connection) -> Schema {
    let mut entries = Vec::new();
    let mut malformed = false;
    let scanned = scan_by_rowid(
        source,
        "sqlite_schema",
        "type, name, tbl_name, rootpage, sql",
        &mut |row| {
            match schema_entry(row) {
                Ok(entry) => entries.push(entry),
                // Values of the wrong type: as good as unreadable.
                Err(_) => malformed = true,
            }
            Ok(())
        },
    );
    // Only the closure above could fail the scan, and it never does.
    let gap = scanned.unwrap_or(Some(i64::MIN));
    let hidden = gap.is_some_and(|at| entries.iter().any(|entry| entry.rowid >= at));
    let damaged = gap.is_some() || malformed;
    if damaged {
        tracing::warn!(
            readable = entries.len(),
            hidden,
            "part of the damaged database's schema could not be read"
        );
    }
    Schema {
        entries,
        damaged,
        hidden,
    }
}

fn schema_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<SchemaEntry> {
    Ok(SchemaEntry {
        rowid: row.get(0)?,
        kind: row.get(1)?,
        name: row.get(2)?,
        tbl_name: row.get(3)?,
        rootpage: row.get(4)?,
        sql: row.get(5)?,
    })
}

/// A copy of the damaged file whose schema holds just `entries`, opened for reading, so SQLite
/// knows the tables listed after an unreadable part of the schema again ([`Schema::hidden`]). The
/// copy is page for page the same (the backup API copies pages without parsing them, taking the
/// newest from the `-wal`), except that page 1, where the schema starts, is made an empty leaf and
/// filled with `entries` again: every table still points at its own pages. `None` when the copy
/// can't be made (no room for it, say); the tables listed after the damage are then lost.
fn rebuild_schema(
    source: &Connection,
    scratch: &Path,
    entries: &[SchemaEntry],
) -> Option<Connection> {
    let rebuilt =
        write_rebuilt_schema(source, scratch, entries).and_then(|()| open_source(scratch));
    match rebuilt {
        Ok(conn) => {
            relax_schema_parsing(&conn);
            Some(conn)
        }
        Err(error) => {
            tracing::warn!(%error, "could not rebuild the damaged database's schema; the tables listed after the damage are lost");
            remove_with_sidecars(scratch);
            None
        }
    }
}

fn write_rebuilt_schema(
    source: &Connection,
    scratch: &Path,
    entries: &[SchemaEntry],
) -> Result<()> {
    source.backup(rusqlite::MAIN_DB, scratch, None)?;
    reset_schema_root(scratch).map_err(CoreError::io(scratch))?;
    let mut conn = Connection::open(scratch)?;
    conn.pragma_update(None, "writable_schema", true)?;
    // A scratch file, removed once the salvage is done.
    conn.pragma_update(None, "synchronous", "OFF")?;
    let tx = conn.transaction()?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO sqlite_schema (type, name, tbl_name, rootpage, sql) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for entry in entries {
            insert.execute(rusqlite::params![
                entry.kind,
                entry.name,
                entry.tbl_name,
                entry.rootpage,
                entry.sql
            ])?;
        }
    }
    tx.commit()?;
    conn.close().map_err(|(_, error)| error)?;
    Ok(())
}

/// Makes page 1 of the copy at `path` an empty table leaf: a schema with no entries. The free list
/// is emptied too, since its pages may be damaged, so filling the schema in again only ever appends
/// pages; and the copy is marked as using a rollback journal, so it never needs a `-wal`. The file
/// header and the page header after it are laid out in <https://www.sqlite.org/fileformat2.html>.
fn reset_schema_root(path: &Path) -> io::Result<()> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut head = [0u8; 108];
    file.read_exact(&mut head)?;
    let page_size = match u16::from_be_bytes([head[16], head[17]]) {
        1 => 65_536,
        size => u32::from(size),
    };
    // Byte 20 is the space reserved at the end of every page.
    let usable = page_size.saturating_sub(u32::from(head[20]));
    // Bytes 18 and 19: the file format versions, 1 for a rollback journal.
    head[18] = 1;
    head[19] = 1;
    // Bytes 32 to 39: the first free-list trunk page and the number of free pages.
    head[32..40].fill(0);
    // The page header that follows on page 1: a table leaf (13) with no free blocks, no cells,
    // and its cell content area starting at the end of the usable space (0 stands for 65536).
    head[100] = 13;
    head[101..105].fill(0);
    head[105..107].copy_from_slice(&u16::try_from(usable).unwrap_or(0).to_be_bytes());
    head[107] = 0;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&head)
}

/// Builds the new database at `out` from what `src` can read: Ghost's schema for `version`, then
/// every table's rows, then whatever else the damaged schema had.
fn copy_into(
    src: &Connection,
    schema: &Schema,
    out: &Path,
    version: i64,
    migrations: &[M<'_>],
) -> Result<Salvage> {
    let kinds = table_kinds(src);
    let tables: Vec<&SchemaEntry> = schema
        .entries
        .iter()
        .filter(|entry| is_data_table(entry, &kinds, &schema.entries))
        .collect();
    if tables.is_empty() {
        return Ok(Salvage::Nothing);
    }

    let mut conn = Connection::open(out)?;
    // The bundled SQLite turns foreign keys on by default; off, rows can arrive in any order and
    // dangling references are settled at the end.
    conn.pragma_update(None, "foreign_keys", false)?;
    // A scratch file until it is complete; `salvage` syncs it once at the end.
    conn.pragma_update(None, "synchronous", "OFF")?;
    let migrated = create_own_schema(&mut conn, migrations, version)?;
    let own = if migrated {
        own_tables(&conn)?
    } else {
        Vec::new()
    };
    let tx = conn.transaction()?;
    let triggers = take_triggers(&tx)?;
    let mut rows = 0u64;
    // Every table of the new database in schema order, and the ones that came out incomplete.
    let mut order: Vec<&str> = Vec::new();
    let mut lost: Vec<&str> = Vec::new();
    for name in &own {
        order.push(name);
        let Some(table) = tables
            .iter()
            .find(|table| table.name.eq_ignore_ascii_case(name))
        else {
            // Its entry was on an unreadable part of the schema (or the table was missing from
            // the damaged file altogether): it starts out empty.
            tracing::warn!(table = %name, "a table of the damaged database could not be found");
            lost.push(name);
            continue;
        };
        // Rows a migration seeded (a default setting, say) give way to the damaged file's, which
        // may have changed or deleted them since.
        tx.execute_batch(&format!("DELETE FROM {}", quote(name)))?;
        let copy = copy_rows(src, &tx, table, &kinds)?;
        rows += copy.rows;
        if !copy.complete {
            lost.push(name);
        }
    }
    for table in tables.iter().filter(|table| {
        !own.iter()
            .any(|name| name.eq_ignore_ascii_case(&table.name))
    }) {
        if is_shadow_in(&tx, &table.name)? {
            // The storage of one of Ghost's virtual tables, taken for a table of its own
            // because the virtual table's entry was lost: the migrations made it again, and it
            // fills through the virtual table.
            continue;
        }
        // Not one of Ghost's tables (a user's own, or any table of a version the migrations
        // don't know): created as the damaged file had it.
        order.push(&table.name);
        if let Err(error) = tx.execute_batch(table.sql.as_deref().unwrap_or_default()) {
            if is_write_failure(&error) {
                return Err(error.into());
            }
            tracing::warn!(%error, table = %table.name, "could not recreate a table from the damaged database");
            lost.push(&table.name);
            continue;
        }
        let copy = copy_rows(src, &tx, table, &kinds)?;
        rows += copy.rows;
        if !copy.complete {
            lost.push(&table.name);
        }
    }
    let settled = settle_orphans(&tx)?;
    for sql in &triggers {
        tx.execute_batch(sql)?;
    }
    replay_other_objects(&tx, &schema.entries)?;
    copy_sequences(src, &tx)?;
    if !migrated {
        tx.pragma_update(None, "user_version", version)?;
    }
    tx.commit()?;
    conn.close().map_err(|(_, error)| error)?;

    if rows == 0 && !lost.is_empty() {
        return Ok(Salvage::Nothing);
    }
    let incomplete: Vec<String> = order
        .into_iter()
        .filter(|name| lost.contains(name) || settled.iter().any(|table| table.as_str() == *name))
        .map(str::to_owned)
        .collect();
    Ok(if !incomplete.is_empty() {
        Salvage::Partial { tables: incomplete }
    } else if schema.damaged {
        // Whatever the unreadable part of the schema listed is unknown, so the copy can't be
        // called complete.
        Salvage::Partial { tables: Vec::new() }
    } else {
        Salvage::Complete
    })
}

/// A table whose rows are copied: not SQLite's own, and not a virtual table's storage.
fn is_data_table(
    entry: &SchemaEntry,
    kinds: &HashMap<String, TableKind>,
    entries: &[SchemaEntry],
) -> bool {
    entry.kind == "table"
        && entry.sql.is_some()
        && !entry.name.starts_with("sqlite_")
        && !is_shadow(&entry.name, kinds, entries)
}

/// Gives the new database Ghost's schema at `version`, from the migrations, so every table,
/// index, and virtual table of that version exists even when the damaged schema lost its entry.
/// False for a version the migrations don't take a database to (0, or one above them, which the
/// open never lets through): the damaged file's own schema is replayed instead, as it is for any
/// table that isn't Ghost's.
fn create_own_schema(out: &mut Connection, migrations: &[M<'_>], version: i64) -> Result<bool> {
    let Ok(version) = usize::try_from(version) else {
        return Ok(false);
    };
    if version == 0 || version > migrations.len() {
        return Ok(false);
    }
    match Migrations::from_slice(migrations).to_version(out, version) {
        Ok(()) => Ok(true),
        Err(rusqlite_migration::Error::RusqliteError { err, .. }) => Err(err.into()),
        Err(error) => Err(CoreError::Migration(format!(
            "rebuilding version {version} for the repaired copy: {}",
            describe_migration_error(&error)
        ))),
    }
}

/// The tables the migrations created, in creation order, without SQLite's own and the shadow
/// tables a virtual table keeps its data in (it fills those itself).
fn own_tables(out: &Connection) -> Result<Vec<String>> {
    let mut stmt = out.prepare(
        "SELECT s.name FROM sqlite_schema AS s \
         JOIN pragma_table_list AS t ON t.schema = 'main' AND t.name = s.name \
         WHERE s.type = 'table' AND t.type IN ('table', 'virtual') ORDER BY s.rowid",
    )?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(names
        .into_iter()
        .filter(|name| !name.starts_with("sqlite_"))
        .collect())
}

/// Drops the triggers the migrations created and returns their SQL, to run again once the rows
/// are in: none may fire on a copied row (a trigger that indexed messages for search would index
/// every copied message a second time).
fn take_triggers(out: &Connection) -> Result<Vec<String>> {
    let triggers: Vec<(String, String)> = {
        let mut stmt = out.prepare(
            "SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' AND sql IS NOT NULL ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (name, _) in &triggers {
        out.execute_batch(&format!("DROP TRIGGER {}", quote(name)))?;
    }
    Ok(triggers.into_iter().map(|(_, sql)| sql).collect())
}

/// The damaged file's indexes, views, and triggers that the new database doesn't have (a user's
/// own, or all of them for a version the migrations don't know), created after the rows so an
/// index is built in one pass and no trigger fires on a copied row. One the new database rejects
/// (its table was lost, say) is skipped.
fn replay_other_objects(out: &Connection, entries: &[SchemaEntry]) -> Result<()> {
    for kind in ["index", "view", "trigger"] {
        for entry in entries.iter().filter(|entry| entry.kind == kind) {
            let Some(sql) = entry.sql.as_deref() else {
                continue;
            };
            if exists_in(out, &entry.name)? {
                continue;
            }
            if let Err(error) = out.execute_batch(sql) {
                if is_write_failure(&error) {
                    return Err(error.into());
                }
                tracing::warn!(%error, name = %entry.name, kind, "could not recreate a schema object from the damaged database");
            }
        }
    }
    Ok(())
}

/// Whether the new database already has a schema object called `name` (names are compared the
/// way SQLite does, ignoring ASCII case).
fn exists_in(out: &Connection, name: &str) -> Result<bool> {
    let found: i64 = out.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name = ?1 COLLATE NOCASE",
        [name],
        |row| row.get(0),
    )?;
    Ok(found > 0)
}

/// Whether `name` is the storage of a virtual table in the new database.
fn is_shadow_in(out: &Connection, name: &str) -> Result<bool> {
    let found: i64 = out.query_row(
        "SELECT count(*) FROM pragma_table_list \
         WHERE schema = 'main' AND type = 'shadow' AND name = ?1 COLLATE NOCASE",
        [name],
        |row| row.get(0),
    )?;
    Ok(found > 0)
}

/// `PRAGMA table_list` for the main schema; empty if it can't be read.
fn table_kinds(source: &Connection) -> HashMap<String, TableKind> {
    let mut kinds = HashMap::new();
    let Ok(mut stmt) =
        source.prepare("SELECT name, type, wr FROM pragma_table_list WHERE schema = 'main'")
    else {
        return kinds;
    };
    let Ok(mut rows) = stmt.query([]) else {
        return kinds;
    };
    while let Ok(Some(row)) = rows.next() {
        if let (Ok(name), Ok(kind), Ok(wr)) = (
            row.get::<_, String>(0),
            row.get::<_, String>(1),
            row.get::<_, i64>(2),
        ) {
            kinds.insert(
                name,
                TableKind {
                    kind,
                    without_rowid: wr != 0,
                },
            );
        }
    }
    kinds
}

/// Shadow tables hold a virtual table's data and are recreated (and filled) through it.
fn is_shadow(name: &str, kinds: &HashMap<String, TableKind>, entries: &[SchemaEntry]) -> bool {
    match kinds.get(name) {
        Some(kind) => kind.kind == "shadow",
        // table_list leaves out what it can't parse. SQLite names shadow tables `<vtab>_<suffix>`.
        None => entries.iter().any(|entry| {
            entry.kind == "table"
                && is_virtual(entry, kinds)
                && name
                    .strip_prefix(entry.name.as_str())
                    .is_some_and(|rest| rest.starts_with('_'))
        }),
    }
}

fn is_virtual(entry: &SchemaEntry, kinds: &HashMap<String, TableKind>) -> bool {
    match kinds.get(&entry.name) {
        Some(kind) => kind.kind == "virtual",
        None => entry.sql.as_deref().is_some_and(|sql| {
            sql.trim_start()
                .get(.."CREATE VIRTUAL TABLE".len())
                .is_some_and(|start| start.eq_ignore_ascii_case("CREATE VIRTUAL TABLE"))
        }),
    }
}

/// `"name"`, with embedded quotes doubled.
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The columns a copy reads and writes: everything but hidden columns (a virtual table's own
/// columns) and generated ones (which can't be inserted). `None` when the table's definition can't
/// be read.
fn columns_of(source: &Connection, table: &str) -> Option<Vec<String>> {
    let mut stmt = source
        .prepare("SELECT name, hidden FROM pragma_table_xinfo(?1)")
        .ok()?;
    let mut rows = stmt.query([table]).ok()?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().ok()? {
        let name: String = row.get(0).ok()?;
        let hidden: i64 = row.get(1).ok()?;
        if hidden == 0 {
            columns.push(name);
        }
    }
    (!columns.is_empty()).then_some(columns)
}

fn copy_rows(
    source: &Connection,
    out: &Connection,
    table: &SchemaEntry,
    kinds: &HashMap<String, TableKind>,
) -> Result<TableCopy> {
    let unreadable = TableCopy {
        rows: 0,
        complete: false,
    };
    let Some(columns) = columns_of(source, &table.name) else {
        return Ok(unreadable);
    };
    let quoted = quote(&table.name);
    let list = columns
        .iter()
        .map(|column| quote(column))
        .collect::<Vec<_>>()
        .join(", ");
    let params = (1..=columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    // A virtual table settles conflicts itself (FTS5 has no unique key to collide on). Elsewhere a
    // duplicate can only come from an index the damage broke; the first copy wins.
    let verb = if is_virtual(table, kinds) {
        "INSERT"
    } else {
        "INSERT OR IGNORE"
    };
    let mut insert = match out.prepare(&format!("{verb} INTO {quoted} ({list}) VALUES ({params})"))
    {
        Ok(stmt) => stmt,
        Err(error) if is_write_failure(&error) => return Err(error.into()),
        Err(error) => {
            tracing::warn!(%error, table = %table.name, "could not prepare the copy of a table");
            return Ok(unreadable);
        }
    };
    let has_rowid = match kinds.get(&table.name) {
        Some(kind) => !kind.without_rowid,
        None => source
            .prepare(&format!("SELECT rowid FROM {quoted} LIMIT 0"))
            .is_ok(),
    };
    let mut copy = TableCopy {
        rows: 0,
        complete: true,
    };
    if has_rowid {
        copy_by_rowid(
            source,
            &quoted,
            &list,
            columns.len(),
            &mut insert,
            &mut copy,
        )?;
    } else {
        copy_in_order(
            source,
            &quoted,
            &list,
            columns.len(),
            &mut insert,
            &mut copy,
        )?;
    }
    Ok(copy)
}

/// Inserts the `count` columns of `row` starting at `first`. A row that can't be read or that the
/// new database rejects is skipped and makes the copy incomplete; only a write failure is an error.
fn insert_row(
    insert: &mut Statement<'_>,
    row: &rusqlite::Row<'_>,
    first: usize,
    count: usize,
    copy: &mut TableCopy,
) -> Result<()> {
    let values: rusqlite::Result<Vec<ValueRef<'_>>> = (first..first + count)
        .map(|index| row.get_ref(index))
        .collect();
    let Ok(values) = values else {
        copy.complete = false;
        return Ok(());
    };
    let values = values.into_iter().map(ToSqlOutput::Borrowed);
    match insert.execute(rusqlite::params_from_iter(values)) {
        Ok(changed) => {
            copy.rows += changed as u64;
            Ok(())
        }
        Err(error) if is_write_failure(&error) => Err(error.into()),
        Err(error) => {
            tracing::debug!(%error, "skipping a row the new database rejected");
            copy.complete = false;
            Ok(())
        }
    }
}

/// How a forward scan ended.
enum Scan {
    Finished,
    /// A row at or after `at` could not be read (`at` is one past the last row read, or the rowid
    /// the scan started from when it read nothing).
    Unreadable {
        at: i64,
    },
}

/// Copies a rowid table in rowid order ([`scan_by_rowid`]).
fn copy_by_rowid(
    source: &Connection,
    quoted: &str,
    list: &str,
    count: usize,
    insert: &mut Statement<'_>,
    copy: &mut TableCopy,
) -> Result<()> {
    let gap = scan_by_rowid(source, quoted, list, &mut |row| {
        insert_row(insert, row, 1, count, copy)
    })?;
    if gap.is_some() {
        copy.complete = false;
    }
    Ok(())
}

/// Reads a rowid table in rowid order, handing each row to `each` (the rowid first, then the
/// columns in `list`). When a page can't be read, the rows on it are lost, but the scan seeks past
/// it and carries on, so one bad page costs one page of rows, not the rest of the table. Returns
/// where the first unreadable stretch began (see [`Scan::Unreadable`]), if there was one. Only an
/// error from `each` stops it.
fn scan_by_rowid(
    source: &Connection,
    quoted: &str,
    list: &str,
    each: &mut dyn FnMut(&rusqlite::Row<'_>) -> Result<()>,
) -> Result<Option<i64>> {
    let select = format!("SELECT rowid, {list} FROM {quoted} WHERE rowid >= ?1 ORDER BY rowid");
    let seek = format!("SELECT rowid FROM {quoted} WHERE rowid >= ?1 ORDER BY rowid LIMIT 1");
    let mut next = i64::MIN;
    let mut first_gap = None;
    loop {
        match scan_from(source, &select, next, each)? {
            Scan::Finished => return Ok(first_gap),
            Scan::Unreadable { at } => {
                if first_gap.is_none() {
                    first_gap = Some(at);
                }
                match next_readable(source, &seek, at) {
                    Some(resume) => next = resume,
                    None => return Ok(first_gap),
                }
            }
        }
    }
}

fn scan_from(
    source: &Connection,
    select: &str,
    next: i64,
    each: &mut dyn FnMut(&rusqlite::Row<'_>) -> Result<()>,
) -> Result<Scan> {
    let Ok(mut stmt) = source.prepare(select) else {
        return Ok(Scan::Unreadable { at: next });
    };
    let Ok(mut rows) = stmt.query([next]) else {
        return Ok(Scan::Unreadable { at: next });
    };
    let mut at = next;
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let Ok(rowid) = row.get::<_, i64>(0) else {
                    return Ok(Scan::Unreadable { at });
                };
                each(row)?;
                match rowid.checked_add(1) {
                    Some(following) => at = following,
                    None => return Ok(Scan::Finished),
                }
            }
            Ok(None) => return Ok(Scan::Finished),
            Err(_) => return Ok(Scan::Unreadable { at }),
        }
    }
}

/// The first rowid after `at` that a seek can reach again, or `None` when nothing after it can be
/// read. A seek for rowid `x` fails while it lands on the damaged page and works once `x` is past
/// it, so this steps out exponentially until a seek works, then bisects back to the first one that
/// does, so readable rows just past the damage aren't skipped.
fn next_readable(source: &Connection, seek: &str, at: i64) -> Option<i64> {
    // Some(found) when the seek worked (found: the first rowid >= from, if any); None when it failed.
    let probe = |from: i64| match source.query_row(seek, [from], |row| row.get::<_, i64>(0)) {
        Ok(rowid) => Some(Some(rowid)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Some(None),
        Err(_) => None,
    };
    let mut low = at;
    let mut step: i64 = 1;
    let (mut high, mut found) = loop {
        let candidate = low.saturating_add(step);
        if candidate == low {
            return None;
        }
        match probe(candidate) {
            Some(found) => break (candidate, found),
            None => {
                low = candidate;
                step = step.saturating_mul(2);
            }
        }
    };
    // Invariant: the seek for `low` failed (or `low` is the unreadable row), the seek for `high`
    // worked and found `found`. i128 because `high - low` can exceed i64::MAX.
    while i128::from(high) - i128::from(low) > 1 {
        let mid = ((i128::from(low) + i128::from(high)) / 2) as i64;
        match probe(mid) {
            Some(result) => {
                high = mid;
                found = result;
            }
            None => low = mid,
        }
    }
    found
}

/// Copies a table without rowids (nothing to seek by) up to the first page it can't read.
fn copy_in_order(
    source: &Connection,
    quoted: &str,
    list: &str,
    count: usize,
    insert: &mut Statement<'_>,
    copy: &mut TableCopy,
) -> Result<()> {
    let Ok(mut stmt) = source.prepare(&format!("SELECT {list} FROM {quoted}")) else {
        copy.complete = false;
        return Ok(());
    };
    let Ok(mut rows) = stmt.query([]) else {
        copy.complete = false;
        return Ok(());
    };
    loop {
        match rows.next() {
            Ok(Some(row)) => insert_row(insert, row, 0, count, copy)?,
            Ok(None) => return Ok(()),
            Err(_) => {
                copy.complete = false;
                return Ok(());
            }
        }
    }
}

/// AUTOINCREMENT counters, so the copy never hands out an id the damaged database already used.
/// Copying rows with their ids already moved each counter to the largest id copied; this raises it
/// to the old value where that was higher (rows deleted from the end of a table).
fn copy_sequences(source: &Connection, out: &Connection) -> Result<()> {
    let Ok(mut stmt) = source.prepare("SELECT name, seq FROM sqlite_sequence") else {
        // No AUTOINCREMENT table, or the counters are unreadable.
        return Ok(());
    };
    let Ok(mut rows) = stmt.query([]) else {
        return Ok(());
    };
    while let Ok(Some(row)) = rows.next() {
        let (Ok(name), Ok(seq)) = (row.get::<_, String>(0), row.get::<_, i64>(1)) else {
            continue;
        };
        let raised = out
            .execute(
                "UPDATE sqlite_sequence SET seq = ?2 WHERE name = ?1 AND seq < ?2",
                rusqlite::params![name, seq],
            )
            .and_then(|_| {
                out.execute(
                    "INSERT INTO sqlite_sequence (name, seq) SELECT ?1, ?2 \
                     WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = ?1)",
                    rusqlite::params![name, seq],
                )
            });
        match raised {
            Ok(_) => {}
            Err(error) if is_write_failure(&error) => return Err(error.into()),
            Err(error) => {
                tracing::debug!(%error, table = %name, "could not copy an AUTOINCREMENT counter")
            }
        }
    }
    Ok(())
}

/// Settles the rows whose parent row didn't survive, pass after pass, until `foreign_key_check` is
/// clean, since every later migration runs that check. A dangling reference in a column that may
/// be NULL is cleared: the row is intact and still has a place in the UI (a chat whose folder was
/// lost is still in its project). A row whose required reference dangles is dropped (a chat whose
/// project was lost, then that chat's events), since nothing in the UI leads to it; it is still in
/// the quarantined file. Returns the tables that lost rows or references this way.
fn settle_orphans(out: &Connection) -> Result<Vec<String>> {
    let mut settled: Vec<String> = Vec::new();
    for _ in 0..MAX_ORPHAN_PASSES {
        let dangling: rusqlite::Result<Vec<(String, i64)>> = out
            .prepare("SELECT DISTINCT \"table\", fkid FROM pragma_foreign_key_check")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            });
        let dangling = match dangling {
            Ok(dangling) => dangling,
            Err(error) if is_write_failure(&error) => return Err(error.into()),
            Err(error) => {
                tracing::warn!(%error, "could not check the salvaged rows' references");
                break;
            }
        };
        if dangling.is_empty() {
            break;
        }
        let mut changed = 0;
        for (table, fkid) in dangling {
            let count = settle_reference(out, &table, fkid)?;
            changed += count;
            if count > 0 && !settled.contains(&table) {
                settled.push(table);
            }
        }
        if changed == 0 {
            break;
        }
    }
    Ok(settled)
}

/// Clears foreign key `fkid` of `table` where it dangles, or drops those rows when it can't be
/// cleared; returns how many rows changed.
fn settle_reference(out: &Connection, table: &str, fkid: i64) -> Result<usize> {
    let quoted = quote(table);
    let dangling = "SELECT rowid FROM pragma_foreign_key_check(?1) WHERE fkid = ?2";
    if let Some(columns) = nullable_reference(out, table, fkid) {
        let cleared = columns
            .iter()
            .map(|column| format!("{} = NULL", quote(column)))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("UPDATE {quoted} SET {cleared} WHERE rowid IN ({dangling})");
        match out.execute(&sql, rusqlite::params![table, fkid]) {
            Ok(count) => return Ok(count),
            Err(error) if is_write_failure(&error) => return Err(error.into()),
            // A CHECK constraint can refuse the NULL; the rows are dropped instead.
            Err(error) => {
                tracing::debug!(%error, %table, "could not clear a dangling reference; dropping the rows instead")
            }
        }
    }
    let sql = format!("DELETE FROM {quoted} WHERE rowid IN ({dangling})");
    match out.execute(&sql, rusqlite::params![table, fkid]) {
        Ok(count) => Ok(count),
        Err(error) if is_write_failure(&error) => Err(error.into()),
        // A table without rowids can't be targeted this way; its orphans stay.
        Err(error) => {
            tracing::warn!(%error, %table, "could not drop rows with broken references");
            Ok(0)
        }
    }
}

/// The columns of foreign key `fkid` of `table` when every one of them may be NULL (declared
/// without NOT NULL and not part of the primary key); `None` when the reference is required, or
/// its columns can't be read.
fn nullable_reference(out: &Connection, table: &str, fkid: i64) -> Option<Vec<String>> {
    let mut stmt = out
        .prepare(
            "SELECT fk.\"from\", info.\"notnull\" = 0 AND info.pk = 0 \
             FROM pragma_foreign_key_list(?1) AS fk \
             LEFT JOIN pragma_table_info(?1) AS info ON info.name = fk.\"from\" \
             WHERE fk.id = ?2 ORDER BY fk.seq",
        )
        .ok()?;
    let columns = stmt
        .query_map(rusqlite::params![table, fkid], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<bool>>(1)?))
        })
        .ok()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .ok()?;
    let nullable =
        !columns.is_empty() && columns.iter().all(|(_, nullable)| *nullable == Some(true));
    nullable.then(|| columns.into_iter().map(|(column, _)| column).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;

    /// Core fixture tables with row assertions; the complete schema is checked dynamically.
    const TABLES: [&str; 15] = [
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
        "message_fts",
        "queued_prompts",
        "composer_attachments",
        "composer_drafts",
        "chat_continuations",
    ];

    struct Fixture {
        _dir: tempfile::TempDir,
        db: PathBuf,
        out: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("aura.db");
        let out = dir.path().join("aura.db.salvage.partial");
        Fixture { _dir: dir, db, out }
    }

    fn migrations() -> &'static [M<'static>] {
        super::super::migrations()
    }

    fn backups(f: &Fixture) -> PathBuf {
        f.db.parent().expect("parent").join("backups")
    }

    /// A v1 database (the real schema) with two projects, three chats, `events` events spread over
    /// the chats, a search entry, an attachment, and a cached agent, closed so every page is in the
    /// main file.
    fn populated(f: &Fixture, events: i64) {
        let (store, _) = super::super::Store::open_at(&f.db, &backups(f)).expect("create");
        let conn = store.conn();
        let tx = conn.unchecked_transaction().expect("tx");
        for (project, name) in [("p1", "Alpha"), ("p2", "Beta")] {
            tx.execute(
                "INSERT INTO projects (id, name, created_at, updated_at, device_id) VALUES (?1, ?2, 1, 1, 'd')",
                [project, name],
            )
            .expect("project");
        }
        for (chat, project) in [("c1", "p1"), ("c2", "p1"), ("c3", "p2")] {
            tx.execute(
                "INSERT INTO chats (id, project_id, created_at, updated_at, device_id) VALUES (?1, ?2, 1, 1, 'd')",
                [chat, project],
            )
            .expect("chat");
        }
        insert_events(&tx, &["c1", "c2", "c3"], events);
        tx.execute_batch(
            "INSERT INTO message_fts (chat_id, message_id, text) VALUES ('c1', 'm1', 'crème brûlée recipe'); \
             INSERT INTO attachments (sha256, mime, size, created_at) VALUES ('ab12', 'image/png', 3, 1); \
             INSERT INTO agent_cache (agent_id, detection, checked_at) VALUES ('claude', '{}', 1);",
        )
        .expect("search entry, attachment, cached agent");
        tx.commit().expect("commit");
    }

    fn insert_events(conn: &Connection, chats: &[&str], events: i64) {
        for seq in 0..events {
            let chat = chats[usize::try_from(seq).expect("small") % chats.len()];
            conn.execute(
                "INSERT INTO events (id, chat_id, seq, kind, payload, created_at, device_id) \
                 VALUES (?1, ?2, ?3, 'message_delta', ?4, 1, 'd')",
                rusqlite::params![
                    format!("e{seq:05}"),
                    chat,
                    seq,
                    format!("{{\"text\": \"padding padding padding {seq}\"}}")
                ],
            )
            .expect("event");
        }
    }

    fn raw(path: &Path) -> Connection {
        Connection::open(path).expect("raw open")
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count")
    }

    fn count_where(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).expect("count")
    }

    fn quick_check(conn: &Connection) -> String {
        conn.query_row("PRAGMA quick_check", [], |row| row.get(0))
            .expect("quick_check")
    }

    fn user_version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("version")
    }

    fn fk_violations(conn: &Connection) -> i64 {
        count_where(conn, "SELECT count(*) FROM pragma_foreign_key_check")
    }

    fn fts_matches(conn: &Connection, query: &str) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM message_fts WHERE message_fts MATCH ?1",
            [query],
            |row| row.get(0),
        )
        .expect("fts")
    }

    fn rootpage(path: &Path, name: &str) -> i64 {
        raw(path)
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE name = ?1",
                [name],
                |row| row.get(0),
            )
            .expect("rootpage")
    }

    /// Page numbers of a table's pages of one kind in key order (`dbstat`'s path), filtered by a
    /// LIKE pattern on that path.
    fn pages_like(path: &Path, name: &str, kind: &str, like: &str) -> Vec<i64> {
        let conn = raw(path);
        let mut stmt = conn
            .prepare(
                "SELECT pageno FROM dbstat WHERE name = ?1 AND pagetype = ?2 AND path LIKE ?3 ORDER BY path",
            )
            .expect("dbstat");
        stmt.query_map([name, kind, like], |row| row.get(0))
            .expect("pages")
            .map(|row| row.expect("page"))
            .collect()
    }

    fn pages(path: &Path, name: &str, kind: &str) -> Vec<i64> {
        pages_like(path, name, kind, "%")
    }

    /// Overwrites the start of page `page` (1-based) so SQLite can't parse it.
    fn smash(path: &Path, page: i64) {
        let page_size: i64 = raw(path)
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .expect("page_size");
        let mut bytes = fs::read(path).expect("read");
        let start = usize::try_from((page - 1) * page_size).expect("offset");
        bytes[start..start + 16].fill(0xA5);
        fs::write(path, bytes).expect("write");
    }

    /// The names in `sqlite_schema` a lookup by rowid can still read, after the damage.
    fn readable_entries(path: &Path) -> BTreeSet<String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read-only open");
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

    /// A read-only connection to a WAL database creates an empty `-wal` (and `-shm`) when none
    /// exists, and can't delete them; nothing may be written into the WAL, though.
    fn nothing_written_to_the_wal(path: &Path) -> bool {
        fs::metadata(fsutil::with_suffix(path, "-wal")).map_or(true, |meta| meta.len() == 0)
    }

    #[test]
    fn leaked_pages_are_dropped_and_everything_else_is_kept() {
        let f = fixture();
        populated(&f, 50);
        {
            let conn = raw(&f.db);
            conn.execute_batch(
                "CREATE TABLE junk (x); WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 300) \
                 INSERT INTO junk SELECT printf('%0200d', i) FROM n; \
                 PRAGMA writable_schema = ON; DELETE FROM sqlite_schema WHERE name = 'junk'; PRAGMA writable_schema = OFF;",
            )
            .expect("leak the pages of a dropped table");
        }
        let check = quick_check(&raw(&f.db));
        assert!(check.contains("never used"), "{check}");
        let before = fs::read(&f.db).expect("read");

        assert_eq!(
            salvage(&f.db, &f.out, migrations()).expect("salvage"),
            Salvage::Complete
        );
        assert_eq!(
            fs::read(&f.db).expect("read"),
            before,
            "the source is only read"
        );
        assert!(nothing_written_to_the_wal(&f.db));
        let copy = raw(&f.out);
        assert_eq!(quick_check(&copy), "ok");
        assert_eq!(count(&copy, "projects"), 2);
        assert_eq!(count(&copy, "events"), 50);
        assert_eq!(
            user_version(&copy),
            i64::from(super::super::SCHEMA_VERSION),
            "user_version is kept"
        );
    }

    #[test]
    fn one_bad_leaf_costs_only_its_rows() {
        let f = fixture();
        populated(&f, 3000);
        let leaves = pages(&f.db, "events", "leaf");
        assert!(leaves.len() > 10, "{leaves:?}");
        smash(&f.db, leaves[leaves.len() / 2]);
        let before = fs::read(&f.db).expect("read");

        let salvage = salvage(&f.db, &f.out, migrations()).expect("salvage");
        assert_eq!(
            salvage,
            Salvage::Partial {
                tables: vec!["events".into()]
            }
        );
        assert_eq!(fs::read(&f.db).expect("read"), before);
        let copy = raw(&f.out);
        assert_eq!(quick_check(&copy), "ok");
        assert_eq!(count(&copy, "projects"), 2);
        assert_eq!(count(&copy, "chats"), 3);
        let events = count(&copy, "events");
        // One leaf holds a few dozen of these rows; everything before and after it survives.
        assert!((2800..3000).contains(&events), "{events} events kept");
        let first_and_last = count_where(
            &copy,
            "SELECT count(*) FROM events WHERE id IN ('e00000', 'e02999')",
        );
        assert_eq!(
            first_and_last, 2,
            "rows past the bad page were reached by seeking past it"
        );
        assert_eq!(
            fts_matches(&copy, "creme"),
            1,
            "the full-text index was rebuilt through its virtual table"
        );
        assert_eq!(user_version(&copy), i64::from(super::super::SCHEMA_VERSION));
    }

    #[test]
    fn a_bad_first_leaf_still_lets_the_rest_of_the_table_through() {
        let f = fixture();
        populated(&f, 3000);
        // In key order, so this is the leaf holding the smallest rowids, where every scan starts.
        let leaves = pages(&f.db, "events", "leaf");
        smash(&f.db, leaves[0]);
        let salvage = salvage(&f.db, &f.out, migrations()).expect("salvage");
        assert!(matches!(salvage, Salvage::Partial { .. }), "{salvage:?}");
        let events = count(&raw(&f.out), "events");
        assert!((2800..3000).contains(&events), "{events} events kept");
    }

    #[test]
    fn a_bad_overflow_page_costs_only_its_row() {
        let f = fixture();
        populated(&f, 10);
        {
            let conn = raw(&f.db);
            for n in 0..5 {
                conn.execute(
                    "INSERT INTO events (id, chat_id, seq, kind, payload, created_at, device_id) \
                     VALUES (?1, 'c1', ?2, 'tool_update', ?3, 1, 'd')",
                    rusqlite::params![format!("big{n}"), 100 + n, "x".repeat(20_000)],
                )
                .expect("big event");
            }
        }
        // The first page of a chain: its next-page pointer is what SQLite follows (and rejects),
        // while the last page's pointer is never read.
        let overflow = pages_like(&f.db, "events", "overflow", "%+000000");
        assert!(overflow.len() >= 5, "{overflow:?}");
        smash(&f.db, overflow[overflow.len() / 2]);
        let salvage = salvage(&f.db, &f.out, migrations()).expect("salvage");
        assert_eq!(
            salvage,
            Salvage::Partial {
                tables: vec!["events".into()]
            }
        );
        let copy = raw(&f.out);
        assert_eq!(count(&copy, "events"), 14, "15 events, one unreadable");
    }

    #[test]
    fn rows_whose_required_parent_was_lost_are_dropped() {
        let f = fixture();
        populated(&f, 30);
        smash(&f.db, rootpage(&f.db, "chats"));
        let salvage = salvage(&f.db, &f.out, migrations()).expect("salvage");
        assert_eq!(
            salvage,
            Salvage::Partial {
                tables: vec!["chats".into(), "events".into()]
            }
        );
        let copy = raw(&f.out);
        assert_eq!(
            count(&copy, "projects"),
            2,
            "parents above the damage are kept"
        );
        assert_eq!(count(&copy, "chats"), 0);
        assert_eq!(
            count(&copy, "events"),
            0,
            "their events had nowhere to belong"
        );
        assert_eq!(fk_violations(&copy), 0);
    }

    /// One project with one folder (and its location on this device), three chats in the folder
    /// and one outside it, and `events` events over the four chats.
    fn with_a_folder(f: &Fixture, events: i64) {
        let (store, _) = super::super::Store::open_at(&f.db, &backups(f)).expect("create");
        let tx = store.conn().unchecked_transaction().expect("tx");
        tx.execute_batch(
            "INSERT INTO projects (id, name, created_at, updated_at, device_id) VALUES ('p1', 'Alpha', 1, 1, 'd'); \
             INSERT INTO project_folders (id, project_id, display_name, kind, created_at, updated_at, device_id) \
                 VALUES ('f1', 'p1', 'repo', 'git', 1, 1, 'd'); \
             INSERT INTO folder_locations (folder_id, device_id, path) VALUES ('f1', 'd', '/src/repo'); \
             INSERT INTO chats (id, project_id, folder_id, created_at, updated_at, device_id) VALUES \
                 ('c1', 'p1', 'f1', 1, 1, 'd'), ('c2', 'p1', 'f1', 1, 1, 'd'), ('c3', 'p1', 'f1', 1, 1, 'd'), \
                 ('c4', 'p1', NULL, 1, 1, 'd');",
        )
        .expect("project, folder, chats");
        insert_events(&tx, &["c1", "c2", "c3", "c4"], events);
        tx.commit().expect("commit");
    }

    /// Finding: every row the foreign key check flagged was deleted, so losing the one page of
    /// `project_folders` also deleted the chats in that folder (whose folder_id may be NULL) and,
    /// in the next pass, all their events, although the project and every chat and event page
    /// were intact.
    #[test]
    fn a_lost_folder_clears_the_chats_reference_instead_of_dropping_them() {
        let f = fixture();
        with_a_folder(&f, 400);
        smash(&f.db, rootpage(&f.db, "project_folders"));
        let salvage = salvage(&f.db, &f.out, migrations()).expect("salvage");
        assert_eq!(
            salvage,
            Salvage::Partial {
                tables: vec![
                    "project_folders".into(),
                    "folder_locations".into(),
                    "chats".into()
                ]
            },
            "chats are reported: their folder references were cleared"
        );
        let copy = raw(&f.out);
        assert_eq!(
            count(&copy, "chats"),
            4,
            "every chat is still in its project"
        );
        assert_eq!(count(&copy, "events"), 400);
        assert_eq!(
            count_where(
                &copy,
                "SELECT count(*) FROM chats WHERE folder_id IS NOT NULL"
            ),
            0
        );
        assert_eq!(
            count(&copy, "folder_locations"),
            0,
            "a location can't exist without its folder"
        );
        assert_eq!(fk_violations(&copy), 0);
        assert_eq!(quick_check(&copy), "ok");
    }

    /// The same for the optional link from a turn to its agent session: a damaged
    /// `agent_sessions` page used to take the turns and every event of those turns with it.
    #[test]
    fn a_lost_agent_session_clears_the_turns_reference_and_keeps_their_events() {
        let f = fixture();
        populated(&f, 0);
        {
            let conn = raw(&f.db);
            conn.execute(
                "INSERT INTO agent_sessions (id, chat_id, agent_id, cwd, created_at, updated_at, device_id) \
                 VALUES ('s1', 'c1', 'claude', '/src', 1, 1, 'd')",
                [],
            )
            .expect("session");
            for turn in 0..3i64 {
                conn.execute(
                    "INSERT INTO turns (id, chat_id, agent_session_id, seq, status, started_at, device_id) \
                     VALUES (?1, 'c1', 's1', ?2, 'completed', 1, 'd')",
                    rusqlite::params![format!("t{turn}"), turn],
                )
                .expect("turn");
                for n in 0..5i64 {
                    conn.execute(
                        "INSERT INTO events (id, chat_id, turn_id, seq, kind, payload, created_at, device_id) \
                         VALUES (?1, 'c1', ?2, ?3, 'message_delta', '{}', 1, 'd')",
                        rusqlite::params![format!("t{turn}e{n}"), format!("t{turn}"), turn * 5 + n],
                    )
                    .expect("event");
                }
            }
        }
        smash(&f.db, rootpage(&f.db, "agent_sessions"));
        let salvage = salvage(&f.db, &f.out, migrations()).expect("salvage");
        assert_eq!(
            salvage,
            Salvage::Partial {
                tables: vec!["agent_sessions".into(), "turns".into()]
            }
        );
        let copy = raw(&f.out);
        assert_eq!(count(&copy, "turns"), 3);
        assert_eq!(
            count_where(
                &copy,
                "SELECT count(*) FROM turns WHERE agent_session_id IS NOT NULL"
            ),
            0
        );
        assert_eq!(count(&copy, "events"), 15);
        assert_eq!(
            count_where(&copy, "SELECT count(*) FROM events WHERE turn_id IS NULL"),
            0,
            "the turns survived, so their events keep pointing at them"
        );
        assert_eq!(fk_violations(&copy), 0);
    }

    /// Finding: the schema was read only up to its first unreadable page and the copy was built
    /// from what that read found, stamped with the damaged file's `user_version`. A damaged second
    /// leaf dropped every table listed on it while reporting Complete, and no migration would ever
    /// create them again; a damaged first leaf salvaged nothing although every data page, and the
    /// entries on the other leaf, were fine.
    #[test]
    fn a_damaged_schema_leaf_keeps_every_table_and_what_its_entries_still_reach() {
        let leaves = {
            let f = fixture();
            populated(&f, 300);
            pages(&f.db, "sqlite_schema", "leaf")
        };
        assert!(
            leaves.len() >= 2,
            "the v1 schema spans several leaves: {leaves:?}"
        );
        for (index, leaf) in leaves.iter().enumerate() {
            let f = fixture();
            populated(&f, 300);
            let expected_tables: BTreeSet<_> =
                own_tables(&raw(&f.db)).unwrap().into_iter().collect();
            smash(&f.db, *leaf);
            let readable = readable_entries(&f.db);
            let before = fs::read(&f.db).expect("read");

            let salvage = salvage(&f.db, &f.out, migrations()).expect("salvage");
            let Salvage::Partial { tables } = &salvage else {
                panic!("leaf {index}: a damaged schema is never complete, got {salvage:?}");
            };
            assert_eq!(fs::read(&f.db).expect("read"), before, "only read");
            assert!(!scratch_path(&f.out).exists(), "the rebuilt copy is gone");
            let copy = raw(&f.out);
            assert_eq!(quick_check(&copy), "ok");
            assert_eq!(
                own_tables(&copy)
                    .unwrap()
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                expected_tables,
                "every table from the current migrations must be reconstructed"
            );
            assert_eq!(user_version(&copy), i64::from(super::super::SCHEMA_VERSION));
            assert_eq!(fk_violations(&copy), 0);
            for table in TABLES {
                // Fails with "no such table" if the copy lacks it.
                let rows = count(&copy, table);
                if !readable.contains(table) {
                    assert!(
                        tables.iter().any(|lost| lost == table),
                        "leaf {index}: {table} was unreadable but isn't reported: {tables:?}"
                    );
                    assert_eq!(rows, 0, "leaf {index}: {table}");
                }
            }
            // Tables that need no parent row keep every row wherever their entry is readable.
            for (table, rows) in [("projects", 2), ("attachments", 1), ("agent_cache", 1)] {
                if readable.contains(table) {
                    assert_eq!(count(&copy, table), rows, "leaf {index}: {table}");
                }
            }
            if readable.contains("message_fts_content") {
                assert_eq!(fts_matches(&copy, "creme"), 1, "leaf {index}");
            }
            if index == 0 {
                // The first leaf holds the first tables made; the ones listed after it are read
                // through the rebuilt schema.
                assert!(!readable.contains("projects"), "{readable:?}");
                assert!(readable.contains("agent_cache"), "{readable:?}");
                assert!(tables.iter().any(|lost| lost == "projects"), "{tables:?}");
                assert_eq!(count(&copy, "agent_cache"), 1);
                // Readable, but their chats went with the first leaf, and an event can't exist
                // without its chat.
                assert_eq!(count(&copy, "events"), 0);
                assert!(tables.iter().any(|lost| lost == "events"), "{tables:?}");
            }
            if index == leaves.len() - 1 {
                assert!(readable.contains("projects"), "{readable:?}");
                // Later migrations can move FTS off the last schema leaf. Check the actual
                // unreadable table set instead of assuming a fixed schema-page layout.
                assert!(
                    expected_tables
                        .iter()
                        .any(|table| !readable.contains(table)),
                    "{readable:?}"
                );
            }
        }
    }

    /// Damage often comes with an unclean shutdown, so the newest rows may be only in the damaged
    /// file's `-wal`. The rebuilt schema's copy is made through SQLite, so they come along.
    #[test]
    fn rows_only_in_the_wal_come_through_a_rebuilt_schema() {
        let f = fixture();
        populated(&f, 30);
        let first_leaf = pages(&f.db, "sqlite_schema", "leaf")[0];
        let page_size: i64 = raw(&f.db)
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .expect("page_size");
        {
            let conn = raw(&f.db);
            conn.execute(
                "INSERT INTO agent_cache (agent_id, detection, checked_at) VALUES ('codex', '{}', 2)",
                [],
            )
            .expect("a row only in the -wal");
            conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
                .expect("keep the wal");
        }
        let wal = fsutil::with_suffix(&f.db, "-wal");
        assert!(fs::metadata(&wal).expect("wal").len() > 0);
        // Not `smash`: opening the database to read its page size would checkpoint the -wal.
        let mut bytes = fs::read(&f.db).expect("read");
        let start = usize::try_from((first_leaf - 1) * page_size).expect("offset");
        bytes[start..start + 16].fill(0xA5);
        fs::write(&f.db, bytes).expect("damage the first schema leaf");
        let wal_before = fs::read(&wal).expect("wal");

        let salvage = salvage(&f.db, &f.out, migrations()).expect("salvage");
        assert!(matches!(salvage, Salvage::Partial { .. }), "{salvage:?}");
        assert_eq!(fs::read(&wal).expect("wal"), wal_before, "only read");
        assert_eq!(
            count(&raw(&f.out), "agent_cache"),
            2,
            "the row only in the -wal came along"
        );
    }

    /// A damaged schema page that listed only indexes costs no Ghost table, but whatever else
    /// it listed is unknown, so the salvage still isn't called complete.
    #[test]
    fn a_damaged_schema_page_is_never_complete_even_when_every_table_survives() {
        let f = fixture();
        populated(&f, 10);
        {
            // Enough indexes, with long enough names, that the schema's last leaf lists only them.
            let conn = raw(&f.db);
            for n in 0..60 {
                conn.execute_batch(&format!(
                    "CREATE INDEX extra_{n:02}_{} ON events (kind, created_at);",
                    "x".repeat(150)
                ))
                .expect("index");
            }
        }
        let leaves = pages(&f.db, "sqlite_schema", "leaf");
        smash(&f.db, leaves[leaves.len() - 1]);
        let readable = readable_entries(&f.db);
        assert!(
            TABLES.iter().all(|table| readable.contains(*table)),
            "the last leaf lists only indexes: {readable:?}"
        );

        assert_eq!(
            salvage(&f.db, &f.out, migrations()).expect("salvage"),
            Salvage::Partial { tables: Vec::new() }
        );
        let copy = raw(&f.out);
        assert_eq!(count(&copy, "events"), 10);
        assert_eq!(count(&copy, "projects"), 2);
        assert_eq!(
            count_where(
                &copy,
                "SELECT count(*) FROM sqlite_schema WHERE name = 'events_turn_id'"
            ),
            1,
            "Ghost's own indexes come from the migrations"
        );
    }

    /// Without the rebuilt schema (no room for the copy it needs), the tables listed after the
    /// damage can't be read, and the salvage says what it could do instead of failing the repair.
    #[test]
    fn without_a_rebuilt_schema_the_salvage_still_finishes() {
        let f = fixture();
        populated(&f, 30);
        smash(&f.db, pages(&f.db, "sqlite_schema", "leaf")[0]);
        let scratch = scratch_path(&f.out);
        fs::create_dir(&scratch).expect("something in the way of the copy");
        assert_eq!(
            salvage(&f.db, &f.out, migrations()).expect("salvage"),
            Salvage::Nothing,
            "nothing SQLite still knows of could be read"
        );
        assert!(!f.out.exists());
        assert!(scratch.is_dir(), "what was in the way is left alone");
    }

    #[test]
    fn a_table_missing_from_an_intact_schema_is_created_empty_and_reported() {
        let f = fixture();
        populated(&f, 3000);
        raw(&f.db)
            .execute_batch("DROP TABLE agent_cache;")
            .expect("drop");
        let leaves = pages(&f.db, "events", "leaf");
        smash(&f.db, leaves[leaves.len() / 2]);
        assert_eq!(
            salvage(&f.db, &f.out, migrations()).expect("salvage"),
            Salvage::Partial {
                tables: vec!["events".into(), "agent_cache".into()]
            }
        );
        assert_eq!(count(&raw(&f.out), "agent_cache"), 0);
    }

    /// The storage of a virtual table whose own entry was lost is not a table of its own: it isn't
    /// reported (the virtual table is), and the copy's comes from the migrations.
    #[test]
    fn a_virtual_tables_storage_without_its_entry_is_not_reported_as_a_table() {
        let f = fixture();
        populated(&f, 3000);
        raw(&f.db)
            .execute_batch(
                "PRAGMA writable_schema = ON; DELETE FROM sqlite_schema WHERE name = 'message_fts';",
            )
            .expect("lose the virtual table's entry");
        let leaves = pages(&f.db, "events", "leaf");
        smash(&f.db, leaves[leaves.len() / 2]);
        assert_eq!(
            salvage(&f.db, &f.out, migrations()).expect("salvage"),
            Salvage::Partial {
                tables: vec!["events".into(), "message_fts".into()]
            }
        );
        let copy = raw(&f.out);
        assert_eq!(count(&copy, "message_fts"), 0);
        assert_eq!(quick_check(&copy), "ok");
    }

    /// A migration's trigger must not fire on copied rows (here it would count every copied event
    /// into its chat a second time), and is back once they are in. Rows a migration seeds give way
    /// to the damaged file's.
    #[test]
    fn triggers_dont_fire_on_copied_rows_and_seeded_rows_give_way() {
        let f = fixture();
        let mut v2 = migrations().to_vec();
        v2.push(M::up(
            "CREATE TABLE event_count (n INTEGER NOT NULL); INSERT INTO event_count VALUES (0); \
             CREATE TRIGGER events_counted AFTER INSERT ON events BEGIN \
                 UPDATE event_count SET n = n + 1; \
                 UPDATE chats SET updated_at = updated_at + 1 WHERE id = new.chat_id; \
             END; \
             INSERT INTO settings (key, value, updated_at) VALUES ('theme', 'light', 0);",
        ));
        {
            let (store, _) = super::super::open_with(&f.db, &backups(&f), &v2).expect("v2");
            let tx = store.conn().unchecked_transaction().expect("tx");
            tx.execute_batch(
                "INSERT INTO projects (id, name, created_at, updated_at, device_id) VALUES ('p1', 'Alpha', 1, 1, 'd'); \
                 INSERT INTO chats (id, project_id, created_at, updated_at, device_id) VALUES ('c1', 'p1', 1, 1, 'd'); \
                 UPDATE settings SET value = 'dark', updated_at = 5 WHERE key = 'theme';",
            )
            .expect("project, chat, setting");
            insert_events(&tx, &["c1"], 3000);
            tx.commit().expect("commit");
        }
        let leaves = pages(&f.db, "events", "leaf");
        smash(&f.db, leaves[leaves.len() / 2]);
        let salvage = salvage(&f.db, &f.out, &v2).expect("salvage");
        assert_eq!(
            salvage,
            Salvage::Partial {
                tables: vec!["events".into()]
            }
        );
        let copy = raw(&f.out);
        assert_eq!(
            user_version(&copy),
            i64::from(super::super::SCHEMA_VERSION) + 1
        );
        // `chats` is copied before `events`, so a trigger firing on copied events would show.
        assert_eq!(
            count_where(&copy, "SELECT updated_at FROM chats WHERE id = 'c1'"),
            3001,
            "the chat as it was, not touched again by the copy"
        );
        assert_eq!(count(&copy, "event_count"), 1, "the seeded row gave way");
        assert_eq!(
            count_where(&copy, "SELECT n FROM event_count"),
            3000,
            "the count copied over"
        );
        let theme: String = copy
            .query_row(
                "SELECT value FROM settings WHERE key = 'theme'",
                [],
                |row| row.get(0),
            )
            .expect("theme");
        assert_eq!(theme, "dark", "the user's setting, not the seeded default");
        copy.execute(
            "INSERT INTO events (id, chat_id, seq, kind, payload, created_at, device_id) \
             VALUES ('new', 'c1', 99999, 'message_delta', '{}', 1, 'd')",
            [],
        )
        .expect("a new event");
        assert_eq!(
            count_where(&copy, "SELECT n FROM event_count"),
            3001,
            "the trigger is back"
        );
        assert_eq!(
            count_where(&copy, "SELECT updated_at FROM chats WHERE id = 'c1'"),
            3002
        );
    }

    /// Only reachable by calling `salvage` directly (the open refuses a newer database first).
    #[test]
    fn a_version_the_migrations_dont_know_keeps_the_damaged_files_own_schema() {
        let f = fixture();
        populated(&f, 3000);
        let future_version = super::super::SCHEMA_VERSION + 1;
        raw(&f.db)
            .execute_batch(&format!(
                "CREATE TABLE later (x); INSERT INTO later VALUES ('from a later version'); PRAGMA user_version = {future_version};"
            ))
            .expect("future schema version");
        let leaves = pages(&f.db, "events", "leaf");
        smash(&f.db, leaves[leaves.len() / 2]);
        assert_eq!(
            salvage(&f.db, &f.out, migrations()).expect("salvage"),
            Salvage::Partial {
                tables: vec!["events".into()]
            }
        );
        let copy = raw(&f.out);
        assert_eq!(user_version(&copy), i64::from(future_version));
        assert_eq!(count(&copy, "later"), 1);
        assert_eq!(count(&copy, "projects"), 2);
        assert_eq!(fts_matches(&copy, "creme"), 1);
    }

    #[test]
    fn a_malformed_schema_entry_costs_only_its_table() {
        let f = fixture();
        populated(&f, 20);
        {
            let conn = raw(&f.db);
            conn.execute_batch(
                "PRAGMA writable_schema = ON; \
                 INSERT INTO sqlite_schema (type, name, tbl_name, rootpage, sql) \
                 VALUES ('table', 'broken', 'broken', 0, 'CREATE TABLE broken (x MALFORMED ((');",
            )
            .expect("malformed entry");
        }
        let plain: rusqlite::Result<i64> =
            raw(&f.db).query_row("SELECT count(*) FROM projects", [], |row| row.get(0));
        assert!(
            plain.is_err(),
            "every statement fails on a malformed schema"
        );

        let salvage = salvage(&f.db, &f.out, migrations()).expect("salvage");
        assert_eq!(
            salvage,
            Salvage::Partial {
                tables: vec!["broken".into()]
            }
        );
        let copy = raw(&f.out);
        assert_eq!(count(&copy, "projects"), 2);
        assert_eq!(count(&copy, "events"), 20);
    }

    #[test]
    fn tables_without_rowids_are_copied_too() {
        let f = fixture();
        populated(&f, 3000);
        {
            let conn = raw(&f.db);
            conn.execute_batch(
                "CREATE TABLE tags (name TEXT PRIMARY KEY, color TEXT) WITHOUT ROWID; \
                 INSERT INTO tags VALUES ('red', '#f00'), ('blue', '#00f');",
            )
            .expect("without rowid");
        }
        // Force the table-by-table path with a bad events leaf.
        let leaves = pages(&f.db, "events", "leaf");
        smash(&f.db, leaves[leaves.len() / 2]);
        assert!(matches!(
            salvage(&f.db, &f.out, migrations()).expect("salvage"),
            Salvage::Partial { .. }
        ));
        let copy = raw(&f.out);
        assert_eq!(count(&copy, "tags"), 2);
    }

    #[test]
    fn random_bytes_salvage_nothing_and_leave_nothing_behind() {
        let f = fixture();
        fs::write(&f.db, vec![0x5a; 16 * 1024]).expect("garbage");
        assert_eq!(
            salvage(&f.db, &f.out, migrations()).expect("salvage"),
            Salvage::Nothing
        );
        assert!(!f.out.exists());
        assert!(!fsutil::with_suffix(&f.out, "-journal").exists());
    }

    /// Only problems writing the copy are errors, and they leave the damaged file as it was.
    #[test]
    fn an_unwritable_output_is_an_error_and_the_source_is_untouched() {
        let f = fixture();
        populated(&f, 5);
        let before = fs::read(&f.db).expect("read");
        let out =
            f.db.with_file_name("no such folder")
                .join("aura.db.salvage-1.partial");
        let result = salvage(&f.db, &out, migrations());
        assert!(result.is_err(), "{result:?}");
        assert_eq!(fs::read(&f.db).expect("read"), before);
        assert!(!out.exists());
    }

    #[test]
    fn a_stale_output_is_replaced() {
        let f = fixture();
        populated(&f, 5);
        fs::write(&f.out, b"left over from an interrupted repair").expect("stale");
        assert_eq!(
            salvage(&f.db, &f.out, migrations()).expect("salvage"),
            Salvage::Complete
        );
        assert_eq!(count(&raw(&f.out), "events"), 5);
    }

    #[test]
    fn the_wal_of_the_damaged_database_is_read_but_not_touched() {
        let f = fixture();
        populated(&f, 5);
        {
            let conn = raw(&f.db);
            conn.execute(
                "INSERT INTO projects (id, name, created_at, updated_at, device_id) VALUES ('p3', 'Only in the WAL', 1, 1, 'd')",
                [],
            )
            .expect("insert");
            conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
                .expect("keep the wal");
        }
        let wal = fsutil::with_suffix(&f.db, "-wal");
        let wal_before = fs::read(&wal).expect("wal");
        let db_before = fs::read(&f.db).expect("db");
        assert_eq!(
            salvage(&f.db, &f.out, migrations()).expect("salvage"),
            Salvage::Complete
        );
        assert_eq!(fs::read(&wal).expect("wal"), wal_before);
        assert_eq!(fs::read(&f.db).expect("db"), db_before);
        assert_eq!(
            count(&raw(&f.out), "projects"),
            3,
            "the WAL's rows are salvaged"
        );
    }

    #[test]
    fn next_readable_bisects_to_the_first_reachable_rowid() {
        let conn = Connection::open_in_memory().expect("memory");
        conn.execute_batch(
            "CREATE TABLE t (v); WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100) \
             INSERT INTO t (rowid, v) SELECT i, i FROM n;",
        )
        .expect("rows");
        // A seek fails (json() of malformed text raises an error) for rowids up to 40, a stand-in
        // for a damaged page, and works from 41.
        let seek = "SELECT rowid FROM t WHERE rowid >= ?1 AND CASE WHEN ?1 <= 40 THEN json('x') ELSE 1 END ORDER BY rowid LIMIT 1";
        assert!(
            conn.query_row(seek, [5], |row| row.get::<_, i64>(0))
                .is_err()
        );
        assert_eq!(next_readable(&conn, seek, 10), Some(41));
        assert_eq!(next_readable(&conn, seek, i64::MIN), Some(41));
        // Nothing readable after the end.
        let none = "SELECT rowid FROM t WHERE rowid >= ?1 AND CASE WHEN ?1 <= 1000 THEN json('x') ELSE 1 END ORDER BY rowid LIMIT 1";
        assert_eq!(next_readable(&conn, none, 10), None);
        assert_eq!(next_readable(&conn, seek, i64::MAX), None);
    }

    #[test]
    fn write_failures_are_told_apart_from_rejected_statements() {
        let full = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
            None,
        );
        assert!(is_write_failure(&full));
        let constraint = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
            None,
        );
        assert!(!is_write_failure(&constraint));
        assert!(!is_write_failure(&rusqlite::Error::QueryReturnedNoRows));
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(quote("events"), "\"events\"");
        assert_eq!(quote("odd\"name"), "\"odd\"\"name\"");
    }

    #[test]
    fn the_rebuilt_schema_copy_is_named_after_the_salvage_copy() {
        assert_eq!(
            scratch_path(Path::new("/p/aura.db.salvage-17.partial")),
            PathBuf::from("/p/aura.db.salvage-17.schema.partial")
        );
        assert_eq!(
            scratch_path(Path::new("/p/copy.db")),
            PathBuf::from("/p/copy.db.schema")
        );
    }
}
