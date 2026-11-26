//! Async locks by key: work on one key runs in arrival order, work on other keys runs at once.
//!
//! Each key's lock exists only while someone holds it or waits for it: the map keeps weak handles,
//! and a key nobody uses is dropped the next time a new key is added.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Locks by key. Tokio's mutex is fair, so waiters on one key are let in first come, first served.
pub(crate) struct KeyedLock<K> {
    keys: Mutex<HashMap<K, Weak<AsyncMutex<()>>>>,
}

/// One key held; dropping it lets the next waiter in.
#[must_use = "the key is released as soon as the guard is dropped"]
pub(crate) struct KeyedGuard {
    _guard: OwnedMutexGuard<()>,
}

impl<K: Eq + Hash + Clone> KeyedLock<K> {
    pub(crate) fn new() -> Self {
        Self {
            keys: Mutex::new(HashMap::new()),
        }
    }

    fn handle(&self, key: K) -> Arc<AsyncMutex<()>> {
        let mut keys = self.keys.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(lock) = keys.get(&key).and_then(Weak::upgrade) {
            return lock;
        }
        keys.retain(|_, lock| lock.strong_count() > 0);
        let lock = Arc::new(AsyncMutex::new(()));
        keys.insert(key, Arc::downgrade(&lock));
        lock
    }

    /// Waits for `key`, behind everyone who asked for it first.
    pub(crate) async fn lock(&self, key: K) -> KeyedGuard {
        KeyedGuard {
            _guard: self.handle(key).lock_owned().await,
        }
    }

    /// Every key once, in sorted order, so two callers that share keys can never wait on each
    /// other in a cycle. Dropping the future part way releases what it took.
    pub(crate) async fn lock_all(&self, mut keys: Vec<K>) -> Vec<KeyedGuard>
    where
        K: Ord,
    {
        keys.sort();
        keys.dedup();
        let mut guards = Vec::with_capacity(keys.len());
        for key in keys {
            guards.push(self.lock(key).await);
        }
        guards
    }

    /// How many keys the map still tracks (tests).
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|lock| lock.strong_count() > 0)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn one_key_runs_in_arrival_order_and_other_keys_do_not_wait() {
        let locks = Arc::new(KeyedLock::new());
        let first = locks.lock("repo").await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut waiters = Vec::new();
        for name in ["second", "third"] {
            let locks = Arc::clone(&locks);
            let order = Arc::clone(&order);
            waiters.push(tokio::spawn(async move {
                let _held = locks.lock("repo").await;
                order.lock().unwrap().push(name);
            }));
            // Each waiter queues before the next one starts.
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Another key is free while "repo" is held.
        drop(
            tokio::time::timeout(Duration::from_secs(1), locks.lock("other"))
                .await
                .expect("another key does not wait"),
        );
        assert!(order.lock().unwrap().is_empty(), "nobody passed the holder");
        drop(first);
        for waiter in waiters {
            waiter.await.unwrap();
        }
        assert_eq!(*order.lock().unwrap(), ["second", "third"]);
    }

    #[tokio::test]
    async fn sorted_keys_never_deadlock_and_unused_keys_are_forgotten() {
        let locks = Arc::new(KeyedLock::new());
        let mut tasks = Vec::new();
        for round in 0..64 {
            let locks = Arc::clone(&locks);
            tasks.push(tokio::spawn(async move {
                let keys = if round % 2 == 0 {
                    vec!["b", "a", "c"]
                } else {
                    vec!["c", "b", "a", "a"]
                };
                let held = locks.lock_all(keys).await;
                assert_eq!(held.len(), 3);
                tokio::task::yield_now().await;
            }));
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            for task in tasks {
                task.await.unwrap();
            }
        })
        .await
        .expect("callers taking overlapping keys finish");
        assert_eq!(locks.tracked(), 0);
        let _held = locks.lock("d").await;
        assert_eq!(locks.tracked(), 1);
    }
}
