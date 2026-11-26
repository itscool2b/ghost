//! Protocol logs: one JSON Lines file per agent process, for diagnostics and replay fixtures.
//!
//! `<profile>/logs/protocol/<chat_id>/<spawn_id>.jsonl` (`spawn_id` is a UUIDv7, so names sort by
//! time), rotated to `.1.jsonl` and `.2.jsonl`. Records, in the order they happened:
//!
//! ```text
//! {"t":1792840001230,"d":"note","note":{"spawn":{"label":"hermes acp","program":"<abs path>","args":["acp"],"cwd":"...","removed":["CLAUDECODE"],"set":[],"pid":5001}}}
//! {"t":1792840001240,"d":"out","msg":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{...}}}
//! {"t":1792840001900,"d":"in","msg":{"jsonrpc":"2.0","id":1,"result":{...}}}
//! {"t":1792840002001,"d":"err","line":"INFO gateway ready"}
//! {"t":1792840003000,"d":"cmd","cmd":{"type":"prompt","turn_id":"019a...","parts":[...]}}
//! {"t":1792840003500,"d":"in","raw":"{\"jsonrpc\":\"2.0\",\"id\":"}
//! {"t":1792840009000,"d":"note","note":{"noise":{"bytes":40}}}
//! {"t":1792840010000,"d":"note","note":{"exit":{"code":0,"signal":null}}}
//! ```
//!
//! `d` is `out` (to the agent), `in` (from it; `msg` for JSON, `raw` for a line the driver got that
//! is not valid JSON), `err`, `cmd` (a driver command, in processing order), or `note`; `t` is for
//! humans, replay uses line order. A stdout line that is not JSON at all never reaches the driver,
//! so it is not an `in` record either: it leaves a `noise` note with its size.
//!
//! A record over the size cap, whatever its `d`, has exactly these keys and nothing to replay:
//!
//! ```text
//! {"t":1792840004000,"d":"in","raw":"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",...","truncated":2097152}
//! ```
//!
//! `d` is the record's own; `raw` is the first half of its payload as text (a `msg`, `cmd` or `note`
//! serialized first, redacted before it is cut); `truncated` is the number of bytes cut.
//!
//! Writing never blocks the protocol path: records queue to a writer task, which redacts them
//! ([`Redactor`]) before any byte reaches disk. A full queue drops and counts, and a
//! `{"note":{"dropped":n}}` record marks the gap, at the latest when the log closes; an I/O error
//! stops logging for that spawn with one warning. The env is never logged.

mod bundle;
mod redact;
mod retention;
mod writer;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

pub use redact::Redactor;

/// Size limits of one spawn's log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLimits {
    /// A longer record keeps its first half and says how much was cut.
    pub max_record_bytes: usize,
    /// A file this large is rotated.
    pub max_file_bytes: u64,
    /// Files kept per spawn, the current one included.
    pub max_files: u32,
    /// Records waiting for the writer; more are dropped and counted.
    pub queue: usize,
}

impl LogLimits {
    /// 256 KiB records, 8 MiB files, 3 files, 4,096 queued records.
    pub const STANDARD: LogLimits = LogLimits {
        max_record_bytes: 256 * 1024,
        max_file_bytes: 8 * 1024 * 1024,
        max_files: 3,
        queue: 4096,
    };

    /// The bytes all queued records may hold together: 64 records at the cap (16 MiB for
    /// [`STANDARD`](Self::STANDARD)), so a flood of large lines drops and counts instead of growing
    /// memory, however many records the count limit would allow.
    fn queue_bytes(&self) -> usize {
        self.max_record_bytes.saturating_mul(64).max(1024 * 1024)
    }
}

/// Lines up to this size reach the writer whole, so the key rules see their structure; a longer
/// line is cut to the record cap before it is queued (it is written cut in any case).
const PARSE_LIMIT: usize = 4 * 1024 * 1024;

/// What retention keeps across a profile's protocol logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub max_total_bytes: u64,
    pub max_age: Duration,
}

impl RetentionPolicy {
    /// 256 MiB in all, 14 days.
    pub const STANDARD: RetentionPolicy = RetentionPolicy {
        max_total_bytes: 256 * 1024 * 1024,
        max_age: Duration::from_secs(14 * 24 * 60 * 60),
    };
}

/// What [`LogFactory::prune`] removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionReport {
    pub files_removed: u32,
    pub bytes_removed: u64,
    pub errors: Vec<String>,
}

/// The logs still being written, as `(chat dir, spawn id)`: retention never deletes their files.
type LiveLogs = Arc<Mutex<HashSet<(PathBuf, String)>>>;

/// Opens protocol logs under one root (`<profile>/logs/protocol`).
#[derive(Debug, Clone)]
pub struct LogFactory {
    root: PathBuf,
    redactor: Arc<Redactor>,
    limits: LogLimits,
    rt: Handle,
    live: LiveLogs,
}

impl LogFactory {
    /// `root` is `<profile>/logs/protocol`; writer tasks run on `rt`.
    pub fn new(root: PathBuf, redactor: Arc<Redactor>, rt: Handle) -> LogFactory {
        LogFactory {
            root,
            redactor,
            limits: LogLimits::STANDARD,
            rt,
            live: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// The same factory with other limits (tests: small files and queues).
    #[cfg(test)]
    pub(crate) fn with_limits(mut self, limits: LogLimits) -> LogFactory {
        self.limits = limits;
        self
    }

    /// The log of one spawn. Never fails: names that are not plain file names, and an I/O error
    /// later (with one warning), leave the log disabled while the spawn goes ahead.
    pub fn open(&self, chat_id: &str, spawn_id: &str) -> ProtocolLog {
        if !is_plain_name(chat_id) || !is_plain_name(spawn_id) {
            tracing::warn!(
                chat_id,
                spawn_id,
                "a protocol log needs plain file names; this agent's log is off"
            );
            return ProtocolLog::disabled_with_id(spawn_id.to_owned());
        }
        let dir = self.chat_dir(chat_id);
        let path = dir.join(format!("{spawn_id}.jsonl"));
        let (tx, rx) = mpsc::channel(self.limits.queue.max(1));
        let shared = Arc::new(SinkShared::default());
        lock(&self.live).insert((dir.clone(), spawn_id.to_owned()));
        let writer = writer::Writer::new(
            dir,
            spawn_id.to_owned(),
            self.limits,
            Arc::clone(&self.redactor),
            Arc::clone(&shared),
        );
        let live = Arc::clone(&self.live);
        self.rt.spawn(writer::run(rx, writer, live));
        ProtocolLog {
            inner: Arc::new(LogInner {
                spawn_id: spawn_id.to_owned(),
                sink: Some(Sink {
                    tx,
                    shared,
                    path,
                    limits: self.limits,
                }),
            }),
        }
    }

    /// `<root>/<chat_id>`, the folder "Show log" reveals.
    pub fn chat_dir(&self, chat_id: &str) -> PathBuf {
        self.root.join(chat_id)
    }

    /// Deletes logs older than the policy's age, then the oldest files until the total fits. The
    /// files of logs still being written are never deleted.
    pub async fn prune(&self, policy: RetentionPolicy) -> RetentionReport {
        let root = self.root.clone();
        let live = lock(&self.live).clone();
        let now = std::time::SystemTime::now();
        match self
            .rt
            .spawn_blocking(move || retention::prune(&root, policy, &live, now))
            .await
        {
            Ok(report) => report,
            Err(error) => RetentionReport {
                errors: vec![format!("retention stopped: {error}")],
                ..RetentionReport::default()
            },
        }
    }

    /// Resolves once every log this factory opened has written its last record and closed.
    #[cfg(test)]
    pub(crate) async fn idle(&self) {
        for _ in 0..2000 {
            if lock(&self.live).is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("protocol log writers still running");
    }

    /// Resolves once everything queued on `log` so far is on disk (or its logging stopped).
    #[cfg(test)]
    pub(crate) async fn idle_writes(&self, log: &ProtocolLog) {
        let Some(sink) = log.inner.sink.as_ref() else {
            return;
        };
        for _ in 0..2000 {
            if sink.shared.queued_bytes.load(Ordering::Acquire) == 0
                || sink.shared.stopped.load(Ordering::Acquire)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("protocol log writes still queued");
    }
}

/// A chat id or spawn id that is safe as one path component.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// One spawn's log. Cloning shares it; every method is non-blocking and callable from any thread.
#[derive(Clone)]
pub struct ProtocolLog {
    inner: Arc<LogInner>,
}

struct LogInner {
    spawn_id: String,
    sink: Option<Sink>,
}

/// The producer's end of an open log.
struct Sink {
    tx: mpsc::Sender<Record>,
    shared: Arc<SinkShared>,
    path: PathBuf,
    limits: LogLimits,
}

/// What the producers and the writer share.
#[derive(Debug, Default)]
pub(crate) struct SinkShared {
    /// Records dropped since the last `dropped` note.
    dropped: AtomicU64,
    /// Bytes of the records waiting for the writer.
    queued_bytes: AtomicUsize,
    /// Set by the writer after an I/O error: logging for this spawn is over.
    stopped: AtomicBool,
}

impl SinkShared {
    /// The writer finished with a record of this size.
    pub(crate) fn release(&self, bytes: usize) {
        self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
    }

    /// How many records were dropped since the last `dropped` note, and starts counting again.
    pub(crate) fn take_dropped(&self) -> u64 {
        self.dropped.swap(0, Ordering::AcqRel)
    }

    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
}

/// One record on its way to the writer.
#[derive(Debug)]
pub(crate) struct Record {
    /// Unix ms when it was produced.
    pub(crate) t: i64,
    pub(crate) body: Body,
    /// What it counts against the queue's byte budget.
    pub(crate) bytes: usize,
}

#[derive(Debug)]
pub(crate) enum Body {
    /// A line through a pipe. `text` was cut before queueing when `total` is larger.
    Line {
        direction: Direction,
        text: String,
        total: usize,
    },
    Command(serde_json::Value),
    Note(LogNote),
    Dropped(u64),
}

impl ProtocolLog {
    /// A log that writes nothing, carrying a fresh UUIDv7 spawn id.
    pub fn disabled() -> ProtocolLog {
        ProtocolLog::disabled_with_id(uuid::Uuid::now_v7().hyphenated().to_string())
    }

    /// A log that writes nothing, carrying `spawn_id`: a replay spawns with the recorded id, so the
    /// ids a driver mints outside a turn match the golden events.
    pub fn disabled_with_id(spawn_id: impl Into<String>) -> ProtocolLog {
        ProtocolLog {
            inner: Arc::new(LogInner {
                spawn_id: spawn_id.into(),
                sink: None,
            }),
        }
    }

    /// Copied into [`SpawnedProcess::spawn_id`](crate::spawn::SpawnedProcess::spawn_id).
    pub fn spawn_id(&self) -> &str {
        &self.inner.spawn_id
    }

    /// One line through a pipe. Non-blocking: drops and counts when the queue is full.
    pub fn line(&self, direction: Direction, line: &str) {
        let Some(sink) = self.sink() else {
            return;
        };
        let text = if line.len() <= PARSE_LIMIT {
            line.to_owned()
        } else {
            head(line, sink.limits.max_record_bytes).to_owned()
        };
        let bytes = text.len();
        sink.offer(Record {
            t: now_millis(),
            body: Body::Line {
                direction,
                text,
                total: line.len(),
            },
            bytes,
        });
    }

    /// A driver command, as a `cmd` record (see `LoggedCommand` in ghost-drivers).
    pub fn command(&self, command: serde_json::Value) {
        let Some(sink) = self.sink() else {
            return;
        };
        let bytes = approx_bytes(&command);
        sink.offer(Record {
            t: now_millis(),
            body: Body::Command(command),
            bytes,
        });
    }

    pub fn note(&self, note: LogNote) {
        let Some(sink) = self.sink() else {
            return;
        };
        sink.offer(Record {
            t: now_millis(),
            body: Body::Note(note),
            bytes: 256,
        });
    }

    /// The current file, while logging is on.
    pub fn path(&self) -> Option<PathBuf> {
        self.sink().map(|sink| sink.path.clone())
    }

    /// The open sink, unless logging is off or stopped.
    fn sink(&self) -> Option<&Sink> {
        self.inner
            .sink
            .as_ref()
            .filter(|sink| !sink.shared.stopped.load(Ordering::Acquire))
    }
}

impl Sink {
    /// Queues a record, first the count of what was dropped before it, if anything was.
    fn offer(&self, record: Record) {
        let pending = self.shared.take_dropped();
        if pending > 0 {
            let marker = Record {
                t: record.t,
                body: Body::Dropped(pending),
                bytes: 64,
            };
            if !self.try_queue(marker) {
                self.shared
                    .dropped
                    .fetch_add(pending.saturating_add(1), Ordering::AcqRel);
                return;
            }
        }
        if !self.try_queue(record) {
            self.shared.dropped.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn try_queue(&self, record: Record) -> bool {
        let bytes = record.bytes;
        let before = self.shared.queued_bytes.fetch_add(bytes, Ordering::AcqRel);
        if before > 0 && before.saturating_add(bytes) > self.limits.queue_bytes() {
            self.shared.release(bytes);
            return false;
        }
        if self.tx.try_send(record).is_err() {
            self.shared.release(bytes);
            return false;
        }
        true
    }
}

impl std::fmt::Debug for ProtocolLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtocolLog")
            .field("spawn_id", &self.inner.spawn_id)
            .field("path", &self.path())
            .finish()
    }
}

/// Which way a line went: the `d` of its record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// To the agent's stdin.
    Out,
    /// From its stdout.
    In,
    /// From its stderr.
    Err,
}

impl Direction {
    /// The record's `d`.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Direction::Out => "out",
            Direction::In => "in",
            Direction::Err => "err",
        }
    }
}

/// A `note` record: `{"note":{"<kind>":...}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogNote {
    /// Names only: `removed` are the scrubbed variables, `set` the spawn's own.
    Spawn {
        label: String,
        program: String,
        args: Vec<String>,
        cwd: String,
        removed: Vec<String>,
        set: Vec<String>,
        pid: u32,
    },
    /// The driver's session setup, secrets excluded (no env), written right after the spawn note.
    Start(serde_json::Value),
    Exit {
        code: Option<i32>,
        signal: Option<i32>,
    },
    Oversized {
        bytes: u64,
        method: Option<String>,
    },
    /// A stdout line that was not JSON, dropped.
    Noise {
        bytes: usize,
    },
    Shutdown {
        steps: Vec<String>,
        took_ms: u64,
    },
    Text(String),
}

/// The longest prefix of `text` of at most `max` bytes that ends on a character boundary.
pub(crate) fn head(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default()
}

/// Roughly the bytes a JSON value serializes to: its strings plus a little per item.
fn approx_bytes(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(text) => text.len() + 2,
        serde_json::Value::Array(items) => items.iter().map(approx_bytes).sum::<usize>() + 2,
        serde_json::Value::Object(fields) => {
            fields
                .iter()
                .map(|(key, field)| key.len() + 4 + approx_bytes(field))
                .sum::<usize>()
                + 2
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => 8,
    }
}

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Locks `mutex`, recovering the data of a thread that panicked while holding it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::*;

    fn factory(root: &Path) -> LogFactory {
        LogFactory::new(
            root.to_path_buf(),
            Arc::new(Redactor::standard()),
            Handle::current(),
        )
    }

    /// Every record of a log file, parsed.
    fn records(path: &Path) -> Vec<Value> {
        let text = std::fs::read_to_string(path).expect("log file");
        text.lines()
            .map(|line| serde_json::from_str(line).expect("each record is JSON"))
            .collect()
    }

    fn without_t(mut record: Value) -> Value {
        let t = record
            .as_object_mut()
            .and_then(|fields| fields.remove("t"))
            .expect("every record has t");
        assert!(t.as_i64().is_some_and(|t| t > 1_700_000_000_000), "{t}");
        record
    }

    #[tokio::test]
    async fn image_payloads_never_reach_protocol_log_files_even_when_cut_before_their_type() {
        let directory = tempfile::tempdir().expect("directory");
        let factory = factory(directory.path());
        let log = factory.open("chat", "images");
        let path = log.path().expect("logging enabled");
        log.line(Direction::In, r#"{"method":"session/update","content":[{"type":"image","mimeType":"image/png","data":"PRIVATE_SMALL_IMAGE"},{"type":"text","text":"Visible explanation"}],"apiKey":"private-key"}"#);
        for (head, tail) in [
            (
                r#"{"method":"session/update","content":{"data":""#,
                r#"","type":"image"}}"#,
            ),
            (
                r#"{"message":{"content":[{"type":"image","source":{"data":""#,
                r#"","media_type":"image/png"}}]}}"#,
            ),
            (
                r#"{"content":{"resource":{"blob":""#,
                r#"","mimeType":"image/webp"},"type":"resource"}}"#,
            ),
            (r#"{"item":{"result":""#, r#"","type":"imageGeneration"}}"#),
            (
                r#"{"resource":{"uri":"data:\u0069mage\/png;base64,"#,
                r#"","mimeType":"image/png"}}"#,
            ),
        ] {
            let payload = "QUJDREVGR0hJSktM".repeat(PARSE_LIMIT / 16 + 1);
            log.line(Direction::In, &format!("{head}{payload}{tail}"));
            factory.idle_writes(&log).await;
        }
        drop(log);
        factory.idle().await;
        let text = std::fs::read_to_string(&path).expect("saved log");
        assert!(!text.contains("PRIVATE_SMALL_IMAGE"));
        assert!(
            !text.contains("QUJD"),
            "even encoded prefixes must be absent"
        );
        assert!(!text.contains("private-key"));
        assert!(text.contains("Visible explanation"));
        assert!(text.contains("image/png"));
        assert_eq!(records(&path).len(), 6);
        assert_eq!(
            records(&path)[0]["msg"]["content"][0]["data"],
            "<IMAGE_DATA>"
        );
        assert!(
            records(&path)
                .iter()
                .skip(1)
                .all(|record| record["truncated"].as_u64().is_some_and(|cut| cut > 0))
        );
    }

    #[test]
    fn notes_serialize_as_the_record_format_says() {
        let spawn = LogNote::Spawn {
            label: "hermes acp".into(),
            program: "/usr/local/bin/hermes".into(),
            args: vec!["acp".into()],
            cwd: "/work".into(),
            removed: vec!["CLAUDECODE".into()],
            set: Vec::new(),
            pid: 5001,
        };
        assert_eq!(
            serde_json::to_value(&spawn).expect("serialize"),
            json!({"spawn": {"label": "hermes acp", "program": "/usr/local/bin/hermes",
                "args": ["acp"], "cwd": "/work", "removed": ["CLAUDECODE"], "set": [], "pid": 5001}})
        );
        assert_eq!(
            serde_json::to_value(LogNote::Exit {
                code: Some(0),
                signal: None
            })
            .expect("serialize"),
            json!({"exit": {"code": 0, "signal": null}})
        );
        assert_eq!(
            serde_json::to_value(LogNote::Oversized {
                bytes: 71_303_168,
                method: Some("session/update".into())
            })
            .expect("serialize"),
            json!({"oversized": {"bytes": 71303168, "method": "session/update"}})
        );
        assert_eq!(
            serde_json::to_value(Direction::Err).expect("serialize"),
            json!("err")
        );
    }

    #[test]
    fn a_disabled_log_has_a_v7_spawn_id_and_no_file() {
        let log = ProtocolLog::disabled();
        let id = uuid::Uuid::parse_str(log.spawn_id()).expect("uuid");
        assert_eq!(id.get_version_num(), 7);
        assert_ne!(log.spawn_id(), ProtocolLog::disabled().spawn_id());
        log.line(Direction::In, "{}");
        log.note(LogNote::Text("x".into()));
        log.command(json!({"type": "interrupt"}));
        assert_eq!(log.path(), None);
    }

    #[test]
    fn a_disabled_log_can_carry_a_given_spawn_id() {
        let log = ProtocolLog::disabled_with_id("019a0000-0000-7000-8000-00000000abcd");
        assert_eq!(log.spawn_id(), "019a0000-0000-7000-8000-00000000abcd");
        assert_eq!(log.clone().spawn_id(), log.spawn_id());
        assert_eq!(log.path(), None);
    }

    #[tokio::test]
    async fn records_are_written_in_order_in_the_documented_format() {
        let dir = tempfile::tempdir().expect("tempdir");
        let factory = factory(dir.path());
        let log = factory.open("chat-1", "019a0000-0000-7000-8000-000000000001");
        assert_eq!(log.spawn_id(), "019a0000-0000-7000-8000-000000000001");
        let path = log.path().expect("logging is on");
        assert_eq!(
            path,
            dir.path()
                .join("chat-1")
                .join("019a0000-0000-7000-8000-000000000001.jsonl")
        );
        assert_eq!(factory.chat_dir("chat-1"), dir.path().join("chat-1"));
        log.note(LogNote::Spawn {
            label: "fake acp".into(),
            program: "/bin/fake".into(),
            args: vec!["acp".into()],
            cwd: "/work".into(),
            removed: vec!["CLAUDECODE".into()],
            set: vec!["CODEX_PATH".into()],
            pid: 7,
        });
        log.line(
            Direction::Out,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#,
        );
        log.line(Direction::In, r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        log.line(Direction::In, r#"{"jsonrpc":"2.0","id":"#);
        log.line(Direction::Err, "INFO gateway ready");
        log.command(json!({"type": "interrupt"}));
        log.note(LogNote::Noise { bytes: 12 });
        drop(log);
        factory.idle().await;

        let written: Vec<Value> = records(&path).into_iter().map(without_t).collect();
        assert_eq!(
            written,
            [
                json!({"d": "note", "note": {"spawn": {"label": "fake acp", "program": "/bin/fake",
                    "args": ["acp"], "cwd": "/work", "removed": ["CLAUDECODE"], "set": ["CODEX_PATH"], "pid": 7}}}),
                json!({"d": "out", "msg": {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": {"protocolVersion": 1}}}),
                json!({"d": "in", "msg": {"jsonrpc": "2.0", "id": 1, "result": {}}}),
                json!({"d": "in", "raw": r#"{"jsonrpc":"2.0","id":"#}),
                json!({"d": "err", "line": "INFO gateway ready"}),
                json!({"d": "cmd", "cmd": {"type": "interrupt"}}),
                json!({"d": "note", "note": {"noise": {"bytes": 12}}}),
            ]
        );
        // Keys keep the order they were written in.
        let first_line = std::fs::read_to_string(&path).expect("read");
        assert!(first_line.starts_with(r#"{"t":"#), "{first_line}");
        assert!(first_line.contains(r#""d":"out","msg":{"jsonrpc":"2.0","id":1,"method""#));
    }

    #[tokio::test]
    async fn redaction_happens_before_anything_reaches_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let redactor = Arc::new(Redactor::standard());
        redactor.set_secrets(vec!["sk-proj-0123456789abcdef".into()]);
        let factory = LogFactory::new(dir.path().to_path_buf(), redactor, Handle::current());
        let log = factory.open("chat", "spawn");
        let path = log.path().expect("on");
        log.line(
            Direction::In,
            r#"{"jsonrpc":"2.0","method":"_auth/status_update","params":{"authStatus":{"account":{"email":"me@example.com","organization":"Acme"}}}}"#,
        );
        log.line(
            Direction::In,
            r#"{"id":2,"result":{"usage":{"inputTokens":120,"outputTokens":30},"accessToken":"opaque"}}"#,
        );
        log.line(
            Direction::Err,
            "401: key sk-proj-0123456789abcdef rejected for me@example.com",
        );
        log.line(
            Direction::In,
            "not json mentioning ghp_0123456789abcdefghij",
        );
        log.command(json!({"type": "prompt", "parts": [{"type": "text", "text": "my token is sk-proj-0123456789abcdef"}]}));
        drop(log);
        factory.idle().await;
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(!text.contains("me@example.com"), "{text}");
        assert!(!text.contains("Acme"), "{text}");
        assert!(!text.contains("opaque"), "{text}");
        assert!(!text.contains("sk-proj"), "{text}");
        assert!(!text.contains("ghp_"), "{text}");
        let written: Vec<Value> = records(&path).into_iter().map(without_t).collect();
        assert_eq!(
            written[0]["msg"]["params"]["authStatus"]["account"],
            json!({"email": "<REDACTED>", "organization": "<REDACTED>"})
        );
        assert_eq!(
            written[1]["msg"]["result"],
            json!({"usage": {"inputTokens": 120, "outputTokens": 30}, "accessToken": "<REDACTED>"})
        );
        assert_eq!(
            written[2]["line"],
            json!("401: key <SECRET> rejected for <EMAIL>")
        );
        assert_eq!(written[3]["raw"], json!("not json mentioning <TOKEN>"));
        assert_eq!(
            written[4]["cmd"]["parts"][0]["text"],
            json!("my token is <SECRET>")
        );
    }

    /// Claude's `initialize` account and claude-acp's `_auth/status_update` never reach disk.
    #[tokio::test]
    async fn auth_status_redacted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let factory = factory(dir.path());
        let log = factory.open("chat", "auth");
        let path = log.path().expect("on");
        log.line(
            Direction::In,
            r#"{"jsonrpc":"2.0","method":"_auth/status_update","params":{"sessionId":"s1","authStatus":{"status":"authenticated","account":{"email":"someone@example.com","organization":"Acme Corp"}}}}"#,
        );
        log.line(
            Direction::In,
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentInfo":{"name":"claude-agent-acp","version":"0.95.0"},"account":{"emailAddress":"someone@example.com","organizationName":"Acme Corp","organizationUuid":"6f1d2c3b-0000-4000-8000-000000000000","accountUuid":"a1b2"},"models":[{"modelId":"opus","name":"Opus","displayName":"Claude Opus"}]}}"#,
        );
        drop(log);
        factory.idle().await;
        let text = std::fs::read_to_string(&path).expect("read");
        for private in ["someone@example.com", "Acme Corp", "6f1d2c3b", "a1b2"] {
            assert!(!text.contains(private), "{private} reached the log: {text}");
        }
        let written = records(&path);
        assert_eq!(
            written[0]["msg"]["params"]["authStatus"],
            json!({"status": "<REDACTED>", "account": {"email": "<REDACTED>", "organization": "<REDACTED>"}})
        );
        assert_eq!(
            written[1]["msg"]["result"]["agentInfo"]["name"],
            json!("claude-agent-acp")
        );
        assert_eq!(
            written[1]["msg"]["result"]["models"][0],
            json!({"modelId": "opus", "name": "Opus", "displayName": "Claude Opus"}),
            "model names stay readable"
        );
    }

    #[tokio::test]
    async fn long_records_keep_their_first_half_and_say_how_much_was_cut() {
        let dir = tempfile::tempdir().expect("tempdir");
        let limits = LogLimits {
            max_record_bytes: 16 * 1024,
            ..LogLimits::STANDARD
        };
        let factory = factory(dir.path()).with_limits(limits);
        let log = factory.open("chat", "spawn");
        let path = log.path().expect("on");
        let big = format!(
            r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"password":"hunter2hunter2","text":"{}"}}}}"#,
            "x".repeat(20_000)
        );
        log.line(Direction::In, &big);
        // Over the parse limit: cut before it is queued, still redacted by its keys.
        let huge = format!(
            r#"{{"accessToken":"opaque-secret","text":"{}"}}"#,
            "y".repeat(PARSE_LIMIT)
        );
        log.line(Direction::In, &huge);
        log.line(Direction::Out, r#"{"small":true}"#);
        drop(log);
        factory.idle().await;
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.lines().all(|line| line.len() <= 16 * 1024));
        let written = records(&path);
        assert_eq!(written.len(), 3);
        for record in &written[..2] {
            let raw = record["raw"].as_str().expect("raw");
            assert_eq!(raw.len(), 8 * 1024);
            assert!(record["truncated"].as_u64().is_some_and(|cut| cut > 0));
            assert!(!raw.contains("hunter2"), "{raw}");
            assert!(!raw.contains("opaque-secret"), "{raw}");
        }
        assert!(
            written[0]["raw"]
                .as_str()
                .expect("raw")
                .starts_with(r#"{"jsonrpc":"2.0","method":"session/update","params":{"password":"<REDACTED>","text":"xxx"#)
        );
        assert!(
            written[1]["raw"]
                .as_str()
                .expect("raw")
                .starts_with(r#"{"accessToken":"<REDACTED>","text":"yyy"#)
        );
        let cut = written[1]["truncated"].as_u64().expect("cut");
        assert_eq!(
            cut as usize,
            huge.len() - 8 * 1024 - "opaque-secret".len() + "<REDACTED>".len()
        );
        assert_eq!(written[2]["msg"], json!({"small": true}));
    }

    #[tokio::test]
    async fn files_rotate_at_the_size_limit_and_keep_three() {
        let dir = tempfile::tempdir().expect("tempdir");
        let limits = LogLimits {
            max_file_bytes: 4096,
            ..LogLimits::STANDARD
        };
        let factory = factory(dir.path()).with_limits(limits);
        let log = factory.open("chat", "spawn");
        for index in 0..200 {
            log.line(
                Direction::In,
                &format!(r#"{{"i":{index},"pad":"{}"}}"#, "p".repeat(80)),
            );
            if index % 20 == 0 {
                tokio::task::yield_now().await;
            }
        }
        drop(log);
        factory.idle().await;
        let chat = dir.path().join("chat");
        let mut names: Vec<String> = std::fs::read_dir(&chat)
            .expect("dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(names, ["spawn.1.jsonl", "spawn.2.jsonl", "spawn.jsonl"]);
        let mut seen = Vec::new();
        for name in ["spawn.2.jsonl", "spawn.1.jsonl", "spawn.jsonl"] {
            let path = chat.join(name);
            assert!(std::fs::metadata(&path).expect("meta").len() <= 4096);
            for record in records(&path) {
                seen.push(record["msg"]["i"].as_u64().expect("i"));
            }
        }
        // The oldest records are gone, the rest are in order and end with the last one.
        assert_eq!(seen.last(), Some(&199));
        assert!(
            seen.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "{seen:?}"
        );
        assert!(seen[0] > 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_full_queue_drops_and_counts_and_a_note_marks_the_gap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let limits = LogLimits {
            queue: 8,
            ..LogLimits::STANDARD
        };
        let factory = factory(dir.path()).with_limits(limits);
        let log = factory.open("chat", "spawn");
        let path = log.path().expect("on");
        // The writer cannot run while this task does not yield: 8 fit, 12 are dropped.
        for index in 0..20 {
            log.line(Direction::In, &format!(r#"{{"i":{index}}}"#));
        }
        tokio::task::yield_now().await;
        factory.idle_writes(&log).await;
        log.line(Direction::In, r#"{"i":20}"#);
        drop(log);
        factory.idle().await;
        let written = records(&path);
        let kinds: Vec<Value> = written
            .iter()
            .map(|record| {
                record["msg"]["i"]
                    .clone()
                    .as_u64()
                    .map_or_else(|| record["note"].clone(), Value::from)
            })
            .collect();
        let mut expected: Vec<Value> = (0..8).map(Value::from).collect();
        expected.push(json!({"dropped": 12}));
        expected.push(Value::from(20));
        assert_eq!(kinds, expected);
    }

    /// A burst dropped right before the end (the exit and shutdown notes among it) still leaves a
    /// note, though no later record comes to carry it.
    #[tokio::test(flavor = "current_thread")]
    async fn records_dropped_at_the_very_end_still_leave_a_note() {
        let dir = tempfile::tempdir().expect("tempdir");
        let limits = LogLimits {
            queue: 8,
            ..LogLimits::STANDARD
        };
        let factory = factory(dir.path()).with_limits(limits);
        let log = factory.open("chat", "spawn");
        let path = log.path().expect("on");
        // The writer cannot run while this task does not yield: 8 fit, 12 are dropped.
        for index in 0..19 {
            log.line(Direction::In, &format!(r#"{{"i":{index}}}"#));
        }
        log.note(LogNote::Exit {
            code: Some(0),
            signal: None,
        });
        let clone = log.clone();
        drop(log);
        drop(clone);
        factory.idle().await;
        let written = records(&path);
        let kinds: Vec<Value> = written
            .iter()
            .map(|record| {
                record["msg"]["i"]
                    .clone()
                    .as_u64()
                    .map_or_else(|| record["note"].clone(), Value::from)
            })
            .collect();
        let mut expected: Vec<Value> = (0..8).map(Value::from).collect();
        expected.push(json!({"dropped": 12}));
        assert_eq!(kinds, expected);
    }

    /// A line over the parse limit is cut before it is queued, so the writer never sees whole
    /// JSON; the key rules still reach every string under a sensitive or `env` key.
    #[tokio::test]
    async fn a_cut_line_keeps_nothing_under_sensitive_or_env_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let factory = factory(dir.path());
        let log = factory.open("chat", "spawn");
        let path = log.path().expect("on");
        let huge = format!(
            r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"account":{{"uuid":"acct-uuid-1","subscriptionType":"tier-max"}},"credentials":{{"user":"cred-user-1"}},"env":{{"FOO":"env-value-1","PORT":3000}},"mcp":[{{"env":[{{"name":"API_KEY","value":"pair-value-1"}}],"headers":[{{"name":"X","value":"header-value-1"}}]}}],"text":"{}"}}}}"#,
            "y".repeat(PARSE_LIMIT)
        );
        log.line(Direction::In, &huge);
        drop(log);
        factory.idle().await;
        let text = std::fs::read_to_string(&path).expect("read");
        for private in [
            "acct-uuid-1",
            "tier-max",
            "cred-user-1",
            "env-value-1",
            "pair-value-1",
            "header-value-1",
        ] {
            assert!(!text.contains(private), "{private} reached the log");
        }
        let written = records(&path);
        assert_eq!(written.len(), 1);
        assert!(written[0]["truncated"].as_u64().is_some_and(|cut| cut > 0));
        let raw = written[0]["raw"].as_str().expect("raw");
        assert!(
            raw.contains(r#""env":{"FOO":"<REDACTED>","PORT":3000}"#),
            "{}",
            head(raw, 600)
        );
    }

    #[tokio::test]
    async fn an_unwritable_folder_stops_logging_with_no_error_for_the_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"file").expect("write");
        let factory = factory(&blocker);
        let log = factory.open("chat", "spawn");
        log.line(Direction::In, "{}");
        factory.idle_writes(&log).await;
        assert_eq!(log.path(), None, "stopped after the I/O error");
        log.line(Direction::In, "{}");
        drop(log);
        factory.idle().await;
    }

    #[test]
    fn names_with_separators_never_open_a_file() {
        assert!(is_plain_name("019a0000-0000-7000-8000-000000000001"));
        assert!(is_plain_name("chat_1.x"));
        for bad in ["", "..", ".hidden", "a/b", "a\\b", "c:d", "a b"] {
            assert!(!is_plain_name(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_bad_name_gives_a_disabled_log_with_the_spawn_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = factory(dir.path()).open("../escape", "spawn-1");
        assert_eq!(log.spawn_id(), "spawn-1");
        assert_eq!(log.path(), None);
        assert!(std::fs::read_dir(dir.path()).expect("dir").next().is_none());
    }

    #[test]
    fn heads_end_on_character_boundaries() {
        assert_eq!(head("héllo", 2), "h");
        assert_eq!(head("héllo", 3), "hé");
        assert_eq!(head("abc", 10), "abc");
    }
}
