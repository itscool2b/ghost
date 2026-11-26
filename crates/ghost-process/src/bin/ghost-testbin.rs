//! A small, predictable child process for ghost-process's tests: an agent that behaves exactly as a
//! test needs, a fake login shell, and processes that escape their group the ways real agent trees
//! do. Every long-lived mode exits by itself after ten minutes, so a failed test can never leave a
//! process behind for good; the tests kill what they spawn well before that.
//!
//! Modes (the first argument):
//! - `json-lines <n> [pad]`: prints `n` JSON lines (each padded with `pad` bytes), then exits.
//! - `flood-stderr <bytes>`: writes `bytes` to stderr, then `{"done":true}` to stdout, then exits.
//! - `ignore-sigterm`: ignores SIGTERM and stdin's end, prints `{"ready":true}`, and waits.
//! - `spawn-group-child <pidfile> [wait|exit|hang]`: starts `hang` in this process's group, writes
//!   its pid, prints ready, then waits for stdin's end (`wait`), exits at once (`exit`), or sleeps
//!   whatever stdin does (`hang`).
//! - `spawn-setsid-daemon <pidfile>`: starts `daemon run` in a new session (macOS has no `setsid`
//!   command); the daemon starts a `hang` child and writes both pids. Then waits for stdin's end.
//! - `spawn-group-daemon <pidfile>`: the same daemon in a group of its own, same session.
//! - `spawn-pty-child <pidfile>`: `hang` in a new session, standing in for a PTY host or a detached
//!   Node child. Then waits for stdin's end.
//! - `spawn-job-child <pidfile>`: `hang` in a group of its own, same session (shell job control).
//! - `daemon run <pidfile>`: the daemon those modes start.
//! - `exit-before-read <code>`: exits with `code` at once.
//! - `exit-with-buffered <lines>`: writes `lines` JSON lines and exits at once.
//! - `oversized <bytes> <request|response|notification>`: one line of about `bytes` whose `id`
//!   comes after its huge payload, then `{"after":true}`.
//! - `noise`: noise lines mixed with JSON lines.
//! - `hang [seconds]`: sleeps (ignoring stdin).
//! - `sleep <seconds>`: sleeps.
//! - `print-env`: prints `{"env":{...}}`. `print-cwd`: prints `{"cwd":"..."}`.
//! - `echo`: echoes stdin lines until its end, then prints `{"eof":true}` and exits 0.
//! - `raw-echo`: copies stdin bytes unchanged to stdout until end of file.
//! - `orphan <seconds>`: starts `sleep <seconds>` through ghost-process's `Spawner`, prints its pid,
//!   waits for a line on stdin, then exits without running destructors, the way a crash would.
//!
//! Invoked under a shell's name (`zsh`, `bash`, `fish`, `tcsh`, ...), it is a fake login shell:
//! `GHOST_TESTBIN_SHELL` picks `ok`, `junk`, `hang`, `hang-interactive`, `nonzero`, `no-marker` or
//! `no-path`; `GHOST_TESTBIN_RC` (`A=1;B=2`) adds variables as an rc file would; each run appends its
//! argv to the file `GHOST_TESTBIN_RECORD` names, and `hang` starts a child in a group of its own
//! (same session) and appends its own and that child's pid to `GHOST_TESTBIN_PIDFILE`.

use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// How long any waiting mode lives at most.
const LIFETIME: Duration = Duration::from_secs(600);

/// Starts the arguments of a child this binary spawns itself, whatever name it runs under.
const MODE_FLAG: &str = "--testbin-mode";

const SHELLS: &[&str] = &[
    "bash", "zsh", "sh", "dash", "ksh", "mksh", "yash", "fish", "tcsh", "csh",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let invoked = args
        .first()
        .and_then(|argv0| Path::new(argv0).file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let rest = args.get(1..).unwrap_or_default();
    // A copy running under a shell's name is a fake shell, except for the children it starts
    // itself, which name their mode after this flag.
    let result = match rest.split_first() {
        Some((flag, mode)) if flag == MODE_FLAG => run(mode),
        _ if SHELLS.contains(&invoked.as_str()) => fake_shell(rest),
        _ => run(rest),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ghost-testbin: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode> {
    let (mode, rest) = args
        .split_first()
        .ok_or("missing mode; see the module docs")?;
    match mode.as_str() {
        "terminal-echo" => {
            println!(
                "terminal-ready stdin={} stdout={} args={}",
                io::stdin().is_terminal(),
                io::stdout().is_terminal(),
                serde_json::to_string(rest)?
            );
            for line in io::stdin().lock().lines() {
                let line = line?;
                if line == "quit" {
                    break;
                }
                if line == "size" {
                    println!("terminal-size {}", terminal_dimensions()?);
                } else {
                    println!("terminal-received {line}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        "raw-echo" => {
            let mut input = io::stdin().lock();
            let mut output = io::stdout().lock();
            let mut buffer = [0; 4096];
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                output.write_all(&buffer[..count])?;
                output.flush()?;
            }
            Ok(ExitCode::SUCCESS)
        }
        "json-lines" => json_lines(number(rest, 0)?, number(rest, 1).unwrap_or(0)),
        "flood-stderr" => {
            write_repeated(&mut io::stderr().lock(), number(rest, 0)?)?;
            println!(r#"{{"done":true}}"#);
            Ok(ExitCode::SUCCESS)
        }
        "ignore-sigterm" => ignore_sigterm(),
        "spawn-group-child" => {
            let pidfile = path(rest, 0)?;
            let child = spawn_self(&["hang"], Placement::Inherit)?;
            write_pids(&pidfile, &[child])?;
            ready()?;
            match rest.get(1).map(String::as_str) {
                Some("exit") => Ok(ExitCode::SUCCESS),
                Some("hang") => {
                    std::thread::sleep(LIFETIME);
                    Ok(ExitCode::SUCCESS)
                }
                _ => wait_for_eof(),
            }
        }
        "spawn-setsid-daemon" | "spawn-group-daemon" => {
            let pidfile = path(rest, 0)?;
            let placement = if mode == "spawn-setsid-daemon" {
                Placement::Session
            } else {
                Placement::Group
            };
            let pidfile_arg = pidfile.to_string_lossy().into_owned();
            spawn_self(&["daemon", "run", &pidfile_arg], placement)?;
            // The daemon writes its pidfile once its own child runs.
            wait_for_file(&pidfile)?;
            ready()?;
            wait_for_eof()
        }
        // A grandchild whose parent has exited, as `Start-Process` leaves one: `orphan-parent`
        // starts `hang`, writes its pid and exits at once.
        "spawn-orphan" => {
            let pidfile = path(rest, 0)?;
            let pidfile_arg = pidfile.to_string_lossy().into_owned();
            spawn_self(&["orphan-parent", &pidfile_arg], Placement::Inherit)?;
            wait_for_file(&pidfile)?;
            ready()?;
            wait_for_eof()
        }
        "orphan-parent" => {
            let pidfile = path(rest, 0)?;
            let grandchild = spawn_self(&["hang"], Placement::Inherit)?;
            // Its own pid second: it is still exiting when the file appears, and a test that
            // needs it gone waits for that pid.
            write_pids(&pidfile, &[grandchild, std::process::id()])?;
            Ok(ExitCode::SUCCESS)
        }
        "spawn-pty-child" | "spawn-job-child" => {
            let pidfile = path(rest, 0)?;
            let placement = if mode == "spawn-pty-child" {
                Placement::Session
            } else {
                Placement::Group
            };
            let child = spawn_self(&["hang"], placement)?;
            write_pids(&pidfile, &[child])?;
            ready()?;
            wait_for_eof()
        }
        "daemon" => {
            let pidfile = path(rest, 1)?;
            let child = spawn_self(&["hang"], Placement::Inherit)?;
            write_pids(&pidfile, &[std::process::id(), child])?;
            std::thread::sleep(LIFETIME);
            Ok(ExitCode::SUCCESS)
        }
        "exit-before-read" => Ok(ExitCode::from(u8::try_from(number(rest, 0)?)?)),
        "exit-with-buffered" => {
            let mut out = io::stdout().lock();
            for index in 0..number(rest, 0)? {
                writeln!(
                    out,
                    r#"{{"jsonrpc":"2.0","method":"buffered","params":{{"i":{index}}}}}"#
                )?;
            }
            out.flush()?;
            Ok(ExitCode::SUCCESS)
        }
        "oversized" => oversized(
            number(rest, 0)?,
            rest.get(1).map_or("notification", String::as_str),
        ),
        "noise" => {
            let mut out = io::stdout().lock();
            writeln!(out, "Debugger listening on ws://127.0.0.1:9229/0f2c")?;
            writeln!(out, r#"{{"jsonrpc":"2.0","method":"first"}}"#)?;
            writeln!(out, "npm notice: a new version is available")?;
            writeln!(out, "   ")?;
            writeln!(out, r#"  {{"jsonrpc":"2.0","method":"second"}}"#)?;
            out.flush()?;
            Ok(ExitCode::SUCCESS)
        }
        "hang" => {
            let seconds = number(rest, 0).map_or(LIFETIME, Duration::from_secs);
            std::thread::sleep(seconds);
            Ok(ExitCode::SUCCESS)
        }
        "sleep" => {
            std::thread::sleep(Duration::from_secs(number(rest, 0)?));
            Ok(ExitCode::SUCCESS)
        }
        "print-env" => {
            let env: serde_json::Map<String, serde_json::Value> = std::env::vars_os()
                .map(|(name, value)| {
                    (
                        name.to_string_lossy().into_owned(),
                        serde_json::Value::from(value.to_string_lossy().into_owned()),
                    )
                })
                .collect();
            println!("{}", serde_json::json!({ "env": env }));
            Ok(ExitCode::SUCCESS)
        }
        "print-cwd" => {
            let cwd = std::env::current_dir()?;
            println!("{}", serde_json::json!({ "cwd": cwd.to_string_lossy() }));
            Ok(ExitCode::SUCCESS)
        }
        "echo" => {
            let stdin = io::stdin();
            let mut out = io::stdout().lock();
            for line in stdin.lock().lines() {
                writeln!(out, "{}", line?)?;
                out.flush()?;
            }
            writeln!(out, r#"{{"eof":true}}"#)?;
            Ok(ExitCode::SUCCESS)
        }
        "orphan" => orphan(number(rest, 0)?),
        other => Err(format!("unknown mode {other:?}").into()),
    }
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "read the harmless test process's own console dimensions"
)]
fn terminal_dimensions() -> Result<String> {
    use windows::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: the handle is this test process's stdout; info is a correctly typed output buffer.
    unsafe {
        GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE)?, &mut info)?;
    }
    Ok(format!(
        "{}x{}",
        info.srWindow.Right - info.srWindow.Left + 1,
        info.srWindow.Bottom - info.srWindow.Top + 1
    ))
}

#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "read the harmless test process's own terminal dimensions"
)]
fn terminal_dimensions() -> Result<String> {
    let mut size = nix::libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: stdout is an open descriptor and size is the expected ioctl output buffer.
    if unsafe { nix::libc::ioctl(1, nix::libc::TIOCGWINSZ, &mut size) } == -1 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(format!("{}x{}", size.ws_col, size.ws_row))
}

fn number(args: &[String], index: usize) -> Result<u64> {
    let raw = args.get(index).ok_or("missing numeric argument")?;
    Ok(raw.parse()?)
}

fn path(args: &[String], index: usize) -> Result<PathBuf> {
    Ok(PathBuf::from(
        args.get(index).ok_or("missing path argument")?,
    ))
}

fn ready() -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, r#"{{"ready":true}}"#)?;
    out.flush()?;
    Ok(())
}

/// Reads stdin until its end, then exits 0.
fn wait_for_eof() -> Result<ExitCode> {
    let mut sink = Vec::new();
    io::copy(&mut io::stdin().lock(), &mut sink)?;
    Ok(ExitCode::SUCCESS)
}

fn json_lines(count: u64, pad: u64) -> Result<ExitCode> {
    let pad = "p".repeat(usize::try_from(pad)?);
    let mut out = io::stdout().lock();
    for index in 0..count {
        writeln!(
            out,
            r#"{{"jsonrpc":"2.0","method":"tick","params":{{"i":{index},"pad":"{pad}"}}}}"#
        )?;
    }
    out.flush()?;
    Ok(ExitCode::SUCCESS)
}

fn write_repeated(out: &mut impl Write, mut remaining: u64) -> io::Result<()> {
    let mut chunk = [b'e'; 8192];
    // Newlines every 100 bytes, so the log sees lines.
    for (index, byte) in chunk.iter_mut().enumerate() {
        if index % 100 == 99 {
            *byte = b'\n';
        }
    }
    while remaining > 0 {
        let len = usize::try_from(remaining).map_or(chunk.len(), |left| left.min(chunk.len()));
        out.write_all(chunk.get(..len).unwrap_or_default())?;
        remaining -= len as u64;
    }
    out.flush()
}

fn oversized(bytes: u64, kind: &str) -> Result<ExitCode> {
    let payload = "x".repeat(usize::try_from(bytes)?);
    let line = match kind {
        "request" => format!(
            r#"{{"jsonrpc":"2.0","method":"session/request_permission","params":{{"blob":"{payload}"}},"id":7}}"#
        ),
        "response" => format!(r#"{{"jsonrpc":"2.0","result":{{"blob":"{payload}"}},"id":7}}"#),
        _ => format!(
            r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"blob":"{payload}"}}}}"#
        ),
    };
    let mut out = io::stdout().lock();
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n{\"after\":true}\n")?;
    out.flush()?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(unix)]
fn ignore_sigterm() -> Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        ready()?;
        let deadline = tokio::time::sleep(LIFETIME);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = terminate.recv() => eprintln!("ignoring SIGTERM"),
                () = &mut deadline => return Ok(ExitCode::SUCCESS),
            }
        }
    })
}

#[cfg(not(unix))]
fn ignore_sigterm() -> Result<ExitCode> {
    ready()?;
    std::thread::sleep(LIFETIME);
    Ok(ExitCode::SUCCESS)
}

/// Where a spawned child goes relative to this process.
#[derive(Clone, Copy)]
enum Placement {
    /// This process's group and session.
    Inherit,
    /// A group of its own, this session.
    Group,
    /// A new session.
    Session,
}

/// Starts this binary again with `args`; returns the child's pid. The child is never waited for:
/// it lives on after this process, as the processes it stands in for do.
fn spawn_self(args: &[&str], placement: Placement) -> Result<u32> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let _context = runtime.enter();
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .arg(MODE_FLAG)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut wrapped = process_wrap::tokio::CommandWrap::from(command);
    #[cfg(unix)]
    match placement {
        Placement::Inherit => {}
        Placement::Group => {
            wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
        }
        Placement::Session => {
            wrapped.wrap(process_wrap::tokio::ProcessSession);
        }
    }
    #[cfg(not(unix))]
    let _ = placement;
    let child = wrapped.spawn()?;
    let pid = child.id().ok_or("the child has no pid")?;
    // Leak the handle: dropping it would be harmless (no kill-on-drop), but the child must
    // outlive this process without anything waiting on it here.
    std::mem::forget(child);
    std::mem::forget(runtime);
    Ok(pid)
}

/// Writes pids atomically, space-separated.
fn write_pids(path: &Path, pids: &[u32]) -> Result<()> {
    let text = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

fn wait_for_file(path: &Path) -> Result<()> {
    for _ in 0..3000 {
        if path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(format!("{} never appeared", path.display()).into())
}

#[cfg(unix)]
fn os_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    value.to_string_lossy().into_owned().into_bytes()
}

/// A login shell that prints its environment between the markers of its `-c` command.
fn fake_shell(args: &[String]) -> Result<ExitCode> {
    if let Some(record) = std::env::var_os("GHOST_TESTBIN_RECORD") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(record)?;
        // One write per line, so concurrent runs never interleave.
        file.write_all(format!("{}\n", serde_json::to_string(args)?).as_bytes())?;
    }
    let mode = std::env::var("GHOST_TESTBIN_SHELL").unwrap_or_else(|_| "ok".into());
    let interactive = args.iter().any(|arg| arg == "-i");
    if mode == "hang" || (mode == "hang-interactive" && interactive) {
        // A background job of an rc file: a group of its own in the shell's session, which only
        // killing the whole session reaches.
        let child = spawn_self(&["hang"], Placement::Group)?;
        if let Some(pidfile) = std::env::var_os("GHOST_TESTBIN_PIDFILE") {
            // Appended: each run of the shell adds its own line.
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(pidfile)?;
            file.write_all(format!("{} {child}\n", std::process::id()).as_bytes())?;
        }
        std::thread::sleep(LIFETIME);
        return Ok(ExitCode::SUCCESS);
    }
    let command = args
        .iter()
        .position(|arg| arg == "-c")
        .and_then(|at| args.get(at + 1))
        .ok_or("no -c command")?;
    let mut markers = command
        .split('\'')
        .filter(|piece| piece.starts_with("__AURA_ENV_"));
    let start = markers.next().ok_or("no start marker")?;
    let end = markers.next().ok_or("no end marker")?;

    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    if let Ok(exports) = std::env::var("GHOST_TESTBIN_RC") {
        for export in exports.split(';').filter(|export| !export.is_empty()) {
            if let Some((name, value)) = export.split_once('=') {
                vars.retain(|(known, _)| known != name);
                vars.push((name.into(), value.replace("\\n", "\n").into()));
            }
        }
    }
    if mode == "no-path" {
        vars.retain(|(name, _)| name != "PATH");
    }
    // A shell's own bookkeeping, which the resolver removes.
    vars.push(("SHLVL".into(), "2".into()));
    vars.push(("PWD".into(), "/somewhere".into()));
    vars.push(("_".into(), "/usr/bin/env".into()));

    let mut out = io::stdout().lock();
    if mode == "junk" {
        out.write_all(b"Last login: today\n__AURA_ENV_0000000000000000_START__ from an old run\n")?;
    }
    if mode != "no-marker" {
        out.write_all(start.as_bytes())?;
    }
    for (name, value) in &vars {
        out.write_all(&os_bytes(name))?;
        out.write_all(b"=")?;
        out.write_all(&os_bytes(value))?;
        out.write_all(b"\0")?;
    }
    if mode != "no-marker" {
        out.write_all(end.as_bytes())?;
    }
    if mode == "junk" {
        out.write_all(b"\nlogout\n")?;
    }
    out.flush()?;
    if mode == "nonzero" {
        return Ok(ExitCode::from(3));
    }
    Ok(ExitCode::SUCCESS)
}

/// Starts a sleeper through the real spawner, reports its pid, waits for a line, and exits
/// without cleanup: only the kernel can then reach the sleeper (a job object does; Unix does not).
fn orphan(seconds: u64) -> Result<ExitCode> {
    use std::sync::Arc;

    use ghost_process::{
        EnvSource, LaunchSpec, LeftoverRegistry, ProcessCwd, ProcessSpawner, ProtocolLog,
        ResolvedEnv, SpawnSpec, Spawner, process_env,
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let pid = runtime.block_on(async {
        let spawner = Spawner::new(
            LeftoverRegistry::disabled(),
            tokio::runtime::Handle::current(),
        );
        let folder = std::env::temp_dir();
        let spec = SpawnSpec {
            launch: LaunchSpec {
                label: "orphaned sleeper".into(),
                program: std::env::current_exe()?,
                args: vec!["sleep".into(), seconds.to_string().into()],
                cwd: ProcessCwd::Fixed(folder.clone()),
                set_env: Vec::new(),
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(60),
            },
            folder,
            env: Arc::new(ResolvedEnv {
                vars: process_env(),
                source: EnvSource::Fallback {
                    reason: "test".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            max_line_bytes: ghost_process::MAX_LINE_BYTES,
        };
        let spawned = spawner.spawn(spec, ProtocolLog::disabled()).await?;
        let pid = spawned.control.pid();
        // Never dropped: dropping the control would kill the sleeper.
        std::mem::forget(spawned);
        Ok::<u32, Box<dyn std::error::Error>>(pid)
    })?;
    println!("{pid}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    std::process::exit(0)
}
