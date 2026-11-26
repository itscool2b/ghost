//! `ghost --ghost-mobile-helper --mcp`: the stdio MCP server an agent starts for the chat's device.
//! It never talks to a device itself; each call goes to the running app over the session's
//! loopback grant (`GHOST_MOBILE_ENDPOINT`, `GHOST_MOBILE_TOKEN`).

use std::io::{self, BufRead, Read, Write};

use serde_json::{Value, json};

/// How the agent should use the device: the MCP server's instructions, which agents read with
/// the tools (the skill the Mobile plugin carries).
pub(crate) const INSTRUCTIONS: &str = include_str!("../../resources/mobile/INSTRUCTIONS.md");

fn request(tool: &str, arguments: Value) -> Value {
    request_to(
        std::env::var("GHOST_MOBILE_ENDPOINT").ok(),
        std::env::var("GHOST_MOBILE_TOKEN").ok(),
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
            return Err("The device service must be local to the Ghost app.".into());
        }
        let token = token.ok_or("no token")?;
        let mut stream =
            std::net::TcpStream::connect_timeout(&endpoint, std::time::Duration::from_secs(3))?;
        // Longer than the app allows one call, so the app's own answer is the one that arrives.
        stream.set_read_timeout(Some(std::time::Duration::from_secs(
            super::CALL_SECONDS + 30,
        )))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
        let mut bytes = serde_json::to_vec(&json!({
            "token": token,
            "tool": tool,
            "arguments": arguments,
        }))?;
        if bytes.len() as u64 >= super::REQUEST_LIMIT {
            return Err("That device request is too large.".into());
        }
        bytes.push(b'\n');
        stream.write_all(&bytes)?;
        let mut line = String::new();
        io::BufReader::new(stream.take(super::REPLY_LIMIT)).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    })();
    result.unwrap_or_else(|_| {
        json!({
            "text": "Ghost's devices are unavailable: the app cannot be reached, or this session's device access ended. Ask the person to restart the session in Ghost, or work without a device.",
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
    json!({ "type": "string", "description": "A ref from the latest mobile_snapshot (d12)." })
}

fn package() -> Value {
    json!({ "type": "string", "description": "The app's Android package or iPhone bundle id, like com.example.app." })
}

fn nothing() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

pub(crate) fn tools() -> Value {
    json!([
        {
            "name": "mobile_devices",
            "description": "List the devices Ghost knows (its own Android emulators, its own iPhone simulators on a Mac, and phones on a cable), each one's state, and which is this chat's.",
            "inputSchema": nothing()
        },
        {
            "name": "mobile_start",
            "description": "Start this chat's device and wait until it is ready: an Android emulator of this chat's own, or with platform ios an iPhone simulator of its own (a Mac with Xcode), made if there is none. Call it yourself, without being asked, whenever you build, run, fix or test a phone app, and check your work on the device: it opens the Device pane beside the chat, so the person need not. A first start takes about a minute. Says the screen's size, and for an iPhone its UDID to build for. Only the person can choose a phone, in the Device pane.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "platform": { "type": "string", "enum": ["android", "ios"], "description": "android (the default when Android is set up) or ios." },
                    "device": { "type": "string", "description": "A device id from mobile_devices, to start that emulator or simulator instead of any free one." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_install",
            "description": "Install an app built on this computer on the device, replacing the copy that is there: an .apk for Android, or the .app a build made for the iPhone simulator. Build it first with the project's own tools.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string", "description": "The .apk file's or .app folder's full path on this computer." } },
                "required": ["path"],
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_launch",
            "description": "Start an installed app at its first screen (by its package, or its bundle id on an iPhone), or at the Android activity named. Says what was started.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "package": package(),
                    "activity": { "type": "string", "description": "Android only: an activity to start instead of the app's launcher screen, like .SettingsActivity." }
                },
                "required": ["package"],
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_stop_app",
            "description": "End an app's processes, so its next start is a fresh one.",
            "inputSchema": {
                "type": "object",
                "properties": { "package": package() },
                "required": ["package"],
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_snapshot",
            "description": "Read the screen as an outline: each item's role, name and value, with a ref (d12) on everything you can act on. Password text is never shown. Take a new snapshot after the screen changes; refs from an old one may be gone.",
            "inputSchema": nothing()
        },
        {
            "name": "mobile_tap",
            "description": "Tap an item by its ref, or a point by x and y in a screenshot's pixels. long holds the finger down for a long press.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": reference(),
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "long": { "type": "boolean", "description": "Press and hold." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_type",
            "description": "Type text where the focus is, or into the field named by ref (it is tapped first). clear replaces what the field holds; submit presses Enter afterwards.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "ref": reference(),
                    "clear": { "type": "boolean", "description": "Select everything in the field and replace it." },
                    "submit": { "type": "boolean", "description": "Press Enter afterwards." }
                },
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_swipe",
            "description": "Move a finger across the screen: in a direction (the finger moves that way, so up scrolls a list down), inside the item named by ref or across the middle of the screen; or from one point to another, as [x, y] in a screenshot's pixels.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "ref": reference(),
                    "from": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                    "to": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                    "duration_ms": { "type": "number", "description": "How long the finger takes (300 by default). Slower drags; faster flings." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_press",
            "description": "Press a key: back, home, recents, enter, tab, escape, delete, space, up, down, left, right, page_up, page_down, menu, volume_up, volume_down or power. On an iPhone, back swipes in from the left edge, and menu and the volume keys do not exist.",
            "inputSchema": {
                "type": "object",
                "properties": { "key": { "type": "string" } },
                "required": ["key"],
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_screenshot",
            "description": "See the screen as it is drawn. Use it to check layout, colours and visuals; read text with mobile_snapshot. Says the picture's size, which is what x and y for mobile_tap and mobile_swipe are measured in.",
            "inputSchema": nothing()
        },
        {
            "name": "mobile_logs",
            "description": "The log lines of an app while it runs, and its last crash with the stack trace. Without a package: the app this chat last started, or the one in front.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "package": package(),
                    "lines": { "type": "number", "description": "How many of the latest lines (120 by default, at most 500)." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_open_link",
            "description": "Open a link on the device as a tap on it would: a web page (https://...) or one of an app's own links (myapp://screen).",
            "inputSchema": {
                "type": "object",
                "properties": { "link": { "type": "string" } },
                "required": ["link"],
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_settings",
            "description": "Change how the device is set, to check the app under each: appearance (light or dark), text_scale (0.85 to 2 on Android, up to 3 on an iPhone; 1 is usual), orientation (portrait or landscape, Android only), or grant or take away one of an app's permissions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "appearance": { "type": "string", "enum": ["light", "dark"] },
                    "text_scale": { "type": "number" },
                    "orientation": { "type": "string", "enum": ["portrait", "landscape"] },
                    "permission": {
                        "type": "object",
                        "properties": {
                            "package": package(),
                            "name": { "type": "string", "description": "Android: like android.permission.CAMERA. iPhone: calendar, contacts, location, location-always, photos, photos-add, media-library, microphone, motion, reminders, siri or all." },
                            "granted": { "type": "boolean" }
                        },
                        "required": ["package", "name"],
                        "additionalProperties": false
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_wait_for",
            "description": "Wait until text appears on the screen (or is gone, with gone), up to timeout_seconds (10 by default, at most 60). Answers with the screen's outline once it has.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "gone": { "type": "boolean" },
                    "timeout_seconds": { "type": "number" }
                },
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "mobile_stop",
            "description": "Stop this chat's emulator or iPhone simulator when the work with it is done, or let go of the person's phone. Its state is kept, so its next start is quicker.",
            "inputSchema": nothing()
        }
    ])
}

/// Every tool the helper offers.
pub fn tool_names() -> Vec<String> {
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
        print!("{}", include_str!("../../resources/mobile/SKILL.md"));
        return;
    }
    if !std::env::args().any(|value| value == "--mcp") {
        println!(
            "Usage: ghost --ghost-mobile-helper --mcp (an MCP server for the chat's device) or --skill"
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
                "serverInfo": { "name": "ghost-mobile", "version": "1.0.0" },
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
                        &json!({ "text": format!("{name} is not one of Ghost's device tools."), "error": true }),
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
        assert_eq!(tools.len(), 16);
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            assert!(name.starts_with("mobile_"), "{name}");
            assert!(tool["description"].as_str().unwrap().len() > 40, "{name}");
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
        }
        let launch = tools
            .iter()
            .find(|tool| tool["name"] == "mobile_launch")
            .unwrap();
        assert_eq!(launch["inputSchema"]["required"], json!(["package"]));
        // No tool sets up the SDK, accepts a licence or hands an agent a phone: those are the
        // person's, in the pane.
        for name in tool_names() {
            for word in ["fetch", "license", "licence", "allow", "consent", "publish"] {
                assert!(!name.contains(word), "{name}");
            }
        }
    }

    #[test]
    fn replies_become_mcp_content_with_their_screenshot() {
        let plain = content(&json!({ "text": "Tapped.", "error": false }));
        assert_eq!(plain["content"].as_array().unwrap().len(), 1);
        assert_eq!(plain["isError"], false);
        let shot = content(&json!({
            "text": "The screen.",
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
            "mobile_start",
            "mobile_snapshot",
            "ref",
            "mobile_screenshot",
            "untrusted",
            "Ask the person",
            "Passwords",
            "Only the person can choose a phone",
            "signing keys",
            // Agents start the device themselves for phone work (owner, 24 November 2026).
            "Use it without being asked",
            "your first call opens it",
        ] {
            assert!(INSTRUCTIONS.contains(phrase), "{phrase}");
        }
        // Agents that never read a server's instructions see it in the tool itself.
        let start = tools()
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "mobile_start")
            .unwrap()["description"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(start.contains("without being asked"), "{start}");
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
            let reply = request_to(endpoint, token, "mobile_snapshot", json!({}));
            assert_eq!(reply["error"], true);
            assert!(reply["text"].as_str().unwrap().contains("unavailable"));
        }
    }
}
