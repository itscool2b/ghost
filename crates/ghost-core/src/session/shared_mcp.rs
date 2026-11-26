use super::{SessionError, SessionManager};
use ghost_protocol::mcp::*;

/// The shared MCP servers a launch takes, whether Aura's memory tools can join them, and a notice
/// when shared servers were left out. Shared MCP applies to compatible agents: one whose own MCP
/// configuration Ghost cannot read (Antigravity, oh-my-pi), a restricted fallback connection
/// and an agent on another host start without shared servers, and their chat says so. A
/// compatible agent whose configuration cannot be read still requires review, since its own
/// servers might clash with shared ones.
pub(super) async fn prepare_launch(
    context: &super::workspace_folders::LaunchFolders,
    env: &ghost_process::ResolvedEnv,
    spec: &ghost_agents::AgentSpec,
    host: &ghost_protocol::agents::AgentHost,
) -> Result<(Vec<ResolvedMcpServer>, bool, Option<String>), String> {
    let roots = super::config_library::roots(
        &env.vars,
        Some((
            context.chat.folder_id.as_deref().unwrap_or("active"),
            &context.chat.folder,
        )),
    );
    let provider = ghost_agents::agent_of(spec.id).to_owned();
    let compatible = matches!(
        provider.as_str(),
        "claude" | "codex" | "pi" | "opencode" | "cursor" | "grok" | "hermes" | "mock"
    );
    let variables = env.vars.clone();
    let folder = context.chat.folder.clone();
    let (names, checked) = tokio::task::spawn_blocking(move || {
        let provider_match = |p: ghost_config::Provider| match p {
            ghost_config::Provider::Claude => provider == "claude",
            ghost_config::Provider::Codex => provider == "codex",
            ghost_config::Provider::Pi => provider == "pi",
            _ => false,
        };
        let mut checked = compatible;
        let mut names = Vec::new();
        let mut selected = Vec::new();
        let mut native_roots = false;
        for source in roots
            .sources
            .into_iter()
            .filter(|s| provider_match(s.provider))
        {
            native_roots = true;
            match std::fs::symlink_metadata(&source.root) {
                Ok(meta) if meta.is_dir() => selected.push(source),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => checked = false,
            }
        }
        if matches!(provider.as_str(), "claude" | "codex" | "pi") && !native_roots {
            checked = false;
        }
        if !selected.is_empty() {
            let inventory = ghost_config::Library::new(selected)
                .map_err(|e| e.to_string())?
                .inspect();
            checked &=
                inventory.notices.is_empty() && inventory.sources.iter().all(|s| s.available);
            names.extend(
                inventory
                    .items
                    .into_iter()
                    .filter(|item| item.item.kind == ghost_config::Kind::McpServer)
                    .map(|item| item.item.key),
            );
        }
        if matches!(provider.as_str(), "opencode" | "cursor" | "grok" | "hermes") {
            let variable = |name: &str| {
                variables
                    .get(&ghost_process::EnvKey::from(name))
                    .map(std::path::PathBuf::from)
                    .filter(|p| p.is_absolute())
            };
            let home = variable(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .or_else(|| variable("HOME"));
            let user = match provider.as_str() {
                "opencode" => variable("OPENCODE_CONFIG_DIR")
                    .or_else(|| super::plugin_library::opencode_root(&variables)),
                "hermes" => {
                    variable("HERMES_HOME").or_else(|| home.as_ref().map(|p| p.join(".hermes")))
                }
                "cursor" => home.as_ref().map(|p| p.join(".cursor")),
                _ => home.as_ref().map(|p| p.join(".grok")),
            };
            if user.is_none() {
                checked = false;
            }
            let mut paths = user.into_iter().collect::<Vec<_>>();
            if provider == "opencode" {
                paths.push(folder.clone());
            } else if provider == "cursor" {
                paths.push(folder.join(".cursor"));
            } else if provider == "grok" {
                paths.extend(folder.ancestors().take(64).map(|p| p.join(".grok")));
            }
            for root in paths {
                match std::fs::symlink_metadata(&root) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Ok(meta) if meta.is_dir() => {}
                    _ => {
                        checked = false;
                        continue;
                    }
                }
                let inventory = if provider == "opencode" {
                    ghost_config::extensions::inspect_opencode(&root)
                } else {
                    ghost_config::extensions::inspect_extra(&provider, &root)
                };
                checked &= inventory.notices.is_empty();
                names.extend(inventory.servers.into_iter().map(|s| s.name));
            }
        }
        Ok::<_, String>((names, checked))
    })
    .await
    .map_err(|_| "Native MCP inventory could not be inspected.".to_owned())??;
    // Ghost's own tools (project memory, the browser, the device) carry reserved names, so they join every
    // agent that takes MCP servers, also one whose own configuration Ghost cannot read: an
    // agent added later gets them with no step (owner, 10 November 2026: "if they add a new agent
    // ... those skills should transfer over"). A listed agent whose configuration could not be
    // read whole still waits for review, and a server of the same name keeps them out.
    //
    // The tradeoff, for an agent whose configuration cannot be read at all: a server of a reserved
    // name in that agent's own configuration is not seen here, and could shadow Ghost's tool in
    // the agent's list. It gains nothing Ghost holds: the browser and project memory answer
    // only Ghost's own helper, which carries a token made for this session and passed in its
    // environment, never written to any configuration. A shadowing server can do what any server
    // in the agent's configuration already can, and no more.
    let memory_available = (checked || !compatible)
        && !names.iter().any(|name| {
            matches!(
                name.replace('-', "_").as_str(),
                "ghost_project_memory" | "ghost_browser" | "ghost_mobile" | "ghost_project"
            )
        });
    match context
        .store
        .shared_mcp(crate::mcp::Command::ResolveChat {
            chat_id: context.chat.id.clone(),
            // The agent this launch starts, which may be a connection of the chat's agent.
            agent: Some(ghost_agents::agent_of(spec.id).to_owned()),
            native_names: names,
        })
        .await
        .map_err(|e| e.to_string())?
    {
        crate::mcp::Reply::Resolved(servers) => {
            let left_out = |reason: &str| {
                let names = servers
                    .iter()
                    .map(|server| server.definition.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "Shared MCP servers were not loaded in this chat ({names}): {reason}. It runs with {}'s own MCP servers.",
                    spec.name
                ))
            };
            if servers.is_empty() {
                return Ok((servers, memory_available, None));
            }
            let native = *host == ghost_protocol::agents::AgentHost::Native;
            let headless = matches!(spec.kind, ghost_protocol::DriverKind::Headless);
            if let Some(reason) = left_out_because(native, headless, compatible) {
                return Ok((Vec::new(), memory_available, left_out(reason)));
            }
            if !checked {
                return Err("Native MCP configuration could not be fully inspected. Review its diagnostics before loading shared definitions; no native configuration was overwritten.".into());
            }
            Ok((servers, memory_available, None))
        }
        _ => Err("Unexpected shared MCP resolution response.".into()),
    }
}

/// Why a launch takes no shared MCP servers at all, or `None` when it can take them.
fn left_out_because(native: bool, headless: bool, compatible: bool) -> Option<&'static str> {
    if !native {
        Some("shared servers run only for agents on this device")
    } else if headless {
        Some("this restricted connection runs without MCP servers")
    } else if !compatible {
        Some(
            "Ghost cannot read this agent's own MCP configuration to rule out a server of the same name",
        )
    } else {
        None
    }
}

#[cfg(test)]
mod launch_tests {
    use super::left_out_because;

    #[test]
    fn only_compatible_native_full_connections_take_shared_servers() {
        assert_eq!(left_out_because(true, false, true), None);
        // Antigravity and oh-my-pi start without them instead of failing to start.
        assert!(
            left_out_because(true, false, false)
                .unwrap()
                .contains("cannot read")
        );
        assert!(
            left_out_because(true, true, true)
                .unwrap()
                .contains("restricted")
        );
        assert!(
            left_out_because(false, false, true)
                .unwrap()
                .contains("this device")
        );
    }
}

impl SessionManager {
    pub async fn mcp_session_action(
        &self,
        chat_id: &str,
        server_name: String,
        action: McpSessionAction,
    ) -> Result<McpActionResult, SessionError> {
        tokio::time::timeout(
            super::OPTION_TIMEOUT,
            self.ask(chat_id, |reply| super::actor::ChatMsg::McpAction {
                server_name: server_name.clone(),
                action,
                reply,
            }),
        )
        .await
        .unwrap_or(Err(SessionError::SessionNotReady))
    }
    pub async fn test_shared_mcp(
        &self,
        expected: u32,
        id: String,
        project_id: Option<String>,
        chat_id: Option<String>,
    ) -> Result<McpTestResult, SessionError> {
        let _owner = self
            .shared()
            .live
            .try_maintenance()
            .ok_or(SessionError::ShuttingDown)?;
        let snapshot = self.shared_mcp(crate::mcp::Command::Snapshot).await?;
        if snapshot.revision != expected {
            return Err(SessionError::Organization(
                "Shared MCP changed. Refresh before testing.".into(),
            ));
        }
        let servers = self
            .resolve_shared_mcp(project_id.clone(), Vec::new())
            .await?;
        let server = servers
            .into_iter()
            .find(|s| s.definition.id == id)
            .ok_or_else(|| {
                SessionError::Organization(
                    "Enable this server and supply its required local credentials before testing."
                        .into(),
                )
            })?;
        let test_folder = if matches!(server.definition.transport, McpTransport::Stdio { .. }) {
            if let Some(chat_id) = chat_id {
                let (chat, _, _) = self
                    .ask(&chat_id, |reply| super::actor::ChatMsg::FolderContext {
                        reply,
                    })
                    .await?;
                if chat.agent_host != ghost_protocol::agents::AgentHost::Native {
                    return Err(SessionError::Organization(
                        "Select a native chat before testing a stdio server.".into(),
                    ));
                }
                if !crate::mcp::for_agent(&server.definition, &chat.agent_id) {
                    return Err(SessionError::Organization(
                        "This server is not enabled for the selected chat's agent. Add the agent to the server's agents, or test it from a chat with one of its agents.".into(),
                    ));
                }
                let effective = self
                    .deps()
                    .store
                    .shared_mcp(crate::mcp::Command::ResolveChat {
                        chat_id,
                        agent: None,
                        native_names: Vec::new(),
                    })
                    .await
                    .map_err(|e| SessionError::Organization(e.to_string()))?;
                if !matches!(effective,crate::mcp::Reply::Resolved(ref servers) if servers.iter().any(|s|s.definition==server.definition))
                {
                    return Err(SessionError::Organization("This definition is not effective in the selected chat's project. Select a chat in the matching scope.".into()));
                }
                Some(chat.folder)
            } else if let Some(project_id) = project_id {
                match self
                    .deps()
                    .store
                    .shared_mcp(crate::mcp::Command::TestFolder {
                        project_id,
                        device_id: self.deps().device_id.clone(),
                    })
                    .await
                    .map_err(|e| SessionError::Organization(e.to_string()))?
                {
                    crate::mcp::Reply::Folder(folder) => Some(std::path::PathBuf::from(folder)),
                    _ => {
                        return Err(SessionError::Internal(
                            "Unexpected MCP test-folder response.".into(),
                        ));
                    }
                }
            } else {
                return Err(SessionError::Organization("Select an active native chat before testing an all-projects stdio server. Its working directory must be explicit.".into()));
            }
        } else {
            None
        };
        let initialize = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"ghost","version":self.deps().app_version}}});
        let work = async {
            match &server.definition.transport {
                McpTransport::Stdio{command,args}=>{
                    use ghost_process::{LaunchSpec,ProcessCwd,SpawnSpec,ProtocolLog,StdoutItem};
                    let env=self.deps().env.get().await;
                    let folder=test_folder.clone().filter(|p|p.is_absolute()&&p.is_dir()).ok_or_else(||"The selected project folder is unavailable.".to_owned())?;
                    let program=test_program(command,&folder,&env.vars).ok_or_else(||"The MCP executable is unavailable in the selected project folder or this device's PATH.".to_owned())?;
                    let launch=LaunchSpec{label:"Shared MCP connection test".into(),program,args:args.iter().map(Into::into).collect(),cwd:ProcessCwd::ChatFolder,set_env:server.credentials.env.iter().map(|(k,v)|(k.into(),v.into())).collect(),daemons:Vec::new(),startup_timeout:std::time::Duration::from_secs(5)};
                    let mut process=self.deps().spawner.spawn(SpawnSpec{launch,folder,env,max_line_bytes:64*1024},ProtocolLog::disabled_with_id(crate::ids::new_id())).await.map_err(|_|"The MCP test process could not start.".to_owned())?;
                    process.io.stdin.send(initialize.to_string()).map_err(|_|"The MCP server closed its input.".to_owned())?;
                    let result=loop {
                        let Some(item)=process.io.stdout.next().await else{break Err("The MCP server exited before initialization.".to_owned());};
                        let StdoutItem::Line(line)=item else{break Err("The MCP server exceeded the response size limit.".to_owned());};
                        let value:serde_json::Value=serde_json::from_str(&line).map_err(|_|"The MCP server sent an invalid JSON response.".to_owned())?;
                        if value["id"]==1 {break inspect_test_response(&value);}
                    };
                    process.io.stdin.close();
                    process.control.kill_tree();
                    result
                },
                McpTransport::Http{url}=>{
                    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(10)).build().map_err(|_|"The HTTP client is unavailable.".to_owned())?;
                    let mut request=client.post(url).header("Content-Type","application/json").header("Accept","application/json, text/event-stream").body(initialize.to_string());
                    for(name,value)in &server.credentials.headers{request=request.header(name,value);}
                    let mut response=request.send().await.map_err(|_|"The MCP endpoint could not be reached.".to_owned())?;
                    if response.status().as_u16()==401||response.status().as_u16()==403{return Ok(McpTestResult{state:"auth_required".into(),message:"The endpoint requires local authentication. Supply a header credential or use the agent's native OAuth flow.".into()});}
                    if !response.status().is_success(){return Err(format!("The MCP endpoint returned HTTP {}.",response.status().as_u16()));}
                    let mut bytes=Vec::new();
                    while let Some(chunk)=response.chunk().await.map_err(|_|"The MCP response could not be read.".to_owned())? {
                        if bytes.len()+chunk.len()>64*1024{return Err("The MCP response exceeded 64 KB.".into());} bytes.extend_from_slice(&chunk);
                        if let Ok(value)=serde_json::from_slice::<serde_json::Value>(&bytes){return inspect_test_response(&value);}
                        if let Ok(text)=std::str::from_utf8(&bytes){for line in text.lines(){if let Some(data)=line.strip_prefix("data: ") && let Ok(value)=serde_json::from_str::<serde_json::Value>(data) && value["id"]==1{return inspect_test_response(&value);}}}
                    }
                    Err("The endpoint did not return an MCP initialization result.".into())
                },
                McpTransport::Sse{..}=>Ok(McpTestResult{state:"unsupported".into(),message:"Direct testing of legacy SSE requires the agent's native MCP panel. Use a compatible session to inspect this server.".into()}),
            }
        };
        match tokio::time::timeout(std::time::Duration::from_secs(12),work).await {Ok(Ok(result))=>Ok(result),Ok(Err(message))=>Ok(McpTestResult{state:"failed".into(),message}),Err(_)=>Ok(McpTestResult{state:"failed".into(),message:"The MCP initialization test timed out after 12 seconds. Its owned process was stopped.".into()})}
    }
    pub async fn shared_mcp(
        &self,
        command: crate::mcp::Command,
    ) -> Result<McpSnapshot, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        match self
            .deps()
            .store
            .shared_mcp(command)
            .await
            .map_err(|e| SessionError::Organization(e.to_string()))?
        {
            crate::mcp::Reply::Snapshot(snapshot) => Ok(snapshot),
            _ => Err(SessionError::Internal(
                "Unexpected MCP storage response.".into(),
            )),
        }
    }
    pub async fn resolve_shared_mcp(
        &self,
        project_id: Option<String>,
        native_names: Vec<String>,
    ) -> Result<Vec<ResolvedMcpServer>, SessionError> {
        match self
            .deps()
            .store
            .shared_mcp(crate::mcp::Command::Resolve {
                project_id,
                // Ghost's own test connects to the server itself, for whichever agent.
                agent: None,
                native_names,
            })
            .await
            .map_err(|e| SessionError::Organization(e.to_string()))?
        {
            crate::mcp::Reply::Resolved(servers) => {
                self.deps().redactor.set_secrets(
                    servers
                        .iter()
                        .flat_map(|s| {
                            s.credentials
                                .env
                                .values()
                                .chain(s.credentials.headers.values())
                        })
                        .cloned()
                        .collect(),
                );
                Ok(servers)
            }
            _ => Err(SessionError::Internal(
                "Unexpected MCP resolution response.".into(),
            )),
        }
    }
}
fn inspect_test_response(value: &serde_json::Value) -> Result<McpTestResult, String> {
    if value["jsonrpc"] != "2.0" || value["id"] != 1 {
        return Err(
            "The endpoint did not return the matching MCP initialization acknowledgement.".into(),
        );
    }
    if value["error"].is_object() {
        return Err(
            "The MCP server rejected initialization. Inspect its native diagnostics for details."
                .into(),
        );
    }
    if value["result"]["protocolVersion"].as_str().is_none()
        || !value["result"]["capabilities"].is_object()
    {
        return Err("The endpoint returned an invalid MCP initialization result.".into());
    }
    Ok(McpTestResult{state:"connected".into(),message:"MCP initialization succeeded. This direct test does not establish an agent session connection; restart a compatible idle chat to apply its current definition.".into()})
}
fn test_program(
    command: &str,
    folder: &std::path::Path,
    env: &ghost_process::EnvMap,
) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(command);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    if command.contains(['/', '\\']) {
        let path = folder.join(command);
        return path.is_file().then_some(path);
    }
    ghost_process::find_program(command, env)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_test_executable_is_resolved_from_selected_project() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::write(bin.join("mcp"), b"synthetic fixture").unwrap();
        assert_eq!(
            test_program("./bin/mcp", temp.path(), &ghost_process::EnvMap::new()),
            Some(temp.path().join("./bin/mcp"))
        );
        assert!(test_program("./missing", temp.path(), &ghost_process::EnvMap::new()).is_none());
    }
    #[test]
    fn direct_test_requires_matching_jsonrpc_initialize_acknowledgement() {
        let valid = serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{}}});
        assert!(inspect_test_response(&valid).is_ok());
        for (mut value, field, replacement) in [
            (valid.clone(), "id", serde_json::json!(2)),
            (valid.clone(), "jsonrpc", serde_json::json!("1.0")),
            (valid.clone(), "id", serde_json::Value::Null),
        ] {
            value[field] = replacement;
            assert!(inspect_test_response(&value).is_err());
        }
    }
}
