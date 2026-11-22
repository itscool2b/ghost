//! Native wire projections for an immutable shared-MCP launch snapshot.
use o7_protocol::mcp::{McpTransport, ResolvedMcpServer};
use serde_json::{Value, json};
pub fn managed_memory(server: &ResolvedMcpServer) -> bool {
    server.definition.id == "o7-project-memory"
        && server.definition.name.replace('-', "_") == "o7_project_memory"
}
/// o7's browser server (the Browser universal plugin's tools).
pub fn managed_browser(server: &ResolvedMcpServer) -> bool {
    server.definition.id == "o7-browser" && server.definition.name.replace('-', "_") == "o7_browser"
}
/// o7's mobile server (the Mobile universal plugin's tools, on the chat's device).
pub fn managed_mobile(server: &ResolvedMcpServer) -> bool {
    server.definition.id == "o7-mobile" && server.definition.name.replace('-', "_") == "o7_mobile"
}
/// o7's project server (a coordinated project's tools, for its coordinator and workers).
pub fn managed_project(server: &ResolvedMcpServer) -> bool {
    server.definition.id == "aura-project"
        && server.definition.name.replace('-', "_") == "o7_project"
}
/// One of o7's own optional servers: left out, not failed, where a connection cannot load it.
pub fn managed(server: &ResolvedMcpServer) -> bool {
    managed_memory(server)
        || managed_browser(server)
        || managed_mobile(server)
        || managed_project(server)
}
pub fn acp_session(
    servers: &[ResolvedMcpServer],
    stdio: bool,
    http: bool,
    sse: bool,
) -> Result<(Value, bool), &'static str> {
    let supported = |server: &ResolvedMcpServer| match server.definition.transport {
        McpTransport::Stdio { .. } => stdio,
        McpTransport::Http { .. } => http,
        McpTransport::Sse { .. } => sse,
    };
    let omitted = servers.iter().any(|s| managed(s) && !supported(s));
    let selected = servers
        .iter()
        .filter(|s| !managed(s) || supported(s))
        .cloned()
        .collect::<Vec<_>>();
    if selected.iter().any(|s| !supported(s)) {
        return Err(
            "This agent did not advertise the configured MCP transport. Disable that shared server or choose a compatible transport before restarting.",
        );
    }
    Ok((acp(&selected, http, sse)?, omitted))
}
pub fn unsupported(
    reply: tokio::sync::oneshot::Sender<
        Result<o7_protocol::mcp::McpActionResult, crate::driver::DriverError>,
    >,
    message: &str,
) {
    let _ = reply.send(Ok(o7_protocol::mcp::McpActionResult {
        state: "unsupported".into(),
        message: message.into(),
        authorization_url: None,
        servers: Vec::new(),
    }));
}
pub fn response(
    value: &Value,
    action: o7_protocol::mcp::McpSessionAction,
    server_name: &str,
    codex: bool,
) -> Result<o7_protocol::mcp::McpActionResult, crate::driver::DriverError> {
    use o7_protocol::mcp::*;
    let error = |message: &str| crate::driver::DriverError {
        kind: o7_protocol::ErrorKind::ProtocolError,
        message: message.into(),
        detail: None,
    };
    if action == McpSessionAction::Login {
        let url=value["authorizationUrl"].as_str().or_else(||value["url"].as_str()).filter(|url|url.starts_with("https://") || url.starts_with("http://localhost:") || url.starts_with("http://127.0.0.1:")).ok_or_else(||error("The agent did not return a supported authorization URL. Use its native MCP authentication flow."))?;
        return Ok(McpActionResult{state:"authorization_required".into(),message:"Open the authorization link and complete sign-in. Refresh session status afterwards; authentication stays local to the agent.".into(),authorization_url:Some(url.into()),servers:Vec::new()});
    }
    let rows = value[if codex { "data" } else { "mcpServers" }]
        .as_array()
        .ok_or_else(|| error("The agent returned an invalid MCP status response."))?;
    let servers = rows
        .iter()
        .filter(|row| row["name"] == server_name)
        .map(|row| McpRuntimeServer {
            name: server_name.into(),
            state: row[if codex { "runtimeStatus" } else { "status" }]
                .as_str()
                .unwrap_or("unknown")
                .into(),
            authentication: row["authStatus"].as_str().unwrap_or("agent_managed").into(),
            tool_count: if codex {
                row["tools"].as_object().map_or(0, |t| t.len())
            } else {
                row["tools"].as_array().map_or(0, |t| t.len())
            }
            .min(u32::MAX as usize) as u32,
        })
        .collect();
    Ok(McpActionResult{state:"observed".into(),message:"Status reported by this running agent session. Saved definition edits require an idle restart.".into(),authorization_url:None,servers})
}

pub fn codex(servers: &[ResolvedMcpServer]) -> Value {
    let mut result = serde_json::Map::new();
    for server in servers {
        let mut value = match &server.definition.transport {
            McpTransport::Stdio { command, args } => {
                json!({"command":command,"args":args,"env":server.credentials.env})
            }
            McpTransport::Http { url } => {
                json!({"url":url,"http_headers":server.credentials.headers})
            }
            McpTransport::Sse { .. } => continue,
        };
        value["enabled"] = json!(true);
        result.insert(server.definition.name.clone(), value);
    }
    Value::Object(result)
}
pub fn codex_session(servers: &[ResolvedMcpServer]) -> Value {
    let mut result = codex(servers);
    for (i, server) in servers.iter().enumerate() {
        if !server.credentials.headers.is_empty() {
            let value = &mut result[&server.definition.name];
            if let Some(object) = value.as_object_mut() {
                object.remove("http_headers");
            }
            value["env_http_headers"] = json!(
                server
                    .credentials
                    .headers
                    .keys()
                    .enumerate()
                    .map(|(j, key)| (key.clone(), format!("O7_CODEX_MCP_{i}_{j}")))
                    .collect::<std::collections::BTreeMap<_, _>>()
            );
        }
    }
    result
}
pub fn codex_environment(servers: &[ResolvedMcpServer], launch: &mut o7_process::LaunchSpec) {
    for (i, server) in servers.iter().enumerate() {
        for (j, value) in server.credentials.headers.values().enumerate() {
            launch
                .set_env
                .push((format!("O7_CODEX_MCP_{i}_{j}").into(), value.into()));
        }
    }
}
/// Environment expansion avoids credential values in argv and the spawn log. Native Claude
/// configurations remain loaded because we deliberately do not set --strict-mcp-config.
pub fn claude(servers: &[ResolvedMcpServer], launch: &mut o7_process::LaunchSpec) {
    let mut result = serde_json::Map::new();
    for (i, server) in servers.iter().enumerate() {
        let mut value = match &server.definition.transport {
            McpTransport::Stdio { command, args } => {
                json!({"type":"stdio","command":command,"args":args})
            }
            McpTransport::Http { url } => json!({"type":"http","url":url}),
            McpTransport::Sse { url } => json!({"type":"sse","url":url}),
        };
        for (group, values) in [
            ("env", &server.credentials.env),
            ("headers", &server.credentials.headers),
        ] {
            if values.is_empty() {
                continue;
            }
            let mut refs = serde_json::Map::new();
            for (j, (key, value)) in values.iter().enumerate() {
                let name = format!("O7_MCP_{i}_{group}_{j}");
                launch.set_env.push((name.clone().into(), value.into()));
                refs.insert(key.clone(), json!(format!("${{{name}}}")));
            }
            value[group] = Value::Object(refs);
        }
        result.insert(server.definition.name.clone(), value);
    }
    if !result.is_empty() {
        launch.args.extend([
            "--mcp-config".into(),
            json!({"mcpServers":result}).to_string().into(),
        ]);
    }
}
pub fn acp(servers: &[ResolvedMcpServer], http: bool, sse: bool) -> Result<Value, &'static str> {
    let mut result = Vec::new();
    for server in servers {
        let pairs = |values: &std::collections::BTreeMap<String, String>| {
            values
                .iter()
                .map(|(name, value)| json!({"name":name,"value":value}))
                .collect::<Vec<_>>()
        };
        result.push(match &server.definition.transport {
            McpTransport::Stdio{command,args}=>json!({"name":server.definition.name,"command":command,"args":args,"env":pairs(&server.credentials.env)}),
            McpTransport::Http{url} if http=>json!({"type":"http","name":server.definition.name,"url":url,"headers":pairs(&server.credentials.headers)}),
            McpTransport::Sse{url} if sse=>json!({"type":"sse","name":server.definition.name,"url":url,"headers":pairs(&server.credentials.headers)}),
            _=>return Err("This agent did not advertise the configured MCP transport. Disable that shared server or choose a compatible transport before restarting."),
        });
    }
    Ok(json!(result))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use o7_protocol::mcp::*;
    use std::collections::BTreeMap;
    pub(crate) fn server() -> ResolvedMcpServer {
        ResolvedMcpServer {
            definition: McpDefinition {
                id: "a071f5e0-2902-4283-afc4-15110ad75553".into(),
                name: "docs".into(),
                project_id: None,
                enabled: true,
                transport: McpTransport::Http {
                    url: "https://example.test/mcp".into(),
                },
                env_keys: vec![],
                header_keys: vec!["X-Key".into()],
                agents: None,
            },
            revision: 3,
            credentials: McpCredentials {
                env: BTreeMap::new(),
                headers: BTreeMap::from([("X-Key".into(), "short".into())]),
            },
        }
    }
    pub(crate) fn memory() -> ResolvedMcpServer {
        let mut server = server();
        server.definition.id = "o7-project-memory".into();
        server.definition.name = "o7_project_memory".into();
        server.definition.transport = McpTransport::Stdio {
            command: "node".into(),
            args: vec![],
        };
        server.definition.header_keys.clear();
        server.credentials.headers.clear();
        server
    }
    pub(crate) fn browser() -> ResolvedMcpServer {
        let mut server = memory();
        server.definition.id = "o7-browser".into();
        server.definition.name = "o7_browser".into();
        server
    }
    /// The mobile server is one of o7's own too: left out where it cannot load, and a user's
    /// server that only borrows the name is not it.
    #[test]
    fn the_mobile_server_is_optional_like_the_browsers() {
        let mut mobile = memory();
        mobile.definition.id = "o7-mobile".into();
        mobile.definition.name = "o7_mobile".into();
        assert!(managed(&mobile) && managed_mobile(&mobile));
        assert!(!managed_browser(&mobile) && !managed_memory(&mobile));
        let mut lookalike = server();
        lookalike.definition.name = "o7_mobile".into();
        assert!(!managed(&lookalike), "the id must be o7's too");
        let (servers, omitted) = acp_session(&[mobile], false, false, false).unwrap();
        assert!(omitted);
        assert_eq!(servers, json!([]));
    }
    /// o7's browser server is one of its own optional servers, like project memory: a
    /// connection that cannot load it leaves it out instead of failing the start. A user's
    /// server only borrowing the name is not.
    #[test]
    fn the_browser_server_is_optional_like_memory() {
        assert!(managed(&browser()) && managed_browser(&browser()));
        assert!(!managed_memory(&browser()));
        let mut lookalike = server();
        lookalike.definition.name = "o7_browser".into();
        assert!(!managed(&lookalike), "the id must be o7's too");
        let (servers, omitted) = acp_session(&[browser(), memory()], false, false, false).unwrap();
        assert!(omitted);
        assert_eq!(servers, json!([]));
    }
    #[test]
    fn optional_memory_is_omitted_when_transport_unsupported_but_real_shared_servers_refuse() {
        let (servers, omitted) = acp_session(&[memory()], false, false, false).unwrap();
        assert_eq!(servers, json!([]));
        assert!(omitted);
        assert!(acp_session(&[server()], false, false, false).is_err());
    }
    #[test]
    fn codex_uses_launch_env_header_references_without_serializing_values() {
        let server = server();
        let config = codex_session(&[server]);
        assert!(!config.to_string().contains("short"));
        assert_eq!(
            config["docs"]["env_http_headers"]["X-Key"],
            "O7_CODEX_MCP_0_0"
        );
    }
    #[test]
    fn claude_descriptor_preserves_native_configs_and_keeps_credentials_out_of_argv() {
        let server = server();
        let mut launch = o7_process::LaunchSpec {
            label: "claude".into(),
            program: "claude".into(),
            args: Vec::new(),
            cwd: o7_process::ProcessCwd::ChatFolder,
            set_env: Vec::new(),
            daemons: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(5),
        };
        claude(&[server], &mut launch);
        let argv = format!("{:?}", launch.args);
        assert!(!argv.contains("short"));
        assert!(!argv.contains("strict-mcp"));
        assert!(argv.contains("mcp-config"));
        assert!(launch.set_env.iter().any(|(_, v)| v == "short"));
    }
    #[test]
    fn acp_remote_transports_require_advertised_support() {
        let mut server = server();
        assert!(acp(&[server.clone()], false, false).is_err());
        assert_eq!(
            acp(&[server.clone()], true, false).unwrap()[0]["type"],
            "http"
        );
        server.definition.transport = McpTransport::Sse {
            url: "https://example.test/sse".into(),
        };
        assert!(acp(&[server.clone()], true, false).is_err());
        assert_eq!(acp(&[server], false, true).unwrap()[0]["type"], "sse");
    }
    #[test]
    fn status_projection_exposes_only_session_state_and_tool_count() {
        let value = json!({"data":[{"name":"docs","runtimeStatus":"connected","authStatus":"oauth","tools":{"lookup":{"description":"private response"}},"httpOrigin":"private-origin","toolsError":"private error"}]});
        let result = response(&value, McpSessionAction::Status, "docs", true).unwrap();
        assert_eq!(result.servers[0].tool_count, 1);
        assert!(!serde_json::to_string(&result).unwrap().contains("private"));
    }
}
