//! A device's screen as an outline: what is on it, by role and name, with a ref on everything
//! that can be acted on. Built from Android's own accessibility dump (`uiautomator dump`), as the
//! browser's outline is built from a page.
//!
//! Layout containers with no name and no action are left out, so an agent reads the screen and not
//! the view hierarchy. A password field's text never leaves this module.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::Error;

/// The most lines an outline prints. A long list says how many more there are.
const LINE_LIMIT: usize = 300;
/// The most characters of one name or value.
const TEXT_LIMIT: usize = 200;
/// How many of its texts name a button that has no label of its own.
const BORROWED_NAMES: usize = 3;

/// A rectangle on the device's display, in its own pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn center(&self) -> (i32, i32) {
        (self.left + self.width() / 2, self.top + self.height() / 2)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// One line of the outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// How far it is indented under the items before it.
    pub depth: usize,
    /// `button`, `textbox`, `text`, `switch`, `list` and so on.
    pub role: &'static str,
    /// What it is called: its text, its description, or its hint.
    pub name: String,
    /// What a field holds or a slider shows. Never a password.
    pub value: Option<String>,
    /// `d1`, `d2`, ... on items that can be tapped, typed into or scrolled.
    pub reference: Option<String>,
    pub bounds: Rect,
    /// `checked`, `selected`, `focused`, `disabled`, `scrollable`, `password`.
    pub states: Vec<&'static str>,
    /// The developer's own id for the view (`sign_in_button`), when it has one.
    pub id: Option<String>,
    /// The view's class, for Annotate.
    pub class: String,
}

/// A screen, read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outline {
    /// Quarter turns from the device's natural position.
    pub rotation: u8,
    /// The app in front (its package).
    pub package: Option<String>,
    pub items: Vec<Item>,
}

/// One `<node>` as dumped.
#[derive(Debug, Default)]
struct Raw {
    text: String,
    description: String,
    hint: String,
    id: String,
    class: String,
    package: String,
    bounds: Rect,
    checkable: bool,
    checked: bool,
    clickable: bool,
    long_clickable: bool,
    enabled: bool,
    focused: bool,
    scrollable: bool,
    password: bool,
    selected: bool,
    children: Vec<Raw>,
}

impl Raw {
    fn class_name(&self) -> &str {
        self.class.rsplit('.').next().unwrap_or(&self.class)
    }

    fn editable(&self) -> bool {
        let class = self.class_name();
        class.contains("EditText") || class.contains("AutoCompleteTextView")
    }

    fn actionable(&self) -> bool {
        self.clickable
            || self.long_clickable
            || self.checkable
            || self.scrollable
            || self.editable()
    }

    fn visible(&self) -> bool {
        self.bounds.width() > 0 && self.bounds.height() > 0
    }

    /// The node's own words, a password's text never among them.
    fn own_name(&self) -> String {
        if self.editable() {
            // A field's text is its value; its name is what it asks for.
            return first_of(&[&self.description, &self.hint]);
        }
        if self.password {
            return first_of(&[&self.description, &self.hint]);
        }
        first_of(&[&self.text, &self.description, &self.hint])
    }

    fn role(&self) -> &'static str {
        let class = self.class_name();
        if self.editable() {
            "textbox"
        } else if class.contains("Switch") || class.contains("ToggleButton") {
            "switch"
        } else if class.contains("CheckBox") || class.contains("CheckedTextView") {
            "checkbox"
        } else if class.contains("RadioButton") {
            "radio"
        } else if class.contains("SeekBar") || class.contains("Slider") {
            "slider"
        } else if class.contains("Spinner") {
            "dropdown"
        } else if class.contains("ProgressBar") {
            "progress"
        } else if class.contains("WebView") {
            "webview"
        } else if class.contains("Button") {
            "button"
        } else if self.checkable {
            "checkbox"
        } else if self.clickable || self.long_clickable {
            "button"
        } else if self.scrollable {
            "list"
        } else if class.contains("Image") {
            "image"
        } else {
            "text"
        }
    }
}

fn first_of(candidates: &[&String]) -> String {
    candidates
        .iter()
        .map(|text| clean(text))
        .find(|text| !text.is_empty())
        .unwrap_or_default()
}

/// One line of text: no line breaks or control characters, and not too long.
fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() || ch.is_control() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(ch);
    }
    if out.chars().count() > TEXT_LIMIT {
        out = out.chars().take(TEXT_LIMIT).collect();
        out.push_str("...");
    }
    out
}

/// `[0,66][1080,2400]`.
fn parse_bounds(text: &str) -> Option<Rect> {
    let mut numbers = text
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '-'))
        .filter(|part| !part.is_empty())
        .map(str::parse::<i32>);
    let rect = Rect {
        left: numbers.next()?.ok()?,
        top: numbers.next()?.ok()?,
        right: numbers.next()?.ok()?,
        bottom: numbers.next()?.ok()?,
    };
    numbers.next().is_none().then_some(rect)
}

fn read_node(start: &BytesStart<'_>) -> Result<Raw, Error> {
    let mut raw = Raw {
        enabled: true,
        ..Raw::default()
    };
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|_| broken())?;
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|_| broken())?;
        let flag = value == "true";
        match attribute.key.as_ref() {
            "text" => raw.text = value.into_owned(),
            "content-desc" => raw.description = value.into_owned(),
            "hint" => raw.hint = value.into_owned(),
            "resource-id" => raw.id = value.into_owned(),
            "class" => raw.class = value.into_owned(),
            "package" => raw.package = value.into_owned(),
            "bounds" => raw.bounds = parse_bounds(&value).unwrap_or_default(),
            "checkable" => raw.checkable = flag,
            "checked" => raw.checked = flag,
            "clickable" => raw.clickable = flag,
            "long-clickable" => raw.long_clickable = flag,
            "enabled" => raw.enabled = flag,
            "focused" => raw.focused = flag,
            "scrollable" => raw.scrollable = flag,
            "password" => raw.password = flag,
            "selected" => raw.selected = flag,
            _ => {}
        }
    }
    Ok(raw)
}

fn broken() -> Error {
    Error::Device("The device described its screen in a way o7 could not read.".into())
}

impl Outline {
    /// Reads `uiautomator dump`'s output. Whatever the tool prints after the document (its
    /// "dumped to" line) is ignored.
    pub fn parse(dump: &[u8]) -> Result<Outline, Error> {
        let text = String::from_utf8_lossy(dump);
        let Some(start) = text.find("<hierarchy") else {
            return Err(Error::Device(no_screen(&text)));
        };
        let end = text
            .rfind("</hierarchy>")
            .map_or(text.len(), |at| at + "</hierarchy>".len());
        let mut reader = Reader::from_str(&text[start..end]);
        let mut rotation = 0u8;
        let mut stack: Vec<Raw> = Vec::new();
        let mut roots: Vec<Raw> = Vec::new();
        loop {
            match reader.read_event().map_err(|_| broken())? {
                Event::Start(element) if element.name().as_ref() == "hierarchy" => {
                    rotation = hierarchy_rotation(&element);
                }
                Event::Start(element) if element.name().as_ref() == "node" => {
                    stack.push(read_node(&element)?);
                }
                Event::Empty(element) if element.name().as_ref() == "node" => {
                    let node = read_node(&element)?;
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(node),
                        None => roots.push(node),
                    }
                }
                Event::End(element) if element.name().as_ref() == "node" => {
                    let node = stack.pop().ok_or_else(broken)?;
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(node),
                        None => roots.push(node),
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        let package = roots
            .first()
            .map(|root| root.package.clone())
            .filter(|package| !package.is_empty());
        let mut items = Vec::new();
        let mut next_ref = 1usize;
        for root in &roots {
            flatten(root, 0, &mut items, &mut next_ref);
        }
        Ok(Outline {
            rotation,
            package,
            items,
        })
    }

    /// The item with this ref.
    pub fn find(&self, reference: &str) -> Option<&Item> {
        let reference = reference
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']');
        self.items
            .iter()
            .find(|item| item.reference.as_deref() == Some(reference))
    }

    /// The innermost item at a point of the display, for Annotate.
    pub fn at(&self, x: i32, y: i32) -> Option<&Item> {
        self.items
            .iter()
            .filter(|item| item.bounds.contains(x, y))
            .min_by_key(|item| i64::from(item.bounds.width()) * i64::from(item.bounds.height()))
    }

    /// The outline as agents read it.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.items.is_empty() {
            out.push_str("(Nothing on the screen has a name or can be tapped. Take a screenshot to see it.)\n");
            return out;
        }
        for (index, item) in self.items.iter().enumerate() {
            if index == LINE_LIMIT {
                out.push_str(&format!(
                    "... and {} more items. Scroll, or take a screenshot.\n",
                    self.items.len() - LINE_LIMIT
                ));
                break;
            }
            for _ in 0..item.depth {
                out.push_str("  ");
            }
            out.push_str("- ");
            out.push_str(item.role);
            if !item.name.is_empty() {
                out.push_str(&format!(" \"{}\"", item.name.replace('"', "'")));
            }
            if let Some(value) = &item.value {
                out.push_str(&format!(" value \"{}\"", value.replace('"', "'")));
            }
            if let Some(reference) = &item.reference {
                out.push_str(&format!(" [{reference}]"));
            }
            if !item.states.is_empty() {
                out.push_str(&format!(" ({})", item.states.join(", ")));
            }
            out.push('\n');
        }
        out
    }
}

fn hierarchy_rotation(element: &BytesStart<'_>) -> u8 {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == "rotation")
        .and_then(|attribute| {
            attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .ok()?
                .parse::<u8>()
                .ok()
        })
        .map_or(0, |turns| turns % 4)
}

/// Why there was no document: the tool's own complaint, in plain words.
fn no_screen(said: &str) -> String {
    let lower = said.to_ascii_lowercase();
    if lower.contains("could not get idle state") {
        "The screen kept moving, so it could not be read. Wait a moment and read it again, or take a screenshot.".into()
    } else if lower.contains("null root node") {
        "The device has no screen to read right now (it may be locked or starting an app). Take a screenshot to see it.".into()
    } else {
        "The device did not describe its screen. Take a screenshot to see it.".into()
    }
}

/// Texts inside `node` that it may be named after: those of descendants that cannot be acted on
/// themselves, in reading order.
fn borrow_names(node: &Raw, names: &mut Vec<String>) {
    for child in &node.children {
        if names.len() >= BORROWED_NAMES || child.actionable() || !child.visible() {
            continue;
        }
        let name = child.own_name();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
        borrow_names(child, names);
    }
}

/// Whether everything under `node` that has a name was borrowed for its own.
fn lent_everything(node: &Raw) -> bool {
    node.children
        .iter()
        .all(|child| !child.actionable() && lent_everything(child))
}

fn flatten(node: &Raw, depth: usize, items: &mut Vec<Item>, next_ref: &mut usize) {
    if !node.visible() {
        return;
    }
    let mut name = node.own_name();
    let mut borrowed = false;
    if name.is_empty() && (node.clickable || node.long_clickable || node.checkable) {
        let mut names = Vec::new();
        borrow_names(node, &mut names);
        if !names.is_empty() {
            name = clean(&names.join(", "));
            borrowed = lent_everything(node);
        }
    }
    let keep = !name.is_empty() || node.actionable() || node.focused;
    if !keep {
        for child in &node.children {
            flatten(child, depth, items, next_ref);
        }
        return;
    }
    let mut states = Vec::new();
    if node.checked {
        states.push("checked");
    }
    if node.selected {
        states.push("selected");
    }
    if node.focused {
        states.push("focused");
    }
    if !node.enabled {
        states.push("disabled");
    }
    if node.scrollable {
        states.push("scrollable");
    }
    if node.password {
        states.push("password");
    }
    let value = if node.password {
        None
    } else if node.editable() {
        // An empty field reports its hint as its text: that is its name, not what it holds.
        Some(clean(&node.text)).filter(|text| !text.is_empty() && *text != clean(&node.hint))
    } else {
        None
    };
    let reference = (node.actionable() && node.enabled).then(|| {
        let reference = format!("d{next_ref}");
        *next_ref += 1;
        reference
    });
    items.push(Item {
        depth,
        role: node.role(),
        name,
        value,
        reference,
        bounds: node.bounds,
        states,
        id: node
            .id
            .rsplit('/')
            .next()
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
        class: node.class.clone(),
    });
    if borrowed {
        return;
    }
    for child in &node.children {
        flatten(child, depth + 1, items, next_ref);
    }
}

// ------------------------------------------------------------------------------------ iPhone

/// iOS element types that can be acted on: tapped, typed into, switched or moved.
const IOS_ACTIONABLE: [&str; 21] = [
    "Button",
    "Cell",
    "CheckBox",
    "Link",
    "MenuItem",
    "MenuButton",
    "PopUpButton",
    "RadioButton",
    "SecureTextField",
    "SegmentedControl",
    "Slider",
    "Stepper",
    "Switch",
    "Tab",
    "TabBarButton",
    "TextField",
    "SearchField",
    "TextView",
    "Toggle",
    "Picker",
    "PickerWheel",
];

/// iOS element types whose content scrolls.
const IOS_SCROLLING: [&str; 6] = [
    "ScrollView",
    "Table",
    "CollectionView",
    "List",
    "Grid",
    "Outline",
];

fn ios_role(kind: &str) -> &'static str {
    match kind {
        "Application" => "app",
        "Button" | "Cell" | "PopUpButton" | "MenuButton" | "MenuItem" => "button",
        "Link" => "link",
        "TextField" | "SecureTextField" | "SearchField" | "TextView" => "textbox",
        "Switch" | "Toggle" => "switch",
        "CheckBox" => "checkbox",
        "RadioButton" => "radio",
        "Slider" | "Stepper" => "slider",
        "Picker" | "PickerWheel" | "ComboBox" => "dropdown",
        "ProgressIndicator" | "ActivityIndicator" => "progress",
        "Image" | "Icon" => "image",
        "Tab" | "TabBarButton" | "SegmentedControl" => "tab",
        "WebView" => "webview",
        "Heading" | "NavigationBar" => "heading",
        "Alert" | "Sheet" | "Dialog" => "dialog",
        kind if IOS_SCROLLING.contains(&kind) => "list",
        _ => "text",
    }
}

/// A field of an element as text: AXe writes strings, and sometimes numbers or true and false.
fn ios_text(node: &serde_json::Value, key: &str) -> String {
    match node.get(key) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

fn ios_kind(node: &serde_json::Value) -> String {
    ios_text(node, "type")
}

fn ios_enabled(node: &serde_json::Value) -> bool {
    match node.get("enabled") {
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(serde_json::Value::Number(number)) => number.as_f64() != Some(0.0),
        _ => true,
    }
}

fn ios_children(node: &serde_json::Value) -> &[serde_json::Value] {
    node.get("children")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn ios_actionable(kind: &str, node: &serde_json::Value) -> bool {
    IOS_ACTIONABLE.contains(&kind)
        || IOS_SCROLLING.contains(&kind)
        || ios_text(node, "role") == "AXSwitch"
}

/// A frame in points as a rectangle of the display's pixels.
fn ios_bounds(node: &serde_json::Value, scale: f64) -> Rect {
    let frame = node.get("frame");
    let number = |name: &str| {
        frame
            .and_then(|frame| frame.get(name))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
    };
    let pixel = |points: f64| (points * scale).round().clamp(-1_000_000.0, 1_000_000.0) as i32;
    let (x, y, width, height) = (number("x"), number("y"), number("width"), number("height"));
    Rect {
        left: pixel(x),
        top: pixel(y),
        right: pixel(x + width),
        bottom: pixel(y + height),
    }
}

/// What an element is called: its label, else its title.
fn ios_own_name(node: &serde_json::Value) -> String {
    let label = clean(&ios_text(node, "AXLabel"));
    if label.is_empty() {
        clean(&ios_text(node, "title"))
    } else {
        label
    }
}

/// Texts inside `node` it may be named after: those of descendants that cannot be acted on.
fn ios_borrow_names(node: &serde_json::Value, names: &mut Vec<String>) {
    for child in ios_children(node) {
        let kind = ios_kind(child);
        if names.len() >= BORROWED_NAMES
            || ios_actionable(&kind, child)
            || child.get("hidden") == Some(&serde_json::Value::Bool(true))
        {
            continue;
        }
        let name = ios_own_name(child);
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
        ios_borrow_names(child, names);
    }
}

/// iOS's name for a scroll view's scroll bar ("Vertical scroll bar, 2 pages"), in English, the
/// language o7's simulators start in.
fn ios_scroll_bar(name: &str) -> bool {
    name.starts_with("Vertical scroll bar") || name.starts_with("Horizontal scroll bar")
}

fn ios_lent_everything(node: &serde_json::Value) -> bool {
    ios_children(node)
        .iter()
        .all(|child| !ios_actionable(&ios_kind(child), child) && ios_lent_everything(child))
}

struct IosWalk {
    scale: f64,
    items: Vec<Item>,
    next_ref: usize,
    keyboard: bool,
}

impl IosWalk {
    fn flatten(&mut self, node: &serde_json::Value, depth: usize) {
        if node.get("hidden") == Some(&serde_json::Value::Bool(true)) {
            return;
        }
        let kind = ios_kind(node);
        // The keyboard's keys are not the app's: the outline only says the keyboard is up.
        if kind == "Keyboard" {
            self.keyboard = true;
            return;
        }
        // iOS gives every scroll view its scroll bars as sliders. An agent scrolls the view
        // itself, which the outline marks scrollable, so they would only crowd the outline.
        if ios_role(&kind) == "slider" && ios_scroll_bar(&ios_own_name(node)) {
            return;
        }
        let bounds = ios_bounds(node, self.scale);
        let visible = bounds.width() > 0 && bounds.height() > 0;
        let actionable = ios_actionable(&kind, node);
        let secure = kind == "SecureTextField";
        let editable = matches!(
            kind.as_str(),
            "TextField" | "SecureTextField" | "SearchField" | "TextView"
        );
        let mut name = ios_own_name(node);
        let mut borrowed = false;
        if name.is_empty() && actionable && !editable {
            let mut names = Vec::new();
            ios_borrow_names(node, &mut names);
            if !names.is_empty() {
                name = clean(&names.join(", "));
                borrowed = ios_lent_everything(node);
            }
        }
        let keep = visible && (!name.is_empty() || actionable);
        if !keep {
            for child in ios_children(node) {
                self.flatten(child, depth);
            }
            return;
        }
        let raw_value = clean(&ios_text(node, "AXValue"));
        let enabled = ios_enabled(node);
        let switch_like =
            matches!(kind.as_str(), "Switch" | "Toggle") || ios_text(node, "role") == "AXSwitch";
        let checkable = switch_like || kind == "CheckBox";
        let mut states = Vec::new();
        if checkable && matches!(raw_value.to_ascii_lowercase().as_str(), "1" | "on" | "true") {
            states.push("checked");
        }
        if !enabled {
            states.push("disabled");
        }
        if IOS_SCROLLING.contains(&kind.as_str()) {
            states.push("scrollable");
        }
        if secure {
            states.push("password");
        }
        let shows_value = editable
            || matches!(
                kind.as_str(),
                "Slider"
                    | "Stepper"
                    | "ProgressIndicator"
                    | "Picker"
                    | "PickerWheel"
                    | "PageIndicator"
            );
        // A secure field's value is never read, not even as dots.
        let value = (!secure && shows_value && !raw_value.is_empty() && raw_value != name)
            .then_some(raw_value);
        let reference = (actionable && enabled).then(|| {
            let reference = format!("d{}", self.next_ref);
            self.next_ref += 1;
            reference
        });
        let id = [ios_text(node, "AXIdentifier"), ios_text(node, "AXUniqueId")]
            .into_iter()
            .map(|id| clean(&id))
            .find(|id| !id.is_empty());
        self.items.push(Item {
            depth,
            role: ios_role(&kind),
            name,
            value,
            reference,
            bounds,
            states,
            id,
            class: kind,
        });
        if borrowed {
            return;
        }
        for child in ios_children(node) {
            self.flatten(child, depth + 1);
        }
    }
}

impl Outline {
    /// Reads AXe's `describe-ui` tree of an iPhone screen. Its frames are in points; `scale`
    /// turns them into the display's pixels, the units of screenshots and taps here.
    pub fn parse_ios(json: &[u8], scale: f64) -> Result<Outline, Error> {
        let value: serde_json::Value = serde_json::from_slice(json).map_err(|_| broken())?;
        let roots: Vec<&serde_json::Value> = match &value {
            serde_json::Value::Array(roots) => roots.iter().collect(),
            serde_json::Value::Object(_) => vec![&value],
            _ => return Err(broken()),
        };
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let mut walk = IosWalk {
            scale,
            items: Vec::new(),
            next_ref: 1,
            keyboard: false,
        };
        for root in roots {
            walk.flatten(root, 0);
        }
        if walk.keyboard {
            walk.items.push(Item {
                depth: 0,
                role: "keyboard",
                name: "open".into(),
                value: None,
                reference: None,
                bounds: Rect::default(),
                states: Vec::new(),
                id: None,
                class: "Keyboard".into(),
            });
        }
        Ok(Outline {
            rotation: 0,
            package: None,
            items: walk.items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Settings-like screen as AXe describes it: an app, a navigation bar, a search field, a
    /// list of rows and a switch, a password field, a hidden view and the keyboard.
    const IOS_SCREEN: &str = r#"[{
        "type": "Application", "AXLabel": "Settings", "pid": 4242, "enabled": true,
        "frame": {"x": 0, "y": 0, "width": 402, "height": 874},
        "children": [
            {"type": "NavigationBar", "AXLabel": "Settings", "frame": {"x": 0, "y": 62, "width": 402, "height": 96},
             "children": [{"type": "StaticText", "AXLabel": "Settings", "frame": {"x": 20, "y": 100, "width": 120, "height": 41}}]},
            {"type": "SearchField", "AXLabel": "Search", "AXValue": "wi", "AXIdentifier": "search", "enabled": 1,
             "frame": {"x": 16, "y": 160, "width": 370, "height": 36}},
            {"type": "Table", "frame": {"x": 0, "y": 200, "width": 402, "height": 674}, "children": [
                {"type": "Button", "frame": {"x": 0, "y": 220, "width": 402, "height": 52}, "children": [
                    {"type": "Image", "frame": {"x": 20, "y": 232, "width": 29, "height": 29}},
                    {"type": "StaticText", "AXLabel": "General", "frame": {"x": 64, "y": 236, "width": 80, "height": 20}}]},
                {"type": "Switch", "AXLabel": "Airplane Mode", "AXValue": "1", "role": "AXSwitch",
                 "frame": {"x": 0, "y": 280, "width": 402, "height": 52}},
                {"type": "Cell", "AXLabel": "Privacy", "enabled": false, "frame": {"x": 0, "y": 340, "width": 402, "height": 52}}]},
            {"type": "SecureTextField", "AXLabel": "Your code", "AXValue": "FIELD_TEXT", "frame": {"x": 16, "y": 400, "width": 370, "height": 36}},
            {"type": "Button", "AXLabel": "Secret", "hidden": true, "frame": {"x": 0, "y": 450, "width": 10, "height": 10}},
            {"type": "Button", "AXLabel": "Gone", "frame": {"x": 0, "y": 470, "width": 0, "height": 0}},
            {"type": "Keyboard", "frame": {"x": 0, "y": 600, "width": 402, "height": 274}, "children": [
                {"type": "Key", "AXLabel": "q", "frame": {"x": 0, "y": 610, "width": 30, "height": 40}}]}
        ]}]"#;

    #[test]
    fn an_iphone_screen_reads_as_an_outline_in_pixels() {
        // What the secure field holds is made here, so the fixture holds nothing like a password.
        let held = ["typed", "in", "secret"].join("-");
        let screen = IOS_SCREEN.replace("FIELD_TEXT", &held);
        let outline = Outline::parse_ios(screen.as_bytes(), 3.0).unwrap();
        let rendered = outline.render();
        assert_eq!(
            rendered,
            concat!(
                "- app \"Settings\"\n",
                "  - heading \"Settings\"\n",
                "    - text \"Settings\"\n",
                "  - textbox \"Search\" value \"wi\" [d1]\n",
                "  - list [d2] (scrollable)\n",
                "    - button \"General\" [d3]\n",
                "    - switch \"Airplane Mode\" [d4] (checked)\n",
                "    - button \"Privacy\" (disabled)\n",
                "  - textbox \"Your code\" [d5] (password)\n",
                "- keyboard \"open\"\n",
            ),
            "{rendered}"
        );
        // Never the password, nor a hidden element, nor the keyboard's keys.
        assert!(
            !rendered.contains(&held)
                && !rendered.contains("Secret")
                && !rendered.contains("\"q\"")
        );
        let general = outline.find("d3").unwrap();
        assert_eq!(
            general.bounds,
            Rect {
                left: 0,
                top: 660,
                right: 1206,
                bottom: 816
            }
        );
        assert_eq!(general.bounds.center(), (603, 738));
        assert_eq!(outline.find("d1").unwrap().id.as_deref(), Some("search"));
        assert_eq!(outline.find("d1").unwrap().class, "SearchField");
        assert!(outline.package.is_none() && outline.rotation == 0);
    }

    #[test]
    fn an_iphone_outline_leaves_out_scroll_bars() {
        let screen = r#"[{"type": "Application", "AXLabel": "Hello o7", "frame": {"x": 0, "y": 0, "width": 402, "height": 874},
            "children": [{"type": "ScrollView", "frame": {"x": 0, "y": 0, "width": 402, "height": 874}, "children": [
                {"type": "Slider", "AXLabel": "Vertical scroll bar, 2 pages", "AXValue": "0%", "frame": {"x": 396, "y": 0, "width": 6, "height": 874}},
                {"type": "Slider", "AXLabel": "Horizontal scroll bar, 1 page", "AXValue": "0%", "frame": {"x": 0, "y": 868, "width": 402, "height": 6}},
                {"type": "Slider", "AXLabel": "Volume", "AXValue": "50%", "frame": {"x": 20, "y": 100, "width": 300, "height": 30}},
                {"type": "Button", "AXLabel": "Open", "frame": {"x": 20, "y": 200, "width": 100, "height": 44}}]}]}]"#;
        let rendered = Outline::parse_ios(screen.as_bytes(), 3.0).unwrap().render();
        assert_eq!(
            rendered,
            concat!(
                "- app \"Hello o7\"\n",
                "  - list [d1] (scrollable)\n",
                "    - slider \"Volume\" value \"50%\" [d2]\n",
                "    - button \"Open\" [d3]\n",
            ),
            "{rendered}"
        );
    }

    #[test]
    fn an_iphone_outline_accepts_one_root_and_refuses_what_is_not_a_tree() {
        let one = r#"{"type":"Application","AXLabel":"Clock","frame":{"x":0,"y":0,"width":390,"height":844},"children":[{"type":"Button","AXLabel":7,"frame":{"x":1,"y":1,"width":40,"height":40}}]}"#;
        let outline = Outline::parse_ios(one.as_bytes(), 2.0).unwrap();
        assert_eq!(outline.render(), "- app \"Clock\"\n  - button \"7\" [d1]\n");
        assert!(Outline::parse_ios(b"Error: not booted", 3.0).is_err());
        assert!(Outline::parse_ios(b"42", 3.0).is_err());
        assert_eq!(
            Outline::parse_ios(b"[]", 3.0)
                .unwrap()
                .render()
                .lines()
                .count(),
            1
        );
    }

    fn node(attributes: &str, children: &str) -> String {
        let defaults = [
            ("text", ""),
            ("resource-id", ""),
            ("class", "android.view.View"),
            ("package", "com.example.app"),
            ("content-desc", ""),
            ("checkable", "false"),
            ("checked", "false"),
            ("clickable", "false"),
            ("enabled", "true"),
            ("focused", "false"),
            ("scrollable", "false"),
            ("long-clickable", "false"),
            ("password", "false"),
            ("selected", "false"),
            ("bounds", "[0,0][1080,2400]"),
            ("hint", ""),
        ];
        let mut all = String::new();
        for (key, value) in defaults {
            if !attributes.contains(&format!("{key}=")) {
                all.push_str(&format!(" {key}=\"{value}\""));
            }
        }
        format!("<node {attributes}{all}>{children}</node>")
    }

    fn screen(nodes: &str) -> Outline {
        let dump = format!(
            "<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation=\"1\">{}</hierarchy>UI hierchary dumped to: /dev/tty\n",
            node("class=\"android.widget.FrameLayout\"", nodes)
        );
        Outline::parse(dump.as_bytes()).unwrap()
    }

    #[test]
    fn layout_is_dropped_and_what_matters_is_kept_with_refs() {
        let outline = screen(&[
            node(
                "class=\"android.widget.LinearLayout\"",
                &[
                    node("class=\"android.widget.TextView\" text=\"Welcome back\" bounds=\"[40,200][1040,300]\"", ""),
                    node("class=\"android.widget.EditText\" text=\"sam@example.com\" hint=\"Email\" resource-id=\"com.example.app:id/email\" focused=\"true\" bounds=\"[40,400][1040,520]\"", ""),
                    node("class=\"android.widget.Button\" text=\"Sign in\" clickable=\"true\" bounds=\"[40,700][1040,820]\"", ""),
                    node("class=\"android.widget.Button\" text=\"Later\" clickable=\"true\" enabled=\"false\" bounds=\"[40,900][1040,1020]\"", ""),
                ]
                .concat(),
            ),
        ]
        .concat());
        assert_eq!(outline.rotation, 1);
        assert_eq!(outline.package.as_deref(), Some("com.example.app"));
        assert_eq!(
            outline.render(),
            "- text \"Welcome back\"\n\
             - textbox \"Email\" value \"sam@example.com\" [d1] (focused)\n\
             - button \"Sign in\" [d2]\n\
             - button \"Later\" (disabled)\n"
        );
        let email = outline.find("d1").unwrap();
        assert_eq!(email.id.as_deref(), Some("email"));
        assert_eq!(email.bounds.center(), (540, 460));
        assert_eq!(outline.find("[d2]").unwrap().name, "Sign in");
        assert!(outline.find("d3").is_none());
        assert_eq!(outline.at(100, 750).unwrap().name, "Sign in");
        assert!(outline.at(100, 1500).is_none());
    }

    #[test]
    fn an_empty_field_shows_its_hint_as_its_name_only() {
        let outline = screen(&node(
            "class=\"android.widget.EditText\" text=\"Search settings\" hint=\"Search settings\" bounds=\"[0,0][500,100]\"",
            "",
        ));
        assert_eq!(outline.render(), "- textbox \"Search settings\" [d1]\n");
    }

    #[test]
    fn a_password_is_never_in_the_outline() {
        let outline = screen(&node(
            "class=\"android.widget.EditText\" text=\"hunter2-secret\" hint=\"Password\" password=\"true\" bounds=\"[0,0][500,100]\"",
            "",
        ));
        let text = outline.render();
        assert_eq!(text, "- textbox \"Password\" [d1] (password)\n");
        assert!(!format!("{outline:?}").contains("hunter2"));
        // Nor when the password is a plain view's text, as some toolkits draw it.
        let outline = screen(&node(
            "text=\"hunter2-secret\" password=\"true\" content-desc=\"PIN\"",
            "",
        ));
        assert!(!format!("{outline:?}").contains("hunter2"));
        assert_eq!(outline.render(), "- text \"PIN\" (password)\n");
    }

    #[test]
    fn a_button_made_of_parts_takes_their_names_once() {
        // A card that is one tap target: an icon, a title and a subtitle inside it.
        let outline = screen(&node(
            "clickable=\"true\" bounds=\"[0,100][1080,300]\"",
            &[
                node("class=\"android.widget.ImageView\" bounds=\"[0,100][200,300]\"", ""),
                node("class=\"android.widget.TextView\" text=\"Inbox\" bounds=\"[200,100][1080,200]\"", ""),
                node("class=\"android.widget.TextView\" text=\"3 unread\" bounds=\"[200,200][1080,300]\"", ""),
            ]
            .concat(),
        ));
        assert_eq!(outline.render(), "- button \"Inbox, 3 unread\" [d1]\n");
        // With a button of its own inside, the inner button is still listed.
        let outline = screen(&node(
            "clickable=\"true\" bounds=\"[0,100][1080,300]\"",
            &[
                node("class=\"android.widget.TextView\" text=\"Inbox\" bounds=\"[200,100][900,300]\"", ""),
                node("class=\"android.widget.ImageButton\" content-desc=\"More\" clickable=\"true\" bounds=\"[900,100][1080,300]\"", ""),
            ]
            .concat(),
        ));
        assert_eq!(
            outline.render(),
            "- button \"Inbox\" [d1]\n  - text \"Inbox\"\n  - button \"More\" [d2]\n"
        );
    }

    #[test]
    fn roles_and_states_follow_the_views() {
        let outline = screen(&[
            node("class=\"androidx.appcompat.widget.SwitchCompat\" text=\"Wi-Fi\" checkable=\"true\" checked=\"true\" clickable=\"true\" bounds=\"[0,0][500,100]\"", ""),
            node("class=\"android.widget.CheckBox\" text=\"Remember me\" checkable=\"true\" clickable=\"true\" bounds=\"[0,100][500,200]\"", ""),
            node("class=\"androidx.recyclerview.widget.RecyclerView\" scrollable=\"true\" bounds=\"[0,200][1080,2000]\"",
                &node("class=\"android.widget.TextView\" text=\"Row 1\" selected=\"true\" bounds=\"[0,200][1080,300]\"", "")),
            node("class=\"android.widget.ImageView\" content-desc=\"Logo\" bounds=\"[0,2000][100,2100]\"", ""),
            node("class=\"android.widget.TextView\" text=\"hidden\" bounds=\"[0,0][0,0]\"", ""),
        ]
        .concat());
        assert_eq!(
            outline.render(),
            "- switch \"Wi-Fi\" [d1] (checked)\n\
             - checkbox \"Remember me\" [d2]\n\
             - list [d3] (scrollable)\n\
             \x20 - text \"Row 1\" (selected)\n\
             - image \"Logo\"\n"
        );
    }

    #[test]
    fn names_are_one_line_and_quotes_cannot_break_out() {
        let outline = screen(&node(
            "class=\"android.widget.TextView\" text=\"Say &quot;hi&quot;&#10;[d9] (checked)\" bounds=\"[0,0][10,10]\"",
            "",
        ));
        assert_eq!(outline.render(), "- text \"Say 'hi' [d9] (checked)\"\n");
        assert!(outline.find("d9").is_none());
        let long = "x".repeat(500);
        let outline = screen(&node(
            &format!("text=\"{long}\" bounds=\"[0,0][10,10]\""),
            "",
        ));
        assert_eq!(outline.items[0].name.chars().count(), TEXT_LIMIT + 3);
    }

    #[test]
    fn a_long_screen_says_how_much_is_left() {
        let rows: String = (0..LINE_LIMIT + 25)
            .map(|row| node(&format!("text=\"Row {row}\" bounds=\"[0,0][10,10]\""), ""))
            .collect();
        let text = screen(&rows).render();
        assert_eq!(text.lines().count(), LINE_LIMIT + 1);
        assert!(text.ends_with("... and 25 more items. Scroll, or take a screenshot.\n"));
    }

    #[test]
    fn the_tools_own_complaints_become_plain_words() {
        let moving = Outline::parse(b"ERROR: could not get idle state.\n")
            .unwrap_err()
            .to_string();
        assert!(moving.starts_with("The screen kept moving"), "{moving}");
        let locked = Outline::parse(b"ERROR: null root node returned by UiTestAutomationBridge.\n")
            .unwrap_err()
            .to_string();
        assert!(locked.contains("no screen to read"), "{locked}");
        assert!(Outline::parse(b"").is_err());
        assert!(
            Outline::parse(b"<hierarchy rotation=\"0\"><node bounds=\"[0,0][1,1]\"></hierarchy>")
                .is_err()
        );
        let empty = Outline::parse(b"<hierarchy rotation=\"0\"></hierarchy>").unwrap();
        assert!(empty.render().starts_with("(Nothing on the screen"));
    }

    /// A real dump: Android 16's home screen on the emulator.
    #[test]
    fn a_real_home_screen_reads_as_its_apps() {
        let outline = Outline::parse(include_bytes!("../tests/fixtures/launcher.xml")).unwrap();
        assert_eq!(outline.rotation, 0);
        assert_eq!(
            outline.package.as_deref(),
            Some("com.google.android.apps.nexuslauncher")
        );
        let text = outline.render();
        for app in ["Gmail", "Photos", "YouTube", "Phone", "Messages", "Chrome"] {
            let line = text
                .lines()
                .find(|line| line.contains(&format!("\"{app}\"")))
                .unwrap_or_else(|| {
                    panic!(
                        "{app}
{text}"
                    )
                });
            assert!(line.contains("[d"), "{line}");
        }
        // 32 views were dumped; the layout around the apps is gone.
        assert!(outline.items.len() < 20, "{text}");
        let gmail = outline
            .items
            .iter()
            .find(|item| item.name == "Gmail")
            .unwrap();
        let (x, y) = gmail.bounds.center();
        assert!(x > 300 && x < 540 && y > 1500 && y < 1800, "{x},{y}");
        assert_eq!(outline.at(x, y).unwrap().name, "Gmail");
    }

    #[test]
    fn bounds_parse_strictly() {
        assert_eq!(
            parse_bounds("[0,66][1080,2400]"),
            Some(Rect {
                left: 0,
                top: 66,
                right: 1080,
                bottom: 2400
            })
        );
        assert_eq!(parse_bounds("[-5,0][10,10]").unwrap().left, -5);
        assert_eq!(parse_bounds("[0,0][10]"), None);
        assert_eq!(parse_bounds("[0,0][10,10][3,3]"), None);
        assert_eq!(parse_bounds(""), None);
    }
}
