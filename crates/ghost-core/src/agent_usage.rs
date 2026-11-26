//! What each agent account last reported about its usage limits (docs/agent-accounts.md).
//!
//! Agents report their account's windows while a chat runs on it: Claude Code's session, weekly
//! and per-model limits, Codex's primary and secondary ones. The book keeps the last report per
//! agent and account, so every account's usage can be seen, including those no chat runs on now.
//! A local profile keeps the book in `agent-usage.json` across restarts; it holds percentages and
//! reset times only, never a token or an email. An account profile, whose files are all
//! encrypted, keeps it in memory.
use crate::fsutil::{JsonFile, read_json, write_json_atomic};
use ghost_protocol::{
    LimitWindow, Millis,
    agents::{AccountUsage, AgentUsage},
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

const VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Book {
    version: u32,
    #[serde(default)]
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    agent_id: String,
    account_id: Option<String>,
    windows: Vec<LimitWindow>,
    observed_at: Millis,
}

#[derive(Debug, Default)]
pub struct UsageBook {
    state: Mutex<(Option<PathBuf>, Vec<Entry>)>,
}

impl UsageBook {
    /// Keeps the book in `path` from now on, starting from what it holds. A damaged or newer
    /// file is left alone and the book starts empty.
    pub fn persist_at(&self, path: &Path) {
        let entries = match read_json::<Book>(path) {
            JsonFile::Valid(book) if book.version == VERSION => book.entries,
            _ => Vec::new(),
        };
        let mut state = self.lock();
        let mut merged = entries;
        for entry in std::mem::take(&mut state.1) {
            upsert(&mut merged, entry);
        }
        *state = (Some(path.to_path_buf()), merged);
    }

    /// Records the windows an agent's account reported. An empty report keeps the last one.
    pub fn record(
        &self,
        agent_id: &str,
        account_id: Option<&str>,
        windows: &[LimitWindow],
        at: Millis,
    ) {
        if windows.is_empty() {
            return;
        }
        let mut state = self.lock();
        upsert(
            &mut state.1,
            Entry {
                agent_id: agent_id.to_owned(),
                account_id: account_id.map(str::to_owned),
                windows: windows.to_vec(),
                observed_at: at,
            },
        );
        save(&state);
    }

    /// Forgets a removed account's usage.
    pub fn forget(&self, agent_id: &str, account_id: &str) {
        let mut state = self.lock();
        let before = state.1.len();
        state.1.retain(|entry| {
            entry.agent_id != agent_id || entry.account_id.as_deref() != Some(account_id)
        });
        if state.1.len() != before {
            save(&state);
        }
    }

    /// The agent's accounts and what each last reported, the newest report first.
    pub fn usage(&self, agent_id: &str) -> AgentUsage {
        let state = self.lock();
        let mut accounts: Vec<AccountUsage> = state
            .1
            .iter()
            .filter(|entry| entry.agent_id == agent_id)
            .map(|entry| AccountUsage {
                account_id: entry.account_id.clone(),
                windows: entry.windows.clone(),
                observed_at: entry.observed_at,
            })
            .collect();
        accounts.sort_by_key(|account| std::cmp::Reverse(account.observed_at));
        AgentUsage {
            agent_id: agent_id.to_owned(),
            accounts,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (Option<PathBuf>, Vec<Entry>)> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn upsert(entries: &mut Vec<Entry>, entry: Entry) {
    match entries
        .iter_mut()
        .find(|old| old.agent_id == entry.agent_id && old.account_id == entry.account_id)
    {
        Some(old) if old.observed_at <= entry.observed_at => *old = entry,
        Some(_) => {}
        None => entries.push(entry),
    }
}

/// Best effort: the book is a convenience, so a write that fails leaves it in memory only.
fn save(state: &(Option<PathBuf>, Vec<Entry>)) {
    if let Some(path) = &state.0 {
        let book = Book {
            version: VERSION,
            entries: state.1.clone(),
        };
        if let Err(error) = write_json_atomic(path, &book) {
            tracing::warn!(%error, "could not save agent usage");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(label: &str, used: f64) -> LimitWindow {
        LimitWindow {
            label: label.into(),
            used_percent: used,
            resets_at: Some(1_000),
        }
    }

    #[test]
    fn keeps_each_accounts_last_report_newest_first_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-usage.json");
        let book = UsageBook::default();
        book.persist_at(&path);
        book.record("claude", None, &[window("Session", 40.0)], 10);
        book.record("claude", Some("a1"), &[window("Session", 5.0)], 20);
        book.record("claude", None, &[window("Session", 60.0)], 30);
        book.record("claude", None, &[], 40);
        book.record("codex", None, &[window("5h", 1.0)], 50);
        let usage = book.usage("claude");
        assert_eq!(usage.accounts.len(), 2);
        assert_eq!(usage.accounts[0].account_id, None);
        assert_eq!(usage.accounts[0].windows[0].used_percent, 60.0);
        assert_eq!(usage.accounts[0].observed_at, 30);
        assert_eq!(usage.accounts[1].account_id.as_deref(), Some("a1"));

        let again = UsageBook::default();
        again.persist_at(&path);
        assert_eq!(again.usage("claude"), usage);
        assert_eq!(again.usage("codex").accounts.len(), 1);
        again.forget("claude", "a1");
        assert_eq!(again.usage("claude").accounts.len(), 1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("a1"),
            "a removed account's usage is gone from the file"
        );
    }

    #[test]
    fn an_older_report_never_replaces_a_newer_one_and_a_damaged_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-usage.json");
        std::fs::write(&path, "{ not json").unwrap();
        let book = UsageBook::default();
        book.record("claude", None, &[window("Week", 10.0)], 100);
        book.persist_at(&path);
        book.record("claude", None, &[window("Week", 99.0)], 50);
        assert_eq!(
            book.usage("claude").accounts[0].windows[0].used_percent,
            10.0
        );
    }
}
