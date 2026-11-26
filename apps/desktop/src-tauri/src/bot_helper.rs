//! `ghost --ghost-bot-helper`: Ghost's bot tools as an MCP server on standard input and output, for an
//! assistant that runs on this computer too (Hermes, OpenClaw). It reaches the running Ghost through
//! its door on loopback (`ghost_remote`, `door.json` in Ghost's data folder), never through the relay,
//! so a bot's calls stay on this computer.
//!
//! The assistant starts it with the bot link's address in `GHOST_LINK`, as Settings shows it: the
//! link's token is what Ghost checks, and the link's level decides the tools it lists and what each
//! call may do, exactly as through the relay. The tools are the relay's own list
//! (`apps/relay/src/tools.json`), filtered as the relay filters them.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// The tools the relay lists, with its marks (`full`, `since`).
const TOOLS: &str = include_str!("../../../relay/src/tools.json");

/// What a bot is told first, as the relay tells a link of the newest set and a read-only link.
const INSTRUCTIONS: &str = "Ghost runs coding agents (Claude Code, Codex and others) on a person's computer, on the person's own subscriptions; these tools reach its chats while Ghost is open there. For new work, call task once: Ghost starts an agent on its own branch, has it plan, build, check and commit, and has a second agent review it, then tells you when it is all done if this link has a call back. Then end your turn; do not wait or poll. Work that can wait goes to task with when tonight. send continues a chat, and like wait returns the turn's result with the commands that ran and what still fails. When you come back, inbox says what needs you in one call. Where look is listed, it shows a chat's page or emulator as a picture, when seeing the work matters. Agents edit files and run commands, so hand over only what the person asked for.";
const READ_INSTRUCTIONS: &str = "Ghost runs coding agents (Claude Code, Codex and others) on a person's computer; this link reads its chats while Ghost is open there, and cannot send, answer or change anything. inbox says what needs the person in one call; read and wait return what a chat did.";

/// The longest line read from the assistant, as the relay takes a request.
const MAX_LINE: usize = 256 * 1024;
/// How long a call may take: a wait lasts up to 200 seconds.
const CALL_TIMEOUT: Duration = Duration::from_secs(260);

/// What the door answered about a link.
struct Link {
    full: bool,
    read: bool,
    version: u32,
}

pub fn run() {
    let link = std::env::var("GHOST_LINK").unwrap_or_default();
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let write = {
        let out = Arc::clone(&out);
        move |message: Value| {
            let mut out = out
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = writeln!(out, "{message}");
            let _ = out.flush();
        }
    };
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    loop {
        let mut line = String::new();
        match (&mut reader).take(MAX_LINE as u64).read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        // A notification gets no answer.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                let read = rights(&link).is_ok_and(|link| link.read);
                write(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": params
                            .get("protocolVersion")
                            .and_then(Value::as_str)
                            .unwrap_or("2025-06-18"),
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": "ghost", "title": "Ghost ADE", "version": "2.0.0" },
                        "instructions": if read { READ_INSTRUCTIONS } else { INSTRUCTIONS },
                    }
                }));
            }
            Some("ping") => write(json!({ "jsonrpc": "2.0", "id": id, "result": {} })),
            Some("tools/list") => {
                let tools = match rights(&link) {
                    Ok(link) => listed(link.full, link.read, link.version),
                    Err(_) => Vec::new(),
                };
                write(json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools } }));
            }
            Some("tools/call") => {
                // A call may wait minutes; others are answered meanwhile.
                let (link, write) = (link.clone(), write.clone());
                std::thread::spawn(move || {
                    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                    let args = params.get("arguments").cloned().unwrap_or(json!({}));
                    write(json!({ "jsonrpc": "2.0", "id": id, "result": call(&link, name, args) }));
                });
            }
            _ => write(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "Method not found" }
            })),
        }
    }
}

/// One tool call, as MCP answers it: the text, and a picture when the call returned one.
fn call(link: &str, name: &str, args: Value) -> Value {
    let reply = door(json!({ "link": link, "op": "call", "tool": name, "args": args }));
    let reply = match reply {
        Ok(reply) => reply,
        Err(text) => {
            return json!({ "content": [{ "type": "text", "text": text }], "isError": true });
        }
    };
    let text = reply
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut content = vec![json!({ "type": "text", "text": text })];
    if let Some(image) = reply.get("image").filter(|image| image.is_object()) {
        content.push(json!({
            "type": "image",
            "data": image.get("data").cloned().unwrap_or_default(),
            "mimeType": image.get("mime").cloned().unwrap_or_default(),
        }));
    }
    json!({
        "content": content,
        "isError": reply.get("error").and_then(Value::as_bool).unwrap_or(true),
    })
}

fn rights(link: &str) -> Result<Link, String> {
    let reply = door(json!({ "link": link, "op": "rights" }))?;
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(reply
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("Ghost refused this link.")
            .to_owned());
    }
    Ok(Link {
        full: reply.get("full").and_then(Value::as_bool).unwrap_or(false),
        read: reply.get("read").and_then(Value::as_bool).unwrap_or(false),
        version: reply
            .get("v")
            .and_then(Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
            .unwrap_or(2),
    })
}

/// One request to the running Ghost's door, and its answer.
fn door(request: Value) -> Result<Value, String> {
    let away = || {
        "Ghost is not running on this computer, or its remote access is off. Open Ghost and turn remote access on in Settings, then try again.".to_owned()
    };
    if request["link"].as_str().is_none_or(str::is_empty) {
        return Err(
            "Set GHOST_LINK to this bot link's address, from Ghost: Settings, Remote access."
                .into(),
        );
    }
    let root = ghost_core::paths::DataRoot::resolve().map_err(|_| away())?;
    let file = std::fs::read(root.root().join("door.json")).map_err(|_| away())?;
    let door = serde_json::from_slice::<Value>(&file).map_err(|_| away())?;
    let port = door
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(away)?;
    let key = door
        .get("key")
        .and_then(Value::as_str)
        .and_then(ghost_remote::crypto::decode::<32>)
        .ok_or_else(away)?;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream =
        TcpStream::connect_timeout(&address, Duration::from_secs(2)).map_err(|_| away())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|_| away())?
            .take(4 * 1024 * 1024),
    );
    // The door proves it is this run's Ghost before the token is sent: whatever else might hold the
    // port when Ghost is not running cannot.
    let challenge = ghost_remote::crypto::random::<16>().map_err(|_| away())?;
    let hello = json!({ "op": "hello", "challenge": ghost_remote::crypto::encode(&challenge) });
    stream
        .write_all(format!("{hello}\n").as_bytes())
        .map_err(|_| away())?;
    let mut proof = String::new();
    reader.read_line(&mut proof).map_err(|_| away())?;
    let proven = serde_json::from_str::<Value>(&proof)
        .ok()
        .and_then(|proof| {
            proof
                .get("proof")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|proof| ghost_remote::crypto::door_proven(&key, &challenge, &proof));
    if !proven {
        return Err("Something other than Ghost answered at its door; nothing was sent. Open Ghost and try again.".into());
    }
    let mut bytes = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_LINE {
        return Err("This call is larger than Ghost takes.".into());
    }
    bytes.push(b'\n');
    let _ = stream.set_read_timeout(Some(CALL_TIMEOUT));
    stream.write_all(&bytes).map_err(|_| away())?;
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|_| "Ghost did not answer in time.".to_owned())?;
    serde_json::from_str(&line).map_err(|_| "Ghost did not answer in time.".to_owned())
}

/// The relay's tool list as a link of this level and set is shown it: a tool or an argument
/// marked `full` only for full control, one marked `since` only from that set on, only the tools
/// that read for a read-only link, and none of the marks.
fn listed(full: bool, read: bool, version: u32) -> Vec<Value> {
    let tools: Vec<Value> = serde_json::from_str(TOOLS).unwrap_or_default();
    let since = |value: &Value| {
        value
            .get("since")
            .and_then(Value::as_u64)
            .map_or(2, |since| u32::try_from(since).unwrap_or(u32::MAX))
    };
    let marked = |value: &Value| value.get("full").and_then(Value::as_bool) == Some(true);
    tools
        .into_iter()
        .filter(|tool| (full || !marked(tool)) && since(tool) <= version)
        .filter(|tool| !read || tool["annotations"]["readOnlyHint"].as_bool() == Some(true))
        .map(|mut tool| {
            if let Some(object) = tool.as_object_mut() {
                object.remove("full");
                object.remove("since");
            }
            if let Some(properties) = tool["inputSchema"]["properties"].as_object_mut() {
                properties.retain(|_, argument| {
                    (full || !marked(argument)) && since(argument) <= version
                });
                for argument in properties.values_mut() {
                    if let Some(argument) = argument.as_object_mut() {
                        argument.remove("full");
                        argument.remove("since");
                    }
                }
            }
            tool
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(tools: &[Value]) -> Vec<&str> {
        tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect()
    }

    /// The helper lists what the relay lists to a link of the same level and set.
    #[test]
    fn a_link_is_listed_the_tools_its_level_and_set_have() {
        let chats = listed(false, false, 6);
        assert_eq!(
            names(&chats),
            [
                "inbox", "chats", "projects", "read", "task", "send", "wait", "answer", "stop"
            ]
        );
        let full = listed(true, false, 6);
        assert!(names(&full).contains(&"look") && names(&full).contains(&"set"));
        assert_eq!(
            names(&listed(false, true, 6)),
            ["inbox", "chats", "projects", "read", "wait"]
        );
        assert!(
            !names(&listed(true, false, 4)).contains(&"look"),
            "look is set 5"
        );
        let task = |tools: &[Value]| {
            tools
                .iter()
                .find(|tool| tool["name"] == "task")
                .and_then(|tool| tool["inputSchema"]["properties"].as_object())
                .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        assert!(task(&chats).contains(&"when".to_owned()));
        assert!(
            !task(&listed(false, false, 5)).contains(&"when".to_owned()),
            "when is set 6"
        );
        let send = full
            .iter()
            .find(|tool| tool["name"] == "send")
            .expect("send");
        assert!(send["inputSchema"]["properties"].get("access").is_some());
        let send = chats
            .iter()
            .find(|tool| tool["name"] == "send")
            .expect("send");
        assert!(
            send["inputSchema"]["properties"].get("access").is_none(),
            "full only"
        );
        for tool in full.iter().chain(&chats) {
            assert!(tool.get("full").is_none() && tool.get("since").is_none());
        }
    }

    /// What a bot is told first is what the relay tells it, word for word.
    #[test]
    fn the_instructions_are_the_relays() {
        let relay = include_str!("../../../relay/src/index.ts");
        assert!(
            relay.contains(INSTRUCTIONS),
            "the newest set's instructions changed"
        );
        assert!(
            relay.contains(READ_INSTRUCTIONS),
            "the read-only instructions changed"
        );
    }
}
