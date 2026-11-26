//! The stderr drain: read continuously into a 16 KiB ring (the tail error cards show) and split into
//! log lines of at most 4 KiB, at most 256 KiB per second logged and the rest counted, so a flood
//! never blocks stdout and never fills the disk.

use std::collections::VecDeque;
use std::io;
use std::sync::{Mutex, MutexGuard};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::{Duration, Instant};

use crate::log::{Direction, LogNote, ProtocolLog};
use crate::{STDERR_LINE_BYTES, STDERR_LOG_BYTES_PER_SEC, STDERR_TAIL_BYTES};

/// The newest stderr bytes, raw.
#[derive(Debug, Default)]
pub(crate) struct StderrTail {
    ring: Mutex<VecDeque<u8>>,
}

impl StderrTail {
    pub(crate) fn push(&self, bytes: &[u8]) {
        let mut ring = lock(&self.ring);
        ring.extend(bytes);
        let excess = ring.len().saturating_sub(STDERR_TAIL_BYTES);
        ring.drain(..excess);
    }

    /// The last `max_bytes` of text: lossy, ANSI escapes and control characters other than
    /// newlines and tabs removed, starting on a character boundary. Not redacted.
    pub(crate) fn tail(&self, max_bytes: usize) -> String {
        let raw: Vec<u8> = lock(&self.ring).iter().copied().collect();
        // The ring may start inside a multibyte character.
        let start = raw
            .iter()
            .position(|byte| byte & 0b1100_0000 != 0b1000_0000)
            .unwrap_or(raw.len());
        let text = strip_ansi(&String::from_utf8_lossy(
            raw.get(start..).unwrap_or_default(),
        ));
        tail_of(&text, max_bytes).to_owned()
    }
}

/// The last `max_bytes` of `text`, starting on a character boundary.
pub(crate) fn tail_of(text: &str, max_bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(max_bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.get(start..).unwrap_or_default()
}

/// Removes terminal escape sequences (CSI, OSC, and two- or three-byte escapes) and every control
/// character except newline and tab.
pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => skip_csi(&mut chars),
                Some(']') => {
                    // OSC: until BEL or ESC \.
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            chars.next_if_eq(&'\\');
                            break;
                        }
                    }
                }
                Some('(' | ')' | '*' | '+' | '-' | '.' | '/') => {
                    chars.next();
                }
                _ => {}
            },
            '\u{9b}' => skip_csi(&mut chars),
            '\n' | '\t' => out.push(c),
            _ if c.is_control() => {}
            _ => out.push(c),
        }
    }
    out
}

/// Skips a CSI sequence's parameters up to and including its final byte (`@` to `~`).
fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for c in chars.by_ref() {
        if ('@'..='~').contains(&c) {
            break;
        }
    }
}

/// Splits stderr bytes into log lines: on `\n`, a trailing `\r` dropped, at most
/// [`STDERR_LINE_BYTES`] each (a longer line is cut on a character boundary), empty lines skipped.
#[derive(Debug, Default)]
pub(crate) struct LineSplitter {
    buf: Vec<u8>,
}

impl LineSplitter {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in chunk {
            if byte == b'\n' {
                self.emit(&mut lines);
                continue;
            }
            self.buf.push(byte);
            if self.buf.len() >= STDERR_LINE_BYTES {
                let rest = self.buf.split_off(complete_prefix(&self.buf));
                self.emit(&mut lines);
                self.buf = rest;
            }
        }
        lines
    }

    /// The last partial line, at end of stream.
    pub(crate) fn finish(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        self.emit(&mut lines);
        lines
    }

    fn emit(&mut self, lines: &mut Vec<String>) {
        let mut line = std::mem::take(&mut self.buf);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if !line.is_empty() {
            lines.push(String::from_utf8_lossy(&line).into_owned());
        }
    }
}

/// The length of `buf` without a character its last bytes only begin, so a cut there never splits
/// one.
fn complete_prefix(buf: &[u8]) -> usize {
    let len = buf.len();
    for back in 1..=len.min(4) {
        let at = len - back;
        let byte = buf.get(at).copied().unwrap_or_default();
        if byte & 0b1100_0000 == 0b1000_0000 {
            continue;
        }
        let width = match byte {
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => 1,
        };
        return if at + width > len && at > 0 { at } else { len };
    }
    len
}

/// At most [`STDERR_LOG_BYTES_PER_SEC`] logged per one-second window; the rest is counted and
/// reported once the next window opens.
#[derive(Debug)]
pub(crate) struct RateLimit {
    window: Instant,
    logged: usize,
    dropped: u64,
}

impl RateLimit {
    pub(crate) fn new(now: Instant) -> RateLimit {
        RateLimit {
            window: now,
            logged: 0,
            dropped: 0,
        }
    }

    /// Whether a line of `bytes` may be logged now. When a new window opens after drops, the count
    /// of dropped bytes comes back first, for the log.
    pub(crate) fn admit(&mut self, now: Instant, bytes: usize) -> (bool, Option<u64>) {
        let mut report = None;
        if now.duration_since(self.window) >= Duration::from_secs(1) {
            self.window = now;
            self.logged = 0;
            if self.dropped > 0 {
                report = Some(std::mem::take(&mut self.dropped));
            }
        }
        if self.logged + bytes <= STDERR_LOG_BYTES_PER_SEC {
            self.logged += bytes;
            (true, report)
        } else {
            self.dropped = self
                .dropped
                .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
            (false, report)
        }
    }

    /// Bytes dropped in the current window and not reported yet.
    pub(crate) fn pending(&mut self) -> Option<u64> {
        (self.dropped > 0).then(|| std::mem::take(&mut self.dropped))
    }
}

/// Reads stderr to its end: every byte into the tail, lines into the log within the rate limit.
pub(crate) async fn drain(
    mut pipe: impl AsyncRead + Unpin,
    tail: std::sync::Arc<StderrTail>,
    log: ProtocolLog,
) {
    let mut chunk = vec![0u8; 16 * 1024];
    let mut splitter = LineSplitter::default();
    let mut limit = RateLimit::new(Instant::now());
    loop {
        let read = match pipe.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            // A read error ends the stream like end of file.
            Err(_) => break,
        };
        let bytes = chunk.get(..read).unwrap_or_default();
        tail.push(bytes);
        for line in splitter.push(bytes) {
            log_line(&log, &mut limit, &line);
        }
    }
    for line in splitter.finish() {
        log_line(&log, &mut limit, &line);
    }
    if let Some(dropped) = limit.pending() {
        log.note(not_logged(dropped));
    }
}

fn log_line(log: &ProtocolLog, limit: &mut RateLimit, line: &str) {
    let (admitted, report) = limit.admit(Instant::now(), line.len());
    if let Some(dropped) = report {
        log.note(not_logged(dropped));
    }
    if admitted {
        log.line(Direction::Err, line);
    }
}

fn not_logged(bytes: u64) -> LogNote {
    LogNote::Text(format!(
        "stderr: {bytes} bytes not logged (over {} KiB per second)",
        STDERR_LOG_BYTES_PER_SEC / 1024
    ))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_keeps_the_newest_bytes_without_escapes() {
        let tail = StderrTail::default();
        tail.push(b"\x1b[31merror:\x1b[0m boom\r\n");
        tail.push("bell\u{7} and \x1b]0;title\x07osc and \x1b(Bcharset\n".as_bytes());
        assert_eq!(tail.tail(1024), "error: boom\nbell and osc and charset\n");
        assert_eq!(tail.tail(8), "charset\n");
        tail.push(&vec![b'x'; STDERR_TAIL_BYTES]);
        assert_eq!(tail.tail(usize::MAX).len(), STDERR_TAIL_BYTES);
    }

    #[test]
    fn a_ring_that_starts_inside_a_character_decodes_cleanly() {
        let tail = StderrTail::default();
        // The ring keeps the last 16 KiB, which begins with the second byte of an "é".
        let mut bytes = "é".as_bytes().to_vec();
        bytes.extend(vec![b'a'; STDERR_TAIL_BYTES - 1]);
        tail.push(&bytes);
        let text = tail.tail(usize::MAX);
        assert_eq!(text.len(), STDERR_TAIL_BYTES - 1);
        assert!(text.bytes().all(|byte| byte == b'a'));
        assert_eq!(tail.tail(3), "aaa");
    }

    #[test]
    fn lines_split_on_newlines_and_at_four_kib() {
        let mut splitter = LineSplitter::default();
        assert_eq!(splitter.push(b"one\r\ntwo\n\nthr"), ["one", "two"]);
        assert_eq!(splitter.push(b"ee\n"), ["three"]);
        // Odd alignment: a two-byte character straddles every 4 KiB boundary.
        let long = format!("a{}", "é".repeat(STDERR_LINE_BYTES));
        let lines = splitter.push(long.as_bytes());
        assert!(lines.iter().all(|line| line.len() <= STDERR_LINE_BYTES));
        assert!(
            lines.iter().all(|line| !line.contains('\u{fffd}')),
            "cut between characters"
        );
        let rest = splitter.finish();
        let total: usize = lines.iter().chain(&rest).map(String::len).sum();
        assert_eq!(total, long.len());
        assert!(splitter.finish().is_empty());
    }

    #[test]
    fn the_rate_limit_counts_what_it_drops_and_reports_it_next_window() {
        let start = Instant::now();
        let mut limit = RateLimit::new(start);
        assert_eq!(
            limit.admit(start, STDERR_LOG_BYTES_PER_SEC - 10),
            (true, None)
        );
        assert_eq!(limit.admit(start, 20), (false, None));
        assert_eq!(limit.admit(start, 5), (true, None));
        assert_eq!(
            limit.admit(start + Duration::from_millis(1001), 100),
            (true, Some(20))
        );
        assert_eq!(limit.pending(), None);
        assert_eq!(
            limit.admit(
                start + Duration::from_millis(1002),
                STDERR_LOG_BYTES_PER_SEC
            ),
            (false, None)
        );
        assert_eq!(
            limit.pending(),
            Some(u64::try_from(STDERR_LOG_BYTES_PER_SEC).expect("fits"))
        );
    }

    /// Paused time keeps the whole flood inside one rate-limit window.
    #[tokio::test(start_paused = true)]
    async fn a_flood_is_tailed_and_logged_within_the_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let factory = crate::log::LogFactory::new(
            dir.path().to_path_buf(),
            std::sync::Arc::new(crate::log::Redactor::standard()),
            tokio::runtime::Handle::current(),
        );
        let log = factory.open("chat", "spawn");
        let path = log.path().expect("on");
        let flood = format!("{}\n", "e".repeat(99)).repeat(20_000);
        let tail = std::sync::Arc::new(StderrTail::default());
        drain(flood.as_bytes(), std::sync::Arc::clone(&tail), log).await;
        factory.idle().await;
        assert_eq!(tail.tail(usize::MAX).len(), STDERR_TAIL_BYTES);
        let text = std::fs::read_to_string(path).expect("log");
        let logged = text
            .lines()
            .filter(|line| line.contains("\"d\":\"err\""))
            .count();
        assert_eq!(logged, STDERR_LOG_BYTES_PER_SEC / 99);
        assert!(text.contains("bytes not logged"), "the rest is counted");
    }
}
