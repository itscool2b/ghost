//! Reads one scope's instruction files and works out what every agent loads from them.

use std::path::{Path, PathBuf};

use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use sha2::{Digest, Sha256};

use super::{
    AgentInfo, AgentStatus, Context, FileInfo, FileKind, GitState, Managed, Mode, NestedFile,
    ScopeKind, Snapshot,
    agents::{self, AGENTS, AgentRule, Limit, Pick, RootId},
    apply::Expect,
    imports, manifest, text,
};
use crate::{Error, Result, safe_fs};

/// Names worth listing when they appear in subfolders or parent folders.
const NESTED_NAMES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "AGENTS.override.md",
    "GEMINI.md",
    "CLAUDE.local.md",
    "HERMES.md",
    ".hermes.md",
];
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".turbo",
    ".cache",
];
const NESTED_DEPTH: usize = 6;
const NESTED_DIRS: usize = 4000;
const NESTED_LIMIT: usize = 50;
const LISTING_LIMIT: usize = 50_000;

pub(crate) struct Root {
    pub id: RootId,
    pub path: PathBuf,
    pub display: String,
}

pub(crate) struct Found {
    pub root: usize,
    pub path: &'static str,
    pub key: String,
    pub display: String,
    /// The relative path on disk when its letter case differs.
    pub disk: Option<String>,
    pub kind: FileKind,
    pub bytes: Option<Vec<u8>>,
    pub text: Option<String>,
    pub encoding: Option<text::Encoding>,
    pub size: u64,
    pub read_only: bool,
    pub link_raw: Option<PathBuf>,
    /// The file a link or hard link leads to.
    pub link_to: Option<usize>,
    pub git: Option<GitState>,
}

impl Found {
    pub fn exists(&self) -> bool {
        !matches!(self.kind, FileKind::Missing | FileKind::Dangling)
    }

    /// The path as it exists on disk, for reads and writes.
    pub fn disk_path(&self) -> &str {
        self.disk.as_deref().unwrap_or(self.path)
    }

    /// What the transaction must find there to apply.
    pub fn expect(&self) -> Expect {
        match self.kind {
            FileKind::Missing => Expect::Missing,
            FileKind::Link | FileKind::Outside | FileKind::Dangling if self.link_raw.is_some() => {
                Expect::Link(self.link_raw.clone().unwrap_or_default())
            }
            _ => Expect::File(self.bytes.clone().unwrap_or_default()),
        }
    }

    fn blank(&self) -> bool {
        self.size == 0
            || self
                .text
                .as_deref()
                .is_some_and(|text| text.trim().is_empty())
    }
}

pub(crate) struct Model<'a> {
    pub context: &'a Context,
    pub roots: Vec<Root>,
    pub files: Vec<Found>,
    pub identity: String,
    pub record: Option<manifest::Entry>,
    /// A CLAUDE file in a parent folder, which makes Claude Code skip AGENTS.md.
    pub parent_claude: Option<String>,
    pub parents: Vec<NestedFile>,
    pub nested: Vec<NestedFile>,
    pub notices: Vec<String>,
}

/// What one agent loads at this scope.
pub(crate) struct Loaded {
    pub files: Vec<usize>,
}

impl<'a> Model<'a> {
    pub fn scan(context: &'a Context) -> Result<Self> {
        let project = context.scope == ScopeKind::Project;
        let mut roots = Vec::new();
        if project {
            let path = context.project.clone().ok_or(Error::Unavailable)?;
            if !path.is_absolute() {
                return Err(Error::UnsafePath);
            }
            roots.push(Root {
                id: RootId::Project,
                display: path.to_string_lossy().into_owned(),
                path,
            });
        } else {
            for root in &context.user_roots {
                let id = RootId::parse(&root.id).ok_or(Error::UnsafePath)?;
                if id == RootId::Project || !root.path.is_absolute() {
                    return Err(Error::UnsafePath);
                }
                roots.push(Root {
                    id,
                    path: root.path.clone(),
                    display: root.display.clone(),
                });
            }
        }
        let mut files = Vec::new();
        let mut listings = Listings::default();
        for (root_id, path) in agents::known(project) {
            let Some(root) = roots.iter().position(|root| root.id == root_id) else {
                continue;
            };
            files.push(scan_file(&roots[root], root, path, context, &mut listings));
        }
        link_files(&roots, &mut files);
        let identity = if project {
            std::fs::canonicalize(&roots[0].path)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| roots[0].display.clone())
        } else {
            "user".into()
        };
        let record = manifest::Manifest::load(&context.state_dir)
            .roots
            .remove(&identity);
        let mut model = Self {
            context,
            roots,
            files,
            identity,
            record,
            parent_claude: None,
            parents: Vec::new(),
            nested: Vec::new(),
            notices: Vec::new(),
        };
        if project {
            model.scan_parents();
            model.scan_nested();
        }
        Ok(model)
    }

    pub fn file(&self, key: &str) -> Option<usize> {
        self.files.iter().position(|file| file.key == key)
    }

    pub fn by_path(&self, root: RootId, path: &str) -> Option<usize> {
        self.files
            .iter()
            .position(|file| self.roots[file.root].id == root && file.path == path)
    }

    pub fn project(&self) -> bool {
        self.context.scope == ScopeKind::Project
    }

    pub fn reads(&self, agent: &AgentRule) -> Option<agents::Reads> {
        if self.project() {
            Some(agent.project)
        } else {
            agent.user
        }
    }

    pub fn installed(&self, agent: &AgentRule) -> bool {
        self.context.installed.iter().any(|id| id == agent.id)
    }

    /// The text a file contributes, following a link inside the folder.
    pub fn content(&self, index: usize) -> Option<&str> {
        let file = &self.files[index];
        match (file.kind, file.link_to) {
            (FileKind::Link, Some(target)) => self.files[target].text.as_deref(),
            _ => file.text.as_deref(),
        }
    }

    /// The file this one only imports, resolved inside the scope.
    pub fn import_of(&self, index: usize) -> Option<usize> {
        let file = &self.files[index];
        let text = file.text.as_deref()?;
        if file.kind != FileKind::File || !imports::only_imports(text) {
            return None;
        }
        let found = imports::find(text);
        let [import] = found.as_slice() else {
            return None;
        };
        if self.project() {
            let resolved = imports::resolve(file.path, import)?;
            self.files.iter().position(|other| {
                other.root == file.root && other.path.eq_ignore_ascii_case(&resolved)
            })
        } else {
            let home = self.context.home.as_deref()?;
            let target = match import.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => PathBuf::from(import),
            };
            self.files.iter().position(|other| {
                same_text_path(&self.roots[other.root].path.join(other.path), &target)
            })
        }
    }

    pub fn loaded(&self, agent: &AgentRule) -> Option<Loaded> {
        let reads = self.reads(agent)?;
        let indices = reads
            .files
            .iter()
            .filter_map(|(root, path)| self.by_path(*root, path))
            .collect::<Vec<_>>();
        let usable = |index: &usize| {
            let file = &self.files[*index];
            file.exists() && !(agent.skips_ignored && file.git == Some(GitState::Ignored))
        };
        let files = match reads.pick {
            Pick::First => indices.into_iter().find(usable).into_iter().collect(),
            Pick::FirstNonEmpty => indices
                .into_iter()
                .find(|index| usable(index) && !self.files[*index].blank())
                .into_iter()
                .collect(),
            Pick::All => indices.into_iter().filter(usable).collect(),
            Pick::ClaudeCode => {
                let claude = indices
                    .iter()
                    .copied()
                    .filter(|index| self.files[*index].path != "AGENTS.md")
                    .filter(usable)
                    .collect::<Vec<_>>();
                if !claude.is_empty() || self.parent_claude.is_some() {
                    claude
                } else {
                    indices
                        .into_iter()
                        .filter(|index| self.files[*index].path == "AGENTS.md")
                        .filter(usable)
                        .collect()
                }
            }
        };
        Some(Loaded { files })
    }

    /// The text an agent ends up with from one loaded file: imports expanded for agents that
    /// read them, a bare pointer contributing nothing for agents that do not.
    pub fn contribution(&self, agent: &AgentRule, index: usize) -> Option<(usize, String)> {
        if let Some(target) = self.import_of(index) {
            if agent.imports && (self.project() || agent.id == "claude") {
                return self
                    .content(target)
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| (target, normalize(text)));
            }
            return None;
        }
        self.content(index)
            .filter(|text| !text.trim().is_empty())
            .map(|text| (index, normalize(text)))
    }

    /// Every agent's loaded text as one comparable string, or `None` when it reads nothing.
    pub fn effective(&self, agent: &AgentRule) -> Option<String> {
        let loaded = self.loaded(agent)?;
        // The same text loaded twice is a warning, not a different set of instructions.
        let mut parts: Vec<String> = Vec::new();
        for text in loaded
            .files
            .iter()
            .filter_map(|index| self.contribution(agent, *index).map(|(_, text)| text))
        {
            if !parts.contains(&text) {
                parts.push(text);
            }
        }
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    pub fn source_choices(&self) -> Vec<usize> {
        if self.project() {
            return self
                .by_path(RootId::Project, "AGENTS.md")
                .into_iter()
                .collect();
        }
        let order = [
            "codex",
            "claude",
            "pi",
            "opencode",
            "omp",
            "antigravity",
            "grok",
        ];
        order
            .iter()
            .filter_map(|id| agents::rule(id))
            .filter_map(|agent| agent.user)
            .filter_map(|reads| self.by_path(reads.own.0, reads.own.1))
            .collect()
    }

    /// The source a switch to Universal uses: the recorded one, else the first choice with text
    /// (installed agents' files first), else the first installed agent's file.
    pub fn default_source(&self) -> Option<usize> {
        let choices = self.source_choices();
        if let Some(record) = &self.record
            && let Some(index) = self.file(&record.source)
            && choices.contains(&index)
        {
            return Some(index);
        }
        let installed_file = |index: &usize| {
            AGENTS.iter().any(|agent| {
                self.installed(agent)
                    && self
                        .reads(agent)
                        .is_some_and(|reads| self.by_path(reads.own.0, reads.own.1) == Some(*index))
            })
        };
        choices
            .iter()
            .copied()
            .find(|index| installed_file(index) && self.has_text(*index))
            .or_else(|| choices.iter().copied().find(|index| self.has_text(*index)))
            .or_else(|| choices.iter().copied().find(installed_file))
            .or_else(|| choices.first().copied())
    }

    pub fn has_text(&self, index: usize) -> bool {
        self.content(index)
            .is_some_and(|text| !text.trim().is_empty())
    }

    /// The universal source this scope already follows, if any.
    pub fn universal_source(&self) -> Option<usize> {
        if let Some(record) = &self.record
            && let Some(index) = self.file(&record.source)
            && self.files[index].exists()
        {
            return Some(index);
        }
        let mut texts = Vec::new();
        for agent in AGENTS {
            if let Some(text) = self.effective(agent)
                && !texts.contains(&text)
            {
                texts.push(text);
            }
        }
        let [shared] = texts.as_slice() else {
            return None;
        };
        // Only a file that may hold the universal text counts: a project whose agents all read
        // one CLAUDE.md is still per agent until AGENTS.md holds the text.
        let preferred = self.default_source();
        let candidates = preferred.into_iter().chain(self.source_choices());
        candidates.into_iter().find(|index| {
            self.files[*index].kind == FileKind::File
                && self
                    .content(*index)
                    .is_some_and(|text| normalize(text) == *shared)
        })
    }

    pub fn mode(&self) -> (Mode, Option<usize>) {
        if let Some(source) = self.universal_source() {
            return (Mode::Universal, Some(source));
        }
        let reads_any = AGENTS.iter().any(|agent| self.effective(agent).is_some());
        (
            if reads_any {
                Mode::PerAgent
            } else {
                Mode::None
            },
            None,
        )
    }

    pub fn managed(&self, index: usize) -> Option<Managed> {
        let record = self.record.as_ref()?;
        let file = &self.files[index];
        let written = record.copies.get(&file.key)?;
        let source = self.file(&record.source)?;
        let current = file.bytes.as_deref().map(safe_fs::hash);
        let source_now = self.files[source].bytes.as_deref().map(safe_fs::hash);
        let in_sync = current.is_some() && current == source_now;
        Some(Managed {
            source: record.source.clone(),
            edited: !in_sync && current.as_ref() != Some(written),
            source_changed: !in_sync && source_now.as_ref() != Some(&record.source_hash),
        })
    }

    /// The file already holds these bytes, or the same text.
    pub fn copy_of_bytes(&self, index: usize, bytes: &[u8]) -> bool {
        let file = &self.files[index];
        file.bytes.as_deref() == Some(bytes)
            || text::decode(bytes)
                .zip(self.content(index))
                .is_some_and(|((text, _), current)| normalize(&text) == normalize(current))
    }

    /// A file holding exactly the source's text without being a link or import.
    pub fn copy_of(&self, index: usize, source: usize) -> bool {
        index != source
            && self.files[index].kind == FileKind::File
            && self.files[source].kind == FileKind::File
            && self.has_text(index)
            && self.content(index).map(normalize) == self.content(source).map(normalize)
    }

    fn status(
        &self,
        agent: &AgentRule,
        source: Option<usize>,
    ) -> (AgentStatus, Option<usize>, Option<usize>, Vec<usize>) {
        let Some(loaded) = self.loaded(agent) else {
            return (AgentStatus::Unsupported, None, None, Vec::new());
        };
        let contributing = loaded
            .files
            .iter()
            .filter_map(|index| {
                self.contribution(agent, *index)
                    .map(|(from, _)| (*index, from))
            })
            .collect::<Vec<_>>();
        let Some(&(first, _)) = contributing.first() else {
            return (
                AgentStatus::NotSet,
                loaded.files.first().copied(),
                None,
                Vec::new(),
            );
        };
        let main = source
            .and_then(|source| {
                contributing
                    .iter()
                    .find(|(_, from)| *from == source || self.files[*from].link_to == Some(source))
                    .map(|(index, _)| *index)
            })
            .unwrap_or(first);
        let also = contributing
            .iter()
            .map(|(index, _)| *index)
            .filter(|index| *index != main)
            .collect::<Vec<_>>();
        let Some(source) = source else {
            // Per agent: an override still says that it hides the shared file.
            let hidden = self.default_source().is_some_and(|shared| {
                main != shared
                    && (agents::is_override(self.files[main].path)
                        || self.shadows(agent, main, shared))
            });
            let status = if hidden {
                AgentStatus::Override
            } else {
                AgentStatus::Own
            };
            return (status, Some(main), None, also);
        };
        let file = &self.files[main];
        let status = if main == source {
            AgentStatus::Direct
        } else if self.import_of(main) == Some(source) {
            AgentStatus::Import
        } else if matches!(file.kind, FileKind::Link | FileKind::HardLink)
            && file.link_to == Some(source)
        {
            AgentStatus::Link
        } else if let Some(managed) = self.managed(main)
            && managed.source == self.files[source].key
        {
            if managed.edited || managed.source_changed {
                AgentStatus::Drift
            } else {
                AgentStatus::Copy
            }
        } else if self.copy_of(main, source) {
            AgentStatus::Copy
        } else if agents::is_override(file.path) || self.shadows(agent, main, source) {
            AgentStatus::Override
        } else {
            AgentStatus::Own
        };
        let via = matches!(
            status,
            AgentStatus::Import | AgentStatus::Link | AgentStatus::Copy | AgentStatus::Drift
        )
        .then_some(source);
        (status, Some(main), via, also)
    }

    /// True when `file` comes before `source` in this agent's own precedence list.
    pub fn shadows(&self, agent: &AgentRule, file: usize, source: usize) -> bool {
        let Some(reads) = self.reads(agent) else {
            return false;
        };
        if !matches!(reads.pick, Pick::First | Pick::FirstNonEmpty) {
            return false;
        }
        let position = |index: usize| {
            let found = &self.files[index];
            reads
                .files
                .iter()
                .position(|(root, path)| self.roots[found.root].id == *root && found.path == *path)
        };
        matches!((position(file), position(source)), (Some(a), Some(b)) if a < b)
    }

    /// Files with distinct text a switch to Universal would have to reconcile, source first.
    pub fn conflict(&self, source: usize) -> Vec<usize> {
        let mut seen: Vec<String> = Vec::new();
        let mut keys = Vec::new();
        let mut consider = |index: usize, keys: &mut Vec<usize>| {
            let file = &self.files[index];
            if file.kind != FileKind::File || self.import_of(index).is_some() {
                return;
            }
            let Some(text) = self.content(index).filter(|text| !text.trim().is_empty()) else {
                return;
            };
            let text = normalize(text);
            if !seen.contains(&text) {
                seen.push(text);
                keys.push(index);
            }
        };
        consider(source, &mut keys);
        for agent in AGENTS {
            let Some(loaded) = self.loaded(agent) else {
                continue;
            };
            for index in loaded.files {
                if self.files[index].path == "CLAUDE.local.md" {
                    continue;
                }
                if index != source && self.files[index].link_to != Some(source) {
                    consider(index, &mut keys);
                }
            }
            if let Some(reads) = self.reads(agent)
                && let Some(own) = self.by_path(reads.own.0, reads.own.1)
            {
                consider(own, &mut keys);
            }
        }
        if keys.len() < 2 {
            return Vec::new();
        }
        keys
    }

    fn warnings(&self, agent: &AgentRule, loaded: &[usize]) -> Vec<String> {
        let mut warnings = Vec::new();
        let sizes = loaded
            .iter()
            .map(|index| {
                let file = match (self.files[*index].kind, self.files[*index].link_to) {
                    (FileKind::Link, Some(target)) => &self.files[target],
                    _ => &self.files[*index],
                };
                let expanded = self
                    .import_of(*index)
                    .filter(|_| agent.imports)
                    .map(|target| &self.files[target]);
                let file = expanded.unwrap_or(file);
                (
                    file.display.clone(),
                    file.size as usize,
                    file.text.as_deref().map_or(0, |text| text.chars().count()),
                    file.text.as_deref().map_or(0, text::lines),
                )
            })
            .collect::<Vec<_>>();
        for limit in agent.limits {
            match *limit {
                Limit::CombinedBytes(default) => {
                    let limit = if agent.id == "codex" {
                        self.context.codex_limit.unwrap_or(default)
                    } else {
                        default
                    };
                    let total = sizes.iter().map(|(_, bytes, _, _)| bytes).sum::<usize>();
                    if total > limit {
                        warnings.push(format!(
                            "{} reads the first {}. This text is {}, so the end is cut.",
                            agent.name,
                            kib(limit),
                            kib(total)
                        ));
                    }
                }
                Limit::FileBytes(limit) => {
                    for (path, bytes, _, _) in &sizes {
                        if *bytes > limit {
                            warnings.push(format!(
                                "{} cuts each file at {} bytes. {path} is {} bytes.",
                                agent.name,
                                group(limit),
                                group(*bytes)
                            ));
                        }
                    }
                }
                Limit::FileChars(limit) => {
                    for (path, _, chars, _) in &sizes {
                        if *chars > limit {
                            warnings.push(format!(
                                "{} may cut {path} on smaller models, keeping its start and end. It has {} characters; the floor is {}.",
                                agent.name,
                                group(*chars),
                                group(limit)
                            ));
                        }
                    }
                }
                Limit::SkipOver(limit) => {
                    for (path, bytes, _, _) in &sizes {
                        if *bytes > limit {
                            warnings.push(format!(
                                "{} skips {path} entirely: it is over {}.",
                                agent.name,
                                kib(limit)
                            ));
                        }
                    }
                }
            }
        }
        if let Some(recommended) = agent.recommended_lines {
            for (path, _, _, lines) in &sizes {
                if *lines > recommended {
                    warnings.push(format!(
                        "{} suggests under {recommended} lines. {path} has {lines}.",
                        agent.name
                    ));
                }
            }
        }
        if agent.skips_ignored
            && let Some(reads) = self.reads(agent)
        {
            for (root, path) in reads.files {
                if let Some(index) = self.by_path(*root, path)
                    && self.files[index].exists()
                    && self.files[index].git == Some(GitState::Ignored)
                {
                    warnings.push(format!(
                        "{} skips {path} because Git ignores it.",
                        agent.name
                    ));
                }
            }
        }
        let texts = loaded
            .iter()
            .filter_map(|index| self.contribution(agent, *index).map(|(_, text)| text))
            .collect::<Vec<_>>();
        if texts
            .iter()
            .enumerate()
            .any(|(at, text)| texts[..at].contains(text))
        {
            warnings.push(format!(
                "{} loads the same text twice from two files. An import in place of the copy avoids that.",
                agent.name
            ));
        }
        warnings
    }

    fn detail(
        &self,
        agent: &AgentRule,
        status: AgentStatus,
        main: Option<usize>,
        via: Option<usize>,
    ) -> String {
        let name = |index: Option<usize>| {
            index.map_or_else(String::new, |index| self.files[index].display.clone())
        };
        match status {
            AgentStatus::Direct => format!("Reads {}.", name(main)),
            AgentStatus::Import => format!("Reads {}, which imports {}.", name(main), name(via)),
            AgentStatus::Link => format!("Reads {}, a link to {}.", name(main), name(via)),
            AgentStatus::Copy => {
                if main.and_then(|index| self.managed(index)).is_some() {
                    format!(
                        "Reads {}, a copy of {} that Ghost keeps in sync.",
                        name(main),
                        name(via)
                    )
                } else {
                    format!(
                        "Reads {}, which has the same text as {}.",
                        name(main),
                        name(via)
                    )
                }
            }
            AgentStatus::Drift => {
                let managed = main.and_then(|index| self.managed(index));
                if managed.as_ref().is_some_and(|managed| managed.edited) {
                    format!(
                        "Reads {}, a copy of {} that was edited by hand.",
                        name(main),
                        name(via)
                    )
                } else {
                    format!("Reads {}, a copy that is behind {}.", name(main), name(via))
                }
            }
            AgentStatus::Override => format!("Reads {} instead of the shared file.", name(main)),
            AgentStatus::Own => match main {
                Some(index) if self.import_of(index).is_some() && !agent.imports => format!(
                    "Reads {}, which only points to another file. {} doesn't follow imports.",
                    name(main),
                    agent.name
                ),
                _ => format!("Reads {}.", name(main)),
            },
            AgentStatus::NotSet => match main {
                Some(index) if self.files[index].blank() => {
                    format!("{} is empty, so there is nothing to read.", name(main))
                }
                Some(index) if self.import_of(index).is_some() => format!(
                    "Reads {}, which only points to another file. {} doesn't follow imports.",
                    name(main),
                    agent.name
                ),
                _ => "Reads nothing here yet.".into(),
            },
            AgentStatus::Unsupported => agent.no_user_file.into(),
        }
    }

    pub fn own_file(&self, agent: &AgentRule) -> Option<usize> {
        if !self.project() {
            let reads = agent.user?;
            return self.by_path(reads.own.0, reads.own.1);
        }
        let path = match agent.id {
            "claude" => "CLAUDE.md",
            "codex" | "pi" => "AGENTS.override.md",
            "hermes" => ".hermes.md",
            "omp" => ".omp/AGENTS.md",
            "antigravity" => "GEMINI.md",
            _ => return None,
        };
        self.by_path(RootId::Project, path)
    }

    pub fn snapshot(&self) -> Snapshot {
        let (mode, source) = self.mode();
        let default_source = self.default_source();
        let mut readers = vec![Vec::<String>::new(); self.files.len()];
        let mut agents_out = Vec::new();
        let mut mains = Vec::new();
        for agent in AGENTS {
            let (status, main, via, also) = self.status(agent, source);
            if let Some(loaded) = self.loaded(agent) {
                for index in &loaded.files {
                    readers[*index].push(agent.id.into());
                    if let Some(target) = self.import_of(*index).filter(|_| agent.imports)
                        && !readers[target].iter().any(|id| id == agent.id)
                    {
                        readers[target].push(agent.id.into());
                    }
                }
            }
            let warnings = self
                .loaded(agent)
                .map(|loaded| self.warnings(agent, &loaded.files))
                .unwrap_or_default();
            mains.push(main.filter(|_| status != AgentStatus::NotSet));
            agents_out.push(AgentInfo {
                id: agent.id.into(),
                name: agent.name.into(),
                installed: self.installed(agent),
                status,
                file: main.map(|index| self.files[index].key.clone()),
                via: via.map(|index| self.files[index].key.clone()),
                also: also
                    .iter()
                    .map(|index| self.files[*index].key.clone())
                    .collect(),
                shared_with: Vec::new(),
                detail: self.detail(agent, status, main, via),
                warnings,
                override_file: self
                    .own_file(agent)
                    .map(|index| self.files[index].key.clone()),
            });
        }
        for (at, main) in mains.iter().enumerate() {
            if let Some(main) = main {
                agents_out[at].shared_with = mains
                    .iter()
                    .enumerate()
                    .filter(|(other, file)| *other != at && *file == &Some(*main))
                    .map(|(other, _)| AGENTS[other].id.to_owned())
                    .collect();
            }
        }
        let relevant = |index: usize| {
            let file = &self.files[index];
            file.exists()
                || Some(index) == source
                || Some(index) == default_source
                || AGENTS.iter().any(|agent| {
                    self.installed(agent)
                        && (self.own_file(agent) == Some(index)
                            || self.reads(agent).is_some_and(|reads| {
                                self.by_path(reads.own.0, reads.own.1) == Some(index)
                            }))
                })
        };
        let files = (0..self.files.len())
            .filter(|index| relevant(*index))
            .map(|index| self.file_info(index, source, readers[index].clone()))
            .collect();
        let mut notices = self.context.notices.clone();
        notices.extend(self.notices.iter().cloned());
        if let Some(parent) = &self.parent_claude {
            notices.push(format!(
                "{parent} is in a parent folder, so Claude Code reads CLAUDE files here instead of AGENTS.md."
            ));
        }
        Snapshot {
            scope: self.context.scope,
            folder: self.project().then(|| self.roots[0].display.clone()),
            mode,
            source: source.map(|index| self.files[index].key.clone()),
            default_source: default_source.map(|index| self.files[index].key.clone()),
            source_choices: self
                .source_choices()
                .into_iter()
                .map(|index| self.files[index].key.clone())
                .collect(),
            conflict: default_source
                .map(|source| {
                    self.conflict(source)
                        .into_iter()
                        .map(|index| self.files[index].key.clone())
                        .collect()
                })
                .unwrap_or_default(),
            agents: agents_out,
            files,
            nested: self.nested.clone(),
            parents: self.parents.clone(),
            notices,
            git_repository: self.context.git.repository,
            worktree_branch: self.context.git.worktree_branch.clone(),
            revision: self.revision(),
        }
    }

    pub fn revision(&self) -> String {
        let mut digest = Sha256::new();
        for file in &self.files {
            digest.update(file.key.as_bytes());
            digest.update([file.kind as u8, u8::from(file.read_only)]);
            if let Some(bytes) = &file.bytes {
                digest.update(safe_fs::hash(bytes).as_bytes());
            }
        }
        if let Some(record) = &self.record {
            digest.update(record.source.as_bytes());
            for (key, hash) in &record.copies {
                digest.update(key.as_bytes());
                digest.update(hash.as_bytes());
            }
        }
        format!("{:x}", digest.finalize())
    }

    /// Why a file cannot be written here, if it cannot.
    pub fn blocked(&self, index: usize) -> Option<String> {
        let file = &self.files[index];
        match file.kind {
            FileKind::Missing | FileKind::File | FileKind::HardLink if file.read_only => {
                Some(format!(
                    "{} is read-only. Clear its read-only setting to edit it here.",
                    file.display
                ))
            }
            FileKind::Missing | FileKind::File | FileKind::HardLink => {
                if file.kind != FileKind::Missing && file.text.is_none() {
                    Some(format!(
                        "{} is not UTF-8 text. Edit it in another editor.",
                        file.display
                    ))
                } else {
                    None
                }
            }
            FileKind::Link => Some(format!(
                "{} is a link. Edit the file it points to.",
                file.display
            )),
            FileKind::Outside => Some(format!(
                "{} links outside this folder, so Ghost doesn't follow or change it.",
                file.display
            )),
            FileKind::Dangling => Some(format!(
                "{} is a link to a file that doesn't exist. Agents read nothing there; remove the link to use this name.",
                file.display
            )),
            FileKind::Directory => Some(format!("{} is a folder, not a file.", file.display)),
            FileKind::NotText => Some(format!(
                "{} is not UTF-8 text. Edit it in another editor.",
                file.display
            )),
            FileKind::TooLarge => Some(format!(
                "{} is over 1 MiB. Open it in your editor.",
                file.display
            )),
            FileKind::Ambiguous => Some(format!(
                "Two files named like {} differ only by letter case. Keep one of them first.",
                file.path
            )),
            FileKind::Unreadable => Some(format!(
                "{} could not be read. Check its permissions.",
                file.display
            )),
        }
    }

    fn file_info(&self, index: usize, source: Option<usize>, readers: Vec<String>) -> FileInfo {
        let file = &self.files[index];
        let blocked = self.blocked(index);
        let managed = self.managed(index);
        let copy_of = source
            .filter(|source| managed.is_none() && self.copy_of(index, *source))
            .map(|source| self.files[source].key.clone());
        let note = blocked.clone().or_else(|| {
            if let Some(disk) = &file.disk {
                return Some(format!(
                    "Named {disk} on disk. Agents find it on Windows and macOS, which ignore letter case, but Git and other tools treat it as a different name."
                ));
            }
            if file.kind == FileKind::HardLink {
                return file.link_to.map(|target| {
                    format!(
                        "The same file as {} (a hard link). Saving writes both as separate files with the same text.",
                        self.files[target].display
                    )
                });
            }
            if file.exists() && file.blank() {
                return Some("Empty. Agents skip it.".into());
            }
            match file.git {
                Some(GitState::Untracked) => {
                    Some("Not committed, so new worktree chats won't have it.".into())
                }
                Some(GitState::Ignored) => {
                    Some("Ignored by Git, so new worktree chats won't have it.".into())
                }
                Some(GitState::Modified) => Some(
                    "Has changes that aren't committed. New worktree chats start from the committed version."
                        .into(),
                ),
                _ => None,
            }
        });
        FileInfo {
            key: file.key.clone(),
            path: file.display.clone(),
            kind: file.kind,
            disk_name: file.disk.clone(),
            link_to: file.link_to.map(|target| self.files[target].key.clone()),
            text: (blocked.is_none() || file.read_only)
                .then(|| file.text.clone())
                .flatten(),
            encoding: file.encoding,
            bytes: u32::try_from(file.size).unwrap_or(u32::MAX),
            lines: file.text.as_deref().map_or(0, |text| {
                u32::try_from(text::lines(text)).unwrap_or(u32::MAX)
            }),
            revision: file.bytes.as_deref().map(safe_fs::hash),
            read_only: file.read_only,
            empty: file.exists() && file.blank(),
            editable: blocked.is_none(),
            import_of: self
                .import_of(index)
                .map(|target| self.files[target].key.clone()),
            imports: file.text.as_deref().map(imports::find).unwrap_or_default(),
            managed,
            copy_of,
            readers,
            git: file.git,
            note,
        }
    }

    fn scan_parents(&mut self) {
        let root = &self.roots[0].path;
        let home = self.context.home.clone();
        let mut current = root.parent().map(Path::to_path_buf);
        let mut depth = 0;
        while let Some(folder) = current {
            depth += 1;
            if depth > 16 {
                break;
            }
            for name in NESTED_NAMES.iter().copied().chain([".claude/CLAUDE.md"]) {
                // The home folder's .claude/CLAUDE.md is the user file, not a project parent.
                if name == ".claude/CLAUDE.md" && home.as_deref() == Some(folder.as_path()) {
                    continue;
                }
                let path = folder.join(name);
                let Ok(metadata) = std::fs::metadata(&path) else {
                    continue;
                };
                if !metadata.is_file() {
                    continue;
                }
                let display = super::display_path(&path, home.as_deref());
                let claude = matches!(name, "CLAUDE.md" | ".claude/CLAUDE.md" | "CLAUDE.local.md");
                if claude && self.parent_claude.is_none() {
                    self.parent_claude = Some(display.clone());
                }
                let readers = parent_readers(name, self.context.git.repository);
                if self.parents.len() < NESTED_LIMIT {
                    self.parents.push(NestedFile {
                        path: display,
                        bytes: u32::try_from(metadata.len()).unwrap_or(u32::MAX),
                        readers,
                    });
                }
            }
            current = folder.parent().map(Path::to_path_buf);
        }
    }

    fn scan_nested(&mut self) {
        let Ok(root) = Dir::open_ambient_dir(&self.roots[0].path, cap_std::ambient_authority())
        else {
            return;
        };
        let mut queue = vec![(root, String::new(), 0usize)];
        let mut visited = 0;
        while let Some((dir, prefix, depth)) = queue.pop() {
            visited += 1;
            if visited > NESTED_DIRS || self.nested.len() >= NESTED_LIMIT {
                self.notices.push(
                    "Only part of this folder was searched for instruction files in subfolders."
                        .into(),
                );
                break;
            }
            let Ok(entries) = dir.entries() else {
                continue;
            };
            let mut children = Vec::new();
            for entry in entries.flatten().take(LISTING_LIMIT) {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    if depth < NESTED_DEPTH
                        && !SKIP_DIRS.contains(&name.as_str())
                        && !name.starts_with('.')
                    {
                        children.push(name);
                    }
                } else if !prefix.is_empty()
                    && kind.is_file()
                    && let Some(known) = NESTED_NAMES
                        .iter()
                        .find(|known| known.eq_ignore_ascii_case(&name))
                {
                    let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                    self.nested.push(NestedFile {
                        path: format!("{prefix}{name}"),
                        bytes: u32::try_from(size).unwrap_or(u32::MAX),
                        readers: nested_readers(known),
                    });
                }
            }
            children.sort();
            for name in children.into_iter().rev() {
                if let Ok(child) = dir.open_dir_nofollow(&name) {
                    queue.push((child, format!("{prefix}{name}/"), depth + 1));
                }
            }
        }
        self.nested.sort_by(|a, b| a.path.cmp(&b.path));
    }
}

fn parent_readers(name: &str, repository: bool) -> Vec<String> {
    let mut readers = match name {
        "AGENTS.md" | "AGENTS.override.md" => vec![
            "codex",
            "pi",
            "omp",
            "opencode",
            "hermes",
            "grok",
            "antigravity",
        ],
        "CLAUDE.md" | ".claude/CLAUDE.md" | "CLAUDE.local.md" => {
            vec!["claude", "pi", "omp", "opencode", "grok"]
        }
        "GEMINI.md" => vec!["antigravity"],
        _ => vec!["hermes"],
    };
    if !repository {
        // Outside a repository Codex, Hermes and Grok Build read only the working folder.
        readers.retain(|id| !matches!(*id, "codex" | "hermes" | "grok"));
    }
    readers.into_iter().map(str::to_owned).collect()
}

fn nested_readers(name: &str) -> Vec<String> {
    match name {
        "AGENTS.md" | "AGENTS.override.md" => {
            vec!["claude", "codex", "hermes", "cursor", "grok", "antigravity"]
        }
        "CLAUDE.md" | "CLAUDE.local.md" => vec!["claude", "cursor", "grok", "hermes"],
        "GEMINI.md" => vec!["antigravity"],
        _ => vec!["hermes"],
    }
    .into_iter()
    .map(str::to_owned)
    .collect()
}

pub(crate) fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").trim_end().to_owned()
}

fn kib(bytes: usize) -> String {
    if bytes.is_multiple_of(1024) {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}

fn group(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (at, ch) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn same_text_path(a: &Path, b: &Path) -> bool {
    let norm = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        if cfg!(any(windows, target_os = "macos")) {
            text.to_lowercase()
        } else {
            text
        }
    };
    norm(a) == norm(b)
}

/// Finds the entry for `path` regardless of letter case, refusing linked parent folders.
enum Lookup {
    Missing,
    Found(String),
    Ambiguous,
    Unsafe,
}

/// One directory listing per folder and scan, so a large project root is listed once.
#[derive(Default)]
struct Listings {
    folders: std::collections::HashMap<(usize, String), Option<Vec<String>>>,
}

impl Listings {
    /// `None` when the folder is missing; an error when it is a link or unreadable.
    fn get(
        &mut self,
        root: usize,
        dir: &Dir,
        parent: &str,
    ) -> std::result::Result<Option<&[String]>, ()> {
        let key = (root, parent.to_owned());
        if !self.folders.contains_key(&key) {
            let listing = if parent.is_empty() {
                Some(list(dir).ok_or(())?)
            } else {
                match dir.symlink_metadata(parent) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(_) => return Err(()),
                    Ok(metadata) if metadata.file_type().is_symlink() => return Err(()),
                    Ok(metadata) if !metadata.is_dir() => None,
                    Ok(_) => {
                        let child = safe_fs::directory(dir, parent).map_err(|_| ())?;
                        Some(list(&child).ok_or(())?)
                    }
                }
            };
            self.folders.insert(key.clone(), listing);
        }
        Ok(self
            .folders
            .get(&key)
            .and_then(|listing| listing.as_deref()))
    }
}

fn list(dir: &Dir) -> Option<Vec<String>> {
    Some(
        dir.entries()
            .ok()?
            .flatten()
            .take(LISTING_LIMIT)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect(),
    )
}

fn lookup(listings: &mut Listings, root: usize, dir: &Dir, path: &str) -> Lookup {
    let (parent, name) = match path.rsplit_once('/') {
        Some((parent, name)) => (parent, name),
        None => ("", path),
    };
    let names = match listings.get(root, dir, parent) {
        Err(()) => return Lookup::Unsafe,
        Ok(None) => return Lookup::Missing,
        Ok(Some(names)) => names,
    };
    let matches = names
        .iter()
        .filter(|found| found.eq_ignore_ascii_case(name))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Lookup::Missing,
        [one] => Lookup::Found(if parent.is_empty() {
            (*one).clone()
        } else {
            format!("{parent}/{one}")
        }),
        _ => Lookup::Ambiguous,
    }
}

fn scan_file(
    root_info: &Root,
    root: usize,
    path: &'static str,
    context: &Context,
    listings: &mut Listings,
) -> Found {
    let key = format!("{}:{path}", root_info.id.as_str());
    let display = if root_info.id == RootId::Project {
        path.to_owned()
    } else {
        format!("{}/{path}", root_info.display.trim_end_matches('/'))
    };
    let mut found = Found {
        root,
        path,
        key,
        display,
        disk: None,
        kind: FileKind::Missing,
        bytes: None,
        text: None,
        encoding: None,
        size: 0,
        read_only: false,
        link_raw: None,
        link_to: None,
        git: None,
    };
    let Ok(dir) = Dir::open_ambient_dir(&root_info.path, cap_std::ambient_authority()) else {
        return found;
    };
    let actual = match lookup(listings, root, &dir, path) {
        Lookup::Missing => return found,
        Lookup::Ambiguous => {
            found.kind = FileKind::Ambiguous;
            return found;
        }
        Lookup::Unsafe => {
            found.kind = FileKind::Outside;
            return found;
        }
        Lookup::Found(actual) => actual,
    };
    if actual != path {
        found.disk = Some(actual.clone());
    }
    if root_info.id == RootId::Project {
        found.git = context.git.files.get(path).copied();
    }
    let Ok(metadata) = dir.symlink_metadata(&actual) else {
        found.kind = FileKind::Unreadable;
        return found;
    };
    found.read_only = metadata.permissions().readonly();
    if metadata.file_type().is_symlink() {
        found.kind = FileKind::Outside;
        found.link_raw = dir.read_link_contents(&actual).ok();
        return found;
    }
    if metadata.is_dir() {
        found.kind = FileKind::Directory;
        return found;
    }
    found.size = metadata.len();
    match safe_fs::read(&dir, &actual) {
        Ok(Some(bytes)) => {
            found.kind = FileKind::File;
            match text::decode(&bytes) {
                Some((text, encoding)) => {
                    found.text = Some(text);
                    found.encoding = Some(encoding);
                }
                None => found.kind = FileKind::NotText,
            }
            found.bytes = Some(bytes);
        }
        Ok(None) => {}
        Err(Error::Limit) => found.kind = FileKind::TooLarge,
        Err(Error::UnsafePath) => found.kind = FileKind::Outside,
        Err(_) => found.kind = FileKind::Unreadable,
    }
    found
}

/// Resolves symbolic links to instruction files in the same folder and finds hard links.
/// Where a link really leads, as the operating system follows it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    /// Inside the folder, at this relative path (forward slashes).
    Inside(String),
    /// Outside the folder, or not resolvable for a reason other than a missing target.
    Outside,
    /// The target does not exist.
    Dangling,
}

/// Classifies a link from the canonical folder and the link path's own canonical resolution.
/// Never resolves `..` by text: a linked folder in the target makes text and the OS disagree.
pub(crate) fn classify_link(root: Option<&Path>, target: std::io::Result<PathBuf>) -> LinkTarget {
    let target = match target {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return LinkTarget::Dangling,
        Err(_) => return LinkTarget::Outside,
    };
    let Some(root) = root else {
        return LinkTarget::Outside;
    };
    match target.strip_prefix(root) {
        Ok(rest) if !rest.as_os_str().is_empty() => {
            let rest = rest.to_string_lossy().replace('\\', "/");
            if rest
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            {
                LinkTarget::Outside
            } else {
                LinkTarget::Inside(rest)
            }
        }
        _ => LinkTarget::Outside,
    }
}

pub(crate) fn link_files(roots: &[Root], files: &mut [Found]) {
    for index in 0..files.len() {
        if files[index].link_raw.is_none() {
            continue;
        }
        let root = &roots[files[index].root];
        // Ask the OS: canonicalizing the link's own path follows it, and every linked folder in
        // its target, exactly as an agent opening the file would.
        let canonical_root = std::fs::canonicalize(&root.path).ok();
        let resolved = std::fs::canonicalize(root.path.join(files[index].disk_path()));
        let found = match classify_link(canonical_root.as_deref(), resolved) {
            LinkTarget::Inside(target) => (0..files.len()).find(|other| {
                *other != index
                    && files[*other].root == files[index].root
                    && files[*other].kind == FileKind::File
                    && files[*other].disk_path().eq_ignore_ascii_case(&target)
            }),
            LinkTarget::Outside => None,
            LinkTarget::Dangling => {
                files[index].kind = FileKind::Dangling;
                continue;
            }
        };
        if let Some(target) = found {
            files[index].kind = FileKind::Link;
            files[index].link_to = Some(target);
            files[index].size = files[target].size;
        }
    }
    for index in 0..files.len() {
        if files[index].kind != FileKind::File {
            continue;
        }
        for earlier in 0..index {
            if files[earlier].kind != FileKind::File || files[earlier].bytes != files[index].bytes {
                continue;
            }
            let a = roots[files[earlier].root]
                .path
                .join(files[earlier].disk_path());
            let b = roots[files[index].root].path.join(files[index].disk_path());
            if same_file::is_same_file(&a, &b).unwrap_or(false) && !same_text_path(&a, &b) {
                files[index].kind = FileKind::HardLink;
                files[index].link_to = Some(earlier);
                break;
            }
        }
    }
}
