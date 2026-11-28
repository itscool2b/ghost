//! Shared MCP persistence, called inside the profile writer transaction.
//! Local credential values never enter exports.
use crate::chatstore::StoreError;
use ghost_protocol::mcp::*;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};

pub enum Command {
    Snapshot,
    Save {
        expected: u32,
        definition: McpDefinition,
    },
    Delete {
        expected: u32,
        id: String,
        project_id: Option<String>,
    },
    Credentials {
        expected: u32,
        id: String,
        project_id: Option<String>,
        credentials: McpCredentials,
    },
    Import {
        expected: u32,
        document: McpExport,
    },
    ImportNative {
        expected: u32,
        project_id: Option<String>,
        document: String,
    },
    /// `agent`: only the servers that agent loads; None (Ghost's own connection test) takes
    /// every server in scope.
    Resolve {
        project_id: Option<String>,
        agent: Option<String>,
        native_names: Vec<String>,
    },
    /// `agent`: the agent the chat is starting; None reads the chat's own.
    ResolveChat {
        chat_id: String,
        agent: Option<String>,
        native_names: Vec<String>,
    },
    TestFolder {
        project_id: String,
        device_id: String,
    },
}
pub enum Reply {
    Snapshot(McpSnapshot),
    Resolved(Vec<ResolvedMcpServer>),
    Folder(String),
}
impl Command {
    pub(crate) fn writes(&self) -> bool {
        !matches!(
            self,
            Self::Snapshot
                | Self::Resolve { .. }
                | Self::ResolveChat { .. }
                | Self::TestFolder { .. }
        )
    }
    pub(crate) fn bytes(&self) -> usize {
        1024 + match self {
            Self::Credentials { credentials, .. } => credentials
                .env
                .iter()
                .chain(&credentials.headers)
                .map(|(k, v)| k.len() + v.len() + 16)
                .sum(),
            Self::ImportNative { document, .. } => document.len(),
            Self::Save { definition, .. } => {
                serde_json::to_vec(definition).map_or(24 * 1024, |v| v.len())
            }
            Self::Import { document, .. } => document
                .definitions
                .iter()
                .map(|d| serde_json::to_vec(d).map_or(24 * 1024, |v| v.len()))
                .sum(),
            _ => 1024,
        }
    }
}
fn invalid(message: &str) -> StoreError {
    StoreError::Other(message.into())
}
fn db(error: rusqlite::Error) -> StoreError {
    StoreError::Other(format!("Shared MCP storage failed: {error}"))
}
fn json<T: serde::Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|_| invalid("The MCP definition could not be encoded."))
}
fn valid_uuid(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}
/// The definition as it is stored: its agents as agent IDs (a connection such as Claude Code's
/// ACP adapter counts as its agent), sorted and without repeats.
pub fn normalized(mut def: McpDefinition) -> McpDefinition {
    if let Some(agents) = &mut def.agents {
        for agent in agents.iter_mut() {
            *agent = ghost_agents::agent_of(agent).to_owned();
        }
        agents.sort();
        agents.dedup();
    }
    def
}
/// Whether `agent` loads this definition: every agent when it names none.
pub fn for_agent(def: &McpDefinition, agent: &str) -> bool {
    def.agents
        .as_ref()
        .is_none_or(|agents| agents.iter().any(|a| a == ghost_agents::agent_of(agent)))
}
pub fn validate(def: &McpDefinition) -> Result<(), StoreError> {
    if serde_json::to_vec(def)
        .map_err(|_| invalid("The MCP definition is invalid."))?
        .len()
        > 24 * 1024
    {
        return Err(invalid(
            "One portable MCP definition must fit within 24 KB.",
        ));
    }
    let key = |s: &str| {
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    };
    if !valid_uuid(&def.id)
        || !key(&def.name)
        || matches!(
            def.name.replace('-', "_").as_str(),
            "ghost_project_memory"
                | "ghost_browser"
                | "ghost_mobile"
                | "ghost_game"
                | "ghost_project"
        )
        || def.project_id.as_ref().is_some_and(|p| !valid_uuid(p))
    {
        return Err(invalid(
            "Use a stable server UUID, a server name containing letters, numbers, dots, underscores or hyphens, and a valid project. Ghost's internal server names are reserved.",
        ));
    }
    if let Some(agents) = &def.agents {
        let agent = |id: &String| {
            !id.is_empty()
                && id.len() <= 32
                && id.as_bytes()[0].is_ascii_lowercase()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        // Sorted and unique, as `normalized` leaves it: one spelling per scope, so definitions
        // compare equal across devices.
        if agents.is_empty()
            || agents.len() > 32
            || !agents.iter().all(agent)
            || !agents.windows(2).all(|pair| pair[0] < pair[1])
        {
            return Err(invalid(
                "Choose at least one agent for this server, or make it available to every agent.",
            ));
        }
    }
    for keys in [&def.env_keys, &def.header_keys] {
        if keys.len() > 128
            || keys.iter().any(|name| !key(name))
            || keys.iter().collect::<BTreeSet<_>>().len() != keys.len()
        {
            return Err(invalid(
                "Credential names must be unique and contain only letters, numbers, dots, underscores or hyphens.",
            ));
        }
    }
    match &def.transport {
        McpTransport::Stdio { command, args } => {
            if command.is_empty()
                || command.starts_with('-')
                || command.len() > 4096
                || command.chars().any(char::is_control)
                || args.len() > 128
                || args.iter().any(|a| a.len() > 4096 || a.contains('\0'))
                || !def.header_keys.is_empty()
            {
                return Err(invalid("The stdio command or arguments are invalid."));
            }
            if args.iter().any(|a| {
                ["--token", "--api-key", "--password", "--secret"]
                    .iter()
                    .any(|flag| a == flag || a.starts_with(&format!("{flag}=")))
            }) {
                return Err(invalid(
                    "Put authentication in local environment credentials, not portable command arguments.",
                ));
            }
        }
        McpTransport::Http { url } | McpTransport::Sse { url } => {
            let endpoint = reqwest::Url::parse(url)
                .map_err(|_| invalid("The MCP endpoint URL is invalid."))?;
            if endpoint.scheme() != "https"
                && !(endpoint.scheme() == "http"
                    && endpoint
                        .host_str()
                        .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "[::1]")))
            {
                return Err(invalid("Use HTTPS, or HTTP for a loopback MCP endpoint."));
            }
            if url.len() > 4096
                || endpoint.host_str().is_none()
                || !endpoint.username().is_empty()
                || endpoint.password().is_some()
                || endpoint.query().is_some()
                || endpoint.fragment().is_some()
                || !def.env_keys.is_empty()
            {
                return Err(invalid(
                    "Remote URLs cannot contain credentials, query parameters or fragments. Store authentication in local headers.",
                ));
            }
        }
    }
    Ok(())
}
fn scope(project: Option<&str>) -> &str {
    project.unwrap_or("")
}
fn revision(conn: &Connection) -> Result<u32, StoreError> {
    conn.query_row(
        "SELECT revision FROM shared_mcp_settings WHERE id=1",
        [],
        |r| r.get(0),
    )
    .map_err(db)
}
fn check(conn: &Connection, expected: u32) -> Result<(), StoreError> {
    if revision(conn)? != expected {
        return Err(invalid(
            "Shared MCP changed elsewhere. Refresh and review your changes before saving.",
        ));
    }
    Ok(())
}
fn bump(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE shared_mcp_settings SET revision=revision+1 WHERE id=1",
        [],
    )
    .map_err(db)?;
    Ok(())
}
fn rows(conn: &Connection) -> Result<Vec<(McpDefinition, u32, McpCredentials)>, StoreError> {
    conn.prepare("SELECT definition_json,revision,local_json FROM shared_mcp WHERE scope='' OR EXISTS(SELECT 1 FROM projects WHERE projects.id=shared_mcp.scope AND deleted=0) ORDER BY scope,server_id").map_err(db)?.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,u32>(1)?,r.get::<_,String>(2)?))).map_err(db)?.map(|r| {
        let (definition,revision,local) = r.map_err(db)?;
        let def = serde_json::from_str::<McpDefinition>(&definition).map_err(|_| invalid("An MCP definition is damaged."))?;
        validate(&def)?;
        Ok((def,revision,serde_json::from_str(&local).map_err(|_| invalid("Local MCP credentials are damaged."))?))
    }).collect()
}
fn missing(def: &McpDefinition, local: &McpCredentials) -> Vec<String> {
    def.env_keys
        .iter()
        .filter(|key| !local.env.contains_key(*key))
        .map(|k| format!("env/{k}"))
        .chain(
            def.header_keys
                .iter()
                .filter(|key| !local.headers.contains_key(*key))
                .map(|k| format!("headers/{k}")),
        )
        .collect()
}
fn snapshot(conn: &Connection) -> Result<McpSnapshot, StoreError> {
    Ok(McpSnapshot {
        revision: revision(conn)?,
        servers: rows(conn)?
            .into_iter()
            .map(|(definition, revision, local)| McpRecord {
                missing_credentials: missing(&definition, &local),
                definition,
                revision,
            })
            .collect(),
    })
}
fn apply_definition(conn: &Connection, def: &McpDefinition) -> Result<(), StoreError> {
    validate(def)?;
    if let Some(project) = &def.project_id
        && !conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1 AND deleted=0)",
                [project],
                |r| r.get::<_, bool>(0),
            )
            .map_err(db)?
    {
        return Err(invalid("This project is unavailable."));
    }
    let prior: Option<String> = conn
        .query_row(
            "SELECT definition_json FROM shared_mcp WHERE scope=?1 AND server_id=?2",
            params![scope(def.project_id.as_deref()), def.id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db)?;
    let preserve = prior
        .as_ref()
        .and_then(|s| serde_json::from_str::<McpDefinition>(s).ok())
        .is_some_and(|old| {
            old.transport == def.transport
                && old.env_keys == def.env_keys
                && old.header_keys == def.header_keys
        });
    conn.execute("INSERT INTO shared_mcp(scope,server_id,revision,definition_json,local_json) VALUES(?1,?2,1,?3,?4) ON CONFLICT(scope,server_id) DO UPDATE SET revision=revision+1,definition_json=excluded.definition_json,local_json=CASE WHEN ?5 THEN local_json ELSE excluded.local_json END",params![scope(def.project_id.as_deref()),def.id,json(def)?,json(&McpCredentials::default())?,preserve]).map_err(db)?;
    Ok(())
}
pub(crate) fn execute(conn: &Connection, command: Command) -> Result<Reply, StoreError> {
    match command {
        Command::TestFolder {
            project_id,
            device_id,
        } => {
            let paths=conn.prepare("SELECT DISTINCT fl.path FROM project_folders pf JOIN projects p ON p.id=pf.project_id JOIN folder_locations fl ON fl.folder_id=pf.id AND fl.device_id=?2 WHERE pf.project_id=?1 AND pf.deleted=0 AND p.deleted=0 LIMIT 2").map_err(db)?.query_map(params![project_id,device_id],|r|r.get::<_,String>(0)).map_err(db)?.collect::<rusqlite::Result<Vec<_>>>().map_err(db)?;
            if paths.len() != 1 {
                return Err(invalid(
                    "Select an active native chat in this project before testing. The project does not have one unambiguous local folder.",
                ));
            }
            return Ok(Reply::Folder(paths[0].clone()));
        }
        Command::ResolveChat {
            chat_id,
            agent,
            native_names,
        } => {
            let (project_id, active): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT project_id,active_agent FROM chats WHERE id=?1 AND deleted=0",
                    [chat_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(db)?;
            return execute(
                conn,
                Command::Resolve {
                    project_id,
                    agent: agent.or(active),
                    native_names,
                },
            );
        }
        Command::Snapshot => {}
        Command::Save {
            expected,
            definition,
        } => {
            check(conn, expected)?;
            apply_definition(conn, &normalized(definition))?;
            bump(conn)?;
        }
        Command::Delete {
            expected,
            id,
            project_id,
        } => {
            check(conn, expected)?;
            if !valid_uuid(&id) || project_id.as_ref().is_some_and(|p| !valid_uuid(p)) {
                return Err(invalid("The MCP server identity or scope is invalid."));
            }
            conn.execute(
                "DELETE FROM shared_mcp WHERE scope=?1 AND server_id=?2",
                params![scope(project_id.as_deref()), id],
            )
            .map_err(db)?;
            bump(conn)?;
        }
        Command::Credentials {
            expected,
            id,
            project_id,
            credentials,
        } => {
            check(conn, expected)?;
            let (def, _, _) = rows(conn)?
                .into_iter()
                .find(|(d, _, _)| d.id == id && d.project_id == project_id)
                .ok_or_else(|| invalid("This MCP definition no longer exists."))?;
            if credentials.env.keys().any(|k| !def.env_keys.contains(k))
                || credentials
                    .headers
                    .keys()
                    .any(|k| !def.header_keys.contains(k))
                || credentials
                    .env
                    .values()
                    .chain(credentials.headers.values())
                    .any(|v| v.is_empty() || v.len() > 16384 || v.contains(['\0', '\r', '\n']))
            {
                return Err(invalid(
                    "Supply only the named local credentials using nonempty single-line values.",
                ));
            }
            conn.execute(
                "UPDATE shared_mcp SET local_json=?3 WHERE scope=?1 AND server_id=?2",
                params![scope(project_id.as_deref()), id, json(&credentials)?],
            )
            .map_err(db)?;
            bump(conn)?;
        }
        Command::Import { expected, document } => {
            check(conn, expected)?;
            if document.version != 1 || document.definitions.len() > 128 {
                return Err(invalid(
                    "This shared MCP export version or size is unsupported.",
                ));
            }
            let definitions: Vec<_> = document.definitions.into_iter().map(normalized).collect();
            let mut ids = BTreeSet::new();
            for def in &definitions {
                validate(def)?;
                if !ids.insert((def.id.clone(), def.project_id.clone())) {
                    return Err(invalid(
                        "The import repeats a server identity in the same scope.",
                    ));
                }
            }
            for def in definitions {
                apply_definition(conn, &def)?;
            }
            bump(conn)?;
        }
        Command::ImportNative {
            expected,
            project_id,
            document,
        } => {
            check(conn, expected)?;
            if document.len() > 256 * 1024 {
                return Err(invalid("The native MCP import exceeds 256 KB."));
            }
            let value: serde_json::Value = serde_json::from_str(&document)
                .map_err(|_| invalid("Use native MCP JSON with a top-level mcpServers object."))?;
            let servers = value["mcpServers"]
                .as_object()
                .filter(|s| s.len() <= 128)
                .ok_or_else(|| invalid("Use native MCP JSON with up to 128 mcpServers entries."))?;
            let existing = rows(conn)?;
            for (name, value) in servers {
                let object = value
                    .as_object()
                    .ok_or_else(|| invalid("The native server entry must be an object."))?;
                if object.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "type" | "command" | "args" | "env" | "headers" | "url" | "enabled"
                    )
                }) {
                    return Err(invalid(
                        "This native entry has vendor-specific options. Review it and enter a portable shared definition without discarding behavior.",
                    ));
                }
                if object.contains_key("command") && object.contains_key("url")
                    || object.get("command").is_some_and(|v| !v.is_string())
                    || object.get("url").is_some_and(|v| !v.is_string())
                    || object.get("enabled").is_some_and(|v| !v.is_boolean())
                    || object.get("type").is_some_and(|v| !v.is_string())
                {
                    return Err(invalid(
                        "The native transport or enabled state is ambiguous or invalid. Review this entry explicitly.",
                    ));
                }
                if object.contains_key("command")
                    && value["type"].as_str().is_some_and(|kind| kind != "stdio")
                {
                    return Err(invalid(
                        "A native command cannot be imported with a remote transport type.",
                    ));
                }
                let transport = if let Some(command) = value["command"].as_str() {
                    McpTransport::Stdio {
                        command: command.into(),
                        args: match value.get("args") {
                            None => Vec::new(),
                            Some(args) => serde_json::from_value(args.clone()).map_err(|_| {
                                invalid("Native command arguments must be strings.")
                            })?,
                        },
                    }
                } else {
                    let url = value["url"]
                        .as_str()
                        .ok_or_else(|| invalid("The native server requires a command or URL."))?
                        .to_owned();
                    match value["type"].as_str() {
                        Some("sse") => McpTransport::Sse { url },
                        None | Some("http" | "streamable-http") => McpTransport::Http { url },
                        _ => return Err(invalid("The native transport is unsupported.")),
                    }
                };
                let values = |key: &str| -> Result<BTreeMap<String, String>, StoreError> {
                    value
                        .get(key)
                        .map(|values| {
                            serde_json::from_value(values.clone())
                                .map_err(|_| invalid("Native credentials must be string maps."))
                        })
                        .unwrap_or_else(|| Ok(BTreeMap::new()))
                };
                let local = McpCredentials {
                    env: values("env")?,
                    headers: values("headers")?,
                };
                // Importing again over a server keeps its identity and the agents it is for.
                let prior = existing
                    .iter()
                    .find(|(d, _, _)| d.name == *name && d.project_id == project_id)
                    .map(|(d, _, _)| d);
                let definition = McpDefinition {
                    id: prior
                        .map(|d| d.id.clone())
                        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
                    name: name.clone(),
                    project_id: project_id.clone(),
                    enabled: value["enabled"].as_bool().unwrap_or(true),
                    transport,
                    env_keys: local.env.keys().cloned().collect(),
                    header_keys: local.headers.keys().cloned().collect(),
                    agents: prior.and_then(|d| d.agents.clone()),
                };
                validate(&definition)?;
                if existing.iter().any(|(d, _, _)| {
                    d.name == definition.name
                        && d.project_id == definition.project_id
                        && *d != definition
                }) {
                    return Err(invalid(
                        "A shared definition with this native name already exists. Review and edit that definition explicitly, or import under a new name.",
                    ));
                }
                if local
                    .env
                    .values()
                    .chain(local.headers.values())
                    .any(|v| v.is_empty() || v.len() > 16384 || v.contains(['\0', '\r', '\n']))
                {
                    return Err(invalid(
                        "Native credentials must be bounded single-line values.",
                    ));
                }
                apply_definition(conn, &definition)?;
                conn.execute(
                    "UPDATE shared_mcp SET local_json=?3 WHERE scope=?1 AND server_id=?2",
                    params![scope(project_id.as_deref()), definition.id, json(&local)?],
                )
                .map_err(db)?;
            }
            bump(conn)?;
        }
        Command::Resolve {
            project_id,
            agent,
            native_names,
        } => {
            let mut selected = BTreeMap::new();
            let rows = rows(conn)?;
            for project in [None, project_id.as_deref()] {
                if project.is_none() && !selected.is_empty() {
                    continue;
                }
                for (def, revision, local) in &rows {
                    if def.project_id.as_deref() == project {
                        selected.insert(
                            def.id.clone(),
                            ResolvedMcpServer {
                                definition: def.clone(),
                                revision: *revision,
                                credentials: local.clone(),
                            },
                        );
                    }
                }
            }
            let mut names = BTreeSet::new();
            let mut result = Vec::new();
            // The project override wins first; then only what this agent loads, so a server's name
            // only clashes in the agents it is for.
            for server in selected.into_values().filter(|s| {
                s.definition.enabled
                    && agent
                        .as_deref()
                        .is_none_or(|agent| for_agent(&s.definition, agent))
            }) {
                let normalized = server.definition.name.replace('-', "_");
                if native_names
                    .iter()
                    .any(|name| name.replace('-', "_") == normalized)
                    || !names.insert(normalized)
                {
                    return Err(invalid(
                        "A shared MCP name conflicts with a native or shared server. Review and rename the shared server before starting.",
                    ));
                }
                if !missing(&server.definition, &server.credentials).is_empty() {
                    continue;
                }
                result.push(server);
            }
            return Ok(Reply::Resolved(result));
        }
    }
    Ok(Reply::Snapshot(snapshot(conn)?))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY,deleted INTEGER NOT NULL DEFAULT 0); CREATE TABLE chats(id TEXT PRIMARY KEY,project_id TEXT,active_agent TEXT,deleted INTEGER NOT NULL DEFAULT 0);").unwrap();
        conn.execute_batch(include_str!("../migrations/0018_shared_mcp.sql"))
            .unwrap();
        conn
    }
    pub(crate) fn definition() -> McpDefinition {
        McpDefinition {
            id: "61a6ce00-b99f-4f8a-9148-8f880f3c735f".into(),
            name: "docs".into(),
            project_id: None,
            enabled: true,
            transport: McpTransport::Stdio {
                command: "node".into(),
                args: vec!["server.js".into()],
            },
            env_keys: Vec::new(),
            header_keys: Vec::new(),
            agents: None,
        }
    }
    fn saved(conn: &Connection) -> McpSnapshot {
        let Reply::Snapshot(v) = execute(conn, Command::Snapshot).unwrap() else {
            panic!()
        };
        v
    }
    fn resolved(
        conn: &Connection,
        project_id: Option<String>,
        names: Vec<String>,
    ) -> Result<Vec<ResolvedMcpServer>, StoreError> {
        let Reply::Resolved(v) = execute(
            conn,
            Command::Resolve {
                project_id,
                agent: None,
                native_names: names,
            },
        )?
        else {
            panic!()
        };
        Ok(v)
    }
    #[test]
    fn project_identity_overrides_all_projects_and_disabled_override_does_not_fall_through() {
        let conn = fixture();
        let mut def = definition();
        let id = uuid::Uuid::now_v7().to_string();
        conn.execute("INSERT INTO projects(id)VALUES(?1)", [&id])
            .unwrap();
        execute(
            &conn,
            Command::Save {
                expected: 0,
                definition: def.clone(),
            },
        )
        .unwrap();
        def.project_id = Some(id.clone());
        def.name = "project_docs".into();
        execute(
            &conn,
            Command::Save {
                expected: 1,
                definition: def.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            resolved(&conn, Some(id.clone()), vec![]).unwrap()[0]
                .definition
                .name,
            "project_docs"
        );
        assert_eq!(
            resolved(&conn, None, vec![]).unwrap()[0].definition.name,
            "docs"
        );
        def.enabled = false;
        execute(
            &conn,
            Command::Save {
                expected: 2,
                definition: def,
            },
        )
        .unwrap();
        assert!(resolved(&conn, Some(id), vec![]).unwrap().is_empty());
    }
    #[test]
    fn native_collisions_require_review_and_cannot_silently_replace_native_config() {
        let conn = fixture();
        execute(
            &conn,
            Command::Save {
                expected: 0,
                definition: definition(),
            },
        )
        .unwrap();
        assert!(resolved(&conn, None, vec!["docs".into()]).is_err());
    }
    #[test]
    fn credentials_never_enter_snapshots_debug_exports_or_definition_history_and_target_changes_clear_them()
     {
        let conn = fixture();
        let mut def = definition();
        def.env_keys = vec!["TOKEN".into()];
        execute(
            &conn,
            Command::Save {
                expected: 0,
                definition: def.clone(),
            },
        )
        .unwrap();
        assert!(resolved(&conn, None, vec![]).unwrap().is_empty());
        let local = McpCredentials {
            env: BTreeMap::from([("TOKEN".into(), "local-only-synthetic-secret".into())]),
            headers: BTreeMap::new(),
        };
        execute(
            &conn,
            Command::Credentials {
                expected: 1,
                id: def.id.clone(),
                project_id: None,
                credentials: local.clone(),
            },
        )
        .unwrap();
        let snapshot = saved(&conn);
        assert!(snapshot.servers[0].missing_credentials.is_empty());
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("local-only")
        );
        assert!(!format!("{local:?}").contains("local-only"));
        def.transport = McpTransport::Stdio {
            command: "other".into(),
            args: vec![],
        };
        execute(
            &conn,
            Command::Save {
                expected: 2,
                definition: def,
            },
        )
        .unwrap();
        assert_eq!(saved(&conn).servers[0].missing_credentials, ["env/TOKEN"]);
    }
    #[test]
    fn stale_writes_and_native_vendor_options_are_rejected_without_partial_import() {
        let mut conn = fixture();
        execute(
            &conn,
            Command::Save {
                expected: 0,
                definition: definition(),
            },
        )
        .unwrap();
        assert!(
            execute(
                &conn,
                Command::Save {
                    expected: 0,
                    definition: definition()
                }
            )
            .is_err()
        );
        let tx = conn.transaction().unwrap();
        let result=execute(&tx,Command::ImportNative{expected:1,project_id:None,document:r#"{"mcpServers":{"first":{"command":"node"},"second":{"command":"node","vendor_secret":"hidden"}}}"#.into()});
        assert!(result.is_err());
        drop(tx);
        assert_eq!(saved(&conn).servers.len(), 1);
    }
    #[test]
    fn native_import_extracts_credentials_and_preserves_aura_identity_on_repeat() {
        let conn = fixture();
        let document=r#"{"mcpServers":{"remote":{"url":"https://example.test/mcp","headers":{"X-Key":"private-local-value"}}}}"#.to_owned();
        execute(
            &conn,
            Command::ImportNative {
                expected: 0,
                project_id: None,
                document: document.clone(),
            },
        )
        .unwrap();
        let first = saved(&conn);
        assert!(
            !serde_json::to_string(&first)
                .unwrap()
                .contains("private-local")
        );
        let id = first.servers[0].definition.id.clone();
        execute(
            &conn,
            Command::ImportNative {
                expected: 1,
                project_id: None,
                document,
            },
        )
        .unwrap();
        assert_eq!(saved(&conn).servers[0].definition.id, id);
    }
    #[test]
    fn endpoints_and_record_sizes_are_bounded() {
        let mut def = definition();
        for url in [
            "http://localhost.evil.test/mcp",
            "https://user:secret@example.test/mcp",
            "https://example.test/mcp?token=hidden",
            "ftp://example.test/mcp",
        ] {
            def.transport = McpTransport::Http { url: url.into() };
            assert!(validate(&def).is_err(), "{url}");
        }
        def.transport = McpTransport::Stdio {
            command: "node".into(),
            args: vec!["x".repeat(4096); 7],
        };
        assert!(validate(&def).is_err());
    }
    #[test]
    fn deleted_project_definitions_do_not_reappear_in_snapshots_or_resolution() {
        let conn = fixture();
        let mut def = definition();
        let id = uuid::Uuid::now_v7().to_string();
        conn.execute("INSERT INTO projects(id)VALUES(?1)", [&id])
            .unwrap();
        def.project_id = Some(id.clone());
        execute(
            &conn,
            Command::Save {
                expected: 0,
                definition: def,
            },
        )
        .unwrap();
        conn.execute("UPDATE projects SET deleted=1 WHERE id=?1", [&id])
            .unwrap();
        assert!(saved(&conn).servers.is_empty());
        assert!(resolved(&conn, Some(id), vec![]).unwrap().is_empty());
    }
    #[test]
    fn reserved_memory_aliases_are_rejected_by_save_and_native_import() {
        for name in [
            "ghost_project_memory",
            "ghost-project-memory",
            "ghost_browser",
            "ghost-browser",
            "ghost_mobile",
            "ghost-mobile",
            "ghost_game",
            "ghost-game",
            "ghost_project",
            "ghost-project",
        ] {
            let conn = fixture();
            let mut def = definition();
            def.name = name.into();
            assert!(
                execute(
                    &conn,
                    Command::Save {
                        expected: 0,
                        definition: def
                    }
                )
                .is_err()
            );
            assert!(
                execute(
                    &conn,
                    Command::ImportNative {
                        expected: 0,
                        project_id: None,
                        document: format!(r#"{{"mcpServers":{{"{name}":{{"command":"node"}}}}}}"#)
                    }
                )
                .is_err()
            );
        }
    }
    #[test]
    fn stdio_test_selects_only_a_single_local_project_folder() {
        let conn = fixture();
        conn.execute_batch("CREATE TABLE project_folders(id TEXT,project_id TEXT,deleted INTEGER);CREATE TABLE folder_locations(folder_id TEXT,device_id TEXT,path TEXT);INSERT INTO projects VALUES('project',0);INSERT INTO project_folders VALUES('one','project',0);INSERT INTO folder_locations VALUES('one','device','/project/one');").unwrap();
        let command = || Command::TestFolder {
            project_id: "project".into(),
            device_id: "device".into(),
        };
        assert!(
            matches!(execute(&conn,command()).unwrap(),Reply::Folder(path)if path=="/project/one")
        );
        conn.execute_batch("INSERT INTO project_folders VALUES('two','project',0);INSERT INTO folder_locations VALUES('two','device','/project/two');").unwrap();
        assert!(execute(&conn, command()).is_err());
        assert!(
            execute(
                &conn,
                Command::TestFolder {
                    project_id: "project".into(),
                    device_id: "other-device".into()
                }
            )
            .is_err()
        );
    }

    fn for_agent_named(
        conn: &Connection,
        agent: &str,
        names: Vec<String>,
    ) -> Result<Vec<ResolvedMcpServer>, StoreError> {
        let Reply::Resolved(v) = execute(
            conn,
            Command::Resolve {
                project_id: None,
                agent: Some(agent.into()),
                native_names: names,
            },
        )?
        else {
            panic!()
        };
        Ok(v)
    }
    fn names(servers: &[ResolvedMcpServer]) -> Vec<String> {
        servers.iter().map(|s| s.definition.name.clone()).collect()
    }
    #[test]
    fn a_server_for_some_agents_loads_only_in_them_and_one_for_every_agent_in_all() {
        let conn = fixture();
        let every = definition();
        let mut claude_only = definition();
        claude_only.id = "0b7c5b0e-6f7a-4c39-9f4e-2a8d7c1e5b10".into();
        claude_only.name = "browser".into();
        claude_only.agents = Some(vec!["claude".into()]);
        for (expected, def) in [(0, every), (1, claude_only)] {
            execute(
                &conn,
                Command::Save {
                    expected,
                    definition: def,
                },
            )
            .unwrap();
        }
        assert_eq!(
            names(&for_agent_named(&conn, "claude", vec![]).unwrap()),
            ["browser", "docs"]
        );
        // A connection of an agent counts as that agent.
        assert_eq!(
            names(&for_agent_named(&conn, "claude-acp", vec![]).unwrap()),
            ["browser", "docs"]
        );
        // An agent added later gets every-agent servers without anyone editing them.
        assert_eq!(
            names(&for_agent_named(&conn, "codex", vec![]).unwrap()),
            ["docs"]
        );
        assert_eq!(
            names(&for_agent_named(&conn, "hermes", vec![]).unwrap()),
            ["docs"]
        );
        // Ghost's own connection test sees every server in scope.
        assert_eq!(
            names(&resolved(&conn, None, vec![]).unwrap()),
            ["browser", "docs"]
        );
    }
    #[test]
    fn agents_are_saved_as_sorted_agent_ids_and_an_empty_choice_is_refused() {
        let conn = fixture();
        let mut def = definition();
        def.agents = Some(vec!["codex".into(), "claude-acp".into(), "claude".into()]);
        execute(
            &conn,
            Command::Save {
                expected: 0,
                definition: def.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            saved(&conn).servers[0].definition.agents,
            Some(vec!["claude".to_owned(), "codex".to_owned()])
        );
        def.agents = Some(Vec::new());
        assert!(
            execute(
                &conn,
                Command::Save {
                    expected: 1,
                    definition: def.clone()
                }
            )
            .is_err()
        );
        for bad in [
            vec!["Claude".to_owned()],
            vec!["codex".into(), "claude".into()],
            vec!["".into()],
        ] {
            def.agents = Some(bad.clone());
            // Saving normalizes its own lists, so an unsorted one was written by something else.
            assert!(validate(&def).is_err(), "{bad:?}");
        }
        // An agent ID this device does not know yet (a newer device added it) is kept.
        def.agents = Some(vec!["future-agent".into()]);
        assert!(validate(&def).is_ok());
    }
    #[test]
    fn a_definition_for_every_agent_reads_exactly_as_before_agents_existed() {
        let def = definition();
        let text = serde_json::to_string(&def).unwrap();
        assert!(!text.contains("agents"), "{text}");
        let old = r#"{"id":"61a6ce00-b99f-4f8a-9148-8f880f3c735f","name":"docs","project_id":null,"enabled":true,"transport":{"type":"stdio","command":"node","args":["server.js"]},"env_keys":[],"header_keys":[]}"#;
        assert_eq!(serde_json::from_str::<McpDefinition>(old).unwrap(), def);
    }
    #[test]
    fn a_name_only_clashes_with_native_servers_of_the_agents_it_loads_into() {
        let conn = fixture();
        let mut def = definition();
        def.agents = Some(vec!["codex".into()]);
        execute(
            &conn,
            Command::Save {
                expected: 0,
                definition: def,
            },
        )
        .unwrap();
        // Claude Code has its own "docs": the Codex-only shared one does not stop it starting.
        assert!(
            for_agent_named(&conn, "claude", vec!["docs".into()])
                .unwrap()
                .is_empty()
        );
        assert!(for_agent_named(&conn, "codex", vec!["docs".into()]).is_err());
    }
    #[test]
    fn changing_the_agents_keeps_local_authentication() {
        let conn = fixture();
        let mut def = definition();
        def.env_keys = vec!["TOKEN".into()];
        execute(
            &conn,
            Command::Save {
                expected: 0,
                definition: def.clone(),
            },
        )
        .unwrap();
        let value = format!("local-{}", 7);
        execute(
            &conn,
            Command::Credentials {
                expected: 1,
                id: def.id.clone(),
                project_id: None,
                credentials: McpCredentials {
                    env: BTreeMap::from([("TOKEN".to_owned(), value.clone())]),
                    headers: BTreeMap::new(),
                },
            },
        )
        .unwrap();
        def.agents = Some(vec!["claude".into()]);
        execute(
            &conn,
            Command::Save {
                expected: 2,
                definition: def,
            },
        )
        .unwrap();
        let servers = for_agent_named(&conn, "claude", vec![]).unwrap();
        assert_eq!(servers[0].credentials.env.get("TOKEN"), Some(&value));
    }
    #[test]
    fn importing_native_servers_again_keeps_the_agents_chosen_for_them() {
        let conn = fixture();
        let document = r#"{"mcpServers":{"remote":{"url":"https://example.test/mcp"}}}"#.to_owned();
        let import = |expected| Command::ImportNative {
            expected,
            project_id: None,
            document: document.clone(),
        };
        execute(&conn, import(0)).unwrap();
        let mut def = saved(&conn).servers[0].definition.clone();
        def.agents = Some(vec!["pi".into()]);
        execute(
            &conn,
            Command::Save {
                expected: 1,
                definition: def,
            },
        )
        .unwrap();
        execute(&conn, import(2)).unwrap();
        assert_eq!(
            saved(&conn).servers[0].definition.agents,
            Some(vec!["pi".to_owned()])
        );
    }
    #[test]
    fn a_chat_resolves_for_its_own_agent_unless_the_launch_names_another() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO chats(id,project_id,active_agent) VALUES('c1',NULL,'codex')",
            [],
        )
        .unwrap();
        let mut def = definition();
        def.agents = Some(vec!["claude".into()]);
        execute(
            &conn,
            Command::Save {
                expected: 0,
                definition: def,
            },
        )
        .unwrap();
        let chat = |agent: Option<&str>| {
            let Reply::Resolved(v) = execute(
                &conn,
                Command::ResolveChat {
                    chat_id: "c1".into(),
                    agent: agent.map(str::to_owned),
                    native_names: vec![],
                },
            )
            .unwrap() else {
                panic!()
            };
            v.len()
        };
        assert_eq!(chat(None), 0);
        assert_eq!(chat(Some("claude")), 1);
    }
}
