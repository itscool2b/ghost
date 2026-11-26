//! A fake of every program detection and the adapter install run, for ghost-agents' tests: tests copy
//! it under the names `node`, `npm`, `claude`, `codex` and `hermes` (with `.exe` on Windows) into a
//! folder on the env's `PATH`, so no real agent, adapter or npm ever runs. It lives in this crate
//! because a test can only find the binaries of its own package.
//!
//! Everything it does is set by files in the folder `GHOST_FAKE_DIR` names, which tests may change
//! between runs: `<name>.mode` picks a behaviour (default `ok`), `<name>.version` what `--version`
//! prints. Each run appends `{"cwd", "argv", "update_notifier"}` to `<name>.log`; a hanging run
//! writes its own and its child's pid to `<name>.pids`.
//!
//! - `--version` (every name): `ok` prints the version; `fail` prints an error to stderr and exits
//!   1; `hang` starts a child and sleeps.
//! - `node <entry> --version`: prints the `FAKE_ADAPTER_VERSION=` the entry file holds, or fails
//!   like a missing module. `node <entry>`: prints `{"cwd", "entry"}` and exits (a launched adapter).
//! - `npm ci ...`: `ok` installs a fake package for the first dependency of `./package.json`, with
//!   a `bin` named after its last path segment; `slow` does the same after 1.5 s; `fail` prints npm's
//!   404 lines and exits 1; `partial` leaves the `bin` file out; `broken-entry` writes an entry
//!   node cannot load; `hang` writes part of the package, starts a child and sleeps.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// How long any waiting mode lives at most.
const LIFETIME: Duration = Duration::from_secs(300);

/// The argument of a child this binary starts itself: it only sleeps, whatever name it runs under.
const SLEEP_FLAG: &str = "--fake-sleep";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == SLEEP_FLAG) {
        std::thread::sleep(LIFETIME);
        return ExitCode::SUCCESS;
    }
    let name = args
        .first()
        .and_then(|argv0| Path::new(argv0).file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    match run(&name, args.get(1..).unwrap_or_default()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{name}: {error}");
            ExitCode::FAILURE
        }
    }
}

fn fake_dir() -> Result<PathBuf> {
    std::env::var_os("GHOST_FAKE_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "GHOST_FAKE_DIR is not set".into())
}

fn setting(name: &str, kind: &str) -> Option<String> {
    let dir = fake_dir().ok()?;
    std::fs::read_to_string(dir.join(format!("{name}.{kind}")))
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn record(name: &str, args: &[String]) -> Result<()> {
    let line = serde_json::json!({
        "cwd": std::env::current_dir()?.to_string_lossy(),
        "argv": args,
        "update_notifier": std::env::var("npm_config_update_notifier").ok(),
    });
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(fake_dir()?.join(format!("{name}.log")))?;
    // One write per line: concurrent probes append to the same log.
    log.write_all(format!("{line}\n").as_bytes())?;
    Ok(())
}

fn run(name: &str, args: &[String]) -> Result<ExitCode> {
    record(name, args)?;
    let mode = setting(name, "mode").unwrap_or_else(|| "ok".into());
    match (name, args.first().map(String::as_str)) {
        ("claude", Some("auth")) | ("codex", Some("login")) => {
            if setting(name, "maintenance").as_deref() == Some("fail") {
                eprintln!("Error checking login status: private diagnostic");
                return Ok(ExitCode::FAILURE);
            }
            if name == "claude" {
                println!(
                    "{{\"loggedIn\":true,\"email\":\"private@example.test\",\"token\":\"discard\"}}"
                );
            } else {
                eprintln!("Logged in using ChatGPT");
            }
            Ok(ExitCode::SUCCESS)
        }
        ("claude" | "codex", Some("update")) => {
            match setting(name, "maintenance").as_deref() {
                Some("slow") => std::thread::sleep(Duration::from_millis(500)),
                Some("fail") => return Ok(ExitCode::FAILURE),
                Some("break-version") => {
                    std::fs::write(fake_dir()?.join(format!("{name}.version")), "unknown")?
                }
                Some("verify-gated") => {
                    std::fs::write(fake_dir()?.join(format!("{name}.mode")), "gated-version")?
                }
                _ => {}
            }
            println!("Updated");
            Ok(ExitCode::SUCCESS)
        }
        ("claude" | "codex", Some("plugin" | "mcp")) | ("pi", Some("list")) => listing(name, args),
        ("npm", _) => npm(&mode),
        (_, Some("--version")) => version(name, &mode),
        ("node", Some(entry)) => {
            let entry = PathBuf::from(entry);
            if args.get(1).is_some_and(|arg| arg == "--version") {
                adapter_version(&entry)
            } else {
                println!(
                    "{}",
                    serde_json::json!({
                        "cwd": std::env::current_dir()?.to_string_lossy(),
                        "entry": entry.to_string_lossy(),
                    })
                );
                Ok(ExitCode::SUCCESS)
            }
        }
        _ => Err(format!("no fake behaviour for {name} {args:?}").into()),
    }
}

/// A listing command reads its answers from `fake-listing.json` in the agent's own config home,
/// found only through the agent's documented variable, so a test proves the home it was given was
/// the one used. Each answer is `{"stdout", "stderr", "code", "hang"}`, keyed by the joined args.
fn listing(name: &str, args: &[String]) -> Result<ExitCode> {
    let variable = match name {
        "claude" => "CLAUDE_CONFIG_DIR",
        "codex" => "CODEX_HOME",
        _ => "PI_CODING_AGENT_DIR",
    };
    let line = serde_json::json!({
        "argv": args,
        "home": std::env::var(variable).ok(),
        "pi_offline": std::env::var("PI_OFFLINE").ok(),
        "no_color": std::env::var("NO_COLOR").ok(),
    });
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(fake_dir()?.join(format!("{name}.listing.log")))?;
    log.write_all(format!("{line}\n").as_bytes())?;
    let Some(home) = std::env::var_os(variable) else {
        eprintln!("{variable} is not set");
        return Ok(ExitCode::from(3));
    };
    let answers: serde_json::Value =
        serde_json::from_slice(&std::fs::read(Path::new(&home).join("fake-listing.json"))?)?;
    let Some(answer) = answers.get(args.join(" ")) else {
        eprintln!("error: unknown option '{}'", args.join(" "));
        return Ok(ExitCode::from(2));
    };
    if let Some(delay) = answer["delay_ms"].as_u64() {
        std::thread::sleep(Duration::from_millis(delay.min(15_000)));
    }
    if let Some(release) = answer["wait_for"].as_str() {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !Path::new(&home).join(release).exists() {
            if std::time::Instant::now() >= deadline {
                return Ok(ExitCode::FAILURE);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    if answer["hang"] == true {
        return hang(name);
    }
    print!("{}", answer["stdout"].as_str().unwrap_or_default());
    eprint!("{}", answer["stderr"].as_str().unwrap_or_default());
    Ok(ExitCode::from(
        u8::try_from(answer["code"].as_u64().unwrap_or(0)).unwrap_or(1),
    ))
}

fn version(name: &str, mode: &str) -> Result<ExitCode> {
    if mode == "gated-version" {
        let root = fake_dir()?;
        std::fs::write(root.join("verification-ready"), "ready")?;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !root.join("verification-release").exists() {
            if std::time::Instant::now() >= deadline {
                return Ok(ExitCode::FAILURE);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    match mode {
        "fail" => {
            eprintln!("ImportError: No module named '{name}_cli'");
            eprintln!("  (a second line nobody quotes)");
            Ok(ExitCode::FAILURE)
        }
        "hang" => hang(name),
        _ => {
            let printed = setting(name, "version").unwrap_or_else(|| match name {
                "node" => "v26.0.0".into(),
                "claude" => "2.1.310 (Claude Code)".into(),
                "codex" => "codex-cli 0.164.0".into(),
                "hermes" => "Hermes Agent v0.22.5".into(),
                _ => "1.0.0".into(),
            });
            println!("{printed}");
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn adapter_version(entry: &Path) -> Result<ExitCode> {
    let text = std::fs::read_to_string(entry).unwrap_or_default();
    match text.split("FAKE_ADAPTER_VERSION=").nth(1) {
        Some(rest) => {
            println!("{}", rest.split_whitespace().next().unwrap_or_default());
            Ok(ExitCode::SUCCESS)
        }
        None => {
            eprintln!("Error: Cannot find module '{}'", entry.display());
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Starts a sleeping child, records both pids, and sleeps: a tree for a timeout or a drop to kill.
fn hang(name: &str) -> Result<ExitCode> {
    let child = std::process::Command::new(std::env::current_exe()?)
        .arg(SLEEP_FLAG)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let pids = format!("{} {}", std::process::id(), child.id());
    let path = fake_dir()?.join(format!("{name}.pids"));
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    std::fs::write(&temp, pids)?;
    std::fs::rename(&temp, &path)?;
    std::thread::sleep(LIFETIME);
    Ok(ExitCode::SUCCESS)
}

fn npm(mode: &str) -> Result<ExitCode> {
    if mode == "fail" {
        eprintln!("npm warn config production Use `--omit=dev` instead.");
        eprintln!("npm error code E404");
        eprintln!(
            "npm error 404 Not Found - GET https://registry.npmjs.org/@agentclientprotocol%2fx - Not found"
        );
        return Ok(ExitCode::FAILURE);
    }
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read("package.json")?)?;
    let (package, version) = manifest["dependencies"]
        .as_object()
        .and_then(|dependencies| dependencies.iter().next())
        .and_then(|(package, version)| Some((package.clone(), version.as_str()?.to_owned())))
        .ok_or("package.json has no dependency")?;
    let bin = package.rsplit('/').next().unwrap_or(&package).to_owned();
    let dir = Path::new("node_modules").join(&package);
    std::fs::create_dir_all(dir.join("dist"))?;
    let package_json = serde_json::json!({
        "name": package,
        "version": version,
        "bin": { bin: "dist/index.js" },
    });
    std::fs::write(dir.join("package.json"), package_json.to_string())?;
    match mode {
        "hang" => return hang("npm"),
        "partial" => return Ok(ExitCode::SUCCESS),
        "slow" => std::thread::sleep(Duration::from_millis(1500)),
        _ => {}
    }
    let entry = if mode == "broken-entry" {
        "// half a module\n".to_owned()
    } else {
        format!("// FAKE_ADAPTER_VERSION={version}\n")
    };
    std::fs::write(dir.join("dist").join("index.js"), entry)?;
    println!("added 1 package in 1s");
    Ok(ExitCode::SUCCESS)
}
