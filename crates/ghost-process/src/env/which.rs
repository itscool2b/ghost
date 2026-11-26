//! Finding programs on an env's `PATH`.
//!
//! `PATH` is read in order and the first match wins (shadowed installs are M1e's). Empty and
//! relative entries are skipped: they would resolve against whatever folder Ghost runs in.
//! Symlinks are never resolved: agents launch through their stable symlink or shim.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::{EnvKey, EnvMap};

/// `PATHEXT` when the env has none.
#[cfg(windows)]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// The first file on the env's `PATH`, in order, that is executable (on Windows, with each
/// `PATHEXT` extension, or as named when the name already has one). An absolute name is checked
/// as it is.
pub fn find_program(name: &str, env: &EnvMap) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let given = Path::new(name);
    if given.is_absolute() {
        return is_program(given).then(|| given.to_path_buf());
    }
    if given.components().count() != 1 {
        return None;
    }
    #[cfg(windows)]
    let extensions = pathext(env);
    path_dirs(env).find_map(|dir| {
        #[cfg(windows)]
        {
            let has_extension = Path::new(name).extension().is_some_and(|extension| {
                extensions.iter().any(|known| {
                    known
                        .trim_start_matches('.')
                        .eq_ignore_ascii_case(&extension.to_string_lossy())
                })
            });
            if has_extension {
                let candidate = dir.join(name);
                return candidate.is_file().then_some(candidate);
            }
            // Lowercase, as the files are usually named (`node.exe`, `npm.cmd`): Windows matches
            // either way, and the path is then the one a listing shows.
            extensions.iter().find_map(|extension| {
                let candidate = dir.join(format!("{name}{}", extension.to_ascii_lowercase()));
                candidate.is_file().then_some(candidate)
            })
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join(name);
            is_program(&candidate).then_some(candidate)
        }
    })
}

/// [`find_program`] on Unix; on Windows only `<name>.exe`, because the adapters spawn the path they
/// are given and Node refuses to spawn `.cmd` shims without a shell. A name typed with its `.exe`
/// (`PING.EXE`, as Windows people often write it) is that file, not `PING.EXE.exe`.
pub fn find_exe(name: &str, env: &EnvMap) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if name.is_empty() || Path::new(name).components().count() != 1 {
            return None;
        }
        let typed_exe = name.len() > 4
            && name
                .get(name.len() - 4..)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(".exe"));
        let file = if typed_exe {
            name.to_owned()
        } else {
            format!("{name}.exe")
        };
        path_dirs(env).find_map(|dir| {
            let candidate = dir.join(&file);
            candidate.is_file().then_some(candidate)
        })
    }
    #[cfg(not(windows))]
    {
        find_program(name, env)
    }
}

/// The absolute directories of the env's `PATH`, in order.
pub(crate) fn path_dirs(env: &EnvMap) -> impl Iterator<Item = PathBuf> + '_ {
    let value = env
        .get(&EnvKey::from("PATH"))
        .map(|value| value.as_os_str())
        .unwrap_or(OsStr::new(""));
    std::env::split_paths(value).filter(|dir| dir.is_absolute())
}

/// A file this user may run: on Unix any execute bit (symlinks followed for the check only); on
/// Windows any existing file, since the extension decides.
pub(crate) fn is_program(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    // Each system's whole answer in one expression: an early `return false` before a bare `true`
    // is a needless bool on Windows (Clippy, Rust 1.100).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// The env's `PATHEXT` extensions, each with its dot.
#[cfg(windows)]
pub(crate) fn pathext(env: &EnvMap) -> Vec<String> {
    let value = env
        .get(&EnvKey::from("PATHEXT"))
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PATHEXT.to_owned());
    value
        .split(';')
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(|extension| {
            if extension.starts_with('.') {
                extension.to_owned()
            } else {
                format!(".{extension}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn env_with_path(dirs: &[&Path]) -> EnvMap {
        let mut env = EnvMap::new();
        let path = std::env::join_paths(dirs).expect("join");
        env.insert(EnvKey::from("PATH"), path);
        env
    }

    #[cfg(unix)]
    fn program(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, b"#!/bin/sh\n").expect("write");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    #[cfg(unix)]
    #[test]
    fn find_program_follows_path_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (first, second, third) = (
            dir.path().join("a"),
            dir.path().join("b"),
            dir.path().join("c"),
        );
        for folder in [&first, &second, &third] {
            std::fs::create_dir(folder).expect("mkdir");
        }
        // Not executable, a folder, then the two real ones: the earlier real one wins.
        std::fs::write(first.join("hermes"), b"data").expect("write");
        std::fs::create_dir(first.join("claude")).expect("mkdir");
        program(&second.join("hermes"));
        program(&third.join("hermes"));
        program(&third.join("claude"));
        let env = env_with_path(&[&first, &second, &third]);
        assert_eq!(find_program("hermes", &env), Some(second.join("hermes")));
        assert_eq!(find_exe("hermes", &env), Some(second.join("hermes")));
        assert_eq!(find_program("claude", &env), Some(third.join("claude")));
        assert_eq!(find_program("codex", &env), None);
        assert_eq!(find_program("", &env), None);
        assert_eq!(find_program("b/hermes", &env), None, "no relative paths");
        let absolute = second.join("hermes");
        assert_eq!(
            find_program(absolute.to_str().expect("utf-8"), &EnvMap::new()),
            Some(absolute)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_found_but_never_resolved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("versions");
        let bin = dir.path().join("bin");
        std::fs::create_dir(&real).expect("mkdir");
        std::fs::create_dir(&bin).expect("mkdir");
        program(&real.join("2.1.310"));
        std::os::unix::fs::symlink(real.join("2.1.310"), bin.join("claude")).expect("symlink");
        std::os::unix::fs::symlink(real.join("missing"), bin.join("broken")).expect("symlink");
        let env = env_with_path(&[&bin]);
        assert_eq!(find_program("claude", &env), Some(bin.join("claude")));
        assert_eq!(find_program("broken", &env), None);
    }

    #[test]
    fn empty_and_relative_path_entries_are_skipped() {
        let mut env = EnvMap::new();
        let separator = if cfg!(windows) { ";" } else { ":" };
        env.insert(
            EnvKey::from("PATH"),
            OsString::from(format!("{separator}.{separator}relative{separator}")),
        );
        assert_eq!(path_dirs(&env).count(), 0);
        assert_eq!(path_dirs(&EnvMap::new()).count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_pathext_and_exe_only_for_find_exe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shims, exes) = (dir.path().join("npm"), dir.path().join("local"));
        std::fs::create_dir(&shims).expect("mkdir");
        std::fs::create_dir(&exes).expect("mkdir");
        std::fs::write(shims.join("claude.cmd"), b"@echo off").expect("write");
        std::fs::write(exes.join("claude.exe"), b"MZ").expect("write");
        std::fs::write(shims.join("npm.cmd"), b"@echo off").expect("write");
        let mut env = env_with_path(&[&shims, &exes]);
        env.insert(
            EnvKey::from("PATHEXT"),
            OsString::from(".COM;.EXE;.BAT;.CMD"),
        );
        assert_eq!(find_program("claude", &env), Some(shims.join("claude.cmd")));
        assert_eq!(find_exe("claude", &env), Some(exes.join("claude.exe")));
        assert_eq!(find_program("npm", &env), Some(shims.join("npm.cmd")));
        assert_eq!(find_exe("npm", &env), None);
        assert_eq!(find_program("npm.cmd", &env), Some(shims.join("npm.cmd")));
        // Windows names compare without case: `Path` is `PATH`.
        let mut mixed = EnvMap::new();
        mixed.insert(
            EnvKey::from("Path"),
            std::env::join_paths([&exes]).expect("join"),
        );
        assert_eq!(find_exe("claude", &mixed), Some(exes.join("claude.exe")));
        // A name typed with its extension, in any case, is that file.
        assert_eq!(find_exe("claude.exe", &env), Some(exes.join("claude.exe")));
        assert_eq!(find_exe("CLAUDE.EXE", &env), Some(exes.join("CLAUDE.EXE")));
        assert_eq!(find_exe("npm.cmd", &env), None);
        assert_eq!(find_exe(".exe", &env), None);
    }
}
