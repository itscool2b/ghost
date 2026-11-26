//! Git's `--progress` lines ("Receiving objects:  45% (450/1000), 1.20 MiB | 1.10 MiB/s").
//! Progress rewrites one line with carriage returns; every other line is kept, bounded, for a
//! failed clone's explanation. Nothing here is logged.
use ghost_protocol::setup_actions::CloneProgress;

const KEPT_LINES: usize = 8;
const LINE_CHARS: usize = 300;

#[derive(Default)]
pub(super) struct Parser {
    partial: Vec<u8>,
    pub(super) progress: Option<CloneProgress>,
    pub(super) output: Vec<String>,
}

impl Parser {
    /// Feeds bytes; returns whether the progress or the kept lines changed.
    pub(super) fn feed(&mut self, bytes: &[u8]) -> bool {
        let mut changed = false;
        for &byte in bytes {
            if byte == b'\r' || byte == b'\n' {
                let line = std::mem::take(&mut self.partial);
                changed |= self.line(&line);
            } else if self.partial.len() < 4096 {
                self.partial.push(byte);
            }
        }
        changed
    }
    pub(super) fn finish(&mut self) -> bool {
        let line = std::mem::take(&mut self.partial);
        self.line(&line)
    }
    fn line(&mut self, bytes: &[u8]) -> bool {
        let text = strip_escapes(&String::from_utf8_lossy(bytes));
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        if let Some((stage, stage_percent, received)) = parse(text) {
            let percent = overall(&stage, stage_percent).unwrap_or(0).max(
                self.progress
                    .as_ref()
                    .map_or(0, |progress| progress.percent),
            );
            let received = received.or_else(|| {
                self.progress
                    .as_ref()
                    .and_then(|progress| progress.received.clone())
            });
            let next = CloneProgress {
                stage,
                stage_percent,
                percent,
                received,
            };
            if self.progress.as_ref() == Some(&next) {
                return false;
            }
            self.progress = Some(next);
            return true;
        }
        let kept: String = text.chars().take(LINE_CHARS).collect();
        if self.output.last() == Some(&kept) {
            return false;
        }
        self.output.push(kept);
        if self.output.len() > KEPT_LINES {
            self.output.remove(0);
        }
        true
    }
}

/// `Stage: NN% (a/b)[, size | rate][, done.]`, optionally after Git's `remote: `.
fn parse(text: &str) -> Option<(String, u8, Option<String>)> {
    let text = text.strip_prefix("remote: ").unwrap_or(text);
    let (stage, rest) = text.split_once(':')?;
    let stage = stage.trim();
    if stage.is_empty() || stage.len() > 40 || !stage.chars().all(|c| c.is_alphabetic() || c == ' ')
    {
        return None;
    }
    let rest = rest.trim_start();
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 3 || rest.as_bytes().get(digits) != Some(&b'%') {
        return None;
    }
    let percent: u8 = rest[..digits].parse().ok().filter(|value| *value <= 100)?;
    // ", 1.20 MiB | 1.10 MiB/s" follows the counts while objects arrive.
    let received = rest
        .split_once("), ")
        .map(|(_, after)| after.split(['|', ',']).next().unwrap_or("").trim())
        .filter(|size| !size.is_empty() && size != &"done." && size.len() <= 24)
        .map(str::to_owned);
    Some((stage.to_owned(), percent, received))
}

/// The whole clone from Git's stage: the server's counting, then receiving (most of the time),
/// resolving deltas, and writing the files. Stages Git names differently keep the last figure.
fn overall(stage: &str, percent: u8) -> Option<u8> {
    let (start, span): (u8, u8) = match stage {
        "Enumerating objects" | "Counting objects" | "Compressing objects" => (0, 5),
        "Receiving objects" => (5, 70),
        "Resolving deltas" => (75, 15),
        "Updating files" | "Checking out files" | "Filtering content" => (90, 10),
        _ => return None,
    };
    Some(start + (u16::from(span) * u16::from(percent) / 100) as u8)
}

/// Drops terminal escape sequences a server or helper may print.
fn strip_escapes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if !c.is_control() {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_follows_gits_stages_and_other_lines_are_kept_bounded() {
        let mut parser = Parser::default();
        assert!(parser.feed(b"Cloning into 'C:\\code\\repo'...\n"));
        assert!(parser.feed(
            b"remote: Enumerating objects: 120, done.\nremote: Counting objects:  50% (60/120)\r"
        ));
        assert_eq!(parser.progress.as_ref().unwrap().percent, 2);
        parser.feed(b"Receiving objects:  45% (54/120), 1.20 MiB | 1.10 MiB/s\r");
        let progress = parser.progress.clone().unwrap();
        assert_eq!(progress.stage, "Receiving objects");
        assert_eq!(progress.stage_percent, 45);
        assert_eq!(progress.percent, 36);
        assert_eq!(progress.received.as_deref(), Some("1.20 MiB"));
        parser.feed(b"Receiving objects: 100% (120/120), 2.40 MiB | 1.20 MiB/s, done.\n");
        parser.feed(b"Resolving deltas:  50% (10/20)\r");
        let progress = parser.progress.clone().unwrap();
        assert_eq!((progress.percent, progress.stage_percent), (82, 50));
        assert_eq!(
            progress.received.as_deref(),
            Some("2.40 MiB"),
            "kept from receiving"
        );
        parser.feed(b"Updating files: 100% (40/40), done.\n");
        assert_eq!(parser.progress.as_ref().unwrap().percent, 100);
        // A stage Git names in another language keeps the last overall figure.
        parser.feed(b"Empfange Objekte:  10% (1/10)\r");
        assert_eq!(parser.progress.as_ref().unwrap().percent, 100);
        assert_eq!(parser.progress.as_ref().unwrap().stage, "Empfange Objekte");
        parser.feed(b"fatal: \x1b[31mrepository not found\x1b[0m");
        assert!(parser.finish());
        assert_eq!(
            parser.output,
            [
                "Cloning into 'C:\\code\\repo'...",
                "remote: Enumerating objects: 120, done.",
                "fatal: repository not found"
            ]
        );
        for index in 0..20 {
            parser.feed(format!("line {index}\n").as_bytes());
        }
        assert_eq!(parser.output.len(), KEPT_LINES);
        assert_eq!(parser.output.last().map(String::as_str), Some("line 19"));
    }
}
