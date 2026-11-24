//! iPhone simulators on a Mac (docs/mobile.md, "iPhone on a Mac").
//!
//! Xcode's `simctl` runs a simulator's life, its apps, links and settings; AXe ([`crate::axe`],
//! fetched at a pinned version) reads its screen as an outline, takes its pictures for the pane and
//! sends it touches and keys. The pane shows the screen, as it shows the emulator's. With Xcode
//! 26, AXe can read and touch a simulator only while Simulator.app shows it, so o7 opens
//! Simulator.app hidden and in the background ([`Xcode::show`]); Xcode 27 does without.
//!
//! o7 makes its own simulators ("o7 iPhone 1" and so on) in Xcode's usual device set, which is the
//! one AXe reads, and keeps their identities in its own folder ([`Sims`]). It never boots, changes
//! or deletes a simulator it did not make.
//!
//! Everything here compiles on every system, so the checks on Windows and Linux read it too; it
//! only finds Xcode on a Mac.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::Error;
use crate::outline::Outline;

/// The oldest Xcode o7 drives (the oldest AXe supports).
const OLDEST_XCODE: u32 = 26;
/// How long a quick `simctl` call may take.
const QUICK: Duration = Duration::from_secs(30);
/// How long one AXe call may take. Its first input after a simulator starts sets up its input
/// service, which took up to 20 seconds on GitHub's Macs.
const AXE: Duration = Duration::from_secs(60);
/// How long a simulator may take to boot (a first boot of a new simulator is the slowest).
pub const BOOT: Duration = Duration::from_secs(300);
/// The most a list or a screen's outline may print.
const LIST_LIMIT: usize = 16 * 1024 * 1024;
/// The most one picture of the screen may be.
const FRAME_LIMIT: usize = 16 * 1024 * 1024;
/// Apple's keyboard usage codes (USB HID, page 7) that AXe sends.
pub mod hid {
    pub const A: u8 = 4;
    pub const V: u8 = 25;
    pub const ENTER: u8 = 40;
    pub const ESCAPE: u8 = 41;
    pub const BACKSPACE: u8 = 42;
    pub const TAB: u8 = 43;
    pub const SPACE: u8 = 44;
    pub const PAGE_UP: u8 = 75;
    pub const DELETE_FORWARD: u8 = 76;
    pub const PAGE_DOWN: u8 = 78;
    pub const RIGHT: u8 = 79;
    pub const LEFT: u8 = 80;
    pub const DOWN: u8 = 81;
    pub const UP: u8 = 82;
    pub const SHIFT: u8 = 225;
    pub const COMMAND: u8 = 227;
}

/// Why this computer cannot start an iPhone simulator, in words a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    NotMac,
    NoXcode,
    CommandLineToolsOnly,
    OldXcode(String),
    License,
    NoRuntime,
    Broken(String),
}

impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Missing::NotMac => write!(f, "iPhone simulators run only on a Mac."),
            Missing::NoXcode => {
                write!(
                    f,
                    "Install Xcode from the App Store and open it once, then come back."
                )
            }
            Missing::CommandLineToolsOnly => write!(
                f,
                "Only Apple's command line tools are installed. Install Xcode from the App Store and open it once, then come back."
            ),
            Missing::OldXcode(version) => write!(
                f,
                "o7 drives the simulator with Xcode {OLDEST_XCODE} or later, and this Mac has Xcode {version}. Update Xcode, then come back."
            ),
            Missing::License => {
                write!(f, "Open Xcode once and accept its licence, then come back.")
            }
            Missing::NoRuntime => write!(
                f,
                "Xcode has no iPhone simulator yet. In Xcode, open Settings > Components and get iOS, then come back."
            ),
            Missing::Broken(why) => write!(f, "Xcode's simulator tools did not answer: {why}"),
        }
    }
}

/// Xcode, as found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xcode {
    /// Its developer folder (`/Applications/Xcode.app/Contents/Developer`), given to every tool
    /// o7 starts as `DEVELOPER_DIR`, so they all use the same Xcode.
    pub developer: PathBuf,
    /// "26.1".
    pub version: String,
}

/// A runtime the simulator can run: one iOS version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub identifier: String,
    /// "iOS 26.1".
    pub name: String,
    pub version: Vec<u32>,
    pub device_types: Vec<DeviceType>,
}

/// A kind of simulated device ("iPhone 17 Pro").
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceType {
    pub identifier: String,
    pub name: String,
    #[serde(rename = "productFamily", default)]
    pub family: Option<String>,
}

/// One simulator in Xcode's device set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimDevice {
    pub udid: String,
    pub name: String,
    pub booted: bool,
    pub available: bool,
    pub runtime: String,
}

/// A screen's size in points (AXe's units) and how many pixels a point is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

impl Display {
    /// Its size in pixels, the units of screenshots and of the outline o7 shows.
    pub fn pixels(&self) -> (u32, u32) {
        let side = |points: f64| (points * self.scale).round().clamp(1.0, 65_535.0) as u32;
        (side(self.width), side(self.height))
    }
}

/// A helper program's whole answer.
#[derive(Debug)]
pub(crate) struct Ran {
    pub(crate) ok: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: String,
}

impl Ran {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// What it said went wrong: the last line of its errors, without AXe's "Error: ".
    fn complaint(&self) -> String {
        let said = if self.stderr.trim().is_empty() {
            self.text()
        } else {
            self.stderr.clone()
        };
        let line = said
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .unwrap_or("it gave no reason")
            .trim_start_matches("Error:")
            .trim_start_matches("error:")
            .trim();
        crate::shell::shown(line)
    }
}

/// Reads `reader` to its end, keeping at most `limit` bytes. The rest is read and dropped, so the
/// program never blocks on a full pipe; when `strict`, a longer answer then fails with
/// [`Error::TooLarge`].
async fn read_all(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
    strict: bool,
) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut over = false;
    // On the heap: a future keeps its buffers, and one on the stack makes every future that
    // waits on this one that much larger.
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return if over && strict {
                Err(Error::TooLarge)
            } else {
                Ok(out)
            };
        }
        if over || out.len() + read > limit {
            over = true;
            continue;
        }
        out.extend_from_slice(&chunk[..read]);
    }
}

/// Runs a helper program to its end: `input` on its standard input, at most `limit` bytes of
/// answer, within `within`. A program still running then is killed.
pub(crate) async fn run(
    mut command: tokio::process::Command,
    input: Option<Vec<u8>>,
    limit: usize,
    within: Duration,
) -> Result<Ran, Error> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Protocol("No output.".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Protocol("No output.".into()))?;
    let work = async {
        let write = async {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                // A program that stops reading early has its reasons, which its errors say.
                let _ = stdin.write_all(&input).await;
                let _ = stdin.shutdown().await;
            }
        };
        let (_, out, err) = tokio::join!(
            write,
            read_all(stdout, limit, true),
            read_all(stderr, 64 * 1024, false)
        );
        let status = child.wait().await?;
        Ok::<_, Error>(Ran {
            ok: status.success(),
            stdout: out?,
            stderr: String::from_utf8_lossy(&err?).into_owned(),
        })
    };
    tokio::time::timeout(within, work)
        .await
        .map_err(|_| Error::Timeout)?
}

/// A tool of Xcode's, told which Xcode it belongs to.
fn tool(program: &Path, developer: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command.env("DEVELOPER_DIR", developer);
    command
}

/// Whether `text` is one of Xcode's refusals until its licence is accepted.
fn license_refusal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("license") && (lower.contains("agree") || lower.contains("accept"))
}

/// Finds Xcode and checks it can run a simulator: Xcode 26 or later, its licence accepted, and an
/// iOS runtime installed. Never asks macOS to install anything: a missing tool is only reported.
pub async fn find_xcode() -> Result<(Xcode, Vec<Runtime>), Missing> {
    if !cfg!(target_os = "macos") {
        return Err(Missing::NotMac);
    }
    let developer = developer_folder().await?;
    let version = xcode_version(&developer).await?;
    let major = version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or(0);
    if major < OLDEST_XCODE {
        return Err(Missing::OldXcode(version));
    }
    let xcode = Xcode { developer, version };
    let listed = run(
        {
            let mut command = xcode.simctl();
            command.args(["list", "-j", "runtimes"]);
            command
        },
        None,
        LIST_LIMIT,
        QUICK,
    )
    .await
    .map_err(|error| Missing::Broken(error.to_string()))?;
    if !listed.ok {
        if license_refusal(&listed.stderr) {
            return Err(Missing::License);
        }
        return Err(Missing::Broken(listed.complaint()));
    }
    let runtimes =
        parse_runtimes(&listed.stdout).map_err(|error| Missing::Broken(error.to_string()))?;
    if runtimes.is_empty() {
        return Err(Missing::NoRuntime);
    }
    Ok((xcode, runtimes))
}

/// The developer folder of the Xcode to use: `DEVELOPER_DIR`, else the one `xcode-select`
/// chose, else Xcode in Applications when only the command line tools are chosen.
async fn developer_folder() -> Result<PathBuf, Missing> {
    let has_simctl = |folder: &Path| folder.join("usr/bin/simctl").is_file();
    if let Some(folder) = std::env::var_os("DEVELOPER_DIR").map(PathBuf::from)
        && has_simctl(&folder)
    {
        return Ok(folder);
    }
    let mut command = tokio::process::Command::new("/usr/bin/xcode-select");
    command.arg("-p");
    let chosen = run(command, None, 4096, QUICK)
        .await
        .ok()
        .filter(|ran| ran.ok)
        .map(|ran| PathBuf::from(ran.text().trim()));
    if let Some(folder) = &chosen
        && has_simctl(folder)
    {
        return Ok(folder.clone());
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir("/Applications")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("Xcode") && name.ends_with(".app"))
        })
        .map(|app| app.join("Contents/Developer"))
        .filter(|folder| has_simctl(folder))
        .collect();
    // Xcode.app itself before betas and older copies beside it.
    candidates.sort_by_key(|folder| !folder.starts_with("/Applications/Xcode.app"));
    if let Some(folder) = candidates.into_iter().next() {
        return Ok(folder);
    }
    match chosen {
        Some(folder) if folder.to_string_lossy().contains("CommandLineTools") => {
            Err(Missing::CommandLineToolsOnly)
        }
        _ => Err(Missing::NoXcode),
    }
}

async fn xcode_version(developer: &Path) -> Result<String, Missing> {
    let mut command = tool(&developer.join("usr/bin/xcodebuild"), developer);
    command.arg("-version");
    let ran = run(command, None, 64 * 1024, Duration::from_secs(60))
        .await
        .map_err(|error| Missing::Broken(error.to_string()))?;
    if !ran.ok {
        if license_refusal(&ran.stderr) || license_refusal(&ran.text()) {
            return Err(Missing::License);
        }
        return Err(Missing::Broken(ran.complaint()));
    }
    parse_xcode_version(&ran.text()).ok_or_else(|| Missing::Broken("no version was printed".into()))
}

/// `Xcode 26.1\nBuild version 17B55` gives "26.1".
pub fn parse_xcode_version(said: &str) -> Option<String> {
    said.lines().find_map(|line| {
        let version = line.trim().strip_prefix("Xcode ")?.trim();
        version
            .chars()
            .next()?
            .is_ascii_digit()
            .then(|| version.to_owned())
    })
}

fn version_numbers(text: &str) -> Vec<u32> {
    text.split('.')
        .map_while(|part| part.trim().parse::<u32>().ok())
        .collect()
}

#[derive(Deserialize)]
struct RuntimeList {
    #[serde(default)]
    runtimes: Vec<RuntimeJson>,
}

#[derive(Deserialize)]
struct RuntimeJson {
    identifier: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    platform: Option<String>,
    #[serde(rename = "isAvailable", default)]
    available: bool,
    #[serde(rename = "supportedDeviceTypes", default)]
    device_types: Vec<DeviceType>,
}

/// `simctl list -j runtimes`: the iOS runtimes that can run, newest first.
pub fn parse_runtimes(json: &[u8]) -> Result<Vec<Runtime>, Error> {
    let list: RuntimeList = serde_json::from_slice(json)
        .map_err(|_| Error::Protocol("simctl listed its runtimes oddly.".into()))?;
    let mut runtimes: Vec<Runtime> = list
        .runtimes
        .into_iter()
        .filter(|runtime| {
            runtime.available
                && match &runtime.platform {
                    Some(platform) => platform == "iOS",
                    None => runtime.identifier.contains(".SimRuntime.iOS-"),
                }
        })
        .map(|runtime| Runtime {
            version: version_numbers(&runtime.version),
            name: if runtime.name.is_empty() {
                format!("iOS {}", runtime.version)
            } else {
                runtime.name
            },
            identifier: runtime.identifier,
            device_types: runtime
                .device_types
                .into_iter()
                .filter(|kind| {
                    kind.family
                        .as_deref()
                        .map_or(kind.name.starts_with("iPhone"), |family| family == "iPhone")
                })
                .collect(),
        })
        .filter(|runtime| !runtime.device_types.is_empty())
        .collect();
    // The general runtimes first, newest first: a runtime made for one new device only runs that
    // device, which may not be the iPhone o7 would choose.
    runtimes.sort_by(|a, b| {
        device_specific(&a.identifier)
            .cmp(&device_specific(&b.identifier))
            .then_with(|| b.version.cmp(&a.version))
    });
    Ok(runtimes)
}

/// A runtime made for one new device only, which Xcode 27 lists beside the general one of the
/// same version: `...SimRuntime.iOS-27-2-iPhone19-4` beside `...SimRuntime.iOS-27-2`.
fn device_specific(identifier: &str) -> bool {
    identifier
        .rsplit_once(".SimRuntime.")
        .is_some_and(|(_, name)| {
            name.split('-')
                .skip(1)
                .any(|part| part.parse::<u32>().is_err())
        })
}

/// The iPhone to simulate: the newest "Pro" (not a Max), else the newest of any kind.
pub fn choose_device_type(kinds: &[DeviceType]) -> Option<&DeviceType> {
    let generation = |name: &str| {
        name.split_whitespace()
            .find_map(|word| word.parse::<u32>().ok())
            .unwrap_or(0)
    };
    let plain_pro = |name: &str| name.ends_with(" Pro");
    kinds
        .iter()
        .filter(|kind| kind.name.starts_with("iPhone"))
        .max_by_key(|kind| {
            (
                generation(&kind.name),
                plain_pro(&kind.name),
                !kind.name.contains("Max"),
            )
        })
}

#[derive(Deserialize)]
struct DeviceList {
    #[serde(default)]
    devices: std::collections::BTreeMap<String, Vec<DeviceJson>>,
}

#[derive(Deserialize)]
struct DeviceJson {
    udid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    state: String,
    #[serde(rename = "isAvailable", default = "yes")]
    available: bool,
}

fn yes() -> bool {
    true
}

/// `simctl list -j devices`: every simulator, with its runtime.
pub fn parse_devices(json: &[u8]) -> Result<Vec<SimDevice>, Error> {
    let list: DeviceList = serde_json::from_slice(json)
        .map_err(|_| Error::Protocol("simctl listed its simulators oddly.".into()))?;
    Ok(list
        .devices
        .into_iter()
        .flat_map(|(runtime, devices)| {
            devices.into_iter().map(move |device| SimDevice {
                booted: device.state == "Booted",
                udid: device.udid,
                name: device.name,
                available: device.available,
                runtime: runtime.clone(),
            })
        })
        .collect())
}

/// A simulator's identity: a UUID, so it can never be read as an option or a path.
pub fn udid(text: &str) -> Result<&str, Error> {
    let text = text.trim();
    let shape = text.len() == 36
        && text.char_indices().all(|(at, ch)| {
            if matches!(at, 8 | 13 | 18 | 23) {
                ch == '-'
            } else {
                ch.is_ascii_hexdigit()
            }
        });
    if shape {
        Ok(text)
    } else {
        Err(Error::Protocol("simctl named a simulator oddly.".into()))
    }
}

/// An app's bundle id (`com.example.app`): letters, digits, hyphens and dots, as Apple allows.
pub fn bundle_id(name: &str) -> Result<&str, Error> {
    let name = name.trim();
    let shape = !name.is_empty()
        && name.len() <= 155
        && !name.starts_with(['.', '-'])
        && !name.ends_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'));
    if shape {
        Ok(name)
    } else {
        Err(Error::Refused(format!(
            "{} is not an app's bundle id. Give one like com.example.app.",
            crate::shell::shown(name)
        )))
    }
}

/// What iOS calls a text size, for a scale where 1 is the usual size.
pub fn content_size(scale: f32) -> Result<&'static str, Error> {
    if !(0.8..=3.2).contains(&scale) || !scale.is_finite() {
        return Err(Error::Refused(
            "Text size is a number from 0.8 to 3.".into(),
        ));
    }
    // Apple's steps, with the size each draws body text at against the usual 17 points.
    const STEPS: [(f32, &str); 12] = [
        (14.0 / 17.0, "extra-small"),
        (15.0 / 17.0, "small"),
        (16.0 / 17.0, "medium"),
        (1.0, "large"),
        (19.0 / 17.0, "extra-large"),
        (21.0 / 17.0, "extra-extra-large"),
        (23.0 / 17.0, "extra-extra-extra-large"),
        (28.0 / 17.0, "accessibility-medium"),
        (33.0 / 17.0, "accessibility-large"),
        (40.0 / 17.0, "accessibility-extra-large"),
        (47.0 / 17.0, "accessibility-extra-extra-large"),
        (53.0 / 17.0, "accessibility-extra-extra-extra-large"),
    ];
    Ok(STEPS
        .iter()
        .min_by(|a, b| (a.0 - scale).abs().total_cmp(&(b.0 - scale).abs()))
        .map_or("large", |step| step.1))
}

/// The privacy services `simctl privacy` grants and takes away.
pub const PRIVACY: [&str; 13] = [
    "all",
    "calendar",
    "contacts-limited",
    "contacts",
    "location",
    "location-always",
    "photos-add",
    "photos",
    "media-library",
    "microphone",
    "motion",
    "reminders",
    "siri",
];

/// `CFBundleExecutable = Hello;` from `simctl appinfo`.
pub fn parse_executable(said: &str) -> Option<String> {
    said.lines().find_map(|line| {
        let value = line
            .trim()
            .strip_prefix("CFBundleExecutable")?
            .trim_start()
            .strip_prefix('=')?;
        let value = value.trim().trim_end_matches(';').trim().trim_matches('"');
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// The app with `pid`, from `launchctl list` in the simulator:
/// `12345  0  UIKitApplication:com.apple.Preferences[1a2b][rb-legacy]`.
pub fn parse_launched(said: &str, pid: u32) -> Option<String> {
    said.lines().find_map(|line| {
        let mut columns = line.split_whitespace();
        let listed = columns.next()?.parse::<u32>().ok()?;
        let label = columns.nth(1)?;
        if listed != pid {
            return None;
        }
        let app = label.strip_prefix("UIKitApplication:")?;
        let app = app.split('[').next()?;
        bundle_id(app).ok().map(str::to_owned)
    })
}

/// AXe's answer with `--output json`: `{"didError": false, "data": {"result": ...}}`, or its error.
#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(rename = "didError", default)]
    did_error: bool,
    #[serde(default)]
    data: serde_json::Value,
    #[serde(default)]
    error: Option<EnvelopeError>,
}

#[derive(Debug, Deserialize)]
struct EnvelopeError {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

/// An AXe command's outcome: its result, or its error code and message.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Done(serde_json::Value),
    Failed { code: String, message: String },
}

/// Reads AXe's envelope from the last line of its output that is JSON.
pub fn parse_answer(said: &[u8]) -> Option<Answer> {
    let text = String::from_utf8_lossy(said);
    let envelope: Envelope = text
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .find_map(|line| serde_json::from_str(line).ok())?;
    Some(match (envelope.did_error, envelope.error) {
        (false, _) => Answer::Done(envelope.data.get("result").cloned().unwrap_or_default()),
        (true, error) => {
            let error = error.unwrap_or(EnvelopeError {
                code: String::new(),
                message: String::new(),
            });
            Answer::Failed {
                code: error.code,
                message: error.message,
            }
        }
    })
}

/// `displays`' result: the active screen's size in points and its scale.
pub fn parse_display(result: &serde_json::Value) -> Option<Display> {
    let screen = result.get("activeScreen")?;
    let number = |name: &str| screen.get(name).and_then(serde_json::Value::as_f64);
    let display = Display {
        width: number("width")?,
        height: number("height")?,
        scale: number("scale").unwrap_or(1.0),
    };
    let sane = |value: f64| value.is_finite() && value > 0.0 && value < 100_000.0;
    (sane(display.width) && sane(display.height) && (1.0..=4.0).contains(&display.scale))
        .then_some(display)
}

/// A JPEG's width and height, from its frame header.
pub fn jpeg_size(data: &[u8]) -> Option<(u16, u16)> {
    if data.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut at = 2;
    while at + 9 < data.len() {
        if data[at] != 0xff {
            return None;
        }
        let marker = data[at + 1];
        // Padding between segments.
        if marker == 0xff {
            at += 1;
            continue;
        }
        let length = usize::from(u16::from_be_bytes([data[at + 2], data[at + 3]]));
        let frame = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if frame {
            let height = u16::from_be_bytes([data[at + 5], data[at + 6]]);
            let width = u16::from_be_bytes([data[at + 7], data[at + 8]]);
            return (width > 0 && height > 0).then_some((width, height));
        }
        if length < 2 {
            return None;
        }
        at += 2 + length;
    }
    None
}

/// Reads one picture from AXe's `raw` stream: a big-endian length, then that many bytes of JPEG.
/// `None` once the stream has ended.
pub async fn next_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<Option<Vec<u8>>, Error> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > FRAME_LIMIT {
        return Err(Error::TooLarge);
    }
    let mut frame = vec![0u8; length];
    reader.read_exact(&mut frame).await?;
    Ok(Some(frame))
}

/// The scale for the pane's pictures: the screen's longest side drawn at about 1,400 pixels.
pub fn stream_scale(pixels: (u32, u32)) -> f64 {
    let longest = f64::from(pixels.0.max(pixels.1).max(1));
    ((1400.0 / longest).clamp(0.1, 1.0) * 100.0).round() / 100.0
}

impl Xcode {
    fn simctl(&self) -> tokio::process::Command {
        tool(&self.developer.join("usr/bin/simctl"), &self.developer)
    }

    /// Runs `simctl` with `args`; its standard output, or why it failed, as `what` failing.
    async fn simctl_run(
        &self,
        args: &[&str],
        what: &str,
        within: Duration,
    ) -> Result<String, Error> {
        let mut command = self.simctl();
        command.args(args);
        self.simctl_finish(command, what, within).await
    }

    /// Runs a `simctl` command already set up; its standard output, or why it failed.
    async fn simctl_finish(
        &self,
        command: tokio::process::Command,
        what: &str,
        within: Duration,
    ) -> Result<String, Error> {
        let ran = run(command, None, LIST_LIMIT, within).await?;
        if ran.ok {
            Ok(ran.text())
        } else if license_refusal(&ran.stderr) {
            Err(Error::Refused(Missing::License.to_string()))
        } else {
            Err(Error::Device(format!("{what} failed: {}", ran.complaint())))
        }
    }

    pub async fn devices(&self) -> Result<Vec<SimDevice>, Error> {
        let said = self
            .simctl_run(&["list", "-j", "devices"], "Listing the simulators", QUICK)
            .await?;
        parse_devices(said.as_bytes())
    }

    /// Makes a simulator; answers its UDID.
    pub async fn create(&self, name: &str, kind: &str, runtime: &str) -> Result<String, Error> {
        let said = self
            .simctl_run(
                &["create", name, kind, runtime],
                "Making the simulator",
                Duration::from_secs(120),
            )
            .await?;
        udid(said.lines().last().unwrap_or_default()).map(str::to_owned)
    }

    /// Boots a simulator and waits until it has finished booting.
    pub async fn boot(&self, sim: &str) -> Result<(), Error> {
        let sim = udid(sim)?;
        let mut command = self.simctl();
        command.args(["boot", sim]);
        let ran = run(command, None, 64 * 1024, Duration::from_secs(120)).await?;
        // Booted already (by o7 before a restart), or booting (Simulator.app was asked to show
        // it first): not a failure; bootstatus waits for it either way.
        if !ran.ok
            && !ran.stderr.contains("current state: Booted")
            && !ran.stderr.contains("current state: Booting")
        {
            if license_refusal(&ran.stderr) {
                return Err(Error::Refused(Missing::License.to_string()));
            }
            return Err(Error::Device(format!(
                "The simulator did not start: {}",
                ran.complaint()
            )));
        }
        self.simctl_run(&["bootstatus", sim, "-b"], "Starting the simulator", BOOT)
            .await
            .map(drop)
    }

    /// Shuts a simulator down. One that is already down is not a failure.
    pub async fn shutdown(&self, sim: &str) -> Result<(), Error> {
        let sim = udid(sim)?;
        let mut command = self.simctl();
        command.args(["shutdown", sim]);
        let ran = run(command, None, 64 * 1024, Duration::from_secs(60)).await?;
        if ran.ok || ran.stderr.contains("current state: Shutdown") {
            Ok(())
        } else {
            Err(Error::Device(format!(
                "The simulator did not stop: {}",
                ran.complaint()
            )))
        }
    }

    pub async fn delete(&self, sim: &str) -> Result<(), Error> {
        let sim = udid(sim)?;
        self.simctl_run(
            &["delete", sim],
            "Removing the simulator",
            Duration::from_secs(60),
        )
        .await
        .map(drop)
    }

    /// The major version, 26 for "26.6".
    pub fn major(&self) -> u32 {
        self.version
            .split('.')
            .next()
            .and_then(|major| major.trim().parse().ok())
            .unwrap_or(0)
    }

    /// With Xcode 26, a simulator's screen can be read and touched only while Simulator.app shows
    /// it (AXe says so; Xcode 27's Device Hub does without). Opens Simulator.app hidden and in the
    /// background, aimed at `sim`: it takes no focus and shows no window unless the person asks.
    pub async fn show(&self, sim: &str) -> Result<(), Error> {
        let sim = udid(sim)?;
        let app = self.developer.join("Applications/Simulator.app");
        if self.major() >= 27 || !app.is_dir() {
            return Ok(());
        }
        // A first launch can take most of a minute: `open` is not waited on past a moment, and
        // the screen is ready when AXe can read it ([`Simulator::settle`]).
        let mut command = tokio::process::Command::new("/usr/bin/open");
        command
            .args(["-g", "-j", "-a"])
            .arg(&app)
            .args(["--args", "-CurrentDeviceUDID", sim])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        if let Ok(Ok(status)) = tokio::time::timeout(Duration::from_secs(10), child.wait()).await
            && !status.success()
        {
            return Err(Error::Device("Simulator.app did not open.".into()));
        }
        Ok(())
    }
}

/// A booted simulator, for the pane and the agent: `simctl` and AXe aimed at it.
#[derive(Debug, Clone)]
pub struct Simulator {
    pub xcode: Xcode,
    pub axe: PathBuf,
    pub udid: String,
    /// A folder of o7's for screenshots on their way.
    pub scratch: PathBuf,
}

impl Simulator {
    fn simctl(&self) -> tokio::process::Command {
        self.xcode.simctl()
    }

    fn axe_command(&self) -> tokio::process::Command {
        tool(&self.axe, &self.xcode.developer)
    }

    /// Runs one AXe command, as `axe --output json <args> --udid <udid>`; its result, or its own
    /// words for the failure. `input` goes to its standard input (`type --stdin`).
    async fn axe(
        &self,
        args: &[&str],
        input: Option<Vec<u8>>,
        what: &str,
    ) -> Result<Answer, Error> {
        self.axe_within(args, input, what, AXE).await
    }

    async fn axe_within(
        &self,
        args: &[&str],
        input: Option<Vec<u8>>,
        what: &str,
        within: Duration,
    ) -> Result<Answer, Error> {
        let mut command = self.axe_command();
        command
            .args(["--output", "json"])
            .args(args)
            .args(["--udid", &self.udid]);
        let ran = run(command, input, LIST_LIMIT, within).await?;
        match parse_answer(&ran.stdout) {
            Some(answer) => Ok(answer),
            None if ran.ok => Ok(Answer::Done(serde_json::Value::Null)),
            None => Err(Error::Device(format!("{what} failed: {}", ran.complaint()))),
        }
    }

    /// An AXe command that must succeed.
    async fn axe_done(&self, args: &[&str], what: &str) -> Result<serde_json::Value, Error> {
        let mut tries = 0;
        loop {
            tries += 1;
            match self.axe(args, None, what).await? {
                Answer::Done(result) => return Ok(result),
                // Now and then AXe lacks the simulator's point translation for a moment (seen on
                // GitHub's Macs). It sends no input when it says so, so asking again cannot
                // repeat a touch.
                Answer::Failed { message, .. }
                    if tries < 4 && message.contains("No translation object") =>
                {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                }
                Answer::Failed { code, message } => return Err(axe_error(what, &code, &message)),
            }
        }
    }

    /// The screen's size in points and its scale. Where AXe cannot read the display catalogue
    /// (some Xcode 26 releases), the outline's own frame and a screenshot's size say the same.
    pub async fn display(&self) -> Result<Display, Error> {
        match self
            .axe(&["displays"], None, "Measuring the screen")
            .await?
        {
            Answer::Done(result) => parse_display(&result)
                .ok_or_else(|| Error::Protocol("AXe measured the screen oddly.".into())),
            Answer::Failed { code, message } => match self.measure_otherwise().await {
                Some(display) => Ok(display),
                None => Err(axe_error("Measuring the screen", &code, &message)),
            },
        }
    }

    async fn measure_otherwise(&self) -> Option<Display> {
        let mut command = self.axe_command();
        command.args(["describe-ui", "--udid", &self.udid]);
        let ran = run(command, None, LIST_LIMIT, AXE)
            .await
            .ok()
            .filter(|ran| ran.ok)?;
        let (width, height) = root_frame(&ran.stdout)?;
        let (pixels, _) = png_size(&self.screenshot().await.ok()?)?;
        let scale = (f64::from(pixels) / width).round().clamp(1.0, 4.0);
        Some(Display {
            width,
            height,
            scale,
        })
    }

    /// Waits until AXe can read the screen and measures it. A simulator that has just booted,
    /// or a Simulator.app still opening, can keep AXe waiting for a while: each try is short,
    /// and AXe's display catalogue is not asked again once it said it cannot read it.
    pub async fn settle(&self, within: Duration) -> Result<Display, Error> {
        let deadline = Instant::now() + within;
        let mut catalogue = true;
        loop {
            let measured = if catalogue {
                match self
                    .axe_within(
                        &["displays"],
                        None,
                        "Measuring the screen",
                        Duration::from_secs(20),
                    )
                    .await
                {
                    Ok(Answer::Done(result)) => parse_display(&result)
                        .ok_or_else(|| Error::Protocol("AXe measured the screen oddly.".into())),
                    Ok(Answer::Failed { code, message }) => {
                        catalogue = code != "readFailed";
                        match self.measure_otherwise().await {
                            Some(display) => Ok(display),
                            None => Err(axe_error("Measuring the screen", &code, &message)),
                        }
                    }
                    Err(error) => Err(error),
                }
            } else {
                self.measure_otherwise().await.ok_or_else(|| {
                    Error::Device("The simulator's screen cannot be read yet.".into())
                })
            };
            match measured {
                Ok(display) => return Ok(display),
                Err(error) if Instant::now() >= deadline => return Err(error),
                Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        }
    }

    /// Presses Shift once, which does nothing on its own, so AXe's input service is up before the
    /// first real touch.
    pub async fn warm(&self) {
        let _ = self.key(hid::SHIFT).await;
    }

    /// The screen as an outline, in pixels; with the app in front when `app` is asked for.
    pub async fn outline(&self, scale: f64, app: bool) -> Result<Outline, Error> {
        let mut command = self.axe_command();
        command.args(["describe-ui", "--udid", &self.udid]);
        let ran = run(command, None, LIST_LIMIT, AXE).await?;
        if !ran.ok {
            return Err(Error::Device(format!(
                "Reading the screen failed: {}",
                ran.complaint()
            )));
        }
        let mut outline = Outline::parse_ios(&ran.stdout, scale)?;
        if app && let Some(pid) = outline_pid(&ran.stdout) {
            outline.package = self.app_with_pid(pid).await;
        }
        Ok(outline)
    }

    async fn app_with_pid(&self, pid: u32) -> Option<String> {
        let mut command = self.simctl();
        command.args(["spawn", &self.udid, "launchctl", "list"]);
        let ran = run(command, None, 4 * 1024 * 1024, QUICK).await.ok()?;
        parse_launched(&ran.text(), pid)
    }

    /// The app in front, by its bundle id.
    pub async fn foreground(&self) -> Option<String> {
        let mut command = self.axe_command();
        command.args(["describe-ui", "--udid", &self.udid]);
        let ran = run(command, None, LIST_LIMIT, AXE).await.ok()?;
        let pid = outline_pid(&ran.stdout)?;
        self.app_with_pid(pid).await
    }

    /// A picture of the screen, as a PNG at its own size.
    pub async fn screenshot(&self) -> Result<Vec<u8>, Error> {
        tokio::fs::create_dir_all(&self.scratch).await?;
        let file = self.scratch.join(format!(
            "{}-{}.png",
            self.udid,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos())
        ));
        let path = file.to_string_lossy().into_owned();
        let taken = self
            .xcode
            .simctl_run(
                &["io", &self.udid, "screenshot", "--type=png", &path],
                "Taking a screenshot",
                AXE,
            )
            .await;
        let png = match taken {
            Ok(_) => tokio::fs::read(&file).await.map_err(Error::from),
            Err(error) => Err(error),
        };
        let _ = tokio::fs::remove_file(&file).await;
        png
    }

    /// Installs a `.app` built for the simulator.
    pub async fn install(&self, app: &Path) -> Result<(), Error> {
        let is_app = app
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"));
        if app
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ipa"))
        {
            return Err(Error::Refused(
                "An .ipa is built for a real iPhone. Build the app for the simulator (xcodebuild with -destination 'platform=iOS Simulator,id=...') and give the .app it makes.".into(),
            ));
        }
        if !is_app || !app.is_absolute() || !app.join("Info.plist").is_file() {
            return Err(Error::Refused(
                "Give the full path of the .app folder the build made for the simulator.".into(),
            ));
        }
        let path = app.to_string_lossy().into_owned();
        self.xcode
            .simctl_run(
                &["install", &self.udid, &path],
                "Installing the app",
                Duration::from_secs(180),
            )
            .await
            .map(drop)
    }

    /// Starts an installed app; answers its process id.
    pub async fn launch(&self, bundle: &str) -> Result<Option<u32>, Error> {
        let bundle = bundle_id(bundle)?;
        // The app's own output goes to files o7 reads for its log, as Xcode's console shows it:
        // what it prints, and its NSLog and os_log lines, which OS_ACTIVITY_DT_MODE also sends to
        // standard error. The simulator's own log cannot always be read as text (on iOS 27 none
        // of an app's own lines are).
        let (out, err) = self.output_files(bundle);
        if let Some(folder) = out.parent() {
            tokio::fs::create_dir_all(folder).await?;
        }
        let _ = tokio::fs::remove_file(&out).await;
        let _ = tokio::fs::remove_file(&err).await;
        let mut command = self.xcode.simctl();
        command
            .env("SIMCTL_CHILD_OS_ACTIVITY_DT_MODE", "YES")
            .arg("launch")
            .arg(format!("--stdout={}", out.display()))
            .arg(format!("--stderr={}", err.display()))
            .args([self.udid.as_str(), bundle]);
        // A simulator that has just booted may still be setting up its home screen; the first
        // launch waits for that.
        let said = self
            .xcode
            .simctl_finish(command, "Starting the app", Duration::from_secs(120))
            .await?;
        Ok(said
            .rsplit(':')
            .next()
            .and_then(|pid| pid.trim().parse::<u32>().ok()))
    }

    /// Where an app o7 started writes its standard output and its standard error.
    fn output_files(&self, bundle: &str) -> (PathBuf, PathBuf) {
        let folder = self.scratch.join("output");
        (
            folder.join(format!("{bundle}.out.txt")),
            folder.join(format!("{bundle}.err.txt")),
        )
    }

    pub async fn terminate(&self, bundle: &str) -> Result<(), Error> {
        let bundle = bundle_id(bundle)?;
        let mut command = self.simctl();
        command.args(["terminate", &self.udid, bundle]);
        let ran = run(command, None, 64 * 1024, QUICK).await?;
        // Not running is what was wanted.
        if ran.ok || ran.stderr.contains("found nothing to terminate") {
            Ok(())
        } else {
            Err(Error::Device(format!(
                "Stopping the app failed: {}",
                ran.complaint()
            )))
        }
    }

    pub async fn open_link(&self, link: &str) -> Result<(), Error> {
        self.xcode
            .simctl_run(&["openurl", &self.udid, link], "Opening the link", QUICK)
            .await
            .map(drop)
    }

    pub async fn set_appearance(&self, dark: bool) -> Result<(), Error> {
        let mode = if dark { "dark" } else { "light" };
        self.xcode
            .simctl_run(
                &["ui", &self.udid, "appearance", mode],
                "Changing the appearance",
                QUICK,
            )
            .await
            .map(drop)
    }

    pub async fn set_text_scale(&self, scale: f32) -> Result<(), Error> {
        let size = content_size(scale)?;
        self.xcode
            .simctl_run(
                &["ui", &self.udid, "content_size", size],
                "Changing the text size",
                QUICK,
            )
            .await
            .map(drop)
    }

    /// Grants or takes away one of `simctl privacy`'s services for an app.
    pub async fn set_permission(
        &self,
        bundle: &str,
        service: &str,
        granted: bool,
    ) -> Result<(), Error> {
        let bundle = bundle_id(bundle)?;
        let service = PRIVACY
            .iter()
            .find(|known| known.eq_ignore_ascii_case(service.trim()))
            .ok_or_else(|| {
                Error::Refused(format!(
                    "On an iPhone a permission is one of {}.",
                    PRIVACY.join(", ")
                ))
            })?;
        let verb = if granted { "grant" } else { "revoke" };
        self.xcode
            .simctl_run(
                &["privacy", &self.udid, verb, service, bundle],
                "Changing the permission",
                QUICK,
            )
            .await
            .map(drop)
    }

    /// The name of an installed app's program, which its log lines and crash reports carry.
    async fn executable(&self, bundle: &str) -> Option<String> {
        let mut command = self.simctl();
        command.args(["appinfo", &self.udid, bundle]);
        let ran = run(command, None, 256 * 1024, QUICK).await.ok()?;
        parse_executable(&ran.text())
    }

    /// The app's log lines from the last ten minutes, and its last crash report.
    pub async fn logs(&self, bundle: Option<&str>, lines: u32) -> Result<String, Error> {
        let lines = lines.clamp(1, 500) as usize;
        let mut predicate = None;
        let mut program = None;
        if let Some(bundle) = bundle {
            let bundle = bundle_id(bundle)?;
            let name = self.executable(bundle).await.ok_or_else(|| {
                Error::Refused(format!("{bundle} is not installed on this simulator."))
            })?;
            // The name goes into a predicate between quotes: only plain names do.
            if !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '_' | '-' | '.'))
            {
                return Err(Error::Refused(
                    "That app's program name cannot be searched for.".into(),
                ));
            }
            predicate = Some(app_log_predicate(&name));
            program = Some(name);
        }
        let mut command = self.simctl();
        command.args([
            "spawn", &self.udid, "log", "show", "--last", "10m", "--style", "compact",
        ]);
        // An app's own lines, info included; the whole simulator's would drown in it.
        if predicate.is_some() {
            command.arg("--info");
        }
        if let Some(predicate) = &predicate {
            command.args(["--predicate", predicate]);
        }
        let ran = run(command, None, 64 * 1024 * 1024, Duration::from_secs(90)).await?;
        if !ran.ok {
            return Err(Error::Device(format!(
                "Reading the log failed: {}",
                ran.complaint()
            )));
        }
        let text = ran.text();
        let kept: Vec<&str> = text
            .lines()
            .filter(|line| !line.starts_with("Timestamp") && !line.starts_with("Filtering the log"))
            .collect();
        // Lines the log could not compose as text say nothing; a framework repeating itself would
        // fill the window with one line and push the app's own lines out of it.
        let readable: Vec<&str> = kept.into_iter().filter(|line| composed(line)).collect();
        let kept = collapse_repeats(&readable);
        let tail = &kept[kept.len().saturating_sub(lines)..];
        let mut out = match bundle {
            Some(bundle) if tail.is_empty() => {
                format!(
                    "The simulator's log holds no line from {bundle} in the last ten minutes that can be read.\n"
                )
            }
            Some(bundle) => format!(
                "Log of {bundle} (last {} lines):\n{}\n",
                tail.len(),
                tail.join("\n")
            ),
            None => format!(
                "The simulator's log (last {} lines):\n{}\n",
                tail.len(),
                tail.join("\n")
            ),
        };
        // What the app itself wrote, since o7 last started it: often all there is to read.
        if let Some(bundle) = bundle {
            let (out_file, err_file) = self.output_files(bundle);
            for (label, path) in [("standard output", out_file), ("standard error", err_file)] {
                let Ok(written) = tokio::fs::read(&path).await else {
                    continue;
                };
                let written = String::from_utf8_lossy(&written);
                let written: Vec<&str> = written.lines().collect();
                if written.is_empty() {
                    continue;
                }
                let tail = &written[written.len().saturating_sub(lines)..];
                out.push_str(&format!(
                    "\nWhat {bundle} wrote to its {label} since o7 started it (last {} lines):\n{}\n",
                    tail.len(),
                    tail.join("\n")
                ));
            }
        }
        if let Some(program) = program
            && let Some(crash) = last_crash(&program, Duration::from_secs(30 * 60)).await
        {
            out.push_str(&format!("\nIts last crash:\n{crash}\n"));
        }
        Ok(tail_of(&out, 60_000))
    }

    /// A tap at a point, in points; held for a long press.
    pub async fn tap(&self, x: f64, y: f64, hold: Option<Duration>) -> Result<(), Error> {
        let (x, y) = (format!("{x:.1}"), format!("{y:.1}"));
        let hold = hold.map(|hold| format!("{:.2}", hold.as_secs_f64().clamp(0.05, 10.0)));
        // A real touch, as a finger's: AXe's default for a plain tap goes through the accessibility
        // tree at the point, which a simulator that has just started may not have ready.
        let mut args = vec!["tap", "-x", &x, "-y", &y, "--tap-style", "physical"];
        if let Some(hold) = &hold {
            args.extend(["--hold", hold]);
        }
        self.axe_done(&args, "Tapping").await.map(drop)
    }

    /// A finger drawn from one point to another, in points, over `duration`.
    pub async fn swipe(
        &self,
        from: (f64, f64),
        to: (f64, f64),
        duration: Duration,
    ) -> Result<(), Error> {
        let values = [from.0, from.1, to.0, to.1].map(|value| format!("{value:.1}"));
        let duration = format!("{:.2}", duration.as_secs_f64().clamp(0.05, 5.0));
        self.axe_done(
            &[
                "swipe",
                "--start-x",
                &values[0],
                "--start-y",
                &values[1],
                "--end-x",
                &values[2],
                "--end-y",
                &values[3],
                "--duration",
                &duration,
            ],
            "Swiping",
        )
        .await
        .map(drop)
    }

    /// Types text on the simulator's keyboard. Text beyond plain ASCII (other scripts, accents,
    /// emoji) is pasted instead: AXe 2.0.4 does not always refuse what the keyboard has no keys
    /// for, and can time out on it. The text goes to AXe on its standard input, so it is never
    /// in a command line.
    pub async fn type_text(&self, text: &str) -> Result<(), Error> {
        if text.is_empty() {
            return Ok(());
        }
        if !keyboard_text(text) {
            return self.paste(text).await;
        }
        match self
            .axe(
                &["type", "--stdin"],
                Some(text.as_bytes().to_vec()),
                "Typing",
            )
            .await?
        {
            Answer::Done(_) => Ok(()),
            Answer::Failed { code, .. } if code == "invalidArguments" => self.paste(text).await,
            Answer::Failed { code, message } => Err(axe_error("Typing", &code, &message)),
        }
    }

    /// Puts text on the simulator's clipboard and pastes it where the focus is. The app takes a
    /// paste from the clipboard in its own time, so this waits (a few seconds at most) until the
    /// screen shows the text: an Enter pressed next would otherwise land before it.
    async fn paste(&self, text: &str) -> Result<(), Error> {
        let mut command = self.simctl();
        command.args(["pbcopy", &self.udid]);
        let ran = run(command, Some(text.as_bytes().to_vec()), 64 * 1024, QUICK).await?;
        if !ran.ok {
            return Err(Error::Device(format!("Typing failed: {}", ran.complaint())));
        }
        self.combo(&[hid::COMMAND], hid::V).await?;
        let shown = async {
            loop {
                if self.screen_shows(text).await {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        };
        // Not seen in time (a field that hides what it holds, a slow Mac): typing goes on.
        let _ = tokio::time::timeout(Duration::from_secs(8), shown).await;
        Ok(())
    }

    /// Whether any element on the screen holds `text`, in its value or its name.
    async fn screen_shows(&self, text: &str) -> bool {
        let mut command = self.axe_command();
        command.args(["describe-ui", "--udid", &self.udid]);
        let Ok(ran) = run(command, None, LIST_LIMIT, QUICK).await else {
            return false;
        };
        serde_json::from_slice::<serde_json::Value>(&ran.stdout)
            .is_ok_and(|tree| holds_text(&tree, text))
    }

    /// Presses one key by its keyboard usage code ([`hid`]).
    pub async fn key(&self, code: u8) -> Result<(), Error> {
        self.axe_done(&["key", &code.to_string()], "Pressing a key")
            .await
            .map(drop)
    }

    /// Holds `modifiers` and presses `code`: Command-A selects everything in a field.
    pub async fn combo(&self, modifiers: &[u8], code: u8) -> Result<(), Error> {
        let modifiers = modifiers
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        self.axe_done(
            &[
                "key-combo",
                "--modifiers",
                &modifiers,
                "--key",
                &code.to_string(),
            ],
            "Pressing keys",
        )
        .await
        .map(drop)
    }

    /// Presses a hardware button: `home`, `lock`, `side-button` or `siri`.
    pub async fn button(&self, name: &str) -> Result<(), Error> {
        self.axe_done(&["button", name], "Pressing a button")
            .await
            .map(drop)
    }

    /// Opens the app switcher: Home pressed twice, quickly, in one session.
    pub async fn app_switcher(&self) -> Result<(), Error> {
        self.axe_done(
            &["batch", "--step", "button home", "--step", "button home"],
            "Opening the app switcher",
        )
        .await
        .map(drop)
    }

    /// iOS has no Back button: a swipe in from the left edge goes back, as a thumb does.
    pub async fn back(&self) -> Result<(), Error> {
        self.axe_done(&["gesture", "swipe-from-left-edge"], "Going back")
            .await
            .map(drop)
    }

    /// The screen as pictures for the pane, `fps` a second at most, at `scale` of its size.
    pub fn stream(&self, fps: u32, scale: f64) -> Result<tokio::process::Child, Error> {
        let mut command = self.axe_command();
        command
            .args(["stream-video", "--udid", &self.udid, "--format", "raw"])
            .args(["--fps", &fps.clamp(1, 30).to_string()])
            .args(["--quality", "70"])
            .args(["--scale", &format!("{:.2}", scale.clamp(0.1, 1.0))])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        Ok(command.spawn()?)
    }
}

/// The `log show` predicate for an app's log: its own lines (NSLog, print, its own subsystems),
/// and errors and faults from Apple's frameworks in it, without their everyday chatter.
/// `program` must already be a plain name.
pub fn app_log_predicate(program: &str) -> String {
    format!(
        "process == \"{program}\" AND (NOT (subsystem BEGINSWITH \"com.apple.\") OR messageType == error OR messageType == fault)"
    )
}

/// Whether text is plain enough for any keyboard layout: printable ASCII, spaces, tabs and new
/// lines.
pub fn keyboard_text(text: &str) -> bool {
    text.chars()
        .all(|ch| ch.is_ascii_graphic() || matches!(ch, ' ' | '\n' | '\t'))
}

/// AXe's error, in words for a person or an agent.
fn axe_error(what: &str, code: &str, message: &str) -> Error {
    let message = crate::shell::shown(message.trim());
    match code {
        "invalidArguments" => Error::Refused(format!("{what} was refused: {message}")),
        "targetNotFound" | "notBooted" => Error::DeviceGone,
        "busy" => Error::Device(format!(
            "{what} could not start: the simulator is busy. Try again in a moment."
        )),
        "timeout" => Error::Timeout,
        "xcodeUnavailable" => {
            Error::Device(format!("{what} failed: Xcode could not be used. {message}"))
        }
        _ if message.is_empty() => Error::Device(format!("{what} failed ({code}).")),
        _ => Error::Device(format!("{what} failed: {message}")),
    }
}

/// The size in points of the outline's root (the app in front, which fills the screen).
pub fn root_frame(json: &[u8]) -> Option<(f64, f64)> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    let root = match &value {
        serde_json::Value::Array(roots) => roots.first()?,
        other => other,
    };
    let frame = root.get("frame")?;
    let width = frame.get("width")?.as_f64()?;
    let height = frame.get("height")?.as_f64()?;
    (width > 0.0 && height > 0.0).then_some((width, height))
}

/// A PNG's width and height, from its header.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.get(..8)? != b"\x89PNG\r\n\x1a\n" || png.get(12..16)? != b"IHDR" {
        return None;
    }
    let number = |at: usize| Some(u32::from_be_bytes(png.get(at..at + 4)?.try_into().ok()?));
    Some((number(16)?, number(20)?))
}

/// The process id of the app the outline was read from (its root element's `pid`).
pub fn outline_pid(json: &[u8]) -> Option<u32> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    let root = match &value {
        serde_json::Value::Array(roots) => roots.first()?,
        other => other,
    };
    root.get("pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
}

/// The last `limit` bytes of `text`, cut at a line.
/// The length of the time a compact `log show` line starts with: "2026-11-24 03:44:06.865 ".
const LOG_TIME: usize = 24;

/// Runs of the same log line, but for its time, as one line that says how often it came: the
/// first of the run, then "(the same line N times)".
fn collapse_repeats(lines: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current: Option<(&str, usize)> = None;
    let flush = |out: &mut Vec<String>, current: Option<(&str, usize)>| {
        if let Some((line, count)) = current {
            out.push(if count > 1 {
                format!("{line} (the same line {count} times)")
            } else {
                line.to_owned()
            });
        }
    };
    for &line in lines {
        let message = line.get(LOG_TIME..).unwrap_or(line);
        match current {
            Some((first, count)) if first.get(LOG_TIME..).unwrap_or(first) == message => {
                current = Some((first, count + 1));
            }
            _ => {
                flush(&mut out, current);
                current = Some((line, 1));
            }
        }
    }
    flush(&mut out, current);
    out
}

/// Whether a compact `log show` line holds text: iOS 27's log leaves many as
/// "<compose failure [shared UUID]>", or as nothing after their sender's image UUID.
fn composed(line: &str) -> bool {
    if line.contains("<compose failure") {
        return false;
    }
    // "2026-11-24 04:11:43.684 A  HelloO7[14946:ce92] (C54AD6FA-9FF2-3EDB-9C8A-53A82682D342) "
    let rest = line.get(LOG_TIME..).unwrap_or(line).trim_end();
    !(rest.ends_with(')')
        && rest
            .rsplit_once(" (")
            .is_some_and(|(_, uuid)| is_uuid(uuid.trim_end_matches(')'))))
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.chars().enumerate().all(|(at, ch)| {
            if matches!(at, 8 | 13 | 18 | 23) {
                ch == '-'
            } else {
                ch.is_ascii_hexdigit()
            }
        })
}

/// Whether any string in AXe's tree holds `text`.
fn holds_text(tree: &serde_json::Value, text: &str) -> bool {
    match tree {
        serde_json::Value::String(value) => value.contains(text),
        serde_json::Value::Array(items) => items.iter().any(|item| holds_text(item, text)),
        serde_json::Value::Object(fields) => fields.values().any(|value| holds_text(value, text)),
        _ => false,
    }
}

fn tail_of(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let cut = text[start..]
        .find('\n')
        .map_or(start, |line| start + line + 1);
    format!("(earlier lines left out)\n{}", &text[cut..])
}

/// The newest crash report macOS wrote for `program` within `within`, summarised: what killed it
/// and the frames of the thread that crashed.
async fn last_crash(program: &str, within: Duration) -> Option<String> {
    let folder = PathBuf::from(std::env::var_os("HOME")?).join("Library/Logs/DiagnosticReports");
    let program = program.to_owned();
    tokio::task::spawn_blocking(move || {
        let newest = std::fs::read_dir(&folder)
            .ok()?
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".ips")
                    && (name.starts_with(&format!("{program}-"))
                        || name.starts_with(&format!("{program}_")))
            })
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .filter(|(modified, _)| modified.elapsed().is_ok_and(|age| age <= within))
            .max_by_key(|(modified, _)| *modified)?
            .1;
        let report = std::fs::read(&newest).ok()?;
        Some(summarise_crash(&String::from_utf8_lossy(
            &report[..report.len().min(4 * 1024 * 1024)],
        )))
    })
    .await
    .ok()
    .flatten()
}

/// A crash report (`.ips`: one line of header, then the report as JSON) as a few lines.
pub fn summarise_crash(report: &str) -> String {
    let body = report.split_once('\n').map_or(report, |(_, body)| body);
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return tail_of(report, 4000);
    };
    let mut out = String::new();
    let exception = &json["exception"];
    if let Some(kind) = exception["type"].as_str() {
        out.push_str(&format!("Exception: {kind}"));
        if let Some(signal) = exception["signal"].as_str() {
            out.push_str(&format!(" ({signal})"));
        }
        out.push('\n');
    }
    if let Some(reason) = json["termination"]["indicator"].as_str() {
        out.push_str(&format!("Termination: {reason}\n"));
    }
    if let Some(reason) = json["asi"].as_object().and_then(|asi| asi.values().next()) {
        out.push_str(&format!(
            "Reason: {}\n",
            crate::shell::shown(&reason.to_string())
        ));
    }
    let images = json["usedImages"].as_array();
    let thread = json["faultingThread"].as_u64().unwrap_or(0) as usize;
    if let Some(frames) = json["threads"][thread]["frames"].as_array() {
        out.push_str(&format!("Thread {thread} crashed:\n"));
        for (number, frame) in frames.iter().take(25).enumerate() {
            let image = frame["imageIndex"]
                .as_u64()
                .and_then(|index| images?.get(index as usize)?["name"].as_str())
                .unwrap_or("?");
            let symbol = frame["symbol"].as_str().unwrap_or("???");
            out.push_str(&format!("{number:>3}  {image}  {symbol}\n"));
        }
    }
    if out.is_empty() {
        tail_of(report, 4000)
    } else {
        out
    }
}

/// The simulators o7 made, by number, kept in `<devices>/ios/simulators.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sims {
    #[serde(default)]
    pub made: Vec<Made>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Made {
    pub number: usize,
    pub udid: String,
}

impl Sims {
    pub fn load(file: &Path) -> Sims {
        std::fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Sims>(&bytes).ok())
            .map(|mut sims| {
                sims.made
                    .retain(|made| udid(&made.udid).is_ok() && made.number > 0);
                sims
            })
            .unwrap_or_default()
    }

    pub fn save(&self, file: &Path) -> Result<(), Error> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|_| Error::Protocol("The simulators could not be written down.".into()))?;
        let temporary = file.with_extension("json.new");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(&temporary, file)?;
        Ok(())
    }

    pub fn udid_of(&self, number: usize) -> Option<&str> {
        self.made
            .iter()
            .find(|made| made.number == number)
            .map(|made| made.udid.as_str())
    }

    pub fn set(&mut self, number: usize, sim: &str) {
        self.made.retain(|made| made.number != number);
        self.made.push(Made {
            number,
            udid: sim.to_owned(),
        });
        self.made.sort_by_key(|made| made.number);
    }

    pub fn forget(&mut self, number: usize) {
        self.made.retain(|made| made.number != number);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNTIMES: &str = r#"{"runtimes":[
        {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-0","name":"iOS 26.0","version":"26.0","platform":"iOS","isAvailable":true,
         "supportedDeviceTypes":[{"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17","name":"iPhone 17","productFamily":"iPhone"}]},
        {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-1","name":"iOS 26.1","version":"26.1","platform":"iOS","isAvailable":true,
         "supportedDeviceTypes":[
            {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro-Max","name":"iPhone 17 Pro Max","productFamily":"iPhone"},
            {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro","name":"iPhone 17 Pro","productFamily":"iPhone"},
            {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-16e","name":"iPhone 16e","productFamily":"iPhone"},
            {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPad-Pro-13-inch-M5","name":"iPad Pro 13-inch (M5)","productFamily":"iPad"}]},
        {"identifier":"com.apple.CoreSimulator.SimRuntime.watchOS-26-0","name":"watchOS 26.0","version":"26.0","platform":"watchOS","isAvailable":true,"supportedDeviceTypes":[]},
        {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-18-6","name":"iOS 18.6","version":"18.6","platform":"iOS","isAvailable":false,"supportedDeviceTypes":[]}
    ]}"#;

    #[test]
    fn the_newest_ios_runtime_that_runs_comes_first_with_its_iphones() {
        let runtimes = parse_runtimes(RUNTIMES.as_bytes()).unwrap();
        assert_eq!(
            runtimes.len(),
            2,
            "watchOS and an unavailable runtime are left out"
        );
        assert_eq!(runtimes[0].name, "iOS 26.1");
        assert_eq!(runtimes[0].version, [26, 1]);
        assert_eq!(runtimes[0].device_types.len(), 3, "no iPad");
        let chosen = choose_device_type(&runtimes[0].device_types).unwrap();
        assert_eq!(chosen.name, "iPhone 17 Pro");
        assert_eq!(
            choose_device_type(&runtimes[1].device_types).unwrap().name,
            "iPhone 17"
        );
        assert!(choose_device_type(&[]).is_none());
        assert!(parse_runtimes(b"not json").is_err());
    }

    /// iOS 27's UIKit wrote one keyboard error dozens of times in a row on GitHub's Macs, which
    /// pushed the app's own lines out of the last 120.
    #[test]
    fn a_line_repeated_in_a_run_is_said_once_with_its_count() {
        let lines = [
            "2026-11-24 03:44:06.800 Df HelloO7[9780:888b] o7-e2e: tapped",
            "2026-11-24 03:44:06.865 E  HelloO7[9780:888b] [com.apple.UIKit:UIKBRenderingLog] <compose failure>",
            "2026-11-24 03:44:06.866 E  HelloO7[9780:888b] [com.apple.UIKit:UIKBRenderingLog] <compose failure>",
            "2026-11-24 03:44:06.867 E  HelloO7[9780:888b] [com.apple.UIKit:UIKBRenderingLog] <compose failure>",
            "2026-11-24 03:44:07.000 Df HelloO7[9780:888b] o7-e2e: typed 3 characters",
            "short",
            "short",
        ];
        assert_eq!(
            collapse_repeats(&lines),
            [
                "2026-11-24 03:44:06.800 Df HelloO7[9780:888b] o7-e2e: tapped",
                "2026-11-24 03:44:06.865 E  HelloO7[9780:888b] [com.apple.UIKit:UIKBRenderingLog] <compose failure> (the same line 3 times)",
                "2026-11-24 03:44:07.000 Df HelloO7[9780:888b] o7-e2e: typed 3 characters",
                "short (the same line 2 times)",
            ]
        );
        assert!(collapse_repeats(&[]).is_empty());
    }

    /// On Xcode 27, `log show` inside the simulator composed none of an app's own lines.
    #[test]
    fn log_lines_with_no_text_are_left_out() {
        assert!(!composed(
            "2026-11-24 04:11:43.592 E  HelloO7[14946:ce92] [com.apple.corehaptics:hapi] <compose failure [shared UUID]>"
        ));
        assert!(!composed(
            "2026-11-24 04:11:43.684 A  HelloO7[14946:ce92] (C54AD6FA-9FF2-3EDB-9C8A-53A82682D342) "
        ));
        assert!(composed(
            "2026-11-24 04:11:44.101 Sd HelloO7[14946:0] (B76E27F9-3E90-367E-B5D5-8D8BDC140E09) IOHIDManager State"
        ));
        assert!(composed(
            "2026-11-24 04:11:44.200 Df HelloO7[14946:ce92] o7-e2e: tapped"
        ));
        assert!(composed(
            "2026-11-24 04:11:44.300 Df HelloO7[1:2] loaded (2 items)"
        ));
        assert!(composed("    DispatchQueue = 0;"));
        assert!(is_uuid("C54AD6FA-9FF2-3EDB-9C8A-53A82682D342"));
        assert!(!is_uuid("2 items"));
    }

    #[test]
    fn text_is_found_anywhere_in_axes_tree() {
        let tree: serde_json::Value = serde_json::from_str(
            r#"[{"type":"Application","children":[{"type":"TextField","AXValue":"日本 ok","frame":{"x":1}}]}]"#,
        )
        .unwrap();
        assert!(holds_text(&tree, "日本 ok"));
        assert!(holds_text(&tree, "Application"));
        assert!(!holds_text(&tree, "Typed"));
    }

    /// Xcode 27 on GitHub's Macs lists a runtime made for one new device beside the general one
    /// of the same version, first: it ran only that device, a foldable whose screen stayed black.
    #[test]
    fn a_runtime_made_for_one_device_comes_after_the_general_ones() {
        let listed = r#"{"runtimes":[
            {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-27-2-iPhone19-4","name":"iOS 27.2","version":"27.2","platform":"iOS","isAvailable":true,
             "supportedDeviceTypes":[{"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-Duo","name":"iPhone Duo","productFamily":"iPhone"}]},
            {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-27-1","name":"iOS 27.1","version":"27.1","platform":"iOS","isAvailable":true,
             "supportedDeviceTypes":[{"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-18-Pro","name":"iPhone 18 Pro","productFamily":"iPhone"}]},
            {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-27-2","name":"iOS 27.2","version":"27.2","platform":"iOS","isAvailable":true,
             "supportedDeviceTypes":[
                {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-Duo","name":"iPhone Duo","productFamily":"iPhone"},
                {"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-18-Pro","name":"iPhone 18 Pro","productFamily":"iPhone"}]},
            {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-27-0","name":"iOS 27.0","version":"27.0","platform":"iOS","isAvailable":true,
             "supportedDeviceTypes":[{"identifier":"com.apple.CoreSimulator.SimDeviceType.iPad-Air-M4","name":"iPad Air (M4)","productFamily":"iPad"}]}
        ]}"#;
        let runtimes = parse_runtimes(listed.as_bytes()).unwrap();
        let order: Vec<&str> = runtimes
            .iter()
            .map(|runtime| runtime.identifier.as_str())
            .collect();
        assert_eq!(
            order,
            [
                "com.apple.CoreSimulator.SimRuntime.iOS-27-2",
                "com.apple.CoreSimulator.SimRuntime.iOS-27-1",
                "com.apple.CoreSimulator.SimRuntime.iOS-27-2-iPhone19-4",
            ],
            "a runtime with no iPhone is left out"
        );
        assert_eq!(
            choose_device_type(&runtimes[0].device_types).unwrap().name,
            "iPhone 18 Pro"
        );
        assert!(!device_specific(
            "com.apple.CoreSimulator.SimRuntime.iOS-26-5"
        ));
        assert!(device_specific(
            "com.apple.CoreSimulator.SimRuntime.iOS-27-2-iPhone19-4"
        ));
        assert!(!device_specific("not a runtime"));
    }

    #[test]
    fn simulators_are_listed_with_their_state() {
        let json = r#"{"devices":{
            "com.apple.CoreSimulator.SimRuntime.iOS-26-1":[
                {"udid":"0D3B2A61-4C8E-4D0A-9C55-1B9E9B7B0A01","name":"o7 iPhone 1","state":"Booted","isAvailable":true},
                {"udid":"7A1E0F33-2B6C-4E5D-8F70-AB12CD34EF56","name":"iPhone 17","state":"Shutdown","isAvailable":false}]}}"#;
        let devices = parse_devices(json.as_bytes()).unwrap();
        assert_eq!(devices.len(), 2);
        assert!(devices[0].booted && devices[0].available);
        assert_eq!(
            devices[0].runtime,
            "com.apple.CoreSimulator.SimRuntime.iOS-26-1"
        );
        assert!(!devices[1].booted && !devices[1].available);
    }

    #[test]
    fn identities_and_bundle_ids_are_strict() {
        assert!(udid("0D3B2A61-4C8E-4D0A-9C55-1B9E9B7B0A01").is_ok());
        assert!(udid(" 0D3B2A61-4C8E-4D0A-9C55-1B9E9B7B0A01\n").is_ok());
        for bad in [
            "--set",
            "booted",
            "0D3B2A61-4C8E-4D0A-9C55-1B9E9B7B0A0",
            "../../etc/passwd",
            "",
        ] {
            assert!(udid(bad).is_err(), "{bad}");
        }
        assert_eq!(
            bundle_id("com.example.my-app").unwrap(),
            "com.example.my-app"
        );
        assert!(bundle_id("com.apple.Preferences").is_ok());
        for bad in [
            "", "-com.x", ".com", "com..x", "com.x.", "com x", "com.x;rm", "com/x", "a\"b",
        ] {
            assert!(bundle_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_apps_log_keeps_its_own_lines_and_apples_errors() {
        assert_eq!(
            app_log_predicate("HelloO7"),
            "process == \"HelloO7\" AND (NOT (subsystem BEGINSWITH \"com.apple.\") OR messageType == error OR messageType == fault)"
        );
    }

    #[test]
    fn only_plain_text_is_typed_key_by_key() {
        assert!(keyboard_text("Ada Lovelace, 1815!\nok\t"));
        assert!(!keyboard_text("\u{65e5}\u{672c} ok"));
        assert!(!keyboard_text("Gr\u{fc}\u{df}e"));
        assert!(!keyboard_text("\u{1f642}"));
        assert!(!keyboard_text("a\u{7}b"));
    }

    #[test]
    fn text_sizes_map_to_apples_steps() {
        assert_eq!(content_size(1.0).unwrap(), "large");
        assert_eq!(content_size(0.85).unwrap(), "extra-small");
        assert_eq!(content_size(1.3).unwrap(), "extra-extra-extra-large");
        assert_eq!(content_size(2.0).unwrap(), "accessibility-large");
        assert_eq!(
            content_size(3.0).unwrap(),
            "accessibility-extra-extra-extra-large"
        );
        assert!(content_size(0.5).is_err());
        assert!(content_size(f32::NAN).is_err());
    }

    #[test]
    fn xcode_tools_answers_are_read() {
        assert_eq!(
            parse_xcode_version("Xcode 26.1\nBuild version 17B55\n").as_deref(),
            Some("26.1")
        );
        assert_eq!(
            parse_xcode_version("Xcode 27.0 beta\n").as_deref(),
            Some("27.0 beta")
        );
        assert_eq!(parse_xcode_version("xcodebuild: error"), None);
        assert!(license_refusal(
            "You have not agreed to the Xcode license agreements. Please run 'sudo xcodebuild -license'"
        ));
        assert!(!license_refusal(
            "Unable to boot device in current state: Booted"
        ));
        let info = "{\n    ApplicationType = User;\n    CFBundleExecutable = \"Hello o7\";\n    CFBundleIdentifier = \"com.o7.e2e\";\n}";
        assert_eq!(parse_executable(info).as_deref(), Some("Hello o7"));
        assert_eq!(
            parse_executable("CFBundleExecutable = Hello;").as_deref(),
            Some("Hello")
        );
        assert_eq!(parse_executable("nothing"), None);
        let launched = "PID\tStatus\tLabel\n-\t0\tcom.apple.thing\n812\t0\tUIKitApplication:com.apple.Preferences[1a2b][rb-legacy]\n913\t0\tUIKitApplication:com.o7.e2e[77][rb-legacy]\n";
        assert_eq!(parse_launched(launched, 913).as_deref(), Some("com.o7.e2e"));
        assert_eq!(
            parse_launched(launched, 812).as_deref(),
            Some("com.apple.Preferences")
        );
        assert_eq!(parse_launched(launched, 1), None);
    }

    #[test]
    fn axes_answers_are_read_from_its_envelope() {
        let done = br#"{"command":"displays","data":{"result":{"udid":"X","name":"o7 iPhone 1","activeScreen":{"id":1,"uuid":"U","role":"main","width":402,"height":874,"scale":3,"rotation":0,"orientation":"portrait","state":"on"},"otherScreens":[]}},"didError":false,"nextSteps":[],"schema":"axe.output.command-result","schemaVersion":"1"}"#;
        let Some(Answer::Done(result)) = parse_answer(done) else {
            panic!("not read")
        };
        let display = parse_display(&result).unwrap();
        assert_eq!(
            (display.width, display.height, display.scale),
            (402.0, 874.0, 3.0)
        );
        assert_eq!(display.pixels(), (1206, 2622));
        let failed = b"Starting...\n{\"command\":\"tap\",\"data\":{},\"didError\":true,\"error\":{\"code\":\"elementNotFound\",\"message\":\"No element.\"}}\n";
        assert_eq!(
            parse_answer(failed),
            Some(Answer::Failed {
                code: "elementNotFound".into(),
                message: "No element.".into()
            })
        );
        assert_eq!(parse_answer(b"Error: something\n"), None);
        assert!(
            parse_display(&serde_json::json!({ "activeScreen": { "width": 0, "height": 10 } }))
                .is_none()
        );
        assert!(matches!(
            axe_error("Tapping", "notBooted", ""),
            Error::DeviceGone
        ));
        assert!(matches!(
            axe_error("Typing", "invalidArguments", "no key"),
            Error::Refused(_)
        ));
    }

    #[test]
    fn the_screen_is_measured_from_the_outline_and_a_picture() {
        let tree =
            br#"[{"type":"Application","frame":{"x":0,"y":0,"width":402,"height":874},"pid":42}]"#;
        assert_eq!(root_frame(tree), Some((402.0, 874.0)));
        assert_eq!(outline_pid(tree), Some(42));
        assert_eq!(
            root_frame(br#"{"type":"Application","frame":{"width":0,"height":5}}"#),
            None
        );
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&1206u32.to_be_bytes());
        png.extend_from_slice(&2622u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((1206, 2622)));
        assert_eq!(png_size(b"GIF89a"), None);
        let xcode = |version: &str| Xcode {
            developer: PathBuf::from("/x"),
            version: version.into(),
        };
        assert_eq!(xcode("26.6").major(), 26);
        assert_eq!(xcode("27.0 beta").major(), 27);
        assert_eq!(xcode("").major(), 0);
    }

    #[test]
    fn a_jpegs_size_is_read_from_its_frame_header() {
        // SOI, an APP0 segment, then a baseline frame header for 603 by 1311.
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x4a, 0x46];
        jpeg.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08, 0x05, 0x1f, 0x02, 0x5b, 0x03]);
        jpeg.extend_from_slice(&[0; 16]);
        assert_eq!(jpeg_size(&jpeg), Some((603, 1311)));
        assert_eq!(jpeg_size(b"\x89PNG"), None);
        assert_eq!(jpeg_size(&[0xff, 0xd8, 0xff]), None);
    }

    #[tokio::test]
    async fn the_raw_stream_is_read_picture_by_picture() {
        let mut bytes = Vec::new();
        for picture in [&b"first"[..], &b"second!"[..]] {
            bytes.extend_from_slice(&(picture.len() as u32).to_be_bytes());
            bytes.extend_from_slice(picture);
        }
        let mut reader = &bytes[..];
        assert_eq!(
            next_frame(&mut reader).await.unwrap().as_deref(),
            Some(&b"first"[..])
        );
        assert_eq!(
            next_frame(&mut reader).await.unwrap().as_deref(),
            Some(&b"second!"[..])
        );
        assert_eq!(next_frame(&mut reader).await.unwrap(), None);
        let mut huge = &[0xff, 0xff, 0xff, 0xff][..];
        assert!(matches!(next_frame(&mut huge).await, Err(Error::TooLarge)));
        let mut cut = &[0, 0, 0, 9, 1, 2][..];
        assert!(next_frame(&mut cut).await.is_err());
    }

    #[test]
    fn the_panes_pictures_are_about_fourteen_hundred_pixels_long() {
        assert_eq!(stream_scale((1206, 2622)), 0.53);
        assert_eq!(stream_scale((640, 1136)), 1.0);
        assert_eq!(stream_scale((0, 0)), 1.0);
    }

    #[test]
    fn a_crash_report_is_summarised() {
        let report = concat!(
            "{\"app_name\":\"Hello\",\"bundleID\":\"com.o7.e2e\"}\n",
            "{\"exception\":{\"type\":\"EXC_CRASH\",\"signal\":\"SIGABRT\"},",
            "\"termination\":{\"indicator\":\"Abort trap: 6\"},\"faultingThread\":0,",
            "\"usedImages\":[{\"name\":\"Hello\"},{\"name\":\"libsystem_kernel.dylib\"}],",
            "\"threads\":[{\"frames\":[{\"imageIndex\":1,\"symbol\":\"__pthread_kill\"},{\"imageIndex\":0,\"symbol\":\"-[ViewController crash:]\"}]}]}"
        );
        let summary = summarise_crash(report);
        assert!(
            summary.contains("Exception: EXC_CRASH (SIGABRT)"),
            "{summary}"
        );
        assert!(summary.contains("Termination: Abort trap: 6"), "{summary}");
        assert!(
            summary.contains("Hello  -[ViewController crash:]"),
            "{summary}"
        );
        assert_eq!(summarise_crash("not a report"), "not a report");
    }

    #[test]
    fn the_simulators_o7_made_are_remembered_by_number() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("ios").join("simulators.json");
        assert_eq!(Sims::load(&file), Sims::default());
        let mut sims = Sims::default();
        sims.set(2, "7A1E0F33-2B6C-4E5D-8F70-AB12CD34EF56");
        sims.set(1, "0D3B2A61-4C8E-4D0A-9C55-1B9E9B7B0A01");
        sims.save(&file).unwrap();
        let loaded = Sims::load(&file);
        assert_eq!(
            loaded.udid_of(1),
            Some("0D3B2A61-4C8E-4D0A-9C55-1B9E9B7B0A01")
        );
        assert_eq!(loaded.made[0].number, 1);
        let mut loaded = loaded;
        loaded.forget(1);
        assert_eq!(loaded.udid_of(1), None);
        // A file someone edited to point elsewhere loses what is not a simulator's identity.
        std::fs::write(&file, r#"{"made":[{"number":1,"udid":"--set /tmp"},{"number":0,"udid":"0D3B2A61-4C8E-4D0A-9C55-1B9E9B7B0A01"}]}"#).unwrap();
        assert!(Sims::load(&file).made.is_empty());
    }

    #[tokio::test]
    async fn off_a_mac_there_is_no_simulator() {
        if !cfg!(target_os = "macos") {
            assert_eq!(find_xcode().await.unwrap_err(), Missing::NotMac);
            assert_eq!(
                Missing::NotMac.to_string(),
                "iPhone simulators run only on a Mac."
            );
        }
    }

    #[tokio::test]
    async fn a_helper_program_is_held_to_its_limits() {
        // `run` itself, with a program every system has.
        let program = if cfg!(windows) { "cmd" } else { "sh" };
        let mut command = tokio::process::Command::new(program);
        if cfg!(windows) {
            command.args(["/C", "echo hello"]);
        } else {
            command.args(["-c", "cat; echo err >&2"]);
        }
        let input = (!cfg!(windows)).then(|| b"hello".to_vec());
        let ran = run(command, input, 1024, Duration::from_secs(20))
            .await
            .unwrap();
        assert!(ran.ok);
        assert!(ran.text().contains("hello"));
        let mut command = tokio::process::Command::new(program);
        if cfg!(windows) {
            command.args(["/C", "echo 0123456789"]);
        } else {
            command.args(["-c", "echo 0123456789"]);
        }
        assert!(matches!(
            run(command, None, 4, Duration::from_secs(20)).await,
            Err(Error::TooLarge)
        ));
    }
}
