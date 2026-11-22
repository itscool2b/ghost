use std::io::{BufRead, Write};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::*;

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(String, String, serde_json::Value)>>,
}

impl MobileBridge for Recorder {
    fn call(&self, chat_id: String, tool: String, arguments: serde_json::Value) -> MobileFuture {
        self.calls
            .lock()
            .unwrap()
            .push((chat_id.clone(), tool.clone(), arguments));
        Box::pin(async move {
            MobileToolReply {
                text: format!("{tool} in {chat_id}"),
                image: (tool == "mobile_screenshot").then(|| ("image/png".into(), "QUJD".into())),
                error: false,
            }
        })
    }
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

fn credentials(grant: &MobileGrant) -> (String, String) {
    let server = grant.mcp().expect("a helper definition");
    (
        server.credentials.env["O7_MOBILE_ENDPOINT"].clone(),
        server.credentials.env["O7_MOBILE_TOKEN"].clone(),
    )
}

#[tokio::test]
async fn a_call_reaches_the_chats_device_with_its_screenshot() {
    let bridge = Arc::new(Recorder::default());
    let enabled = Arc::new(AtomicBool::new(true));
    let grant = MobileGrant::start(
        bridge.clone(),
        "chat-1".into(),
        enabled,
        &tokio::runtime::Handle::current(),
    )
    .unwrap();
    let (endpoint, token) = credentials(&grant);
    let reply = ask(
        endpoint.clone(),
        serde_json::json!({ "token": token, "tool": "mobile_screenshot", "arguments": { "ref": "d3" } })
            .to_string(),
    )
    .await;
    assert_eq!(reply["text"], "mobile_screenshot in chat-1");
    assert_eq!(reply["error"], false);
    assert_eq!(reply["image"]["mime"], "image/png");
    let calls = bridge.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].0, "chat-1",
        "the grant's own chat, never one the helper names"
    );
    assert_eq!(calls[0].2["ref"], "d3");
}

#[tokio::test]
async fn a_wrong_token_and_junk_are_refused() {
    let bridge = Arc::new(Recorder::default());
    let grant = MobileGrant::start(
        bridge.clone(),
        "chat-1".into(),
        Arc::new(AtomicBool::new(true)),
        &tokio::runtime::Handle::current(),
    )
    .unwrap();
    let (endpoint, token) = credentials(&grant);
    let wrong = format!("{}x", &token[..token.len() - 1]);
    let reply = ask(
        endpoint.clone(),
        serde_json::json!({ "token": wrong, "tool": "mobile_snapshot" }).to_string(),
    )
    .await;
    assert_eq!(reply["error"], true);
    assert!(reply["text"].as_str().unwrap().contains("expired"));
    let reply = ask(endpoint.clone(), "not json".into()).await;
    assert_eq!(reply["error"], true);
    let reply = ask(
        endpoint,
        serde_json::json!({ "token": token, "tool": "mobile_snapshot", "chat_id": "other" })
            .to_string(),
    )
    .await;
    assert_eq!(
        reply["error"], true,
        "unknown fields (a chat of its own) are refused"
    );
    assert!(bridge.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn turning_mobile_off_reaches_a_running_grant() {
    let bridge = Arc::new(Recorder::default());
    let enabled = Arc::new(AtomicBool::new(true));
    let grant = MobileGrant::start(
        bridge.clone(),
        "chat-1".into(),
        enabled.clone(),
        &tokio::runtime::Handle::current(),
    )
    .unwrap();
    let (endpoint, token) = credentials(&grant);
    enabled.store(false, std::sync::atomic::Ordering::Release);
    let reply = ask(
        endpoint,
        serde_json::json!({ "token": token, "tool": "mobile_snapshot" }).to_string(),
    )
    .await;
    assert_eq!(reply["error"], true);
    assert_eq!(reply["text"], TURNED_OFF);
    assert!(bridge.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn dropping_the_grant_closes_its_listener() {
    let grant = MobileGrant::start(
        Arc::new(Recorder::default()),
        "chat-1".into(),
        Arc::new(AtomicBool::new(true)),
        &tokio::runtime::Handle::current(),
    )
    .unwrap();
    let (endpoint, _) = credentials(&grant);
    drop(grant);
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
    let grant = MobileGrant::start(
        Arc::new(Recorder::default()),
        "chat-1".into(),
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
            assert_eq!(args, &["--o7-mobile-helper", "--mcp"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        server.definition.env_keys,
        ["O7_MOBILE_ENDPOINT", "O7_MOBILE_TOKEN"]
    );
    assert!(server.credentials.env["O7_MOBILE_ENDPOINT"].starts_with("127.0.0.1:"));
}
