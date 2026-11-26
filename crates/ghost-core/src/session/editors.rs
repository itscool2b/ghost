//! Metadata-only detection of installed editors, and the exact way each one opens a folder or a
//! file at a line.
//!
//! Only standard install locations are checked, by file metadata; no editor configuration, state
//! or credential is read. On Windows every editor opens through the executable found here, given
//! the folder or file as its argument: Cursor and the VS Code builds read a file's place as
//! `--goto path:line:column` and take it without the confirmation their URL handler asks for
//! every time. On macOS Cursor and the VS Code builds open through their registered URL handler
//! (which asks, until the person allows local paths), and Zed and the JetBrains IDEs through their
//! application bundle (`/usr/bin/open -a`). A file at a line goes to Zed as `path:line:column`
//! (on macOS through the bundle's own `cli`), and to a JetBrains IDE as `--line` and
//! `--column` before the path (on macOS to the bundle's executable itself, since Launch Services
//! drops arguments for a running app). A JetBrains Toolbox shell script on Windows is read (a few
//! KiB at most) only for the absolute path of the IDE executable it starts; the script itself never
//! runs. Custom installs stay manual.
use ghost_process::ResolvedEnv;
use ghost_protocol::project_tools::ProjectEditor;
use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Platform {
    Windows,
    MacOs,
    Linux,
}

impl Platform {
    /// The release targets Windows, macOS and Linux; elsewhere nothing is detected.
    pub(super) fn current() -> Option<Platform> {
        if cfg!(windows) {
            Some(Platform::Windows)
        } else if cfg!(target_os = "macos") {
            Some(Platform::MacOs)
        } else if cfg!(target_os = "linux") {
            Some(Platform::Linux)
        } else {
            None
        }
    }
}

/// Where standard installs live. Every root is absolute.
#[derive(Debug, Default)]
pub(super) struct Roots {
    /// `%LOCALAPPDATA%`: per-user installs, Toolbox IDEs and Toolbox scripts.
    pub local_app_data: Option<PathBuf>,
    /// `%ProgramFiles%` and `%ProgramFiles(x86)%`.
    pub program_files: Vec<PathBuf>,
    /// `/Applications` and `~/Applications` (where JetBrains Toolbox installs on macOS).
    pub applications: Vec<PathBuf>,
    /// Linux: the system root packages install under (`/`), and the home folder.
    pub system: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

impl Roots {
    pub(super) fn from_env(platform: Platform, env: &ResolvedEnv) -> Roots {
        let var = |name: &str| {
            env.vars
                .get(&name.into())
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        match platform {
            Platform::Windows => Roots {
                local_app_data: var("LOCALAPPDATA"),
                program_files: ["ProgramFiles", "ProgramFiles(x86)"]
                    .into_iter()
                    .filter_map(var)
                    .collect(),
                applications: Vec::new(),
                ..Roots::default()
            },
            Platform::MacOs => Roots {
                local_app_data: None,
                program_files: Vec::new(),
                applications: std::iter::once(PathBuf::from("/Applications"))
                    .chain(var("HOME").map(|home| home.join("Applications")))
                    .collect(),
                ..Roots::default()
            },
            Platform::Linux => Roots {
                system: Some(PathBuf::from("/")),
                home: var("HOME"),
                ..Roots::default()
            },
        }
    }
}

/// How a detected editor opens a folder or a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Opener {
    /// macOS, Cursor and the VS Code builds: the registered `scheme://file/<path>` URL handler.
    Url,
    /// Windows: this executable, with the folder or file as its argument.
    Executable(PathBuf),
    /// macOS: `/usr/bin/open -a <bundle> <folder>`.
    Bundle(PathBuf),
}

/// One detected editor. `file` is what a review binds by identity: the executable itself, or the
/// bundle's main executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Detected {
    pub editor: ProjectEditor,
    pub opener: Opener,
    pub file: PathBuf,
}

/// A place in a file, counted from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Position {
    pub line: u32,
    pub column: Option<u32>,
}

/// What an editor opens: a folder, or a file, at a position when one is known. The path is
/// always absolute, so it can never read as an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target<'a> {
    Folder(&'a str),
    File {
        path: &'a str,
        position: Option<Position>,
    },
}

/// How a launch is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Command {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// `/usr/bin/open` answers once Launch Services has the request, so its status is the
    /// receipt. Anything else may be the editor itself, which Ghost never waits for.
    pub wait: bool,
}

/// `path:line:column`, as Zed and the VS Code builds read a place on their command line.
fn colon_place(path: &str, position: Position) -> OsString {
    match position.column {
        Some(column) => format!("{path}:{}:{column}", position.line),
        None => format!("{path}:{}", position.line),
    }
    .into()
}

/// `--line N [--column N] path`, as JetBrains launchers read a place.
fn jetbrains_place(path: &str, position: Position) -> Vec<OsString> {
    let mut args = vec!["--line".into(), position.line.to_string().into()];
    if let Some(column) = position.column {
        args.extend(["--column".into(), column.to_string().into()]);
    }
    args.push(path.into());
    args
}

/// The program and arguments that open `target` in `detected`, or `None` for a URL handler.
pub(super) fn command(detected: &Detected, target: Target<'_>) -> Option<Command> {
    let (path, position) = match target {
        Target::Folder(path) => (path, None),
        Target::File { path, position } => (path, position),
    };
    let zed = detected.editor == ProjectEditor::Zed;
    let code = matches!(
        detected.editor,
        ProjectEditor::Cursor | ProjectEditor::VsCode | ProjectEditor::VsCodeInsiders
    );
    match (&detected.opener, position) {
        (Opener::Url, _) => None,
        (Opener::Executable(executable), None) => Some(Command {
            program: executable.clone(),
            args: vec![path.into()],
            wait: false,
        }),
        (Opener::Executable(executable), Some(position)) => Some(Command {
            program: executable.clone(),
            args: if code {
                vec!["--goto".into(), colon_place(path, position)]
            } else if zed {
                vec![colon_place(path, position)]
            } else {
                jetbrains_place(path, position)
            },
            wait: false,
        }),
        (Opener::Bundle(bundle), None) => Some(Command {
            program: PathBuf::from("/usr/bin/open"),
            args: vec!["-a".into(), bundle.into(), path.into()],
            wait: true,
        }),
        // Launch Services passes no arguments to an app that already runs, so a place goes to
        // the bundle's own command-line program, which hands it to the running editor.
        (Opener::Bundle(bundle), Some(position)) if zed => Some(Command {
            program: bundle.join("Contents/MacOS/cli"),
            args: vec![colon_place(path, position)],
            wait: false,
        }),
        (Opener::Bundle(_), Some(position)) => Some(Command {
            program: detected.file.clone(),
            args: jetbrains_place(path, position),
            wait: false,
        }),
    }
}

/// A JetBrains IDE: its Windows executable, macOS executable, Toolbox script name, the folder
/// names its installers use on Windows, and its bundle names on macOS.
struct JetBrains {
    editor: ProjectEditor,
    windows_exe: &'static str,
    mac_exe: &'static str,
    script: &'static str,
    windows_dirs: &'static [&'static str],
    bundles: &'static [&'static str],
}

const JETBRAINS: &[JetBrains] = &[
    JetBrains {
        editor: ProjectEditor::IntellijIdea,
        windows_exe: "idea64.exe",
        mac_exe: "idea",
        script: "idea",
        windows_dirs: &[
            "IntelliJ IDEA Ultimate",
            "IntelliJ IDEA",
            "IntelliJ IDEA Community Edition",
        ],
        bundles: &[
            "IntelliJ IDEA",
            "IntelliJ IDEA Ultimate",
            "IntelliJ IDEA CE",
            "IntelliJ IDEA Community Edition",
        ],
    },
    JetBrains {
        editor: ProjectEditor::WebStorm,
        windows_exe: "webstorm64.exe",
        mac_exe: "webstorm",
        script: "webstorm",
        windows_dirs: &["WebStorm"],
        bundles: &["WebStorm"],
    },
    JetBrains {
        editor: ProjectEditor::PyCharm,
        windows_exe: "pycharm64.exe",
        mac_exe: "pycharm",
        script: "pycharm",
        windows_dirs: &[
            "PyCharm Professional",
            "PyCharm",
            "PyCharm Community",
            "PyCharm Community Edition",
        ],
        bundles: &[
            "PyCharm",
            "PyCharm Professional Edition",
            "PyCharm CE",
            "PyCharm Community Edition",
        ],
    },
    JetBrains {
        editor: ProjectEditor::RustRover,
        windows_exe: "rustrover64.exe",
        mac_exe: "rustrover",
        script: "rustrover",
        windows_dirs: &["RustRover"],
        bundles: &["RustRover"],
    },
    JetBrains {
        editor: ProjectEditor::GoLand,
        windows_exe: "goland64.exe",
        mac_exe: "goland",
        script: "goland",
        windows_dirs: &["GoLand"],
        bundles: &["GoLand"],
    },
    JetBrains {
        editor: ProjectEditor::CLion,
        windows_exe: "clion64.exe",
        mac_exe: "clion",
        script: "clion",
        windows_dirs: &["CLion"],
        bundles: &["CLion"],
    },
];

/// Every installed editor, each once, in a stable order; the first location found wins.
pub(super) fn detect(platform: Platform, roots: &Roots) -> Vec<Detected> {
    let mut found: Vec<Detected> = Vec::new();
    for candidate in candidates(platform, roots) {
        if !found.iter().any(|item| item.editor == candidate.editor) && candidate.file.is_file() {
            found.push(candidate);
        }
    }
    found
}

fn candidates(platform: Platform, roots: &Roots) -> Vec<Detected> {
    let mut out = Vec::new();
    match platform {
        Platform::Windows => {
            let programs = roots
                .local_app_data
                .as_ref()
                .map(|root| root.join("Programs"));
            let installs: Vec<&PathBuf> = programs.iter().chain(&roots.program_files).collect();
            for root in &installs {
                for (editor, file) in [
                    (ProjectEditor::Cursor, "cursor/Cursor.exe"),
                    (ProjectEditor::VsCode, "Microsoft VS Code/Code.exe"),
                    (
                        ProjectEditor::VsCodeInsiders,
                        "Microsoft VS Code Insiders/Code - Insiders.exe",
                    ),
                ] {
                    out.push(executable(editor, root.join(file)));
                }
            }
            for root in &installs {
                out.push(executable(
                    ProjectEditor::Zed,
                    root.join("Zed").join("Zed.exe"),
                ));
            }
            for product in JETBRAINS {
                // JetBrains Toolbox 2 installs each IDE under %LOCALAPPDATA%\Programs.
                if let Some(programs) = &programs {
                    for dir in product.windows_dirs {
                        out.push(executable(
                            product.editor,
                            programs.join(dir).join("bin").join(product.windows_exe),
                        ));
                    }
                }
                // The standalone installer uses a versioned folder under Program Files\JetBrains.
                for root in &roots.program_files {
                    for dir in versioned(&root.join("JetBrains"), product.windows_dirs) {
                        out.push(executable(
                            product.editor,
                            dir.join("bin").join(product.windows_exe),
                        ));
                    }
                }
                // An older Toolbox keeps versioned folders; its shell script names the executable.
                if let Some(local) = &roots.local_app_data {
                    let script = local
                        .join("JetBrains/Toolbox/scripts")
                        .join(format!("{}.cmd", product.script));
                    if let Some(file) = script_target(&script, product.windows_exe) {
                        out.push(executable(product.editor, file));
                    }
                }
            }
        }
        Platform::Linux => {
            // The VS Code builds' and Cursor's own executables from their .deb and .rpm packages,
            // started like Windows' with --goto; and Zed's command line from its install script.
            // Snaps and Flatpaks run confined, and JetBrains IDEs have no standard place: they
            // stay manual.
            if let Some(system) = &roots.system {
                for (editor, file) in [
                    (ProjectEditor::Cursor, "usr/share/cursor/cursor"),
                    (ProjectEditor::VsCode, "usr/share/code/code"),
                    (
                        ProjectEditor::VsCodeInsiders,
                        "usr/share/code-insiders/code-insiders",
                    ),
                ] {
                    out.push(executable(editor, system.join(file)));
                }
            }
            if let Some(home) = &roots.home {
                out.push(executable(
                    ProjectEditor::Zed,
                    home.join(".local/zed.app/bin/zed"),
                ));
            }
        }
        Platform::MacOs => {
            for root in &roots.applications {
                for (editor, bundle, file) in [
                    (ProjectEditor::Cursor, "Cursor.app", "Cursor"),
                    (ProjectEditor::VsCode, "Visual Studio Code.app", "Electron"),
                    (
                        ProjectEditor::VsCodeInsiders,
                        "Visual Studio Code - Insiders.app",
                        "Electron",
                    ),
                ] {
                    out.push(url(
                        editor,
                        root.join(bundle).join("Contents/MacOS").join(file),
                    ));
                }
            }
            for root in &roots.applications {
                for name in ["Zed.app", "Zed Preview.app"] {
                    out.push(bundle(ProjectEditor::Zed, root.join(name), "zed"));
                }
            }
            for product in JETBRAINS {
                for root in &roots.applications {
                    for name in product.bundles {
                        out.push(bundle(
                            product.editor,
                            root.join(format!("{name}.app")),
                            product.mac_exe,
                        ));
                    }
                }
            }
        }
    }
    out
}

fn url(editor: ProjectEditor, file: PathBuf) -> Detected {
    Detected {
        editor,
        opener: Opener::Url,
        file,
    }
}

fn executable(editor: ProjectEditor, file: PathBuf) -> Detected {
    Detected {
        editor,
        opener: Opener::Executable(file.clone()),
        file,
    }
}

fn bundle(editor: ProjectEditor, app: PathBuf, executable: &str) -> Detected {
    Detected {
        editor,
        file: app.join("Contents/MacOS").join(executable),
        opener: Opener::Bundle(app),
    }
}

/// Folders named `<name> <version>` (or just `<name>`) for any of `names`, newest name first.
fn versioned(parent: &Path, names: &[&str]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|entry| {
            names.iter().any(|name| {
                entry == name
                    || entry
                        .strip_prefix(name)
                        .and_then(|rest| rest.strip_prefix(' '))
                        .is_some_and(|version| version.starts_with(|c: char| c.is_ascii_digit()))
            })
        })
        .take(64)
        .collect();
    found.sort_by(|left, right| right.cmp(left));
    found.into_iter().map(|name| parent.join(name)).collect()
}

/// The absolute executable a Toolbox `.cmd` script starts, when it names `exe` in quotes. The
/// script is read, never run, and only when it is a small regular file.
fn script_target(script: &Path, exe: &str) -> Option<PathBuf> {
    let metadata = std::fs::symlink_metadata(script).ok()?;
    if !metadata.is_file() || metadata.len() > 8 * 1024 {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(script)
        .ok()?
        .take(8 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    text.split('"')
        .skip(1)
        .step_by(2)
        .map(PathBuf::from)
        .find(|path| {
            path.is_absolute()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case(exe))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    fn editors(found: &[Detected]) -> Vec<ProjectEditor> {
        found.iter().map(|item| item.editor).collect()
    }

    #[test]
    fn windows_detects_editors_and_starts_each_from_its_executable() {
        let local = tempfile::tempdir().unwrap();
        let program_files = tempfile::tempdir().unwrap();
        let roots = Roots {
            local_app_data: Some(local.path().into()),
            program_files: vec![program_files.path().into()],
            applications: Vec::new(),
            ..Roots::default()
        };
        assert!(detect(Platform::Windows, &roots).is_empty());
        let programs = local.path().join("Programs");
        touch(&programs.join("cursor/Cursor.exe"));
        touch(&programs.join("Zed/Zed.exe"));
        touch(&programs.join("IntelliJ IDEA Ultimate/bin/idea64.exe"));
        touch(&programs.join("RustRover/bin/rustrover64.exe"));
        let found = detect(Platform::Windows, &roots);
        assert_eq!(
            editors(&found),
            [
                ProjectEditor::Cursor,
                ProjectEditor::Zed,
                ProjectEditor::IntellijIdea,
                ProjectEditor::RustRover
            ]
        );
        assert_eq!(
            found[0].opener,
            Opener::Executable(programs.join("cursor/Cursor.exe"))
        );
        assert_eq!(
            found[1].opener,
            Opener::Executable(programs.join("Zed").join("Zed.exe"))
        );
        let idea = programs.join("IntelliJ IDEA Ultimate/bin/idea64.exe");
        assert_eq!(
            command(&found[2], Target::Folder(r"C:\work\my project")),
            Some(Command {
                program: idea.clone(),
                args: vec![OsString::from(r"C:\work\my project")],
                wait: false,
            })
        );
        assert_eq!(
            command(&found[0], Target::Folder(r"C:\work")).unwrap().args,
            [OsString::from(r"C:\work")]
        );
        // A file at a place: JetBrains takes --line and --column before the path, Zed one
        // `path:line:column` argument and Cursor `--goto` before it; without a place each takes
        // the path alone.
        let place = Some(Position {
            line: 42,
            column: Some(7),
        });
        let file = r"C:\work\src\main.rs";
        assert_eq!(
            command(
                &found[2],
                Target::File {
                    path: file,
                    position: place
                }
            )
            .unwrap()
            .args,
            ["--line", "42", "--column", "7", file].map(OsString::from)
        );
        assert_eq!(
            command(
                &found[2],
                Target::File {
                    path: file,
                    position: Some(Position {
                        line: 3,
                        column: None
                    })
                }
            )
            .unwrap()
            .args,
            ["--line", "3", file].map(OsString::from)
        );
        assert_eq!(
            command(
                &found[1],
                Target::File {
                    path: file,
                    position: place
                }
            ),
            Some(Command {
                program: programs.join("Zed").join("Zed.exe"),
                args: vec![OsString::from(format!("{file}:42:7"))],
                wait: false,
            })
        );
        assert_eq!(
            command(
                &found[1],
                Target::File {
                    path: file,
                    position: None
                }
            )
            .unwrap()
            .args,
            [OsString::from(file)]
        );
        assert_eq!(
            command(
                &found[0],
                Target::File {
                    path: file,
                    position: place
                }
            ),
            Some(Command {
                program: programs.join("cursor/Cursor.exe"),
                args: vec![
                    OsString::from("--goto"),
                    OsString::from(format!("{file}:42:7"))
                ],
                wait: false,
            })
        );
    }

    #[test]
    fn windows_finds_the_newest_standalone_install_and_ignores_lookalike_folders() {
        let program_files = tempfile::tempdir().unwrap();
        let jetbrains = program_files.path().join("JetBrains");
        let roots = Roots {
            local_app_data: None,
            program_files: vec![program_files.path().into()],
            applications: Vec::new(),
            ..Roots::default()
        };
        // A folder that only starts with the name, or lacks its executable, is not an install.
        touch(&jetbrains.join("GoLand Helper 2025.1/bin/goland64.exe"));
        fs::create_dir_all(jetbrains.join("CLion 2025.2/bin")).unwrap();
        touch(&jetbrains.join("PyCharm Community Edition 2024.3/bin/pycharm64.exe"));
        touch(&jetbrains.join("WebStorm 2024.3.5/bin/webstorm64.exe"));
        touch(&jetbrains.join("WebStorm 2025.2.1/bin/webstorm64.exe"));
        let found = detect(Platform::Windows, &roots);
        assert_eq!(
            editors(&found),
            [ProjectEditor::WebStorm, ProjectEditor::PyCharm]
        );
        assert_eq!(
            found[0].file,
            jetbrains.join("WebStorm 2025.2.1/bin/webstorm64.exe")
        );
    }

    #[test]
    fn windows_reads_a_toolbox_script_only_for_the_executable_it_names() {
        let local = tempfile::tempdir().unwrap();
        let scripts = local.path().join("JetBrains/Toolbox/scripts");
        let ide = local
            .path()
            .join("JetBrains/Toolbox/apps/Goland/ch-0/252.1/bin/goland64.exe");
        touch(&ide);
        fs::create_dir_all(&scripts).unwrap();
        fs::write(
            scripts.join("goland.cmd"),
            format!("@echo off\r\nstart \"\" \"{}\" %*\r\n", ide.display()),
        )
        .unwrap();
        // A script naming another program, or a missing one, adds nothing.
        fs::write(
            scripts.join("clion.cmd"),
            "@echo off\r\nstart \"\" \"C:\\Windows\\System32\\cmd.exe\" %*\r\n",
        )
        .unwrap();
        fs::write(
            scripts.join("webstorm.cmd"),
            "@echo off\r\nstart \"\" \"C:\\missing\\bin\\webstorm64.exe\" %*\r\n",
        )
        .unwrap();
        // An oversized script is not read.
        fs::write(
            scripts.join("pycharm.cmd"),
            format!("{}\"{}\"", " ".repeat(9000), ide.display()),
        )
        .unwrap();
        let roots = Roots {
            local_app_data: Some(local.path().into()),
            program_files: Vec::new(),
            applications: Vec::new(),
            ..Roots::default()
        };
        let found = detect(Platform::Windows, &roots);
        assert_eq!(editors(&found), [ProjectEditor::GoLand]);
        assert_eq!(found[0].opener, Opener::Executable(ide));
    }

    #[test]
    fn macos_detects_bundles_and_opens_them_through_launch_services() {
        let system = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = Roots {
            local_app_data: None,
            program_files: Vec::new(),
            applications: vec![system.path().into(), home.path().join("Applications")],
            ..Roots::default()
        };
        touch(
            &system
                .path()
                .join("Visual Studio Code.app/Contents/MacOS/Electron"),
        );
        touch(&system.path().join("Zed.app/Contents/MacOS/zed"));
        touch(
            &home
                .path()
                .join("Applications/PyCharm Professional Edition.app/Contents/MacOS/pycharm"),
        );
        touch(
            &home
                .path()
                .join("Applications/CLion.app/Contents/MacOS/clion"),
        );
        // A bundle without its main executable is not an install.
        fs::create_dir_all(system.path().join("GoLand.app/Contents/MacOS")).unwrap();
        let found = detect(Platform::MacOs, &roots);
        assert_eq!(
            editors(&found),
            [
                ProjectEditor::VsCode,
                ProjectEditor::Zed,
                ProjectEditor::PyCharm,
                ProjectEditor::CLion
            ]
        );
        // Built as detection builds it, so the arguments match byte for byte on every platform.
        let clion = home.path().join("Applications").join("CLion.app");
        assert_eq!(found[3].opener, Opener::Bundle(clion.clone()));
        assert_eq!(
            command(&found[3], Target::Folder("/Users/me/my project")),
            Some(Command {
                program: PathBuf::from("/usr/bin/open"),
                args: vec![
                    OsString::from("-a"),
                    clion.clone().into_os_string(),
                    OsString::from("/Users/me/my project")
                ],
                wait: true,
            })
        );
        // Visual Studio Code opens through its URL handler on macOS, a place and all.
        assert_eq!(found[0].opener, Opener::Url);
        assert_eq!(
            command(
                &found[0],
                Target::File {
                    path: "/Users/me/a.ts",
                    position: None
                }
            ),
            None
        );
        // A file without a place opens as a folder does; with one, the bundle's executable takes
        // it, and Zed's bundle hands it to its cli.
        let file = "/Users/me/my project/src/main.c";
        assert_eq!(
            command(
                &found[3],
                Target::File {
                    path: file,
                    position: None
                }
            )
            .unwrap()
            .program,
            PathBuf::from("/usr/bin/open")
        );
        let place = Some(Position {
            line: 9,
            column: None,
        });
        assert_eq!(
            command(
                &found[3],
                Target::File {
                    path: file,
                    position: place
                }
            ),
            Some(Command {
                program: clion.join("Contents/MacOS").join("clion"),
                args: ["--line", "9", file].map(OsString::from).to_vec(),
                wait: false,
            })
        );
        assert_eq!(
            command(
                &found[1],
                Target::File {
                    path: file,
                    position: place
                }
            ),
            Some(Command {
                program: system.path().join("Zed.app").join("Contents/MacOS/cli"),
                args: vec![OsString::from(format!("{file}:9"))],
                wait: false,
            })
        );
        // The same roots on Windows find nothing: each platform checks only its own layout.
        assert!(detect(Platform::Windows, &roots).is_empty());
    }

    #[test]
    fn linux_finds_packaged_editors_and_zeds_command_line() {
        let system = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = Roots {
            system: Some(system.path().into()),
            home: Some(home.path().into()),
            ..Roots::default()
        };
        assert!(detect(Platform::Linux, &roots).is_empty());
        touch(&system.path().join("usr/share/code/code"));
        touch(&home.path().join(".local/zed.app/bin/zed"));
        let found = detect(Platform::Linux, &roots);
        assert_eq!(editors(&found), [ProjectEditor::VsCode, ProjectEditor::Zed]);
        let file = "/home/me/project/src/main.rs";
        let place = Some(Position {
            line: 7,
            column: None,
        });
        assert_eq!(
            command(
                &found[0],
                Target::File {
                    path: file,
                    position: place
                }
            ),
            Some(Command {
                program: system.path().join("usr/share/code/code"),
                args: vec![
                    OsString::from("--goto"),
                    OsString::from(format!("{file}:7"))
                ],
                wait: false,
            })
        );
        assert_eq!(
            command(
                &found[1],
                Target::File {
                    path: file,
                    position: place
                }
            )
            .unwrap()
            .args,
            [OsString::from(format!("{file}:7"))]
        );
        // The Windows layout finds nothing in a Linux system root.
        assert!(detect(Platform::Windows, &roots).is_empty());
    }

    #[test]
    fn roots_come_only_from_absolute_variables() {
        let mut vars = ghost_process::EnvMap::new();
        vars.insert("LOCALAPPDATA".into(), "relative/local".into());
        vars.insert("ProgramFiles".into(), r"C:\Program Files".into());
        vars.insert("HOME".into(), "/Users/me".into());
        let env = ResolvedEnv {
            vars,
            source: ghost_process::EnvSource::Fallback {
                reason: "editor roots".into(),
            },
            warnings: Vec::new(),
            resolved_at_ms: 0,
        };
        let windows = Roots::from_env(Platform::Windows, &env);
        assert_eq!(windows.local_app_data, None);
        assert!(windows.applications.is_empty());
        let mac = Roots::from_env(Platform::MacOs, &env);
        assert_eq!(mac.applications[0], PathBuf::from("/Applications"));
        assert!(mac.local_app_data.is_none() && mac.program_files.is_empty());
        if cfg!(windows) {
            assert_eq!(windows.program_files, [PathBuf::from(r"C:\Program Files")]);
        } else {
            assert_eq!(mac.applications[1], PathBuf::from("/Users/me/Applications"));
        }
    }
}
