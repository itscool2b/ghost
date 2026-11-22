//! The notes of a project's memory, and the revisions the brief and the notes share.
//!
//! Memory was one document that every chat received whole. It reached its limit within days, so
//! every fact an agent added pushed another out, and every chat paid for all of it whether it
//! mattered to its task or not. Now a project has a short **brief**, which every chat receives,
//! and **notes**: each a document with a name and one line that says when to read it. A chat
//! receives the brief and the list of notes; its agent reads a note when its line fits the task,
//! and saves one note without touching the others.
//!
//! Who may do what: the user reads and writes everything, through Settings. An agent reads and
//! writes only its own chat's project, through its chat's grant (`memory_service`), which names
//! the chat; it cannot name a project. A linked bot writes only the brief. Every write names the
//! revision it read (of the brief, or of that one note), and a write over a newer revision is
//! refused. An agent's write is refused while the profile's switch is off, when its chat does not
//! belong to the project, and past a number of writes an hour.
//!
//! What a note's line may hold matters more than its text: the line reaches every chat of the
//! project in the list, the text only a chat that asks for it. So a name is lowercase letters,
//! digits and dashes, and a description is one bounded line with no control characters, shown in
//! quotes as a record of what the note is about.
//!
//! One chain of revisions covers a project: each revision is of the brief or of one note, and
//! points at the revision that was the project's newest before it. The newest of all is the
//! project's head (`project_memory.revision`), which is what a chat is compared against to decide
//! whether memory changed since it last received it.

use o7_protocol::memory::{DeleteMemoryNote, MemoryNote, MemoryNoteSummary, SaveMemoryNote};
use rusqlite::{Connection, OptionalExtension, params};

use super::memory::{AGENT_SOURCE, MAX_SOURCE, invalid};

/// The most the brief holds. Every chat of the project receives it, so it stays short.
pub(crate) const MAX_BRIEF: usize = 4 * 1024;
/// The most one note holds.
pub(crate) const MAX_NOTE: usize = 16 * 1024;
/// The most notes a project has: the list reaches every chat too.
pub(crate) const MAX_NOTES: usize = 32;
/// The longest name of a note.
pub(crate) const MAX_NAME: usize = 40;
/// The longest description of a note, in characters.
pub(crate) const MAX_DESCRIPTION: usize = 120;
/// Revisions kept of the brief and of each note: what Undo and the list need, and no more.
const REVISIONS_KEPT: i64 = 20;
/// How many times one chat's agent may save memory in an hour.
pub(crate) const AGENT_WRITES_AN_HOUR: i64 = 20;
/// That hour, in milliseconds. No revision younger than this is pruned: the limit is counted from
/// the revisions themselves, so trimming them early would let other writers push a chat's own
/// saves out of its count.
const WRITE_WINDOW_MS: i64 = 3_600_000;
/// The source of a revision the user saved.
pub(crate) const USER_SOURCE: &str = "user";
/// The source of a revision a linked bot saved.
pub(crate) const BOT_SOURCE: &str = "bot";
/// How far back the changes since a chat's last message are followed before it is simply sent
/// everything again.
const CHANGES_FOLLOWED: usize = 32;

/// Who saved a revision, as the list says it: the user, an agent, or a linked bot.
pub(crate) fn by(source: &str) -> &'static str {
    if source.starts_with(AGENT_SOURCE) {
        "agent"
    } else if source == BOT_SOURCE {
        "bot"
    } else {
        "user"
    }
}

/// The project's newest revision, of the brief or of a note.
pub(crate) fn head(conn: &Connection, project: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT revision FROM project_memory WHERE project_id=?1",
        [project],
        |r| r.get(0),
    )
    .optional()
}

/// The brief's own newest revision: what a save of the brief names as its base.
pub(crate) fn brief_revision(conn: &Connection, project: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT revision FROM project_memory_revisions WHERE project_id=?1 AND note IS NULL \
         ORDER BY rowid DESC LIMIT 1",
        [project],
        |r| r.get(0),
    )
    .optional()
}

/// Adds a revision of the brief (`note` is `None`) or of a note and makes it the project's head.
/// Returns the new revision and the head it replaced. The caller has checked everything.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_revision(
    conn: &Connection,
    project: &str,
    note: Option<&str>,
    description: Option<&str>,
    markdown: &str,
    deleted: bool,
    reason: &str,
    source: &str,
    now: i64,
) -> rusqlite::Result<(String, Option<String>)> {
    let parent = head(conn, project)?;
    let revision = crate::ids::new_id();
    conn.execute(
        "INSERT INTO project_memory_revisions(revision,project_id,parent_revision,markdown,reason,source,created_at,note,description,deleted) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![revision, project, parent, markdown, reason, source, now, note, description, deleted],
    )?;
    if note.is_none() {
        conn.execute(
            "INSERT INTO project_memory(project_id,revision,markdown,updated_at) VALUES(?1,?2,?3,?4) \
             ON CONFLICT(project_id) DO UPDATE SET revision=excluded.revision,markdown=excluded.markdown,updated_at=excluded.updated_at",
            params![project, revision, markdown, now],
        )?;
    } else {
        // The brief and its time stay as they are; only the head moves.
        conn.execute(
            "INSERT INTO project_memory(project_id,revision,markdown,updated_at) VALUES(?1,?2,'',?3) \
             ON CONFLICT(project_id) DO UPDATE SET revision=excluded.revision",
            params![project, revision, now],
        )?;
    }
    // Keep the newest of this subject only, and everything from the last hour, which the limit on
    // an agent's saves is counted from. The newest is always kept, so the brief's and each note's
    // current revision can always be found.
    conn.execute(
        "DELETE FROM project_memory_revisions WHERE project_id=?1 AND note IS ?2 AND created_at<=?4 AND rowid NOT IN \
         (SELECT rowid FROM project_memory_revisions WHERE project_id=?1 AND note IS ?2 ORDER BY rowid DESC LIMIT ?3)",
        params![project, note, REVISIONS_KEPT, now - WRITE_WINDOW_MS],
    )?;
    Ok((revision, parent))
}

/// A reason is one bounded sentence.
pub(crate) fn check_reason(reason: &str) -> rusqlite::Result<()> {
    if reason.trim().is_empty() || reason.len() > 2048 {
        return Err(invalid(
            "Say what changed and why, in a sentence of at most 2 KiB.",
        ));
    }
    Ok(())
}

/// Memory is text: no control characters but line breaks and tabs.
pub(crate) fn check_text(markdown: &str) -> rusqlite::Result<()> {
    if markdown
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(invalid(
            "Memory is plain Markdown: it cannot hold control characters.",
        ));
    }
    Ok(())
}

/// A note's name: lowercase letters, digits and dashes, starting with a letter or a digit. It is
/// what an agent passes to read the note, and it reaches every chat in the list.
pub(crate) fn check_name(name: &str) -> rusqlite::Result<()> {
    let mut chars = name.chars();
    let first = chars.next();
    let fits = name.len() <= MAX_NAME
        && first.is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !fits {
        return Err(invalid(
            "Name a note like windows-build: lowercase letters, digits and dashes, at most 40 characters.",
        ));
    }
    Ok(())
}

/// A note's description: one line that says when to read it. Every chat of the project receives
/// it, so it is bounded and cannot break out of its line.
pub(crate) fn check_description(description: &str) -> rusqlite::Result<()> {
    let trimmed = description.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_DESCRIPTION
        || trimmed.chars().any(char::is_control)
    {
        return Err(invalid(
            "A note needs a one-line description of at most 120 characters that says when to read it.",
        ));
    }
    Ok(())
}

/// A description as the list shows it: inside quotes, so its own quotes become apostrophes.
fn quoted(description: &str) -> String {
    description
        .trim()
        .chars()
        .map(|c| match c {
            '"' => '\'',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect()
}

fn summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryNoteSummary> {
    let source: String = row.get(3)?;
    let bytes: i64 = row.get(5)?;
    Ok(MemoryNoteSummary {
        name: row.get(0)?,
        description: row.get(1)?,
        revision: row.get(2)?,
        by: by(&source).to_owned(),
        updated_at: row.get(4)?,
        bytes: u32::try_from(bytes).unwrap_or(u32::MAX),
    })
}

/// The project's notes, by name.
pub(crate) fn list(conn: &Connection, project: &str) -> rusqlite::Result<Vec<MemoryNoteSummary>> {
    conn.prepare_cached(
        "SELECT name,description,revision,source,updated_at,octet_length(markdown) \
         FROM project_memory_notes WHERE project_id=?1 ORDER BY name",
    )?
    .query_map([project], summary)?
    .collect()
}

fn find(conn: &Connection, project: &str, name: &str) -> rusqlite::Result<Option<MemoryNote>> {
    conn.query_row(
        "SELECT description,revision,markdown,updated_at,source FROM project_memory_notes \
         WHERE project_id=?1 AND name=?2",
        params![project, name],
        |r| {
            let source: String = r.get(4)?;
            Ok(MemoryNote {
                project_id: project.to_owned(),
                name: name.to_owned(),
                description: r.get(0)?,
                revision: r.get(1)?,
                markdown: r.get(2)?,
                updated_at: r.get(3)?,
                by: by(&source).to_owned(),
            })
        },
    )
    .optional()
}

fn missing(name: &str) -> rusqlite::Error {
    invalid(&format!(
        "There is no note {name}. o7_memory_read lists the notes there are."
    ))
}

/// One note with its text.
pub(crate) fn read(conn: &Connection, project: &str, name: &str) -> rusqlite::Result<MemoryNote> {
    check_name(name)?;
    find(conn, project, name)?.ok_or_else(|| missing(name))
}

/// What a save of a note did.
pub(crate) struct Saved {
    pub(crate) note: MemoryNote,
    /// The revision it added and the head that revision replaced; `None` when the note already
    /// read exactly so and nothing was added.
    pub(crate) added: Option<(String, Option<String>)>,
}

/// Saves a note whole: adds it when `expected_revision` is `None`, replaces it otherwise, and
/// refuses a save over a revision other than the one named. Saving a note exactly as it reads
/// answers it as it is, so a retry that already landed neither fails nor adds a revision.
pub(crate) fn save(
    conn: &Connection,
    input: &SaveMemoryNote,
    now: i64,
    source: &str,
) -> rusqlite::Result<Saved> {
    check_name(&input.name)?;
    check_description(&input.description)?;
    check_text(&input.markdown)?;
    check_reason(&input.reason)?;
    if input.markdown.len() > MAX_NOTE {
        return Err(invalid(
            "A note holds at most 16 KiB. Shorten it, or move part of it to another note.",
        ));
    }
    let description = input.description.trim();
    let name = input.name.as_str();
    let current = find(conn, &input.project_id, name)?;
    match &current {
        Some(note) if note.markdown == input.markdown && note.description == description => {
            return Ok(Saved {
                note: note.clone(),
                added: None,
            });
        }
        Some(note) if input.expected_revision.as_deref() != Some(note.revision.as_str()) => {
            return Err(invalid(&match &input.expected_revision {
                Some(base) => format!(
                    "The note {name} changed since revision {base}: it is now revision {}. Read it again, apply your edit to the current text, and write with the new base revision.",
                    note.revision
                ),
                None => format!(
                    "There is already a note {name}, at revision {}. Read it and write with that base revision, or pick another name.",
                    note.revision
                ),
            }));
        }
        Some(_) => {}
        None if input.expected_revision.is_some() => return Err(missing(name)),
        None => {
            let count: i64 = conn.query_row(
                "SELECT count(*) FROM project_memory_notes WHERE project_id=?1",
                [&input.project_id],
                |r| r.get(0),
            )?;
            if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_NOTES {
                return Err(invalid(
                    "This project already has 32 notes. Add to one, or delete one that is out of date.",
                ));
            }
        }
    }
    let added = add_revision(
        conn,
        &input.project_id,
        Some(name),
        Some(description),
        &input.markdown,
        false,
        &input.reason,
        source,
        now,
    )?;
    put(
        conn,
        &input.project_id,
        name,
        description,
        &added.0,
        &input.markdown,
        source,
        now,
    )?;
    let note = find(conn, &input.project_id, name)?.ok_or_else(|| missing(name))?;
    Ok(Saved {
        note,
        added: Some(added),
    })
}

#[allow(clippy::too_many_arguments)]
fn put(
    conn: &Connection,
    project: &str,
    name: &str,
    description: &str,
    revision: &str,
    markdown: &str,
    source: &str,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO project_memory_notes(project_id,name,description,revision,markdown,source,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7) \
         ON CONFLICT(project_id,name) DO UPDATE SET description=excluded.description,revision=excluded.revision,markdown=excluded.markdown,source=excluded.source,updated_at=excluded.updated_at",
        params![project, name, description, revision, markdown, source, now],
    )?;
    Ok(())
}

/// Removes a note, when it is still at the revision named. Returns the revision that records the
/// removal and the head it replaced.
pub(crate) fn delete(
    conn: &Connection,
    input: &DeleteMemoryNote,
    now: i64,
    source: &str,
) -> rusqlite::Result<(String, Option<String>)> {
    check_name(&input.name)?;
    check_reason(&input.reason)?;
    let name = input.name.as_str();
    let note = find(conn, &input.project_id, name)?.ok_or_else(|| missing(name))?;
    if input.expected_revision.as_deref() != Some(note.revision.as_str()) {
        return Err(invalid(&format!(
            "The note {name} changed since revision {}: it is now revision {}. Read it again before you delete it.",
            input.expected_revision.as_deref().unwrap_or("none"),
            note.revision
        )));
    }
    let added = add_revision(
        conn,
        &input.project_id,
        Some(name),
        Some(&note.description),
        "",
        true,
        &input.reason,
        source,
        now,
    )?;
    conn.execute(
        "DELETE FROM project_memory_notes WHERE project_id=?1 AND name=?2",
        params![input.project_id, name],
    )?;
    Ok(added)
}

/// The source an agent's write in `chat` is saved under, when that chat belongs to `project` and
/// its agent has not saved too often.
pub(crate) fn agent_source(
    conn: &Connection,
    chat: &str,
    project: &str,
    now: i64,
) -> rusqlite::Result<String> {
    let belongs: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1 AND project_id=?2 AND deleted=0)",
        params![chat, project],
        |r| r.get(0),
    )?;
    // The source must stay within what a revision may name.
    if chat.is_empty() || AGENT_SOURCE.len() + chat.len() > MAX_SOURCE || !belongs {
        return Err(invalid(
            "This chat does not belong to the memory's project.",
        ));
    }
    not_too_often(conn, chat, now)?;
    Ok(format!("{AGENT_SOURCE}{chat}"))
}

/// Refuses an agent's write once its chat has saved memory as often as it may in an hour: a loop
/// that rewrites memory must not run on unseen.
pub(crate) fn not_too_often(conn: &Connection, chat: &str, now: i64) -> rusqlite::Result<()> {
    let recent: i64 = conn.query_row(
        "SELECT count(*) FROM project_memory_revisions WHERE source=?1 AND created_at>?2",
        params![format!("{AGENT_SOURCE}{chat}"), now - WRITE_WINDOW_MS],
        |r| r.get(0),
    )?;
    if recent >= AGENT_WRITES_AN_HOUR {
        return Err(invalid(
            "This chat has saved project memory 20 times in the last hour, which is the most it may. Tell the user what you would have saved instead.",
        ));
    }
    Ok(())
}

/// Takes back a run of edits to the brief or to one note, as the user's own save: `first` is the
/// earliest of them and `last` the latest (the same revision for a single edit). What the brief
/// or the note held before `first` comes back: its earlier text, a note the run removed, or
/// nothing where the run added the note. Refused once the brief or the note has moved on from
/// `last`, so an undo never takes a later edit with it.
pub(crate) fn undo(
    conn: &Connection,
    project: &str,
    first: &str,
    last: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let moved = || {
        invalid(
            "Memory changed after that edit, so it cannot be undone. Open Project memory to change it.",
        )
    };
    let find_edit = |revision: &str| -> rusqlite::Result<Option<(i64, Option<String>, bool)>> {
        conn.query_row(
            "SELECT rowid,note,deleted FROM project_memory_revisions WHERE revision=?1 AND project_id=?2",
            params![revision, project],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
    };
    let (Some((rowid, note, _)), Some((last_rowid, last_note, deleted))) =
        (find_edit(first)?, find_edit(last)?)
    else {
        return Err(moved());
    };
    if note != last_note || last_rowid < rowid {
        return Err(moved());
    }
    let before: Option<(Option<String>, String, bool)> = conn
        .query_row(
            "SELECT description,markdown,deleted FROM project_memory_revisions \
             WHERE project_id=?1 AND note IS ?2 AND rowid<?3 ORDER BY rowid DESC LIMIT 1",
            params![project, note, rowid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    const REASON: &str = "Undid an agent's edit";
    let Some(name) = note.as_deref() else {
        if brief_revision(conn, project)?.as_deref() != Some(last) {
            return Err(moved());
        }
        let markdown = before.map(|(_, markdown, _)| markdown).unwrap_or_default();
        add_revision(
            conn,
            project,
            None,
            None,
            &markdown,
            false,
            REASON,
            USER_SOURCE,
            now,
        )?;
        return Ok(());
    };
    let current = find(conn, project, name)?;
    let untouched = match &current {
        Some(note) => !deleted && note.revision == last,
        None => deleted,
    };
    if !untouched {
        return Err(moved());
    }
    match before {
        // It had this text before: put that back, whether the run changed or removed it.
        Some((description, markdown, false)) => {
            let description = description.unwrap_or_default();
            let (added, _) = add_revision(
                conn,
                project,
                Some(name),
                Some(&description),
                &markdown,
                false,
                REASON,
                USER_SOURCE,
                now,
            )?;
            put(
                conn,
                project,
                name,
                &description,
                &added,
                &markdown,
                USER_SOURCE,
                now,
            )?;
        }
        // The run added it (or added it back): it goes again.
        _ => {
            if let Some(note) = current {
                add_revision(
                    conn,
                    project,
                    Some(name),
                    Some(&note.description),
                    "",
                    true,
                    REASON,
                    USER_SOURCE,
                    now,
                )?;
                conn.execute(
                    "DELETE FROM project_memory_notes WHERE project_id=?1 AND name=?2",
                    params![project, name],
                )?;
            }
        }
    }
    Ok(())
}

/// How old a note is, in words that stay true when the message that carried them is read later:
/// its age when it was sent.
fn age(updated_at: i64, now: i64) -> String {
    match (now - updated_at).max(0) / 86_400_000 {
        0 => "today".to_owned(),
        1 => "1 day old".to_owned(),
        days => format!("{days} days old"),
    }
}

/// The list of notes a chat receives: one line a note, with who saved it last and how old it is,
/// so an agent can weigh an old note against the code before it trusts it.
pub(crate) fn index(notes: &[MemoryNoteSummary], now: i64) -> String {
    notes
        .iter()
        .map(|note| {
            format!(
                "- {} | {} | {} | \"{}\"",
                note.name,
                age(note.updated_at, now),
                note.by,
                quoted(&note.description)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What changed in a project's memory since a chat last received it, leaving out what the chat's
/// own agent saved.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    /// Someone else saved the brief.
    pub(crate) brief: bool,
    /// Notes someone else saved or removed, newest change first: name, who, and whether it went.
    pub(crate) notes: Vec<(String, &'static str, bool)>,
}

impl Changes {
    pub(crate) fn is_empty(&self) -> bool {
        !self.brief && self.notes.is_empty()
    }

    /// The notes that changed, as one line for the chat.
    pub(crate) fn line(&self) -> String {
        let notes = self
            .notes
            .iter()
            .map(|(name, by, deleted)| {
                if *deleted {
                    format!("{name} (deleted by {})", who(by))
                } else {
                    format!("{name} (saved by {})", who(by))
                }
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "Notes changed since your last message: {notes}. Read one with o7_memory_read when it matters to your task."
        )
    }
}

fn who(by: &str) -> &'static str {
    match by {
        "agent" => "an agent",
        "bot" => "a linked bot",
        _ => "the user",
    }
}

/// The changes from `previous` (the head a chat last received) up to `head`. `None` when they
/// cannot be told: the chat received nothing yet, or the way back is longer than is followed or
/// was pruned. The chat is then sent everything.
pub(crate) fn changes_since(
    conn: &Connection,
    head: Option<&str>,
    previous: Option<&str>,
    reader: &str,
) -> rusqlite::Result<Option<Changes>> {
    let Some(previous) = previous else {
        return Ok(None);
    };
    let own = format!("{AGENT_SOURCE}{reader}");
    let mut changes = Changes::default();
    let mut at = head.map(str::to_owned);
    for _ in 0..CHANGES_FOLLOWED {
        let Some(revision) = at else {
            return Ok(None);
        };
        if revision == previous {
            return Ok(Some(changes));
        }
        let row: Option<(Option<String>, String, Option<String>, bool)> = conn
            .query_row(
                "SELECT parent_revision,source,note,deleted FROM project_memory_revisions WHERE revision=?1",
                [&revision],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((parent, source, note, deleted)) = row else {
            return Ok(None);
        };
        if source != own {
            match note {
                None => changes.brief = true,
                // The newest change to a note is the one that says how it stands.
                Some(name) if !changes.notes.iter().any(|(known, _, _)| *known == name) => {
                    changes.notes.push((name, by(&source), deleted));
                }
                Some(_) => {}
            }
        }
        at = parent;
    }
    Ok(None)
}

/// A name for a note made from a heading: its letters and digits in lowercase, dashes between.
fn slug(heading: &str) -> String {
    let mut name = String::new();
    for c in heading.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            name.push(c);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
        if name.len() >= MAX_NAME {
            break;
        }
    }
    let name = name.trim_matches('-').to_owned();
    if name.is_empty() {
        "note".to_owned()
    } else {
        name
    }
}

/// A heading as a note's description: one line of at most [`MAX_DESCRIPTION`] characters.
fn heading_description(heading: &str) -> String {
    let line: String = heading
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let line: String = line.chars().take(MAX_DESCRIPTION).collect();
    if line.is_empty() {
        "Notes".to_owned()
    } else {
        line
    }
}

/// A document split into a brief and notes: the brief's text, then each note's name, description
/// and text.
pub(crate) type Split = (String, Vec<(String, String, String)>);

/// Splits a document that is too long for a brief at its `## ` headings, without a model and
/// without losing a character: the text before the first heading and the sections after it stay
/// the brief for as long as they fit in [`MAX_BRIEF`], and from the first section that does not
/// fit, every section becomes a note named for its heading. A document usually opens with what
/// matters most, and that is what every chat keeps receiving. `None` when the document fits as
/// it is, or has no heading to split at.
pub(crate) fn split(markdown: &str) -> Option<Split> {
    if markdown.len() <= MAX_BRIEF {
        return None;
    }
    // Where each section starts: a line that begins "## ", outside a code fence.
    let mut starts = Vec::new();
    let mut fenced = false;
    let mut offset = 0;
    for line in markdown.split_inclusive('\n') {
        let text = line.trim_end();
        if text.starts_with("```") || text.starts_with("~~~") {
            fenced = !fenced;
        } else if !fenced && text.starts_with("## ") {
            starts.push(offset);
        }
        offset += line.len();
    }
    if starts.is_empty() {
        return None;
    }
    let mut bounds = starts.clone();
    bounds.push(markdown.len());
    // The sections that stay in the brief: the leading ones, while the brief still fits.
    let mut kept = 0;
    while kept < starts.len() && bounds[kept + 1] <= MAX_BRIEF {
        kept += 1;
    }
    let brief = markdown[..bounds[kept]].to_owned();
    let mut notes: Vec<(String, String, String)> = Vec::new();
    for at in kept..starts.len() {
        let section = &markdown[bounds[at]..bounds[at + 1]];
        let heading = section
            .lines()
            .next()
            .unwrap_or_default()
            .trim_start_matches('#')
            .trim();
        // Past the most notes a project has, the rest joins the last note.
        if notes.len() == MAX_NOTES {
            if let Some(last) = notes.last_mut() {
                last.2.push_str(section);
            }
            continue;
        }
        let base = slug(heading);
        let mut name = base.clone();
        let mut copy = 2;
        while notes.iter().any(|(known, _, _)| *known == name) {
            let suffix = format!("-{copy}");
            let room = MAX_NAME.saturating_sub(suffix.len());
            name = format!("{}{suffix}", &base[..base.len().min(room)]);
            copy += 1;
        }
        notes.push((name, heading_description(heading), section.to_owned()));
    }
    if notes.is_empty() {
        return None;
    }
    Some((brief, notes))
}

/// Migration 22: every project whose memory is too long for a brief has it split into a brief and
/// notes ([`split`]). The notes keep the source of the document's last save, so what an agent
/// wrote still reads as an agent's.
pub(crate) fn split_all(conn: &Connection, now: i64) -> rusqlite::Result<u32> {
    let long: Vec<(String, String)> = conn
        .prepare("SELECT project_id,markdown FROM project_memory WHERE octet_length(markdown)>?1")?
        .query_map([i64::try_from(MAX_BRIEF).unwrap_or(i64::MAX)], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut done = 0;
    for (project, markdown) in long {
        let Some((brief, notes)) = split(&markdown) else {
            continue;
        };
        let source: String = conn
            .query_row(
                "SELECT source FROM project_memory_revisions WHERE project_id=?1 AND note IS NULL \
                 ORDER BY rowid DESC LIMIT 1",
                [&project],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| USER_SOURCE.to_owned());
        const REASON: &str = "Split into a brief and notes when o7 was updated";
        for (name, description, text) in &notes {
            let (revision, _) = add_revision(
                conn,
                &project,
                Some(name),
                Some(description),
                text,
                false,
                REASON,
                &source,
                now,
            )?;
            put(
                conn,
                &project,
                name,
                description,
                &revision,
                text,
                &source,
                now,
            )?;
        }
        add_revision(
            conn, &project, None, None, &brief, false, REASON, &source, now,
        )?;
        done += 1;
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatstore::test_support::TestDb;

    fn fixture() -> (TestDb, String, String) {
        let db = TestDb::new();
        let chat = db.chat("/notes");
        let project = db
            .conn()
            .query_row("SELECT project_id FROM chats WHERE id=?1", [&chat], |r| {
                r.get(0)
            })
            .unwrap();
        (db, project, chat)
    }

    fn note(project: &str, name: &str, base: Option<&str>, text: &str) -> SaveMemoryNote {
        SaveMemoryNote {
            project_id: project.into(),
            name: name.into(),
            expected_revision: base.map(str::to_owned),
            description: format!("Read for {name}"),
            markdown: text.into(),
            reason: "Learned it".into(),
        }
    }

    fn refusal(result: rusqlite::Result<impl Sized>) -> String {
        match result {
            Err(rusqlite::Error::ToSqlConversionFailure(message)) => message.to_string(),
            Err(other) => panic!("another error: {other}"),
            Ok(_) => panic!("it was allowed"),
        }
    }

    #[test]
    fn a_note_is_added_replaced_and_removed_only_from_the_revision_that_was_read() {
        let (db, project, _) = fixture();
        let conn = db.conn();
        let added = save(
            conn,
            &note(&project, "windows-build", None, "Use tb2."),
            10,
            USER_SOURCE,
        )
        .unwrap();
        assert_eq!(added.note.by, "user");
        assert_eq!(added.note.description, "Read for windows-build");
        let first = added.note.revision.clone();
        assert_eq!(
            head(conn, &project).unwrap().as_deref(),
            Some(first.as_str())
        );
        // Adding it again is refused: the caller has not read what is there.
        assert!(
            refusal(save(
                conn,
                &note(&project, "windows-build", None, "Other"),
                11,
                USER_SOURCE
            ))
            .starts_with("There is already a note windows-build, at revision ")
        );
        // Saving it exactly as it reads adds nothing, whatever base is named.
        let same = save(
            conn,
            &note(&project, "windows-build", None, "Use tb2."),
            12,
            USER_SOURCE,
        )
        .unwrap();
        assert!(same.added.is_none());
        assert_eq!(same.note.revision, first);
        let replaced = save(
            conn,
            &note(
                &project,
                "windows-build",
                Some(&first),
                "Use tb2. Never cargo.",
            ),
            13,
            "agent:chat",
        )
        .unwrap();
        assert_eq!(replaced.note.by, "agent");
        let second = replaced.note.revision.clone();
        assert_eq!(replaced.added, Some((second.clone(), Some(first.clone()))));
        // A save over the older revision is refused, and says what to do.
        let stale = refusal(save(
            conn,
            &note(&project, "windows-build", Some(&first), "Mine"),
            14,
            USER_SOURCE,
        ));
        assert!(
            stale.contains(&format!("changed since revision {first}")),
            "{stale}"
        );
        assert!(stale.contains("Read it again"));
        // A note that is not there cannot be replaced, read or removed.
        for missing in [
            refusal(save(
                conn,
                &note(&project, "gone", Some("r"), "x"),
                15,
                USER_SOURCE,
            )),
            refusal(read(conn, &project, "gone")),
            refusal(delete(
                conn,
                &DeleteMemoryNote {
                    project_id: project.clone(),
                    name: "gone".into(),
                    expected_revision: Some("r".into()),
                    reason: "Out of date".into(),
                },
                15,
                USER_SOURCE,
            )),
        ] {
            assert_eq!(
                missing,
                "There is no note gone. o7_memory_read lists the notes there are."
            );
        }
        let remove = |base: &str| DeleteMemoryNote {
            project_id: project.clone(),
            name: "windows-build".into(),
            expected_revision: Some(base.into()),
            reason: "Out of date".into(),
        };
        assert!(refusal(delete(conn, &remove(&first), 16, USER_SOURCE)).contains("Read it again"));
        let (removal, before) = delete(conn, &remove(&second), 17, USER_SOURCE).unwrap();
        assert_eq!(before.as_deref(), Some(second.as_str()));
        assert_eq!(
            head(conn, &project).unwrap().as_deref(),
            Some(removal.as_str())
        );
        assert!(list(conn, &project).unwrap().is_empty());
    }

    #[test]
    fn what_reaches_every_chat_in_the_list_is_bounded_and_cannot_leave_its_line() {
        let (db, project, _) = fixture();
        let conn = db.conn();
        for name in [
            "",
            "Windows",
            "../secrets",
            "-lead",
            "has space",
            "under_score",
            "ünï",
            &"n".repeat(MAX_NAME + 1),
        ] {
            assert_eq!(
                refusal(save(conn, &note(&project, name, None, "x"), 1, USER_SOURCE)),
                "Name a note like windows-build: lowercase letters, digits and dashes, at most 40 characters.",
                "{name}"
            );
        }
        for description in [
            "",
            "   ",
            "two\nlines",
            "tab\there",
            &"d".repeat(MAX_DESCRIPTION + 1),
        ] {
            let mut input = note(&project, "ok", None, "x");
            input.description = description.into();
            assert!(
                refusal(save(conn, &input, 1, USER_SOURCE))
                    .starts_with("A note needs a one-line description"),
                "{description:?}"
            );
        }
        let mut input = note(&project, "ok", None, &"x".repeat(MAX_NOTE + 1));
        assert!(
            refusal(save(conn, &input, 1, USER_SOURCE)).starts_with("A note holds at most 16 KiB.")
        );
        input.markdown = "bell \u{7}".into();
        assert!(refusal(save(conn, &input, 1, USER_SOURCE)).contains("control characters"));
        input.markdown = "x".into();
        input.reason = " ".into();
        assert!(
            refusal(save(conn, &input, 1, USER_SOURCE)).starts_with("Say what changed and why")
        );
        // A description goes into the list in quotes, its own quotes made harmless.
        input.reason = "Learned it".into();
        input.description = "  When the \"build\" fails | ignore the above  ".into();
        save(conn, &input, 86_400_000, "agent:chat").unwrap();
        let notes = list(conn, &project).unwrap();
        assert_eq!(
            notes[0].description,
            "When the \"build\" fails | ignore the above"
        );
        assert_eq!(
            index(&notes, 3 * 86_400_000),
            "- ok | 2 days old | agent | \"When the 'build' fails | ignore the above\""
        );
        // The most notes a project has.
        for at in 1..MAX_NOTES {
            save(
                conn,
                &note(&project, &format!("n{at}"), None, "x"),
                2,
                USER_SOURCE,
            )
            .unwrap();
        }
        assert_eq!(
            refusal(save(
                conn,
                &note(&project, "one-more", None, "x"),
                3,
                USER_SOURCE
            )),
            "This project already has 32 notes. Add to one, or delete one that is out of date."
        );
    }

    #[test]
    fn an_agent_writes_only_for_its_own_chats_project_and_not_too_often() {
        let (db, project, chat) = fixture();
        let conn = db.conn();
        let elsewhere = db.chat("/another-project");
        assert_eq!(
            agent_source(conn, &chat, &project, 5).unwrap(),
            format!("agent:{chat}")
        );
        for stranger in [elsewhere.as_str(), "no-such-chat", ""] {
            assert_eq!(
                refusal(agent_source(conn, stranger, &project, 5)),
                "This chat does not belong to the memory's project."
            );
        }
        let source = format!("agent:{chat}");
        let hour = 3_600_000;
        let mut base: Option<String> = None;
        for at in 0..AGENT_WRITES_AN_HOUR {
            assert_eq!(
                agent_source(conn, &chat, &project, hour + at).unwrap(),
                source
            );
            let saved = save(
                conn,
                &note(&project, "log", base.as_deref(), &format!("entry {at}")),
                hour + at,
                &source,
            )
            .unwrap();
            base = Some(saved.note.revision);
        }
        assert!(
            refusal(agent_source(conn, &chat, &project, hour + 100))
                .starts_with("This chat has saved project memory 20 times in the last hour")
        );
        // Other writers saving the same note do not push this chat's saves out of its count:
        // nothing younger than the hour is trimmed, however many revisions the note gathers.
        for at in 0..2 * REVISIONS_KEPT {
            let saved = save(
                conn,
                &note(&project, "log", base.as_deref(), &format!("theirs {at}")),
                hour + 200 + at,
                USER_SOURCE,
            )
            .unwrap();
            base = Some(saved.note.revision);
        }
        assert!(
            refusal(agent_source(conn, &chat, &project, hour + 500))
                .starts_with("This chat has saved project memory 20 times in the last hour")
        );
        let count = || -> i64 {
            conn.query_row(
                "SELECT count(*) FROM project_memory_revisions WHERE project_id=?1 AND note='log'",
                [&project],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(count(), AGENT_WRITES_AN_HOUR + 2 * REVISIONS_KEPT);
        // An hour on, it may again; and the next save trims the note to its newest revisions.
        assert!(agent_source(conn, &chat, &project, 2 * hour + 1000).is_ok());
        save(
            conn,
            &note(&project, "log", base.as_deref(), "an hour later"),
            2 * hour + 1000,
            &source,
        )
        .unwrap();
        assert_eq!(count(), REVISIONS_KEPT);
    }

    #[test]
    fn undo_puts_back_what_one_edit_replaced_and_never_takes_a_later_edit_with_it() {
        let (db, project, chat) = fixture();
        let conn = db.conn();
        let agent = format!("agent:{chat}");
        // An agent adds a note: undoing removes it.
        let added = save(conn, &note(&project, "release", None, "Tag it."), 1, &agent).unwrap();
        undo(
            conn,
            &project,
            &added.note.revision,
            &added.note.revision,
            2,
        )
        .unwrap();
        assert!(list(conn, &project).unwrap().is_empty());
        // The user writes it, an agent changes it: undoing puts the user's text back, as the user's.
        let mine = save(
            conn,
            &note(&project, "release", None, "Tag, then publish."),
            3,
            USER_SOURCE,
        )
        .unwrap();
        let theirs = save(
            conn,
            &note(
                &project,
                "release",
                Some(&mine.note.revision),
                "Publish only.",
            ),
            4,
            &agent,
        )
        .unwrap();
        undo(
            conn,
            &project,
            &theirs.note.revision,
            &theirs.note.revision,
            5,
        )
        .unwrap();
        let back = read(conn, &project, "release").unwrap();
        assert_eq!(
            (back.markdown.as_str(), back.by.as_str()),
            ("Tag, then publish.", "user")
        );
        // An agent removes it: undoing brings it back.
        let (removal, _) = delete(
            conn,
            &DeleteMemoryNote {
                project_id: project.clone(),
                name: "release".into(),
                expected_revision: Some(back.revision.clone()),
                reason: "Out of date".into(),
            },
            6,
            &agent,
        )
        .unwrap();
        undo(conn, &project, &removal, &removal, 7).unwrap();
        let again = read(conn, &project, "release").unwrap();
        assert_eq!(again.markdown, "Tag, then publish.");
        // Once the note has moved on, the older edit cannot be undone, and neither can a
        // revision of another project or one that is not there.
        let later = save(
            conn,
            &note(
                &project,
                "release",
                Some(&again.revision),
                "Tag, publish, announce.",
            ),
            8,
            &agent,
        )
        .unwrap();
        save(
            conn,
            &note(
                &project,
                "release",
                Some(&later.note.revision),
                "Newer still.",
            ),
            9,
            USER_SOURCE,
        )
        .unwrap();
        for revision in [later.note.revision.as_str(), "no-such-revision"] {
            assert!(
                refusal(undo(conn, &project, revision, revision, 10))
                    .starts_with("Memory changed after that edit")
            );
        }
        assert!(
            refusal(undo(
                conn,
                "another-project",
                &later.note.revision,
                &later.note.revision,
                10
            ))
            .starts_with("Memory changed")
        );
        assert_eq!(
            read(conn, &project, "release").unwrap().markdown,
            "Newer still."
        );
        // The brief: undone to what it held before, or to nothing when the edit wrote the first.
        let (brief, _) = add_revision(
            conn,
            &project,
            None,
            None,
            "Agent's brief",
            false,
            "r",
            &agent,
            11,
        )
        .unwrap();
        undo(conn, &project, &brief, &brief, 12).unwrap();
        // A run of edits to one note goes back as one: to what it held before the first of them,
        // and only while the note is still at the last.
        let kept = read(conn, &project, "release").unwrap();
        let one = save(
            conn,
            &note(&project, "release", Some(&kept.revision), "First change."),
            20,
            &agent,
        )
        .unwrap();
        let two = save(
            conn,
            &note(
                &project,
                "release",
                Some(&one.note.revision),
                "Second change.",
            ),
            21,
            &agent,
        )
        .unwrap();
        assert!(
            refusal(undo(
                conn,
                &project,
                &one.note.revision,
                &one.note.revision,
                22
            ))
            .starts_with("Memory changed after that edit"),
            "the first alone is no longer where the note stands"
        );
        assert!(
            refusal(undo(
                conn,
                &project,
                &two.note.revision,
                &one.note.revision,
                22
            ))
            .starts_with("Memory changed"),
            "a run is named first to last"
        );
        assert!(
            refusal(undo(conn, &project, &brief, &two.note.revision, 22))
                .starts_with("Memory changed"),
            "a run is of one subject"
        );
        undo(conn, &project, &one.note.revision, &two.note.revision, 23).unwrap();
        assert_eq!(
            read(conn, &project, "release").unwrap().markdown,
            kept.markdown
        );
        let text: String = conn
            .query_row(
                "SELECT markdown FROM project_memory WHERE project_id=?1",
                [&project],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(text, "");
    }

    #[test]
    fn a_chat_hears_what_others_changed_since_it_last_received_memory() {
        let (db, project, chat) = fixture();
        let conn = db.conn();
        let own = format!("agent:{chat}");
        let (start, _) = add_revision(
            conn,
            &project,
            None,
            None,
            "Brief",
            false,
            "r",
            USER_SOURCE,
            1,
        )
        .unwrap();
        assert_eq!(
            changes_since(conn, Some(&start), None, &chat).unwrap(),
            None,
            "nothing received yet"
        );
        assert_eq!(
            changes_since(conn, Some(&start), Some(&start), &chat).unwrap(),
            Some(Changes::default())
        );
        // Its own saves are not news to it.
        let mine = save(conn, &note(&project, "mine", None, "x"), 2, &own).unwrap();
        let head_now = head(conn, &project).unwrap();
        let quiet = changes_since(conn, head_now.as_deref(), Some(&start), &chat)
            .unwrap()
            .unwrap();
        assert!(quiet.is_empty());
        // Others' are: a note saved twice counts once, as it stands; a removed note says so.
        let other = save(conn, &note(&project, "build", None, "a"), 3, "agent:other").unwrap();
        save(
            conn,
            &note(&project, "build", Some(&other.note.revision), "b"),
            4,
            USER_SOURCE,
        )
        .unwrap();
        delete(
            conn,
            &DeleteMemoryNote {
                project_id: project.clone(),
                name: "mine".into(),
                expected_revision: Some(mine.note.revision.clone()),
                reason: "Out of date".into(),
            },
            5,
            BOT_SOURCE,
        )
        .unwrap();
        let head_now = head(conn, &project).unwrap();
        let heard = changes_since(conn, head_now.as_deref(), Some(&start), &chat)
            .unwrap()
            .unwrap();
        assert!(!heard.brief);
        assert_eq!(
            heard.notes,
            [
                ("mine".to_owned(), "bot", true),
                ("build".to_owned(), "user", false)
            ]
        );
        assert_eq!(
            heard.line(),
            "Notes changed since your last message: mine (deleted by a linked bot); build (saved by the user). Read one with o7_memory_read when it matters to your task."
        );
        // The brief changing is told apart, and a revision that is not on the way back is not
        // guessed at.
        add_revision(
            conn,
            &project,
            None,
            None,
            "New brief",
            false,
            "r",
            USER_SOURCE,
            6,
        )
        .unwrap();
        let head_now = head(conn, &project).unwrap();
        assert!(
            changes_since(conn, head_now.as_deref(), Some(&start), &chat)
                .unwrap()
                .unwrap()
                .brief
        );
        assert_eq!(
            changes_since(conn, head_now.as_deref(), Some("never-a-revision"), &chat).unwrap(),
            None
        );
    }

    #[test]
    fn a_long_document_splits_at_its_headings_without_losing_a_character() {
        assert_eq!(split("short"), None);
        assert_eq!(
            split(&"no headings ".repeat(600)),
            None,
            "nothing to split at"
        );
        let section =
            |title: &str, bytes: usize| format!("## {title}\n\n{}\n\n", "w".repeat(bytes));
        let document = format!(
            "# Project memory\n\nRead this first.\n\n{}{}{}```\n## not a heading\n```\n\n{}{}",
            section("Security comes first", 1200),
            section("Dates", 900),
            section("Mobile: a phone beside the chat", 2500),
            section("Mobile: a phone beside the chat", 300),
            section("???", 100),
        );
        let (brief, notes) = split(&document).expect("split");
        // What comes first stays in the brief, as far as it fits.
        assert!(
            brief.starts_with("# Project memory\n\nRead this first.\n\n## Security comes first")
        );
        assert!(brief.contains("## Dates"));
        assert!(!brief.contains("## Mobile"));
        assert!(brief.len() <= MAX_BRIEF);
        let names: Vec<&str> = notes.iter().map(|(name, _, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "mobile-a-phone-beside-the-chat",
                "mobile-a-phone-beside-the-chat-2",
                "note"
            ]
        );
        assert_eq!(notes[0].1, "Mobile: a phone beside the chat");
        assert!(
            notes[0].2.contains("## not a heading"),
            "a fence keeps its lines"
        );
        for (name, description, _) in &notes {
            check_name(name).unwrap();
            check_description(description).unwrap();
        }
        let whole: String = std::iter::once(brief.as_str())
            .chain(notes.iter().map(|(_, _, text)| text.as_str()))
            .collect();
        assert_eq!(whole, document);
    }

    #[test]
    fn splitting_every_long_memory_keeps_who_wrote_it_and_leaves_short_ones_alone() {
        let (db, project, chat) = fixture();
        let conn = db.conn();
        let short = db.chat("/short-project");
        let short_project: String = conn
            .query_row("SELECT project_id FROM chats WHERE id=?1", [&short], |r| {
                r.get(0)
            })
            .unwrap();
        add_revision(
            conn,
            &short_project,
            None,
            None,
            "Short",
            false,
            "r",
            USER_SOURCE,
            1,
        )
        .unwrap();
        let long = format!(
            "# Memory\n\n## Rules\n\n{}\n\n## Build\n\n{}\n\n## Release\n\n{}\n",
            "r".repeat(1000),
            "b".repeat(4000),
            "p".repeat(3000)
        );
        add_revision(
            conn,
            &project,
            None,
            None,
            &long,
            false,
            "r",
            &format!("agent:{chat}"),
            2,
        )
        .unwrap();
        assert_eq!(split_all(conn, 9).unwrap(), 1);
        assert_eq!(split_all(conn, 10).unwrap(), 0, "once split, it is short");
        let brief: String = conn
            .query_row(
                "SELECT markdown FROM project_memory WHERE project_id=?1",
                [&project],
                |r| r.get(0),
            )
            .unwrap();
        assert!(brief.ends_with(&format!("{}\n\n", "r".repeat(1000))));
        let notes = list(conn, &project).unwrap();
        assert_eq!(
            notes
                .iter()
                .map(|note| (note.name.as_str(), note.by.as_str()))
                .collect::<Vec<_>>(),
            [("build", "agent"), ("release", "agent")]
        );
        assert_eq!(
            read(conn, &project, "build").unwrap().markdown,
            format!("## Build\n\n{}\n\n", "b".repeat(4000))
        );
        // The head is the brief's new revision, and the brief's own revision with it.
        assert_eq!(
            head(conn, &project).unwrap(),
            brief_revision(conn, &project).unwrap()
        );
        let untouched: String = conn
            .query_row(
                "SELECT markdown FROM project_memory WHERE project_id=?1",
                [&short_project],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(untouched, "Short");
        assert!(list(conn, &short_project).unwrap().is_empty());
    }
}
