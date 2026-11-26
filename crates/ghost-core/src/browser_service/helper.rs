//! `ghost --ghost-browser-helper --mcp`: the stdio MCP server an agent starts for Ghost's browser.
//! It never opens a page or a profile itself; each call goes to the running app over the session's
//! loopback grant (`GHOST_BROWSER_ENDPOINT`, `GHOST_BROWSER_TOKEN`).

use std::io::{self, BufRead, Read, Write};

use serde_json::{Value, json};

/// How the agent should use the browser: the MCP server's instructions, which agents read with
/// the tools (the skill the Browser plugin carries).
pub(crate) const INSTRUCTIONS: &str = include_str!("../../resources/browser/INSTRUCTIONS.md");

fn request(tool: &str, arguments: Value) -> Value {
    request_to(
        std::env::var("GHOST_BROWSER_ENDPOINT").ok(),
        std::env::var("GHOST_BROWSER_TOKEN").ok(),
        tool,
        arguments,
    )
}

fn request_to(
    endpoint: Option<String>,
    token: Option<String>,
    tool: &str,
    arguments: Value,
) -> Value {
    let result = (|| -> Result<Value, Box<dyn std::error::Error>> {
        let endpoint = endpoint
            .ok_or("no endpoint")?
            .parse::<std::net::SocketAddr>()?;
        if !endpoint.ip().is_loopback() {
            return Err("The browser service must be local to the Ghost app.".into());
        }
        let token = token.ok_or("no token")?;
        let mut stream =
            std::net::TcpStream::connect_timeout(&endpoint, std::time::Duration::from_secs(3))?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(150)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
        let mut bytes = serde_json::to_vec(&json!({
            "token": token,
            "tool": tool,
            "arguments": arguments,
        }))?;
        if bytes.len() as u64 >= super::REQUEST_LIMIT {
            return Err("That browser request is too large.".into());
        }
        bytes.push(b'\n');
        stream.write_all(&bytes)?;
        let mut line = String::new();
        io::BufReader::new(stream.take(super::REPLY_LIMIT)).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    })();
    result.unwrap_or_else(|_| {
        json!({
            "text": "Ghost's browser is unavailable: the app cannot be reached, or this session's browser access ended. Ask the person to restart the session in Ghost, or work without the browser.",
            "error": true,
        })
    })
}

/// The MCP result for a reply from the app: its text, and a screenshot as an image.
fn content(reply: &Value) -> Value {
    let mut content = vec![json!({
        "type": "text",
        "text": reply["text"].as_str().unwrap_or(""),
    })];
    if let (Some(mime), Some(data)) = (
        reply["image"]["mime"].as_str(),
        reply["image"]["data"].as_str(),
    ) {
        content.push(json!({ "type": "image", "mimeType": mime, "data": data }));
    }
    json!({ "content": content, "isError": reply["error"].as_bool().unwrap_or(false) })
}

fn reference() -> Value {
    json!({ "type": "string", "description": "A ref from the latest browser_snapshot (e12), or a CSS selector." })
}

pub(crate) fn tools() -> Value {
    json!([
        {
            "name": "browser_navigate",
            "description": "Open an address in the chat's browser tab in front (a URL, a host such as localhost:5173, or words to search for; web pages only), or go back, forward or reload with action. Waits for the page to load and says where it is.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "What to open." },
                    "action": { "type": "string", "enum": ["back", "forward", "reload"], "description": "Instead of url: move through the tab's history or reload." },
                    "new_tab": { "type": "boolean", "description": "Open it in a new tab instead of the one in front." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "browser_snapshot",
            "description": "Read the page as an outline: each element's role, name and value, with a ref (e12) on everything you can act on. Password values are never shown. Take a new snapshot after the page changes; refs from an old one may be gone.",
            "inputSchema": {
                "type": "object",
                "properties": { "selector": { "type": "string", "description": "Read only the part of the page this CSS selector matches." } },
                "additionalProperties": false
            }
        },
        {
            "name": "browser_click",
            "description": "Click an element (scrolled into view first). Says what was clicked, if something else covered it, and where the tab is afterwards.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": reference(),
                    "button": { "type": "string", "enum": ["left", "right", "middle"] },
                    "double": { "type": "boolean", "description": "Double-click." },
                    "modifiers": { "type": "array", "items": { "type": "string", "enum": ["Alt", "Control", "Meta", "Shift"] } }
                },
                "required": ["ref"],
                "additionalProperties": false
            }
        },
        {
            "name": "browser_type",
            "description": "Type into a text field or editable element: it is focused and its text replaced (clear false keeps it), then Enter is pressed if submit is true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": reference(),
                    "text": { "type": "string" },
                    "clear": { "type": "boolean", "description": "Replace what the field holds (the default)." },
                    "submit": { "type": "boolean", "description": "Press Enter afterwards." }
                },
                "required": ["ref", "text"],
                "additionalProperties": false
            }
        },
        {
            "name": "browser_press_key",
            "description": "Press a key or a chord where the page's focus is: Enter, Escape, Tab, ArrowDown, Backspace, PageDown, a letter, or a chord such as Control+a or Shift+Tab.",
            "inputSchema": {
                "type": "object",
                "properties": { "key": { "type": "string" } },
                "required": ["key"],
                "additionalProperties": false
            }
        },
        {
            "name": "browser_hover",
            "description": "Move the pointer over an element (menus and tooltips that open on hover).",
            "inputSchema": {
                "type": "object",
                "properties": { "ref": reference() },
                "required": ["ref"],
                "additionalProperties": false
            }
        },
        {
            "name": "browser_select",
            "description": "Choose options in a select element by their value or visible text.",
            "inputSchema": {
                "type": "object",
                "properties": { "ref": reference(), "values": { "type": "array", "items": { "type": "string" } } },
                "required": ["ref", "values"],
                "additionalProperties": false
            }
        },
        {
            "name": "browser_scroll",
            "description": "Scroll the page (dy pixels down, dx right; 600 down by default), scroll inside an element, or bring an element into view (ref without dx or dy).",
            "inputSchema": {
                "type": "object",
                "properties": { "ref": reference(), "dx": { "type": "number" }, "dy": { "type": "number" } },
                "additionalProperties": false
            }
        },
        {
            "name": "browser_screenshot",
            "description": "See the page as it is drawn: what the tab shows, one element (ref), or the whole page (full_page). Use it to check layout and visuals; read text with browser_snapshot.",
            "inputSchema": {
                "type": "object",
                "properties": { "ref": reference(), "full_page": { "type": "boolean" } },
                "additionalProperties": false
            }
        },
        {
            "name": "browser_console",
            "description": "The page's console messages and uncaught errors since it loaded, oldest first.",
            "inputSchema": {
                "type": "object",
                "properties": { "clear": { "type": "boolean", "description": "Empty the list after reading it." } },
                "additionalProperties": false
            }
        },
        {
            "name": "browser_network",
            "description": "The requests the page made (documents, scripts, fetches, images) with status, kind and time.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "browser_evaluate",
            "description": "Run JavaScript in the page and get its value as JSON. The last expression is the value; a body with return statements works too, and awaits are allowed where the engine supports them.",
            "inputSchema": {
                "type": "object",
                "properties": { "expression": { "type": "string" } },
                "required": ["expression"],
                "additionalProperties": false
            }
        },
        {
            "name": "browser_wait_for",
            "description": "Wait until text or a CSS selector appears on the page (or is gone, with gone), up to seconds (10 by default, at most 30). With neither, just wait the seconds.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "selector": { "type": "string" },
                    "gone": { "type": "boolean" },
                    "seconds": { "type": "number" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "browser_tabs",
            "description": "List the chat's browser tabs, open a new one (optionally at url), bring one to the front, or close one, by its index in the list.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "new", "select", "close"] },
                    "index": { "type": "integer", "minimum": 0 },
                    "url": { "type": "string" }
                },
                "additionalProperties": false
            }
        }
    ])
}

/// Every tool the helper offers.
pub(crate) fn tool_names() -> Vec<String> {
    tools()
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub fn run_helper() {
    if std::env::args().any(|value| value == "--skill") {
        print!("{}", include_str!("../../resources/browser/SKILL.md"));
        return;
    }
    if !std::env::args().any(|value| value == "--mcp") {
        println!(
            "Usage: ghost --ghost-browser-helper --mcp (an MCP server for Ghost's browser) or --skill"
        );
        return;
    }
    let names = tool_names();
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.len() as u64 > super::REQUEST_LIMIT {
            continue;
        }
        let Ok(input) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // Notifications (no id) need no answer.
        let Some(id) = input.get("id").cloned() else {
            continue;
        };
        let result = match input["method"].as_str() {
            Some("initialize") => json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "ghost-browser", "version": "1.0.0" },
                "instructions": INSTRUCTIONS,
            }),
            Some("ping") => json!({}),
            Some("tools/list") => json!({ "tools": tools() }),
            Some("tools/call") => {
                let name = input["params"]["name"].as_str().unwrap_or("");
                let mut arguments = input["params"]["arguments"].clone();
                if !arguments.is_object() {
                    arguments = json!({});
                }
                if names.iter().any(|known| known == name) {
                    content(&request(name, arguments))
                } else {
                    content(
                        &json!({ "text": format!("{name} is not one of Ghost's browser tools."), "error": true }),
                    )
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_strict_schema_and_a_description() {
        let tools = tools();
        let tools = tools.as_array().unwrap();
        assert_eq!(tools.len(), 14);
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            assert!(name.starts_with("browser_"), "{name}");
            assert!(tool["description"].as_str().unwrap().len() > 40, "{name}");
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
        }
        let click = tools
            .iter()
            .find(|tool| tool["name"] == "browser_click")
            .unwrap();
        assert_eq!(click["inputSchema"]["required"], json!(["ref"]));
    }

    #[test]
    fn replies_become_mcp_content_with_their_screenshot() {
        let plain = content(&json!({ "text": "Clicked.", "error": false }));
        assert_eq!(plain["content"].as_array().unwrap().len(), 1);
        assert_eq!(plain["isError"], false);
        let shot = content(&json!({
            "text": "Now at x.",
            "error": false,
            "image": { "mime": "image/png", "data": "iVBORw0KGgo=" }
        }));
        assert_eq!(shot["content"][1]["type"], "image");
        assert_eq!(shot["content"][1]["mimeType"], "image/png");
        let failed = content(&json!({ "text": "No.", "error": true }));
        assert_eq!(failed["isError"], true);
    }

    #[test]
    fn the_instructions_teach_the_tools_and_the_rules() {
        for phrase in [
            "browser_navigate",
            "browser_snapshot",
            "ref",
            "browser_screenshot",
            "untrusted",
            "Ask the person",
            "passwords",
            "annotat",
        ] {
            assert!(INSTRUCTIONS.contains(phrase), "{phrase}");
        }
        for name in tool_names() {
            assert!(INSTRUCTIONS.contains(&name), "the instructions name {name}");
        }
    }

    #[test]
    fn without_the_app_a_call_says_how_to_go_on() {
        for (endpoint, token) in [
            (None, None),
            (Some("not an address".to_owned()), Some("x".to_owned())),
            // Never anywhere but this machine.
            (Some("192.0.2.1:9".to_owned()), Some("x".to_owned())),
        ] {
            let reply = request_to(endpoint, token, "browser_snapshot", json!({}));
            assert_eq!(reply["error"], true);
            assert!(reply["text"].as_str().unwrap().contains("unavailable"));
        }
    }
}
