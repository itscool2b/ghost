//! The tools agents get (`o7_mobile`), each run on the calling chat's own device.
//!
//! What a device shows is untrusted: replies say what is on the screen and never act on it. The
//! text an agent types is not logged and not shown in the pane.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::android::{Appearance, Orientation};
use crate::manager::{
    ANY_EMULATOR, ANY_IPHONE, DeviceKind, DeviceState, Live, Manager, NOT_SET_UP, TURNED_OFF,
    ToolReply,
};
use crate::outline::Item;
use crate::scrcpy::key;
use crate::{Error, picture};

/// Every tool, in the order they are listed to agents.
pub const TOOLS: [&str; 16] = [
    "mobile_devices",
    "mobile_start",
    "mobile_install",
    "mobile_launch",
    "mobile_stop_app",
    "mobile_snapshot",
    "mobile_tap",
    "mobile_type",
    "mobile_swipe",
    "mobile_press",
    "mobile_screenshot",
    "mobile_logs",
    "mobile_open_link",
    "mobile_settings",
    "mobile_wait_for",
    "mobile_stop",
];

/// How long `mobile_start` waits for a device (a first start takes about a minute).
const START: Duration = Duration::from_secs(200);
/// How long the screen is given to settle after an action before the reply is sent.
const SETTLE: Duration = Duration::from_millis(350);
/// How long a change of appearance, text size or turn takes to be drawn: a screenshot taken at
/// once after a quicker reply still showed the old look (24 November 2026).
const REDRAW: Duration = Duration::from_millis(1500);

/// What an agent is told when it asks for a device and Android is not there: the pane has opened
/// beside the chat with the setup, which only the person may accept.
const SET_UP_IN_THE_PANE: &str = "Android is not set up on this computer yet. o7 opened the Device pane beside the chat with Android's setup: ask the person to press Accept and download there (Google's Android SDK licence; about 2.5 GB to download, 8 GB on disk), then call mobile_start again.";

fn refused(text: impl Into<String>) -> Error {
    Error::Refused(text.into())
}

fn text_arg<'a>(arguments: &'a Value, name: &str) -> Option<&'a str> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn flag(arguments: &Value, name: &str) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn number(arguments: &Value, name: &str) -> Option<f64> {
    arguments
        .get(name)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

/// What the pane says the agent is doing. Never the text being typed, a link or a path.
pub(crate) fn label(tool: &str, arguments: &Value) -> String {
    let reference = text_arg(arguments, "ref").map(crate::shell::shown);
    match tool {
        "mobile_start" => "Starting the device".into(),
        "mobile_install" => "Installing the app".into(),
        "mobile_launch" => "Starting the app".into(),
        "mobile_stop_app" => "Stopping the app".into(),
        "mobile_snapshot" => "Reading the screen".into(),
        "mobile_tap" => match reference {
            Some(reference) => format!("Tapping {reference}"),
            None => "Tapping".into(),
        },
        "mobile_type" => "Typing".into(),
        "mobile_swipe" => match text_arg(arguments, "direction") {
            Some(direction @ ("up" | "down" | "left" | "right")) => format!("Swiping {direction}"),
            _ => "Swiping".into(),
        },
        "mobile_press" => match text_arg(arguments, "key").and_then(named_key) {
            Some((name, _)) => format!("Pressing {name}"),
            None => "Pressing a key".into(),
        },
        "mobile_screenshot" => "Taking a screenshot".into(),
        "mobile_logs" => "Reading the log".into(),
        "mobile_open_link" => "Opening a link".into(),
        "mobile_settings" => "Changing a setting".into(),
        "mobile_wait_for" => "Waiting for the screen".into(),
        "mobile_stop" => "Stopping the device".into(),
        _ => "Using the device".into(),
    }
}

/// A key by the name agents use, as the name shown and Android's key code.
fn named_key(name: &str) -> Option<(&'static str, u32)> {
    Some(
        match name.to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "back" => ("Back", key::BACK),
            "home" => ("Home", key::HOME),
            "recents" | "app_switch" | "overview" => ("Recents", key::APP_SWITCH),
            "enter" | "return" => ("Enter", key::ENTER),
            "tab" => ("Tab", key::TAB),
            "escape" | "esc" => ("Escape", key::ESCAPE),
            "delete" | "backspace" => ("Delete", key::DELETE_BACK),
            "forward_delete" => ("Forward delete", key::DELETE_FORWARD),
            "space" => ("Space", key::SPACE),
            "up" => ("Up", key::DPAD_UP),
            "down" => ("Down", key::DPAD_DOWN),
            "left" => ("Left", key::DPAD_LEFT),
            "right" => ("Right", key::DPAD_RIGHT),
            "page_up" => ("Page up", key::PAGE_UP),
            "page_down" => ("Page down", key::PAGE_DOWN),
            "menu" => ("Menu", key::MENU),
            "volume_up" => ("Volume up", key::VOLUME_UP),
            "volume_down" => ("Volume down", key::VOLUME_DOWN),
            "power" => ("Power", key::POWER),
            _ => return None,
        },
    )
}

/// Runs `tool` for `chat_id`, and says in the pane what the agent is doing meanwhile.
pub(crate) async fn call(
    manager: &Manager,
    chat_id: &str,
    tool: &str,
    arguments: &Value,
) -> ToolReply {
    if !manager.enabled() {
        return ToolReply::failed(TURNED_OFF);
    }
    if !TOOLS.contains(&tool) {
        return ToolReply::failed(format!(
            "There is no tool called {}.",
            crate::shell::shown(tool)
        ));
    }
    // Until the pane was opened nothing had been looked at, and Android was reported missing.
    manager.look_once().await;
    if !matches!(tool, "mobile_devices" | "mobile_stop") {
        // The pane opens beside the chat while the agent works on the device, even before it
        // has one: when Android is not set up, the pane shows its setup.
        manager.summon(chat_id);
        manager.agent_doing(chat_id, Some(label(tool, arguments)));
    }
    let reply = run(manager, chat_id, tool, arguments).await;
    manager.agent_doing(chat_id, None);
    reply.unwrap_or_else(|error| ToolReply::failed(error.to_string()))
}

async fn run(
    manager: &Manager,
    chat_id: &str,
    tool: &str,
    arguments: &Value,
) -> Result<ToolReply, Error> {
    match tool {
        "mobile_devices" => return Ok(ToolReply::text(devices(manager, chat_id))),
        "mobile_start" => return start(manager, chat_id, arguments).await,
        "mobile_stop" => {
            let status = manager.status();
            let held = status.chats.iter().find(|held| held.chat_id == chat_id);
            return match held {
                None => Ok(ToolReply::text("This chat has no device.")),
                Some(held) if held.kind != DeviceKind::Phone => {
                    manager.stop_device(&held.device_id).await?;
                    Ok(ToolReply::text(format!("{} is stopped.", held.title)))
                }
                Some(_) => {
                    manager.detach(chat_id);
                    Ok(ToolReply::text(
                        "This chat let go of the phone. It is still on and plugged in.",
                    ))
                }
            };
        }
        _ => {}
    }
    let live = manager.live_for_agent(chat_id)?;
    let iphone = live.iphone().is_some();
    match tool {
        "mobile_snapshot" => snapshot(&live).await,
        "mobile_tap" => tap(&live, arguments).await,
        "mobile_type" => type_text(&live, arguments).await,
        "mobile_swipe" => swipe(&live, arguments).await,
        "mobile_press" => {
            let name = text_arg(arguments, "key").ok_or_else(|| refused("Name a key to press."))?;
            let (shown, code) = named_key(name).ok_or_else(|| {
                refused("That is not a key. Use back, home, recents, enter, tab, escape, delete, space, up, down, left, right, page_up, page_down, menu, volume_up, volume_down or power.")
            })?;
            live.press_key(code).await?;
            tokio::time::sleep(SETTLE).await;
            Ok(ToolReply::text(if iphone && code == key::BACK {
                "Swiped in from the left edge, which is Back on an iPhone.".to_owned()
            } else {
                format!("Pressed {shown}.")
            }))
        }
        "mobile_screenshot" => {
            let png = live.screenshot().await?;
            let picture = tokio::task::spawn_blocking(move || picture::for_agent(&png))
                .await
                .map_err(|_| Error::Device("The screenshot could not be read.".into()))??;
            Ok(ToolReply {
                text: format!(
                    "The screen, {} by {} pixels. Coordinates for mobile_tap and mobile_swipe are in this picture's pixels.",
                    picture.width, picture.height
                ),
                image: Some(picture.png),
                error: false,
            })
        }
        "mobile_install" => {
            let what = if iphone {
                "Give the full path of the .app the build made for the simulator."
            } else {
                "Give the path of the .apk file to install."
            };
            let path = text_arg(arguments, "path").ok_or_else(|| refused(what))?;
            live.install(Path::new(path)).await?;
            Ok(ToolReply::text(if iphone {
                "Installed. Start it with mobile_launch and its bundle id."
            } else {
                "Installed. Start it with mobile_launch."
            }))
        }
        "mobile_launch" => {
            let package =
                text_arg(arguments, "package").ok_or_else(|| refused(name_the_app(iphone)))?;
            let component = live
                .launch(package, text_arg(arguments, "activity"))
                .await?;
            *lock(&live.app) = Some(package.to_owned());
            tokio::time::sleep(SETTLE).await;
            Ok(ToolReply::text(format!(
                "Started {component}. Read the screen with mobile_snapshot."
            )))
        }
        "mobile_stop_app" => {
            let package =
                text_arg(arguments, "package").ok_or_else(|| refused(name_the_app(iphone)))?;
            live.stop_app(package).await?;
            Ok(ToolReply::text(format!("Stopped {package}.")))
        }
        "mobile_open_link" => {
            let link =
                text_arg(arguments, "link").ok_or_else(|| refused("Give the link to open."))?;
            live.open_link(link).await?;
            tokio::time::sleep(SETTLE).await;
            let web = ["http:", "https:"].iter().any(|scheme| {
                link.trim()
                    .get(..scheme.len())
                    .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
            });
            Ok(ToolReply::text(if iphone && !web {
                // As it asks a person, iOS asks before a link opens an app.
                "Opened the link. iOS may first ask whether to open the app: read the screen with mobile_snapshot, and tap Open if it asks."
            } else {
                "Opened the link. Read the screen with mobile_snapshot."
            }))
        }
        "mobile_logs" => {
            let lines =
                number(arguments, "lines").map_or(120, |lines| lines.clamp(1.0, 500.0) as u32);
            let package = match text_arg(arguments, "package") {
                Some(package) => Some(package.to_owned()),
                None => {
                    let remembered = lock(&live.app).clone();
                    match remembered {
                        Some(app) => Some(app),
                        None => live.foreground().await,
                    }
                }
            };
            Ok(ToolReply::text(live.logs(package.as_deref(), lines).await?))
        }
        "mobile_settings" => settings(&live, arguments).await,
        "mobile_wait_for" => wait_for(&live, arguments).await,
        _ => Err(refused("There is no such tool.")),
    }
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn name_the_app(iphone: bool) -> &'static str {
    if iphone {
        "Name the app's bundle id, like com.example.app."
    } else {
        "Name the app's package, like com.example.app."
    }
}

/// What exists and who holds it, for `mobile_devices`.
fn devices(manager: &Manager, chat_id: &str) -> String {
    let status = manager.status();
    let mut out = String::new();
    if let Some(unavailable) = &status.unavailable {
        return format!("{unavailable}\n");
    }
    if !status.setup.emulator {
        out.push_str("Android is not set up on this computer yet. Call mobile_start: o7 opens the Device pane beside the chat with Android's setup, for the person to accept.\n");
    } else if status
        .setup
        .acceleration
        .as_ref()
        .is_some_and(|acceleration| !acceleration.usable)
    {
        out.push_str("The emulator is installed, but this computer cannot run it fast yet. The Device pane says what to turn on.\n");
    }
    let ios = &status.setup.ios;
    if ios.possible {
        match (&ios.detail, &ios.missing) {
            (Some(detail), _) if ios.ready => out.push_str(&format!(
                "An iPhone simulator can run here ({detail}): mobile_start with platform ios.\n"
            )),
            (_, Some(missing)) => out.push_str(&format!("No iPhone simulator yet: {missing}\n")),
            _ => out.push_str("This is a Mac: mobile_start with platform ios starts an iPhone simulator once Xcode is found.\n"),
        }
    }
    for device in &status.devices {
        // An emulator left from before Android was removed cannot start: not listed as if it
        // could.
        if device.kind == DeviceKind::Emulator && !status.setup.emulator {
            continue;
        }
        let kind = match device.kind {
            DeviceKind::Emulator => "emulator",
            DeviceKind::Phone => "phone",
            DeviceKind::Simulator => "iPhone simulator",
        };
        let state = match device.state {
            DeviceState::Stopped => "stopped",
            DeviceState::Starting => "starting",
            DeviceState::Ready => "running",
            DeviceState::Stopping => "stopping",
            DeviceState::Unauthorized => {
                "waiting for the person to allow this computer on the phone"
            }
            DeviceState::Offline => "not reachable",
        };
        let holder = match &device.chat_id {
            Some(holder) if holder == chat_id => ", this chat's device",
            Some(_) => ", in use by another chat",
            None => "",
        };
        out.push_str(&format!(
            "- {} ({kind}, id {}): {state}{holder}\n",
            device.title, device.id
        ));
    }
    if status.devices.is_empty() && (status.setup.emulator || ios.ready) {
        out.push_str("No device exists yet. mobile_start makes and starts one.\n");
    }
    match status.chats.iter().find(|held| held.chat_id == chat_id) {
        Some(held) if !held.agent_allowed => out.push_str(
            "This chat's device is the person's phone, and they have not allowed agents to use it.\n",
        ),
        Some(held) if held.live => {
            out.push_str(&format!("This chat's device, {}, is ready.\n", held.title));
        }
        Some(held) if held.starting => {
            out.push_str(&format!("This chat's device, {}, is starting.\n", held.title));
        }
        Some(held) => out.push_str(&format!(
            "This chat's device, {}, is not running{}\n",
            held.title,
            held.error.as_ref().map_or(".".to_owned(), |error| format!(": {error}"))
        )),
        None => out.push_str("This chat has no device. Call mobile_start to get one.\n"),
    }
    out
}

async fn start(manager: &Manager, chat_id: &str, arguments: &Value) -> Result<ToolReply, Error> {
    let platform = match text_arg(arguments, "platform")
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None => None,
        Some("ios" | "iphone") => Some(ANY_IPHONE),
        Some("android") => Some(ANY_EMULATOR),
        Some(_) => return Err(refused("platform is android or ios.")),
    };
    let wanted = text_arg(arguments, "device").or(platform);
    let holds = |id: &str| {
        manager
            .status()
            .chats
            .iter()
            .any(|held| held.chat_id == chat_id && held.device_id == id)
    };
    if let Some(id) = wanted {
        // A phone is the person's to hand over, in the pane. An agent may only keep using the
        // one its chat already holds.
        if id.starts_with("adb:") && !holds(id) {
            return Err(refused(
                "Only the person can choose a phone, in the Device pane. Call mobile_start with no device for an emulator.",
            ));
        }
    }
    if manager.agent_refused(chat_id) {
        return Err(refused(
            "This chat's device is the person's phone, and they have not allowed agents to use it. Ask them to turn on \"Let the agent use this phone\" in the Device pane.",
        ));
    }
    match manager.attach(chat_id, wanted) {
        Ok(()) => {}
        Err(Error::Refused(text)) if text == NOT_SET_UP => {
            return Err(refused(SET_UP_IN_THE_PANE));
        }
        // An iPhone that cannot start yet: the pane beside the chat shows what is missing too.
        Err(Error::Refused(text)) if wanted == Some(ANY_IPHONE) => {
            return Err(refused(format!(
                "{text} o7 opened the Device pane beside the chat, which shows the same."
            )));
        }
        Err(error) => return Err(error),
    }
    let live = manager.wait_live(chat_id, START).await?;
    let (width, height) = live.screen();
    let system = live.system().await;
    let title = manager
        .status()
        .chats
        .iter()
        .find(|held| held.chat_id == chat_id)
        .map_or_else(|| "The device".to_owned(), |held| held.title.clone());
    let text = match live.iphone() {
        Some(iphone) => format!(
            "{title} is ready: {system}, screen {width} by {height} pixels. Its UDID is {udid}: build the app for it with xcodebuild -destination 'platform=iOS Simulator,id={udid}', install the .app with mobile_install, start it with mobile_launch and its bundle id, and read the screen with mobile_snapshot.",
            udid = iphone.sim.udid
        ),
        None => format!(
            "{title} is ready: {system}, screen {width} by {height}. Install an app with mobile_install, start it with mobile_launch, and read the screen with mobile_snapshot."
        ),
    };
    Ok(ToolReply::text(text))
}

async fn snapshot(live: &Live) -> Result<ToolReply, Error> {
    let outline = live.read_outline(true).await?;
    let (width, height) = live.screen();
    let mut text = format!("Screen {width} by {height}");
    if let Some(package) = &outline.package {
        text.push_str(&format!(", app {package}"));
    }
    text.push_str(".\n");
    text.push_str(&outline.render());
    *lock(&live.outline) = Some(outline);
    Ok(ToolReply::text(text))
}

/// The item a ref names, from the outline the agent last read.
fn item(live: &Live, reference: &str) -> Result<Item, Error> {
    let outline = lock(&live.outline);
    outline.as_ref().and_then(|outline| outline.find(reference)).cloned().ok_or_else(|| {
        refused(format!(
            "There is no {} in the last mobile_snapshot. The screen may have changed: take a new snapshot.",
            crate::shell::shown(reference)
        ))
    })
}

fn described(item: &Item) -> String {
    if item.name.is_empty() {
        item.role.to_owned()
    } else {
        format!("{} \"{}\"", item.role, item.name)
    }
}

/// A point given in a screenshot's pixels, as a point of the display.
fn picture_point(live: &Live, x: f64, y: f64) -> Result<(i32, i32), Error> {
    let (width, height) = live.screen();
    let factor = f64::from(picture::factor_for(width, height));
    let (x, y) = (x * factor, y * factor);
    if x < 0.0 || y < 0.0 || x >= f64::from(width) || y >= f64::from(height) {
        return Err(refused(format!(
            "That point is off the screen. A screenshot is {} by {} pixels.",
            width / factor as u32,
            height / factor as u32
        )));
    }
    Ok((x as i32, y as i32))
}

async fn tap(live: &Live, arguments: &Value) -> Result<ToolReply, Error> {
    let hold = if flag(arguments, "long") {
        Duration::from_millis(800)
    } else {
        Duration::from_millis(60)
    };
    let verb = if flag(arguments, "long") {
        "Long-pressed"
    } else {
        "Tapped"
    };
    let (point, what) = match (
        text_arg(arguments, "ref"),
        number(arguments, "x"),
        number(arguments, "y"),
    ) {
        (Some(reference), _, _) => {
            let item = item(live, reference)?;
            (item.bounds.center(), described(&item))
        }
        (None, Some(x), Some(y)) => (
            picture_point(live, x, y)?,
            format!("the point {x:.0}, {y:.0}"),
        ),
        _ => {
            return Err(refused(
                "Give a ref from mobile_snapshot, or x and y from a screenshot.",
            ));
        }
    };
    live.tap(point.0, point.1, hold).await?;
    tokio::time::sleep(SETTLE).await;
    Ok(ToolReply::text(format!(
        "{verb} {what}. The screen may have changed: read it again before using another ref."
    )))
}

async fn type_text(live: &Live, arguments: &Value) -> Result<ToolReply, Error> {
    let text = arguments
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if text.is_empty() && !flag(arguments, "submit") && !flag(arguments, "clear") {
        return Err(refused("Give the text to type."));
    }
    let mut into = String::new();
    if let Some(reference) = text_arg(arguments, "ref") {
        let item = item(live, reference)?;
        let (x, y) = item.bounds.center();
        live.tap(x, y, Duration::from_millis(60)).await?;
        tokio::time::sleep(Duration::from_millis(400)).await;
        into = format!(" into {}", described(&item));
    }
    live.type_text(text, flag(arguments, "clear"), flag(arguments, "submit"))
        .await?;
    tokio::time::sleep(SETTLE).await;
    let count = text.chars().count();
    Ok(ToolReply::text(format!(
        "Typed {count} character{}{into}{}.",
        if count == 1 { "" } else { "s" },
        if flag(arguments, "submit") {
            " and pressed Enter"
        } else {
            ""
        }
    )))
}

async fn swipe(live: &Live, arguments: &Value) -> Result<ToolReply, Error> {
    let duration = Duration::from_millis(
        number(arguments, "duration_ms").map_or(300, |ms| ms.clamp(50.0, 5000.0) as u64),
    );
    let pair = |name: &str| -> Option<(f64, f64)> {
        let values = arguments.get(name)?.as_array()?;
        Some((values.first()?.as_f64()?, values.get(1)?.as_f64()?))
    };
    let (from, to, what) = match (text_arg(arguments, "direction"), pair("from"), pair("to")) {
        (Some(direction), _, _) => {
            // Inside the item named, or the middle of the screen: the finger moves the way
            // named, so "up" scrolls a list down, as a finger does.
            let (width, height) = live.screen();
            let area = match text_arg(arguments, "ref") {
                Some(reference) => item(live, reference)?.bounds,
                None => crate::outline::Rect {
                    left: 0,
                    top: 0,
                    right: width as i32,
                    bottom: height as i32,
                },
            };
            let (cx, cy) = area.center();
            let (dx, dy) = (area.width() * 3 / 10, area.height() * 3 / 10);
            let (from, to) = match direction {
                "up" => ((cx, cy + dy), (cx, cy - dy)),
                "down" => ((cx, cy - dy), (cx, cy + dy)),
                "left" => ((cx + dx, cy), (cx - dx, cy)),
                "right" => ((cx - dx, cy), (cx + dx, cy)),
                _ => return Err(refused("A direction is up, down, left or right.")),
            };
            (from, to, format!("Swiped {direction}"))
        }
        (None, Some(from), Some(to)) => (
            picture_point(live, from.0, from.1)?,
            picture_point(live, to.0, to.1)?,
            "Swiped".to_owned(),
        ),
        _ => {
            return Err(refused(
                "Give a direction (up, down, left or right), or from and to as [x, y] from a screenshot.",
            ));
        }
    };
    live.swipe(from, to, duration).await?;
    tokio::time::sleep(SETTLE).await;
    Ok(ToolReply::text(format!(
        "{what}. Read the screen again to see what moved."
    )))
}

async fn settings(live: &Live, arguments: &Value) -> Result<ToolReply, Error> {
    let mut done = Vec::new();
    if let Some(appearance) = text_arg(arguments, "appearance") {
        let appearance = match appearance {
            "dark" => Appearance::Dark,
            "light" => Appearance::Light,
            _ => return Err(refused("Appearance is light or dark.")),
        };
        live.set_appearance(appearance).await?;
        done.push(match appearance {
            Appearance::Dark => "The device is in dark mode.".to_owned(),
            Appearance::Light => "The device is in light mode.".to_owned(),
        });
    }
    if let Some(scale) = number(arguments, "text_scale") {
        live.set_text_scale(scale as f32).await?;
        done.push(format!("Text is at {scale:.2} times its usual size."));
    }
    if let Some(orientation) = text_arg(arguments, "orientation") {
        let orientation = match orientation {
            "portrait" => Orientation::Portrait,
            "landscape" => Orientation::Landscape,
            _ => return Err(refused("Orientation is portrait or landscape.")),
        };
        let turned = live.set_orientation(orientation).await?;
        done.push(match (orientation, turned) {
            (Orientation::Portrait, true) => "The device is upright.".to_owned(),
            (Orientation::Landscape, true) => "The device is on its side.".to_owned(),
            (_, false) => "The device did not turn: Android kept it as it was. Take a screenshot to see it, and try again.".to_owned(),
        });
    }
    // A change of look is drawn a moment after the device takes it.
    let redraw = ["appearance", "text_scale", "orientation"]
        .iter()
        .any(|name| arguments.get(*name).is_some());
    if let Some(permission) = arguments
        .get("permission")
        .filter(|value| value.is_object())
    {
        let iphone = live.iphone().is_some();
        let package = text_arg(permission, "package")
            .ok_or_else(|| refused("A permission change names the app's package or bundle id."))?;
        let name = text_arg(permission, "name").ok_or_else(|| {
            refused(if iphone {
                "A permission change names the permission, like photos or location."
            } else {
                "A permission change names the permission, like android.permission.CAMERA."
            })
        })?;
        let granted = permission
            .get("granted")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        live.set_permission(package, name, granted).await?;
        done.push(format!(
            "{name} is {} for {package}.",
            if granted { "granted" } else { "taken away" }
        ));
    }
    if done.is_empty() {
        return Err(refused(
            "Give appearance (light or dark), text_scale (0.85 to 2), orientation (portrait or landscape), or permission.",
        ));
    }
    tokio::time::sleep(if redraw { REDRAW } else { SETTLE }).await;
    Ok(ToolReply::text(done.join(" ")))
}

async fn wait_for(live: &Live, arguments: &Value) -> Result<ToolReply, Error> {
    let text = text_arg(arguments, "text").ok_or_else(|| refused("Give the text to wait for."))?;
    let gone = flag(arguments, "gone");
    let within = Duration::from_secs_f64(
        number(arguments, "timeout_seconds").map_or(10.0, |seconds| seconds.clamp(1.0, 60.0)),
    );
    let deadline = tokio::time::Instant::now() + within;
    let wanted = text.to_lowercase();
    loop {
        // A screen that is moving cannot be read; that is not the answer, so it is asked again.
        if let Ok(outline) = live.read_outline(false).await {
            let shows = outline.items.iter().any(|item| {
                item.name.to_lowercase().contains(&wanted)
                    || item
                        .value
                        .as_ref()
                        .is_some_and(|value| value.to_lowercase().contains(&wanted))
            });
            if shows != gone {
                let rendered = outline.render();
                *lock(&live.outline) = Some(outline);
                return Ok(ToolReply::text(format!(
                    "\"{}\" is {}. The screen now:\n{rendered}",
                    crate::shell::shown(text),
                    if gone { "gone" } else { "on the screen" }
                )));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Device(format!(
                "\"{}\" {} within {} seconds. Read the screen or take a screenshot to see what is there.",
                crate::shell::shown(text),
                if gone {
                    "was still on the screen"
                } else {
                    "did not appear"
                },
                within.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::manager::tests::{hold_phone, manager};

    #[test]
    fn what_the_pane_shows_never_holds_what_was_typed() {
        let secret = json!({ "text": "hunter2-secret", "ref": "d4", "submit": true });
        assert_eq!(label("mobile_type", &secret), "Typing");
        assert_eq!(
            label(
                "mobile_open_link",
                &json!({ "link": "https://example.com/?token=abc" })
            ),
            "Opening a link"
        );
        assert_eq!(
            label(
                "mobile_install",
                &json!({ "path": "/home/sam/secret/app.apk" })
            ),
            "Installing the app"
        );
        assert_eq!(label("mobile_tap", &json!({ "ref": "d3" })), "Tapping d3");
        assert_eq!(label("mobile_tap", &json!({ "x": 5, "y": 9 })), "Tapping");
        assert_eq!(
            label("mobile_swipe", &json!({ "direction": "up" })),
            "Swiping up"
        );
        assert_eq!(
            label("mobile_swipe", &json!({ "direction": "<b>sideways</b>" })),
            "Swiping"
        );
        assert_eq!(
            label("mobile_press", &json!({ "key": "Back" })),
            "Pressing Back"
        );
        assert_eq!(
            label("mobile_press", &json!({ "key": "rm -rf" })),
            "Pressing a key"
        );
        for tool in TOOLS {
            assert!(!label(tool, &secret).contains("hunter2"), "{tool}");
        }
    }

    #[test]
    fn keys_have_names_agents_would_guess() {
        assert_eq!(named_key("back"), Some(("Back", key::BACK)));
        assert_eq!(named_key("Home"), Some(("Home", key::HOME)));
        assert_eq!(named_key("app-switch"), Some(("Recents", key::APP_SWITCH)));
        assert_eq!(named_key("volume up"), Some(("Volume up", key::VOLUME_UP)));
        assert_eq!(named_key("f13"), None);
        assert_eq!(named_key(""), None);
    }

    #[tokio::test]
    async fn every_tool_refuses_when_mobile_is_off() {
        let folder = tempfile::tempdir().unwrap();
        let manager = manager(folder.path());
        manager.set_enabled(false).await;
        for tool in TOOLS {
            let reply = manager.tool("chat", tool, &json!({})).await;
            assert!(reply.error, "{tool}");
            assert_eq!(reply.text, TURNED_OFF, "{tool}");
        }
    }

    #[tokio::test]
    async fn a_chat_without_a_device_is_told_to_start_one() {
        let folder = tempfile::tempdir().unwrap();
        let manager = manager(folder.path());
        for tool in TOOLS {
            if matches!(tool, "mobile_devices" | "mobile_start" | "mobile_stop") {
                continue;
            }
            let reply = manager
                .tool("chat", tool, &json!({ "ref": "d1", "text": "x" }))
                .await;
            assert!(reply.error, "{tool}");
            assert_eq!(
                reply.text, "This chat has no device yet. Call mobile_start first.",
                "{tool}"
            );
        }
        let unknown = manager.tool("chat", "mobile_format_disk", &json!({})).await;
        assert!(unknown.error);
        assert_eq!(unknown.text, "There is no tool called mobile_format_disk.");
        let listed = manager.tool("chat", "mobile_devices", &json!({})).await;
        assert!(!listed.error);
        assert!(
            listed.text.contains("Android is not set up"),
            "{}",
            listed.text
        );
        assert!(
            listed.text.contains("This chat has no device."),
            "{}",
            listed.text
        );
        let start = manager.tool("chat", "mobile_start", &json!({})).await;
        assert!(start.error);
        assert!(
            start.text.starts_with("Android is not set up"),
            "{}",
            start.text
        );
        // The agent is told the pane opened with the setup, which the person accepts.
        assert!(
            start.text.contains("o7 opened the Device pane"),
            "{}",
            start.text
        );
        assert!(!manager.tool("chat", "mobile_stop", &json!({})).await.error);
    }

    #[tokio::test]
    async fn an_agents_call_opens_the_pane_even_before_android_is_set_up() {
        let folder = tempfile::tempdir().unwrap();
        let manager = manager(folder.path());
        let seq = |chat: &str| {
            manager
                .status()
                .summons
                .iter()
                .find(|summon| summon.chat_id == chat)
                .map(|summon| summon.seq)
        };
        // Listing and stopping do not open the pane.
        manager.tool("chat", "mobile_devices", &json!({})).await;
        manager.tool("chat", "mobile_stop", &json!({})).await;
        assert_eq!(seq("chat"), None);
        // Every other call does, refused or not, and each call is a new number.
        let mut last = 0;
        for tool in TOOLS {
            if matches!(tool, "mobile_devices" | "mobile_stop") {
                continue;
            }
            manager.tool("chat", tool, &json!({})).await;
            let now = seq("chat").unwrap_or_else(|| panic!("{tool} did not summon"));
            assert!(now > last, "{tool}");
            last = now;
        }
        manager.tool("other", "mobile_start", &json!({})).await;
        assert!(seq("other").unwrap() > last);
        assert_eq!(seq("chat"), Some(last));
        // Off, nothing is summoned.
        manager.set_enabled(false).await;
        manager.tool("chat", "mobile_start", &json!({})).await;
        assert_eq!(seq("chat"), Some(last));
    }

    #[tokio::test]
    async fn an_agent_cannot_use_or_take_a_phone_the_person_has_not_handed_over() {
        let folder = tempfile::tempdir().unwrap();
        let manager = manager(folder.path());
        hold_phone(&manager, "chat", "R58M");
        hold_phone(&manager, "other", "OTHER1");
        for tool in TOOLS {
            if matches!(tool, "mobile_devices" | "mobile_stop") {
                continue;
            }
            let reply = manager
                .tool("chat", tool, &json!({ "ref": "d1", "text": "x" }))
                .await;
            assert!(reply.error, "{tool}");
            assert!(
                reply.text.contains("not allowed agents to use"),
                "{tool}: {}",
                reply.text
            );
        }
        // Nor may it reach for a phone its chat does not hold, allowed or not.
        let folder = tempfile::tempdir().unwrap();
        let manager = manager_with_phone(folder.path());
        let grabbed = manager
            .tool("mine", "mobile_start", &json!({ "device": "adb:OTHER1" }))
            .await;
        assert!(grabbed.error);
        assert!(
            grabbed
                .text
                .starts_with("Only the person can choose a phone"),
            "{}",
            grabbed.text
        );
        assert!(
            manager
                .status()
                .chats
                .iter()
                .all(|held| held.chat_id != "mine")
        );
        let listed = manager.tool("other", "mobile_devices", &json!({})).await;
        assert!(
            listed
                .text
                .contains("Pixel 8 (phone, id adb:OTHER1): running, this chat's device"),
            "{}",
            listed.text
        );
        assert!(
            listed.text.contains("they have not allowed agents"),
            "{}",
            listed.text
        );
        let elsewhere = manager.tool("mine", "mobile_devices", &json!({})).await;
        assert!(
            elsewhere.text.contains("in use by another chat"),
            "{}",
            elsewhere.text
        );
    }

    fn manager_with_phone(folder: &Path) -> Manager {
        let manager = manager(folder);
        hold_phone(&manager, "other", "OTHER1");
        manager
    }

    #[tokio::test]
    async fn the_pane_is_told_what_the_agent_does_and_when_it_stops() {
        let folder = tempfile::tempdir().unwrap();
        let manager = manager(folder.path());
        hold_phone(&manager, "chat", "R58M");
        manager.agent_doing("chat", Some("Tapping d3".into()));
        assert_eq!(
            manager.status().chats[0].agent.as_deref(),
            Some("Tapping d3")
        );
        manager.agent_doing("chat", None);
        assert_eq!(manager.status().chats[0].agent, None);
        // A refused call leaves nothing behind.
        let _ = manager
            .tool("chat", "mobile_type", &json!({ "text": "secret" }))
            .await;
        assert_eq!(manager.status().chats[0].agent, None);
    }
}
