//! Native Pi 1.x RPC adapter.
mod metadata;
mod session;
#[cfg(test)]
mod tests;
pub mod translate;
mod ui;
pub mod wire;

use crate::driver::{AgentDriver, DriverContext, DriverExit};
use futures::{FutureExt, future::BoxFuture};
#[derive(Debug, Default)]
pub struct PiDriver;
impl PiDriver {
    pub fn new() -> Self {
        Self
    }
}
impl AgentDriver for PiDriver {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        session::run(ctx).boxed()
    }
}

/// Aura's session-only MCP extension for Pi: it reads the servers from `GHOST_SHARED_MCP` (and
/// removes the variable), so no credential is written to a file or a command line.
pub(crate) const SHARED_MCP_EXTENSION: &[u8] = include_bytes!("shared-mcp.ts");

/// A server Pi's extension can register: stdio or HTTP, named with letters, numbers, underscores
/// and hyphens.
pub(crate) fn shared_mcp_supported(server: &ghost_protocol::mcp::ResolvedMcpServer) -> bool {
    !matches!(
        server.definition.transport,
        ghost_protocol::mcp::McpTransport::Sse { .. }
    ) && !server.definition.name.contains('.')
}

/// The servers as `GHOST_SHARED_MCP` carries them to the extension.
pub(crate) fn shared_mcp_json(servers: &[ghost_protocol::mcp::ResolvedMcpServer]) -> String {
    let servers = crate::mcp::codex(servers);
    let mut servers = servers.as_object().cloned().unwrap_or_default();
    for server in servers.values_mut() {
        if let Some(headers) = server
            .as_object_mut()
            .and_then(|s| s.remove("http_headers"))
        {
            server["headers"] = headers;
        }
    }
    serde_json::Value::Object(servers).to_string()
}
