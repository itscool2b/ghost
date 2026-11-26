//! The streaming top-level key sniffer. Fed the bytes of an oversized line in any number of pieces,
//! it tracks depth, strings and escapes in constant memory and captures the raw top-level `"id"`
//! value and the `"method"` and `"type"` strings, each at most 256 bytes, wherever they appear in
//! the top-level object: an `"id"` written after a 70 MB `params` is still found. Keys of nested
//! objects are ignored, and a top level that is not an object yields nothing.

use crate::io::TopKeys;

/// The longest captured value, in bytes (quotes not counted).
const MAX_CAPTURE: usize = 256;

/// The keys whose values are captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Id,
    Method,
    Type,
}

/// Where the sniffer is inside the top-level object, outside nested values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum At {
    /// Before a key (after `{` or `,`).
    #[default]
    Key,
    /// Inside a key string.
    InKey,
    /// After a key, before `:`.
    Colon,
    /// After `:`, before the value.
    Value,
    /// Inside a string value.
    InString,
    /// Inside a number or `true`/`false`/`null`.
    InScalar,
    /// Inside a nested object or array.
    InNested,
    /// After a value, before `,` or `}`.
    Comma,
}

#[derive(Debug, Default)]
pub(crate) struct Sniffer {
    /// The first non-space byte was seen.
    started: bool,
    /// The top-level value ended, or is not an object.
    done: bool,
    at: At,
    /// Nesting below the top-level object (1 directly inside a nested value).
    nested: u32,
    in_string: bool,
    escaped: bool,
    /// The current key's bytes, while short enough to be one of ours.
    key: Vec<u8>,
    /// The field the current value belongs to, if it is captured.
    field: Option<Field>,
    /// The current value's raw bytes, quotes included, while it is captured.
    value: Vec<u8>,
    capturing: bool,
    id: Option<Vec<u8>>,
    method: Option<Vec<u8>>,
    kind: Option<Vec<u8>>,
}

impl Sniffer {
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.done {
                return;
            }
            self.step(byte);
        }
    }

    /// The keys found. A scalar the line ended in the middle of still counts.
    pub(crate) fn finish(mut self) -> TopKeys {
        if self.at == At::InScalar {
            self.end_value();
        }
        let text =
            |raw: Option<Vec<u8>>| raw.and_then(|raw| serde_json::from_slice::<String>(&raw).ok());
        TopKeys {
            id: self
                .id
                .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
                .filter(|id| !id.is_object() && !id.is_array()),
            method: text(self.method),
            kind: text(self.kind),
        }
    }

    fn step(&mut self, byte: u8) {
        if !self.started {
            if !byte.is_ascii_whitespace() {
                self.started = true;
                // Only an object has top-level keys.
                self.done = byte != b'{';
            }
            return;
        }
        if self.in_string {
            self.step_in_string(byte);
            return;
        }
        match self.at {
            At::Key => match byte {
                b'"' => {
                    self.in_string = true;
                    self.at = At::InKey;
                    self.key.clear();
                }
                b'}' => self.done = true,
                _ => {}
            },
            At::Colon => {
                if byte == b':' {
                    self.at = At::Value;
                }
            }
            At::Value => {
                if byte.is_ascii_whitespace() {
                    return;
                }
                self.value.clear();
                match byte {
                    b'"' => {
                        self.in_string = true;
                        self.at = At::InString;
                        self.capturing = self.field.is_some();
                        self.push_value(byte);
                    }
                    b'{' | b'[' => {
                        self.at = At::InNested;
                        self.nested = 1;
                        self.capturing = false;
                    }
                    _ => {
                        self.at = At::InScalar;
                        self.capturing = self.field == Some(Field::Id);
                        self.push_value(byte);
                    }
                }
            }
            At::InScalar => match byte {
                b',' => {
                    self.end_value();
                    self.at = At::Key;
                }
                b'}' => {
                    self.end_value();
                    self.done = true;
                }
                _ if byte.is_ascii_whitespace() => {
                    self.end_value();
                    self.at = At::Comma;
                }
                _ => self.push_value(byte),
            },
            At::InNested => match byte {
                b'"' => self.in_string = true,
                b'{' | b'[' => self.nested += 1,
                b'}' | b']' => {
                    self.nested = self.nested.saturating_sub(1);
                    if self.nested == 0 {
                        self.at = At::Comma;
                    }
                }
                _ => {}
            },
            At::Comma => match byte {
                b',' => self.at = At::Key,
                b'}' => self.done = true,
                _ => {}
            },
            At::InKey | At::InString => {}
        }
    }

    fn step_in_string(&mut self, byte: u8) {
        let closing = !self.escaped && byte == b'"';
        if self.escaped {
            self.escaped = false;
        } else if byte == b'\\' {
            self.escaped = true;
        }
        match self.at {
            At::InKey => {
                if closing {
                    self.in_string = false;
                    self.field = match self.key.as_slice() {
                        b"id" => Some(Field::Id),
                        b"method" => Some(Field::Method),
                        b"type" => Some(Field::Type),
                        _ => None,
                    };
                    self.at = At::Colon;
                } else if self.key.len() <= MAX_CAPTURE {
                    self.key.push(byte);
                }
            }
            At::InString => {
                self.push_value(byte);
                if closing {
                    self.in_string = false;
                    self.end_value();
                    self.at = At::Comma;
                }
            }
            // A string inside a nested value: only its end matters.
            _ => {
                if closing {
                    self.in_string = false;
                }
            }
        }
    }

    fn push_value(&mut self, byte: u8) {
        if !self.capturing {
            return;
        }
        if self.value.len() >= MAX_CAPTURE + 2 {
            // Too long to be one of ours: give up on this value.
            self.capturing = false;
            self.value.clear();
            return;
        }
        self.value.push(byte);
    }

    /// Stores the captured value under its field, unless that field already has one.
    fn end_value(&mut self) {
        if !self.capturing {
            return;
        }
        self.capturing = false;
        let value = std::mem::take(&mut self.value);
        let slot = match self.field {
            Some(Field::Id) => &mut self.id,
            Some(Field::Method) => &mut self.method,
            Some(Field::Type) => &mut self.kind,
            None => return,
        };
        if slot.is_none() {
            *slot = Some(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sniff(text: &str) -> TopKeys {
        let mut sniffer = Sniffer::default();
        sniffer.feed(text.as_bytes());
        sniffer.finish()
    }

    /// Byte by byte, as the worst split of reads would deliver it.
    fn sniff_bytewise(text: &str) -> TopKeys {
        let mut sniffer = Sniffer::default();
        for byte in text.as_bytes() {
            sniffer.feed(std::slice::from_ref(byte));
        }
        sniffer.finish()
    }

    #[test]
    fn top_level_keys_are_found_and_nested_ones_ignored() {
        let text =
            r#"{"params":{"id":5,"method":"inner","type":"x"},"method":"outer","id":7,"type":"t"}"#;
        for keys in [sniff(text), sniff_bytewise(text)] {
            assert_eq!(keys.id, Some(json!(7)));
            assert_eq!(keys.method.as_deref(), Some("outer"));
            assert_eq!(keys.kind.as_deref(), Some("t"));
        }
    }

    #[test]
    fn sniffer_finds_late_id() {
        let text = format!(
            r#"{{"jsonrpc":"2.0","method":"session/request_permission","params":{{"blob":"{}","nested":[{{"id":1}}]}},"id":"req-9"}}"#,
            "a\\\"}]{[".repeat(100_000)
        );
        let mut sniffer = Sniffer::default();
        for chunk in text.as_bytes().chunks(65_536) {
            sniffer.feed(chunk);
        }
        let keys = sniffer.finish();
        assert_eq!(keys.id, Some(json!("req-9")));
        assert_eq!(keys.method.as_deref(), Some("session/request_permission"));
    }

    #[test]
    fn ids_keep_their_json_type() {
        assert_eq!(sniff(r#"{"id":-12.5}"#).id, Some(json!(-12.5)));
        assert_eq!(sniff(r#"{ "id" : null , "x":1}"#).id, Some(json!(null)));
        assert_eq!(sniff(r#"{"id":"a\"b"}"#).id, Some(json!("a\"b")));
        assert_eq!(sniff(r#"{"id":{"x":1}}"#).id, None, "an object is no id");
        assert_eq!(sniff(r#"{"id":12"#).id, Some(json!(12)), "cut after the id");
    }

    #[test]
    fn strings_with_braces_quotes_and_escapes_never_confuse_depth() {
        let keys = sniff(r#"{"params":{"text":"he said \"}\" {[ \\"},"id":3,"result":"\\\"}"}"#);
        assert_eq!(keys.id, Some(json!(3)));
        assert_eq!(
            sniff(r#"{"a":"\\","method":"m"}"#).method.as_deref(),
            Some("m")
        );
    }

    #[test]
    fn the_first_of_duplicate_keys_wins() {
        assert_eq!(sniff(r#"{"id":1,"id":2}"#).id, Some(json!(1)));
    }

    #[test]
    fn values_over_the_limit_or_of_the_wrong_kind_are_not_captured() {
        let long = format!(r#"{{"method":"{}","id":1}}"#, "m".repeat(300));
        let keys = sniff(&long);
        assert_eq!(keys.method, None);
        assert_eq!(keys.id, Some(json!(1)));
        assert_eq!(sniff(r#"{"method":5,"type":["a"]}"#), TopKeys::default());
        let at_limit = format!(r#"{{"method":"{}"}}"#, "m".repeat(256));
        assert_eq!(
            sniff(&at_limit).method.map(|method| method.len()),
            Some(256)
        );
    }

    #[test]
    fn a_top_level_that_is_not_an_object_has_no_keys() {
        assert_eq!(sniff(r#"[{"id":1,"method":"m"}]"#), TopKeys::default());
        assert_eq!(sniff("  \"id\""), TopKeys::default());
        assert_eq!(sniff(""), TopKeys::default());
        assert_eq!(
            sniff(r#"{"a":1} {"id":2}"#).id,
            None,
            "after the object ended"
        );
    }
}
