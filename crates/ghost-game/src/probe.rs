//! Whether another engine's own agent tools are within reach on this computer: Unreal's MCP
//! server answering on its port, Unity's CLI installed, Roblox Studio's MCP launcher in place.
//!
//! Ghost does not run these engines. Their makers' tools drive their editors, and the pane's
//! Connect card says how to turn them on; this is the card's one live line. Each look is cheap
//! and changes nothing: one loopback connection that is closed at once, or a file looked for.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::detect::GameEngine;

/// Where Unreal's MCP server listens unless the person changed it in Editor Preferences.
pub const UNREAL_MCP: &str = "http://127.0.0.1:8000/mcp";
const UNREAL_PORT: u16 = 8000;

/// What the look found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct EngineProbe {
    pub engine: GameEngine,
    /// The engine's agent tools can be reached, or are installed, now.
    pub ready: bool,
    /// What was found, in words for the person.
    pub detail: String,
}

/// Roblox Studio's MCP launcher, where Roblox's own guide says it is.
pub fn roblox_launcher() -> Option<PathBuf> {
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA")?;
        Some(Path::new(&local).join("Roblox").join("mcp.bat"))
    } else if cfg!(target_os = "macos") {
        Some(PathBuf::from(
            "/Applications/RobloxStudio.app/Contents/MacOS/StudioMCP",
        ))
    } else {
        None
    }
}

/// Whether a program named `name` is in one of the folders of `PATH`, without running anything.
fn on_path(name: &str, path: Option<std::ffi::OsString>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let endings: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat", ""]
    } else {
        &[""]
    };
    std::env::split_paths(&path).take(256).any(|folder| {
        endings
            .iter()
            .any(|ending| folder.join(format!("{name}{ending}")).is_file())
    })
}

/// Looks for `engine`'s agent tools. Engines Ghost runs itself, and ones with no such tools,
/// answer not ready with nothing to say.
pub async fn probe(engine: GameEngine) -> EngineProbe {
    let (ready, detail) = match engine {
        GameEngine::Unreal => {
            let reached = tokio::time::timeout(
                Duration::from_millis(600),
                tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, UNREAL_PORT)),
            )
            .await
            .is_ok_and(|connected| connected.is_ok());
            if reached {
                (
                    true,
                    format!(
                        "Something answers on port {UNREAL_PORT}, where Unreal's MCP server listens."
                    ),
                )
            } else {
                (
                    false,
                    format!(
                        "Nothing answers on port {UNREAL_PORT}: the Unreal editor is not running, or its MCP server is off."
                    ),
                )
            }
        }
        GameEngine::Unity => {
            let installed =
                tokio::task::spawn_blocking(|| on_path("unity", std::env::var_os("PATH")))
                    .await
                    .unwrap_or(false);
            if installed {
                (true, "The Unity CLI is installed.".to_owned())
            } else {
                (
                    false,
                    "The Unity CLI is not on this computer yet.".to_owned(),
                )
            }
        }
        GameEngine::Roblox => match roblox_launcher() {
            None => (false, "Roblox Studio runs on Windows and macOS.".to_owned()),
            Some(launcher) => {
                let there = tokio::task::spawn_blocking(move || launcher.is_file())
                    .await
                    .unwrap_or(false);
                if there {
                    (
                        true,
                        "Roblox Studio's MCP launcher is on this computer.".to_owned(),
                    )
                } else {
                    (
                        false,
                        "Roblox Studio's MCP launcher is not here: install or update Roblox Studio."
                            .to_owned(),
                    )
                }
            }
        },
        _ => (false, String::new()),
    };
    EngineProbe {
        engine,
        ready,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_found_on_the_path_without_being_run() {
        let scratch = tempfile::tempdir().unwrap();
        let (with, without) = (scratch.path().join("bin"), scratch.path().join("other"));
        std::fs::create_dir_all(&with).unwrap();
        std::fs::create_dir_all(&without).unwrap();
        let name = if cfg!(windows) { "unity.cmd" } else { "unity" };
        // Not a program at all: finding it must not depend on running it.
        std::fs::write(with.join(name), "not runnable").unwrap();
        // A folder of that name is not a program.
        std::fs::create_dir(without.join("unity")).unwrap();
        let path = |folders: &[&Path]| Some(std::env::join_paths(folders).unwrap());
        assert!(on_path("unity", path(&[&without, &with])));
        assert!(!on_path("unity", path(&[&without])));
        assert!(!on_path("unity", None));
        assert!(!on_path("unity", Some(std::ffi::OsString::new())));
    }

    #[tokio::test]
    async fn unreal_is_ready_only_while_something_listens_on_its_port() {
        // Whatever this machine runs, the answer names the port and matches what a connection finds.
        let found = probe(GameEngine::Unreal).await;
        let listening =
            std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, UNREAL_PORT)).is_ok();
        assert_eq!(found.ready, listening);
        assert!(found.detail.contains("8000"));
        // Engines Ghost runs itself have no such tools to look for.
        for engine in [GameEngine::Godot, GameEngine::Web, GameEngine::Bevy] {
            let found = probe(engine).await;
            assert!(!found.ready && found.detail.is_empty(), "{engine:?}");
        }
    }
}
