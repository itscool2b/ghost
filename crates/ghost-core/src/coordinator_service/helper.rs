//! `ghost --ghost-project-helper --mcp`: the stdio MCP server an agent starts for its project's
//! tools. It holds nothing and decides nothing: every request goes to the running app, which
//! knows the chat from the grant's token. It never opens a profile database.
use serde_json::{Value, json};
use std::io::{self, BufRead, Read, Write};

const UNAVAILABLE: &str = "Ghost's project tools are unavailable: the session's grant expired or the app closed. Tell the person, and go on without them if you can.";

fn request(mut value: Value) -> Value {
    let result = (|| -> Result<Value, Box<dyn std::error::Error>> {
        let endpoint = std::env::var("GHOST_PROJECT_ENDPOINT")?.parse::<std::net::SocketAddr>()?;
        if !endpoint.ip().is_loopback() {
            return Err("The project service must be local to the Ghost app.".into());
        }
        value["token"] = Value::String(std::env::var("GHOST_PROJECT_TOKEN")?);
        let mut stream =
            std::net::TcpStream::connect_timeout(&endpoint, std::time::Duration::from_secs(2))?;
        // Longer than the app allows one call, so the app's own answer arrives first.
        stream.set_read_timeout(Some(std::time::Duration::from_secs(150)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        stream.write_all(&bytes)?;
        let mut line = String::new();
        io::BufReader::new(stream.take(super::REPLY_LIMIT)).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    })();
    result.unwrap_or_else(|_| json!({ "text": UNAVAILABLE, "error": true }))
}

pub fn run_helper() {
    if !std::env::args().any(|value| value == "--mcp") {
        eprintln!(
            "Usage: ghost --ghost-project-helper --mcp (an MCP server for a project's tools)"
        );
        return;
    }
    // Asked once: a chat's part does not change while its session runs, and the app checks it on
    // every call whatever this list says.
    let mut described: Option<Value> = None;
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.len() > 512 * 1024 {
            break;
        }
        let Ok(input) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = input.get("id").cloned() else {
            continue;
        };
        let result = match input["method"].as_str() {
            Some("initialize") => {
                let about = described.get_or_insert_with(|| request(json!({"op": "describe"})));
                let mut result = json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "aura-project", "version": "1.0.0" },
                });
                if let Some(instructions) = about["instructions"].as_str() {
                    result["instructions"] = json!(instructions);
                }
                result
            }
            Some("ping") => json!({}),
            Some("tools/list") => {
                let about = described.get_or_insert_with(|| request(json!({"op": "describe"})));
                json!({ "tools": about.get("tools").cloned().unwrap_or_else(|| json!([])) })
            }
            Some("tools/call") => {
                let name = input["params"]["name"].as_str().unwrap_or("");
                let mut arguments = input["params"]["arguments"].clone();
                if !arguments.is_object() {
                    arguments = json!({});
                }
                let reply = request(json!({"op": "call", "tool": name, "arguments": arguments}));
                json!({
                    "content": [{ "type": "text", "text": reply["text"].as_str().unwrap_or(UNAVAILABLE) }],
                    "isError": reply["error"].as_bool().unwrap_or(true),
                })
            }
            _ => {
                println!(
                    "{}",
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
                );
                let _ = io::stdout().flush();
                continue;
            }
        };
        println!("{}", json!({"jsonrpc":"2.0","id":id,"result":result}));
        let _ = io::stdout().flush();
    }
}
