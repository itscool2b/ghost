use serde_json::{Map, Value, json};

use crate::{Error, Provider, Result};

/// The copyable subset is deliberately closed. Unknown vendor fields are not discarded to make a
/// conversion appear successful. Values from credential-bearing fields never enter a preview.
pub(crate) fn preview(value: &Value, provider: Provider) -> (String, Option<String>) {
    match portable(value, provider, provider) {
        Ok(value) => (
            serde_json::to_string_pretty(&value).unwrap_or_default(),
            None,
        ),
        Err(error) => {
            let fields = value
                .as_object()
                .map(|object| object.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            (serde_json::to_string_pretty(&json!({"fields": fields, "values": "Hidden because this entry requires a manual review in the agent's configuration."})).unwrap_or_default(), Some(error.to_string()))
        }
    }
}

pub(crate) fn portable(value: &Value, from: Provider, to: Provider) -> Result<Value> {
    let object = value.as_object().ok_or(Error::Format)?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "command" | "args" | "url" | "type" | "enabled"
        )
    }) {
        return Err(Error::Unsupported("This server has authentication, environment, or vendor options that require manual setup. No values will be copied.".into()));
    }
    let mut result = Map::new();
    match (object.get("command"), object.get("url")) {
        (Some(Value::String(command)), None) => {
            if command.is_empty()
                || command.len() > 4096
                || sensitive(command)
                || command.contains(['\n', '\r', '\0'])
            {
                return Err(Error::Unsupported(
                    "This server command requires manual review.".into(),
                ));
            }
            if object
                .get("type")
                .is_some_and(|kind| kind.as_str() != Some("stdio"))
            {
                return Err(Error::Format);
            }
            result.insert("command".into(), Value::String(command.clone()));
            if let Some(args) = object.get("args") {
                let args = args.as_array().ok_or(Error::Format)?;
                if args.len() > 128
                    || args
                        .iter()
                        .any(|arg| arg.as_str().is_none_or(|s| s.len() > 4096 || sensitive(s)))
                {
                    return Err(Error::Unsupported("Server arguments may contain authentication or executable interpolation. Review them in the agent's configuration.".into()));
                }
                result.insert("args".into(), Value::Array(args.clone()));
            }
        }
        (None, Some(Value::String(address))) => {
            let url = url::Url::parse(address).map_err(|_| Error::Format)?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || sensitive(url.path())
            {
                return Err(Error::Unsupported("This server URL may contain credentials or unsupported parameters. Configure it manually.".into()));
            }
            if object.contains_key("args") {
                return Err(Error::Format);
            }
            let kind = object
                .get("type")
                .map(|value| value.as_str().ok_or(Error::Format))
                .transpose()?;
            if (from == Provider::Claude && kind.is_none())
                || kind.is_some_and(|kind| {
                    kind != "http"
                        && !(matches!(from, Provider::Pi | Provider::Claude)
                            && kind == "streamable-http")
                })
            {
                return Err(Error::Unsupported(
                    "This transport cannot be converted without changing its behavior.".into(),
                ));
            }
            result.insert("url".into(), Value::String(address.clone()));
            if to == Provider::Claude {
                result.insert("type".into(), Value::String("http".into()));
            }
        }
        _ => return Err(Error::Format),
    }
    if let Some(enabled) = object.get("enabled") {
        let enabled = enabled.as_bool().ok_or(Error::Format)?;
        if to == Provider::Claude {
            // Claude's state is represented separately from an MCP server declaration.
            if !enabled {
                return Err(Error::Unsupported(
                    "A disabled server cannot be enabled by copying it to Claude.".into(),
                ));
            }
        } else {
            result.insert("enabled".into(), Value::Bool(enabled));
        }
    }
    Ok(Value::Object(result))
}

fn sensitive(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "api-key",
        "apikey",
        "authorization",
        "bearer ",
        "sk-",
        "ghp_",
        "github_pat_",
        "${",
        "$(",
        "`",
        "-----begin",
    ]
    .iter()
    .any(|part| lower.contains(part))
}

pub(crate) fn toml_json(item: &toml_edit::Item) -> Result<Value> {
    if let Some(table) = item.as_table_like() {
        let mut object = Map::new();
        for (key, value) in table.iter() {
            object.insert(key.into(), toml_json(value)?);
        }
        return Ok(Value::Object(object));
    }
    value_json(item.as_value().ok_or(Error::Format)?)
}

fn value_json(value: &toml_edit::Value) -> Result<Value> {
    Ok(match value {
        toml_edit::Value::String(value) => json!(value.value()),
        toml_edit::Value::Integer(value) => json!(value.value()),
        toml_edit::Value::Float(value) => json!(value.value()),
        toml_edit::Value::Boolean(value) => json!(value.value()),
        toml_edit::Value::Array(array) => {
            Value::Array(array.iter().map(value_json).collect::<Result<_>>()?)
        }
        toml_edit::Value::InlineTable(table) => Value::Object(
            table
                .iter()
                .map(|(key, value)| Ok((key.to_owned(), value_json(value)?)))
                .collect::<Result<_>>()?,
        ),
        toml_edit::Value::Datetime(_) => return Err(Error::Format),
    })
}

/// An agent's MCP server entry as one native `mcpServers` value that Ghost's shared MCP can
/// import for every agent. Wider than [`portable`]: `env` and `headers` are allowed, because the
/// shared import keeps their values as this device's local credentials (never exported)
/// and only their names in the definition. The command, its arguments and the URL are checked as
/// for a copy, since they become part of the shared definition. Vendor options still refuse.
pub(crate) fn shareable(value: &Value, from: Provider) -> Result<Value> {
    let object = value.as_object().ok_or(Error::Format)?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "command" | "args" | "url" | "type" | "enabled" | "env" | "headers"
        )
    }) {
        return Err(Error::Unsupported("This server has vendor options that every agent cannot take. Share it by entering a definition in Shared MCP.".into()));
    }
    let strings = |key: &str| -> Result<Option<Map<String, Value>>> {
        match object.get(key) {
            None => Ok(None),
            Some(Value::Object(map))
                if map.len() <= 128 && map.values().all(|value| value.is_string()) =>
            {
                Ok(Some(map.clone()))
            }
            Some(_) => Err(Error::Format),
        }
    };
    let env = strings("env")?;
    let headers = strings("headers")?;
    let mut core = object.clone();
    core.remove("env");
    core.remove("headers");
    let remote = object.contains_key("url");
    // SSE is a transport shared MCP knows, though a copy between agents cannot carry it.
    let sse = remote && object.get("type").and_then(Value::as_str) == Some("sse");
    if sse {
        core.insert("type".into(), Value::String("http".into()));
    } else if remote && from == Provider::Claude && !core.contains_key("type") {
        return Err(Error::Unsupported(
            "This server's transport is not named. Review it in Claude Code's configuration."
                .into(),
        ));
    }
    let mut result = portable(&Value::Object(core), from, Provider::Pi)?;
    let map = result.as_object_mut().ok_or(Error::Format)?;
    if remote {
        map.insert(
            "type".into(),
            Value::String(if sse { "sse" } else { "http" }.into()),
        );
    }
    if headers.is_some() && !remote {
        return Err(Error::Format);
    }
    if let Some(env) = env {
        map.insert("env".into(), Value::Object(env));
    }
    if let Some(headers) = headers {
        map.insert("headers".into(), Value::Object(headers));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_http_requires_its_native_discriminator_but_supports_the_documented_alias() {
        assert!(
            portable(
                &json!({"url":"https://example.test/mcp"}),
                Provider::Claude,
                Provider::Pi
            )
            .is_err()
        );
        let converted = portable(
            &json!({"type":"streamable-http", "url":"https://example.test/mcp"}),
            Provider::Claude,
            Provider::Codex,
        )
        .unwrap();
        assert_eq!(converted, json!({"url":"https://example.test/mcp"}));
        assert!(
            portable(
                &json!({"type":"sse", "url":"https://example.test/sse"}),
                Provider::Claude,
                Provider::Pi
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod share_tests {
    use super::*;

    #[test]
    fn a_server_shares_with_its_credentials_kept_apart_and_its_command_checked() {
        let local = json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-github"],
            "env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "value-stays-on-this-device"}
        });
        assert_eq!(
            shareable(&local, Provider::Claude).unwrap(),
            json!({
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"],
                "env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "value-stays-on-this-device"}
            })
        );
        let remote = json!({"type": "http", "url": "https://mcp.example.test/mcp", "headers": {"X-Key": "local"}});
        assert_eq!(
            shareable(&remote, Provider::Claude).unwrap(),
            json!({"url": "https://mcp.example.test/mcp", "type": "http", "headers": {"X-Key": "local"}})
        );
        let events = json!({"type": "sse", "url": "https://mcp.example.test/sse"});
        assert_eq!(
            shareable(&events, Provider::Claude).unwrap()["type"],
            json!("sse")
        );
        // Secrets in the command line or address would become part of the shared definition.
        for refused in [
            json!({"command": "server", "args": ["--token", "abc"]}),
            json!({"url": "https://user:pass@example.test/mcp", "type": "http"}),
            json!({"command": "server", "startup_timeout_sec": 30}),
            json!({"command": "server", "env": {"KEY": 1}}),
            json!({"command": "server", "headers": {"X": "y"}}),
            json!({"url": "https://example.test/mcp"}),
        ] {
            assert!(shareable(&refused, Provider::Claude).is_err(), "{refused}");
        }
        // Codex names no transport for a URL; it is HTTP.
        assert_eq!(
            shareable(&json!({"url": "https://example.test/mcp"}), Provider::Codex).unwrap(),
            json!({"url": "https://example.test/mcp", "type": "http"})
        );
    }
}
