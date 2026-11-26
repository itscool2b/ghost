//! The login shell (macOS and Linux): which shell, its argv per family, and parsing its output.
//!
//! The command prints a fresh nonce marker, `/usr/bin/env -0`, and a second marker, so whatever the
//! user's rc files print around it is ignored: `<shell> -i -l -c "printf '%s' 'S'; /usr/bin/env -0;
//! printf '%s' 'E'"` for the POSIX family and fish, `-i -c` for csh and tcsh (csh takes `-l` only as
//! its sole argument). Shells that speak neither (`nu`, `xonsh`) are replaced by the platform's
//! default shell, with a warning naming them.

#[cfg(any(unix, test))]
use std::ffi::OsStr;
use std::ffi::OsString;
#[cfg(any(unix, test))]
use std::hash::{BuildHasher, Hasher};
#[cfg(any(unix, test))]
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

use super::{EnvKey, EnvMap};

/// The most a shell may print while it reports its environment.
#[cfg(unix)]
const MAX_SHELL_OUTPUT: usize = 4 * 1024 * 1024;

/// How a shell takes the command.
#[cfg(any(unix, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(unix, test))]
pub(crate) enum ShellFamily {
    /// `bash`, `zsh`, `sh`, `dash`, `ksh`, `mksh`, `yash`, `fish`: `-i -l -c`.
    Posix,
    /// `tcsh`, `csh`: `-i -c`.
    Csh,
    /// Anything else.
    Unsupported,
}

#[cfg(any(unix, test))]
pub(crate) fn family(shell: &Path) -> ShellFamily {
    match shell.file_name().and_then(OsStr::to_str) {
        Some("bash" | "zsh" | "sh" | "dash" | "ksh" | "mksh" | "yash" | "fish") => {
            ShellFamily::Posix
        }
        Some("tcsh" | "csh") => ShellFamily::Csh,
        _ => ShellFamily::Unsupported,
    }
}

/// The shell's arguments: interactive and login first, then the retry without `-i`.
#[cfg(any(unix, test))]
pub(crate) fn argv(family: ShellFamily, interactive: bool, command: &str) -> Vec<OsString> {
    let flags: &[&str] = match (family, interactive) {
        (ShellFamily::Csh, true) => &["-i", "-c"],
        (ShellFamily::Csh, false) => &["-c"],
        (_, true) => &["-i", "-l", "-c"],
        (_, false) => &["-l", "-c"],
    };
    flags
        .iter()
        .map(OsString::from)
        .chain(std::iter::once(OsString::from(command)))
        .collect()
}

/// The command every family runs.
#[cfg(any(unix, test))]
pub(crate) fn command_text(start: &str, end: &str) -> String {
    format!("printf '%s' '{start}'; /usr/bin/env -0; printf '%s' '{end}'")
}

/// A fresh pair of markers: `__AURA_ENV_<16 hex>_START__` and `..._END__`.
#[cfg(any(unix, test))]
pub(crate) fn markers() -> (String, String) {
    let mut hasher = std::hash::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos()),
    );
    let nonce = format!("{:016x}", hasher.finish());
    (
        format!("__AURA_ENV_{nonce}_START__"),
        format!("__AURA_ENV_{nonce}_END__"),
    )
}

/// The platform's default shell.
#[cfg(unix)]
pub(crate) fn platform_default() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/bin/zsh")
    } else {
        PathBuf::from("/bin/sh")
    }
}

/// `$SHELL` when absolute and executable, else the passwd entry's shell when it is, else the
/// platform default.
#[cfg(unix)]
pub(crate) fn pick_shell(
    env_shell: Option<&OsStr>,
    passwd_shell: impl FnOnce() -> Option<PathBuf>,
    executable: impl Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(shell) = env_shell.map(Path::new)
        && shell.is_absolute()
        && executable(shell)
    {
        return shell.to_path_buf();
    }
    if let Some(shell) = passwd_shell()
        && shell.is_absolute()
        && executable(&shell)
    {
        return shell;
    }
    platform_default()
}

/// Reads the env from the user's login shell: interactive first, then without `-i`. A shell that
/// speaks neither form is replaced by `default` ([`platform_default`]; tests pass a fake). On
/// failure, the reason (for the fallback) and the warnings so far.
#[cfg(unix)]
pub(crate) async fn login_shell(
    config: &super::EnvResolverConfig,
    registry: &crate::registry::LeftoverRegistry,
    base: &EnvMap,
    home: &Path,
    default: &Path,
) -> Result<super::ResolvedEnv, (String, Vec<String>)> {
    use std::sync::Arc;

    let started = tokio::time::Instant::now();
    let mut warnings = Vec::new();
    let shell = config.shell.clone().unwrap_or_else(|| {
        pick_shell(
            base.get(&EnvKey::from("SHELL")).map(OsString::as_os_str),
            passwd_shell,
            super::is_program,
        )
    });
    let (shell, family) = match family(&shell) {
        ShellFamily::Unsupported => {
            let default = default.to_path_buf();
            warnings.push(format!(
                "Ghost can't read the environment from {}; it used {} instead.",
                shell.file_name().map_or_else(
                    || shell.display().to_string(),
                    |name| name.to_string_lossy().into_owned()
                ),
                default.display()
            ));
            let family = family(&default);
            (default, family)
        }
        family => (shell, family),
    };
    let input = Arc::new(super::ResolvedEnv {
        vars: base.clone(),
        source: super::EnvSource::Fallback {
            reason: "the login shell's input".into(),
        },
        warnings: Vec::new(),
        resolved_at_ms: 0,
    });
    let run = |interactive: bool, timeout: std::time::Duration| {
        run_once(registry, &shell, family, interactive, home, &input, timeout)
    };
    let (vars, interactive, mut more) = match run(true, config.timeout).await {
        Ok((vars, more)) => (vars, true, more),
        Err(first) => match run(false, config.retry_timeout).await {
            Ok((vars, more)) => {
                warnings.push(format!(
                    "Your shell's interactive startup failed ({first}); Ghost read the environment without it."
                ));
                (vars, false, more)
            }
            Err(second) => {
                let reason = format!(
                    "Your login shell ({}) didn't report its environment: {second}.",
                    shell.display()
                );
                return Err((reason, warnings));
            }
        },
    };
    warnings.append(&mut more);
    Ok(super::ResolvedEnv {
        vars,
        source: super::EnvSource::LoginShell {
            shell,
            interactive,
            took_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        },
        warnings,
        resolved_at_ms: crate::log::now_millis(),
    })
}

/// One run of the shell; the env it printed, or why there is none.
#[cfg(unix)]
async fn run_once(
    registry: &crate::registry::LeftoverRegistry,
    shell: &Path,
    family: ShellFamily,
    interactive: bool,
    home: &Path,
    input: &std::sync::Arc<super::ResolvedEnv>,
    timeout: std::time::Duration,
) -> Result<(EnvMap, Vec<String>), String> {
    let (start, end) = markers();
    let spec = crate::spawn::HelperSpec {
        program: shell.to_path_buf(),
        args: argv(family, interactive, &command_text(&start, &end)),
        cwd: home.to_path_buf(),
        env: crate::spawn::HelperEnv::Resolved(std::sync::Arc::clone(input)),
        set: vec![("GHOST_RESOLVING_ENV".into(), "1".into())],
        // An interactive shell must never take the foreground of the terminal Ghost was
        // started from.
        new_session: true,
        timeout,
        max_stdout: MAX_SHELL_OUTPUT,
    };
    let output = crate::spawn::run_helper(registry, spec)
        .await
        .map_err(|error| error.to_string())?;
    if output.timed_out {
        return Err(format!("it did not finish within {}", seconds(timeout)));
    }
    let code = output.exit.and_then(|exit| exit.code);
    let Some(vars) = parse_env0(&output.stdout, &start, &end) else {
        return Err(match code {
            Some(code) if code != 0 => format!("it exited with code {code} before printing it"),
            _ => "its output held no environment".into(),
        });
    };
    if !vars.contains_key(&EnvKey::from("PATH")) {
        return Err("the environment it printed has no PATH".into());
    }
    let mut warnings = Vec::new();
    if code != Some(0) {
        warnings.push(format!(
            "Your shell exited with {} while reporting its environment; Ghost used what it printed.",
            code.map_or_else(|| "a signal".to_owned(), |code| format!("code {code}"))
        ));
    }
    Ok((vars, warnings))
}

/// "1 second", "10 seconds".
#[cfg(unix)]
fn seconds(duration: std::time::Duration) -> String {
    match duration.as_secs() {
        1 => "1 second".to_owned(),
        count => format!("{count} seconds"),
    }
}

/// The person's own shell, for a terminal: `SHELL` from the resolved environment when it is an
/// absolute path to a program, else the passwd entry's shell when it is. Unlike the environment's
/// capture this takes any shell (nu, xonsh, elvish): a terminal only starts it. `None` when
/// neither names a program, so the caller offers the shells it knows instead. The path comes from
/// the person's login environment and passwd entry only, never from a project or a chat.
#[cfg(unix)]
pub fn user_shell(vars: &EnvMap) -> Option<PathBuf> {
    user_shell_from(vars, passwd_shell, super::is_program)
}

#[cfg(unix)]
fn user_shell_from(
    vars: &EnvMap,
    passwd_shell: impl FnOnce() -> Option<PathBuf>,
    executable: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let usable = |shell: &Path| shell.is_absolute() && executable(shell);
    vars.get(&EnvKey::from("SHELL"))
        .map(PathBuf::from)
        .filter(|shell| usable(shell))
        .or_else(|| passwd_shell().filter(|shell| usable(shell)))
}

/// How a terminal starts the person's shell, as their own terminal app would: interactive, so it
/// reads its startup files (prompt, aliases, completions, tool activation). macOS terminals start
/// a login shell (Terminal and iTerm run `login`); Linux terminals (kitty, GNOME Terminal,
/// Konsole) start an interactive non-login one, since the session already ran the login files and
/// the environment the shell inherits came from a login shell. csh and tcsh take `-l` only as
/// their sole argument, and a shell Ghost does not know gets no arguments: the terminal makes
/// it interactive.
pub fn terminal_args(shell: &std::path::Path, macos: bool) -> Vec<&'static str> {
    let name = shell.file_name().and_then(std::ffi::OsStr::to_str);
    match (name, macos) {
        (Some("tcsh" | "csh"), true) => vec!["-l"],
        (Some("tcsh" | "csh"), false) => vec!["-i"],
        (
            Some(
                "bash" | "zsh" | "sh" | "dash" | "ksh" | "mksh" | "yash" | "fish" | "nu" | "xonsh",
            ),
            true,
        ) => vec!["-l", "-i"],
        (
            Some(
                "bash" | "zsh" | "sh" | "dash" | "ksh" | "mksh" | "yash" | "fish" | "nu" | "xonsh",
            ),
            false,
        ) => vec!["-i"],
        _ => Vec::new(),
    }
}

/// The passwd entry's shell.
#[cfg(unix)]
fn passwd_shell() -> Option<PathBuf> {
    nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.shell)
}

/// Parses the env between the last `start` marker before the last `end` marker and that `end`:
/// NUL-separated `NAME=value` entries, split at the first `=`; entries without `=` or with an empty
/// name are skipped, and a later duplicate wins. `None` when a marker is missing. Pure.
pub fn parse_env0(output: &[u8], start: &str, end: &str) -> Option<EnvMap> {
    let end_at = rfind(output, end.as_bytes())?;
    let before_end = output.get(..end_at)?;
    let start_at = rfind(before_end, start.as_bytes())?;
    let body = before_end.get(start_at + start.len()..)?;
    let mut vars = EnvMap::new();
    for entry in body.split(|&byte| byte == 0) {
        let Some(equals) = entry.iter().position(|&byte| byte == b'=') else {
            continue;
        };
        if equals == 0 {
            continue;
        }
        let (name, value) = (entry.get(..equals)?, entry.get(equals + 1..)?);
        vars.insert(EnvKey::from(os_string(name)), os_string(value));
    }
    Some(vars)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(unix)]
fn os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

#[cfg(not(unix))]
fn os_string(bytes: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(vars: &'a EnvMap, name: &str) -> Option<&'a str> {
        vars.get(&EnvKey::from(name))
            .and_then(|value| value.to_str())
    }

    #[test]
    fn env_ignores_profile_output() {
        let (start, end) = ("__AURA_ENV_1_START__", "__AURA_ENV_1_END__");
        let mut output =
            b"Welcome back!\n__AURA_ENV_1_START__ printed early by a broken rc\n".to_vec();
        output.extend_from_slice(start.as_bytes());
        output.extend_from_slice(b"PATH=/usr/bin:/bin\0HOME=/Users/me\0");
        output.extend_from_slice(end.as_bytes());
        output.extend_from_slice(b"Goodbye\n");
        let vars = parse_env0(&output, start, end).expect("markers");
        assert_eq!(vars.len(), 2);
        assert_eq!(value(&vars, "PATH"), Some("/usr/bin:/bin"));
        assert_eq!(value(&vars, "HOME"), Some("/Users/me"));
    }

    #[test]
    fn env_values_with_newlines() {
        let output = b"S\0A=one\ntwo=three\0EMPTY=\0=nameless\0noequals\0B=x\0B=later\0E";
        let vars = parse_env0(output, "S", "E").expect("markers");
        assert_eq!(value(&vars, "A"), Some("one\ntwo=three"));
        assert_eq!(value(&vars, "EMPTY"), Some(""));
        assert_eq!(value(&vars, "B"), Some("later"), "a later duplicate wins");
        assert_eq!(vars.len(), 3);
    }

    #[test]
    fn a_missing_marker_is_no_env() {
        assert!(parse_env0(b"S PATH=/bin", "S", "E").is_none());
        assert!(parse_env0(b"PATH=/bin E", "S", "E").is_none());
        assert!(
            parse_env0(b"E then S", "S", "E").is_none(),
            "the end must come after the start"
        );
        assert_eq!(parse_env0(b"SE", "S", "E").map(|vars| vars.len()), Some(0));
    }

    #[test]
    fn markers_are_fresh_and_shaped() {
        let (start, end) = markers();
        assert!(
            start.starts_with("__AURA_ENV_") && start.ends_with("_START__"),
            "{start}"
        );
        assert_eq!(start.len(), "__AURA_ENV__START__".len() + 16);
        assert_eq!(end, start.replace("_START__", "_END__"));
        assert_ne!(markers().0, start);
        assert_eq!(
            command_text(&start, &end),
            format!("printf '%s' '{start}'; /usr/bin/env -0; printf '%s' '{end}'")
        );
    }

    #[test]
    fn shell_argv_per_family() {
        for shell in [
            "/bin/bash",
            "/bin/zsh",
            "/bin/sh",
            "/bin/dash",
            "/bin/ksh",
            "/bin/mksh",
            "/usr/bin/yash",
            "/opt/homebrew/bin/fish",
        ] {
            assert_eq!(family(Path::new(shell)), ShellFamily::Posix, "{shell}");
        }
        for shell in ["/bin/tcsh", "/bin/csh"] {
            assert_eq!(family(Path::new(shell)), ShellFamily::Csh, "{shell}");
        }
        for shell in ["/opt/homebrew/bin/nu", "/usr/bin/xonsh", "/bin/zsh-5.9", ""] {
            assert_eq!(
                family(Path::new(shell)),
                ShellFamily::Unsupported,
                "{shell}"
            );
        }
        let words = |family, interactive| -> Vec<String> {
            argv(family, interactive, "CMD")
                .into_iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(words(ShellFamily::Posix, true), ["-i", "-l", "-c", "CMD"]);
        assert_eq!(words(ShellFamily::Posix, false), ["-l", "-c", "CMD"]);
        assert_eq!(words(ShellFamily::Csh, true), ["-i", "-c", "CMD"]);
        assert_eq!(words(ShellFamily::Csh, false), ["-c", "CMD"]);
    }

    /// Unix paths: on Windows `/bin/bash` is not absolute (and no login shell is read there).
    #[cfg(unix)]
    #[test]
    fn shell_choice_fallbacks() {
        let runs = |path: &Path| path != Path::new("/bin/not-executable");
        let passwd = || Some(PathBuf::from("/usr/local/bin/fish"));
        assert_eq!(
            pick_shell(Some(OsStr::new("/bin/bash")), passwd, runs),
            PathBuf::from("/bin/bash")
        );
        // Unset, relative or not executable: the passwd entry.
        for env_shell in [
            None,
            Some(OsStr::new("zsh")),
            Some(OsStr::new("/bin/not-executable")),
        ] {
            assert_eq!(
                pick_shell(env_shell, passwd, runs),
                PathBuf::from("/usr/local/bin/fish")
            );
        }
        // No usable passwd entry either: the platform default.
        assert_eq!(pick_shell(None, || None, runs), platform_default());
        assert_eq!(
            pick_shell(None, || Some(PathBuf::from("/bin/not-executable")), runs),
            platform_default()
        );
        assert_eq!(
            pick_shell(None, || Some(PathBuf::from("relative")), runs),
            platform_default()
        );
        // The passwd lookup only runs when $SHELL is unusable.
        let mut asked = false;
        let _ = pick_shell(
            Some(OsStr::new("/bin/bash")),
            || {
                asked = true;
                None
            },
            runs,
        );
        assert!(!asked);
    }

    /// A terminal takes any shell the person set, nu included, and nothing when neither `SHELL`
    /// nor the passwd entry names a program: then the terminal offers the shells it knows.
    #[cfg(unix)]
    #[test]
    fn a_terminal_starts_the_persons_own_shell() {
        let runs = |path: &Path| path != Path::new("/bin/not-executable");
        let env = |shell: Option<&str>| {
            let mut vars = EnvMap::new();
            if let Some(shell) = shell {
                vars.insert(EnvKey::from("SHELL"), OsString::from(shell));
            }
            vars
        };
        let passwd = || Some(PathBuf::from("/usr/bin/fish"));
        assert_eq!(
            user_shell_from(&env(Some("/usr/bin/nu")), passwd, runs),
            Some(PathBuf::from("/usr/bin/nu"))
        );
        for unusable in [None, Some("zsh"), Some("/bin/not-executable")] {
            assert_eq!(
                user_shell_from(&env(unusable), passwd, runs),
                Some(PathBuf::from("/usr/bin/fish")),
                "{unusable:?}"
            );
        }
        assert_eq!(user_shell_from(&env(None), || None, runs), None);
        assert_eq!(
            user_shell_from(
                &env(None),
                || Some(PathBuf::from("/bin/not-executable")),
                runs
            ),
            None
        );
    }

    /// Interactive everywhere, a login shell on macOS only, and csh's `-l` alone.
    #[test]
    fn terminal_args_follow_the_platforms_terminals() {
        let args = |shell: &str, macos: bool| terminal_args(Path::new(shell), macos);
        for shell in [
            "/usr/bin/bash",
            "/bin/zsh",
            "/usr/bin/fish",
            "/usr/bin/nu",
            "/bin/sh",
        ] {
            assert_eq!(args(shell, false), ["-i"], "{shell} on Linux");
            assert_eq!(args(shell, true), ["-l", "-i"], "{shell} on macOS");
        }
        assert_eq!(args("/bin/tcsh", false), ["-i"]);
        assert_eq!(args("/bin/tcsh", true), ["-l"]);
        assert!(args("/usr/bin/elvish", false).is_empty());
        assert!(args("/usr/bin/elvish", true).is_empty());
    }

    /// A fake login shell in `sh` (the test child is only built for integration tests): it
    /// records its argv, then runs the command it was given, which prints `env -0` between the
    /// markers. Run as a script, `sh` reads no rc file.
    #[cfg(unix)]
    const FAKE_SHELL: &str = r##"#!/bin/sh
printf '%s\n' "$*" >> "$GHOST_FAKE_RECORD"
while [ "$#" -gt 0 ]; do
  if [ "$1" = -c ]; then eval "$2"; exit; fi
  shift
done
exit 64
"##;

    /// E5: a shell that speaks neither argv form is replaced by the platform default, run with
    /// the first form, and a warning names the shell that was skipped.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_unsupported_shell_runs_the_default_with_a_warning() {
        use std::os::unix::fs::PermissionsExt;

        use crate::env::{EnvResolverConfig, EnvSource};
        use crate::registry::LeftoverRegistry;

        let dir = tempfile::tempdir().expect("tempdir");
        let default = dir.path().join("sh");
        std::fs::write(&default, FAKE_SHELL).expect("write");
        std::fs::set_permissions(&default, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let record = dir.path().join("argv");
        let base: EnvMap = [
            ("PATH", "/usr/bin:/bin"),
            ("GHOST_FAKE_RECORD", record.to_str().expect("utf-8")),
            ("FROM_BASE", "1"),
        ]
        .into_iter()
        .map(|(name, value)| (EnvKey::from(name), OsString::from(value)))
        .collect();
        for unsupported in ["nu", "xonsh"] {
            let config = EnvResolverConfig {
                shell: Some(dir.path().join(unsupported)),
                ..EnvResolverConfig::standard()
            };
            let registry = LeftoverRegistry::disabled();
            let env = login_shell(&config, &registry, &base, dir.path(), &default)
                .await
                .expect("the default shell's env");
            assert!(
                matches!(&env.source, EnvSource::LoginShell { shell, interactive: true, .. } if *shell == default),
                "{:?}",
                env.source
            );
            assert_eq!(
                env.warnings,
                [format!(
                    "Ghost can't read the environment from {unsupported}; it used {} instead.",
                    default.display()
                )]
            );
            assert_eq!(value(&env.vars, "FROM_BASE"), Some("1"));
        }
        let runs = std::fs::read_to_string(&record).expect("the default ran");
        let runs: Vec<&str> = runs.lines().collect();
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert!(
            runs.iter().all(|argv| argv.starts_with("-i -l -c printf")),
            "the first form, once each: {runs:?}"
        );
    }
}
