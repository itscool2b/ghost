//! Which engine a project is made with, told from the files at its top: no program is run and
//! nothing is written. A chat gets Ghost's game tools when its folder is a game.

use std::path::Path;

use serde::Serialize;

/// The most of one file that is read to tell an engine.
const READ_LIMIT: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum GameEngine {
    /// Godot 4, which Ghost runs itself.
    Godot,
    /// A game that is a web page (Three.js, Phaser and the like), which runs in Ghost's browser.
    Web,
    /// The engines Ghost connects to through their makers' own tools.
    Unity,
    Unreal,
    Roblox,
    /// Recognised and named, with no special support.
    Bevy,
    Love,
    Pygame,
    GameMaker,
    Defold,
    MonoGame,
}

impl GameEngine {
    /// The engine's name as its makers write it.
    pub fn name(self) -> &'static str {
        match self {
            GameEngine::Godot => "Godot",
            GameEngine::Web => "Web",
            GameEngine::Unity => "Unity",
            GameEngine::Unreal => "Unreal Engine",
            GameEngine::Roblox => "Roblox",
            GameEngine::Bevy => "Bevy",
            GameEngine::Love => "LÖVE",
            GameEngine::Pygame => "Pygame",
            GameEngine::GameMaker => "GameMaker",
            GameEngine::Defold => "Defold",
            GameEngine::MonoGame => "MonoGame",
        }
    }
}

/// What a folder's game is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct GameProject {
    pub engine: GameEngine,
    /// The game's own name, where the project states one.
    pub name: Option<String>,
    /// The engine version the project was made with ("4.7", "6000.0.23f1"), where it says.
    pub version: Option<String>,
    /// The library a web game is built on ("three", "phaser").
    pub framework: Option<String>,
    /// Why Ghost cannot run this project itself, in words for the person. `None` when it can, or
    /// when the engine is one Ghost connects to instead.
    pub unsupported: Option<String>,
}

impl GameProject {
    fn of(engine: GameEngine) -> Self {
        Self {
            engine,
            name: None,
            version: None,
            framework: None,
            unsupported: None,
        }
    }

    /// Whether Ghost runs this game itself, so its own game tools apply.
    pub fn runs_in_ghost(&self) -> bool {
        matches!(self.engine, GameEngine::Godot | GameEngine::Web) && self.unsupported.is_none()
    }
}

fn read(path: &Path) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(READ_LIMIT)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The names at the top of `folder`, lowercased, files and folders alike.
fn top(folder: &Path) -> Vec<(String, bool)> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .take(4096)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let is_file = entry.file_type().ok()?.is_file();
            Some((name, is_file))
        })
        .collect()
}

fn with_extension<'a>(names: &'a [(String, bool)], extension: &str) -> Option<&'a str> {
    names
        .iter()
        .find(|(name, is_file)| {
            *is_file
                && Path::new(name)
                    .extension()
                    .is_some_and(|found| found.eq_ignore_ascii_case(extension))
        })
        .map(|(name, _)| name.as_str())
}

/// The value of `key="value"` or `key=value` on a line of its own in an INI-like file.
fn ini_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line
            .trim()
            .strip_prefix(key)?
            .trim_start()
            .strip_prefix('=')?;
        Some(rest.trim().trim_matches('"').to_owned())
    })
}

fn godot(folder: &Path, names: &[(String, bool)]) -> Option<GameProject> {
    let text = read(&folder.join("project.godot"))?;
    let mut project = GameProject::of(GameEngine::Godot);
    project.name = ini_value(&text, "config/name").filter(|name| !name.is_empty());
    let features = ini_value(&text, "config/features").unwrap_or_default();
    // `PackedStringArray("4.7", "GL Compatibility")`: the version is the entry that reads as one.
    project.version = features
        .split('"')
        .map(str::trim)
        .find(|part| {
            let mut numbers = part.split('.');
            matches!(
                (numbers.next(), numbers.next(), numbers.next()),
                (Some(major), Some(minor), None)
                    if major.parse::<u8>().is_ok() && minor.parse::<u8>().is_ok()
            )
        })
        .map(str::to_owned);
    let older = ini_value(&text, "config_version").is_some_and(|version| version != "5");
    if older
        || project
            .version
            .as_deref()
            .is_some_and(|v| v.starts_with("3."))
    {
        project.unsupported = Some(
            "This is a Godot 3 project. Ghost runs Godot 4 games; convert it in Godot 4 first."
                .into(),
        );
    } else if features.contains("\"C#\"") || with_extension(names, "csproj").is_some() {
        project.unsupported = Some(
            "This Godot project is written in C#, which Ghost does not run yet. GDScript projects work."
                .into(),
        );
    }
    Some(project)
}

/// The libraries a web game is built on, most telling first, by package name.
const WEB_FRAMEWORKS: [(&str, &str); 11] = [
    ("three", "three"),
    ("@react-three/fiber", "three"),
    ("phaser", "phaser"),
    ("@babylonjs/core", "babylon"),
    ("babylonjs", "babylon"),
    ("playcanvas", "playcanvas"),
    ("pixi.js", "pixi"),
    ("kaplay", "kaplay"),
    ("kaboom", "kaboom"),
    ("excalibur", "excalibur"),
    ("melonjs", "melonjs"),
];

fn web(folder: &Path) -> Option<GameProject> {
    let text = read(&folder.join("package.json"))?;
    let package: serde_json::Value = serde_json::from_str(&text).ok()?;
    let depends = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|section| package[*section].get(name).is_some())
    };
    let (_, framework) = WEB_FRAMEWORKS.iter().find(|(name, _)| depends(name))?;
    let mut project = GameProject::of(GameEngine::Web);
    project.framework = Some((*framework).to_owned());
    project.name = package["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    Some(project)
}

fn unity(folder: &Path) -> Option<GameProject> {
    let text = read(&folder.join("ProjectSettings").join("ProjectVersion.txt"))?;
    let mut project = GameProject::of(GameEngine::Unity);
    project.version = text.lines().find_map(|line| {
        line.strip_prefix("m_EditorVersion:")
            .map(|version| version.trim().to_owned())
    });
    Some(project)
}

fn unreal(folder: &Path, names: &[(String, bool)]) -> Option<GameProject> {
    let file = with_extension(names, "uproject")?;
    let mut project = GameProject::of(GameEngine::Unreal);
    project.name = Path::new(file)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned);
    project.version = read(&folder.join(file))
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value["EngineAssociation"].as_str().map(str::to_owned))
        .filter(|version| !version.is_empty());
    Some(project)
}

fn has(names: &[(String, bool)], wanted: &str) -> bool {
    names
        .iter()
        .any(|(name, is_file)| *is_file && name.eq_ignore_ascii_case(wanted))
}

fn mentions(folder: &Path, file: &str, word: &str) -> bool {
    read(&folder.join(file)).is_some_and(|text| text.to_ascii_lowercase().contains(word))
}

/// What game `folder` holds, or `None` when it is not one Ghost knows. It reads a handful of
/// small files and blocks while it does: call it from a thread that may.
pub fn detect(folder: &Path) -> Option<GameProject> {
    let names = top(folder);
    if has(&names, "project.godot") {
        return godot(folder, &names);
    }
    if let Some(project) = unity(folder) {
        return Some(project);
    }
    if let Some(project) = unreal(folder, &names) {
        return Some(project);
    }
    if has(&names, "default.project.json")
        || with_extension(&names, "rbxl").is_some()
        || with_extension(&names, "rbxlx").is_some()
    {
        return Some(GameProject::of(GameEngine::Roblox));
    }
    if has(&names, "package.json")
        && let Some(project) = web(folder)
    {
        return Some(project);
    }
    if has(&names, "game.project") {
        return Some(GameProject::of(GameEngine::Defold));
    }
    if with_extension(&names, "yyp").is_some() {
        return Some(GameProject::of(GameEngine::GameMaker));
    }
    if has(&names, "Cargo.toml") && mentions(folder, "Cargo.toml", "\nbevy") {
        return Some(GameProject::of(GameEngine::Bevy));
    }
    if has(&names, "main.lua") && (has(&names, "conf.lua") || mentions(folder, "main.lua", "love."))
    {
        return Some(GameProject::of(GameEngine::Love));
    }
    if ["requirements.txt", "pyproject.toml"]
        .iter()
        .any(|file| has(&names, file) && mentions(folder, file, "pygame"))
    {
        return Some(GameProject::of(GameEngine::Pygame));
    }
    if let Some(file) = with_extension(&names, "csproj")
        && mentions(folder, file, "monogame")
    {
        return Some(GameProject::of(GameEngine::MonoGame));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[(&str, &str)]) -> tempfile::TempDir {
        let folder = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = folder.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        folder
    }

    const GODOT: &str = "config_version=5\n\n[application]\n\nconfig/name=\"Moon Hop\"\nrun/main_scene=\"res://main.tscn\"\nconfig/features=PackedStringArray(\"4.7\", \"GL Compatibility\")\n";

    #[test]
    fn a_godot_4_project_is_one_ghost_runs() {
        let game = folder(&[("project.godot", GODOT)]);
        let project = detect(game.path()).unwrap();
        assert_eq!(project.engine, GameEngine::Godot);
        assert_eq!(project.name.as_deref(), Some("Moon Hop"));
        assert_eq!(project.version.as_deref(), Some("4.7"));
        assert!(project.runs_in_ghost());
    }

    #[test]
    fn godot_3_and_c_sharp_projects_are_named_and_declined() {
        let old = folder(&[(
            "project.godot",
            "config_version=4\n[application]\nconfig/name=\"Old\"\n",
        )]);
        let project = detect(old.path()).unwrap();
        assert_eq!(project.engine, GameEngine::Godot);
        assert!(project.unsupported.as_deref().unwrap().contains("Godot 3"));
        assert!(!project.runs_in_ghost());

        let sharp = folder(&[
            (
                "project.godot",
                &GODOT.replace("\"GL Compatibility\"", "\"C#\""),
            ),
            ("Game.csproj", "<Project Sdk=\"Godot.NET.Sdk/4.7.0\" />"),
        ]);
        let project = detect(sharp.path()).unwrap();
        assert_eq!(project.engine, GameEngine::Godot, "not MonoGame");
        assert!(project.unsupported.as_deref().unwrap().contains("C#"));
    }

    #[test]
    fn a_web_game_is_told_by_what_its_package_depends_on() {
        let three = folder(&[(
            "package.json",
            r#"{"name":"orbit","dependencies":{"three":"^0.170.0"},"devDependencies":{"vite":"^6"}}"#,
        )]);
        let project = detect(three.path()).unwrap();
        assert_eq!(project.engine, GameEngine::Web);
        assert_eq!(project.framework.as_deref(), Some("three"));
        assert_eq!(project.name.as_deref(), Some("orbit"));
        assert!(project.runs_in_ghost());

        let phaser = folder(&[("package.json", r#"{"devDependencies":{"phaser":"3.87.0"}}"#)]);
        assert_eq!(
            detect(phaser.path()).unwrap().framework.as_deref(),
            Some("phaser")
        );
        // A web app that is not a game is not one.
        let app = folder(&[("package.json", r#"{"dependencies":{"react":"19"}}"#)]);
        assert_eq!(detect(app.path()), None);
        let broken = folder(&[("package.json", "{ not json")]);
        assert_eq!(detect(broken.path()), None);
    }

    #[test]
    fn the_engines_ghost_connects_to_are_recognised_with_their_versions() {
        let unity = folder(&[(
            "ProjectSettings/ProjectVersion.txt",
            "m_EditorVersion: 6000.0.23f1\nm_EditorVersionWithRevision: 6000.0.23f1 (abc)\n",
        )]);
        let project = detect(unity.path()).unwrap();
        assert_eq!(project.engine, GameEngine::Unity);
        assert_eq!(project.version.as_deref(), Some("6000.0.23f1"));
        assert!(!project.runs_in_ghost());

        let unreal = folder(&[("Shooter.uproject", r#"{"EngineAssociation":"5.8"}"#)]);
        let project = detect(unreal.path()).unwrap();
        assert_eq!(project.engine, GameEngine::Unreal);
        assert_eq!(project.name.as_deref(), Some("Shooter"));
        assert_eq!(project.version.as_deref(), Some("5.8"));

        let roblox = folder(&[("default.project.json", "{}")]);
        assert_eq!(detect(roblox.path()).unwrap().engine, GameEngine::Roblox);
    }

    #[test]
    fn smaller_engines_are_named_and_an_ordinary_folder_is_nothing() {
        for (files, engine) in [
            (
                vec![("Cargo.toml", "[dependencies]\nbevy = \"0.18\"\n")],
                GameEngine::Bevy,
            ),
            (
                vec![("main.lua", "function love.draw() end")],
                GameEngine::Love,
            ),
            (
                vec![("requirements.txt", "pygame==2.6\n")],
                GameEngine::Pygame,
            ),
            (vec![("game.project", "[project]\n")], GameEngine::Defold),
            (vec![("Thing.yyp", "{}")], GameEngine::GameMaker),
            (
                vec![(
                    "Game.csproj",
                    "<PackageReference Include=\"MonoGame.Framework.DesktopGL\" />",
                )],
                GameEngine::MonoGame,
            ),
        ] {
            let game = folder(&files);
            assert_eq!(detect(game.path()).map(|p| p.engine), Some(engine));
        }
        let plain = folder(&[
            ("Cargo.toml", "[dependencies]\nserde = \"1\"\n"),
            ("main.lua", "print('hi')"),
            ("README.md", "# hello"),
        ]);
        assert_eq!(detect(plain.path()), None);
        assert_eq!(detect(&plain.path().join("missing")), None);
    }

    #[test]
    fn a_folder_named_like_a_project_file_is_not_one() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder.path().join("project.godot")).unwrap();
        assert_eq!(detect(folder.path()), None);
    }
}
