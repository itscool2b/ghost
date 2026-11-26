//! In-memory agent processes for other crates' tests (feature `testing`).
//!
//! [`FakeSpawner`] implements [`ProcessSpawner`]. Every spawn hands the test a [`FakeAgentEnd`], the
//! agent's side of the pipes, and returns a [`SpawnedProcess`] whose control is a [`FakeControl`].
//! The driver sees what it would see with a real agent: a stdin that never blocks, a stdout bounded
//! like the real reader's (16 lines, 8 MiB), the stderr tail, an exit, and kills. Lines go through
//! the spawn's [`ProtocolLog`] as they would from a real process.
//!
//! The agent's exit and the end of its stdout are independent, as with a real agent whose
//! descendant still holds the pipe: send [`FakeLine::Eof`] (or drop `to_client`) to end stdout, and
//! send on `exit` to end the process. Dropping `exit` unsent counts as an exit without a code. A
//! kill ends both: the stream after the lines already written, the process with signal 9.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::sync::{mpsc, oneshot, watch};

use crate::STDERR_TAIL_BYTES;
use crate::control::{ControlImpl, ExitInfo, ProcessControl};
use crate::env::child_env;
use crate::io::{AgentIo, LineSink, StdinMsg, StdoutItem, StdoutSender, TopKeys, stdout_channel};
use crate::log::{Direction, LogNote, ProtocolLog};
use crate::shutdown::{ShutdownPlan, ShutdownReport, ShutdownStep};
use crate::spawn::{ProcessCwd, ProcessSpawner, SpawnError, SpawnSpec, SpawnedProcess};

/// The first pid a [`FakeSpawner`] hands out; later spawns count up from it.
pub const FIRST_FAKE_PID: u32 = 90_000;

/// The signal a kill reports, as SIGKILL does on Unix.
const KILLED_SIGNAL: i32 = 9;

/// Spawns [`FakeControl`] processes and hands their pipes to the test.
pub struct FakeSpawner {
    state: Mutex<SpawnerState>,
}

struct SpawnerState {
    ends: mpsc::UnboundedSender<FakeAgentEnd>,
    failures: VecDeque<SpawnError>,
    next_pid: u32,
    controls: Vec<Arc<FakeControl>>,
}

impl FakeSpawner {
    /// A spawner and the receiver that gets every spawned agent's end, in spawn order.
    pub fn new() -> (Arc<FakeSpawner>, mpsc::UnboundedReceiver<FakeAgentEnd>) {
        let (ends, receiver) = mpsc::unbounded_channel();
        let spawner = FakeSpawner {
            state: Mutex::new(SpawnerState {
                ends,
                failures: VecDeque::new(),
                next_pid: FIRST_FAKE_PID,
                controls: Vec::new(),
            }),
        };
        (Arc::new(spawner), receiver)
    }

    /// The next spawn fails with `error` instead of starting. Several calls queue up, one per spawn.
    pub fn fail_next(&self, error: SpawnError) {
        lock(&self.state).failures.push_back(error);
    }

    /// Every control this spawner created, in spawn order, so a test can read what was called on
    /// each ([`FakeControl::calls`]).
    pub fn controls(&self) -> Vec<Arc<FakeControl>> {
        lock(&self.state).controls.clone()
    }
}

impl fmt::Debug for FakeSpawner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = lock(&self.state);
        f.debug_struct("FakeSpawner")
            .field("spawned", &state.controls.len())
            .field("failures_queued", &state.failures.len())
            .finish()
    }
}

impl ProcessSpawner for FakeSpawner {
    fn spawn(
        &self,
        spec: SpawnSpec,
        log: ProtocolLog,
    ) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        let mut state = lock(&self.state);
        if let Some(error) = state.failures.pop_front() {
            return async move { Err(error) }.boxed();
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return async { Err(SpawnError::NoRuntime) }.boxed();
        };
        let pid = state.next_pid;
        state.next_pid = state.next_pid.wrapping_add(1);
        let (stdin, stdin_rx) = LineSink::channel();
        let (killed, killed_rx) = watch::channel(false);
        let inner = Arc::new(FakeInner {
            pid,
            stdin: stdin.clone(),
            exit: watch::Sender::new(None),
            killed,
            stderr: Mutex::new(String::new()),
            calls: Mutex::new(Vec::new()),
            chain: Mutex::new(None),
            log: log.clone(),
        });
        let control = Arc::new(FakeControl {
            inner: Arc::clone(&inner),
        });
        state.controls.push(Arc::clone(&control));
        let ends = state.ends.clone();
        drop(state);

        let (from_client_tx, from_client) = mpsc::unbounded_channel();
        let (to_client, to_client_rx) = mpsc::unbounded_channel();
        let (stderr, stderr_rx) = mpsc::unbounded_channel();
        let (exit, exit_rx) = oneshot::channel();
        let (stdout_tx, stdout) = stdout_channel();
        runtime.spawn(forward_stdin(stdin_rx, from_client_tx, Arc::clone(&inner)));
        runtime.spawn(forward_stdout(
            to_client_rx,
            stdout_tx,
            killed_rx.clone(),
            log.clone(),
        ));
        runtime.spawn(collect_stderr(stderr_rx, Arc::clone(&inner)));
        runtime.spawn(await_exit(exit_rx, Arc::clone(&inner)));

        log.note(spawn_note(&spec, pid));
        let end = FakeAgentEnd {
            spec,
            from_client,
            to_client,
            stderr,
            exit,
            killed: killed_rx,
        };
        // A test that dropped its receiver drops the end too: its agent then sees EOF and exits.
        let _ = ends.send(end);
        let process = SpawnedProcess {
            spawn_id: log.spawn_id().to_owned(),
            io: AgentIo { stdin, stdout },
            control: ProcessControl::new(control),
            log,
        };
        async move { Ok(process) }.boxed()
    }
}

/// The note a real spawn writes: names only, never values.
fn spawn_note(spec: &SpawnSpec, pid: u32) -> LogNote {
    let launch = &spec.launch;
    let cwd = match &launch.cwd {
        ProcessCwd::ChatFolder => &spec.folder,
        ProcessCwd::Fixed(folder) => folder,
    };
    let (_, scrubbed) = child_env(&spec.env, &launch.set_env);
    LogNote::Spawn {
        label: launch.label.clone(),
        program: launch.program.display().to_string(),
        args: launch
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        cwd: cwd.display().to_string(),
        removed: scrubbed.removed,
        set: launch
            .set_env
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect(),
        pid,
    }
}

/// The agent's side of one fake process.
#[derive(Debug)]
pub struct FakeAgentEnd {
    /// What the driver asked to spawn.
    pub spec: SpawnSpec,
    /// Every line the driver wrote to stdin. Ends when stdin closes or the process exits.
    pub from_client: mpsc::UnboundedReceiver<String>,
    /// Lines for the driver's stdout.
    pub to_client: mpsc::UnboundedSender<FakeLine>,
    /// Lines for the stderr tail and the log.
    pub stderr: mpsc::UnboundedSender<String>,
    /// Ends the process with this exit.
    pub exit: oneshot::Sender<ExitInfo>,
    /// Becomes true when the process is killed (`kill_tree`, or the end of a shutdown chain) before
    /// it exited by itself.
    pub killed: watch::Receiver<bool>,
}

/// What a fake agent writes to stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeLine {
    Line(String),
    /// A line over the cap, as the real framer reports one.
    Oversized {
        bytes: u64,
        keys: TopKeys,
    },
    /// End of stdout.
    Eof,
}

/// A call made on a [`FakeControl`], in the order made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeCall {
    CloseStdin,
    KillTree,
    Shutdown(ShutdownPlan),
    RecordEscaped,
}

/// The control of a fake process. Records every call; [`FakeSpawner::controls`] hands them out.
pub struct FakeControl {
    inner: Arc<FakeInner>,
}

struct FakeInner {
    pid: u32,
    stdin: LineSink,
    exit: watch::Sender<Option<ExitInfo>>,
    killed: watch::Sender<bool>,
    stderr: Mutex<String>,
    calls: Mutex<Vec<FakeCall>>,
    chain: Mutex<Option<Shared<BoxFuture<'static, ShutdownReport>>>>,
    log: ProtocolLog,
}

impl FakeControl {
    /// Every call made so far, in order.
    pub fn calls(&self) -> Vec<FakeCall> {
        lock(&self.inner.calls).clone()
    }

    /// Whether the process was killed before it exited by itself.
    pub fn was_killed(&self) -> bool {
        *self.inner.killed.borrow()
    }
}

impl fmt::Debug for FakeControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeControl")
            .field("pid", &self.inner.pid)
            .field("exit", &*self.inner.exit.borrow())
            .field("calls", &self.calls())
            .finish()
    }
}

impl ControlImpl for FakeControl {
    fn pid(&self) -> u32 {
        self.inner.pid
    }

    fn close_stdin(&self) {
        self.inner.record(FakeCall::CloseStdin);
        self.inner.stdin.close();
    }

    fn kill_tree(&self) {
        self.inner.record(FakeCall::KillTree);
        self.inner.kill();
    }

    fn shutdown(&self, plan: ShutdownPlan) -> BoxFuture<'static, ShutdownReport> {
        self.inner.record(FakeCall::Shutdown(plan));
        let mut chain = lock(&self.inner.chain);
        let shared = chain
            .get_or_insert_with(|| run_chain(Arc::clone(&self.inner), plan).boxed().shared())
            .clone();
        shared.boxed()
    }

    fn exited(&self) -> BoxFuture<'static, ExitInfo> {
        let mut exit = self.inner.exit.subscribe();
        async move {
            let found = exit
                .wait_for(Option::is_some)
                .await
                .ok()
                .and_then(|exit| *exit);
            found.unwrap_or_else(unknown_exit)
        }
        .boxed()
    }

    fn try_exited(&self) -> Option<ExitInfo> {
        *self.inner.exit.borrow()
    }

    fn record_escaped(&self) -> BoxFuture<'static, ()> {
        self.inner.record(FakeCall::RecordEscaped);
        async {}.boxed()
    }

    fn stderr_tail(&self, max_bytes: usize) -> String {
        let tail = lock(&self.inner.stderr);
        tail_of(&tail, max_bytes).to_owned()
    }
}

impl FakeInner {
    fn record(&self, call: FakeCall) {
        lock(&self.calls).push(call);
    }

    /// Records the exit unless the process already ended. Returns whether this call ended it.
    fn set_exit(&self, info: ExitInfo) -> bool {
        let ended = self.exit.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(info);
            true
        });
        if ended {
            self.log.note(LogNote::Exit {
                code: info.code,
                signal: info.signal,
            });
        }
        ended
    }

    /// Ends the process with SIGKILL, unless it already exited. Needs no runtime.
    fn kill(&self) {
        let killed = ExitInfo {
            code: None,
            signal: Some(KILLED_SIGNAL),
            at_ms: now_millis(),
        };
        if self.set_exit(killed) {
            self.killed.send_replace(true);
        }
    }

    /// The exit, if it comes within `limit`.
    async fn wait_exit(&self, limit: Duration) -> Option<ExitInfo> {
        let mut exit = self.exit.subscribe();
        tokio::time::timeout(limit, exit.wait_for(Option::is_some))
            .await
            .ok()
            .and_then(|found| found.ok().and_then(|exit| *exit))
    }
}

/// The shutdown chain of a fake process: close stdin (unless the plan skips it) and wait for the
/// exit, "send SIGTERM" (a fake process cannot see signals) and wait again, then kill.
async fn run_chain(inner: Arc<FakeInner>, plan: ShutdownPlan) -> ShutdownReport {
    let started = tokio::time::Instant::now();
    let mut steps = Vec::new();
    let mut exit = *inner.exit.borrow();
    if exit.is_some() {
        steps.push(ShutdownStep::AlreadyExited);
    }
    if exit.is_none() && !plan.stdin_eof_grace.is_zero() {
        inner.stdin.close();
        steps.push(ShutdownStep::ClosedStdin);
        exit = inner.wait_exit(plan.stdin_eof_grace).await;
        if exit.is_some() {
            steps.push(ShutdownStep::ExitedAfterEof);
        }
    }
    if exit.is_none() {
        steps.push(ShutdownStep::SentSigterm { processes: 1 });
        exit = inner.wait_exit(plan.sigterm_grace).await;
        if exit.is_some() {
            steps.push(ShutdownStep::ExitedAfterSigterm);
        }
    }
    if exit.is_none() {
        inner.kill();
        steps.push(ShutdownStep::Killed { processes: 1 });
        exit = inner.wait_exit(plan.kill_confirm).await;
    }
    let report = ShutdownReport {
        exit,
        steps,
        leftovers: Vec::new(),
        spared: Vec::new(),
        took: started.elapsed(),
    };
    inner.log.note(LogNote::Shutdown {
        steps: report
            .steps
            .iter()
            .map(|step| format!("{step:?}"))
            .collect(),
        took_ms: u64::try_from(report.took.as_millis()).unwrap_or(u64::MAX),
    });
    report
}

/// Passes the driver's lines to the test until stdin closes, the test stops reading, or the process
/// exits (a real process reads nothing after its exit).
async fn forward_stdin(
    mut lines: mpsc::UnboundedReceiver<StdinMsg>,
    to_agent: mpsc::UnboundedSender<String>,
    inner: Arc<FakeInner>,
) {
    let mut exit = inner.exit.subscribe();
    loop {
        tokio::select! {
            msg = lines.recv() => match msg {
                Some(StdinMsg::Line(line)) => {
                    inner.log.line(Direction::Out, &line);
                    if to_agent.send(line).is_err() {
                        return;
                    }
                }
                Some(StdinMsg::Close) | None => return,
            },
            () = until_exited(&mut exit) => return,
        }
    }
}

/// Passes the test's lines to the driver's bounded stdout until EOF or a kill.
async fn forward_stdout(
    mut lines: mpsc::UnboundedReceiver<FakeLine>,
    stdout: StdoutSender,
    mut killed: watch::Receiver<bool>,
    log: ProtocolLog,
) {
    loop {
        let line = tokio::select! {
            biased;
            line = lines.recv() => line,
            () = until_killed(&mut killed) => {
                // The stream ends after what the agent wrote before it died.
                while let Ok(line) = lines.try_recv() {
                    if !forward_line(line, &stdout, &log).await {
                        return;
                    }
                }
                return;
            }
        };
        match line {
            Some(line) => {
                if !forward_line(line, &stdout, &log).await {
                    return;
                }
            }
            None => return,
        }
    }
}

/// Returns once the process was killed (or its control is gone). The borrowed value is released
/// here, so no lock guard is held across the caller's awaits.
async fn until_killed(killed: &mut watch::Receiver<bool>) {
    let _ = killed.wait_for(|killed| *killed).await;
}

/// Returns once the process exited (or its control is gone).
async fn until_exited(exit: &mut watch::Receiver<Option<ExitInfo>>) {
    let _ = exit.wait_for(Option::is_some).await;
}

/// Forwards one line; false once the stream should end (EOF, or the driver dropped stdout).
async fn forward_line(line: FakeLine, stdout: &StdoutSender, log: &ProtocolLog) -> bool {
    let item = match line {
        FakeLine::Line(text) => {
            log.line(Direction::In, &text);
            StdoutItem::Line(text)
        }
        FakeLine::Oversized { bytes, keys } => {
            log.note(LogNote::Oversized {
                bytes,
                method: keys.method.clone(),
            });
            StdoutItem::Oversized { bytes, keys }
        }
        FakeLine::Eof => return false,
    };
    stdout.send(item).await.is_ok()
}

async fn collect_stderr(mut lines: mpsc::UnboundedReceiver<String>, inner: Arc<FakeInner>) {
    while let Some(line) = lines.recv().await {
        inner.log.line(Direction::Err, &line);
        let mut tail = lock(&inner.stderr);
        tail.push_str(&line);
        tail.push('\n');
        if tail.len() > STDERR_TAIL_BYTES {
            let keep = tail_of(&tail, STDERR_TAIL_BYTES).to_owned();
            *tail = keep;
        }
    }
}

async fn await_exit(exit: oneshot::Receiver<ExitInfo>, inner: Arc<FakeInner>) {
    let info = exit.await.unwrap_or_else(|_| unknown_exit());
    inner.set_exit(info);
}

/// The last `max_bytes` of `text`, starting on a character boundary.
fn tail_of(text: &str, max_bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(max_bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.get(start..).unwrap_or_default()
}

/// An exit nobody reported a code for.
fn unknown_exit() -> ExitInfo {
    ExitInfo {
        code: None,
        signal: None,
        at_ms: now_millis(),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Locks `mutex`, recovering the data of a thread that panicked while holding it: a test double
/// must keep answering after a test's own panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;
    use crate::env::{EnvKey, EnvMap, EnvSource, ResolvedEnv};
    use crate::spawn::LaunchSpec;

    fn spec() -> SpawnSpec {
        let mut vars = EnvMap::new();
        vars.insert(EnvKey::from("PATH"), OsString::from("/usr/bin"));
        vars.insert(EnvKey::from("CLAUDECODE"), OsString::from("1"));
        SpawnSpec {
            launch: LaunchSpec {
                label: "fake acp".into(),
                program: PathBuf::from("/bin/fake"),
                args: vec!["acp".into()],
                cwd: ProcessCwd::ChatFolder,
                set_env: Vec::new(),
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(60),
            },
            folder: PathBuf::from("/work"),
            env: Arc::new(ResolvedEnv {
                vars,
                source: EnvSource::Fallback {
                    reason: "test".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            max_line_bytes: crate::MAX_LINE_BYTES,
        }
    }

    async fn spawn_one() -> (Arc<FakeSpawner>, SpawnedProcess, FakeAgentEnd) {
        let (spawner, mut ends) = FakeSpawner::new();
        let process = spawner
            .spawn(spec(), ProtocolLog::disabled())
            .await
            .expect("spawn");
        let end = ends.recv().await.expect("an end");
        (spawner, process, end)
    }

    #[tokio::test]
    async fn lines_flow_both_ways_and_eof_ends_stdout() {
        let (_spawner, mut process, mut end) = spawn_one().await;
        assert_eq!(process.control.pid(), FIRST_FAKE_PID);
        assert_eq!(end.spec.launch.label, "fake acp");
        process.io.stdin.send("{\"id\":1}".into()).expect("stdin");
        assert_eq!(end.from_client.recv().await.as_deref(), Some("{\"id\":1}"));

        end.to_client
            .send(FakeLine::Line("{\"id\":1,\"result\":{}}".into()))
            .expect("stdout");
        end.to_client
            .send(FakeLine::Oversized {
                bytes: 70_000_000,
                keys: TopKeys::default(),
            })
            .expect("stdout");
        end.to_client.send(FakeLine::Eof).expect("eof");
        assert_eq!(
            process.io.stdout.next().await,
            Some(StdoutItem::Line("{\"id\":1,\"result\":{}}".into()))
        );
        assert!(matches!(
            process.io.stdout.next().await,
            Some(StdoutItem::Oversized {
                bytes: 70_000_000,
                ..
            })
        ));
        assert_eq!(process.io.stdout.next().await, None);
        assert_eq!(process.control.try_exited(), None, "EOF is not an exit");
    }

    #[tokio::test]
    async fn the_test_ends_the_process_and_the_driver_sees_the_exit() {
        let (_spawner, process, end) = spawn_one().await;
        end.stderr.send("boom".into()).expect("stderr");
        end.stderr.send("second line".into()).expect("stderr");
        let exit = ExitInfo {
            code: Some(3),
            signal: None,
            at_ms: 1,
        };
        end.exit.send(exit).expect("exit");
        assert_eq!(process.control.exited().await, exit);
        assert_eq!(process.control.try_exited(), Some(exit));
        // The stderr task may still be appending; wait for it.
        for _ in 0..100 {
            if process.control.stderr_tail(1024).contains("second") {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(process.control.stderr_tail(1024), "boom\nsecond line\n");
        assert_eq!(process.control.stderr_tail(5), "line\n");
        assert!(!*end.killed.borrow());
        // A process that exited takes no more stdin.
        for _ in 0..100 {
            if process.io.stdin.send("{}".into()).is_err() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("stdin still open after the exit");
    }

    #[tokio::test]
    async fn kill_tree_kills_once_and_records_the_call() {
        let (spawner, process, end) = spawn_one().await;
        process.control.kill_tree();
        process.control.kill_tree();
        let exit = process.control.exited().await;
        assert_eq!(exit.signal, Some(KILLED_SIGNAL));
        assert!(*end.killed.borrow());
        let control = &spawner.controls()[0];
        assert_eq!(control.calls(), [FakeCall::KillTree, FakeCall::KillTree]);
        assert!(control.was_killed());
        // The test's own exit comes too late to change anything.
        let _ = end.exit.send(ExitInfo {
            code: Some(0),
            signal: None,
            at_ms: 2,
        });
        assert_eq!(process.control.try_exited(), Some(exit));
    }

    #[tokio::test(start_paused = true)]
    async fn a_graceful_shutdown_closes_stdin_and_waits_for_the_exit() {
        let (spawner, process, mut end) = spawn_one().await;
        let agent = tokio::spawn(async move {
            // The agent exits once its stdin closes.
            while end.from_client.recv().await.is_some() {}
            end.exit
                .send(ExitInfo {
                    code: Some(0),
                    signal: None,
                    at_ms: 3,
                })
                .expect("exit");
            end.killed
        });
        let first = process.control.shutdown(ShutdownPlan::STANDARD);
        let second = process.control.shutdown(ShutdownPlan::FORCE);
        let (report, again) = tokio::join!(first, second);
        assert_eq!(report, again, "a second call joins the first chain");
        assert_eq!(
            report.steps,
            [ShutdownStep::ClosedStdin, ShutdownStep::ExitedAfterEof]
        );
        assert_eq!(report.exit.and_then(|exit| exit.code), Some(0));
        let killed = agent.await.expect("join");
        assert!(!*killed.borrow());
        assert_eq!(
            spawner.controls()[0].calls(),
            [
                FakeCall::Shutdown(ShutdownPlan::STANDARD),
                FakeCall::Shutdown(ShutdownPlan::FORCE)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_force_stop_of_a_wedged_process_ends_in_a_kill() {
        let (_spawner, process, end) = spawn_one().await;
        let report = process.control.shutdown(ShutdownPlan::FORCE).await;
        assert_eq!(
            report.steps,
            [
                ShutdownStep::SentSigterm { processes: 1 },
                ShutdownStep::Killed { processes: 1 }
            ]
        );
        assert_eq!(report.took, ShutdownPlan::FORCE.sigterm_grace);
        assert!(*end.killed.borrow());
        // The kill ended stdout too.
        let mut stdout = process.io.stdout;
        assert_eq!(stdout.next().await, None);
        // An exited process reports so at once.
        assert_eq!(
            process.control.shutdown(ShutdownPlan::STANDARD).await.steps,
            report.steps,
            "the chain is shared, even after it finished"
        );
    }

    #[tokio::test]
    async fn dropping_the_last_control_kills_the_process() {
        let (spawner, process, end) = spawn_one().await;
        let SpawnedProcess { control, .. } = process;
        let clone = control.clone();
        drop(control);
        assert!(!*end.killed.borrow());
        drop(clone);
        assert!(*end.killed.borrow());
        assert_eq!(spawner.controls()[0].calls(), [FakeCall::KillTree]);
    }

    #[tokio::test]
    async fn queued_failures_fail_spawns_in_order() {
        let (spawner, _ends) = FakeSpawner::new();
        spawner.fail_next(SpawnError::ProgramNotFound {
            program: "hermes".into(),
        });
        spawner.fail_next(SpawnError::NoRuntime);
        let first = spawner.spawn(spec(), ProtocolLog::disabled()).await;
        assert!(matches!(first, Err(SpawnError::ProgramNotFound { .. })));
        let second = spawner.spawn(spec(), ProtocolLog::disabled()).await;
        assert!(matches!(second, Err(SpawnError::NoRuntime)));
        let third = spawner
            .spawn(spec(), ProtocolLog::disabled())
            .await
            .expect("spawns again");
        assert_eq!(third.control.pid(), FIRST_FAKE_PID);
        assert_eq!(spawner.controls().len(), 1);
    }

    #[tokio::test]
    async fn a_dropped_end_reads_as_eof_and_an_exit() {
        let (_spawner, mut process, end) = spawn_one().await;
        drop(end);
        assert_eq!(process.io.stdout.next().await, None);
        let exit = process.control.exited().await;
        assert_eq!((exit.code, exit.signal), (None, None));
    }

    #[test]
    fn spawning_outside_a_runtime_fails() {
        use std::task::{Context, Poll, Waker};

        let (spawner, _ends) = FakeSpawner::new();
        let mut spawning = spawner.spawn(spec(), ProtocolLog::disabled());
        let mut context = Context::from_waker(Waker::noop());
        let result = match spawning.as_mut().poll(&mut context) {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("a spawn without a runtime must fail at once"),
        };
        assert!(matches!(result, Err(SpawnError::NoRuntime)));
        assert!(spawner.controls().is_empty());
    }

    #[test]
    fn tails_start_on_character_boundaries() {
        assert_eq!(tail_of("héllo", 4), "llo");
        assert_eq!(tail_of("abc", 10), "abc");
        assert_eq!(tail_of("abc", 0), "");
    }
}
