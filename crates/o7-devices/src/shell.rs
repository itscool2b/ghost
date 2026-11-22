//! Words for a device's shell. adb's `shell:` and `exec:` services take one command line that the
//! device's `sh` parses, so anything that did not come from this crate is quoted or refused here
//! before it is put into one.

use crate::Error;

/// `text` as one shell word: single quotes, with each inner quote closed, escaped and reopened.
/// A NUL cannot be passed in a command line and is refused.
pub fn quote(text: &str) -> Result<String, Error> {
    if text.contains('\0') {
        return Err(Error::Refused("Text for the device cannot hold a NUL character.".into()));
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for ch in text.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    Ok(out)
}

/// An Android application id (`com.example.app`): letters, digits and underscores in at least two
/// dot-separated parts, each starting with a letter. Checked so one can be put into a command
/// unquoted and so a tool call cannot pass an option where a package belongs.
pub fn package(name: &str) -> Result<&str, Error> {
    let parts: Vec<&str> = name.split('.').collect();
    let valid = name.len() <= 255
        && parts.len() >= 2
        && parts.iter().all(|part| {
            let mut chars = part.chars();
            chars.next().is_some_and(|first| first.is_ascii_alphabetic())
                && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        });
    if valid {
        Ok(name)
    } else {
        Err(Error::Refused(format!(
            "\"{}\" is not an Android package name (like com.example.app).",
            shown(name)
        )))
    }
}

/// An activity or other component inside a package: `.MainActivity` or `com.example.Main`, with
/// `$` for inner classes.
pub fn component(name: &str) -> Result<&str, Error> {
    let valid = !name.is_empty()
        && name.len() <= 255
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '$'))
        && name.chars().any(|ch| ch.is_ascii_alphabetic());
    if valid {
        Ok(name)
    } else {
        Err(Error::Refused(format!("\"{}\" is not an activity name.", shown(name))))
    }
}

/// At most 80 characters of `text`, for an error message.
pub(crate) fn shown(text: &str) -> String {
    let mut out: String = text.chars().filter(|ch| !ch.is_control()).take(80).collect();
    if text.chars().count() > 80 {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_every_shell_character() {
        assert_eq!(quote("plain").unwrap(), "'plain'");
        assert_eq!(quote("").unwrap(), "''");
        assert_eq!(quote("it's").unwrap(), "'it'\\''s'");
        assert_eq!(quote("$(reboot); `id` && rm -rf /").unwrap(), "'$(reboot); `id` && rm -rf /'");
        assert_eq!(quote("line\nbreak").unwrap(), "'line\nbreak'");
        assert!(quote("a\0b").is_err());
    }

    #[test]
    fn package_names_are_strict() {
        assert_eq!(package("com.example.app").unwrap(), "com.example.app");
        assert_eq!(package("a.b_c.D1").unwrap(), "a.b_c.D1");
        for bad in [
            "",
            "app",
            "com..app",
            ".com.app",
            "com.app.",
            "com.1app",
            "com.app; reboot",
            "-r",
            "com.app --user 0",
            "com.ap p",
            "com.app\n",
        ] {
            assert!(package(bad).is_err(), "{bad:?}");
        }
        assert!(package(&format!("a.{}", "b".repeat(300))).is_err());
    }

    #[test]
    fn component_names_allow_inner_classes_only() {
        assert!(component(".MainActivity").is_ok());
        assert!(component("com.example.Main$Inner").is_ok());
        for bad in ["", "...", "Main Activity", "Main;id", "-n", "a/b"] {
            assert!(component(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn shown_text_is_short_and_printable() {
        assert_eq!(shown("a\nb"), "ab");
        assert_eq!(shown(&"x".repeat(100)).chars().count(), 83);
    }
}
