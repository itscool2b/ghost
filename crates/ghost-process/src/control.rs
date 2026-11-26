//! Controlling a running agent: stdin, kills, the shutdown chain, and its exit.
//!
//! [`ProcessControl`] is a cheap, cloneable handle that forwards to a [`ControlImpl`]: the real one
//! that `spawn` builds, or `testing::FakeControl`. The session keeps every control a driver spawned,
//! so force stop and quit reach the processes even when a driver is wedged. When the last handle
//! drops, the whole tree is killed.
//!
//! Exit detection waits on the direct child on every OS, never on its group or its job: a crashed
//! adapter whose descendant lingers must still look exited.

use std::fmt;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use process_wrap::tokio::ChildWrapper;
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::io::LineSink;
use crate::log::{LogNote, ProtocolLog, now_millis};
use crate::registry::RegistryHandle;
use crate::shutdown::{ShutdownPlan, ShutdownReport};
#[cfg(unix)]
use crate::spawn::DaemonMatch;
use crate::stderr::StderrTail;

/// What a [`ProcessControl`] forwards to.
pub trait ControlImpl: Send + Sync + 'static {
    fn pid(&self) -> u32;
    fn close_stdin(&self);
    /// Immediate and callable from any thread, with or without a runtime: a run-event callback at
    /// quit has none. Unix: the group is stopped at once, the walk for escaped descendants runs on
    /// a thread of its own, and the call returns once the group and those descendants got SIGKILL
    /// (the group gets it after a second at most, walk or not); Windows: a request to terminate
    /// the job.
    fn kill_tree(&self);
    /// The shutdown chain. Idempotent and shared: a second call joins the first chain.
    fn shutdown(&self, plan: ShutdownPlan) -> BoxFuture<'static, ShutdownReport>;
    /// Explicit cleanup recovery. Ordinary shutdown remains idempotent; implementations may
    /// retry a completed chain that could not confirm exit without replacing an in-flight one.
    fn retry_unconfirmed_shutdown(&self, plan: ShutdownPlan) -> BoxFuture<'static, ShutdownReport> {
        self.shutdown(plan)
    }
    /// The direct child's exit on every OS: never a job-wide or group-wide wait.
    fn exited(&self) -> BoxFuture<'static, ExitInfo>;
    fn try_exited(&self) -> Option<ExitInfo>;
    /// Unix: one process-table snapshot (on a blocking thread) recording, in the leftover registry,
    /// the descendants that left the agent's group, minus declared daemons. Windows: nothing.
    fn record_escaped(&self) -> BoxFuture<'static, ()>;
    /// The last `max_bytes` of stderr: lossy, ANSI stripped, NOT redacted.
    fn stderr_tail(&self, max_bytes: usize) -> String;
    /// Windows: the processes in the agent's own tree job ([`crate::tree`]), which includes
    /// programs whose parent already exited. `None` where there is no such job.
    fn job_members(&self) -> Option<Vec<u32>> {
        None
    }
}

/// A handle to a running agent. Methods forward to its [`ControlImpl`]; the last handle dropped
/// kills the tree.
#[derive(Clone)]
pub struct ProcessControl(Arc<KillOnLastDrop>);

/// Shared by every clone of one [`ProcessControl`]; dropping the last clone drops this.
struct KillOnLastDrop(Arc<dyn ControlImpl>);

impl Drop for KillOnLastDrop {
    fn drop(&mut self) {
        self.0.kill_tree();
    }
}

impl ProcessControl {
    pub fn new(imp: Arc<dyn ControlImpl>) -> ProcessControl {
        ProcessControl(Arc::new(KillOnLastDrop(imp)))
    }

    fn imp(&self) -> &dyn ControlImpl {
        self.0.0.as_ref()
    }

    pub fn pid(&self) -> u32 {
        self.imp().pid()
    }

    pub fn close_stdin(&self) {
        self.imp().close_stdin();
    }

    /// See [`ControlImpl::kill_tree`].
    pub fn kill_tree(&self) {
        self.imp().kill_tree();
    }

    /// See [`ControlImpl::shutdown`].
    pub fn shutdown(&self, plan: ShutdownPlan) -> BoxFuture<'static, ShutdownReport> {
        self.imp().shutdown(plan)
    }

    pub fn retry_unconfirmed_shutdown(
        &self,
        plan: ShutdownPlan,
    ) -> BoxFuture<'static, ShutdownReport> {
        self.imp().retry_unconfirmed_shutdown(plan)
    }

    pub fn exited(&self) -> BoxFuture<'static, ExitInfo> {
        self.imp().exited()
    }

    pub fn try_exited(&self) -> Option<ExitInfo> {
        self.imp().try_exited()
    }

    /// See [`ControlImpl::record_escaped`].
    pub fn record_escaped(&self) -> BoxFuture<'static, ()> {
        self.imp().record_escaped()
    }

    /// See [`ControlImpl::stderr_tail`].
    pub fn stderr_tail(&self, max_bytes: usize) -> String {
        self.imp().stderr_tail(max_bytes)
    }

    /// See [`ControlImpl::job_members`].
    pub fn job_members(&self) -> Option<Vec<u32>> {
        self.imp().job_members()
    }
}

impl fmt::Debug for ProcessControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessControl")
            .field("pid", &self.pid())
            .field("exited", &self.try_exited())
            .finish()
    }
}

/// How a process ended. `signal` is set (Unix) when a signal ended it; `at_ms` is Unix ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub at_ms: i64,
}

impl ExitInfo {
    pub(crate) fn from_status(status: std::process::ExitStatus) -> ExitInfo {
        #[cfg(unix)]
        let signal = {
            use std::os::unix::process::ExitStatusExt;
            status.signal()
        };
        #[cfg(not(unix))]
        let signal = None;
        ExitInfo {
            code: status.code(),
            signal,
            at_ms: now_millis(),
        }
    }

    /// An exit nobody could read a status for.
    pub(crate) fn unknown() -> ExitInfo {
        ExitInfo {
            code: None,
            signal: None,
            at_ms: now_millis(),
        }
    }
}

fn invalidate_unconfirmed(chain: &mut Option<Shared<BoxFuture<'static, ShutdownReport>>>) {
    if chain
        .as_ref()
        .and_then(Shared::peek)
        .is_some_and(|report| report.exit.is_none() || !report.leftovers.is_empty())
    {
        *chain = None;
    }
}

/// The shutdown chain can close either protocol lines or an unlogged binary transport.
pub(crate) enum StdinControl {
    Lines(LineSink),
    Bytes(crate::raw::ByteSink),
}

impl StdinControl {
    pub(crate) fn close(&self) {
        match self {
            Self::Lines(stdin) => stdin.close(),
            Self::Bytes(stdin) => stdin.close(),
        }
    }
}

impl From<LineSink> for StdinControl {
    fn from(stdin: LineSink) -> Self {
        Self::Lines(stdin)
    }
}

impl From<crate::raw::ByteSink> for StdinControl {
    fn from(stdin: crate::raw::ByteSink) -> Self {
        Self::Bytes(stdin)
    }
}

/// What a real agent's control and its shutdown chain share.
pub(crate) struct ControlState {
    pub(crate) pid: u32,
    /// Unix: the agent leads its own group, so this is its pid.
    #[cfg(unix)]
    pub(crate) pgid: u32,
    /// Unix: its start time, so a recycled pid is never taken for it.
    #[cfg(unix)]
    pub(crate) start: crate::ptable::StartTime,
    pub(crate) stdin: StdinControl,
    pub(crate) exit: watch::Receiver<Option<ExitInfo>>,
    pub(crate) tail: Arc<StderrTail>,
    pub(crate) log: ProtocolLog,
    pub(crate) rt: Handle,
    pub(crate) registry: RegistryHandle,
    /// Unix: spared by the kill rule (Windows has no daemons in a job).
    #[cfg(unix)]
    pub(crate) daemons: Vec<DaemonMatch>,
    /// Unix: descendants that left the group, as last recorded; kept while they still run.
    #[cfg(unix)]
    pub(crate) escaped: Mutex<Vec<crate::registry::EscapedProc>>,
    /// Unix: set once the agent's group is known empty for good (shared with the waiter, which
    /// looks right after it reaps the agent). Its id is free from then on, and another group may
    /// take it, so nothing signals the group again.
    #[cfg(unix)]
    pub(crate) group_gone: Arc<AtomicBool>,
    /// A shutdown that could not confirm cleanup must leave its recovery record intact.
    #[cfg(unix)]
    pub(crate) retain_registry: AtomicBool,
    /// Windows: kill requests to the waiter, which owns the job.
    #[cfg(windows)]
    pub(crate) kill: tokio::sync::mpsc::UnboundedSender<()>,
    /// Windows: the job the agent joined to list its tree ([`crate::tree`]); never kills.
    #[cfg(windows)]
    pub(crate) tree: Option<crate::tree::TreeJob>,
    chain: Mutex<Option<Shared<BoxFuture<'static, ShutdownReport>>>>,
}

impl ControlState {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        pid: u32,
        #[cfg(unix)] start: crate::ptable::StartTime,
        stdin: impl Into<StdinControl>,
        exit: watch::Receiver<Option<ExitInfo>>,
        tail: Arc<StderrTail>,
        log: ProtocolLog,
        rt: Handle,
        registry: RegistryHandle,
        #[cfg(unix)] daemons: Vec<DaemonMatch>,
        #[cfg(unix)] group_gone: Arc<AtomicBool>,
        #[cfg(windows)] kill: tokio::sync::mpsc::UnboundedSender<()>,
    ) -> ControlState {
        ControlState {
            pid,
            #[cfg(unix)]
            pgid: pid,
            #[cfg(unix)]
            start,
            stdin: stdin.into(),
            exit,
            tail,
            log,
            rt,
            registry,
            #[cfg(unix)]
            daemons,
            #[cfg(unix)]
            escaped: Mutex::new(Vec::new()),
            #[cfg(unix)]
            group_gone,
            #[cfg(unix)]
            retain_registry: AtomicBool::new(false),
            #[cfg(windows)]
            kill,
            #[cfg(windows)]
            tree: None,
            chain: Mutex::new(None),
        }
    }

    pub(crate) fn exited_now(&self) -> Option<ExitInfo> {
        *self.exit.borrow()
    }

    /// Unix: whether the agent's group is known empty for good.
    #[cfg(unix)]
    pub(crate) fn group_gone(&self) -> bool {
        self.group_gone.load(Ordering::Acquire)
    }

    #[cfg(unix)]
    pub(crate) fn mark_group_gone(&self) {
        self.group_gone.store(true, Ordering::Release);
    }
}

impl Drop for ControlState {
    /// The last handle and any chain are gone, so is the process tree (the last handle's drop
    /// killed it): the registry no longer needs its entry. Registry writes block, so on the pool.
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.retain_registry.load(Ordering::Acquire) {
            return;
        }
        let registry = self.registry.clone();
        let pid = self.pid;
        drop(self.rt.spawn_blocking(move || registry.unregister(pid)));
    }
}

/// The control of a process [`Spawner`](crate::spawn::Spawner) started.
pub(crate) struct ChildControl {
    state: Arc<ControlState>,
}

impl ChildControl {
    pub(crate) fn new(state: ControlState) -> ChildControl {
        ChildControl {
            state: Arc::new(state),
        }
    }
}

impl ControlImpl for ChildControl {
    fn pid(&self) -> u32 {
        self.state.pid
    }

    fn close_stdin(&self) {
        self.state.stdin.close();
    }

    fn kill_tree(&self) {
        crate::shutdown::kill_tree(&self.state);
    }

    fn shutdown(&self, plan: ShutdownPlan) -> BoxFuture<'static, ShutdownReport> {
        let mut chain = lock(&self.state.chain);
        if let Some(running) = chain.as_ref() {
            return running.clone().boxed();
        }
        // A task, so the chain finishes even when every caller stops waiting for it.
        let task = self
            .state
            .rt
            .spawn(crate::shutdown::run(Arc::clone(&self.state), plan));
        let shared = async move { task.await.unwrap_or_default() }
            .boxed()
            .shared();
        *chain = Some(shared.clone());
        shared.boxed()
    }

    fn exited(&self) -> BoxFuture<'static, ExitInfo> {
        let mut exit = self.state.exit.clone();
        async move {
            match exit.wait_for(Option::is_some).await {
                Ok(found) => found.unwrap_or_else(ExitInfo::unknown),
                // The waiter is gone without a status (the runtime shut down).
                Err(_) => ExitInfo::unknown(),
            }
        }
        .boxed()
    }

    fn try_exited(&self) -> Option<ExitInfo> {
        self.state.exited_now()
    }

    fn retry_unconfirmed_shutdown(&self, plan: ShutdownPlan) -> BoxFuture<'static, ShutdownReport> {
        invalidate_unconfirmed(&mut lock(&self.state.chain));
        self.shutdown(plan)
    }

    fn record_escaped(&self) -> BoxFuture<'static, ()> {
        crate::shutdown::record_escaped(Arc::clone(&self.state))
    }

    fn stderr_tail(&self, max_bytes: usize) -> String {
        self.state.tail.tail(max_bytes)
    }

    #[cfg(windows)]
    fn job_members(&self) -> Option<Vec<u32>> {
        self.state.tree.as_ref().map(crate::tree::TreeJob::members)
    }
}

/// The native child beneath the wrappers, found by walking `inner_mut` and downcasting: waiting on
/// it is the direct child's exit, never a group-wide or job-wide wait.
pub(crate) fn native_child(child: &mut dyn ChildWrapper) -> Option<&mut tokio::process::Child> {
    if (&*child as &dyn std::any::Any).is::<tokio::process::Child>() {
        return (child as &mut dyn std::any::Any).downcast_mut::<tokio::process::Child>();
    }
    let here = std::ptr::from_ref::<dyn ChildWrapper>(child).cast::<()>();
    let next = child.inner_mut();
    if std::ptr::eq(
        here,
        std::ptr::from_ref::<dyn ChildWrapper>(next).cast::<()>(),
    ) {
        return None;
    }
    native_child(next)
}

/// What the Windows waiter is waiting for.
#[cfg(windows)]
enum Event {
    Exited(std::io::Result<std::process::ExitStatus>),
    /// A kill request; `false` once the control is gone.
    Kill(bool),
}

/// Waits for the direct child's exit and publishes it. On Windows it also serves kill requests,
/// before and after the exit, because terminating the job needs the child it owns; it keeps the job
/// until the control is gone, so a late kill still reaches lingering descendants. On Unix, right
/// after it reaps the agent (whose pid is its group's id), it notes whether the group emptied
/// (`group`: the id and the flag the kill paths read), before that id can belong to anyone else.
pub(crate) async fn wait_child(
    mut child: Box<dyn ChildWrapper>,
    exit: watch::Sender<Option<ExitInfo>>,
    log: ProtocolLog,
    #[cfg(unix)] group: (u32, Arc<AtomicBool>),
    #[cfg(windows)] mut kills: tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    #[cfg(not(windows))]
    let status = match native_child(child.as_mut()) {
        Some(native) => native.wait().await,
        None => child.wait().await,
    };
    #[cfg(unix)]
    crate::shutdown::note_reaped(group.0, &group.1);
    #[cfg(windows)]
    let mut kills_open = true;
    #[cfg(windows)]
    let status = loop {
        let event = match native_child(child.as_mut()) {
            Some(native) => tokio::select! {
                status = native.wait() => Event::Exited(status),
                kill = kills.recv(), if kills_open => Event::Kill(kill.is_some()),
            },
            None => tokio::select! {
                status = child.wait() => Event::Exited(status),
                kill = kills.recv(), if kills_open => Event::Kill(kill.is_some()),
            },
        };
        match event {
            Event::Exited(status) => break status,
            Event::Kill(open) => {
                kills_open = open;
                let _ = child.start_kill();
            }
        }
    };
    let info = status.map_or_else(|_| ExitInfo::unknown(), ExitInfo::from_status);
    log.note(LogNote::Exit {
        code: info.code,
        signal: info.signal,
    });
    exit.send_replace(Some(info));
    #[cfg(windows)]
    {
        while kills_open && kills.recv().await.is_some() {
            let _ = child.start_kill();
        }
        // The control is gone: whatever is left in the job goes with it.
        let _ = child.start_kill();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn retry_releases_only_completed_unconfirmed_chains() {
        let pending = futures::future::pending::<ShutdownReport>()
            .boxed()
            .shared();
        let mut chain = Some(pending);
        invalidate_unconfirmed(&mut chain);
        assert!(chain.is_some(), "an in-flight chain must still be shared");
        let failed = futures::future::ready(ShutdownReport::default())
            .boxed()
            .shared();
        failed.clone().await;
        chain = Some(failed);
        invalidate_unconfirmed(&mut chain);
        assert!(
            chain.is_none(),
            "unknown exit cannot be a permanent cached confirmation"
        );
        let confirmed = ShutdownReport {
            exit: Some(ExitInfo {
                code: Some(0),
                signal: None,
                at_ms: 0,
            }),
            ..Default::default()
        };
        let complete = futures::future::ready(confirmed.clone()).boxed().shared();
        complete.clone().await;
        chain = Some(complete);
        invalidate_unconfirmed(&mut chain);
        assert_eq!(chain.as_ref().and_then(Shared::peek), Some(&confirmed));
        let leftovers = ShutdownReport {
            leftovers: vec![123],
            ..confirmed
        };
        let incomplete = futures::future::ready(leftovers).boxed().shared();
        incomplete.clone().await;
        chain = Some(incomplete);
        invalidate_unconfirmed(&mut chain);
        assert!(
            chain.is_none(),
            "a leader exit does not prove descendant cleanup"
        );
    }
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[derive(Default)]
    struct Counting {
        kills: AtomicU32,
        closes: AtomicU32,
        plans: Mutex<Vec<ShutdownPlan>>,
    }

    impl ControlImpl for Counting {
        fn pid(&self) -> u32 {
            42
        }
        fn close_stdin(&self) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
        fn kill_tree(&self) {
            self.kills.fetch_add(1, Ordering::SeqCst);
        }
        fn shutdown(&self, plan: ShutdownPlan) -> BoxFuture<'static, ShutdownReport> {
            self.plans.lock().expect("lock").push(plan);
            async { ShutdownReport::default() }.boxed()
        }
        fn exited(&self) -> BoxFuture<'static, ExitInfo> {
            futures::future::pending().boxed()
        }
        fn try_exited(&self) -> Option<ExitInfo> {
            None
        }
        fn record_escaped(&self) -> BoxFuture<'static, ()> {
            async {}.boxed()
        }
        fn stderr_tail(&self, _max_bytes: usize) -> String {
            String::new()
        }
    }

    #[test]
    fn only_the_last_handle_kills_the_tree() {
        let imp = Arc::new(Counting::default());
        let control = ProcessControl::new(imp.clone());
        let clone = control.clone();
        assert_eq!(control.pid(), 42);
        drop(control);
        assert_eq!(
            imp.kills.load(Ordering::SeqCst),
            0,
            "a clone is still alive"
        );
        clone.close_stdin();
        assert_eq!(imp.closes.load(Ordering::SeqCst), 1);
        drop(clone);
        assert_eq!(imp.kills.load(Ordering::SeqCst), 1, "the last drop kills");
    }

    #[tokio::test]
    async fn methods_forward_to_the_implementation() {
        let imp = Arc::new(Counting::default());
        let control = ProcessControl::new(imp.clone());
        control.shutdown(ShutdownPlan::FORCE).await;
        control.kill_tree();
        assert_eq!(
            imp.plans.lock().expect("lock").as_slice(),
            [ShutdownPlan::FORCE]
        );
        assert_eq!(imp.kills.load(Ordering::SeqCst), 1);
        assert!(format!("{control:?}").contains("42"));
    }
}
