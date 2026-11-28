//! The browser tools agents get (ghost-core's browser grant and the app's `--ghost-browser-helper`
//! MCP server carry them here). Each works in the chat's browser, in the tab in front, and the
//! panel shows what the agent does while it does it. Page content is untrusted: the replies say
//! what the page shows, never act on it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Browser, BrowserError, GoAction, Shot, address, host};
use crate::sync::lock;

/// What a tool answers: text, and a screenshot for the screenshot tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolReply {
    pub text: String,
    /// A PNG or JPEG, base64, with its MIME type.
    pub image: Option<(String, String)>,
    pub error: bool,
}

impl ToolReply {
    fn text(text: impl Into<String>) -> ToolReply {
        ToolReply {
            text: text.into(),
            image: None,
            error: false,
        }
    }

    fn failed(text: impl Into<String>) -> ToolReply {
        ToolReply {
            text: text.into(),
            image: None,
            error: true,
        }
    }
}

const LOAD_WAIT: Duration = Duration::from_secs(20);

/// Runs one browser tool for `chat_id`.
pub async fn call(browser: &Arc<Browser>, chat_id: &str, tool: &str, args: &Value) -> ToolReply {
    if let Err(error) = browser.check() {
        return ToolReply::failed(error.to_string());
    }
    let label = label(tool, args);
    let tab = active(browser, chat_id);
    let opened = tool == "browser_navigate" || (tool == "browser_tabs" && args["action"] == "new");
    browser.agent_activity(chat_id, tab.as_deref(), Some(&label), opened);
    let reply = run(browser, chat_id, tool, args).await;
    let tab = active(browser, chat_id);
    browser.agent_activity(chat_id, tab.as_deref(), None, false);
    // A page kept drawing for the agent hides a little after it stops.
    let idle = Arc::clone(browser);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let _ = idle.on_main(host::unpark_idle).await;
    });
    match reply {
        Ok(reply) => reply,
        Err(error) => ToolReply::failed(error.to_string()),
    }
}

pub(super) fn active(browser: &Browser, chat_id: &str) -> Option<String> {
    lock(&browser.inner)
        .chats
        .get(chat_id)
        .and_then(|chat| chat.active.clone())
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn label(tool: &str, args: &Value) -> String {
    let target = str_arg(args, "ref").unwrap_or("the page");
    match tool {
        "browser_navigate" => match (str_arg(args, "url"), str_arg(args, "action")) {
            (Some(url), _) => format!(
                "Opening {}",
                address::resolve(url).map_or_else(|| url.to_owned(), |url| address::site(&url))
            ),
            (None, Some("back")) => "Going back".into(),
            (None, Some("forward")) => "Going forward".into(),
            _ => "Reloading".into(),
        },
        "browser_snapshot" => "Reading the page".into(),
        "browser_click" => format!("Clicking {target}"),
        "browser_type" => format!("Typing into {target}"),
        "browser_press_key" => format!("Pressing {}", str_arg(args, "key").unwrap_or("a key")),
        "browser_hover" => format!("Pointing at {target}"),
        "browser_select" => format!("Choosing in {target}"),
        "browser_scroll" => "Scrolling".into(),
        "browser_screenshot" => "Taking a screenshot".into(),
        "browser_console" => "Reading the console".into(),
        "browser_network" => "Reading the network log".into(),
        "browser_evaluate" => "Running a script in the page".into(),
        "browser_wait_for" => "Waiting on the page".into(),
        "browser_tabs" => "Working with tabs".into(),
        _ => "Using the browser".into(),
    }
}

/// The chat's tab in front, built and drawing; a new one when the chat has none and `open`.
pub(super) async fn tab_for(browser: &Arc<Browser>, chat_id: &str) -> Result<String, BrowserError> {
    let tab = active(browser, chat_id).ok_or_else(|| {
        BrowserError::Invalid(
            "No page is open in this chat's browser. Call browser_navigate first.".into(),
        )
    })?;
    let has_page = lock(&browser.inner)
        .tabs
        .get(&tab)
        .is_some_and(|tab| tab.url.is_some());
    if !has_page {
        return Err(BrowserError::Invalid(
            "The tab in front has no page yet. Call browser_navigate first.".into(),
        ));
    }
    let id = tab.clone();
    browser
        .on_main(move |browser| host::prepare_for_agent(browser, &id))
        .await??;
    Ok(tab)
}

fn loads(browser: &Browser, tab: &str) -> u64 {
    lock(&browser.inner)
        .tabs
        .get(tab)
        .map_or(0, |tab| tab.loads)
}

/// Waits for what an action set off: a new page that finishes loading, or (when nothing starts
/// loading within a moment) nothing at all. Never longer than [`LOAD_WAIT`].
async fn settle(browser: &Arc<Browser>, tab: &str, before: u64, navigation_expected: bool) {
    let started = Instant::now();
    // How long a load gets to start: a click may navigate a moment later, a typed address at once.
    let grace = Duration::from_millis(if navigation_expected { 1500 } else { 400 });
    loop {
        let state = lock(&browser.inner)
            .tabs
            .get(tab)
            .map(|tab| (tab.loading, tab.loads));
        let Some((loading, loads)) = state else {
            return;
        };
        let waited = started.elapsed();
        if (loads > before && !loading) || waited >= LOAD_WAIT || (!loading && waited >= grace) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
}

/// Where the tab is now, in a line.
fn whereabouts(browser: &Browser, tab: &str) -> String {
    let inner = lock(&browser.inner);
    let Some(tab) = inner.tabs.get(tab) else {
        return "The tab closed.".into();
    };
    let url = tab.url.clone().unwrap_or_else(|| "a new tab".into());
    let mut line = if tab.title.is_empty() {
        format!("Now at {url}")
    } else {
        format!("Now at {url} (\"{}\")", tab.title)
    };
    if tab.loading {
        line.push_str(", still loading");
    }
    if let Some(error) = &tab.error {
        line.push_str(&format!(". The load failed: {error}"));
    }
    if tab.crashed {
        line.push_str(". The page crashed; reload it with browser_navigate action \"reload\"");
    }
    line.push('.');
    line
}

async fn run(
    browser: &Arc<Browser>,
    chat_id: &str,
    tool: &str,
    args: &Value,
) -> Result<ToolReply, BrowserError> {
    match tool {
        "browser_navigate" => navigate(browser, chat_id, args).await,
        "browser_snapshot" => {
            let tab = tab_for(browser, chat_id).await?;
            let scope = str_arg(args, "selector");
            let outline = browser
                .call(&tab, &format!("api.snapshot({})", js(&scope)))
                .await?;
            let text = outline["text"].as_str().unwrap_or("");
            let head = whereabouts(browser, &tab);
            Ok(ToolReply::text(if text.is_empty() {
                format!("{head}\nThe page shows nothing readable yet.")
            } else {
                format!("{head}\n{text}")
            }))
        }
        "browser_click" => {
            let tab = tab_for(browser, chat_id).await?;
            let target = required(args, "ref")?;
            let place = locate(browser, &tab, target).await?;
            let before = loads(browser, &tab);
            let button = match str_arg(args, "button") {
                Some("right") => "right",
                Some("middle") => "middle",
                _ => "left",
            };
            let double = args["double"].as_bool().unwrap_or(false);
            #[cfg(windows)]
            {
                let modifiers: Vec<String> = args["modifiers"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(|m| m.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                super::webview2::click(
                    browser,
                    &tab,
                    place.x,
                    place.y,
                    button,
                    if double { 2 } else { 1 },
                    super::webview2::modifier_bits(&modifiers),
                )
                .await?;
            }
            #[cfg(not(windows))]
            {
                let options = json!({ "button": button, "double": double });
                browser
                    .call(
                        &tab,
                        &format!("api.click({}, {options})", js(&Some(target))),
                    )
                    .await?;
            }
            settle(browser, &tab, before, false).await;
            let mut text = format!("Clicked {}.", place.name);
            if let Some(covered) = place.covered {
                text.push_str(&format!(
                    " Note: {covered} was on top of it where it was clicked."
                ));
            }
            if place.disabled {
                text.push_str(" It is disabled, so the click may have done nothing.");
            }
            let now = active(browser, chat_id);
            if now.as_deref() != Some(tab.as_str()) {
                text.push_str(" It opened a new tab, which is now in front.");
            } else if loads(browser, &tab) == before {
                // The same page: say what the element reads now, when the click changed it.
                match browser
                    .call(&tab, &format!("api.locate({})", js(&Some(target))))
                    .await
                {
                    Ok(after) if after["error"].is_null() => {
                        let name = after["name"].as_str().unwrap_or("");
                        if !name.is_empty() && format!("\"{name}\"") != place.name {
                            text.push_str(&format!(" It now reads \"{name}\"."));
                        }
                    }
                    Ok(_) => text.push_str(" It is no longer on the page."),
                    Err(_) => {}
                }
            }
            Ok(ToolReply::text(format!(
                "{text}\n{}",
                whereabouts(browser, now.as_deref().unwrap_or(&tab))
            )))
        }
        "browser_type" => {
            let tab = tab_for(browser, chat_id).await?;
            let target = required(args, "ref")?;
            let text = args["text"]
                .as_str()
                .ok_or_else(|| BrowserError::Invalid("Say what to type in \"text\".".into()))?;
            if text.len() > 100_000 {
                return Err(BrowserError::Invalid(
                    "That is too much text to type at once.".into(),
                ));
            }
            let clear = args["clear"].as_bool().unwrap_or(true);
            let submit = args["submit"].as_bool().unwrap_or(false);
            let before = loads(browser, &tab);
            #[cfg(windows)]
            {
                let focused = browser
                    .call(&tab, &format!("api.focus({}, {clear})", js(&Some(target))))
                    .await?;
                if let Some(error) = focused["error"].as_str() {
                    return Err(BrowserError::Page(error.to_owned()));
                }
                if !text.is_empty() {
                    super::webview2::insert(browser, &tab, text).await?;
                }
                if submit {
                    super::webview2::press(browser, &tab, "Enter").await?;
                }
            }
            #[cfg(not(windows))]
            {
                let typed = browser
                    .call(
                        &tab,
                        &format!(
                            "api.type({}, {}, {clear})",
                            js(&Some(target)),
                            js(&Some(text))
                        ),
                    )
                    .await?;
                if let Some(error) = typed["error"].as_str() {
                    return Err(BrowserError::Page(error.to_owned()));
                }
                if submit {
                    browser.call(&tab, "api.press(\"Enter\", [])").await?;
                }
            }
            settle(browser, &tab, before, submit).await;
            Ok(ToolReply::text(format!(
                "Typed {} characters{}.\n{}",
                text.chars().count(),
                if submit { " and pressed Enter" } else { "" },
                whereabouts(browser, &tab)
            )))
        }
        "browser_press_key" => {
            let tab = tab_for(browser, chat_id).await?;
            let key = required(args, "key")?;
            let before = loads(browser, &tab);
            #[cfg(windows)]
            super::webview2::press(browser, &tab, key).await?;
            #[cfg(not(windows))]
            {
                let mut parts: Vec<&str> = key.split('+').collect();
                let main = parts.pop().unwrap_or(key);
                browser
                    .call(
                        &tab,
                        &format!("api.press({}, {})", js(&Some(main)), json!(parts)),
                    )
                    .await?;
            }
            settle(browser, &tab, before, key.ends_with("Enter")).await;
            Ok(ToolReply::text(format!(
                "Pressed {key}.\n{}",
                whereabouts(browser, &tab)
            )))
        }
        "browser_hover" => {
            let tab = tab_for(browser, chat_id).await?;
            let target = required(args, "ref")?;
            let place = locate(browser, &tab, target).await?;
            #[cfg(windows)]
            super::webview2::hover(browser, &tab, place.x, place.y).await?;
            #[cfg(not(windows))]
            browser
                .call(&tab, &format!("api.hover({})", js(&Some(target))))
                .await?;
            Ok(ToolReply::text(format!(
                "The pointer is over {}.",
                place.name
            )))
        }
        "browser_select" => {
            let tab = tab_for(browser, chat_id).await?;
            let target = required(args, "ref")?;
            let values: Vec<String> = args["values"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            if values.is_empty() {
                return Err(BrowserError::Invalid(
                    "Name the options to choose in \"values\".".into(),
                ));
            }
            let chosen = browser
                .call(
                    &tab,
                    &format!("api.select({}, {})", js(&Some(target)), json!(values)),
                )
                .await?;
            if let Some(error) = chosen["error"].as_str() {
                return Err(BrowserError::Page(error.to_owned()));
            }
            Ok(ToolReply::text(format!("Chose {}.", values.join(", "))))
        }
        "browser_scroll" => {
            let tab = tab_for(browser, chat_id).await?;
            let target = str_arg(args, "ref");
            let dx = args["dx"].as_f64().unwrap_or(0.0);
            let dy = args["dy"]
                .as_f64()
                .unwrap_or(if target.is_some() { 0.0 } else { 600.0 });
            let scrolled = browser
                .call(&tab, &format!("api.scroll({}, {dx}, {dy})", js(&target)))
                .await?;
            if let Some(error) = scrolled["error"].as_str() {
                return Err(BrowserError::Page(error.to_owned()));
            }
            Ok(ToolReply::text(match target {
                Some(target) if dx == 0.0 && dy == 0.0 => format!("{target} is in view."),
                _ => format!(
                    "Scrolled. The page is now {} px from its top.",
                    scrolled["y"].as_f64().unwrap_or(0.0).round()
                ),
            }))
        }
        "browser_screenshot" => screenshot(browser, chat_id, args).await,
        "browser_console" => {
            let tab = tab_for(browser, chat_id).await?;
            let clear = args["clear"].as_bool().unwrap_or(false);
            let logs = browser.call(&tab, &format!("api.logs({clear})")).await?;
            let entries = logs.as_array().cloned().unwrap_or_default();
            if entries.is_empty() {
                return Ok(ToolReply::text("The console is empty."));
            }
            let mut lines = Vec::new();
            let mut size = 0;
            for entry in entries.iter().rev() {
                let line = format!(
                    "[{}] {}",
                    entry["level"].as_str().unwrap_or("info"),
                    entry["text"].as_str().unwrap_or("")
                );
                size += line.len();
                if size > 40_000 {
                    lines.push("… earlier messages left out".to_owned());
                    break;
                }
                lines.push(line);
            }
            lines.reverse();
            // A script may have printed a password.
            let secrets = page_secrets(browser, &tab).await.ok_or_else(|| BrowserError::Page("Ghost could not check this page's password fields, so this is withheld. Try again.".into()))?;
            Ok(ToolReply::text(redact(
                lines.join("\n"),
                secrets.iter().map(String::as_str),
            )))
        }
        "browser_network" => {
            let tab = tab_for(browser, chat_id).await?;
            let entries = browser.call(&tab, "api.network()").await?;
            let entries = entries.as_array().cloned().unwrap_or_default();
            if entries.is_empty() {
                return Ok(ToolReply::text("No requests are recorded for this page."));
            }
            let lines: Vec<String> = entries
                .iter()
                .rev()
                .take(150)
                .rev()
                .map(|entry| {
                    let status = entry["status"]
                        .as_u64()
                        .filter(|status| *status > 0)
                        .map_or_else(|| "—".to_owned(), |status| status.to_string());
                    format!(
                        "{status} {} {}ms {}",
                        entry["type"].as_str().unwrap_or("other"),
                        entry["ms"].as_u64().unwrap_or(0),
                        entry["url"].as_str().unwrap_or("")
                    )
                })
                .collect();
            // A form sent by GET carries its password in the address.
            let secrets = page_secrets(browser, &tab).await.ok_or_else(|| BrowserError::Page("Ghost could not check this page's password fields, so this is withheld. Try again.".into()))?;
            Ok(ToolReply::text(redact(
                format!(
                    "Requests this page made (status, kind, time, address; — where the page could not tell):\n{}",
                    lines.join("\n")
                ),
                secrets.iter().map(String::as_str),
            )))
        }
        "browser_evaluate" => evaluate(browser, chat_id, args).await,
        "browser_wait_for" => {
            let tab = tab_for(browser, chat_id).await?;
            let text = str_arg(args, "text");
            let selector = str_arg(args, "selector");
            if text.is_none() && selector.is_none() {
                let seconds = args["seconds"].as_f64().unwrap_or(1.0).clamp(0.0, 30.0);
                tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
                return Ok(ToolReply::text(format!("Waited {seconds} s.")));
            }
            let gone = args["gone"].as_bool().unwrap_or(false);
            let seconds = args["seconds"].as_f64().unwrap_or(10.0).clamp(0.5, 30.0);
            let started = Instant::now();
            loop {
                let found = browser
                    .call(&tab, &format!("api.find({}, {})", js(&text), js(&selector)))
                    .await?
                    .as_bool()
                    .unwrap_or(false);
                if found != gone {
                    let what = text.or(selector).unwrap_or("");
                    return Ok(ToolReply::text(if gone {
                        format!("{what} is gone.")
                    } else {
                        format!("{what} is on the page.")
                    }));
                }
                if started.elapsed().as_secs_f64() >= seconds {
                    return Err(BrowserError::Page(format!(
                        "After {seconds} s the page still {} it.",
                        if gone { "shows" } else { "does not show" }
                    )));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        "browser_tabs" => tabs(browser, chat_id, args).await,
        _ => Err(BrowserError::Invalid(format!(
            "{tool} is not a browser tool."
        ))),
    }
}

fn required<'a>(args: &'a Value, name: &str) -> Result<&'a str, BrowserError> {
    str_arg(args, name).ok_or_else(|| BrowserError::Invalid(format!("Give \"{name}\".")))
}

/// A string as a JavaScript literal (or `null`).
fn js(value: &Option<&str>) -> String {
    match value {
        Some(value) => serde_json::to_string(value).unwrap_or_else(|_| "null".into()),
        None => "null".into(),
    }
}

/// Where an element is to act on: its centre in the viewport (real input events use it on
/// Windows; elsewhere the page script acts on the element itself), its name and what is over it.
struct Place {
    #[cfg_attr(not(windows), allow(dead_code))]
    x: f64,
    #[cfg_attr(not(windows), allow(dead_code))]
    y: f64,
    name: String,
    covered: Option<String>,
    disabled: bool,
    rect: (f64, f64, f64, f64),
}

async fn locate(browser: &Arc<Browser>, tab: &str, target: &str) -> Result<Place, BrowserError> {
    let place = browser
        .call(tab, &format!("api.locate({})", js(&Some(target))))
        .await?;
    if let Some(error) = place["error"].as_str() {
        return Err(BrowserError::Page(error.to_owned()));
    }
    if place["visible"].as_bool() == Some(false) {
        return Err(BrowserError::Page(format!(
            "{target} takes no room on the page (it is hidden or empty)."
        )));
    }
    let rect = &place["rect"];
    Ok(Place {
        x: place["x"].as_f64().unwrap_or(0.0),
        y: place["y"].as_f64().unwrap_or(0.0),
        name: place["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .map_or_else(|| target.to_owned(), |name| format!("\"{name}\"")),
        covered: place["covered"]["tag"].as_str().map(|tag| {
            let text = place["covered"]["text"].as_str().unwrap_or("");
            if text.is_empty() {
                format!("a {tag}")
            } else {
                format!(
                    "a {tag} (\"{}\")",
                    text.chars().take(60).collect::<String>()
                )
            }
        }),
        disabled: place["disabled"].as_bool().unwrap_or(false),
        rect: (
            rect["x"].as_f64().unwrap_or(0.0),
            rect["y"].as_f64().unwrap_or(0.0),
            rect["width"].as_f64().unwrap_or(0.0),
            rect["height"].as_f64().unwrap_or(0.0),
        ),
    })
}

async fn navigate(
    browser: &Arc<Browser>,
    chat_id: &str,
    args: &Value,
) -> Result<ToolReply, BrowserError> {
    if let Some(action) = str_arg(args, "action") {
        let tab = tab_for(browser, chat_id).await?;
        let action = match action {
            "back" => GoAction::Back,
            "forward" => GoAction::Forward,
            "reload" => GoAction::Reload,
            other => {
                return Err(BrowserError::Invalid(format!(
                    "\"{other}\" is not back, forward or reload."
                )));
            }
        };
        let (from, possible) = lock(&browser.inner)
            .tabs
            .get(&tab)
            .map(|state| {
                let possible = match action {
                    GoAction::Back => state.can_back,
                    GoAction::Forward => state.can_forward,
                    _ => true,
                };
                (state.url.clone(), possible)
            })
            .unwrap_or((None, false));
        if !possible {
            let way = if matches!(action, GoAction::Back) {
                "back"
            } else {
                "forward"
            };
            return Ok(ToolReply::text(format!(
                "There is no page to go {way} to. {}",
                whereabouts(browser, &tab)
            )));
        }
        let before = loads(browser, &tab);
        browser.go(&tab, action).await?;
        settle(browser, &tab, before, true).await;
        // A page kept in memory comes back without loading; wait for the address to move too.
        if !matches!(action, GoAction::Reload) {
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(3)
                && lock(&browser.inner).tabs.get(&tab).map(|state| &state.url) == Some(&from)
            {
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
        }
        return Ok(ToolReply::text(whereabouts(browser, &tab)));
    }
    let input = required(args, "url")?;
    let url = address::resolve_for_agent(input).map_err(BrowserError::Invalid)?;
    let new_tab = args["new_tab"].as_bool().unwrap_or(false);
    let current = active(browser, chat_id);
    let tab = match current {
        Some(tab) if !new_tab => {
            let before = loads(browser, &tab);
            browser.navigate(&tab, &url).await?;
            let id = tab.clone();
            browser
                .on_main(move |browser| host::prepare_for_agent(browser, &id))
                .await??;
            settle(browser, &tab, before, true).await;
            tab
        }
        _ => {
            let tab = browser.open_tab(chat_id, Some(url), true)?.id;
            let id = tab.clone();
            browser
                .on_main(move |browser| host::prepare_for_agent(browser, &id))
                .await??;
            browser.agent_activity(chat_id, Some(&tab), Some("Opening the page"), true);
            settle(browser, &tab, 0, true).await;
            browser.agent_activity(chat_id, Some(&tab), None, false);
            tab
        }
    };
    Ok(ToolReply::text(format!(
        "{}\nCall browser_snapshot to read it.",
        whereabouts(browser, &tab)
    )))
}

async fn screenshot(
    browser: &Arc<Browser>,
    chat_id: &str,
    args: &Value,
) -> Result<ToolReply, BrowserError> {
    let tab = tab_for(browser, chat_id).await?;
    let full = args["full_page"].as_bool().unwrap_or(false);
    let shot = match str_arg(args, "ref") {
        Some(target) => {
            let place = locate(browser, &tab, target).await?;
            let (x, y, width, height) = place.rect;
            Shot::Clip {
                x,
                y,
                width: width.max(1.0),
                height: height.max(1.0),
            }
        }
        None if full => Shot::FullPage,
        None => Shot::Viewport { quality: 80 },
    };
    match host::screenshot(browser, &tab, shot).await? {
        Some(data) => {
            let mime = if matches!(shot, Shot::Viewport { .. }) {
                "image/jpeg"
            } else {
                "image/png"
            };
            Ok(ToolReply {
                text: whereabouts(browser, &tab),
                image: Some((mime.to_owned(), data)),
                error: false,
            })
        }
        None => Err(BrowserError::Unavailable(
            "Screenshots of the browser are not available on this system yet. Use browser_snapshot to read the page.".into(),
        )),
    }
}

/// The DevTools object group an agent's script values live in until they are read.
#[cfg(windows)]
const AGENT_OBJECTS: &str = "aura-agent";

/// Turns a script's value (`this`) into plain data an agent can read: elements as their tags and
/// text, windows, functions, maps, sets, dates and errors in words, cycles and depth cut short.
#[cfg(windows)]
const PLAIN_DATA: &str = r##"function () {
  const seen = new WeakSet();
  const element = (node) => {
    if (node.nodeType !== 1)
      return node.nodeName + (node.textContent ? ": " + node.textContent.trim().slice(0, 200) : "");
    const id = node.id ? "#" + node.id : "";
    const classes =
      typeof node.className === "string" && node.className.trim()
        ? "." + node.className.trim().split(/\s+/).slice(0, 3).join(".")
        : "";
    const text = (node.innerText || node.textContent || "").trim().replace(/\s+/g, " ").slice(0, 200);
    return "<" + node.tagName.toLowerCase() + id + classes + ">" + (text ? " " + JSON.stringify(text) : "");
  };
  const plain = (value, depth) => {
    if (value === undefined) return null;
    if (typeof value === "bigint") return value.toString() + "n";
    if (typeof value === "symbol") return value.toString();
    if (typeof value === "function") return "function " + (value.name || "(anonymous)");
    if (value === null || typeof value !== "object") return value;
    if (typeof Node === "function" && value instanceof Node) return element(value);
    if (value.window === value)
      return "a window (" + (value === window ? location.href : "another page") + ")";
    if (seen.has(value)) return "(shown above)";
    if (depth > 6) return Array.isArray(value) ? "(a list of " + value.length + ")" : "(an object)";
    seen.add(value);
    if (value instanceof Date) return value.toISOString();
    if (value instanceof Error) return String(value.stack || value);
    if (value instanceof Map)
      return Array.from(value)
        .slice(0, 200)
        .map(([key, item]) => [plain(key, depth + 1), plain(item, depth + 1)]);
    if (
      Array.isArray(value) ||
      value instanceof Set ||
      (typeof NodeList === "function" && value instanceof NodeList) ||
      (typeof HTMLCollection === "function" && value instanceof HTMLCollection)
    )
      return Array.from(value)
        .slice(0, 500)
        .map((item) => plain(item, depth + 1));
    const out = {};
    let count = 0;
    for (const key of Object.keys(value)) {
      if (count++ >= 200) {
        out["(more)"] = "keys left out";
        break;
      }
      try {
        out[key] = plain(value[key], depth + 1);
      } catch {
        out[key] = "(unreadable)";
      }
    }
    return out;
  };
  return plain(this, 0);
}"##;

async fn evaluate(
    browser: &Arc<Browser>,
    chat_id: &str,
    args: &Value,
) -> Result<ToolReply, BrowserError> {
    let tab = tab_for(browser, chat_id).await?;
    let code = required(args, "expression")?;
    if code.len() > 200_000 {
        return Err(BrowserError::Invalid("That script is too long.".into()));
    }
    // The page's passwords as they are before the script runs (it might empty a field after
    // reading it); the ones after it are added below.
    let before = page_secrets(browser, &tab).await;
    #[cfg(windows)]
    let value = {
        let call = |method: &'static str, params: Value| {
            let browser = Arc::clone(browser);
            let tab = tab.clone();
            async move { super::webview2::devtools(&browser, &tab, method, params).await }
        };
        // Whether it is a function's body with return statements. A return cannot stand in a
        // class's static block, so parsing it there says so without running anything (a page
        // that forbids eval answers no, and the engine's own answer below still tells).
        let literal = serde_json::to_string(code).unwrap_or_else(|_| "\"\"".into());
        let probe = format!(
            "(() => {{ try {{ new Function(\"class Probe {{ static {{\\n\" + {literal} + \"\\n}} }}\"); return false; }} catch (error) {{ return /Illegal return/.test(String(error)); }} }})()"
        );
        let body = call(
            "Runtime.evaluate",
            json!({ "expression": probe, "returnByValue": true, "userGesture": false }),
        )
        .await
        .ok()
        .and_then(|reply| reply["result"]["value"].as_bool())
        .unwrap_or(false);
        let wrapped = format!("(async () => {{\n{code}\n}})()");
        // Code runs as the console would (top-level await, the last expression's value); a body,
        // as its function, whose promise the console's mode would not wait for.
        let run = |expression: String, console: bool| {
            call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": false,
                    "replMode": console,
                    "objectGroup": AGENT_OBJECTS,
                    // Never a user gesture: one would let page scripts read a password the
                    // browser filled in.
                    "userGesture": false,
                    "timeout": 15000,
                }),
            )
        };
        let illegal_return = |reply: &Value| {
            let details = &reply["exceptionDetails"];
            details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .is_some_and(|text| text.contains("Illegal return"))
        };
        let mut reply = if body {
            run(wrapped.clone(), false).await?
        } else {
            run(code.to_owned(), true).await?
        };
        if !body && illegal_return(&reply) {
            reply = run(wrapped, false).await?;
        }
        if let Some(details) = reply.get("exceptionDetails") {
            let _ = call(
                "Runtime.releaseObjectGroup",
                json!({ "objectGroup": AGENT_OBJECTS }),
            )
            .await;
            let message = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("The script threw.");
            let message: String = message.chars().take(4000).collect();
            return Ok(ToolReply::failed(redact(
                message,
                before.iter().flatten().map(String::as_str),
            )));
        }
        let result = &reply["result"];
        let value = match result["objectId"].as_str() {
            // An object becomes plain data in the page: elements as their tags and text, windows,
            // functions, maps, sets and cycles in words.
            Some(object) => call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object,
                    "functionDeclaration": PLAIN_DATA,
                    "returnByValue": true,
                    "userGesture": false,
                }),
            )
            .await
            .ok()
            .filter(|plain| plain.get("exceptionDetails").is_none())
            .map(|plain| plain["result"]["value"].clone())
            .unwrap_or_else(|| {
                Value::String(
                    result["description"]
                        .as_str()
                        .unwrap_or("an object")
                        .to_owned(),
                )
            }),
            None => match result.get("value") {
                Some(value) => value.clone(),
                None => Value::String(
                    result["unserializableValue"]
                        .as_str()
                        .or_else(|| result["description"].as_str())
                        .unwrap_or(result["type"].as_str().unwrap_or("undefined"))
                        .to_owned(),
                ),
            },
        };
        let _ = call(
            "Runtime.releaseObjectGroup",
            json!({ "objectGroup": AGENT_OBJECTS }),
        )
        .await;
        value
    };
    #[cfg(not(windows))]
    let value = {
        let body = if code.contains("return") {
            code.to_owned()
        } else {
            format!("return ({code});")
        };
        let script = format!(
            "(() => {{ try {{ const value = (() => {{ {body}\n }})(); return JSON.stringify({{ ok: true, value: value === undefined ? null : value }}); }} catch (error) {{ return JSON.stringify({{ error: String((error && error.stack) || error) }}); }} }})()"
        );
        let answer = browser.eval(&tab, script, super::SCRIPT_TIMEOUT).await?;
        if let Some(error) = answer["error"].as_str() {
            let error: String = error.chars().take(4000).collect();
            return Ok(ToolReply::failed(redact(
                error,
                before.iter().flatten().map(String::as_str),
            )));
        }
        answer["value"].clone()
    };
    let mut text = match &value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    // Agents never see a password the page holds (best effort: a value the script changed past
    // recognising, or sent off the page itself, is beyond what Ghost can check).
    let after = page_secrets(browser, &tab).await;
    if before.is_none() && after.is_none() {
        return Ok(ToolReply::failed(
            "Ghost could not check this page's password fields, so the script's result is withheld.",
        ));
    }
    let secrets: Vec<String> = before.into_iter().chain(after).flatten().collect();
    text = redact(text, secrets.iter().map(String::as_str));
    if text.len() > 40_000 {
        let mut end = 40_000;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n… (cut at 40 000 characters)");
    }
    Ok(ToolReply::text(if text.is_empty() {
        "undefined".into()
    } else {
        text
    }))
}

/// The page's password values, so what agents read can leave them out; `None` when the page
/// cannot be asked (no page script runs in it).
async fn page_secrets(browser: &Arc<Browser>, tab: &str) -> Option<Vec<String>> {
    let found = browser.call(tab, "api.secrets()").await.ok()?;
    Some(
        found
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    )
}

/// `text` with every password value of the page (`secrets`) taken out, and a line saying so.
fn redact<'a>(mut text: String, secrets: impl Iterator<Item = &'a str>) -> String {
    let mut hidden = false;
    for secret in secrets {
        if secret.chars().count() >= 4 && text.contains(secret) {
            text = text.replace(secret, "[a password, hidden from agents]");
            hidden = true;
        }
    }
    if hidden {
        text.push_str("\n(Password values are hidden from agents.)");
    }
    text
}

async fn tabs(
    browser: &Arc<Browser>,
    chat_id: &str,
    args: &Value,
) -> Result<ToolReply, BrowserError> {
    let list = |browser: &Browser| {
        let chat = browser.chat(chat_id);
        if chat.tabs.is_empty() {
            return "No tabs are open.".to_owned();
        }
        chat.tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                format!(
                    "{}{index}: {} {}",
                    if chat.active.as_deref() == Some(tab.id.as_str()) {
                        "* "
                    } else {
                        "  "
                    },
                    if tab.title.is_empty() {
                        "(untitled)"
                    } else {
                        tab.title.as_str()
                    },
                    tab.url.as_deref().unwrap_or("(new tab)")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let index = |browser: &Browser| -> Result<String, BrowserError> {
        let chat = browser.chat(chat_id);
        let index = args["index"].as_u64().ok_or_else(|| {
            BrowserError::Invalid("Give the tab's \"index\" from the list.".into())
        })?;
        chat.tabs
            .get(usize::try_from(index).unwrap_or(usize::MAX))
            .map(|tab| tab.id.clone())
            .ok_or_else(|| BrowserError::Invalid(format!("There is no tab {index}.")))
    };
    match str_arg(args, "action").unwrap_or("list") {
        "list" => Ok(ToolReply::text(list(browser))),
        "new" => {
            let url = match str_arg(args, "url") {
                Some(input) => {
                    Some(address::resolve_for_agent(input).map_err(BrowserError::Invalid)?)
                }
                None => None,
            };
            let tab = browser.open_tab(chat_id, url.clone(), true)?;
            if url.is_some() {
                let id = tab.id.clone();
                browser
                    .on_main(move |browser| host::prepare_for_agent(browser, &id))
                    .await??;
                settle(browser, &tab.id, 0, true).await;
            }
            Ok(ToolReply::text(format!("Opened a tab.\n{}", list(browser))))
        }
        "select" => {
            let tab = index(browser)?;
            browser.activate(chat_id, &tab)?;
            Ok(ToolReply::text(list(browser)))
        }
        "close" => {
            let tab = index(browser)?;
            browser.close_tab(&tab)?;
            Ok(ToolReply::text(list(browser)))
        }
        other => Err(BrowserError::Invalid(format!(
            "\"{other}\" is not list, new, select or close."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_say_what_the_agent_does() {
        assert_eq!(
            label("browser_navigate", &json!({ "url": "localhost:3000" })),
            "Opening localhost:3000"
        );
        assert_eq!(
            label("browser_navigate", &json!({ "action": "back" })),
            "Going back"
        );
        assert_eq!(
            label("browser_click", &json!({ "ref": "e12" })),
            "Clicking e12"
        );
        assert_eq!(
            label("browser_press_key", &json!({ "key": "Enter" })),
            "Pressing Enter"
        );
        assert_eq!(label("unknown", &json!({})), "Using the browser");
    }

    #[test]
    fn page_passwords_never_reach_an_agent() {
        let text = redact(
            r#"{"user":"ana","pw":"hunter22"}"#.into(),
            ["hunter22", "ab"].into_iter(),
        );
        assert!(!text.contains("hunter22"));
        assert!(text.contains("hidden from agents"));
        // Too short to tell from ordinary text, and absent ones change nothing.
        assert_eq!(
            redact("ab cd".into(), ["ab", "zzzzzz"].into_iter()),
            "ab cd"
        );
    }

    #[test]
    fn strings_become_script_literals() {
        assert_eq!(js(&Some("a\"b</script>")), "\"a\\\"b</script>\"");
        assert_eq!(js(&None), "null");
    }
}
