//! Starting a game from nothing: a small project Ghost wrote, put in a folder that holds no game
//! yet, so the first thing said to an agent can be "make it a platformer".
//!
//! It is the person's, from the pane: no tool starts one. Nothing that is already in the folder
//! is ever written over, and a folder that already holds a game is left as it is.

use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Error;

/// The projects Ghost can start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Starter {
    /// Godot, 2D: a player on a floor that runs and jumps.
    Godot2d,
    /// Godot, 3D: a player on a floor, a camera and a sun.
    Godot3d,
    /// A plain canvas page with no dependencies, and a small server for it.
    Web2d,
    /// Three.js with Vite.
    Web3d,
}

impl Starter {
    /// Its files, by their names at the top of the folder.
    fn files(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Starter::Godot2d => &[
                (
                    "project.godot",
                    include_str!("../resources/starters/godot-2d/project.godot"),
                ),
                (
                    "main.tscn",
                    include_str!("../resources/starters/godot-2d/main.tscn"),
                ),
                (
                    "player.gd",
                    include_str!("../resources/starters/godot-2d/player.gd"),
                ),
            ],
            Starter::Godot3d => &[
                (
                    "project.godot",
                    include_str!("../resources/starters/godot-3d/project.godot"),
                ),
                (
                    "main.tscn",
                    include_str!("../resources/starters/godot-3d/main.tscn"),
                ),
                (
                    "player.gd",
                    include_str!("../resources/starters/godot-3d/player.gd"),
                ),
            ],
            Starter::Web2d => &[
                (
                    "package.json",
                    include_str!("../resources/starters/web-2d/package.json"),
                ),
                (
                    "index.html",
                    include_str!("../resources/starters/web-2d/index.html"),
                ),
                (
                    "game.js",
                    include_str!("../resources/starters/web-2d/game.js"),
                ),
                (
                    "serve.mjs",
                    include_str!("../resources/starters/web-2d/serve.mjs"),
                ),
            ],
            Starter::Web3d => &[
                (
                    "package.json",
                    include_str!("../resources/starters/web-3d/package.json"),
                ),
                (
                    "index.html",
                    include_str!("../resources/starters/web-3d/index.html"),
                ),
                (
                    "main.js",
                    include_str!("../resources/starters/web-3d/main.js"),
                ),
            ],
        }
    }
}

/// The game's name, from its folder's: what a person reads, with nothing that would break the
/// file it is written into.
fn display_name(folder: &Path) -> String {
    let name: String = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|ch| !ch.is_control() && !matches!(ch, '"' | '\\' | '<' | '>' | '&'))
        .take(60)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "My Game".to_owned()
    } else {
        name.to_owned()
    }
}

/// The name as a package's: lowercase letters, digits and dashes.
fn package_name(display: &str) -> String {
    let mut name = String::new();
    for ch in display.chars() {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
        } else if !name.ends_with('-') && !name.is_empty() {
            name.push('-');
        }
    }
    let name = name.trim_matches('-');
    if name.is_empty() {
        "my-game".to_owned()
    } else {
        name.to_owned()
    }
}

/// Writes `starter` into `folder` and answers the names of the files it wrote. `godot` is the
/// Godot the project is marked as made with ("4.7"): the one that will run it.
///
/// A folder that already holds a game, or any file of the starter's name, is refused whole:
/// nothing is written over, and nothing is written at all when one file cannot be.
pub fn write(folder: &Path, starter: Starter, godot: &str) -> Result<Vec<String>, Error> {
    let is_folder = std::fs::symlink_metadata(folder).is_ok_and(|meta| meta.is_dir());
    if !is_folder {
        return Err(Error::Refused("The chat's folder is not there.".into()));
    }
    if let Some(project) = crate::detect(folder) {
        return Err(Error::Refused(format!(
            "This folder already holds a {} project.",
            project.engine.name()
        )));
    }
    let files = starter.files();
    if let Some((name, _)) = files
        .iter()
        .find(|(name, _)| std::fs::symlink_metadata(folder.join(name)).is_ok())
    {
        return Err(Error::Refused(format!(
            "This folder already has a file named {name}. Ghost does not write over files: start the game in an empty folder."
        )));
    }
    let name = display_name(folder);
    let package = package_name(&name);
    let version: String = godot
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .take(8)
        .collect();
    let mut written = Vec::new();
    for (file, text) in files {
        let text = text
            .replace("__NAME__", &name)
            .replace("__PACKAGE__", &package)
            .replace("__GODOT__", &version);
        // `create_new`: a file that appeared since the look above is not written over either.
        let result = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(folder.join(file))
            .and_then(|mut out| out.write_all(text.as_bytes()));
        if let Err(error) = result {
            for done in &written {
                let _ = std::fs::remove_file(folder.join(done));
            }
            return Err(Error::Io(error));
        }
        written.push((*file).to_owned());
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::GameEngine;

    #[test]
    fn each_starter_becomes_a_project_ghost_knows_named_after_its_folder() {
        for (starter, engine, files) in [
            (Starter::Godot2d, GameEngine::Godot, 3),
            (Starter::Godot3d, GameEngine::Godot, 3),
            (Starter::Web2d, GameEngine::Web, 4),
            (Starter::Web3d, GameEngine::Web, 3),
        ] {
            let scratch = tempfile::tempdir().unwrap();
            let folder = scratch.path().join("Moon Hop");
            std::fs::create_dir(&folder).unwrap();
            // What a new chat's folder may already hold is left alone.
            std::fs::write(folder.join("README.md"), "# notes").unwrap();
            let written = write(&folder, starter, "4.7").unwrap();
            assert_eq!(written.len(), files, "{starter:?}");
            let project = crate::detect(&folder).unwrap();
            assert_eq!(project.engine, engine, "{starter:?}");
            assert!(project.unsupported.is_none());
            match engine {
                GameEngine::Godot => {
                    assert_eq!(project.name.as_deref(), Some("Moon Hop"));
                    assert_eq!(project.version.as_deref(), Some("4.7"));
                }
                _ => assert_eq!(project.name.as_deref(), Some("moon-hop")),
            }
            assert_eq!(
                std::fs::read_to_string(folder.join("README.md")).unwrap(),
                "# notes"
            );
            // No placeholder is left in any file.
            for file in &written {
                let text = std::fs::read_to_string(folder.join(file)).unwrap();
                assert!(!text.contains("__"), "{file} of {starter:?}");
            }
        }
    }

    #[test]
    fn nothing_is_written_over_and_a_refusal_writes_nothing() {
        let scratch = tempfile::tempdir().unwrap();
        // A file of the starter's name is in the way: refused, and the rest are not written.
        std::fs::write(scratch.path().join("main.tscn"), "mine").unwrap();
        let refused = write(scratch.path(), Starter::Godot2d, "4.7").unwrap_err();
        assert!(refused.to_string().contains("main.tscn"), "{refused}");
        assert_eq!(
            std::fs::read_to_string(scratch.path().join("main.tscn")).unwrap(),
            "mine"
        );
        assert!(!scratch.path().join("project.godot").exists());
        assert!(!scratch.path().join("player.gd").exists());

        // A folder that already holds a game is left as it is, whatever is asked for.
        let game = tempfile::tempdir().unwrap();
        write(game.path(), Starter::Web2d, "4.7").unwrap();
        let before = std::fs::read_to_string(game.path().join("game.js")).unwrap();
        for starter in [Starter::Godot2d, Starter::Web2d, Starter::Web3d] {
            let refused = write(game.path(), starter, "4.7").unwrap_err();
            assert!(refused.to_string().contains("already holds"), "{refused}");
        }
        assert_eq!(
            std::fs::read_to_string(game.path().join("game.js")).unwrap(),
            before
        );
        assert!(!game.path().join("project.godot").exists());

        // Not a folder.
        assert!(write(&scratch.path().join("missing"), Starter::Godot2d, "4.7").is_err());
        assert!(write(&scratch.path().join("main.tscn"), Starter::Godot2d, "4.7").is_err());
    }

    #[test]
    fn a_folders_name_cannot_break_the_files_it_is_written_into() {
        assert_eq!(display_name(Path::new("/games/Moon Hop")), "Moon Hop");
        assert_eq!(display_name(Path::new("/games/a\"b<c>&d\\e")), "abcde");
        assert_eq!(display_name(Path::new("/")), "My Game");
        assert_eq!(
            display_name(Path::new(&"x".repeat(200))).chars().count(),
            60
        );
        assert_eq!(package_name("Moon Hop!"), "moon-hop");
        assert_eq!(package_name("  ---  "), "my-game");
        assert_eq!(package_name("Été 2"), "t-2");
        // The Godot version written into the project is digits and dots, whatever is given.
        let scratch = tempfile::tempdir().unwrap();
        write(
            scratch.path(),
            Starter::Godot2d,
            "4.7\")\nrun/main_scene=\"res://evil",
        )
        .unwrap();
        let project = std::fs::read_to_string(scratch.path().join("project.godot")).unwrap();
        assert!(project.contains("PackedStringArray(\"4.7\", \"GL Compatibility\")"));
        assert!(!project.contains("evil"));
    }
}
