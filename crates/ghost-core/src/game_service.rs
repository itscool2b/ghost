//! The game tools a native Chat session's agent gets (docs/game.md).
//!
//! As with the browser and the device, the app's own executable runs as a stdio MCP server
//! (`--ghost-game-helper --mcp`) that the agent starts. It reaches the running app over a loopback
//! listener with a random per-session token; the app runs each call on the chat's own game through
//! its [`GameBridge`]. The grant belongs to one session generation: stopping, failing or replacing
//! the session drops it, and the listener goes with it. Game's switch is read on every call, so
//! turning it off reaches running chats at once.
//!
//! Unlike the browser's and the device's, this grant is not for every chat: a session gets it only
//! when its folder holds a game or the person turned Game on for the chat ([`GameBridge::wanted`]),
//! so other chats carry none of its tools. The folder a call acts on is the one the session was
//! started in, kept here: no tool names a chat or a folder.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ghost_protocol::mcp::{McpCredentials, McpDefinition, McpTransport, ResolvedMcpServer};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

/// The MCP server's id and name: reserved, so no user definition can take them.
pub const SERVER_ID: &str = "ghost-game";
pub const SERVER_NAME: &str = "ghost_game";

/// The largest request a helper may send (code to run in the game) and the largest reply (an
/// action's filmstrip and its last frame).
const REQUEST_LIMIT: u64 = 1024 * 1024;
const REPLY_LIMIT: u64 = 32 * 1024 * 1024;

/// How long one call may take. A project's first start imports its assets, and an action may play
/// two minutes of game time; the app's own waits for both are shorter than this.
const CALL_SECONDS: u64 = 300;

/// What one tool call answers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GameToolReply {
    pub text: String,
    /// Pictures of the game, in order: each one's MIME type and base64 bytes.
    pub images: Vec<(String, String)>,
    pub error: bool,
}

pub type GameFuture = Pin<Box<dyn Future<Output = GameToolReply> + Send>>;
pub type WantedFuture = Pin<Box<dyn Future<Output = bool> + Send>>;

/// The app's games, as sessions see them.
pub trait GameBridge: Send + Sync {
    /// Whether a session of `chat_id` starting in `folder` gets the game tools: the folder holds
    /// a game, or the person turned Game on for the chat.
    fn wanted(&self, chat_id: String, folder: PathBuf) -> WantedFuture;
    /// Runs `tool` with `arguments` on `chat_id`'s game, which lives in `folder`.
    fn call(
        &self,
        chat_id: String,
        folder: PathBuf,
        tool: String,
        arguments: serde_json::Value,
    ) -> GameFuture;
    /// A session of `chat_id` was given the tools, or let go of them, for the pane to say.
    fn granted(&self, chat_id: &str, granted: bool);
}

pub(crate) struct GameGrant {
    endpoint: String,
    token: String,
    abort: AbortHandle,
    bridge: Arc<dyn GameBridge>,
    chat_id: String,
}

impl std::fmt::Debug for GameGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GameGrant").finish_non_exhaustive()
    }
}

impl Drop for GameGrant {
    fn drop(&mut self) {
        self.abort.abort();
        self.bridge.granted(&self.chat_id, false);
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

/// What an agent reads when the person turned Game off.
pub(crate) const TURNED_OFF: &str = "Game is turned off in Ghost (Settings > Plugins & MCP > Universal plugins > Game). Ask the person to turn it on, or work without it.";

impl GameGrant {
    /// Listens for the session's helper. `folder` is where the session runs: the only folder its
    /// calls reach. `enabled` is Game's switch.
    pub(crate) fn start(
        bridge: Arc<dyn GameBridge>,
        chat_id: String,
        folder: PathBuf,
        enabled: Arc<AtomicBool>,
        rt: &tokio::runtime::Handle,
    ) -> std::io::Result<Self> {
        let socket = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        socket.set_nonblocking(true)?;
        let endpoint = socket.local_addr()?.to_string();
        let token = format!("{}{}", crate::ids::new_id(), crate::ids::new_id());
        let credential = token.clone();
        let (calls, chat) = (Arc::clone(&bridge), chat_id.clone());
        let task = rt.spawn(async move {
            let Ok(listener) = TcpListener::from_std(socket) else {
                return;
            };
            // One call at a time, as the helper sends them: two actions never race on one game.
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
                    None => error_reply("Invalid game request."),
                    Some(request) if !same_token(&credential, &request.token) => {
                        error_reply("Game access expired. Restart the Ghost session.")
                    }
                    Some(_) if !enabled.load(Ordering::Acquire) => error_reply(TURNED_OFF),
                    Some(request) => {
                        let call = calls.call(
                            chat.clone(),
                            folder.clone(),
                            request.tool,
                            request.arguments,
                        );
                        let limit = std::time::Duration::from_secs(CALL_SECONDS);
                        match tokio::time::timeout(limit, call).await {
                            Ok(reply) => reply,
                            Err(_) => error_reply("The game did not answer in five minutes."),
                        }
                    }
                };
                let images: Vec<serde_json::Value> = reply
                    .images
                    .iter()
                    .map(|(mime, data)| serde_json::json!({ "mime": mime, "data": data }))
                    .collect();
                let body = serde_json::json!({
                    "text": reply.text,
                    "error": reply.error,
                    "images": images,
                });
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
        bridge.granted(&chat_id, true);
        Ok(Self {
            endpoint,
            token,
            abort: task.abort_handle(),
            bridge,
            chat_id,
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
                    args: vec!["--ghost-game-helper".into(), "--mcp".into()],
                },
                env_keys: vec![
                    "GHOST_GAME_ENDPOINT".into(),
                    "GHOST_GAME_HELPER_TOKEN".into(),
                ],
                header_keys: vec![],
                agents: None,
            },
            revision: 1,
            credentials: McpCredentials {
                env: BTreeMap::from([
                    ("GHOST_GAME_ENDPOINT".into(), self.endpoint.clone()),
                    ("GHOST_GAME_HELPER_TOKEN".into(), self.token.clone()),
                ]),
                headers: BTreeMap::new(),
            },
        })
    }
}

fn error_reply(text: &str) -> GameToolReply {
    GameToolReply {
        text: text.into(),
        images: Vec::new(),
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
