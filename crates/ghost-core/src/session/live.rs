//! Live-agent bookkeeping: the cap on live agents, and one live process per native session across
//! chats.
//!
//! A chat holds a slot from the moment its start begins (preparation included) until its driver
//! ended and the processes a force stop or a crash kills are gone. A start beyond the cap makes
//! room: the agent idle longest (by the ten-minute idle rule's own test, which its actor reports
//! here) is asked to stop, and the start waits for its slot, which passes to it straight away. An
//! agent asked while it no longer idles declines, and the next idle one stops instead (an owed
//! stop, paid by the next agent to turn idle when none is). A start is refused only when no agent
//! is idle. A native session is claimed for the resume id before spawning and again for the id the
//! agent returns at `Ready`, and stays claimed as long as the slot: two processes on one session
//! interleave one transcript (the Claude spike), so the second chat is refused.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use ghost_process::{EnvKey, LaunchSpec, ProcessCwd, ResolvedEnv};
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

/// What a live agent is doing, as its actor last reported it.
#[derive(Debug, Clone)]
pub(crate) enum Activity {
    /// Idle by the idle rule (ready, unwatched, nothing open, queued or running in the
    /// background) since `since`: it may be stopped to make room. `notify` asks it.
    Idle { since: Instant, notify: Arc<Notify> },
    /// Starting, stopping, running a turn or background work, or about to send a message.
    Working,
    /// Waiting for the person to answer a request or question.
    Waiting,
    /// Open in the agent's own terminal.
    Terminal,
    /// Ready, with a view of the chat open, since its last activity at `since`.
    Shown { since: Instant },
    /// Stopping, or stopped while its processes still exit.
    Stopping,
}

/// What a live agent does, for a person deciding whether to stop it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Doing {
    Working,
    Waiting,
    Terminal,
    Idle,
}

/// The live agents by what they are doing, for a start that found no room.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Occupancy {
    pub(crate) working: usize,
    pub(crate) waiting: usize,
    pub(crate) terminals: usize,
    pub(crate) shown: usize,
    pub(crate) stopping: usize,
}

/// Whether a start may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// It holds a slot.
    Admitted,
    /// Every slot is taken and an idle agent was asked to stop: its slot comes to this chat.
    Waiting,
    /// Every slot is taken and no agent is idle.
    Full(Occupancy),
    /// An update or a quit closed admission.
    Closed,
}

/// Native and ACP transports use the same provider's transcript store. The namespace also
/// includes its host and configuration directory, because another realm can reuse the same id.
/// All current launches are local; a future remote launcher must supply its host realm here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct NativeSessionScope {
    provider: String,
    host: String,
    config_dir: Option<PathBuf>,
}

impl NativeSessionScope {
    pub(crate) fn local(
        agent_id: &str,
        device_id: &str,
        env: &ResolvedEnv,
        launch: &LaunchSpec,
        folder: &Path,
    ) -> Self {
        // Every connection of one agent shares its transcript store.
        let provider = ghost_agents::agent_of(agent_id);
        let config = match provider {
            "claude" => Some(("CLAUDE_CONFIG_DIR", ".claude")),
            "codex" => Some(("CODEX_HOME", ".codex")),
            _ => None,
        };
        // Only configuration-path variables are consulted. Credential files and values are
        // neither inspected nor added to the claim key.
        let variable = |name: &str| -> Option<OsString> {
            let key = EnvKey::from(name);
            launch
                .set_env
                .iter()
                .rev()
                .find(|(name, _)| EnvKey::from(name.as_os_str()) == key)
                .map(|(_, value)| value.clone())
                .or_else(|| env.vars.get(&key).cloned())
                .filter(|value| !value.is_empty())
        };
        let config_dir = config.and_then(|(override_name, default)| {
            let home = if cfg!(windows) {
                variable("USERPROFILE").or_else(|| variable("HOME"))
            } else {
                variable("HOME")
            };
            variable(override_name)
                .map(PathBuf::from)
                .or_else(|| home.map(|home| PathBuf::from(home).join(default)))
                .map(|path| {
                    // ACP's Node launcher runs in its installation directory, but the native
                    // CLI it creates uses the session's chat folder.
                    let cwd = if matches!(agent_id, "claude-acp" | "codex-acp") {
                        folder
                    } else {
                        match &launch.cwd {
                            ProcessCwd::ChatFolder => folder,
                            ProcessCwd::Fixed(cwd) => cwd,
                        }
                    };
                    normalize_path(if path.is_absolute() {
                        path
                    } else {
                        cwd.join(path)
                    })
                })
        });
        Self {
            provider: provider.into(),
            host: device_id.into(),
            config_dir,
        }
    }
}

/// Resolve directory aliases using metadata only. A new configuration directory may not exist
/// yet, so resolve its nearest existing ancestor before normalizing the remaining components.
/// No provider configuration or credential file is read.
pub(super) fn normalize_path(path: PathBuf) -> PathBuf {
    let path = path
        .ancestors()
        .find_map(|ancestor| {
            std::fs::canonicalize(ancestor)
                .ok()
                .and_then(|root| path.strip_prefix(ancestor).ok().map(|tail| root.join(tail)))
        })
        .unwrap_or(path);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    #[cfg(windows)]
    {
        fold_windows_path(normalized)
    }
    #[cfg(not(windows))]
    {
        normalized
    }
}

/// Claim keys conservatively treat Windows paths as case insensitive, including a not-yet-created
/// suffix. Windows' invariant casing preserves UTF-16 paths without a lossy UTF-8 conversion.
#[cfg(windows)]
#[allow(unsafe_code)] // The OS casing API has no safe standard-library equivalent.
fn fold_windows_path(path: PathBuf) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Globalization::{
        LCMAP_UPPERCASE, LCMapStringEx, LOCALE_NAME_INVARIANT,
    };

    let source = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let Ok(length) = i32::try_from(source.len()) else {
        return path;
    };
    // SAFETY: the source is live for both calls, its length is explicit, and the first call only
    // queries the required output length. Optional NLS arguments are null.
    let required = unsafe {
        LCMapStringEx(
            LOCALE_NAME_INVARIANT,
            LCMAP_UPPERCASE,
            source.as_ptr(),
            length,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };
    if required <= 0 {
        let mut fallback = path.into_os_string();
        fallback.make_ascii_uppercase();
        return fallback.into();
    }
    let mut output = vec![0_u16; required as usize];
    // SAFETY: the output buffer has the length returned above, and neither buffer is moved or
    // mutated for the duration of the call.
    let written = unsafe {
        LCMapStringEx(
            LOCALE_NAME_INVARIANT,
            LCMAP_UPPERCASE,
            source.as_ptr(),
            length,
            output.as_mut_ptr(),
            required,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };
    if written <= 0 {
        return path;
    }
    output.truncate(written as usize);
    OsString::from_wide(&output).into()
}

/// The live agents of every chat.
#[derive(Debug)]
pub(crate) struct LiveAgents {
    max: usize,
    inner: Mutex<Inner>,
    /// Counts the times a slot was given back, for starts and stops that wait for one.
    released: watch::Sender<u64>,
}

#[derive(Debug, Default)]
struct Inner {
    quiescing: bool,
    maintenance: usize,
    owned_draining: bool,
    slots: HashSet<String>,
    folders: HashMap<String, Vec<PathBuf>>,
    mutations: HashSet<PathBuf>,
    folder_uses: HashMap<PathBuf, usize>,
    /// `(provider realm, native_session_id)` -> the chat holding it.
    sessions: HashMap<(NativeSessionScope, String), String>,
    /// What each live agent does, as its actor last reported (none yet: starting, so working).
    activity: HashMap<String, Activity>,
    /// Idle agents asked to stop so that a waiting start gets their slot.
    evicting: HashSet<String>,
    /// Starts waiting for a slot, first come first served, each with the folder it registers.
    /// Each has an agent stopping for it in `evicting`, or is owed one: the next agent to turn idle.
    waiting: VecDeque<(String, PathBuf)>,
    /// Folders a quiet change holds (a commit, a setup copy): agents and terminals stay, new
    /// turns wait, and only an exclusive change is kept out. Counted per folder.
    quiet: HashMap<PathBuf, usize>,
}

/// Whether two paths overlap: one is the other or inside it.
fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

impl Inner {
    /// The idle agent with the oldest activity that is not already stopping for someone.
    fn longest_idle(&self) -> Option<(String, Arc<Notify>)> {
        self.activity
            .iter()
            .filter_map(|(chat, activity)| match activity {
                Activity::Idle { since, notify }
                    if self.slots.contains(chat) && !self.evicting.contains(chat) =>
                {
                    Some((*since, chat, notify))
                }
                _ => None,
            })
            .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
            .map(|(_, chat, notify)| (chat.clone(), Arc::clone(notify)))
    }

    /// Asks idle agents to stop until every waiting start has one stopping for it.
    fn ask_idle_to_stop(&mut self) {
        while self.waiting.len() > self.evicting.len() {
            let Some((chat, notify)) = self.longest_idle() else {
                return;
            };
            self.evicting.insert(chat);
            notify.notify_one();
        }
    }

    fn occupancy(&self) -> Occupancy {
        let mut occupancy = Occupancy::default();
        for chat in &self.slots {
            match self.activity.get(chat) {
                _ if self.evicting.contains(chat) => occupancy.stopping += 1,
                Some(Activity::Waiting) => occupancy.waiting += 1,
                Some(Activity::Terminal) => occupancy.terminals += 1,
                Some(Activity::Shown { .. }) => occupancy.shown += 1,
                Some(Activity::Stopping) => occupancy.stopping += 1,
                Some(Activity::Idle { .. } | Activity::Working) | None => occupancy.working += 1,
            }
        }
        occupancy
    }

    /// Room goes to the waiting starts first, in the order they came.
    fn admit_waiting(&mut self, max: usize) -> bool {
        let mut admitted = false;
        while self.slots.len() < max {
            let Some((chat, folder)) = self.waiting.pop_front() else {
                break;
            };
            self.folders.insert(chat.clone(), vec![folder]);
            self.slots.insert(chat);
            admitted = true;
        }
        admitted
    }
}

pub(crate) struct MaintenanceGuard<'a>(&'a LiveAgents);
impl Drop for MaintenanceGuard<'_> {
    fn drop(&mut self) {
        self.0.end_maintenance();
    }
}

impl LiveAgents {
    pub(crate) fn new(max: usize) -> LiveAgents {
        LiveAgents {
            max,
            inner: Mutex::new(Inner::default()),
            released: watch::Sender::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Takes a slot for `chat` (idempotent) without making room; false when none is free. A slot
    /// taken this way has no folder until `set_folders`, so it overlaps every folder.
    #[cfg(test)]
    pub(crate) fn acquire(&self, chat: &str) -> bool {
        let mut inner = self.lock();
        if inner.quiescing || inner.owned_draining {
            return false;
        }
        if inner.slots.contains(chat) {
            return true;
        }
        if inner.slots.len() + inner.waiting.len() >= self.max {
            return false;
        }
        inner.slots.insert(chat.to_owned());
        true
    }

    /// A slot for `chat`, starting in `folder` (registered at once, so a start still preparing
    /// overlaps only its own folder; `set_folders` checks it again before anything spawns). At the
    /// cap, the agent idle longest is asked to stop and `chat` waits for its slot.
    pub(crate) fn admit(&self, chat: &str, folder: &Path) -> Admission {
        let mut inner = self.lock();
        if inner.quiescing || inner.owned_draining {
            return Admission::Closed;
        }
        if inner.slots.contains(chat) {
            return Admission::Admitted;
        }
        if inner.waiting.iter().any(|(waiting, _)| waiting == chat) {
            return Admission::Waiting;
        }
        // Each waiting start will take a slot, so they count as taken.
        if inner.slots.len() + inner.waiting.len() < self.max {
            inner.slots.insert(chat.to_owned());
            inner
                .folders
                .insert(chat.to_owned(), vec![folder.to_path_buf()]);
            return Admission::Admitted;
        }
        // An agent already stopping for a start that gave up can serve this one.
        if inner.evicting.len() > inner.waiting.len() || inner.longest_idle().is_some() {
            inner
                .waiting
                .push_back((chat.to_owned(), folder.to_path_buf()));
            inner.ask_idle_to_stop();
            return Admission::Waiting;
        }
        Admission::Full(inner.occupancy())
    }

    /// Whether `chat` waits for a slot.
    pub(crate) fn is_waiting(&self, chat: &str) -> bool {
        self.lock()
            .waiting
            .iter()
            .any(|(waiting, _)| waiting == chat)
    }

    /// `chat` no longer waits for a slot (its start was stopped or gave up).
    pub(crate) fn cancel_wait(&self, chat: &str) {
        let mut inner = self.lock();
        let before = inner.waiting.len();
        inner.waiting.retain(|(waiting, _)| waiting != chat);
        let changed = inner.waiting.len() != before;
        drop(inner);
        if changed {
            self.released
                .send_modify(|count| *count = count.wrapping_add(1));
        }
    }

    /// Records what `chat`'s agent does now. True when it is idle and owed: a start waits with
    /// no agent stopping for it, so this one stops now.
    pub(crate) fn report(&self, chat: &str, activity: Activity) -> bool {
        let mut inner = self.lock();
        if !inner.slots.contains(chat) {
            inner.activity.remove(chat);
            return false;
        }
        let idle = matches!(activity, Activity::Idle { .. });
        inner.activity.insert(chat.to_owned(), activity);
        if idle && !inner.evicting.contains(chat) && inner.waiting.len() > inner.evicting.len() {
            inner.evicting.insert(chat.to_owned());
            return true;
        }
        false
    }

    /// `chat` was asked to stop for a waiting start but no longer idles: another idle agent is
    /// asked instead, or the next one to turn idle.
    pub(crate) fn decline_stop(&self, chat: &str) {
        let mut inner = self.lock();
        if inner.evicting.remove(chat) {
            inner.ask_idle_to_stop();
        }
    }

    /// Every chat that holds a slot or waits for one.
    pub(crate) fn live_chats(&self) -> Vec<String> {
        let inner = self.lock();
        let mut chats: Vec<String> = inner
            .slots
            .iter()
            .cloned()
            .chain(inner.waiting.iter().map(|(chat, _)| chat.clone()))
            .collect();
        chats.sort();
        chats.dedup();
        chats
    }

    /// The chats whose agent works in one of `folders` or inside one (or holds a slot with no
    /// folder known yet), and the starts waiting to work there.
    pub(crate) fn holders(&self, folders: &[PathBuf]) -> Vec<String> {
        let inner = self.lock();
        let overlaps = |held: &PathBuf| {
            folders
                .iter()
                .any(|folder| folder.starts_with(held) || held.starts_with(folder))
        };
        let mut chats: Vec<String> = inner
            .slots
            .iter()
            .filter(|chat| {
                inner
                    .folders
                    .get(*chat)
                    .is_none_or(|held| held.iter().any(overlaps))
            })
            .cloned()
            .chain(
                inner
                    .waiting
                    .iter()
                    .filter(|(_, folder)| overlaps(folder))
                    .map(|(chat, _)| chat.clone()),
            )
            .collect();
        chats.sort();
        chats.dedup();
        chats
    }

    /// Counts slot releases, for a wait that must see a slot really free.
    pub(crate) fn released(&self) -> watch::Receiver<u64> {
        self.released.subscribe()
    }

    /// Atomically excludes new starts and folder mutations while the caller prepares an update.
    pub(crate) fn try_quiesce(&self) -> bool {
        let mut inner = self.lock();
        if inner.quiescing
            || inner.owned_draining
            || inner.maintenance > 0
            || !inner.slots.is_empty()
            || !inner.waiting.is_empty()
            || !inner.mutations.is_empty()
            || !inner.quiet.is_empty()
            || !inner.folder_uses.is_empty()
        {
            return false;
        }
        inner.quiescing = true;
        true
    }

    /// Whether an update or a quit closed admission.
    pub(crate) fn quiescing_now(&self) -> bool {
        let inner = self.lock();
        inner.quiescing || inner.owned_draining
    }

    pub(crate) fn resume_admission(&self) {
        self.lock().quiescing = false;
    }

    pub(crate) fn try_maintenance(&self) -> Option<MaintenanceGuard<'_>> {
        self.begin_maintenance().then(|| MaintenanceGuard(self))
    }

    pub(crate) fn begin_maintenance(&self) -> bool {
        let mut inner = self.lock();
        if inner.quiescing || inner.owned_draining {
            return false;
        }
        let Some(count) = inner.maintenance.checked_add(1) else {
            return false;
        };
        inner.maintenance = count;
        true
    }

    pub(crate) fn end_maintenance(&self) {
        self.lock().maintenance -= 1;
    }

    pub(crate) fn begin_owned_drain(&self) -> bool {
        let mut inner = self.lock();
        if inner.owned_draining || inner.quiescing {
            return false;
        }
        inner.owned_draining = true;
        true
    }

    pub(crate) fn end_owned_drain(&self) {
        self.lock().owned_draining = false;
    }

    pub(crate) fn owned_work_count(&self) -> usize {
        let inner = self.lock();
        inner
            .maintenance
            .saturating_add(inner.mutations.len())
            .saturating_add(inner.quiet.values().sum::<usize>())
    }

    pub(crate) fn project_terminal_count(&self) -> usize {
        self.lock().folder_uses.values().sum()
    }

    /// Gives `chat`'s slot back, and every native session it held, once nothing of it runs; ends
    /// its wait for a slot. A start waiting for room gets the slot at once.
    pub(crate) fn release(&self, chat: &str) {
        let mut inner = self.lock();
        let held = inner.slots.remove(chat);
        inner.folders.remove(chat);
        inner.sessions.retain(|_, holder| holder != chat);
        inner.activity.remove(chat);
        inner.evicting.remove(chat);
        let before = inner.waiting.len();
        inner.waiting.retain(|(waiting, _)| waiting != chat);
        let waited = inner.waiting.len() != before;
        let admitted = inner.admit_waiting(self.max);
        // A start that still waits has an agent stopping for it, or the next idle one.
        inner.ask_idle_to_stop();
        drop(inner);
        if held || waited || admitted {
            self.released
                .send_modify(|count| *count = count.wrapping_add(1));
        }
    }

    /// Claims a native session for `chat`; false when another chat holds it.
    pub(crate) fn claim(
        &self,
        scope: &NativeSessionScope,
        native_session_id: &str,
        chat: &str,
    ) -> bool {
        let mut inner = self.lock();
        // UUID spelling (case, hyphens) must not create another claim for one native transcript.
        // Non-UUID IDs remain opaque for providers with a different identifier format.
        let identity = uuid::Uuid::parse_str(native_session_id)
            .map(|id| id.to_string())
            .unwrap_or_else(|_| native_session_id.to_owned());
        let key = (scope.clone(), identity);
        match inner.sessions.get(&key) {
            Some(holder) if holder != chat => false,
            _ => {
                inner.sessions.insert(key, chat.to_owned());
                true
            }
        }
    }

    pub(crate) fn count(&self) -> usize {
        self.lock().slots.len()
    }

    pub(crate) fn contains(&self, chat: &str) -> bool {
        self.lock().slots.contains(chat)
    }

    /// Canonical folder registration is atomic with revert reservations, before native spawn.
    #[cfg(test)]
    pub(crate) fn set_folder(&self, chat: &str, folder: PathBuf) -> bool {
        self.set_folders(chat, vec![folder])
    }
    pub(crate) fn set_folders(&self, chat: &str, folders: Vec<PathBuf>) -> bool {
        let mut inner = self.lock();
        if folders.is_empty()
            || folders.iter().any(|folder| {
                inner
                    .mutations
                    .iter()
                    .any(|held| folder.starts_with(held) || held.starts_with(folder))
            })
        {
            return false;
        }
        inner.folders.insert(chat.into(), folders);
        true
    }

    pub(crate) fn reserve_folder(&self, folder: &Path) -> bool {
        self.reserve_folders(&[folder.to_owned()])
    }

    /// One mutation may own nested paths. Check every path against prior owners before
    /// recording any of them, so paths in this operation cannot reject one another.
    pub(crate) fn reserve_folders(&self, folders: &[PathBuf]) -> bool {
        let mut inner = self.lock();
        if inner.quiescing
            || inner.owned_draining
            || folders.iter().any(|folder| {
                inner
                    .folder_uses
                    .keys()
                    .any(|held| folder.starts_with(held) || held.starts_with(folder))
                    || inner
                        .mutations
                        .iter()
                        .any(|held| folder.starts_with(held) || held.starts_with(folder))
                    || inner.quiet.keys().any(|held| overlap(folder, held))
                    || inner.slots.iter().any(|chat| {
                        inner.folders.get(chat).is_none_or(|held| {
                            held.iter()
                                .any(|held| folder.starts_with(held) || held.starts_with(folder))
                        })
                    })
            })
        {
            return false;
        }
        inner.mutations.extend(folders.iter().cloned());
        true
    }

    pub(crate) fn release_folder(&self, folder: &Path) {
        self.lock().mutations.remove(folder);
    }

    /// A quiet hold of `folders`: agents and terminals there stay; only an exclusive change
    /// (and an update or a quit) is kept out. False when one holds an overlapping folder.
    pub(crate) fn hold_quietly(&self, folders: &[PathBuf]) -> bool {
        let mut inner = self.lock();
        if inner.quiescing
            || inner.owned_draining
            || folders
                .iter()
                .any(|folder| inner.mutations.iter().any(|held| overlap(folder, held)))
        {
            return false;
        }
        for folder in folders {
            *inner.quiet.entry(folder.clone()).or_default() += 1;
        }
        true
    }

    pub(crate) fn release_quiet(&self, folders: &[PathBuf]) {
        let mut inner = self.lock();
        for folder in folders {
            if let Some(count) = inner.quiet.get_mut(folder) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    inner.quiet.remove(folder);
                }
            }
        }
    }

    /// Whether `chat` makes its folder busy for a new chat: working, waiting for an answer, open
    /// in a terminal, waiting for a slot, or active within `settle` (just started or just
    /// finished a turn, so likely to be given the next message). An agent idle longer, or
    /// stopping, edits nothing.
    pub(crate) fn busy(&self, chat: &str, settle: std::time::Duration) -> bool {
        let inner = self.lock();
        if inner.waiting.iter().any(|(waiting, _)| waiting == chat) {
            return true;
        }
        if !inner.slots.contains(chat) || inner.evicting.contains(chat) {
            return false;
        }
        match inner.activity.get(chat) {
            Some(Activity::Idle { since, .. } | Activity::Shown { since }) => {
                since.elapsed() < settle
            }
            Some(Activity::Stopping) => false,
            Some(Activity::Working | Activity::Waiting | Activity::Terminal) | None => true,
        }
    }

    /// What `chat`'s agent does, if it holds a slot or waits for one.
    pub(crate) fn doing(&self, chat: &str) -> Option<Doing> {
        let inner = self.lock();
        if inner.waiting.iter().any(|(waiting, _)| waiting == chat) {
            return Some(Doing::Working);
        }
        if !inner.slots.contains(chat) {
            return None;
        }
        Some(match inner.activity.get(chat) {
            Some(Activity::Idle { .. } | Activity::Shown { .. }) => Doing::Idle,
            Some(Activity::Waiting) => Doing::Waiting,
            Some(Activity::Terminal) => Doing::Terminal,
            Some(Activity::Working | Activity::Stopping) | None => Doing::Working,
        })
    }

    /// How many project terminals (and Runs) are open in `folders` or inside one.
    pub(crate) fn terminals_in(&self, folders: &[PathBuf]) -> usize {
        let inner = self.lock();
        inner
            .folder_uses
            .iter()
            .filter(|(held, _)| folders.iter().any(|folder| overlap(folder, held)))
            .map(|(_, count)| *count)
            .sum()
    }

    /// Whether a change to an overlapping folder is in progress.
    pub(crate) fn mutation_in(&self, folders: &[PathBuf]) -> bool {
        let inner = self.lock();
        folders.iter().any(|folder| {
            inner.mutations.iter().any(|held| overlap(folder, held))
                || inner.quiet.keys().any(|held| overlap(folder, held))
        })
    }

    /// A terminal can share a folder with agents, but cannot race destructive folder operations.
    pub(crate) fn begin_folder_use(&self, folder: PathBuf) -> bool {
        let mut inner = self.lock();
        if inner.quiescing
            || inner.owned_draining
            || inner
                .mutations
                .iter()
                .any(|held| folder.starts_with(held) || held.starts_with(&folder))
        {
            return false;
        }
        let count = inner.folder_uses.entry(folder).or_default();
        let Some(next) = count.checked_add(1) else {
            return false;
        };
        *count = next;
        true
    }

    pub(crate) fn end_folder_use(&self, folder: &Path) {
        let mut inner = self.lock();
        if let Some(count) = inner.folder_uses.get_mut(folder) {
            *count -= 1;
            if *count == 0 {
                inner.folder_uses.remove(folder);
            }
        }
    }

    pub(crate) fn max(&self) -> usize {
        self.max
    }
}

#[cfg(test)]
mod tests {
    use ghost_process::{EnvMap, EnvSource};
    use std::time::Duration;

    use super::*;

    #[test]
    fn failed_batch_admission_does_not_reserve_an_earlier_disjoint_path() {
        let live = LiveAgents::new(12);
        let free = PathBuf::from("/fixture/free");
        let busy = PathBuf::from("/fixture/busy");
        assert!(live.begin_folder_use(busy.clone()));
        assert!(!live.reserve_folders(&[free.clone(), busy.clone()]));
        assert_eq!(live.owned_work_count(), 0);
        assert!(live.begin_folder_use(free.clone()));
        live.end_folder_use(&free);
        live.end_folder_use(&busy);
        assert!(live.reserve_folders(&[free.clone(), free.join("child")]));
        assert!(!live.begin_folder_use(free.join("child/deeper")));
        assert!(!live.reserve_folder(&free));
        live.release_folder(&free);
        assert!(
            !live.reserve_folder(&free),
            "the requested child remains held"
        );
        live.release_folder(&free.join("child"));
        assert!(live.reserve_folder(&free));
        live.release_folder(&free);
    }

    #[test]
    fn project_terminal_shares_agents_but_excludes_overlapping_mutations_and_idle_barriers() {
        let live = LiveAgents::new(12);
        let folder = PathBuf::from("/fixture/project");
        assert!(live.acquire("agent"));
        assert!(live.set_folder("agent", folder.clone()));
        assert!(live.begin_folder_use(folder.clone()));
        assert!(live.begin_folder_use(folder.join("child")));
        assert_eq!(live.project_terminal_count(), 2);
        assert_eq!(live.owned_work_count(), 0);
        assert!(live.acquire("second"));
        assert!(live.set_folder("second", folder.clone()));
        live.release("agent");
        live.release("second");
        assert!(!live.reserve_folder(&folder));
        assert!(!live.reserve_folder(Path::new("/fixture")));
        assert!(!live.reserve_folder(&folder.join("child/deeper")));
        assert!(!live.try_quiesce());
        live.end_folder_use(&folder);
        assert!(!live.reserve_folder(&folder));
        live.end_folder_use(&folder.join("child"));
        assert!(live.reserve_folder(&folder));
        assert!(!live.begin_folder_use(folder.join("child")));
        live.release_folder(&folder);
        assert!(live.try_quiesce());
        assert!(!live.begin_folder_use(folder.clone()));
        live.resume_admission();
        assert!(live.begin_owned_drain());
        assert!(!live.begin_folder_use(folder));
    }

    #[test]
    fn update_quiesce_excludes_agent_starts_and_git_mutations_atomically() {
        let live = LiveAgents::new(12);
        let folder = Path::new("/fixture");
        assert!(live.acquire("one"));
        assert!(!live.try_quiesce());
        live.release("one");
        assert!(live.reserve_folder(folder));
        assert!(!live.try_quiesce());
        live.release_folder(folder);
        assert!(live.try_quiesce());
        assert!(!live.try_quiesce());
        assert!(!live.begin_owned_drain());
        assert!(!live.acquire("two"));
        assert!(!live.reserve_folder(folder));
        live.resume_admission();
        assert!(live.acquire("two"));
    }

    #[test]
    fn racing_start_and_update_cannot_both_be_admitted() {
        for _ in 0..32 {
            let live = LiveAgents::new(12);
            let barrier = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                let start = scope.spawn(|| {
                    barrier.wait();
                    live.acquire("racing")
                });
                barrier.wait();
                let update = live.try_quiesce();
                assert_ne!(start.join().unwrap(), update);
            });
        }
    }

    #[test]
    fn vendor_maintenance_blocks_application_update_until_its_owner_finishes() {
        let live = LiveAgents::new(12);
        let first = live.try_maintenance().unwrap();
        let second = live.try_maintenance().unwrap();
        assert!(!live.try_quiesce());
        drop(first);
        assert!(!live.try_quiesce());
        drop(second);
        assert!(live.try_quiesce());
        assert!(live.try_maintenance().is_none());
        live.resume_admission();
        assert!(live.try_maintenance().is_some());
    }

    #[test]
    fn owned_terminal_maintenance_stays_counted_during_quit_and_preserves_session_cap() {
        let live = LiveAgents::new(1);
        assert!(live.begin_maintenance());
        assert!(live.acquire("one"));
        assert!(!live.acquire("two"));
        assert_eq!(live.owned_work_count(), 1);
        assert!(live.begin_owned_drain());
        assert!(!live.begin_maintenance());
        assert!(live.try_maintenance().is_none());
        assert_eq!(live.owned_work_count(), 1);
        live.end_maintenance();
        assert_eq!(live.owned_work_count(), 0);
        live.end_owned_drain();
        live.release("one");
        assert!(live.try_quiesce());
    }

    #[test]
    fn quit_drain_waits_existing_owners_and_blocks_new_work_until_released() {
        let live = LiveAgents::new(4);
        let maintenance = live.try_maintenance().unwrap();
        let folder = Path::new("owned-folder");
        assert!(live.reserve_folder(folder));
        assert!(live.begin_owned_drain());
        let drain = super::super::OwnedWorkDrain { live: &live };
        assert!(!live.begin_owned_drain());
        assert_eq!(live.owned_work_count(), 2);
        assert!(live.try_maintenance().is_none());
        assert!(!live.reserve_folder(Path::new("new-folder")));
        assert!(!live.acquire("new-chat"));
        assert!(!live.try_quiesce());
        drop(maintenance);
        assert_eq!(live.owned_work_count(), 1);
        live.release_folder(folder);
        assert_eq!(live.owned_work_count(), 0);
        drop(drain);
        assert!(live.try_maintenance().is_some());
        assert!(live.acquire("new-chat"));
    }

    #[test]
    fn folder_mutation_reservation_excludes_live_agents_and_late_starts() {
        let live = LiveAgents::new(4);
        let folder = PathBuf::from("/target");
        assert!(live.acquire("starting"));
        assert!(
            !live.reserve_folder(&folder),
            "an unresolved startup may target this folder"
        );
        assert!(live.set_folder("starting", PathBuf::from("/other")));
        assert!(
            live.reserve_folder(&folder),
            "an unrelated running folder is allowed"
        );
        assert!(live.acquire("later"));
        assert!(
            !live.set_folder("later", folder.clone()),
            "a later start cannot spawn in a reserved folder"
        );
        live.release("later");
        live.release_folder(&folder);
        assert!(live.acquire("target"));
        assert!(live.set_folder("target", folder.clone()));
        assert!(!live.reserve_folder(&folder));
        live.release("target");
        assert!(live.reserve_folder(&folder));
    }

    fn scope(agent: &str, host: &str, vars: &[(&str, &str)]) -> NativeSessionScope {
        let env = ResolvedEnv {
            vars: vars
                .iter()
                .map(|(name, value)| (EnvKey::from(*name), (*value).into()))
                .collect::<EnvMap>(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        };
        NativeSessionScope::local(agent, host, &env, &launch(), Path::new("/work"))
    }

    fn launch() -> LaunchSpec {
        LaunchSpec {
            label: "test".into(),
            program: "agent".into(),
            args: Vec::new(),
            cwd: ProcessCwd::ChatFolder,
            set_env: Vec::new(),
            daemons: Vec::new(),
            startup_timeout: Duration::from_secs(60),
        }
    }

    fn idle(notify: &Arc<Notify>, age: Duration) -> Activity {
        Activity::Idle {
            since: Instant::now() - age,
            notify: Arc::clone(notify),
        }
    }

    fn asked(notify: &Arc<Notify>) -> bool {
        futures::FutureExt::now_or_never(notify.notified()).is_some()
    }

    #[test]
    fn a_start_at_the_cap_asks_the_longest_idle_agent_and_takes_its_slot() {
        let live = LiveAgents::new(2);
        let (old, new) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        let folder = PathBuf::from("/fixture/old");
        assert_eq!(live.admit("old", &folder), Admission::Admitted);
        assert_eq!(
            live.admit("new", Path::new("/fixture/new")),
            Admission::Admitted
        );
        // A start still preparing holds only its own folder, not every folder.
        assert!(live.reserve_folder(Path::new("/fixture/elsewhere")));
        live.release_folder(Path::new("/fixture/elsewhere"));
        assert!(!live.report("old", idle(&old, Duration::from_secs(60))));
        assert!(!live.report("new", idle(&new, Duration::from_secs(1))));
        let released = live.released();
        assert_eq!(
            live.admit("third", Path::new("/fixture/third")),
            Admission::Waiting
        );
        assert!(asked(&old), "the agent idle longest is asked");
        assert!(!asked(&new));
        assert!(live.is_waiting("third") && !live.contains("third"));
        assert_eq!(
            live.admit("third", Path::new("/fixture/third")),
            Admission::Waiting,
            "asking again changes nothing"
        );
        // Its slot passes to the waiting start once its processes are gone.
        live.release("old");
        assert!(live.contains("third") && !live.is_waiting("third"));
        assert!(released.has_changed().unwrap());
        assert_eq!(live.holders(&[PathBuf::from("/fixture/third")]), ["third"]);
        assert_eq!(live.count(), 2);
    }

    #[test]
    fn a_declined_stop_passes_on_and_an_owed_one_is_paid_by_the_next_idle_agent() {
        let live = LiveAgents::new(2);
        let (one, two) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        for chat in ["one", "two"] {
            assert_eq!(live.admit(chat, Path::new("/fixture")), Admission::Admitted);
        }
        assert!(!live.report("one", idle(&one, Duration::from_secs(60))));
        assert!(!live.report("two", idle(&two, Duration::from_secs(1))));
        assert_eq!(
            live.admit("start", Path::new("/fixture")),
            Admission::Waiting
        );
        assert!(asked(&one));
        // It began a turn meanwhile: the other idle agent is asked instead.
        assert!(!live.report("one", Activity::Working));
        live.decline_stop("one");
        assert!(asked(&two));
        // That one too: nobody idles, so the stop is owed...
        assert!(!live.report("two", Activity::Waiting));
        live.decline_stop("two");
        assert!(live.is_waiting("start"));
        // ...and paid by the next agent to turn idle.
        assert!(
            live.report("one", idle(&one, Duration::ZERO)),
            "it stops at once"
        );
        assert!(
            !live.report("two", idle(&two, Duration::ZERO)),
            "one stop per waiting start"
        );
        live.release("one");
        assert!(live.contains("start"));
    }

    #[test]
    fn a_full_table_says_what_its_agents_do_and_a_cancelled_wait_gives_its_place_up() {
        let live = LiveAgents::new(3);
        let notify = Arc::new(Notify::new());
        for chat in ["work", "ask", "term"] {
            assert_eq!(live.admit(chat, Path::new("/fixture")), Admission::Admitted);
        }
        live.report("work", Activity::Working);
        live.report("ask", Activity::Waiting);
        live.report("term", Activity::Terminal);
        assert_eq!(
            live.admit("new", Path::new("/fixture")),
            Admission::Full(Occupancy {
                working: 1,
                waiting: 1,
                terminals: 1,
                shown: 0,
                stopping: 0,
            })
        );
        assert!(!live.is_waiting("new"), "a refused start does not wait");
        live.report("work", idle(&notify, Duration::ZERO));
        assert_eq!(
            live.admit("first", Path::new("/fixture")),
            Admission::Waiting
        );
        // A start that gives up leaves the agent stopping for it to the next start.
        live.cancel_wait("first");
        assert_eq!(
            live.admit("second", Path::new("/fixture")),
            Admission::Waiting
        );
        live.release("work");
        assert!(live.contains("second") && !live.contains("first"));
        // Waiting starts keep an update out until they have their slots.
        let live = LiveAgents::new(1);
        let notify = Arc::new(Notify::new());
        assert_eq!(live.admit("a", Path::new("/fixture")), Admission::Admitted);
        live.report("a", idle(&notify, Duration::ZERO));
        assert_eq!(live.admit("b", Path::new("/fixture")), Admission::Waiting);
        assert!(!live.try_quiesce());
    }

    #[test]
    fn a_quiet_hold_keeps_agents_and_terminals_and_keeps_out_only_exclusive_changes() {
        let live = LiveAgents::new(4);
        let folder = PathBuf::from("/fixture/project");
        let notify = Arc::new(Notify::new());
        assert_eq!(live.admit("agent", &folder), Admission::Admitted);
        live.report("agent", idle(&notify, Duration::ZERO));
        assert!(live.begin_folder_use(folder.clone()));
        // An agent and a terminal in the folder do not stand in a quiet change's way.
        assert!(live.hold_quietly(std::slice::from_ref(&folder)));
        assert!(
            live.hold_quietly(&[folder.join("child")]),
            "quiet changes share folders"
        );
        assert!(live.contains("agent"), "nothing was stopped");
        assert_eq!(live.owned_work_count(), 2, "a quit waits for them");
        assert!(!live.try_quiesce());
        assert!(live.mutation_in(std::slice::from_ref(&folder)));
        live.release_quiet(&[folder.join("child")]);
        live.release_quiet(std::slice::from_ref(&folder));
        assert_eq!(live.owned_work_count(), 0);
        // A quiet change and an exclusive one keep each other out, nested folders included.
        let other = PathBuf::from("/fixture/other");
        assert!(live.hold_quietly(&[other.join("inner")]));
        assert!(!live.reserve_folder(&other));
        live.release_quiet(&[other.join("inner")]);
        assert!(live.reserve_folder(&other));
        assert!(!live.hold_quietly(&[other.join("inner")]));
        live.release_folder(&other);
        // Busy for a new chat: working or recently active only.
        assert!(!live.busy("agent", Duration::ZERO));
        assert!(live.busy("agent", Duration::from_secs(60)));
        live.report("agent", Activity::Working);
        assert!(live.busy("agent", Duration::ZERO));
        assert_eq!(live.doing("agent"), Some(Doing::Working));
        assert_eq!(live.terminals_in(&[PathBuf::from("/fixture")]), 1);
        live.end_folder_use(&folder);
    }

    #[test]
    fn slots_are_capped_and_idempotent() {
        let live = LiveAgents::new(2);
        assert!(live.acquire("a"));
        assert!(live.acquire("a"), "the same chat again");
        assert!(live.acquire("b"));
        assert!(!live.acquire("c"), "nothing is evicted to make room");
        assert_eq!(live.count(), 2);
        live.release("a");
        assert!(live.acquire("c"));
    }

    #[test]
    fn a_native_session_belongs_to_one_chat() {
        let live = LiveAgents::new(12);
        let claude = scope("claude-acp", "local", &[]);
        assert!(live.claim(&claude, "s1", "a"));
        assert!(live.claim(&claude, "s1", "a"), "its own claim");
        assert!(!live.claim(&claude, "s1", "b"));
        assert!(
            live.claim(&scope("codex-acp", "local", &[]), "s1", "b"),
            "another agent's id space"
        );
        live.release("a");
        assert!(live.claim(&claude, "s1", "b"));
    }

    #[test]
    fn uuid_spelling_does_not_create_another_native_session() {
        let live = LiveAgents::new(12);
        let claude = scope("claude", "local", &[]);
        assert!(live.claim(&claude, "019abcde-0000-7000-8000-000000000001", "one"));
        assert!(!live.claim(&claude, "019ABCDE000070008000000000000001", "two"));
        assert!(live.claim(&claude, "s1", "one"));
        assert!(
            live.claim(&claude, "S1", "two"),
            "other identifiers stay opaque"
        );
    }

    #[test]
    fn native_and_acp_aliases_share_claims_in_both_directions() {
        for (native, acp) in [
            ("claude", "claude-acp"),
            ("codex", "codex-acp"),
            ("opencode", "opencode-v2"),
            ("antigravity", "agy-stream"),
            ("grok", "grok-stream"),
        ] {
            for (first, second) in [(native, acp), (acp, native)] {
                let live = LiveAgents::new(12);
                assert!(live.claim(&scope(first, "local", &[]), "s1", "a"));
                assert!(!live.claim(&scope(second, "local", &[]), "s1", "b"));
                live.release("a");
                assert!(live.claim(&scope(second, "local", &[]), "s1", "b"));
            }
        }
    }

    #[test]
    fn host_and_configuration_realms_keep_separate_native_id_spaces() {
        for (provider, variable) in [("claude", "CLAUDE_CONFIG_DIR"), ("codex", "CODEX_HOME")] {
            let live = LiveAgents::new(12);
            assert!(live.claim(
                &scope(provider, "local", &[(variable, "/config/one")]),
                "s1",
                "a"
            ));
            assert!(live.claim(
                &scope(provider, "remote", &[(variable, "/config/one")]),
                "s1",
                "b"
            ));
            assert!(live.claim(
                &scope(provider, "local", &[(variable, "/config/two")]),
                "s1",
                "c"
            ));
            assert!(!live.claim(
                &scope(provider, "local", &[(variable, "/config/./one")]),
                "s1",
                "d"
            ));
        }
    }

    #[test]
    fn launch_overrides_and_default_home_resolve_the_configuration_realm() {
        let env = ResolvedEnv {
            vars: [
                (EnvKey::from("HOME"), "/users/me".into()),
                (EnvKey::from("USERPROFILE"), "/users/me".into()),
            ]
            .into(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        };
        let mut launch = launch();
        let default =
            NativeSessionScope::local("claude", "local", &env, &launch, Path::new("/work"));
        launch
            .set_env
            .push(("CLAUDE_CONFIG_DIR".into(), "/users/me/.claude".into()));
        assert_eq!(
            default,
            NativeSessionScope::local("claude-acp", "local", &env, &launch, Path::new("/work"))
        );
        launch
            .set_env
            .push(("CLAUDE_CONFIG_DIR".into(), "/separate".into()));
        assert_ne!(
            default,
            NativeSessionScope::local("claude", "local", &env, &launch, Path::new("/work"))
        );
    }

    #[test]
    fn relative_configuration_roots_use_the_native_clis_folder_for_acp() {
        let env = ResolvedEnv {
            vars: [(EnvKey::from("CLAUDE_CONFIG_DIR"), "config".into())].into(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        };
        let mut launch = launch();
        let native =
            NativeSessionScope::local("claude", "local", &env, &launch, Path::new("/work"));
        launch.cwd = ProcessCwd::Fixed("/adapter".into());
        assert_eq!(
            native,
            NativeSessionScope::local("claude-acp", "local", &env, &launch, Path::new("/work"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_aliases_cannot_claim_the_same_native_session_twice() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("Répo");
        std::fs::create_dir(&folder).unwrap();
        let env = ResolvedEnv {
            vars: [(EnvKey::from("CLAUDE_CONFIG_DIR"), "NewConfig".into())].into(),
            source: EnvSource::Fallback {
                reason: "test".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        };
        let first = NativeSessionScope::local("claude", "local", &env, &launch(), &folder);
        let alias = PathBuf::from(folder.to_str().unwrap().to_uppercase());
        let second = NativeSessionScope::local("claude-acp", "local", &env, &launch(), &alias);
        let live = LiveAgents::new(12);
        assert!(live.claim(&first, "session", "one"));
        assert!(!live.claim(&second, "session", "two"));
        assert_eq!(
            normalize_path(folder.join("NEWCONFIG")),
            first.config_dir.unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_symlinks_share_a_claim_but_distinct_case_sensitive_paths_do_not() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        let alias = temp.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        assert_eq!(
            normalize_path(real.join("new")),
            normalize_path(alias.join("new"))
        );
        assert_ne!(
            normalize_path(real.join("New")),
            normalize_path(real.join("new"))
        );
    }
}
