//! Who has which device.
//!
//! The manager knows what is installed, which emulators run, which phones are plugged in, and
//! which chat holds which device. A chat holds at most one device and a device is held by at most
//! one chat: a chat's worktree is its own copy of a project, so two chats on one device would
//! install over each other. Everything the pane and agents do goes through here, by chat id.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::broadcast;

use crate::adb::{Adb, DeviceState as Connection, Listed};
use crate::android::Device;
use crate::emulator::{self, Emulator, Launch};
use crate::install::{self, Need, Plan};
use crate::outline::Outline;
use crate::repo::Host;
use crate::scrcpy::{self, Control, Packet, Touch};
use crate::sdk::{self, Acceleration, Installed, Places};
use crate::{Error, avd};

/// How many emulators may run at once unless the person says otherwise. Each takes 2 to 4 GB.
pub const DEFAULT_RUNNING: usize = 2;
/// How many emulators o7 keeps on disk at most.
const SLOTS: usize = 4;
/// A device nobody watches and no agent uses is stopped after this long.
const IDLE: Duration = Duration::from_secs(600);
/// How long an emulator may take to start (a first start is about a minute).
const BOOT: Duration = Duration::from_secs(300);
/// The id prefix of a device that o7 did not start: a phone on a cable, or someone's emulator.
const PHONE: &str = "adb:";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where things are and how many may run.
#[derive(Debug, Clone)]
pub struct Config {
    /// o7's folder for devices (`<data>/devices`).
    pub root: PathBuf,
    pub places: Places,
    pub adb: Adb,
    pub max_running: usize,
}

impl Config {
    fn android(&self) -> PathBuf {
        self.root.join("android")
    }

    fn sdk(&self) -> PathBuf {
        self.android().join("sdk")
    }

    fn avds(&self) -> PathBuf {
        self.android().join("avd")
    }
}

/// Everything the pane shows, as one value. It is small, and sent whole on every change.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct MobileStatus {
    /// The Mobile plugin is on.
    pub enabled: bool,
    /// Why Android devices cannot run on this computer at all, when they cannot.
    pub unavailable: Option<String>,
    pub setup: Setup,
    pub devices: Vec<DeviceInfo>,
    pub chats: Vec<ChatDevice>,
}

/// What is installed and what is not.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct Setup {
    /// adb was found: phones on a cable can be used.
    pub adb: bool,
    /// An emulator and an image for it were found: a device can be started.
    pub emulator: bool,
    /// Whether the emulator can run fast here, once it is installed and was asked.
    pub acceleration: Option<AccelerationInfo>,
    pub fetch: Fetch,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct AccelerationInfo {
    pub usable: bool,
    /// The emulator's own words.
    pub detail: String,
    /// The one thing to do about it on this system, when it is not usable.
    pub fix: Option<String>,
}

/// Fetching the SDK packages.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Fetch {
    Idle,
    /// Reading Google's index.
    Planning,
    /// What would be fetched, and the licence to accept first.
    Offer { items: Vec<FetchItem>, total_mb: f64, license: String },
    Running { name: String, phase: FetchPhase, received_mb: f64, total_mb: f64 },
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct FetchItem {
    pub name: String,
    pub size_mb: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum FetchPhase {
    Downloading,
    Checking,
    Unpacking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// An emulator o7 made and runs.
    Emulator,
    /// A device o7 did not start: a phone on a cable, or an emulator from another program.
    Phone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Stopped,
    Starting,
    Ready,
    Stopping,
    /// A phone that has not allowed this computer yet.
    Unauthorized,
    /// A phone adb knows but cannot reach.
    Offline,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct DeviceInfo {
    pub id: String,
    pub kind: DeviceKind,
    pub title: String,
    pub state: DeviceState,
    /// Why it stopped or would not start, when it did not.
    pub error: Option<String>,
    /// The chat that holds it.
    pub chat_id: Option<String>,
    /// Its process, for the Processes page.
    pub pid: Option<u32>,
}

/// A chat's device.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct ChatDevice {
    pub chat_id: String,
    pub device_id: String,
    pub kind: DeviceKind,
    pub title: String,
    /// The device is being started or connected to.
    pub starting: bool,
    /// Its screen is live: there is video to watch and it takes input.
    pub live: bool,
    /// Whether the chat's agent may use it. Always true for o7's own emulators.
    pub agent_allowed: bool,
    /// What the agent is doing on it right now ("Tapping d3").
    pub agent: Option<String>,
    pub error: Option<String>,
}

/// What a tool answers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolReply {
    pub text: String,
    /// A PNG.
    pub image: Option<Vec<u8>>,
    pub error: bool,
}

impl ToolReply {
    pub(crate) fn text(text: impl Into<String>) -> ToolReply {
        ToolReply { text: text.into(), image: None, error: false }
    }

    pub(crate) fn failed(text: impl Into<String>) -> ToolReply {
        ToolReply { text: text.into(), image: None, error: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Stopped,
    Starting,
    Ready,
    Stopping,
}

/// One emulator o7 keeps.
#[derive(Debug)]
struct Slot {
    name: String,
    title: String,
    phase: Phase,
    port: Option<u16>,
    emulator: Option<Emulator>,
    error: Option<String>,
}

impl Slot {
    fn numbered(number: usize) -> Slot {
        Slot {
            name: format!("o7_phone_{number}"),
            title: format!("Phone {number}"),
            phase: Phase::Stopped,
            port: None,
            emulator: None,
            error: None,
        }
    }
}

/// What a chat holds.
struct Held {
    device_id: String,
    kind: DeviceKind,
    title: String,
    starting: bool,
    live: Option<Arc<Live>>,
    agent_allowed: bool,
    agent: Option<String>,
    error: Option<String>,
    used: Instant,
}

/// One packet for a viewer.
#[derive(Debug, Clone)]
enum Shown {
    Size(u16, u16),
    Config(Arc<[u8]>),
    Frame { key: bool, pts: u64, data: Arc<[u8]> },
    Ended,
}

/// A device whose screen is being read: its video, its input, and what was last learned of it.
pub(crate) struct Live {
    pub(crate) device: Device,
    control: tokio::sync::Mutex<Control>,
    /// The video's size now.
    size: Mutex<(u16, u16)>,
    config: Mutex<Option<Arc<[u8]>>>,
    video: broadcast::Sender<Shown>,
    /// The display's size in its natural position.
    display: (u32, u32),
    /// The outline an agent last read: its refs are what `mobile_tap` means.
    pub(crate) outline: Mutex<Option<Outline>>,
    /// The app an agent last installed or started.
    pub(crate) app: Mutex<Option<String>>,
    viewers: AtomicUsize,
    reader: tokio::task::AbortHandle,
    _server: scrcpy::Server,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl Live {
    fn video_size(&self) -> (u16, u16) {
        *lock(&self.size)
    }

    /// The display's size the way it is turned now.
    pub(crate) fn screen(&self) -> (u32, u32) {
        let (width, height) = self.display;
        let (video_width, video_height) = self.video_size();
        if (video_width > video_height) != (width > height) { (height, width) } else { (width, height) }
    }

    /// A point of the display, in the video's pixels.
    fn to_video(&self, x: i32, y: i32) -> (u32, u32, (u16, u16)) {
        let size = self.video_size();
        let (width, height) = self.screen();
        scale_point(x, y, (width, height), size)
    }

    pub(crate) async fn touch_video(&self, phase: Touch, x: u32, y: u32) -> Result<(), Error> {
        let size = self.video_size();
        self.control.lock().await.touch(phase, x, y, size).await
    }

    /// A tap at a point of the display, held for `hold`.
    pub(crate) async fn tap(&self, x: i32, y: i32, hold: Duration) -> Result<(), Error> {
        let (x, y, size) = self.to_video(x, y);
        let mut control = self.control.lock().await;
        control.touch(Touch::Down, x, y, size).await?;
        tokio::time::sleep(hold).await;
        control.touch(Touch::Up, x, y, size).await
    }

    /// A finger drawn from one point of the display to another over `duration`.
    pub(crate) async fn swipe(
        &self,
        from: (i32, i32),
        to: (i32, i32),
        duration: Duration,
    ) -> Result<(), Error> {
        const STEPS: i32 = 16;
        let (x, y, size) = self.to_video(from.0, from.1);
        let mut control = self.control.lock().await;
        control.touch(Touch::Down, x, y, size).await?;
        for step in 1..=STEPS {
            tokio::time::sleep(duration / STEPS as u32).await;
            let at = (
                from.0 + (to.0 - from.0) * step / STEPS,
                from.1 + (to.1 - from.1) * step / STEPS,
            );
            let (x, y, size) = self.to_video(at.0, at.1);
            control.touch(Touch::Move, x, y, size).await?;
        }
        // A pause at the end, so the drag is a drag and not a fling.
        tokio::time::sleep(Duration::from_millis(80)).await;
        let (x, y, size) = self.to_video(to.0, to.1);
        control.touch(Touch::Up, x, y, size).await
    }

    pub(crate) async fn control(&self) -> tokio::sync::MutexGuard<'_, Control> {
        self.control.lock().await
    }
}

/// `(x, y)` of a `from` sized display as a point of a `to` sized video, kept inside it.
fn scale_point(x: i32, y: i32, from: (u32, u32), to: (u16, u16)) -> (u32, u32, (u16, u16)) {
    let scale = |value: i32, from: u32, to: u16| {
        if from == 0 || to == 0 {
            return 0;
        }
        let scaled = i64::from(value.max(0)) * i64::from(to) / i64::from(from);
        scaled.clamp(0, i64::from(to) - 1) as u32
    };
    (scale(x, from.0, to.0), scale(y, from.1, to.1), to)
}

/// A device's video, as packets for the webview: one byte of kind, then the payload.
///
/// - `1`: the size, as two big-endian `u16`.
/// - `2`: the decoder's configuration.
/// - `3`: a key frame, as a big-endian `u64` time in microseconds, then the picture.
/// - `4`: a frame that builds on the one before, in the same form.
/// - `5`: the stream ended.
pub struct Viewer {
    live: Arc<Live>,
    receiver: broadcast::Receiver<Shown>,
    /// What to send before anything from the stream: the size and configuration already known.
    first: Vec<Vec<u8>>,
    /// A key frame has been sent, so the frames after it can be drawn.
    started: bool,
    ended: bool,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.live.viewers.fetch_sub(1, Ordering::AcqRel);
    }
}

fn encode(shown: &Shown) -> Vec<u8> {
    match shown {
        Shown::Size(width, height) => {
            let mut out = vec![1];
            out.extend_from_slice(&width.to_be_bytes());
            out.extend_from_slice(&height.to_be_bytes());
            out
        }
        Shown::Config(data) => {
            let mut out = Vec::with_capacity(1 + data.len());
            out.push(2);
            out.extend_from_slice(data);
            out
        }
        Shown::Frame { key, pts, data } => {
            let mut out = Vec::with_capacity(9 + data.len());
            out.push(if *key { 3 } else { 4 });
            out.extend_from_slice(&pts.to_be_bytes());
            out.extend_from_slice(data);
            out
        }
        Shown::Ended => vec![5],
    }
}

impl Viewer {
    /// The next packet, or `None` once the stream has ended and said so.
    pub async fn next(&mut self) -> Option<Vec<u8>> {
        if !self.first.is_empty() {
            return Some(self.first.remove(0));
        }
        if self.ended {
            return None;
        }
        loop {
            match self.receiver.recv().await {
                Ok(Shown::Frame { key: false, .. }) if !self.started => {}
                Ok(shown) => {
                    match &shown {
                        Shown::Frame { key: true, .. } => self.started = true,
                        // A new configuration means a new stream: wait for its key frame.
                        Shown::Config(_) => self.started = false,
                        Shown::Ended => self.ended = true,
                        _ => {}
                    }
                    return Some(encode(&shown));
                }
                // This viewer fell behind and frames were dropped: what follows cannot be drawn
                // until the encoder starts again.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    self.started = false;
                    let _ = self.live.control.lock().await.reset_video().await;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    self.ended = true;
                    return Some(encode(&Shown::Ended));
                }
            }
        }
    }
}

struct State {
    installed: Installed,
    acceleration: Option<Acceleration>,
    fetch: Fetch,
    plan: Option<Plan>,
    slots: Vec<Slot>,
    phones: Vec<Listed>,
    chats: HashMap<String, Held>,
    tracking: bool,
    reaping: bool,
    notified: Instant,
}

struct Inner {
    config: Config,
    host: Option<Host>,
    enabled: AtomicBool,
    cancel_fetch: AtomicBool,
    notify: Box<dyn Fn() + Send + Sync>,
    state: Mutex<State>,
}

/// The devices of one running o7.
#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Manager").finish_non_exhaustive()
    }
}

impl Manager {
    /// `notify` is called after every change to [`Manager::status`].
    pub fn new(config: Config, notify: impl Fn() + Send + Sync + 'static) -> Manager {
        let host = Host::current();
        let slots = (1..=SLOTS)
            .filter(|number| avd::exists(&config.avds(), &format!("o7_phone_{number}")))
            .map(Slot::numbered)
            .collect();
        Manager {
            inner: Arc::new(Inner {
                config,
                host,
                enabled: AtomicBool::new(true),
                cancel_fetch: AtomicBool::new(false),
                notify: Box::new(notify),
                state: Mutex::new(State {
                    installed: Installed::default(),
                    acceleration: None,
                    fetch: Fetch::Idle,
                    plan: None,
                    slots,
                    phones: Vec::new(),
                    chats: HashMap::new(),
                    tracking: false,
                    reaping: false,
                    notified: Instant::now(),
                }),
            }),
        }
    }

    fn changed(&self) {
        lock(&self.inner.state).notified = Instant::now();
        (self.inner.notify)();
    }

    pub fn enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Acquire)
    }

    fn check(&self) -> Result<(), Error> {
        if self.enabled() {
            Ok(())
        } else {
            Err(Error::Refused(TURNED_OFF.into()))
        }
    }

    /// Everything the pane shows.
    pub fn status(&self) -> MobileStatus {
        let state = lock(&self.inner.state);
        let holder = |device_id: &str| {
            state
                .chats
                .iter()
                .find(|(_, held)| held.device_id == device_id)
                .map(|(chat_id, _)| chat_id.clone())
        };
        let mut devices: Vec<DeviceInfo> = state
            .slots
            .iter()
            .map(|slot| DeviceInfo {
                id: slot.name.clone(),
                kind: DeviceKind::Emulator,
                title: slot.title.clone(),
                state: match slot.phase {
                    Phase::Stopped => DeviceState::Stopped,
                    Phase::Starting => DeviceState::Starting,
                    Phase::Ready => DeviceState::Ready,
                    Phase::Stopping => DeviceState::Stopping,
                },
                error: slot.error.clone(),
                chat_id: holder(&slot.name),
                pid: slot.emulator.as_ref().and_then(Emulator::pid),
            })
            .collect();
        devices.extend(state.phones.iter().map(|phone| {
            let id = format!("{PHONE}{}", phone.serial);
            DeviceInfo {
                chat_id: holder(&id),
                id,
                kind: DeviceKind::Phone,
                title: phone_title(phone),
                state: match &phone.state {
                    Connection::Device => DeviceState::Ready,
                    Connection::Unauthorized => DeviceState::Unauthorized,
                    _ => DeviceState::Offline,
                },
                error: None,
                pid: None,
            }
        }));
        let mut chats: Vec<ChatDevice> = state
            .chats
            .iter()
            .map(|(chat_id, held)| ChatDevice {
                chat_id: chat_id.clone(),
                device_id: held.device_id.clone(),
                kind: held.kind,
                title: held.title.clone(),
                starting: held.starting,
                live: held.live.is_some(),
                agent_allowed: held.agent_allowed,
                agent: held.agent.clone(),
                error: held.error.clone(),
            })
            .collect();
        chats.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
        MobileStatus {
            enabled: self.enabled(),
            unavailable: self.inner.host.is_none().then(|| {
                "Android's tools are not published for this kind of computer.".to_owned()
            }),
            setup: Setup {
                adb: state.installed.adb.is_some(),
                emulator: state.installed.can_emulate(),
                acceleration: state.acceleration.as_ref().map(|acceleration| AccelerationInfo {
                    usable: acceleration.usable,
                    detail: acceleration.detail.clone(),
                    fix: (!acceleration.usable).then(acceleration_fix).flatten(),
                }),
                fetch: state.fetch.clone(),
            },
            devices,
            chats,
        }
    }

    /// Turns Mobile on or off. Off stops every device o7 started and lets go of every phone.
    pub async fn set_enabled(&self, enabled: bool) {
        self.inner.enabled.store(enabled, Ordering::Release);
        if !enabled {
            self.inner.cancel_fetch.store(true, Ordering::Release);
            self.stop_everything(Duration::from_secs(20)).await;
        }
        self.changed();
    }

    /// Looks again at what is installed, and starts watching for phones once adb is there.
    pub async fn refresh(&self) {
        let Some(host) = self.inner.host else { return };
        let config = self.inner.config.clone();
        let installed =
            tokio::task::spawn_blocking(move || sdk::find(&config.sdk(), &config.places, host))
                .await
                .unwrap_or_default();
        let acceleration = match &installed.emulator_root {
            Some(root) => Some(sdk::acceleration(root).await),
            None => None,
        };
        {
            let mut state = lock(&self.inner.state);
            state.installed = installed;
            state.acceleration = acceleration;
        }
        self.changed();
        self.watch_phones();
        self.reap_idle();
    }

    /// Follows adb's device list for as long as Mobile is on.
    fn watch_phones(&self) {
        let adb_program = {
            let mut state = lock(&self.inner.state);
            if state.tracking || !self.enabled() {
                return;
            }
            let Some(adb) = state.installed.adb.clone() else { return };
            state.tracking = true;
            adb
        };
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                let Some(manager) = upgrade(&weak) else { return };
                if !manager.enabled() {
                    break;
                }
                let adb = manager.inner.config.adb.clone();
                drop(manager);
                if adb.ensure(&adb_program).await.is_ok()
                    && let Ok(mut tracker) = adb.track().await
                {
                    while let Ok(listed) = tracker.next().await {
                        let Some(manager) = upgrade(&weak) else { return };
                        manager.phones_changed(listed);
                    }
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            if let Some(manager) = upgrade(&weak) {
                lock(&manager.inner.state).tracking = false;
            }
        });
    }

    /// adb's list changed: phones came or went. o7's own emulators are not phones.
    fn phones_changed(&self, listed: Vec<Listed>) {
        {
            let mut state = lock(&self.inner.state);
            let ours: Vec<u16> = state.slots.iter().filter_map(|slot| slot.port).collect();
            state.phones = listed
                .into_iter()
                .filter(|device| !device.emulator_port().is_some_and(|port| ours.contains(&port)))
                .collect();
            // A chat whose phone is gone keeps its place and says why its screen is not there.
            let present: Vec<String> = state
                .phones
                .iter()
                .filter(|phone| phone.state == Connection::Device)
                .map(|phone| format!("{PHONE}{}", phone.serial))
                .collect();
            for held in state.chats.values_mut() {
                if held.kind == DeviceKind::Phone
                    && !held.starting
                    && !present.contains(&held.device_id)
                {
                    held.live = None;
                    held.error = Some("The phone was disconnected.".into());
                }
            }
        }
        self.changed();
    }

    /// Stops devices nobody is using, for as long as Mobile is on.
    fn reap_idle(&self) {
        {
            let mut state = lock(&self.inner.state);
            if state.reaping {
                return;
            }
            state.reaping = true;
        }
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let Some(manager) = upgrade(&weak) else { return };
                manager.stop_idle(IDLE).await;
            }
        });
    }

    /// Lets go of what has not been watched or used for `idle`, and stops emulators nobody holds.
    async fn stop_idle(&self, idle: Duration) {
        let stale: Vec<String> = {
            let state = lock(&self.inner.state);
            state
                .chats
                .iter()
                .filter(|(_, held)| {
                    !held.starting
                        && held.used.elapsed() >= idle
                        && held.live.as_ref().is_none_or(|live| {
                            live.viewers.load(Ordering::Acquire) == 0
                        })
                })
                .map(|(chat_id, _)| chat_id.clone())
                .collect()
        };
        for chat_id in stale {
            self.detach(&chat_id);
        }
        let unheld: Vec<usize> = {
            let state = lock(&self.inner.state);
            state
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| {
                    slot.phase == Phase::Ready
                        && !state.chats.values().any(|held| held.device_id == slot.name)
                })
                .map(|(index, _)| index)
                .collect()
        };
        for index in unheld {
            self.stop_slot(index, Duration::from_secs(40)).await;
        }
    }

    // ---------------------------------------------------------------- setup

    /// Reads Google's index and puts what is missing, and the licence, before the person.
    pub async fn fetch_offer(&self) {
        let Some(host) = self.inner.host else { return };
        let need = {
            let mut state = lock(&self.inner.state);
            if matches!(state.fetch, Fetch::Planning | Fetch::Running { .. }) {
                return;
            }
            state.fetch = Fetch::Planning;
            state.plan = None;
            let installed = &state.installed;
            let emulate = installed.can_emulate();
            Need {
                adb: installed.adb.is_none(),
                emulator: !emulate && !installed.own_emulator,
                image: !emulate && !installed.own_image,
            }
        };
        self.changed();
        let planned = async {
            if need.nothing() {
                return Err(Error::Refused("Android is already set up.".into()));
            }
            install::plan(&install::client()?, host, need).await
        }
        .await;
        {
            let mut state = lock(&self.inner.state);
            match planned {
                Ok(plan) => {
                    state.fetch = Fetch::Offer {
                        items: plan
                            .items
                            .iter()
                            .map(|item| FetchItem {
                                name: item.choice.name.clone(),
                                size_mb: megabytes(item.choice.archive.size),
                            })
                            .collect(),
                        total_mb: megabytes(plan.download_size()),
                        license: plan.license.clone(),
                    };
                    state.plan = Some(plan);
                }
                Err(error) => state.fetch = Fetch::Failed { message: error.to_string() },
            }
        }
        self.changed();
    }

    /// The person accepted the licence in the offer: fetch what it listed.
    pub fn fetch_start(&self) -> Result<(), Error> {
        self.check()?;
        let plan = {
            let mut state = lock(&self.inner.state);
            let Some(plan) = state.plan.take() else {
                return Err(Error::Refused("There is nothing offered to download.".into()));
            };
            state.fetch = Fetch::Running {
                name: plan.items.first().map(|item| item.choice.name.clone()).unwrap_or_default(),
                phase: FetchPhase::Downloading,
                received_mb: 0.0,
                total_mb: megabytes(plan.download_size()),
            };
            plan
        };
        self.inner.cancel_fetch.store(false, Ordering::Release);
        self.changed();
        let manager = self.clone();
        tokio::spawn(async move {
            let reporter = manager.clone();
            let report = move |progress: install::Progress| {
                let due = {
                    let mut state = lock(&reporter.inner.state);
                    let whole = progress.phase != install::Phase::Downloading;
                    state.fetch = Fetch::Running {
                        name: progress.name,
                        phase: match progress.phase {
                            install::Phase::Downloading => FetchPhase::Downloading,
                            install::Phase::Checking => FetchPhase::Checking,
                            install::Phase::Unpacking => FetchPhase::Unpacking,
                        },
                        received_mb: megabytes(progress.received),
                        total_mb: megabytes(progress.total),
                    };
                    whole || state.notified.elapsed() >= Duration::from_millis(250)
                };
                if due {
                    reporter.changed();
                }
            };
            let fetched = async {
                let client = install::client()?;
                let root = manager.inner.config.sdk();
                install::fetch(&client, &plan, &root, &report, &manager.inner.cancel_fetch).await
            }
            .await;
            lock(&manager.inner.state).fetch = match fetched {
                Ok(()) | Err(Error::Cancelled) => Fetch::Idle,
                Err(error) => Fetch::Failed { message: error.to_string() },
            };
            manager.refresh().await;
        });
        Ok(())
    }

    /// Stops a fetch, or puts an offer or a failure away.
    pub fn fetch_cancel(&self) {
        self.inner.cancel_fetch.store(true, Ordering::Release);
        {
            let mut state = lock(&self.inner.state);
            if !matches!(state.fetch, Fetch::Running { .. }) {
                state.fetch = Fetch::Idle;
                state.plan = None;
            }
        }
        self.changed();
    }

    // -------------------------------------------------------------- devices

    /// Gives `chat_id` a device and starts it: the one named, or one of o7's emulators. Returns
    /// at once; [`Manager::status`] says how it goes. A chat that already holds the device it
    /// asks for keeps it.
    pub fn attach(&self, chat_id: &str, device_id: Option<&str>) -> Result<(), Error> {
        self.check()?;
        let target = {
            let mut state = lock(&self.inner.state);
            if let Some(held) = state.chats.get_mut(chat_id) {
                held.used = Instant::now();
                let same = device_id.is_none_or(|device_id| device_id == held.device_id);
                if same && (held.starting || held.live.is_some()) {
                    return Ok(());
                }
            }
            let wanted = device_id
                .map(str::to_owned)
                .or_else(|| state.chats.get(chat_id).map(|held| held.device_id.clone()));
            let taken = |state: &State, id: &str| {
                state.chats.iter().any(|(other, held)| other != chat_id && held.device_id == id)
            };
            let target = match wanted {
                Some(id) if id.starts_with(PHONE) => {
                    let serial = &id[PHONE.len()..];
                    let phone = state.phones.iter().find(|phone| phone.serial == serial);
                    match phone.map(|phone| &phone.state) {
                        Some(Connection::Device) => {}
                        Some(Connection::Unauthorized) => return Err(Error::Unauthorized),
                        _ => return Err(Error::DeviceGone),
                    }
                    if taken(&state, &id) {
                        return Err(Error::Refused("Another chat is using that phone.".into()));
                    }
                    let title = phone.map(phone_title).unwrap_or_default();
                    Target { id, kind: DeviceKind::Phone, title, slot: None }
                }
                Some(id) => {
                    let Some(index) = state.slots.iter().position(|slot| slot.name == id) else {
                        return Err(Error::Refused("That device does not exist.".into()));
                    };
                    if taken(&state, &id) {
                        return Err(Error::Refused("Another chat is using that device.".into()));
                    }
                    let running =
                        state.slots.iter().filter(|slot| slot.phase != Phase::Stopped).count();
                    if state.slots[index].phase == Phase::Stopped
                        && running >= self.inner.config.max_running
                    {
                        return Err(Error::Refused(too_many(self.inner.config.max_running)));
                    }
                    let title = state.slots[index].title.clone();
                    Target { id, kind: DeviceKind::Emulator, title, slot: Some(index) }
                }
                None => {
                    if !state.installed.can_emulate() {
                        return Err(Error::Refused(NOT_SET_UP.into()));
                    }
                    let index = choose_slot(&mut state, self.inner.config.max_running)?;
                    let slot = &state.slots[index];
                    Target {
                        id: slot.name.clone(),
                        kind: DeviceKind::Emulator,
                        title: slot.title.clone(),
                        slot: Some(index),
                    }
                }
            };
            let agent_allowed = match state.chats.get(chat_id) {
                // The person's answer for a phone lasts while the chat keeps that phone.
                Some(held) if held.device_id == target.id => held.agent_allowed,
                _ => target.kind == DeviceKind::Emulator,
            };
            state.chats.insert(
                chat_id.to_owned(),
                Held {
                    device_id: target.id.clone(),
                    kind: target.kind,
                    title: target.title.clone(),
                    starting: true,
                    live: None,
                    agent_allowed,
                    agent: None,
                    error: None,
                    used: Instant::now(),
                },
            );
            target
        };
        self.changed();
        let manager = self.clone();
        let chat_id = chat_id.to_owned();
        tokio::spawn(async move {
            let brought = manager.bring_up(&chat_id, &target).await;
            {
                let mut state = lock(&manager.inner.state);
                // The chat may have let go, or taken another device, while this one started.
                if let Some(held) =
                    state.chats.get_mut(&chat_id).filter(|held| held.device_id == target.id)
                {
                    held.starting = false;
                    held.used = Instant::now();
                    match brought {
                        Ok(live) => held.live = Some(live),
                        Err(error) => held.error = Some(error.to_string()),
                    }
                }
            }
            manager.changed();
        });
        Ok(())
    }

    /// Starts the device if it is not running, and opens its screen.
    async fn bring_up(&self, chat_id: &str, target: &Target) -> Result<Arc<Live>, Error> {
        let adb_program = lock(&self.inner.state)
            .installed
            .adb
            .clone()
            .ok_or_else(|| Error::Refused(NOT_SET_UP.into()))?;
        let adb = self.inner.config.adb.clone();
        adb.ensure(&adb_program).await?;
        let serial = match target.slot {
            Some(index) => self.boot(index).await?,
            None => target.id[PHONE.len()..].to_owned(),
        };
        let session = scrcpy::start(&adb, &serial, scrcpy::Options::default()).await?;
        let device = Device::new(adb, serial);
        let display = device.display_size().await?;
        let scrcpy::Session { video, control, server, .. } = session;
        let (sender, _) = broadcast::channel(256);
        let for_reader = sender.clone();
        let weak = Arc::downgrade(&self.inner);
        let chat_id = chat_id.to_owned();
        let device_id = target.id.clone();
        let live = Arc::new_cyclic(|live: &Weak<Live>| {
            let live = live.clone();
            let reader = tokio::spawn(async move {
                let sender = for_reader;
                let mut video = video;
                loop {
                    let shown = match video.next().await {
                        Ok(Packet::Size { width, height }) => {
                            let size = (
                                u16::try_from(width).unwrap_or(u16::MAX),
                                u16::try_from(height).unwrap_or(u16::MAX),
                            );
                            if let Some(live) = live.upgrade() {
                                *lock(&live.size) = size;
                            }
                            Shown::Size(size.0, size.1)
                        }
                        Ok(Packet::Config(data)) => {
                            let data: Arc<[u8]> = data.into();
                            if let Some(live) = live.upgrade() {
                                *lock(&live.config) = Some(Arc::clone(&data));
                            }
                            Shown::Config(data)
                        }
                        Ok(Packet::Frame { key, pts, data }) => {
                            Shown::Frame { key, pts, data: data.into() }
                        }
                        Err(_) => break,
                    };
                    let _ = sender.send(shown);
                }
                let _ = sender.send(Shown::Ended);
                // The stream ended by itself: the device went away under the chat.
                if let Some(manager) = upgrade(&weak) {
                    manager.screen_ended(&chat_id, &device_id);
                }
            })
            .abort_handle();
            Live {
                device,
                control: tokio::sync::Mutex::new(control),
                size: Mutex::new((0, 0)),
                config: Mutex::new(None),
                video: sender,
                display,
                outline: Mutex::new(None),
                app: Mutex::new(None),
                viewers: AtomicUsize::new(0),
                reader,
                _server: server,
            }
        });
        Ok(live)
    }

    fn screen_ended(&self, chat_id: &str, device_id: &str) {
        {
            let mut state = lock(&self.inner.state);
            let Some(held) =
                state.chats.get_mut(chat_id).filter(|held| held.device_id == device_id)
            else {
                return;
            };
            if held.live.take().is_none() {
                return;
            }
            held.error = Some("The device's screen stopped. Start it again.".into());
        }
        self.changed();
    }

    /// Makes sure the emulator in slot `index` is running and has booted; returns its serial.
    async fn boot(&self, index: usize) -> Result<String, Error> {
        let deadline = Instant::now() + BOOT;
        loop {
            {
                let mut state = lock(&self.inner.state);
                let slot = state.slots.get_mut(index).ok_or(Error::DeviceGone)?;
                match (slot.phase, slot.port) {
                    (Phase::Ready, Some(port)) => return Ok(emulator::serial(port)),
                    (Phase::Stopped, _) | (Phase::Ready, None) => {
                        slot.phase = Phase::Starting;
                        slot.error = None;
                        break;
                    }
                    // Another chat's request is starting or stopping it: wait for that.
                    (Phase::Starting | Phase::Stopping, _) => {}
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        self.changed();
        let started = self.start_emulator(index, deadline).await;
        {
            let mut state = lock(&self.inner.state);
            if let Some(slot) = state.slots.get_mut(index) {
                match &started {
                    Ok(_) => slot.phase = Phase::Ready,
                    Err(error) => {
                        slot.phase = Phase::Stopped;
                        slot.port = None;
                        // Dropping the emulator ends its processes.
                        slot.emulator = None;
                        slot.error = Some(error.to_string());
                    }
                }
            }
        }
        self.changed();
        started
    }

    async fn start_emulator(&self, index: usize, deadline: Instant) -> Result<String, Error> {
        let config = &self.inner.config;
        let serial = {
            let mut state = lock(&self.inner.state);
            let root =
                state.installed.emulator_root.clone().ok_or_else(|| Error::Refused(NOT_SET_UP.into()))?;
            let image =
                state.installed.images.first().ok_or_else(|| Error::Refused(NOT_SET_UP.into()))?.path.clone();
            let taken: Vec<u16> = state.slots.iter().filter_map(|slot| slot.port).collect();
            let port = emulator::free_port(&taken).ok_or_else(|| {
                Error::Device("No port is free for another emulator.".into())
            })?;
            let slot = state.slots.get_mut(index).ok_or(Error::DeviceGone)?;
            // A device made for an image that is gone starts over on the image there is.
            if avd::image_of(&config.avds(), &slot.name).is_some_and(|have| have != image) {
                avd::remove(&config.avds(), &slot.name)?;
            }
            if !avd::exists(&config.avds(), &slot.name) {
                std::fs::create_dir_all(config.avds())?;
                avd::write(
                    &config.avds(),
                    &avd::Spec {
                        name: slot.name.clone(),
                        title: format!("o7 {}", slot.title),
                        image,
                        shape: avd::Shape::PHONE,
                        ram_mb: 2048,
                        cores: 4,
                        storage_gb: 6,
                    },
                )?;
            }
            let launch = Launch {
                sdk_root: root,
                avd_home: config.avds(),
                emulator_home: config.android().join("emulator-home"),
                name: slot.name.clone(),
                port,
                log: config.android().join("logs").join(format!("{}.log", slot.name)),
            };
            let emulator = emulator::start(&launch)?;
            let serial = emulator.serial.clone();
            slot.port = Some(port);
            slot.emulator = Some(emulator);
            serial
        };
        self.changed();
        let adb = &config.adb;
        loop {
            {
                let mut state = lock(&self.inner.state);
                let slot = state.slots.get_mut(index).ok_or(Error::DeviceGone)?;
                match slot.emulator.as_mut() {
                    // Someone stopped it while it started.
                    None => return Err(Error::Cancelled),
                    Some(emulator) => {
                        if emulator.ended() {
                            return Err(Error::Device(emulator.failure()));
                        }
                    }
                }
            }
            if emulator::booted(adb, &serial).await {
                // Android says it has booted a moment before its home screen is there to use:
                // wait until some app is in front, so the first look at the screen sees one.
                let device = Device::new(adb.clone(), serial.clone());
                let settled = Instant::now() + Duration::from_secs(25);
                while device.foreground().await.is_none() && Instant::now() < settled {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                return Ok(serial);
            }
            if Instant::now() >= deadline {
                return Err(Error::Device(
                    "The emulator did not finish starting. It may need more memory or disk space."
                        .into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    }

    /// Lets go of a chat's device. One of o7's emulators keeps running for a while, so coming
    /// back is quick; it stops by itself when nobody takes it.
    pub fn detach(&self, chat_id: &str) {
        let had = lock(&self.inner.state).chats.remove(chat_id).is_some();
        if had {
            self.changed();
        }
    }

    /// Stops one of o7's emulators, whoever holds it.
    pub async fn stop_device(&self, device_id: &str) -> Result<(), Error> {
        let index = lock(&self.inner.state)
            .slots
            .iter()
            .position(|slot| slot.name == device_id)
            .ok_or_else(|| Error::Refused("o7 can only stop the devices it started.".into()))?;
        self.stop_slot(index, Duration::from_secs(40)).await;
        Ok(())
    }

    async fn stop_slot(&self, index: usize, patience: Duration) {
        let (emulator, adb_program) = {
            let mut state = lock(&self.inner.state);
            let Some(slot) = state.slots.get_mut(index) else { return };
            if slot.phase == Phase::Stopped || slot.phase == Phase::Stopping {
                return;
            }
            slot.phase = Phase::Stopping;
            let emulator = slot.emulator.take();
            let name = slot.name.clone();
            state.chats.retain(|_, held| held.device_id != name);
            (emulator, state.installed.adb.clone())
        };
        self.changed();
        if let Some(emulator) = emulator {
            match adb_program {
                Some(adb_program) => {
                    emulator.stop(&adb_program, &self.inner.config.adb, patience).await;
                }
                None => emulator.kill().await,
            }
        }
        {
            let mut state = lock(&self.inner.state);
            if let Some(slot) = state.slots.get_mut(index) {
                slot.phase = Phase::Stopped;
                slot.port = None;
            }
        }
        self.changed();
    }

    async fn stop_everything(&self, patience: Duration) {
        let count = {
            let mut state = lock(&self.inner.state);
            state.chats.clear();
            state.slots.len()
        };
        let mut stops = Vec::new();
        for index in 0..count {
            let manager = self.clone();
            stops.push(tokio::spawn(async move { manager.stop_slot(index, patience).await }));
        }
        for stop in stops {
            let _ = stop.await;
        }
    }

    /// o7 is quitting: every emulator is asked to save and stop, and none outlives the app.
    pub async fn shutdown(&self) {
        self.inner.cancel_fetch.store(true, Ordering::Release);
        self.stop_everything(Duration::from_secs(12)).await;
    }

    /// The person's answer to "may the agent use this phone?", for the chat's device.
    pub fn allow_agent(&self, chat_id: &str, allowed: bool) {
        if let Some(held) = lock(&self.inner.state).chats.get_mut(chat_id) {
            held.agent_allowed = allowed || held.kind == DeviceKind::Emulator;
        }
        self.changed();
    }

    // ------------------------------------------------------------ the screen

    fn live(&self, chat_id: &str) -> Result<Arc<Live>, Error> {
        self.check()?;
        let mut state = lock(&self.inner.state);
        let held = state
            .chats
            .get_mut(chat_id)
            .ok_or_else(|| Error::Refused("This chat has no device.".into()))?;
        held.used = Instant::now();
        held.live.clone().ok_or_else(|| {
            Error::Refused(match (&held.error, held.starting) {
                (_, true) => "The device is still starting.".into(),
                (Some(error), _) => error.clone(),
                _ => "The device is not running.".into(),
            })
        })
    }

    /// The chat's device for its agent: only when the person allowed it.
    pub(crate) fn live_for_agent(&self, chat_id: &str) -> Result<Arc<Live>, Error> {
        self.check()?;
        {
            let state = lock(&self.inner.state);
            match state.chats.get(chat_id) {
                None => return Err(Error::Refused(NO_DEVICE.into())),
                Some(held) if !held.agent_allowed => {
                    return Err(Error::Refused(NOT_ALLOWED.into()));
                }
                Some(_) => {}
            }
        }
        self.live(chat_id)
    }

    /// Whether the chat holds a device its agent may not use.
    pub(crate) fn agent_refused(&self, chat_id: &str) -> bool {
        lock(&self.inner.state).chats.get(chat_id).is_some_and(|held| !held.agent_allowed)
    }

    /// Says in the pane what the agent is doing on the chat's device, or that it has finished.
    pub(crate) fn agent_doing(&self, chat_id: &str, label: Option<String>) {
        {
            let mut state = lock(&self.inner.state);
            let Some(held) = state.chats.get_mut(chat_id) else { return };
            if held.agent == label {
                return;
            }
            held.agent = label;
            held.used = Instant::now();
        }
        self.changed();
    }

    /// The chat's device as video. The encoder is asked to start again, so the viewer's first
    /// frame is one it can draw.
    pub async fn watch(&self, chat_id: &str) -> Result<Viewer, Error> {
        let live = self.live(chat_id)?;
        let receiver = live.video.subscribe();
        let mut first = Vec::new();
        let (width, height) = live.video_size();
        if width > 0 {
            first.push(encode(&Shown::Size(width, height)));
        }
        if let Some(config) = lock(&live.config).clone() {
            first.push(encode(&Shown::Config(config)));
        }
        live.viewers.fetch_add(1, Ordering::AcqRel);
        let viewer = Viewer { live: Arc::clone(&live), receiver, first, started: false, ended: false };
        live.control.lock().await.reset_video().await?;
        Ok(viewer)
    }

    /// The person's finger, at a point of the video they are watching.
    pub async fn touch(&self, chat_id: &str, phase: Touch, x: u32, y: u32) -> Result<(), Error> {
        self.live(chat_id)?.touch_video(phase, x, y).await
    }

    /// The person's wheel, at a point of the video.
    pub async fn scroll(
        &self,
        chat_id: &str,
        x: u32,
        y: u32,
        horizontal: f32,
        vertical: f32,
    ) -> Result<(), Error> {
        let live = self.live(chat_id)?;
        let size = live.video_size();
        live.control.lock().await.scroll(x, y, size, horizontal, vertical).await
    }

    /// A key from the person: one of [`scrcpy::key`].
    pub async fn press(&self, chat_id: &str, keycode: u32) -> Result<(), Error> {
        self.live(chat_id)?.control.lock().await.press(keycode).await
    }

    /// Text the person typed.
    pub async fn text(&self, chat_id: &str, text: &str) -> Result<(), Error> {
        self.live(chat_id)?.control.lock().await.text(text).await
    }

    pub async fn back(&self, chat_id: &str) -> Result<(), Error> {
        self.live(chat_id)?.control.lock().await.back().await
    }

    pub async fn rotate(&self, chat_id: &str) -> Result<(), Error> {
        self.live(chat_id)?.control.lock().await.rotate().await
    }

    /// The chat's screen as a PNG at its own size, for the person to save.
    pub async fn screenshot(&self, chat_id: &str) -> Result<Vec<u8>, Error> {
        self.live(chat_id)?.device.screenshot().await
    }

    /// Runs one of the agent's tools for `chat_id`.
    pub async fn tool(&self, chat_id: &str, tool: &str, arguments: &serde_json::Value) -> ToolReply {
        crate::tools::call(self, chat_id, tool, arguments).await
    }

    /// Waits until the chat's device is live, for a tool that just asked for it.
    pub(crate) async fn wait_live(&self, chat_id: &str, within: Duration) -> Result<Arc<Live>, Error> {
        let deadline = Instant::now() + within;
        loop {
            {
                let state = lock(&self.inner.state);
                match state.chats.get(chat_id) {
                    None => return Err(Error::Refused(NO_DEVICE.into())),
                    Some(held) => {
                        if let Some(live) = &held.live {
                            return Ok(Arc::clone(live));
                        }
                        if !held.starting {
                            return Err(Error::Device(
                                held.error.clone().unwrap_or_else(|| "The device did not start.".into()),
                            ));
                        }
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::Device(
                    "The device is still starting. Call mobile_start again in a moment.".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

/// What a device answers when Mobile is off.
pub const TURNED_OFF: &str = "Mobile is turned off in Settings > Plugins & MCP. Ask the person to turn it on.";
const NOT_SET_UP: &str = "Android is not set up on this computer yet. Ask the person to open the Device pane and set it up.";
const NO_DEVICE: &str = "This chat has no device yet. Call mobile_start first.";
const NOT_ALLOWED: &str = "This chat's device is the person's phone, and they have not allowed agents to use it. Ask them to turn on \"Let the agent use this phone\" in the Device pane.";

struct Target {
    id: String,
    kind: DeviceKind,
    title: String,
    slot: Option<usize>,
}

fn upgrade(weak: &Weak<Inner>) -> Option<Manager> {
    weak.upgrade().map(|inner| Manager { inner })
}

fn megabytes(bytes: u64) -> f64 {
    (bytes as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0
}

fn too_many(limit: usize) -> String {
    format!(
        "{limit} devices are already running, which is the most at once. Stop one, or close the Device pane in another chat, then try again."
    )
}

/// A phone's name: its model with the server's underscores as spaces, or its serial.
fn phone_title(phone: &Listed) -> String {
    match (&phone.model, phone.emulator_port()) {
        (_, Some(port)) => format!("Emulator on port {port}"),
        (Some(model), None) => model.replace('_', " "),
        (None, None) => phone.serial.clone(),
    }
}

/// One of o7's emulators for a chat that named none: a running one nobody holds, else a stopped
/// one, else a new one, while fewer than `max_running` run.
fn choose_slot(state: &mut State, max_running: usize) -> Result<usize, Error> {
    let free = |state: &State, slot: &Slot| {
        !state.chats.values().any(|held| held.device_id == slot.name)
    };
    if let Some(index) =
        state.slots.iter().position(|slot| slot.phase == Phase::Ready && free(state, slot))
    {
        return Ok(index);
    }
    let running = state.slots.iter().filter(|slot| slot.phase != Phase::Stopped).count();
    if running >= max_running {
        return Err(Error::Refused(too_many(max_running)));
    }
    if let Some(index) =
        state.slots.iter().position(|slot| slot.phase == Phase::Stopped && free(state, slot))
    {
        return Ok(index);
    }
    if state.slots.len() >= SLOTS {
        return Err(Error::Refused(too_many(state.slots.len())));
    }
    let number = (1..=SLOTS)
        .find(|number| !state.slots.iter().any(|slot| slot.name == format!("o7_phone_{number}")))
        .unwrap_or(state.slots.len() + 1);
    state.slots.push(Slot::numbered(number));
    Ok(state.slots.len() - 1)
}

/// What to do about acceleration that does not work, on this system.
fn acceleration_fix() -> Option<String> {
    match std::env::consts::OS {
        "windows" => Some(
            "Open \"Turn Windows features on or off\", tick Windows Hypervisor Platform, then restart the computer.".into(),
        ),
        "linux" => Some(
            "Turn on virtualization in the computer's firmware settings if it is off, add your user to the kvm group (sudo usermod -aG kvm $USER), then sign in again.".into(),
        ),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn manager(folder: &std::path::Path) -> Manager {
        Manager::new(
            Config {
                root: folder.to_owned(),
                places: Places::default(),
                // Nothing listens here: a test that reaches adb fails at once.
                adb: Adb::at(1),
                max_running: DEFAULT_RUNNING,
            },
            || {},
        )
    }

    /// Puts a phone in adb's list and gives it to `chat_id`, as if the person had picked it.
    pub(crate) fn hold_phone(manager: &Manager, chat_id: &str, serial: &str) {
        let mut state = lock(&manager.inner.state);
        state.phones.push(Listed {
            serial: serial.into(),
            state: Connection::Device,
            model: Some("Pixel_8".into()),
        });
        state.chats.insert(
            chat_id.into(),
            Held {
                device_id: format!("{PHONE}{serial}"),
                kind: DeviceKind::Phone,
                title: "Pixel 8".into(),
                starting: false,
                live: None,
                agent_allowed: false,
                agent: None,
                error: None,
                used: Instant::now(),
            },
        );
    }

    fn state_with(slots: Vec<(Phase, Option<&str>)>) -> State {
        let mut state = State {
            installed: Installed::default(),
            acceleration: None,
            fetch: Fetch::Idle,
            plan: None,
            slots: Vec::new(),
            phones: Vec::new(),
            chats: HashMap::new(),
            tracking: false,
            reaping: false,
            notified: Instant::now(),
        };
        for (number, (phase, chat)) in slots.into_iter().enumerate() {
            let mut slot = Slot::numbered(number + 1);
            slot.phase = phase;
            if let Some(chat) = chat {
                state.chats.insert(
                    chat.into(),
                    Held {
                        device_id: slot.name.clone(),
                        kind: DeviceKind::Emulator,
                        title: slot.title.clone(),
                        starting: false,
                        live: None,
                        agent_allowed: true,
                        agent: None,
                        error: None,
                        used: Instant::now(),
                    },
                );
            }
            state.slots.push(slot);
        }
        state
    }

    #[test]
    fn a_chat_gets_a_running_free_device_before_a_new_one() {
        let mut state = state_with(vec![(Phase::Ready, Some("a")), (Phase::Ready, None)]);
        assert_eq!(choose_slot(&mut state, 2).unwrap(), 1);
        let mut state = state_with(vec![(Phase::Ready, Some("a")), (Phase::Stopped, None)]);
        assert_eq!(choose_slot(&mut state, 2).unwrap(), 1);
        // Nothing made yet: the first device is made.
        let mut state = state_with(vec![]);
        assert_eq!(choose_slot(&mut state, 2).unwrap(), 0);
        assert_eq!(state.slots[0].name, "o7_phone_1");
    }

    #[test]
    fn the_limit_on_running_devices_is_kept() {
        let mut state = state_with(vec![
            (Phase::Ready, Some("a")),
            (Phase::Starting, Some("b")),
            (Phase::Stopped, None),
        ]);
        let refused = choose_slot(&mut state, 2).unwrap_err().to_string();
        assert!(refused.starts_with("2 devices are already running"), "{refused}");
        assert_eq!(choose_slot(&mut state, 3).unwrap(), 2);
        // Every slot held: no fifth device is made.
        let mut state = state_with(vec![
            (Phase::Stopped, Some("a")),
            (Phase::Stopped, Some("b")),
            (Phase::Stopped, Some("c")),
            (Phase::Stopped, Some("d")),
        ]);
        assert!(choose_slot(&mut state, 9).is_err());
        assert_eq!(state.slots.len(), SLOTS);
    }

    #[test]
    fn points_scale_to_the_video_and_stay_inside_it() {
        // A 1080x2400 display shown as a 720x1600 video.
        assert_eq!(scale_point(540, 1200, (1080, 2400), (720, 1600)), (360, 800, (720, 1600)));
        assert_eq!(scale_point(0, 0, (1080, 2400), (720, 1600)), (0, 0, (720, 1600)));
        assert_eq!(scale_point(1080, 2400, (1080, 2400), (720, 1600)), (719, 1599, (720, 1600)));
        assert_eq!(scale_point(-50, 99_999, (1080, 2400), (720, 1600)), (0, 1599, (720, 1600)));
        assert_eq!(scale_point(5, 5, (0, 0), (720, 1600)), (0, 0, (720, 1600)));
        assert_eq!(scale_point(5, 5, (10, 10), (0, 0)), (0, 0, (0, 0)));
    }

    #[test]
    fn video_packets_have_one_byte_of_kind() {
        assert_eq!(encode(&Shown::Size(720, 1600)), [1, 0x02, 0xd0, 0x06, 0x40]);
        assert_eq!(encode(&Shown::Config(vec![9, 8].into())), [2, 9, 8]);
        assert_eq!(
            encode(&Shown::Frame { key: true, pts: 0x0102, data: vec![7].into() }),
            [3, 0, 0, 0, 0, 0, 0, 1, 2, 7]
        );
        assert_eq!(encode(&Shown::Frame { key: false, pts: 0, data: vec![].into() })[0], 4);
        assert_eq!(encode(&Shown::Ended), [5]);
    }

    #[tokio::test]
    async fn nothing_set_up_means_no_device_and_a_plain_reason() {
        let folder = tempfile::tempdir().unwrap();
        let manager = manager(folder.path());
        let status = manager.status();
        assert!(status.enabled && !status.setup.adb && !status.setup.emulator);
        assert!(status.devices.is_empty() && status.chats.is_empty());
        assert_eq!(status.setup.fetch, Fetch::Idle);
        let refused = manager.attach("chat", None).unwrap_err().to_string();
        assert!(refused.starts_with("Android is not set up"), "{refused}");
        assert!(manager.attach("chat", Some("o7_phone_9")).is_err());
        assert!(manager.attach("chat", Some("adb:R58M")).is_err());
        assert!(manager.fetch_start().is_err(), "nothing was offered, so nothing is fetched");
        assert!(manager.stop_device("adb:R58M").await.is_err());
        assert!(manager.status().chats.is_empty());
    }

    #[tokio::test]
    async fn turning_mobile_off_lets_go_of_everything_and_refuses_the_rest() {
        let folder = tempfile::tempdir().unwrap();
        let manager = manager(folder.path());
        hold_phone(&manager, "chat", "R58M");
        assert_eq!(manager.status().chats.len(), 1);
        manager.set_enabled(false).await;
        let status = manager.status();
        assert!(!status.enabled && status.chats.is_empty());
        assert_eq!(manager.attach("chat", None).unwrap_err().to_string(), TURNED_OFF);
        assert_eq!(manager.watch("chat").await.err().unwrap().to_string(), TURNED_OFF);
        assert_eq!(manager.fetch_start().unwrap_err().to_string(), TURNED_OFF);
        manager.set_enabled(true).await;
        assert!(manager.status().enabled);
    }

    #[tokio::test]
    async fn a_phone_is_the_persons_until_they_say_otherwise() {
        let folder = tempfile::tempdir().unwrap();
        let manager = manager(folder.path());
        hold_phone(&manager, "chat", "R58M");
        let held = &manager.status().chats[0];
        assert_eq!(held.kind, DeviceKind::Phone);
        assert_eq!(held.title, "Pixel 8");
        assert!(!held.agent_allowed);
        assert!(manager.agent_refused("chat"));
        assert_eq!(manager.live_for_agent("chat").err().unwrap().to_string(), NOT_ALLOWED);
        manager.allow_agent("chat", true);
        assert!(!manager.agent_refused("chat"));
        // Allowed now, but its screen is not open: a different, honest reason.
        assert_eq!(
            manager.live_for_agent("chat").err().unwrap().to_string(),
            "The device is not running."
        );
        manager.allow_agent("chat", false);
        assert!(manager.agent_refused("chat"));
        // Another chat cannot take a phone that is held.
        let taken = manager.attach("other", Some("adb:R58M")).unwrap_err().to_string();
        assert_eq!(taken, "Another chat is using that phone.");
        assert_eq!(manager.live_for_agent("other").err().unwrap().to_string(), NO_DEVICE);
        manager.detach("chat");
        assert!(manager.status().chats.is_empty());
    }

    #[tokio::test]
    async fn a_phone_that_is_unplugged_tells_its_chat() {
        let folder = tempfile::tempdir().unwrap();
        let manager = manager(folder.path());
        hold_phone(&manager, "chat", "R58M");
        manager.phones_changed(vec![Listed {
            serial: "R58M".into(),
            state: Connection::Offline,
            model: None,
        }]);
        let status = manager.status();
        assert_eq!(status.chats[0].error.as_deref(), Some("The phone was disconnected."));
        assert_eq!(status.devices[0].state, super::DeviceState::Offline);
        // Not authorized yet: it is listed, and cannot be taken.
        manager.detach("chat");
        manager.phones_changed(vec![Listed {
            serial: "NEW1".into(),
            state: Connection::Unauthorized,
            model: None,
        }]);
        assert_eq!(manager.status().devices[0].state, super::DeviceState::Unauthorized);
        assert!(matches!(manager.attach("chat", Some("adb:NEW1")), Err(Error::Unauthorized)));
    }

    #[tokio::test]
    async fn an_idle_chat_lets_go_of_its_device() {
        let folder = tempfile::tempdir().unwrap();
        let manager = manager(folder.path());
        hold_phone(&manager, "chat", "R58M");
        manager.stop_idle(Duration::from_secs(3600)).await;
        assert_eq!(manager.status().chats.len(), 1, "used a moment ago");
        manager.stop_idle(Duration::ZERO).await;
        assert!(manager.status().chats.is_empty());
    }

    #[test]
    fn names_and_sizes_read_plainly() {
        let phone = |serial: &str, model: Option<&str>| Listed {
            serial: serial.into(),
            state: Connection::Device,
            model: model.map(str::to_owned),
        };
        assert_eq!(phone_title(&phone("R58M", Some("Pixel_8_Pro"))), "Pixel 8 Pro");
        assert_eq!(phone_title(&phone("R58M", None)), "R58M");
        assert_eq!(phone_title(&phone("emulator-5554", Some("sdk_gphone64"))), "Emulator on port 5554");
        assert_eq!(megabytes(1_895_447_397), 1807.6);
        assert_eq!(megabytes(0), 0.0);
    }
}
