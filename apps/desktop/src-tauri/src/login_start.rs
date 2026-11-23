//! Opening o7 when the person signs in to the computer (Settings > Remote access), so a phone or
//! a bot can reach it after a restart without anyone opening it by hand. Off unless the person
//! turns it on.
//!
//! The operating system's own record is the only one: a value under Windows' `Run` key, a launch
//! agent on macOS, an autostart entry on Linux. o7 keeps no copy, so a start turned off elsewhere
//! (Windows' Startup apps, a removed file) reads as off here, and an entry that opens another
//! copy of o7 (an older install somewhere else) is not this one's. The start passes
//! [`BACKGROUND_ARG`]: the window stays hidden until the tray icon, the Dock or another launch
//! opens it.
//!
//! A development build keeps an entry of its own under another name, so trying it never touches
//! the installed app's. A launch with its own data folder (`O7_DATA_DIR`) cannot have one: the
//! start at sign-in would open the usual folder instead.

use std::path::{Path, PathBuf};

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::{
    error::{AppError, AppErrorCode},
    state::AppState,
};

/// What a start at sign-in passes, so the window stays hidden.
pub const BACKGROUND_ARG: &str = "--background";

/// The entry's name: the installed app's, or a development build's own.
const NAME: &str = if cfg!(debug_assertions) {
    "o7 (development)"
} else {
    "o7"
};

/// Whether this launch asked to start in the background. `args` are the program's, its own
/// path first.
pub fn in_background(args: &[String]) -> bool {
    args.iter().skip(1).any(|arg| arg == BACKGROUND_ARG)
}

/// Whether this copy of o7 opens when the person signs in, for Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct LoginStart {
    pub on: bool,
    /// Why it cannot be switched in this copy, when it cannot.
    pub unavailable: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub fn login_start_status(state: State<'_, AppState>) -> LoginStart {
    status(&state)
}

/// Opens this copy of o7 at sign-in, in the background, or stops.
#[tauri::command]
#[specta::specta]
pub fn login_start_set(state: State<'_, AppState>, on: bool) -> Result<LoginStart, AppError> {
    if let Some(reason) = unavailable(&state) {
        return Err(AppError::invalid_argument(reason));
    }
    let failed = |error: std::io::Error| {
        tracing::warn!(%error, on, "could not change the start at sign-in");
        AppError::new(
            AppErrorCode::StorageUnavailable,
            format!("The system did not take the change: {error}"),
        )
    };
    platform::set(on, &program().map_err(failed)?).map_err(failed)?;
    Ok(status(&state))
}

fn status(state: &AppState) -> LoginStart {
    let unavailable = unavailable(state);
    LoginStart {
        on: unavailable.is_none() && program().is_ok_and(|program| platform::enabled(&program)),
        unavailable,
    }
}

fn unavailable(state: &AppState) -> Option<String> {
    if state.launch.is_smoke() {
        return Some("Not in a test run.".into());
    }
    if state.launch.names_data_dir {
        return Some(
            "This copy of o7 opens a data folder of its own, which a start at sign-in would not."
                .into(),
        );
    }
    if !cfg!(any(windows, target_os = "macos", target_os = "linux")) {
        return Some("Not on this system.".into());
    }
    None
}

/// The program a start at sign-in runs: this one, or on Linux the AppImage it came from (the
/// program itself is inside the image, at a place that changes every run).
fn program() -> std::io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(image) = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|image| image.is_absolute())
    {
        return Ok(image);
    }
    std::env::current_exe()
}

fn unwritable() -> std::io::Error {
    std::io::Error::other("the program's path has characters a start at sign-in cannot hold")
}

/// The command under Windows' `Run` key.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_command(program: &Path) -> std::io::Result<String> {
    let path = program.to_str().filter(|path| !path.contains('"'));
    let path = path.ok_or_else(unwritable)?;
    Ok(format!("\"{path}\" {BACKGROUND_ARG}"))
}

/// Whether a `Run` command starts `program`: its first word, quoted or not, names it. Windows'
/// paths ignore case.
#[cfg_attr(not(windows), allow(dead_code))]
fn starts(command: &str, program: &Path) -> bool {
    let command = command.trim_start();
    let first = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => command.split(' ').next().unwrap_or_default(),
    };
    program
        .to_str()
        .is_some_and(|program| first.to_lowercase() == program.to_lowercase())
}

/// A macOS launch agent that opens `program` at sign-in.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launch_agent(label: &str, program: &Path) -> std::io::Result<String> {
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{}</string>
	<key>ProgramArguments</key>
	<array>
		{}
		<string>{BACKGROUND_ARG}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#,
        xml(label),
        plist_program(program)?
    ))
}

/// The line of a launch agent that names the program, which also tells it is this copy's.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn plist_program(program: &Path) -> std::io::Result<String> {
    let path = program
        .to_str()
        .filter(|path| !path.chars().any(char::is_control));
    Ok(format!(
        "<string>{}</string>",
        xml(path.ok_or_else(unwritable)?)
    ))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// An XDG autostart entry that opens `program` at sign-in.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn desktop_entry(program: &Path) -> std::io::Result<String> {
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName={NAME}\nComment=Opens o7 in the background when you sign in\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        desktop_exec(program)?
    ))
}

/// The entry's `Exec` value: the program quoted as the Desktop Entry Specification asks. Inside
/// quotes `"`, `` ` ``, `$` and `\` take a backslash, and the file's own escaping doubles every
/// backslash again; `%` starts a field code, so it is doubled.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn desktop_exec(program: &Path) -> std::io::Result<String> {
    let path = program
        .to_str()
        .filter(|path| !path.chars().any(char::is_control));
    let mut quoted = String::from("\"");
    for c in path.ok_or_else(unwritable)?.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    Ok(format!("{quoted} {BACKGROUND_ARG}"))
}

#[cfg(windows)]
mod platform {
    use std::path::Path;

    use windows_registry::CURRENT_USER;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    /// Windows' Startup apps switch for each `Run` value: an odd first byte when the person turned
    /// the start off there. A missing one is on.
    const APPROVED: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    pub fn enabled(program: &Path) -> bool {
        let Ok(command) = CURRENT_USER
            .open(RUN)
            .and_then(|key| key.get_string(super::NAME))
        else {
            return false;
        };
        let off = CURRENT_USER
            .open(APPROVED)
            .and_then(|key| key.get_value(super::NAME))
            .is_ok_and(|value| value.first().is_some_and(|byte| byte & 1 == 1));
        super::starts(&command, program) && !off
    }

    pub fn set(on: bool, program: &Path) -> std::io::Result<()> {
        if on {
            let command = super::windows_command(program)?;
            CURRENT_USER
                .create(RUN)
                .and_then(|key| key.set_string(super::NAME, command))
                .map_err(io)?;
        } else {
            remove(RUN)?;
        }
        // Turned on here after Startup apps turned it off: on again. Turned off: nothing left.
        remove(APPROVED)
    }

    fn remove(path: &str) -> std::io::Result<()> {
        let removed = CURRENT_USER
            .options()
            .read()
            .write()
            .open(path)
            .and_then(|key| key.remove_value(super::NAME));
        match removed {
            Ok(()) => Ok(()),
            // ERROR_FILE_NOT_FOUND: no such key or value, so nothing to remove.
            Err(error) if error.code().0 == 0x8007_0002_u32 as i32 => Ok(()),
            Err(error) => Err(io(error)),
        }
    }

    fn io(error: impl std::fmt::Display) -> std::io::Error {
        std::io::Error::other(error.to_string())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::{Path, PathBuf};

    const LABEL: &str = if cfg!(debug_assertions) {
        "codes.o7.development.open-at-sign-in"
    } else {
        "codes.o7.open-at-sign-in"
    };

    fn file() -> std::io::Result<PathBuf> {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .ok_or_else(|| std::io::Error::other("there is no home folder"))?;
        Ok(PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    pub fn enabled(program: &Path) -> bool {
        let (Ok(file), Ok(line)) = (file(), super::plist_program(program)) else {
            return false;
        };
        std::fs::read_to_string(file).is_ok_and(|text| text.contains(&line))
    }

    pub fn set(on: bool, program: &Path) -> std::io::Result<()> {
        let file = file()?;
        if !on {
            return match std::fs::remove_file(&file) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            };
        }
        let text = super::launch_agent(LABEL, program)?;
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(&file, text)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::{Path, PathBuf};

    const FILE: &str = if cfg!(debug_assertions) {
        "o7-development.desktop"
    } else {
        "o7.desktop"
    };

    /// `$XDG_CONFIG_HOME/autostart`, by default `~/.config/autostart`.
    fn folder() -> std::io::Result<PathBuf> {
        if let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|config| config.is_absolute())
        {
            return Ok(config.join("autostart"));
        }
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .ok_or_else(|| std::io::Error::other("there is no home folder"))?;
        Ok(PathBuf::from(home).join(".config/autostart"))
    }

    pub fn enabled(program: &Path) -> bool {
        folder().is_ok_and(|folder| enabled_in(&folder, program))
    }

    pub fn set(on: bool, program: &Path) -> std::io::Result<()> {
        set_in(&folder()?, on, program)
    }

    /// An entry of this copy's that the desktop has not been told to skip.
    pub(super) fn enabled_in(folder: &Path, program: &Path) -> bool {
        let (Ok(text), Ok(exec)) = (
            std::fs::read_to_string(folder.join(FILE)),
            super::desktop_exec(program),
        ) else {
            return false;
        };
        let exec = format!("Exec={exec}");
        text.lines().any(|line| line.trim() == exec)
            && !text.lines().any(|line| {
                matches!(
                    line.trim(),
                    "Hidden=true" | "X-GNOME-Autostart-enabled=false"
                )
            })
    }

    pub(super) fn set_in(folder: &Path, on: bool, program: &Path) -> std::io::Result<()> {
        let file = folder.join(FILE);
        if !on {
            return match std::fs::remove_file(&file) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            };
        }
        let text = super::desktop_entry(program)?;
        std::fs::create_dir_all(folder)?;
        std::fs::write(&file, text)
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod platform {
    pub fn enabled(_program: &std::path::Path) -> bool {
        false
    }

    pub fn set(_on: bool, _program: &std::path::Path) -> std::io::Result<()> {
        Err(std::io::Error::other("not on this system"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn a_start_in_the_background_is_asked_for_by_its_argument_only() {
        assert!(in_background(&args(&["o7", "--background"])));
        assert!(!in_background(&args(&["o7"])));
        assert!(
            !in_background(&args(&["--background"])),
            "the program's own path is not an argument"
        );
    }

    #[test]
    fn the_windows_command_quotes_the_program_and_is_found_again() {
        let program = Path::new(r"C:\Users\Ann Lee\AppData\Local\o7\o7.exe");
        let command = windows_command(program).expect("command");
        assert_eq!(
            command,
            r#""C:\Users\Ann Lee\AppData\Local\o7\o7.exe" --background"#
        );
        assert!(starts(&command, program));
        assert!(starts(
            r#""c:\users\ann lee\appdata\local\o7\O7.EXE" --background"#,
            program
        ));
        assert!(starts(
            r"C:\o7\o7.exe --background",
            Path::new(r"C:\o7\o7.exe")
        ));
        assert!(
            !starts(r#""C:\Old\o7.exe" --background"#, program),
            "another copy's"
        );
        assert!(!starts("", program));
    }

    #[test]
    fn a_launch_agent_names_the_program_escaped() {
        let program = Path::new("/Applications/o7 & co.app/Contents/MacOS/o7");
        let text = launch_agent("codes.o7.open-at-sign-in", program).expect("agent");
        assert!(text.contains("<string>codes.o7.open-at-sign-in</string>"));
        assert!(text.contains("<string>/Applications/o7 &amp; co.app/Contents/MacOS/o7</string>"));
        assert!(text.contains(&plist_program(program).expect("line")));
        assert!(text.contains("<string>--background</string>"));
        assert!(text.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(plist_program(Path::new("/tmp/o7\n")).is_err());
    }

    #[test]
    fn an_autostart_entry_quotes_the_program_as_the_spec_asks() {
        assert_eq!(
            desktop_exec(Path::new("/opt/o7/o7")).expect("exec"),
            "\"/opt/o7/o7\" --background"
        );
        assert_eq!(
            desktop_exec(Path::new("/home/a/$x \"q\" 100%/o7\\b")).expect("exec"),
            "\"/home/a/\\\\$x \\\\\"q\\\\\" 100%%/o7\\\\\\\\b\" --background"
        );
        assert!(desktop_exec(Path::new("/tmp/o7\nExec=evil")).is_err());
        let entry = desktop_entry(Path::new("/opt/o7/o7")).expect("entry");
        assert!(entry.starts_with("[Desktop Entry]\nType=Application\n"));
        assert!(entry.contains("\nExec=\"/opt/o7/o7\" --background\n"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_autostart_entry_is_written_read_and_removed() {
        let folder = tempfile::tempdir().expect("folder");
        let autostart = folder.path().join("autostart");
        let program = Path::new("/opt/o7/o7");
        assert!(!platform::enabled_in(&autostart, program));
        platform::set_in(&autostart, true, program).expect("on");
        assert!(platform::enabled_in(&autostart, program));
        assert!(
            !platform::enabled_in(&autostart, Path::new("/usr/bin/o7")),
            "another copy's entry is not this one's"
        );
        let file = std::fs::read_dir(&autostart)
            .expect("listed")
            .next()
            .expect("one")
            .expect("entry")
            .path();
        let text = std::fs::read_to_string(&file).expect("read");
        std::fs::write(&file, format!("{text}Hidden=true\n")).expect("hide");
        assert!(
            !platform::enabled_in(&autostart, program),
            "the desktop was told to skip it"
        );
        platform::set_in(&autostart, false, program).expect("off");
        assert!(!file.exists());
        platform::set_in(&autostart, false, program).expect("off again is fine");
    }
}
