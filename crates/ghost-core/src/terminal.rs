//! Ephemeral terminal ownership. No output, input or exit state is written to the chat store.
//! A caller reserves a page-owned slot before asynchronous preparation, so reload/close also
//! cancels a launch that has not produced a process yet.
use ghost_process::{
    ByteSink, SpawnSpec, Spawner,
    pty::{PtyControl, PtyOutput, PtySize},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, watch};

/// Sign-in and installation terminals: one per window, at most four in the app.
const MAX_TERMINALS: usize = 4;
/// Worktree setup commands: several per window, so two projects' new worktrees set up at once,
/// and at most four in the app, each a bounded, owned process.
pub const MAX_SETUP_TERMINALS: usize = 4;
/// Chats open in their agent's own interface: several per window, as chats are, each running its
/// agent until closed. Eight keeps the app's agent processes bounded (owner, 10 November 2026: a
/// second terminal chat was refused while the first ran).
pub const MAX_CHAT_TERMINALS: usize = 8;
/// Project shells in the docked panel are a separate pool, so a sign-in never waits on them.
/// Each one is an owned shell process; eight keeps the app's process count bounded.
pub const MAX_PROJECT_TERMINALS: usize = 8;
const MAX_INPUT: usize = 16 * 1024;
const READ_WAIT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("This terminal is closed or belongs to another window.")]
    Closed,
    #[error("A terminal is already open in this window.")]
    AlreadyOpen,
    #[error("Close another terminal before opening one.")]
    Full,
    #[error("Eight project terminals are open. Close one before opening another.")]
    ProjectFull,
    #[error("Eight chats are open in the terminal. Close one before opening another.")]
    ChatFull,
    #[error("Four setup commands are running. Wait for one to finish, then retry setup.")]
    SetupFull,
    #[error("This terminal was not reserved for a project shell.")]
    NotProject,
    #[error("This terminal was not reserved for a terminal chat.")]
    NotChat,
    #[error("A terminal read is already pending.")]
    ReadPending,
    #[error("Terminal input is limited to 16 KiB per write.")]
    InputTooLarge,
    #[error("The terminal cannot accept input yet. Try again after it catches up.")]
    InputBlocked,
    #[error("The terminal could not start: {0}")]
    Start(String),
    #[error("The terminal size is invalid or the terminal is closed.")]
    Resize,
    #[error(
        "The installation reached its ten-minute limit and was stopped. Review the output and refresh agents before trying again."
    )]
    TimedOut,
    #[error(
        "Terminal cleanup could not be confirmed. Its folders remain reserved until the application closes; inspect the owned-process recovery information before restarting work."
    )]
    CleanupUnconfirmed,
    #[error(
        "Setup reached its ten-minute limit and was stopped. Review the terminal output and worktree before preparing another command."
    )]
    SetupTimedOut,
}

/// One bounded pull. An empty nonterminal chunk means only that the read wait elapsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct TerminalChunk {
    pub bytes: Vec<u8>,
    pub done: bool,
    pub exit_code: Option<i32>,
    pub truncated: bool,
}

struct Running {
    input: ByteSink,
    output: AsyncMutex<PtyOutput>,
    control: PtyControl,
}
/// The pool a terminal counts against, fixed when it is reserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pool {
    /// Sign-in and installation: one per window, at most [`MAX_TERMINALS`].
    Vendor,
    /// A worktree's setup command: several per window, at most [`MAX_SETUP_TERMINALS`].
    Setup,
    /// A chat in its agent's own interface: several per window, at most [`MAX_CHAT_TERMINALS`].
    Chat,
    /// A project shell in the docked panel: several per window, at most [`MAX_PROJECT_TERMINALS`].
    Project,
}

struct Slot {
    pending_setup: Mutex<Option<PendingSetup>>,
    owner: String,
    claimed: AtomicBool,
    timed_out: AtomicBool,
    canceled: watch::Sender<bool>,
    running: Mutex<Option<Arc<Running>>>,
    cleanup_unconfirmed: AtomicBool,
    cleanup: watch::Sender<Option<bool>>,
    setup_timed_out: AtomicBool,
    pool: Pool,
}
impl Slot {
    fn closed_error(&self) -> TerminalError {
        if self.timed_out.load(Ordering::Acquire) {
            TerminalError::TimedOut
        } else {
            TerminalError::Closed
        }
    }
    fn cancel(&self) {
        self.canceled.send_replace(true);
        lock(&self.pending_setup).take();
        if let Some(running) = lock(&self.running).as_ref() {
            running.control.kill();
        }
    }
}

struct PendingSetup {
    chat_id: String,
    token: String,
    launch: crate::session::WorktreeSetupLaunch,
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Default)]
struct Inner {
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    stopped: AtomicBool,
    quiescing: AtomicBool,
    cleanup_guards: Mutex<Vec<Box<dyn Send + Sync>>>,
}

/// Blocks new terminal reservations without canceling any existing terminal.
pub struct IdleTerminalGuard<'a>(&'a Inner);

impl Drop for IdleTerminalGuard<'_> {
    fn drop(&mut self) {
        self.0.quiescing.store(false, Ordering::Release);
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        for slot in lock(&self.slots).values() {
            slot.cancel();
        }
    }
}

/// Clones share ownership. Sign-in and installation terminals: one per window, at most four in
/// this app instance. Worktree setup commands, terminal chats and project shells: any number per
/// window, at most [`MAX_SETUP_TERMINALS`], [`MAX_CHAT_TERMINALS`] and [`MAX_PROJECT_TERMINALS`]
/// in the app.
#[derive(Clone, Default)]
pub struct Terminals(Arc<Inner>);

pub struct TerminalTicket {
    terminals: Terminals,
    id: String,
    slot: Arc<Slot>,
    committed: bool,
}
impl Drop for TerminalTicket {
    fn drop(&mut self) {
        if !self.committed {
            lock(&self.terminals.0.slots).remove(&self.id);
            self.slot.cancel();
        }
    }
}
impl TerminalTicket {
    /// A prepared setup remains owned while reserve/start cross IPC. Closing the pending slot
    /// drops its Rust completion owner and holds the generation even if Start never arrives.
    pub fn pending_setup(
        self,
        chat_id: String,
        token: String,
        launch: crate::session::WorktreeSetupLaunch,
    ) -> Result<String, TerminalError> {
        {
            let mut pending = lock(&self.slot.pending_setup);
            if *self.slot.canceled.borrow() {
                return Err(TerminalError::Closed);
            }
            *pending = Some(PendingSetup {
                chat_id,
                token,
                launch,
            });
        }
        Ok(self.pending())
    }
    /// A setup launch owns the caller's folder reservations through process cleanup. Startup
    /// continues in an owned task if the IPC future disappears; its ticket still responds to Close.
    pub async fn start_guarded(
        self,
        spawner: Arc<Spawner>,
        spec: SpawnSpec,
        size: PtySize,
        guard: Box<dyn Send + Sync>,
    ) -> Result<String, TerminalError> {
        tokio::spawn(self.start_guarded_owned(
            spawner,
            spec,
            size,
            guard,
            Some(Duration::from_secs(600)),
            None,
        ))
        .await
        .map_err(|_| TerminalError::Start("Setup startup ended unexpectedly.".into()))?
    }

    /// Reports setup completion from the process owner after confirmed cleanup. Dropping this
    /// sender on any launch error leaves the persisted attempt failed, never successful.
    pub async fn start_setup(
        self,
        spawner: Arc<Spawner>,
        spec: SpawnSpec,
        size: PtySize,
        guard: Box<dyn Send + Sync>,
        completion: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    ) -> Result<String, TerminalError> {
        tokio::spawn(self.start_guarded_owned(
            spawner,
            spec,
            size,
            guard,
            Some(Duration::from_secs(600)),
            completion,
        ))
        .await
        .map_err(|_| TerminalError::Start("Setup startup ended unexpectedly.".into()))?
    }

    /// An explicitly opened project shell has no setup deadline. Its folder and profile guard
    /// remains owned until confirmed process cleanup, including after page or IPC cancellation.
    /// The slot must come from [`Terminals::reserve_project`], so the project pool stays bounded.
    pub async fn start_project(
        self,
        spawner: Arc<Spawner>,
        spec: SpawnSpec,
        size: PtySize,
        guard: Box<dyn Send + Sync>,
    ) -> Result<String, TerminalError> {
        if self.slot.pool != Pool::Project {
            return Err(TerminalError::NotProject);
        }
        tokio::spawn(self.start_guarded_owned(spawner, spec, size, guard, None, None))
            .await
            .map_err(|_| TerminalError::Start("Terminal startup ended unexpectedly.".into()))?
    }

    async fn start_guarded_owned(
        mut self,
        spawner: Arc<Spawner>,
        spec: SpawnSpec,
        size: PtySize,
        guard: Box<dyn Send + Sync>,
        limit: Option<Duration>,
        completion: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    ) -> Result<String, TerminalError> {
        if *self.slot.canceled.borrow() {
            return Err(TerminalError::Closed);
        }
        let process = tokio::time::timeout(Duration::from_secs(15), spawner.spawn_pty(spec, size))
            .await
            .map_err(|_| TerminalError::Start("Startup timed out.".into()))?
            .map_err(|error| TerminalError::Start(error.to_string()))?;
        let control = process.control.clone();
        let mut canceled = self.slot.canceled.subscribe();
        let slot = Arc::downgrade(&self.slot);
        let terminals = self.terminals.clone();
        self.slot.cleanup.send_replace(None);
        // This monitor is independent of output consumption, window lifetime and IPC cancellation.
        tokio::spawn(async move {
            let failure = tokio::select! {
                biased;
                _=canceled.wait_for(|value|*value)=>Some("Setup was canceled when its terminal closed. Retry setup.".to_owned()),
                _=control.exited()=>None,
                _=async {match limit {Some(limit)=>tokio::time::sleep(limit).await,None=>std::future::pending().await}}=>{
                    if let Some(slot)=slot.upgrade(){slot.setup_timed_out.store(true,Ordering::Release);}
                    control.kill();
                    Some("Setup timed out after ten minutes. Retry setup.".to_owned())
                },
            };
            let report = control.shutdown().await;
            let confirmed = report.exit.is_some() && report.leftovers.is_empty();
            if !confirmed {
                lock(&terminals.0.cleanup_guards).push(guard);
            } else {
                drop(guard);
            }
            if let Some(slot) = slot.upgrade() {
                slot.cleanup_unconfirmed
                    .store(!confirmed, Ordering::Release);
                slot.cleanup.send_replace(Some(confirmed));
            }
            if let Some(completion) = completion {
                let result = match (failure,confirmed,report.exit.and_then(|exit|exit.code)) {
                    (None,true,Some(0)) => Ok(()),
                    (Some(detail),_,_) => Err(detail),
                    (_,false,_) => Err("Setup process cleanup could not be confirmed. Review its terminal before retrying.".into()),
                    (_,_,Some(code)) => Err(format!("Setup exited with code {code}. Retry setup.")),
                    _ => Err("Setup ended without a successful exit status. Retry setup.".into()),
                };
                let _ = completion.send(result);
            }
        });
        let mut running = lock(&self.slot.running);
        if *self.slot.canceled.borrow() {
            process.control.kill();
            return Err(TerminalError::Closed);
        }
        *running = Some(Arc::new(Running {
            input: process.input,
            output: AsyncMutex::new(process.output),
            control: process.control,
        }));
        self.committed = true;
        Ok(self.id.clone())
    }

    /// Resolves once this terminal is closed (by its page, a reload or quit) before it started, so
    /// a launch still being prepared, a terminal chat waiting for its worktree's setup say, stops
    /// waiting then.
    pub async fn closed(&self) {
        let mut canceled = self.slot.canceled.subscribe();
        let _ = canceled.wait_for(|value| *value).await;
    }

    /// Return the reservation before beginning any vendor work. A close can now name and cancel
    /// it even if a later start handler has not begun yet.
    pub fn pending(mut self) -> String {
        self.slot.claimed.store(false, Ordering::Release);
        self.committed = true;
        self.id.clone()
    }
    pub async fn start(
        self,
        spawner: &Spawner,
        spec: SpawnSpec,
        size: PtySize,
    ) -> Result<String, TerminalError> {
        self.start_with_limit(spawner, spec, size, None).await
    }

    /// An installer is bounded even when the renderer stops polling. Login keeps its existing
    /// interactive lifetime; callers cannot choose an arbitrary program through this seam.
    pub async fn start_install(
        self,
        spawner: &Spawner,
        spec: SpawnSpec,
        size: PtySize,
    ) -> Result<String, TerminalError> {
        self.start_with_limit(spawner, spec, size, Some(Duration::from_secs(600)))
            .await
    }

    async fn start_with_limit(
        mut self,
        spawner: &Spawner,
        spec: SpawnSpec,
        size: PtySize,
        limit: Option<Duration>,
    ) -> Result<String, TerminalError> {
        if *self.slot.canceled.borrow() {
            return Err(TerminalError::Closed);
        }
        let process = tokio::time::timeout(Duration::from_secs(15), spawner.spawn_pty(spec, size))
            .await
            .map_err(|_| TerminalError::Start("Startup timed out.".into()))?
            .map_err(|error| TerminalError::Start(error.to_string()))?;
        let mut running = lock(&self.slot.running);
        if *self.slot.canceled.borrow() {
            return Err(TerminalError::Closed);
        }
        if let Some(limit) = limit {
            let slot = Arc::downgrade(&self.slot);
            let control = process.control.clone();
            tokio::spawn(async move {
                tokio::select! {
                    biased;
                    _ = control.exited() => {},
                    _ = tokio::time::sleep(limit) => {
                        if let Some(slot) = slot.upgrade() {
                            slot.timed_out.store(true, Ordering::Release);
                            slot.cancel();
                        }
                    }
                }
            });
        }
        *running = Some(Arc::new(Running {
            input: process.input,
            output: AsyncMutex::new(process.output),
            control: process.control,
        }));
        self.committed = true;
        Ok(self.id.clone())
    }
}

impl Terminals {
    /// Only succeeds with no reserved or running terminal. Uses the reservation lock so a new
    /// login cannot slip between the idle check and an application update.
    pub fn try_quiesce_when_idle(&self) -> Option<IdleTerminalGuard<'_>> {
        let slots = lock(&self.0.slots);
        if !slots.is_empty()
            || self.0.stopped.load(Ordering::Acquire)
            || self.0.quiescing.swap(true, Ordering::AcqRel)
        {
            return None;
        }
        Some(IdleTerminalGuard(&self.0))
    }

    /// A sign-in or installation terminal: one per window.
    pub fn reserve(&self, owner: &str) -> Result<TerminalTicket, TerminalError> {
        self.reserve_in(owner, Pool::Vendor)
    }

    /// A worktree's setup command: several per window, bounded across the app, so one project's
    /// setup never refuses another's, and a sign-in never waits on either.
    pub fn reserve_setup(&self, owner: &str) -> Result<TerminalTicket, TerminalError> {
        self.reserve_in(owner, Pool::Setup)
    }

    /// A chat in its agent's own interface: several per window, bounded across the app. Each
    /// keeps running while another chat is open, so one per window would refuse the second.
    pub fn reserve_chat(&self, owner: &str) -> Result<TerminalTicket, TerminalError> {
        self.reserve_in(owner, Pool::Chat)
    }

    /// A project shell for the docked panel: several per window, bounded across the app. Quit
    /// preflight keeps these until the user confirms, like every project shell.
    pub fn reserve_project(&self, owner: &str) -> Result<TerminalTicket, TerminalError> {
        self.reserve_in(owner, Pool::Project)
    }

    fn reserve_in(&self, owner: &str, pool: Pool) -> Result<TerminalTicket, TerminalError> {
        let mut slots = lock(&self.0.slots);
        if self.0.stopped.load(Ordering::Acquire) || self.0.quiescing.load(Ordering::Acquire) {
            return Err(TerminalError::Closed);
        }
        let members: Vec<&Arc<Slot>> = slots.values().filter(|slot| slot.pool == pool).collect();
        match pool {
            Pool::Vendor if members.iter().any(|slot| slot.owner == owner) => {
                return Err(TerminalError::AlreadyOpen);
            }
            Pool::Vendor if members.len() >= MAX_TERMINALS => return Err(TerminalError::Full),
            Pool::Chat if members.len() >= MAX_CHAT_TERMINALS => {
                return Err(TerminalError::ChatFull);
            }
            Pool::Setup if members.len() >= MAX_SETUP_TERMINALS => {
                return Err(TerminalError::SetupFull);
            }
            Pool::Project if members.len() >= MAX_PROJECT_TERMINALS => {
                return Err(TerminalError::ProjectFull);
            }
            _ => {}
        }
        drop(members);
        let id = uuid::Uuid::now_v7().to_string();
        let (canceled, _) = watch::channel(false);
        let (cleanup, _) = watch::channel(Some(true));
        let slot = Arc::new(Slot {
            pending_setup: Mutex::new(None),
            owner: owner.into(),
            claimed: AtomicBool::new(true),
            timed_out: AtomicBool::new(false),
            canceled,
            running: Mutex::new(None),
            cleanup_unconfirmed: AtomicBool::new(false),
            cleanup,
            setup_timed_out: AtomicBool::new(false),
            pool,
        });
        slots.insert(id.clone(), Arc::clone(&slot));
        Ok(TerminalTicket {
            terminals: self.clone(),
            id,
            slot,
            committed: false,
        })
    }

    /// Claims a terminal reserved with [`Terminals::reserve_chat`] for its chat's agent, so the
    /// chat pool, not the one-per-window sign-in pool, bounds terminal chats.
    pub fn claim_chat(&self, owner: &str, id: &str) -> Result<TerminalTicket, TerminalError> {
        if self.slot(owner, id)?.pool != Pool::Chat {
            return Err(TerminalError::NotChat);
        }
        self.claim(owner, id)
    }

    pub fn claim(&self, owner: &str, id: &str) -> Result<TerminalTicket, TerminalError> {
        let slot = self.slot(owner, id)?;
        if lock(&slot.pending_setup).is_some() {
            return Err(TerminalError::AlreadyOpen);
        }
        if slot.claimed.swap(true, Ordering::AcqRel) {
            return Err(TerminalError::AlreadyOpen);
        }
        Ok(TerminalTicket {
            terminals: self.clone(),
            id: id.into(),
            slot,
            committed: false,
        })
    }

    pub fn claim_setup(
        &self,
        owner: &str,
        id: &str,
        chat_id: &str,
        token: &str,
    ) -> Result<(TerminalTicket, crate::session::WorktreeSetupLaunch), TerminalError> {
        let slot = self.slot(owner, id)?;
        let launch = {
            let mut pending = lock(&slot.pending_setup);
            if *slot.canceled.borrow()
                || pending
                    .as_ref()
                    .is_none_or(|entry| entry.chat_id != chat_id || entry.token != token)
            {
                return Err(TerminalError::Closed);
            }
            if slot.claimed.swap(true, Ordering::AcqRel) {
                return Err(TerminalError::AlreadyOpen);
            }
            pending.take().ok_or(TerminalError::Closed)?.launch
        };
        Ok((
            TerminalTicket {
                terminals: self.clone(),
                id: id.into(),
                slot,
                committed: false,
            },
            launch,
        ))
    }

    fn slot(&self, owner: &str, id: &str) -> Result<Arc<Slot>, TerminalError> {
        lock(&self.0.slots)
            .get(id)
            .filter(|slot| slot.owner == owner)
            .cloned()
            .ok_or(TerminalError::Closed)
    }
    fn running(&self, owner: &str, id: &str) -> Result<(Arc<Slot>, Arc<Running>), TerminalError> {
        let slot = self.slot(owner, id)?;
        let running = lock(&slot.running).clone().ok_or(TerminalError::Closed)?;
        if *slot.canceled.borrow() {
            return Err(slot.closed_error());
        }
        Ok((slot, running))
    }
    pub async fn read(&self, owner: &str, id: &str) -> Result<TerminalChunk, TerminalError> {
        let (slot, running) = self.running(owner, id)?;
        if slot.cleanup_unconfirmed.load(Ordering::Acquire) {
            return Err(TerminalError::CleanupUnconfirmed);
        }
        if slot.setup_timed_out.load(Ordering::Acquire) {
            return Err(TerminalError::SetupTimedOut);
        }
        let mut output = running
            .output
            .try_lock()
            .map_err(|_| TerminalError::ReadPending)?;
        let mut canceled = slot.canceled.subscribe();
        let next = tokio::select! {
            biased;
            _ = canceled.wait_for(|closed| *closed) => return Err(slot.closed_error()),
            next = output.next() => Some(next),
            _ = tokio::time::sleep(READ_WAIT) => None,
        };
        let done = matches!(next, Some(None));
        if done {
            let mut cleanup = slot.cleanup.subscribe();
            tokio::select! {
                _=canceled.wait_for(|closed|*closed)=>return Err(TerminalError::Closed),
                result=cleanup.wait_for(|result|result.is_some())=>{
                    match result {Ok(value) if *value==Some(true)=>{},_=>return Err(TerminalError::CleanupUnconfirmed)}
                },
            }
        }
        Ok(TerminalChunk {
            bytes: next.flatten().unwrap_or_default(),
            done,
            exit_code: running.control.try_exited().and_then(|exit| exit.code),
            truncated: output.truncated(),
        })
    }
    pub fn write(&self, owner: &str, id: &str, bytes: Vec<u8>) -> Result<(), TerminalError> {
        if bytes.len() > MAX_INPUT {
            return Err(TerminalError::InputTooLarge);
        }
        let (_, running) = self.running(owner, id)?;
        running
            .input
            .try_write(bytes)
            .map_err(|_| TerminalError::InputBlocked)
    }
    pub fn resize(&self, owner: &str, id: &str, size: PtySize) -> Result<(), TerminalError> {
        let (_, running) = self.running(owner, id)?;
        running
            .control
            .resize(size)
            .map_err(|_| TerminalError::Resize)
    }
    pub fn close(&self, owner: &str, id: &str) -> Result<(), TerminalError> {
        let mut slots = lock(&self.0.slots);
        if let Some(slot) = slots.get(id) {
            if slot.owner != owner {
                return Err(TerminalError::Closed);
            }
            slot.cancel();
            slots.remove(id);
        }
        Ok(())
    }
    pub fn drop_owner(&self, owner: &str) {
        lock(&self.0.slots).retain(|_, slot| {
            if slot.owner == owner {
                slot.cancel();
                false
            } else {
                true
            }
        });
    }
    pub fn stop_all(&self) {
        self.0.stopped.store(true, Ordering::Release);
        self.close_all();
    }
    pub fn close_all(&self) {
        let mut slots = lock(&self.0.slots);
        for slot in slots.values() {
            slot.cancel();
        }
        slots.clear();
    }

    /// Preserve general shells until quit is confirmed. Existing bounded setup/vendor shutdown
    /// can finish before that question without leaving file mutations behind.
    pub fn close_non_project(&self) {
        self.close_where(|pool| pool != Pool::Project);
    }
    pub fn close_projects(&self) {
        self.close_where(|pool| pool == Pool::Project);
    }
    fn close_where(&self, close: impl Fn(Pool) -> bool) {
        lock(&self.0.slots).retain(|_, slot| {
            if close(slot.pool) {
                slot.cancel();
                false
            } else {
                true
            }
        });
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_process::{EnvSource, LaunchSpec, LeftoverRegistry, ProcessCwd, ResolvedEnv};
    use std::io::Write;

    #[tokio::test]
    async fn guarded_setup_holds_reservations_until_close_finishes_owned_cleanup() {
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let folder = tempfile::tempdir().unwrap();
        let terminals = Terminals::default();
        let released = Arc::new(AtomicBool::new(false));
        let ticket = terminals.reserve("setup").unwrap();
        let id = ticket
            .start_guarded(
                Spawner::new(
                    LeftoverRegistry::disabled(),
                    tokio::runtime::Handle::current(),
                ),
                local_spec(folder.path()),
                PtySize { rows: 24, cols: 80 },
                Box::new(Guard(released.clone())),
            )
            .await
            .unwrap();
        let (_, running) = terminals.running("setup", &id).unwrap();
        assert!(!released.load(Ordering::Acquire));
        terminals.close("setup", &id).unwrap();
        released_after_cleanup(&released, Some(&running)).await;
        assert!(running.control.try_exited().is_some());
        assert!(running.control.shutdown().await.leftovers.is_empty());
    }

    #[tokio::test]
    async fn project_shell_retains_its_guard_after_start_and_releases_only_after_page_cleanup() {
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let folder = tempfile::tempdir().unwrap();
        let terminals = Terminals::default();
        let released = Arc::new(AtomicBool::new(false));
        let id = terminals
            .reserve_project("main")
            .unwrap()
            .start_project(
                Spawner::new(
                    LeftoverRegistry::disabled(),
                    tokio::runtime::Handle::current(),
                ),
                local_spec(folder.path()),
                PtySize { rows: 24, cols: 80 },
                Box::new(Guard(released.clone())),
            )
            .await
            .unwrap();
        read_until(&terminals, &id, "local-terminal-ready").await;
        assert!(!released.load(Ordering::Acquire));
        let (_, running) = terminals.running("main", &id).unwrap();
        terminals.drop_owner("main");
        released_after_cleanup(&released, Some(&running)).await;
        assert!(running.control.try_exited().is_some());
        assert!(running.control.shutdown().await.leftovers.is_empty());
    }

    #[tokio::test]
    async fn guarded_setup_deadline_cleans_up_without_renderer_polling() {
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let folder = tempfile::tempdir().unwrap();
        let terminals = Terminals::default();
        let released = Arc::new(AtomicBool::new(false));
        let (completion, finished) = tokio::sync::oneshot::channel();
        let id = terminals
            .reserve("setup")
            .unwrap()
            .start_guarded_owned(
                Spawner::new(
                    LeftoverRegistry::disabled(),
                    tokio::runtime::Handle::current(),
                ),
                local_spec(folder.path()),
                PtySize { rows: 24, cols: 80 },
                Box::new(Guard(released.clone())),
                Some(Duration::from_millis(250)),
                Some(completion),
            )
            .await
            .unwrap();
        let running = terminals
            .running("setup", &id)
            .ok()
            .map(|(_, running)| running);
        released_after_cleanup(&released, running.as_ref()).await;
        assert!(finished.await.unwrap().unwrap_err().contains("timed out"));
        assert!(matches!(
            terminals.read("setup", &id).await,
            Err(TerminalError::SetupTimedOut)
        ));
        terminals.close("setup", &id).unwrap();
    }

    // A harmless test-harness subprocess. The ordinary unit test returns immediately; only the
    // explicit PTY test below supplies this marker and invokes this exact test name.
    #[test]
    fn updater_admission_never_cancels_a_terminal_or_reopens_permanent_stop() {
        let terminals = Terminals::default();
        let pending = terminals.reserve("main").unwrap();
        assert!(terminals.try_quiesce_when_idle().is_none());
        drop(pending);
        let barrier = terminals.try_quiesce_when_idle().unwrap();
        assert!(matches!(
            terminals.reserve("main"),
            Err(TerminalError::Closed)
        ));
        assert!(terminals.try_quiesce_when_idle().is_none());
        drop(barrier);
        drop(terminals.reserve("main").unwrap());
        let barrier = terminals.try_quiesce_when_idle().unwrap();
        terminals.stop_all();
        drop(barrier);
        assert!(matches!(
            terminals.reserve("main"),
            Err(TerminalError::Closed)
        ));
    }

    /// The product sets no deadline on owned cleanup. A force stop allows about four seconds and
    /// macOS reads its process table through `ps`, so a loaded runner gets a generous wait, and a
    /// timeout reports what shutdown saw instead of a bare elapsed error.
    async fn released_after_cleanup(released: &AtomicBool, running: Option<&Arc<Running>>) {
        let started = std::time::Instant::now();
        let waited = tokio::time::timeout(Duration::from_secs(30), async {
            while !released.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if waited.is_ok() {
            return;
        }
        let detail = match running {
            Some(running) => format!(
                "exit {:?}; shutdown now {:?}",
                running.control.try_exited(),
                running.control.shutdown().await
            ),
            None => "no running handle".into(),
        };
        panic!(
            "the guard was not released after {:?}: {detail}",
            started.elapsed()
        );
    }
    #[test]
    fn terminal_child() {
        if std::env::var_os("GHOST_TERMINAL_TEST_CHILD").is_none() {
            return;
        }
        println!("local-terminal-ready");
        std::io::stdout().flush().unwrap();
        if let Some(code) = std::env::var_os("GHOST_TERMINAL_TEST_EXIT") {
            std::process::exit(code.to_string_lossy().parse().unwrap());
        }
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        println!("local-terminal-input:{}", line.trim());
        std::io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }

    #[tokio::test]
    async fn setup_completion_comes_from_owned_exit_even_without_output_polling() {
        for code in [0, 3] {
            let folder = tempfile::tempdir().unwrap();
            let terminals = Terminals::default();
            let mut spec = local_spec(folder.path());
            spec.launch
                .set_env
                .push(("GHOST_TERMINAL_TEST_EXIT".into(), code.to_string().into()));
            let (completion, finished) = tokio::sync::oneshot::channel();
            let id = terminals
                .reserve("setup")
                .unwrap()
                .start_setup(
                    Spawner::new(
                        LeftoverRegistry::disabled(),
                        tokio::runtime::Handle::current(),
                    ),
                    spec,
                    PtySize { rows: 24, cols: 80 },
                    Box::new(()),
                    Some(completion),
                )
                .await
                .unwrap();
            let result = tokio::time::timeout(Duration::from_secs(30), finished)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.is_ok(), code == 0, "{result:?}");
            terminals.close("setup", &id).unwrap();
        }
    }

    #[tokio::test]
    async fn setup_close_and_launch_failure_cannot_report_success() {
        let folder = tempfile::tempdir().unwrap();
        let terminals = Terminals::default();
        let spawner = Spawner::new(
            LeftoverRegistry::disabled(),
            tokio::runtime::Handle::current(),
        );
        let (completion, finished) = tokio::sync::oneshot::channel();
        let id = terminals
            .reserve("setup")
            .unwrap()
            .start_setup(
                spawner.clone(),
                local_spec(folder.path()),
                PtySize { rows: 24, cols: 80 },
                Box::new(()),
                Some(completion),
            )
            .await
            .unwrap();
        terminals.close("setup", &id).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(30), finished)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
                .contains("canceled")
        );
        let (completion, finished) = tokio::sync::oneshot::channel();
        let mut missing = local_spec(folder.path());
        missing.launch.program = folder.path().join("missing-setup-executable");
        assert!(
            terminals
                .reserve("setup")
                .unwrap()
                .start_setup(
                    spawner,
                    missing,
                    PtySize { rows: 24, cols: 80 },
                    Box::new(()),
                    Some(completion)
                )
                .await
                .is_err()
        );
        assert!(
            finished.await.is_err(),
            "launch failure drops its completion sender"
        );
    }

    fn local_spec(folder: &std::path::Path) -> SpawnSpec {
        SpawnSpec {
            launch: LaunchSpec {
                label: "local terminal test".into(),
                program: std::env::current_exe().unwrap(),
                args: ["--exact", "terminal::tests::terminal_child", "--nocapture"]
                    .map(Into::into)
                    .into(),
                cwd: ProcessCwd::ChatFolder,
                set_env: vec![("GHOST_TERMINAL_TEST_CHILD".into(), "1".into())],
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(15),
            },
            folder: folder.into(),
            env: Arc::new(ResolvedEnv {
                vars: ghost_process::process_env(),
                source: EnvSource::Fallback {
                    reason: "test".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            max_line_bytes: ghost_process::MAX_LINE_BYTES,
        }
    }

    async fn read_until(terminals: &Terminals, id: &str, marker: &str) {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut output = Vec::new();
            loop {
                let chunk = terminals.read("main", id).await.unwrap();
                assert!(chunk.bytes.len() <= 8192);
                output.extend(chunk.bytes);
                assert!(output.len() < 100_000);
                if String::from_utf8_lossy(&output).contains(marker) {
                    break;
                }
                assert!(!chunk.done);
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn owned_local_terminal_rejects_other_windows_and_close_cancels_reads() {
        let folder = tempfile::tempdir().unwrap();
        let terminals = Terminals::default();
        let id = terminals.reserve("main").unwrap().pending();
        let spawner = Spawner::new(
            LeftoverRegistry::disabled(),
            tokio::runtime::Handle::current(),
        );
        terminals
            .claim("main", &id)
            .unwrap()
            .start(
                &spawner,
                local_spec(folder.path()),
                PtySize {
                    rows: 24,
                    cols: 100,
                },
            )
            .await
            .unwrap();
        read_until(&terminals, &id, "local-terminal-ready").await;
        assert!(matches!(
            terminals.read("other", &id).await,
            Err(TerminalError::Closed)
        ));
        assert!(matches!(
            terminals.write("other", &id, b"no\r".to_vec()),
            Err(TerminalError::Closed)
        ));
        terminals.write("main", &id, b"hello\r".to_vec()).unwrap();
        read_until(&terminals, &id, "local-terminal-input:hello").await;
        let (_, running) = terminals.running("main", &id).unwrap();
        let output = running.output.lock().await;
        assert!(matches!(
            terminals.read("main", &id).await,
            Err(TerminalError::ReadPending)
        ));
        drop(output);
        let pending = async {
            loop {
                if let Err(error) = terminals.read("main", &id).await {
                    return error;
                }
            }
        };
        tokio::pin!(pending);
        // Any final console rendering is harmless. When a read waits, closing wakes it without
        // waiting the read timeout, and reaches the owned process despite outstanding controls.
        assert!(matches!(
            futures::poll!(&mut pending),
            std::task::Poll::Pending
        ));
        terminals.drop_owner("main");
        let result = tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .unwrap();
        assert!(matches!(result, TerminalError::Closed));
        tokio::time::timeout(Duration::from_secs(5), running.control.exited())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn installation_deadline_stops_its_process_without_renderer_polling() {
        let folder = tempfile::tempdir().unwrap();
        let terminals = Terminals::default();
        let ticket = terminals.reserve("main").unwrap();
        let id = ticket.id.clone();
        let spawner = Spawner::new(
            LeftoverRegistry::disabled(),
            tokio::runtime::Handle::current(),
        );
        ticket
            .start_with_limit(
                &spawner,
                local_spec(folder.path()),
                PtySize { rows: 24, cols: 80 },
                Some(Duration::from_millis(250)),
            )
            .await
            .unwrap();
        let slot = terminals.slot("main", &id).unwrap();
        let running = lock(&slot.running).clone().unwrap();
        // No reads are necessary for the Rust-side deadline to own cancellation.
        tokio::time::timeout(Duration::from_secs(10), running.control.exited())
            .await
            .unwrap();
        assert!(slot.timed_out.load(Ordering::Acquire));
        assert!(matches!(
            terminals.read("main", &id).await,
            Err(TerminalError::TimedOut)
        ));
        terminals.close("main", &id).unwrap();
        assert!(terminals.reserve("main").is_ok());
    }

    #[test]
    fn reservations_bound_concurrency_and_cancellation_releases_the_slot() {
        let terminals = Terminals::default();
        let first = terminals.reserve("main").unwrap();
        assert!(matches!(
            terminals.reserve("main"),
            Err(TerminalError::AlreadyOpen)
        ));
        let other: Vec<_> = (0..3)
            .map(|i| terminals.reserve(&i.to_string()).unwrap())
            .collect();
        assert!(matches!(
            terminals.reserve("overflow"),
            Err(TerminalError::Full)
        ));
        drop(first);
        let next = terminals.reserve("main").unwrap();
        assert!(matches!(
            terminals.close("wrong-window", &next.id),
            Err(TerminalError::Closed)
        ));
        assert!(!*next.slot.canceled.borrow());
        terminals.drop_owner("main");
        assert!(*next.slot.canceled.borrow());
        assert!(other.iter().all(|ticket| !*ticket.slot.canceled.borrow()));
        assert!(terminals.reserve("main").is_ok());
    }

    #[test]
    fn quit_preflight_preserves_project_slots_until_explicit_close() {
        let terminals = Terminals::default();
        let project = terminals.reserve_project("project").unwrap();
        let second = terminals.reserve_project("project").unwrap();
        let vendor = terminals.reserve("vendor").unwrap();
        terminals.close_non_project();
        assert!(*vendor.slot.canceled.borrow());
        assert!(!*project.slot.canceled.borrow());
        assert!(!*second.slot.canceled.borrow());
        assert!(terminals.try_quiesce_when_idle().is_none());
        terminals.close_projects();
        assert!(*project.slot.canceled.borrow());
        assert!(*second.slot.canceled.borrow());
        assert!(terminals.try_quiesce_when_idle().is_some());
    }

    #[test]
    fn project_shells_share_one_bounded_pool_apart_from_vendor_terminals() {
        let terminals = Terminals::default();
        // Several shells in one window, and across windows, up to the app's cap.
        let mut shells: Vec<_> = (0..MAX_PROJECT_TERMINALS - 1)
            .map(|_| terminals.reserve_project("main").unwrap())
            .collect();
        shells.push(terminals.reserve_project("library").unwrap());
        assert!(matches!(
            terminals.reserve_project("main"),
            Err(TerminalError::ProjectFull)
        ));
        // A sign-in in the same window still opens, and stays one per window.
        let sign_in = terminals.reserve("main").unwrap();
        assert!(matches!(
            terminals.reserve("main"),
            Err(TerminalError::AlreadyOpen)
        ));
        // Closing one shell frees exactly one place, and only for project shells.
        let closed = shells.pop().unwrap().pending();
        terminals.close("library", &closed).unwrap();
        let replacement = terminals.reserve_project("main").unwrap();
        assert!(matches!(
            terminals.reserve_project("main"),
            Err(TerminalError::ProjectFull)
        ));
        // A reload drops every terminal the window owns, in both pools.
        terminals.drop_owner("main");
        assert!(shells.iter().all(|ticket| *ticket.slot.canceled.borrow()));
        assert!(*replacement.slot.canceled.borrow());
        assert!(*sign_in.slot.canceled.borrow());
        assert!(terminals.try_quiesce_when_idle().is_some());
    }

    #[test]
    fn setup_terminals_have_their_own_pool() {
        let terminals = Terminals::default();
        // Two projects' worktree setups in one window run at once...
        let first = terminals.reserve_setup("main").unwrap();
        let second = terminals.reserve_setup("main").unwrap();
        // ...while a sign-in in that window still opens, one per window as before.
        let sign_in = terminals.reserve("main").unwrap();
        assert!(matches!(
            terminals.reserve("main"),
            Err(TerminalError::AlreadyOpen)
        ));
        // The setup pool is bounded across the app and says so.
        let third = terminals.reserve_setup("library").unwrap();
        let fourth = terminals.reserve_setup("library").unwrap();
        assert!(matches!(
            terminals.reserve_setup("main"),
            Err(TerminalError::SetupFull)
        ));
        // Project shells never count against it.
        let shell = terminals.reserve_project("main").unwrap();
        let freed = fourth.pending();
        terminals.close("library", &freed).unwrap();
        let again = terminals.reserve_setup("main").unwrap();
        for ticket in [&first, &second, &third, &again, &sign_in, &shell] {
            assert!(!*ticket.slot.canceled.borrow());
        }
        terminals.drop_owner("main");
        terminals.drop_owner("library");
        assert!(terminals.try_quiesce_when_idle().is_some());
    }

    #[test]
    fn terminal_chats_open_several_per_window_in_their_own_bounded_pool() {
        let terminals = Terminals::default();
        // A chat left running in the terminal no longer refuses the next one in its window.
        let mut chats: Vec<_> = (0..MAX_CHAT_TERMINALS - 1)
            .map(|_| terminals.reserve_chat("main").unwrap())
            .collect();
        chats.push(terminals.reserve_chat("library").unwrap());
        assert!(matches!(
            terminals.reserve_chat("main"),
            Err(TerminalError::ChatFull)
        ));
        // Sign-in keeps its own place, one per window, and project shells theirs.
        let sign_in = terminals.reserve("main").unwrap();
        assert!(matches!(
            terminals.reserve("main"),
            Err(TerminalError::AlreadyOpen)
        ));
        let shell = terminals.reserve_project("main").unwrap();
        // Quit preflight closes terminal chats with the sign-ins and keeps project shells.
        terminals.close_non_project();
        assert!(chats.iter().all(|ticket| *ticket.slot.canceled.borrow()));
        assert!(*sign_in.slot.canceled.borrow());
        assert!(!*shell.slot.canceled.borrow());
    }

    #[test]
    fn only_a_chat_reservation_starts_a_terminal_chat() {
        let terminals = Terminals::default();
        let sign_in = terminals.reserve("main").unwrap().pending();
        assert!(matches!(
            terminals.claim_chat("main", &sign_in),
            Err(TerminalError::NotChat)
        ));
        let chat = terminals.reserve_chat("main").unwrap().pending();
        assert!(matches!(
            terminals.claim_chat("library", &chat),
            Err(TerminalError::Closed)
        ));
        let claimed = terminals.claim_chat("main", &chat).unwrap();
        assert!(matches!(
            terminals.claim_chat("main", &chat),
            Err(TerminalError::AlreadyOpen)
        ));
        // A claim that never starts gives the chat's place back.
        drop(claimed);
        assert!(matches!(
            terminals.claim_chat("main", &chat),
            Err(TerminalError::Closed)
        ));
    }

    #[tokio::test]
    async fn a_vendor_reservation_cannot_start_a_project_shell() {
        let folder = tempfile::tempdir().unwrap();
        let terminals = Terminals::default();
        let released = Arc::new(AtomicBool::new(false));
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let result = terminals
            .reserve("main")
            .unwrap()
            .start_project(
                Spawner::new(
                    LeftoverRegistry::disabled(),
                    tokio::runtime::Handle::current(),
                ),
                local_spec(folder.path()),
                PtySize { rows: 24, cols: 80 },
                Box::new(Guard(released.clone())),
            )
            .await;
        assert!(matches!(result, Err(TerminalError::NotProject)));
        // Nothing launched, the folder guard is released and the refused slot is gone.
        assert!(released.load(Ordering::Acquire));
        assert!(terminals.try_quiesce_when_idle().is_some());
    }

    #[tokio::test]
    async fn several_project_shells_in_one_window_clean_up_one_by_one() {
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let folder = tempfile::tempdir().unwrap();
        let terminals = Terminals::default();
        let spawner = Spawner::new(
            LeftoverRegistry::disabled(),
            tokio::runtime::Handle::current(),
        );
        let mut shells = Vec::new();
        for _ in 0..2 {
            let released = Arc::new(AtomicBool::new(false));
            let id = terminals
                .reserve_project("main")
                .unwrap()
                .start_project(
                    spawner.clone(),
                    local_spec(folder.path()),
                    PtySize { rows: 24, cols: 80 },
                    Box::new(Guard(released.clone())),
                )
                .await
                .unwrap();
            read_until(&terminals, &id, "local-terminal-ready").await;
            shells.push((id, released));
        }
        let (first, first_released) = &shells[0];
        let (second, second_released) = &shells[1];
        // Input reaches only the shell it names.
        terminals.write("main", second, b"two\r".to_vec()).unwrap();
        read_until(&terminals, second, "local-terminal-input:two").await;
        let (_, first_running) = terminals.running("main", first).unwrap();
        let (_, second_running) = terminals.running("main", second).unwrap();
        terminals.close("main", first).unwrap();
        released_after_cleanup(first_released, Some(&first_running)).await;
        assert!(!second_released.load(Ordering::Acquire));
        assert!(terminals.running("main", second).is_ok());
        // Confirmed quit closes the rest and still waits for owned cleanup.
        terminals.close_projects();
        released_after_cleanup(second_released, Some(&second_running)).await;
        assert!(first_running.control.shutdown().await.leftovers.is_empty());
        assert!(second_running.control.shutdown().await.leftovers.is_empty());
    }

    #[test]
    fn stop_cancels_pending_launches_and_refuses_new_work() {
        let terminals = Terminals::default();
        let ticket = terminals.reserve("main").unwrap();
        terminals.stop_all();
        assert!(*ticket.slot.canceled.borrow());
        assert!(matches!(
            terminals.reserve("main"),
            Err(TerminalError::Closed)
        ));
        assert!(matches!(
            terminals.write("main", &ticket.id, vec![0; MAX_INPUT + 1]),
            Err(TerminalError::InputTooLarge)
        ));
    }

    #[test]
    fn close_before_start_cannot_launch_and_duplicate_claim_cannot_replace_it() {
        let terminals = Terminals::default();
        let id = terminals.reserve("main").unwrap().pending();
        assert!(matches!(
            terminals.claim("other", &id),
            Err(TerminalError::Closed)
        ));
        let ticket = terminals.claim("main", &id).unwrap();
        assert!(matches!(
            terminals.claim("main", &id),
            Err(TerminalError::AlreadyOpen)
        ));
        terminals.close("main", &id).unwrap();
        assert!(*ticket.slot.canceled.borrow());
        assert!(matches!(
            terminals.claim("main", &id),
            Err(TerminalError::Closed)
        ));
    }
}
