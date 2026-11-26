//! Resolve reviewed npm package bins without interpreting shell shim contents.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use ghost_process::{EnvMap, find_exe};

const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_VERSION_FILE: u64 = 64;

/// What Pi's own launcher tells the release it starts: the install it belongs to, which Pi's
/// updater and cleanup read.
const PI_MANAGED_INSTALL_ROOT: &str = "PI_MANAGED_INSTALL_ROOT";

fn package(agent: &str) -> Option<(&'static str, &'static str)> {
    match agent {
        "pi" => Some(("@earendil-works/pi-coding-agent", "pi")),
        "omp" => Some(("@oh-my-pi/pi-coding-agent", "omp")),
        "grok" | "grok-stream" => Some(("@xai-official/grok", "grok")),
        // OpenCode's package declares its native executable as the bin (bin/opencode.exe).
        "opencode" | "opencode-v2" => Some(("opencode-ai", "opencode")),
        _ => None,
    }
}

pub(crate) fn is_shim(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        ["cmd", "bat", "ps1"]
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

/// A shim's package entry, and the install root a vendor's own launcher would announce to it.
struct Resolved {
    entry: PathBuf,
    managed_root: Option<PathBuf>,
}

fn resolve(agent: &str, shim: &Path) -> Option<Resolved> {
    let (package, command) = package(agent)?;
    if !is_shim(shim) || !shim.file_stem()?.eq_ignore_ascii_case(command) || !shim.is_file() {
        return None;
    }
    let prefix = shim.parent()?;
    // A project-local .bin must not be treated as a global installation or updated with npm -g.
    if prefix.file_name().is_some_and(|name| name == ".bin") {
        return None;
    }
    if let Ok(root) = std::fs::canonicalize(prefix.join("node_modules").join(package)) {
        let entry = package_bin(&root, package, command)?;
        return Some(Resolved {
            entry,
            managed_root: None,
        });
    }
    if agent != "pi" {
        return None;
    }
    let (install, root) = managed_pi(prefix, package)?;
    Some(Resolved {
        entry: package_bin(&root, package, command)?,
        managed_root: Some(install),
    })
}

/// Pi's own installer (pi.dev) keeps each release in `<agent>/install/releases/<version>` and
/// starts the one `current-version` names through `<agent>/bin/pi-launcher.js`. Its marker and
/// version file are read as data, like a package manifest; neither the launcher nor the shim runs.
fn managed_pi(bin: &Path, package: &str) -> Option<(PathBuf, PathBuf)> {
    if !bin.file_name()?.eq_ignore_ascii_case("bin") {
        return None;
    }
    let install = bin.parent()?.join("install");
    let marker: serde_json::Value = serde_json::from_slice(&read_bounded(
        &install.join("managed-install.json"),
        MAX_MANIFEST,
    )?)
    .ok()?;
    if marker.get("kind")?.as_str()? != "pi-managed-install"
        || marker.get("layout")?.as_str()? != "releases-v1"
    {
        return None;
    }
    let version = String::from_utf8(read_bounded(
        &install.join("current-version"),
        MAX_VERSION_FILE,
    )?)
    .ok()?;
    let version = version.trim();
    // The launcher's own rule: one plain path segment.
    if version.is_empty()
        || version == "."
        || version == ".."
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
    {
        return None;
    }
    let releases = std::fs::canonicalize(install.join("releases")).ok()?;
    let root =
        std::fs::canonicalize(releases.join(version).join("node_modules").join(package)).ok()?;
    root.starts_with(&releases).then_some((install, root))
}

fn read_bounded(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

/// Only package.json is read. A relative bin must remain inside this exact reviewed package.
fn package_bin(root: &Path, package: &str, command: &str) -> Option<PathBuf> {
    let manifest: serde_json::Value =
        serde_json::from_slice(&read_bounded(&root.join("package.json"), MAX_MANIFEST)?).ok()?;
    if manifest.get("name")?.as_str()? != package {
        return None;
    }
    let bin = manifest.get("bin")?;
    let relative = bin.as_str().or_else(|| bin.get(command)?.as_str())?;
    if relative.is_empty()
        || relative.contains(['\\', '\0', ':'])
        || Path::new(relative)
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let entry = std::fs::canonicalize(root.join(relative)).ok()?;
    if !entry.starts_with(root) || !entry.is_file() {
        return None;
    }
    entry
        .extension()
        .is_some_and(|extension| {
            ["js", "mjs", "cjs", "exe"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
        .then_some(entry)
}

/// Win32's MAX_PATH less the terminating NUL.
const MAX_PLAIN_PATH: usize = 259;

/// The plain spelling of a Windows verbatim drive path (`\\?\C:\x` is `C:\x`), which
/// `std::fs::canonicalize` returns and Node cannot run as its main script ("EISDIR: illegal
/// operation on a directory, lstat 'C:'", Node 24). The prefix stays whenever the plain spelling
/// could name another file: past MAX_PATH, on a share, or with a part Win32 parsing would change.
fn plain(path: PathBuf) -> PathBuf {
    let Some(rest) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) else {
        return path;
    };
    let bytes = rest.as_bytes();
    let drive =
        bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    if drive
        && rest.encode_utf16().count() <= MAX_PLAIN_PATH
        && rest[3..].split('\\').all(plain_part)
    {
        PathBuf::from(rest)
    } else {
        path
    }
}

/// Whether one path part reads the same with and without the verbatim prefix.
fn plain_part(part: &str) -> bool {
    if part.is_empty()
        || part == "."
        || part == ".."
        || part.ends_with(['.', ' '])
        || part
            .chars()
            .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*'))
    {
        return false;
    }
    // A DOS device name stays one with an extension: `CON.txt`.
    let stem = part
        .split('.')
        .next()
        .unwrap_or(part)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|rest| {
            rest.chars().count() == 1
                && rest
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
        })
    };
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !numbered("COM")
        && !numbered("LPT")
}

/// The package entry a shim stands for.
pub(crate) fn entry(agent: &str, shim: &Path) -> Option<PathBuf> {
    if agent == "cursor" {
        return crate::cursor_entry::command(shim).map(|(_, entry)| entry);
    }
    resolve(agent, shim).map(|resolved| resolved.entry)
}

/// What runs for a launch path: the program, the package entry it takes first, and the variables
/// the vendor's own launcher would have set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Command {
    pub(crate) program: PathBuf,
    pub(crate) prefix: Vec<OsString>,
    pub(crate) set_env: Vec<(OsString, OsString)>,
}

/// The stable shim identifies an installation, but the launched command never invokes its shell.
pub(crate) fn command(agent: &str, path: &Path, env: &EnvMap) -> Result<Command, String> {
    let direct = |program: PathBuf, prefix: Vec<OsString>| Command {
        program,
        prefix,
        set_env: Vec::new(),
    };
    if !cfg!(windows) || !is_shim(path) {
        return Ok(direct(path.to_owned(), Vec::new()));
    }
    if agent == "cursor" {
        return crate::cursor_entry::command(path)
            .map(|(node, entry)| direct(node, vec![entry.into_os_string()]))
            .ok_or_else(|| {
                "Cursor's Windows runtime is incomplete. Reinstall its official CLI distribution."
                    .to_owned()
            });
    }
    let resolved = resolve(agent, path).ok_or_else(|| {
        "The shell shim has no supported package bin. Install the vendor's executable or reviewed npm package."
            .to_owned()
    })?;
    let set_env = resolved
        .managed_root
        .map(|root| vec![(PI_MANAGED_INSTALL_ROOT.into(), root.into_os_string())])
        .unwrap_or_default();
    // What runs takes the plain spelling of the entry detection checked.
    let entry = plain(resolved.entry);
    if entry
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Ok(Command {
            program: entry,
            prefix: Vec::new(),
            set_env,
        });
    }
    let node = find_exe("node", env)
        .ok_or_else(|| "Node.js was not found for this npm package entry.".to_owned())?;
    Ok(Command {
        program: node,
        prefix: vec![entry.into_os_string()],
        set_env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_reviewed_package_bin_is_resolved_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("pi.cmd");
        std::fs::write(&shim, "never interpreted").unwrap();
        let root = dir
            .path()
            .join("node_modules/@earendil-works/pi-coding-agent");
        std::fs::create_dir_all(root.join("dist")).unwrap();
        let target = root.join("dist/cli.js");
        std::fs::write(&target, "owned fixture").unwrap();
        let write = |name: &str, bin: serde_json::Value| {
            std::fs::write(
                root.join("package.json"),
                serde_json::json!({"name":name,"bin":bin}).to_string(),
            )
            .unwrap();
        };
        write(
            "@earendil-works/pi-coding-agent",
            serde_json::json!({"pi":"dist/cli.js"}),
        );
        assert_eq!(
            entry("pi", &shim),
            Some(std::fs::canonicalize(&target).unwrap())
        );
        assert_eq!(entry("omp", &shim), None);
        for bin in [
            "../outside.js",
            "/outside.js",
            "C:/outside.js",
            "dist\\cli.js",
            "dist/missing.js",
        ] {
            write("@earendil-works/pi-coding-agent", serde_json::json!(bin));
            assert_eq!(entry("pi", &shim), None, "{bin}");
        }
        write(
            "@oh-my-pi/pi-coding-agent",
            serde_json::json!("dist/cli.js"),
        );
        assert_eq!(entry("pi", &shim), None);
        std::fs::write(
            root.join("package.json"),
            vec![b' '; MAX_MANIFEST as usize + 1],
        )
        .unwrap();
        assert_eq!(entry("pi", &shim), None);
    }

    /// OpenCode's npm package (opencode-ai) installs the native executable as its bin.
    #[test]
    fn opencode_resolves_to_the_executable_its_package_declares() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("opencode.cmd");
        std::fs::write(&shim, "never interpreted").unwrap();
        let root = dir.path().join("node_modules/opencode-ai");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let exe = root.join("bin/opencode.exe");
        std::fs::write(&exe, "synthetic executable").unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"opencode-ai","bin":{"opencode":"./bin/opencode.exe"}}"#,
        )
        .unwrap();
        let expected = Some(std::fs::canonicalize(&exe).unwrap());
        assert_eq!(entry("opencode", &shim), expected);
        assert_eq!(entry("opencode-v2", &shim), expected);
        // Another agent's shim name, or a lookalike package, is not OpenCode's.
        assert_eq!(entry("grok", &shim), None);
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"opencode","bin":{"opencode":"./bin/opencode.exe"}}"#,
        )
        .unwrap();
        assert_eq!(entry("opencode", &shim), None);
        if cfg!(windows) {
            std::fs::write(
                root.join("package.json"),
                r#"{"name":"opencode-ai","bin":{"opencode":"./bin/opencode.exe"}}"#,
            )
            .unwrap();
            let launched = command("opencode", &shim, &EnvMap::new()).unwrap();
            assert_eq!(Some(launched.program), expected.map(plain));
            assert!(launched.prefix.is_empty() && launched.set_env.is_empty());
        }
    }

    /// Pi's own installer: <agent>/bin/pi.cmd, and the release current-version names under
    /// <agent>/install/releases.
    fn managed_pi_fixture(dir: &Path, version: &str) -> (PathBuf, PathBuf) {
        let bin = dir.join("agent").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let shim = bin.join("pi.cmd");
        std::fs::write(&shim, "never interpreted").unwrap();
        std::fs::write(bin.join("pi-launcher.js"), "never run").unwrap();
        let install = dir.join("agent").join("install");
        let root = install
            .join("releases")
            .join(version)
            .join("node_modules/@earendil-works/pi-coding-agent");
        std::fs::create_dir_all(root.join("dist/bundle")).unwrap();
        std::fs::write(root.join("dist/bundle/cli.js"), "owned fixture").unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"@earendil-works/pi-coding-agent","bin":{"pi":"dist/bundle/cli.js"}}"#,
        )
        .unwrap();
        std::fs::write(
            install.join("managed-install.json"),
            r#"{"kind":"pi-managed-install","schemaVersion":1,"layout":"releases-v1"}"#,
        )
        .unwrap();
        std::fs::write(install.join("current-version"), format!("{version}\n")).unwrap();
        (shim, root.join("dist/bundle/cli.js"))
    }

    #[test]
    fn pi_installers_release_is_the_one_its_version_file_names() {
        let dir = tempfile::tempdir().unwrap();
        let (shim, cli) = managed_pi_fixture(dir.path(), "1.0.2");
        let cli = std::fs::canonicalize(cli).unwrap();
        assert_eq!(entry("pi", &shim), Some(cli.clone()));
        // Only Pi has this layout.
        assert_eq!(entry("omp", &shim), None);
        let install = dir.path().join("agent").join("install");
        if cfg!(windows) {
            let mut env = EnvMap::new();
            let node_dir = dir.path().join("node");
            std::fs::create_dir_all(&node_dir).unwrap();
            std::fs::write(node_dir.join("node.exe"), "synthetic executable").unwrap();
            env.insert(
                ghost_process::EnvKey::from("PATH"),
                node_dir.as_os_str().to_owned(),
            );
            let launched = command("pi", &shim, &env).unwrap();
            // Node runs the plain spelling: it cannot start a verbatim (\\?\) main script.
            assert_eq!(launched.prefix, vec![plain(cli.clone()).into_os_string()]);
            assert!(!launched.prefix[0].to_string_lossy().starts_with(r"\\?\"));
            assert_eq!(
                launched.set_env,
                vec![(
                    OsString::from(PI_MANAGED_INSTALL_ROOT),
                    install.clone().into_os_string()
                )]
            );
        }
        // A new release takes over when the version file names it.
        let (_, newer) = managed_pi_fixture(dir.path(), "1.1.0");
        assert_eq!(
            entry("pi", &shim),
            Some(std::fs::canonicalize(newer).unwrap())
        );
        // Versions that are not one plain segment are refused, as Pi's launcher refuses them.
        for version in ["", ".", "..", "../1.0.2", "1.0.2/x", "1.0.2\\x", "1 0"] {
            std::fs::write(install.join("current-version"), version).unwrap();
            assert_eq!(entry("pi", &shim), None, "{version:?}");
        }
        std::fs::write(install.join("current-version"), "9.9.9").unwrap();
        assert_eq!(entry("pi", &shim), None, "a release that is not installed");
        std::fs::write(install.join("current-version"), "1.0.2").unwrap();
        assert_eq!(entry("pi", &shim), Some(cli));
        // The marker must say it is Pi's managed install, in the layout this reads.
        for marker in [
            r#"{"kind":"pi-managed-install","layout":"releases-v2"}"#,
            r#"{"kind":"other","layout":"releases-v1"}"#,
            "not json",
        ] {
            std::fs::write(install.join("managed-install.json"), marker).unwrap();
            assert_eq!(entry("pi", &shim), None, "{marker}");
        }
        std::fs::remove_file(install.join("managed-install.json")).unwrap();
        assert_eq!(entry("pi", &shim), None);
    }

    #[test]
    fn verbatim_paths_take_their_plain_spelling_only_when_it_names_the_same_file() {
        let path = |text: &str| PathBuf::from(text);
        assert_eq!(
            plain(path(r"\\?\C:\Users\me\.pi\agent\cli.js")),
            path(r"C:\Users\me\.pi\agent\cli.js")
        );
        for kept in [
            r"\\?\UNC\server\share\cli.js",
            r"\\?\C:\work\CON\cli.js",
            r"\\?\C:\work\com1.txt\cli.js",
            r"\\?\C:\work\trailing.\cli.js",
            r"\\?\C:\work\..\cli.js",
            r"\\?\C:\work\\cli.js",
            r"\\?\Volume{0b1c2d3e-0000-0000-0000-000000000000}\cli.js",
            r"\\?\C:",
            r"C:\already\plain.js",
            "/home/me/cli.js",
        ] {
            assert_eq!(plain(path(kept)), path(kept), "{kept}");
        }
        let long = format!(r"\\?\C:\{}\cli.js", "a".repeat(260));
        assert_eq!(plain(path(&long)), path(&long));
        // A device-like name that is not one is plain.
        assert_eq!(
            plain(path(r"\\?\C:\work\console\com10\cli.js")),
            path(r"C:\work\console\com10\cli.js")
        );
    }

    #[test]
    fn a_managed_pi_shim_must_sit_in_its_bin_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (shim, _) = managed_pi_fixture(dir.path(), "1.0.2");
        let elsewhere = dir.path().join("agent").join("pi.cmd");
        std::fs::copy(&shim, &elsewhere).unwrap();
        assert_eq!(entry("pi", &elsewhere), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_package_bin_symlink_cannot_escape_its_package() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("omp.cmd");
        std::fs::write(&shim, "fixture").unwrap();
        let root = dir.path().join("node_modules/@oh-my-pi/pi-coding-agent");
        std::fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("outside.js");
        std::fs::write(&outside, "fixture").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("cli.js")).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"@oh-my-pi/pi-coding-agent","bin":{"omp":"cli.js"}}"#,
        )
        .unwrap();
        assert_eq!(entry("omp", &shim), None);
    }
}
