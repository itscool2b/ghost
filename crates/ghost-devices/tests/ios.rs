//! A real iPhone simulator, driven the way the Device pane and an agent drive it
//! (docs/mobile.md, "iPhone on a Mac"). It needs a Mac with Xcode, so it runs only when asked:
//!
//! ```sh
//! GHOST_IOS_E2E=1 GHOST_IOS_E2E_APP=/path/HelloGhost.app GHOST_IOS_E2E_OUT=/tmp/ios-e2e \
//!   cargo test -p ghost-devices --test ios -- --nocapture
//! ```
//!
//! The "iPhone simulator" workflow builds the app with `tests/ios-app/build.sh` and keeps what
//! this writes to `GHOST_IOS_E2E_OUT`: a transcript of every call, screenshots, and the pane's
//! pictures.

// The whole file is one test: its helpers fail it by panicking, as the test itself does.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ghost_devices::adb::Adb;
use ghost_devices::manager::{Config, DeviceKind, DeviceState, Manager, ToolReply};
use ghost_devices::scrcpy::{Touch, key};
use ghost_devices::sdk::Places;
use serde_json::{Value, json};

const CHAT: &str = "ios-e2e";

struct Run {
    manager: Manager,
    out: PathBuf,
    transcript: String,
    started: Instant,
}

impl Run {
    fn note(&mut self, line: &str) {
        let line = format!("[{:>6.1}s] {line}", self.started.elapsed().as_secs_f64());
        println!("{line}");
        self.transcript.push_str(&line);
        self.transcript.push('\n');
        let _ = std::fs::write(self.out.join("transcript.txt"), &self.transcript);
    }

    /// Runs one of the agents' tools and writes down what it answered.
    async fn tool(&mut self, tool: &str, arguments: Value) -> ToolReply {
        let began = Instant::now();
        // Boxed: a debug build gives every awaited future room in the caller's frame.
        let reply = Box::pin(self.manager.tool(CHAT, tool, &arguments)).await;
        let mut line = format!(
            "{tool} {arguments} -> {} in {:.1}s{}",
            if reply.error { "ERROR" } else { "ok" },
            began.elapsed().as_secs_f64(),
            reply.image.as_ref().map_or(String::new(), |png| format!(
                ", a {} byte picture",
                png.len()
            )),
        );
        for text in reply.text.lines().take(60) {
            let _ = write!(line, "\n    {text}");
        }
        self.note(&line);
        reply
    }

    /// A tool that must succeed.
    async fn ok(&mut self, tool: &str, arguments: Value) -> String {
        let reply = self.tool(tool, arguments).await;
        assert!(!reply.error, "{tool} failed: {}", reply.text);
        reply.text
    }

    /// A tool that must succeed within `tries` calls, called again when it fails, as an agent
    /// would: GitHub's virtual Macs sometimes take a minute over what a real Mac does in a second.
    async fn ok_after(&mut self, tries: u32, tool: &str, arguments: Value) -> String {
        for attempt in 1..=tries {
            let reply = self.tool(tool, arguments.clone()).await;
            if !reply.error {
                return reply.text;
            }
            assert!(
                attempt < tries,
                "{tool} failed {tries} times: {}",
                reply.text
            );
            if tool == "mobile_wait_for" {
                // What the screen showed instead, for the record.
                self.tool("mobile_snapshot", json!({})).await;
            }
        }
        unreachable!("the last try returns or fails")
    }

    /// Waits for `text` to show, or with `gone` to go: up to three waits of a minute.
    async fn wait(&mut self, text: &str, gone: bool) -> String {
        self.ok_after(
            3,
            "mobile_wait_for",
            json!({ "text": text, "gone": gone, "timeout_seconds": 60 }),
        )
        .await
    }

    /// A tool that must be refused, saying `why`.
    async fn refused(&mut self, tool: &str, arguments: Value, why: &str) {
        let reply = self.tool(tool, arguments).await;
        assert!(
            reply.error,
            "{tool} should have been refused: {}",
            reply.text
        );
        assert!(reply.text.contains(why), "{tool}: {}", reply.text);
    }

    /// Starts an app; a simulator still settling after its first boot gets a second try, as an
    /// agent would give it.
    async fn launch(&mut self, bundle: &str) {
        let first = self
            .tool("mobile_launch", json!({ "package": bundle }))
            .await;
        if first.error {
            tokio::time::sleep(Duration::from_secs(5)).await;
            self.ok("mobile_launch", json!({ "package": bundle })).await;
        }
    }

    async fn screenshot(&mut self, name: &str) {
        match Box::pin(self.manager.screenshot(CHAT)).await {
            Ok(png) => {
                let _ = std::fs::write(self.out.join(format!("{name}.png")), &png);
                self.note(&format!("screenshot {name}.png, {} bytes", png.len()));
            }
            Err(error) => self.note(&format!("screenshot {name} failed: {error}")),
        }
    }
}

/// How bright the top of a screenshot is, 0 to 255: the screen's background below the status
/// bar, which dark mode turns black.
fn brightness(png: &[u8]) -> Option<f64> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0u8; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut pixels).ok()?;
    let samples = frame.color_type.samples();
    let (width, height) = (frame.width as usize, frame.height as usize);
    let (mut sum, mut count) = (0u64, 0u64);
    for y in (height / 10..height / 5).step_by(4) {
        for x in (0..width).step_by(8) {
            let at = y * frame.line_size + x * samples;
            let rgb = pixels.get(at..at + 3)?;
            sum += rgb.iter().map(|value| u64::from(*value)).sum::<u64>() / 3;
            count += 1;
        }
    }
    (count > 0).then(|| sum as f64 / count as f64)
}

/// The ref of the first line of an outline that names `name`.
fn ref_of(outline: &str, name: &str) -> Option<String> {
    outline
        .lines()
        .find(|line| line.contains(&format!("\"{name}\"")) && line.contains("[d"))
        .and_then(|line| {
            let start = line.find("[d")? + 1;
            let end = start + line[start..].find(']')?;
            Some(line[start..end].to_owned())
        })
}

/// The ref of the first line with `role`.
fn ref_of_role(outline: &str, role: &str) -> Option<String> {
    outline
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("- {role}")) && line.contains("[d"))
        .and_then(|line| {
            let start = line.find("[d")? + 1;
            let end = start + line[start..].find(']')?;
            Some(line[start..end].to_owned())
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_iphone_simulator_does_what_the_pane_and_the_agent_ask() {
    if std::env::var("GHOST_IOS_E2E").as_deref() != Ok("1") {
        eprintln!("Skipped: set GHOST_IOS_E2E=1 on a Mac with Xcode to drive a real simulator.");
        return;
    }
    // The whole run waits on many calls in turn; on the heap, it cannot outgrow a test's stack.
    Box::pin(drive()).await;
}

async fn drive() {
    let out = PathBuf::from(
        std::env::var("GHOST_IOS_E2E_OUT").unwrap_or_else(|_| "target/ios-e2e".into()),
    );
    std::fs::create_dir_all(&out).unwrap();
    let app = PathBuf::from(
        std::env::var("GHOST_IOS_E2E_APP").expect("GHOST_IOS_E2E_APP names the test app"),
    );
    let root = out.join("data");
    let manager = Manager::new(
        Config {
            root,
            places: Places::current(),
            adb: Adb::from_env(),
            max_running: 2,
        },
        || {},
    );
    let mut run = Run {
        manager: manager.clone(),
        out: out.clone(),
        transcript: String::new(),
        started: Instant::now(),
    };

    // What this Mac has.
    Box::pin(manager.refresh()).await;
    let setup = manager.status().setup.ios;
    run.note(&format!("setup: {setup:?}"));
    assert!(
        setup.possible && setup.ready,
        "Xcode was not found ready: {setup:?}"
    );

    let devices = run.ok("mobile_devices", json!({})).await;
    assert!(
        devices.contains("An iPhone simulator can run here"),
        "{devices}"
    );

    // What the manager says while things happen, so a slow or stuck step shows where it is.
    let watcher = {
        let manager = manager.clone();
        let began = run.started;
        tokio::spawn(async move {
            let mut last = String::new();
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let status = manager.status();
                let line = format!(
                    "devices {:?} | chats {:?} | AXe {:?}",
                    status
                        .devices
                        .iter()
                        .map(|device| (&device.id, device.state, &device.error))
                        .collect::<Vec<_>>(),
                    status
                        .chats
                        .iter()
                        .map(|chat| (chat.starting, chat.live, &chat.error))
                        .collect::<Vec<_>>(),
                    status.setup.ios.tool,
                );
                if line != last {
                    println!("[{:>6.1}s] status: {line}", began.elapsed().as_secs_f64());
                    last = line;
                }
            }
        })
    };

    // The first start fetches AXe, makes the simulator and boots it. A first boot on a fresh Mac
    // can outlast one call, which then says to call again, as an agent would.
    let mut started = String::new();
    for attempt in 1..=5 {
        let reply = run.tool("mobile_start", json!({ "platform": "ios" })).await;
        if !reply.error {
            started = reply.text;
            break;
        }
        // "Still starting", or a start that failed: either way an agent calls again.
        assert!(attempt < 5, "mobile_start failed: {}", reply.text);
    }
    assert!(
        started.starts_with("iPhone 1 is ready: iPhone"),
        "{started}"
    );
    let udid = started
        .split("Its UDID is ")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .to_owned();
    assert_eq!(udid.len(), 36, "{started}");
    let status = manager.status();
    let held = status
        .chats
        .iter()
        .find(|held| held.chat_id == CHAT)
        .unwrap();
    assert_eq!(held.kind, DeviceKind::Simulator);
    assert!(held.live && held.agent_allowed);
    assert!(matches!(
        status.setup.ios.tool,
        ghost_devices::manager::ToolFetch::Ready
    ));
    assert!(
        ghost_devices::axe::installed(&out.join("data")).is_some(),
        "AXe was fetched"
    );

    // The pane's pictures: a size, then whole JPEGs, several a second.
    let mut viewer = Box::pin(manager.watch(CHAT)).await.unwrap();
    let (mut size, mut pictures, began) = (None, 0usize, Instant::now());
    while pictures < 8 && began.elapsed() < Duration::from_secs(90) {
        let Ok(Some(packet)) = tokio::time::timeout(Duration::from_secs(40), viewer.next()).await
        else {
            break;
        };
        match packet.first() {
            Some(1) if packet.len() == 5 => {
                size = Some((
                    u16::from_be_bytes([packet[1], packet[2]]),
                    u16::from_be_bytes([packet[3], packet[4]]),
                ));
            }
            Some(6) => {
                pictures += 1;
                assert_eq!(&packet[1..3], [0xff, 0xd8], "a JPEG");
                if pictures == 1 || pictures == 2 {
                    std::fs::write(out.join(format!("pane-{pictures}.jpg")), &packet[1..]).unwrap();
                }
            }
            _ => {}
        }
    }
    let rate = pictures as f64 / began.elapsed().as_secs_f64();
    run.note(&format!(
        "pane: {pictures} pictures in {:.1}s ({rate:.1} a second), size {size:?}",
        began.elapsed().as_secs_f64()
    ));
    assert!(pictures >= 2, "the pane got {pictures} pictures");
    let (width, height) = size.expect("the pane was told the picture's size");
    assert!(height > width && height <= 1500, "{width} by {height}");
    drop(viewer);
    // Nobody watches now: AXe's stream of pictures has stopped.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let streaming = std::process::Command::new("pgrep")
        .args(["-f", &format!("stream-video --udid {udid}")])
        .output()
        .unwrap();
    assert!(
        !streaming.status.success(),
        "the stream still runs: {}",
        String::from_utf8_lossy(&streaming.stdout)
    );
    run.note("the pane's pictures stopped with the last viewer");

    // The agent's look at the home screen. The first picture after a boot can be slow to come.
    let mut shot = run.tool("mobile_screenshot", json!({})).await;
    if shot.error {
        shot = run.tool("mobile_screenshot", json!({})).await;
    }
    assert!(
        !shot.error
            && shot
                .image
                .as_ref()
                .is_some_and(|png| png.starts_with(b"\x89PNG")),
        "{}",
        shot.text
    );
    std::fs::write(out.join("agent-home.png"), shot.image.unwrap()).unwrap();
    run.screenshot("home").await;
    let home = run.ok_after(2, "mobile_snapshot", json!({})).await;
    assert!(home.starts_with("Screen "), "{home}");

    // Refusals that are an iPhone's own.
    run.refused(
        "mobile_settings",
        json!({ "orientation": "landscape" }),
        "cannot be turned",
    )
    .await;
    run.refused("mobile_press", json!({ "key": "volume_up" }), "no such key")
        .await;
    run.refused(
        "mobile_launch",
        json!({ "package": "com.apple.Preferences", "activity": ".Main" }),
        "no activities",
    )
    .await;
    run.refused(
        "mobile_install",
        json!({ "path": "/tmp/App.ipa" }),
        "real iPhone",
    )
    .await;
    run.refused(
        "mobile_launch",
        json!({ "package": "--set /tmp" }),
        "bundle id",
    )
    .await;

    // Settings: open it, go into General by its ref, and back with the edge swipe.
    run.launch("com.apple.Preferences").await;
    let settings = run.wait("General", false).await;
    run.screenshot("settings").await;
    let general = ref_of(&settings, "General").expect("General has a ref");
    run.ok("mobile_tap", json!({ "ref": general })).await;
    run.wait("About", false).await;
    run.screenshot("general").await;
    run.ok("mobile_press", json!({ "key": "back" })).await;
    // A swipe a slow Mac drew late may not have counted as one: an agent tries again.
    let back = run
        .tool(
            "mobile_wait_for",
            json!({ "text": "About", "gone": true, "timeout_seconds": 30 }),
        )
        .await;
    if back.error {
        run.ok("mobile_press", json!({ "key": "back" })).await;
        run.wait("About", true).await;
    }
    // A system app's log, for the record: its program may not be one simctl names.
    run.tool("mobile_logs", json!({ "lines": 20 })).await;

    // Dark and large, then back.
    run.ok(
        "mobile_settings",
        json!({ "appearance": "dark", "text_scale": 1.3 }),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    run.screenshot("settings-dark-large").await;
    let dark = Box::pin(manager.screenshot(CHAT))
        .await
        .ok()
        .and_then(|png| brightness(&png));
    run.note(&format!("brightness in dark mode: {dark:?}"));
    assert!(
        dark.is_some_and(|level| level < 90.0),
        "dark mode did not show: {dark:?}"
    );
    run.ok(
        "mobile_settings",
        json!({ "appearance": "light", "text_scale": 1.0 }),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let light = Box::pin(manager.screenshot(CHAT))
        .await
        .ok()
        .and_then(|png| brightness(&png));
    run.note(&format!("brightness in light mode: {light:?}"));
    assert!(
        light.is_some_and(|level| level > 160.0),
        "light mode did not come back: {light:?}"
    );

    // A web page in Safari, for the record: how fast it loads is the network's and the runner's.
    run.ok("mobile_open_link", json!({ "link": "https://example.com" }))
        .await;
    let page = run
        .tool(
            "mobile_wait_for",
            json!({ "text": "example.com", "timeout_seconds": 60 }),
        )
        .await;
    run.screenshot("safari").await;
    run.note(if page.error {
        "Safari had not shown the page yet"
    } else {
        "Safari showed the page"
    });

    // The test app: install, launch, tap, type, read its log.
    run.ok("mobile_install", json!({ "path": app })).await;
    run.launch("dev.onghost.e2e").await;
    let screen = run.wait("Say hello", false).await;
    let hello = ref_of(&screen, "Say hello").expect("the button has a ref");
    run.ok("mobile_tap", json!({ "ref": hello })).await;
    // The wait's reply is the screen as read last, which the next refs come from.
    let screen = run.wait("Hello from Ghost", false).await;
    let field = ref_of_role(&screen, "textbox").expect("the field has a ref");
    run.ok(
        "mobile_type",
        json!({ "ref": field, "text": "Ada", "submit": true }),
    )
    .await;
    run.wait("Typed Ada", false).await;
    run.screenshot("app-typed").await;
    // Text the keyboard has no keys for is pasted.
    let screen = run.ok("mobile_snapshot", json!({})).await;
    let field = ref_of_role(&screen, "textbox").expect("the field has a ref");
    run.ok(
        "mobile_type",
        json!({ "ref": field, "text": "日本 ok", "clear": true, "submit": true }),
    )
    .await;
    run.wait("Typed 日本 ok", false).await;
    let logs = run
        .ok("mobile_logs", json!({ "package": "dev.onghost.e2e" }))
        .await;
    // The app's NSLog line, from the log or its standard error, and what it printed.
    run.note(&format!(
        "log: NSLog line {}, printed line {}",
        logs.contains("ghost-e2e: tapped"),
        logs.contains("ghost-e2e out: tapped")
    ));
    assert!(logs.contains("ghost-e2e out: tapped"), "{logs}");
    assert!(logs.contains("ghost-e2e: tapped"), "{logs}");
    run.ok(
        "mobile_settings",
        json!({ "permission": { "package": "dev.onghost.e2e", "name": "photos", "granted": true } }),
    )
    .await;
    run.refused(
        "mobile_settings",
        json!({ "permission": { "package": "dev.onghost.e2e", "name": "android.permission.CAMERA" } }),
        "permission is one of",
    )
    .await;
    // One of the app's own links, which needs no network: the app says what it was opened with.
    // iOS may first ask whether to open the app, as it asks a person; the agent taps Open.
    let reply = run
        .ok("mobile_open_link", json!({ "link": "ghoste2e://hello" }))
        .await;
    assert!(reply.contains("tap Open if it asks"), "{reply}");
    let mut opened = false;
    for _ in 0..12 {
        let screen = run.ok_after(2, "mobile_snapshot", json!({})).await;
        if screen.contains("Opened ghoste2e://hello") {
            opened = true;
            break;
        }
        if screen.contains("Open in")
            && let Some(open) = ref_of(&screen, "Open")
        {
            run.ok("mobile_tap", json!({ "ref": open })).await;
            run.note("iOS asked before the link opened the app, and Open was tapped");
            continue;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    assert!(opened, "the app never said the link opened it");

    // The person's own input, through the calls the pane makes.
    run.ok("mobile_stop_app", json!({ "package": "dev.onghost.e2e" }))
        .await;
    run.launch("dev.onghost.e2e").await;
    run.wait("Waiting", false).await;
    let (x, y) = (u32::from(width) / 2, u32::from(height) * 3 / 10);
    Box::pin(manager.touch(CHAT, Touch::Down, x, y))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    Box::pin(manager.touch(CHAT, Touch::Up, x, y))
        .await
        .unwrap();
    run.wait("Hello from Ghost", false).await;
    run.note("a click on the pane's picture tapped the button");
    let screen = run.ok("mobile_snapshot", json!({})).await;
    let field = ref_of_role(&screen, "textbox").expect("the field has a ref");
    run.ok("mobile_tap", json!({ "ref": field })).await;
    for letter in ["Z", "e", "d"] {
        Box::pin(manager.text(CHAT, letter)).await.unwrap();
    }
    Box::pin(manager.press(CHAT, key::ENTER)).await.unwrap();
    run.wait("Typed Zed", false).await;
    run.note("keys typed on the pane reached the field in order");
    // A drag is a swipe; the wheel is short swipes. Neither may fail.
    Box::pin(manager.touch(CHAT, Touch::Down, x, u32::from(height) * 8 / 10))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    Box::pin(manager.touch(CHAT, Touch::Up, x, u32::from(height) * 6 / 10))
        .await
        .unwrap();
    Box::pin(manager.scroll(CHAT, x, u32::from(height) / 2, 0.0, -2.0))
        .await
        .unwrap();
    // Home leaves the app.
    Box::pin(manager.press(CHAT, key::HOME)).await.unwrap();
    run.wait("Say hello", true).await;
    run.screenshot("home-again").await;

    // Stopping shuts the simulator down, and quitting leaves none running.
    let listed = run.ok("mobile_devices", json!({})).await;
    assert!(
        listed.contains("iPhone 1 (iPhone simulator, id o7_iphone_1): running, this chat's device"),
        "{listed}"
    );
    run.ok("mobile_stop", json!({})).await;
    let status = manager.status();
    assert!(status.chats.is_empty());
    let sim = status
        .devices
        .iter()
        .find(|device| device.id == "o7_iphone_1")
        .unwrap();
    assert_eq!(sim.state, DeviceState::Stopped);
    // A second start is the same simulator, booted again.
    let again = run.ok("mobile_start", json!({ "platform": "ios" })).await;
    assert!(again.contains(&udid), "the same simulator: {again}");
    Box::pin(manager.shutdown()).await;
    assert!(
        manager
            .status()
            .devices
            .iter()
            .all(|device| device.state == DeviceState::Stopped)
    );
    watcher.abort();
    run.note("done");
}
