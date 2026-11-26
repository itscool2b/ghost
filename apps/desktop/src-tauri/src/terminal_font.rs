//! The font the person's own terminal app draws with, so Ghost's terminals look like it (owner,
//! 17 November 2026: the terminal should be "the default terminal of the system itself").
//!
//! Read-only and best effort. The terminal is the one the person set as their default (`TERMINAL`,
//! then `xdg-terminals.list`), else the first of kitty, Ghostty, Alacritty, foot and Konsole whose
//! settings name a font, else the desktop's monospace font (GNOME's setting, fontconfig's
//! `monospace`, macOS's system monospace, Windows Terminal's default). Only formats that parse
//! safely are read: no Lua (WezTerm), no archived plists (Terminal, iTerm), and nothing is ever
//! run but `fc-match` and `gsettings` with fixed arguments. Sizes are points, as the terminals
//! store them, turned into CSS pixels for the platform. The family is checked before it reaches
//! the page.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::error::AppError;
use crate::state::AppState;

/// What the page shows and uses.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
pub struct SystemTerminalFont {
    /// The font family as CSS takes it, or `None` when only the size is known.
    pub family: Option<String>,
    /// The size in CSS pixels, or `None` when only the family is known.
    pub size_px: Option<f64>,
    /// The same size in points, as the terminal's own settings say it.
    pub size_points: Option<f64>,
    /// Where it came from, as Settings names it: "kitty", "GNOME" and the like.
    pub source: String,
}

/// How long `fc-match` or `gsettings` may take.
const HELPER_TIMEOUT: Duration = Duration::from_secs(2);
/// The most a settings file may hold, and how many files one terminal's settings may span.
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_FILES: usize = 24;

/// The person's terminal font, or `None` when nothing could be read.
#[tauri::command]
#[specta::specta]
pub async fn terminal_system_font(
    state: State<'_, AppState>,
) -> Result<Option<SystemTerminalFont>, AppError> {
    let vars = match state.sessions() {
        Ok(sessions) => env_vars(&sessions.resolved_env().await.vars),
        Err(_) => std::env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .collect(),
    };
    let found = tauri::async_runtime::spawn_blocking(move || {
        let reader = Fs;
        find(&vars, &reader, platform())
    })
    .await
    .ok()
    .flatten();
    Ok(match found {
        Some(found) => finish(found, platform()).await,
        None => fallback(platform()).await,
    })
}

fn env_vars(vars: &ghost_process::EnvMap) -> BTreeMap<String, String> {
    vars.iter()
        .filter_map(|(name, value)| {
            Some((
                name.as_os_str().to_str()?.to_owned(),
                value.to_str()?.to_owned(),
            ))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Linux,
    Macos,
    Windows,
}

fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Linux
    }
}

/// Points to CSS pixels: a point is a pixel on macOS, and 96/72 of one elsewhere.
fn points_to_px(points: f64, platform: Platform) -> f64 {
    match platform {
        Platform::Macos => points,
        Platform::Linux | Platform::Windows => points * 96.0 / 72.0,
    }
}

/// What a terminal's settings say, in points.
#[derive(Debug, Clone, Default, PartialEq)]
struct Found {
    family: Option<String>,
    points: Option<f64>,
    source: &'static str,
}

/// Reads files; tests pass their own.
trait Reader {
    fn read(&self, path: &Path) -> Option<String>;
}

struct Fs;

impl Reader for Fs {
    fn read(&self, path: &Path) -> Option<String> {
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            return None;
        }
        std::fs::read_to_string(path).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Terminal {
    Kitty,
    Ghostty,
    Alacritty,
    Foot,
    Konsole,
    Gnome,
    Windows,
}

impl Terminal {
    fn from_name(name: &str) -> Option<Terminal> {
        let name = name.trim().trim_start_matches(['+', '-']);
        let name = name.split(':').next().unwrap_or(name);
        let name = name.strip_suffix(".desktop").unwrap_or(name);
        let name = Path::new(name)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(name)
            .to_ascii_lowercase();
        Some(match name.as_str() {
            "kitty" => Terminal::Kitty,
            "ghostty" | "com.mitchellh.ghostty" => Terminal::Ghostty,
            "alacritty" | "org.alacritty.alacritty" => Terminal::Alacritty,
            "foot" | "footclient" | "foot-server" | "org.codeberg.dnkl.foot" => Terminal::Foot,
            "konsole" | "org.kde.konsole" => Terminal::Konsole,
            "gnome-terminal" | "org.gnome.terminal" | "kgx" | "org.gnome.console"
            | "gnome-console" => Terminal::Gnome,
            "wt" | "wt.exe" | "windowsterminal" | "windowsterminal.exe" => Terminal::Windows,
            _ => return None,
        })
    }
}

/// The person's default terminals, most preferred first: `TERMINAL`, then the desktop's and the
/// person's `xdg-terminals.list` (what `xdg-terminal-exec` reads).
fn preferred(vars: &BTreeMap<String, String>, reader: &dyn Reader) -> Vec<Terminal> {
    let mut found = Vec::new();
    if let Some(terminal) = vars
        .get("TERMINAL")
        .and_then(|name| Terminal::from_name(name))
    {
        found.push(terminal);
    }
    let home = vars.get("HOME").map(PathBuf::from);
    let config_home = vars
        .get("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    let mut dirs: Vec<PathBuf> = config_home.into_iter().collect();
    dirs.extend(
        vars.get("XDG_CONFIG_DIRS")
            .map_or("/etc/xdg", String::as_str)
            .split(':')
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute()),
    );
    let desktops: Vec<String> = vars
        .get("XDG_CURRENT_DESKTOP")
        .map(|value| {
            value
                .split(':')
                .filter(|name| !name.is_empty())
                .map(str::to_ascii_lowercase)
                .collect()
        })
        .unwrap_or_default();
    for dir in &dirs {
        let names = desktops
            .iter()
            .map(|desktop| format!("{desktop}-xdg-terminals.list"))
            .chain(std::iter::once("xdg-terminals.list".to_owned()));
        for name in names {
            let Some(text) = reader.read(&dir.join(name)) else {
                continue;
            };
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
                    continue;
                }
                if let Some(terminal) = Terminal::from_name(line)
                    && !found.contains(&terminal)
                {
                    found.push(terminal);
                }
            }
        }
    }
    found
}

fn find(vars: &BTreeMap<String, String>, reader: &dyn Reader, platform: Platform) -> Option<Found> {
    let home = vars.get("HOME").map(PathBuf::from);
    let config_home = vars
        .get("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    let read = |terminal: Terminal| -> Option<Found> {
        let config = config_home.as_deref();
        match terminal {
            Terminal::Kitty => kitty(config?, vars, reader),
            Terminal::Ghostty => ghostty(config?, home.as_deref(), platform, reader),
            Terminal::Alacritty => alacritty(config?, vars, reader),
            Terminal::Foot => foot(config?, reader),
            Terminal::Konsole => konsole(config?, home.as_deref(), vars, reader),
            Terminal::Gnome => Some(Found {
                source: "GNOME",
                ..Found::default()
            }),
            Terminal::Windows => windows_terminal(vars, reader),
        }
    };
    let preferred = preferred(vars, reader);
    if let Some(found) = preferred.iter().find_map(|terminal| read(*terminal)) {
        return Some(found);
    }
    // No default set, or its settings unreadable: the first terminal whose settings name a font,
    // else the first one with settings at all (kitty's often name only a size).
    let installed: Vec<Found> = [
        Terminal::Kitty,
        Terminal::Ghostty,
        Terminal::Alacritty,
        Terminal::Foot,
        Terminal::Konsole,
        Terminal::Windows,
    ]
    .into_iter()
    .filter(|terminal| !preferred.contains(terminal))
    .filter_map(read)
    .collect();
    let named = installed.iter().position(|found| found.family.is_some());
    let mut installed = installed.into_iter();
    match named {
        Some(at) => installed.nth(at),
        None => installed.next(),
    }
}

/// A family the page may use: trimmed, at most 80 characters, no quotes, brackets, separators or
/// control characters, and not a generic keyword that means "whatever the system picks".
fn clean_family(family: &str) -> Option<String> {
    let family = family.trim().trim_matches(['"', '\'']).trim();
    let generic = matches!(
        family.to_ascii_lowercase().as_str(),
        "" | "monospace" | "auto" | "default" | "mono" | "system" | "system-ui"
    );
    let safe = family.chars().count() <= 80
        && !family.chars().any(|c| {
            c.is_control() || matches!(c, '"' | '\'' | '\\' | ';' | '{' | '}' | '<' | '>' | ',')
        });
    (!generic && safe).then(|| family.to_owned())
}

/// A size in points the terminals allow (4 to 72); anything else is ignored.
fn clean_points(value: &str) -> Option<f64> {
    let points: f64 = value.trim().trim_end_matches("pt").trim().parse().ok()?;
    (points.is_finite() && (4.0..=72.0).contains(&points)).then_some(points)
}

/// `~`, `$HOME` and `${VAR}` in an include path, resolved against `dir` when relative.
fn expand(path: &str, dir: &Path, vars: &BTreeMap<String, String>) -> Option<PathBuf> {
    let mut text = path.trim().trim_matches(['"', '\'']).to_owned();
    if let Some(rest) = text.strip_prefix('~') {
        text = format!("{}{rest}", vars.get("HOME")?);
    }
    for (name, value) in vars {
        for form in [format!("${{{name}}}"), format!("${name}")] {
            if text.contains(&form) {
                text = text.replace(&form, value);
            }
        }
    }
    if text.contains('$') || text.contains('*') || text.contains('?') {
        return None;
    }
    let path = PathBuf::from(text);
    Some(if path.is_absolute() {
        path
    } else {
        dir.join(path)
    })
}

/// kitty: `font_family` and `font_size` in `kitty.conf` (or `$KITTY_CONFIG_DIRECTORY`), following
/// `include` in order, later settings winning. `geninclude` runs a program and is never followed;
/// `globinclude` and `envinclude` are skipped.
fn kitty(config: &Path, vars: &BTreeMap<String, String>, reader: &dyn Reader) -> Option<Found> {
    let dir = vars
        .get("KITTY_CONFIG_DIRECTORY")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| config.join("kitty"));
    let mut found = Found {
        source: "kitty",
        ..Found::default()
    };
    let mut files = 0;
    let read_any = kitty_file(
        &dir.join("kitty.conf"),
        vars,
        reader,
        &mut found,
        &mut files,
    );
    // kitty's own defaults: the system monospace font at 11 points.
    if read_any {
        found.points = found.points.or(Some(11.0));
        Some(found)
    } else {
        None
    }
}

fn kitty_file(
    path: &Path,
    vars: &BTreeMap<String, String>,
    reader: &dyn Reader,
    found: &mut Found,
    files: &mut usize,
) -> bool {
    if *files >= MAX_FILES {
        return false;
    }
    let Some(text) = reader.read(path) else {
        return false;
    };
    *files += 1;
    let dir = path.parent().unwrap_or(Path::new("/"));
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let value = value.trim();
        match key {
            "font_family" => {
                // `font_family family="JetBrains Mono" style=Regular` or a plain name.
                let family = match value.strip_prefix("family=") {
                    Some(rest) => quoted_or_word(rest),
                    None => value.to_owned(),
                };
                found.family = clean_family(&family);
            }
            "font_size" => {
                if let Some(points) = clean_points(value) {
                    found.points = Some(points);
                }
            }
            "include" => {
                if let Some(path) = expand(value, dir, vars) {
                    kitty_file(&path, vars, reader, found, files);
                }
            }
            _ => {}
        }
    }
    true
}

/// `"two words" rest` or `word rest`: the first value.
fn quoted_or_word(text: &str) -> String {
    let text = text.trim();
    for quote in ['"', '\''] {
        if let Some(rest) = text.strip_prefix(quote) {
            return rest.split(quote).next().unwrap_or("").to_owned();
        }
    }
    text.split_whitespace().next().unwrap_or("").to_owned()
}

/// Ghostty: the first `font-family` and the last `font-size` in its config, following
/// `config-file` (a leading `?` marks an optional one). An empty `font-family` starts the list
/// again, as Ghostty does.
fn ghostty(
    config: &Path,
    home: Option<&Path>,
    platform: Platform,
    reader: &dyn Reader,
) -> Option<Found> {
    let mut places = vec![
        config.join("ghostty/config"),
        config.join("ghostty/config.ghostty"),
    ];
    if platform == Platform::Macos
        && let Some(home) = home
    {
        let support = home.join("Library/Application Support/com.mitchellh.ghostty");
        places.push(support.join("config"));
        places.push(support.join("config.ghostty"));
    }
    let mut found = Found {
        source: "Ghostty",
        ..Found::default()
    };
    let mut files = 0;
    let mut first_family: Option<Option<String>> = None;
    let mut read_any = false;
    for place in places {
        read_any |= ghostty_file(&place, reader, &mut found, &mut first_family, &mut files);
    }
    found.family = first_family.flatten();
    read_any.then_some(found)
}

fn ghostty_file(
    path: &Path,
    reader: &dyn Reader,
    found: &mut Found,
    first_family: &mut Option<Option<String>>,
    files: &mut usize,
) -> bool {
    if *files >= MAX_FILES {
        return false;
    }
    let Some(text) = reader.read(path) else {
        return false;
    };
    *files += 1;
    let dir = path.parent().unwrap_or(Path::new("/"));
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "font-family" if value.is_empty() => *first_family = None,
            "font-family" if first_family.is_none() => *first_family = Some(clean_family(value)),
            "font-size" => {
                if let Some(points) = clean_points(value) {
                    found.points = Some(points);
                }
            }
            "config-file" => {
                let value = value.trim_start_matches('?');
                let path = Path::new(value);
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    dir.join(path)
                };
                ghostty_file(&path, reader, found, first_family, files);
            }
            _ => {}
        }
    }
    true
}

/// Alacritty: `[font] size` and `[font.normal] family` (or `normal = { family = "..." }`) in
/// `alacritty.toml`, following `import` lists. Only these keys are read, by a line reader: the
/// file is TOML, and nothing else in it matters here.
fn alacritty(config: &Path, vars: &BTreeMap<String, String>, reader: &dyn Reader) -> Option<Found> {
    let mut found = Found {
        source: "Alacritty",
        ..Found::default()
    };
    let mut files = 0;
    alacritty_file(
        &config.join("alacritty/alacritty.toml"),
        vars,
        reader,
        &mut found,
        &mut files,
    )
    .then_some(found)
}

fn alacritty_file(
    path: &Path,
    vars: &BTreeMap<String, String>,
    reader: &dyn Reader,
    found: &mut Found,
    files: &mut usize,
) -> bool {
    if *files >= MAX_FILES {
        return false;
    }
    let Some(text) = reader.read(path) else {
        return false;
    };
    *files += 1;
    let dir = path.parent().unwrap_or(Path::new("/"));
    // Imports come first: the file's own settings override them.
    let mut table = String::new();
    let mut imports = Vec::new();
    let mut in_import = false;
    for line in text.lines() {
        let line = line.split(" #").next().unwrap_or(line).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if in_import || ((table.is_empty() || table == "general") && line.starts_with("import")) {
            let list = line.split_once('=').map_or(line, |(_, rest)| rest);
            for part in list.split(',') {
                let part = part
                    .trim()
                    .trim_matches(['[', ']'])
                    .trim()
                    .trim_matches('"');
                if !part.is_empty() {
                    imports.push(part.to_owned());
                }
            }
            in_import = !line.contains(']');
            continue;
        }
        if line.starts_with('[') {
            table = line.trim_matches(['[', ']']).trim().to_owned();
        }
    }
    for import in imports {
        if let Some(path) = expand(&import, dir, vars) {
            alacritty_file(&path, vars, reader, found, files);
        }
    }
    table.clear();
    for line in text.lines() {
        let line = line.split(" #").next().unwrap_or(line).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            table = line.trim_matches(['[', ']']).trim().to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match (table.as_str(), key) {
            ("font", "size") => {
                if let Some(points) = clean_points(value) {
                    found.points = Some(points);
                }
            }
            ("font.normal", "family") => found.family = clean_family(value),
            ("font", "normal") => {
                if let Some(rest) = value.split("family").nth(1)
                    && let Some(rest) = rest.trim_start().strip_prefix('=')
                {
                    found.family = clean_family(&quoted_or_word(rest));
                }
            }
            _ => {}
        }
    }
    true
}

/// foot: the first pattern of `font=` in `foot.ini`'s main section (`Family:size=11` or
/// `:pixelsize=15`). foot's own default is the system monospace font.
fn foot(config: &Path, reader: &dyn Reader) -> Option<Found> {
    let text = reader.read(&config.join("foot/foot.ini"))?;
    let mut found = Found {
        source: "foot",
        ..Found::default()
    };
    let mut section = String::from("main");
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']']).trim().to_owned();
            continue;
        }
        if section != "main" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "font" {
            continue;
        }
        let first = value.split(',').next().unwrap_or("").trim();
        let mut parts = first.split(':');
        found.family = parts.next().and_then(clean_family);
        for part in parts {
            if let Some(size) = part.strip_prefix("size=") {
                found.points = clean_points(size);
            } else if let Some(pixels) = part.strip_prefix("pixelsize=") {
                // Pixels at 96 dots per inch: back to points so every source speaks one unit.
                found.points = clean_points(pixels).map(|pixels| pixels * 72.0 / 96.0);
            }
        }
    }
    Some(found)
}

/// Konsole: the default profile's `Font=Family,size,...` (`konsolerc`'s `DefaultProfile`, in
/// `~/.local/share/konsole`).
fn konsole(
    config: &Path,
    home: Option<&Path>,
    vars: &BTreeMap<String, String>,
    reader: &dyn Reader,
) -> Option<Found> {
    let rc = reader.read(&config.join("konsolerc"))?;
    let profile = rc
        .lines()
        .find_map(|line| line.trim().strip_prefix("DefaultProfile="))?
        .trim()
        .to_owned();
    if profile.is_empty() || profile.contains('/') || profile.contains("..") {
        return None;
    }
    let data_home = vars
        .get("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".local/share")))?;
    let text = reader.read(&data_home.join("konsole").join(profile))?;
    let font = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Font="))?;
    let mut parts = font.split(',');
    Some(Found {
        family: parts.next().and_then(clean_family),
        points: parts.next().and_then(clean_points),
        source: "Konsole",
    })
}

/// Windows Terminal: `profiles.defaults.font.face` and `.size` in its `settings.json` (or the
/// older `fontFace` and `fontSize`). Comments are allowed there and stripped first.
fn windows_terminal(vars: &BTreeMap<String, String>, reader: &dyn Reader) -> Option<Found> {
    let local = vars
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("LOCALAPPDATA"))
        .map(|(_, value)| PathBuf::from(value))?;
    let places = [
        local.join("Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json"),
        local.join(
            "Packages/Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe/LocalState/settings.json",
        ),
        local.join("Microsoft/Windows Terminal/settings.json"),
    ];
    let text = places.iter().find_map(|path| reader.read(path))?;
    let value: serde_json::Value = serde_json::from_str(&strip_json_comments(&text)).ok()?;
    let defaults = value.get("profiles")?.get("defaults")?;
    let font = defaults.get("font");
    let face = font
        .and_then(|font| font.get("face"))
        .or_else(|| defaults.get("fontFace"))
        .and_then(serde_json::Value::as_str);
    let size = font
        .and_then(|font| font.get("size"))
        .or_else(|| defaults.get("fontSize"))
        .and_then(serde_json::Value::as_f64);
    Some(Found {
        family: face.and_then(clean_family),
        points: size.and_then(|size| clean_points(&size.to_string())),
        source: "Windows Terminal",
    })
}

/// JSON with `//` and `/* */` comments and trailing commas, as Windows Terminal writes it.
fn strip_json_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for next in chars.by_ref() {
                    if last == '*' && next == '/' {
                        break;
                    }
                    last = next;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas before a closing bracket.
    let mut cleaned = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    for (index, c) in bytes.iter().enumerate() {
        if *c == ',' {
            let next = bytes[index + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}' | ']')) {
                continue;
            }
        }
        cleaned.push(*c);
    }
    cleaned
}

/// Fills what the terminal's settings left to the system: the system monospace family (kitty's
/// and foot's default), GNOME's setting for GNOME's terminals.
async fn finish(found: Found, platform: Platform) -> Option<SystemTerminalFont> {
    let mut family = found.family.clone();
    let mut points = found.points;
    if found.source == "GNOME" {
        let (gnome_family, gnome_points) = gnome_monospace().await.unwrap_or((None, None));
        family = family.or(gnome_family);
        points = points.or(gnome_points);
    }
    if family.is_none() {
        family = system_monospace(platform).await;
    }
    if family.is_none() && points.is_none() {
        return None;
    }
    Some(SystemTerminalFont {
        family,
        size_px: points.map(|points| round(points_to_px(points, platform))),
        size_points: points,
        source: found.source.to_owned(),
    })
}

/// No terminal settings at all: the desktop's own monospace font.
async fn fallback(platform: Platform) -> Option<SystemTerminalFont> {
    match platform {
        Platform::Linux => {
            if let Some((family, points)) = gnome_monospace().await
                && (family.is_some() || points.is_some())
            {
                return Some(SystemTerminalFont {
                    family,
                    size_px: points.map(|points| round(points_to_px(points, platform))),
                    size_points: points,
                    source: "GNOME".into(),
                });
            }
            let family = system_monospace(platform).await?;
            Some(SystemTerminalFont {
                family: Some(family),
                size_px: None,
                size_points: None,
                source: "your system".into(),
            })
        }
        // Terminal's default since macOS 10.15: SF Mono, which pages reach as `ui-monospace`.
        Platform::Macos => Some(SystemTerminalFont {
            family: Some("ui-monospace".into()),
            size_px: Some(points_to_px(11.0, platform)),
            size_points: Some(11.0),
            source: "macOS".into(),
        }),
        // Windows Terminal's default.
        Platform::Windows => Some(SystemTerminalFont {
            family: Some("Cascadia Mono".into()),
            size_px: Some(round(points_to_px(12.0, platform))),
            size_points: Some(12.0),
            source: "Windows Terminal".into(),
        }),
    }
}

fn round(px: f64) -> f64 {
    (px * 100.0).round() / 100.0
}

/// fontconfig's `monospace` on Linux (what kitty and foot draw with by default).
async fn system_monospace(platform: Platform) -> Option<String> {
    if platform != Platform::Linux {
        return None;
    }
    let output = helper("fc-match", &["-f", "%{family[0]}", "monospace"]).await?;
    clean_family(&output)
}

/// GNOME's monospace font setting, `'Source Code Pro 10'`.
async fn gnome_monospace() -> Option<(Option<String>, Option<f64>)> {
    let output = helper(
        "gsettings",
        &["get", "org.gnome.desktop.interface", "monospace-font-name"],
    )
    .await?;
    let value = output.trim().trim_matches('\'');
    let (family, size) = match value.rsplit_once(' ') {
        Some((family, size)) if clean_points(size).is_some() => (family, clean_points(size)),
        _ => (value, None),
    };
    Some((clean_family(family), size))
}

/// Runs a fixed system program with fixed arguments and the system's own library path (an
/// AppImage's launcher points `LD_LIBRARY_PATH` into the image), for at most two seconds.
async fn helper(program: &str, args: &[&str]) -> Option<String> {
    let path = ["/usr/bin", "/bin", "/usr/local/bin"]
        .iter()
        .map(|dir| Path::new(dir).join(program))
        .find(|path| path.is_file())?;
    let mut command = tokio::process::Command::new(path);
    command
        .args(args)
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("LD_PRELOAD")
        .env_remove("GSETTINGS_SCHEMA_DIR")
        .env_remove("GIO_MODULE_DIR")
        .env_remove("XDG_DATA_DIRS")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(HELPER_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty() && text.len() <= 200).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Files(BTreeMap<PathBuf, String>);

    impl Reader for Files {
        fn read(&self, path: &Path) -> Option<String> {
            self.0.get(path).cloned()
        }
    }

    fn files(entries: &[(&str, &str)]) -> Files {
        Files(
            entries
                .iter()
                .map(|(path, text)| (PathBuf::from(path), (*text).to_owned()))
                .collect(),
        )
    }

    fn vars(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// The owner's machine: `TERMINAL=xdg-terminal-exec`, kitty first in the list, a kitty.conf
    /// that includes its theme and sets only the size.
    #[test]
    fn the_default_terminal_comes_from_the_xdg_list_and_kitty_includes_are_followed() {
        let env = vars(&[
            ("HOME", "/home/me"),
            ("TERMINAL", "xdg-terminal-exec"),
            ("XDG_CURRENT_DESKTOP", "Hyprland"),
        ]);
        let fs = files(&[
            (
                "/home/me/.config/xdg-terminals.list",
                "# Terminal emulator preference order\nkitty.desktop\n",
            ),
            (
                "/home/me/.config/kitty/kitty.conf",
                "include ~/.local/state/omarchy/theme/kitty.conf\nfont_size 11.0\n",
            ),
            (
                "/home/me/.local/state/omarchy/theme/kitty.conf",
                "foreground #ffffff\nfont_family family=\"JetBrainsMono Nerd Font\" style=Regular\n",
            ),
        ]);
        let found = find(&env, &fs, Platform::Linux).expect("kitty");
        assert_eq!(found.source, "kitty");
        assert_eq!(found.family.as_deref(), Some("JetBrainsMono Nerd Font"));
        assert_eq!(found.points, Some(11.0));
    }

    #[test]
    fn kitty_without_a_family_leaves_it_to_the_system_at_eleven_points() {
        let env = vars(&[("HOME", "/home/me"), ("TERMINAL", "kitty")]);
        let fs = files(&[(
            "/home/me/.config/kitty/kitty.conf",
            "font_family monospace\n",
        )]);
        let found = find(&env, &fs, Platform::Linux).expect("kitty");
        assert_eq!(found.family, None);
        assert_eq!(found.points, Some(11.0));
    }

    #[test]
    fn a_desktop_list_comes_before_the_plain_one_and_exclusions_are_skipped() {
        let env = vars(&[("HOME", "/home/me"), ("XDG_CURRENT_DESKTOP", "GNOME")]);
        let fs = files(&[
            (
                "/home/me/.config/gnome-xdg-terminals.list",
                "-kitty.desktop\nfoot.desktop\n",
            ),
            ("/home/me/.config/xdg-terminals.list", "kitty.desktop\n"),
            (
                "/home/me/.config/foot/foot.ini",
                "[main]\nfont=Iosevka Term:size=12, Noto Color Emoji\n",
            ),
            (
                "/home/me/.config/kitty/kitty.conf",
                "font_family Fira Code\n",
            ),
        ]);
        let found = find(&env, &fs, Platform::Linux).expect("foot");
        assert_eq!(found.source, "foot");
        assert_eq!(found.family.as_deref(), Some("Iosevka Term"));
        assert_eq!(found.points, Some(12.0));
    }

    #[test]
    fn ghostty_takes_its_first_family_and_follows_config_files() {
        let env = vars(&[("HOME", "/home/me"), ("TERMINAL", "ghostty")]);
        let fs = files(&[
            (
                "/home/me/.config/ghostty/config",
                "font-family = \"Berkeley Mono\"\nfont-family = Symbols Nerd Font\nconfig-file = ?extra\n",
            ),
            ("/home/me/.config/ghostty/extra", "font-size = 14\n"),
        ]);
        let found = find(&env, &fs, Platform::Linux).expect("ghostty");
        assert_eq!(found.family.as_deref(), Some("Berkeley Mono"));
        assert_eq!(found.points, Some(14.0));
    }

    #[test]
    fn alacritty_reads_its_font_tables_and_imports() {
        let env = vars(&[("HOME", "/home/me"), ("TERMINAL", "alacritty")]);
        let fs = files(&[
            (
                "/home/me/.config/alacritty/alacritty.toml",
                "[general]\nimport = [\"~/.config/alacritty/theme.toml\"]\n\n[font]\nnormal = { family = \"CaskaydiaMono Nerd Font\", style = \"Regular\" }\n",
            ),
            (
                "/home/me/.config/alacritty/theme.toml",
                "[font]\nsize = 9.5\n",
            ),
        ]);
        let found = find(&env, &fs, Platform::Linux).expect("alacritty");
        assert_eq!(found.family.as_deref(), Some("CaskaydiaMono Nerd Font"));
        assert_eq!(found.points, Some(9.5));
        let fs = files(&[(
            "/home/me/.config/alacritty/alacritty.toml",
            "[font.normal]\nfamily = \"Hack\"\n[font]\nsize = 10\n",
        )]);
        let found = find(&env, &fs, Platform::Linux).expect("alacritty");
        assert_eq!(found.family.as_deref(), Some("Hack"));
        assert_eq!(found.points, Some(10.0));
    }

    #[test]
    fn konsole_reads_its_default_profile() {
        let env = vars(&[("HOME", "/home/me"), ("TERMINAL", "konsole")]);
        let fs = files(&[
            (
                "/home/me/.config/konsolerc",
                "[Desktop Entry]\nDefaultProfile=Mine.profile\n",
            ),
            (
                "/home/me/.local/share/konsole/Mine.profile",
                "[Appearance]\nFont=Hack,10,-1,7,400,0,0,0,0,0\n",
            ),
        ]);
        let found = find(&env, &fs, Platform::Linux).expect("konsole");
        assert_eq!(found.family.as_deref(), Some("Hack"));
        assert_eq!(found.points, Some(10.0));
    }

    #[test]
    fn windows_terminal_settings_with_comments_and_trailing_commas() {
        let env = vars(&[("LOCALAPPDATA", "C:/Users/me/AppData/Local")]);
        let fs = files(&[(
            "C:/Users/me/AppData/Local/Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json",
            "{\n  // defaults\n  \"profiles\": { \"defaults\": { \"font\": { \"face\": \"Cascadia Code\", \"size\": 11, }, }, /* list */ \"list\": [] },\n}\n",
        )]);
        let found = find(&env, &fs, Platform::Windows).expect("windows terminal");
        assert_eq!(found.family.as_deref(), Some("Cascadia Code"));
        assert_eq!(found.points, Some(11.0));
    }

    #[test]
    fn without_a_default_the_first_terminal_naming_a_font_wins() {
        let env = vars(&[("HOME", "/home/me")]);
        let fs = files(&[
            ("/home/me/.config/kitty/kitty.conf", "font_size 12\n"),
            (
                "/home/me/.config/ghostty/config",
                "font-family = Monaspace Neon\n",
            ),
        ]);
        let found = find(&env, &fs, Platform::Linux).expect("ghostty");
        assert_eq!(found.source, "Ghostty");
        // Only a size anywhere: that terminal's size, the family left to the system.
        let fs = files(&[("/home/me/.config/kitty/kitty.conf", "font_size 12\n")]);
        let found = find(&env, &fs, Platform::Linux).expect("kitty");
        assert_eq!(
            (found.source, found.family, found.points),
            ("kitty", None, Some(12.0))
        );
        assert!(find(&vars(&[("HOME", "/home/me")]), &files(&[]), Platform::Linux).is_none());
    }

    #[test]
    fn families_that_could_break_out_of_css_are_refused() {
        for bad in [
            "Mono\"; } body { color: red",
            "a<script>",
            "Font, monospace",
            "monospace",
            "",
            &"x".repeat(81),
        ] {
            assert_eq!(clean_family(bad), None, "{bad}");
        }
        assert_eq!(clean_family(" 'Fira Code' ").as_deref(), Some("Fira Code"));
        assert_eq!(clean_points("11.5"), Some(11.5));
        for bad in ["0", "200", "NaN", "eleven"] {
            assert_eq!(clean_points(bad), None, "{bad}");
        }
    }

    #[test]
    fn includes_are_bounded_and_never_run_or_glob() {
        let env = vars(&[("HOME", "/home/me"), ("TERMINAL", "kitty")]);
        let fs = files(&[
            (
                "/home/me/.config/kitty/kitty.conf",
                "include kitty.conf\ngeninclude gen.py\nglobinclude *.conf\nfont_size 13\n",
            ),
            ("/home/me/.config/kitty/a.conf", "font_size 20\n"),
        ]);
        let found = find(&env, &fs, Platform::Linux).expect("kitty");
        assert_eq!(found.points, Some(13.0));
    }

    #[test]
    fn points_become_css_pixels_for_the_platform() {
        assert_eq!(round(points_to_px(11.0, Platform::Linux)), 14.67);
        assert_eq!(points_to_px(11.0, Platform::Macos), 11.0);
        assert_eq!(points_to_px(12.0, Platform::Windows), 16.0);
    }
}
