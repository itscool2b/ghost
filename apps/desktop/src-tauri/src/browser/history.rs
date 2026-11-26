//! The browser's history: every page opened in it, with its title, how often and when last. The
//! address bar searches it, and Settings > Browser lists, searches and removes it. It is kept in
//! `<profile>/browser/history.json`.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::address;

/// At most this many pages; the least recently visited go first.
pub const LIMIT: usize = 5000;

/// One page in history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visits: u32,
    /// The last visit, in milliseconds since the Unix epoch.
    pub last_visit: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    version: u32,
    entries: Vec<HistoryEntry>,
}

#[derive(Debug, Default)]
pub struct History {
    entries: HashMap<String, HistoryEntry>,
    dirty: bool,
}

impl History {
    /// Reads `path`; a missing or unreadable file is an empty history (never an error).
    pub fn load(path: &Path) -> History {
        let entries = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<File>(&bytes).ok())
            .map(|file| file.entries)
            .unwrap_or_default();
        let mut history = History::default();
        for entry in entries {
            if address::remembered(&entry.url) && entry.last_visit.is_finite() {
                history.entries.insert(entry.url.clone(), entry);
            }
        }
        history.trim();
        history.dirty = false;
        history
    }

    /// The file's bytes when something changed since the last save.
    pub fn take_changes(&mut self) -> Option<Vec<u8>> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        let mut entries: Vec<HistoryEntry> = self.entries.values().cloned().collect();
        entries.sort_by(|a, b| b.last_visit.total_cmp(&a.last_visit));
        serde_json::to_vec(&File {
            version: 1,
            entries,
        })
        .ok()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// A page opened (a finished load); `title` may still be empty.
    pub fn visit(&mut self, url: &str, title: &str, at: f64) {
        if !address::remembered(url) {
            return;
        }
        let entry = self
            .entries
            .entry(url.to_owned())
            .or_insert_with(|| HistoryEntry {
                url: url.to_owned(),
                title: String::new(),
                visits: 0,
                last_visit: at,
            });
        entry.visits = entry.visits.saturating_add(1);
        entry.last_visit = at;
        if !title.trim().is_empty() {
            entry.title = clip(title);
        }
        self.dirty = true;
        self.trim();
    }

    /// The page at `url` got its title after it loaded.
    pub fn retitle(&mut self, url: &str, title: &str) {
        if title.trim().is_empty() {
            return;
        }
        if let Some(entry) = self.entries.get_mut(url)
            && entry.title != title
        {
            entry.title = clip(title);
            self.dirty = true;
        }
    }

    pub fn remove(&mut self, urls: &[String]) -> usize {
        let before = self.entries.len();
        for url in urls {
            self.entries.remove(url);
        }
        let removed = before - self.entries.len();
        if removed > 0 {
            self.dirty = true;
        }
        removed
    }

    pub fn clear(&mut self) {
        if !self.entries.is_empty() {
            self.entries.clear();
            self.dirty = true;
        }
    }

    /// History newest first, those matching every word of `query` (in the title or address), at
    /// most `limit`, visited before `before` when given (paging).
    pub fn list(&self, query: &str, limit: usize, before: Option<f64>) -> Vec<HistoryEntry> {
        let words = words(query);
        let mut found: Vec<&HistoryEntry> = self
            .entries
            .values()
            .filter(|entry| before.is_none_or(|before| entry.last_visit < before))
            .filter(|entry| matches_all(entry, &words))
            .collect();
        found.sort_by(|a, b| b.last_visit.total_cmp(&a.last_visit));
        found.into_iter().take(limit).cloned().collect()
    }

    /// What the address bar offers for `input`: matching pages, best first. A page whose site
    /// starts with what was typed leads, then often and recently visited ones.
    pub fn suggest(&self, input: &str, limit: usize, now: f64) -> Vec<HistoryEntry> {
        let typed = input.trim().to_lowercase();
        let words = words(&typed);
        if words.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(f64, &HistoryEntry)> = self
            .entries
            .values()
            .filter(|entry| matches_all(entry, &words))
            .map(|entry| (score(entry, &typed, now), entry))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, entry)| entry.clone())
            .collect()
    }

    fn trim(&mut self) {
        if self.entries.len() <= LIMIT {
            return;
        }
        let mut by_age: Vec<(f64, String)> = self
            .entries
            .values()
            .map(|entry| (entry.last_visit, entry.url.clone()))
            .collect();
        by_age.sort_by(|a, b| a.0.total_cmp(&b.0));
        let excess = self.entries.len() - LIMIT;
        for (_, url) in by_age.into_iter().take(excess) {
            self.entries.remove(&url);
        }
        self.dirty = true;
    }
}

fn clip(title: &str) -> String {
    let title = title.trim();
    if title.chars().count() > 300 {
        title.chars().take(300).collect()
    } else {
        title.to_owned()
    }
}

fn words(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|word| !word.is_empty())
        .take(12)
        .collect()
}

fn matches_all(entry: &HistoryEntry, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let url = entry.url.to_lowercase();
    let title = entry.title.to_lowercase();
    words
        .iter()
        .all(|word| url.contains(word.as_str()) || title.contains(word.as_str()))
}

const DAY: f64 = 86_400_000.0;

fn score(entry: &HistoryEntry, typed: &str, now: f64) -> f64 {
    let site = address::site(&entry.url).to_lowercase();
    let bare = entry
        .url
        .to_lowercase()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .to_owned();
    let mut score = f64::from(entry.visits.min(100)) * 4.0;
    if bare.starts_with(typed) {
        score += 400.0;
    } else if site.starts_with(typed) {
        score += 300.0;
    } else if entry
        .title
        .to_lowercase()
        .split_whitespace()
        .any(|word| word.starts_with(typed))
    {
        score += 120.0;
    }
    let age = (now - entry.last_visit).max(0.0);
    score += if age < DAY {
        100.0
    } else if age < 7.0 * DAY {
        50.0
    } else if age < 30.0 * DAY {
        20.0
    } else {
        0.0
    };
    // A site's home before deep pages of it, unless those are visited far more.
    let home = tauri::Url::parse(&entry.url)
        .is_ok_and(|url| url.path() == "/" && url.query().is_none() && url.fragment().is_none());
    if home {
        score += 60.0;
    }
    score - (entry.url.len() as f64 / 40.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_800_000_000_000.0;

    #[test]
    fn visits_count_and_titles_follow() {
        let mut history = History::default();
        history.visit("https://example.com/", "", NOW - 10.0);
        history.retitle("https://example.com/", "Example Domain");
        history.visit("https://example.com/", "", NOW);
        let list = history.list("", 10, None);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].visits, 2);
        assert_eq!(list[0].title, "Example Domain");
        assert_eq!(list[0].last_visit, NOW);
    }

    #[test]
    fn only_pages_and_files_are_kept() {
        let mut history = History::default();
        for url in [
            "about:blank",
            "data:text/html,x",
            "javascript:alert(1)",
            "mailto:a@b.c",
        ] {
            history.visit(url, "x", NOW);
        }
        assert_eq!(history.len(), 0);
        history.visit("file:///C:/site/index.html", "Site", NOW);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn the_list_searches_every_word_newest_first_and_pages() {
        let mut history = History::default();
        history.visit(
            "https://github.com/itscool2b/ghost",
            "Ghost repo",
            NOW - 3.0,
        );
        history.visit("http://localhost:5173/", "Vite App", NOW - 2.0);
        history.visit("https://docs.rs/wry", "wry - Rust", NOW - 1.0);
        let all: Vec<String> = history
            .list("", 10, None)
            .into_iter()
            .map(|e| e.url)
            .collect();
        assert_eq!(
            all,
            [
                "https://docs.rs/wry",
                "http://localhost:5173/",
                "https://github.com/itscool2b/ghost"
            ]
        );
        assert_eq!(history.list("rust WRY", 10, None).len(), 1);
        assert_eq!(history.list("github repo", 10, None).len(), 1);
        assert_eq!(history.list("github vite", 10, None).len(), 0);
        let older = history.list("", 10, Some(NOW - 1.5));
        assert_eq!(older.len(), 2);
        assert_eq!(older[0].url, "http://localhost:5173/");
    }

    #[test]
    fn suggestions_put_the_typed_site_first() {
        let mut history = History::default();
        for _ in 0..20 {
            history.visit(
                "https://news.example.com/localhost-story",
                "Story",
                NOW - 40.0 * DAY,
            );
        }
        history.visit("http://localhost:3000/", "Dev server", NOW - 2.0 * DAY);
        history.visit("http://localhost:3000/settings/profile", "Settings", NOW);
        let first = history.suggest("local", 5, NOW);
        assert_eq!(first[0].url, "http://localhost:3000/");
        assert_eq!(first.len(), 3);
        assert!(history.suggest("   ", 5, NOW).is_empty());
    }

    #[test]
    fn the_oldest_go_past_the_limit() {
        let mut history = History::default();
        for index in 0..(LIMIT + 5) {
            history.visit(&format!("https://e.com/{index}"), "", index as f64);
        }
        assert_eq!(history.len(), LIMIT);
        assert!(
            history.list("e.com/0", 1, None).is_empty(),
            "the oldest went"
        );
        assert_eq!(
            history.list("", 1, None)[0].url,
            format!("https://e.com/{}", LIMIT + 4)
        );
    }

    #[test]
    fn it_round_trips_and_survives_a_bad_file() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("history.json");
        let mut history = History::default();
        history.visit("https://example.com/", "Example", NOW);
        std::fs::write(&path, history.take_changes().expect("changes")).expect("write");
        assert!(history.take_changes().is_none(), "nothing new to save");
        let loaded = History::load(&path);
        assert_eq!(loaded.list("", 5, None), history.list("", 5, None));
        std::fs::write(&path, b"{not json").expect("write");
        assert_eq!(History::load(&path).len(), 0);
        assert_eq!(History::load(&folder.path().join("missing.json")).len(), 0);
    }

    #[test]
    fn removing_and_clearing() {
        let mut history = History::default();
        history.visit("https://a.com/", "A", NOW);
        history.visit("https://b.com/", "B", NOW);
        assert_eq!(
            history.remove(&["https://a.com/".into(), "https://zzz/".into()]),
            1
        );
        history.clear();
        assert_eq!(history.len(), 0);
    }
}
