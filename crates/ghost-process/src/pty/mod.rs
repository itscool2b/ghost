//! Interactive native terminals with bounded, unlogged output and owned-process cleanup.
//! Windows uses ConPTY and a job assigned before the suspended child runs. Unix opens a real
//! controlling terminal in an owned session. This module never chooses or invokes a shell.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use crate::{
    ByteSink, ExitInfo, LeftoverRegistry, PIPE_EOF_GRACE, ProcessControl, ProcessCwd, ProtocolLog,
    ShutdownPlan, ShutdownReport, SpawnError, SpawnSpec, child_env,
};
use process_wrap::tokio::ChildWrapper;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

const CHUNK_BYTES: usize = 8192;
const OUTPUT_CHUNKS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}
impl PtySize {
    pub fn validate(self) -> io::Result<Self> {
        if self.rows == 0 || self.cols == 0 || self.rows > 1000 || self.cols > 1000 {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal size must be between 1 and 1000 rows and columns",
            ))
        } else {
            Ok(self)
        }
    }
}

trait Terminal: Send + Sync {
    fn resize(&self, size: PtySize) -> io::Result<()>;
    /// Must not block the runtime or prevent the output reader from draining.
    fn close(&self);
}
struct Prepared {
    child: Box<dyn ChildWrapper>,
    input: Box<dyn AsyncWrite + Unpin + Send>,
    output: Box<dyn AsyncRead + Unpin + Send>,
    terminal: Arc<dyn Terminal>,
}

struct Owner {
    process: ProcessControl,
    input: ByteSink,
    terminal: Arc<dyn Terminal>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.input.close();
        self.process.kill_tree();
        self.terminal.close();
    }
}

/// Clones share terminal ownership. Dropping the last control ends only this owned tree.
#[derive(Clone)]
pub struct PtyControl(Arc<Owner>);
impl PtyControl {
    pub fn pid(&self) -> u32 {
        self.0.process.pid()
    }
    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        self.0.terminal.resize(size.validate()?)
    }
    pub fn kill(&self) {
        self.0.input.close();
        self.0.process.kill_tree();
        self.0.terminal.close();
    }
    pub async fn shutdown(&self) -> ShutdownReport {
        self.0.input.close();
        let report = self.0.process.shutdown(ShutdownPlan::FORCE).await;
        self.0.terminal.close();
        report
    }
    pub async fn exited(&self) -> ExitInfo {
        self.0.process.exited().await
    }
    pub fn try_exited(&self) -> Option<ExitInfo> {
        self.0.process.try_exited()
    }
}
impl std::fmt::Debug for PtyControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyControl")
            .field("pid", &self.pid())
            .finish_non_exhaustive()
    }
}

pub struct PtyOutput {
    rx: mpsc::Receiver<Vec<u8>>,
    truncated: Arc<AtomicBool>,
}
impl PtyOutput {
    pub async fn next(&mut self) -> Option<Vec<u8>> {
        self.rx.recv().await
    }
    /// True if a read failed or the post-exit drain deadline dropped output.
    pub fn truncated(&self) -> bool {
        self.truncated.load(Ordering::Acquire)
    }
}
pub struct PtyProcess {
    pub input: ByteSink,
    pub output: PtyOutput,
    pub control: PtyControl,
}

pub(crate) async fn spawn(
    registry: Arc<LeftoverRegistry>,
    rt: Handle,
    spec: SpawnSpec,
    size: PtySize,
) -> Result<PtyProcess, SpawnError> {
    let error = |source| crate::spawn::spawn_error(&spec.launch.program, source);
    size.validate().map_err(error)?;
    let cwd = match &spec.launch.cwd {
        ProcessCwd::ChatFolder => &spec.folder,
        ProcessCwd::Fixed(path) => path,
    };
    let (mut vars, _) = child_env(&spec.env, &spec.launch.set_env);
    terminal_identity(&mut vars, &spec.launch.set_env);
    crate::spawn::check(&rt, cwd, &spec.launch.program, &vars).await?;
    #[cfg(windows)]
    let prepared = windows::prepare(&spec.launch.program, &spec.launch.args, cwd, &vars, size)
        .await
        .map_err(error)?;
    #[cfg(unix)]
    let prepared =
        unix::prepare(&spec.launch.program, &spec.launch.args, cwd, &vars, size).map_err(error)?;
    let (input, receiver) = ByteSink::channel();
    let process = crate::spawn::own_child(
        registry,
        rt.clone(),
        prepared.child,
        &spec.launch,
        input.clone(),
        ProtocolLog::disabled(),
        Arc::new(crate::stderr::StderrTail::default()),
    )
    .await?;
    let exited = process.exited();
    let (tx, rx) = mpsc::channel(OUTPUT_CHUNKS);
    let truncated = Arc::new(AtomicBool::new(false));
    rt.spawn(receiver.forward(prepared.input));
    rt.spawn(read_output(
        prepared.output,
        tx,
        exited,
        Arc::clone(&prepared.terminal),
        Arc::clone(&truncated),
    ));
    Ok(PtyProcess {
        input: input.clone(),
        output: PtyOutput { rx, truncated },
        control: PtyControl(Arc::new(Owner {
            process,
            input,
            terminal: prepared.terminal,
        })),
    })
}

async fn read_output(
    mut output: Box<dyn AsyncRead + Unpin + Send>,
    tx: mpsc::Sender<Vec<u8>>,
    mut exited: futures::future::BoxFuture<'static, ExitInfo>,
    terminal: Arc<dyn Terminal>,
    truncated: Arc<AtomicBool>,
) {
    let mut deadline = None;
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        let count = tokio::select! {
            biased;
            _ = deadline_reached(deadline) => { truncated.store(true, Ordering::Release); break; },
            _ = &mut exited, if deadline.is_none() => {
                terminal.close();
                deadline = Some(tokio::time::Instant::now() + PIPE_EOF_GRACE);
                continue;
            },
            read = output.read(&mut buffer) => match read {
                Ok(0) => break,
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => break,
                Err(_) => { truncated.store(true, Ordering::Release); break; },
                Ok(count) => count,
            },
        };
        let send = tx.send(buffer[..count].to_vec());
        tokio::pin!(send);
        loop {
            tokio::select! {
                biased;
                _ = deadline_reached(deadline) => {
                    truncated.store(true, Ordering::Release);
                    return;
                },
                _ = &mut exited, if deadline.is_none() => {
                    terminal.close();
                    deadline = Some(tokio::time::Instant::now() + PIPE_EOF_GRACE);
                },
                result = &mut send => {
                    // A vanished viewer cannot stall ConPTY's close. Drain without retaining data.
                    if result.is_err() { truncated.store(true, Ordering::Release); }
                    break;
                }
            }
        }
    }
}

async fn deadline_reached(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// What every terminal inside Ghost says it is, whatever the environment it came from: xterm.js
/// speaks xterm's codes with 24-bit colour. A `TERM` inherited from the terminal Ghost was
/// started from (`xterm-kitty`, `xterm-ghostty`) would make programs send codes this terminal
/// does not draw, and without `COLORTERM` agents fall back to 256 colours. A launch's own
/// `set_env` still wins.
const TERMINAL_IDENTITY: &[(&str, &str)] = &[
    ("TERM", "xterm-256color"),
    ("COLORTERM", "truecolor"),
    ("TERM_PROGRAM", "ghost"),
];

fn terminal_identity(
    vars: &mut crate::EnvMap,
    set_env: &[(std::ffi::OsString, std::ffi::OsString)],
) {
    for (name, value) in TERMINAL_IDENTITY {
        let key = crate::EnvKey::from(*name);
        if set_env
            .iter()
            .any(|(set, _)| crate::EnvKey::from(set.as_os_str()) == key)
        {
            continue;
        }
        vars.insert(key, (*value).into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvKey, EnvMap};

    fn value(vars: &EnvMap, name: &str) -> Option<String> {
        vars.get(&EnvKey::from(name))
            .map(|value| value.to_string_lossy().into_owned())
    }

    #[test]
    fn a_terminal_says_what_it_is_unless_its_launch_does() {
        let mut vars = EnvMap::new();
        vars.insert(EnvKey::from("TERM"), "xterm-kitty".into());
        vars.insert(EnvKey::from("PATH"), "/usr/bin".into());
        terminal_identity(&mut vars, &[]);
        assert_eq!(value(&vars, "TERM").as_deref(), Some("xterm-256color"));
        assert_eq!(value(&vars, "COLORTERM").as_deref(), Some("truecolor"));
        assert_eq!(value(&vars, "TERM_PROGRAM").as_deref(), Some("ghost"));
        assert_eq!(value(&vars, "PATH").as_deref(), Some("/usr/bin"));

        let mut vars = EnvMap::new();
        vars.insert(EnvKey::from("TERM"), "dumb".into());
        terminal_identity(&mut vars, &[("TERM".into(), "dumb".into())]);
        assert_eq!(value(&vars, "TERM").as_deref(), Some("dumb"));
        assert_eq!(value(&vars, "COLORTERM").as_deref(), Some("truecolor"));
    }
}
