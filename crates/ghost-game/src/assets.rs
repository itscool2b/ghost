//! The pictures, sounds and 3D models of a game's folder, for the pane's Assets view: a list of
//! what is there, and one file's bytes for the person to look at or hear.
//!
//! Both stay inside the folder they are given. Nothing a link points at is listed or read (a
//! link in a project may lead anywhere on the computer), every part of a path is opened without
//! following links, and only a file that is an asset by its name is read, up to a size its kind
//! allows: this is a way to look at art, not a way to read a project's files.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use serde::{Deserialize, Serialize};

use crate::Error;

/// What an asset is, by its file's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Picture,
    Sound,
    Model,
}

impl AssetKind {
    /// The kind of a file with this name, when it is an asset Ghost shows.
    pub fn of(name: &str) -> Option<Self> {
        let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
        Some(match extension.as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg" => Self::Picture,
            "wav" | "ogg" | "mp3" | "flac" => Self::Sound,
            "glb" | "gltf" => Self::Model,
            _ => return None,
        })
    }

    /// The most of one file Ghost reads to show it.
    pub fn most(self) -> u64 {
        match self {
            Self::Picture => 12 * 1024 * 1024,
            Self::Sound => 24 * 1024 * 1024,
            Self::Model => 32 * 1024 * 1024,
        }
    }
}

/// One asset of the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct GameAsset {
    /// Its path inside the project, with forward slashes ("art/player/run.png").
    pub path: String,
    pub kind: AssetKind,
    /// Its size in bytes (4 GiB and more reads as 4 GiB).
    pub bytes: u32,
    /// When it last changed, in seconds since 1970: what is new comes first.
    pub modified: u32,
    /// Larger than Ghost reads to show it: listed, not previewed.
    pub too_large: bool,
}

/// What a folder holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct GameAssets {
    /// The newest first.
    pub assets: Vec<GameAsset>,
    /// The folder holds more than Ghost lists.
    pub cut: bool,
}

/// The most assets one list holds, the most entries one look at a folder reads, and how deep it
/// goes.
const MOST_ASSETS: usize = 1500;
const MOST_ENTRIES: usize = 40_000;
const DEEPEST: usize = 10;

/// Folders that hold no art of the game's own: tools' caches, dependencies, builds.
fn skipped(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "node_modules" | "target" | "build" | "dist" | "out" | "__pycache__" | "venv"
        )
}

/// Every picture, sound and model under `folder`, the newest first. Links are not followed and
/// hidden folders, dependencies and builds are not entered.
pub fn list(folder: &Path) -> GameAssets {
    let mut assets = Vec::new();
    let mut cut = false;
    let mut seen = 0usize;
    let mut waiting: Vec<(PathBuf, String, usize)> = vec![(folder.to_owned(), String::new(), 0)];
    while let Some((directory, prefix, depth)) = waiting.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MOST_ENTRIES {
                cut = true;
                break;
            }
            // The entry's own kind: a link is neither a folder nor a file here.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if depth + 1 < DEEPEST && !skipped(&name) {
                    waiting.push((entry.path(), format!("{prefix}{name}/"), depth + 1));
                }
                continue;
            }
            let (true, Some(asset)) = (kind.is_file(), AssetKind::of(&name)) else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let modified = metadata
                .modified()
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| {
                    u32::try_from(since.as_secs()).unwrap_or(u32::MAX)
                });
            assets.push(GameAsset {
                path: format!("{prefix}{name}"),
                kind: asset,
                bytes: u32::try_from(metadata.len()).unwrap_or(u32::MAX),
                modified,
                too_large: metadata.len() > asset.most(),
            });
        }
        if cut {
            break;
        }
    }
    assets.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    if assets.len() > MOST_ASSETS {
        assets.truncate(MOST_ASSETS);
        cut = true;
    }
    GameAssets { assets, cut }
}

fn refused(why: &str) -> Error {
    Error::Refused(why.to_owned())
}

/// The path's parts, when it is a plain path inside a folder: no root, no `..`, no hidden part.
fn parts(path: &str) -> Option<Vec<&str>> {
    if path.is_empty() || path.len() > 1024 || path.contains(['\0', '\\', ':']) {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    let plain = parts.iter().all(|part| {
        !part.is_empty()
            && !part.starts_with('.')
            && matches!(
                Path::new(part).components().collect::<Vec<_>>().as_slice(),
                [Component::Normal(_)]
            )
    });
    plain.then_some(parts)
}

/// One asset's bytes. `path` is as [`list`] gives it. The file is read through handles that
/// follow no link, so a path that is swapped for a link while it is being read cannot lead out
/// of the folder.
pub fn read(folder: &Path, path: &str) -> Result<(AssetKind, Vec<u8>), Error> {
    let parts = parts(path).ok_or_else(|| refused("That is not an asset of this project."))?;
    let name = *parts.last().unwrap_or(&"");
    let kind =
        AssetKind::of(name).ok_or_else(|| refused("That is not a picture, a sound or a model."))?;
    let mut directory = cap_std::fs::Dir::open_ambient_dir(folder, cap_std::ambient_authority())
        .map_err(|_| refused("The chat's folder cannot be opened."))?;
    for part in &parts[..parts.len() - 1] {
        directory = directory
            .open_dir_nofollow(part)
            .map_err(|_| refused("The asset's folder is gone, or is a link."))?;
    }
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = directory
        .open_with(name, &options)
        .map_err(|_| refused("The asset is gone, or is a link."))?;
    let metadata = file
        .metadata()
        .map_err(|_| refused("The asset cannot be read."))?;
    if !metadata.is_file() {
        return Err(refused("That is not a file."));
    }
    if metadata.len() > kind.most() {
        return Err(refused("The file is too large to show here."));
    }
    let mut bytes = Vec::new();
    file.take(kind.most() + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| refused("The asset cannot be read."))?;
    if bytes.len() as u64 > kind.most() {
        return Err(refused("The file is too large to show here."));
    }
    Ok((kind, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }

    #[test]
    fn a_folder_lists_its_pictures_sounds_and_models_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "icon.svg", b"<svg/>");
        write(root, "art/player/run.PNG", b"png");
        write(root, "sound/jump.wav", b"wav");
        write(root, "models/tree.glb", b"glb");
        // Not assets: code, text, Godot's sidecar files.
        write(root, "player.gd", b"extends Node");
        write(root, "art/player/run.png.import", b"[remap]");
        write(root, "README.md", b"# Game");
        // Not the game's own: a hidden cache, dependencies, a build.
        write(root, ".godot/imported/run.png", b"png");
        write(root, "node_modules/three/logo.png", b"png");
        write(root, "build/web/index.png", b"png");
        let found = list(root);
        assert!(!found.cut);
        let mut paths: Vec<(&str, AssetKind)> = found
            .assets
            .iter()
            .map(|asset| (asset.path.as_str(), asset.kind))
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                ("art/player/run.PNG", AssetKind::Picture),
                ("icon.svg", AssetKind::Picture),
                ("models/tree.glb", AssetKind::Model),
                ("sound/jump.wav", AssetKind::Sound),
            ]
        );
        let sound = found
            .assets
            .iter()
            .find(|asset| asset.path == "sound/jump.wav")
            .unwrap();
        assert_eq!((sound.bytes, sound.too_large), (3, false));
        assert!(sound.modified > 1_600_000_000);
        // A folder that is not there lists nothing.
        assert_eq!(list(&root.join("missing")).assets, []);
    }

    #[test]
    fn an_asset_is_read_by_its_listed_path_and_nothing_else_is() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "art/player/run.png", b"the picture");
        write(root, "secrets.env", b"no");
        write(root, ".hidden/cover.png", b"no");
        let (kind, bytes) = read(root, "art/player/run.png").unwrap();
        assert_eq!(
            (kind, bytes.as_slice()),
            (AssetKind::Picture, &b"the picture"[..])
        );
        for path in [
            "",
            "secrets.env",
            "art/player",
            "art/player/missing.png",
            "../outside.png",
            "art/../art/player/run.png",
            "art/./player/run.png",
            "/art/player/run.png",
            "art//player/run.png",
            "art\\player\\run.png",
            "C:/art/player/run.png",
            ".hidden/cover.png",
            "art/player/run.png\0.gd",
        ] {
            assert!(read(root, path).is_err(), "{path:?} was read");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_neither_listed_nor_read() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "private.png", b"someone else's");
        write(outside.path(), "folder/inside.png", b"someone else's");
        let root = dir.path();
        write(root, "real.png", b"mine");
        std::os::unix::fs::symlink(outside.path().join("private.png"), root.join("linked.png"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path().join("folder"), root.join("linked")).unwrap();
        let paths: Vec<String> = list(root)
            .assets
            .into_iter()
            .map(|asset| asset.path)
            .collect();
        assert_eq!(paths, ["real.png"]);
        assert!(read(root, "linked.png").is_err());
        assert!(read(root, "linked/inside.png").is_err());
        assert_eq!(read(root, "real.png").unwrap().1, b"mine");
    }

    #[test]
    fn a_file_larger_than_its_kind_allows_is_listed_and_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = std::fs::File::create(root.join("huge.png")).unwrap();
        file.set_len(AssetKind::Picture.most() + 1).unwrap();
        let found = list(root);
        assert_eq!(found.assets.len(), 1);
        assert!(found.assets[0].too_large);
        assert!(read(root, "huge.png").is_err());
    }

    #[test]
    fn kinds_go_by_the_end_of_a_name() {
        assert_eq!(AssetKind::of("a.b.PNG"), Some(AssetKind::Picture));
        assert_eq!(AssetKind::of("theme.ogg"), Some(AssetKind::Sound));
        assert_eq!(AssetKind::of("tree.gltf"), Some(AssetKind::Model));
        assert_eq!(AssetKind::of("png"), None);
        assert_eq!(AssetKind::of("run.png.import"), None);
        assert_eq!(AssetKind::of("player.tscn"), None);
    }
}
