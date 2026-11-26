//! Cloning a repository into a new folder with the person's own Git.
//!
//! A check is inert: it reads the form, finds Git and answers with the exact command, or the first
//! problem, beside the field it belongs to. Start creates the destination itself, atomically, so
//! Ghost knows which folder is its own; a canceled or failed clone removes that folder and only
//! that folder. Git runs with the person's own configuration, credential helpers and SSH setup:
//! Ghost never reads, stores or passes credentials.
mod files;
mod journal;
mod process;
mod progress;
mod validate;

use cap_std::fs::Dir;
use ghost_process::{EnvKey, ResolvedEnv, find_exe};
use ghost_protocol::setup_actions::{
    CloneCheck, CloneField, CloneInput, CloneProblem, CloneRecord, CloneRecovery, CloneReview,
    CloneState,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tokio::sync::watch;

type Result<T> = std::result::Result<T, CloneError>;
#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("{0}")]
    Message(String),
    #[error("The folder could not be read.")]
    Io(#[from] std::io::Error),
}
impl CloneError {
    pub(crate) fn message(value: &str) -> Self {
        Self::Message(value.into())
    }
}

/// Variables that point Git at another repository. The person's own Git settings
/// (`GIT_CONFIG_*`, `GIT_SSH_COMMAND`, `GIT_ASKPASS`) pass through untouched.
const REPOSITORY_VARIABLES: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_REPLACE_REF_BASE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_SHALLOW_FILE",
    "GIT_QUARANTINE_PATH",
    "GIT_INTERNAL_SUPER_PREFIX",
];

/// Windows clone paths need room for Git's own files under the new folder.
const WINDOWS_PATH_LIMIT: usize = 220;

pub(crate) struct Prepared {
    pub(crate) view: CloneReview,
    pub(crate) parent: PathBuf,
    parent_handle: Dir,
    name: String,
    program: PathBuf,
    arguments: Vec<String>,
    env: Arc<ResolvedEnv>,
    expires: Instant,
}

pub(crate) struct Job {
    owner: String,
    record: Mutex<CloneRecord>,
    canceled: watch::Sender<bool>,
    changed: watch::Sender<u32>,
    journal: Option<Arc<journal::Journal>>,
}
impl Job {
    pub(crate) fn snapshot(&self) -> CloneRecord {
        lock(&self.record).clone()
    }
    /// Changes the record and wakes every watcher.
    pub(crate) fn update(&self, change: impl FnOnce(&mut CloneRecord)) {
        let revision = {
            let mut record = lock(&self.record);
            change(&mut record);
            record.revision = record.revision.wrapping_add(1);
            record.revision
        };
        self.changed.send_replace(revision);
    }
    pub(crate) fn finish(&self, state: CloneState, detail: &str) {
        let saved = self
            .journal
            .as_ref()
            .map(|journal| journal.finish(state))
            .transpose();
        self.update(|record| match saved {
            Ok(_) => {
                record.state = state;
                record.detail = detail.into();
            }
            Err(error) => {
                record.state = CloneState::Interrupted;
                record.detail = format!("{detail} {error}");
            }
        });
    }
}

struct Pending {
    owner: String,
    prepared: Prepared,
    created: Instant,
}

#[derive(Default)]
pub struct Clones {
    pending: Mutex<HashMap<String, Pending>>,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    root: Option<PathBuf>,
}
impl Drop for Clones {
    fn drop(&mut self) {
        self.cancel_all();
    }
}
impl Clones {
    /// The installation's durable data root. Production clones require this journal location.
    pub fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            pending: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
        }
    }

    pub fn recovery_list(&self) -> Result<Vec<CloneRecovery>> {
        let Some(root) = &self.root else {
            return Ok(Vec::new());
        };
        let active: Vec<_> = lock(&self.jobs)
            .values()
            .filter(|job| {
                matches!(
                    job.snapshot().state,
                    CloneState::Running | CloneState::CleanupPending
                )
            })
            .map(|job| job.snapshot().id)
            .collect();
        journal::list(root, &active)
    }

    pub fn recovery_get(&self, id: &str) -> Result<CloneRecovery> {
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| CloneError::message("Clone recovery storage is unavailable."))?;
        journal::get(root, id)
    }

    pub fn recovery_dismiss(&self, id: &str) -> Result<()> {
        if lock(&self.jobs).get(id).is_some_and(|job| {
            matches!(
                job.snapshot().state,
                CloneState::Running | CloneState::CleanupPending
            )
        }) {
            return Err(CloneError::message(
                "Wait for the running clone to stop before dismissing its record.",
            ));
        }
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| CloneError::message("Clone recovery storage is unavailable."))?;
        journal::dismiss(root, id)
    }
    /// Reads the form and, when it is complete, keeps a startable review for ten minutes. Typing
    /// asks again on every change, so only the newest reviews per window are kept.
    pub async fn check(
        &self,
        owner: &str,
        input: CloneInput,
        env: Arc<ResolvedEnv>,
    ) -> Result<CloneCheck> {
        let (check, prepared) = tokio::task::spawn_blocking(move || prepare(&input, &env))
            .await
            .map_err(|_| CloneError::message("The clone form could not be checked."))?;
        if let Some(prepared) = prepared {
            let mut pending = lock(&self.pending);
            pending.retain(|_, value| value.prepared.expires > Instant::now());
            while pending
                .values()
                .filter(|value| value.owner == owner)
                .count()
                >= 8
            {
                let oldest = pending
                    .iter()
                    .filter(|(_, value)| value.owner == owner)
                    .min_by_key(|(_, value)| value.created)
                    .map(|(id, _)| id.clone());
                match oldest {
                    Some(id) => pending.remove(&id),
                    None => break,
                };
            }
            if pending.len() >= 64 {
                return Err(CloneError::message(
                    "Too many clone forms are open. Close one and try again.",
                ));
            }
            pending.insert(
                prepared.view.id.clone(),
                Pending {
                    owner: owner.into(),
                    prepared,
                    created: Instant::now(),
                },
            );
        }
        Ok(check)
    }
    pub fn cancel_review(&self, owner: &str, id: &str) {
        let mut pending = lock(&self.pending);
        if pending.get(id).is_some_and(|value| value.owner == owner) {
            pending.remove(id);
        }
    }
    pub(crate) fn take(&self, owner: &str, id: &str) -> Result<(Prepared, Arc<Job>)> {
        let mut pending = lock(&self.pending);
        if !pending
            .get(id)
            .is_some_and(|value| value.owner == owner && value.prepared.expires > Instant::now())
        {
            return Err(CloneError::message(
                "This clone form is out of date. Check the address and folder again.",
            ));
        }
        let mut jobs = lock(&self.jobs);
        let active = |job: &Arc<Job>| {
            matches!(
                job.snapshot().state,
                CloneState::Running | CloneState::CleanupPending
            )
        };
        if jobs.values().filter(|job| active(job)).count() >= 2 {
            return Err(CloneError::message(
                "Two clones are already running. Wait for one to finish.",
            ));
        }
        if jobs.len() >= 32 {
            jobs.retain(|_, job| active(job));
        }
        let Some(Pending { prepared, .. }) = pending.remove(id) else {
            return Err(CloneError::message("Check the clone form again."));
        };
        let record = CloneRecord {
            id: id.into(),
            revision: 0,
            remote: prepared.view.remote.clone(),
            repository: prepared.view.repository.clone(),
            destination: prepared.view.destination.clone(),
            state: CloneState::Running,
            detail: "Starting Git.".into(),
            progress: None,
            output: Vec::new(),
            removed: false,
        };
        let journal = match &self.root {
            Some(root) => Some(journal::Journal::begin(root, &prepared)?),
            // Unit fixtures may use an ephemeral service. Production never starts without a journal.
            None if cfg!(test) => None,
            None => {
                return Err(CloneError::message(
                    "Clone recovery storage is unavailable. No folder was created.",
                ));
            }
        };
        let job = Arc::new(Job {
            owner: owner.into(),
            record: Mutex::new(record),
            canceled: watch::channel(false).0,
            changed: watch::channel(0).0,
            journal,
        });
        jobs.insert(id.into(), Arc::clone(&job));
        Ok((prepared, job))
    }
    /// The clone's record. With `after`, waits up to ten seconds for a newer revision first, so
    /// a window can follow Git's progress without polling.
    pub async fn status(&self, owner: &str, id: &str, after: Option<u32>) -> Result<CloneRecord> {
        let job = lock(&self.jobs)
            .get(id)
            .filter(|job| job.owner == owner)
            .cloned()
            .ok_or_else(|| CloneError::message("This clone is no longer open."))?;
        if let Some(after) = after {
            let mut changed = job.changed.subscribe();
            if *changed.borrow_and_update() == after {
                let _ = tokio::time::timeout(Duration::from_secs(10), changed.changed()).await;
            }
        }
        Ok(job.snapshot())
    }
    pub fn cancel(&self, owner: &str, id: &str) {
        self.cancel_review(owner, id);
        if let Some(job) = lock(&self.jobs).get(id).filter(|job| job.owner == owner) {
            job.canceled.send_replace(true);
        }
    }
    pub fn drop_owner(&self, owner: &str) {
        lock(&self.pending).retain(|_, value| value.owner != owner);
        for job in lock(&self.jobs).values().filter(|job| job.owner == owner) {
            job.canceled.send_replace(true);
        }
    }
    pub fn cancel_all(&self) {
        lock(&self.pending).clear();
        for job in lock(&self.jobs).values() {
            job.canceled.send_replace(true);
        }
    }
}

fn problem(field: CloneField, message: impl Into<String>) -> Option<CloneProblem> {
    Some(CloneProblem {
        field,
        message: message.into(),
    })
}

/// Reads the form in order (address, name, folder, Git) and stops at the first problem.
fn prepare(input: &CloneInput, env: &ResolvedEnv) -> (CloneCheck, Option<Prepared>) {
    let mut check = CloneCheck {
        repository: None,
        name: None,
        review: None,
        problem: None,
    };
    let remote = match validate::remote(&input.remote) {
        Ok(remote) => remote,
        Err(message) => {
            check.problem = problem(CloneField::Remote, message);
            return (check, None);
        }
    };
    check.repository = Some(remote.repository.clone());
    let name = input
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&remote.name)
        .to_owned();
    check.name = Some(name.clone());
    if let Err(message) = validate::name(&name) {
        check.problem = problem(CloneField::Name, message);
        return (check, None);
    }
    match plan(remote, name, input.parent.as_deref(), env) {
        Ok(prepared) => {
            check.review = Some(prepared.view.clone());
            (check, Some(prepared))
        }
        Err(found) => {
            check.problem = Some(found);
            (check, None)
        }
    }
}

fn plan(
    remote: validate::Remote,
    name: String,
    parent: Option<&str>,
    env: &ResolvedEnv,
) -> std::result::Result<Prepared, CloneProblem> {
    let fail = |field, message: &str| CloneProblem {
        field,
        message: message.into(),
    };
    let Some(raw) = parent.filter(|parent| !parent.is_empty()) else {
        return Err(fail(CloneField::Parent, "Choose a folder to clone into."));
    };
    let raw = PathBuf::from(raw);
    if !raw.is_absolute() {
        return Err(fail(CloneField::Parent, "Choose a folder to clone into."));
    }
    let unavailable = "That folder is not available any more. Choose another.";
    let parent = std::fs::canonicalize(&raw).map_err(|_| fail(CloneField::Parent, unavailable))?;
    if !parent.is_dir() {
        return Err(fail(CloneField::Parent, unavailable));
    }
    let shown = plain_path(&parent);
    let Some(shown_text) = shown.to_str().map(str::to_owned) else {
        return Err(fail(
            CloneField::Parent,
            "This folder's path has characters Git cannot be given. Choose another.",
        ));
    };
    let parent_handle = Dir::open_ambient_dir(&parent, cap_std::ambient_authority())
        .map_err(|_| fail(CloneField::Parent, unavailable))?;
    match files::absent(&parent_handle, &name) {
        Ok(true) => {}
        Ok(false) => {
            return Err(CloneProblem {
                field: CloneField::Name,
                message: format!("{shown_text} already has a {name} folder. Choose another name."),
            });
        }
        Err(_) => return Err(fail(CloneField::Parent, unavailable)),
    }
    let destination = shown.join(&name);
    let destination_text = destination.to_string_lossy().into_owned();
    if cfg!(windows) && destination_text.encode_utf16().count() > WINDOWS_PATH_LIMIT {
        return Err(fail(
            CloneField::Name,
            "This path is too long for Windows. Choose a shorter name or a folder nearer the drive.",
        ));
    }
    let mut env = env.clone();
    for key in REPOSITORY_VARIABLES {
        env.vars.remove(&EnvKey::from(*key));
    }
    let Some(program) = find_exe("git", &env.vars) else {
        return Err(fail(
            CloneField::Git,
            "Install Git to clone repositories. Ghost uses the Git on this computer.",
        ));
    };
    let arguments = vec![
        "clone".to_owned(),
        "--progress".to_owned(),
        "--".to_owned(),
        remote.url.clone(),
        destination_text.clone(),
    ];
    let command = std::iter::once("git".to_owned())
        .chain(arguments.iter().map(|argument| quoted(argument)))
        .collect::<Vec<_>>()
        .join(" ");
    let view = CloneReview {
        id: uuid::Uuid::now_v7().to_string(),
        remote: remote.url,
        repository: remote.repository,
        parent: shown_text,
        destination: destination_text,
        program: program.to_string_lossy().into_owned(),
        arguments: arguments.clone(),
        command,
    };
    Ok(Prepared {
        view,
        parent,
        parent_handle,
        name,
        program,
        arguments,
        env: Arc::new(env),
        expires: Instant::now() + Duration::from_secs(600),
    })
}

/// Display quoting only: the argument vector is passed to Git as is, never through a shell.
fn quoted(argument: &str) -> String {
    if argument.is_empty() || argument.contains(|c: char| c.is_whitespace() || c == '"') {
        format!("\"{}\"", argument.replace('"', "\\\""))
    } else {
        argument.to_owned()
    }
}

/// `\\?\C:\code` as `C:\code`, the form people and Git recognise. Other paths are unchanged.
pub(crate) fn plain_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::path::{Component, Prefix};
        let mut parts = path.components();
        let base = match parts.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::VerbatimDisk(drive) => {
                    Some(PathBuf::from(format!("{}:\\", char::from(drive))))
                }
                Prefix::VerbatimUNC(server, share) => {
                    let mut base = OsString::from("\\\\");
                    base.push(server);
                    base.push("\\");
                    base.push(share);
                    Some(PathBuf::from(base))
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(mut base) = base {
            for part in parts {
                if !matches!(part, Component::RootDir) {
                    base.push(part.as_os_str());
                }
            }
            return base;
        }
    }
    path.to_path_buf()
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) use process::execute;

#[cfg(test)]
mod tests;
