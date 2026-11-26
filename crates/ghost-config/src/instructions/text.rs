//! Byte-faithful text handling. The editor works on plain LF text; saving writes the file's own
//! byte order mark and line endings back, so an edit never rewrites what the person did not touch.

use serde::{Deserialize, Serialize};
use specta::Type;

const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", rename = "InstructionsLineEnding")]
pub enum LineEnding {
    /// No line break yet, or only LF.
    Lf,
    Crlf,
    /// Both kinds. A save uses the more common one and the review says so.
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename = "InstructionsEncoding")]
pub struct Encoding {
    pub bom: bool,
    pub line_ending: LineEnding,
    /// Mixed files are written with CRLF when CRLF lines are the majority.
    pub crlf_majority: bool,
}

impl Default for Encoding {
    fn default() -> Self {
        Self {
            bom: false,
            line_ending: LineEnding::Lf,
            crlf_majority: false,
        }
    }
}

/// UTF-8 text without NUL bytes. A leading BOM is removed, CRLF becomes LF.
pub(crate) fn decode(bytes: &[u8]) -> Option<(String, Encoding)> {
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    if body.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(body).ok()?;
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    let line_ending = match (crlf, lf) {
        (0, _) => LineEnding::Lf,
        (_, 0) => LineEnding::Crlf,
        _ => LineEnding::Mixed,
    };
    Some((
        text.replace("\r\n", "\n"),
        Encoding {
            bom,
            line_ending,
            crlf_majority: crlf > lf,
        },
    ))
}

/// The bytes a save writes for LF `text` in a file with this encoding.
pub(crate) fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    let text = text.replace("\r\n", "\n");
    let crlf = match encoding.line_ending {
        LineEnding::Lf => false,
        LineEnding::Crlf => true,
        LineEnding::Mixed => encoding.crlf_majority,
    };
    let body = if crlf {
        text.replace('\n', "\r\n")
    } else {
        text
    };
    let mut bytes = Vec::with_capacity(body.len() + 3);
    if encoding.bom {
        bytes.extend_from_slice(BOM);
    }
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

/// What a review says about how the bytes are kept.
pub(crate) fn encoding_note(encoding: Encoding) -> Option<String> {
    match (encoding.bom, encoding.line_ending) {
        (false, LineEnding::Lf) => None,
        (true, LineEnding::Lf) => Some("Keeps the file's byte order mark.".into()),
        (false, LineEnding::Crlf) => Some("Keeps Windows (CRLF) line endings.".into()),
        (true, LineEnding::Crlf) => {
            Some("Keeps the byte order mark and Windows (CRLF) line endings.".into())
        }
        (bom, LineEnding::Mixed) => Some(format!(
            "The file mixes line endings. Saving uses {} everywhere{}.",
            if encoding.crlf_majority { "CRLF" } else { "LF" },
            if bom {
                " and keeps the byte order mark"
            } else {
                ""
            }
        )),
    }
}

/// The line count agents' guidance refers to.
pub(crate) fn lines(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_and_crlf_survive_an_edit_and_lf_files_stay_lf() {
        let original = b"\xEF\xBB\xBF# Rules\r\nUse tabs.\r\n";
        let (text, encoding) = decode(original).unwrap();
        assert_eq!(text, "# Rules\nUse tabs.\n");
        assert!(encoding.bom);
        assert_eq!(encoding.line_ending, LineEnding::Crlf);
        assert_eq!(encode(&text, encoding), original);
        assert_eq!(
            encode("# Rules\nUse spaces.\n", encoding),
            b"\xEF\xBB\xBF# Rules\r\nUse spaces.\r\n"
        );
        let (plain, lf) = decode(b"a\nb\n").unwrap();
        assert_eq!(encode(&plain, lf), b"a\nb\n");
        assert!(encoding_note(lf).is_none());
        assert!(encoding_note(encoding).unwrap().contains("CRLF"));
    }

    #[test]
    fn mixed_endings_use_the_majority_and_say_so() {
        let (text, encoding) = decode(b"a\r\nb\r\nc\n").unwrap();
        assert_eq!(encoding.line_ending, LineEnding::Mixed);
        assert!(encoding.crlf_majority);
        assert_eq!(encode(&text, encoding), b"a\r\nb\r\nc\r\n");
        assert!(encoding_note(encoding).unwrap().contains("mixes"));
    }

    #[test]
    fn non_utf8_and_binary_files_are_not_text_and_empty_files_are() {
        assert!(decode(b"\xFF\xFEh\x00i\x00").is_none());
        assert!(decode(b"caf\xE9").is_none());
        assert!(decode(b"a\0b").is_none());
        let (text, encoding) = decode(b"").unwrap();
        assert_eq!(text, "");
        assert_eq!(encoding, Encoding::default());
        assert_eq!(lines(""), 0);
        assert_eq!(lines("a\nb"), 2);
    }
}
