//! Coordinated projects (an experiment): a project's coordinator chat, the worker chats it
//! starts, the Context files they share, the task board, the decision log and routines.
//!
//! A project is the sidebar's project. Coordination adds to it; nothing here replaces a chat, and
//! a coordinator or a worker is an ordinary chat with a few more tools.
use serde::{Deserialize, Serialize};
use specta::Type;

/// What a chat is in its project's coordination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationRole {
    /// Plans, delegates and reports to the person. It does not do the work itself.
    Coordinator,
    /// Does one assignment in a context of its own, then reports.
    Worker,
}

/// A task's column on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorTaskStatus {
    Todo,
    Doing,
    Review,
    Done,
}

impl CoordinatorTaskStatus {
    pub const ALL: [Self; 4] = [Self::Todo, Self::Doing, Self::Review, Self::Done];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Doing => "doing",
            Self::Review => "review",
            Self::Done => "done",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }
}

/// One tracked task. `number` names it everywhere (`T-12`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CoordinatorTask {
    pub number: u32,
    pub title: String,
    pub body: String,
    pub status: CoordinatorTaskStatus,
    pub tags: Vec<String>,
    /// The worker it is assigned to, by name.
    pub worker: Option<String>,
    /// A pull request or any other address that shows the work.
    pub link: Option<String>,
    /// Who last changed it: `you`, `coordinator` or `worker:<name>`.
    pub source: String,
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
}

/// One worker chat. Whether it is working now is its chat's status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CoordinatorWorker {
    pub chat_id: String,
    pub name: String,
    pub task: Option<u32>,
    /// What it last reported, cut to a preview.
    pub report: Option<String>,
    /// Turns it has finished.
    pub turns: u32,
    /// Its last report has not reached the coordinator yet.
    pub pending: bool,
    /// Dismissed from the list. Its chat stays.
    pub closed: bool,
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub reported_at: Option<i64>,
}

/// One Context file, without its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ContextFileInfo {
    pub path: String,
    pub revision: String,
    pub bytes: u32,
    /// Its first heading or line, for the index agents are primed with.
    pub summary: String,
    pub source: String,
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
}

/// One Context file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ContextFile {
    pub path: String,
    pub revision: String,
    pub content: String,
    pub source: String,
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
    /// Earlier revisions, newest first: who wrote each and when. The text of one is read by its
    /// revision.
    pub history: Vec<ContextRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ContextRevision {
    pub revision: String,
    pub source: String,
    pub bytes: u32,
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
}

/// One entry of the decision log. Entries are only ever added, by the person or the coordinator;
/// `source` says which, and agents are shown the two apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CoordinatorDecision {
    pub number: u32,
    pub text: String,
    pub source: String,
    /// The person took it back: it stays in the log, and agents are no longer shown it.
    pub retracted: bool,
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
}

/// A prompt the coordinator is sent again and again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CoordinatorRoutine {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub every_minutes: u32,
    pub enabled: bool,
    #[specta(type = specta_typescript::Number)]
    pub next_at: i64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_at: Option<i64>,
}

/// Everything the project page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CoordinatorSnapshot {
    pub project_id: String,
    /// The coordinator's chat, while it exists and is not archived.
    pub coordinator_chat_id: Option<String>,
    /// What the project is for, as the person wrote it.
    pub brief: String,
    /// No worker starts, no routine fires and no report is passed on while this is set.
    pub paused: bool,
    /// Why Ghost paused it by itself, if it did.
    pub pause_reason: Option<String>,
    pub workers: Vec<CoordinatorWorker>,
    pub tasks: Vec<CoordinatorTask>,
    pub files: Vec<ContextFileInfo>,
    /// Newest first.
    pub decisions: Vec<CoordinatorDecision>,
    pub routines: Vec<CoordinatorRoutine>,
}

/// A chat's place in a coordinated project, for the sidebar and the chat's header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CoordinationMember {
    pub chat_id: String,
    pub project_id: String,
    pub role: CoordinationRole,
    /// A worker's name; `coordinator` for the coordinator.
    pub name: String,
}

/// Starting a project's coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CoordinatorStart {
    pub project_id: String,
    /// The project folder the coordinator works from.
    pub folder_id: String,
    /// The agent to coordinate with. `None` takes the project's default agent, else the first ready.
    pub agent_id: Option<String>,
    pub brief: String,
}

/// One change to a project's coordination, from the project page or an agent's tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoordinatorAction {
    Pause {
        paused: bool,
    },
    Brief {
        text: String,
    },
    /// Adds a task (`number: None`) or changes one. Absent fields keep their value.
    TaskSave {
        number: Option<u32>,
        title: Option<String>,
        body: Option<String>,
        status: Option<CoordinatorTaskStatus>,
        tags: Option<Vec<String>>,
        link: Option<String>,
    },
    /// Moves a task to a column, before the task `before` or to the column's end.
    TaskMove {
        number: u32,
        status: CoordinatorTaskStatus,
        before: Option<u32>,
    },
    TaskDelete {
        number: u32,
    },
    /// Replaces a file's whole text. `expected_revision` is the revision that was read; `None`
    /// creates the file and fails if it exists.
    FileSave {
        path: String,
        content: String,
        expected_revision: Option<String>,
    },
    /// Adds text to a file's end, creating it if needed. Never loses what another writer added.
    FileAppend {
        path: String,
        text: String,
    },
    FileDelete {
        path: String,
        expected_revision: String,
    },
    /// Puts back an earlier revision's text as a new revision.
    FileRestore {
        path: String,
        revision: String,
    },
    DecisionAdd {
        text: String,
    },
    /// Takes a decision back. Only the person can.
    DecisionRetract {
        number: u32,
    },
    RoutineSave {
        id: Option<String>,
        title: String,
        prompt: String,
        every_minutes: u32,
        enabled: bool,
    },
    RoutineDelete {
        id: String,
    },
    /// Dismisses a worker from the list. Its chat stays.
    WorkerClose {
        chat_id: String,
    },
}

/// What the window hears after a committed change to a project's coordination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CoordinatorChanged {
    pub project_id: String,
}

/// The limits every writer is held to, the person and agents alike.
pub mod limits {
    /// One Context file's text.
    pub const FILE_BYTES: usize = 64 * 1024;
    pub const FILES: usize = 200;
    pub const PATH_CHARS: usize = 120;
    pub const PATH_SEGMENTS: usize = 4;
    /// Revisions kept for one file; older ones go.
    pub const FILE_REVISIONS: usize = 20;
    pub const DECISION_BYTES: usize = 2 * 1024;
    pub const DECISIONS: usize = 2000;
    pub const TASKS: usize = 3000;
    pub const TASK_TITLE_CHARS: usize = 200;
    pub const TASK_BODY_BYTES: usize = 8 * 1024;
    pub const TASK_TAGS: usize = 8;
    pub const TAG_CHARS: usize = 32;
    pub const LINK_CHARS: usize = 400;
    pub const ROUTINES: usize = 20;
    pub const ROUTINE_TITLE_CHARS: usize = 120;
    pub const ROUTINE_PROMPT_BYTES: usize = 4 * 1024;
    pub const ROUTINE_MIN_MINUTES: u32 = 5;
    pub const ROUTINE_MAX_MINUTES: u32 = 7 * 24 * 60;
    pub const BRIEF_BYTES: usize = 8 * 1024;
    pub const REPORT_BYTES: usize = 16 * 1024;
    pub const WORKER_NAME_CHARS: usize = 40;
    /// Worker records a project keeps, dismissed ones included.
    pub const WORKERS: usize = 1000;
    /// Workers with a turn under way at once.
    pub const LIVE_WORKERS: usize = 4;
    /// An assignment or a message to a worker.
    pub const ASSIGNMENT_BYTES: usize = 24 * 1024;
    /// Reports and routines passed to one coordinator in an hour before Ghost pauses the
    /// project, so two agents answering each other cannot run all night.
    pub const EVENTS_PER_HOUR: usize = 60;
}
