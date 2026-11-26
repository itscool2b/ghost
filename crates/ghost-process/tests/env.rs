//! The env resolver against a fake login shell (the test child copied under a shell's name):
//! markers, retries, timeouts that kill the shell's session, launch-context stripping, sharing, and
//! the redactor. Nothing of the user's own shell runs. macOS and Linux only; Windows reads the
//! registry (`env/windows.rs` tests it on Windows).

#![cfg(unix)]

#[cfg(test)]
mod common;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::{GONE_WITHIN, Reaper, testbin, wait_gone};
use ghost_process::{
    EnvKey, EnvMap, EnvResolver, EnvResolverConfig, EnvSource, LeftoverRegistry, Redactor,
    ResolvedEnv, ResolverStatus,
};
use tokio::runtime::Handle;

/// A fake shell named `name` in a temp folder, with a home and the files it records into.
#[cfg(test)]
struct FakeShell {
    dir: tempfile::TempDir,
    shell: PathBuf,
    home: PathBuf,
    record: PathBuf,
    pidfile: PathBuf,
}

#[cfg(test)]
impl FakeShell {
    fn new(name: &str) -> FakeShell {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("bin");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&bin).expect("mkdir");
        std::fs::create_dir_all(&home).expect("mkdir");
        let shell = bin.join(name);
        std::fs::copy(testbin(), &shell).expect("copy");
        FakeShell {
            record: dir.path().join("argv.jsonl"),
            pidfile: dir.path().join("shell.pids"),
            dir,
            shell,
            home,
        }
    }

    /// A config running this shell in `mode`, with `extra` in its input env.
    fn config(&self, mode: &str, extra: &[(&str, &str)]) -> EnvResolverConfig {
        let mut base = EnvMap::new();
        for (name, value) in [
            ("PATH", "/usr/bin:/bin"),
            ("HOME", self.home.to_str().expect("utf-8")),
            ("GHOST_TESTBIN_SHELL", mode),
            ("GHOST_TESTBIN_RECORD", self.record.to_str().expect("utf-8")),
            (
                "GHOST_TESTBIN_PIDFILE",
                self.pidfile.to_str().expect("utf-8"),
            ),
        ]
        .into_iter()
        .chain(extra.iter().copied())
        {
            base.insert(EnvKey::from(name), OsString::from(value));
        }
        EnvResolverConfig {
            shell: Some(self.shell.clone()),
            base: Some(base),
            home: Some(self.home.clone()),
            known_dirs: Some(Vec::new()),
            ..EnvResolverConfig::standard()
        }
    }

    /// Every argv the shell was run with.
    fn runs(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(&self.record)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("argv"))
            .collect()
    }
}

#[cfg(test)]
fn get<'a>(env: &'a ResolvedEnv, name: &str) -> Option<&'a str> {
    env.vars
        .get(&EnvKey::from(name))
        .and_then(|value| value.to_str())
}

#[cfg(test)]
async fn resolve(config: EnvResolverConfig) -> (Arc<EnvResolver>, Arc<ResolvedEnv>) {
    let resolver = EnvResolver::start(config, LeftoverRegistry::disabled(), Handle::current());
    let env = resolver.get().await;
    (resolver, env)
}

#[tokio::test]
async fn resolves_env_from_fake_login_shell() {
    let fake = FakeShell::new("zsh");
    let local_bin = fake.home.join(".local/bin");
    std::fs::create_dir_all(&local_bin).expect("mkdir");
    let redactor = Arc::new(Redactor::standard());
    let config = EnvResolverConfig {
        known_dirs: Some(vec![local_bin.clone(), fake.home.join("missing/bin")]),
        redactor: Some(Arc::clone(&redactor)),
        ..fake.config(
            "ok",
            &[(
                "GHOST_TESTBIN_RC",
                "FROM_RC=1;MY_API_KEY=a-secret-value-from-rc",
            )],
        )
    };
    let (resolver, env) = resolve(config).await;
    let EnvSource::LoginShell {
        shell,
        interactive,
        took_ms,
    } = &env.source
    else {
        panic!("a login shell env: {:?}", env.source);
    };
    assert_eq!(shell, &fake.shell);
    assert!(*interactive);
    assert!(env.warnings.is_empty(), "{:?}", env.warnings);
    assert_eq!(
        get(&env, "FROM_RC"),
        Some("1"),
        "what the rc files export comes back"
    );
    for name in ["PWD", "OLDPWD", "SHLVL", "_", "GHOST_RESOLVING_ENV"] {
        assert_eq!(get(&env, name), None, "{name}");
    }
    let path = get(&env, "PATH").expect("PATH");
    assert_eq!(
        path,
        format!("/usr/bin:/bin:{}", local_bin.display()),
        "existing known dirs are appended, missing ones are not"
    );
    // The shell ran once, interactive and login, in its own session with the marker command.
    let runs = fake.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(&runs[0][..3], ["-i", "-l", "-c"]);
    assert!(runs[0][3].contains("/usr/bin/env -0") && runs[0][3].contains("__AURA_ENV_"));
    assert_eq!(
        resolver.status(),
        ResolverStatus::Ready {
            source: env.source.clone(),
            took_ms: *took_ms,
            vars: env.vars.len(),
        }
    );
    assert_eq!(
        redactor.redact_text("echo a-secret-value-from-rc"),
        "echo <SECRET>",
        "the shell's secrets reach the redactor before anyone gets the env"
    );
}

#[tokio::test]
async fn env_ignores_profile_output() {
    let fake = FakeShell::new("bash");
    let (_, env) = resolve(fake.config("junk", &[])).await;
    assert!(
        matches!(
            env.source,
            EnvSource::LoginShell {
                interactive: true,
                ..
            }
        ),
        "{:?}",
        env.source
    );
    assert_eq!(get(&env, "PATH"), Some("/usr/bin:/bin"));
    assert!(env.vars.keys().all(|name| {
        let name = name.as_os_str().to_string_lossy();
        !name.contains("Last login") && !name.contains("logout")
    }));
}

#[tokio::test]
async fn env_nonzero_exit_with_markers() {
    let fake = FakeShell::new("zsh");
    let (_, env) = resolve(fake.config("nonzero", &[])).await;
    assert!(
        matches!(
            env.source,
            EnvSource::LoginShell {
                interactive: true,
                ..
            }
        ),
        "{:?}",
        env.source
    );
    assert_eq!(env.warnings.len(), 1, "{:?}", env.warnings);
    assert!(env.warnings[0].contains("code 3"), "{:?}", env.warnings);
    assert_eq!(fake.runs().len(), 1, "accepted without a retry");
}

#[tokio::test]
async fn env_values_with_newlines() {
    let fake = FakeShell::new("zsh");
    let (_, env) =
        resolve(fake.config("ok", &[("GHOST_TESTBIN_RC", "MULTI=line one\\nline=two")])).await;
    assert_eq!(get(&env, "MULTI"), Some("line one\nline=two"));
}

#[tokio::test]
async fn env_timeout_retries_then_falls_back() {
    let fake = FakeShell::new("zsh");
    // Long enough for a loaded machine to start each shell and its child before the limit.
    let config = EnvResolverConfig {
        timeout: Duration::from_secs(3),
        retry_timeout: Duration::from_secs(2),
        ..fake.config("hang", &[("TERM_PROGRAM", "Apple_Terminal")])
    };
    let (resolver, env) = resolve(config).await;
    let EnvSource::Fallback { reason } = &env.source else {
        panic!("a fallback: {:?}", env.source);
    };
    assert!(
        reason.contains("did not finish within 2 seconds"),
        "{reason}"
    );
    assert!(matches!(resolver.status(), ResolverStatus::Fallback { .. }));
    // The fallback is Ghost's own env without its launch context.
    assert_eq!(get(&env, "PATH"), Some("/usr/bin:/bin"));
    assert_eq!(get(&env, "TERM_PROGRAM"), None);
    // Interactive first, then the login shell without -i.
    let runs = fake.runs();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(&runs[0][..2], ["-i", "-l"]);
    assert_eq!(&runs[1][..2], ["-l", "-c"]);
    // Each hanging shell and the child it started in a group of its own (where killing the
    // shell's group never reaches) were killed with their session.
    let pids: Vec<u32> = std::fs::read_to_string(&fake.pidfile)
        .expect("pidfile")
        .split_whitespace()
        .map(|pid| pid.parse().expect("pid"))
        .collect();
    assert_eq!(pids.len(), 4);
    let mut reaper = Reaper::new();
    reaper.hold_all(&pids);
    for pid in pids {
        assert!(
            wait_gone(pid, GONE_WITHIN).await,
            "{pid} outlived the timeout"
        );
    }
    reaper.assert_gone().await;
}

/// Quit during boot: the runtime shuts down with the resolution in flight. The shell's whole
/// session dies with the dropped attempt, and whoever waits gets the fallback, whose secrets
/// reached the redactor first.
#[test]
fn a_resolution_dropped_at_shutdown_kills_the_shells_session() {
    let fake = FakeShell::new("zsh");
    let redactor = Arc::new(Redactor::standard());
    let config = EnvResolverConfig {
        redactor: Some(Arc::clone(&redactor)),
        ..fake.config("hang", &[("MY_API_KEY", "fallback-secret-0123456789")])
    };
    let doomed = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    let resolver = EnvResolver::start(
        config,
        LeftoverRegistry::disabled(),
        doomed.handle().clone(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let waiting = runtime.spawn({
        let resolver = Arc::clone(&resolver);
        async move { resolver.get().await }
    });
    let mut reaper = Reaper::new();
    let pids = runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + common::STARTUP;
        loop {
            let pids: Vec<u32> = std::fs::read_to_string(&fake.pidfile)
                .unwrap_or_default()
                .split_whitespace()
                .map(|pid| pid.parse().expect("pid"))
                .collect();
            if pids.len() == 2 {
                return pids;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the shell never hung"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    reaper.hold_all(&pids);
    doomed.shutdown_timeout(Duration::from_secs(10));
    let env = runtime.block_on(waiting).expect("join");
    assert_eq!(
        env.source,
        EnvSource::Fallback {
            reason: "the environment could not be read".into()
        }
    );
    assert_eq!(get(&env, "MY_API_KEY"), Some("fallback-secret-0123456789"));
    assert_eq!(
        redactor.redact_text("echo fallback-secret-0123456789"),
        "echo <SECRET>"
    );
    runtime.block_on(async {
        for pid in &pids {
            assert!(
                wait_gone(*pid, GONE_WITHIN).await,
                "{pid} outlived its dropped resolution"
            );
        }
        reaper.assert_gone().await;
    });
}

#[tokio::test]
async fn an_rc_that_hangs_only_interactively_is_read_without_it() {
    let fake = FakeShell::new("zsh");
    let config = EnvResolverConfig {
        timeout: Duration::from_secs(3),
        ..fake.config("hang-interactive", &[])
    };
    let (_, env) = resolve(config).await;
    assert!(
        matches!(
            env.source,
            EnvSource::LoginShell {
                interactive: false,
                ..
            }
        ),
        "{:?}",
        env.source
    );
    assert_eq!(env.warnings.len(), 1, "{:?}", env.warnings);
    let pids: Vec<u32> = std::fs::read_to_string(&fake.pidfile)
        .expect("pidfile")
        .split_whitespace()
        .map(|pid| pid.parse().expect("pid"))
        .collect();
    let mut reaper = Reaper::new();
    reaper.hold_all(&pids);
    reaper.assert_gone().await;
}

#[tokio::test]
async fn output_without_markers_or_path_falls_back() {
    for (mode, expected) in [
        ("no-marker", "its output held no environment"),
        ("no-path", "has no PATH"),
    ] {
        let fake = FakeShell::new("zsh");
        let (_, env) = resolve(fake.config(mode, &[])).await;
        let EnvSource::Fallback { reason } = &env.source else {
            panic!("{mode}: a fallback: {:?}", env.source);
        };
        assert!(reason.contains(expected), "{mode}: {reason}");
        assert_eq!(fake.runs().len(), 2, "{mode}: retried once");
    }
}

#[tokio::test]
async fn shell_argv_per_family_reaches_the_shell() {
    let fake = FakeShell::new("tcsh");
    let (_, env) = resolve(fake.config("ok", &[])).await;
    assert!(matches!(env.source, EnvSource::LoginShell { .. }));
    let runs = fake.runs();
    assert_eq!(
        &runs[0][..2],
        ["-i", "-c"],
        "csh takes -l only as its sole argument"
    );
    let fake = FakeShell::new("fish");
    resolve(fake.config("ok", &[])).await;
    assert_eq!(&fake.runs()[0][..3], ["-i", "-l", "-c"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn env_resolution_is_shared() {
    let fake = FakeShell::new("zsh");
    let resolver = EnvResolver::start(
        fake.config("ok", &[]),
        LeftoverRegistry::disabled(),
        Handle::current(),
    );
    let mut waiting = Vec::new();
    for _ in 0..16 {
        let resolver = Arc::clone(&resolver);
        waiting.push(tokio::spawn(async move { resolver.get().await }));
    }
    let mut envs = Vec::new();
    for task in waiting {
        envs.push(task.await.expect("join"));
    }
    assert!(
        envs.windows(2).all(|pair| Arc::ptr_eq(&pair[0], &pair[1])),
        "one shared env"
    );
    assert_eq!(fake.runs().len(), 1, "one shell for every caller");
    // A refresh runs the shell again; the env already handed out stays as it was.
    resolver.refresh();
    let again = resolver.get().await;
    assert!(!Arc::ptr_eq(&again, &envs[0]));
    assert_eq!(fake.runs().len(), 2);
}

#[tokio::test]
async fn launch_context_stripped_from_resolver() {
    let fake = FakeShell::new("zsh");
    let config = fake.config(
        "ok",
        &[
            ("TERM_PROGRAM", "Apple_Terminal"),
            ("TERM_PROGRAM_VERSION", "455"),
            ("TERM_SESSION_ID", "w0t0p0"),
            ("SHELL_SESSION_HISTFILE", "/x"),
            ("npm_config_registry", "https://registry.example"),
            ("npm_lifecycle_event", "dev"),
            ("INIT_CWD", "/repo"),
            ("PNPM_HOME", "/pnpm"),
            ("TAURI_ENV_PLATFORM", "darwin"),
            ("NPM_TOKEN", "the-users-own-token"),
            // The user's rc exports one of them itself: that one comes back.
            ("GHOST_TESTBIN_RC", "TERM_PROGRAM=from-rc"),
        ],
    );
    let (_, env) = resolve(config).await;
    assert!(matches!(env.source, EnvSource::LoginShell { .. }));
    for name in [
        "TERM_PROGRAM_VERSION",
        "TERM_SESSION_ID",
        "SHELL_SESSION_HISTFILE",
        "npm_config_registry",
        "npm_lifecycle_event",
        "INIT_CWD",
        "PNPM_HOME",
        "TAURI_ENV_PLATFORM",
    ] {
        assert_eq!(get(&env, name), None, "{name} never reaches the shell");
    }
    assert_eq!(get(&env, "TERM_PROGRAM"), Some("from-rc"));
    assert_eq!(get(&env, "NPM_TOKEN"), Some("the-users-own-token"));
}

#[tokio::test]
async fn a_missing_home_is_a_fallback_with_a_reason() {
    let fake = FakeShell::new("zsh");
    let config = EnvResolverConfig {
        home: Some(fake.dir.path().join("no-such-home")),
        ..fake.config("ok", &[])
    };
    let (_, env) = resolve(config).await;
    let EnvSource::Fallback { reason } = &env.source else {
        panic!("a fallback: {:?}", env.source);
    };
    assert!(reason.contains("no-such-home"), "{reason}");
    assert!(fake.runs().is_empty());
}
