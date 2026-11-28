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

use ghost_game::{Config, GameInput, GameState, GodotFetch, Manager, Starter, ToolReply};
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
            if let GodotFetch::Failed { message } = &status.godot.fetch {
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
    assert_eq!(status.godot.fetch, GodotFetch::Idle);

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
        &[GameInput::Key {
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
        &[GameInput::Key {
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

    // Point: the node under a place in the picture, and where it is defined.
    let at = player_x(&manager, &game).await as f32;
    let picked = manager
        .pick("chat", &game, at, 300.0)
        .await
        .unwrap()
        .expect("the player is under the pointer");
    assert_eq!(
        (
            picked.name.as_str(),
            picked.class.as_str(),
            picked.path.as_str()
        ),
        ("Player", "Polygon2D", "/root/Main/Player")
    );
    assert_eq!(
        (
            picked.scene.as_str(),
            picked.in_scene.as_str(),
            picked.scene_line
        ),
        ("res://main.tscn", "Player", Some(8))
    );
    assert_eq!(
        (picked.script.as_str(), picked.script_on.as_str()),
        ("res://main.gd", "/root/Main")
    );
    assert!(
        (picked.area[2] - 40.0).abs() < 1.0 && (picked.area[0] - (at - 20.0)).abs() < 1.5,
        "{:?} with the player at {at}",
        picked.area
    );
    let label = manager
        .pick("chat", &game, 24.0, 28.0)
        .await
        .unwrap()
        .expect("the score is under the pointer");
    assert_eq!(
        (label.name.as_str(), label.class.as_str()),
        ("Hud", "Label")
    );
    assert!(label.text.starts_with("Score "), "{}", label.text);
    assert!(
        manager
            .pick("chat", &game, 600.0, 500.0)
            .await
            .unwrap()
            .is_none(),
        "nothing is drawn there"
    );
    assert!(manager.pick("chat", &game, f32::NAN, 0.0).await.is_err());

    // Tune: what the script exports, turned while the game runs.
    let speed_of = |tunables: &ghost_game::GameTunables| {
        tunables
            .values
            .iter()
            .find(|value| value.name == "speed")
            .cloned()
            .expect("speed is exported")
    };
    let tunables = manager.tunables("chat", ".").await.unwrap();
    assert_eq!(
        (tunables.script.as_str(), tunables.path.as_str()),
        ("res://main.gd", "/root/Main")
    );
    let speed = speed_of(&tunables);
    assert_eq!((speed.kind.as_str(), speed.initial), ("float", Some(200.0)));
    assert!(
        tunables.values.iter().all(|value| value.name != "frames"),
        "what a script does not export is not offered"
    );
    manager
        .set_value("chat", ".", "speed", "float", 300.0)
        .await
        .unwrap();
    assert_eq!(
        speed_of(&manager.tunables("chat", ".").await.unwrap()).value,
        300.0
    );
    assert!(
        manager
            .set_value("chat", ".", "speed = 1; x", "float", 1.0)
            .await
            .is_err(),
        "a value's name is a name"
    );
    assert!(
        manager
            .set_value("chat", ".", "speed", "text", 1.0)
            .await
            .is_err()
    );

    // The last seconds, and the run again. A fresh run is set up by an agent, played by its
    // action and then by the person's own keys, kept, and replayed: the replay ends in the
    // very state the first run did, to the frame.
    let again = tool(&manager, &game, "game_run", json!({})).await;
    assert!(!again.error, "{}", again.text);
    let set = tool(
        &manager,
        &game,
        "game_set",
        json!({"node": ".", "property": "speed", "value": "330"}),
    )
    .await;
    assert!(!set.error, "{}", set.text);
    let acted = tool(
        &manager,
        &game,
        "game_act",
        json!({"steps": [
            {"press": "ui_right", "ms": 400},
            {"press": "ui_accept", "ms": 50},
            {"press": "ui_left", "ms": 100},
        ]}),
    )
    .await;
    assert!(!acted.error, "{}", acted.text);
    tool(&manager, &game, "game_time", json!({"frozen": false})).await;
    let key = |down| GameInput::Key {
        code: "ArrowRight".into(),
        down,
        text: String::new(),
        shift: false,
        ctrl: false,
        alt: false,
    };
    manager.input("chat", &[key(true)]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    manager.input("chat", &[key(false)]);
    tokio::time::sleep(Duration::from_millis(120)).await;
    tool(&manager, &game, "game_time", json!({"frozen": true})).await;
    // A frame for the freeze to take hold: the game stops between steps, never inside one.
    tokio::time::sleep(Duration::from_millis(120)).await;
    async fn state(manager: &Manager, folder: &Path) -> String {
        let reply = tool(
            manager,
            folder,
            "game_eval",
            json!({"code": "[scene.frames, Globals.score, scene.get_node(\"Player\").position.x, scene.speed]"}),
        )
        .await;
        assert!(!reply.error, "{}", reply.text);
        reply.text.lines().next().unwrap_or_default().to_owned()
    }
    let first = state(&manager, &game).await;
    let recent = manager.recent("chat", 15.0).await.unwrap();
    assert!(is_jpeg(&recent.picture));
    assert!(recent.unrepeatable.is_none(), "{:?}", recent.unrepeatable);
    assert!(recent.inputs >= 8, "{} inputs", recent.inputs);
    assert!(
        recent.run_seconds > 1.0 && recent.seconds <= recent.run_seconds + 0.01,
        "{} s of a {} s run",
        recent.seconds,
        recent.run_seconds
    );
    let name = recent.replay.expect("the run is kept");
    assert_eq!(name, "r1");
    let replayed = tool(&manager, &game, "game_replay", json!({"replay": name})).await;
    assert!(!replayed.error, "{}", replayed.text);
    assert_eq!(
        replayed.images.len(),
        2,
        "its last stretch and its last frame"
    );
    assert!(replayed.images.iter().all(|image| is_jpeg(image)));
    assert!(
        replayed.text.contains("frozen where the replay ends"),
        "{}",
        replayed.text
    );
    assert_eq!(
        state(&manager, &game).await,
        first,
        "the replay ends where the run did"
    );
    // And an earlier moment of it: a fifth of a second in, twelve steps.
    let early = tool(
        &manager,
        &game,
        "game_replay",
        json!({"replay": "r1", "until_s": 0.2}),
    )
    .await;
    assert!(!early.error, "{}", early.text);
    let early_state = state(&manager, &game).await;
    assert!(early_state.starts_with("[12, 0, "), "{early_state}");
    let unknown = tool(&manager, &game, "game_replay", json!({"replay": "r9"})).await;
    assert!(
        unknown.error && unknown.text.contains("Kept: r1"),
        "{}",
        unknown.text
    );

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

    // The games Ghost starts from nothing load and play on this Godot, with the same tools: a
    // player that stands on its floor, walks right and jumps, and a log with no errors.
    for (starter, name, step, up) in [
        (Starter::Godot2d, "Sky Hop", 100.0, -1.0),
        (Starter::Godot3d, "Sky Hop 3D", 1.0, 1.0),
    ] {
        let folder = scratch.path().join(name);
        std::fs::create_dir(&folder).unwrap();
        let project = manager
            .start_project("starter", &folder, starter)
            .await
            .unwrap();
        assert_eq!(project.name.as_deref(), Some(name));
        assert_eq!(project.version.as_deref(), Some("4.7"));
        // A second press writes over nothing.
        assert!(
            manager
                .start_project("starter", &folder, starter)
                .await
                .is_err()
        );
        let call = async |tool: &str, arguments: Value| {
            let reply = manager.tool("starter", &folder, tool, &arguments).await;
            println!(
                "--- [{name}] {tool} {arguments}\n{}\n({} pictures, error {})",
                reply.text,
                reply.images.len(),
                reply.error
            );
            reply
        };
        let number = async |code: &str| -> f64 {
            let reply = call("game_eval", json!({ "code": code })).await;
            assert!(!reply.error, "{}", reply.text);
            reply
                .text
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .parse()
                .unwrap_or(f64::NAN)
        };
        let checked = call("game_check", json!({})).await;
        assert!(checked.text.starts_with("All "), "{}", checked.text);
        let started = call("game_run", json!({})).await;
        assert!(
            !started.error && started.images.len() == 1,
            "{}",
            started.text
        );
        let landed = call(
            "game_wait_for",
            json!({ "until": "scene.get_node(\"Player\").is_on_floor()", "timeout_ms": 3000 }),
        )
        .await;
        assert!(
            landed.text.starts_with("The condition became true"),
            "{}",
            landed.text
        );
        let x0 = number("scene.get_node(\"Player\").position.x").await;
        let walked = call(
            "game_act",
            json!({ "steps": [{ "press": "ui_right", "ms": 500 }] }),
        )
        .await;
        assert!(!walked.error && walked.images.len() == 2, "{}", walked.text);
        let x1 = number("scene.get_node(\"Player\").position.x").await;
        assert!(x1 - x0 >= step, "{name} walked from {x0} to {x1}");
        let y0 = number("scene.get_node(\"Player\").position.y").await;
        let jumped = call(
            "game_act",
            json!({ "steps": [{ "press": "ui_accept", "ms": 50 }, { "ms": 150 }], "shots": 0 }),
        )
        .await;
        assert!(!jumped.error, "{}", jumped.text);
        let y1 = number("scene.get_node(\"Player\").position.y").await;
        assert!((y1 - y0) * up > 0.05, "{name} jumped from {y0} to {y1}");
        let log = call("game_logs", json!({})).await;
        assert!(
            log.text.starts_with("No errors or warnings this run."),
            "{}",
            log.text
        );
        let stopped = call("game_stop", json!({})).await;
        assert!(!stopped.error);
    }

    // Off: nothing answers, and what ran is stopped.
    manager.set_enabled(false);
    let off = tool(&manager, &game, "game_status", json!({})).await;
    assert!(off.error && off.text.contains("turned off"), "{}", off.text);
    manager.shutdown().await;
}
