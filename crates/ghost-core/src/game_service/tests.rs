use std::io::{BufRead, Write};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::*;

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(String, PathBuf, String, serde_json::Value)>>,
    /// Every "given" (true) and "let go" (false), in order.
    grants: Mutex<Vec<(String, bool)>>,
}

impl GameBridge for Recorder {
    fn wanted(&self, _chat_id: String, _folder: PathBuf) -> WantedFuture {
        Box::pin(async { true })
    }

    fn call(
        &self,
        chat_id: String,
        folder: PathBuf,
        tool: String,
        arguments: serde_json::Value,
    ) -> GameFuture {
        self.calls
            .lock()
            .unwrap()
            .push((chat_id.clone(), folder, tool.clone(), arguments));
        Box::pin(async move {
            GameToolReply {
                text: format!("{tool} in {chat_id}"),
                images: if tool == "game_act" {
                    vec![
                        ("image/jpeg".into(), "c3RyaXA=".into()),
                        ("image/jpeg".into(), "ZW5k".into()),
                    ]
                } else {
                    Vec::new()
                },
                error: false,
            }
        })
    }

    fn granted(&self, chat_id: &str, granted: bool) {
        self.grants
            .lock()
            .unwrap()
            .push((chat_id.to_owned(), granted));
    }
}

fn grant(bridge: Arc<Recorder>, enabled: Arc<AtomicBool>) -> GameGrant {
    GameGrant::start(
        bridge,
        "chat-1".into(),
        PathBuf::from("/games/moon hop"),
        enabled,
        &tokio::runtime::Handle::current(),
    )
    .unwrap()
}

/// Sends one line to the grant's listener and reads its answer, as the helper does.
async fn ask(endpoint: String, line: String) -> serde_json::Value {
    tokio::task::spawn_blocking(move || {
        let mut stream = std::net::TcpStream::connect(endpoint).unwrap();
        stream.write_all(line.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
        let mut reply = String::new();
        std::io::BufReader::new(stream)
            .read_line(&mut reply)
            .unwrap();
        serde_json::from_str(&reply).unwrap()
    })
    .await
    .unwrap()
}

fn credentials(grant: &GameGrant) -> (String, String) {
    let server = grant.mcp().expect("a helper definition");
    (
        server.credentials.env["GHOST_GAME_ENDPOINT"].clone(),
        server.credentials.env["GHOST_GAME_HELPER_TOKEN"].clone(),
    )
}

#[tokio::test]
async fn a_call_reaches_the_chats_own_game_in_the_sessions_own_folder() {
    let bridge = Arc::new(Recorder::default());
    let grant = grant(bridge.clone(), Arc::new(AtomicBool::new(true)));
    let (endpoint, token) = credentials(&grant);
    let reply = ask(
        endpoint.clone(),
        serde_json::json!({
            "token": token,
            "tool": "game_act",
            "arguments": { "steps": [{ "press": "ui_right", "ms": 500 }] },
        })
        .to_string(),
    )
    .await;
    assert_eq!(reply["text"], "game_act in chat-1");
    assert_eq!(reply["error"], false);
    let images = reply["images"].as_array().unwrap();
    assert_eq!(
        images.len(),
        2,
        "the filmstrip and the last frame, in order"
    );
    assert_eq!(images[0]["mime"], "image/jpeg");
    assert_eq!(images[0]["data"], "c3RyaXA=");
    assert_eq!(images[1]["data"], "ZW5k");
    let calls = bridge.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].0, "chat-1",
        "the grant's own chat, never one the helper names"
    );
    assert_eq!(
        calls[0].1,
        PathBuf::from("/games/moon hop"),
        "the folder the session started in, never one the helper names"
    );
    assert_eq!(calls[0].3["steps"][0]["press"], "ui_right");
}

#[tokio::test]
async fn a_wrong_token_and_junk_are_refused() {
    let bridge = Arc::new(Recorder::default());
    let grant = grant(bridge.clone(), Arc::new(AtomicBool::new(true)));
    let (endpoint, token) = credentials(&grant);
    let wrong = format!("{}x", &token[..token.len() - 1]);
    let reply = ask(
        endpoint.clone(),
        serde_json::json!({ "token": wrong, "tool": "game_look" }).to_string(),
    )
    .await;
    assert_eq!(reply["error"], true);
    assert!(reply["text"].as_str().unwrap().contains("expired"));
    let reply = ask(endpoint.clone(), "not json".into()).await;
    assert_eq!(reply["error"], true);
    for extra in ["chat_id", "folder"] {
        let mut request = serde_json::json!({ "token": token, "tool": "game_look" });
        request[extra] = "other".into();
        let reply = ask(endpoint.clone(), request.to_string()).await;
        assert_eq!(
            reply["error"], true,
            "unknown fields (a chat or a folder of its own) are refused"
        );
    }
    assert!(bridge.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn turning_game_off_reaches_a_running_grant() {
    let bridge = Arc::new(Recorder::default());
    let enabled = Arc::new(AtomicBool::new(true));
    let grant = grant(bridge.clone(), enabled.clone());
    let (endpoint, token) = credentials(&grant);
    enabled.store(false, std::sync::atomic::Ordering::Release);
    let reply = ask(
        endpoint,
        serde_json::json!({ "token": token, "tool": "game_run" }).to_string(),
    )
    .await;
    assert_eq!(reply["error"], true);
    assert_eq!(reply["text"], TURNED_OFF);
    assert!(bridge.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn dropping_the_grant_closes_its_listener_and_says_the_tools_are_gone() {
    let bridge = Arc::new(Recorder::default());
    let grant = grant(bridge.clone(), Arc::new(AtomicBool::new(true)));
    assert_eq!(
        *bridge.grants.lock().unwrap(),
        [("chat-1".to_owned(), true)]
    );
    let (endpoint, _) = credentials(&grant);
    drop(grant);
    assert_eq!(
        *bridge.grants.lock().unwrap(),
        [("chat-1".to_owned(), true), ("chat-1".to_owned(), false)]
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let refused = tokio::task::spawn_blocking(move || {
        std::net::TcpStream::connect_timeout(
            &endpoint.parse().unwrap(),
            std::time::Duration::from_millis(500),
        )
        .is_err()
    })
    .await
    .unwrap();
    assert!(refused, "nothing listens once the session's grant is gone");
}

#[test]
fn the_helper_is_this_executable_with_reserved_names() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let grant = GameGrant::start(
        Arc::new(Recorder::default()),
        "chat-1".into(),
        PathBuf::from("/games/moon hop"),
        Arc::new(AtomicBool::new(true)),
        runtime.handle(),
    )
    .unwrap();
    let server = grant.mcp().unwrap();
    assert_eq!(server.definition.id, SERVER_ID);
    assert_eq!(server.definition.name, SERVER_NAME);
    match &server.definition.transport {
        McpTransport::Stdio { command, args } => {
            assert_eq!(command, &std::env::current_exe().unwrap().to_string_lossy());
            assert_eq!(args, &["--ghost-game-helper", "--mcp"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        server.definition.env_keys,
        ["GHOST_GAME_ENDPOINT", "GHOST_GAME_HELPER_TOKEN"]
    );
    assert!(server.credentials.env["GHOST_GAME_ENDPOINT"].starts_with("127.0.0.1:"));
    // The helper's token is not the one a running game shows Ghost: the names differ, so
    // neither can be taken for the other.
    assert!(!server.credentials.env.contains_key("GHOST_GAME_TOKEN"));
}
