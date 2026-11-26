//! The bounded line framer, ported from `docs/spikes/claude-control/src/framing.rs`.
//!
//! Lines split on `\n` only; one trailing `\r` is dropped, so Windows output frames the same way;
//! bytes decode lossily, because a tool can echo invalid UTF-8 into a message and that must never
//! end a session; empty lines are skipped. A line whose first non-space byte is neither `{` nor `[`
//! is noise (a debugger banner, a library's stray print): it is reported by size and never passed
//! on. A line over the cap is not buffered past it: the framer drops what it held and feeds the rest
//! of the line to the [`Sniffer`], so only the line's top-level `id`, `method` and `type` survive.

use crate::io::TopKeys;
use crate::sniff::Sniffer;

/// One complete unit read from stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    /// A JSON line, without its line ending.
    Line(String),
    /// A JSON line over the cap; only its size and top-level keys survive.
    Oversized { bytes: u64, keys: TopKeys },
    /// A line that is not JSON, dropped.
    Noise { bytes: usize },
}

/// Reassembles lines from read chunks that can split anywhere, even inside a UTF-8 sequence.
#[derive(Debug)]
pub(crate) struct LineFramer {
    pending: Vec<u8>,
    max_line: usize,
    /// The current line, once it went over the cap.
    skipping: Option<Skipping>,
}

#[derive(Debug)]
struct Skipping {
    bytes: u64,
    sniffer: Sniffer,
    /// Its first non-space byte opened an object or array.
    json: bool,
}

impl LineFramer {
    pub(crate) fn new(max_line: usize) -> LineFramer {
        LineFramer {
            pending: Vec::new(),
            max_line,
            skipping: None,
        }
    }

    /// Adds a chunk and returns every frame it completed.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<Frame> {
        let mut frames = Vec::new();
        let mut rest = chunk;
        while let Some(newline) = rest.iter().position(|&byte| byte == b'\n') {
            let (line, tail) = rest.split_at(newline);
            if let Some(frame) = self.complete(line) {
                frames.push(frame);
            }
            rest = tail.get(1..).unwrap_or_default();
        }
        self.extend(rest);
        frames
    }

    /// The trailing partial line at end of stream, if any: agents end every message with a
    /// newline, so a tail here means the writer was cut off mid-line.
    pub(crate) fn finish(&mut self) -> Option<Frame> {
        if self.pending.is_empty() && self.skipping.is_none() {
            return None;
        }
        self.complete(&[])
    }

    fn extend(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if let Some(skipping) = &mut self.skipping {
            skipping.bytes = skipping
                .bytes
                .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            skipping.sniffer.feed(bytes);
        } else if self.pending.len() + bytes.len() > self.max_line {
            let mut sniffer = Sniffer::default();
            sniffer.feed(&self.pending);
            sniffer.feed(bytes);
            let json = matches!(
                first_non_space(&self.pending).or_else(|| first_non_space(bytes)),
                Some(b'{' | b'[')
            );
            let held = self.pending.len() + bytes.len();
            self.skipping = Some(Skipping {
                bytes: u64::try_from(held).unwrap_or(u64::MAX),
                sniffer,
                json,
            });
            // Release the partial line now; holding it would defeat the bound.
            self.pending = Vec::new();
        } else {
            self.pending.extend_from_slice(bytes);
        }
    }

    fn complete(&mut self, tail: &[u8]) -> Option<Frame> {
        self.extend(tail);
        if let Some(skipping) = self.skipping.take() {
            return Some(if skipping.json {
                Frame::Oversized {
                    bytes: skipping.bytes,
                    keys: skipping.sniffer.finish(),
                }
            } else {
                Frame::Noise {
                    bytes: usize::try_from(skipping.bytes).unwrap_or(usize::MAX),
                }
            });
        }
        let mut line = std::mem::take(&mut self.pending);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        match first_non_space(&line) {
            None => None,
            Some(b'{' | b'[') => Some(Frame::Line(match String::from_utf8(line) {
                Ok(text) => text,
                Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
            })),
            Some(_) => Some(Frame::Noise { bytes: line.len() }),
        }
    }
}

fn first_non_space(bytes: &[u8]) -> Option<u8> {
    bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn line(text: &str) -> Frame {
        Frame::Line(text.to_owned())
    }

    #[test]
    fn framer_reassembles_lines_split_across_chunks() {
        let mut framer = LineFramer::new(1024);
        assert_eq!(framer.push(b"{\"a\":"), vec![]);
        assert_eq!(framer.push(b"1}\n{\"b\""), vec![line("{\"a\":1}")]);
        assert_eq!(framer.push(b":2}\n"), vec![line("{\"b\":2}")]);
        assert_eq!(framer.finish(), None);
    }

    #[test]
    fn framer_keeps_order_and_skips_empty_lines() {
        let mut framer = LineFramer::new(1024);
        assert_eq!(
            framer.push(b"{\"n\":1}\n[2]\n\n  \t\n\r\n{\"n\":3}"),
            vec![line("{\"n\":1}"), line("[2]")]
        );
        assert_eq!(framer.finish(), Some(line("{\"n\":3}")));
        assert_eq!(framer.finish(), None);
    }

    #[test]
    fn framer_frames_crlf_like_lf() {
        let mut framer = LineFramer::new(1024);
        assert_eq!(
            framer.push(b"{\"w\":1}\r\n{\"u\":2}\n"),
            vec![line("{\"w\":1}"), line("{\"u\":2}")]
        );
        // Only one trailing `\r` goes.
        assert_eq!(framer.push(b"{}\r\r\n"), vec![line("{}\r")]);
    }

    #[test]
    fn framer_replaces_invalid_utf8() {
        let mut framer = LineFramer::new(1024);
        assert_eq!(
            framer.push(b"{\"t\":\"ok \xff\xfe end\"}\n"),
            vec![line("{\"t\":\"ok \u{fffd}\u{fffd} end\"}")]
        );
    }

    #[test]
    fn framer_reassembles_a_multibyte_character_split_between_chunks() {
        let mut framer = LineFramer::new(1024);
        let rocket = "🚀".as_bytes();
        assert_eq!(framer.push(b"{\"r\":\""), vec![]);
        assert_eq!(framer.push(&rocket[..2]), vec![]);
        assert_eq!(
            framer.push(&[&rocket[2..], b"\"}\n"].concat()),
            vec![line("{\"r\":\"🚀\"}")]
        );
    }

    #[test]
    fn framer_oversized_line() {
        let mut framer = LineFramer::new(16);
        assert_eq!(framer.push(b"{\"params\":\"0123"), vec![]);
        assert_eq!(framer.push(b"456789abcdef\",\"id\""), vec![]);
        assert_eq!(
            framer.push(b":4,\"method\":\"m\"}\n{\"next\":1}\n"),
            vec![
                Frame::Oversized {
                    bytes: 49,
                    keys: TopKeys {
                        id: Some(json!(4)),
                        method: Some("m".into()),
                        kind: None,
                    }
                },
                line("{\"next\":1}"),
            ]
        );
        // A line of exactly the cap passes.
        assert_eq!(
            framer.push(b"{\"0123456789ab\"}\n"),
            vec![line("{\"0123456789ab\"}")]
        );
    }

    #[test]
    fn framer_reports_an_oversized_tail_at_eof() {
        let mut framer = LineFramer::new(4);
        assert_eq!(framer.push(b"{\"id\":1"), vec![]);
        assert_eq!(
            framer.finish(),
            Some(Frame::Oversized {
                bytes: 7,
                keys: TopKeys {
                    id: Some(json!(1)),
                    ..TopKeys::default()
                }
            })
        );
        assert_eq!(framer.finish(), None);
    }

    #[test]
    fn noise_lines_are_reported_by_size_only() {
        let mut framer = LineFramer::new(32);
        assert_eq!(
            framer.push(b"Debugger listening on ws://127.0.0.1:9229/x\n{\"ok\":1}\n  plain\n"),
            vec![
                Frame::Noise { bytes: 43 },
                line("{\"ok\":1}"),
                Frame::Noise { bytes: 7 }
            ]
        );
        assert_eq!(
            framer.push(b"  {\"lead\":\"space\"}\n"),
            vec![line("  {\"lead\":\"space\"}")]
        );
    }
}
