//! The writer thread: the FIFO and its group commits, the read channel served from saved data while
//! saving is paused, disk-full and busy pauses, poison isolation into the dead-letter file, and
//! shutdown.
//!
//! The FIFO is a queue under one mutex plus two semaphores (ops and bytes) that async senders
//! reserve from, so a full FIFO is backpressure: an actor waits for room, stops reading its driver's
//! events, and the agent blocks on its own stdout. The writer takes up to 256 ops or 4 MiB at a time
//! into one `BEGIN IMMEDIATE` transaction; a read (or any call that answers) ends the batch, runs
//! after the commit, and so sees every op queued before it. Permits are released when the writer
//! takes ops out of the FIFO, so memory stays bounded by the FIFO plus one batch.
//!
//! A full disk or a database busy past its timeout stalls the writer on the failed batch: it keeps
//! only that batch, retries it (a busy one without SQLite's own wait, so the retries never hold the
//! reads up), and stops taking ops, while it still serves the second channel (reads from saved
//! data). Any other error rolls back and re-runs the batch one op per transaction; an op that fails
//! again (or panics) goes to `<profile>/backups/deadletter-<millis>.jsonl`. Damage is recorded for
//! the next open's repair, and from then on every write goes to the dead-letter file so sessions
//! keep running.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ghost_protocol::session::{BlobText, HistoryPage, PauseReason, StorageState, StorageStatus};
use rusqlite::{ErrorCode, TransactionBehavior};
use serde::Serialize;
use tokio::runtime::Handle;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot, watch};

use super::blobs::{self, BLOB_GONE, BLOB_UNSAVED};
use super::compact::{self, CompactLimits};
use super::recover::{self, Scope};
use super::{
    AgentSessionPatch, AgentSessionRow, ChatLoad, ChatRecord, EventRow, NewChat, Op, QueuedRow,
    RecoveryReport, StoreError, TitleSource, TurnEnd, TurnRow, history, rows,
};
use crate::error::CoreError;
use crate::fsutil;
use crate::owner::ProfileOwner;
use crate::store::{Store, is_damage};
use crate::time::now_millis;

/// The status a damaged database shows.
const DAMAGED: &str = "Ghost's database is damaged. Restart Ghost to repair it; recent messages are kept in a recovery file.";

/// The writer's limits and timers (appendix A); tests shrink them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WriterConfig {
    /// The FIFO holds this many ops at most...
    pub(crate) fifo_ops: usize,
    /// ...and this many bytes (one larger op passes alone).
    pub(crate) fifo_bytes: usize,
    pub(crate) batch_ops: usize,
    pub(crate) batch_bytes: usize,
    /// A failed batch on a full disk is retried this often.
    pub(crate) disk_full_retry: Duration,
    /// A busy database is retried after this, doubling...
    pub(crate) busy_backoff_first: Duration,
    /// ...up to this.
    pub(crate) busy_backoff_max: Duration,
    /// A database busy this long pauses saving.
    pub(crate) busy_pause_after: Duration,
    pub(crate) compact: CompactLimits,
    /// Whether the store's pass after startup runs ([`super::ChatStore`]'s leftovers).
    pub(crate) leftovers: bool,
    /// How long after startup that pass builds the indexes recovery reads through, when they are
    /// missing. A build visits every event, and the writer answers nothing meanwhile, so it must
    /// not fall on the moment the window asks for its first lists.
    pub(crate) index_delay: Duration,
}

impl WriterConfig {
    /// FIFO 1,024 ops and 32 MiB; batches of 256 ops or 4 MiB; disk-full retry 5 s; busy backoff
    /// 100 ms to 2 s, paused after 10 s; the pass after startup runs, and builds missing indexes
    /// 15 s in. This crate's own tests leave the pass off unless they ask for it: they place every
    /// op of a batch themselves, and its requests would land between them.
    pub(crate) const STANDARD: WriterConfig = WriterConfig {
        fifo_ops: 1024,
        fifo_bytes: 32 * 1024 * 1024,
        batch_ops: 256,
        batch_bytes: 4 * 1024 * 1024,
        disk_full_retry: Duration::from_secs(5),
        busy_backoff_first: Duration::from_millis(100),
        busy_backoff_max: Duration::from_secs(2),
        busy_pause_after: Duration::from_secs(10),
        compact: CompactLimits::STANDARD,
        leftovers: !cfg!(test),
        index_delay: Duration::from_secs(15),
    };
}

/// A duration that is not known yet.
const NOT_TIMED: u64 = u64::MAX;

/// Milliseconds since `started`, for a log line or a timing.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .min(NOT_TIMED - 1)
}

/// What the store and the writer share.
pub(crate) struct Shared {
    pub(crate) workspace_activity: Arc<super::workspace_activity::Activity>,
    /// Each committed memory write, from the editor or an agent's grant alike.
    pub(crate) memory_changes:
        tokio::sync::broadcast::Sender<ghost_protocol::memory::ProjectMemoryChanged>,
    /// Each committed change to a project's coordination.
    pub(crate) coordinator_changes:
        tokio::sync::broadcast::Sender<ghost_protocol::coordinator::CoordinatorChanged>,
    queue: Mutex<Queue>,
    wake: Condvar,
    pub(crate) count: Arc<Semaphore>,
    pub(crate) bytes: Arc<Semaphore>,
    pub(crate) status: watch::Sender<StorageStatus>,
    paused: AtomicBool,
    /// The writer is at work it knows to be long (startup recovery, building an index). A caller
    /// waiting for an answer keeps waiting meanwhile, instead of reading the database as busy.
    settling: AtomicBool,
    /// Wakes the store's pass over ended turns that are not compacted yet.
    pub(crate) leftovers: tokio::sync::Notify,
    /// The indexes recovery reads through may be built now ([`WriterConfig::index_delay`] passed).
    index_due: AtomicBool,
    /// How long startup recovery took, and building recovery's indexes when this start built any,
    /// in milliseconds; [`NOT_TIMED`] until then.
    recovery_ms: AtomicU64,
    indexes_ms: AtomicU64,
    pub(crate) config: WriterConfig,
    /// Tests: transactions committed for batches.
    #[cfg(test)]
    pub(crate) commits: std::sync::atomic::AtomicU64,
}

struct Queue {
    items: VecDeque<Item>,
    saved: VecDeque<SavedRead>,
    pending: Pending,
    /// A shutdown was asked for, to finish by this deadline.
    shutdown: Option<Instant>,
    /// Told whether everything was saved, once the writer finished.
    waiters: Vec<std::sync::mpsc::Sender<bool>>,
    /// The writer finished: whether everything queued was saved.
    finished: Option<bool>,
}

/// One unit of the writer's work, as queued.
pub(crate) struct Item {
    work: Work,
    ticket: Ticket,
    permits: Option<Permits>,
}

/// Room an item holds in the FIFO until the writer takes it out.
pub(crate) struct Permits {
    _count: OwnedSemaphorePermit,
    _bytes: OwnedSemaphorePermit,
}

impl Permits {
    pub(crate) fn new(count: OwnedSemaphorePermit, bytes: OwnedSemaphorePermit) -> Permits {
        Permits {
            _count: count,
            _bytes: bytes,
        }
    }
}

/// What the writer does with an item.
pub(crate) enum Work {
    Op(Op),
    Request(Request),
    #[cfg(test)]
    Test(TestWork),
}

/// A store call that waits for its answer through the FIFO.
pub(crate) enum Request {
    SharedMcp {
        command: crate::mcp::Command,
        reply: oneshot::Sender<Result<crate::mcp::Reply, StoreError>>,
    },
    Memory {
        command: super::memory::Command,
        reply: oneshot::Sender<Result<super::memory::Reply, StoreError>>,
    },
    Coordinator {
        command: super::coordinator::Command,
        reply: oneshot::Sender<Result<super::coordinator::Reply, StoreError>>,
    },
    ReadSetupAttempt {
        chat_id: String,
        reply: oneshot::Sender<
            Result<Option<ghost_protocol::worktree_setup::WorktreeSetupAttempt>, StoreError>,
        >,
    },
    SetupAttempt {
        command: super::setup_attempt::Command,
        reply: oneshot::Sender<
            Result<Option<ghost_protocol::worktree_setup::WorktreeSetupAttempt>, StoreError>,
        >,
    },
    WorkspaceRemoval {
        command: super::workspace_removal::Command,
        reply: oneshot::Sender<Result<super::workspace_removal::Reply, StoreError>>,
    },
    FolderResolution {
        command: super::folder_resolution::Command,
        reply: oneshot::Sender<Result<super::folder_resolution::Reply, StoreError>>,
    },
    AgentCatalogs {
        command: super::agent_catalogs::Command,
        reply: oneshot::Sender<Result<Option<String>, StoreError>>,
    },
    Steering {
        command: super::steering::Command,
        reply: oneshot::Sender<Result<(), StoreError>>,
    },
    Flush {
        reply: oneshot::Sender<Result<(), StoreError>>,
    },
    WorktreeSetup {
        command: super::worktree_setup::Command,
        reply: oneshot::Sender<Result<ghost_protocol::worktree_setup::WorktreeSetup, StoreError>>,
    },
    Search {
        query: String,
        include_archived: bool,
        reply: oneshot::Sender<Result<ghost_protocol::search::SearchResults, StoreError>>,
    },
    /// Saves the line edits a pass counted off the writer ([`super::line_edits::save`]).
    SaveLineEdits {
        counted: Vec<super::line_edits::Counted>,
        reply: oneshot::Sender<Result<(), StoreError>>,
    },
    ReadLineEdits {
        since: ghost_protocol::Millis,
        reply: oneshot::Sender<Result<Vec<ghost_protocol::activity::LineEditBucket>, StoreError>>,
    },
    /// Every ended turn not compacted yet ([`compact::leftovers`]).
    Leftovers {
        reply: oneshot::Sender<Result<Vec<compact::Leftover>, StoreError>>,
    },
    /// Builds the indexes recovery reads through ([`recover::ensure_indexes`]); answers how many
    /// it built.
    EnsureIndexes {
        reply: oneshot::Sender<Result<u32, StoreError>>,
    },
    /// Stores the full-text rows read off the writer for a turn too large to merge
    /// ([`compact::save_text`]); answers whether it compacted the turn.
    SaveTurnText {
        chat_id: String,
        turn_id: String,
        rows: Vec<compact::FtsRow>,
        reply: oneshot::Sender<Result<bool, StoreError>>,
    },
    Conversation {
        command: super::conversation::ConversationCommand,
        reply: oneshot::Sender<Result<super::conversation::ConversationReply, StoreError>>,
    },
    Composer {
        command: super::composer::ComposerCommand,
        reply: oneshot::Sender<Result<super::composer::ComposerReply, StoreError>>,
    },
    CheckpointContext {
        chat_id: String,
        turn_id: Option<String>,
        reply: oneshot::Sender<Result<(ChatRecord, Option<String>), StoreError>>,
    },
    Organization {
        command: super::organization::OrganizationCommand,
        reply: oneshot::Sender<Result<super::organization::Organization, StoreError>>,
    },
    CreateChat {
        new: NewChat,
        reply: oneshot::Sender<Result<ChatRecord, StoreError>>,
    },
    ExportMarkdown {
        chat_id: String,
        reply: oneshot::Sender<Result<String, StoreError>>,
    },
    Chats {
        reply: oneshot::Sender<Result<Vec<ChatRecord>, StoreError>>,
    },
    LoadChat {
        chat_id: String,
        reply: oneshot::Sender<Result<Option<ChatLoad>, StoreError>>,
    },
    ReadBlob {
        sha256: String,
        offset: u64,
        len: u32,
        reply: oneshot::Sender<Result<BlobText, StoreError>>,
    },
    Acknowledge {
        quarantined: String,
        reply: oneshot::Sender<Result<bool, CoreError>>,
    },
}

/// A read served from saved data, outside the FIFO, even while the writer is stalled.
pub(crate) enum SavedRead {
    Search {
        query: String,
        include_archived: bool,
        reply: oneshot::Sender<Result<ghost_protocol::search::SearchResults, StoreError>>,
    },
    CheckpointContext {
        chat_id: String,
        turn_id: Option<String>,
        reply: oneshot::Sender<Result<(ChatRecord, Option<String>), StoreError>>,
    },
    Organization {
        filter: ghost_protocol::workspace::WorkspaceFilter,
        reply: oneshot::Sender<Result<super::organization::Organization, StoreError>>,
    },
    Chats {
        reply: oneshot::Sender<Result<Vec<ChatRecord>, StoreError>>,
    },
    ReadBlob {
        sha256: String,
        offset: u64,
        len: u32,
        reply: oneshot::Sender<Result<BlobText, StoreError>>,
    },
    Page {
        chat_id: String,
        before_seq: u32,
        max_bytes: usize,
        reply: oneshot::Sender<Result<HistoryPage, StoreError>>,
    },
    LoadChat {
        chat_id: String,
        reply: oneshot::Sender<Result<Option<ChatLoad>, StoreError>>,
    },
    /// Tests: runs on the writer's store between retries (to free "disk space").
    #[cfg(test)]
    WithStore(Box<dyn FnOnce(&mut Store) + Send>),
}

/// Test-only work items.
#[cfg(test)]
pub(crate) enum TestWork {
    /// A write that panics inside its batch.
    Panic,
}

/// Which chat an item counts toward until it is committed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Attribution {
    /// A blob, a read: no chat.
    None,
    Chat(String),
    /// An op whose chat the store cannot tell (an id it never saw).
    Unknown,
}

/// What an item counts toward until it is committed or dead-lettered.
#[derive(Debug, Clone)]
struct Ticket {
    chat: Attribution,
    bytes: u64,
    /// The lowest seq of the rows it carries.
    min_seq: Option<u32>,
    blob: Option<String>,
    /// Owner entries to forget once it is committed.
    forget: Vec<(OwnerKind, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerKind {
    Turn,
    Queued,
}

/// Everything sent and not yet committed, so reads while paused know what is missing.
#[derive(Debug, Default)]
struct Pending {
    by_chat: HashMap<String, u32>,
    unknown: u32,
    bytes: u64,
    /// Per chat, the lowest row seq of each pending op that carries rows, in send order.
    first_seqs: HashMap<String, VecDeque<u32>>,
    blobs: HashMap<String, u32>,
    turns: HashMap<String, String>,
    sessions: HashMap<String, String>,
    queued: HashMap<String, String>,
}

impl Pending {
    fn ticket(&mut self, work: &Work) -> Ticket {
        let mut ticket = Ticket {
            chat: Attribution::None,
            bytes: work_bytes(work) as u64,
            min_seq: None,
            blob: None,
            forget: Vec::new(),
        };
        let Work::Op(op) = work else {
            return ticket;
        };
        let known = |map: &HashMap<String, String>, id: &str| {
            map.get(id)
                .map_or(Attribution::Unknown, |chat| Attribution::Chat(chat.clone()))
        };
        ticket.chat = match op {
            Op::InsertAgentSession(row) => {
                self.sessions.insert(row.id.clone(), row.chat_id.clone());
                Attribution::Chat(row.chat_id.clone())
            }
            Op::SaveTerminalSession { chat_id, .. } => Attribution::Chat(chat_id.clone()),
            Op::UpdateAgentSession { id, .. } | Op::SaveNativeIdentity { id, .. } => {
                known(&self.sessions, id)
            }
            Op::BeginTurn { turn, rows } => {
                self.turns.insert(turn.id.clone(), turn.chat_id.clone());
                ticket.min_seq = rows.iter().map(|row| row.seq).min();
                ticket
                    .forget
                    .push((OwnerKind::Queued, turn.prompt_id.clone()));
                Attribution::Chat(turn.chat_id.clone())
            }
            Op::AppendEvents { chat_id, rows } => {
                ticket.min_seq = rows.iter().map(|row| row.seq).min();
                Attribution::Chat(chat_id.clone())
            }
            Op::EndTurn(end) => {
                ticket.forget.push((OwnerKind::Turn, end.turn_id.clone()));
                known(&self.turns, &end.turn_id)
            }
            Op::CompactTurn { chat_id, .. }
            | Op::SetTitle { chat_id, .. }
            | Op::RecordLineEdits { chat_id, .. }
            | Op::TouchChat { chat_id, .. }
            | Op::QueueHoldAll { chat_id }
            | Op::QueueReleaseAll { chat_id } => Attribution::Chat(chat_id.clone()),
            Op::PutBlob { sha256, .. } => {
                ticket.blob = Some(sha256.clone());
                Attribution::None
            }
            Op::QueuePut(row) => {
                self.queued.insert(row.id.clone(), row.chat_id.clone());
                Attribution::Chat(row.chat_id.clone())
            }
            Op::QueueRemove { id } => {
                ticket.forget.push((OwnerKind::Queued, id.clone()));
                known(&self.queued, id)
            }
            Op::ReadPage { .. } | Op::Barrier(_) => Attribution::None,
        };
        ticket
    }

    fn add(&mut self, ticket: &Ticket) {
        self.bytes = self.bytes.saturating_add(ticket.bytes);
        match &ticket.chat {
            Attribution::Chat(chat) => {
                *self.by_chat.entry(chat.clone()).or_default() += 1;
                if let Some(seq) = ticket.min_seq {
                    self.first_seqs
                        .entry(chat.clone())
                        .or_default()
                        .push_back(seq);
                }
            }
            Attribution::Unknown => self.unknown = self.unknown.saturating_add(1),
            Attribution::None => {}
        }
        if let Some(blob) = &ticket.blob {
            *self.blobs.entry(blob.clone()).or_default() += 1;
        }
    }

    fn remove(&mut self, ticket: &Ticket) {
        self.bytes = self.bytes.saturating_sub(ticket.bytes);
        match &ticket.chat {
            Attribution::Chat(chat) => {
                if let Some(count) = self.by_chat.get_mut(chat) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        self.by_chat.remove(chat);
                    }
                }
                if let (Some(seq), Some(seqs)) = (ticket.min_seq, self.first_seqs.get_mut(chat)) {
                    if let Some(at) = seqs.iter().position(|pending| *pending == seq) {
                        seqs.remove(at);
                    }
                    if seqs.is_empty() {
                        self.first_seqs.remove(chat);
                    }
                }
            }
            Attribution::Unknown => self.unknown = self.unknown.saturating_sub(1),
            Attribution::None => {}
        }
        if let Some(blob) = &ticket.blob
            && let Some(count) = self.blobs.get_mut(blob)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.blobs.remove(blob);
            }
        }
        for (kind, id) in &ticket.forget {
            match kind {
                OwnerKind::Turn => self.turns.remove(id),
                OwnerKind::Queued => self.queued.remove(id),
            };
        }
    }

    /// Whether `chat` has ops sent and not committed (or ops the store cannot place).
    fn has_unsaved(&self, chat: &str) -> bool {
        self.unknown > 0 || self.by_chat.contains_key(chat)
    }

    /// The first unsaved row seq of `chat` below `cursor`.
    fn unsaved_from(&self, chat: &str, cursor: u32) -> Option<u32> {
        self.first_seqs
            .get(chat)
            .and_then(|seqs| seqs.iter().copied().min())
            .filter(|seq| *seq < cursor)
    }
}

/// The approximate size of an op, as the FIFO's byte bound counts it.
pub(crate) fn op_bytes(op: &Op) -> usize {
    const ROW_OVERHEAD: usize = 96;
    const SMALL: usize = 256;
    let rows = |rows: &[EventRow]| {
        rows.iter()
            .map(|row| row.payload.len().saturating_add(ROW_OVERHEAD))
            .fold(SMALL, usize::saturating_add)
    };
    match op {
        Op::BeginTurn { rows: events, .. } | Op::AppendEvents { rows: events, .. } => rows(events),
        Op::PutBlob { bytes, .. } => bytes.len().saturating_add(SMALL),
        Op::QueuePut(row) => row.text.len().saturating_add(SMALL),
        Op::SetTitle { title, .. } => title.len().saturating_add(SMALL),
        Op::RecordLineEdits { edits, .. } => edits.len().saturating_mul(16).saturating_add(SMALL),
        Op::UpdateAgentSession { patch, .. } => patch
            .config_json
            .as_ref()
            .map_or(SMALL, |json| json.len().saturating_add(SMALL))
            .saturating_add(patch.controls_json.as_ref().map_or(0, String::len)),
        Op::SaveTerminalSession { record, .. } => record.as_ref().map_or(SMALL, |record| {
            record
                .connection
                .len()
                .saturating_add(record.session.len())
                .saturating_add(SMALL)
        }),
        Op::InsertAgentSession(row) => row
            .config_json
            .as_ref()
            .map_or(SMALL, |json| json.len().saturating_add(SMALL))
            .saturating_add(row.controls_json.as_ref().map_or(0, String::len)),
        Op::EndTurn(_)
        | Op::CompactTurn { .. }
        | Op::TouchChat { .. }
        | Op::QueueRemove { .. }
        | Op::QueueHoldAll { .. }
        | Op::QueueReleaseAll { .. }
        | Op::ReadPage { .. }
        | Op::SaveNativeIdentity { .. }
        | Op::Barrier(_) => SMALL,
    }
}

fn work_bytes(work: &Work) -> usize {
    match work {
        Work::Request(Request::SharedMcp { command, .. }) => command.bytes(),
        Work::Op(op) => op_bytes(op),
        Work::Request(Request::Organization { command, .. }) => command.bytes(),
        Work::Request(Request::WorktreeSetup { command, .. }) => command.bytes(),
        Work::Request(Request::SetupAttempt { .. }) => 2048,
        Work::Request(Request::ReadSetupAttempt { .. }) => 1024,
        Work::Request(Request::FolderResolution { command, .. }) => command.bytes(),
        Work::Request(Request::AgentCatalogs { command, .. }) => command.bytes(),
        Work::Request(Request::WorkspaceRemoval { command, .. }) => command.bytes(),
        Work::Request(Request::Steering { command, .. }) => command.bytes(),
        Work::Request(Request::Composer { command, .. }) => command.bytes(),
        Work::Request(Request::Memory { command, .. }) => command.bytes(),
        Work::Request(Request::Coordinator { command, .. }) => command.bytes(),
        Work::Request(Request::Search { query, .. }) => 256 + query.len(),
        Work::Request(Request::SaveTurnText { rows, .. }) => rows
            .iter()
            .map(|row| row.text.len().saturating_add(row.message_id.len()))
            .fold(256, usize::saturating_add),
        Work::Request(_) => 256,
        #[cfg(test)]
        Work::Test(_) => 256,
    }
}

/// Whether the item answers someone, which ends a batch: it runs after the batch commits.
fn ends_batch(work: &Work) -> bool {
    match work {
        Work::Op(Op::ReadPage { .. } | Op::SaveNativeIdentity { .. } | Op::Barrier(_))
        | Work::Request(_) => true,
        Work::Op(_) => false,
        #[cfg(test)]
        Work::Test(_) => false,
    }
}

impl Shared {
    pub(crate) fn new(config: WriterConfig) -> Shared {
        Shared {
            workspace_activity: Arc::default(),
            memory_changes: tokio::sync::broadcast::Sender::new(64),
            coordinator_changes: tokio::sync::broadcast::Sender::new(256),
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                saved: VecDeque::new(),
                pending: Pending::default(),
                shutdown: None,
                waiters: Vec::new(),
                finished: None,
            }),
            wake: Condvar::new(),
            count: Arc::new(Semaphore::new(config.fifo_ops)),
            bytes: Arc::new(Semaphore::new(config.fifo_bytes)),
            status: watch::Sender::new(StorageStatus::default()),
            paused: AtomicBool::new(false),
            // From the start: startup recovery is the writer's first op.
            settling: AtomicBool::new(true),
            leftovers: tokio::sync::Notify::new(),
            index_due: AtomicBool::new(false),
            recovery_ms: AtomicU64::new(NOT_TIMED),
            indexes_ms: AtomicU64::new(NOT_TIMED),
            config,
            #[cfg(test)]
            commits: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// The writer thread died: nothing more is saved, and every waiter learns it.
    pub(crate) fn abandon(&self) {
        self.count.close();
        self.bytes.close();
        let (items, waiters) = {
            let mut queue = self.lock();
            queue.finished = Some(false);
            queue.saved.clear();
            (
                std::mem::take(&mut queue.items),
                std::mem::take(&mut queue.waiters),
            )
        };
        drop(items);
        for waiter in waiters {
            let _ = waiter.send(false);
        }
        self.paused.store(false, Ordering::Release);
        self.set_settling(false);
        self.leftovers.notify_one();
        self.status.send_replace(StorageStatus {
            state: StorageState::Failed {
                message: "Ghost stopped saving because of an internal error. Restart Ghost.".into(),
            },
            unsaved_bytes: 0,
        });
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Whether the writer is at work it knows to be long.
    pub(crate) fn is_settling(&self) -> bool {
        self.settling.load(Ordering::Acquire)
    }

    fn set_settling(&self, settling: bool) {
        self.settling.store(settling, Ordering::Release);
    }

    /// Whether the indexes recovery reads through may be built now.
    pub(crate) fn index_due(&self) -> bool {
        self.index_due.load(Ordering::Acquire)
    }

    /// Says the indexes may be built, and wakes the pass that builds them.
    pub(crate) fn set_index_due(&self) {
        self.index_due.store(true, Ordering::Release);
        self.leftovers.notify_one();
    }

    /// How long startup recovery took and, when this start built recovery's indexes, how long
    /// that took, each once it is known.
    pub(crate) fn startup_timings(&self) -> (Option<u64>, Option<u64>) {
        let timed = |slot: &AtomicU64| {
            let ms = slot.load(Ordering::Acquire);
            (ms != NOT_TIMED).then_some(ms)
        };
        (timed(&self.recovery_ms), timed(&self.indexes_ms))
    }

    /// Tests: how many items wait in the FIFO.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.lock().items.len()
    }

    /// The pause's reason, while paused.
    pub(crate) fn pause_reason(&self) -> Option<PauseReason> {
        match self.status.borrow().state {
            StorageState::Paused { reason, .. } => Some(reason),
            StorageState::Ok | StorageState::Failed { .. } => None,
        }
    }

    /// Queues work for the writer. Never blocks; after the writer finished, the work is dropped
    /// (its reply, if any, reads as a closed store) and logged.
    pub(crate) fn push(&self, work: Work, permits: Option<Permits>) {
        let mut queue = self.lock();
        if queue.finished.is_some() {
            drop(queue);
            tracing::warn!(op = %work_name(&work), "Ghost's database is closed; an op arrived after shutdown and was dropped");
            return;
        }
        let ticket = queue.pending.ticket(&work);
        queue.pending.add(&ticket);
        let unsaved = queue.pending.bytes;
        queue.items.push_back(Item {
            work,
            ticket,
            permits,
        });
        drop(queue);
        self.show_unsaved(unsaved);
        self.wake.notify_one();
    }

    /// Queues a read for the second channel.
    pub(crate) fn push_saved(&self, read: SavedRead) {
        let mut queue = self.lock();
        if queue.finished.is_some() {
            return;
        }
        queue.saved.push_back(read);
        drop(queue);
        self.wake.notify_one();
    }

    /// Asks the writer to finish by `deadline`; `done` learns whether everything was saved. After
    /// the writer finished, `done` learns it at once.
    pub(crate) fn request_shutdown(&self, deadline: Instant, done: std::sync::mpsc::Sender<bool>) {
        let mut queue = self.lock();
        if let Some(saved) = queue.finished {
            let _ = done.send(saved);
            return;
        }
        queue.shutdown = Some(match queue.shutdown {
            Some(earlier) => earlier.min(deadline),
            None => deadline,
        });
        queue.waiters.push(done);
        drop(queue);
        // New reservations fail from now on; sends that already hold room still arrive.
        self.count.close();
        self.bytes.close();
        self.wake.notify_one();
    }

    fn committed(&self, tickets: &[Ticket]) {
        let mut queue = self.lock();
        for ticket in tickets {
            queue.pending.remove(ticket);
        }
        let unsaved = queue.pending.bytes;
        drop(queue);
        self.show_unsaved(unsaved);
    }

    /// While paused, the banner shows how much is unsaved.
    fn show_unsaved(&self, unsaved: u64) {
        self.status.send_if_modified(|status| {
            matches!(status.state, StorageState::Paused { .. })
                && replace(&mut status.unsaved_bytes, unsaved)
        });
    }
}

/// Sets `slot` to `value`; returns whether it changed.
fn replace<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

fn work_name(work: &Work) -> &'static str {
    match work {
        Work::Request(Request::Flush { .. }) => "flush",
        Work::Op(op) => op_name(op),
        Work::Request(Request::CreateChat { .. }) => "create_chat",
        Work::Request(Request::SharedMcp { .. }) => "shared_mcp",
        Work::Request(Request::Organization { .. }) => "organization",
        Work::Request(Request::WorktreeSetup { .. }) => "worktree setup",
        Work::Request(Request::SetupAttempt { .. }) => "worktree setup attempt",
        Work::Request(Request::ReadSetupAttempt { .. }) => "read worktree setup attempt",
        Work::Request(Request::FolderResolution { .. }) => "folder resolution",
        Work::Request(Request::AgentCatalogs { .. }) => "agent catalogs",
        Work::Request(Request::WorkspaceRemoval { .. }) => "workspace removal",
        Work::Request(Request::Steering { .. }) => "steering",
        Work::Request(Request::Composer { .. }) => "composer",
        Work::Request(Request::Memory { .. }) => "memory",
        Work::Request(Request::Coordinator { .. }) => "coordinator",
        Work::Request(Request::CheckpointContext { .. }) => "checkpoint_context",

        Work::Request(Request::Conversation { .. }) => "conversation",
        Work::Request(Request::Chats { .. }) => "chats",
        Work::Request(Request::Search { .. }) => "search",
        Work::Request(Request::SaveLineEdits { .. }) => "save_line_edits",
        Work::Request(Request::ReadLineEdits { .. }) => "read_line_edits",
        Work::Request(Request::Leftovers { .. }) => "leftover_turns",
        Work::Request(Request::EnsureIndexes { .. }) => "ensure_indexes",
        Work::Request(Request::SaveTurnText { .. }) => "save_turn_text",
        Work::Request(Request::LoadChat { .. }) => "load_chat",
        Work::Request(Request::ExportMarkdown { .. }) => "export_markdown",
        Work::Request(Request::ReadBlob { .. }) => "read_blob",
        Work::Request(Request::Acknowledge { .. }) => "acknowledge_recovery",
        #[cfg(test)]
        Work::Test(_) => "test",
    }
}

pub(crate) fn op_name(op: &Op) -> &'static str {
    match op {
        Op::InsertAgentSession(_) => "insert_agent_session",
        Op::SaveTerminalSession { .. } => "save_terminal_session",
        Op::UpdateAgentSession { .. } => "update_agent_session",
        Op::SaveNativeIdentity { .. } => "save_native_identity",
        Op::BeginTurn { .. } => "begin_turn",
        Op::AppendEvents { .. } => "append_events",
        Op::EndTurn(_) => "end_turn",
        Op::CompactTurn { .. } => "compact_turn",
        Op::PutBlob { .. } => "put_blob",
        Op::SetTitle { .. } => "set_title",
        Op::RecordLineEdits { .. } => "record_line_edits",
        Op::TouchChat { .. } => "touch_chat",
        Op::QueuePut(_) => "queue_put",
        Op::QueueRemove { .. } => "queue_remove",
        Op::QueueHoldAll { .. } => "queue_hold_all",
        Op::QueueReleaseAll { .. } => "queue_release_all",
        Op::ReadPage { .. } => "read_page",
        Op::Barrier(_) => "barrier",
    }
}

/// Why a transaction failed.
enum Failure {
    DiskFull,
    Busy,
    /// The bytes are damaged; the error is kept to record it for the next open.
    Damage(rusqlite::Error),
    Other(String),
    Panic,
}

impl Failure {
    /// The pause a failure stalls the writer with, if it does.
    fn stall_reason(&self) -> Option<PauseReason> {
        match self {
            Failure::DiskFull => Some(PauseReason::DiskFull),
            Failure::Busy => Some(PauseReason::Busy),
            Failure::Damage(_) | Failure::Other(_) | Failure::Panic => None,
        }
    }
}

fn classify(error: rusqlite::Error) -> Failure {
    match error.sqlite_error_code() {
        Some(ErrorCode::DiskFull) => Failure::DiskFull,
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => Failure::Busy,
        _ if is_damage(&error) => Failure::Damage(error),
        _ => Failure::Other(error.to_string()),
    }
}

fn classify_io(error: &io::Error) -> Failure {
    if error.kind() == io::ErrorKind::StorageFull {
        Failure::DiskFull
    } else {
        Failure::Other(error.to_string())
    }
}

/// Items taken from the FIFO: writes for one transaction, then at most one item that answers.
struct Batch {
    writes: Vec<Item>,
    end: Option<Item>,
}

/// A stalled writer's bookkeeping.
#[derive(Debug, Clone, Copy)]
struct Stall {
    reason: PauseReason,
    since: Instant,
    backoff: Duration,
    next_retry: Instant,
    /// The pause shows in the status (at once for a full disk, after a while for a busy database).
    announced: bool,
}

impl Stall {
    fn new(reason: PauseReason, config: &WriterConfig, now: Instant) -> Stall {
        let backoff = config.busy_backoff_first;
        Stall {
            reason,
            since: now,
            backoff: (backoff * 2).min(config.busy_backoff_max),
            next_retry: now
                + match reason {
                    PauseReason::DiskFull => config.disk_full_retry,
                    PauseReason::Busy => backoff,
                },
            announced: false,
        }
    }

    /// The same failure came back on a retry.
    fn again(self, reason: PauseReason, config: &WriterConfig, now: Instant) -> Stall {
        if reason != self.reason {
            return Stall {
                announced: self.announced,
                ..Stall::new(reason, config, now)
            };
        }
        match reason {
            PauseReason::DiskFull => Stall {
                next_retry: now + config.disk_full_retry,
                ..self
            },
            PauseReason::Busy => Stall {
                next_retry: now + self.backoff,
                backoff: (self.backoff * 2).min(config.busy_backoff_max),
                ..self
            },
        }
    }

    /// Whether the pause should show by now.
    fn due(&self, config: &WriterConfig, now: Instant) -> bool {
        match self.reason {
            PauseReason::DiskFull => true,
            PauseReason::Busy => now.duration_since(self.since) >= config.busy_pause_after,
        }
    }
}

/// How the writer is doing.
enum Mode {
    Normal,
    /// Holding a failed batch, retrying it, taking no ops.
    Stalled {
        failed: Box<Batch>,
        stall: Stall,
    },
    /// The database is damaged: writes go to the dead-letter file.
    Damaged,
}

/// What wakes the writer.
enum Next {
    Saved(Vec<SavedRead>),
    Batch(Box<Batch>),
    Retry,
    Shutdown(Instant),
}

/// The thread that owns the store.
pub(crate) struct Writer {
    shared: Arc<Shared>,
    store: Option<Store>,
    owner: Option<ProfileOwner>,
    device_id: String,
    files: super::files::Files,
    recovered: watch::Sender<Option<RecoveryReport>>,
    mode: Mode,
    dead_letters: Option<File>,
}

impl Writer {
    pub(crate) fn new(
        shared: Arc<Shared>,
        store: Store,
        owner: ProfileOwner,
        device_id: String,
        files: super::files::Files,
        recovered: watch::Sender<Option<RecoveryReport>>,
    ) -> Writer {
        Writer {
            shared,
            store: Some(store),
            owner: Some(owner),
            device_id,
            files,
            recovered,
            mode: Mode::Normal,
            dead_letters: None,
        }
    }

    /// Recovery first, then work until shutdown.
    pub(crate) fn run(mut self) {
        let started = Instant::now();
        let report = self.recover_all();
        let elapsed = elapsed_ms(started);
        tracing::info!(elapsed_ms = elapsed, "startup recovery finished");
        self.shared.recovery_ms.store(elapsed, Ordering::Release);
        self.shared.set_settling(false);
        self.recovered.send_replace(Some(report));
        // The turns recovery left for after startup.
        self.shared.leftovers.notify_one();
        loop {
            match self.next() {
                Next::Saved(reads) => {
                    for read in reads {
                        self.serve_saved(read);
                    }
                }
                Next::Batch(batch) => self.run_batch(*batch),
                Next::Retry => self.retry(),
                Next::Shutdown(deadline) => {
                    self.finish(deadline);
                    // Lets the store's leftovers pass see that the store closed.
                    self.shared.leftovers.notify_one();
                    return;
                }
            }
        }
    }

    fn recover_all(&mut self) -> RecoveryReport {
        let device = self.device_id.clone();
        let limits = self.shared.config.compact;
        let Some(store) = self.store.as_mut() else {
            return RecoveryReport::default();
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            recover::recover(store.conn_mut(), Scope::All, &device, &limits, now_millis())
        }));
        match result {
            Ok(Ok(report)) => {
                if report != RecoveryReport::default() {
                    tracing::info!(?report, "recovered chats an earlier run left unfinished");
                }
                report
            }
            Ok(Err(error)) => {
                tracing::error!(%error, "startup recovery failed; the next start tries again");
                if is_damage(&error) {
                    self.damaged(&error);
                }
                RecoveryReport::default()
            }
            Err(_) => {
                tracing::error!("startup recovery panicked; the next start tries again");
                RecoveryReport::default()
            }
        }
    }

    fn next(&self) -> Next {
        let mut queue = self.shared.lock();
        loop {
            if !queue.saved.is_empty() {
                return Next::Saved(queue.saved.drain(..).collect());
            }
            if let Some(deadline) = queue.shutdown {
                return Next::Shutdown(deadline);
            }
            let retry_at = match &self.mode {
                Mode::Stalled { stall, .. } => Some(stall.next_retry),
                Mode::Normal | Mode::Damaged => None,
            };
            match retry_at {
                None if !queue.items.is_empty() => {
                    return Next::Batch(Box::new(take_batch(&mut queue, &self.shared.config)));
                }
                Some(at) if Instant::now() >= at => return Next::Retry,
                _ => {}
            }
            queue = match retry_at {
                None => self
                    .shared
                    .wake
                    .wait(queue)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                Some(at) => {
                    self.shared
                        .wake
                        .wait_timeout(queue, at.saturating_duration_since(Instant::now()))
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0
                }
            };
        }
    }

    fn run_batch(&mut self, batch: Batch) {
        if let Mode::Damaged = self.mode {
            self.dead_letter_all(batch.writes, DAMAGED);
            if let Some(end) = batch.end {
                self.answer(end);
            }
            return;
        }
        match self.commit(&batch.writes) {
            Ok(()) => {
                self.committed(&batch.writes);
                if let Some(end) = batch.end {
                    self.answer(end);
                }
            }
            Err(failure) => self.failed(batch, failure),
        }
    }

    /// Handles a batch whose transaction failed.
    fn failed(&mut self, batch: Batch, failure: Failure) {
        match failure {
            Failure::DiskFull | Failure::Busy => {
                let reason = failure.stall_reason().unwrap_or(PauseReason::DiskFull);
                let mut stall = Stall::new(reason, &self.shared.config, Instant::now());
                self.announce(&mut stall);
                self.mode = Mode::Stalled {
                    failed: Box::new(batch),
                    stall,
                };
            }
            Failure::Damage(error) => {
                self.damaged(&error);
                self.dead_letter_all(batch.writes, &error.to_string());
                if let Some(end) = batch.end {
                    self.answer(end);
                }
            }
            Failure::Other(_) | Failure::Panic => self.isolate(batch),
        }
    }

    /// Re-runs a failed batch one op per transaction; an op that fails again is dead-lettered.
    fn isolate(&mut self, batch: Batch) {
        let Batch { writes, end } = batch;
        let mut rest = writes.into_iter();
        while let Some(item) = rest.next() {
            match self.commit(std::slice::from_ref(&item)) {
                Ok(()) => self.committed(std::slice::from_ref(&item)),
                Err(failure @ (Failure::DiskFull | Failure::Busy | Failure::Damage(_))) => {
                    // The rest waits (or is dead-lettered) together with this op.
                    let mut writes = vec![item];
                    writes.extend(rest);
                    self.failed(Batch { writes, end }, failure);
                    return;
                }
                Err(Failure::Other(message)) => {
                    tracing::error!(op = %work_name(&item.work), error = %message, "an op failed twice; it goes to the dead-letter file");
                    self.dead_letter_all(vec![item], &message);
                }
                Err(Failure::Panic) => {
                    tracing::error!(op = %work_name(&item.work), "an op panicked twice; it goes to the dead-letter file");
                    self.dead_letter_all(vec![item], "the op panicked");
                }
            }
        }
        if let Some(end) = end {
            self.answer(end);
        }
    }

    fn retry(&mut self) {
        let Mode::Stalled { failed, stall } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return;
        };
        let failed = *failed;
        let outcome = if stall.reason == PauseReason::Busy {
            // A busy database is retried without SQLite's busy handler, whose wait (5 s) would hold
            // every read of the second channel behind each retry: the backoff does the waiting.
            let busy_timeout = self.swap_busy_timeout(Duration::ZERO);
            let outcome = self.commit(&failed.writes);
            if let Some(busy_timeout) = busy_timeout {
                self.swap_busy_timeout(busy_timeout);
            }
            outcome
        } else {
            self.commit(&failed.writes)
        };
        match outcome {
            Ok(()) => {
                self.resume(&stall);
                self.committed(&failed.writes);
                if let Some(end) = failed.end {
                    self.answer(end);
                }
            }
            Err(failure) => match failure.stall_reason() {
                Some(reason) => {
                    let mut stall = stall.again(reason, &self.shared.config, Instant::now());
                    self.announce(&mut stall);
                    self.mode = Mode::Stalled {
                        failed: Box::new(failed),
                        stall,
                    };
                }
                None => {
                    self.resume(&stall);
                    self.failed(failed, failure);
                }
            },
        }
    }

    /// Sets the connection's busy timeout; returns the one it replaced, or `None` (logged) when it
    /// could not be read or set.
    fn swap_busy_timeout(&self, timeout: Duration) -> Option<Duration> {
        let conn = self.store.as_ref()?.conn();
        let swapped = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .and_then(|previous| conn.busy_timeout(timeout).map(|()| previous));
        match swapped {
            Ok(previous) => Some(Duration::from_millis(
                u64::try_from(previous).unwrap_or_default(),
            )),
            Err(error) => {
                tracing::warn!(%error, "could not change the database's busy timeout");
                None
            }
        }
    }

    /// Shows the pause once it is due.
    fn announce(&self, stall: &mut Stall) {
        if !stall.due(&self.shared.config, Instant::now()) {
            return;
        }
        let unsaved = self.shared.lock().pending.bytes;
        let fresh = !stall.announced;
        stall.announced = true;
        let reason = stall.reason;
        self.shared.paused.store(true, Ordering::Release);
        self.shared.status.send_if_modified(|status| {
            let since = match status.state {
                StorageState::Paused { since, .. } if !fresh => since,
                _ => now_millis(),
            };
            replace(
                status,
                StorageStatus {
                    state: StorageState::Paused { reason, since },
                    unsaved_bytes: unsaved,
                },
            )
        });
        if fresh {
            tracing::warn!(
                ?reason,
                "saving paused; the failed batch is kept and retried"
            );
        }
    }

    /// Saving works again.
    fn resume(&self, stall: &Stall) {
        if stall.announced {
            self.shared.paused.store(false, Ordering::Release);
            self.shared.status.send_replace(StorageStatus::default());
            tracing::info!(reason = ?stall.reason, "saving resumed");
        }
    }

    /// Records damage for the next open's repair; from now on writes go to the dead-letter file.
    fn damaged(&mut self, error: &rusqlite::Error) {
        if let Some(store) = &self.store
            && let Err(note) = store.note_query_error(error)
        {
            tracing::warn!(error = %note, "could not record the damage for the next open");
        }
        if !matches!(self.mode, Mode::Damaged) {
            tracing::error!(%error, "the database is damaged; later writes go to the dead-letter file");
            self.mode = Mode::Damaged;
            self.shared.paused.store(false, Ordering::Release);
            self.shared.status.send_replace(StorageStatus {
                state: StorageState::Failed {
                    message: DAMAGED.to_owned(),
                },
                unsaved_bytes: 0,
            });
        }
    }

    /// Runs `writes` in one transaction.
    fn commit(&mut self, writes: &[Item]) -> Result<(), Failure> {
        if writes.is_empty() {
            return Ok(());
        }
        let Some(store) = self.store.as_mut() else {
            return Err(Failure::Other("Ghost's database is closed".into()));
        };
        let result = commit(
            store,
            writes,
            &self.device_id,
            &self.files.attachments,
            &self.shared.config.compact,
        );
        #[cfg(test)]
        if result.is_ok() {
            self.shared.commits.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    fn committed(&self, items: &[Item]) {
        let tickets: Vec<Ticket> = items.iter().map(|item| item.ticket.clone()).collect();
        self.shared.committed(&tickets);
        // A turn too large to merge was left where it is (`apply`): the store's pass takes it.
        if items
            .iter()
            .any(|item| matches!(item.work, Work::Op(Op::CompactTurn { .. })))
        {
            self.shared.leftovers.notify_one();
        }
    }

    /// Runs a read on the connection, outside any transaction.
    fn read<T>(
        &mut self,
        read: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> Result<T, StoreError> {
        let Some(store) = self.store.as_ref() else {
            return Err(StoreError::Closed);
        };
        let conn = store.conn();
        match catch_unwind(AssertUnwindSafe(|| read(conn))) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(self.read_error(error)),
            Err(_) => Err(StoreError::Other(
                "a database read failed inside Ghost".into(),
            )),
        }
    }

    fn read_error(&mut self, error: rusqlite::Error) -> StoreError {
        match classify(error) {
            Failure::DiskFull => StoreError::DiskFull,
            Failure::Busy => StoreError::Busy,
            Failure::Damage(error) => {
                self.damaged(&error);
                StoreError::Damaged(error.to_string())
            }
            Failure::Other(message) => StoreError::Other(message),
            Failure::Panic => StoreError::Other("a database read failed inside Ghost".into()),
        }
    }

    /// Answers an item that ended a batch, after the batch committed.
    fn answer(&mut self, item: Item) {
        let Item { work, ticket, .. } = item;
        match work {
            Work::Op(Op::ReadPage {
                chat_id,
                before_seq,
                max_bytes,
                reply,
            }) => {
                let page = self.read(|conn| history::page(conn, &chat_id, before_seq, max_bytes));
                let _ = reply.send(page);
            }
            Work::Op(Op::Barrier(reply)) => {
                let _ = reply.send(());
            }
            Work::Op(Op::SaveNativeIdentity {
                id,
                native_id,
                updated_at,
                reply,
            }) => {
                let device = self.device_id.clone();
                let result = if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.to_owned()))
                } else {
                    self.write_tx(|conn| {
                        rows::save_native_identity(conn, &device, &id, &native_id, updated_at)
                    })
                };
                let _ = reply.send(result);
            }
            Work::Request(request) => self.answer_request(request),
            Work::Op(_) => {}
            #[cfg(test)]
            Work::Test(_) => {}
        }
        self.shared.committed(&[ticket]);
    }

    fn answer_request(&mut self, request: Request) {
        match request {
            Request::Steering { command, reply } => {
                let now = now_millis();
                let result = if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| super::steering::execute(conn, now, command))
                };
                let _ = reply.send(result);
            }
            Request::Flush { reply } => {
                let result = match self.mode {
                    Mode::Normal if self.store.is_some() => Ok(()),
                    Mode::Damaged => Err(StoreError::Damaged(DAMAGED.into())),
                    _ => Err(StoreError::Closed),
                };
                let _ = reply.send(result);
            }
            Request::WorktreeSetup { command, reply } => {
                let device = self.device_id.clone();
                let now = now_millis();
                let result = if command.save.is_none() {
                    self.read(|conn| super::worktree_setup::execute(conn, &device, now, command))
                } else if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| {
                        super::worktree_setup::execute(conn, &device, now, command)
                    })
                };
                let _ = reply.send(result);
            }
            Request::SetupAttempt { command, reply } => {
                let device = self.device_id.clone();
                let result = if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| super::setup_attempt::execute(conn, &device, command))
                };
                let _ = reply.send(result);
            }
            Request::SharedMcp { command, reply } => {
                let writes = command.writes();
                let execute = |conn: &rusqlite::Connection| {
                    crate::mcp::execute(conn, command)
                        .map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))
                };
                let result = if !writes {
                    self.read(execute)
                } else if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(execute)
                };
                let _ = reply.send(result);
            }
            Request::ReadSetupAttempt { chat_id, reply } => {
                let device = self.device_id.clone();
                let result = self.read(|conn| super::setup_attempt::read(conn, &device, &chat_id));
                let _ = reply.send(result);
            }
            Request::WorkspaceRemoval { command, reply } => {
                let activity = Arc::clone(&self.shared.workspace_activity);
                let mut active = activity.lock();
                let device = self.device_id.clone();
                let now = now_millis();
                let result = if !command.writes() {
                    self.read(|conn| super::workspace_removal::execute(conn, &device, now, command))
                } else if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| {
                        super::workspace_removal::execute(conn, &device, now, command)
                    })
                };
                if let Some(store) = &self.store
                    && super::workspace_activity::refresh(store.conn(), &mut active)
                {
                    activity.changed();
                }
                let _ = reply.send(result);
            }
            Request::FolderResolution { command, reply } => {
                let device = self.device_id.clone();
                let now = now_millis();
                let result = if !command.writes() {
                    self.read(|conn| super::folder_resolution::execute(conn, &device, now, command))
                } else if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| {
                        super::folder_resolution::execute(conn, &device, now, command)
                    })
                };
                let _ = reply.send(result);
            }
            Request::AgentCatalogs { command, reply } => {
                let device = self.device_id.clone();
                let now = now_millis();
                let result = if !command.writes() {
                    self.read(|conn| super::agent_catalogs::execute(conn, &device, now, command))
                } else if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| {
                        super::agent_catalogs::execute(conn, &device, now, command)
                    })
                };
                let _ = reply.send(result);
            }
            Request::Conversation { command, reply } => {
                let device = self.device_id.clone();
                let now = now_millis();
                let result = if !command.writes() {
                    self.read(|conn| super::conversation::execute(conn, &device, now, command))
                } else if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| super::conversation::execute(conn, &device, now, command))
                };
                let _ = reply.send(result);
            }
            Request::Search {
                query,
                include_archived,
                reply,
            } => {
                let _ = reply
                    .send(self.read(|conn| super::search::execute(conn, &query, include_archived)));
            }
            Request::SaveLineEdits { counted, reply } => {
                let now = now_millis();
                let result = if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| super::line_edits::save(conn, &counted, now))
                };
                let _ = reply.send(result);
            }
            Request::ReadLineEdits { since, reply } => {
                let _ = reply.send(self.read(|conn| super::line_edits::read(conn, since)));
            }
            Request::Leftovers { reply } => {
                let _ = reply.send(self.read(compact::leftovers));
            }
            Request::EnsureIndexes { reply } => {
                let result = if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    // One pass over every event for each missing index: callers wait it out.
                    self.shared.set_settling(true);
                    let started = Instant::now();
                    let result = self.read(recover::ensure_indexes);
                    self.shared.set_settling(false);
                    if let Ok(built @ 1..) = result {
                        let elapsed = elapsed_ms(started);
                        tracing::info!(
                            built,
                            elapsed_ms = elapsed,
                            "built the indexes recovery reads through"
                        );
                        self.shared.indexes_ms.store(elapsed, Ordering::Release);
                    }
                    result
                };
                let _ = reply.send(result);
            }
            Request::SaveTurnText {
                chat_id,
                turn_id,
                rows,
                reply,
            } => {
                let now = now_millis();
                let result = if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| compact::save_text(conn, &chat_id, &turn_id, &rows, now))
                };
                let _ = reply.send(result);
            }
            Request::Memory { command, reply } => {
                let now = now_millis();
                let result = if command.read_only() {
                    self.read(|conn| super::memory::execute(conn, command, now))
                } else if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    // Committed before it is announced, so an open editor can re-read, even if
                    // this caller stopped waiting.
                    self.write_tx(|conn| super::memory::execute_tracked(conn, command, now))
                        .map(|(reply, changed)| {
                            if let Some(changed) = changed {
                                let _ = self.shared.memory_changes.send(changed);
                            }
                            reply
                        })
                };
                let _ = reply.send(result);
            }
            Request::Coordinator { command, reply } => {
                let now = now_millis();
                let result = if command.read_only() {
                    self.read(|conn| super::coordinator::execute(conn, command, now))
                } else if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    // Committed before it is announced, so an open project page can re-read.
                    self.write_tx(|conn| super::coordinator::execute_tracked(conn, command, now))
                        .map(|(reply, changed)| {
                            if let Some(changed) = changed {
                                let _ = self.shared.coordinator_changes.send(changed);
                            }
                            reply
                        })
                };
                let _ = reply.send(result);
            }
            Request::Composer { command, reply } => {
                let dir = self.files.attachments.clone();
                let now = now_millis();
                let result = if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| {
                        super::composer::execute(conn, &dir, now, command).map_err(|error| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                        })
                    })
                };
                let _ = reply.send(result);
            }
            Request::CheckpointContext {
                chat_id,
                turn_id,
                reply,
            } => {
                let device = self.device_id.clone();
                let _ = reply.send(self.read(|conn| {
                    rows::checkpoint_context(conn, &device, &chat_id, turn_id.as_deref())
                }));
            }
            Request::Organization { command, reply } => {
                let device = self.device_id.clone();
                let now = now_millis();
                let result = if command.is_read() {
                    self.read(|conn| super::organization::execute(conn, &device, now, command))
                } else if matches!(self.mode, Mode::Damaged) {
                    Err(StoreError::Damaged(DAMAGED.into()))
                } else {
                    self.write_tx(|conn| super::organization::execute(conn, &device, now, command))
                };
                let _ = reply.send(result);
            }
            Request::CreateChat { new, reply } => {
                let created = if let Mode::Damaged = self.mode {
                    Err(StoreError::Damaged(DAMAGED.to_owned()))
                } else {
                    self.write_tx(|conn| rows::create_chat(conn, &new))
                };
                let _ = reply.send(created);
            }
            Request::Chats { reply } => {
                let device = self.device_id.clone();
                let _ = reply.send(self.read(|conn| rows::chats(conn, &device)));
            }
            Request::LoadChat { chat_id, reply } => {
                let _ = reply.send(self.load_chat(&chat_id));
            }
            Request::ExportMarkdown { chat_id, reply } => {
                let _ = reply.send(self.read(|conn| super::export::markdown(conn, &chat_id)));
            }
            Request::ReadBlob {
                sha256,
                offset,
                len,
                reply,
            } => {
                let _ = reply.send(self.read_blob(&sha256, offset, len));
            }
            Request::Acknowledge { quarantined, reply } => {
                let acknowledged = match &self.store {
                    Some(store) => store.acknowledge_recovery(&quarantined),
                    None => Err(CoreError::Io {
                        path: self.files.backups.clone(),
                        source: io::Error::other("Ghost's database is closed"),
                    }),
                };
                let _ = reply.send(acknowledged);
            }
        }
    }

    /// One write in its own transaction, for a call that answers.
    fn write_tx<T>(
        &mut self,
        write: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> Result<T, StoreError> {
        let Some(store) = self.store.as_mut() else {
            return Err(StoreError::Closed);
        };
        let conn = store.conn_mut();
        let outcome = catch_unwind(AssertUnwindSafe(|| -> rusqlite::Result<T> {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = write(&tx)?;
            tx.commit()?;
            Ok(value)
        }));
        settle(conn);
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(self.read_error(error)),
            Err(_) => Err(StoreError::Other(
                "a database write failed inside Ghost".into(),
            )),
        }
    }

    /// Per-chat recovery (for an actor that died), then everything an actor needs. `None`: no such
    /// chat.
    fn load_chat(&mut self, chat_id: &str) -> Result<Option<ChatLoad>, StoreError> {
        let mut recovered = 0;
        if !matches!(self.mode, Mode::Damaged) {
            let device = self.device_id.clone();
            let limits = self.shared.config.compact;
            let Some(store) = self.store.as_mut() else {
                return Err(StoreError::Closed);
            };
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                recover::recover(
                    store.conn_mut(),
                    Scope::Chat(chat_id),
                    &device,
                    &limits,
                    now_millis(),
                )
            }));
            settle(store.conn_mut());
            match outcome {
                Ok(Ok(report)) => {
                    recovered = report.interrupted_turns;
                    if recovered > 0 {
                        // A turn this ended may be too large to compact here.
                        self.shared.leftovers.notify_one();
                    }
                }
                Ok(Err(error)) => return Err(self.read_error(error)),
                Err(_) => {
                    return Err(StoreError::Other(
                        "recovering this chat failed inside Ghost".into(),
                    ));
                }
            }
        }
        let device = self.device_id.clone();
        let activity = Arc::clone(&self.shared.workspace_activity);
        let mut load = self.read(|conn| {
            activity.loaded(conn, chat_id)?;
            rows::load_chat(conn, &device, chat_id, recovered)
        })?;
        if let Some(load) = &mut load {
            // Every row queued before the actor that loads it waits for the user (decision 11):
            // recovery holds them, and when it could not (a damaged database, a chat whose recovery
            // failed) they read as held all the same.
            for row in &mut load.queue {
                row.held = true;
            }
            let mut queue = self.shared.lock();
            if let Some(session) = &load.agent_session {
                queue
                    .pending
                    .sessions
                    .insert(session.id.clone(), session.chat_id.clone());
            }
            for row in &load.queue {
                queue
                    .pending
                    .queued
                    .insert(row.id.clone(), row.chat_id.clone());
            }
        }
        Ok(load)
    }

    fn read_blob(&mut self, sha256: &str, offset: u64, len: u32) -> Result<BlobText, StoreError> {
        match self.files.attachments.read_page(sha256, offset, len) {
            Ok(text) => Ok(text),
            Err(blobs::ReadError::Gone) => Err(StoreError::BlobMissing(BLOB_GONE.to_owned())),
            Err(blobs::ReadError::Io(error)) => Err(StoreError::Other(error.to_string())),
        }
    }

    /// Serves a read from saved data.
    fn serve_saved(&mut self, read: SavedRead) {
        match read {
            SavedRead::Search {
                query,
                include_archived,
                reply,
            } => {
                let _ = reply
                    .send(self.read(|conn| super::search::execute(conn, &query, include_archived)));
            }
            SavedRead::CheckpointContext {
                chat_id,
                turn_id,
                reply,
            } => {
                let device = self.device_id.clone();
                let _ = reply.send(self.read(|conn| {
                    rows::checkpoint_context(conn, &device, &chat_id, turn_id.as_deref())
                }));
            }
            SavedRead::Organization { filter, reply } => {
                let device = self.device_id.clone();
                let _ = reply.send(self.read(|conn| {
                    super::organization::execute(
                        conn,
                        &device,
                        now_millis(),
                        super::organization::OrganizationCommand::Snapshot(filter),
                    )
                }));
            }
            SavedRead::Chats { reply } => {
                let device = self.device_id.clone();
                let _ = reply.send(self.read(|conn| rows::chats(conn, &device)));
            }
            SavedRead::ReadBlob {
                sha256,
                offset,
                len,
                reply,
            } => {
                let unsaved = self.shared.lock().pending.blobs.contains_key(&sha256);
                let text = if unsaved {
                    Err(StoreError::BlobMissing(BLOB_UNSAVED.to_owned()))
                } else {
                    self.read_blob(&sha256, offset, len)
                };
                let _ = reply.send(text);
            }
            SavedRead::Page {
                chat_id,
                before_seq,
                max_bytes,
                reply,
            } => {
                let page = self
                    .read(|conn| history::page(conn, &chat_id, before_seq, max_bytes))
                    .map(|mut page| {
                        page.unsaved_from = self
                            .shared
                            .lock()
                            .pending
                            .unsaved_from(&chat_id, before_seq);
                        page
                    });
                let _ = reply.send(page);
            }
            SavedRead::LoadChat { chat_id, reply } => {
                let _ = reply.send(self.load_saved_chat(&chat_id));
            }
            #[cfg(test)]
            SavedRead::WithStore(run) => {
                if let Some(store) = self.store.as_mut() {
                    run(store);
                }
            }
        }
    }

    /// `load_chat` from saved data, for a chat with no unsaved ops and no running turn; recovery
    /// would write, so the queue just reads as held.
    fn load_saved_chat(&mut self, chat_id: &str) -> Result<Option<ChatLoad>, StoreError> {
        let paused = match self.shared.pause_reason() {
            Some(PauseReason::Busy) => StoreError::Busy,
            _ => StoreError::DiskFull,
        };
        if self.shared.lock().pending.has_unsaved(chat_id) {
            return Err(paused);
        }
        let running = self.read(|conn| {
            conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM turns WHERE chat_id = ?1 AND status = 'running')",
                [chat_id],
                |row| row.get::<_, bool>(0),
            )
        })?;
        if running {
            return Err(paused);
        }
        let device = self.device_id.clone();
        let activity = Arc::clone(&self.shared.workspace_activity);
        let load = self.read(|conn| {
            activity.loaded(conn, chat_id)?;
            rows::load_chat(conn, &device, chat_id, 0)
        })?;
        Ok(load.map(|mut load| {
            for row in &mut load.queue {
                row.held = true;
            }
            load
        }))
    }

    /// Commits what is queued until `deadline`, closes the connection (folding the WAL back), and
    /// tells every waiter whether everything was saved.
    fn finish(&mut self, deadline: Instant) {
        loop {
            let saved: Vec<SavedRead> = self.shared.lock().saved.drain(..).collect();
            for read in saved {
                self.serve_saved(read);
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            if let Mode::Stalled { stall, .. } = &self.mode {
                if stall.next_retry > now {
                    std::thread::sleep(
                        stall
                            .next_retry
                            .min(deadline)
                            .saturating_duration_since(now),
                    );
                    continue;
                }
                self.retry();
                continue;
            }
            let batch = {
                let mut queue = self.shared.lock();
                if queue.items.is_empty() {
                    None
                } else {
                    Some(take_batch(&mut queue, &self.shared.config))
                }
            };
            match batch {
                Some(batch) => self.run_batch(batch),
                None => break,
            }
        }
        let mut dropped: Vec<Item> = self.shared.lock().items.drain(..).collect();
        if let Mode::Stalled { failed, .. } = std::mem::replace(&mut self.mode, Mode::Normal) {
            let Batch { writes, end } = *failed;
            let held: Vec<Item> = writes.into_iter().chain(end).collect();
            dropped.splice(0..0, held);
        }
        let mut lost: HashMap<String, u32> = HashMap::new();
        for item in &dropped {
            if ends_batch(&item.work) {
                continue;
            }
            let chat = match &item.ticket.chat {
                Attribution::Chat(chat) => chat.clone(),
                Attribution::None => "(no chat)".to_owned(),
                Attribution::Unknown => "(unknown chat)".to_owned(),
            };
            *lost.entry(chat).or_default() += 1;
        }
        let saved_everything = lost.is_empty();
        if !saved_everything {
            tracing::error!(
                ?lost,
                "Ghost quit before it could save these ops (counts per chat); recovery ends their turns at the next start"
            );
        }
        drop(dropped);
        // Closing the connection checkpoints the WAL and removes it, so the next launch sees a clean
        // shutdown; the owner lock goes after the store.
        drop(self.store.take());
        drop(self.owner.take());
        let waiters = {
            let mut queue = self.shared.lock();
            queue.finished = Some(saved_everything);
            queue.saved.clear();
            std::mem::take(&mut queue.waiters)
        };
        for waiter in waiters {
            let _ = waiter.send(saved_everything);
        }
    }

    /// Writes ops to the dead-letter file, so nothing is silently lost.
    fn dead_letter_all(&mut self, items: Vec<Item>, error: &str) {
        if items.is_empty() {
            return;
        }
        let at = now_millis();
        let lines: Vec<String> = items
            .iter()
            .filter_map(|item| dead_letter_line(item, error, at))
            .collect();
        match self.dead_letter_file() {
            Ok(file) => {
                let written = lines
                    .iter()
                    .try_for_each(|line| {
                        file.write_all(line.as_bytes())
                            .and_then(|()| file.write_all(b"\n"))
                    })
                    .and_then(|()| file.sync_data());
                if let Err(error) = written {
                    tracing::error!(%error, count = lines.len(), "could not write ops to the dead-letter file; they are lost");
                }
            }
            Err(error) => {
                tracing::error!(%error, count = lines.len(), "could not open the dead-letter file; these ops are lost");
            }
        }
        self.committed(&items);
    }

    fn dead_letter_file(&mut self) -> io::Result<&mut File> {
        if self.dead_letters.is_none() {
            fsutil::create_private_dir_all(&self.files.backups).map_err(io::Error::other)?;
            let path = fsutil::free_path(
                &self.files.backups,
                &format!("deadletter-{}", now_millis()),
                ".jsonl",
            );
            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(&path)?;
            tracing::warn!(path = %path.display(), "writing ops the database refused to the dead-letter file");
            self.dead_letters = Some(file);
        }
        self.dead_letters
            .as_mut()
            .ok_or_else(|| io::Error::other("the dead-letter file is not open"))
    }
}

/// Takes the next batch: writes up to the limits, ending at the first item that answers.
fn take_batch(queue: &mut Queue, config: &WriterConfig) -> Batch {
    let mut writes = Vec::new();
    let mut bytes = 0usize;
    while let Some(front) = queue.items.front() {
        let size = usize::try_from(front.ticket.bytes).unwrap_or(usize::MAX);
        if !writes.is_empty()
            && (writes.len() >= config.batch_ops || bytes.saturating_add(size) > config.batch_bytes)
        {
            break;
        }
        let Some(mut item) = queue.items.pop_front() else {
            break;
        };
        // Out of the FIFO: its room goes back to the senders.
        item.permits = None;
        if ends_batch(&item.work) {
            return Batch {
                writes,
                end: Some(item),
            };
        }
        bytes = bytes.saturating_add(size);
        writes.push(item);
    }
    Batch { writes, end: None }
}

/// Runs `writes` in one `BEGIN IMMEDIATE` transaction; a panic rolls it back like an error.
fn commit(
    store: &mut Store,
    writes: &[Item],
    device_id: &str,
    attachments: &super::files::Attachments,
    limits: &CompactLimits,
) -> Result<(), Failure> {
    let conn = store.conn_mut();
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<(), Failure> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(classify)?;
        let now = now_millis();
        for item in writes {
            apply(&tx, item, device_id, attachments, limits, now)?;
        }
        tx.commit().map_err(classify)
    }));
    settle(conn);
    outcome.unwrap_or(Err(Failure::Panic))
}

/// After a failed or panicked transaction, makes sure none is left open (SQLite rolls some back on
/// its own, such as after SQLITE_FULL).
fn settle(conn: &mut rusqlite::Connection) {
    if !conn.is_autocommit()
        && let Err(error) = conn.execute_batch("ROLLBACK")
    {
        tracing::warn!(%error, "could not roll back a failed transaction");
    }
}

/// Applies one write op in the caller's transaction.
fn apply(
    conn: &rusqlite::Connection,
    item: &Item,
    device_id: &str,
    attachments: &super::files::Attachments,
    limits: &CompactLimits,
    now: i64,
) -> Result<(), Failure> {
    let op = match &item.work {
        Work::Op(op) => op,
        Work::Request(_) => return Ok(()),
        #[cfg(test)]
        Work::Test(TestWork::Panic) => panic!("a test op panicked inside its batch"),
    };
    let sql = |result: rusqlite::Result<()>| result.map_err(classify);
    match op {
        Op::InsertAgentSession(row) => sql(rows::insert_agent_session(conn, device_id, row)),
        Op::SaveTerminalSession { chat_id, record } => sql(rows::save_terminal_session(
            conn,
            device_id,
            chat_id,
            record.as_ref(),
        )),
        Op::UpdateAgentSession { id, patch } => {
            sql(rows::update_agent_session(conn, device_id, id, patch))
        }
        Op::BeginTurn { turn, rows: events } => {
            sql(rows::begin_turn(conn, device_id, turn, events))
        }
        Op::AppendEvents {
            chat_id,
            rows: events,
        } => sql(rows::append_events(conn, device_id, chat_id, events)),
        Op::EndTurn(end) => sql(rows::end_turn(conn, end)),
        Op::CompactTurn { chat_id, turn_id } => {
            // A turn too large to merge is only read for its text, all of it: the store does that
            // on a connection of its own (its leftovers pass), so nothing waits behind it here.
            if compact::turn_bytes(conn, turn_id).map_err(classify)? > limits.merge_limit {
                return Ok(());
            }
            compact::apply(conn, chat_id, turn_id, device_id, limits, now)
                .map(|_| ())
                .map_err(classify)
        }
        Op::PutBlob {
            sha256,
            mime,
            bytes,
        } => {
            let size = attachments
                .write(sha256, bytes)
                .map_err(|error| classify_io(&error))?;
            sql(rows::insert_attachment(conn, sha256, mime, size, now))
        }
        Op::SetTitle {
            chat_id,
            title,
            source,
        } => sql(rows::set_title(conn, chat_id, title, *source)),
        Op::RecordLineEdits { turn_id, edits, .. } => {
            sql(super::line_edits::record(conn, turn_id, edits, now))
        }
        Op::TouchChat { chat_id, at } => sql(rows::touch_chat(conn, chat_id, *at)),
        Op::QueuePut(row) => sql(rows::queue_put(conn, device_id, row)),
        Op::QueueRemove { id } => sql(rows::queue_remove(conn, id)),
        Op::QueueHoldAll { chat_id } => {
            sql(rows::queue_hold(conn, Some(chat_id), true).map(|_| ()))
        }
        Op::QueueReleaseAll { chat_id } => {
            sql(rows::queue_hold(conn, Some(chat_id), false).map(|_| ()))
        }
        Op::ReadPage { .. } | Op::SaveNativeIdentity { .. } | Op::Barrier(_) => Ok(()),
    }
}

/// An op as the dead-letter file records it: everything needed to replay it by hand.
#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum DeadOp<'a> {
    InsertAgentSession {
        row: &'a AgentSessionRow,
    },
    SaveTerminalSession {
        chat_id: &'a str,
        record: Option<&'a super::TerminalSessionRecord>,
    },
    UpdateAgentSession {
        id: &'a str,
        patch: &'a AgentSessionPatch,
    },
    BeginTurn {
        turn: &'a TurnRow,
        rows: &'a [EventRow],
    },
    AppendEvents {
        chat_id: &'a str,
        rows: &'a [EventRow],
    },
    EndTurn {
        end: &'a TurnEnd,
    },
    CompactTurn {
        chat_id: &'a str,
        turn_id: &'a str,
    },
    PutBlob {
        sha256: &'a str,
        mime: &'a str,
        size: usize,
        text: std::borrow::Cow<'a, str>,
    },
    SetTitle {
        chat_id: &'a str,
        title: &'a str,
        source: TitleSource,
    },
    TouchChat {
        chat_id: &'a str,
        at: i64,
    },
    RecordLineEdits {
        turn_id: &'a str,
        edits: &'a [(i64, u64)],
    },
    QueuePut {
        row: &'a QueuedRow,
    },
    QueueRemove {
        id: &'a str,
    },
    QueueHoldAll {
        chat_id: &'a str,
    },
    QueueReleaseAll {
        chat_id: &'a str,
    },
    #[cfg(test)]
    TestPanic,
}

#[derive(Serialize)]
struct DeadLetter<'a> {
    at: i64,
    error: &'a str,
    op: DeadOp<'a>,
}

fn dead_letter_line(item: &Item, error: &str, at: i64) -> Option<String> {
    serde_json::to_string(&dead_letter(item, error, at)?).ok()
}

fn dead_letter<'a>(item: &'a Item, error: &'a str, at: i64) -> Option<DeadLetter<'a>> {
    let op = match &item.work {
        Work::Op(op) => match op {
            Op::InsertAgentSession(row) => DeadOp::InsertAgentSession { row },
            Op::SaveTerminalSession { chat_id, record } => DeadOp::SaveTerminalSession {
                chat_id,
                record: record.as_ref(),
            },
            Op::UpdateAgentSession { id, patch } => DeadOp::UpdateAgentSession { id, patch },
            Op::BeginTurn { turn, rows } => DeadOp::BeginTurn { turn, rows },
            Op::AppendEvents { chat_id, rows } => DeadOp::AppendEvents { chat_id, rows },
            Op::EndTurn(end) => DeadOp::EndTurn { end },
            Op::CompactTurn { chat_id, turn_id } => DeadOp::CompactTurn { chat_id, turn_id },
            Op::PutBlob {
                sha256,
                mime,
                bytes,
            } => DeadOp::PutBlob {
                sha256,
                mime,
                size: bytes.len(),
                text: String::from_utf8_lossy(bytes),
            },
            Op::SetTitle {
                chat_id,
                title,
                source,
            } => DeadOp::SetTitle {
                chat_id,
                title,
                source: *source,
            },
            Op::TouchChat { chat_id, at } => DeadOp::TouchChat { chat_id, at: *at },
            Op::RecordLineEdits { turn_id, edits, .. } => {
                DeadOp::RecordLineEdits { turn_id, edits }
            }
            Op::QueuePut(row) => DeadOp::QueuePut { row },
            Op::QueueRemove { id } => DeadOp::QueueRemove { id },
            Op::QueueHoldAll { chat_id } => DeadOp::QueueHoldAll { chat_id },
            Op::QueueReleaseAll { chat_id } => DeadOp::QueueReleaseAll { chat_id },
            Op::ReadPage { .. } | Op::SaveNativeIdentity { .. } | Op::Barrier(_) => return None,
        },
        Work::Request(_) => return None,
        #[cfg(test)]
        Work::Test(TestWork::Panic) => DeadOp::TestPanic,
    };
    Some(DeadLetter { at, error, op })
}

/// Why waiting for the writer's answer ended without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitError {
    /// No answer within the bound.
    TimedOut,
    /// The writer dropped the reply (it finished, or the store closed).
    Gone,
}

/// Waits for the writer's answer, at most `limit`. The wait runs on tokio's blocking pool: a real
/// thread works meanwhile, and a blocking task keeps a paused test clock from jumping ahead, so a
/// bound never fires early in tests on paused time.
pub(crate) async fn await_reply<T: Send + 'static>(
    rt: &Handle,
    reply: oneshot::Receiver<T>,
    limit: Duration,
) -> Result<T, WaitError> {
    await_reply_while(rt, reply, limit, limit, || false).await
}

/// How often a wait that outlasted its bound asks again whether to go on.
const WAIT_STEP: Duration = Duration::from_millis(250);

/// [`await_reply`], going on past `limit` while `go_on` says so (asked when `limit` passes and
/// every [`WAIT_STEP`] after), for at most `longest` in all.
pub(crate) async fn await_reply_while<T: Send + 'static>(
    rt: &Handle,
    reply: oneshot::Receiver<T>,
    limit: Duration,
    longest: Duration,
    go_on: impl Fn() -> bool,
) -> Result<T, WaitError> {
    let mut task = rt.spawn_blocking(move || reply.blocking_recv());
    let started = tokio::time::Instant::now();
    let mut bound = limit;
    loop {
        match tokio::time::timeout(bound, &mut task).await {
            Ok(Ok(Ok(value))) => return Ok(value),
            Ok(Ok(Err(_)) | Err(_)) => return Err(WaitError::Gone),
            Err(_) if go_on() && started.elapsed() < longest => bound = WAIT_STEP,
            Err(_) => return Err(WaitError::TimedOut),
        }
    }
}

/// Waits for a shutdown's result from any thread, with or without a runtime.
pub(crate) fn wait_finished(done: &std::sync::mpsc::Receiver<bool>, limit: Duration) -> bool {
    done.recv_timeout(limit).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::paths::DataRoot;
    use crate::profile::Profiles;

    /// A caller waits past its bound only while it is told to, and never past the longest wait.
    #[tokio::test]
    async fn a_wait_goes_on_past_its_bound_only_while_asked_to() {
        let rt = Handle::current();
        let late = |after: Duration| {
            let (reply, answer) = oneshot::channel();
            std::thread::spawn(move || {
                std::thread::sleep(after);
                let _ = reply.send(7);
            });
            answer
        };
        let short = Duration::from_millis(20);
        let long = Duration::from_secs(30);
        assert_eq!(
            await_reply_while(&rt, late(Duration::from_millis(400)), short, long, || true).await,
            Ok(7)
        );
        assert_eq!(
            await_reply_while(&rt, late(Duration::from_millis(400)), short, long, || false).await,
            Err(WaitError::TimedOut)
        );
        assert_eq!(
            await_reply_while(
                &rt,
                late(Duration::from_secs(2)),
                short,
                Duration::from_millis(300),
                || true
            )
            .await,
            Err(WaitError::TimedOut),
            "the longest wait still ends it"
        );
        assert_eq!(
            await_reply(&rt, late(Duration::from_millis(400)), short).await,
            Err(WaitError::TimedOut)
        );
        let (reply, answer) = oneshot::channel::<u32>();
        drop(reply);
        assert_eq!(
            await_reply_while(&rt, answer, short, long, || true).await,
            Err(WaitError::Gone)
        );
    }

    /// Decision 11 when recovery cannot run: in a damaged database `load_chat` skips recovery, and
    /// the rows queued before the crash still come back held, so a start sends none of them.
    #[test]
    fn a_load_without_recovery_still_holds_the_queue() {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = Profiles::new(DataRoot::at(dir.path().join("data")))
            .open_active()
            .expect("profile");
        let owner = ProfileOwner::acquire(&profile).expect("owner");
        let (store, _) = Store::open(&profile).expect("store");
        let chat = rows::create_chat(
            store.conn(),
            &NewChat {
                agent_host: Default::default(),
                agent_id: "mock".into(),
                folder: PathBuf::from("/work"),
                device_id: "dev".into(),
                now: 1,
            },
        )
        .expect("chat");
        rows::queue_put(
            store.conn(),
            "dev",
            &QueuedRow {
                parts: Vec::new(),
                id: "q".into(),
                chat_id: chat.id.clone(),
                text: "typed before the crash".into(),
                held: false,
                created_at: 1,
            },
        )
        .expect("queue");
        let (recovered, _) = watch::channel(None);
        let mut writer = Writer::new(
            Arc::new(Shared::new(WriterConfig::STANDARD)),
            store,
            owner,
            "dev".into(),
            super::super::files::Files::new(&profile),
            recovered,
        );
        writer.mode = Mode::Damaged;
        let load = writer.load_chat(&chat.id).expect("load").expect("the chat");
        assert_eq!(load.queue.len(), 1);
        assert!(load.queue[0].held, "held although recovery could not run");
        let stored =
            rows::queued(writer.store.as_ref().expect("store").conn(), &chat.id).expect("queued");
        assert!(!stored[0].held, "recovery did not run");
    }
}
