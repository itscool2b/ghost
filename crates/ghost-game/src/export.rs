//! A game packed for the web: a Godot project exported with Godot's own exporter, or a web
//! game's built folder, and either one as a zip that itch.io takes as it is.
//!
//! Only the person exports, from the pane. Nothing is uploaded anywhere: the zip is theirs to
//! upload.
//!
//! For a Godot project, Ghost runs its own Godot without a window
//! (`--headless --export-release "Web"`). Godot needs two things the project may not have:
//!
//! - **The web export template**: ten megabytes inside Godot's 1.28 GB archive of templates.
//!   Ghost fetches that one part ([`ghost_fetch::download_entry`]) and checks it against a pin
//!   made from the archive Godot publishes, itself checked against Godot's own digest.
//! - **An export preset** in the project (`export_presets.cfg`). A project that has one with a
//!   Web preset is exported with it. For one that has none, Ghost writes a preset for the run
//!   and takes it away again when the export ends, however it ends.
//!
//! Godot's exporter imports the project first, as opening it in Godot's editor does, which
//! leaves Godot's own files beside the project's (`.godot`, `*.uid`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use ghost_fetch::EntryPin;
use serde::Serialize;

use crate::Error;
use crate::detect::{GameEngine, GameProject};
use crate::godot;

/// The hosts a GitHub release download passes through.
static HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// Godot 4.7.2's web export template without threads (`templates/web_nothreads_release.zip`):
/// the one that runs on any web host, with no special headers. Where it lies in
/// `Godot_v4.7.2-stable_export_templates.tpz` and what it hashes to were read on 28 November
/// 2026 from a copy of that archive whose SHA-512 matched Godot's `SHA512-SUMS.txt`.
pub const WEB_TEMPLATE: EntryPin<'static> = EntryPin {
    url: "https://github.com/godotengine/godot/releases/download/4.7.2-stable/Godot_v4.7.2-stable_export_templates.tpz",
    offset: 1_053_533_745,
    packed: 10_221_796,
    size: 10_245_903,
    sha256: "d3ee2f08cef0cf3cf6678a6355a92a8db48ccdd35cbd2e8bfd5f0e8a0b4032a0",
};

/// How long Godot may take to export. A project's first export imports every asset.
const PATIENCE: Duration = Duration::from_secs(15 * 60);
/// The most a packed game holds.
const ZIP_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

/// What an export made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct GameExport {
    /// The folder the game was exported to: `index.html` and what it loads.
    pub folder: String,
    /// The same, packed with `index.html` at its top, as itch.io takes it.
    pub zip: String,
    pub files: u32,
    /// The zip's size in bytes.
    pub bytes: u32,
    /// What was exported, in a few words ("Godot's web export", "The built folder dist").
    pub what: String,
}

fn refused(why: impl Into<String>) -> Error {
    Error::Refused(why.into())
}

/// A name a file may have on any system: letters, digits, `-` and `_`.
pub fn file_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 48 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "game".to_owned()
    } else {
        out
    }
}

/// Where the web template is kept once fetched.
pub fn template_path(root: &Path) -> PathBuf {
    root.join("templates")
        .join(godot::VERSION)
        .join("web_nothreads_release.zip")
}

/// The web template, fetched if it is not here yet. A file of the pinned size is taken as the
/// template: it was given its name only after its hash was checked.
pub async fn web_template(root: &Path) -> Result<PathBuf, Error> {
    let path = template_path(root);
    if tokio::fs::metadata(&path)
        .await
        .is_ok_and(|found| found.len() == WEB_TEMPLATE.size)
    {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let client = ghost_fetch::client(&HOSTS)
        .map_err(|error| Error::Download(format!("Godot's web template: {error}.")))?;
    ghost_fetch::download_entry(&client, &HOSTS, WEB_TEMPLATE, &path)
        .await
        .map_err(|error| {
            Error::Download(format!(
                "Godot's web template could not be fetched: {error}."
            ))
        })?;
    Ok(path)
}

/// Ghost's Godot under a second name in a folder of its own, beside a file (`._sc_`) that
/// tells Godot to keep its editor's settings and caches in that folder. Exporting runs Godot's
/// editor without a window, and this way it leaves nothing in the person's own Godot folder, or
/// makes one for a person who never opened Godot. On a Mac, where Godot is a bundle, and
/// wherever the second name cannot be made, Godot runs as it is.
pub async fn exporter(root: &Path, program: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        return program.to_owned();
    }
    let Some(name) = program.file_name() else {
        return program.to_owned();
    };
    let folder = root.join("exporter").join(godot::VERSION);
    let kept = folder.join(name);
    let made = async {
        tokio::fs::create_dir_all(&folder).await?;
        if tokio::fs::metadata(&kept).await.is_err()
            && tokio::fs::hard_link(program, &kept).await.is_err()
        {
            tokio::fs::copy(program, &kept).await?;
        }
        let marker = folder.join("._sc_");
        if tokio::fs::metadata(&marker).await.is_err() {
            tokio::fs::write(&marker, b"").await?;
        }
        std::io::Result::Ok(())
    };
    match made.await {
        Ok(()) => kept,
        Err(_) => program.to_owned(),
    }
}

/// The name of the project's own Web preset, when its `export_presets.cfg` has one.
pub fn web_preset(presets: &str) -> Option<String> {
    let mut name: Option<String> = None;
    let mut web = false;
    let mut in_preset = false;
    let done = |name: &Option<String>, web: bool| web.then(|| name.clone()).flatten();
    for line in presets.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if in_preset && let Some(found) = done(&name, web) {
                return Some(found);
            }
            // `[preset.0]` begins one; `[preset.0.options]` belongs to the one before.
            in_preset = line.starts_with("[preset.") && !line.ends_with(".options]");
            if in_preset {
                name = None;
                web = false;
            }
            continue;
        }
        if !in_preset {
            continue;
        }
        if let Some(value) = line.strip_prefix("name=") {
            name = Some(value.trim_matches('"').to_owned());
        } else if let Some(value) = line.strip_prefix("platform=") {
            web = value.trim_matches('"') == "Web";
        }
    }
    done(&name, web).filter(|_| in_preset)
}

/// The preset Ghost writes for a project that has none: every resource, no threads (so the game
/// runs on any host), and the template by its path in Ghost's folder.
pub fn preset_for(template: &Path) -> String {
    // Godot reads the value as a string with its own escapes.
    let template = template
        .to_string_lossy()
        .replace('\\', "/")
        .replace('"', "\\\"");
    format!(
        "[preset.0]\n\nname=\"Web\"\nplatform=\"Web\"\nrunnable=true\nexport_filter=\"all_resources\"\nexport_path=\"\"\n\n[preset.0.options]\n\ncustom_template/release=\"{template}\"\nvariant/thread_support=false\n"
    )
}

/// Takes away the preset Ghost wrote, when the export ends or is dropped.
struct Written(Option<PathBuf>);

impl Drop for Written {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Exports a Godot project for the web into `out` with `program`, and answers what Godot said
/// last. `out` is emptied first.
pub async fn export_godot(
    program: &Path,
    project: &Path,
    template: &Path,
    out: &Path,
) -> Result<(), Error> {
    let presets = project.join("export_presets.cfg");
    let (preset, _written) = match tokio::fs::read_to_string(&presets).await {
        Ok(text) => {
            let name = web_preset(&text).ok_or_else(|| {
                refused("This project has export presets but none for the web. Add a Web preset in Godot (Project > Export), or ask your agent to.")
            })?;
            (name, Written(None))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Never over a file that is there: made new, or not at all.
            let mut options = tokio::fs::OpenOptions::new();
            options.write(true).create_new(true);
            let mut file = options.open(&presets).await?;
            let written = Written(Some(presets.clone()));
            tokio::io::AsyncWriteExt::write_all(&mut file, preset_for(template).as_bytes()).await?;
            tokio::io::AsyncWriteExt::flush(&mut file).await?;
            ("Web".to_owned(), written)
        }
        Err(error) => return Err(error.into()),
    };
    if tokio::fs::metadata(out).await.is_ok() {
        tokio::fs::remove_dir_all(out).await?;
    }
    tokio::fs::create_dir_all(out).await?;
    let mut command = crate::command(program);
    command
        .arg("--headless")
        .arg("--path")
        .arg(project)
        .arg("--export-release")
        .arg(&preset)
        .arg(out.join("index.html"))
        .env_remove("GHOST_GAME_BRIDGE")
        .env_remove("GHOST_GAME_TOKEN")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let child = command
        .spawn()
        .map_err(|error| Error::Game(format!("Godot could not be started: {error}")))?;
    let output = tokio::time::timeout(PATIENCE, child.wait_with_output())
        .await
        .map_err(|_| Error::Game("Godot took too long to export the game.".into()))??;
    let made = tokio::fs::metadata(out.join("index.html")).await.is_ok()
        && tokio::fs::metadata(out.join("index.wasm")).await.is_ok();
    if made {
        return Ok(());
    }
    // What Godot said, cut to its last lines and to plain text.
    let said: String = String::from_utf8_lossy(&output.stderr)
        .lines()
        .chain(String::from_utf8_lossy(&output.stdout).lines())
        .map(str::trim)
        .rfind(|line| line.contains("ERROR") || line.contains("rror:"))
        .unwrap_or("Godot gave no reason.")
        .chars()
        .filter(|ch| !ch.is_control())
        .take(300)
        .collect();
    Err(Error::Game(format!(
        "Godot did not export the game: {said}"
    )))
}

/// The folder of a web game that a host serves: its build (`dist`, `build`, `out`, `public`
/// with an `index.html`), or the project itself when it is a plain page.
pub fn web_folder(project: &Path) -> Option<(PathBuf, String)> {
    for built in ["dist", "build", "out"] {
        let folder = project.join(built);
        if folder.join("index.html").is_file() {
            return Some((folder, format!("The built folder {built}")));
        }
    }
    project
        .join("index.html")
        .is_file()
        .then(|| (project.to_owned(), "The project's own files".to_owned()))
}

/// Exports the project in `folder` for the web. `godot` is the Godot that exports a Godot
/// project (Ghost's own, of the pinned version); `root` is Ghost's folder for games; the zip
/// goes into `downloads`.
pub async fn export(
    project: &GameProject,
    folder: &Path,
    godot: Option<&Path>,
    root: &Path,
    downloads: &Path,
) -> Result<GameExport, Error> {
    // The game's own name, or its folder's when the project states none.
    let name = file_name(
        project
            .name
            .as_deref()
            .or_else(|| folder.file_name().and_then(|name| name.to_str()))
            .unwrap_or("game"),
    );
    let (served, what) = match project.engine {
        GameEngine::Godot => {
            let program = godot.ok_or_else(|| {
                refused(format!(
                    "Exporting uses Ghost's own Godot {}. Get it in the Game pane first.",
                    godot::VERSION
                ))
            })?;
            let template = web_template(root).await?;
            let out = root.join("exports").join(&name);
            let program = exporter(root, program).await;
            export_godot(&program, folder, &template, &out).await?;
            (out, "Godot's web export".to_owned())
        }
        GameEngine::Web => web_folder(folder).ok_or_else(|| {
            refused("This web game has no page to pack yet. Build it first (ask your agent to run the project's build), so that a folder named dist or build holds an index.html.")
        })?,
        other => {
            return Err(refused(format!(
                "{} builds its own games. Export from its editor.",
                other.name()
            )));
        }
    };
    tokio::fs::create_dir_all(downloads).await?;
    let zip = downloads.join(format!("{name}-web.zip"));
    let (from, to) = (served.clone(), zip.clone());
    let files = tokio::task::spawn_blocking(move || {
        // A web project packed from its own folder leaves its tools and its sources' caches out.
        ghost_fetch::zip_folder(
            &from,
            &to,
            &["node_modules", "target", "src-tauri", "package-lock.json"],
            ZIP_LIMIT,
        )
    })
    .await
    .map_err(|_| Error::Game("The game could not be packed.".into()))?
    .map_err(|error| Error::Game(format!("The game could not be packed: {error}")))?;
    let bytes = tokio::fs::metadata(&zip)
        .await
        .map_or(0, |found| found.len());
    Ok(GameExport {
        folder: served.to_string_lossy().into_owned(),
        zip: zip.to_string_lossy().into_owned(),
        files,
        bytes: u32::try_from(bytes).unwrap_or(u32::MAX),
        what,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_projects_own_web_preset_is_found_by_its_platform() {
        let presets = "[preset.0]\n\nname=\"Windows Desktop\"\nplatform=\"Windows Desktop\"\n\n[preset.0.options]\n\ncustom_template/release=\"\"\n\n[preset.1]\n\nname=\"itch\"\nplatform=\"Web\"\nrunnable=true\n\n[preset.1.options]\n\nvariant/thread_support=false\n";
        assert_eq!(web_preset(presets), Some("itch".to_owned()));
        assert_eq!(
            web_preset("[preset.0]\nname=\"Linux\"\nplatform=\"Linux\"\n"),
            None
        );
        assert_eq!(web_preset(""), None);
        // The last preset of the file, and a platform written before its name.
        assert_eq!(
            web_preset("[preset.0]\nplatform=\"Web\"\nname=\"Web\"\n"),
            Some("Web".to_owned())
        );
        // What Ghost writes is one it would find.
        assert_eq!(
            web_preset(&preset_for(Path::new("/ghost/templates/web.zip"))),
            Some("Web".to_owned())
        );
    }

    #[test]
    fn the_preset_names_the_template_as_godot_reads_a_path() {
        let preset = preset_for(Path::new(r"C:\Users\me\Ghost\templates\4.7.2\web.zip"));
        assert!(
            preset
                .contains("custom_template/release=\"C:/Users/me/Ghost/templates/4.7.2/web.zip\""),
            "{preset}"
        );
        assert!(preset.contains("variant/thread_support=false"));
        assert!(preset.contains("export_filter=\"all_resources\""));
    }

    #[test]
    fn a_games_name_becomes_a_files() {
        assert_eq!(file_name("Moon Hop"), "moon-hop");
        assert_eq!(file_name("  Sky/Hop: 3D!  "), "sky-hop-3d");
        assert_eq!(file_name("../../etc"), "etc");
        assert_eq!(file_name("???"), "game");
        assert_eq!(file_name(""), "game");
        assert!(file_name(&"a".repeat(200)).len() <= 48);
    }

    #[test]
    fn a_web_game_is_packed_from_its_build_or_from_itself() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(web_folder(root), None);
        std::fs::write(root.join("index.html"), b"<html>").unwrap();
        assert_eq!(
            web_folder(root),
            Some((root.to_owned(), "The project's own files".to_owned()))
        );
        std::fs::create_dir_all(root.join("dist")).unwrap();
        // A build folder with no page in it is not the game.
        assert_eq!(web_folder(root).unwrap().0, root);
        std::fs::write(root.join("dist/index.html"), b"<html>").unwrap();
        assert_eq!(
            web_folder(root),
            Some((root.join("dist"), "The built folder dist".to_owned()))
        );
    }

    #[tokio::test]
    async fn a_preset_ghost_wrote_is_taken_away_and_the_projects_own_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let out = dir.path().join("out");
        // A Godot that is not there: the export fails after the preset was written.
        let missing = dir.path().join("no-godot");
        let failed = export_godot(&missing, &project, Path::new("/t/web.zip"), &out).await;
        assert!(failed.is_err());
        assert!(
            !project.join("export_presets.cfg").exists(),
            "the preset written for the run is gone"
        );
        // The project's own presets are never written over or removed.
        let own = "[preset.0]\nname=\"Mine\"\nplatform=\"Web\"\n";
        std::fs::write(project.join("export_presets.cfg"), own).unwrap();
        let failed = export_godot(&missing, &project, Path::new("/t/web.zip"), &out).await;
        assert!(failed.is_err());
        assert_eq!(
            std::fs::read_to_string(project.join("export_presets.cfg")).unwrap(),
            own
        );
        // Presets with no Web one: said, and nothing run.
        std::fs::write(
            project.join("export_presets.cfg"),
            "[preset.0]\nname=\"Linux\"\nplatform=\"Linux\"\n",
        )
        .unwrap();
        let refused = export_godot(&missing, &project, Path::new("/t/web.zip"), &out).await;
        assert!(matches!(refused, Err(Error::Refused(why)) if why.contains("none for the web")));
    }

    #[tokio::test]
    async fn only_godot_and_web_projects_are_exported_here() {
        let dir = tempfile::tempdir().unwrap();
        let project = |engine| GameProject {
            engine,
            name: Some("Harbor".into()),
            version: None,
            framework: None,
            unsupported: None,
        };
        let unity = export(
            &project(GameEngine::Unity),
            dir.path(),
            None,
            dir.path(),
            dir.path(),
        )
        .await;
        assert!(
            matches!(unity, Err(Error::Refused(why)) if why.contains("Export from its editor"))
        );
        // A Godot project with no Godot of Ghost's own: said, and nothing fetched.
        let godot = export(
            &project(GameEngine::Godot),
            dir.path(),
            None,
            dir.path(),
            dir.path(),
        )
        .await;
        assert!(
            matches!(godot, Err(Error::Refused(why)) if why.contains("Get it in the Game pane"))
        );
        assert!(!template_path(dir.path()).exists());
        // A web game with nothing built.
        let web = export(
            &project(GameEngine::Web),
            dir.path(),
            None,
            dir.path(),
            dir.path(),
        )
        .await;
        assert!(matches!(web, Err(Error::Refused(why)) if why.contains("Build it first")));
        // And one with a page: packed, with the page at the archive's top.
        let game = dir.path().join("game");
        std::fs::create_dir_all(game.join("dist/assets")).unwrap();
        std::fs::create_dir_all(game.join("node_modules/x")).unwrap();
        std::fs::write(game.join("dist/index.html"), b"<html>").unwrap();
        std::fs::write(game.join("dist/assets/a.js"), b"js").unwrap();
        let downloads = dir.path().join("Downloads");
        let done = export(
            &project(GameEngine::Web),
            &game,
            None,
            dir.path(),
            &downloads,
        )
        .await
        .unwrap();
        assert_eq!(
            (done.files, done.what.as_str()),
            (2, "The built folder dist")
        );
        assert_eq!(done.zip, downloads.join("harbor-web.zip").to_string_lossy());
        assert!(done.bytes > 0);
    }
}
