//! One agent's detection: finding its programs on the resolved env's `PATH` and running their
//! version commands as helpers (10 s, 64 KiB, killed with their tree on timeout). Binaries run in
//! `$HOME`, adapter probes in the adapter folder. Nothing here installs or runs an adapter.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ghost_process::{
    EnvKey, EnvMap, HelperEnv, HelperSpec, LeftoverRegistry, Redactor, ResolvedEnv, find_exe,
    find_program, run_helper, secret_values,
};
use ghost_protocol::session::Availability;

use crate::catalog::{AgentSpec, Recipe, UserBinary, parse_version, version_at_least};
use crate::install::{folder_name, installed_entry};
use crate::{PROBE_MAX_STDOUT, PROBE_TIMEOUT};

/// What a built-in agent reports as its path.
pub(crate) const BUILT_IN: &str = "built in";

/// One detection's result, with the programs a launch needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Detection {
    pub(crate) installation: Option<ghost_protocol::agents::AgentInstallation>,
    pub(crate) availability: Availability,
    /// An npm adapter that is not installed yet.
    pub(crate) needs_download: bool,
    /// Set when `availability` is `Ready` for an agent with a process.
    pub(crate) located: Option<Located>,
}

/// The programs detection found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Located {
    /// The agent's binary, or the user's own CLI behind an adapter.
    pub(crate) program: PathBuf,
    /// Fixed package entry arguments, preceding protocol and session arguments.
    pub(crate) prefix_args: Vec<OsString>,
    /// An adapter's `node`.
    pub(crate) node: Option<PathBuf>,
    /// What the vendor's own launcher would set for the program (Pi's install root).
    pub(crate) set_env: Vec<(OsString, OsString)>,
}

impl Detection {
    pub(crate) fn missing(reason: String) -> Detection {
        Detection {
            installation: None,
            availability: Availability::Missing { reason },
            needs_download: false,
            located: None,
        }
    }

    fn broken(reason: String) -> Detection {
        Detection {
            installation: None,
            availability: Availability::Broken { reason },
            needs_download: false,
            located: None,
        }
    }
}

/// Where the user's CLI is, for direct launches and adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UserBinaryLookup {
    Found(PathBuf),
    /// Windows: only a shell shim (`.cmd`, `.bat`, `.ps1`), which a direct CLI launch cannot spawn.
    ShimOnly(PathBuf),
    NotFound,
}

/// The binary's variable when it names an executable file (a `.exe` under Windows rules), else the
/// binary on `PATH`: any executable file on Unix, only `<program>.exe` under Windows rules, where a
/// shim alone is reported as such. `windows` picks the rules, so both are tested on every OS.
pub(crate) fn locate_user_binary(
    binary: &UserBinary,
    vars: &EnvMap,
    windows: bool,
) -> UserBinaryLookup {
    if let Some(value) = vars.get(&EnvKey::from(binary.env)) {
        let path = Path::new(value);
        if !value.is_empty() && path.is_absolute() && runnable(path, windows) {
            return UserBinaryLookup::Found(path.to_path_buf());
        }
    }
    if !windows {
        return find_exe(binary.program, vars)
            .map_or(UserBinaryLookup::NotFound, UserBinaryLookup::Found);
    }
    let dirs = windows_path_dirs(vars);
    let exe = format!("{}.exe", binary.program);
    if let Some(found) = dirs
        .iter()
        .map(|dir| dir.join(&exe))
        .find(|path| path.is_file())
    {
        return UserBinaryLookup::Found(found);
    }
    for dir in &dirs {
        for extension in ["cmd", "bat", "ps1"] {
            let shim = dir.join(format!("{}.{extension}", binary.program));
            if shim.is_file() {
                return UserBinaryLookup::ShimOnly(shim);
            }
        }
    }
    UserBinaryLookup::NotFound
}

/// Under Windows rules a program is a `.exe` file; on Unix, a file with an execute bit.
pub(crate) fn runnable(path: &Path, windows: bool) -> bool {
    if windows {
        return path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// `PATH` split the Windows way (`;`, quotes trimmed), absolute entries only.
pub(crate) fn windows_path_dirs(vars: &EnvMap) -> Vec<PathBuf> {
    let Some(path) = vars.get(&EnvKey::from("PATH")) else {
        return Vec::new();
    };
    path.to_string_lossy()
        .split(';')
        .map(|entry| entry.trim().trim_matches('"'))
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute() || looks_like_drive_path(dir))
        .collect()
}

/// `C:\...` reads as relative off Windows; under Windows rules it is absolute.
fn looks_like_drive_path(dir: &Path) -> bool {
    let text = dir.to_string_lossy();
    let bytes = text.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// "Claude" for "Claude Code": the adapter's name in messages.
pub(crate) fn short_name(binary: &UserBinary) -> &'static str {
    binary
        .label
        .split_whitespace()
        .next()
        .unwrap_or(binary.label)
}

/// Why an adapter cannot start without its user binary.
pub(crate) fn missing_user_binary(binary: &UserBinary, lookup: &UserBinaryLookup) -> String {
    match lookup {
        UserBinaryLookup::ShimOnly(shim) => format!(
            "{label} is installed through npm here, and the adapter can't run its {shim} shim on Windows. Install the standalone {label}, or set {env} to a {program}.exe.",
            label = binary.label,
            shim = shim.file_name().map_or_else(
                || shim.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            ),
            env = binary.env,
            program = binary.program,
        ),
        UserBinaryLookup::NotFound | UserBinaryLookup::Found(_) => format!(
            "The {short} adapter drives your own {label}, and {program} was not found on your PATH.",
            short = short_name(binary),
            label = binary.label,
            program = binary.program,
        ),
    }
}

/// Why a direct native driver cannot start the user's CLI.
fn missing_native_binary(binary: &UserBinary, lookup: &UserBinaryLookup) -> String {
    match lookup {
        UserBinaryLookup::ShimOnly(shim) => format!(
            "Ghost can't run the {shim} shell shim as native {label} on Windows. Install the standalone {label}, or set {env} to a {program}.exe.",
            shim = shim.file_name().map_or_else(
                || shim.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            ),
            label = binary.label,
            env = binary.env,
            program = binary.program,
        ),
        UserBinaryLookup::NotFound | UserBinaryLookup::Found(_) => {
            format!("{} was not found on your PATH.", binary.program)
        }
    }
}

/// The folder probes of binaries run in: `$HOME`, else the temp folder.
fn home(env: &ResolvedEnv) -> PathBuf {
    env.vars
        .get(&EnvKey::from(if cfg!(windows) {
            "USERPROFILE"
        } else {
            "HOME"
        }))
        .map(PathBuf::from)
        .filter(|home| home.is_absolute() && home.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

/// Runs `<program> <args>` as a probe; its stdout (or stderr, for tools that print their version
/// there), or the reason it failed, quoting its first stderr line, redacted.
async fn probe(
    registry: &LeftoverRegistry,
    env: &Arc<ResolvedEnv>,
    program: &Path,
    args: &[&str],
    cwd: &Path,
) -> Result<String, String> {
    probe_with_prefix(registry, env, program, &[], args, cwd).await
}

async fn probe_with_prefix(
    registry: &LeftoverRegistry,
    env: &Arc<ResolvedEnv>,
    program: &Path,
    prefix: &[OsString],
    args: &[&str],
    cwd: &Path,
) -> Result<String, String> {
    let name = command_name(program);
    let command = std::iter::once(name.as_str())
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    let output = run_helper(
        registry,
        HelperSpec {
            program: program.to_path_buf(),
            args: prefix
                .iter()
                .cloned()
                .chain(args.iter().map(OsString::from))
                .collect(),
            cwd: cwd.to_path_buf(),
            env: HelperEnv::Resolved(Arc::clone(env)),
            set: Vec::new(),
            new_session: false,
            timeout: PROBE_TIMEOUT,
            max_stdout: PROBE_MAX_STDOUT,
        },
    )
    .await
    .map_err(|error| format!("{} can't be run: {error}.", program.display()))?;
    let redactor = Redactor::standard();
    redactor.set_secrets(secret_values(&env.vars));
    let first_stderr = output
        .stderr_tail
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| {
            redactor
                .redact_text(line)
                .chars()
                .take(300)
                .collect::<String>()
        });
    if output.timed_out {
        return Err(match first_stderr {
            Some(line) => format!("`{command}` did not answer within 10 seconds: {line}"),
            None => format!("`{command}` did not answer within 10 seconds."),
        });
    }
    let code = output.exit.and_then(|exit| exit.code);
    if code != Some(0) {
        return Err(match (first_stderr, code) {
            (Some(line), _) => format!("`{command}` failed: {line}"),
            (None, Some(code)) => format!("`{command}` failed with exit code {code}."),
            (None, None) => format!("`{command}` was stopped by a signal."),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if stdout.trim().is_empty() {
        return Ok(output.stderr_tail);
    }
    Ok(stdout)
}

async fn launch_command(
    id: &str,
    path: &Path,
    env: &EnvMap,
) -> Result<crate::npm_entry::Command, String> {
    let (id, path, env) = (id.to_owned(), path.to_owned(), env.clone());
    tokio::task::spawn_blocking(move || crate::npm_entry::command(&id, &path, &env))
        .await
        .map_err(|_| "The agent package could not be inspected.".to_owned())?
}

/// The program as a user types it in messages: its file name, without the `.exe` Windows adds.
fn command_name(program: &Path) -> String {
    let exe = cfg!(windows)
        && program
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
    let shown = if exe {
        program.file_stem()
    } else {
        program.file_name()
    };
    shown.map_or_else(
        || program.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// A reviewed candidate is probed directly, without rewriting PATH or its helper resolution.
pub(crate) async fn detect_at(
    spec: &AgentSpec,
    env: &Arc<ResolvedEnv>,
    registry: &LeftoverRegistry,
    adapters_dir: &Path,
    candidate: Option<&Path>,
) -> Detection {
    if let Some(path) = candidate
        && (!path.is_absolute() || !path.is_file())
    {
        return Detection::missing("The selected executable is no longer available. Choose another installation or use the default.".into());
    }
    let mut detected = detect_inner(spec, env, registry, adapters_dir, candidate).await;
    if let Availability::Ready {
        version: Some(version),
        path,
    } = &detected.availability
    {
        let agent_id = spec.id.to_owned();
        let version = version.clone();
        let path = path.clone();
        detected.installation = tokio::task::spawn_blocking(move || {
            crate::installation::native(&agent_id, &version, &path)
        })
        .await
        .ok()
        .flatten();
    }
    detected
}

async fn detect_inner(
    spec: &AgentSpec,
    env: &Arc<ResolvedEnv>,
    registry: &LeftoverRegistry,
    adapters_dir: &Path,
    candidate: Option<&Path>,
) -> Detection {
    match &spec.recipe {
        Recipe::Builtin => Detection {
            installation: None,
            availability: Availability::Ready {
                version: None,
                path: BUILT_IN.into(),
            },
            needs_download: false,
            located: None,
        },
        Recipe::Binary {
            program,
            version_args,
            min_version,
            ..
        } => {
            let Some(path) = candidate
                .map(Path::to_owned)
                .or_else(|| find_program(program, &env.vars))
            else {
                return Detection::missing(format!("{program} was not found on your PATH."));
            };
            let crate::npm_entry::Command {
                program,
                prefix: prefix_args,
                set_env,
            } = match launch_command(spec.id, &path, &env.vars).await {
                Ok(command) => command,
                Err(reason) => return Detection::broken(reason),
            };
            let printed = match probe_with_prefix(
                registry,
                env,
                &program,
                &prefix_args,
                version_args,
                &home(env),
            )
            .await
            {
                Ok(printed) => printed,
                Err(reason) => return Detection::broken(reason),
            };
            let version = parse_version(&printed);
            if let Some(floor) = min_version
                && !version
                    .as_deref()
                    .is_some_and(|found| version_at_least(found, floor))
            {
                return Detection::broken(format!(
                    "Ghost needs {} {floor} or newer (found {}).",
                    spec.name,
                    version.as_deref().unwrap_or("an unknown version")
                ));
            }
            let shown = version.map_or_else(
                || {
                    printed
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .chars()
                        .take(80)
                        .collect()
                },
                |version| format!("{} {version}", spec.name),
            );
            Detection {
                installation: None,
                availability: Availability::Ready {
                    version: (!shown.is_empty()).then_some(shown),
                    path: path.display().to_string(),
                },
                needs_download: false,
                located: Some(Located {
                    program,
                    prefix_args,
                    node: None,
                    set_env,
                }),
            }
        }
        Recipe::NativeBinary { user_binary, .. } => {
            let path = match candidate
                .map(|path| UserBinaryLookup::Found(path.to_owned()))
                .unwrap_or_else(|| locate_user_binary(user_binary, &env.vars, cfg!(windows)))
            {
                UserBinaryLookup::Found(path) => path,
                lookup => return Detection::missing(missing_native_binary(user_binary, &lookup)),
            };
            if !runnable(&path, cfg!(windows)) {
                return Detection::broken(
                    "This native driver requires an executable, not a shell shim.".into(),
                );
            }
            let printed =
                match probe(registry, env, &path, user_binary.version_args, &home(env)).await {
                    Ok(printed) => printed,
                    Err(reason) => return Detection::broken(reason),
                };
            let version = parse_version(&printed);
            if !version
                .as_deref()
                .is_some_and(|found| version_at_least(found, user_binary.min_version))
            {
                return Detection::broken(format!(
                    "Ghost needs {} {} or newer (found {}).",
                    user_binary.label,
                    user_binary.min_version,
                    version.as_deref().unwrap_or("an unknown version")
                ));
            }
            Detection {
                installation: None,
                availability: Availability::Ready {
                    version: Some(format!(
                        "{} {}",
                        user_binary.label,
                        version.as_deref().unwrap_or_default()
                    )),
                    path: path.display().to_string(),
                },
                needs_download: false,
                located: Some(Located {
                    program: path,
                    prefix_args: Vec::new(),
                    node: None,
                    set_env: Vec::new(),
                }),
            }
        }
        Recipe::QualifiedBinary {
            program,
            version_args,
            min_version,
            major,
            ..
        } => {
            let Some(path) = candidate
                .map(Path::to_owned)
                .or_else(|| find_program(program, &env.vars))
            else {
                return Detection::missing(format!(
                    "{program} was not found on your PATH. Install the official {} distribution first.",
                    spec.name
                ));
            };
            let crate::npm_entry::Command {
                program,
                prefix: prefix_args,
                set_env,
            } = match launch_command(spec.id, &path, &env.vars).await {
                Ok(command) => command,
                Err(reason) => return Detection::broken(reason),
            };
            let printed = match probe_with_prefix(
                registry,
                env,
                &program,
                &prefix_args,
                version_args,
                &home(env),
            )
            .await
            {
                Ok(printed) => printed,
                Err(reason) => return Detection::broken(reason),
            };
            let version = match qualified_version(&printed, min_version, *major) {
                Ok(version) => version,
                Err(reason) => return Detection::broken(format!("{}: {reason}", spec.name)),
            };
            Detection {
                installation: None,
                availability: Availability::Ready {
                    version: Some(format!("{} {version} (unverified)", spec.name)),
                    path: path.display().to_string(),
                },
                needs_download: false,
                located: Some(Located {
                    program,
                    prefix_args,
                    node: None,
                    set_env,
                }),
            }
        }
        Recipe::NpmAdapter {
            package,
            version,
            bin,
            min_node_major,
            user_binary,
        } => {
            let folder = adapters_dir.join(folder_name(package, version));
            let probe_dir = if folder.is_dir() {
                folder.clone()
            } else {
                home(env)
            };
            let node_required = |found: &str| {
                format!("Node.js {min_node_major} or newer is required (found {found}).")
            };
            let Some(node) = find_program("node", &env.vars) else {
                return Detection::missing(node_required("none"));
            };
            let printed = match probe(registry, env, &node, &["--version"], &probe_dir).await {
                Ok(printed) => printed,
                Err(reason) => return Detection::broken(reason),
            };
            let node_version = parse_version(&printed);
            let major = node_version
                .as_deref()
                .and_then(|found| found.split('.').next())
                .and_then(|major| major.parse::<u32>().ok());
            if major.is_none_or(|major| major < *min_node_major) {
                return Detection::missing(node_required(
                    node_version.as_deref().unwrap_or("an unknown version"),
                ));
            }
            let user = match candidate
                .map(|path| UserBinaryLookup::Found(path.to_owned()))
                .unwrap_or_else(|| locate_user_binary(user_binary, &env.vars, cfg!(windows)))
            {
                UserBinaryLookup::Found(path) => path,
                lookup => return Detection::missing(missing_user_binary(user_binary, &lookup)),
            };
            if !runnable(&user, cfg!(windows)) {
                return Detection::broken(
                    "This adapter requires an executable, not a shell shim.".into(),
                );
            }
            let printed =
                match probe(registry, env, &user, user_binary.version_args, &probe_dir).await {
                    Ok(printed) => printed,
                    Err(reason) => return Detection::broken(reason),
                };
            let user_version = parse_version(&printed);
            if !user_version
                .as_deref()
                .is_some_and(|found| version_at_least(found, user_binary.min_version))
            {
                return Detection::broken(format!(
                    "Ghost needs {} {} or newer (found {}).",
                    user_binary.label,
                    user_binary.min_version,
                    user_version.as_deref().unwrap_or("an unknown version")
                ));
            }
            let installed = installed_entry(&folder, package, version).is_some();
            if !installed && find_program("npm", &env.vars).is_none() {
                return Detection::missing(
                    "npm was not found; Ghost needs it once to download the adapter.".into(),
                );
            }
            Detection {
                installation: None,
                availability: Availability::Ready {
                    version: Some(format!(
                        "{} {} via {bin} {version}",
                        user_binary.label,
                        user_version.as_deref().unwrap_or_default()
                    )),
                    path: user.display().to_string(),
                },
                needs_download: !installed,
                located: Some(Located {
                    program: user,
                    prefix_args: Vec::new(),
                    node: Some(node),
                    set_env: Vec::new(),
                }),
            }
        }
    }
}

/// Protocol baselines are deliberately separate for OpenCode's incompatible major lines.
fn qualified_version(printed: &str, floor: &str, major: Option<u64>) -> Result<String, String> {
    let version = parse_version(printed)
        .ok_or_else(|| "the version command did not identify a supported version".to_owned())?;
    let found_major = version
        .split('.')
        .next()
        .and_then(|value| value.parse::<u64>().ok());
    if major.is_some() && major != found_major {
        return Err(format!(
            "this catalog entry requires the {}.x release line; found {version}",
            major.unwrap_or_default()
        ));
    }
    if !version_at_least(&version, floor) {
        return Err(format!(
            "requires the {floor} protocol baseline or newer; found {version}"
        ));
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::catalog::get;

    fn claude_binary() -> UserBinary {
        match &get("claude-acp").expect("claude").recipe {
            Recipe::NpmAdapter { user_binary, .. } => *user_binary,
            _ => panic!("an adapter"),
        }
    }

    fn vars(path: &[&Path], extra: &[(&str, &str)]) -> EnvMap {
        let mut vars = EnvMap::new();
        let joined = path
            .iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(";");
        vars.insert(EnvKey::from("PATH"), OsString::from(joined));
        for (name, value) in extra {
            vars.insert(EnvKey::from(*name), OsString::from(*value));
        }
        vars
    }

    #[test]
    fn user_binary_requires_exe_on_windows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (npm_shims, standalone) = (dir.path().join("npm"), dir.path().join("Claude"));
        std::fs::create_dir_all(&npm_shims).expect("mkdir");
        std::fs::create_dir_all(&standalone).expect("mkdir");
        std::fs::write(npm_shims.join("claude.cmd"), b"@echo off").expect("write");
        std::fs::write(npm_shims.join("claude.ps1"), b"").expect("write");
        let binary = claude_binary();

        // Only the npm shim: Missing, with the reason.
        let lookup = locate_user_binary(&binary, &vars(&[&npm_shims], &[]), true);
        assert_eq!(
            lookup,
            UserBinaryLookup::ShimOnly(npm_shims.join("claude.cmd"))
        );
        assert_eq!(
            missing_user_binary(&binary, &lookup),
            "Claude Code is installed through npm here, and the adapter can't run its claude.cmd shim on Windows. Install the standalone Claude Code, or set CLAUDE_CODE_EXECUTABLE to a claude.exe."
        );
        assert_eq!(
            missing_native_binary(&binary, &lookup),
            "Ghost can't run the claude.cmd shell shim as native Claude Code on Windows. Install the standalone Claude Code, or set CLAUDE_CODE_EXECUTABLE to a claude.exe."
        );

        // A .exe elsewhere on PATH is used, even after the shim's folder.
        std::fs::write(standalone.join("claude.exe"), b"MZ").expect("write");
        assert_eq!(
            locate_user_binary(&binary, &vars(&[&npm_shims, &standalone], &[]), true),
            UserBinaryLookup::Found(standalone.join("claude.exe"))
        );

        // The variable must name a .exe too; a .cmd there is overridden.
        let shim = npm_shims.join("claude.cmd");
        let env = vars(
            &[&standalone],
            &[("CLAUDE_CODE_EXECUTABLE", shim.to_str().expect("utf-8"))],
        );
        assert_eq!(
            locate_user_binary(&binary, &env, true),
            UserBinaryLookup::Found(standalone.join("claude.exe"))
        );
        let elsewhere = dir.path().join("custom.exe");
        std::fs::write(&elsewhere, b"MZ").expect("write");
        let env = vars(
            &[&npm_shims],
            &[("CLAUDE_CODE_EXECUTABLE", elsewhere.to_str().expect("utf-8"))],
        );
        assert_eq!(
            locate_user_binary(&binary, &env, true),
            UserBinaryLookup::Found(elsewhere)
        );

        // Nothing at all.
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).expect("mkdir");
        let lookup = locate_user_binary(&binary, &vars(&[&empty], &[]), true);
        assert_eq!(lookup, UserBinaryLookup::NotFound);
        assert_eq!(
            missing_user_binary(&binary, &lookup),
            "The Claude adapter drives your own Claude Code, and claude was not found on your PATH."
        );
    }

    #[test]
    fn drive_paths_count_as_absolute_under_windows_rules() {
        assert!(looks_like_drive_path(Path::new("C:\\Users\\me\\bin")));
        assert!(looks_like_drive_path(Path::new("d:/tools")));
        assert!(!looks_like_drive_path(Path::new("relative\\bin")));
        let mut env = EnvMap::new();
        env.insert(
            EnvKey::from("PATH"),
            OsString::from("\"C:\\Program Files\\Claude\";;relative;C:\\Tools"),
        );
        assert_eq!(
            windows_path_dirs(&env),
            [
                PathBuf::from("C:\\Program Files\\Claude"),
                PathBuf::from("C:\\Tools")
            ]
        );
    }

    #[test]
    fn messages_name_programs_as_typed() {
        assert_eq!(command_name(Path::new("/opt/tools/hermes")), "hermes");
        #[cfg(windows)]
        assert_eq!(command_name(Path::new(r"C:\Tools\Hermes.EXE")), "Hermes");
    }

    #[test]
    fn adapter_names_come_from_the_binary_label() {
        assert_eq!(short_name(&claude_binary()), "Claude");
        let codex = match &get("codex-acp").expect("codex").recipe {
            Recipe::NpmAdapter { user_binary, .. } => *user_binary,
            _ => panic!("an adapter"),
        };
        assert_eq!(short_name(&codex), "Codex");
        assert_eq!(
            missing_user_binary(&codex, &UserBinaryLookup::NotFound),
            "The Codex adapter drives your own Codex, and codex was not found on your PATH."
        );
    }
}
