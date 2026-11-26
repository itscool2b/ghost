//! A chat's file references (paths in answers, tool rows and Changes), opened in the person's
//! editor at their line.
//!
//! A reference names a file inside the chat's folder or nothing. An absolute one is matched
//! against the folder's spelling as text first, so a path anywhere else (another folder, a
//! network share) is never touched; then each part is checked from the folder down without
//! following links, so a link inside the folder cannot lead out of it either. A bare name or a
//! partial path (`ChatView.tsx`, `chat/ChatView.tsx`) is looked up in the folder, the way the
//! composer's mentions search it, and names a file only when exactly one file ends with it. A
//! diff's `a/` or `b/` prefix is read as the path without it. Nothing is read from any file.
use super::editors::{self, Detected, Position, Target};
use super::project_tools::{EditorLaunch, EditorOpen, UrlTarget, detected, display_path};
use super::*;
use ghost_protocol::agents::AgentHost;
use ghost_protocol::project_tools::ProjectEditor;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::Component;

/// References one check may name.
const MAX_FILE_REFERENCES: usize = 200;
const MAX_REFERENCE_BYTES: usize = 4096;
/// The search for bare names and partial paths: as bounded as the composer's mention search.
const SEARCH_ENTRIES: usize = 50_000;
const SEARCH_TIME: Duration = Duration::from_millis(400);
const SEARCH_DEPTH: usize = 16;

const NO_EDITOR: &str = "No supported editor was found. Install Cursor, Visual Studio Code, Zed or a JetBrains IDE in its standard location.";

fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::Organization(message.into())
}

/// A chat's folder: where every file is looked up, and how references may spell it.
#[derive(Debug)]
pub(super) struct Folder {
    /// Canonical; every check walks down from here.
    canonical: PathBuf,
    /// The canonical folder as tools spell it, then the saved spelling when it differs (a folder
    /// reached through a link).
    spellings: Vec<PathBuf>,
}

impl Folder {
    pub(super) fn new(saved: &Path) -> Option<Folder> {
        let canonical = saved.canonicalize().ok().filter(|path| path.is_dir())?;
        let mut spellings = vec![crate::fsutil::plain_path(&canonical)];
        let saved = crate::fsutil::plain_path(saved);
        if !spellings.contains(&saved) {
            spellings.push(saved);
        }
        Some(Folder {
            canonical,
            spellings,
        })
    }
}

/// The file a reference names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Found {
    pub absolute: PathBuf,
    /// From the folder, with `/` between parts.
    pub relative: String,
}

impl SessionManager {
    /// The editors installed in their standard locations, in the order Settings lists them.
    pub async fn detected_editors(&self) -> Vec<ProjectEditor> {
        let env = self.deps().env.get().await;
        self.deps()
            .rt
            .spawn_blocking(move || detected(&env).into_iter().map(|item| item.editor).collect())
            .await
            .unwrap_or_default()
    }

    /// For each reference, the folder-relative path of the file it names in the chat's folder, or
    /// `None`.
    pub async fn chat_file_check(
        &self,
        chat_id: &str,
        references: Vec<String>,
    ) -> Result<Vec<Option<String>>, SessionError> {
        if references.len() > MAX_FILE_REFERENCES {
            return Err(invalid("Check fewer file references at a time."));
        }
        let folder = self.file_folder(chat_id).await?;
        self.deps()
            .rt
            .spawn_blocking(move || {
                resolve(&folder, &references)
                    .into_iter()
                    .map(|found| found.map(|found| found.relative))
                    .collect()
            })
            .await
            .map_err(|_| invalid("The chat's files could not be checked."))
    }

    /// How to open the file `reference` names, at `line` and `column` when given: in
    /// `preferred` when it is installed, otherwise in the first editor detected. Also says which
    /// editor that is.
    pub async fn chat_file_open(
        &self,
        chat_id: &str,
        reference: String,
        line: Option<u32>,
        column: Option<u32>,
        preferred: Option<ProjectEditor>,
    ) -> Result<(ProjectEditor, EditorOpen), SessionError> {
        // Opening a file changes nothing, so it waits for no other change.
        let folder = self.file_folder(chat_id).await?;
        let env = self.deps().env.get().await;
        let position = position(line, column);
        self.deps()
            .rt
            .spawn_blocking(move || {
                let file = resolve(&folder, std::slice::from_ref(&reference))
                    .pop()
                    .flatten()
                    .ok_or_else(|| invalid("This file is not in the chat's folder."))?;
                let found = detected(&env);
                let editor = choose(&found, preferred).ok_or_else(|| invalid(NO_EDITOR))?;
                let path = display_path(&file.absolute)?;
                let open = match editors::command(
                    editor,
                    Target::File {
                        path: &path,
                        position,
                    },
                ) {
                    None => EditorOpen::Url {
                        path,
                        target: UrlTarget::File {
                            line: position.map(|place| place.line),
                            column: position.and_then(|place| place.column),
                        },
                    },
                    Some(command) => EditorOpen::Launch(EditorLaunch::new(command, &env)),
                };
                Ok((editor.editor, open))
            })
            .await
            .map_err(|_| invalid("The file could not be opened."))?
    }

    async fn file_folder(&self, chat_id: &str) -> Result<Folder, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let (chat, _) = self
            .deps()
            .store
            .checkpoint_context(chat_id, None)
            .await
            .map_err(|error| super::actor::storage_error(&error))?;
        if chat.agent_host != AgentHost::Native {
            return Err(invalid(
                "Files open in an editor only for chats that run on this device.",
            ));
        }
        Folder::new(&chat.folder).ok_or_else(|| invalid("This chat's folder is unavailable."))
    }
}

/// The preferred editor when it is installed, otherwise the first one detected.
fn choose(found: &[Detected], preferred: Option<ProjectEditor>) -> Option<&Detected> {
    preferred
        .and_then(|editor| found.iter().find(|item| item.editor == editor))
        .or_else(|| found.first())
}

/// A place counted from 1; a line of 0 is no place, and a column of 0 no column.
fn position(line: Option<u32>, column: Option<u32>) -> Option<Position> {
    let line = line.filter(|line| *line > 0)?;
    Some(Position {
        line,
        column: column.filter(|column| *column > 0),
    })
}

/// The files `references` name, in order.
pub(super) fn resolve(folder: &Folder, references: &[String]) -> Vec<Option<Found>> {
    let mut found = Vec::with_capacity(references.len());
    let mut unanchored = Vec::new();
    for (index, reference) in references.iter().enumerate() {
        let Some((parts, anchored)) = parts(folder, reference) else {
            found.push(None);
            continue;
        };
        let direct = walk(&folder.canonical, &parts).or_else(|| {
            // A diff's `a/` or `b/` prefix names the same file without it.
            (parts.len() > 1 && (parts[0] == "a" || parts[0] == "b"))
                .then(|| walk(&folder.canonical, &parts[1..]))
                .flatten()
        });
        if direct.is_none() && !anchored {
            unanchored.push((index, parts));
        }
        found.push(direct);
    }
    if !unanchored.is_empty() {
        for (index, file) in search(&folder.canonical, &unanchored) {
            found[index] = Some(file);
        }
    }
    found
}

/// A reference's parts below the folder, and whether it said where it starts (an absolute path,
/// or one starting with `./`). `None` when it cannot name a file in the folder.
fn parts(folder: &Folder, reference: &str) -> Option<(Vec<OsString>, bool)> {
    if reference.is_empty()
        || reference.len() > MAX_REFERENCE_BYTES
        || reference.chars().any(char::is_control)
    {
        return None;
    }
    let reference = drive_spelling(reference);
    let path = Path::new(reference.as_ref());
    let (rest, anchored): (Vec<Component<'_>>, bool) = if path.has_root() || path.is_absolute() {
        let rest = folder.spellings.iter().find_map(|root| below(path, root))?;
        (rest, true)
    } else {
        let components: Vec<Component<'_>> = path.components().collect();
        let anchored = matches!(components.first(), Some(Component::CurDir));
        let rest = components
            .into_iter()
            .filter(|part| !matches!(part, Component::CurDir))
            .collect();
        (rest, anchored)
    };
    if rest.is_empty() {
        return None;
    }
    let parts = rest
        .into_iter()
        .map(|part| match part {
            Component::Normal(name) if allowed(name) => Some(name.to_owned()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((parts, anchored))
}

/// Git Bash spells `C:\x` as `/c/x/`; on Windows such a reference is read as the drive path.
fn drive_spelling(reference: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(windows) {
        let bytes = reference.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b'/'
        {
            return format!(
                "{}:\\{}",
                char::from(bytes[1].to_ascii_uppercase()),
                &reference[3..]
            )
            .into();
        }
    }
    reference.into()
}

/// The parts of `path` below `root`, compared as text (without case on Windows).
fn below<'a>(path: &'a Path, root: &Path) -> Option<Vec<Component<'a>>> {
    let mut parts = path.components();
    for expected in root.components() {
        let part = parts.next()?;
        if !same(part.as_os_str(), expected.as_os_str()) {
            return None;
        }
    }
    Some(parts.collect())
}

fn same(left: &OsStr, right: &OsStr) -> bool {
    if cfg!(windows) {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    } else {
        left == right
    }
}

/// A name a file in the folder can have. On Windows a colon would name an alternate data stream.
fn allowed(name: &OsStr) -> bool {
    !name.is_empty() && !(cfg!(windows) && name.to_string_lossy().contains(':'))
}

/// The regular file at `parts` below `root`, reached through folders only, never a link.
fn walk(root: &Path, parts: &[OsString]) -> Option<Found> {
    let last = parts.len().checked_sub(1)?;
    let mut path = root.to_path_buf();
    for (index, name) in parts.iter().enumerate() {
        path.push(name);
        let kind = std::fs::symlink_metadata(&path).ok()?.file_type();
        // On Windows a junction reads as a link too.
        let fits = if index < last {
            kind.is_dir()
        } else {
            kind.is_file()
        };
        if kind.is_symlink() || !fits {
            return None;
        }
    }
    Some(Found {
        absolute: path,
        relative: relative_text(parts),
    })
}

fn relative_text(parts: &[OsString]) -> String {
    parts
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn key(name: &OsStr) -> String {
    if cfg!(windows) {
        name.to_string_lossy().to_lowercase()
    } else {
        name.to_string_lossy().into_owned()
    }
}

/// Finds the one file each unanchored reference ends with, by one bounded walk of the folder that
/// skips what the composer's search skips (dot folders, dependencies and build output) and never
/// follows a link. A reference two files end with names neither.
fn search(root: &Path, wanted: &[(usize, Vec<OsString>)]) -> Vec<(usize, Found)> {
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (slot, (_, parts)) in wanted.iter().enumerate() {
        if let Some(name) = parts.last() {
            by_name.entry(key(name)).or_default().push(slot);
        }
    }
    let mut matches: Vec<Vec<Vec<OsString>>> = vec![Vec::new(); wanted.len()];
    let started = std::time::Instant::now();
    let mut visited = 0usize;
    let mut stack: Vec<Vec<OsString>> = vec![Vec::new()];
    'walk: while let Some(below) = stack.pop() {
        let mut dir = root.to_path_buf();
        dir.extend(&below);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > SEARCH_ENTRIES || started.elapsed() > SEARCH_TIME {
                break 'walk;
            }
            let name = entry.file_name();
            let text = name.to_string_lossy();
            if text.starts_with('.')
                || matches!(text.as_ref(), "node_modules" | "target" | "dist" | "vendor")
            {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let mut parts = below.clone();
            parts.push(name.clone());
            if kind.is_dir() {
                if parts.len() < SEARCH_DEPTH {
                    stack.push(parts);
                }
            } else if kind.is_file()
                && let Some(slots) = by_name.get(&key(&name))
            {
                for &slot in slots {
                    let wanted = &wanted[slot].1;
                    let ends = parts.len() >= wanted.len()
                        && parts[parts.len() - wanted.len()..]
                            .iter()
                            .zip(wanted)
                            .all(|(have, want)| same(have, want));
                    // Two are enough to know it is ambiguous.
                    if ends && matches[slot].len() < 2 {
                        matches[slot].push(parts.clone());
                    }
                }
            }
        }
    }
    matches
        .into_iter()
        .enumerate()
        .filter(|(_, found)| found.len() == 1)
        .filter_map(|(slot, mut found)| {
            let parts = found.pop()?;
            // Checked like a direct reference, so the answer holds as the walk saw it.
            walk(root, &parts).map(|file| (wanted[slot].0, file))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    fn names(folder: &Folder, references: &[&str]) -> Vec<Option<String>> {
        let references: Vec<String> = references.iter().map(|item| (*item).to_owned()).collect();
        resolve(folder, &references)
            .into_iter()
            .map(|found| found.map(|found| found.relative))
            .collect()
    }

    #[test]
    fn references_name_files_inside_the_folder_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        touch(&root.join("src/main.rs"));
        touch(&root.join("README.md"));
        touch(&temp.path().join("outside.rs"));
        let folder = Folder::new(&root).unwrap();
        let absolute = crate::fsutil::plain_path(&root.canonicalize().unwrap())
            .join("src")
            .join("main.rs");
        let absolute = absolute.to_str().unwrap().to_owned();
        let found = names(
            &folder,
            &[
                "src/main.rs",
                "./src/main.rs",
                "README.md",
                &absolute,
                "b/src/main.rs",
                "src/missing.rs",
                "src",
                "../outside.rs",
                "src/../README.md",
                "",
            ],
        );
        assert_eq!(
            found,
            [
                Some("src/main.rs".to_owned()),
                Some("src/main.rs".to_owned()),
                Some("README.md".to_owned()),
                Some("src/main.rs".to_owned()),
                Some("src/main.rs".to_owned()),
                None,
                None,
                None,
                None,
                None,
            ]
        );
        // An absolute path elsewhere is refused by its spelling alone.
        let outside = temp.path().join("outside.rs");
        assert_eq!(names(&folder, &[outside.to_str().unwrap()]), [None]);
    }

    #[test]
    fn bare_names_and_partial_paths_name_the_one_file_they_end_with() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        touch(&root.join("apps/desktop/src/components/chat/ChatView.tsx"));
        touch(&root.join("apps/desktop/src/lib/index.ts"));
        touch(&root.join("crates/core/src/index.ts"));
        touch(&root.join("node_modules/pkg/only.js"));
        touch(&root.join(".github/workflows/build.yml"));
        let folder = Folder::new(root).unwrap();
        assert_eq!(
            names(
                &folder,
                &[
                    "ChatView.tsx",
                    "chat/ChatView.tsx",
                    "components/ChatView.tsx",
                    "index.ts",
                    "lib/index.ts",
                    "only.js",
                    "build.yml",
                    ".github/workflows/build.yml",
                ]
            ),
            [
                Some("apps/desktop/src/components/chat/ChatView.tsx".to_owned()),
                Some("apps/desktop/src/components/chat/ChatView.tsx".to_owned()),
                None,
                // Two files end with it: neither is meant for certain.
                None,
                Some("apps/desktop/src/lib/index.ts".to_owned()),
                // Dependencies and dot folders are not searched, only named.
                None,
                None,
                Some(".github/workflows/build.yml".to_owned()),
            ]
        );
        // An anchored reference is never searched for.
        assert_eq!(names(&folder, &["./ChatView.tsx"]), [None]);
    }

    #[cfg(unix)]
    #[test]
    fn links_inside_the_folder_are_not_followed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        touch(&root.join("real/file.rs"));
        touch(&temp.path().join("secret/file.rs"));
        std::os::unix::fs::symlink(temp.path().join("secret"), root.join("linked")).unwrap();
        std::os::unix::fs::symlink(root.join("real/file.rs"), root.join("alias.rs")).unwrap();
        let folder = Folder::new(&root).unwrap();
        assert_eq!(
            names(&folder, &["real/file.rs", "linked/file.rs", "alias.rs"]),
            [Some("real/file.rs".to_owned()), None, None]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_spellings_reach_the_same_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Project");
        touch(&root.join("src/main.rs"));
        let folder = Folder::new(&root).unwrap();
        let plain = crate::fsutil::plain_path(&root.canonicalize().unwrap());
        let plain = plain.to_str().unwrap().to_owned();
        let forward = format!("{}/src/main.rs", plain.replace('\\', "/"));
        let upper = format!("{}\\SRC\\main.rs", plain.to_uppercase());
        let bash = format!(
            "/{}/{}/src/main.rs",
            plain[..1].to_lowercase(),
            plain[3..].replace('\\', "/")
        );
        let found = names(
            &folder,
            &[
                &forward,
                &upper,
                &bash,
                r"src\main.rs",
                "src/main.rs:stream",
                r"\\server\share\src\main.rs",
                r"C:src\main.rs",
            ],
        );
        assert_eq!(found[0].as_deref(), Some("src/main.rs"));
        assert_eq!(found[1].as_deref(), Some("SRC/main.rs"));
        assert_eq!(found[2].as_deref(), Some("src/main.rs"));
        assert_eq!(found[3].as_deref(), Some("src/main.rs"));
        assert_eq!(&found[4..], [None, None, None]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_junctions_are_not_followed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        touch(&root.join("real/file.rs"));
        touch(&temp.path().join("secret/file.rs"));
        // mklink /J needs no privilege, unlike a symbolic link.
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.join("linked"))
            .arg(temp.path().join("secret"))
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        let folder = Folder::new(&root).unwrap();
        assert_eq!(
            names(&folder, &["linked/file.rs", "file.rs"]),
            // The search does not enter the junction either, so only the real file matches.
            [None, Some("real/file.rs".to_owned())]
        );
    }

    #[test]
    fn places_count_from_one_and_the_preferred_editor_wins_when_installed() {
        assert_eq!(position(None, Some(4)), None);
        assert_eq!(position(Some(0), Some(4)), None);
        assert_eq!(
            position(Some(12), Some(0)),
            Some(Position {
                line: 12,
                column: None
            })
        );
        let detected = |editor| Detected {
            editor,
            opener: editors::Opener::Url,
            file: PathBuf::from("editor"),
        };
        let found = [
            detected(ProjectEditor::Cursor),
            detected(ProjectEditor::Zed),
        ];
        assert_eq!(
            choose(&found, Some(ProjectEditor::Zed)).map(|item| item.editor),
            Some(ProjectEditor::Zed)
        );
        assert_eq!(
            choose(&found, Some(ProjectEditor::GoLand)).map(|item| item.editor),
            Some(ProjectEditor::Cursor)
        );
        assert!(choose(&[], Some(ProjectEditor::Zed)).is_none());
    }
}
