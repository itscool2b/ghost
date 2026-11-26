//! One piece of work shared by everyone waiting for it: a probe, or an adapter install. It runs as a
//! task, so it makes progress however its waiters are polled, and it is cancelled when the last of
//! them stops waiting before it finished, which is how an Esc during a start kills a running
//! install or probe (its helper dies with the dropped future).

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::AbortHandle;

pub(crate) struct SharedTask<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Clone for SharedTask<T> {
    fn clone(&self) -> SharedTask<T> {
        SharedTask {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> std::fmt::Debug for SharedTask<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedTask")
            .field("waiting", &*lock(&self.inner.waiting))
            .field("finished", &self.inner.abort.is_finished())
            .finish()
    }
}

struct Inner<T> {
    result: watch::Receiver<Option<T>>,
    abort: AbortHandle,
    /// Callers waiting (or holding) right now.
    waiting: Mutex<usize>,
}

impl<T: Clone + Send + Sync + 'static> SharedTask<T> {
    pub(crate) fn spawn(
        rt: &Handle,
        work: impl Future<Output = T> + Send + 'static,
    ) -> SharedTask<T> {
        let (sender, result) = watch::channel(None);
        let task = rt.spawn(async move {
            let value = work.await;
            sender.send_replace(Some(value));
        });
        SharedTask {
            inner: Arc::new(Inner {
                result,
                abort: task.abort_handle(),
                waiting: Mutex::new(0),
            }),
        }
    }

    /// Whether it can still give a result: it has one, or it is running.
    pub(crate) fn is_live(&self) -> bool {
        self.inner.result.borrow().is_some() || !self.inner.abort.is_finished()
    }

    /// Whether it is still working (no result yet, not cancelled).
    pub(crate) fn is_running(&self) -> bool {
        self.inner.result.borrow().is_none() && !self.inner.abort.is_finished()
    }

    /// Cancels superseded work, including every helper owned by its future.
    pub(crate) fn cancel(&self) {
        self.inner.abort.abort();
    }

    /// Counts as a waiter until the returned guard drops, without waiting yet.
    pub(crate) fn hold(&self) -> Hold<T> {
        *lock(&self.inner.waiting) += 1;
        Hold {
            inner: Arc::clone(&self.inner),
        }
    }

    /// The result; `None` if the work was cancelled before it finished.
    pub(crate) async fn wait(&self) -> Option<T> {
        let _hold = self.hold();
        let mut result = self.inner.result.clone();
        match result.wait_for(Option::is_some).await {
            Ok(value) => value.clone(),
            Err(_) => None,
        }
    }
}

/// A waiter's place; the last one dropped before the result cancels the work.
pub(crate) struct Hold<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Drop for Hold<T> {
    fn drop(&mut self) {
        let mut waiting = lock(&self.inner.waiting);
        *waiting = waiting.saturating_sub(1);
        if *waiting == 0 && self.inner.result.borrow().is_none() {
            self.inner.abort.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn every_waiter_gets_the_one_result() {
        let runs = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&runs);
        let task = SharedTask::spawn(&Handle::current(), async move {
            counted.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            7
        });
        let other = task.clone();
        let (a, b) = tokio::join!(task.wait(), other.wait());
        assert_eq!((a, b), (Some(7), Some(7)));
        assert_eq!(task.wait().await, Some(7), "a late waiter gets it at once");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(task.is_live());
    }

    /// Sets a flag when dropped: the work's future was cancelled.
    struct Dropped(Arc<AtomicBool>);

    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn the_last_waiter_leaving_cancels_the_work() {
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = Dropped(Arc::clone(&dropped));
        let task: SharedTask<u32> = SharedTask::spawn(&Handle::current(), async move {
            let _flag = flag;
            std::future::pending::<()>().await;
            1
        });
        let first = tokio::spawn({
            let task = task.clone();
            async move { task.wait().await }
        });
        let second = tokio::spawn({
            let task = task.clone();
            async move { task.wait().await }
        });
        tokio::task::yield_now().await;
        first.abort();
        let _ = first.await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!dropped.load(Ordering::SeqCst), "one waiter is still there");
        second.abort();
        let _ = second.await;
        for _ in 0..100 {
            if dropped.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            dropped.load(Ordering::SeqCst),
            "cancelled with its last waiter"
        );
        assert!(!task.is_live());
        assert_eq!(task.wait().await, None);
    }

    #[tokio::test]
    async fn a_hold_keeps_the_work_running_without_waiting() {
        let task = SharedTask::spawn(&Handle::current(), async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            "done"
        });
        let hold = task.hold();
        let waiter = tokio::spawn({
            let task = task.clone();
            async move { task.wait().await }
        });
        tokio::task::yield_now().await;
        waiter.abort();
        let _ = waiter.await;
        assert_eq!(task.wait().await, Some("done"), "the hold kept it alive");
        drop(hold);
        assert!(task.is_live(), "finished work stays readable");
    }
}
