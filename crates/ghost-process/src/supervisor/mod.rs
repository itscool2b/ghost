//! Linux-side ownership over an explicit, leased control connection. This module is testable with
//! fake processes on every OS; deployment remains gated by [`crate::HostExecution`]. Only the
//! Linux binary may attach the production spawner. No shell command or vendor config is involved.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::{
    LaunchSpec, PIPE_EOF_GRACE, ProcessCwd, ProcessSpawner, ProtocolLog, Redactor, ResolvedEnv,
    STDERR_TAIL_SHOWN_BYTES, STDOUT_CHANNEL_BYTES, ShutdownPlan, SpawnSpec, StdoutItem,
};

pub mod environment;
pub mod inspect;
pub mod wire;
use wire::{Ended, Frame, LaunchRequest, Oversized, PROTOCOL_VERSION, Ready, Tag, invalid};

/// The environment and redactor are resolved together on Linux. They never cross the wire.
pub struct SupervisorContext {
    pub spawner: Arc<dyn ProcessSpawner>,
    pub env: Arc<ResolvedEnv>,
    pub redactor: Arc<Redactor>,
    pub log: ProtocolLog,
    /// The Linux endpoint's SIGTERM/SIGINT/SIGHUP bridge. No signal handler kills the relay first.
    pub shutdown: Option<tokio::sync::watch::Receiver<bool>>,
}

#[derive(Clone)]
struct Output {
    tx: mpsc::Sender<(Frame, OwnedSemaphorePermit)>,
    bytes: Arc<Semaphore>,
}

impl Output {
    async fn send(&self, frame: Frame) -> io::Result<()> {
        let size = frame.payload.len().clamp(1, STDOUT_CHANNEL_BYTES) as u32;
        let permit = Arc::clone(&self.bytes)
            .acquire_many_owned(size)
            .await
            .map_err(io::Error::other)?;
        self.tx
            .send((frame, permit))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "supervisor output closed"))
    }
}

struct Tasks {
    fixed: Vec<tokio::task::AbortHandle>,
    periodic: Option<tokio::task::AbortHandle>,
}
impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.fixed {
            task.abort();
        }
        if let Some(task) = &self.periodic {
            task.abort();
        }
    }
}

/// Serves exactly one owned process. EOF, malformed control, a missed lease or a broken output
/// kills it through the ordinary shutdown chain. A second Launch cannot start or replace a child.
pub async fn serve<R, W>(reader: R, writer: W, context: SupervisorContext) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    serve_with_lease(reader, writer, context, wire::LEASE).await
}

async fn serve_with_lease<R, W>(
    mut reader: R,
    mut writer: W,
    mut context: SupervisorContext,
    lease: Duration,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut shutdown = context.shutdown.take();
    let (input, mut commands) = mpsc::channel(1);
    let read_task = tokio::spawn(async move {
        loop {
            let next = wire::read_frame(&mut reader).await;
            let terminal = !matches!(&next, Ok(Some(_)));
            if input.send(next).await.is_err() || terminal {
                break;
            }
        }
    });
    let (tx, mut frames) = mpsc::channel::<(Frame, OwnedSemaphorePermit)>(16);
    let bytes = Arc::new(Semaphore::new(STDOUT_CHANNEL_BYTES));
    let output = Output {
        tx,
        bytes: Arc::clone(&bytes),
    };
    let mut write_task = tokio::spawn(async move {
        while let Some((frame, _permit)) = frames.recv().await {
            if wire::write_frame(&mut writer, &frame).await.is_err() {
                break;
            }
        }
        bytes.close();
    });
    let mut tasks = Tasks {
        fixed: vec![read_task.abort_handle(), write_task.abort_handle()],
        periodic: None,
    };
    let Some(mut first) = setup_frame(&mut commands, &mut shutdown, lease).await? else {
        return Ok(());
    };
    let mut preparation = None;
    if first.tag == Tag::Inspect {
        let request: inspect::InspectRequest = first.decode()?;
        request.validate()?;
        let env = Arc::clone(&context.env);
        let task = tokio::task::spawn_blocking(move || inspect::Preparation::new(request, &env));
        let result = tokio::select! {
            _ = stopping(&mut shutdown) => return Err(io::Error::new(io::ErrorKind::Interrupted, "supervisor stopped during inspection")),
            result = tokio::time::timeout(lease, task) => result,
        }
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "host inspection timed out"))?
            .map_err(io::Error::other)??;
        tokio::time::timeout(
            lease,
            output.send(Frame::json(Tag::Inspection, result.response())?),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "host inspection output stalled"))??;
        preparation = Some(result);
        // Keep the exact resolved login environment alive. A new connection cannot use this
        // token, even if a second shell happens to report the same paths.
        let Some(next) = setup_frame(&mut commands, &mut shutdown, lease).await? else {
            return Ok(());
        };
        first = next;
    }
    if first.tag != Tag::Launch {
        return Err(invalid("supervisor expected its launch frame"));
    }
    let request: LaunchRequest = first.decode()?;
    request.validate()?;
    if let Some(preparation) = preparation {
        let env = Arc::clone(&context.env);
        let launch = request.clone();
        let task = tokio::task::spawn_blocking(move || preparation.verify(&launch, &env));
        tokio::select! {
            _ = stopping(&mut shutdown) => return Err(io::Error::new(io::ErrorKind::Interrupted, "supervisor stopped during preparation")),
            result = tokio::time::timeout(lease, task) => result,
        }
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "host preparation timed out"))?
            .map_err(io::Error::other)??;
    } else if request.preparation.is_some() {
        return Err(invalid(
            "supervisor preparation requires inspection on this connection",
        ));
    }
    context.redactor.set_secrets(crate::secret_values(
        &request.set_env.effective(&context.env).vars,
    ));
    let spec = SpawnSpec {
        launch: LaunchSpec {
            label: "WSL agent".into(),
            program: request.program.into(),
            args: request.args.into_iter().map(Into::into).collect(),
            cwd: ProcessCwd::ChatFolder,
            set_env: request.set_env.entries(),
            daemons: request.daemons,
            startup_timeout: lease,
        },
        folder: request.cwd.into(),
        env: context.env,
        max_line_bytes: request.max_line_bytes,
    };
    // A cancelled or timed-out spawn retains the ordinary SpawnGuard's cleanup guarantee.
    let spawned = tokio::select! {
        _ = stopping(&mut shutdown) => return Err(io::Error::new(io::ErrorKind::Interrupted, "supervisor stopped during launch")),
        spawned = tokio::time::timeout(lease, context.spawner.spawn(spec, context.log)) => spawned,
    }
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "supervisor spawn timed out"))?
        .map_err(|_| io::Error::other("supervisor could not start the requested process"))?;
    let control = spawned.control;
    let mut stdout = spawned.io.stdout;
    let stdin = spawned.io.stdin;
    tokio::time::timeout(
        lease,
        output.send(Frame::json(
            Tag::Ready,
            &Ready {
                protocol: PROTOCOL_VERSION,
                pid: control.pid(),
            },
        )?),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "supervisor output stalled"))??;
    let stream_output = output.clone();
    let mut stream = tokio::spawn(async move {
        while let Some(item) = stdout.next().await {
            let frame = match item {
                StdoutItem::Line(line) => Frame {
                    tag: Tag::Stdout,
                    payload: line.into_bytes(),
                },
                StdoutItem::Oversized { bytes, keys } => Frame::json(
                    Tag::Oversized,
                    &Oversized {
                        bytes,
                        id: keys.id,
                        method: keys.method,
                        kind: keys.kind,
                    },
                )?,
            };
            stream_output.send(frame).await?;
        }
        Ok::<_, io::Error>(())
    });
    tasks.fixed.push(stream.abort_handle());
    let mut deadline = Instant::now() + lease;
    let mut stdin_closed = false;
    let mut snapshot: Option<JoinHandle<()>> = None;
    let mut record = tokio::time::interval(Duration::from_secs(1));
    let failure = loop {
        tokio::select! {
            biased;
            _ = stopping(&mut shutdown) => break Some("supervisor endpoint stopping"),
            _ = output.tx.closed() => break Some("supervisor output closed"),
            _ = tokio::time::sleep_until(deadline) => break Some("supervisor parent lease expired"),
            next = commands.recv() => {
                let frame = match next {
                    Some(Ok(Some(frame))) => frame,
                    Some(Err(_)) => break Some("invalid supervisor control frame"),
                    _ => break Some("supervisor parent disconnected"),
                };
                match frame.tag {
                    Tag::Heartbeat => deadline = Instant::now() + lease,
                    Tag::Stdin if !stdin_closed => {
                        let Ok(line) = String::from_utf8(frame.payload) else { break Some("invalid supervisor stdin encoding"); };
                        if line.contains(['\n', '\r']) || stdin.send(line).is_err() {
                            break Some("supervisor stdin is closed or not a single line");
                        }
                    }
                    Tag::CloseStdin => { stdin_closed = true; stdin.close(); }
                    Tag::Stop => break None,
                    _ => break Some("unexpected supervisor control frame"),
                }
            }
            _ = control.exited() => break None,
            _ = stdin.writer_closed(), if !stdin_closed => {
                if control.try_exited().is_none() { break Some("agent stdin writer stopped"); }
            }
            _ = record.tick(), if snapshot.as_ref().is_none_or(JoinHandle::is_finished) => {
                let owned = control.clone();
                let task = tokio::spawn(async move { owned.record_escaped().await; });
                tasks.periodic = Some(task.abort_handle());
                snapshot = Some(task);
            }
        }
    };
    // Stop accepting commands before cleanup. Queued Stdin frames cannot reach a later process.
    read_task.abort();
    commands.close();
    let report = control.shutdown(ShutdownPlan::FORCE).await;
    if tokio::time::timeout(PIPE_EOF_GRACE, &mut stream)
        .await
        .is_err()
    {
        stream.abort();
    }
    if let Some(message) = failure {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            output.send(Frame::json(Tag::Failure, &message)?),
        )
        .await;
    }
    let cleanup_confirmed = report.leftovers.is_empty() && report.exit.is_some();
    let ended = Ended {
        exit: report.exit,
        leftovers: report.leftovers,
        spared: report.spared,
        stderr_tail: context
            .redactor
            .redact_text(&control.stderr_tail(STDERR_TAIL_SHOWN_BYTES))
            .into_owned(),
    };
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        output.send(Frame::json(Tag::Ended, &ended)?),
    )
    .await;
    drop(output);
    let _ = tokio::time::timeout(PIPE_EOF_GRACE, &mut write_task).await;
    if cleanup_confirmed {
        Ok(())
    } else {
        Err(io::Error::other(
            "supervisor could not confirm owned process cleanup",
        ))
    }
}

async fn setup_frame(
    commands: &mut mpsc::Receiver<io::Result<Option<Frame>>>,
    shutdown: &mut Option<tokio::sync::watch::Receiver<bool>>,
    lease: Duration,
) -> io::Result<Option<Frame>> {
    loop {
        let next = tokio::select! {
            _ = stopping(shutdown) => return Err(io::Error::new(io::ErrorKind::Interrupted, "supervisor stopped before launch")),
            next = tokio::time::timeout(lease, commands.recv()) => next,
        };
        match next {
            Ok(Some(Ok(Some(Frame {
                tag: Tag::Heartbeat,
                ..
            })))) => {}
            Ok(Some(Ok(Some(Frame { tag: Tag::Stop, .. })))) => return Ok(None),
            Ok(Some(Ok(Some(frame)))) => return Ok(Some(frame)),
            _ => return Err(invalid("supervisor expected its setup frame")),
        }
    }
}

async fn stopping(signal: &mut Option<tokio::sync::watch::Receiver<bool>>) {
    match signal {
        Some(signal) => {
            let _ = signal.wait_for(|stop| *stop).await;
        }
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod tests;
