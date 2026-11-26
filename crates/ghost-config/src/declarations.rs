//! One top-level field of a provider's settings document, read so a reviewed action can tell
//! whether the declarations it was reviewed against are still the ones on disk.
//!
//! The Plugin library's install, update and remove commands hash these fields at review and
//! again before they start. Reads are bounded, relative to the approved root and never follow a
//! link; a document that cannot be read or parsed is an error, never an absent field.

use serde_json::{Value, value::RawValue};

use crate::{Error, Result, Source, inventory, mcp_document::RawObject, safe_fs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFormat {
    Json,
    Toml,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declaration {
    /// The document, or this field in it, does not exist.
    Absent,
    /// The field's value as canonical JSON: object keys sorted, so two readings of the same
    /// declarations give the same bytes whatever order the file lists them in.
    Present(Vec<u8>),
}

/// Reads `field` from the document at `path` under the source's root.
pub fn read(
    source: &Source,
    path: &str,
    format: DocumentFormat,
    field: &str,
) -> Result<Declaration> {
    let root = inventory::open(source)?;
    let Some(bytes) = safe_fs::read(&root, path)? else {
        return Ok(Declaration::Absent);
    };
    let document = parse(inventory::text(&bytes)?, format)?;
    let Some(value) = document.as_object().ok_or(Error::Format)?.get(field) else {
        return Ok(Declaration::Absent);
    };
    let mut value = value.clone();
    canonicalize(&mut value);
    serde_json::to_vec(&value)
        .map(Declaration::Present)
        .map_err(|_| Error::Format)
}

fn parse(text: &str, format: DocumentFormat) -> Result<Value> {
    match format {
        DocumentFormat::Toml => {
            let document = text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|_| Error::Format)?;
            let mut value = serde_json::Map::new();
            for (key, item) in document.iter() {
                value.insert(key.into(), crate::mcp::toml_json(item)?);
            }
            Ok(Value::Object(value))
        }
        DocumentFormat::Json => {
            let raw: &RawValue = serde_json::from_str(text).map_err(|_| Error::Format)?;
            strict_value(raw, 0)
        }
    }
}

/// JSON with bounded depth and width, so a hostile settings file cannot make a review expensive.
fn strict_value(raw: &RawValue, depth: usize) -> Result<Value> {
    if depth > 16 {
        return Err(Error::Limit);
    }
    let text = raw.get().trim_start();
    if text.starts_with('{') {
        let object: RawObject<'_> = serde_json::from_str(text).map_err(|_| Error::Format)?;
        if object.0.len() > 2048 {
            return Err(Error::Limit);
        }
        let mut result = serde_json::Map::new();
        for (key, value) in object.0 {
            result.insert(key, strict_value(value, depth + 1)?);
        }
        Ok(Value::Object(result))
    } else if text.starts_with('[') {
        let values: Vec<&RawValue> = serde_json::from_str(text).map_err(|_| Error::Format)?;
        if values.len() > 1024 {
            return Err(Error::Limit);
        }
        values
            .into_iter()
            .map(|value| strict_value(value, depth + 1))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array)
    } else {
        serde_json::from_str(text).map_err(|_| Error::Format)
    }
}

fn canonicalize(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.sort_keys();
            for value in object.values_mut() {
                canonicalize(value);
            }
        }
        Value::Array(array) => {
            for value in array {
                canonicalize(value);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Provider, Scope};

    fn source(root: &std::path::Path) -> Source {
        Source {
            id: "user:claude".into(),
            provider: Provider::Claude,
            scope: Scope::User,
            root: root.into(),
            claude_state_only: false,
        }
    }

    #[test]
    fn a_field_reads_the_same_in_any_key_order_and_changes_with_its_value() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("settings.json");
        let read = || {
            read(
                &source(temp.path()),
                "settings.json",
                DocumentFormat::Json,
                "enabledPlugins",
            )
        };
        assert_eq!(read().unwrap(), Declaration::Absent, "no document");
        std::fs::write(&file, r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(read().unwrap(), Declaration::Absent, "no field");
        std::fs::write(&file, r#"{"enabledPlugins":{"b@m":true,"a@m":false}}"#).unwrap();
        let first = read().unwrap();
        assert_eq!(
            first,
            Declaration::Present(br#"{"a@m":false,"b@m":true}"#.to_vec())
        );
        std::fs::write(
            &file,
            r#"{"enabledPlugins":{"a@m":false,"b@m":true},"x":1}"#,
        )
        .unwrap();
        assert_eq!(
            read().unwrap(),
            first,
            "key order and other fields do not matter"
        );
        std::fs::write(&file, r#"{"enabledPlugins":{"a@m":true,"b@m":true}}"#).unwrap();
        assert_ne!(read().unwrap(), first);
    }

    #[test]
    fn toml_tables_read_as_the_same_declaration_model() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("config.toml"),
            "model = \"x\"\n[plugins.\"tool@market\"]\nenabled = true\n",
        )
        .unwrap();
        let read = |field| {
            read(
                &source(temp.path()),
                "config.toml",
                DocumentFormat::Toml,
                field,
            )
        };
        assert_eq!(
            read("plugins").unwrap(),
            Declaration::Present(br#"{"tool@market":{"enabled":true}}"#.to_vec())
        );
        assert_eq!(read("marketplaces").unwrap(), Declaration::Absent);
    }

    #[test]
    fn an_unreadable_document_is_an_error_and_never_an_absent_field() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("settings.json");
        let read = || {
            read(
                &source(temp.path()),
                "settings.json",
                DocumentFormat::Json,
                "packages",
            )
        };
        std::fs::write(&file, "{ not json").unwrap();
        assert!(matches!(read(), Err(Error::Format)));
        std::fs::write(&file, "[1, 2]").unwrap();
        assert!(matches!(read(), Err(Error::Format)), "not an object");
        let deep = format!("{}1{}", "[".repeat(40), "]".repeat(40));
        std::fs::write(&file, format!(r#"{{"packages":{deep}}}"#)).unwrap();
        assert!(matches!(read(), Err(Error::Limit)));
        #[cfg(unix)]
        {
            std::fs::remove_file(&file).unwrap();
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("real.json"), r#"{"packages":[]}"#).unwrap();
            std::os::unix::fs::symlink(outside.path().join("real.json"), &file).unwrap();
            assert!(read().is_err(), "a link is never followed");
        }
    }
}
