//! A terminal chat's conversation, read back from its agent's own session record
//! (docs/superpowers/plans/2026-11-06-terminal-chats.md, decision 4 and "Every agent"): Claude
//! Code's `<config>/projects/<folder>/<session>.jsonl`, Codex's
//! `<home>/sessions/**/rollout-*-<thread>.jsonl`, Grok Build's
//! `<home>/sessions/<folder>/<id>/chat_history.jsonl`, Pi's and oh-my-pi's
//! `<agent dir>/sessions/<folder>/<time>_<id>.jsonl`, and Hermes' and OpenCode's through their own
//! export commands.
//! Only what the terminal added is read (from the file's length when it opened), as turns of a
//! prompt and the agent's answer text; tool calls, thinking, injected context and lines that do not
//! parse are left out and counted. Reads are bounded; nothing here writes.
//!
//! Each turn also keeps the line edits its agent's edit tools made, for the Dashboard's AI line
//! edits (`chatstore::line_edits`), counted as Chat counts them: Claude Code's edits that got a
//! result without an error, Codex's completed file changes, Pi's and oh-my-pi's edits whose result
//! is not an error, and Cursor's, Hermes' and OpenCode's edit tools (Cursor and Hermes record no
//! result to check). Grok Build's history records no tool calls, and Antigravity's is not read.
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use std::collections::HashMap;

use ghost_protocol::Millis;
use serde_json::Value;

use crate::chatstore::line_edits::{edit_tool, input_lines, line_count, patch_lines};

/// The most a terminal's session can add that one import reads.
pub(crate) const READ_LIMIT: u64 = 64 * 1024 * 1024;
/// Folders a session file is looked for in, at most.
const DIRS_LIMIT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Vendor {
    Claude,
    Codex,
    Grok,
    /// Pi and oh-my-pi.
    Pi,
    /// Read through `hermes sessions export`.
    Hermes,
    /// Read through `opencode export`.
    OpenCode,
    /// Its conversations are binary: a new one's ID is learned, its turns are not read.
    Antigravity,
    /// Cursor: `<home>/.cursor/projects/<folder>/agent-transcripts/<id>/<id>.jsonl`.
    Cursor,
}

/// One exchange from the terminal: what the person typed, and the agent's answer text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportedTurn {
    pub prompt: String,
    pub answer: String,
    pub started_at: Millis,
    pub ended_at: Millis,
    /// The lines each completed edit changed, with when it was made (0 when the record does not
    /// say: the turn's start stands in).
    pub edits: Vec<(Millis, u64)>,
}

/// The lines an edit call's input changes, by its tool's name: 0 for a tool that does not edit.
fn tool_edit(name: &str, input: &Value) -> u64 {
    if edit_tool(name) {
        input_lines(input)
    } else {
        0
    }
}

/// The lines Codex's file changes made: a hunk per updated file, a new or deleted file's text.
fn codex_changes(changes: &Value) -> u64 {
    let list: Vec<&Value> = match changes {
        Value::Object(map) => map.values().collect(),
        Value::Array(list) => list.iter().collect(),
        _ => Vec::new(),
    };
    list.into_iter()
        .map(|change| match change["type"].as_str() {
            Some("update") => patch_lines(change["unified_diff"].as_str().unwrap_or("")),
            _ => line_count(change["content"].as_str().unwrap_or("")),
        })
        .fold(0, u64::saturating_add)
}

/// The turns read, and how many lines were left out because they did not parse.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Imported {
    pub turns: Vec<ImportedTurn>,
    pub skipped: usize,
    /// The read stopped at `READ_LIMIT`; the rest of the session stays in the agent's file.
    pub truncated: bool,
}

/// Claude Code's session file for `session_id` under its config folder (an added account's
/// `CLAUDE_CONFIG_DIR`, or `~/.claude`): found by name in any project folder, since the folder's
/// name is Claude Code's own spelling of the path.
pub(crate) fn claude_session(config: &Path, session_id: &str) -> Option<PathBuf> {
    if !plain_id(session_id) {
        return None;
    }
    let name = format!("{session_id}.jsonl");
    std::fs::read_dir(config.join("projects"))
        .ok()?
        .flatten()
        .take(DIRS_LIMIT)
        .map(|entry| entry.path().join(&name))
        .find(|path| path.is_file())
}

/// Codex's rollout for `thread_id` under its home (`CODEX_HOME`, or `~/.codex`).
pub(crate) fn codex_session(home: &Path, thread_id: &str) -> Option<PathBuf> {
    if !plain_id(thread_id) {
        return None;
    }
    let suffix = format!("-{thread_id}.jsonl");
    rollouts(home).into_iter().find(|path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(&suffix))
    })
}

/// A new Codex terminal session names itself: the newest rollout begun in `folder` at or after
/// `since` (milliseconds), with its thread ID.
pub(crate) fn newest_codex_session(
    home: &Path,
    folder: &Path,
    since: Millis,
) -> Option<(PathBuf, String)> {
    let mut found: Option<(Millis, PathBuf, String)> = None;
    for path in rollouts(home) {
        let Some((id, cwd, at)) = codex_meta(&path) else {
            continue;
        };
        if at + 2000 < since || !same_folder(&cwd, folder) {
            continue;
        }
        if found.as_ref().is_none_or(|(best, _, _)| at > *best) {
            found = Some((at, path, id));
        }
    }
    found.map(|(_, path, id)| (path, id))
}

/// The file's length now: an import later starts after what is already there.
pub(crate) fn length(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

/// The turns the file gained after `from` bytes. A file now shorter than `from` was rewritten, and
/// is read from its start.
pub(crate) fn read_since(vendor: Vendor, path: &Path, from: u64) -> Imported {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Imported::default();
    };
    let len = file.metadata().map_or(0, |meta| meta.len());
    let start = if from > len { 0 } else { from };
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Imported::default();
    }
    let mut bytes = Vec::new();
    let truncated = len.saturating_sub(start) > READ_LIMIT;
    if file.take(READ_LIMIT).read_to_end(&mut bytes).is_err() {
        return Imported::default();
    }
    // A partly written last line (the agent still writing) is left for later.
    if let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') {
        bytes.truncate(end + 1);
    } else {
        bytes.clear();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut imported = match vendor {
        Vendor::Claude => parse_claude(&text),
        Vendor::Codex => parse_codex(&text),
        Vendor::Grok => parse_grok(&text),
        Vendor::Pi => parse_pi(&text),
        Vendor::Cursor => parse_cursor(&text),
        Vendor::Hermes | Vendor::OpenCode | Vendor::Antigravity => Imported::default(),
    };
    imported.truncated = truncated;
    imported
}

/// Claude Code's lines: a user line with text (a string, or text parts) opens a turn; assistant
/// text parts are its answer. Tool results, thinking, tool calls, sidechains, meta lines and the
/// local-command wrappers Claude Code writes as user lines are left out.
pub(crate) fn parse_claude(text: &str) -> Imported {
    let mut out = Imported::default();
    let mut open: Option<ImportedTurn> = None;
    // Edit calls waiting for their result: id, when, lines.
    let mut pending: HashMap<String, (Millis, u64)> = HashMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            out.skipped += 1;
            continue;
        };
        if value["isSidechain"] == true || value["isMeta"] == true {
            continue;
        }
        let at = value["timestamp"]
            .as_str()
            .and_then(rfc3339_millis)
            .unwrap_or(0);
        let content = &value["message"]["content"];
        let parts = content.as_array().map(Vec::as_slice).unwrap_or_default();
        match value["type"].as_str() {
            Some("user") => {
                for part in parts.iter().filter(|part| part["type"] == "tool_result") {
                    let done = part["tool_use_id"]
                        .as_str()
                        .and_then(|id| pending.remove(id))
                        .filter(|_| part["is_error"] != true);
                    if let (Some(edit), Some(turn)) = (done, open.as_mut()) {
                        turn.edits.push(edit);
                    }
                }
                let prompt = match content {
                    Value::String(text) => text.clone(),
                    Value::Array(parts) => joined(parts, "text"),
                    _ => String::new(),
                };
                let prompt = prompt.trim();
                if prompt.is_empty() || wrapper(prompt) {
                    continue;
                }
                if let Some(turn) = open.take() {
                    out.turns.push(turn);
                }
                open = Some(ImportedTurn {
                    prompt: prompt.to_owned(),
                    answer: String::new(),
                    started_at: at,
                    ended_at: at,
                    edits: Vec::new(),
                });
            }
            Some("assistant") => {
                for part in parts.iter().filter(|part| part["type"] == "tool_use") {
                    let lines = tool_edit(part["name"].as_str().unwrap_or(""), &part["input"]);
                    if let (Some(id), true) = (part["id"].as_str(), lines > 0) {
                        pending.insert(id.to_owned(), (at, lines));
                    }
                }
                let Some(turn) = open.as_mut() else {
                    continue;
                };
                let answer = match content {
                    Value::Array(parts) => joined(parts, "text"),
                    Value::String(text) => text.clone(),
                    _ => String::new(),
                };
                append(&mut turn.answer, &answer);
                turn.ended_at = turn.ended_at.max(at);
            }
            _ => {}
        }
    }
    out.turns.extend(open);
    out
}

/// Codex's rollout: a user message opens a turn, its parts without the context blocks Codex
/// injects (`<environment_context>`, instruction files); assistant `output_text` is its answer.
pub(crate) fn parse_codex(text: &str) -> Imported {
    let mut out = Imported::default();
    let mut open: Option<ImportedTurn> = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            out.skipped += 1;
            continue;
        };
        let at = value["timestamp"]
            .as_str()
            .and_then(rfc3339_millis)
            .unwrap_or(0);
        let payload = &value["payload"];
        if value["type"] == "event_msg" && payload["type"] == "item_completed" {
            let item = &payload["item"];
            if item["type"] == "FileChange"
                && item["status"] == "completed"
                && let Some(turn) = open.as_mut()
            {
                let lines = codex_changes(&item["changes"]);
                if lines > 0 {
                    let when = payload["completed_at_ms"].as_i64().unwrap_or(at);
                    turn.edits.push((when, lines));
                }
            }
            continue;
        }
        if value["type"] != "response_item" || payload["type"] != "message" {
            continue;
        }
        let Value::Array(parts) = &payload["content"] else {
            continue;
        };
        match payload["role"].as_str() {
            Some("user") => {
                let prompt = parts
                    .iter()
                    .filter(|part| part["type"] == "input_text")
                    .filter_map(|part| part["text"].as_str())
                    .map(str::trim)
                    .filter(|text| !text.is_empty() && !injected(text))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if prompt.is_empty() {
                    continue;
                }
                if let Some(turn) = open.take() {
                    out.turns.push(turn);
                }
                open = Some(ImportedTurn {
                    prompt,
                    answer: String::new(),
                    started_at: at,
                    ended_at: at,
                    edits: Vec::new(),
                });
            }
            Some("assistant") => {
                let Some(turn) = open.as_mut() else {
                    continue;
                };
                append(&mut turn.answer, &joined(parts, "output_text"));
                turn.ended_at = turn.ended_at.max(at);
            }
            _ => {}
        }
    }
    out.turns.extend(open);
    out
}

fn joined(parts: &[Value], kind: &str) -> String {
    parts
        .iter()
        .filter(|part| part["type"] == kind)
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn append(answer: &mut String, more: &str) {
    let more = more.trim();
    if more.is_empty() {
        return;
    }
    if !answer.is_empty() {
        answer.push_str("\n\n");
    }
    answer.push_str(more);
}

/// Claude Code's own wrappers around local commands and their output, written as user lines.
fn wrapper(text: &str) -> bool {
    [
        "<command-name>",
        "<command-message>",
        "<local-command-",
        "<bash-",
        "Caveat: ",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
}

/// Context Codex adds to the first user message: XML-tagged blocks and instruction files.
fn injected(text: &str) -> bool {
    text.starts_with('<') || text.starts_with("# AGENTS.md") || text.starts_with("# Instructions")
}

/// A vendor session ID Ghost will look a file up by: no path separators or dots.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Every rollout under `<home>/sessions/YYYY/MM/DD`, newest days first, bounded.
fn rollouts(home: &Path) -> Vec<PathBuf> {
    let mut days = Vec::new();
    let level = |dir: &Path| -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|read| {
                read.flatten()
                    .take(DIRS_LIMIT)
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by(|a, b| b.cmp(a));
        entries
    };
    for year in level(&home.join("sessions")) {
        for month in level(&year) {
            days.extend(level(&month));
            if days.len() >= 62 {
                break;
            }
        }
        if days.len() >= 62 {
            break;
        }
    }
    let mut files = Vec::new();
    for day in days.into_iter().take(62) {
        if let Ok(read) = std::fs::read_dir(&day) {
            files.extend(
                read.flatten()
                    .take(DIRS_LIMIT)
                    .map(|e| e.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                    }),
            );
        }
    }
    files
}

/// A rollout's thread ID, folder and start, from its first line (`session_meta`).
fn codex_meta(path: &Path) -> Option<(String, PathBuf, Millis)> {
    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(file.take(256 * 1024)),
        &mut first,
    )
    .ok()?;
    let value: Value = serde_json::from_str(first.trim()).ok()?;
    if value["type"] != "session_meta" {
        return None;
    }
    let payload = &value["payload"];
    let id = payload["id"]
        .as_str()
        .or_else(|| payload["session_id"].as_str())?
        .to_owned();
    let cwd = PathBuf::from(payload["cwd"].as_str()?);
    let at = payload["timestamp"]
        .as_str()
        .or_else(|| value["timestamp"].as_str())
        .and_then(rfc3339_millis)?;
    plain_id(&id).then_some((id, cwd, at))
}

/// Whether `a` and `b` name one folder: as written (case aside on Windows), or on Unix once their
/// links are resolved. An agent records the resolved folder it runs in, and on macOS the temporary
/// and many other folders sit behind a link (`/var` is `/private/var`). Windows compares the text
/// only, so a path on an unreachable share never stalls the read.
fn same_folder(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let text = p.to_string_lossy().replace('\\', "/");
        let text = text.trim_end_matches('/').to_owned();
        if cfg!(windows) {
            text.to_ascii_lowercase()
        } else {
            text
        }
    };
    norm(a) == norm(b)
        || (cfg!(unix) && matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b))
}

/// `2026-11-06T07:45:16.764Z` (or with an offset) in milliseconds since the epoch.
pub(crate) fn rfc3339_millis(text: &str) -> Option<Millis> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let num = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &text[19..];
    let mut millis = 0i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits: String = fraction.chars().take_while(char::is_ascii_digit).collect();
        rest = &fraction[digits.len()..];
        let padded = format!("{digits:0<3}");
        millis = padded.get(..3)?.parse().ok()?;
    }
    let offset = match rest {
        "Z" | "z" => 0,
        zone if zone.len() == 6 && (zone.starts_with('+') || zone.starts_with('-')) => {
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6)?.parse().ok()?;
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            sign * (hours * 60 + minutes)
        }
        _ => return None,
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset * 60;
    Some(seconds * 1000 + millis)
}

/// Session files a folder scan looks at, at most.
const FILES_LIMIT: usize = 65_536;
/// How long an agent's own command gets to answer when its sessions are read through it.
const CLI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// A session that began this long before the terminal opened still counts as the terminal's: the
/// two clocks are the same machine's, but an agent can stamp a session as it starts.
const CLOCK_SLACK: Millis = 2000;

/// Grok Build's history for `session_id` under its home (`~/.grok`): found by ID in any folder's
/// sessions, since the folder's name is Grok Build's own spelling of the path.
pub(crate) fn grok_session(home: &Path, session_id: &str) -> Option<PathBuf> {
    if !plain_id(session_id) {
        return None;
    }
    std::fs::read_dir(home.join("sessions"))
        .ok()?
        .flatten()
        .take(DIRS_LIMIT)
        .map(|entry| entry.path().join(session_id).join("chat_history.jsonl"))
        .find(|path| path.is_file())
}

/// Pi's (or oh-my-pi's) session file for `session_id` under its agent folder: `<time>_<id>.jsonl`
/// in any folder's sessions.
pub(crate) fn pi_session(base: &Path, session_id: &str) -> Option<PathBuf> {
    if !plain_id(session_id) {
        return None;
    }
    let suffix = format!("_{session_id}.jsonl");
    pi_files(base).into_iter().find(|path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(&suffix))
    })
}

/// A new oh-my-pi terminal session names itself: the newest session file begun in `folder` at or
/// after `since`, with its ID, from the file's first line (`{"type":"session","id","cwd"}`).
pub(crate) fn newest_pi_session(
    base: &Path,
    folder: &Path,
    since: Millis,
) -> Option<(PathBuf, String)> {
    let mut found: Option<(Millis, PathBuf, String)> = None;
    for path in pi_files(base) {
        let modified = modified_millis(&path).unwrap_or(0);
        if modified + CLOCK_SLACK < since {
            continue;
        }
        let Some((id, cwd)) = first_line(&path).and_then(|value| {
            (value["type"] == "session").then(|| {
                (
                    value["id"].as_str().map(str::to_owned),
                    value["cwd"].as_str().map(PathBuf::from),
                )
            })
        }) else {
            continue;
        };
        let (Some(id), Some(cwd)) = (id, cwd) else {
            continue;
        };
        if !plain_id(&id) || !same_folder(&cwd, folder) {
            continue;
        }
        if found.as_ref().is_none_or(|(best, _, _)| modified > *best) {
            found = Some((modified, path, id));
        }
    }
    found.map(|(_, path, id)| (path, id))
}

/// Every `*.jsonl` directly in a folder of `<base>/sessions`, bounded.
fn pi_files(base: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(dirs) = std::fs::read_dir(base.join("sessions")) else {
        return files;
    };
    for dir in dirs.flatten().take(DIRS_LIMIT) {
        let Ok(read) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for entry in read.flatten() {
            if files.len() >= FILES_LIMIT {
                return files;
            }
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                files.push(path);
            }
        }
    }
    files
}

/// Antigravity's conversation `id` under its CLI's folder (`~/.gemini/antigravity-cli`).
pub(crate) fn antigravity_conversation(base: &Path, id: &str) -> Option<PathBuf> {
    if !plain_id(id) {
        return None;
    }
    std::fs::read_dir(base.join("conversations"))
        .ok()?
        .flatten()
        .take(FILES_LIMIT)
        .map(|entry| entry.path())
        .find(|path| path.file_stem().and_then(|s| s.to_str()) == Some(id))
}

/// A new Antigravity terminal conversation: the newest one written at or after `since`. Its
/// conversations are binary, so only its ID is learned, never its turns.
pub(crate) fn newest_antigravity_conversation(base: &Path, since: Millis) -> Option<String> {
    let mut found: Option<(Millis, String)> = None;
    for entry in std::fs::read_dir(base.join("conversations"))
        .ok()?
        .flatten()
        .take(FILES_LIMIT)
    {
        let path = entry.path();
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !plain_id(id) {
            continue;
        }
        let at = created_millis(&path).unwrap_or(0);
        if at + CLOCK_SLACK < since {
            continue;
        }
        if found.as_ref().is_none_or(|(best, _)| at > *best) {
            found = Some((at, id.to_owned()));
        }
    }
    found.map(|(_, id)| id)
}

fn modified_millis(path: &Path) -> Option<Millis> {
    let time = std::fs::metadata(path).ok()?.modified().ok()?;
    Millis::try_from(time.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// When a file was made, or last written where the system does not keep that.
fn created_millis(path: &Path) -> Option<Millis> {
    let meta = std::fs::metadata(path).ok()?;
    let time = meta.created().or_else(|_| meta.modified()).ok()?;
    Millis::try_from(time.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis()).ok()
}

fn first_line(path: &Path) -> Option<Value> {
    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(file.take(256 * 1024)),
        &mut first,
    )
    .ok()?;
    serde_json::from_str(first.trim()).ok()
}

/// Text from a string, or the `text` of an array's text parts.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => joined(parts, "text"),
        _ => String::new(),
    }
}

/// Grok Build's history: a prompt (a user line with `prompt_index`, its text in `<user_query>`)
/// opens a turn; assistant content is its answer. What Grok Build writes itself as user lines is
/// left out: lines with `synthetic_reason` (reminders) and tagged context without a prompt index
/// (`<user_info>`, `<rules>`), as are reasoning and system lines.
pub(crate) fn parse_grok(text: &str) -> Imported {
    let mut out = Imported::default();
    let mut open: Option<ImportedTurn> = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            out.skipped += 1;
            continue;
        };
        match value["type"].as_str() {
            Some("user") => {
                if !value["synthetic_reason"].is_null() {
                    continue;
                }
                let text = text_of(&value["content"]);
                let mut prompt = text.trim();
                if let Some(inner) = user_query(prompt) {
                    prompt = inner;
                } else if value["prompt_index"].is_null() && prompt.starts_with('<') {
                    continue;
                }
                if prompt.is_empty() {
                    continue;
                }
                if let Some(turn) = open.take() {
                    out.turns.push(turn);
                }
                open = Some(ImportedTurn {
                    prompt: prompt.to_owned(),
                    answer: String::new(),
                    started_at: 0,
                    ended_at: 0,
                    edits: Vec::new(),
                });
            }
            Some("assistant") => {
                if let Some(turn) = open.as_mut() {
                    append(&mut turn.answer, &text_of(&value["content"]));
                }
            }
            _ => {}
        }
    }
    out.turns.extend(open);
    out
}

/// Pi's (and oh-my-pi's) session: a user message opens a turn; assistant text parts are its
/// answer. Tool calls, tool results, thinking and session entries are left out.
pub(crate) fn parse_pi(text: &str) -> Imported {
    let mut out = Imported::default();
    let mut open: Option<ImportedTurn> = None;
    let mut pending: HashMap<String, (Millis, u64)> = HashMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            out.skipped += 1;
            continue;
        };
        if value["type"] != "message" {
            continue;
        }
        let message = &value["message"];
        let at = value["timestamp"]
            .as_str()
            .and_then(rfc3339_millis)
            .or_else(|| message["timestamp"].as_i64())
            .unwrap_or(0);
        match message["role"].as_str() {
            Some("user") => {
                let prompt = text_of(&message["content"]);
                let prompt = prompt.trim();
                if prompt.is_empty() {
                    continue;
                }
                if let Some(turn) = open.take() {
                    out.turns.push(turn);
                }
                open = Some(ImportedTurn {
                    prompt: prompt.to_owned(),
                    answer: String::new(),
                    started_at: at,
                    ended_at: at,
                    edits: Vec::new(),
                });
            }
            Some("assistant") => {
                for part in message["content"].as_array().into_iter().flatten() {
                    if part["type"] != "toolCall" {
                        continue;
                    }
                    let lines = tool_edit(part["name"].as_str().unwrap_or(""), &part["arguments"]);
                    if let (Some(id), true) = (part["id"].as_str(), lines > 0) {
                        pending.insert(id.to_owned(), (at, lines));
                    }
                }
                if let Some(turn) = open.as_mut() {
                    append(&mut turn.answer, &text_of(&message["content"]));
                    turn.ended_at = turn.ended_at.max(at);
                }
            }
            Some("toolResult") => {
                let done = message["toolCallId"]
                    .as_str()
                    .and_then(|id| pending.remove(id))
                    .filter(|_| message["isError"] != true);
                if let (Some(edit), Some(turn)) = (done, open.as_mut()) {
                    turn.edits.push(edit);
                }
            }
            _ => {}
        }
    }
    out.turns.extend(open);
    out
}

/// An agent's own command line, for agents whose sessions are read through it (Hermes, OpenCode):
/// its program and the chat's environment.
#[derive(Debug, Clone)]
pub(crate) struct Cli {
    pub program: PathBuf,
    pub env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

impl Cli {
    /// Runs the command in `folder` with the chat's environment, without a console window, and
    /// returns what it printed, bounded in time and size. Anything else (it failed to start, ran
    /// too long) reads as nothing.
    fn run(&self, folder: &Path, args: &[&str]) -> Option<String> {
        use std::process::{Command, Stdio};
        let mut command = Command::new(&self.program);
        command
            .args(args)
            .current_dir(folder)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            drop((&mut stdout).take(READ_LIMIT).read_to_end(&mut bytes));
            bytes
        });
        let deadline = std::time::Instant::now() + CLI_TIMEOUT;
        let finished = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                _ => {
                    drop(child.kill());
                    drop(child.wait());
                    break false;
                }
            }
        };
        let bytes = reader.join().ok()?;
        finished.then(|| String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Milliseconds from a number of seconds or of milliseconds (agents write either).
fn epoch_millis(value: &Value) -> Option<Millis> {
    let number = value.as_f64()?;
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, reason = "an epoch time fits")]
    let millis = if number < 1e11 {
        (number * 1000.0) as Millis
    } else {
        number as Millis
    };
    Some(millis)
}

/// Hermes' terminal session, through `hermes sessions export`: the known session, or the newest one
/// begun in `folder` at or after `since`, which is learned. Its messages from `since` on (all of a
/// learned one's) become turns.
pub(crate) fn read_hermes(
    cli: &Cli,
    folder: &Path,
    native_id: Option<&str>,
    since: Millis,
) -> (Imported, Option<String>) {
    let folder_text = folder.to_string_lossy().into_owned();
    let mut args = vec!["sessions", "export", "--format", "jsonl", "--yes"];
    match native_id {
        Some(id) => args.extend(["--session-id", id]),
        None => args.extend(["--cwd", folder_text.as_str(), "--newer-than", "2d"]),
    }
    args.push("-");
    let Some(text) = cli.run(folder, &args) else {
        return (Imported::default(), None);
    };
    hermes_export(&text, folder, native_id, since)
}

/// What `hermes sessions export --format jsonl` printed, as [`read_hermes`] reads it.
pub(crate) fn hermes_export(
    text: &str,
    folder: &Path,
    native_id: Option<&str>,
    since: Millis,
) -> (Imported, Option<String>) {
    let mut skipped = 0;
    let sessions: Vec<Value> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let parsed = serde_json::from_str::<Value>(line).ok();
            skipped += usize::from(parsed.is_none());
            parsed
        })
        .collect();
    let chosen = match native_id {
        Some(id) => sessions.into_iter().find(|session| session["id"] == id),
        None => sessions
            .into_iter()
            .filter(|session| {
                session["id"].as_str().is_some_and(plain_id)
                    && epoch_millis(&session["started_at"])
                        .is_some_and(|at| at + CLOCK_SLACK >= since)
                    && session["cwd"]
                        .as_str()
                        .is_some_and(|cwd| same_folder(Path::new(cwd), folder))
            })
            .max_by_key(|session| epoch_millis(&session["started_at"]).unwrap_or(0)),
    };
    let Some(session) = chosen else {
        return (Imported::default(), None);
    };
    let learned = native_id
        .is_none()
        .then(|| session["id"].as_str().map(str::to_owned))
        .flatten();
    let from = if native_id.is_some() { since } else { 0 };
    let mut imported = Imported {
        skipped,
        ..Imported::default()
    };
    let mut open: Option<ImportedTurn> = None;
    for message in session["messages"].as_array().into_iter().flatten() {
        // The terminal opened before it wrote anything: no slack for a message.
        let at = epoch_millis(&message["timestamp"]).unwrap_or(0);
        if at != 0 && at < from {
            continue;
        }
        let text = text_of(&message["content"]);
        match message["role"].as_str() {
            Some("user") => {
                let prompt = text.trim();
                if prompt.is_empty() {
                    continue;
                }
                if let Some(turn) = open.take() {
                    imported.turns.push(turn);
                }
                open = Some(ImportedTurn {
                    prompt: prompt.to_owned(),
                    answer: String::new(),
                    started_at: at,
                    ended_at: at,
                    edits: Vec::new(),
                });
            }
            Some("assistant") => {
                if let Some(turn) = open.as_mut() {
                    append(&mut turn.answer, &text);
                    turn.ended_at = turn.ended_at.max(at);
                    for call in message["tool_calls"].as_array().into_iter().flatten() {
                        let function = if call["function"].is_object() {
                            &call["function"]
                        } else {
                            call
                        };
                        let arguments = match &function["arguments"] {
                            Value::String(json) => serde_json::from_str(json).unwrap_or_default(),
                            other => other.clone(),
                        };
                        let lines = tool_edit(function["name"].as_str().unwrap_or(""), &arguments);
                        if lines > 0 {
                            turn.edits.push((at, lines));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    imported.turns.extend(open);
    (imported, learned)
}

/// OpenCode's terminal session, through `opencode export`: the known session, or the newest one
/// `opencode session list` shows begun at or after `since` in the chat's folder, which is learned.
/// Its messages from `since` on (all of a learned one's) become turns.
pub(crate) fn read_opencode(
    cli: &Cli,
    folder: &Path,
    native_id: Option<&str>,
    since: Millis,
) -> (Imported, Option<String>) {
    let (id, learned) = match native_id {
        Some(id) => (id.to_owned(), None),
        None => {
            let newest = cli
                .run(folder, &["session", "list", "--format", "json", "-n", "20"])
                .and_then(|text| opencode_newest(&text, folder, since));
            let Some(id) = newest else {
                return (Imported::default(), None);
            };
            (id.clone(), Some(id))
        }
    };
    if !plain_id(&id) {
        return (Imported::default(), learned);
    }
    let Some(export) = cli.run(folder, &["export", &id]) else {
        return (Imported::default(), learned);
    };
    let from = if native_id.is_some() { since } else { 0 };
    (opencode_export(&export, from), learned)
}

fn opencode_created(value: &Value) -> Option<Millis> {
    epoch_millis(&value["time"]["created"]).or_else(|| epoch_millis(&value["created"]))
}

/// The newest session `opencode session list --format json` shows begun at or after `since` in
/// `folder`.
pub(crate) fn opencode_newest(text: &str, folder: &Path, since: Millis) -> Option<String> {
    json_in(text)?
        .as_array()?
        .iter()
        .filter(|session| {
            session["id"].as_str().is_some_and(plain_id)
                && opencode_created(session).is_some_and(|at| at + CLOCK_SLACK >= since)
                && session["directory"]
                    .as_str()
                    .is_none_or(|dir| same_folder(Path::new(dir), folder))
        })
        .max_by_key(|session| opencode_created(session).unwrap_or(0))
        .and_then(|session| session["id"].as_str().map(str::to_owned))
}

/// What `opencode export` printed, as turns from `from` on.
pub(crate) fn opencode_export(text: &str, from: Millis) -> Imported {
    let created = opencode_created;
    let Some(export) = json_in(text) else {
        return Imported {
            skipped: usize::from(!text.trim().is_empty()),
            ..Imported::default()
        };
    };
    let mut imported = Imported::default();
    let mut open: Option<ImportedTurn> = None;
    for message in export["messages"].as_array().into_iter().flatten() {
        let info = &message["info"];
        let at = created(info).unwrap_or(0);
        if at != 0 && at < from {
            continue;
        }
        let text = message["parts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "text" && part["synthetic"] != true)
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        match info["role"].as_str() {
            Some("user") => {
                let prompt = text.trim();
                if prompt.is_empty() {
                    continue;
                }
                if let Some(turn) = open.take() {
                    imported.turns.push(turn);
                }
                open = Some(ImportedTurn {
                    prompt: prompt.to_owned(),
                    answer: String::new(),
                    started_at: at,
                    ended_at: at,
                    edits: Vec::new(),
                });
            }
            Some("assistant") => {
                if let Some(turn) = open.as_mut() {
                    append(&mut turn.answer, &text);
                    let done = epoch_millis(&info["time"]["completed"]).unwrap_or(at);
                    turn.ended_at = turn.ended_at.max(done);
                    for part in message["parts"].as_array().into_iter().flatten() {
                        let state = &part["state"];
                        if part["type"] != "tool" || state["status"] != "completed" {
                            continue;
                        }
                        let lines = tool_edit(part["tool"].as_str().unwrap_or(""), &state["input"]);
                        if lines > 0 {
                            let when = epoch_millis(&state["time"]["end"]).unwrap_or(done);
                            turn.edits.push((when, lines));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    imported.turns.extend(open);
    imported
}

/// The JSON a command printed, after anything it printed first.
fn json_in(text: &str) -> Option<Value> {
    let start = text.find(['{', '['])?;
    serde_json::from_str(text[start..].trim()).ok()
}

/// The person's own words in a prompt an agent wrapped in `<user_query>` (Grok Build, Cursor),
/// whatever context it put around them.
fn user_query(text: &str) -> Option<&str> {
    let start = text.find("<user_query>")? + "<user_query>".len();
    let end = start + text[start..].find("</user_query>")?;
    Some(text[start..end].trim())
}

/// Cursor's transcript for `chat_id` under the person's home:
/// `.cursor/projects/<folder>/agent-transcripts/<id>/<id>.jsonl`, found by ID in any folder's.
pub(crate) fn cursor_transcript(home: &Path, chat_id: &str) -> Option<PathBuf> {
    if !plain_id(chat_id) {
        return None;
    }
    let name = format!("{chat_id}.jsonl");
    std::fs::read_dir(home.join(".cursor").join("projects"))
        .ok()?
        .flatten()
        .take(DIRS_LIMIT)
        .map(|entry| {
            entry
                .path()
                .join("agent-transcripts")
                .join(chat_id)
                .join(&name)
        })
        .find(|path| path.is_file())
}

/// A new Cursor terminal chat names itself: the newest chat begun in `folder` at or after `since`,
/// from its `.cursor/chats/<workspace>/<id>/meta.json` (`cwd`, `createdAtMs`).
pub(crate) fn newest_cursor_chat(home: &Path, folder: &Path, since: Millis) -> Option<String> {
    let mut found: Option<(Millis, String)> = None;
    let mut seen = 0usize;
    for workspace in std::fs::read_dir(home.join(".cursor").join("chats"))
        .ok()?
        .flatten()
        .take(DIRS_LIMIT)
    {
        let Ok(chats) = std::fs::read_dir(workspace.path()) else {
            continue;
        };
        for chat in chats.flatten() {
            seen += 1;
            if seen > FILES_LIMIT {
                return found.map(|(_, id)| id);
            }
            let Some(id) = chat.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !plain_id(&id) {
                continue;
            }
            let Some(meta) = std::fs::read(chat.path().join("meta.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            else {
                continue;
            };
            let Some(at) = meta["createdAtMs"].as_i64() else {
                continue;
            };
            if at + CLOCK_SLACK < since
                || !meta["cwd"]
                    .as_str()
                    .is_some_and(|cwd| same_folder(Path::new(cwd), folder))
            {
                continue;
            }
            if found.as_ref().is_none_or(|(best, _)| at > *best) {
                found = Some((at, id));
            }
        }
    }
    found.map(|(_, id)| id)
}

/// Cursor's transcript: a user line opens a turn with the person's words (inside the
/// `<user_query>` Cursor wraps them in, after its `<timestamp>`); assistant text is its answer.
pub(crate) fn parse_cursor(text: &str) -> Imported {
    let mut out = Imported::default();
    let mut open: Option<ImportedTurn> = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            out.skipped += 1;
            continue;
        };
        let content = text_of(&value["message"]["content"]);
        match value["role"].as_str() {
            Some("user") => {
                let prompt = match user_query(&content) {
                    Some(inner) => inner,
                    None if content.trim_start().starts_with('<') => continue,
                    None => content.trim(),
                };
                if prompt.is_empty() {
                    continue;
                }
                if let Some(turn) = open.take() {
                    out.turns.push(turn);
                }
                open = Some(ImportedTurn {
                    prompt: prompt.to_owned(),
                    answer: String::new(),
                    started_at: 0,
                    ended_at: 0,
                    edits: Vec::new(),
                });
            }
            Some("assistant") => {
                if let Some(turn) = open.as_mut() {
                    append(&mut turn.answer, &content);
                    for part in value["message"]["content"].as_array().into_iter().flatten() {
                        if part["type"] != "tool_use" {
                            continue;
                        }
                        let lines = tool_edit(part["name"].as_str().unwrap_or(""), &part["input"]);
                        if lines > 0 {
                            turn.edits.push((0, lines));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out.turns.extend(open);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_read_as_the_epoch_milliseconds_they_name() {
        assert_eq!(rfc3339_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            rfc3339_millis("2026-11-06T07:45:16.764Z"),
            Some(1_793_951_116_764)
        );
        assert_eq!(
            rfc3339_millis("2026-11-06T02:45:16.764-05:00"),
            Some(1_793_951_116_764)
        );
        assert_eq!(
            rfc3339_millis("2026-11-06T07:45:16.7Z"),
            Some(1_793_951_116_700)
        );
        assert_eq!(rfc3339_millis("yesterday"), None);
        assert_eq!(rfc3339_millis("2026-13-06T07:45:16Z"), None);
    }

    #[test]
    fn claude_turns_keep_prompts_and_answer_text_only() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"Fix the header"},"timestamp":"2026-11-06T07:00:00.000Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"Looking."}]},"timestamp":"2026-11-06T07:00:01.000Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read"}]},"timestamp":"2026-11-06T07:00:02.000Z"}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"file"}]},"timestamp":"2026-11-06T07:00:03.000Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixed it."}]},"timestamp":"2026-11-06T07:00:09.000Z"}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":"subagent prompt"}}"#,
            r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#,
            "not json",
            r#"{"type":"user","message":{"content":[{"type":"text","text":"Now the footer"}]},"timestamp":"2026-11-06T07:01:00.000Z"}"#,
        ]
        .join("\n");
        let imported = parse_claude(&lines);
        assert_eq!(imported.skipped, 1);
        assert_eq!(imported.turns.len(), 2);
        assert_eq!(imported.turns[0].prompt, "Fix the header");
        assert_eq!(imported.turns[0].answer, "Looking.\n\nFixed it.");
        assert_eq!(
            imported.turns[0].ended_at - imported.turns[0].started_at,
            9000
        );
        assert_eq!(imported.turns[1].prompt, "Now the footer");
        assert_eq!(imported.turns[1].answer, "");
    }

    #[test]
    fn codex_turns_leave_out_injected_context() {
        let lines = [
            r#"{"type":"session_meta","payload":{"id":"t1","cwd":"/w","timestamp":"2026-11-06T07:00:00Z"}}"#,
            r#"{"type":"response_item","timestamp":"2026-11-06T07:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>cwd</environment_context>"},{"type":"input_text","text":"Add a test"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-11-06T07:00:05Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Added."}]}}"#,
        ]
        .join("\n");
        let imported = parse_codex(&lines);
        assert_eq!(
            imported.turns,
            vec![ImportedTurn {
                prompt: "Add a test".into(),
                answer: "Added.".into(),
                started_at: rfc3339_millis("2026-11-06T07:00:01Z").unwrap(),
                ended_at: rfc3339_millis("2026-11-06T07:00:05Z").unwrap(),
                edits: Vec::new(),
            }]
        );
    }

    #[test]
    fn reads_only_what_the_terminal_added_and_never_a_half_written_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let before = r#"{"type":"user","message":{"content":"from Chat"}}"#.to_owned() + "\n";
        let added =
            r#"{"type":"user","message":{"content":"from the terminal"}}"#.to_owned() + "\n";
        let partial = r#"{"type":"user","message":{"content":"still wri"#;
        std::fs::write(&path, format!("{before}{added}{partial}")).unwrap();
        let imported = read_since(Vendor::Claude, &path, before.len() as u64);
        let prompts: Vec<_> = imported.turns.iter().map(|t| t.prompt.as_str()).collect();
        assert_eq!(prompts, ["from the terminal"]);
        // A file rewritten shorter than where the import began is read from its start.
        std::fs::write(&path, &added).unwrap();
        assert_eq!(read_since(Vendor::Claude, &path, 10_000).turns.len(), 1);
        assert_eq!(
            read_since(Vendor::Claude, &dir.path().join("missing"), 0),
            Imported::default()
        );
    }

    #[test]
    fn session_files_are_found_by_id_and_a_new_codex_session_by_folder_and_time() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("projects").join("C--work-app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("abc-123.jsonl"), "").unwrap();
        assert_eq!(
            claude_session(dir.path(), "abc-123"),
            Some(project.join("abc-123.jsonl"))
        );
        assert_eq!(claude_session(dir.path(), "../escape"), None);

        let day = dir
            .path()
            .join("sessions")
            .join("2026")
            .join("11")
            .join("06");
        std::fs::create_dir_all(&day).unwrap();
        let meta = |id: &str, cwd: &str, at: &str| {
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{cwd}","timestamp":"{at}"}}}}"#
            ) + "\n"
        };
        std::fs::write(
            day.join("rollout-2026-11-06T07-00-00-old.jsonl"),
            meta("old", "/work/app", "2026-11-06T06:00:00Z"),
        )
        .unwrap();
        std::fs::write(
            day.join("rollout-2026-11-06T07-10-00-other.jsonl"),
            meta("other", "/work/elsewhere", "2026-11-06T07:10:00Z"),
        )
        .unwrap();
        std::fs::write(
            day.join("rollout-2026-11-06T07-05-00-new.jsonl"),
            meta("new", "/work/app/", "2026-11-06T07:05:00Z"),
        )
        .unwrap();
        let since = rfc3339_millis("2026-11-06T07:04:00Z").unwrap();
        let (path, id) = newest_codex_session(dir.path(), Path::new("/work/app"), since).unwrap();
        assert_eq!(id, "new");
        assert_eq!(codex_session(dir.path(), "new"), Some(path));
    }

    #[test]
    fn grok_turns_leave_out_what_grok_wrote_itself() {
        let text = [
            r#"{"type":"system","content":"You are Grok Build"}"#,
            r#"{"type":"user","content":[{"type":"text","text":"<instructions>"}],"synthetic_reason":"instructions"}"#,
            r#"{"type":"user","content":[{"type":"text","text":"<user_info>\nOS Version: windows\n</user_info>\n<rules>be brief</rules>"}]}"#,
            r#"{"type":"user","content":[{"type":"text","text":"<user_query>\nRename the file\n</user_query>"}],"prompt_index":0}"#,
            r#"{"type":"reasoning","id":"r1","summary":[]}"#,
            r#"{"type":"assistant","content":"Renamed it."}"#,
            r#"{"type":"user","content":[{"type":"text","text":"Thanks"}]}"#,
            "not json",
        ]
        .join("\n");
        let imported = parse_grok(&text);
        assert_eq!(imported.skipped, 1);
        assert_eq!(imported.turns.len(), 2);
        assert_eq!(imported.turns[0].prompt, "Rename the file");
        assert_eq!(imported.turns[0].answer, "Renamed it.");
        assert_eq!(imported.turns[1].prompt, "Thanks");
        assert_eq!(imported.turns[1].answer, "");
    }

    #[test]
    fn pi_turns_keep_prompts_and_answer_text_only() {
        let text = [
            r#"{"type":"session","version":3,"id":"p1","timestamp":"2026-11-08T10:00:00Z","cwd":"/work/app"}"#,
            r#"{"type":"model_change","id":"m","parentId":null,"provider":"x","modelId":"y"}"#,
            r#"{"type":"message","id":"a","timestamp":"2026-11-08T10:00:01Z","message":{"role":"user","content":[{"type":"text","text":"Fix the test"}]}}"#,
            r#"{"type":"message","id":"b","timestamp":"2026-11-08T10:00:05Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"toolCall","name":"bash"}]}}"#,
            r#"{"type":"message","id":"c","message":{"role":"toolResult","content":[{"type":"text","text":"ok"}]}}"#,
            r#"{"type":"message","id":"d","timestamp":"2026-11-08T10:00:09Z","message":{"role":"assistant","content":[{"type":"text","text":"Fixed."}]}}"#,
        ]
        .join("\n");
        let imported = parse_pi(&text);
        assert_eq!(imported.skipped, 0);
        assert_eq!(imported.turns.len(), 1);
        let turn = &imported.turns[0];
        assert_eq!(turn.prompt, "Fix the test");
        assert_eq!(turn.answer, "Fixed.");
        assert_eq!(turn.ended_at - turn.started_at, 8000);
    }

    #[test]
    fn grok_pi_and_antigravity_sessions_are_found_by_id_and_new_ones_by_folder_and_time() {
        let dir = tempfile::tempdir().unwrap();
        let grok = dir.path().join("sessions").join("C%3A%5Cwork").join("g-1");
        std::fs::create_dir_all(&grok).unwrap();
        std::fs::write(grok.join("chat_history.jsonl"), "").unwrap();
        assert_eq!(
            grok_session(dir.path(), "g-1"),
            Some(grok.join("chat_history.jsonl"))
        );
        assert_eq!(grok_session(dir.path(), "g-2"), None);
        assert_eq!(grok_session(dir.path(), "../g-1"), None);

        let pi = tempfile::tempdir().unwrap();
        let folder = pi.path().join("sessions").join("--work-app--");
        std::fs::create_dir_all(&folder).unwrap();
        let header = |id: &str, cwd: &str| {
            format!(r#"{{"type":"session","version":3,"id":"{id}","cwd":"{cwd}"}}"#) + "\n"
        };
        std::fs::write(
            folder.join("2026-11-08T10-00-00-000Z_p-old.jsonl"),
            header("p-old", "/work/app"),
        )
        .unwrap();
        assert_eq!(
            pi_session(pi.path(), "p-old"),
            Some(folder.join("2026-11-08T10-00-00-000Z_p-old.jsonl"))
        );
        let since =
            modified_millis(&folder.join("2026-11-08T10-00-00-000Z_p-old.jsonl")).unwrap() + 60_000;
        assert_eq!(
            newest_pi_session(pi.path(), Path::new("/work/app"), since),
            None,
            "a session older than the terminal is not its"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        let now = modified_millis(&folder.join("2026-11-08T10-00-00-000Z_p-old.jsonl")).unwrap();
        std::fs::write(
            folder.join("2026-11-08T11-00-00-000Z_p-new.jsonl"),
            header("p-new", "/work/app"),
        )
        .unwrap();
        std::fs::write(
            folder.join("2026-11-08T11-00-01-000Z_p-else.jsonl"),
            header("p-else", "/work/elsewhere"),
        )
        .unwrap();
        let (_, id) = newest_pi_session(pi.path(), Path::new("/work/app"), now).unwrap();
        assert!(id == "p-new" || id == "p-old", "{id}");
        assert_ne!(id, "p-else");

        let agy = tempfile::tempdir().unwrap();
        let conversations = agy.path().join("conversations");
        std::fs::create_dir_all(&conversations).unwrap();
        std::fs::write(conversations.join("c-1.pb"), b"\x01").unwrap();
        assert_eq!(
            antigravity_conversation(agy.path(), "c-1"),
            Some(conversations.join("c-1.pb"))
        );
        assert_eq!(
            newest_antigravity_conversation(agy.path(), 0),
            Some("c-1".to_owned())
        );
        assert_eq!(
            newest_antigravity_conversation(agy.path(), Millis::MAX / 2),
            None
        );
    }

    #[test]
    fn hermes_export_learns_a_new_session_in_the_folder_and_reads_its_turns() {
        let since = 1_762_600_000_000;
        let session = |id: &str, cwd: &str, started: f64, messages: &str| {
            format!(
                r#"{{"id":"{id}","source":"cli","cwd":"{cwd}","started_at":{started},"messages":{messages}}}"#
            )
        };
        let messages = r#"[
            {"role":"system","content":"You are Hermes","timestamp":1762600001.0},
            {"role":"user","content":"Plan the release","timestamp":1762600002.5},
            {"role":"assistant","content":"","tool_calls":[{"id":"t"}],"timestamp":1762600003.0},
            {"role":"tool","content":"done","timestamp":1762600004.0},
            {"role":"assistant","content":"Here is the plan.","timestamp":1762600005.0}
        ]"#
        .replace('\n', " ");
        let text = [
            session("local-old", "/work/app", 1_762_500_000.0, "[]"),
            session("local-new", "/work/app", 1_762_600_001.0, &messages),
            session("local-sub", "/work/app/sub", 1_762_600_009.0, "[]"),
        ]
        .join("\n");
        let (imported, learned) = hermes_export(&text, Path::new("/work/app"), None, since);
        assert_eq!(learned.as_deref(), Some("local-new"));
        assert_eq!(imported.turns.len(), 1);
        assert_eq!(imported.turns[0].prompt, "Plan the release");
        assert_eq!(imported.turns[0].answer, "Here is the plan.");
        // A resumed session: only what came after the terminal opened.
        let (imported, learned) = hermes_export(
            &text,
            Path::new("/work/app"),
            Some("local-new"),
            1_762_600_006_000,
        );
        assert_eq!(learned, None);
        assert!(imported.turns.is_empty(), "{imported:?}");
    }

    #[test]
    fn opencode_learns_the_newest_session_begun_in_the_folder_and_reads_its_text() {
        let list = r#"Loading...
[{"id":"ses_old","directory":"/work/app","time":{"created":1762500000000,"updated":1762500000000}},
 {"id":"ses_new","directory":"/work/app","time":{"created":1762600001000,"updated":1762600009000}},
 {"id":"ses_else","directory":"/work/else","time":{"created":1762600002000,"updated":1762600002000}}]"#;
        assert_eq!(
            opencode_newest(list, Path::new("/work/app"), 1_762_600_000_000).as_deref(),
            Some("ses_new")
        );
        assert_eq!(
            opencode_newest(list, Path::new("/work/app"), 1_762_700_000_000),
            None
        );
        let export = r#"{"info":{"id":"ses_new"},"messages":[
            {"info":{"id":"m1","role":"user","time":{"created":1762600002000}},"parts":[{"type":"text","text":"Add a test"},{"type":"text","text":"<context>","synthetic":true}]},
            {"info":{"id":"m2","role":"assistant","time":{"created":1762600003000,"completed":1762600008000}},"parts":[{"type":"reasoning","text":"thinking"},{"type":"tool","tool":"bash"},{"type":"text","text":"Added."}]}
        ]}"#;
        let imported = opencode_export(export, 0);
        assert_eq!(imported.turns.len(), 1);
        assert_eq!(imported.turns[0].prompt, "Add a test");
        assert_eq!(imported.turns[0].answer, "Added.");
        assert_eq!(imported.turns[0].ended_at, 1_762_600_008_000);
        assert!(opencode_export(export, 1_762_700_000_000).turns.is_empty());
        assert_eq!(opencode_export("garbage", 0).skipped, 1);
    }

    #[test]
    fn epoch_times_read_as_seconds_or_milliseconds() {
        assert_eq!(
            epoch_millis(&serde_json::json!(1_762_600_002.5)),
            Some(1_762_600_002_500)
        );
        assert_eq!(
            epoch_millis(&serde_json::json!(1_762_600_002_500_i64)),
            Some(1_762_600_002_500)
        );
        assert_eq!(epoch_millis(&serde_json::json!("soon")), None);
        assert_eq!(epoch_millis(&serde_json::json!(-1)), None);
    }

    #[test]
    fn an_agents_own_command_runs_in_the_folder_with_the_chats_environment_and_no_more() {
        let dir = tempfile::tempdir().unwrap();
        // The agent's command, as a script that prints its folder and one variable.
        #[cfg(windows)]
        let (program, body) = (
            dir.path().join("agent.cmd"),
            "@echo off\r\necho %CD%^|%GHOST_TEST_VALUE%\r\n",
        );
        #[cfg(not(windows))]
        let (program, body) = (
            dir.path().join("agent.sh"),
            "#!/bin/sh\necho \"$(pwd)|$GHOST_TEST_VALUE\"\n",
        );
        std::fs::write(&program, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os()
            .filter(|(key, _)| {
                let key = key.to_string_lossy().to_ascii_uppercase();
                key == "PATH" || key == "SYSTEMROOT" || key == "COMSPEC"
            })
            .collect();
        env.push(("GHOST_TEST_VALUE".into(), "seen".into()));
        let cli = Cli { program, env };
        let printed = cli.run(dir.path(), &[]).expect("output");
        let (folder, value) = printed.trim().split_once('|').expect("both");
        assert!(same_folder(Path::new(folder), dir.path()), "{folder}");
        assert_eq!(value, "seen");
        let missing = Cli {
            program: dir.path().join("missing"),
            env: Vec::new(),
        };
        assert_eq!(missing.run(dir.path(), &[]), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_reached_through_a_link_is_the_same_folder() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("project");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("linked");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(same_folder(&link, &real));
        assert!(same_folder(&real.join(""), &real));
        assert!(!same_folder(&link, dir.path()));
        // A folder that is gone matches only as written.
        let gone = dir.path().join("gone");
        assert!(same_folder(&gone, &gone));
        assert!(!same_folder(&gone, &real));
    }

    #[test]
    fn cursor_turns_keep_the_persons_words_and_a_new_chat_is_learned_from_its_meta() {
        let text = [
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<timestamp>Sunday</timestamp>\n<user_query>\nReply with only the word ok.\n</user_query>"}]}}"#,
            r#"{"role":"assistant","message":{"content":[{"type":"text","text":"ok"}]}}"#,
            r#"{"type":"turn_ended","status":"success"}"#,
        ]
        .join("\n");
        let imported = parse_cursor(&text);
        assert_eq!(imported.turns.len(), 1);
        assert_eq!(imported.turns[0].prompt, "Reply with only the word ok.");
        assert_eq!(imported.turns[0].answer, "ok");

        let home = tempfile::tempdir().unwrap();
        let transcript = home
            .path()
            .join(".cursor/projects/C-work-app/agent-transcripts/c-1");
        std::fs::create_dir_all(&transcript).unwrap();
        std::fs::write(transcript.join("c-1.jsonl"), "").unwrap();
        assert_eq!(
            cursor_transcript(home.path(), "c-1"),
            Some(transcript.join("c-1.jsonl"))
        );
        let chat = |id: &str, cwd: &str, at: i64| {
            let dir = home.path().join(".cursor/chats/ws").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("meta.json"),
                serde_json::json!({"createdAtMs": at, "cwd": cwd, "hasConversation": true})
                    .to_string(),
            )
            .unwrap();
        };
        chat("c-old", "/work/app", 1_794_180_000_000);
        chat("c-new", "/work/app", 1_794_180_439_416);
        chat("c-else", "/work/else", 1_794_180_440_000);
        assert_eq!(
            newest_cursor_chat(home.path(), Path::new("/work/app"), 1_794_180_430_000).as_deref(),
            Some("c-new")
        );
        assert_eq!(
            newest_cursor_chat(home.path(), Path::new("/work/app"), 1_794_180_500_000),
            None
        );
    }

    #[test]
    fn claude_terminal_edits_count_once_their_result_came_back_without_an_error() {
        let lines = [
            r#"{"type":"user","message":{"content":"Fix the header"},"timestamp":"2026-11-06T07:00:00.000Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"a.rs","old_string":"a","new_string":"b\nc"}}]},"timestamp":"2026-11-06T07:00:02.000Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"e2","name":"Write","input":{"file_path":"b.rs","content":"1\n2\n3\n"}}]},"timestamp":"2026-11-06T07:00:03.000Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"c.rs"}}]},"timestamp":"2026-11-06T07:00:04.000Z"}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"e1","content":"ok"},{"type":"tool_result","tool_use_id":"e2","is_error":true,"content":"denied"},{"type":"tool_result","tool_use_id":"r1","content":"file"}]},"timestamp":"2026-11-06T07:00:05.000Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixed it."}]},"timestamp":"2026-11-06T07:00:09.000Z"}"#,
        ]
        .join("\n");
        let imported = parse_claude(&lines);
        assert_eq!(imported.turns.len(), 1);
        assert_eq!(imported.turns[0].answer, "Fixed it.");
        assert_eq!(
            imported.turns[0].edits,
            vec![(rfc3339_millis("2026-11-06T07:00:02.000Z").unwrap(), 2)]
        );
    }

    #[test]
    fn codex_terminal_edits_are_its_completed_file_changes() {
        let lines = [
            r#"{"type":"response_item","timestamp":"2026-11-06T07:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Add a test"}]}}"#,
            r#"{"type":"event_msg","timestamp":"2026-11-06T07:00:03Z","payload":{"type":"item_completed","completed_at_ms":1793951000000,"item":{"type":"FileChange","status":"completed","changes":{"/w/a.rs":{"type":"update","unified_diff":"@@ -1 +1,2 @@\n-a\n+b\n+c\n","move_path":null},"/w/new.rs":{"type":"add","content":"x\ny\nz\n"}}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-11-06T07:00:04Z","payload":{"type":"item_completed","item":{"type":"FileChange","status":"failed","changes":{"/w/b.rs":{"type":"add","content":"no\n"}}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-11-06T07:00:04Z","payload":{"type":"item_completed","item":{"type":"CommandExecution","status":"completed"}}}"#,
            r#"{"type":"response_item","timestamp":"2026-11-06T07:00:05Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Added."}]}}"#,
        ]
        .join("\n");
        let imported = parse_codex(&lines);
        assert_eq!(imported.turns.len(), 1);
        assert_eq!(imported.turns[0].answer, "Added.");
        assert_eq!(imported.turns[0].edits, vec![(1_793_951_000_000, 5)]);
    }

    #[test]
    fn pi_terminal_edits_count_once_their_result_is_not_an_error() {
        let lines = [
            r#"{"type":"message","timestamp":"2026-11-06T07:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"Tidy it"}]}}"#,
            r#"{"type":"message","timestamp":"2026-11-06T07:00:01.000Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"edit","arguments":{"path":"a.ts","edits":[{"oldText":"a","newText":"b"},{"oldText":"c\nd","newText":"e"}]}},{"type":"toolCall","id":"c2","name":"write","arguments":{"path":"b.ts","content":"1\n2\n"}}]}}"#,
            r#"{"type":"message","timestamp":"2026-11-06T07:00:02.000Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"edit","isError":false,"content":[]}}"#,
            r#"{"type":"message","timestamp":"2026-11-06T07:00:02.000Z","message":{"role":"toolResult","toolCallId":"c2","toolName":"write","isError":true,"content":[]}}"#,
            r#"{"type":"message","timestamp":"2026-11-06T07:00:03.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Done."}]}}"#,
        ]
        .join("\n");
        let imported = parse_pi(&lines);
        assert_eq!(imported.turns.len(), 1);
        assert_eq!(imported.turns[0].answer, "Done.");
        assert_eq!(
            imported.turns[0].edits,
            vec![(rfc3339_millis("2026-11-06T07:00:01.000Z").unwrap(), 3)]
        );
    }

    #[test]
    fn cursor_terminal_edits_are_its_edit_tools() {
        let lines = [
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>Rename it</user_query>"}]}}"#,
            r#"{"role":"assistant","message":{"content":[{"type":"text","text":"Renaming."},{"type":"tool_use","name":"StrReplace","input":{"path":"a.rs","old_string":"old","new_string":"new"}},{"type":"tool_use","name":"Read","input":{"path":"a.rs"}}]}}"#,
        ]
        .join("\n");
        let imported = parse_cursor(&lines);
        assert_eq!(imported.turns.len(), 1);
        assert_eq!(imported.turns[0].edits, vec![(0, 1)]);
    }

    #[test]
    fn opencode_and_hermes_terminal_edits_are_their_edit_tools() {
        let export = r#"{"messages":[
            {"info":{"role":"user","time":{"created":1793951000000}},"parts":[{"type":"text","text":"Fix it"}]},
            {"info":{"role":"assistant","time":{"created":1793951001000,"completed":1793951009000}},"parts":[
                {"type":"text","text":"Fixed."},
                {"type":"tool","tool":"edit","state":{"status":"completed","input":{"filePath":"a.ts","oldString":"a","newString":"b\nc"},"time":{"start":1793951002000,"end":1793951003000}}},
                {"type":"tool","tool":"write","state":{"status":"error","input":{"filePath":"b.ts","content":"x\n"}}},
                {"type":"tool","tool":"read","state":{"status":"completed","input":{"filePath":"a.ts"}}}
            ]}
        ]}"#;
        let imported = opencode_export(export, 0);
        assert_eq!(imported.turns.len(), 1);
        assert_eq!(imported.turns[0].edits, vec![(1_793_951_003_000, 2)]);

        let session = serde_json::json!({
            "id": "20261106_070000_abcdef",
            "cwd": "/w",
            "started_at": 1_793_951_000,
            "messages": [
                {"role": "user", "content": "Fix it", "timestamp": 1_793_951_000},
                {"role": "assistant", "content": "Fixed.", "timestamp": 1_793_951_002, "tool_calls": [
                    {"id": "t1", "type": "function", "function": {"name": "patch", "arguments": "{\"path\":\"a.py\",\"old_string\":\"a\",\"new_string\":\"b\"}"}},
                    {"id": "t2", "type": "function", "function": {"name": "write_file", "arguments": {"path": "b.py", "content": "1\n2\n"}}},
                    {"id": "t3", "type": "function", "function": {"name": "terminal", "arguments": "{\"command\":\"ls\"}"}}
                ]}
            ]
        });
        let (imported, _) = hermes_export(
            &session.to_string(),
            Path::new("/w"),
            Some("20261106_070000_abcdef"),
            0,
        );
        assert_eq!(imported.turns.len(), 1);
        let lines: u64 = imported.turns[0].edits.iter().map(|(_, lines)| lines).sum();
        assert_eq!(lines, 3);
    }
}
