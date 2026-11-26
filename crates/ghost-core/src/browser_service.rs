//! The browser tools a native Chat session's agent gets (docs/superpowers/plans/2026-11-09-browser.md).
//!
//! Like the project-memory helper, the app's own executable runs as a stdio MCP server
//! (`--ghost-browser-helper --mcp`) that the agent starts. It reaches the running app over a
//! loopback listener with a random per-session token; the app runs each call in the chat's own
//! browser through its [`BrowserBridge`]. The grant belongs to one session generation: stopping,
//! failing or replacing the session drops it, and the listener goes with it. The Browser plugin's
//! switch is read on every call, so turning it off reaches running chats at once.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ghost_protocol::mcp::{McpCredentials, McpDefinition, McpTransport, ResolvedMcpServer};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

/// The MCP server's id and name: reserved, so no user definition can take them.
pub const SERVER_ID: &str = "ghost-browser";
pub const SERVER_NAME: &str = "ghost_browser";

/// The largest request a helper may send (a long text to type) and the largest reply (a full
/// page's screenshot).
const REQUEST_LIMIT: u64 = 1024 * 1024;
const REPLY_LIMIT: u64 = 32 * 1024 * 1024;

/// What one tool call answers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BrowserToolReply {
    pub text: String,
    /// A screenshot: its MIME type and base64 bytes.
    pub image: Option<(String, String)>,
    pub error: bool,
}

pub type BrowserFuture = Pin<Box<dyn Future<Output = BrowserToolReply> + Send>>;

/// The app's browser, as sessions see it: runs `tool` with `arguments` in `chat_id`'s browser.
pub trait BrowserBridge: Send + Sync {
    fn call(&self, chat_id: String, tool: String, arguments: serde_json::Value) -> BrowserFuture;
}

pub(crate) struct BrowserGrant {
    endpoint: String,
    token: String,
    abort: AbortHandle,
}

impl std::fmt::Debug for BrowserGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserGrant").finish_non_exhaustive()
    }
}

impl Drop for BrowserGrant {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    token: String,
    tool: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

/// What an agent reads when the person turned the browser off.
pub(crate) const TURNED_OFF: &str = "The browser is turned off in Ghost (Settings > Plugins & MCP > Universal plugins > Browser use). Ask the person to turn it on, or work without it.";

impl BrowserGrant {
    /// Listens for the session's helper. `enabled` is the Browser plugin's switch.
    pub(crate) fn start(
        bridge: Arc<dyn BrowserBridge>,
        chat_id: String,
        enabled: Arc<AtomicBool>,
        rt: &tokio::runtime::Handle,
    ) -> std::io::Result<Self> {
        let socket = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        socket.set_nonblocking(true)?;
        let endpoint = socket.local_addr()?.to_string();
        let token = format!("{}{}", crate::ids::new_id(), crate::ids::new_id());
        let credential = token.clone();
        let task = rt.spawn(async move {
            let Ok(listener) = TcpListener::from_std(socket) else {
                return;
            };
            // One call at a time, as the helper sends them: two actions never race in one page.
            while let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::AsyncReadExt;
                let request = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    let mut input = Vec::new();
                    let mut reader = BufReader::new((&mut stream).take(REQUEST_LIMIT));
                    reader.read_until(b'\n', &mut input).await.ok()?;
                    if input.last() != Some(&b'\n') {
                        return None;
                    }
                    serde_json::from_slice::<Request>(&input).ok()
                })
                .await
                .ok()
                .flatten();
                let reply = match request {
                    None => error_reply("Invalid browser request."),
                    Some(request) if !same_token(&credential, &request.token) => {
                        error_reply("Browser access expired. Restart the Ghost session.")
                    }
                    Some(_) if !enabled.load(Ordering::Acquire) => error_reply(TURNED_OFF),
                    Some(request) => {
                        let call = bridge.call(chat_id.clone(), request.tool, request.arguments);
                        match tokio::time::timeout(std::time::Duration::from_secs(120), call).await
                        {
                            Ok(reply) => reply,
                            Err(_) => error_reply("The browser did not answer in two minutes."),
                        }
                    }
                };
                let mut body = serde_json::json!({ "text": reply.text, "error": reply.error });
                if let Some((mime, data)) = reply.image {
                    body["image"] = serde_json::json!({ "mime": mime, "data": data });
                }
                if let Ok(mut bytes) = serde_json::to_vec(&body) {
                    bytes.push(b'\n');
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        stream.write_all(&bytes),
                    )
                    .await;
                }
            }
        });
        Ok(Self {
            endpoint,
            token,
            abort: task.abort_handle(),
        })
    }

    /// The MCP server the session's agent starts: the app's own executable as the helper
    /// ([`crate::paths::helper_program`]).
    pub(crate) fn mcp(&self) -> Option<ResolvedMcpServer> {
        let exe = crate::paths::helper_program()?;
        Some(ResolvedMcpServer {
            definition: McpDefinition {
                id: SERVER_ID.into(),
                name: SERVER_NAME.into(),
                project_id: None,
                enabled: true,
                transport: McpTransport::Stdio {
                    command: exe.to_string_lossy().into_owned(),
                    args: vec!["--ghost-browser-helper".into(), "--mcp".into()],
                },
                env_keys: vec![
                    "GHOST_BROWSER_ENDPOINT".into(),
                    "GHOST_BROWSER_TOKEN".into(),
                ],
                header_keys: vec![],
                agents: None,
            },
            revision: 1,
            credentials: McpCredentials {
                env: BTreeMap::from([
                    ("GHOST_BROWSER_ENDPOINT".into(), self.endpoint.clone()),
                    ("GHOST_BROWSER_TOKEN".into(), self.token.clone()),
                ]),
                headers: BTreeMap::new(),
            },
        })
    }
}

fn error_reply(text: &str) -> BrowserToolReply {
    BrowserToolReply {
        text: text.into(),
        image: None,
        error: true,
    }
}

fn same_token(a: &str, b: &str) -> bool {
    let mut diff = a.len() ^ b.len();
    for (i, x) in a.bytes().enumerate() {
        diff |= usize::from(x ^ b.as_bytes().get(i).copied().unwrap_or(0));
    }
    diff == 0
}

mod helper;
pub use helper::run_helper;

#[cfg(test)]
mod tests;
