//! The tools agents play a game with (`game_*`): what each does to the chat's own game, and the
//! words and pictures it answers with.
//!
//! Every tool acts on the game of the chat whose session called it; none takes a chat or a folder.
//! No tool fetches an engine, exports or publishes: those are the person's, in the pane. What the
//! game says (node names, text, errors) is cut to size before it reaches the agent, and it is data.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::bridge::Reply;
use crate::detect::{GameEngine, GameProject};
use crate::manager::{GameState, Manager, TURNED_OFF, ToolReply};
use crate::{Error, check, run};

/// Every tool, as agents name them.
pub const TOOLS: [&str; 14] = [
    "game_status",
    "game_check",
    "game_run",
    "game_stop",
    "game_look",
    "game_tree",
    "game_inspect",
    "game_set",
    "game_act",
    "game_wait_for",
    "game_logs",
    "game_eval",
    "game_perf",
    "game_time",
];

/// How long the game has to answer an ordinary question.
const ANSWER: Duration = Duration::from_secs(10);
/// How long an action or a wait may take in real time: two minutes of game time runs in seconds,
/// but a heavy game may not manage better than real time.
const ACTION: Duration = Duration::from_secs(150);

fn word(arguments: &Value, key: &str) -> String {
    arguments[key]
        .as_str()
        .unwrap_or_default()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(40)
        .collect()
}

/// What the pane says an agent is doing. It never repeats code, values or text the agent sent
/// beyond a short name: those may be private, and the pane is not where they belong.
pub fn label(tool: &str, arguments: &Value) -> String {
    match tool {
        "game_status" => "Checking the game".into(),
        "game_check" => "Checking the scripts".into(),
        "game_run" => "Starting the game".into(),
        "game_stop" => "Stopping the game".into(),
        "game_look" => "Looking".into(),
        "game_tree" => "Reading the scene".into(),
        "game_inspect" => format!("Inspecting {}", word(arguments, "node"))
            .trim()
            .to_owned(),
        "game_set" => "Changing a value".into(),
        "game_act" => {
            let first = arguments["steps"]
                .as_array()
                .and_then(|steps| steps.first());
            match first {
                Some(step) if step["press"].is_string() => {
                    format!("Holding {}", word(step, "press"))
                }
                Some(step) if step["hold"].is_array() => "Holding keys".into(),
                Some(step) if step["click"].is_array() => "Clicking".into(),
                _ => "Playing".into(),
            }
        }
        "game_wait_for" => "Waiting".into(),
        "game_logs" => "Reading the log".into(),
        "game_eval" => "Running code".into(),
        "game_perf" => "Measuring".into(),
        "game_time" => match arguments["frozen"].as_bool() {
            Some(true) => "Freezing".into(),
            Some(false) => "Resuming".into(),
            None => "Setting the speed".into(),
        },
        _ => "Working".into(),
    }
}

/// One tool call. The chat and its folder come from the session's grant.
pub async fn call(
    manager: &Manager,
    chat_id: &str,
    folder: &Path,
    tool: &str,
    arguments: &Value,
) -> ToolReply {
    if !manager.enabled() {
        return ToolReply::failed(TURNED_OFF);
    }
    if !TOOLS.contains(&tool) {
        return ToolReply::failed(format!("Ghost's game tools have no \"{tool}\"."));
    }
    let project = manager.look(chat_id, folder).await;
    // A web game is a page: its tools go to the app's browser, which shows what the agent does
    // there. So do the tools of a chat with no game Ghost knows, where the game may be a plain
    // page, and a game_run that names an address. The status is put together below.
    let web = match &project {
        Some(project) => project.engine == GameEngine::Web,
        None => true,
    } || (tool == "game_run" && arguments["url"].is_string());
    if web
        && tool != "game_status"
        && let Some(browser) = manager.web()
    {
        return browser
            .call(chat_id.to_owned(), tool.to_owned(), arguments.clone())
            .await;
    }
    // Reading what is there does not open the pane; anything that shows or changes the game does.
    if !matches!(
        tool,
        "game_status" | "game_stop" | "game_logs" | "game_check"
    ) {
        manager.summon(chat_id);
    }
    manager.agent_doing(chat_id, Some(label(tool, arguments)));
    let reply = match dispatch(manager, chat_id, folder, project.as_ref(), tool, arguments).await {
        Ok(reply) => reply,
        Err(error) => ToolReply::failed(error.to_string()),
    };
    manager.agent_doing(chat_id, None);
    reply
}

async fn dispatch(
    manager: &Manager,
    chat_id: &str,
    folder: &Path,
    project: Option<&GameProject>,
    tool: &str,
    arguments: &Value,
) -> Result<ToolReply, Error> {
    match tool {
        "game_status" => Ok(status(manager, chat_id, project).await),
        "game_check" => check_scripts(manager, folder, project).await,
        "game_run" => start(manager, chat_id, folder, arguments).await,
        "game_stop" => {
            manager.stop(chat_id).await;
            Ok(ToolReply::text("The game is stopped."))
        }
        "game_look" => {
            let reply = manager
                .ask(chat_id, "shot", json!({"max": 1568}), ANSWER)
                .await?;
            let frozen = reply.value["frozen"].as_bool().unwrap_or(false);
            Ok(pictures(
                format!(
                    "The game as it is now ({}x{}, {}). Positions in game_tree and for clicks are in these pixels.",
                    reply.value["width"],
                    reply.value["height"],
                    if frozen { "frozen" } else { "running live" }
                ),
                reply,
            ))
        }
        "game_tree" => {
            let reply = manager.ask(chat_id, "tree", json!({}), ANSWER).await?;
            Ok(ToolReply::text(outline(&reply.value)))
        }
        "game_inspect" => {
            let node = required(arguments, "node")?;
            let reply = manager
                .ask(chat_id, "inspect", json!({"node": node}), ANSWER)
                .await?;
            Ok(ToolReply::text(inspected(&reply.value)))
        }
        "game_set" => {
            let fields = json!({
                "node": required(arguments, "node")?,
                "property": required(arguments, "property")?,
                "value": required(arguments, "value")?,
            });
            let reply = manager.ask(chat_id, "set", fields, ANSWER).await?;
            Ok(ToolReply::text(format!(
                "Set for this run only: it is now {}. To keep it, change the scene or script file.",
                shown(&reply.value["value"], 240)
            )))
        }
        "game_act" => act(manager, chat_id, arguments).await,
        "game_wait_for" => {
            let until = required(arguments, "until")?;
            let timeout_ms = arguments["timeout_ms"].as_f64().unwrap_or(5000.0);
            let reply = manager
                .ask(
                    chat_id,
                    "wait",
                    json!({"until": until, "timeout_ms": timeout_ms, "max": 1568}),
                    ACTION,
                )
                .await?;
            let met = reply.value["met"].as_bool().unwrap_or(false);
            let text = format!(
                "{} after {:.0} ms of game time. The game is frozen again.{}",
                if met {
                    "The condition became true"
                } else {
                    "The condition was still false"
                },
                reply.value["game_ms"].as_f64().unwrap_or(0.0),
                fresh(manager, chat_id)
            );
            Ok(pictures(text, reply))
        }
        "game_logs" => Ok(logs(manager, chat_id, arguments)),
        "game_eval" => {
            let code = required(arguments, "code")?;
            if code.len() > 16 * 1024 {
                return Err(Error::Refused("The code is longer than 16 KB.".into()));
            }
            let reply = manager
                .ask(chat_id, "eval", json!({"code": code}), ANSWER)
                .await?;
            Ok(ToolReply::text(format!(
                "{}{}",
                shown(&reply.value["value"], 2000),
                fresh(manager, chat_id)
            )))
        }
        "game_perf" => {
            let frames = arguments["frames"].as_u64().unwrap_or(120).clamp(10, 600);
            let reply = manager
                .ask(chat_id, "perf", json!({"frames": frames}), ACTION)
                .await?;
            Ok(ToolReply::text(measured(&reply.value)))
        }
        "game_time" => {
            let frozen = arguments["frozen"].as_bool();
            let speed = arguments["speed"].as_f64().map(|speed| speed as f32);
            let (frozen, speed) = manager.time(chat_id, frozen, speed).await?;
            Ok(ToolReply::text(format!(
                "The game is {} at {speed}x speed.",
                if frozen { "frozen" } else { "running live" }
            )))
        }
        _ => Err(Error::Refused(
            "Ghost's game tools have no such tool.".into(),
        )),
    }
}

fn required<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, Error> {
    arguments[key]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| Error::Refused(format!("\"{key}\" is missing.")))
}

/// What the game said, as text for the agent: cut, and with nothing that could break a line.
fn shown(value: &Value, limit: usize) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Null => "null".to_owned(),
        other => other.to_string(),
    };
    let mut out: String = text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(limit)
        .collect();
    if text.chars().count() > limit {
        out.push('…');
    }
    out
}

fn pictures(text: String, reply: Reply) -> ToolReply {
    ToolReply {
        text,
        images: reply
            .pictures
            .into_iter()
            .map(|picture| picture.jpeg)
            .collect(),
        error: false,
    }
}

/// " New errors: ..." when the last thing asked of the game caused any.
fn fresh(manager: &Manager, chat_id: &str) -> String {
    let errors = manager.fresh_errors(chat_id);
    if errors.is_empty() {
        String::new()
    } else {
        format!("\nNew errors from the game:\n{}", errors.join("\n"))
    }
}

async fn status(manager: &Manager, chat_id: &str, project: Option<&GameProject>) -> ToolReply {
    manager.looked().await;
    let mut lines = Vec::new();
    match project {
        None => lines.push(
            "This chat's folder holds no game Ghost knows. A Godot project has a project.godot at its top; a web game's package.json depends on three, phaser or the like."
                .to_owned(),
        ),
        Some(project) => {
            let name = project.name.as_deref().unwrap_or("(unnamed)");
            let version = project
                .version
                .as_deref()
                .map(|version| format!(" {version}"))
                .unwrap_or_default();
            lines.push(format!(
                "Project: {name}, made with {}{version}.",
                project.engine.name()
            ));
            if let Some(why) = &project.unsupported {
                lines.push(why.clone());
            }
            match project.engine {
                GameEngine::Godot if project.unsupported.is_none() => {
                    match manager.godot_for(project) {
                        Ok(program) => lines.push(format!(
                            "Godot: {} (use this path to run Godot yourself, for example headless tests).",
                            program.display()
                        )),
                        Err(why) => lines.push(format!(
                            "Godot: {why} Only the person can get it: ask them to open the Game pane and press Get Godot."
                        )),
                    }
                }
                GameEngine::Godot => {}
                GameEngine::Web => lines.push(
                    "A web game runs in Ghost's browser beside the chat. Start its dev server (the chat's Run, or your shell), then call game_run with its url: it opens there with its clock under your control, and game_act, game_wait_for, game_look, game_eval (JavaScript), game_tree, game_logs and game_time work on it."
                        .to_owned(),
                ),
                GameEngine::Unity | GameEngine::Unreal | GameEngine::Roblox => lines.push(format!(
                    "Ghost does not run {} itself. Its maker's own agent tools drive its editor; the person connects them in the Game pane.",
                    project.engine.name()
                )),
                _ => lines.push(format!(
                    "Ghost has no special support for {}: run it from your shell.",
                    project.engine.name()
                )),
            }
        }
    }
    // Where a web game stands is the browser's to say.
    let web = project.is_none_or(|project| project.engine == GameEngine::Web);
    if web && let Some(browser) = manager.web() {
        let said = browser
            .call(chat_id.to_owned(), "game_status".to_owned(), json!({}))
            .await;
        lines.push(said.text);
        return ToolReply::text(lines.join("\n"));
    }
    if let Some(game) = manager.chat(chat_id) {
        lines.push(match game.state {
            GameState::Idle => "Game: not running. Start it with game_run.".to_owned(),
            GameState::Starting => "Game: starting.".to_owned(),
            GameState::Running => format!(
                "Game: running {} at {}x{}, {}, on Godot {}.",
                if game.scene.is_empty() {
                    "its main scene"
                } else {
                    &game.scene
                },
                game.width,
                game.height,
                if game.frozen {
                    "frozen (it moves only during game_act and game_wait_for)".to_owned()
                } else {
                    format!("live at {:.0} frames a second", game.fps)
                },
                game.engine_version
            ),
            GameState::Ended => format!(
                "Game: not running. {}",
                game.ended.as_deref().unwrap_or("It ended.")
            ),
        });
        if game.errors > 0 {
            lines.push(format!(
                "Errors this run: {} (game_logs lists them).",
                game.errors
            ));
        }
    }
    ToolReply::text(lines.join("\n"))
}

async fn check_scripts(
    manager: &Manager,
    folder: &Path,
    project: Option<&GameProject>,
) -> Result<ToolReply, Error> {
    let project = project
        .filter(|project| project.engine == GameEngine::Godot)
        .ok_or_else(|| Error::Refused("game_check is for Godot projects.".into()))?;
    if let Some(why) = &project.unsupported {
        return Err(Error::Refused(why.clone()));
    }
    manager.looked().await;
    let godot = manager.godot_for(project).map_err(Error::Refused)?;
    let root = manager.root().to_owned();
    let scripts = tokio::task::spawn_blocking(move || run::write_scripts(&root))
        .await
        .map_err(|_| Error::Game("Ghost's game scripts could not be written.".into()))??;
    let check_script = scripts.with_file_name("check.gd");
    let report = check::run(&godot, folder, &check_script).await?;
    let mut text = if !report.finished {
        "The check did not run to its end: Godot stopped first.".to_owned()
    } else if report.failed == 0 && report.problems.is_empty() {
        format!(
            "All {} scripts and scenes load. This checks that they parse, not that the game plays right: game_run and game_act do that.",
            report.files
        )
    } else {
        format!(
            "{} of {} scripts and scenes did not load.",
            report.failed, report.files
        )
    };
    for problem in &report.problems {
        text.push('\n');
        text.push_str(problem);
    }
    Ok(ToolReply {
        text,
        images: Vec::new(),
        error: !report.finished,
    })
}

async fn start(
    manager: &Manager,
    chat_id: &str,
    folder: &Path,
    arguments: &Value,
) -> Result<ToolReply, Error> {
    let scene = arguments["scene"]
        .as_str()
        .filter(|scene| !scene.is_empty());
    manager.run(chat_id, folder, scene).await?;
    // Half a second of game time, so the first picture is the game and not its first blank
    // frame; then it is frozen, as it stays between an agent's actions.
    let reply = manager
        .ask(
            chat_id,
            "act",
            json!({"steps": [{"ms": 500}], "shots": 0, "max": 1568}),
            ACTION,
        )
        .await?;
    let game = manager.chat(chat_id);
    let (width, height, version) = game
        .map(|game| (game.width, game.height, game.engine_version))
        .unwrap_or_default();
    let text = format!(
        "The game is running on Godot {version}, {width}x{height}, and is frozen after its first half second.\nIt moves only while you act: game_act plays input for a stretch of game time and shows you what happened, game_wait_for runs it until something is true. game_tree lists what is in the scene, game_time lets it run live for the person.{}",
        fresh(manager, chat_id)
    );
    Ok(pictures(text, reply))
}

async fn act(manager: &Manager, chat_id: &str, arguments: &Value) -> Result<ToolReply, Error> {
    let steps = arguments["steps"]
        .as_array()
        .filter(|steps| !steps.is_empty())
        .ok_or_else(|| {
            Error::Refused(
                "\"steps\" is a list like [{\"press\": \"ui_right\", \"ms\": 500}, {\"press\": \"ui_accept\", \"ms\": 50}]."
                    .into(),
            )
        })?;
    if steps.len() > 64 {
        return Err(Error::Refused("An action has at most 64 steps.".into()));
    }
    let shots = arguments["shots"].as_u64().unwrap_or(6).min(12);
    let reply = manager
        .ask(
            chat_id,
            "act",
            json!({"steps": steps, "shots": shots, "max": 1568}),
            ACTION,
        )
        .await?;
    let strip = reply.value["strip"].as_u64().unwrap_or(0);
    let text = format!(
        "Played {} frames ({:.0} ms of game time). The game is frozen again.\n{}{}",
        reply.value["frames"].as_u64().unwrap_or(0),
        reply.value["game_ms"].as_f64().unwrap_or(0.0),
        if strip > 1 {
            format!(
                "First picture: the action in {strip} frames, left to right and top to bottom. Second picture: where it ended."
            )
        } else {
            "The picture is where it ended.".to_owned()
        },
        fresh(manager, chat_id)
    );
    Ok(pictures(text, reply))
}

fn logs(manager: &Manager, chat_id: &str, arguments: &Value) -> ToolReply {
    let lines = arguments["lines"].as_u64().unwrap_or(60).clamp(1, 300) as usize;
    let (errors, printed, dropped) = manager.log(chat_id, lines);
    let mut text = String::new();
    if errors.is_empty() {
        text.push_str("No errors or warnings this run.\n");
    } else {
        text.push_str("Errors and warnings this run, oldest first:\n");
        for error in &errors {
            text.push_str(&error.line());
            text.push('\n');
            for frame in error.stack.iter().skip(1).take(4) {
                text.push_str("    ");
                text.push_str(frame);
                text.push('\n');
            }
        }
    }
    if printed.is_empty() {
        text.push_str("The game has printed nothing.");
    } else {
        text.push_str(&format!("Last {} lines the game printed", printed.len()));
        if dropped > 0 {
            text.push_str(&format!(" ({dropped} older lines are gone)"));
        }
        text.push_str(":\n");
        text.push_str(&printed.join("\n"));
    }
    ToolReply::text(text)
}

/// The scene as lines: each node's ref, name, class, where it is on screen, and what it says.
pub fn outline(value: &Value) -> String {
    let mut lines = Vec::new();
    let scene = value["scene"].as_str().unwrap_or_default();
    if !scene.is_empty() {
        lines.push(format!("Scene {}", shown(&json!(scene), 200)));
    }
    for node in value["nodes"].as_array().into_iter().flatten().take(600) {
        let indent = "  ".repeat(node["depth"].as_u64().unwrap_or(0).min(24) as usize);
        if let Some(more) = node["more"].as_u64() {
            lines.push(format!("{indent}… and {more} more"));
            continue;
        }
        let mut line = format!(
            "{indent}{} {} ({})",
            shown(&node["ref"], 8),
            shown(&node["name"], 60),
            shown(&node["class"], 40)
        );
        if let Some(at) = node["at"].as_array().filter(|at| at.len() == 2) {
            line.push_str(&format!(" at {},{}", at[0], at[1]));
        } else if let Some(rect) = node["rect"].as_array().filter(|rect| rect.len() == 4) {
            line.push_str(&format!(
                " at {},{} size {}x{}",
                rect[0], rect[1], rect[2], rect[3]
            ));
        }
        if node["visible"].as_bool() == Some(false) {
            line.push_str(" [hidden]");
        }
        if let Some(script) = node["script"].as_str() {
            line.push_str(&format!(" script {}", shown(&json!(script), 120)));
        }
        if let Some(text) = node["text"].as_str() {
            line.push_str(&format!(" \"{}\"", shown(&json!(text), 80)));
        }
        lines.push(line);
    }
    if value["cut"].as_bool() == Some(true) {
        lines.push("… the scene has more nodes than are listed.".to_owned());
    }
    lines.push(
        "Refs (g1, g2…) are good until the next game_tree. Positions are in the game's own pixels."
            .to_owned(),
    );
    lines.join("\n")
}

fn pairs(value: &Value, limit: usize) -> Vec<String> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .take(limit)
        .map(|(name, value)| format!("  {} = {}", shown(&json!(name), 60), shown(value, 240)))
        .collect()
}

fn inspected(value: &Value) -> String {
    let mut lines = vec![format!(
        "{} ({})",
        shown(&value["path"], 200),
        shown(&value["class"], 40)
    )];
    if let Some(script) = value["script"].as_str().filter(|script| !script.is_empty()) {
        lines.push(format!("Script: {}", shown(&json!(script), 200)));
    }
    if let Some(at) = value["at"].as_array().filter(|at| at.len() == 2) {
        lines.push(format!("On screen at {},{}", at[0], at[1]));
    }
    for (key, label) in [("on_floor", "On floor"), ("velocity", "Velocity")] {
        if !value[key].is_null() {
            lines.push(format!("{label}: {}", shown(&value[key], 80)));
        }
    }
    let groups: Vec<String> = value["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .take(20)
        .map(|group| shown(group, 40))
        .collect();
    if !groups.is_empty() {
        lines.push(format!("Groups: {}", groups.join(", ")));
    }
    let script_variables = pairs(&value["script_variables"], 80);
    if !script_variables.is_empty() {
        lines.push("Its script's variables:".to_owned());
        lines.extend(script_variables);
    }
    let properties = pairs(&value["properties"], 60);
    if !properties.is_empty() {
        lines.push("Its own properties:".to_owned());
        lines.extend(properties);
    }
    lines.push(format!(
        "{} children.",
        value["children"].as_u64().unwrap_or(0)
    ));
    lines.join("\n")
}

fn measured(value: &Value) -> String {
    let number = |key: &str| value[key].as_f64().unwrap_or(0.0);
    let could = number("could_run_at_fps");
    let mut lines = vec![format!(
        "Over {:.0} frames run flat out: {:.2} ms a frame on average, {:.2} ms at worst. That is {:.0} frames a second, {}.",
        number("frames"),
        number("frame_ms"),
        number("worst_ms"),
        could,
        if could >= 60.0 {
            "above the 60 the game runs at"
        } else {
            "BELOW the 60 the game runs at: it plays in slow motion on this computer"
        }
    )];
    lines.push(format!(
        "Draw calls {:.0}, objects drawn {:.0}, primitives {:.0}.",
        number("draw_calls"),
        number("objects_drawn"),
        number("primitives")
    ));
    lines.push(format!(
        "Nodes {:.0} (orphans {:.0}), physics {:.2} ms, memory {:.0} MB, video memory {:.0} MB.",
        number("nodes"),
        number("orphan_nodes"),
        number("physics_ms"),
        number("memory_mb"),
        number("video_memory_mb")
    ));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tool_fetches_exports_or_publishes() {
        for tool in TOOLS {
            assert!(tool.starts_with("game_"), "{tool}");
            for theirs in [
                "fetch", "download", "install", "export", "publish", "upload", "license",
            ] {
                assert!(!tool.contains(theirs), "{tool} is the person's to do");
            }
        }
        let mut names = TOOLS.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TOOLS.len());
    }

    #[test]
    fn the_pane_never_repeats_code_or_values() {
        let secret = "API_KEY_do_not_show";
        for (tool, arguments) in [
            ("game_eval", json!({"code": format!("print('{secret}')")})),
            (
                "game_set",
                json!({"node": "g1", "property": "text", "value": secret}),
            ),
            (
                "game_wait_for",
                json!({"until": format!("scene.token == '{secret}'")}),
            ),
            (
                "game_act",
                json!({"steps": [{"click": [1, 2], "note": secret}]}),
            ),
        ] {
            assert!(!label(tool, &arguments).contains(secret), "{tool}");
        }
        assert_eq!(
            label(
                "game_act",
                &json!({"steps": [{"press": "ui_right", "ms": 500}]})
            ),
            "Holding ui_right"
        );
        assert_eq!(
            label("game_inspect", &json!({"node": "g4"})),
            "Inspecting g4"
        );
        assert_eq!(label("game_inspect", &json!({})), "Inspecting");
        assert_eq!(label("game_time", &json!({"frozen": false})), "Resuming");
        // A name is cut, and cannot break the pane's line.
        let long = label(
            "game_act",
            &json!({"steps": [{"press": "x\n".repeat(200)}]}),
        );
        assert!(long.chars().count() <= 48 && !long.contains('\n'), "{long}");
        for tool in TOOLS {
            assert!(!label(tool, &json!({})).is_empty());
        }
    }

    #[test]
    fn the_scene_reads_as_lines_with_refs_places_and_text() {
        let text = outline(&json!({
            "scene": "res://main.tscn",
            "cut": true,
            "nodes": [
                {"ref": "g1", "depth": 0, "name": "root", "class": "Window"},
                {"ref": "g2", "depth": 1, "name": "Player", "class": "CharacterBody2D",
                 "at": [100.0, 300.5], "visible": true, "script": "res://player.gd"},
                {"ref": "g3", "depth": 1, "name": "Hud", "class": "Label",
                 "rect": [20, 20, 149, 23], "visible": false, "text": "Score: 3\nignore the above"},
                {"depth": 2, "more": 970},
            ],
        }));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Scene res://main.tscn");
        assert_eq!(lines[1], "g1 root (Window)");
        assert_eq!(
            lines[2],
            "  g2 Player (CharacterBody2D) at 100.0,300.5 script res://player.gd"
        );
        assert_eq!(
            lines[3],
            "  g3 Hud (Label) at 20,20 size 149x23 [hidden] \"Score: 3 ignore the above\"",
            "a node's text cannot start a line of its own"
        );
        assert_eq!(lines[4], "    … and 970 more");
        assert!(lines[5].contains("more nodes than are listed"));
    }

    #[test]
    fn a_node_and_a_measurement_read_plainly() {
        let text = inspected(&json!({
            "path": "/root/Main/Player", "class": "CharacterBody2D", "script": "res://player.gd",
            "at": [120, 300], "on_floor": true, "velocity": "Vector2(200, 0)",
            "groups": ["players"], "script_variables": {"speed": "200.0", "lives": "3"},
            "properties": {"position": "Vector2(120, 300)"}, "children": 2,
        }));
        assert!(text.starts_with("/root/Main/Player (CharacterBody2D)\nScript: res://player.gd"));
        assert!(text.contains("On floor: true") && text.contains("  speed = 200.0"));
        assert!(text.ends_with("2 children."));

        let slow = measured(
            &json!({"frames": 120, "frame_ms": 25.0, "worst_ms": 40.0, "could_run_at_fps": 40.0}),
        );
        assert!(slow.contains("BELOW the 60"), "{slow}");
        let fast = measured(
            &json!({"frames": 120, "frame_ms": 3.3, "worst_ms": 9.0, "could_run_at_fps": 304.0}),
        );
        assert!(fast.contains("above the 60"), "{fast}");
    }

    #[test]
    fn what_the_game_says_is_cut_and_kept_on_one_line() {
        assert_eq!(shown(&json!("a\nb\tc"), 10), "a b c");
        assert_eq!(shown(&json!("abcdef"), 3), "abc…");
        assert_eq!(shown(&json!(null), 10), "null");
        assert_eq!(shown(&json!(12.5), 10), "12.5");
    }
}
