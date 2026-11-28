//! `ghost --ghost-game-helper --mcp`: the stdio MCP server an agent starts for the chat's game.
//! It never talks to a game itself; each call goes to the running app over the session's
//! loopback grant (`GHOST_GAME_ENDPOINT`, `GHOST_GAME_HELPER_TOKEN`).

use std::io::{self, BufRead, Read, Write};

use serde_json::{Value, json};

/// How the agent should work on a game: the MCP server's instructions, which agents read with
/// the tools (the skill the Game plugin carries).
pub(crate) const INSTRUCTIONS: &str = include_str!("../../resources/game/INSTRUCTIONS.md");

fn request(tool: &str, arguments: Value) -> Value {
    request_to(
        std::env::var("GHOST_GAME_ENDPOINT").ok(),
        std::env::var("GHOST_GAME_HELPER_TOKEN").ok(),
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
            return Err("The game service must be local to the Ghost app.".into());
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
            return Err("That game request is too large.".into());
        }
        bytes.push(b'\n');
        stream.write_all(&bytes)?;
        let mut line = String::new();
        io::BufReader::new(stream.take(super::REPLY_LIMIT)).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    })();
    result.unwrap_or_else(|_| {
        json!({
            "text": "Ghost's game tools are unavailable: the app cannot be reached, or this session's game access ended. Ask the person to restart the session in Ghost, or run the game from your shell.",
            "error": true,
        })
    })
}

/// The MCP result for a reply from the app: its text, then its pictures in order.
fn content(reply: &Value) -> Value {
    let mut content = vec![json!({
        "type": "text",
        "text": reply["text"].as_str().unwrap_or(""),
    })];
    for image in reply["images"].as_array().into_iter().flatten() {
        if let (Some(mime), Some(data)) = (image["mime"].as_str(), image["data"].as_str()) {
            content.push(json!({ "type": "image", "mimeType": mime, "data": data }));
        }
    }
    json!({ "content": content, "isError": reply["error"].as_bool().unwrap_or(false) })
}

fn node() -> Value {
    json!({ "type": "string", "description": "A ref from the latest game_tree (g12), a path from the scene's top like Player/Sprite2D, or . for the scene's top node." })
}

fn nothing() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

pub(crate) fn tools() -> Value {
    json!([
        {
            "name": "game_status",
            "description": "What this chat's folder holds (the engine, its version, the project's name), where Godot is on this computer, and whether the game runs now. Start here.",
            "inputSchema": nothing()
        },
        {
            "name": "game_check",
            "description": "Load every script and scene of a Godot project without running the game, and list what does not parse, by file and line. Run it after you edit scripts or scenes and before game_run: it takes seconds.",
            "inputSchema": nothing()
        },
        {
            "name": "game_run",
            "description": "Start this chat's game, or start it again after you changed its files (a running game does not pick up edits). It opens the Game pane beside the chat, plays half a second and freezes: the answer is a picture of the game and any errors from its start. Call it yourself, without being asked, whenever you build or change a game, and check your work in it. A web game: give its url (start its dev server first) and it opens in Ghost's browser beside the chat instead, with its clock under your control.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scene": { "type": "string", "description": "A scene to run instead of the project's main one, as a path inside the project like res://levels/two.tscn." },
                    "url": { "type": "string", "description": "A web game only: its address, like http://localhost:5173. Without it the page in front of the chat's browser is taken." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "game_stop",
            "description": "Stop this chat's game when the work with it is done.",
            "inputSchema": nothing()
        },
        {
            "name": "game_look",
            "description": "See the game as it is drawn now, without moving time. Says the picture's size: positions in game_tree and for clicks are in its pixels.",
            "inputSchema": nothing()
        },
        {
            "name": "game_tree",
            "description": "Read the running scene as an outline: each node's ref (g12), name and class, where it is on screen, and the text it shows (a label, a button). Refs are from this outline; read it again after the scene changes.",
            "inputSchema": nothing()
        },
        {
            "name": "game_inspect",
            "description": "One node's own values as the game has them now: its class and script, where it is, its exported and script variables, its groups, and for a character body whether it stands on the floor and how fast it moves.",
            "inputSchema": {
                "type": "object",
                "properties": { "node": node() },
                "required": ["node"],
                "additionalProperties": false
            }
        },
        {
            "name": "game_set",
            "description": "Change one property of a node in the running game, to try a value (a speed, a colour, a position) before you write it down. It lasts for this run only: to keep it, change the scene or script file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node": node(),
                    "property": { "type": "string", "description": "The property's name, like speed or position." },
                    "value": { "type": "string", "description": "The new value as Godot writes it: 400.0, true, \"text\", Vector2(10, 20), Color(1, 0, 0)." }
                },
                "required": ["node", "property", "value"],
                "additionalProperties": false
            }
        },
        {
            "name": "game_act",
            "description": "Play the game: hold inputs for stretches of game time, one step after another, then freeze again. Time runs in fixed steps of 1/60 s, as fast as the computer can, so the same steps give the same result every time. Answers with a filmstrip of the action (its frames in one picture, left to right and top to bottom), a picture of where it ended, and any new errors. A step holds an input action of the project (ui_right, jump) or a key by its name (ArrowLeft, Space, KeyW) for ms milliseconds; a step with only ms lets time pass. A web game takes keys by name and clicks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 64,
                        "description": "What to do, in order, like [{\"press\": \"ui_right\", \"ms\": 500}, {\"hold\": [\"ui_right\", \"jump\"], \"ms\": 300}, {\"ms\": 200}].",
                        "items": {
                            "type": "object",
                            "properties": {
                                "press": { "type": "string", "description": "One action or key, held for the whole step." },
                                "hold": { "type": "array", "items": { "type": "string" }, "description": "Several actions or keys, held together for the whole step." },
                                "ms": { "type": "number", "description": "How long the step lasts in game time (100 by default)." },
                                "click": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "A mouse click at [x, y] in the picture's pixels, held for the step." },
                                "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "The mouse button for click (left by default)." },
                                "move": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Move the mouse to [x, y] without clicking." }
                            },
                            "additionalProperties": false
                        }
                    },
                    "shots": { "type": "number", "description": "How many frames the filmstrip shows (6 by default, at most 12; 0 for the last frame only)." }
                },
                "required": ["steps"],
                "additionalProperties": false
            }
        },
        {
            "name": "game_wait_for",
            "description": "Let the game run, with no input, until an expression becomes true or the time is up, then freeze again. Answers whether it became true, after how much game time, with a picture. The expression is GDScript with tree, scene and root to use, like scene.get_node(\"Player\").is_on_floor(); in a web game it is JavaScript, like player.onGround.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "until": { "type": "string", "description": "An expression that becomes true: GDScript, or JavaScript in a web game." },
                    "timeout_ms": { "type": "number", "description": "The most game time to wait (5000 by default, at most 120000)." }
                },
                "required": ["until"],
                "additionalProperties": false
            }
        },
        {
            "name": "game_logs",
            "description": "This run's errors and warnings, each with its file, line and the project's own stack frames, and the last lines the game printed. Read it when the game shows nothing, stops, or behaves wrongly.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "lines": { "type": "number", "description": "How many of the latest printed lines (60 by default, at most 300)." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "game_eval",
            "description": "Run GDScript inside the running game and read what it returns: one expression, or several lines as the body of a function that may return a value. tree (the SceneTree), scene (the running scene's top node) and root are there to use. For questions the outline cannot answer and for setting a test up (put the player at the door, give 99 coins). It changes this run only. In a web game the code is JavaScript, run in the page.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "The expression or the function's body, at most 16 KB." }
                },
                "required": ["code"],
                "additionalProperties": false
            }
        },
        {
            "name": "game_perf",
            "description": "Measure the game over a stretch of frames run flat out: how long a frame takes on average and at worst, the frame rate the game could reach, draw calls, nodes, physics time and memory. Use it to find what makes a game slow.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "frames": { "type": "number", "description": "How many frames to measure (120 by default, 10 to 600)." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "game_time",
            "description": "Freeze the game or let it run live, or change how fast it runs. Unfreeze it when you are done and the person wants to play; freeze it again before you act.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "frozen": { "type": "boolean", "description": "true freezes the game; false lets it run live for the person." },
                    "speed": { "type": "number", "description": "How fast live time runs: 0.1 to 4, where 1 is usual." }
                },
                "additionalProperties": false
            }
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
        print!("{}", include_str!("../../resources/game/SKILL.md"));
        return;
    }
    if !std::env::args().any(|value| value == "--mcp") {
        println!(
            "Usage: ghost --ghost-game-helper --mcp (an MCP server for the chat's game) or --skill"
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
                "serverInfo": { "name": "ghost-game", "version": "1.0.0" },
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
                        &json!({ "text": format!("{name} is not one of Ghost's game tools."), "error": true }),
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
            assert!(name.starts_with("game_"), "{name}");
            assert!(tool["description"].as_str().unwrap().len() > 40, "{name}");
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
        }
        let act = tools
            .iter()
            .find(|tool| tool["name"] == "game_act")
            .unwrap();
        assert_eq!(act["inputSchema"]["required"], json!(["steps"]));
        assert_eq!(
            act["inputSchema"]["properties"]["steps"]["items"]["additionalProperties"],
            false
        );
        // No tool fetches an engine, exports a game or puts one anywhere: those are the
        // person's, in the pane. None names a chat or a folder either.
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            for word in [
                "fetch", "download", "install", "export", "publish", "upload", "share",
            ] {
                assert!(!name.contains(word), "{name}");
            }
            for argument in ["chat", "chat_id", "folder", "path", "project"] {
                assert!(
                    tool["inputSchema"]["properties"].get(argument).is_none(),
                    "{name} takes {argument}"
                );
            }
        }
    }

    #[test]
    fn replies_become_mcp_content_with_their_pictures_in_order() {
        let plain = content(&json!({ "text": "Stopped.", "error": false, "images": [] }));
        assert_eq!(plain["content"].as_array().unwrap().len(), 1);
        assert_eq!(plain["isError"], false);
        let acted = content(&json!({
            "text": "Played 30 frames.",
            "error": false,
            "images": [
                { "mime": "image/jpeg", "data": "c3RyaXA=" },
                { "mime": "image/jpeg", "data": "ZW5k" },
                { "mime": 7 },
            ],
        }));
        let parts = acted["content"].as_array().unwrap();
        assert_eq!(parts.len(), 3, "a picture without its bytes is left out");
        assert_eq!(parts[1]["type"], "image");
        assert_eq!(parts[1]["mimeType"], "image/jpeg");
        assert_eq!(parts[1]["data"], "c3RyaXA=");
        assert_eq!(parts[2]["data"], "ZW5k");
        let failed = content(&json!({ "text": "No.", "error": true }));
        assert_eq!(failed["isError"], true);
    }

    #[test]
    fn the_instructions_teach_the_tools_and_the_rules() {
        for phrase in [
            "frozen",
            "filmstrip",
            "game_check",
            "untrusted",
            "Only the person",
            "Use it without being asked",
            "does not pick up",
            "this run only",
            "fun",
        ] {
            assert!(INSTRUCTIONS.contains(phrase), "{phrase}");
        }
        // Agents that never read a server's instructions see it in the tool itself.
        let run = tools()
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "game_run")
            .unwrap()["description"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(run.contains("without being asked"), "{run}");
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
            let reply = request_to(endpoint, token, "game_look", json!({}));
            assert_eq!(reply["error"], true);
            assert!(reply["text"].as_str().unwrap().contains("unavailable"));
        }
    }
}
