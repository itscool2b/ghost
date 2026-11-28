//! Starting a Godot game for Ghost: the engine on the project's folder with Ghost's own main loop,
//! out of sight, in a job object (Windows) or a process group (Unix) so that it ends with Ghost.
//!
//! Godot is started with its arguments apart, never through a shell. The loop and bridge scripts
//! are Ghost's own, written to its folder from this build: nothing is written into the project.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::Error;

const LOOP: &str = include_str!("../resources/godot/loop.gd");
const BRIDGE: &str = include_str!("../resources/godot/bridge.gd");
const LOGGER: &str = include_str!("../resources/godot/logger.gd");
const CHECK: &str = include_str!("../resources/godot/check.gd");

/// Writes Ghost's scripts for a Godot game into `root` and answers the loop's path. They are
/// written again on every start, so what runs is always this build's.
pub fn write_scripts(root: &Path) -> std::io::Result<PathBuf> {
    let folder = root.join("godot").join("bridge");
    std::fs::create_dir_all(&folder)?;
    for (name, text) in [
        ("loop.gd", LOOP),
        ("bridge.gd", BRIDGE),
        ("logger.gd", LOGGER),
        ("check.gd", CHECK),
    ] {
        std::fs::write(folder.join(name), text)?;
    }
    Ok(folder.join("loop.gd"))
}

/// A scene a run may start on: a path inside the project, to a scene file, with nothing odd in
/// it. Godot reads it from an environment variable, never from the command line.
pub fn checked_scene(scene: &str) -> Result<&str, Error> {
    let fine = scene.starts_with("res://")
        && (scene.ends_with(".tscn") || scene.ends_with(".scn"))
        && scene.len() <= 300
        && !scene.contains("..")
        && scene.chars().all(|ch| !ch.is_control());
    if fine {
        Ok(scene)
    } else {
        Err(Error::Refused(
            "A scene to run is a path inside the project, like res://levels/one.tscn.".into(),
        ))
    }
}

/// Everything one start needs.
pub struct Launch<'a> {
    pub godot: &'a Path,
    pub project: &'a Path,
    /// Ghost's loop script ([`write_scripts`]).
    pub main_loop: &'a Path,
    pub bridge_address: &'a str,
    pub token: &'a str,
    pub scene: Option<&'a str>,
    /// Ghost's own window, which the game's window is owned by so that it has no taskbar entry
    /// and can sit off-screen. Without one the game's window shows as any window does.
    pub owner_window: Option<i64>,
    /// What the game's random numbers start from. Ghost names it so that a run can be played
    /// again with the same numbers.
    pub seed: u32,
    /// Start standing still, before the first step: a run that is about to be given a replay.
    pub held: bool,
}

/// The arguments Godot is started with.
pub fn arguments(launch: &Launch<'_>) -> Vec<std::ffi::OsString> {
    let mut arguments: Vec<std::ffi::OsString> = vec![
        "--path".into(),
        launch.project.into(),
        "-s".into(),
        launch.main_loop.into(),
        // Every frame is exactly one sixtieth of a second of game time, so a run can be stepped
        // and repeated exactly. Ghost's bridge keeps real time while a person plays.
        "--fixed-fps".into(),
        "60".into(),
        "--disable-vsync".into(),
    ];
    if let Some(window) = launch.owner_window.filter(|window| *window != 0) {
        arguments.extend([
            "--wid".into(),
            window.to_string().into(),
            "--position".into(),
            "30000,30000".into(),
        ]);
    }
    arguments
}

/// A started game.
pub struct Running {
    child: Box<dyn ChildWrapper>,
    /// Held from just before the start until the game has said hello ([`foreground`]).
    foreground: Option<foreground::Held>,
}

impl Running {
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Whether the process has ended, and how: its exit code where it has one.
    pub fn ended(&mut self) -> Option<Option<i32>> {
        match self.child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => Some(status.code()),
            Err(_) => Some(None),
        }
    }

    /// The game is up: it may no longer take the keyboard by starting, so the lock goes.
    pub fn settled(&mut self) {
        self.foreground = None;
    }

    /// Ends the game now. Its bridge was asked to quit first by whoever calls this.
    pub fn kill(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Starts the game. `on_line` hears each line it prints, from either stream.
pub fn start(
    launch: &Launch<'_>,
    on_line: impl Fn(String) + Send + Sync + Clone + 'static,
) -> Result<Running, Error> {
    let mut command = tokio::process::Command::new(launch.godot);
    command
        .args(arguments(launch))
        .env("GHOST_GAME_BRIDGE", launch.bridge_address)
        .env("GHOST_GAME_TOKEN", launch.token)
        .env("GHOST_GAME_SEED", launch.seed.to_string())
        .env_remove("GHOST_GAME_SCENE")
        .env_remove("GHOST_GAME_HOLD")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(scene) = launch.scene {
        command.env("GHOST_GAME_SCENE", checked_scene(scene)?);
    }
    if launch.held {
        command.env("GHOST_GAME_HOLD", "1");
    }
    let mut wrapped = CommandWrap::from(command);
    wrapped.wrap(KillOnDrop);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    {
        use process_wrap::tokio::{CreationFlags, JobObject};
        wrapped.wrap(CreationFlags(
            windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ));
        wrapped.wrap(JobObject);
    }
    // A starting game asks for the keyboard. While Ghost is in front it can say no for a moment.
    let lock = foreground::hold();
    let mut child = wrapped
        .spawn()
        .map_err(|error| Error::Game(format!("Godot could not be started: {error}")))?;
    if let Some(stdout) = child.stdout().take() {
        let heard = on_line.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                heard(line);
            }
        });
    }
    if let Some(stderr) = child.stderr().take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                on_line(line);
            }
        });
    }
    Ok(Running {
        child,
        foreground: Some(lock),
    })
}

/// Keeping a starting game from taking the keyboard (Windows). A game Godot starts calls
/// `SetForegroundWindow` on its own window; a process started by the one in front is allowed to.
/// While Ghost is that process it can lock the foreground, which it does from just before the
/// start until the game has said hello. Where Ghost is not in front the game is not allowed to
/// take the foreground anyway, and the lock is refused, which is fine.
pub mod foreground {
    /// The lock; letting go of it unlocks. It holds whether the lock was taken: off Windows, and
    /// where Ghost was not in front, it was not.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub struct Held(bool);

    #[cfg(windows)]
    #[allow(unsafe_code)] // Two calls that take a constant and touch no memory of ours.
    pub fn hold() -> Held {
        use windows::Win32::UI::WindowsAndMessaging::{LSFW_LOCK, LockSetForegroundWindow};
        // SAFETY: the call takes one constant and reads or writes nothing of this process's.
        Held(unsafe { LockSetForegroundWindow(LSFW_LOCK) }.is_ok())
    }

    #[cfg(windows)]
    #[allow(unsafe_code)]
    impl Drop for Held {
        fn drop(&mut self) {
            use windows::Win32::UI::WindowsAndMessaging::{LSFW_UNLOCK, LockSetForegroundWindow};
            if self.0 {
                // SAFETY: as above.
                let _ = unsafe { LockSetForegroundWindow(LSFW_UNLOCK) };
            }
        }
    }

    #[cfg(not(windows))]
    pub fn hold() -> Held {
        Held(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_is_started_with_ghosts_loop_at_a_fixed_step_and_out_of_sight_when_it_can_be() {
        let launch = |owner_window| Launch {
            godot: Path::new("/godot"),
            project: Path::new("/games/moon hop"),
            main_loop: Path::new("/ghost/game/godot/bridge/loop.gd"),
            bridge_address: "127.0.0.1:5000",
            token: "t",
            scene: None,
            owner_window,
            seed: 7,
            held: false,
        };
        let plain: Vec<String> = arguments(&launch(None))
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            plain,
            [
                "--path",
                "/games/moon hop",
                "-s",
                "/ghost/game/godot/bridge/loop.gd",
                "--fixed-fps",
                "60",
                "--disable-vsync"
            ],
            "a folder with a space is one argument"
        );
        let owned: Vec<String> = arguments(&launch(Some(4242)))
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(owned[7..], ["--wid", "4242", "--position", "30000,30000"]);
        assert_eq!(
            arguments(&launch(Some(0))).len(),
            7,
            "no window is no owner"
        );
    }

    #[test]
    fn a_scene_to_run_is_a_scene_inside_the_project() {
        for fine in [
            "res://main.tscn",
            "res://levels/one two.tscn",
            "res://a.scn",
        ] {
            assert_eq!(checked_scene(fine).unwrap(), fine);
        }
        for refused in [
            "main.tscn",
            "/etc/passwd",
            "C:/games/other/main.tscn",
            "res://../other/main.tscn",
            "res://main.gd",
            "res://main.tscn\n--quit",
            "",
        ] {
            assert!(checked_scene(refused).is_err(), "{refused:?}");
        }
        assert!(checked_scene(&format!("res://{}.tscn", "a".repeat(400))).is_err());
    }

    #[test]
    fn ghosts_scripts_are_written_whole_and_again_each_time() {
        let root = tempfile::tempdir().unwrap();
        let main_loop = write_scripts(root.path()).unwrap();
        assert_eq!(main_loop, root.path().join("godot/bridge/loop.gd"));
        let folder = main_loop.parent().unwrap();
        assert!(
            std::fs::read_to_string(&main_loop)
                .unwrap()
                .starts_with("# Ghost's main loop")
        );
        std::fs::write(folder.join("bridge.gd"), "tampered").unwrap();
        write_scripts(root.path()).unwrap();
        let bridge = std::fs::read_to_string(folder.join("bridge.gd")).unwrap();
        assert!(bridge.contains("extends Node") && bridge.contains("GHOST_GAME_TOKEN"));
        assert!(
            std::fs::read_to_string(folder.join("logger.gd"))
                .unwrap()
                .contains("extends Logger")
        );
        // The bridge never listens: it only connects out, to this computer.
        assert!(!bridge.contains("TCPServer") && bridge.contains("127.0.0.1"));
    }
}
