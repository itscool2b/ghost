//! Curation: a raw protocol log into a fixture (the log's redaction re-run, then home, user, host
//! and folder placeholders, session ids and timestamps), and [`scan_clean`], which must find nothing
//! in a committed fixture.
//!
//! Placeholders: `<CWD>` and `<HOME>` (and Claude Code's project-directory spelling of each,
//! `<CWD_ENC>` and `<HOME_ENC>`), an npm adapter's install folder (the spawn note's `cwd` when it is
//! not the chat folder) as `<ADAPTER>`, `<USER>`, `<HOST>`, native session ids as `<SESSION_n>` in
//! order of first appearance, ISO timestamps as `<TIME>`; `t` becomes milliseconds since the first
//! record.

use std::{collections::BTreeMap, sync::LazyLock};

use ghost_process::Redactor;
use regex::Regex;
use serde_json::{Map, Value};

/// Shorter needles would shred unrelated text (a short user name, a small number).
const MIN_NEEDLE: usize = 4;

/// Why a log could not become a fixture.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CurateError {
    #[error("line {line} is not a protocol log record")]
    NotARecord { line: usize },
    /// The log capped this record (spec 3.8), so a fixture made from the log could not replay it.
    #[error(
        "line {line} is a record the log capped ({cut} bytes cut), which a fixture cannot replay"
    )]
    Capped { line: usize, cut: u64 },
    #[error("the curated fixture still holds personal data: {}", .findings.join("; "))]
    Unclean { findings: Vec<String> },
}

static TIMESTAMP: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"\b\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?").ok()
});

/// A Windows profile folder in any spelling the home needle misses: the 8.3 short name Windows
/// gives the temp folder (`C:\Users\RUNNER~1\AppData\Local\Temp`), forward slashes, and the `\\?\`
/// prefix of a canonical path, which stays in front of a `<HOME>` the needle already put there.
static WINDOWS_HOME: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:\\\\\?\\)?(?:[a-z]:[\\/]users[\\/][^\\/"\s]+|<HOME>)"#).ok()
});

/// Token shapes, with the name a finding gives each.
static TOKENS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("an Anthropic key", r"sk-ant-[A-Za-z0-9_-]{10,}"),
        ("an API key", r"sk-[A-Za-z0-9_-]{20,}"),
        ("a GitHub token", r"gh[pousr]_[A-Za-z0-9]{20,}"),
        ("a GitHub token", r"github_pat_[A-Za-z0-9_]{20,}"),
        ("a Slack token", r"xox[abpr]-[A-Za-z0-9-]{10,}"),
        ("an AWS key", r"AKIA[0-9A-Z]{16}"),
        ("a Google key", r"AIza[0-9A-Za-z_-]{35}"),
        ("a GitLab token", r"glpat-[A-Za-z0-9_-]{20,}"),
        (
            "a JWT",
            r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
        ),
        ("a bearer token", r"Bearer [A-Za-z0-9._~+/=-]{16,}"),
    ]
    .into_iter()
    .filter_map(|(name, pattern)| Some((name, Regex::new(pattern).ok()?)))
    .collect()
});

/// Home folders on macOS, Linux and Windows (the last as written raw and as JSON escapes it).
const HOME_PREFIXES: [&str; 4] = ["/Users/", "/home/", "C:\\Users\\", "C:\\\\Users\\\\"];

/// A raw protocol log into a fixture. `home`, `cwd`, `user` and `host` are the literal values to
/// replace; empty or short ones are skipped.
pub fn curate(
    log_text: &str,
    home: &str,
    cwd: &str,
    user: &str,
    host: &str,
) -> Result<String, CurateError> {
    let redactor = Redactor::standard();
    let mut records = Vec::new();
    for (index, line) in log_text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut record: Value =
            serde_json::from_str(line).map_err(|_| CurateError::NotARecord { line: index + 1 })?;
        if !record.is_object() {
            return Err(CurateError::NotARecord { line: index + 1 });
        }
        if let Some(cut) = record.get("truncated") {
            return Err(CurateError::Capped {
                line: index + 1,
                cut: cut.as_u64().unwrap_or_default(),
            });
        }
        redactor.redact_value(&mut record);
        records.push(record);
    }

    let mut needles: Vec<(String, String)> = Vec::new();
    let mut add = |needle: &str, placeholder: &str| {
        if needle.chars().count() >= MIN_NEEDLE && !needles.iter().any(|(known, _)| known == needle)
        {
            needles.push((needle.to_owned(), placeholder.to_owned()));
        }
    };
    add(cwd, "<CWD>");
    add(&encode_project_dir(cwd), "<CWD_ENC>");
    // An npm adapter runs in its install folder (spec 4), under the run's data root.
    for record in &records {
        if let Some(folder) = record.pointer("/note/spawn/cwd").and_then(Value::as_str)
            && folder != cwd
        {
            add(folder, "<ADAPTER>");
        }
    }
    add(home, "<HOME>");
    add(&encode_project_dir(home), "<HOME_ENC>");
    add(user, "<USER>");
    add(host, "<HOST>");
    let mut sessions = Vec::new();
    for record in &records {
        collect_sessions(record, &mut sessions);
    }
    for (index, session) in sessions.iter().enumerate() {
        add(session, &format!("<SESSION_{}>", index + 1));
    }
    // Longest first, so a path holding the home folder is replaced whole.
    needles.sort_by_key(|(needle, _)| std::cmp::Reverse(needle.len()));
    curate_native_tool_json(&mut records, &needles, &redactor)?;

    let first_t = records
        .iter()
        .find_map(|record| record.get("t").and_then(Value::as_i64));
    let mut out = String::new();
    for mut record in records {
        if let (Some(first), Some(t)) = (first_t, record.get("t").and_then(Value::as_i64)) {
            record["t"] = Value::from(t.saturating_sub(first));
        }
        replace_strings(&mut record, &needles);
        out.push_str(&record.to_string());
        out.push('\n');
    }
    let findings = scan_clean(&out);
    if findings.is_empty() {
        Ok(out)
    } else {
        Err(CurateError::Unclean { findings })
    }
}

/// Native tool input is JSON split across arbitrary string boundaries. Redact complete values
/// before replacing individual records, otherwise an escaped or split home path can survive and
/// reappear in the replay's tool input. Preserve record order and count; changed inputs necessarily
/// have synthetic chunk boundaries after redaction. Unchanged inputs retain their original chunks.
fn curate_native_tool_json(
    records: &mut [Value],
    needles: &[(String, String)],
    redactor: &Redactor,
) -> Result<(), CurateError> {
    let mut generations = BTreeMap::<String, usize>::new();
    let mut groups = BTreeMap::<(String, String, u64), Vec<(usize, usize)>>::new();
    for (position, record) in records.iter().enumerate() {
        let message = &record["msg"];
        if message["type"] == "message_start" && message["message"]["role"] == "assistant" {
            *generations.entry("pi".into()).or_default() += 1;
        }
        if message["type"] == "message_update"
            && message["assistantMessageEvent"]["type"] == "toolcall_delta"
        {
            let event = &message["assistantMessageEvent"];
            if let (Some(index), Some(partial)) =
                (event["contentIndex"].as_u64(), event["delta"].as_str())
            {
                let generation = generations.get("pi").copied().unwrap_or(0);
                groups
                    .entry(("pi".into(), format!("message-{generation}"), index))
                    .or_default()
                    .push((position, partial.len()));
            }
            continue;
        }
        if message["type"] != "stream_event" {
            continue;
        }
        let parent = message["parent_tool_use_id"].as_str().unwrap_or_default();
        let generation = generations.entry(parent.to_owned()).or_default();
        let event = &message["event"];
        if event["type"] == "message_start" {
            *generation += 1;
        }
        if event["type"] != "content_block_delta" || event["delta"]["type"] != "input_json_delta" {
            continue;
        }
        let (Some(index), Some(partial)) = (
            event["index"].as_u64(),
            event["delta"]["partial_json"].as_str(),
        ) else {
            continue;
        };
        let id = message["api_message_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("message-{generation}"));
        groups
            .entry((parent.to_owned(), id, index))
            .or_default()
            .push((position, partial.len()));
    }
    for (number, positions) in groups.values().enumerate() {
        let original = positions
            .iter()
            .filter_map(|(position, _)| {
                records[*position]
                    .pointer(partial_pointer(&records[*position]))?
                    .as_str()
            })
            .collect::<String>();
        let curated = if let Ok(mut input) = serde_json::from_str::<Value>(&original) {
            let before = input.clone();
            redactor.redact_value(&mut input);
            replace_strings(&mut input, needles);
            if input == before {
                original.clone()
            } else {
                input.to_string()
            }
        } else {
            // Interrupted tool inputs may never form valid JSON. Replace the JSON-escaped spelling
            // first, then redact free text. A remaining recognizable path/token fails closed below.
            let mut partial = original.clone();
            for (needle, placeholder) in needles {
                let escaped = Value::String(needle.clone()).to_string();
                partial = partial.replace(&escaped[1..escaped.len() - 1], placeholder);
            }
            let mut partial = Value::String(redactor.redact_text(&partial).into_owned());
            replace_strings(&mut partial, needles);
            partial.as_str().unwrap_or_default().to_owned()
        };
        let findings = scan_clean(&curated);
        if !findings.is_empty() {
            return Err(CurateError::Unclean {
                findings: findings
                    .into_iter()
                    .map(|finding| format!("native tool input block {}: {finding}", number + 1))
                    .collect(),
            });
        }
        if curated == original {
            continue;
        }
        let mut start = 0;
        for (chunk, (position, length)) in positions.iter().enumerate() {
            let mut end = if chunk + 1 == positions.len() {
                curated.len()
            } else {
                (start + length).min(curated.len())
            };
            while !curated.is_char_boundary(end) {
                end -= 1;
            }
            let pointer = partial_pointer(&records[*position]);
            if let Some(value) = records[*position].pointer_mut(pointer) {
                *value = Value::String(curated[start..end].to_owned());
            }
            start = end;
        }
    }
    Ok(())
}

fn partial_pointer(record: &Value) -> &'static str {
    if record["msg"]["type"] == "message_update" {
        "/msg/assistantMessageEvent/delta"
    } else {
        "/msg/event/delta/partial_json"
    }
}

/// What makes a fixture unfit to commit: email addresses, token shapes, home folders. Each finding
/// names the line and the kind, never the value.
pub fn scan_clean(text: &str) -> Vec<String> {
    let mut findings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if has_email(line) {
            findings.push(format!("line {number}: an email address"));
        }
        for (name, pattern) in TOKENS.iter() {
            if pattern.is_match(line) {
                findings.push(format!("line {number}: {name}"));
            }
        }
        if HOME_PREFIXES.iter().any(|prefix| line.contains(prefix)) {
            findings.push(format!("line {number}: a home folder path"));
        }
    }
    findings.dedup();
    findings
}

/// Claude Code's name for a working directory under `~/.claude/projects/`: every character that is
/// not ASCII alphanumeric becomes `-`.
pub fn encode_project_dir(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Session identities in ACP fields and native Claude envelopes, in first-appearance order.
fn collect_sessions(value: &Value, sessions: &mut Vec<String>) {
    let mut note = |id: &str| {
        if !id.is_empty() && !sessions.iter().any(|known| known == id) {
            sessions.push(id.to_owned());
        }
    };
    if let Some(start) = value.pointer("/note/start") {
        let native = matches!(start["agent_id"].as_str(), Some("claude" | "pi"))
            || matches!(start["dialect"].as_str(), Some("claude" | "pi"));
        if let Some(resume) = start["resume"].as_str()
            && (!native || uuid::Uuid::parse_str(resume).is_ok())
        {
            note(resume);
        }
        if native {
            for id in [
                start["native_session_id"].as_str(),
                start["native_start"]["new_session_id"].as_str(),
            ]
            .into_iter()
            .flatten()
            {
                if uuid::Uuid::parse_str(id).is_ok() {
                    note(id);
                }
            }
        }
    }
    if let Some(message) = value.get("msg")
        && message.get("jsonrpc").is_none()
        && message.get("type").is_some()
        && let Some(id) = message["session_id"].as_str()
    {
        note(id);
    }
    walk_objects(value, &mut |object| {
        if let Some(id) = object.get("sessionId").and_then(Value::as_str) {
            note(id);
        }
    });
}

fn walk_objects(value: &Value, visit: &mut impl FnMut(&Map<String, Value>)) {
    match value {
        Value::Object(object) => {
            visit(object);
            for child in object.values() {
                walk_objects(child, visit);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_objects(item, visit);
            }
        }
        _ => {}
    }
}

fn replace_strings(value: &mut Value, needles: &[(String, String)]) {
    match value {
        Value::String(text) => {
            for (needle, placeholder) in needles {
                if text.contains(needle.as_str()) {
                    *text = text.replace(needle.as_str(), placeholder);
                }
            }
            if let Some(timestamp) = TIMESTAMP.as_ref()
                && timestamp.is_match(text)
            {
                *text = timestamp.replace_all(text, "<TIME>").into_owned();
            }
            if let Some(home) = WINDOWS_HOME.as_ref()
                && home.is_match(text)
            {
                *text = home.replace_all(text, "<HOME>").into_owned();
            }
        }
        Value::Array(items) => {
            for item in items {
                replace_strings(item, needles);
            }
        }
        Value::Object(object) => {
            for child in object.values_mut() {
                replace_strings(child, needles);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn is_local_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"._%+-".contains(&byte)
}

fn is_domain_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-'
}

/// A dotted domain whose last label is at least two letters, so `react@18.3.1` and `user@localhost`
/// are not emails.
fn is_domain(domain: &[u8]) -> bool {
    let Some(dot) = domain.iter().rposition(|&byte| byte == b'.') else {
        return false;
    };
    let tld = &domain[dot + 1..];
    dot > 0 && tld.len() >= 2 && tld.iter().all(u8::is_ascii_alphabetic)
}

/// Whether `text` holds `local@domain.tld` (the spike's email rules).
fn has_email(text: &str) -> bool {
    let bytes = text.as_bytes();
    for (at, _) in text.match_indices('@') {
        let mut start = at;
        while start > 0 && is_local_char(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = at + 1;
        while end < bytes.len() && is_domain_char(bytes[end]) {
            end += 1;
        }
        while end > at + 1 && matches!(bytes[end - 1], b'.' | b'-') {
            end -= 1;
        }
        if start < at && is_domain(&bytes[at + 1..end]) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const HOME: &str = "/Users/someone";
    const CWD: &str = "/Users/someone/scratch/repo-a";

    fn log(records: &[Value]) -> String {
        records.iter().map(|record| format!("{record}\n")).collect()
    }

    #[test]
    fn placeholders_replace_paths_sessions_timestamps_and_t() {
        let text = log(&[
            json!({"t": 1_792_840_001_230_i64, "d": "note", "note": {"spawn": {"label": "hermes acp",
                "program": "/Users/someone/.local/bin/hermes", "cwd": CWD, "pid": 5001}}}),
            json!({"t": 1_792_840_001_231_i64, "d": "note", "note": {"start": {"agent_id": "hermes",
                "resume": "4f7c-old-session", "spawn_id": "019a0000-0000-7000-8000-000000000001"}}}),
            json!({"t": 1_792_840_001_240_i64, "d": "out", "msg": {"jsonrpc": "2.0", "id": 2,
                "method": "session/load", "params": {"sessionId": "4f7c-old-session", "cwd": CWD}}}),
            json!({"t": 1_792_840_001_300_i64, "d": "in", "msg": {"jsonrpc": "2.0", "method": "session/update",
                "params": {"sessionId": "4f7c-old-session", "update": {"sessionUpdate": "session_info_update",
                "title": "Fix", "updatedAt": "2026-10-24T10:00:00.123Z"}}}}),
            json!({"t": 1_792_840_001_400_i64, "d": "in", "msg": {"jsonrpc": "2.0", "id": 3, "result": {
                "sessionId": "9b1e-new-session", "_meta": {"transcript":
                "/Users/someone/.claude/projects/-Users-someone-scratch-repo-a/9b1e-new-session.jsonl",
                "host": "someones-macbook", "who": "someone"}}}}),
            json!({"t": 1_792_840_001_500_i64, "d": "err", "line": "INFO started at 2026-10-24 10:00:01"}),
        ]);
        let curated = curate(&text, HOME, CWD, "someone", "someones-macbook").expect("clean");
        let lines: Vec<Value> = curated
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(lines[0]["t"], 0);
        assert_eq!(lines[5]["t"], 270);
        assert_eq!(
            lines[0]["note"]["spawn"]["program"],
            "<HOME>/.local/bin/hermes"
        );
        assert_eq!(lines[0]["note"]["spawn"]["cwd"], "<CWD>");
        assert_eq!(lines[1]["note"]["start"]["resume"], "<SESSION_1>");
        assert_eq!(
            lines[1]["note"]["start"]["spawn_id"],
            "019a0000-0000-7000-8000-000000000001"
        );
        assert_eq!(
            lines[2]["msg"]["params"],
            json!({"sessionId": "<SESSION_1>", "cwd": "<CWD>"})
        );
        assert_eq!(lines[3]["msg"]["params"]["update"]["updatedAt"], "<TIME>");
        assert_eq!(
            lines[4]["msg"]["result"]["_meta"],
            json!({"transcript": "<HOME>/.claude/projects/<CWD_ENC>/<SESSION_2>.jsonl",
                   "host": "<HOST>", "who": "<USER>"})
        );
        assert_eq!(lines[5]["line"], "INFO started at <TIME>");
        assert!(scan_clean(&curated).is_empty());
    }

    /// An npm adapter runs in its install folder (spec 4), which the spawn note names as its `cwd`,
    /// under whatever data root the run used: a fixture names it `<ADAPTER>`, as it names the chat
    /// folder `<CWD>`.
    #[test]
    fn an_adapters_install_folder_is_a_placeholder() {
        let adapter =
            "/private/tmp/runs/session-1/data-root/adapters/agentclientprotocol-codex-acp-2.2.2";
        let text = log(&[
            json!({"t": 1, "d": "note", "note": {"spawn": {"label": "codex-acp",
                "program": "/opt/homebrew/bin/node",
                "args": [format!("{adapter}/node_modules/@agentclientprotocol/codex-acp/dist/index.js")],
                "cwd": adapter, "pid": 5001}}}),
            json!({"t": 2, "d": "out", "msg": {"jsonrpc": "2.0", "id": 2, "method": "session/new",
                "params": {"cwd": CWD, "mcpServers": []}}}),
            json!({"t": 3, "d": "err", "line": format!("loaded {adapter}/node_modules/x.js")}),
        ]);
        let curated = curate(&text, HOME, CWD, "someone", "host-name").expect("clean");
        let lines: Vec<Value> = curated
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(lines[0]["note"]["spawn"]["cwd"], "<ADAPTER>");
        assert_eq!(
            lines[0]["note"]["spawn"]["args"],
            json!(["<ADAPTER>/node_modules/@agentclientprotocol/codex-acp/dist/index.js"])
        );
        assert_eq!(lines[1]["msg"]["params"]["cwd"], "<CWD>");
        assert_eq!(lines[2]["line"], "loaded <ADAPTER>/node_modules/x.js");
    }

    /// Recorded on CI's Windows runner: its temp folder is spelled with the profile's 8.3 short
    /// name, which the home needle (the long name) misses, so the scan refused the fixture.
    #[test]
    fn windows_profile_folders_in_any_spelling_become_home() {
        let text = log(&[
            json!({"t": 1, "d": "note", "note": {"spawn": {"label": "hermes acp",
                "program": r"C:\Users\RUNNER~1\AppData\Local\Temp\.tmpA\bin\hermes.exe",
                "args": [r"\\?\C:\Users\runneradmin\AppData\Local\x.js", "c:/users/Someone/x"],
                "cwd": CWD, "pid": 5001}}}),
        ]);
        let curated = curate(
            &text,
            r"C:\Users\runneradmin",
            CWD,
            "runneradmin",
            "host-name",
        )
        .expect("clean");
        let line: Value =
            serde_json::from_str(curated.lines().next().expect("a line")).expect("json");
        assert_eq!(
            line["note"]["spawn"]["program"],
            r"<HOME>\AppData\Local\Temp\.tmpA\bin\hermes.exe"
        );
        assert_eq!(
            line["note"]["spawn"]["args"],
            json!([r"<HOME>\AppData\Local\x.js", "<HOME>/x"])
        );
    }

    #[test]
    fn what_redaction_misses_fails_the_scan() {
        // A home folder this machine does not own, and token shapes nothing replaced.
        let text = log(&[json!({"d": "err", "line": "loading /home/other/.config"})]);
        let error = curate(&text, HOME, CWD, "someone", "host-name").expect_err("unclean");
        assert_eq!(
            error,
            CurateError::Unclean {
                findings: vec!["line 1: a home folder path".into()]
            }
        );
        assert!(matches!(
            curate("not json\n", HOME, CWD, "u", "h"),
            Err(CurateError::NotARecord { line: 1 })
        ));
        assert!(matches!(
            curate("[1]\n", HOME, CWD, "u", "h"),
            Err(CurateError::NotARecord { line: 1 })
        ));
    }

    #[test]
    fn a_capped_record_is_refused_with_its_line() {
        let text = log(&[
            json!({"t": 1, "d": "err", "line": "INFO ready"}),
            json!({"t": 2, "d": "in", "raw": "{\"jsonrpc\":\"2.0\",\"method\":\"session/upd",
                "truncated": 131_072}),
        ]);
        let error = curate(&text, HOME, CWD, "someone", "host-name").expect_err("capped");
        assert_eq!(
            error,
            CurateError::Capped {
                line: 2,
                cut: 131_072
            }
        );
        assert!(error.to_string().contains("cannot replay"), "{error}");
    }

    #[test]
    fn the_scan_finds_emails_tokens_and_home_paths_without_quoting_them() {
        let findings = scan_clean(
            "fine line\n\
             contact a.b+c@example.com\n\
             key sk-ant-api03-abcdefghijklmn\n\
             gh ghp_abcdefghijklmnopqrstuvwxyz\n\
             jwt eyJhbGciOiJ.eyJzdWIiOiIx.c2lnbmF0dXJl\n\
             auth Bearer abcdefghijklmnopqrstuvwx\n\
             aws AKIAABCDEFGHIJKLMNOP\n\
             path /Users/x/y and C:\\Users\\x\n",
        );
        assert_eq!(
            findings,
            [
                "line 2: an email address",
                "line 3: an Anthropic key",
                "line 3: an API key",
                "line 4: a GitHub token",
                "line 5: a JWT",
                "line 6: a bearer token",
                "line 7: an AWS key",
                "line 8: a home folder path",
            ]
        );
        assert!(
            !findings
                .iter()
                .any(|finding| finding.contains("example.com"))
        );
        // Lookalikes stay.
        assert!(
            scan_clean("@anthropic-ai/claude-code react@18.3.1 user@localhost a@b.c <EMAIL>")
                .is_empty()
        );
    }

    #[test]
    fn project_dir_encoding_matches_claude_code() {
        assert_eq!(
            encode_project_dir("/Users/me/Desktop/coding-folder/auracode"),
            "-Users-me-Desktop-coding-folder-auracode"
        );
        assert_eq!(encode_project_dir("/tmp/a_b.c"), "-tmp-a-b-c");
    }

    #[test]
    fn short_needles_are_skipped() {
        let text = log(&[json!({"d": "err", "line": "the bob ran in /srv/x"})]);
        let curated = curate(&text, "", "/srv/x", "bob", "").expect("clean");
        assert!(curated.contains("the bob ran in <CWD>"), "{curated}");
    }
    #[test]
    fn native_session_envelopes_and_start_ids_share_placeholders() {
        let main = "019a0000-0000-7000-8000-000000000001";
        let fork = "019a0000-0000-7000-8000-000000000002";
        let text = log(&[
            json!({"d":"note","note":{"start":{"agent_id":"claude","resume":main,"native_session_id":fork,"native_start":{"new_session_id":fork,"fork":true}}}}),
            json!({"d":"in","msg":{"type":"system","subtype":"init","session_id":fork}}),
            json!({"d":"out","msg":{"type":"user","session_id":fork,"message":{"content":"hello"}}}),
            json!({"d":"in","msg":{"type":"control_request","request":{"input":{"session_id":"opaque-tool-value"}}}}),
        ]);
        let text = curate(&text, "", "", "", "").expect("native curation");
        let lines = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("JSON"))
            .collect::<Vec<_>>();
        assert_eq!(lines[0]["note"]["start"]["resume"], "<SESSION_1>");
        assert_eq!(
            lines[0]["note"]["start"]["native_session_id"],
            "<SESSION_2>"
        );
        assert_eq!(
            lines[0]["note"]["start"]["native_start"]["new_session_id"],
            "<SESSION_2>"
        );
        assert_eq!(lines[1]["msg"]["session_id"], "<SESSION_2>");
        assert_eq!(lines[2]["msg"]["session_id"], "<SESSION_2>");
        assert_eq!(
            lines[3]["msg"]["request"]["input"]["session_id"],
            "opaque-tool-value"
        );
    }

    fn partial(parent: Option<&str>, index: u64, text: &str) -> Value {
        json!({"d":"in","msg":{"type":"stream_event","parent_tool_use_id":parent,
            "event":{"type":"content_block_delta","index":index,
                "delta":{"type":"input_json_delta","partial_json":text}}}})
    }

    fn input_text(records: &[Value], parent: Option<&str>) -> String {
        records
            .iter()
            .filter(|record| record["msg"]["parent_tool_use_id"].as_str() == parent)
            .filter_map(|record| {
                record
                    .pointer("/msg/event/delta/partial_json")
                    .and_then(Value::as_str)
            })
            .collect()
    }

    #[test]
    fn native_split_tool_inputs_redact_escaped_paths_without_mixing_subagents() {
        let windows_home = r"C:\Users\someone";
        let windows_cwd = r"C:\Users\someone\scratch";
        let main =
            json!({"file_path":format!(r"{windows_cwd}\a.txt"),"content":"héllo"}).to_string();
        let subagent = json!({"file_path":format!(r"{windows_home}\b.txt")}).to_string();
        let split_main = main.find("Users").expect("path") + 2;
        let split_subagent = subagent.find("someone").expect("user") + 3;
        let records = [
            partial(None, 0, &main[..split_main]),
            partial(Some("tool-child"), 0, &subagent[..split_subagent]),
            partial(None, 0, &main[split_main..]),
            partial(Some("tool-child"), 0, &subagent[split_subagent..]),
        ];
        let curated = curate(
            &log(&records),
            windows_home,
            windows_cwd,
            "someone",
            "host-name",
        )
        .expect("curated native input");
        let curated: Vec<Value> = curated
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON"))
            .collect();
        assert_eq!(curated.len(), records.len());
        let main = input_text(&curated, None);
        let subagent = input_text(&curated, Some("tool-child"));
        assert_eq!(
            serde_json::from_str::<Value>(&main).expect("main JSON"),
            json!({"file_path":r"<CWD>\a.txt","content":"héllo"})
        );
        assert_eq!(
            serde_json::from_str::<Value>(&subagent).expect("subagent JSON"),
            json!({"file_path":r"<HOME>\b.txt"})
        );
        assert!(scan_clean(&main).is_empty());
        assert!(scan_clean(&subagent).is_empty());
    }

    #[test]
    fn native_tool_json_keeps_unchanged_chunks_and_redacts_interrupted_values() {
        let safe = [
            partial(None, 0, "{\"text\": \""),
            partial(None, 0, "safe\"}"),
        ];
        let curated = curate(&log(&safe), "", "", "", "").expect("safe input");
        assert_eq!(curated, log(&safe));

        let incomplete = [
            partial(None, 0, r#"{"file_path":"C:\\Us"#),
            partial(None, 0, r#"ers\\someone\\scratch\\a.txt"#),
        ];
        let curated = curate(
            &log(&incomplete),
            r"C:\Users\someone",
            r"C:\Users\someone\scratch",
            "someone",
            "",
        )
        .expect("interrupted input");
        let records = curated
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON"))
            .collect::<Vec<_>>();
        assert_eq!(input_text(&records, None), r#"{"file_path":"<CWD>\\a.txt"#);
    }

    #[test]
    fn native_split_unknown_home_path_is_refused_without_disclosing_it() {
        let records = [
            partial(None, 0, "{\"path\":\"/ho"),
            partial(None, 0, "me/other/file\"}"),
        ];
        let error = curate(&log(&records), HOME, CWD, "someone", "").expect_err("unknown home");
        assert!(error.to_string().contains("native tool input block"));
        assert!(!error.to_string().contains("other"));
    }

    #[test]
    fn pi_tool_argument_deltas_are_redacted_after_reassembly() {
        let delta = |text: &str| {
            json!({"d":"in","msg":{"type":"message_update","assistantMessageEvent":{
            "type":"toolcall_delta","contentIndex":0,"delta":text}}})
        };
        let records = [
            json!({"d":"in","msg":{"type":"message_start","message":{"role":"assistant"}}}),
            delta(r#"{"path":"C:\\Us"#),
            delta(r#"ers\\someone\\scratch\\a.txt"}"#),
        ];
        let curated = curate(
            &log(&records),
            r"C:\Users\someone",
            r"C:\Users\someone\scratch",
            "someone",
            "",
        )
        .unwrap();
        let value: String = curated
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter_map(|record| {
                record
                    .pointer("/msg/assistantMessageEvent/delta")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        assert_eq!(
            serde_json::from_str::<Value>(&value).unwrap(),
            json!({"path":r"<CWD>\a.txt"})
        );
        assert!(scan_clean(&value).is_empty());
    }
}
