//! The session's spawner: what drivers spawn through. It mints each spawn id, opens the spawn's
//! protocol log, and records every process control, so force stop reaches them.
//!
//! The record is the chat's [`ProcessBook`], shared with the actor, its supervisor and the manager's
//! `kill_all_now`. Each driver start is a generation: a force stop closes its generation, so a
//! process a wedged driver spawns afterwards is killed at once instead of escaping the stop.

use std::sync::{Arc, Mutex, MutexGuard};

use futures::FutureExt;
use futures::future::BoxFuture;
use ghost_drivers::driver::AgentSpawner;
use ghost_process::{
    LogFactory, ProcessControl, ProcessSpawner, SpawnError, SpawnSpec, SpawnedProcess,
};
use ghost_protocol::tasks::{BackgroundTask, TaskStatus};

use crate::ids::new_id;

/// Tasks remembered per chat at most, for the Processes page; the oldest go first.
const MAX_TASKS: usize = 64;

/// Every process a chat's drivers spawned and that may still run, and the latest record of each
/// task its agent reported (subagents run inside the agent's own process, so only these show them).
#[derive(Debug, Default)]
pub(crate) struct ProcessBook {
    inner: Mutex<BookInner>,
}

#[derive(Debug, Default)]
struct BookInner {
    entries: Vec<(u64, ProcessControl)>,
    /// Generations that were force-stopped: what they spawn from now on is killed at once.
    closed: Vec<u64>,
    tasks: Vec<BackgroundTask>,
}

impl ProcessBook {
    fn lock(&self) -> MutexGuard<'_, BookInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records a process of generation `generation`. Returns false (and records nothing) when that
    /// generation was force-stopped; the caller kills it.
    fn record(&self, generation: u64, control: ProcessControl) -> bool {
        let mut inner = self.lock();
        inner
            .entries
            .retain(|(_, control)| control.try_exited().is_none());
        if inner.closed.contains(&generation) {
            return false;
        }
        inner.entries.push((generation, control));
        true
    }

    /// Closes `generation` and returns its processes, for the force-stop chain.
    pub(crate) fn close(&self, generation: u64) -> Vec<ProcessControl> {
        let mut inner = self.lock();
        if !inner.closed.contains(&generation) {
            inner.closed.push(generation);
        }
        inner
            .entries
            .iter()
            .filter(|(entry, _)| *entry == generation)
            .map(|(_, control)| control.clone())
            .collect()
    }

    /// Keeps the latest record of `task`. An update carries the whole record except a prompt or
    /// summary already known, which stay.
    pub(crate) fn note_task(&self, task: &BackgroundTask) {
        let mut inner = self.lock();
        if let Some(known) = inner
            .tasks
            .iter_mut()
            .find(|known| known.task_id == task.task_id)
        {
            let prompt = known.prompt.take();
            let summary = known.summary.take();
            *known = task.clone();
            known.prompt = known.prompt.take().or(prompt);
            known.summary = known.summary.take().or(summary);
        } else {
            inner.tasks.push(task.clone());
            if inner.tasks.len() > MAX_TASKS {
                inner.tasks.remove(0);
            }
        }
    }

    /// The tasks reported so far, in the order they were first heard of.
    pub(crate) fn tasks(&self) -> Vec<BackgroundTask> {
        self.lock().tasks.clone()
    }

    /// Forgets `generation`'s processes once its driver ended; dropping the last handle kills a
    /// tree that is still alive. A task still running ran inside those processes: it stopped too.
    pub(crate) fn clear(&self, generation: u64) {
        let removed: Vec<ProcessControl> = {
            let mut inner = self.lock();
            for task in &mut inner.tasks {
                if task.status == TaskStatus::Running {
                    task.status = TaskStatus::Stopped;
                }
            }
            let (gone, kept) = std::mem::take(&mut inner.entries)
                .into_iter()
                .partition(|(entry, _)| *entry == generation);
            inner.entries = kept;
            gone.into_iter().map(|(_, control)| control).collect()
        };
        drop(removed);
    }

    /// Every recorded process (`record_escaped`, the supervisor's cleanup).
    pub(crate) fn all(&self) -> Vec<ProcessControl> {
        self.lock()
            .entries
            .iter()
            .map(|(_, control)| control.clone())
            .collect()
    }

    /// `kill_tree()` on every recorded process; synchronous, callable from any thread.
    pub(crate) fn kill_all(&self) {
        for control in self.all() {
            control.kill_tree();
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lock().entries.is_empty()
    }
}

/// What one driver start spawns through.
pub(crate) struct SessionSpawner {
    base: Arc<dyn ProcessSpawner>,
    logs: LogFactory,
    chat_id: String,
    book: Arc<ProcessBook>,
    generation: u64,
}

impl SessionSpawner {
    pub(crate) fn new(
        base: Arc<dyn ProcessSpawner>,
        logs: LogFactory,
        chat_id: String,
        book: Arc<ProcessBook>,
        generation: u64,
    ) -> SessionSpawner {
        SessionSpawner {
            base,
            logs,
            chat_id,
            book,
            generation,
        }
    }
}

impl AgentSpawner for SessionSpawner {
    fn spawn(&self, spec: SpawnSpec) -> BoxFuture<'static, Result<SpawnedProcess, SpawnError>> {
        let spawn_id = new_id();
        let log = self.logs.open(&self.chat_id, &spawn_id);
        let spawned = self.base.spawn(spec, log);
        let book = Arc::clone(&self.book);
        let generation = self.generation;
        async move {
            let process = spawned.await?;
            if !book.record(generation, process.control.clone()) {
                process.control.kill_tree();
                return Err(SpawnError::Spawn {
                    program: process.spawn_id,
                    source: std::io::Error::other(
                        "the session was force-stopped while this process started",
                    ),
                });
            }
            Ok(process)
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use ghost_process::testing::{FakeCall, FakeSpawner};
    use ghost_process::{EnvMap, EnvSource, LaunchSpec, ProcessCwd, Redactor, ResolvedEnv};
    use tokio::runtime::Handle;

    use super::*;

    fn spec() -> SpawnSpec {
        SpawnSpec {
            launch: LaunchSpec {
                label: "fake".into(),
                program: PathBuf::from("/bin/fake"),
                args: Vec::new(),
                cwd: ProcessCwd::ChatFolder,
                set_env: Vec::new(),
                daemons: Vec::new(),
                startup_timeout: Duration::from_secs(5),
            },
            folder: PathBuf::from("/work"),
            env: Arc::new(ResolvedEnv {
                vars: EnvMap::new(),
                source: EnvSource::Fallback {
                    reason: "test".into(),
                },
                warnings: Vec::new(),
                resolved_at_ms: 0,
            }),
            max_line_bytes: 1024,
        }
    }

    fn task(id: &str, status: TaskStatus) -> BackgroundTask {
        BackgroundTask {
            task_id: id.into(),
            kind: ghost_protocol::tasks::TaskKind::Agent,
            title: format!("Task {id}"),
            agent_type: Some("Explore".into()),
            tool_call_id: None,
            parent_task_id: None,
            backgrounded: true,
            status,
            activity: None,
            tool_uses: None,
            tokens: None,
            duration_ms: None,
            started_at: None,
            ended_at: None,
            prompt: None,
            summary: None,
            summary_truncated: None,
        }
    }

    #[test]
    fn the_book_keeps_each_tasks_latest_record_and_known_text() {
        let book = ProcessBook::default();
        book.note_task(&BackgroundTask {
            prompt: Some("Map the code".into()),
            ..task("a", TaskStatus::Running)
        });
        book.note_task(&BackgroundTask {
            activity: Some("Reading lib.rs".into()),
            ..task("a", TaskStatus::Running)
        });
        book.note_task(&task("b", TaskStatus::Running));
        book.note_task(&BackgroundTask {
            summary: Some("Done".into()),
            ..task("b", TaskStatus::Completed)
        });
        let tasks = book.tasks();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].prompt.as_deref(), Some("Map the code"), "kept");
        assert_eq!(
            tasks[0].activity.as_deref(),
            Some("Reading lib.rs"),
            "updated"
        );
        assert_eq!(tasks[1].status, TaskStatus::Completed);
        assert_eq!(tasks[1].summary.as_deref(), Some("Done"));
        // The driver ended: what still ran inside its processes stopped with them.
        book.clear(1);
        let tasks = book.tasks();
        assert_eq!(tasks[0].status, TaskStatus::Stopped);
        assert_eq!(
            tasks[1].status,
            TaskStatus::Completed,
            "an ended task keeps its end"
        );
        for n in 0..MAX_TASKS + 5 {
            book.note_task(&task(&format!("t{n}"), TaskStatus::Running));
        }
        let tasks = book.tasks();
        assert_eq!(tasks.len(), MAX_TASKS);
        assert_eq!(tasks.last().unwrap().task_id, format!("t{}", MAX_TASKS + 4));
    }

    #[tokio::test]
    async fn spawns_are_recorded_and_a_closed_generation_kills_late_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (base, _ends) = FakeSpawner::new();
        let logs = LogFactory::new(
            dir.path().to_path_buf(),
            Arc::new(Redactor::standard()),
            Handle::current(),
        );
        let book = Arc::new(ProcessBook::default());
        let first = SessionSpawner::new(
            base.clone(),
            logs.clone(),
            "chat".into(),
            Arc::clone(&book),
            1,
        );
        let process = first.spawn(spec()).await.expect("spawn");
        assert_ne!(process.spawn_id, "");
        assert_eq!(book.all().len(), 1);

        let closed = book.close(1);
        assert_eq!(closed.len(), 1);
        let late = first.spawn(spec()).await;
        assert!(late.is_err(), "a force-stopped generation spawns nothing");
        let controls = base.controls();
        assert_eq!(
            controls[1].calls().first(),
            Some(&FakeCall::KillTree),
            "killed at once"
        );

        let second = SessionSpawner::new(base.clone(), logs, "chat".into(), Arc::clone(&book), 2);
        second.spawn(spec()).await.expect("a new generation spawns");
        book.clear(1);
        assert_eq!(book.all().len(), 1, "generation 2 stays recorded");
        book.kill_all();
        assert!(
            controls[0].calls().is_empty(),
            "generation 1's process is no longer recorded"
        );
        assert!(base.controls()[2].calls().contains(&FakeCall::KillTree));
    }
}
