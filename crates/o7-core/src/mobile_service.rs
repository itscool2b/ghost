//! The device tools a native Chat session's agent gets (docs/superpowers/plans/2026-11-21-mobile.md).
//!
//! As with the browser, the app's own executable runs as a stdio MCP server
//! (`--o7-mobile-helper --mcp`) that the agent starts. It reaches the running app over a loopback
//! listener with a random per-session token; the app runs each call on the chat's own device
//! through its [`MobileBridge`]. The grant belongs to one session generation: stopping, failing
//! or replacing the session drops it, and the listener goes with it. The Mobile plugin's switch
//! is read on every call, so turning it off reaches running chats at once.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use o7_protocol::mcp::{McpCredentials, McpDefinition, McpTransport, ResolvedMcpServer};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

/// The MCP server's id and name: reserved, so no user definition can take them.
pub const SERVER_ID: &str = "o7-mobile";
pub const SERVER_NAME: &str = "o7_mobile";

/// The largest request a helper may send (a long text to type) and the largest reply (a
/// screenshot).
const REQUEST_LIMIT: u64 = 1024 * 1024;
const REPLY_LIMIT: u64 = 32 * 1024 * 1024;

/// How long one call may take. Starting a device for the first time takes over a minute, and the
/// app's own wait for it is shorter than this.
const CALL_SECONDS: u64 = 240;

/// What one tool call answers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MobileToolReply {
    pub text: String,
    /// A screenshot: its MIME type and base64 bytes.
    pub image: Option<(String, String)>,
    pub error: bool,
}

pub type MobileFuture = Pin<Box<dyn Future<Output = MobileToolReply> + Send>>;

/// The app's devices, as sessions see them: runs `tool` with `arguments` on `chat_id`'s device.
pub trait MobileBridge: Send + Sync {
    fn call(&self, chat_id: String, tool: String, arguments: serde_json::Value) -> MobileFuture;
}

pub(crate) struct MobileGrant {
    endpoint: String,
    token: String,
    abort: AbortHandle,
}

impl std::fmt::Debug for MobileGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobileGrant").finish_non_exhaustive()
    }
}

impl Drop for MobileGrant {
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

/// What an agent reads when the person turned Mobile off.
pub(crate) const TURNED_OFF: &str = "Mobile is turned off in o7 (Settings > Plugins & MCP > Universal plugins > Mobile). Ask the person to turn it on, or work without a device.";

impl MobileGrant {
    /// Listens for the session's helper. `enabled` is the Mobile plugin's switch.
    pub(crate) fn start(
        bridge: Arc<dyn MobileBridge>,
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
            // One call at a time, as the helper sends them: two actions never race on one device.
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
                    None => error_reply("Invalid device request."),
                    Some(request) if !same_token(&credential, &request.token) => {
                        error_reply("Device access expired. Restart the o7 session.")
                    }
                    Some(_) if !enabled.load(Ordering::Acquire) => error_reply(TURNED_OFF),
                    Some(request) => {
                        let call = bridge.call(chat_id.clone(), request.tool, request.arguments);
                        let limit = std::time::Duration::from_secs(CALL_SECONDS);
                        match tokio::time::timeout(limit, call).await {
                            Ok(reply) => reply,
                            Err(_) => error_reply("The device did not answer in four minutes."),
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
                    args: vec!["--o7-mobile-helper".into(), "--mcp".into()],
                },
                env_keys: vec!["O7_MOBILE_ENDPOINT".into(), "O7_MOBILE_TOKEN".into()],
                header_keys: vec![],
                agents: None,
            },
            revision: 1,
            credentials: McpCredentials {
                env: BTreeMap::from([
                    ("O7_MOBILE_ENDPOINT".into(), self.endpoint.clone()),
                    ("O7_MOBILE_TOKEN".into(), self.token.clone()),
                ]),
                headers: BTreeMap::new(),
            },
        })
    }
}

fn error_reply(text: &str) -> MobileToolReply {
    MobileToolReply {
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
pub use helper::{run_helper, tool_names};

#[cfg(test)]
mod tests;
