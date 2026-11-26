//! How this launch was asked to behave, read once from the environment at startup.
//!
//! These switches exist for automation, not for users: `GHOST_SMOKE` runs a scripted check and
//! reports the result on stdout, `GHOST_STRESS` opens straight into the stress view for the power
//! spike, and `GHOST_NO_GLASS` turns the native window material off so its cost can be measured.
//! The frontend learns the outcome through `app_info` (`LaunchMode`), never by reading the
//! environment itself.
//!
//! `GHOST_DATA_DIR` is ghost-core's: it moves the data folder. This module only notes whether the
//! launch opens a folder other than the installed app's, because such a launch is its own instance
//! ([`LaunchOptions::single_instance`]). A development build always does: without
//! `GHOST_DATA_DIR` it opens ghost-core's development folder (`ghost_core::paths`). A smoke run
//! must name its folder ([`SMOKE_NEEDS_DATA_DIR`]).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use ghost_core::paths::{DATA_DIR_ENV, DEVELOPMENT_BUILD};
use serde::{Deserialize, Serialize};

use crate::smoke::SmokeScenario;

/// Which smoke check to run ([`SmokeScenario`]): `1` (or `stream`) the stream check (a demo turn, a
/// cancelled turn, and a short stress burst), `chat` the chat screens against the demo agent, and
/// `chat-reopen` the second launch of that check on the same data folder.
pub const SMOKE_ENV: &str = "GHOST_SMOKE";
/// With smoke mode, `1` exits after the report: code 0 when it passed, 1 otherwise.
pub const SMOKE_EXIT_ENV: &str = "GHOST_SMOKE_EXIT";
/// The chat scenarios' chat folder, an absolute path. The folder picker answers it without showing
/// a dialog, so the run creates its chat through the New chat screen as a user would.
pub const SMOKE_FOLDER_ENV: &str = "GHOST_SMOKE_FOLDER";
/// With `GHOST_SMOKE=chat`, one minimal chat with this agent (a catalog id) instead of the demo
/// agent's scenario. A real agent spends its user's quota, so this is refused under CI.
pub const SMOKE_AGENT_ENV: &str = "GHOST_SMOKE_AGENT";
/// Set by CI systems; GitHub Actions sets both.
pub const CI_ENVS: [&str; 2] = ["CI", "GITHUB_ACTIONS"];
/// The built-in demo agent's catalog id, which the chat scenarios use.
pub const DEMO_AGENT: &str = "mock";
/// `orbs=24,rate=200,aura=1,seconds=0` opens the stress view with those settings.
pub const STRESS_ENV: &str = "GHOST_STRESS";
/// `1` disables window effects and transparency (an opaque window).
pub const NO_GLASS_ENV: &str = "GHOST_NO_GLASS";
/// The webview profile folder inside a data folder, the name WebView2 and Tauri use by default.
pub const WEBVIEW_DATA_FOLDER: &str = "EBWebView";
/// Why a smoke run without `GHOST_DATA_DIR` does not run. A smoke run skips the single-instance
/// hand-over, so without a folder of its own it would open the default one beside a running app:
/// for an installed build, the installed app's chats, which the run's quit flow and any newer
/// schema would then change.
pub const SMOKE_NEEDS_DATA_DIR: &str = "a smoke run needs GHOST_DATA_DIR: a data folder of its own, so it never opens the installed app's data.";

/// Bounds for [`StressConfig`], shared with the stress view's controls.
pub const MIN_ORBS: u32 = 1;
pub const MAX_ORBS: u32 = 24;
pub const MIN_RATE: u32 = ghost_drivers::mock::MIN_TOKENS_PER_SEC;
pub const MAX_RATE: u32 = ghost_drivers::mock::MAX_TOKENS_PER_SEC;
/// A day. Longer runs use `seconds=0` (until the window closes).
pub const MAX_SECONDS: u32 = 86_400;

/// Settings for the stress view: `orbs` live orbs, a stream at `rate` tokens per second, the aura
/// field on or off, for `seconds` seconds (0 keeps streaming until the window closes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct StressConfig {
    pub orbs: u32,
    pub rate: u32,
    pub aura: bool,
    pub seconds: u32,
}

impl Default for StressConfig {
    fn default() -> StressConfig {
        StressConfig {
            orbs: 6,
            rate: 200,
            aura: true,
            seconds: 0,
        }
    }
}

/// Why a `GHOST_STRESS` value was rejected. Rejected rather than clamped: the value drives a
/// power measurement, and a silently adjusted setting would measure something else.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StressConfigError {
    #[error("\"{0}\" is not a key=value pair")]
    NotAPair(String),
    #[error("unknown setting \"{0}\" (expected orbs, rate, aura or seconds)")]
    UnknownKey(String),
    #[error("\"{0}\" is set more than once")]
    Duplicate(String),
    #[error("{key} must be a whole number from {min} to {max}, not \"{value}\"")]
    OutOfRange {
        key: &'static str,
        value: String,
        min: u32,
        max: u32,
    },
    #[error("aura must be 1 or 0, not \"{0}\"")]
    NotABool(String),
}

/// Parses `GHOST_STRESS`: comma-separated `key=value` pairs (`orbs`, `rate`, `aura`, `seconds`)
/// in any order, with defaults for missing keys. A bare `1`/`true`/`yes`/`on` means all defaults.
/// Keys are case-insensitive and whitespace around entries is ignored. The frontend's
/// `parseStressConfig` (lib/stress.ts) applies the same rules to the stress view's settings line,
/// and both test the same cases.
pub fn parse_stress_config(input: &str) -> Result<StressConfig, StressConfigError> {
    let trimmed = input.trim();
    let mut config = StressConfig::default();
    if is_truthy(trimmed) {
        return Ok(config);
    }
    let mut seen: Vec<&'static str> = Vec::new();
    for entry in trimmed.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let Some((raw_key, raw_value)) = entry.split_once('=') else {
            return Err(StressConfigError::NotAPair(entry.to_string()));
        };
        let key = match raw_key.trim().to_ascii_lowercase().as_str() {
            "orbs" => "orbs",
            "rate" => "rate",
            "aura" => "aura",
            "seconds" => "seconds",
            _ => return Err(StressConfigError::UnknownKey(raw_key.trim().to_string())),
        };
        if seen.contains(&key) {
            return Err(StressConfigError::Duplicate(key.to_string()));
        }
        seen.push(key);
        let value = raw_value.trim();
        match key {
            "orbs" => config.orbs = parse_bounded(key, value, MIN_ORBS, MAX_ORBS)?,
            "rate" => config.rate = parse_bounded(key, value, MIN_RATE, MAX_RATE)?,
            "seconds" => config.seconds = parse_bounded(key, value, 0, MAX_SECONDS)?,
            _ => config.aura = parse_bool(value)?,
        }
    }
    Ok(config)
}

/// Only plain decimal digits: `u32::from_str` would also take `+5`, which the frontend's parser
/// rejects, and the two must agree.
fn parse_bounded(
    key: &'static str,
    value: &str,
    min: u32,
    max: u32,
) -> Result<u32, StressConfigError> {
    let out_of_range = || StressConfigError::OutOfRange {
        key,
        value: value.to_string(),
        min,
        max,
    };
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(out_of_range());
    }
    match value.parse::<u32>() {
        Ok(number) if (min..=max).contains(&number) => Ok(number),
        _ => Err(out_of_range()),
    }
}

fn parse_bool(value: &str) -> Result<bool, StressConfigError> {
    if is_truthy(value) {
        Ok(true)
    } else if is_falsy(value) {
        Ok(false)
    } else {
        Err(StressConfigError::NotABool(value.to_string()))
    }
}

fn is_truthy(value: &str) -> bool {
    ["1", "true", "yes", "on"]
        .iter()
        .any(|v| value.eq_ignore_ascii_case(v))
}

fn is_falsy(value: &str) -> bool {
    ["0", "false", "no", "off"]
        .iter()
        .any(|v| value.eq_ignore_ascii_case(v))
}

/// Whether an on/off environment variable is on. Unset, blank, and anything but `1`, `true`, `yes`
/// or `on` (any case) is off, so `GHOST_SMOKE=0` really means off.
pub fn env_flag(value: Option<&str>) -> bool {
    value.is_some_and(|v| is_truthy(v.trim()))
}

/// Whether a `GHOST_DATA_DIR` value moves the data folder, by the rule ghost-core's
/// `DataRoot::resolve` applies: a value that is empty or only whitespace is ignored, and one that
/// is not valid Unicode counts unless it is empty. ghost-core keeps the rule private, so
/// `own_data_dir_agrees_with_where_aura_core_puts_the_data` holds the two together.
fn moves_data_root(value: &OsStr) -> bool {
    match value.to_str() {
        Some(text) => !text.trim().is_empty(),
        None => !value.is_empty(),
    }
}

/// What a smoke launch runs: its scenario, with the agent and folder the chat scenarios use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct SmokePlan {
    pub scenario: SmokeScenario,
    /// The agent the chat scenarios create their chat with: the demo agent for `chat` and
    /// `chat_reopen`, `GHOST_SMOKE_AGENT`'s for `agent`; `null` for `stream`.
    pub agent_id: Option<String>,
    /// `GHOST_SMOKE_FOLDER`, which the folder picker answers; `null` for `stream`.
    pub folder: Option<String>,
}

impl SmokePlan {
    /// Whether this plan opens chats (every scenario but the stream check).
    pub fn chats(&self) -> bool {
        self.scenario != SmokeScenario::Stream
    }
}

/// Why the smoke switches cannot run a scenario. Smoke mode still holds (automation is waiting
/// for a result), so the launch prints a failing report instead of running one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SmokeSetupError {
    /// The scenario asked for, when `GHOST_SMOKE` named one.
    pub scenario: Option<SmokeScenario>,
    pub message: String,
}

impl SmokeSetupError {
    fn new(scenario: Option<SmokeScenario>, message: String) -> SmokeSetupError {
        SmokeSetupError { scenario, message }
    }
}

/// Reads the smoke switches: `None` when `GHOST_SMOKE` is unset, blank, or an explicit off
/// (`0`, `false`, `no`, `off`); otherwise the plan, or why it cannot run.
///
/// - `1`, `true`, `yes`, `on` or `stream` (any case): the stream check. `chat`: the chat scenario,
///   or with `agent` set the minimal chat with that agent. `chat-reopen`: the chat scenario's
///   second launch. Anything else names no scenario.
/// - The chat scenarios need `folder` (`GHOST_SMOKE_FOLDER`): an absolute path in valid Unicode
///   (a chat folder reaches the agent as a JSON string). Whether it exists is the session manager's
///   to check, as for any new chat.
/// - `agent` (`GHOST_SMOKE_AGENT`) must be a catalog id and goes with `chat` only. It is refused
///   when `in_ci` (`CI` or `GITHUB_ACTIONS` is on): a real agent run spends the user's quota.
pub fn parse_smoke(
    value: Option<&str>,
    folder: Option<OsString>,
    agent: Option<&str>,
    in_ci: bool,
) -> Option<Result<SmokePlan, SmokeSetupError>> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty() && !is_falsy(v))?;
    let named = if is_truthy(value) {
        Some(SmokeScenario::Stream)
    } else {
        match value.to_ascii_lowercase().as_str() {
            "stream" => Some(SmokeScenario::Stream),
            "chat" => Some(SmokeScenario::Chat),
            "chat-reopen" | "chat_reopen" => Some(SmokeScenario::ChatReopen),
            _ => None,
        }
    };
    let Some(named) = named else {
        return Some(Err(SmokeSetupError::new(
            None,
            format!(
                "{SMOKE_ENV}=\"{value}\" names no smoke run: use 1 (or stream), chat or chat-reopen."
            ),
        )));
    };
    Some(smoke_plan(
        named,
        folder,
        agent.map(str::trim).filter(|agent| !agent.is_empty()),
        in_ci,
    ))
}

fn smoke_plan(
    named: SmokeScenario,
    folder: Option<OsString>,
    agent: Option<&str>,
    in_ci: bool,
) -> Result<SmokePlan, SmokeSetupError> {
    let scenario = match (named, agent) {
        (SmokeScenario::Chat, Some(_)) => SmokeScenario::Agent,
        (_, Some(_)) => {
            return Err(SmokeSetupError::new(
                Some(named),
                format!("{SMOKE_AGENT_ENV} goes with {SMOKE_ENV}=chat only."),
            ));
        }
        (named, None) => named,
    };
    if scenario == SmokeScenario::Stream {
        return Ok(SmokePlan {
            scenario,
            agent_id: None,
            folder: None,
        });
    }
    let agent_id = match agent {
        None => DEMO_AGENT.to_owned(),
        Some(_) if in_ci => {
            return Err(SmokeSetupError::new(
                Some(scenario),
                format!(
                    "{SMOKE_AGENT_ENV} runs a real agent, which spends its user's quota, so it is for local runs only: it is refused while CI or GITHUB_ACTIONS is set."
                ),
            ));
        }
        Some(id) => match ghost_agents::get(id) {
            Some(spec) => spec.id.to_owned(),
            None => {
                let known: Vec<&str> = ghost_agents::catalog().iter().map(|spec| spec.id).collect();
                return Err(SmokeSetupError::new(
                    Some(scenario),
                    format!(
                        "{SMOKE_AGENT_ENV}=\"{id}\" is not an agent Ghost knows ({}).",
                        known.join(", ")
                    ),
                ));
            }
        },
    };
    let needs = || {
        SmokeSetupError::new(
            Some(scenario),
            format!(
                "{SMOKE_ENV}={} needs {SMOKE_FOLDER_ENV}: an absolute path to the folder its chat opens in.",
                if scenario == SmokeScenario::ChatReopen {
                    "chat-reopen"
                } else {
                    "chat"
                }
            ),
        )
    };
    // Blank reads as unset, the rule `GHOST_DATA_DIR` follows.
    let folder = folder
        .filter(|folder| moves_data_root(folder))
        .ok_or_else(needs)?;
    let folder = folder.into_string().map_err(|folder| {
        SmokeSetupError::new(
            Some(scenario),
            format!(
                "{SMOKE_FOLDER_ENV} must be valid Unicode, not {}.",
                Path::new(&folder).display()
            ),
        )
    })?;
    if !Path::new(&folder).is_absolute() {
        return Err(SmokeSetupError::new(
            Some(scenario),
            format!("{SMOKE_FOLDER_ENV} must be an absolute path, not \"{folder}\"."),
        ));
    }
    Ok(SmokePlan {
        scenario,
        agent_id: Some(agent_id),
        folder: Some(folder),
    })
}

/// What the UI should open into. Smoke mode wins over the stress view, since it is the one that
/// exits by itself and a CI job is waiting on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LaunchMode {
    Normal,
    Smoke { plan: SmokePlan },
    Stress { config: StressConfig },
}

/// Every launch switch, read once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    /// `GHOST_SMOKE` asked for a smoke run: what it runs, or why it cannot ([`parse_smoke`]).
    pub smoke: Option<Result<SmokePlan, SmokeSetupError>>,
    pub smoke_exit: bool,
    pub no_glass: bool,
    /// This launch opens a data folder other than the installed app's: `GHOST_DATA_DIR` names
    /// one (a second dev instance, a portable install), or this is a development build, whose
    /// default folder is its own.
    pub own_data_dir: bool,
    /// `GHOST_DATA_DIR` names a data folder: a start at sign-in, which cannot pass it, would open
    /// the usual one instead (`login_start`).
    pub names_data_dir: bool,
    /// `None` when `GHOST_STRESS` is unset, blank (like `GHOST_DATA_DIR`, an exported but
    /// empty variable must not change behaviour), or an explicit off (`0`, `false`, `no`, `off`).
    pub stress: Option<Result<StressConfig, StressConfigError>>,
}

impl LaunchOptions {
    pub fn from_env() -> LaunchOptions {
        LaunchOptions::from_lookup(|name| std::env::var_os(name))
    }

    /// [`LaunchOptions::from_env`] with the variables passed in, so tests never touch the process
    /// environment (shared by every test thread). Values come as the OS gave them: ghost-core
    /// honours a `GHOST_DATA_DIR` that is not valid Unicode, so this must see it too. The
    /// other switches treat such a value as unset.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> LaunchOptions {
        LaunchOptions::from_lookup_in(DEVELOPMENT_BUILD, lookup)
    }

    /// [`LaunchOptions::from_lookup`] for a development build or an installed one, so tests cover
    /// both from one test binary.
    pub fn from_lookup_in(
        development: bool,
        lookup: impl Fn(&str) -> Option<OsString>,
    ) -> LaunchOptions {
        let text = |name: &str| lookup(name).and_then(|value| value.into_string().ok());
        let in_ci = CI_ENVS.iter().any(|name| env_flag(text(name).as_deref()));
        // The override under the app's earlier names counts too (ghost_core::paths).
        let names_data_dir = std::iter::once(DATA_DIR_ENV)
            .chain(ghost_core::paths::LEGACY_DATA_DIR_ENVS)
            .any(|name| lookup(name).is_some_and(|value| moves_data_root(&value)));
        let smoke = parse_smoke(
            text(SMOKE_ENV).as_deref(),
            lookup(SMOKE_FOLDER_ENV),
            text(SMOKE_AGENT_ENV).as_deref(),
            in_ci,
        )
        .map(|smoke| {
            smoke.and_then(|plan| {
                if names_data_dir {
                    Ok(plan)
                } else {
                    Err(SmokeSetupError::new(
                        Some(plan.scenario),
                        SMOKE_NEEDS_DATA_DIR.to_owned(),
                    ))
                }
            })
        });
        LaunchOptions {
            smoke_exit: smoke.is_some() && env_flag(text(SMOKE_EXIT_ENV).as_deref()),
            smoke,
            no_glass: env_flag(text(NO_GLASS_ENV).as_deref()),
            own_data_dir: development || names_data_dir,
            names_data_dir,
            stress: text(STRESS_ENV)
                .filter(|value| !value.trim().is_empty() && !is_falsy(value.trim()))
                .map(|value| parse_stress_config(&value)),
        }
    }

    /// Whether `GHOST_SMOKE` asked for a smoke run, whether or not it can run.
    pub fn is_smoke(&self) -> bool {
        self.smoke.is_some()
    }

    /// The smoke run this launch makes, when its switches are valid.
    pub fn smoke_plan(&self) -> Option<&SmokePlan> {
        self.smoke.as_ref().and_then(|smoke| smoke.as_ref().ok())
    }

    /// The folder the folder picker answers without a dialog: a chat scenario's chat folder.
    pub fn smoke_folder(&self) -> Option<&str> {
        self.smoke_plan()
            .filter(|plan| plan.chats())
            .and_then(|plan| plan.folder.as_deref())
    }

    /// Whether this launch registers the single-instance plugin, which makes a later launch focus
    /// this one's window and exit. The plugin recognizes the running instance by the bundle
    /// identifier alone, so every launch on the machine counts as the same app. Two kinds of
    /// launch run on their own instead: a smoke run, which has to run even while someone has
    /// Ghost open, and a launch with its own data folder, which would otherwise exit without
    /// ever opening it. Such a launch never hands over to a running instance, and later launches
    /// never hand over to it. Sharing data with a running instance is safe: ghost-core coordinates
    /// processes that open the same database.
    pub fn single_instance(&self) -> bool {
        !self.is_smoke() && !self.own_data_dir
    }

    /// Where the webview keeps its profile (storage, caches) for a launch with its own data
    /// folder: `EBWebView` inside that folder, the same layout as the default folder, where Tauri
    /// puts it on its own. Without this a throwaway first launch would share the installed app's
    /// webview profile, and WebView2 refuses a second app on one profile while the first runs.
    /// `None` keeps Tauri's default. macOS ignores the setting and keeps WebKit's per-app store.
    ///
    /// A copy updated from o7 or from auracode keeps its data in the folder it used then
    /// (ghost_core::paths), which is no longer Tauri's default. Tauri gave WebView2 that folder
    /// itself, and WebView2 keeps its profile in `EBWebView` inside the folder it is given, so the
    /// same folder is given again: the profile is the one the app had then, with the page's own
    /// settings and the browser's sign-ins. (macOS keys WebKit's store by the bundle identifier,
    /// so there they start over.)
    pub fn webview_data_dir(&self, data_dir: Option<&Path>) -> Option<PathBuf> {
        let root = data_dir?;
        if self.own_data_dir {
            return Some(root.join(WEBVIEW_DATA_FOLDER));
        }
        let default = root
            .file_name()
            .is_some_and(|name| name == ghost_core::paths::APP_IDENTIFIER);
        (!default).then(|| root.to_path_buf())
    }

    /// Whether the window-state plugin restores and saves the window's geometry. It keeps one
    /// file per bundle identifier in the roaming config folder, which a launch with its own data
    /// folder must not read or write: the installed app's window would open where a test left it.
    pub fn remembers_window_geometry(&self) -> bool {
        !self.own_data_dir
    }

    /// A smoke launch whose switches are not valid opens the normal screen: it reports its failure
    /// itself (and exits with `GHOST_SMOKE_EXIT`), and must not open the stress view either.
    pub fn mode(&self) -> LaunchMode {
        match &self.smoke {
            Some(Ok(plan)) => return LaunchMode::Smoke { plan: plan.clone() },
            Some(Err(_)) => return LaunchMode::Normal,
            None => {}
        }
        match &self.stress {
            Some(Ok(config)) => LaunchMode::Stress { config: *config },
            Some(Err(_)) | None => LaunchMode::Normal,
        }
    }

    /// Sentences for the UI's notices list: settings that were present but could not be used.
    pub fn notices(&self) -> Vec<String> {
        let mut notices = Vec::new();
        if let Some(Err(error)) = &self.smoke {
            notices.push(format!("{SMOKE_ENV} was ignored: {error}"));
        }
        if let Some(Err(error)) = &self.stress {
            notices.push(format!(
                "{STRESS_ENV} was ignored because it is not valid: {error}."
            ));
        }
        if self.no_glass {
            notices.push(format!(
                "Window glass is off because {NO_GLASS_ENV} is set."
            ));
        }
        notices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config(orbs: u32, rate: u32, aura: bool, seconds: u32) -> StressConfig {
        StressConfig {
            orbs,
            rate,
            aura,
            seconds,
        }
    }

    #[test]
    fn parses_the_documented_example() {
        assert_eq!(
            parse_stress_config("orbs=24,rate=200,aura=1,seconds=0"),
            Ok(config(24, 200, true, 0))
        );
    }

    #[test]
    fn missing_keys_take_defaults_and_order_does_not_matter() {
        assert_eq!(
            parse_stress_config("seconds=30, orbs=3"),
            Ok(config(3, 200, true, 30))
        );
        assert_eq!(parse_stress_config("aura=0"), Ok(config(6, 200, false, 0)));
    }

    #[test]
    fn a_bare_flag_means_all_defaults() {
        for flag in ["1", "true", "YES", " on "] {
            assert_eq!(
                parse_stress_config(flag),
                Ok(StressConfig::default()),
                "{flag}"
            );
        }
    }

    #[test]
    fn whitespace_case_and_empty_entries_are_tolerated() {
        assert_eq!(
            parse_stress_config(" ORBS = 2 ,, Rate=10 , aura = off ,"),
            Ok(config(2, 10, false, 0))
        );
    }

    #[test]
    fn bounds_are_inclusive() {
        assert_eq!(
            parse_stress_config("orbs=1,rate=1,seconds=86400"),
            Ok(config(1, 1, true, 86_400))
        );
        assert_eq!(
            parse_stress_config("orbs=24,rate=5000"),
            Ok(config(24, 5000, true, 0))
        );
    }

    #[test]
    fn out_of_range_and_malformed_numbers_are_rejected() {
        for input in [
            "orbs=0",
            "orbs=25",
            "rate=0",
            "rate=5001",
            "seconds=86401",
            "orbs=-1",
            "orbs=+5",
            "orbs=2.5",
            "orbs=",
            "rate=99999999999",
            "orbs=1e3",
        ] {
            assert!(
                matches!(
                    parse_stress_config(input),
                    Err(StressConfigError::OutOfRange { .. })
                ),
                "{input}"
            );
        }
    }

    #[test]
    fn unknown_duplicate_and_unpaired_entries_are_rejected() {
        assert_eq!(
            parse_stress_config("orbs=2,speed=3"),
            Err(StressConfigError::UnknownKey("speed".into()))
        );
        assert_eq!(
            parse_stress_config("orbs=2,ORBS=3"),
            Err(StressConfigError::Duplicate("orbs".into()))
        );
        assert_eq!(
            parse_stress_config("orbs"),
            Err(StressConfigError::NotAPair("orbs".into()))
        );
        assert_eq!(
            parse_stress_config("aura=maybe"),
            Err(StressConfigError::NotABool("maybe".into()))
        );
    }

    #[test]
    fn errors_read_as_sentences_in_the_notice() {
        let options =
            LaunchOptions::from_lookup(|name| (name == STRESS_ENV).then(|| "orbs=99".into()));
        assert_eq!(options.mode(), LaunchMode::Normal);
        assert_eq!(
            options.notices(),
            vec![
                "GHOST_STRESS was ignored because it is not valid: orbs must be a whole number from 1 to 24, not \"99\"."
                    .to_string()
            ]
        );
    }

    /// An installed (release) build's options. The test binary is a development build, which
    /// [`development_options`] covers.
    fn options(vars: &[(&str, &str)]) -> LaunchOptions {
        options_in(false, vars)
    }

    fn development_options(vars: &[(&str, &str)]) -> LaunchOptions {
        options_in(true, vars)
    }

    fn options_in(development: bool, vars: &[(&str, &str)]) -> LaunchOptions {
        let map: HashMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        LaunchOptions::from_lookup_in(development, |name| map.get(name).cloned())
    }

    /// A smoke run's own data folder, which every runnable smoke run names.
    fn smoke_data() -> String {
        absolute("smoke data")
    }

    #[test]
    fn flags_need_an_explicit_on_value() {
        assert!(env_flag(Some("1")));
        assert!(env_flag(Some(" TRUE ")));
        assert!(!env_flag(Some("0")));
        assert!(!env_flag(Some("")));
        assert!(!env_flag(Some("off")));
        assert!(!env_flag(None));
    }

    fn stream_plan() -> SmokePlan {
        SmokePlan {
            scenario: SmokeScenario::Stream,
            agent_id: None,
            folder: None,
        }
    }

    /// An absolute folder on every OS (a Windows path needs its drive).
    fn absolute(name: &str) -> String {
        std::env::temp_dir()
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn plan(
        value: &str,
        folder: Option<&str>,
        agent: Option<&str>,
        in_ci: bool,
    ) -> Option<Result<SmokePlan, SmokeSetupError>> {
        parse_smoke(Some(value), folder.map(OsString::from), agent, in_ci)
    }

    fn setup_error(scenario: Option<SmokeScenario>, message: &str) -> SmokeSetupError {
        SmokeSetupError {
            scenario,
            message: message.to_owned(),
        }
    }

    #[test]
    fn smoke_wins_over_stress_and_exit_needs_smoke() {
        let data = smoke_data();
        let both = options(&[
            (SMOKE_ENV, "1"),
            (SMOKE_EXIT_ENV, "1"),
            (STRESS_ENV, "orbs=3"),
            (DATA_DIR_ENV, &data),
        ]);
        assert_eq!(
            both.mode(),
            LaunchMode::Smoke {
                plan: stream_plan()
            }
        );
        assert!(both.smoke_exit);

        let exit_alone = options(&[(SMOKE_EXIT_ENV, "1")]);
        assert!(!exit_alone.smoke_exit, "exit without smoke must do nothing");
        assert_eq!(exit_alone.mode(), LaunchMode::Normal);
    }

    #[test]
    fn the_stream_check_answers_to_on_values_and_its_name() {
        for value in ["1", "true", " YES ", "on", "stream", "Stream"] {
            assert_eq!(
                plan(value, None, None, false),
                Some(Ok(stream_plan())),
                "{value:?}"
            );
        }
        // Its folder is unused, so it is not checked.
        assert_eq!(
            plan("1", Some("relative"), None, false),
            Some(Ok(stream_plan()))
        );
    }

    #[test]
    fn unset_blank_and_off_values_are_no_smoke_run() {
        assert_eq!(parse_smoke(None, None, None, false), None);
        for value in ["", "  ", "0", "off", "FALSE", "no"] {
            assert_eq!(
                plan(value, Some("/x"), Some("mock"), false),
                None,
                "{value:?}"
            );
        }
    }

    #[test]
    fn the_chat_scenarios_use_the_demo_agent_in_their_folder() {
        let folder = absolute("smoke chat");
        assert_eq!(
            plan(" chat ", Some(&folder), None, false),
            Some(Ok(SmokePlan {
                scenario: SmokeScenario::Chat,
                agent_id: Some(DEMO_AGENT.into()),
                folder: Some(folder.clone()),
            }))
        );
        for value in ["chat-reopen", "CHAT_REOPEN"] {
            assert_eq!(
                plan(value, Some(&folder), None, false),
                Some(Ok(SmokePlan {
                    scenario: SmokeScenario::ChatReopen,
                    agent_id: Some(DEMO_AGENT.into()),
                    folder: Some(folder.clone()),
                })),
                "{value}"
            );
        }
    }

    /// The UI never runs a scenario these switches cannot run; the launch reports why instead.
    #[test]
    fn a_smoke_run_that_cannot_run_says_why() {
        assert_eq!(
            plan("chta", None, None, false),
            Some(Err(setup_error(
                None,
                "GHOST_SMOKE=\"chta\" names no smoke run: use 1 (or stream), chat or chat-reopen."
            )))
        );
        for folder in [None, Some(""), Some("   ")] {
            assert_eq!(
                plan("chat", folder, None, false),
                Some(Err(setup_error(
                    Some(SmokeScenario::Chat),
                    "GHOST_SMOKE=chat needs GHOST_SMOKE_FOLDER: an absolute path to the folder its chat opens in."
                ))),
                "{folder:?}"
            );
        }
        assert_eq!(
            plan("chat-reopen", None, None, false),
            Some(Err(setup_error(
                Some(SmokeScenario::ChatReopen),
                "GHOST_SMOKE=chat-reopen needs GHOST_SMOKE_FOLDER: an absolute path to the folder its chat opens in."
            )))
        );
        assert_eq!(
            plan("chat", Some("work/chat"), None, false),
            Some(Err(setup_error(
                Some(SmokeScenario::Chat),
                "GHOST_SMOKE_FOLDER must be an absolute path, not \"work/chat\"."
            )))
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_smoke_folder_must_be_unicode() {
        let folder = not_unicode(std::env::temp_dir().as_os_str());
        let refused = parse_smoke(Some("chat"), Some(folder), None, false);
        let Some(Err(error)) = refused else {
            panic!("accepted: {refused:?}");
        };
        assert_eq!(error.scenario, Some(SmokeScenario::Chat));
        assert!(
            error
                .message
                .starts_with("GHOST_SMOKE_FOLDER must be valid Unicode"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_smoke_agent_runs_the_minimal_chat_with_that_agent() {
        let folder = absolute("agent chat");
        for (given, id) in [
            ("claude", "claude"),
            ("claude-acp", "claude-acp"),
            (" hermes ", "hermes"),
        ] {
            assert_eq!(
                plan("chat", Some(&folder), Some(given), false),
                Some(Ok(SmokePlan {
                    scenario: SmokeScenario::Agent,
                    agent_id: Some(id.into()),
                    folder: Some(folder.clone()),
                }))
            );
        }
        // Blank means unset: the demo agent's scenario.
        assert_eq!(
            plan("chat", Some(&folder), Some("  "), false).map(|plan| plan.map(|p| p.scenario)),
            Some(Ok(SmokeScenario::Chat))
        );
        assert_eq!(
            plan("chat", Some(&folder), Some("gpt-cli"), false),
            Some(Err(setup_error(
                Some(SmokeScenario::Agent),
                &format!(
                    "GHOST_SMOKE_AGENT=\"gpt-cli\" is not an agent Ghost knows ({}).",
                    ghost_agents::catalog()
                        .iter()
                        .map(|spec| spec.id)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            )))
        );
        for value in ["1", "chat-reopen"] {
            let refused = plan(value, Some(&folder), Some("claude-acp"), false);
            assert!(
                matches!(&refused, Some(Err(error)) if error.message == "GHOST_SMOKE_AGENT goes with GHOST_SMOKE=chat only."),
                "{value}: {refused:?}"
            );
        }
    }

    /// A real agent spends its user's quota: CI never runs one, even the demo agent's minimal chat.
    #[test]
    fn a_smoke_agent_is_refused_under_ci() {
        let folder = absolute("agent chat");
        for agent in ["claude-acp", "mock"] {
            assert_eq!(
                plan("chat", Some(&folder), Some(agent), true),
                Some(Err(setup_error(
                    Some(SmokeScenario::Agent),
                    "GHOST_SMOKE_AGENT runs a real agent, which spends its user's quota, so it is for local runs only: it is refused while CI or GITHUB_ACTIONS is set."
                )))
            );
        }
        // The demo agent's own scenario runs in CI.
        assert!(matches!(
            plan("chat", Some(&folder), None, true),
            Some(Ok(_))
        ));
        for ci in ["CI", "GITHUB_ACTIONS"] {
            let refused = options(&[
                (SMOKE_ENV, "chat"),
                (SMOKE_FOLDER_ENV, &folder),
                (SMOKE_AGENT_ENV, "claude-acp"),
                (ci, "true"),
            ]);
            assert!(matches!(refused.smoke, Some(Err(_))), "{ci}");
        }
        let data = smoke_data();
        let local = options(&[
            (SMOKE_ENV, "chat"),
            (SMOKE_FOLDER_ENV, &folder),
            (SMOKE_AGENT_ENV, "claude-acp"),
            ("CI", "false"),
            (DATA_DIR_ENV, &data),
        ]);
        assert_eq!(
            local.smoke_plan().map(|plan| plan.scenario),
            Some(SmokeScenario::Agent)
        );
    }

    #[test]
    fn a_smoke_run_that_cannot_run_still_counts_as_smoke() {
        let broken = options(&[
            (SMOKE_ENV, "chat"),
            (SMOKE_EXIT_ENV, "1"),
            (STRESS_ENV, "orbs=3"),
        ]);
        assert!(broken.is_smoke());
        assert!(broken.smoke_exit, "it exits after reporting its failure");
        assert!(!broken.single_instance());
        assert_eq!(broken.smoke_plan(), None);
        assert_eq!(
            broken.mode(),
            LaunchMode::Normal,
            "neither a scenario nor the stress view"
        );
        assert_eq!(
            broken.notices(),
            vec![
                "GHOST_SMOKE was ignored: GHOST_SMOKE=chat needs GHOST_SMOKE_FOLDER: an absolute path to the folder its chat opens in."
                    .to_string()
            ]
        );
    }

    #[test]
    fn the_folder_picker_answers_only_a_chat_scenario_folder() {
        let folder = absolute("picked");
        let data = smoke_data();
        let chat = options(&[
            (SMOKE_ENV, "chat"),
            (SMOKE_FOLDER_ENV, &folder),
            (DATA_DIR_ENV, &data),
        ]);
        assert_eq!(chat.smoke_folder(), Some(folder.as_str()));
        let stream = options(&[
            (SMOKE_ENV, "1"),
            (SMOKE_FOLDER_ENV, &folder),
            (DATA_DIR_ENV, &data),
        ]);
        assert_eq!(stream.smoke_folder(), None);
        let plain = options(&[(SMOKE_FOLDER_ENV, &folder)]);
        assert_eq!(plain.smoke_folder(), None);
    }

    #[test]
    fn the_demo_agent_is_in_the_catalog() {
        let spec = ghost_agents::get(DEMO_AGENT).expect("the demo agent");
        assert_eq!(spec.kind, ghost_protocol::DriverKind::Mock);
    }

    #[test]
    fn stress_mode_carries_its_config() {
        let stress = options(&[(STRESS_ENV, "orbs=12,rate=400,aura=0,seconds=60")]);
        assert_eq!(
            stress.mode(),
            LaunchMode::Stress {
                config: config(12, 400, false, 60)
            }
        );
        assert!(stress.notices().is_empty());
    }

    #[test]
    fn a_blank_or_off_stress_variable_is_ignored() {
        for value in ["  ", "0", "off", "FALSE"] {
            let off = options(&[(STRESS_ENV, value)]);
            assert_eq!(off.stress, None, "{value:?}");
            assert_eq!(off.mode(), LaunchMode::Normal);
            assert!(off.notices().is_empty());
        }
    }

    /// Every launch registered the single-instance plugin, so a second one with its own data folder
    /// focused the running window and exited before opening that folder.
    #[test]
    fn a_launch_with_its_own_data_folder_is_its_own_instance() {
        let plain = options(&[]);
        assert!(!plain.own_data_dir);
        assert!(
            plain.single_instance(),
            "a plain launch hands over to a running one"
        );

        // "0" is not a flag here: ghost-core reads it as a folder named 0.
        for value in [
            "/tmp/second instance",
            " relative ",
            r"D:\portable\data",
            "0",
        ] {
            let own = options(&[(DATA_DIR_ENV, value)]);
            assert!(own.own_data_dir, "{value:?}");
            assert!(!own.single_instance(), "{value:?}");
            assert!(own.notices().is_empty(), "nothing to report: {value:?}");
        }
        // ghost-core ignores these, so the launch opens the default folder like any other.
        for value in ["", "   ", "\t\n"] {
            let blank = options(&[(DATA_DIR_ENV, value)]);
            assert!(!blank.own_data_dir, "{value:?}");
            assert!(blank.single_instance(), "{value:?}");
        }

        assert!(
            !options(&[(SMOKE_ENV, "1")]).single_instance(),
            "a smoke run"
        );
        let both = options(&[(SMOKE_ENV, "1"), (DATA_DIR_ENV, "/tmp/smoke data")]);
        assert!(!both.single_instance());
    }

    /// A first launch on a throwaway data folder kept its webview profile in the installed app's
    /// folder and restored or saved the installed app's window geometry.
    #[test]
    fn a_launch_with_its_own_data_folder_keeps_webview_and_window_state_there() {
        let root = Path::new("scratch").join("data");
        let own = options(&[(DATA_DIR_ENV, "scratch/data")]);
        assert_eq!(
            own.webview_data_dir(Some(&root)),
            Some(root.join(WEBVIEW_DATA_FOLDER))
        );
        assert!(!own.remembers_window_geometry());
        assert_eq!(own.webview_data_dir(None), None, "no data folder at all");

        let default = Path::new("local").join(ghost_core::paths::APP_IDENTIFIER);
        for plain in [options(&[]), options(&[(SMOKE_ENV, "1")])] {
            assert_eq!(
                plain.webview_data_dir(Some(&default)),
                None,
                "Tauri's default"
            );
            assert!(plain.remembers_window_geometry());
        }
    }

    /// A build from a working copy shared the installed app's instance and data: launched beside
    /// it, it handed over and exited; launched alone, it opened (and with a newer schema migrated)
    /// the installed app's chats; its cleanup and quit request reached the installed app.
    #[test]
    fn a_development_build_is_always_its_own_instance() {
        let root = Path::new("scratch").join("data");
        for vars in [
            &[][..],
            &[(DATA_DIR_ENV, "")][..],
            &[(DATA_DIR_ENV, "   ")][..],
            &[(DATA_DIR_ENV, "scratch/data")][..],
        ] {
            let development = development_options(vars);
            assert!(development.own_data_dir, "{vars:?}");
            assert!(!development.single_instance(), "{vars:?}");
            assert!(!development.remembers_window_geometry(), "{vars:?}");
            assert_eq!(
                development.webview_data_dir(Some(&root)),
                Some(root.join(WEBVIEW_DATA_FOLDER)),
                "{vars:?}"
            );
            assert!(development.notices().is_empty(), "{vars:?}");
        }
        // An installed build keeps the hand-over and the default webview profile.
        let installed = options(&[]);
        assert!(!installed.own_data_dir);
        assert!(installed.single_instance());
        let default = Path::new("local").join(ghost_core::paths::APP_IDENTIFIER);
        assert_eq!(installed.webview_data_dir(Some(&default)), None);
        // In a folder an earlier name used, WebView2 is given that folder, as Tauri gave it then:
        // the profile is the one from then, in its `EBWebView`, not a new one one level down.
        for legacy in ghost_core::paths::LEGACY_IDENTIFIERS {
            let legacy = Path::new("local").join(legacy);
            assert_eq!(
                installed.webview_data_dir(Some(&legacy)),
                Some(legacy.clone())
            );
        }
        // The override's earlier names are a folder of the launch's own too.
        for name in ghost_core::paths::LEGACY_DATA_DIR_ENVS {
            let named = options(&[(name, "scratch/data")]);
            assert!(named.own_data_dir, "{name}");
            assert!(!named.single_instance(), "{name}");
        }
    }

    /// A smoke run skips the hand-over, so without a folder of its own it opened the default
    /// one beside the running app: the installed app's chats, for an installed build.
    #[test]
    fn a_smoke_run_needs_its_own_data_folder() {
        let folder = absolute("smoke chat");
        let data = smoke_data();
        for development in [false, true] {
            for (scenario, extra) in [
                (SmokeScenario::Stream, None),
                (SmokeScenario::Chat, Some("chat")),
                (SmokeScenario::ChatReopen, Some("chat-reopen")),
            ] {
                let value = extra.unwrap_or("1");
                for blank in [None, Some(""), Some("  ")] {
                    let mut vars = vec![
                        (SMOKE_ENV, value),
                        (SMOKE_EXIT_ENV, "1"),
                        (SMOKE_FOLDER_ENV, folder.as_str()),
                    ];
                    if let Some(blank) = blank {
                        vars.push((DATA_DIR_ENV, blank));
                    }
                    let refused = options_in(development, &vars);
                    assert_eq!(
                        refused.smoke,
                        Some(Err(setup_error(Some(scenario), SMOKE_NEEDS_DATA_DIR))),
                        "{development} {value} {blank:?}"
                    );
                    assert!(refused.is_smoke() && refused.smoke_exit);
                    assert!(!refused.single_instance(), "never hands over either");
                    assert_eq!(refused.mode(), LaunchMode::Normal);
                    assert_eq!(
                        refused.notices(),
                        vec![format!("GHOST_SMOKE was ignored: {SMOKE_NEEDS_DATA_DIR}")]
                    );
                }
                let runs = options_in(
                    development,
                    &[
                        (SMOKE_ENV, value),
                        (SMOKE_FOLDER_ENV, &folder),
                        (DATA_DIR_ENV, &data),
                    ],
                );
                assert_eq!(
                    runs.smoke_plan().map(|plan| plan.scenario),
                    Some(scenario),
                    "{development} {value}"
                );
            }
        }
        // A switch that cannot run says its own reason first.
        assert_eq!(
            options(&[(SMOKE_ENV, "chat")]).notices(),
            vec![
                "GHOST_SMOKE was ignored: GHOST_SMOKE=chat needs GHOST_SMOKE_FOLDER: an absolute path to the folder its chat opens in."
                    .to_string()
            ]
        );
    }

    /// `prefix` followed by a folder name that is not valid Unicode, which the OS allows in a
    /// variable: invalid UTF-8 on Unix, a lone surrogate on Windows.
    #[cfg(unix)]
    fn not_unicode(prefix: &OsStr) -> OsString {
        use std::os::unix::ffi::OsStrExt;
        let mut value = prefix.to_os_string();
        value.push(OsStr::from_bytes(b"/caf\xe9"));
        value
    }

    #[cfg(windows)]
    fn not_unicode(prefix: &OsStr) -> OsString {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let mut wide: Vec<u16> = prefix.encode_wide().collect();
        wide.extend(r"\caf".encode_utf16());
        wide.push(0xD800);
        OsString::from_wide(&wide)
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_data_folder_that_is_not_unicode_still_counts() {
        let folder = not_unicode(OsStr::new("data"));
        assert_eq!(folder.to_str(), None);
        // An installed build's options: a development build is its own instance anyway.
        let own = LaunchOptions::from_lookup_in(false, |name| {
            (name == DATA_DIR_ENV).then(|| folder.clone())
        });
        assert!(own.own_data_dir, "ghost-core moves the data there");
        assert!(!own.single_instance());

        // The other switches read such a value as unset, as they always have.
        let garbled = LaunchOptions::from_lookup_in(false, |name| {
            (name != DATA_DIR_ENV && !ghost_core::paths::LEGACY_DATA_DIR_ENVS.contains(&name))
                .then(|| not_unicode(OsStr::new("1")))
        });
        assert!(garbled.smoke.is_none() && !garbled.no_glass && !garbled.own_data_dir);
        assert_eq!(garbled.stress, None);
    }

    /// Marks a re-executed copy of the test binary (see the next test).
    const CHILD_CHECKS_DATA_DIR: &str = "GHOST_DESKTOP_TEST_CHECK_DATA_DIR";

    /// ghost-core keeps its rule for `GHOST_DATA_DIR` private and `moves_data_root` restates it.
    /// If the two drifted apart, a launch on the default folder would skip the single-instance
    /// plugin, or one with its own folder would hand over and exit. So for each value a child
    /// process (this process's environment must not change) checks that `own_data_dir` holds
    /// exactly when ghost-core resolves a folder other than the installed app's, the only one
    /// named `dev.onghost.desktop`. The test binary is a development build, so that is every
    /// value, blank ones included. Resolving only computes the path; nothing is created.
    #[test]
    fn own_data_dir_agrees_with_where_aura_core_puts_the_data() {
        if std::env::var_os(CHILD_CHECKS_DATA_DIR).is_some() {
            let root = ghost_core::paths::DataRoot::resolve().expect("resolve the data folder");
            let default = root.root().ends_with(ghost_core::paths::APP_IDENTIFIER);
            assert_eq!(
                LaunchOptions::from_env().own_data_dir,
                !default,
                "{}",
                root.root().display()
            );
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().join("own data").into_os_string();
        let mut padded = OsString::from("  ");
        padded.push(&folder);
        padded.push(" \t");
        let mut values = vec![
            OsString::new(),
            OsString::from("   "),
            OsString::from("\t\n"),
            OsString::from("0"),
            folder,
            padded,
        ];
        #[cfg(any(unix, windows))]
        values.push(not_unicode(dir.path().as_os_str()));
        for value in values {
            crate::test_support::run_child(
                "launch::tests::own_data_dir_agrees_with_where_aura_core_puts_the_data",
                |child| {
                    child
                        .env(CHILD_CHECKS_DATA_DIR, "1")
                        .env(DATA_DIR_ENV, &value);
                },
            );
        }
    }

    #[test]
    fn no_glass_is_reported_as_a_notice() {
        let opaque = options(&[(NO_GLASS_ENV, "1")]);
        assert!(opaque.no_glass);
        assert_eq!(
            opaque.notices(),
            vec!["Window glass is off because GHOST_NO_GLASS is set.".to_string()]
        );
    }

    #[test]
    fn launch_mode_serializes_with_a_type_tag() {
        assert_eq!(
            serde_json::to_value(LaunchMode::Stress {
                config: config(2, 50, true, 0)
            })
            .expect("serialize"),
            serde_json::json!({
                "type": "stress",
                "config": { "orbs": 2, "rate": 50, "aura": true, "seconds": 0 }
            })
        );
        assert_eq!(
            serde_json::to_value(LaunchMode::Normal).expect("serialize"),
            serde_json::json!({ "type": "normal" })
        );
        assert_eq!(
            serde_json::to_value(LaunchMode::Smoke {
                plan: SmokePlan {
                    scenario: SmokeScenario::ChatReopen,
                    agent_id: Some(DEMO_AGENT.into()),
                    folder: Some("/work".into()),
                }
            })
            .expect("serialize"),
            serde_json::json!({
                "type": "smoke",
                "plan": { "scenario": "chat_reopen", "agent_id": "mock", "folder": "/work" }
            })
        );
    }
}
