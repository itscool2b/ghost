//! Instruction files (AGENTS.md, CLAUDE.md and the rest) as one universal file every agent reads,
//! or one file per agent. Rust owns every path, every read and every write: the interface sends a
//! file key and text, gets a review of exact bytes back, and applies it by token.
//!
//! Nothing here runs a process, reads credentials or looks up the environment; the host supplies
//! the folders, which agents are installed, Git facts, and the app-owned state folder.

mod agents;
mod apply;
mod imports;
mod manifest;
mod model;
mod plan;
#[cfg(test)]
mod tests;
mod text;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use specta::Type;

pub use apply::ApplyResult;
pub use text::{Encoding, LineEnding};

/// What the Instructions surface is open on. The host resolves it to folders; the interface
/// never sends a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case", rename = "InstructionsTarget")]
pub enum Target {
    /// Each agent's user-level file.
    Global,
    /// A project folder on this device.
    Folder { folder_id: String },
    /// A chat's own folder: its worktree for a worktree chat.
    Chat { chat_id: String },
}

/// Which folder a snapshot covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", rename = "InstructionsScope")]
pub enum ScopeKind {
    /// A project folder (or a worktree chat's worktree).
    Project,
    /// Each agent's user-level file.
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", rename = "InstructionsMode")]
pub enum Mode {
    /// No agent reads an instruction file here yet.
    None,
    /// Every agent that reads something reads the same text.
    Universal,
    /// Agents read different text.
    PerAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", rename = "InstructionsFileKind")]
pub enum FileKind {
    Missing,
    File,
    /// A symbolic link to another instruction file in the same folder. Edits go to its target.
    Link,
    /// The same file as another instruction file (a hard link).
    HardLink,
    /// A link that leaves the folder, a linked parent folder, or a link to another kind of file.
    /// Ghost never follows it.
    Outside,
    /// A link whose target does not exist. Agents read nothing there; Ghost never writes it.
    Dangling,
    Directory,
    /// Not UTF-8 text. Copies stay byte-identical; editing happens in another editor.
    NotText,
    /// Over 1 MiB, the editor's limit.
    TooLarge,
    /// Two spellings that differ only by case exist side by side.
    Ambiguous,
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", rename = "InstructionsGitState")]
pub enum GitState {
    Tracked,
    /// Tracked, with changes not committed. New worktrees get the committed version.
    Modified,
    Untracked,
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", rename = "InstructionsAgentStatus")]
pub enum AgentStatus {
    /// Reads the file itself.
    Direct,
    /// Reads its own file, which only imports the source.
    Import,
    /// Reads a link to the source.
    Link,
    /// Reads a byte-identical copy Ghost keeps in sync (or an identical copy it found).
    Copy,
    /// Reads a copy that was edited by hand or is out of date.
    Drift,
    /// An override or higher-priority file hides the shared file from this agent.
    Override,
    /// Reads its own text.
    Own,
    /// Reads nothing here yet.
    NotSet,
    /// This agent has no file at this scope.
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "InstructionsManaged")]
pub struct Managed {
    pub source: String,
    /// The copy no longer matches what Ghost wrote.
    pub edited: bool,
    /// The source changed since the copy was written.
    pub source_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "InstructionsFile")]
pub struct FileInfo {
    pub key: String,
    /// Display path: `AGENTS.md`, `~/.codex/AGENTS.md`.
    pub path: String,
    pub kind: FileKind,
    /// The spelling on disk when it differs from the agents' spelling (`agents.md`).
    pub disk_name: Option<String>,
    /// The file a link points to.
    pub link_to: Option<String>,
    /// LF text without a byte order mark; only for editable files.
    pub text: Option<String>,
    pub encoding: Option<Encoding>,
    pub bytes: u32,
    pub lines: u32,
    pub revision: Option<String>,
    pub read_only: bool,
    pub empty: bool,
    pub editable: bool,
    /// The file only imports this one (`@AGENTS.md`).
    pub import_of: Option<String>,
    /// Every `@path` import, as written.
    pub imports: Vec<String>,
    pub managed: Option<Managed>,
    /// Identical text to this file without being recorded as a managed copy.
    pub copy_of: Option<String>,
    /// Agent ids that load this file here.
    pub readers: Vec<String>,
    pub git: Option<GitState>,
    /// One sentence when the file needs attention or cannot be edited here.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "InstructionsAgent")]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub status: AgentStatus,
    /// The main file it reads here.
    pub file: Option<String>,
    /// The source an import, link or copy leads to.
    pub via: Option<String>,
    /// Other files it loads with text of their own.
    pub also: Vec<String>,
    /// Other agents whose main file is the same.
    pub shared_with: Vec<String>,
    pub detail: String,
    pub warnings: Vec<String>,
    /// An agent-specific override file this agent supports here, for per-agent text.
    pub override_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "InstructionsNestedFile")]
pub struct NestedFile {
    /// Display path relative to the project folder (`packages/api/AGENTS.md`), or a parent path.
    pub path: String,
    pub bytes: u32,
    pub readers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "InstructionsSnapshot")]
pub struct Snapshot {
    pub scope: ScopeKind,
    /// The project folder as a display path; `None` for the global scope.
    pub folder: Option<String>,
    pub mode: Mode,
    /// The universal source when the mode is universal.
    pub source: Option<String>,
    /// The file a switch to Universal keeps the shared text in.
    pub default_source: Option<String>,
    /// Files that may hold the universal text at this scope.
    pub source_choices: Vec<String>,
    pub files: Vec<FileInfo>,
    pub agents: Vec<AgentInfo>,
    /// Files with different text that a switch to Universal must reconcile.
    pub conflict: Vec<String>,
    /// Instruction files in subfolders, shown read-only.
    pub nested: Vec<NestedFile>,
    /// Instruction files in parent folders that agents also load.
    pub parents: Vec<NestedFile>,
    pub notices: Vec<String>,
    pub git_repository: bool,
    /// The worktree's branch when this folder is a worktree chat's own folder.
    pub worktree_branch: Option<String>,
    /// Hash over every file's state, for refreshing after outside edits.
    pub revision: String,
}

/// Where a switch to Universal takes the shared text from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "file", rename_all = "snake_case")]
#[serde(rename = "InstructionsContent")]
pub enum Content {
    /// Keep the source file's text.
    Source,
    /// Use this file's text.
    File(String),
    /// Join every distinct text, the source's first.
    Combine,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case", rename = "InstructionsChange")]
pub enum Change {
    /// Write the editor's text into one file, creating it when missing.
    Save { file: String, text: String },
    /// One file every agent reads. `remove` lists overrides or extra files the person chose to
    /// remove (each is backed up). `agents` limits which agents get a file; `None` means the
    /// installed ones.
    Universal {
        source: String,
        content: Content,
        remove: Vec<String>,
        agents: Option<Vec<String>>,
    },
    /// Separate files: imports become full copies, links become files, managed copies stop
    /// being managed. Text stays as it is.
    PerAgent,
    /// Put the source's text back into a managed copy that was edited by hand.
    Restore { file: String },
    /// Take a hand-edited copy's text into the source and the other copies.
    Adopt { file: String },
    /// Rename a file spelled with other letter case to the agents' spelling.
    Rename { file: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", rename = "InstructionsAction")]
pub enum Action {
    Create,
    Replace,
    Remove,
    Rename,
    /// Not written; listed because its role changes (a copy stops being managed).
    Keep,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "InstructionsReviewFile")]
pub struct ReviewFile {
    pub key: String,
    pub path: String,
    pub action: Action,
    pub before: Option<String>,
    pub after: Option<String>,
    /// How this file serves its agents afterwards: "Imports AGENTS.md", "Managed copy of AGENTS.md".
    pub how: Option<String>,
    pub notes: Vec<String>,
    /// The replaced text is backed up in Ghost's data folder.
    pub backup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename = "InstructionsReview")]
pub struct Review {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub files: Vec<ReviewFile>,
    pub notes: Vec<String>,
    /// Overrides or extra files the person may choose to remove (each is backed up).
    pub optional_removals: Vec<String>,
}

/// One agent's user folder, resolved by the host from its environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRoot {
    pub id: String,
    pub path: PathBuf,
    /// `~/.codex`, or the full path when it is outside the home folder.
    pub display: String,
}

/// Git facts for a project folder, gathered by the host with read-only commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitFacts {
    pub repository: bool,
    /// Relative path to state, for the known instruction files that exist.
    pub files: HashMap<String, GitState>,
    pub worktree_branch: Option<String>,
}

/// Everything a snapshot or a change needs. Paths are absolute and chosen by the host.
#[derive(Debug, Clone)]
pub struct Context {
    pub scope: ScopeKind,
    pub project: Option<PathBuf>,
    pub user_roots: Vec<UserRoot>,
    pub home: Option<PathBuf>,
    /// Instruction agent ids (see [`agent_for_catalog`]) whose CLI is installed.
    pub installed: Vec<String>,
    pub git: GitFacts,
    /// App-owned folder for the managed-copy record and backups.
    pub state_dir: PathBuf,
    /// Codex `project_doc_max_bytes` when the person changed it.
    pub codex_limit: Option<usize>,
    /// Sentences the host adds to the snapshot (a Git query that could not run).
    pub notices: Vec<String>,
}

/// Codex's `project_doc_max_bytes` from `config.toml` in its home, when set. A missing, large or
/// malformed file gives `None`, and the documented 32 KiB default applies.
pub fn codex_doc_limit(codex_home: &Path) -> Option<usize> {
    let dir = cap_std::fs::Dir::open_ambient_dir(codex_home, cap_std::ambient_authority()).ok()?;
    let bytes = crate::safe_fs::read(&dir, "config.toml").ok()??;
    let document = std::str::from_utf8(&bytes)
        .ok()?
        .parse::<toml_edit::DocumentMut>()
        .ok()?;
    let value = document.get("project_doc_max_bytes")?.as_integer()?;
    usize::try_from(value).ok().filter(|value| *value > 0)
}

/// A reviewed change waiting for Apply. It expires after ten minutes.
pub struct Prepared {
    review: Review,
    roots: Vec<PathBuf>,
    mutations: Vec<apply::Mutation>,
    dependencies: Vec<apply::Dependency>,
    manifest: Option<apply::MetadataIntent>,
    state_dir: PathBuf,
    expires: Instant,
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // File contents stay out of logs.
        formatter
            .debug_struct("Prepared")
            .field("id", &self.review.id)
            .field("files", &self.review.files.len())
            .finish_non_exhaustive()
    }
}

impl Prepared {
    pub fn review(&self) -> &Review {
        &self.review
    }

    pub fn is_expired(&self) -> bool {
        Instant::now() > self.expires
    }
}

/// The instruction agent behind a catalog id (`claude-acp` is Claude Code).
pub fn agent_for_catalog(id: &str) -> Option<&'static str> {
    agents::for_catalog(id).map(|agent| agent.id)
}

/// Relative paths of every project-level file the scan reads, for the host's Git query.
pub fn project_paths() -> Vec<&'static str> {
    agents::known(true)
        .into_iter()
        .map(|(_, path)| path)
        .collect()
}

/// Each agent's user folder. `var` looks up one named variable in the host's resolved
/// environment; only the documented relocation variables are consulted.
pub fn user_roots(home: &Path, var: impl Fn(&str) -> Option<PathBuf>) -> Vec<UserRoot> {
    let absolute = |path: Option<PathBuf>| path.filter(|path| path.is_absolute());
    let config = absolute(var("XDG_CONFIG_HOME")).unwrap_or_else(|| home.join(".config"));
    let pi = absolute(var("PI_CODING_AGENT_DIR"));
    let roots = [
        (
            "claude",
            absolute(var("CLAUDE_CONFIG_DIR")).unwrap_or_else(|| home.join(".claude")),
        ),
        (
            "codex",
            absolute(var("CODEX_HOME")).unwrap_or_else(|| home.join(".codex")),
        ),
        ("pi", pi.clone().unwrap_or_else(|| home.join(".pi/agent"))),
        // oh-my-pi honours PI_CODING_AGENT_DIR for its default profile too.
        ("omp", pi.unwrap_or_else(|| home.join(".omp/agent"))),
        ("opencode", config.join("opencode")),
        ("gemini", home.join(".gemini")),
        (
            "grok",
            absolute(var("GROK_HOME")).unwrap_or_else(|| home.join(".grok")),
        ),
    ];
    roots
        .into_iter()
        .map(|(id, path)| UserRoot {
            id: id.into(),
            display: display_path(&path, Some(home)),
            path,
        })
        .collect()
}

/// `~/...` for paths in the home folder, forward slashes everywhere.
pub fn display_path(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        let rest = rest.to_string_lossy().replace('\\', "/");
        return if rest.is_empty() {
            "~".into()
        } else {
            format!("~/{rest}")
        };
    }
    path.to_string_lossy().replace('\\', "/")
}

/// Read every instruction file for the scope. Explicit action only; nothing is written.
pub fn snapshot(context: &Context) -> crate::Result<Snapshot> {
    let mut snapshot = model::Model::scan(context)?.snapshot();
    snapshot
        .notices
        .extend(recovery_notices(&context.state_dir));
    Ok(snapshot)
}

/// Build the review for a change. Nothing is written.
pub fn prepare(context: &Context, change: &Change) -> crate::Result<Prepared> {
    let model = model::Model::scan(context)?;
    let mut prepared = plan::prepare(&model, change)?;
    prepared.expires = Instant::now() + Duration::from_secs(600);
    Ok(prepared)
}

/// Apply a reviewed change. Refuses with [`crate::Error::Stale`] when any file changed since the
/// review, before writing anything. The host serializes this with other configuration writes.
pub fn apply(prepared: Prepared) -> crate::Result<ApplyResult> {
    if prepared.is_expired() {
        return Err(crate::Error::Stale);
    }
    let unresolved = apply::recovery::recover(&prepared.state_dir);
    let blocked = unresolved.iter().find(|pending| {
        prepared.mutations.iter().any(|mutation| {
            let root = &prepared.roots[mutation.root];
            pending.targets.iter().any(|target| {
                apply::recovery::overlaps(target, &root.join(&mutation.path))
                    || mutation
                        .rename_from
                        .as_ref()
                        .is_some_and(|from| apply::recovery::overlaps(target, &root.join(from)))
            })
        }) || prepared
            .manifest
            .as_ref()
            .is_some_and(|intent| pending.identity.as_ref() == Some(&intent.identity))
    });
    if let Some(pending) = blocked {
        return Ok(ApplyResult {
            id: prepared.review.id,
            changed: Vec::new(), rolled_back: false, recovery_required: true,
            message: Some("This change overlaps an interrupted Instructions change that still needs attention. External edits were preserved.".into()),
            backup: Some(pending.path.to_string_lossy().into_owned()),
        });
    }
    if let Some(intent) = &prepared.manifest {
        let record = manifest::Manifest::load_checked(&prepared.state_dir)?;
        if record.roots.get(&intent.identity) != intent.before.as_ref() {
            return Err(crate::Error::Stale);
        }
    }
    apply::Transaction {
        id: prepared.review.id.clone(),
        roots: &prepared.roots,
        mutations: prepared.mutations,
        dependencies: prepared.dependencies,
        backups: &prepared.state_dir.join("backups"),
        metadata: prepared.manifest,
        #[cfg(test)]
        fail_at: None,
    }
    .run()
}

/// Attempt exact-state recovery. The host must hold its configuration mutation gate.
pub fn recover(state_dir: &Path) {
    apply::recovery::recover(state_dir);
}

/// Read-only startup and Instructions notices, including retained recovery paths.
pub fn recovery_notices(state_dir: &Path) -> Vec<String> {
    apply::recovery::notices(state_dir)
}

#[cfg(test)]
pub(crate) fn apply_failing_at(prepared: Prepared, index: usize) -> crate::Result<ApplyResult> {
    apply::Transaction {
        id: prepared.review.id.clone(),
        roots: &prepared.roots,
        mutations: prepared.mutations,
        dependencies: prepared.dependencies,
        backups: &prepared.state_dir.join("backups"),
        metadata: prepared.manifest,
        fail_at: Some(index),
    }
    .run()
}
