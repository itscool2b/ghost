//! Changes one MCP entry. Unselected JSON values are borrowed raw and never exported as previews.
use serde::{
    Deserialize,
    de::{MapAccess, Visitor},
};
use serde_json::{Value, value::RawValue};

use crate::{Error, Provider, Result, Scope, Source};

pub(crate) fn path(source: &Source) -> Result<&'static str> {
    match (source.provider, source.scope, source.claude_state_only) {
        (Provider::Claude, Scope::User, true) => Ok(".claude.json"),
        (Provider::Claude, Scope::Project, false) => Ok(".mcp.json"),
        (Provider::Codex, Scope::User, false) => Ok("config.toml"),
        (Provider::Codex, Scope::Project, false) => Ok(".codex/config.toml"),
        (Provider::Pi, Scope::User, false) => Ok("mcp.json"),
        (Provider::Pi, Scope::Project, false) => Ok(".pi/mcp.json"),
        _ => Err(Error::Unsupported("Choose an MCP configuration source. Claude personal servers use its separate user MCP source.".into())),
    }
}

pub(crate) fn selected(bytes: &[u8], provider: Provider, key: &str) -> Result<Option<Value>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if provider == Provider::Codex {
        let document = crate::inventory::text(bytes)?
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| Error::Format)?;
        let Some(group) = document.get("mcp_servers") else {
            return Ok(None);
        };
        let group = group.as_table_like().ok_or(Error::Format)?;
        return group.get(key).map(crate::mcp::toml_json).transpose();
    }
    let document: RawObject<'_> = serde_json::from_slice(bytes).map_err(|_| Error::Format)?;
    let Some(group) = document.get("mcpServers") else {
        return Ok(None);
    };
    let entries: RawObject<'_> = serde_json::from_str(group.get()).map_err(|_| Error::Format)?;
    entries
        .get(key)
        .map(|entry| {
            let _: RawObject<'_> = serde_json::from_str(entry.get()).map_err(|_| Error::Format)?;
            serde_json::from_str(entry.get()).map_err(|_| Error::Format)
        })
        .transpose()
}

pub(crate) fn replace(
    bytes: &[u8],
    provider: Provider,
    key: &str,
    value: Option<&Value>,
) -> Result<Vec<u8>> {
    let text = crate::inventory::text(bytes)?;
    if provider == Provider::Codex {
        let mut document = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| Error::Format)?;
        if document.get("mcp_servers").is_none() {
            if value.is_none() {
                return Ok(bytes.into());
            }
            document["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let group = document["mcp_servers"]
            .as_table_like_mut()
            .ok_or(Error::Format)?;
        if let Some(value) = value {
            group.insert(key, toml_item(value)?);
        } else {
            group.remove(key);
        }
        return bounded(document.to_string());
    }
    let text = if text.is_empty() { "{}\n" } else { text };
    let document: RawObject<'_> = serde_json::from_str(text).map_err(|_| Error::Format)?;
    let updated = if let Some(group) = document.get("mcpServers") {
        let entries: RawObject<'_> =
            serde_json::from_str(group.get()).map_err(|_| Error::Format)?;
        let group_text = entries.replace(
            group.get(),
            key,
            value
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| Error::Format)?
                .as_deref(),
        )?;
        splice_value(text, group, &group_text)?
    } else if let Some(value) = value {
        let key = serde_json::to_string(key).map_err(|_| Error::Format)?;
        let value = serde_json::to_string(value).map_err(|_| Error::Format)?;
        document.replace(text, "mcpServers", Some(&format!("{{{key}:{value}}}")))?
    } else {
        text.to_owned()
    };
    bounded(updated)
}

fn bounded(text: String) -> Result<Vec<u8>> {
    if text.len() > crate::safe_fs::FILE_LIMIT {
        return Err(Error::Limit);
    }
    Ok(text.into_bytes())
}

fn toml_item(value: &Value) -> Result<toml_edit::Item> {
    let mut table = toml_edit::Table::new();
    for (key, value) in value.as_object().ok_or(Error::Format)? {
        let value = match value {
            Value::String(value) => toml_edit::value(value),
            Value::Bool(value) => toml_edit::value(*value),
            Value::Array(values) => {
                let mut array = toml_edit::Array::new();
                for value in values {
                    array.push(value.as_str().ok_or(Error::Format)?);
                }
                toml_edit::value(array)
            }
            _ => return Err(Error::Format),
        };
        table.insert(key, value);
    }
    Ok(toml_edit::Item::Table(table))
}

pub(crate) struct RawObject<'a>(pub(crate) Vec<(String, &'a RawValue)>);

impl<'a> RawObject<'a> {
    pub(crate) fn get(&self, key: &str) -> Option<&'a RawValue> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| *value)
    }

    pub(crate) fn replace(&self, text: &str, key: &str, value: Option<&str>) -> Result<String> {
        if let Some(index) = self.0.iter().position(|(name, _)| name == key) {
            let entry = self.0[index].1;
            if let Some(value) = value {
                return splice_value(text, entry, value);
            }
            let end = offset(text, entry)? + entry.get().len();
            let (start, end) = if index > 0 {
                let previous = self.0[index - 1].1;
                (offset(text, previous)? + previous.get().len(), end)
            } else if self.0.len() > 1 {
                let next = self.0[1].1;
                let next_value = offset(text, next)?;
                // Between two values lies one comma and the next JSON key. The key's first quote
                // is unambiguous because whitespace and commas cannot include a quoted string.
                let next_key = end + text[end..next_value].find('"').ok_or(Error::Format)?;
                (text.find('{').ok_or(Error::Format)? + 1, next_key)
            } else {
                (text.find('{').ok_or(Error::Format)? + 1, end)
            };
            return Ok(format!("{}{}", &text[..start], &text[end..]));
        }
        let Some(value) = value else {
            return Ok(text.into());
        };
        let end = text.rfind('}').ok_or(Error::Format)?;
        let key = serde_json::to_string(key).map_err(|_| Error::Format)?;
        let separator = if self.0.is_empty() { "" } else { "," };
        Ok(format!(
            "{}{separator}\n  {key}: {value}\n{}",
            &text[..end],
            &text[end..]
        ))
    }
}

fn offset(text: &str, raw: &RawValue) -> Result<usize> {
    let offset = (raw.get().as_ptr() as usize)
        .checked_sub(text.as_ptr() as usize)
        .ok_or(Error::Format)?;
    if offset
        .checked_add(raw.get().len())
        .is_none_or(|end| end > text.len())
    {
        return Err(Error::Format);
    }
    Ok(offset)
}

fn splice_value(text: &str, raw: &RawValue, replacement: &str) -> Result<String> {
    let start = offset(text, raw)?;
    Ok(format!(
        "{}{}{}",
        &text[..start],
        replacement,
        &text[start + raw.get().len()..]
    ))
}

impl<'de> Deserialize<'de> for RawObject<'de> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = RawObject<'de>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object with unique keys")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                let mut keys = std::collections::HashSet::new();
                while let Some((key, value)) = map.next_entry::<String, &'de RawValue>()? {
                    if !keys.insert(key.clone()) {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    values.push((key, value));
                }
                Ok(RawObject(values))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_edit_preserves_unselected_raw_values_and_rejects_duplicate_targets() {
        let original = br#"{ "env": {"PRIVATE":"SYNTHETIC"}, "mcpServers": { "a": {"command":"old"}, "b" : { "url" : "https://example.test" } }, "other": 1e2 }"#;
        let next = replace(original, Provider::Pi, "a", Some(&json!({"command":"new"}))).unwrap();
        assert!(
            std::str::from_utf8(&next)
                .unwrap()
                .contains("\"b\" : { \"url\" : \"https://example.test\" }")
        );
        assert!(
            std::str::from_utf8(&next)
                .unwrap()
                .ends_with(", \"other\": 1e2 }")
        );
        assert_eq!(
            selected(&next, Provider::Pi, "a").unwrap(),
            Some(json!({"command":"new"}))
        );
        for key in ["a", "b"] {
            let removed = replace(original, Provider::Pi, key, None).unwrap();
            assert_eq!(selected(&removed, Provider::Pi, key).unwrap(), None);
            assert!(serde_json::from_slice::<Value>(&removed).is_ok());
        }
        assert!(selected(br#"{"mcpServers":{},"mcpServers":{}}"#, Provider::Pi, "a").is_err());
    }

    #[test]
    fn codex_edit_preserves_unrelated_tables_comments_and_auth_fields() {
        let original = b"# fixture\nmodel = 'local'\n\n[mcp_servers.demo]\ncommand='old'\n\n[projects.'private']\ntrust_level='trusted'\n";
        let next = replace(
            original,
            Provider::Codex,
            "demo",
            Some(&json!({"command":"new","args":["run"]})),
        )
        .unwrap();
        let text = std::str::from_utf8(&next).unwrap();
        assert!(text.starts_with("# fixture\nmodel = 'local'"));
        assert!(text.ends_with("[projects.'private']\ntrust_level='trusted'\n"));
        assert_eq!(
            selected(&next, Provider::Codex, "demo").unwrap(),
            Some(json!({"command":"new","args":["run"]}))
        );
        let inline = b"mcp_servers = { docs = {command='old'}, another = {command='keep'} }\n";
        let updated = replace(
            inline,
            Provider::Codex,
            "docs",
            Some(&json!({"command":"new"})),
        )
        .unwrap();
        assert_eq!(
            selected(&updated, Provider::Codex, "docs").unwrap(),
            Some(json!({"command":"new"}))
        );
        assert_eq!(
            selected(&updated, Provider::Codex, "another").unwrap(),
            Some(json!({"command":"keep"}))
        );
    }
}
