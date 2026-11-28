//! Ghost's game tools against a real Godot, from fetching it to playing a game with it. It needs
//! the network, a display (a virtual one will do) and a minute, so it runs only when asked:
//!
//! ```text
//! GHOST_GODOT_E2E=1 xvfb-run -a cargo test -p ghost-game --test godot -- --nocapture
//! ```
//!
//! `GHOST_GODOT_ROOT` names a folder to keep the fetched Godot in between runs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ghost_game::{Config, Fetch, GameState, InputEvent, Manager, ToolReply};
use serde_json::{Value, json};

fn copy_fixture(to: &Path) -> std::io::Result<()> {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/godot-app");
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        std::fs::copy(entry.path(), to.join(entry.file_name()))?;
    }
    Ok(())
}

async fn tool(manager: &Manager, folder: &Path, name: &str, arguments: Value) -> ToolReply {
    let reply = manager.tool("chat", folder, name, &arguments).await;
    println!(
        "--- {name} {arguments}\n{}\n({} pictures, error {})",
        reply.text,
        reply.images.len(),
        reply.error
    );
    reply
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() > 1000 && bytes.starts_with(&[0xFF, 0xD8])
}

/// The player's x, read from the running game.
async fn player_x(manager: &Manager, folder: &Path) -> f64 {
    let reply = tool(
        manager,
        folder,
        "game_eval",
        json!({"code": "scene.get_node(\"Player\").position.x"}),
    )
    .await;
    assert!(!reply.error, "{}", reply.text);
    let first = reply.text.lines().next().unwrap_or_default().trim();
    first.parse().unwrap_or(f64::NAN)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_real_godot_is_fetched_and_plays_the_fixture() {
    if std::env::var("GHOST_GODOT_E2E").as_deref() != Ok("1") {
        eprintln!("skipped: set GHOST_GODOT_E2E=1 to run it");
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    let root = std::env::var_os("GHOST_GODOT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| scratch.path().join("ghost"));
    let game = scratch.path().join("moon hop");
    copy_fixture(&game).unwrap();
    let before: Vec<_> = std::fs::read_dir(&game)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let manager = Manager::new(
        Config {
            root,
            max_running: 2,
        },
        || {},
    );

    // Nothing runs before Godot is here, and the tool says whose step that is.
    manager.refresh().await;
    if !manager.status().godot.fetched {
        let refused = tool(&manager, &game, "game_run", json!({})).await;
        assert!(
            refused.error && refused.text.contains("Game pane"),
            "{}",
            refused.text
        );
        manager.fetch_start();
        for _ in 0..600 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let status = manager.status();
            if let Fetch::Failed { message } = &status.godot.fetch {
                panic!("the fetch failed: {message}");
            }
            if status.godot.fetched {
                break;
            }
        }
    }
    let status = manager.status();
    assert!(
        status.godot.fetched,
        "Godot was fetched: {:?}",
        status.godot
    );
    assert_eq!(status.godot.fetch, Fetch::Idle);

    let said = tool(&manager, &game, "game_status", json!({})).await;
    assert!(
        said.text.contains("Ghost Fixture, made with Godot 4.7"),
        "{}",
        said.text
    );
    assert!(said.text.contains("Game: not running"), "{}", said.text);

    let checked = tool(&manager, &game, "game_check", json!({})).await;
    assert!(
        !checked.error && checked.text.starts_with("All 3 scripts and scenes load"),
        "{}",
        checked.text
    );

    // Run: a picture of the game half a second in, and then it holds still.
    let started = tool(&manager, &game, "game_run", json!({})).await;
    assert!(!started.error, "{}", started.text);
    assert!(
        started.text.contains("960x540") && started.text.contains("frozen"),
        "{}",
        started.text
    );
    assert_eq!(started.images.len(), 1);
    assert!(is_jpeg(&started.images[0]));
    let chat = manager.chat("chat").unwrap();
    assert_eq!(chat.state, GameState::Running);
    assert!(
        chat.engine_version.starts_with("4.7"),
        "{}",
        chat.engine_version
    );
    let x = player_x(&manager, &game).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(player_x(&manager, &game).await, x, "frozen: nothing moves");

    let tree = tool(&manager, &game, "game_tree", json!({})).await;
    assert!(
        tree.text.contains("Player (Polygon2D) at 100.0,300.0"),
        "{}",
        tree.text
    );
    assert!(
        tree.text.contains("Hud (Label)") && tree.text.contains("\"Score 0\""),
        "{}",
        tree.text
    );
    assert!(tree.text.contains("script res://main.gd"), "{}", tree.text);
    assert!(
        !tree.text.contains("GhostBridge"),
        "Ghost's own node is not the game's"
    );
    let player = tree
        .text
        .lines()
        .find(|line| line.contains(" Player "))
        .unwrap()
        .trim();
    let player_ref = player.split(' ').next().unwrap().to_owned();

    // An action in game time: half a second of right is exactly 100 pixels at 200 a second,
    // however long it took in real time, and the game is frozen again after it.
    let acted = tool(
        &manager,
        &game,
        "game_act",
        json!({"steps": [{"press": "ui_right", "ms": 500}]}),
    )
    .await;
    assert!(!acted.error, "{}", acted.text);
    assert!(
        acted
            .text
            .starts_with("Played 30 frames (500 ms of game time)"),
        "{}",
        acted.text
    );
    assert_eq!(acted.images.len(), 2, "a filmstrip and where it ended");
    assert!(acted.images.iter().all(|image| is_jpeg(image)));
    let moved = player_x(&manager, &game).await - x;
    assert!((moved - 100.0).abs() < 0.01, "moved {moved}");

    // The same action again moves it exactly as far: runs repeat.
    tool(
        &manager,
        &game,
        "game_act",
        json!({"steps": [{"press": "ArrowLeft", "ms": 500}], "shots": 0}),
    )
    .await;
    assert!((player_x(&manager, &game).await - x).abs() < 0.01);

    // A press the game reads once, and waiting for what it causes.
    tool(
        &manager,
        &game,
        "game_act",
        json!({"steps": [{"press": "ui_accept", "ms": 50}, {"ms": 50}], "shots": 0}),
    )
    .await;
    let waited = tool(
        &manager,
        &game,
        "game_wait_for",
        json!({"until": "root.get_node(\"Globals\").score == 1", "timeout_ms": 1000}),
    )
    .await;
    assert!(
        waited.text.starts_with("The condition became true"),
        "{}",
        waited.text
    );
    let never = tool(
        &manager,
        &game,
        "game_wait_for",
        json!({"until": "root.get_node(\"Globals\").score == 99", "timeout_ms": 300}),
    )
    .await;
    assert!(
        never
            .text
            .starts_with("The condition was still false after 3"),
        "{}",
        never.text
    );

    let inspected = tool(&manager, &game, "game_inspect", json!({"node": player_ref})).await;
    assert!(
        inspected.text.contains("/root/Main/Player (Polygon2D)"),
        "{}",
        inspected.text
    );
    let main = tool(&manager, &game, "game_inspect", json!({"node": "."})).await;
    assert!(
        main.text.contains("  speed = 200.0") && main.text.contains("  frames = "),
        "{}",
        main.text
    );

    let set = tool(
        &manager,
        &game,
        "game_set",
        json!({"node": ".", "property": "speed", "value": "400.0"}),
    )
    .await;
    assert!(set.text.contains("it is now 400.0"), "{}", set.text);
    tool(
        &manager,
        &game,
        "game_act",
        json!({"steps": [{"press": "ui_right", "ms": 250}], "shots": 0}),
    )
    .await;
    assert!(
        (player_x(&manager, &game).await - x - 100.0).abs() < 0.01,
        "250 ms at 400 a second"
    );

    let perf = tool(&manager, &game, "game_perf", json!({"frames": 60})).await;
    assert!(
        perf.text.contains("frames a second") && perf.text.contains("Nodes "),
        "{}",
        perf.text
    );
    let looked = tool(&manager, &game, "game_look", json!({})).await;
    assert!(
        looked.text.contains("frozen") && looked.images.len() == 1,
        "{}",
        looked.text
    );

    // An error in the game's own script reaches the agent with its file and line.
    let exploded = tool(
        &manager,
        &game,
        "game_eval",
        json!({"code": "scene.explode()"}),
    )
    .await;
    assert!(
        exploded.text.contains("New errors from the game"),
        "{}",
        exploded.text
    );
    assert!(
        exploded.text.contains("res://main.gd:19"),
        "{}",
        exploded.text
    );
    let logs = tool(&manager, &game, "game_logs", json!({})).await;
    assert!(
        logs.text.contains("[script]") && logs.text.contains("res://main.gd:19"),
        "{}",
        logs.text
    );
    assert!(
        logs.text.contains("fixture ready"),
        "what the game printed: {}",
        logs.text
    );
    assert_eq!(manager.chat("chat").unwrap().errors, 1);

    // Refusals say why.
    for (name, arguments, why) in [
        (
            "game_act",
            json!({"steps": [{"press": "no_such_action", "ms": 100}]}),
            "neither an input action",
        ),
        ("game_act", json!({"steps": []}), "\"steps\" is a list"),
        ("game_inspect", json!({"node": "g999"}), "No such node"),
        (
            "game_set",
            json!({"node": ".", "property": "nope", "value": "1"}),
            "has no property",
        ),
        (
            "game_run",
            json!({"scene": "../other.tscn"}),
            "inside the project",
        ),
        ("game_eval", json!({}), "\"code\" is missing"),
    ] {
        let refused = tool(&manager, &game, name, arguments).await;
        assert!(
            refused.error && refused.text.contains(why),
            "{name}: {}",
            refused.text
        );
    }
    assert_eq!(
        manager.chat("chat").unwrap().state,
        GameState::Running,
        "a refusal ends nothing"
    );

    // The pane: live, the frames come and the person's keys move the game.
    let live = tool(&manager, &game, "game_time", json!({"frozen": false})).await;
    assert!(live.text.contains("running live"), "{}", live.text);
    let mut viewer = manager.watch("chat", &game);
    let mut frames = 0;
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        let packet = tokio::time::timeout(Duration::from_secs(5), viewer.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(packet[0], 6);
        assert!(is_jpeg(&packet[1..]));
        frames += 1;
    }
    assert!(frames >= 10, "{frames} frames in two seconds");
    let from = player_x(&manager, &game).await;
    manager.input(
        "chat",
        &[InputEvent::Key {
            code: "ArrowRight".into(),
            down: true,
            text: String::new(),
            shift: false,
            ctrl: false,
            alt: false,
        }],
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    manager.input(
        "chat",
        &[InputEvent::Key {
            code: "ArrowRight".into(),
            down: false,
            text: String::new(),
            shift: false,
            ctrl: false,
            alt: false,
        }],
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let to = player_x(&manager, &game).await;
    assert!(
        to - from > 40.0 && to - from < 320.0,
        "about half a second at 400 a second: {}",
        to - from
    );
    drop(viewer);

    // Another scene by name, then stop.
    let scene = tool(
        &manager,
        &game,
        "game_run",
        json!({"scene": "res://main.tscn"}),
    )
    .await;
    assert!(!scene.error, "{}", scene.text);
    assert_eq!(
        manager.chat("chat").unwrap().errors,
        0,
        "a new run starts clean"
    );
    let stopped = tool(&manager, &game, "game_stop", json!({})).await;
    assert_eq!(stopped.text, "The game is stopped.");
    let chat = manager.chat("chat").unwrap();
    assert_eq!(
        (chat.state, chat.ended.as_deref()),
        (GameState::Ended, Some("Stopped."))
    );
    let after = tool(&manager, &game, "game_look", json!({})).await;
    assert!(
        after.error && after.text.contains("not running"),
        "{}",
        after.text
    );

    // Nothing of Ghost's was written into the project: only Godot's own cache folder is new.
    let mut now: Vec<_> = std::fs::read_dir(&game)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    now.retain(|name| name != ".godot" && !before.contains(name));
    assert!(now.is_empty(), "new in the project: {now:?}");

    // A script that does not parse is found without running the game.
    std::fs::write(
        game.join("broken.gd"),
        "extends Node\nfunc _ready():\n\tvar x = \n",
    )
    .unwrap();
    let broken = tool(&manager, &game, "game_check", json!({})).await;
    assert!(
        broken
            .text
            .starts_with("1 of 4 scripts and scenes did not load."),
        "{}",
        broken.text
    );
    assert!(
        broken.text.contains("res://broken.gd:3: Parse Error"),
        "{}",
        broken.text
    );

    // Off: nothing answers, and what ran is stopped.
    manager.set_enabled(false);
    let off = tool(&manager, &game, "game_status", json!({})).await;
    assert!(off.error && off.text.contains("turned off"), "{}", off.text);
    manager.shutdown().await;
}
