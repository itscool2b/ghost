//! The writer task: one per spawn. It drains the bounded record queue in batches, and each batch is
//! formatted, redacted, capped and appended on the blocking pool, so the protocol path never waits
//! for the disk and a paused test clock is held only while a batch is written. The current file
//! rotates at `LogLimits::max_file_bytes`: `<spawn_id>.jsonl` becomes `.1.jsonl`, `.1` becomes `.2`,
//! and the oldest beyond `LogLimits::max_files` is replaced.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use super::{
    Body, Direction, LiveLogs, LogLimits, Record, Redactor, SinkShared, head, lock, now_millis,
};

/// Records taken from the queue at once.
const BATCH: usize = 256;

/// When a line was cut before it was queued, the kept text never reaches into this many bytes
/// before the cut (or the longest secret literal, if longer): a secret cut in half there would no
/// longer match its literal.
const CUT_MARGIN: usize = 4096;

/// The state the blocking pool works on, moved there and back for each batch.
#[derive(Debug)]
pub(crate) struct Writer {
    dir: PathBuf,
    spawn_id: String,
    limits: LogLimits,
    redactor: Arc<Redactor>,
    shared: Arc<SinkShared>,
    file: Option<BufWriter<File>>,
    /// Bytes in the current file.
    size: u64,
    failed: bool,
}

impl Writer {
    pub(crate) fn new(
        dir: PathBuf,
        spawn_id: String,
        limits: LogLimits,
        redactor: Arc<Redactor>,
        shared: Arc<SinkShared>,
    ) -> Writer {
        Writer {
            dir,
            spawn_id,
            limits,
            redactor,
            shared,
            file: None,
            size: 0,
            failed: false,
        }
    }

    fn path(&self, generation: u32) -> PathBuf {
        if generation == 0 {
            self.dir.join(format!("{}.jsonl", self.spawn_id))
        } else {
            self.dir
                .join(format!("{}.{generation}.jsonl", self.spawn_id))
        }
    }

    fn write_batch(&mut self, batch: Vec<Record>) {
        let mut released = 0;
        for record in batch {
            released += record.bytes;
            if self.failed {
                continue;
            }
            let line = self.format(record);
            if let Err(error) = self.append(&line) {
                self.fail(&error);
            }
        }
        if !self.failed
            && let Some(file) = &mut self.file
            && let Err(error) = file.flush()
        {
            self.fail(&error);
        }
        self.shared.release(released);
    }

    fn append(&mut self, line: &str) -> io::Result<()> {
        let len = u64::try_from(line.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if self.file.is_some()
            && self.size > 0
            && self.size.saturating_add(len) > self.limits.max_file_bytes
        {
            self.rotate()?;
        }
        if self.file.is_none() {
            self.open()?;
        }
        if let Some(file) = &mut self.file {
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
            self.size = self.size.saturating_add(len);
        }
        Ok(())
    }

    fn open(&mut self) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path(0);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        self.size = file.metadata()?.len();
        self.file = Some(BufWriter::new(file));
        Ok(())
    }

    /// Shifts every kept file one generation up and starts an empty current file.
    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        let oldest = self.limits.max_files.saturating_sub(1);
        if oldest == 0 {
            remove_if_present(&self.path(0))?;
        } else {
            remove_if_present(&self.path(oldest))?;
            for generation in (1..oldest).rev() {
                rename_if_present(&self.path(generation), &self.path(generation + 1))?;
            }
            rename_if_present(&self.path(0), &self.path(1))?;
        }
        self.size = 0;
        self.open()
    }

    fn fail(&mut self, error: &io::Error) {
        if !self.failed {
            tracing::warn!(
                path = %self.path(0).display(),
                %error,
                "protocol logging for this agent stopped"
            );
        }
        self.failed = true;
        self.file = None;
    }

    /// One record as one JSON line (without the newline), redacted and capped.
    fn format(&self, record: Record) -> String {
        let Record { t, body, .. } = record;
        let redactor = &self.redactor;
        let (direction, key, payload, cut) = match body {
            Body::Line {
                direction: Direction::Err,
                text,
                total,
            } => {
                let cut = total.saturating_sub(text.len());
                let line = redactor.redact_text(&text).into_owned();
                ("err", "line", Payload::Text(line), cut)
            }
            Body::Line {
                direction,
                text,
                total,
            } => {
                let cut = total.saturating_sub(text.len());
                let parsed = if cut == 0 && looks_like_json(&text) {
                    serde_json::from_str::<Value>(&text).ok()
                } else {
                    None
                };
                match parsed {
                    Some(mut value) => {
                        redactor.redact_value(&mut value);
                        (direction.as_str(), "msg", Payload::Value(value), 0)
                    }
                    None if looks_like_json(&text) => {
                        let raw = redactor.redact_json_text(&text).into_owned();
                        (direction.as_str(), "raw", Payload::Text(raw), cut)
                    }
                    None => {
                        let raw = redactor.redact_text(&text).into_owned();
                        (direction.as_str(), "raw", Payload::Text(raw), cut)
                    }
                }
            }
            Body::Command(mut value) => {
                redactor.redact_value(&mut value);
                ("cmd", "cmd", Payload::Value(value), 0)
            }
            Body::Note(note) => {
                let mut value = serde_json::to_value(note).unwrap_or(Value::Null);
                redactor.redact_value(&mut value);
                ("note", "note", Payload::Value(value), 0)
            }
            Body::Dropped(count) => ("note", "note", Payload::Value(json!({"dropped": count})), 0),
        };

        let mut fields = Map::new();
        fields.insert("t".into(), Value::from(t));
        fields.insert("d".into(), Value::from(direction));
        if cut == 0 {
            let value = match &payload {
                Payload::Value(value) => value.clone(),
                Payload::Text(text) => Value::from(text.as_str()),
            };
            fields.insert(key.into(), value);
            let line = Value::Object(fields.clone()).to_string();
            if line.len() <= self.limits.max_record_bytes {
                return line;
            }
            fields.remove(key);
        }
        self.truncated(fields, payload, cut)
    }

    /// The capped form: the first half of the payload's text as `raw`, and how much was cut.
    fn truncated(&self, mut fields: Map<String, Value>, payload: Payload, cut: usize) -> String {
        let text = match payload {
            Payload::Value(value) => value.to_string(),
            Payload::Text(text) => text,
        };
        let mut keep = head(&text, self.limits.max_record_bytes / 2).len();
        if cut > 0 {
            let margin = CUT_MARGIN.max(self.redactor.longest_secret());
            keep = head(&text, keep.min(text.len().saturating_sub(margin))).len();
        }
        loop {
            let kept = text.get(..keep).unwrap_or_default();
            fields.insert("raw".into(), Value::from(kept));
            fields.insert(
                "truncated".into(),
                Value::from(cut.saturating_add(text.len() - keep)),
            );
            let line = Value::Object(fields.clone()).to_string();
            let over = line.len().saturating_sub(self.limits.max_record_bytes);
            if over == 0 || keep == 0 {
                return line;
            }
            // Escaping made it longer than the cap: keep less.
            keep = head(&text, keep.saturating_sub(over)).len();
        }
    }
}

/// A record's payload before it is placed in the record.
enum Payload {
    Value(Value),
    Text(String),
}

/// Whether the first non-space byte opens a JSON object or array.
fn looks_like_json(text: &str) -> bool {
    matches!(
        text.bytes().find(|byte| !byte.is_ascii_whitespace()),
        Some(b'{' | b'[')
    )
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn rename_if_present(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The writer task: until every [`ProtocolLog`](super::ProtocolLog) clone is gone, or an I/O error
/// stops it. Records dropped after the last one queued are counted in a last `dropped` note, so
/// the end of a log never hides a gap.
pub(crate) async fn run(mut rx: mpsc::Receiver<Record>, writer: Writer, live: LiveLogs) {
    let key = (writer.dir.clone(), writer.spawn_id.clone());
    let shared = Arc::clone(&writer.shared);
    let mut writer = Some(writer);
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while batch.len() < BATCH {
            match rx.try_recv() {
                Ok(record) => batch.push(record),
                Err(_) => break,
            }
        }
        let Some(mut current) = writer.take() else {
            break;
        };
        let written = tokio::task::spawn_blocking(move || {
            current.write_batch(batch);
            current
        })
        .await;
        match written {
            Ok(current) if !current.failed => writer = Some(current),
            _ => {
                // Logging for this spawn is over: producers stop queueing, and what is queued is
                // released unwritten.
                shared.stop();
                rx.close();
                while let Ok(record) = rx.try_recv() {
                    shared.release(record.bytes);
                }
                break;
            }
        }
    }
    // Every producer is gone, so the count is final.
    let dropped = shared.take_dropped();
    if dropped > 0
        && let Some(mut current) = writer.take()
    {
        let marker = Record {
            t: now_millis(),
            body: Body::Dropped(dropped),
            bytes: 0,
        };
        let _ = tokio::task::spawn_blocking(move || current.write_batch(vec![marker])).await;
    }
    lock(&live).remove(&key);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writer(limits: LogLimits) -> Writer {
        Writer::new(
            PathBuf::from("/unused"),
            "spawn".into(),
            limits,
            Arc::new(Redactor::standard()),
            Arc::new(SinkShared::default()),
        )
    }

    fn line(direction: Direction, text: &str) -> Record {
        Record {
            t: 1,
            body: Body::Line {
                direction,
                text: text.into(),
                total: text.len(),
            },
            bytes: text.len(),
        }
    }

    #[test]
    fn json_lines_become_msg_and_other_text_raw() {
        let writer = writer(LogLimits::STANDARD);
        assert_eq!(
            writer.format(line(Direction::In, r#" {"a":1}"#)),
            r#"{"t":1,"d":"in","msg":{"a":1}}"#
        );
        assert_eq!(
            writer.format(line(Direction::Out, "[1,2]")),
            r#"{"t":1,"d":"out","msg":[1,2]}"#
        );
        assert_eq!(
            writer.format(line(Direction::In, r#"{"a":"#)),
            r#"{"t":1,"d":"in","raw":"{\"a\":"}"#
        );
        assert_eq!(
            writer.format(line(Direction::Err, "warn: x")),
            r#"{"t":1,"d":"err","line":"warn: x"}"#
        );
        assert_eq!(
            writer.format(Record {
                t: 2,
                body: Body::Dropped(3),
                bytes: 0
            }),
            r#"{"t":2,"d":"note","note":{"dropped":3}}"#
        );
    }

    /// The capped shape replay fixtures read (spec 3.8): whatever the record's `d`, exactly `t`,
    /// that `d`, the first half of the payload's text as `raw`, and the bytes cut; never a `msg`,
    /// `cmd`, `line` or `note` that could be replayed as if it were whole.
    #[test]
    fn a_capped_record_keeps_its_direction_and_holds_raw_text_and_the_cut() {
        let limits = LogLimits {
            max_record_bytes: 1024,
            ..LogLimits::STANDARD
        };
        let writer = writer(limits);
        let big = "x".repeat(4096);
        let records = [
            (
                "out",
                line(
                    Direction::Out,
                    &format!(
                        r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"text":"{big}"}}}}"#
                    ),
                ),
            ),
            (
                "in",
                line(
                    Direction::In,
                    &format!(r#"{{"jsonrpc":"2.0","id":3,"result":{{"text":"{big}"}}}}"#),
                ),
            ),
            ("err", line(Direction::Err, &big)),
            (
                "cmd",
                Record {
                    t: 1,
                    body: Body::Command(
                        json!({"type": "prompt", "parts": [{"type": "text", "text": big}]}),
                    ),
                    bytes: 0,
                },
            ),
            (
                "note",
                Record {
                    t: 1,
                    body: Body::Note(crate::log::LogNote::Text(big.clone())),
                    bytes: 0,
                },
            ),
        ];
        for (direction, record) in records {
            let text = writer.format(record);
            assert!(text.len() <= 1024, "{direction}: {}", text.len());
            let value: Value = serde_json::from_str(&text).expect("json");
            let keys: Vec<&str> = value
                .as_object()
                .expect("an object")
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(keys, ["t", "d", "raw", "truncated"], "{direction}: {text}");
            assert_eq!(value["d"], direction);
            assert!(
                value["raw"]
                    .as_str()
                    .is_some_and(|raw| raw.starts_with(['{', 'x'])),
                "{direction}: {text}"
            );
            assert!(value["truncated"].as_u64().is_some_and(|cut| cut > 1024));
        }
    }

    #[test]
    fn escaping_never_pushes_a_capped_record_over_the_cap() {
        let limits = LogLimits {
            max_record_bytes: 600,
            ..LogLimits::STANDARD
        };
        let writer = writer(limits);
        let quotes = format!("not json {}", "\"\u{1}".repeat(400));
        let record = writer.format(line(Direction::In, &quotes));
        assert!(record.len() <= 600, "{}", record.len());
        let value: Value = serde_json::from_str(&record).expect("json");
        assert!(value["truncated"].as_u64().expect("cut") > 0);
    }

    #[test]
    fn a_cut_line_never_keeps_the_text_next_to_its_cut() {
        let limits = LogLimits {
            max_record_bytes: 20_000,
            ..LogLimits::STANDARD
        };
        let writer = writer(limits);
        // 9,000 bytes of kept text, then a secret that the cut split in half.
        let text = format!("{}sk-proj-01234", "a".repeat(9000));
        let record = writer.format(Record {
            t: 1,
            body: Body::Line {
                direction: Direction::In,
                text: text.clone(),
                total: text.len() + 100,
            },
            bytes: text.len(),
        });
        let value: Value = serde_json::from_str(&record).expect("json");
        let raw = value["raw"].as_str().expect("raw");
        assert!(!raw.contains("sk-proj"), "{raw}");
        assert_eq!(raw.len(), text.len() - CUT_MARGIN);
        assert_eq!(
            value["truncated"].as_u64(),
            Some(u64::try_from(100 + CUT_MARGIN).expect("fits"))
        );
    }
}
