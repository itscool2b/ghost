//! Small, profile-local setup preferences. No agent or credential data belongs here.
//! The profile owner lock serializes processes; this store serializes commands within that owner.
//! Settings' General page keeps its defaults for new chats and the native shell here too.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ghost_protocol::Autonomy;
use ghost_protocol::project_tools::ProjectEditor;
use serde::{Deserialize, Serialize};

use crate::fsutil::{quarantine_file, write_json_atomic};

const VERSION: u32 = 1;
const MAX_BYTES: u64 = 16 * 1024;
/// How many tour stops a profile may record as seen, and how long a stop's name may be.
const MAX_TOUR_STOPS: usize = 64;
const MAX_TOUR_STOP_NAME: usize = 40;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SetupStep {
    #[default]
    Welcome,
    Agents,
    Motion,
    Project,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MotionPreference {
    /// Full motion still honors the operating system's reduced-motion setting.
    #[default]
    Full,
    Reduced,
    Off,
}

/// Appearance belongs to the active profile on this device, just like motion preferences.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AppearancePreference {
    #[default]
    System,
    Light,
    Dark,
}

/// How large the chat's text sets (owner, 4 November 2026: fit more text in, as Codex does).
/// Compact is the default; Comfortable keeps the earlier, larger size. A file written before this
/// setting existed, or naming a size this build lacks, reads as Compact ([`fill_defaults`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TextSizePreference {
    #[default]
    Compact,
    Comfortable,
}

impl TextSizePreference {
    /// Every size's saved name, as serde writes it.
    pub const NAMES: [&'static str; 2] = ["compact", "comfortable"];
}

/// The desktop's theme: Main, Ghost's own neutral palette, or one per supported agent (owner,
/// 3 November 2026). It belongs to the profile on this device, like appearance, and works in both
/// appearances. Main is the default (owner, 25 November 2026; Claude Code's was from 4 November):
/// a profile that never chose one, or a file from a newer build that names a theme this build
/// lacks, reads as it ([`fill_defaults`]). Main is saved as `aura`, the name this palette had
/// before it was Main, so files written by earlier builds still read. A saved choice is kept,
/// except the one move [`meet_main`] makes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    Aura,
    Claude,
    Codex,
    Grok,
    Pi,
    Antigravity,
    Cursor,
    Opencode,
    OhMyPi,
    Hermes,
}

impl ThemePreference {
    /// Every theme's saved name, as serde writes it.
    pub const NAMES: [&'static str; 10] = [
        "aura",
        "claude",
        "codex",
        "grok",
        "pi",
        "antigravity",
        "cursor",
        "opencode",
        "oh_my_pi",
        "hermes",
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SetupPreferences {
    pub revision: u32,
    pub step: SetupStep,
    pub motion: MotionPreference,
    pub appearance: AppearancePreference,
    pub theme: ThemePreference,
    pub text_size: TextSizePreference,
    pub completed: bool,
    pub first_project_id: Option<String>,
    /// The tour's stops this profile has been shown or has declined, by name (owner,
    /// 22 November 2026: every feature with a UI joins the tour). A stop that is not listed is
    /// new to this person, so an update that adds stops can offer a short flight of only those.
    /// Empty for a profile that never met the tour; a file from before it reads as empty.
    pub tour_seen: Vec<String>,
    /// Files written before these settings existed read their defaults ([`fill_defaults`]).
    pub general: GeneralPreferences,
}

/// Settings' General page: what new chats start with, and how Ghost behaves while agents work
/// in the background. Full auto stays the default; an agent's own restrictions still apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct GeneralPreferences {
    /// The agent New chat selects first. `None`: the person picks each time.
    pub default_agent: Option<String>,
    /// The model new chats with `default_agent` start on. `None`: the agent's own default.
    pub default_model: Option<String>,
    /// The thinking level new chats with `default_agent` start on, chosen for `default_model`.
    /// `None`: the model's own default.
    pub default_effort: Option<String>,
    pub default_autonomy: Autonomy,
    /// Save an Aura file snapshot before every queued prompt. Off unless explicitly enabled.
    pub automatic_checkpoints: bool,
    /// Notify when a background chat finishes, fails or needs you.
    pub notifications: bool,
    /// Play a short sound when a chat finishes, fails or needs you. Off unless the person turns
    /// it on.
    pub sounds: bool,
    /// Keep the computer from idle sleep while an agent works.
    pub keep_awake: bool,
    /// Closing the window keeps Ghost running in the tray or menu bar.
    pub close_to_tray: bool,
    /// Copy each source folder's Git-ignored instruction files (a local `AGENTS.md` or
    /// `CLAUDE.md`) into the worktrees new chats get, so agents there read them too. On unless the
    /// person turns it off in Instructions.
    pub copy_instructions_to_worktrees: bool,
    /// Agents edit project memory; each edit shows a notice with Undo. Off: agents read it and
    /// cannot change it. On unless the person turns it off in Project memory (the owner reversed
    /// the earlier review-only decision on 1 November 2026, and dropped proposals on 10 November).
    pub agent_memory_writes: bool,
    /// A finished turn shows its work (interim messages, tools, thinking) instead of folding it
    /// under "Worked for" above the answer. Off unless the person turns it on in Settings > Chats.
    pub keep_steps_open: bool,
    /// The editor a chat's files open in, at their line. `None`, or an editor that is no longer
    /// installed: the first one detected.
    pub editor: Option<ProjectEditor>,
    /// The Browser universal plugin: the browser beside the chat, Annotate, and the browser tools
    /// agents get. On unless the person turns it off in Settings > Plugins & MCP (owner, 9
    /// November 2026: "on by default", and off "disables the browser on the UI").
    pub browser: bool,
    /// The browser offers to save sign-ins and fills them in (WebView2's own password manager, on
    /// Windows). On by default.
    pub browser_passwords: bool,
    /// The Mobile universal plugin: the Device pane beside the chat and the device tools agents
    /// get. On unless the person turns it off in Settings > Plugins & MCP; off stops every device
    /// Ghost started (owner, 21 November 2026, docs/superpowers/plans/2026-11-21-mobile.md).
    pub mobile: bool,
    /// The Game universal plugin: the Game pane beside the chat and, in chats whose folder holds
    /// a game, the game tools agents get. On unless the person turns it off in Settings >
    /// Plugins & MCP; off stops every game Ghost started (owner, 27 November 2026, docs/game.md).
    pub game: bool,
    /// Look for a newer Ghost shortly after start and a few times a day, and show Update when
    /// there is one (owner, 18 November 2026: "an update button pops up for every merge"). It
    /// reads one public file from the project's GitHub releases; downloading and installing still
    /// wait for the button. On unless the person turns it off in Settings > General.
    pub automatic_updates: bool,
    /// Coordinated projects, an experiment (owner, 19 November 2026): a project gets a coordinator
    /// chat that plans and delegates to worker chats, with shared Context files, a task board and
    /// routines. Off until the person turns it on in Settings > Experiments; off again stops the
    /// project tools at once and keeps what the projects hold.
    pub coordinated_projects: bool,
    /// The voice answers are read aloud in (owner, 22 November 2026): one of the voices bundled
    /// with the app, by the id the desktop's speech module gives it. `None`, or a voice a build
    /// no longer has: the voice model's own.
    pub speech_voice: Option<String>,
}

impl Default for GeneralPreferences {
    fn default() -> Self {
        Self {
            default_agent: None,
            default_model: None,
            default_effort: None,
            default_autonomy: Autonomy::FullAuto,
            automatic_checkpoints: false,
            notifications: true,
            sounds: false,
            keep_awake: true,
            close_to_tray: true,
            copy_instructions_to_worktrees: true,
            agent_memory_writes: true,
            keep_steps_open: false,
            editor: None,
            browser: true,
            browser_passwords: true,
            mobile: true,
            game: true,
            automatic_updates: true,
            coordinated_projects: false,
            speech_voice: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SetupSnapshot {
    pub preferences: SetupPreferences,
    /// Stable for this process, even when React remounts or the webview reloads.
    pub cold_start: bool,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SetupAction {
    Visit {
        step: SetupStep,
    },
    Motion {
        value: MotionPreference,
    },
    Appearance {
        value: AppearancePreference,
    },
    Theme {
        value: ThemePreference,
    },
    TextSize {
        value: TextSizePreference,
    },
    Project {
        project_id: String,
    },
    Finish,
    Restart,
    /// Records tour stops as seen. Names already listed are kept once; the list only grows.
    TourSeen {
        stops: Vec<String>,
    },
    /// Changing the agent clears the default model and thinking level, which belong to the
    /// previous agent.
    DefaultAgent {
        agent_id: Option<String>,
    },
    /// Changing the model clears the default thinking level, which was chosen for the old one.
    DefaultModel {
        model: Option<String>,
    },
    DefaultEffort {
        effort: Option<String>,
    },
    DefaultAutonomy {
        value: Autonomy,
    },
    AutomaticCheckpoints {
        enabled: bool,
    },
    Notifications {
        enabled: bool,
    },
    Sounds {
        enabled: bool,
    },
    Editor {
        editor: Option<ProjectEditor>,
    },
    KeepAwake {
        enabled: bool,
    },
    CloseToTray {
        enabled: bool,
    },
    /// Turns on or off copying Git-ignored instruction files into new worktree chats.
    CopyInstructionsToWorktrees {
        enabled: bool,
    },
    /// Turns on or off agents' edits to project memory. Off leaves it read-only for them.
    AgentMemoryWrites {
        enabled: bool,
    },
    /// Turns on or off showing a finished turn's work instead of folding it.
    KeepStepsOpen {
        enabled: bool,
    },
    /// Turns the Browser universal plugin on or off.
    Browser {
        enabled: bool,
    },
    /// Turns on or off the browser's offer to save sign-ins.
    BrowserPasswords {
        enabled: bool,
    },
    /// Turns the Mobile universal plugin on or off.
    Mobile {
        enabled: bool,
    },
    /// Turns the Game universal plugin on or off.
    Game {
        enabled: bool,
    },
    /// Turns on or off looking for a newer Ghost by itself.
    AutomaticUpdates {
        enabled: bool,
    },
    /// Turns the coordinated projects experiment on or off.
    CoordinatedProjects {
        enabled: bool,
    },
    /// Chooses the voice answers are read aloud in; `None` returns to the model's own.
    SpeechVoice {
        voice: Option<String>,
    },
}

/// The default look's number once a file has met Main.
const MAIN_LOOK: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    version: u32,
    boot_seen: bool,
    /// Which default look this file has met: 0 before Main, [`MAIN_LOOK`] since. It is the
    /// file's own, not a preference, so the interface never sees it. A build from before Main
    /// ignores it.
    #[serde(default)]
    look: u32,
    preferences: SetupPreferences,
}

/// Moves a file from before Main onto it, once. Until Main, Claude Code's theme was the default
/// and every save wrote it out, so a saved `claude` cannot tell a choice from a default: such a
/// profile takes the new default. A profile on any other theme chose it, and keeps it. Choosing
/// Claude Code's theme afterwards saves it beside the raised number, so it stays.
fn meet_main(mut record: Record) -> Record {
    if record.look < MAIN_LOOK {
        if record.preferences.theme == ThemePreference::Claude {
            record.preferences.theme = ThemePreference::Aura;
        }
        record.look = MAIN_LOOK;
    }
    record
}

impl Default for Record {
    fn default() -> Self {
        Self {
            version: VERSION,
            boot_seen: false,
            look: MAIN_LOOK,
            preferences: SetupPreferences {
                revision: 0,
                step: SetupStep::Welcome,
                motion: MotionPreference::Full,
                appearance: AppearancePreference::System,
                theme: ThemePreference::default(),
                text_size: TextSizePreference::default(),
                completed: false,
                first_project_id: None,
                tour_seen: Vec::new(),
                general: GeneralPreferences::default(),
            },
        }
    }
}

/// What a workspace opened for the first time takes from the one being left: how Ghost looks
/// and behaves, and how far setup got. The first project stays behind, since projects belong to
/// their workspace.

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("Setup changed in another view. Reload it and try again.")]
    Stale,
    #[error("{0}")]
    Invalid(&'static str),
    #[error("Setup preferences were saved by a newer Ghost. They have been left unchanged.")]
    NewerVersion,
    #[error("Could not save setup preferences: {0}")]
    Storage(#[from] crate::CoreError),
}

pub struct OnboardingStore {
    path: PathBuf,
    record: Mutex<Record>,
    cold_start: bool,
    notice: Option<String>,
}

impl OnboardingStore {
    /// Call only while holding this profile's owner lock. Missing files begin at Welcome.
    pub fn open(profile_dir: &Path) -> Result<Self, SetupError> {
        let path = profile_dir.join("onboarding.json");
        let mut notice = None;
        let record = match read(&path)? {
            None => Record::default(),
            Some(value) => {
                // Check the version before deserializing fields so future layouts stay intact.
                if value
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|version| version > u64::from(VERSION))
                {
                    return Err(SetupError::NewerVersion);
                }
                match serde_json::from_value::<Record>(fill_defaults(value)) {
                    Ok(record) if record.version == VERSION => meet_main(record),
                    _ => {
                        let moved = quarantine_file(&path)?;
                        notice = Some(format!(
                            "Setup preferences could not be read. The previous file is preserved at {}. Your chats are unchanged.",
                            moved.display()
                        ));
                        Record::default()
                    }
                }
            }
        };
        Ok(Self {
            path,
            cold_start: !record.boot_seen,
            record: Mutex::new(record),
            notice,
        })
    }

    fn save(&self, record: &Record) -> Result<(), SetupError> {
        write_json_atomic(&self.path, record)?;
        Ok(())
    }

    /// The saved preferences, for the native shell and new chats. Unlike [`Self::snapshot`],
    /// reading never marks the boot seen.
    pub fn preferences(&self) -> SetupPreferences {
        self.record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .preferences
            .clone()
    }

    /// Marks the boot seen only when the interface asks for setup, never in headless runs.
    pub fn snapshot(&self) -> Result<SetupSnapshot, SetupError> {
        let mut record = self
            .record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !record.boot_seen {
            let mut next = record.clone();
            next.boot_seen = true;
            self.save(&next)?;
            *record = next;
        }
        Ok(self.view(&record))
    }

    pub fn apply(&self, revision: u32, action: SetupAction) -> Result<SetupSnapshot, SetupError> {
        let mut record = self
            .record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if revision != record.preferences.revision {
            return Err(SetupError::Stale);
        }
        let mut next = record.clone();
        match action {
            SetupAction::Visit { step } => next.preferences.step = step,
            SetupAction::Motion { value } => next.preferences.motion = value,
            SetupAction::Appearance { value } => next.preferences.appearance = value,
            SetupAction::Theme { value } => next.preferences.theme = value,
            SetupAction::TextSize { value } => next.preferences.text_size = value,
            SetupAction::Project { project_id } => {
                if project_id.is_empty()
                    || project_id.len() > 128
                    || project_id.chars().any(char::is_control)
                {
                    return Err(SetupError::Invalid("The project identifier is invalid."));
                }
                next.preferences.first_project_id = Some(project_id);
            }
            SetupAction::Finish => next.preferences.completed = true,
            SetupAction::Restart => {
                next.preferences.step = SetupStep::Welcome;
                next.preferences.completed = false;
            }
            SetupAction::TourSeen { stops } => {
                if stops.len() > MAX_TOUR_STOPS || !stops.iter().all(|stop| tour_stop(stop)) {
                    return Err(SetupError::Invalid("The tour stop is invalid."));
                }
                let seen = &mut next.preferences.tour_seen;
                for stop in stops {
                    if !seen.contains(&stop) {
                        seen.push(stop);
                    }
                }
                if seen.len() > MAX_TOUR_STOPS {
                    return Err(SetupError::Invalid("Too many tour stops are recorded."));
                }
            }
            SetupAction::DefaultAgent { agent_id } => {
                if let Some(agent) = &agent_id {
                    identifier(agent, 128, "The agent identifier is invalid.")?;
                }
                let agent_id = agent_id.map(|agent| ghost_agents::agent_of(&agent).to_owned());
                let general = &mut next.preferences.general;
                if general.default_agent != agent_id {
                    general.default_model = None;
                    general.default_effort = None;
                }
                general.default_agent = agent_id;
            }
            SetupAction::DefaultModel { model } => {
                if let Some(model) = &model {
                    identifier(model, 256, "The model identifier is invalid.")?;
                    if next.preferences.general.default_agent.is_none() {
                        return Err(SetupError::Invalid(
                            "Choose a default agent before its model.",
                        ));
                    }
                }
                let general = &mut next.preferences.general;
                if general.default_model != model {
                    general.default_effort = None;
                }
                general.default_model = model;
            }
            SetupAction::DefaultEffort { effort } => {
                if let Some(effort) = &effort {
                    identifier(effort, 128, "The thinking level is invalid.")?;
                    if next.preferences.general.default_agent.is_none() {
                        return Err(SetupError::Invalid(
                            "Choose a default agent before its thinking level.",
                        ));
                    }
                }
                next.preferences.general.default_effort = effort;
            }
            SetupAction::DefaultAutonomy { value } => {
                next.preferences.general.default_autonomy = value;
            }
            SetupAction::AutomaticCheckpoints { enabled } => {
                next.preferences.general.automatic_checkpoints = enabled;
            }
            SetupAction::Notifications { enabled } => {
                next.preferences.general.notifications = enabled;
            }
            SetupAction::Sounds { enabled } => next.preferences.general.sounds = enabled,
            SetupAction::Editor { editor } => next.preferences.general.editor = editor,
            SetupAction::KeepAwake { enabled } => next.preferences.general.keep_awake = enabled,
            SetupAction::CloseToTray { enabled } => {
                next.preferences.general.close_to_tray = enabled;
            }
            SetupAction::CopyInstructionsToWorktrees { enabled } => {
                next.preferences.general.copy_instructions_to_worktrees = enabled;
            }
            SetupAction::AgentMemoryWrites { enabled } => {
                next.preferences.general.agent_memory_writes = enabled;
            }
            SetupAction::KeepStepsOpen { enabled } => {
                next.preferences.general.keep_steps_open = enabled;
            }
            SetupAction::Browser { enabled } => next.preferences.general.browser = enabled,
            SetupAction::BrowserPasswords { enabled } => {
                next.preferences.general.browser_passwords = enabled;
            }
            SetupAction::Mobile { enabled } => next.preferences.general.mobile = enabled,
            SetupAction::Game { enabled } => next.preferences.general.game = enabled,
            SetupAction::AutomaticUpdates { enabled } => {
                next.preferences.general.automatic_updates = enabled;
            }
            SetupAction::CoordinatedProjects { enabled } => {
                next.preferences.general.coordinated_projects = enabled;
            }
            SetupAction::SpeechVoice { voice } => {
                if let Some(voice) = &voice {
                    identifier(voice, 64, "The voice is invalid.")?;
                }
                next.preferences.general.speech_voice = voice;
            }
        }
        next.preferences.revision = revision
            .checked_add(1)
            .ok_or(SetupError::Invalid("Setup revision limit reached."))?;
        self.save(&next)?;
        *record = next;
        Ok(self.view(&record))
    }

    fn view(&self, record: &Record) -> SetupSnapshot {
        SetupSnapshot {
            preferences: record.preferences.clone(),
            cold_start: self.cold_start,
            notice: self.notice.clone(),
        }
    }
}

/// Supplies every appearance and General preference a saved file lacks (files from before those settings
/// existed, or from before one was added) with its default, so the file still reads. The types
/// keep every field required, which keeps the TypeScript bindings exact.
fn fill_defaults(mut value: serde_json::Value) -> serde_json::Value {
    let Some(preferences) = value
        .get_mut("preferences")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return value;
    };
    preferences
        .entry("appearance")
        .or_insert_with(|| serde_json::Value::String("system".into()));
    // Setup once had a sign-in step between the agents and the first project. A person who
    // stopped there continues at the project instead of losing the whole file to one unknown name.
    if let Some(step) = preferences.get_mut("step")
        && step.as_str() == Some("account")
    {
        *step = serde_json::Value::String("project".into());
    }
    // No theme yet, or one a newer build added: the default, Main.
    let default_theme = || serde_json::to_value(ThemePreference::default()).unwrap_or_default();
    let theme = preferences.entry("theme").or_insert_with(default_theme);
    if !theme
        .as_str()
        .is_some_and(|name| ThemePreference::NAMES.contains(&name))
    {
        *theme = default_theme();
    }
    // No text size yet, or one a newer build added: Compact.
    let text_size = preferences
        .entry("text_size")
        .or_insert_with(|| serde_json::Value::String("compact".into()));
    if !text_size
        .as_str()
        .is_some_and(|name| TextSizePreference::NAMES.contains(&name))
    {
        *text_size = serde_json::Value::String("compact".into());
    }
    // No tour yet: nothing seen. A list this build cannot read (too long, or holding anything
    // but stop names) reads as empty too, so the tour is offered again instead of the whole
    // file being set aside.
    let tour = preferences
        .entry("tour_seen")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let readable = tour.as_array().is_some_and(|stops| {
        stops.len() <= MAX_TOUR_STOPS
            && stops
                .iter()
                .all(|stop| stop.as_str().is_some_and(tour_stop))
    });
    if !readable {
        *tour = serde_json::Value::Array(Vec::new());
    }
    let Ok(serde_json::Value::Object(defaults)) =
        serde_json::to_value(GeneralPreferences::default())
    else {
        return value;
    };
    let general = preferences
        .entry("general")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(general) = general.as_object_mut() {
        for (key, default) in defaults {
            general.entry(key).or_insert(default);
        }
        // An editor a newer build added reads as none, the first detected, instead of failing
        // the whole file.
        if let Some(editor) = general.get_mut("editor")
            && serde_json::from_value::<Option<ProjectEditor>>(editor.clone()).is_err()
        {
            *editor = serde_json::Value::Null;
        }
        // A default saved as a connection (`claude-acp`) names its agent.
        if let Some(serde_json::Value::String(agent)) = general.get_mut("default_agent") {
            let canonical = ghost_agents::agent_of(agent).to_owned();
            *agent = canonical;
        }
    }
    value
}

/// A tour stop's name: lowercase letters, digits and dashes, as the interface names its stops.
fn tour_stop(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOUR_STOP_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Agent and model identifiers are opaque, bounded and single-line.
fn identifier(value: &str, limit: usize, message: &'static str) -> Result<(), SetupError> {
    if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err(SetupError::Invalid(message));
    }
    Ok(())
}

fn read(path: &Path) -> Result<Option<serde_json::Value>, SetupError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => return Ok(Some(serde_json::Value::Null)),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(crate::CoreError::Io {
                path: path.into(),
                source,
            }
            .into());
        }
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(crate::CoreError::Io {
                path: path.into(),
                source,
            }
            .into());
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(crate::CoreError::io(path))?;
    // Invalid and oversized files take the same preserving recovery path in open().
    if bytes.len() as u64 > MAX_BYTES {
        return Ok(Some(serde_json::Value::Null));
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    Ok(Some(
        serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_checkpoints_are_opt_in_for_legacy_and_separate_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let mut legacy = serde_json::to_value(Record::default()).unwrap();
        legacy["preferences"]["general"]
            .as_object_mut()
            .unwrap()
            .remove("automatic_checkpoints");
        std::fs::write(
            dir.path().join("onboarding.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(!store.preferences().general.automatic_checkpoints);
        store
            .apply(0, SetupAction::AutomaticCheckpoints { enabled: true })
            .unwrap();
        assert!(
            OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .general
                .automatic_checkpoints
        );
        let separate = tempfile::tempdir().unwrap();
        assert!(
            !OnboardingStore::open(separate.path())
                .unwrap()
                .preferences()
                .general
                .automatic_checkpoints
        );
        store
            .apply(1, SetupAction::AutomaticCheckpoints { enabled: false })
            .unwrap();
        assert!(
            !OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .general
                .automatic_checkpoints
        );
    }

    #[test]
    fn worktree_instruction_copies_are_on_for_legacy_profiles_and_can_be_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut legacy = serde_json::to_value(Record::default()).unwrap();
        legacy["preferences"]["general"]
            .as_object_mut()
            .unwrap()
            .remove("copy_instructions_to_worktrees");
        std::fs::write(
            dir.path().join("onboarding.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(store.preferences().general.copy_instructions_to_worktrees);
        store
            .apply(
                0,
                SetupAction::CopyInstructionsToWorktrees { enabled: false },
            )
            .unwrap();
        assert!(
            !OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .general
                .copy_instructions_to_worktrees
        );
    }

    /// The Browser plugin is on by default (owner, 9 November 2026), including for profiles saved
    /// before it existed, and its switches stay as set.
    #[test]
    fn the_browser_is_on_for_legacy_profiles_and_can_be_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut legacy = serde_json::to_value(Record::default()).unwrap();
        let general = legacy["preferences"]["general"].as_object_mut().unwrap();
        general.remove("browser");
        general.remove("browser_passwords");
        std::fs::write(
            dir.path().join("onboarding.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(store.snapshot().unwrap().notice.is_none());
        assert!(store.preferences().general.browser);
        assert!(store.preferences().general.browser_passwords);
        store
            .apply(0, SetupAction::Browser { enabled: false })
            .unwrap();
        store
            .apply(1, SetupAction::BrowserPasswords { enabled: false })
            .unwrap();
        let reopened = OnboardingStore::open(dir.path()).unwrap();
        assert!(!reopened.preferences().general.browser);
        assert!(!reopened.preferences().general.browser_passwords);
        assert_eq!(
            reopened.preferences().general,
            GeneralPreferences {
                browser: false,
                browser_passwords: false,
                ..GeneralPreferences::default()
            }
        );
    }

    /// Mobile is on by default, including for profiles saved before it existed, and stays off
    /// once turned off.
    #[test]
    fn mobile_is_on_for_legacy_profiles_and_can_be_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut legacy = serde_json::to_value(Record::default()).unwrap();
        legacy["preferences"]["general"]
            .as_object_mut()
            .unwrap()
            .remove("mobile");
        std::fs::write(
            dir.path().join("onboarding.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(store.snapshot().unwrap().notice.is_none());
        assert!(store.preferences().general.mobile);
        store
            .apply(0, SetupAction::Mobile { enabled: false })
            .unwrap();
        let reopened = OnboardingStore::open(dir.path()).unwrap();
        assert_eq!(
            reopened.preferences().general,
            GeneralPreferences {
                mobile: false,
                ..GeneralPreferences::default()
            }
        );
    }

    /// Game is on by default, including for profiles saved before it existed, and stays off
    /// once turned off.
    #[test]
    fn game_is_on_for_legacy_profiles_and_can_be_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut legacy = serde_json::to_value(Record::default()).unwrap();
        legacy["preferences"]["general"]
            .as_object_mut()
            .unwrap()
            .remove("game");
        std::fs::write(
            dir.path().join("onboarding.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(store.snapshot().unwrap().notice.is_none());
        assert!(store.preferences().general.game);
        store
            .apply(0, SetupAction::Game { enabled: false })
            .unwrap();
        let reopened = OnboardingStore::open(dir.path()).unwrap();
        assert_eq!(
            reopened.preferences().general,
            GeneralPreferences {
                game: false,
                ..GeneralPreferences::default()
            }
        );
    }

    #[test]
    fn agent_memory_writes_are_on_for_legacy_profiles_and_can_be_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut legacy = serde_json::to_value(Record::default()).unwrap();
        legacy["preferences"]["general"]
            .as_object_mut()
            .unwrap()
            .remove("agent_memory_writes");
        std::fs::write(
            dir.path().join("onboarding.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(store.snapshot().unwrap().notice.is_none());
        assert!(store.preferences().general.agent_memory_writes);
        store
            .apply(0, SetupAction::AgentMemoryWrites { enabled: false })
            .unwrap();
        let reopened = OnboardingStore::open(dir.path()).unwrap();
        assert!(!reopened.preferences().general.agent_memory_writes);
        // Turning it off keeps every other preference.
        assert_eq!(
            reopened.preferences().general,
            GeneralPreferences {
                agent_memory_writes: false,
                ..GeneralPreferences::default()
            }
        );
        reopened
            .apply(1, SetupAction::AgentMemoryWrites { enabled: true })
            .unwrap();
        assert!(
            OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .general
                .agent_memory_writes
        );
    }

    #[test]
    fn a_profile_on_the_earlier_default_moves_to_main_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onboarding.json");
        // Saved before Main: Claude Code's theme was the default and every save wrote it out.
        std::fs::write(
            &path,
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":2,"step":"project","motion":"full","appearance":"dark","theme":"claude","completed":true,"first_project_id":null}}"#,
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert_eq!(
            store.preferences().theme,
            ThemePreference::Aura,
            "moved to Main"
        );
        assert_eq!(
            store.preferences().appearance,
            AppearancePreference::Dark,
            "kept"
        );
        // Choosing Claude Code's theme again is a choice, and it stays.
        store
            .apply(
                2,
                SetupAction::Theme {
                    value: ThemePreference::Claude,
                },
            )
            .unwrap();
        assert_eq!(
            OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .theme,
            ThemePreference::Claude,
            "moved once only"
        );
        // A theme someone picked before Main is theirs to keep.
        std::fs::write(
            &path,
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":2,"step":"project","motion":"full","appearance":"dark","theme":"hermes","completed":true,"first_project_id":null}}"#,
        )
        .unwrap();
        assert_eq!(
            OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .theme,
            ThemePreference::Hermes
        );
        // A new profile starts on Main and has nothing to move.
        assert_eq!(Record::default().look, MAIN_LOOK);
    }

    #[test]
    fn themes_default_to_main_keep_a_saved_choice_and_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":2,"step":"project","motion":"full","appearance":"dark","completed":true,"first_project_id":null}}"#,
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert_eq!(
            store.preferences().theme,
            ThemePreference::Aura,
            "a profile that never chose a theme reads as Main"
        );
        let next = store
            .apply(
                2,
                SetupAction::Theme {
                    value: ThemePreference::Codex,
                },
            )
            .unwrap();
        assert_eq!(next.preferences.theme, ThemePreference::Codex);
        assert_eq!(
            next.preferences.appearance,
            AppearancePreference::Dark,
            "kept"
        );
        assert_eq!(
            OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .theme,
            ThemePreference::Codex,
            "a saved choice is kept"
        );
        assert_eq!(Record::default().preferences.theme, ThemePreference::Aura);
        // Every name round-trips as serde writes it.
        for name in ThemePreference::NAMES {
            let value: ThemePreference = serde_json::from_value(serde_json::json!(name)).unwrap();
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::json!(name)
            );
        }
        // A theme from a newer build reads as the default instead of failing the whole file.
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":5,"step":"project","motion":"full","appearance":"light","theme":"future_agent","completed":true,"first_project_id":null}}"#,
        )
        .unwrap();
        let reopened = OnboardingStore::open(dir.path()).unwrap();
        let snapshot = reopened.snapshot().unwrap();
        assert!(snapshot.notice.is_none(), "{:?}", snapshot.notice);
        assert_eq!(snapshot.preferences.theme, ThemePreference::Aura);
        assert_eq!(snapshot.preferences.appearance, AppearancePreference::Light);
        // So does an editor from a newer build; a known one is kept.
        for (saved, expected) in [
            (r#""future_editor""#, None),
            (r#""cursor""#, Some(ProjectEditor::Cursor)),
        ] {
            std::fs::write(
                dir.path().join("onboarding.json"),
                format!(
                    r#"{{"version":1,"boot_seen":true,"preferences":{{"revision":5,"step":"project","motion":"full","appearance":"light","completed":true,"first_project_id":null,"general":{{"editor":{saved}}}}}}}"#
                ),
            )
            .unwrap();
            let snapshot = OnboardingStore::open(dir.path())
                .unwrap()
                .snapshot()
                .unwrap();
            assert!(snapshot.notice.is_none(), "{:?}", snapshot.notice);
            assert_eq!(snapshot.preferences.general.editor, expected);
        }
    }

    #[test]
    fn text_size_defaults_to_compact_changes_and_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":2,"step":"project","motion":"full","appearance":"dark","theme":"cursor","completed":true,"first_project_id":null}}"#,
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert_eq!(
            store.preferences().text_size,
            TextSizePreference::Compact,
            "files from before the setting read as Compact"
        );
        let next = store
            .apply(
                2,
                SetupAction::TextSize {
                    value: TextSizePreference::Comfortable,
                },
            )
            .unwrap();
        assert_eq!(next.preferences.text_size, TextSizePreference::Comfortable);
        assert_eq!(next.preferences.theme, ThemePreference::Cursor, "kept");
        assert_eq!(
            OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .text_size,
            TextSizePreference::Comfortable
        );
        for name in TextSizePreference::NAMES {
            let value: TextSizePreference =
                serde_json::from_value(serde_json::json!(name)).unwrap();
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::json!(name)
            );
        }
        // A size from a newer build reads as Compact instead of failing the whole file.
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":5,"step":"project","motion":"full","appearance":"light","text_size":"huge","completed":true,"first_project_id":null}}"#,
        )
        .unwrap();
        let snapshot = OnboardingStore::open(dir.path())
            .unwrap()
            .snapshot()
            .unwrap();
        assert!(snapshot.notice.is_none(), "{:?}", snapshot.notice);
        assert_eq!(snapshot.preferences.text_size, TextSizePreference::Compact);
    }

    #[test]
    fn appearance_defaults_migrate_and_changes_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":3,"step":"project","motion":"reduced","completed":true,"first_project_id":"existing"}}"#,
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        let migrated = store.snapshot().unwrap();
        assert!(migrated.notice.is_none());
        assert_eq!(
            migrated.preferences.appearance,
            AppearancePreference::System
        );
        assert_eq!(migrated.preferences.revision, 3);
        assert_eq!(migrated.preferences.motion, MotionPreference::Reduced);
        for (revision, value) in [
            (3, AppearancePreference::Light),
            (4, AppearancePreference::Dark),
            (5, AppearancePreference::System),
        ] {
            let next = store
                .apply(revision, SetupAction::Appearance { value })
                .unwrap();
            assert_eq!(next.preferences.revision, revision + 1);
            assert_eq!(next.preferences.appearance, value);
            assert_eq!(
                next.preferences.first_project_id.as_deref(),
                Some("existing")
            );
            assert!(matches!(
                store.apply(revision, SetupAction::Appearance { value }),
                Err(SetupError::Stale)
            ));
            assert_eq!(
                OnboardingStore::open(dir.path())
                    .unwrap()
                    .preferences()
                    .appearance,
                value
            );
        }
        let separate = tempfile::tempdir().unwrap();
        assert_eq!(
            OnboardingStore::open(separate.path())
                .unwrap()
                .preferences()
                .appearance,
            AppearancePreference::System
        );
    }

    /// The tour's seen stops: empty for a profile from before the tour, growing only by valid
    /// names, kept once, bounded, and kept across Restart and reopening.
    #[test]
    fn tour_stops_seen_are_recorded_once_bounded_and_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":3,"step":"project","motion":"full","completed":true,"first_project_id":null}}"#,
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        let snapshot = store.snapshot().unwrap();
        assert!(snapshot.notice.is_none(), "an older file is not damaged");
        assert!(snapshot.preferences.tour_seen.is_empty());
        let seen = |stops: &[&str]| SetupAction::TourSeen {
            stops: stops.iter().map(|stop| (*stop).to_owned()).collect(),
        };
        let next = store.apply(3, seen(&["projects", "new-chat"])).unwrap();
        assert_eq!(next.preferences.tour_seen, ["projects", "new-chat"]);
        let again = store.apply(4, seen(&["new-chat", "browser"])).unwrap();
        assert_eq!(
            again.preferences.tour_seen,
            ["projects", "new-chat", "browser"],
            "a stop is listed once"
        );
        // Refusals leave the list and the revision alone.
        let too_long = "x".repeat(MAX_TOUR_STOP_NAME + 1);
        for bad in [
            "",
            "New-Chat",
            "a b",
            "a/b",
            "../x",
            "line\nbreak",
            too_long.as_str(),
        ] {
            assert!(
                matches!(store.apply(5, seen(&[bad])), Err(SetupError::Invalid(_))),
                "{bad:?}"
            );
        }
        let many: Vec<String> = (0..=MAX_TOUR_STOPS).map(|at| format!("s{at}")).collect();
        assert!(matches!(
            store.apply(5, SetupAction::TourSeen { stops: many }),
            Err(SetupError::Invalid(_))
        ));
        // Names that are each fine still cannot grow the list past its bound.
        let most: Vec<String> = (0..MAX_TOUR_STOPS - 2).map(|at| format!("s{at}")).collect();
        assert!(matches!(
            store.apply(5, SetupAction::TourSeen { stops: most }),
            Err(SetupError::Invalid(_))
        ));
        assert_eq!(store.preferences().revision, 5);
        // Running setup again does not bring the tour back by itself.
        let restarted = store.apply(5, SetupAction::Restart).unwrap();
        assert_eq!(restarted.preferences.tour_seen.len(), 3);
        assert_eq!(
            OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .tour_seen,
            ["projects", "new-chat", "browser"]
        );
        // A list this build cannot read is empty, and the rest of the file is kept.
        for saved in [r#""yes""#, r#"["ok","Not Ok"]"#, r#"[1,2]"#] {
            std::fs::write(
                dir.path().join("onboarding.json"),
                format!(
                    r#"{{"version":1,"boot_seen":true,"preferences":{{"revision":9,"step":"project","motion":"reduced","completed":true,"first_project_id":null,"tour_seen":{saved}}}}}"#
                ),
            )
            .unwrap();
            let snapshot = OnboardingStore::open(dir.path())
                .unwrap()
                .snapshot()
                .unwrap();
            assert!(snapshot.notice.is_none(), "{saved}");
            assert!(snapshot.preferences.tour_seen.is_empty(), "{saved}");
            assert_eq!(snapshot.preferences.motion, MotionPreference::Reduced);
        }
    }

    #[test]
    fn progress_resumes_and_boot_is_stable_across_reads() {
        let dir = tempfile::tempdir().unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(store.snapshot().unwrap().cold_start);
        assert!(store.snapshot().unwrap().cold_start);
        store
            .apply(
                0,
                SetupAction::Visit {
                    step: SetupStep::Motion,
                },
            )
            .unwrap();
        store
            .apply(
                1,
                SetupAction::Motion {
                    value: MotionPreference::Off,
                },
            )
            .unwrap();
        let reopened = OnboardingStore::open(dir.path())
            .unwrap()
            .snapshot()
            .unwrap();
        assert!(!reopened.cold_start);
        assert_eq!(reopened.preferences.step, SetupStep::Motion);
        assert_eq!(reopened.preferences.motion, MotionPreference::Off);
    }

    #[test]
    fn finish_and_restart_keep_preferences_and_reject_stale_views() {
        let dir = tempfile::tempdir().unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        store
            .apply(
                0,
                SetupAction::Project {
                    project_id: "project".into(),
                },
            )
            .unwrap();
        assert!(matches!(
            store.apply(0, SetupAction::Finish),
            Err(SetupError::Stale)
        ));
        assert!(
            store
                .apply(1, SetupAction::Finish)
                .unwrap()
                .preferences
                .completed
        );
        let restarted = store.apply(2, SetupAction::Restart).unwrap().preferences;
        assert!(!restarted.completed);
        assert_eq!(restarted.first_project_id.as_deref(), Some("project"));
        assert_eq!(restarted.step, SetupStep::Welcome);
    }

    /// Setup had a sign-in step while Ghost had accounts. A file saved on it still reads,
    /// with everything else the person chose.
    #[test]
    fn a_setup_left_on_the_removed_sign_in_step_continues_at_the_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":4,"step":"account","motion":"reduced","appearance":"dark","completed":false,"first_project_id":null,"general":{"default_agent":"codex","config_sync_paused":["pi"]}}}"#,
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(store.notice.is_none(), "the file is not treated as damage");
        let preferences = store.preferences();
        assert_eq!(preferences.step, SetupStep::Project);
        assert_eq!(preferences.revision, 4);
        assert_eq!(preferences.motion, MotionPreference::Reduced);
        assert_eq!(preferences.appearance, AppearancePreference::Dark);
        assert_eq!(preferences.general.default_agent.as_deref(), Some("codex"));
        assert!(!preferences.completed);
    }

    #[test]
    fn general_preferences_default_to_full_auto_with_every_shell_service_on() {
        let dir = tempfile::tempdir().unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        let general = store.preferences().general;
        assert_eq!(general, GeneralPreferences::default());
        assert_eq!(general.default_autonomy, Autonomy::FullAuto);
        assert!(!general.automatic_checkpoints);
        assert!(general.notifications && general.keep_awake && general.close_to_tray);
        assert_eq!(general.default_agent, None);
        // Reading for the shell is not the interface asking for setup.
        assert!(store.snapshot().unwrap().cold_start);
    }

    #[test]
    fn finished_turns_fold_unless_steps_are_kept_open_and_old_profiles_read_folded() {
        let dir = tempfile::tempdir().unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(!store.preferences().general.keep_steps_open);
        let next = store
            .apply(0, SetupAction::KeepStepsOpen { enabled: true })
            .unwrap();
        assert!(next.preferences.general.keep_steps_open);
        drop(store);
        assert!(
            OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .general
                .keep_steps_open
        );
        // A profile saved before the setting existed folds, as it did.
        let older = tempfile::tempdir().unwrap();
        std::fs::write(
            older.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":2,"step":"project","motion":"full","appearance":"light","completed":true,"first_project_id":null,"general":{"default_agent":null,"default_model":null,"default_effort":null,"default_autonomy":"full_auto","automatic_checkpoints":false,"notifications":true,"keep_awake":true,"close_to_tray":true,"config_sync_paused":[],"copy_instructions_to_worktrees":true,"agent_memory_writes":true}}}"#,
        )
        .unwrap();
        assert!(
            !OnboardingStore::open(older.path())
                .unwrap()
                .preferences()
                .general
                .keep_steps_open
        );
    }

    #[test]
    fn a_profile_saved_with_settings_roles_still_opens() {
        // Models by role lived in Settings for a day (19 November 2026) before moving to each
        // chat; a profile saved then keeps every other choice.
        let older = tempfile::tempdir().unwrap();
        std::fs::write(
            older.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":2,"step":"project","motion":"full","appearance":"light","completed":true,"first_project_id":null,"general":{"default_agent":"codex","default_model":null,"default_effort":null,"default_autonomy":"full_auto","automatic_checkpoints":false,"notifications":true,"keep_awake":true,"close_to_tray":true,"config_sync_paused":[],"copy_instructions_to_worktrees":true,"agent_memory_writes":true,"coordinated_projects":true,"roles":{"plan":{"agent":"claude","model":"claude-fable-5-2","effort":"high"},"build":null,"coordinator":null,"worker":null}}}}"#,
        )
        .unwrap();
        let general = OnboardingStore::open(older.path())
            .unwrap()
            .preferences()
            .general;
        assert_eq!(general.default_agent.as_deref(), Some("codex"));
        assert!(general.coordinated_projects);
    }

    /// The voice for reading aloud is the model's own until one is chosen, also for a profile
    /// from before the setting, and a choice is a bounded single line.
    #[test]
    fn the_read_aloud_voice_is_saved_and_refuses_anything_but_a_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":2,"step":"project","motion":"full","completed":true,"first_project_id":null,"general":{"notifications":false}}}"#,
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(store.snapshot().unwrap().notice.is_none());
        assert_eq!(store.preferences().general.speech_voice, None);
        let chosen = store
            .apply(
                2,
                SetupAction::SpeechVoice {
                    voice: Some("NbpkqMVS3CJeq2j8".into()),
                },
            )
            .unwrap();
        assert_eq!(
            chosen.preferences.general.speech_voice.as_deref(),
            Some("NbpkqMVS3CJeq2j8")
        );
        assert!(!chosen.preferences.general.notifications, "kept");
        for bad in [String::new(), "x".repeat(65), "line\nbreak".into()] {
            assert!(matches!(
                store.apply(3, SetupAction::SpeechVoice { voice: Some(bad) }),
                Err(SetupError::Invalid(_))
            ));
        }
        assert_eq!(
            OnboardingStore::open(dir.path())
                .unwrap()
                .preferences()
                .general
                .speech_voice
                .as_deref(),
            Some("NbpkqMVS3CJeq2j8")
        );
        let cleared = store
            .apply(3, SetupAction::SpeechVoice { voice: None })
            .unwrap();
        assert_eq!(cleared.preferences.general.speech_voice, None);
    }

    #[test]
    fn general_preferences_are_saved_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        let actions = [
            SetupAction::DefaultAgent {
                agent_id: Some("codex".into()),
            },
            SetupAction::DefaultModel {
                model: Some("gpt-5.5-codex".into()),
            },
            SetupAction::DefaultAutonomy {
                value: Autonomy::Ask,
            },
            SetupAction::AutomaticCheckpoints { enabled: true },
            SetupAction::Notifications { enabled: false },
            SetupAction::Sounds { enabled: true },
            SetupAction::Editor {
                editor: Some(ProjectEditor::Zed),
            },
            SetupAction::KeepAwake { enabled: false },
            SetupAction::CloseToTray { enabled: false },
        ];
        for (revision, action) in actions.into_iter().enumerate() {
            store.apply(revision as u32, action).unwrap();
        }
        let general = OnboardingStore::open(dir.path())
            .unwrap()
            .preferences()
            .general;
        assert_eq!(
            general,
            GeneralPreferences {
                default_agent: Some("codex".into()),
                default_model: Some("gpt-5.5-codex".into()),
                default_effort: None,
                default_autonomy: Autonomy::Ask,
                automatic_checkpoints: true,
                notifications: false,
                sounds: true,
                keep_awake: false,
                close_to_tray: false,
                copy_instructions_to_worktrees: true,
                agent_memory_writes: true,
                keep_steps_open: false,
                editor: Some(ProjectEditor::Zed),
                browser: true,
                browser_passwords: true,
                mobile: true,
                game: true,
                automatic_updates: true,
                coordinated_projects: false,
                speech_voice: None,
            }
        );
    }

    #[test]
    fn a_new_default_agent_clears_the_previous_agents_model() {
        let dir = tempfile::tempdir().unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(matches!(
            store.apply(
                0,
                SetupAction::DefaultModel {
                    model: Some("model".into())
                }
            ),
            Err(SetupError::Invalid(_))
        ));
        store
            .apply(
                0,
                SetupAction::DefaultAgent {
                    agent_id: Some("codex".into()),
                },
            )
            .unwrap();
        store
            .apply(
                1,
                SetupAction::DefaultModel {
                    model: Some("model".into()),
                },
            )
            .unwrap();
        // Choosing the same agent again keeps its model; another agent clears it.
        let same = store
            .apply(
                2,
                SetupAction::DefaultAgent {
                    agent_id: Some("codex".into()),
                },
            )
            .unwrap();
        assert_eq!(
            same.preferences.general.default_model.as_deref(),
            Some("model")
        );
        let other = store
            .apply(
                3,
                SetupAction::DefaultAgent {
                    agent_id: Some("claude".into()),
                },
            )
            .unwrap();
        assert_eq!(other.preferences.general.default_model, None);
        for bad in [String::new(), "x".repeat(129), "line\nbreak".into()] {
            assert!(matches!(
                store.apply(
                    4,
                    SetupAction::DefaultAgent {
                        agent_id: Some(bad)
                    }
                ),
                Err(SetupError::Invalid(_))
            ));
        }
    }

    /// The thinking level belongs to the agent and model it was chosen for.
    #[test]
    fn a_default_thinking_level_follows_its_agent_and_model() {
        let dir = tempfile::tempdir().unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        let effort = |effort: &str| SetupAction::DefaultEffort {
            effort: Some(effort.into()),
        };
        assert!(matches!(
            store.apply(0, effort("high")),
            Err(SetupError::Invalid(_))
        ));
        let agent = |id: &str| SetupAction::DefaultAgent {
            agent_id: Some(id.into()),
        };
        let model = |model: &str| SetupAction::DefaultModel {
            model: Some(model.into()),
        };
        store.apply(0, agent("claude")).unwrap();
        let set = store.apply(1, effort("high")).unwrap();
        assert_eq!(
            set.preferences.general.default_effort.as_deref(),
            Some("high"),
            "a level for the agent's own default model"
        );
        let same = store.apply(2, model_none()).unwrap();
        assert_eq!(
            same.preferences.general.default_effort.as_deref(),
            Some("high"),
            "the same model keeps it"
        );
        let moved = store.apply(3, model("opus")).unwrap();
        assert_eq!(moved.preferences.general.default_effort, None);
        store.apply(4, effort("max")).unwrap();
        let other = store.apply(5, agent("codex")).unwrap();
        assert_eq!(other.preferences.general.default_effort, None);
        for bad in [String::new(), "x".repeat(129), "line\nbreak".into()] {
            assert!(matches!(
                store.apply(6, SetupAction::DefaultEffort { effort: Some(bad) }),
                Err(SetupError::Invalid(_))
            ));
        }
        let cleared = store
            .apply(6, SetupAction::DefaultEffort { effort: None })
            .unwrap();
        assert_eq!(cleared.preferences.general.default_effort, None);
    }

    fn model_none() -> SetupAction {
        SetupAction::DefaultModel { model: None }
    }

    /// A default saved as "Claude Code (ACP)" from before agents had one entry each is Claude
    /// Code, with its model; choosing an adapter id now saves the agent.
    #[test]
    fn a_default_agent_saved_as_a_connection_reads_as_its_agent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":3,"step":"project","motion":"full","completed":true,"first_project_id":null,"general":{"default_agent":"codex-acp","default_model":"gpt-6"}}}"#,
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        let general = store.snapshot().unwrap().preferences.general;
        assert_eq!(general.default_agent.as_deref(), Some("codex"));
        assert_eq!(general.default_model.as_deref(), Some("gpt-6"));
        // The same agent through its other id keeps the model.
        let same = store
            .apply(
                3,
                SetupAction::DefaultAgent {
                    agent_id: Some("codex-acp".into()),
                },
            )
            .unwrap();
        assert_eq!(
            same.preferences.general.default_agent.as_deref(),
            Some("codex")
        );
        assert_eq!(
            same.preferences.general.default_model.as_deref(),
            Some("gpt-6")
        );
    }

    #[test]
    fn files_from_before_general_preferences_read_their_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":3,"step":"project","motion":"reduced","completed":true,"first_project_id":null}}"#,
        )
        .unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        let snapshot = store.snapshot().unwrap();
        assert!(snapshot.notice.is_none(), "an older file is not damaged");
        assert_eq!(snapshot.preferences.motion, MotionPreference::Reduced);
        assert_eq!(snapshot.preferences.general, GeneralPreferences::default());
        // A file missing only some of them keeps what it has.
        std::fs::write(
            dir.path().join("onboarding.json"),
            br#"{"version":1,"boot_seen":true,"preferences":{"revision":3,"step":"project","motion":"full","completed":true,"first_project_id":null,"general":{"notifications":false}}}"#,
        )
        .unwrap();
        let general = OnboardingStore::open(dir.path())
            .unwrap()
            .preferences()
            .general;
        assert!(!general.notifications);
        assert!(general.keep_awake && general.close_to_tray);
        assert_eq!(general.default_autonomy, Autonomy::FullAuto);
    }

    #[test]
    fn failed_write_does_not_advance_memory_or_acknowledge_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        std::fs::create_dir(dir.path().join("onboarding.json")).unwrap();
        assert!(store.apply(0, SetupAction::Finish).is_err());
        assert_eq!(store.record.lock().unwrap().preferences.revision, 0);
        assert!(!store.record.lock().unwrap().preferences.completed);
    }

    #[test]
    fn damaged_preferences_are_preserved_but_future_data_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onboarding.json");
        std::fs::write(&path, b"broken").unwrap();
        let store = OnboardingStore::open(dir.path()).unwrap();
        assert!(store.snapshot().unwrap().notice.is_some());
        assert!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .any(|entry| std::fs::read(entry.unwrap().path()).unwrap() == b"broken")
        );
        std::fs::write(&path, br#"{"version":2,"future":true}"#).unwrap();
        assert!(matches!(
            OnboardingStore::open(dir.path()),
            Err(SetupError::NewerVersion)
        ));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            br#"{"version":2,"future":true}"#
        );
    }
}
